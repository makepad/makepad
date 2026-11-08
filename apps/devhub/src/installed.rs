//! devhub on the Mac: "Open at login" in the tray menu makes the per-user LaunchAgent
//! `nl.makepad.devhub`, which starts the Dock wrapper (`~/Applications/devhub.app`, made by
//! local/dock-launchers.sh: the master's own build of this app in its target dir,
//! `makepad_devhub::orch::roll::build_master_app`'s command, then that binary, so the master runs
//! the landed code natively) at login, and again whenever it exits with an error (a crash, a failed
//! build; Quit exits cleanly and stays quit, and so does a second master, which refuses to start).
//! It is offered only when that wrapper exists. The running master's pid is
//! `~/.makepad/devhub/master.pid` (`MAKEPAD_HOME` honoured), whatever runs it.

use makepad_shell_kit::tray_app::LoginItem;
use std::path::PathBuf;

const LABEL: &str = "nl.makepad.devhub";
/// launchd waits this long before it starts a crashed master again, seconds
const THROTTLE_S: u32 = 30;

/// The Dock wrapper's executable: what the login item starts.
fn wrapper() -> Option<PathBuf> {
    let exe = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?).join("Applications/devhub.app/Contents/MacOS/devhub");
    (cfg!(target_os = "macos") && exe.is_file()).then_some(exe)
}

fn item() -> Option<LoginItem> {
    Some(item_for(wrapper()?))
}

fn item_for(program: PathBuf) -> LoginItem {
    LoginItem {
        label: LABEL,
        program,
        // (launchd ends a job's process group with it: the master's end, a crash or a Quit, would
        // end what it started, the apps it shows and its bucket runs, which outlive it)
        extra: format!("    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n    <key>ThrottleInterval</key>\n    <integer>{THROTTLE_S}</integer>\n    <key>AbandonProcessGroup</key>\n    <true/>\n"),
    }
}

/// None: not offered (no Dock wrapper). Some(on): the menu item's check.
pub fn login() -> Option<bool> {
    item().map(|i| i.is_on())
}

pub fn set_login(on: bool) -> Result<(), String> {
    item().ok_or("no Dock wrapper ~/Applications/devhub.app (local/dock-launchers.sh makes it)")?.set(on)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_login_item_starts_the_dock_wrapper_restarts_it_after_a_crash_and_keeps_its_children() {
        let text = super::item_for("/Users/u/Applications/devhub.app/Contents/MacOS/devhub".into()).plist_text();
        assert!(text.contains("devhub.app/Contents/MacOS/devhub</string>"));
        assert!(text.contains("<key>SuccessfulExit</key>\n        <false/>"));
        assert!(text.contains("<key>AbandonProcessGroup</key>\n    <true/>"), "what the master started outlives it");
    }
}
