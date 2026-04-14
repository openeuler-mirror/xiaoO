use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::embedding::EmbeddingProvider;
use crate::vector::{bytes_to_vec, cosine_similarity, hybrid_merge, vec_to_bytes};
use crate::{
    DurableMemory, DurableMemoryKind, DurableMemoryManifestEntry, MemoryError, MemoryResult,
};

use super::semantic_store::{ScoredMemory, SemanticMemoryStore, SemanticSearchQuery};
use super::DurableMemoryStore;

pub struct SqliteDurableMemoryStore {
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    db_path: PathBuf,
    embedder: Arc<dyn EmbeddingProvider>,
    vector_weight: f32,
    keyword_weight: f32,
    cache_max: usize,
}

impl SqliteDurableMemoryStore {
    pub fn new(
        db_path: impl AsRef<Path>,
        embedder: Arc<dyn EmbeddingProvider>,
        vector_weight: f32,
        keyword_weight: f32,
        cache_max: usize,
    ) -> MemoryResult<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError::Io(e))?;
        }

        let conn = Connection::open(&db_path).map_err(|e| MemoryError::Embedding {
            message: format!("failed to open SQLite database: {e}"),
        })?;

        Self::init_schema(&conn)?;
        Self::apply_pragmas(&conn)?;
        Self::check_embedding_model(&conn, embedder.as_ref())?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
            embedder,
            vector_weight,
            keyword_weight,
            cache_max,
        })
    }

    fn init_schema(conn: &Connection) -> MemoryResult<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memories (
                id          TEXT PRIMARY KEY,
                kind        TEXT NOT NULL,
                content     TEXT NOT NULL,
                source      TEXT NOT NULL,
                embedding   BLOB,
                updated_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memories_kind ON memories(kind);

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                id, content, content=memories, content_rowid=rowid
            );

            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, id, content)
                VALUES (new.rowid, new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, content)
                VALUES ('delete', old.rowid, old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, content)
                VALUES ('delete', old.rowid, old.id, old.content);
                INSERT INTO memories_fts(rowid, id, content)
                VALUES (new.rowid, new.id, new.content);
            END;

            CREATE TABLE IF NOT EXISTS embedding_cache (
                content_hash TEXT PRIMARY KEY,
                embedding    BLOB NOT NULL,
                created_at   TEXT NOT NULL,
                accessed_at  TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_cache_accessed ON embedding_cache(accessed_at);

            CREATE TABLE IF NOT EXISTS embedding_meta (
                id          INTEGER PRIMARY KEY CHECK (id = 1),
                model_name  TEXT NOT NULL,
                dimensions  INTEGER NOT NULL
            );
            ",
        )
        .map_err(|e| MemoryError::Embedding {
            message: format!("failed to init SQLite schema: {e}"),
        })?;
        Ok(())
    }

    /// Check if the current embedder matches the stored model metadata.
    /// If mismatch, NULL all embeddings and clear cache so reindex picks them up.
    fn check_embedding_model(
        conn: &Connection,
        embedder: &dyn EmbeddingProvider,
    ) -> MemoryResult<()> {
        let current_name = embedder.name();
        let current_dims = embedder.dimensions();

        // Noop embedder (dims=0) — skip tracking entirely
        if current_dims == 0 {
            return Ok(());
        }

        let stored: Option<(String, usize)> = conn
            .query_row(
                "SELECT model_name, dimensions FROM embedding_meta WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        match stored {
            Some((name, dims)) if name == current_name && dims == current_dims => {
                // Model matches — nothing to do
                Ok(())
            }
            Some((old_name, old_dims)) => {
                // Model changed — invalidate all embeddings
                eprintln!(
                    "embedding model changed: {old_name}({old_dims}d) -> {current_name}({current_dims}d), invalidating embeddings"
                );
                conn.execute("UPDATE memories SET embedding = NULL", [])
                    .map_err(|e| MemoryError::Embedding {
                        message: format!("failed to invalidate embeddings: {e}"),
                    })?;
                conn.execute("DELETE FROM embedding_cache", [])
                    .map_err(|e| MemoryError::Embedding {
                        message: format!("failed to clear embedding cache: {e}"),
                    })?;
                conn.execute(
                    "INSERT OR REPLACE INTO embedding_meta (id, model_name, dimensions) VALUES (1, ?1, ?2)",
                    rusqlite::params![current_name, current_dims],
                )
                .map_err(|e| MemoryError::Embedding {
                    message: format!("failed to update embedding meta: {e}"),
                })?;
                Ok(())
            }
            None => {
                // First time — record model info
                conn.execute(
                    "INSERT INTO embedding_meta (id, model_name, dimensions) VALUES (1, ?1, ?2)",
                    rusqlite::params![current_name, current_dims],
                )
                .map_err(|e| MemoryError::Embedding {
                    message: format!("failed to insert embedding meta: {e}"),
                })?;
                Ok(())
            }
        }
    }

    fn apply_pragmas(conn: &Connection) -> MemoryResult<()> {
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous  = NORMAL;
            PRAGMA mmap_size    = 8388608;
            PRAGMA cache_size   = -2000;
            PRAGMA temp_store   = MEMORY;
            ",
        )
        .map_err(|e| MemoryError::Embedding {
            message: format!("failed to apply PRAGMA: {e}"),
        })?;
        Ok(())
    }

    fn content_hash(text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        let result = hasher.finalize();
        hex::encode(&result[..8])
    }

    async fn get_or_compute_embedding(&self, text: &str) -> MemoryResult<Option<Vec<f32>>> {
        if self.embedder.dimensions() == 0 {
            return Ok(None);
        }

        let hash = Self::content_hash(text);
        let conn = self.conn.clone();
        let hash_clone = hash.clone();

        // Check cache
        let cached = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT embedding FROM embedding_cache WHERE content_hash = ?1",
                )
                .ok()?;
            let blob: Option<Vec<u8>> = stmt
                .query_row(rusqlite::params![hash_clone], |row| row.get(0))
                .ok();
            if blob.is_some() {
                let _ = conn.execute(
                    "UPDATE embedding_cache SET accessed_at = datetime('now') WHERE content_hash = ?1",
                    rusqlite::params![hash_clone],
                );
            }
            blob
        })
        .await
        .unwrap_or(None);

        if let Some(blob) = cached {
            return Ok(Some(bytes_to_vec(&blob)));
        }

        // Compute embedding
        let embedding = self.embedder.embed_one(text).await?;
        if embedding.is_empty() {
            return Ok(None);
        }

        // Store in cache + LRU eviction
        let conn = self.conn.clone();
        let blob = vec_to_bytes(&embedding);
        let cache_max = self.cache_max;
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, created_at, accessed_at)
                 VALUES (?1, ?2, datetime('now'), datetime('now'))",
                rusqlite::params![hash, blob],
            );
            // LRU eviction
            let _ = conn.execute(
                "DELETE FROM embedding_cache WHERE content_hash IN (
                    SELECT content_hash FROM embedding_cache
                    ORDER BY accessed_at ASC
                    LIMIT MAX(0, (SELECT COUNT(*) FROM embedding_cache) - ?1)
                )",
                rusqlite::params![cache_max as i64],
            );
        })
        .await
        .unwrap_or(());

        Ok(Some(embedding))
    }

    fn kind_to_str(kind: &DurableMemoryKind) -> &'static str {
        match kind {
            DurableMemoryKind::Preference => "preference",
            DurableMemoryKind::Constraint => "constraint",
            DurableMemoryKind::Fact => "fact",
            DurableMemoryKind::Procedure => "procedure",
        }
    }

    fn str_to_kind(s: &str) -> DurableMemoryKind {
        match s {
            "preference" => DurableMemoryKind::Preference,
            "constraint" => DurableMemoryKind::Constraint,
            "procedure" => DurableMemoryKind::Procedure,
            _ => DurableMemoryKind::Fact,
        }
    }

    fn fts5_search(conn: &Connection, query: &str, limit: usize) -> Vec<(String, f32)> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| {
                let escaped = w.replace('"', "\"\"");
                format!("\"{escaped}\"")
            })
            .collect();
        if words.is_empty() {
            return Vec::new();
        }
        let match_expr = words.join(" OR ");

        let mut stmt = match conn.prepare(
            "SELECT m.id, bm25(memories_fts) as score
             FROM memories_fts f
             JOIN memories m ON m.rowid = f.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: FTS5 query prepare failed (FTS5 may be unavailable): {e}");
                return Vec::new();
            }
        };

        let rows: Vec<(String, f32)> = stmt
            .query_map(rusqlite::params![match_expr, limit as i64], |row| {
                let id: String = row.get(0)?;
                let score: f64 = row.get(1)?;
                Ok((id, -score as f32)) // BM25 returns negative; negate for ranking
            })
            .ok()
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        rows
    }

    fn vector_search(
        conn: &Connection,
        query_embedding: &[f32],
        limit: usize,
        kind_filter: Option<&str>,
    ) -> Vec<(String, f32)> {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match kind_filter {
            Some(kind) => (
                "SELECT id, embedding FROM memories WHERE embedding IS NOT NULL AND kind = ?1",
                vec![Box::new(kind.to_string()) as Box<dyn rusqlite::ToSql>],
            ),
            None => (
                "SELECT id, embedding FROM memories WHERE embedding IS NOT NULL",
                vec![],
            ),
        };

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut rows: Vec<(String, f32)> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })
            .ok()
            .map(|iter| {
                iter.filter_map(|r| r.ok())
                    .map(|(id, blob): (String, Vec<u8>)| {
                        let vec = bytes_to_vec(&blob);
                        let score = cosine_similarity(query_embedding, &vec);
                        (id, score)
                    })
                    .filter(|(_, score)| *score > 0.0)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rows.truncate(limit);
        rows
    }

    fn load_memories_by_ids(conn: &Connection, ids: &[String]) -> Vec<DurableMemory> {
        if ids.is_empty() {
            return Vec::new();
        }
        let placeholders: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT id, kind, content, source, updated_at FROM memories WHERE id IN ({})",
            placeholders.join(",")
        );
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        stmt.query_map(params.as_slice(), |row| {
            let id: String = row.get(0)?;
            let kind_str: String = row.get(1)?;
            let content: String = row.get(2)?;
            let source: String = row.get(3)?;
            let updated_at: u64 = row.get(4)?;
            Ok(DurableMemory {
                memory_id: id,
                kind: Self::str_to_kind(&kind_str),
                content,
                source,
                updated_at,
            })
        })
        .ok()
        .map(|iter| iter.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }
}

#[async_trait]
impl DurableMemoryStore for SqliteDurableMemoryStore {
    async fn save_memory(&self, memory: &DurableMemory) -> std::io::Result<()> {
        let embedding = self
            .get_or_compute_embedding(&memory.content)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let conn = self.conn.clone();
        let id = memory.memory_id.clone();
        let kind = Self::kind_to_str(&memory.kind).to_string();
        let content = memory.content.clone();
        let source = memory.source.clone();
        let updated_at = memory.updated_at;
        let blob = embedding.map(|e| vec_to_bytes(&e));

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO memories (id, kind, content, source, embedding, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                     kind = excluded.kind,
                     content = excluded.content,
                     source = excluded.source,
                     embedding = excluded.embedding,
                     updated_at = excluded.updated_at",
                rusqlite::params![id, kind, content, source, blob, updated_at],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .map_err(|e| std::io::Error::other(e.to_string()))
    }

    async fn load_memory(&self, memory_id: &str) -> std::io::Result<DurableMemory> {
        let conn = self.conn.clone();
        let id = memory_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.query_row(
                "SELECT id, kind, content, source, updated_at FROM memories WHERE id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok(DurableMemory {
                        memory_id: row.get(0)?,
                        kind: SqliteDurableMemoryStore::str_to_kind(&row.get::<_, String>(1)?),
                        content: row.get(2)?,
                        source: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .map_err(|e| std::io::Error::other(e.to_string()))
        })
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
    }

    async fn list_memories(&self) -> std::io::Result<Vec<DurableMemoryManifestEntry>> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare("SELECT id, kind, updated_at FROM memories ORDER BY updated_at DESC")
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            let entries: Vec<DurableMemoryManifestEntry> = stmt
                .query_map([], |row| {
                    Ok(DurableMemoryManifestEntry {
                        memory_id: row.get(0)?,
                        kind: SqliteDurableMemoryStore::str_to_kind(&row.get::<_, String>(1)?),
                        updated_at: row.get(2)?,
                    })
                })
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(entries)
        })
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
    }

    async fn delete_memory(&self, memory_id: &str) -> std::io::Result<()> {
        let conn = self.conn.clone();
        let id = memory_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
    }

    async fn replace_all(
        &self,
        memories: &[DurableMemory],
    ) -> std::io::Result<Vec<DurableMemoryManifestEntry>> {
        // Compute embeddings first (async)
        let mut memory_embeddings = Vec::new();
        for memory in memories {
            let embedding = self
                .get_or_compute_embedding(&memory.content)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            memory_embeddings.push((memory.clone(), embedding));
        }

        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute("DELETE FROM memories", [])
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            for (memory, embedding) in &memory_embeddings {
                let blob = embedding.as_ref().map(|e| vec_to_bytes(e));
                conn.execute(
                    "INSERT INTO memories (id, kind, content, source, embedding, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        memory.memory_id,
                        SqliteDurableMemoryStore::kind_to_str(&memory.kind),
                        memory.content,
                        memory.source,
                        blob,
                        memory.updated_at,
                    ],
                )
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            }

            // Rebuild FTS5 after bulk delete+insert to ensure index consistency
            conn.execute_batch("INSERT INTO memories_fts(memories_fts) VALUES('rebuild')")
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            let mut stmt = conn
                .prepare("SELECT id, kind, updated_at FROM memories ORDER BY updated_at DESC")
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            let entries: Vec<DurableMemoryManifestEntry> = stmt
                .query_map([], |row| {
                    Ok(DurableMemoryManifestEntry {
                        memory_id: row.get(0)?,
                        kind: SqliteDurableMemoryStore::str_to_kind(&row.get::<_, String>(1)?),
                        updated_at: row.get(2)?,
                    })
                })
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(entries)
        })
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
    }
}

#[async_trait]
impl SemanticMemoryStore for SqliteDurableMemoryStore {
    async fn search(&self, query: &SemanticSearchQuery) -> MemoryResult<Vec<ScoredMemory>> {
        let query_embedding = self.get_or_compute_embedding(&query.query_text).await?;
        let conn = self.conn.clone();
        let query_text = query.query_text.clone();
        let limit = query.limit;
        let kind_filter = query.kind_filter.as_ref().map(Self::kind_to_str);
        let kind_str = kind_filter.map(|s| s.to_string());
        let vw = self.vector_weight;
        let kw = self.keyword_weight;

        let scored_ids = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let keyword_results = Self::fts5_search(&conn, &query_text, limit * 2);
            let vector_results = match &query_embedding {
                Some(qe) => Self::vector_search(&conn, qe, limit * 2, kind_str.as_deref()),
                None => Vec::new(),
            };
            hybrid_merge(&vector_results, &keyword_results, vw, kw, limit)
        })
        .await
        .map_err(|e| MemoryError::Embedding {
            message: format!("search task failed: {e}"),
        })?;

        let ids: Vec<String> = scored_ids.iter().map(|r| r.id.clone()).collect();
        let conn = self.conn.clone();
        let memories = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            Self::load_memories_by_ids(&conn, &ids)
        })
        .await
        .map_err(|e| MemoryError::Embedding {
            message: format!("load task failed: {e}"),
        })?;

        let mut results = Vec::new();
        for scored in &scored_ids {
            if let Some(memory) = memories.iter().find(|m| m.memory_id == scored.id) {
                results.push(ScoredMemory {
                    memory: memory.clone(),
                    score: scored.final_score as f64,
                });
            }
        }
        Ok(results)
    }

    async fn reindex(&self) -> MemoryResult<usize> {
        // Step 1: Rebuild FTS5
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute_batch("INSERT INTO memories_fts(memories_fts) VALUES('rebuild')")
                .map_err(|e| MemoryError::Embedding {
                    message: format!("FTS5 rebuild failed: {e}"),
                })
        })
        .await
        .map_err(|e| MemoryError::Embedding {
            message: format!("reindex task failed: {e}"),
        })??;

        // Step 2: Re-embed NULL embeddings
        let conn = self.conn.clone();
        let rows: Vec<(String, String)> = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare("SELECT id, content FROM memories WHERE embedding IS NULL")
                .map_err(|e| MemoryError::Embedding {
                    message: format!("select NULL embeddings failed: {e}"),
                })?;
            let rows: Vec<(String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| MemoryError::Embedding {
                    message: format!("query NULL embeddings failed: {e}"),
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok::<_, MemoryError>(rows)
        })
        .await
        .map_err(|e| MemoryError::Embedding {
            message: format!("reindex task failed: {e}"),
        })??;

        let mut count = 0;
        for (id, content) in &rows {
            if let Some(embedding) = self.get_or_compute_embedding(content).await? {
                let conn = self.conn.clone();
                let id = id.clone();
                let blob = vec_to_bytes(&embedding);
                tokio::task::spawn_blocking(move || {
                    let conn = conn.lock();
                    let _ = conn.execute(
                        "UPDATE memories SET embedding = ?1 WHERE id = ?2",
                        rusqlite::params![blob, id],
                    );
                })
                .await
                .unwrap_or(());
                count += 1;
            }
        }

        Ok(count)
    }
}
