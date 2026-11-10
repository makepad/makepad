//! What one machine adds to the registry's launch of an app. The launch definition is the
//! registry's (`AppLaunch`: one per app, the same for the WM's dock, devhub's launches and the
//! shell's `makepad run`); a machine only adds settings the person needs THERE, from files:
//!
//! - `<makepad home>/apps/<id>.conf`: this machine's settings for one app, `key = value` lines
//!   (`#` comments): `lan = on|off` (the app's local-network switch, `MAKEPAD_LAN`), and
//!   `env NAME=value` for a setting the app reads from its environment (a printer conf name).
//! - `<makepad home>/wm/jit.env`: `NAME=value` lines every JIT run on this machine gets (how a
//!   node debugs its JIT apps: `MAKEPAD_RUN_VERBOSE=1`, an on-disk `TMPDIR`).
//! - `<makepad home>/apps/local.json`: this machine's own apps, rows of the shared table's shape
//!   (`apps.json`) for checkouts of other repositories it runs (table.rs). The WM's dock and
//!   menus list them; `<makepad home>/wm/apps.splash` picks how each runs, as for any app.
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

/// This machine's own apps: rows of the shared table's shape (table.rs).
pub fn local_apps_path() -> PathBuf {
    makepad_home().join("apps").join("local.json")
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
        && !test_hook(name)
}

/// A test's switch (`MAKEPAD_RUN_TEST_COMPILE_PANIC`, `MAKEPAD_TRAY_SELFTEST`, ...): a name with a
/// `TEST` word. A test sets it on the one process it runs; a machine file never does, or every
/// later launch on the machine runs the test (a forced compile panic in the dock's app).
pub fn test_hook(name: &str) -> bool {
    name.split('_').any(|word| word.ends_with("TEST"))
}

/// `NAME=value` lines (`#` comments, blank lines), names checked by [`settable`] (jit.env may
/// also set `TMPDIR`).
pub fn parse_env_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| {
            let ok = settable(k);
            if !ok { eprintln!("makepad: {k} is not a machine setting (a test switch, or a name that changes how code loads): ignored"); }
            ok
        })
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
/// the registry's one launch definition. `native`: its compiled build, whatever runs it otherwise.
/// Ok once the WM took the request.
#[cfg(unix)]
pub fn request_launch(path: &std::path::Path, id: &str, native: bool) -> Result<(), String> {
    use std::io::{BufRead, Write};
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) || id.is_empty() {
        return Err(format!("not an app id: {id:?}"));
    }
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let how = if native { " native" } else { "" };
    (&stream).write_all(format!("makepad-wm launch {id}{how}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&stream).read_line(&mut line).map_err(|e| format!("the WM did not answer: {e}"))?;
    match line.trim() {
        "makepad-wm launched" => Ok(()),
        // the id is checked above: only a WM from before `native` reads the two words as a bad id
        "makepad-wm refused not an app id" if native => {
            let package = crate::find(id).map_or(id, |a| a.package);
            Err(format!("this WM is older than native launches: it takes them after its next restart. Until then, a screen lease that takes the test environment off: `devhub run <lane> screen --node <node> --native <why> -- env -u MAKEPAD_HIDE_WINDOWS -u MAKEPAD_TEST_LANE -u SANDBOX_MUTE -u MAKEPAD_LAN cargo run --release -p {package}`, which queues while the person is at the box (`devhub node <node> back` lets it through)"))
        }
        other => Err(other.strip_prefix("makepad-wm refused ").map_or_else(|| format!("unexpected answer {other:?}"), str::to_string)),
    }
}

/// Asks the WM at `path` for a toast (`makepad-wm notify <title>\t<body>[\t<action>]`): the WM
/// draws it in its notification stack, no OS popup. `action`: what clicking it does
/// (`relaunch=<pid>`: that JIT's `session relaunch`, an offered restart). Ok once the WM took it.
#[cfg(unix)]
pub fn request_notify(path: &std::path::Path, title: &str, body: &str, action: Option<&str>) -> Result<(), String> {
    use std::io::{BufRead, Write};
    let flat = |s: &str| -> String { s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string() };
    // the WM reads one line of at most 512 bytes
    let cut = |s: String, max: usize| -> String {
        if s.len() <= max {
            return s;
        }
        let mut end = max.saturating_sub(3);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    };
    let title = cut(flat(title), 96);
    let action = action.map(flat).unwrap_or_default();
    let body = cut(flat(body), 470usize.saturating_sub(title.len() + action.len()));
    if title.is_empty() {
        return Err("a toast needs a title".into());
    }
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let action = if action.is_empty() { String::new() } else { format!("\t{action}") };
    (&stream).write_all(format!("makepad-wm notify {title}\t{body}{action}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&stream).read_line(&mut line).map_err(|e| format!("the WM did not answer: {e}"))?;
    match line.trim() {
        "makepad-wm notified" => Ok(()),
        other => Err(other.strip_prefix("makepad-wm refused ").map_or_else(|| format!("unexpected answer {other:?}"), str::to_string)),
    }
}

/// Asks the WM at `path` how long ago a person used this machine (`makepad-wm activity`): its
/// answer's JSON (`input_ms`, `seq`, `origin`, `what`; `makepad_hal::wm_socket`).
#[cfg(unix)]
pub fn request_activity(path: &std::path::Path) -> Result<String, String> {
    use std::io::{BufRead, Write};
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    (&stream).write_all(b"makepad-wm activity\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&stream).read_line(&mut line).map_err(|e| format!("the WM did not answer: {e}"))?;
    line.trim().strip_prefix("makepad-wm activity ").map(str::to_string).ok_or_else(|| format!("unexpected answer {:?} (a WM from before `activity`)", line.trim()))
}

/// Unsaved work of the apps running on this machine (Unix), as `(pid, app, what)`: what each
/// app's platform publishes in `$XDG_RUNTIME_DIR/makepad-unsaved/<pid>`
/// (`makepad_platform::restart::set_unsaved`), for live processes only. A whole-session restart
/// (the WM's) waits while this is not empty.
#[cfg(unix)]
pub fn unsaved_work() -> Vec<(u32, String, String)> {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else { return Vec::new() };
    unsaved_in(&PathBuf::from(dir).join("makepad-unsaved"))
}

/// The records of `dir` ([`unsaved_work`]) that their pid's process wrote: one written before
/// that process started is a dead app's, whose pid came back. An app killed with unsaved work
/// leaves its record (the runtime dir lives until the session ends): a Fab's from a week before
/// named a pid that one of the WM's threads had taken since, and held a node's WM restart on
/// "code being typed" with no Fab open and no input on the box (11-08).
#[cfg(unix)]
fn unsaved_in(dir: &std::path::Path) -> Vec<(u32, String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Some(written) = entry.metadata().and_then(|m| m.modified()).ok() else { continue };
        if started(pid).is_none_or(|start| written < start) { continue }
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

/// When process `pid` started: the boot time (`btime` of /proc/stat, whole seconds, so at most
/// a second early) plus its start in clock ticks after boot (field 22 of /proc/<pid>/stat). None:
/// no such process, or no /proc (macOS).
#[cfg(unix)]
fn started(pid: u32) -> Option<std::time::SystemTime> {
    // /proc's clock ticks: Linux's USER_HZ, 100 on every architecture it exports
    // (`sysconf(_SC_CLK_TCK)` answers it)
    const TICKS_PER_S: u64 = 100;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // the fields after the command name, which is in parentheses and may hold anything: from
    // the state (field 3) on
    let ticks: u64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let boot: u64 = std::fs::read_to_string("/proc/stat").ok()?.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse().ok()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(boot) + std::time::Duration::from_millis(ticks * 1000 / TICKS_PER_S))
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

    #[cfg(unix)]
    #[test]
    fn a_native_launch_asks_for_it_and_an_older_wm_says_why() {
        use std::io::{BufRead, Write};
        let dir = std::env::temp_dir().join(format!("makepad-request-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wm");
        let socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let wm = std::thread::spawn(move || {
            let mut heard = Vec::new();
            for answer in ["makepad-wm launched", "makepad-wm launched", "makepad-wm refused not an app id"] {
                let (stream, _) = socket.accept().unwrap();
                let mut line = String::new();
                std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
                heard.push(line);
                (&stream).write_all(format!("{answer}\n").as_bytes()).unwrap();
            }
            heard
        });
        assert_eq!(request_launch(&path, "fab", false), Ok(()));
        assert_eq!(request_launch(&path, "fab", true), Ok(()));
        let older = request_launch(&path, "fab", true).unwrap_err();
        assert!(older.contains("next restart") && older.contains("cargo run --release -p makepad-fab"), "{older}");
        assert_eq!(wm.join().unwrap(), ["makepad-wm launch fab\n", "makepad-wm launch fab native\n", "makepad-wm launch fab native\n"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_app_conf_gives_lan_and_named_settings_only() {
        let env = parse_app_conf("# a node's app\nlan = on\nenv APP_SETTING=x\nenv CARGO_TARGET_DIR=/x\nenv MAKEPAD_TEST_LANE=x\nnonsense\n");
        assert_eq!(env, vec![("MAKEPAD_LAN".to_string(), "1".to_string()), ("APP_SETTING".to_string(), "x".to_string())]);
        assert_eq!(parse_app_conf("lan = off"), vec![("MAKEPAD_LAN".to_string(), "0".to_string())]);
    }

    /// A record its pid's process wrote is unsaved work; the same record written before that
    /// process started is a dead app's whose pid came back, and one whose pid no process has is
    /// nobody's.
    #[cfg(target_os = "linux")]
    #[test]
    fn unsaved_work_is_what_the_process_with_that_pid_wrote() {
        let me = std::process::id();
        let dir = std::env::temp_dir().join(format!("makepad-unsaved-test-{me}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(me.to_string()), "code being typed\n").unwrap();
        std::fs::write(dir.join((u32::MAX - 1).to_string()), "code being typed\n").unwrap();
        let found = |dir: &std::path::Path| unsaved_in(dir).into_iter().map(|(pid, _, what)| (pid, what)).collect::<Vec<_>>();
        assert_eq!(found(&dir), vec![(me, "code being typed".to_string())]);
        let before = started(me).unwrap() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(dir.join(me.to_string())).unwrap().set_modified(before).unwrap();
        assert_eq!(found(&dir), Vec::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn jit_env_lines_skip_comments_and_build_inputs() {
        let env = parse_env_lines("# debug\nMAKEPAD_RUN_VERBOSE=1\nTMPDIR = /data/tmp\nRUSTFLAGS=-Cx\nlower=1\n");
        assert_eq!(env, vec![("MAKEPAD_RUN_VERBOSE".to_string(), "1".to_string()), ("TMPDIR".to_string(), "/data/tmp".to_string())]);
    }
}
