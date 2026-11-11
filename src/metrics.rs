//! Lightweight in-process metrics + health (#6).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};

pub struct Metrics {
    pub start: Instant,
    requests: AtomicU64,
    tool_calls: AtomicU64,
    tool_errors: AtomicU64,
    latency_sum_us: AtomicU64,
    by_tool: Mutex<HashMap<String, u64>>,
}

impl Metrics {
    pub fn new() -> Metrics {
        Metrics {
            start: Instant::now(),
            requests: AtomicU64::new(0),
            tool_calls: AtomicU64::new(0),
            tool_errors: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            by_tool: Mutex::new(HashMap::new()),
        }
    }

    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tool(&self, name: &str, is_error: bool, latency_us: u64) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
        if is_error {
            self.tool_errors.fetch_add(1, Ordering::Relaxed);
        }
        self.latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);
        *self.by_tool.lock().unwrap().entry(name.to_string()).or_insert(0) += 1;
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }

    pub fn snapshot(&self) -> Value {
        let calls = self.tool_calls.load(Ordering::Relaxed);
        let avg_us = if calls > 0 {
            self.latency_sum_us.load(Ordering::Relaxed) / calls
        } else {
            0
        };
        let by_tool = self.by_tool.lock().unwrap().clone();
        json!({
            "uptime_secs": self.uptime_secs(),
            "requests_total": self.requests.load(Ordering::Relaxed),
            "tool_calls_total": calls,
            "tool_errors_total": self.tool_errors.load(Ordering::Relaxed),
            "tool_avg_latency_us": avg_us,
            "tool_calls_by_name": by_tool,
        })
    }

    /// Prometheus-style text exposition.
    pub fn prometheus(&self) -> String {
        let calls = self.tool_calls.load(Ordering::Relaxed);
        let avg_us = if calls > 0 { self.latency_sum_us.load(Ordering::Relaxed) / calls } else { 0 };
        let mut out = String::new();
        out.push_str(&format!("# TYPE rlm_uptime_seconds gauge\nrlm_uptime_seconds {}\n", self.uptime_secs()));
        out.push_str(&format!("# TYPE rlm_requests_total counter\nrlm_requests_total {}\n", self.requests.load(Ordering::Relaxed)));
        out.push_str(&format!("# TYPE rlm_tool_calls_total counter\nrlm_tool_calls_total {}\n", calls));
        out.push_str(&format!("# TYPE rlm_tool_errors_total counter\nrlm_tool_errors_total {}\n", self.tool_errors.load(Ordering::Relaxed)));
        out.push_str(&format!("# TYPE rlm_tool_avg_latency_microseconds gauge\nrlm_tool_avg_latency_microseconds {}\n", avg_us));
        for (name, n) in self.by_tool.lock().unwrap().iter() {
            out.push_str(&format!("rlm_tool_calls_by_name{{tool=\"{}\"}} {}\n", name, n));
        }
        out
    }
}