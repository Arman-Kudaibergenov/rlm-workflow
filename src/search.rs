//! rlm_search_facts — port of _patch_search_facts (#19/#36/#38).

use serde_json::{json, Value};

use crate::embed::Embedder;
use crate::store::Store;
use crate::util::{self, cosine, is_noise_content, jaccard, py_round};
use std::sync::Arc;

pub fn hybrid_search(
    store: &Store,
    embedder: Option<&Arc<Embedder>>,
    query: &str,
    top_k: usize,
    semantic_weight: f64,
    keyword_weight: f64,
    recency_weight: f64,
) -> Value {
    let facts_with_emb = match store.get_facts_with_embeddings() {
        Ok(f) => f,
        Err(e) => return json!({"status": "error", "message": e.to_string()}),
    };
    let all_facts = if facts_with_emb.is_empty() {
        store.get_all_facts(false, false).unwrap_or_default()
    } else {
        Vec::new()
    };

    let query_embedding: Option<Vec<f32>> = embedder.and_then(|e| match e.encode(query) {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!("[#19] Embedding failed: {err}");
            None
        }
    });

    let query_tokens: std::collections::HashSet<String> =
        query.to_lowercase().split_whitespace().map(String::from).collect();
    let now = chrono::Local::now();

    let min_semantic: f64 = std::env::var("RLM_MIN_SEMANTIC")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);
    let min_score: f64 = std::env::var("RLM_MIN_SCORE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.1);

    let mut scored: Vec<(crate::store::Fact, f64)> = Vec::new();

    for (fact, emb) in facts_with_emb.iter() {
        if fact.is_stale || is_noise_content(&fact.content) {
            continue;
        }
        let mut semantic = match (&query_embedding, emb.is_empty()) {
            (Some(q), false) => cosine(q, emb),
            _ => 0.0,
        };
        if semantic < 0.0 {
            semantic = 0.0;
        }
        let fact_tokens: std::collections::HashSet<String> =
            fact.content.to_lowercase().split_whitespace().map(String::from).collect();
        let union_size = query_tokens.union(&fact_tokens).count();
        let inter = query_tokens.intersection(&fact_tokens).count();
        let keyword = inter as f64 / union_size.max(1) as f64;

        // #38 relevance gate: keyword overlap OR strong semantic match
        if keyword == 0.0 && semantic < min_semantic {
            continue;
        }

        let relevance = semantic_weight * semantic + keyword_weight * keyword;
        let age_hours = age_hours(&fact.created_at, &now);
        let recency = (-age_hours / 168.0).exp();
        let score = relevance + recency_weight * recency;
        scored.push((fact.clone(), score));
    }

    // Facts without embeddings (only when there are no embedded facts at all)
    let emb_ids: std::collections::HashSet<&str> = facts_with_emb.iter().map(|(f, _)| f.id.as_str()).collect();
    for fact in &all_facts {
        if fact.is_stale || is_noise_content(&fact.content) {
            continue;
        }
        if emb_ids.contains(fact.id.as_str()) {
            continue;
        }
        let fact_tokens: std::collections::HashSet<String> =
            fact.content.to_lowercase().split_whitespace().map(String::from).collect();
        let union_size = query_tokens.union(&fact_tokens).count();
        let inter = query_tokens.intersection(&fact_tokens).count();
        let keyword = inter as f64 / union_size.max(1) as f64;
        if keyword == 0.0 {
            continue;
        }
        let relevance = keyword_weight * keyword;
        let age_hours = age_hours(&fact.created_at, &now);
        let recency = (-age_hours / 168.0).exp();
        let score = relevance + recency_weight * recency;
        scored.push((fact.clone(), score));
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let results: Vec<Value> = scored
        .into_iter()
        .take(top_k)
        .filter(|(_, s)| *s >= min_score)
        .map(|(fact, score)| {
            json!({
                "fact_id": fact.id,
                "content": fact.content,
                "entity_type": fact.domain.clone().unwrap_or_else(|| "fact".to_string()),
                "score": py_round(score, 4),
            })
        })
        .collect();

    json!({
        "status": "success",
        "query": query,
        "results": results,
    })
}

pub fn age_hours(created_at: &str, now: &chrono::DateTime<chrono::Local>) -> f64 {
    match util::parse_iso(created_at) {
        Some(c) => (*now - c).num_milliseconds() as f64 / 3_600_000.0,
        None => 0.0,
    }
}

#[allow(dead_code)]
fn unused_jaccard(a: &str, b: &str) -> f64 {
    jaccard(a, b)
}