# 42 — ambient-v0 conformance vectors

> Status: implemented and green (2026-11-10). Spec §9.4 now cites
> the corpus by name.

## 1. The scope

§9.4's ambient import got its conformance corpus, mirroring
barrier-v0 (round 39) exactly: a machine generator
(`examples/gen-ambient-v0.rs` — never hand-type hex), the committed
corpus (`vectors/ambient-v0.json`), and a harness
(`mwydyn-vectors/tests/ambient.rs`) that reconstructs every case
from JSON, re-encodes, and verifies byte-identity + refusals
forever after.

## 2. What the corpus pins

- **THE FOUR REQUEST FORMS** — `blob.get` / `blob.len` (a validated
  BlobRef, digest machine-computed from fixed bytes+type), `blob.put`
  (ttl `null` and ttl-int variants), `caps` — canonical CBOR pinned
  as hex, round-tripped through `AmbientCall::to_value` → decode →
  `from_value` → re-encode byte-identical.
- **THE REFUSAL TABLE** — unknown verb (`UnsupportedVerb`, with the
  offending NAME asserted), `ref` missing / not a BlobRef,
  `bytes`/`mediaType`/`ttl` wrong types, negative ttl
  (`BadVerbField`), non-map request / non-text verb (`BadForm`),
  garbage bytes (`Decode`).
- **THE RESPONSE ENVELOPE FORMS** — `{"ok": <value>}` for each
  verb's result shape (ref / bytes / int / caps rows) and
  `{"err": "<code>"}` with the EXACT SPELLING of every dispatch
  refusal: `Unauthorized`, `Missing`, `Integrity`, `Unavailable` —
  a second-realm SDK that spells one differently fails the corpus.

Out of scope (recorded so nobody re-litigates): the `-1`-retry
scratch growth protocol (linear-memory mechanics, not CBOR) and the
SDK reader's malformed-envelope errors (SDK host tests already pin
them; the corpus pins CONTRACT forms, not reader diagnostics).

## 3. The case schema

Every case carries `kind: request|response`, a JSON `fields`/`ok`/
`err` body with two desugars (`{"$hex": "…"}` → Bytes,
`{"$ref": {digest, mediaType, size}}` → BlobRef — the harness
converts), optional machine-pinned `canonical_hex`, and optional
`expect_error`. Malformed shapes are expressed as plain JSON values
(a Text where a ref belongs, a negative ttl) — the harness encodes
them through the SAME canonical encoder, so even refusal inputs are
proven canonical bytes.

## Provenance

Owner, 2026-11-10: "let's do the conformance vectors first" — the
round is round-39's pattern applied to §9.4, no new forks.
