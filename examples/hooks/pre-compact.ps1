# Pre-Compact Hook for RLM Workflow
# Event: PreCompact — fires before Claude Code auto-compacts the conversation.
# Mechanism: stdout output is injected as a system message into Claude's context.
#            Claude reads the instruction and saves to RLM before compaction.
#
# Installation: add to ~/.claude/settings.json under "PreCompact" hooks
# See: docs/ru/хуки.md

param()

$ts = Get-Date -Format "yyyy-MM-ddTHH:mm:ss"

Write-Output @"
AUTO-COMPACT TRIGGERED at $ts. MANDATORY: Save session state to RLM NOW — before compacting erases context.

Execute these 3 calls immediately, before anything else:

1. rlm_add_hierarchical_fact(
     content="TASK PROGRESS [task_id if known] auto-compact ${ts}:
      Done: <1-2 sentence summary of accomplished work>.
      Files: <list key modified files>.
      Decisions: <key architectural/design decisions>.
      Remaining: <what still needs to be done>.",
     domain="retrospective", level=1
   )

2. rlm_add_hierarchical_fact(
     content="PENDING tasks next session [task_id: <id if known>]: <numbered list. Write 'none' if nothing pending>",
     domain="workflow", level=1
   )

3. rlm_sync_state()

Context can be restored in next session with: контекст
"@

exit 0
