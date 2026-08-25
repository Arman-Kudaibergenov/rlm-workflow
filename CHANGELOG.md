# Changelog — Customizations over RLM-Toolkit

This file documents all modifications made to the original [RLM-Toolkit](https://github.com/DmitrL-dev/AISecurity/tree/main/rlm-toolkit) by Dmitry Labintcev.

## v2.0.1 (2026-08-25)

### Fixed
- **Docker image crash-loop (#45)** — `docker/Dockerfile` relied on the `rlm-toolkit[all]` extra to pull the MCP SDK, but its `mcp>=1.0` constraint now resolves to mcp 2.x, which has no `mcp.server.fastmcp`; the toolkit's import guard then set `server.mcp = None` and `start_server.py` crashed at `@server.mcp.tool(...)`. The Dockerfile now pins `mcp==1.27.0` and `sentence-transformers==5.4.0` (mirroring `Dockerfile.prod`), and every patch that registers MCP tools (`#42`/`#44`/`#45`/`#47`) skips registration with a loud warning, followed by a clear FATAL at startup, if `server.mcp` is ever `None` again.

## v2.0.0 (2026-08-02)

Major release: hybrid search, bi-temporal memory, agent loadout, autonomous freshness, reproducible Docker image. Quality gate: hit@5 = 27/27 (1.00) on the regression bench.

### Added
- **`rlm_diff(since)`** — memory delta tool: new/updated/staled facts since a timestamp or relative window (`24h`, `7d`), grouped, paginated.
- **Freshness in routing** — every fact in `rlm_route_context` / `rlm_enterprise_context` now carries its date and `[STALE]` marker.
- **Card embeddings** — facts are embedded as structured cards (`domain | level | title | content`) instead of raw text (proven discovery improvement).
- **FTS5 + RRF hybrid search** — BM25 (with rare-token tier) fused with vector search via RRF; single-word queries favor exact match, ranked output is now strictly by relevance (no more level-grouped formatting).
- **Reranker integration** — optional cross-encoder rerank (`RERANK_URL`) with circuit breaker (3s timeout, 120s cooldown) and query-window document trimming.
- **Agent loadout profiles** — `profile` parameter (`dev-1c`, `em`, `infra`) in routing: domain-scoped boost/penalty; `rlm_list_profiles` tool.
- **Bi-temporal validity** — facts have `valid_from`/`valid_until`; invalidation closes the window instead of deleting; `as_of` queries ("how memory looked on date X") in route_context and diff; soft delete.
- **`rlm_invalidate_fact` / `rlm_list_inactive`** — explicit invalidation with timestamp and review list.
- **Revisor (autonomous freshness)** — built-in daily pass: per-domain TTL rules (`/data/revisor.yaml`), service liveness pings, optional LLM review (`REVISOR_LLM_URL`); `REVISOR_MODE=report|auto`, invalidation cap, report facts; `rlm_revisor_run` manual trigger.
- **Reproducible Docker image** — `Dockerfile.prod` (embedding model baked in), published as `ghcr.io/arman-kudaibergenov/rlm-workflow:latest`.
- **Quality bench** — regression harness with hit@5 metric (prod suite 27 questions, neutral template in repo).

### Fixed
- **Ranking**: L2/L3 facts no longer buried by level-grouped formatting; level weights (L2 ×1.15, L3 ×1.25) applied in RRF.
- **FTS**: rare-token tier (df≤10) prevents generic-term noise from drowning exact matches.
- **Reranker**: query-term windowing fixes misses on long facts (>256 chars).
- **Archive filtering**: archived facts excluded from all read paths.

### Changed
- Write paths (`mark_stale`, `archive_fact`, `delete`) now stamp `valid_until` (bi-temporal) while keeping legacy flags for compatibility.
- Total MCP tools: 18.

## v1.4.0 (2026-04-03)

### Bug Fixes
- **#40**: `rlm_get_facts_by_domain` and `rlm_get_stale_facts` now return `fact_id` in each fact dict (consistent with all other tools). Both `id` and `fact_id` are present for backward compatibility.
- **#41**: `rlm_enterprise_context` description no longer says "Zero configuration" — updated to indicate `query` parameter is required.

### Other
- Added `VERSION` file (v1.4.0)
- Added `tests/test_regression_40.py` — 14 test cases for #40/#41

## Workflow Layer (CLAUDE.md rituals)

### `суммаризируем` / `summarize` ritual
End-of-session ritual that:
1. Reads autocapture buffer to collect all file changes and commands
2. Saves all important facts to RLM (decisions, architecture, file changes)
3. Saves PENDING task list as a specially-formatted fact for next session recovery
4. Runs git status and commits/stages if session work is complete
5. Clears context and signals user to run `/clear`

### `контекст` / `context` ritual
Start-of-session ritual that:
1. Initializes RLM session with `restore=true`
2. Fetches enterprise context for current project
3. Searches for `PENDING tasks next session` facts (keyword-weighted search)
4. Filters PENDING by current project (suppresses other-project tasks)
5. If PENDING is non-empty — auto-starts the first task without asking

### `новая задача` / `new task` ritual
Task initialization ritual that:
1. Resets RLM session (`restore=false`) for clean task state
2. Evaluates need for brainstorm / openspec based on task complexity
3. Generates human-readable `task_id` slug for cross-session linking
4. Saves TASK START fact with approach, expected MCP, expected files
5. Creates team via TeamCreate if multi-agent work is needed

## Automation Hooks

### Pre-compact hook (`~/.claude/hooks/pre-compact.ps1`)
Fires automatically before Claude Code's context compaction.
Triggers the `суммаризируем` ritual without user intervention.
Ensures zero information loss on auto-compact.

### Context-monitor hook (`~/.claude/hooks/context-monitor.ps1`)
PostToolUse hook that monitors context window usage:
- WARNING at ≥60% (~120k tokens) — notifies user
- AUTO-SUMMARIZE at ≥65% (~130k tokens) — triggers full summarize ritual
- CRITICAL cascade for multi-agent: saves all agent states, shuts down team

### Autocapture buffer
Pre-tool hook that logs every Edit/Write/Bash tool call to:
`~/.claude/autocapture-buffer.jsonl`
Provides accurate file change history even if context is lost.
Buffer is read during `суммаризируем` and cleared after.

## Multi-Agent Memory Protocol

Original RLM has no multi-agent concept. We added:

- **Team memory**: Leader saves task context to RLM before spawning agents
- **Checkpoint protocol**: Agents checkpoint to RLM after each completed task
- **Rotate pattern**: Leader spawns fresh agent after 2-3 heavy tasks (context hygiene)
- **Mandatory RLM block**: Every agent spawn prompt includes RLM initialization rules
- **Cross-agent limitation**: Each agent's RLM session is isolated (known limitation, accepted)
- **Workaround**: Critical findings must come via `SendMessage`, not only RLM

## PENDING Task Tracking

Extended the session fact model with structured PENDING facts:

```
PENDING tasks next session [task_id: <id>] [project: <name>]:
1) <task>. 2) <task>. ...
```

Features:
- `[task_id: ...]` allows retrospective to recover full task history
- `[project: ...]` scopes tasks to specific project
- Keyword-weighted search (`keyword_weight=0.8`) ensures exact phrase matching
- Quality rules: `❓` marker for unconfirmed completions, no `(возможно выполнено)`

## MCP Endpoint Configuration

Original: no specific endpoint recommendation.
Our setup: RLM server at `http://server-ip:8200/mcp` (HTTP transport, not SSE).

## MCP Tools Exposed (confirmed from running server)

```
rlm_start_session       — start/restore session
rlm_enterprise_context  — one-call context load (RECOMMENDED)
rlm_route_context       — semantic routing, returns only relevant facts
rlm_add_hierarchical_fact — add fact at L0–L3
rlm_record_causal_decision — record decision with reasons/consequences/alternatives
rlm_search_facts        — hybrid search (semantic + keyword + recency, configurable weights)
rlm_sync_state          — persist cognitive state to disk
rlm_discover_project    — cold-start project detection and seeding
```

## Observed Benefits (production use, daily development)

- Context loss rate: **0** across months of use (pre-compact hook catches all auto-compacts)
- Token overhead per session: ~2–3k tokens for `rlm_enterprise_context` call vs entire chat history
- Cross-session continuity: tasks spanning days resume without re-explanation
- Multi-day task tracking: PENDING facts survive indefinitely (TTL=30 days)

## Fork Base

Forked from: `DmitrL-dev/AISecurity` — `rlm-toolkit` subdirectory
Original license: Apache 2.0 (preserved, see NOTICE)
