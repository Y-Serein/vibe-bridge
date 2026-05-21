param(
    [int]$Seconds = 30
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = "Stop"

$State = Join-Path $env:LOCALAPPDATA "VibeBridge\state.json"
if (-not (Test-Path $State)) {
    throw "state file not found: $State"
}

function Decode-Input([string]$Event) {
    if ($Event -notmatch 'key bits=0x([0-9a-fA-F]+) enc=([01])') {
        return $Event
    }
    $bits = [Convert]::ToInt32($Matches[1], 16)
    $names = @("REJECT", "VOICE", "SESSION", "VOTE_REVIEW", "AGENT_MODEL", "MULTI_FUNCTION", "CONFIRM", "MENU_DEBUG")
    $pressed = @()
    for ($i = 0; $i -lt $names.Count; $i++) {
        if (($bits -band (1 -shl $i)) -ne 0) {
            $pressed += $names[$i]
        }
    }
    if ($Matches[2] -eq "1") {
        $pressed += "ENCODER_PRESS"
    }
    if ($pressed.Count -eq 0) {
        return "none"
    }
    return ($pressed -join ", ")
}

function Get-JsonString($Object, [string]$Name, [string]$Default = "") {
    if ($null -eq $Object) {
        return $Default
    }
    $prop = $Object.PSObject.Properties[$Name]
    if ($null -eq $prop -or $null -eq $prop.Value) {
        return $Default
    }
    return [string]$prop.Value
}

function Get-JsonInt($Object, [string]$Name, [int]$Default = 0) {
    if ($null -eq $Object) {
        return $Default
    }
    $prop = $Object.PSObject.Properties[$Name]
    if ($null -eq $prop -or $null -eq $prop.Value) {
        return $Default
    }
    try {
        return [int]$prop.Value
    } catch {
        return $Default
    }
}

Write-Host "Watching board input for $Seconds seconds."
Write-Host "Press SESSION, rotate encoder, press encoder, then press each key once."
Write-Host ""

$deadline = (Get-Date).AddSeconds($Seconds)
$lastSeq = -1
$lastEvent = ""
while ((Get-Date) -lt $deadline) {
    try {
        $stateJson = Get-Content $State -Raw | ConvertFrom-Json
        $event = Get-JsonString $stateJson "last_board_event"
        $seq = Get-JsonInt $stateJson "last_board_event_seq" 0
        $changed = $false
        if ($seq -ne 0) {
            $changed = ($seq -ne $lastSeq)
        } else {
            $changed = ($event -ne $lastEvent)
        }
        if ($event -and $changed) {
            $lastSeq = $seq
            $lastEvent = $event
            $decoded = Decode-Input $event
            $panel = Get-JsonString $stateJson "board_panel"
            $active = Get-JsonString $stateJson "active_sid"
            Write-Host ("{0:HH:mm:ss.fff}  #{1,-4} {2,-28}  decoded={3}  panel={4} active={5}" -f (Get-Date), $seq, $event, $decoded, $panel, $active)
        }
    } catch {
        Write-Host "read state failed: $_"
    }
    Start-Sleep -Milliseconds 150
}
