//! The author-facing contract: one trait, one context, one effect
//! builder — the plugin author's entire mwydyn vocabulary. Mirrors
//! the native `receive → effects` contract on purpose (compose-
//! parity): knowledge written against this SDK transfers to native
//! actors and back.

use mwydyn_data::ambient::AmbientVerb;
use mwydyn_data::barrier::BarrierEffect;
use mwydyn_data::{address_from_value, Address, DecodeError, EnvelopeError, Letter, Value};

use crate::ambient_call;

/// One actor. A letter arrives (runtime-minted sender rides `cx`);
/// zero or more effects come back, applied atomically by the host —
/// all of them or none of them.
///
/// # Failure is a panic
///
/// There is no error return because a failed transition is not the
/// actor's decision to communicate: **panic** (or
/// [`fail!`](crate::fail)) and the
/// runtime counts a strike, rolls the guest back if it declared
/// statefulness, and supervision decides the rest. A stateful actor
/// that panics mid-letter never observes its own partial mutation.
pub trait Actor {
    /// Handle one letter, return the effects to apply.
    ///
    /// The letter arrives BY VALUE — it is yours (read `kind`, keep
    /// the `correlation`, decode the `payload`). The payload is
    /// canonical CBOR; [`LetterExt::payload_value`] decodes it.
    fn receive(&mut self, letter: Letter, cx: &Cx) -> Vec<Effect>;
}

/// Durable state serialization (§7.6): the actor can save its state
/// as bytes and rehydrate from them across process death. Required
/// by [`durable_entry!`](crate::durable_entry!) at compile time —
/// forgetting your serialization is a build error, not a silent
/// no-op snapshot.
///
/// Your state format is yours; CBOR via [`mwydyn_data::encode`](mwydyn_data::encode()) is
/// the house style. `restore` receives exactly what `snapshot`
/// returned, at boot, before any letter.
pub trait Durable: Actor {
    /// Serialize the current state.
    fn snapshot(&self) -> Vec<u8>;
    /// Replace the current state with the given bytes.
    fn restore(&mut self, state: &[u8]);
}

/// A host refusal, BY NAME (`Unauthorized`, `Missing`, `Integrity`,
/// `UnsupportedVerb`, …) — refusals are observed, never swallowed.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// A content-addressed blob reference (`Value::BlobRef` dressed for
/// work). Blobs never transit messages; they travel as these.
#[derive(Debug, Clone, PartialEq)]
pub struct BlobRef(pub Value);

impl BlobRef {
    /// The SHA-256 digest (64-char lowercase hex).
    pub fn digest(&self) -> &str {
        match &self.0 {
            Value::BlobRef { digest, .. } => digest,
            _ => "",
        }
    }
    /// The hash-bound media type — content identity includes type.
    pub fn media_type(&self) -> &str {
        match &self.0 {
            Value::BlobRef { media_type, .. } => media_type,
            _ => "",
        }
    }
    /// The byte size.
    pub fn size(&self) -> u64 {
        match &self.0 {
            Value::BlobRef { size, .. } => *size,
            _ => 0,
        }
    }
}

impl From<Value> for BlobRef {
    fn from(v: Value) -> Self {
        BlobRef(v)
    }
}

impl From<BlobRef> for Value {
    fn from(r: BlobRef) -> Value {
        r.0
    }
}

/// One capability row the actor holds (`{"verb", "object"}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub verb: String,
    pub object: String,
}

/// What a letter arrives with: the runtime-minted sender and the
/// ambient face (blobs, caps). Everything here is cap-checked by
/// the host; refusals come back by name.
pub struct Cx<'a> {
    from: &'a Address,
}

impl<'a> Cx<'a> {
    /// A `Cx` for a sender. The entry macros construct this per
    /// letter; calling it yourself is for unit-testing your actor
    /// on a host target (where ambient calls refuse by name — the
    /// round-40 stub) and for host-side embedders of SDK logic.
    pub fn new(from: &'a Address) -> Self {
        Cx { from }
    }

    /// The sender's address — the model's return address. Reply
    /// with [`Effect::reply`].
    pub fn sender(&self) -> &Address {
        self.from
    }

    /// Store bytes, get a reference back. Eager: the ref is in your
    /// hand when this returns (allocation is not an effect).
    pub fn blob_put(&self, bytes: Vec<u8>, media_type: &str) -> Result<BlobRef, Refusal> {
        self.blob_put_ttl(bytes, media_type, None)
    }

    /// [`Cx::blob_put`] with a TTL (microseconds).
    pub fn blob_put_ttl(
        &self,
        bytes: Vec<u8>,
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, Refusal> {
        let call = verb_value(AmbientVerb::BlobPut {
            bytes,
            media_type: media_type.to_string(),
            ttl_micros,
        });
        match ambient_call(call) {
            Ok(v @ Value::BlobRef { .. }) => Ok(BlobRef(v)),
            Ok(other) => panic!("mwydyn-guest: blob.put returned {other:?}"),
            Err(code) => Err(Refusal(code)),
        }
    }

    /// Fetch a blob's bytes (loud-missing, integrity-checked).
    pub fn blob_get(&self, r: &BlobRef) -> Result<Vec<u8>, Refusal> {
        let call = verb_value(AmbientVerb::BlobGet { ref_: r.0.clone() });
        match ambient_call(call) {
            Ok(Value::Bytes(b)) => Ok(b),
            Ok(other) => panic!("mwydyn-guest: blob.get returned {other:?}"),
            Err(code) => Err(Refusal(code)),
        }
    }

    /// A blob's size without materializing it.
    pub fn blob_len(&self, r: &BlobRef) -> Result<i64, Refusal> {
        let call = verb_value(AmbientVerb::BlobLen { ref_: r.0.clone() });
        match ambient_call(call) {
            Ok(Value::Int(n)) => Ok(n),
            Ok(other) => panic!("mwydyn-guest: blob.len returned {other:?}"),
            Err(code) => Err(Refusal(code)),
        }
    }

    /// The capability rows this actor holds — self-inspection, free.
    pub fn caps(&self) -> Vec<Row> {
        match ambient_call(verb_value(AmbientVerb::Caps)) {
            Ok(Value::Array(rows)) => rows
                .into_iter()
                .filter_map(|row| {
                    let Value::Map(pairs) = row else {
                        return None;
                    };
                    let get = |k: &str| {
                        pairs
                            .iter()
                            .find(|(name, _)| name == k)
                            .and_then(|(_, v)| match v {
                                Value::Text(t) => Some(t.clone()),
                                _ => None,
                            })
                    };
                    Some(Row {
                        verb: get("verb")?,
                        object: get("object")?,
                    })
                })
                .collect(),
            // caps is free: a refusal here means the parts were not
            // handed over — surface it as zero rows, loudly in docs.
            _ => Vec::new(),
        }
    }
}

fn verb_value(verb: AmbientVerb) -> Value {
    mwydyn_data::ambient::AmbientCall { verb }.to_value()
}

/// One declared effect. Writing is declaring: return effects, the
/// host validates the whole list, then applies it atomically.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Deliver a letter to an address.
    Post { to: Address, letter: Letter },
    /// Best-effort telemetry (§3.6, round 44): `log` and `metric`
    /// records, built via the helpers. The wire form is the DECLARE
    /// form — kind + payload only; the runtime stamps ts/actor/seq.
    Emit { record: mwydyn_data::telemetry::TelemetryRecord },
}

impl Effect {
    /// Post a fresh letter of `kind` with canonical-CBOR `payload`.
    /// `kind` is your schema id (`<namespace>.<name>@<major>`); a
    /// bad kind is your bug — hence the `Result`.
    pub fn post(to: Address, kind: &str, payload: Vec<u8>) -> Result<Effect, EnvelopeError> {
        Ok(Effect::Post {
            to,
            letter: Letter::new(kind, payload, None, None, vec![])?,
        })
    }

    /// A log record (§3.6) — best-effort, lossy by design, never
    /// backpressures your business posts.
    pub fn log(level: mwydyn_data::telemetry::Level, message: impl Into<String>) -> Effect {
        Effect::Emit {
            record: mwydyn_data::telemetry::TelemetryRecord::declared(
                mwydyn_data::telemetry::TelemetryKind::Log,
                mwydyn_data::telemetry::TelPayload::Log {
                    level,
                    message: message.into(),
                    attrs: None,
                },
            ),
        }
    }

    /// A log record with attributes (a canonical map).
    pub fn log_with(
        level: mwydyn_data::telemetry::Level,
        message: impl Into<String>,
        attrs: Value,
    ) -> Effect {
        Effect::Emit {
            record: mwydyn_data::telemetry::TelemetryRecord::declared(
                mwydyn_data::telemetry::TelemetryKind::Log,
                mwydyn_data::telemetry::TelPayload::Log {
                    level,
                    message: message.into(),
                    attrs: Some(attrs),
                },
            ),
        }
    }

    /// A metric observation (§3.6): points-not-aggregates, delta
    /// temporality — the sink aggregates.
    pub fn metric(
        name: impl Into<String>,
        instrument: mwydyn_data::telemetry::Instrument,
        value: f64,
    ) -> Effect {
        Effect::Emit {
            record: mwydyn_data::telemetry::TelemetryRecord::declared(
                mwydyn_data::telemetry::TelemetryKind::Metric,
                mwydyn_data::telemetry::TelPayload::Metric {
                    name: name.into(),
                    instrument,
                    value,
                    unit: None,
                    attrs: None,
                },
            ),
        }
    }

    /// A metric observation with unit and/or attributes.
    pub fn metric_with(
        name: impl Into<String>,
        instrument: mwydyn_data::telemetry::Instrument,
        value: f64,
        unit: Option<&str>,
        attrs: Option<Value>,
    ) -> Effect {
        Effect::Emit {
            record: mwydyn_data::telemetry::TelemetryRecord::declared(
                mwydyn_data::telemetry::TelemetryKind::Metric,
                mwydyn_data::telemetry::TelPayload::Metric {
                    name: name.into(),
                    instrument,
                    value,
                    unit: unit.map(str::to_string),
                    attrs,
                },
            ),
        }
    }

    /// Reply to the SENDER (`cx.sender()`), carrying the incoming
    /// letter's correlation — the model's return-address path. For
    /// routing to a third place by payload convention, see
    /// [`Effect::reply_via`] (spec §9.5).
    pub fn reply(
        letter: &Letter,
        cx: &Cx,
        kind: &str,
        payload: Vec<u8>,
    ) -> Result<Effect, EnvelopeError> {
        Ok(Effect::Post {
            to: cx.sender().clone(),
            letter: Letter::new(kind, payload, None, letter.correlation.clone(), vec![])?,
        })
    }

    /// The §9.5 payload-convention reply: post to the letter's
    /// `replyTo` field, carrying its correlation — the usual way a
    /// plugin reports to the embedding's outbox. `Ok(None)` = the
    /// payload carries no routing; fall through to
    /// [`Effect::reply`] (the sender) or silence.
    pub fn reply_via(
        letter: &Letter,
        kind: &str,
        payload: Vec<u8>,
    ) -> Result<Option<Effect>, EnvelopeError> {
        let Some(to) = letter.reply_to() else {
            return Ok(None);
        };
        Ok(Some(Effect::Post {
            to,
            letter: Letter::new(kind, payload, None, letter.correlation.clone(), vec![])?,
        }))
    }

    /// The barrier wire form (canonical CBOR value).
    pub fn to_value(&self) -> Value {
        match self {
            Effect::Post { to, letter } => BarrierEffect::Post {
                to: to.clone(),
                letter: letter.clone(),
            }
            .to_value(),
            Effect::Emit { record } => BarrierEffect::Emit { record: record.clone() }.to_value(),
        }
    }
}

/// The documented panic: fail a transition loudly (see [`Actor`]).
#[macro_export]
macro_rules! fail {
    ($($arg:tt)*) => {
        panic!("mwydyn-guest: {}", format!($($arg)*))
    };
}

/// Letter ergonomics: decode the payload, read the reply convention.
pub trait LetterExt {
    /// Decode the payload (canonical CBOR — it always is, by the
    /// payload wall). Zero-byte payloads are refused by the wall;
    /// use `Null` for "nothing".
    fn payload_value(&self) -> Result<Value, DecodeError>;

    /// The payload-convention reply target (spec §9.5): an Address
    /// in a `replyTo` field of the payload map — how a letter names
    /// a THIRD place to report to (typically the embedding's
    /// external outbox). Actor-to-actor replies use the sender:
    /// [`Effect::reply`]. `None` = the payload carries no routing.
    fn reply_to(&self) -> Option<Address>;
}

impl LetterExt for Letter {
    fn payload_value(&self) -> Result<Value, DecodeError> {
        mwydyn_data::decode(&self.payload, &mwydyn_data::DecodeOptions::default())
    }

    fn reply_to(&self) -> Option<Address> {
        let Value::Map(pairs) = self.payload_value().ok()? else {
            return None;
        };
        pairs
            .iter()
            .find(|(k, _)| k == "replyTo")
            .and_then(|(_, v)| address_from_value(v))
    }
}

/// Map-field getters for decoded payloads — the daily grind, typed.
pub trait ValueExt {
    /// The field `name` of a map, if present.
    fn field(&self, name: &str) -> Option<&Value>;
    /// A text field.
    fn field_str(&self, name: &str) -> Option<&str>;
    /// A byte field.
    fn field_bytes(&self, name: &str) -> Option<&[u8]>;
    /// An integer field.
    fn field_int(&self, name: &str) -> Option<i64>;
    /// An address field (the envelope's address vocabulary).
    fn field_addr(&self, name: &str) -> Option<Address>;
}

impl ValueExt for Value {
    fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Map(pairs) => pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    fn field_str(&self, name: &str) -> Option<&str> {
        match self.field(name) {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        }
    }

    fn field_bytes(&self, name: &str) -> Option<&[u8]> {
        match self.field(name) {
            Some(Value::Bytes(b)) => Some(b),
            _ => None,
        }
    }

    fn field_int(&self, name: &str) -> Option<i64> {
        match self.field(name) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    fn field_addr(&self, name: &str) -> Option<Address> {
        self.field(name).and_then(address_from_value)
    }
}
