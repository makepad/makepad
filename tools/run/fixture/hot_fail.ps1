# A failed hot load ends in a warm restart and a ledger report, never a parked panic: hot_fail.py on
# Windows, where the boxes have no Python and a restart is another process under one parent
# (restart_parent.rs).
#
# usage: powershell -NoProfile -ExecutionPolicy Bypass -File tools/run/fixture/hot_fail.ps1
#   (from the workspace root; MAKEPAD_RUN_STD in the environment, default libs/relax/compiler/std)
#
# Runs hot_fail/ as hot_fail.py does, in its error and panic cases, with the same checks. Besides those:
#   - the restarted run has another pid, the same parent process, and answers `inspect` at its pid
#     and at the parent's, the one whoever started it holds;
#   - the discovery file of the first pid is gone (the hub reads one left behind as a crash);
#   - the error case: the run's exit code comes out of the parent, in the directory it started in;
#   - the panic case: killing the parent ends the run.
# Prints "hot_fail: <case>: OK ..." or "hot_fail: <case>: FAIL ..."; exit 1 on any FAIL.
$ErrorActionPreference = 'Continue'
$Root = (Get-Location).Path
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Makepad = Join-Path $Target 'host\makepad.exe'
$Bad = 'parked', 'paused on a panic', 'stale access', 'FATAL', 'Segmentation'
if (-not $env:MAKEPAD_RUN_STD) { $env:MAKEPAD_RUN_STD = Join-Path $Root 'libs\relax\compiler\std' }

cargo build --profile host -q --manifest-path (Join-Path $Root 'Cargo.toml') -p makepad-run --bin makepad
if ($LASTEXITCODE -ne 0) { 'hot_fail: FAIL no build'; exit 1 }

function Lines($path) {
    # the run holds the file open and keeps writing
    if (-not (Test-Path $path)) { return @() }
    $f = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite,Delete')
    $r = New-Object IO.StreamReader($f)
    $text = $r.ReadToEnd()
    $r.Close()
    return @($text -split "`r?`n" | Where-Object { $_ -ne '' })
}

function Case($name, $hooks, $killParent) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("hot-fail-" + [Guid]::NewGuid().ToString('N'))
    $pkg = Join-Path $tmp 'pkg'
    $null = New-Item -ItemType Directory -Force $tmp
    Copy-Item -Recurse (Join-Path $Here 'hot_fail') $pkg
    $home_ = Join-Path $tmp 'home'
    $ready = Join-Path $tmp 'ready'
    $outFile = Join-Path $tmp 'out.txt'
    $errFile = Join-Path $tmp 'err.txt'
    $env:MAKEPAD_RUN_LIVE_READY = $ready
    $env:MAKEPAD_RUN_HOME = $home_
    $env:MAKEPAD_HOME = Join-Path $tmp 'makepad'
    $env:MAKEPAD_RUN_CACHE_DIR = Join-Path $home_ 'cache'
    $env:MAKEPAD_RUN_TARGET_DIR = Join-Path $home_ 'target'
    $env:MAKEPAD_RUN_LIVE_TRACE = '1'
    foreach ($h in $hooks) { Set-Item "env:$h" 'hot_fail_target' }
    $p = Start-Process -FilePath $Makepad -ArgumentList 'run', '--ahead=minimal', 'relax-hot-fail' -WorkingDirectory $pkg -NoNewWindow -PassThru -RedirectStandardOutput $outFile -RedirectStandardError $errFile
    $null = $p.Handle # (ExitCode is only there with the handle taken at the start)

    $wait = {
        param($pred)
        while (-not (& $pred)) {
            if ($p.HasExited) { Start-Sleep -Milliseconds 500; return [bool](& $pred) }
            Start-Sleep -Milliseconds 20
        }
        return $true
    }
    $ticks = { @(Lines $outFile | Where-Object { $_ -like 'out *' -and $_ -notlike 'out restored*' }) }
    $restored = { @(Lines $outFile | Where-Object { $_ -like 'out restored *' } | ForEach-Object { [int64]($_ -split ' ')[2] }) }
    $pids = { @(Lines $outFile | Where-Object { $_ -like 'pid *' } | ForEach-Object { [int]($_ -split ' ')[1] }) }
    $ledger = {
        @(Get-ChildItem (Join-Path $tmp 'makepad\devhub\hot-outbox\*.json') -ErrorAction SilentlyContinue | ForEach-Object {
            try { Get-Content -Raw $_.FullName | ConvertFrom-Json } catch { }
        })
    }
    $parentOf = { param($id) (Get-CimInstance Win32_Process -Filter "ProcessId=$id").ParentProcessId }
    $outcomes = { @(& $ledger | ForEach-Object { $_.outcome }) }
    $registry = if ($env:MAKEPAD_REMOTE_REGISTRY) { $env:MAKEPAD_REMOTE_REGISTRY } else { [IO.Path]::GetTempPath() }

    $fails = New-Object Collections.ArrayList
    $before = -1
    $first = 0
    $now = 0
    try {
        $started = & $wait { (Test-Path $ready) -and @(& $ticks).Count -ge 20 -and @(@(Lines $errFile) -match 'watcher gate open').Count -gt 0 }
        if (-not $started) {
            $null = $fails.Add('no-start')
        } else {
            $seen = @(& $ticks)
            $before = [int64]($seen[-1] -split ' ')[1]
            $nTicks = $seen.Count
            $first = @(& $pids)[0]
            if ((& $parentOf $first) -ne $p.Id) { $null = $fails.Add('first-run-not-under-the-parent') }
            $source = Join-Path $pkg 'src\tick.rs'
            $text = [IO.File]::ReadAllText($source) -replace 'pub fn value\(\) -> u64 \{\r?\n    1\r?\n\}', "pub fn value() -> u64 {`n    hot_fail_target()`n}"
            [IO.File]::WriteAllText("$source.new", $text)
            [IO.File]::Replace("$source.new", $source, [NullString]::Value)
            $applied = { @(& $ticks | Select-Object -Skip $nTicks | Where-Object { $_ -like '*value=2*' }).Count -gt 0 }
            if (-not (& $wait { @(& $restored).Count -gt 0 -and (& $applied) })) {
                $null = $fails.Add($(if (@(& $restored).Count -gt 0) { 'no-new-code' } else { 'no-restored-state' }))
            } elseif (@(& $restored)[-1] -lt $before) {
                $null = $fails.Add('state-went-back')
            }
            if ($fails.Count -eq 0) {
                # the edit after the restart: the restarted run's watcher takes it (a recovery its
                # restore never ended parks the watcher, and every later edit is dropped)
                # (until its value shows, a record or refusal of it comes, or the run ends)
                $ids = @(& $ledger | ForEach-Object { $_.id })
                $nLater = @(& $ticks).Count
                $nErr = @(Lines $errFile).Count
                $text = [IO.File]::ReadAllText($source) -replace 'pub fn value\(\) -> u64 \{\r?\n    hot_fail_target\(\)\r?\n\}', "pub fn value() -> u64 {`n    hot_fail_target() + 1`n}"
                [IO.File]::WriteAllText("$source.new", $text)
                [IO.File]::Replace("$source.new", $source, [NullString]::Value)
                $later = { @(& $ticks | Select-Object -Skip $nLater | Where-Object { $_ -like '*value=3*' }).Count -gt 0 }
                $came = { @(& $ledger | Where-Object { $ids -notcontains $_.id }) }
                $refused = { @(Lines $errFile | Select-Object -Skip $nErr | Where-Object { $_ -like '*NOT applied*' }) }
                $verifiedAt = -1
                while (-not $p.HasExited -and -not (& $later) -and @(& $refused).Count -eq 0 -and @(& $came | Where-Object { $_.outcome -ne 'verified' }).Count -eq 0) {
                    # verified: its value is in the next ticks
                    if (@(& $came).Count -gt 0) {
                        if ($verifiedAt -lt 0) { $verifiedAt = @(& $ticks).Count }
                        if (@(& $ticks).Count -ge $verifiedAt + 10) { break }
                    }
                    Start-Sleep -Milliseconds 100
                }
                if (-not (& $later)) {
                    $what = "records $(@(& $came | ForEach-Object { $_.outcome }) -join ','); $(@(& $refused) + @('no refusal') | Select-Object -First 1); exited $($p.HasExited)"
                    $null = $fails.Add("later-edit-not-patched($what)")
                }
                elseif (@(& $pids).Count -ne 2) { $null = $fails.Add("later-edit-restarted-$(@(& $pids) -join ',')") }
            }
            if ($fails.Count -eq 0 -and -not (& $wait { (& $outcomes) -contains 'restarted' -or (& $outcomes) -contains 'restart_failed' })) { $null = $fails.Add("no-restart-record($(@(& $outcomes) -join ','))") }
            elseif ($fails.Count -eq 0 -and (& $outcomes) -notcontains 'restarted') { $null = $fails.Add("no-restart-record($(@(& $outcomes) -join ','))") }
            if ((& $outcomes) -notcontains 'panic') { $null = $fails.Add('no-failure-record') }
            $loads = @(& $ledger | Where-Object { $_.outcome -in 'panic', 'restarted' } | ForEach-Object { $_.load } | Sort-Object -Unique)
            if ($loads.Count -ne 1) { $null = $fails.Add("records-of-$($loads.Count)-loads") }
            if ($fails.Count -eq 0) {
                $all = @(& $pids)
                $now = $all[-1]
                if ($all.Count -ne 2 -or $now -eq $first) { $null = $fails.Add("pids-$($all -join ',')") }
                elseif ((& $parentOf $now) -ne $p.Id) { $null = $fails.Add('another-parent') }
                if (Test-Path (Join-Path $registry "makepad-remote-$first.json")) { $null = $fails.Add('first-discovery-file-left') }
                $answer = & $Makepad inspect $now stacks 2>&1 | Out-String
                if ($LASTEXITCODE -ne 0) { $null = $fails.Add("no-inspect: $($answer.Substring(0, [Math]::Min(200, $answer.Length)))") }
                $answer = & $Makepad inspect $p.Id stacks 2>&1 | Out-String
                if ($LASTEXITCODE -ne 0) { $null = $fails.Add("no-inspect-at-the-parent: $($answer.Substring(0, [Math]::Min(200, $answer.Length)))") }
            }
            if ($fails.Count -eq 0 -and $killParent) {
                $nKill = @(& $ticks).Count
                Stop-Process -Id $p.Id -Force
                # until the run is gone, or it ticks on a second (50 ticks) without its parent
                while ((Get-Process -Id $now -ErrorAction SilentlyContinue) -and @(& $ticks).Count -lt $nKill + 50) { Start-Sleep -Milliseconds 50 }
                if (Get-Process -Id $now -ErrorAction SilentlyContinue) { $null = $fails.Add('run-outlived-its-parent') }
            } elseif ($fails.Count -eq 0) {
                $null = New-Item -ItemType File (Join-Path $pkg 'exit-now')
                $p.WaitForExit()
                if ($p.ExitCode -ne 7) { $null = $fails.Add("exit-code-$($p.ExitCode)") }
            }
        }
        $wrong = Lines $errFile | Where-Object { $line = $_; $Bad | Where-Object { $line -like "*$_*" } } | Select-Object -First 1
        if ($wrong) { $null = $fails.Add("bad-line: $($wrong.Substring(0, [Math]::Min(200, $wrong.Length)))") }
    } catch {
        $null = $fails.Add("script: $_")
    } finally {
        if (-not $p.HasExited) { taskkill /T /F /PID $p.Id 2>&1 | Out-Null }
        foreach ($h in $hooks) { Remove-Item "env:$h" }
    }
    if ($fails.Count -gt 0) {
        "hot_fail: ${name}: FAIL $($fails -join ' ')"
        Lines $errFile | Select-Object -Last 40
        Lines $outFile | Select-Object -Last 5
        return $false
    }
    "hot_fail: ${name}: OK restored at tick $(@(& $restored)[-1]) (edit at $before), pid $first -> $now under $($p.Id), ledger $(@(& $outcomes | Sort-Object) -join ',')"
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    return $true
}

$ok = $true
foreach ($c in @(@('error', @('MAKEPAD_RUN_TEST_HOT_BUILD_ERROR'), $false), @('panic', @('MAKEPAD_RUN_TEST_HOT_BUILD_ERROR', 'MAKEPAD_RUN_TEST_HOT_COMPILE_PANIC'), $true))) {
    foreach ($line in (Case $c[0] $c[1] $c[2])) {
        if ($line -is [bool]) { $ok = $ok -and $line } else { "$line" }
    }
}
exit $(if ($ok) { 0 } else { 1 })
