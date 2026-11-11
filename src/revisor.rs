//! Autonomous freshness revisor (#47) — port of _patch_revisor.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Datelike;
use serde_json::{json, Value};

use crate::store::Store;
use crate::util::{char_slice, now_iso, parse_iso};

const DEFAULT_TTL_DAYS: i64 = 90;
const MAX_INVALIDATE: usize = 20;

fn default_ttl_map() -> HashMap<String, i64> {
    HashMap::from([
        ("work_current".to_string(), 7),
        ("tasks".to_string(), 30),
        ("project_status".to_string(), 30),
        ("infrastructure".to_string(), 90),
        ("deploy".to_string(), 30),
        ("deployment".to_string(), 30),
    ])
}

fn default_never() -> HashSet<String> {
    ["pitfalls", "decisions", "security", "licensing", "reference", "causal", "revisor"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

struct RevisorConfig {
    ttl: HashMap<String, i64>,
    never: HashSet<String>,
    default_ttl: i64,
}

fn load_config() -> RevisorConfig {
    let mut cfg = RevisorConfig { ttl: default_ttl_map(), never: default_never(), default_ttl: DEFAULT_TTL_DAYS };
    let text = match std::fs::read_to_string("/data/revisor.yaml") {
        Ok(t) => t,
        Err(_) => return cfg,
    };
    if let Ok(data) = serde_yaml::from_str::<serde_yaml::Value>(&text) {
        if let Some(ttl) = data.get("ttl_days").and_then(|v| v.as_mapping()) {
            for (k, v) in ttl {
                if let (Some(k), Some(v)) = (k.as_str(), v.as_i64()) {
                    cfg.ttl.insert(k.to_string(), v);
                }
            }
        }
        if let Some(never) = data.get("never").and_then(|v| v.as_sequence()) {
            for item in never {
                if let Some(s) = item.as_str() {
                    cfg.never.insert(s.to_string());
                }
            }
        }
        if let Some(d) = data.get("default_ttl_days").and_then(|v| v.as_i64()) {
            cfg.default_ttl = d;
        }
    }
    cfg
}

/// Probe URLs (HTTP HEAD) and host:port (TCP) in fact content. Any alive -> true.
fn check_endpoints(content: &str) -> (Option<bool>, Vec<String>) {
    let url_re = regex::Regex::new(r#"https?://[^\s"'<>\)\]]+"#).unwrap();
    let hp_re = regex::Regex::new(r"\b((?:\d{1,3}\.){3}\d{1,3}):(\d{2,5})\b").unwrap();
    let mut evidence = Vec::new();
    let mut alive_any = false;
    let mut dead_any = false;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok();

    for m in url_re.find_iter(content).take(3) {
        let u = m.as_str();
        let alive = match &client {
            Some(c) => match c.head(u).send() {
                Ok(_) => true,
                Err(e) if e.is_status() => true, // host responded — alive
                Err(_) => false,
            },
            None => false,
        };
        if alive {
            alive_any = true;
            evidence.push(format!("alive: {u}"));
        } else {
            dead_any = true;
            evidence.push(format!("dead: {u}"));
        }
    }
    for c in hp_re.captures_iter(content).take(3) {
        let h = &c[1];
        let p: u16 = match c[2].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let addr = format!("{h}:{p}");
        let alive = std::net::TcpStream::connect_timeout(
            &addr.to_socket_addrs_ok(),
            Duration::from_secs(2),
        )
        .is_ok();
        if alive {
            alive_any = true;
            evidence.push(format!("alive: {h}:{p}"));
        } else {
            dead_any = true;
            evidence.push(format!("dead: {h}:{p}"));
        }
    }
    if alive_any {
        (Some(true), evidence)
    } else if dead_any {
        (Some(false), evidence)
    } else {
        (None, evidence)
    }
}

trait ToSocketAddrsOk {
    fn to_socket_addrs_ok(&self) -> std::net::SocketAddr;
}
impl ToSocketAddrsOk for String {
    fn to_socket_addrs_ok(&self) -> std::net::SocketAddr {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], 0)))
    }
}

fn llm_verdict(content: &str, llm_url: &str, llm_model: &str) -> (Option<String>, String) {
    if llm_url.is_empty() {
        return (None, String::new());
    }
    let today = chrono::Local::now().format("%Y-%m-%d");
    let prompt = format!(
        "You audit a knowledge-base fact for freshness. Today is {today}. Reply with exactly one word — STALE, CURRENT, or DEBATABLE — then a dash and a one-line reason.\n\nFACT:\n{}",
        char_slice(content, 0, 1500)
    );
    let client = match reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build() {
        Ok(c) => c,
        Err(e) => return (None, format!("llm unavailable: {e}")),
    };
    let resp = client
        .post(format!("{llm_url}/chat/completions"))
        .json(&json!({
            "model": llm_model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0,
            "max_tokens": 200,
            "stream": false,
        }))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json::<Value>());
    let text = match resp {
        Ok(v) => v
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        Err(e) => return (None, format!("llm unavailable: {e}")),
    };
    let think_re = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
    let text = think_re.replace_all(&text, "").trim().to_string();
    let low = text.to_lowercase();
    let short = char_slice(&text, 0, 120);
    if low.starts_with("stale") {
        (Some("stale".into()), short)
    } else if low.starts_with("current") {
        (Some("current".into()), short)
    } else {
        (Some("debatable".into()), short)
    }
}

pub struct Revisor {
    pub store: Arc<Store>,
    pub mode_default: String,
    pub llm_url: String,
    pub llm_model: String,
}

impl Revisor {
    pub fn from_env(store: Arc<Store>) -> Revisor {
        Revisor {
            store,
            mode_default: std::env::var("REVISOR_MODE").unwrap_or_else(|_| "report".into()).to_lowercase(),
            llm_url: std::env::var("REVISOR_LLM_URL").unwrap_or_default().trim_end_matches('/').to_string(),
            llm_model: std::env::var("REVISOR_LLM_MODEL").unwrap_or_else(|_| "qwen3:8b".into()),
        }
    }

    pub fn run(&self, deep: bool, mode: &str) -> Result<Value> {
        let mode = if mode.is_empty() { self.mode_default.clone() } else { mode.to_lowercase() };
        let cfg = load_config();
        let now = chrono::Local::now();

        let rows: Vec<(String, String, Option<String>, i64, String)> = self.store.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, content, domain, level, created_at FROM hierarchical_facts
                 WHERE is_stale = 0 AND is_archived = 0 AND valid_until IS NULL",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

        let mut candidates: Vec<((String, String, Option<String>, i64, String), i64)> = Vec::new();
        for r in &rows {
            let d = r.2.clone().unwrap_or_default().trim().to_string();
            if cfg.never.contains(&d) {
                continue;
            }
            let ttl = *cfg.ttl.get(&d).unwrap_or(&cfg.default_ttl);
            let created = match parse_iso(&r.4) {
                Some(c) => c,
                None => continue,
            };
            if (now - created).num_days() >= ttl {
                candidates.push((r.clone(), ttl));
            }
        }

        // concurrent endpoint probes (16 workers like the Python ThreadPoolExecutor)
        let probe_results: Vec<(Option<bool>, Vec<String>)> = {
            let chunks: Vec<Vec<String>> = candidates.chunks(16).map(|c| c.iter().map(|(r, _)| r.1.clone()).collect()).collect();
            let mut handles = Vec::new();
            for chunk in chunks {
                handles.push(std::thread::spawn(move || {
                    chunk.iter().map(|c| check_endpoints(c)).collect::<Vec<_>>()
                }));
            }
            let mut out = Vec::new();
            for h in handles {
                out.extend(h.join().unwrap_or_default());
            }
            out
        };

        let mut decisions: Vec<Value> = Vec::new();
        let mut ping_alive_n = 0i64;
        let mut evidence_log: Vec<String> = Vec::new();
        let mut applied = 0usize;

        for ((r, ttl), (alive, ev)) in candidates.iter().zip(probe_results.iter()) {
            evidence_log.extend(ev.clone());
            if *alive == Some(true) {
                ping_alive_n += 1;
                continue;
            }
            let reason = if *alive == Some(false) {
                format!("endpoint dead: {}", ev.last().cloned().unwrap_or_else(|| "unreachable".into()))
            } else if deep {
                let (verdict, why) = llm_verdict(&r.1, &self.llm_url, &self.llm_model);
                if verdict.as_deref() == Some("stale") {
                    format!("llm stale: {why}")
                } else {
                    continue;
                }
            } else {
                format!("ttl {ttl}d exceeded (domain={})", r.2.clone().unwrap_or_else(|| "-".into()))
            };

            let mut action = "would_invalidate";
            if mode == "auto" {
                if applied < MAX_INVALIDATE {
                    let _ = self.store.mark_stale(&r.0);
                    applied += 1;
                    action = "invalidated";
                } else {
                    action = "over_limit";
                }
            }
            decisions.push(json!({
                "id": r.0, "reason": reason, "action": action,
                "domain": r.2, "created_at": r.4,
            }));
        }

        let report = json!({
            "status": "success",
            "date": now.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "mode": mode,
            "deep": deep,
            "live_facts": rows.len(),
            "candidates": candidates.len(),
            "invalidated": applied,
            "ping_alive_skipped": ping_alive_n,
            "decisions": decisions.iter().take(MAX_INVALIDATE).cloned().collect::<Vec<_>>(),
            "evidence": evidence_log.iter().take(20).cloned().collect::<Vec<_>>(),
        });

        // Report fact into RLM itself (level 1, domain 'revisor')
        let mut lines = vec![format!(
            "REVISOR RUN {} | mode={} deep={} | live={} candidates={} invalidated={} ping-alive={}",
            report["date"].as_str().unwrap_or(""),
            mode,
            deep,
            rows.len(),
            candidates.len(),
            applied,
            ping_alive_n
        )];
        for d in decisions.iter().take(MAX_INVALIDATE) {
            lines.push(format!(
                "- [{}] {} ({}) {}",
                d["action"].as_str().unwrap_or(""),
                char_slice(d["id"].as_str().unwrap_or(""), 0, 8),
                d["domain"].as_str().unwrap_or(""),
                char_slice(d["reason"].as_str().unwrap_or(""), 0, 100)
            ));
        }
        if let Err(e) = self.store.add_fact(&lines.join("\n"), 1, Some("revisor"), None, None, None, None, None, 1.0, "revisor", None) {
            tracing::warn!("[#47] failed to store report fact: {e}");
        }
        Ok(report)
    }
}

/// Scheduler: daily 04:17 local (quick), Sunday 04:17 deep (+LLM).
pub async fn scheduler_loop(revisor: Arc<Revisor>) {
    loop {
        let now = chrono::Local::now();
        let mut next = now
            .date_naive()
            .and_hms_opt(4, 17, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap_or(now);
        if next <= now {
            next += chrono::Duration::days(1);
        }
        let secs = (next - now).num_seconds().max(1) as u64;
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let deep = chrono::Local::now().weekday() == chrono::Weekday::Sun;
        let rev = revisor.clone();
        let result = tokio::task::spawn_blocking(move || rev.run(deep, "")).await;
        match result {
            Ok(Ok(rep)) => tracing::info!(
                "[#47] scheduled run: mode={} deep={} candidates={} invalidated={}",
                rep["mode"], rep["deep"], rep["candidates"], rep["invalidated"]
            ),
            Ok(Err(e)) => tracing::error!("[#47] scheduled run failed: {e}"),
            Err(e) => tracing::error!("[#47] scheduled run join failed: {e}"),
        }
    }
}

#[allow(dead_code)]
fn unused_now() -> String {
    now_iso()
}