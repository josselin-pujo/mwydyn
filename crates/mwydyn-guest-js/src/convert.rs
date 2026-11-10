//! Value ⇄ JS conversion (round 43): ONE canon — the wrapper owns
//! the CBOR, authors see native JS values. The table (work-notes/43
//! §3): Null↔null/undefined, Bool↔boolean, Int↔number (±2^53;
//! beyond ↔ BigInt via i64), Float↔number, Text↔string,
//! Bytes↔Uint8Array, Map↔plain object, Array↔Array,
//! BlobRef↔frozen {digest, mediaType, size} with a non-enumerable
//! `__mwRef` brand. Timestamps cross as µs numbers (the prelude
//! documents the loss of the tag); BigPos/BigNeg beyond i64 are a
//! loud refusal in v0.

use crate::ffi::{self, JSValue};
use crate::{Engine, JsError};
use mwydyn_data::Value;

const REF_BRAND: &[u8] = b"__mwRef";
const SAFE: f64 = 9_007_199_254_740_992.0; // 2^53

/// JS → data-model Value. Errors are JsError and become strikes.
pub fn js_to_value(e: &mut Engine, v: JSValue) -> Result<Value, JsError> {
    unsafe {
        let ctx = e.ctx();
        match v.tag() {
            ffi::JS_TAG_INT => Ok(Value::Int(v.u.int32 as i64)),
            ffi::JS_TAG_BOOL => Ok(Value::Bool(v.u.int32 != 0)),
            ffi::JS_TAG_NULL | ffi::JS_TAG_UNDEFINED => Ok(Value::Null),
            ffi::JS_TAG_FLOAT64 => {
                let f = v.u.float64;
                if f.fract() == 0.0 && f.abs() <= SAFE {
                    Ok(Value::Int(f as i64))
                } else {
                    Value::float(f).map_err(|e| JsError { message: format!("{e:?}") })
                }
            }
            ffi::JS_TAG_STRING => {
                let s = e.js_to_string(v);
                Ok(Value::Text(s))
            }
            ffi::JS_TAG_BIG_INT => {
                // v0: i64-range bigints via toString; larger = loud
                let s = bigint_to_string(e, v)?;
                let n: i64 = s
                    .parse()
                    .map_err(|_| JsError { message: format!("bigint {s} beyond i64 (v0)") })?;
                Ok(Value::Int(n))
            }
            ffi::JS_TAG_OBJECT => object_to_value(e, v),
            other => Err(JsError {
                message: format!("unsupported JS value (tag {other})"),
            }),
        }
    }
}

unsafe fn object_to_value(e: &mut Engine, v: JSValue) -> Result<Value, JsError> {
    unsafe {
        let ctx = e.ctx();

        // BlobRef brand first (non-enumerable, prelude-set)
        let brand = ffi::JS_GetPropertyStr(ctx, v, REF_BRAND.as_ptr());
        let branded = brand.tag() == ffi::JS_TAG_BOOL && brand.u.int32 != 0;
        ffi::JS_FreeValue(ctx, brand);
        if branded {
            let get = |e: &mut Engine, k: &str| -> Result<String, JsError> {
                let pv = ffi::JS_GetPropertyStr(ctx, v, k.as_ptr());
                let s = e.js_to_string(pv);
                Ok(s)
            };
            let digest = get(e, "digest")?;
            let media_type = get(e, "mediaType")?;
            let size_prop = ffi::JS_GetPropertyStr(ctx, v, b"size\0".as_ptr());
            let mut size = 0f64;
            ffi::JS_ToFloat64(ctx, &mut size, size_prop);
            ffi::JS_FreeValue(ctx, size_prop);
            Value::validate_blob_ref(&digest, &media_type)
                .map_err(|err| JsError { message: format!("bad ref: {err:?}") })?;
            return Ok(Value::BlobRef {
                digest,
                media_type,
                size: size as u64,
            });
        }

        if ffi::JS_IsArray(v) {
            let mut len_prop = ffi::JS_GetPropertyStr(ctx, v, b"length\0".as_ptr());
            let mut len = 0i32;
            ffi::JS_ToInt32(ctx, &mut len, len_prop);
            ffi::JS_FreeValue(ctx, len_prop);
            let mut items = Vec::with_capacity(len as usize);
            for i in 0..len as u32 {
                let item = ffi::JS_GetPropertyUint32(ctx, v, i);
                items.push(js_to_value(e, item)?);
                ffi::JS_FreeValue(ctx, item);
            }
            return Ok(Value::Array(items));
        }

        // Uint8Array → Bytes (via its ArrayBuffer view)
        let mut off = 0usize;
        let mut blen = 0usize;
        let mut bpe = 0usize;
        let buf = ffi::JS_GetTypedArrayBuffer(ctx, v, &mut off, &mut blen, &mut bpe);
        if !buf.is_exception() {
            let mut pbuf = std::ptr::null_mut();
            let mut plen = 0usize;
            // ArrayBuffer size via its byteLength property
            let bl = ffi::JS_GetPropertyStr(ctx, buf, b"byteLength\0".as_ptr());
            let mut total = 0f64;
            ffi::JS_ToFloat64(ctx, &mut total, bl);
            ffi::JS_FreeValue(ctx, bl);
            if ffi::JS_GetArrayBuffer(ctx, &mut pbuf, buf) == 0 {
                let _ = plen;
                let start = pbuf.add(off);
                let bytes = std::slice::from_raw_parts(start, blen).to_vec();
                ffi::JS_FreeValue(ctx, buf);
                return Ok(Value::Bytes(bytes));
            }
            let _ = total;
        }

        // plain object → Map (own enumerable string keys)
        let mut tab: *mut ffi::JSPropertyEnum = std::ptr::null_mut();
        let mut count = 0u32;
        if ffi::JS_GetOwnPropertyNames(ctx, &mut tab, &mut count, v, ffi::JS_GPN_STRING_MASK) != 0 {
            return Err(e.take_exception());
        }
        let mut pairs = Vec::with_capacity(count as usize);
        for i in 0..count {
            let atom = std::ptr::read_unaligned(
                (tab as *const u64).add(i as usize), // JSPropertyEnum { is_enumerable: u8, atom: u64 } — atom via JS_AtomToCString
            );
            let _ = atom;
            // atom → string via the runtime helper
            let cstr = js_atom_to_cstring(e, tab, i);
            let key = cstr;
            let pv = ffi::JS_GetPropertyStr(ctx, v, key.as_ptr());
            let value = js_to_value(e, pv)?;
            ffi::JS_FreeValue(ctx, pv);
            pairs.push((key, value));
        }
        ffi::JS_FreePropertyEnum(ctx, tab, count);
        Value::map(pairs).map_err(|err| JsError { message: format!("bad map keys: {err:?}") })
    }
}

// JSPropertyEnum layout: { uint8_t is_enumerable; JSAtom atom; } —
// atom is u32 in quickjs-ng; we read it and stringify via the engine.
unsafe fn js_atom_to_cstring(e: &mut Engine, tab: *mut ffi::JSPropertyEnum, i: u32) -> String {
    unsafe {
        let entry = (tab as *const u8).add(i as usize * 8); // align-padded
        let atom = u32::from_le_bytes([
            *entry.add(4),
            *entry.add(5),
            *entry.add(6),
            *entry.add(7),
        ]);
        let ctx = e.ctx();
        let mut len = 0usize;
        let p = ffi::JS_AtomToCStringLen(ctx, &mut len, atom);
        if p.is_null() {
            return String::new();
        }
        let s = String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned();
        ffi::JS_FreeCString(ctx, p);
        s
    }
}

unsafe fn bigint_to_string(e: &mut Engine, v: JSValue) -> Result<String, JsError> {
    unsafe {
        let ctx = e.ctx();
        let tostr = ffi::JS_GetPropertyStr(ctx, v, b"toString\0".as_ptr());
        let mut argv = [];
        let s = ffi::JS_Call(ctx, tostr, v, 0, argv.as_mut_ptr());
        ffi::JS_FreeValue(ctx, tostr);
        if s.is_exception() {
            let err = e.take_exception();
            ffi::JS_FreeValue(ctx, s);
            return Err(err);
        }
        let out = e.js_to_string(s);
        ffi::JS_FreeValue(ctx, s);
        Ok(out)
    }
}

/// Data-model Value → JS.
pub fn value_to_js(e: &mut Engine, v: &Value) -> JSValue {
    unsafe {
        let ctx = e.ctx();
        match v {
            Value::Null => JSValue::mkval(ffi::JS_TAG_NULL, 0),
            Value::Bool(b) => JSValue::mkval(ffi::JS_TAG_BOOL, *b as i32),
            Value::Int(i) => {
                if *i >= i32::MIN as i64 && *i <= i32::MAX as i64 {
                    JSValue::mkval(ffi::JS_TAG_INT, *i as i32)
                } else if i.abs() <= SAFE as i64 {
                    JSValue::float(*i as f64)
                } else {
                    ffi::JS_NewBigInt64(ctx, *i)
                }
            }
            Value::Float(f) => JSValue::float(*f),
            Value::Text(s) => ffi::JS_NewStringLen(ctx, s.as_ptr(), s.len()),
            Value::Bytes(b) => ffi::JS_NewUint8ArrayCopy(ctx, b.as_ptr(), b.len()),
            Value::BigPos(_) | Value::BigNeg(_) => {
                // v0 loud refusal: trap-shaped — the byzantine law
                JSValue::mkval(ffi::JS_TAG_EXCEPTION, 0)
            }
            Value::Timestamp(us) => JSValue::float(*us as f64),
            Value::Array(items) => {
                let arr = ffi::JS_NewArray(ctx);
                for (i, item) in items.iter().enumerate() {
                    let js = value_to_js(e, item);
                    ffi::JS_SetPropertyUint32(ctx, arr, i as u32, js);
                }
                arr
            }
            Value::Map(pairs) => {
                let obj = ffi::JS_NewObject(ctx);
                for (k, val) in pairs {
                    let js = value_to_js(e, val);
                    ffi::JS_SetPropertyStr(ctx, obj, k.as_ptr(), js);
                }
                obj
            }
            Value::BlobRef {
                digest,
                media_type,
                size,
            } => {
                // frozen {digest, mediaType, size} + the brand
                let obj = ffi::JS_NewObject(ctx);
                set_str(e, obj, "digest", digest);
                set_str(e, obj, "mediaType", media_type);
                let sz = JSValue::float(*size as f64);
                ffi::JS_SetPropertyStr(ctx, obj, b"size\0".as_ptr(), sz);
                let brand = JSValue::mkval(ffi::JS_TAG_BOOL, 1);
                ffi::JS_DefinePropertyValueStr(
                    ctx,
                    obj,
                    REF_BRAND.as_ptr(),
                    brand,
                    ffi::JS_PROP_C_W_E & !0x02, // enumerable off
                );
                obj
            }
        }
    }
}

unsafe fn set_str(e: &mut Engine, obj: JSValue, k: &str, s: &str) {
    unsafe {
        let v = ffi::JS_NewStringLen(e.ctx(), s.as_ptr(), s.len());
        ffi::JS_SetPropertyStr(e.ctx(), obj, k.as_ptr() as *const u8, v);
    }
}
