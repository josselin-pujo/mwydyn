//! THE WRAPPER (round 43, wasm-only): boots quickjs-ng lazily,
//! evaluates prelude + script, and declares the §9 exports — with
//! mwydyn_decl reading the SCRIPT's registration (probe-time boot:
//! the declaration is the author's, whatever engine it takes).
//! Panic = strike (the SDK's exports catch + abort inside the
//! boundary).

#![cfg(target_arch = "wasm32")]

use crate::convert::{js_to_value, value_to_js};
use crate::ffi::{self, JSValue};
use crate::Engine;
use mwydyn_data::telemetry::TelemetryRecord;
use mwydyn_data::{address_from_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_guest::{Actor, Cx, Durable, Effect};

/// The script slot: `mwydyn-js build` appends an active data segment
/// here (script bytes + NUL). 4 MiB cap; BSS costs nothing until
/// written.
///
/// MUST be `static mut` (raw-pointer reads): the tool's data segment
/// initializes it at instantiation — but an IMMUTABLE static lets
/// LLVM fold reads to the all-zero initializer at opt-level >= 1
/// (the "no script in the slot" release-only crash, found the hard
/// way: the bytes were in memory, the fold never looked).
static mut MWJS_SCRIPT: [u8; 4 << 20] = [0; 4 << 20];

/// THE BARRIER EXCHANGE: decode → receive → encode. Same law as the
/// SDK's process_impl (strikes panic), local because the ambient
/// trampoline needs the singleton actor.
///
/// # Safety
/// `req_ptr` must point to `req_len` readable bytes; the build tool
/// guarantees the script segment lands inside the slot.
unsafe fn barrier_exchange(req_ptr: *const u8, req_len: usize) -> *mut u8 {
    use mwydyn_data::barrier::BarrierRequest;
    // SAFETY: caller staged the request.
    let bytes = unsafe { std::slice::from_raw_parts(req_ptr, req_len) };
    let value = decode(bytes, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("mwydyn-js: request decode: {e:?}"));
    let req = BarrierRequest::from_value(&value)
        .unwrap_or_else(|e| panic!("mwydyn-js: request shape: {e:?}"));
    let mwydyn_data::barrier::BarrierRequest { from, letter } = req;
    let effects = with_actor(|actor| {
        let cx = Cx::new(&from);
        actor.receive(letter, &cx)
    });
    let response = Value::Array(effects.iter().map(|e| e.to_value()).collect());
    mwydyn_guest::respond(&encode(&response))
}

#[no_mangle]
#[no_mangle]
pub extern "C" fn mwydyn_script_slot() -> i32 {
    // SAFETY: address computation only.
    std::ptr::addr_of_mut!(MWJS_SCRIPT) as i32
}

fn script_source() -> &'static [u8] {
    // SAFETY: the data segment wrote the script before any call;
    // single-threaded wasm. The raw read defeats the zero-fold.
    unsafe {
        let slot = std::ptr::addr_of_mut!(MWJS_SCRIPT);
        let bytes = &*slot;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(0);
        &bytes[..end]
    }
}

/// The whole actor: a lazily-booted engine + the script's
/// registration.
pub struct JsActor {
    engine: Option<Engine>,
}

impl JsActor {
    pub fn new() -> JsActor {
        JsActor { engine: None }
    }

    fn boot(&mut self) {
        if self.engine.is_some() {
            return;
        }
        let mut e = Engine::new().expect("mwydyn-js: engine boots");
        e.eval(include_str!("prelude.js"), "mwydyn-prelude.js")
            .expect("mwydyn-js: prelude (a runtime bug — report it)");
        let script = script_source();
        assert!(!script.is_empty(), "mwydyn-js: no script in the slot — build with mwydyn-js");
        let script = String::from_utf8_lossy(script);
        e.eval(&script, "guest.js").expect("mwydyn-js: script error (the author's — see strike)");
        register_ambient(&mut e);
        self.engine = Some(e);
    }

    fn with_engine<R>(&mut self, f: impl FnOnce(&mut Engine) -> R) -> R {
        self.boot();
        f(self.engine.as_mut().expect("booted"))
    }
}

/// Register the `__mwAmbient` intrinsic: JS object → CBOR → the §9.4
/// import → back to JS (refusals ride `{__refused: code}`).
unsafe extern "C" fn ambient_trampoline(
    _ctx: *mut std::ffi::c_void,
    _this: JSValue,
    argc: i32,
    argv: *mut JSValue,
) -> JSValue {
    unsafe {
        let actor = {
            let slot = &mut *std::ptr::addr_of_mut!(JS_ACTOR_SINGLETON);
            slot.as_mut().expect("engine live during ambient")
        };
        let engine = actor.engine.as_mut().expect("engine live during ambient");
        let _ = argc;
        let req = std::ptr::read_unaligned(argv);
        // argv carries JSValueConst — BORROWED references quickjs frees
        // itself at frame teardown. Freeing one here double-frees (found
        // by platformc: the JS guest's first cx.blobGet crashed in
        // js_free_value_rt — the JS x ambient combination had no test
        // until the round-47 regression pin).
        let request = match js_to_value(engine, req) {
            Ok(v) => v,
            Err(e) => return throw_js(engine, &e.message),
        };
        match mwydyn_guest::ambient_call(request) {
            Ok(v) => value_to_js(engine, &v),
            Err(code) => {
                // the prelude turns this into MwydynRefusal(code)
                let obj = ffi::JS_NewObject(engine.ctx());
                let val = ffi::JS_NewStringLen(engine.ctx(), code.as_ptr(), code.len());
                ffi::JS_SetPropertyStr(engine.ctx(), obj, b"__refused\0".as_ptr(), val);
                obj
            }
        }
    }
}

unsafe fn throw_js(e: &mut Engine, msg: &str) -> JSValue {
    unsafe {
        let s = ffi::JS_NewStringLen(e.ctx(), msg.as_ptr(), msg.len());
        let exc = ffi::JS_Throw(e.ctx(), s);
        exc
    }
}

fn register_ambient(e: &mut Engine) {
    unsafe {
        let f = ffi::JS_NewCFunction2(
            e.ctx(),
            Some(ambient_trampoline),
            c"__mwAmbient".as_ptr() as *const u8,
            1,
            0, // JS_CFUNC_generic
            0,
        );
        let global = ffi::JS_GetGlobalObject(e.ctx());
        ffi::JS_SetPropertyStr(e.ctx(), global, c"__mwAmbient".as_ptr() as *const u8, f);
        ffi::JS_FreeValue(e.ctx(), global);
    }
}

/// The one live JsActor — ambient trampolines reach it through here.
/// SAFETY-singleton: wasm guests are single-threaded (§9.1).
static mut JS_ACTOR_SINGLETON: Option<JsActor> = None;

fn with_actor<R>(f: impl FnOnce(&mut JsActor) -> R) -> R {
    // SAFETY: single-threaded wasm; the static owns the actor.
    unsafe {
        let slot = &mut *std::ptr::addr_of_mut!(JS_ACTOR_SINGLETON);
        if slot.is_none() {
            *slot = Some(JsActor::new());
        }
        f(slot.as_mut().expect("singleton"))
    }
}

fn js_addr_to_rust(e: &mut Engine, v: JSValue) -> Address {
    let value = js_to_value(e, v).expect("address value");
    address_from_value(&value).expect("address shape")
}

fn convert_effects(e: &mut Engine, arr: JSValue) -> Vec<Effect> {
    unsafe {
        let ctx = e.ctx();
        let mut out = Vec::new();
        let len_prop = ffi::JS_GetPropertyStr(ctx, arr, b"length\0".as_ptr());
        let mut len = 0i32;
        ffi::JS_ToInt32(ctx, &mut len, len_prop);
        ffi::JS_FreeValue(ctx, len_prop);
        for i in 0..len as u32 {
            let item = ffi::JS_GetPropertyUint32(ctx, arr, i);
            if item.tag() == ffi::JS_TAG_NULL || item.tag() == ffi::JS_TAG_UNDEFINED {
                ffi::JS_FreeValue(ctx, item);
                continue; // e.g. a missed replyVia the author left in
            }
            let post = ffi::JS_GetPropertyStr(ctx, item, b"post\0".as_ptr());
            if post.tag() == ffi::JS_TAG_OBJECT {
                let to_v = ffi::JS_GetPropertyStr(ctx, post, b"to\0".as_ptr());
                let letter_v = ffi::JS_GetPropertyStr(ctx, post, b"letter\0".as_ptr());
                let to = js_addr_to_rust(e, to_v);
                ffi::JS_FreeValue(ctx, to_v);
                let kind = string_prop(e, letter_v, "kind");
                let payload_v = ffi::JS_GetPropertyStr(ctx, letter_v, b"payload\0".as_ptr());
                let payload = js_to_value(e, payload_v).expect("payload converts");
                ffi::JS_FreeValue(ctx, payload_v);
                let cause = opt_string_prop(e, letter_v, "cause");
                let correlation = opt_string_prop(e, letter_v, "correlation");
                let auth_v = ffi::JS_GetPropertyStr(ctx, letter_v, b"authority\0".as_ptr());
                let authority: Vec<String> = if auth_v.tag() == ffi::JS_TAG_OBJECT {
                    let items = js_to_value(e, auth_v).unwrap_or(Value::Array(vec![]));
                    ffi::JS_FreeValue(ctx, auth_v);
                    match items {
                        Value::Array(xs) => xs
                            .into_iter()
                            .filter_map(|x| match x {
                                Value::Text(t) => Some(t),
                                _ => None,
                            })
                            .collect(),
                        _ => vec![],
                    }
                } else {
                    ffi::JS_FreeValue(ctx, auth_v);
                    vec![]
                };
                ffi::JS_FreeValue(ctx, letter_v);
                ffi::JS_FreeValue(ctx, post);
                ffi::JS_FreeValue(ctx, item);
                let letter = Letter::new(&kind, encode(&payload), cause, correlation, authority)
                    .expect("author letter");
                out.push(Effect::Post { to, letter });
                continue;
            }
            ffi::JS_FreeValue(ctx, post);
            let emit = ffi::JS_GetPropertyStr(ctx, item, b"emit\0".as_ptr());
            if emit.tag() == ffi::JS_TAG_OBJECT {
                let record_v = js_to_value(e, emit).expect("emit record converts");
                ffi::JS_FreeValue(ctx, emit);
                ffi::JS_FreeValue(ctx, item);
                let record = TelemetryRecord::declare_from_value(&record_v).unwrap_or_else(|e| {
                    panic!("emit declare failed ({e:?}) — the value as converted: {record_v:?}")
                });
                out.push(Effect::Emit { record });
                continue;
            }
            ffi::JS_FreeValue(ctx, emit);
            ffi::JS_FreeValue(ctx, item);
            panic!("mwydyn-js: effect must be Effect.post(...)/log(...)/metric(...)");
        }
        out
    }
}

fn string_prop(e: &mut Engine, obj: JSValue, k: &str) -> String {
    opt_string_prop(e, obj, k).unwrap_or_default()
}

/// THE NUL WALL (again): property names cross as C strings — a Rust
/// `&str` key without its terminator lands on adjacent bytes (found
/// the hard way: `letter.kind` read back undefined).
fn opt_string_prop(e: &mut Engine, obj: JSValue, k: &str) -> Option<String> {
    let key = std::ffi::CString::new(k).expect("prop key without NUL");
    unsafe {
        let v = ffi::JS_GetPropertyStr(e.ctx(), obj, key.as_ptr() as *const u8);
        let s = if v.tag() == ffi::JS_TAG_STRING {
            Some(e.js_to_string(v))
        } else {
            None
        };
        ffi::JS_FreeValue(e.ctx(), v);
        s
    }
}

impl Actor for JsActor {
    fn receive(&mut self, letter: Letter, cx: &Cx) -> Vec<Effect> {
        self.with_engine(|e| unsafe {
            let ctx = e.ctx();
            // letter → JS {kind, payload(decoded), cause, correlation, authority}
            let payload_value = decode(&letter.payload, &DecodeOptions::default())
                .unwrap_or(Value::Null);
            let lobj = ffi::JS_NewObject(ctx);
            set_str_prop(e, lobj, "kind", &letter.kind);
            let pv = value_to_js(e, &payload_value);
            ffi::JS_SetPropertyStr(ctx, lobj, b"payload\0".as_ptr(), pv);
            if let Some(c) = &letter.cause {
                set_str_prop(e, lobj, "cause", c);
            }
            if let Some(c) = &letter.correlation {
                set_str_prop(e, lobj, "correlation", c);
            }
            let auth = ffi::JS_NewArray(ctx);
            for (i, a) in letter.authority.iter().enumerate() {
                let s = ffi::JS_NewStringLen(ctx, a.as_ptr(), a.len());
                ffi::JS_SetPropertyUint32(ctx, auth, i as u32, s);
            }
            ffi::JS_SetPropertyStr(ctx, lobj, b"authority\0".as_ptr(), auth);

            // cx → JS (kept alive through the call)
            let sender = mwydyn_data::address_to_value(cx.sender());
            let sender_js = value_to_js(e, &sender);
            let global = ffi::JS_GetGlobalObject(ctx);
            let mwd = ffi::JS_GetPropertyStr(ctx, global, b"mwydyn ".as_ptr());
            let mkcx = ffi::JS_GetPropertyStr(ctx, mwd, b"__mkCx ".as_ptr());
            let mut cx_args = [sender_js];
            let cx_obj = ffi::JS_Call(ctx, mkcx, mwd, 1, cx_args.as_mut_ptr());

            // actor = mwydyn.__actor(); actor.receive(letter, cx)
            let actor_fn = ffi::JS_GetPropertyStr(ctx, mwd, b"__actor ".as_ptr());
            let mut no_args = [];
            let actor = ffi::JS_Call(ctx, actor_fn, mwd, 0, no_args.as_mut_ptr());
            ffi::JS_FreeValue(ctx, actor_fn);
            let recv = ffi::JS_GetPropertyStr(ctx, actor, b"receive ".as_ptr());
            let mut args = [lobj, cx_obj];
            let result = ffi::JS_Call(ctx, recv, actor, 2, args.as_mut_ptr());
            ffi::JS_FreeValue(ctx, recv);
            ffi::JS_FreeValue(ctx, actor);
            ffi::JS_FreeValue(ctx, cx_obj);
            ffi::JS_FreeValue(ctx, mkcx);
            ffi::JS_FreeValue(ctx, mwd);
            ffi::JS_FreeValue(ctx, global);
            if result.is_exception() {
                let err = e.take_exception();
                panic!("mwydyn-js: receive threw — {err}"); // trap = strike
            }
            let effects = convert_effects(e, result);
            ffi::JS_FreeValue(ctx, result);
            effects
        })
    }
}

fn set_str_prop(e: &mut Engine, obj: JSValue, k: &str, s: &str) {
    let key = std::ffi::CString::new(k).expect("prop key without NUL");
    unsafe {
        let v = ffi::JS_NewStringLen(e.ctx(), s.as_ptr(), s.len());
        ffi::JS_SetPropertyStr(e.ctx(), obj, key.as_ptr() as *const u8, v);
    }
}

impl Durable for JsActor {
    fn snapshot(&self) -> Vec<u8> {
        // SAFETY: single-threaded wasm (the actor static is ours).
        let mut actor = unsafe { JS_ACTOR_SINGLETON.as_mut().expect("singleton") };
        actor.with_engine(|e| unsafe {
            let ctx = e.ctx();
            let global = ffi::JS_GetGlobalObject(ctx);
            let mwd = ffi::JS_GetPropertyStr(ctx, global, b"mwydyn\0".as_ptr());
            let dur = ffi::JS_GetPropertyStr(ctx, mwd, b"__durable\0".as_ptr());
            assert!(
                dur.tag() == ffi::JS_TAG_OBJECT,
                "snapshot on a non-durable script — durableEntry! is the declaration"
            );
            let snap = ffi::JS_GetPropertyStr(ctx, dur, b"snapshot\0".as_ptr());
            let mut argv = [];
            let result = ffi::JS_Call(ctx, snap, dur, 0, argv.as_mut_ptr());
            ffi::JS_FreeValue(ctx, snap);
            ffi::JS_FreeValue(ctx, dur);
            ffi::JS_FreeValue(ctx, mwd);
            ffi::JS_FreeValue(ctx, global);
            if result.is_exception() {
                let err = e.take_exception();
                panic!("mwydyn-js: snapshot threw — {err}");
            }
            let v = js_to_value(e, result).expect("snapshot value converts");
            ffi::JS_FreeValue(ctx, result);
            encode(&v) // state format is the actor's own; CBOR keeps it whole
        })
    }

    fn restore(&mut self, state: &[u8]) {
        self.with_engine(|e| {
            let value = decode(state, &DecodeOptions::default())
                .expect("mwydyn-js: snapshot bytes decode (they are ours)");
            unsafe {
                let ctx = e.ctx();
                let sv = value_to_js(e, &value);
                let global = ffi::JS_GetGlobalObject(ctx);
                let mwd = ffi::JS_GetPropertyStr(ctx, global, b"mwydyn\0".as_ptr());
                let dur = ffi::JS_GetPropertyStr(ctx, mwd, b"__durable\0".as_ptr());
                assert!(
                    dur.tag() == ffi::JS_TAG_OBJECT,
                    "restore on a non-durable script"
                );
                let res = ffi::JS_GetPropertyStr(ctx, dur, b"restore\0".as_ptr());
                let mut argv = [sv];
                let result = ffi::JS_Call(ctx, res, dur, 1, argv.as_mut_ptr());
                ffi::JS_FreeValue(ctx, res);
                ffi::JS_FreeValue(ctx, dur);
                ffi::JS_FreeValue(ctx, mwd);
                ffi::JS_FreeValue(ctx, global);
                if result.is_exception() {
                    let err = e.take_exception();
                    panic!("mwydyn-js: restore threw — {err}");
                }
                ffi::JS_FreeValue(ctx, result);
            }
        });
    }
}

// ---- the §9 exports (hand-rolled: the DECLARATION boots the probe
// instance to read the script's registration — no macro bakes it) ----

/// §9.2 — the script's own declaration, read by booting prelude +
/// script once (probe cost at composition, not per letter).
#[no_mangle]
pub extern "C" fn mwydyn_decl() -> u32 {
    crate::mwjs_install_panic_hook();
    with_actor(|a| {
        a.boot();
        a.with_engine(|e| unsafe {
            let ctx = e.ctx();
            let global = ffi::JS_GetGlobalObject(ctx);
            let mwd = ffi::JS_GetPropertyStr(ctx, global, b"mwydyn\0".as_ptr());
            let mode_fn = ffi::JS_GetPropertyStr(ctx, mwd, b"__mode\0".as_ptr());
            let mut argv = [];
            let mode = ffi::JS_Call(ctx, mode_fn, mwd, 0, argv.as_mut_ptr());
            ffi::JS_FreeValue(ctx, mode_fn);
            ffi::JS_FreeValue(ctx, mwd);
            ffi::JS_FreeValue(ctx, global);
            let mut m = 0i32;
            ffi::JS_ToInt32(ctx, &mut m, mode);
            ffi::JS_FreeValue(ctx, mode);
            (m & 1) as u32
        })
    })
}

#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    mwydyn_guest::alloc(len)
}

/// # Safety
/// See [`mwydyn_guest::dealloc`].
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    unsafe { mwydyn_guest::dealloc(ptr, len) }
}

/// # Safety
/// `req_ptr` must point to `req_len` readable bytes in guest linear
/// memory.
#[no_mangle]
pub unsafe extern "C" fn process(req_ptr: *const u8, req_len: usize) -> *mut u8 {
    // the ambient trampoline reaches the actor through the singleton
    crate::mwjs_install_panic_hook();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        barrier_exchange(req_ptr, req_len)
    })) {
        Ok(ptr) => ptr,
        Err(_) => std::process::abort(),
    }
}

/// # Safety
/// See [`mwydyn_guest::restore_impl`].
#[no_mangle]
pub unsafe extern "C" fn restore(ptr: *const u8, len: usize) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut actor = unsafe { JS_ACTOR_SINGLETON.as_mut().expect("singleton") };
        let state = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
        <JsActor as Durable>::restore(actor, &state);
    })) {
        Ok(()) => {}
        Err(_) => std::process::abort(),
    }
}

#[no_mangle]
pub extern "C" fn snapshot() -> *mut u8 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let actor = unsafe { JS_ACTOR_SINGLETON.as_mut().expect("singleton") };
        let state = Durable::snapshot(actor);
        mwydyn_guest::respond(&state)
    })) {
        Ok(ptr) => ptr,
        Err(_) => std::process::abort(),
    }
}
