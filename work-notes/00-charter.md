# 00 — Charter: mwydyn, extracted greenfield

> Status: decided (charter round 2026-11-08, with the project owner).
> This note records the founding decisions; the contract lives in
> [docs/spec-v0.md](../docs/spec-v0.md).

## 1. Why mwydyn exists

Athena1 (`C:\projects\Athena`) neared 1.0 carrying three plugin systems
(QuickJS, WASM, browser JS), a post-hoc blob layer, display-independent
UX baggage, and a desktop-first architecture the hosted tier had to be
surgically spread from. The Athena2 greenfield risk — second-system
syndrome — was defused by inversion: build the substrate FIRST, as a
thing with independent consumers, and let Athena2 be its most demanding
adopter rather than its reason to exist.

## 2. Founding decisions

1. **Standalone project**, named mwydyn (the 2019 name revived —
   `C:\projects\mwydyn.old` holds the warpstone-era ancestor; prior-art
   mining optional).
2. **Actors are the core primitive**: inbox + outbox + telemetry
   outbox. Task actors are a first-class derived pattern for the
   function-shaped consumers.
3. **Messages are data**: Athena1's canonical CBOR profile, 1 MiB hard
   cap, envelope-only routing, host-side decode at the WASM barrier.
4. **Two type layers**: frozen WIT protocol world + versioned CBOR
   schema registry. Evolution happens in layer 2, never in WIT.
5. **Runtime-owned durability, tiered** — inbox/outbox are always
   durable objects; RAM → disk per actor.
6. **Blob service first**: well-known address, collocated, hardcoded
   Rust, actor-by-interface-only, content-hash idempotent.
7. **Stateful actors**: blob-self-state OR the suspendable contract
   (replay-backed until wasmtime live snapshots verify).
8. **Runtime: wasmtime**; epoch/fuel/memory knobs per A1's spike
   [A1:40].
9. **Athena2 waits for mwydyn** — the actor system is the substrate,
   not just the plugin story.
10. **Spec + walking skeleton in lockstep** — an RFC-first marathon is
    the twin of second-system syndrome.
11. **The PaaS is a distribution, not a dependency** — Athena2 embeds
    the runtime directly; "Athena2 on the PaaS" is a later deployment
    choice, never an architectural one.

## 3. The consumer matrix

| Consumer | Shape | Stress-tests |
|---|---|---|
| Platform compiler | task-shaped, parallel pure transforms | zero-cap scoping, fan-out, hot-path perf, contract pinning |
| Docgen | task-shaped, heavy plugins | on-demand install/bundles, deterministic render, CLI embed (RAM tier) |
| Athena2 hosted | long-lived session actors, multi-user | time-travel layer, hot composition, durable mailboxes |
| Athena2 local | same actors, in-process, single binary | flatten: same bytes, different hosting |
| PaaS (idea) | scale-to-zero, agent-operated | parking/snapshot, cold start, machine-operable control plane |

Design rule: the **intersection** of consumer needs is the core; the
**union** goes behind layers.

## 4. Spec recovery from Athena1

The specifications worth recovering (not the implementations):

- canonical CBOR profile [A1:04 §1]
- envelope discipline, byzantine posture, cause/correlation,
  correlation-dedup [A1:04]
- seam spec format: purpose / vocabulary / roles / invariants +
  orthogonality note [A1:05]
- blob reference shape + the digest layering law [A1:05, A1:51]
- watermark-stamped snapshots + bounded replay + fork-on-loss [A1:21 §5]
- walled-egress posture: absence over policy [A1:21]
- wasm budget knobs [A1:40]
- test vectors as the conformance harness (`cbor-v1.json` ports first)

## 5. Provenance

Charter discussion with the project owner, 2026-11-08. Sources read:
Athena1 `AGENTS.md`, `work-notes/04`, `work-notes/05`,
`work-notes/21`; `mwydyn.old` directory listing.
