//! The entry points: what a guest exports and how it maps to the
//! [`Actor`] trait. One artifact, one actor — invoke exactly one
//! entry macro, at the crate root:
//!
//! - [`entry!`] — stateless (the default: nothing owed on refusal)
//! - [`stateful_entry!`] — strike rollback (memory-image, exactly-
//!   once internal state)
//! - [`durable_entry!`] — stateful PLUS §7.6 snapshots (requires
//!   [`Durable`], checked at compile time)
//!
//! Every macro emits the declaration export `mwydyn_decl() -> u32`
//! (spec §9.2: the GUEST owns its statefulness declaration — bit 0
//! set = stateful; the embedder cannot override it).

use mwydyn_data::barrier::BarrierRequest;
use mwydyn_data::{decode, encode, DecodeOptions, Letter, Value};

use crate::actor::{Actor, Cx, Durable};

/// The actor slot: one lazily-created instance, alive for the
/// artifact's lifetime. State lives in the actor struct — which IS
/// linear memory — so the runtime's memory-image rollback (stateful
/// declarations) covers it byte-exact, no serialization needed.
pub struct Slot<A: ?Sized> {
    actor: std::cell::UnsafeCell<Option<Box<A>>>,
}

// SAFETY: wasm32-unknown-unknown guests are single-threaded — there
// is no second thread to race with. Host-target builds (SDK tests)
// touch the slot from one test thread only. The barrier never
// re-enters: ambient calls do not call back into `process`.
unsafe impl<A: ?Sized> Sync for Slot<A> {}

impl<A: ?Sized> Default for Slot<A> {
    fn default() -> Self {
        Slot::new()
    }
}

impl<A: ?Sized> Slot<A> {
    /// The empty slot.
    pub const fn new() -> Self {
        Slot {
            actor: std::cell::UnsafeCell::new(None),
        }
    }

    /// Run `f` against the actor, creating it from `make` on first
    /// use.
    fn with<R>(&self, make: impl FnOnce() -> Box<A>, f: impl FnOnce(&mut A) -> R) -> R {
        let slot = unsafe { &mut *self.actor.get() };
        let actor = slot.get_or_insert_with(make);
        f(actor)
    }
}

/// THE BARRIER EXCHANGE (host calls `process`): decode the request,
/// run the actor, encode the effects. Every failure on this path is
/// a panic — a trap — a strike. Never a silent swallow.
///
/// # Safety
/// `req_ptr` must point to `req_len` readable bytes in guest linear
/// memory (the generated `process` export guarantees it).
pub unsafe fn process_impl<A: Actor + ?Sized>(
    slot: &Slot<A>,
    make: impl FnOnce() -> Box<A>,
    req_ptr: *const u8,
    req_len: usize,
) -> *mut u8 {
    // SAFETY: the host wrote `req_len` readable bytes at `req_ptr`.
    let bytes = unsafe { std::slice::from_raw_parts(req_ptr, req_len) };
    let value = decode(bytes, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("mwydyn-guest: barrier request decode: {e:?}"));
    let req = BarrierRequest::from_value(&value)
        .unwrap_or_else(|e| panic!("mwydyn-guest: barrier request shape: {e:?}"));
    let BarrierRequest { from, letter } = req;
    let effects = slot.with(make, |actor| {
        let cx = Cx::new(&from);
        actor.receive(letter, &cx)
    });
    let response = Value::Array(effects.iter().map(|e| e.to_value()).collect());
    crate::respond(&encode(&response))
}

/// §7.6 SAVE (host calls `snapshot` after instantiation): the
/// durable state as `[u32 le len][bytes]`.
pub fn snapshot_impl<A: Durable + ?Sized>(
    slot: &Slot<A>,
    make: impl FnOnce() -> Box<A>,
) -> *mut u8 {
    let state = slot.with(make, |actor| actor.snapshot());
    crate::respond(&state)
}

/// §7.6 RESTORE (host calls `restore` at boot, before any letter).
///
/// # Safety
/// `ptr` must point to `len` readable bytes in guest linear memory.
pub unsafe fn restore_impl<A: Durable + ?Sized>(
    slot: &Slot<A>,
    make: impl FnOnce() -> Box<A>,
    ptr: *const u8,
    len: usize,
) {
    // SAFETY: caller (the generated export) guarantees the slice.
    let state = unsafe { std::slice::from_raw_parts(ptr, len) };
    slot.with(make, |actor| actor.restore(state));
}

/// A letter payload for tests and host-side SDK users: canonical
/// CBOR or panic (the payload wall's own rule).
pub fn letter(kind: &str, payload: Value, correlation: Option<String>) -> Letter {
    Letter::new(kind, encode(&payload), None, correlation, vec![])
        .expect("mwydyn-guest: fixed kind and canonical payload")
}

/// Declare a STATELESS actor (round-37 default): the runtime owes
/// nothing on refusal — the instance is simply reused, mutated
/// state and all. Pick this when your actor keeps no state, or
/// state that is deliberately disposable.
///
/// ```ignore
/// mwydyn_guest::entry!(Echo::new());
/// ```
#[macro_export]
macro_rules! entry {
    ($make:expr) => {
        static MWYDYN_SLOT: $crate::Slot<dyn $crate::Actor> = $crate::Slot::new();
        $crate::mwydyn_exports_barrier!(MWYDYN_SLOT, dyn $crate::Actor, $make, 0);
    };
}

/// Declare a STATEFUL actor: refused transitions roll the guest's
/// linear memory back to pre-letter state — exactly-once internal
/// state. Keep state in the struct (linear memory); mutable globals
/// are NOT covered by the image.
///
/// ```ignore
/// mwydyn_guest::stateful_entry!(Cache::new());
/// ```
#[macro_export]
macro_rules! stateful_entry {
    ($make:expr) => {
        static MWYDYN_SLOT: $crate::Slot<dyn $crate::Actor> = $crate::Slot::new();
        $crate::mwydyn_exports_barrier!(MWYDYN_SLOT, dyn $crate::Actor, $make, 1);
    };
}

/// Declare a DURABLE actor: stateful (rollback) PLUS §7.6 boot
/// snapshots. `$ty` must implement [`Durable`] — this is enforced
/// by the slot's type at compile time; a vacuous snapshot cannot
/// build.
///
/// ```ignore
/// mwydyn_guest::durable_entry!(Counter { count: 0 });
/// ```
#[macro_export]
macro_rules! durable_entry {
    ($make:expr) => {
        static MWYDYN_SLOT: $crate::Slot<dyn $crate::Durable> = $crate::Slot::new();
        $crate::mwydyn_exports_barrier!(MWYDYN_SLOT, dyn $crate::Durable, $make, 1);
        $crate::mwydyn_exports_durable!(MWYDYN_SLOT, $make);
    };
}

/// The barrier exports every guest needs (round 33 + the §9.2
/// declaration). `&'static Slot<dyn A>` works for both `Actor` and
/// `Durable` slots: `dyn Durable` upcasts to the `receive` path.
///
/// THE PANIC WALL: wasm panics abort (the trap IS the strike); on
/// host targets a panic must not unwind through `extern "C"` — the
/// exports catch, let the panic hook speak, and abort inside the
/// boundary. Loud either way, never a swallow.
#[doc(hidden)]
#[macro_export]
macro_rules! mwydyn_exports_barrier {
    ($slot:ident, $trait:ty, $make:expr, $decl:expr) => {
        /// §9.2 — the guest's own statefulness declaration.
        #[no_mangle]
        pub extern "C" fn mwydyn_decl() -> u32 {
            $decl
        }

        #[no_mangle]
        pub extern "C" fn alloc(len: usize) -> *mut u8 {
            $crate::alloc(len)
        }

        /// # Safety
        /// See [`mwydyn_guest::dealloc`].
        #[no_mangle]
        pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
            unsafe { $crate::dealloc(ptr, len) }
        }

        /// # Safety
        /// `req_ptr` must point to `req_len` readable bytes in
        /// guest linear memory.
        #[no_mangle]
        pub unsafe extern "C" fn process(req_ptr: *const u8, req_len: usize) -> *mut u8 {
            unsafe {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    $crate::process_impl(&$slot, || Box::new($make) as Box<$trait>, req_ptr, req_len)
                })) {
                    Ok(ptr) => ptr,
                    Err(_) => std::process::abort(),
                }
            }
        }
    };
}

/// The §7.6 exports only durable guests carry.
#[doc(hidden)]
#[macro_export]
macro_rules! mwydyn_exports_durable {
    ($slot:ident, $make:expr) => {
        /// §7.6 SAVE — see [`mwydyn_guest::Durable::snapshot`].
        #[no_mangle]
        pub extern "C" fn snapshot() -> *mut u8 {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::snapshot_impl(&$slot, || Box::new($make) as Box<dyn $crate::Durable>)
            })) {
                Ok(ptr) => ptr,
                Err(_) => std::process::abort(),
            }
        }

        /// §7.6 RESTORE — see [`mwydyn_guest::Durable::restore`].
        ///
        /// # Safety
        /// `ptr` must point to `len` readable bytes in guest linear
        /// memory.
        #[no_mangle]
        pub unsafe extern "C" fn restore(ptr: *const u8, len: usize) {
            unsafe {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    $crate::restore_impl(
                        &$slot,
                        || Box::new($make) as Box<dyn $crate::Durable>,
                        ptr,
                        len,
                    )
                })) {
                    Ok(()) => {}
                    Err(_) => std::process::abort(),
                }
            }
        }
    };
}
