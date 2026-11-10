//! mwydyn-guest-store — the STATELESS example (round 40, on the SDK
//! since round 41): the ambient face as a plugin author meets it.
//! One letter in — `{"body": bytes, "replyTo": addr}` — one ambient
//! round (put → get → len) out, every refusal a FACT in the
//! receipt, posted back to `replyTo`.

use mwydyn_guest::prelude::*;
use mwydyn_guest::ValueExt;

struct Store;

impl Actor for Store {
    fn receive(&mut self, letter: Letter, cx: &Cx) -> Vec<Effect> {
        let order = letter.payload_value().expect("canonical order");
        let body = order
            .field_bytes("body")
            .expect("body bytes")
            .to_vec();

        let mut receipt = Vec::new();

        // blob.put (needs blob.write) — a refusal is observed, named
        let put = match cx.blob_put(body.clone(), "text/plain") {
            Ok(r) => {
                receipt.push(("putLen".into(), Value::Int(r.size() as i64)));
                Some(r)
            }
            Err(code) => {
                receipt.push(("putLen".into(), Value::Int(-1)));
                receipt.push(("putErr".into(), Value::Text(code.to_string())));
                None
            }
        };

        // blob.get + blob.len (need blob.read) — only with a ref
        if let Some(r) = &put {
            match cx.blob_get(r) {
                Ok(got) => {
                    receipt.push(("gotLen".into(), Value::Int(got.len() as i64)));
                    receipt.push(("match".into(), Value::Int((got == body) as i64)));
                }
                Err(code) => {
                    receipt.push(("gotLen".into(), Value::Int(-1)));
                    receipt.push(("getErr".into(), Value::Text(code.to_string())));
                }
            }
            match cx.blob_len(r) {
                Ok(n) => {
                    receipt.push(("lenOk".into(), Value::Int((n == body.len() as i64) as i64)))
                }
                Err(code) => receipt.push(("lenErr".into(), Value::Text(code.to_string()))),
            }
        }

        // caps: what THIS actor holds — the capped/uncapped
        // difference the embedding declared
        receipt.push(("capRows".into(), Value::Int(cx.caps().len() as i64)));

        finish(letter, receipt)
    }
}

/// Post the receipt to the payload's `replyTo`, correlation carried.
fn finish(letter: Letter, receipt: Vec<(String, Value)>) -> Vec<Effect> {
    let payload = Value::map(receipt).expect("fixed receipt keys");
    match Effect::reply_via(&letter, "test.store@1", encode(&payload)).unwrap() {
        Some(e) => vec![e],
        None => fail!("store: payload carries no replyTo routing"),
    }
}

mwydyn_guest::entry!(Store);
