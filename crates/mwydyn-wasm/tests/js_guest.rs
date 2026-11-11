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

/// ROUND 47 — THE JS GUEST'S AMBIENT SURFACE (the regression pin for the
/// double-free the trampoline carried since round 43: it freed its
/// JSValueConst argv, and quickjs freed it again at frame teardown —
/// invisible until the first script that actually called cx.blob*;
/// the echo never did, and the Rust store guest couldn't catch it).
/// A composed JS guest puts a blob, gets it back, and replies with both
/// the ref and the round-tripped bytes. Cap rows granted at spawn — the
/// ambient surface is grant-checked at every call.
#[test]
fn js_guest_ambient_blob_round_trip() {
    if !Path::new("tests/artifacts/mwydyn_guest_js.wasm").exists() {
        eprintln!("SKIP: engine artifact missing — run scripts/build-guest-js.sh");
        return;
    }
    // Compose the ambient script into the engine through the same
    // library the build tool uses — no second committed artifact to rot.
    let engine_bytes = std::fs::read("tests/artifacts/mwydyn_guest_js.wasm").expect("engine");
    let script = r#"
mwydyn.entry({
  receive(letter, cx) {
    const body = new Uint8Array([1, 2, 3, 0xff]);
    const ref = cx.blobPut(body, "application/x-probe");
    const back = cx.blobGet(ref);
    const len = cx.blobLen(ref);
    return [mwydyn.Effect.reply(letter, cx, "probe.ambient@1", {
      ref: ref,
      bytes: back,
      len: len,
      caps: cx.caps().length,
    })];
  },
});
"#;
    let composed = mwydyn_js::compose(&engine_bytes, script.as_bytes()).expect("compose");
    let mut ctx = Context::new("ja");
    ctx.spawn(
        Spawn::new(
            Some("amb"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(
                    WasmActor::from_compiled(&engine, &composed.wasm).expect("guest"),
                )
            }),
        )
        .rows(vec![
            mwydyn_core::CapRow::new("blob.read", "*"),
            mwydyn_core::CapRow::new("blob.write", "*"),
        ])
        .supervision(Supervision::Restart),
    )
    .unwrap();
    ctx.post(
        &addr("ja", "amb"),
        Letter::new("probe.kick@1", encode(&Value::Null), None, None, vec![]).unwrap(),
        vec![],
        None,
    );
    ctx.run_until_idle(1000);

    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "one reply (a double-free here is a strike, not a reply)");
    let v = decode(&drained[0].letter.payload, &DecodeOptions::default()).expect("canonical");
    let obj = match &v { Value::Map(pairs) => pairs.clone(), other => panic!("map expected, got {other:?}") };
    let get = |k: &str| obj.iter().find(|(key, _)| key == k).map(|(_, v)| v).expect("field");
    // the ref round-tripped as a real BlobRef (branded object → Value)
    let (digest, size) = match get("ref") {
        Value::BlobRef { digest, size, .. } => (digest.clone(), *size),
        other => panic!("BlobRef expected, got {other:?}"),
    };
    // the bytes round-tripped Uint8Array → Bytes, content identical
    assert_eq!(
        get("bytes"),
        &Value::Bytes(vec![1, 2, 3, 0xff]),
        "blob bytes survive the ambient round trip"
    );
    assert_eq!(get("len"), &Value::Int(4), "blobLen agrees with the bytes");
    assert_eq!(size, 4, "the ref's hash-bound size agrees");
    assert!(!digest.is_empty());
    assert_eq!(get("caps"), &Value::Int(2), "the two granted rows are visible");
}
