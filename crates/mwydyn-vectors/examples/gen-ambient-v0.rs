//! THE ambient-v0 GENERATOR (round 42) — machine-builds the §9.4
//! ambient-import contract corpus. House rule: never hand-type hex —
//! canonical bytes come from the encoder itself, digests from the
//! data model's own `blob_digest` (content identity includes type),
//! and the harness re-verifies encode() byte-identity forever after.
//!
//!   cargo run -p mwydyn-vectors --example gen-ambient-v0
//!
//! Writes ../../vectors/ambient-v0.json (committed).

use mwydyn_data::ambient::{AmbientCall, AmbientVerb};
use mwydyn_data::{encode, Value};
use serde_json::{json, Value as Json};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The corpus's fixed blob identity — a REAL digest, machine-computed.
const BODY: &[u8] = b"ambient-vector-body";
const MEDIA_TYPE: &str = "text/plain";

fn sample_ref() -> Value {
    let digest = Value::blob_digest(BODY, MEDIA_TYPE).expect("digest");
    Value::BlobRef {
        digest,
        media_type: MEDIA_TYPE.into(),
        size: BODY.len() as u64,
    }
}

/// The request map as corpus JSON (with $ref/$hex desugars the
/// harness converts back).
fn request_json(call: &AmbientCall) -> Json {
    match &call.verb {
        AmbientVerb::BlobGet { ref_ } => json!({
            "verb": "blob.get",
            "ref": {"$ref": ref_parts(ref_)},
        }),
        AmbientVerb::BlobLen { ref_ } => json!({
            "verb": "blob.len",
            "ref": {"$ref": ref_parts(ref_)},
        }),
        AmbientVerb::BlobPut {
            bytes,
            media_type,
            ttl_micros,
        } => json!({
            "verb": "blob.put",
            "bytes": {"$hex": hex(bytes)},
            "mediaType": media_type,
            "ttl": ttl_micros.map(|t| json!(t as i64)).unwrap_or(Json::Null),
        }),
        AmbientVerb::Caps => json!({"verb": "caps"}),
    }
}

fn ref_parts(v: &Value) -> Json {
    let Value::BlobRef {
        digest,
        media_type,
        size,
    } = v
    else {
        unreachable!("sample refs are refs")
    };
    json!({"digest": digest, "mediaType": media_type, "size": size})
}

fn req_case(name: &str, call: &AmbientCall) -> Json {
    json!({
        "name": name,
        "kind": "request",
        "fields": request_json(call),
        "canonical_hex": hex(&encode(&call.to_value())),
    })
}

fn ok_case(name: &str, value: &Value) -> Json {
    let value_json = match value {
        Value::Bytes(b) => json!({"$hex": hex(b)}),
        Value::BlobRef { .. } => json!({"$ref": ref_parts(value)}),
        Value::Int(i) => json!(i),
        Value::Array(rows) => Json::Array(
            rows.iter()
                .map(|r| match r {
                    Value::Map(pairs) => {
                        let get = |k: &str| {
                            pairs
                                .iter()
                                .find(|(name, _)| name == k)
                                .and_then(|(_, v)| v.as_text())
                                .unwrap_or_default()
                                .to_string()
                        };
                        json!({"verb": get("verb"), "object": get("object")})
                    }
                    _ => unreachable!("caps rows are maps"),
                })
                .collect(),
        ),
        other => unreachable!("ok payload {other:?}"),
    };
    json!({
        "name": name,
        "kind": "response",
        "ok": value_json,
        "canonical_hex": hex(&encode(&AmbientCall::ok_response(value.clone()))),
    })
}

fn err_case(name: &str, code: &str) -> Json {
    json!({
        "name": name,
        "kind": "response",
        "err": code,
        "canonical_hex": hex(&encode(&AmbientCall::err_response(code))),
    })
}

/// Corpus JSON → Value with the $hex/$ref desugars — ONE desugar,
/// used to pin canonical bytes for the refusal cases too (even
/// malformed SHAPES are canonical bytes; the harness re-encodes and
/// compares).
fn desugar(j: &Json) -> Value {
    match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Int(n.as_i64().expect("corpus ints")),
        Json::String(s) => Value::Text(s.clone()),
        Json::Array(items) => Value::Array(items.iter().map(desugar).collect()),
        Json::Object(map) => {
            if map.len() == 1 {
                if let Some(h) = map.get("$hex") {
                    let bytes = (0..h.as_str().expect("$hex").len() / 2)
                        .map(|i| {
                            u8::from_str_radix(&h.as_str().expect("$hex")[2 * i..2 * i + 2], 16)
                                .expect("hex byte")
                        })
                        .collect();
                    return Value::Bytes(bytes);
                }
                if let Some(r) = map.get("$ref") {
                    return Value::BlobRef {
                        digest: r["digest"].as_str().expect("digest").into(),
                        media_type: r["mediaType"].as_str().expect("mediaType").into(),
                        size: r["size"].as_u64().expect("size"),
                    };
                }
            }
            Value::map(
                map.iter()
                    .map(|(k, v)| (k.clone(), desugar(v)))
                    .collect(),
            )
            .expect("hygienic keys")
        }
    }
}

/// A refusal-shape request case — canonical bytes pinned from the
/// desugared fields, expect_error (and optionally the verb name).
fn refuse_case(name: &str, fields: Json, error: &str) -> Json {
    let bytes = encode(&desugar(&fields));
    json!({
        "name": name,
        "kind": "request",
        "fields": fields,
        "canonical_hex": hex(&bytes),
        "expect_error": error,
    })
}

fn main() {
    let r = sample_ref();
    let cases = vec![
        // ---- the four canonical request forms (roundtrip + bytes)
        req_case("request-blob-get", &AmbientCall {
            verb: AmbientVerb::BlobGet { ref_: r.clone() },
        }),
        req_case("request-blob-len", &AmbientCall {
            verb: AmbientVerb::BlobLen { ref_: r.clone() },
        }),
        req_case("request-blob-put-no-ttl", &AmbientCall {
            verb: AmbientVerb::BlobPut {
                bytes: BODY.to_vec(),
                media_type: MEDIA_TYPE.into(),
                ttl_micros: None,
            },
        }),
        req_case("request-blob-put-ttl", &AmbientCall {
            verb: AmbientVerb::BlobPut {
                bytes: BODY.to_vec(),
                media_type: MEDIA_TYPE.into(),
                ttl_micros: Some(3_600_000_000),
            },
        }),
        req_case("request-caps", &AmbientCall { verb: AmbientVerb::Caps }),
        // ---- the refusal table (malformed SHAPES are canonical
        //      bytes too — pinned, like everything else)
        {
            let mut c = refuse_case(
                "request-unknown-verb",
                json!({"verb": "blob.stats", "ref": {"$ref": ref_parts(&r)}}),
                "UnsupportedVerb",
            );
            c["expect_verb_name"] = json!("blob.stats");
            c
        },
        refuse_case(
            "request-get-ref-missing",
            json!({"verb": "blob.get"}),
            "BadVerbField",
        ),
        refuse_case(
            "request-get-ref-not-blobref",
            json!({"verb": "blob.get", "ref": "not-a-ref"}),
            "BadVerbField",
        ),
        refuse_case(
            "request-put-bytes-not-bytes",
            json!({"verb": "blob.put", "bytes": "text", "mediaType": MEDIA_TYPE, "ttl": null}),
            "BadVerbField",
        ),
        refuse_case(
            "request-put-mediatype-not-text",
            json!({"verb": "blob.put", "bytes": {"$hex": hex(BODY)}, "mediaType": 7, "ttl": null}),
            "BadVerbField",
        ),
        refuse_case(
            "request-put-ttl-not-int",
            json!({"verb": "blob.put", "bytes": {"$hex": hex(BODY)}, "mediaType": MEDIA_TYPE, "ttl": "soon"}),
            "BadVerbField",
        ),
        refuse_case(
            "request-put-ttl-negative",
            json!({"verb": "blob.put", "bytes": {"$hex": hex(BODY)}, "mediaType": MEDIA_TYPE, "ttl": -1}),
            "BadVerbField",
        ),
        refuse_case("request-not-a-map", json!([1, 2]), "BadForm"),
        refuse_case("request-verb-not-text", json!({"verb": 3}), "BadForm"),
        json!({
            "name": "request-garbage-bytes",
            "kind": "request",
            "canonical_hex": "ff00",
            "expect_error": "Decode",
        }),
        // ---- the response envelope forms (result shapes + the
        //      refusal codes' EXACT SPELLING)
        ok_case("response-ok-put-ref", &r),
        ok_case("response-ok-get-bytes", &Value::Bytes(BODY.to_vec())),
        ok_case("response-ok-len-int", &Value::Int(BODY.len() as i64)),
        ok_case(
            "response-ok-caps-rows",
            &Value::Array(vec![
                Value::map(vec![
                    ("verb".into(), Value::Text("blob.read".into())),
                    ("object".into(), Value::Text("*".into())),
                ])
                .unwrap(),
                Value::map(vec![
                    ("verb".into(), Value::Text("blob.write".into())),
                    ("object".into(), Value::Text("*".into())),
                ])
                .unwrap(),
            ]),
        ),
        err_case("response-err-unauthorized", "Unauthorized"),
        err_case("response-err-missing", "Missing"),
        err_case("response-err-integrity", "Integrity"),
        err_case("response-err-unavailable", "Unavailable"),
    ];

    let corpus = json!({"profile": "ambient-v0", "cases": cases});
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/ambient-v0.json");
    std::fs::write(&path, serde_json::to_string_pretty(&corpus).unwrap() + "\n")
        .expect("corpus writes");
    println!("wrote {} ({} cases)", path.display(), cases.len());
}
