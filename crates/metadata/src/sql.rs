//! SQL-backed metadata store (sqlite / postgres via sqlx).
//!
//! Uses JSON columns for payload to keep the schema simple while still
//! supporting tag + tenant filtering in SQL.

use async_trait::async_trait;
use mcp_oxide_core::{
    adapter::Adapter,
    providers::{DeploymentKind, DeploymentStatusRecord, Filter, MetadataStore},
    tool::Tool,
    Error, Result,
};
use sqlx::Pool;
use std::marker::PhantomData;

/// Sentinel for the "global" (no-tenant) row. SQLite/Postgres make NULL-safe
/// equality awkward, so we encode the absence of tenant as the empty string
/// inside the DB and translate at the boundary.
const TENANT_NONE: &str = "";

fn enc_tenant(t: Option<&str>) -> &str {
    t.unwrap_or(TENANT_NONE)
}

fn dec_tenant(s: &str) -> Option<String> {
    if s.is_empty() { None } else { Some(s.to_string()) }
}

/// Kind-tagged trait to let us parameterize over the SQL backend.
#[async_trait]
pub trait SqlBackend: Send + Sync + 'static {
    type Db: sqlx::Database;
    fn kind_str() -> &'static str;
    async fn init_schema(pool: &Pool<Self::Db>) -> Result<()>;
    async fn put_adapter(pool: &Pool<Self::Db>, a: &Adapter) -> Result<()>;
    async fn update_adapter_cas(
        pool: &Pool<Self::Db>,
        a: &Adapter,
        expected_revision: u64,
    ) -> Result<()>;
    async fn get_adapter(
        pool: &Pool<Self::Db>,
        name: &str,
        tenant: Option<&str>,
    ) -> Result<Option<Adapter>>;
    async fn list_adapters(pool: &Pool<Self::Db>, filter: &Filter) -> Result<Vec<Adapter>>;
    async fn delete_adapter(pool: &Pool<Self::Db>, name: &str, tenant: Option<&str>) -> Result<()>;
    async fn put_tool(pool: &Pool<Self::Db>, t: &Tool) -> Result<()>;
    async fn update_tool_cas(
        pool: &Pool<Self::Db>,
        t: &Tool,
        expected_revision: u64,
    ) -> Result<()>;
    async fn get_tool(
        pool: &Pool<Self::Db>,
        name: &str,
        tenant: Option<&str>,
    ) -> Result<Option<Tool>>;
    async fn list_tools(pool: &Pool<Self::Db>, filter: &Filter) -> Result<Vec<Tool>>;
    async fn delete_tool(pool: &Pool<Self::Db>, name: &str, tenant: Option<&str>) -> Result<()>;
    async fn record_status(
        pool: &Pool<Self::Db>,
        kind: DeploymentKind,
        name: &str,
        tenant: Option<&str>,
        record: &DeploymentStatusRecord,
    ) -> Result<()>;
    async fn get_status(
        pool: &Pool<Self::Db>,
        kind: DeploymentKind,
        name: &str,
        tenant: Option<&str>,
    ) -> Result<Option<DeploymentStatusRecord>>;
}

/// Generic SQL metadata store.
pub struct SqlMetadataStore<B: SqlBackend> {
    pool: Pool<B::Db>,
    _marker: PhantomData<B>,
}

impl<B: SqlBackend> std::fmt::Debug for SqlMetadataStore<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlMetadataStore")
            .field("kind", &B::kind_str())
            .finish_non_exhaustive()
    }
}

impl<B: SqlBackend> SqlMetadataStore<B> {
    pub async fn new(pool: Pool<B::Db>) -> Result<Self> {
        B::init_schema(&pool).await?;
        Ok(Self {
            pool,
            _marker: PhantomData,
        })
    }
}

#[async_trait]
impl<B: SqlBackend> MetadataStore for SqlMetadataStore<B> {
    async fn put_adapter(&self, a: &Adapter) -> Result<()> {
        B::put_adapter(&self.pool, a).await
    }
    async fn update_adapter_cas(&self, a: &Adapter, expected_revision: u64) -> Result<()> {
        B::update_adapter_cas(&self.pool, a, expected_revision).await
    }
    async fn get_adapter(&self, name: &str, tenant: Option<&str>) -> Result<Option<Adapter>> {
        B::get_adapter(&self.pool, name, tenant).await
    }
    async fn list_adapters(&self, filter: &Filter) -> Result<Vec<Adapter>> {
        B::list_adapters(&self.pool, filter).await
    }
    async fn delete_adapter(&self, name: &str, tenant: Option<&str>) -> Result<()> {
        B::delete_adapter(&self.pool, name, tenant).await
    }
    async fn put_tool(&self, t: &Tool) -> Result<()> {
        B::put_tool(&self.pool, t).await
    }
    async fn update_tool_cas(&self, t: &Tool, expected_revision: u64) -> Result<()> {
        B::update_tool_cas(&self.pool, t, expected_revision).await
    }
    async fn get_tool(&self, name: &str, tenant: Option<&str>) -> Result<Option<Tool>> {
        B::get_tool(&self.pool, name, tenant).await
    }
    async fn list_tools(&self, filter: &Filter) -> Result<Vec<Tool>> {
        B::list_tools(&self.pool, filter).await
    }
    async fn delete_tool(&self, name: &str, tenant: Option<&str>) -> Result<()> {
        B::delete_tool(&self.pool, name, tenant).await
    }
    async fn record_status(
        &self,
        kind: DeploymentKind,
        name: &str,
        tenant: Option<&str>,
        record: &DeploymentStatusRecord,
    ) -> Result<()> {
        B::record_status(&self.pool, kind, name, tenant, record).await
    }
    async fn get_status(
        &self,
        kind: DeploymentKind,
        name: &str,
        tenant: Option<&str>,
    ) -> Result<Option<DeploymentStatusRecord>> {
        B::get_status(&self.pool, kind, name, tenant).await
    }
    fn kind(&self) -> &'static str {
        B::kind_str()
    }
}

pub(crate) fn map_sqlx_err(e: &sqlx::Error) -> Error {
    Error::Internal(format!("sql: {e}"))
}

pub(crate) fn json_encode<T: serde::Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Internal(format!("json encode: {e}")))
}

pub(crate) fn json_decode<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(s).map_err(|e| Error::Internal(format!("json decode: {e}")))
}

// ---------------------------------------------------------------------------
// SQLite backend
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
pub mod sqlite {
    use super::{
        dec_tenant, enc_tenant, json_decode, json_encode, map_sqlx_err, SqlBackend, TENANT_NONE,
    };
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use mcp_oxide_core::{
        adapter::Adapter,
        providers::{DeploymentKind, DeploymentStatus, DeploymentStatusRecord, Filter},
        tool::Tool,
        Error, Result,
    };
    use sqlx::sqlite::{Sqlite, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::{ConnectOptions, Pool, Row};
    use std::str::FromStr;
    use std::time::Duration;

    #[derive(Debug)]
    pub struct Sqlite_;

    /// Composite primary key `(tenant, name)` so the same `name` can exist
    /// across tenants. `tenant=''` is the sentinel for "global / no tenant"
    /// — see `enc_tenant`/`dec_tenant`. Indexes on `tenant` keep list scans
    /// cheap as the row count grows.
    const SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS adapters (
    tenant    TEXT NOT NULL DEFAULT '',
    name      TEXT NOT NULL,
    tags      TEXT NOT NULL DEFAULT '[]',
    payload   TEXT NOT NULL,
    revision  INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (tenant, name)
);
CREATE INDEX IF NOT EXISTS idx_adapters_tenant ON adapters(tenant);
CREATE TABLE IF NOT EXISTS tools (
    tenant    TEXT NOT NULL DEFAULT '',
    name      TEXT NOT NULL,
    tags      TEXT NOT NULL DEFAULT '[]',
    payload   TEXT NOT NULL,
    revision  INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (tenant, name)
);
CREATE INDEX IF NOT EXISTS idx_tools_tenant ON tools(tenant);
CREATE TABLE IF NOT EXISTS deployment_status (
    kind            TEXT NOT NULL,
    tenant          TEXT NOT NULL DEFAULT '',
    name            TEXT NOT NULL,
    ready           INTEGER NOT NULL,
    replicas        INTEGER NOT NULL,
    ready_replicas  INTEGER NOT NULL,
    message         TEXT,
    observed_at     TEXT NOT NULL,
    PRIMARY KEY (kind, tenant, name)
);
";

    #[async_trait]
    impl SqlBackend for Sqlite_ {
        type Db = Sqlite;
        fn kind_str() -> &'static str {
            "sqlite"
        }

        async fn init_schema(pool: &Pool<Sqlite>) -> Result<()> {
            for stmt in SCHEMA_SQL.split(';') {
                let s = stmt.trim();
                if s.is_empty() {
                    continue;
                }
                sqlx::query(s).execute(pool).await.map_err(|e| map_sqlx_err(&e))?;
            }
            Ok(())
        }

        async fn put_adapter(pool: &Pool<Sqlite>, a: &Adapter) -> Result<()> {
            let payload = json_encode(a)?;
            let tags = json_encode(&a.tags)?;
            let updated = chrono::Utc::now().to_rfc3339();
            let revision = i64::try_from(a.revision.unwrap_or(1)).unwrap_or(1);
            let outcome = sqlx::query(
                "INSERT INTO adapters (tenant, name, tags, payload, revision, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(enc_tenant(a.tenant.as_deref()))
            .bind(&a.name)
            .bind(&tags)
            .bind(&payload)
            .bind(revision)
            .bind(&updated)
            .execute(pool)
            .await;
            match outcome {
                Ok(_) => Ok(()),
                Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("2067")
                    || e.code().as_deref() == Some("1555") =>
                {
                    // SQLite UNIQUE constraint violation codes (2067 / 1555).
                    Err(Error::Conflict(format!(
                        "adapter '{}' already exists in tenant {:?}",
                        a.name, a.tenant
                    )))
                }
                Err(e) => Err(map_sqlx_err(&e)),
            }
        }

        async fn update_adapter_cas(
            pool: &Pool<Sqlite>,
            a: &Adapter,
            expected_revision: u64,
        ) -> Result<()> {
            let payload = json_encode(a)?;
            let tags = json_encode(&a.tags)?;
            let updated = chrono::Utc::now().to_rfc3339();
            let new_rev = i64::try_from(a.revision.unwrap_or(1)).unwrap_or(1);
            let expected = i64::try_from(expected_revision).unwrap_or(1);
            let res = sqlx::query(
                "UPDATE adapters SET tags = ?1, payload = ?2, revision = ?3, updated_at = ?4 \
                 WHERE tenant = ?5 AND name = ?6 AND revision = ?7",
            )
            .bind(&tags)
            .bind(&payload)
            .bind(new_rev)
            .bind(&updated)
            .bind(enc_tenant(a.tenant.as_deref()))
            .bind(&a.name)
            .bind(expected)
            .execute(pool)
            .await
            .map_err(|e| map_sqlx_err(&e))?;
            if res.rows_affected() == 0 {
                // Either the row is gone or the revision changed — both map
                // to Conflict. NotFound would require a separate read; we
                // skip it because the control plane already proved the row
                // existed before calling CAS.
                return Err(Error::Conflict("revision mismatch".into()));
            }
            Ok(())
        }

        async fn get_adapter(
            pool: &Pool<Sqlite>,
            name: &str,
            tenant: Option<&str>,
        ) -> Result<Option<Adapter>> {
            let row = sqlx::query("SELECT payload FROM adapters WHERE tenant = ?1 AND name = ?2")
                .bind(enc_tenant(tenant))
                .bind(name)
                .fetch_optional(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            match row {
                Some(r) => {
                    let s: String = r.try_get("payload").map_err(|e| map_sqlx_err(&e))?;
                    Ok(Some(json_decode(&s)?))
                }
                None => Ok(None),
            }
        }

        async fn list_adapters(pool: &Pool<Sqlite>, filter: &Filter) -> Result<Vec<Adapter>> {
            let rows = if let Some(t) = filter.tenant.as_deref() {
                sqlx::query(
                    "SELECT tenant, payload, tags FROM adapters WHERE tenant = ?1 ORDER BY name",
                )
                .bind(t)
                .fetch_all(pool)
                .await
            } else {
                sqlx::query("SELECT tenant, payload, tags FROM adapters ORDER BY tenant, name")
                    .fetch_all(pool)
                    .await
            }
            .map_err(|e| map_sqlx_err(&e))?;

            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let tags_s: String = r.try_get("tags").map_err(|e| map_sqlx_err(&e))?;
                let tags: Vec<String> = json_decode(&tags_s)?;
                if !filter.tags.is_empty() && !filter.tags.iter().all(|t| tags.contains(t)) {
                    continue;
                }
                let payload: String = r.try_get("payload").map_err(|e| map_sqlx_err(&e))?;
                let mut adapter: Adapter = json_decode(&payload)?;
                // Trust the row column over the JSON payload — keeps history
                // consistent if a row was written before tenant existed.
                let row_tenant: String = r.try_get("tenant").unwrap_or_default();
                adapter.tenant = dec_tenant(&row_tenant);
                let _ = TENANT_NONE; // keep the sentinel referenced
                out.push(adapter);
            }
            Ok(out)
        }

        async fn delete_adapter(
            pool: &Pool<Sqlite>,
            name: &str,
            tenant: Option<&str>,
        ) -> Result<()> {
            sqlx::query("DELETE FROM adapters WHERE tenant = ?1 AND name = ?2")
                .bind(enc_tenant(tenant))
                .bind(name)
                .execute(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            Ok(())
        }

        async fn put_tool(pool: &Pool<Sqlite>, t: &Tool) -> Result<()> {
            let payload = json_encode(t)?;
            let tags = json_encode(&t.tags)?;
            let updated = chrono::Utc::now().to_rfc3339();
            let revision = i64::try_from(t.revision.unwrap_or(1)).unwrap_or(1);
            let outcome = sqlx::query(
                "INSERT INTO tools (tenant, name, tags, payload, revision, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(enc_tenant(t.tenant.as_deref()))
            .bind(&t.name)
            .bind(&tags)
            .bind(&payload)
            .bind(revision)
            .bind(&updated)
            .execute(pool)
            .await;
            match outcome {
                Ok(_) => Ok(()),
                Err(sqlx::Error::Database(e))
                    if e.code().as_deref() == Some("2067")
                        || e.code().as_deref() == Some("1555") =>
                {
                    Err(Error::Conflict(format!(
                        "tool '{}' already exists in tenant {:?}",
                        t.name, t.tenant
                    )))
                }
                Err(e) => Err(map_sqlx_err(&e)),
            }
        }

        async fn update_tool_cas(
            pool: &Pool<Sqlite>,
            t: &Tool,
            expected_revision: u64,
        ) -> Result<()> {
            let payload = json_encode(t)?;
            let tags = json_encode(&t.tags)?;
            let updated = chrono::Utc::now().to_rfc3339();
            let new_rev = i64::try_from(t.revision.unwrap_or(1)).unwrap_or(1);
            let expected = i64::try_from(expected_revision).unwrap_or(1);
            let res = sqlx::query(
                "UPDATE tools SET tags = ?1, payload = ?2, revision = ?3, updated_at = ?4 \
                 WHERE tenant = ?5 AND name = ?6 AND revision = ?7",
            )
            .bind(&tags)
            .bind(&payload)
            .bind(new_rev)
            .bind(&updated)
            .bind(enc_tenant(t.tenant.as_deref()))
            .bind(&t.name)
            .bind(expected)
            .execute(pool)
            .await
            .map_err(|e| map_sqlx_err(&e))?;
            if res.rows_affected() == 0 {
                return Err(Error::Conflict("revision mismatch".into()));
            }
            Ok(())
        }

        async fn get_tool(
            pool: &Pool<Sqlite>,
            name: &str,
            tenant: Option<&str>,
        ) -> Result<Option<Tool>> {
            let row = sqlx::query("SELECT payload FROM tools WHERE tenant = ?1 AND name = ?2")
                .bind(enc_tenant(tenant))
                .bind(name)
                .fetch_optional(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            match row {
                Some(r) => {
                    let s: String = r.try_get("payload").map_err(|e| map_sqlx_err(&e))?;
                    Ok(Some(json_decode(&s)?))
                }
                None => Ok(None),
            }
        }

        async fn list_tools(pool: &Pool<Sqlite>, filter: &Filter) -> Result<Vec<Tool>> {
            let rows = if let Some(t) = filter.tenant.as_deref() {
                sqlx::query(
                    "SELECT tenant, payload, tags FROM tools WHERE tenant = ?1 ORDER BY name",
                )
                .bind(t)
                .fetch_all(pool)
                .await
            } else {
                sqlx::query("SELECT tenant, payload, tags FROM tools ORDER BY tenant, name")
                    .fetch_all(pool)
                    .await
            }
            .map_err(|e| map_sqlx_err(&e))?;

            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let tags_s: String = r.try_get("tags").map_err(|e| map_sqlx_err(&e))?;
                let tags: Vec<String> = json_decode(&tags_s)?;
                if !filter.tags.is_empty() && !filter.tags.iter().all(|t| tags.contains(t)) {
                    continue;
                }
                let payload: String = r.try_get("payload").map_err(|e| map_sqlx_err(&e))?;
                let mut tool: Tool = json_decode(&payload)?;
                let row_tenant: String = r.try_get("tenant").unwrap_or_default();
                tool.tenant = dec_tenant(&row_tenant);
                out.push(tool);
            }
            Ok(out)
        }

        async fn delete_tool(
            pool: &Pool<Sqlite>,
            name: &str,
            tenant: Option<&str>,
        ) -> Result<()> {
            sqlx::query("DELETE FROM tools WHERE tenant = ?1 AND name = ?2")
                .bind(enc_tenant(tenant))
                .bind(name)
                .execute(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            Ok(())
        }

        async fn record_status(
            pool: &Pool<Sqlite>,
            kind: DeploymentKind,
            name: &str,
            tenant: Option<&str>,
            record: &DeploymentStatusRecord,
        ) -> Result<()> {
            let observed = record.observed_at.to_rfc3339();
            sqlx::query(
                "INSERT INTO deployment_status \
                 (kind, tenant, name, ready, replicas, ready_replicas, message, observed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(kind, tenant, name) DO UPDATE SET \
                   ready          = excluded.ready, \
                   replicas       = excluded.replicas, \
                   ready_replicas = excluded.ready_replicas, \
                   message        = excluded.message, \
                   observed_at    = excluded.observed_at",
            )
            .bind(kind.as_str())
            .bind(enc_tenant(tenant))
            .bind(name)
            .bind(i64::from(record.status.ready))
            .bind(i64::from(record.status.replicas))
            .bind(i64::from(record.status.ready_replicas))
            .bind(record.status.message.as_deref())
            .bind(observed)
            .execute(pool)
            .await
            .map_err(|e| map_sqlx_err(&e))?;
            Ok(())
        }

        async fn get_status(
            pool: &Pool<Sqlite>,
            kind: DeploymentKind,
            name: &str,
            tenant: Option<&str>,
        ) -> Result<Option<DeploymentStatusRecord>> {
            let row = sqlx::query(
                "SELECT ready, replicas, ready_replicas, message, observed_at \
                 FROM deployment_status \
                 WHERE kind = ?1 AND tenant = ?2 AND name = ?3",
            )
            .bind(kind.as_str())
            .bind(enc_tenant(tenant))
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(|e| map_sqlx_err(&e))?;
            let _ = TENANT_NONE; // referenced for parity with adapter list path
            let Some(r) = row else { return Ok(None) };
            let ready: i64 = r.try_get("ready").map_err(|e| map_sqlx_err(&e))?;
            let replicas: i64 = r.try_get("replicas").map_err(|e| map_sqlx_err(&e))?;
            let ready_replicas: i64 = r.try_get("ready_replicas").map_err(|e| map_sqlx_err(&e))?;
            let message: Option<String> = r.try_get("message").map_err(|e| map_sqlx_err(&e))?;
            let observed: String = r.try_get("observed_at").map_err(|e| map_sqlx_err(&e))?;
            let observed_at = DateTime::parse_from_rfc3339(&observed)
                .map_err(|e| Error::Internal(format!("observed_at parse: {e}")))?
                .with_timezone(&Utc);
            Ok(Some(DeploymentStatusRecord {
                status: DeploymentStatus {
                    ready: ready != 0,
                    #[allow(clippy::cast_sign_loss)]
                    replicas: u32::try_from(replicas).unwrap_or(0),
                    #[allow(clippy::cast_sign_loss)]
                    ready_replicas: u32::try_from(ready_replicas).unwrap_or(0),
                    message,
                },
                observed_at,
            }))
        }
    }

    pub type SqliteMetadataStore = super::SqlMetadataStore<Sqlite_>;

    impl SqliteMetadataStore {
        /// Open a `SQLite` database with WAL + a 5s busy timeout. WAL gives
        /// readers a snapshot while a writer is in flight, which materially
        /// reduces "database is locked" errors under concurrent writes; the
        /// busy timeout absorbs the remaining contention.
        pub async fn connect(url: &str) -> Result<Self> {
            let opts = SqliteConnectOptions::from_str(url)
                .map_err(|e| map_sqlx_err(&e))?
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5))
                .foreign_keys(true)
                .log_statements(tracing::log::LevelFilter::Trace);
            let pool = SqlitePoolOptions::new()
                .max_connections(8)
                .connect_with(opts)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            Self::new(pool).await
        }
    }
}

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteMetadataStore;
