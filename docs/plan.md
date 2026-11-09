# mwydyn — walking skeleton plan (M-slices)

> Status: ACTIVE (green-lit round 11, 2026-11-08). M0 in progress:
> workspace scaffolded, `mwydyn-data` canonical CBOR profile built,
> cbor-profile-v0 corpus green (`cargo test`).

## Round-11 directives (owner)

- **Compose-parity law**: native Rust actors exist anyway (performance-
  critical sections); what matters is that they compose IDENTICALLY
  to WASM actors — same `receive → effects` trait, same admission,
  same semantics. M0 native / M1 WASM split exists to keep that
  interface honest (and non-wasmtime-specific). The frozen world
  RESISTS extension, which is what makes additional guest languages
  cheap later.
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
- `crates/mwydyn-wasmtime` — guest hosting of world-v0 (bindings,
  decode at the barrier, epoch/fuel budgets). Feature-gated.
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
  39). Remaining: the ambient host-face import surface.
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
