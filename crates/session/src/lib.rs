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
use std::time::{Duration, Instant};

#[cfg(feature = "in-memory")]
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    inner: RwLock<HashMap<(String, String), Entry>>,
}

#[derive(Debug, Clone)]
struct Entry {
    backend: BackendId,
    expires_at: Instant,
}

#[cfg(feature = "in-memory")]
impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
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
        self.inner.write().insert(
            (session_id.0.clone(), adapter.to_string()),
            Entry {
                backend,
                expires_at: Instant::now() + ttl,
            },
        );
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
