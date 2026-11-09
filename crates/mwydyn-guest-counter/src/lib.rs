//! mwydyn-guest-counter — the round-37 declared-statefulness proof
//! guest. A static counter lives in LINEAR MEMORY (a .data segment —
//! the stateful memory image restores it byte-exact). Every letter
//! increments it and posts a receipt `{"count": n}` to the letter's
//! `replyTo`.
//!
//! Same barrier ABI as guest-minimal: `alloc`/`dealloc`/
//! `process(req_ptr, req_len) → [u32 len][CBOR]`, zero imports.

use std::sync::atomic::{AtomicU32, Ordering};

use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{address_from_value, decode, encode, DecodeOptions, Letter, Value};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn die(why: &str) -> ! {
    panic!("guest-counter: {why}")
}

fn field<'a>(pairs: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

/// §7.6 RESTORE ABI (round 39): the container blob_gets the boot's
/// snapshot host-side and hands the bytes over. The counter's state
/// format is its own: 4 bytes, little-endian u32.
/// # Safety
/// `ptr` must point to `len` readable bytes in guest linear memory.
#[no_mangle]
pub unsafe extern "C" fn restore(ptr: *const u8, len: usize) {
    assert!(len == 4, "guest-counter: snapshot is 4 bytes");
    let b = unsafe { std::slice::from_raw_parts(ptr, len) };
    let n = u32::from_le_bytes(b.try_into().expect("4 bytes"));
    COUNTER.store(n, Ordering::SeqCst);
}

/// §7.6 SAVE ABI (round 39): return `[u32 len][state bytes]` —
/// `Context::snapshot_actor` stores it and lands the covers-stamped
/// letter.
#[no_mangle]
pub extern "C" fn snapshot() -> *mut u8 {
    let n = COUNTER.load(Ordering::SeqCst);
    respond(&n.to_le_bytes())
}

#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    assert!(len > 0);
    let mut buf = vec![0u8; len];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// # Safety
/// `ptr` must be a pointer previously returned by [`alloc`] with the
/// same `len`, not yet deallocated.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

/// # Safety
/// `req_ptr` must point to `req_len` readable bytes in guest linear
/// memory (the host writes the request at a pointer from `alloc`).
#[no_mangle]
pub unsafe extern "C" fn process(req_ptr: *const u8, req_len: usize) -> *mut u8 {
    let req_bytes = unsafe { std::slice::from_raw_parts(req_ptr, req_len) };

    let req_value = decode(req_bytes, &DecodeOptions::default())
        .unwrap_or_else(|e| die(&format!("request decode: {e:?}")));
    let req = BarrierRequest::from_value(&req_value)
        .unwrap_or_else(|e| die(&format!("request shape: {e:?}")));

    let order = decode(&req.letter.payload, &DecodeOptions::default())
        .unwrap_or_else(|e| die(&format!("payload decode: {e:?}")));
    let Value::Map(pairs) = order else {
        die("payload not a map");
    };

    // THE STATE: a linear-memory static, incremented per letter.
    let count = COUNTER.fetch_add(1, Ordering::SeqCst) + 1;

    // The MESSAGE carries the routing (no reply-to field).
    let dest = match field(&pairs, "replyTo").and_then(address_from_value) {
        Some(a) => a,
        None => die("bad replyTo"),
    };

    let payload = encode(&Value::map(vec![(
        "count".into(),
        Value::Int(count as i64),
    )])
    .expect("fixed key"));
    let letter = Letter::new("test.counter@1", payload, None, req.letter.correlation.clone(), vec![])
        .unwrap_or_else(|e| die(&format!("receipt letter: {e:?}")));
    let effects = [BarrierEffect::Post { to: dest, letter }];
    let v = Value::Array(effects.iter().map(|e| e.to_value()).collect());

    respond(&encode(&v))
}

/// Wrap response bytes in the ABI block `[u32 le len][bytes]`.
fn respond(bytes: &[u8]) -> *mut u8 {
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    Box::into_raw(buf.into_boxed_slice()) as *mut u8
}
