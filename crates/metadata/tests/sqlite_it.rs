//! `SQLite` `MetadataStore` integration tests.

#![cfg(feature = "sqlite")]

use mcp_oxide_core::{
    adapter::{Adapter, Endpoint, ImageRef},
    providers::{Filter, MetadataStore},
};
use mcp_oxide_metadata::SqliteMetadataStore;

fn sample_adapter(name: &str, tags: &[&str]) -> Adapter {
    Adapter {
        name: name.into(),
        description: Some("test".into()),
        image: ImageRef {
            reference: "example:1".into(),
        },
        endpoint: Endpoint {
            port: 8080,
            path: "/mcp".into(),
        },
        upstream: Some("http://example.local/mcp".into()),
        replicas: 1,
        env: vec![],
        secret_refs: vec![],
        required_roles: vec![],
        tags: tags.iter().map(|s| (*s).to_string()).collect(),
        resources: mcp_oxide_core::adapter::Resources::default(),
        health: None,
        session_affinity: mcp_oxide_core::adapter::SessionAffinity::default(),
        labels: std::collections::BTreeMap::default(),
        revision: Some(1),
        created_at: None,
        updated_at: None,
    }
}

#[tokio::test]
async fn sqlite_adapter_crud_roundtrip() {
    let store = SqliteMetadataStore::connect("sqlite::memory:").await.unwrap();

    let a = sample_adapter("demo", &["public"]);
    store.put_adapter(&a).await.unwrap();

    let got = store.get_adapter("demo").await.unwrap().unwrap();
    assert_eq!(got.name, "demo");
    assert_eq!(got.upstream.as_deref(), Some("http://example.local/mcp"));
    assert_eq!(got.revision, Some(1));

    // Update with bumped revision.
    let mut a2 = got.clone();
    a2.description = Some("updated".into());
    a2.revision = Some(2);
    store.put_adapter(&a2).await.unwrap();

    let got2 = store.get_adapter("demo").await.unwrap().unwrap();
    assert_eq!(got2.description.as_deref(), Some("updated"));
    assert_eq!(got2.revision, Some(2));

    // List.
    let all = store.list_adapters(&Filter::default()).await.unwrap();
    assert_eq!(all.len(), 1);

    // Delete.
    store.delete_adapter("demo").await.unwrap();
    assert!(store.get_adapter("demo").await.unwrap().is_none());
}

#[tokio::test]
async fn sqlite_list_filters_by_tag() {
    let store = SqliteMetadataStore::connect("sqlite::memory:").await.unwrap();

    store.put_adapter(&sample_adapter("a", &["public"])).await.unwrap();
    store.put_adapter(&sample_adapter("b", &["private", "mutating"])).await.unwrap();
    store.put_adapter(&sample_adapter("c", &["public", "mutating"])).await.unwrap();

    let public_only = store
        .list_adapters(&Filter {
            tenant: None,
            tags: vec!["public".into()],
        })
        .await
        .unwrap();
    assert_eq!(public_only.len(), 2);

    let mutating_only = store
        .list_adapters(&Filter {
            tenant: None,
            tags: vec!["mutating".into()],
        })
        .await
        .unwrap();
    assert_eq!(mutating_only.len(), 2);

    let both = store
        .list_adapters(&Filter {
            tenant: None,
            tags: vec!["public".into(), "mutating".into()],
        })
        .await
        .unwrap();
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].name, "c");
}

#[tokio::test]
async fn sqlite_persists_across_reconnect() {
    // File-backed sqlite: ensure a row survives closing + reopening the pool.
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "mcp-oxide-test-{}.db",
        uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());

    {
        let store = SqliteMetadataStore::connect(&url).await.unwrap();
        store
            .put_adapter(&sample_adapter("persist", &["t"]))
            .await
            .unwrap();
    }
    {
        let store = SqliteMetadataStore::connect(&url).await.unwrap();
        let got = store.get_adapter("persist").await.unwrap().unwrap();
        assert_eq!(got.name, "persist");
    }

    let _ = std::fs::remove_file(&path);
}
