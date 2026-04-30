//! SQL-backed metadata store (sqlite / postgres via sqlx).
//!
//! Uses JSON columns for payload to keep the schema simple while still
//! supporting tag + tenant filtering in SQL.

use async_trait::async_trait;
use mcp_oxide_core::{
    adapter::Adapter,
    providers::{Filter, MetadataStore},
    tool::Tool,
    Error, Result,
};
use sqlx::Pool;
use std::marker::PhantomData;

/// Kind-tagged trait to let us parameterize over the SQL backend.
#[async_trait]
pub trait SqlBackend: Send + Sync + 'static {
    type Db: sqlx::Database;
    fn kind_str() -> &'static str;
    async fn init_schema(pool: &Pool<Self::Db>) -> Result<()>;
    async fn put_adapter(pool: &Pool<Self::Db>, a: &Adapter) -> Result<()>;
    async fn get_adapter(pool: &Pool<Self::Db>, name: &str) -> Result<Option<Adapter>>;
    async fn list_adapters(pool: &Pool<Self::Db>, filter: &Filter) -> Result<Vec<Adapter>>;
    async fn delete_adapter(pool: &Pool<Self::Db>, name: &str) -> Result<()>;
    async fn put_tool(pool: &Pool<Self::Db>, t: &Tool) -> Result<()>;
    async fn get_tool(pool: &Pool<Self::Db>, name: &str) -> Result<Option<Tool>>;
    async fn list_tools(pool: &Pool<Self::Db>, filter: &Filter) -> Result<Vec<Tool>>;
    async fn delete_tool(pool: &Pool<Self::Db>, name: &str) -> Result<()>;
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
    async fn get_adapter(&self, name: &str) -> Result<Option<Adapter>> {
        B::get_adapter(&self.pool, name).await
    }
    async fn list_adapters(&self, filter: &Filter) -> Result<Vec<Adapter>> {
        B::list_adapters(&self.pool, filter).await
    }
    async fn delete_adapter(&self, name: &str) -> Result<()> {
        B::delete_adapter(&self.pool, name).await
    }
    async fn put_tool(&self, t: &Tool) -> Result<()> {
        B::put_tool(&self.pool, t).await
    }
    async fn get_tool(&self, name: &str) -> Result<Option<Tool>> {
        B::get_tool(&self.pool, name).await
    }
    async fn list_tools(&self, filter: &Filter) -> Result<Vec<Tool>> {
        B::list_tools(&self.pool, filter).await
    }
    async fn delete_tool(&self, name: &str) -> Result<()> {
        B::delete_tool(&self.pool, name).await
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
    use super::{json_decode, json_encode, map_sqlx_err, SqlBackend};
    use async_trait::async_trait;
    use mcp_oxide_core::{
        adapter::Adapter,
        providers::Filter,
        tool::Tool,
        Result,
    };
    use sqlx::sqlite::{Sqlite, SqlitePoolOptions};
    use sqlx::{Pool, Row};

    #[derive(Debug)]
    pub struct Sqlite_;

    const SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS adapters (
    name      TEXT PRIMARY KEY,
    tags      TEXT NOT NULL DEFAULT '[]',
    payload   TEXT NOT NULL,
    revision  INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tools (
    name      TEXT PRIMARY KEY,
    tags      TEXT NOT NULL DEFAULT '[]',
    payload   TEXT NOT NULL,
    revision  INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
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
            let rev = i64::try_from(a.revision.unwrap_or(1)).unwrap_or(1);
            sqlx::query(
                "INSERT INTO adapters (name, tags, payload, revision, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(name) DO UPDATE SET tags=excluded.tags, payload=excluded.payload, revision=excluded.revision, updated_at=excluded.updated_at",
            )
            .bind(&a.name)
            .bind(&tags)
            .bind(&payload)
            .bind(rev)
            .bind(&updated)
            .execute(pool)
            .await
            .map_err(|e| map_sqlx_err(&e))?;
            Ok(())
        }

        async fn get_adapter(pool: &Pool<Sqlite>, name: &str) -> Result<Option<Adapter>> {
            let row = sqlx::query("SELECT payload FROM adapters WHERE name = ?1")
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
            let rows = sqlx::query("SELECT payload, tags FROM adapters ORDER BY name")
                .fetch_all(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let tags_s: String = r.try_get("tags").map_err(|e| map_sqlx_err(&e))?;
                let tags: Vec<String> = json_decode(&tags_s)?;
                if !filter.tags.is_empty() && !filter.tags.iter().all(|t| tags.contains(t)) {
                    continue;
                }
                let payload: String = r.try_get("payload").map_err(|e| map_sqlx_err(&e))?;
                out.push(json_decode(&payload)?);
            }
            Ok(out)
        }

        async fn delete_adapter(pool: &Pool<Sqlite>, name: &str) -> Result<()> {
            sqlx::query("DELETE FROM adapters WHERE name = ?1")
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
            let rev = i64::try_from(t.revision.unwrap_or(1)).unwrap_or(1);
            sqlx::query(
                "INSERT INTO tools (name, tags, payload, revision, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(name) DO UPDATE SET tags=excluded.tags, payload=excluded.payload, revision=excluded.revision, updated_at=excluded.updated_at",
            )
            .bind(&t.name)
            .bind(&tags)
            .bind(&payload)
            .bind(rev)
            .bind(&updated)
            .execute(pool)
            .await
            .map_err(|e| map_sqlx_err(&e))?;
            Ok(())
        }

        async fn get_tool(pool: &Pool<Sqlite>, name: &str) -> Result<Option<Tool>> {
            let row = sqlx::query("SELECT payload FROM tools WHERE name = ?1")
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
            let rows = sqlx::query("SELECT payload, tags FROM tools ORDER BY name")
                .fetch_all(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let tags_s: String = r.try_get("tags").map_err(|e| map_sqlx_err(&e))?;
                let tags: Vec<String> = json_decode(&tags_s)?;
                if !filter.tags.is_empty() && !filter.tags.iter().all(|t| tags.contains(t)) {
                    continue;
                }
                let payload: String = r.try_get("payload").map_err(|e| map_sqlx_err(&e))?;
                out.push(json_decode(&payload)?);
            }
            Ok(out)
        }

        async fn delete_tool(pool: &Pool<Sqlite>, name: &str) -> Result<()> {
            sqlx::query("DELETE FROM tools WHERE name = ?1")
                .bind(name)
                .execute(pool)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            Ok(())
        }
    }

    pub type SqliteMetadataStore = super::SqlMetadataStore<Sqlite_>;

    impl SqliteMetadataStore {
        pub async fn connect(url: &str) -> Result<Self> {
            let pool = SqlitePoolOptions::new()
                .max_connections(8)
                .connect(url)
                .await
                .map_err(|e| map_sqlx_err(&e))?;
            Self::new(pool).await
        }
    }
}

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteMetadataStore;
