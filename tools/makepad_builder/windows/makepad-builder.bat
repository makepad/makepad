@echo off
rem The download runs beside the screen as this script's own job.
if "%~1"==":fetch_job" goto :fetch_job
if "%~1"==":unpack_job" goto :unpack_job
rem Makepad Builder for Windows, compiled from source on this computer and
rem run in this window.
rem
rem This download holds no program of ours: the Builder's source is in
rem builder\source, and you can read all of it. This script downloads the
rem official Rust compiler for Windows (the x86_64-pc-windows-gnu toolchain,
rem from static.rust-lang.org, checked against the SHA-256 sums written
rem below), compiles the Builder with it, and starts it here. Everything it
rem and the Builder write stays in builder\; beside this file there are only
rem makepad-builder.exe (it starts the Builder too) and the apps you build.
rem Run it again at any time: the compiler is downloaded once and the Builder
rem is compiled again only when its source changed.
rem
rem Besides what it downloads it runs only Windows' own curl.exe, tar.exe and
rem certutil.exe.
setlocal EnableExtensions EnableDelayedExpansion

set "ROOT=%~dp0"
set "ROOT=%ROOT:~0,-1%"
set "STATE=%ROOT%\builder"
set "SOURCE_DIR=%STATE%\source"
set "RUST_VERSION=1.98.0"
set "TRIPLE=x86_64-pc-windows-gnu"
set "DIST=https://static.rust-lang.org/dist"
rem Where the Builder itself keeps this Rust too: it compiles apps with it
rem when you choose Rust's GNU toolchain.
set "TOOLCHAIN=%STATE%\toolchain\rust\%RUST_VERSION%-%TRIPLE%"
set "DOWNLOADS=%STATE%\cache"
set "PARTS=rustc cargo rust-std rust-mingw"

rem The four parts of Rust a build needs, with their sizes and published
rem SHA-256.
set "SIZE_rustc=158249903"
set "SIZE_cargo=18429051"
set "SIZE_rust-std=43334608"
set "SIZE_rust-mingw=9594885"
set "SHA_rustc=c97a98af7a8c84cee611a9a8a5bba6a804f82b09899a2e451a33b5934d684542"
set "SHA_cargo=79da7e6bb0c7ac1583b380c702477da298f2dace9ba22f41150698c516533406"
set "SHA_rust-std=47bceabaceabc4de97e20f7470d9973fb20e844986da01b3a31cf28e24596660"
set "SHA_rust-mingw=14fc86abb02d7d0575eb38a738161dcd87aa4032e091cce3424e78321b145e5e"

title Makepad Builder
rem The Builder's own look: the same header, rows, rule and footer as its
rem TUI, in plain terminal colours (bold title, dim secondary text, cyan for
rem work in progress, green checks).
chcp 65001 >nul
for /f %%E in ('echo prompt $E ^| cmd') do set "ESC=%%E"
set "B=%ESC%[0;1m"
set "D=%ESC%[0;2m"
set "C=%ESC%[0;36m"
set "G=%ESC%[0;32m"
set "Y=%ESC%[0;33m"
set "R=%ESC%[0;31m"
set "N=%ESC%[0m"
set "RULE=────────────────────────────────────────────────────────────────────────────"
set "FRAMED="
cd /d "%ROOT%" || goto :fail
rem A folder an earlier download unpacked the source into builder\ itself.
if exist "%STATE%\Cargo.toml" if exist "%SOURCE_DIR%\Cargo.toml" (
    for %%F in (Cargo.toml Cargo.lock) do del "%STATE%\%%F" 2>nul
    for %%F in (libs tools) do if exist "%STATE%\%%F" rmdir /s /q "%STATE%\%%F"
)
if not exist "%SOURCE_DIR%\Cargo.toml" (
    set "T=%Y%The Builder's source is missing.%N% Unzip the whole download into one folder."
    goto :fail
)

rem What there is to do: nothing (start the Builder), compile it, or first
rem download Rust too.
set "NEED_RUST="
if not exist "%TOOLCHAIN%\.toolchain-version" set "NEED_RUST=1"
if not exist "%TOOLCHAIN%\bin\cargo.exe" set "NEED_RUST=1"
set "BUILT="
if exist "%STATE%\target\builder\built-from" set /p BUILT=<"%STATE%\target\builder\built-from"
set "SOURCE="
set /p SOURCE=<"%SOURCE_DIR%\Cargo.toml"
set "NEED_BUILD="
if not exist "%STATE%\target\builder\release\makepad-builder.exe" set "NEED_BUILD=1"
if not "!BUILT!"=="!SOURCE!" set "NEED_BUILD=1"
if defined NEED_RUST set "NEED_BUILD=1"
if defined NEED_BUILD call :frame

rem --- 1. The Rust compiler: downloaded in pieces side by side, checked,
rem then unpacked side by side ----------------------------------------------
if not defined NEED_RUST goto :have_rust
where curl.exe >nul 2>nul || (set "T=%Y%This Windows has no curl.exe;%N% Windows 10 version 1803 or later is needed." & goto :fail)
where tar.exe >nul 2>nul || (set "T=%Y%This Windows has no tar.exe;%N% Windows 10 version 1803 or later is needed." & goto :fail)
if not exist "%DOWNLOADS%" mkdir "%DOWNLOADS%" || goto :fail
rem One connection is slow and rustc alone is 158 MB, so every archive is
rem fetched as pieces of at most 25 MB, all at once, and joined. A piece
rem that is already here whole is kept, so a stopped download continues.
rem The pieces to fetch go into a curl config file (one transfer each);
rem curl runs as a background job of this script, and the Rust row shows
rem the bytes arrived as a bar, like the Builder's own.
set "PIECE=25000000"
set "CURLCFG=%DOWNLOADS%\rust-download.curl"
set "FETCHED=%DOWNLOADS%\rust-download.done"
del "%CURLCFG%" "%FETCHED%" 2>nul
set /a "TOTAL=0"
for %%P in (%PARTS%) do if not exist "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" (
    set /a "TOTAL+=!SIZE_%%P!"
    call :pieces %%P
)
if exist "%CURLCFG%" (
    call :status "Downloading Rust from static.rust-lang.org; once, into builder\cache."
    set /a "TOTAL_MB=TOTAL/1048576"
    start "" /b cmd /d /c call "%~f0" :fetch_job
    call :fetching
    set /p FETCH_RESULT=<"%FETCHED%"
    if not "!FETCH_RESULT: =!"=="ok" (
        set "T=%R%✗%N% The download stopped. %D%Run makepad-builder.bat again to continue it.%N%"
        goto :fail
    )
    del "%CURLCFG%" "%FETCHED%" "%CURLCFG%.log" 2>nul
    call :status "Setting up this folder once; nothing outside it changes."
)
for %%P in (%PARTS%) do if not exist "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" call :join %%P || goto :fail
set "T=%C%●%N% checking %D%· SHA-256 of each archive%N%"
call :row 5 "Rust %RUST_VERSION%"
for %%P in (%PARTS%) do (
    call :verify "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" "!SHA_%%P!" || (del "%DOWNLOADS%\%%P-%RUST_VERSION%-%TRIPLE%.tar.gz" & goto :fail)
)
set "T=%C%●%N% unpacking %D%· 0 of 4%N%"
call :row 5 "Rust %RUST_VERSION%"
set "STAGE=%TOOLCHAIN%.unpack"
if exist "%STAGE%" rmdir /s /q "%STAGE%"
mkdir "%STAGE%" || goto :fail
rem Each archive holds <name>\<part>\{bin,lib}; the parts merge into one tree,
rem each in its own directory first so they unpack side by side.
for %%P in (%PARTS%) do (
    mkdir "%STAGE%\%%P" || goto :fail
    del "%STAGE%\%%P.done" 2>nul
    rem A job of this script, so no path is quoted inside another quote
    rem (a folder named "makepad-builder (2)" ended such a line early).
    start "" /b cmd /d /c call "%~f0" :unpack_job %%P
)
:unpacking
set "WAITING="
set /a "UNPACKED=0"
for %%P in (%PARTS%) do if not exist "%STAGE%\%%P.done" (set "WAITING=1") else (set /a "UNPACKED+=1")
if defined WAITING (
    set "T=%C%●%N% unpacking %D%· !UNPACKED! of 4%N%"
    call :row 5 "Rust %RUST_VERSION%"
    ping -n 2 127.0.0.1 >nul
    goto :unpacking
)
set "READY=%STAGE%\ready"
mkdir "%READY%" || goto :fail
for %%P in (%PARTS%) do (
    set /p DONE=<"%STAGE%\%%P.done"
    if not "!DONE: =!"=="ok" (set "T=%R%✗%N% Unpacking %%P failed." & goto :fail)
    robocopy "%STAGE%\%%P" "%READY%" /e /move /njh /njs /nfl /ndl /np >nul
    if errorlevel 8 (set "T=%R%✗%N% Unpacking %%P failed." & goto :fail)
)
rem The same stamp the Builder writes for a Rust it installed itself.
<nul set /p "=%RUST_VERSION% %TRIPLE%" > "%READY%\.toolchain-version"
if exist "%TOOLCHAIN%" rmdir /s /q "%TOOLCHAIN%"
move "%READY%" "%TOOLCHAIN%" >nul || goto :fail
rmdir /s /q "%STAGE%"
set "T=%G%✓%N% ready %D%· in builder\toolchain%N%"
call :row 5 "Rust %RUST_VERSION%"
:have_rust

rem --- 2. Compile the Builder ------------------------------------------------
rem The build sees no Rust settings from elsewhere on this computer, and
rem writes only into builder\.
set "PATH=%TOOLCHAIN%\bin;%SystemRoot%\System32;%SystemRoot%;%SystemRoot%\System32\WindowsPowerShell\v1.0"
set "CARGO_HOME=%STATE%\cargo-home"
set "CARGO_TARGET_DIR=%STATE%\target\builder"
for %%V in (RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS RUSTC_WRAPPER RUSTC RUSTDOC) do set "%%V="
rem A different source (a new download unzipped here) compiles from clean:
rem its files carry the ZIP's timestamps, which Cargo would take as older
rem than the last build.
if defined NEED_BUILD if exist "%STATE%\target\builder\built-from" rmdir /s /q "%STATE%\target\builder"
if defined NEED_BUILD (
    set "T=%C%●%N% compiling from builder\source %D%· about a minute%N%"
    call :row 7 "Makepad Builder"
    call :status "Compiling the Builder from its source."
    call :place 8
)
cargo.exe build --release --offline --locked --quiet --manifest-path "%SOURCE_DIR%\Cargo.toml" -p makepad-loader --bin makepad-builder
if errorlevel 1 (
    set "T=%R%✗%N% The Builder did not compile. %D%Nothing outside this folder was changed.%N%"
    goto :fail
)
if defined NEED_BUILD (
    >"%STATE%\target\builder\built-from" echo(!SOURCE!
    set "T=%G%✓%N% compiled"
    call :row 7 "Makepad Builder"
    call :status "Starting the Builder."
)
if "%MAKEPAD_BUILDER_BUILD_ONLY%"=="1" exit /b 0

rem --- 3. Run it, in this window ------------------------------------------
rem Beside this script, where you and coding agents find it (makepad-builder.exe
rem rebuild). A copy that is running already stays as it is.
fc /b "%STATE%\target\builder\release\makepad-builder.exe" "%ROOT%\makepad-builder.exe" >nul 2>nul || copy /y "%STATE%\target\builder\release\makepad-builder.exe" "%ROOT%\makepad-builder.exe" >nul 2>nul
set "CARGO_TARGET_DIR="
set "CARGO_HOME="
set "MAKEPAD_LOADER_ROOT=%ROOT%"
"%ROOT%\makepad-builder.exe" tui %*
exit /b %errorlevel%

rem ---------------------------------------------------------------------------
rem The screen, as the TUI draws its work page: header, rows, rule, status
rem and footer. Rows are rewritten in place.
:frame
set "FRAMED=1"
cls
echo.
echo   %B%Makepad%N% commercial apps › Getting started
echo   %D%Makepad apps ship as source, so your own coding agent can customize them.%N%
echo.
if defined NEED_RUST (set "T=%D%○ downloads first%N%") else (set "T=%G%✓%N% ready")
call :row 5 "Rust %RUST_VERSION%"
set "T=%D%○ compiles from builder\source once Rust is here%N%"
call :row 7 "Makepad Builder"
echo %ESC%[10;1H%ESC%[2K  %D%%RULE%%N%
call :status "Setting up this folder once; nothing outside it changes."
echo %ESC%[13;1H%ESC%[2K  %D%working · ctrl+c stops%N%
exit /b 0

rem :row N "name" writes T after the name column on screen row N.
:row
set "NAME=%~2                      "
echo %ESC%[%1;1H%ESC%[2K  !NAME:~0,20!!T!
exit /b 0

:clear
echo %ESC%[%1;1H%ESC%[2K
exit /b 0

:status
echo %ESC%[11;1H%ESC%[2K  %~1
exit /b 0

rem Put the cursor on row N (where curl draws its bar).
:place
<nul set /p "=%ESC%[%1;1H%ESC%[2K"
exit /b 0

rem :pieces PART adds the pieces of PART not yet here to FETCH.
:pieces
set "SIZE=!SIZE_%1!"
set /a "IDX=0, FROM=0"
:pieces_next
set /a "TO=FROM+PIECE-1"
if !TO! geq !SIZE! set /a "TO=SIZE-1"
set /a "LENGTH=TO-FROM+1"
set "FILE=%DOWNLOADS%\%1-%RUST_VERSION%-%TRIPLE%.tar.gz.!IDX!"
set "HAVE=0"
if exist "!FILE!" for %%A in ("!FILE!") do set "HAVE=%%~zA"
rem Each piece is its own transfer (next), with its own range; curl reads
rem the path with forward slashes, which need no escaping in its config.
if not "!HAVE!"=="!LENGTH!" (
    if exist "%CURLCFG%" >>"%CURLCFG%" echo next
    >>"%CURLCFG%" echo url = "%DIST%/%1-%RUST_VERSION%-%TRIPLE%.tar.gz"
    >>"%CURLCFG%" echo output = "!FILE:\=/!"
    >>"%CURLCFG%" echo range = !FROM!-!TO!
    >>"%CURLCFG%" echo fail
    >>"%CURLCFG%" echo location
    >>"%CURLCFG%" echo retry = 3
)
set /a "IDX+=1, FROM=TO+1"
if !FROM! lss !SIZE! goto :pieces_next
exit /b 0

rem :fetching draws the download's bar on the Rust row until the job ends.
:fetching
set /a "SUM=0"
for %%F in ("%DOWNLOADS%\*-%RUST_VERSION%-%TRIPLE%.tar.gz.*") do if not "%%~xF"==".gz" if not "%%~xF"==".joined" set /a "SUM+=%%~zF"
set /a "PERCENT=SUM/(TOTAL/100), FILLED=SUM/(TOTAL/24), SUM_MB=SUM/1048576"
if !PERCENT! gtr 100 set "PERCENT=100"
if !FILLED! gtr 24 set "FILLED=24"
set "DONE_BAR="
set "LEFT_BAR="
for /l %%I in (1,1,24) do if %%I leq !FILLED! (set "DONE_BAR=!DONE_BAR!━") else (set "LEFT_BAR=!LEFT_BAR!─")
set "PERCENT=  !PERCENT!"
set "T=%C%●%N% %C%!DONE_BAR!%D%!LEFT_BAR!%N% !PERCENT:~-3!%% %D%· !SUM_MB! / !TOTAL_MB! MB%N%"
call :row 5 "Rust %RUST_VERSION%"
if exist "%FETCHED%" exit /b 0
ping -n 2 127.0.0.1 >nul
goto :fetching

rem The background job: every piece at once, over as many connections.
:fetch_job
curl.exe -sS -Z --parallel-immediate --parallel-max 16 -K "%CURLCFG%" 2>"%CURLCFG%.log" && (>"%FETCHED%" echo ok) || (>"%FETCHED%" echo failed)
exit /b 0

rem The background job that unpacks one part.
:unpack_job
tar.exe -xzf "%DOWNLOADS%\%~2-%RUST_VERSION%-%TRIPLE%.tar.gz" --strip-components=2 -C "%STAGE%\%~2" && (>"%STAGE%\%~2.done" echo ok) || (>"%STAGE%\%~2.done" echo failed)
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
copy /b !LIST! "!BASE!.joined" >nul || (set "T=%R%✗%N% Could not join the pieces of %1." & exit /b 1)
for /l %%I in (0,1,!IDX!) do del "!BASE!.%%I" 2>nul
move /y "!BASE!.joined" "!BASE!" >nul
exit /b 0

:verify
set "GOT="
for /f "skip=1 tokens=*" %%L in ('certutil.exe -hashfile "%~1" SHA256') do if not defined GOT set "GOT=%%L"
set "GOT=!GOT: =!"
if /i not "!GOT!"=="%~2" (
    set "T=%R%✗%N% Checksum mismatch for %~nx1 %D%· downloaded again on the next run%N%"
    exit /b 1
)
exit /b 0

:fail
if not defined FRAMED (
    echo.
    echo   !T!
    echo.
    echo   %D%press a key to close%N%
) else (
    echo %ESC%[11;1H%ESC%[2K  !T!
    echo %ESC%[13;1H%ESC%[2K  %D%press a key to close%N%
)
pause >nul
exit /b 1
