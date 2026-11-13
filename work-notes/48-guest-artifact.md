# 48 — The guest artifact: the module/actor split made public

> Status: implemented and green (2026-11-13) — 47 suites, clippy clean
> workspace-wide. First round taken from the platformc consumer's
> handoff (docs/handoff-platformc-consumer.md, ADR-0019 debt 1). One
> latent test defect found and fixed on the way (the mwydyn-js lib pin
> was born skipped — §5). No design change to §9.2's law: the split
> CARRIES the probe's result; it does not reopen the override.

## 1. The debt, recontextualized

The consumer measured `WasmActor::from_compiled` recompiling the
module per invocation — ~40 ms for a 165 KiB Rust guest, ~350 ms for
the 1.4 MiB quickjs engine (release, wasmtime 47) — plus one
throwaway probe instantiation before every real one. Invisible at
batch CLI cadence; dominant the moment passes run per serve request.

The owner's demotion is on record and accepted: platformc's
fresh-actor-per-invocation is its own determinism law, and the
platformc contract could assume statelessness is clean to dodge the
cost entirely. The MWYDYN-side debt is different and does not depend
on platformc: **the public constructor conflated "load guest code"
with "create a guest actor."** Sharding — N distinct actors with
their own mailboxes and cursors from one codebase — is not fork
(fork is lineage; ghost branches are reads); construction is the
only path, and construction was O(compile) per shard. Meanwhile the
pump's fork path already instantiated from a held module since
round 34/37 (round 33's complaint was re-instantiation cost, never
recompile): the split existed internally, `from_compiled` just never
got it. The fix serves a core actor pattern; serve-mode economics
fall out as a side effect.

## 2. The arbitration (owner, in session)

Three candidate shapes were on the table:

- **`from_precompiled(engine, Module, GuestMode)`** — the handoff's
  sketch. REJECTED: `GuestMode` is a pub enum, and a public
  mode-TAKING constructor is the round-41 override path reborn — the
  type system cannot tell *carrying the probe's result* from
  *forging one*, and `from_precompiled(engine, module,
  GuestMode::Stateless)` on a stateful-declaring module is exactly
  the silent exactly-once break round 41 killed by removing
  `from_compiled_stateful`. The handoff's own numbers show the
  GuestMode-carrying buys almost nothing: the probe is 0–3 ms of
  350 ms — noise. Carrying a forgeable parameter to save noise is a
  bad trade.
- **`from_module(engine, Module)` re-probing per spawn** — law-safe,
  smallest delta, but every spawn pays a throwaway instantiation and
  the engine/module pairing is left to the caller (a wasmtime Module
  is engine-bound; handing the wrong pair is a runtime error waiting
  for a serve loop).
- **The opaque `GuestArtifact`** — RATIFIED (owner, with the
  recommendation). Compile once, probe once, store the mode in a
  PRIVATE field; `Clone` is Arc-cost; `WasmActor::from_artifact`
  reads the stored mode. No public constructor anywhere TAKES a
  mode, so §9.2's "no embedder override exists" stays literally
  true with the split made public.

## 3. The shape

`GuestArtifact { engine: Engine, module: Module, mode: GuestMode }`
(private fields, `#[derive(Clone)]` — both innards are Arcs in
wasmtime):

- `GuestArtifact::compile(&WasmEngine, &[u8])` — the plugin-LOAD
  path: compile + probe in one step, so a bad module or a trapped
  declaration read surfaces at load, not at first spawn.
- `GuestArtifact::from_module(&WasmEngine, Module)` — same probe for
  a module the embedder compiled itself (and the engine pairing is
  the artifact's job, not the caller's).
- `GuestArtifact::mode()` — read-only, diagnostics.
- `WasmActor::from_artifact(&GuestArtifact)` — instantiation-only
  spawn: Arc-cloned module, stored mode. Instantiation stays LAZY
  (round 37's law): `from_artifact` is pure assembly, the first
  exchange pays the real instantiation (~120 µs, round 37's number).

`from_compiled` and `from_wat` survive as one-shot conveniences, now
literally `from_artifact(&GuestArtifact::compile(...))` underneath;
`probe_mode` demoted from an associated fn to a private free fn next
to `build_linker` — the probe's result now lives in exactly one
place, inside the artifact.

## 4. The pins (`mwydyn-wasm/tests/artifact.rs`)

- **THE LAW** — `the_declaration_rides_inside_the_artifact`: the
  round-41 WAT probe table (absent / bit 0 / explicit zero) driven
  through `from_module`; clones carry the same declaration; spawned
  actors agree via `fork_carries_state`.
- **THE SHARDING SHAPE** — `one_artifact_many_shards`: one
  `GuestArtifact::compile`, two named counter shards, independent
  instances — both count 1. The artifact is probed Stateful at
  COMPILE time (`durable_entry!` declares it), pinning that the
  probe ran once, at artifact construction.
- **THE COST BOUNDARY** — `spawn_from_artifact_never_compiles`:
  twenty `from_artifact` spawns must cost less than ONE raw
  `Module::new` on the same bytes. Measured (debug, this box): one
  compile 442.8 ms, artifact (compile+probe, warm caches) 251.9 ms,
  20 spawns 3.4 µs — **170 ns/spawn**, pure assembly. The honest
  caveat is in the test's doc: this pin guards the compile+probe
  class (the debt); the lazy instantiation is the first exchange's
  cost by design and is round 37's territory.

## 5. Latent defect found and fixed: the born-skipped pin

The workspace run (this round's, the first full one since round 47's
script began staging the uncomposed engine) failed
`mwydyn-js::compose_engine_and_script_yields_a_valid_module` —
clean main included. The round-46 pin scanned
`seg.data.windows(5).any(|w| w == b"mwydyn")`: a 5-byte window can
never equal a 6-byte literal, so the assertion could NEVER pass; the
test read green at round 46 only because the engine artifact was
absent (skip-with-message), and began failing the moment round 47's
build script staged the engine on this box. Fix: the pin now asserts
the FULL script lands VERBATIM in the composed data section — the
stated intent, actually enforced.

The class is worth naming: **a skip-with-message pin is green until
its artifact exists — the first full run after staging is its real
first run.** Same family as round 47's "zero tests until a real
consumer crossed the surface", one layer down: the coverage claim
itself was untested. (Also picked up in passing: round 47's
`js_panic_stash` had a `clone()` on wasmtime 47's now-`Copy`
`Global` — clippy's word, fixed.)

## 6. Consumer mapping

platformc's `load_plugin` compiles + probes ONCE per plugin (its
sha256 pin and early error wants stay consumer-side), hands
`artifact.clone()` per serve spawn — ADR-0019 debt 1 closes, its
fresh-actor-per-invocation determinism law untouched. The optional
serialized-artifact (cwasm) cross-process cache stays OUT of scope,
as the handoff itself deferred it — in-process Module reuse is the
fix; serialization is a separate conversation.

Rounds 49 (JS SDK 1.0 surface: prelude CBOR codec first, then the
three round-43 fork ratifications — owner-ratified bundle) and 50
(tasks meet the ambient surface: Spawn-threaded `call_task_with`,
zero-grant default named, correlation mint already core-side) are
queued from the same handoff.
