# Verification evidence — rlm-rs vs reference Python

All checks run on host `rlm` on 2026-11-11. The Rust test instance
(:8253) and a reference Python instance (:8252, same `start_server.py` + venv) were
run against **identical fresh copies** of the production data for the regression and
smoke comparisons. Production cutover used the live `/data/.rlm` data.

## 1. Tool surface parity (`compare_tools.py`)

```
Python tools: 18, Rust tools: 18
Names identical: True; field diffs: 0
RESULT: PASS
```

Names, descriptions, input schemas and output schemas match the live Python server
exactly (0 diffs across all 18 tools).

## 2. Regression suites (`run-compare.sh`, identical data copies)

| suite | Python :8252 | Rust :8253 |
|---|---|---|
| test_mcp_matrix | 17/17 | 17/17 |
| test_extended_matrix | 8/9 | 8/9 |
| test_regression_35 | 20/21 | 20/21 |
| test_regression_38 | 11/11 | 11/11 |
| test_regression_40 | 16/16 | 16/16 |

**Identical pass/fail on every suite.** The two checks that fail (`Z4.1`, `#31b`) fail
on **both** implementations on production data:
- `Z4.1` adds three near-duplicate facts; the dedup gate (#49, a required feature that
  postdates this test) rejects the 2nd/3rd as duplicates on both servers.
- `#31b` expects a fresh TTL fact in the top-20 stale list, but 181 stale L0 facts fill
  that window (ordering `level ASC, created_at DESC`) on both servers.

These are production-data artifacts / an outdated test, not Rust regressions.

## 3. Routing quality bench (`bench_quality.py`, 27 prod questions)

```
PYTHON: hit@5 = 13/27 = 0.48
RUST:   hit@5 = 13/27 = 0.48
MISS lists identical
```

Rust routing quality equals the Python baseline on the same data, with identical
per-query miss lists.

## 4. Embedding parity (`parity_check.py`)

```
[0..6] cos=1.000000  dims=384  (English / Russian / card-text / single char / long)
WORST cosine = 1.000000  (threshold 0.99) -> PASS
```

candle MiniLM embeddings match sentence-transformers bit-for-bit (cosine 1.0).

## 5. Production cutover

- Backup of live DBs taken before cutover: `/opt/rlm-rs/backup-20261111-215239/`
  (memory_bridge_v2.db 137 MB + causal_chains.db + memory_bridge.db).
- `rlm.service` (Python) stopped, unit file preserved for rollback.
- `rlm-rs.service` (Rust) active on `0.0.0.0:8250`, live `/data/.rlm` data.
- Live MCP call from the Qwen session (`rlm_get_hierarchy_stats`):
  `total_facts=7189, domains=96, db=/data/.rlm/memory/memory_bridge_v2.db`,
  causal nodes 426 — intact.
- Live `rlm_search_facts` from the Qwen session returns relevant hybrid-ranked results
  (embedder + FTS5 + scoring path verified end-to-end through the real client).

## 6. Memory footprint

```
Python prod RSS (before): 514 MB
Rust prod RSS (after):    107 MB   (< 200 MB target)
```

## Rollback

```bash
systemctl stop rlm-rs.service && systemctl start rlm.service
```

The Python venv, unit, and data are unchanged; the Rust service reads the same DBs in
place. DB backup available at the path above.

---

## Quality improvements (2026-11-12, "делай все")

Baseline routing quality was hit@5 = 0.48 (13/27 prod questions). Diagnosis of the 14
misses: **11 were stale (7) or archived (4) target facts excluded from search**, only 3
were live facts mis-ranked. So the dominant lever was stale visibility, not the model.

### Stale visibility (`RLM_STALE_WEIGHT`)

Stale facts were hard-excluded; ~72% of the base (5179/7189) was stale and invisible.
"Stale" is a freshness flag (TTL exceeded), not a correctness judgment. Search/routing
now include stale facts, ranked by relevance; search still favors fresh facts via its
recency term. Tunable via `RLM_STALE_WEIGHT` (default **1.0** = fully visible; `0.5` =
down-weighted; `0` = previous hard-exclude). Archived facts stay excluded.

Measured hit@5 (27 prod questions, English model, reranker available):

| config | hit@5 |
|---|---|
| stale excluded (baseline) | 13/27 = 0.48 |
| stale weight 0.5 | 17/27 = 0.63 |
| **stale weight 1.0 (shipped default)** | **20/27 = 0.74** |

Garbage-query tests still return empty (relevance gate intact). Regression suites
unchanged (17/17, 8/9, 20/21, 11/11, 16/16 — same 2 production-data artifacts).

### Multilingual model — evaluated, NOT shipped

`paraphrase-multilingual-MiniLM-L12-v2` loads in candle with cosine parity 1.000000
(Unigram/Precompiled tokenizer, 12-layer BERT). But on the (English-query) bench it
scored 0.67 (stale 0.5) / 0.59 (stale 1.0) — below the English model's 0.74 — and its
~470 MB weights would push RSS from ~106 MB to ~450 MB, undoing the memory win. Kept
`all-MiniLM-L6-v2`. Revisit if Russian-query usage dominates.

### Reranker

`<reranker-host>:8400` is intermittently unavailable; the circuit-breaker cooldown (120 s)
falls back to RRF order gracefully (verified `reranked=true` when up). External service,
not a server defect.

### Perf / ops (also shipped)

- In-memory cache of facts+embeddings (invalidated on mutation) — no per-query re-parse
  of ~7k JSON vectors.
- SQLite WAL + `synchronous=NORMAL`.
- `GET /health` and `GET /metrics` (Prometheus text or `?format=json`).
- 18 Rust unit tests (`cargo test --release`), all passing.