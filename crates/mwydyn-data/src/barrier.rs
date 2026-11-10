//! The WASM barrier contract (round 33) — canonical CBOR all the
//! way. The barrier has no type system of its own: linear memory
//! carries bytes, and the bytes are THE wire format (canonical CBOR,
//! the same profile as the journal). `Value`-in-memory never crosses
//! a boundary.
//!
//! Two enforced envelope types replace any interface-definition
//! layer (WIT or otherwise):
//!
//! - [`BarrierRequest`] — host → guest, one canonical CBOR map:
//!   `{"from": <address>, "letter": <letter>, "v": <ABI version>}`.
//! - the response — guest → host, one canonical CBOR array of
//!   single-entry maps: `[{"post": {"to": …, "letter": …}}, …]`.
//!
//! Enforcement is the wall: the host decodes both directions through
//! the canonical decoder, then shape-checks. Unknown verbs are
//! refused with a stable error; the payload bytes of a returned
//! letter face the SAME payload wall as native effects (the core
//! refuses non-canonical payloads at admission regardless of actor
//! realm — compose-parity at the gate).
//!
//! ABI mechanics (linear memory, guest exports): `alloc(len) → ptr`,
//! `dealloc(ptr, len)`, `process(req_ptr, req_len) → resp_ptr`, the
//! response block being `[u32 le resp_len][resp bytes]`. Guests
//! compile to plain `wasm32-unknown-unknown` with ZERO imports — no
//! WASI, nothing to fake.

use crate::envelope::{address_from_value, address_to_value, Address, EnvelopeError, Letter};
use crate::Value;

/// The barrier ABI version — the request's `v` field. Mismatches are
/// refused before anything else is interpreted.
pub const BARRIER_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq)]
pub enum BarrierError {
    /// Request/response bytes did not decode under the canonical
    /// profile (or the shape was wrong).
    BadForm(&'static str),
    /// Request carried a `v` this host does not speak.
    BadVersion(i64),
    /// The embedded letter failed structural validation.
    BadLetter(EnvelopeError),
    /// Response named a verb the host does not implement (`post` +
    /// `emit` since round 44; `mint` arrives with its effect; a
    /// runtime-only telemetry kind reads as `emit:<kind>`).
    UnsupportedVerb(String),
}

/// Host → guest: the one call the barrier makes. The runtime-minted
/// fields (sender) ride OUTSIDE the letter — the byzantine posture
/// (§2) holds at the barrier too.
#[derive(Debug, Clone, PartialEq)]
pub struct BarrierRequest {
    pub from: Address,
    pub letter: Letter,
}

impl BarrierRequest {
    pub fn to_value(&self) -> Value {
        Value::map(vec![
            ("from".into(), address_to_value(&self.from)),
            ("letter".into(), self.letter.to_value()),
            ("v".into(), Value::Int(BARRIER_VERSION)),
        ])
        .expect("fixed hygienic keys")
    }

    pub fn from_value(v: &Value) -> Result<BarrierRequest, BarrierError> {
        const E: BarrierError = BarrierError::BadForm("malformed barrier request");
        let Value::Map(pairs) = v else {
            return Err(E);
        };
        let field = |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let Value::Int(v) = field("v").ok_or(E)? else {
            return Err(E);
        };
        if *v != BARRIER_VERSION {
            return Err(BarrierError::BadVersion(*v));
        }
        let from =
            address_from_value(field("from").ok_or(E)?).ok_or(BarrierError::BadForm(
                "malformed barrier request address",
            ))?;
        let letter = Letter::from_value(field("letter").ok_or(E)?)
            .map_err(BarrierError::BadLetter)?;
        Ok(BarrierRequest { from, letter })
    }
}

/// Guest → host: one declared effect. `post` since round 33, `emit`
/// since round 44 (§3.6) — telemetry crosses the barrier as the
/// GUEST-DECLARE form: kind + payload only, log|metric only. The
/// runtime kinds are refused by name (`emit:exchange` & co. — the
/// stable error vectors pin); minted fields never ride at all.
#[derive(Debug, Clone, PartialEq)]
pub enum BarrierEffect {
    Post { to: Address, letter: Letter },
    Emit { record: crate::telemetry::TelemetryRecord },
}

impl BarrierEffect {
    pub fn to_value(&self) -> Value {
        match self {
            BarrierEffect::Post { to, letter } => Value::map(vec![(
                "post".into(),
                Value::map(vec![
                    ("letter".into(), letter.to_value()),
                    ("to".into(), address_to_value(to)),
                ])
                .expect("fixed hygienic keys"),
            )])
            .expect("fixed hygienic keys"),
            BarrierEffect::Emit { record } => Value::map(vec![(
                "emit".into(),
                record.declare_value(),
            )])
            .expect("fixed hygienic keys"),
        }
    }

    /// The response array → declared effects. Every item must be a
    /// single-entry map whose verb the host implements.
    pub fn effects_from_value(v: &Value) -> Result<Vec<BarrierEffect>, BarrierError> {
        const E: BarrierError = BarrierError::BadForm("malformed barrier response");
        let Value::Array(items) = v else {
            return Err(E);
        };
        let mut effects = Vec::with_capacity(items.len());
        for item in items {
            let Value::Map(pairs) = item else {
                return Err(E);
            };
            if pairs.len() != 1 {
                return Err(E);
            }
            let (verb, body) = &pairs[0];
            match verb.as_str() {
                "post" => {
                    let Value::Map(fields) = body else {
                        return Err(E);
                    };
                    let field =
                        |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
                    let to = address_from_value(field("to").ok_or(E)?)
                        .ok_or(BarrierError::BadForm("malformed effect address"))?;
                    let letter = Letter::from_value(field("letter").ok_or(E)?)
                        .map_err(BarrierError::BadLetter)?;
                    effects.push(BarrierEffect::Post { to, letter });
                }
                "emit" => match crate::telemetry::TelemetryRecord::declare_from_value(body) {
                    Ok(record) => effects.push(BarrierEffect::Emit { record }),
                    // Name-bearing refusals, spellings pinned by
                    // telemetry-v0: a runtime kind or an unknown kind
                    // is an unimplemented verb (`emit:exchange`), not
                    // a decode accident.
                    Err(crate::telemetry::TelemetryError::RuntimeKind(k))
                    | Err(crate::telemetry::TelemetryError::UnknownKind(k)) => {
                        return Err(BarrierError::UnsupportedVerb(format!("emit:{k}")))
                    }
                    Err(crate::telemetry::TelemetryError::BadForm(m)) => {
                        return Err(BarrierError::BadForm(m))
                    }
                    Err(crate::telemetry::TelemetryError::Decode(_)) => return Err(E),
                },
                other => return Err(BarrierError::UnsupportedVerb(other.to_string())),
            }
        }
        Ok(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode, DecodeOptions};

    fn sample_letter() -> Letter {
        Letter::new(
            "test.work-order@1",
            vec![0xA0], // empty canonical map — bytes are opaque here
            Some("ctx-a:41".into()),
            Some("ex-9".into()),
            vec!["post:work".into()],
        )
        .unwrap()
    }

    #[test]
    fn request_roundtrips_and_binds_version() {
        let req = BarrierRequest {
            from: Address::Named {
                context: "pp".into(),
                name: "ping".into(),
            },
            letter: sample_letter(),
        };
        let bytes = encode(&req.to_value());
        let back = BarrierRequest::from_value(
            &decode(&bytes, &DecodeOptions::default()).expect("canonical"),
        )
        .expect("roundtrip");
        assert_eq!(req, back);

        // wrong version refused, before anything else is interpreted
        let mut pairs = match req.to_value() {
            Value::Map(pairs) => pairs,
            _ => unreachable!("request is a map"),
        };
        for (k, v) in pairs.iter_mut() {
            if k == "v" {
                *v = Value::Int(99);
            }
        }
        let bad = Value::Map(pairs);
        assert!(matches!(
            BarrierRequest::from_value(&bad),
            Err(BarrierError::BadVersion(99))
        ));
    }

    #[test]
    fn effects_roundtrip_and_refuse_unknown_verbs() {
        let effects = vec![BarrierEffect::Post {
            to: Address::Named {
                context: "pp".into(),
                name: "pong".into(),
            },
            letter: sample_letter(),
        }];
        let v = Value::Array(effects.iter().map(|e| e.to_value()).collect());
        let bytes = encode(&v);
        let back = BarrierEffect::effects_from_value(
            &decode(&bytes, &DecodeOptions::default()).expect("canonical"),
        )
        .expect("roundtrip");
        assert_eq!(effects, back);

        // unknown verb: refused with the verb named
        let foreign = Value::Array(vec![Value::map(vec![(
            "blog".into(),
            Value::map(vec![]).unwrap(),
        )])
        .unwrap()]);
        assert!(matches!(
            BarrierEffect::effects_from_value(&foreign),
            Err(BarrierError::UnsupportedVerb(ref s)) if s == "blog"
        ));

        // two-entry map is not an effect record
        let two_entries = Value::Array(vec![Value::map(vec![
            ("post".into(), Value::map(vec![]).unwrap()),
            ("mint".into(), Value::map(vec![]).unwrap()),
        ])
        .unwrap()]);
        assert!(matches!(
            BarrierEffect::effects_from_value(&two_entries),
            Err(BarrierError::BadForm(_))
        ));
    }

    #[test]
    fn response_rejects_non_canonical_outer_bytes() {
        // a byzantine guest handing back garbage must not decode
        let garbage = [0xFF, 0x00, 0x01];
        assert!(decode(&garbage, &DecodeOptions::default()).is_err());
    }

    /// ROUND 44: the emit verb — declare-form roundtrip, runtime
    /// kinds refused BY NAME, minted fields refused outright.
    #[test]
    fn emit_verb_roundtrips_and_refuses_runtime_kinds() {
        use crate::telemetry::{Level, TelPayload, TelemetryKind, TelemetryRecord};
        let record = TelemetryRecord::declared(
            TelemetryKind::Log,
            TelPayload::Log {
                level: Level::Warn,
                message: "queue depth odd".into(),
                attrs: None,
            },
        );
        let effects = vec![BarrierEffect::Emit { record }];
        let v = Value::Array(effects.iter().map(|e| e.to_value()).collect());
        // The wire form carries NO minted fields.
        let bytes = encode(&v);
        assert!(!bytes.is_empty());
        let back = BarrierEffect::effects_from_value(
            &decode(&bytes, &DecodeOptions::default()).expect("canonical"),
        )
        .expect("roundtrip");
        assert_eq!(effects, back);

        // A runtime kind is refused by name — spellings pinned by
        // telemetry-v0.
        let forged = Value::Array(vec![Value::map(vec![(
            "emit".into(),
            Value::map(vec![
                ("kind".into(), Value::Text("exchange".into())),
                (
                    "exchange".into(),
                    Value::map(vec![
                        ("from".into(), Value::Text("x".into())),
                        ("outcome".into(), Value::Text("committed".into())),
                        ("ts_in".into(), Value::Int(0)),
                    ])
                    .unwrap(),
                ),
            ])
            .unwrap(),
        )])
        .unwrap()]);
        assert!(matches!(
            BarrierEffect::effects_from_value(&forged),
            Err(BarrierError::UnsupportedVerb(ref s)) if s == "emit:exchange"
        ));

        // Minted fields riding the declare form: refused (byzantine
        // posture — attribution is the runtime's to mint).
        let riding = Value::Array(vec![Value::map(vec![(
            "emit".into(),
            Value::map(vec![
                ("actor".into(), Value::Text("forged".into())),
                ("kind".into(), Value::Text("log".into())),
                (
                    "log".into(),
                    Value::map(vec![
                        ("level".into(), Value::Text("info".into())),
                        ("message".into(), Value::Text("hi".into())),
                    ])
                    .unwrap(),
                ),
            ])
            .unwrap(),
        )])
        .unwrap()]);
        assert!(matches!(
            BarrierEffect::effects_from_value(&riding),
            Err(BarrierError::BadForm(_))
        ));
    }

    #[test]
    fn letter_form_roundtrips() {
        let l = sample_letter();
        let bytes = encode(&l.to_value());
        let back =
            Letter::from_value(&decode(&bytes, &DecodeOptions::default()).expect("canonical"))
                .expect("roundtrip");
        assert_eq!(l, back);
        // payload must be Bytes
        let wrong = Value::map(vec![(
            "payload".into(),
            Value::Text("not bytes".into()),
        )])
        .unwrap();
        assert!(Letter::from_value(&wrong).is_err());
    }
}
