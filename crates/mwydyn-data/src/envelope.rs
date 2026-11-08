//! Envelope and letter (spec §2) — the delivery unit's two disjoint
//! field groups, the canonical form checksums ride, and the layer-2
//! kind syntax.
//!
//! Byzantine posture: runtime-assigned fields never ride the guest
//! `Letter`; the host wraps it. "Processed" claims live in §3.1's
//! cursor, not here.

use crate::{sha256_hex, Value};

/// Hard cap on a letter's payload bytes (spec §1: messages are data,
/// not blob containers — large bodies travel by blob reference, §4).
pub const LETTER_PAYLOAD_CAP: usize = 1 << 20;

/// Addresses are data (spec §3.4): readable names are trust-context-
/// scoped, instance ids are opaque and journaled at creation.
#[derive(Debug, Clone, PartialEq)]
pub enum Address {
    Named { context: String, name: String },
    Instance(String),
}

/// The producer-supplied half — exactly what the guest world's
/// `receive` sees wrapped. Payload is canonical CBOR bytes, decoded
/// host-side at the barrier (spec §1).
#[derive(Debug, Clone, PartialEq)]
pub struct Letter {
    /// Layer-2 schema id: `<namespace>.<name>@<major>` (spec §7.9).
    pub kind: String,
    pub payload: Vec<u8>,
    /// Provenance: the message(s) that prompted this one.
    pub cause: Option<String>,
    /// Exchange id — reply matching; dedup is defense-in-depth (§2).
    pub correlation: Option<String>,
    /// Row-cites presented (spec §3.3). Checked live at admission.
    pub authority: Vec<String>,
}

/// The full committed record: letter + runtime-assigned fields.
/// The byzantine invariant (§2) is structural: `Letter` physically
/// cannot carry runtime-assigned fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub letter: Letter,
    /// Identity = (context, seq) — the context journal's contiguous
    /// position (log tier; RAM tier mints ephemeral positions).
    pub seq: u64,
    /// Sender instance id — replies target it; no reply-to field.
    pub from: Address,
    pub to: Address,
    /// RUNTIME-STAMPED (round 26/29): the seq of the letter the
    /// sender was processing when it posted this one — "covers".
    /// Minted in the same transaction as the sender's ack, so a
    /// snapshot letter stamped ack=P provably covers position P.
    /// `None` for letters posted by the embedding.
    pub ack: Option<u64>,
    /// Tagged timestamp, audit only — never a branch input (§2).
    /// `None` on the RAM tier.
    pub commit_ts: Option<i64>,
    /// Sending actor's implementation digest — blue/green and A/B
    /// forensics (§2, round 08).
    pub producer: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeError {
    PayloadTooLarge(usize),
    EmptyKind,
    BadKind(String),
    EmptyReference,
    EmptyProducer,
    /// Canonical-form decoding failure (the SQLite provider stores
    /// envelopes as their canonical CBOR; this is the inverse path).
    BadForm(&'static str),
}

impl Letter {
    /// Structural validation (spec §3.5 admission's letter half).
    /// Pub so the runtime can re-check letters whose fields were set
    /// directly — the core trusts nothing.
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.kind.is_empty() {
            return Err(EnvelopeError::EmptyKind);
        }
        if !valid_kind(&self.kind) {
            return Err(EnvelopeError::BadKind(self.kind.clone()));
        }
        if self.payload.len() > LETTER_PAYLOAD_CAP {
            return Err(EnvelopeError::PayloadTooLarge(self.payload.len()));
        }
        if self.cause.as_ref().is_some_and(|s| s.is_empty())
            || self.correlation.as_ref().is_some_and(|s| s.is_empty())
            || self.authority.iter().any(|a| a.is_empty())
        {
            return Err(EnvelopeError::EmptyReference);
        }
        Ok(())
    }

    pub fn new(
        kind: impl Into<String>,
        payload: Vec<u8>,
        cause: Option<String>,
        correlation: Option<String>,
        authority: Vec<String>,
    ) -> Result<Letter, EnvelopeError> {
        let letter = Letter {
            kind: kind.into(),
            payload,
            cause,
            correlation,
            authority,
        };
        letter.validate()?;
        Ok(letter)
    }
}

impl Envelope {
    pub fn new(
        letter: Letter,
        seq: u64,
        from: Address,
        to: Address,
        ack: Option<u64>,
        commit_ts: Option<i64>,
        producer: impl Into<String>,
    ) -> Result<Envelope, EnvelopeError> {
        let producer = producer.into();
        if producer.is_empty() {
            return Err(EnvelopeError::EmptyProducer);
        }
        Ok(Envelope {
            letter,
            seq,
            from,
            to,
            ack,
            commit_ts,
            producer,
        })
    }

    /// The canonical data-model form checksums ride (spec §2:
    /// integrity over canonical CBOR). Field names are fixed and
    /// hygienic; `Value::map` sorts them canonically.
    pub fn to_value(&self) -> Value {
        let l = &self.letter;
        debug_assert!(self.seq <= i64::MAX as u64);
        let opt_text = |s: &Option<String>| match s {
            Some(s) => Value::Text(s.clone()),
            None => Value::Null,
        };
        let addr = |a: &Address| match a {
            Address::Named { context, name } => Value::map(vec![
                ("name".into(), Value::Text(name.clone())),
                ("context".into(), Value::Text(context.clone())),
            ])
            .expect("fixed hygienic keys"),
            Address::Instance(id) => Value::Text(id.clone()),
        };
        Value::map(vec![
            ("to".into(), addr(&self.to)),
            ("seq".into(), Value::Int(self.seq as i64)),
            ("ack".into(), match self.ack {
                Some(p) => Value::Int(p as i64),
                None => Value::Null,
            }),
            ("from".into(), addr(&self.from)),
            ("kind".into(), Value::Text(l.kind.clone())),
            ("cause".into(), opt_text(&l.cause)),
            ("payload".into(), Value::Bytes(l.payload.clone())),
            (
                "commitTs".into(),
                match self.commit_ts {
                    Some(micros) => Value::Timestamp(micros),
                    None => Value::Null,
                },
            ),
            ("producer".into(), Value::Text(self.producer.clone())),
            (
                "authority".into(),
                Value::Array(
                    l.authority
                        .iter()
                        .map(|a| Value::Text(a.clone()))
                        .collect(),
                ),
            ),
            ("correlation".into(), opt_text(&l.correlation)),
        ])
        .expect("fixed hygienic keys")
    }

    /// Integrity over the canonical CBOR of the envelope (§2, §8 of
    /// A1's event model — the recovered discipline). Assigned at
    /// commit; recomputing it over a copy verifies the bytes.
    pub fn checksum(&self) -> String {
        sha256_hex(&crate::encode(&self.to_value()))
    }

    /// The inverse of [`Envelope::to_value`] — the storage path of
    /// the SQLite provider (round 22/23: envelopes stored as their
    /// canonical CBOR). Shape-validated on the way in; the checksum
    /// is NOT restored — the journal recomputes and verifies.
    pub fn from_value(v: &Value) -> Result<Envelope, EnvelopeError> {
        const E: EnvelopeError = EnvelopeError::BadForm("malformed canonical envelope");
        fn field<'a>(pairs: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
            pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
        }
        fn text<'a>(pairs: &'a [(String, Value)], name: &str) -> Result<&'a str, EnvelopeError> {
            field(pairs, name).and_then(Value::as_text).ok_or(E)
        }
        fn opt_text(
            pairs: &[(String, Value)],
            name: &str,
        ) -> Result<Option<String>, EnvelopeError> {
            match field(pairs, name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Text(s)) => Ok(Some(s.clone())),
                _ => Err(E),
            }
        }
        fn addr(v: &Value) -> Option<Address> {
            match v {
                Value::Text(s) => Some(Address::Instance(s.clone())),
                // canonical key order: "name"(4) < "context"(7)
                Value::Map(p) if p.len() == 2 && p[0].0 == "name" && p[1].0 == "context" => {
                    Some(Address::Named {
                        name: p[0].1.as_text()?.to_string(),
                        context: p[1].1.as_text()?.to_string(),
                    })
                }
                _ => None,
            }
        }
        let Value::Map(pairs) = v else {
            return Err(E);
        };

        let kind = text(pairs, "kind")?.to_string();
        let Value::Bytes(payload) = field(pairs, "payload").ok_or(E)? else {
            return Err(E);
        };
        let payload = payload.clone();
        let cause = opt_text(pairs, "cause")?;
        let correlation = opt_text(pairs, "correlation")?;
        let Value::Array(authority_items) = field(pairs, "authority").ok_or(E)? else {
            return Err(E);
        };
        let mut authority = Vec::new();
        for item in authority_items {
            authority.push(item.as_text().ok_or(E)?.to_string());
        }
        let seq = field(pairs, "seq")
            .and_then(Value::as_int)
            .filter(|n| *n >= 0)
            .ok_or(E)? as u64;
        let ack = match field(pairs, "ack") {
            Some(Value::Int(p)) if *p >= 0 => Some(*p as u64),
            Some(Value::Null) | None => None,
            _ => return Err(E),
        };
        let from = addr(field(pairs, "from").ok_or(E)?).ok_or(E)?;
        let to = addr(field(pairs, "to").ok_or(E)?).ok_or(E)?;
        let commit_ts = match field(pairs, "commitTs") {
            Some(Value::Timestamp(m)) => Some(*m),
            Some(Value::Null) | None => None,
            _ => return Err(E),
        };
        let producer = text(pairs, "producer")?.to_string();
        let letter = Letter {
            kind,
            payload,
            cause,
            correlation,
            authority,
        };
        letter.validate()?;
        Ok(Envelope {
            letter,
            seq,
            from,
            to,
            ack,
            commit_ts,
            producer,
        })
    }
}

/// Layer-2 kind syntax: `<namespace>.<name>@<major>` (§7.9) —
/// segments of `[a-z0-9_-]` joined by dots, one `@`, decimal major.
/// Schema EXISTENCE is the registry's concern (the core is
/// schema-blind); this pins only the syntax.
pub fn valid_kind(kind: &str) -> bool {
    let Some((body, major)) = kind.rsplit_once('@') else {
        return false;
    };
    if body.is_empty()
        || major.is_empty()
        || !major.bytes().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    if !body
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'_' || c == b'-')
    {
        return false;
    }
    !body.starts_with('.') && !body.ends_with('.') && !body.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_letter() -> Letter {
        Letter::new(
            "mwydyn.timer.arm@1",
            vec![0x01, 0x02],
            Some("ctx-a:41".into()),
            Some("ex-7".into()),
            vec!["post:timer".into()],
        )
        .unwrap()
    }

    #[test]
    fn letter_validation() {
        assert!(Letter::new("", vec![], None, None, vec![]).is_err());
        assert!(Letter::new("timer.arm", vec![], None, None, vec![]).is_err()); // no @
        assert!(Letter::new("timer.arm@x", vec![], None, None, vec![]).is_err()); // non-digit major
        assert!(Letter::new("Timer.Arm@1", vec![], None, None, vec![]).is_err()); // uppercase
        assert!(Letter::new("a..b@1", vec![], None, None, vec![]).is_err());
        let big = vec![0u8; LETTER_PAYLOAD_CAP + 1];
        assert!(matches!(
            Letter::new("mwydyn.timer.arm@1", big, None, None, vec![]),
            Err(EnvelopeError::PayloadTooLarge(_))
        ));
        assert!(Letter::new("mwydyn.timer.arm@1", vec![], Some("".into()), None, vec![])
            .is_err());
        assert!(Letter::new("mwydyn.timer.arm@1", vec![], None, None, vec!["".into()])
            .is_err());
        assert_eq!(
            good_letter().payload.len(),
            2,
            "the boundary case: exactly-at-cap must pass"
        );
        let at_cap = vec![0u8; LETTER_PAYLOAD_CAP];
        assert!(Letter::new("mwydyn.timer.arm@1", at_cap, None, None, vec![]).is_ok());
    }

    #[test]
    fn envelope_checksum_is_deterministic_and_binds_fields() {
        let a = Envelope::new(
            good_letter(),
            7,
            Address::Instance("ins-1".into()),
            Address::Named {
                context: "ctx-a".into(),
                name: "ticker".into(),
            },
            Some(5),
            Some(1_770_000_000_123_456),
            "sha256-impl-digest",
        )
        .unwrap();
        let b = a.clone();
        assert_eq!(a.checksum(), b.checksum());

        let mut c = a.clone();
        c.seq = 8;
        assert_ne!(a.checksum(), c.checksum(), "seq is checksum-bound");

        let mut d = a.clone();
        d.commit_ts = None;
        assert_ne!(a.checksum(), d.checksum(), "commitTs is checksum-bound");

        let mut e = a.clone();
        e.ack = None;
        assert_ne!(a.checksum(), e.checksum(), "ack is checksum-bound (round 26/29)");
    }

    #[test]
    fn envelope_canonical_form_roundtrips() {
        let a = Envelope::new(
            good_letter(),
            7,
            Address::Instance("ins-1".into()),
            Address::Named {
                context: "ctx-a".into(),
                name: "ticker".into(),
            },
            Some(5),
            None,
            "sha256-impl-digest",
        )
        .unwrap();
        let v = a.to_value();
        let bytes = crate::encode(&v);
        let v2 = crate::decode(&bytes, &crate::DecodeOptions::default()).unwrap();
        assert_eq!(v, v2, "canonical form survives its own profile");
        let bytes2 = crate::encode(&v2);
        assert_eq!(bytes, bytes2, "re-encode is byte-identical");
    }

    #[test]
    fn kind_syntax_cases() {
        assert!(valid_kind("mwydyn.timer.arm@1"));
        assert!(valid_kind("acme.compiler.ir@12"));
        assert!(valid_kind("x_y@0"));
        assert!(!valid_kind("no-at-sign"));
        assert!(!valid_kind("@1"));
        assert!(!valid_kind("a@"));
        assert!(!valid_kind("a@1b")); // major must be all digits
        assert!(!valid_kind("a@1@2")); // rsplit takes the last @ → body has an @ → bad char
        assert!(!valid_kind(".a@1"));
        assert!(!valid_kind("a.@1"));
        assert!(!valid_kind("a..b@1"));
    }
}
