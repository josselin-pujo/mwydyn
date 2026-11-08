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

pub struct RamBlobStore {
    entries: HashMap<String, (String, Vec<u8>, Option<u64>)>,
}

impl Default for RamBlobStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RamBlobStore {
    pub fn new() -> RamBlobStore {
        RamBlobStore {
            entries: HashMap::new(),
        }
    }

    /// Pure in its inputs; idempotent by content (§4) — replay
    /// re-puts to the identical digest. TTL is stored but sweep
    /// policy is M3 (GC above the live watermark).
    pub fn put(
        &mut self,
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
        self.entries.insert(digest, (media_type.to_string(), bytes.to_vec(), ttl_micros));
        Ok(r)
    }

    /// Verify-on-read: (type, bytes) must hash to the ref's digest.
    pub fn get(&self, r: &BlobRef) -> Result<Vec<u8>, BlobAccess> {
        let Some((ty, bytes, _)) = self.entries.get(&r.digest) else {
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
        self.entries.len()
    }
}
