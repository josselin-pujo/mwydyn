//! The SQLite blob provider (§4/§8.5 pattern applied to blobs) —
//! the durable tier behind the `sqlite` cargo feature. Same file
//! format law as the journal: content-addressed rows, canonical
//! media types hash-bound into the digest (round 12), verify-on-read
//! (a row that no longer hashes is a loud integrity failure).

use std::path::Path;
use std::sync::Mutex;

use mwydyn_data::Value;
use rusqlite::{params, Connection};

use crate::blob::{BlobAccess, BlobFace, BlobRef};
use crate::journal::JournalError;
use crate::sqlite_journal::sql_err;


pub struct SQLiteBlobStore {
    conn: Mutex<Connection>,
}

impl SQLiteBlobStore {
    pub fn open(path: &Path) -> Result<SQLiteBlobStore, JournalError> {
        let conn = Connection::open(path).map_err(sql_err)?;
        Self::init(conn)
    }

    pub fn open_memory() -> Result<SQLiteBlobStore, JournalError> {
        let conn = Connection::open_in_memory().map_err(sql_err)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<SQLiteBlobStore, JournalError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blobs (
                digest     TEXT PRIMARY KEY,
                media_type TEXT NOT NULL,
                bytes      BLOB NOT NULL,
                ttl_micros INTEGER
            );",
        )
        .map_err(sql_err)?;
        Ok(SQLiteBlobStore {
            conn: Mutex::new(conn),
        })
    }
}

impl BlobFace for SQLiteBlobStore {
    fn put(
        &self,
        bytes: &[u8],
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, &'static str> {
        let digest = Value::blob_digest(bytes, media_type).map_err(|_| "bad media type")?;
        let conn = self.conn.lock().expect("blob store mutex poisoned");
        conn.execute(
            "INSERT INTO blobs (digest, media_type, bytes, ttl_micros)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(digest) DO NOTHING", // idempotent by content (§4)
            params![digest, media_type, bytes, ttl_micros],
        )
        .map_err(|_| "sqlite write failed")?;
        Ok(BlobRef {
            digest,
            media_type: media_type.to_string(),
            size: bytes.len() as u64,
        })
    }

    fn get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess> {
        let conn = self.conn.lock().expect("blob store mutex poisoned");
        let row: (String, Vec<u8>) = conn
            .query_row(
                "SELECT media_type, bytes FROM blobs WHERE digest=?1",
                params![r.digest],
                |row| {
                    let ty: String = row.get(0)?;
                    let bytes: Vec<u8> = row.get(1)?;
                    Ok((ty, bytes))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => BlobAccess::Missing,
                _ => BlobAccess::Integrity,
            })?;
        let (ty, bytes) = row;
        // verify-on-read: (type, bytes) must hash to the ref's digest
        let recomputed = Value::blob_digest(&bytes, &ty).map_err(|_| BlobAccess::Integrity)?;
        if recomputed != r.digest || ty != r.media_type || bytes.len() as u64 != r.size {
            return Err(BlobAccess::Integrity);
        }
        Ok(bytes)
    }

    fn len(&self, r: &BlobRef) -> Result<u64, BlobAccess> {
        self.get(r).map(|b| b.len() as u64)
    }

    fn retain(&self, keep: &std::collections::HashSet<String>) -> usize {
        let conn = self.conn.lock().expect("blob store mutex poisoned");
        let all: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT digest FROM blobs")
                .expect("blob store mutex poisoned");
            stmt.query_map([], |r| r.get(0))
                .expect("blob store mutex poisoned")
                .filter_map(|x| x.ok())
                .collect()
        };
        let mut n = 0;
        for digest in all
            .iter()
            .filter(|d| !keep.contains(*d))
        {
            if conn
                .execute("DELETE FROM blobs WHERE digest=?1", params![digest])
                .is_ok()
            {
                n += 1;
            }
        }
        n
    }

    fn count(&self) -> usize {
        let conn = self.conn.lock().expect("blob store mutex poisoned");
        conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
            .unwrap_or(0)
    }
}
