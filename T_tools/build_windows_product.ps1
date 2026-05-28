param(
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $repoRoot "D_deliverables\windows"
$targetDir = Join-Path $outDir "build-target"
$targetExe = Join-Path $targetDir "release\vb-daemon.exe"
$setupExe = Join-Path $outDir "VibeBridgeSetup.exe"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw "Rust/Cargo is required. Install Rust stable with rustup on Windows first."
}

Push-Location $repoRoot
try {
  Write-Host "cargo: $(cargo -V)"
  Write-Host "target: $targetDir"
  Write-Host ""

  if (-not $SkipBuild) {
    $env:CARGO_TARGET_DIR = $targetDir
    cargo build -p vb-daemon --release
    if ($LASTEXITCODE -ne 0) {
      throw "cargo build failed with exit code $LASTEXITCODE"
    }
  }

  if (-not (Test-Path $targetExe)) {
    throw "Release binary not found: $targetExe. Run without -SkipBuild first."
  }

  New-Item -ItemType Directory -Force -Path $outDir | Out-Null
  Copy-Item -Force $targetExe $setupExe

  Write-Host ""
  Write-Host "Product artifact:"
  Write-Host $setupExe
  Write-Host ""
  Write-Host "Run VibeBridgeSetup.exe to install or repair Vibe Bridge."
  Write-Host "The installer copies a versioned daemon into LOCALAPPDATA, starts it,"
  Write-Host "adds Startup and Start Menu entries, installs WSL shell hooks,"
  Write-Host "and restores native Windows Terminal profiles by default."
} finally {
  Pop-Location
}
