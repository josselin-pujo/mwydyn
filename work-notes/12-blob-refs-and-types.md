# 12 — Blob refs: a hard type code, and where the mediaType lives

> Status: [proposed, in discussion] (round 2026-11-08, owner raised
> both questions). Amends [spec-v0](../docs/spec-v0.md) §4 when
> ratified; profile gains tag 1002 (v0 → v0.1). The WIT surface does
> NOT change — `put-effect` already carries `media-type`, the ref
> record already carries it; only the digest construction and the
> data-model tag move.

## 1. The owner's two questions

1. **Hard type code for blobrefs?** — refs currently look like plain
   maps on the wire; blobness is convention, not type.
2. **The mediaType problem**: we like media types (distributed
   intelligence, convention-driven dispatch), but if the type is part
   of the *pointer* and not the *blob*, it is not part of the
   content-hash and cannot be a deterministic result of put — and
   put should return only a ref, pure, no magic detection. Owner's
   stated concerns: **forgeability and type confusion**.

## 2. The analysis

**The load-bearing observation: anything claimed outside the hash is
mutable-by-transit.** A ref is data; any actor, router, or log entry
between producer and consumer can rewrite a decorative `mediaType`
field, and nothing detects it. If the type drives *dispatch* — which
is precisely the "distributed intelligence" we like — then a forged
type is an injection vector into dispatch decisions (SVG-with-script
as claimed `image/svg+xml` is the classic).

**Placement options:**

- **(a) Decorative hint** (current draft shape): type rides the ref,
  unverified. Max flexibility; every consumer inherits a permanent
  verify-before-trusting duty. Contrary to the absence-over-policy
  posture.
- **(b) Hash-bound** (proposed): `put(bytes, mediaType)` →
  digest = SHA-256(canonical CBOR of `{bytes, type}`). The type is
  content; re-typing in transit is *detectable* at any verification
  point; put stays pure (the type is an *input* — no sniffing in the
  purity path, per the owner's requirement); replay-safety unchanged
  (deterministic inputs → identical digest). Costs, accepted: same
  bytes under two types = two blobs (no cross-type dedup); type
  frozen at put.
- **(c) Split** (b + advisory hint fields riding outside the hash):
  possible later for filename/language-style metadata; v0 adds
  nothing (freeze discipline).

**Hard type code**: yes — **tag 1002** wrapping the ref record;
enforced at the barrier decode (mwydyn-data's known-tag set), so
ref-ness is a data-model fact. Shape: `{digest: 64-hex, mediaType:
text, size: u64}`.

## 3. Ripples if ratified

- Profile v0 → v0.1 (tag 1002 joins the known set; vectors for
  ref-shape validation).
- `mwydyn-data`: the tag + shape validation; digest construction
  helper (`blob_digest(bytes, media_type)`).
- The blob STORE stores the (type, bytes) pair — `get` verifies
  digest on read (M3 inherits integrity checking for free).
- Sniff-validation: an optional embedding face, default absent —
  never a purity dependency.
- Spec §4 amendments drafted (marked proposed).

## Provenance

Owner questions, 2026-11-08. Sources: A1 work-notes 04 §1 (tagged
timestamp precedent — 1001; projections), 05 §2 (digest reference
shape), 35/D-028 (opaque-origin lessons on trusting metadata).
