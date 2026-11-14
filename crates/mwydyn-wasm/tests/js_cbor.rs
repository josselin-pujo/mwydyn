//! ROUND 49 — THE PRELUDE CBOR CODEC AGAINST THE CORPUS, INSIDE THE
//! REAL COMPOSED ENGINE. The JS realm's `mwydyn.decode`/`encode` is
//! the second-realm decoder cbor-profile-v0 and blob-refs-v0 were
//! written for; this harness drives every case through ONE composed
//! quickjs guest (the round-47 lesson: test the FFI layer that
//! actually runs, not a host-side interpreter).
//!
//! Driving protocol: each case is a letter {op, name, value?/hex?};
//! the guest answers {name, ok: <canonical hex>} | {name, err:
//! <code>} — err codes are the corpus kebab names (refusals by
//! name). Two JS-realm facts shape the routing:
//!
//! - The BARRIER cannot carry everything: BigPos/BigNeg refuse
//!   loudly (v0) and Timestamps cross as bare µs numbers (tag lost).
//!   Encode inputs that would distort in transit ride an IN-GUEST
//!   literal table keyed by case name; decode results report `big`
//!   (decimal string) instead of echoing an uncrossable value.
//! - The JS value model normalizes what the Rust model types apart:
//!   in-i64 bignums are plain ints (the barrier's own rule), and
//!   bare integral numbers are ints — floatness is explicit
//!   (`mwydyn.float`). Where the corpus pins the RUST type
//!   distinction (dec-tag2-minimal), the harness compares the
//!   reported decimal, not a re-encode.
//!
//! Skips, with reasons on record (see work-notes/49): op=Digest
//! (digests are computed host-side at put — a JS SHA-256 is not part
//! of the canon codec), and the three enc-ref-reject-* cases (they
//! pin the VALIDATOR layer — model→Value in the Rust realm; the JS
//! realm's validator is the barrier, and codec-encode is a
//! serializer by law, so those cases have no JS-side equivalent at
//! this layer).

use mwydyn_core::{Context, Spawn, Supervision};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_vectors::{json_to_value, load_corpus, model_to_value, Case, Model, Op};
use mwydyn_wasm::{WasmActor, WasmEngine};
use std::collections::HashMap;
use std::path::Path;

/// The three blob-corpus cases that pin the validator layer — see
/// the module doc.
const VALIDATOR_LAYER: [&str; 3] = [
    "enc-ref-reject-nonhex-digest",
    "enc-ref-reject-uppercase-digest",
    "enc-ref-reject-empty-type",
];

const GUEST_SCRIPT: &str = r#"
function hexOf(u8) {
  let s = "";
  for (let i = 0; i < u8.length; i++) s += (u8[i] < 16 ? "0" : "") + u8[i].toString(16);
  return s;
}
function unhexOf(s) {
  const u = new Uint8Array(s.length / 2);
  for (let i = 0; i < u.length; i++) u[i] = parseInt(s.substring(i * 2, i * 2 + 2), 16);
  return u;
}
// Encode inputs that cannot cross the barrier undistorted — keyed by
// corpus case name (an unknown key is a loud err, so corpus edits
// can't drift past this table silently).
const DOT = {};
DOT["has.dot"] = 1;
const PROTO = {};
Object.defineProperty(PROTO, "__proto__", { value: 1, enumerable: true, configurable: true });
const LITERALS = {
  "enc-big-2^63": 9223372036854775808n,
  "enc-big-neg": -9223372036854775810n,
  "enc-timestamp-0": mwydyn.timestamp(0),
  "enc-float-3.0-floatness-preserved": mwydyn.float(3.0),
  "enc-map-rejects-dot-key": DOT,
  "enc-map-rejects-proto-key": PROTO,
};
function walk(v, d) {
  // returns {big, brand, depth} — depth = this value's deepest leaf
  // at its own root-relative level; the reply map wraps val one
  // level deep, and the payload wall decodes at MAX_DEPTH 64, so a
  // val deeper than 63 cannot be echoed (dec-64-deep-ok rides the
  // canonical re-encode alone).
  if (v === null) return { big: false, brand: false, depth: d };
  const t = typeof v;
  if (t === "bigint") return { big: true, brand: false, depth: d };
  if (t !== "object") return { big: false, brand: false, depth: d };
  if (v.__mwRef === true || v.__mwTs === true || v.__mwFloat === true) {
    return { big: false, brand: true, depth: d };
  }
  const r = { big: false, brand: false, depth: d };
  const kids = Array.isArray(v) ? v : Object.keys(v).map(function (k) { return v[k]; });
  for (let i = 0; i < kids.length; i++) {
    const w = walk(kids[i], d + 1);
    r.big = r.big || w.big;
    r.brand = r.brand || w.brand;
    r.depth = Math.max(r.depth, w.depth);
  }
  return r;
}
mwydyn.entry({
  receive(letter, cx) {
    const p = letter.payload;
    const name = p.name;
    try {
      if (p.op === "encode") {
        return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", { name: name, ok: hexOf(mwydyn.encode(p.value)) })];
      }
      if (p.op === "encodeLiteral") {
        const v = Object.prototype.hasOwnProperty.call(LITERALS, name) ? LITERALS[name] : undefined;
        if (v === undefined) {
          return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", { name: name, err: "no-literal:" + name })];
        }
        return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", { name: name, ok: hexOf(mwydyn.encode(v)) })];
      }
      if (p.op === "decode") {
        let v;
        try {
          v = mwydyn.decode(unhexOf(p.hex));
        } catch (e) {
          return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", { name: name, err: e.code || "malformed" })];
        }
        const w = walk(v, 0);
        const r = { name: name, ok: hexOf(mwydyn.encode(v)) };
        if (!w.big && !w.brand && w.depth + 1 <= 64) r.val = v;
        if (w.big && typeof v === "bigint") r.big = v.toString();
        return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", r)];
      }
      return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", { name: name, err: "bad-op:" + p.op })];
    } catch (e) {
      return [mwydyn.Effect.reply(letter, cx, "vec.reply@1", { name: name, err: (e && e.code) || ("throw:" + e.message) })];
    }
  },
});
"#;

fn addr(context: &str, name: &str) -> Address {
    Address::Named {
        context: context.into(),
        name: name.into(),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Value equality with float-bits semantics (the runner's rule).
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && same_value(v1, v2))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same_value(a, b))
        }
        (a, b) => a == b,
    }
}

/// Anything the barrier cannot carry undistorted: bignums (loud
/// refusal in v0), the timestamp tag (crosses as bare µs), and
/// INTEGRAL floats (a bare JS number cannot hold floatness — the
/// barrier normalizes 3.0 to the int 3; mwydyn.float is explicit).
fn uncrossable(v: &Value) -> bool {
    match v {
        Value::BigPos(_) | Value::BigNeg(_) | Value::Timestamp(_) => true,
        Value::Float(f) => f.fract() == 0.0,
        Value::Array(items) => items.iter().any(uncrossable),
        Value::Map(pairs) => pairs.iter().any(|(_, v)| uncrossable(v)),
        _ => false,
    }
}

/// The decimal string the guest reports for a bare-BigInt decode.
fn big_decimal(m: &Model) -> String {
    let Model::Big { sign, hex: h } = m else {
        panic!("big_decimal on a non-big model");
    };
    let mag = u128::from_str_radix(h, 16).expect("magnitude hex");
    if sign == "pos" {
        mag.to_string()
    } else {
        // tag 3: value = -1 - magnitude
        (-(mag as i128) - 1).to_string()
    }
}

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    let Value::Map(pairs) = v else {
        panic!("reply is a map, got {v:?}");
    };
    pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v)
}

fn run_corpus(ctx: &mut Context, file: &str, failures: &mut Vec<String>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(file);
    let corpus = load_corpus(&path).unwrap_or_else(|e| panic!("{file} loads: {e}"));

    // Route each case to a letter (or a recorded skip).
    let mut letters = Vec::new();
    let mut plan: HashMap<String, &Case> = HashMap::new();
    for case in &corpus.cases {
        let mk = |pairs: Vec<(&str, Value)>| {
            encode(&Value::map(
                pairs
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            )
            .unwrap())
        };
        let payload = match case.op {
            Op::Digest => {
                eprintln!("SKIP {}: digest is host-side at put — JS SHA-256 is out of codec scope", case.name);
                continue;
            }
            _ if VALIDATOR_LAYER.contains(&case.name.as_str()) => {
                eprintln!("SKIP {}: pins the Rust validator layer (model→Value); the JS realm validates at the barrier, codec-encode is a serializer", case.name);
                continue;
            }
            Op::Encode => {
                let input = match (&case.input.json, &case.input.model) {
                    (Some(j), _) => json_to_value(j),
                    (None, Some(m)) => model_to_value(m),
                    _ => Err("no input".into()),
                };
                match input {
                    Ok(v) if !uncrossable(&v) => mk(vec![
                        ("op", Value::Text("encode".into())),
                        ("name", Value::Text(case.name.clone())),
                        ("value", v),
                    ]),
                    _ => mk(vec![
                        ("op", Value::Text("encodeLiteral".into())),
                        ("name", Value::Text(case.name.clone())),
                    ]),
                }
            }
            Op::Roundtrip => {
                let v = match (&case.input.json, &case.input.model) {
                    (Some(j), _) => json_to_value(j).unwrap_or_else(|e| panic!("{}: {e}", case.name)),
                    (None, Some(m)) => model_to_value(m).unwrap_or_else(|e| panic!("{}: {e}", case.name)),
                    _ => panic!("{}: no input", case.name),
                };
                assert!(
                    !uncrossable(&v),
                    "{}: roundtrip inputs must cross the barrier",
                    case.name
                );
                mk(vec![
                    ("op", Value::Text("encode".into())),
                    ("name", Value::Text(case.name.clone())),
                    ("value", v),
                ])
            }
            Op::Decode => {
                let h = case.input.hex.clone().expect("decode needs hex");
                mk(vec![
                    ("op", Value::Text("decode".into())),
                    ("name", Value::Text(case.name.clone())),
                    ("hex", Value::Text(h)),
                ])
            }
        };
        letters.push((case.name.clone(), payload));
        plan.insert(case.name.clone(), case);
    }

    for (_name, payload) in &letters {
        let letter = Letter::new("vec.case@1", payload.clone(), None, None, vec![]).unwrap();
        ctx.post(&addr("jv", "vec"), letter, vec![], None);
    }
    ctx.run_until_idle(10_000);

    let mut replies: HashMap<String, Value> = HashMap::new();
    for e in ctx.drain_external() {
        let v = decode(&e.letter.payload, &DecodeOptions::default()).expect("canonical reply");
        let name = match field(&v, "name") {
            Some(Value::Text(s)) => s.clone(),
            other => panic!("reply name: {other:?}"),
        };
        replies.insert(name, v);
    }
    assert_eq!(
        replies.len(),
        letters.len(),
        "every posted case answered (a strike or a lost reply is a failure); missing: {:?}; dead: {:?}",
        letters
            .iter()
            .map(|(n, _)| n)
            .filter(|n| !replies.contains_key(*n))
            .collect::<Vec<_>>(),
        ctx.dead_letters()
            .iter()
            .map(|d| d.letter.kind.clone())
            .collect::<Vec<_>>(),
    );

    for (name, _) in &letters {
        let case = plan[name.as_str()];
        let reply = &replies[name];
        let err = field(reply, "err").and_then(|v| match v {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        });
        let ok = field(reply, "ok").and_then(|v| match v {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        });
        let res: Result<(), String> = (|| {
            match case.op {
                Op::Encode | Op::Roundtrip => {
                    let input = match (&case.input.json, &case.input.model) {
                        (Some(j), _) => json_to_value(j),
                        (None, Some(m)) => model_to_value(m),
                        _ => Err("no input".into()),
                    };
                    if let Some(code) = &case.expect.reject {
                        match &err {
                            Some(e) if e == code => return Ok(()),
                            other => {
                                return Err(format!("want reject {code}, got {other:?} / ok={ok:?}"))
                            }
                        }
                    }
                    let hex_ok = ok.as_deref().unwrap_or_else(|| {
                        panic!("{name}: no ok hex, err={err:?}")
                    });
                    if let Some(want) = &case.expect.hex {
                        if hex_ok != want {
                            return Err(format!("hex mismatch: got {hex_ok}, want {want}"));
                        }
                    }
                    if case.op == Op::Roundtrip {
                        let v = input.unwrap_or_else(|e| panic!("{name}: {e}"));
                        let bytes = unhex(hex_ok);
                        let back = decode(&bytes, &DecodeOptions::default())
                            .map_err(|e| format!("JS bytes re-decode: {e:?}"))?;
                        if !same_value(&v, &back) {
                            return Err(format!("roundtrip value mismatch: {v:?} vs {back:?}"));
                        }
                        if encode(&v) != bytes {
                            return Err("roundtrip byte mismatch (JS encode != Rust canon)".into());
                        }
                    }
                    Ok(())
                }
                Op::Decode => {
                    if let Some(code) = &case.expect.reject {
                        match &err {
                            Some(e) if e == code => return Ok(()),
                            other => {
                                return Err(format!("want reject {code}, got {other:?} / ok={ok:?}"))
                            }
                        }
                    }
                    let Some(hex_ok) = &ok else {
                        return Err(format!("decoded ok expected, got err={err:?}"));
                    };
                    // What the JS realm reports back: canonical
                    // re-encode + (when crossable) the value itself.
                    if let Some(m) = &case.expect.model {
                        if let Model::Big { .. } = m {
                            let want = big_decimal(m);
                            let got = match field(reply, "big") {
                                Some(Value::Text(s)) => s.clone(),
                                other => panic!("{name}: big report: {other:?}"),
                            };
                            if got != want {
                                return Err(format!("bigint mismatch: got {got}, want {want}"));
                            }
                            return Ok(());
                        }
                        let expected = model_to_value(m).unwrap();
                        let canonical = hex(&encode(&expected));
                        if hex_ok != &canonical {
                            return Err(format!(
                                "canonical re-encode mismatch: got {hex_ok}, want {canonical}"
                            ));
                        }
                        return Ok(());
                    }
                    let expected = case
                        .expect
                        .json
                        .as_ref()
                        .map(|j| json_to_value(j).expect("json expect"))
                        .or_else(|| {
                            case.expect.model.as_ref().map(|m| model_to_value(m).expect("model expect"))
                        });
                    let Some(expected) = expected else {
                        // hex-only expectation
                        if let Some(want) = &case.expect.hex {
                            if hex_ok != want {
                                return Err(format!("hex mismatch: got {hex_ok}, want {want}"));
                            }
                        }
                        return Ok(());
                    };
                    let canonical = hex(&encode(&expected));
                    if hex_ok != &canonical {
                        return Err(format!(
                            "canonical re-encode mismatch: got {hex_ok}, want {canonical}"
                        ));
                    }
                    if let Some(val) = field(reply, "val") {
                        if !same_value(val, &expected) {
                            return Err(format!(
                                "value mismatch: got {val:?}, want {expected:?}"
                            ));
                        }
                    }
                    Ok(())
                }
                Op::Digest => unreachable!("digest cases skipped above"),
            }
        })();
        if let Err(e) = res {
            failures.push(format!("{file}/{}: {e}", case.name));
        }
    }
}

/// Round 49 — the prelude codec IS the second-realm decoder the
/// corpus was written for. Skip-with-message when the engine
/// artifact is absent (scripts/build-guest-js.sh).
#[test]
fn prelude_codec_pins_the_corpus_through_the_real_engine() {
    let engine_path = Path::new("tests/artifacts/mwydyn_guest_js.wasm");
    if !engine_path.exists() {
        eprintln!("SKIP: engine artifact missing — run scripts/build-guest-js.sh");
        return;
    }
    let engine_bytes = std::fs::read(engine_path).expect("engine artifact");
    let composed = mwydyn_js::compose(&engine_bytes, GUEST_SCRIPT.as_bytes()).expect("compose");

    let mut ctx = Context::new("jv");
    ctx.spawn(
        Spawn::new(
            Some("vec"),
            Box::new(move || {
                let engine = WasmEngine::new().expect("engine");
                Box::new(WasmActor::from_compiled(&engine, &composed.wasm).expect("guest"))
            }),
        )
        .supervision(Supervision::Restart),
    )
    .unwrap();

    let mut failures = Vec::new();
    run_corpus(&mut ctx, "cbor-profile-v0.json", &mut failures);
    run_corpus(&mut ctx, "blob-refs-v0.json", &mut failures);
    assert!(
        failures.is_empty(),
        "{} corpus failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
