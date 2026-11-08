//! The SQLite provider (§8.5) — the file-format journal. The owner
//! uses SQLite everywhere as a file format and persistence system;
//! this is the durable tier's first real implementation, behind the
//! `sqlite` cargo feature (default ON).
//!
//! Durability point: WAL journal mode + `synchronous=FULL` — a
//! returned COMMIT is crash-consistent (§8.2: durable before
//! deliverable). Storage law honored: physically append-only
//! (entries never updated), forked prefixes NOT copied — child
//! reads delegate to parent lineages exactly like the in-memory
//! provider, so a ghost branch costs nothing until it grows its own
//! tail. Envelopes are stored as their canonical CBOR (the
//! projection the digest law already trusts) and VERIFIED on read:
//! a stored row whose recomputed checksum differs is an integrity
//! failure, never silent divergence.

use std::path::Path;
use std::sync::Mutex;

use mwydyn_data::{encode, Envelope, Value};
use rusqlite::{params, Connection};

use crate::context::ClockFace;
use crate::journal::{
    Commit, JournalError, JournalFace, JournalOp, LineageInfo,
};

pub struct SQLiteJournal {
    /// rusqlite connections are Send but not Sync; the face is
    /// Send + Sync (round 21), so the connection sits behind a
    /// Mutex. The lock is held for the duration of one face call —
    /// no user code runs inside.
    conn: Mutex<Connection>,
    clock: Box<dyn ClockFace>,
}

impl SQLiteJournal {
    pub fn open(path: &Path, clock: Box<dyn ClockFace>) -> Result<SQLiteJournal, JournalError> {
        let conn = Connection::open(path).map_err(sql_err)?;
        Self::init(conn, clock)
    }

    /// SQL-path testing without a file (`:memory:`).
    pub fn open_memory(clock: Box<dyn ClockFace>) -> Result<SQLiteJournal, JournalError> {
        let conn = Connection::open_in_memory().map_err(sql_err)?;
        Self::init(conn, clock)
    }

    fn init(
        conn: Connection,
        clock: Box<dyn ClockFace>,
    ) -> Result<SQLiteJournal, JournalError> {
        conn.pragma_update(None, "journal_mode", "WAL").map_err(sql_err)?;
        conn.pragma_update(None, "synchronous", "FULL").map_err(sql_err)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS lineages (
                context   TEXT    NOT NULL,
                id        INTEGER NOT NULL,
                parent_id INTEGER,
                fork_seq  INTEGER,
                PRIMARY KEY (context, id)
            );
            CREATE TABLE IF NOT EXISTS entries (
                context  TEXT    NOT NULL,
                lineage  INTEGER NOT NULL,
                seq      INTEGER NOT NULL,
                to_actor TEXT    NOT NULL,
                checksum TEXT    NOT NULL,
                blob     BLOB    NOT NULL,
                PRIMARY KEY (context, lineage, seq)
            );
            CREATE INDEX IF NOT EXISTS idx_entries_pending
                ON entries (context, to_actor, lineage, seq);
            CREATE TABLE IF NOT EXISTS cursors (
                context  TEXT    NOT NULL,
                lineage  INTEGER NOT NULL,
                actor    TEXT    NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (context, lineage, actor)
            );
            CREATE TABLE IF NOT EXISTS facts (
                context TEXT NOT NULL,
                kind    TEXT NOT NULL,
                data    BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
                context TEXT NOT NULL,
                key     TEXT NOT NULL,
                value   TEXT NOT NULL,
                PRIMARY KEY (context, key)
            );",
        )
        .map_err(sql_err)?;
        Ok(SQLiteJournal {
            conn: Mutex::new(conn),
            clock,
        })
    }
}

fn sql_err(e: rusqlite::Error) -> JournalError {
    JournalError::Storage(format!("sqlite: {e}"))
}

fn active_of(conn: &Connection, context: &str) -> Result<u64, JournalError> {
    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE context=?1 AND key='active'",
            params![context],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => JournalError::UnknownContext,
            e => sql_err(e),
        })?;
    v.parse().map_err(|_| JournalError::Storage("bad active lineage".into()))
}

/// Root lineage + active pointer, created lazily per context.
fn ensure_context(conn: &Connection, context: &str) -> Result<u64, JournalError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM lineages WHERE context=?1 AND id=1)",
            params![context],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    if !exists {
        conn.execute(
            "INSERT INTO lineages (context, id, parent_id, fork_seq) VALUES (?1, 1, NULL, NULL)",
            params![context],
        )
        .map_err(sql_err)?;
        conn.execute(
            "INSERT INTO meta (context, key, value) VALUES (?1, 'active', '1')",
            params![context],
        )
        .map_err(sql_err)?;
    }
    active_of(conn, context)
}

fn next_seq(conn: &Connection, context: &str, lineage: u64) -> Result<u64, JournalError> {
    let local: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM entries WHERE context=?1 AND lineage=?2",
            params![context, lineage],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    let fork: Option<i64> = conn
        .query_row(
            "SELECT fork_seq FROM lineages WHERE context=?1 AND id=?2",
            params![context, lineage],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    Ok(local.max(fork.unwrap_or(0)) as u64 + 1)
}

/// Total coverage of a lineage: its own tail, or its fork point if
/// it has no tail yet (the shared prefix belongs to the parent).
fn covered(conn: &Connection, context: &str, lineage: u64) -> Result<u64, JournalError> {
    let local: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM entries WHERE context=?1 AND lineage=?2",
            params![context, lineage],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    let fork: Option<i64> = conn
        .query_row(
            "SELECT fork_seq FROM lineages WHERE context=?1 AND id=?2",
            params![context, lineage],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    Ok(local.max(fork.unwrap_or(0)) as u64)
}

/// Partition [lo, hi] on `start_lineage` into PHYSICAL segments
/// (lineage, from, to), delegating shared prefixes to parents.
fn segments(
    conn: &Connection,
    context: &str,
    start: u64,
    lo: u64,
    hi: u64,
) -> Result<Vec<(u64, u64, u64)>, JournalError> {
    let total = covered(conn, context, start)?;
    if hi > total {
        return Err(JournalError::BeyondHistory);
    }
    let mut out = Vec::new();
    let mut cur = start;
    let seg_lo = lo;
    let mut seg_hi = hi;
    while seg_lo <= seg_hi {
    let fork: Option<(u64, u64)> = conn
        .query_row(
            "SELECT parent_id, fork_seq FROM lineages WHERE context=?1 AND id=?2",
            params![context, cur],
            |r| {
                let p: Option<i64> = r.get(0)?;
                let f: Option<i64> = r.get(1)?;
                Ok(match (p, f) {
                    (Some(p), Some(f)) => Some((p as u64, f as u64)),
                    _ => None,
                })
            },
        )
        .map_err(sql_err)?;
        match fork {
            Some((parent, fork_at)) if seg_lo <= fork_at => {
                if seg_hi > fork_at {
                    // local tail beyond the fork belongs to this lineage
                    out.push((cur, fork_at + 1, seg_hi));
                    seg_hi = fork_at;
                }
                // shared prefix delegates to the parent
                cur = parent;
            }
            _ => {
                out.push((cur, seg_lo, seg_hi));
                break;
            }
        }
    }
    // Physical segments are discovered parent-tail-first; the reader
    // consumes ascending seq order.
    out.sort_by_key(|(_, from, _)| *from);
    Ok(out)
}

fn decode_entry(blob: &[u8], checksum: &str) -> Result<Envelope, JournalError> {
    let v = mwydyn_data::decode(blob, &mwydyn_data::DecodeOptions::default())
        .map_err(|_| JournalError::Integrity("entry blob does not decode"))?;
    let env = Envelope::from_value(&v)
        .map_err(|_| JournalError::Integrity("entry blob is not an envelope"))?;
    if env.checksum() != checksum {
        return Err(JournalError::Integrity("checksum mismatch on read"));
    }
    Ok(env)
}

impl JournalFace for SQLiteJournal {
    fn commit(&mut self, context: &str, ops: Vec<JournalOp>) -> Result<Commit, JournalError> {
        let now = self.clock.now_micros();
        let mut conn = self.conn.lock().expect("journal mutex poisoned");
        let tx = conn.transaction().map_err(sql_err)?;
        let mut active = ensure_context(&tx, context)?;
        let mut first_seq: Option<u64> = None;
        let mut count = 0usize;
        let mut appended: Vec<Envelope> = Vec::new();
        for op in ops {
            match op {
                JournalOp::Append(p) => {
                    let seq = next_seq(&tx, context, active)?;
                    let env = Envelope::new(
                        p.letter.clone(),
                        seq,
                        p.from.clone(),
                        p.to.clone(),
                        Some(now),
                        p.producer.clone(),
                    )
                    .map_err(|_| JournalError::BadRequest("invalid letter in append"))?;
                    let checksum = env.checksum();
                    let blob = encode(&env.to_value());
                    tx.execute(
                        "INSERT INTO entries (context, lineage, seq, to_actor, checksum, blob)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![context, active, seq, p.to_actor, checksum, blob],
                    )
                    .map_err(sql_err)?;
                    appended.push(env);
                    first_seq.get_or_insert(seq);
                    count += 1;
                }
                JournalOp::Ack { actor, position } => {
                    // Log owns acks (§3.1): the cursor advance rides
                    // this transaction.
                    tx.execute(
                        "INSERT INTO cursors (context, lineage, actor, position)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(context, lineage, actor)
                         DO UPDATE SET position=excluded.position",
                        params![context, active, actor, position],
                    )
                    .map_err(sql_err)?;
                }
                JournalOp::DeadLetter { actor, why } => {
                    let data = encode(
                        &Value::map(vec![
                            ("actor".into(), Value::Text(actor.clone())),
                            ("why".into(), Value::Text(why)),
                        ])
                        .map_err(|_| JournalError::BadRequest("dead-letter data"))?,
                    );
                    tx.execute(
                        "INSERT INTO facts (context, kind, data) VALUES (?1, 'dead_letter', ?2)",
                        params![context, data],
                    )
                    .map_err(sql_err)?;
                }
                JournalOp::Fork { parent_seq } => {
                    let next: i64 = tx
                        .query_row(
                            "SELECT COALESCE(MAX(id), 0) + 1 FROM lineages WHERE context=?1",
                            params![context],
                            |r| r.get(0),
                        )
                        .map_err(sql_err)?;
                    tx.execute(
                        "INSERT INTO lineages (context, id, parent_id, fork_seq)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![context, next, active, parent_seq],
                    )
                    .map_err(sql_err)?;
                    tx.execute(
                        "INSERT INTO meta (context, key, value) VALUES (?1, 'active', ?2)
                         ON CONFLICT(context, key) DO UPDATE SET value=excluded.value",
                        params![context, next.to_string()],
                    )
                    .map_err(sql_err)?;
                    active = next as u64;
                }
                JournalOp::Mint {
                    actor,
                    verb,
                    object,
                } => {
                    let data = encode(
                        &Value::map(vec![
                            ("actor".into(), Value::Text(actor)),
                            ("object".into(), Value::Text(object)),
                            ("verb".into(), Value::Text(verb)),
                        ])
                        .map_err(|_| JournalError::BadRequest("mint fact"))?,
                    );
                    tx.execute(
                        "INSERT INTO facts (context, kind, data) VALUES (?1, 'mint', ?2)",
                        params![context, data],
                    )
                    .map_err(sql_err)?;
                }
            }
        }
        let lineage = active;
        tx.commit().map_err(sql_err)?; // THE durability point (§8.2)
        Ok(Commit {
            lineage,
            first_seq: first_seq.unwrap_or(0),
            count,
            commit_ts: now,
            appended,
        })
    }

    fn read(
        &self,
        context: &str,
        lineage: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<Envelope>, JournalError> {
        let conn = self.conn.lock().expect("journal mutex poisoned");
        let mut out = Vec::new();
        for (lin, lo, hi) in segments(&conn, context, lineage, from, to)? {
            let mut stmt = conn
                .prepare(
                    "SELECT seq, checksum, blob FROM entries
                     WHERE context=?1 AND lineage=?2 AND seq BETWEEN ?3 AND ?4 ORDER BY seq",
                )
                .map_err(sql_err)?;
            let mut rows = stmt
                .query(params![context, lin, lo, hi])
                .map_err(sql_err)?;
            while let Some(row) = rows.next().map_err(sql_err)? {
                let seq: u64 = row.get(0).map_err(sql_err)?;
                let checksum: String = row.get(1).map_err(sql_err)?;
                let blob: Vec<u8> = row.get(2).map_err(sql_err)?;
                let env = decode_entry(&blob, &checksum)?;
                if env.seq != seq {
                    return Err(JournalError::Integrity("row seq does not match index"));
                }
                out.push(env);
            }
        }
        Ok(out)
    }

    fn pending(&self, context: &str, actor: &str) -> Result<Vec<Envelope>, JournalError> {
        let conn = self.conn.lock().expect("journal mutex poisoned");
        let active = active_of(&conn, context)?;
        let cursor = Self::cursor_of(&conn, context, active, actor)?.unwrap_or(0);
        let total = covered(&conn, context, active)?;
        if cursor >= total {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for (lin, lo, hi) in segments(&conn, context, active, cursor + 1, total)? {
            let mut stmt = conn
                .prepare(
                    "SELECT seq, checksum, blob FROM entries
                     WHERE context=?1 AND lineage=?2 AND to_actor=?3
                       AND seq BETWEEN ?4 AND ?5 ORDER BY seq",
                )
                .map_err(sql_err)?;
            let mut rows = stmt
                .query(params![context, lin, actor, lo, hi])
                .map_err(sql_err)?;
            while let Some(row) = rows.next().map_err(sql_err)? {
                let seq: u64 = row.get(0).map_err(sql_err)?;
                let checksum: String = row.get(1).map_err(sql_err)?;
                let blob: Vec<u8> = row.get(2).map_err(sql_err)?;
                let mut env = decode_entry(&blob, &checksum)?;
                if env.seq != seq {
                    return Err(JournalError::Integrity("row seq does not match index"));
                }
                env.seq = seq;
                out.push(env);
            }
        }
        Ok(out)
    }

    fn pending_count(&self, context: &str, actor: &str) -> Result<usize, JournalError> {
        let conn = self.conn.lock().expect("journal mutex poisoned");
        let active = active_of(&conn, context)?;
        let cursor = Self::cursor_of(&conn, context, active, actor)?.unwrap_or(0);
        let total = covered(&conn, context, active)?;
        if cursor >= total {
            return Ok(0);
        }
        let mut n = 0usize;
        for (lin, lo, hi) in segments(&conn, context, active, cursor + 1, total)? {
            n += conn
                .query_row(
                    "SELECT COUNT(*) FROM entries
                     WHERE context=?1 AND lineage=?2 AND to_actor=?3
                       AND seq BETWEEN ?4 AND ?5",
                    params![context, lin, actor, lo, hi],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(sql_err)? as usize;
        }
        Ok(n)
    }

    fn cursor(
        &self,
        context: &str,
        lineage: u64,
        actor: &str,
    ) -> Result<Option<u64>, JournalError> {
        let conn = self.conn.lock().expect("journal mutex poisoned");
        Self::cursor_of(&conn, context, lineage, actor)
    }

    fn active_lineage(&self, context: &str) -> Result<u64, JournalError> {
        let conn = self.conn.lock().expect("journal mutex poisoned");
        ensure_context(&conn, context)
    }

    fn lineages(&self, context: &str) -> Result<Vec<LineageInfo>, JournalError> {
        let conn = self.conn.lock().expect("journal mutex poisoned");
        let active = ensure_context(&conn, context)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, parent_id, fork_seq FROM lineages
                 WHERE context=?1 ORDER BY id",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query(params![context])
            .map_err(sql_err)?
            .mapped(|r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })
            .collect::<Result<Vec<_>, _>>().map_err(sql_err)?;
        Ok(rows
            .into_iter()
            .map(|(id, parent, fork)| LineageInfo {
                id,
                parent: match (parent, fork) {
                    (Some(p), Some(f)) => Some((p as u64, f as u64)),
                    _ => None,
                },
                active: id == active,
            })
            .collect())
    }
}

impl SQLiteJournal {
    fn cursor_of(
        conn: &Connection,
        context: &str,
        lineage: u64,
        actor: &str,
    ) -> Result<Option<u64>, JournalError> {
        // No cursor row is NOT an error — it means "nothing acked on
        // this lineage yet"; the fork chain then supplies the clamp.
        let row: Option<i64> = match conn.query_row(
            "SELECT position FROM cursors WHERE context=?1 AND lineage=?2 AND actor=?3",
            params![context, lineage, actor],
            |r| r.get(0),
        ) {
            Ok(p) => Some(p),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(sql_err(e)),
        };
        if let Some(p) = row {
            return Ok(Some(p as u64));
        }
        // Lazy clamp through the fork chain: a child lineage inherits
        // min(parent cursor, fork point).
        let parent_fork: Option<(i64, i64)> = match conn.query_row(
            "SELECT parent_id, fork_seq FROM lineages WHERE context=?1 AND id=?2",
            params![context, lineage],
            |r| {
                let p: Option<i64> = r.get(0)?;
                let f: Option<i64> = r.get(1)?;
                Ok(match (p, f) {
                    (Some(p), Some(f)) => Some((p, f)),
                    _ => None,
                })
            },
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(sql_err(e)),
        };
        match parent_fork {
            Some((p, f)) => {
                let inherited = Self::cursor_of(conn, context, p as u64, actor)?;
                Ok(inherited.map(|c| c.min(f as u64)))
            }
            _ => Ok(None),
        }
    }
}
