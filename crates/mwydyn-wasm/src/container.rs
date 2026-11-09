//! The WASM actor container (round 33) — instantiates plain
//! `wasm32-unknown-unknown` modules as actors and drives the barrier
//! ABI per letter. Compose-parity with native actors: the same
//! `receive(ctx, letter) → effects` shape, the same payload wall,
//! the same supervision semantics; WASM execution realm.
//!
//! CBOR all the way (round 33): the barrier speaks canonical CBOR
//! bytes over linear memory — `alloc`/`dealloc`/`process(ptr, len)
//! → [u32 len][bytes]`. Request = [`BarrierRequest`]; response =
//! the canonical-CBOR effects array. Any violation is a guest crash
//! (panic → strike/dead-letter), never a silent swallow.

use mwydyn_core::{Actor, Effect, Host, MsgCtx};
use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter};
use wasmtime::{Config, Engine, Instance, Memory, Module, Store, TypedFunc};

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

/// A WASM actor: one module instance behind the barrier ABI.
pub struct WasmActor {
    engine: Engine,
    module: Module,
    store: Store<()>,
    alloc_fn: TypedFunc<i32, i32>,
    dealloc_fn: TypedFunc<(i32, i32), ()>,
    process_fn: TypedFunc<(i32, i32), i32>,
    memory: Memory,
}

impl WasmActor {
    /// From compiled guest bytes (the `scripts/build-guest.sh`
    /// artifact — plain wasm32-unknown-unknown, zero imports).
    pub fn from_compiled(
        engine: &WasmEngine,
        wasm: &[u8],
    ) -> Result<WasmActor, anyhow::Error> {
        let module = Module::new(engine.engine(), wasm)?;
        WasmActor::instantiate(engine.engine(), module)
    }

    /// From WAT text — the test-guest path (ABI mechanics, byzantine
    /// fixtures); production loads compiled artifacts.
    pub fn from_wat(engine: &WasmEngine, wat: &str) -> Result<WasmActor, anyhow::Error> {
        let module = Module::new(engine.engine(), wat)?;
        WasmActor::instantiate(engine.engine(), module)
    }

    fn instantiate(engine: &Engine, module: Module) -> Result<WasmActor, anyhow::Error> {
        let mut store = Store::new(engine, ());
        // Epoch budget: effectively unbounded here; the production
        // executor arms the deadline per exchange (§6).
        store.set_epoch_deadline(1_000_000);
        let instance = Instance::new(&mut store, &module, &[])?;
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
        Ok(WasmActor {
            engine: engine.clone(),
            module,
            store,
            alloc_fn,
            dealloc_fn,
            process_fn,
            memory,
        })
    }

    /// One barrier exchange: canonical request in, wall-validated
    /// effects out. EVERY failure mode — trap, garbage bytes, bad
    /// shape — is a crash, and crashes are supervision's problem.
    fn barrier_exchange(&mut self, req: &BarrierRequest) -> Vec<Effect> {
        let req_bytes = encode(&req.to_value());

        let in_ptr = self
            .alloc_fn
            .call(&mut self.store, req_bytes.len() as i32)
            .unwrap_or_else(|e| panic!("wasm barrier: alloc trap: {e}"));
        self.memory
            .write(&mut self.store, in_ptr as usize, &req_bytes)
            .unwrap_or_else(|e| panic!("wasm barrier: memory write: {e}"));

        let out_ptr = self
            .process_fn
            .call(&mut self.store, (in_ptr, req_bytes.len() as i32))
            .unwrap_or_else(|e| panic!("wasm barrier: process trap: {e}"));

        let mut len_buf = [0u8; 4];
        self.memory
            .read(&self.store, out_ptr as usize, &mut len_buf)
            .unwrap_or_else(|e| panic!("wasm barrier: response prefix: {e}"));
        let resp_len = u32::from_le_bytes(len_buf) as usize;
        let mut resp = vec![0u8; resp_len];
        self.memory
            .read(&self.store, out_ptr as usize + 4, &mut resp)
            .unwrap_or_else(|e| panic!("wasm barrier: response bytes: {e}"));

        let _ = self
            .dealloc_fn
            .call(&mut self.store, (in_ptr, req_bytes.len() as i32));
        let _ = self
            .dealloc_fn
            .call(&mut self.store, (out_ptr, (resp_len + 4) as i32));

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
            })
            .collect()
    }
}

impl Actor for WasmActor {
    /// Fork: re-instantiate from the same module (stateless
    /// approximation — §7.6 snapshots apply to stateful WASM).
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(
            WasmActor::instantiate(&self.engine, self.module.clone())
                .expect("fork re-instantiates the same module"),
        )
    }

    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        let req = BarrierRequest {
            from: ctx.from.clone(),
            letter: letter.clone(),
        };
        self.barrier_exchange(&req)
    }
}

/// The address guests use to report OUT to the embedding (drained
/// via `Context::drain_external`). Same value as the core's
/// `EXTERNAL` — kept in data-land so guests need no core dep.
pub fn external_address() -> Address {
    Address::Instance("external".into())
}
