# Offline regressions for the bootstrap's PowerShell steps. No downloads or builds.
param([ValidateSet('Paths', 'Update', 'Rust', 'Folder')][string]$Case = 'Paths')
$ErrorActionPreference = 'Stop'
$source = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'makepad.bat')
$base = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N') + " O'Brien (2) [data]")
[IO.Directory]::CreateDirectory($base) | Out-Null
$utf8 = New-Object Text.UTF8Encoding($false)
function Run-Fragment([string]$text) {
    [IO.File]::WriteAllText((Join-Path $base 'step.bat'), "@echo off`r`nsetlocal EnableExtensions EnableDelayedExpansion`r`n" + ($text -replace "`r?`n", "`r`n") + "`r`n", $utf8)
    # From its folder: cmd /c strips the quotes of a path with parentheses in it.
    Push-Location -LiteralPath $base
    try { & cmd.exe /d /c step.bat } finally { Pop-Location }
    return $LASTEXITCODE
}
function Require($ok, $message) { if (-not $ok) { throw $message } }
try {
    $env:ROOT = $base
    $env:CACHE = Join-Path $base 'cache'
    $env:SRC = Join-Path $base 'source'
    $env:REL = 'fixture'
    $env:SERVICE = 'https://example.invalid'
    $env:RUNNER = Join-Path $base 'latest.json'
    [IO.Directory]::CreateDirectory((Join-Path $base 'apps')) | Out-Null
    [IO.Directory]::CreateDirectory($env:SRC) | Out-Null
    $new = Join-Path $env:CACHE 'loader\.unpack\fixture'
    [IO.Directory]::CreateDirectory($new) | Out-Null
    $release = '{"release":"fixture","loader":{"name":"loader.zip","bytes":1,"sha256":"' + ('a' * 64) + '"},"repositories":[]}'
    [IO.File]::WriteAllText($env:RUNNER, $release, $utf8)
    $update = [regex]::Match($source, '(?ms)^        powershell -NoProfile -Command \^\r?\n.*?^            \|\| .*?$').Value
    Require ($update.Length -gt 0) 'source update command missing'
    if ($Case -eq 'Paths') {
        # Parsed on every start, whether or not the folder needs making.
        $make = [regex]::Match($source, '(?m)^if not exist "%ROOT%" mkdir .*$').Value
        Require ($make.Length -gt 0) 'folder creation missing'
        $code = Run-Fragment ($make + "`nexit /b 0")
        Require ($code[-1] -eq 0) 'folder creation failed for a literal folder name'
        $block = [regex]::Match($source, '(?ms)^if not "!HAVE_VERSION!"==.*?^\)\r?$').Value
        Require ($block.Length -gt 0) 'source update block missing'
        $code = Run-Fragment ('set "VERSION=same" & set "HAVE_VERSION=same"' + "`n" + $block + "`nexit /b 0")
        Require ($code[-1] -eq 0) 'source update block failed to parse for a literal folder name'
        $read = [regex]::Match($source, '(?ms)^if exist "%RUNNER%" for /f .*?^\)').Value
        Require ($read.Length -gt 0) 'release reader missing'
        $clear = 'set "REL=" & set "ZIPNAME="' + "`n"
        $code = Run-Fragment ($clear + $read + "`nif not defined REL exit /b 1`nif not defined ZIPNAME exit /b 1`nexit /b 0")
        Require ($code[-1] -eq 0) 'release reader failed for a literal folder name'
        [IO.File]::WriteAllText((Join-Path $new 'file.rs'), 'new', $utf8)
        $code = Run-Fragment ($update + "`nexit /b 0")
        Require ($code[-1] -eq 0) 'source update failed for a literal folder name'
        Require ([IO.File]::ReadAllText((Join-Path $env:SRC 'file.rs')) -eq 'new') 'source update did not copy the file'
        $launch = [regex]::Match($source, '(?ms)^    powershell -NoProfile -Command \^\r?\n.*?^        \|\| .*?$').Value
        Require ($launch.Length -gt 0) 'launch writer missing'
        $code = Run-Fragment ($launch + "`nexit /b 0")
        Require ($code[-1] -eq 0) 'launch writer failed for a literal folder name'
        Require ([IO.File]::Exists((Join-Path $base 'apps\runner.launch.part'))) 'launch record missing'
        foreach ($bad in @(
            $release.Replace('"fixture"', '"..\\evil"'),
            $release.Replace('"fixture"', '"fixture\n"'),
            $release.Replace('"loader.zip"', '"loader.zip\n"'),
            $release.Replace('"sha256":"' + ('a' * 64), '"sha256":"' + ('a' * 64) + '\n'),
            $release.Replace('[]', '[{"name":"a\n","path":"makepad","commit":""}]'),
            $release.Replace('"loader.zip"', '"..\\evil.zip"'),
            $release.Replace('"loader.zip"', '".."'),
            $release.Replace('"sha256":"a', '"sha256":"&a'),
            $release.Replace('[]', '[{"name":"a&b","path":"makepad","commit":""}]'))) {
            [IO.File]::WriteAllText($env:RUNNER, $bad, $utf8)
            $code = Run-Fragment ($clear + $read + "`nif defined REL exit /b 1`nif defined ZIPNAME exit /b 1`nexit /b 0")
            Require ($code[-1] -eq 0) "release reader took $bad"
        }
    }
    if ($Case -eq 'Update') {
        $target = Join-Path $env:SRC 'locked.rs'
        [IO.File]::WriteAllText($target, 'old', $utf8)
        [IO.File]::WriteAllText((Join-Path $new 'locked.rs'), 'replacement', $utf8)
        $lock = [IO.File]::Open($target, 'Open', 'ReadWrite', 'None')
        try {
            $code = Run-Fragment ($update + "`nexit /b 0")
            Require ($code[-1] -ne 0) 'locked source replacement reported success'
        } finally { $lock.Dispose() }
        Require ([IO.File]::ReadAllText($target) -eq 'old') 'failed update changed the locked file'
        # A name that changes kind between versions, both ways; target\ and the stamps stay.
        foreach ($old in 'was_folder\deep\old.rs', 'was_file', 'target\stays', '.version', 'gone\empty\old.rs') {
            $path = Join-Path $env:SRC $old
            [IO.Directory]::CreateDirectory((Split-Path $path)) | Out-Null
            [IO.File]::WriteAllText($path, 'old', $utf8)
        }
        foreach ($now in 'was_folder', 'was_file\inner.rs') {
            $path = Join-Path $new $now
            [IO.Directory]::CreateDirectory((Split-Path $path)) | Out-Null
            [IO.File]::WriteAllText($path, 'new', $utf8)
        }
        $code = Run-Fragment ($update + "`nexit /b 0")
        Require ($code[-1] -eq 0) 'update failed on a name that changed between file and folder'
        Require ([IO.File]::ReadAllText((Join-Path $env:SRC 'was_folder')) -eq 'new') 'folder not replaced by the file'
        Require ([IO.File]::ReadAllText((Join-Path $env:SRC 'was_file\inner.rs')) -eq 'new') 'file not replaced by the folder'
        Require ([IO.File]::ReadAllText((Join-Path $env:SRC 'target\stays')) -eq 'old') 'update touched target'
        Require ([IO.File]::ReadAllText((Join-Path $env:SRC '.version')) -eq 'old') 'update touched a stamp'
        Require (-not (Test-Path -LiteralPath (Join-Path $env:SRC 'gone'))) 'a folder the new version has no more stayed'
    }
    if ($Case -eq 'Rust') {
        $env:RUST = Join-Path $base 'rust'
        $env:RUST_VERSION = '1.98.0'
        $env:TRIPLE = 'x86_64-pc-windows-gnu'
        [IO.Directory]::CreateDirectory((Join-Path $env:RUST 'bin')) | Out-Null
        [IO.File]::WriteAllText((Join-Path $env:RUST 'bin\cargo.exe'), '', $utf8)
        [IO.File]::WriteAllText((Join-Path $env:RUST 'bin\rustc.exe'), '', $utf8)
        $check = [regex]::Match($source, '(?ms)^set "NEED_RUST=".*?^if not defined NEED_RUST goto :have_rust').Value
        Require ($check.Length -gt 0) 'Rust selection missing'
        $check += "`nexit /b 7`n:have_rust`nexit /b 0"
        foreach ($stamp in @('1.97.0 x86_64-pc-windows-gnu', '1.98.0 x86_64-pc-windows-msvc')) {
            [IO.File]::WriteAllText((Join-Path $env:RUST '.toolchain-version'), $stamp, $utf8)
            $code = Run-Fragment $check
            Require ($code[-1] -eq 7) 'mismatched Rust was reused'
        }
        [IO.File]::WriteAllText((Join-Path $env:RUST '.toolchain-version'), "$env:RUST_VERSION $env:TRIPLE", $utf8)
        $code = Run-Fragment $check
        Require ($code[-1] -eq 0) 'matching Rust was not reused'
        [IO.File]::Delete((Join-Path $env:RUST 'bin\rustc.exe'))
        $code = Run-Fragment $check
        Require ($code[-1] -eq 7) 'Rust without rustc was reused'
    }
    if ($Case -eq 'Folder') {
        # The folder choice as makepad.bat runs it from where it lies. Opened inside a zip it runs in a
        # temporary folder (TEMP, or only TMP, its long name or its short one): it goes to the profile's
        # folder and says why. The folder Extract All makes of a second download, "(1)" in its name, is
        # its own; a download among other files goes to the profile's folder, and says so.
        $choose = [regex]::Match($source, '(?ms)^set "ROOT=%~dp0"\r?$.*?^if /i not "%~dp0"==.*?$').Value
        Require ($choose.Length -gt 0) 'folder choice missing'
        $check = [regex]::Match($source, '(?ms)^:temporary\r?$.*?^exit /b 0').Value
        Require ($check.Length -gt 0) 'temporary folder check missing'
        Remove-Item Env:MAKEPAD_LOADER_ROOT -ErrorAction SilentlyContinue
        $env:USERPROFILE = Join-Path $base 'profile'
        $profileRoot = Join-Path $env:USERPROFILE 'makepad-runner'
        $temp = Join-Path $base 'Local Temp'
        $other = Join-Path $base 'Other Temp'
        $downloads = Join-Path $base 'Downloads'
        foreach ($dir in $temp, $other, $downloads) { [IO.Directory]::CreateDirectory($dir) | Out-Null }
        function Choose([string]$dir, [string]$tempVar, [string]$tmpVar) {
            $env:TEMP = $tempVar; $env:TMP = $tmpVar
            [IO.Directory]::CreateDirectory($dir) | Out-Null
            $bat = "@echo off`nsetlocal EnableExtensions EnableDelayedExpansion`n$choose`necho ROOT=!ROOT!`nexit /b 0`n$check`n"
            [IO.File]::WriteAllText((Join-Path $dir 'makepad.bat'), ($bat -replace "`r?`n", "`r`n"), $utf8)
            Push-Location -LiteralPath $dir
            try { return (& cmd.exe /d /c makepad.bat) -join "`n" } finally { Pop-Location }
        }
        function At([string]$out, [string]$dir) { $out -match ('(?m)^ROOT=' + [regex]::Escape($dir) + '\r?$') }
        $short = (New-Object -ComObject Scripting.FileSystemObject).GetFolder($temp).ShortPath
        "temporary folder: $temp"
        "its short name:   $short"
        foreach ($run in @(
                @('Temp1_makepad-runner.zip', $temp, $temp, 'TEMP'),
                @('Temp2_makepad-runner.zip', $other, $temp, 'only TMP'),
                @('Temp3_makepad-runner.zip', $short, $short, 'a short TEMP'))) {
            $out = Choose (Join-Path $temp $run[0]) $run[1] $run[2]
            Require (At $out $profileRoot) "inside a zip, $($run[3]): not the profile's folder: $out"
            Require ($out -match 'This is a temporary folder') "inside a zip, $($run[3]): no line saying why: $out"
        }
        Require ([IO.File]::Exists((Join-Path $profileRoot 'makepad.bat'))) 'makepad.bat was not copied to the profile''s folder'
        $second = Join-Path $downloads 'makepad-runner (1)'
        $out = Choose $second $temp $temp
        Require (At $out $second) "a second download's folder is not its own: $out"
        Require ($out -notmatch 'Makepad goes into') "a second download's folder: a line saying it moved: $out"
        [IO.File]::WriteAllText((Join-Path $downloads 'other.txt'), 'other', $utf8)
        $out = Choose $downloads $temp $temp
        Require (At $out $profileRoot) "a download among other files: not the profile's folder: $out"
        Require ($out -match 'This folder holds other files') "a download among other files: no line saying why: $out"
    }
    "PASS $Case"
} finally {
    Remove-Item -LiteralPath $base -Recurse -Force
}
