# Run on a Windows build node. This never contacts a remote or commits in the checkout.
$ErrorActionPreference = 'Stop'
$gitExe = (Get-Command git -CommandType Application).Source
$checkout = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path.Replace('\', '/')
$tempRoot = if ($env:TMPDIR) { $env:TMPDIR } else { [IO.Path]::GetTempPath() }
$scratch = Join-Path $tempRoot ('ci-merge-' + [Guid]::NewGuid().ToString('N'))
$utf8 = New-Object System.Text.UTF8Encoding($false)
function Git-At {
    & $gitExe -C $scratch @args
    if ($LASTEXITCODE -ne 0) { throw "git $args failed ($LASTEXITCODE)" }
}
function Shell-Quote([string] $text) {
    # Git for Windows executes drivers with sh, not PowerShell.
    return "'" + $text.Replace("'", "'\''") + "'"
}
function Section([string] $name, [int] $digest, [string] $work, [string] $date) {
    return "`n[$name]`ndigest = `"$($digest.ToString().PadLeft(64, '0'))`"`nwork = `"$work`"`ndate = `"$date`"`n"
}
function Write-Proofs([int] $app, [string] $work, [string] $date, [int] $tests, [string] $testWork, [int] $job, [string] $jobWork, [int] $tie) {
    $text = "# CI proofs fixture`n"
    $text += Section 'app.linux' $app $work $date
    $text += Section 'tests.macos' $tests $testWork '2026-10-02'
    $text += Section 'job.jit.windows' $job $jobWork '2026-10-02'
    $text += Section 'job.tie.linux' $tie $work '2026-10-02'
    [IO.File]::WriteAllText($proof, $text, $utf8)
}
function Verify {
    $unmerged = Git-At ls-files -u
    $text = [IO.File]::ReadAllText($proof)
    if ($unmerged -or $text -match '(?m)^(<<<<<<<|=======|>>>>>>>|\|\|\|\|\|\|\|)') { throw 'unresolved proof conflict' }
    foreach ($work in @('right', 'left-tests', 'right-job')) {
        if (-not $text.Contains('work = "' + $work + '"')) { throw "missing proof: $work" }
    }
    if ($text.Contains('[job.tie.linux]')) { throw 'ambiguous section was retained' }
}
$oldEditor = $env:GIT_EDITOR
$oldSequenceEditor = $env:GIT_SEQUENCE_EDITOR
try {
    New-Item -ItemType Directory -Path $scratch | Out-Null
    Git-At init -q -b base
    Git-At config user.name ci-merge-check
    Git-At config user.email ci-merge-check@example.invalid
    Git-At config commit.gpgsign false
    Git-At config core.hooksPath (Join-Path $scratch 'no-hooks')
    $driver = 'cargo run -q --release --manifest-path ' + (Shell-Quote "$checkout/tools/ci_merge/Cargo.toml") + ' --target-dir ' + (Shell-Quote "$checkout/target/ci_merge") + ' -- %O %A %B %P'
    Git-At config merge.ci-proof.name 'ci.toml proofs'
    Git-At config merge.ci-proof.driver $driver
    [IO.File]::WriteAllText((Join-Path $scratch '.gitattributes'), "ci.toml merge=ci-proof`n", $utf8)
    New-Item -ItemType Directory -Path (Join-Path $scratch 'crate space') | Out-Null
    $proof = Join-Path $scratch 'crate space/ci.toml'
    Write-Proofs 1 base 2026-10-01 1 base-tests 1 base-job 1
    Git-At add .
    Git-At commit -qm base
    Git-At checkout -qb left
    Write-Proofs 2 left 2026-10-02 2 left-tests 1 base-job 2
    Git-At commit -qam left
    Git-At checkout -qb right base
    Write-Proofs 3 right 2026-10-03 1 base-tests 3 right-job 3
    Git-At commit -qam right
    Git-At checkout -qb merge-check left
    Git-At merge --no-edit right
    Verify
    Git-At checkout -qb rebase-check left
    $env:GIT_EDITOR = ':'
    $env:GIT_SEQUENCE_EDITOR = ':'
    Git-At rebase right
    Verify
    Git-At checkout -qb pick-check right
    Git-At cherry-pick left
    Verify
    Git-At checkout -qb pull-check left
    Git-At pull --no-rebase --no-edit . right
    Verify
    Write-Output 'ci-merge: merge, rebase, cherry-pick and local pull passed'
} finally {
    $env:GIT_EDITOR = $oldEditor
    $env:GIT_SEQUENCE_EDITOR = $oldSequenceEditor
    if ($scratch -and (Test-Path -LiteralPath $scratch)) { Remove-Item -LiteralPath $scratch -Recurse -Force }
}
