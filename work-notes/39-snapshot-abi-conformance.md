# 39 — The §7.6 guest snapshot ABI + conformance vectors

> Status: implemented and green (2026-11-09). 24 suites ok, clippy
> clean. Two corpora added (overlay-v0, barrier-v0).

## 1. A latent bug, found by contradiction

While wiring the WASM restore path I noticed scan_snapshot matches
`"ref"` as a DIRECT `Value::BlobRef`, but the native counter posted
it **Bytes-wrapped** — those cannot match, yet the snapshot test
passed. Empirical probe confirmed: **the counter's snapshot letter
was never found; boot.snapshot was always None; the restore path ran
vacuously.** The test passed because WITHOUT a snapshot,
`replay_from = 1` and full deterministic replay reconstructs the
same count (1+2 replayed + 3 live = 3 restored + 3 live = 6) —
indistinguishable at the assertion. Bonus defect: a Bytes-wrapped
ref is **invisible to GC reachability** — the snapshot blob was
collectable while referenced.

Fixes:
- `scan_snapshot` accepts BOTH forms (direct preferred);
- the native counter posts DIRECT BlobRef (GC-visible);
- the test is HARDENED: the counter announces `RESTORED` on the
  next receive after a boot restore — restore-from-snapshot is now
  distinguishable from full-replay luck.

## 2. The §7.6 guest snapshot ABI (round 39)

**Restore** (boot → guest): guests MAY export
`restore(state_ptr, state_len)`. At `init`, a boot snapshot ref is
`blob_get`-ed host-side (capability-gated — the actor needs
`blob.read`; the test grants it on the Spawn) and the bytes cross
the barrier through `alloc`/`restore`. A stateful guest carrying a
snapshot but lacking the export panics LOUD — refusing to silently
drop durable state.

**Save** (guest → durable): guests MAY export
`snapshot() -> [u32 len][bytes]`. The embedding calls
`Context::snapshot_actor(name)`: the runtime pulls state through the
new `Actor::snapshot_state` face, stores it (the RUNTIME's ambient
blob power — caps are for actors; restore-through-host keeps the
byzantine posture), and lands the snapshot letter with the
runtime-stamped `covers` = the actor's CURRENT cursor. The ref is a
direct BlobRef. Reopen + spawn under the same name restores it;
`covers + 1` seeds `replay_from`.

**The durable round-trip proof** (counter guest, SQLite tier):
3 letters → snapshot_actor → 1 pending letter → die → reopen →
spawn (boot scans) → hydrate (covered prefix skipped) → receipt
count=4, exactly-once. The SAME guest artifact serves stateful and
stateless lifecycles; the state format is the guest's own (4-byte
LE counter).

## 3. Conformance vectors (the round-34 lesson, permanent)

- **overlay-v0** (behavioral, mwydyn-vectors/tests/overlay.rs): the
  three overlay roles driven through the FACE — the recording skin
  records and drains exactly; a durability face is silent; a plain
  hot tier is silent (recording is the overlay's wrapper's job).
  This is the test that would have caught the round-31/34
  inherent-vs-trait silent no-op at admission time. Future
  providers (NATS/Kafka hot views, quorum logs) satisfy the same
  three vectors.
- **barrier-v0** (data, vectors/barrier-v0.json + harness): the
  barrier envelope contract as data — canonical request/response
  forms (byte-exact, MACHINE-GENERATED via
  `examples/gen-barrier-v0.rs` — never hand-typed hex) and the
  refusal table (BadVersion, BadForm, UnsupportedVerb,
  non-canonical bytes). Any second-realm decoder or external rig
  consumes the same corpus.

## Provenance

Owner: "Good, guest snapshot-restore ABI and conformance is next!"
2026-11-09.
