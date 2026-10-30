//! What one machine adds to the registry's launch of an app. The launch definition is the
//! registry's (`AppLaunch`: one per app, the same for the WM's dock, devhub's launches and the
//! shell's `makepad run`); a machine only adds settings the person needs THERE, from files:
//!
//! - `<makepad home>/apps/<id>.conf`: this machine's settings for one app, `key = value` lines
//!   (`#` comments): `lan = on|off` (the app's local-network switch, `MAKEPAD_LAN`), and
//!   `env NAME=value` for a setting the app reads from its environment (a printer conf name).
//! - `<makepad home>/wm/jit.env`: `NAME=value` lines every JIT run on this machine gets (how a
//!   node debugs its JIT apps: `MAKEPAD_RUN_VERBOSE=1`, an on-disk `TMPDIR`).
//!
//! What a launcher's own environment carries from where it was started (a lane's target dir, a
//! test's hidden or LAN-off switches, a lane's test badge) never reaches a registry launch:
//! [`crate::child_env::scrub_lane`]. So what the person sees after a launch is the same app,
//! the same version and the same configuration however it was started.

use std::path::PathBuf;

/// What devhub's sync writes into a machine's synced checkout after it moved the files (the
/// commits it holds): the mark of a checkout only landed work reaches.
pub const FLEET_REVISION: &str = "FLEET-REVISION.txt";

/// `MAKEPAD_HOME`, else `~/.makepad` (`%USERPROFILE%\.makepad`). An empty `MAKEPAD_HOME` counts
/// as unset.
pub fn makepad_home() -> PathBuf {
    if let Some(home) = std::env::var_os("MAKEPAD_HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default().join(".makepad")
}

/// This machine's settings file for app `id`.
pub fn app_conf_path(id: &str) -> PathBuf {
    makepad_home().join("apps").join(format!("{id}.conf"))
}

/// The machine-wide environment of JIT runs.
pub fn jit_env_path() -> PathBuf {
    makepad_home().join("wm").join("jit.env")
}

/// A name a machine file may set: an upper-case variable that changes neither how code is
/// built or loaded nor whether the instance is a hidden or test one.
pub fn settable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
        && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !name.starts_with("LD_")
        && !name.starts_with("CARGO")
        && !name.starts_with("RUST")
        && !name.starts_with("DYLD_")
        && !matches!(name, "PATH" | "HOME" | "SHELL" | "MAKEPAD" | "MAKEPAD_HIDE_WINDOWS" | "MAKEPAD_TEST_LANE" | "XDG_RUNTIME_DIR")
}

/// `NAME=value` lines (`#` comments, blank lines), names checked by [`settable`] (jit.env may
/// also set `TMPDIR`).
pub fn parse_env_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| settable(k))
        .collect()
}

/// One app's settings file as the environment it gives the app.
pub fn parse_app_conf(text: &str) -> Vec<(String, String)> {
    let mut env = Vec::new();
    for line in text.lines().map(|l| l.split('#').next().unwrap_or("").trim()) {
        if let Some(rest) = line.strip_prefix("env ") {
            env.extend(parse_env_lines(rest));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match (key.trim(), value.trim()) {
            ("lan", "on") => env.push(("MAKEPAD_LAN".to_string(), "1".to_string())),
            ("lan", "off") => env.push(("MAKEPAD_LAN".to_string(), "0".to_string())),
            _ => {}
        }
    }
    env
}

/// What this machine adds to a launch of app `id`: jit.env for a JIT run, then the app's own
/// settings file (which wins).
pub fn launch_env(id: &str, jit: bool) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if jit {
        for (key, value) in parse_env_lines(&std::fs::read_to_string(jit_env_path()).unwrap_or_default()) {
            // an on-disk TMPDIR for the session transcript: there before the run writes to it
            if key == "TMPDIR" {
                let _ = std::fs::create_dir_all(&value);
            }
            env.push((key, value));
        }
    }
    env.extend(parse_app_conf(&std::fs::read_to_string(app_conf_path(id)).unwrap_or_default()));
    env
}

/// This session's window manager socket (Linux): `MAKEPAD_WM_SOCKET` (what the WM gives every
/// process it starts), else the visible WM's `$XDG_RUNTIME_DIR/makepad-wm-0` (a hidden WM never
/// takes index 0). The protocol is `makepad_hal::wm_socket`'s.
#[cfg(unix)]
pub fn wm_socket() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MAKEPAD_WM_SOCKET").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    let path = PathBuf::from(dir).join("makepad-wm-0");
    path.exists().then_some(path)
}

/// Asks the WM at `path` to open its dock app `id` (`makepad-wm launch <id>`): launch-or-focus,
/// the registry's one launch definition. Ok once the WM took the request.
#[cfg(unix)]
pub fn request_launch(path: &std::path::Path, id: &str) -> Result<(), String> {
    use std::io::{BufRead, Write};
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) || id.is_empty() {
        return Err(format!("not an app id: {id:?}"));
    }
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    (&stream).write_all(format!("makepad-wm launch {id}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&stream).read_line(&mut line).map_err(|e| format!("the WM did not answer: {e}"))?;
    match line.trim() {
        "makepad-wm launched" => Ok(()),
        other => Err(other.strip_prefix("makepad-wm refused ").map_or_else(|| format!("unexpected answer {other:?}"), str::to_string)),
    }
}

/// Unsaved work of the apps running on this machine (Unix), as `(pid, app, what)`: what each
/// app's platform publishes in `$XDG_RUNTIME_DIR/makepad-unsaved/<pid>`
/// (`makepad_platform::restart::set_unsaved`), for live processes only. A whole-session restart
/// (the WM's) waits while this is not empty.
#[cfg(unix)]
pub fn unsaved_work() -> Vec<(u32, String, String)> {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(PathBuf::from(dir).join("makepad-unsaved")) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let app = cmdline.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned())
            .find(|a| find_app_name(a).is_some()).and_then(|a| find_app_name(&a)).unwrap_or_else(|| format!("pid {pid}"));
        for what in std::fs::read_to_string(entry.path()).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()) {
            out.push((pid, app.clone(), what.trim().to_string()));
        }
    }
    out.sort();
    out
}

/// The registry label of a package or binary named by a command-line word (a path's file name).
#[cfg(unix)]
fn find_app_name(word: &str) -> Option<String> {
    let name = word.rsplit('/').next().unwrap_or(word);
    crate::apps().iter().find(|a| a.package == name || a.bin == name).map(|a| a.label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_conf_gives_lan_and_named_settings_only() {
        let env = parse_app_conf("# 104's Fab\nlan = on\nenv FAB_PRINTER_CONF=h2d\nenv CARGO_TARGET_DIR=/x\nenv MAKEPAD_TEST_LANE=x\nnonsense\n");
        assert_eq!(env, vec![("MAKEPAD_LAN".to_string(), "1".to_string()), ("FAB_PRINTER_CONF".to_string(), "h2d".to_string())]);
        assert_eq!(parse_app_conf("lan = off"), vec![("MAKEPAD_LAN".to_string(), "0".to_string())]);
    }

    #[test]
    fn jit_env_lines_skip_comments_and_build_inputs() {
        let env = parse_env_lines("# debug\nMAKEPAD_RUN_VERBOSE=1\nTMPDIR = /data/tmp\nRUSTFLAGS=-Cx\nlower=1\n");
        assert_eq!(env, vec![("MAKEPAD_RUN_VERBOSE".to_string(), "1".to_string()), ("TMPDIR".to_string(), "/data/tmp".to_string())]);
    }
}
