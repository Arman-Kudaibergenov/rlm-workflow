# SDD: search_facts reliability — issues #36, #37, #38

## Problem & Goal

Three bugs in `search_facts` found by Sergey Muravyov via scenario tests:

1. **#36** — `search_facts` always returns empty list. `min_score=0.55` is too high; PENDING-type facts with keyword_weight=0.8 score 0.1–0.4 and get filtered out. The `_is_noise()` guard already handles garbage.
2. **#37** — After changing `RLM_EMBEDDING_MODEL`, old embeddings remain in v2 store indexed with old model. Cosine similarity between different vector spaces is meaningless → search silently returns nothing. No error, no warning.
3. **#38** — Fresh facts with zero relevance pass `min_score` via recency boost alone. `recency_weight=0.2 * recency=1.0 = 0.2 > min_score=0.1` → nonsense queries return results.

**Goal:** Make search_facts reliable — return relevant results when they exist (#36), detect model mismatch and reindex (#37), reject irrelevant results even if fresh (#38).

## Architecture

All fixes are in `rlm-workflow/docker/start_server.py` — the monkey-patch layer over `rlm-toolkit==2.3.1`.

- **#36 + #38** — both in `_v2_hybrid_search()` (line 399–519): scoring and filtering logic
- **#37** — new function `_check_embedding_model_mismatch()` called at startup, before `_patch_embedding()`

No DB schema changes. No new dependencies.

## Changes to Existing Code

### File: `docker/start_server.py`

#### Fix #36 + #38: Replace flat min_score with relevance gate

**Current code (lines 507–509):**
```python
min_score = float(os.environ.get("RLM_MIN_SCORE", "0.55"))
result = sorted(scored, key=lambda x: -x[1])[:top_k]
result = [(fact, score) for fact, score in result if score >= min_score]
```

**New code:**
```python
min_relevance = float(os.environ.get("RLM_MIN_RELEVANCE", "0.05"))
min_score = float(os.environ.get("RLM_MIN_SCORE", "0.1"))
```

In the scoring loop, compute `relevance = semantic + keyword` (before recency), skip facts where relevance < min_relevance. Then apply min_score to final combined score.

This fixes:
- **#36**: min_score lowered from 0.55 to 0.1, so PENDING facts pass
- **#38**: min_relevance gate (0.05) requires at least some semantic or keyword match before recency can boost score. Fresh garbage with 0 relevance is excluded regardless of recency.

#### Fix #37: Detect embedding model mismatch and reindex on startup

**New function `_check_and_reindex_embeddings(server)`** called in `main()` after `_patch_embedding()`.

Logic:
1. Read `RLM_EMBEDDING_MODEL` env var
2. Query v2 store for existing embeddings and their model metadata
3. If model_name in stored embeddings differs from current → reindex all facts
4. Log warning with count of reindexed facts

The v2 `HierarchicalMemoryStore` stores embeddings in SQLite table `embeddings`. Need to check if model metadata is tracked there. If not, store model name in a metadata key (`_embedding_model`) in the store's metadata table.

### Implementation detail for #37

Check `HierarchicalMemoryStore` for:
- How embeddings are stored (table schema)
- Whether model name is tracked per embedding
- `get_facts_with_embeddings()` return format

## Implementation Stages

### Stage 1: Fix #36 + #38 (scoring logic)

1. In `_v2_hybrid_search()`, split scoring into relevance + recency
2. Add `min_relevance` gate before recency boost
3. Lower `min_score` default to 0.1
4. **Verify:** unit test with nonsense query returns empty, PENDING query returns results

### Stage 2: Fix #37 (model mismatch detection + reindex)

1. Add `_check_and_reindex_embeddings(server)` function
2. Store current model name in metadata after successful reindex
3. On startup: compare stored model vs current, reindex if mismatch
4. **Verify:** test with model switch scenario

### Stage 3: Regression tests

1. Add tests to `tests/test_regression_38.py` covering all three issues
2. Test matrix: empty results, garbage filtering, model switch reindex

## Acceptance Criteria

- [ ] `search_facts(query="PENDING tasks")` returns matching facts when they exist
- [ ] `search_facts(query="xyzzy12345nonsense")` returns empty list
- [ ] After model switch + restart, search continues to find facts
- [ ] Server logs reindex warning when model mismatch detected
- [ ] All existing regression tests (#35) still pass
- [ ] `RLM_MIN_SCORE` and `RLM_MIN_RELEVANCE` env vars are respected

## No-Go Criteria

- Reindex corrupts or loses existing facts
- Existing tests break
- Server startup time increases by more than 30s (reindex should be fast for <1000 facts)
