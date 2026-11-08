//! The actor contract, native form (spec §3, compose-parity round 11):
//! `receive` is a pure transition over (state, letter, host reads) →
//! effects. Writing is declaring — there is no post/put/mint/emit
//! import, only reads (§3.5).

use crate::blob::{BlobAccess, BlobRef, RamBlobStore};
use mwydyn_data::{Address, Letter};

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
/// after validate-all (§3.5).
#[derive(Debug, Clone)]
pub enum Effect {
    Post {
        to: mwydyn_data::Address,
        letter: Letter,
    },
    Put {
        bytes: Vec<u8>,
        media_type: String,
        ttl_micros: Option<u64>,
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
/// the ticker's clock read. Nothing here can have effects.
pub struct Host<'a> {
    blobs: &'a RamBlobStore,
    caps: &'a [CapRow],
    now_micros: i64,
}

impl<'a> Host<'a> {
    pub(crate) fn new(blobs: &'a RamBlobStore, caps: &'a [CapRow], now_micros: i64) -> Host<'a> {
        Host {
            blobs,
            caps,
            now_micros,
        }
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
        self.blob_get(r).map(|b| b.len() as u64)
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

pub trait Actor {
    /// Boot configuration kick (the `init` export of the world).
    fn init(&mut self, _host: &Host, _spec: &[u8]) {}

    /// Called ONCE per inbox message, sequentially — the ABI itself
    /// is the single-actor, single-cursor invariant made mechanical.
    /// Long computations are fuel/epoch-bounded in the WASM tier
    /// (§6); native actors are trusted to return.
    fn receive(&mut self, ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect>;

    /// Poll affinity: only the ticker sets this (the pump pokes poll
    /// letters while timers are armed — timers fire as messages, §6).
    fn wants_poll(&self) -> bool {
        false
    }
}
