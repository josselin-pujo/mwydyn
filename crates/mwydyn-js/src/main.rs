//! mwydyn-js — THE JS GUEST BUILD TOOL (round 43). `script.js` +
//! prebuilt engine → one self-contained §9 wasm artifact:
//!
//!   mwydyn-js build --engine <engine.wasm> --script <script.js> -o <guest.wasm>
//!
//! The script lands in the wrapper's reserved slot as an ACTIVE data
//! segment (script bytes + NUL); the slot address is PROBED via one
//! throwaway wasmtime instantiation of the engine — no fixed
//! addresses, and the artifact stays a plain zero-import... one
//! ambient-import §9 guest the host already knows.

use anyhow::{bail, Context, Result};
use wasm_encoder::{ConstExpr, DataSection, Module, RawSection, SectionId};
use wasmparser::{DataKind, Parser, Payload};

fn main() {
    if let Err(e) = run() {
        eprintln!("mwydyn-js: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut engine_path = None;
    let mut script_path = None;
    let mut out_path = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "build" => i += 1,
            "--engine" => {
                engine_path = Some(args.get(i + 1).context("--engine needs a path")?.clone());
                i += 2;
            }
            "--script" => {
                script_path = Some(args.get(i + 1).context("--script needs a path")?.clone());
                i += 2;
            }
            "-o" | "--out" => {
                out_path = Some(args.get(i + 1).context("-o needs a path")?.clone());
                i += 2;
            }
            other => bail!("unknown argument {other:?} — see the header"),
        }
    }
    let engine_path = engine_path.context("--engine <path> required")?;
    let script_path = script_path.context("--script <path> required")?;
    let out_path = out_path.context("-o <path> required")?;

    let engine_wasm = std::fs::read(&engine_path).with_context(|| format!("read {engine_path}"))?;
    let mut script = std::fs::read(&script_path).with_context(|| format!("read {script_path}"))?;
    script.push(b'\0'); // the slot's terminator

    // 1. PROBE: where does this engine keep its script slot?
    let ptr = probe_slot(&engine_wasm)?;
    // 2. SPLICE: append the active data segment at that address.
    let guest = splice(&engine_wasm, ptr, &script)?;
    std::fs::write(&out_path, guest).with_context(|| format!("write {out_path}"))?;
    println!(
        "mwydyn-js: {} + {} -> {} (slot @ {ptr})",
        engine_path,
        script_path,
        out_path
    );
    Ok(())
}

/// One throwaway instantiation: call `mwydyn_script_slot()` — the
/// ambient import gets a refusing stub (nothing calls it during the
/// probe; the epoch budget mirrors the container's probe lesson).
fn probe_slot(engine_wasm: &[u8]) -> Result<i32> {
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
        .context("engine lacks mwydyn_script_slot — wrong wasm?")?;
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
