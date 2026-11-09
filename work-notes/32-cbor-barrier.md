# 32 — CBOR all the way: the WASM barrier without WIT

> Status: implemented and green (2026-11-09). All suites: 18 walking
> + 5 face + 1 throughput + 4 envelope/barrier units + 2 corpora
> (72 cases) + 2 schema + 4 wasm barrier + 1 wasm perf + 1 wasm
> ping/pong. Clippy clean across the workspace.

## 1. The interrogation

Owner: "my idea of decode/encode outside the barrier came from a
world where runtime typing is a thing, quickjs and v8, but wasm
doesn't have those."

Answer: in V8/QuickJS, host and guest share a runtime type system —
"passing a value" is handing a reference into the engine's heap.
WASM has no shared heap and no runtime types. Trees cannot cross;
decode happens on BOTH sides regardless. The only real question is
whose representation is canonical AT the barrier.

Ratified law: **canonical CBOR bytes are the WASM ABI.
`Value`-in-memory never crosses a boundary** — the same law the
journal already runs. Journal ⇄ wire ⇄ barrier: one canon.

## 2. The owner's kill shot: WIT dies

Once payload stays bytes, WIT would only type the ENVELOPE — but
those types (verbs, addresses, correlation, letters) already exist
as canonical CBOR in mwydyn-data. A WIT world would be a THIRD
representation of types that already have one. Owner: "let's get
rid of WIT once and for all... a few enforced **external** CBOR
envelope types replace our WIT contract."

Consequences:
- Guests compile to plain `wasm32-unknown-unknown` with ZERO
  imports — no WASI, nothing to fake, the surface literally does
  not exist.
- The component model is gone: `wasmtime = "26"` with no
  component-model feature; engine.rs deleted; the Windows
  `__fastfail` epoch-trap limitation died with it.
- Any host that can call ptr/len can run a mwydyn guest.
- What we honestly drop: component-model ecosystem interop. Guests
  are mwydyn actors, not generic components — we were never going
  to use it.

## 3. The barrier contract (mwydyn-data/src/barrier.rs)

- ABI: guest exports `alloc(len)→ptr`, `dealloc(ptr,len)`,
  `process(req_ptr, req_len)→resp_ptr`; response block is
  `[u32 le resp_len][resp bytes]`.
- Request (host→guest): canonical CBOR map
  `{"from": <address>, "letter": <letter>, "v": 1}` —
  `BARRIER_VERSION` checked before anything else is interpreted.
- Response (guest→host): canonical CBOR array of single-entry maps
  `[{"post": {"to": …, "letter": …}}, …]`; unknown verbs refused
  with `UnsupportedVerb`.
- The guest-side codec IS mwydyn-data (one canonical-CBOR
  definition, both realms — compose-parity down to the wire format).
- `Letter::to_value/from_value` + `address_to_value/from_value`
  extracted in envelope.rs — the barrier reuses the journal's
  vocabulary; the flat envelope form is untouched (checksums and
  stored rows depend on it).

## 4. THE PAYLOAD WALL (the round's structural change)

The wall now refuses non-canonical payload bytes at BOTH admission
points, whatever realm produced them:
- `Context::post` (embedding letters) → `Verdict::Refused(Invalid)`;
- `deliver` Phase 2 `Effect::Post` (actor effects, native or WASM)
  → Strike("non-canonical payload in effects").

The journal never stores undecodable payloads — GC/snapshot/replay
can always walk one. The types already declared it (`Letter.payload`
doc: "canonical CBOR bytes"); the wall enforces the declared
invariant. Empty payloads: a zero-byte payload is NOT canonical
CBOR — the law has no special cases; letters that carry nothing use
`encode(&Value::Null)` (0xF6). Test letters and the ticker's tick
payload were updated (ticks now carry `{"firedAtUs": N}` — auditable).

## 5. The container (mwydyn-wasm/src/container.rs)

`WasmActor` = one module instance behind the ABI. `barrier_exchange`:
encode request → guest alloc → write → process → read prefix+bytes →
dealloc both → **the wall** (canonical decode + shape check).
EVERY failure mode — trap, garbage bytes, bad shape — panics: a
crash, and crashes are supervision's problem. Proven by the
byzantine test: a guest returning garbage dead-letters through the
real strike path (MAX_ATTEMPTS), never a silent empty-effect commit.

## 6. Numbers

| Tier | Throughput |
|---|---|
| Native ping/pong (group commit, RAM) | 54k letters/s |
| WASM i64 raw bounce (old ABI) | 3,434 bounces/s |
| WASM CBOR work-order bounce (this round) | ~330 hops/s |
| Raw wasmtime floor | 1.4M calls/s |

The CBOR tier's cost is honest work, not barrier tax: the
work-order grows a hop per bounce (O(n²) total encode work across
a run) and every fork re-instantiates the module (stateless-fork
approximation; §7.6 snapshots are the fix). Guest artifact: ~94 KB,
zero imports.

## 7. Bug story (recorded for the pattern)

First bounce run: "non-canonical response: Empty" ×3 → dead letter.
Root cause was TWO design bugs in the guest, not the container:
1. No reply-to-sender routing — pong posted back to payload
   `replyTo`, i.e. ITSELF, self-looping to the budget.
2. The budget branch answered with `respond(&[])` — a zero-length
   block, which is not the encoding of an empty effects array
   (that's `[0x80]`, len 1). The wall rightly refused it.
Raw-wasmtime probe (throwaway test, deleted after) isolated the
guest as healthy — the discipline: probe the layer below before
blaming the layer above.

## 8. Arbitration addendum — the wall's boundary line

Owner asked for precision: "the outside decoder only validates that
the envelope is valid CBOR right, internal payloads are seen as
bytea fields... those may well be under a contract, but it's not
the host responsibility to enforce it."

Answer, ratified: YES at the shape layer (payload is `Value::Bytes`,
never interpreted by the envelope decoder), but the wall enforces
one step beyond envelope validity — payload bytes must decode under
the canonical CBOR profile. That is the DATA-MODEL layer, not the
contract layer. Owner then recalled the load-bearing reason:
"forgot about the introspection need for blobrefs... no choice
there if we want deep pointer inspection."

The line, for the record:
- **VALIDITY** (is this canonical CBOR?) — host territory,
  mwydyn-data's domain, enforced at post + Phase 2.
- **SHAPE** (what the payload means) — the kind's contract,
  registry/consumer-side; the host never checks fields, types, or
  structure; a letter of any kind with payload `Null` passes.

The wall exists because the runtime is ITSELF a payload consumer:
GC's tag-1002 sweep, snapshot restore scans, ticker arm parsing.
Without the wall, undecodable payloads make blob refs invisible to
GC reachability — a correctness hole, not a style choice.

## Provenance

Owner ratifications, 2026-11-09: "canonical CBOR bytes are the WASM
ABI, plugin SDK provides the wasm side marshall/unmarshall code"
+ "let's get rid of WIT once and for all... pure wasm-unknown-
unknown which is more widely supported anyways" + the validity/
shape arbitration above.
