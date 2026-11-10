# 41 — The Rust guest SDK (author-facing mwydyn)

> Status: implemented and green (2026-11-10). 31 suites / 71 tests,
> clippy clean workspace-wide. All three forks arbitrated by the
> owner (§2).

## 1. The problem

`mwydyn-guest` (round 40) is barrier glue: scratch, the response
block, the ambient-call wrapper. Every guest still hand-writes the
`#[no_mangle]` exports, decodes `BarrierRequest`, destructures
payloads with a hand-rolled `field()`, and assembles effect values —
exactly the internals knowledge a plugin author must NOT need.

**The bar** (owner): an agent or developer builds a plugin with zero
knowledge of mwydyn internals — stateless or stateful — from the
crate's own documentation alone. Docs and examples are product, not
afterthought: the SDK must be self-supporting.

## 2. The forks (arbitration record)

### 2.1 WHO DECLARES STATEFULNESS — guest declares [DECIDED]

Round 37 made statefulness an EMBEDDER declaration
(`from_compiled` vs `from_compiled_stateful`) while native actors
self-declare via `fork_carries_state` — an asymmetry. Owner chose
guest-declares (2026-11-10):

- The SDK emits a declaration export from every entry macro:
  `#[no_mangle] mwydyn_decl() -> u32`, bit flags; bit 0 = STATEFUL.
  Stateless entries emit it too (explicit zero).
- The container reads it as THE source of truth for the guest's
  lifecycle; `fork_carries_state` answers from it.
- `from_compiled_stateful` is REMOVED (fix-forward; pre-1.0
  workspace). No embedder override exists: forcing a stateful guest
  stateless to save memory silently breaks the author's exactly-once
  expectation — the refusal is the design. `from_compiled` probes
  `mwydyn_decl` once at composition (one throwaway instantiation;
  the mode rides the struct as today).
- ABSENCE of the export = Stateless BY OMISSION — WAT fixtures and
  hand-rolled guests unchanged. SDK users never face this path (all
  three entry macros emit the export).

### 2.2 THE ENTRY SURFACE — three entry macros [DECIDED]

`entry!` / `stateful_entry!` / `durable_entry!` over `Actor` +
`Durable` traits. The author's model is three words: stateless /
stateful / durable.

- `durable_entry!` implies the stateful flag AND generates the §7.6
  `snapshot`/`snapshot`/`restore` exports — durability without
  rollback would record torn state.
- `Durable` is COMPILE-REQUIRED for `durable_entry!`: the
  vacuous-snapshot bug class (round 39's latent restore bug: ran
  empty, tests couldn't tell) dies at build time. Export presence
  stays the container's capability probe — no §9 ABI change.

### 2.3 REPLY ROUTING — bless replyTo [DECIDED]

The model has no reply-to field (owner law; envelope `from` IS the
return address for actor→actor). But every example guest hand-rolls
the same idiom: payload field `replyTo` naming a third place (the
embedding's external outbox). RATIFIED (2026-11-10, after shipping
as the in-force default): the SDK documents `replyTo` as THE plugin
convention and ships the helpers — `letter.reply_to()` and
`Effect::reply_via(letter, kind, payload)` (posts to the field,
carries the correlation) — clearly labeled convention-not-model.
Spec §9.5.

## 3. The design

The whole author-facing surface:

```rust
use mwydyn_guest::prelude::*;

struct Counter { count: u32 }

impl Actor for Counter {
    fn receive(&mut self, letter: Letter, cx: &Cx) -> Vec<Effect> {
        let order = letter.payload_value().expect("canonical order");
        let n = self.count + 1;
        self.count = n;
        vec![Effect::reply(&letter, cx, encode(&Value::Int(n as i64)))]
    }
}

impl Durable for Counter {
    fn snapshot(&self) -> Vec<u8> { self.count.to_le_bytes().to_vec() }
    fn restore(&mut self, state: &[u8]) {
        self.count = u32::from_le_bytes(state.try_into().expect("4 bytes"));
    }
}

mwydyn_guest::durable_entry!(Counter { count: 0 });
```

- **The trait mirrors the native contract on purpose** (compose-
  parity law): `receive(&mut self, letter, cx) -> effects`, effects
  RETURNED, never called. The letter arrives BY VALUE (the guest
  owns it); the minted sender rides `Cx::from` — byzantine posture
  holds at the SDK exactly as at the barrier.
- **`Cx`** = `from: &Address` + the typed ambient face:
  `blob_get(&BlobRef)`, `blob_put(bytes, media_type) -> BlobRef`,
  `blob_len(&BlobRef)`, `caps() -> rows` — refusals surface BY NAME
  (`Unauthorized`, `Missing`, …) as `Result<_, Refusal>`. No clock
  verb, ever (§6: the ticker is the only wall-clock reader).
- **`Effect`** = the barrier's verbs as builders (`Effect::post(to,
  kind, payload)`); `mint`/`emit` join when the barrier grows them.
  `Effect::reply(&letter, cx, payload)` posts to `cx.from` carrying
  the correlation (the model's return-address path).
- **Panic = strike**, documented as THE failure path (crashes are
  supervision's problem); `fail!("why")` for readability.
- **Payloads stay `Value`** with helpers (`payload_value()`, typed
  map getters). NO serde — serde_cbor bytes would hit the payload
  wall; one canon, both realms. A ToValue/FromValue pair + derive is
  a later round if the grind demands it.
- **Instance storage**: the entry macros hold the actor in a
  single-threaded wasm32 static (`UnsafeCell`); sound because the
  barrier never re-enters `process` (ambient calls do not call back
  into the guest). Stateful state lives in the struct = linear
  memory = the round-37 image rollback, free.

## 4. The pieces

- `mwydyn-guest` grows in place: prelude (Actor, Durable, Cx,
  Effect, Letter, Address, Value, BlobRef, Refusal, codec, macros) —
  a plugin is ONE dependency; mwydyn-data types ride re-exports.
- `mwydyn-wasm`: build() probes `mwydyn_decl`; stateful ctor removed;
  statefulness test drops `mode_ctor` (the guest artifact now
  decides — the same binary, the declaration inside).
- `guest-counter` → the `durable_entry!` example (round-39 durable
  proof, behavior unchanged); `guest-store` → the `entry!` example
  (round-40 ambient proof); `guest-minimal` STAYS hand-rolled — the
  byzantine fixture must not depend on the thing it tests. NEW:
  a `stateful_entry!` example (ephemeral dedup window) — three
  examples, one per word.
- Host-target SDK unit tests keep the refusing-import stub pattern
  (round 40).

## 5. Docs-as-product

The crate-level doc IS the guide: front-page example, the three
words, panic=strike, caps and refusals, one section per entry kind
linking its example crate. Per-API docs carry runnable snippets.
Self-supporting test: an agent that has never read the spec ships a
green plugin from `mwydyn-guest` docs alone.

## 6. Tests (as shipped)

- stateless: the round-40 store suite on the SDK build — receipt
  shape preserved field-for-field (capped green / uncapped sees
  `Unauthorized`; one migration bug caught: an early return on
  put-refusal skipped `capRows` — the uncapped test saw it).
- stateful + THE DUAL (amended in flight): `guest-dedup` builds ONE
  source under TWO declarations (`--features stateless-build`) —
  stateful: a correlation whose attempts all struck is still fresh;
  stateless: the strike-dirtied mark stays. The round-37 dual proof
  survives guest-declaration exactly because the feature flag is
  the declaration.
- durable: the round-39 round-trip on the SDK counter (3 letters →
  snapshot → pending → die → reopen SQLite → boot restore →
  hydrate → count=4 exactly-once).
- decl probe: WAT fixtures all three ways (absent → Stateless by
  omission, explicit zero → Stateless, bit 0 → Stateful).
- SDK host-target tests: dispatch round trip + both reply
  conventions, decl bits, snapshot/restore through the generated
  symbols, garbage request bytes = strike (observed on the plain
  inner path), refusals by name on the stub host.
- barrier-v0 vectors unchanged — SDK responses are ordinary
  canonical CBOR; the SDK is invisible to the wall.

## 7. The lessons the implementation taught

- **THE PANIC WALL**: host targets cannot unwind through
  `extern "C"` — the first `catch_unwind` around the generated
  `process` aborted the test runner (STATUS_STACK_BUFFER_OVERRUN).
  The generated exports now catch + abort INSIDE the boundary; wasm
  traps natively. Loud either way, never a swallow.
- **The probe store needs its own epoch budget**: wasmtime's
  default epoch deadline is EXPIRED — the decl probe's single
  `mwydyn_decl` call trapped instantly until the probe set
  `set_epoch_deadline` like `make_rt` does.
- Test singletons: the generated slot is one per artifact —
  parallel host tests in one binary race it. The SDK docs say
  wasm guests are single-threaded by construction; the host tests
  run sequentially.

## Provenance

Owner, 2026-11-10: "guest-declares" (fork 1), "three entry macros"
(fork 2), "agreed on the replyTo convention" (fork 3 — ratified
after shipping as the in-force default; §2.3 records both moments).
