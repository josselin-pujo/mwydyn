# 43 — The JS/TS guest SDK (QuickJS-ng on WASM)

> Status: ENGINE MILESTONE LANDED (2026-11-10, part 1 of the
> phasing below): quickjs-ng v0.18.0 compiles freestanding to
> wasm32-unknown-unknown and evaluates real JS under wasmtime —
> arithmetic, JSON escaping, Math.pow, RegExp, frozen Date,
> exception recovery — on the Rust-owned heap. Host engine tests
> green at native speed; workspace 35 suites, clippy clean.
> Remaining: wrapper/conversion/prelude (phase 2-3), build tool,
> TS package, examples. Direction owner-ratified in session:
> JS guest next, TYPESCRIPT FIRST-CLASS — the types are a
> conformance aid, not documentation. Forks below carry DEFAULTS IN
> FORCE, none individually ratified (owner unanswered in session —
> same protocol as §9.5: defaults ship, arbitration recorded).
> Implementation gated on the TOOLCHAIN prerequisite (§6).

## 1. The law the round must keep

**The host never learns that JS exists.** The artifact is a plain
§9 wasm guest: same exports, same declaration, same barrier, same
caps. §9.6's three-words law gets its first second-language test:
`mwydyn.entry(actor)` / `mwydyn.statefulEntry(actor)` /
`mwydyn.durableEntry(actor, {snapshot, restore})`, called at module
top level. Zero-import law intact: quickjs-ng compiled FREESTANDING
to `wasm32-unknown-unknown` — no WASI, nothing to fake.

## 2. The artifact path (TS → .wasm)

1. Author writes TS against `@mwydyn/guest`, transpiles with their
   own tsc/esbuild (standard JS workflow, ZERO Rust) → `script.js`.
2. `mwydyn-js build script.js -o guest.wasm`: takes the prebuilt
   engine+wrapper wasm (built once per SDK release by the repo's
   build script), asks the module — ONE throwaway wasmtime
   instantiation — where its script slot lives (the wrapper exports
   `mwydyn_script_slot() -> (ptr, cap)`), then APPENDS an active
   data segment writing the script bytes there (wasm-encoder).
   No fixed addresses (the probe learns them), no host changes,
   self-contained artifact. Oversized script → loud tool refusal.
3. The host composes the result like any wasm guest.

Custom-section alternatives were rejected: guests cannot read their
own custom sections, and host-side script handoff would make the
container script-aware — an ABI change for a build concern.

## 3. The pieces

- **`crates/mwydyn-guest-js`** (cdylib, guest-realm peer of
  mwydyn-guest — core untouched): builds ON the round-41 SDK (its
  entry macros generate the exports). Owns:
  - engine lifecycle: boot quickjs-ng once (lazy), evaluate the
    PRELUDE then the script (from the script slot), persistent for
    the artifact's lifetime — round-37 laziness applies;
  - the CONVERSION table (Value ⇄ JS): Null↔null, Bool↔boolean,
    Int↔number (±2^53; beyond → BigInt — see fork 3), Float↔number,
    Text↔string, Bytes↔Uint8Array, BigPos/BigNeg↔BigInt,
    Timestamp↔µs number (branded type), Map↔plain object (fork 2
    residue: key order is the canon's, not the object's),
    BlobRef↔frozen `{digest, mediaType, size}`, Array↔Array;
  - ambient bridging: one intrinsic (`__mwydyn_ambient(reqObj)`)
    exposed to JS; the wrapper CBOR-encodes, calls the §9.4 import,
    decodes — authors see `cx.blobGet(ref)` etc., never bytes;
  - the three words + registration state (the registered actor);
  - durability: `snapshot()`'s JS value converts to Value →
    canonical CBOR (state format is the actor's own; CBOR keeps
    Uint8Array/timestamps alive where JSON would flatten).
- **The prelude** (a JS source const in the wrapper): defines the
  global `mwydyn` API — entry registration, `Effect.post/reply/
  replyVia`, the `Cx` surface (`sender`, `blobPut/blobGet/blobLen`,
  `caps` — refusals THROWN as `MwydynRefusal` with the code),
  `fail()`. ALL runtime code lives here (fork 1).
- **`@mwydyn/guest` npm package**: `.d.ts` + compile-time helpers
  ONLY (fork 1 default): `Actor`/`DurableActor` interfaces, `Letter`,
  `Cx`, `Effect`, `BlobRef`, `MwydynRefusal`, and `declare global {
  var mwydyn }` — the editor enforces the three words and the
  effect shapes BEFORE transpile; conformant-by-construction is the
  TS-first promise.
- **`mwydyn-js` build tool** (host bin): wasmtime (probe) +
  wasm-encoder/wasmparser (append the segment). All Bytecodealliance,
  Apache-2.0/MIT — admission list clean.
- **Example guests + tests**: TS echo (stateless), TS dedup
  (stateful — the dual proof shape from round 41), TS counter
  (durable). Built via `scripts/build-guest-js.sh` (tsc + tool);
  tests skip-with-message if the artifact is absent (house
  convention). The Rust SDK's test matrix re-runs against them.

## 4. The forks (defaults in force, unratified)

1. **PACKAGE SHAPE — types-only + injected prelude** (default):
   the npm package is types + a declared global; all runtime code
   is the wrapper's prelude. No version skew (one runtime per
   artifact), the artifact carries exactly one implementation.
   Rejected: runtime-in-package (familiar npm ergonomics, but two
   runtimes per artifact and skew risk between bundle and prelude).
2. **PAYLOAD TYPING — static types + bring-your-own-guard**
   (default): `letter.payload` is typed by the author's `TPayload`
   parameter; RUNTIME validation is the author's guard
   (`letter.payloadOf(guard)` — refuses loudly = strike on
   malformed shape). Rejected: shipping a zod/io-ts dependency;
   rejected: host-side CDDL validation at admission (the core is
   schema-blind — law).
3. **I64 BOUNDARY — number when safe, BigInt beyond** (default):
   Ints within ±2^53 arrive as `number`, outside as `bigint`; the
   TS types say `number | bigint` with a lint rule nudging authors
   to widen early. Rejected: always-number (silent lossy),
   always-BigInt (ergonomic tax on the common case).

## 5. TS-first conformance aids (the owner's emphasis)

- The three words are the ONLY registration calls; anything else
  fails the build (types) or the boot (prelude throws = strike).
- `Effect.post` overloads take `kind: Kinds<T>` template-literal
  types — a typo'd kind fails COMPILE. (Best-effort: the registry
  isn't TS-visible; authors may widen to `string`.)
- `receive(letter, cx): Effect[]` — the return type IS the effect
  list law; effects are returned, never called (parity with Rust).
- Experimental APIs stay unlisted in the .d.ts — if types can't
  see it, conformant code can't call it.

## 6. Toolchain prerequisite (BLOCKING)

The engine is C: compiling quickjs-ng for wasm32-unknown-unknown
needs a C compiler, and this box has NONE (no clang, no zig, no
VS — Rust rides GNU). Options:
- **zig cc** (RECOMMENDED): single zip on PATH, `zig cc -target
  wasm32-freestanding` via the cc crate's CC env — no install
  ceremony, MIT license.
- LLVM/clang: winget LLVM.LLVM — heavier, equally clean via cc.

Owner picks (or installs); `build.rs` auto-detects and fails loudly
with instructions otherwise. quickjs-ng SOURCE is vendored into the
crate (pinned release, checksum recorded) — the build never fetches.

## 7. Phasing (implementation order)

1. Toolchain + vendored quickjs-ng + build.rs (the risk front-loaded:
   freestanding shims — memcpy/memset/libm bits — until the engine
   evaluates a hello script under wasmtime).
2. Wrapper: engine lifecycle + conversion table + the barrier
   exchange against a hand-registered actor (no prelude yet).
3. Prelude + the three words + ambient bridging; the Rust-SDK test
   matrix (echo/dedup/counter shapes) against JS actors.
4. The build tool (script-slot probe + segment append) + TS package.
5. TS examples + scripts + full suite; docs (AGENTS state, plan.md).

## 8. The lessons part 1 taught

- **THE NUL WALL**: quickjs's lexer peeks one byte PAST the input
  (`p[1]` lookaheads); a non-terminated Rust `&str` leaks adjacent
  rodata into the parse — `1` failed, `1000000` parsed, depending
  on the neighbor byte. Ground truth won it: a C driver against
  the SAME objects was perfect, isolating the FFI layer. Inputs
  and filenames now cross NUL-terminated.
- **C11 6.2.1 tag scope**: a struct tag FIRST DECLARED inside a
  prototype is prototype-scoped — the later file-scope definition
  is a second, incompatible type. Forward-declare tags before any
  prototype mentions them (the compat headers now do).
- **The realloc header poison**: the size-header heap must copy
  USER bytes only in realloc — copying from the block start drags
  the old size header over the new one and every later free traps
  (dlmalloc checks layout at dealloc).
- **Wasm panic diagnostics**: a freestanding guest has no stderr;
  panics abort silently. The panic hook stashes
  `MWJS-PANIC <msg>` into linear memory and the harness scans it
  out after the trap (a trapped instance takes no further calls).
- Toolchain: LLVM/clang 23 alone suffices for BOTH targets (the
  Windows Kits headers feed the MSVC-flavored host compile).

## Provenance

Owner, 2026-11-10: "JS guest first, with typescript as the
first-class implementation language… use typescript's type safety as
a helper for conformant guest code." Forks 1–3 unanswered in
session — defaults in force per the §9.5 protocol.
