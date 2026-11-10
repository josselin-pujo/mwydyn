//! ambient-v0 — THE §9.4 AMBIENT IMPORT CONTRACT AS DATA (round 42).
//! The four canonical request forms, the refusal table (with the
//! offending verb's NAME), and the response envelope forms including
//! the exact spelling of every dispatch refusal — pinned so any
//! second-realm decoder or SDK fails the corpus instead of drifting.
//! Canonical bytes are MACHINE-GENERATED (examples/gen-ambient-v0) —
//! never hand-typed.

use mwydyn_data::ambient::{AmbientCall, AmbientError};
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
    ok: Option<Json>,
    #[serde(default)]
    err: Option<String>,
    #[serde(default)]
    canonical_hex: Option<String>,
    #[serde(default)]
    expect_error: Option<String>,
    #[serde(default)]
    expect_verb_name: Option<String>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex byte"))
        .collect()
}

/// Corpus JSON → data-model Value, with the two model desugars
/// (`$hex` → Bytes, `$ref` → a VALIDATED BlobRef — corpus refs are
/// validated, never trusted, same law as the runner's models).
fn json_to_value(j: &Json) -> Value {
    match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Int(n.as_i64().expect("corpus ints")),
        Json::String(s) => Value::Text(s.clone()),
        Json::Array(items) => Value::Array(items.iter().map(json_to_value).collect()),
        Json::Object(map) => {
            if map.len() == 1 {
                if let Some(h) = map.get("$hex") {
                    return Value::Bytes(hex_bytes(h.as_str().expect("$hex is text")));
                }
                if let Some(r) = map.get("$ref") {
                    let digest = r["digest"].as_str().expect("digest");
                    let media_type = r["mediaType"].as_str().expect("mediaType");
                    Value::validate_blob_ref(digest, media_type).expect("corpus ref validates");
                    return Value::BlobRef {
                        digest: digest.into(),
                        media_type: media_type.into(),
                        size: r["size"].as_u64().expect("size"),
                    };
                }
            }
            Value::map(
                map.iter()
                    .map(|(k, v)| (k.clone(), json_to_value(v)))
                    .collect(),
            )
            .expect("hygienic corpus keys")
        }
    }
}

fn err_name(e: &AmbientError) -> &'static str {
    match e {
        AmbientError::BadForm(_) => "BadForm",
        AmbientError::UnsupportedVerb(_) => "UnsupportedVerb",
        AmbientError::BadVerbField(_) => "BadVerbField",
        AmbientError::Decode(_) => "Decode",
    }
}

fn run() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/ambient-v0.json");
    let raw = std::fs::read_to_string(&path).expect("corpus reads");
    let corpus: Corpus = serde_json::from_str(&raw).expect("corpus parses");
    let opts = DecodeOptions::default();
    let mut failures = Vec::new();

    for case in &corpus.cases {
        let canonical = case
            .canonical_hex
            .as_deref()
            .map(hex_bytes)
            .expect("every ambient case pins its bytes");

        match case.kind.as_str() {
            "request" => {
                // garbage bytes: the decoder refuses before any verb
                if case.expect_error.as_deref() == Some("Decode") {
                    match decode(&canonical, &opts) {
                        Err(_) => continue,
                        Ok(_) => {
                            failures.push(format!("{}: garbage decoded", case.name));
                            continue;
                        }
                    }
                }

                let value = json_to_value(case.fields.as_ref().expect("fields"));
                let bytes = encode(&value);

                // byte-identity: the corpus JSON and the pinned hex
                // are the same canonical bytes
                if bytes != canonical {
                    failures.push(format!(
                        "{}: canonical bytes diverged (got {}, want {})",
                        case.name,
                        hex(&bytes),
                        hex(&canonical)
                    ));
                    continue;
                }

                let decoded = decode(&bytes, &opts).expect("canonical corpus bytes decode");
                match AmbientCall::from_value(&decoded) {
                    Err(e) => {
                        let got = err_name(&e);
                        match (&case.expect_error, got) {
                            (Some(expected), got) if expected == got => {}
                            _ => failures.push(format!(
                                "{}: unexpected refusal {e:?} (want {:?})",
                                case.name, case.expect_error
                            )),
                        }
                        // the unknown verb arrives BY NAME
                        if let AmbientError::UnsupportedVerb(name) = &e {
                            if case.expect_verb_name.as_deref() != Some(name.as_str()) {
                                failures.push(format!(
                                    "{}: verb name {name:?}, want {:?}",
                                    case.name, case.expect_verb_name
                                ));
                            }
                        }
                    }
                    Ok(call) => {
                        if case.expect_error.is_some() {
                            failures.push(format!(
                                "{}: expected refusal, decoded {call:?}",
                                case.name
                            ));
                            continue;
                        }
                        // typed roundtrip: re-encode the DECODED call
                        // byte-identically — the typed path and the
                        // raw bytes are one canon
                        if encode(&call.to_value()) != bytes {
                            failures.push(format!(
                                "{}: typed re-encode diverged from corpus bytes",
                                case.name
                            ));
                        }
                    }
                }
            }
            "response" => {
                let envelope = if let Some(ok) = &case.ok {
                    AmbientCall::ok_response(json_to_value(ok))
                } else {
                    let code = case.err.as_deref().expect("ok or err");
                    AmbientCall::err_response(code)
                };
                let bytes = encode(&envelope);
                if bytes != canonical {
                    failures.push(format!(
                        "{}: canonical bytes diverged (got {}, want {})",
                        case.name,
                        hex(&bytes),
                        hex(&canonical)
                    ));
                    continue;
                }
                // the wire roundtrip: decode the pinned bytes back
                // to the same envelope
                let back = decode(&canonical, &opts).expect("canonical response decodes");
                if back != envelope {
                    failures.push(format!(
                        "{}: wire roundtrip diverged: {back:?} vs {envelope:?}",
                        case.name
                    ));
                }
            }
            other => failures.push(format!("{}: unknown kind {other}", case.name)),
        }
    }

    if !failures.is_empty() {
        panic!(
            "ambient-v0: {} vector failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn ambient_v0() {
    run();
}
