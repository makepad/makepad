# First-run test of makepad.bat in a fresh folder against a local release service serving tiny
# fixture loaders (the same fixture as test.sh): ctrl+c while Rust downloads (in a pseudo console),
# first run, unchanged run (no rebuild), changed loader (rebuild), the coding-agent form (makepad.ps1
# fetching makepad.bat and running `makepad.bat open fab`), broken loader (error, log excerpt and
# path).
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
    New-Item -ItemType Directory -Force -Path "$zipdir\fx1\dep-a", "$zipdir\fx1\tools\run\entry", "$zipdir\fx1\libs\loader_switch\src" | Out-Null
    $switchSource = "$here\..\..\..\libs\loader_switch"
    Copy-Item -LiteralPath "$switchSource\Cargo.toml" -Destination "$zipdir\fx1\libs\loader_switch\Cargo.toml"
    Copy-Item -LiteralPath "$switchSource\src\lib.rs" -Destination "$zipdir\fx1\libs\loader_switch\src\lib.rs"
    $files = @{
        'fx1/Cargo.toml' = "[package]`nname = `"makepad-run`"`nversion = `"0.1.0`"`nedition = `"2021`"`n[dependencies]`ndep-a = { path = `"dep-a`" }`nmakepad-loader-switch = { path = `"libs/loader_switch`" }`n[[bin]]`nname = `"makepad`"`npath = `"main.rs`"`n[[bin]]`nname = `"makepad-launcher`"`npath = `"launcher.rs`"`n[profile.host]`ninherits = `"release`"`n"
        'fx1/Cargo.lock' = "version = 4`n`n[[package]]`nname = `"dep-a`"`nversion = `"0.1.0`"`n`n[[package]]`nname = `"makepad-loader-switch`"`nversion = `"0.1.0`"`n`n[[package]]`nname = `"makepad-run`"`nversion = `"0.1.0`"`ndependencies = [`n `"dep-a`",`n `"makepad-loader-switch`",`n]`n"
        'fx1/dep-a/Cargo.toml' = "[package]`nname = `"dep-a`"`nversion = `"0.1.0`"`nedition = `"2021`"`n[lib]`npath = `"lib.rs`"`n"
        'fx1/dep-a/lib.rs' = "pub fn text() -> &'static str { `"$text`" }`n"
        'fx1/launcher.rs' = "fn main() {}`n"
        # makepad.bat compiles the assembler rust-mingw's dlltool needs from the loader's source
        'fx1/tools/run/entry/assemble.rs' = [IO.File]::ReadAllText("$here\assemble.rs")
        'fx1/main.rs' = $(if ($broken) { "fn main() { this is not rust }`n" } else {
@'
fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("RELAX_HOME").unwrap());
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|cmd| cmd == "recover") {
        std::process::exit(makepad_loader_switch::recover_command(&home.join("cache/loader"), &args[1..]));
    }
    let home = home.canonicalize().unwrap();
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    let loaders = home.join("cache/loader");
    let folder = if target.parent() == Some(home.join("target/loader").as_path()) {
        loaders.join(target.file_name().unwrap())
    } else {
        assert!(target.ends_with("target") && target.parent().unwrap().parent() == Some(loaders.as_path()));
        target.parent().unwrap().to_path_buf()
    };
    let record = folder.join("runner.launch");
    println!("fixture loader: {} launch record present: {} args: {:?}", dep_a::text(), record.is_file(), args);
    println!("fixture loader folder: {}", folder.file_name().unwrap().to_string_lossy());
}
'@
        })
    }
    foreach ($k in $files.Keys) { [IO.File]::WriteAllText("$zipdir\$($k.Replace('/', '\'))", $files[$k], $utf8) }
    $zip = "$Base\www\public\app\runner\fx1\loader.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Push-Location $zipdir
    & tar.exe -a -cf $zip fx1
    Pop-Location
    $sum = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    $commit = '0123456789abcdef0123456789abcdef01234567'
    $json = '{"release":"fx1","repositories":[{"name":"makepad","path":"makepad","commit":"' + $commit + '"},{"name":"extra","path":"makepad/apps/extra","commit":""}],"loader":{"name":"loader.zip","bytes":' + (Get-Item $zip).Length + ',"sha256":"' + $sum + '","commit":"' + $commit + '"}}'
    [IO.File]::WriteAllText("$Base\www\public\app\runner\latest", $json, $utf8)
}
# the entry script as the server sends it: CRLF line ends
$bat = (Get-Content -Raw -Encoding UTF8 "$here\makepad.bat") -replace "`r?`n", "`r`n"
[IO.File]::WriteAllText("$Base\home\makepad.bat", $bat, $utf8)
# the one-line PowerShell entry and the download it fetches (makepad.bat in a ZIP), where the site
# serves them
New-Item -ItemType Directory -Force -Path "$Base\batzip", "$Base\www\runner" | Out-Null
[IO.File]::WriteAllText("$Base\batzip\makepad.bat", $bat, $utf8)
Push-Location "$Base\batzip"
& tar.exe -a -cf "$Base\www\runner\windows.zip" makepad.bat
Pop-Location
Move-Item "$Base\www\runner\windows.zip" "$Base\www\runner\windows"
[IO.File]::WriteAllText("$Base\www\runner\windows.ps1", [IO.File]::ReadAllText("$here\makepad.ps1"), $utf8)

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
            if ($f.EndsWith('.ps1')) { $c.Response.ContentType = 'text/plain; charset=utf-8' }
            $c.Response.OutputStream.Write($b, 0, $b.Length)
        } else { $c.Response.StatusCode = 404 }
        $c.Response.Close()
    }
}
Start-Sleep 2
$env:MAKEPAD_LOADER_SERVICE = "http://127.0.0.1:$port"
$env:MAKEPAD_LOADER_ROOT = "$Base\install"
function Run($name) {
    # every run here has a folder of its own, never the profile's
    if ($env:MAKEPAD_LOADER_ROOT -ne "$Base\install") { throw "a run without its own MAKEPAD_LOADER_ROOT" }
    $out = & cmd.exe /d /c "`"$Base\home\makepad.bat`" 2>&1" | Out-String
    $script:code = $LASTEXITCODE
    "=== $name (exit $script:code)"; $out
    return $out
}
# The same entry runs against each target layout. Its only job while pending is selection.
function Check-PendingEntry($layout) {
    foreach ($previous in $hex, '../outside', $unbuilt) {
        if ($previous -eq $hex) { $selected = 'src'; $expectedFolder = $hex }
        else { $selected = $hex; $expectedFolder = 'src' }
        [IO.File]::WriteAllText("$loader\current", "$selected`n", $utf8)
        [IO.File]::WriteAllText("$loader\previous", "$previous`n", $utf8)
        [IO.File]::WriteAllText("$loader\pending", "$hex`n", $utf8)
        $entry = & "$Base\install\makepad.cmd" 2>&1 | Out-String
        Check "pending entry ($layout): exits 0" "$LASTEXITCODE" '^0$'
        Check "pending entry ($layout): previous $previous" $entry "(?m)^fixture loader folder: $expectedFolder\r?$"
        Check "pending entry ($layout): switch state intact" (([IO.File]::ReadAllText("$loader\current") -ceq "$selected`n") -and
            ([IO.File]::ReadAllText("$loader\previous") -ceq "$previous`n") -and ([IO.File]::ReadAllText("$loader\pending") -ceq "$hex`n") -and
            !(Test-Path "$loader\rolled-back")) '^True$'
    }
    Remove-Item -LiteralPath "$loader\pending", "$loader\previous"
}
# ctrl+c in a console (a pseudo console, as Windows Terminal hosts) while Rust downloads: the jobs
# beside the screen stop with the script, whatever cmd's "Terminate batch job (Y/N)?" is answered.
# Y ends it; N goes on to a screen that says the download stopped (a key closes it). Afterwards no
# process names the test's folder, the pieces downloaded stop growing, and the download's done file
# does not say ok: a download that ran on unseen ends ok there, however fast the line is.
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public class Con {
    [StructLayout(LayoutKind.Sequential)] public struct COORD { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct STARTUPINFO {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars;
        public int dwFillAttribute; public int dwFlags; public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2;
        public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError;
    }
    [StructLayout(LayoutKind.Sequential)] public struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CreatePipe(out SafeFileHandle r, out SafeFileHandle w, IntPtr sa, int size);
    [DllImport("kernel32.dll", SetLastError = true)] static extern int CreatePseudoConsole(COORD size, SafeFileHandle input, SafeFileHandle output, uint flags, out IntPtr pc);
    [DllImport("kernel32.dll", SetLastError = true)] static extern void ClosePseudoConsole(IntPtr pc);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attr, IntPtr value, IntPtr size, IntPtr prev, IntPtr ret);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool CreateProcessW(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);

    IntPtr pc, list;
    FileStream input;
    Thread reader;
    PROCESS_INFORMATION pi;
    readonly StringBuilder seen = new StringBuilder();
    public int Pid { get { return pi.dwProcessId; } }
    public string Seen { get { lock (seen) { return seen.ToString(); } } }

    // cmd in a pseudo console of cols x rows; what it shows collects in Seen. It starts with ctrl+c on,
    // as from a terminal: this test may run under a parent that ignores ctrl+c, which every process
    // started from here would inherit (cmd's start /b turns it back on for cmd as it goes).
    public static Con Start(string cmd, string cwd, short cols, short rows) {
        var c = new Con();
        SafeFileHandle inR, inW, outR, outW;
        if (!CreatePipe(out inR, out inW, IntPtr.Zero, 0)) throw new Exception("CreatePipe");
        if (!CreatePipe(out outR, out outW, IntPtr.Zero, 0)) throw new Exception("CreatePipe");
        COORD size; size.X = cols; size.Y = rows;
        int hr = CreatePseudoConsole(size, inR, outW, 0, out c.pc);
        if (hr != 0) throw new Exception("CreatePseudoConsole " + hr);
        inR.Dispose(); outW.Dispose();
        IntPtr bytes = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref bytes);
        c.list = Marshal.AllocHGlobal(bytes);
        if (!InitializeProcThreadAttributeList(c.list, 1, 0, ref bytes)) throw new Exception("InitializeProcThreadAttributeList");
        if (!UpdateProcThreadAttribute(c.list, 0, (IntPtr)0x00020016, c.pc, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero)) throw new Exception("UpdateProcThreadAttribute");
        var si = new STARTUPINFOEX();
        si.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
        si.StartupInfo.dwFlags = 0x100;
        si.lpAttributeList = c.list;
        SetConsoleCtrlHandler(IntPtr.Zero, false);
        if (!CreateProcessW(null, cmd, IntPtr.Zero, IntPtr.Zero, false, 0x00080000, IntPtr.Zero, cwd, ref si, out c.pi))
            throw new Exception("CreateProcess " + Marshal.GetLastWin32Error());
        c.input = new FileStream(inW, FileAccess.Write);
        c.reader = new Thread(() => {
            var stream = new FileStream(outR, FileAccess.Read);
            var buf = new byte[65536];
            while (true) {
                int n;
                try { n = stream.Read(buf, 0, buf.Length); } catch { break; }
                if (n <= 0) break;
                lock (c.seen) { c.seen.Append(Encoding.UTF8.GetString(buf, 0, n)); }
            }
        });
        c.reader.IsBackground = true;
        c.reader.Start();
        return c;
    }
    public bool WaitFor(string text, int ms) {
        var t0 = DateTime.UtcNow;
        while ((DateTime.UtcNow - t0).TotalMilliseconds < ms) { if (Seen.Contains(text)) return true; Thread.Sleep(100); }
        return false;
    }
    public void Send(string text) { var b = Encoding.UTF8.GetBytes(text); input.Write(b, 0, b.Length); input.Flush(); }
    public bool Exited(int ms) { return WaitForSingleObject(pi.hProcess, (uint)ms) == 0; }
    public uint Code() { uint code; GetExitCodeProcess(pi.hProcess, out code); return code; }
    public void Close() { ClosePseudoConsole(pc); reader.Join(5000); }
}
'@
function Stop-Tree([int]$id) {
    foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $id")) { Stop-Tree $c.ProcessId }
    Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
}
# the processes whose command line names the test's folder (the jobs' do), but for this test itself
function Ours { @(Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.IndexOf($Base, [StringComparison]::OrdinalIgnoreCase) -ge 0 }) }
function Pieces { [long](Get-ChildItem "$Base\install\cache\downloads" -Filter '*.tar.gz*' -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum }
function CtrlC($answer) {
    if ($env:MAKEPAD_LOADER_ROOT -ne "$Base\install") { throw "a run without its own MAKEPAD_LOADER_ROOT" }
    $c = [Con]::Start("cmd.exe /d /c `"$Base\home\makepad.bat`"", "$Base\home", 100, 30)
    try {
        Check "ctrl+c ($answer): Rust downloads" "$($c.WaitFor('downloading', 180000))" 'True'
        Start-Sleep -Milliseconds 700
        $curls = @(Ours | Where-Object { $_.Name -eq 'curl.exe' }).Count
        $c.Send([string][char]3)
        Check "ctrl+c ($answer): cmd asks" "$($c.WaitFor('Terminate batch job', 15000))" 'True'
        Start-Sleep 1
        $c.Send("$answer`r")
        if ($answer -eq 'N') {
            Check "ctrl+c (N): the screen says so" "$($c.WaitFor('press a key to close', 60000))" 'True'
            Check "ctrl+c (N): the download stopped" $c.Seen 'The download stopped\.'
            $c.Send("`r")
        }
        Check "ctrl+c ($answer): the script ended" "$($c.Exited(30000))" 'True'
        Start-Sleep 3
        $left = @(Ours)
        $grown = Pieces
        Start-Sleep 3
        $grown = (Pieces) - $grown
        $done = "$(Get-Content -Raw "$Base\install\cache\downloads\rust.done" -ErrorAction SilentlyContinue)".Trim()
        "ctrl+c ($answer): $curls curl.exe before, $($left.Count) processes of the folder after, $grown bytes more 3 s later, rust.done '$done'"
        foreach ($p in $left) { "  left: $($p.Name) $($p.ProcessId) $($p.CommandLine)" }
        $text = $c.Seen -replace ([string][char]27 + '\[[0-9;?]*[A-Za-z]'), ' ' -replace '\s+', ' '
        "  the screen's end: " + $text.Substring([Math]::Max(0, $text.Length - 400))
        Check "ctrl+c ($answer): curl.exe ran" "$curls" '^[1-9]'
        Check "ctrl+c ($answer): nothing left running" "$($left.Count)" '^0$'
        Check "ctrl+c ($answer): no download goes on" "$grown" '^0$'
        Check "ctrl+c ($answer): the download did not finish unseen" "rust.done: $done" '^rust\.done: (failed)?$'
    } finally {
        foreach ($p in (Ours)) { Stop-Tree $p.ProcessId }
        if (-not $c.Exited(0)) { Stop-Tree $c.Pid }
        $c.Close()
    }
}
try {
    # a test service without a folder of its own stops before it makes anything (the profile a
    # folder of the test's, in case it did not)
    $ownProfile = $env:USERPROFILE
    Remove-Item Env:MAKEPAD_LOADER_ROOT
    $env:USERPROFILE = "$Base\profile"
    try { $r0 = & cmd.exe /d /c "`"$Base\home\makepad.bat`" 2>&1" | Out-String; $code0 = $LASTEXITCODE }
    finally { $env:USERPROFILE = $ownProfile; $env:MAKEPAD_LOADER_ROOT = "$Base\install" }
    "=== run0 (exit $code0)"; $r0
    Check 'no loader root: refused' $r0 'MAKEPAD_LOADER_SERVICE is set without MAKEPAD_LOADER_ROOT'
    Check 'no loader root: nothing made' ("$code0 " + (Test-Path "$Base\profile") + ' ' + @(Get-ChildItem -Force "$Base\home").Count) '^1 False 1$'
    CtrlC 'Y'
    CtrlC 'N'
    $r1 = Run 'run1'
    Check 'first run exits 0' "$script:code" '^0$'
    Check 'progress line, the real total' $r1 'Loader build: compiling .+ [0-9]+ / 3 crates'
    Check 'built: all crates' $r1 'Loader build: compiled .+in [0-9:]+ .+ 3 crates'
    Check 'rust downloaded' $r1 'Rust 1\.98\.0: ready .+downloaded and unpacked in'
    Check 'loader started' $r1 'fixture loader: one launch record present: true'
    $service = $env:MAKEPAD_LOADER_SERVICE
    $expected = @"
title	Makepad
arg	run
arg	--quiet
arg	--cached
arg	$service/public/git/runner/makepad.git#fx1@0123456789abcdef0123456789abcdef01234567
arg	--mount
arg	apps/extra=$service/public/git/runner/extra.git#fx1
arg	makepad-runner
"@
    $expected = $expected.Replace("`r`n", "`n") + "`n"
    $record = "$Base\install\cache\loader\src\runner.launch"
    Check 'Runner record text' ([Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($record)) -ceq $expected) '^True$'
    '--- sizes (KB), first run'
    foreach ($d in 'install\rust', 'install\cache\downloads', 'install') { '{0}  {1}' -f [int]((Get-ChildItem "$Base\$d" -Recurse -Force -File | Measure-Object Length -Sum).Sum / 1024), $d }
    # Put the built loader where older installs kept it, with no source marker.
    $loader = "$Base\install\cache\loader"
    $targets = "$Base\install\target\loader"
    Check 'built loader: source commit' (Get-Content -LiteralPath "$loader\src\.built") '^0123456789abcdef0123456789abcdef01234567 '
    $srcMarker = '^\.\./\.\./\.\./cache/loader/src$'
    Check 'built loader: source marker' (Get-Content -LiteralPath "$targets\src\source") $srcMarker
    Move-Item -LiteralPath "$targets\src" -Destination "$loader\src\target"
    Remove-Item -LiteralPath "$loader\src\target\source"
    # An open handle in the tree prevents its move. Keep the loader itself free to run.
    $held = [IO.File]::Open("$loader\src\target\host\makepad-launcher.exe", 'Open', 'Read', 'None')
    try {
        [IO.File]::WriteAllText("$Base\install\apps\runner.launch", 'old record', $utf8)
        [IO.File]::WriteAllText("$Base\install\cache\runner\release", 'old release', $utf8)
        Move-Item -LiteralPath "$Base\www\public\app\runner\latest" -Destination "$Base\offline-latest"
        try { $skipped = Run 'skipped' }
        finally { Move-Item -LiteralPath "$Base\offline-latest" -Destination "$Base\www\public\app\runner\latest" }
        Check 'offline: folder record kept and old files removed' (([Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($record)) -ceq $expected) -and
            !(Test-Path "$Base\install\apps\runner.launch") -and !(Test-Path "$Base\install\cache\runner\release")) '^True$'
        Check 'skipped move: exits 0' "$script:code" '^0$'
        Check 'skipped move: notice' $skipped 'Skipped moving .+src\\target'
        Check 'skipped move: no rebuild' $skipped 'Loader build: up to date'
        Check 'skipped move: old loader started' $skipped 'fixture loader: one launch record present: true'
        $inPlace = (Test-Path "$loader\src\target\host\makepad.exe") -and !(Test-Path "$targets\src") -and !(Test-Path "$loader\src\target\source")
        Check 'skipped move: old tree in place' $inPlace '^True$'
        $entry = & "$Base\install\makepad.cmd" 2>&1 | Out-String
        Check 'skipped move: stable entry finds the old build' $entry 'fixture loader: one launch record present: true'
    } finally {
        $held.Dispose()
    }
    # Exercise an old installer stamp on the unchanged ZIP.
    $stamp = "$loader\src\.built"
    $archiveSha = (Get-Content -LiteralPath "$loader\src\.loader-sha256").Trim()
    [IO.File]::WriteAllText($stamp, ([IO.File]::ReadAllText($stamp) -replace '^\S+', $archiveSha), $utf8)
    $r2 = Run 'run2'
    Check 'reused loader: source commit replaces old archive stamp' (Get-Content -LiteralPath $stamp) '^0123456789abcdef0123456789abcdef01234567 '
    Check 'migrated loader: exits 0 after the handle closes' "$script:code" '^0$'
    Check 'migrated loader: no rebuild' $r2 'Loader build: up to date'
    $moved = !(Test-Path "$loader\src\target") -and (Test-Path "$targets\src\host\makepad.exe") -and (Test-Path "$targets\src\host\makepad-launcher.exe")
    Check 'migrated loader: build moved' $moved '^True$'
    Check 'migrated loader: source marker' (Get-Content -LiteralPath "$targets\src\source") $srcMarker
    Publish 'two' $false
    $r3 = Run 'run3'
    Check 'changed loader: rebuilt' $r3 'Loader build: compiled .+in [0-9:]+'
    Check 'changed loader: new text' $r3 'fixture loader: two'
    $hex = '0123456789abcdef0123456789abcdef01234567'
    Copy-Item -Recurse "$loader\src" "$loader\$hex"
    # The old Runner built this commit. Its new target exists but has no binary yet.
    Copy-Item -Recurse "$targets\src" "$loader\$hex\target"
    Remove-Item "$loader\$hex\target\source"
    New-Item -ItemType Directory -Path "$targets\$hex" | Out-Null
    Remove-Item "$loader\$hex\.loader-sha256"
    [IO.File]::WriteAllText("$loader\previous", "src`n", $utf8); [IO.File]::WriteAllText("$loader\current", "$hex`n", $utf8)
    $unbuilt = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
    New-Item -ItemType Directory -Path "$loader\$unbuilt" | Out-Null
    Check-PendingEntry 'old target'
    $r5 = Run 'run5'
    Check 'current folder: built in place' $r5 'Loader build: compiled .+in [0-9:]+'
    Check 'current folder: stamp written there' (Test-Path "$loader\$hex\.loader-sha256") 'True'
    Check 'current folder: built in the existing new target' (Test-Path "$targets\$hex\host\makepad.exe" -PathType Leaf) 'True'
    Check 'current folder: source marker' (Get-Content -LiteralPath "$targets\$hex\source") "^\.\./\.\./\.\./cache/loader/$hex$"
    Check-PendingEntry 'shared target'
    Remove-Item -LiteralPath "$loader\$unbuilt"
    # With no release answer, only recovery can restore the active folder's record.
    $trialRecord = "$loader\$hex\runner.launch"
    Copy-Item -LiteralPath $trialRecord -Destination "$trialRecord.previous"
    [IO.File]::WriteAllText($trialRecord, "unconfirmed trial`n", $utf8)
    [IO.File]::WriteAllText("$loader\previous", "$hex`n", $utf8)
    [IO.File]::WriteAllText("$loader\pending", "$hex`n", $utf8)
    Move-Item -LiteralPath "$Base\www\public\app\runner\latest" -Destination "$Base\offline-latest"
    try { $codeRollback = Run 'code-rollback' }
    finally { Move-Item -LiteralPath "$Base\offline-latest" -Destination "$Base\www\public\app\runner\latest" }
    Check 'code rollback: selected folder runs' $codeRollback "(?m)^fixture loader folder: $hex\r?$"
    # nothing tries this switch: recovery settles it at once, with no waiting line
    Check 'code rollback: no wait with no open Makepad' (-not (($codeRollback | Out-String) -match 'Waiting for the update')) '^True$'
    Check 'code rollback: recovery note' ([IO.File]::ReadAllText("$loader\rolled-back") -ceq 'The update did not confirm its first frame.') '^True$'
    Check 'code rollback: exits 0' "$script:code" '^0$'
    Check 'code rollback: old record restored on the same loader' (([IO.File]::ReadAllText($trialRecord) -ceq $expected) -and
        ((Get-Content -LiteralPath "$loader\current") -ceq $hex) -and !(Test-Path "$trialRecord.previous") -and
        !(Test-Path "$loader\pending") -and (Get-Content -Raw -LiteralPath "$loader\rolled-back")) '^True$'
    Remove-Item -LiteralPath "$loader\rolled-back"
    [IO.File]::WriteAllText("$loader\previous", "src`n", $utf8)
    [IO.File]::WriteAllText("$loader\pending", "$hex`n", $utf8)
    $r6 = Run 'run6'
    Check 'rollback: recovery note' ([IO.File]::ReadAllText("$loader\rolled-back") -ceq 'The update did not confirm its first frame.') '^True$'
    Check 'rollback: src binary runs' $r6 'fixture loader: two'
    Check 'rollback: selected folder' $r6 '(?m)^fixture loader folder: src\r?$'
    Check 'rollback: pointers' ("" + (-not (Test-Path "$loader\pending")) + (-not (Test-Path "$loader\current")) + (Test-Path "$loader\rolled-back")) 'TrueTrueTrue'
    # The coding-agent form: the one-line PowerShell entry, as from the site, fetches makepad.bat in
    # its ZIP and runs `makepad.bat open fab`, which hands over to the loader; the stable entry
    # makepad.cmd runs it again; a mistyped id stops before setup.
    $line = "& ([scriptblock]::Create((irm http://127.0.0.1:$port/runner/windows.ps1))) open fab"
    $o1 = & powershell -NoProfile -c $line 2>&1 | Out-String; $codeO1 = $LASTEXITCODE
    "=== open1 (exit $codeO1)"; $o1
    Check 'open: exits 0' "$codeO1" '^0$'
    Check 'open: makepad.bat fetched' (Test-Path "$Base\install\makepad.bat") 'True'
    Check 'open: hands over' $o1 'Handing over to makepad open fab\.'
    Check 'open: the loader gets open fab' $o1 'fixture loader: two launch record present: true args: \["open", "fab"\]'
    $o2 = & cmd.exe /d /c "`"$Base\install\makepad.cmd`" open fab 2>&1" | Out-String
    "=== open2 (exit $LASTEXITCODE)"; $o2
    Check 'open: the stable entry runs the loader' $o2 'fixture loader: two launch record present: true args: \["open", "fab"\]'
    $o3 = & cmd.exe /d /c "`"$Base\home\makepad.bat`" open fab.x 2>&1" | Out-String; $codeO3 = $LASTEXITCODE
    "=== open3 (exit $codeO3)"; $o3
    Check 'open: a bad id refused' "$codeO3 $o3" '^1 .*Invalid app id'
    Publish 'three' $true
    $r4 = Run 'run4'
    Check 'broken loader: nonzero exit' "$script:code" '^[1-9]'
    Check 'broken loader: error line' $r4 'Error: The loader did not compile'
    Check 'broken loader: log path' $r4 'Full log: .*build\.log'
    Check 'broken loader: error text' $r4 'error'
} finally {
    Stop-Job $server; Remove-Job $server -Force
}
"failures: $script:failures"
exit $script:failures
