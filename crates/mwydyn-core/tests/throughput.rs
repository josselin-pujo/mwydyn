//! Round 32 group-commit throughput: N letters through a Context
//! with batched journal commits (RAM tier — measures the framework
//! overhead per letter, excluding fsync).

use mwydyn_core::{Actor, Context, Effect, Host, MsgCtx, Spawn};
use mwydyn_data::{Address, DecodeOptions, Letter, Value};

fn addr(context: &str, name: &str) -> Address {
    Address::Named { context: context.into(), name: name.into() }
}

fn cbor_int(n: i64) -> Vec<u8> {
    mwydyn_data::encode(&Value::Int(n))
}

struct Bouncer {
    target: Address,
}
impl Actor for Bouncer {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(Bouncer { target: self.target.clone() })
    }
    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        let count = match mwydyn_data::decode(&letter.payload, &DecodeOptions::default()) {
            Ok(Value::Int(n)) => n,
            _ => return vec![],
        };
        if count <= 0 {
            return vec![];
        }
        vec![Effect::Post {
            to: self.target.clone(),
            letter: Letter::new(
                "test.bounce@1",
                cbor_int(count - 1),
                None,
                None,
                vec![],
            )
            .unwrap(),
        }]
    }
}

#[test]
fn context_group_commit_throughput() {
    let mut ctx = Context::new("perf");
    let n = 50_000u64;
    ctx.spawn(
        Spawn::new(Some("a"), Box::new(|| {
            Box::new(Bouncer { target: addr("perf", "b") })
        })),
    )
    .unwrap();
    ctx.spawn(
        Spawn::new(Some("b"), Box::new(|| {
            Box::new(Bouncer { target: addr("perf", "a") })
        })),
    )
    .unwrap();

    let kick = Letter::new(
        "test.bounce@1",
        cbor_int(n as i64),
        None,
        None,
        vec![],
    )
    .unwrap();
    ctx.post(&addr("perf", "a"), kick, vec![], None);

    let start = std::time::Instant::now();
    ctx.run_until_idle(10_000_000);
    let elapsed = start.elapsed();
    let throughput = n as f64 / elapsed.as_secs_f64();
    eprintln!(
        "Context pump (group commit, RAM tier): {} letters in {:?} → {:.0} letters/s",
        n, elapsed, throughput
    );
    assert!(throughput > 10_000.0, ">10k letters/s on RAM tier (got {:.0})", throughput);
}
