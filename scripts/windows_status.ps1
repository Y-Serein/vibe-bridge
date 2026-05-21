param(
    [string]$Sock = "tcp://127.0.0.1:8765"
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = "Stop"

$Repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Src = (Resolve-Path (Join-Path $Repo "src")).Path
$State = Join-Path $env:LOCALAPPDATA "VibeBridge\state.json"
$env:PYTHONPATH = $Src
Set-Location $Repo

python -m vibe_bridge.main --sock $Sock sessions --state $State
