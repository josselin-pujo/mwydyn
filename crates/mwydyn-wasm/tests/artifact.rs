//! ROUND 48 — THE MODULE/ACTOR SPLIT. `GuestArtifact`: compile once,
//! probe once, clone per spawn. Three pins:
//!
//! - THE LAW: the declaration rides INSIDE the artifact — every
//!   spawn (and every clone) obeys the same probed `GuestMode`, and
//!   no constructor anywhere takes a mode (§9.2's "no embedder
//!   override" stays true with the split made public).
//! - THE SHARDING SHAPE: one artifact, two named actors, one
//!   codebase — independent instances, independent counts.
//! - THE COST BOUNDARY: twenty spawns from one artifact cost less
//!   than ONE compile — the serve/shard path never recompiles and
//!   never re-probes (the round's debt was exactly that).

use mwydyn_core::{Actor as _, Context, Spawn, Supervision};
use mwydyn_data::{address_to_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_wasm::{GuestArtifact, WasmActor, WasmEngine};
use std::path::Path;
use wasmtime::Module as WtModule;

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

fn kick(context: &str, reply_to: &str) -> Letter {
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
    Letter::new("test.kick@1", payload, None, None, vec![]).unwrap()
}

fn artifact_bytes(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/artifacts/{name}.wasm")).expect("artifact")
}

fn have(name: &str) -> bool {
    Path::new(&format!("tests/artifacts/{name}.wasm")).exists()
}

/// THE LAW — the mode is the artifact's stored probe result, however
/// the artifact was built and however often it is cloned. WAT
/// fixtures mirror the round-41 probe table (absent / bit 0 /
/// explicit zero); `from_module` exercises the embedder-compiled
/// path, which is the same probe under a different compiler.
#[test]
fn the_declaration_rides_inside_the_artifact() {
    let engine = WasmEngine::new().expect("engine");
    // an UNCLOSED module body — each variant appends its exports
    let base = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "dealloc") (param i32 i32))
          (func (export "process") (param i32 i32) (result i32) (i32.const 0))"#;
    let wat_artifact = |tail: &str| {
        let module = WtModule::new(engine.engine(), format!("{base}{tail}")).expect("WAT");
        GuestArtifact::from_module(&engine, module).expect("artifact from WAT")
    };

    let absent = wat_artifact(")");
    assert_eq!(absent.mode(), mwydyn_wasm::GuestMode::Stateless);
    assert!(
        !WasmActor::from_artifact(&absent)
            .expect("spawn")
            .fork_carries_state(),
        "no mwydyn_decl export → Stateless by omission, carried by the artifact"
    );

    let declaring = wat_artifact(
        r#"(func (export "mwydyn_decl") (result i32) (i32.const 1)))"#,
    );
    assert_eq!(declaring.mode(), mwydyn_wasm::GuestMode::Stateful);
    // THE CLONE: same declaration, no re-probe — the result rides.
    let cloned = declaring.clone();
    assert_eq!(cloned.mode(), mwydyn_wasm::GuestMode::Stateful);
    assert!(
        WasmActor::from_artifact(&cloned)
            .expect("spawn from clone")
            .fork_carries_state(),
        "bit 0 → Stateful, carried by the artifact and its clones"
    );

    let zero = wat_artifact(r#"(func (export "mwydyn_decl") (result i32) (i32.const 0)))"#);
    assert_eq!(zero.mode(), mwydyn_wasm::GuestMode::Stateless);
}

/// THE SHARDING SHAPE — one artifact, two named actors (the N-shards-
/// one-codebase pattern that made the split worth public). Each shard
/// gets its own instance: independent counts from one compilation.
#[test]
fn one_artifact_many_shards() {
    if !have("mwydyn_guest_counter") {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let engine = WasmEngine::new().expect("engine");
    let art = GuestArtifact::compile(&engine, &artifact_bytes("mwydyn_guest_counter"))
        .expect("compile once");
    assert_eq!(
        art.mode(),
        mwydyn_wasm::GuestMode::Stateful,
        "durable_entry! counter declares stateful — probed at ARTIFACT time"
    );

    let mut ctx = Context::new("shard");
    for name in ["s1", "s2"] {
        let art = art.clone();
        ctx.spawn(
            Spawn::new(
                Some(name),
                Box::new(move || Box::new(WasmActor::from_artifact(&art).expect("shard spawn"))),
            )
            .supervision(Supervision::Restart),
        )
        .unwrap();
    }
    for name in ["s1", "s2"] {
        ctx.post(&addr("shard", name), kick("shard", "external"), vec![], None);
    }
    ctx.run_until_idle(200);

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
    counts.sort();
    assert_eq!(
        counts,
        vec![1, 1],
        "two shards from one artifact: independent instances, each counting from zero"
    );
}

/// THE COST BOUNDARY — the serve path's per-spawn API cost carries
/// neither a compile nor a probe. Twenty `from_artifact` spawns must
/// cost less than ONE raw `Module::new` on the same bytes (the
/// per-invocation cost the debt measured at ~40 ms for a 165 KiB
/// guest). Spawn-side instantiation is lazy by design (round 37):
/// the first exchange pays it, and it is not what this pin guards.
#[test]
fn spawn_from_artifact_never_compiles() {
    if !have("mwydyn_guest_counter") {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let engine = WasmEngine::new().expect("engine");
    let wasm = artifact_bytes("mwydyn_guest_counter");

    let t0 = std::time::Instant::now();
    let raw = WtModule::new(engine.engine(), &wasm).expect("raw compile");
    let raw_compile = t0.elapsed();
    drop(raw);

    let t1 = std::time::Instant::now();
    let art = GuestArtifact::compile(&engine, &wasm).expect("artifact");
    let artifact_once = t1.elapsed();

    let t2 = std::time::Instant::now();
    for _ in 0..20 {
        let _ = WasmActor::from_artifact(&art).expect("spawn");
    }
    let twenty_spawns = t2.elapsed();

    eprintln!(
        "artifact economics: one compile {:?}, artifact(compile+probe) {:?}, 20 spawns {:?} ({:?}/spawn)",
        raw_compile,
        artifact_once,
        twenty_spawns,
        twenty_spawns / 20
    );
    assert!(
        twenty_spawns < raw_compile,
        "20 spawns from one artifact ({twenty_spawns:?}) must cost less than ONE compile ({raw_compile:?}) — the serve path never recompiles"
    );
}
