# 15 — M0 part 3: the worm walks

> Status: implemented and green (2026-11-08). `mwydyn-core` runs the
> whole ratified contract end to end with native Rust actors.
> 10 walking tests + 4 envelope units + 2 corpora, clippy clean.

## 1. What exists now (`crates/mwydyn-core`)

- **actor.rs** — the contract, native form: `receive(ctx, host,
  letter) → Vec<Effect>` is the pure transition (§3.5); `MsgCtx`
  carries the runtime-minted `from` (read-only — the byzantine
  posture as a parameter split: the guest authored the letter, the
  runtime owns the rest). `Host` is the pure-read face: verified
  blob reads (cap-checked), caps list, and the clock read the
  TICKER alone should touch. Effects: Post / Put / Mint / Emit.
- **blob.rs** — RamBlobStore (§4): pure content addressing,
  idempotent-by-content puts, and **verify-on-read**: (type, bytes)
  must hash to the ref's digest, so a re-typed ref fails loudly
  (round 12). TTL stored, sweep deferred to M3's watermark GC.
- **context.rs** — the context as trust-boundary/clock-domain/
  transaction-boundary (round 08). Mailbox + cursor in one object
  (§3.1); admission with the coarse refusal vocabulary; the effects
  engine in four phases (pure transition → validate-all →
  classify-refusal → apply-then-commit-cursor); supervision
  FailFast/Restart; round-robin delivery scan.
- **ticker.rs** — the well-known NAME (runtime-reserved); arms are
  letters with DURATION payloads; ticks are letters to the armer;
  polls are letters the pump injects while work is armed; the wall
  clock arrives ONLY through the injected `ClockFace` (tests use a
  manual clock — §6 honored).

## 2. Design findings from making it run

- **`receive` needs `from`** — replies target the sender, but the
  byzantine posture keeps sender identity OUT of the letter. Fix:
  `MsgCtx` (runtime-authored, read-only) as a separate parameter —
  the guest sees runtime fields, cannot forge them. World draft
  bumped to v0.2 (pre-freeze, the draft exists to find exactly this).
- **Round-robin delivery is not optional**: first-match scanning
  starves every actor behind a perpetually-busy one. The rotating
  scan start is what makes busy-requeue self-healing actually heal.
- **Encode is a serializer; the barrier is the validator** (learned
  in round 12, confirmed here): a forged-uppercase ref panics a
  native actor that decodes payloads itself — the data-model
  rejection hits BEFORE the actor's integrity check. Poison-letter
  semantics absorb it.
- **§3.5 wording nuance**: "digests resolvable at validation"
  applies to effect-level structure; payload-internal references are
  opaque CBOR — actors compute digests themselves via the same pure
  function (`Value::blob_digest`) and the runtime's apply recomputes.
- The atomic-effects test writes a blob + posts to a dead address in
  ONE list: nothing applies, the letter strikes out — the banking
  nerve's rollback, demonstrated.

## 3. Gates

`cargo test`: 10 walking + 4 envelope units + 2 corpora (59+13
cases), green. `cargo clippy`: clean.

## 4. Next (M0 remainder)

Task-pattern sugar over unnamed spawns; the timer arm/tick schemas
into `mwydyn-schema` (first CDDL + CBOR projection + registry);
then M2's journal — the cursor leaves memory for a face.

## Provenance

Owner "Onward!", 2026-11-08. Face pattern: A1 work-notes/50.
