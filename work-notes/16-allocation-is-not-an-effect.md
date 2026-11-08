# 16 — Blob puts leave the effect list

> Status: DECIDED and implemented (2026-11-08). Amends
> [spec-v0](../docs/spec-v0.md) §3.5 (round 16 amendment), §4;
> [world-v0.wit](../docs/world-v0.wit) draft v0.3 (`reads` →
> `ambient`, put-effect removed). Gates green.

## 1. The owner's catch

*"We have blob puts in the effects list, which means they can
rollback — I'd rather have blob puts happen-before the effects
emission, because they are practically side-effect free: they just
create garbage that will be collected if the same byte-identical put
never happens to create a journaled persistent reference. Having
them in the effects list means we don't actually have the ref to
insert into a posted message — how do any actor ever get a blobRef?"*

And the meta-catch: the spec contradicted itself. §4 (round 02) had
`put` as an ambient WIT function; round 07's effect list silently
overrode that. Round 16 restores §4's logic with a law behind it.

## 2. The law

**Allocation is not an effect.** A put creates content-addressed
bytes and nothing else. If no committed reference ever forms, it is
unobservable garbage that reference-aware GC collects (M3, above the
live watermark). A rolled-back put and an unexecuted put are
indistinguishable in observable state — so moving puts off the
transactional surface weakens NOTHING:

- the atomic unit becomes state + posts + mints (the banking nerve's
  actual needs);
- purity survives (put is pure in `(bytes, type)`, replay re-puts
  identically);
- capability enforcement survives (the `blob.write` row is checked
  at the eager call, same as `blob.read` at get).

## 3. The :-P answered

`host.blob_put(bytes, media_type, ttl) → ref` — the ref is IN HAND
during receive; posts embed it the same breath. The walking test now
does put → post-ref → verified-read in ONE receive flow. The
out-of-band `Value::blob_digest` workaround is gone from the happy
path (the function stays, for verifying refs read from elsewhere).

## 4. Implemented

- world draft **v0.3**: `reads` interface renamed **`ambient`**
  (repeatable reads + non-observable allocation); `blob-put` added;
  `put-effect` deleted; effects = post/mint/emit.
- core: `Effect::Put` removed; `Host::blob_put` (cap-checked);
  RamBlobStore gains RefCell interior mutability (eager `&self`
  puts; single-threaded pump — revisit if hosting goes
  multi-threaded); engine drops the put arms.
- tests: `blob_ref_in_hand_put_then_post_then_verified_read` (the
  one-flow story); `atomic_effects_rollback_observable_state_
  allocations_are_garbage` (post rolls back, allocation lingers as
  garbage — observable state identical, count asserts the new
  semantics).

## Provenance

Owner pushback, 2026-11-08. Sources: round 02 (ambient law), round
07 (the effect list this round corrects), A1 work-notes 04 §10
(replay-safe content addressing).
