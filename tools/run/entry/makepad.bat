@echo off
rem The downloads and the unpacking run beside the screen, as jobs of this same script.
if "%~1"==":fetch_job" goto :fetch_job
if "%~1"==":unpack_job" goto :unpack_job
rem Makepad for Windows: gets Rust, compiles Makepad's loader (makepad run) from its source on
rem this computer and starts it. Read it before you run it; it is this one file.
rem The same steps as makepad.sh (macOS, Linux):
rem
rem 1. Rust 1.98.0 for x86_64-pc-windows-gnu (rustc, cargo, rust-std, rust-mingw: its own linker,
rem    no Visual Studio) from static.rust-lang.org, fetched as pieces of at most 25 MB over parallel
rem    connections (a stopped download keeps the whole pieces), joined and checked against the
rem    sizes and SHA-256 below, then unpacked side by side.
rem 2. The loader's source: the release service's licence-free loader release, a ZIP whose size
rem    and SHA-256 the service names (checked before unpacking).
rem 3. `cargo build --profile host --offline -p makepad-run --bin makepad --bin makepad-launcher` for this computer's
rem    processor (the ZIP holds every crate). The loader ZIP's SHA-256 keys .version and, after a
rem    successful build, .loader-sha256; .built also records Rust and the target triple. A matching
rem    source marker reuses the source; matching SHA-256 and Rust stamps reuse the binary.
rem    A changed ZIP replaces only changed files, so only their crates
rem    compile again. Then `makepad` with no arguments: the runner,
rem    from its launch record (apps\runner.launch, the release service's latest `runner` release),
rem    which downloads the rest. With arguments, the loader runs with them instead.
rem
rem In a console it draws one screen, redrawn in place about every second: a row for each step
rem (Rust, the loader's source, its build, the Runner) with a bar, sizes, speed or crates, the time
rem so far and what goes on now. Output that is no console (a file, a pipe) gets plain lines.
rem The full output of curl and Cargo stays in cache\logs.
rem
rem In its folder it creates rust\, cache\, apps\ and runner\, the same folders as makepad.sh. That is
rem this file's own folder when it holds nothing else, like the folder Explorer's Extract All makes of
rem makepad-runner.zip. Otherwise it is %USERPROFILE%\makepad-runner, saying so: for a download among
rem other files, and for a temporary folder, where Windows runs a file opened inside a zip that was
rem not extracted (and which it empties later).
rem Under cache\loader, current selects src or a commit folder; pending rolls back to previous
rem unless the new runner confirmed its first start. Each folder keeps its own source and binary.
rem Run it again at any time: nothing is downloaded or compiled twice. It uses only Windows' own
rem curl.exe, tar.exe, certutil.exe and PowerShell (for the console, release JSON, source updates).
setlocal EnableExtensions EnableDelayedExpansion

rem A release service of its own (a test's) installs only into a folder named for it: a run that sets
rem MAKEPAD_LOADER_SERVICE without MAKEPAD_LOADER_ROOT stops before it makes or changes anything.
if defined MAKEPAD_LOADER_SERVICE if not defined MAKEPAD_LOADER_ROOT (echo   MAKEPAD_LOADER_SERVICE is set without MAKEPAD_LOADER_ROOT: give the run a folder of its own. & exit /b 1)

rem The folder: this file's own when it is an installation already, or holds nothing else (a folder
rem made for it) and is no temporary folder; else %USERPROFILE%\makepad-runner, where it copies itself:
rem a download among other files, or a file opened inside a zip (Windows runs it from %TEMP%).
set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"
set "NOT_HOME="
if not defined MAKEPAD_LOADER_ROOT if not exist "%ROOT%\cache" (
    for /f "delims=" %%F in ('dir /b /a "%ROOT%" 2^>nul') do if /i not "%%F"=="%~nx0" if /i not "%%F"=="desktop.ini" set "NOT_HOME=others"
    if not defined NOT_HOME call :temporary
)
if defined MAKEPAD_LOADER_ROOT set "ROOT=%MAKEPAD_LOADER_ROOT%"
if defined NOT_HOME set "ROOT=%USERPROFILE%\makepad-runner"
set "NOTE=" & set "NOTE2="
if "!NOT_HOME!"=="temporary" set "NOTE=This is a temporary folder (a zip opened, not extracted)."
if "!NOT_HOME!"=="others" set "NOTE=This folder holds other files."
if defined NOT_HOME set "NOTE2=Makepad goes into the folder above: start makepad.bat there next time."
if not exist "%ROOT%" mkdir "%ROOT%" || (echo   Cannot create the folder !ROOT!. & exit /b 1)
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
rem https only, except a release service on this machine (tests, a local service)
set "SVCPROTO==https"
if /i "%SERVICE:~0,17%"=="http://127.0.0.1:" set "SVCPROTO==http,https"
if /i "%SERVICE:~0,17%"=="http://localhost:" set "SVCPROTO==http,https"
for /f "tokens=2 delims=/" %%H in ("%SERVICE%") do set "SVCHOST=%%H"
set "SELF=%~f0"
title Makepad
chcp 65001 >nul

rem --- The screen ---------------------------------------------------------------------------------
rem The Builder's look: a header, a row for each step, a rule, a line saying what goes on, rows
rem rewritten in place. It needs a console that takes VT sequences (Windows 10 1511 or later, not
rem the legacy console): PowerShell turns them on for this console and tells which it is. The
rem console's own window (conhost: its Consolas has no check mark, cross or braille) gets the glyphs
rem that font has; a terminal (Windows Terminal, through a pseudo console) gets the Builder's set.
rem Exit codes: 2 no console (a file, a pipe), 3 conhost, 4 a terminal, 5 a console without VT (or
rem one PowerShell may not look at): plain lines there, and a key closes it after an error.
powershell -NoProfile -Command "try { Add-Type -Name Console -Namespace MakepadScreen -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern IntPtr GetStdHandle(int n); [DllImport(\"kernel32.dll\")] public static extern bool GetConsoleMode(IntPtr h, out int m); [DllImport(\"kernel32.dll\")] public static extern bool SetConsoleMode(IntPtr h, int m); [DllImport(\"kernel32.dll\")] public static extern IntPtr GetConsoleWindow(); [DllImport(\"user32.dll\")] public static extern bool IsWindowVisible(IntPtr w);' } catch { if ([Console]::IsOutputRedirected) { exit 2 }; exit 5 }; $k = [MakepadScreen.Console]; $h = $k::GetStdHandle(-11); $m = 0; if (-not $k::GetConsoleMode($h, [ref]$m)) { exit 2 }; if (-not $k::SetConsoleMode($h, ($m -bor 5))) { exit 5 }; if ($k::IsWindowVisible($k::GetConsoleWindow())) { exit 3 }; exit 4" 2>nul
set "PROBE=%errorlevel%"
set "SCREEN=" & set "CONSOLE="
if %PROBE% geq 3 set "CONSOLE=1"
if %PROBE% equ 3 set "SCREEN=conhost"
if %PROBE% equ 4 set "SCREEN=terminal"
set "ESC=" & set "B=" & set "D=" & set "C=" & set "BC=" & set "G=" & set "Y=" & set "R=" & set "N="
if defined SCREEN for /f %%E in ('echo prompt $E ^| cmd') do set "ESC=%%E"
rem Bold, dim (bright black: conhost draws no faint), Makepad's coral (and bold), green, yellow, red.
if defined SCREEN (
    set "B=!ESC![0;1m"
    set "D=!ESC![0;90m"
    set "C=!ESC![0;38;2;255;92;57m"
    set "BC=!ESC![0;1;38;2;255;92;57m"
    set "G=!ESC![0;32m"
    set "Y=!ESC![0;33m"
    set "R=!ESC![0;31m"
    set "N=!ESC![0m"
)
set "BARS=━━━━━━━━━━━━"
set "BARO=────────────"
set "RULE=────────────────────────────────────────────────────────────────────────────"
if "!SCREEN!"=="conhost" (
    set "GL_ok=%G%√%N%" & set "GL_fail=%R%×%N%" & set "SPINS=·•●•" & set "SPINN=4"
) else (
    set "GL_ok=%G%✓%N%" & set "GL_fail=%R%✗%N%" & set "SPINS=⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏" & set "SPINN=10"
)
set "GL_wait=%D%○%N%" & set "GL_doing=%C%●%N%"
set "SPINI=0" & set "PLAIN_TICKS=0" & set "CURROW=" & set "LOGFILE=" & set "TAIL=" & set "T2=" & set "NOTE3=" & set "CLOCKSTOP="
set "NAME1=Rust %RUST_VERSION%" & set "NAME2=Loader source" & set "NAME3=Loader build" & set "NAME4=Runner"
set "WHERE=!ROOT!"
if not "!WHERE:~68!"=="" set "WHERE=…!WHERE:~-67!"
set "WHERE=Folder: !WHERE!"
set "FOOT=Looking at what is here already."
set "HINT=ctrl+c stops · run makepad.bat again to continue"
set "HOLD=1"
for %%I in (1 2 3 4) do (set "K=wait" & set "S=next" & set "INFO=" & call :put %%I)
set "HOLD="
call :now
set "T0=!NOW!"
call :frame

set "CURL_VERSION=" & set "CURL_AGE=0"
for /f "tokens=2" %%V in ('curl.exe --version 2^>nul') do if not defined CURL_VERSION set "CURL_VERSION=%%V"
if defined CURL_VERSION for /f "tokens=1,2 delims=." %%A in ("!CURL_VERSION!") do set /a "CURL_AGE=%%A*1000+%%B" 2>nul
if not defined CURL_VERSION set "CURL_VERSION=no curl.exe"
rem 7.68 is the first curl with --parallel-immediate (the pieces start at once); Windows 10 1803 and
rem 1809 came with 7.55.1, and Windows 10 and 11 have 8.x through their updates.
if !CURL_AGE! lss 7068 (
    set "T=Makepad needs curl.exe 7.68 or newer; this Windows has !CURL_VERSION!."
    set "T2=Windows Update brings it to Windows 10 and 11: update, then run this again."
    goto :fail
)
where tar.exe >nul 2>nul || (set "T=Makepad needs tar.exe: Windows 10 version 1803 or later." & goto :fail)
for %%D in ("%DOWNLOADS%" "%LOGS%" "%CACHE%\loader" "%CACHE%\runner" "%ROOT%\apps") do if not exist "%%~D" mkdir "%%~D" || (set "T=Cannot create the folder %%~D." & goto :fail)
set "MAKEPAD_RUN_HOME=%ROOT%"

rem Restore an unconfirmed switch before choosing the source folder or checking its build stamp.
set "LOADER=!CACHE!\loader"
if exist "!LOADER!\pending" (
    call :loader_folder previous
    if "!LOADER_NAME!"=="src" (
        if exist "!LOADER!\current" del /q "!LOADER!\current"
        if exist "!LOADER!\current" exit /b 1
    ) else (
        >"!LOADER!\current.part" echo !LOADER_NAME!|| exit /b 1
        move /y "!LOADER!\current.part" "!LOADER!\current" >nul || exit /b 1
    )
    >"!LOADER!\rolled-back.part" echo the new runner did not confirm its first start|| exit /b 1
    move /y "!LOADER!\rolled-back.part" "!LOADER!\rolled-back" >nul || exit /b 1
    del /q "!LOADER!\pending"
    if exist "!LOADER!\pending" exit /b 1
    set "NOTE3=Went back to !LOADER_NAME:~0,10!: the new runner did not confirm its first start."
    if defined SCREEN (call :draw) else echo   !NOTE3!
)
call :loader_folder current
set "SRC=!LOADER!\!LOADER_NAME!"

rem --- 1. Rust ------------------------------------------------------------------------------------
set "NEED_RUST="
set "HAVE_TOOLCHAIN="
if exist "%RUST%\.toolchain-version" set /p HAVE_TOOLCHAIN=<"%RUST%\.toolchain-version"
if not "!HAVE_TOOLCHAIN!"=="%RUST_VERSION% %TRIPLE%" set "NEED_RUST=1"
if not exist "%RUST%\bin\cargo.exe" set "NEED_RUST=1"
if not exist "%RUST%\bin\rustc.exe" set "NEED_RUST=1"
if not defined NEED_RUST goto :have_rust
set "CURROW=1"
call :now
set "STEP_T0=!NOW!"
rem One connection is slow and rustc alone is 158 MB, so every archive is fetched as pieces of at
rem most 25 MB, all at once, and joined. A piece that is here whole already is kept, so a stopped
rem download continues. The pieces go into a curl config file (one transfer each); curl runs as a
rem job of this script, and the Rust row shows the bytes arrived as a bar.
set "PIECE=25000000"
set "CFG=%DOWNLOADS%\rust.curl"
set "FETCHED=%DOWNLOADS%\rust.done"
set "FETCHLOG=%LOGS%\download.log"
del "%CFG%" "%FETCHED%" "%FETCHED%.part" 2>nul
set "TOTAL=0" & set "COUNT=0" & set "PIECES="
for %%P in (%PARTS%) do (
    set "FILE=%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz"
    set "HAVE=0"
    if exist "!FILE!" for %%S in ("!FILE!") do set "HAVE=%%~zS"
    if not "!HAVE!"=="!SIZE_%%P!" (
        del "!FILE!" 2>nul
        set /a "TOTAL+=!SIZE_%%P!"
        call :pieces %%P
    )
)
if exist "%CFG%" (
    type nul >"!FETCHLOG!"
    set "FETCHWHAT=Downloading Rust from static.rust-lang.org in !COUNT! pieces"
    start "" /b cmd /d /c call "%SELF%" :fetch_job <nul >nul 2>&1
    call :fetching
    set "FETCH_RESULT=none"
    set /p FETCH_RESULT=<"%FETCHED%"
    if not "!FETCH_RESULT: =!"=="ok" (
        set "T=The download stopped."
        set "T2=Run makepad.bat again to continue it: the pieces that arrived whole stay."
        set "LOGFILE=!FETCHLOG!"
        goto :fail
    )
    del "%CFG%" "%FETCHED%" 2>nul
)
set "K=doing" & set "S=joining" & set "INFO=%D%the pieces of each archive%N%"
set "FOOT=Joining the pieces."
call :put 1
for %%P in (%PARTS%) do if not exist "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" call :join %%P || goto :fail
set "FOOT=Checking each archive against its published SHA-256."
set "CHECKED=0"
for %%P in (%PARTS%) do (
    set "K=doing" & set "S=checking" & set "INFO=!CHECKED! of 4 archives %D%· SHA-256%N%"
    call :put 1
    call :verify "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" "!SHA_%%P!" || (
        del "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz"
        set "T=Checksum mismatch for %%P."
        set "T2=It is downloaded again on the next run: run makepad.bat again."
        goto :fail
    )
    set /a "CHECKED+=1"
)
rem Each archive holds <name>\<part>\{bin,lib}; each part unpacks into its own folder, side by side
rem (a job of this script, so no path is quoted inside another quote), and they merge into one tree.
set "STAGE=%RUST%.unpack"
if exist "%STAGE%" rmdir /s /q "%STAGE%"
mkdir "%STAGE%" || (set "T=Cannot create the folder !STAGE!." & goto :fail)
for %%P in (%PARTS%) do (
    mkdir "%STAGE%\%%P" || (set "T=Cannot create the folder !STAGE!\%%P." & goto :fail)
    start "" /b cmd /d /c call "%SELF%" :unpack_job %%P <nul >nul 2>&1
)
set "FOOT=Unpacking Rust into rust\, the four archives side by side."
:unpacking
set "WAITING=" & set "UNPACKED=0"
for %%P in (%PARTS%) do if exist "%STAGE%\%%P.done" (set /a "UNPACKED+=1") else set "WAITING=1"
set /a "FILLED=UNPACKED*3"
call :bar !FILLED!
set "K=spin" & set "S=unpacking" & set "INFO=!UNPACKED! of 4 archives"
call :put 1
if defined WAITING (call :tick & goto :unpacking)
set "READY=%STAGE%\ready"
mkdir "%READY%" || (set "T=Cannot create the folder !READY!." & goto :fail)
for %%P in (%PARTS%) do (
    set "DONE=none"
    set /p DONE=<"%STAGE%\%%P.done"
    if not "!DONE: =!"=="ok" (set "T=Unpacking %%P failed." & set "LOGFILE=%STAGE%\%%P.log" & set "TAIL=1" & goto :fail)
    robocopy "%STAGE%\%%P" "%READY%" /e /move /njh /njs /nfl /ndl /np >nul
    if errorlevel 8 (set "T=Unpacking %%P failed: it could not be moved into place." & goto :fail)
)
<nul set /p "=%RUST_VERSION% %TRIPLE%" > "%READY%\.toolchain-version"
if exist "%RUST%" rmdir /s /q "%RUST%"
move "%READY%" "%RUST%" >nul || (set "T=Cannot move Rust into !RUST!." & goto :fail)
rmdir /s /q "%STAGE%"
for %%P in (%PARTS%) do del "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" 2>nul
call :since STEP_T0
call :mmss !SINCE! DUR
set "K=ok" & set "S=ready" & set "INFO=%D%downloaded and unpacked in !DUR!%N%"
call :put 1
:have_rust
if not defined NEED_RUST (set "K=ok" & set "S=ready" & set "INFO=%D%in rust\%N%" & call :put 1)

rem --- 2. The runner release and its loader's source, cut from the same Makepad commit (so the two
rem always fit). With arguments a loader compiled before stays as it is (the runner and the apps it
rem recorded were made for it); offline, the one built before.
rem The loader ZIP's SHA-256 names its version, independent of the runner's commit.
rem One source folder for every version (target\ inside it: makepad finds its std two levels up).
set "CURROW=2"
if not exist "%CACHE%\runner" mkdir "%CACHE%\runner"
set "RUNNER=%CACHE%\runner\latest.json"
del "%RUNNER%" 2>nul
set "ASK="
if "%~1"=="" set "ASK=1"
if not exist "%SRC%\.built" set "ASK=1"
if defined ASK (
    set "K=doing" & set "S=asking" & set "INFO=%D%!SVCHOST! for the latest Runner release%N%"
    set "FOOT=Asking the release service for the latest Runner."
    call :put 2
)
if defined ASK curl.exe -fsS --proto %SVCPROTO% -m 60 "%SERVICE%/public/app/runner/latest" -o "%RUNNER%" 2>>"%LOGS%\download.log"
set "REL=" & set "VERSION=" & set "ZIPNAME=" & set "ZIPBYTES=" & set "ZIPSHA="
if exist "%RUNNER%" for /f "tokens=1-5" %%A in ('powershell -NoProfile -Command "$j = Get-Content -Raw -LiteralPath $env:RUNNER | ConvertFrom-Json; if ($j.release -cmatch '\A[A-Za-z0-9_-]+\z' -and $j.loader.name -cmatch '\A[A-Za-z0-9_-]([A-Za-z0-9_.-]*[A-Za-z0-9_-])?\z' -and $j.loader.bytes -cmatch '\A[0-9]+\z' -and $j.loader.sha256 -cmatch '\A[0-9a-f]{64}\z' -and -not ($j.repositories | Where-Object { $_.name -cnotmatch '\A[A-Za-z0-9_-]+\z' -or $_.path -cnotmatch '\A[A-Za-z0-9_/-]+\z' -or $_.commit -cnotmatch '\A([0-9a-f]{40})?\z' })) { '{0} {1} {2} {3} {4}' -f $j.release, $j.loader.sha256, $j.loader.name, $j.loader.bytes, $j.loader.sha256 }"') do (
    set "REL=%%A" & set "VERSION=%%B" & set "ZIPNAME=%%C" & set "ZIPBYTES=%%D" & set "ZIPSHA=%%E"
)
rem What a release names ends up in paths and URLs: anything makepad.sh would not take is no release.
if not defined REL del "%RUNNER%" 2>nul
set "HAVE_LOADER_SHA="
if exist "!SRC!\.loader-sha256" set /p HAVE_LOADER_SHA=<"!SRC!\.loader-sha256"
if not defined VERSION set "VERSION=!HAVE_LOADER_SHA!"
if not defined VERSION if exist "!SRC!\.version" set /p VERSION=<"!SRC!\.version"
if not defined VERSION (
    set "T=No answer from %SVCHOST%, and no loader is here yet."
    set "T2=Check the connection, then run makepad.bat again."
    set "LOGFILE=%LOGS%\download.log"
    goto :fail
)
set "MAKEPAD=%SRC%\target\host\makepad.exe"
set "BUILT=%VERSION% %RUST_VERSION% %TRIPLE%"
set "HAVE_RUST="
if exist "!SRC!\.built" for /f "usebackq tokens=2,3" %%A in ("!SRC!\.built") do set "HAVE_RUST=%%A %%B"
set "HAVE_VERSION="
if exist "!SRC!\.version" set /p HAVE_VERSION=<"!SRC!\.version"
set "K=ok" & set "S=up to date" & set "INFO=%D%!VERSION:~0,10!%N%"
if not defined REL if defined ASK set "S=kept" & set "INFO=%D%!VERSION:~0,10! · the release service did not answer%N%"
if not defined ASK set "S=kept" & set "INFO=%D%!VERSION:~0,10! · the one your arguments run with%N%"
if not "!HAVE_VERSION!"=="!VERSION!" if not "!HAVE_LOADER_SHA!"=="!VERSION!" (
    if not defined ZIPNAME (
        set "T=The release service did not answer; the loader's source is not here yet."
        set "T2=Check the connection, then run makepad.bat again."
        set "LOGFILE=%LOGS%\download.log"
        goto :fail
    )
    call :get_loader || goto :fail
    set "K=doing" & set "S=unpacking" & set "INFO=%D%!ZIPNAME!%N%"
    set "FOOT=Unpacking the loader's source."
    call :put 2
    if exist "%CACHE%\loader\.unpack" rmdir /s /q "%CACHE%\loader\.unpack"
    mkdir "%CACHE%\loader\.unpack" || exit /b 1
    tar.exe -xf "%DOWNLOADS%\%ZIPNAME%" -C "%CACHE%\loader\.unpack" 2>"%LOGS%\unpack.log" || (set "T=Unpacking the loader's source failed. Run makepad.bat again." & set "LOGFILE=%LOGS%\unpack.log" & goto :fail)
    if not exist "%SRC%\Cargo.toml" (
        if exist "%SRC%" rmdir /s /q "%SRC%"
        move "%CACHE%\loader\.unpack\%REL%" "%SRC%" >nul || exit /b 1
    ) else (
        rem an update: files and folders the new version no longer has removed first (deepest first),
        rem so a name that went from file to folder or back is free; then changed files replaced
        rem (with the time of now, so cargo sees them as changed); the rest (and target\) left as they are
        set "K=doing" & set "S=updating" & set "INFO=%D%to !VERSION:~0,10!, only the files that changed%N%"
        set "FOOT=Updating the loader's source: only the files that changed are replaced."
        call :put 2
        powershell -NoProfile -Command ^
            "$ErrorActionPreference = 'Stop'; $new = Join-Path $env:CACHE ('loader\.unpack\' + $env:REL); $src = $env:SRC; $now = Get-Date; $keep = @{};" ^
            "foreach ($f in Get-ChildItem -LiteralPath $new -Recurse) { $keep[$f.FullName.Substring($new.Length + 1)] = $f.PSIsContainer };" ^
            "$gone = Get-ChildItem -LiteralPath $src -Recurse | Where-Object { $rel = $_.FullName.Substring($src.Length + 1);" ^
            "  $rel -ne 'target' -and -not $rel.StartsWith('target\') -and $rel -ne '.version' -and $rel -ne '.built' -and $rel -ne '.loader-sha256' -and $keep[$rel] -ne $_.PSIsContainer };" ^
            "foreach ($f in $gone | Sort-Object { $_.FullName.Length } -Descending) { Remove-Item -Recurse -Force -LiteralPath $f.FullName };" ^
            "foreach ($f in Get-ChildItem -LiteralPath $new -Recurse -File) { $dst = Join-Path $src $f.FullName.Substring($new.Length + 1);" ^
            "  if (-not (Test-Path -LiteralPath $dst) -or (Get-FileHash -LiteralPath $f.FullName).Hash -ne (Get-FileHash -LiteralPath $dst).Hash) {" ^
            "    New-Item -ItemType Directory -Force -Path (Split-Path $dst) | Out-Null; Move-Item -Force -LiteralPath $f.FullName -Destination $dst; (Get-Item -LiteralPath $dst).LastWriteTime = $now } }" ^
            || (set "T=Could not update the loader's source in !SRC!." & goto :fail)
    )
    >"!SRC!\.version.part" echo !VERSION!|| exit /b 1
    move /y "!SRC!\.version.part" "!SRC!\.version" >nul || exit /b 1
    rmdir /s /q "%CACHE%\loader\.unpack"
    del "%DOWNLOADS%\%ZIPNAME%"
    set "K=ok" & set "S=ready" & set "INFO=%D%!VERSION:~0,10!%N%"
)
call :put 2
set "CURROW=3"
if exist "!MAKEPAD!" if exist "!SRC!\target\host\makepad-launcher.exe" if "!HAVE_LOADER_SHA!"=="!VERSION!" if "!HAVE_RUST!"=="!RUST_VERSION! !TRIPLE!" (
    set "K=ok" & set "S=up to date" & set "INFO=%D%for this processor%N%"
    call :put 3
    goto :have_loader
)

rem --- 3. Compile the loader: no Rust settings from elsewhere on this computer; output in cache\
rem rust-mingw keeps the dlltool that raw-dylib imports need beside its linker, where rustc does not
rem look, and has no assembler for it: rustc assembles (assemble.rs).
set "RUST_TOOLS=%RUST%\lib\rustlib\%TRIPLE%\bin\self-contained"
set "PATH=%RUST%\bin;%RUST_TOOLS%;%SystemRoot%\System32;%SystemRoot%;%SystemRoot%\System32\WindowsPowerShell\v1.0"
if not exist "!RUST_TOOLS!\as.exe" (
    set "K=doing" & set "S=compiling" & set "INFO=%D%the assembler for dlltool%N%"
    set "FOOT=Compiling the assembler that Windows' GNU linker tools use."
    call :put 3
    rustc.exe --edition=2021 -O -o "!RUST_TOOLS!\as.exe" "!SRC!\tools\run\entry\assemble.rs" >"!LOGS!\assemble.log" 2>&1 || (set "T=Could not compile the assembler." & set "LOGFILE=!LOGS!\assemble.log" & set "TAIL=1" & goto :fail)
)
set "CARGO_HOME=%CACHE%\cargo-home"
set "CARGO_INCREMENTAL=0"
set "CARGO_TERM_COLOR=never"
set "CARGO_TERM_PROGRESS_WHEN=never"
for %%V in (RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTDOC) do set "%%V="
rem [profile.host]: opt-level 3, 16 codegen units, LTO within each crate; target-cpu=native for this computer's processor
rem and rustc's parallel frontend, which stable rustc takes under RUSTC_BOOTSTRAP=1, with an empty allow list so
rem that no #![feature] builds (the runner's own flags: apps/commercial/runner build_work.rs RUSTFLAGS)
set "RUSTFLAGS=-C target-cpu=native -Z threads=8 -Z allow-features="
set "RUSTC_BOOTSTRAP=1"
set "RUSTC=%RUST%\bin\rustc.exe"
set "LOG=%LOGS%\build.log"
set "COMPILED=0" & set "CRATES=0"
set "BUILD_DONE=%CACHE%\loader\build.done"
del "!BUILD_DONE!" "!BUILD_DONE!.code" 2>nul
type nul >"!LOG!"
rem The crates to compile: this computer's normal and build dependencies of makepad-run, resolved
rem offline from the source (not other targets, not the workspace's dev-dependencies), each once.
set "K=doing" & set "S=counting" & set "INFO=%D%the crates to compile%N%"
set "FOOT=Listing the loader's crates."
call :put 3
pushd "!SRC!" || goto :build_failed
cargo.exe tree --offline --target %TRIPLE% -p makepad-run --edges normal,build --prefix none --format {p} >"!LOGS!\crates.log" 2>"!LOG!" || (popd & set "T=Cannot list the loader's crates from its source." & set "LOGFILE=!LOG!" & set "TAIL=1" & goto :fail)
popd
set "CRATES=0"
for /f "usebackq tokens=1,2" %%A in ("!LOGS!\crates.log") do if not defined CRATE_%%A_%%B (set "CRATE_%%A_%%B=1" & set /a "CRATES+=1")
for /f "delims==" %%V in ('set CRATE_ 2^>nul') do set "%%V="
if "!CRATES!"=="0" (set "T=The loader's source lists no crates." & set "LOGFILE=!LOGS!\crates.log" & goto :fail)
set "CRATES_W=1"
if !CRATES! geq 10 set "CRATES_W=2"
if !CRATES! geq 100 set "CRATES_W=3"
if !CRATES! geq 1000 set "CRATES_W=4"
call :now
set "BUILD_T0=!NOW!"
pushd "!SRC!" || goto :build_failed
rem Expand the exit code in the child, after cargo. Publish the complete done-file by rename.
rem Delayed expansion is off in the parent here; the child's variable values are not rescanned.
setlocal DisableDelayedExpansion
set "ERRORLEVEL="
start "" /b cmd /d /v:on /c "cargo.exe build --profile host --offline -p makepad-run --bin makepad --bin makepad-launcher >"!LOG!" 2>&1 <nul & >"!BUILD_DONE!.code" echo !errorlevel!& move /y "!BUILD_DONE!.code" "!BUILD_DONE!" >nul"
endlocal & set "BUILD_START=%errorlevel%"
popd
if not "!BUILD_START!"=="0" goto :build_failed
:build_wait
call :build_progress
if exist "!BUILD_DONE!" goto :build_done
call :tick
goto :build_wait
:build_done
call :build_progress
set "BUILD="
set /p BUILD=<"!BUILD_DONE!"
del "!BUILD_DONE!" 2>nul
set "RUSTFLAGS="
if not "!BUILD!"=="0" goto :build_failed
>"!SRC!\.built.part" echo !BUILT!|| exit /b 1
move /y "!SRC!\.built.part" "!SRC!\.built" >nul || exit /b 1
>"!SRC!\.loader-sha256.part" echo !VERSION!|| exit /b 1
move /y "!SRC!\.loader-sha256.part" "!SRC!\.loader-sha256" >nul || exit /b 1
call :since BUILD_T0
call :mmss !SINCE! DUR
if !COMPILED! geq !CRATES! (set "INFO=%D%in !DUR! · !CRATES! crates%N%") else set "INFO=%D%in !DUR! · !COMPILED! of !CRATES! crates, the rest kept%N%"
set "K=ok" & set "S=compiled"
call :put 3
goto :have_loader
:build_failed
set "RUSTFLAGS="
set "K=fail" & set "S=failed" & set "INFO=%D%!COMPILED! of !CRATES! crates%N%"
call :put 3
set "T=The loader did not compile."
set "T2=The end of the build log is below."
set "LOGFILE=!LOG!" & set "TAIL=1"
goto :fail
:have_loader
set "PATH=%RUST%\bin;%PATH%"
set "MAKEPAD_LOADER_ROOT=%ROOT%"
set "MAKEPAD_LOADER_SERVICE=%SERVICE%"
set "CURROW=4"
if not "%~1"=="" (
    set "K=wait" & set "S=not started" & set "INFO=%D%the loader runs with your arguments%N%"
    call :put 4
    call :ready "Running the loader with your arguments."
    "%MAKEPAD%" %*
    exit /b !errorlevel!
)

rem --- 4. The runner's launch record, then `makepad` alone runs it (makepad launch.rs) ------------
rem The service's latest licence-free `runner` release: its repositories on the service's read-only
rem git face, the one at `makepad` the workspace, the others mounted at their folders.
if exist "%RUNNER%" (
    powershell -NoProfile -Command ^
        "$ErrorActionPreference = 'Stop'; $j = Get-Content -Raw -LiteralPath $env:RUNNER | ConvertFrom-Json; $g = $env:SERVICE + '/public/git/runner'; $r = $j.release;" ^
        "$l = @('title`tMakepad', 'arg`trun', 'arg`t--quiet', 'arg`t--cached'); $m = @();" ^
        "foreach ($p in $j.repositories) { $u = '{0}/{1}.git#{2}@{3}' -f $g, $p.name, $r, $p.commit;" ^
        "  if ($p.path -eq 'makepad') { $l += ('arg`t' + $u) } elseif ($p.path.StartsWith('makepad/')) { $m += 'arg`t--mount'; $m += ('arg`t' + $p.path.Substring(8) + '=' + $u) } };" ^
        "$l += $m; $l += 'arg`tmakepad-runner';" ^
        "[IO.File]::WriteAllText((Join-Path $env:ROOT 'apps\runner.launch.part'), (($l | ForEach-Object { $_ -replace '`t', [char]9 }) -join [char]10) + [char]10)" ^
        || (set "T=The runner release could not be read." & goto :fail)
    move /y "%ROOT%\apps\runner.launch.part" "%ROOT%\apps\runner.launch" >nul || exit /b 1
)
if not exist "%ROOT%\apps\runner.launch" (
    set "T=No answer from %SVCHOST%, and the Runner was never set up here."
    set "T2=Check the connection, then run makepad.bat again."
    goto :fail
)
set "K=doing" & set "S=starting" & set "INFO=%D%Makepad's loader opens its window%N%"
call :put 4
call :ready "The Runner opens in a moment."
"%MAKEPAD%"
exit /b %errorlevel%

rem :temporary sets NOT_HOME when ROOT is inside %TEMP% or %TMP%. GetFullPath gives each its long
rem name, so a short 8.3 one (JOHNSM~1) matches too; PowerShell reads them, cmd expands no path.
:temporary
for /f %%T in ('powershell -NoProfile -Command "$h = [IO.Path]::GetFullPath($env:ROOT + '\'); foreach ($t in $env:TEMP, $env:TMP) { if ($t -and $h.StartsWith([IO.Path]::GetFullPath($t.TrimEnd('\') + '\'), [StringComparison]::OrdinalIgnoreCase)) { 'temporary'; break } }"') do set "NOT_HOME=%%T"
exit /b 0

rem :loader_folder POINTER returns only a valid folder with a binary, otherwise src.
rem Read untrusted pointer text in PowerShell so cmd never expands it as commands or paths.
:loader_folder
set "LOADER_NAME=src"
set "LOADER_POINTER=%~1"
if not exist "!LOADER!\!LOADER_POINTER!" exit /b 0
for /f "delims=" %%N in ('powershell -NoProfile -Command "try { $n = [IO.File]::ReadAllText((Join-Path $env:LOADER $env:LOADER_POINTER)).Trim(); if ($n -cmatch '\A(src|[0-9a-f]{40})\z' -and [IO.File]::Exists((Join-Path $env:LOADER ($n + '\target\host\makepad.exe')))) { $n } } catch {}"') do set "LOADER_NAME=%%N"
exit /b 0

rem :build_progress counts Cargo's own Compiling lines and keeps the last crate's name, no version:
rem the build row's bar, N / M crates and that crate; the time so far and what is left below.
rem (A line with ! in it loses one level of ^ even inside quotes, hence the doubled ones.)
:build_progress
set "COMPILED=0"
set "CURRENT="
for /f "tokens=2" %%C in ('findstr /r /c:"^^ *Compiling [^^ ][^^ ]* v[0-9]" "!LOG!"') do (
    set /a COMPILED+=1 >nul
    set "CURRENT=%%C"
)
if !COMPILED! gtr !CRATES! set "COMPILED=!CRATES!"
set /a "FILLED=COMPILED*12/CRATES"
call :bar !FILLED!
set "CN_=      !COMPILED!"
for %%W in (!CRATES_W!) do set "CN_=!CN_:~-%%W!"
if defined CURRENT if not "!CURRENT:~17!"=="" set "CURRENT=!CURRENT:~0,16!…"
set "INFO=!CN_! / !CRATES! crates  %D%!CURRENT!%N%"
call :since BUILD_T0
call :mmss !SINCE! SOFAR
set "FOOT=Compiling Makepad's loader for this processor: !SOFAR! so far"
set /a "TENTH=COMPILED*10"
if !COMPILED! gtr 0 if !TENTH! geq !CRATES! (
    set /a "LEFTS=SINCE*(CRATES-COMPILED)/COMPILED"
    call :mmss !LEFTS! LEFT
    set "FOOT=!FOOT!, about !LEFT! left"
)
set "FOOT=!FOOT!."
if not defined SCREEN set "INFO=!INFO! · !SOFAR!"
set "K=spin" & set "S=compiling"
call :put 3
exit /b 0

rem --- The screen's parts ---------------------------------------------------------------------------
rem :put N   sets row N (1 Rust, 2 the loader's source, 3 its build, 4 the Runner) from K (wait, doing,
rem          spin, ok, fail), S (its state, one column), BAR (from :bar, used once) and INFO (the
rem          rest, colours included), and draws the screen. Plain output gets a line when the state
rem          changes, and every tenth tick (about ten seconds) while it stays.
:put
if "%K%"=="wait" (set "SC_=%D%") else set "SC_="
set "S_=!S!            "
set "G_=!GL_%K%!"
if "%K%"=="spin" for %%i in (!SPINI!) do set "G_=%C%!SPINS:~%%i,1!%N%"
set "ROW%1=!G_! !SC_!!S_:~0,12!%N%!BAR!!INFO!"
set "BAR="
if defined HOLD exit /b 0
if defined SCREEN goto :draw
if "!LAST%1!"=="!S!" if !PLAIN_TICKS! lss 10 exit /b 0
set "LAST%1=!S!"
set "PLAIN_TICKS=0"
if defined INFO (echo   !NAME%1!: !S! · !INFO!) else echo   !NAME%1!: !S!
exit /b 0

rem :draw writes the whole screen as one string (one write: nothing flickers), each line cleared to
rem its end after its text; the cursor waits below it. Every line fits 80 columns.
:draw
call :clock
set "E_=              elapsed !CLOCK!"
set "F=%ESC%[2;3H%BC%Makepad%N%%B% Runner%N%%ESC%[K%ESC%[2;66H%D%!E_:~-14!%N%"
set "F=!F!%ESC%[3;3H%D%!WHERE!%N%%ESC%[K%ESC%[4;3H%D%!RULE!%N%%ESC%[5;1H%ESC%[K"
for %%i in (1 2 3 4) do (
    set /a "Y_=5+%%i"
    set "P_=!NAME%%i!               "
    for %%y in (!Y_!) do set "F=!F!%ESC%[%%y;3H!P_:~0,15!!ROW%%i!%N%%ESC%[K"
)
set "F=!F!%ESC%[10;1H%ESC%[K%ESC%[11;3H%D%!RULE!%N%%ESC%[12;3H!FOOT!%N%%ESC%[K%ESC%[13;3H%D%!HINT!%N%%ESC%[K"
set "F=!F!%ESC%[14;3H%Y%!NOTE!%N%%ESC%[K%ESC%[15;3H%D%!NOTE2!%N%%ESC%[K%ESC%[16;3H%Y%!NOTE3!%N%%ESC%[K%ESC%[17;1H"
echo !F!
exit /b 0

rem :frame clears the console for the screen; plain output gets the folder and why it is that one.
:frame
if defined SCREEN (
    cls
    goto :draw
)
echo   !WHERE!
if defined NOTE echo   !NOTE!
if defined NOTE2 echo   !NOTE2!
exit /b 0

rem :ready MESSAGE: every step done; the clock stops and the screen says so, the cursor below it.
:ready
call :clock
set "CLOCKSTOP=1"
set "FOOT=!GL_ok! Ready in !CLOCK!. %~1"
set "HINT="
if defined SCREEN (call :draw) else echo   Ready in !CLOCK!. %~1
exit /b 0

rem :fail shows what went wrong (T, and T2 below it), the current row crossed out, the end of the log
rem it names (TAIL) and where the log is (LOGFILE); a console waits for a key before it closes.
:fail
if defined CURROW if not "!K!"=="fail" (set "K=fail" & set "S=stopped" & call :put !CURROW!)
set "FOOT=!GL_fail! !T!"
set "HINT=!T2!"
if defined SCREEN (
    call :draw
) else (
    echo   Error: !T!
    if defined T2 echo   !T2!
)
if defined TAIL if exist "!LOGFILE!" powershell -NoProfile -Command "Get-Content -LiteralPath $env:LOGFILE -Tail 20"
if defined LOGFILE echo   Full log: !LOGFILE!
if defined CONSOLE (
    echo.
    echo   %D%press a key to close%N%
    pause >nul
)
exit /b 1

rem :bar N   BAR: N of 12 filled, in coral, the rest dim; nothing on plain output.
:bar
set "BAR="
if not defined SCREEN exit /b 0
set /a "BL_=12-%1"
set "BAR=%C%!BARS:~0,%1!%D%!BARO:~0,%BL_%!%N% "
exit /b 0

rem :tick waits about a second (ping waits one between its two echoes) and moves the spinner.
:tick
ping -n 2 127.0.0.1 >nul
set /a "PLAIN_TICKS+=1, SPINI=(SPINI+1)%%SPINN"
exit /b 0

rem :now   NOW: hundredths of a second since midnight, from %TIME% (any separators, hours with a
rem        space or a zero); :since VAR   SINCE: whole seconds since the NOW kept in VAR, over midnight too
:now
set "NOW_=!TIME: =0!"
for /f "tokens=1-4 delims=:.," %%a in ("!NOW_!") do set /a "NOW=((100%%a %% 100)*3600+(100%%b %% 100)*60+(100%%c %% 100))*100+(100%%d %% 100)"
exit /b 0
:since
call :now
set /a "SINCE=NOW-%1"
if !SINCE! lss 0 set /a "SINCE+=8640000"
set /a "SINCE/=100"
exit /b 0

rem :mmss SECONDS VAR   VAR: m:ss, or h:mm:ss from an hour on; :clock   CLOCK: the time so far
:mmss
set /a "HH_=%1/3600, MM_=%1/60%%60, SS_=%1%%60+100"
if !HH_! gtr 0 (set /a "MM_+=100" & set "%2=!HH_!:!MM_:~1!:!SS_:~1!") else set "%2=!MM_!:!SS_:~1!"
exit /b 0
:clock
if defined CLOCKSTOP exit /b 0
call :since T0
call :mmss !SINCE! CLOCK
exit /b 0

rem :pieces PART adds the pieces of PART to PIECES, and those not here whole to the curl config.
:pieces
set "SIZE=!SIZE_%1!"
set /a "IDX=0, FROM=0"
:pieces_next
set /a "TO=FROM+PIECE-1"
if !TO! geq !SIZE! set /a "TO=SIZE-1"
set /a "LENGTH=TO-FROM+1"
set "FILE=%DOWNLOADS%\%1-%RUST_VERSION%-%TRIPLE%.tar.gz.!IDX!"
set "PIECES=!PIECES! "!FILE!""
set "HAVE=0"
if exist "!FILE!" for %%A in ("!FILE!") do set "HAVE=%%~zA"
rem Each piece is its own transfer (next), with its own range; curl reads the path with forward
rem slashes, which need no escaping in its config.
if not "!HAVE!"=="!LENGTH!" (
    del "!FILE!" 2>nul
    if exist "%CFG%" >>"%CFG%" echo next
    >>"%CFG%" echo url = "%DIST%/%1-%RUST_VERSION%-%TRIPLE%.tar.gz"
    >>"%CFG%" echo output = "!FILE:\=/!"
    >>"%CFG%" echo range = !FROM!-!TO!
    >>"%CFG%" echo proto = =https
    >>"%CFG%" echo fail
    >>"%CFG%" echo retry = 3
    set /a "COUNT+=1"
)
set /a "IDX+=1, FROM=TO+1"
if !FROM! lss !SIZE! goto :pieces_next
exit /b 0

rem :fetching draws the download on row CURROW until the job ends: the bar, the percent, MB of MB
rem and the speed (over the last few seconds), and below it the time left once there is a speed.
:fetching
call :fetch_sum
call :now
set /a "LASTKB=SUM/1024, LASTT=NOW, SPEED=0, TICKS=0, KTOTAL=TOTAL/1024, TMB=TOTAL/1048576"
set "UNIT=MB" & set "UNITW=3" & set "UNITB=1048576"
if !TOTAL! lss 10485760 (set "UNIT=KB" & set "UNITW=5" & set "UNITB=1024" & set /a "TMB=TOTAL/1024")
:fetching_tick
call :fetch_sum
call :now
set /a "DT=NOW-LASTT, KB=SUM/1024"
if !DT! lss 0 set /a "DT+=8640000"
if !DT! geq 50 (
    set /a "INST=(KB-LASTKB)*100/DT, LASTKB=KB, LASTT=NOW, TICKS+=1"
    if !TICKS! equ 1 (set /a "SPEED=INST") else set /a "SPEED=(SPEED*3+INST)/4"
)
if !TOTAL! geq 1200 (set /a "PCT=SUM/(TOTAL/100), FILLED=SUM/(TOTAL/12)") else (set "PCT=0" & set "FILLED=0")
if !PCT! gtr 100 set "PCT=100"
if !FILLED! gtr 12 set "FILLED=12"
call :bar !FILLED!
set /a "SMB=SUM/UNITB"
set "PCT_=  !PCT!" & set "SMB_=    !SMB!" & set "SPD="
for %%w in (!UNITW!) do set "SMB_=!SMB_:~-%%w!"
if !SPEED! gtr 0 (
    set /a "T10=SPEED*10/1024, SW=T10/10, SF=T10%%10"
    set "SPD=          !SW!.!SF! MB/s"
    set "SPD=  !SPD:~-10!"
)
set "FOOT=!FETCHWHAT!."
if !SPEED! gtr 0 if !TICKS! geq 3 (
    set /a "LEFTS=(KTOTAL-KB)/SPEED"
    if !LEFTS! lss 0 set "LEFTS=0"
    call :mmss !LEFTS! LEFT
    set "FOOT=!FETCHWHAT! · about !LEFT! left."
)
set "K=spin" & set "S=downloading" & set "INFO=!PCT_:~-3!%%  !SMB_! of !TMB! !UNIT!!SPD!"
call :put !CURROW!
if exist "!FETCHED!" exit /b 0
call :tick
goto :fetching_tick

:fetch_sum
set "SUM=0"
for %%F in (!PIECES!) do if exist %%F set /a "SUM+=%%~zF"
exit /b 0

rem :get_loader downloads the loader's source (a job, with the download's bar on its row) and checks
rem its size and SHA-256; T says why when it returns 1.
:get_loader
set "ZIP=%DOWNLOADS%\%ZIPNAME%"
del "!ZIP!" 2>nul
set "CFG=%DOWNLOADS%\loader.curl"
set "FETCHED=%DOWNLOADS%\loader.done"
set "FETCHLOG=%LOGS%\download.log"
del "!CFG!" "!FETCHED!" "!FETCHED!.part" 2>nul
>"!CFG!" echo url = "%SERVICE%/public/app/runner/%REL%/%ZIPNAME%"
>>"!CFG!" echo output = "!ZIP:\=/!"
>>"!CFG!" echo proto = %SVCPROTO%
>>"!CFG!" echo fail
>>"!CFG!" echo retry = 3
set "TOTAL=%ZIPBYTES%"
set "PIECES="!ZIP!""
set "FETCHWHAT=Downloading the loader's source from !SVCHOST!"
start "" /b cmd /d /c call "%SELF%" :fetch_job <nul >nul 2>&1
call :fetching
set "FETCH_RESULT=none"
set /p FETCH_RESULT=<"!FETCHED!"
del "!CFG!" "!FETCHED!" 2>nul
if not "!FETCH_RESULT: =!"=="ok" (
    set "T=The download of the loader's source stopped."
    set "T2=Run makepad.bat again to continue."
    set "LOGFILE=!FETCHLOG!"
    exit /b 1
)
set "HAVE=0"
for %%S in ("!ZIP!") do set "HAVE=%%~zS"
if not "!HAVE!"=="%ZIPBYTES%" (del "!ZIP!" & set "T=The loader's source has the wrong size." & set "T2=Run makepad.bat again." & exit /b 1)
set "K=doing" & set "S=checking" & set "INFO=%D%!ZIPNAME! · SHA-256%N%"
set "FOOT=Checking the loader's source against its SHA-256."
call :put 2
call :verify "!ZIP!" "%ZIPSHA%" || (del "!ZIP!" & set "T=Checksum mismatch for %ZIPNAME%." & set "T2=Run makepad.bat again." & exit /b 1)
exit /b 0

rem :join PART puts the pieces of PART together into its archive.
:join
set "BASE=%DOWNLOADS%\%1-%RUST_VERSION%-%TRIPLE%.tar.gz"
set "LIST="
set /a "IDX=0"
:join_next
if not exist "!BASE!.!IDX!" goto :join_done
if defined LIST (set "LIST=!LIST!+"!BASE!.!IDX!"") else (set "LIST="!BASE!.!IDX!"")
set /a "IDX+=1"
goto :join_next
:join_done
copy /b !LIST! "!BASE!.joined" >nul || (set "T=Could not join the pieces of %1." & exit /b 1)
for /l %%I in (0,1,!IDX!) do del "!BASE!.%%I" 2>nul
move /y "!BASE!.joined" "!BASE!" >nul || (set "T=Could not join the pieces of %1." & exit /b 1)
exit /b 0

rem :verify FILE SHA256 checks a file's SHA-256 (certutil's answer, spaces removed, any case).
:verify
set "GOT="
for /f "skip=1 tokens=*" %%L in ('certutil.exe -hashfile "%~1" SHA256') do if not defined GOT set "GOT=%%L"
set "GOT=%GOT: =%"
if /i "%GOT%"=="%~2" exit /b 0
exit /b 1

rem --- The jobs ---------------------------------------------------------------------------------
rem Every transfer of the config at once, over as many connections; FETCHED says ok or failed.
:fetch_job
curl.exe -sS -Z --parallel-immediate --parallel-max 16 -K "%CFG%" 2>>"%FETCHLOG%" && (set "RESULT=ok") || (set "RESULT=failed")
>"%FETCHED%.part" echo %RESULT%
move /y "%FETCHED%.part" "%FETCHED%" >nul
exit /b 0

rem One part of Rust unpacked into its own folder; its .done file says ok or failed.
:unpack_job
tar.exe -xzf "%DOWNLOADS%\%~2-%RUST_VERSION%-%TRIPLE%.tar.gz" --strip-components=2 -C "%STAGE%\%~2" 2>"%STAGE%\%~2.log" && (set "RESULT=ok") || (set "RESULT=failed")
>"%STAGE%\%~2.part" echo %RESULT%
move /y "%STAGE%\%~2.part" "%STAGE%\%~2.done" >nul
exit /b 0
