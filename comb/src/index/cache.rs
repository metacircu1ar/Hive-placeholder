//! One bounded resident cache per serving process, never per client. Values are
//! immutable: a read snapshot can safely retain a value after its LRU eviction.
use super::Index;
use crate::embedding::{Lease, decode};
use crate::locks::{Health, Mutex};
use crate::{Error, Result, values::ContentHash};
use hashlink::LinkedHashMap;
use rusqlite::params;
use std::sync::Arc;

const WARM_VECTORS: &str = "SELECT v.input_hash,v.vector FROM (SELECT DISTINCT input_hash FROM \
    chunks) c JOIN vector_cache.vectors v ON v.input_hash=c.input_hash WHERE v.fingerprint=?1 \
    LIMIT ?2";
const CACHED_VECTOR: &str =
    "SELECT vector FROM vector_cache.vectors WHERE fingerprint=?1 AND input_hash=?2";

impl Index {
    pub(crate) fn warm_vectors(&self, cache: &Resident) -> Result<()> {
        let mut stmt = self.connection.prepare(WARM_VECTORS)?;
        let limit = i64::try_from(cache.capacity)
            .map_err(|cause| Error::cache_budget_overflow().caused_by(cause))?;
        let mut count = 0;
        for row in stmt.query_map(params![cache.lease.fingerprint, limit], |r| {
            Ok((r.get::<_, ContentHash>(0)?, r.get::<_, Vec<u8>>(1)?))
        })? {
            let (hash, bytes) = row?;
            cache.insert(hash, decode(&bytes, cache.lease.dimensions)?.into())?;
            count += 1;
        }
        tracing::info!(name: "comb.vector_cache_warmed", count = count);
        Ok(())
    }

    pub(super) fn cached_vector(&self, cache: &Resident, hash: &ContentHash) -> Result<Arc<[f32]>> {
        cache.vector(hash, || {
            // The caller's WAL snapshot chooses this immutable row. In particular an
            // old snapshot remains valid after another writer edits or deletes a page.
            let bytes: Vec<u8> = self.connection.query_row(
                CACHED_VECTOR,
                params![cache.lease.fingerprint, hash],
                |r| r.get(0),
            )?;
            Ok(decode(&bytes, cache.lease.dimensions)?.into())
        })
    }
}

pub(crate) struct Resident {
    lease: Lease,
    capacity: usize,
    entries: Mutex<LinkedHashMap<ContentHash, Arc<[f32]>>>,
    #[cfg(test)]
    pub misses: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    poison_next_read: std::sync::atomic::AtomicBool,
}
impl Resident {
    pub fn new(lease: Lease, budget: usize, health: Arc<Health>) -> Result<Self> {
        // Include conservative per-entry key/map/Arc overhead in the budget.
        let capacity = budget / (lease.dimensions * 4 + super::limits::CACHE_ENTRY_OVERHEAD_BYTES);
        if capacity == 0 {
            return Err(Error::cache_budget_too_small());
        }
        Ok(Self {
            lease,
            capacity,
            entries: Mutex::new(LinkedHashMap::new(), health),
            #[cfg(test)]
            misses: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            poison_next_read: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub fn matches(&self, lease: &Lease) -> bool {
        &self.lease == lease
    }
    pub fn insert(&self, hash: ContentHash, vector: Arc<[f32]>) -> Result<()> {
        let mut cache = self.entries.lock()?;
        // Existing content-addressed rows win, just like the SQLite cache.
        if cache.to_back(&hash).is_some() {
            return Ok(());
        }
        while cache.len() >= self.capacity {
            cache.pop_front();
        }
        cache.insert(hash, vector);
        Ok(())
    }
    fn vector(
        &self,
        hash: &ContentHash,
        load: impl FnOnce() -> Result<Arc<[f32]>>,
    ) -> Result<Arc<[f32]>> {
        #[cfg(test)]
        if self
            .poison_next_read
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = self.entries.lock().unwrap();
                panic!("resident cache poison fixture");
            }));
            assert!(failed.is_err());
        }
        let resident = self.entries.lock()?.to_back(hash).cloned();
        if let Some(vector) = resident {
            return Ok(vector);
        }
        // The LRU lock is released before loading from the caller's snapshot.
        #[cfg(test)]
        self.misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let vector = load()?;
        self.insert(hash.into(), vector.clone())?;
        Ok(vector)
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().expect("healthy test cache").len()
    }
    #[cfg(test)]
    pub fn poison_on_next_read(&self) {
        self.poison_next_read
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::{ActivationId, Fingerprint};
    use std::sync::atomic::Ordering;

    #[test]
    fn misses_load_without_holding_the_lru_lock_and_hits_skip_loading() -> Result<()> {
        let cache = Resident::new(
            Lease {
                fingerprint: Fingerprint::digest("space"),
                activation: ActivationId::generate(),
                dimensions: 2,
            },
            264,
            Arc::new(Health::default()),
        )?;
        let hash = ContentHash::digest("input");
        let vector: Arc<[f32]> = vec![1.0, 0.0].into();
        let loaded = cache.vector(&hash, || {
            assert!(cache.entries.is_unlocked());
            Ok(vector.clone())
        })?;
        assert!(Arc::ptr_eq(&loaded, &vector));
        assert_eq!(cache.misses.load(Ordering::Relaxed), 1);

        let resident = cache.vector(&hash, || panic!("a resident vector must skip loading"))?;
        assert!(Arc::ptr_eq(&resident, &vector));
        assert_eq!(cache.misses.load(Ordering::Relaxed), 1);
        Ok(())
    }
}
