//! Semantic router — port of _hybrid_route (#43/#44/#45/#46) and the
//! freshness formatter (#42) from start_server.py.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use crate::embed::Embedder;
use crate::profiles;
use crate::rerank::Reranker;
use crate::store::{Fact, Store};
use crate::util::{self, char_slice, cosine, date_prefix_re, fact_date, is_noise_content, py_round, query_tokens};

const RRF_K: f64 = 60.0;
const TOP_N: usize = 50;
const RERANK_TOP: usize = 10;

pub struct RoutingResult {
    pub facts: Vec<Fact>,
    pub total_tokens: i64,
    pub routing_confidence: f64,
    pub routing_explanation: String,
    pub domains_loaded: Vec<String>,
    pub fallback_used: bool,
    pub reranked: bool,
    pub profile: String,
    pub profile_warning: Option<String>,
    pub as_of: Option<String>,
}

pub struct Router {
    pub store: Arc<Store>,
    pub embedder: Option<Arc<Embedder>>,
    pub reranker: Arc<Reranker>,
    pub similarity_threshold: f64,
    pub max_tokens: i64,
}

impl Router {
    pub fn new(store: Arc<Store>, embedder: Option<Arc<Embedder>>, reranker: Arc<Reranker>) -> Router {
        // external embedding providers get a lower threshold (start_server.py #5)
        let provider = std::env::var("RLM_EMBEDDING_PROVIDER").unwrap_or_default().to_lowercase();
        let threshold = if provider == "ollama" || provider == "openai" { 0.15 } else { 0.5 };
        Router { store, embedder, reranker, similarity_threshold: threshold, max_tokens: 2000 }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn route(
        &self,
        query: &str,
        max_tokens: Option<i64>,
        include_stale: bool,
        include_l0: bool,
        target_domains: Option<&HashSet<String>>,
        profile: Option<&str>,
        as_of: Option<&str>,
    ) -> Result<RoutingResult> {
        let max_tokens = max_tokens.unwrap_or(self.max_tokens);
        let mut selected_facts: Vec<Fact> = Vec::new();
        let mut total_tokens: i64 = 0;
        let mut domains_loaded: Vec<String> = Vec::new();
        let mut explanations: Vec<String> = Vec::new();
        let mut reranked = false;

        // Agent loadout (#44)
        let mut profile_domains: Option<HashSet<&str>> = None;
        let mut profile_warning: Option<String> = None;
        if let Some(p) = profile {
            if !p.is_empty() && p != "default" {
                match profiles::get(p) {
                    None => {
                        profile_warning = Some(format!(
                            "unknown profile '{p}' — default routing used; see rlm_list_profiles"
                        ));
                    }
                    Some(prof) => {
                        if !prof.domains.is_empty() {
                            profile_domains = Some(prof.domains.iter().copied().collect());
                        }
                    }
                }
            }
        }

        // Bi-temporal (#45)
        let mut as_of_iso: Option<String> = None;
        if let Some(a) = as_of {
            if !a.is_empty() {
                match util::parse_iso(a) {
                    Some(dt) => as_of_iso = Some(dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
                    None => {
                        let msg = format!("invalid as_of '{a}' — ignored");
                        profile_warning = Some(match profile_warning {
                            Some(w) => format!("{w}; {msg}"),
                            None => msg,
                        });
                    }
                }
            }
        }

        // Step 1: L0 facts always
        if include_l0 {
            let l0_facts = if let Some(iso) = &as_of_iso {
                self.store
                    .get_facts_by_level(0, None, true, true)?
                    .into_iter()
                    .filter(|f| f.valid_at(iso))
                    .collect::<Vec<_>>()
            } else {
                self.store.get_facts_by_level(0, None, include_stale, false)?
            };
            let l0_budget = max_tokens as f64 * 0.3;
            for fact in &l0_facts {
                let tokens = fact.token_estimate();
                if (total_tokens + tokens) as f64 <= l0_budget {
                    selected_facts.push(fact.clone());
                    total_tokens += tokens;
                }
            }
            explanations.push(format!("Loaded {} L0 project facts", l0_facts.len()));
        }

        let embedder = match &self.embedder {
            Some(e) => e,
            None => anyhow::bail!("no embedding service available"),
        };
        let query_embedding = embedder.encode(query)?;
        let facts_with_embeddings = self.store.get_facts_with_embeddings()?;

        if facts_with_embeddings.is_empty() {
            // Upstream fallback: no embeddings at all -> first 10 L1 facts
            let l1_facts = self.store.get_facts_by_level(1, None, include_stale, false)?;
            for fact in l1_facts.iter().take(10) {
                let tokens = fact.token_estimate();
                if total_tokens + tokens <= max_tokens {
                    selected_facts.push(fact.clone());
                    total_tokens += tokens;
                    if let Some(d) = &fact.domain {
                        if !domains_loaded.contains(d) {
                            domains_loaded.push(d.clone());
                        }
                    }
                }
            }
            return Ok(RoutingResult {
                facts: selected_facts,
                total_tokens,
                routing_confidence: 0.5,
                routing_explanation: format!("Fallback routing (no embeddings): {}", explanations.join("; ")),
                domains_loaded,
                fallback_used: true,
                reranked: false,
                profile: profile.filter(|p| !p.is_empty()).unwrap_or("default").to_string(),
                profile_warning,
                as_of: as_of_iso.clone(),
            });
        }

        // Candidate pool
        let candidates = if let Some(iso) = &as_of_iso {
            self.store.get_facts_with_embeddings_as_of(iso)?
        } else {
            facts_with_embeddings
        };
        let mut pool: Vec<(Fact, Vec<f32>)> = Vec::new();
        for (fact, emb) in candidates {
            if fact.level == 0 {
                continue;
            }
            if as_of_iso.is_none() {
                if fact.is_archived {
                    continue;
                }
                if !include_stale && fact.is_stale {
                    continue;
                }
            }
            if let Some(td) = target_domains {
                if let Some(d) = &fact.domain {
                    if !td.contains(d) {
                        continue;
                    }
                }
            }
            pool.push((fact, emb));
        }
        let pool_by_id: HashMap<String, Fact> = pool.iter().map(|(f, _)| (f.id.clone(), f.clone())).collect();

        // Vector ranking
        let mut sims: HashMap<String, f64> = HashMap::new();
        let mut vec_ranked: Vec<(String, f64)> = Vec::new();
        for (fact, emb) in &pool {
            let sim = cosine(&query_embedding, emb);
            if sim >= self.similarity_threshold {
                sims.insert(fact.id.clone(), sim);
                vec_ranked.push((fact.id.clone(), sim));
            }
        }
        vec_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let vec_top: Vec<String> = vec_ranked.into_iter().take(TOP_N).map(|(id, _)| id).collect();

        // FTS ranking (tiered, #46)
        let (fts_raw, rare_n) = self.store.fts_search(query, TOP_N);
        let rare_ids: HashSet<String> = fts_raw.iter().take(rare_n).cloned().collect();
        let fts_ids: Vec<String> = fts_raw.into_iter().filter(|id| pool_by_id.contains_key(id)).collect();

        // Weighted RRF fusion
        let w_fts = if query_tokens(query).len() == 1 { 2.0 } else { 1.0 };
        let mut rrf: HashMap<String, f64> = HashMap::new();
        for (rank, id) in vec_top.iter().enumerate() {
            *rrf.entry(id.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64);
        }
        for (rank, id) in fts_ids.iter().enumerate() {
            let w = w_fts * if rare_ids.contains(id) { 1.5 } else { 1.0 };
            *rrf.entry(id.clone()).or_insert(0.0) += w / (RRF_K + rank as f64);
        }
        explanations.push(format!(
            "hybrid: {} vector + {} fts candidates (w_fts={}) -> rrf {}",
            vec_top.len(),
            fts_ids.len(),
            w_fts,
            rrf.len()
        ));

        // Profile boost (#44)
        if let Some(pd) = &profile_domains {
            for (fid, score) in rrf.iter_mut() {
                let in_profile = pool_by_id.get(fid).and_then(|f| f.domain.as_deref()).map(|d| pd.contains(d)).unwrap_or(false);
                *score *= if in_profile { 2.0 } else { 0.5 };
            }
            explanations.push(format!("profile={}: {} domains boosted", profile.unwrap_or(""), pd.len()));
        }

        // Level weight (#46)
        for (fid, score) in rrf.iter_mut() {
            if let Some(f) = pool_by_id.get(fid) {
                let w = match f.level {
                    2 => 1.15,
                    3 => 1.25,
                    _ => 1.0,
                };
                *score *= w;
            }
        }

        let mut ordered_ids: Vec<String> = rrf.keys().cloned().collect();
        // stable sort desc by score (Python sorted is stable)
        ordered_ids.sort_by(|a, b| rrf[b].partial_cmp(&rrf[a]).unwrap_or(std::cmp::Ordering::Equal));
        let mut ordered: Vec<Fact> = ordered_ids.iter().filter_map(|id| pool_by_id.get(id).cloned()).collect();

        // Rerank RRF top-10 (#43/#47 window docs)
        let q_tokens = query_tokens(query);
        if !ordered.is_empty() {
            let head: Vec<Fact> = ordered.iter().take(RERANK_TOP).cloned().collect();
            let tail: Vec<Fact> = ordered.iter().skip(RERANK_TOP).cloned().collect();
            let docs: Vec<String> = head.iter().map(|f| rerank_doc(&f.content, &q_tokens)).collect();
            if let Some(rr) = self.reranker.call(query, &docs, RERANK_TOP) {
                let mut new_ordered: Vec<Fact> = rr.iter().filter_map(|(i, _)| head.get(*i).cloned()).collect();
                new_ordered.extend(tail);
                ordered = new_ordered;
                reranked = true;
            }
        }

        // Token budget in final order
        for fact in &ordered {
            let tokens = fact.token_estimate();
            if total_tokens + tokens <= max_tokens {
                selected_facts.push(fact.clone());
                total_tokens += tokens;
                if let Some(d) = &fact.domain {
                    if !domains_loaded.contains(d) {
                        domains_loaded.push(d.clone());
                    }
                }
            }
        }

        let selected_sims: Vec<f64> = selected_facts.iter().filter_map(|f| sims.get(&f.id).copied()).collect();
        let confidence = if !selected_sims.is_empty() {
            let avg = selected_sims.iter().sum::<f64>() / selected_sims.len() as f64;
            (avg * 1.2).min(1.0)
        } else {
            0.5
        };

        let l0_selected = selected_facts.iter().filter(|f| f.level == 0).count();
        explanations.push(format!(
            "Loaded {} additional facts from {} domains",
            selected_facts.len() - l0_selected,
            domains_loaded.len()
        ));

        let _cross_refs = resolve_cross_references(&selected_facts, 5);

        Ok(RoutingResult {
            facts: selected_facts,
            total_tokens,
            routing_confidence: py_round(confidence, 3),
            routing_explanation: explanations.join("; "),
            domains_loaded,
            fallback_used: false,
            reranked,
            profile: profile.filter(|p| !p.is_empty()).unwrap_or("default").to_string(),
            profile_warning,
            as_of: as_of_iso,
        })
    }

    /// rlm_route_context response (#44 _route_fn).
    pub fn route_tool_response(&self, r: &RoutingResult) -> Value {
        let formatted = format_context_for_injection(r);
        let mut resp = json!({
            "status": "success",
            "facts_count": r.facts.len(),
            "total_tokens": r.total_tokens,
            "routing_confidence": r.routing_confidence,
            "routing_explanation": r.routing_explanation,
            "domains_loaded": r.domains_loaded,
            "fallback_used": r.fallback_used,
            "reranked": r.reranked,
            "profile": r.profile,
            "context": formatted,
        });
        if let Some(a) = &r.as_of {
            resp["as_of"] = json!(a);
        }
        if let Some(w) = &r.profile_warning {
            resp["warning"] = json!(w);
        }
        resp
    }
}

/// head + window around the first query-token occurrence past char 256 (#47).
fn rerank_doc(content: &str, q_tokens: &[String]) -> String {
    let head = char_slice(content, 0, 256);
    let low = content.to_lowercase();
    let mut best: Option<usize> = None;
    for t in q_tokens {
        if let Some(i) = low.find(t.as_str()) {
            let char_i = low[..i].chars().count();
            if char_i > 256 && (best.is_none() || char_i < best.unwrap()) {
                best = Some(char_i);
            }
        }
    }
    match best {
        None => head,
        Some(b) => {
            let start = b.saturating_sub(64);
            format!("{} ... {}", head, char_slice(content, start, 256))
        }
    }
}

fn resolve_cross_references(facts: &[Fact], max_additional: usize) -> Vec<(String, String)> {
    let ids: HashSet<&str> = facts.iter().map(|f| f.id.as_str()).collect();
    let mut out = Vec::new();
    for fact in facts {
        if let Some(pid) = &fact.parent_id {
            if !ids.contains(pid.as_str()) {
                out.push((fact.id.clone(), pid.clone()));
            }
        }
        for child in &fact.children_ids {
            if !ids.contains(child.as_str()) {
                out.push((fact.id.clone(), child.clone()));
            }
        }
    }
    out.truncate(max_additional);
    out
}

/// Fact line with freshness prefix (#42): '- [date] [L2] [STALE] [domain] content'
pub fn fmt_fact_line(fact: &Fact, with_domain: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    let date = fact_date(&fact.created_at);
    if !date.is_empty() && !date_prefix_re().is_match(&fact.content) {
        parts.push(format!("[{date}]"));
    }
    if fact.level >= 2 {
        parts.push(format!("[L{}]", fact.level));
    }
    if fact.is_stale {
        parts.push("[STALE]".to_string());
    }
    if with_domain {
        if let Some(d) = &fact.domain {
            parts.push(format!("[{d}]"));
        }
    }
    parts.push(fact.content.clone());
    format!("- {}", parts.join(" "))
}

/// #42 formatter: L0 section + rank-ordered rest, noise filtered, deduped.
pub fn format_context_for_injection(r: &RoutingResult) -> String {
    let mut l0_facts: Vec<&Fact> = Vec::new();
    let mut ranked: Vec<&Fact> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for fact in &r.facts {
        let c = fact.content.trim().to_string();
        if is_noise_content(&fact.content) || seen.contains(&c) {
            continue;
        }
        seen.insert(c);
        if fact.level == 0 {
            l0_facts.push(fact);
        } else {
            ranked.push(fact);
        }
    }

    let mut lines: Vec<String> = Vec::new();
    if !l0_facts.is_empty() {
        lines.push("\n## PROJECT OVERVIEW".to_string());
        for fact in l0_facts {
            lines.push(fmt_fact_line(fact, true));
        }
    }
    if !ranked.is_empty() {
        lines.push("\n## ROUTED FACTS (by relevance)".to_string());
        for fact in ranked {
            lines.push(fmt_fact_line(fact, true));
        }
    }
    lines.push("\n---".to_string());
    lines.push(format!("Routing confidence: {:.0}%", r.routing_confidence * 100.0));
    lines.push(format!("Domains: {}", r.domains_loaded.join(", ")));
    lines.join("\n")
}