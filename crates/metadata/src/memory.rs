//! In-memory metadata store (dev / testing).

#![allow(clippy::default_trait_access)]

use async_trait::async_trait;
use mcp_oxide_core::{
    adapter::Adapter,
    providers::{Filter, MetadataStore},
    tool::Tool,
    Result,
};
use parking_lot::RwLock;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct InMemoryMetadataStore {
    adapters: RwLock<BTreeMap<String, Adapter>>,
    tools: RwLock<BTreeMap<String, Tool>>,
}

impl InMemoryMetadataStore {
    pub fn new() -> Self {
        Self::default()
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
        self.adapters.write().insert(a.name.clone(), a.clone());
        Ok(())
    }
    async fn get_adapter(&self, name: &str) -> Result<Option<Adapter>> {
        Ok(self.adapters.read().get(name).cloned())
    }
    async fn list_adapters(&self, filter: &Filter) -> Result<Vec<Adapter>> {
        Ok(self
            .adapters
            .read()
            .values()
            .filter(|a| matches_filter(*a, filter, |a| &a.tags))
            .cloned()
            .collect())
    }
    async fn delete_adapter(&self, name: &str) -> Result<()> {
        self.adapters.write().remove(name);
        Ok(())
    }

    async fn put_tool(&self, t: &Tool) -> Result<()> {
        self.tools.write().insert(t.name.clone(), t.clone());
        Ok(())
    }
    async fn get_tool(&self, name: &str) -> Result<Option<Tool>> {
        Ok(self.tools.read().get(name).cloned())
    }
    async fn list_tools(&self, filter: &Filter) -> Result<Vec<Tool>> {
        Ok(self
            .tools
            .read()
            .values()
            .filter(|t| matches_filter(*t, filter, |t| &t.tags))
            .cloned()
            .collect())
    }
    async fn delete_tool(&self, name: &str) -> Result<()> {
        self.tools.write().remove(name);
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

    fn sample(name: &str) -> Adapter {
        Adapter {
            name: name.into(),
            description: None,
            image: ImageRef {
                reference: "example:1".into(),
            },
            endpoint: Endpoint {
                port: 8080,
                path: "/mcp".into(),
            },
            replicas: 1,
            env: vec![],
            secret_refs: vec![],
            required_roles: vec![],
            tags: vec!["t".into()],
            resources: Default::default(),
            health: None,
            session_affinity: Default::default(),
            labels: Default::default(),
        }
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let store = InMemoryMetadataStore::new();
        let a = sample("x");
        store.put_adapter(&a).await.unwrap();
        assert_eq!(store.get_adapter("x").await.unwrap().unwrap().name, "x");
        assert_eq!(
            store.list_adapters(&Filter::default()).await.unwrap().len(),
            1
        );
        store.delete_adapter("x").await.unwrap();
        assert!(store.get_adapter("x").await.unwrap().is_none());
    }
}
