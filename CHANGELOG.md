# Changelog — Customizations over RLM-Toolkit

This file documents all modifications made to the original [RLM-Toolkit](https://github.com/DmitrL-dev/AISecurity/tree/main/rlm-toolkit) by Dmitry Labintcev.

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

## Fork Base

Forked from: `DmitrL-dev/AISecurity` (commit: TODO — add specific commit hash)
Fork date: TODO
Original license: Apache 2.0 (preserved)
