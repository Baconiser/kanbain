//! `embeddings.sqlite` (E3) with sqlite-vec loaded: embedding spaces and
//! their vec0 tables (E4), sources and chunks with FTS5 (E5, E16), and the
//! index job queue (E18, E20).
//!
//! Queries here are built at runtime: sqlx's compile-time checks cannot see
//! the sqlite-vec module or the per-space vec tables.

use std::{path::Path, sync::Once};

use serde::Serialize;
use sqlx::{
    FromRow, Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use ts_rs::TS;

use crate::{chunk::text::Chunk, config::EmbedderConfig};

static REGISTER: Once = Once::new();

/// Source kind of code chunks; every other kind is text.
pub const KIND_CODE: &str = "code";

/// Registers sqlite-vec with every SQLite connection opened afterwards in
/// this process. The extension is compiled in (no DLL on Windows).
pub fn register_sqlite_vec() {
    REGISTER.call_once(|| unsafe {
        // SAFETY: sqlite3_vec_init has the signature sqlite3_auto_extension
        // expects; registration happens once, before connections open.
        #[allow(clippy::missing_transmute_annotations)]
        libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

pub async fn open_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    register_sqlite_vec();
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Text,
    Code,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Code => "code",
        }
    }

    pub fn of_kind(kind: &str) -> Self {
        if kind == KIND_CODE {
            Self::Code
        } else {
            Self::Text
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, TS)]
pub struct Space {
    #[ts(type = "number")]
    pub id: i64,
    pub purpose: String,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[ts(type = "number")]
    pub dimension: i64,
    pub query_prefix: String,
    pub doc_prefix: String,
    pub config_hash: String,
    /// `building`, `active` or `retired`.
    pub state: String,
    pub stale: bool,
    pub last_error: Option<String>,
    pub created_at: String,
    pub activated_at: Option<String>,
}

impl Space {
    pub fn vec_table(&self) -> String {
        vec_table(self.id)
    }
}

fn vec_table(space_id: i64) -> String {
    format!("vec_space_{space_id}")
}

/// Identifies a source. Unused parts are empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SourceKey {
    pub kind: String,
    pub scope: String,
    pub ref_id: String,
    pub repo_id: String,
    pub path: String,
}

impl SourceKey {
    /// A text source on `main` (tickets, comments, runs …).
    pub fn text(kind: &str, ref_id: &str) -> Self {
        Self {
            kind: kind.to_string(),
            scope: "main".to_string(),
            ref_id: ref_id.to_string(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceMeta {
    pub project_id: String,
    pub issue_id: String,
    pub blob_sha: Option<String>,
}

/// What [`Store::put_source`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceUpdate {
    pub source_id: i64,
    /// The content hash was unchanged; nothing was touched.
    pub unchanged: bool,
    /// Chunks added or whose text changed (they need embedding).
    pub changed_chunks: usize,
    pub removed_chunks: usize,
}

/// A chunk waiting for its vector in a space.
#[derive(Debug, Clone, FromRow)]
pub struct PendingChunk {
    pub chunk_id: i64,
    pub text: String,
    pub text_hash: String,
    pub project_id: String,
    pub kind: String,
    pub scope: String,
}

/// Restricts search candidates.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// `None` searches every project (D47).
    pub project_id: Option<String>,
    /// `None` allows every kind of the searched purpose.
    pub kinds: Option<Vec<String>>,
    /// `main` plus ticket overlays (D45).
    pub scopes: Vec<String>,
}

/// A chunk with its source, for building hits.
#[derive(Debug, Clone, FromRow)]
pub struct ChunkRow {
    pub chunk_id: i64,
    pub kind: String,
    pub project_id: String,
    pub issue_id: String,
    pub scope: String,
    pub ref_id: String,
    pub repo_id: String,
    pub path: String,
    pub text: String,
    pub symbol: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
pub struct IndexJob {
    pub id: i64,
    pub source_kind: String,
    pub source_ref: String,
    pub attempts: i64,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
pub struct JobCounts {
    #[ts(type = "number")]
    pub queued: i64,
    #[ts(type = "number")]
    pub running: i64,
    #[ts(type = "number")]
    pub failed: i64,
}

fn vector_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Hash identifying an embedder configuration with its probed dimension.
pub fn config_hash(purpose: Purpose, config: &EmbedderConfig, dimension: usize) -> String {
    let key = serde_json::json!({
        "purpose": purpose.as_str(),
        "provider": config.provider,
        "base_url": config.base_url.trim().trim_end_matches('/'),
        "model": config.model.trim(),
        "dimension": dimension,
        "dimensions": config.dimensions,
        "distance": "cosine",
        "query_prefix": config.query_prefix,
        "document_prefix": config.document_prefix,
    });
    crate::chunk::text::hash_text(&key.to_string())
}

/// FTS5 query for free text: every term as a quoted phrase, OR-ed. Ids such
/// as `PROJ-123` become the phrase `proj 123`.
pub fn fts_query(text: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let terms: Vec<String> = text
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .replace('"', "")
        })
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        // Filler words match everything and drown the useful terms.
        .filter(|t| !STOPWORDS.contains(&t.to_lowercase().as_str()))
        .filter(|t| seen.insert(t.to_lowercase()))
        .take(32)
        .map(|t| format!("\"{t}\""))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// Common English words left out of keyword queries.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "can", "do", "does", "for", "from",
    "has", "have", "how", "i", "if", "in", "into", "is", "it", "its", "no", "not", "of", "on",
    "or", "our", "should", "so", "that", "the", "their", "them", "then", "there", "they", "this",
    "to", "up", "was", "we", "were", "what", "when", "where", "which", "who", "will", "with",
    "would", "you", "your",
];

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        let pool = open_pool(path).await?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---- spaces ----

    pub async fn spaces(&self) -> Result<Vec<Space>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM embedding_spaces ORDER BY id")
            .fetch_all(&self.pool)
            .await
    }

    pub async fn space_by_hash(&self, hash: &str) -> Result<Option<Space>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM embedding_spaces WHERE config_hash = ?")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn active_space(&self, purpose: Purpose) -> Result<Option<Space>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM embedding_spaces WHERE purpose = ? AND state = 'active' \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(purpose.as_str())
        .fetch_optional(&self.pool)
        .await
    }

    /// Spaces that hold vectors (building or active).
    pub async fn live_spaces(&self) -> Result<Vec<Space>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM embedding_spaces WHERE state != 'retired' ORDER BY id")
            .fetch_all(&self.pool)
            .await
    }

    /// A new building space for this configuration, with its vec table. A
    /// retired space with the same configuration is brought back.
    pub async fn create_space(
        &self,
        purpose: Purpose,
        config: &EmbedderConfig,
        dimension: usize,
    ) -> Result<Space, sqlx::Error> {
        let hash = config_hash(purpose, config, dimension);
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM embedding_spaces WHERE config_hash = ? AND state = 'retired'")
            .bind(&hash)
            .execute(&mut *tx)
            .await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO embedding_spaces (purpose, provider, base_url, model, dimension, \
             query_prefix, doc_prefix, config_hash, state) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'building') RETURNING id",
        )
        .bind(purpose.as_str())
        .bind(
            serde_json::to_value(config.provider)
                .unwrap()
                .as_str()
                .unwrap_or("openai"),
        )
        .bind(config.base_url.trim())
        .bind(config.model.trim())
        .bind(dimension as i64)
        .bind(&config.query_prefix)
        .bind(&config.document_prefix)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(&format!(
            "CREATE VIRTUAL TABLE {} USING vec0(chunk_id INTEGER PRIMARY KEY, \
             embedding float[{dimension}] distance_metric=cosine, \
             project_id TEXT PARTITION KEY, kind TEXT, scope TEXT)",
            vec_table(id)
        ))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.space_by_hash(&hash)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Makes a space the active one of its purpose; the previous active
    /// space is retired and its vectors dropped (blue/green, §2).
    pub async fn activate(&self, space_id: i64) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let purpose: String =
            sqlx::query_scalar("SELECT purpose FROM embedding_spaces WHERE id = ?")
                .bind(space_id)
                .fetch_one(&mut *tx)
                .await?;
        let old: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM embedding_spaces WHERE purpose = ? AND state = 'active' AND id != ?",
        )
        .bind(&purpose)
        .bind(space_id)
        .fetch_all(&mut *tx)
        .await?;
        for id in old {
            Self::retire_in(&mut tx, id).await?;
        }
        sqlx::query(
            "UPDATE embedding_spaces SET state = 'active', \
             activated_at = datetime('now', 'subsec') WHERE id = ?",
        )
        .bind(space_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    async fn retire_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        space_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&format!("DROP TABLE IF EXISTS {}", vec_table(space_id)))
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM chunk_embeddings WHERE space_id = ?")
            .bind(space_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE embedding_spaces SET state = 'retired' WHERE id = ?")
            .bind(space_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    pub async fn retire(&self, space_id: i64) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        Self::retire_in(&mut tx, space_id).await?;
        tx.commit().await
    }

    pub async fn set_space_error(
        &self,
        space_id: i64,
        error: Option<&str>,
        stale: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE embedding_spaces SET last_error = ?, stale = ? WHERE id = ?")
            .bind(error)
            .bind(stale)
            .bind(space_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Drops a space's vectors so everything is embedded again.
    pub async fn clear_space(&self, space: &Space) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!("DELETE FROM {}", space.vec_table()))
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM chunk_embeddings WHERE space_id = ?")
            .bind(space.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE embedding_spaces SET stale = 0, last_error = NULL WHERE id = ?")
            .bind(space.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    // ---- sources and chunks ----

    async fn drop_vectors(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        spaces: &[Space],
        chunk_id: i64,
    ) -> Result<(), sqlx::Error> {
        for space in spaces {
            sqlx::query(&format!(
                "DELETE FROM {} WHERE chunk_id = ?",
                space.vec_table()
            ))
            .bind(chunk_id)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query("DELETE FROM chunk_embeddings WHERE chunk_id = ?")
            .bind(chunk_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Stores a source's chunks. Unchanged chunks (same ordinal and text)
    /// keep their vectors; only changed ones need embedding again (E16).
    pub async fn put_source(
        &self,
        key: &SourceKey,
        meta: &SourceMeta,
        content_hash: &str,
        chunks: &[Chunk],
    ) -> Result<SourceUpdate, sqlx::Error> {
        let spaces = self.live_spaces().await?;
        let mut tx = self.pool.begin().await?;
        let existing: Option<(i64, String)> = sqlx::query_as(
            "SELECT id, content_hash FROM sources \
             WHERE kind = ? AND scope = ? AND ref_id = ? AND repo_id = ? AND path = ?",
        )
        .bind(&key.kind)
        .bind(&key.scope)
        .bind(&key.ref_id)
        .bind(&key.repo_id)
        .bind(&key.path)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, hash)) = &existing
            && hash == content_hash
        {
            return Ok(SourceUpdate {
                source_id: *id,
                unchanged: true,
                ..Default::default()
            });
        }
        let source_id = match existing {
            Some((id, _)) => {
                sqlx::query(
                    "UPDATE sources SET project_id = ?, issue_id = ?, blob_sha = ?, \
                     content_hash = ?, updated_at = datetime('now', 'subsec') WHERE id = ?",
                )
                .bind(&meta.project_id)
                .bind(&meta.issue_id)
                .bind(&meta.blob_sha)
                .bind(content_hash)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                id
            }
            None => {
                sqlx::query_scalar(
                    "INSERT INTO sources (kind, project_id, issue_id, scope, ref_id, repo_id, \
                     path, blob_sha, content_hash) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     RETURNING id",
                )
                .bind(&key.kind)
                .bind(&meta.project_id)
                .bind(&meta.issue_id)
                .bind(&key.scope)
                .bind(&key.ref_id)
                .bind(&key.repo_id)
                .bind(&key.path)
                .bind(&meta.blob_sha)
                .bind(content_hash)
                .fetch_one(&mut *tx)
                .await?
            }
        };

        let old: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT id, ordinal, text_hash FROM chunks WHERE source_id = ? ORDER BY ordinal",
        )
        .bind(source_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut update = SourceUpdate {
            source_id,
            ..Default::default()
        };
        for chunk in chunks {
            match old.iter().find(|(_, ordinal, _)| *ordinal == chunk.ordinal) {
                Some((id, _, hash)) if *hash == chunk.text_hash => {
                    sqlx::query(
                        "UPDATE chunks SET symbol = ?, language = ?, start_line = ?, \
                         end_line = ? WHERE id = ?",
                    )
                    .bind(&chunk.symbol)
                    .bind(&chunk.language)
                    .bind(chunk.start_line)
                    .bind(chunk.end_line)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                }
                Some((id, _, _)) => {
                    Self::drop_vectors(&mut tx, &spaces, *id).await?;
                    sqlx::query(
                        "UPDATE chunks SET text = ?, text_hash = ?, token_count = ?, symbol = ?, \
                         language = ?, start_line = ?, end_line = ? WHERE id = ?",
                    )
                    .bind(&chunk.text)
                    .bind(&chunk.text_hash)
                    .bind(chunk.token_count)
                    .bind(&chunk.symbol)
                    .bind(&chunk.language)
                    .bind(chunk.start_line)
                    .bind(chunk.end_line)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    update.changed_chunks += 1;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO chunks (source_id, ordinal, symbol, language, start_line, \
                         end_line, text, token_count, text_hash) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(source_id)
                    .bind(chunk.ordinal)
                    .bind(&chunk.symbol)
                    .bind(&chunk.language)
                    .bind(chunk.start_line)
                    .bind(chunk.end_line)
                    .bind(&chunk.text)
                    .bind(chunk.token_count)
                    .bind(&chunk.text_hash)
                    .execute(&mut *tx)
                    .await?;
                    update.changed_chunks += 1;
                }
            }
        }
        for (id, ordinal, _) in &old {
            if !chunks.iter().any(|c| c.ordinal == *ordinal) {
                Self::drop_vectors(&mut tx, &spaces, *id).await?;
                sqlx::query("DELETE FROM chunks WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                update.removed_chunks += 1;
            }
        }
        tx.commit().await?;
        Ok(update)
    }

    /// Deletes sources matching `condition` (a WHERE clause over `sources`)
    /// with their chunks and vectors.
    async fn delete_sources_where(&self, condition: &str, bind: &str) -> Result<u64, sqlx::Error> {
        let spaces = self.live_spaces().await?;
        let mut tx = self.pool.begin().await?;
        let chunk_ids: Vec<i64> = sqlx::query_scalar(&format!(
            "SELECT c.id FROM chunks c JOIN sources s ON s.id = c.source_id WHERE {condition}"
        ))
        .bind(bind)
        .fetch_all(&mut *tx)
        .await?;
        for id in chunk_ids {
            Self::drop_vectors(&mut tx, &spaces, id).await?;
        }
        let deleted = sqlx::query(&format!("DELETE FROM sources AS s WHERE {condition}"))
            .bind(bind)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn delete_source(&self, key: &SourceKey) -> Result<u64, sqlx::Error> {
        let spaces = self.live_spaces().await?;
        let mut tx = self.pool.begin().await?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM sources \
             WHERE kind = ? AND scope = ? AND ref_id = ? AND repo_id = ? AND path = ?",
        )
        .bind(&key.kind)
        .bind(&key.scope)
        .bind(&key.ref_id)
        .bind(&key.repo_id)
        .bind(&key.path)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(id) = id else {
            return Ok(0);
        };
        let chunk_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM chunks WHERE source_id = ?")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
        for chunk_id in chunk_ids {
            Self::drop_vectors(&mut tx, &spaces, chunk_id).await?;
        }
        sqlx::query("DELETE FROM sources WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(1)
    }

    /// Everything indexed for a deleted ticket.
    pub async fn delete_issue_sources(&self, issue_id: &str) -> Result<u64, sqlx::Error> {
        self.delete_sources_where("s.issue_id = ?", issue_id).await
    }

    /// Sources of one kind whose ref starts with `prefix` and that are not
    /// in `keep` (for example the attachments a ticket no longer has).
    pub async fn source_refs(
        &self,
        kind: &str,
        issue_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT ref_id FROM sources WHERE kind = ? AND issue_id = ?")
            .bind(kind)
            .bind(issue_id)
            .fetch_all(&self.pool)
            .await
    }

    /// Chunks of `space`'s purpose without a vector in it.
    pub async fn pending_chunks(
        &self,
        space: &Space,
        limit: i64,
        exclude_projects: &[String],
    ) -> Result<Vec<PendingChunk>, sqlx::Error> {
        let kind_clause = Self::purpose_clause(space, exclude_projects);
        let sql = format!(
            "SELECT c.id AS chunk_id, c.text, c.text_hash, s.project_id, s.kind, s.scope \
             FROM chunks c JOIN sources s ON s.id = c.source_id \
             LEFT JOIN chunk_embeddings e ON e.chunk_id = c.id AND e.space_id = ? \
             WHERE e.chunk_id IS NULL AND {kind_clause} \
             ORDER BY s.updated_at DESC, c.id LIMIT ?"
        );
        let mut query = sqlx::query_as(&sql).bind(space.id);
        for project in exclude_projects {
            query = query.bind(project);
        }
        query.bind(limit).fetch_all(&self.pool).await
    }

    /// Chunks of a space's purpose, minus excluded projects (`?` binds).
    fn purpose_clause(space: &Space, exclude_projects: &[String]) -> String {
        let mut clause = if space.purpose == "code" {
            "s.kind = 'code'".to_string()
        } else {
            "s.kind != 'code'".to_string()
        };
        if !exclude_projects.is_empty() {
            clause.push_str(&format!(
                " AND s.project_id NOT IN ({})",
                Self::in_list(exclude_projects.len())
            ));
        }
        clause
    }

    /// Stores vectors for chunks whose text is still what was embedded.
    pub async fn store_vectors(
        &self,
        space: &Space,
        vectors: &[(PendingChunk, Vec<f32>)],
    ) -> Result<usize, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let mut stored = 0;
        for (chunk, vector) in vectors {
            let current: Option<String> =
                sqlx::query_scalar("SELECT text_hash FROM chunks WHERE id = ?")
                    .bind(chunk.chunk_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if current.as_deref() != Some(chunk.text_hash.as_str()) {
                continue;
            }
            sqlx::query(&format!(
                "INSERT OR REPLACE INTO {} (chunk_id, embedding, project_id, kind, scope) \
                 VALUES (?, ?, ?, ?, ?)",
                space.vec_table()
            ))
            .bind(chunk.chunk_id)
            .bind(vector_blob(vector))
            .bind(&chunk.project_id)
            .bind(&chunk.kind)
            .bind(&chunk.scope)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT OR REPLACE INTO chunk_embeddings \
                 (chunk_id, space_id, model, dimension, text_hash) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(chunk.chunk_id)
            .bind(space.id)
            .bind(&space.model)
            .bind(space.dimension)
            .bind(&chunk.text_hash)
            .execute(&mut *tx)
            .await?;
            stored += 1;
        }
        tx.commit().await?;
        Ok(stored)
    }

    /// Chunks of a purpose (minus excluded projects), and how many have a
    /// vector in `space`.
    pub async fn coverage(
        &self,
        space: &Space,
        exclude_projects: &[String],
    ) -> Result<(i64, i64), sqlx::Error> {
        let clause = Self::purpose_clause(space, exclude_projects);
        let sql = format!(
            "SELECT COUNT(*) FROM chunks c JOIN sources s ON s.id = c.source_id WHERE {clause}"
        );
        let mut total = sqlx::query_scalar(&sql);
        for project in exclude_projects {
            total = total.bind(project);
        }
        let total: i64 = total.fetch_one(&self.pool).await?;
        let embedded: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM chunk_embeddings WHERE space_id = ?")
                .bind(space.id)
                .fetch_one(&self.pool)
                .await?;
        Ok((total, embedded.min(total)))
    }

    /// Code sources of a repo in one scope, as (path, content hash); see
    /// [`crate::chunk::code::content_hash`].
    pub async fn code_sources(
        &self,
        scope: &str,
        project_id: &str,
        repo_id: &str,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT path, content_hash FROM sources \
             WHERE kind = 'code' AND scope = ? AND ref_id = ? AND repo_id = ?",
        )
        .bind(scope)
        .bind(project_id)
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await
    }

    /// (repo id, path) of a ticket's overlay files (D45).
    pub async fn overlay_paths(
        &self,
        issue_id: &str,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as("SELECT repo_id, path FROM sources WHERE kind = 'code' AND scope = ?")
            .bind(issue_id)
            .fetch_all(&self.pool)
            .await
    }

    /// Every overlay scope with code sources.
    pub async fn overlay_scopes(&self) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT DISTINCT scope FROM sources WHERE kind = 'code' AND scope != 'main'",
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Symbols of the main-branch code chunks that overlap `ranges` of a
    /// file (E19).
    pub async fn symbols_at(
        &self,
        repo_id: &str,
        path: &str,
        ranges: &[(i64, i64)],
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(Option<String>, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT c.symbol, c.start_line, c.end_line FROM chunks c \
             JOIN sources s ON s.id = c.source_id \
             WHERE s.kind = 'code' AND s.scope = 'main' AND s.repo_id = ? AND s.path = ? \
             ORDER BY c.start_line",
        )
        .bind(repo_id)
        .bind(path)
        .fetch_all(&self.pool)
        .await?;
        let mut symbols = Vec::new();
        for (symbol, start, end) in rows {
            let (Some(symbol), Some(start), Some(end)) = (symbol, start, end) else {
                continue;
            };
            if ranges.iter().any(|(a, b)| *a <= end && *b >= start) && !symbols.contains(&symbol) {
                symbols.push(symbol);
            }
        }
        Ok(symbols)
    }

    pub async fn chunk_count(&self) -> Result<(i64, i64), sqlx::Error> {
        let row = sqlx::query("SELECT COUNT(*), (SELECT COUNT(*) FROM sources) FROM chunks")
            .fetch_one(&self.pool)
            .await?;
        Ok((row.get(0), row.get(1)))
    }

    // ---- search primitives ----

    fn in_list(n: usize) -> String {
        vec!["?"; n].join(", ")
    }

    /// Nearest chunks to `vector` in `space` (cosine distance, smaller is
    /// closer).
    pub async fn knn(
        &self,
        space: &Space,
        vector: &[f32],
        filter: &Filter,
        k: usize,
    ) -> Result<Vec<(i64, f64)>, sqlx::Error> {
        let mut sql = format!(
            "SELECT chunk_id, distance FROM {} WHERE embedding MATCH ? AND k = ?",
            space.vec_table()
        );
        if filter.project_id.is_some() {
            sql.push_str(" AND project_id = ?");
        }
        if let Some(kinds) = &filter.kinds {
            sql.push_str(&format!(" AND kind IN ({})", Self::in_list(kinds.len())));
        }
        if !filter.scopes.is_empty() {
            sql.push_str(&format!(
                " AND scope IN ({})",
                Self::in_list(filter.scopes.len())
            ));
        }
        sql.push_str(" ORDER BY distance");
        let mut query = sqlx::query_as::<_, (i64, f64)>(&sql)
            .bind(vector_blob(vector))
            .bind(k as i64);
        if let Some(project) = &filter.project_id {
            query = query.bind(project);
        }
        if let Some(kinds) = &filter.kinds {
            for kind in kinds {
                query = query.bind(kind);
            }
        }
        for scope in &filter.scopes {
            query = query.bind(scope);
        }
        query.fetch_all(&self.pool).await
    }

    /// Keyword matches (bm25; smaller is better) of chunks of `purpose`.
    pub async fn keyword(
        &self,
        query_text: &str,
        purpose: Purpose,
        filter: &Filter,
        k: usize,
    ) -> Result<Vec<(i64, f64)>, sqlx::Error> {
        let Some(fts) = fts_query(query_text) else {
            return Ok(vec![]);
        };
        let mut sql = String::from(
            "SELECT c.id, bm25(chunks_fts) AS score FROM chunks_fts \
             JOIN chunks c ON c.id = chunks_fts.rowid JOIN sources s ON s.id = c.source_id \
             WHERE chunks_fts MATCH ?",
        );
        sql.push_str(match purpose {
            Purpose::Code => " AND s.kind = 'code'",
            Purpose::Text => " AND s.kind != 'code'",
        });
        if filter.project_id.is_some() {
            sql.push_str(" AND s.project_id = ?");
        }
        if let Some(kinds) = &filter.kinds {
            sql.push_str(&format!(" AND s.kind IN ({})", Self::in_list(kinds.len())));
        }
        if !filter.scopes.is_empty() {
            sql.push_str(&format!(
                " AND s.scope IN ({})",
                Self::in_list(filter.scopes.len())
            ));
        }
        sql.push_str(" ORDER BY score LIMIT ?");
        let mut query = sqlx::query_as::<_, (i64, f64)>(&sql).bind(fts);
        if let Some(project) = &filter.project_id {
            query = query.bind(project);
        }
        if let Some(kinds) = &filter.kinds {
            for kind in kinds {
                query = query.bind(kind);
            }
        }
        for scope in &filter.scopes {
            query = query.bind(scope);
        }
        query.bind(k as i64).fetch_all(&self.pool).await
    }

    pub async fn chunk_rows(&self, ids: &[i64]) -> Result<Vec<ChunkRow>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let sql = format!(
            "SELECT c.id AS chunk_id, s.kind, s.project_id, s.issue_id, s.scope, s.ref_id, \
             s.repo_id, s.path, c.text, c.symbol, c.start_line, c.end_line \
             FROM chunks c JOIN sources s ON s.id = c.source_id WHERE c.id IN ({})",
            Self::in_list(ids.len())
        );
        let mut query = sqlx::query_as::<_, ChunkRow>(&sql);
        for id in ids {
            query = query.bind(id);
        }
        query.fetch_all(&self.pool).await
    }

    // ---- jobs ----

    /// Queues a source; a queued job for it is pushed back by `delay_ms`
    /// (debounce, E18).
    pub async fn enqueue(
        &self,
        kind: &str,
        reference: &str,
        delay_ms: i64,
    ) -> Result<(), sqlx::Error> {
        let modifier = format!("+{} seconds", delay_ms as f64 / 1000.0);
        sqlx::query(
            "INSERT INTO index_jobs (source_kind, source_ref, not_before) \
             VALUES (?, ?, datetime('now', 'subsec', ?)) \
             ON CONFLICT (source_kind, source_ref) DO UPDATE \
             SET state = CASE WHEN state = 'running' THEN state ELSE 'queued' END, \
                 attempts = 0, last_error = NULL, not_before = excluded.not_before",
        )
        .bind(kind)
        .bind(reference)
        .bind(modifier)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Takes due jobs and marks them running.
    pub async fn take_due(&self, limit: i64) -> Result<Vec<IndexJob>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let jobs: Vec<IndexJob> = sqlx::query_as(
            "SELECT id, source_kind, source_ref, attempts FROM index_jobs \
             WHERE state = 'queued' AND julianday(not_before) <= julianday('now') \
             ORDER BY julianday(not_before) LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        for job in &jobs {
            sqlx::query("UPDATE index_jobs SET state = 'running' WHERE id = ?")
                .bind(job.id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(jobs)
    }

    /// Done. A job queued again while it ran stays queued.
    pub async fn finish_job(&self, job: &IndexJob) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM index_jobs WHERE id = ? AND state = 'running'")
            .bind(job.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Retried with backoff, then left as failed after five attempts.
    pub async fn fail_job(&self, job: &IndexJob, error: &str) -> Result<(), sqlx::Error> {
        let attempts = job.attempts + 1;
        let state = if attempts >= 5 { "failed" } else { "queued" };
        let delay = format!("+{} seconds", 2i64.pow(attempts.min(10) as u32) * 5);
        sqlx::query(
            "UPDATE index_jobs SET state = ?, attempts = ?, last_error = ?, \
             not_before = datetime('now', 'subsec', ?) WHERE id = ?",
        )
        .bind(state)
        .bind(attempts)
        .bind(error)
        .bind(delay)
        .bind(job.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Jobs left running by a previous launch go back to the queue.
    pub async fn requeue_running(&self) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE index_jobs SET state = 'queued' WHERE state = 'running'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn job_counts(&self) -> Result<JobCounts, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(state = 'queued'), 0), COALESCE(SUM(state = 'running'), 0), \
             COALESCE(SUM(state = 'failed'), 0) FROM index_jobs",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(JobCounts {
            queued: row.get(0),
            running: row.get(1),
            failed: row.get(2),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_call(
        &self,
        space_id: Option<i64>,
        purpose: Purpose,
        kind: &str,
        inputs: usize,
        tokens: Option<u64>,
        latency_ms: u64,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO embedding_calls (space_id, purpose, kind, inputs, tokens, latency_ms, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(space_id)
        .bind(purpose.as_str())
        .bind(kind)
        .bind(inputs as i64)
        .bind(tokens.map(|t| t as i64))
        .bind(latency_ms as i64)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rand::Rng;

    use super::*;
    use crate::chunk::text::{chunk_text, hash_text};

    pub async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("embeddings.sqlite"))
            .await
            .unwrap();
        (dir, store)
    }

    fn config(model: &str) -> EmbedderConfig {
        EmbedderConfig {
            base_url: "http://localhost:8080".into(),
            model: model.into(),
            ..Default::default()
        }
    }

    /// sqlite-vec in sqlx's bundled SQLite: a filtered KNN over 1,000
    /// vectors returns the same neighbours as brute force (the 3c spike).
    #[tokio::test]
    async fn filtered_knn_matches_brute_force() {
        let (_dir, store) = store().await;
        let version: String = sqlx::query_scalar("SELECT vec_version()")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert!(version.starts_with("v0.1"), "{version}");
        let space = store
            .create_space(Purpose::Text, &config("m"), 8)
            .await
            .unwrap();
        let mut rng = rand::rng();
        let mut all = Vec::new();
        let mut pending = Vec::new();
        for i in 0..1000i64 {
            let v: Vec<f32> = (0..8).map(|_| rng.random_range(-1.0..1.0)).collect();
            let project = if i % 2 == 0 { "p1" } else { "p2" };
            let kind = if i % 3 == 0 { "comment" } else { "ticket" };
            let key = SourceKey::text(kind, &i.to_string());
            let chunks = chunk_text("H", &format!("text {i}"), 400, 0);
            let put = store
                .put_source(
                    &key,
                    &SourceMeta {
                        project_id: project.into(),
                        ..Default::default()
                    },
                    &hash_text(&i.to_string()),
                    &chunks,
                )
                .await
                .unwrap();
            let chunk_id: i64 = sqlx::query_scalar("SELECT id FROM chunks WHERE source_id = ?")
                .bind(put.source_id)
                .fetch_one(store.pool())
                .await
                .unwrap();
            pending.push((
                PendingChunk {
                    chunk_id,
                    text: chunks[0].text.clone(),
                    text_hash: chunks[0].text_hash.clone(),
                    project_id: project.into(),
                    kind: kind.into(),
                    scope: "main".into(),
                },
                v.clone(),
            ));
            all.push((chunk_id, project, kind, v));
        }
        assert_eq!(store.store_vectors(&space, &pending).await.unwrap(), 1000);
        assert_eq!(store.coverage(&space, &[]).await.unwrap(), (1000, 1000));

        let query: Vec<f32> = (0..8).map(|_| rng.random_range(-1.0..1.0)).collect();
        let filter = Filter {
            project_id: Some("p1".into()),
            kinds: Some(vec!["ticket".into(), "comment".into()]),
            scopes: vec!["main".into()],
        };
        let got: Vec<i64> = store
            .knn(&space, &query, &filter, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let cosine = |a: &[f32], b: &[f32]| {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            1.0 - dot / (na * nb)
        };
        let mut expected: Vec<(i64, f32)> = all
            .iter()
            .filter(|(_, p, _, _)| *p == "p1")
            .map(|(id, _, _, v)| (*id, cosine(v, &query)))
            .collect();
        expected.sort_by(|a, b| a.1.total_cmp(&b.1));
        let expected: Vec<i64> = expected.iter().take(10).map(|(id, _)| *id).collect();
        assert_eq!(got, expected);

        let comments = store
            .knn(
                &space,
                &query,
                &Filter {
                    project_id: Some("p2".into()),
                    kinds: Some(vec!["comment".into()]),
                    scopes: vec![],
                },
                5,
            )
            .await
            .unwrap();
        assert_eq!(comments.len(), 5);
        for (id, _) in comments {
            let (_, p, k, _) = all.iter().find(|(c, ..)| *c == id).unwrap();
            assert_eq!((*p, *k), ("p2", "comment"));
        }
    }

    #[tokio::test]
    async fn only_changed_chunks_need_embedding() {
        let (_dir, store) = store().await;
        let space = store
            .create_space(Purpose::Text, &config("m"), 4)
            .await
            .unwrap();
        let key = SourceKey::text("ticket", "issue-1");
        let meta = SourceMeta {
            project_id: "p".into(),
            issue_id: "issue-1".into(),
            ..Default::default()
        };
        let one = "one ".repeat(25);
        let first = "first ".repeat(10);
        let v1 = chunk_text("KAN-1", &format!("## A\n\n{one}\n\n## B\n\n{first}"), 40, 0);
        let put = store.put_source(&key, &meta, "h1", &v1).await.unwrap();
        assert_eq!((put.changed_chunks, put.removed_chunks), (2, 0));
        let pending = store.pending_chunks(&space, 10, &[]).await.unwrap();
        assert_eq!(pending.len(), 2);
        let vectors: Vec<_> = pending
            .into_iter()
            .map(|c| (c, vec![1.0, 0.0, 0.0, 0.0]))
            .collect();
        store.store_vectors(&space, &vectors).await.unwrap();
        assert!(
            store
                .pending_chunks(&space, 10, &[])
                .await
                .unwrap()
                .is_empty()
        );

        // The same content changes nothing.
        assert!(
            store
                .put_source(&key, &meta, "h1", &v1)
                .await
                .unwrap()
                .unchanged
        );
        // Editing the second section re-embeds only that chunk.
        let second = "second ".repeat(10);
        let v2 = chunk_text(
            "KAN-1",
            &format!("## A\n\n{one}\n\n## B\n\n{second}"),
            40,
            0,
        );
        let put = store.put_source(&key, &meta, "h2", &v2).await.unwrap();
        assert_eq!(put.changed_chunks, 1);
        let pending = store.pending_chunks(&space, 10, &[]).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].text.contains("second"));

        // Keywords are stemmed: "seconds" finds "second".
        assert_eq!(
            store
                .keyword("seconds", Purpose::Text, &Filter::default(), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        // Keywords find the new text, not the old.
        let filter = Filter {
            project_id: Some("p".into()),
            ..Default::default()
        };
        assert_eq!(
            store
                .keyword("second", Purpose::Text, &filter, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .keyword("first", Purpose::Text, &filter, 10)
                .await
                .unwrap()
                .is_empty()
        );

        store.delete_issue_sources("issue-1").await.unwrap();
        assert_eq!(store.chunk_count().await.unwrap(), (0, 0));
        let left: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}", space.vec_table()))
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn spaces_are_blue_green() {
        let (_dir, store) = store().await;
        let a = store
            .create_space(Purpose::Text, &config("a"), 4)
            .await
            .unwrap();
        store.activate(a.id).await.unwrap();
        let b = store
            .create_space(Purpose::Text, &config("b"), 8)
            .await
            .unwrap();
        assert_eq!(b.state, "building");
        assert_eq!(
            store.active_space(Purpose::Text).await.unwrap().unwrap().id,
            a.id
        );
        store.activate(b.id).await.unwrap();
        let spaces = store.spaces().await.unwrap();
        assert_eq!(spaces[0].state, "retired");
        assert_eq!(spaces[1].state, "active");
        // The retired space's vec table is gone.
        assert!(
            sqlx::query(&format!("SELECT * FROM {}", a.vec_table()))
                .fetch_all(store.pool())
                .await
                .is_err()
        );
        // Going back to `a` builds it again.
        let again = store
            .create_space(Purpose::Text, &config("a"), 4)
            .await
            .unwrap();
        assert_eq!(again.state, "building");
        assert_ne!(
            config_hash(Purpose::Text, &config("a"), 4),
            config_hash(Purpose::Code, &config("a"), 4)
        );
    }

    #[tokio::test]
    async fn jobs_debounce_and_retry() {
        let (_dir, store) = store().await;
        store.enqueue("issue", "x", 0).await.unwrap();
        store.enqueue("issue", "x", 0).await.unwrap();
        store.enqueue("issue", "later", 60_000).await.unwrap();
        let jobs = store.take_due(10).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].source_ref, "x");
        store.fail_job(&jobs[0], "boom").await.unwrap();
        let counts = store.job_counts().await.unwrap();
        assert_eq!((counts.queued, counts.running, counts.failed), (2, 0, 0));
        assert!(store.take_due(10).await.unwrap().is_empty());
        store.enqueue("issue", "x", 0).await.unwrap();
        let jobs = store.take_due(10).await.unwrap();
        store.finish_job(&jobs[0]).await.unwrap();
        assert_eq!(store.job_counts().await.unwrap().queued, 1);
    }

    #[test]
    fn fts_queries() {
        assert_eq!(fts_query("PROJ-123").as_deref(), Some("\"PROJ-123\""));
        assert_eq!(
            fts_query("reset \"password\" !!").as_deref(),
            Some("\"reset\" OR \"password\"")
        );
        assert_eq!(
            fts_query("Users cannot reset their password, users!").as_deref(),
            Some("\"Users\" OR \"cannot\" OR \"reset\" OR \"password\"")
        );
        assert_eq!(fts_query("the of"), None);
        assert_eq!(fts_query("  "), None);
    }
}
