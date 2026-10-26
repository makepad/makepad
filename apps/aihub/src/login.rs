//! "Open at login" (macOS): a per-user LaunchAgent, `~/Library/LaunchAgents/nl.makepad.aihub.plist`
//! (RunAtLoad, ProgramArguments = this bundle's executable), written and removed by the app
//! itself. No admin rights, no `launchctl` (launchd reads the folder at the next login, so
//! switching it on never starts a second copy), no privacy prompt.
//!
//! Offered only when the app runs from an installed bundle (`.../AI Hub.app/Contents/MacOS/<exe>`):
//! the entry must point at something that is still there at the next login, which the
//! `makepad run` loader's temporary build is not. The switch shows on when the plist exists and
//! points at this bundle's executable; a plist pointing elsewhere (an older copy) shows off, and
//! switching on rewrites it for this bundle.
//!
//! The app also records its pid in `app.pid` next to its settings while it runs from a bundle,
//! so the installer can stop the installed copy (by that pid only) before replacing it.

use makepad_shell_kit::tray_app::{bundle_exe, AppPid, LoginItem};

pub const LABEL: &str = "nl.makepad.aihub";

fn item() -> Option<LoginItem> {
    Some(LoginItem { label: LABEL, program: bundle_exe()?, extra: String::new() })
}

/// None: not offered here. Some(on): the switch's state.
pub fn state() -> Option<bool> {
    item().map(|i| i.is_on())
}

pub fn set(on: bool) -> Result<(), String> {
    item().ok_or("not running from an installed bundle")?.set(on)
}

/// The pid file next to the settings (the installer reads `~/.makepad/ai-hub/app.pid`).
fn pid() -> AppPid {
    AppPid(makepad_ai_hub::home::makepad_home().join("ai-hub").join("app.pid"))
}

/// Records this process while it runs from a bundle.
pub fn write_pid() {
    pid().write()
}

pub fn clear_pid() {
    pid().clear()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_installed_bundle_offers_the_switch() {
        // the test binary runs from target/, not from an .app
        assert_eq!(state(), None);
        assert!(set(true).is_err());
    }
}
