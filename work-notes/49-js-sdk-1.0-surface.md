# 49 — The JS SDK's 1.0 surface: the prelude codec + the three forks ratified

> Status: implemented and green (2026-11-14) — the corpus harness
> green through the real composed engine (65 of 72 cases driven, 7
> skips with recorded reasons), 48 suites, clippy clean
> workspace-wide. Second round from the platformc handoff (ADR-0019
> debts 4+3, owner-ratified bundle: codec first, then the forks).
> Spec §9.7 added — the JS SDK surface is FINAL as of this round.
> Two latent host-side defects found and fixed on the way (§4) —
> both in the round-43 conversion layer, both luck-dependent, both
> invisible until a second realm read the field names by their
> clean values.

## 1. The codec — one canon, both realms, completed

Round 33's law: "guest-side codec IS mwydyn-data — one canon, both
realms." The Rust realm ships mwydyn-data; the JS realm shipped no
codec, so a JS author whose BLOB CONTENTS are CBOR hand-rolled one
(platformc's pass.js interim, flagged temporary from birth). The
prelude now ships `mwydyn.decode(bytes)` / `mwydyn.encode(value)`:
the conversion table's natural completion, pinned against
cbor-profile-v0 + blob-refs-v0 — the corpora were written to be
consumed by exactly this second realm, and the round-47 lesson
(the surface with zero tests is where the defects live) applied
verbatim: the vectors runner IS the coverage.

**Decode is liberal, encode is strict** (owner, in session — the
round's one open fork): decode NORMALIZES non-canonical but valid
CBOR within the restricted type set — non-shortest heads accepted,
map keys sorted on read (duplicates refused), f16/f32 widened to
f64 — mirroring what the Rust decoder already did (the corpus's
`dec-int-24-nonshortest-accepted` and
`dec-map-decodes-and-canonicalizes` were normalization pins all
along). What decode refuses, it refuses BY NAME: the corpus kebab
codes (`indefinite`, `malformed`, `trailing`, `empty`,
`duplicate-key`, `non-finite`, `unknown-tag`, `bad-key`,
`too-deep`, `bad-bignum`, `bad-ref`) ride `.code` on the thrown
error.

The JS value model mirrors the barrier's table (compose-parity of
the codec's own surface): number within ±2^53, BigInt beyond;
Uint8Array bytes; and a decoded BlobRef carries the SAME `__mwRef`
brand the host sets — a decoded ref crosses the barrier as a real
tag-1002 BlobRef, which the harness proves end to end.

Two JS-realm facts are explicit, not lossy surprises:
- **Floatness is a box.** A bare integral JS number IS an int
  (`3.0 === 3`; the barrier normalizes it on crossing — this was
  already the table's rule). The corpus's
  `enc-float-3.0-floatness-preserved` is unreachable through a bare
  number, so `mwydyn.float(n)` is the explicit form — and the only
  way a bare 3 encodes as `fb4008...`.
- **The timestamp tag is a box.** Timestamps cross the barrier as
  bare µs numbers (tag lost, documented since round 43);
  `mwydyn.timestamp(µs)` and decode of tag 1001 produce a branded
  `{micros}` box that re-encodes with the tag.

Encode of a branded ref is a serializer, not a validator — the
round-12 law: forged-but-shaped refs encode; validation is the
barrier's on crossing and decode's on reading.

## 2. The harness — the corpus through the real engine

`mwydyn-wasm/tests/js_cbor.rs`: ONE quickjs guest composed through
the mwydyn-js library (the library the consumer uses — no second
artifact), one context, every case a letter `{op, name, ...}`, the
guest answering `{name, ok: <canonical hex>} | {name, err: <code>}`.
Driving through the real engine is the point: the round-47 defects
were all in the FFI layer a host-side interpreter would never run.

The routing encodes the barrier's honest limits — what cannot cross
undistorted rides an IN-GUEST literal table keyed by case name (an
unknown key fails loudly, so corpus edits can't drift silently):
bignums (loud refusal at the barrier, v0), timestamps (tag loss),
and integral floats (the box fact above). Decode results that are
BigInts report a decimal string instead of echoing an uncrossable
value; a 64-deep value is not echoed at all (the reply map's
wrapper would push it past the decoder's MAX_DEPTH at the payload
wall — `dec-64-deep-ok` proved the wall bites at depth 65; its
canonical re-encode carries the proof alone).

**65 of 72 driven; 7 skips, each with its reason on record:**
- op=Digest (4): digests are computed HOST-SIDE at put; a JS
  SHA-256 is not part of the canon codec (a future nicety, not
  canon).
- enc-ref-reject-* (3): these pin the VALIDATOR layer (model→Value
  in the Rust runner); the JS realm's validator is the BARRIER
  (js_to_value validates branded refs on crossing — round 47), and
  codec-encode is a serializer by law. No JS-side equivalent at
  this layer — asserting the opposite would encode the divergence
  confusingly.

## 3. The three forks — ratified (spec §9.7, DECIDED)

Substance uncontested (defaults in force since round 43; platformc
inherits, doesn't own them):

1. **Package shape: types-only + injected prelude.** One runtime
   per artifact, no skew. Rejected: runtime-in-package.
2. **Payload typing: static types + BYO guard.** `payloadOf(guard)`
   — loud refusal = strike. Rejected: zod/io-ts (weight, and the
   schema-blind core law); host-side CDDL at admission (same law).
3. **I64 boundary: number when safe, BigInt beyond.** The only
   lossless option without the ergonomic tax. The codec and the
   barrier now share this boundary EXACTLY — which is how the
   round found §4's second defect.

The declared global is final with the codec on it
(`decode`/`encode`/`float`/`timestamp` in the `.d.ts`), so
platformc's `.d.ts` copy cites a stable ruling — the bundling
order (codec first, ratify after) was the owner's call and this is
why.

## 4. Two latent defects found and fixed — the conversion layer's luck ran out

Both in `convert.rs`, both present since round 43, both surviving
because write-side and read-side garbage AGREED — same compilation
unit, same rodata, same extended bytes — until the prelude grew and
the engine rebuild shifted the layout.

1. **The ref fields' property names were never NUL-terminated.**
   `set_str` passed a raw `&str`'s `as_ptr()` to
   `JS_SetPropertyStr` (and the brand-closure read side did the
   same to `JS_GetPropertyStr`): the property name read past
   `"digest"` into adjacent rodata — the harness's first ref reply
   carried keys like `digestmediaTypebad-keycalled
   \`Result::unwrap()\` on an \`Err\` value,…`. Round 43's "NUL
   WALL" fixed the map-key branch with CStrings; the ref-field
   branch never got it. The prelude codec reads `v.digest` by its
   CLEAN name — the first reader in five rounds that doesn't share
   the wrapper's rodata — and broke the luck. Fix: C literals
   (`c"digest"` etc.) on both sides. **The class: host-side
   write/read pairs over the same FFI can validate each other
   forever; only a SECOND consumer of the names (the guest's own
   code, a corpus, a debugger) can catch the name was never the
   name.**
2. **`i64::MIN` crossed as a float.** The boundary check was
   `i.abs() <= SAFE` — `i64::MIN.abs()` overflows (panics in
   debug; wraps to still-negative in release), so `-2^63 ≤ 2^53`
   evaluated TRUE and the value took the float64 branch:
   `enc-int-i64min` got `fbc3e0…` (f64 -2^63) instead of
   `3b7fff…`. Fix: range comparison, no `abs()`. First exercise of
   the i64 boundary's floor since the fork was written.

Plus one prelude-side embarrassment the harness caught before any
consumer could: the ref encoder wrote the map head and the three
values but never the three KEY strings (and `readFloat` re-read
float bytes the head reader had already consumed — the Rust
reader's comment "for floats the argument carries the raw bits" was
the spec, unread). All three landed as corpus failures on the
first green-path run; the corpus did its job.

## 5. Consumer mapping

When platformc adopts this: `mwydyn.decode/encode` replace the
contract-scoped codec in `plugins/js-example/pass.js` — delete the
interim, ADR-0019's "contract-scoped codec" debt closes. The
`.d.ts` copy cites spec §9.7. Nothing else moves consumer-side.

## 6. State

- `mwydyn-guest-js/src/prelude.js` — the codec block + four new
  words on the global (`decode`, `encode`, `float`, `timestamp`).
- `crates/mwydyn-guest-js/ts/index.d.ts` — `CBORValue`,
  `TimestampBox`, `FloatBox`, `MwydynCborError`, the four fns.
- `crates/mwydyn-wasm/tests/js_cbor.rs` — the corpus harness
  (mwydyn-wasm gains a dev-dep on mwydyn-vectors for the loader
  and projections).
- `docs/spec-v0.md` — §9.7 added; §9 header carries round 49.
- FOLLOW-UP (owner nit, same day): the TS surface — `Actor.receive`
  now returns `Effect[]`, not `object[]`; `Effect` is an opaque
  branded type (unique symbol) whose only typed path is a builder —
  hand-shaped objects are untyped by design, symmetric with "a bare
  number is an int until mwydyn.float says otherwise."
- Round 50 queued unchanged: tasks meet the ambient surface
  (`call_task_with(spec: Spawn, letter)`).
