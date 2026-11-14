//! ROUND 50 — THE TASK VERBS MEET THE AMBIENT SURFACE. The task
//! pattern (round 17) predates capability rows at the ambient face
//! (round 40): `call_task` spawned a capability-LESS task, so every
//! blob-working one-call round trip forced embedders to
//! re-implement the verb's body by hand (the first consumer's
//! bridge was the proof). `call_task_with(spec, letter)` threads a
//! full `Spawn` — rows, name, init_spec — through the existing
//! mint/post/run/outbox-match body. Five pins:
//!
//! - ZERO-GRANT DEFAULT: `call_task`'s task touching blobs is
//!   refused Unauthorized BY NAME (observed, never swallowed).
//! - THE ROWS-BEARING CALL: put/get green through granted rows —
//!   the same actor, the capability the only delta.
//! - THE MINT: correlations unique across back-to-back calls, each
//!   reply matched to its own request.
//! - THE BYSTANDER: non-matching outbox entries survive the scan.
//! - FAILFAST IS THE VERB'S: a Restart spec cannot make a one-call
//!   task outlive its strike.

use mwydyn_core::{Actor, CapRow, Context, Effect, Host, MsgCtx, Spawn, Supervision};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn txt(s: &str) -> Vec<u8> {
    encode(&Value::Text(s.into()))
}

fn payload_map(letter: &Letter) -> Vec<(String, Value)> {
    match decode(&letter.payload, &DecodeOptions::default()).expect("canonical") {
        Value::Map(pairs) => pairs,
        other => panic!("map payload, got {other:?}"),
    }
}

fn field<'a>(pairs: &'a [(String, Value)], k: &str) -> &'a Value {
    &pairs.iter().find(|(key, _)| key == k).expect(k).1
}

/// ONE actor, TWO verdicts: stage bytes through the ambient face and
/// report — refusal BY NAME when the rows are missing, the digest +
/// round-trip proof when they are granted. The capability is the
/// only delta between the two pins below.
struct TryStage;

impl Actor for TryStage {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(TryStage) // a stateless unit — fresh is faithful
    }

    fn receive(&mut self, ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        let report = match host.blob_put(b"round fifty", "text/x-round50", None) {
            Err(refused) => Value::map(vec![
                ("staged".into(), Value::Bool(false)),
                ("refused".into(), Value::Text(format!("{refused:?}"))),
            ])
            .unwrap(),
            Ok(r) => {
                let back = host.blob_get(&r);
                Value::map(vec![
                    ("staged".into(), Value::Bool(true)),
                    ("digest".into(), Value::Text(r.digest.clone())),
                    (
                        "roundtrip".into(),
                        Value::Bool(matches!(back, Ok(b) if b == b"round fifty")),
                    ),
                ])
                .unwrap()
            }
        };
        vec![Effect::Post {
            to: ctx.from.clone(),
            letter: Letter::new(
                "stage.reply@1",
                encode(&report),
                None,
                letter.correlation.clone(),
                vec![],
            )
            .unwrap(),
        }]
    }
}

#[test]
fn zero_grant_call_task_refuses_blob_work_by_name() {
    let mut ctx = Context::new("t50");
    let reply = ctx
        .call_task(
            Box::new(|| Box::new(TryStage)),
            Letter::new("stage.request@1", txt("go"), None, None, vec![]).unwrap(),
        )
        .expect("call")
        .expect("the refusal itself is reported, not swallowed into a strike");
    let pairs = payload_map(&reply);
    assert_eq!(field(&pairs, "staged"), &Value::Bool(false));
    assert_eq!(
        field(&pairs, "refused"),
        &Value::Text("Unauthorized".into()),
        "capability-quiet by default — and the refusal is BY NAME"
    );
}

#[test]
fn rows_bearing_call_task_does_blob_work() {
    let mut ctx = Context::new("t50");
    let spec = Spawn::new(Some("stager"), Box::new(|| Box::new(TryStage)))
        .rows(vec![
            CapRow::new("blob.read", "*"),
            CapRow::new("blob.write", "*"),
        ]);
    let reply = ctx
        .call_task_with(
            spec,
            Letter::new("stage.request@1", txt("go"), None, None, vec![]).unwrap(),
        )
        .expect("call")
        .expect("the staged reply");
    let pairs = payload_map(&reply);
    assert_eq!(field(&pairs, "staged"), &Value::Bool(true));
    assert_eq!(field(&pairs, "roundtrip"), &Value::Bool(true));
    assert!(matches!(field(&pairs, "digest"), Value::Text(d) if d.len() == 64));
    assert!(
        reply.correlation.as_deref().is_some_and(|c| c.starts_with("call-")),
        "the reply rides the verb's minted correlation"
    );
}

/// Echoes the request under the minted correlation AND posts one
/// uncorrelated bystander letter to the external outbox.
struct Chatty;

impl Actor for Chatty {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(Chatty)
    }

    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        vec![
            Effect::Post {
                to: ctx.from.clone(),
                letter: Letter::new(
                    "chat.reply@1",
                    letter.payload.clone(),
                    None,
                    letter.correlation.clone(),
                    vec![],
                )
                .unwrap(),
            },
            Effect::Post {
                to: Address::Instance("external".into()),
                letter: Letter::new("chat.noise@1", txt("unrelated"), None, None, vec![]).unwrap(),
            },
        ]
    }
}

#[test]
fn the_mint_is_unique_and_bystanders_survive_the_scan() {
    let mut ctx = Context::new("t50");
    let mut correlations = Vec::new();
    for marker in ["one", "two"] {
        let reply = ctx
            .call_task(
                Box::new(|| Box::new(Chatty)),
                Letter::new("chat.request@1", txt(marker), None, None, vec![]).unwrap(),
            )
            .expect("call")
            .unwrap_or_else(|| panic!("reply for {marker} matched by its own correlation"));
        assert_eq!(reply.payload, txt(marker), "each call matched ITS reply");
        correlations.push(reply.correlation.expect("minted"));
    }
    assert_ne!(correlations[0], correlations[1], "call-N is unique per call");

    // Both bystanders survived both scans — the match never drains
    // what it did not match.
    let bystanders = ctx.drain_external();
    assert_eq!(bystanders.len(), 2, "one bystander per call, both kept");
    assert!(bystanders
        .iter()
        .all(|e| e.letter.kind == "chat.noise@1" && e.letter.correlation.is_none()));
}

/// Strikes on first touch, answers honestly ever after — IF anything
/// gives it a second attempt. Under a RESPECTED Restart spec the
/// respawned task would answer attempt two; the verb forces FailFast,
/// so a one-call task never outlives its strike.
struct PoisonOnce(Arc<AtomicBool>);

impl Actor for PoisonOnce {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(PoisonOnce(Arc::clone(&self.0))) // the flag rides: once poisoned, always poisoned
    }

    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        if !self.0.swap(true, Ordering::SeqCst) {
            panic!("first touch strikes (by design)");
        }
        vec![Effect::Post {
            to: ctx.from.clone(),
            letter: Letter::new(
                "poison.reply@1",
                txt("survived"),
                None,
                letter.correlation.clone(),
                vec![],
            )
            .unwrap(),
        }]
    }
}

#[test]
fn failfast_is_the_verbs_not_the_specs() {
    let mut ctx = Context::new("t50");
    let flag = Arc::new(AtomicBool::new(false));
    let f = flag.clone();
    let spec = Spawn::new(None, Box::new(move || Box::new(PoisonOnce(f.clone()))))
        .supervision(Supervision::Restart); // the verb must NOT honor this
    let reply = ctx
        .call_task_with(
            spec,
            Letter::new("poison.request@1", txt("try"), None, None, vec![]).unwrap(),
        )
        .expect("call");
    assert!(
        reply.is_none(),
        "no answer: FailFast was forced — under Restart the respawned task would have answered attempt two"
    );
    assert!(
        !ctx.dead_letters().is_empty(),
        "the struck request dead-lettered"
    );
}
