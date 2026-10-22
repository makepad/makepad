//! Launch records: what `makepad app <id>` runs, and what `makepad` with no arguments runs.
//!
//! A launcher (the runner app) keeps one small text file per app it can start,
//! `<home>/apps/<id>.launch`, one `key<TAB>value` line each:
//!
//! ```text
//! title   Calculator              the app's name (the OS identity shows it)
//! cwd     /path/to/workspace      the folder the command runs in (a checkout run); optional
//! env     NAME=value              an environment variable for the run; repeatable
//! arg     run                     the command line after the binary, one word per line
//! ```
//!
//! `makepad app <id>` reads that file, sets MAKEPAD_APP_ID=<id> (the platform names its windows
//! and its process after it: Linux app_id / WM_CLASS `nl.makepad.<id>`) and MAKEPAD_RUN_HOME, and
//! runs the words as if they were its own command line. The per-app OS entries the launcher
//! writes (a macOS bundle, a Windows Start menu shortcut, a Linux .desktop file) all start this
//! same binary that way, so every app runs the same bytes and shares the warm JIT cache.
//!
//! `makepad` with no arguments: inside a macOS app bundle whose Info.plist names `MakepadApp`
//! (and `MakepadRunHome`), that app; otherwise the launch record `runner` (the entry scripts
//! write it), otherwise the host's default (the `makepad` binary: the runner of the checkout it
//! was built from).
//!
//! A launcher that wants to follow a start sets MAKEPAD_RUN_REPORT_TO=<file> in the environment
//! of the launch (macOS `open --env`): a `run` then reports to it as with `--report-to`, after a
//! first line `pid <pid>` that names the process.
use std::path::{Path, PathBuf};

/// One launch record (see the module doc).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub id: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
}

/// An app id: letters, digits, `-` and `_` (a file name and an identifier part on every OS).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `<home>/apps`
pub fn dir(home: &Path) -> PathBuf {
    home.join("apps")
}

pub fn path(home: &Path, id: &str) -> PathBuf {
    dir(home).join(format!("{id}.launch"))
}

/// Parses a launch record's text.
pub fn parse(id: &str, text: &str) -> Result<Launch, String> {
    let mut l = Launch { id: id.to_string(), ..Launch::default() };
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('\t') else {
            return Err(format!("line {}: expected `key<TAB>value`", n + 1));
        };
        match key {
            "title" => l.title = value.to_string(),
            "cwd" => l.cwd = Some(PathBuf::from(value)),
            "env" => match value.split_once('=') {
                Some((k, v)) if !k.is_empty() => l.env.push((k.to_string(), v.to_string())),
                _ => return Err(format!("line {}: env wants NAME=value", n + 1)),
            },
            "arg" => l.args.push(value.to_string()),
            _ => return Err(format!("line {}: unknown key `{key}`", n + 1)),
        }
    }
    if l.args.is_empty() {
        return Err("no command (`arg` lines)".to_string());
    }
    Ok(l)
}

/// The text of a launch record (what `parse` reads back).
pub fn format(l: &Launch) -> String {
    let mut s = String::new();
    if !l.title.is_empty() {
        s.push_str(&format!("title\t{}\n", one_line(&l.title)));
    }
    if let Some(cwd) = &l.cwd {
        s.push_str(&format!("cwd\t{}\n", one_line(&cwd.to_string_lossy())));
    }
    for (k, v) in &l.env {
        s.push_str(&format!("env\t{}={}\n", one_line(k), one_line(v)));
    }
    for a in &l.args {
        s.push_str(&format!("arg\t{}\n", one_line(a)));
    }
    s
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

/// Reads `<home>/apps/<id>.launch`.
pub fn read(home: &Path, id: &str) -> Result<Launch, String> {
    if !valid_id(id) {
        return Err(format!("`{id}` is not an app id"));
    }
    let p = path(home, id);
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    parse(id, &text).map_err(|e| format!("{}: {e}", p.display()))
}

/// Writes `<home>/apps/<id>.launch` (beside it, then renamed: a reader never sees half a file).
pub fn write(home: &Path, l: &Launch) -> Result<(), String> {
    if !valid_id(&l.id) {
        return Err(format!("`{}` is not an app id", l.id));
    }
    let d = dir(home);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let p = path(home, &l.id);
    let part = d.join(format!("{}.launch.part", l.id));
    std::fs::write(&part, format(l)).and_then(|_| std::fs::rename(&part, &p)).map_err(|e| format!("{}: {e}", p.display()))
}

/// The value of `<key>NAME</key><string>VALUE</string>` in an Info.plist's text.
pub fn plist_string(plist: &str, name: &str) -> Option<String> {
    let key = format!("<key>{name}</key>");
    let rest = &plist[plist.find(&key)? + key.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("<string>")?;
    let value = &rest[..rest.find("</string>")?];
    Some(value.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}

/// macOS: the app this executable's bundle names (`MakepadApp`) and the home it keeps its launch
/// records in (`MakepadRunHome`), when the executable sits in `<X>.app/Contents/MacOS/`.
fn bundle_app(exe: &Path) -> Option<(String, Option<PathBuf>)> {
    let contents = exe.parent()?.parent()?;
    if exe.parent()?.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    let plist = std::fs::read_to_string(contents.join("Info.plist")).ok()?;
    let id = plist_string(&plist, "MakepadApp")?;
    Some((id, plist_string(&plist, "MakepadRunHome").map(PathBuf::from)))
}

/// What a command line becomes when it names a launch record: `[exe]` (no arguments) or
/// `[exe, "app", <id>, extra...]`. `default` is the host's own no-argument launch. None: the
/// command line stays as it is. The launch's environment and folder are applied here.
pub fn resolve(raw: &[Vec<u8>], default: Option<fn() -> Option<Launch>>) -> Option<Result<Vec<Vec<u8>>, String>> {
    let exe = raw.first().cloned().unwrap_or_default();
    let launch = match raw.get(1).map(Vec::as_slice) {
        None => {
            let here = std::env::current_exe().ok();
            if let Some((id, home)) = here.as_deref().and_then(bundle_app) {
                if let Some(h) = home {
                    std::env::set_var("MAKEPAD_RUN_HOME", h);
                }
                read(&crate::fetch::home(), &id)
            } else if path(&crate::fetch::home(), "runner").is_file() {
                read(&crate::fetch::home(), "runner")
            } else {
                match default.and_then(|f| f()) {
                    Some(l) => Ok(l),
                    None => return None,
                }
            }
        }
        Some(b"app") => match raw.get(2) {
            Some(id) => read(&crate::fetch::home(), &String::from_utf8_lossy(id)),
            None => Err("usage: makepad app <id>".to_string()),
        },
        Some(_) => return None,
    };
    let launch = match launch {
        Ok(l) => l,
        Err(e) => return Some(Err(e)),
    };
    Some(apply(&launch).map(|mut words| {
        let mut out = vec![exe];
        out.append(&mut words);
        // `makepad app <id> -- <program args>`: after the record's own words
        if raw.get(1).map(Vec::as_slice) == Some(b"app") && raw.len() > 3 {
            out.extend(raw[3..].iter().cloned());
        }
        out
    }))
}

/// Applies a launch's identity, environment and folder to this process; returns its words.
fn apply(l: &Launch) -> Result<Vec<Vec<u8>>, String> {
    std::env::set_var("MAKEPAD_APP_ID", &l.id);
    if !l.title.is_empty() {
        std::env::set_var("MAKEPAD_APP_TITLE", &l.title);
    }
    // the .ico the runner made for the app's Start menu entry: its windows' relaunch icon
    #[cfg(windows)]
    {
        let ico = dir(&crate::fetch::home()).join(format!("{}.ico", l.id));
        if ico.is_file() {
            std::env::set_var("MAKEPAD_APP_ICON", ico);
        }
    }
    for (k, v) in &l.env {
        std::env::set_var(k, v);
    }
    if let Some(cwd) = &l.cwd {
        std::env::set_current_dir(cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    }
    let mut words: Vec<Vec<u8>> = l.args.iter().map(|a| a.as_bytes().to_vec()).collect();
    // a launcher following the start (MAKEPAD_RUN_REPORT_TO): the run reports to that file
    if let Some(report) = std::env::var_os("MAKEPAD_RUN_REPORT_TO") {
        if words.first().map(Vec::as_slice) == Some(b"run") && !words.iter().any(|w| w == b"--report-to") {
            let report = report.to_string_lossy().into_owned();
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&report) {
                let _ = f.write_all(format!("pid\t{}\t0\n", std::process::id()).as_bytes());
            }
            words.insert(1, b"--report-to".to_vec());
            words.insert(2, report.into_bytes());
        }
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let l = Launch {
            id: "calculator".into(),
            title: "Calculator".into(),
            cwd: Some(PathBuf::from("/w/makepad")),
            env: vec![("MAKEPAD_RUN_EDITS".into(), "/e/calc".into())],
            args: vec!["run".into(), "--quiet".into(), "makepad-app-calculator".into(), "--features".into(), "standalone".into()],
        };
        assert_eq!(parse("calculator", &format(&l)).unwrap(), l);
    }

    #[test]
    fn refuses() {
        assert!(parse("x", "title\tX\n").is_err());
        assert!(parse("x", "arg\trun\nbogus\tv\n").is_err());
        assert!(parse("x", "env\tnovalue\narg\trun\n").is_err());
        assert!(!valid_id("../etc"));
        assert!(!valid_id(""));
        assert!(valid_id("stage_2-x"));
    }

    #[test]
    fn plist() {
        let p = "<dict>\n  <key>MakepadApp</key>\n  <string>scope</string>\n  <key>MakepadRunHome</key><string>/a &amp; b</string>\n</dict>";
        assert_eq!(plist_string(p, "MakepadApp").as_deref(), Some("scope"));
        assert_eq!(plist_string(p, "MakepadRunHome").as_deref(), Some("/a & b"));
        assert_eq!(plist_string(p, "Missing"), None);
    }
}
