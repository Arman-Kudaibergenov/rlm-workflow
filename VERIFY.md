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