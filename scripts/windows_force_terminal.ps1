param(
    [int]$Sid = 0,
    [string]$Sock = "tcp://127.0.0.1:8765"
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = "Stop"

$Repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Src = (Resolve-Path (Join-Path $Repo "src")).Path
$State = Join-Path $env:LOCALAPPDATA "VibeBridge\state.json"
$env:PYTHONPATH = $Src
Set-Location $Repo

if ($Sid -le 0) {
    if (-not (Test-Path $State)) {
        throw "state file not found: $State"
    }
    $stateJson = Get-Content $State -Raw | ConvertFrom-Json
    $Sid = [int]$stateJson.active_sid
    if ($Sid -le 0) {
        throw "no active sid in $State"
    }
}

python -m vibe_bridge.main --sock $Sock window-activate --sid $Sid
python -m vibe_bridge.main --sock $Sock sessions --state $State
