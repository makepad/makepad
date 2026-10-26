//! devhub installed as a macOS menu-bar app (`tools/devhub/install-mac.sh` makes
//! `~/Applications/devhub.app`, LSUIElement): "Open at login" in the tray menu (a per-user
//! LaunchAgent `nl.makepad.devhub`, offered only when running from the bundle: a `cargo run`
//! copy has no stable executable to name) and the pid record the installer stops the installed
//! copy by (`~/.makepad/devhub/app.pid`, `MAKEPAD_HOME` honoured).
//!
//! The bundle's own icon (Info.plist CFBundleIconFile) is what macOS shows; devhub sends no
//! runtime app icon of its own (the platform skips its default one for a bundle that names an icon).

use makepad_devhub::proto;
use makepad_shell_kit::tray_app::{bundle_exe, AppPid, LoginItem};

const LABEL: &str = "nl.makepad.devhub";

fn item() -> Option<LoginItem> {
    Some(LoginItem { label: LABEL, program: bundle_exe()?, extra: String::new() })
}

/// None: not offered (not an installed bundle). Some(on): the menu item's check.
pub fn login() -> Option<bool> {
    item().map(|i| i.is_on())
}

pub fn set_login(on: bool) -> Result<(), String> {
    item().ok_or("not running from an installed bundle")?.set(on)
}

fn pid() -> AppPid {
    AppPid(proto::home().join("app.pid"))
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
    #[test]
    fn only_an_installed_bundle_offers_open_at_login() {
        // the test binary runs from target/, not from an .app
        assert_eq!(super::login(), None);
        assert!(super::set_login(true).is_err());
    }
}
