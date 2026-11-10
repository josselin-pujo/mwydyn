//! The ticker (§6) — the well-known NAME every context binds, and
//! the only wall-clock reader (through the injected clock face).
//! Arm requests carry DURATIONS, never timestamps; ticks are
//! ordinary letters, so rewind un-happens them and replay re-fires
//! them from the log watermark (M2 carries that part).

use mwydyn_data::{Address, Letter, Value};

use crate::actor::{Actor, Effect, Host, Level, MsgCtx};

pub const TIMER_ARM_KIND: &str = "mwydyn.timer.arm@1";
pub const TIMER_TICK_KIND: &str = "mwydyn.timer.tick@1";
pub const TIMER_POLL_KIND: &str = "mwydyn.timer.poll@1";

#[derive(Debug, Clone)]
struct Scheduled {
    fire_at_micros: i64,
    to: Address,
    correlation: Option<String>,
}

/// The timer service actor. Timeout idiom = race the tick against
/// the expected letter; first message wins. There is no other clock.
#[derive(Clone)]
pub struct Ticker {
    schedule: Vec<Scheduled>,
}

impl Default for Ticker {
    fn default() -> Self {
        Self::new()
    }
}

impl Ticker {
    pub fn new() -> Ticker {
        Ticker {
            schedule: Vec::new(),
        }
    }

    /// The arm payload is canonical CBOR `{"afterUs": <int>}`.
    pub fn arm_payload(after_us: u64) -> Vec<u8> {
        mwydyn_data::encode(
            &Value::map(vec![("afterUs".into(), Value::Int(after_us as i64))])
                .expect("fixed key"),
        )
    }

    fn after_us(payload: &[u8]) -> Option<u64> {
        let v = mwydyn_data::decode(payload, &mwydyn_data::DecodeOptions::default()).ok()?;
        let Value::Map(pairs) = v else {
            return None;
        };
        for (k, val) in pairs {
            if k == "afterUs" {
                if let Value::Int(us) = val {
                    if us >= 0 {
                        return Some(us as u64);
                    }
                }
                return None;
            }
        }
        None
    }
}

impl Actor for Ticker {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(self.clone())
    }
    fn receive(&mut self, ctx: &MsgCtx, host: &Host, letter: &Letter) -> Vec<Effect> {
        match letter.kind.as_str() {
            TIMER_ARM_KIND => {
                let now = host.now_micros();
                match Self::after_us(&letter.payload) {
                    Some(after_us) => {
                        self.schedule.push(Scheduled {
                            fire_at_micros: now.saturating_add(after_us as i64),
                            to: ctx.from.clone(),
                            correlation: letter.correlation.clone(),
                        });
                        Vec::new()
                    }
                    None => vec![Effect::log(
                        Level::Warn,
                        "ticker: malformed arm payload — ignored",
                    )],
                }
            }
            TIMER_POLL_KIND => {
                let now = host.now_micros();
                let due: Vec<Scheduled> = self
                    .schedule
                    .extract_if(.., |s| s.fire_at_micros <= now)
                    .collect();
                // The tick payload is canonical CBOR `{"firedAtUs": <int>}`
                // — the payload wall (round 33) admits nothing else.
                let fired_at = Value::map(vec![("firedAtUs".into(), Value::Int(now))])
                    .expect("fixed key");
                due.into_iter()
                    .map(|s| Effect::Post {
                        to: s.to,
                        letter: Letter::new(
                            TIMER_TICK_KIND,
                            mwydyn_data::encode(&fired_at),
                            None,
                            s.correlation,
                            Vec::new(),
                        )
                        .expect("fixed kind"),
                    })
                    .collect()
            }
            other => vec![Effect::log(
                Level::Warn,
                format!("ticker: ignoring unknown kind {other}"),
            )],
        }
    }

    fn wants_poll(&self) -> bool {
        !self.schedule.is_empty()
    }
}
