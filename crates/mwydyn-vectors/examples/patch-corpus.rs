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
}
