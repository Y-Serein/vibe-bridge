param(
    [string]$Sock = "tcp://127.0.0.1:8765",
    [string]$Distro = "Ubuntu-22.04",
    [string]$CodexPath = "/home/rv_nano/.nvm/versions/node/v22.22.2/bin/codex",
    [int]$SessionCount = 2,
    [switch]$NoStartDaemon,
    [switch]$NoLaunchSessions
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = "Stop"

$Repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Src = (Resolve-Path (Join-Path $Repo "src")).Path
$State = Join-Path $env:LOCALAPPDATA "VibeBridge\state.json"
$env:PYTHONPATH = $Src
Set-Location $Repo

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "== $Message ==" -ForegroundColor Cyan
}

function Test-TcpEndpoint([string]$Endpoint) {
    if ($Endpoint -notmatch '^tcp://([^:]+):(\d+)$') {
        return $false
    }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $task = $client.ConnectAsync($Matches[1], [int]$Matches[2])
        if (-not $task.Wait(800)) {
            return $false
        }
        return $client.Connected
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

function Split-TcpEndpoint([string]$Endpoint) {
    if ($Endpoint -notmatch '^tcp://([^:]+):(\d+)$') {
        throw "Only tcp://HOST:PORT endpoints are supported by this smoke script: $Endpoint"
    }
    return @{ Host = $Matches[1]; Port = [int]$Matches[2] }
}

function Invoke-Bridge([string[]]$ArgsList, [switch]$AllowFail) {
    & python -m vibe_bridge.main @ArgsList
    $code = $LASTEXITCODE
    if ($code -ne 0 -and -not $AllowFail) {
        throw "python -m vibe_bridge.main $($ArgsList -join ' ') failed with exit code $code"
    }
    return $code
}

function Start-BridgePowerShell([string]$Command) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Command))
    Start-Process powershell.exe -ArgumentList @(
        "-NoExit",
        "-ExecutionPolicy", "Bypass",
        "-EncodedCommand", $encoded
    ) | Out-Null
}

Write-Step "vibe-bridge Windows smoke"
Write-Host "repo  : $Repo"
Write-Host "sock  : $Sock"
Write-Host "state : $State"
Write-Host "wsl   : $Distro"
Write-Host "codex : $CodexPath"

if (-not (Test-TcpEndpoint $Sock)) {
    if ($NoStartDaemon) {
        throw "daemon is not reachable at $Sock and -NoStartDaemon was set"
    }
    $endpoint = Split-TcpEndpoint $Sock
    Write-Step "starting windows daemon"
    $daemonCommand = @"
cd "$Repo"
`$env:PYTHONPATH = "$Src"
python -m vibe_bridge.main -v windows daemon --host "$($endpoint.Host)" --port $($endpoint.Port)
"@
    Start-BridgePowerShell $daemonCommand

    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-TcpEndpoint $Sock) {
            $ready = $true
            break
        }
    }
    if (-not $ready) {
        throw "daemon did not become reachable at $Sock"
    }
} else {
    Write-Step "daemon already reachable"
}

Write-Step "windows doctor"
Invoke-Bridge @("windows", "doctor") -AllowFail | Out-Null

Write-Step "current sessions"
Invoke-Bridge @("--sock", $Sock, "sessions", "--state", $State) -AllowFail | Out-Null

if (-not $NoLaunchSessions) {
    Write-Step "launching WSL Codex sessions"
    for ($i = 1; $i -le $SessionCount; $i++) {
        $sessionCommand = @"
cd "$Repo"
`$env:PYTHONPATH = "$Src"
python -m vibe_bridge.main windows wsl-cli --ipc "$Sock" --distro "$Distro" -- "$CodexPath"
"@
        Start-BridgePowerShell $sessionCommand
        Write-Host "launched session window $i/$SessionCount"
        Start-Sleep -Milliseconds 800
    }
    Start-Sleep -Seconds 3
}

Write-Step "sessions after launch"
Invoke-Bridge @("--sock", $Sock, "sessions", "--state", $State) -AllowFail | Out-Null

Write-Step "board-side manual check"
Write-Host "1. Press SESSION on the board."
Write-Host "2. Rotate the encoder; the focused wsl-cli row should move."
Write-Host "3. Press encoder/confirm; the board should enter that Codex terminal."
Write-Host "4. The screen should not show raw JSON like {`"type`":`"clear`"}."
Write-Host ""
Write-Host "To close a launched session window, focus that PowerShell window and press Ctrl+C."
