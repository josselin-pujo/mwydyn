//! mwydyn-py — THE PYTHON HOST EMBEDDING (round 45, work-notes/45):
//! mwydyn as a Python library. Scripted test harnesses now; python
//! nodes when a network transport exists. Python actors are
//! NATIVE-REALM (trusted in-process code, GIL-serialized,
//! compose-parity with wasm guests in the same context).
//!
//! The vocabulary mirrors the guest SDKs (§9.6's law, host-side):
//! effects are RETURNED, never called; refusals arrive BY NAME as
//! `MwydynRefusal`; a raised exception in `receive` IS a panic IS a
//! strike. v0: Python actors are stateless-declared (the instance
//! persists; no rollback promised — work-notes/45 §2).

#![allow(deprecated)] // pyo3 to_object/into_py → IntoPyObject migration: v0 batch

use mwydyn_core::{Actor, Host, MsgCtx};
use mwydyn_data::Letter;
use mwydyn_data::telemetry::{Instrument, Level, TelPayload, TelemetryRecord};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Value};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString};

// ---- exceptions ----

pyo3::create_exception!(mwydyn, MwydynRefusal, PyException, "a host refusal, BY NAME (code attribute)");

// ---- Value ⇄ Python (one canon: CBOR stays mwydyn-data) ----

#[allow(deprecated)] // pyo3 to_object→IntoPyObject migration: v0
fn value_to_py(py: Python, v: &Value) -> PyObject {
    match v {
        Value::Null => py.None(),
        Value::Bool(b) => PyBool::new(py, *b).to_object(py),
        Value::Int(i) => i.to_object(py),
        Value::Float(f) => PyFloat::new(py, *f).to_object(py),
        Value::Text(s) => PyString::new(py, s).to_object(py),
        Value::Bytes(b) => PyBytes::new(py, b).to_object(py),
        Value::BigPos(mag) | Value::BigNeg(mag) => {
            // arbitrary-precision ints ride Python's bignums
            let mut n = num_from_bytes(mag);
            if matches!(v, Value::BigNeg(_)) {
                n = -n;
            }
            n.to_object(py)
        }
        Value::Timestamp(us) => us.to_object(py),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for i in items {
                let _ = list.append(value_to_py(py, i));
            }
            list.to_object(py)
        }
        Value::Map(pairs) => {
            let d = PyDict::new(py);
            for (k, val) in pairs {
                d.set_item(k, value_to_py(py, val)).expect("dict set");
            }
            d.to_object(py)
        }
        Value::BlobRef {
            digest,
            media_type,
            size,
        } => Py::new(py, MwBlobRef {
            digest: digest.clone(),
            media_type: media_type.clone(),
            size: *size,
        })
        .expect("pyclass alloc")
        .into_py(py),
    }
}

fn num_from_bytes(mag: &[u8]) -> i128 {
    let mut n: i128 = 0;
    for &b in mag {
        n = (n << 8) | b as i128;
    }
    n
}

#[allow(clippy::only_used_in_recursion)] // py traverses nested containers
fn py_to_value(py: Python, obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(b) = obj.downcast::<PyBool>() {
        return Ok(Value::Bool(b.is_true()));
    }
    if let Ok(i) = obj.downcast::<PyInt>() {
        let n: i128 = i.extract()?;
        if n >= i64::MIN as i128 && n <= i64::MAX as i128 {
            return Ok(Value::Int(n as i64));
        }
        let neg = n < 0;
        let mag = (-n).unsigned_abs();
        let mut bytes = mag.to_be_bytes().to_vec();
        while bytes.len() > 1 && bytes[0] == 0 {
            bytes.remove(0);
        }
        let v = if neg {
            Value::big_neg(bytes).map_err(value_err)?
        } else {
            Value::big_pos(bytes).map_err(value_err)?
        };
        return Ok(v);
    }
    if let Ok(f) = obj.downcast::<PyFloat>() {
        let x: f64 = f.extract()?;
        return Value::float(x).map_err(value_err);
    }
    if let Ok(s) = obj.downcast::<PyString>() {
        return Ok(Value::Text(s.extract()?));
    }
    if let Ok(b) = obj.downcast::<PyBytes>() {
        return Ok(Value::Bytes(b.as_bytes().to_vec()));
    }
    if let Ok(l) = obj.downcast::<PyList>() {
        let mut items = Vec::with_capacity(l.len());
        for item in l.iter() {
            items.push(py_to_value(py, &item)?);
        }
        return Ok(Value::Array(items));
    }
    if let Ok(d) = obj.downcast::<PyDict>() {
        // BlobRef objects pass through as refs
        if let Ok(r) = obj.extract::<MwBlobRef>() {
            Value::validate_blob_ref(&r.digest, &r.media_type).map_err(value_err)?;
            return Ok(Value::BlobRef {
                digest: r.digest,
                media_type: r.media_type,
                size: r.size,
            });
        }
        let mut pairs = Vec::with_capacity(d.len());
        for (k, v) in d.iter() {
            let key: String = k.extract()?;
            pairs.push((key, py_to_value(py, &v)?));
        }
        return Value::map(pairs).map_err(value_err);
    }
    Err(PyException::new_err(format!(
        "mwydyn: value does not convert ({}); use dict/list/str/int/float/bool/None/bytes",
        obj.get_type()
    )))
}

fn value_err(e: mwydyn_data::ValueError) -> PyErr {
    PyException::new_err(format!("mwydyn: value rejected: {e:?}"))
}

/// A blob reference crossing the boundary.
#[pyclass(frozen)]
#[derive(Clone)]
struct MwBlobRef {
    #[pyo3(get)]
    digest: String,
    #[pyo3(get)]
    media_type: String,
    #[pyo3(get)]
    size: u64,
}

// ---- addresses ----

fn addr_to_py(py: Python, a: &Address) -> PyObject {
    match a {
        Address::Named { context, name } => {
            let d = PyDict::new(py);
            d.set_item("context", context).unwrap();
            d.set_item("name", name).unwrap();
            d.to_object(py)
        }
        Address::Instance(id) => id.to_object(py),
    }
}

fn addr_from_py(_py: Python, obj: &Bound<'_, PyAny>, default_ctx: &str) -> PyResult<Address> {
    if let Ok(s) = obj.downcast::<PyString>() {
        let id: String = s.extract()?;
        if id == "external" {
            return Ok(Address::Instance("external".into()));
        }
        return Ok(Address::Named {
            context: default_ctx.into(),
            name: id,
        });
    }
    if let Ok(d) = obj.downcast::<PyDict>() {
        let context: String = d.get_item("context")?.map(|v| v.extract()).transpose()?.unwrap_or(default_ctx.into());
        let name: String = d
            .get_item("name")?
            .ok_or_else(|| PyException::new_err("address dict needs 'name'"))?
            .extract()?;
        return Ok(Address::Named { context, name });
    }
    Err(PyException::new_err(
        "address: a name string, 'external', or {context, name}",
    ))
}

// ---- the Python-visible effect builders ----

/// Effects are RETURNED from receive, never called.
#[pyclass]
struct Effect;

#[pymethods]
impl Effect {
    #[staticmethod]
    #[pyo3(name = "post")]
    fn post(_to: PyObject, kind: String, payload: PyObject) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            // `to` rides AS-GIVEN (a name string or {context, name}):
            // the SHIM resolves it against the actor's real context
            let d = PyDict::new(py);
            let inner = PyDict::new(py);
            let ld = PyDict::new(py);
            ld.set_item("kind", kind)?;
            ld.set_item("payload", payload)?;
            inner.set_item("to", _to)?;
            inner.set_item("letter", ld)?;
            d.set_item("post", inner)?;
            Ok(d.to_object(py))
        })
    }

    #[staticmethod]
    #[pyo3(name = "log")]
    fn log(level: String, message: String) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let level_v = Level::parse(&level)
                .ok_or_else(|| PyException::new_err(format!("bad level {level:?}")))?;
            let rec = TelemetryRecord::declared(
                mwydyn_data::telemetry::TelemetryKind::Log,
                TelPayload::Log {
                    level: level_v,
                    message,
                    attrs: None,
                },
            );
            effect_dict_from_record(py, &rec)
        })
    }

    #[staticmethod]
    #[pyo3(name = "metric")]
    fn metric(name: String, instrument: String, value: f64) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let inst = Instrument::parse(&instrument)
                .ok_or_else(|| PyException::new_err(format!("bad instrument {instrument:?}")))?;
            let rec = TelemetryRecord::declared(
                mwydyn_data::telemetry::TelemetryKind::Metric,
                TelPayload::Metric {
                    name,
                    instrument: inst,
                    value,
                    unit: None,
                    attrs: None,
                },
            );
            effect_dict_from_record(py, &rec)
        })
    }
}

fn effect_dict_from_record(py: Python, rec: &TelemetryRecord) -> PyResult<PyObject> {
    let d = PyDict::new(py);
    let emit = PyDict::new(py);
    let declare = rec.declare_value();
    let Value::Map(pairs) = declare else { unreachable!("declare is a map") };
    for (k, v) in pairs {
        let vs = match &v {
            Value::Text(s) => s.clone(),
            other => format!("{other:?}"),
        };
        if k == "kind" {
            emit.set_item("kind", vs)?;
        } else {
            // the payload map under the kind key
            emit.set_item(k.clone(), value_to_py(py, &v))?;
            let _ = vs;
        }
    }
    d.set_item("emit", emit)?;
    Ok(d.to_object(py))
}

// ---- the Python actor shim (native realm) ----

struct PyActor {
    obj: Py<PyAny>,
    parts: std::cell::RefCell<Option<mwydyn_core::actor::AmbientParts>>,
}

impl PyActor {
    fn convert_effects(
        py: Python,
        list: &Bound<'_, PyList>,
        default_ctx: &str,
    ) -> PyResult<Vec<mwydyn_core::Effect>> {
        let mut out = Vec::with_capacity(list.len());
        for item in list.iter() {
            let d = item
                .downcast::<PyDict>()
                .map_err(|_| PyException::new_err("effects: list of Effect dicts"))?;
            let post_obj = d.get_item("post")?;
            let post = match post_obj {
                Some(po) => po.downcast_into::<PyDict>().ok(),
                None => None,
            };
            if let Some(post) = post {
                let to_v = post
                    .get_item("to")?
                    .ok_or_else(|| PyException::new_err("post needs 'to'"))?;
                let to = addr_from_py(py, &to_v, default_ctx)?;
                let ld_obj = post
                    .get_item("letter")?
                    .ok_or_else(|| PyException::new_err("post needs 'letter'"))?;
                let ld = ld_obj
                    .downcast_into::<PyDict>()
                    .map_err(|_| PyException::new_err("letter is a dict"))?;
                let kind: String = ld
                    .get_item("kind")?
                    .and_then(|v| v.extract().ok())
                    .ok_or_else(|| PyException::new_err("letter.kind required"))?;
                let payload = ld
                    .get_item("payload")?
                    .map(|v| py_to_value(py, &v))
                    .transpose()?
                    .unwrap_or(Value::Null);
                let correlation: Option<String> =
                    ld.get_item("correlation")?.and_then(|v| v.extract().ok());
                let letter = Letter::new(&kind, encode(&payload), None, correlation, vec![])
                    .map_err(|e| PyException::new_err(format!("bad letter: {e:?}")))?;
                out.push(mwydyn_core::Effect::Post { to, letter });
                continue;
            }
            let emit = d.get_item("emit")?;
            if let Some(emit_obj) = emit {
                let v = py_to_value(py, &emit_obj)?;
                let rec = TelemetryRecord::declare_from_value(&v)
                    .map_err(|e| PyException::new_err(format!("emit: {e:?}")))?;
                out.push(mwydyn_core::Effect::Emit { record: rec });
                continue;
            }
            return Err(PyException::new_err(
                "effect must be Effect.post/log/metric",
            ));
        }
        Ok(out)
    }
}

impl Actor for PyActor {
    fn set_ambient(&mut self, parts: mwydyn_core::actor::AmbientParts) {
        *self.parts.borrow_mut() = Some(parts);
    }

    fn receive(&mut self, ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<mwydyn_core::Effect> {
        Python::with_gil(|py| -> Vec<mwydyn_core::Effect> {
            let obj = self.obj.bind(py);
            let ld = PyDict::new(py);
            let _ = ld.set_item("kind", letter.kind.clone());
            let payload = decode(&letter.payload, &DecodeOptions::default())
                .unwrap_or(Value::Null);
            let _ = ld.set_item("payload", value_to_py(py, &payload));
            if let Some(c) = &letter.cause {
                let _ = ld.set_item("cause", c);
            }
            if let Some(c) = &letter.correlation {
                let _ = ld.set_item("correlation", c);
            }
            let auth = PyList::empty(py);
            for a in &letter.authority {
                let _ = auth.append(a);
            }
            let _ = ld.set_item("authority", auth);

            let (blobs, rows, ctx_name) = {
                let slot = self.parts.borrow_mut();
                if slot.is_none() {
                    // ambient not yet handed over: derive from the host view
                    // is impossible (borrows) — the pumps always call
                    // set_ambient before receive; hold the line loudly
                    panic!("mwydyn-py: ambient parts missing (set_ambient not called)");
                }
                let p = slot.as_ref().expect("just checked");
                (p.blobs.clone(), p.rows.clone(), p.context.to_string())
            };
            let cx = Py::new(
                py,
                MwCx {
                    sender: ctx.from.clone(),
                    blobs,
                    rows,
                },
            )
            .expect("pyclass alloc")
            .into_py(py);

            let receive = match obj.getattr("receive") {
                Ok(f) => f,
                Err(_) => {
                    return vec![mwydyn_core::Effect::Emit {
                        record: TelemetryRecord::declared(
                            mwydyn_data::telemetry::TelemetryKind::Log,
                            TelPayload::Log {
                                level: Level::Error,
                                message: "mwydyn-py: actor has no receive method".into(),
                                attrs: None,
                            },
                        ),
                    }]
                }
            };
            let result = match receive.call((ld, cx), None) {
                Ok(r) => r,
                Err(e) => {
                    // a raised exception IS a panic IS a strike
                    let msg = e.to_string();
                    panic!("mwydyn-py: receive raised — {msg}");
                }
            };
            let list = match result.downcast::<PyList>() {
                Ok(l) => l,
                Err(_) => panic!("mwydyn-py: receive must return a list of effects"),
            };
            match Self::convert_effects(py, list, &ctx_name) {
                Ok(effects) => effects,
                Err(e) => panic!("mwydyn-py: bad effects — {e}"),
            }
        })
    }

    fn fork(&self) -> Box<dyn Actor> {
        Python::with_gil(|py| {
            // v0 STATELESS-BY-DECLARATION: the same PyObject rides
            // every fork — the instance persists, no rollback
            // promised.
            Box::new(PyActor {
                obj: self.obj.clone_ref(py),
                parts: std::cell::RefCell::new(None),
            })
        })
    }
}

/// The `cx` a Python actor receives: sender + the ambient face
/// (owned Arc parts — the pyclass cannot borrow the Host view).
#[pyclass]
struct MwCx {
    sender: Address,
    blobs: std::sync::Arc<dyn mwydyn_core::blob::BlobFace>,
    rows: Vec<mwydyn_core::CapRow>,
}

#[pymethods]
impl MwCx {
    #[getter]
    fn from_(&self, py: Python) -> PyObject {
        addr_to_py(py, &self.sender)
    }

    fn sender(&self, py: Python) -> PyObject {
        addr_to_py(py, &self.sender)
    }

    fn blob_put(&self, py: Python, bytes: Vec<u8>, media_type: String) -> PyResult<PyObject> {
        if !self.rows.iter().any(|r| r.holds("blob.write", "*")) {
            return Err(MwydynRefusal::new_err("Unauthorized"));
        }
        match self.blobs.put(&bytes, &media_type, None) {
            Ok(r) => Ok(Py::new(
                py,
                MwBlobRef {
                    digest: r.digest,
                    media_type: r.media_type,
                    size: r.size,
                },
            )
            .expect("pyclass alloc")
            .into_py(py)),
            Err(code) => Err(MwydynRefusal::new_err(format!("{code:?}"))),
        }
    }

    fn blob_get(&self, py: Python, r: &MwBlobRef) -> PyResult<PyObject> {
        let core_ref = mwydyn_core::BlobRef {
            digest: r.digest.clone(),
            media_type: r.media_type.clone(),
            size: r.size,
        };
        if !self.rows.iter().any(|r| r.holds("blob.read", "*")) {
            return Err(MwydynRefusal::new_err("Unauthorized"));
        }
        match self.blobs.get(&core_ref) {
            Ok(b) => Ok(PyBytes::new(py, &b).to_object(py)),
            Err(code) => Err(MwydynRefusal::new_err(format!("{code:?}"))),
        }
    }

    fn blob_len(&self, r: &MwBlobRef) -> PyResult<u64> {
        let core_ref = mwydyn_core::BlobRef {
            digest: r.digest.clone(),
            media_type: r.media_type.clone(),
            size: r.size,
        };
        if !self.rows.iter().any(|r| r.holds("blob.read", "*")) {
            return Err(MwydynRefusal::new_err("Unauthorized"));
        }
        self.blobs.len(&core_ref).map_err(|c| MwydynRefusal::new_err(format!("{c:?}")))
    }

    fn caps(&self, py: Python) -> PyObject {
        let list = PyList::empty(py);
        for row in &self.rows {
            let d = PyDict::new(py);
            let _ = d.set_item("verb", row.verb.clone());
            let _ = d.set_item("object", row.object.clone());
            let _ = list.append(d);
        }
        list.to_object(py)
    }
}

// ---- the module ----

#[pymodule]
pub fn mwydyn(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Effect>()?;
    m.add_class::<MwBlobRef>()?;
    m.add_class::<MwCx>()?;
    m.add("MwydynRefusal", py.get_type::<MwydynRefusal>())?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<MwContext>()?;
    Ok(())
}

mod context;
pub use context::MwContext;
