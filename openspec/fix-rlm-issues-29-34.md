# SDD: Fix RLM-Toolkit Issues #29–#34

**Status**: REVISED (post SDD_AUDIT)
**Target**: `docker/start_server.py`, `docker/Dockerfile`
**Validation**: Issue #35 regression matrix (17 test cases)

---

## 1. Problem & Goal

Six open bugs in rlm-workflow (GitHub issues #29–#34) cause:
- Missing MCP tool (`rlm_get_facts_by_domain`) — users can't query by domain
- `watchdog` not installed — file-based TTL refresh silently disabled
- `ttl_days=0` ignored — falsy check treats 0 as "no TTL"
- `rlm_discover_project` returns `unknown`/`data` — misleading on Windows clients
- `rlm_route_context` polluted with `Unknown project` noise — repeated L0 lines
- `rlm_search_facts` returns garbage results — no minimum score threshold

**Goal**: Fix all six bugs with monkey-patches in `start_server.py` + Dockerfile change, validate with #35 test matrix, deploy to CT105.

---

## 2. Architecture

Two categories of patches — distinguished by timing and target:

### Category A: Class-level patches (applied BEFORE `create_server()`)
These patch class methods on upstream types. They work because `create_server()` instantiates the classes after our patches are applied.

- `_patch_search_facts()` — patches `MemoryBridgeManager.hybrid_search` (#19, extended for #34)
- `_patch_discover_project()` — patches `ColdStartOptimizer.discover_project` (#20, extended for #32)
- `_patch_project_overview()` — patches `EnterpriseContextBuilder._get_project_overview` and `EnterpriseContext.to_injection_string` (#23)

### Category B: Post-creation patches (applied AFTER `create_server()`)
These must run after tool registration because they intercept tool closures or format functions.

- `_patch_ttl_days_zero()` — patches `rlm_add_hierarchical_fact` tool closure in `facts.py` (#31). Must run AFTER `create_server()` because the falsy `if ttl_days:` check lives in a registered tool closure, not a class method.
- `_patch_format_context()` — patches `SemanticRouter.format_context_for_injection()` (#33). The noise appears in the formatted string output, not in `route()` which returns `RoutingResult`.

### Category C: Static changes
- `_DEFAULT_TOOLS` set — add `rlm_get_facts_by_domain` (#29)
- `Dockerfile` — add `watchdog` to pip install (#30)

```
start_server.py main() call order:
  1. _suppress_misleading_logs()           # #17
  2. _patch_session_restore()              # #8/#7/#18/#21
  3. _patch_search_facts()                 # #19 + #34 (min_score)
  4. _patch_discover_project()             # #20 + #32 (improved)
  5. _patch_causal_context()               # #22
  6. _patch_project_overview()             # #23
  7. _patch_prevent_default_embedding()    # #24
  8. create_server()                       ← tool closures registered here
  9. _patch_embedding(server)              # #5/#4/#13/#16
  10. _filter_tools(server)                # tool filtering (#29 via _DEFAULT_TOOLS)
  11. _patch_ttl_days_zero(server)         # #31 — AFTER create_server
  12. _patch_format_context()              # #33 — AFTER create_server (patches class, but needs correct timing)
```

---

## 3. DB Changes

None. All patches are runtime monkey-patches.

---

## 4. Changes to Existing Code

### 4.1 Issue #29: Add `rlm_get_facts_by_domain` to `_DEFAULT_TOOLS`

**File**: `docker/start_server.py`, lines 337–350

**Change**: Add `"rlm_get_facts_by_domain"` to the `_DEFAULT_TOOLS` set.

**Why**: Tool is implemented in upstream `facts.py:203-232` but filtered out by `_filter_tools()`. Referenced in `CLAUDE.md.example`.

**Risk**: None — additive change.

### 4.2 Issue #30: Install `watchdog` in Dockerfile

**File**: `docker/Dockerfile`, line 8–9

**Change**: Add `watchdog` to pip install:
```dockerfile
pip install --no-cache-dir \
    "rlm-toolkit[all]==2.3.1" watchdog
```

**Why**: `rlm-toolkit[all]` does not include `watchdog`. Without it, file-based TTL refresh is silently disabled. Upstream `ttl.py:35` logs "watchdog not installed" and `ttl.py:271` logs "Cannot start file watcher: watchdog not installed".

**Risk**: Minimal — watchdog is a well-maintained package. Only activates if upstream `start_file_watcher()` is called.

**Verification**: `docker logs rlm` after start shows NO "watchdog not installed" warning. If file watcher is configured, logs show "FileWatcher started" (real, not the misleading one we suppress via #17).

### 4.3 Issue #31: Fix `ttl_days=0` falsy check

**File**: `docker/start_server.py` — new function `_patch_ttl_days_zero(server)`

**Root cause**: In upstream `facts.py:75`, the tool closure `rlm_add_hierarchical_fact` uses:
```python
if ttl_days:          # line 75 — 0 is falsy!
    ttl_config = TTLConfig(
        ttl_seconds=ttl_days * 24 * 3600,
        on_expire=TTLAction.MARK_STALE,
    )
```
When `ttl_days=0`, this skips TTL creation entirely. The upstream `HierarchicalMemoryStore.add_fact()` takes `ttl_config` (not `ttl_days`), so patching the store won't help.

**Approach**: The tool closure is registered via `@server.tool()` decorator during `create_server()`. After creation, we can access the tool handlers and wrap the closure. However, the simplest approach is to **re-register the tool** with a corrected closure that uses `if ttl_days is not None:` instead of `if ttl_days:`.

```python
def _patch_ttl_days_zero(server):
    """Fix ttl_days=0 being ignored due to falsy check in tool closure.

    Fixes GitHub issue #31: upstream facts.py:75 uses `if ttl_days:` which
    treats 0 as falsy. ttl_days=0 should mean 'expires immediately'.

    Must run AFTER create_server() because the bug is in a tool closure.
    """
    from rlm_toolkit.memory_bridge.v2.hierarchical import TTLConfig, TTLAction, MemoryLevel

    # Access the server's tool components to get the store reference
    store = server.memory_bridge_v2_store

    # Get MCP tool handlers
    handlers = getattr(server.mcp, "_tool_handlers", None)
    tool_manager = getattr(server.mcp, "_tool_manager", None)

    if handlers and "rlm_add_hierarchical_fact" in handlers:
        _original_handler = handlers["rlm_add_hierarchical_fact"]

        async def _patched_handler(
            content: str,
            level: int = 0,
            domain=None,
            module=None,
            code_ref=None,
            parent_id=None,
            ttl_days=None,
        ):
            # Fix: use `is not None` instead of truthiness check
            ttl_config = None
            if ttl_days is not None:
                ttl_config = TTLConfig(
                    ttl_seconds=ttl_days * 24 * 3600,
                    on_expire=TTLAction.MARK_STALE,
                )

            try:
                fact_id = store.add_fact(
                    content=content,
                    level=MemoryLevel(level),
                    domain=domain,
                    module=module,
                    code_ref=code_ref,
                    parent_id=parent_id,
                    ttl_config=ttl_config,
                    source="manual",
                    confidence=1.0,
                )
                return {
                    "status": "success",
                    "fact_id": fact_id,
                    "content": content,
                    "level": MemoryLevel(level).name,
                    "domain": domain,
                    "module": module,
                }
            except Exception as e:
                return {"status": "error", "message": str(e)}

        handlers["rlm_add_hierarchical_fact"] = _patched_handler
        print("  [#31] Patched rlm_add_hierarchical_fact: ttl_days=0 now creates TTL")

    elif tool_manager and hasattr(tool_manager, "_tools"):
        # MCP SDK v2+ path
        tools_dict = tool_manager._tools
        if "rlm_add_hierarchical_fact" in tools_dict:
            tool_entry = tools_dict["rlm_add_hierarchical_fact"]
            _original_fn = tool_entry.fn if hasattr(tool_entry, 'fn') else None

            if _original_fn:
                async def _patched_fn(
                    content: str,
                    level: int = 0,
                    domain=None,
                    module=None,
                    code_ref=None,
                    parent_id=None,
                    ttl_days=None,
                ):
                    ttl_config = None
                    if ttl_days is not None:
                        ttl_config = TTLConfig(
                            ttl_seconds=ttl_days * 24 * 3600,
                            on_expire=TTLAction.MARK_STALE,
                        )
                    try:
                        fact_id = store.add_fact(
                            content=content,
                            level=MemoryLevel(level),
                            domain=domain,
                            module=module,
                            code_ref=code_ref,
                            parent_id=parent_id,
                            ttl_config=ttl_config,
                            source="manual",
                            confidence=1.0,
                        )
                        return {
                            "status": "success",
                            "fact_id": fact_id,
                            "content": content,
                            "level": MemoryLevel(level).name,
                            "domain": domain,
                            "module": module,
                        }
                    except Exception as e:
                        return {"status": "error", "message": str(e)}

                tool_entry.fn = _patched_fn
                print("  [#31] Patched rlm_add_hierarchical_fact (v2): ttl_days=0 now creates TTL")
    else:
        print("  [#31] WARNING: Cannot patch rlm_add_hierarchical_fact — no handler found")
```

**Verification**: `rlm_add_hierarchical_fact(content="test-ttl-zero", ttl_days=0)` → fact appears in `rlm_get_stale_facts()` immediately.

**Risk**: Medium — depends on MCP SDK handler access pattern. Both v1 (`_tool_handlers`) and v2 (`_tool_manager._tools`) paths covered. If neither matches, logs warning and continues.

### 4.4 Issue #34: Add `min_score` threshold to `search_facts`

**File**: `docker/start_server.py`, modify existing `_v2_hybrid_search()` inside `_patch_search_facts()` (line 492)

**Root cause**: `_v2_hybrid_search()` always returns `top_k` results regardless of relevance. Garbage queries get results with scores 0.35–0.48.

**Change**: Add score filtering after sorting, before return:

```python
# Current line 492:
result = sorted(scored, key=lambda x: -x[1])[:top_k]

# Replace with:
min_score = float(os.environ.get("RLM_MIN_SCORE", "0.45"))
result = sorted(scored, key=lambda x: -x[1])[:top_k]
result = [(fact, score) for fact, score in result if score >= min_score]
```

**Threshold rationale**:
- Garbage results observed: 0.35–0.48 (from issue #34 test with nonsense query)
- Good matches: 0.6+
- Default threshold: 0.45 — filters most garbage while keeping borderline matches
- Configurable via `RLM_MIN_SCORE` env var for tuning per deployment

**Verification**: `rlm_search_facts(query="NONEXISTENT_FACT_ABCDEF_ZZZZ")` → returns empty results list.

**Risk**: Medium — threshold too high could filter legitimate results. Mitigated by env var override. Need empirical testing with real data during Stage 6.

### 4.5 Issue #32: Improve `discover_project` for Windows clients

**File**: `docker/start_server.py`, modify existing `_patch_discover_project()` (lines 505–533)

**Root cause**: Windows path is converted to `/data`, but if no project files are bind-mounted, discovery returns `project_info.name="data"`, `file_count=0`. This is technically correct (container has no project files) but misleading.

**Upstream return type**: `ColdStartOptimizer.discover_project()` returns `DiscoveryResult` dataclass (coldstart.py:70-86):
```python
@dataclass
class DiscoveryResult:
    project_info: ProjectInfo   # has .name, .project_type, .file_count
    facts_created: int
    discovery_tokens: int
    suggested_domains: List[str]
    warnings: List[str]
```
The MCP tool (`discovery.py:45-55`) reads `result.project_info.name`, `result.project_info.file_count`, `result.warnings`.

**Change**: Modify `_patched_discover()` to preserve `DiscoveryResult` return type:

```python
def _patched_discover(self, root=None, task_hint=None, **kwargs):
    import re
    from pathlib import Path

    original_path = str(root) if root else None

    if root and isinstance(root, (str, Path)):
        path_str = str(root)
        if re.match(r'^[A-Za-z]:[/\\]', path_str) or '\\' in path_str:
            container_root = os.environ.get("RLM_PROJECT_ROOT", "/data")
            print(f"  [#20] Windows path '{path_str}' → container path '{container_root}'")
            root = Path(container_root)

    result = _original_discover(self, root=root, task_hint=task_hint, **kwargs)

    # Fix project name: if upstream returned "data" (container path name),
    # replace with meaningful name from original Windows path
    if original_path and result.project_info.name == "data":
        win_name = original_path.replace('\\', '/').rstrip('/').split('/')[-1]
        result.project_info.name = win_name
        print(f"  [#32] project_name 'data' → '{win_name}' (from Windows path)")

    # Add bind-mount guidance if container path is empty
    if result.project_info.file_count == 0 and original_path:
        win_name = original_path.replace('\\', '/').rstrip('/').split('/')[-1]
        guidance = (f"Project files not mounted in container. "
                    f"Use: -v /host/path/{win_name}:{container_root}:ro")
        result.warnings.append(guidance)
        print(f"  [#32] {guidance}")

    return result
```

**Verification**:
- `rlm_discover_project(project_root="d:\\Repos\\BIT")` → `project_name="BIT"`, not `"data"`
- `warnings` list contains bind-mount guidance when container path is empty

**Risk**: Low — preserves `DiscoveryResult` type, only mutates `.project_info.name` and `.warnings`.

### 4.6 Issue #33: Filter noise from `format_context_for_injection()`

**File**: `docker/start_server.py` — new function `_patch_format_context()`

**Root cause**: The noise (`Unknown project`, `__FINGERPRINT__`) appears in `SemanticRouter.format_context_for_injection()` output (router.py:305-357). This method iterates `routing_result.facts` and formats each fact's `.content`. L0 facts containing `__FINGERPRINT__:` or `"data is a Unknown project"` get included verbatim.

Current patches in `_patch_project_overview()` target `EnterpriseContextBuilder._get_project_overview()` and `EnterpriseContext.to_injection_string()`, but `rlm_route_context` uses `SemanticRouter.format_context_for_injection()` directly (routing.py:43), which is NOT covered.

**Change**: Patch `format_context_for_injection()` to filter noisy L0 facts:

```python
def _patch_format_context():
    """Filter noise from format_context_for_injection output.

    Fixes GitHub issue #33: route_context returns repeated 'Unknown project' lines
    and __FINGERPRINT__ data. The noise is in L0 facts passed through
    format_context_for_injection(), not in route() which returns RoutingResult.
    """
    try:
        from rlm_toolkit.memory_bridge.v2.router import SemanticRouter
    except ImportError:
        print("  [#33] Cannot patch format_context — SemanticRouter import failed")
        return

    _original_format = SemanticRouter.format_context_for_injection

    def _patched_format(self, routing_result, include_metadata=True):
        # Filter noisy facts from routing_result before formatting
        clean_facts = []
        seen_content = set()
        for fact in routing_result.facts:
            content = fact.content.strip()
            # Skip fingerprint facts
            if content.startswith("__FINGERPRINT__:"):
                continue
            # Skip "X is a Unknown project" noise
            if "is a Unknown project" in content:
                continue
            if content == "Unknown project":
                continue
            # Deduplicate identical content
            if content in seen_content:
                continue
            seen_content.add(content)
            clean_facts.append(fact)

        routing_result.facts = clean_facts
        return _original_format(self, routing_result, include_metadata=include_metadata)

    SemanticRouter.format_context_for_injection = _patched_format
    print("  [#33] Patched format_context_for_injection: filtering noise from L0 facts")
```

**Verification**: `rlm_route_context(query="test")` output contains no `Unknown project`, no `__FINGERPRINT__`, no duplicate lines.

**Risk**: Low — only filters known noise patterns from fact content. Mutates `routing_result.facts` in-place (list replacement), which is safe since the result is consumed immediately by the formatter.

---

## 5. Implementation Stages

### Stage 1: Trivial fixes (#29, #30)
- Add `rlm_get_facts_by_domain` to `_DEFAULT_TOOLS`
- Add `watchdog` to Dockerfile pip install
- **Verify**: Docker build succeeds, tool appears in ListTools, `docker logs` shows no watchdog warning

### Stage 2: Search score threshold (#34)
- Modify `_v2_hybrid_search()` to add `min_score` filtering
- **Verify**: Garbage query returns empty list, real query returns results with score ≥ 0.45

### Stage 3: TTL zero fix (#31)
- Add `_patch_ttl_days_zero(server)` function — runs AFTER `create_server()`
- **Verify**: `ttl_days=0` fact appears in `rlm_get_stale_facts()` immediately

### Stage 4: Discover project improvement (#32)
- Modify existing `_patched_discover()` to fix `project_info.name` and add `warnings`
- **Verify**: Windows path returns `project_name` from path basename, warnings contain bind-mount guidance

### Stage 5: Route context noise filter (#33)
- Add `_patch_format_context()` — patches `format_context_for_injection()`
- Wire into `main()` AFTER `create_server()` (patches class method, timing doesn't matter but keeping consistent)
- **Verify**: No noise in route_context output

### Stage 6: Integration validation (#35)
- Build Docker image locally on laptop
- Run ALL 17 test cases from #35 matrix with real MCP calls
- Verify regression tests pass (especially #13, #19, #20, #24)
- **Verify**: All tests pass, docker logs clean

---

## 6. Acceptance Criteria

| # | Criterion | Test |
|---|-----------|------|
| AC-1 | `rlm_get_facts_by_domain` appears in MCP ListTools and returns results | Call tool with known domain, get facts |
| AC-2 | `docker logs` shows no "watchdog not installed" warning; if file watcher configured, it starts | Check logs after start; verify watcher behavior if applicable |
| AC-3 | `rlm_add_hierarchical_fact(ttl_days=0)` creates fact that appears in `rlm_get_stale_facts()` immediately | Add fact with ttl_days=0, call get_stale_facts, verify fact present |
| AC-4 | `rlm_search_facts(query="NONEXISTENT_GARBAGE")` returns empty results list | Verify `facts` array is empty |
| AC-5 | `rlm_search_facts(query="<existing fact>")` returns non-empty results with score ≥ 0.45 | Verify real results returned |
| AC-6 | `rlm_discover_project(project_root="d:\\Repos\\BIT")` returns `project_name="BIT"` and `warnings` contains bind-mount guidance | Call and verify both name and warnings |
| AC-7 | `rlm_route_context(query="test")` contains no `Unknown project`, no `__FINGERPRINT__`, no duplicates | Call and inspect output |
| AC-8 | All #35 regression matrix tests pass (17/17) | Run full matrix |
| AC-9 | Existing patches (#8, #13, #17, #19, #22, #23, #24) still work — no regressions | Smoke test session restore, search, embedding |

---

## 7. No-Go Criteria

- Any existing patch (#8–#24) breaks → STOP, fix regression first
- `min_score` threshold filters legitimate results in production data → lower threshold or add env var override
- `_patch_ttl_days_zero` can't access tool handlers → investigate MCP SDK internals, find alternative
- Docker build fails → fix Dockerfile before proceeding
- `DiscoveryResult` type contract broken → revert #32 changes

---

## 8. Deploy Plan

1. Local Docker build + test on laptop
2. All 17 tests pass → push to GitHub
3. Build and push Docker image: `ghcr.io/arman-kudaibergenov/rlm-workflow:latest`
4. Deploy to CT105: pull + restart container
5. Smoke test on CT105 via MCP calls
6. Close issues #29–#34 with test evidence

---

## Appendix: Codex SDD_AUDIT Response

**Verdict**: NEEDS_FIXES (3 HIGH, 2 MEDIUM)

### Findings addressed in this revision:

1. **[HIGH] #31 patch targeted wrong seam** → FIXED: Now patches tool closure handler after `create_server()`, not `HierarchicalMemoryStore.add_fact()`
2. **[HIGH] #32 broke DiscoveryResult contract** → FIXED: Now mutates `result.project_info.name` and `result.warnings` on the `DiscoveryResult` dataclass, not returning a dict
3. **[HIGH] #33 patched wrong method** → FIXED: Now patches `format_context_for_injection()` which produces the string output, not `route()` which returns `RoutingResult`
4. **[MEDIUM] AC incomplete for #32 and #30** → FIXED: AC-2 now checks watcher behavior, AC-6 checks both name and warnings
5. **[MEDIUM] Architecture didn't distinguish patch categories** → FIXED: Section 2 now distinguishes Category A (class-level, pre-create) from Category B (post-creation) patches
