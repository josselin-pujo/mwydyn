use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pyo3::exceptions::PyException;
use crate::{addr_from_py, addr_to_py, py_to_value, value_to_py, PyActor};
use mwydyn_data::{decode, encode, DecodeOptions, Value};
use mwydyn_data::telemetry::TelPayload;

// The Context bindings (round 45): the walking-test surface as a
// Python class. spawn_wasm composes round-43 JS artifacts in the
// SAME context as Python actors — the interop bench.

/// The mwydyn context, driven from Python.
/// Context is Send but not Sync (it owns actors); the PyMutex makes
/// the pyclass Sync for pyo3 — one Python thread drives it anyway.
#[pyclass]
pub struct MwContext {
    inner: std::sync::Mutex<(mwydyn_core::Context, String)>,
}

#[pymethods]
impl MwContext {
    #[new]
    #[pyo3(signature = (name, sqlite_dir=None))]
    fn new(name: String, sqlite_dir: Option<String>) -> PyResult<Self> {
        let ctx = match sqlite_dir {
            None => mwydyn_core::Context::new(&name),
            Some(dir) => {
                let journal = std::path::Path::new(&dir).join("journal.sqlite");
                let blobs = std::path::Path::new(&dir).join("blobs.sqlite");
                let j = mwydyn_core::SQLiteJournal::open(
                    &journal,
                    Box::new(mwydyn_core::ManualClock(0)),
                )
                .map_err(|e| PyException::new_err(format!("journal: {e:?}")))?;
                let b = mwydyn_core::SQLiteBlobStore::open(&blobs)
                    .map_err(|e| PyException::new_err(format!("blobs: {e:?}")))?;
                mwydyn_core::Context::with_stores(&name, Box::new(j), Box::new(b))
            }
        };
        Ok(MwContext { inner: std::sync::Mutex::new((ctx, name)) })
    }

    /// Spawn a PYTHON actor (native realm: trusted, stateless v0).
    #[pyo3(signature = (name, actor, restart=false, rows=Vec::new()))]
    fn spawn_python(
        &mut self,
        py: Python,
        name: String,
        actor: Py<PyAny>,
        restart: bool,
        rows: Vec<(String, String)>,
    ) -> PyResult<()> {
        let _ = py;
        let cap_rows: Vec<mwydyn_core::CapRow> = rows
            .into_iter()
            .map(|(verb, object)| mwydyn_core::CapRow::new(&verb, &object))
            .collect();
        let mut spec = mwydyn_core::Spawn::new(
            Some(&name),
            Box::new(move || {
                Python::with_gil(|py| {
                    Box::new(PyActor {
                        obj: actor.clone_ref(py),
                        parts: std::cell::RefCell::new(None),
                    })
                })
            }),
        );
        if restart {
            spec = spec.supervision(mwydyn_core::Supervision::Restart);
        }
        if !cap_rows.is_empty() {
            spec = spec.rows(cap_rows);
        }
        self.inner.lock().unwrap().0
            .spawn(spec)
            .map_err(|e| PyException::new_err(format!("spawn: {e:?}")))?;
        Ok(())
    }

    /// Spawn a WASM guest artifact (JS or any §9 guest) in the same
    /// context — compose-parity, one bench.
    #[pyo3(signature = (name, artifact, restart=true, rows=Vec::new()))]
    fn spawn_wasm(
        &mut self,
        name: String,
        artifact: String,
        restart: bool,
        rows: Vec<(String, String)>,
    ) -> PyResult<()> {
        let bytes = std::fs::read(&artifact)
            .map_err(|e| PyException::new_err(format!("read {artifact}: {e}")))?;
        let cap_rows: Vec<mwydyn_core::CapRow> = rows
            .into_iter()
            .map(|(verb, object)| mwydyn_core::CapRow::new(&verb, &object))
            .collect();
        let mut spec = mwydyn_core::Spawn::new(
            Some(&name),
            Box::new(move || {
                let engine = mwydyn_wasm::WasmEngine::new().expect("engine");
                Box::new(mwydyn_wasm::WasmActor::from_compiled(&engine, &bytes).expect("guest"))
            }),
        );
        if restart {
            spec = spec.supervision(mwydyn_core::Supervision::Restart);
        }
        if !cap_rows.is_empty() {
            spec = spec.rows(cap_rows);
        }
        self.inner.lock().unwrap().0.spawn(spec)
            .map_err(|e| PyException::new_err(format!("spawn: {e:?}")))?;
        Ok(())
    }

    /// Post a letter. `to` is an actor name in this context, or
    /// "external". Payload converts from Python values; it is
    /// canonical CBOR from here on.
    #[pyo3(signature = (to, kind, payload=None, correlation=None, cites=Vec::new(), parent=None))]
    #[allow(clippy::too_many_arguments)] // the pyo3 signature mirrors the core face
    fn post(
        &mut self,
        py: Python,
        to: &Bound<'_, PyAny>,
        kind: String,
        payload: Option<Bound<'_, PyAny>>,
        correlation: Option<String>,
        cites: Vec<String>,
        parent: Option<u64>,
    ) -> PyResult<Option<u64>> {
        let addr = addr_from_py(py, to, &self.inner.lock().unwrap().1)?;
        let pv = match &payload {
            Some(p) => py_to_value(py, p)?,
            None => Value::Null,
        };
        let letter = mwydyn_data::Letter::new(&kind, encode(&pv), None, correlation, cites)
            .map_err(|e| PyException::new_err(format!("bad letter: {e:?}")))?;
        match self.inner.lock().unwrap().0.post(&addr, letter, vec![], parent) {
            mwydyn_core::Verdict::Accepted { seq } => Ok(Some(seq)),
            mwydyn_core::Verdict::Refused(why) => {
                Err(PyException::new_err(format!("post refused: {why:?}")))
            }
        }
    }

    fn run_until_idle(&mut self, max_passes: usize) {
        self.inner.lock().unwrap().0.run_until_idle(max_passes);
    }

    /// The embedding's outbox: envelopes as dicts
    /// {to, kind, payload, correlation}.
    fn drain_external(&mut self, py: Python) -> PyResult<PyObject> {
        let out = PyList::empty(py);
        for env in self.inner.lock().unwrap().0.drain_external() {
            let d = PyDict::new(py);
            d.set_item("to", addr_to_py(py, &env.to))?;
            d.set_item("kind", env.letter.kind.clone())?;
            let payload = decode(&env.letter.payload, &DecodeOptions::default())
                .unwrap_or(Value::Null);
            d.set_item("payload", value_to_py(py, &payload))?;
            d.set_item("correlation", env.letter.correlation.clone())?;
            out.append(d)?;
        }
        Ok(out.to_object(py))
    }

    /// The telemetry sink (§3.6): records from EVERY realm composing
    /// in this context — Python and JS alike.
    fn drain_telemetry(&mut self, py: Python) -> PyResult<PyObject> {
        let out = PyList::empty(py);
        for r in self.inner.lock().unwrap().0.drain_telemetry() {
            let d = PyDict::new(py);
            d.set_item("kind", r.kind.as_str())?;
            d.set_item("actor", r.actor.clone())?;
            match &r.payload {
                TelPayload::Log {
                    level,
                    message,
                    attrs,
                } => {
                    d.set_item("level", level.as_str())?;
                    d.set_item("message", message.clone())?;
                    if let Some(a) = attrs {
                        d.set_item("attrs", value_to_py(py, a))?;
                    }
                }
                TelPayload::Metric {
                    name,
                    instrument,
                    value,
                    unit,
                    attrs,
                } => {
                    d.set_item("name", name.clone())?;
                    d.set_item("instrument", instrument.as_str())?;
                    d.set_item("value", *value)?;
                    if let Some(u) = unit {
                        d.set_item("unit", u.clone())?;
                    }
                    if let Some(a) = attrs {
                        d.set_item("attrs", value_to_py(py, a))?;
                    }
                }
                other => {
                    d.set_item("detail", format!("{other:?}"))?;
                }
            }
            out.append(d)?;
        }
        Ok(out.to_object(py))
    }

    fn dead_letters(&mut self, py: Python) -> PyResult<PyObject> {
        let out = PyList::empty(py);
        for env in self.inner.lock().unwrap().0.dead_letters() {
            let d = PyDict::new(py);
            d.set_item("kind", env.letter.kind.clone())?;
            let payload = decode(&env.letter.payload, &DecodeOptions::default())
                .unwrap_or(Value::Null);
            d.set_item("payload", value_to_py(py, &payload))?;
            out.append(d)?;
        }
        Ok(out.to_object(py))
    }

    fn snapshot_actor(&mut self, name: String) -> PyResult<()> {
        self.inner
            .lock()
            .unwrap()
            .0
            .snapshot_actor(&name)
            .map_err(|e| PyException::new_err(format!("snapshot: {e:?}")))
    }
}
