//! Auto-mode orchestrator + enterprise context builder — port of v2 automode.py
//! with #22 causal fallback, #23/#42 noise filtering and freshness formatting,
//! #44 profile pass-through.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{json, Value};

use crate::causal::CausalTracker;
use crate::coldstart::{self, DiscoveryResult};
use crate::router::{fmt_fact_line, Router};
use crate::store::{Fact, Store};
use crate::util::{is_noise_content, now_iso};

#[derive(Clone)]
struct Fingerprint {
    root_path: String,
    project_type: String,
    file_count: i64,
    last_discovery: String,
}

pub struct Orchestrator {
    store: Arc<Store>,
    pub project_root: String,
    fingerprint: Mutex<Option<Fingerprint>>,
    pub last_discovery_performed: Mutex<bool>,
}

impl Orchestrator {
    pub fn new(store: Arc<Store>, project_root: &str) -> Orchestrator {
        Orchestrator {
            store,
            project_root: project_root.to_string(),
            fingerprint: Mutex::new(None),
            last_discovery_performed: Mutex::new(false),
        }
    }

    fn load_fingerprint(&self) -> Option<Fingerprint> {
        if let Some(fp) = self.fingerprint.lock().unwrap().as_ref() {
            return Some(fp.clone());
        }
        let facts = self.store.get_facts_by_level(0, None, false, false).ok()?;
        for fact in facts {
            if let Some(json_str) = fact.content.strip_prefix("__FINGERPRINT__:") {
                if let Ok(v) = serde_json::from_str::<Value>(json_str) {
                    let fp = Fingerprint {
                        root_path: v.get("root_path")?.as_str()?.to_string(),
                        project_type: v.get("project_type")?.as_str()?.to_string(),
                        file_count: v.get("file_count")?.as_i64()?,
                        last_discovery: v.get("last_discovery")?.as_str()?.to_string(),
                    };
                    *self.fingerprint.lock().unwrap() = Some(fp.clone());
                    return Some(fp);
                }
            }
        }
        None
    }

    fn save_fingerprint(&self, result: &DiscoveryResult) -> Result<()> {
        let now = now_iso();
        let fp = Fingerprint {
            root_path: self.project_root.clone(),
            project_type: result.project_info.project_type.clone(),
            file_count: result.project_info.file_count,
            last_discovery: now.clone(),
        };
        let content = format!(
            "__FINGERPRINT__:{}",
            json!({
                "root_path": fp.root_path,
                "project_type": fp.project_type,
                "file_count": fp.file_count,
                "created_at": now,
                "last_discovery": fp.last_discovery,
            })
        );
        // remove old fingerprints (soft delete per #45)
        let facts = self.store.get_facts_by_level(0, None, false, false)?;
        for fact in facts {
            if fact.content.starts_with("__FINGERPRINT__:") {
                self.store.delete_fact(&fact.id)?;
            }
        }
        self.store.add_fact(&content, 0, None, None, None, None, None, None, 1.0, "fingerprint", None)?;
        *self.fingerprint.lock().unwrap() = Some(fp);
        Ok(())
    }

    fn should_discover(&self) -> (bool, &'static str) {
        let l0 = self.store.get_facts_by_level(0, None, false, false).unwrap_or_default();
        if l0.is_empty() {
            return (true, "no_l0_facts");
        }
        let fp = match self.load_fingerprint() {
            Some(fp) => fp,
            None => return (true, "no_fingerprint"),
        };
        if fp.root_path != self.project_root {
            return (true, "root_changed");
        }
        if let Some(last) = crate::util::parse_iso(&fp.last_discovery) {
            let days = (chrono::Local::now() - last).num_days();
            if days > 30 {
                return (true, "stale_discovery");
            }
        }
        (false, "up_to_date")
    }

    pub fn discover_or_restore(&self, task_hint: Option<&str>) -> Result<()> {
        let (should, reason) = self.should_discover();
        if should {
            tracing::info!("Running discovery (reason: {reason})");
            let result = coldstart::discover_project(&self.store, &self.project_root, task_hint)?;
            self.save_fingerprint(&result)?;
            *self.last_discovery_performed.lock().unwrap() = true;
        } else {
            *self.last_discovery_performed.lock().unwrap() = false;
        }
        Ok(())
    }

    pub fn force_discovery(&self, task_hint: Option<&str>) -> Result<()> {
        let result = coldstart::discover_project(&self.store, &self.project_root, task_hint)?;
        self.save_fingerprint(&result)?;
        *self.last_discovery_performed.lock().unwrap() = true;
        Ok(())
    }
}

pub struct EnterpriseContext {
    pub facts: Vec<Fact>,
    pub causal_summary: String,
    pub project_overview: String,
    pub total_tokens: i64,
    pub discovery_performed: bool,
    pub suggestions: Vec<Value>,
    pub profile_warning: Option<String>,
}

impl EnterpriseContext {
    /// #42 to_injection_string with freshness dates and noise filtering.
    pub fn to_injection_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let overview = &self.project_overview;
        if !overview.is_empty() && !overview.contains("__FINGERPRINT__") && !overview.contains("Unknown project") {
            parts.push("## Project Overview".to_string());
            parts.push(overview.clone());
            parts.push(String::new());
        }
        let clean: Vec<&Fact> = self.facts.iter().filter(|f| !is_noise_content(&f.content)).collect();
        let l0: Vec<&&Fact> = clean.iter().filter(|f| f.level == 0).collect();
        let l1: Vec<&&Fact> = clean.iter().filter(|f| f.level == 1).collect();
        let l2: Vec<&&Fact> = clean.iter().filter(|f| f.level == 2).collect();

        if !l0.is_empty() {
            parts.push("## Architecture".to_string());
            for f in l0 {
                parts.push(fmt_fact_line(f, false));
            }
            parts.push(String::new());
        }
        if !l1.is_empty() {
            parts.push("## Domains".to_string());
            for f in l1 {
                parts.push(fmt_fact_line(f, true));
            }
            parts.push(String::new());
        }
        if !l2.is_empty() {
            parts.push("## Modules".to_string());
            for f in l2.iter().take(10) {
                parts.push(fmt_fact_line(f, false));
            }
            parts.push(String::new());
        }
        if !self.causal_summary.is_empty() {
            parts.push("## Past Decisions".to_string());
            parts.push(self.causal_summary.clone());
        }
        parts.join("\n")
    }
}

pub struct ContextBuilder {
    pub store: Arc<Store>,
    pub router: Arc<Router>,
    pub causal: Arc<CausalTracker>,
    pub orchestrator: Arc<Orchestrator>,
}

impl ContextBuilder {
    pub fn build(
        &self,
        query: &str,
        max_tokens: i64,
        include_causal: bool,
        task_hint: Option<&str>,
        profile: Option<&str>,
    ) -> Result<EnterpriseContext> {
        self.orchestrator.discover_or_restore(task_hint)?;
        let causal_budget = if include_causal { 500 } else { 0 };
        let routing = self.router.route(query, Some(max_tokens - causal_budget), false, true, None, profile, None)?;

        let causal_summary = if include_causal { self.get_causal_summary(query)? } else { String::new() };
        let project_overview = self.get_project_overview()?;
        let suggestions = self.build_suggestions()?;

        let mut total_tokens = routing.total_tokens;
        total_tokens += causal_summary.chars().count() as i64 / 4;
        total_tokens += project_overview.chars().count() as i64 / 4;

        Ok(EnterpriseContext {
            facts: routing.facts,
            causal_summary,
            project_overview,
            total_tokens,
            discovery_performed: *self.orchestrator.last_discovery_performed.lock().unwrap(),
            suggestions,
            profile_warning: routing.profile_warning,
        })
    }

    /// #22: upstream LIKE query first, then recent-decisions fallback.
    fn get_causal_summary(&self, query: &str) -> Result<String> {
        if let Ok(Some(chain)) = self.causal.query_chain(query, 3) {
            let s = self.causal.format_chain_summary(&chain);
            if !s.is_empty() {
                return Ok(s);
            }
        }
        match self.causal.recent_decisions_summary(query) {
            Ok(s) => Ok(s),
            Err(e) => {
                tracing::warn!("[#22] Causal fallback query failed: {e}");
                Ok(String::new())
            }
        }
    }

    fn get_project_overview(&self) -> Result<String> {
        let l0 = self.store.get_facts_by_level(0, None, false, false)?;
        for f in l0 {
            if !f.content.starts_with("__FINGERPRINT__:") {
                return Ok(f.content);
            }
        }
        Ok(String::new())
    }

    fn build_suggestions(&self) -> Result<Vec<Value>> {
        let mut suggestions = Vec::new();
        let git_dir = std::path::Path::new(&self.orchestrator.project_root).join(".git");
        if git_dir.exists() && !git_dir.join("hooks/post-commit").exists() {
            suggestions.push(json!({
                "type": "install_git_hook",
                "message": "Install git hook for auto-extract",
                "command": "rlm_install_git_hooks()",
                "priority": "medium",
            }));
        }
        let stats = self.store.get_stats()?;
        let total = stats.get("total_facts").and_then(|v| v.as_i64()).unwrap_or(0);
        let with_emb = stats.get("with_embeddings").and_then(|v| v.as_i64()).unwrap_or(0);
        if total > 0 && with_emb == 0 {
            suggestions.push(json!({
                "type": "index_embeddings",
                "message": "Index facts with embeddings for better routing",
                "command": "rlm_index_embeddings()",
                "priority": "high",
            }));
        }
        Ok(suggestions)
    }
}

pub fn enterprise_response(ctx: &EnterpriseContext, profile: &str) -> Value {
    let mut resp = json!({
        "status": "success",
        "context": ctx.to_injection_string(),
        "facts_count": ctx.facts.len(),
        "tokens_used": ctx.total_tokens,
        "discovery_performed": ctx.discovery_performed,
        "causal_included": !ctx.causal_summary.is_empty(),
        "profile": if profile.is_empty() { "default" } else { profile },
        "suggestions": ctx.suggestions,
    });
    if let Some(w) = &ctx.profile_warning {
        resp["warning"] = json!(w);
    }
    resp
}