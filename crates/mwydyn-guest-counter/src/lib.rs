//! mwydyn-guest-counter — the DURABLE example (rounds 37+39, on the
//! SDK since round 41). State lives in the actor struct — linear
//! memory — so the declared rollback covers it byte-exact, and
//! [`Durable`] carries it across process death. Every letter
//! increments and posts a receipt `{"count": n}` to the payload's
//! `replyTo`.

use mwydyn_guest::prelude::*;

/// The whole actor: a count in the struct (linear memory).
struct Counter {
    count: u32,
}

impl Actor for Counter {
    fn receive(&mut self, letter: Letter, _cx: &Cx) -> Vec<Effect> {
        // THE STATE — incremented per ATTEMPT; the durable_entry!
        // declaration owes the rollback that makes strikes not count
        self.count += 1;
        let receipt =
            Value::map(vec![("count".into(), Value::Int(self.count as i64))]).unwrap();
        match Effect::reply_via(&letter, "test.counter@1", encode(&receipt)).unwrap() {
            Some(e) => vec![e],
            None => fail!("counter: payload carries no replyTo routing"),
        }
    }
}

impl Durable for Counter {
    /// 4 bytes, little-endian — the state format is the actor's own.
    fn snapshot(&self) -> Vec<u8> {
        self.count.to_le_bytes().to_vec()
    }

    fn restore(&mut self, state: &[u8]) {
        self.count = u32::from_le_bytes(state.try_into().expect("4-byte snapshot"));
    }
}

mwydyn_guest::durable_entry!(Counter { count: 0 });
