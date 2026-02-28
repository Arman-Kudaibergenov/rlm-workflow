# Auto-Capture Hook for RLM Workflow
# Event: PostToolUse — fires after every tool call.
# Silently records significant mutations (Edit, Write, notable Bash) to a JSONL buffer.
# Buffer is flushed to RLM during "суммаризируем" ritual.
#
# Buffer path: $USERPROFILE\.claude\autocapture-buffer.jsonl
# Installation: add to ~/.claude/settings.json under "PostToolUse" hooks
# See: docs/ru/хуки.md

param()

$ErrorActionPreference = 'SilentlyContinue'

$bufferFile = "$env:USERPROFILE\.claude\autocapture-buffer.jsonl"

try {
    $inputData = [Console]::In.ReadToEnd()
    if ([string]::IsNullOrWhiteSpace($inputData)) { exit 0 }

    $event = $inputData | ConvertFrom-Json -ErrorAction Stop

    $toolName = $event.tool_name
    if ([string]::IsNullOrWhiteSpace($toolName)) { exit 0 }

    $entry = $null

    switch ($toolName) {
        "Edit" {
            $filePath = $event.tool_input.file_path
            if ($filePath) {
                $entry = [ordered]@{
                    ts      = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
                    tool    = "Edit"
                    file    = $filePath
                    session = $event.session_id
                }
            }
        }
        "Write" {
            $filePath = $event.tool_input.file_path
            if ($filePath) {
                $entry = [ordered]@{
                    ts      = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
                    tool    = "Write"
                    file    = $filePath
                    session = $event.session_id
                }
            }
        }
        "Bash" {
            $cmd = $event.tool_input.command
            # Capture only significant commands: git commits, builds, tests
            if ($cmd -match "(git commit|git push|git tag|git merge|npm run build|dotnet build|pytest|cargo build|make |mvn |gradle )") {
                $cmdShort = ($cmd -replace '\s+', ' ').Trim()
                if ($cmdShort.Length -gt 200) { $cmdShort = $cmdShort.Substring(0, 200) + "..." }
                $entry = [ordered]@{
                    ts      = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
                    tool    = "Bash"
                    cmd     = $cmdShort
                    session = $event.session_id
                }
            }
        }
    }

    if ($entry) {
        ($entry | ConvertTo-Json -Compress) | Add-Content -Path $bufferFile -Encoding UTF8
    }

} catch {
    # Never block — hook must be silent on error
    exit 0
}

exit 0
