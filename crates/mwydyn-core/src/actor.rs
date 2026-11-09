//! The actor contract, native form (spec §3, compose-parity round 11):
//! `receive` is a pure transition over (state, letter, host reads) →
//! effects. Writing is declaring — there is no post/put/mint/emit
//! import, only reads (§3.5).

use crate::blob::{BlobAccess, BlobRef};
use mwydyn_data::{Address, Envelope, Letter};

/// Telemetry levels (§3.2: lossy by design, outside the transaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Effects declared by `receive`, applied by the runtime in order
/// after validate-all (§3.5). ONLY the observable things — `put` is
/// not here: allocation is not an effect (round 16), it happens
/// eagerly on the ambient face with the ref in hand.
#[derive(Debug, Clone)]
pub enum Effect {
    Post {
        to: mwydyn_data::Address,
        letter: Letter,
    },
    /// Attenuation-only, holder-only (§3.3). M0's algebra is the
    /// trivial one — exact source-row match; per-service algebras are
    /// vector-tested when services define them.
    Mint {
        source_verb: String,
        source_object: String,
        verb: String,
        object: String,
    },
    Emit {
        level: Level,
        message: String,
    },
}

/// Grant row `{verb, object}` (spec §3.3). Checked per-use against
/// the live set — never inferred from anything else (A1 CTR-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapRow {
    pub verb: String,
    pub object: String,
}

impl CapRow {
    pub fn new(verb: &str, object: &str) -> CapRow {
        CapRow {
            verb: verb.into(),
            object: object.into(),
        }
    }

    fn holds(&self, verb: &str, object: &str) -> bool {
        self.verb == verb && (self.object == "*" || self.object == object)
    }
}

/// The read face receive sees — the native mirror of the frozen
/// world's pure-read imports (spec §5): blob reads + caps list +
/// the ticker's clock read + the GHOST gate (§8.3). Nothing here can
/// have observable effects.
pub struct Host<'a> {
    blobs: &'a dyn crate::blob::BlobFace,
    journal: &'a dyn crate::journal::JournalRead,
    context: &'a str,
    caps: &'a [CapRow],
    now_micros: i64,
}

impl<'a> Host<'a> {
    pub(crate) fn new(
        blobs: &'a dyn crate::blob::BlobFace,
        journal: &'a dyn crate::journal::JournalRead,
        context: &'a str,
        caps: &'a [CapRow],
        now_micros: i64,
    ) -> Host<'a> {
        Host {
            blobs,
            journal,
            context,
            caps,
            now_micros,
        }
    }

    /// Eager ambient allocation (round 16): the ref is in hand the
    /// moment this returns — embed it in posts, save it in state, or
    /// drop it (unreferenced garbage; GC collects, M3). Requires a
    /// live `blob.write` row. NOT an effect: nothing here is
    /// observable to other actors until a committed reference forms.
    pub fn blob_put(
        &self,
        bytes: &[u8],
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, BlobAccess> {
        if !self.caps.iter().any(|row| row.holds("blob.write", "*")) {
            return Err(BlobAccess::Unauthorized);
        }
        self.blobs
            .put(bytes, media_type, ttl_micros)
            .map_err(|_| BlobAccess::Integrity)
    }

    /// Repeatable by contract (§3.5): content-addressed answers; a
    /// swept ref is a loud error, never changed data. Requires a
    /// live `blob.read` row.
    pub fn blob_get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess> {
        if !self.caps.iter().any(|row| row.holds("blob.read", "*")) {
            return Err(BlobAccess::Unauthorized);
        }
        match self.blobs.get(r) {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(e),
        }
    }

    pub fn blob_len(&self, r: &BlobRef) -> Result<u64, BlobAccess> {
        if !self.caps.iter().any(|row| row.holds("blob.read", "*")) {
            return Err(BlobAccess::Unauthorized);
        }
        self.blobs.len(r)
    }

    /// Range read WITHOUT materializing the blob (round 28: the
    /// provider seeks; no full fetch, no per-read verification).
    pub fn blob_get_range(
        &self,
        r: &BlobRef,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, BlobAccess> {
        if !self.caps.iter().any(|row| row.holds("blob.read", "*")) {
            return Err(BlobAccess::Unauthorized);
        }
        self.blobs.get_range(r, offset, len)
    }

    /// THE GHOST GATE (§8.3, round 19): read any lineage's range —
    /// live or ghost ("ghosts of an alternate reality": the abandoned
    /// tail of a rewind, readable, never effect-creating). Requires a
    /// live `ghost.read` row; per-lineage attenuation is the object
    /// grammar (`ghost.read:*` or `ghost.read:<lineage-id>`).
    /// Default-off everywhere — the owner's "solid gate now": the
    /// granted path is TESTED even though no consumer flips it yet.
    pub fn ghost_read(
        &self,
        lineage: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<Envelope>, BlobAccess> {
        let held = self
            .caps
            .iter()
            .any(|row| row.holds("ghost.read", &lineage.to_string()));
        if !held {
            return Err(BlobAccess::Unauthorized);
        }
        self.journal
            .read(self.context, lineage, from, to)
            .map_err(|_| BlobAccess::Missing)
    }

    /// The LIVE grant set — introspection, never authority (§3.3).
    pub fn caps(&self) -> &[CapRow] {
        self.caps
    }

    /// Clock reads exist on this face for the TICKER only (§6: the
    /// ticker is the only wall-clock reader). Other actors calling
    /// this is a design smell the review should catch — the data
    /// model has no clock type to branch on.
    pub fn now_micros(&self) -> i64 {
        self.now_micros
    }
}

/// Runtime-authored, read-only per-delivery context — the envelope's
/// runtime-assigned fields, VISIBLE but never SETTABLE by the guest
/// (the byzantine posture, §2: what the guest authored rides the
/// letter; what the runtime minted rides here).
pub struct MsgCtx {
    /// Sender instance id — replies target it; there is no reply-to
    /// field (owner's law, round 09).
    pub from: Address,
}

/// `Send` (round 21, multithread-readiness): actor state MOVES —
/// between the scheduler and transition workers once the executor
/// lands. This forbids `Rc`/non-Send state in actors *by contract*,
/// early, while only toy actors exist to fix.
pub trait Actor: Send {
    /// Boot (§7.6, round 29): identity, parent handle, durable state
    /// ref, config. Called once at spawn/reopen — the RESTORE point.
    fn init(&mut self, _host: &Host, _boot: &crate::context::BootRecord) {}

    /// Called ONCE per inbox message, sequentially — the ABI itself
    /// is the single-cursor invariant made mechanical.
    /// Long computations are fuel/epoch-bounded in the WASM tier
    /// (§6); native actors are trusted to return.
    fn receive(&mut self, ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect>;

    /// Poll affinity: only the ticker sets this (the pump pokes poll
    /// letters while timers are armed — timers fire as messages, §6).
    fn wants_poll(&self) -> bool {
        false
    }

    /// Transactional receive support (round 32): fork the current
    /// state before receive; on refusal, restore the fork. This is
    /// the shadow-copy that makes busy-refusal + strike state-safe
    /// for stateful actors.
    fn fork(&self) -> Box<dyn Actor>;

    /// ROUND 37 — does the fork carry the actor's live state (so a
    /// refusal must restore it)? Default true. Actors that DECLARE
    /// themselves stateless may return false: on refusal the runtime
    /// then keeps the mutated instance ("just reused" — dirty state
    /// across refusals is the author's declared problem, and the
    /// restore copy is not even materialized).
    fn fork_carries_state(&self) -> bool {
        true
    }

    /// §7.6 SAVE (round 39): the actor's durable state for a
    /// runtime-driven snapshot (`Context::snapshot_actor` stores it
    /// and lands the covers-stamped letter). `None` = the actor has
    /// nothing to say (the default; WASM guests answer when they
    /// export the `snapshot` ABI hook).
    fn snapshot_state(&mut self) -> Option<Vec<u8>> {
        None
    }
}
