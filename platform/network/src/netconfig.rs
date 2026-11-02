//! User network settings in `<MAKEPAD_HOME or ~/.makepad>/network.conf`.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

pub const NETWORKED_AI_HUB_KEY: &str = "networked_ai_hub";
pub const AI_HUB_ROLE_KEY: &str = "ai_hub_role";
pub const NODE_CONF_ENV: &str = "MAKEPAD_AI_HUB_NODE_CONF";

const HEADER: &str = "# Networked AI hub lets user-machine apps find and use AI hubs on the local network\n\
# and lets a user machine's hub serve peers on that network.\n\
# Default: off, to avoid the macOS local-network permission prompt.\n\
# Apps read this setting when they connect. The runner's Settings toggles it.\n\
# networked_ai_hub = off|on\n\
# Fleet nodes serve peers regardless of this toggle: a Node fleet credential,\n\
# MAKEPAD_AI_HUB_NODE_CONF, or ai_hub_role = server identifies a fleet server.\n\
# ai_hub_role = server\n\
# MAKEPAD_AI_HUB_ROLE=server|user overrides the role for this process.\n\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubRole {
    UserMachine,
    FleetServer,
}

#[derive(Clone, Copy, Default)]
struct Settings {
    on: bool,
    server: bool,
}

struct Cached {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
    settings: Settings,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Read the current setting. Missing, unreadable or invalid configuration means off.
/// Unchanged files cost one stat; no file I/O is performed while holding the cache lock.
///
/// `MAKEPAD_NETWORKED_AI_HUB=1` / `=0` overrides the file for this process (a test machine or a
/// test that needs a LAN hub sets it, as `MAKEPAD_LAN` does).
pub fn networked_ai_hub() -> bool {
    networked_from(std::env::var("MAKEPAD_NETWORKED_AI_HUB").ok().as_deref(), || {
        read_at(&crate::fleet::makepad_home().join("network.conf")).on
    })
}

fn networked_from(env: Option<&str>, setting: impl FnOnce() -> bool) -> bool {
    match env.map(str::trim) {
        Some("1") => true,
        Some("0") => false,
        _ => setting(),
    }
}

/// Fleet nodes serve the fleet independently of the user-facing network toggle.
/// `MAKEPAD_AI_HUB_ROLE=server|user` overrides all automatic role detection.
pub fn hub_role() -> HubRole {
    role_from(
        std::env::var("MAKEPAD_AI_HUB_ROLE").ok().as_deref(),
        std::env::var_os(NODE_CONF_ENV).as_deref(),
        || read_at(&crate::fleet::makepad_home().join("network.conf")).server,
        || {
            #[cfg(not(target_arch = "wasm32"))]
            {
                crate::fleet::id::me().map(|me| me.cred.role)
            }
            #[cfg(target_arch = "wasm32")]
            {
                None
            }
        },
    )
}

fn role_from(
    env: Option<&str>,
    node_conf: Option<&OsStr>,
    server_setting: impl FnOnce() -> bool,
    credential_role: impl FnOnce() -> Option<crate::fleet::id::Role>,
) -> HubRole {
    match env.map(str::trim) {
        Some("server") => HubRole::FleetServer,
        Some("user") => HubRole::UserMachine,
        _ => {
            if node_conf.is_some_and(|path| !path.is_empty())
                || server_setting()
                || credential_role() == Some(crate::fleet::id::Role::Node)
            {
                HubRole::FleetServer
            } else {
                HubRole::UserMachine
            }
        }
    }
}

pub fn serves_fleet() -> bool {
    hub_role() == HubRole::FleetServer
}

/// Fleet servers stay networked even when the user-facing toggle is off.
pub fn hub_networked() -> bool {
    hub_networked_from(serves_fleet(), networked_ai_hub)
}

pub(crate) fn hub_networked_from(serves_fleet: bool, setting: impl FnOnce() -> bool) -> bool {
    serves_fleet || setting()
}

fn read_at(path: &Path) -> Settings {
    let stamp = fs::metadata(path).and_then(|meta| Ok((meta.modified()?, meta.len())));
    let Ok((modified, len)) = stamp else {
        *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Settings::default();
    };
    {
        let cached = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = cached.as_ref() {
            if cached.path == path && cached.modified == modified && cached.len == len {
                return cached.settings;
            }
        }
    }
    let settings = fs::read_to_string(path).map(|text| parse(&text)).unwrap_or_default();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cached { path: path.to_path_buf(), modified, len, settings });
    settings
}

/// Atomically replace just this setting, preserving other keys and comments.
pub fn set_networked_ai_hub(on: bool) -> io::Result<()> {
    set_at(&crate::fleet::makepad_home().join("network.conf"), on)
}

fn set_at(path: &Path, on: bool) -> io::Result<()> {
    let home = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "network.conf needs a parent directory"))?;
    fs::create_dir_all(home)?;
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => HEADER.to_string(),
        Err(e) => return Err(e),
    };
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let (temp, mut file) = loop {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = home.join(format!(".network.conf.{}.{id}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        file.write_all(set_in(&text, on).as_bytes())?;
        file.sync_all()
    })();
    drop(file);
    let result = result.and_then(|_| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    result
}

fn parse(text: &str) -> Settings {
    let mut settings = Settings::default();
    for line in text.lines() {
        if let Some((key, value)) = line.split('#').next().and_then(|line| line.split_once('=')) {
            match key.trim() {
                NETWORKED_AI_HUB_KEY => settings.on = value.trim() == "on",
                AI_HUB_ROLE_KEY => settings.server = value.trim() == "server",
                _ => {}
            }
        }
    }
    settings
}

fn set_in(text: &str, on: bool) -> String {
    let value = if on { "on" } else { "off" };
    let mut out = String::with_capacity(text.len() + 32);
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let content = line.split('#').next().unwrap_or(line);
        if let Some((key, old)) = content.split_once('=') {
            if key.trim() == NETWORKED_AI_HUB_KEY {
                found = true;
                let leading = old.len() - old.trim_start_matches([' ', '\t']).len();
                let end = old.trim_end().len().max(leading);
                out.push_str(key);
                out.push('=');
                out.push_str(&old[..leading]);
                out.push_str(value);
                out.push_str(&old[end..]);
                out.push_str(&line[content.len()..]);
                continue;
            }
        }
        out.push_str(line);
    }
    if !found {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{NETWORKED_AI_HUB_KEY} = {value}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_without_changing_makepad_home() {
        let unique = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("makepad-network-conf-{}-{unique}", std::process::id()));
        let path = dir.join("network.conf");
        assert!(!read_at(&path).on);
        set_at(&path, true).unwrap();
        assert!(fs::read_to_string(&path).unwrap().starts_with(HEADER));
        assert!(read_at(&path).on);
        fs::write(&path, "ai_hub_role = server\nnetworked_ai_hub = on\n").unwrap();
        assert!(read_at(&path).server);
        set_at(&path, false).unwrap();
        assert!(!read_at(&path).on);
        assert!(read_at(&path).server);
        fs::write(&path, "ai_hub_role = user\n").unwrap();
        assert!(!read_at(&path).server);
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&dir).unwrap();
        assert!(!read_at(&path).on);
        assert!(!read_at(&path).server);
    }

    #[test]
    fn absent_and_garbage_are_off() {
        for text in ["", "other = on\n", "# networked_ai_hub = on\n", "networked_ai_hub = yes", "networked_ai_hub = ON", "networked_ai_hub =", "networked_ai_hub = on\nnetworked_ai_hub = garbage"] {
            assert!(!parse(text).on, "{text}");
        }
        assert!(parse("networked_ai_hub = on").on);
        assert!(parse("  networked_ai_hub = on # enabled\n").on);
    }

    #[test]
    fn fleet_role_sources_and_overrides() {
        use crate::fleet::id::Role;

        for text in ["", "# ai_hub_role = server\n", "ai_hub_role = user", "ai_hub_role = SERVER", "ai_hub_role =", "ai_hub_role = server\nai_hub_role = garbage"] {
            assert!(!parse(text).server, "{text}");
        }
        let settings = parse("  ai_hub_role = server # fleet node\nnetworked_ai_hub = off\n");
        assert!(settings.server);
        assert!(!settings.on);
        assert_eq!(role_from(None, None, || settings.server, || None), HubRole::FleetServer);
        for role in [None, Some(Role::Device), Some(Role::Paired), Some(Role::Node)] {
            let expected = if role == Some(Role::Node) { HubRole::FleetServer } else { HubRole::UserMachine };
            assert_eq!(role_from(None, None, || false, || role), expected);
        }
        assert_eq!(role_from(None, Some(OsStr::new("ai-hub.conf")), || false, || None), HubRole::FleetServer);
        assert_eq!(role_from(None, Some(OsStr::new("")), || false, || None), HubRole::UserMachine);
        assert_eq!(role_from(Some(" server "), None, || false, || None), HubRole::FleetServer);
        assert_eq!(role_from(Some(" user "), Some(OsStr::new("ai-hub.conf")), || true, || Some(Role::Node)), HubRole::UserMachine);
        assert_eq!(role_from(Some("invalid"), None, || false, || Some(Role::Node)), HubRole::FleetServer);
        assert_eq!(role_from(Some(""), None, || false, || None), HubRole::UserMachine);
    }

    #[test]
    fn toggle_overrides_do_not_disable_fleet_servers() {
        for (env, expected) in [(Some(" 1 "), true), (Some(" 0 "), false)] {
            assert_eq!(networked_from(env, || !expected), expected);
        }
        for env in [None, Some(""), Some("invalid")] {
            assert!(networked_from(env, || true));
            assert!(!networked_from(env, || false));
        }
        for (env, expected) in [("server", true), ("user", false)] {
            let role = role_from(Some(env), None, || false, || None);
            assert_eq!(hub_networked_from(role == HubRole::FleetServer, || networked_from(Some("0"), || true)), expected);
        }
    }

    #[test]
    fn setting_preserves_other_lines_and_comments() {
        let text = "# user notes\r\nother = on\r\n networked_ai_hub = off  # hub\r\n# end";
        let enabled = "# user notes\r\nother = on\r\n networked_ai_hub = on  # hub\r\n# end";
        assert_eq!(set_in(text, true), enabled);
        assert_eq!(set_in(enabled, false), text);
        assert_eq!(set_in("# notes\nother = on", false), "# notes\nother = on\nnetworked_ai_hub = off\n");
        assert_eq!(set_in("", true), "networked_ai_hub = on\n");
        assert_eq!(set_in("networked_ai_hub = on\nnetworked_ai_hub = yes\n", false), "networked_ai_hub = off\nnetworked_ai_hub = off\n");
    }
}
