# 27 — M3: blob persistence + reference-aware GC

> Status: implemented and green (2026-11-08). **M3 IS CLOSED** —
> blobs are durable (SQLite blob provider), and GC is the round-16
> law mechanized: eager allocations that never earned a journaled
> reference are collected; everything referenced by ANY lineage
> (active or ghost) survives. Gates: 16 walking + 5 face + 4
> envelope units + 2 corpora (72 cases) + 2 schema, clippy clean.

## 1. What landed

- **`BlobFace`** (§4 as a trait, mirroring the journal pattern):
  put/get/len/retain/count, `Send + Sync`, boxed-face passthrough.
  `RamBlobStore` implements it (RAM tier); **`SQLiteBlobStore`**
  (the durable tier, `sqlite` feature): content-addressed rows,
  idempotent-by-content puts (`ON CONFLICT DO NOTHING`), and
  VERIFY-ON-READ — a row that no longer hashes is a loud integrity
  failure (round 12's law, mechanically enforced).
- **`retain(keep)`** — the GC primitive on the face: remove every
  blob whose digest is not in the reachable set. The REACHABILITY
  computation lives in the runtime (it needs the journal), the
  COLLECTION lives in the face (it needs the storage) — the same
  split as journal commits.
- **`covered(context, lineage)`** on `JournalFace` — a lineage's
  total served range (shared prefix included); GC's reachability
  scan needs lineage extents.
- **`collect_blob_refs`** in mwydyn-data: recursive tag-1002 walk
  with the decoder's depth limit. DATA-MODEL work (blob refs are
  first-class primitives, §1), so GC payload scanning does NOT
  violate the schema-blind core law (round 10) — and it is a sweep,
  never the hot path (§1's envelope-only law intact).
- **`Context::gc()`** — reachability = ∪ tag-1002 digests across
  EVERY lineage's full range (active AND ghost — round 19: ghost
  branches are readable, so ghost-referenced blobs survive GC);
  then `blobs.retain(&referenced)`.
- **`Context::with_stores(name, journal, blobs)`** — the durable
  combo constructor; `with_journal` keeps RAM blobs by default.

## 2. The proof

- `gc_collects_unreferenced_allocations_and_keeps_journaled_ones`:
  two eager allocations, one posted (journaled ref), one dropped.
  GC collects exactly the dropped one; the survivor still verifies
  and reads (hash-bound law intact).
- `durable_blobs_survive_reopen_and_gc_respects_ghost_reachability`:
  eager put durable across reopen (SQLite blob store); a blob
  referenced ONLY by a ghost letter survives GC — round 19's
  "ghosts are readable" requires it, and the test proves the
  reachability set includes ghost lineages.

## 3. Findings

- The reachability decision (ghost lineages in or out) resolved
  itself from an earlier ratification: ghosts are readable, so
  ghost-referenced blobs are immortal. No new policy was needed —
  just noticing that round 19 had already decided it.
- `u64::MAX` as a read-range end was the in-memory-invisible bug
  class again (BeyondHistory on every SQLite pending call) — the
  provider-parameterized suite catches what single-impl tests
  structurally cannot.

## 4. Known limits

- Durable ACTOR state (§7.6 snapshots) will join the reachability
  set when snapshots land — until then, an actor that saves state
  by blob and loses the digest outside the journal loses the blob.
- TTL enforcement (expiry sweep) remains future work; TTL is stored
  but inert.
- GC is manual (`ctx.gc()`); automatic watermark-triggered sweeps
  are policy for later.

## 5. Next

M3 closes the durability story. Remaining board: the executor
(round 21 — parallel transitions on workers), WASM guests (M1's
world binding), §7.6 snapshots, hydration of blob-backed state.
Then 1.0: "everything hard that isn't core is a pluggable
implementation away."

## Provenance

Owner "M3 it is!", 2026-11-08. Sources: rounds 12 (hash-bound
types), 16 (allocation law), 19 (ghost readability), spec §4 + §8.
