//! ROUND 37 — THE DECLARED-STATEFULNESS PROOFS. One guest (a static
//! counter in linear memory), two lifecycles:
//! - STATEFUL: refused transitions roll back byte-exact (memory
//!   image) — three strikes against the SAME letter leave the count
//!   untouched; the next honest letter sees count=1.
//! - STATELESS: the same flow leaves the dirty state in place —
//!   count=4. The declaration changed what the runtime owed; the
//!   author owns the difference.

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

fn kick(reply_to: &str) -> Letter {
    // "external" is the INSTANCE address of the embedding's outbox —
    // everything else is a named actor in this context.
    let dest = if reply_to == "external" {
        Address::Instance("external".into())
    } else {
        addr("sf", reply_to)
    };
    let payload = encode(&Value::map(vec![(
        "replyTo".into(),
        address_to_value(&dest),
    )])
    .unwrap());
    Letter::new("test.count@1", payload, None, None, vec![]).unwrap()
}

/// Run the counter guest in `mode`: one poison letter (replyTo =
/// "void" — refused, MAX_ATTEMPTS strikes → dead-letter), then one
/// honest letter (replyTo = "external"). Returns the receipt count.
fn counter_receipt_after_poison(mode_ctor: fn(&WasmEngine, &[u8]) -> Result<WasmActor, anyhow::Error>) -> i64 {
    let wasm = std::fs::read("tests/artifacts/mwydyn_guest_counter.wasm").expect("artifact");
    let mut ctx = Context::new("sf");
    ctx.spawn(
        Spawn::new(
            Some("ctr"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(mode_ctor(&engine, &wasm).expect("guest"))
            }),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();

    // THE POISON: its effects target a nonexistent actor — Phase 2
    // strikes; the letter dead-letters after MAX_ATTEMPTS.
    ctx.post(&addr("sf", "ctr"), kick("void"), vec![]);
    ctx.run_until_idle(500);
    assert_eq!(ctx.dead_letters().len(), 1, "the poison letter dead-letters");
    assert!(ctx.drain_external().is_empty(), "no receipts for refused effects");

    // THE HONEST LETTER.
    ctx.post(&addr("sf", "ctr"), kick("external"), vec![]);
    ctx.run_until_idle(500);
    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "the honest letter's receipt");
    let receipt = decode(&drained[0].letter.payload, &DecodeOptions::default()).expect("canonical");
    let Value::Map(pairs) = receipt else {
        panic!("receipt is a map");
    };
    match pairs.iter().find(|(k, _)| k == "count").map(|(_, v)| v) {
        Some(Value::Int(n)) => *n,
        other => panic!("receipt count: {other:?}"),
    }
}

#[test]
fn declared_stateful_rollback_is_exact() {
    if !Path::new("tests/artifacts/mwydyn_guest_counter.wasm").exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let count = counter_receipt_after_poison(WasmActor::from_compiled_stateful);
    assert_eq!(
        count, 1,
        "three refused attempts left NO residue — the memory image rolled the guest back"
    );
}

#[test]
fn declared_stateless_dirty_state_is_the_authors_problem() {
    if !Path::new("tests/artifacts/mwydyn_guest_counter.wasm").exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let count = counter_receipt_after_poison(WasmActor::from_compiled);
    assert_eq!(
        count, 4,
        "stateless: refused attempts DIRTY the guest (1 per strike attempt + the honest one) — no rollback owed"
    );
}
