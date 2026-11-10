//! # mwydyn-guest — write a plugin, not a runtime
//!
//! One dependency, one trait, one entry line. Everything else —
//! the WASM exports, the CBOR codec, the linear-memory scratch,
//! the capability-checked host calls — is this crate's problem.
//!
//! A whole plugin:
//!
//! ```ignore
//! use mwydyn_guest::prelude::*;
//!
//! struct Counter { count: u32 }
//!
//! impl Actor for Counter {
//!     fn receive(&mut self, letter: Letter, cx: &Cx) -> Vec<Effect> {
//!         self.count += 1;
//!         let receipt = Value::map(vec![("count".into(), Value::Int(self.count as i64))])
//!             .expect("fixed key");
//!         // report to whoever the payload's replyTo names, keeping
//!         // the conversation's correlation
//!         match letter.reply_to() {
//!             Some(to) => vec![Effect::post(to, "example.counted@1", encode(&receipt))
//!                 .expect("fixed kind")],
//!             None => vec![Effect::reply(&letter, cx, "example.counted@1", encode(&receipt))
//!                 .expect("fixed kind")],
//!         }
//!     }
//! }
//!
//! impl Durable for Counter {
//!     fn snapshot(&self) -> Vec<u8> { self.count.to_le_bytes().to_vec() }
//!     fn restore(&mut self, state: &[u8]) {
//!         self.count = u32::from_le_bytes(state.try_into().expect("4 bytes"));
//!     }
//! }
//!
//! mwydyn_guest::durable_entry!(Counter { count: 0 });
//! ```
//!
//! Compile to `wasm32-unknown-unknown` (a cdylib), drop the
//! `.wasm` into a host — that's the deployment story.
//!
//! ## The three words: stateless / stateful / durable
//!
//! Your one design decision is what the runtime owes your actor
//! when a transition is refused (bad effects, a panic, a crash —
//! all are *strikes*, and supervision decides the rest):
//!
//! | Entry | On refusal | On process death |
//! |---|---|---|
//! | [`entry!`] | nothing owed — the instance is reused, mutated state and all | state is gone |
//! | [`stateful_entry!`] | linear memory rolls back to pre-letter state — exactly-once internal state | state is gone |
//! | [`durable_entry!`] | rollback (as stateful) | state restores from the last snapshot |
//!
//! Stateful means: keep state in the actor struct (it lives in
//! linear memory; the runtime snapshots it per letter — mutable
//! globals are *not* covered, don't use them for state). Durable
//! adds [`Durable`] — your `snapshot`/`restore` serialization —
//! and is a compile error to declare without implementing.
//!
//! ## The vocabulary
//!
//! - [`Actor::receive`] — one letter in (by value: `kind`,
//!   `payload`, `correlation` are yours), effects out. Applied
//!   atomically by the host: all of them or none.
//! - [`LetterExt::payload_value`] — decode the payload (it is
//!   always canonical CBOR; that's the runtime's wall, not yours),
//!   then read fields with [`ValueExt`] (`field_str`,
//!   `field_bytes`, `field_int`, `field_addr`).
//! - [`Effect::post`] / [`Effect::reply`] — writing is declaring:
//!   return effects; there is no `send` call.
//! - [`Cx`] — the letter's sender ([`Cx::sender`]) and the ambient
//!   face: [`Cx::blob_put`], [`Cx::blob_get`], [`Cx::blob_len`],
//!   [`Cx::caps`]. Every call is capability-checked by the host;
//!   refusals come back BY NAME as [`Refusal`] — show them, don't
//!   swallow them.
//! - [`fail!`] — the documented way to fail a transition: it
//!   panics, the runtime strikes, stateful actors roll back.
//!
//! ## The conventions worth knowing
//!
//! - **Panic = strike.** No error returns; a failed transition is
//!   not a message. Panic (or `fail!`) and let the runtime owe you
//!   what your entry declared.
//! - **Blobs never transit messages.** Bytes go to the store via
//!   `blob_put`; messages carry the returned [`BlobRef`].
//! - **Reply routing** (spec §9.5): actor-to-actor, the sender IS
//!   the return address ([`Effect::reply`]); to name a third place
//!   (typically the embedding's outbox), the payload convention is
//!   a `replyTo` field — [`LetterExt::reply_to`] reads it.
//! - **No clock.** The runtime is clock-oblivious; timers arrive as
//!   letters. Don't look for `now()` — there isn't one.
//! - **One actor per artifact.** Invoke exactly one entry macro, at
//!   the crate root.
//!
//! ## Examples that compile
//!
//! The workspace's example guests are living documentation:
//! `mwydyn-guest-store` (stateless, ambient blobs, refusals),
//! `mwydyn-guest-dedup` (stateful, rollback semantics), and
//! `mwydyn-guest-counter` (durable, snapshot/restore across process
//! death). Start there; copy the one shaped like your problem.
//!
//! ---
//!
//! Under the hood (you never need this): this crate also exposes
//! the raw barrier mechanics — [`alloc`], [`dealloc`],
//! [`respond`], [`ambient_call`] — for hand-rolled guests and
//! tests. The codec is `mwydyn-data`: one canonical CBOR profile,
//! both realms.

use mwydyn_data::{decode, encode, DecodeOptions, Value};

mod actor;
mod entry;

pub use actor::{Actor, BlobRef, Cx, Durable, Effect, LetterExt, Refusal, Row, ValueExt};
pub use entry::{letter, process_impl, restore_impl, snapshot_impl, Slot};

/// Everything a plugin imports, in one line:
/// `use mwydyn_guest::prelude::*;`
pub mod prelude {
    pub use crate::{
        durable_entry, entry, fail, stateful_entry,
        actor::{Actor, BlobRef, Cx, Durable, Effect, LetterExt, Refusal, Row, ValueExt},
    };
    // The data-model vocabulary a payload touches — one canon,
    // both realms (re-exported so a plugin is ONE dependency).
    pub use mwydyn_data::{decode, encode, Address, EnvelopeError, Letter, Value};
}

/// Hand `len` bytes of scratch to the host (the barrier's inbound
/// buffer). Backed by a leaked boxed slice — exact capacity.
pub fn alloc(len: usize) -> *mut u8 {
    assert!(len > 0, "guest sdk: zero-length alloc");
    let buf = vec![0u8; len].into_boxed_slice();
    Box::into_raw(buf) as *mut u8
}

/// Return scratch previously handed out by [`alloc`].
///
/// # Safety
/// `ptr` must be a pointer previously returned by [`alloc`] with the
/// same `len`, not yet deallocated.
pub unsafe fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// Wrap response bytes in the ABI block `[u32 le len][bytes]`.
pub fn respond(bytes: &[u8]) -> *mut u8 {
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    Box::into_raw(buf.into_boxed_slice()) as *mut u8
}

/// Read a response block handed back by the host.
///
/// # Safety
/// `ptr` must point to a block the host wrote: `[u32 le len][bytes]`.
pub unsafe fn read_block(ptr: *const u8) -> Vec<u8> {
    let prefix = unsafe { std::slice::from_raw_parts(ptr, 4) };
    let len = u32::from_le_bytes(prefix.try_into().expect("4 bytes")) as usize;
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr.add(4), len) });
    out
}

// The import lives under its own module name; on HOST builds the
// symbol does not exist — host-target compiles (SDK unit tests) get
// a stub that refuses, wasm builds get the real import.
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "mwydyn")]
extern "C" {
    // the wasm import is ("mwydyn", "ambient") — the linker's name
    fn ambient(in_ptr: *const u8, in_len: usize, out_ptr: *mut u8, out_cap: usize) -> i32;
}

#[cfg(target_arch = "wasm32")]
unsafe fn ambient_raw(
    in_ptr: *const u8,
    in_len: usize,
    out_ptr: *mut u8,
    out_cap: usize,
) -> i32 {
    unsafe { ambient(in_ptr, in_len, out_ptr, out_cap) }
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn ambient_raw(
    _in_ptr: *const u8,
    _in_len: usize,
    _out_ptr: *mut u8,
    _out_cap: usize,
) -> i32 {
    -1
}

/// The ambient call: canonical CBOR request in, `{"ok"} | {"err"}`
/// response out. `Err(code)` carries the host's refusal code
/// (`Unauthorized`, `Missing`, `UnsupportedVerb`, ...). The guest
/// keeps ONE response scratch that grows on demand (the host refuses
/// oversized writes with -1 and the call retries).
///
/// Prefer the typed surface ([`Cx`]); this is the raw escape hatch.
pub fn ambient_call(call: Value) -> Result<Value, String> {
    let request = encode(&call);
    let in_ptr = alloc(request.len());
    // THE WRITE the first cut forgot: scratch arrives zeroed — the
    // request must land in it or the host reads zeros (BadForm).
    unsafe {
        std::slice::from_raw_parts_mut(in_ptr, request.len()).copy_from_slice(&request);
    }
    // SAFETY: in_ptr is a live alloc of exactly request.len().
    let mut out_cap = 256usize;
    loop {
        let out_ptr = alloc(out_cap);
        // SAFETY: both pointers are live scratch with the declared
        // lengths; the host never retains them.
        let written = unsafe {
            ambient_raw(in_ptr, request.len(), out_ptr, out_cap)
        };
        if written >= 0 {
            // SAFETY: the host wrote `written` bytes at out_ptr.
            let resp_bytes = unsafe { read_block_from(out_ptr, written as usize) };
            unsafe { dealloc(in_ptr, request.len()) };
            unsafe { dealloc(out_ptr, out_cap) };
            let v = decode(&resp_bytes, &DecodeOptions::default())
                .map_err(|e| format!("BadForm: {e:?}"))?;
            return match &v {
                Value::Map(pairs) => {
                    for (k, val) in pairs {
                        match k.as_str() {
                            "ok" => return Ok(val.clone()),
                            "err" => {
                                return match val {
                                    Value::Text(code) => Err(code.clone()),
                                    _ => Err("err without a code".into()),
                                }
                            }
                            _ => {}
                        }
                    }
                    Err("response carries neither ok nor err".into())
                }
                _ => Err("response not a map".into()),
            };
        }
        unsafe { dealloc(out_ptr, out_cap) };
        out_cap *= 2;
        if out_cap > (1 << 22) {
            return Err("OutCap".into());
        }
    }
}

/// Read `len` bytes at `ptr` (the host-written response body).
/// # Safety
/// `ptr` must point to `len` initialized bytes.
unsafe fn read_block_from(ptr: *mut u8, len: usize) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
}
