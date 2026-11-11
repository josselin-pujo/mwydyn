//! The `mwydyn-js` CLI — a thin argument-parsing skin over the
//! library ([`mwydyn_js::compose`]); the composition logic lives in
//! `lib.rs` since round 46 so embedders consume it directly.

use anyhow::{bail, Context, Result};

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
    let script = std::fs::read(&script_path).with_context(|| format!("read {script_path}"))?;

    let composed = mwydyn_js::compose(&engine_wasm, &script)?;
    std::fs::write(&out_path, composed.wasm).with_context(|| format!("write {out_path}"))?;
    println!(
        "mwydyn-js: {} + {} -> {} (slot @ {})",
        engine_path, script_path, out_path, composed.slot
    );
    Ok(())
}
