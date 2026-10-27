@echo off
rem Makepad for Windows: gets Rust, compiles Makepad's loader (makepad run) from its source on
rem this computer and starts it. Read it before you run it; it is this one file. The same steps as
rem makepad.sh (macOS, Linux):
rem
rem 1. Rust 1.98.0 for x86_64-pc-windows-gnu (rustc, cargo, rust-std, rust-mingw: its own linker,
rem    no Visual Studio) from static.rust-lang.org, checked against the sizes and SHA-256 below.
rem 2. The loader's source: the release service's licence-free loader release, a ZIP whose size
rem    and SHA-256 the service names (checked before unpacking).
rem 3. `cargo build --profile host --offline -p makepad-run --bin makepad` for this computer's
rem    processor (the ZIP holds every crate; a new loader release replaces only the files that
rem    changed, so only their crates compile again), then `makepad` with no arguments: the runner,
rem    from its launch record (apps\runner.launch, the release service's latest `runner` release),
rem    which downloads the rest. With arguments, the loader runs with them instead.
rem
rem In its folder (this file's own when it holds nothing else, else %USERPROFILE%\makepad-runner) it
rem creates rust\, cache\, apps\ and runner\, the same folders as makepad.sh.
rem Run it again at any time: nothing is downloaded or compiled twice. It uses only Windows' own
rem curl.exe, tar.exe, certutil.exe and PowerShell (to read the service's JSON answers).
setlocal EnableExtensions EnableDelayedExpansion

rem The folder: this file's own when it is an installation already or holds nothing else (a folder
rem made for it); else %USERPROFILE%\makepad-runner (a download among other files), where it copies itself.
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"
if not exist "%ROOT%\cache" (
    for /f "delims=" %%F in ('dir /b /a "%ROOT%" 2^>nul') do if /i not "%%F"=="%~nx0" if /i not "%%F"=="desktop.ini" set "ROOT=%USERPROFILE%\makepad-runner"
)
if defined MAKEPAD_LOADER_ROOT set "ROOT=%MAKEPAD_LOADER_ROOT%"
if not exist "%ROOT%" mkdir "%ROOT%" || (echo   Cannot create the folder %ROOT%. & exit /b 1)
if /i not "%~dp0"=="%ROOT%\" copy /y "%~f0" "%ROOT%\makepad.bat" >nul
set "RUST_VERSION=1.98.0"
set "TRIPLE=x86_64-pc-windows-gnu"
set "DIST=https://static.rust-lang.org/dist"
set "RUST=%ROOT%\rust"
set "CACHE=%ROOT%\cache"
set "DOWNLOADS=%CACHE%\downloads"
set "LOGS=%CACHE%\logs"
set "PARTS=rustc cargo rust-std rust-mingw"
set "SIZE_rustc=158249903"
set "SIZE_cargo=18429051"
set "SIZE_rust-std=43334608"
set "SIZE_rust-mingw=9594885"
set "SHA_rustc=c97a98af7a8c84cee611a9a8a5bba6a804f82b09899a2e451a33b5934d684542"
set "SHA_cargo=79da7e6bb0c7ac1583b380c702477da298f2dace9ba22f41150698c516533406"
set "SHA_rust-std=47bceabaceabc4de97e20f7470d9973fb20e844986da01b3a31cf28e24596660"
set "SHA_rust-mingw=14fc86abb02d7d0575eb38a738161dcd87aa4032e091cce3424e78321b145e5e"
set "SERVICE=https://makepad.nl/api/loader"
if defined MAKEPAD_LOADER_SERVICE set "SERVICE=%MAKEPAD_LOADER_SERVICE%"
title Makepad
chcp 65001 >nul

where curl.exe >nul 2>nul || (echo Makepad needs curl.exe: Windows 10 version 1803 or later. & exit /b 1)
where tar.exe >nul 2>nul || (echo Makepad needs tar.exe: Windows 10 version 1803 or later. & exit /b 1)
for %%D in ("%DOWNLOADS%" "%LOGS%" "%CACHE%\loader" "%CACHE%\runner" "%ROOT%\apps") do if not exist "%%~D" mkdir "%%~D" || exit /b 1
set "MAKEPAD_RUN_HOME=%ROOT%"

rem --- 1. Rust ------------------------------------------------------------------------------------
set "NEED_RUST="
if not exist "%RUST%\.toolchain-version" set "NEED_RUST=1"
if not exist "%RUST%\bin\cargo.exe" set "NEED_RUST=1"
if not defined NEED_RUST goto :have_rust
echo   Rust %RUST_VERSION%: downloading, once, into this folder
set "CFG=%DOWNLOADS%\rust.curl"
del "%CFG%" 2>nul
for %%P in (%PARTS%) do (
    set "FILE=%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz"
    set "HAVE=0"
    if exist "!FILE!" for %%S in ("!FILE!") do set "HAVE=%%~zS"
    if not "!HAVE!"=="!SIZE_%%P!" (
        del "!FILE!" 2>nul
        >>"%CFG%" echo url = "%DIST%/%%P-%RUST_VERSION%-%TRIPLE%.tar.gz"
        >>"%CFG%" echo output = "!FILE:\=/!"
        >>"%CFG%" echo proto = =https
        >>"%CFG%" echo fail
        >>"%CFG%" echo retry = 3
    )
)
if exist "%CFG%" (
    curl.exe -sS -Z --parallel-immediate -K "%CFG%" 2>"%LOGS%\download.log" || (echo   The download stopped. Run makepad.bat again to continue it. & exit /b 1)
    del "%CFG%"
)
for %%P in (%PARTS%) do (
    call :verify "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" "!SHA_%%P!" || (del "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" & echo   Checksum mismatch for %%P: downloaded again on the next run. & exit /b 1)
)
set "STAGE=%RUST%.unpack"
if exist "%STAGE%" rmdir /s /q "%STAGE%"
mkdir "%STAGE%\ready" || exit /b 1
for %%P in (%PARTS%) do (
    mkdir "%STAGE%\%%P" || exit /b 1
    tar.exe -xzf "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" --strip-components=2 -C "%STAGE%\%%P" || (echo   Unpacking %%P failed. & exit /b 1)
    robocopy "%STAGE%\%%P" "%STAGE%\ready" /e /move /njh /njs /nfl /ndl /np >nul
    if errorlevel 8 (echo   Unpacking %%P failed. & exit /b 1)
)
<nul set /p "=%RUST_VERSION% %TRIPLE%" > "%STAGE%\ready\.toolchain-version"
if exist "%RUST%" rmdir /s /q "%RUST%"
move "%STAGE%\ready" "%RUST%" >nul || exit /b 1
rmdir /s /q "%STAGE%"
for %%P in (%PARTS%) do del "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" 2>nul
:have_rust

rem --- 2. The loader's source: the service's latest loader release (offline: the one built before)
set "LATEST=%CACHE%\loader-latest.json"
del "%LATEST%" 2>nul
curl.exe -fsS --proto =https -m 60 "%SERVICE%/public/app/relax/latest" -o "%LATEST%" 2>>"%LOGS%\download.log"
set "VERSION=" & set "ZIPNAME=" & set "ZIPBYTES=" & set "ZIPSHA="
if exist "%LATEST%" for /f "tokens=1-4" %%A in ('powershell -NoProfile -Command "$j = Get-Content -Raw '%LATEST%' | ConvertFrom-Json; '{0} {1} {2} {3}' -f $j.release, $j.archive.name, $j.archive.bytes, $j.archive.sha256"') do (
    set "VERSION=%%A" & set "ZIPNAME=%%B" & set "ZIPBYTES=%%C" & set "ZIPSHA=%%D"
)
rem One source folder for every version (target\ inside it: makepad finds its std two levels up).
set "SRC=%CACHE%\loader\src"
if not defined VERSION if exist "%SRC%\.version" set /p VERSION=<"%SRC%\.version"
if not defined VERSION (echo   Could not reach the release service ^(%SERVICE%^) and no loader is here yet. Check the connection, then run this again. & exit /b 1)
set "MAKEPAD=%SRC%\target\host\makepad.exe"
set "BUILT=%VERSION% %RUST_VERSION% %TRIPLE%"
set "HAVE_BUILT="
if exist "%SRC%\.built" set /p HAVE_BUILT=<"%SRC%\.built"
if exist "%MAKEPAD%" if "%HAVE_BUILT%"=="%BUILT%" goto :have_loader
set "HAVE_VERSION="
if exist "%SRC%\.version" set /p HAVE_VERSION=<"%SRC%\.version"
if not "%HAVE_VERSION%"=="%VERSION%" (
    if not defined ZIPNAME (echo   The release service did not answer; the loader's source is not here yet. Check the connection, then run this again. & exit /b 1)
    echo   Makepad's loader: downloading its source
    curl.exe -fsS --proto =https --retry 3 "%SERVICE%/public/app/relax/%VERSION%/%ZIPNAME%" -o "%DOWNLOADS%\%ZIPNAME%" 2>>"%LOGS%\download.log" || (echo   The download stopped. & exit /b 1)
    for %%S in ("%DOWNLOADS%\%ZIPNAME%") do if not "%%~zS"=="%ZIPBYTES%" (del "%DOWNLOADS%\%ZIPNAME%" & echo   The loader's source has the wrong size. & exit /b 1)
    call :verify "%DOWNLOADS%\%ZIPNAME%" "%ZIPSHA%" || (del "%DOWNLOADS%\%ZIPNAME%" & echo   Checksum mismatch for %ZIPNAME%. & exit /b 1)
    if exist "%CACHE%\loader\.unpack" rmdir /s /q "%CACHE%\loader\.unpack"
    mkdir "%CACHE%\loader\.unpack" || exit /b 1
    tar.exe -xf "%DOWNLOADS%\%ZIPNAME%" -C "%CACHE%\loader\.unpack" || (echo   Unpacking the loader's source failed. Run this again. & exit /b 1)
    if not exist "%SRC%\Cargo.toml" (
        if exist "%SRC%" rmdir /s /q "%SRC%"
        move "%CACHE%\loader\.unpack\%VERSION%" "%SRC%" >nul || exit /b 1
    ) else (
        rem an update: changed files replaced (with the time of now, so cargo sees them as changed),
        rem files the new version no longer has removed, the rest (and target\) left as they are
        echo   Makepad's loader: updating its source to %VERSION%
        powershell -NoProfile -Command ^
            "$new = '%CACHE%\loader\.unpack\%VERSION%'; $src = '%SRC%'; $now = Get-Date; $keep = @{};" ^
            "foreach ($f in Get-ChildItem -LiteralPath $new -Recurse -File) { $rel = $f.FullName.Substring($new.Length + 1); $keep[$rel] = 1; $dst = Join-Path $src $rel;" ^
            "  if (-not (Test-Path -LiteralPath $dst) -or (Get-FileHash -LiteralPath $f.FullName).Hash -ne (Get-FileHash -LiteralPath $dst).Hash) {" ^
            "    New-Item -ItemType Directory -Force -Path (Split-Path $dst) | Out-Null; Move-Item -Force -LiteralPath $f.FullName -Destination $dst; (Get-Item -LiteralPath $dst).LastWriteTime = $now } };" ^
            "foreach ($f in Get-ChildItem -LiteralPath $src -Recurse -File) { $rel = $f.FullName.Substring($src.Length + 1);" ^
            "  if (-not $rel.StartsWith('target\') -and $rel -ne '.version' -and $rel -ne '.built' -and -not $keep.ContainsKey($rel)) { Remove-Item -Force -LiteralPath $f.FullName } }" ^
            || (echo   Could not update the loader's source in %SRC%. & exit /b 1)
    )
    >"%SRC%\.version" echo %VERSION%
    rmdir /s /q "%CACHE%\loader\.unpack"
    del "%DOWNLOADS%\%ZIPNAME%"
)

rem --- 3. Compile the loader: no Rust settings from elsewhere on this computer; output in cache\
echo   Makepad's loader: compiling for this computer, once per version
set "PATH=%RUST%\bin;%SystemRoot%\System32;%SystemRoot%;%SystemRoot%\System32\WindowsPowerShell\v1.0"
set "CARGO_HOME=%CACHE%\cargo-home"
set "CARGO_INCREMENTAL=0"
for %%V in (RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTDOC) do set "%%V="
rem [profile.host]: thin LTO, 16 codegen units; target-cpu=native for this computer's processor
set "RUSTFLAGS=-Ctarget-cpu=native"
set "RUSTC=%RUST%\bin\rustc.exe"
pushd "%SRC%"
cargo.exe build --profile host --offline -p makepad-run --bin makepad >"%LOGS%\build.log" 2>&1
set "BUILD=%errorlevel%"
popd
set "RUSTFLAGS="
if not "%BUILD%"=="0" (echo   Makepad's loader did not compile. Run this again; if it stops the same way, the log says why: %LOGS%\build.log & exit /b 1)
>"%SRC%\.built" echo %BUILT%
:have_loader
set "PATH=%RUST%\bin;%PATH%"
set "MAKEPAD_LOADER_ROOT=%ROOT%"
set "MAKEPAD_LOADER_SERVICE=%SERVICE%"
if not "%~1"=="" (
    "%MAKEPAD%" %*
    exit /b !errorlevel!
)

rem --- 4. The runner's launch record, then `makepad` alone runs it (makepad launch.rs) ------------
rem The service's latest licence-free `runner` release: its repositories on the service's read-only
rem git face, the one at `makepad` the workspace, the others mounted at their folders.
set "RUNNER=%CACHE%\runner\latest.json"
del "%RUNNER%" 2>nul
curl.exe -fsS --proto =https -m 60 "%SERVICE%/public/app/runner/latest" -o "%RUNNER%" 2>>"%LOGS%\download.log"
if exist "%RUNNER%" (
    powershell -NoProfile -Command ^
        "$j = Get-Content -Raw '%RUNNER%' | ConvertFrom-Json; $g = '%SERVICE%/public/git/runner'; $r = $j.release;" ^
        "$l = @('title`tMakepad', 'arg`trun', 'arg`t--quiet', 'arg`t--cached'); $m = @();" ^
        "foreach ($p in $j.repositories) { $u = '{0}/{1}.git#{2}@{3}' -f $g, $p.name, $r, $p.commit;" ^
        "  if ($p.path -eq 'makepad') { $l += ('arg`t' + $u) } elseif ($p.path.StartsWith('makepad/')) { $m += 'arg`t--mount'; $m += ('arg`t' + $p.path.Substring(8) + '=' + $u) } };" ^
        "$l += $m; $l += 'arg`tmakepad-runner';" ^
        "[IO.File]::WriteAllText('%ROOT%\apps\runner.launch.part', (($l | ForEach-Object { $_ -replace '`t', [char]9 }) -join [char]10) + [char]10)" ^
        || (echo   The runner release could not be read. & exit /b 1)
    move /y "%ROOT%\apps\runner.launch.part" "%ROOT%\apps\runner.launch" >nul || exit /b 1
)
if not exist "%ROOT%\apps\runner.launch" (echo   Could not reach the release service ^(%SERVICE%^) and the runner was never set up. & exit /b 1)
echo   Makepad: starting
"%MAKEPAD%"
exit /b %errorlevel%

rem :verify FILE SHA256 checks a file's SHA-256 (certutil's answer, spaces removed, any case).
:verify
set "GOT="
for /f "skip=1 tokens=*" %%L in ('certutil.exe -hashfile "%~1" SHA256') do if not defined GOT set "GOT=%%L"
set "GOT=%GOT: =%"
if /i "%GOT%"=="%~2" exit /b 0
exit /b 1
