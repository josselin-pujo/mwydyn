# Handoff — the platformc consumer's open debts (input for the next mwydyn rounds)

> Status: session input, 2026-11-13 — taken up the same day; see
> "Session outcomes" at the bottom for what was decided and queued.
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

---

## Session outcomes (2026-11-13 — taken up in the mwydyn session)

The debts were recontextualized mwydyn-side and the round queue was
shaped; nothing above is rewritten. Decisions on record:

- **Debt 1 → ROUND 48, LANDED** (`GuestArtifact`, work-notes/48).
  The handoff's `from_precompiled(…, GuestMode)` sketch was REJECTED
  in arbitration: a pub-enum mode parameter is the round-41 override
  reborn (carrying a probe result is indistinguishable from forging
  one), and the probe it would save is 0–3 ms of 350 ms. The landed
  shape is the opaque artifact — compile once, probe once, mode
  private; `WasmActor::from_artifact` spawns instantiation-only
  (170 ns/spawn measured; the real instantiation is the first
  exchange's, per round 37's lazy law). platformc's `load_plugin`
  compiles + probes once per plugin and clones per serve spawn; the
  cwasm serialization extension stays deferred, as the handoff
  itself deferred it. The mwydyn-side justification on record is
  sharding (N actors, one codebase), not platformc's serve mode —
  which is why the round was worth landing despite the owner's
  demotion of the consumer-side urgency.
- **Debt 4 + Debt 3 → ROUND 49, LANDED** (work-notes/49; spec §9.7
  DECIDED — the JS-SDK surface is final, platformc's .d.ts copy
  cites it). Original queue note (bundle, codec first):
 The prelude codec (`mwydyn.decode`/`encode`) lands
  first — pinned to cbor-profile-v0 + blob-refs-v0, driven INSIDE
  the real composed engine (round-47 lesson: test the FFI layer that
  actually runs) — then the three round-43 forks are ratified DECIDED
  in the same round, so the declared-global surface (which grows
  `decode`/`encode`) is final before platformc's `.d.ts` copy cites
  the ruling. Substance of all three forks uncontested.
- **Debt 2 → ROUND 50, LANDED** (work-notes/50; spec §3.4 round-50
  bullet). Original queue note, default as stated: `call_task_with(spec:
  Spawn, letter)` with `call_task` kept and documented as the
  zero-grant convenience (capability-quiet-by-default named in the
  spec); the verb forces `Supervision::FailFast` like `spawn_task` —
  a one-call round trip presumes a task that terminates. Reframe on
  record: the correlation mint was already core-side (`call-N` off a
  context counter) — what platformc hardcoded was its
  re-implementation's copy; the actual debt is that the task verbs
  (round 17) predate the ambient surface (round 40) and never met —
  `spawn_task` hardcodes a capability-less `Spawn`. Pins: rows-bearing
  task does blob get/put; capability-less task refused Unauthorized
  BY NAME; correlation uniqueness under concurrent calls.

---

## Round 49 outcomes (2026-11-14)

Debt 4 + debt 3 landed as one round, codec first as ratified. The
prelude now ships `mwydyn.decode`/`mwydyn.encode` — decode
NORMALIZES non-canonical-but-valid CBOR in the restricted type set
(the owner's in-session fork decision), encode is strict canonical,
refusals by name (corpus kebab codes). Pinned against
cbor-profile-v0 + blob-refs-v0 THROUGH THE REAL COMPOSED ENGINE
(65/72 driven; 7 skips with recorded reasons — digests are
host-side at put, and the three validator-layer ref cases live at
the JS realm's barrier, not codec-encode). The three round-43
forks are ratified DECIDED in spec §9.7 with the declared global
final (`decode`/`encode`/`float`/`timestamp` in the .d.ts).
Consumer-side: adopt and delete the pass.js interim codec — that
ADR-0019 debt closes. Bonus for ADR-0019's ledger: the round found
and fixed two latent host-side conversion defects (the ref-field
property names were never NUL-terminated — garbage names that
agreed by rodata luck since round 43; and i64::MIN crossed as a
float via an abs()-overflow in the ±2^53 boundary check) — both
would have bitten platformc's ref-typed passes eventually.

---

## Round 50 outcomes (2026-11-14)

Debt 2 landed; all four handoff debts are now closed. Consumer-side
moves when adopted: `call_task_with(Spawn::new(None,
factory).rows(vec![...]), letter)` replaces the bridge's
re-implementation — its hardcoded correlation token dies with the
copy (the mint was core-side all along). Round 48's `GuestArtifact`
feeds `load_plugin` (compile + probe once per plugin, clone per
serve spawn); round 49's `mwydyn.decode`/`encode` replaces the
pass.js interim codec and the `.d.ts` copy cites spec §9.7.
