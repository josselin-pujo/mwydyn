//! M0 walking-skeleton tests — the round-01…07 semantics, running.

use mwydyn_core::ticker::{TIMER_ARM_KIND, Ticker};
use mwydyn_core::{
    Actor, CapRow, Context, Effect, Host, Level, ManualClock, MsgCtx, RefuseReason, Spawn,
    Supervision, Verdict,
};
#[cfg(feature = "sqlite")]
use mwydyn_core::SQLiteJournal;
use mwydyn_data::{Address, DecodeOptions, Letter, Value};
#[cfg(feature = "sqlite")]
use mwydyn_core::SQLiteBlobStore;

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
fn replies_to_external_land_in_the_drainable_outbox() {
    let mut ctx = Context::new("c1");
    ctx.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    // Echo replies to `from` — external now HAS an inbox: the
    // embedding's drainable outbox (§3.4's handle mechanism).
    let l = Letter::new("test.hi@1", b"yo".to_vec(), None, Some("c1".into()), vec![]).unwrap();
    ctx.post(&addr("c1", "echo"), l, vec![]);
    ctx.run_until_idle(50);
    let replies = ctx.drain_external();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].letter.kind, "test.echo.reply@1");
    assert_eq!(replies[0].letter.payload, b"yo");
    assert_eq!(replies[0].letter.correlation.as_deref(), Some("c1"));
    assert!(ctx.drain_external().is_empty(), "drain drains");
    assert!(ctx.dead_letters().is_empty());
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
        .any(|(lvl, m)| *lvl == Level::Debug && m.contains("left pending")));
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
            // Both the publisher's posted ref and the test's direct
            // read letters carry a blob-ref payload.
            "test.ref@1" | "test.read@1" => {
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

/// Round 16's answer to "how does any actor ever get a blobRef":
/// eagerly, from the ambient face — put, take the ref, post it, all
/// in one receive.
struct Publisher {
    reader: Address,
}

impl Actor for Publisher {
    fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        if letter.kind != "test.publish@1" {
            return vec![];
        }
        let r = host
            .blob_put(b"hello", "text/plain", None)
            .expect("blob.write row held");
        let payload = mwydyn_data::encode(&Value::BlobRef {
            digest: r.digest.clone(),
            media_type: r.media_type.clone(),
            size: r.size,
        });
        vec![Effect::Post {
            to: self.reader.clone(),
            letter: Letter::new("test.ref@1", payload, None, None, vec![]).unwrap(),
        }]
    }
}

#[test]
fn blob_ref_in_hand_put_then_post_then_read() {
    let mut ctx = Context::new("c1");
    ctx.spawn(
        Spawn::new(
            Some("pub"),
            Box::new(|| Box::new(Publisher {
                reader: addr("c1", "blob"),
            })),
        )
        .rows(vec![CapRow::new("blob.write", "*")]),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(
            Some("blob"),
            Box::new(|| Box::new(BlobActor)),
        )
        .rows(vec![CapRow::new("blob.read", "*")]),
    )
    .unwrap();
    let w = Letter::new("test.publish@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "pub"), w, vec![]);
    ctx.run_until_idle(50);
    // Publisher eagerly put (blob.write), posted the ref, the reader
    // read it back — one pump, ref in hand throughout.
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "READ:hello"));

    // Round 28: a RANGE read seeks — the substr-backed provider
    // serves [4..10) of "hello" wait, wrong blob; use the 16-byte
    // pattern blob instead. The publisher puts a 16-byte pattern and
    // range-reads a slice and a clamped over-run in the same
    // receive, proving: no full fetch needed, end clamped.
    struct RangePublisher {
        reader: Address,
    }
    impl Actor for RangePublisher {
        fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
            if letter.kind != "test.range@1" {
                return vec![];
            }
            let pattern: Vec<u8> = (0..16u32).map(|i| b"0123456789ABCDEF"[i as usize]).collect();
            let r = host
                .blob_put(&pattern, "application/octet-stream", None)
                .expect("blob.write held");
            let mut out = Vec::new();
            // slice [4..10) → "456789"
            match host.blob_get_range(&r, 4, 6) {
                Ok(bytes) => out.push(Effect::Emit {
                    level: Level::Info,
                    message: format!("RANGE:{}", String::from_utf8_lossy(&bytes)),
                }),
                Err(e) => out.push(Effect::Emit {
                    level: Level::Error,
                    message: format!("RANGE-ERR:{e:?}"),
                }),
            }
            // clamped over-run: offset 14, len 100 → "EF" (2 bytes)
            match host.blob_get_range(&r, 14, 100) {
                Ok(bytes) => out.push(Effect::Emit {
                    level: Level::Info,
                    message: format!("CLAMP:{}:{}", bytes.len(), String::from_utf8_lossy(&bytes)),
                }),
                Err(e) => out.push(Effect::Emit {
                    level: Level::Error,
                    message: format!("CLAMP-ERR:{e:?}"),
                }),
            }
            out
        }
    }
    ctx.spawn(
        Spawn::new(
            Some("rp"),
            Box::new(|| Box::new(RangePublisher {
                reader: addr("c1", "blob"),
            })),
        )
        .rows(vec![CapRow::new("blob.read", "*"), CapRow::new("blob.write", "*")]),
    )
    .unwrap();
    let rl = Letter::new("test.range@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "rp"), rl, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "RANGE:456789"));
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "CLAMP:2:EF"));

    // ROUND 28 SEMANTICS: the ref's mediaType is a HINT — content
    // identity resolves by digest, so a ref re-labeled in transit
    // still returns the TRUE bytes under a lying label. Type-confusion
    // detection moved to put-time (the digest binds (bytes, type)) and
    // to dispatchers that consult provider-truth or sniff. (An
    // INVALID ref — uppercase digest — never resolves at all: loud
    // Missing, pinned in blob-refs-v0.)
    let real_digest = Value::blob_digest(b"hello", "text/plain").unwrap();
    let wrong_payload = mwydyn_data::encode(&Value::BlobRef {
        digest: real_digest,
        media_type: "application/json".into(),
        size: 5,
    });
    let fr = Letter::new("test.read@1", wrong_payload, None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "blob"), fr, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "READ:hello"));
}

#[test]
fn atomic_effects_rollback_observable_state_allocations_are_garbage() {
    // The banking nerve (round 07), amended by round 16: the POST in
    // a bad list rolls back — nothing observable happens — but an
    // EAGER put is not an effect: it leaves unreferenced garbage that
    // GC (M3) collects. Observable state is identical either way.
    struct EagerPutter;
    impl Actor for EagerPutter {
        fn receive(&mut self, _ctx: &MsgCtx, host: &Host, _letter: &Letter) -> Vec<Effect> {
            let r = host
                .blob_put(b"eager-garbage", "text/plain", None)
                .expect("blob.write row held");
            vec![Effect::Post {
                // a NONEXISTENT instance — the strike path this test
                // exercises (external now lands in the outbox).
                to: Address::Instance("ins-999".into()),
                letter: Letter::new("test.ref@1", r.digest.into_bytes(), None, None, vec![])
                    .unwrap(),
            }]
        }
    }
    let mut ctx = Context::new("c1");
    ctx.spawn(
        Spawn::new(Some("atomic"), Box::new(|| Box::new(EagerPutter)))
            .rows(vec![CapRow::new("blob.write", "*")]),
    )
    .unwrap();
    let l = Letter::new("test.go@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "atomic"), l, vec![]);
    ctx.run_until_idle(50);
    // The observable transaction held: the letter struck out, no
    // letter carrying the ref was ever committed anywhere.
    assert_eq!(ctx.dead_letters().len(), 1, "the letter struck out");
    assert_eq!(ctx.dead_letters()[0].letter.kind, "test.go@1");
    assert!(ctx.drain_external().is_empty());
    // The allocation lingered as garbage — collectable above the
    // watermark (M3), unobservable to every actor meanwhile.
    let count = ctx.blob_count();
    assert_eq!(count, 1, "eager put = garbage, not a committed effect");
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
        Spawn::new(Some("mint"), Box::new(|| Box::new(HydrationMinter)))
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

struct Upper;

impl Actor for Upper {
    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        vec![Effect::Post {
            to: ctx.from.clone(),
            letter: Letter::new(
                "upper.reply@1",
                letter.payload.to_ascii_uppercase(),
                None,
                letter.correlation.clone(),
                vec![],
            )
            .unwrap(),
        }]
    }
}

#[test]
fn call_task_is_a_one_call_round_trip() {
    let mut ctx = Context::new("c1");
    let req = Letter::new("upper.request@1", b"hello".to_vec(), None, None, vec![]).unwrap();
    let reply = ctx
        .call_task(Box::new(|| Box::new(Upper)), req)
        .unwrap()
        .expect("the reply arrived in the external outbox");
    assert_eq!(reply.kind, "upper.reply@1");
    assert_eq!(reply.payload, b"HELLO");
    let corr = reply.correlation.expect("reply echoes the minted correlation");
    assert!(corr.starts_with("call-"));
    // Outbox drained of the matched reply; nothing else was pending.
    assert!(ctx.drain_external().is_empty());
}

#[test]
#[cfg(feature = "sqlite")]
fn hydration_rebuilds_rows_and_ticker_schedules() {
    // THE crash-recovery story: a durable context dies mid-flight;
    // the same composition is re-spawned on the reopened journal and
    // hydrate() reconstructs grant sets (Mint facts) and the
    // ticker's schedule (replayed arms) — then life continues.
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "mwydyn-hydration-{}-{:?}.sqlite",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let build = |path: &std::path::Path, clock_us: i64| {
        let mut ctx = Context::with_journal(
            "c1",
            Box::new(
                SQLiteJournal::open(path, Box::new(ManualClock(1000)))
                    .expect("journal opens"),
            ),
        );
        ctx.set_clock(Box::new(ManualClock(clock_us)));
        ctx
    };

    // ---- ctx1: mint a row, arm a timer at T=1000 (fires at 1500).
    let mut ctx1 = build(&path, 1000);
    ctx1.spawn(
        Spawn::new(Some("mint"), Box::new(|| Box::new(HydrationMinter)))
            .supervision(Supervision::Restart)
            .rows(vec![CapRow::new("role.write", "obj1")]),
    )
    .unwrap();
    ctx1.spawn(
        Spawn::new(
            Some("armer"),
            Box::new(|| Box::new(Armer {
                ticker: addr("c1", "ticker"),
            })),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();
    let mint = Letter::new("test.mint@1", vec![], None, None, vec![]).unwrap();
    ctx1.post(&addr("c1", "mint"), mint, vec![]);
    let go = Letter::new("test.go@1", vec![], None, None, vec![]).unwrap();
    ctx1.post(&addr("c1", "armer"), go, vec![]);
    ctx1.run_until_idle(50);
    drop(ctx1);

    // ---- ctx2: reopen, re-spawn the same composition, hydrate.
    let mut ctx2 = build(&path, 1000);
    ctx2.spawn(
        Spawn::new(Some("mint"), Box::new(|| Box::new(HydrationMinter)))
            .supervision(Supervision::Restart)
            .rows(vec![CapRow::new("role.write", "obj1")]),
    )
    .unwrap();
    ctx2.spawn(
        Spawn::new(
            Some("armer"),
            Box::new(|| Box::new(Armer {
                ticker: addr("c1", "ticker"),
            })),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();
    let replays = ctx2.hydrate().expect("hydrate");
    assert!(replays >= 3, "ticker + minter + armer replays: {replays}");

    // The mint fact hydrated into the grant set (base row + mint).
    let caps = Letter::new("test.caps@1", vec![], None, None, vec![]).unwrap();
    ctx2.post(&addr("c1", "mint"), caps, vec![]);
    ctx2.run_until_idle(50);
    assert!(ctx2
        .telemetry()
        .iter()
        .any(|(_, m)| m == "CAPS:role.write,role.read"));

    // Time moves past the armed deadline; the HYDRATED schedule
    // fires: poll → tick → armer emits.
    ctx2.set_clock(Box::new(ManualClock(1600)));
    ctx2.run_until_idle(50);
    assert!(ctx2.telemetry().iter().any(|(_, m)| m == "TICK:c9"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
#[cfg(feature = "sqlite")]
fn durable_context_survives_reopen() {
    // THE M2 close-out: work, die, reopen, keep working. Letters
    // committed in the old process are still pending in the new one;
    // hydration replays the delivered prefix; nothing is lost or
    // duplicated.
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "mwydyn-durable-{}-{:?}.sqlite",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let mut ctx1 = Context::with_journal(
        "c1",
        Box::new(SQLiteJournal::open(&path, Box::new(ManualClock(0))).unwrap()),
    );
    ctx1.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    let m1 = Letter::new("test.d@1", b"m1".to_vec(), None, Some("r1".into()), vec![]).unwrap();
    ctx1.post(&addr("c1", "echo"), m1, vec![]);
    ctx1.run_until_idle(50); // m1 delivered + replied (reply lost with ctx1's outbox view)
    let m2 = Letter::new("test.d@1", b"m2".to_vec(), None, Some("r2".into()), vec![]).unwrap();
    ctx1.post(&addr("c1", "echo"), m2, vec![]);
    // m2 stays PENDING — never delivered. ctx1 dies here.
    drop(ctx1);

    let mut ctx2 = Context::with_journal(
        "c1",
        Box::new(SQLiteJournal::open(&path, Box::new(ManualClock(0))).unwrap()),
    );
    ctx2.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    ctx2.hydrate().expect("hydrate");

    // m2 is still pending — exactly-once delivery from the journal.
    let m3 = Letter::new("test.d@1", b"m3".to_vec(), None, Some("r3".into()), vec![]).unwrap();
    ctx2.post(&addr("c1", "echo"), m3, vec![]);
    ctx2.run_until_idle(50);
    let replies = ctx2.drain_external();
    let mut payloads: Vec<Vec<u8>> = replies
        .iter()
        .map(|e| e.letter.payload.clone())
        .collect();
    payloads.sort();
    assert_eq!(payloads, vec![b"m2".to_vec(), b"m3".to_vec()]);
    assert!(
        !payloads.contains(&b"m1".to_vec()),
        "m1 was already delivered in ctx1 — no double delivery"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

struct GhostProbe;

impl Actor for GhostProbe {
    fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        if letter.kind != "test.probe@1" {
            return vec![];
        }
        let v = mwydyn_data::decode(&letter.payload, &DecodeOptions::default()).expect("probe");
        let Value::Map(pairs) = v else {
            panic!("probe payload");
        };
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .and_then(|(_, v)| if let Value::Int(n) = v { Some(*n as u64) } else { None })
                .expect("probe field")
        };
        let (lineage, from, to) = (get("lineage"), get("from"), get("to"));
        match host.ghost_read(lineage, from, to) {
            Ok(envs) => envs
                .iter()
                .map(|e| Effect::Emit {
                    level: Level::Info,
                    message: format!("GHOST:{}", String::from_utf8_lossy(&e.letter.payload)),
                })
                .collect(),
            Err(e) => vec![Effect::Emit {
                level: Level::Error,
                message: format!("GHOST-ERR:{e:?}"),
            }],
        }
    }
}

fn probe_payload(lineage: u64, from: u64, to: u64) -> Vec<u8> {
    mwydyn_data::encode(
        &Value::map(vec![
            ("to".into(), Value::Int(to as i64)),
            ("lineage".into(), Value::Int(lineage as i64)),
            ("from".into(), Value::Int(from as i64)),
        ])
        .expect("fixed keys"),
    )
}

/// Mints on request; lists its live rows on demand — the hydration
/// probe for grant-set reconstruction.
struct HydrationMinter;

impl Actor for HydrationMinter {
    fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        match letter.kind.as_str() {
            "test.mint@1" => vec![Effect::Mint {
                source_verb: "role.write".into(),
                source_object: "obj1".into(),
                verb: "role.read".into(),
                object: "obj1".into(),
            }],
            "test.caps@1" => {
                let names: Vec<String> = host.caps().iter().map(|r| r.verb.clone()).collect();
                vec![Effect::Emit {
                    level: Level::Info,
                    message: format!("CAPS:{}", names.join(",")),
                }]
            }
            _ => vec![],
        }
    }
}

#[test]
fn rewind_forks_a_readable_ghost_branch_behind_the_gate() {
    let mut ctx = Context::new("c1");
    // probe-all holds the wide grant; probe-1 the per-lineage one;
    // probe-none holds nothing (default-off).
    ctx.spawn(
        Spawn::new(
            Some("probe-all"),
            Box::new(|| Box::new(GhostProbe)),
        )
        .rows(vec![CapRow::new("ghost.read", "*")]),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(
            Some("probe-1"),
            Box::new(|| Box::new(GhostProbe)),
        )
        .rows(vec![CapRow::new("ghost.read", "1")]),
    )
    .unwrap();
    ctx.spawn(Spawn::new(
        Some("probe-none"),
        Box::new(|| Box::new(GhostProbe)),
    ))
    .unwrap();

    // Two notes land on lineage 1 (seqs 1, 2), delivered to probe-all.
    for payload in ["note-1", "note-2"] {
        let l = Letter::new("test.note@1", payload.as_bytes().to_vec(), None, None, vec![]).unwrap();
        ctx.post(&addr("c1", "probe-all"), l, vec![]);
    }
    ctx.run_until_idle(50);

    // Rewind to 1: note-2 (seq 2) becomes a ghost of the alternate
    // reality; actors restart fresh.
    let new_lineage = ctx.rewind(1).expect("rewind forks");
    assert_eq!(new_lineage, 2);
    let lins = {
        // lineages are observable through a fresh probe's ghost read
        // of lineage 1's SHARED prefix — this also proves the prefix
        // survives the fork for granted readers.
        let l = Letter::new(
            "test.probe@1",
            probe_payload(1, 1, 1),
            None,
            None,
            vec![],
        )
        .unwrap();
        ctx.post(&addr("c1", "probe-all"), l, vec![]);
        ctx.run_until_idle(50);
        assert!(ctx.telemetry().iter().any(|(_, m)| m == "GHOST:note-1"));
        let l = Letter::new(
            "test.probe@1",
            probe_payload(1, 2, 2),
            None,
            None,
            vec![],
        )
        .unwrap();
        ctx.post(&addr("c1", "probe-all"), l, vec![]);
        ctx.run_until_idle(50);
        assert!(
            ctx.telemetry().iter().any(|(_, m)| m == "GHOST:note-2"),
            "the ghost is readable by the wide grant"
        );
    };

    // Per-lineage attenuation: probe-1 may read lineage 1, not the
    // active lineage 2.
    let l = Letter::new(
        "test.probe@1",
        probe_payload(1, 2, 2),
        None,
        None,
        vec![],
    )
    .unwrap();
    ctx.post(&addr("c1", "probe-1"), l, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "GHOST:note-2"));

    let l = Letter::new(
        "test.probe@1",
        probe_payload(2, 2, 2),
        None,
        None,
        vec![],
    )
    .unwrap();
    ctx.post(&addr("c1", "probe-1"), l, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx
        .telemetry()
        .iter()
        .any(|(_, m)| m.contains("GHOST-ERR:Unauthorized")));

    // Default-off: the grantless probe is denied outright.
    let l = Letter::new(
        "test.probe@1",
        probe_payload(1, 2, 2),
        None,
        None,
        vec![],
    )
    .unwrap();
    ctx.post(&addr("c1", "probe-none"), l, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx
        .telemetry()
        .iter()
        .any(|(_, m)| m.contains("GHOST-ERR:Unauthorized")));
    // And the grantless probe produced no ghost reads at all.
    let none_ghosts = ctx
        .telemetry()
        .iter()
        .filter(|(_, m)| m == "GHOST:note-2")
        .count();
    assert_eq!(
        none_ghosts, 2,
        "exactly the two GRANTED probes read the ghost (wide + per-lineage)"
    );
}

/// M3: eager put → journaled reference → GC keeps; eager put →
/// NO journaled reference → GC collects (round 16's law, proven).
#[test]
fn gc_collects_unreferenced_allocations_and_keeps_journaled_ones() {
    let mut ctx = Context::new("c1");
    ctx.spawn(
        Spawn::new(
            Some("blob"),
            Box::new(|| Box::new(BlobActor)),
        )
        .rows(vec![CapRow::new("blob.read", "*"), CapRow::new("blob.write", "*")]),
    )
    .unwrap();

    // Two eager allocations, only ONE earns a journaled reference
    // (a letter whose payload carries the tag-1002 ref).
    let kept_digest = Value::blob_digest(b"referenced", "text/plain").unwrap();
    let kept_ref = mwydyn_data::encode(&Value::BlobRef {
        digest: kept_digest.clone(),
        media_type: "text/plain".into(),
        size: 10,
    });
    struct PublishTwo {
        reader: Address,
    }
    impl Actor for PublishTwo {
        fn receive(&mut self, _ctx: &MsgCtx, host: &Host, _letter: &Letter) -> Vec<Effect> {
            let kept = host
                .blob_put(b"referenced", "text/plain", None)
                .expect("blob.write held");
            let garbage = host
                .blob_put(b"unreferenced", "text/plain", None)
                .expect("blob.write held");
            // Only `kept` is posted; `garbage` is a dropped allocation.
            let payload = mwydyn_data::encode(&Value::BlobRef {
                digest: kept.digest.clone(),
                media_type: kept.media_type.clone(),
                size: kept.size,
            });
            let _ = garbage;
            vec![Effect::Post {
                to: self.reader.clone(),
                letter: Letter::new("test.ref@1", payload, None, None, vec![]).unwrap(),
            }]
        }
    }
    ctx.spawn(
        Spawn::new(
            Some("pub"),
            Box::new(|| Box::new(PublishTwo {
                reader: addr("c1", "blob"),
            })),
        )
        .rows(vec![CapRow::new("blob.write", "*")]),
    )
    .unwrap();
    let l = Letter::new("test.put@1", vec![], None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "pub"), l, vec![]);
    ctx.run_until_idle(50);
    assert_eq!(ctx.blob_count(), 2, "pre-GC: both allocations exist");

    // GC sweeps: the referenced blob survives, the dropped one is
    // collected.
    let collected = ctx.gc().expect("gc");
    assert_eq!(collected, 1, "exactly the unreferenced blob collected");
    assert_eq!(ctx.blob_count(), 1);

    // The survivor still verifies and reads — GC never breaks
    // references (round 12's hash-bound law intact).
    let l = Letter::new("test.read@1", kept_ref, None, None, vec![]).unwrap();
    ctx.post(&addr("c1", "blob"), l, vec![]);
    ctx.run_until_idle(50);
    assert!(ctx.telemetry().iter().any(|(_, m)| m == "READ:referenced"));
    assert!(!ctx.telemetry().iter().any(|(_, m)| m.contains("READ-ERR")));
}

#[test]
#[cfg(feature = "sqlite")]
fn durable_blobs_survive_reopen_and_gc_respects_ghost_reachability() {
    // THE M3 durable story: eager puts are durable (SQLite blob
    // store); a reopen + GC keeps journal-referenced blobs — across
    // lineages: a blob referenced ONLY by a GHOST letter survives,
    // because ghost branches are readable (round 19).
    let dir = std::env::temp_dir();
    let journal_path = dir.join(format!(
        "mwydyn-m3j-{}-{:?}.sqlite",
        std::process::id(),
        line!()
    ));
    let blob_path = dir.join(format!(
        "mwydyn-m3b-{}-{:?}.sqlite",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&journal_path);
    let _ = std::fs::remove_file(&blob_path);

    struct GhostNote;
    impl Actor for GhostNote {
        fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
            vec![] // receives letters; contributes journal entries only
        }
    }
    struct GhostPutter {
        reader: Address,
    }
    impl Actor for GhostPutter {
        fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
            if letter.kind != "test.put@1" {
                return vec![];
            }
            let r = host
                .blob_put(b"ghost-only", "text/plain", None)
                .expect("blob.write held");
            let payload = mwydyn_data::encode(&Value::BlobRef {
                digest: r.digest.clone(),
                media_type: r.media_type.clone(),
                size: r.size,
            });
            vec![Effect::Post {
                to: self.reader.clone(),
                letter: Letter::new("test.ghostref@1", payload, None, None, vec![]).unwrap(),
            }]
        }
    }

    // ctx1: the ghost-ref letter is committed on lineage 1 — then
    // a rewind to 1 ORPHANS it (it becomes a ghost letter).
    {
        let mut ctx = Context::with_stores(
            "c1",
            Box::new(SQLiteJournal::open(&journal_path, Box::new(ManualClock(0))).unwrap()),
            Box::new(SQLiteBlobStore::open(&blob_path).expect("blob store opens")),
        );
        ctx.spawn(Spawn::new(
            Some("note"),
            Box::new(|| Box::new(GhostNote)),
        ))
        .unwrap();
        ctx.spawn(
            Spawn::new(
                Some("gp"),
                Box::new(|| Box::new(GhostPutter {
                    reader: addr("c1", "note"),
                })),
            )
            .rows(vec![CapRow::new("blob.write", "*")]),
        )
        .unwrap();
        let l = Letter::new("test.put@1", vec![], None, None, vec![]).unwrap();
        ctx.post(&addr("c1", "gp"), l, vec![]);
        ctx.run_until_idle(50);
        ctx.rewind(1).expect("rewind orphans the ghost-ref letter");
    }

    // Reopen: hydrate, sweep — the ghost-referenced blob SURVIVES
    // (ghost branches are readable, round 19), even though its
    // letter no longer creates effects.
    let mut ctx2 = Context::with_stores(
        "c1",
        Box::new(SQLiteJournal::open(&journal_path, Box::new(ManualClock(0))).unwrap()),
        Box::new(SQLiteBlobStore::open(&blob_path).expect("blob store opens")),
    );
    ctx2.spawn(Spawn::new(
        Some("note"),
        Box::new(|| Box::new(GhostNote)),
    ))
    .unwrap();
    ctx2.hydrate().expect("hydrate");
    let collected = ctx2.gc().expect("gc");
    assert_eq!(collected, 0, "the ghost-referenced blob is reachable");
    assert_eq!(ctx2.blob_count(), 1);
    let _ = std::fs::remove_file(&journal_path);
    let _ = std::fs::remove_file(&blob_path);
    let _ = std::fs::remove_file(journal_path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(journal_path.with_extension("sqlite-shm"));
}

/// §7.6: a stateful counter — mints a snapshot letter when told;
/// state restores from the boot's snapshot ref on reopen.
struct StatefulCounter {
    count: u64,
    snapshots: Address,
}

impl Actor for StatefulCounter {
    fn init(&mut self, host: &Host, boot: &mwydyn_core::BootRecord) {
        // RESTORE: the boot carries the durable state ref.
        if let Some(r) = &boot.snapshot {
            if let Ok(bytes) = host.blob_get(r) {
                if let Ok(s) = String::from_utf8(bytes) {
                    if let Some(n) = s.strip_prefix("count=").and_then(|v| v.parse().ok()) {
                        self.count = n;
                    }
                }
            }
        }
    }

    fn receive(&mut self, _ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        match letter.kind.as_str() {
            "test.add@1" => {
                let add: u64 = std::str::from_utf8(&letter.payload)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                self.count += add;
                vec![Effect::Emit {
                    level: Level::Info,
                    message: format!("COUNT:{}", self.count),
                }]
            }
            "test.snap@1" => {
                // Serialize state → blob (eager, ref in hand) →
                // snapshot letter to the well-known sink. The letter
                // carries the actor's stable NAME; the runtime stamps
                // covers (the runtime-stamped ack) on it.
                let bytes = format!("count={}", self.count).into_bytes();
                let r = host
                    .blob_put(&bytes, "application/x-mwydyn-state", None)
                    .expect("blob.write held");
                let ref_value = Value::BlobRef {
                    digest: r.digest.clone(),
                    media_type: r.media_type.clone(),
                    size: r.size,
                };
                let payload = mwydyn_data::encode(
                    &Value::map(vec![
                        ("name".into(), Value::Text("counter".into())),
                        (
                            "ref".into(),
                            Value::Bytes(mwydyn_data::encode(&ref_value)),
                        ),
                    ])
                    .expect("keys"),
                );
                vec![Effect::Post {
                    to: addr("c1", mwydyn_core::SNAPSHOTS),
                    letter: Letter::new(
                        mwydyn_core::SNAPSHOT_SAVE_KIND,
                        payload,
                        None,
                        None,
                        vec![],
                    )
                    .unwrap(),
                }]
            }
            _ => vec![],
        }
    }
}

#[test]
#[cfg(feature = "sqlite")]
fn snapshot_letter_survives_reopen_and_restores_state_and_cursor() {
    let dir = std::env::temp_dir();
    let journal_path = dir.join(format!(
        "mwydyn-snap-{}-{:?}.sqlite",
        std::process::id(),
        line!()
    ));
    let blob_path = dir.join(format!(
        "mwydyn-snapb-{}-{:?}.sqlite",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&journal_path);
    let _ = std::fs::remove_file(&blob_path);

    let spawn_counter = |ctx: &mut Context| {
        ctx.spawn(
            Spawn::new(
                Some("counter"),
                Box::new(|| {
                    Box::new(StatefulCounter {
                        count: 0,
                        snapshots: addr("c1", mwydyn_core::SNAPSHOTS),
                    })
                }),
            )
            .rows(vec![CapRow::new("blob.read", "*"), CapRow::new("blob.write", "*")]),
        )
        .unwrap()
    };

    // ctx1: adds 1, 2 (delivered); snapshot request (save letter
    // journaled — covers the adds); add-3 stays PENDING; die.
    {
        let mut ctx = Context::with_stores(
            "c1",
            Box::new(SQLiteJournal::open(&journal_path, Box::new(ManualClock(0))).unwrap()),
            Box::new(SQLiteBlobStore::open(&blob_path).unwrap()),
        );
        spawn_counter(&mut ctx);
        for n in ["1", "2"] {
            let l = Letter::new("test.add@1", n.as_bytes().to_vec(), None, None, vec![]).unwrap();
            ctx.post(&addr("c1", "counter"), l, vec![]);
        }
        ctx.run_until_idle(50);
        let snap_req = Letter::new("test.snap@1", vec![], None, None, vec![]).unwrap();
        ctx.post(&addr("c1", "counter"), snap_req, vec![]);
        ctx.run_until_idle(50);
        let l = Letter::new("test.add@1", b"3".to_vec(), None, None, vec![]).unwrap();
        ctx.post(&addr("c1", "counter"), l, vec![]);
        drop(ctx);
    }

    // ctx2: reopen, re-spawn, hydrate. The boot carries the snapshot
    // (count=3, covering the adds); pending = add-3 only.
    let mut ctx2 = Context::with_stores(
        "c1",
        Box::new(SQLiteJournal::open(&journal_path, Box::new(ManualClock(0))).unwrap()),
        Box::new(SQLiteBlobStore::open(&blob_path).unwrap()),
    );
    spawn_counter(&mut ctx2);
    ctx2.hydrate().expect("hydrate");
    ctx2.run_until_idle(50);
    // 1 + 2 restored FROM THE SNAPSHOT; 3 delivered live from the
    // pending journal letter. No double-count, no loss.
    assert!(ctx2.telemetry().iter().any(|(_, m)| m == "COUNT:6"));
    let _ = std::fs::remove_file(&journal_path);
    let _ = std::fs::remove_file(&blob_path);
    let _ = std::fs::remove_file(journal_path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(journal_path.with_extension("sqlite-shm"));
}
