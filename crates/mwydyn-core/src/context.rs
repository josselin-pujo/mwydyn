//! The context (spec §3): the trust context is the clock domain is
//! the transaction boundary (round 08) — one wall, three names. RAM
//! tier: mailboxes are VecDeques, the cursor rides the same memory
//! (the round-01 durability rule, trivially), and the atomic commit
//! is validate-all-then-apply-all (§3.5).

use std::collections::{HashMap, VecDeque};

use mwydyn_data::{Address, Envelope, Letter};

use crate::actor::{Actor, CapRow, Effect, Host, Level, MsgCtx};
use crate::blob::RamBlobStore;
use crate::ticker::Ticker;

/// Poison-letter strikes before the dead-letter path (§3.1's
/// policy-authorized cursor advance).
pub const MAX_ATTEMPTS: u64 = 3;

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
/// spawn; escalation routes to the owner handle).
pub struct Spawn {
    pub name: Option<String>,
    pub factory: Box<dyn Fn() -> Box<dyn Actor>>,
    pub supervision: Supervision,
    pub rows: Vec<CapRow>,
    pub mailbox_bound: usize,
    pub init_spec: Vec<u8>,
}

impl Spawn {
    pub fn new(name: Option<&str>, factory: Box<dyn Fn() -> Box<dyn Actor>>) -> Spawn {
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
pub trait ClockFace {
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
    pub delivered: usize,
    pub busy_refusals: usize,
    pub dead_lettered: usize,
}

struct Mailbox {
    queue: VecDeque<Envelope>,
    /// Delivery cursor — part of the SAME durable object as the
    /// queue, never separate bookkeeping (§3.1: log owns acks; on
    /// the RAM tier both are memory, same tier trivially).
    cursor: u64,
    /// Strikes on the queue-front message (poison letters, §3.1).
    attempts: u64,
    bound: usize,
}

impl Mailbox {
    fn new(bound: usize) -> Mailbox {
        Mailbox {
            queue: VecDeque::new(),
            cursor: 0,
            attempts: 0,
            bound,
        }
    }
}

struct Entry {
    id: String,
    /// Mirrors the binding; read by embeddings via `Handle`, kept
    /// here so the binding and the entry cannot disagree.
    #[allow(dead_code)]
    name: Option<String>,
    /// `None` = terminal (fail-fast crash); pending mail drains to
    /// dead letters — fail-fast is loud, never silently holding.
    actor: Option<Box<dyn Actor>>,
    factory: Box<dyn Fn() -> Box<dyn Actor>>,
    supervision: Supervision,
    rows: Vec<CapRow>,
    mailbox: Mailbox,
}

pub struct Context {
    name: String,
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
    blobs: RamBlobStore,
    telemetry: Vec<(Level, String)>,
    dead_letters: Vec<Envelope>,
    clock: Box<dyn ClockFace>,
    counters: u64,
    /// Round-robin scan start — a perpetually-requeued actor must
    /// never starve its neighbors (the busy self-heal depends on
    /// neighbors getting turns).
    next_scan: usize,
}

impl Context {
    /// A context ships with its ticker collocated (rounds 02/04:
    /// ambient services live in the runtime; the ticker is the one
    /// well-known NAME). The runtime mounts it through the internal
    /// path — the reservation exists for EXTERNAL callers.
    pub fn new(name: impl Into<String>) -> Context {
        let mut ctx = Context {
            name: name.into(),
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            blobs: RamBlobStore::new(),
            telemetry: Vec::new(),
            dead_letters: Vec::new(),
            clock: Box::new(WallClock),
            counters: 0,
            next_scan: 0,
        };
        ctx.spawn_entry(
            Spawn::new(Some("ticker"), Box::new(|| Box::new(Ticker::new())))
                .supervision(Supervision::Restart),
        )
        .expect("the ticker is the first spawn; no collision possible");
        ctx
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_clock(&mut self, clock: Box<dyn ClockFace>) {
        self.clock = clock;
    }

    pub fn telemetry(&self) -> &[(Level, String)] {
        &self.telemetry
    }

    pub fn dead_letters(&self) -> &[Envelope] {
        &self.dead_letters
    }

    /// Blob-store size — diagnostics/garbage observation only. Reach-
    /// ability (what GC may collect) is a watermark question, M3.
    pub fn blob_count(&self) -> usize {
        self.blobs.count()
    }

    /// Spawn an actor. Naming is opt-in (§3.4): named = addressable,
    /// refuse-on-collision; unnamed = only the spawner's handle.
    /// `ticker` is runtime-reserved.
    pub fn spawn(&mut self, spec: Spawn) -> Result<Handle, SpawnError> {
        if spec.name.as_deref() == Some("ticker") {
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
        {
            let host = Host::new(&self.blobs, &spec.rows, self.clock.now_micros());
            actor.init(&host, &spec.init_spec);
        }
        self.entries.push(Entry {
            id: id.clone(),
            name: spec.name.clone(),
            actor: Some(actor),
            factory: spec.factory,
            supervision: spec.supervision,
            rows: spec.rows,
            mailbox: Mailbox::new(spec.mailbox_bound),
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
    /// Structural admission only (round 10: the core is schema-blind)
    /// with the coarse refusal vocabulary — never a finer word than
    /// `busy | unauthorized | unknown-target | invalid`.
    pub fn post(&mut self, to: &Address, letter: Letter, authority: Vec<String>) -> Verdict {
        let mut letter = letter;
        letter.authority = authority;
        if letter.validate().is_err() {
            return Verdict::Refused(RefuseReason::Invalid);
        }
        let Some(idx) = self.resolve(to) else {
            return Verdict::Refused(RefuseReason::UnknownTarget);
        };
        let envelope = Envelope::new(
            letter,
            0, // seq is minted at commit on the log tier; the RAM tier runs on the cursor
            Address::Instance("external".into()),
            to.clone(),
            None,
            "external",
        )
        .expect("letter validated above");
        let bound = self.entries[idx].mailbox.bound;
        if self.entries[idx].mailbox.queue.len() >= bound {
            return Verdict::Refused(RefuseReason::Busy);
        }
        self.entries[idx].mailbox.queue.push_back(envelope);
        Verdict::Accepted
    }

    /// Deliver ONE message, round-robin from `next_scan` —
    /// deterministic AND fair: a perpetually-busy actor cannot starve
    /// its neighbors. Returns false when nothing was deliverable.
    fn deliver_one(&mut self) -> bool {
        let n = self.entries.len();
        if n == 0 {
            return false;
        }
        for k in 0..n {
            let idx = (self.next_scan + k) % n;
            let entry = &self.entries[idx];
            if entry.actor.is_none() && !entry.mailbox.queue.is_empty() {
                // Terminal: fail-fast is loud — never silently holding
                // mail. Drain to dead letters.
                let drained: Vec<Envelope> = {
                    let entry = &mut self.entries[idx];
                    let mut out = Vec::new();
                    while let Some(env) = entry.mailbox.queue.pop_front() {
                        entry.mailbox.cursor += 1;
                        out.push(env);
                    }
                    out
                };
                self.dead_letters.extend(drained);
                self.next_scan = (idx + 1) % n;
                return true;
            }
            if entry.actor.is_some() && !entry.mailbox.queue.is_empty() {
                let env = entry.mailbox.queue.front().cloned().expect("non-empty");
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
                blobs,
                entries,
                clock,
                ..
            } = self;
            let entry = &mut entries[idx];
            let host = Host::new(blobs, &entry.rows, clock.now_micros());
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

        // Phase 2 — validate ALL effects before applying ANY
        // (fail-before-compute, §3.5). Projected capacity counts this
        // list's own contributions.
        enum Block {
            Busy,
            Strike(&'static str),
        }
        let mut block: Option<Block> = None;
        let mut projected: HashMap<usize, usize> = HashMap::new();
        for eff in &effects {
            match eff {
                Effect::Post { to, letter } => {
                    if letter.validate().is_err() {
                        block = Some(Block::Strike("invalid letter in effects"));
                        break;
                    }
                    let Some(tidx) = self.resolve(to) else {
                        block = Some(Block::Strike("unknown target in effects"));
                        break;
                    };
                    let mailbox = &self.entries[tidx].mailbox;
                    let inflight = projected.entry(tidx).or_insert(0);
                    if mailbox.queue.len() + *inflight >= mailbox.bound {
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

        // Phase 3 — the two refusal classes (§3.5 / round 07).
        match block {
            // Temporary: requeue at tail, NO attempt burned — overload
            // self-heals; the actor re-receives and re-declares.
            Some(Block::Busy) => {
                let entry = &mut self.entries[idx];
                let env = entry.mailbox.queue.pop_front().expect("front exists");
                entry.mailbox.queue.push_back(env);
                self.telemetry.push((
                    Level::Debug,
                    format!("{}: target busy — requeued, no attempt burned", entry.id),
                ));
                return true;
            }
            // Permanent: burn an attempt; N strikes → the dead-letter
            // path (the policy-authorized cursor advance, §3.1).
            Some(Block::Strike(why)) => {
                let entry = &mut self.entries[idx];
                entry.mailbox.attempts += 1;
                let attempts = entry.mailbox.attempts;
                self.telemetry.push((
                    Level::Warn,
                    format!(
                        "{}: effects refused ({why}) — attempt {attempts}",
                        entry.id
                    ),
                ));
                if attempts >= MAX_ATTEMPTS {
                    let env = entry.mailbox.queue.pop_front().expect("front exists");
                    entry.mailbox.cursor += 1;
                    entry.mailbox.attempts = 0;
                    self.dead_letters.push(env);
                }
                return true;
            }
            None => {}
        }

        // Phase 4 — apply in order; the cursor advance rides the same
        // step (§3.1: log owns acks — on RAM tier, validate→apply
        // without early exits IS the atomic commit).
        let actor_id = self.entries[idx].id.clone();
        for eff in &effects {
            match eff {
            Effect::Post { to, letter } => {
                let tidx = self.resolve(to).expect("validated above");
                let envelope = Envelope::new(
                    letter.clone(),
                    0,
                    Address::Instance(actor_id.clone()),
                    to.clone(),
                    None,
                    actor_id.clone(),
                )
                .expect("validated above");
                self.entries[tidx].mailbox.queue.push_back(envelope);
            }
            Effect::Mint { verb, object, .. } => {
                self.entries[idx].rows.push(CapRow::new(verb, object));
            }
                Effect::Emit { level, message } => {
                    self.telemetry.push((*level, message.clone()));
                }
            }
        }
        let entry = &mut self.entries[idx];
        entry.mailbox.queue.pop_front();
        entry.mailbox.cursor += 1;
        entry.mailbox.attempts = 0;
        true
    }

    fn handle_crash(&mut self, idx: usize) {
        self.entries[idx].mailbox.attempts += 1;
        let supervision = self.entries[idx].supervision;
        let attempts = self.entries[idx].mailbox.attempts;
        let id = self.entries[idx].id.clone();
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
                    let spec: Vec<u8> = Vec::new();
                    let host = Host::new(&self.blobs, &entry.rows, self.clock.now_micros());
                    entry.actor.as_mut().expect("fresh from factory").init(&host, &spec);
                }
                self.telemetry.push((
                    Level::Warn,
                    format!("{id}: crashed — restarted fresh, attempt {attempts}"),
                ));
                if attempts >= MAX_ATTEMPTS {
                    let env = self.entries[idx].mailbox.queue.pop_front().expect("front exists");
                    self.entries[idx].mailbox.cursor += 1;
                    self.entries[idx].mailbox.attempts = 0;
                    self.dead_letters.push(env);
                }
            }
        }
    }

    /// Drive the context until no message is deliverable. `max_passes`
    /// bounds livelock (a permanent busy-loop is a bug, and the bound
    /// makes it a visible one).
    pub fn run_until_idle(&mut self, max_passes: usize) -> PumpStats {
        let mut stats = PumpStats::default();
        for _ in 0..max_passes {
            // Poke poll-affine actors while they hold armed work —
            // timers fire as messages, never as callbacks (§6).
            let pollers: Vec<usize> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.actor.as_ref().is_some_and(|a| a.wants_poll()))
                .map(|(i, _)| i)
                .collect();
            for idx in pollers {
                let poll =
                    Letter::new("mwydyn.timer.poll@1", Vec::new(), None, None, Vec::new())
                        .expect("fixed kind");
                let envelope = Envelope::new(
                    poll,
                    0,
                    Address::Instance("external".into()),
                    Address::Instance(self.entries[idx].id.clone()),
                    None,
                    "external",
                )
                .expect("fixed letter");
                self.entries[idx].mailbox.queue.push_back(envelope);
            }
            if self.deliver_one() {
                stats.delivered += 1;
            } else {
                break;
            }
        }
        stats
    }
}
