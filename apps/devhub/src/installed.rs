//! devhub on the Mac: "Open at login" in the tray menu makes the per-user LaunchAgent
//! `nl.makepad.devhub`, which starts the Dock wrapper (`~/Applications/devhub.app`, made by
//! local/dock-launchers.sh: `cargo build` of the JIT host, then `makepad run makepad-app-devhub`,
//! so the master always runs the current code) at login, and again whenever it exits with an error
//! (a crash, a failed build; Quit exits cleanly and stays quit). It is offered only when that
//! wrapper exists. The running master's pid is `~/.makepad/devhub/master.pid` (`MAKEPAD_HOME`
//! honoured), whatever runs it.

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
        extra: format!("    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n    <key>ThrottleInterval</key>\n    <integer>{THROTTLE_S}</integer>\n"),
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
    fn the_login_item_starts_the_dock_wrapper_and_restarts_it_after_a_crash() {
        let text = super::item_for("/Users/u/Applications/devhub.app/Contents/MacOS/devhub".into()).plist_text();
        assert!(text.contains("devhub.app/Contents/MacOS/devhub</string>"));
        assert!(text.contains("<key>SuccessfulExit</key>\n        <false/>"));
    }
}
