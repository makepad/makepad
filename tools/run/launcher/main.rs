//! Tiny OS entry point: select the installation's active loader and start one app.
//! This binary uses only std; it must not link the makepad runner or its JIT dependencies.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

// Mirrors apps/commercial/runner/src/jitupdate.rs active(), including its src fallback.
fn loader_path(root: &Path, current: &str, is_file: impl Fn(&Path) -> bool) -> PathBuf {
    let named = |name: &str| root.join(name).join("target/host").join(if cfg!(windows) { "makepad.exe" } else { "makepad" });
    let name = current.trim();
    if name == "src" || (name.len() == 40 && name.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))) {
        let selected = named(name);
        if is_file(&selected) { return selected; }
    }
    named("src")
}

fn active(home: &Path) -> PathBuf {
    let root = home.join("cache/loader");
    let current = std::fs::read_to_string(root.join("current")).unwrap_or_default();
    loader_path(&root, &current, Path::is_file)
}

// The same app-id rule as tools/run/runner/src/launch.rs valid_id().
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The value of `<key>NAME</key><string>VALUE</string>` in an Info.plist's text.
#[cfg(any(target_os = "macos", test))]
fn plist_string(plist: &str, name: &str) -> Option<String> {
    let key = format!("<key>{name}</key>");
    let rest = &plist[plist.find(&key)? + key.len()..];
    let rest = rest.trim_start().strip_prefix("<string>")?;
    let value = &rest[..rest.find("</string>")?];
    Some(value.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}

#[cfg(target_os = "macos")]
fn launch(exe: &Path) -> Result<(), String> {
    use std::os::unix::{fs::symlink, process::CommandExt};
    let macos = exe.parent().ok_or("the launcher has no parent directory")?;
    let contents = macos.parent().ok_or("the launcher has no Contents directory")?;
    let plist_path = contents.join("Info.plist");
    let plist = std::fs::read_to_string(&plist_path).map_err(|e| format!("{}: {e}", plist_path.display()))?;
    let id = plist_string(&plist, "MakepadApp").filter(|id| valid_id(id)).ok_or("Info.plist has no valid MakepadApp")?;
    let home = plist_string(&plist, "MakepadRunHome").filter(|home| !home.is_empty()).map(PathBuf::from).ok_or("Info.plist has no MakepadRunHome")?;
    let loader = active(&home);
    let link = macos.join("makepad");
    if std::fs::read_link(&link).ok().as_deref() != Some(loader.as_path()) {
        let part = macos.join(format!(".makepad-{}.part", std::process::id()));
        match std::fs::remove_file(&part) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", part.display())),
            _ => {}
        }
        symlink(&loader, &part).map_err(|e| format!("{}: {e}", part.display()))?;
        if let Err(e) = std::fs::rename(&part, &link) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{}: {e}", link.display()));
        }
    }
    // Both the exec path and argv[0] stay inside the bundle so NSBundle sees its identity.
    // Launch Services' extra arguments are not arguments to the app.
    let error = Command::new(&link).arg0(&link).arg("app").arg(id).env("MAKEPAD_RUN_HOME", home).exec();
    Err(format!("{}: {error}", link.display()))
}

#[cfg(not(target_os = "macos"))]
fn launch(exe: &Path) -> Result<(), String> {
    let home = exe.parent().and_then(Path::parent).ok_or("the launcher is not inside <home>/apps")?;
    let mut args = std::env::args_os().skip(1);
    let id = args.next().and_then(|id| id.into_string().ok()).filter(|id| valid_id(id)).ok_or("usage: makepad-launcher <app-id>")?;
    if args.next().is_some() { return Err("usage: makepad-launcher <app-id>".into()); }
    let loader = active(home);
    let mut command = Command::new(&loader);
    command.arg("app").arg(id).env("MAKEPAD_RUN_HOME", home);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(format!("{}: {}", loader.display(), command.exec()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW).spawn().map_err(|e| format!("{}: {e}", loader.display()))?;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("app launchers are not supported on this platform".into())
    }
}

fn main() {
    let result = std::env::current_exe().map_err(|e| format!("cannot find the launcher: {e}")).and_then(|exe| launch(&exe));
    if let Err(error) = result {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "makepad-launcher: {}", error.replace(['\n', '\r'], " "));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_selection() {
        let root = Path::new("installation/cache/loader");
        let binary = if cfg!(windows) { "makepad.exe" } else { "makepad" };
        let src = root.join("src/target/host").join(binary);
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let selected = root.join(commit).join("target/host").join(binary);
        assert_eq!(loader_path(root, &format!(" {commit}\n"), |p| p == selected), selected);
        assert_eq!(loader_path(root, commit, |_| false), src);
        assert_eq!(loader_path(root, "src\n", |p| p == src), src);
        for invalid in ["", "../outside", "/outside", "0123456789ABCDEF0123456789ABCDEF01234567", "abc", "src/../other"] {
            assert_eq!(loader_path(root, invalid, |_| panic!("invalid pointer reached the filesystem")), src);
        }
    }

    #[test]
    fn plist_values() {
        let plist = "<dict><key>MakepadApp</key>\n<string>scope</string><key>MakepadRunHome</key><string>/a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos; &amp;lt;</string><key>NotAString</key><true/></dict>";
        assert_eq!(plist_string(plist, "MakepadApp").as_deref(), Some("scope"));
        assert_eq!(plist_string(plist, "MakepadRunHome").as_deref(), Some("/a & <b> \"c\" 'd' &lt;"));
        assert_eq!(plist_string(plist, "Missing"), None);
        assert_eq!(plist_string(plist, "NotAString"), None);
        assert_eq!(plist_string("<key>MakepadApp</key><string>unfinished", "MakepadApp"), None);
    }
}
