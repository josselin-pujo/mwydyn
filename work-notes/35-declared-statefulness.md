# 35 — Declared statefulness: the persistent WASM instance

> Status: implemented and green (2026-11-09). 22 suites ok, clippy
> clean.

## 1. The owner's diagnosis (all three points confirmed)

"Am I correct in assuming that you are instantiating the WASM
instance for every letter instead of reusing the existing instance?"

YES. `fork()` did `Store::new` + `Instance::new` per delivery (the
rollback copy), serial AND pool. The measured 120µs "fork" is
instantiation — memory alloc/init dominating; verification is NOT
repeated (the module compiles once, wasmtime instantiates from the
compiled artifact). The full barrier exchange (call + CBOR + four
memory crossings) is 71µs — execution is a slice of that. And the
deeper flaw: fresh instance per letter = fresh memory per letter —
**a stateful WASM actor was impossible by construction**.

## 2. The lifecycle (round 37)

- The instance is **lazy and persistent**: created on first use,
  reused across letters. `fork()` returns a cheap handle (no
  instantiation) — per-letter WASM cost drops from ~191µs
  (fork 120 + exchange 71) to ~71µs.
- Instantiation happens on actual refusal/crash paths — per-refusal,
  never per-letter.

## 3. THE DECLARED GUARANTEE (owner-ratified)

The transactional guarantee follows the guest's DECLARATION, not the
runtime's guess:

- **`GuestMode::Stateful`** — pre-receive MEMORY IMAGE rollback:
  `fork` captures linear memory before the transition; on refusal a
  fresh instance is grown to fit and the image written back. Exactly-
  once internal state; doubled footprint while the image lives
  (owner: "I'd go with the double-memory-footprint, but SOLELY for
  actors that declare statefulness").
- **`GuestMode::Stateless`** — "just reused": the mutated instance
  STAYS on refusal (the fork is dropped, not restored). No once-and-
  only-once internal-state guarantee — "it's on them to not have
  internal state that fucks things up". The declaration rides to the
  core as `Actor::fork_carries_state()` (default true; stateless
  guests override false) — both pumps branch on it.

Restart-on-refusal + replay was considered and rejected as the
default (owner): "potentially cheaper but too variable a cost to
always be so". No stack closure exists across exchanges (sync
receive completes before effects classification), so a linear-memory
image is sufficient for the rollback state.

## 4. The mechanics that matter (bugs found on the way)

- **The image must be KEPT, not consumed.** First cut: the
  placeholder applied its image at first use — attempt 2's fork then
  captured the FAILED attempt's mutations. Fix: apply a CLONE, pass
  the original down the strike chain; every re-fork restores the
  SAME pre-letter state.
- **`fork(&self)` cannot capture mutable globals** (wasmtime's
  global read needs a mutable store context). Documented contract:
  stateful guests keep state in LINEAR MEMORY (restored byte-exact);
  mutable globals revert to initial values on a restore. Linear
  memory IS the guest's state; the image grows the fresh instance to
  fit before writing.
- The declaration test pair uses ONE guest artifact in BOTH modes:
  three strikes against the same letter, then an honest letter —
  stateful sees count=1 (exact rollback), stateless sees count=4
  (dirty residue, no rollback owed). The `mwydyn-guest-counter`
  crate exists to prove the declaration does something real.

## 5. Numbers (8 WASM guests, workers=4)

| | before (per-letter instantiate) | after (persistent + lazy fork) |
|---|---|---|
| serial | 2,671/s | **4,189/s** (+57%) |
| pool(4) | 3,799/s | **9,829/s (2.35× serial)** |
| wall per exchange | 248µs | **86µs** |
| committer per exchange | 148µs | 41.8µs |

The fork lever is dead. Remaining gap to the ~23.9k/s committer
bound is wave granularity (round 36's ledger shows it) — the next
throughput levers are pipelining depth and apply cost, not the WASM
tier.

## Provenance

Owner diagnosis + ratifications, 2026-11-09: "Instantiation happens
only on actual refusal/crash paths → that's what I wanted to read";
double footprint for declared-stateful only; stateless "just
reused".
