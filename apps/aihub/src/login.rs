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

use std::path::PathBuf;

pub const LABEL: &str = "nl.makepad.aihub";

/// This bundle's executable, when the app runs from an installed `.app`.
pub fn bundle_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let in_bundle = exe.parent()?.file_name()? == "MacOS" && exe.parent()?.parent()?.file_name()? == "Contents" && exe.parent()?.parent()?.parent()?.extension()? == "app";
    (cfg!(target_os = "macos") && in_bundle).then_some(exe)
}

/// Where the plist lives; `MAKEPAD_AIHUB_LAUNCH_AGENTS_DIR` replaces `~/Library/LaunchAgents` (tests).
fn plist_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("MAKEPAD_AIHUB_LAUNCH_AGENTS_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?).join("Library/LaunchAgents"),
    };
    Some(dir.join(format!("{LABEL}.plist")))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn plist_text(exe: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{LABEL}</string>\n    <key>ProgramArguments</key>\n    <array>\n        <string>{}</string>\n    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>LimitLoadToSessionType</key>\n    <string>Aqua</string>\n</dict>\n</plist>\n",
        xml(exe)
    )
}

/// None: not offered here. Some(on): the switch's state.
pub fn state() -> Option<bool> {
    let exe = bundle_exe()?;
    let text = std::fs::read_to_string(plist_path()?).unwrap_or_default();
    Some(text.contains(&format!("<string>{}</string>", xml(&exe.to_string_lossy()))) && text.contains("<key>RunAtLoad</key>"))
}

pub fn set(on: bool) -> Result<(), String> {
    let exe = bundle_exe().ok_or("not running from an installed bundle")?;
    let path = plist_path().ok_or("no home folder")?;
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, plist_text(&exe.to_string_lossy())).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        }
    }
}

/// The pid file next to the settings (the installer reads `~/.makepad/ai-hub/app.pid`).
fn pid_path() -> PathBuf {
    makepad_ai_hub::home::makepad_home().join("ai-hub").join("app.pid")
}

/// Records this process while it runs from a bundle.
pub fn write_pid() {
    if bundle_exe().is_some() {
        let path = pid_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("{}\n", std::process::id()));
    }
}

pub fn clear_pid() {
    if bundle_exe().is_some() {
        let path = pid_path();
        if std::fs::read_to_string(&path).ok().and_then(|t| t.trim().parse::<u32>().ok()) == Some(std::process::id()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_names_the_label_the_executable_and_run_at_load() {
        let text = plist_text("/Users/x/Applications/AI Hub.app/Contents/MacOS/makepad-app-aihub");
        assert!(text.contains("<string>nl.makepad.aihub</string>"));
        assert!(text.contains("<string>/Users/x/Applications/AI Hub.app/Contents/MacOS/makepad-app-aihub</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n    <true/>"));
    }

    #[test]
    fn only_an_installed_bundle_offers_the_switch() {
        // the test binary runs from target/, not from an .app
        assert_eq!(state(), None);
        assert!(set(true).is_err());
    }
}
