# mwydyn — walking skeleton plan (M-slices)

> Status: ACTIVE (green-lit round 11, 2026-11-08). M0 in progress:
> workspace scaffolded, `mwydyn-data` canonical CBOR profile built,
> cbor-profile-v0 corpus green (`cargo test`).

## Round-11 directives (owner)

- **Compose-parity law**: native Rust actors exist anyway (performance-
  critical sections); what matters is that they compose IDENTICALLY
  to WASM actors — same `receive → effects` trait, same admission,
  same semantics. M0 native / M1 WASM split exists to keep that
  interface honest (and non-wasmtime-specific). The frozen guest
  ABI (spec §9) RESISTS extension, which is what makes additional
  guest languages cheap later.
- **Faces, not implementations (M2/M3)**: the journal and blob
  concepts are universal; implementations are not. Single-node
  SQLite is v0; a distributed journal (raft or lolPaxos) and tiered
  blob stores come later BEHIND THE SAME FACE — the consensus
  machinery must never be in the way of the single-node version.
  (The A1 runtime-crate pattern: narrow boxed faces, host-injected
  at one wiring point, no domain types across.)

## Workspace layout

- `crates/mwydyn-data` — canonical CBOR profile (encode/decode,
  key hygiene, limits), `Value`, the `blob-ref` type, envelope
  structs, checksums. Zero mwydyn dependencies.
- `crates/mwydyn-schema` — CDDL load (authoritative) ⇄ CBOR
  projection; the registry (compose, claim namespaces, conflict
  refusal, digest pinning). Schema-blind core consumes none of it.
- `crates/mwydyn-core` — contexts, mailboxes/cursors, the effects
  engine, structural admission, supervision policies (`fail-fast`,
  `restart`), the ticker, the blob store, the task pattern. The
  runtime library — usable without any WASM.
- `crates/mwydyn-wasm` — guest hosting over wasmtime: the CBOR
  barrier container (lazy persistent instances, declared
  statefulness, §7.6 snapshot ABI), the ambient import dispatch,
  epoch budgets. Peer to the core, never in it.
- `crates/mwydyn-vectors` — the vector runner (JSON vectors, CBOR as
  hex + diagnostic notation); consumed by tests AND external
  implementations.
- `vectors/` — the corpora. `crates/` before `docs/` never; docs
  stay authoritative (AGENTS law).

## M-slices

- **M0 — native walking skeleton (RAM tier).** `mwydyn-data`
  (canonical CBOR, envelope, checksums) + `mwydyn-core` (contexts,
  mailboxes/cursors, admission, the effects engine, atomic in-proc
  commit, `fail-fast` + `restart`, the ticker, the RAM blob store,
  the task pattern) — driven by **native Rust actors implementing
  the `receive → effects` trait**: the world's exact shape without
  the WASM boundary. Vectors: cbor-profile (ported from A1),
  schema-registry, letter→verdict, receive-purity.
- **M1 — wasmtime guests.** ~~world-v0.wit bound for real~~
  SUPERSEDED (round 33): the component model is gone — CBOR all
  the way, plain `wasm32-unknown-unknown` guests, zero imports,
  barrier contract in `mwydyn-data/src/barrier.rs`. Landed: the
  barrier (request/response + payload wall), the container, the
  guest-minimal work-order transform, byzantine-crash proof,
  CBOR-tier throughput. Remaining: epoch/fuel budgets armed per
  exchange (executor round); barrier conformance vectors; the
  ambient host-face import surface (blob_get etc. across the
  barrier).
- **M2 — log tier.** The context journal: contiguous `seq`,
  per-message checksums, cursor-over-journal inboxes, journaled
  ticks, rewind — the time-travel layer's first mechanical form.
  §7.6 (snapshot contract) lands here, nearly free per round 07.
- **M3 — blob persistence + reference-aware GC.** Sweep only above
  the live watermark; loud-missing proven by vectors.
- **Executor + IO pipelining (rounds 34–38, LANDED):** worker pool
  (wave dispatch at depth, dispatch-order commits, CQRS face) +
  durability overlay policy, verified at the SQLite boundary;
  §7.6 guest snapshot ABI + overlay-v0/barrier-v0 vectors (round
  39); the ambient host-face import surface (round 40, `mwydyn.
  ambient` + the mwydyn-guest SDK). Remaining: ambient conformance
  vectors (ambient-v0); mwydyn-stdlib (the merge actor).
- **Guest SDK — Rust (round 41, spec §9.6):** author-facing
  `mwydyn-guest` (Actor/Durable traits, `entry!`/`stateful_entry!`/
  `durable_entry!`, typed `Cx`/`Effect`, one-dependency prelude);
  statefulness becomes GUEST-DECLARED (`mwydyn_decl` export, spec
  §9.2 — `from_compiled_stateful` removed); counter/store migrate
  onto the SDK as living examples, minimal stays the byzantine
  fixture; new stateful-ephemeral example; docs-as-product (the
  crate doc is the guide). **ambient-v0 vectors LANDED (round 42):**
  the §9.4 contract as data — canonical verb forms, refusal table,
  response envelope spellings (generator + harness, barrier-v0's
  pattern). Remaining: mwydyn-stdlib (the merge actor); guest SDKs
  beyond Rust.
- **Telemetry outbox (round 44, spec §3.6 — LANDED 2026-11-10,
  three commits):** closed-kind
  records (`log | metric | exchange | lifecycle | refusal | drop`),
  runtime-minted `ts`/`actor`/`seq`, core-attached lineage (one
  `exchange` record per delivery attempt = the span skeleton),
  shared bounded ring + `drain_telemetry()` with leading `drop`
  records, ungated emission, the injection handle (`post` returns
  seq, accepts a core-validated parent). OTel mapping stays out of
  core, out of scope. Landed as three commits, each independently
  green (36 → 36 → 37 suites):
  1. **data + core** — `mwydyn-data/src/telemetry.rs` (record +
     canonical forms + guest-declare subset); core: `Effect::Emit
     { record }` + helpers, the ring/drain/drop ledger, exchange/
     lifecycle/refusal minting in the pump paths, injection handle.
     Done when: workspace tests + clippy green with the old
     `Vec<(Level, String)>` gone.
  2. **barrier + SDK** — barrier response grows the additive
     `emit` verb (guest-declarable kinds `log|metric` ONLY — the
     runtime kinds are refused at the barrier); wasm container
     maps it; `mwydyn-guest` gains `Effect::emit`/`log`/metric
     helpers. Done when: a guest's emit lands in the drain
     stream, stamped by the core (compose-parity proof).
  3. **vectors** — `telemetry-v0` corpus (canonical forms per
     kind, unknown-kind refusal, drop + exchange forms, the
     guest-declare form without minted fields) + generator +
     harness. Done when: `cargo test -p mwydyn-vectors` green on
     the committed corpus.
- **M4 — the A1 corpus, conformance-green.** `cbor-v1.json` et al.
  pass in `mwydyn-vectors`; every DECIDED spec section has vectors;
  spec v0 declared complete.

## First concrete work (on green light)

1. Verify the toolchain first: rustc version on this box (A1's note:
   rustc 1.95 pinned, wasmtime 49 wanted 1.96 → A1 pinned wasmtime
   48.0.2). Choose the wasmtime pin accordingly — for M0 it only
   matters as a footnote; for M1 it matters immediately.
2. Scaffold the workspace; `mwydyn-data` first, zero deps beyond the
   CBOR library.
3. Port/adapt the cbor-profile vectors; make them pass. Then
   letter→verdict vectors against a stub admission path.
