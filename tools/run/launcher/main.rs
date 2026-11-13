//! Tiny OS entry point: select the installation's active loader and start one app.
//! This binary uses only std; it must not link the makepad runner or its JIT dependencies.
//!
//! It starts the loader from a hard link named as the app's program (`program`), so the exec
//! names the process after it as cargo's binary is named (relax-compiler selfexe.rs launch_link;
//! macOS names a process by its exec alone), and names the loader in RELAX_LOADER, which the
//! runner goes by. Where the link cannot be made it starts the loader as before, under the
//! loader's name, and says why in `<home>/cache/launch/launcher.log` (and on stderr).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

// Select the active installed loader, or the source loader when the selection is unavailable.
fn loader_path(root: &Path, current: &str, is_file: impl Fn(&Path) -> bool) -> PathBuf {
    let named = |name: &str| root.join(name).join("target/host").join(if cfg!(windows) { "makepad.exe" } else { "makepad" });
    let name = current.trim();
    if name == "src" || (name.len() == 40 && name.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))) {
        let selected = named(name);
        if is_file(&selected) { return selected; }
    }
    named("src")
}

/// The program `makepad app <id>` runs, named as the run names its own executable (relax-compiler
/// selfexe.rs program_name): `--bin`'s value, else its package's last part, from the launch
/// record's `run` words (tools/run/runner/src/launch.rs). None for another command or a name that
/// is no file name.
#[cfg(any(unix, test))]
fn program(words: &[String]) -> Option<String> {
    // the loader's options before the package that take a value (relax-compiler run.rs)
    const VALUE: [&str; 11] = ["--units", "--profile", "--harness-restart", "--subject", "--report-to", "--entry", "--cfg", "--exports", "--run-dir", "--workspace-dir", "--boundary"];
    let mut it = words.iter();
    if it.next()? != "run" {
        return None;
    }
    let package = loop {
        let w = it.next()?;
        if !w.starts_with("--") {
            break w;
        }
        if VALUE.contains(&w.as_str()) {
            it.next();
        }
    };
    let flags: Vec<&String> = it.take_while(|w| *w != "--").collect();
    let name = flags.iter().position(|f| *f == "--bin").and_then(|i| flags.get(i + 1)).map(|s| s.to_string())
        .or_else(|| flags.iter().find_map(|f| f.strip_prefix("--bin=").map(str::to_string)))
        .or_else(|| Path::new(package).file_stem().map(|s| s.to_string_lossy().into_owned()))?;
    (!name.is_empty() && !name.starts_with('.') && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))).then_some(name)
}

/// The program of the launch record `<home>/apps/<id>.launch` (its `arg` lines).
#[cfg(unix)]
fn program_of(home: &Path, id: &str) -> Option<String> {
    let text = std::fs::read_to_string(home.join("apps").join(format!("{id}.launch"))).ok()?;
    let words: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("arg\t")).map(str::to_string).collect();
    program(&words)
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino())
}

/// `<dir>/<name>`, a hard link of `loader` (made anew when it is another file: the active loader
/// changed). The error names the path and the OS error (another volume, no permission).
#[cfg(unix)]
fn link_as(dir: &Path, loader: &Path, name: &str) -> Result<PathBuf, String> {
    let link = dir.join(name);
    if !same_file(&link, loader) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let part = dir.join(format!(".{name}-{}.part", std::process::id()));
        let _ = std::fs::remove_file(&part);
        std::fs::hard_link(loader, &part).map_err(|e| format!("{}: a hard link of {}: {e}", link.display(), loader.display()))?;
        if let Err(e) = std::fs::rename(&part, &link) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{}: {e}", link.display()));
        }
    }
    Ok(link)
}

/// Why this start runs under the loader's name: one line in `<home>/cache/launch/launcher.log`,
/// and on stderr (which a macOS bundle's start shows no one).
#[cfg(unix)]
fn note(home: &Path, why: &str) {
    use std::io::Write;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let line = format!("makepad-launcher: {}\n", why.replace(['\n', '\r'], " "));
    let _ = std::io::stderr().write_all(line.as_bytes());
    let dir = home.join("cache/launch");
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("launcher.log")) {
            let _ = f.write_all(format!("{at} {line}").as_bytes());
        }
    }
}

/// The start of `loader` named as the app's program in `dir` (the launch record's `run`): the
/// link, else None, with why noted when a link was due but could not be made.
#[cfg(unix)]
fn named_start(home: &Path, id: &str, dir: &Path, loader: &Path, launcher: &Path) -> Option<PathBuf> {
    let name = program_of(home, id)?;
    let at = dir.join(&name);
    let made = if at.as_path() == launcher || same_file(&at, launcher) {
        Err(format!("{}: is the launcher itself", at.display()))
    } else {
        link_as(dir, loader, &name)
    };
    made.map_err(|why| note(home, &format!("{why}; app {id} runs under the loader's name"))).ok()
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
    // the app's program: a hard link in the bundle (never the launcher itself), else the
    // `makepad` symlink below
    if let Some(named) = named_start(&home, &id, macos, &loader, exe) {
        let error = Command::new(&named).arg0(&named).arg("app").arg(&id).env("RELAX_HOME", &home).env("RELAX_LOADER", &loader).exec();
        return Err(format!("{}: {error}", named.display()));
    }
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
    let error = Command::new(&link).arg0(&link).arg("app").arg(id).env("RELAX_HOME", home).exec();
    Err(format!("{}: {error}", link.display()))
}

#[cfg(not(target_os = "macos"))]
fn launch(exe: &Path) -> Result<(), String> {
    let home = exe.parent().and_then(Path::parent).ok_or("the launcher is not inside <home>/apps")?;
    let mut args = std::env::args_os().skip(1);
    let id = args.next().and_then(|id| id.into_string().ok()).filter(|id| valid_id(id)).ok_or("usage: makepad-launcher <app-id>")?;
    if args.next().is_some() { return Err("usage: makepad-launcher <app-id>".into()); }
    let loader = active(home);
    // the app's program: a hard link in a folder of its own per app, `+launch` as the runner's
    #[cfg(unix)]
    let start = named_start(home, &id, &home.join("cache/launch").join(&id).join("+launch"), &loader, exe).unwrap_or_else(|| loader.clone());
    #[cfg(not(unix))]
    let start = loader.clone();
    let mut command = Command::new(&start);
    command.arg("app").arg(id).env("RELAX_HOME", home).env("RELAX_LOADER", &loader);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(format!("{}: {}", start.display(), command.exec()))
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
    fn program_names() {
        let program = |words: &[&str]| program(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>());
        assert_eq!(program(&["run", "makepad-app-calculator"]).as_deref(), Some("makepad-app-calculator"));
        assert_eq!(program(&["run", "--report-to", "/r", "--no-live", "apps/demo", "--release", "--bin", "demo-bin", "--", "--bin", "x"]).as_deref(), Some("demo-bin"), "the loader's options and the app's arguments are no part");
        assert_eq!(program(&["run", "https://example.com/org/robrix", "--bin=robrix-app"]).as_deref(), Some("robrix-app"));
        assert_eq!(program(&["run", "https://example.com/org/robrix"]).as_deref(), Some("robrix"));
        assert_eq!(program(&["test", "x"]), None, "only a run is a program");
        assert_eq!(program(&["run", "--report-to"]), None, "no package");
    }

    #[test]
    fn plist_values() {
        let plist = "<dict><key>MakepadApp</key>\n<string>demo</string><key>MakepadRunHome</key><string>/a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos; &amp;lt;</string><key>NotAString</key><true/></dict>";
        assert_eq!(plist_string(plist, "MakepadApp").as_deref(), Some("demo"));
        assert_eq!(plist_string(plist, "MakepadRunHome").as_deref(), Some("/a & <b> \"c\" 'd' &lt;"));
        assert_eq!(plist_string(plist, "Missing"), None);
        assert_eq!(plist_string(plist, "NotAString"), None);
        assert_eq!(plist_string("<key>MakepadApp</key><string>unfinished", "MakepadApp"), None);
    }
}
