//! The AMBIENT HOST-FACE contract (round 40) — the guest-visible
//! surface for runtime services, CBOR all the way like every other
//! boundary. ONE import (`mwydyn.ambient`) carries ONE canonical
//! CBOR envelope: `{"verb": ..., <verb fields>}` in,
//! `{"ok": <value>}` or `{"err": "<code>"}` out. Verbs evolve as
//! data (the refusal wall tolerates unknown verbs), the shapes are
//! vectors-testable, and the guest SDK (mwydyn-data for Rust
//! guests) owns the marshaling.
//!
//! THE VERB SET mirrors the native `Host` face — same capabilities,
//! same cap checks (`row.holds(verb-domain, "*")`):
//! - `blob.get`  {ref}            → {ok: Bytes}   (needs blob.read)
//! - `blob.put`  {bytes, media_type, ttl?} → {ok: BlobRef} (blob.write)
//! - `blob.len`  {ref}            → {ok: Int}     (needs blob.read)
//! - `caps`      {}               → {ok: [{verb, object}, ...]} (free)
//!
//! NO clock verb exists — the ticker is the only wall-clock reader
//! (§6 clock obliviousness). `ghost.read` arrives with its face.
//!
//! Refusal codes (canonical strings): `Unauthorized` (cap missing),
//! `Missing`, `Integrity`, `UnsupportedVerb`, `BadForm`.

use crate::{DecodeError, Value};

/// The ambient verbs — decoded from the request envelope's `verb`
/// field with its verb fields alongside.
#[derive(Debug, Clone, PartialEq)]
pub enum AmbientVerb {
    BlobGet { ref_: Value },
    BlobPut {
        bytes: Vec<u8>,
        media_type: String,
        ttl_micros: Option<u64>,
    },
    BlobLen { ref_: Value },
    Caps,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AmbientCall {
    pub verb: AmbientVerb,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AmbientError {
    /// Canonical decode failed (or the shape was wrong).
    BadForm(&'static str),
    /// The verb is not part of the surface.
    UnsupportedVerb(String),
    /// The envelope decoded but a verb field was malformed.
    BadVerbField(&'static str),
    /// Convenience passthrough for callers decoding raw bytes.
    Decode(DecodeError),
}

impl AmbientCall {
    /// Encode to the canonical request map.
    pub fn to_value(&self) -> Value {
        match &self.verb {
            AmbientVerb::BlobGet { ref_ } => Value::map(vec![
                ("ref".into(), ref_.clone()),
                ("verb".into(), Value::Text("blob.get".into())),
            ])
            .expect("fixed hygienic keys"),
            AmbientVerb::BlobPut {
                bytes,
                media_type,
                ttl_micros,
            } => Value::map(vec![
                ("bytes".into(), Value::Bytes(bytes.clone())),
                ("mediaType".into(), Value::Text(media_type.clone())),
                (
                    "ttl".into(),
                    match ttl_micros {
                        Some(t) => Value::Int(*t as i64),
                        None => Value::Null,
                    },
                ),
                ("verb".into(), Value::Text("blob.put".into())),
            ])
            .expect("fixed hygienic keys"),
            AmbientVerb::BlobLen { ref_ } => Value::map(vec![
                ("ref".into(), ref_.clone()),
                ("verb".into(), Value::Text("blob.len".into())),
            ])
            .expect("fixed hygienic keys"),
            AmbientVerb::Caps => Value::map(vec![(
                "verb".into(),
                Value::Text("caps".into()),
            )])
            .expect("fixed hygienic keys"),
        }
    }

    /// Decode the request map: the verb field dispatches, the verb's
    /// fields are shape-checked alongside.
    pub fn from_value(v: &Value) -> Result<AmbientCall, AmbientError> {
        const E: AmbientError = AmbientError::BadForm("malformed ambient call");
        let Value::Map(pairs) = v else {
            return Err(E);
        };
        let field = |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let Value::Text(verb) = field("verb").ok_or(E)? else {
            return Err(E);
        };
        match verb.as_str() {
            "blob.get" | "blob.len" => {
                let ref_ = match field("ref") {
                    Some(v @ Value::BlobRef { .. }) => v.clone(),
                    Some(_) => {
                        return Err(AmbientError::BadVerbField("ref must be a BlobRef"))
                    }
                    None => return Err(AmbientError::BadVerbField("ref required")),
                };
                if verb.as_str() == "blob.get" {
                    Ok(AmbientCall {
                        verb: AmbientVerb::BlobGet { ref_ },
                    })
                } else {
                    Ok(AmbientCall {
                        verb: AmbientVerb::BlobLen { ref_ },
                    })
                }
            }
            "blob.put" => {
                let Value::Bytes(bytes) = field("bytes").ok_or(E)? else {
                    return Err(AmbientError::BadVerbField("bytes must be Bytes"));
                };
                let Value::Text(media_type) = field("mediaType").ok_or(E)? else {
                    return Err(AmbientError::BadVerbField("mediaType must be Text"));
                };
                let ttl = match field("ttl") {
                    None | Some(Value::Null) => None,
                    Some(Value::Int(t)) if *t >= 0 => Some(*t as u64),
                    _ => return Err(AmbientError::BadVerbField("ttl must be Int|Null")),
                };
                Ok(AmbientCall {
                    verb: AmbientVerb::BlobPut {
                        bytes: bytes.clone(),
                        media_type: media_type.clone(),
                        ttl_micros: ttl,
                    },
                })
            }
            "caps" => Ok(AmbientCall { verb: AmbientVerb::Caps }),
            other => Err(AmbientError::UnsupportedVerb(other.to_string())),
        }
    }

    /// The `{"ok": <value>}` response envelope.
    pub fn ok_response(value: Value) -> Value {
        Value::map(vec![("ok".into(), value)]).expect("fixed hygienic key")
    }

    /// The `{"err": "<code>"}` response envelope.
    pub fn err_response(code: &str) -> Value {
        Value::map(vec![("err".into(), Value::Text(code.into()))])
            .expect("fixed hygienic key")
    }

    /// Pull the payload out of a response envelope (guest-side SDK
    /// convenience).
    pub fn response_payload(v: &Value) -> Result<Option<&Value>, String> {
        let Value::Map(pairs) = v else {
            return Err("response not a map".into());
        };
        for (k, val) in pairs {
            match k.as_str() {
                "ok" => return Ok(Some(val)),
                "err" => {
                    return match val.as_text() {
                        Some(code) => Err(code.to_string()),
                        None => Err("err without a code".into()),
                    }
                }
                _ => {}
            }
        }
        Err("response carries neither ok nor err".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode, DecodeOptions};

    fn sample_ref() -> Value {
        Value::BlobRef {
            digest: "a".repeat(64),
            media_type: "text/plain".into(),
            size: 12,
        }
    }

    #[test]
    fn verbs_roundtrip_canonically() {
        for call in [
            AmbientCall {
                verb: AmbientVerb::BlobGet { ref_: sample_ref() },
            },
            AmbientCall {
                verb: AmbientVerb::BlobPut {
                    bytes: b"hello".to_vec(),
                    media_type: "text/plain".into(),
                    ttl_micros: None,
                },
            },
            AmbientCall {
                verb: AmbientVerb::BlobLen { ref_: sample_ref() },
            },
            AmbientCall {
                verb: AmbientVerb::Caps,
            },
        ] {
            let bytes = encode(&call.to_value());
            let back =
                AmbientCall::from_value(&decode(&bytes, &DecodeOptions::default()).expect("canonical"))
                    .expect("roundtrip");
            assert_eq!(call, back);
        }
    }

    #[test]
    fn unknown_verbs_and_bad_fields_are_named_refusals() {
        let foreign = Value::map(vec![("verb".into(), Value::Text("clock.now".into()))]).unwrap();
        assert!(matches!(
            AmbientCall::from_value(&foreign),
            Err(AmbientError::UnsupportedVerb(v)) if v == "clock.now"
        ));
        let no_ref = Value::map(vec![("verb".into(), Value::Text("blob.get".into()))]).unwrap();
        assert!(matches!(
            AmbientCall::from_value(&no_ref),
            Err(AmbientError::BadVerbField(_))
        ));
        let bad_ttl = Value::map(vec![
            ("verb".into(), Value::Text("blob.put".into())),
            ("bytes".into(), Value::Bytes(vec![1])),
            ("mediaType".into(), Value::Text("text/plain".into())),
            ("ttl".into(), Value::Int(-5)),
        ])
        .unwrap();
        assert!(matches!(
            AmbientCall::from_value(&bad_ttl),
            Err(AmbientError::BadVerbField(_))
        ));
    }

    #[test]
    fn responses_carry_ok_or_err() {
        let ok = AmbientCall::ok_response(Value::Bytes(b"state".to_vec()));
        assert_eq!(
            AmbientCall::response_payload(&ok).unwrap(),
            Some(&Value::Bytes(b"state".to_vec()))
        );
        let err = AmbientCall::err_response("Unauthorized");
        assert_eq!(AmbientCall::response_payload(&err).unwrap_err(), "Unauthorized");
        let empty = Value::map(vec![]).unwrap();
        assert!(AmbientCall::response_payload(&empty).is_err());
    }
}
