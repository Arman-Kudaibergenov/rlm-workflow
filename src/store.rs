//! Hierarchical fact store — drop-in port of rlm_toolkit memory_bridge v2
//! HierarchicalMemoryStore plus the start_server.py write-path patches
//! (#37 model_name, #42 valid_until stamps, #43 card embeddings, #45 soft delete).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::embed::Embedder;
use crate::util::{self, level_name, now_iso};

#[derive(Debug, Clone)]
pub struct TtlConfig {
    pub ttl_seconds: i64,
    pub refresh_trigger: Option<String>,
    pub on_expire: String, // mark_stale | archive | delete
}

impl TtlConfig {
    pub fn to_json(&self) -> Value {
        json!({
            "ttl_seconds": self.ttl_seconds,
            "refresh_trigger": self.refresh_trigger,
            "on_expire": self.on_expire,
        })
    }
    pub fn from_json(v: &Value) -> Option<TtlConfig> {
        Some(TtlConfig {
            ttl_seconds: v.get("ttl_seconds")?.as_i64()?,
            refresh_trigger: v.get("refresh_trigger").and_then(|x| x.as_str()).map(String::from),
            on_expire: v.get("on_expire").and_then(|x| x.as_str()).unwrap_or("mark_stale").to_string(),
        })
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Fact {
    pub id: String,
    pub content: String,
    pub level: i64,
    pub domain: Option<String>,
    pub module: Option<String>,
    pub code_ref: Option<String>,
    pub parent_id: Option<String>,
    pub children_ids: Vec<String>,
    pub ttl_config: Option<TtlConfig>,
    pub created_at: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub is_stale: bool,
    pub is_archived: bool,
    pub confidence: f64,
    pub source: String,
}

impl Fact {
    pub fn token_estimate(&self) -> i64 {
        util::char_len(&self.content) as i64 / 4 + 20
    }

    pub fn is_expired(&self) -> bool {
        match &self.ttl_config {
            None => false,
            Some(t) => {
                let created = match util::parse_iso(&self.created_at) {
                    Some(c) => c,
                    None => return false,
                };
                let expiry = created + chrono::Duration::seconds(t.ttl_seconds);
                chrono::Local::now() > expiry
            }
        }
    }

    /// Bi-temporal check (#45): was this fact valid at moment dt (ISO string compare)?
    pub fn valid_at(&self, dt_iso: &str) -> bool {
        let vf = if self.valid_from.is_empty() { &self.created_at } else { &self.valid_from };
        if !vf.is_empty() && vf.as_str() > dt_iso {
            return false;
        }
        if let Some(vu) = &self.valid_until {
            if !vu.is_empty() && vu.as_str() <= dt_iso {
                return false;
            }
        }
        true
    }
}

pub struct Store {
    pub db_path: String,
    conn: Mutex<Connection>,
    pub embedder: RwLock<Option<Arc<Embedder>>>,
    pub model_name: RwLock<String>,
    cache: RwLock<Option<Arc<StoreCache>>>,
}

/// In-memory snapshot of the hot read paths (children map + facts with embeddings).
/// Rebuilt lazily and cleared on every mutation, so searches skip re-parsing ~7k
/// JSON embedding vectors from SQLite on each call.
struct StoreCache {
    children: HashMap<String, Vec<String>>,
    facts_emb: Arc<Vec<(Fact, Vec<f32>)>>,
}

fn open_conn(path: &str) -> Result<Connection> {
    let conn = if path == ":memory:" {
        Connection::open_in_memory()?
    } else {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Connection::open(path)?
    };
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    if path != ":memory:" {
        // WAL: concurrent readers don't block the writer; NORMAL is safe with WAL.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    }
    Ok(conn)
}

fn text_of(row: &rusqlite::Row, col: &str) -> Option<String> {
    use rusqlite::types::ValueRef;
    match row.get_ref(col) {
        Ok(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
        Ok(ValueRef::Blob(b)) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

impl Store {
    pub fn new(db_path: &str) -> Result<Store> {
        let conn = open_conn(db_path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store {
            db_path: db_path.to_string(),
            conn: Mutex::new(conn),
            embedder: RwLock::new(None),
            model_name: RwLock::new("all-MiniLM-L6-v2".to_string()),
            cache: RwLock::new(None),
        })
    }

    pub fn set_embedder(&self, e: Arc<Embedder>) {
        *self.model_name.write().unwrap() = e.model_name().to_string();
        *self.embedder.write().unwrap() = Some(e);
    }

    fn invalidate_cache(&self) {
        *self.cache.write().unwrap() = None;
    }

    /// Return the cached children map + facts-with-embeddings, rebuilding if stale.
    /// Locks `conn` only while building, never while the caller holds it.
    fn get_cache(&self) -> Result<Arc<StoreCache>> {
        if let Some(c) = self.cache.read().unwrap().as_ref() {
            return Ok(c.clone());
        }
        let built = {
            let conn = self.conn.lock().unwrap();
            let children = Self::load_children(&conn)?;
            let mut stmt = conn.prepare(
                "SELECT hf.*, ei.embedding as stored_embedding
                 FROM hierarchical_facts hf
                 JOIN embeddings_index ei ON hf.id = ei.fact_id
                 WHERE hf.is_archived = 0",
            )?;
            let mut facts_emb = Vec::new();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let fact = Self::row_to_fact(row, &children)?;
                let emb = text_of(row, "stored_embedding")
                    .and_then(|s| serde_json::from_str::<Vec<f32>>(&s).ok())
                    .unwrap_or_default();
                facts_emb.push((fact, emb));
            }
            Arc::new(StoreCache { children, facts_emb: Arc::new(facts_emb) })
        };
        *self.cache.write().unwrap() = Some(built.clone());
        Ok(built)
    }

    fn row_to_fact(row: &rusqlite::Row, children: &HashMap<String, Vec<String>>) -> rusqlite::Result<Fact> {
        let id: String = row.get("id")?;
        let ttl_config = text_of(row, "ttl_config")
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| TtlConfig::from_json(&v));
        Ok(Fact {
            children_ids: children.get(&id).cloned().unwrap_or_default(),
            id,
            content: row.get("content")?,
            level: row.get("level")?,
            domain: row.get("domain")?,
            module: row.get("module")?,
            code_ref: row.get("code_ref")?,
            parent_id: row.get("parent_id")?,
            ttl_config,
            created_at: row.get("created_at")?,
            valid_from: row.get("valid_from")?,
            valid_until: row.get("valid_until")?,
            is_stale: row.get::<_, i64>("is_stale")? != 0,
            is_archived: row.get::<_, i64>("is_archived")? != 0,
            confidence: row.get("confidence")?,
            source: row.get("source")?,
        })
    }

    fn load_children(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
        let mut stmt = conn.prepare("SELECT parent_id, child_id FROM fact_hierarchy")?;
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (p, c) = row?;
            map.entry(p).or_default().push(c);
        }
        Ok(map)
    }

    /// Card-text wrapper for embeddings (#43) — KEEP IN SYNC with migrate_card_embeddings.py
    pub fn card_text(content: &str, domain: Option<&str>, level: i64, module: Option<&str>) -> String {
        let first = content
            .trim()
            .lines()
            .next()
            .map(|l| util::char_slice(l, 0, 120))
            .unwrap_or_default();
        let mut parts = vec![
            format!("domain: {}", domain.unwrap_or("general")),
            format!("level: {}", level),
        ];
        if let Some(m) = module {
            if !m.is_empty() {
                parts.push(format!("module: {}", m));
            }
        }
        if !first.is_empty() {
            parts.push(format!("title: {}", first));
        }
        let joined = format!("{} | {}", parts.join(" | "), content);
        util::char_slice(&joined, 0, 1000)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_fact(
        &self,
        content: &str,
        level: i64,
        domain: Option<&str>,
        module: Option<&str>,
        code_ref: Option<&str>,
        parent_id: Option<&str>,
        ttl_config: Option<TtlConfig>,
        embedding: Option<Vec<f32>>,
        confidence: f64,
        source: &str,
        session_id: Option<&str>,
    ) -> Result<String> {
        let fact_id = uuid::Uuid::new_v4().to_string();
        let now = now_iso();

        // Default TTL for L2/L3 (v2.1 memory lifecycle)
        let ttl_config = ttl_config.or(match level {
            2 => Some(TtlConfig { ttl_seconds: 30 * 24 * 3600, refresh_trigger: None, on_expire: "mark_stale".into() }),
            3 => Some(TtlConfig { ttl_seconds: 7 * 24 * 3600, refresh_trigger: None, on_expire: "mark_stale".into() }),
            _ => None,
        });

        // Card-text embedding on write (#43), computed outside the DB lock
        let embedding = match embedding {
            Some(e) => Some(e),
            None => {
                let emb = self.embedder.read().unwrap().clone();
                match emb {
                    Some(e) => {
                        let card = Self::card_text(content, domain, level, module);
                        match e.encode(&card) {
                            Ok(v) => Some(v),
                            Err(err) => {
                                tracing::warn!("[#43] card embedding failed, raw fallback: {err}");
                                None
                            }
                        }
                    }
                    None => None,
                }
            }
        };

        let emb_json = embedding.as_ref().map(|e| serde_json::to_string(e).unwrap());
        let ttl_json = ttl_config.as_ref().map(|t| serde_json::to_string(&t.to_json()).unwrap());
        let model_name = self.model_name.read().unwrap().clone();

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO hierarchical_facts (
                id, content, level, domain, module, code_ref, parent_id,
                embedding, ttl_config, created_at, valid_from, confidence,
                source, session_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                fact_id, content, level, domain, module, code_ref, parent_id,
                emb_json, ttl_json, now, confidence, source, session_id
            ],
        )?;
        if let Some(pid) = parent_id {
            conn.execute(
                "INSERT OR IGNORE INTO fact_hierarchy (parent_id, child_id) VALUES (?1, ?2)",
                rusqlite::params![pid, fact_id],
            )?;
        }
        if let Some(ej) = &emb_json {
            // #37: write model_name explicitly (upstream relied on column DEFAULT)
            conn.execute(
                "INSERT OR REPLACE INTO embeddings_index (fact_id, embedding, model_name, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![fact_id, ej, model_name, now],
            )?;
        }
        drop(conn);
        self.invalidate_cache();
        Ok(fact_id)
    }

    #[allow(dead_code)]
    pub fn get_fact(&self, fact_id: &str) -> Result<Option<Fact>> {
        let cache = self.get_cache()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM hierarchical_facts WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![fact_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::row_to_fact(row, &cache.children)?)),
            None => Ok(None),
        }
    }

    pub fn get_facts_by_level(
        &self,
        level: i64,
        domain: Option<&str>,
        include_stale: bool,
        include_archived: bool,
    ) -> Result<Vec<Fact>> {
        let cache = self.get_cache()?;
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from("SELECT * FROM hierarchical_facts WHERE level = ?1");
        if domain.is_some() {
            sql.push_str(" AND domain = ?2");
        }
        if !include_stale {
            sql.push_str(" AND is_stale = 0");
        }
        if !include_archived {
            sql.push_str(" AND is_archived = 0");
        }
        sql.push_str(" ORDER BY created_at DESC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = if let Some(d) = domain {
            stmt.query_map(rusqlite::params![level, d], |r| Self::row_to_fact(r, &cache.children))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(rusqlite::params![level], |r| Self::row_to_fact(r, &cache.children))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn get_all_facts(&self, include_stale: bool, include_archived: bool) -> Result<Vec<Fact>> {
        let cache = self.get_cache()?;
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from("SELECT * FROM hierarchical_facts WHERE 1=1");
        if !include_stale {
            sql.push_str(" AND is_stale = 0");
        }
        if !include_archived {
            sql.push_str(" AND is_archived = 0");
        }
        sql.push_str(" ORDER BY level ASC, created_at DESC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], |r| Self::row_to_fact(r, &cache.children))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_domain_facts(&self, domain: &str) -> Result<Vec<Fact>> {
        let cache = self.get_cache()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT * FROM hierarchical_facts
             WHERE domain = ?1 AND is_stale = 0 AND is_archived = 0
             ORDER BY level ASC, created_at DESC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![domain], |r| Self::row_to_fact(r, &cache.children))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// JOIN embeddings_index, is_archived = 0 (includes stale — callers filter).
    /// Served from the in-memory cache (rebuilt on mutation).
    pub fn get_facts_with_embeddings(&self) -> Result<Arc<Vec<(Fact, Vec<f32>)>>> {
        Ok(self.get_cache()?.facts_emb.clone())
    }

    /// Bi-temporal candidate pool (#45): valid_from <= as_of AND window open at as_of.
    pub fn get_facts_with_embeddings_as_of(&self, as_of_iso: &str) -> Result<Vec<(Fact, Vec<f32>)>> {
        let cache = self.get_cache()?;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT hf.*, ei.embedding AS stored_embedding
             FROM hierarchical_facts hf
             JOIN embeddings_index ei ON hf.id = ei.fact_id
             WHERE hf.valid_from <= ?1
             AND (hf.valid_until IS NULL OR hf.valid_until > ?1)",
        )?;
        let mut out = Vec::new();
        let mut rows = stmt.query(rusqlite::params![as_of_iso])?;
        while let Some(row) = rows.next()? {
            let fact = Self::row_to_fact(row, &cache.children)?;
            let emb = text_of(row, "stored_embedding")
                .and_then(|s| serde_json::from_str::<Vec<f32>>(&s).ok())
                .unwrap_or_default();
            out.push((fact, emb));
        }
        Ok(out)
    }

    pub fn update_embedding(&self, fact_id: &str, embedding: &[f32], model_name: &str) -> Result<()> {
        let ej = serde_json::to_string(embedding)?;
        let now = now_iso();
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE hierarchical_facts SET embedding = ?1 WHERE id = ?2", rusqlite::params![ej, fact_id])?;
        conn.execute(
            "INSERT OR REPLACE INTO embeddings_index (fact_id, embedding, model_name, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![fact_id, ej, model_name, now],
        )?;
        drop(conn);
        self.invalidate_cache();
        Ok(())
    }

    /// #42: stamp valid_until (COALESCE keeps a real expiry if set)
    pub fn mark_stale(&self, fact_id: &str) -> Result<bool> {
        let now = now_iso();
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE hierarchical_facts SET is_stale = 1, valid_until = COALESCE(valid_until, ?1) WHERE id = ?2",
            rusqlite::params![now, fact_id],
        )?;
        drop(conn);
        self.invalidate_cache();
        Ok(n > 0)
    }

    pub fn archive_fact(&self, fact_id: &str) -> Result<bool> {
        let now = now_iso();
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE hierarchical_facts SET is_archived = 1, valid_until = COALESCE(valid_until, ?1) WHERE id = ?2",
            rusqlite::params![now, fact_id],
        )?;
        drop(conn);
        self.invalidate_cache();
        Ok(n > 0)
    }

    /// #45: soft delete — close the window instead of removing the row.
    pub fn delete_fact(&self, fact_id: &str) -> Result<bool> {
        let now = now_iso();
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE hierarchical_facts
             SET is_archived = 1, is_stale = 1, valid_until = COALESCE(valid_until, ?1) WHERE id = ?2",
            rusqlite::params![now, fact_id],
        )?;
        drop(conn);
        self.invalidate_cache();
        Ok(n > 0)
    }

    /// Dedup gate #49 touch: refresh valid_from of a live duplicate.
    pub fn touch_valid_from(&self, fact_id: &str) -> Result<()> {
        let now = now_iso();
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE hierarchical_facts SET valid_from = ?1 WHERE id = ?2", rusqlite::params![now, fact_id])?;
        drop(conn);
        self.invalidate_cache();
        Ok(())
    }

    pub fn get_stats(&self) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM hierarchical_facts", [], |r| r.get(0))?;
        let mut by_level = serde_json::Map::new();
        for (lvl, name) in [(0, "L0_PROJECT"), (1, "L1_DOMAIN"), (2, "L2_MODULE"), (3, "L3_CODE")] {
            let c: i64 = conn.query_row("SELECT COUNT(*) FROM hierarchical_facts WHERE level = ?1", [lvl], |r| r.get(0))?;
            by_level.insert(name.to_string(), json!(c));
        }
        let stale: i64 = conn.query_row("SELECT COUNT(*) FROM hierarchical_facts WHERE is_stale = 1", [], |r| r.get(0))?;
        let archived: i64 = conn.query_row("SELECT COUNT(*) FROM hierarchical_facts WHERE is_archived = 1", [], |r| r.get(0))?;
        let with_emb: i64 = conn.query_row("SELECT COUNT(*) FROM embeddings_index", [], |r| r.get(0))?;
        let domains: i64 = conn.query_row("SELECT COUNT(DISTINCT domain) FROM hierarchical_facts WHERE domain IS NOT NULL", [], |r| r.get(0))?;
        Ok(json!({
            "total_facts": total,
            "by_level": Value::Object(by_level),
            "stale_facts": stale,
            "archived_facts": archived,
            "with_embeddings": with_emb,
            "domains": domains,
            "db_path": self.db_path,
            "schema_version": "2.0.0",
        }))
    }

    #[allow(dead_code)]
    pub fn get_domains(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT domain FROM hierarchical_facts WHERE domain IS NOT NULL")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Raw query access for tools that need custom SQL (diff, list_inactive, revisor).
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }

    /// #43: FTS5 index + triggers, idempotent, self-healing.
    pub fn ensure_fts(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(FTS_SCHEMA)?;
        let live: i64 = conn.query_row("SELECT COUNT(*) FROM hierarchical_facts WHERE is_archived = 0", [], |r| r.get(0))?;
        let indexed: i64 = conn.query_row("SELECT COUNT(*) FROM facts_fts", [], |r| r.get(0))?;
        if indexed != live {
            conn.execute_batch(
                "DELETE FROM facts_fts;
                 INSERT INTO facts_fts(content, domain, fact_id)
                 SELECT content, COALESCE(domain, ''), id
                 FROM hierarchical_facts WHERE is_archived = 0;",
            )?;
            tracing::info!("[#43] facts_fts rebuilt: {indexed} -> {live} rows");
        } else {
            tracing::info!("[#43] facts_fts OK: {live} live facts indexed");
        }
        Ok(())
    }

    /// Tiered FTS search (#46): rare-token tier, AND, OR fill. Returns (ids, rare_count).
    pub fn fts_search(&self, query: &str, limit: usize) -> (Vec<String>, usize) {
        let tokens = util::query_tokens(query);
        if tokens.is_empty() {
            return (Vec::new(), 0);
        }
        let quoted: Vec<String> = tokens.iter().map(|t| format!("\"{}\"", t.replace('"', ""))).collect();
        let conn = self.conn.lock().unwrap();
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut rare_n = 0usize;

        let run_match = |sql_match: &str, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| -> usize {
            let mut added = 0;
            let prep = conn.prepare(
                "SELECT fact_id FROM facts_fts WHERE facts_fts MATCH ?1 ORDER BY bm25(facts_fts) LIMIT ?2",
            );
            if let Ok(mut stmt) = prep {
                if let Ok(rows) = stmt.query_map(rusqlite::params![sql_match, limit as i64], |r| r.get::<_, String>(0)) {
                    for row in rows.flatten() {
                        if !seen.contains(&row) {
                            out.push(row.clone());
                            seen.insert(row);
                            added += 1;
                            if out.len() >= limit {
                                break;
                            }
                        }
                    }
                }
            }
            added
        };

        if quoted.len() > 1 {
            // Tier 0: facts matching the RAREST query token (df <= 10)
            let mut dfs: Vec<(i64, &String)> = Vec::new();
            for qt in &quoted {
                let df: i64 = conn
                    .query_row("SELECT COUNT(*) FROM facts_fts WHERE facts_fts MATCH ?1", [qt], |r| r.get(0))
                    .unwrap_or(0);
                dfs.push((df, qt));
            }
            if let Some((rare_df, rare_q)) = dfs.iter().min_by_key(|(df, _)| *df) {
                if *rare_df > 0 && *rare_df <= 10 {
                    run_match(rare_q, &mut out, &mut seen);
                    rare_n = out.len();
                }
            }
            // Tier 1: AND — all tokens
            if out.len() < limit {
                let and_q = quoted.join(" ");
                run_match(&and_q, &mut out, &mut seen);
            }
        }
        if out.len() < limit {
            let or_q = quoted.join(" OR ");
            run_match(&or_q, &mut out, &mut seen);
        }
        (out, rare_n)
    }

    /// #37 startup check data: distinct models + stale count + marker.
    pub fn embedding_model_state(&self) -> Result<(Vec<String>, i64, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT model_name FROM embeddings_index LIMIT 5")?;
        let models = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        let in_progress: bool = conn
            .query_row("SELECT COUNT(*) FROM schema_info WHERE key = 'reindex_in_progress'", [], |r| r.get::<_, i64>(0))?
            > 0;
        Ok((models, 0, in_progress))
    }

    pub fn stale_embedding_count(&self, model: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let c: i64 = conn.query_row("SELECT COUNT(*) FROM embeddings_index WHERE model_name != ?1", [model], |r| r.get(0))?;
        Ok(c)
    }

    pub fn set_schema_marker(&self, key: &str, value: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        match value {
            Some(v) => {
                conn.execute("INSERT OR REPLACE INTO schema_info (key, value) VALUES (?1, ?2)", [key, v])?;
            }
            None => {
                conn.execute("DELETE FROM schema_info WHERE key = ?1", [key])?;
            }
        }
        Ok(())
    }

    pub fn all_fact_id_content(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, content FROM hierarchical_facts WHERE is_archived = 0")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hierarchical_facts (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    level INTEGER NOT NULL DEFAULT 0,
    domain TEXT,
    module TEXT,
    code_ref TEXT,
    parent_id TEXT,
    embedding BLOB,
    ttl_config TEXT,
    created_at TEXT NOT NULL,
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    is_stale INTEGER DEFAULT 0,
    is_archived INTEGER DEFAULT 0,
    confidence REAL DEFAULT 1.0,
    source TEXT DEFAULT 'manual',
    session_id TEXT,
    FOREIGN KEY (parent_id) REFERENCES hierarchical_facts(id)
);
CREATE TABLE IF NOT EXISTS fact_hierarchy (
    parent_id TEXT NOT NULL,
    child_id TEXT NOT NULL,
    relationship TEXT DEFAULT 'contains',
    PRIMARY KEY (parent_id, child_id),
    FOREIGN KEY (parent_id) REFERENCES hierarchical_facts(id),
    FOREIGN KEY (child_id) REFERENCES hierarchical_facts(id)
);
CREATE TABLE IF NOT EXISTS embeddings_index (
    fact_id TEXT PRIMARY KEY,
    embedding BLOB NOT NULL,
    model_name TEXT DEFAULT 'all-MiniLM-L6-v2',
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (fact_id) REFERENCES hierarchical_facts(id)
);
CREATE TABLE IF NOT EXISTS domain_centroids (
    domain TEXT PRIMARY KEY,
    centroid BLOB NOT NULL,
    fact_count INTEGER DEFAULT 0,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_facts_level ON hierarchical_facts(level);
CREATE INDEX IF NOT EXISTS idx_facts_domain ON hierarchical_facts(domain);
CREATE INDEX IF NOT EXISTS idx_facts_module ON hierarchical_facts(module);
CREATE INDEX IF NOT EXISTS idx_facts_stale ON hierarchical_facts(is_stale);
CREATE INDEX IF NOT EXISTS idx_facts_session ON hierarchical_facts(session_id);
CREATE TABLE IF NOT EXISTS schema_info (key TEXT PRIMARY KEY, value TEXT);
INSERT OR REPLACE INTO schema_info (key, value) VALUES ('version', '2.0.0');
";

const FTS_SCHEMA: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(content, domain, fact_id UNINDEXED);
CREATE TRIGGER IF NOT EXISTS facts_fts_ai AFTER INSERT ON hierarchical_facts
WHEN NEW.is_archived = 0
BEGIN
    INSERT INTO facts_fts(content, domain, fact_id)
    VALUES (NEW.content, COALESCE(NEW.domain, ''), NEW.id);
END;
CREATE TRIGGER IF NOT EXISTS facts_fts_ad AFTER DELETE ON hierarchical_facts
BEGIN
    DELETE FROM facts_fts WHERE fact_id = OLD.id;
END;
CREATE TRIGGER IF NOT EXISTS facts_fts_ac AFTER UPDATE OF content, domain ON hierarchical_facts
BEGIN
    DELETE FROM facts_fts WHERE fact_id = NEW.id;
    INSERT INTO facts_fts(content, domain, fact_id)
    SELECT NEW.content, COALESCE(NEW.domain, ''), NEW.id
    WHERE NEW.is_archived = 0;
END;
CREATE TRIGGER IF NOT EXISTS facts_fts_aa AFTER UPDATE OF is_archived ON hierarchical_facts
BEGIN
    DELETE FROM facts_fts WHERE fact_id = NEW.id;
    INSERT INTO facts_fts(content, domain, fact_id)
    SELECT NEW.content, COALESCE(NEW.domain, ''), NEW.id
    WHERE NEW.is_archived = 0;
END;
";

#[allow(dead_code)]
fn unused_level_name(l: i64) -> &'static str {
    level_name(l)
}