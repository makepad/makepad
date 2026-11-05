# Makepad Runner on Windows, in real windows, from a checkout: its list, a live run, and Stop.
#
# Windows differs here: no process can stand in a virtual folder, a live run stands behind a thin
# parent without a window (restart_parent.rs), and there is no signal to end it with, so what works
# on the other systems breaks here unseen.
#
# usage: powershell -NoProfile -ExecutionPolicy Bypass -File tools/run/fixture/runner_windows.ps1
#   (from the workspace root, with the commercial apps at apps/commercial)
#
# With a fresh home: passes the first-start card, selects Task, runs it live, checks that it drew
# and that its card says it runs, presses Stop and checks the whole tree ends without a failure.
# Then Enter (a double click's path) does the same, then a run frozen before Stop. Then adds Task to a Start Menu under the temp
# folder, reads the shortcut as the shell does, starts it and removes it.
# Prints "runner_windows: OK ..." or "runner_windows: FAIL ..."; exit 1 on FAIL.
$ErrorActionPreference = 'Continue'
$Root = (Get-Location).Path
$Target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Makepad = [IO.Path]::GetFullPath((Join-Path $Target 'host\makepad.exe'))
if (-not $env:MAKEPAD_RUN_STD) { $env:MAKEPAD_RUN_STD = Join-Path $Root 'libs\relax\compiler\std' }
Remove-Item env:MAKEPAD_HIDE_WINDOWS -ErrorAction SilentlyContinue

cargo build --profile host -q --manifest-path (Join-Path $Root 'Cargo.toml') -p makepad-run --bin makepad
if ($LASTEXITCODE -ne 0) { 'runner_windows: FAIL no build'; exit 1 }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("runner-windows-" + [Guid]::NewGuid().ToString('N'))
$home_ = Join-Path $tmp 'home'
$null = New-Item -ItemType Directory -Force $home_
$outFile = Join-Path $tmp 'out.txt'
$errFile = Join-Path $tmp 'err.txt'
$env:MAKEPAD_RUN_HOME = $home_
$env:MAKEPAD_HOME = Join-Path $tmp 'makepad'
$env:APPDATA = Join-Path $tmp 'appdata'
$link = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Makepad\Task.lnk'
$report = Join-Path $home_ 'runner\launch\task.report'
$log = Join-Path $home_ 'runner\launch\task.log'
$p = Start-Process -FilePath $Makepad -ArgumentList 'run', 'makepad-runner' -WorkingDirectory $Root -NoNewWindow -PassThru -RedirectStandardOutput $outFile -RedirectStandardError $errFile
$null = $p.Handle

function Lines($path) {
    # the run holds the file open and keeps writing
    if (-not (Test-Path $path)) { return @() }
    $f = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite,Delete')
    $r = New-Object IO.StreamReader($f)
    $text = $r.ReadToEnd()
    $r.Close()
    return @($text -split "`r?`n" | Where-Object { $_ -ne '' })
}
function Kids($id) { @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$id and Name<>'conhost.exe'") }
# The tree under a process, itself first: a live run's children end with it.
function Tree($id) { @($id) + @(Kids $id | ForEach-Object { Tree ([int]$_.ProcessId) }) }
# (the pid held is the parent's: the discovery file of the run behind it names it)
function Ui { & $Makepad inspect $p.Id ui 2>&1 | Out-String }
function Act {
    $answer = & $Makepad inspect $p.Id @args 2>&1 | Out-String
    if ($answer -match '"err"') { throw "inspect $args`: $answer" }
    return $answer
}
function Until($what, $pred) {
    while (-not (& $pred)) {
        if ($p.HasExited) { throw "the runner ended while waiting for $what" }
        Start-Sleep -Milliseconds 100
    }
}
function Fresh { if (Test-Path $report) { Remove-Item $report -Force } }

# One run of Task, started by $start: it draws, says so, and Stop ends all of it.
function Run-And-Stop($how, $start) {
    Fresh
    & $start | Out-Null
    Until "$how to start" { @(Lines $report) -match '^(present|exit\t)' }
    $seen = @(Lines $report)
    if (-not ($seen -contains 'present')) { throw "$how did not draw: $($seen -join ' / ') $(Lines $log | Select-Object -Last 20)" }
    $errors = @($seen -match '^(error|panic)\t')
    if ($errors.Count -gt 0) { throw "$how reported: $($errors -join ' / ')" }
    Until "$how to show as running" { (Ui) -match 'click to stop' }
    $pid_ = [int](($seen -match '^pid\t')[0] -split "`t")[1]
    $tree = @(Tree $pid_)
    # the one press that stops it: the card's own
    Act ui act press 'q=name~=live' w=0 | Out-Null
    Until "$how to end" { @($tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count -eq 0 }
    Until "$how to show as stopped" { (Ui) -notmatch 'click to stop' }
    $ui = Ui
    if ($ui -match "Didn't open|taskkill|exit code") { throw "$how ended with a failure on its tile: $ui" }
    return "$how pid $pid_ tree of $($tree.Count)"
}

# A frozen run: one press of Stop ends it, with no failure on its tile. The wait is bounded so a
# Stop that does nothing fails here rather than holding the screen.
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class Freeze { [DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr process); }'
function Frozen-Stop {
    Fresh
    Act ui act press 'q=name~=live' w=0 | Out-Null
    Until 'a run to freeze' { @(Lines $report) -match '^(present|exit\t)' }
    Until 'it to show as running' { (Ui) -match 'click to stop' }
    $pid_ = [int]((@(Lines $report) -match '^pid\t')[0] -split "`t")[1]
    $tree = @(Tree $pid_)
    if ($tree.Count -lt 2) { throw "no run under the parent: $($tree -join ',')" }
    $run = Get-Process -Id $tree[1]
    if ([Freeze]::NtSuspendProcess($run.Handle) -ne 0) { throw "could not freeze $($tree[1])" }
    Act ui act press 'q=name~=live' w=0 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (@($tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count -gt 0) {
        if ($sw.Elapsed.TotalSeconds -gt 60) { throw "Stop did not end the frozen run in 60 s" }
        Start-Sleep -Milliseconds 100
    }
    $took = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
    Until 'it to show as stopped' { (Ui) -notmatch 'click to stop' }
    $ui = Ui
    if ($ui -match "Didn't open|taskkill|exit code") { throw "the frozen run ended with a failure on its tile: $ui" }
    return "the frozen run in $took s"
}

# The Start Menu entry: the shell must read a target from it and starting it must run the app.
function Started { @(Get-CimInstance Win32_Process -Filter "Name='makepad.exe'" | Where-Object { $_.ExecutablePath -eq $Makepad -and $_.CommandLine -match 'app task' }) }
function Pin-And-Start {
    Act ui act press role=button 'q=name~=Start' w=0 | Out-Null
    Until 'the shortcut' { Test-Path $link }
    $l = (New-Object -ComObject WScript.Shell).CreateShortcut($link)
    if ($l.TargetPath -ne $Makepad) { throw "the shortcut's target is '$($l.TargetPath)', not $Makepad" }
    if ($l.Arguments -ne 'app task') { throw "the shortcut's arguments are '$($l.Arguments)'" }
    Invoke-Item $link
    Until 'the shortcut to start the app' { @(Started).Count -gt 0 }
    foreach ($a in @(Started)) { taskkill /T /F /PID $a.ProcessId 2>&1 | Out-Null }
    Act ui act press role=button q=name=Remove w=0 | Out-Null
    Until 'the shortcut to go' { -not (Test-Path $link) }
    return 'the Start Menu entry'
}

$failed = $null
$said = @()
try {
    Until 'the app list' { (Ui) -match 'button "Task"' }
    if ((Ui) -match 'button "Continue"') {
        Act ui act press q=name=Continue w=0 | Out-Null
        Until 'the first-start card to go' { (Ui) -notmatch 'button "Continue"' }
    }
    Act ui act press role=button q=name=Task w=0 | Out-Null
    Until 'the detail pane' { (Ui) -match 'static_text "Run Live"' }
    $said += Run-And-Stop 'Run Live' { Act ui act press 'q=name~=live' w=0 }
    # the tile takes the keys when pressed, and a card's press took them from it: Enter runs what a
    # double click runs
    $said += Run-And-Stop 'Enter' { Act ui act press role=button q=name=Task w=0; Act key c=ReturnKey }
    $said += Frozen-Stop
    $said += Pin-And-Start
} catch {
    $failed = "$_"
} finally {
    if (-not $p.HasExited) { taskkill /T /F /PID $p.Id 2>&1 | Out-Null }
    # the shell's start is not in the runner's tree
    foreach ($a in @(Started)) { taskkill /T /F /PID $a.ProcessId 2>&1 | Out-Null }
}
$tail = if ($failed) { Lines $errFile | Select-Object -Last 30 }
Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
if ($failed) {
    "runner_windows: FAIL $failed"
    $tail
    exit 1
}
"runner_windows: OK $($said -join ', ')"
exit 0
