//! mwydyn-guest-dedup — the STATEFUL example (round 41): an
//! exactly-once dedup window in one screenful. Correlation ids a
//! COMMITTED receive has seen come back as duplicates; fresh ones
//! pass with a receipt `{"corr", "first"}`.
//!
//! THE DECLARATION IS THE SEMANTICS: this crate builds two artifacts
//! from one source. The default (`stateful_entry!`) rolls the
//! seen-set back on refused transitions — a correlation whose
//! attempts all struck is still fresh. `--features stateless-build`
//! keeps the strike-dirtied marks instead — the author's problem,
//! by declaration (the round-37 dual proof, alive and observable).

use mwydyn_guest::prelude::*;

struct Dedup {
    /// Example-grade: unbounded and linear. A real window caps and
    /// buckets; the semantics it owes are identical.
    seen: Vec<String>,
}

impl Actor for Dedup {
    fn receive(&mut self, letter: Letter, _cx: &Cx) -> Vec<Effect> {
        let Some(corr) = letter.correlation.clone() else {
            fail!("dedup: correlation is required — it is the dedup key")
        };
        // the mark lands on EVERY attempt — what rollback undoes
        // (and what statelessness keeps) is the whole point
        let first = !self.seen.iter().any(|c| c == &corr);
        self.seen.push(corr.clone());
        let receipt = Value::map(vec![
            ("corr".into(), Value::Text(corr)),
            ("first".into(), Value::Bool(first)),
        ])
        .unwrap();
        match Effect::reply_via(&letter, "test.dedup@1", encode(&receipt)).unwrap() {
            Some(e) => vec![e],
            None => fail!("dedup: payload carries no replyTo routing"),
        }
    }
}

#[cfg(not(feature = "stateless-build"))]
mwydyn_guest::stateful_entry!(Dedup { seen: Vec::new() });

#[cfg(feature = "stateless-build")]
mwydyn_guest::entry!(Dedup { seen: Vec::new() });
