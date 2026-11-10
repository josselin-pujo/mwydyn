# quickjs-ng vendored runtime

- Upstream: https://github.com/quickjs-ng/quickjs
- Version: v0.18.0 (release tarball)
- Tarball SHA-256:
  `28bcd89aa3683fd2a946537e5ce3f11cc46b5cbf3383c7561aaef3df5ff9f1dd`
- License: MIT (see LICENSE) — admission-list clean (round 24 policy)

Only the runtime translation units are vendored (quickjs.c, dtoa.c,
libregexp.c, libunicode.c + their headers): the OS layer
(quickjs-libc), the CLIs (qjs/qjsc), and test harnesses are NOT
vendored — a wasm guest is freestanding by law (§9.1, zero imports).

Built via build.rs with clang for both targets, JS_NAN_BOXING=0 for
a uniform 16-byte JSValue across host tests and wasm. Compat
headers + shims under ../compat/ supply the freestanding libc
surface for wasm32-unknown-unknown (real libc links on native).

Update policy: re-vendor by downloading a new release tarball,
re-recording the checksum here, and re-running the full suite. Do
not hand-edit vendored files.
