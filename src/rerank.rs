//! Reranker client (#43) with circuit-breaker cooldown.

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Reranker {
    pub enabled: bool,
    pub url: String,
    pub timeout: Duration,
    pub cooldown: Duration,
    client: reqwest::blocking::Client,
    cooldown_until: Mutex<Option<Instant>>,
}

impl Reranker {
    pub fn from_env() -> Reranker {
        let enabled = std::env::var("RERANK_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase();
        let enabled = !matches!(enabled.as_str(), "0" | "false" | "no");
        let url = std::env::var("RERANK_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8400".to_string())
            .trim_end_matches('/')
            .to_string();
        let timeout = std::env::var("RERANK_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(3.0);
        Reranker {
            enabled,
            url,
            timeout: Duration::from_secs_f64(timeout),
            cooldown: Duration::from_secs(120),
            client: reqwest::blocking::Client::new(),
            cooldown_until: Mutex::new(None),
        }
    }

    /// -> Some([(index, score)]) or None if unavailable/disabled/cooling down.
    pub fn call(&self, query: &str, documents: &[String], top_k: usize) -> Option<Vec<(usize, f64)>> {
        if !self.enabled || documents.is_empty() {
            return None;
        }
        {
            let guard = self.cooldown_until.lock().unwrap();
            if let Some(until) = *guard {
                if Instant::now() < until {
                    return None;
                }
            }
        }
        let result = self
            .client
            .post(format!("{}/rerank", self.url))
            .timeout(self.timeout)
            .json(&serde_json::json!({"query": query, "documents": documents, "top_k": top_k}))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json::<serde_json::Value>());
        match result {
            Ok(data) => {
                let mut out = Vec::new();
                if let Some(results) = data.get("results").and_then(|r| r.as_array()) {
                    for item in results {
                        let idx = item.get("index").and_then(|i| i.as_i64()).unwrap_or(-1);
                        if idx >= 0 && (idx as usize) < documents.len() {
                            let score = item.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
                            out.push((idx as usize, score));
                        }
                    }
                }
                Some(out)
            }
            Err(e) => {
                *self.cooldown_until.lock().unwrap() = Some(Instant::now() + self.cooldown);
                tracing::warn!("[#43] reranker unavailable ({e:?}) — RRF order kept, cooldown 120s");
                None
            }
        }
    }
}