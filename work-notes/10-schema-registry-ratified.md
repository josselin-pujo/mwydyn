# 10 — Schema registry ratified; the skeleton plan

> Status: §7.9 DECIDED (round 2026-11-08) — the last critical-path
> fork. The shopping list is complete; planning for the walking
> skeleton begins: [docs/plan.md](../docs/plan.md) (PROPOSED,
> awaiting green light). Amends [spec-v0](../docs/spec-v0.md)
> §7.9, §3.5.

## 1. Ratified — the registry, with the owner's refinements

- **CDDL authoritative; canonical CBOR projection** for storage and
  orthogonality — deterministic mapping, roundtrip-vector-pinned.
  The registry practices A1's projection law (JSON/CBOR §1) on
  itself.
- **Additive-within-major** — non-breaking evolution matters for a
  system that composes sources and sinks from **distinct delivery
  lifetimes** (owner).
- **Optional consumed-kind declarations, and NO core enforcement** —
  owner's law: **"CBOR is just CBOR."** From the core's POV, `kind`
  is an opaque string; payloads are opaque bytes under structural
  envelope laws. The registry stays queryable metadata (library API
  + external tooling for compatibility checks and typed views) but
  is never admission-enforced and never guest-visible in v0 — the
  frozen world is untouched.
- Ripple: §3.5's admission validation drops schema validity
  (structural laws only); the core is schema-blind.

## 2. The skeleton plan (see docs/plan.md)

- Workspace: `mwydyn-data`, `mwydyn-schema`, `mwydyn-core`,
  `mwydyn-wasmtime` (feature-gated), `mwydyn-vectors`; `vectors/`.
- **M0 = native walking skeleton (RAM tier)**: native Rust actors
  implement the `receive → effects` trait — the world's exact shape
  without the WASM boundary; core semantics (admission, effects,
  atomic commit, cursors, ticker, task pattern) validated with fast
  actors first. **M1 = wasmtime guests**; M2 = log tier (journal,
  rewind); M3 = blob persistence + reference-aware GC; M4 = the A1
  corpus conformance-green.
- Vectors-first discipline throughout (charter decision #10).

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §1 (projection law), 04 §7 (additive evolution);
docs/plan.md conventions from A1's docs/README.
