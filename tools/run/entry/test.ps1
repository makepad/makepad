# First-run test of makepad.bat in a fresh folder against a local release service serving tiny
# fixture loaders (the same fixture as test.sh): ctrl+c while Rust downloads (in a pseudo console),
# first run, unchanged run (no rebuild), changed loader (rebuild), broken loader (error, log excerpt
# and path).
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
    New-Item -ItemType Directory -Force -Path "$zipdir\fx1\dep-a", "$zipdir\fx1\tools\run\entry" | Out-Null
    $files = @{
        'fx1/Cargo.toml' = "[package]`nname = `"makepad-run`"`nversion = `"0.1.0`"`nedition = `"2021`"`n[dependencies]`ndep-a = { path = `"dep-a`" }`n[[bin]]`nname = `"makepad`"`npath = `"main.rs`"`n[[bin]]`nname = `"makepad-launcher`"`npath = `"launcher.rs`"`n[profile.host]`ninherits = `"release`"`n"
        'fx1/Cargo.lock' = "version = 4`n`n[[package]]`nname = `"dep-a`"`nversion = `"0.1.0`"`n`n[[package]]`nname = `"makepad-run`"`nversion = `"0.1.0`"`ndependencies = [`n `"dep-a`",`n]`n"
        'fx1/dep-a/Cargo.toml' = "[package]`nname = `"dep-a`"`nversion = `"0.1.0`"`nedition = `"2021`"`n[lib]`npath = `"lib.rs`"`n"
        'fx1/dep-a/lib.rs' = "pub fn text() -> &'static str { `"$text`" }`n"
        'fx1/launcher.rs' = "fn main() {}`n"
        # makepad.bat compiles the assembler rust-mingw's dlltool needs from the loader's source
        'fx1/tools/run/entry/assemble.rs' = [IO.File]::ReadAllText("$here\assemble.rs")
        'fx1/main.rs' = $(if ($broken) { "fn main() { this is not rust }`n" } else { "fn main() {`n    let home = std::env::var(`"RELAX_HOME`").unwrap_or_default();`n    let record = std::path::Path::new(&home).join(`"apps/runner.launch`");`n    println!(`"fixture loader: {} launch record present: {}`", dep_a::text(), record.is_file());`n}`n" })
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
    # every run here has a folder of its own, never the profile's
    if ($env:MAKEPAD_LOADER_ROOT -ne "$Base\install") { throw "a run without its own MAKEPAD_LOADER_ROOT" }
    $out = & cmd.exe /d /c "`"$Base\home\makepad.bat`" 2>&1" | Out-String
    $script:code = $LASTEXITCODE
    "=== $name (exit $script:code)"; $out
    return $out
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
    Check 'progress line, the real total' $r1 'Loader build: compiling .+ [0-9]+ / 2 crates'
    Check 'built: all crates' $r1 'Loader build: compiled .+in [0-9:]+ .+ 2 crates'
    Check 'rust downloaded' $r1 'Rust 1\.98\.0: ready .+downloaded and unpacked in'
    Check 'loader started' $r1 'fixture loader: one launch record present: true'
    '--- sizes (KB), first run'
    foreach ($d in 'install\rust', 'install\cache\downloads', 'install') { '{0}  {1}' -f [int]((Get-ChildItem "$Base\$d" -Recurse -Force -File | Measure-Object Length -Sum).Sum / 1024), $d }
    $r2 = Run 'run2'
    Check 'unchanged loader: no rebuild' $r2 'Loader build: up to date'
    Publish 'two' $false
    $r3 = Run 'run3'
    Check 'changed loader: rebuilt' $r3 'Loader build: compiled .+in [0-9:]+'
    Check 'changed loader: new text' $r3 'fixture loader: two'
    $loader = "$Base\install\cache\loader"
    $hex = '0123456789abcdef0123456789abcdef01234567'
    Copy-Item -Recurse "$loader\src" "$loader\$hex"
    Remove-Item "$loader\$hex\.loader-sha256"
    [IO.File]::WriteAllText("$loader\previous", "src`n", $utf8); [IO.File]::WriteAllText("$loader\current", "$hex`n", $utf8)
    $r5 = Run 'run5'
    Check 'current folder: built in place' $r5 'Loader build: compiled .+in [0-9:]+'
    Check 'current folder: stamp written there' (Test-Path "$loader\$hex\.loader-sha256") 'True'
    [IO.File]::WriteAllText("$loader\pending", "$hex`n", $utf8)
    $r6 = Run 'run6'
    Check 'rollback: line' $r6 'Went back to src: the new runner did not confirm'
    Check 'rollback: src binary runs' $r6 'fixture loader: two'
    Check 'rollback: pointers' ("" + (-not (Test-Path "$loader\pending")) + (-not (Test-Path "$loader\current")) + (Test-Path "$loader\rolled-back")) 'TrueTrueTrue'
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
