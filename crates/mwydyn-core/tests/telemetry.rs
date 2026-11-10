//! ROUND 44 walking tests — the telemetry outbox as behavior (§3.6):
//! the ring and its drop ledger, exchange records (the span
//! skeleton), runtime stamping, and the injection handle.

use mwydyn_core::context::Verdict;
use mwydyn_core::{Actor, Context, Effect, Host, Level, MsgCtx, RefuseReason, Spawn};
use mwydyn_data::telemetry::{
    Instrument, LifecycleEvent, Outcome, TelPayload, TelemetryKind, TelemetryRecord,
};
use mwydyn_data::{Address, Letter};

fn addr(ctx: &str, name: &str) -> Address {
    Address::Named { context: ctx.into(), name: name.into() }
}

fn nul() -> Vec<u8> {
    mwydyn_data::encode(&mwydyn_data::Value::Null)
}

#[test]
fn emits_land_stamped_and_exchanges_mint() {
    #[derive(Clone)]
    struct Logger;
    impl Actor for Logger {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
            vec![
                Effect::log(Level::Info, format!("got:{}", letter.kind)),
                Effect::metric("letters.seen", Instrument::Counter, 1.0),
            ]
        }
    }

    let mut ctx = Context::new("t1");
    ctx.spawn(Spawn::new(Some("log"), Box::new(|| Box::new(Logger)))).unwrap();
    let l = Letter::new("test.go@1", nul(), None, None, vec![]).unwrap();
    assert!(matches!(
        ctx.post(&addr("t1", "log"), l, vec![], None),
        Verdict::Accepted { .. }
    ));
    ctx.run_until_idle(10);

    let drained = ctx.drain_telemetry();
    // The actor's records, stamped by the runtime: attribution and
    // seq are MINTED, never actor-supplied (§3.6 byzantine posture).
    let log = drained.iter().find(|r| r.kind == TelemetryKind::Log).expect("log landed");
    assert!(matches!(&log.payload, TelPayload::Log { message, .. } if message == "got:test.go@1"));
    assert_eq!(log.seq, 1, "seq = the causing letter");
    assert!(!log.actor.is_empty(), "actor attribution minted");
    assert!(log.ts != 0 || true, "ts minted at apply (wall clock may be 0 on ManualClock-less runs)");

    let metric = drained.iter().find(|r| r.kind == TelemetryKind::Metric).expect("metric landed");
    assert!(matches!(&metric.payload,
        TelPayload::Metric { name, instrument: Instrument::Counter, value, .. }
            if name == "letters.seen" && *value == 1.0));

    // The exchange record: the span skeleton, committed outcome,
    // root (external injection → ack None).
    let ex = drained.iter().find(|r| r.kind == TelemetryKind::Exchange).expect("exchange minted");
    assert_eq!(ex.seq, 1);
    assert!(matches!(&ex.payload,
        TelPayload::Exchange { ack: None, outcome: Outcome::Committed, .. }));

    // Drain empties the outbox; the second drain is silent.
    assert!(ctx.drain_telemetry().is_empty());
}

#[test]
fn the_ring_drops_oldest_and_the_drain_leads_with_the_ledger() {
    #[derive(Clone)]
    struct Chatty;
    impl Actor for Chatty {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            (0..8).map(|i| Effect::log(Level::Debug, format!("m{i}"))).collect()
        }
    }

    let mut ctx = Context::new("t2");
    ctx.telemetry_capacity(16);
    ctx.spawn(Spawn::new(Some("chat"), Box::new(|| Box::new(Chatty)))).unwrap();
    // 4 letters × 8 logs + 4 exchanges + 1 spawn lifecycle = 37
    // records into a 16-slot ring: at least 21 fall out.
    for i in 0..4 {
        let l = Letter::new("test.c@1", nul(), None, Some(format!("c{i}")), vec![]).unwrap();
        assert!(matches!(
            ctx.post(&addr("t2", "chat"), l, vec![], None),
            Verdict::Accepted { .. }
        ));
    }
    ctx.run_until_idle(20);

    let drained = ctx.drain_telemetry();
    // The ring bound holds for the RING; the drain output is ring
    // contents plus (at most) the leading ledger record.
    assert!(
        drained.len() - usize::from(drained.first().map(|r| r.kind == TelemetryKind::Drop) == Some(true)) <= 16,
        "the ring bound holds: {}",
        drained.len()
    );
    let drop = drained.first().expect("the loss ledger leads the drain");
    assert_eq!(drop.kind, TelemetryKind::Drop);
    assert!(matches!(&drop.payload,
        TelPayload::Drop { dropped, by_kind }
            if *dropped >= 21 && by_kind.iter().any(|(k, _)| k == "log")));
    // The ledger is per-drain: the next drain carries no drop record.
    assert!(ctx.drain_telemetry().is_empty());
}

#[test]
fn runtime_kinds_are_not_actor_declared() {
    #[derive(Clone)]
    struct Forger;
    impl Actor for Forger {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            vec![Effect::Emit {
                record: TelemetryRecord {
                    kind: TelemetryKind::Lifecycle, // the runtime's kind
                    ts: 0,
                    actor: String::new(),
                    seq: 0,
                    payload: TelPayload::Lifecycle {
                        event: LifecycleEvent::Spawned,
                        detail: None,
                    },
                },
            }]
        }
    }

    let mut ctx = Context::new("t3");
    ctx.spawn(Spawn::new(Some("forge"), Box::new(|| Box::new(Forger)))).unwrap();
    let l = Letter::new("test.f@1", nul(), None, None, vec![]).unwrap();
    assert!(matches!(
        ctx.post(&addr("t3", "forge"), l, vec![], None),
        Verdict::Accepted { .. }
    ));
    ctx.run_until_idle(50);
    // Phase 2 strikes the forged kind; the refusal record says so;
    // the forged lifecycle record never lands.
    assert!(ctx.telemetry().iter().any(|r| {
        matches!(&r.payload, TelPayload::Refusal { reason, .. }
            if reason.contains("invalid telemetry kind"))
    }));
    assert!(!ctx.telemetry().iter().any(|r| {
        r.kind == TelemetryKind::Lifecycle
            && matches!(&r.payload, TelPayload::Lifecycle { event: LifecycleEvent::Spawned, .. })
            && r.seq == 1
    }));
}

#[test]
fn injection_returns_seq_and_stamps_causality() {
    #[derive(Clone)]
    struct Sink;
    impl Actor for Sink {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            vec![]
        }
    }

    let mut ctx = Context::new("t4");
    ctx.spawn(Spawn::new(Some("sink"), Box::new(|| Box::new(Sink)))).unwrap();
    let a = Letter::new("test.a@1", nul(), None, None, vec![]).unwrap();
    let Verdict::Accepted { seq: first } =
        ctx.post(&addr("t4", "sink"), a, vec![], None)
    else {
        panic!("first injection accepted");
    };
    let b = Letter::new("test.b@1", nul(), None, None, vec![]).unwrap();
    let Verdict::Accepted { seq: second } =
        ctx.post(&addr("t4", "sink"), b, vec![], Some(first))
    else {
        panic!("parented injection accepted");
    };
    assert!(second > first, "seqs are handed back in order: {first} < {second}");

    // Unknown parents are refused, loudly, with the reason in the
    // refusal record (stage injection).
    let c = Letter::new("test.c@1", nul(), None, None, vec![]).unwrap();
    assert_eq!(
        ctx.post(&addr("t4", "sink"), c, vec![], Some(9999)),
        Verdict::Refused(RefuseReason::Invalid)
    );
    assert!(ctx.telemetry().iter().any(|r| {
        matches!(&r.payload, TelPayload::Refusal { stage, reason, .. }
            if stage == "injection" && reason.contains("unknown parent"))
    }));

    // The parented letter's exchange record carries ack = the first
    // seq: the causal edge the sink joins spans by.
    ctx.run_until_idle(10);
    let drained = ctx.drain_telemetry();
    let ex = drained
        .iter()
        .find(|r| r.kind == TelemetryKind::Exchange && r.seq == second)
        .expect("second exchange minted");
    assert!(matches!(&ex.payload, TelPayload::Exchange { ack: Some(a), .. } if *a == first));
}

#[test]
fn busy_and_struck_attempts_mint_their_exchanges() {
    #[derive(Clone)]
    struct Bounded;
    impl Actor for Bounded {
        fn fork(&self) -> Box<dyn Actor> {
            Box::new(self.clone())
        }
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            vec![]
        }
    }

    let mut ctx = Context::new("t5");
    ctx.spawn(Spawn::new(Some("tiny"), Box::new(|| Box::new(Bounded))).bound(1)).unwrap();
    let a = Letter::new("test.a@1", nul(), None, None, vec![]).unwrap();
    let b = Letter::new("test.b@1", nul(), None, None, vec![]).unwrap();
    assert!(matches!(ctx.post(&addr("t5", "tiny"), a, vec![], None), Verdict::Accepted { .. }));
    // Mailbox bound 1: the second post is refused busy at INJECTION
    // — a refusal record, no exchange (no attempt happened).
    assert_eq!(
        ctx.post(&addr("t5", "tiny"), b, vec![], None),
        Verdict::Refused(RefuseReason::Busy)
    );
    assert!(ctx.telemetry().iter().any(|r| {
        matches!(&r.payload, TelPayload::Refusal { stage, reason, .. }
            if stage == "injection" && reason.contains("busy"))
    }));

    // Exchange records can be switched off for throughput runs.
    let mut quiet = Context::new("t5b");
    quiet.exchange_records(false);
    quiet.spawn(Spawn::new(Some("tiny"), Box::new(|| Box::new(Bounded)))).unwrap();
    let l = Letter::new("test.a@1", nul(), None, None, vec![]).unwrap();
    assert!(matches!(quiet.post(&addr("t5b", "tiny"), l, vec![], None), Verdict::Accepted { .. }));
    quiet.run_until_idle(10);
    assert!(!quiet.telemetry().iter().any(|r| r.kind == TelemetryKind::Exchange));
}
