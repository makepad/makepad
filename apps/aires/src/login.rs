use makepad_shell_kit::tray_app::{bundle_exe, AppPid, LoginItem};

/// launchd starts it again after a crash, not after Quit (an exit 0). tools/aires/install-mac.sh
/// writes the same plist, so this switch shows what the installer set up.
const KEEP_ALIVE: &str = "    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n";

fn item() -> Option<LoginItem> {
    Some(LoginItem {
        label: "nl.makepad.aires",
        program: bundle_exe()?,
        extra: KEEP_ALIVE.into(),
    })
}

pub fn state() -> Option<bool> {
    item().map(|i| i.is_on())
}

pub fn set(on: bool) -> Result<(), String> {
    item().ok_or("not running from an installed bundle")?.set(on)
}

pub fn pid() -> AppPid {
    let mut home = makepad_devhub::proto::home();
    home.pop();
    AppPid(home.join("aires/app.pid"))
}
