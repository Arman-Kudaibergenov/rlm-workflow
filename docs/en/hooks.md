# Automation Hooks

Hooks make RLM save automatically — no manual `суммаризируем` needed.

## Pre-compact Hook

Fires before Claude Code's context compaction (`/compact` or auto-compact).
Triggers the summarize ritual automatically so nothing is lost.

**File**: `~/.claude/hooks/pre-compact.ps1` (Windows) or `~/.claude/hooks/pre-compact.sh` (Linux/Mac)

```powershell
# pre-compact.ps1
# Automatically save session to RLM before context is cleared

# This hook fires when Claude is about to compact the context.
# Inject "суммаризируем" into the conversation to trigger the save ritual.
# The exact implementation depends on your Claude Code version — see examples/hooks/
```

See [examples/hooks/pre-compact.ps1](../../examples/hooks/pre-compact.ps1) for full implementation.

## Context-Monitor Hook

PostToolUse hook. After each tool call, checks context window usage.

Thresholds:
- **60% (~120k tokens)**: WARNING — notify user to prepare for summarize
- **65% (~130k tokens)**: AUTO-SUMMARIZE — trigger full summarize ritual
- Multi-agent mode: also broadcasts checkpoint to all teammates, waits for RLM saves, then shuts down team

**File**: `~/.claude/hooks/context-monitor.ps1`

See [examples/hooks/context-monitor.ps1](../../examples/hooks/context-monitor.ps1) for full implementation.

## Autocapture Buffer

PreToolUse hook that logs every file operation (Edit/Write/Bash) to a buffer file.

```
~/.claude/autocapture-buffer.jsonl
```

Each line is a JSON record: `{"tool": "Edit", "file": "src/foo.py", "timestamp": "..."}`.

During `суммаризируем`, Claude reads this buffer to build an accurate file change log
even if context was lost. Buffer is cleared after each session.

## Statusline (Context Monitoring)

Script that shows context window % in your terminal statusline.

**File**: `~/.claude/statusline.ps1`

Install: configure your terminal (Windows Terminal / iTerm2 / etc.) to run this script
and show its output in the title bar or status area.

---

## Hook Configuration in Claude Code

Add hooks to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreCompact": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -File ~/.claude/hooks/pre-compact.ps1"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -File ~/.claude/hooks/context-monitor.ps1"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Edit|Write|Bash",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -File ~/.claude/hooks/autocapture.ps1"
          }
        ]
      }
    ]
  }
}
```

> **Note**: Hook paths and command syntax vary by OS. Linux/Mac users: replace
> `powershell -File` with `bash` and use `.sh` scripts.
