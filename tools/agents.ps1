$ErrorActionPreference = 'Stop'
$repoDir = Split-Path -Parent $PSScriptRoot
Push-Location $repoDir
try {
    cargo build --release -p makepad-screen
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
$screenTargetDir = $env:CARGO_TARGET_DIR
if (-not $screenTargetDir) { $screenTargetDir = Join-Path $repoDir 'target' }
elseif (-not [System.IO.Path]::IsPathRooted($screenTargetDir)) {
    $screenTargetDir = Join-Path $repoDir $screenTargetDir
}
$workspaceSessions = Join-Path $repoDir 'local/agent_state/studio/iteration-verification/state/agent_sessions'
if (-not $env:MAKEPAD_SCREEN_STATE_DIR -and (Test-Path -PathType Container $workspaceSessions)) {
    $env:MAKEPAD_SCREEN_STATE_DIR = $workspaceSessions
}
& (Join-Path $screenTargetDir 'release/makepad-screen.exe') agents @args
exit $LASTEXITCODE
