//! MCP tool registry — exact schemas from the live Python server (tools_list.json)
//! and dispatch implementing all 18 tools with the start_server.py patches.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::automode::{enterprise_response, ContextBuilder, Orchestrator};
use crate::causal::CausalTracker;
use crate::coldstart;
use crate::consolidate;
use crate::embed::Embedder;
use crate::profiles;
use crate::revisor::Revisor;
use crate::router::Router;
use crate::search;
use crate::seqmatch;
use crate::session::SessionManager;
use crate::store::{Store, TtlConfig};
use crate::ttl;
use crate::util::{self, char_slice, level_name, now_iso, py_round, truncate};

pub struct AppState {
    pub store: Arc<Store>,
    pub embedder: Option<Arc<Embedder>>,
    pub router: Arc<Router>,
    pub causal: Arc<CausalTracker>,
    pub sessions: Arc<SessionManager>,
    pub orchestrator: Arc<Orchestrator>,
    pub builder: Arc<ContextBuilder>,
    pub revisor: Arc<Revisor>,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub has_output_schema: bool,
}

fn opt_str(title: &str) -> Value {
    json!({"anyOf": [{"type": "string"}, {"type": "null"}], "default": null, "title": title})
}
fn opt_int(title: &str) -> Value {
    json!({"anyOf": [{"type": "integer"}, {"type": "null"}], "default": null, "title": title})
}
fn opt_str_array(title: &str) -> Value {
    json!({"anyOf": [{"items": {"type": "string"}, "type": "array"}, {"type": "null"}], "default": null, "title": title})
}
fn out_schema(name: &str) -> Value {
    json!({
        "properties": {"result": {"additionalProperties": true, "title": "Result", "type": "object"}},
        "required": ["result"],
        "title": format!("{name}Output"),
        "type": "object"
    })
}

pub fn tool_registry() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "rlm_discover_project",
            description: "Smart cold start discovery for new projects. Detects project type, seeds template facts, discovers domains.",
            input_schema: json!({
                "properties": {"project_root": opt_str("Project Root"), "task_hint": opt_str("Task Hint")},
                "title": "rlm_discover_projectArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_route_context",
            description: "Semantic routing to get only relevant facts for a query. Loads L0 always, routes L1/L2 by similarity. Optional 'profile': agent loadout profile (see rlm_list_profiles). Optional 'as_of' (ISO date): knowledge state at that moment.",
            input_schema: json!({
                "properties": {
                    "query": {"title": "Query", "type": "string"},
                    "max_tokens": {"default": 2000, "title": "Max Tokens", "type": "integer"},
                    "include_stale": {"default": false, "title": "Include Stale", "type": "boolean"},
                    "profile": {"default": "", "title": "Profile", "type": "string"},
                    "as_of": {"default": "", "title": "As Of", "type": "string"}
                },
                "required": ["query"], "title": "_route_fnArguments", "type": "object"
            }),
            has_output_schema: false,
        },
        ToolDef {
            name: "rlm_enterprise_context",
            description: "One-call enterprise context with auto-discovery, semantic routing, and causal chains. Requires a query string. RECOMMENDED: Use this instead of individual tools. Optional 'profile': agent loadout profile (see rlm_list_profiles). Optional 'as_of' (ISO date): knowledge state at that moment.",
            input_schema: json!({
                "properties": {
                    "query": {"title": "Query", "type": "string"},
                    "max_tokens": {"default": 3000, "title": "Max Tokens", "type": "integer"},
                    "mode": {"default": "auto", "title": "Mode", "type": "string"},
                    "include_causal": {"default": true, "title": "Include Causal", "type": "boolean"},
                    "task_hint": {"default": "", "title": "Task Hint", "type": "string"},
                    "profile": {"default": "", "title": "Profile", "type": "string"}
                },
                "required": ["query"], "title": "_enterprise_fnArguments", "type": "object"
            }),
            has_output_schema: false,
        },
        ToolDef {
            name: "rlm_add_hierarchical_fact",
            description: "Add fact with hierarchical levels (L0-L3).",
            input_schema: json!({
                "properties": {
                    "content": {"title": "Content", "type": "string"},
                    "level": {"default": 0, "title": "Level", "type": "integer"},
                    "domain": opt_str("Domain"), "module": opt_str("Module"),
                    "code_ref": opt_str("Code Ref"), "parent_id": opt_str("Parent Id"),
                    "ttl_days": opt_int("Ttl Days")
                },
                "required": ["content"], "title": "rlm_add_hierarchical_factArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_get_stale_facts",
            description: "Get facts that have expired or need review.",
            input_schema: json!({
                "properties": {"include_archived": {"default": false, "title": "Include Archived", "type": "boolean"}},
                "title": "rlm_get_stale_factsArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_get_hierarchy_stats",
            description: "Get statistics about the hierarchical memory store.",
            input_schema: json!({"properties": {}, "title": "rlm_get_hierarchy_statsArguments", "type": "object"}),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_get_facts_by_domain",
            description: "Get all facts for a specific domain.",
            input_schema: json!({
                "properties": {
                    "domain": {"title": "Domain", "type": "string"},
                    "include_stale": {"default": false, "title": "Include Stale", "type": "boolean"}
                },
                "required": ["domain"], "title": "rlm_get_facts_by_domainArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_delete_fact",
            description: "Delete a fact from the hierarchical memory store.",
            input_schema: json!({
                "properties": {"fact_id": {"title": "Fact Id", "type": "string"}},
                "required": ["fact_id"], "title": "rlm_delete_factArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_consolidate_facts",
            description: "Consolidate granular facts into higher-level summaries. Aggregates L3→L2→L1 and deduplicates similar facts.",
            input_schema: json!({
                "properties": {"min_facts": {"default": 5, "title": "Min Facts", "type": "integer"}},
                "title": "rlm_consolidate_factsArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_record_causal_decision",
            description: "Record a decision with full causal context: reasons, consequences, constraints, alternatives.",
            input_schema: json!({
                "properties": {
                    "decision": {"title": "Decision", "type": "string"},
                    "reasons": opt_str_array("Reasons"), "consequences": opt_str_array("Consequences"),
                    "constraints": opt_str_array("Constraints"), "alternatives": opt_str_array("Alternatives")
                },
                "required": ["decision"], "title": "rlm_record_causal_decisionArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_start_session",
            description: "Start a new session or restore existing one. Required before adding facts.",
            input_schema: json!({
                "properties": {"session_id": opt_str("Session Id"), "restore": {"default": true, "title": "Restore", "type": "boolean"}},
                "title": "rlm_start_sessionArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_sync_state",
            description: "Save current cognitive state to persistent storage",
            input_schema: json!({"properties": {}, "title": "rlm_sync_stateArguments", "type": "object"}),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_search_facts",
            description: "Hybrid search across facts (semantic + keyword + recency)",
            input_schema: json!({
                "properties": {
                    "query": {"title": "Query", "type": "string"},
                    "top_k": {"default": 10, "title": "Top K", "type": "integer"},
                    "semantic_weight": {"default": 0.5, "title": "Semantic Weight", "type": "number"},
                    "keyword_weight": {"default": 0.3, "title": "Keyword Weight", "type": "number"},
                    "recency_weight": {"default": 0.2, "title": "Recency Weight", "type": "number"}
                },
                "required": ["query"], "title": "rlm_search_factsArguments", "type": "object"
            }),
            has_output_schema: true,
        },
        ToolDef {
            name: "rlm_diff",
            description: "Show what changed in memory since a point in time. since: ISO date/datetime or relative '24h', '7d', '2w'. Groups: new (created after since), updated (valid_from changed), staled (window closed, valid_until after since; falls back to created_at for pre-#45 rows). Archived facts are excluded unless include_archived=true. Optional as_of (ISO date): keep only facts valid at that moment (valid_from <= as_of, window open at as_of) — this supersedes the is_archived flag filter. Paginate limit/offset.",
            input_schema: json!({
                "properties": {
                    "since": {"title": "Since", "type": "string"},
                    "include_archived": {"default": false, "title": "Include Archived", "type": "boolean"},
                    "limit": {"default": 200, "title": "Limit", "type": "integer"},
                    "offset": {"default": 0, "title": "Offset", "type": "integer"},
                    "as_of": {"default": "", "title": "As Of", "type": "string"}
                },
                "required": ["since"], "title": "rlm_diffArguments", "type": "object"
            }),
            has_output_schema: false,
        },
        ToolDef {
            name: "rlm_list_profiles",
            description: "List agent routing profiles (loadout). Pass profile=<name> to rlm_route_context / rlm_enterprise_context: profile-domain facts are boosted x2, others down-weighted x0.5. 'default' = no bias.",
            input_schema: json!({"properties": {}, "title": "rlm_list_profilesArguments", "type": "object"}),
            has_output_schema: false,
        },
        ToolDef {
            name: "rlm_invalidate_fact",
            description: "Close a fact's validity window (bi-temporal invalidate): valid_until=now + is_stale=1. The fact is NOT deleted — it stays visible to as_of queries about the past. reason is logged only.",
            input_schema: json!({
                "properties": {"fact_id": {"title": "Fact Id", "type": "string"}, "reason": {"default": "", "title": "Reason", "type": "string"}},
                "required": ["fact_id"], "title": "rlm_invalidate_factArguments", "type": "object"
            }),
            has_output_schema: false,
        },
        ToolDef {
            name: "rlm_list_inactive",
            description: "List recently invalidated facts (valid_until set), newest first — for memory reviews. Includes valid_until, stale/archived flags.",
            input_schema: json!({
                "properties": {"limit": {"default": 50, "title": "Limit", "type": "integer"}},
                "title": "rlm_list_inactiveArguments", "type": "object"
            }),
            has_output_schema: false,
        },
        ToolDef {
            name: "rlm_revisor_run",
            description: "Run the freshness revisor now. Levels: TTL rules per domain (/data/revisor.yaml), endpoint ping, optional LLM (deep=true). mode: '' = REVISOR_MODE env (default report), 'report' = dry run, 'auto' = invalidate (close validity window, cap 20). Writes a report fact (domain 'revisor').",
            input_schema: json!({
                "properties": {"deep": {"default": false, "title": "Deep", "type": "boolean"}, "mode": {"default": "", "title": "Mode", "type": "string"}},
                "title": "rlm_revisor_runArguments", "type": "object"
            }),
            has_output_schema: false,
        },
    ]
}

pub fn tools_list_value() -> Value {
    let tools: Vec<Value> = tool_registry()
        .into_iter()
        .map(|t| {
            let mut v = json!({"name": t.name, "description": t.description, "inputSchema": t.input_schema});
            if t.has_output_schema {
                v["outputSchema"] = out_schema(t.name);
            }
            v
        })
        .collect();
    json!({"tools": tools})
}

// ---- argument helpers ----

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}
fn arg_str_req<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    arg_str(args, key).ok_or_else(|| format!("Input validation error: '{key}' is a required property"))
}
fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}
fn arg_f64(args: &Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}
fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}
fn arg_str_array(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn err_value(msg: impl std::fmt::Display) -> Value {
    json!({"status": "error", "message": msg.to_string()})
}

/// Dispatch a tool call. Err(text) becomes an isError=true MCP result.
pub fn dispatch(state: &AppState, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "rlm_discover_project" => {
            let root = arg_str(args, "project_root")
                .map(String::from)
                .unwrap_or_else(|| state.orchestrator.project_root.clone());
            let hint = arg_str(args, "task_hint");
            match coldstart::discover_project(&state.store, &root, hint) {
                Ok(r) => Ok(coldstart::discovery_response(&r)),
                Err(e) => Ok(err_value(e)),
            }
        }
        "rlm_route_context" => {
            let query = arg_str_req(args, "query")?;
            let max_tokens = arg_i64(args, "max_tokens", 2000);
            let include_stale = arg_bool(args, "include_stale", false);
            let profile = arg_str(args, "profile").unwrap_or("");
            let as_of = arg_str(args, "as_of").unwrap_or("");
            match state.router.route(
                query,
                Some(max_tokens),
                include_stale,
                true,
                None,
                if profile.is_empty() { None } else { Some(profile) },
                if as_of.is_empty() { None } else { Some(as_of) },
            ) {
                Ok(r) => Ok(state.router.route_tool_response(&r)),
                Err(e) => Ok(err_value(e)),
            }
        }
        "rlm_enterprise_context" => {
            let query = arg_str_req(args, "query")?;
            let max_tokens = arg_i64(args, "max_tokens", 3000);
            let mode = arg_str(args, "mode").unwrap_or("auto");
            let include_causal = arg_bool(args, "include_causal", true);
            let task_hint = arg_str(args, "task_hint").unwrap_or("");
            let profile = arg_str(args, "profile").unwrap_or("");
            let hint = if task_hint.is_empty() { None } else { Some(task_hint) };
            let run = || -> anyhow::Result<Value> {
                if mode == "discovery" {
                    state.orchestrator.force_discovery(hint)?;
                } else if mode == "auto" {
                    state.orchestrator.discover_or_restore(hint)?;
                }
                let ctx = state.builder.build(
                    query,
                    max_tokens,
                    include_causal,
                    hint,
                    if profile.is_empty() { None } else { Some(profile) },
                )?;
                Ok(enterprise_response(&ctx, profile))
            };
            Ok(run().unwrap_or_else(err_value))
        }
        "rlm_add_hierarchical_fact" => Ok(add_fact_tool(state, args)),
        "rlm_get_stale_facts" => {
            let _include_archived = arg_bool(args, "include_archived", false);
            let report = ttl::process_expired(&state.store);
            let facts = state.store.get_all_facts(true, false).map_err(|e| e.to_string())?;
            let stale: Vec<_> = facts.into_iter().filter(|f| f.is_stale).collect();
            let stale_facts: Vec<Value> = stale
                .iter()
                .take(20)
                .map(|f| {
                    json!({
                        "id": f.id,
                        "content": truncate(&f.content, 100),
                        "level": level_name(f.level),
                        "domain": f.domain,
                        "created_at": f.created_at,
                        "fact_id": f.id,
                    })
                })
                .collect();
            Ok(json!({
                "status": "success",
                "stale_count": stale.len(),
                "stale_facts": stale_facts,
                "ttl_report": report,
            }))
        }
        "rlm_get_hierarchy_stats" => {
            let stats = state.store.get_stats().map_err(|e| e.to_string())?;
            let cstats = state.causal.get_stats().map_err(|e| e.to_string())?;
            Ok(json!({"status": "success", "memory_store": stats, "causal_chains": cstats}))
        }
        "rlm_get_facts_by_domain" => {
            let domain = arg_str_req(args, "domain")?;
            let include_stale = arg_bool(args, "include_stale", false);
            let mut facts = state.store.get_domain_facts(domain).map_err(|e| e.to_string())?;
            if !include_stale {
                facts.retain(|f| !f.is_stale);
            }
            let items: Vec<Value> = facts
                .iter()
                .map(|f| {
                    json!({
                        "id": f.id,
                        "content": f.content,
                        "level": level_name(f.level),
                        "module": f.module,
                        "is_stale": f.is_stale,
                        "fact_id": f.id,
                    })
                })
                .collect();
            Ok(json!({
                "status": "success",
                "domain": domain,
                "facts_count": items.len(),
                "facts": items,
            }))
        }
        "rlm_delete_fact" => {
            let fact_id = arg_str_req(args, "fact_id")?;
            match state.store.delete_fact(fact_id) {
                Ok(true) => Ok(json!({"status": "success", "fact_id": fact_id, "message": "Fact deleted"})),
                Ok(false) => Ok(json!({"status": "error", "fact_id": fact_id, "message": "Fact not found"})),
                Err(e) => Ok(err_value(e)),
            }
        }
        "rlm_consolidate_facts" => {
            let min_facts = arg_i64(args, "min_facts", 5).max(0) as usize;
            Ok(consolidate::consolidate(&state.store, min_facts).unwrap_or_else(err_value))
        }
        "rlm_record_causal_decision" => {
            let decision = arg_str_req(args, "decision")?;
            let reasons = arg_str_array(args, "reasons");
            let consequences = arg_str_array(args, "consequences");
            let constraints = arg_str_array(args, "constraints");
            let alternatives = arg_str_array(args, "alternatives");
            match state.causal.record_decision(decision, &reasons, &consequences, &constraints, &alternatives) {
                Ok(id) => Ok(json!({
                    "status": "success",
                    "decision_id": id,
                    "decision": decision,
                    "reasons_count": reasons.len(),
                    "consequences_count": consequences.len(),
                    "constraints_count": constraints.len(),
                    "alternatives_count": alternatives.len(),
                })),
                Err(e) => Ok(err_value(e)),
            }
        }
        "rlm_start_session" => {
            let session_id = arg_str(args, "session_id");
            let restore = arg_bool(args, "restore", true);
            Ok(state.sessions.start_session(session_id, restore).unwrap_or_else(err_value))
        }
        "rlm_sync_state" => Ok(state.sessions.sync_state().unwrap_or_else(err_value)),
        "rlm_search_facts" => {
            let query = arg_str_req(args, "query")?;
            let top_k = arg_i64(args, "top_k", 10).max(0) as usize;
            let sw = arg_f64(args, "semantic_weight", 0.5);
            let kw = arg_f64(args, "keyword_weight", 0.3);
            let rw = arg_f64(args, "recency_weight", 0.2);
            Ok(search::hybrid_search(&state.store, state.embedder.as_ref(), query, top_k, sw, kw, rw))
        }
        "rlm_diff" => Ok(diff_tool(state, args)),
        "rlm_list_profiles" => Ok(profiles::list_response()),
        "rlm_invalidate_fact" => {
            let fact_id = arg_str_req(args, "fact_id")?;
            let reason = arg_str(args, "reason").unwrap_or("");
            match state.store.mark_stale(fact_id) {
                Ok(true) => {
                    if !reason.is_empty() {
                        tracing::info!("[#45] invalidate {fact_id}: {reason}");
                    }
                    Ok(json!({
                        "status": "success",
                        "fact_id": fact_id,
                        "valid_until": now_iso(),
                        "note": "window closed; still visible via as_of queries",
                    }))
                }
                Ok(false) => Ok(err_value(format!("fact not found: {fact_id}"))),
                Err(e) => Ok(err_value(e)),
            }
        }
        "rlm_list_inactive" => {
            let limit = arg_i64(args, "limit", 50).clamp(1, 500);
            let rows = state.store.with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, level, domain, created_at, valid_until, is_stale, is_archived,
                     substr(content, 1, 160) AS content
                     FROM hierarchical_facts WHERE valid_until IS NOT NULL
                     ORDER BY valid_until DESC LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map([limit], |r| {
                        Ok(json!({
                            "id": r.get::<_, String>(0)?,
                            "level": r.get::<_, i64>(1)?,
                            "domain": r.get::<_, Option<String>>(2)?,
                            "created_at": r.get::<_, String>(3)?,
                            "valid_until": r.get::<_, Option<String>>(4)?,
                            "is_stale": r.get::<_, i64>(5)?,
                            "is_archived": r.get::<_, i64>(6)?,
                            "content": r.get::<_, String>(7)?,
                        }))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            });
            match rows {
                Ok(items) => Ok(json!({"status": "success", "count": items.len(), "inactive": items})),
                Err(e) => Ok(err_value(e)),
            }
        }
        "rlm_revisor_run" => {
            let deep = arg_bool(args, "deep", false);
            let mode = arg_str(args, "mode").unwrap_or("");
            Ok(state.revisor.run(deep, mode).unwrap_or_else(err_value))
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

// ---- add_hierarchical_fact with WRITE-RULES (#48) and dedup (#49) gates ----

fn norm_fact_text(s: &str) -> String {
    let re = regex::Regex::new(r"\[auto-ingest[^\]]*\]").unwrap();
    let s = re.replace_all(s, " ");
    let ws = regex::Regex::new(r"\s+").unwrap();
    ws.replace_all(&s.to_lowercase(), " ").trim().to_string()
}

fn dedup_find_existing(state: &AppState, content: &str, domain: &str, level: i64) -> Option<(crate::store::Fact, f64)> {
    let facts = state.store.get_domain_facts(domain).ok()?;
    let nc = norm_fact_text(content);
    if nc.is_empty() {
        return None;
    }
    let mut best: Option<(crate::store::Fact, f64)> = None;
    let mut best_ratio = 0.0f64;
    for f in facts {
        if f.is_stale || f.is_archived || f.level != level {
            continue;
        }
        let nf = norm_fact_text(&f.content);
        if nf.is_empty() {
            continue;
        }
        if nf == nc {
            return Some((f, 1.0));
        }
        let ln = nf.chars().count() as f64;
        let lc = nc.chars().count() as f64;
        if (ln - lc).abs() / ln.max(lc).max(1.0) > 0.15 {
            continue;
        }
        let r = seqmatch::ratio(&nc, &nf);
        if r > best_ratio {
            best = Some((f, r));
            best_ratio = r;
        }
    }
    if best_ratio >= 0.90 {
        best
    } else {
        None
    }
}

fn add_fact_tool(state: &AppState, args: &Value) -> Value {
    let content = match arg_str(args, "content") {
        Some(c) => c,
        None => return err_value("Input validation error: 'content' is a required property"),
    };
    let level = arg_i64(args, "level", 0);
    let domain = arg_str(args, "domain");
    let module = arg_str(args, "module");
    let code_ref = arg_str(args, "code_ref");
    let parent_id = arg_str(args, "parent_id");
    let ttl_days = args.get("ttl_days").and_then(|v| v.as_i64());

    // WRITE-RULES gate #48
    let domain = match domain {
        Some(d) if !d.is_empty() => d,
        _ => return json!({"status": "rejected", "reason": "domain обязателен (WRITE-RULES)"}),
    };
    let bad_re = regex::Regex::new(
        r"(?i)^(SESSION-END|PRE-COMPACT|POST-COMPACT|HANDOFF|CONTEXT-PREP|SUBAGENT-RESULT|HEARTBEAT)",
    )
    .unwrap();
    if let Some(c) = bad_re.captures(content.trim()) {
        return json!({
            "status": "rejected",
            "reason": format!("протоколы сессий запрещены (WRITE-RULES): {}", &c[1]),
        });
    }

    // Dedup gate #49
    if let Some((dup, sim)) = dedup_find_existing(state, content, domain, level) {
        let _ = state.store.touch_valid_from(&dup.id);
        return json!({
            "status": "duplicate",
            "fact_id": dup.id,
            "similarity": py_round(sim, 3),
            "reason": "совпадает с живым фактом, valid_from обновлён (dedup-gate #49)",
        });
    }

    if !(0..=3).contains(&level) {
        return err_value(format!("{level} is not a valid MemoryLevel"));
    }

    // #31: ttl_days=0 must create a TTL (not be treated as falsy)
    let ttl_config = ttl_days.map(|d| TtlConfig {
        ttl_seconds: d * 24 * 3600,
        refresh_trigger: None,
        on_expire: "mark_stale".into(),
    });

    match state.store.add_fact(content, level, Some(domain), module, code_ref, parent_id, ttl_config, None, 1.0, "manual", None) {
        Ok(fact_id) => json!({
            "status": "success",
            "fact_id": fact_id,
            "content": content,
            "level": level_name(level),
            "domain": domain,
            "module": module,
        }),
        Err(e) => err_value(e),
    }
}

// ---- rlm_diff (#42) ----

fn parse_since(since: &str) -> Result<String, String> {
    let s = since.trim();
    let re = regex::Regex::new(r"^(\d+)\s*([hdw])$").unwrap();
    if let Some(c) = re.captures(&s.to_lowercase()) {
        let n: i64 = c[1].parse().map_err(|_| "bad number".to_string())?;
        let delta = match &c[2] {
            "h" => chrono::Duration::hours(n),
            "d" => chrono::Duration::days(n),
            _ => chrono::Duration::weeks(n),
        };
        return Ok((chrono::Local::now() - delta).format("%Y-%m-%dT%H:%M:%S%.6f").to_string());
    }
    match util::parse_iso(s) {
        Some(dt) => Ok(dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
        None => Err(format!(
            "Invalid 'since': '{since}'. Use ISO date/datetime or relative like '24h', '7d', '2w'."
        )),
    }
}

fn diff_tool(state: &AppState, args: &Value) -> Value {
    let since = match arg_str(args, "since") {
        Some(s) => s,
        None => return err_value("Input validation error: 'since' is a required property"),
    };
    let include_archived = arg_bool(args, "include_archived", false);
    let limit = arg_i64(args, "limit", 200).clamp(1, 1000);
    let offset = arg_i64(args, "offset", 0).max(0);
    let as_of = arg_str(args, "as_of").unwrap_or("");

    let cutoff = match parse_since(since) {
        Ok(c) => c,
        Err(e) => return err_value(e),
    };

    let arch = if !as_of.is_empty() {
        let asof = match util::parse_iso(as_of.trim()) {
            Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
            None => return err_value(format!("Invalid 'as_of': '{as_of}' (use ISO date)")),
        };
        format!(" AND valid_from <= '{asof}' AND (valid_until IS NULL OR valid_until > '{asof}')")
    } else if include_archived {
        String::new()
    } else {
        " AND is_archived = 0".to_string()
    };

    let result = state.store.with_conn(|conn| {
        let fetch = |sql: &str| -> anyhow::Result<Vec<Value>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map(rusqlite::params![cutoff], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>("id")?,
                        "level": r.get::<_, i64>("level")?,
                        "domain": r.get::<_, Option<String>>("domain")?,
                        "created_at": r.get::<_, String>("created_at")?,
                        "stale": r.get::<_, i64>("is_stale")? != 0,
                        "archived": r.get::<_, i64>("is_archived")? != 0,
                        "content": char_slice(&r.get::<_, String>("content")?, 0, 160),
                    }))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        };
        let staled = fetch(&format!(
            "SELECT * FROM hierarchical_facts WHERE is_stale = 1 AND COALESCE(valid_until, created_at) >= ?1{arch}"
        ))?;
        let staled_ids: std::collections::HashSet<String> =
            staled.iter().map(|r| r["id"].as_str().unwrap_or("").to_string()).collect();
        let new_all = fetch(&format!("SELECT * FROM hierarchical_facts WHERE created_at >= ?1{arch}"))?;
        let new: Vec<Value> = new_all
            .into_iter()
            .filter(|r| !staled_ids.contains(r["id"].as_str().unwrap_or("")))
            .collect();
        let updated = fetch(&format!(
            "SELECT * FROM hierarchical_facts WHERE valid_from > created_at AND valid_from >= ?1{arch}"
        ))?;
        Ok((staled, new, updated))
    });

    let (staled, new, updated) = match result {
        Ok(r) => r,
        Err(e) => return err_value(e),
    };

    let mut tagged: Vec<(&str, Value)> = Vec::new();
    for r in staled.iter() {
        tagged.push(("staled", r.clone()));
    }
    for r in new.iter() {
        tagged.push(("new", r.clone()));
    }
    for r in updated.iter() {
        tagged.push(("updated", r.clone()));
    }
    tagged.sort_by(|a, b| {
        b.1["created_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a.1["created_at"].as_str().unwrap_or(""))
    });
    let total = tagged.len();
    let page: Vec<(&str, Value)> = tagged
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    let mut groups = json!({"new": [], "updated": [], "staled": []});
    for (kind, row) in &page {
        groups[*kind].as_array_mut().unwrap().push(row.clone());
    }

    json!({
        "status": "success",
        "since": cutoff,
        "as_of": if as_of.is_empty() { Value::Null } else { json!(as_of) },
        "total_changes": total,
        "returned": page.len(),
        "offset": offset,
        "has_more": offset + limit < total as i64,
        "counts": {"new": new.len(), "updated": updated.len(), "staled": staled.len()},
        "groups": groups,
    })
}