# Workflow Rituals

The core value of this project is not the RLM server itself — it's the **workflow patterns**
that make Claude Code remember everything across sessions automatically.

Three trigger phrases control the session lifecycle:

## `контекст` / context — Session Start

Run at the beginning of any work session.

```
контекст
```

What happens:
1. `rlm_start_session(restore=true)` — restore previous session state
2. `rlm_enterprise_context` — fetch project-specific context
3. `rlm_search_facts(query="PENDING tasks next session", keyword_weight=0.8)` — find pending tasks
4. Filter PENDING by current project (suppress other-project tasks)
5. If PENDING is non-empty → announce first task and start immediately, no questions asked
6. If PENDING is empty → ask what to work on

**Result**: Claude knows exactly where you left off, what's pending, and what decisions were made.

## `суммаризируем` / summarize — Session End

Run when finishing work (or when context approaches ~60-65%).

```
суммаризируем
```

What happens:
1. Read autocapture buffer (`~/.claude/autocapture-buffer.jsonl`) for file change log
2. Save to RLM:
   - Completed work (task facts)
   - Architectural decisions (`rlm_record_causal_decision`)
   - File changes, technical details, found errors and solutions
3. Save PENDING fact (required format):
   ```
   PENDING tasks next session [task_id: <id>]: 1) <task>. 2) <task>.
   ```
4. Run `git status` → commit completed work or add to PENDING
5. Report what was saved → tell user to run `/clear`

**Quality rules for PENDING:**
- Only include tasks that are DEFINITELY unfinished
- Mark uncertain completions with `❓`: `❓ db load — no explicit confirmation`
- Tasks from other projects → separate fact with `[project: other-name]`

## `новая задача` / new task — Task Initialization

```
новая задача
```

What happens:
1. `rlm_start_session(restore=false)` — fresh session for new task
2. Clear autocapture buffer
3. Evaluate: does this need brainstorm/openspec? (new feature → yes; bugfix → skip)
4. Generate `task_id` slug: `<project>-<feature>-YYYY-MM-DD`
5. Save TASK START fact to RLM with approach, expected files, expected MCP tools
6. If multi-agent: create team, assign tasks, spawn agents

---

## Example Session Flow

```
# Monday morning
You:  контекст
AI:   [restores context, finds PENDING]
      Pending: 1) Implement payment scheduler. 2) Write tests.
      Starting task 1...

# 2 hours later, context at 58%
You:  суммаризируем
AI:   [saves to RLM]
      Saved: payment-scheduler implementation, DB schema decisions
      PENDING: 1) Write tests for payment scheduler. 2) Code review.
      Committed: src/scheduler.py, docs/api.md
      Run /clear

You:  /clear

# Next day
You:  контекст
AI:   [restores, finds PENDING]
      Pending: 1) Write tests for payment scheduler...
      Starting task 1...
```

**Zero information loss across days, `/compact` events, or machine restarts.**

---

## PENDING Quality Checklist

Before saving PENDING, verify each task:

- [ ] Is this task actually unfinished? (don't include completed items)
- [ ] Does it have enough context to resume without re-reading the chat?
- [ ] Cross-project tasks → separate fact with `[project: name]`
- [ ] Unknown completion status → mark with `❓`
- [ ] Active task_id included → `[task_id: slug]`
