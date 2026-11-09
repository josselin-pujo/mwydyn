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

/// §7.6 THE FULL DURABLE ROUND TRIP for a STATEFUL WASM guest
/// (round 39): work → snapshot_actor (runtime save, covers-stamped)
/// → more work → die → reopen on the durable tier → spawn under the
/// same name (boot scans the snapshot) → hydrate (replay skips the
/// covered prefix) → the pending letter lands → EXACTLY-ONCE.
#[test]
fn stateful_guest_snapshot_survives_reopen_and_is_exactly_once() {
    if !Path::new("tests/artifacts/mwydyn_guest_counter.wasm").exists() {
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
                    Box::new(WasmActor::from_compiled_stateful(&engine, &wasm).expect("guest"))
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
            ctx.post(&addr("sf", "ctr"), kick("external"), vec![]);
            ctx.run_until_idle(100);
        }
        assert_eq!(drain_counts(&mut ctx), vec![1, 2, 3]);
        ctx.snapshot_actor("ctr").expect("runtime save");
        // one more letter, PENDING across the death (never delivered)
        ctx.post(&addr("sf", "ctr"), kick("external"), vec![]);
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
