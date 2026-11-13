//! mwydyn-js — THE JS GUEST BUILD TOOL (round 43), as a library since
//! round 46. `script.js` + prebuilt engine → one self-contained §9
//! wasm artifact:
//!
//! ```text
//! let composed = mwydyn_js::compose(&engine_wasm, b"script bytes")?;
//! std::fs::write("guest.wasm", composed.wasm)?; // composed.slot: diagnostics
//! ```
//!
//! The script lands in the wrapper's reserved slot as an ACTIVE data
//! segment (script bytes + NUL — the terminator is the library's job,
//! callers pass raw script bytes); the slot address is PROBED via one
//! throwaway wasmtime instantiation of the engine — no fixed
//! addresses, and the artifact stays a plain zero-import... one
//! ambient-import §9 guest the host already knows.
//!
//! The `mwydyn-js` binary is a thin CLI over [`compose`]; embedders
//! (platformc's `compose-js`) consume the library directly.

use anyhow::{bail, Result};
// wasmtime 47 dropped `std::error::Error` on its Error type; the
// `anyhow` cargo feature on the wasmtime dep restores the `?`
// conversion into anyhow::Error.
use wasm_encoder::{ConstExpr, DataSection, Module, RawSection, SectionId};
use wasmparser::{DataKind, Parser, Payload};

/// A composed guest: the self-contained §9 artifact plus the slot
/// address the script was written to (diagnostics only — the guest
/// itself needs no external knowledge of it).
pub struct Composed {
    pub wasm: Vec<u8>,
    pub slot: i32,
}

/// Compose `script` into the engine's reserved slot: probe the slot
/// address, splice the active data segment, return the artifact.
pub fn compose(engine_wasm: &[u8], script: &[u8]) -> Result<Composed> {
    let mut script = script.to_vec();
    script.push(b'\0'); // the slot's terminator
    let ptr = probe_slot(engine_wasm)?;
    let wasm = splice(engine_wasm, ptr, &script)?;
    Ok(Composed { wasm, slot: ptr })
}

/// One throwaway instantiation: call `mwydyn_script_slot()` — the
/// ambient import gets a refusing stub (nothing calls it during the
/// probe; the epoch budget mirrors the container's probe lesson).
pub fn probe_slot(engine_wasm: &[u8]) -> Result<i32> {
    use wasmtime::{Engine, Linker, Module, Store};
    let engine = Engine::default();
    let module = Module::new(&engine, engine_wasm)?;
    let mut linker: Linker<()> = Linker::new(&engine);
    linker.func_wrap("mwydyn", "ambient", |_a: i32, _b: i32, _c: i32, _d: i32| -> i32 {
        -1 // refusing stub — the probe never triggers it
    })?;
    let mut store = Store::new(&engine, ());
    store.set_epoch_deadline(1_000_000);
    let instance = linker.instantiate(&mut store, &module)?;
    let slot = instance
        .get_typed_func::<(), i32>(&mut store, "mwydyn_script_slot")
        .map_err(|e| anyhow::anyhow!("engine lacks mwydyn_script_slot — wrong wasm?: {e}"))?;
    let ptr = slot.call(&mut store, ())?;
    Ok(ptr)
}

/// Re-emit the module, extending (or adding) the data section with
/// one active segment writing `script` into linear memory at `ptr`.
fn splice(engine_wasm: &[u8], ptr: i32, script: &[u8]) -> Result<Vec<u8>> {
    let mut out = Module::new();
    let mut appended = false;
    for payload in Parser::new(0).parse_all(engine_wasm) {
        let payload = payload?;
        match payload {
            Payload::DataSection(reader) => {
                let mut data = DataSection::new();
                for seg in reader {
                    let seg = seg?;
                    match seg.kind {
                        DataKind::Active {
                            memory_index,
                            offset_expr,
                        } => {
                            let offset = const_expr_bytes(&offset_expr)?;
                            data.active(memory_index, &offset, seg.data.to_vec());
                        }
                        DataKind::Passive => {
                            data.passive(seg.data.to_vec());
                        }
                    }
                }
                data.active(0, &ConstExpr::i32_const(ptr), script.to_vec());
                appended = true;
                out.section(&data);
            }
            Payload::CodeSectionStart { .. } | Payload::CodeSectionEntry { .. } => {
                // re-emit the whole code section verbatim on START
                if let Payload::CodeSectionStart { range, .. } = payload {
                    out.section(&RawSection {
                        id: SectionId::Code as u8,
                        data: &engine_wasm[range],
                    });
                }
            }
            Payload::CustomSection(_) => {
                // custom sections re-emit verbatim (name rides the raw bytes)
                if let Some((_, range)) = payload.as_section() {
                    out.section(&RawSection { id: 0, data: &engine_wasm[range] });
                }
            }
            Payload::Version { .. } | Payload::End(_) => {}
            other => {
                if let Some((id, range)) = other.as_section() {
                    out.section(&RawSection {
                        id,
                        data: &engine_wasm[range],
                    });
                }
            }
        }
    }
    if !appended {
        bail!("engine has no data section to extend (unexpected layout)");
    }
    Ok(out.finish())
}

fn const_expr_bytes(expr: &wasmparser::ConstExpr) -> Result<ConstExpr> {
    // i32.const offsets dominate; re-emit losslessly
    let mut reader = expr.get_operators_reader();
    let op = reader.read()?;
    if let wasmparser::Operator::I32Const { value } = op {
        Ok(ConstExpr::i32_const(value))
    } else {
        bail!("unsupported data-offset expression {op:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine artifact staged by `scripts/build-guest-js.sh`
    /// (uncomposed engine+wrapper). Skips with a message when absent —
    /// the repo-wide convention, so the suite runs without the wasm
    /// toolchain. The composed bytes must parse as a module and carry
    /// the script verbatim in their data section.
    #[test]
    fn compose_engine_and_script_yields_a_valid_module() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mwydyn-wasm/tests/artifacts/mwydyn_guest_js.wasm"
        );
        let Ok(engine) = std::fs::read(path) else {
            eprintln!("skip: engine artifact absent — build via scripts/build-guest-js.sh");
            return;
        };
        let script = b"mwydyn.entry({ receive() { return []; } });";
        let composed = compose(&engine, script).expect("compose engine + script");
        let module = wasmtime::Module::new(&wasmtime::Engine::default(), &composed.wasm)
            .expect("composed artifact parses as a module");
        let mut found = false;
        for payload in wasmparser::Parser::new(0).parse_all(&composed.wasm) {
            if let wasmparser::Payload::DataSection(reader) = payload.expect("data section") {
                for seg in reader {
                    let seg = seg.expect("segment");
                    // FULL script, verbatim — the round-46 pin scanned
                    // `windows(5)` against the 6-byte literal "mwydyn"
                    // (never true) and read green only while the engine
                    // artifact was absent; born skipped, found by round
                    // 48's workspace run once round 47 staged the engine.
                    if seg.data.windows(script.len()).any(|w| w == script) {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "script bytes must land VERBATIM in the composed data section");
        assert!(module.imports().count() <= 1, "at most the ambient import");
        assert!(composed.slot > 0, "slot probe returned a live address");
    }
}
