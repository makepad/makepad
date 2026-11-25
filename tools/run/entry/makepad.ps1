# Makepad for Windows. Read it before you run it. This entry script is one file.
# powershell -NoProfile -c "& ([scriptblock]::Create((irm https://makepad.nl/runner/windows.ps1))) open fab"
# It downloads only makepad.bat (the ZIP the site serves at /runner/windows, which holds just that
# file) over HTTPS from the release service, then runs it.
# makepad.bat checks the SHA-256 of the Rust archives and loader source it downloads.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($PSVersionTable.PSVersion.Major -eq 5) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

$service = $env:MAKEPAD_LOADER_SERVICE
$root = $env:MAKEPAD_LOADER_ROOT
if ($service -and -not $root) {
    [Console]::Error.WriteLine('  MAKEPAD_LOADER_SERVICE is set without MAKEPAD_LOADER_ROOT: give the run a folder of its own.')
    exit 1
}
if (-not $service) { $service = 'https://makepad.nl' }
if (-not $root) { $root = Join-Path $env:USERPROFILE 'makepad-runner' }
# makepad.bat names the folder as it starts
$root = (New-Item -ItemType Directory -Path $root -Force).FullName

$bat = Join-Path $root 'makepad.bat'
$part = "$bat.part"
$zip = "$bat.zip.part"
$url = $service.TrimEnd('/') + '/runner/windows'
try {
    # Like makepad.bat, allow HTTP only for a local test service.
    if ($url -notmatch '^https://' -and $url -notmatch '^http://(127\.0\.0\.1|localhost):[0-9]+/') {
        throw 'The service requires HTTPS.'
    }
    Invoke-WebRequest -UseBasicParsing -Uri $url -MaximumRedirection 0 -OutFile $zip
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try {
        $entry = $archive.GetEntry('makepad.bat')
        if (-not $entry) { throw 'Expected makepad.bat in the download.' }
        $reader = [IO.StreamReader]::new($entry.Open())
        try { $body = $reader.ReadToEnd() } finally { $reader.Dispose() }
    } finally { $archive.Dispose() }
    Remove-Item -LiteralPath $zip -Force
    if ($body -cnotmatch '\A@echo off(\r|\n|\z)') { throw 'Expected makepad.bat.' }
    $body = $body -replace '\r\n?|\n', "`r`n"
    [IO.File]::WriteAllText($part, $body, [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $part -Destination $bat -Force
} catch {
    Remove-Item -LiteralPath $part, $zip -Force -ErrorAction SilentlyContinue
    [Console]::Error.WriteLine("  Could not download or save makepad.bat from $url.")
    exit 1
}

$quotedArgs = foreach ($arg in $args) { '"' + ([string]$arg).Replace('"', '""') + '"' }
# cmd expands the command once, so percent signs in paths and arguments stay literal.
$env:MAKEPAD_ENTRY_COMMAND = "`"$bat`" " + ($quotedArgs -join ' ')
& cmd.exe /d /v:off /c '%MAKEPAD_ENTRY_COMMAND%'
exit $LASTEXITCODE
