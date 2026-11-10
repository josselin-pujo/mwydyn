//! THE telemetry-v0 GENERATOR (round 44) — machine-builds the §3.6
//! telemetry-record corpus. House rule: never hand-type hex — the
//! canonical bytes come from the encoder over the TYPED records, and
//! the harness re-verifies byte-identity forever after.
//!
//!   cargo run -p mwydyn-vectors --example gen-telemetry-v0
//!
//! Writes ../../vectors/telemetry-v0.json (committed).

use mwydyn_data::telemetry::{
    Instrument, Level, LifecycleEvent, Outcome, TelPayload, TelemetryKind, TelemetryRecord,
};
use mwydyn_data::{encode, Value};
use serde_json::{json, Value as Json};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sample_log(attrs: Option<Value>) -> TelemetryRecord {
    TelemetryRecord {
        kind: TelemetryKind::Log,
        ts: 1_772_000_000_000_000,
        actor: "ins-7".into(),
        seq: 42,
        payload: TelPayload::Log {
            level: Level::Warn,
            message: "queue depth odd".into(),
            attrs,
        },
    }
}

/// Corpus JSON → Value with the ONE desugar this corpus needs:
/// floats (metric values) — integers stay integers.
fn desugar(j: &Json) -> Value {
    match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().expect("finite corpus floats"))
            }
        }
        Json::String(s) => Value::Text(s.clone()),
        Json::Array(items) => Value::Array(items.iter().map(desugar).collect()),
        Json::Object(map) => Value::map(
            map.iter().map(|(k, v)| (k.clone(), desugar(v))).collect(),
        )
        .expect("hygienic keys"),
    }
}

/// A full-form record case: fields JSON mirrors the typed record;
/// canonical_hex comes from the encoder over the record ITSELF.
fn record_case(name: &str, record: &TelemetryRecord, fields: Json) -> Json {
    json!({
        "name": name,
        "kind": "record",
        "fields": fields,
        "canonical_hex": hex(&encode(&record.to_value())),
    })
}

/// A guest-declare case: the barrier's wire form, machine-encoded
/// from the typed record via declare_value.
fn declare_case(name: &str, record: &TelemetryRecord, fields: Json) -> Json {
    json!({
        "name": name,
        "kind": "declare",
        "fields": fields,
        "canonical_hex": hex(&encode(&record.declare_value())),
    })
}

/// A refusal-shape case — canonical bytes pinned from the desugared
/// fields (malformed SHAPES are canonical bytes too).
fn refuse_case(name: &str, form: &str, fields: Json, error: &str) -> Json {
    let bytes = encode(&desugar(&fields));
    json!({
        "name": name,
        "kind": form,
        "fields": fields,
        "canonical_hex": hex(&bytes),
        "expect_error": error,
    })
}

fn main() {
    let attrs = Value::map(vec![("attempt".into(), Value::Int(2))]).unwrap();

    let mut cases = vec![
        // ---- the full (drained) forms, one per kind
        record_case(
            "record-log-attrs",
            &sample_log(Some(attrs.clone())),
            json!({
                "actor": "ins-7", "kind": "log", "seq": 42, "ts": 1772000000000000i64,
                "log": {"attrs": {"attempt": 2}, "level": "warn", "message": "queue depth odd"},
            }),
        ),
        record_case(
            "record-log-no-attrs",
            &sample_log(None),
            json!({
                "actor": "ins-7", "kind": "log", "seq": 42, "ts": 1772000000000000i64,
                "log": {"level": "warn", "message": "queue depth odd"},
            }),
        ),
        record_case(
            "record-metric-counter-whole-value-is-float",
            &TelemetryRecord {
                kind: TelemetryKind::Metric,
                ts: 5,
                actor: "ins-2".into(),
                seq: 7,
                payload: TelPayload::Metric {
                    name: "letters.seen".into(),
                    instrument: Instrument::Counter,
                    value: 3.0,
                    unit: None,
                    attrs: None,
                },
            },
            json!({
                "actor": "ins-2", "kind": "metric", "seq": 7, "ts": 5,
                "metric": {"instrument": "counter", "name": "letters.seen", "value": 3.0},
            }),
        ),
        record_case(
            "record-metric-gauge-unit",
            &TelemetryRecord {
                kind: TelemetryKind::Metric,
                ts: 5,
                actor: "ins-2".into(),
                seq: 7,
                payload: TelPayload::Metric {
                    name: "queue.depth".into(),
                    instrument: Instrument::Gauge,
                    value: 0.25,
                    unit: Some("{letters}".into()),
                    attrs: None,
                },
            },
            json!({
                "actor": "ins-2", "kind": "metric", "seq": 7, "ts": 5,
                "metric": {
                    "instrument": "gauge", "name": "queue.depth",
                    "unit": "{letters}", "value": 0.25,
                },
            }),
        ),
        record_case(
            "record-exchange-root",
            &TelemetryRecord {
                kind: TelemetryKind::Exchange,
                ts: 99,
                actor: "worker".into(),
                seq: 42,
                payload: TelPayload::Exchange {
                    from: mwydyn_data::Address::Instance("external".into()),
                    ack: None,
                    ts_in: 90,
                    outcome: Outcome::Committed,
                },
            },
            json!({
                "actor": "worker", "kind": "exchange", "seq": 42, "ts": 99,
                "exchange": {
                    "from": "external",
                    "outcome": "committed", "ts_in": 90,
                },
            }),
        ),
        record_case(
            "record-exchange-parented-busy",
            &TelemetryRecord {
                kind: TelemetryKind::Exchange,
                ts: 99,
                actor: "worker".into(),
                seq: 43,
                payload: TelPayload::Exchange {
                    from: mwydyn_data::Address::Instance("external".into()),
                    ack: Some(42),
                    ts_in: 91,
                    outcome: Outcome::Busy,
                },
            },
            json!({
                "actor": "worker", "kind": "exchange", "seq": 43, "ts": 99,
                "exchange": {
                    "ack": 42, "from": "external",
                    "outcome": "busy", "ts_in": 91,
                },
            }),
        ),
        record_case(
            "record-lifecycle-crashed",
            &TelemetryRecord {
                kind: TelemetryKind::Lifecycle,
                ts: 12,
                actor: "ins-9".into(),
                seq: 4,
                payload: TelPayload::Lifecycle {
                    event: LifecycleEvent::Crashed,
                    detail: Some("restarted fresh, attempt 2".into()),
                },
            },
            json!({
                "actor": "ins-9", "kind": "lifecycle", "seq": 4, "ts": 12,
                "lifecycle": {"detail": "restarted fresh, attempt 2", "event": "crashed"},
            }),
        ),
        record_case(
            "record-refusal-effects",
            &TelemetryRecord {
                kind: TelemetryKind::Refusal,
                ts: 12,
                actor: "ins-9".into(),
                seq: 4,
                payload: TelPayload::Refusal {
                    stage: "effects".into(),
                    reason: "unknown target in effects — attempt 1".into(),
                },
            },
            json!({
                "actor": "ins-9", "kind": "refusal", "seq": 4, "ts": 12,
                "refusal": {"reason": "unknown target in effects — attempt 1", "stage": "effects"},
            }),
        ),
        // by_kind carries canonical map order (key length, bytes)
        record_case(
            "record-drop-ledger",
            &TelemetryRecord {
                kind: TelemetryKind::Drop,
                ts: 100,
                actor: String::new(),
                seq: 0,
                payload: TelPayload::Drop {
                    dropped: 21,
                    by_kind: vec![
                        ("log".into(), 12),
                        ("exchange".into(), 9),
                    ],
                },
            },
            json!({
                "actor": "", "kind": "drop", "seq": 0, "ts": 100,
                "drop": {"by_kind": {"log": 12, "exchange": 9}, "dropped": 21},
            }),
        ),
        // ---- the guest-declare forms (log | metric ONLY)
        declare_case(
            "declare-log",
            &sample_log(None),
            json!({
                "kind": "log",
                "log": {"level": "warn", "message": "queue depth odd"},
            }),
        ),
        declare_case(
            "declare-metric",
            &TelemetryRecord {
                kind: TelemetryKind::Metric,
                ts: 0,
                actor: String::new(),
                seq: 0,
                payload: TelPayload::Metric {
                    name: "letters.seen".into(),
                    instrument: Instrument::Counter,
                    value: 1.0,
                    unit: None,
                    attrs: None,
                },
            },
            json!({
                "kind": "metric",
                "metric": {"instrument": "counter", "name": "letters.seen", "value": 1.0},
            }),
        ),
        // ---- the refusal table
        refuse_case(
            "declare-exchange-runtime-kind",
            "declare",
            json!({
                "kind": "exchange",
                "exchange": {"from": "external", "outcome": "committed", "ts_in": 0},
            }),
            "RuntimeKind",
        ),
        refuse_case(
            "declare-drop-runtime-kind",
            "declare",
            json!({"kind": "drop", "drop": {"by_kind": {}, "dropped": 1}}),
            "RuntimeKind",
        ),
        refuse_case(
            "declare-minted-fields-ride",
            "declare",
            json!({
                "actor": "forged", "kind": "log",
                "log": {"level": "info", "message": "hi"},
            }),
            "BadForm",
        ),
        refuse_case(
            "declare-unknown-kind",
            "declare",
            json!({"kind": "spans", "spans": {}}),
            "UnknownKind",
        ),
        refuse_case(
            "record-unknown-kind",
            "record",
            json!({"actor": "a", "kind": "trace", "seq": 0, "ts": 0}),
            "UnknownKind",
        ),
        refuse_case(
            "record-attrs-not-a-map",
            "record",
            json!({
                "actor": "a", "kind": "log", "seq": 1, "ts": 1,
                "log": {"attrs": "no", "level": "info", "message": "hi"},
            }),
            "BadForm",
        ),
        json!({
            "name": "garbage-bytes",
            "kind": "record",
            "canonical_hex": "ff00",
            "expect_error": "Decode",
        }),
    ];

    // One more refusal the fields-desugar cannot express (floats in
    // a where ints belong): outcome wrong type.
    cases.push(refuse_case(
        "record-outcome-not-text",
        "record",
        json!({
            "actor": "w", "kind": "exchange", "seq": 1, "ts": 1,
            "exchange": {"from": "external", "outcome": 3, "ts_in": 0},
        }),
        "BadForm",
    ));

    let corpus = json!({"profile": "telemetry-v0", "cases": cases});
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/telemetry-v0.json");
    std::fs::write(&path, serde_json::to_string_pretty(&corpus).unwrap() + "\n")
        .expect("corpus writes");
    println!("wrote {} ({} cases)", path.display(), corpus["cases"].as_array().unwrap().len());
}
