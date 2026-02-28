# Context Monitor Hook for RLM Workflow
# Event: PostToolUse — fires after every tool call.
# Reads context % from statusline state file.
# At 70%: warning. At 80%: injects full суммаризируем instruction.
#
# Depends on: statusline.ps1 writing $TEMP\claude-ctx-state.json each turn.
# Installation: add to ~/.claude/settings.json under "PostToolUse" hooks
# See: docs/ru/хуки.md

param()

$ErrorActionPreference = 'SilentlyContinue'

# Adjust buffer path if your Claude home is different
$bufferFile = "$env:USERPROFILE\.claude\autocapture-buffer.jsonl"

try {
    $inputData = [Console]::In.ReadToEnd()
    if ([string]::IsNullOrWhiteSpace($inputData)) { exit 0 }

    $event = $inputData | ConvertFrom-Json -ErrorAction Stop
    $sessionId = $event.session_id
    if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

    # Read context state written by statusline on every turn
    $ctxFile = "$env:TEMP\claude-ctx-state.json"
    if (-not (Test-Path $ctxFile)) { exit 0 }

    $fileAge = ((Get-Date) - (Get-Item $ctxFile).LastWriteTime).TotalSeconds
    if ($fileAge -gt 300) { exit 0 }  # stale > 5 min, skip

    $ctx = Get-Content $ctxFile -Raw | ConvertFrom-Json
    $pct = [int]$ctx.pct
    if ($pct -le 0) { exit 0 }

    # Per-session dedup — fire once per level per session
    $stateDir = "$env:TEMP\claude-ctx-alerts"
    if (-not (Test-Path $stateDir)) { New-Item -ItemType Directory -Path $stateDir | Out-Null }

    $warnFile = Join-Path $stateDir "warn_${sessionId}"
    $critFile = Join-Path $stateDir "crit_${sessionId}"

    $tokens = $ctx.tokens
    $limit  = $ctx.limit

    if ($pct -ge 80) {
        if (-not (Test-Path $critFile)) {
            New-Item -ItemType File -Path $critFile | Out-Null
            Write-Output @"
CONTEXT AUTO-SUMMARIZE: ${pct}% used (${tokens}/${limit} tokens). Execute full суммаризируем ritual NOW before responding to anything else:

IF active agent team exists (TeamCreate was called this session):
  A. SendMessage(type="broadcast", content="CHECKPOINT: Save your current state to RLM immediately (rlm_add_hierarchical_fact with what you've done, files changed, decisions, remaining). Then stop and wait.")
  B. Wait for teammates to confirm saves (or 30s timeout)
  C. SendMessage(type="shutdown_request") to each teammate
  D. Wait for shutdown confirmations

THEN (always):
1. Read autocapture buffer: cat '$bufferFile'
2. rlm_add_hierarchical_fact(content="TASK PROGRESS [task_id] auto-save at ${pct}%: Done: <summary>. Files: <list>. Decisions: <list>. Remaining: <list>.", domain="retrospective", level=1)
3. rlm_add_hierarchical_fact(content="PENDING tasks next session [task_id: <id>]: <numbered list or 'none'>", domain="workflow", level=1)
4. rlm_sync_state()
5. git status → commit if needed
6. Clear buffer: echo "" > "$bufferFile"
7. Tell user: "Контекст сохранён в RLM (${pct}%). Запусти /clear"
"@
        }
    } elseif ($pct -ge 70) {
        if (-not (Test-Path $warnFile)) {
            New-Item -ItemType File -Path $warnFile | Out-Null
            Write-Output "CONTEXT WARNING: ${pct}% used (${tokens}/${limit} tokens). Start wrapping up current task — суммаризируем will auto-trigger at 80%."
        }
    }

} catch {
    # Never block — hook must be silent on error
    exit 0
}

exit 0
