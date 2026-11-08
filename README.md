# mwydyn

An embeddable WASM actor runtime: **messages as data**, actors as the
core primitive, durability as a tier — not an option. mwydyn is a
library first; distributions (CLI embed, the Athena2 kernel, a future
PaaS) compose on top of it.

Named in 2019 (`C:\projects\mwydyn.old` — the warpstone era), revived
2026-11-08 as the substrate carved out of the Athena2 design rounds.

## Why

Athena1 accreted three execution stories (QuickJS, WASM, browser JS)
and a post-hoc blob layer. mwydyn is the clean replacement: ONE actor
contract, WASM guests via the component model, native host services as
actors-by-interface, blobs as the first well-known service.

## Consumers (the design matrix)

| Consumer | Shape | Stress-tests |
|---|---|---|
| Platform compiler | task-shaped, parallel pure transforms | zero-cap scoping, fan-out, hot-path perf, contract pinning |
| Docgen | task-shaped, heavy plugins | on-demand install/bundles, deterministic render, CLI embed (RAM tier) |
| Athena2 hosted | long-lived session actors, multi-user | time-travel layer, hot composition, durable mailboxes |
| Athena2 local | same actors, in-process, single binary | flatten: same bytes, different hosting |
| PaaS (idea) | scale-to-zero, agent-operated | parking/snapshot, cold start, machine-operable control plane |

Design rule: the **intersection** of consumer needs is the core; the
**union** goes behind layers.

## Status

Spec v0 skeleton + charter, no code yet.

- [docs/spec-v0.md](docs/spec-v0.md) — the contract (DECIDED / OPEN sections)
- [work-notes/00-charter.md](work-notes/00-charter.md) — founding decisions
