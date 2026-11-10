//! THE INTEROP BENCH (round 45, work-notes/45 §3): one Python-driven
//! context, a PYTHON actor and the round-43 JS echo GUEST composing,
//! and ONE drain_telemetry showing logs from BOTH realms. Skip-if-
//! absent on the JS artifact (scripts/build-guest-js.sh).

use pyo3::prelude::*;

const SCRIPT: &str = r#"
import mwydyn

class KickerActor:
    def receive(self, letter, cx):
        # a python log AND a kick to the JS guest, replyTo = external
        kick = {"n": letter["payload"]["n"], "replyTo": "external"}
        return [
            mwydyn.Effect.log("info", "python kicking with " + str(letter["payload"]["n"])),
            mwydyn.Effect.post("js_echo", "demo.kick@1", kick),
        ]

ctx = mwydyn.MwContext("bench")
ctx.spawn_python("kicker", KickerActor(), restart=True)
ctx.spawn_wasm("js_echo", "crates/mwydyn-wasm/tests/artifacts/mwydyn_guest_js_echo.wasm")

ctx.post("kicker", "demo.start@1", {"n": 7})
ctx.run_until_idle(2000)

drained = ctx.drain_external()
tel = ctx.drain_telemetry()

logs = [r for r in tel if r["kind"] == "log"]
py_logs = [r for r in logs if "python kicking" in r["message"]]
js_logs = [r for r in logs if "echo saw" in r["message"]]
assert len(drained) == 1, f"one echo out, got {len(drained)}"
assert drained[0]["payload"]["n"] == 7, drained[0]["payload"]
assert len(py_logs) == 1, f"python log: {logs}"
assert len(js_logs) == 1, f"js log: {logs}"
"ok: %s py-logs, %s js-logs, echo payload %s" % (len(py_logs), len(js_logs), drained[0]["payload"]["n"])
"#;

#[test]
fn python_and_js_actors_compose_with_one_telemetry_drain() {
    let artifact = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mwydyn-wasm/tests/artifacts/mwydyn_guest_js_echo.wasm");
    if !artifact.exists() {
        eprintln!("SKIP: JS guest artifact missing — run scripts/build-guest-js.sh");
        return;
    }
    use mwydyn_py::mwydyn;
    pyo3::append_to_inittab!(mwydyn);
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| -> PyResult<()> {
        // workspace-relative paths inside the script
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::env::set_current_dir(&root).ok();
        py.run(&std::ffi::CString::new(SCRIPT).unwrap(), None, None)?;
        Ok(())
    })
    .expect("the interop bench passes");
}
