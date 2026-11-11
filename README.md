# rlm-rs

Rust rewrite of the **RLM hierarchical memory MCP server** — a drop-in replacement for the
Python `rlm-toolkit` + `docker/start_server.py` deployment that runs on a dedicated LXC host. Same 18 MCP tools, same SQLite data, same MCP streamable-HTTP protocol,
same `all-MiniLM-L6-v2` embeddings.

## Why

The Python service loads PyTorch + sentence-transformers and sits at ~530 MB RSS. This
rewrite runs the same MiniLM model on [candle](https://github.com/huggingface/candle)
(pure-Rust inference) and serves MCP over [axum](https://github.com/tokio-rs/axum),
cutting memory and startup time while keeping the on-disk data and the client-facing
contract byte-compatible.

## Compatibility contract

- **Tools**: the exact 18 tools, names, descriptions, input/output JSON schemas match the
  Python server (verified with `compare_tools.py`, 0 diffs).
- **Data**: reads/writes the existing `memory_bridge_v2.db` (schema 2.0.0),
  `causal_chains.db`, and `memory_bridge.db` in place. No migration. Embeddings are stored
  as JSON float arrays exactly like upstream; new writes use the same card-text wrapper
  (`domain | level | module | title | content`, truncated to 1000 chars).
- **Embeddings**: `all-MiniLM-L6-v2` loaded from the local HF cache
  (`~/.cache/huggingface/hub/models--sentence-transformers--all-MiniLM-L6-v2`), mean
  pooling + L2 normalize, 256-token truncation. Cosine parity vs sentence-transformers
  measured at **1.000000** across English/Russian/card/long texts (`parity_check.py`).
- **Behavior**: ports the `start_server.py` monkey-patches — hybrid FTS5+RRF routing with
  profile/level boosts and reranker (#43/#44/#46), bi-temporal `as_of`/invalidate/diff
  (#42/#45), dedup gate (#49), WRITE-RULES gate (#48), `ttl_days=0` (#31), `fact_id`
  consistency (#40), freshness revisor (#47), noise filtering (#23/#33), model-mismatch
  reindex (#37), Windows-path discovery normalization (#20/#32).

## Layout

```
src/
  main.rs        startup, config, reindex, schedulers, axum serve
  mcp.rs         MCP streamable-HTTP transport (initialize/tools/list/tools/call/ping, SSE)
  tools.rs       18 tool schemas + dispatch, add-fact gates, diff
  store.rs       hierarchical fact store (rusqlite), FTS5, card embeddings, soft delete
  bert.rs        MiniLM BERT forward on candle-nn
  embed.rs       embedder: local candle / Ollama / OpenAI providers
  search.rs      hybrid search (semantic + keyword + recency, relevance gate)
  router.rs      hybrid route (vector + tiered FTS + weighted RRF + rerank), formatter
  rerank.rs      reranker HTTP client with circuit-breaker cooldown
  causal.rs      causal chain tracker + recent-decisions fallback
  session.rs     v1 cognitive-state sessions ("default" canonical)
  revisor.rs     freshness revisor + daily scheduler
  ttl.rs         TTL expiration processing
  coldstart.rs   project discovery
  automode.rs    orchestrator + enterprise context builder
  consolidate.rs fact consolidation
  profiles.rs    agent loadout profiles
  seqmatch.rs    Python difflib.SequenceMatcher.ratio (dedup gate)
  util.rs        timestamps, cosine, jaccard, noise filter, rounding
```

## Build & run

```bash
cargo build --release
# server (defaults: 0.0.0.0:8200, data dir /data/.rlm)
./target/release/rlm-rs --transport streamable-http --host 0.0.0.0 --port 8250
# embedding parity/debug helper
./target/release/rlm-rs --embed-text "some text"   # prints JSON vector
```

Environment (mirrors the Python unit): `RLM_DATA_DIR` (honored if it exists, else
`/data/.rlm`), `RLM_EMBEDDING_MODEL` (default `all-MiniLM-L6-v2`), `RLM_EMBEDDING_PROVIDER`
(local/ollama/openai), `RERANK_ENABLED`, `RERANK_URL`, `REVISOR_MODE`, `REVISOR_ENABLED`,
`RLM_PROJECT_ROOT`, `HF_HUB_OFFLINE`.

The production systemd unit is `rlm-rs.service` (port 8250, live data). A test unit
`rlm-rs-test.service` (port 8253) runs against a copy in `testdata/`.

## Verification

- `compare_tools.py` — tools/list parity vs the Python server.
- `parity_check.py` — embedding cosine parity (run with the Python venv).
- `tests/` — the upstream regression suites (`test_mcp_matrix`, `test_extended_matrix`,
  `test_regression_35/38/40`), parameterized via `RLM_TEST_URL`.
- `bench_quality.py` — routing hit@5 bench.

On a copy of the production data, Rust matches the reference Python implementation
exactly: identical pass/fail on all five regression suites and identical routing hit@5
with identical per-query miss lists. Two suite checks (`Z4.1`, `#31b`) fail identically on
**both** implementations on production data — they are artifacts of the production data
set (the dedup gate #49 rejects near-duplicate adds; 181 stale L0 facts fill the top-20
stale window), not regressions.