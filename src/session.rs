//! v1 cognitive-state sessions — port of StateStorage + MemoryBridgeManager
//! with the "default" canonical-session patch (#8/#7/#18/#21).

use std::path::Path;
use std::sync::Mutex;

use anyhow::{bail, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::util::now_iso;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS states (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    timestamp TEXT NOT NULL,
    checksum TEXT NOT NULL,
    data BLOB NOT NULL,
    nonce BLOB,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_id, version)
);
CREATE INDEX IF NOT EXISTS idx_session_id ON states(session_id);
CREATE INDEX IF NOT EXISTS idx_timestamp ON states(timestamp);
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    action TEXT NOT NULL,
    version INTEGER,
    timestamp TEXT NOT NULL,
    details TEXT,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_audit_session ON audit_log(session_id);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
";

pub struct SessionManager {
    pub db_path: String,
    conn: Mutex<Connection>,
    current: Mutex<Option<Value>>,
}

fn new_state(session_id: &str) -> Value {
    json!({
        "session_id": session_id,
        "version": 1,
        "timestamp": now_iso(),
        "primary_goal": null,
        "active_sub_goal": null,
        "hypotheses": [],
        "decisions": [],
        "facts": [],
        "communities": [],
        "confidence_map": {},
        "key_facts": [],
        "open_questions": [],
        "context_summary": null,
    })
}

impl SessionManager {
    pub fn new(db_path: &str) -> Result<SessionManager> {
        if let Some(parent) = Path::new(db_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA)?;
        Ok(SessionManager { db_path: db_path.to_string(), conn: Mutex::new(conn), current: Mutex::new(None) })
    }

    fn load_state(&self, session_id: &str) -> Result<Option<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT data, checksum FROM states WHERE session_id = ?1 ORDER BY version DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![session_id])?;
        match rows.next()? {
            None => Ok(None),
            Some(row) => {
                let data: Vec<u8> = row.get(0)?;
                let checksum: String = row.get(1)?;
                let mut hasher = Sha256::new();
                hasher.update(&data);
                let computed = format!("{:x}", hasher.finalize());
                if computed != checksum {
                    bail!("Checksum mismatch for session {session_id}");
                }
                let v: Value = serde_json::from_slice(&data)?;
                Ok(Some(v))
            }
        }
    }

    fn save_state(&self, state: &Value) -> Result<i64> {
        // Python: json.dumps(to_dict(), ensure_ascii=False, indent=2)
        let json_data = serde_json::to_string_pretty(state)?;
        let bytes = json_data.as_bytes();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let checksum = format!("{:x}", hasher.finalize());
        let session_id = state.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
        let version = state.get("version").and_then(|v| v.as_i64()).unwrap_or(1);
        let timestamp = state.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let facts_count = state.get("facts").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

        let conn = self.conn.lock().unwrap();
        let existing: i64 = conn.query_row("SELECT COUNT(*) FROM states WHERE session_id = ?1", [session_id], |r| r.get(0))?;
        let action = if existing == 0 { "create" } else { "update" };
        conn.execute(
            "INSERT INTO states (session_id, version, timestamp, checksum, data, nonce)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            rusqlite::params![session_id, version, timestamp, checksum, bytes],
        )?;
        conn.execute(
            "INSERT INTO audit_log (session_id, action, version, timestamp, details)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, action, version, now_iso(), format!("Saved state with {facts_count} facts")],
        )?;
        Ok(version)
    }

    /// Patched start_session (#8/#7/#18/#21): "default" canonical when session_id=None.
    pub fn start_session(&self, session_id: Option<&str>, restore: bool) -> Result<Value> {
        let state = match session_id {
            None => {
                if restore {
                    if let Some(mut s) = self.load_state("default")? {
                        let v = s.get("version").and_then(|x| x.as_i64()).unwrap_or(0) + 1;
                        s["version"] = json!(v);
                        s["timestamp"] = json!(now_iso());
                        s
                    } else {
                        self.new_default_with_version_check()?
                    }
                } else {
                    self.new_default_with_version_check()?
                }
            }
            Some(sid) => {
                if restore {
                    if let Some(mut s) = self.load_state(sid)? {
                        let v = s.get("version").and_then(|x| x.as_i64()).unwrap_or(0) + 1;
                        s["version"] = json!(v);
                        s["timestamp"] = json!(now_iso());
                        s
                    } else {
                        new_state(sid)
                    }
                } else {
                    new_state(sid)
                }
            }
        };
        let sid = state.get("session_id").and_then(|v| v.as_str()).unwrap_or("default").to_string();
        let version = state.get("version").and_then(|v| v.as_i64()).unwrap_or(1);
        *self.current.lock().unwrap() = Some(state);
        Ok(json!({
            "status": "success",
            "session_id": sid,
            "version": version,
            "restored": restore && version > 1,
            "message": format!("Session started: {sid}"),
        }))
    }

    fn new_default_with_version_check(&self) -> Result<Value> {
        let mut state = new_state("default");
        if state.get("version").and_then(|v| v.as_i64()) == Some(1) {
            if let Some(existing) = self.load_state("default")? {
                let ev = existing.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
                if ev >= 1 {
                    state["version"] = json!(ev + 1);
                }
            }
        }
        Ok(state)
    }

    pub fn sync_state(&self) -> Result<Value> {
        let mut guard = self.current.lock().unwrap();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return Ok(json!({"status": "error", "message": "No active session"})),
        };
        let version = self.save_state(state)?;
        state["version"] = json!(version + 1);
        state["timestamp"] = json!(now_iso());
        Ok(json!({
            "status": "success",
            "version": version,
            "message": format!("State synced (version {version})"),
            "db_path": self.db_path,
        }))
    }
}