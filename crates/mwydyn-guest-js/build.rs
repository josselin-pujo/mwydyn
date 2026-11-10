//! Build quickjs-ng (vendored, pinned — see vendor/quickjs-ng/
//! provenance.md) with clang for BOTH targets: the wasm32-unknown-
//! unknown guest artifact and the native host (SDK tests iterate at
//! native speed). Freestanding shims compile ONLY for wasm; native
//! links the real libc.

use std::env;
use std::path::PathBuf;

fn find_clang() -> PathBuf {
    // PATH first, then the standard Windows LLVM location.
    for candidate in [
        which_clang_from_path(),
        Some(PathBuf::from(r"C:\Program Files\LLVM\bin\clang.exe")),
    ]
    .into_iter()
    .flatten()
    {
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!(
        "mwydyn-guest-js needs clang to compile the quickjs-ng engine \
         (any target). Install LLVM (winget install LLVM.LLVM) or put \
         clang on PATH."
    )
}

fn which_clang_from_path() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let cand = dir.join("clang.exe");
        if cand.is_file() {
            return Some(cand);
        }
        let cand = dir.join("clang");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let target = env::var("TARGET").expect("TARGET");
    let clang = find_clang();
    println!("cargo:rerun-if-changed={}", manifest.join("vendor").display());
    println!("cargo:rerun-if-changed={}", manifest.join("compat").display());

    let is_wasm = target == "wasm32-unknown-unknown";

    let mut build = cc::Build::new();
    build
        .compiler(&clang)
        .file(manifest.join("vendor/quickjs-ng/quickjs.c"))
        .file(manifest.join("vendor/quickjs-ng/dtoa.c"))
        .file(manifest.join("vendor/quickjs-ng/libregexp.c"))
        .file(manifest.join("vendor/quickjs-ng/libunicode.c"))
        .define("QUICKJS_NG_BUILD", None)
        // one JSValue layout everywhere: the 16-byte struct, on
        // wasm32 too (the default there is nan-boxing u64)
        .define("JS_NAN_BOXING", "0")
        .opt_level(2)
        .warnings(false); // the engine's warnings are not ours to fix

    if is_wasm {
        // SHIMS SEPARATE, AT -O0: clang's loop-idiom recognition
        // rewrites the shim memcpy/memset byte LOOPS as calls to
        // memcpy/memset — i.e. INTO THEMSELVES — at -O2 (the
        // release-only boot crash, found the hard way). The engine
        // TU keeps its optimization.
        let mut shims = cc::Build::new();
        shims
            .compiler(&clang)
            .file(manifest.join("compat/shims.c"))
            .include(manifest.join("compat/include"))
            .define("QUICKJS_NG_BUILD", None)
            .opt_level(0)
            .warnings(false);
        shims.compile("mwjs-shims");
        build.include(manifest.join("compat/include"));
    } else {
        // Host: compile with clang against the default native
        // target — headers/CRT come from the platform (MSVC flavor
        // on this toolchain). Force the MSVC ABI explicitly so the
        // objects match what rustc links regardless of PATH setup.
        build.flag("--target=x86_64-pc-windows-msvc");
    }
    build.compile("quickjs-ng");

    let _ = out;
}
