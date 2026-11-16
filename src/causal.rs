//! Causal chain tracker — port of rlm_toolkit memory_bridge v2 causal.py
//! plus the #22 recent-decisions fallback from start_server.py.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::util::{char_slice, now_iso};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS causal_nodes (
    id TEXT PRIMARY KEY,
    node_type TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    session_id TEXT,
    metadata TEXT DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS causal_edges (
    from_id TEXT NOT NULL,
    to_id TEXT NOT NULL,
    edge_type TEXT NOT NULL,
    strength REAL DEFAULT 1.0,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (from_id, to_id, edge_type),
    FOREIGN KEY (from_id) REFERENCES causal_nodes(id),
    FOREIGN KEY (to_id) REFERENCES causal_nodes(id)
);
CREATE INDEX IF NOT EXISTS idx_nodes_type ON causal_nodes(node_type);
CREATE INDEX IF NOT EXISTS idx_nodes_session ON causal_nodes(session_id);
CREATE INDEX IF NOT EXISTS idx_edges_from ON causal_edges(from_id);
CREATE INDEX IF NOT EXISTS idx_edges_to ON causal_edges(to_id);
";

#[derive(Clone)]
#[allow(dead_code)]
pub struct CausalNode {
    pub id: String,
    pub node_type: String,
    pub content: String,
    pub created_at: String,
}

pub struct CausalChain {
    pub root: CausalNode,
    pub nodes: Vec<CausalNode>,
}

pub struct CausalTracker {
    pub db_path: String,
    conn: Mutex<Connection>,
}

impl CausalTracker {
    pub fn new(db_path: &str) -> Result<CausalTracker> {
        if let Some(parent) = Path::new(db_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA)?;
        Ok(CausalTracker { db_path: db_path.to_string(), conn: Mutex::new(conn) })
    }

    pub fn record_decision(
        &self,
        decision: &str,
        reasons: &[String],
        consequences: &[String],
        constraints: &[String],
        alternatives: &[String],
    ) -> Result<String> {
        let decision_id = uuid::Uuid::new_v4().to_string();
        let now = now_iso();
        let conn = self.conn.lock().unwrap();
        let save_node = |id: &str, node_type: &str, content: &str| -> Result<()> {
            conn.execute(
                "INSERT OR REPLACE INTO causal_nodes (id, node_type, content, created_at, session_id, metadata)
                 VALUES (?1, ?2, ?3, ?4, NULL, '{}')",
                rusqlite::params![id, node_type, content, now],
            )?;
            Ok(())
        };
        let save_edge = |from: &str, to: &str, edge_type: &str, strength: f64| -> Result<()> {
            conn.execute(
                "INSERT OR REPLACE INTO causal_edges (from_id, to_id, edge_type, strength, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![from, to, edge_type, strength, now],
            )?;
            Ok(())
        };
        save_node(&decision_id, "decision", decision)?;
        for r in reasons {
            let id = uuid::Uuid::new_v4().to_string();
            save_node(&id, "reason", r)?;
            save_edge(&id, &decision_id, "justifies", 1.0)?;
        }
        for c in consequences {
            let id = uuid::Uuid::new_v4().to_string();
            save_node(&id, "consequence", c)?;
            save_edge(&decision_id, &id, "leads_to", 1.0)?;
        }
        for c in constraints {
            let id = uuid::Uuid::new_v4().to_string();
            save_node(&id, "constraint", c)?;
            save_edge(&id, &decision_id, "blocks", 0.5)?;
        }
        for a in alternatives {
            let id = uuid::Uuid::new_v4().to_string();
            save_node(&id, "alternative", a)?;
            save_edge(&id, &decision_id, "conflicts", 0.3)?;
        }
        Ok(decision_id)
    }

    fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<CausalNode> {
        Ok(CausalNode {
            id: row.get("id")?,
            node_type: row.get("node_type")?,
            content: row.get("content")?,
            created_at: row.get::<_, Option<String>>("created_at")?.unwrap_or_default(),
        })
    }

    /// LIKE substring match on decision content, newest first.
    pub fn query_chain(&self, query: &str, max_depth: usize) -> Result<Option<CausalChain>> {
        let decision_id = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id FROM causal_nodes
                 WHERE node_type = 'decision' AND content LIKE ?1
                 ORDER BY created_at DESC LIMIT 1",
            )?;
            let pattern = format!("%{query}%");
            let mut rows = stmt.query(rusqlite::params![pattern])?;
            match rows.next()? {
                Some(r) => r.get::<_, String>(0)?,
                None => return Ok(None),
            }
        };
        self.get_chain_for_decision(&decision_id, max_depth)
    }

    pub fn get_chain_for_decision(&self, decision_id: &str, max_depth: usize) -> Result<Option<CausalChain>> {
        let conn = self.conn.lock().unwrap();
        let root = {
            let mut stmt = conn.prepare("SELECT * FROM causal_nodes WHERE id = ?1")?;
            let mut rows = stmt.query(rusqlite::params![decision_id])?;
            match rows.next()? {
                Some(r) => Self::row_to_node(r)?,
                None => return Ok(None),
            }
        };
        let mut visited: HashSet<String> = HashSet::from([decision_id.to_string()]);
        let mut nodes = vec![root.clone()];
        let mut frontier = vec![decision_id.to_string()];
        let mut depth = 0;
        while !frontier.is_empty() && depth < max_depth {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                let mut stmt = conn.prepare("SELECT from_id, to_id FROM causal_edges WHERE from_id = ?1 OR to_id = ?1")?;
                let edges: Vec<(String, String)> = stmt
                    .query_map(rusqlite::params![node_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                for (from, to) in edges {
                    let other = if &from == node_id { to.clone() } else { from.clone() };
                    if !visited.contains(&other) {
                        visited.insert(other.clone());
                        let mut ns = conn.prepare("SELECT * FROM causal_nodes WHERE id = ?1")?;
                        let mut rows = ns.query(rusqlite::params![other])?;
                        if let Some(r) = rows.next()? {
                            nodes.push(Self::row_to_node(r)?);
                            next_frontier.push(other);
                        }
                    }
                }
            }
            frontier = next_frontier;
            depth += 1;
        }
        Ok(Some(CausalChain { root, nodes }))
    }

    pub fn format_chain_summary(&self, chain: &CausalChain) -> String {
        let mut lines = vec![format!("## Decision: {}", chain.root.content)];
        let reasons: Vec<&CausalNode> = chain.nodes.iter().filter(|n| n.node_type == "reason").collect();
        let constraints: Vec<&CausalNode> = chain.nodes.iter().filter(|n| n.node_type == "constraint").collect();
        let consequences: Vec<&CausalNode> = chain.nodes.iter().filter(|n| n.node_type == "consequence").collect();
        if !reasons.is_empty() {
            lines.push("\n### Reasons:".to_string());
            for r in reasons {
                lines.push(format!("- {}", r.content));
            }
        }
        if !constraints.is_empty() {
            lines.push("\n### Constraints:".to_string());
            for c in constraints {
                lines.push(format!("- {}", c.content));
            }
        }
        if !consequences.is_empty() {
            lines.push("\n### Consequences:".to_string());
            for c in consequences {
                lines.push(format!("- {}", c.content));
            }
        }
        lines.join("\n")
    }

    /// #22 fallback: recent decisions matched by any query word, with reasons.
    pub fn recent_decisions_summary(&self, query: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let words: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.chars().count() > 2)
            .map(|w| w.to_lowercase())
            .collect();
        let rows: Vec<(String, String, String)> = if !words.is_empty() {
            let conditions = words.iter().map(|_| "LOWER(content) LIKE ?").collect::<Vec<_>>().join(" OR ");
            let sql = format!(
                "SELECT id, content, created_at FROM causal_nodes
                 WHERE node_type = 'decision' AND ({conditions})
                 ORDER BY created_at DESC LIMIT 5"
            );
            let params: Vec<String> = words.iter().map(|w| format!("%{w}%")).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default()))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, content, created_at FROM causal_nodes
                 WHERE node_type = 'decision' ORDER BY created_at DESC LIMIT 5",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        if rows.is_empty() {
            return Ok(String::new());
        }
        let mut parts = vec!["## Recent Decisions".to_string()];
        for (id, content, created) in rows {
            parts.push(format!("- [{}] {}", char_slice(&created, 0, 16), content));
            let mut stmt = conn.prepare(
                "SELECT cn.content FROM causal_nodes cn
                 JOIN causal_edges ce ON ce.from_id = cn.id
                 WHERE ce.to_id = ?1 AND cn.node_type = 'reason' LIMIT 3",
            )?;
            let reasons: Vec<String> = stmt
                .query_map(rusqlite::params![id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if !reasons.is_empty() {
                parts.push(format!("  Reasons: {}", reasons.join(", ")));
            }
        }
        Ok(parts.join("\n"))
    }

    pub fn get_stats(&self) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        let total_nodes: i64 = conn.query_row("SELECT COUNT(*) FROM causal_nodes", [], |r| r.get(0))?;
        let total_edges: i64 = conn.query_row("SELECT COUNT(*) FROM causal_edges", [], |r| r.get(0))?;
        let decisions: i64 = conn.query_row("SELECT COUNT(*) FROM causal_nodes WHERE node_type = 'decision'", [], |r| r.get(0))?;
        let mut by_type = serde_json::Map::new();
        for t in ["decision", "reason", "consequence", "constraint", "assumption", "alternative"] {
            let c: i64 = conn.query_row("SELECT COUNT(*) FROM causal_nodes WHERE node_type = ?1", [t], |r| r.get(0))?;
            by_type.insert(t.to_string(), json!(c));
        }
        Ok(json!({
            "total_nodes": total_nodes,
            "total_edges": total_edges,
            "decisions": decisions,
            "by_type": Value::Object(by_type),
            "db_path": self.db_path,
        }))
    }
}