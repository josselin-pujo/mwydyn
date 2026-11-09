//! ROUND 34 — THE WORKER POOL (§6): transitions run in parallel on
//! worker threads; the pump thread commits in DISPATCH order.
//!
//! What parallel mode asserts: SEMANTIC outcomes (every letter lands,
//! refusals roll back, poison dead-letters). Exact ordering is the
//! journal's — whatever the pool produced is what the log says
//! ("non-deterministic but recorded"). The serial pump remains the
//! deterministic mode; `pool_and_serial_agree` proves the semantic
//! equivalence.

use mwydyn_core::{Actor, Context, Effect, Host, Level, MsgCtx, Spawn, Supervision};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};

fn cbor_text(s: &str) -> Vec<u8> {
    encode(&Value::Text(s.into()))
}

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

fn pool_ctx(name: &str, workers: usize) -> Context {
    let mut ctx = Context::new(name);
    ctx.workers(workers);
    ctx
}

#[derive(Clone)]
struct Echo;
impl Actor for Echo {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(self.clone())
    }
    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        vec![Effect::Post {
            to: ctx.from.clone(),
            letter: Letter::new(
                "test.echo.reply@1",
                letter.payload.clone(),
                None,
                letter.correlation.clone(),
                vec![],
            )
            .unwrap(),
        }]
    }
}

#[test]
fn fan_out_lands_every_letter_under_parallel_transitions() {
    let mut ctx = pool_ctx("fan", 4);
    ctx.spawn(
        Spawn::new(Some("echo-a"), Box::new(|| Box::new(Echo)))
            .supervision(Supervision::Restart),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(Some("echo-b"), Box::new(|| Box::new(Echo)))
            .supervision(Supervision::Restart),
    )
    .unwrap();

    let n = 200;
    for i in 0..n {
        let target = if i % 2 == 0 { "echo-a" } else { "echo-b" };
        let l = Letter::new(
            "test.m@1",
            cbor_text(&format!("m{i}")),
            None,
            Some(format!("r{i}")),
            vec![],
        )
        .unwrap();
        ctx.post(&addr("fan", target), l, vec![]);
    }
    ctx.run_until_idle(4_000);

    let replies: Vec<String> = ctx
        .drain_external()
        .iter()
        .filter_map(|e| e.letter.correlation.clone())
        .collect();
    assert_eq!(replies.len(), n, "every letter answered");
    let mut sorted = replies.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), n, "no duplicates: exactly-once delivery");
    for i in 0..n {
        assert!(replies.contains(&format!("r{i}")), "r{i} missing");
    }
}

#[test]
fn busy_refusal_rolls_back_and_heals_under_parallel() {
    #[derive(Clone)]
    struct Doubler {
        target: Address,
    }
    impl Actor for Doubler {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            let mk = || Letter::new("test.d@1", cbor_text("x"), None, None, vec![]).unwrap();
            vec![
                Effect::Post {
                    to: self.target.clone(),
                    letter: mk(),
                },
                Effect::Post {
                    to: self.target.clone(),
                    letter: mk(),
                },
            ]
        }
    }
    #[derive(Clone)]
    struct Sink {
        tag: String,
    }
    impl Actor for Sink {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
            vec![Effect::Emit {
                level: Level::Info,
                message: format!("{}:{}", self.tag, decode_text(&letter.payload)),
            }]
        }
    }
    fn decode_text(bytes: &[u8]) -> String {
        match decode(bytes, &DecodeOptions::default()) {
            Ok(Value::Text(s)) => s,
            _ => "<non-text>".into(),
        }
    }

    let mut ctx = pool_ctx("busy", 4);
    ctx.spawn(
        Spawn::new(
            Some("dbl"),
            Box::new(|| {
                Box::new(Doubler {
                    target: addr("busy", "sink"),
                })
            }),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(Some("sink"), Box::new(|| {
            Box::new(Sink {
                tag: "s".into(),
            })
        }))
        .bound(3)
        .supervision(Supervision::Restart),
    )
    .unwrap();
    for _ in 0..2 {
        let l = Letter::new("test.pre@1", cbor_text("p"), None, None, vec![]).unwrap();
        ctx.post(&addr("busy", "sink"), l, vec![]);
    }
    let go = Letter::new("test.go@1", cbor_text("g"), None, None, vec![]).unwrap();
    ctx.post(&addr("busy", "dbl"), go, vec![]);
    ctx.run_until_idle(1_000);

    // All four letters eventually landed (2 pre + 2 doubled), busy
    // refusals requeued without burning attempts, rollbacks held.
    let emissions = ctx
        .telemetry()
        .iter()
        .filter(|(_, m)| m.starts_with("s:p") || m.starts_with("s:x"))
        .count();
    assert_eq!(emissions, 4, "all four letters landed: {:?}", ctx.telemetry());
    assert!(ctx.dead_letters().is_empty());
}

#[test]
fn poison_dead_letters_through_the_strike_path_under_parallel() {
    #[derive(Clone)]
    struct Panicky;
    impl Actor for Panicky {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
            if letter.payload == cbor_text("boom") {
                panic!("synthetic boom");
            }
            vec![Effect::Emit {
                level: Level::Info,
                message: format!("ok:{}", decode_text(&letter.payload)),
            }]
        }
    }
    fn decode_text(bytes: &[u8]) -> String {
        match decode(bytes, &DecodeOptions::default()) {
            Ok(Value::Text(s)) => s,
            _ => "<non-text>".into(),
        }
    }

    let mut ctx = pool_ctx("poison", 4);
    ctx.spawn(
        Spawn::new(Some("pan"), Box::new(|| Box::new(Panicky)))
            .supervision(Supervision::Restart),
    )
    .unwrap();
    for payload in ["boom", "fine"] {
        let l = Letter::new("test.p@1", cbor_text(payload), None, None, vec![]).unwrap();
        ctx.post(&addr("poison", "pan"), l, vec![]);
    }
    ctx.run_until_idle(500);
    assert_eq!(ctx.dead_letters().len(), 1, "poison dead-letters");
    assert!(
        ctx.telemetry().iter().any(|(_, m)| m == "ok:fine"),
        "the healthy letter still ran"
    );
}

/// Semantic equivalence, not order equivalence: the same workload
/// through the serial pump and the pool produces the same multiset
/// of replies. Exact ordering is the journal's law — whatever the
/// pool produced is what the log records.
#[test]
fn pool_and_serial_agree_on_outcomes() {
    let run = |workers: usize| -> Vec<String> {
        let mut ctx = if workers > 1 {
            pool_ctx("equiv", workers)
        } else {
            Context::new("equiv")
        };
        ctx.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
            .unwrap();
        for i in 0..40 {
            let l = Letter::new(
                "test.m@1",
                cbor_text(&format!("m{i}")),
                None,
                Some(format!("r{i}")),
                vec![],
            )
            .unwrap();
            ctx.post(&addr("equiv", "echo"), l, vec![]);
        }
        ctx.run_until_idle(1_000);
        let mut replies: Vec<String> = ctx
            .drain_external()
            .iter()
            .map(|e| String::from_utf8_lossy(&e.letter.payload).to_string())
            .collect();
        replies.sort();
        replies
    };
    let serial = run(0);
    let parallel = run(4);
    assert_eq!(
        serial, parallel,
        "serial and parallel agree on the outcome multiset"
    );
}

/// THROUGHPUT: fan-out across 8 actors, serial vs pool(4). Transitions
/// here are cheap, so this measures the pool's WAVE MACHINERY, not
/// transition parallelism — the number to beat later (persistent
/// worker threads) is the spawn-per-wave cost visible in this gap.
/// The pool pays off where transitions are EXPENSIVE (WASM guests).
#[test]
fn fan_out_parallel_throughput_report() {
    use std::time::Instant;

    let run = |workers: usize| -> f64 {
        let mut ctx = if workers > 1 {
            pool_ctx("tp", workers)
        } else {
            Context::new("tp")
        };
        for a in 0..8 {
            let name = format!("echo-{a}");
            ctx.spawn(Spawn::new(Some(&name), Box::new(|| Box::new(Echo))))
                .unwrap();
        }
        let n = 2_000;
        let start = Instant::now();
        for i in 0..n {
            let target = format!("echo-{}", i % 8);
            let l = Letter::new(
                "test.m@1",
                cbor_text(&format!("m{i}")),
                None,
                Some(format!("r{i}")),
                vec![],
            )
            .unwrap();
            ctx.post(&addr("tp", &target), l, vec![]);
        }
        ctx.run_until_idle(20_000);
        let elapsed = start.elapsed();
        let replies = ctx.drain_external().len();
        assert_eq!(replies, n, "every letter answered at workers={workers}");
        n as f64 / elapsed.as_secs_f64()
    };
    let serial = run(0);
    let parallel = run(4);
    eprintln!(
        "fan-out 8 actors: serial {:.0} letters/s, pool(4) {:.0} letters/s",
        serial, parallel
    );
}
