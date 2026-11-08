//! The journal face (spec §8) and its first implementation.
//!
//! §8's law: the contract is invariants + a provider admission list.
//! `JournalFace` is that contract in type form; `InMemoryJournal`
//! proves the face is not SQLite-shaped (the SQLite provider lands
//! behind a cargo feature, next).
//!
//! Identity: `(context, lineage, seq)`. Forks are journaled FACTS
//! (round 19); abandoned tails are GHOST branches — read-only,
//! never delivered, readable by whoever holds `ghost.read`.

use mwydyn_data::{Address, Envelope, Letter};
use std::collections::HashMap;

use crate::context::ClockFace;

// ------------------------------------------------------------------ types

/// A pending append — the journal mints identity (lineage, seq),
/// commitTs, and checksum at commit (A1 Q9/Q22 recovered).
#[derive(Debug, Clone)]
pub struct Pending {
    pub letter: Letter,
    pub from: Address,
    /// The delivering actor's id (pending-index key); empty string
    /// for letters no actor delivers (the external outbox).
    pub to_actor: String,
    pub to: Address,
    /// RUNTIME-STAMPED at commit (round 26/29): the seq of the letter
    /// the sender was processing — the snapshot `covers` position.
    /// `None` for embedding/pump posts.
    pub ack: Option<u64>,
    pub producer: String,
}

/// One journal transaction (§8.2): the atomic unit is the WHOLE set —
/// appends, cursor advances, facts — all-or-nothing at the durability
/// point. External providers map this to ONE atomic record.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // transactions are built one-op-at-a-time; boxing would add indirection to the hot path
pub enum JournalOp {
    Append(Pending),
    /// Log owns acks (§3.1): the cursor advance rides the same
    /// transaction as the effects that earned it. `position` is the
    /// acknowledged letter's seq on the active lineage.
    Ack { actor: String, position: u64 },
    /// The policy-authorized cursor advance past a poison letter
    /// (§3.1) — journaled as a fact, rewind-consistent.
    DeadLetter { actor: String, why: String },
    /// Rewind: a new lineage from the active lineage's `parent_seq`.
    /// Shared prefix is logical (no copying); the abandoned tail
    /// becomes a ghost branch (§8.3).
    Fork { parent_seq: u64 },
    /// Grant-set mutation fact (§3.3 tier rule: log-tier actors
    /// journal their mints).
    Mint { actor: String, verb: String, object: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub lineage: u64,
    pub first_seq: u64,
    pub count: usize,
    pub commit_ts: i64,
    /// The stamped envelopes for this transaction's APPENDS — identity,
    /// timestamps, checksums filled. Callers that hold refs (the
    /// external outbox view) take them from here.
    pub appended: Vec<Envelope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageInfo {
    pub id: u64,
    /// (parent lineage, fork point) — None for the root.
    pub parent: Option<(u64, u64)>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    UnknownContext,
    UnknownLineage,
    /// Position beyond a lineage's known history (the shared prefix
    /// delegates to parents; beyond everything is an error, never a
    /// silent gap).
    BeyondHistory,
    BadRequest(&'static str),
    /// A provider-side storage failure (SQL, IO, poisoning).
    Storage(String),
    /// Stored bytes no longer verify — bit rot, tampering, or a
    /// broken provider (§8.4: loud, never silent divergence).
    Integrity(&'static str),
}

/// A journaled grant-set mutation (§3.3 tier rule: log-tier actors
/// journal their mints; hydration reconstructs grant sets from them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintFact {
    pub actor: String,
    pub verb: String,
    pub object: String,
}

/// The face (§8). Providers MUST: atomic batch commit at a durable
/// point; contiguous per-lineage sequence; append-only, no retention
/// loss; ordered byte-identical replay; serialized writes per
/// context (v0: single-writer).
///
/// `Send + Sync` (round 21, multithread-readiness): transitions will
/// run on workers while commits serialize — the face must cross
/// threads. External providers (NATS/Kafka clients) are threaded
/// anyway; a provider that cannot cross threads is not a real
/// provider.
pub trait JournalFace: Send + Sync {
    fn commit(&mut self, context: &str, ops: Vec<JournalOp>) -> Result<Commit, JournalError>;
    /// Ordered replay of a lineage range, INCLUSIVE bounds, verified.
    /// Works on any lineage — live or ghost (§8.3).
    fn read(
        &self,
        context: &str,
        lineage: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<Envelope>, JournalError>;
    /// Undelivered letters for an actor on the ACTIVE lineage, in seq
    /// order (the mailbox IS a cursor over the journal, §2/§3.1).
    fn pending(&self, context: &str, actor: &str) -> Result<Vec<Envelope>, JournalError>;
    fn pending_count(&self, context: &str, actor: &str) -> Result<usize, JournalError>;
    /// The actor's cursor ON THE GIVEN LINEAGE (cursors are per-
    /// lineage; forks seed them clamped to the fork point).
    fn cursor(&self, context: &str, lineage: u64, actor: &str) -> Result<Option<u64>, JournalError>;
    fn active_lineage(&self, context: &str) -> Result<u64, JournalError>;
    fn lineages(&self, context: &str) -> Result<Vec<LineageInfo>, JournalError>;
    /// A lineage's total covered range end — the highest seq it can
    /// serve, counting the shared prefix delegated to ancestors.
    /// Ghost reads and GC reachability scans both need this.
    fn covered(&self, context: &str, lineage: u64) -> Result<u64, JournalError>;
    /// All letters addressed to `actor` with seq in `1..=up_to`, in
    /// seq order — delivered AND undelivered. THE HYDRATION READ
    /// (round 26): replay `1..=cursor` through a fresh instance with
    /// effects discarded, and actor state is exactly as of the ack
    /// boundary. Ghost letters are excluded (active lineage only).
    fn letters_for(
        &self,
        context: &str,
        actor: &str,
        up_to: u64,
    ) -> Result<Vec<Envelope>, JournalError>;
    /// Mint facts for an actor (§3.3 tier rule) — hydration
    /// reconstructs grant sets from these, on top of spawn rows.
    fn mints(&self, context: &str, actor: &str) -> Result<Vec<MintFact>, JournalError>;
    /// Ops committed but not yet synced to a durability target.
    /// The hot face returns them; a durability face returns empty.
    /// (Round 31: pipelining overlay drain.)
    fn unsynced_ops(&self) -> &[(String, JournalOp)] {
        &[]
    }
    /// Remove the first `n` ops (post-flush truncate). Round 31.
    fn take_ops(&mut self, _n: usize) -> Vec<(String, JournalOp)> {
        Vec::new()
    }
}

// ------------------------------------------------------ in-memory impl

#[derive(Debug, Clone)]
struct Lineage {
    /// (parent lineage id, fork point seq) — None for the root.
    parent: Option<(u64, u64)>,
}

/// Local entries only: (lineage, seq) → envelope. Forked prefixes are
/// NOT copied — reads delegate to the parent (storage dedup for free,
/// and the ghost branch costs nothing until it grows its own tail).
struct CtxJournal {
    lineages: Vec<Lineage>, // lineage id = index + 1
    active: u64,
    entries: HashMap<(u64, u64), Envelope>,
    local_max: HashMap<u64, u64>,
    cursors: HashMap<(u64, String), u64>,
    /// (lineage, actor) → undelivered seqs, ascending.
    pending: HashMap<(u64, String), Vec<u64>>,
    /// Journaled mint facts: (actor, verb, object) — hydration's
    /// grant-set reconstruction reads these (round 26).
    mints: Vec<MintFact>,
}

impl CtxJournal {
    fn new() -> CtxJournal {
        CtxJournal {
            lineages: vec![Lineage { parent: None }],
            active: 1,
            entries: HashMap::new(),
            local_max: HashMap::new(),
            cursors: HashMap::new(),
            pending: HashMap::new(),
            mints: Vec::new(),
        }
    }

    fn next_seq(&self, lineage: u64) -> u64 {
        let local = self.local_max.get(&lineage).copied().unwrap_or(0);
        match &self.lineages[(lineage - 1) as usize].parent {
            Some((_, fork_at)) => (*fork_at).max(local) + 1,
            None => local + 1,
        }
    }

    /// Where does (lineage, seq) physically live? Forked prefixes
    /// delegate to parents; beyond everything is an error.
    fn owner(&self, mut lineage: u64, seq: u64) -> Result<(u64, u64), JournalError> {
        loop {
            if lineage == 0 || lineage > self.lineages.len() as u64 {
                return Err(JournalError::UnknownLineage);
            }
            match self.lineages[(lineage - 1) as usize].parent {
                Some((parent, fork_at)) if seq <= fork_at => lineage = parent,
                _ => {
                    return if seq >= 1 && seq < self.next_seq(lineage) {
                        Ok((lineage, seq))
                    } else {
                        Err(JournalError::BeyondHistory)
                    }
                }
            }
        }
    }

    fn stamp(
        &self,
        p: &Pending,
        _lineage: u64,
        seq: u64,
        commit_ts: i64,
    ) -> Result<Envelope, JournalError> {
        Envelope::new(
            p.letter.clone(),
            seq,
            p.from.clone(),
            p.to.clone(),
            p.ack,
            Some(commit_ts),
            p.producer.clone(),
        )
        .map_err(|_| JournalError::BadRequest("invalid letter in append"))
    }
}

/// The first provider. In-memory, single-writer, trivially durable
/// (commit IS the durability point) — the substrate-independence
/// witness (the SQLite provider must satisfy the same tests).
pub struct InMemoryJournal {
    clock: Box<dyn ClockFace>,
    ctxs: HashMap<String, CtxJournal>,
    /// Op history since the last durability sync — the pipelining
    /// overlay drained by the DurabilityDriver (round 31).
    op_log: Vec<(String, JournalOp)>,
}

impl InMemoryJournal {
    pub fn new(clock: Box<dyn ClockFace>) -> InMemoryJournal {
        InMemoryJournal {
            clock,
            ctxs: HashMap::new(),
            op_log: Vec::new(),
        }
    }

    /// Ops committed but not yet synced to the durable tier.
    pub fn unsynced_ops(&self) -> &[(String, JournalOp)] {
        &self.op_log
    }

    /// Remove the first `n` ops (the durability driver's post-flush
    /// truncate).
    pub fn take_ops(&mut self, n: usize) -> Vec<(String, JournalOp)> {
        self.op_log.drain(..n.min(self.op_log.len())).collect()
    }
}

impl JournalFace for InMemoryJournal {
    fn commit(&mut self, context: &str, ops: Vec<JournalOp>) -> Result<Commit, JournalError> {
        let now = self.clock.now_micros();
        let ctx = self.ctxs.entry(context.to_string()).or_insert_with(CtxJournal::new);
        let mut first_seq: Option<u64> = None;
        let mut count = 0usize;
        let commit_ts = now;
        let mut appended: Vec<Envelope> = Vec::new();
        for op in ops {
            self.op_log.push((context.to_string(), op.clone()));
            match op {
                JournalOp::Append(p) => {
                    let lineage = ctx.active;
                    let seq = ctx.next_seq(lineage);
                    let mut env = ctx.stamp(&p, lineage, seq, now)?;
                    env.ack = p.ack;
                    appended.push(env.clone());
                    ctx.entries.insert((lineage, seq), env);
                    ctx.local_max.insert(lineage, seq);
                    if !p.to_actor.is_empty() {
                        ctx.pending
                            .entry((lineage, p.to_actor.clone()))
                            .or_default()
                            .push(seq);
                    }
                    first_seq.get_or_insert(seq);
                    count += 1;
                }
                JournalOp::Ack { actor, position } => {
                    let lineage = ctx.active;
                    ctx.cursors.insert((lineage, actor.clone()), position);
                    // Drop everything ≤ position from the pending index.
                    if let Some(seqs) = ctx.pending.get_mut(&(lineage, actor.clone())) {
                        while seqs.first().is_some_and(|s| *s <= position) {
                            seqs.remove(0);
                        }
                    }
                }
                JournalOp::DeadLetter { actor, .. } => {
                    // The cursor advance rides the Ack the runtime
                    // pairs with this fact; the fact itself is the
                    // audit trail (§3.1: journaled, rewind-consistent).
                    let _ = actor;
                }
                JournalOp::Fork { parent_seq } => {
                    let parent = ctx.active;
                    let new_id = (ctx.lineages.len() + 1) as u64;
                    // Seed the new lineage: cursors clamp to the fork
                    // point; pending letters within the shared prefix
                    // carry over (seq numbering continues through it).
                    let clamped: Vec<((u64, String), u64)> = ctx
                        .cursors
                        .iter()
                        .filter(|((lin, _), _)| *lin == parent)
                        .map(|((_, actor), c)| ((new_id, actor.clone()), (*c).min(parent_seq)))
                        .collect();
                    let carried: Vec<((u64, String), Vec<u64>)> = ctx
                        .pending
                        .iter()
                        .filter(|((lin, _), seqs)| {
                            *lin == parent && seqs.first().is_some_and(|s| *s <= parent_seq)
                        })
                        .map(|((_, actor), seqs)| {
                            (
                                (new_id, actor.clone()),
                                seqs.iter().copied().filter(|s| *s <= parent_seq).collect(),
                            )
                        })
                        .collect();
                    ctx.lineages.push(Lineage {
                        parent: Some((parent, parent_seq)),
                    });
                    ctx.active = new_id;
                    for (k, v) in clamped {
                        ctx.cursors.insert(k, v);
                    }
                    for (k, v) in carried {
                        ctx.pending.insert(k, v);
                    }
                }
                JournalOp::Mint { actor, verb, object } => {
                    // Journaled for replay (§3.3 tier rule); row
                    // reconstruction is hydration (round 26).
                    ctx.mints.push(MintFact {
                        actor: actor.clone(),
                        verb: verb.clone(),
                        object: object.clone(),
                    });
                }
            }
        }
        Ok(Commit {
            lineage: ctx.active,
            first_seq: first_seq.unwrap_or(0),
            count,
            commit_ts,
            appended,
        })
    }

    fn read(
        &self,
        context: &str,
        lineage: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<Envelope>, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        let mut out = Vec::new();
        for seq in from..=to {
            let (lin, s) = ctx.owner(lineage, seq)?;
            out.push(
                ctx.entries
                    .get(&(lin, s))
                    .cloned()
                    .ok_or(JournalError::BeyondHistory)?,
            );
        }
        Ok(out)
    }

    fn pending(&self, context: &str, actor: &str) -> Result<Vec<Envelope>, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        let active = ctx.active;
        let seqs = ctx
            .pending
            .get(&(active, actor.to_string()))
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for seq in seqs {
            let (lin, s) = ctx.owner(active, seq)?;
            out.push(
                ctx.entries
                    .get(&(lin, s))
                    .cloned()
                    .ok_or(JournalError::BeyondHistory)?,
            );
        }
        Ok(out)
    }

    fn pending_count(&self, context: &str, actor: &str) -> Result<usize, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        Ok(ctx
            .pending
            .get(&(ctx.active, actor.to_string()))
            .map(|v| v.len())
            .unwrap_or(0))
    }

    fn cursor(
        &self,
        context: &str,
        lineage: u64,
        actor: &str,
    ) -> Result<Option<u64>, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        Ok(ctx.cursors.get(&(lineage, actor.to_string())).copied())
    }

    fn active_lineage(&self, context: &str) -> Result<u64, JournalError> {
        self.ctxs
            .get(context)
            .map(|c| c.active)
            .ok_or(JournalError::UnknownContext)
    }

    fn lineages(&self, context: &str) -> Result<Vec<LineageInfo>, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        Ok(ctx
            .lineages
            .iter()
            .enumerate()
            .map(|(i, l)| LineageInfo {
                id: (i + 1) as u64,
                parent: l.parent,
                active: (i + 1) as u64 == ctx.active,
            })
            .collect())
    }

    fn covered(&self, context: &str, lineage: u64) -> Result<u64, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        Ok(ctx.next_seq(lineage).saturating_sub(1))
    }

    fn letters_for(
        &self,
        context: &str,
        actor: &str,
        up_to: u64,
    ) -> Result<Vec<Envelope>, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        let target = Address::Instance(actor.to_string());
        let covered = ctx.next_seq(ctx.active).saturating_sub(1);
        let mut out = Vec::new();
        for seq in 1..=up_to.min(covered) {
            let Ok((lin, s)) = ctx.owner(ctx.active, seq) else {
                continue;
            };
            if let Some(env) = ctx.entries.get(&(lin, s)) {
                if env.to == target {
                    out.push(env.clone());
                }
            }
        }
        Ok(out)
    }

    fn mints(&self, context: &str, actor: &str) -> Result<Vec<MintFact>, JournalError> {
        let Some(ctx) = self.ctxs.get(context) else {
            return Err(JournalError::UnknownContext);
        };
        Ok(ctx
            .mints
            .iter()
            .filter(|m| m.actor == actor)
            .cloned()
            .collect())
    }
}
