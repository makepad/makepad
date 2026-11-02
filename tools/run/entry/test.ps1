# First-run test of makepad.bat in a fresh folder against a local release service serving tiny
# fixture loaders (the same fixture as test.sh): first run, unchanged run (no rebuild), changed
# loader (rebuild), broken loader (error, log excerpt and path).
# Usage: powershell -NoProfile -File test.ps1 BASE_DIR   (BASE_DIR: a new, empty folder)
# Never run it against an existing installation or on a machine someone works on.
param([Parameter(Mandatory = $true)][string]$Base)
$ErrorActionPreference = 'Stop'
$Base = [IO.Path]::GetFullPath($Base)
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if ((Test-Path $Base) -and (Get-ChildItem $Base -Force | Select-Object -First 1)) { throw "$Base is not empty" }
New-Item -ItemType Directory -Force -Path "$Base\www\public\app\runner\fx1", "$Base\home" | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
$script:failures = 0
function Check($name, $text, $pattern) {
    if ($text -match $pattern) { "PASS $name" } else { "FAIL $name (no '$pattern')"; $script:failures++ }
}
function Publish($text, $broken) {
    $zipdir = "$Base\zipfix"
    if (Test-Path $zipdir) { Remove-Item -Recurse -Force $zipdir }
    New-Item -ItemType Directory -Force -Path "$zipdir\fx1\dep-a" | Out-Null
    $files = @{
        'fx1/Cargo.toml' = "[package]`nname = `"makepad-run`"`nversion = `"0.1.0`"`nedition = `"2021`"`n[dependencies]`ndep-a = { path = `"dep-a`" }`n[[bin]]`nname = `"makepad`"`npath = `"main.rs`"`n[profile.host]`ninherits = `"release`"`n"
        'fx1/Cargo.lock' = "version = 4`n`n[[package]]`nname = `"dep-a`"`nversion = `"0.1.0`"`n`n[[package]]`nname = `"makepad-run`"`nversion = `"0.1.0`"`ndependencies = [`n `"dep-a`",`n]`n"
        'fx1/dep-a/Cargo.toml' = "[package]`nname = `"dep-a`"`nversion = `"0.1.0`"`nedition = `"2021`"`n[lib]`npath = `"lib.rs`"`n"
        'fx1/dep-a/lib.rs' = "pub fn text() -> &'static str { `"$text`" }`n"
        'fx1/main.rs' = $(if ($broken) { "fn main() { this is not rust }`n" } else { "fn main() {`n    let home = std::env::var(`"MAKEPAD_RUN_HOME`").unwrap_or_default();`n    let record = std::path::Path::new(&home).join(`"apps/runner.launch`");`n    println!(`"fixture loader: {} launch record present: {}`", dep_a::text(), record.is_file());`n}`n" })
    }
    foreach ($k in $files.Keys) { [IO.File]::WriteAllText("$zipdir\$($k.Replace('/', '\'))", $files[$k], $utf8) }
    $zip = "$Base\www\public\app\runner\fx1\loader.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Push-Location $zipdir
    & tar.exe -a -cf $zip fx1
    Pop-Location
    $sum = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    $commit = '0123456789abcdef0123456789abcdef01234567'
    $json = '{"release":"fx1","repositories":[{"name":"makepad","path":"makepad","commit":"' + $commit + '"}],"loader":{"name":"loader.zip","bytes":' + (Get-Item $zip).Length + ',"sha256":"' + $sum + '","commit":"' + $commit + '"}}'
    [IO.File]::WriteAllText("$Base\www\public\app\runner\latest", $json, $utf8)
}
# the entry script as the server sends it: CRLF line ends
$bat = (Get-Content -Raw -Encoding UTF8 "$here\makepad.bat") -replace "`r?`n", "`r`n"
[IO.File]::WriteAllText("$Base\home\makepad.bat", $bat, $utf8)

Publish 'one' $false
$port = 20000 + (Get-Random -Maximum 20000)
$server = Start-Job -ArgumentList $Base, $port -ScriptBlock {
    param($base, $port)
    $l = New-Object System.Net.HttpListener
    $l.Prefixes.Add("http://127.0.0.1:$port/")
    $l.Start()
    while ($true) {
        $c = $l.GetContext()
        $f = Join-Path "$base\www" ($c.Request.Url.AbsolutePath.TrimStart('/').Replace('/', '\'))
        if (Test-Path $f -PathType Leaf) {
            $b = [IO.File]::ReadAllBytes($f); $c.Response.ContentLength64 = $b.Length
            $c.Response.OutputStream.Write($b, 0, $b.Length)
        } else { $c.Response.StatusCode = 404 }
        $c.Response.Close()
    }
}
Start-Sleep 2
$env:MAKEPAD_LOADER_SERVICE = "http://127.0.0.1:$port"
$env:MAKEPAD_LOADER_ROOT = "$Base\install"
function Run($name) {
    $out = & cmd.exe /d /c "`"$Base\home\makepad.bat`" 2>&1" | Out-String
    $script:code = $LASTEXITCODE
    "=== $name (exit $script:code)"; $out
    return $out
}
try {
    $r1 = Run 'run1'
    Check 'first run exits 0' "$script:code" '^0$'
    Check 'progress line' $r1 'Building the JIT .* [0-9]+/2 crates'
    Check 'loader started' $r1 'fixture loader: one launch record present: true'
    '--- sizes (KB), first run'
    foreach ($d in 'install\rust', 'install\cache\downloads', 'install') { '{0}  {1}' -f [int]((Get-ChildItem "$Base\$d" -Recurse -Force -File | Measure-Object Length -Sum).Sum / 1024), $d }
    $r2 = Run 'run2'
    Check 'unchanged loader: no rebuild' $r2 'up to date'
    Publish 'two' $false
    $r3 = Run 'run3'
    Check 'changed loader: rebuilt' $r3 'compiled in'
    Check 'changed loader: new text' $r3 'fixture loader: two'
    $loader = "$Base\install\cache\loader"
    $hex = '0123456789abcdef0123456789abcdef01234567'
    Copy-Item -Recurse "$loader\src" "$loader\$hex"
    Remove-Item "$loader\$hex\.loader-sha256"
    [IO.File]::WriteAllText("$loader\previous", "src`n", $utf8); [IO.File]::WriteAllText("$loader\current", "$hex`n", $utf8)
    $r5 = Run 'run5'
    Check 'current folder: built in place' $r5 'compiled in'
    Check 'current folder: stamp written there' (Test-Path "$loader\$hex\.loader-sha256") 'True'
    [IO.File]::WriteAllText("$loader\pending", "$hex`n", $utf8)
    $r6 = Run 'run6'
    Check 'rollback: line' $r6 'JIT: went back to src'
    Check 'rollback: src binary runs' $r6 'fixture loader: two'
    Check 'rollback: pointers' ("" + (-not (Test-Path "$loader\pending")) + (-not (Test-Path "$loader\current")) + (Test-Path "$loader\rolled-back")) 'TrueTrueTrue'
    Publish 'three' $true
    $r4 = Run 'run4'
    Check 'broken loader: nonzero exit' "$script:code" '^[1-9]'
    Check 'broken loader: error line' $r4 'Error: the JIT did not compile'
    Check 'broken loader: log path' $r4 'Full log: .*build\.log'
    Check 'broken loader: error text' $r4 'error'
} finally {
    Stop-Job $server; Remove-Job $server -Force
}
"failures: $script:failures"
exit $script:failures
