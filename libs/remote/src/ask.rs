//! Whether a process opens its makepad-remote endpoint: one rule, read here, for a native build's
//! app (makepad-platform's `remote_core::request`) and for `makepad run` (the JIT's inspect
//! endpoint, which the app's commands and services register with) alike.
//!
//! The endpoint is off by default. `--remote`, `MAKEPAD_REMOTE=1` or `MAKEPAD_INSPECT=1` open it
//! for an agent, a test or an inspector (the instance behaves as one: no focus taken, a tagged
//! title). `MAKEPAD_REMOTE=local`, or the per-user setting ([`setting_path`], the one word
//! `local`), opens it with the app behaving as an ordinary launch: a developer computer turns it
//! on so a local agent reaches the apps the person started. `MAKEPAD_REMOTE=0` closes it, setting
//! or not.

use std::path::PathBuf;

/// What the command line, the environment and the per-user setting ask of the remote endpoint.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Nothing asked, or `MAKEPAD_REMOTE=0`: no endpoint and no registry file.
    Off,
    /// `MAKEPAD_REMOTE=local` or the per-user setting: the endpoint is open (same user only,
    /// nothing on the network), and the app behaves as an ordinary launch.
    Local,
    /// `--remote`, `MAKEPAD_REMOTE=1` or `MAKEPAD_INSPECT=1`: launched for an agent, a test or an
    /// inspector. The endpoint is open and the instance behaves as one (no focus stealing, a
    /// tagged title, its own instance).
    On,
    /// A form that no longer exists (a TCP port or address); the message says what to use.
    Refused(String),
}

/// The per-user setting's one word that turns the endpoint on.
pub const SETTING_ON: &str = "local";

fn refused(form: &str) -> Request {
    Request::Refused(format!(
        "{form}: a port or address is not supported any more; --remote (no value) or MAKEPAD_REMOTE=1 opens a local \
         Unix-socket endpoint (see the startup line, `makepad-remote app <pid> status`); other machines are reached \
         through the hub (makepad hub)"
    ))
}

/// `host:port` or a bare port.
fn looks_like_bind(value: &str) -> bool {
    let value = value.trim();
    value.parse::<u16>().is_ok()
        || value.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

fn request_from_args_env(args: impl Iterator<Item = String>, env: Option<&str>) -> Option<Request> {
    let mut args = args.skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--remote" {
            if let Some(next) = args.peek().filter(|n| looks_like_bind(n)) {
                return Some(refused(&format!("--remote {next}")));
            }
            return Some(Request::On);
        }
        if let Some(value) = arg.strip_prefix("--remote=") {
            return Some(refused(&format!("--remote={value}")));
        }
    }
    match env.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") => None,
        Some("0" | "off" | "false" | "no") => Some(Request::Off),
        Some("1" | "on" | "true" | "yes") => Some(Request::On),
        Some(SETTING_ON) => Some(Request::Local),
        Some(_) => Some(refused(&format!("MAKEPAD_REMOTE={}", env.unwrap_or_default()))),
    }
}

fn request_from_setting(setting: Option<&str>) -> Request {
    if setting.is_some_and(|s| s.trim().eq_ignore_ascii_case(SETTING_ON)) { Request::Local } else { Request::Off }
}

/// The one rule, for a native build and `makepad run` alike: a command line's `--remote` (the
/// first argument is the program) opens the endpoint for an agent; else `MAKEPAD_REMOTE` (`env`:
/// `1`/`on`/`true`/`yes` for an agent, `local` as an ordinary launch, `0`/`off`/`false`/`no`
/// off); else, with it unset or empty, the per-user setting (`setting`: [`SETTING_ON`] opens it
/// as an ordinary launch); else nothing is open.
pub fn request_from(args: impl Iterator<Item = String>, env: Option<&str>, setting: Option<&str>) -> Request {
    request_from_args_env(args, env).unwrap_or_else(|| request_from_setting(setting))
}

/// The per-user setting's file: `remote` in Makepad's per-user folder (`MAKEPAD_HOME`, else
/// `.makepad` in the user's home; makepad-platform's `home::makepad_home` is the same rule).
/// Read by apps launched outside a shell too, which `MAKEPAD_REMOTE` does not reach.
pub fn setting_path() -> Option<PathBuf> {
    let base = match std::env::var_os("MAKEPAD_HOME").filter(|h| !h.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).filter(|h| !h.is_empty())?).join(".makepad"),
    };
    Some(base.join("remote"))
}

/// The per-user setting's word, if it is set.
pub fn setting() -> Option<String> {
    std::fs::read_to_string(setting_path()?).ok()
}

fn request_for_with(
    args: impl Iterator<Item = String>,
    env: Option<&str>,
    inspect: bool,
    read_setting: impl FnOnce() -> Option<String>,
) -> Request {
    match request_from_args_env(args, env) {
        Some(Request::Off | Request::Local) if inspect => Request::On,
        Some(request) => request,
        None if inspect => Request::On,
        None => request_from_setting(read_setting().as_deref()),
    }
}

/// A process's request by [`request_from`], with its command line (`args`, the program first),
/// `MAKEPAD_REMOTE` (`env`, which a caller may take from elsewhere: an Android app's system
/// property) and the per-user setting (read only when neither the command line, `env`, nor
/// `MAKEPAD_INSPECT=1` already decides); `MAKEPAD_INSPECT=1` asks as `--remote` does.
pub fn request_for(args: impl Iterator<Item = String>, env: Option<&str>) -> Request {
    let inspect = std::env::var("MAKEPAD_INSPECT").as_deref() == Ok("1");
    request_for_with(args, env, inspect, setting)
}

/// [`request_for`] this process's command line.
pub fn request_with(env: Option<&str>) -> Request {
    request_for(std::env::args(), env)
}

/// This process's request ([`request_with`] the environment's `MAKEPAD_REMOTE`).
pub fn request() -> Request {
    request_with(std::env::var("MAKEPAD_REMOTE").ok().as_deref())
}

/// Turns the endpoint on for this user's apps (the per-user setting, [`SETTING_ON`]): what a
/// developer computer does once, and what devhub's hub does at each start on a box it manages.
/// True when it was not on already.
pub fn turn_on() -> std::io::Result<bool> {
    let path = setting_path().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home folder for the per-user setting"))?;
    if std::fs::read_to_string(&path).is_ok_and(|s| s.trim().eq_ignore_ascii_case(SETTING_ON)) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, format!("{SETTING_ON}\n"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one rule: off with nothing asked; `--remote` and `MAKEPAD_REMOTE=1` for an agent;
    /// `MAKEPAD_REMOTE=local` or the per-user setting as an ordinary launch; `MAKEPAD_REMOTE=0`
    /// over the setting; the old port and address forms refused with what to use.
    #[test]
    fn the_endpoint_is_off_unless_a_flag_the_environment_or_the_setting_asks() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(request_from(args(&["app"]), None, None), Request::Off, "unset: off");
        assert_eq!(request_from(args(&["app"]), Some(""), None), Request::Off, "empty: off");
        assert_eq!(request_from(args(&["app", "--remote-title-tag=x"]), None, None), Request::Off, "a title tag asks for nothing");
        assert_eq!(request_from(args(&["app", "--remote"]), None, None), Request::On, "the bare flag: for an agent");
        assert_eq!(request_from(args(&["app", "--remote", "a.png"]), Some("0"), None), Request::On, "the flag over the environment");
        for on in ["1", "on", "true", "yes"] {
            assert_eq!(request_from(args(&["app"]), Some(on), None), Request::On, "MAKEPAD_REMOTE={on}: for an agent");
        }
        assert_eq!(request_from(args(&["app"]), Some("local"), None), Request::Local, "MAKEPAD_REMOTE=local: an ordinary launch");
        assert_eq!(request_from(args(&["app"]), Some(" Local\n"), None), Request::Local, "its case and spaces do not matter");
        // the per-user setting: read only when the environment leaves it open
        assert_eq!(request_from(args(&["app"]), None, Some("local\n")), Request::Local, "the setting turns it on");
        assert_eq!(request_from(args(&["app"]), Some(""), Some("local")), Request::Local, "an empty MAKEPAD_REMOTE leaves it to the setting");
        assert_eq!(request_from(args(&["app"]), None, Some("")), Request::Off, "an empty setting: off");
        assert_eq!(request_from(args(&["app"]), None, Some("yes")), Request::Off, "the setting knows one word");
        for off in ["0", "off", "false", "no"] {
            assert_eq!(request_from(args(&["app"]), Some(off), Some("local")), Request::Off, "MAKEPAD_REMOTE={off} closes it over the setting");
        }
        assert_eq!(request_from(args(&["app"]), Some("1"), Some("local")), Request::On, "an agent's ask over the setting");
        // the HTTP bridge's forms are gone, and say so
        for refused in [
            request_from(args(&["app", "--remote=5000"]), None, None),
            request_from(args(&["app", "--remote", "5000"]), None, None),
            request_from(args(&["app", "--remote", "0.0.0.0:8399"]), None, None),
            request_from(args(&["app"]), Some("8399"), Some("local")),
            request_from(args(&["app"]), Some("127.0.0.1:8399"), None),
        ] {
            match refused {
                Request::Refused(msg) => assert!(msg.contains("no value") && msg.contains("hub"), "{msg}"),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    /// The per-user setting file is touched only when neither `--remote`, `MAKEPAD_REMOTE`,
    /// nor `MAKEPAD_INSPECT=1` already decides the answer.
    #[test]
    fn per_user_setting_is_read_only_when_nothing_else_decides() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        let unread = || -> Option<String> { panic!("setting() must not be read when args or env already decide") };

        // `--remote` (valid or refused) decides without reading the setting.
        assert_eq!(request_for_with(args(&["app", "--remote"]), None, false, unread), Request::On);
        assert_eq!(request_for_with(args(&["app", "--remote"]), None, true, unread), Request::On);
        assert!(matches!(request_for_with(args(&["app", "--remote=5000"]), None, false, unread), Request::Refused(_)));
        assert!(matches!(request_for_with(args(&["app", "--remote=5000"]), None, true, unread), Request::Refused(_)));
        assert!(matches!(request_for_with(args(&["app", "--remote", "5000"]), None, false, unread), Request::Refused(_)));

        // Non-empty `MAKEPAD_REMOTE` decides without reading the setting.
        for off in ["0", "off", "false", "no"] {
            assert_eq!(request_for_with(args(&["app"]), Some(off), false, unread), Request::Off);
            assert_eq!(request_for_with(args(&["app"]), Some(off), true, unread), Request::On);
        }
        for on in ["1", "on", "true", "yes"] {
            assert_eq!(request_for_with(args(&["app"]), Some(on), false, unread), Request::On);
            assert_eq!(request_for_with(args(&["app"]), Some(on), true, unread), Request::On);
        }
        assert_eq!(request_for_with(args(&["app"]), Some("local"), false, unread), Request::Local);
        assert_eq!(request_for_with(args(&["app"]), Some("local"), true, unread), Request::On);
        assert!(matches!(request_for_with(args(&["app"]), Some("8399"), false, unread), Request::Refused(_)));
        assert!(matches!(request_for_with(args(&["app"]), Some("8399"), true, unread), Request::Refused(_)));

        // `MAKEPAD_INSPECT=1` with open args/env also decides without reading the setting.
        assert_eq!(request_for_with(args(&["app"]), None, true, unread), Request::On);
        assert_eq!(request_for_with(args(&["app"]), Some(""), true, unread), Request::On);

        // Only when args, `MAKEPAD_REMOTE`, and `MAKEPAD_INSPECT` leave it open is the setting read once.
        for env in [None, Some(""), Some("  ")] {
            let mut reads = 0;
            assert_eq!(
                request_for_with(args(&["app"]), env, false, || { reads += 1; Some("local\n".into()) }),
                Request::Local,
                "env={env:?} falls through to the setting"
            );
            assert_eq!(reads, 1, "env={env:?} reads the setting exactly once");

            let mut reads = 0;
            assert_eq!(
                request_for_with(args(&["app"]), env, false, || { reads += 1; None }),
                Request::Off,
                "env={env:?} with no setting is Off"
            );
            assert_eq!(reads, 1, "env={env:?} reads the setting exactly once");
        }
    }
}
