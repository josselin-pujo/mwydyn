# SBOM & dependency policy

The SBOM discipline mirrors the §8 provider-admission discipline:
**a dependency is admitted by its license satisfying the policy;
everything else is refused, by name.** The strictness lives in the
exclusion list.

## What lives here

- `deny.toml` (repo root) — the cargo-deny policy: the license
  admission list (round 24), copyleft refusal list, advisory
  checks. Run `cargo deny check licenses` (install once:
  `cargo install cargo-deny --locked`).
- `dependency-tree-YYYY-MM-DD.txt` — periodic snapshots of the
  full tree (`cargo tree --workspace -e normal`). The CURRENT tree
  is small (~30 transitives incl. bundled SQLite); the point of
  starting now is that **wasmtime alone will add ~200** — the gate
  must exist before that lands.

## The licensing decision (round 24)

- Project license: **pure Apache-2.0** (owner preference over the
  Rust MIT convention). The one known cost: Apache-2.0 is
  GPLv3-compatible but **not GPLv2-compatible** — accepted,
  irrelevant for this ecosystem. The benefit: an explicit patent
  grant, meaningful for a sandboxed-runtime substrate.
- Consumption policy for dependencies: prefer `MIT OR Apache-2.0`
  dual crates (choose Apache), accept Apache-only and MIT-only,
  **refuse GPL/AGPL/SSPL in LINKED dependencies**. External
  services (a NATS or Kafka server) are separate processes — their
  licenses do not propagate; only linked crates do.

## The meaty-dependency forecast (all checked, all admissible)

| Dependency | License | Notes |
|---|---|---|
| wasmtime (+cranelift, wasmparser, regalloc2, rustix) | Apache-2.0 WITH LLVM-exception | patent grant + explicit component-linking clause; adds ~200 transitives — hence the gate now |
| tokio (+mio, socket2, bytes, parking_lot) | MIT OR Apache-2.0 | clean |
| hyper / h2 / axum / http / tower | MIT (dual where offered) | clean |
| quinn (QUIC) | MIT OR Apache-2.0 | clean; rustls dual too — **crypto provider choice**: prefer aws-lc-rs (Apache-2.0 OR ISC) over ring (oddball OpenSSL/ISC/BSD mix — permissive but a SBOM footgun) |
| rusqlite | MIT | bundled SQLite itself is PUBLIC DOMAIN — the cleanest license in the tree |
| V8 (rusty_v8/deno_core), if ever | MIT crate + BSD-style V8 (+ICU) | permissive but a heavy C++ build |
| JavaScriptCore, if ever | MIT crate but **libjavascriptcoregtk is LGPL-2.1+** | linked-LGPL compliance burden + GTK-stack-on-Windows pain — a second reason (beyond the owner's WASM preference) to keep JS-on-WASM |
| QuickJS-ng, the license-friendly JS fallback | MIT | A1's engine; small, embeddable |
| kafka client (rdkafka) | Apache-2.0 crate, BSD-2 lib | lib is a dynamic/optional integration |
| NATS client (async-nats) | Apache-2.0 | clean |

## Licensing law (in AGENTS.md, round 24)

Apache-2.0 project; linked-copyleft refused; external services
don't propagate; every new dependency passes `cargo deny check
licenses` before it merges.
