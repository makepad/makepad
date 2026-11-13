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
//! and its process after it: Linux app_id / WM_CLASS `nl.makepad.<id>`) and RELAX_HOME, and
//! runs the words as if they were its own command line. `makepad app --home <dir> <id>` reads the
//! record of that home and runs with RELAX_HOME=<dir>: what a start that cannot set the
//! environment passes (a Windows shortcut). The runner's pinned entries start the loader itself: a
//! macOS bundle whose executable is a symlink to it (its Info.plist's LSEnvironment sets
//! RELAX_APP and RELAX_HOME), a Linux .desktop file (`env RELAX_HOME=<home>`)
//! with `app <id>` or a Windows shortcut with `app --home <home> <id>`, so every app shares the
//! loader and its warm JIT cache.
//!
//! `makepad` with no arguments: the app RELAX_APP names, otherwise the launch record
//! `runner` (the entry scripts write it), then the host's default (the `makepad` binary: the
//! runner of the checkout it was built from).
//!
//! A launcher that wants to follow a start sets RELAX_REPORT_TO=<file> in the environment
//! of the launch (macOS `open --env`): a `run` then reports to it as with `--report-to`, after a
//! first line `pid <pid>` that names the process.
use std::path::{Path, PathBuf};

/// An app id: letters, digits, `-` and `_` (a file name and an identifier part on every OS).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One launch record (see the module doc).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub id: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
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

/// `[exe, "app", ["--home", <dir>,] <id>, extra...]`: the home it names, the id and where the
/// extra words start.
fn app_words(raw: &[Vec<u8>]) -> Result<(Option<PathBuf>, String, usize), String> {
    let usage = || "usage: makepad app [--home <dir>] <id>".to_string();
    let (home, at) = match raw.get(2).map(Vec::as_slice) {
        Some(b"--home") => (Some(raw.get(3).filter(|d| !d.is_empty()).ok_or_else(usage)?), 4),
        _ => (None, 2),
    };
    let id = raw.get(at).ok_or_else(usage)?;
    let os = |b: &Vec<u8>| PathBuf::from(String::from_utf8_lossy(b).into_owned());
    Ok((home.map(os), String::from_utf8_lossy(id).into_owned(), at + 1))
}

/// Resolves `[exe, "app", [--home <dir>,] <id>, extra...]` or no arguments. No arguments run
/// RELAX_APP when it is set (and take it out of the environment), otherwise `runner` or the
/// host's `default`. None leaves the command line unchanged. The launch's environment and folder
/// are applied here.
pub fn resolve(raw: &[Vec<u8>], default: Option<fn() -> Option<Launch>>) -> Option<Result<Vec<Vec<u8>>, String>> {
    let exe = raw.first().cloned().unwrap_or_default();
    let mut extra = raw.len();
    let launch = match raw.get(1).map(Vec::as_slice) {
        None => {
            // a macOS bundle's: Launch Services sets it (with RELAX_HOME) from the bundle's
            // LSEnvironment; the app's own children must not inherit it
            let bundle = std::env::var("RELAX_APP").ok();
            std::env::remove_var("RELAX_APP");
            if let Some(id) = bundle {
                if !valid_id(&id) {
                    return Some(Err(format!("RELAX_APP: not an app id: {id}")));
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
        Some(b"app") => app_words(raw).and_then(|(home, id, at)| {
            // the run and every program it starts keep their records in this home
            if let Some(home) = home {
                std::env::set_var("RELAX_HOME", home);
            }
            extra = at;
            read(&crate::fetch::home(), &id)
        }),
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
        out.extend(raw[extra..].iter().cloned());
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
    // a launcher following the start (RELAX_REPORT_TO): the run reports to that file
    if let Some(report) = std::env::var_os("RELAX_REPORT_TO") {
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
            env: vec![("RELAX_EDITS".into(), "/e/calc".into())],
            args: vec!["run".into(), "--quiet".into(), "makepad-app-calculator".into(), "--features".into(), "standalone".into()],
        };
        assert_eq!(parse("calculator", &format(&l)).unwrap(), l);
    }

    #[test]
    fn app_words_name_a_home() {
        let w = |s: &[&str]| s.iter().map(|a| a.as_bytes().to_vec()).collect::<Vec<_>>();
        assert_eq!(app_words(&w(&["makepad", "app", "calculator"])).unwrap(), (None, "calculator".into(), 3));
        let (home, id, at) = app_words(&w(&["makepad", "app", "--home", "C:\\Users\\A B\\makepad-runner", "runner", "--", "x"])).unwrap();
        assert_eq!((home, id.as_str(), at), (Some(PathBuf::from("C:\\Users\\A B\\makepad-runner")), "runner", 5));
        assert!(app_words(&w(&["makepad", "app"])).is_err());
        assert!(app_words(&w(&["makepad", "app", "--home", "/h"])).is_err(), "a home without an id");
        assert!(app_words(&w(&["makepad", "app", "--home", "", "runner"])).is_err(), "an empty home");
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
}
