//! Server-side cache of top-K query results — `mcp-symbol-search` D12.
//!
//! Top-K paginated tools (`search_symbols`, `search_findings`) materialize
//! their full ranked window (up to `TOP_K_MATERIALIZE = 30` rows) on the
//! agent's first call. The materialized list is stashed here keyed by a
//! random `CacheId` that rides in the `TopK` cursor. Continuation calls
//! slice from the cache, paying no embedding or Lance cost.
//!
//! - **Bound:** LRU with `N = 64` entries. No TTL.
//! - **Eviction:** LRU push-out, plus `clear()` on snapshot rotation
//!   (called from the rotation hook in `state.rs`).
//! - **Cache miss / wrong snapshot:** surfaces as `STALE_CURSOR` per the
//!   pagination spec — the agent restarts the query.
//! - **Lock discipline:** the cache holds a `std::sync::Mutex<LruCache>`.
//!   Slices are cloned out under the lock and returned by value; the
//!   lock NEVER crosses an `.await`.

#![allow(
    clippy::expect_used,
    reason = "mutex poisoning is unrecoverable; expect() is the standard pattern"
)]
#![allow(
    clippy::cast_possible_truncation,
    reason = "all u32 casts are bounded by TOP_K_MATERIALIZE = 30; \
              page_size and offset are clamped well under u32::MAX"
)]
#![allow(
    clippy::indexing_slicing,
    reason = "the only slice (entry.rows[start..end]) is gated by an explicit \
              start.saturating_add(page_size).min(total) bound check"
)]

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use lru::LruCache;

use crate::cursor::{CacheId, SnapshotId};
use crate::error::{QueryError, QueryErrorCode};

/// Maximum number of in-flight top-K result sets per cache surface.
/// At ~30 rows × ~200 B/row = ~6 KB/entry → ~400 KB worst case. Trivial.
const CACHE_CAPACITY: usize = 64;

/// One materialized top-K result set held in the cache.
pub struct CachedTopK<T> {
    /// Defense-in-depth: rotation eviction is supposed to `clear()` the
    /// whole cache before any post-rotation query lands, but if a query
    /// and the rotation hook race, `slice()` can compare this against
    /// the current snapshot and emit `STALE_CURSOR` rather than serve
    /// stale rows.
    snapshot: SnapshotId,
    rows: Vec<T>,
}

/// LRU-bounded result cache parametric over the cached row type. Two
/// concrete instantiations are held by the host and lent to each query
/// through [`QueryCaches`](crate::QueryCaches):
/// `ResultCache<SearchHitRef>` (`search_symbols`) and
/// `ResultCache<RankedFindingView>` (`search_findings`).
pub struct ResultCache<T> {
    inner: Mutex<LruCache<CacheId, CachedTopK<T>>>,
}

impl<T: Clone> Default for ResultCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> ResultCache<T> {
    #[must_use]
    pub fn new() -> Self {
        let cap = NonZeroUsize::new(CACHE_CAPACITY).expect("CACHE_CAPACITY is non-zero");
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Insert a fresh result set, returning the minted `CacheId`. Caller
    /// is responsible for emitting a cursor that carries this id back.
    pub fn put(&self, snapshot: SnapshotId, rows: Vec<T>) -> CacheId {
        let id = mint_cache_id();
        let entry = CachedTopK { snapshot, rows };
        self.inner
            .lock()
            .expect("ResultCache mutex poisoned")
            .put(id, entry);
        id
    }

    /// Combined `put` + take the first `page_size` rows in a single lock
    /// acquisition. Returns the minted id alongside the first slice.
    /// Used by the first-call path in `search_symbols` / `search_findings`.
    pub fn put_and_take_first_page(
        &self,
        snapshot: SnapshotId,
        rows: Vec<T>,
        page_size: usize,
    ) -> (CacheId, Vec<T>) {
        let id = mint_cache_id();
        let first_page: Vec<T> = rows.iter().take(page_size).cloned().collect();
        let entry = CachedTopK { snapshot, rows };
        self.inner
            .lock()
            .expect("ResultCache mutex poisoned")
            .put(id, entry);
        (id, first_page)
    }

    /// Slice a continuation page out of the cache. Returns the cloned
    /// rows and the cached total length. Emits `STALE_CURSOR` if the
    /// `cache_id` is missing OR the cached snapshot doesn't match the
    /// current one (the race-window defense-in-depth path).
    pub fn slice(
        &self,
        cache_id: CacheId,
        offset: u32,
        page_size: usize,
        current_snapshot: SnapshotId,
    ) -> Result<(Vec<T>, usize), QueryError> {
        let mut guard = self.inner.lock().map_err(|e| {
            QueryError::new(
                QueryErrorCode::InternalError,
                format!("ResultCache mutex poisoned: {e}"),
            )
        })?;
        let entry = guard.get(&cache_id).ok_or_else(stale_cursor)?;
        if entry.snapshot != current_snapshot {
            return Err(stale_cursor());
        }
        let total = entry.rows.len();
        let start = offset as usize;
        let end = start.saturating_add(page_size).min(total);
        let page: Vec<T> = if start >= total {
            Vec::new()
        } else {
            entry.rows[start..end].to_vec()
        };
        Ok((page, total))
    }

    /// Drop every entry. Called by the snapshot-rotation hook so that
    /// pre-rotation cursors surface as `STALE_CURSOR` on continuation.
    pub fn clear(&self) {
        self.inner
            .lock()
            .expect("ResultCache mutex poisoned")
            .clear();
    }
}

/// Mint a fresh in-process unique `CacheId`. Uses a monotonic counter
/// in the lower 8 bytes; upper 8 bytes are zero (room for future
/// process-identifier or snapshot-tag encoding). Not cryptographically
/// random — `CacheId` has no security implication; it's an opaque
/// look-up key bounded by an LRU.
fn mint_cache_id() -> CacheId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&n.to_le_bytes());
    id
}

fn stale_cursor() -> QueryError {
    QueryError::new(
        QueryErrorCode::StaleCursor,
        "cursor: result-cache entry not found (snapshot rotated or LRU evicted)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::snapshot_id_from_timestamp;

    fn snap(s: &str) -> SnapshotId {
        snapshot_id_from_timestamp(s)
    }

    #[test]
    fn put_then_slice_round_trips() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s = snap("a");
        let id = cache.put(s, vec![1, 2, 3, 4, 5]);
        let (page, total) = cache.slice(id, 0, 2, s).unwrap();
        assert_eq!(page, vec![1, 2]);
        assert_eq!(total, 5);
        let (page, total) = cache.slice(id, 2, 2, s).unwrap();
        assert_eq!(page, vec![3, 4]);
        assert_eq!(total, 5);
        let (page, total) = cache.slice(id, 4, 2, s).unwrap();
        assert_eq!(page, vec![5]);
        assert_eq!(total, 5);
    }

    #[test]
    fn put_and_take_first_page_returns_first_slice() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s = snap("a");
        let (id, page) = cache.put_and_take_first_page(s, vec![10, 20, 30, 40], 3);
        assert_eq!(page, vec![10, 20, 30]);
        let (rest, total) = cache.slice(id, 3, 3, s).unwrap();
        assert_eq!(rest, vec![40]);
        assert_eq!(total, 4);
    }

    #[test]
    fn lru_eviction_at_bound() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s = snap("a");
        // The very first put is the LRU entry once the cache fills.
        let first = cache.put(s, vec![1]);
        // Fill the rest of capacity. No slice() in between — slice
        // bumps the entry to MRU and would invalidate the eviction
        // expectation.
        for _ in 1..CACHE_CAPACITY {
            let _ = cache.put(s, vec![2]);
        }
        // One more put → evicts the LRU, which is `first`.
        let _ = cache.put(s, vec![3]);
        let err = cache.slice(first, 0, 1, s).unwrap_err();
        assert_eq!(err.code, QueryErrorCode::StaleCursor);
    }

    #[test]
    fn slice_unknown_id_returns_stale_cursor() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s = snap("a");
        let err = cache.slice([0xFF; 16], 0, 1, s).unwrap_err();
        assert_eq!(err.code, QueryErrorCode::StaleCursor);
    }

    #[test]
    fn slice_wrong_snapshot_returns_stale_cursor() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s1 = snap("a");
        let s2 = snap("b");
        let id = cache.put(s1, vec![1, 2, 3]);
        let err = cache.slice(id, 0, 2, s2).unwrap_err();
        assert_eq!(err.code, QueryErrorCode::StaleCursor);
    }

    #[test]
    fn clear_drops_all() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s = snap("a");
        let id1 = cache.put(s, vec![1]);
        let id2 = cache.put(s, vec![2]);
        cache.clear();
        cache.slice(id1, 0, 1, s).unwrap_err();
        cache.slice(id2, 0, 1, s).unwrap_err();
    }

    #[test]
    fn slice_past_end_returns_empty() {
        let cache: ResultCache<u32> = ResultCache::new();
        let s = snap("a");
        let id = cache.put(s, vec![1, 2, 3]);
        let (page, total) = cache.slice(id, 10, 5, s).unwrap();
        assert!(page.is_empty());
        assert_eq!(total, 3);
    }
}
