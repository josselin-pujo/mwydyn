//! telemetry-v0 — THE §3.6 TELEMETRY RECORD CONTRACT AS DATA (round
//! 44). The full (drained) forms of all six kinds, the guest-declare
//! forms (log | metric only), and the refusal table — pinned so any
//! second-realm decoder or SDK fails the corpus instead of drifting.
//! Canonical bytes are MACHINE-GENERATED (examples/gen-telemetry-v0)
//! — never hand-typed.

use mwydyn_data::telemetry::{TelemetryError, TelemetryRecord};
use mwydyn_data::{decode, encode, DecodeOptions, Value};
use serde::Deserialize;
use serde_json::Value as Json;

#[derive(Deserialize)]
struct Corpus {
    #[allow(dead_code)]
    profile: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    kind: String,
    #[serde(default)]
    fields: Option<Json>,
    #[serde(default)]
    canonical_hex: Option<String>,
    #[serde(default)]
    expect_error: Option<String>,
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex byte"))
        .collect()
}

/// Corpus JSON → data-model Value. Integers stay integers; floats
/// stay floats (the whole-value-is-0xfb law rides on this).
fn json_to_value(j: &Json) -> Value {
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
        Json::Array(items) => Value::Array(items.iter().map(json_to_value).collect()),
        Json::Object(map) => Value::map(
            map.iter().map(|(k, v)| (k.clone(), json_to_value(v))).collect(),
        )
        .expect("hygienic corpus keys"),
    }
}

fn err_name(e: &TelemetryError) -> &'static str {
    match e {
        TelemetryError::BadForm(_) => "BadForm",
        TelemetryError::UnknownKind(_) => "UnknownKind",
        TelemetryError::RuntimeKind(_) => "RuntimeKind",
        TelemetryError::Decode(_) => "Decode",
    }
}

fn run() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/telemetry-v0.json");
    let raw = std::fs::read_to_string(&path).expect("corpus reads");
    let corpus: Corpus = serde_json::from_str(&raw).expect("corpus parses");
    let opts = DecodeOptions::default();
    let mut failures = Vec::new();

    for case in &corpus.cases {
        let canonical = case
            .canonical_hex
            .as_deref()
            .map(hex_bytes)
            .expect("every telemetry case pins its bytes");

        // garbage bytes: the decoder refuses before any interpretation
        if case.expect_error.as_deref() == Some("Decode") {
            match decode(&canonical, &opts) {
                Err(_) => continue,
                Ok(_) => failures.push(format!("{}: garbage decoded", case.name)),
            }
            continue;
        }

        let value = json_to_value(case.fields.as_ref().expect("fields"));
        let bytes = encode(&value);

        // byte-identity: the corpus JSON and the pinned hex are the
        // same canonical bytes
        if bytes != canonical {
            failures.push(format!(
                "{}: canonical bytes diverged (got {}, want {})",
                case.name,
                bytes.iter().map(|x| format!("{x:02x}")).collect::<String>(),
                case.canonical_hex.as_deref().unwrap_or_default(),
            ));
            continue;
        }

        let decoded = decode(&bytes, &opts).expect("canonical corpus bytes decode");
        let outcome = match case.kind.as_str() {
            "record" => TelemetryRecord::from_value(&decoded),
            "declare" => TelemetryRecord::declare_from_value(&decoded),
            other => {
                failures.push(format!("{}: unknown case kind {other}", case.name));
                continue;
            }
        };

        match outcome {
            Err(e) => {
                let got = err_name(&e);
                if case.expect_error.as_deref() != Some(got) {
                    failures.push(format!(
                        "{}: unexpected refusal {e:?} (want {:?})",
                        case.name, case.expect_error
                    ));
                }
            }
            Ok(record) => {
                if case.expect_error.is_some() {
                    failures.push(format!(
                        "{}: expected refusal, decoded {record:?}",
                        case.name
                    ));
                    continue;
                }
                // typed roundtrip: re-encode the DECODED record
                // byte-identically — the typed path and the raw bytes
                // are one canon. Declares re-encode via the DECLARE
                // form (what the barrier carries); records via the
                // full form (what a sink serializes).
                let re = match case.kind.as_str() {
                    "declare" => encode(&record.declare_value()),
                    _ => encode(&record.to_value()),
                };
                if re != bytes {
                    failures.push(format!(
                        "{}: typed re-encode diverged from corpus bytes",
                        case.name
                    ));
                }
                // the declare form arrives UNSTAMPED — minted fields
                // are zero, by construction
                if case.kind == "declare" {
                    let stamped = record.ts != 0 || record.seq != 0 || !record.actor.is_empty();
                    if stamped {
                        failures.push(format!("{}: declare form carried stamps", case.name));
                    }
                }
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "telemetry-v0: {} vector failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn telemetry_v0() {
    run();
}
