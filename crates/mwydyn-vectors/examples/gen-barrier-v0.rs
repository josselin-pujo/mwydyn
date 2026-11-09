//! THE barrier-v0 GENERATOR (round 39) — machine-builds the barrier
//! contract corpus. House rule: never hand-type hex — the canonical
//! bytes come from the encoder itself, the JSON records them, and the
//! harness re-verifies encode() byte-identity forever after.
//!
//!   cargo run -p mwydyn-vectors --example gen-barrier-v0
//!
//! Writes ../../vectors/barrier-v0.json (committed).

use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{encode, Address, Letter, Value};
use serde_json::{json, Value as Json};

fn addr_json(a: &Address) -> Json {
    match a {
        Address::Named { context, name } => {
            json!({"name": name, "context": context})
        }
        Address::Instance(id) => json!(id),
    }
}

fn letter_json(l: &Letter) -> Json {
    json!({
        "kind": l.kind,
        "payload_hex": hex(&l.payload),
        "cause": l.cause,
        "correlation": l.correlation,
        "authority": l.authority,
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    let letter = Letter::new(
        "test.work-order@1",
        vec![0xA1, 0x68, 0x6F, 0x72, 0x64, 0x65, 0x72, 0x49, 0x01], // {"order":1}
        None,
        Some("ex-1".into()),
        vec!["post:work".into()],
    )
    .expect("sample letter");
    let to = Address::Named {
        context: "pp".into(),
        name: "pong".into(),
    };
    let from = Address::Named {
        context: "pp".into(),
        name: "ping".into(),
    };

    // 1. request canonical form (roundtrip + byte-exact)
    let req = BarrierRequest {
        from: from.clone(),
        letter: letter.clone(),
    };
    let req_hex = hex(&encode(&req.to_value()));

    // 2. response canonical form
    let effects = [BarrierEffect::Post {
        to: to.clone(),
        letter: Letter::new(
            "test.receipt@1",
            encode(&Value::map(vec![("total".into(), Value::Float(10.0))]).unwrap()),
            None,
            Some("ex-1".into()),
            vec![],
        )
        .unwrap(),
    }];
    let resp_value = Value::Array(effects.iter().map(|e| e.to_value()).collect());
    let resp_hex = hex(&encode(&resp_value));

    // 3. refusals (shape-level; the decoded forms are reconstructed in
    //    the harness from the same JSON)
    let corpus = json!({
        "profile": "barrier-v0",
        "cases": [
            {
                "name": "request-canonical",
                "side": "request",
                "v": 1,
                "from": addr_json(&from),
                "letter": letter_json(&letter),
                "canonical_hex": req_hex,
            },
            {
                "name": "request-bad-version",
                "side": "request",
                "v": 99,
                "from": addr_json(&from),
                "letter": letter_json(&letter),
                "expect_error": "BadVersion",
            },
            {
                "name": "request-missing-letter",
                "side": "request",
                "v": 1,
                "from": addr_json(&from),
                "expect_error": "BadForm",
            },
            {
                "name": "response-canonical",
                "side": "response",
                "effects": [
                    {"post": {"to": addr_json(&to), "letter": {
                        "kind": "test.receipt@1",
                        "payload_hex": hex(&encode(&Value::map(vec![("total".into(), Value::Float(10.0))]).unwrap())),
                        "cause": null,
                        "correlation": "ex-1",
                        "authority": [],
                    }}}
                ],
                "canonical_hex": resp_hex,
            },
            {
                "name": "response-unknown-verb",
                "side": "response",
                "effects": [{"blog": {}}],
                "expect_error": "UnsupportedVerb",
            },
            {
                "name": "response-two-entry-record",
                "side": "response",
                "effects": [{"post": {"to": addr_json(&to), "letter": letter_json(&letter)}, "mint": {}}],
                "expect_error": "BadForm",
            },
            {
                "name": "response-garbage-bytes",
                "side": "response",
                "canonical_hex": "ff00",
                "expect_error": "Decode",
            }
        ]
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/barrier-v0.json");
    std::fs::write(&path, serde_json::to_string_pretty(&corpus).unwrap() + "\n")
        .expect("corpus writes");
    println!("wrote {}", path.display());
}
