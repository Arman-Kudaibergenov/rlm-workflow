# SDD: Fix RLM-Toolkit Issues #40–#41

**Status**: REVISED (post SDD_AUDIT — 2 HIGH, 3 MEDIUM fixed)
**Target**: `docker/start_server.py`, `tests/test_regression_40.py`
**Validation**: Regression test suite + manual Docker smoke test

---

## 1. Problem & Goal

Two API consistency bugs reported by pytest suite (PR #39):

### Issue #40: `rlm_get_facts_by_domain` and `rlm_get_stale_facts` return `id` instead of `fact_id`
- `rlm_add_hierarchical_fact` returns `fact_id`
- `rlm_search_facts` returns `fact_id`
- `rlm_delete_fact` accepts `fact_id`
- But `rlm_get_facts_by_domain` returns `id` (active path: `memory_bridge/tools/facts.py:222`, also in `infra.py:86`)
- And `rlm_get_stale_facts` returns `id` (active path: `memory_bridge/tools/facts.py:151`, also in `lifecycle.py:68`)
- Note: active runtime uses modular `register_memory_bridge_v2_tools()` flow; legacy modules are informational only
- Agent code that chains tool calls breaks with `KeyError: 'fact_id'`

### Issue #41: `rlm_enterprise_context` description says "Zero configuration" but `query` is required
- Tool description: `"One-call enterprise context with auto-discovery, semantic routing, and causal chains. Zero configuration."`
- Schema: `query` is required (no default)
- AI agent reads description, calls without `query`, gets validation error
- Active registration path: `memory_bridge/tools/context.py:56` (modular v2 flow via `register_memory_bridge_v2_tools()`)
- Legacy path also exists in `routing.py:119` but is not the active runtime path

**Goal**: Fix both issues with monkey-patches in `start_server.py`, validate with regression tests.

---

## 2. Architecture

### Issue #40 fix — Category B: Post-creation patch

The `id` key is set inside tool closures registered by `@server.tool()`. We must intercept the tool handlers AFTER `create_server()` — same pattern as `_patch_ttl_days_zero()`.

New function: `_patch_fact_id_consistency(server)`
- Wraps `rlm_get_facts_by_domain` handler to rename `id` → `fact_id` in response dicts
- Wraps `rlm_get_stale_facts` handler to rename `id` → `fact_id` in response dicts
- For backward compatibility: includes BOTH `id` and `fact_id` (same value) — agents using `id` won't break

### Issue #41 fix — Category B: Post-creation patch

The description string lives in the MCP tool registration metadata. We patch the tool's description after `create_server()`.

New function: `_patch_enterprise_context_description(server)`
- Finds `rlm_enterprise_context` in tool registry
- Replaces description: removes "Zero configuration", adds note that `query` is required
- New description: `"One-call enterprise context with auto-discovery, semantic routing, and causal chains. Requires a query string. RECOMMENDED: Use this instead of individual tools."`

---

## 3. DB Changes

None.

---

## 4. New Modules

None. All changes in existing `docker/start_server.py`.

---

## 5. Changes to Existing Modules

### `docker/start_server.py`

#### New function: `_patch_fact_id_consistency(server)`
```python
def _patch_fact_id_consistency(server):
    """Normalize fact identifier key to 'fact_id' in all tool responses.

    Fixes GitHub issue #40: rlm_get_facts_by_domain and rlm_get_stale_facts
    return 'id' while other tools return 'fact_id'. Agents chaining calls
    get KeyError when accessing fact_id uniformly.

    Both 'id' and 'fact_id' are included for backward compatibility.
    Must run AFTER create_server().
    """
```

Implementation (F-2 fix: guard against error responses):
1. Get tool handler for `rlm_get_facts_by_domain` (try v1 `_tool_handlers`, then v2 `_tool_manager._tools`)
2. Wrap original handler: call it, then:
   - **Guard**: if `response.get("status") != "success"`, return original response unchanged
   - **Guard**: if target field (`"facts"` / `"stale_facts"`) is missing or not a list, return unchanged
   - Iterate items, add `"fact_id": item["id"]` to each dict (keeping `"id"` for backward compat)
3. Same for `rlm_get_stale_facts`: iterate `response["stale_facts"]` with same guards
4. Print confirmation: `[#40] Patched rlm_get_facts_by_domain + rlm_get_stale_facts: id → fact_id`

#### New function: `_patch_enterprise_context_description(server)`
```python
def _patch_enterprise_context_description(server):
    """Fix misleading 'Zero configuration' in enterprise_context description.

    Fixes GitHub issue #41: description says 'Zero configuration' but query
    parameter is required. AI agents read the description, call without query,
    and get a validation error.

    Must run AFTER create_server().
    """
```

Implementation (F-1 fix: correct FastMCP v2 object model):
1. Get tool registry via v2 path: `server.mcp._tool_manager._tools`
2. Access `tools_dict["rlm_enterprise_context"].description` directly (FastMCP `Tool` object stores description as a direct attribute, NOT nested under `.tool`)
3. Replace description string: remove "Zero configuration", add "Requires a query string."
4. Fallback for v1: `server.mcp._tool_handlers` — description lives in registered tool list
5. Print confirmation: `[#41] Patched rlm_enterprise_context: removed 'Zero configuration' from description`

#### Update to `main()` — add patch calls

After line 1217 (`_patch_format_context()`), add:
```python
_patch_fact_id_consistency(server)     # #40
_patch_enterprise_context_description(server)  # #41
```

#### Update to docstring (line 1–21)

Add two lines:
```
- #40: fact_id consistency — normalize id → fact_id in get_facts_by_domain and get_stale_facts
- #41: enterprise_context description — remove misleading 'Zero configuration'
```

### `tests/test_regression_40.py` (NEW FILE)

Regression test for issues #40 and #41. Pattern follows `test_regression_35.py`.

Test cases (F-5 fix: stale_facts backward compat added):
1. **T40.1**: `rlm_add_hierarchical_fact` → `rlm_get_facts_by_domain` → verify `fact_id` key exists in each fact
2. **T40.2**: `rlm_add_hierarchical_fact` with `ttl_days=0` → `rlm_get_stale_facts` → verify `fact_id` key exists
3. **T40.3**: `rlm_get_facts_by_domain` → verify backward compat: both `id` AND `fact_id` present in each fact
4. **T40.4**: `rlm_get_stale_facts` → verify backward compat: both `id` AND `fact_id` present in each stale fact
5. **T41.1**: List tools → find `rlm_enterprise_context` → verify description does NOT contain "Zero configuration"
6. **T41.2**: List tools → find `rlm_enterprise_context` → verify description contains "query" or "Requires"
7. **T41.3**: Call `rlm_enterprise_context` with `query` param → verify success (sanity check)

---

## 6. Implementation Stages

### Stage 1: Implement patches + tests

1. Add `_patch_fact_id_consistency(server)` to `start_server.py`
2. Add `_patch_enterprise_context_description(server)` to `start_server.py`
3. Update `main()` to call both patches
4. Update docstring
5. Create `tests/test_regression_40.py`
6. **Verify**: `docker build` + run regression tests locally
7. **Codex**: IMPLEMENTATION_AUDIT

---

## 7. Acceptance Criteria

1. `rlm_get_facts_by_domain` response facts contain `fact_id` key — YES/NO
2. `rlm_get_stale_facts` response facts contain `fact_id` key — YES/NO
3. Backward compat: both `id` and `fact_id` present — YES/NO
4. `rlm_enterprise_context` description does NOT say "Zero configuration" — YES/NO
5. `rlm_enterprise_context` description mentions query requirement — YES/NO
6. All existing tests still pass (run full suite: `test_mcp_matrix.py`, `test_extended_matrix.py`, `test_regression_35.py`, `test_regression_38.py`) — YES/NO
7. New regression tests pass — YES/NO

---

## 8. No-Go Criteria

- Any existing test fails after patches
- `fact_id` key missing from patched responses
- Tool description still contains "Zero configuration" after patch
- Patch fails silently (no print confirmation in logs)

---

## 9. Version Bump

Add `VERSION` file to repo root with semantic version. Current state: no version file exists.
Proposed: `1.4.0` (based on ~14 issue batches fixed since fork).

Update `CHANGELOG.md` with #40/#41 entries.
