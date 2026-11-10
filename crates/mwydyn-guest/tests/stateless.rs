//! The SDK's stateless path, host-target: the generated exports
//! decode a real barrier request, run the actor, and answer with a
//! canonical effects array. The ambient stub refuses (round 40), so
//! refusals are observable here too.

use mwydyn_data::barrier::{BarrierEffect, BarrierRequest};
use mwydyn_data::{address_to_value, decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_guest::{prelude::*, ValueExt};

/// The actor under test: echoes the payload to the replyTo named in
/// it, or back to the sender when no routing is present.
struct Echo;

impl Actor for Echo {
    fn receive(&mut self, letter: Letter, cx: &Cx) -> Vec<Effect> {
        let payload = letter.payload_value().expect("canonical payload");
        // the §9.5 idiom: third-place routing by convention, the
        // sender as the fallback return address
        if let Some(via) = Effect::reply_via(&letter, "test.echo@1", encode(&payload)).unwrap() {
            return vec![via];
        }
        vec![Effect::reply(&letter, cx, "test.echo@1", encode(&payload)).unwrap()]
    }
}

mwydyn_guest::entry!(Echo);

/// Stage a barrier request in guest scratch; returns (ptr, len).
fn stage(req: &BarrierRequest) -> (*mut u8, usize) {
    let bytes = encode(&req.to_value());
    let ptr = mwydyn_guest::alloc(bytes.len());
    unsafe {
        std::slice::from_raw_parts_mut(ptr, bytes.len()).copy_from_slice(&bytes);
    }
    (ptr, bytes.len())
}

/// Drive the generated `process` and decode the effects array.
fn run(req: &BarrierRequest) -> Vec<BarrierEffect> {
    let (ptr, len) = stage(req);
    let resp = unsafe { process(ptr, len) };
    let bytes = unsafe { mwydyn_guest::read_block(resp) };
    let value = decode(&bytes, &DecodeOptions::default()).expect("canonical response");
    BarrierEffect::effects_from_value(&value).expect("effects shape")
}

fn kick(payload: Value, correlation: Option<String>) -> Letter {
    Letter::new("test.kick@1", encode(&payload), None, correlation, vec![]).unwrap()
}

fn sender() -> Address {
    Address::Named {
        context: "pp".into(),
        name: "ping".into(),
    }
}

#[test]
fn stateless_declares_zero() {
    assert_eq!(mwydyn_decl(), 0, "stateless = explicit zero");
}

#[test]
fn reply_conventions_both_paths() {
    // routed: replyTo names a third place
    let third = Address::Instance("external".into());
    let payload = Value::map(vec![("replyTo".into(), address_to_value(&third))]).unwrap();
    let effects = run(&BarrierRequest {
        from: sender(),
        letter: kick(payload, Some("ex-2".into())),
    });
    match &effects[0] {
        BarrierEffect::Post { to, letter } => {
            assert_eq!(*to, third, "replyTo wins for third-place routing");
            assert_eq!(letter.correlation.as_deref(), Some("ex-2"));
            assert_eq!(letter.kind, "test.echo@1");
        }
    }

    // unrouted: the sender is the return address
    let payload = Value::map(vec![("n".into(), Value::Int(1))]).unwrap();
    let effects = run(&BarrierRequest {
        from: sender(),
        letter: kick(payload, Some("ex-3".into())),
    });
    match &effects[0] {
        BarrierEffect::Post { to, letter } => {
            assert_eq!(*to, sender(), "no replyTo → sender");
            assert_eq!(letter.correlation.as_deref(), Some("ex-3"));
        }
    }
}

#[test]
fn bad_request_bytes_are_a_strike_not_a_silence() {
    let bytes = [0xFF, 0x00, 0x01]; // byzantine garbage
    let ptr = mwydyn_guest::alloc(bytes.len());
    unsafe {
        std::slice::from_raw_parts_mut(ptr, bytes.len()).copy_from_slice(&bytes);
    }
    // through the generated export a panic aborts inside the extern
    // boundary (the trap IS the strike); the plain inner path lets
    // the test SEE the panic
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: ptr/len staged above; the slot is a fresh Echo.
        unsafe {
            mwydyn_guest::process_impl(
                &MWYDYN_SLOT,
                || Box::new(Echo) as Box<dyn Actor>,
                ptr,
                bytes.len(),
            )
        }
    }));
    assert!(result.is_err(), "non-canonical request must panic (strike)");
}

#[test]
fn host_refusals_are_observable() {
    // the host-target stub refuses every ambient call (round 40) —
    // caps surfaces zero rows, never a fake success
    let from = sender();
    let cx = Cx::new(&from);
    assert!(cx.caps().is_empty(), "no fake capability rows");
    assert!(
        cx.blob_put(vec![1, 2, 3], "text/plain").is_err(),
        "no fake puts on a refusing host"
    );
}

#[test]
fn value_ext_reads_typed_fields() {
    let v = Value::map(vec![
        ("s".into(), Value::Text("hi".into())),
        ("b".into(), Value::Bytes(vec![1, 2])),
        ("i".into(), Value::Int(-3)),
        (
            "a".into(),
            address_to_value(&Address::Instance("external".into())),
        ),
    ])
    .unwrap();
    assert_eq!(v.field_str("s"), Some("hi"));
    assert_eq!(v.field_bytes("b"), Some(&[1u8, 2][..]));
    assert_eq!(v.field_int("i"), Some(-3));
    assert_eq!(
        v.field_addr("a"),
        Some(Address::Instance("external".into()))
    );
    assert_eq!(v.field_str("missing"), None);
}
