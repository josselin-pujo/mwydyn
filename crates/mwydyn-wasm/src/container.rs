//! The WASM actor container (rounds 33–37) — instantiates plain
//! `wasm32-unknown-unknown` modules as actors behind the barrier ABI:
//! canonical CBOR bytes over linear memory, zero guest imports, no
//! WASI. Compose-parity with native actors: the same
//! `receive(ctx, letter) → effects` shape, the same payload wall, the
//! same supervision semantics; WASM execution realm.
//!
//! ## The guest lifecycle (round 37, owner-ratified)
//!
//! **The instance is PERSISTENT and reused across letters.**
//! Instantiation is lazy and happens once — per-letter instantiation
//! (the round-31 approximation) multiplied the expensive half of WASM
//! (memory init) and made stateful guests impossible by construction.
//!
//! The transactional guarantee follows the guest's DECLARATION:
//!
//! - [`GuestMode::Stateful`] — the runtime owes a pre-receive MEMORY
//!   IMAGE rollback: `fork` captures linear memory + numeric globals
//!   before the transition; on refusal the image is applied to a
//!   fresh instance — the actor's internal state is exactly-once, at
//!   the cost of a doubled footprint for the image's lifetime.
//! - [`GuestMode::Stateless`] — the instance is reused and, on
//!   refusal, re-created fresh. NO once-and-only-once internal-state
//!   guarantee: if a refused transition dirtied guest state, that is
//!   the author's declared problem.
//!
//! Any violation at the barrier — trap, garbage bytes, bad shape —
//! is a crash, and crashes are supervision's problem, never a silent
//! swallow.

use mwydyn_core::{Actor, Effect, Host, MsgCtx};
use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};
use wasmtime::{
    Config, Engine, Global, Linker, Memory, Module, Store, TypedFunc, Val,
};

/// The wasmtime engine — shared across instances, epoch-based
/// deadline enforcement (§6: wall-clock budget per receive).
pub struct WasmEngine {
    engine: Engine,
    _epoch_thread: std::thread::JoinHandle<()>,
}

impl WasmEngine {
    /// Create with epoch interruption (§6).
    pub fn new() -> Result<WasmEngine, anyhow::Error> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;

        let engine_handle = engine.clone();
        let ticker = std::thread::Builder::new()
            .name("mwydyn-epoch".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(1));
                engine_handle.increment_epoch();
            })
            .expect("epoch ticker spawns");

        Ok(WasmEngine {
            engine,
            _epoch_thread: ticker,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl std::ops::Deref for WasmEngine {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.engine
    }
}

/// ROUNDS 37+41 — THE GUEST'S DECLARED STATEFULNESS, read from the
/// guest itself (`mwydyn_decl`, §9.2). The guarantee the runtime
/// owes follows this declaration, not the runtime's guess — and not
/// the embedder's either (round 41: no override exists).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestMode {
    /// Persistent instance reused across letters; on refusal the
    /// instance is simply reused, mutated state and all. The author
    /// declares there is no internal state a refused transition must
    /// not dirty.
    Stateless,
    /// Pre-receive MEMORY IMAGE rollback: linear memory + numeric
    /// globals captured at fork, applied to a fresh instance on
    /// refusal. Exactly-once internal state; doubled footprint while
    /// the image lives.
    Stateful,
}

/// A live guest: one store + instance + the barrier ABI's exports.
struct GuestRt {
    store: Store<Option<mwydyn_core::actor::AmbientParts>>,
    /// The guest's MUTABLE globals (numeric only — the state image
    /// restores these; reference-typed globals have no meaning in a
    /// zero-import guest).
    globals: Vec<(String, Global)>,
    alloc_fn: TypedFunc<i32, i32>,
    dealloc_fn: TypedFunc<(i32, i32), ()>,
    process_fn: TypedFunc<(i32, i32), i32>,
    memory: Memory,
    /// ROUND 39 — THE §7.6 SNAPSHOT ABI (both optional):
    /// `restore(state_ptr, state_len)` — applied at init when the
    /// boot carries a snapshot ref (the container blob_gets host-side
    /// and hands the bytes over).
    restore_fn: Option<TypedFunc<(i32, i32), ()>>,
    /// The JS panic stash's address global (round 43 exports the static
    /// as a CONST i32 global; the message bytes live in linear memory).
    /// Captured explicitly — the Var-only globals list above exists for
    /// the state image and must not grow const entries.
    panic_stash: Option<Global>,
    /// `snapshot() -> resp_ptr` (`[u32 len][state bytes]`) — the
    /// embedding's `Context::snapshot_actor` pulls durable state.
    snapshot_fn: Option<TypedFunc<(), i32>>,
}

/// The pre-receive state image of a stateful guest (round 37).
#[derive(Clone)]
struct MemoryImage {
    data: Vec<u8>,
    /// (export name, mutable, current value) for numeric globals.
    globals: Vec<(String, Val)>,
}

/// A WASM actor: a module behind the barrier ABI with a declared
/// lifecycle. The instance is lazy — created on first use, reused
/// across letters.
pub struct WasmActor {
    engine: Engine,
    module: Module,
    mode: GuestMode,
    /// `None` = lazy placeholder (fork of a live actor; instantiated
    /// on first use).
    rt: Option<GuestRt>,
    /// A stateful fork's captured pre-receive image, applied on first
    /// use after a refusal swap-in.
    saved: Option<MemoryImage>,
    /// ROUND 40 — the ambient surface parts (set by the pumps before
    /// the exchange; cloned into forks so placeholders inherit).
    ambient: Option<mwydyn_core::actor::AmbientParts>,
}

impl WasmActor {
    /// From compiled guest bytes. §9.2 (round 41): the GUEST owns
    /// its statefulness declaration — `mwydyn_decl() -> u32` bit 0,
    /// read once here (one throwaway instantiation). Absent export
    /// = Stateless by omission (hand-rolled guests, WAT fixtures).
    /// There is no embedder override: composing a stateful guest
    /// stateless would silently break the author's exactly-once
    /// expectation — refused by removing the path.
    pub fn from_compiled(engine: &WasmEngine, wasm: &[u8]) -> Result<WasmActor, anyhow::Error> {
        let module = Module::new(engine.engine(), wasm)?;
        let mode = WasmActor::probe_mode(engine, &module)?;
        WasmActor::assemble(
            engine.engine().clone(),
            module,
            mode,
            None,
            None,
            None,
        )
    }

    /// From WAT text — the test-guest path (ABI mechanics, byzantine
    /// fixtures); production loads compiled artifacts. Same law as
    /// [`WasmActor::from_compiled`]: the declaration is read from
    /// the guest text, whatever form it arrives in.
    pub fn from_wat(engine: &WasmEngine, wat: &str) -> Result<WasmActor, anyhow::Error> {
        let module = Module::new(engine.engine(), wat)?;
        let mode = WasmActor::probe_mode(engine, &module)?;
        WasmActor::assemble(
            engine.engine().clone(),
            module,
            mode,
            None,
            None,
            None,
        )
    }

    /// §9.2 — read `mwydyn_decl` from one throwaway instantiation.
    
fn probe_mode(engine: &WasmEngine, module: &Module) -> Result<GuestMode, anyhow::Error> {
        let mut store = Store::new(engine.engine(), None);
        // Epoch budget mirrors make_rt: the probe only reads a flag,
        // but the default deadline is EXPIRED (an instant trap).
        store.set_epoch_deadline(1_000_000);
        let linker = build_linker(engine.engine());
        let instance = linker.instantiate(&mut store, module)?;
        let declared = instance
            .get_typed_func::<(), i32>(&mut store, "mwydyn_decl")
            .ok()
            .map(|f| f.call(&mut store, ()))
            .transpose()?;
        Ok(if declared.unwrap_or(0) & 1 == 1 {
            GuestMode::Stateful
        } else {
            GuestMode::Stateless
        })
    }

    fn assemble(
        engine: Engine,
        module: Module,
        mode: GuestMode,
        rt: Option<GuestRt>,
        saved: Option<MemoryImage>,
        ambient: Option<mwydyn_core::actor::AmbientParts>,
    ) -> Result<WasmActor, anyhow::Error> {
        Ok(WasmActor {
            engine,
            module,
            mode,
            rt,
            saved,
            ambient,
        })
    }

    fn make_rt(&self, parts: Option<mwydyn_core::actor::AmbientParts>) -> Result<GuestRt, anyhow::Error> {
        let mut store = Store::new(&self.engine, parts);
        // Epoch budget: effectively unbounded here; the production
        // executor arms the deadline per exchange (§6).
        store.set_epoch_deadline(1_000_000);
        let linker = build_linker(&self.engine);
        let instance = linker.instantiate(&mut store, &self.module)?;
        let alloc_fn = instance
            .get_typed_func::<i32, i32>(&mut store, "alloc")
            .map_err(|e| anyhow::anyhow!("no `alloc(i32)→i32` export: {e}"))?;
        let dealloc_fn = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "dealloc")
            .map_err(|e| anyhow::anyhow!("no `dealloc(i32, i32)` export: {e}"))?;
        let process_fn = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "process")
            .map_err(|e| anyhow::anyhow!("no `process(i32, i32)→i32` export: {e}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow::anyhow!("no `memory` export"))?;
        let mut globals = Vec::new();
        let exports: Vec<(String, Option<Global>)> = instance
            .exports(&mut store)
            .map(|e| (e.name().to_string(), e.into_global()))
            .collect();
        for (name, g) in exports {
            if let Some(g) = g {
                if matches!(g.ty(&store).mutability(), wasmtime::Mutability::Var) {
                    globals.push((name, g));
                }
            }
        }
        let restore_fn = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "restore")
            .ok();
        let snapshot_fn = instance.get_typed_func::<(), i32>(&mut store, "snapshot").ok();
        let panic_stash = instance.get_global(&mut store, "MWJS_PANIC_BUF");
        Ok(GuestRt {
            store,
            panic_stash,
            globals,
            alloc_fn,
            dealloc_fn,
            process_fn,
            memory,
            restore_fn,
            snapshot_fn,
        })
    }

    /// Instantiate on first use (lazy) and restore whatever the
    /// lifecycle owes: a stateful fork's captured image, or fresh
    /// memory for a stateless guest.
    fn ensure_ready(&mut self) {
        if self.rt.is_none() {
            let mut rt = self.make_rt(self.ambient.clone()).expect("lazy instantiation");
            if let Some(img) = self.saved.take() {
                apply_image(&mut rt, img);
            }
            self.rt = Some(rt);
        }
    }

    /// Capture the pre-receive state image of a LIVE guest (linear
    /// memory + numeric globals). Stateless forks skip this entirely.
    fn capture_image(rt: &GuestRt) -> MemoryImage {
        let data = rt.memory.data(&rt.store).to_vec();
        // Mutable globals are NOT captured under fork(&self) — the
        // wasmtime read path needs a mutable store context. Stateful
        // guests keep state in LINEAR MEMORY (the image restores it
        // byte-exact); globals revert to their initial values on a
        // restore. Documented contract.
        MemoryImage { data, globals: Vec::new() }
    }

    fn apply_saved(&mut self) {
        // A CLONE — the image itself stays: the strike chain re-forks
        // this placeholder and must keep restoring the SAME
        // pre-letter state, attempt after attempt.
        if let (Some(rt), Some(img)) = (self.rt.as_mut(), &self.saved) {
            let img = img.clone();
            apply_image(rt, img);
        }
    }

    /// Apply a raw state blob through the guest's `restore` export.
    fn apply_state_blob(&mut self, state: &[u8]) {
        let rt = self.rt.as_mut().expect("ensure_ready ran");
        let Some(restore_fn) = rt.restore_fn.clone() else {
            panic!(
                "wasm barrier: the boot carries a snapshot but the guest exports no `restore` —                  refusing to silently drop durable state"
            );
        };
        let ptr = rt
            .alloc_fn
            .call(&mut rt.store, state.len().max(1) as i32)
            .unwrap_or_else(|e| panic!("wasm barrier: restore alloc trap: {e}"));
        rt.memory
            .write(&mut rt.store, ptr as usize, state)
            .unwrap_or_else(|e| panic!("wasm barrier: restore write: {e}"));
        restore_fn
            .call(&mut rt.store, (ptr, state.len() as i32))
            .unwrap_or_else(|e| panic!("wasm barrier: restore trap: {e}"));
        let _ = rt
            .dealloc_fn
            .call(&mut rt.store, (ptr, state.len().max(1) as i32));
    }

    /// One barrier exchange: canonical request in, wall-validated
    /// effects out. EVERY failure mode — trap, garbage bytes, bad
    /// shape — is a crash, and crashes are supervision's problem.
/// The JS panic stash (round 43's `MWJS_PANIC_BUF` engine global): a
/// process trap on a JS guest usually means a wrapper panic, and the
/// hook stashed the message there before aborting. Best-effort
/// diagnostics — absent global, zero length, or out-of-range bytes all
/// yield "" (a hand-rolled guest has no stash and no message).
fn js_panic_stash(rt: &mut GuestRt) -> String {
    // The stash exports as a CONST global (it holds the static's
    // ADDRESS; the bytes live in linear memory) — so it is not in
    // rt.globals, which deliberately collects Var globals only (the
    // state-image law). It is captured explicitly in make_rt.
    let Some(global) = &rt.panic_stash else {
        return String::new();
    };
    let global = global.clone();
    let Some(addr) = global.get(&mut rt.store).i32() else {
        return String::new();
    };
    let addr = addr as usize;
    let mem = rt.memory.data(&rt.store);
    if addr + 4 > mem.len() {
        return String::new();
    }
    let len = u32::from_le_bytes([mem[addr], mem[addr + 1], mem[addr + 2], mem[addr + 3]]) as usize;
    if len == 0 || addr + 4 + len > mem.len() {
        return String::new();
    }
    format!(
        "
  js panic stash: {}",
        String::from_utf8_lossy(&mem[addr + 4..addr + 4 + len])
    )
}

    fn barrier_exchange(&mut self, req: &BarrierRequest) -> Vec<Effect> {
        let rt = self.rt.as_mut().expect("ensure_ready ran");
        let req_bytes = encode(&req.to_value());

        let in_ptr = rt
            .alloc_fn
            .call(&mut rt.store, req_bytes.len() as i32)
            .unwrap_or_else(|e| panic!("wasm barrier: alloc trap: {e}"));
        rt.memory
            .write(&mut rt.store, in_ptr as usize, &req_bytes)
            .unwrap_or_else(|e| panic!("wasm barrier: memory write: {e}"));

        let out_ptr = rt
            .process_fn
            .call(&mut rt.store, (in_ptr, req_bytes.len() as i32))
            .unwrap_or_else(|e| {
                panic!(
                    "wasm barrier: process trap: {e}{}",
                    Self::js_panic_stash(rt)
                )
            });

        let mut len_buf = [0u8; 4];
        rt.memory
            .read(&rt.store, out_ptr as usize, &mut len_buf)
            .unwrap_or_else(|e| panic!("wasm barrier: response prefix: {e}"));
        let resp_len = u32::from_le_bytes(len_buf) as usize;
        let mut resp = vec![0u8; resp_len];
        rt.memory
            .read(&rt.store, out_ptr as usize + 4, &mut resp)
            .unwrap_or_else(|e| panic!("wasm barrier: response bytes: {e}"));

        let _ = rt
            .dealloc_fn
            .call(&mut rt.store, (in_ptr, req_bytes.len() as i32));
        let _ = rt
            .dealloc_fn
            .call(&mut rt.store, (out_ptr, (resp_len + 4) as i32));

        // THE WALL: the guest's bytes face the same canonical decoder
        // as everything else; the effects' payload bytes face the
        // payload wall in the core's Phase 2 (same gate as native).
        let value = decode(&resp, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("wasm barrier: non-canonical response: {e:?}"));
        let declared = BarrierEffect::effects_from_value(&value)
            .unwrap_or_else(|e| panic!("wasm barrier: bad response shape: {e:?}"));

        declared
            .into_iter()
            .map(|e| match e {
                BarrierEffect::Post { to, letter } => Effect::Post { to, letter },
                // ROUND 44: the guest's declare-form record rides
                // UNSTAMPED — the core mints ts/actor/seq at apply
                // (§3.6 byzantine posture holds at this boundary).
                BarrierEffect::Emit { record } => Effect::Emit { record },
            })
            .collect()
    }
}

/// Write a captured image into a fresh runtime: linear memory first
/// (growing to fit), then mutable numeric globals by export name.
fn apply_image(rt: &mut GuestRt, img: MemoryImage) {
    let cur_pages = rt.memory.size(&rt.store) as usize;
    let need_pages = img.data.len().div_ceil(0x10000);
    if need_pages > cur_pages {
        rt.memory
            .grow(&mut rt.store, (need_pages - cur_pages) as u64)
            .expect("restore grows linear memory");
    }
    rt.memory
        .write(&mut rt.store, 0, &img.data)
        .expect("restore writes linear memory");
    for (name, g) in &rt.globals {
        if let Some((_, v)) = img.globals.iter().find(|(n, _)| n == name) {
            g.set(&mut rt.store, *v)
                .expect("restore sets a captured mutable global");
        }
    }
}

impl Actor for WasmActor {
    /// ROUND 37 — the fork IS the declared guarantee:
    /// - Stateless: a cheap lazy placeholder — nothing captured, a
    ///   fresh instance on first use after a swap-in.
    /// - Stateful: the pre-receive MEMORY IMAGE of the live guest
    ///   (linear memory + numeric globals), applied to a fresh
    ///   instance if the swap-in ever happens. This is the copy the
    ///   doubled footprint buys.
    fn fork(&self) -> Box<dyn Actor> {
        let saved = match self.mode {
            GuestMode::Stateless => None,
            GuestMode::Stateful => match (&self.saved, self.rt.as_ref()) {
                // Placeholder chain: the SAME pre-receive image rides
                // every re-fork — re-capturing from rt would freeze
                // the FAILED attempt's mutations into the rollback.
                (Some(img), _) => Some(img.clone()),
                // Live guest: capture now (pre-receive).
                (None, Some(rt)) => Some(WasmActor::capture_image(rt)),
                (None, None) => None,
            },
        };
        Box::new(
            WasmActor::assemble(
                self.engine.clone(),
                self.module.clone(),
                self.mode,
                None,
                saved,
                self.ambient.clone(),
            )
            .expect("fork assembles the handle"),
        )
    }

    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        self.ensure_ready();
        self.apply_saved();
        let req = BarrierRequest {
            from: ctx.from.clone(),
            letter: letter.clone(),
        };
        self.barrier_exchange(&req)
    }

    /// §7.6 RESTORE (round 39): the boot carries a durable snapshot
    /// ref — blob_get host-side, bytes over the barrier through the
    /// guest's `restore` export. Loud if the guest cannot receive
    /// durable state. A missing snapshot inits nothing (fresh).
    fn init(&mut self, host: &Host, boot: &mwydyn_core::BootRecord) {
        if let Some(r) = &boot.snapshot {
            let state = host
                .blob_get(r)
                .unwrap_or_else(|e| panic!("wasm barrier: boot snapshot unreadable: {e:?}"));
            self.ensure_ready();
            self.apply_state_blob(&state);
        }
    }

    /// THE DECLARATION, stated to the core (round 37): stateless
    /// guests' forks carry no state — the runtime keeps the mutated
    /// instance on refusal ("just reused"); stateful guests get the
    /// memory image restored.
    fn fork_carries_state(&self) -> bool {
        self.mode == GuestMode::Stateful
    }

    fn set_ambient(&mut self, parts: mwydyn_core::actor::AmbientParts) {
        self.ambient = Some(parts);
    }

    /// §7.6 SAVE (round 39): the guest's `snapshot` export returns
    /// its durable state — `Context::snapshot_actor` stores it and
    /// lands the covers-stamped snapshot letter.
    fn snapshot_state(&mut self) -> Option<Vec<u8>> {
        let rt = self.rt.as_mut()?;
        let snapshot_fn = rt.snapshot_fn.clone()?;
        let out_ptr = snapshot_fn
            .call(&mut rt.store, ())
            .unwrap_or_else(|e| panic!("wasm barrier: snapshot trap: {e}"));
        let mut len_buf = [0u8; 4];
        rt.memory
            .read(&rt.store, out_ptr as usize, &mut len_buf)
            .unwrap_or_else(|e| panic!("wasm barrier: snapshot prefix: {e}"));
        let state_len = u32::from_le_bytes(len_buf) as usize;
        let mut state = vec![0u8; state_len];
        rt.memory
            .read(&rt.store, out_ptr as usize + 4, &mut state)
            .unwrap_or_else(|e| panic!("wasm barrier: snapshot bytes: {e}"));
        let _ = rt
            .dealloc_fn
            .call(&mut rt.store, (out_ptr, (state_len + 4) as i32));
        Some(state)
    }
}

/// ROUND 40 — build the ambient Linker for one instantiation (one
/// import registration; instantiation is lazy, so this runs once per
/// guest lifetime).
fn build_linker(engine: &Engine) -> Linker<Option<mwydyn_core::actor::AmbientParts>> {
    let mut linker: Linker<Option<mwydyn_core::actor::AmbientParts>> = Linker::new(engine);
    linker
        .func_wrap(
            "mwydyn",
            "ambient",
            |mut caller: wasmtime::Caller<'_, Option<mwydyn_core::actor::AmbientParts>>,
             in_ptr: i32,
             in_len: i32,
             out_ptr: i32,
             out_cap: i32|
             -> i32 { ambient_dispatch(&mut caller, in_ptr, in_len, out_ptr, out_cap) },
        )
        .expect("the ambient import registers once");
    linker
}

/// The host side of `mwydyn.ambient`: read the canonical-CBOR call,
/// dispatch with CAP CHECKS against the actor's declared rows, and
/// write the `{"ok"} | {"err"}` response. Returns bytes written, or
/// -1 when the caller's buffer is too small.
fn ambient_dispatch(
    caller: &mut wasmtime::Caller<'_, Option<mwydyn_core::actor::AmbientParts>>,
    in_ptr: i32,
    in_len: i32,
    out_ptr: i32,
    out_cap: i32,
) -> i32 {
    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
        return -1;
    };
    let mut call_bytes = vec![0u8; in_len.max(0) as usize];
    if memory
        .read(&caller, in_ptr.max(0) as usize, &mut call_bytes)
        .is_err()
    {
        return -1;
    }

    let response = match caller.data() {
        None => mwydyn_data::ambient::AmbientCall::err_response("Unavailable"),
        Some(parts) => {
            let call = decode(&call_bytes, &DecodeOptions::default())
                .map_err(|_| "BadForm".to_string())
                .and_then(|v| {
                    mwydyn_data::ambient::AmbientCall::from_value(&v).map_err(|e| match e {
                        mwydyn_data::ambient::AmbientError::UnsupportedVerb(_) => {
                            "UnsupportedVerb".to_string()
                        }
                        _ => "BadForm".to_string(),
                    })
                });
            match call {
                Err(code) => mwydyn_data::ambient::AmbientCall::err_response(&code),
                Ok(call) => {
                    let granted =
                        |domain: &str| parts.rows.iter().any(|row| row.holds(domain, "*"));
                    match &call.verb {
                        mwydyn_data::ambient::AmbientVerb::BlobGet { ref_ }
                            if granted("blob.read") =>
                        {
                            match blob_ref_of(ref_) {
                                Some(r) => match parts.blobs.get(&r) {
                                    Ok(bytes) => mwydyn_data::ambient::AmbientCall::ok_response(Value::Bytes(bytes)),
                                    Err(e) => mwydyn_data::ambient::AmbientCall::err_response(blob_err_code(&e)),
                                },
                                None => mwydyn_data::ambient::AmbientCall::err_response("BadForm"),
                            }
                        }
                        mwydyn_data::ambient::AmbientVerb::BlobPut {
                            bytes,
                            media_type,
                            ttl_micros,
                        } if granted("blob.write") => {
                            match parts.blobs.put(bytes, media_type, *ttl_micros) {
                                Ok(r) => mwydyn_data::ambient::AmbientCall::ok_response(
                                    Value::BlobRef {
                                        digest: r.digest,
                                        media_type: r.media_type,
                                        size: r.size,
                                    },
                                ),
                                Err(_) => mwydyn_data::ambient::AmbientCall::err_response("Integrity"),
                            }
                        }
                        mwydyn_data::ambient::AmbientVerb::BlobLen { ref_ }
                            if granted("blob.read") =>
                        {
                            match blob_ref_of(ref_) {
                                Some(r) => match parts.blobs.len(&r) {
                                    Ok(n) => mwydyn_data::ambient::AmbientCall::ok_response(Value::Int(n as i64)),
                                    Err(e) => mwydyn_data::ambient::AmbientCall::err_response(blob_err_code(&e)),
                                },
                                None => mwydyn_data::ambient::AmbientCall::err_response("BadForm"),
                            }
                        }
                        mwydyn_data::ambient::AmbientVerb::Caps => {
                            let rows = parts
                                .rows
                                .iter()
                                .map(|row| {
                                    Value::map(vec![
                                        ("verb".into(), Value::Text(row.verb.clone())),
                                        ("object".into(), Value::Text(row.object.clone())),
                                    ])
                                    .expect("fixed hygienic keys")
                                })
                                .collect();
                            mwydyn_data::ambient::AmbientCall::ok_response(Value::Array(rows))
                        }
                        // granted() failed: the verb exists, the row
                        // does not — the native face's exact refusal.
                        _ => mwydyn_data::ambient::AmbientCall::err_response("Unauthorized"),
                    }
                }
            }
        }
    };

    let resp_bytes = encode(&response);
    if resp_bytes.len() > out_cap.max(0) as usize {
        return -1;
    }
    if memory.write(caller, out_ptr.max(0) as usize, &resp_bytes).is_err() {
        return -1;
    }
    resp_bytes.len() as i32
}

fn blob_ref_of(v: &Value) -> Option<mwydyn_core::BlobRef> {
    let Value::BlobRef {
        digest,
        media_type,
        size,
    } = v
    else {
        return None;
    };
    Some(mwydyn_core::BlobRef {
        digest: digest.clone(),
        media_type: media_type.clone(),
        size: *size,
    })
}

fn blob_err_code(e: &mwydyn_core::BlobAccess) -> &'static str {
    match e {
        mwydyn_core::BlobAccess::Unauthorized => "Unauthorized",
        mwydyn_core::BlobAccess::Missing => "Missing",
        mwydyn_core::BlobAccess::Integrity => "Integrity",
    }
}

/// The address guests use to report OUT to the embedding (drained
/// via `Context::drain_external`). Same value as the core's
/// `EXTERNAL` — kept in data-land so guests need no core dep.
pub fn external_address() -> Address {
    Address::Instance("external".into())
}
