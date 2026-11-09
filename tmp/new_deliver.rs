    fn deliver(&mut self, idx: usize, env: Envelope) -> bool {
        let actor_id = self.entries[idx].id.clone();

        // §3.5 transactional receive: fork pre-receive state. On
        // refusal, restore the fork — the state rolls back to the
        // ack boundary, the letter stays pending (Busy) or strikes
        // (permanent refusal). No divergence, ever.
        let fork = self.entries[idx].actor.as_ref().expect("live").fork();

        // Phase 1 — the pure transition (§3.5). The host face is
        // read-only; effects come back as data. A panic here is a
        // crash — supervision's problem (§3.2), not the message's.
        let outcome = {
            let Context {
                name,
                blobs,
                entries,
                clock,
                journal,
                ..
            } = self;
            let entry = &mut entries[idx];
            let host = Host::new(blobs, &**journal, name, &entry.rows, clock.now_micros());
            let ctx = MsgCtx {
                from: env.from.clone(),
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                entry
                    .actor
                    .as_mut()
                    .expect("deliver() is only called on live entries")
                    .receive(&ctx, &host, &env.letter)
            }))
        };
        let effects = match outcome {
            Ok(effects) => effects,
            Err(_) => {
                self.entries[idx].actor = Some(fork);
                self.handle_crash(idx);
                return true;
            }
        };

        // Phase 2 — validate ALL effects before committing ANY
        // (fail-before-compute, §3.5). Projected capacity counts this
        // list's own contributions.
        enum Block {
            Busy,
            Strike(&'static str),
        }
        let mut block: Option<Block> = None;
        let mut projected: HashMap<String, usize> = HashMap::new();
        for eff in &effects {
            match eff {
                Effect::Post { to, letter } => {
                    if letter.validate().is_err() {
                        block = Some(Block::Strike("invalid letter in effects"));
                        break;
                    }
                    if Self::is_external(to) {
                        continue;
                    }
                    let Some(tidx) = self.resolve(to) else {
                        block = Some(Block::Strike("unknown target in effects"));
                        break;
                    };
                    let tid = self.entries[tidx].id.clone();
                    let bound = self.entries[tidx].bound;
                    let inflight = projected.entry(tid.clone()).or_insert(0);
                    let queued = self.journal.pending_count(&self.name, &tid).unwrap_or(0);
                    if queued + *inflight >= bound {
                        block = Some(Block::Busy);
                        break;
                    }
                    *inflight += 1;
                }
                Effect::Mint {
                    source_verb,
                    source_object,
                    ..
                } => {
                    let held = self.entries[idx]
                        .rows
                        .iter()
                        .any(|r| r.verb == *source_verb && r.object == *source_object);
                    if !held {
                        block = Some(Block::Strike("mint without holding the source row"));
                        break;
                    }
                }
                Effect::Emit { .. } => {}
            }
        }

        // Phase 3 — classify (§3.5's two refusal classes). On ANY
        // refusal, restore the fork: the state rolls back to the ack
        // boundary, the letter stays pending (Busy: no attempt burned)
        // or burns an attempt (Strike: N strikes → dead-letter).
        match block {
            Some(Block::Busy) => {
                self.entries[idx].actor = Some(fork);
                self.telemetry.push((
                    Level::Debug,
                    format!("{actor_id}: target busy — state rolled back, no attempt burned"),
                ));
                return true;
            }
            Some(Block::Strike(why)) => {
                self.entries[idx].actor = Some(fork);
                let key = (actor_id.clone(), env.seq);
                let strikes = self.strikes.entry(key.clone()).or_insert(0);
                *strikes += 1;
                let n = *strikes;
                self.telemetry.push((
                    Level::Warn,
                    format!("{actor_id}: effects refused ({why}) — attempt {n}"),
                ));
                if n >= MAX_ATTEMPTS {
                    let _ = self.journal.commit(
                        &self.name,
                        vec![
                            JournalOp::DeadLetter {
                                actor: actor_id.clone(),
                                why: why.into(),
                            },
                            JournalOp::Ack {
                                actor: actor_id.clone(),
                                position: env.seq,
                            },
                        ],
                    );
                    self.strikes.remove(&key);
                    self.dead_letters.push(env);
                }
                return true;
            }
            None => {}
        }

        // Phase 4 — build ONE journal transaction: the effect posts,
        // the mint facts, and the ack (log owns acks, §3.1). The
        // mutated state is KEPT (fork is dropped).
        let mut ops: Vec<JournalOp> = Vec::new();
        for eff in &effects {
            match eff {
                Effect::Post { to, letter } => {
                    let to_actor = if Self::is_external(to) {
                        String::new()
                    } else {
                        self.resolve(to)
                            .map(|tidx| self.entries[tidx].id.clone())
                            .expect("validated above")
                    };
                    ops.push(JournalOp::Append(Pending {
                        letter: letter.clone(),
                        from: Address::Instance(actor_id.clone()),
                        to_actor,
                        to: to.clone(),
                        ack: Some(env.seq),
                        producer: actor_id.clone(),
                    }));
                }
                Effect::Mint { verb, object, .. } => {
                    self.entries[idx].rows.push(CapRow::new(verb, object));
                    ops.push(JournalOp::Mint {
                        actor: actor_id.clone(),
                        verb: verb.clone(),
                        object: object.clone(),
                    });
                }
                Effect::Emit { level, message } => {
                    self.telemetry.push((*level, message.clone()));
                }
            }
        }
        ops.push(JournalOp::Ack {
            actor: actor_id.clone(),
            position: env.seq,
        });
        let commit = self.journal.commit(&self.name, ops).expect("journal commit");
        for stamped in &commit.appended {
            if Self::is_external(&stamped.to) {
                self.external_outbox.push_back(stamped.clone());
            }
        }
        self.strikes.remove(&(actor_id.clone(), env.seq));
        true
    }

