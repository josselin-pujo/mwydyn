//! Round 33 barrier tests — CBOR all the way across the WASM
//! boundary:
//! 1. mechanics: the container's alloc/write/process/read round trip
//!    against a runtime-generated WAT guest (machine-encoded bytes —
//!    never hand-typed hex),
//! 2. byzantine: a guest returning garbage responses CRASHES (the
//!    wall refuses; supervision classifies),
//! 3. compiled guest: the real mwydyn-guest-minimal artifact
//!    bouncing a nested work-order tree through the Context pump —
//!    decode/encode exercised in BOTH realms every hop.

use mwydyn_data::telemetry::{LifecycleEvent, TelPayload};
use mwydyn_core::{Context, Spawn};
use mwydyn_data::barrier::BarrierEffect;
use mwydyn_data::{address_to_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_wasm::{external_address, WasmActor, WasmEngine};
use std::path::Path;

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

/// A WAT guest that answers every request with one FIXED response
/// block — the response bytes are machine-encoded from Rust values
/// and spliced into a data segment (house rule: never hand-type hex).
fn fixed_response_guest(effect: &BarrierEffect) -> String {
    let resp = encode(&Value::Array(vec![effect.to_value()]));
    let mut block = Vec::with_capacity(4 + resp.len());
    block.extend_from_slice(&(resp.len() as u32).to_le_bytes());
    block.extend_from_slice(&resp);
    let escaped: String = block.iter().map(|b| format!("\\{b:02x}")).collect();
    let bump = 64 + block.len();
    format!(
        r#"(module
            (memory (export "memory") 1)
            (global $bump (mut i32) i32.const {bump})
            (data (i32.const 64) "{escaped}")
            (func (export "alloc") (param $len i32) (result i32)
                (global.set $bump (i32.add (global.get $bump) (local.get $len)))
                (i32.sub (global.get $bump) (local.get $len)))
            (func (export "dealloc") (param $p i32) (param $n i32))
            (func (export "process") (param $p i32) (param $n i32) (result i32)
                i32.const 64))
        "#
    )
}

#[derive(Clone)]
struct Collector {
    tag: String,
}

impl mwydyn_core::Actor for Collector {
    fn fork(&self) -> Box<dyn mwydyn_core::Actor> {
        Box::new(self.clone())
    }
    fn receive(
        &mut self,
        _ctx: &mwydyn_core::MsgCtx,
        _host: &mwydyn_core::Host,
        letter: &Letter,
    ) -> Vec<mwydyn_core::Effect> {
        vec![mwydyn_core::Effect::log(
            mwydyn_core::Level::Info,
            format!("{}:{}:{}", self.tag, letter.kind, letter.payload.len()),
        )]
    }
}

#[test]
fn barrier_mechanics_round_trip() {
    // The canned guest always posts one fixed letter to the collector.
    let canned = BarrierEffect::Post {
        to: addr("bm", "sink"),
        letter: Letter::new(
            "test.canned@1",
            encode(&Value::Text("hello from the guest".into())),
            None,
            None,
            vec![],
        )
        .unwrap(),
    };
    let wat = fixed_response_guest(&canned);

    let mut ctx = Context::new("bm");
    ctx.spawn(
        Spawn::new(Some("sink"), Box::new(|| Box::new(Collector { tag: "s".into() })))
            .supervision(mwydyn_core::Supervision::Restart),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(
            Some("guest"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_wat(&engine, &wat).expect("guest instantiates"))
            }),
        )
        .supervision(mwydyn_core::Supervision::Restart),
    )
    .unwrap();

    let kick = Letter::new("test.kick@1", encode(&Value::Text("go".into())), None, None, vec![])
        .unwrap();
    ctx.post(&addr("bm", "guest"), kick, vec![], None);
    ctx.run_until_idle(50);

    // The container's request/alloc/write → process → read/decode
    // round trip delivered the guest's canned effect into the sink.
    assert!(
        ctx.telemetry()
            .iter()
            .any(|r| {
                matches!(&r.payload, TelPayload::Log { message, .. }
                    if message.starts_with("s:test.canned@1:"))
            }),
        "canned guest effect landed: {:?}",
        ctx.telemetry()
    );
}

#[test]
fn byzantine_guest_responses_are_crashes_not_swallows() {
    // A guest returning garbage bytes goes through the REAL path:
    // the container panics inside the transactional receive, the
    // fork rolls back, supervision strikes — and after MAX_ATTEMPTS
    // the poison letter dead-letters. Nothing is ever silently
    // swallowed as an empty-effect commit.
    let mut garbage = Vec::new();
    garbage.extend_from_slice(&2u32.to_le_bytes());
    garbage.extend_from_slice(&[0xFF, 0x00]);
    let escaped: String = garbage.iter().map(|b| format!("\\{b:02x}")).collect();
    let wat_garbage = fixed_response_wat_from_block(&garbage, &escaped);

    let mut ctx = Context::new("bz");
    ctx.spawn(
        Spawn::new(
            Some("liar"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_wat(&engine, &wat_garbage).expect("guest instantiates"))
            }),
        )
        .supervision(mwydyn_core::Supervision::Restart),
    )
    .unwrap();
    let kick =
        Letter::new("test.kick@1", encode(&Value::Text("go".into())), None, None, vec![]).unwrap();
    ctx.post(&addr("bz", "liar"), kick, vec![], None);
    ctx.run_until_idle(50);

    assert_eq!(ctx.dead_letters().len(), 1, "the poison letter dead-letters");
    assert!(
        ctx.telemetry()
            .iter()
            .any(|r| {
                matches!(&r.payload, TelPayload::Lifecycle {
                    event: LifecycleEvent::Crashed,
                    detail: Some(d),
                } if d.contains("crashed") || d.contains("restarted"))
            }),
        "crash path telemetry: {:?}",
        ctx.telemetry()
    );
}

/// Same fixed-response WAT skeleton, but with an arbitrary (host
/// provided) block as the canned response.
fn fixed_response_wat_from_block(block: &[u8], escaped: &str) -> String {
    let bump = 64 + block.len();
    format!(
        r#"(module
            (memory (export "memory") 1)
            (global $bump (mut i32) i32.const {bump})
            (data (i32.const 64) "{escaped}")
            (func (export "alloc") (param $len i32) (result i32)
                (global.set $bump (i32.add (global.get $bump) (local.get $len)))
                (i32.sub (global.get $bump) (local.get $len)))
            (func (export "dealloc") (param $p i32) (param $n i32))
            (func (export "process") (param $p i32) (param $n i32) (result i32)
                i32.const 64))
        "#
    )
}

/// The compiled guest: a nested work-order tree bounced between two
/// WASM actors through the full pump. Correctness: hops + float
/// total survive N bounces through decode/encode in BOTH realms.
#[test]
fn compiled_guest_work_order_bounce() {
    let artifact = Path::new("tests/artifacts/mwydyn_guest_minimal.wasm");
    if !artifact.exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let wasm = std::fs::read(artifact).expect("artifact reads");

    let spawn_guests = |ctx: &mut Context| {
        for name in ["ping", "pong"] {
            let wasm = wasm.clone();
            ctx.spawn(
                Spawn::new(
                    Some(name),
                    Box::new(move || {
                        let engine = WasmEngine::new().expect("engine");
                        Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
                    }),
                )
                .supervision(mwydyn_core::Supervision::Restart),
            )
            .unwrap();
        }
    };

    fn work_order(reply_to: &Address, max_hops: i64) -> Vec<u8> {
        let item = |sku: &str, qty: i64, price: f64| {
            Value::map(vec![
                ("qty".into(), Value::Int(qty)),
                ("sku".into(), Value::Text(sku.into())),
                ("unitPrice".into(), Value::Float(price)),
            ])
            .unwrap()
        };
        encode(&Value::map(vec![
            ("due".into(), Value::Timestamp(1_770_000_000_000_000)),
            ("hops".into(), Value::Array(vec![])),
            (
                "items".into(),
                Value::Array(vec![item("widget", 3, 2.5), item("gadget", 2, 1.25)]),
            ),
            ("maxHops".into(), Value::Int(max_hops)),
            ("orderId".into(), Value::Text("wo-1".into())),
            ("priority".into(), Value::Int(7)),
            ("replyTo".into(), address_to_value(reply_to)),
        ])
        .unwrap())
    }

    // 6 hops, then the final work-order reports OUT to the external
    // outbox with every hop stamped and the float total exact
    // (3×2.5 + 2×1.25 = 10.0 — the always-f64 rule).
    let mut ctx = Context::new("wo");
    spawn_guests(&mut ctx);
    let kick = Letter::new(
        "test.work-order@1",
        work_order(&addr("wo", "pong"), 6),
        None,
        None,
        vec![],
    )
    .unwrap();
    ctx.post(&addr("wo", "ping"), kick, vec![], None);
    ctx.run_until_idle(200);

    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "the completed work-order reports out");
    let final_order = decode(&drained[0].letter.payload, &DecodeOptions::default())
        .expect("final payload canonical");
    let Value::Map(pairs) = final_order else {
        panic!("final payload is a map");
    };
    let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v);
    let Some(Value::Array(hops)) = get("hops") else {
        panic!("hops array");
    };
    assert_eq!(hops.len(), 6, "every hop stamped");
    assert_eq!(get("total"), Some(&Value::Float(10.0)), "float total exact");
    let _ = external_address(); // referenced for the report-out contract
}

/// Throughput: 1000 hops of the same tree through the full pump —
/// the CBOR-tier measurement against the native and i64-raw
/// baselines. Lenient floor: parallel test runs share cores.
#[test]
fn compiled_guest_work_order_throughput() {
    let artifact = Path::new("tests/artifacts/mwydyn_guest_minimal.wasm");
    if !artifact.exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let wasm = std::fs::read(artifact).expect("artifact reads");

    let mut ctx = Context::new("wp");
    for name in ["ping", "pong"] {
        let wasm = wasm.clone();
        ctx.spawn(
            Spawn::new(
                Some(name),
                Box::new(move || {
                    let engine = WasmEngine::new().expect("engine");
                    Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
                }),
            )
            .supervision(mwydyn_core::Supervision::Restart),
        )
        .unwrap();
    }
    let reply_to = address_to_value(&addr("wp", "pong"));
    let items = Value::Array(vec![
        Value::map(vec![
            ("qty".into(), Value::Int(3)),
            ("sku".into(), Value::Text("widget".into())),
            ("unitPrice".into(), Value::Float(2.5)),
        ])
        .unwrap(),
        Value::map(vec![
            ("qty".into(), Value::Int(2)),
            ("sku".into(), Value::Text("gadget".into())),
            ("unitPrice".into(), Value::Float(1.25)),
        ])
        .unwrap(),
    ]);
    let n = 1_000i64;
    let payload = encode(&Value::map(vec![
        ("due".into(), Value::Timestamp(1_770_000_000_000_000)),
        ("hops".into(), Value::Array(vec![])),
        ("items".into(), items),
        ("maxHops".into(), Value::Int(n)),
        ("orderId".into(), Value::Text("wo-perf".into())),
        ("priority".into(), Value::Int(7)),
        ("replyTo".into(), reply_to),
    ])
    .unwrap());
    let kick = Letter::new("test.work-order@1", payload, None, None, vec![]).unwrap();
    ctx.post(&addr("wp", "ping"), kick, vec![], None);
    let start = std::time::Instant::now();
    ctx.run_until_idle(100_000);
    let elapsed = start.elapsed();
    assert_eq!(ctx.drain_external().len(), 1, "completed exactly once");
    let throughput = n as f64 / elapsed.as_secs_f64();
    eprintln!(
        "CBOR WASM bounce: {} hops in {:?} → {:.0} hops/s",
        n, elapsed, throughput
    );
    assert!(
        throughput > 150.0,
        "CBOR bouncing >150 hops/s on the RAM tier (got {:.0})",
        throughput
    );
}

/// COMPOSE-PARITY UNDER THE POOL (round 34): WASM guests ride the
/// worker pool exactly like native actors — transitions move across
/// threads (Actor: Send), commits land in dispatch order, the
/// work-order completes and reports OUT through the same barrier.
#[test]
fn pool_runs_wasm_guests_and_natives_alike() {
    let artifact = Path::new("tests/artifacts/mwydyn_guest_minimal.wasm");
    if !artifact.exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let wasm = std::fs::read(artifact).expect("artifact reads");
    let mut ctx = Context::new("wpool");
    ctx.workers(4);
    for name in ["ping", "pong"] {
        let wasm = wasm.clone();
        ctx.spawn(
            Spawn::new(
                Some(name),
                Box::new(move || {
                    let engine = WasmEngine::new().expect("engine");
                    Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
                }),
            )
            .supervision(mwydyn_core::Supervision::Restart),
        )
        .unwrap();
    }
    let reply_to = address_to_value(&addr("wpool", "pong"));
    let items = Value::Array(vec![
        Value::map(vec![
            ("qty".into(), Value::Int(3)),
            ("sku".into(), Value::Text("widget".into())),
            ("unitPrice".into(), Value::Float(2.5)),
        ])
        .unwrap(),
    ]);
    let payload = encode(&Value::map(vec![
        ("due".into(), Value::Timestamp(1_770_000_000_000_000)),
        ("hops".into(), Value::Array(vec![])),
        ("items".into(), items),
        ("maxHops".into(), Value::Int(10)),
        ("orderId".into(), Value::Text("wo-pool".into())),
        ("priority".into(), Value::Int(7)),
        ("replyTo".into(), reply_to),
    ])
    .unwrap());
    let kick = Letter::new("test.work-order@1", payload, None, None, vec![]).unwrap();
    ctx.post(&addr("wpool", "ping"), kick, vec![], None);
    ctx.run_until_idle(500);
    let drained = ctx.drain_external();
    assert_eq!(drained.len(), 1, "the pooled bounce completed");
    let final_order = decode(&drained[0].letter.payload, &DecodeOptions::default()).expect("canonical");
    let Value::Map(pairs) = final_order else {
        panic!("final payload is a map");
    };
    let hops = pairs
        .iter()
        .find(|(k, _)| k == "hops")
        .map(|(_, v)| match v {
            Value::Array(h) => h.len(),
            _ => 0,
        })
        .unwrap_or(0);
    assert_eq!(hops, 10, "every hop stamped across pooled transitions");
}

/// THE POOL'S HEADLINE CLAIM (round 34): WASM transitions are
/// expensive (~300µs of decode/encode) — parallelizing them across
/// workers is where the pool pays. 8 guests × 100 work-orders,
/// serial vs pool(4).
#[test]
fn wasm_guest_transitions_parallelize() {
    let artifact = Path::new("tests/artifacts/mwydyn_guest_minimal.wasm");
    if !artifact.exists() {
        eprintln!("SKIP: guest artifact missing — run scripts/build-guest.sh");
        return;
    }
    let wasm = std::fs::read(artifact).expect("artifact reads");

    let run = |workers: usize| -> f64 {
        let mut ctx = Context::new("wt");
        if workers > 1 {
            ctx.workers(workers);
        }
        for g in 0..8 {
            let name = format!("guest-{g}");
            let wasm = wasm.clone();
            ctx.spawn(
                Spawn::new(
                    Some(&name),
                    Box::new(move || {
                        let engine = WasmEngine::new().expect("engine");
                        Box::new(WasmActor::from_compiled(&engine, &wasm).expect("guest"))
                    }),
                )
                .supervision(mwydyn_core::Supervision::Restart),
            )
            .unwrap();
        }
        let per_guest = 25; // ×2 transitions ×8 guests = 400 exchanges
        let start = std::time::Instant::now();
        for g in 0..8 {
            for k in 0..per_guest {
                let reply_to = address_to_value(&addr("wt", &format!("guest-{}", (g + 1) % 8)));
                let items = Value::Array(vec![Value::map(vec![
                    ("qty".into(), Value::Int(3)),
                    ("sku".into(), Value::Text("widget".into())),
                    ("unitPrice".into(), Value::Float(2.5)),
                ])
                .unwrap()]);
                let payload = encode(&Value::map(vec![
                    ("due".into(), Value::Timestamp(1_770_000_000_000_000)),
                    ("hops".into(), Value::Array(vec![])),
                    ("items".into(), items),
                    ("maxHops".into(), Value::Int(1)),
                    ("orderId".into(), Value::Text(format!("wo-{g}-{k}"))),
                    ("priority".into(), Value::Int(7)),
                    ("replyTo".into(), reply_to),
                ])
                .unwrap());
                let l = Letter::new("test.work-order@1", payload, None, None, vec![]).unwrap();
                ctx.post(&addr("wt", &format!("guest-{g}")), l, vec![], None);
            }
        }
        let stats = ctx.run_until_idle(10_000);
        let elapsed = start.elapsed();
        let done = ctx.drain_external().len();
        assert_eq!(done, 8 * per_guest, "every work-order reported out");
        if let Some(p) = stats.profile {
            let per_ex = p.committer_us_per_exchange(stats.passes);
            eprintln!(
                "pool profile (workers={workers}): waves={} wall={}us compose={}us stage={}us collect={}us apply={}us | committer={:.1}us/ex (bound ~{:.0}/s), wall={:.1}us/ex",
                p.waves, p.wall_us, p.compose_us, p.stage_dispatch_us, p.collect_us,
                p.apply_us, per_ex, 1e6 / per_ex.max(0.001),
                p.wall_us as f64 / stats.passes.max(1) as f64
            );
        }
        (8 * per_guest * 2) as f64 / elapsed.as_secs_f64()
    };

    let serial = run(0);
    let parallel = run(4);
    eprintln!(
        "WASM transitions, 8 guests: serial {:.0} exchanges/s, pool(4) {:.0} exchanges/s",
        serial, parallel
    );
}
