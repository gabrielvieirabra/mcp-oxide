//! `SQLite` `MetadataStore` integration tests.

#![cfg(feature = "sqlite")]

use mcp_oxide_core::{
    adapter::{Adapter, Endpoint, ImageRef},
    providers::{
        DeploymentKind, DeploymentStatus, DeploymentStatusRecord, Filter, MetadataStore,
    },
    Error,
};
use mcp_oxide_metadata::SqliteMetadataStore;

fn sample_adapter(name: &str, tenant: Option<&str>, tags: &[&str]) -> Adapter {
    Adapter {
        name: name.into(),
        tenant: tenant.map(ToString::to_string),
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

    let a = sample_adapter("demo", None, &["public"]);
    store.put_adapter(&a).await.unwrap();

    let got = store.get_adapter("demo", None).await.unwrap().unwrap();
    assert_eq!(got.name, "demo");
    assert_eq!(got.upstream.as_deref(), Some("http://example.local/mcp"));
    assert_eq!(got.revision, Some(1));

    // Atomic update with the correct expected revision succeeds.
    let mut a2 = got.clone();
    a2.description = Some("updated".into());
    a2.revision = Some(2);
    store.update_adapter_cas(&a2, 1).await.unwrap();

    let got2 = store.get_adapter("demo", None).await.unwrap().unwrap();
    assert_eq!(got2.description.as_deref(), Some("updated"));
    assert_eq!(got2.revision, Some(2));

    // Stale CAS fails with Conflict.
    let mut a3 = got2.clone();
    a3.description = Some("racy".into());
    a3.revision = Some(3);
    assert!(matches!(
        store.update_adapter_cas(&a3, 1).await,
        Err(Error::Conflict(_))
    ));

    // List.
    let all = store.list_adapters(&Filter::default()).await.unwrap();
    assert_eq!(all.len(), 1);

    // Delete.
    store.delete_adapter("demo", None).await.unwrap();
    assert!(store.get_adapter("demo", None).await.unwrap().is_none());
}

#[tokio::test]
async fn sqlite_list_filters_by_tag() {
    let store = SqliteMetadataStore::connect("sqlite::memory:").await.unwrap();

    store.put_adapter(&sample_adapter("a", None, &["public"])).await.unwrap();
    store.put_adapter(&sample_adapter("b", None, &["private", "mutating"])).await.unwrap();
    store.put_adapter(&sample_adapter("c", None, &["public", "mutating"])).await.unwrap();

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
async fn sqlite_cross_tenant_isolation() {
    let store = SqliteMetadataStore::connect("sqlite::memory:").await.unwrap();

    // Same name in two tenants — both must coexist.
    store.put_adapter(&sample_adapter("weather", Some("acme"), &["public"])).await.unwrap();
    store.put_adapter(&sample_adapter("weather", Some("other"), &["public"])).await.unwrap();

    // Acme sees their adapter; "other" sees their own; a third tenant sees nothing.
    let acme = store.get_adapter("weather", Some("acme")).await.unwrap().unwrap();
    assert_eq!(acme.tenant.as_deref(), Some("acme"));
    let other = store.get_adapter("weather", Some("other")).await.unwrap().unwrap();
    assert_eq!(other.tenant.as_deref(), Some("other"));
    assert!(store.get_adapter("weather", Some("nope")).await.unwrap().is_none());

    // Tenant-scoped list returns only that tenant's row.
    let f = Filter { tenant: Some("acme".into()), tags: vec![] };
    let rows = store.list_adapters(&f).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tenant.as_deref(), Some("acme"));

    // Delete in one tenant doesn't affect the other.
    store.delete_adapter("weather", Some("acme")).await.unwrap();
    assert!(store.get_adapter("weather", Some("acme")).await.unwrap().is_none());
    assert!(store.get_adapter("weather", Some("other")).await.unwrap().is_some());
}

#[tokio::test]
async fn sqlite_concurrent_writers_no_locked_errors() {
    // WAL + busy_timeout should make many concurrent writers Just Work.
    let dir = std::env::temp_dir();
    let path = dir.join(format!("mcp-oxide-conc-{}.db", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let store = std::sync::Arc::new(SqliteMetadataStore::connect(&url).await.unwrap());

    let mut tasks = Vec::new();
    for i in 0..20 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store
                .put_adapter(&sample_adapter(&format!("a{i}"), None, &["t"]))
                .await
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }

    let all = store.list_adapters(&Filter::default()).await.unwrap();
    assert_eq!(all.len(), 20);

    let _ = std::fs::remove_file(&path);
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
            .put_adapter(&sample_adapter("persist", None, &["t"]))
            .await
            .unwrap();
    }
    {
        let store = SqliteMetadataStore::connect(&url).await.unwrap();
        let got = store.get_adapter("persist", None).await.unwrap().unwrap();
        assert_eq!(got.name, "persist");
    }

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn sqlite_deployment_status_roundtrip_and_tenant_scope() {
    let store = SqliteMetadataStore::connect("sqlite::memory:").await.unwrap();

    let now = chrono::Utc::now();
    let acme = DeploymentStatusRecord {
        status: DeploymentStatus {
            ready: true,
            replicas: 2,
            ready_replicas: 2,
            message: Some("acme-ok".into()),
        },
        observed_at: now,
    };
    let other = DeploymentStatusRecord {
        status: DeploymentStatus {
            ready: false,
            replicas: 1,
            ready_replicas: 0,
            message: Some("other-pending".into()),
        },
        observed_at: now,
    };

    store
        .record_status(DeploymentKind::Adapter, "weather", Some("acme"), &acme)
        .await
        .unwrap();
    store
        .record_status(DeploymentKind::Adapter, "weather", Some("other"), &other)
        .await
        .unwrap();

    // Tenant-scoped read returns only that tenant's record.
    let got = store
        .get_status(DeploymentKind::Adapter, "weather", Some("acme"))
        .await
        .unwrap()
        .unwrap();
    assert!(got.status.ready);
    assert_eq!(got.status.ready_replicas, 2);
    assert_eq!(got.status.message.as_deref(), Some("acme-ok"));

    let got = store
        .get_status(DeploymentKind::Adapter, "weather", Some("other"))
        .await
        .unwrap()
        .unwrap();
    assert!(!got.status.ready);
    assert_eq!(got.status.message.as_deref(), Some("other-pending"));

    // Different kind, same name + tenant: separate row.
    let tool_record = DeploymentStatusRecord {
        status: DeploymentStatus {
            ready: true,
            replicas: 1,
            ready_replicas: 1,
            message: None,
        },
        observed_at: now,
    };
    store
        .record_status(DeploymentKind::Tool, "weather", Some("acme"), &tool_record)
        .await
        .unwrap();
    assert_ne!(
        store
            .get_status(DeploymentKind::Adapter, "weather", Some("acme"))
            .await
            .unwrap()
            .unwrap()
            .status
            .message,
        store
            .get_status(DeploymentKind::Tool, "weather", Some("acme"))
            .await
            .unwrap()
            .unwrap()
            .status
            .message
    );

    // Idempotent overwrite.
    let mut later = acme.clone();
    later.status.ready_replicas = 3;
    store
        .record_status(DeploymentKind::Adapter, "weather", Some("acme"), &later)
        .await
        .unwrap();
    let got = store
        .get_status(DeploymentKind::Adapter, "weather", Some("acme"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status.ready_replicas, 3);

    // Missing row returns None.
    assert!(store
        .get_status(DeploymentKind::Adapter, "missing", Some("acme"))
        .await
        .unwrap()
        .is_none());
}

