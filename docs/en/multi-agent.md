# Multi-Agent Workflow

One of the unique extensions in this workflow: using RLM as **shared memory for AI agent teams**.

## The Problem

When spawning multiple Claude agents for a complex task, each agent has its own context.
Agents can communicate via messages (`SendMessage`), but that's ephemeral — lost on restart.

RLM provides a persistent shared memory layer that all agents can read and write.

## Architecture

```
Leader (Opus)
├── Spawns teammates via Agent tool
├── Saves task context to RLM before spawning
├── Assigns tasks via TaskCreate/TaskUpdate
└── Receives results via SendMessage

Teammate A (Sonnet)           Teammate B (Sonnet)
├── rlm_start_session()       ├── rlm_start_session()
├── Reads task context        ├── Reads task context
├── Works on assigned tasks   ├── Works on assigned tasks
├── Checkpoints to RLM        ├── Checkpoints to RLM
└── Reports via SendMessage   └── Reports via SendMessage
```

## Known Limitation

Each agent's `rlm_start_session()` creates an **isolated session**. Facts written by
Teammate A are NOT visible to Leader via `rlm_search_facts`. Data is written but
segmented by session_id.

**Decision**: Accept this limitation. Don't pass leader's session_id to agents (coupling).

**Workaround**: Critical findings MUST come via `SendMessage`. RLM checkpoints serve as
emergency recovery only — readable only if you know the agent's session_id.

## Mandatory Agent Prompt Block

Every teammate spawn prompt MUST include this block verbatim:

```
MANDATORY RLM RULES (no hooks in subagents — you must do this manually):
1. START: call rlm_start_session(restore=true) before any work.
2. AFTER EACH TASK: call rlm_add_hierarchical_fact(
     content="TEAM [team-name] checkpoint: [your-role] completed [task-name].
      Files: [list changed files]. Decisions: [key decisions]. Next: [remaining work].",
     domain="team", level=1)
3. BEFORE heavy reads (10+ files): save current findings to RLM first.
4. ON shutdown_request: save final state to RLM, then approve shutdown.
Skipping these checkpoints = lost work if context overflows.
```

## Context Health Management

Teammates have NO hooks (context-monitor, pre-compact). Leader is responsible.

**Rotate pattern**: For tasks with 5+ sub-steps or heavy file reading — Leader shuts
down teammate after 2-3 completed tasks, spawns a fresh one. New teammate reads context
from RLM before starting.

**Leader CRITICAL cascade**:
1. Context-monitor fires CRITICAL on leader
2. Broadcast "checkpoint" to all teammates
3. Wait for their RLM saves
4. Shutdown all teammates
5. Report to user, ask for `/clear`

## Checkpoint Format

```
TEAM [team-name] checkpoint: [role] completed [task].
Files: [path1, path2]. Decisions: [key decisions]. Next: [remaining work].
```

Domain: `team`, Level: 1.
