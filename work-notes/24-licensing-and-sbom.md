# 24 — Licensing: Apache-2.0, and the SBOM gate starts now

> Status: decided + implemented (2026-11-08). LICENSE vendored
> (canonical text from apache.org), license fields on every crate,
> `deny.toml` admission policy written, `cargo deny check licenses`
> GREEN, first dependency-tree snapshot in `sbom/`.

## 1. The decision

Project license: **pure Apache-2.0** (owner preference over the
Rust MIT convention). The formal analysis:

- **Benefit**: explicit patent grant + patent-retaliation clause —
  meaningful protection for a sandboxed-runtime substrate.
- **The one known cost**: Apache-2.0 is GPLv3-compatible but NOT
  GPLv2-compatible (the patent-termination clause). Accepted as
  irrelevant for this ecosystem.
- **Redistribution bookkeeping**: NOTICE preservation + change
  statements — trivial, and `cargo-about` can generate notices for
  the single-binary repackage later.

## 2. The dependency forecast (all checked, all admissible)

| Dependency | License | Verdict |
|---|---|---|
| wasmtime (+cranelift, wasmparser, regalloc2, rustix) | Apache-2.0 WITH LLVM-exception | admissible; adds ~200 transitives — hence the gate NOW |
| tokio (+mio, socket2, bytes, parking_lot) | MIT OR Apache-2.0 | clean |
| hyper / h2 / axum / http / tower | MIT (dual where offered) | clean |
| quinn + rustls | MIT OR Apache-2.0 | clean; **crypto provider**: prefer aws-lc-rs (Apache-2.0 OR ISC) over ring (OpenSSL/ISC/BSD mix — permissive but a SBOM footgun) |
| rusqlite | MIT | bundled SQLite is PUBLIC DOMAIN — cleanest in the tree |
| V8 (rusty_v8/deno_core) | MIT crate + BSD-style V8 (+ICU) | permissive, heavy build |
| JavaScriptCore | MIT crate but **LGPL-2.1+ linked lib** | the real flag: linked-LGPL compliance + GTK-on-Windows pain — a second reason (beyond WASM preference) to skip it; QuickJS-ng (MIT, A1's engine) is the license-friendly fallback |
| rdkafka / async-nats | Apache-2.0 crates, BSD/Apache libs | clean; external servers don't propagate anyway |

The consumption law (in AGENTS.md): **linked copyleft refused;
external services don't propagate — only linked dependencies do.**
Consume `MIT OR Apache-2.0` duals as Apache; Apache-only and
MIT-only are both fine into an Apache project.

## 3. Implemented

- `LICENSE` (canonical Apache-2.0, vendored from apache.org),
  `license = "Apache-2.0"` on the workspace and all crates.
- `deny.toml` — the LICENSE ADMISSION LIST: cargo-deny ≥0.20 is
  admission-only (deny lists were removed upstream), which suits
  mwydyn exactly (§8 thinking applied to dependencies): everything
  not admitted is refused by name in review. Copyleft (GPL/AGPL/
  LGPL/SSPL) deliberately OUTSIDE the list.
- `cargo deny check licenses` → **licenses ok** (first run, whole
  tree, including the bundled-SQLite subtree).
- `sbom/dependency-tree-2026-11-08.txt` — the first tree snapshot;
  `sbom/README.md` — the policy + forecast table.
- Tooling: cargo-deny 0.20.2 installed (`cargo install
  cargo-deny --locked`).

## 4. Next

- `cargo deny check` joins the gates (with `cargo audit` once
  advisories matter — first real consumer deployment).
- SPDX/CycloneDX emission (`cargo sbom` / `cyclonedx-rust`) when a
  distribution actually ships.

## Provenance

Owner question ("licensing dependencies + SBOM, prefer Apache-2.0,
contraindications for wasmtime/tokio/QUIC/V8-JSC?"), 2026-11-08.
