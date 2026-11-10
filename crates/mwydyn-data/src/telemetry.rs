//! The TELEMETRY RECORD (round 44, spec §3.6) — what flows through
//! the telemetry outbox, CBOR all the way like every other boundary.
//!
//! ONE record shape, SIX closed kinds — telemetry is infrastructure
//! data and gets barrier-verb discipline (exact spellings, unknown
//! kinds refused), unlike letters whose `kind` is opaque (round 10:
//! application data):
//!
//! - `log`       {level, message, attrs?}            — actor-declared
//! - `metric`    {name, instrument, value, unit?, attrs?} — actor-declared
//! - `exchange`  {from, ack?, ts_in, outcome}        — RUNTIME-minted
//! - `lifecycle` {event, detail?}                    — RUNTIME-minted
//! - `refusal`   {stage, reason}                     — RUNTIME-minted
//! - `drop`      {dropped, by_kind}                  — RUNTIME-minted
//!
//! The full (drained) form is a canonical map:
//! `{"kind": …, "ts": …, "actor": …, "seq": …, "<kind>": {fields}}`.
//! `ts`/`actor`/`seq` are RUNTIME-MINTED at apply — the byzantine
//! posture (§2) holds at the telemetry boundary too: a guest
//! supplies kind + payload ONLY, and the guest-declare form refuses
//! the minted keys outright (they cannot even ride, accidentally or
//! otherwise). `seq` is the causing letter's seq — the correlation
//! handle sinks join spans by (spec §3.6: actors never see lineage;
//! sinks never touch the journal).
//!
//! Guests may declare ONLY `log` and `metric` — the runtime kinds
//! (`exchange`, `lifecycle`, `refusal`, `drop`) are refused at the
//! barrier, so no actor of any realm can forge runtime facts.

use crate::envelope::{address_from_value, address_to_value, Address};
use crate::{DecodeError, Value};

/// Log severity — five values, mapping 1:1 onto OTel's
/// SeverityNumber checkpoints (1/5/9/13/17) at the mapping layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "trace" => Level::Trace,
            "debug" => Level::Debug,
            "info" => Level::Info,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => return None,
        })
    }
}

/// The metric instrument kind — points-not-aggregates (spec §3.6):
/// each emit is one observation, temporality always delta; the sink
/// aggregates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instrument {
    Counter,
    Gauge,
    Histogram,
}

impl Instrument {
    pub const fn as_str(self) -> &'static str {
        match self {
            Instrument::Counter => "counter",
            Instrument::Gauge => "gauge",
            Instrument::Histogram => "histogram",
        }
    }

    pub fn parse(s: &str) -> Option<Instrument> {
        Some(match s {
            "counter" => Instrument::Counter,
            "gauge" => Instrument::Gauge,
            "histogram" => Instrument::Histogram,
            _ => return None,
        })
    }
}

/// The delivery-attempt outcome — the span skeleton's status field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    Busy,
    Struck,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Committed => "committed",
            Outcome::Busy => "busy",
            Outcome::Struck => "struck",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        Some(match s {
            "committed" => Outcome::Committed,
            "busy" => Outcome::Busy,
            "struck" => Outcome::Struck,
            _ => return None,
        })
    }
}

/// Actor lifecycle facts (§3.2) as telemetry events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    Spawned,
    Crashed,
    Terminal,
    DeadLettered,
}

impl LifecycleEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            LifecycleEvent::Spawned => "spawned",
            LifecycleEvent::Crashed => "crashed",
            LifecycleEvent::Terminal => "terminal",
            LifecycleEvent::DeadLettered => "dead-lettered",
        }
    }

    pub fn parse(s: &str) -> Option<LifecycleEvent> {
        Some(match s {
            "spawned" => LifecycleEvent::Spawned,
            "crashed" => LifecycleEvent::Crashed,
            "terminal" => LifecycleEvent::Terminal,
            "dead-lettered" => LifecycleEvent::DeadLettered,
            _ => return None,
        })
    }
}

/// The closed kind set (spec §3.6). `actor_declared` is the gate the
/// barrier applies: only `log` and `metric` may cross from a guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryKind {
    Log,
    Metric,
    Exchange,
    Lifecycle,
    Refusal,
    Drop,
}

impl TelemetryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            TelemetryKind::Log => "log",
            TelemetryKind::Metric => "metric",
            TelemetryKind::Exchange => "exchange",
            TelemetryKind::Lifecycle => "lifecycle",
            TelemetryKind::Refusal => "refusal",
            TelemetryKind::Drop => "drop",
        }
    }

    pub fn parse(s: &str) -> Option<TelemetryKind> {
        Some(match s {
            "log" => TelemetryKind::Log,
            "metric" => TelemetryKind::Metric,
            "exchange" => TelemetryKind::Exchange,
            "lifecycle" => TelemetryKind::Lifecycle,
            "refusal" => TelemetryKind::Refusal,
            "drop" => TelemetryKind::Drop,
            _ => return None,
        })
    }

    pub const fn actor_declared(self) -> bool {
        matches!(self, TelemetryKind::Log | TelemetryKind::Metric)
    }
}

/// The per-kind payload. `attrs` must be a map when present
/// (enforced at decode; constructed by helpers).
#[derive(Debug, Clone, PartialEq)]
pub enum TelPayload {
    Log {
        level: Level,
        message: String,
        attrs: Option<Value>,
    },
    Metric {
        name: String,
        instrument: Instrument,
        value: f64,
        unit: Option<String>,
        attrs: Option<Value>,
    },
    /// The span skeleton: `from`/`ack` are the causal edge (the
    /// envelope's own fields, round 26/29), `ts_in` the dispatch
    /// wall stamp — the record's `ts` is `ts_out`.
    Exchange {
        from: Address,
        ack: Option<u64>,
        ts_in: i64,
        outcome: Outcome,
    },
    Lifecycle {
        event: LifecycleEvent,
        detail: Option<String>,
    },
    Refusal {
        stage: String,
        reason: String,
    },
    /// The outbox's own loss ledger — a drain that lost records
    /// leads with one of these (spec §3.6).
    Drop {
        dropped: u64,
        by_kind: Vec<(String, u64)>,
    },
}

/// One telemetry record. `ts`/`actor`/`seq` are runtime-minted at
/// apply; actor-declared records carry zeros until then.
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryRecord {
    pub kind: TelemetryKind,
    pub ts: i64,
    pub actor: String,
    pub seq: u64,
    pub payload: TelPayload,
}

impl TelemetryRecord {
    /// An actor-declared record, unstamped — the form helpers build.
    pub fn declared(kind: TelemetryKind, payload: TelPayload) -> TelemetryRecord {
        debug_assert!(kind.actor_declared(), "actors declare only log/metric");
        TelemetryRecord { kind, ts: 0, actor: String::new(), seq: 0, payload }
    }

    /// The full (drained) canonical form.
    pub fn to_value(&self) -> Value {
        let payload = match &self.payload {
            TelPayload::Log { level, message, attrs } => {
                let mut fields = vec![
                    ("level".into(), Value::Text(level.as_str().into())),
                    ("message".into(), Value::Text(message.clone())),
                ];
                if let Some(a) = attrs {
                    fields.push(("attrs".into(), a.clone()));
                }
                Value::map(fields).expect("fixed hygienic keys")
            }
            TelPayload::Metric { name, instrument, value, unit, attrs } => {
                let mut fields = vec![
                    ("instrument".into(), Value::Text(instrument.as_str().into())),
                    ("name".into(), Value::Text(name.clone())),
                    // Always 0xfb float — floatness is preserved, so a
                    // whole-valued observation never flips to an int.
                    ("value".into(), Value::Float(*value)),
                ];
                if let Some(u) = unit {
                    fields.push(("unit".into(), Value::Text(u.clone())));
                }
                if let Some(a) = attrs {
                    fields.push(("attrs".into(), a.clone()));
                }
                Value::map(fields).expect("fixed hygienic keys")
            }
            TelPayload::Exchange { from, ack, ts_in, outcome } => {
                let mut fields = vec![
                    ("from".into(), address_to_value(from)),
                    ("outcome".into(), Value::Text(outcome.as_str().into())),
                    ("ts_in".into(), Value::Int(*ts_in)),
                ];
                if let Some(a) = ack {
                    fields.push(("ack".into(), Value::Int(*a as i64)));
                }
                Value::map(fields).expect("fixed hygienic keys")
            }
            TelPayload::Lifecycle { event, detail } => {
                let mut fields =
                    vec![("event".into(), Value::Text(event.as_str().into()))];
                if let Some(d) = detail {
                    fields.push(("detail".into(), Value::Text(d.clone())));
                }
                Value::map(fields).expect("fixed hygienic keys")
            }
            TelPayload::Refusal { stage, reason } => Value::map(vec![
                ("reason".into(), Value::Text(reason.clone())),
                ("stage".into(), Value::Text(stage.clone())),
            ])
            .expect("fixed hygienic keys"),
            TelPayload::Drop { dropped, by_kind } => {
                // Canonical order = map order (key length, then
                // bytes) — normalize so the wire form is stable
                // regardless of how the caller accumulated counts.
                let mut sorted = by_kind.clone();
                sorted.sort_by(|a, b| (a.0.len(), &a.0).cmp(&(b.0.len(), &b.0)));
                Value::map(vec![
                    (
                        "by_kind".into(),
                        Value::map(
                            sorted
                                .iter()
                                .map(|(k, n)| (k.clone(), Value::Int(*n as i64)))
                                .collect(),
                        )
                        .expect("sorted unique keys by construction"),
                    ),
                    ("dropped".into(), Value::Int(*dropped as i64)),
                ])
                .expect("fixed hygienic keys")
            }
        };
        Value::map(vec![
            ("actor".into(), Value::Text(self.actor.clone())),
            ("kind".into(), Value::Text(self.kind.as_str().into())),
            ("seq".into(), Value::Int(self.seq as i64)),
            ("ts".into(), Value::Int(self.ts)),
            (self.kind.as_str().into(), payload),
        ])
        .expect("fixed hygienic keys")
    }

    /// Decode the full form — what vectors and serializing sinks
    /// consume.
    pub fn from_value(v: &Value) -> Result<TelemetryRecord, TelemetryError> {
        const E: TelemetryError = TelemetryError::BadForm("malformed telemetry record");
        let Value::Map(pairs) = v else {
            return Err(E);
        };
        let field = |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let Value::Text(kind_s) = field("kind").ok_or(E)? else {
            return Err(E);
        };
        let kind = TelemetryKind::parse(kind_s).ok_or_else(|| TelemetryError::UnknownKind(kind_s.clone()))?;
        let Value::Int(ts) = field("ts").ok_or(E)? else {
            return Err(E);
        };
        let Value::Text(actor) = field("actor").ok_or(E)? else {
            return Err(E);
        };
        let Value::Int(seq) = field("seq").ok_or(E)? else {
            return Err(E);
        };
        let body = field(kind.as_str()).ok_or(E)?;
        let Value::Map(fields) = body else {
            return Err(E);
        };
        let f = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let attrs_of = |v: &Value| -> Result<Option<Value>, TelemetryError> {
            match v {
                Value::Null => Ok(None),
                Value::Map(_) => Ok(Some(v.clone())),
                _ => Err(TelemetryError::BadForm("attrs must be a map")),
            }
        };
        let payload = match kind {
            TelemetryKind::Log => {
                let Value::Text(level_s) = f("level").ok_or(E)? else {
                    return Err(E);
                };
                let level = Level::parse(level_s)
                    .ok_or(TelemetryError::BadForm("unknown log level"))?;
                let Value::Text(message) = f("message").ok_or(E)?.clone() else {
                    return Err(E);
                };
                let attrs = f("attrs").map(attrs_of).transpose()?.flatten();
                TelPayload::Log { level, message, attrs }
            }
            TelemetryKind::Metric => {
                let Value::Text(name) = f("name").ok_or(E)?.clone() else {
                    return Err(E);
                };
                let Value::Text(instr_s) = f("instrument").ok_or(E)? else {
                    return Err(E);
                };
                let instrument = Instrument::parse(instr_s)
                    .ok_or(TelemetryError::BadForm("unknown instrument"))?;
                let value = match f("value").ok_or(E)? {
                    Value::Float(x) => *x,
                    Value::Int(i) => *i as f64,
                    _ => return Err(E),
                };
                let unit = match f("unit") {
                    Some(Value::Text(u)) => Some(u.clone()),
                    None => None,
                    _ => return Err(E),
                };
                let attrs = f("attrs").map(attrs_of).transpose()?.flatten();
                TelPayload::Metric { name, instrument, value, unit, attrs }
            }
            TelemetryKind::Exchange => {
                let from = address_from_value(f("from").ok_or(E)?)
                    .ok_or(TelemetryError::BadForm("malformed exchange from"))?;
                let ack = match f("ack") {
                    Some(Value::Int(a)) => Some(*a as u64),
                    None => None,
                    _ => return Err(E),
                };
                let Value::Int(ts_in) = f("ts_in").ok_or(E)? else {
                    return Err(E);
                };
                let Value::Text(outcome_s) = f("outcome").ok_or(E)? else {
                    return Err(E);
                };
                let outcome = Outcome::parse(outcome_s)
                    .ok_or(TelemetryError::BadForm("unknown outcome"))?;
                TelPayload::Exchange { from, ack, ts_in: *ts_in, outcome }
            }
            TelemetryKind::Lifecycle => {
                let Value::Text(event_s) = f("event").ok_or(E)? else {
                    return Err(E);
                };
                let event = LifecycleEvent::parse(event_s)
                    .ok_or(TelemetryError::BadForm("unknown lifecycle event"))?;
                let detail = match f("detail") {
                    Some(Value::Text(d)) => Some(d.clone()),
                    None => None,
                    _ => return Err(E),
                };
                TelPayload::Lifecycle { event, detail }
            }
            TelemetryKind::Refusal => {
                let Value::Text(stage) = f("stage").ok_or(E)?.clone() else {
                    return Err(E);
                };
                let Value::Text(reason) = f("reason").ok_or(E)?.clone() else {
                    return Err(E);
                };
                TelPayload::Refusal { stage, reason }
            }
            TelemetryKind::Drop => {
                let Value::Int(dropped) = f("dropped").ok_or(E)? else {
                    return Err(E);
                };
                let Value::Map(by) = f("by_kind").ok_or(E)? else {
                    return Err(E);
                };
                let mut by_kind = Vec::with_capacity(by.len());
                for (k, v) in by {
                    let Value::Int(n) = v else {
                        return Err(E);
                    };
                    by_kind.push((k.clone(), *n as u64));
                }
                // Map order is canonical (key length, then bytes) —
                // sort defensively so in-memory equality holds.
                by_kind.sort_by(|a, b| (a.0.len(), &a.0).cmp(&(b.0.len(), &b.0)));
                TelPayload::Drop { dropped: *dropped as u64, by_kind }
            }
        };
        Ok(TelemetryRecord { kind, ts: *ts, actor: actor.clone(), seq: *seq as u64, payload })
    }

    /// Decode the GUEST-DECLARE form — kind + payload only, no
    /// minted fields (their presence is refused: byzantine posture),
    /// and only actor-declarable kinds (`log` | `metric`) — the
    /// runtime kinds are runtime-minted, never guest-carried.
    pub fn declare_from_value(v: &Value) -> Result<TelemetryRecord, TelemetryError> {
        const E: TelemetryError = TelemetryError::BadForm("malformed telemetry declare");
        let Value::Map(pairs) = v else {
            return Err(E);
        };
        if pairs.iter().any(|(k, _)| matches!(k.as_str(), "ts" | "actor" | "seq")) {
            return Err(TelemetryError::BadForm("minted fields cannot ride the declare form"));
        }
        let Value::Text(kind_s) = pairs.iter().find(|(k, _)| k == "kind").map(|(_, v)| v).ok_or(E)? else {
            return Err(E);
        };
        let kind = TelemetryKind::parse(kind_s)
            .ok_or_else(|| TelemetryError::UnknownKind(kind_s.clone()))?;
        if !kind.actor_declared() {
            return Err(TelemetryError::RuntimeKind(kind_s.clone()));
        }
        let full = Value::map(vec![
            ("actor".into(), Value::Text(String::new())),
            ("kind".into(), Value::Text(kind_s.clone())),
            ("seq".into(), Value::Int(0)),
            ("ts".into(), Value::Int(0)),
            (
                kind_s.clone(),
                pairs
                    .iter()
                    .find(|(k, _)| k == kind_s)
                    .map(|(_, v)| v.clone())
                    .ok_or(E)?,
            ),
        ])
        .expect("fixed hygienic keys");
        let mut record = TelemetryRecord::from_value(&full)?;
        record.ts = 0;
        record.actor = String::new();
        record.seq = 0;
        Ok(record)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TelemetryError {
    /// Canonical decode failed (or the shape was wrong).
    BadForm(&'static str),
    /// The kind is not in the closed set.
    UnknownKind(String),
    /// A guest declared a runtime-only kind (log/metric only).
    RuntimeKind(String),
    /// Convenience passthrough for callers decoding raw bytes.
    Decode(DecodeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode, DecodeOptions};

    fn roundtrip(record: &TelemetryRecord) {
        let bytes = encode(&record.to_value());
        let back = decode(&bytes, &DecodeOptions::default()).expect("decodes");
        let r2 = TelemetryRecord::from_value(&back).expect("roundtrips");
        assert_eq!(&r2, record);
    }

    fn log_record() -> TelemetryRecord {
        TelemetryRecord {
            kind: TelemetryKind::Log,
            ts: 1234,
            actor: "actor-7".into(),
            seq: 42,
            payload: TelPayload::Log {
                level: Level::Warn,
                message: "half-open".into(),
                attrs: Some(Value::map(vec![("retry".into(), Value::Int(2))]).unwrap()),
            },
        }
    }

    #[test]
    fn log_roundtrip() {
        roundtrip(&log_record());
    }

    #[test]
    fn metric_roundtrip_whole_value_stays_float() {
        let r = TelemetryRecord {
            kind: TelemetryKind::Metric,
            ts: 1,
            actor: "a".into(),
            seq: 2,
            payload: TelPayload::Metric {
                name: "orders.seen".into(),
                instrument: Instrument::Counter,
                value: 3.0,
                unit: Some("{orders}".into()),
                attrs: None,
            },
        };
        roundtrip(&r);
        let bytes = encode(&r.to_value());
        assert!(bytes.contains(&0xfb), "whole metric value stays 0xfb float");
    }

    #[test]
    fn exchange_roundtrip_root_and_parented() {
        for ack in [None, Some(41u64)] {
            roundtrip(&TelemetryRecord {
                kind: TelemetryKind::Exchange,
                ts: 99,
                actor: "worker".into(),
                seq: 42,
                payload: TelPayload::Exchange {
                    from: Address::Instance("external".into()),
                    ack,
                    ts_in: 90,
                    outcome: Outcome::Committed,
                },
            });
        }
    }

    #[test]
    fn lifecycle_refusal_drop_roundtrip() {
        roundtrip(&TelemetryRecord {
            kind: TelemetryKind::Lifecycle,
            ts: 5,
            actor: "a".into(),
            seq: 0,
            payload: TelPayload::Lifecycle {
                event: LifecycleEvent::Crashed,
                detail: Some("restarted fresh, attempt 2".into()),
            },
        });
        roundtrip(&TelemetryRecord {
            kind: TelemetryKind::Refusal,
            ts: 5,
            actor: "a".into(),
            seq: 7,
            payload: TelPayload::Refusal {
                stage: "effects".into(),
                reason: "unknown target in effects".into(),
            },
        });
        roundtrip(&TelemetryRecord {
            kind: TelemetryKind::Drop,
            ts: 5,
            actor: String::new(),
            seq: 0,
            payload: TelPayload::Drop {
                dropped: 12,
                by_kind: vec![("log".into(), 3), ("exchange".into(), 9)],
            },
        });
    }

    #[test]
    fn unknown_kind_refused_by_name() {
        let v = Value::map(vec![
            ("actor".into(), Value::Text("a".into())),
            ("kind".into(), Value::Text("spans".into())),
            ("seq".into(), Value::Int(0)),
            ("ts".into(), Value::Int(0)),
        ])
        .unwrap();
        assert_eq!(
            TelemetryRecord::from_value(&v),
            Err(TelemetryError::UnknownKind("spans".into()))
        );
    }

    #[test]
    fn declare_form_accepts_log_and_metric_only() {
        let log_declare = Value::map(vec![
            (
                "kind".into(),
                Value::Text("log".into()),
            ),
            (
                "log".into(),
                Value::map(vec![
                    ("level".into(), Value::Text("info".into())),
                    ("message".into(), Value::Text("hi".into())),
                ])
                .unwrap(),
            ),
        ])
        .unwrap();
        let r = TelemetryRecord::declare_from_value(&log_declare).expect("log declares");
        assert_eq!(r.kind, TelemetryKind::Log);
        assert_eq!((r.ts, r.seq, r.actor.as_str()), (0, 0, ""));

        for runtime_kind in ["exchange", "lifecycle", "refusal", "drop"] {
            let v = Value::map(vec![
                ("kind".into(), Value::Text(runtime_kind.into())),
                (
                    runtime_kind.into(),
                    Value::map(vec![("dropped".into(), Value::Int(0))]).unwrap(),
                ),
            ])
            .unwrap();
            assert_eq!(
                TelemetryRecord::declare_from_value(&v),
                Err(TelemetryError::RuntimeKind(runtime_kind.into())),
                "{runtime_kind} must not be guest-declarable"
            );
        }
    }

    #[test]
    fn declare_form_refuses_minted_fields() {
        let v = Value::map(vec![
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
        .unwrap();
        assert!(matches!(
            TelemetryRecord::declare_from_value(&v),
            Err(TelemetryError::BadForm(_))
        ));
    }

    #[test]
    fn attrs_must_be_a_map() {
        let v = Value::map(vec![
            ("actor".into(), Value::Text("a".into())),
            ("kind".into(), Value::Text("log".into())),
            ("seq".into(), Value::Int(1)),
            ("ts".into(), Value::Int(1)),
            (
                "log".into(),
                Value::map(vec![
                    ("attrs".into(), Value::Text("not a map".into())),
                    ("level".into(), Value::Text("info".into())),
                    ("message".into(), Value::Text("hi".into())),
                ])
                .unwrap(),
            ),
        ])
        .unwrap();
        assert_eq!(
            TelemetryRecord::from_value(&v),
            Err(TelemetryError::BadForm("attrs must be a map"))
        );
    }
}
