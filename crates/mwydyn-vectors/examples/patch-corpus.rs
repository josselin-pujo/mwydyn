//! One-off: patch hand-typed corpus expectations with machine truth.
use mwydyn_data::{encode, Value};

fn hexs(v: &Value) -> String {
    encode(v).iter().map(|b| format!("{:02x}", b)).collect()
}

fn deep(n: usize) -> String {
    let mut s = String::new();
    for _ in 0..n {
        s.push_str("81");
    }
    s.push_str("00");
    s
}

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/cbor-profile-v0.json");
    let mut root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let cases = root["cases"].as_array_mut().unwrap();
    for c in cases.iter_mut() {
        let name = c["name"].as_str().unwrap().to_string();
        match name.as_str() {
            "enc-null" => {
                c["in"] = serde_json::json!({ "model": { "type": "null" } });
            }
            "enc-float-3.5" => {
                c["expect"]["hex"] =
                    serde_json::json!(hexs(&Value::float(3.5).unwrap()));
            }
            "enc-float-3.0-floatness-preserved" => {
                c["expect"]["hex"] =
                    serde_json::json!(hexs(&Value::float(3.0).unwrap()));
            }
            "enc-float-0.5" => {
                c["expect"]["hex"] =
                    serde_json::json!(hexs(&Value::float(0.5).unwrap()));
            }
            "enc-map-sorts-keys" => {
                let m = Value::map(vec![
                    ("b".into(), Value::Int(2)),
                    ("a".into(), Value::Int(1)),
                ])
                .unwrap();
                c["expect"]["hex"] = serde_json::json!(hexs(&m));
            }
            "enc-map-canonical-is-not-alphabetical" => {
                let m = Value::map(vec![
                    ("aa".into(), Value::Int(3)),
                    ("b".into(), Value::Int(1)),
                    ("a".into(), Value::Int(2)),
                ])
                .unwrap();
                c["expect"]["hex"] = serde_json::json!(hexs(&m));
            }
            "roundtrip-composite" => {
                let comp = Value::map(vec![
                    (
                        "a".into(),
                        Value::Array(vec![
                            Value::Int(1),
                            Value::float(2.5).unwrap(),
                            Value::Bool(true),
                            Value::Null,
                            Value::map(vec![("b".into(), Value::Text("c".into()))])
                                .unwrap(),
                        ]),
                    ),
                    ("d".into(), Value::Int(-7)),
                ])
                .unwrap();
                c["expect"]["hex"] = serde_json::json!(hexs(&comp));
            }
            "dec-null" => {
                c["in"] = serde_json::json!({ "hex": "f6" });
                c["expect"] = serde_json::json!({ "model": { "type": "null" } });
            }
            "dec-map-decodes-and-canonicalizes" => {
                // non-canonical input order (b first), by construction
                c["in"]["hex"] = serde_json::json!(format!("a26162{}6161{}", "01", "02"));
                let m = Value::map(vec![
                    ("b".into(), Value::Int(1)),
                    ("a".into(), Value::Int(2)),
                ])
                .unwrap();
                c["expect"] = serde_json::json!({ "hex": hexs(&m) });
            }
            "dec-64-deep-ok" => {
                c["in"]["hex"] = serde_json::json!(deep(64));
                let mut j = serde_json::json!(0);
                for _ in 0..64 {
                    j = serde_json::json!([j]);
                }
                c["expect"] = serde_json::json!({ "json": j });
            }
            "rej-too-deep" => {
                c["in"]["hex"] = serde_json::json!(deep(65));
            }
            _ => {}
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap()).unwrap();
    println!("patched {}", path.display());

    write_blob_corpus();
}

/// Generates vectors/blob-refs-v0.json wholesale — machine-built
/// wants per work-note 11's rule (hand-write rejections only).
fn write_blob_corpus() {
    use mwydyn_data::Value;

    let hexs = |v: &Value| encode(v).iter().map(|b| format!("{:02x}", b)).collect::<String>();

    let png_digest = Value::blob_digest(b"\x01\x02", "image/png").unwrap();
    let txt_digest = Value::blob_digest(b"\x01\x02", "text/plain").unwrap();
    let rf = Value::blob_ref(b"\x01\x02", "image/png").unwrap();
    let ref_hex = hexs(&rf);
    let forged = Value::BlobRef {
        digest: "A".repeat(64),
        media_type: "image/png".into(),
        size: 2,
    };

    let c = |name: &str, op: &str, mut inp: serde_json::Value, expect: serde_json::Value| {
        inp["__placeholder"] = serde_json::Value::Null;
        inp.as_object_mut().unwrap().remove("__placeholder");
        serde_json::json!({ "name": name, "op": op, "in": inp, "expect": expect })
    };

    let cases = vec![
        c("digest-png", "digest",
            serde_json::json!({ "bytes": "0102", "mediaType": "image/png" }),
            serde_json::json!({ "hex": png_digest })),
        c("digest-same-body-other-type-is-not-the-same-content", "digest",
            serde_json::json!({ "bytes": "0102", "mediaType": "text/plain" }),
            serde_json::json!({ "hex": txt_digest })),
        c("digest-determinism-pin", "digest",
            serde_json::json!({ "bytes": "0102", "mediaType": "image/png" }),
            serde_json::json!({ "hex": png_digest })),
        c("digest-reject-empty-type", "digest",
            serde_json::json!({ "bytes": "0102", "mediaType": "" }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("enc-ref-roundtrip", "roundtrip",
            serde_json::json!({ "model": { "type": "ref", "digest": png_digest, "mediaType": "image/png", "size": 2 } }),
            serde_json::json!({ "hex": ref_hex })),
        c("enc-ref-reject-nonhex-digest", "encode",
            serde_json::json!({ "model": { "type": "ref", "digest": "z".repeat(64), "mediaType": "image/png", "size": 2 } }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("enc-ref-reject-uppercase-digest", "encode",
            serde_json::json!({ "model": { "type": "ref", "digest": "A".repeat(64), "mediaType": "image/png", "size": 2 } }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("enc-ref-reject-empty-type", "encode",
            serde_json::json!({ "model": { "type": "ref", "digest": png_digest, "mediaType": "", "size": 2 } }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("dec-ref-canonical", "decode",
            serde_json::json!({ "hex": ref_hex }),
            serde_json::json!({ "hex": ref_hex })),
        c("rej-dec-tag-of-text", "decode",
            serde_json::json!({ "hex": "d903ea6161" }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("rej-dec-tag-of-empty-map", "decode",
            serde_json::json!({ "hex": "d903eaa0" }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("rej-dec-tag-of-2key-map", "decode",
            serde_json::json!({ "hex": format!("d903ea{}", hexs(&Value::map(vec![
                ("a".into(), Value::Int(1)), ("b".into(), Value::Int(2))]).unwrap())) }),
            serde_json::json!({ "reject": "bad-ref" })),
        c("rej-dec-forged-uppercase-digest", "decode",
            serde_json::json!({ "hex": hexs(&forged) }),
            serde_json::json!({ "reject": "bad-ref" })),
    ];

    let corpus = serde_json::json!({
        "profile": "mwydyn-cbor-v0.1-blob-refs",
        "note": "Blob references: tag 1002, hash-bound mediaType (round 12). digest = SHA-256(canonical CBOR of {bytes, type}); content identity includes type — same body under another type is another blob. Generated; edit via the patcher, not by hand.",
        "cases": cases,
    });
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/blob-refs-v0.json");
    std::fs::write(&out, serde_json::to_string_pretty(&corpus).unwrap()).unwrap();
    println!("wrote {}", out.display());
}
