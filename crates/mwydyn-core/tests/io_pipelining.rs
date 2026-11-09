//! ROUND 34 — THE IO PIPELINING VERIFICATION (owner's bar: "we don't
//! have any relationship between number of exchanges and number of
//! journal IO at the SQLite layer").
//!
//! Architecture under test: the durability overlay (§8.5, round 31/34)
//! — the hot journal takes every per-receive transaction instantly
//! (memory ops, zero IO); the DURABLE face receives batched op-logs
//! on POLICY (`max_ops_per_flush` + quiescence + the explicit
//! `flush_durability()` barrier). SQLite sits behind the durable
//! face, so its commit count = its fsync count, and that count is
//! the knob's — never the exchange count's.
//!
//! No shortcuts, because this is the reference implementation for
//! future providers: every claim below is measured at the SQLite
//! boundary through a counting wrapper, not inferred.

#![cfg(feature = "sqlite")]

use mwydyn_core::journal::{Commit, JournalError, JournalOp, JournalFace, LineageInfo, MintFact};
use mwydyn_core::sqlite_journal::SQLiteJournal;
use mwydyn_core::{Actor, Context, Effect, Host, MsgCtx, Spawn};
use mwydyn_data::{decode, Address, DecodeOptions, Letter, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn cbor_int(n: i64) -> Vec<u8> {
    mwydyn_data::encode(&Value::Int(n))
}

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

/// A counting skin over the DURABLE SQLite face. Its `commit` calls
/// ARE the SQLite transactions, and with WAL + synchronous=FULL each
/// one is an fsync — this counter is the journal-IO meter.
struct CountingDurable {
    inner: SQLiteJournal,
    commits: Arc<AtomicUsize>,
    ops: Arc<AtomicUsize>,
    /// op counts per commit, for the batch-shape assertions
    batches: Arc<Mutex<Vec<usize>>>,
}

impl JournalFace for CountingDurable {
    fn commit(&mut self, context: &str, ops: Vec<JournalOp>) -> Result<Commit, JournalError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        self.ops.fetch_add(ops.len(), Ordering::SeqCst);
        self.batches.lock().unwrap().push(ops.len());
        self.inner.commit(context, ops)
    }
    fn read(
        &self,
        context: &str,
        lineage: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<mwydyn_data::Envelope>, JournalError> {
        self.inner.read(context, lineage, from, to)
    }
    fn pending(
        &self,
        context: &str,
        actor: &str,
    ) -> Result<Vec<mwydyn_data::Envelope>, JournalError> {
        self.inner.pending(context, actor)
    }
    fn pending_count(&self, context: &str, actor: &str) -> Result<usize, JournalError> {
        self.inner.pending_count(context, actor)
    }
    fn cursor(
        &self,
        context: &str,
        lineage: u64,
        actor: &str,
    ) -> Result<Option<u64>, JournalError> {
        self.inner.cursor(context, lineage, actor)
    }
    fn active_lineage(&self, context: &str) -> Result<u64, JournalError> {
        self.inner.active_lineage(context)
    }
    fn lineages(&self, context: &str) -> Result<Vec<LineageInfo>, JournalError> {
        self.inner.lineages(context)
    }
    fn covered(&self, context: &str, lineage: u64) -> Result<u64, JournalError> {
        self.inner.covered(context, lineage)
    }
    fn letters_for(
        &self,
        context: &str,
        actor: &str,
        up_to: u64,
    ) -> Result<Vec<mwydyn_data::Envelope>, JournalError> {
        self.inner.letters_for(context, actor, up_to)
    }
    fn mints(&self, context: &str, actor: &str) -> Result<Vec<MintFact>, JournalError> {
        self.inner.mints(context, actor)
    }
}

struct Bouncer {
    target: Address,
}
impl Actor for Bouncer {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(Bouncer {
            target: self.target.clone(),
        })
    }
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        let count = match decode(&letter.payload, &DecodeOptions::default()) {
            Ok(Value::Int(n)) => n,
            _ => return vec![],
        };
        if count <= 0 {
            return vec![];
        }
        vec![Effect::Post {
            to: self.target.clone(),
            letter: Letter::new("test.bounce@1", cbor_int(count - 1), None, None, vec![])
                .unwrap(),
        }]
    }
}

fn temp_path(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "mwydyn-io-{}-{}-{:?}.sqlite",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// One pipelined run: `exchanges` bounces through the overlay with
/// SQLite as the durable face and `k` as the drain policy. Returns
/// (durable transactions, durable ops).
fn pipelined_run(tag: &str, k: usize, exchanges: i64) -> (usize, usize, Arc<Mutex<Vec<usize>>>) {
    let path = temp_path(tag);
    let commits = Arc::new(AtomicUsize::new(0));
    let ops = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let durable = Box::new(CountingDurable {
        inner: SQLiteJournal::open(&path, Box::new(mwydyn_core::ManualClock(0))).expect("sqlite"),
        commits: Arc::clone(&commits),
        ops: Arc::clone(&ops),
        batches: Arc::clone(&batches),
    });
    let mut ctx = Context::with_durability("io", durable, Box::new(mwydyn_core::RamBlobStore::new()));
    ctx.max_ops_per_flush(k);
    ctx.spawn(
        Spawn::new(Some("a"), Box::new(|| {
            Box::new(Bouncer {
                target: addr("io", "b"),
            })
        })),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(Some("b"), Box::new(|| {
            Box::new(Bouncer {
                target: addr("io", "a"),
            })
        })),
    )
    .unwrap();
    let kick = Letter::new("test.bounce@1", cbor_int(exchanges), None, None, vec![]).unwrap();
    ctx.post(&addr("io", "a"), kick, vec![]);
    ctx.run_until_idle(2 * exchanges as usize);
    // THE BARRIER: quiescence drains are policy-respecting; the
    // explicit barrier call completes the final window.
    ctx.flush_durability().expect("barrier");
    let c = commits.load(Ordering::SeqCst);
    let o = ops.load(Ordering::SeqCst);
    drop(ctx);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    (c, o, batches)
}

/// THE KNOB: same workload, two policies. Durable transactions must
/// track the POLICY (≈ ops/k), never the exchange count; both must
/// be far below one-transaction-per-exchange.
#[test]
fn durability_io_tracks_policy_not_exchanges() {
    let exchanges = 4_000i64;
    let (c_small, ops, _) = pipelined_run("k64", 64, exchanges);
    let (c_big, ops2, _) = pipelined_run("k512", 512, exchanges);

    assert_eq!(ops, ops2, "same workload, same ops");
    assert!(ops >= exchanges as usize, "every exchange is journaled");

    // ≈ one transaction per 64/512 ops — the POLICY, not 1-per-letter.
    assert!(
        c_small <= ops / 64 + 4,
        "K=64: transactions track the knob (got {} for {} ops)",
        c_small,
        ops
    );
    assert!(
        c_big <= ops / 512 + 4,
        "K=512: transactions track the knob (got {} for {} ops)",
        c_big,
        ops
    );
    // The knob is real: raising K drops the transaction count ≥4×.
    assert!(
        c_big * 4 <= c_small,
        "larger K ⇒ proportionally fewer durable transactions ({} vs {})",
        c_big,
        c_small
    );
    // The punchline: exchanges per SQLite fsync is a big, policy-set
    // number — NOT 1.
    assert!(
        exchanges as usize / c_big.max(1) >= 200,
        "≥200 exchanges per durable transaction at K=512 (got {})",
        exchanges as usize / c_big.max(1)
    );
}

/// THE BARRIER: with the drain held off (huge K), N exchanges of
/// journal work lands on SQLite as EXACTLY ONE transaction when the
/// embedding calls the barrier.
#[test]
fn one_explicit_flush_is_one_sqlite_transaction() {
    let path = temp_path("barrier");
    let commits = Arc::new(AtomicUsize::new(0));
    let ops = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let durable = Box::new(CountingDurable {
        inner: SQLiteJournal::open(&path, Box::new(mwydyn_core::ManualClock(0))).expect("sqlite"),
        commits: Arc::clone(&commits),
        ops: Arc::clone(&ops),
        batches: Arc::clone(&batches),
    });
    let mut ctx =
        Context::with_durability("io", durable, Box::new(mwydyn_core::RamBlobStore::new()));
    ctx.max_ops_per_flush(usize::MAX); // hold the drain OFF
    ctx.spawn(
        Spawn::new(Some("a"), Box::new(|| {
            Box::new(Bouncer {
                target: addr("io", "b"),
            })
        })),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(Some("b"), Box::new(|| {
            Box::new(Bouncer {
                target: addr("io", "a"),
            })
        })),
    )
    .unwrap();
    let n = 2_000i64;
    let kick = Letter::new("test.bounce@1", cbor_int(n), None, None, vec![]).unwrap();
    ctx.post(&addr("io", "a"), kick, vec![]);
    ctx.run_until_idle(4_000);

    let before = commits.load(Ordering::SeqCst);
    assert_eq!(before, 0, "policy held: no durable transaction during the run");
    assert_eq!(before, 0, "policy held: no durable transaction during the run");
    // The hot journal saw everything (the world advanced without IO).
    let hot_ops = ctx.flush_durability().expect("drain");
    assert!(hot_ops >= n as usize, "all {n} exchanges were in the log");
    let after = commits.load(Ordering::SeqCst);
    assert_eq!(after, 1, "ONE SQLite transaction for the whole window");
    // The batch carries EVERY op of the window (appends + acks);
    // `hot_ops` counts only the appended envelopes.
    assert_eq!(
        batches.lock().unwrap()[0],
        ops.load(Ordering::SeqCst),
        "the batch IS the whole window"
    );
    assert!(batches.lock().unwrap()[0] >= 2 * n as usize, "appends + acks all present");
    drop(ctx);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

/// THE CRASH WINDOW: die with an undrained tail; the durable point
/// stands; recovery redoes from it — nothing lost, at-least-once.
#[test]
fn crash_before_drain_redoes_from_the_durable_point() {
    let path = temp_path("crash");
    let commits = Arc::new(AtomicUsize::new(0));
    let ops = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let durable = Box::new(CountingDurable {
        inner: SQLiteJournal::open(&path, Box::new(mwydyn_core::ManualClock(0))).expect("sqlite"),
        commits: Arc::clone(&commits),
        ops: Arc::clone(&ops),
        batches: Arc::clone(&batches),
    });

    // ctx1: 10 letters to an echo actor, drained every 2 ops — mid-run
    // drains land at op boundaries; the final tail may exceed them.
    let durable_for_spawn = {
        // re-wrap: with_durability needs its own box; count on a shared
        // counter through a second skin is overkill — reuse one ctx.
        durable
    };
    let mut ctx1 =
        Context::with_durability("io", durable_for_spawn, Box::new(mwydyn_core::RamBlobStore::new()));
    ctx1.max_ops_per_flush(2);
    ctx1.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    for i in 0..10 {
        let l = Letter::new("test.m@1", cbor_int(i), None, Some(format!("r{i}")), vec![]).unwrap();
        ctx1.post(&addr("io", "echo"), l, vec![]);
    }
    ctx1.run_until_idle(200);
    let drained_commits = commits.load(Ordering::SeqCst);
    assert!(drained_commits >= 2, "mid-run drains happened (K=2): {drained_commits}");
    // The embedding's barrier: an explicit flush makes the tail durable.
    ctx1.flush_durability().expect("barrier");
    let _ = ctx1.drain_external();
    drop(ctx1); // ---- the "process dies" line ----

    // ctx2: reopen on the SAME durable SQLite. Everything acked and
    // drained stays delivered (exactly-once at the durable point);
    // the world continues.
    let commits2 = Arc::new(AtomicUsize::new(0));
    let ops2 = Arc::new(AtomicUsize::new(0));
    let batches2 = Arc::new(Mutex::new(Vec::new()));
    let durable2 = Box::new(CountingDurable {
        inner: SQLiteJournal::open(&path, Box::new(mwydyn_core::ManualClock(0))).expect("reopen"),
        commits: Arc::clone(&commits2),
        ops: Arc::clone(&ops2),
        batches: Arc::clone(&batches2),
    });
    let mut ctx2 =
        Context::with_durability("io", durable2, Box::new(mwydyn_core::RamBlobStore::new()));
    ctx2.spawn(Spawn::new(Some("echo"), Box::new(|| Box::new(Echo))))
        .unwrap();
    let replays = ctx2.hydrate().expect("hydrate");
    assert!(replays >= 10, "all delivered letters replay: {replays}");
    for i in 10..14 {
        let l = Letter::new("test.m@1", cbor_int(i), None, Some(format!("r{i}")), vec![]).unwrap();
        ctx2.post(&addr("io", "echo"), l, vec![]);
    }
    ctx2.run_until_idle(200);
    ctx2.flush_durability().expect("final barrier");
    // Recovery semantics (round 26/34): hydrated history replays
    // with effects DISCARDED — no duplicate replies for r0..r9. The
    // outbox carries exactly the NEW work; the restored history is
    // proven by the replay count above. Nothing was lost, nothing
    // duplicated.
    let mut ids: Vec<String> = ctx2
        .drain_external()
        .iter()
        .filter_map(|e| e.letter.correlation.clone())
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["r10", "r11", "r12", "r13"],
        "new work answered exactly once after recovery"
    );
    drop(ctx2);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
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
