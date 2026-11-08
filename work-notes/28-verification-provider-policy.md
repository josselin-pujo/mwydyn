# 28 — Verification is provider policy; range reads seek; merge actors

> Status: DECIDED + implemented (2026-11-08, three owner decisions in
> one round). Amends §4 (verification cadence), §8.5-range semantics;
> face gains `get_range`; BlobActor-style whole-fetch verification
> removed from the contracted path.

## 1. The owner's three calls

1. **Verify-on-read left the contracted path.** At scale it is
   hostile: a range read of a large blob would force a full fetch
   AND full re-hash before serving the first byte. CONTENT IDENTITY
   REMAINS THE INVARIANT; VERIFICATION CADENCE IS PROVIDER POLICY
   (eager per-read, background scrub, or CAS-by-construction).
   Corruption is reported loudly whenever the provider detects it.
2. **Range reads seek.** `get_range` on the face: correct default
   (materialize + slice — works everywhere), SQLite overrides with
   `substr` (store-level slice, A1 D-023's seek discipline
   recovered; `LENGTH(bytes)` for len — no fetch at all).
3. **Atomic writes stay; big blobs are assembled by MERGE ACTORS** —
   the owner's mid-round call, opined below.

## 2. Round-28 semantics shift, accepted

The ref's `mediaType` is now a HINT. Content identity resolves by
digest alone; a ref re-labeled in transit still returns the TRUE
bytes under a lying label. Mitigations, unchanged or sharper:
- put-time digest = hash of `(bytes, type)` — the label lie cannot
  change what the bytes ARE;
- dispatchers that care consult provider-truth (`media_type` column
  query — no fetch) or sniff magic bytes (docgen always did);
- every provider MAY still verify (policy), and a scrub detects
  relabeled refs exactly as before.
Test updated: mismatched-type ref → READ:hello (assertion flip),
with the reasoning recorded here.

## 3. Merge actors over progressive writes (owner call, endorsed)

Progressive writes were rejected by the owner and I agree — they
constrain object-store providers (multipart becomes an admission
requirement) and break the single-digest identity: the hash needs
ALL bytes. **The merge actor is strictly better**:
- it is an ACTOR (compose-parity: Rust for speed, WASM for
  portability, platform-specific by construction — exactly the
  round-11 law);
- it inherits capabilities, GC reachability (part refs journaled in
  the merge request letter survive; merged results survive), and
  the round-21 executor for free;
- identical slices dedupe for free across merges (content
  addressing);
- for truly huge blobs, merges compose into TREES — Merkle-shaped
  assembly without ever changing the single-digest identity.
Placement: mwydyn-stdlib actor eventually, NOT a face method (the
face carries only what the contract reads, rounds 10/18).

## 4. Implemented

- `BlobFace::get_range(ref, offset, len)` — clamped semantics;
  default = materialize+slice; SQLiteBlobStore overrides with
  substr + `LENGTH(bytes)` len (no fetch).
- Host: `blob_get_range` (blob.read cap) alongside get/len.
- RAM get/len: direct map operations, no re-hash.
- Walking test updated to round-28 semantics (mismatched-type ref
  reads by digest); range/clamp test added (RANGE:456789,
  CLAMP:2:EF on the 16-byte pattern).

## 5. Gates

Full suite green, clippy clean.

## Provenance

Owner decisions (verification cadence; range seeks; merge actors),
2026-11-08. Sources: spec §4; A1 D-023 (store-level slices); round
12 (hash-bound types — put-side law intact).
