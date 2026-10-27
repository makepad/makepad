//! Desk Link installed as a macOS app (`tools/desklink/install-mac.sh` makes
//! `~/Applications/Desk Link.app`): while it runs from a bundle it records its
//! pid in `~/.makepad/desklink/app.pid` (`MAKEPAD_HOME` honoured), so the
//! installer stops the installed copy by that pid only before replacing it, and it
//! behaves as a Dock app ([`is_dock_app`]).
use std::path::PathBuf;

/// This bundle's executable, when the app runs from an installed `.app`.
fn bundle_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let in_bundle = macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && contents.parent()?.extension()? == "app";
    (cfg!(target_os = "macos") && in_bundle).then_some(exe)
}

/// Running as the installed Desk Link.app: a regular Dock app (window at launch, the Dock icon stays
/// while the window is closed), not only a menu-bar app.
pub fn is_dock_app() -> bool {
    bundle_exe().is_some()
}

fn pid_path() -> PathBuf {
    makepad_network::fleet::makepad_home().join("desklink").join("app.pid")
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

/// Removes the record when it is this process's.
pub fn clear_pid() {
    if bundle_exe().is_some() {
        let path = pid_path();
        if std::fs::read_to_string(&path).ok().and_then(|t| t.trim().parse::<u32>().ok()) == Some(std::process::id()) {
            let _ = std::fs::remove_file(path);
        }
    }
}
