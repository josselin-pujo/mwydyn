//! ROUND 40 — THE AMBIENT SURFACE PROOFS. The store guest puts a
//! body (blob.write), gets it back + lens it (blob.read), and posts
//! a receipt with the round-trip facts. Positive: capped spawn sees
//! everything green. Negative: the SAME guest spawned UNCAPPED sees
//! `Unauthorized` — the surface is granted, never ambient.
//!
//! The capability model held at the barrier unprompted in round 39
//! (restore = a blob read); this round makes it the guest's own
//! visible experience.

use mwydyn_core::{Context, Spawn, Supervision};
use mwydyn_data::{address_to_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_wasm::{WasmActor, WasmEngine};
use std::path::Path;

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

fn store_kick(body: &[u8]) -> Letter {
    let payload = encode(&Value::map(vec![
        ("body".into(), Value::Bytes(body.to_vec())),
        (
            "replyTo".into(),
            address_to_value(&Address::Instance("external".into())),
        ),
    ])
    .unwrap());
    Letter::new("test.store@1", payload, None, None, vec![]).unwrap()
}

fn receipt_of(ctx: &mut Context) -> std::collections::HashMap<String, Value> {
    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "one receipt");
    let v = decode(&drained[0].letter.payload, &DecodeOptions::default()).expect("canonical");
    let Value::Map(pairs) = v else {
        panic!("receipt is a map");
    };
    pairs.into_iter().collect()
}

fn run_store(spawn_uncapped: bool) -> std::collections::HashMap<String, Value> {
    let wasm = std::fs::read("tests/artifacts/mwydyn_guest_store.wasm").expect("artifact");
    let mut ctx = Context::new("am");
    ctx.spawn(
        Spawn::new(
            Some("store"),
            Box::new(move || {
                let wasm = wasm.clone();
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
            }),
        )
        .rows(if spawn_uncapped {
            vec![]
        } else {
            vec![
                mwydyn_core::CapRow::new("blob.read", "*"),
                mwydyn_core::CapRow::new("blob.write", "*"),
            ]
        })
        .supervision(Supervision::Restart),
    )
    .unwrap();
    ctx.post(&addr("am", "store"), store_kick(b"ambient-body-42"), vec![], None);
    ctx.run_until_idle(300);
    receipt_of(&mut ctx)
}

#[test]
fn capped_guest_round_trips_blobs_through_the_ambient_surface() {
    if !Path::new("tests/artifacts/mwydyn_guest_store.wasm").exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let r = run_store(false);
    let int = |k: &str| match r.get(k) {
        Some(Value::Int(n)) => *n,
        other => panic!("{k}: {other:?}"),
    };
    assert_eq!(int("putLen"), 15, "blob.put returned the ref's size");
    assert_eq!(int("gotLen"), 15, "blob.get returned the body");
    assert_eq!(int("match"), 1, "the read bytes match the stored bytes");
    assert_eq!(int("lenOk"), 1, "blob.len agrees");
    assert!(!r.contains_key("putErr"));
    assert!(!r.contains_key("getErr"));
    // caps: the free verb lists exactly the two declared rows
    assert_eq!(int("capRows"), 2, "caps lists the declared rows");
}

#[test]
fn uncapped_guest_sees_unauthorized_not_silence() {
    if !Path::new("tests/artifacts/mwydyn_guest_store.wasm").exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let r = run_store(true);
    let err = |k: &str| match r.get(k) {
        Some(Value::Text(code)) => code.clone(),
        other => panic!("{k}: {other:?}"),
    };
    assert_eq!(err("putErr"), "Unauthorized", "blob.put refused BY NAME");
    // The refused put leaves no ref: get/len are skipped (getErr
    // absent) — the refusal is OBSERVED by the guest, not swallowed.
    assert!(!r.contains_key("getErr"));
    let int = |k: &str| match r.get(k) {
        Some(Value::Int(n)) => *n,
        other => panic!("{k}: {other:?}"),
    };
    assert_eq!(int("putLen"), -1, "no fake successes");
    assert_eq!(int("capRows"), 0, "no rows declared, none listed");
}
