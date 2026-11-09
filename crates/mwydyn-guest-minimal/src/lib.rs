//! mwydyn-guest-minimal — the smallest useful mwydyn actor as a
//! plain `wasm32-unknown-unknown` module with ZERO imports: no WASI,
//! nothing to fake (round 33: CBOR all the way — the barrier has no
//! type system of its own, linear memory carries canonical CBOR
//! bytes).
//!
//! Barrier ABI (guest exports):
//! - `alloc(len) -> ptr`, `dealloc(ptr, len)`
//! - `process(req_ptr, req_len) -> resp_ptr` where the response
//!   block is `[u32 le resp_len][resp bytes]`
//!
//! The request is a [`BarrierRequest`] (canonical CBOR); the
//! response is the canonical-CBOR effects array. mwydyn-data IS the
//! guest-side codec — one canonical-CBOR definition, both realms.
//!
//! The transform: a `test.work-order@1` letter's payload is
//! `{orderId, priority, items: [{qty, sku, unitPrice}], due,
//! maxHops, hops: [], replyTo}` — the guest totals the items, stamps
//! a hop, and posts the work-order on to `replyTo` until the hop
//! budget runs out (then it goes idle: the bounce terminates).

use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{address_from_value, decode, encode, DecodeOptions, Letter, Value};

const WORK_KIND: &str = "test.work-order@1";

/// A barrier violation or malformed letter is a guest crash — trap;
/// the runtime's supervision classifies it (strike/dead-letter).
fn die(why: &str) -> ! {
    panic!("guest-minimal: {why}")
}

fn field<'a>(pairs: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn set_pair(pairs: &mut Vec<(String, Value)>, key: &str, value: Value) {
    match pairs.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => pairs.push((key.into(), value)),
    }
}

fn int(n: i64) -> Value {
    Value::Int(n)
}

/// The barrier ABI: hand the guest `len` bytes of scratch.
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    assert!(len > 0, "guest-minimal: zero-length alloc");
    let mut buf = vec![0u8; len];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// The barrier ABI: return scratch previously handed out by `alloc`.
///
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

/// The one call the barrier makes.
///
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
    let Value::Map(mut pairs) = order else {
        die("payload not a map");
    };

    // total = Σ qty × unitPrice (the exercise: nested decode, float
    // math under the always-f64 rule, re-encode)
    let mut total = 0.0f64;
    if let Some(Value::Array(items)) = field(&pairs, "items") {
        for item in items {
            let Value::Map(ip) = item else { die("item not a map") };
            let qty = match field(ip, "qty") {
                Some(Value::Int(n)) if *n >= 0 => *n as f64,
                _ => die("bad qty"),
            };
            let price = match field(ip, "unitPrice") {
                Some(Value::Float(f)) => *f,
                _ => die("bad unitPrice"),
            };
            total += qty * price;
        }
    }

    // Hop bookkeeping: stamp this processing; stop at the budget.
    let max_hops = match field(&pairs, "maxHops") {
        Some(Value::Int(n)) if *n >= 0 => *n as usize,
        _ => die("bad maxHops"),
    };
    let mut hops = match field(&pairs, "hops") {
        Some(Value::Array(h)) => h.clone(),
        _ => die("bad hops"),
    };

    // Routing (no reply-to field): the payload's replyTo names the
    // FIRST hop after submission; after that the work-order goes
    // back to whoever sent it (the round-09 reply-rides-from idiom).
    // When the budget is spent, the completed work-order reports OUT
    // to the embedding's external outbox.
    let dest = match &req.from {
        mwydyn_data::Address::Instance(id) if id == "external" => {
            match field(&pairs, "replyTo").and_then(address_from_value) {
                Some(a) => a,
                None => die("bad replyTo"),
            }
        }
        other => other.clone(),
    };

    if hops.len() >= max_hops {
        // Budget spent — report the completed work-order OUT.
        let letter = Letter::new(
            WORK_KIND,
            encode(&Value::Map(pairs)),
            None,
            req.letter.correlation.clone(),
            vec![],
        )
        .unwrap_or_else(|e| die(&format!("completion letter: {e:?}")));
        let effects = [BarrierEffect::Post {
            to: mwydyn_data::Address::Instance("external".into()),
            letter,
        }];
        let v = Value::Array(effects.iter().map(|e| e.to_value()).collect());
        return respond(&encode(&v));
    }
    hops.push(Value::map(vec![("n".into(), int(hops.len() as i64 + 1))]).expect("fixed key"));
    set_pair(&mut pairs, "hops", Value::Array(hops));
    set_pair(&mut pairs, "total", Value::Float(total));

    let payload = encode(&Value::Map(pairs));
    let letter = Letter::new(WORK_KIND, payload, None, req.letter.correlation.clone(), vec![])
        .unwrap_or_else(|e| die(&format!("reply letter: {e:?}")));
    let effects = [BarrierEffect::Post { to: dest, letter }];
    let v = Value::Array(effects.iter().map(|e| e.to_value()).collect());
    respond(&encode(&v))
}

/// Wrap the response bytes in the ABI block `[u32 le len][bytes]`.
fn respond(bytes: &[u8]) -> *mut u8 {
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    Box::into_raw(buf.into_boxed_slice()) as *mut u8
}
