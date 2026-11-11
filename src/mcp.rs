//! MCP streamable-HTTP transport — matches FastMCP (mcp SDK 1.27) behavior:
//! JSON responses to POST, Mcp-Session-Id sessions, SSE GET stream, 202 for
//! notifications, 404 for unknown sessions.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::tools::{self, AppState};

type SseTx = tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>;
type SseRx = tokio::sync::mpsc::Receiver<Result<Event, std::convert::Infallible>>;

pub struct StreamEntry {
    // Kept alive so the server->client SSE stream stays open for the session.
    _tx: SseTx,
    rx: Option<SseRx>,
}

pub struct McpState {
    pub app: Arc<AppState>,
    pub sessions: Mutex<HashSet<String>>,
    pub streams: Mutex<HashMap<String, StreamEntry>>,
}

const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

fn jsonrpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub async fn handle_post(State(state): State<Arc<McpState>>, headers: HeaderMap, body: Bytes) -> Response {
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let hdr_sid = session_id(&headers).unwrap_or_default();
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(jsonrpc_error(&Value::Null, -32700, &format!("Parse error: {e}"))),
            )
                .into_response();
        }
    };
    tracing::debug!(
        "[mcp] POST method={} accept='{}' sid={} body_len={}",
        req.get("method").and_then(|m| m.as_str()).unwrap_or("?"),
        accept,
        if hdr_sid.is_empty() { "-" } else { &hdr_sid[..8.min(hdr_sid.len())] },
        body.len()
    );
    state.app.metrics.record_request();

    if req.is_array() {
        return (
            StatusCode::BAD_REQUEST,
            Json(jsonrpc_error(&Value::Null, -32600, "Batch requests are not supported")),
        )
            .into_response();
    }

    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let params = req.get("params").cloned().unwrap_or(json!({}));
    let is_notification = req.get("id").is_none();

    if method == "initialize" {
        let sid = uuid::Uuid::new_v4().to_string();
        state.sessions.lock().unwrap().insert(sid.clone());
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(16);
        state.streams.lock().unwrap().insert(sid.clone(), StreamEntry { _tx: tx, rx: Some(rx) });
        let requested = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-03-26");
        let negotiated = if SUPPORTED_VERSIONS.contains(&requested) { requested } else { "2025-03-26" };
        let resp = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": negotiated,
                "capabilities": {
                    "experimental": {},
                    "prompts": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false},
                    "tools": {"listChanged": false}
                },
                "serverInfo": {"name": "rlm-toolkit", "version": "1.27.0"}
            }
        });
        return (
            StatusCode::OK,
            [(header::HeaderName::from_static("mcp-session-id"), sid)],
            Json(resp),
        )
            .into_response();
    }

    if is_notification {
        // notifications/initialized and any other notification -> 202 Accepted
        return StatusCode::ACCEPTED.into_response();
    }

    // All other requests require a valid session
    let sid = match session_id(&headers) {
        Some(s) if state.sessions.lock().unwrap().contains(&s) => s,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "Session not found"}})),
            )
                .into_response();
        }
    };
    let _ = sid;

    let result = match method {
        "ping" => json!({}),
        "tools/list" => tools::tools_list_value(),
        "prompts/list" => json!({"prompts": []}),
        "resources/list" => json!({"resources": []}),
        "resources/templates/list" => json!({"resourceTemplates": []}),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let app = state.app.clone();
            let name_for_schema = name.clone();
            let t0 = std::time::Instant::now();
            let call = tokio::task::spawn_blocking(move || tools::dispatch(&app, &name, &args)).await;
            let latency_us = t0.elapsed().as_micros() as u64;
            let (result, is_error) = match call {
                Ok(Ok(value)) => {
                    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                    let has_output = tools::tool_registry()
                        .iter()
                        .find(|t| t.name == name_for_schema)
                        .map(|t| t.has_output_schema)
                        .unwrap_or(false);
                    let mut result = json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    });
                    if has_output {
                        result["structuredContent"] = json!({"result": value});
                    }
                    (result, false)
                }
                Ok(Err(msg)) => (json!({
                    "content": [{"type": "text", "text": msg}],
                    "isError": true
                }), true),
                Err(e) => (json!({
                    "content": [{"type": "text", "text": format!("Tool execution failed: {e}")}],
                    "isError": true
                }), true),
            };
            state.app.metrics.record_tool(&name_for_schema, is_error, latency_us);
            result
        }
        _ => {
            return (
                StatusCode::OK,
                Json(jsonrpc_error(&id, -32601, &format!("Method not found: {method}"))),
            )
                .into_response();
        }
    };

    (StatusCode::OK, Json(json!({"jsonrpc": "2.0", "id": id, "result": result}))).into_response()
}

pub async fn handle_get(State(state): State<Arc<McpState>>, headers: HeaderMap) -> Response {
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    tracing::debug!("[mcp] GET stream accept='{}' sid={}", accept, session_id(&headers).unwrap_or_default());
    let sid = match session_id(&headers) {
        Some(s) if state.sessions.lock().unwrap().contains(&s) => s,
        _ => return (StatusCode::BAD_REQUEST, "Missing or unknown session ID").into_response(),
    };
    // Take the receiver created at initialize; the sender stays in the session map so
    // the stream remains open for the session lifetime (FastMCP behavior).
    let rx = state.streams.lock().unwrap().get_mut(&sid).and_then(|e| e.rx.take());
    let rx = match rx {
        Some(rx) => rx,
        None => return (StatusCode::BAD_REQUEST, "Stream already opened for this session").into_response(),
    };
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)).text("ping"))
        .into_response()
}

pub async fn handle_delete(State(state): State<Arc<McpState>>, headers: HeaderMap) -> Response {
    if let Some(sid) = session_id(&headers) {
        state.sessions.lock().unwrap().remove(&sid);
        state.streams.lock().unwrap().remove(&sid);
    }
    (StatusCode::OK, Json(json!({}))).into_response()
}

/// Liveness/readiness probe (#6). No MCP session required.
pub async fn handle_health(State(state): State<Arc<McpState>>) -> Response {
    let app = &state.app;
    let facts = app.store.get_stats().ok();
    let total = facts.as_ref().and_then(|f| f.get("total_facts")).and_then(|v| v.as_i64()).unwrap_or(-1);
    let model = app.store.model_name.read().unwrap().clone();
    let body = json!({
        "status": "ok",
        "server": "rlm-rs",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": app.metrics.uptime_secs(),
        "total_facts": total,
        "embedding_model": model,
        "embeddings_enabled": app.embedder.is_some(),
        "reranker_enabled": app.router.reranker.enabled,
        "reranker_url": app.router.reranker.url,
    });
    (StatusCode::OK, Json(body)).into_response()
}

/// Metrics endpoint (#6): Prometheus text by default, JSON with ?format=json.
pub async fn handle_metrics(State(state): State<Arc<McpState>>, uri: axum::http::Uri) -> Response {
    let want_json = uri.query().map(|q| q.contains("format=json")).unwrap_or(false);
    if want_json {
        (StatusCode::OK, Json(state.app.metrics.snapshot())).into_response()
    } else {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
            state.app.metrics.prometheus(),
        )
            .into_response()
    }
}