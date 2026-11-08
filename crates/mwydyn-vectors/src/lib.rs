//! mwydyn-vectors — the conformance-vector runner (spec §7.9).
//!
//! Vector files are JSON; CBOR appears as hex (and the data model as
//! its JSON projection, or an explicit `model` descriptor for values
//! outside JSON — bytes, timestamps, bignums). Three classes:
//! validity (schema/value × verdict), encode (value → canonical
//! bytes), decode (bytes → value | rejection), roundtrip (decode ∘
//! encode byte-identical). External implementations consume the same
//! corpus through this runner.

use mwydyn_data::{decode, encode, DecodeError, DecodeOptions, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Corpus {
    pub profile: String,
    pub cases: Vec<Case>,
}

#[derive(Deserialize)]
pub struct Case {
    pub name: String,
    pub op: Op,
    #[serde(rename = "in")]
    pub input: Input,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Encode,
    Decode,
    Roundtrip,
    Digest,
}

#[derive(Deserialize, Default)]
pub struct Input {
    #[serde(default)]
    pub json: Option<serde_json::Value>,
    #[serde(default)]
    pub model: Option<Model>,
    #[serde(default)]
    pub hex: Option<String>,
    #[serde(default)]
    pub bytes: Option<String>,
    #[serde(rename = "mediaType", default)]
    pub media_type: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Expect {
    #[serde(default)]
    pub hex: Option<String>,
    #[serde(default)]
    pub json: Option<serde_json::Value>,
    #[serde(default)]
    pub model: Option<Model>,
    #[serde(default)]
    pub reject: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Model {
    Null,
    Bytes { hex: String },
    Timestamp { micros: i64 },
    Big { sign: String, hex: String },
    Float { value: f64 },
    #[serde(rename = "ref")]
    BlobRef {
        digest: String,
        #[serde(rename = "mediaType")]
        media_type: String,
        size: u64,
    },
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.replace(' ', "");
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// JSON projection → Value (the vector-side direction; the crate's
/// own projection API lands with the envelope work).
pub fn json_to_value(j: &serde_json::Value) -> Result<Value, String> {
    Ok(match j {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                let mut mag = u.to_be_bytes().to_vec();
                while mag.len() > 1 && mag[0] == 0 {
                    mag.remove(0);
                }
                Value::big_pos(mag).map_err(|e| format!("{:?}", e))?
            } else {
                let f = n.as_f64().ok_or("number is neither int nor float")?;
                Value::float(f).map_err(|e| format!("{:?}", e))?
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(items) => {
            let mut out = Vec::new();
            for it in items {
                out.push(json_to_value(it)?);
            }
            Value::Array(out)
        }
        serde_json::Value::Object(map) => {
            let mut pairs = Vec::new();
            for (k, v) in map {
                pairs.push((k.clone(), json_to_value(v)?));
            }
            Value::map(pairs).map_err(|e| format!("{:?}", e))?
        }
    })
}

pub fn model_to_value(m: &Model) -> Result<Value, String> {
    Ok(match m {
        Model::Null => Value::Null,
        Model::Bytes { hex: h } => Value::Bytes(unhex(h)?),
        Model::Timestamp { micros } => Value::Timestamp(*micros),
        Model::Big { sign, hex: h } => {
            let mag = unhex(h)?;
            if sign == "pos" {
                Value::big_pos(mag).map_err(|e| format!("{:?}", e))?
            } else {
                Value::big_neg(mag).map_err(|e| format!("{:?}", e))?
            }
        }
        Model::Float { value } => Value::float(*value).map_err(|e| format!("{:?}", e))?,
        Model::BlobRef {
            digest,
            media_type,
            size,
        } => {
            // model-supplied refs are validated, never trusted
            Value::validate_blob_ref(digest, media_type)
                .map_err(|e| format!("{:?}", e))?;
            Value::BlobRef {
                digest: digest.clone(),
                media_type: media_type.clone(),
                size: *size,
            }
        }
    })
}

fn value_to_json(v: &Value) -> Result<serde_json::Value, String> {
    Ok(match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(*f).ok_or("non-finite float")?,
        ),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        // bytes/timestamp/bignum don't project — corpus uses models
        _ => return Err("value does not project to JSON".into()),
    })
}

#[allow(dead_code)] // the envelope round's projection direction
fn _keep_projection(_v: &Value) -> Result<serde_json::Value, String> {
    value_to_json(_v)
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|((k1, v1), (k2, v2))| {
                    k1 == k2 && same_value(v1, v2)
                })
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same_value(a, b))
        }
        (a, b) => a == b,
    }
}

fn decode_error_code(e: &DecodeError) -> &'static str {
    match e {
        DecodeError::Empty => "empty",
        DecodeError::Malformed(_) => "malformed",
        DecodeError::Indefinite => "indefinite",
        DecodeError::TooDeep => "too-deep",
        DecodeError::TrailingBytes => "trailing",
        DecodeError::NonFiniteFloat => "non-finite",
        DecodeError::UnknownTag(_) => "unknown-tag",
        DecodeError::DuplicateKey(_) => "duplicate-key",
        DecodeError::BadKeyHygiene => "bad-key",
        DecodeError::BadBignum => "bad-bignum",
        DecodeError::BadBlobRef => "bad-ref",
    }
}

fn input_to_value(input: &Input) -> Result<Value, String> {
    if let Some(j) = &input.json {
        json_to_value(j)
    } else if let Some(m) = &input.model {
        model_to_value(m)
    } else {
        Err("case has neither json nor model input".into())
    }
}

/// Run a corpus; returns one failure line per violated case.
pub fn check_corpus(cases: &[Case]) -> Vec<String> {
    let mut failures = Vec::new();
    let opts = DecodeOptions::default();
    for case in cases {
        let res: Result<(), String> = (|| {
            match case.op {
                Op::Encode => {
                    let v = match input_to_value(&case.input) {
                        Ok(v) => v,
                        Err(e) => {
                            if case.expect.reject.is_some() {
                                return Ok(());
                            }
                            return Err(e);
                        }
                    };
                    let bytes = encode(&v);
                    if case.expect.reject.is_some() {
                        return Err("expected reject, encoded ok".into());
                    }
                    match &case.expect.hex {
                        Some(h) if hex(&bytes) != *h => {
                            return Err(format!(
                                "hex mismatch: got {}, want {}",
                                hex(&bytes),
                                h
                            ))
                        }
                        _ => {}
                    }
                }
                Op::Decode => {
                    let bytes = unhex(case.input.hex.as_deref().ok_or("decode needs hex")?)?;
                    match decode(&bytes, &opts) {
                        Err(e) => match &case.expect.reject {
                            Some(code) if code == decode_error_code(&e) => {}
                            Some(other) => {
                                return Err(format!("rejected {:?}, want {}", e, other))
                            }
                            None => return Err(format!("unexpectedly rejected: {:?}", e)),
                        },
                        Ok(v) => {
                            if case.expect.reject.is_some() {
                                return Err("expected reject, decoded ok".into());
                            }
                            if let Some(h) = &case.expect.hex {
                                let canonical = hex(&encode(&v));
                                if canonical != *h {
                                    return Err(format!(
                                        "canonical hex mismatch: got {}, want {}",
                                        canonical, h
                                    ));
                                }
                            }
                            let want = if let Some(j) = &case.expect.json {
                                json_to_value(j)?
                            } else if let Some(m) = &case.expect.model {
                                model_to_value(m)?
                            } else {
                                if case.expect.hex.is_some() {
                                    return Ok(());
                                }
                                return Err("decode case lacks an expectation".into());
                            };
                            if !same_value(&v, &want) {
                                return Err(format!("value mismatch: got {:?}, want {:?}", v, want));
                            }
                        }
                    }
                }
                Op::Roundtrip => {
                    let v = input_to_value(&case.input)?;
                    let bytes = encode(&v);
                    let v2 = decode(&bytes, &opts).map_err(|e| format!("re-decode: {:?}", e))?;
                    if !same_value(&v, &v2) {
                        return Err("roundtrip value mismatch".into());
                    }
                    let bytes2 = encode(&v2);
                    if bytes != bytes2 {
                        return Err("roundtrip byte mismatch (non-canonical decode?)".into());
                    }
                    if let Some(h) = &case.expect.hex {
                        if hex(&bytes) != *h {
                            return Err(format!("hex mismatch: got {}, want {}", hex(&bytes), h));
                        }
                    }
                }
                Op::Digest => {
                    let bytes = unhex(case.input.bytes.as_deref().ok_or("digest needs bytes")?)?;
                    let mt = case
                        .input
                        .media_type
                        .as_deref()
                        .ok_or("digest needs mediaType")?;
                    match mwydyn_data::Value::blob_digest(&bytes, mt) {
                        Err(e) => match &case.expect.reject {
                            Some(code) if code == "bad-ref" => {}
                            Some(other) => {
                                return Err(format!("rejected {:?}, want {}", e, other))
                            }
                            None => return Err(format!("unexpectedly rejected: {:?}", e)),
                        },
                        Ok(d) => {
                            if case.expect.reject.is_some() {
                                return Err("expected reject, computed ok".into());
                            }
                            match &case.expect.hex {
                                Some(h) if d != *h => {
                                    return Err(format!("digest mismatch: got {}, want {}", d, h))
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(why) = res {
            failures.push(format!("{}: {}", case.name, why));
        }
    }
    failures
}

pub fn load_corpus(path: &std::path::Path) -> Result<Corpus, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}
