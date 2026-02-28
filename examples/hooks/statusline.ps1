# Claude Code StatusLine Script
# Line 1: Model | Project | tokens used/total | % used <fullused> | % remain <fullremain> | thinking status: on/off
# Line 2: Current (5h): <progressbar> | Weekly (7d): <progressbar>
# Line 3: reset: Current | reset: Weekly | reset: Monthly
 
try {
    $inputText = $input | Out-String
    if (-not $inputText) {
        $inputText = [Console]::In.ReadToEnd()
    }
 
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
 
    if ($inputText) {
        $json = $inputText | ConvertFrom-Json
 
        # Model name
        $modelName = $json.model.display_name
        if (-not $modelName) { $modelName = "Claude" }

        # Project name from current working directory
        $projectName = ""
        if ($json.cwd) {
            $projectName = Split-Path $json.cwd -Leaf
        }
        if (-not $projectName) { $projectName = "no-project" }

        # ANSI escape character
        $e = [char]27
 
        # Colors matching oh-my-posh theme
        $blue = "$e[38;2;0;153;255m"
        $orange = "$e[38;2;255;176;85m"
        $green = "$e[38;2;0;160;0m"
        $cyan = "$e[38;2;46;149;153m"
        $red = "$e[38;2;255;85;85m"
        $yellow = "$e[38;2;230;200;0m"
        $dim = "$e[2m"
        $reset = "$e[0m"
 
        # Format token counts (e.g., 50k / 200k)
        function Format-Tokens($num) {
            if ($num -ge 1000000) { return [math]::Round($num / 1000000, 1).ToString() + "m" }
            elseif ($num -ge 1000) { return [math]::Round($num / 1000, 0).ToString() + "k" }
            else { return $num.ToString() }
        }
 
        # Build a colored progress bar
        function Build-Bar($pct, $width) {
            if ($pct -lt 0) { $pct = 0 }
            if ($pct -gt 100) { $pct = 100 }
            $filled = [math]::Round($pct * $width / 100)
            $empty = $width - $filled
 
            # Color based on usage level
            if ($pct -ge 90) { $barColor = $red }
            elseif ($pct -ge 70) { $barColor = $yellow }
            elseif ($pct -ge 50) { $barColor = $orange }
            else { $barColor = $green }
 
            $filledStr = ""
            $emptyStr = ""
            if ($filled -gt 0) { $filledStr = [string]::new([char]0x25CF, $filled) }
            if ($empty -gt 0) { $emptyStr = [string]::new([char]0x25CB, $empty) }
 
            return "${barColor}${filledStr}${dim}${emptyStr}${reset}"
        }
 
        # Token calculations - default to 0 if no usage data
        $size = [int]($json.context_window.context_window_size)
        if ($size -eq 0) { $size = 200000 }  # Default context size
 
        $usage = $json.context_window.current_usage
        if ($usage) {
            $inputTokens = [int]($usage.input_tokens)
            $cacheCreate = [int]($usage.cache_creation_input_tokens)
            $cacheRead = [int]($usage.cache_read_input_tokens)
            $current = $inputTokens + $cacheCreate + $cacheRead
        } else {
            $current = 0
        }
 
        $usedTokens = Format-Tokens $current
        $totalTokens = Format-Tokens $size
 
        if ($size -gt 0) {
            $pctUsed = [math]::Round(($current / $size) * 100)
        } else {
            $pctUsed = 0
        }
        $pctRemain = 100 - $pctUsed

        # Write context state for context-monitor hook
        try {
            @{ pct = $pctUsed; tokens = $current; limit = $size } |
                ConvertTo-Json -Compress |
                Set-Content "$env:TEMP\claude-ctx-state.json" -Force -NoNewline
        } catch {}

        # Check thinking status from settings file
        $thinkingOn = $false
        $settingsPath = "$env:USERPROFILE\.claude\settings.json"
        if (Test-Path $settingsPath) {
            $settings = Get-Content $settingsPath -Raw | ConvertFrom-Json
            $thinkingOn = ($settings.alwaysThinkingEnabled -eq $true)
        }
 
        # ===== MCP Servers status =====
        # Three-tier verification:
        #   green  name✓ = MCP tool was actually called this session (verified via mcp-stats)
        #   yellow name~ = HTTP health OK but MCP protocol not confirmed
        #   red    name✗ = HTTP unreachable
        #   dim    name? = command-based server, not called yet (unknown)
        $mcpStatuses = @()
        $mcpConfigs = @()

        # Note: PostToolUse hook does not fire for MCP tools in Claude Code,
        # so verified-by-call tracking is not possible. Using TCP check only.

        # Global MCP config
        $globalMcpPath = Join-Path $env:USERPROFILE ".claude\mcp.json"
        if (Test-Path $globalMcpPath) { $mcpConfigs += $globalMcpPath }

        # Project MCP config — look in cwd or parent directories
        if ($json.cwd) {
            $searchDir = $json.cwd
            for ($i = 0; $i -lt 5; $i++) {
                $candidate = Join-Path $searchDir ".mcp.json"
                if (Test-Path $candidate) {
                    $mcpConfigs += $candidate
                    break
                }
                $parent = Split-Path $searchDir -Parent
                if (-not $parent -or $parent -eq $searchDir) { break }
                $searchDir = $parent
            }
        }

        $seenServers = @{}
        foreach ($mcpConfigPath in $mcpConfigs) {
            try {
                $mcpConfig = Get-Content $mcpConfigPath -Raw | ConvertFrom-Json
                $servers = $mcpConfig.mcpServers.PSObject.Properties

                foreach ($srv in $servers) {
                    $name = $srv.Name
                    if ($seenServers.ContainsKey($name)) { continue }
                    $seenServers[$name] = $true
                    $url  = $srv.Value.url
                    # Command-based servers (e.g. playwright, local LSP bridges) have no URL
                    if (-not $url) {
                        $mcpStatuses += "${dim}${name}?$reset"
                        continue
                    }

                    # TCP connectivity check
                    $uri = [System.Uri]$url
                    $online = $false
                    try {
                        $tcp = New-Object System.Net.Sockets.TcpClient
                        $r = $tcp.BeginConnect($uri.Host, $uri.Port, $null, $null)
                        $ok = $r.AsyncWaitHandle.WaitOne(1500, $false)
                        $online = ($ok -and $tcp.Connected)
                        $tcp.Close()
                    } catch { $online = $false }

                    if ($online) {
                        $mcpStatuses += "${green}${name}$reset"
                    } else {
                        $mcpStatuses += "${red}${name}$([char]0x2717)$reset"
                    }
                }
            } catch {
                # Silently ignore MCP config parse errors
            }
        }

        # ===== LINE 1: Model | Project | tokens | % used | % remain | thinking | MCP =====
        $line1 = "$blue$modelName$reset"
        $line1 += " $dim|$reset "
        $line1 += "$cyan$projectName$reset"
        $line1 += " $dim|$reset "
        $line1 += "$orange$usedTokens / $totalTokens$reset"
        $line1 += " $dim|$reset "
        $usedComma = $current.ToString("N0")
        $remainComma = ($size - $current).ToString("N0")
        $line1 += "$green$pctUsed% used ${orange}$usedComma$reset"
        $line1 += " $dim|$reset "
        $line1 += "$cyan$pctRemain% remain ${blue}$remainComma$reset"
        $line1 += " $dim|$reset "
        $line1 += "thinking: "
        if ($thinkingOn) {
            $line1 += "${orange}On$reset"
        } else {
            $line1 += "${dim}Off$reset"
        }

        # ===== LINE 1.5: MCP servers (separate line when servers present) =====
        $lineMcp = ""
        if ($mcpStatuses.Count -gt 0) {
            # Split into two rows of equal halves when more than 4 servers
            $half = [math]::Ceiling($mcpStatuses.Count / 2)
            if ($mcpStatuses.Count -gt 4) {
                $row1 = $mcpStatuses[0..($half - 1)]
                $row2 = $mcpStatuses[$half..($mcpStatuses.Count - 1)]
                $lineMcp  = "${dim}mcp:$reset " + ($row1 -join " ${dim}+$reset ")
                $lineMcp += "`n     " + ($row2 -join " ${dim}+$reset ")
            } else {
                $lineMcp = "${dim}mcp:$reset " + ($mcpStatuses -join " ${dim}+$reset ")
            }
        }

        # ===== LINE 2: Usage limits with progress bars (cached) =====
        $cacheFile = "$env:TEMP\claude-statusline-usage-cache.json"
        $cacheMaxAge = 60  # seconds between API calls
 
        $usageData = $null
        $needsRefresh = $true
 
        # Check cache
        if (Test-Path $cacheFile) {
            $cacheAge = ((Get-Date) - (Get-Item $cacheFile).LastWriteTime).TotalSeconds
            if ($cacheAge -lt $cacheMaxAge) {
                $needsRefresh = $false
                try {
                    $usageData = Get-Content $cacheFile -Raw | ConvertFrom-Json
                } catch {
                    $needsRefresh = $true
                }
            }
        }
 
        # Fetch fresh data if cache is stale
        if ($needsRefresh) {
            try {
                $credsPath = "$env:USERPROFILE\.claude\.credentials.json"
                if (Test-Path $credsPath) {
                    $creds = Get-Content $credsPath -Raw | ConvertFrom-Json
                    $token = $creds.claudeAiOauth.accessToken
                    $headers = @{
                        'Accept'         = 'application/json'
                        'Content-Type'   = 'application/json'
                        'Authorization'  = "Bearer $token"
                        'anthropic-beta' = 'oauth-2025-04-20'
                        'User-Agent'     = "claude-code/$(if ($env:CLAUDE_CODE_VERSION) { $env:CLAUDE_CODE_VERSION } else { '2.1.34' })"
                    }
                    $response = Invoke-RestMethod -Uri 'https://api.anthropic.com/api/oauth/usage' -Headers $headers -Method Get -TimeoutSec 5
                    $usageData = $response
                    # Write cache
                    $response | ConvertTo-Json -Depth 5 | Set-Content $cacheFile -Force
                }
            } catch {
                # Silently fail - use stale cache if available
                if (Test-Path $cacheFile) {
                    try { $usageData = Get-Content $cacheFile -Raw | ConvertFrom-Json } catch {}
                }
            }
        }
 
        # Format ISO reset time to compact local time
        function Format-ResetTime($isoString, $style) {
            if (-not $isoString) { return "" }
            try {
                $utc = [DateTimeOffset]::Parse($isoString)
                $local = $utc.ToLocalTime()
                if ($style -eq "time") {
                    return $local.ToString("h:mmtt").ToLower()
                } elseif ($style -eq "datetime") {
                    return $local.ToString("MMM d, h:mmtt").ToLower()
                }
                return $local.ToString("MMM d").ToLower()
            } catch { return "" }
        }
 
        # Pad visible text to a fixed column width (ignoring ANSI codes)
        function Pad-Column($text, $visibleLen, $colWidth) {
            $padding = $colWidth - $visibleLen
            if ($padding -gt 0) { return $text + (" " * $padding) }
            return $text
        }
 
        $white = "$e[38;2;220;220;220m"
        $sep = " $dim|$reset "
 
        $line2 = ""
        $line3 = ""
        if ($usageData) {
            $barWidth = 10
            $col1w = 23
            $col2w = 22
 
            # ---- 5-hour (current) ----
            $fiveHourPct = 0; $fiveHourReset = ""
            if ($usageData.five_hour -and $null -ne $usageData.five_hour.utilization) {
                $fiveHourPct = [math]::Round([double]$usageData.five_hour.utilization)
                $fiveHourReset = Format-ResetTime $usageData.five_hour.resets_at "time"
            }
            $fiveHourBar = Build-Bar $fiveHourPct $barWidth
            # visible: "current: ●●●●●●●●●● 100%" = 9+10+1+4 = 24 max
            $col1BarVis = "current: " + ("x" * $barWidth) + " ${fiveHourPct}%"
            $col1Bar = "${white}current:$reset $fiveHourBar ${cyan}${fiveHourPct}%$reset"
            $col1Bar = Pad-Column $col1Bar $col1BarVis.Length $col1w
            # reset line
            $col1Reset = "resets $fiveHourReset"
            $col1ResetColored = "${white}resets $fiveHourReset$reset"
            $col1ResetColored = Pad-Column $col1ResetColored $col1Reset.Length $col1w
 
            # ---- 7-day (weekly) ----
            $sevenDayPct = 0; $sevenDayReset = ""
            if ($usageData.seven_day -and $null -ne $usageData.seven_day.utilization) {
                $sevenDayPct = [math]::Round([double]$usageData.seven_day.utilization)
                $sevenDayReset = Format-ResetTime $usageData.seven_day.resets_at "datetime"
            }
            $sevenDayBar = Build-Bar $sevenDayPct $barWidth
            $col2BarVis = "weekly: " + ("x" * $barWidth) + " ${sevenDayPct}%"
            $col2Bar = "${white}weekly:$reset $sevenDayBar ${cyan}${sevenDayPct}%$reset"
            $col2Bar = Pad-Column $col2Bar $col2BarVis.Length $col2w
            $col2Reset = "resets $sevenDayReset"
            $col2ResetColored = "${white}resets $sevenDayReset$reset"
            $col2ResetColored = Pad-Column $col2ResetColored $col2Reset.Length $col2w
 
            # ---- Extra usage ----
            $col3Bar = ""; $col3ResetColored = ""
            if ($usageData.extra_usage -and $usageData.extra_usage.is_enabled) {
                $extraPct = [math]::Round([double]$usageData.extra_usage.utilization)
                $extraUsed = [math]::Round([double]$usageData.extra_usage.used_credits / 100, 2)
                $extraLimit = [math]::Round([double]$usageData.extra_usage.monthly_limit / 100, 2)
                $extraBar = Build-Bar $extraPct $barWidth
                $nextMonth = (Get-Date).AddMonths(1)
                $extraReset = (Get-Date -Year $nextMonth.Year -Month $nextMonth.Month -Day 1).ToString("MMM d").ToLower()
 
                $col3Bar = "${white}extra:$reset $extraBar ${cyan}`$$extraUsed/`$$extraLimit$reset"
                $col3ResetColored = "${white}resets $extraReset$reset"
            }
 
            # Assemble line 2: bars row
            $line2 = $col1Bar + $sep + $col2Bar
            if ($col3Bar) { $line2 += $sep + $col3Bar }
 
            # Assemble line 3: resets row
            $line3 = $col1ResetColored + $sep + $col2ResetColored
            if ($col3ResetColored) { $line3 += $sep + $col3ResetColored }
        }
 
        # Output all lines
        [Console]::Write($line1)
        if ($lineMcp) { [Console]::Write("`n$lineMcp") }
        if ($line2) { [Console]::Write("`n$line2") }
        if ($line3) { [Console]::Write("`n$line3") }
    } else {
        [Console]::Write("Claude")
    }
} catch {
    [Console]::Write("Claude | Error: $_")
}