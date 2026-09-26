//! The Claude Desktop extension for an app: `<app-id>.mcpb`.
//!
//! An MCP Bundle is a zip with a `manifest.json` (manifest_version 0.3).
//! Ours is a `binary` server whose command is the app's own executable,
//! by absolute path, with `--mcp`: nothing is copied into the bundle, so
//! the installed extension always starts the app as it is on disk now.
//! The file is handed to Claude Desktop itself ([`open_in_claude_desktop`]),
//! which shows its install dialog.

use super::server::ToolDef;
use makepad_strict_json::{self as json, Value};
use std::path::{Path, PathBuf};

/// The bundle manifest's format version.
pub const MANIFEST_VERSION: &str = "0.3";

/// The platform name the manifest's `compatibility.platforms` uses.
fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

/// The bundle's `manifest.json`.
pub fn manifest(app_id: &str, title: &str, exe: &Path, tools: &[ToolDef]) -> String {
    let exe = exe.to_string_lossy().into_owned();
    json::obj(vec![
        ("manifest_version", json::s(MANIFEST_VERSION)),
        ("name", json::s(format!("makepad-{}", app_id.to_ascii_lowercase()))),
        ("display_name", json::s(title)),
        ("version", json::s(env!("CARGO_PKG_VERSION"))),
        (
            "description",
            json::s(format!(
                "Lets Claude use {title}'s tools while you work in it. Every call shows in the app's AI panel (F10), and calls that change things outside the app wait for your confirm there."
            )),
        ),
        ("author", json::obj(vec![("name", json::s("Makepad"))])),
        (
            "server",
            json::obj(vec![
                ("type", json::s("binary")),
                ("entry_point", json::s(exe.clone())),
                (
                    "mcp_config",
                    json::obj(vec![
                        ("command", json::s(exe)),
                        ("args", Value::Arr(vec![json::s("--mcp")])),
                        ("env", json::obj(vec![])),
                    ]),
                ),
            ]),
        ),
        (
            "tools",
            Value::Arr(
                tools
                    .iter()
                    .map(|t| json::obj(vec![("name", json::s(&t.name)), ("description", json::s(&t.description))]))
                    .collect(),
            ),
        ),
        ("compatibility", json::obj(vec![("platforms", Value::Arr(vec![json::s(platform())]))])),
    ])
    .to_json()
}

/// Write `<dir>/<app-id>.mcpb` and return its path.
pub fn write(dir: &Path, app_id: &str, title: &str, exe: &Path, tools: &[ToolDef]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{app_id}.mcpb"));
    let manifest = manifest(app_id, title, exe, tools);
    std::fs::write(&path, stored_zip(&[("manifest.json", manifest.as_bytes())]))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// What the person reads when there is no Claude Desktop to open it in.
pub const NOT_INSTALLED: &str = "Claude Desktop isn't installed";

/// Open the bundle in Claude Desktop itself, which shows its install
/// dialog. Windows associates no program with `.mcpb`, but Claude Desktop
/// takes the path as an argument, running or not.
pub fn open_in_claude_desktop(bundle: &Path) -> Result<(), String> {
    let mut command = claude_desktop_command()?;
    command.arg(bundle);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| format!("could not start Claude Desktop: {e}"))?;
    // It exits at once or runs on as Claude itself; reap it off the UI thread.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Is Claude Desktop installed here? Only looks where its installers put
/// it (no process listing), so a panel can ask while it comes up. There
/// is no Claude Desktop for Linux.
pub fn claude_desktop_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        std::iter::once(PathBuf::from("/Applications/Claude.app"))
            .chain(home.map(|h| h.join("Applications").join("Claude.app")))
            .any(|app| app.is_dir())
    }
    #[cfg(target_os = "windows")]
    {
        let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return false };
        std::fs::symlink_metadata(local.join("Microsoft").join("WindowsApps").join("claude-desktop.exe")).is_ok()
            || local.join("AnthropicClaude").join("claude.exe").is_file()
            || local.join("Programs").join("Claude").join("Claude.exe").is_file()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// The command that hands a file to Claude Desktop on this machine.
#[cfg(target_os = "windows")]
fn claude_desktop_command() -> Result<std::process::Command, String> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).ok_or(NOT_INSTALLED)?;
    // The Store package's App Execution Alias: a reparse point, so it is
    // looked at without following it.
    let alias = local.join("Microsoft").join("WindowsApps").join("claude-desktop.exe");
    if std::fs::symlink_metadata(&alias).is_ok() {
        return Ok(std::process::Command::new(alias));
    }
    let installed = running_claude_exe()
        .into_iter()
        .chain([
            local.join("AnthropicClaude").join("claude.exe"),
            local.join("Programs").join("Claude").join("Claude.exe"),
        ])
        .find(|exe| exe.is_file())
        .ok_or(NOT_INSTALLED)?;
    Ok(std::process::Command::new(installed))
}

/// The executable of a running Claude Desktop (the installer's copy lives
/// in a versioned folder). Claude Code's CLI is also `claude.exe`, under a
/// `claude-code` folder, and is not it.
#[cfg(target_os = "windows")]
fn running_claude_exe() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Process -Name Claude -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -notlike '*\\claude-code\\*' } | Select-Object -First 1 -ExpandProperty Path",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(target_os = "macos")]
fn claude_desktop_command() -> Result<std::process::Command, String> {
    if !claude_desktop_installed() {
        return Err(NOT_INSTALLED.into());
    }
    let mut command = std::process::Command::new("open");
    command.args(["-a", "Claude"]);
    Ok(command)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn claude_desktop_command() -> Result<std::process::Command, String> {
    Ok(std::process::Command::new("xdg-open"))
}

/// A zip of uncompressed ("stored") entries.
fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Its central directory record.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra length
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    // End of central directory.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// CRC-32 (IEEE), bit by bit: a manifest is a few kilobytes.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
