//! ROUNDS 37+41 — THE DECLARATION PROOFS. The GUEST owns its
//! statefulness (§9.2): `mwydyn_decl` read by the container at
//! composition; no embedder override exists. Four proofs:
//!
//! - THE PROBE: WAT with/without `mwydyn_decl` — the lifecycle is
//!   read from the guest, whatever text form it arrives in.
//! - ROLLBACK: the durable counter declares stateful — three
//!   strikes leave NO residue; the next honest letter sees count=1.
//! - THE DUAL: one dedup source, two declarations — stateful rolls
//!   the seen-set back (a correlation whose attempts all struck is
//!   still fresh); stateless keeps the strike-dirtied marks.
//! - THE DURABLE ROUND TRIP (round 39): snapshot → death → reopen →
//!   boot restore → hydrate → exactly-once — now on the SDK's
//!   `durable_entry!`.

use mwydyn_core::{Actor as _, Context, Spawn, Supervision};
use mwydyn_data::{address_to_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_wasm::{WasmActor, WasmEngine};
use std::path::Path;

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

fn kick(context: &str, reply_to: &str, correlation: Option<String>) -> Letter {
    // "external" is the INSTANCE address of the embedding's outbox —
    // everything else is a named actor in this context.
    let dest = if reply_to == "external" {
        Address::Instance("external".into())
    } else {
        addr(context, reply_to)
    };
    let payload = encode(&Value::map(vec![(
        "replyTo".into(),
        address_to_value(&dest),
    )])
    .unwrap());
    Letter::new("test.kick@1", payload, None, correlation, vec![]).unwrap()
}

fn artifact(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/artifacts/{name}.wasm")).expect("artifact")
}

fn have(name: &str) -> bool {
    Path::new(&format!("tests/artifacts/{name}.wasm")).exists()
}

/// THE PROBE — the declaration is read from the guest text. These
/// WAT guests never process a letter; only the lifecycle flag is
/// under test.
#[test]
fn declaration_is_read_from_the_guest() {
    let engine = WasmEngine::new().expect("engine");
    // an UNCLOSED module body — each variant appends its exports
    let base = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32))
          (func (export "process") (param i32 i32) (result i32) (i32.const 0))"#;
    // absent export: Stateless BY OMISSION (hand-rolled guests)
    let bare = WasmActor::from_wat(&engine, &format!("{base})")).expect("bare fixture");
    assert!(
        !bare.fork_carries_state(),
        "no mwydyn_decl export → Stateless by omission"
    );
    // bit 0 set: Stateful — the same text, one export apart
    let declaring = WasmActor::from_wat(
        &engine,
        &format!("{base}(func (export \"mwydyn_decl\") (result i32) (i32.const 1)))"),
    )
    .expect("declaring fixture");
    assert!(
        declaring.fork_carries_state(),
        "mwydyn_decl bit 0 → Stateful"
    );
    // explicit zero is still Stateless — the SDK's entry! emits this
    let zero = WasmActor::from_wat(
        &engine,
        &format!("{base}(func (export \"mwydyn_decl\") (result i32) (i32.const 0)))"),
    )
    .expect("zero fixture");
    assert!(!zero.fork_carries_state(), "explicit zero → Stateless");
}

/// ROLLBACK — the counter artifact (durable_entry!: stateful bit +
/// §7.6 exports) under its own declaration. One poison letter
/// (replyTo = "void" — refused, MAX_ATTEMPTS strikes → dead-letter),
/// then one honest letter; returns the receipt count.
fn counter_receipt_after_poison() -> i64 {
    let wasm = artifact("mwydyn_guest_counter");
    let mut ctx = Context::new("sf");
    ctx.spawn(
        Spawn::new(
            Some("ctr"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
            }),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();

    // THE POISON: its effects target a nonexistent actor — Phase 2
    // strikes; the letter dead-letters after MAX_ATTEMPTS.
    ctx.post(&addr("sf", "ctr"), kick("sf", "void", None), vec![]);
    ctx.run_until_idle(500);
    assert_eq!(ctx.dead_letters().len(), 1, "the poison letter dead-letters");
    assert!(ctx.drain_external().is_empty(), "no receipts for refused effects");

    // THE HONEST LETTER.
    ctx.post(&addr("sf", "ctr"), kick("sf", "external", None), vec![]);
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
    if !have("mwydyn_guest_counter") {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let count = counter_receipt_after_poison();
    assert_eq!(
        count, 1,
        "three refused attempts left NO residue — the memory image rolled the guest back"
    );
}

/// THE DUAL — one source, two declarations (guest-dedup, built
/// stateful by default and stateless via --features
/// stateless-build). A fresh letter ("a") lands; a poison letter
/// ("b", replyTo = void) strikes to death; the SAME correlation is
/// re-posted honestly — was it marked?
fn dedup_first_after_poison(artifact_name: &str) -> bool {
    let wasm = artifact(artifact_name);
    let mut ctx = Context::new("dd");
    ctx.spawn(
        Spawn::new(
            Some("dd"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
            }),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();

    // sanity: a fresh correlation is a first
    ctx.post(&addr("dd", "dd"), kick("dd", "external", Some("a".into())), vec![]);
    ctx.run_until_idle(100);
    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "the fresh letter's receipt");
    assert!(receipt_flag(&drained[0].letter, "first"));

    // THE POISON: correlation "b", effects to void → dead-letter
    ctx.post(&addr("dd", "dd"), kick("dd", "void", Some("b".into())), vec![]);
    ctx.run_until_idle(300);
    assert_eq!(ctx.dead_letters().len(), 1, "the poison letter dead-letters");
    assert!(ctx.drain_external().is_empty(), "no receipts for refused effects");

    // THE QUESTION: is "b" still fresh?
    ctx.post(&addr("dd", "dd"), kick("dd", "external", Some("b".into())), vec![]);
    ctx.run_until_idle(100);
    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "the honest letter's receipt");
    receipt_flag(&drained[0].letter, "first")
}

fn receipt_flag(letter: &Letter, key: &str) -> bool {
    let v = decode(&letter.payload, &DecodeOptions::default()).expect("canonical");
    let Value::Map(pairs) = v else {
        panic!("receipt is a map");
    };
    match pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v) {
        Some(Value::Bool(b)) => *b,
        other => panic!("receipt {key}: {other:?}"),
    }
}

#[test]
fn the_declaration_is_the_semantics() {
    if !have("mwydyn_guest_dedup") || !have("mwydyn_guest_dedup_stateless") {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    assert!(
        dedup_first_after_poison("mwydyn_guest_dedup"),
        "stateful: the strike attempts rolled back — 'b' is still fresh"
    );
    assert!(
        !dedup_first_after_poison("mwydyn_guest_dedup_stateless"),
        "stateless: the strike-dirtied mark stays — the author's problem, by declaration"
    );
}

/// §7.6 THE FULL DURABLE ROUND TRIP for a STATEFUL WASM guest
/// (round 39): work → snapshot_actor (runtime save, covers-stamped)
/// → more work → die → reopen on the durable tier → spawn under the
/// same name (boot scans the snapshot) → hydrate (replay skips the
/// covered prefix) → the pending letter lands → EXACTLY-ONCE.
#[test]
fn stateful_guest_snapshot_survives_reopen_and_is_exactly_once() {
    if !have("mwydyn_guest_counter") {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let dir = std::env::temp_dir();
    let journal_path = dir.join(format!("mwydyn-sfw-{}-{:?}.sqlite", std::process::id(), line!()));
    let blob_path = dir.join(format!("mwydyn-sfb-{}-{:?}.sqlite", std::process::id(), line!()));
    let _ = std::fs::remove_file(&journal_path);
    let _ = std::fs::remove_file(&blob_path);

    let spawn_ctr = |ctx: &mut Context| {
        let wasm_path = Path::new("tests/artifacts/mwydyn_guest_counter.wasm")
            .to_path_buf();
        ctx.spawn(
            Spawn::new(
                Some("ctr"),
                Box::new(move || {
                    let wasm = std::fs::read(&wasm_path).expect("artifact");
                    let engine = WasmEngine::new().expect("engine");
                    Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
                }),
            )
            // The capability model holds at the barrier: restoring
            // your own durable state is a blob READ, and reads are
            // granted, not ambient.
            .rows(vec![mwydyn_core::CapRow::new("blob.read", "*")])
            .supervision(Supervision::Restart),
        )
        .unwrap();
    };

    // ---- ctx1 (durable tier): 3 letters → snapshot → 1 letter → die.
    {
        let mut ctx = Context::with_stores(
            "sf",
            Box::new(
                mwydyn_core::SQLiteJournal::open(&journal_path, Box::new(mwydyn_core::ManualClock(0)))
                    .unwrap(),
            ),
            Box::new(mwydyn_core::SQLiteBlobStore::open(&blob_path).unwrap()),
        );
        spawn_ctr(&mut ctx);
        for _ in 0..3 {
            ctx.post(&addr("sf", "ctr"), kick("sf", "external", None), vec![]);
            ctx.run_until_idle(100);
        }
        assert_eq!(drain_counts(&mut ctx), vec![1, 2, 3]);
        ctx.snapshot_actor("ctr").expect("runtime save");
        // one more letter, PENDING across the death (never delivered)
        ctx.post(&addr("sf", "ctr"), kick("sf", "external", None), vec![]);
        drop(ctx); // ---- the death line ----
    }

    // ---- ctx2: reopen, spawn under the same name (boot scans the
    // snapshot letter — covers stamped at save time), hydrate
    // (replay skips the covered prefix), the pending letter lands.
    {
        let mut ctx = Context::with_stores(
            "sf",
            Box::new(
                mwydyn_core::SQLiteJournal::open(&journal_path, Box::new(mwydyn_core::ManualClock(0)))
                    .unwrap(),
            ),
            Box::new(mwydyn_core::SQLiteBlobStore::open(&blob_path).unwrap()),
        );
        spawn_ctr(&mut ctx);
        let replays = ctx.hydrate().expect("hydrate");
        ctx.run_until_idle(200);
        let counts = drain_counts(&mut ctx);
        assert_eq!(
            counts,
            vec![4],
            "exactly-once: restored 3 + the ONE pending letter (replays covered the prefix: {replays})"
        );
        let _ = std::fs::remove_file(&journal_path);
        let _ = std::fs::remove_file(&blob_path);
        let _ = std::fs::remove_file(journal_path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(journal_path.with_extension("sqlite-shm"));
    }
}

fn drain_counts(ctx: &mut Context) -> Vec<i64> {
    let mut counts = Vec::new();
    for d in ctx.drain_external() {
        let v = decode(&d.letter.payload, &DecodeOptions::default()).expect("canonical");
        let Value::Map(pairs) = v else {
            panic!("receipt is a map");
        };
        match pairs.iter().find(|(k, _)| k == "count").map(|(_, v)| v) {
            Some(Value::Int(n)) => counts.push(*n),
            other => panic!("receipt count: {other:?}"),
        }
    }
    counts
}
