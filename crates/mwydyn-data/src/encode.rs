//! Canonical CBOR encoding of [`Value`] (spec §1).

use crate::Value;

/// Encode canonically. Maps must already be canonical (constructor-
/// enforced); `debug_assert`s it anyway.
pub fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_value(&mut out, v);
    out
}

pub fn encode_into(out: &mut Vec<u8>, v: &Value) {
    write_value(out, v);
}

fn write_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => out.push(0xf6),
        Value::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
        Value::Int(i) => write_int(out, *i),
        Value::Float(f) => {
            debug_assert!(f.is_finite());
            out.push(0xfb);
            out.extend_from_slice(&f.to_be_bytes());
        }
        Value::Text(s) => {
            write_head(out, 3, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Bytes(b) => {
            write_head(out, 2, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::BigPos(m) => {
            write_head(out, 6, 2);
            write_head(out, 2, m.len() as u64);
            out.extend_from_slice(m);
        }
        Value::BigNeg(m) => {
            write_head(out, 6, 3);
            write_head(out, 2, m.len() as u64);
            out.extend_from_slice(m);
        }
        Value::Timestamp(micros) => {
            write_head(out, 6, 1001);
            write_int(out, *micros);
        }
        Value::Array(items) => {
            write_head(out, 4, items.len() as u64);
            for item in items {
                write_value(out, item);
            }
        }
        Value::Map(pairs) => {
            debug_assert!(pairs.windows(2).all(|w| {
                crate::map_sort_key(&w[0].0) < crate::map_sort_key(&w[1].0)
            }));
            write_head(out, 5, pairs.len() as u64);
            for (k, val) in pairs {
                write_head(out, 3, k.len() as u64);
                out.extend_from_slice(k.as_bytes());
                write_value(out, val);
            }
        }
    }
}

fn write_head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    match arg {
        0..=23 => out.push(m | arg as u8),
        24..=0xff => {
            out.push(m | 24);
            out.push(arg as u8);
        }
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(arg as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend_from_slice(&(arg as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&arg.to_be_bytes());
        }
    }
}

fn write_int(out: &mut Vec<u8>, i: i64) {
    if i >= 0 {
        write_head(out, 0, i as u64);
    } else {
        // -1 - i for i = i64::MIN yields i64::MAX; no overflow.
        write_head(out, 1, (-1 - i) as u64);
    }
}
