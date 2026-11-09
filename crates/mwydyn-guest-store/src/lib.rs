//! mwydyn-guest-store — the round-40 ambient-surface proof guest.
//! Uses the mwydyn-guest SDK (scratch + respond + ambient wrapper);
//! the codec underneath is mwydyn-data, canonical CBOR both realms.

use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::ambient::AmbientCall;
use mwydyn_data::{
    address_from_value, decode, DecodeOptions, Letter, Value,
};
use mwydyn_guest::{ambient_call, respond};

fn field<'a>(pairs: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn die(why: &str) -> ! {
    panic!("guest-store: {why}")
}

fn blob_ref_from(v: &Value) -> Value {
    // BlobRef values pass through as-is (the data model's native form)
    v.clone()
}

/// One ambient round: put the body, get it back, len it.
fn store_roundtrip(body: &[u8]) -> Value {
    let mut receipt = Vec::new();

    // blob.put (needs blob.write)
    let put_call = AmbientCall {
        verb: mwydyn_data::ambient::AmbientVerb::BlobPut {
            bytes: body.to_vec(),
            media_type: "text/plain".into(),
            ttl_micros: None,
        },
    };
    let ref_ = match ambient_call(put_call.to_value()) {
        Ok(v) => {
            let put_len = match &v {
                Value::BlobRef { size, .. } => *size as i64,
                _ => -1,
            };
            receipt.push(("putLen".into(), Value::Int(put_len)));
            v
        }
        Err(code) => {
            receipt.push(("putLen".into(), Value::Int(-1)));
            receipt.push(("putErr".into(), Value::Text(code)));
            Value::Null
        }
    };

    // blob.get + blob.len (need blob.read)
    if ref_ != Value::Null {
        let get_call = AmbientCall {
            verb: mwydyn_data::ambient::AmbientVerb::BlobGet {
                ref_: blob_ref_from(&ref_),
            },
        };
        match ambient_call(get_call.to_value()) {
            Ok(Value::Bytes(got)) => {
                receipt.push(("gotLen".into(), Value::Int(got.len() as i64)));
                receipt.push(("match".into(), Value::Int((got == body) as i64)));
            }
            Ok(other) => die(&format!("blob.get returned {other:?}")),
            Err(code) => {
                receipt.push(("gotLen".into(), Value::Int(-1)));
                receipt.push(("getErr".into(), Value::Text(code)));
            }
        }

        let len_call = AmbientCall {
            verb: mwydyn_data::ambient::AmbientVerb::BlobLen {
                ref_: blob_ref_from(&ref_),
            },
        };
        match ambient_call(len_call.to_value()) {
            Ok(Value::Int(n)) => {
                receipt.push(("lenOk".into(), Value::Int((n == body.len() as i64) as i64)));
            }
            Ok(other) => die(&format!("blob.len returned {other:?}")),
            Err(code) => {
                receipt.push(("lenErr".into(), Value::Text(code)));
            }
        }
    }

    // caps: the guest lists what it holds (free verb)
    if let Ok(Value::Array(rows)) = ambient_call(
        AmbientCall {
            verb: mwydyn_data::ambient::AmbientVerb::Caps,
        }
        .to_value(),
    ) {
        receipt.push(("capRows".into(), Value::Int(rows.len() as i64)));
    }

    Value::map(receipt).expect("fixed receipt keys")
}

/// # Safety
/// `req_ptr` must point to `req_len` readable bytes in guest linear
/// memory (the host writes the request at a pointer from `alloc`).
/// THE BARRIER ABI (round 40): scratch management rides the SDK —
/// the exports are the guest's to declare.
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    mwydyn_guest::alloc(len)
}

/// # Safety
/// See `mwydyn_guest::dealloc`.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    unsafe { mwydyn_guest::dealloc(ptr, len) }
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

    // The MESSAGE carries the routing (no reply-to field).
    let dest = match field(&pairs, "replyTo").and_then(address_from_value) {
        Some(a) => a,
        None => die("bad replyTo"),
    };
    let body = match field(&pairs, "body") {
        Some(Value::Bytes(b)) => b.clone(),
        _ => die("bad body"),
    };

    let receipt = store_roundtrip(&body);
    let letter = Letter::new(
        "test.store@1",
        mwydyn_data::encode(&receipt),
        None,
        req.letter.correlation.clone(),
        vec![],
    )
    .unwrap_or_else(|e| die(&format!("receipt letter: {e:?}")));
    let effects = [BarrierEffect::Post { to: dest, letter }];
    let v = Value::Array(effects.iter().map(|e| e.to_value()).collect());
    respond(&mwydyn_data::encode(&v))
}
