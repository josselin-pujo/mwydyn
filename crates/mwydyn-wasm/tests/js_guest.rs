//! ROUND 43 — THE JS GUEST THROUGH THE REAL CONTAINER. A script
//! composed by `mwydyn-js build` (quickjs-ng engine + echo script,
//! one artifact) spawned as a plain §9 guest: letters in, effects
//! out, the §3.6 telemetry log visible in the drain. The host never
//! learns that JS exists — that is the round's law, and this is its
//! proof. Skip-with-message if the artifact is absent (build via
//! scripts/build-guest-js.sh).

use mwydyn_core::{Context, Spawn, Supervision};
use mwydyn_data::telemetry::TelPayload;
use mwydyn_data::{address_to_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_wasm::{WasmActor, WasmEngine};
use std::path::Path;

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

fn kick(payload: Value) -> Letter {
    let payload = encode(&Value::map({
        let mut v = vec![(
            "replyTo".to_string(),
            address_to_value(&Address::Instance("external".into())),
        )];
        if let Value::Map(pairs) = payload {
            v.extend(pairs);
        }
        v
    })
    .unwrap());
    Letter::new("demo.kick@1", payload, None, None, vec![]).unwrap()
}

#[test]
fn js_echo_guest_through_the_container() {
    if !Path::new("tests/artifacts/mwydyn_guest_js_echo.wasm").exists() {
        eprintln!("SKIP: JS guest artifact missing — run scripts/build-guest-js.sh");
        return;
    }
    let wasm = std::fs::read("tests/artifacts/mwydyn_guest_js_echo.wasm").expect("artifact");
    let mut ctx = Context::new("je");
    ctx.spawn(
        Spawn::new(
            Some("echo"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
            }),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();

    // JS statelessness declared by the SCRIPT (entry!) — probed by
    // the container from the artifact itself
    ctx.post(
        &addr("je", "echo"),
        kick(Value::map(vec![("n".into(), Value::Int(7))]).unwrap()),
        vec![],
        None,
    );
    ctx.run_until_idle(1000);

    let records = ctx.drain_telemetry();
    for r in &records { eprintln!("TEL: {} {:?}", r.kind.as_str(), r.payload); }
    // the echo came back out of the embedding's outbox
    let dead = ctx.dead_letters().to_vec();
    for d in dead { eprintln!("DEAD: {d:?}"); }
    let drained = ctx.drain_external();
    eprintln!("drained: {}", drained.len());
    assert_eq!(drained.len(), 1, "one echo");
    let v = decode(&drained[0].letter.payload, &DecodeOptions::default()).expect("canonical");
    assert_eq!(
        v,
        Value::map(vec![
            ("n".into(), Value::Int(7)),
            ("replyTo".into(), Value::Text("external".into())),
        ])
        .unwrap(),
        "payload echoed through the JS value conversion round trip (whole map, as an echo should)"
    );

    // and the §3.6 log record the script emitted is in the drain
    let saw_echo_log = records.iter().any(|r| match &r.payload {
        TelPayload::Log { message, .. } => message.contains("echo saw"),
        _ => false,
    });
    assert!(saw_echo_log, "the JS actor's Effect.log reached the outbox");
}
