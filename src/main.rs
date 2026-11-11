//! rlm-rs — Rust rewrite of the RLM MCP memory server (rlm-toolkit + start_server.py).
//! Drop-in compatible: same tools, same SQLite data, same MCP streamable-http protocol.

mod automode;
mod bert;
mod causal;
mod coldstart;
mod consolidate;
mod embed;
mod mcp;
mod profiles;
mod revisor;
mod rerank;
mod router;
mod search;
mod seqmatch;
mod session;
mod store;
mod tools;
mod ttl;
mod util;

use std::sync::Arc;

use anyhow::Result;
use store::Store;
use tools::AppState;

fn data_dir() -> String {
    // The Python deployment's ContextManager resolves storage to /data/.rlm;
    // RLM_DATA_DIR is honored when it points at an existing directory.
    if let Ok(d) = std::env::var("RLM_DATA_DIR") {
        if !d.is_empty() && std::path::Path::new(&d).is_dir() {
            return d;
        }
    }
    "/data/.rlm".to_string()
}

/// #37: detect embedding model mismatch and reindex on startup (crash-safe marker).
fn check_and_reindex(store: &Store, embedder: &embed::Embedder, current_model: &str) {
    let (models, _, in_progress) = match store.embedding_model_state() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("[#37] Reindex check failed: {e}");
            return;
        }
    };
    let mut needs_reindex = in_progress;
    if in_progress {
        tracing::info!("[#37] Previous reindex was interrupted — forcing full reindex");
    }
    if !models.is_empty() && !(models.len() == 1 && models[0] == current_model) {
        if let Ok(stale) = store.stale_embedding_count(current_model) {
            if stale > 0 {
                tracing::info!("[#37] Model mismatch: {models:?} != {{{current_model}}} ({stale} stale embeddings)");
                needs_reindex = true;
            }
        }
    }
    if !needs_reindex {
        return;
    }
    let _ = store.set_schema_marker("reindex_in_progress", Some(&util::now_iso()));
    let facts = match store.all_fact_id_content() {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[#37] cannot list facts: {e}");
            return;
        }
    };
    if facts.is_empty() {
        let _ = store.set_schema_marker("reindex_in_progress", None);
        let _ = store.set_schema_marker("embedding_model", Some(current_model));
        return;
    }
    tracing::info!("[#37] Reindexing {} facts with model '{current_model}'...", facts.len());
    let mut reindexed = 0;
    let mut failed = 0;
    for (i, (id, content)) in facts.iter().enumerate() {
        match embedder.encode(content) {
            Ok(emb) => {
                if store.update_embedding(id, &emb, current_model).is_ok() {
                    reindexed += 1;
                } else {
                    failed += 1;
                }
            }
            Err(_) => failed += 1,
        }
        if (i + 1) % 50 == 0 {
            tracing::info!("[#37] Reindex progress: {}/{}", i + 1, facts.len());
        }
    }
    if failed > 0 {
        tracing::warn!("[#37] Reindex INCOMPLETE: {reindexed}/{} ok, {failed} failed — marker kept", facts.len());
    } else {
        let _ = store.set_schema_marker("reindex_in_progress", None);
        let _ = store.set_schema_marker("embedding_model", Some(current_model));
        tracing::info!("[#37] Reindex complete: {reindexed}/{} facts updated", facts.len());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let mut host = "0.0.0.0".to_string();
    let mut port: u16 = 8200;
    let mut transport = "streamable-http".to_string();
    let mut embed_text: Option<String> = None;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" if i + 1 < args.len() => {
                host = args[i + 1].clone();
                i += 2;
            }
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().unwrap_or(8200);
                i += 2;
            }
            "--transport" if i + 1 < args.len() => {
                transport = args[i + 1].clone();
                i += 2;
            }
            "--embed-text" if i + 1 < args.len() => {
                embed_text = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }

    // Debug/parity mode: print the embedding of a text as JSON and exit.
    if let Some(text) = embed_text {
        let model_name = std::env::var("RLM_EMBEDDING_MODEL").unwrap_or_else(|_| "all-MiniLM-L6-v2".to_string());
        let e = embed::Embedder::create(&model_name)?;
        let v = e.encode(&text)?;
        println!("{}", serde_json::to_string(&v)?);
        return Ok(());
    }
    if transport != "streamable-http" {
        anyhow::bail!("rlm-rs supports only --transport streamable-http (got {transport})");
    }

    let dd = data_dir();
    let mem_dir = format!("{dd}/memory");
    let facts_db = format!("{mem_dir}/memory_bridge_v2.db");
    let causal_db = format!("{mem_dir}/causal_chains.db");
    let bridge_db = format!("{mem_dir}/memory_bridge.db");
    let project_root = std::env::var("RLM_PROJECT_ROOT").unwrap_or_else(|_| "/data".to_string());

    println!("RLM data dir: {dd}");
    let store = Arc::new(Store::new(&facts_db)?);
    println!("Memory Bridge v2.0 initialized at {facts_db}");

    // Embedder
    let model_name = std::env::var("RLM_EMBEDDING_MODEL").unwrap_or_else(|_| "all-MiniLM-L6-v2".to_string());
    let embedder: Option<Arc<embed::Embedder>> = match embed::Embedder::create(&model_name) {
        Ok(e) => Some(Arc::new(e)),
        Err(e) => {
            let strict = std::env::var("RLM_EMBEDDING_STRICT")
                .unwrap_or_default()
                .to_lowercase();
            if matches!(strict.as_str(), "true" | "1" | "yes") {
                anyhow::bail!("RLM_EMBEDDING_STRICT=true — refusing to start: {e}");
            }
            tracing::error!("Embedding override failed for '{model_name}': {e} — continuing without embeddings");
            None
        }
    };
    if let Some(e) = &embedder {
        store.set_embedder(e.clone());
        check_and_reindex(&store, e, e.model_name());
    }

    // FTS5 index + triggers (#43)
    if let Err(e) = store.ensure_fts() {
        tracing::warn!("[#43] FTS5 setup failed ({e}) — vector-only routing");
    }

    let causal = Arc::new(causal::CausalTracker::new(&causal_db)?);
    let sessions = Arc::new(session::SessionManager::new(&bridge_db)?);
    let reranker = Arc::new(rerank::Reranker::from_env());
    let router = Arc::new(router::Router::new(store.clone(), embedder.clone(), reranker.clone()));
    let orchestrator = Arc::new(automode::Orchestrator::new(store.clone(), &project_root));
    let builder = Arc::new(automode::ContextBuilder {
        store: store.clone(),
        router: router.clone(),
        causal: causal.clone(),
        orchestrator: orchestrator.clone(),
    });
    let revisor = Arc::new(revisor::Revisor::from_env(store.clone()));

    let app = Arc::new(AppState {
        store: store.clone(),
        embedder,
        router,
        causal,
        sessions,
        orchestrator,
        builder,
        revisor: revisor.clone(),
    });

    // Revisor scheduler (#47): daily 04:17, Sunday deep
    let revisor_enabled = std::env::var("REVISOR_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase();
    if !matches!(revisor_enabled.as_str(), "0" | "false" | "no") {
        let rev = revisor.clone();
        tokio::spawn(async move { revisor::scheduler_loop(rev).await });
        println!("Revisor scheduler on (mode={}, daily 04:17)", revisor.mode_default);
    } else {
        println!("Revisor scheduler off (REVISOR_ENABLED=false), manual runs only");
    }

    // TTL auto-processor (every 6 hours, like upstream)
    {
        let st = store.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                let st2 = st.clone();
                let _ = tokio::task::spawn_blocking(move || ttl::process_expired(&st2)).await;
            }
        });
    }

    let mcp_state = Arc::new(mcp::McpState {
        app,
        sessions: std::sync::Mutex::new(std::collections::HashSet::new()),
        streams: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let router_http = axum::Router::new()
        .route(
            "/mcp",
            axum::routing::post(mcp::handle_post)
                .get(mcp::handle_get)
                .delete(mcp::handle_delete),
        )
        .with_state(mcp_state);

    let addr = format!("{host}:{port}");
    println!("Starting RLM MCP server (rust) on {addr}/mcp [streamable-http]");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router_http)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}