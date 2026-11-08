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

use mwydyn_data::{Address, Envelope, Letter, Value};

use crate::actor::{Actor, CapRow, Effect, Host, Level, MsgCtx};
use crate::blob::{BlobFace, BlobRef, RamBlobStore};
use crate::journal::{InMemoryJournal, JournalError, JournalFace, JournalOp, Pending};
use crate::ticker::Ticker;

/// The embedding's instance id — posts FROM here; replies TO it land
/// in the drainable external outbox (§3.4: the embedding's handle).
pub const EXTERNAL: &str = "external";

/// Poison-letter strikes before the dead-letter path (§3.1's
/// policy-authorized cursor advance).
pub const MAX_ATTEMPTS: u64 = 3;

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
struct SnapshotsSink;

impl Actor for SnapshotsSink {
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
    Accepted,
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

pub struct Context {
    name: String,
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
    blobs: Box<dyn BlobFace>,
    journal: Box<dyn JournalFace>,
    /// Replies to `external` land here — the embedding's drainable
    /// inbox (§3.4: the embedding's handle). The letters are ALSO
    /// journaled (everything committed is); this is the convenience
    /// view.
    external_outbox: VecDeque<Envelope>,
    telemetry: Vec<(Level, String)>,
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
            blobs,
            journal,
            external_outbox: VecDeque::new(),
            telemetry: Vec::new(),
            dead_letters: Vec::new(),
            strikes: HashMap::new(),
            clock: Box::new(WallClock),
            counters: 0,
            next_scan: 0,
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

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Moves the HOST face's clock (ticker reads, host-side views).
    /// The journal's commitTs clock is fixed at construction.
    pub fn set_clock(&mut self, clock: Box<dyn ClockFace>) {
        self.clock = clock;
    }

    pub fn telemetry(&self) -> &[(Level, String)] {
        &self.telemetry
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
        for lin in self.journal.lineages(&self.name)? {
            let upto = self.journal.covered(&self.name, lin.id)?;
            if upto == 0 {
                continue;
            }
            for env in self.journal.read(&self.name, lin.id, 1, upto)? {
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

        {
            let host = Host::new(
                &self.blobs,
                &*self.journal,
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

    /// External post (the embedding caller speaks from `external`).
    /// The letter is JOURNALED first — write-before-delivery (§8.2) —
    /// then it is simply *pending*: delivery reads the journal.
    pub fn post(&mut self, to: &Address, letter: Letter, authority: Vec<String>) -> Verdict {
        let mut letter = letter;
        letter.authority = authority;
        if letter.validate().is_err() {
            return Verdict::Refused(RefuseReason::Invalid);
        }
        let Some(idx) = self.resolve(to) else {
            return Verdict::Refused(RefuseReason::UnknownTarget);
        };
        let to_actor = self.entries[idx].id.clone();
        let bound = self.entries[idx].bound;
        if self.journal.pending_count(&self.name, &to_actor).unwrap_or(0) >= bound {
            return Verdict::Refused(RefuseReason::Busy);
        }
        let _ = self.journal.commit(
            &self.name,
            vec![JournalOp::Append(Pending {
                letter,
                from: Address::Instance(EXTERNAL.into()),
                to_actor,
                to: to.clone(),
                ack: None,
                producer: EXTERNAL.into(),
            })],
        );
        Verdict::Accepted
    }

    /// §7.6 restore scan: the latest `mwydyn.snapshot.save@1` letter
    /// ON THE ACTIVE LINEAGE carrying `name` — (blob ref, covers).
    /// Ghost letters are excluded by construction (active lineage
    /// only), so a rewind automatically restores the world including
    /// WHICH SNAPSHOTS EXIST (round 19's discipline, applied).
    /// The position comes from the letter's runtime-stamped `ack`
    /// (round 26/29): host-decided, exact, unforgeable.
    fn scan_snapshot(&self, name: &str) -> Option<(BlobRef, u64)> {
        let active = self.journal.active_lineage(&self.name).ok()?;
        let covered = self.journal.covered(&self.name, active).ok()?;
        let mut found: Option<(BlobRef, u64)> = None;
        for env in self.journal.read(&self.name, active, 1, covered).ok()? {
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
            let Some(Value::BlobRef { digest, media_type, size }) =
                pairs.iter().find(|(k, _)| k == "ref").map(|(_, v)| v)
            else {
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
                    size: *size,
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
            let pending = self
                .journal
                .pending(&self.name, &entry.id)
                .unwrap_or_default();
            if entry.actor.is_none() {
                // Terminal: fail-fast is loud — never silently holding
                // mail. Strike the pending letters out (journaled).
                if !pending.is_empty() {
                    let mut ops = Vec::new();
                    for env in &pending {
                        ops.push(JournalOp::DeadLetter {
                            actor: entry.id.clone(),
                            why: "actor terminal (fail-fast)".into(),
                        });
                        ops.push(JournalOp::Ack {
                            actor: entry.id.clone(),
                            position: env.seq,
                        });
                    }
                    let _ = self.journal.commit(&self.name, ops);
                    self.dead_letters.extend(pending);
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
        // Phase 1 — the pure transition (§3.5). The host face is
        // read-only; effects come back as data. A panic here is a
        // crash — supervision's problem (§3.2), not the message's.
        let outcome = {
            let Context {
                name,
                blobs,
                entries,
                clock,
                journal,
                ..
            } = self;
            let entry = &mut entries[idx];
            let host = Host::new(blobs, &**journal, name, &entry.rows, clock.now_micros());
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
                self.handle_crash(idx);
                return true;
            }
        };

        // Phase 2 — validate ALL effects before committing ANY
        // (fail-before-compute, §3.5). Projected capacity counts this
        // list's own contributions. A Busy refusal commits NOTHING —
        // the letter simply stays pending (§3.1: log owns acks), so
        // self-healing needs no requeue machinery at all.
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
                    // The embedding's outbox is unbounded-by-design
                    // (the caller drains it — §3.4's handle).
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
                    let queued = self.journal.pending_count(&self.name, &tid).unwrap_or(0);
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
                Effect::Emit { .. } => {}
            }
        }

        // Phase 3 — classify (§3.5's two refusal classes).
        let actor_id = self.entries[idx].id.clone();
        match block {
            // Temporary: NO commit — the letter stays pending on the
            // journal and is re-delivered on a later pass. No attempt
            // burned (round 07).
            Some(Block::Busy) => {
                self.telemetry.push((
                    Level::Debug,
                    format!("{actor_id}: target busy — left pending, no attempt burned"),
                ));
                return true;
            }
            // Permanent: burn an attempt (runtime bookkeeping); N
            // strikes → the dead-letter commit (DeadLetter fact +
            // cursor advance, journaled together — §3.1).
            Some(Block::Strike(why)) => {
                let key = (actor_id.clone(), env.seq);
                let strikes = self.strikes.entry(key.clone()).or_insert(0);
                *strikes += 1;
                let n = *strikes;
                self.telemetry.push((
                    Level::Warn,
                    format!("{actor_id}: effects refused ({why}) — attempt {n}"),
                ));
                if n >= MAX_ATTEMPTS {
                    let _ = self.journal.commit(
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
                    self.dead_letters.push(env);
                }
                return true;
            }
            None => {}
        }

        // Phase 4 — build ONE journal transaction: the effect posts,
        // the mint facts, and the ack (log owns acks, §3.1; one
        // journal transaction per receive, round 08).
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
                        // THE STAMP (round 26/29): the seq this actor
                        // was processing — a snapshot letter's covers.
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
                Effect::Emit { level, message } => {
                    self.telemetry.push((*level, message.clone()));
                }
            }
        }
        ops.push(JournalOp::Ack {
            actor: actor_id.clone(),
            position: env.seq,
        });
        let commit = self.journal.commit(&self.name, ops).expect("journal commit");
        // The external outbox view takes the stamped envelopes for
        // letters addressed to `external` (§3.4).
        for stamped in &commit.appended {
            if Self::is_external(&stamped.to) {
                self.external_outbox.push_back(stamped.clone());
            }
        }
        self.strikes.remove(&(actor_id.clone(), env.seq));
        true
    }

    fn handle_crash(&mut self, idx: usize) {
        let id = self.entries[idx].id.clone();
        let supervision = self.entries[idx].supervision;
        // The front pending letter is the one that crashed us.
        let front = self
            .journal
            .pending(&self.name, &id)
            .ok()
            .and_then(|p| p.first().map(|e| e.seq));
        match supervision {
            Supervision::FailFast => {
                self.entries[idx].actor = None;
                self.telemetry
                    .push((Level::Error, format!("{id}: crashed — terminal (fail-fast)")));
                // Pending mail drains to dead letters on the next pass.
            }
            Supervision::Restart => {
                let fresh = (self.entries[idx].factory)();
                self.entries[idx].actor = Some(fresh);
                {
                    let entry = &mut self.entries[idx];
                    let host = Host::new(
                        &self.blobs,
                        &*self.journal,
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
                self.telemetry.push((
                    Level::Warn,
                    format!("{id}: crashed — restarted fresh, attempt {attempts}"),
                ));
                if let (Some(seq), true) = (front, attempts >= MAX_ATTEMPTS) {
                    let dead = self
                        .journal
                        .pending(&self.name, &id)
                        .ok()
                        .and_then(|p| p.first().cloned());
                    let _ = self.journal.commit(
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
                }
            }
        }
    }

    /// Rewind the context to `position` on the active lineage (§8.3):
    /// a journaled fork. The future beyond `position` becomes a ghost
    /// branch — readable behind `ghost.read`, never deliverable.
    /// Actors restart FRESH: state-as-of-position is the §7.6
    /// snapshot contract; fresh is the M2 approximation (documented).
    pub fn rewind(&mut self, position: u64) -> Result<u64, JournalError> {
        let c = self
            .journal
            .commit(&self.name, vec![JournalOp::Fork { parent_seq: position }])?;
        for idx in 0..self.entries.len() {
            if self.entries[idx].actor.is_some() {
                let fresh = (self.entries[idx].factory)();
                self.entries[idx].actor = Some(fresh);
                let entry = &mut self.entries[idx];
                let host = Host::new(
                    &self.blobs,
                    &*self.journal,
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
        let verdict = self.post(&Address::Instance(handle.id.clone()), request, Vec::new());
        if verdict != Verdict::Accepted {
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
        let mut replays = 0usize;
        let active = self.journal.active_lineage(&self.name)?;
        for idx in 0..self.entries.len() {
            let id = self.entries[idx].id.clone();
            // Rows: spawn rows + journaled mints (idempotent — the
            // base reset makes repeated hydrate() calls safe).
            let mints = self.journal.mints(&self.name, &id)?;
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
            self.entries[idx].actor = Some(fresh);
            let cursor = self
                .journal
                .cursor(&self.name, active, &id)?
                .unwrap_or(0);
            let replay_from = self.entries[idx].replay_from;
            let letters = self
                .journal
                .letters_for(&self.name, &id, cursor)?
                .into_iter()
                .filter(|env| env.seq >= replay_from)
                .collect::<Vec<_>>();
            {
                let Context {
                    name,
                    blobs,
                    entries,
                    clock,
                    journal,
                    ..
                } = self;
                let entry = &mut entries[idx];
                let host = Host::new(blobs, &**journal, name, &entry.rows, clock.now_micros());
                entry
                    .actor
                    .as_mut()
                    .expect("fresh above")
                    .init(&host, &entry.boot);
            }
            for env in letters {
                let outcome = {
                    let Context {
                        name,
                        blobs,
                        entries,
                        clock,
                        journal,
                        ..
                    } = self;
                    let entry = &mut entries[idx];
                    // Replay clock: the letter's journaled commit time —
                    // deterministic state reconstruction (§6 honored).
                    let ts = env.commit_ts.unwrap_or_else(|| clock.now_micros());
                    let host = Host::new(blobs, &**journal, name, &entry.rows, ts);
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
                // Effects are DISCARDED (they are already journaled);
                // a panic during replay is skipped with a warning —
                // the replayed state is approximate for that actor
                // and the poison letter remains a live-delivery
                // problem for the real pump.
                match outcome {
                    Ok(_) => replays += 1,
                    Err(_) => {
                        replays += 1;
                        self.telemetry.push((
                            Level::Warn,
                            format!("{id}: replay panicked at seq {} — skipped", env.seq),
                        ));
                    }
                }
            }
        }
        Ok(replays)
    }

    /// Drive the context until no message is deliverable. `max_passes`
    /// bounds livelock (a permanent busy-loop is a bug, and the bound
    /// makes it a visible one).
    pub fn run_until_idle(&mut self, max_passes: usize) -> PumpStats {
        let mut stats = PumpStats::default();
        for _ in 0..max_passes {
            // Poke poll-affine actors while they hold armed work —
            // timers fire as messages, never as callbacks (§6). The
            // poll letters are JOURNALED: on the log tier they replay.
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
                let _ = self.journal.commit(
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
            if self.deliver_one() {
                stats.passes += 1;
            } else {
                break;
            }
        }
        stats
    }
}
