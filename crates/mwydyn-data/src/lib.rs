//! mwydyn-data — the data model and canonical CBOR profile (spec §1).
//!
//! The profile: deterministic RFC 8949 with pinned choices —
//! shortest-form integers, always-float64 for floats (f16/f32 are
//! accepted on decode, never emitted), no NaN/Infinity/undefined,
//! string-key-only maps sorted by (content-byte-length, bytes),
//! indefinite lengths forbidden, unknown tags rejected, key hygiene
//! enforced, depth limits at decode.

pub mod decode;
pub mod encode;

pub use decode::decode;
pub use encode::encode;

/// Maximum nesting depth at decode (containers nest at most
/// `MAX_DEPTH` levels; the leaf may sit at `MAX_DEPTH`).
pub const MAX_DEPTH: usize = 64;

/// Always-blacklisted map keys, recursively.
pub const BLACKLISTED_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hygiene {
    /// Blacklist + whitelist `^[A-Za-z_][A-Za-z0-9_-]*$` (the default).
    Strict,
    /// Blacklist only — for callers that own looser payload regimes.
    BlacklistOnly,
}

pub fn key_is_hygienic(key: &str, hygiene: Hygiene) -> bool {
    if BLACKLISTED_KEYS.contains(&key) {
        return false;
    }
    match hygiene {
        Hygiene::BlacklistOnly => true,
        Hygiene::Strict => {
            let b = key.as_bytes();
            if b.is_empty() {
                return false;
            }
            let first_ok = b[0].is_ascii_alphabetic() || b[0] == b'_';
            let rest_ok = b[1..]
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-');
            first_ok && rest_ok
        }
    }
}

/// Canonical map key order: (content byte-length, content bytes).
pub(crate) fn map_sort_key(key: &str) -> (usize, &[u8]) {
    (key.len(), key.as_bytes())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValueError {
    NonFiniteFloat,
    DuplicateKey(String),
    BadKeyHygiene(String),
    BadBignum(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    Empty,
    Malformed(&'static str),
    /// Indefinite length or break — forbidden both ways.
    Indefinite,
    TooDeep,
    TrailingBytes,
    NonFiniteFloat,
    UnknownTag(u64),
    DuplicateKey(String),
    BadKeyHygiene,
    BadBignum,
}

pub struct DecodeOptions {
    pub hygiene: Hygiene,
    pub max_depth: usize,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions {
            hygiene: Hygiene::Strict,
            max_depth: MAX_DEPTH,
        }
    }
}

/// The data model: the JSON model, plus byte buffers as a first-class
/// primitive, plus one tagged timestamp type. JSON is a projection;
/// CBOR is the wire and storage truth (spec §1).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    /// Finite f64 only. Floatness is preserved: `Float(3.0)` encodes
    /// as 0xfb, never as an integer.
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    /// Tag 2 — big-endian magnitude, minimal (non-empty, no leading
    /// zero byte). Integers beyond `i64` travel here.
    BigPos(Vec<u8>),
    /// Tag 3 — value = -1 - n over the magnitude.
    BigNeg(Vec<u8>),
    /// Tag 1001 — integer microseconds since the Unix epoch, UTC
    /// (int64 range).
    Timestamp(i64),
    Array(Vec<Value>),
    /// String-keyed; sorted by `(byte-length, bytes)`; duplicates
    /// rejected; keys hygiene-checked at construction.
    Map(Vec<(String, Value)>),
}

impl Value {
    pub fn float(f: f64) -> Result<Value, ValueError> {
        if f.is_finite() {
            Ok(Value::Float(f))
        } else {
            Err(ValueError::NonFiniteFloat)
        }
    }

    pub fn big_pos(magnitude_be: Vec<u8>) -> Result<Value, ValueError> {
        check_minimal(&magnitude_be)?;
        Ok(Value::BigPos(magnitude_be))
    }

    pub fn big_neg(magnitude_be: Vec<u8>) -> Result<Value, ValueError> {
        check_minimal(&magnitude_be)?;
        Ok(Value::BigNeg(magnitude_be))
    }

    /// Build a map: hygiene (Strict), duplicate rejection, canonical
    /// sort. The sort happens here, so `encode` output is canonical
    /// by construction.
    pub fn map(mut pairs: Vec<(String, Value)>) -> Result<Value, ValueError> {
        for (k, _) in &pairs {
            if !key_is_hygienic(k, Hygiene::Strict) {
                return Err(ValueError::BadKeyHygiene(k.clone()));
            }
        }
        pairs.sort_by(|a, b| map_sort_key(&a.0).cmp(&map_sort_key(&b.0)));
        for w in pairs.windows(2) {
            if w[0].0 == w[1].0 {
                return Err(ValueError::DuplicateKey(w[0].0.clone()));
            }
        }
        Ok(Value::Map(pairs))
    }

    pub(crate) fn map_unchecked(sorted: Vec<(String, Value)>) -> Value {
        Value::Map(sorted)
    }
}

fn check_minimal(magnitude_be: &[u8]) -> Result<(), ValueError> {
    if magnitude_be.is_empty() {
        return Err(ValueError::BadBignum("empty magnitude"));
    }
    if magnitude_be[0] == 0 {
        return Err(ValueError::BadBignum("non-minimal magnitude"));
    }
    Ok(())
}
