//! Session stores.

#![deny(unsafe_code)]

use async_trait::async_trait;
use mcp_oxide_core::{
    providers::SessionStore,
    session::{BackendId, SessionId},
    Result,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Default cap on the number of resident `(session, adapter)` entries before
/// the store starts evicting the oldest. Keeps memory bounded under session-id
/// spam (each unique header value would otherwise leak a row forever).
const DEFAULT_MAX_ENTRIES: usize = 100_000;
/// Default sweep cadence. The sweeper walks the map once per interval and
/// drops entries whose `expires_at` is in the past, so memory tracks active
/// sessions even when no caller hits the affected entry.
const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(feature = "in-memory")]
#[derive(Debug)]
pub struct InMemorySessionStore {
    inner: RwLock<HashMap<(String, String), Entry>>,
    max_entries: usize,
}

#[cfg(feature = "in-memory")]
impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct Entry {
    backend: BackendId,
    expires_at: Instant,
}

#[cfg(feature = "in-memory")]
impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    /// Customize the eviction cap. Useful for tests that want to assert
    /// LRU-style overflow behaviour without inserting 100k rows.
    #[must_use]
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            max_entries: max_entries.max(1),
        }
    }

    /// Spawn a background task that periodically prunes expired entries.
    /// Idempotent: the caller is expected to invoke this exactly once at
    /// startup, holding an `Arc` to the store. The task lives until the
    /// last `Arc` reference is dropped.
    pub fn start_sweeper(self: &Arc<Self>) {
        Self::start_sweeper_with(self, DEFAULT_SWEEP_INTERVAL);
    }

    /// Variant used by tests to drive the sweeper at a tight cadence.
    pub fn start_sweeper_with(this: &Arc<Self>, interval: Duration) {
        let weak = Arc::downgrade(this);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await; // drop the immediate first tick
            loop {
                tick.tick().await;
                let Some(strong) = weak.upgrade() else { break };
                strong.sweep_expired();
            }
        });
    }

    /// Remove every entry whose TTL has elapsed. Exposed for tests.
    pub fn sweep_expired(&self) {
        let now = Instant::now();
        self.inner.write().retain(|_, e| e.expires_at > now);
    }

    /// Number of resident entries (resident, not necessarily live —
    /// `sweep_expired` reconciles the two).
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Whether any entry is resident.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// When the table reaches `max_entries`, drop the entry with the soonest
    /// `expires_at`. Cheap because we only scan when full; under healthy
    /// load this branch is never taken.
    fn evict_one_if_full(&self, guard: &mut HashMap<(String, String), Entry>) {
        if guard.len() < self.max_entries {
            return;
        }
        if let Some(victim) = guard
            .iter()
            .min_by_key(|(_, e)| e.expires_at)
            .map(|(k, _)| k.clone())
        {
            guard.remove(&victim);
        }
    }
}

#[cfg(feature = "in-memory")]
#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn resolve(&self, session_id: &SessionId, adapter: &str) -> Result<Option<BackendId>> {
        let key = (session_id.0.clone(), adapter.to_string());
        let now = Instant::now();
        let guard = self.inner.read();
        Ok(guard
            .get(&key)
            .filter(|e| e.expires_at > now)
            .map(|e| e.backend.clone()))
    }

    async fn bind(
        &self,
        session_id: &SessionId,
        adapter: &str,
        backend: BackendId,
        ttl: Duration,
    ) -> Result<()> {
        let key = (session_id.0.clone(), adapter.to_string());
        let entry = Entry {
            backend,
            expires_at: Instant::now() + ttl,
        };
        let mut guard = self.inner.write();
        // Reuse the existing slot if present — no need to evict for
        // re-binding the same session.
        if !guard.contains_key(&key) {
            self.evict_one_if_full(&mut guard);
        }
        guard.insert(key, entry);
        Ok(())
    }

    async fn drop_session(&self, session_id: &SessionId) -> Result<()> {
        self.inner
            .write()
            .retain(|(sid, _), _| sid != &session_id.0);
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "in-memory"
    }
}

#[cfg(all(test, feature = "in-memory"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sweeper_evicts_expired_entries() {
        let store = Arc::new(InMemorySessionStore::with_capacity(1024));
        InMemorySessionStore::start_sweeper_with(&store, Duration::from_millis(20));

        for i in 0..200u32 {
            store
                .bind(
                    &SessionId(format!("s{i}")),
                    "ad",
                    BackendId(format!("b{i}")),
                    Duration::from_millis(10),
                )
                .await
                .unwrap();
        }
        assert_eq!(store.len(), 200);

        // Wait long enough for at least two sweeper ticks past expiry.
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(store.len(), 0, "sweeper should have evicted everything");
    }

    #[tokio::test]
    async fn capacity_caps_resident_entries() {
        let store = InMemorySessionStore::with_capacity(8);
        for i in 0..32u32 {
            store
                .bind(
                    &SessionId(format!("s{i}")),
                    "ad",
                    BackendId(format!("b{i}")),
                    Duration::from_secs(60),
                )
                .await
                .unwrap();
        }
        // Exactly the cap, despite 32 unique session ids being inserted.
        assert_eq!(store.len(), 8);
    }
}
