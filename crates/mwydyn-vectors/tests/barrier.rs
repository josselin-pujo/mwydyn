//! barrier-v0 — THE BARRIER CONTRACT AS DATA (round 39). The
//! canonical request/response envelope forms and their refusals,
//! pinned as vectors any implementation can consume: the host
//! decoder, a future second-realm decoder, an external test rig.
//! The canonical bytes are MACHINE-GENERATED (examples/gen-barrier-v0)
//! — never hand-typed.

use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};
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
    side: String,
    #[serde(default)]
    v: i64,
    #[serde(default)]
    from: Option<Json>,
    #[serde(default)]
    letter: Option<Json>,
    #[serde(default)]
    effects: Option<Json>,
    #[serde(default)]
    canonical_hex: Option<String>,
    #[serde(default)]
    expect_error: Option<String>,
}

fn json_to_addr(j: &Json) -> Address {
    match j {
        Json::String(s) => Address::Instance(s.clone()),
        Json::Object(o) => Address::Named {
            name: o["name"].as_str().unwrap().into(),
            context: o["context"].as_str().unwrap().into(),
        },
        _ => panic!("address form"),
    }
}

fn json_to_letter(j: &Json) -> Letter {
    Letter::new(
        j["kind"].as_str().unwrap(),
        hex_bytes(j["payload_hex"].as_str().unwrap()),
        j["cause"].as_str().map(|s| s.to_string()),
        j["correlation"].as_str().map(|s| s.to_string()),
        j["authority"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default(),
    )
    .expect("vector letter")
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex byte"))
        .collect()
}

fn run() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/barrier-v0.json");
    let raw = std::fs::read_to_string(&path).expect("corpus reads");
    let corpus: Corpus = serde_json::from_str(&raw).expect("corpus parses");
    let mut failures = Vec::new();

    for case in &corpus.cases {
        match case.side.as_str() {
            "request" => {
                let mut value = Value::map(vec![
                    ("from".into(), {
                        let a = json_to_addr(case.from.as_ref().expect("from"));
                        mwydyn_data::address_to_value(&a)
                    }),
                    (
                        "letter".into(),
                        match &case.letter {
                            Some(l) => json_to_letter(l).to_value(),
                            None => Value::Null,
                        },
                    ),
                    ("v".into(), Value::Int(case.v)),
                ])
                .unwrap();
                // the missing-letter case: drop the field entirely
                if case.letter.is_none() {
                    if let Value::Map(pairs) = &mut value {
                        pairs.retain(|(k, _)| k != "letter");
                    }
                }
                let bytes = encode(&value);
                if let Some(hex) = &case.canonical_hex {
                    if hex_bytes(hex) != bytes {
                        failures.push(format!("{}: canonical bytes diverged", case.name));
                    }
                }
                let decoded = decode(&bytes, &DecodeOptions::default());
                match (decoded, &case.expect_error) {
                    (Ok(v), None) => {
                        if let Err(e) = BarrierRequest::from_value(&v) {
                            failures.push(format!("{}: request refused: {e:?}", case.name));
                        }
                    }
                    (Ok(_), Some(expected)) => {
                        let got = BarrierRequest::from_value(
                            &decode(&bytes, &DecodeOptions::default()).unwrap(),
                        );
                        let names = |e: &mwydyn_data::barrier::BarrierError| match e {
                            mwydyn_data::barrier::BarrierError::BadVersion(_) => "BadVersion",
                            mwydyn_data::barrier::BarrierError::BadForm(_) => "BadForm",
                            mwydyn_data::barrier::BarrierError::BadLetter(_) => "BadLetter",
                            mwydyn_data::barrier::BarrierError::UnsupportedVerb(_) => {
                                "UnsupportedVerb"
                            }
                        };
                        match got {
                            Err(e) if names(&e) == expected.as_str() => {}
                            other => failures.push(format!(
                                "{}: expected refusal {expected}, got {:?}",
                                case.name, other
                            )),
                        }
                    }
                    (Err(_), Some(expected)) if expected == "Decode" => {}
                    (Err(e), Some(expected)) => failures.push(format!(
                        "{}: expected {expected}, decode failed {:?}",
                        case.name, e
                    )),
                    (Err(e), None) => failures.push(format!("{}: decode failed {e:?}", case.name)),
                }
            }
            "response" => {
                if case.expect_error.as_deref() == Some("Decode") {
                    let bytes = hex_bytes(case.canonical_hex.as_deref().expect("garbage hex"));
                    if decode(&bytes, &DecodeOptions::default()).is_ok() {
                        failures.push(format!("{}: garbage decoded", case.name));
                    }
                    continue;
                }
                let items: Vec<Value> = case
                    .effects
                    .as_ref()
                    .expect("effects")
                    .as_array()
                    .expect("array")
                    .iter()
                    .map(|item| {
                        let entries = item.as_object().expect("effect record");
                        // Reproduce the vector's SHAPE — including
                        // multi-entry records (refused: an effect
                        // record carries exactly one verb).
                        let built: Vec<(String, Value)> = entries
                            .iter()
                            .map(|(verb, body)| {
                                match verb.as_str() {
                                    "post" => {
                                        let to = json_to_addr(&body["to"]);
                                        let letter = json_to_letter(&body["letter"]);
                                        (
                                            verb.clone(),
                                            Value::map(vec![
                                                ("letter".into(), letter.to_value()),
                                                ("to".into(), mwydyn_data::address_to_value(&to)),
                                            ])
                                            .unwrap(),
                                        )
                                    }
                                    other => (other.to_string(), Value::map(vec![]).unwrap()),
                                }
                            })
                            .collect();
                        Value::map(built).unwrap()
                    })
                    .collect();
                let bytes = encode(&Value::Array(items));
                if let Some(hex) = &case.canonical_hex {
                    if hex_bytes(hex) != bytes {
                        failures.push(format!("{}: canonical bytes diverged", case.name));
                    }
                }
                let decoded = decode(&bytes, &DecodeOptions::default());
                match decoded {
                    Err(e) => {
                        if case.expect_error.as_deref() != Some("Decode") {
                            failures.push(format!("{}: decode failed {e:?}", case.name));
                        }
                    }
                    Ok(v) => match BarrierEffect::effects_from_value(&v) {
                        Ok(_) => {
                            if case.expect_error.is_some() {
                                failures.push(format!("{}: expected refusal", case.name));
                            }
                        }
                        Err(e) => {
                            let named = match &e {
                                mwydyn_data::barrier::BarrierError::UnsupportedVerb(_) => {
                                    "UnsupportedVerb".to_string()
                                }
                                mwydyn_data::barrier::BarrierError::BadForm(_) => "BadForm".into(),
                                mwydyn_data::barrier::BarrierError::BadVersion(_) => {
                                    "BadVersion".into()
                                }
                                mwydyn_data::barrier::BarrierError::BadLetter(_) => {
                                    "BadLetter".into()
                                }
                            };
                            let _ = named.clone();
                            match (&case.expect_error, named.as_str()) {
                                (Some(expected), got) if expected == got => {}
                                _ => failures.push(format!(
                                    "{}: expected {:?}, got {named}",
                                    case.name, case.expect_error
                                )),
                            }
                        }
                    },
                }
            }
            other => failures.push(format!("{}: unknown side {other}", case.name)),
        }
    }

    if !failures.is_empty() {
        panic!(
            "barrier-v0: {} vector failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn barrier_v0() {
    run();
}
