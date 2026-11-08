//! mwydyn-core — the runtime library (M0, RAM tier).
//!
//! The walking skeleton: contexts, mailboxes with cursor (§3.1),
//! structural admission (§3.5), the effects engine with
//! validate-all-then-apply-all as the atomic in-proc commit,
//! supervision policies (§3.2), the ticker behind an injected clock
//! (§6 clock obliviousness), the RAM blob store (§4), and native
//! Rust actors over the `receive → effects` trait — the same shape
//! WASM guests implement (compose-parity, round 11).

pub mod actor;
pub mod blob;
pub mod context;
pub mod ticker;

pub use actor::{Actor, CapRow, Effect, Host, Level, MsgCtx};
pub use blob::{BlobAccess, BlobRef, RamBlobStore};
pub use context::{
    ClockFace, Context, Handle, ManualClock, PumpStats, RefuseReason, Spawn, SpawnError,
    Supervision, Verdict, WallClock,
};
pub use ticker::Ticker;

