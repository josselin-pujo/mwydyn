//! The SDK's durable path, host-target: `durable_entry!` sets the
//! §9.2 declaration bit, generates the §7.6 exports, and the
//! snapshot → restore round trip runs through the generated
//! `snapshot`/`restore` symbols exactly as the container drives
//! them. One sequential test: the generated slot is a singleton —
//! parallel tests in one binary would race it (on wasm the guest is
//! single-threaded by construction).

use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};
use mwydyn_guest::prelude::*;

struct Counter {
    count: u32,
}

impl Actor for Counter {
    fn receive(&mut self, _letter: Letter, _cx: &Cx) -> Vec<Effect> {
        self.count += 1;
        vec![]
    }
}

impl Durable for Counter {
    fn snapshot(&self) -> Vec<u8> {
        self.count.to_le_bytes().to_vec()
    }
    fn restore(&mut self, state: &[u8]) {
        self.count = u32::from_le_bytes(state.try_into().expect("4 bytes"));
    }
}

mwydyn_guest::durable_entry!(Counter { count: 0 });

fn stage(req: &mwydyn_data::barrier::BarrierRequest) -> (*mut u8, usize) {
    let bytes = encode(&req.to_value());
    let ptr = mwydyn_guest::alloc(bytes.len());
    unsafe {
        std::slice::from_raw_parts_mut(ptr, bytes.len()).copy_from_slice(&bytes);
    }
    (ptr, bytes.len())
}

/// One letter through the generated `process`; returns the raw
/// response block bytes.
fn deliver() -> Vec<u8> {
    let letter = Letter::new("test.count@1", encode(&Value::Null), None, None, vec![]).unwrap();
    let req = mwydyn_data::barrier::BarrierRequest {
        from: Address::Named {
            context: "pp".into(),
            name: "ping".into(),
        },
        letter,
    };
    let (ptr, len) = stage(&req);
    let resp = unsafe { process(ptr, len) };
    unsafe { mwydyn_guest::read_block(resp) }
}

#[test]
fn durable_declares_stateful_and_round_trips_state() {
    // §9.2: durable implies the stateful bit
    assert_eq!(mwydyn_decl(), 1, "durable implies stateful (bit 0)");

    // three letters → count = 3; empty effect lists answer `[]`
    let r1 = deliver();
    let r2 = deliver();
    let r3 = deliver();
    for (i, r) in [r1, r2, r3].into_iter().enumerate() {
        let v = decode(&r, &DecodeOptions::default()).expect("canonical");
        assert_eq!(v, Value::Array(vec![]), "empty effects #{i}");
    }

    // §7.6 SAVE through the generated export
    let out = snapshot();
    let bytes = unsafe { mwydyn_guest::read_block(out) };
    assert_eq!(bytes, 3u32.to_le_bytes(), "snapshot carries the live count");

    // §7.6 RESTORE — the host applies the boot blob before any
    // letter of the new lifetime; here it overwrites the live slot
    let state = 3u32.to_le_bytes();
    let ptr = mwydyn_guest::alloc(state.len());
    unsafe {
        std::slice::from_raw_parts_mut(ptr, state.len()).copy_from_slice(&state);
        restore(ptr, state.len())
    };
    deliver();
    let out = snapshot();
    let bytes = unsafe { mwydyn_guest::read_block(out) };
    assert_eq!(
        bytes,
        4u32.to_le_bytes(),
        "restored-then-advanced state is exact"
    );
}
