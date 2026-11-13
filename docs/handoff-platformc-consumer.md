# Handoff — the platformc consumer's open debts (input for the next mwydyn rounds)

> Status: DRAFT session input, 2026-11-13 — nothing here is decided.
> This collects the debts ADR-0019 (platformc) recorded after landing
> mwydyn as its plugin substrate, with the numbers and the consumer-side
> laws that should shape the designs. Expected to be taken up as one or
> more rounds (the owner suggested a dedicated mwydyn session). Consumer
> evidence lives in platform_generator: `crates/pc-plugin/src/lib.rs`
> (the letter contract is the crate doc), `crates/pc-plugin/tests/`
> (pass suite + js_example e2e), `plugins/js-example/pass.js` (the first
> real JS plugin), ADR-0019 at
> `docs/ideation/knowledge/decisions/0019-mwydyn-plugin-substrate.md`.

## Consumer context, compressed

platformc runs **stateless passes** — pure functions of their input —
as mwydyn guests over the `platformc.pass@1` letter contract:
request `{"input": <BlobRef>}` (blob carriage — real estates exceed the
letter cap), reply `{"ok": {"output": <BlobRef>}}` or
`{"refusal": {"message", "action"?}}` (refusal-as-data). Governance
(manifest, sha256 pins, L0 registrations, capability claims) is
platformc-side and world-agnostic. Per invocation platformc builds a
**fresh RAM context** and spawns a **fresh unnamed fail-fast task** with
exactly `blob.read`/`blob.write` rows. wasmtime is unified at 47
tree-wide. Two guest realms ship today: Rust (`mwydyn-guest`, `entry!`)
and JS (`mwydyn.entry(...)` composed via the mwydyn-js library —
platformc's `compose-js` wraps the lib and embeds the engine).

---

## Debt 1 — `WasmActor::from_compiled` recompiles the module per invocation

**Measured (release build, this box, wasmtime 47.0.4):**

| artifact | size | module compile | from_compiled (compile + §9.2 probe) |
|---|---|---|---|
| pii_pass (Rust pass guest) | 165 KiB | ~40 ms | ~40 ms |
| js_example (quickjs engine + script) | 1442 KiB | ~347 ms | ~350 ms |

Every `platformc pass` invocation pays a full recompile + a throwaway
probe instantiation before the real one. Invisible at batch CLI cadence;
it becomes the dominant plugin cost the moment passes run per watch
regeneration or per serve request.

**Why it is not a statelessness consequence** (the platformc side of
the split, which stays): fresh context + fresh actor per invocation is
platformc's law, deliberately — it continues the fresh-Store-per-`run_pass`
isolation story, and it is what makes the cross-invocation determinism
pin honest: mwydyn's stateless contract says "instance reused, no
guarantee" (mutation persists across letters), so INSTANCE POOLING on
the host side would weaken determinism unless mwydyn ever defines
reset semantics for stateless instances. Fresh actor per invocation is
the shape; only its COST is the debt.

**The actual gap** (mwydyn side): `from_compiled(engine, &[u8])` is the
only public constructor for compiled guests. The actor owns its
`Module`; there is no Module-taking constructor and no way to carry a
probe result across constructions — so the consumer's factory (a fresh
`Box<dyn Actor>` per spawn) can do nothing but recompile and re-probe
every time. Meanwhile wasmtime's `Module` is `#[derive(Clone)]` over an
Arc — compile-once, clone-per-spawn is right there.

**Candidate shapes (for the round to arbitrate, not prescribed):**

- `WasmActor::from_precompiled(engine, Module, GuestMode)` — the
  consumer compiles + probes ONCE at load (its `load_plugin` already
  wants the early compile/probe error) and hands clones per spawn.
  GuestMode in the signature keeps the §9.2 law visible: the probe is
  the guest's declaration; passing it in is carrying the probe's
  RESULT, not an embedder override (the override path stays dead).
- Or `from_compiled` gains a cached/`Arc<Module>` variant, or `WasmActor`
  exposes `module(&self) -> &Module` so the first construction seeds the
  rest.

**Optional extension, only if multi-process ever matters:** serialized
precompiled artifacts (`Engine::precompile_module`/`deserialize` — the
cwasm path platformc's vendored googlesql-rs already uses) for
cross-process caches. In-process Module reuse is the actual fix for
this debt; serialization is a separate conversation.

**Acceptance idea:** a perf pin in `mwydyn-wasm/tests/` — second
`from_precompiled` spawn within the same engine must be
instantiation-only (some generous multiple of instance cost, never a
compile).

---

## Debt 2 — `call_task` spawns capability-less tasks

`Context::call_task` → `spawn_task` → `Spawn::new(None, factory)` with
no `.rows(...)`. Any guest that needs the ambient surface cannot ride
the one-call round trip. platformc's bridge therefore re-implements
call_task's body (spawn → post under a minted correlation →
run_until_idle → match the external outbox) to add
`blob.read`/`blob.write` rows — duplicated machinery on the consumer
side of a face mwydyn already owns.

**Candidate shape:** a rows-bearing variant — `call_task_with(spec:
Spawn, letter)` or `Spawn`-parameterized dispatch — keeping call_task
itself as the zero-grant default (capability-quiet by default is worth
preserving; the round should name the default explicitly). The
correlation minting (`call-N`) belongs on the mwydyn side either way;
platformc currently hardcodes a fixed correlation token valid only
because its context is single-exchange.

---

## Debt 3 — the three unratified JS forks (round 43 §4, defaults in force)

All three are **pure mwydyn** — guest-SDK semantics platformc inherits
but does not own. Verbatim from work-notes/43, for the ratification
pass:

1. **PACKAGE SHAPE — types-only + injected prelude** (default): the npm
   package is types + a declared global; all runtime code is the
   wrapper's prelude. No version skew (one runtime per artifact).
   Rejected: runtime-in-package (two runtimes per artifact, skew risk).
   *Consumer note: platformc copies the .d.ts into
   `plugins/js-example/guest.d.ts`; a ratification would let that copy
   cite a ruling instead of a fork.*
2. **PAYLOAD TYPING — static types + bring-your-own-guard** (default):
   `letter.payload` typed by the author's `TPayload`; runtime
   validation is the author's guard (`letter.payloadOf(guard)` — loud
   refusal = strike on malformed shape). Rejected: shipping zod/io-ts;
   rejected: host-side CDDL at admission (schema-blind core law).
3. **I64 BOUNDARY — number when safe, BigInt beyond** (default): Ints
   within ±2^53 arrive as `number`, outside as `bigint`; TS says
   `number | bigint` with a lint nudge to widen early. Rejected:
   always-number (silent lossy), always-BigInt (ergonomic tax).
   *Consumer note: the pass contract's payloads are all text/maps, so
   platformc never crosses the boundary today — the ratification is
   pure SDK hygiene from where we stand.*

## Debt 4 — the prelude CBOR codec (the one that supersedes consumer code)

The round-43 conversion table covers Value⇄JS at the barrier (payloads
and blob refs arrive decoded; bytes arrive as Uint8Array) — but the
prelude ships no canonical-CBOR codec, so a JS author whose BLOB
CONTENTS are CBOR (the platformc contract's input/output maps) hand-rolls
one. platformc shipped a contract-scoped codec inside
`plugins/js-example/pass.js` (maps/arrays/text; shortest-form lengths;
key sort by byte-length-then-bytes; f64 floats; UTF-8 by hand —
`TextEncoder` is WHATWG, absent from the freestanding build; DataView
is ES 5.1 and works) and flagged it as deliberately temporary.

**Candidate shape:** `mwydyn.decode(bytes)` / `mwydyn.encode(value)` in
the prelude — the conversion table's natural completion, pinned against
the cbor-profile-v0 vectors (the corpus is already written to be
consumable by any second-realm decoder; the JS codec is exactly that
second realm). Same canon as mwydyn-data: one encode/decode, both
realms. Round-47's lesson applies: this surface had zero tests until a
real consumer crossed it — the vectors runner IS the coverage.

When it lands, platformc's example codec is deleted and the ADR-0019
"contract-scoped codec" debt closes.

---

## Priorities, as the consumer sees them

1. **Debt 1** (Module reuse) — the only one with a measured wall-clock
   cost, and the one serve mode pays per request.
2. **Debt 4** (prelude codec) — unblocks real JS plugins beyond the
   example; deletes consumer code.
3. **Debt 2** (call_task rows) — small, pure ergonomics, removes
   duplicated machinery.
4. **Debt 3** (fork ratification) — no behavior change from where
   platformc stands; hygiene for the SDK's 1.0 story.

Debts 2–4 are independent; Debt 1 is self-contained. None block
platformc today — the batch CLI cadence is fine and the interim codec
works.
