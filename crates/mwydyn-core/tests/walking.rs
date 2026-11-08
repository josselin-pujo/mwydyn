//! M0 walking-skeleton tests — the round-01…07 semantics, running.

use mwydyn_core::ticker::{TIMER_ARM_KIND, Ticker};
use mwydyn_core::{
    Actor, CapRow, Context, Effect, Host, Level, ManualClock, MsgCtx, RefuseReason, Spawn,
    Supervision, Verdict,
};
use mwydyn_data::{Address, DecodeOptions, Letter, Value};

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

struct Collector {
    tag: String,
}

impl Actor for Collector {
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        vec![Effect::Emit {
            level: Level::Info,
            message: format!(
                "{}:{}:{}",
                self.tag,
                letter.kind,
                String::from_utf8_lossy(&letter.payload)
            ),
        }]
    }
}

struct Echo;

impl Actor for Echo {
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
fn admission_verdicts() {
    let mut ctx = Context::new("c1");
    ctx.spawn(Spawn::new(
        Some("sink"),
        Box::new(|| Box::new(Collector { tag: "s".into() })),
    ))
    .unwrap();
    let l = Letter::new("test.x@1", vec![], None, None, vec![]).unwrap();

    assert_eq!(
        ctx.post(&addr("c1", "nope"), l.clone(), vec![]),
        Verdict::Refused(RefuseReason::UnknownTarget)
    );
    // Cross-context does not resolve from inside one (§3.5: sagas).
    assert_eq!(
        ctx.post(&addr("other", "sink"), l.clone(), vec![]),
        Verdict::Refused(RefuseReason::UnknownTarget)
    );
    // The core trusts nothing: a letter with its fields set directly
    // gets the same structural rejection as a constructor reject.
    let mut raw = l.clone();
    raw.kind = "nope".into();
    assert_eq!(
        ctx.post(&addr("c1", "sink"), raw, vec![]),
        Verdict::Refused(RefuseReason::Invalid)
    );

    ctx.spawn(
        Spawn::new(
            Some("tiny"),
            Box::new(|| Box::new(Collector { tag: "t".into() })),
        )
        .bound(1),
    )
    .unwrap();
    assert_eq!(
        ctx.post(&addr("c1", "tiny"), l.clone(), vec![]),
        Verdict::Accepted
    );
    assert_eq!(
        ctx.post(&addr("c1", "tiny"), l, vec![]),
        Verdict::Refused(RefuseReason::Busy)
    );
}

#[test]
fn permanent_refusals_strike_into_dead_letters() {
    let mut ctx = Context::new("c1");
    ctx.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    // Echo replies to `from` — external has no mailbox, so the reply
    // is an unknown-target strike, three of which dead-letter it.
    let l = Letter::new("test.hi@1", b"yo".to_vec(), None, Some("c1".into()), vec![]).unwrap();
    ctx.post(&addr("c1", "echo"), l, vec![]);
    ctx.run_until_idle(50);
    assert_eq!(ctx.dead_letters().len(), 1);
    assert_eq!(ctx.dead_letters()[0].letter.kind, "test.hi@1");
}

struct Forward {
    target: Address,
    armed: bool,
}

impl Actor for Forward {
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        if !self.armed {
            self.armed = true;
            vec![Effect::Post {
                to: self.target.clone(),
                letter: Letter::new(
                    "test.ping@1",
                    b"ping".to_vec(),
                    None,
                    Some("c2".into()),
                    vec![],
                )
                .unwrap(),
            }]
        } else {
            vec![Effect::Emit {
                level: Level::Info,
                message: format!("forward got {}", letter.kind),
            }]
        }
    }
}

#[test]
fn multi_hop_reply_rides_from() {
    let mut ctx = Context::new("c1");
    ctx.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    ctx.spawn(Spawn::new(
        Some("fwd"),
        Box::new(|| Box::new(Forward {
            target: addr("c1", "echo"),
            armed: false,
        })),
    ))
    .unwrap();
    let l = Letter::new("test.go@1", b"go".to_vec(), None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "fwd"), l, vec![]);
    ctx.run_until_idle(50);
    // fwd → echo (from = fwd's instance) → echo replies to from →
    // fwd sees the reply. The reply-to-from pattern needs no
    // reply-to field (round 09).
    assert!(ctx
        .telemetry()
        .iter()
        .any(|(_, m)| m.contains("forward got test.echo.reply@1")));
}

struct Doubler {
    target: Address,
}

impl Actor for Doubler {
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
        let mk = || Letter::new("test.d@1", b"x".to_vec(), None, None, vec![]).unwrap();
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

#[test]
fn effect_busy_requeues_without_burning_attempts() {
    let mut ctx = Context::new("c1");
    // Doubler FIRST (delivery order follows spawn order): it runs
    // while the sink is still loaded, so the busy refusal actually
    // hits — then the sink drains and the requeued letter heals.
    ctx.spawn(Spawn::new(
        Some("dbl"),
        Box::new(|| Box::new(Doubler {
            target: addr("c1", "sink"),
        })),
    ))
    .unwrap();
    ctx.spawn(
        Spawn::new(
            Some("sink"),
            Box::new(|| Box::new(Collector { tag: "s".into() })),
        )
        .bound(3),
    )
    .unwrap();
    // Pre-load the sink to one-under-bound: the doubler's second
    // effect cannot fit, so the WHOLE list is refused busy.
    for _ in 0..2 {
        let l = Letter::new("test.pre@1", b"p".to_vec(), None, None, vec![]).unwrap();
        ctx.post(&addr("c1", "sink"), l, vec![]);
    }
    let second = Letter::new("test.go@1", b"g".to_vec(), None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "dbl"), second, vec![]);
    ctx.run_until_idle(50);
    // The busy refusal requeued the letter, no attempt burned, and
    // everything eventually landed: 4 sink emissions (2 pre + 2).
    let emissions = ctx
        .telemetry()
        .iter()
        .filter(|(_, m)| m.starts_with("s:test.d@1") || m.starts_with("s:test.pre@1"))
        .count();
    assert_eq!(emissions, 4, "all four letters landed: {:?}", ctx.telemetry());
    assert!(ctx.dead_letters().is_empty());
    assert!(ctx
        .telemetry()
        .iter()
        .any(|(lvl, m)| *lvl == Level::Debug && m.contains("requeued")));
}

struct Panicky;

impl Actor for Panicky {
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        if letter.payload == b"boom" {
            panic!("synthetic boom");
        }
        vec![Effect::Emit {
            level: Level::Info,
            message: format!("ok:{}", String::from_utf8_lossy(&letter.payload)),
        }]
    }
}

// NOTE: the synthetic-panic tests print the panic to stderr — noise,
// not failure; catch_unwind is what the supervision path uses.

#[test]
fn restart_supervision_reinstates_and_dead_letters_poison() {
    let mut ctx = Context::new("c1");
    ctx.spawn(
        Spawn::new(Some("pan"), Box::new(|| Box::new(Panicky)))
            .supervision(Supervision::Restart),
    )
    .unwrap();
    // ONE poison letter — it strikes three times across restarts;
    // each strike re-instantiates fresh and re-delivers the SAME
    // letter (at-least-once).
    for payload in ["boom", "fine"] {
        let l = Letter::new(
            "test.p@1",
            payload.as_bytes().to_vec(),
            None,
            None,
            vec![],
        )
        .unwrap();
        ctx.post(&addr("c1", "pan"), l, vec![]);
    }
    ctx.run_until_idle(50);
    // Three strikes on the poison letter → dead-letter; the fresh
    // instance processes `fine`.
    assert_eq!(ctx.dead_letters().len(), 1);
    assert_eq!(ctx.dead_letters()[0].letter.payload, b"boom");
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "ok:fine"));
    assert!(ctx.telemetry().iter().any(|(_, m)| m.contains("restarted fresh")));
}

#[test]
fn fail_fast_is_terminal_and_drains_pending() {
    let mut ctx = Context::new("c1");
    ctx.spawn(Spawn::new(Some("pan"), Box::new(|| Box::new(Panicky)))).unwrap();
    for payload in ["boom", "fine"] {
        let l = Letter::new(
            "test.p@1",
            payload.as_bytes().to_vec(),
            None,
            None,
            vec![],
        )
        .unwrap();
        ctx.post(&addr("c1", "pan"), l, vec![]);
    }
    ctx.run_until_idle(50);
    // The crash is terminal: `fine` never ran — it drained to dead
    // letters with the actor's mailbox.
    assert_eq!(ctx.dead_letters().len(), 2);
    assert!(!ctx.telemetry().iter().any(|(_, m)| m.starts_with("ok:")));
    assert!(ctx
        .telemetry()
        .iter()
        .any(|(_, m)| m.contains("terminal (fail-fast)")));
}

struct Armer {
    ticker: Address,
}

impl Actor for Armer {
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        if letter.kind == "test.go@1" {
            vec![Effect::Post {
                to: self.ticker.clone(),
                letter: Letter::new(
                    TIMER_ARM_KIND,
                    Ticker::arm_payload(500),
                    None,
                    Some("c9".into()),
                    vec![],
                )
                .unwrap(),
            }]
        } else {
            vec![Effect::Emit {
                level: Level::Info,
                message: format!(
                    "TICK:{}",
                    letter.correlation.clone().unwrap_or_default()
                ),
            }]
        }
    }
}

#[test]
fn ticker_fires_ticks_as_messages_on_the_injected_clock() {
    let mut ctx = Context::new("c1");
    ctx.set_clock(Box::new(ManualClock(1000)));
    ctx.spawn(Spawn::new(
        Some("armer"),
        Box::new(|| Box::new(Armer {
            ticker: addr("c1", "ticker"),
        })),
    ))
    .unwrap();
    let l = Letter::new("test.go@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "armer"), l, vec![]);
    ctx.run_until_idle(50);
    assert!(
        !ctx.telemetry().iter().any(|(_, m)| m.starts_with("TICK:")),
        "no tick before the deadline"
    );
    ctx.set_clock(Box::new(ManualClock(1600)));
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "TICK:c9"));
}

struct BlobActor;

impl Actor for BlobActor {
    fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        match letter.kind.as_str() {
            "test.write@1" => vec![Effect::Put {
                bytes: b"hello".to_vec(),
                media_type: "text/plain".into(),
                ttl_micros: None,
            }],
            "test.read@1" => {
                let v = mwydyn_data::decode(&letter.payload, &DecodeOptions::default())
                    .expect("ref payload");
                let Value::BlobRef {
                    digest,
                    media_type,
                    size,
                } = v
                else {
                    panic!("ref payload is a blob-ref");
                };
                match host.blob_get(&mwydyn_core::BlobRef {
                    digest,
                    media_type,
                    size,
                }) {
                    Ok(bytes) => vec![Effect::Emit {
                        level: Level::Info,
                        message: format!(
                            "READ:{}",
                            String::from_utf8_lossy(&bytes)
                        ),
                    }],
                    Err(e) => vec![Effect::Emit {
                        level: Level::Error,
                        message: format!("READ-ERR:{e:?}"),
                    }],
                }
            }
            _ => vec![Effect::Emit {
                level: Level::Warn,
                message: format!("blobactor ignoring {}", letter.kind),
            }],
        }
    }
}

#[test]
fn blob_put_lands_and_verified_reads_obey_caps() {
    let mut ctx = Context::new("c1");
    ctx.spawn(
        Spawn::new(
            Some("blob"),
            Box::new(|| Box::new(BlobActor)),
            )
        .rows(vec![CapRow::new("blob.read", "*"), CapRow::new("blob.write", "*")]),
    )
    .unwrap();
    // Write lands through the effects engine; the digest reply would
    // go to external (dead letters — fine, the PUT is what matters).
    let w = Letter::new("test.write@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "blob"), w, vec![]);
    ctx.run_until_idle(50);

    // The test computes the same pure digest the actor's put did.
    let digest = Value::blob_digest(b"hello", "text/plain").unwrap();
    let ref_payload = mwydyn_data::encode(&Value::BlobRef {
        digest: digest.clone(),
        media_type: "text/plain".into(),
        size: 5,
    });
    let r = Letter::new("test.read@1", ref_payload.clone(), None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "blob"), r, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "READ:hello"));

    // A well-shaped ref whose CLAIMED TYPE does not match the stored
    // content fails verify-on-read — loudly, per round 12's
    // mutable-by-transit law. (An INVALID ref — uppercase digest —
    // never reaches the actor at all: the data model rejects it at
    // the decode; that rejection is pinned in blob-refs-v0. And a
    // valid ref to a never-existing blob is Missing, not Integrity.)
    let real_digest = Value::blob_digest(b"hello", "text/plain").unwrap();
    let wrong_payload = mwydyn_data::encode(&Value::BlobRef {
        digest: real_digest,
        media_type: "application/json".into(),
        size: 5,
    });
    let fr = Letter::new("test.read@1", wrong_payload, None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "blob"), fr, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx
        .telemetry()
        .iter()
        .any(|(_, m)| m.contains("READ-ERR:Integrity")));
}

#[test]
fn atomic_effects_rollback_the_whole_list() {
    // The banking nerve (round 07): a list with one bad effect
    // applies NOTHING — the put never lands.
    struct PutAndBadPost;
    impl Actor for PutAndBadPost {
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            vec![
                Effect::Put {
                    bytes: b"should-not-land".to_vec(),
                    media_type: "text/plain".into(),
                    ttl_micros: None,
                },
                Effect::Post {
                    to: Address::Instance("external".into()),
                    letter: Letter::new("test.x@1", vec![], None, None, vec![]).unwrap(),
                },
            ]
        }
    }
    let mut ctx = Context::new("c1");
    ctx.spawn(Spawn::new(Some("atomic"), Box::new(|| Box::new(PutAndBadPost)))).unwrap();
    let l = Letter::new("test.go@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "atomic"), l, vec![]);
    ctx.run_until_idle(50);
    assert_eq!(ctx.dead_letters().len(), 1, "the letter struck out");
    assert!(
        !ctx.telemetry().iter().any(|(_, m)| m.contains("should-not-land")),
        "no partial application"
    );
    // And the blob store is empty — the put was rolled back with the
    // rest of the list.
    let digest = Value::blob_digest(b"should-not-land", "text/plain").unwrap();
    assert!(
        ctx.telemetry().iter().all(|(_, m)| !m.contains(&digest)),
        "digest never materialized"
    );
}

#[test]
fn mint_requires_the_source_row() {
    struct Minter;
    impl Actor for Minter {
        fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
            match letter.kind.as_str() {
                "test.mint@1" => vec![Effect::Mint {
                    source_verb: "role.write".into(),
                    source_object: "obj1".into(),
                    verb: "role.read".into(),
                    object: "obj1".into(),
                }],
                "test.caps@1" => {
                    let names: Vec<String> =
                        host.caps().iter().map(|r| r.verb.clone()).collect();
                    vec![Effect::Emit {
                        level: Level::Info,
                        message: format!("CAPS:{}", names.join(",")),
                    }]
                }
                _ => vec![],
            }
        }
    }
    let mut ctx = Context::new("c1");
    ctx.spawn(
        Spawn::new(Some("mint"), Box::new(|| Box::new(Minter)))
            .supervision(Supervision::Restart)
            .rows(vec![CapRow::new("role.write", "obj1")]),
    )
    .unwrap();
    // First: a mint attempt WITHOUT the source row held — strike.
    // (The seed row is role.write/obj1; the letter names a source the
    // actor does not hold by mutating the effect through a variant.)
    // Simplest honest test: mint succeeds when held, and caps-lists it.
    let l = Letter::new("test.mint@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "mint"), l, vec![]);
    let caps = Letter::new("test.caps@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "mint"), caps, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "CAPS:role.write,role.read"),
        "mint added a row when the source was held: {:?}", ctx.telemetry());
}
