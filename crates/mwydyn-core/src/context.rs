//! The context (spec §3): the trust context is the clock domain is
//! the transaction boundary (round 08) — one wall, three names.
//!
//! M2: the mailbox IS a cursor over the journal (§3.1, literal).
//! Every delivery commits ONE journal transaction — the effect posts,
//! any mint facts, and the ack (log owns acks) — so the round-08
//! "one journal transaction per receive" is the mechanism, not a
//! metaphor. Rewind is a journaled fork (§8.3); abandoned tails are
//! ghosts, readable behind the `ghost.read` cap.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use mwydyn_data::{Address, DecodeOptions, Envelope, Letter, Value};

/// THE PAYLOAD WALL (round 33): a payload is canonical CBOR or it is
/// not admitted. Same gate at `post` (embedding) and in Phase 2 of
/// `deliver` (effects, native or WASM) — compose-parity at the gate.
fn payload_canonical(bytes: &[u8]) -> bool {
    mwydyn_data::decode(bytes, &DecodeOptions::default()).is_ok()
}

/// ROUND 35 — CQRS access helpers. Poison recovery is
/// `into_inner()`: actor panics unwinding through a read guard
/// poison the lock without breaking any journal invariant (reads
/// mutate nothing; writes are the committer's, panic-free by
/// construction), so the guard's guard must not wedge the world.
/// ROUND 35 — CQRS access: the guards are FIELD-scoped (they borrow
/// only the journal lock, never the whole context) so the committer
/// can hold `&mut entries` while a read guard is alive. Poison
/// recovery is `into_inner()`: actor panics unwinding through a
/// read guard poison the lock without breaking any journal
/// invariant (reads mutate nothing; writes are the committer's).
fn jread(
    journal: &RwLock<Box<dyn JournalFace>>,
) -> std::sync::RwLockReadGuard<'_, Box<dyn JournalFace>> {
    journal.read().unwrap_or_else(|poison| poison.into_inner())
}
fn jwrite(
    journal: &RwLock<Box<dyn JournalFace>>,
) -> std::sync::RwLockWriteGuard<'_, Box<dyn JournalFace>> {
    journal.write().unwrap_or_else(|poison| poison.into_inner())
}

/// ROUND 40 — the ambient surface pieces for one actor exchange:
/// owned handles (cheap clones), the actor's OWN rows.
impl Context {
    fn ambient_parts(&self, idx: usize) -> crate::actor::AmbientParts {
        crate::actor::AmbientParts {
            journal: std::sync::Arc::clone(&self.journal),
            blobs: std::sync::Arc::clone(&self.blobs),
            context: std::sync::Arc::from(self.name.as_str()),
            rows: self.entries[idx].rows.clone(),
        }
    }
}

// ------------------------------------------------------------------
// ROUND 34 — the worker pool's transition tier
//

/// One dispatched transition: the actor moves to the worker, runs
/// phase 1 against a read-only host face, and comes back with either
/// effects or a crash. `Actor: Send` (round 21's bound) is what
/// makes this legal.
struct WaveJob {
    seq: u64,
    actor: Box<dyn Actor>,
    letter: Letter,
    from: Address,
    rows: Vec<CapRow>,
    now: i64,
}

struct WaveOutcome {
    seq: u64,
    outcome: std::result::Result<Vec<Effect>, ()>,
    actor: Box<dyn Actor>,
    /// The worker-side pre-receive copy — the rollback state for
    /// refusals/crashes. Forking on the WORKER (not at staging)
    /// parallelizes the fork too, which is the expensive half for
    /// WASM guests (module re-instantiation).
    rollback: Box<dyn Actor>,
    /// The dispatch wall stamp (round 44, §3.6) — rides back so the
    /// committer's exchange records carry real durations without a
    /// second clock read or a map lookup.
    now: i64,
}


use crate::actor::{Actor, CapRow, Effect, Host, MsgCtx};
use crate::blob::{BlobFace, BlobRef, RamBlobStore};
use crate::journal::{InMemoryJournal, JournalError, JournalFace, JournalOp, JournalRead, MintFact, Pending};
use crate::ticker::Ticker;
use mwydyn_data::telemetry::{
    LifecycleEvent, Outcome, TelPayload, TelemetryKind, TelemetryRecord,
};

/// The embedding's instance id — posts FROM here; replies TO it land
/// in the drainable external outbox (§3.4: the embedding's handle).
pub const EXTERNAL: &str = "external";

/// ROUND 34 — default durability-drain policy: the overlay lands
/// the hot op log on the durable face as ONE transaction at 256 ops
/// (and at quiescence). The knob, not the exchange count, drives
/// durability IO.
pub const DEFAULT_MAX_OPS_PER_FLUSH: usize = 256;

/// Poison-letter strikes before the dead-letter path (§3.1's
/// policy-authorized cursor advance).
pub const MAX_ATTEMPTS: u64 = 3;

/// ROUND 44 — the telemetry ring's default capacity (§3.6): the
/// storm valve. Lossiness lives HERE, never at emission.
pub const DEFAULT_TELEMETRY_CAPACITY: usize = 4096;

/// The snapshots sink's well-known name (runtime-reserved, collocated
/// like the ticker — round 29).
pub const SNAPSHOTS: &str = "snapshots";
/// The snapshot-save kind (core schema, §7.6): payload = canonical
/// CBOR `{name, ref}` — the actor's stable NAME keys restoration.
pub const SNAPSHOT_SAVE_KIND: &str = "mwydyn.snapshot.save@1";

/// The actor's runtime-minted identity (§7.6 boot record, round 29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorIdentity {
    pub id: String,
    pub context: String,
    /// Named actors carry responsibility: persistence keys on the
    /// name (round 29: "legion but named"). Unnamed = ephemeral.
    pub name: Option<String>,
}

/// The boot record (§7.6, round 29) — the host-minted kick: identity,
/// parent handle, durable state ref, spawn config. The native mirror
/// of world v0.5's `boot` record.
#[derive(Debug, Clone)]
pub struct BootRecord {
    pub actor: ActorIdentity,
    /// The spawner (external for composition spawns). Required —
    /// a fresh actor's only handle for initiating.
    pub parent: Address,
    /// Durable state, if the journal holds a snapshot letter for
    /// this actor's NAME on the active lineage.
    pub snapshot: Option<BlobRef>,
    /// Spawn config — opaque canonical CBOR, layer-2 schemas
    /// (reserved-key discipline: `mwydyn.*` is runtime-reserved).
    pub config: Vec<u8>,
}

/// The snapshots sink (round 29): receives `snapshot.save` letters
/// and IGNORES them — the journal itself is the registry; the
/// runtime boot-scans it directly for restore.
#[derive(Clone)]
struct SnapshotsSink;

impl Actor for SnapshotsSink {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(self.clone())
    }
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, _letter: &Letter) -> Vec<Effect> {
        vec![] // the journal is the registry; delivery is bookkeeping
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supervision {
    /// Dev / live composition: a crash is terminal and loud (§3.2).
    FailFast,
    /// Production: re-instantiate via the factory and carry on.
    Restart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefuseReason {
    /// Bounds / credits / overload — the HOW is runtime policy and
    /// never disclosed further than this word (rounds 05–06).
    Busy,
    Unauthorized,
    UnknownTarget,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The letter was journaled — `seq` is its stamped identity
    /// (round 44, §3.4: the injection handle; external sources map
    /// their own ids against it, and a reply may cite it as parent).
    Accepted { seq: u64 },
    Refused(RefuseReason),
}

/// Supervision + shape for one spawn (§3.2: policy attaches at
/// spawn; escalation routes to the owner handle). The factory is
/// `Send` with the actor (round 21): restarts may be requested from
/// wherever the scheduler lives.
pub struct Spawn {
    pub name: Option<String>,
    pub factory: Box<dyn Fn() -> Box<dyn Actor> + Send>,
    pub supervision: Supervision,
    pub rows: Vec<CapRow>,
    pub mailbox_bound: usize,
    pub init_spec: Vec<u8>,
}

impl Spawn {
    pub fn new(
        name: Option<&str>,
        factory: Box<dyn Fn() -> Box<dyn Actor> + Send>,
    ) -> Spawn {
        Spawn {
            name: name.map(|s| s.to_string()),
            factory,
            supervision: Supervision::FailFast,
            rows: Vec::new(),
            mailbox_bound: DEFAULT_MAILBOX_BOUND,
            init_spec: Vec::new(),
        }
    }

    pub fn supervision(mut self, s: Supervision) -> Spawn {
        self.supervision = s;
        self
    }

    pub fn rows(mut self, rows: Vec<CapRow>) -> Spawn {
        self.rows = rows;
        self
    }

    pub fn bound(mut self, bound: usize) -> Spawn {
        self.mailbox_bound = bound;
        self
    }

    pub fn init_spec(mut self, spec: &[u8]) -> Spawn {
        self.init_spec = spec.to_vec();
        self
    }
}

pub const DEFAULT_MAILBOX_BOUND: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// Named actors refuse collision (§3.4).
    NameCollision(String),
    /// `ticker` is runtime-reserved (collocated services, §4).
    ReservedName,
}

pub struct Handle {
    pub id: String,
    #[allow(dead_code)] // public API surface; consumed by embeddings
    pub name: Option<String>,
}

/// The clock face — injected, so the ONLY wall-clock reader is
/// whatever sits behind it (§6: clock obliviousness for everyone
/// else). Tests inject a manual clock; determinism is preserved.
/// Send + Sync per the multithread-readiness bounds (round 21).
/// NOTE: `set_clock` moves the HOST face's clock; the journal's
/// commitTs clock is fixed at construction (`with_journal`).
pub trait ClockFace: Send + Sync {
    fn now_micros(&self) -> i64;
}

pub struct WallClock;

impl ClockFace for WallClock {
    fn now_micros(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0)
    }
}

/// Manual clock for tests: time moves when the test says so.
pub struct ManualClock(pub i64);

impl ClockFace for ManualClock {
    fn now_micros(&self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PumpStats {
    /// Progress passes — deliveries, busy re-passes, and dead-letter
    /// drains all count (they all moved the world). The precise
    /// record of refusals lives in telemetry.
    pub passes: usize,
    /// ROUND 36 — the pool pump's phase timings (pool mode only).
    /// `collect_us` is the pump BLOCKING while workers run — the
    /// parallel-executed segment; everything else is the serial
    /// committer's. THE AMDAHL POINT: `serial_fraction()` against
    /// these numbers is where throughput work pays and where it
    /// cannot.
    pub profile: Option<PoolProfile>,
}

/// The committer's phase ledger for one pool run (microseconds).
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolProfile {
    pub waves: usize,
    /// Scanning for deliverable actors (journal pending reads).
    pub compose_us: u64,
    /// Taking actors + building/sending jobs.
    pub stage_dispatch_us: u64,
    /// The pump blocked on results — workers ran here (parallel).
    pub collect_us: u64,
    /// land_effects: validate + classify + commit, in dispatch order.
    pub apply_us: u64,
    /// Poll injection (armed tickers).
    pub polls_us: u64,
    /// The run's total wall time — the denominator the throughput
    /// bounds are computed against.
    pub wall_us: u64,
}

impl PoolProfile {
    /// The committer's serial work per exchange (µs) — THE AMDAHL
    /// POINT: throughput caps at `1e6 / this` no matter how many
    /// workers exist. The lever is shrinking apply+compose, not
    /// adding parallelism.
    pub fn committer_us_per_exchange(&self, exchanges: usize) -> f64 {
        if exchanges == 0 {
            return 0.0;
        }
        let serial = self.compose_us + self.stage_dispatch_us + self.apply_us + self.polls_us;
        serial as f64 / exchanges as f64
    }
}

struct Entry {
    id: String,
    /// `None` = terminal (fail-fast crash); pending mail drains to
    /// dead letters — fail-fast is loud, never silently holding.
    actor: Option<Box<dyn Actor>>,
    factory: Box<dyn Fn() -> Box<dyn Actor> + Send>,
    supervision: Supervision,
    rows: Vec<CapRow>,
    /// The spawn-time rows — hydration resets to these before
    /// replaying journaled Mint facts (idempotent hydration).
    base_rows: Vec<CapRow>,
    /// Mailbox bound — busy checks read the journal's pending count
    /// against this (§3.2). The queue itself lives in the journal
    /// (§3.1: the mailbox IS a cursor over the context journal).
    bound: usize,
    /// The boot record this actor was initialized with (§7.6) —
    /// hydration re-initializes from it (snapshot restore included).
    boot: BootRecord,
    /// Hydration replay start: 1 without a snapshot; the snapshot's
    /// covers position + 1 with one (the boot already restored
    /// state-as-of-covers; replaying the covered prefix would be
    /// redundant work with identical results).
    replay_from: u64,
}

/// ROUND 39 — `snapshot_actor`'s refusal reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    UnknownActor,
    /// The actor's `snapshot_state` face returned nothing (a guest
    /// without the `snapshot` export).
    NoSnapshotState,
    BlobStore,
    BadLetter,
    Journal(JournalError),
}

impl From<JournalError> for SnapshotError {
    fn from(e: JournalError) -> Self {
        SnapshotError::Journal(e)
    }
}

pub struct Context {
    name: String,
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
    /// ROUND 35 — CQRS plumbing: the blob face is shared with the
    /// pool's worker threads (Arc clones); reads are `&self` with
    /// provider-internal locking (round 21's bound).
    blobs: Arc<dyn BlobFace>,
    /// ROUND 35 — the journal sits behind an RwLock so the executor
    /// can hand `JournalRead` to parallel workers while the committer
    /// alone takes write access (single dedicated writer, parallel
    /// readers — mechanically enforced, no unsafe).
    journal: Arc<RwLock<Box<dyn JournalFace>>>,
    /// The durability target (§8.5). `None` = RAM tier (hot journal
    /// only, no persistence). `Some` = the durability driver drains
    /// the hot journal's op log to this face on flush (round 31:
    /// batch + pipeline — the fsync/quorum wait amortizes across
    /// the overlay; crash = redo unflushed activations).
    durable: Option<Box<dyn JournalFace>>,
    /// Index into the hot journal's op log — the sync watermark
    /// (consumed by flush_durability when the durable face is present).
    #[allow(dead_code)]
    synced_through: usize,
    /// ROUND 34 — THE DURABILITY DRAIN POLICY (§8.2/§8.5): when the
    /// hot journal's unsynced op log reaches this many ops (and at
    /// quiescence), `flush_durability` lands them on the durable
    /// face as ONE transaction. Durability IO is a function of THIS
    /// knob — never of the exchange count. Default 256; no-op on the
    /// RAM tier.
    max_ops_per_flush: usize,
    /// ROUND 34 — the worker pool (§6): transitions on `workers`
    /// threads, commits land in dispatch order on the pump thread.
    /// 0/1 = the serial pump (deterministic mode, tests/replay).
    workers: usize,
    /// Replies to `external` land here — the embedding's drainable
    /// inbox (§3.4: the embedding's handle). The letters are ALSO
    /// journaled (everything committed is); this is the convenience
    /// view.
    external_outbox: VecDeque<Envelope>,
    /// ROUND 44 — the telemetry outbox (§3.6): one bounded ring per
    /// context, drop-oldest. "Never backpressures" is structural:
    /// when the ring is full the OLDEST record falls out and the
    /// drop ledger counts it; business flow never waits on telemetry.
    telemetry: VecDeque<TelemetryRecord>,
    /// The ring's capacity (builder knob, default 4096).
    telemetry_cap: usize,
    /// Drops since the last drain, by kind — a drain that lost
    /// records leads with a `drop` record carrying this ledger.
    telemetry_dropped: HashMap<String, u64>,
    /// Whether the pump mints `exchange` records (the span
    /// skeleton, §3.6). Default ON; benchmarks turn it off.
    exchange_records: bool,
    /// RAM mirror of the journal's dead-letter facts (the fact is the
    /// truth; this is the accessor).
    dead_letters: Vec<Envelope>,
    /// Poison-letter strikes, keyed (actor, seq) — RUNTIME bookkeeping
    /// on the mailbox tier, deliberately NOT journaled (retry policy
    /// state; a crash resetting strikes just re-tries, which
    /// at-least-once permits).
    strikes: HashMap<(String, u64), u64>,
    clock: Box<dyn ClockFace>,
    counters: u64,
    /// Round-robin scan start — a perpetually-requeued actor must
    /// never starve its neighbors (the busy self-heal depends on
    /// neighbors getting turns).
    next_scan: usize,
    /// ROUND 36 — indices with a transition dispatched and not yet
    /// applied: compose skips them (single-cursor holds across the
    /// pipeline) and the terminal drain must never fire on them.
    dispatched: std::collections::HashSet<usize>,
    /// The staged wave's landing data, in dispatch order (seq order):
    /// the parked envelope gives land_effects its ack position.
    dispatched_order: VecDeque<usize>,
    dispatched_envs: HashMap<usize, mwydyn_data::Envelope>,
}

impl Context {
    /// A context ships with its ticker collocated (rounds 02/04)
    /// and an in-memory journal. For a durable context:
    /// `Context::with_journal(name, Box::new(SQLiteJournal::open(..)))`.
    pub fn new(name: impl Into<String>) -> Context {
        Context::with_journal(name, Box::new(InMemoryJournal::new(Box::new(WallClock))))
    }

    /// The journal face is injected — the durability tier IS the
    /// face choice (§8.5: InMemory for RAM tier, SQLite for log
    /// tier, external providers later). Blobs stay on the RAM store
    /// by default; see `with_stores` for the durable combo.
    pub fn with_journal(name: impl Into<String>, journal: Box<dyn JournalFace>) -> Context {
        Context::with_stores(name, journal, Box::new(RamBlobStore::new()))
    }

    /// The durable combo: journal face + blob face injected. For the
    /// durable tier both point at SQLite-backed providers.
    pub fn with_stores(
        name: impl Into<String>,
        journal: Box<dyn JournalFace>,
        blobs: Box<dyn BlobFace>,
    ) -> Context {
        let mut ctx = Context {
            name: name.into(),
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            blobs: Arc::from(blobs),
            journal: Arc::new(RwLock::new(journal)),
            durable: None,
            synced_through: 0,
            max_ops_per_flush: DEFAULT_MAX_OPS_PER_FLUSH,
            workers: 0,
            external_outbox: VecDeque::new(),
            telemetry: VecDeque::new(),
            telemetry_cap: DEFAULT_TELEMETRY_CAPACITY,
            telemetry_dropped: HashMap::new(),
            exchange_records: true,
            dead_letters: Vec::new(),
            strikes: HashMap::new(),
            clock: Box::new(WallClock),
            counters: 0,
            next_scan: 0,
            dispatched: std::collections::HashSet::new(),
            dispatched_order: VecDeque::new(),
            dispatched_envs: HashMap::new(),
        };
        ctx.spawn_entry(
            Spawn::new(Some("ticker"), Box::new(|| Box::new(Ticker::new())))
                .supervision(Supervision::Restart),
        )
        .expect("the ticker is the first spawn; no collision possible");
        ctx.spawn_entry(Spawn::new(
            Some(SNAPSHOTS),
            Box::new(|| Box::new(SnapshotsSink)),
        ))
        .expect("the snapshots sink is the second spawn; no collision possible");
        ctx
    }

    /// THE PIPELINED EXECUTOR'S DURABILITY SEAM (round 31): the hot
    /// InMemory journal receives every commit instantly (transitions
    /// never wait); the durable face receives batched ops on flush
    /// — the fsync/quorum wait amortizes across the overlay. Crash
    /// = overlay dies, durable stands, recovery replays from the
    /// last flush (at-least-once activation redo; edge idempotency
    /// by (context, actor, seq)).
    pub fn with_durability(
        name: impl Into<String>,
        durable: Box<dyn JournalFace>,
        blobs: Box<dyn BlobFace>,
    ) -> Context {
        let mut ctx = Context {
            name: name.into(),
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            blobs: Arc::from(blobs),
            journal: Arc::new(RwLock::new(Box::new(crate::journal::RecordingJournal::new(
                Box::new(InMemoryJournal::new(Box::new(WallClock))),
            )))),
            durable: Some(durable),
            synced_through: 0,
            max_ops_per_flush: DEFAULT_MAX_OPS_PER_FLUSH,
            workers: 0,
            external_outbox: VecDeque::new(),
            telemetry: VecDeque::new(),
            telemetry_cap: DEFAULT_TELEMETRY_CAPACITY,
            telemetry_dropped: HashMap::new(),
            exchange_records: true,
            dead_letters: Vec::new(),
            strikes: HashMap::new(),
            clock: Box::new(WallClock),
            counters: 0,
            next_scan: 0,
            dispatched: std::collections::HashSet::new(),
            dispatched_order: VecDeque::new(),
            dispatched_envs: HashMap::new(),
        };
        ctx.spawn_entry(
            Spawn::new(Some("ticker"), Box::new(|| Box::new(Ticker::new())))
                .supervision(Supervision::Restart),
        )
        .expect("the ticker is the first spawn; no collision possible");
        ctx.spawn_entry(Spawn::new(
            Some(SNAPSHOTS),
            Box::new(|| Box::new(SnapshotsSink)),
        ))
        .expect("the snapshots sink is the second spawn; no collision possible");
        ctx
    }

    /// The RAM combo (tests, CLI tools): hot journal only, no
    /// durability target.
    pub fn ram(name: impl Into<String>) -> Context {
        Self::with_journal(name, Box::new(InMemoryJournal::new(Box::new(WallClock))))
    }


    pub fn name(&self) -> &str {
        &self.name
    }

    /// Moves the HOST face's clock (ticker reads, host-side views).
    /// The journal's commitTs clock is fixed at construction.
    pub fn set_clock(&mut self, clock: Box<dyn ClockFace>) {
        self.clock = clock;
    }

    /// The telemetry outbox, undrained (§3.6) — the observation
    /// view (a copy; the ring may wrap, so no slice view exists).
    /// [`Context::drain_telemetry`] is the consumption handle.
    pub fn telemetry(&self) -> Vec<TelemetryRecord> {
        self.telemetry.iter().cloned().collect()
    }

    /// Drain the telemetry outbox (§3.6) — the embedding's handle,
    /// `drain_external`'s sibling. A drain that lost records since
    /// the last one LEADS with a synthetic `drop` record: the
    /// pipeline carries its own loss ledger.
    pub fn drain_telemetry(&mut self) -> Vec<TelemetryRecord> {
        let mut out = Vec::new();
        if !self.telemetry_dropped.is_empty() {
            let dropped: u64 = self.telemetry_dropped.values().sum();
            let by_kind = {
                let mut v: Vec<(String, u64)> =
                    self.telemetry_dropped.drain().collect();
                v.sort_by(|a, b| (a.0.len(), &a.0).cmp(&(b.0.len(), &b.0)));
                v
            };
            out.push(TelemetryRecord {
                kind: TelemetryKind::Drop,
                ts: self.clock.now_micros(),
                actor: String::new(),
                seq: 0,
                payload: TelPayload::Drop { dropped, by_kind },
            });
        }
        out.extend(self.telemetry.drain(..));
        out
    }

    /// The ring write (§3.6): drop-oldest at capacity, the ledger
    /// counts the loss. Records arrive PRE-STAMPED — every producer
    /// mints `ts`/`actor`/`seq` at its own site; nothing rides
    /// unstamped.
    fn push_telemetry(&mut self, record: TelemetryRecord) {
        if self.telemetry.len() >= self.telemetry_cap {
            if let Some(oldest) = self.telemetry.pop_front() {
                *self
                    .telemetry_dropped
                    .entry(oldest.kind.as_str().to_string())
                    .or_insert(0) += 1;
            }
        }
        self.telemetry.push_back(record);
    }

    /// The runtime's own exchange record (§3.6 lineage attachment):
    /// the span skeleton, one per delivery attempt. `ts` is ts_out.
    fn exchange_record(&self, seq: u64, actor: &str, from: &Address, ack: Option<u64>, ts_in: i64, outcome: Outcome) -> TelemetryRecord {
        TelemetryRecord {
            kind: TelemetryKind::Exchange,
            ts: self.clock.now_micros(),
            actor: actor.to_string(),
            seq,
            payload: TelPayload::Exchange { from: from.clone(), ack, ts_in, outcome },
        }
    }

    pub fn dead_letters(&self) -> &[Envelope] {
        &self.dead_letters
    }

    /// Drain the external outbox — the embedding's replies (§3.4).
    pub fn drain_external(&mut self) -> Vec<Envelope> {
        self.external_outbox.drain(..).collect()
    }

    /// Blob-store size — diagnostics/garbage observation only.
    pub fn blob_count(&self) -> usize {
        self.blobs.count()
    }

    /// Reference-aware GC (§4, round 16 mechanized): reachability =
    /// digests referenced by ANY journaled entry on ANY lineage
    /// (active or ghost — ghost reads must keep working, round 19).
    /// Everything else is round-16 garbage: eager allocations that
    /// never earned a journaled reference. Durable actor state will
    /// join the reachability set when §7.6 snapshots land.
    ///
    /// Payload scanning is a GC-sweep concern, not a hot-path one:
    /// blob refs are DATA-MODEL primitives (tag 1002), so this walk
    /// does not violate the schema-blind core law (round 10).
    pub fn gc(&mut self) -> Result<usize, JournalError> {
        let mut referenced = std::collections::HashSet::new();
        for lin in jread(&self.journal).lineages(&self.name)? {
            let upto = jread(&self.journal).covered(&self.name, lin.id)?;
            if upto == 0 {
                continue;
            }
            for env in jread(&self.journal).read(&self.name, lin.id, 1, upto)? {
                // The envelope's canonical form wraps the payload as
                // opaque bytes; the payload ITSELF is canonical CBOR
                // carrying the tag-1002 refs.
                if let Ok(v) =
                    mwydyn_data::decode(&env.letter.payload, &mwydyn_data::DecodeOptions::default())
                {
                    let mut refs = Vec::new();
                    mwydyn_data::collect_blob_refs(&v, &mut refs, 0);
                    referenced.extend(refs);
                }
            }
        }
        Ok(self.blobs.retain(&referenced))
    }

    /// Spawn an actor. Naming is opt-in (§3.4): named = addressable,
    /// refuse-on-collision; unnamed = only the spawner's handle.
    /// `ticker` is runtime-reserved.
    pub fn spawn(&mut self, spec: Spawn) -> Result<Handle, SpawnError> {
        if matches!(spec.name.as_deref(), Some("ticker") | Some(SNAPSHOTS)) {
            return Err(SpawnError::ReservedName);
        }
        self.spawn_entry(spec)
    }

    fn spawn_entry(&mut self, spec: Spawn) -> Result<Handle, SpawnError> {
        if let Some(n) = &spec.name {
            if self.by_name.contains_key(n) {
                return Err(SpawnError::NameCollision(n.clone()));
            }
        }
        self.counters += 1;
        let id = format!("ins-{}", self.counters);
        let mut actor = (spec.factory)();
        let entry_idx = self.entries.len();

        // §7.6 boot record: identity + parent + durable state (named
        // actors scan the journal for their latest snapshot letter on
        // the ACTIVE lineage; unnamed = ephemeral, no restore).
        let snapshot = match &spec.name {
            Some(n) => self.scan_snapshot(n),
            None => None,
        };
        let replay_from = snapshot.as_ref().map(|(_, covers)| covers + 1).unwrap_or(1);
        let boot = BootRecord {
            actor: crate::context::ActorIdentity {
                id: id.clone(),
                context: self.name.clone(),
                name: spec.name.clone(),
            },
            parent: Address::Instance(EXTERNAL.into()),
            snapshot: snapshot.as_ref().map(|(r, _)| r.clone()),
            config: spec.init_spec.clone(),
        };

        actor.set_ambient(crate::actor::AmbientParts {
            journal: std::sync::Arc::clone(&self.journal),
            blobs: std::sync::Arc::clone(&self.blobs),
            context: std::sync::Arc::from(self.name.as_str()),
            rows: spec.rows.clone(),
        });
        {
            let jguard = jread(&self.journal);
            let host = Host::new(
                &*self.blobs,
                &**jguard,
                &self.name,
                &spec.rows,
                self.clock.now_micros(),
            );
            actor.init(&host, &boot);
        }
        self.entries.push(Entry {
            id: id.clone(),
            actor: Some(actor),
            factory: spec.factory,
            supervision: spec.supervision,
            rows: spec.rows.clone(),
            base_rows: spec.rows,
            bound: spec.mailbox_bound,
            boot,
            replay_from,
        });
        if let Some(n) = &spec.name {
            self.by_name.insert(n.clone(), entry_idx);
        }
        self.by_id.insert(id.clone(), entry_idx);
        // §3.2 lifecycle fact as telemetry (§3.6): the runtime
        // declares `spawned` — the embedding's view of the cast.
        self.push_telemetry(TelemetryRecord {
            kind: TelemetryKind::Lifecycle,
            ts: self.clock.now_micros(),
            actor: id.clone(),
            seq: 0,
            payload: TelPayload::Lifecycle {
                event: LifecycleEvent::Spawned,
                detail: spec.name.clone(),
            },
        });
        Ok(Handle {
            id,
            name: spec.name,
        })
    }

    fn is_external(to: &Address) -> bool {
        matches!(to, Address::Instance(id) if id == EXTERNAL)
    }

    fn resolve(&self, to: &Address) -> Option<usize> {
        match to {
            Address::Named { context, name } => {
                if context != &self.name {
                    // Cross-context is saga territory (§3.5) — from a
                    // single context, a foreign address simply does
                    // not resolve.
                    return None;
                }
                self.by_name.get(name).copied()
            }
            Address::Instance(id) => self.by_id.get(id).copied(),
        }
    }

    /// External post — the injection handle (§3.4, round 44). The
    /// letter is JOURNALED first — write-before-delivery (§8.2) —
    /// then it is simply *pending*: delivery reads the journal.
    ///
    /// ROUND 44, two ratified additions:
    /// - **`Accepted` carries the seq** — the durable channel for
    ///   external id ↔ seq mapping. The mapping lives in the
    ///   embedder's store; telemetry never carries it (§3.6).
    /// - **`parent` stamps causality** into the envelope's `ack`
    ///   under the round-26/29 law (runtime-stamped, host-decided —
    ///   the same field the snapshot `covers` machinery trusts).
    ///   The parent must be a seq this context knows (any lineage)
    ///   or the post is refused `invalid` — loud beats a silently
    ///   unresolvable span downstream. This is how request/response
    ///   across the external boundary closes causal loops: drain
    ///   (seq out) → external work → inject (parent in).
    pub fn post(
        &mut self,
        to: &Address,
        letter: Letter,
        authority: Vec<String>,
        parent: Option<u64>,
    ) -> Verdict {
        let mut letter = letter;
        letter.authority = authority;
        if letter.validate().is_err() || !payload_canonical(&letter.payload) {
            self.refuse_injection(to, "invalid");
            return Verdict::Refused(RefuseReason::Invalid);
        }
        if let Some(p) = parent {
            if !jread(&self.journal).seq_exists(&self.name, p).unwrap_or(false) {
                self.refuse_injection(to, "unknown parent seq");
                return Verdict::Refused(RefuseReason::Invalid);
            }
        }
        let Some(idx) = self.resolve(to) else {
            self.refuse_injection(to, "unknown-target");
            return Verdict::Refused(RefuseReason::UnknownTarget);
        };
        let to_actor = self.entries[idx].id.clone();
        let bound = self.entries[idx].bound;
        if jread(&self.journal).pending_count(&self.name, &to_actor).unwrap_or(0) >= bound {
            self.refuse_injection(to, "busy");
            return Verdict::Refused(RefuseReason::Busy);
        }
        let commit = jwrite(&self.journal)
            .commit(
                &self.name,
                vec![JournalOp::Append(Pending {
                    letter,
                    from: Address::Instance(EXTERNAL.into()),
                    to_actor,
                    to: to.clone(),
                    ack: parent,
                    producer: EXTERNAL.into(),
                })],
            )
            .expect("injection commit");
        Verdict::Accepted { seq: commit.first_seq }
    }

    /// An injection refusal lands in telemetry (§3.6, stage
    /// `injection`) — refusal-minting-is-policy, and the policy
    /// events are runtime facts.
    fn refuse_injection(&mut self, to: &Address, reason: &'static str) {
        self.push_telemetry(TelemetryRecord {
            kind: TelemetryKind::Refusal,
            ts: self.clock.now_micros(),
            actor: String::new(),
            seq: 0,
            payload: TelPayload::Refusal {
                stage: "injection".into(),
                reason: format!("{reason} ({to:?})"),
            },
        });
    }

    /// §7.6 restore scan: the latest `mwydyn.snapshot.save@1` letter
    /// ON THE ACTIVE LINEAGE carrying `name` — (blob ref, covers).
    /// Ghost letters are excluded by construction (active lineage
    /// only), so a rewind automatically restores the world including
    /// WHICH SNAPSHOTS EXIST (round 19's discipline, applied).
    /// The position comes from the letter's runtime-stamped `ack`
    /// (round 26/29): host-decided, exact, unforgeable.
    /// §7.6 SAVE, RUNTIME-DRIVEN (round 39): pull the actor's
    /// durable state through its `snapshot_state` face, store it as a
    /// blob (the RUNTIME's own ambient power — caps are for actors),
    /// and land the snapshot letter with the runtime-stamped
    /// `covers` = the actor's CURRENT cursor — everything acked
    /// before this call is inside the snapshot. The ref rides as a
    /// DIRECT BlobRef (GC-visible: the letter protects the blob).
    /// Reopen + spawn under the same NAME restores it (round 39's
    /// scan accepts this letter; `covers + 1` seeds `replay_from`).
    pub fn snapshot_actor(&mut self, name: &str) -> Result<(), SnapshotError> {
        let idx = *self
            .by_name
            .get(name)
            .ok_or(SnapshotError::UnknownActor)?;
        let state = self.entries[idx]
            .actor
            .as_mut()
            .and_then(|a| a.snapshot_state())
            .ok_or(SnapshotError::NoSnapshotState)?;
        let r = self
            .blobs
            .put(&state, "application/x-mwydyn-state", None)
            .map_err(|_| SnapshotError::BlobStore)?;
        let actor_id = self.entries[idx].id.clone();
        let active = jread(&self.journal).active_lineage(&self.name)?;
        let covers =
            jread(&self.journal).cursor(&self.name, active, &actor_id)?.unwrap_or(0);
        let ref_value = Value::BlobRef {
            digest: r.digest,
            media_type: r.media_type,
            size: r.size,
        };
        // The EXACT payload shape scan_snapshot matches (round 39: the
        // ref is a DIRECT BlobRef — GC-visible, and the wrapped
        // spelling never matched).
        let payload = mwydyn_data::encode(
            &Value::map(vec![
                ("name".into(), Value::Text(name.into())),
                ("ref".into(), ref_value),
            ])
            .expect("fixed hygienic keys"),
        );
        let letter = Letter::new(SNAPSHOT_SAVE_KIND, payload, None, None, vec![])
            .map_err(|_| SnapshotError::BadLetter)?;
        let sink_addr = Address::Named {
            context: self.name.clone(),
            name: crate::context::SNAPSHOTS.into(),
        };
        let sink_actor = self
            .resolve(&sink_addr)
            .map(|tidx| self.entries[tidx].id.clone())
            .expect("the snapshots sink is a runtime-reserved spawn");
        let _ = jwrite(&self.journal).commit(
            &self.name,
            vec![JournalOp::Append(Pending {
                letter,
                from: Address::Instance(actor_id),
                to_actor: sink_actor,
                to: sink_addr,
                ack: Some(covers),
                producer: name.to_string(),
            })],
        )?;
        Ok(())
    }

    fn scan_snapshot(&self, name: &str) -> Option<(BlobRef, u64)> {
        let active = jread(&self.journal).active_lineage(&self.name).ok()?;
        let covered = jread(&self.journal).covered(&self.name, active).ok()?;
        let mut found: Option<(BlobRef, u64)> = None;
        for env in jread(&self.journal).read(&self.name, active, 1, covered).ok()? {
            if env.letter.kind != SNAPSHOT_SAVE_KIND {
                continue;
            }
            let Ok(v) =
                mwydyn_data::decode(&env.letter.payload, &mwydyn_data::DecodeOptions::default())
            else {
                continue;
            };
            let Value::Map(pairs) = v else {
                continue;
            };
            let matches = pairs
                .iter()
                .any(|(k, v)| k == "name" && v.as_text() == Some(name));
            if !matches {
                continue;
            }
            let ref_field = pairs.iter().find(|(k, _)| k == "ref").map(|(_, v)| v);
            // ROUND 39: the ref rides as a DIRECT BlobRef (GC-visible —
            // the snapshot blob must be reachable from the letter) and
            // is also accepted Bytes-wrapped (the native counter's
            // original spelling, which never matched — the snapshot
            // restore path ran vacuously until this fix).
            let ref_value = match ref_field {
                Some(v @ Value::BlobRef { .. }) => Some(v.clone()),
                Some(Value::Bytes(wrapped)) => mwydyn_data::decode(
                    wrapped,
                    &mwydyn_data::DecodeOptions::default(),
                )
                .ok(),
                _ => None,
            };
            let Some(Value::BlobRef { digest, media_type, size }) = ref_value else {
                continue;
            };
            // The covers position: the runtime-stamped ack (None here
            // means the stamp predates round 26 — skip).
            let Some(ack) = env.ack else {
                continue;
            };
            found = Some((
                BlobRef {
                    digest: digest.clone(),
                    media_type: media_type.clone(),
                    size,
                },
                ack,
            ));
        }
        found // ascending scan → the LAST match is the latest
    }

    /// Deliver ONE message, round-robin from `next_scan` —
    /// deterministic AND fair. Returns false when nothing was
    /// deliverable.
    fn deliver_one(&mut self) -> bool {
        let n = self.entries.len();
        if n == 0 {
            return false;
        }
        for k in 0..n {
            let idx = (self.next_scan + k) % n;
            let entry = &self.entries[idx];
            let pending = jread(&self.journal).pending(&self.name, &entry.id).unwrap_or_default();
            if entry.actor.is_none() {
                // Terminal: fail-fast is loud — never silently holding
                // mail. Strike the pending letters out (journaled).
                if self.drain_terminal(idx) {
                    self.next_scan = (idx + 1) % n;
                    return true;
                }
                continue;
            }
            if let Some(front) = pending.first() {
                let env = front.clone();
                self.next_scan = (idx + 1) % n;
                return self.deliver(idx, env);
            }
        }
        false
    }

    fn deliver(&mut self, idx: usize, env: Envelope) -> bool {
        // §3.5 transactional receive: fork pre-receive state. On
        // refusal, restore the fork — the state rolls back to the
        // ack boundary, the letter stays pending (Busy) or strikes
        // (permanent refusal). No divergence, ever.
        let parts = self.ambient_parts(idx);
        self.entries[idx]
            .actor
            .as_mut()
            .expect("live")
            .set_ambient(parts);
        let fork = self.entries[idx].actor.as_ref().expect("live").fork();

        // ROUND 44 — the dispatch wall stamp (§3.6): ts_in. The
        // journal has commit times but NOT dispatch times, so the
        // exchange record's duration can only come from here.
        let ts_in = self.clock.now_micros();

        // Phase 1 — the pure transition (§3.5). The host face is
        // read-only; effects come back as data. A panic here is a
        // crash — supervision's problem (§3.2), not the message's.
        let outcome = {
            let Context {
                name,
                blobs,
                entries,
                clock,
                ..
            } = self;
            let jguard = jread(&self.journal);
            let entry = &mut entries[idx];
            let host = Host::new(
                &**blobs,
                &**jguard,
                name,
                &entry.rows,
                clock.now_micros(),
            );
            let ctx = MsgCtx {
                from: env.from.clone(),
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                entry
                    .actor
                    .as_mut()
                    .expect("deliver() is only called on live entries")
                    .receive(&ctx, &host, &env.letter)
            }))
        };
        let effects = match outcome {
            Ok(effects) => effects,
            Err(_) => {
                self.entries[idx].actor = Some(fork);
                self.handle_crash(idx, &env, ts_in);
                return true;
            }
        };
        // Phases 2-4 — validate, classify, buffer (round 34: shared
        // with the worker pool's committer). On refusal the fork
        // replaces the mutated state — UNLESS the actor declared its
        // forks stateless (round 37): "just reused", the dirty
        // instance stays and the fork is dropped.
        let kept = self.land_effects(idx, &env, effects, ts_in);
        if !kept && self.entries[idx].actor.as_ref().expect("live").fork_carries_state() {
            self.entries[idx].actor = Some(fork);
        }
        true
    }

    /// ROUND 34 — the committer's tail (§3.5 phases 2-4), shared by
    /// the serial pump and the worker pool: validate ALL effects,
    /// classify (Busy: no commit, Strike: burn attempt), buffer the
    /// transaction. Returns whether the transition's state mutation
    /// is KEPT (`false` = the caller must restore the fork). The
    /// worker pool applies these in DISPATCH order — the journal's
    /// total order is dispatch order, so replay stays deterministic
    /// given the log (round 21's law).
    fn land_effects(
        &mut self,
        idx: usize,
        env: &Envelope,
        effects: Vec<Effect>,
        ts_in: i64,
    ) -> bool {
        let actor_id = self.entries[idx].id.clone();

        // Phase 2 — validate ALL effects before committing ANY
        // (fail-before-compute, §3.5). Projected capacity counts this
        // list's own contributions.
        enum Block {
            Busy,
            Strike(&'static str),
        }
        let mut block: Option<Block> = None;
        let mut projected: HashMap<String, usize> = HashMap::new();
        for eff in &effects {
            match eff {
                Effect::Post { to, letter } => {
                    if letter.validate().is_err() {
                        block = Some(Block::Strike("invalid letter in effects"));
                        break;
                    }
                    // THE PAYLOAD WALL (round 33): payloads are
                    // canonical CBOR, whatever realm produced them —
                    // the journal never stores undecodable bytes, so
                    // GC/snapshot/replay can always walk a payload.
                    if !payload_canonical(&letter.payload) {
                        block = Some(Block::Strike("non-canonical payload in effects"));
                        break;
                    }
                    if Self::is_external(to) {
                        continue;
                    }
                    let Some(tidx) = self.resolve(to) else {
                        block = Some(Block::Strike("unknown target in effects"));
                        break;
                    };
                    let tid = self.entries[tidx].id.clone();
                    let bound = self.entries[tidx].bound;
                    let inflight = projected.entry(tid.clone()).or_insert(0);
                    let queued = jread(&self.journal).pending_count(&self.name, &tid).unwrap_or(0);
                    if queued + *inflight >= bound {
                        block = Some(Block::Busy);
                        break;
                    }
                    *inflight += 1;
                }
                Effect::Mint {
                    source_verb,
                    source_object,
                    ..
                } => {
                    let held = self.entries[idx]
                        .rows
                        .iter()
                        .any(|r| r.verb == *source_verb && r.object == *source_object);
                    if !held {
                        block = Some(Block::Strike("mint without holding the source row"));
                        break;
                    }
                }
                // ROUND 44 (§3.6): actors declare `log`/`metric` only
                // — the runtime kinds are the runtime's to mint, in
                // either realm (compose parity with the barrier's
                // declare gate). Attrs must be maps, like everywhere.
                Effect::Emit { record } => {
                    if !record.kind.actor_declared() {
                        block = Some(Block::Strike("invalid telemetry kind in effects"));
                        break;
                    }
                    let bad_attrs = |a: &Option<mwydyn_data::Value>| {
                        matches!(a, Some(v) if !matches!(v, mwydyn_data::Value::Map(_)))
                    };
                    let bad = match &record.payload {
                        TelPayload::Log { attrs, .. } => bad_attrs(attrs),
                        TelPayload::Metric { attrs, .. } => bad_attrs(attrs),
                        _ => true,
                    };
                    if bad {
                        block = Some(Block::Strike("telemetry attrs must be a map"));
                        break;
                    }
                }
            }
        }

        // Phase 3 — classify (§3.5's two refusal classes). On ANY
        // refusal, the state rolls back to the ack boundary, the
        // letter stays pending (Busy: no attempt burned) or burns an
        // attempt (Strike: N strikes -> dead-letter). Either way the
        // delivery attempt EARNS its exchange record (§3.6: busy
        // included — late-because-busy is trace-relevant).
        match block {
            Some(Block::Busy) => {
                if self.exchange_records {
                    let r = self.exchange_record(
                        env.seq,
                        &actor_id,
                        &env.from,
                        env.ack,
                        ts_in,
                        Outcome::Busy,
                    );
                    self.push_telemetry(r);
                }
                return false;
            }
            Some(Block::Strike(why)) => {
                let key = (actor_id.clone(), env.seq);
                let strikes = self.strikes.entry(key.clone()).or_insert(0);
                *strikes += 1;
                let n = *strikes;
                if self.exchange_records {
                    let r = self.exchange_record(
                        env.seq,
                        &actor_id,
                        &env.from,
                        env.ack,
                        ts_in,
                        Outcome::Struck,
                    );
                    self.push_telemetry(r);
                }
                self.push_telemetry(TelemetryRecord {
                    kind: TelemetryKind::Refusal,
                    ts: self.clock.now_micros(),
                    actor: actor_id.clone(),
                    seq: env.seq,
                    payload: TelPayload::Refusal {
                        stage: "effects".into(),
                        reason: format!("{why} — attempt {n}"),
                    },
                });
                if n >= MAX_ATTEMPTS {
                    let _ = jwrite(&self.journal).commit(
                        &self.name,
                        vec![
                            JournalOp::DeadLetter {
                                actor: actor_id.clone(),
                                why: why.into(),
                            },
                            JournalOp::Ack {
                                actor: actor_id.clone(),
                                position: env.seq,
                            },
                        ],
                    );
                    self.strikes.remove(&key);
                    self.dead_letters.push(env.clone());
                    self.push_telemetry(TelemetryRecord {
                        kind: TelemetryKind::Lifecycle,
                        ts: self.clock.now_micros(),
                        actor: actor_id.clone(),
                        seq: env.seq,
                        payload: TelPayload::Lifecycle {
                            event: LifecycleEvent::DeadLettered,
                            detail: Some(why.into()),
                        },
                    });
                }
                return false;
            }
            None => {}
        }

        // Phase 4 — build ONE journal transaction: the effect posts,
        // the mint facts, and the ack (log owns acks, §3.1). The
        // mutated state is KEPT (caller drops the fork).
        let mut ops: Vec<JournalOp> = Vec::new();
        for eff in &effects {
            match eff {
                Effect::Post { to, letter } => {
                    let to_actor = if Self::is_external(to) {
                        String::new()
                    } else {
                        self.resolve(to)
                            .map(|tidx| self.entries[tidx].id.clone())
                            .expect("validated above")
                    };
                    ops.push(JournalOp::Append(Pending {
                        letter: letter.clone(),
                        from: Address::Instance(actor_id.clone()),
                        to_actor,
                        to: to.clone(),
                        ack: Some(env.seq),
                        producer: actor_id.clone(),
                    }));
                }
                Effect::Mint { verb, object, .. } => {
                    self.entries[idx].rows.push(CapRow::new(verb, object));
                    ops.push(JournalOp::Mint {
                        actor: actor_id.clone(),
                        verb: verb.clone(),
                        object: object.clone(),
                    });
                }
                // ROUND 44 (§3.6): telemetry is OUTSIDE the
                // transaction, literally — emits apply here,
                // best-effort, and a failed journal commit below
                // does not retract them. The runtime STAMPS
                // ts/actor/seq on apply (byzantine posture: actors
                // never see a clock, and attribution is unforgeable).
                Effect::Emit { record } => {
                    let mut record = record.clone();
                    record.ts = self.clock.now_micros();
                    record.actor = actor_id.clone();
                    record.seq = env.seq;
                    self.push_telemetry(record);
                }
            }
        }
        ops.push(JournalOp::Ack {
            actor: actor_id.clone(),
            position: env.seq,
        });
        // ROUND 32/34 — ONE journal transaction per receive: posts +
        // mints + ack together (§8.2's atomicity unit). The hot face
        // applies it INSTANTLY — the mailbox is a cursor over the
        // journal (§3.1), so a buffered ack would re-offer the
        // letter (round 34's lesson: buffering acks = re-delivery).
        // DURABILITY IO is pipelined separately, at the overlay
        // boundary (`flush_durability`), never by withholding ops
        // from the hot journal.
        let commit = jwrite(&self.journal).commit(&self.name, ops).expect("receive commit");
        // Populate the external outbox from the commit's appended
        // envelopes addressed to `external` (§3.4's handle).
        for env in &commit.appended {
            if Self::is_external(&env.to) {
                self.external_outbox.push_back(env.clone());
            }
        }
        self.strikes.remove(&(actor_id.clone(), env.seq));
        if self.exchange_records {
            let r = self.exchange_record(
                env.seq,
                &actor_id,
                &env.from,
                env.ack,
                ts_in,
                Outcome::Committed,
            );
            self.push_telemetry(r);
        }
        true
    }

    /// A terminal entry's pending mail drains loudly to dead letters
    /// (journaled). Used by the serial scan and the pool's wave
    /// composition alike.
    fn drain_terminal(&mut self, idx: usize) -> bool {
        let pending = jread(&self.journal)
            .pending(&self.name, &self.entries[idx].id)
            .unwrap_or_default();
        if pending.is_empty() {
            return false;
        }
        let mut ops = Vec::new();
        for env in &pending {
            ops.push(JournalOp::DeadLetter {
                actor: self.entries[idx].id.clone(),
                why: "actor terminal (fail-fast)".into(),
            });
            ops.push(JournalOp::Ack {
                actor: self.entries[idx].id.clone(),
                position: env.seq,
            });
        }
        let _ = jwrite(&self.journal).commit(&self.name, ops);
        self.dead_letters.extend(pending);
        true
    }

    fn handle_crash(&mut self, idx: usize, env: &Envelope, ts_in: i64) {
        let id = self.entries[idx].id.clone();
        let supervision = self.entries[idx].supervision;
        // The front pending letter is the one that crashed us.
        let front = jread(&self.journal)
            .pending(&self.name, &id)
            .ok()
            .and_then(|p| p.first().map(|e| e.seq));
        // ROUND 44 (§3.6): a crash IS a strike-burn (round 33:
        // panic = strike) — the attempt earns its exchange record
        // and its lifecycle fact, stamped with the crash site's own
        // envelope (the front pending letter's sender rides `env`).
        if self.exchange_records {
            let r = self.exchange_record(
                env.seq,
                &id,
                &env.from,
                env.ack,
                ts_in,
                Outcome::Struck,
            );
            self.push_telemetry(r);
        }
        match supervision {
            Supervision::FailFast => {
                self.entries[idx].actor = None;
                self.push_telemetry(TelemetryRecord {
                    kind: TelemetryKind::Lifecycle,
                    ts: self.clock.now_micros(),
                    actor: id.clone(),
                    seq: env.seq,
                    payload: TelPayload::Lifecycle {
                        event: LifecycleEvent::Crashed,
                        detail: Some("crashed — terminal (fail-fast)".into()),
                    },
                });
                self.push_telemetry(TelemetryRecord {
                    kind: TelemetryKind::Lifecycle,
                    ts: self.clock.now_micros(),
                    actor: id.clone(),
                    seq: env.seq,
                    payload: TelPayload::Lifecycle {
                        event: LifecycleEvent::Terminal,
                        detail: None,
                    },
                });
                // Pending mail drains to dead letters on the next pass.
            }
            Supervision::Restart => {
                let fresh = (self.entries[idx].factory)();
                let parts = self.ambient_parts(idx);
                let mut fresh = fresh;
                fresh.set_ambient(parts);
                self.entries[idx].actor = Some(fresh);
                {
                    let entry = &mut self.entries[idx];
                    let jguard = jread(&self.journal);
                    let host = Host::new(
                        &*self.blobs,
                        &**jguard,
                        &self.name,
                        &entry.rows,
                        self.clock.now_micros(),
                    );
                    entry
                        .actor
                        .as_mut()
                        .expect("fresh from factory")
                        .init(&host, &entry.boot);
                }
                let attempts = front
                    .map(|seq| {
                        let key = (id.clone(), seq);
                        let n = self.strikes.entry(key).or_insert(0);
                        *n += 1;
                        *n
                    })
                    .unwrap_or(1);
                self.push_telemetry(TelemetryRecord {
                    kind: TelemetryKind::Lifecycle,
                    ts: self.clock.now_micros(),
                    actor: id.clone(),
                    seq: env.seq,
                    payload: TelPayload::Lifecycle {
                        event: LifecycleEvent::Crashed,
                        detail: Some(format!("restarted fresh, attempt {attempts}")),
                    },
                });
                if let (Some(seq), true) = (front, attempts >= MAX_ATTEMPTS) {
                    let dead = jread(&self.journal)
                        .pending(&self.name, &id)
                        .ok()
                        .and_then(|p| p.first().cloned());
                    let _ = jwrite(&self.journal).commit(
                        &self.name,
                        vec![
                            JournalOp::DeadLetter {
                                actor: id.clone(),
                                why: "crash loop".into(),
                            },
                            JournalOp::Ack {
                                actor: id.clone(),
                                position: seq,
                            },
                        ],
                    );
                    self.strikes.remove(&(id.clone(), seq));
                    if let Some(env) = dead {
                        self.dead_letters.push(env);
                    }
                    self.push_telemetry(TelemetryRecord {
                        kind: TelemetryKind::Lifecycle,
                        ts: self.clock.now_micros(),
                        actor: id.clone(),
                        seq,
                        payload: TelPayload::Lifecycle {
                            event: LifecycleEvent::DeadLettered,
                            detail: Some("crash loop".into()),
                        },
                    });
                }
            }
        }
    }

    /// Flush the pipelining overlay to the durable journal (§8.2 as
    /// amended by round 31). The hot journal receives commits
    /// instantly; this method drains its op log to the durability
    /// target in ONE batch — the fsync/quorum wait amortizes across
    /// the entire in-flight window. Returns the number of ops
    /// committed.
    pub fn flush_durability(&mut self) -> Result<usize, JournalError> {
        let pending_ops = jread(&self.journal).unsynced_ops().len();
        if pending_ops == 0 {
            return Ok(0);
        }
        let ops = jwrite(&self.journal).take_ops(pending_ops);
        let mut count = 0usize;
        let mut by_ctx: HashMap<String, Vec<JournalOp>> = HashMap::new();
        for (ctx, op) in ops {
            by_ctx.entry(ctx).or_default().push(op);
        }
        for (ctx, ctx_ops) in &by_ctx {
            let c = self.durable.as_mut().expect("durable checked above").commit(ctx, ctx_ops.clone())?;
            count += c.count;
        }
        Ok(count)
    }

    /// Rewind the context to `position` on the active lineage (§8.3):
    /// a journaled fork. The future beyond `position` becomes a ghost
    /// branch — readable behind `ghost.read`, never deliverable.
    /// Actors restart FRESH: state-as-of-position is the §7.6
    /// snapshot contract; fresh is the M2 approximation (documented).
    pub fn rewind(&mut self, position: u64) -> Result<u64, JournalError> {
        let c = jwrite(&self.journal).commit(&self.name, vec![JournalOp::Fork { parent_seq: position }])?;
        for idx in 0..self.entries.len() {
            if self.entries[idx].actor.is_some() {
                let fresh = (self.entries[idx].factory)();
                let mut fresh = fresh;
                fresh.set_ambient(self.ambient_parts(idx));
                self.entries[idx].actor = Some(fresh);
                let entry = &mut self.entries[idx];
                let jguard = jread(&self.journal);
                let host = Host::new(
                    &*self.blobs,
                    &**jguard,
                    &self.name,
                    &entry.rows,
                    self.clock.now_micros(),
                );
                entry.actor.as_mut().expect("fresh").init(&host, &entry.boot);
            }
        }
        self.strikes.clear();
        Ok(c.lineage)
    }

    /// The task pattern (§3.4): an unnamed, fail-fast, RAM-tier
    /// actor addressed by its handle — scale-to-zero by lifecycle.
    pub fn spawn_task(
        &mut self,
        factory: Box<dyn Fn() -> Box<dyn Actor> + Send>,
        init_spec: &[u8],
    ) -> Result<Handle, SpawnError> {
        self.spawn(
            Spawn::new(None, factory)
                .supervision(Supervision::FailFast)
                .init_spec(init_spec),
        )
    }

    /// `call_task` — the embedding's one-call round trip: spawn an
    /// unnamed task, post the request under a minted correlation, run
    /// to quiescence, return the reply from the external outbox.
    /// (Correlations minted here are reply-matching, §2.)
    pub fn call_task(
        &mut self,
        factory: Box<dyn Fn() -> Box<dyn Actor> + Send>,
        mut request: Letter,
    ) -> Result<Option<Letter>, SpawnError> {
        self.counters += 1;
        let correlation = format!("call-{}", self.counters);
        request.correlation = Some(correlation.clone());
        let handle = self.spawn_task(factory, &[])?;
        let verdict = self.post(&Address::Instance(handle.id.clone()), request, Vec::new(), None);
        if !matches!(verdict, Verdict::Accepted { .. }) {
            return Ok(None);
        }
        self.run_until_idle(200);
        let mut found = None;
        let mut keep = VecDeque::new();
        while let Some(e) = self.external_outbox.pop_front() {
            if found.is_none() && e.letter.correlation.as_deref() == Some(&correlation) {
                found = Some(e.letter);
            } else {
                keep.push_back(e);
            }
        }
        self.external_outbox = keep;
        Ok(found)
    }

    /// Hydration (round 26): rebuild actor state from the journal.
    /// Call after re-spawning the same composition on a reopened
    /// journal — the composition is declarative at open; the journal
    /// is the event truth.
    ///
    /// Per actor: rows reset to spawn rows + journaled Mint facts
    /// (§3.3 tier rule); a fresh instance replays its DELIVERED
    /// letters (`1..=cursor`) through `receive` with effects
    /// DISCARDED and the host clock pinned to each letter's journaled
    /// commit time — state lands exactly at the ack boundary, and
    /// pending letters stay pending for live delivery. Purity
    /// (§3.5) is what makes replay sound at all.
    ///
    /// Returns the number of letters replayed. Known limits (M3+):
    /// the blob store is not yet durable, so replays that read blobs
    /// put before the crash see loud Missing; rewind/fork shared
    /// prefixes replay through the active lineage only.
    pub fn hydrate(&mut self) -> Result<usize, JournalError> {
        // ROUND 34: on the durability overlay tier the HOT journal
        // starts empty on reopen — the delivered history (cursors,
        // letters, mints) lives on the DURABLE face. Read history
        // from wherever it actually is. (Two passes: the immutable
        // history reads complete into owned data before any mutation
        // — the borrow of the history face must not span the replay.)
        type EntryHistory = (Vec<MintFact>, u64, Vec<mwydyn_data::Envelope>);
        let mut per_entry: Vec<EntryHistory> = {
            let history: &dyn JournalFace = match &self.durable {
                Some(d) => &**d,
                None => &**jread(&self.journal),
            };
            let active = history.active_lineage(&self.name)?;
            let mut per_entry = Vec::with_capacity(self.entries.len());
            for entry in &self.entries {
                let mints = history.mints(&self.name, &entry.id)?;
                let cursor = history
                    .cursor(&self.name, active, &entry.id)?
                    .unwrap_or(0);
                let letters = history
                    .letters_for(&self.name, &entry.id, cursor)?
                    .into_iter()
                    .filter(|env| env.seq >= entry.replay_from)
                    .collect::<Vec<_>>();
                per_entry.push((mints, cursor, letters));
            }
            per_entry
        };
        let mut replays = 0usize;
        for idx in 0..self.entries.len() {
            let id = self.entries[idx].id.clone();
            let (mints, _cursor, letters) = per_entry.remove(0);
            // Rows: spawn rows + journaled mints (idempotent — the
            // base reset makes repeated hydrate() calls safe).
            self.entries[idx].rows = self.entries[idx].base_rows.clone();
            for m in &mints {
                self.entries[idx].rows.push(CapRow::new(&m.verb, &m.object));
            }
            // State: fresh instance + boot re-init (snapshot restore
            // included) + deterministic replay of the delivered
            // letters AFTER the snapshot's covers position (the boot
            // already restored state-as-of-covers; replaying the
            // covered prefix would be redundant).
            let fresh = (self.entries[idx].factory)();
            let parts = self.ambient_parts(idx);
            let mut fresh = fresh;
            fresh.set_ambient(parts);
            self.entries[idx].actor = Some(fresh);
            {
                let jguard = jread(&self.journal);
                let Context {
                    name,
                    blobs,
                    entries,
                    clock,
                    ..
                } = self;
                let entry = &mut entries[idx];
                let host = Host::new(&**blobs, &**jguard, name, &entry.rows, clock.now_micros());
                entry
                    .actor
                    .as_mut()
                    .expect("fresh above")
                    .init(&host, &entry.boot);
            }
            for env in letters {
                let jguard = jread(&self.journal);
                let outcome = {
                    let Context {
                        name,
                        blobs,
                        entries,
                        clock,
                        ..
                    } = self;
                    let entry = &mut entries[idx];
                    // Replay clock: the letter's journaled commit time —
                    // deterministic state reconstruction (§6 honored).
                    let ts = env.commit_ts.unwrap_or_else(|| clock.now_micros());
                    let host = Host::new(&**blobs, &**jguard, name, &entry.rows, ts);
                    let ctx = MsgCtx {
                        from: env.from.clone(),
                    };
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        entry
                            .actor
                            .as_mut()
                            .expect("fresh above")
                            .receive(&ctx, &host, &env.letter)
                    }))
                };
                drop(jguard); // telemetry minting below needs &mut self
                // Effects are DISCARDED (they are already journaled);
                // a panic during replay is skipped with a warning —
                // the replayed state is approximate for that actor
                // and the poison letter remains a live-delivery
                // problem for the real pump.
                match outcome {
                    Ok(_) => replays += 1,
                    Err(_) => {
                        replays += 1;
                        self.push_telemetry(TelemetryRecord {
                            kind: TelemetryKind::Refusal,
                            ts: self.clock.now_micros(),
                            actor: id.clone(),
                            seq: env.seq,
                            payload: TelPayload::Refusal {
                                stage: "hydrate".into(),
                                reason: format!("replay panicked at seq {} — skipped", env.seq),
                            },
                        });
                    }
                }
            }
        }
        Ok(replays)
    }

    /// Drive the context until no message is deliverable. `max_passes`
    /// bounds livelock (a permanent busy-loop is a bug, and the bound
    /// makes it a visible one).
    /// ROUND 34 — the durability-drain policy: the overlay lands
    /// the hot journal's op log on the durable face as ONE
    /// transaction at `k` ops (and at quiescence). This knob — never
    /// the exchange count — drives durability IO (fsync/quorum).
    pub fn max_ops_per_flush(&mut self, k: usize) -> &mut Self {
        self.max_ops_per_flush = k.max(1);
        self
    }

    /// ROUND 34 — the worker pool (§6): `n ≥ 2` runs transitions on
    /// `n` worker threads (wave dispatch), commits landing in
    /// dispatch order on the pump thread. 0/1 = the serial pump,
    /// which stays forever as the deterministic mode (tests, replay
    /// verification).
    pub fn workers(&mut self, n: usize) -> &mut Self {
        self.workers = n;
        self
    }

    /// ROUND 44 — the telemetry ring's capacity (§3.6): the storm
    /// valve. Lossiness lives at the ring, never at emission.
    pub fn telemetry_capacity(&mut self, n: usize) -> &mut Self {
        self.telemetry_cap = n.max(1);
        self
    }

    /// ROUND 44 — mint `exchange` records (the span skeleton, §3.6)?
    /// Default ON; throughput runs switch it off.
    pub fn exchange_records(&mut self, on: bool) -> &mut Self {
        self.exchange_records = on;
        self
    }

    pub fn run_until_idle(&mut self, max_passes: usize) -> PumpStats {
        if self.workers > 1 {
            return self.run_until_idle_pool(max_passes);
        }
        let mut stats = PumpStats::default();
        for _ in 0..max_passes {
            self.inject_polls();
            if self.deliver_one() {
                stats.passes += 1;
                self.maybe_flush_durability();
            } else {
                break;
            }
        }
        // Quiescence: the durability barrier is POLICY-respecting —
        // the embedding's explicit `flush_durability()` is THE
        // barrier; tails within the window are the pipelining
        // exposure (at-least-once redo from the durable point).
        self.maybe_flush_durability();
        stats
    }

    /// ROUND 34 — drain the overlay when the hot op log reaches the
    /// policy threshold. No-op on the RAM tier (no durable face).
    fn maybe_flush_durability(&mut self) {
        if self.durable.is_some() && jread(&self.journal).unsynced_ops().len() >= self.max_ops_per_flush
        {
            let _ = self.flush_durability();
        }
    }

    /// ROUND 34 — THE WORKER POOL (§6, round 21's design landed):
    /// transitions run in parallel on `workers` threads (wave
    /// dispatch — one letter per actor per wave, the single-cursor
    /// invariant), and the pump thread is the COMMITTER: results are
    /// applied strictly in dispatch order, so the journal's total
    /// order is dispatch order and replay stays deterministic given
    /// the log ("non-deterministic but recorded"). Phase 2/3/4 — the
    /// validate/classify/buffer tail — never races: it runs on the
    /// committer thread. Journal IO follows the flush policy, never
    /// the exchange count.
    /// ROUND 35 — THE WORKER POOL (§6, CQRS mechanics): persistent
    /// worker threads for the whole run — spawned ONCE (Arc handles
    /// detach them from any borrow of the context), dispatched
    /// through a job queue. Workers take JOURNAL READ guards for
    /// their host face (parallel readers); the pump thread is the
    /// COMMITTER and alone takes write guards between waves (single
    /// dedicated writer — the RwLock enforces the law mechanically).
    /// Results apply strictly in DISPATCH order: the journal's total
    /// order is dispatch order, replay stays deterministic given the
    /// log (round 21's law).
    fn run_until_idle_pool(&mut self, max_passes: usize) -> PumpStats {
        use std::sync::mpsc;
        let run_start = std::time::Instant::now();
        let mut stats = PumpStats::default();
        let workers = self.workers.max(2);
        let (job_tx, job_rx) = mpsc::channel::<WaveJob>();
        let (res_tx, res_rx) = mpsc::channel::<WaveOutcome>();
        let job_rx = Arc::new(std::sync::Mutex::new(job_rx));
        let journal = Arc::clone(&self.journal);
        let blobs = Arc::clone(&self.blobs);
        let name: Arc<str> = Arc::from(self.name.as_str());

        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let rx = Arc::clone(&job_rx);
            let res_tx = res_tx.clone();
            let journal = Arc::clone(&journal);
            let blobs = Arc::clone(&blobs);
            let name = Arc::clone(&name);
            handles.push(std::thread::spawn(move || loop {
                let Ok(job) = rx.lock().expect("job queue").recv() else {
                    break; // queue closed and drained: the run is over
                };
                let WaveJob { seq, mut actor, letter, from, rows, now } = job;
                // ROUND 40: the ambient surface rides the exchange.
                let parts = crate::actor::AmbientParts {
                    journal: std::sync::Arc::clone(&journal),
                    blobs: std::sync::Arc::clone(&blobs),
                    context: std::sync::Arc::clone(&name),
                    rows: rows.clone(),
                };
                actor.set_ambient(parts.clone());
                // Pre-receive copy ON THE WORKER (§3.5's transactional
                // receive): the rollback state, forked in parallel
                // with everything else.
                let rollback = actor.fork();
                let jguard = jread(&journal);
                let read_face: &dyn JournalRead = &**jguard;
                let host = Host::new(&*blobs, read_face, &name, &rows, now);
                let ctx = MsgCtx { from };
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actor.receive(&ctx, &host, &letter)
                }));
                drop(jguard);
                let _ = res_tx.send(WaveOutcome {
                    seq,
                    outcome: outcome.map_err(|_| ()),
                    actor,
                    rollback,
                    now,
                });
            }));
        }
        drop(res_tx); // the pump's own copy; the workers hold clones

        // ROUND 36/38 — WAVE PIPELINING at DEPTH: up to
        // `PIPELINE_DEPTH` waves are dispatched before the oldest one
        // is collected and applied — the committer's collect AND
        // apply phases overlap with worker execution across the whole
        // pipeline, not just one wave deep. Legality (round 36's
        // invariants, unchanged): compose skips dispatched indices
        // (single-cursor holds across the pipeline; the terminal
        // drain can never fire on them), results carry GLOBAL seqs
        // buffered per wave (a young wave's fast results can arrive
        // before an old wave's slow one), and transactions land
        // strictly in dispatch order — the journal's total order =
        // dispatch order.
        const PIPELINE_DEPTH: usize = 4;
        let mut next_seq: u64 = 0;
        let mut stats_passes = 0usize;
        let mut profile = PoolProfile::default();
        let mut buffer: std::collections::BTreeMap<u64, WaveOutcome> = Default::default();
        let mut outstanding: std::collections::VecDeque<(u64, u64)> = Default::default();

        let t = std::time::Instant::now();
        let mut next_wave = self.compose_wave(workers);
        profile.compose_us += t.elapsed().as_micros() as u64;

        loop {
            let tp = std::time::Instant::now();
            self.inject_polls();
            profile.polls_us += tp.elapsed().as_micros() as u64;

            // FILL THE PIPELINE: dispatch composed waves until the
            // depth is reached (or the pass bound bites — a permanent
            // busy-loop is a bug, and the bound makes it a visible
            // one).
            let t = std::time::Instant::now();
            while outstanding.len() < PIPELINE_DEPTH
                && !next_wave.is_empty()
                && stats_passes < max_passes
            {
                let len = next_wave.len();
                let first = next_seq;
                for (idx, env) in next_wave.drain(..) {
                    let entry = &mut self.entries[idx];
                    let moved = entry.actor.take().expect("live");
                    self.dispatched.insert(idx);
                    self.dispatched_order.push_back(idx);
                    self.dispatched_envs.insert(idx, env.clone());
                    let _ = job_tx.send(WaveJob {
                        seq: next_seq,
                        actor: moved,
                        letter: env.letter.clone(),
                        from: env.from.clone(),
                        rows: entry.rows.clone(),
                        now: self.clock.now_micros(),
                    });
                    next_seq += 1;
                }
                outstanding.push_back((first, first + len as u64));
                stats_passes += len;
                profile.waves += 1;

                // COMPOSE the next wave while workers run everything
                // just dispatched (dispatched indices are skipped —
                // they hold their single cursor).
                next_wave = self.compose_wave(workers);
            }
            profile.stage_dispatch_us += t.elapsed().as_micros() as u64;

            // COLLECT + APPLY the OLDEST outstanding wave (workers
            // run the younger ones through both of these phases).
            if let Some((first, end)) = outstanding.pop_front() {
                let t = std::time::Instant::now();
                while !(first..end).all(|sq| buffer.contains_key(&sq)) {
                    match res_rx.recv() {
                        Ok(r) => {
                            buffer.insert(r.seq, r);
                        }
                        Err(_) => {
                            panic!("pool: worker channel closed before the wave answered");
                        }
                    }
                }
                profile.collect_us += t.elapsed().as_micros() as u64;

                let t = std::time::Instant::now();
                for sq in first..end {
                    let r = buffer.remove(&sq).expect("collected above");
                    let idx = self
                        .dispatched_order
                        .pop_front()
                        .expect("staged wave data present");
                    self.dispatched.remove(&idx);
                    let env = self
                        .dispatched_envs
                        .remove(&idx)
                        .expect("staged envelope present");
                    let WaveOutcome { seq: _, outcome, actor, rollback, now } = r;
                    // ROUND 37: the actor's declaration decides the
                    // refusal semantics — stateful forks restore the
                    // pre-receive copy; stateless ones keep the
                    // mutated instance ("just reused").
                    let carries = actor.as_ref().fork_carries_state();
                    match outcome {
                        Ok(effects) => {
                            let kept = self.land_effects(idx, &env, effects, now);
                            if kept || !carries {
                                // Mutation kept — or declared
                                // stateless: the dirty instance stays
                                // and the rollback copy dies.
                                self.entries[idx].actor = Some(actor);
                            } else {
                                // Refused (busy/strike): restore the
                                // pre-receive copy — state rolls back
                                // to the ack boundary.
                                self.entries[idx].actor = Some(rollback);
                            }
                        }
                        Err(()) => {
                            // Crash: restore first, then supervision
                            // decides (restart/fail-fast) — the serial
                            // path's exact semantics.
                            self.entries[idx].actor = Some(rollback);
                            self.handle_crash(idx, &env, now);
                        }
                    }
                }
                profile.apply_us += t.elapsed().as_micros() as u64;
                self.maybe_flush_durability();
            }

            // RE-ARM: the world just moved (posts landed, cursors
            // advanced) — the last compose may have seen every actor
            // in flight. Quiescence: nothing outstanding, nothing
            // composed, or the pass bound has consumed the queue.
            if next_wave.is_empty() && outstanding.is_empty() {
                if stats_passes < max_passes {
                    let t = std::time::Instant::now();
                    next_wave = self.compose_wave(workers);
                    profile.compose_us += t.elapsed().as_micros() as u64;
                }
                if next_wave.is_empty() {
                    break;
                }
            }
        }
        drop(job_tx);
        for h in handles {
            let _ = h.join();
        }
        self.maybe_flush_durability();
        stats.passes += stats_passes;
        profile.wall_us = run_start.elapsed().as_micros() as u64;
        stats.profile = Some(profile);
        stats
    }

    /// Round-robin scan from `next_scan`: one letter per live actor
    /// (the single-cursor invariant), up to `workers` jobs; terminal
    /// entries drain loudly inline. Advances `next_scan` past the
    /// last actioned entry — a perpetually-requeued actor never
    /// starves its neighbors.
    fn compose_wave(&mut self, workers: usize) -> Vec<(usize, Envelope)> {
        let n = self.entries.len();
        let mut wave: Vec<(usize, Envelope)> = Vec::new();
        let mut last_idx: Option<usize> = None;
        for k in 0..n {
            let idx = (self.next_scan + k) % n;
            // ROUND 36: in-flight across the pipeline — its cursor is
            // held; the terminal drain must never fire on it.
            if self.dispatched.contains(&idx) {
                continue;
            }
            if self.entries[idx].actor.is_none() {
                if self.drain_terminal(idx) {
                    last_idx = Some(idx);
                }
                continue;
            }
            if let Ok(Some(front)) =
                jread(&self.journal).pending_first(&self.name, &self.entries[idx].id)
            {
                wave.push((idx, front));
                last_idx = Some(idx);
                if wave.len() >= workers {
                    break;
                }
            }
        }
        if let Some(i) = last_idx {
            self.next_scan = (i + 1) % n;
        }
        wave
    }

    /// Poke poll-affine actors while they hold armed work — timers
    /// fire as messages, never as callbacks (§6). The poll letters
    /// are JOURNALED: on the log tier they replay.
    fn inject_polls(&mut self) {
        let pollers: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.actor.as_ref().is_some_and(|a| a.wants_poll()))
            .map(|(i, _)| i)
            .collect();
        for idx in pollers {
            let poll = Letter::new(
                crate::ticker::TIMER_POLL_KIND,
                Vec::new(),
                None,
                None,
                Vec::new(),
            )
            .expect("fixed kind");
            let _ = jwrite(&self.journal).commit(
                &self.name,
                vec![JournalOp::Append(Pending {
                    letter: poll,
                    from: Address::Instance(EXTERNAL.into()),
                    to_actor: self.entries[idx].id.clone(),
                    to: Address::Instance(self.entries[idx].id.clone()),
                    ack: None,
                    producer: EXTERNAL.into(),
                })],
            );
        }
    }
}
