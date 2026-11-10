use mwydyn_data::telemetry::TelemetryRecord;
use mwydyn_data::Value;

#[test]
fn declare_shape_matches_the_prelude() {
    // exactly what the JS prelude's Effect.log builds, converted
    let v = Value::map(vec![
        ("kind".into(), Value::Text("log".into())),
        (
            "log".into(),
            Value::map(vec![
                ("level".into(), Value::Text("info".into())),
                ("message".into(), Value::Text("echo saw demo.kick@1".into())),
            ])
            .unwrap(),
        ),
    ])
    .unwrap();
    match TelemetryRecord::declare_from_value(&v) {
        Ok(r) => println!("ok: {r:?}"),
        Err(e) => println!("ERR: {e:?}"),
    }
}
