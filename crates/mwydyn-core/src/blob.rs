//! The RAM blob store (spec §4) — the first BlobFace implementation.
//! Hardcoded, collocated, actor-by-interface reduced to its essence:
//! pure content addressing. `get` VERIFIES the hash-bound type on
//! every read — a re-typed ref fails loudly here (round 12).

use std::collections::HashMap;

use mwydyn_data::Value;

/// The core-level mirror of the world's `blob-ref` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    pub digest: String,
    pub media_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobAccess {
    /// No live `blob.read` row (§3.3 — the host face checks caps).
    Unauthorized,
    /// Loud-missing (§4): a missing blob is an integrity failure,
    /// never silent divergence.
    Missing,
    /// (type, bytes) no longer hashes to the ref's digest — the ref
    /// was re-typed in transit, or the entry is corrupt (round 12).
    Integrity,
}

/// One entry: (media type, bytes, ttl — sweep is M3).
type Entry = (String, Vec<u8>, Option<u64>);

pub struct RamBlobStore {
    /// Interior mutability with a Mutex (round 21,
    /// multithread-readiness): puts are EAGER ambient calls during
    /// receive (round 16), so the host face holds `&self`; when
    /// transitions move to workers, the store crosses threads. The
    /// lock is held only for map operations — no user code runs
    /// inside, so poisoning is practically unreachable.
    entries: std::sync::Mutex<HashMap<String, Entry>>,
}

impl Default for RamBlobStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RamBlobStore {
    pub fn new() -> RamBlobStore {
        RamBlobStore {
            entries: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Pure in its inputs; idempotent by content (§4) — replay
    /// re-puts to the identical digest. TTL is stored but sweep
    /// policy is M3 (GC above the live watermark). Non-observable:
    /// an unreferenced put is garbage, nothing else (round 16).
    pub fn put(
        &self,
        bytes: &[u8],
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, &'static str> {
        let digest = Value::blob_digest(bytes, media_type).map_err(|_| "bad media type")?;
        let r = BlobRef {
            digest: digest.clone(),
            media_type: media_type.to_string(),
            size: bytes.len() as u64,
        };
        self.entries
            .lock()
            .expect("blob store mutex poisoned")
            .insert(digest, (media_type.to_string(), bytes.to_vec(), ttl_micros));
        Ok(r)
    }

    /// Verify-on-read: (type, bytes) must hash to the ref's digest.
    pub fn get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess> {
        let entries = self.entries.lock().expect("blob store mutex poisoned");
        let Some((ty, bytes, _)) = entries.get(&r.digest) else {
            return Err(BlobAccess::Missing);
        };
        let recomputed = Value::blob_digest(bytes, ty).map_err(|_| BlobAccess::Integrity)?;
        if recomputed != r.digest || ty != &r.media_type || bytes.len() as u64 != r.size {
            return Err(BlobAccess::Integrity);
        }
        Ok(bytes.clone())
    }

    pub fn len(&self, r: &BlobRef) -> Option<u64> {
        self.get(r).ok().map(|b| b.len() as u64)
    }

    pub fn count(&self) -> usize {
        self.entries.lock().expect("blob store mutex poisoned").len()
    }
}

/// The blob face (§4): content-addressed allocation + verified reads.
/// Providers: RamBlobStore (RAM tier, tests) and SQLiteBlobStore
/// (durable, `sqlite` feature) — the substrate-independence pattern
/// from the journal (§8.5) applied to blobs.
pub trait BlobFace: Send + Sync {
    fn put(
        &self,
        bytes: &[u8],
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, &'static str>;
    fn get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess>;
    fn len(&self, r: &BlobRef) -> Result<u64, BlobAccess>;
    /// Reference-aware collection (round 16's law, mechanized):
    /// remove every blob whose digest is NOT in `keep`. Reachability
    /// = digests referenced by any journaled entry on any lineage
    /// (active or ghost — ghost reads must keep working, round 19)
    /// and, eventually, by durable actor state (§7.6). Returns the
    /// number of entries collected.
    fn retain(&self, keep: &std::collections::HashSet<String>) -> usize;
    fn count(&self) -> usize;
}

/// Boxed faces pass through — the context stores the provider boxed.
impl BlobFace for Box<dyn BlobFace> {
    fn put(
        &self,
        bytes: &[u8],
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, &'static str> {
        (**self).put(bytes, media_type, ttl_micros)
    }
    fn get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess> {
        (**self).get(r)
    }
    fn len(&self, r: &BlobRef) -> Result<u64, BlobAccess> {
        (**self).len(r)
    }
    fn retain(&self, keep: &std::collections::HashSet<String>) -> usize {
        (**self).retain(keep)
    }
    fn count(&self) -> usize {
        (**self).count()
    }
}

impl BlobFace for RamBlobStore {
    fn put(
        &self,
        bytes: &[u8],
        media_type: &str,
        ttl_micros: Option<u64>,
    ) -> Result<BlobRef, &'static str> {
        RamBlobStore::put(self, bytes, media_type, ttl_micros)
    }

    fn get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess> {
        RamBlobStore::get(self, r)
    }

    fn len(&self, r: &BlobRef) -> Result<u64, BlobAccess> {
        RamBlobStore::len(self, r).ok_or(BlobAccess::Missing)
    }

    fn retain(&self, keep: &std::collections::HashSet<String>) -> usize {
        let mut entries = self.entries.lock().expect("blob store mutex poisoned");
        let before = entries.len();
        entries.retain(|digest, _| keep.contains(digest));
        before - entries.len()
    }

    fn count(&self) -> usize {
        RamBlobStore::count(self)
    }
}
