//! In-memory metadata store (dev / testing).

#![allow(clippy::default_trait_access)]

use async_trait::async_trait;
use mcp_oxide_core::{
    adapter::Adapter,
    providers::{Filter, MetadataStore},
    tool::Tool,
    Error, Result,
};
use parking_lot::RwLock;
use std::collections::BTreeMap;

/// Composite key `(tenant, name)`. `None` for tenant means "global" (e.g. a
/// static config-loaded adapter that every tenant sees). The store does NOT
/// merge tenants on lookup — a `(Some("acme"), "weather")` row is invisible
/// to a caller passing `tenant = Some("other")`.
type Key = (Option<String>, String);

#[derive(Debug, Default)]
pub struct InMemoryMetadataStore {
    adapters: RwLock<BTreeMap<Key, Adapter>>,
    tools: RwLock<BTreeMap<Key, Tool>>,
}

impl InMemoryMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn key(tenant: Option<&str>, name: &str) -> Key {
    (tenant.map(ToString::to_string), name.to_string())
}

fn tenant_match(filter: Option<&str>, item: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(t) => item == Some(t),
    }
}

fn matches_filter<T, F: Fn(&T) -> &Vec<String>>(item: &T, filter: &Filter, tags: F) -> bool {
    if filter.tags.is_empty() {
        return true;
    }
    let item_tags = tags(item);
    filter.tags.iter().all(|t| item_tags.contains(t))
}

#[async_trait]
impl MetadataStore for InMemoryMetadataStore {
    async fn put_adapter(&self, a: &Adapter) -> Result<()> {
        let k = key(a.tenant.as_deref(), &a.name);
        let mut g = self.adapters.write();
        if g.contains_key(&k) {
            return Err(Error::Conflict(format!(
                "adapter '{}' already exists in tenant {:?}",
                a.name, a.tenant
            )));
        }
        g.insert(k, a.clone());
        Ok(())
    }

    async fn update_adapter_cas(&self, a: &Adapter, expected_revision: u64) -> Result<()> {
        let k = key(a.tenant.as_deref(), &a.name);
        let mut g = self.adapters.write();
        let entry = g
            .get(&k)
            .ok_or_else(|| Error::NotFound(format!("adapter '{}'", a.name)))?;
        if entry.revision != Some(expected_revision) {
            return Err(Error::Conflict("revision mismatch".into()));
        }
        g.insert(k, a.clone());
        Ok(())
    }

    async fn get_adapter(&self, name: &str, tenant: Option<&str>) -> Result<Option<Adapter>> {
        Ok(self.adapters.read().get(&key(tenant, name)).cloned())
    }

    async fn list_adapters(&self, filter: &Filter) -> Result<Vec<Adapter>> {
        Ok(self
            .adapters
            .read()
            .values()
            .filter(|a| {
                tenant_match(filter.tenant.as_deref(), a.tenant.as_deref())
                    && matches_filter(*a, filter, |a| &a.tags)
            })
            .cloned()
            .collect())
    }

    async fn delete_adapter(&self, name: &str, tenant: Option<&str>) -> Result<()> {
        self.adapters.write().remove(&key(tenant, name));
        Ok(())
    }

    async fn put_tool(&self, t: &Tool) -> Result<()> {
        let k = key(t.tenant.as_deref(), &t.name);
        let mut g = self.tools.write();
        if g.contains_key(&k) {
            return Err(Error::Conflict(format!(
                "tool '{}' already exists in tenant {:?}",
                t.name, t.tenant
            )));
        }
        g.insert(k, t.clone());
        Ok(())
    }

    async fn update_tool_cas(&self, t: &Tool, expected_revision: u64) -> Result<()> {
        let k = key(t.tenant.as_deref(), &t.name);
        let mut g = self.tools.write();
        let entry = g
            .get(&k)
            .ok_or_else(|| Error::NotFound(format!("tool '{}'", t.name)))?;
        if entry.revision != Some(expected_revision) {
            return Err(Error::Conflict("revision mismatch".into()));
        }
        g.insert(k, t.clone());
        Ok(())
    }

    async fn get_tool(&self, name: &str, tenant: Option<&str>) -> Result<Option<Tool>> {
        Ok(self.tools.read().get(&key(tenant, name)).cloned())
    }

    async fn list_tools(&self, filter: &Filter) -> Result<Vec<Tool>> {
        Ok(self
            .tools
            .read()
            .values()
            .filter(|t| {
                tenant_match(filter.tenant.as_deref(), t.tenant.as_deref())
                    && matches_filter(*t, filter, |t| &t.tags)
            })
            .cloned()
            .collect())
    }

    async fn delete_tool(&self, name: &str, tenant: Option<&str>) -> Result<()> {
        self.tools.write().remove(&key(tenant, name));
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "in-memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_oxide_core::adapter::{Endpoint, ImageRef};

    fn sample(name: &str, tenant: Option<&str>) -> Adapter {
        Adapter {
            name: name.into(),
            tenant: tenant.map(ToString::to_string),
            description: None,
            image: ImageRef {
                reference: "example:1".into(),
            },
            endpoint: Endpoint {
                port: 8080,
                path: "/mcp".into(),
            },
            upstream: None,
            replicas: 1,
            env: vec![],
            secret_refs: vec![],
            required_roles: vec![],
            tags: vec!["t".into()],
            resources: Default::default(),
            health: None,
            session_affinity: Default::default(),
            labels: Default::default(),
            revision: Some(1),
            created_at: None,
            updated_at: None,
        }
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let store = InMemoryMetadataStore::new();
        let a = sample("x", None);
        store.put_adapter(&a).await.unwrap();
        assert_eq!(
            store.get_adapter("x", None).await.unwrap().unwrap().name,
            "x"
        );
        assert_eq!(
            store.list_adapters(&Filter::default()).await.unwrap().len(),
            1
        );
        store.delete_adapter("x", None).await.unwrap();
        assert!(store.get_adapter("x", None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cross_tenant_isolation() {
        let store = InMemoryMetadataStore::new();
        store.put_adapter(&sample("weather", Some("acme"))).await.unwrap();
        store.put_adapter(&sample("weather", Some("other"))).await.unwrap();

        // Same name, different tenants — both visible only to their owner.
        assert!(store.get_adapter("weather", Some("acme")).await.unwrap().is_some());
        assert!(store.get_adapter("weather", Some("other")).await.unwrap().is_some());
        // Cross-tenant read returns None.
        assert!(store.get_adapter("weather", Some("nope")).await.unwrap().is_none());

        // List filtered by tenant returns only that tenant's row.
        let f = Filter { tenant: Some("acme".into()), ..Default::default() };
        let rows = store.list_adapters(&f).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tenant.as_deref(), Some("acme"));
    }

    #[tokio::test]
    async fn cas_update_detects_concurrent_writer() {
        let store = InMemoryMetadataStore::new();
        store.put_adapter(&sample("x", None)).await.unwrap();

        let mut next = sample("x", None);
        next.revision = Some(2);
        store.update_adapter_cas(&next, 1).await.unwrap();

        // Stale expected revision → Conflict.
        let mut stale = sample("x", None);
        stale.revision = Some(3);
        assert!(matches!(
            store.update_adapter_cas(&stale, 1).await,
            Err(Error::Conflict(_))
        ));
    }
}
