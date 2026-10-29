//! The environment of a process a makepad host starts. Two kinds of variables never pass from
//! a parent to its children:
//! - the direct display's (DRM/KMS) request and configuration: only the window manager's own
//!   start asks for the direct shell; a child that saw `MAKEPAD=linux_direct` would take the
//!   displays and every input device itself,
//! - the parent's own hosted identity (its host, client id and hidden mode): a child is not its
//!   parent's client; it finds the window manager itself (`MAKEPAD_WM_SOCKET`, which passes).
//!
//! Every spawn site of the window manager, of the apps it hosts and of their tools (Code's runs,
//! the terminal's shell) calls [`scrub`], so the rule holds down the whole process tree.

use std::ffi::OsString;
use std::process::Command;

/// Read only by the direct-display shell and the WM's display settings.
pub const DIRECT_SHELL_ENV: &[&str] = &[
    "MAKEPAD_VULKAN_COMPOSITOR_PCI",
    "MAKEPAD_VULKAN_COMPOSITOR_UUID",
    "MAKEPAD_DISPLAY_ORDER",
    "MAKEPAD_DRM_DEVICE",
    "MAKEPAD_DRM_CONNECTOR",
    "MAKEPAD_DRM_MODE",
    "MAKEPAD_DRM_MODES",
    "MAKEPAD_WM_GPU_FROM_SAVED",
    "MAKEPAD_WM_ORDER_FROM_SAVED",
    "MAKEPAD_WM_MODES_FROM_SAVED",
];

/// What makes a process one particular hosted client of its host.
pub const HOSTED_IDENTITY_ENV: &[&str] = &["STUDIO", "STUDIO_HOST", "STUDIO_BUILD", "STUDIO_CRATE", "MAKEPAD_STDIN_LOOP", "MAKEPAD_HIDE_WINDOWS"];

/// A test instance's marks: a lane's run is badged and kept off the local network
/// (`MAKEPAD_TEST_LANE`, `MAKEPAD_LAN`), a hidden one muted (`SANDBOX_MUTE`), and its builds and
/// JIT state go to the lane's own dirs. A registry launch (the dock's app) never inherits them.
pub const LANE_ENV: &[&str] = &["MAKEPAD_TEST_LANE", "MAKEPAD_LAN", "SANDBOX_MUTE", "CARGO_TARGET_DIR"];

/// The `makepad run` settings a launcher may carry (`MAKEPAD_RUN_HOME`, `_TARGET_DIR`, `_STD`,
/// `_REPORT_TO`, ...): a registry launch starts with none of them; a machine sets its own in
/// jit.env ([`crate::machine`]).
pub const LANE_ENV_PREFIX: &str = "MAKEPAD_RUN_";

/// Clears [`LANE_ENV`] and every `MAKEPAD_RUN_*` of this process from `command`: what a registry
/// launch starts is the same app with the same settings however its launcher was started.
pub fn scrub_lane(command: &mut Command) {
    for key in LANE_ENV {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| key.starts_with(LANE_ENV_PREFIX)) {
            command.env_remove(key);
        }
    }
}

/// `MAKEPAD`'s tokens without the window-backend ones (`linux_direct`, `direct`, `wayland`,
/// `x11`); None when nothing is left.
pub fn child_makepad_env(value: &str) -> Option<String> {
    let kept: Vec<&str> = value
        .split(['+', ','])
        .filter(|t| !t.is_empty() && !matches!(*t, "linux_direct" | "direct" | "wayland" | "x11"))
        .collect();
    (!kept.is_empty()).then(|| kept.join("+"))
}

/// This process's environment as [`scrub`] leaves it for a child, for a spawn site that hands
/// the child a whole environment block (the terminal's Windows PTY). Names compare as the OS
/// does: ignoring case on Windows.
pub fn scrubbed_vars() -> Vec<(OsString, OsString)> {
    let same = |a: &str, b: &str| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    let mut vars = Vec::new();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if DIRECT_SHELL_ENV.iter().chain(HOSTED_IDENTITY_ENV).any(|d| same(&name, d)) {
            continue;
        }
        if same(&name, "MAKEPAD") {
            match value.to_str().map(child_makepad_env) {
                Some(Some(kept)) => vars.push((key, kept.into())),
                Some(None) => {}
                None => vars.push((key, value)),
            }
            continue;
        }
        vars.push((key, value));
    }
    vars
}

/// Clears the variables above from `command`'s environment (a host sets its child's own
/// identity afterwards).
pub fn scrub(command: &mut Command) {
    for key in DIRECT_SHELL_ENV.iter().chain(HOSTED_IDENTITY_ENV) {
        command.env_remove(key);
    }
    match std::env::var("MAKEPAD").ok().as_deref().map(child_makepad_env) {
        Some(Some(kept)) => {
            command.env("MAKEPAD", kept);
        }
        Some(None) => {
            command.env_remove("MAKEPAD");
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_tokens_leave_makepad() {
        assert_eq!(child_makepad_env("linux_direct+vulkan").as_deref(), Some("vulkan"));
        assert_eq!(child_makepad_env("linux_direct"), None);
        assert_eq!(child_makepad_env("wayland,gl").as_deref(), Some("gl"));
        assert_eq!(child_makepad_env("vulkan").as_deref(), Some("vulkan"));
    }

    #[test]
    fn scrub_removes_direct_and_hosted_identity() {
        let mut command = Command::new("true");
        command.env("MAKEPAD_DRM_MODES", "x").env("STUDIO_HOST", "y");
        scrub(&mut command);
        let removed: Vec<String> = command.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        for key in DIRECT_SHELL_ENV.iter().chain(HOSTED_IDENTITY_ENV) {
            assert!(removed.iter().any(|k| k == key), "{key} is not removed");
        }
        // a host's own setting after the scrub wins
        command.env("STUDIO_HOST", "mine");
        assert!(command.get_envs().any(|(k, v)| k == "STUDIO_HOST" && v.is_some_and(|v| v == "mine")));
    }
}
