//! MCP streamable-HTTP transport — matches FastMCP (mcp SDK 1.27) behavior:
//! JSON responses to POST, Mcp-Session-Id sessions, SSE GET stream, 202 for
//! notifications, 404 for unknown sessions.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::tools::{self, AppState};

pub struct McpState {
    pub app: Arc<AppState>,
    pub sessions: Mutex<HashSet<String>>,
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
            let call = tokio::task::spawn_blocking(move || tools::dispatch(&app, &name, &args)).await;
            match call {
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
                    result
                }
                Ok(Err(msg)) => json!({
                    "content": [{"type": "text", "text": msg}],
                    "isError": true
                }),
                Err(e) => json!({
                    "content": [{"type": "text", "text": format!("Tool execution failed: {e}")}],
                    "isError": true
                }),
            }
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
    let sid = match session_id(&headers) {
        Some(s) if state.sessions.lock().unwrap().contains(&s) => s,
        _ => return (StatusCode::BAD_REQUEST, "Missing or unknown session ID").into_response(),
    };
    let _ = sid;
    // Server->client stream: stays open, no unsolicited messages in this deployment.
    let (_tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(1);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)).text("ping"))
        .into_response()
}

pub async fn handle_delete(State(state): State<Arc<McpState>>, headers: HeaderMap) -> Response {
    if let Some(sid) = session_id(&headers) {
        state.sessions.lock().unwrap().remove(&sid);
    }
    (StatusCode::OK, Json(json!({}))).into_response()
}