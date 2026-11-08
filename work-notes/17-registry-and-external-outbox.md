# 17 — The registry ships; external gets an inbox; call_task is a verb

> Status: implemented and green (2026-11-08). 12 walking + 4 envelope
> + 2 corpora + 2 schema unit tests, clippy clean. Amends §3.4 with
> the external outbox as the embedding's handle (see §1).

## 1. What landed

- **`mwydyn-schema`** — the registry (§7.9): `SchemaDef` (id + CDDL
  verbatim + optional summary) with its canonical CBOR projection
  and pinned digest; `claim` (idempotent for identical digests —
  composition is commutative; conflicts refuse loudly);
  `compose` (embeddings over the runtime); `core_schemas()` ships
  `mwydyn.timer.{arm,tick,poll}@1` with real CDDL text (durations
  never timestamps, §6). v0 scope note: the registry carries and
  pins; payload validation AGAINST CDDL is M1+ (parser). Digest
  stability across registries is a test.
- **Contexts seed their registry** with the core schemas; embeddings
  compose via `claim_schema`/`Registry::compose`. The core still
  never consults it (schema-blind, round 10) — it exists for
  embeddings and tooling (typed views, compatibility checks).
- **The external outbox** — the piece the task pattern was missing.
  Posts to `Instance("external")` used to strike (unknown-target);
  now they land in a drainable outbox. This is §3.4's embedding
  handle made concrete: unnamed actors reply to their spawner, the
  spawner (an embedding!) drains. `drain_external()` is the API.
- **`spawn_task` + `call_task`** — the task pattern as verbs:
  unnamed, fail-fast, RAM-tier spawn; and the one-call round trip
  (spawn → post under a minted correlation → run to quiescence →
  return the reply from the outbox).

## 2. Ripple honesty

The old `permanent_refusals` echo test relied on replies-to-external
STRIKING. With the outbox, they land — the test became
`replies_to_external_land_in_the_drainable_outbox`. The strike path
stays covered by the atomic test (nonexistent instance target) and
the restart test (poison strikes).

## 3. Next (M2)

The journal face: cursor leaves memory, contiguous seq arrives,
journaled ticks make rewind real — behind the FACE the owner
demanded (single-node SQLite first, distributed later, consensus
never in the way).

## Provenance

Owner "Onwards!", 2026-11-08. Sources: A1 work-notes 05 (seam spec
format), 45 (reserved ids / binding-swap precedent).
