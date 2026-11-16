//! The land rule's hook. The rule is a private binary beside devhub's own (`target/host/` of the
//! checkout), built with devhub by the master's self-update from the private checkout
//! ([`CRATE_DIR`]). devhub knows only this contract with it:
//! - `<bin> land-rule check|land --branch <branch> --makepad <dir> <from> <merge>
//!   [--commercial <dir> <from> <merge>]`: exit 0, the landing may land (stdout: notes to say, a
//!   line each); exit 1, it may not (stdout: why, a line each). `check` asks while the queue
//!   prepares the landing, `land` at the fast-forward itself (the rule keeps its own record).
//! - `<bin> tick`: one round of the master's periodic work. Each stdout line
//!   `<kind>\t<info|warn|err>\t<text>` is said on the master's events; a line `wait <seconds>`
//!   says when to ask again.
//! - `makepad devhub intake <words>` is `<bin> <words>`, with the caller's terminal.
//!
//! It fails closed: a missing binary, one that does not start, and any other exit refuse the
//! landing, so a checkout without the private one lands nothing. With the private checkout, a
//! binary not built yet holds the landing until the master has built it.
//!
//! `devhub land rule off <why>` switches it off ([`crate::state::Doc::land_rule_off`], kept across
//! restarts) until `devhub land rule on`: no landing runs it (each says "land rule off (<why>):
//! nothing checked", [`crate::land`]), the master builds no binary of it ([`crate::orch`]'s
//! self-update) and its ticks rest.

use crate::proto::{Record, Sev};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

/// The binary's name, in `target/host/` of the checkout.
pub const BIN: &str = "makepad-intake";
/// Its package in the checkout: present only with the private checkout.
pub const CRATE_DIR: &str = "apps/commercial/intake";
/// `makepad devhub <WORD> ...` runs the binary.
pub const WORD: &str = "intake";

/// The binary the rule runs as (swapped in with devhub's own CLI, [`crate::orch`]'s self-update).
pub fn bin(repo: &Path) -> PathBuf {
    repo.join("target/host").join(format!("{BIN}{}", std::env::consts::EXE_SUFFIX))
}

/// Whether the checkout holds the binary's package.
pub fn present(repo: &Path) -> bool {
    repo.join(CRATE_DIR).join("Cargo.toml").is_file()
}

/// One repository of a landing: `name` (`makepad` or `commercial`), its checkout, the branch's tip
/// before the landing and the landing's merge.
pub struct Side<'a> {
    pub name: &'a str,
    pub dir: &'a Path,
    pub from: &'a str,
    pub merge: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Check,
    Land,
}

/// Why a landing does not land now.
#[derive(Debug, PartialEq)]
pub enum No {
    /// the binary is not built yet, and the master builds it: the landing waits
    Wait(String),
    /// the rule refused it, or could not answer: why, a line each
    Refused(Vec<String>),
}

/// The rule's answer for one landing: Ok with the notes to say.
pub fn land_rule(repo: &Path, verb: Verb, branch: &str, sides: &[Side]) -> Result<Vec<String>, No> {
    run_rule(&bin(repo), present(repo), repo, verb, branch, sides)
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn run_rule(bin: &Path, present: bool, repo: &Path, verb: Verb, branch: &str, sides: &[Side]) -> Result<Vec<String>, No> {
    if !bin.is_file() {
        if present {
            return Err(No::Wait(format!("the land rule's binary {} is not built yet: the master builds it", bin.display())));
        }
        return Err(No::Refused(vec![format!("the land rule's binary {} is missing (it builds from the private checkout, which this one lacks): no landing lands without it", bin.display())]));
    }
    let mut cmd = Command::new(bin);
    cmd.args(["land-rule", if verb == Verb::Check { "check" } else { "land" }, "--branch", branch]);
    for s in sides {
        cmd.arg(format!("--{}", s.name)).arg(s.dir).args([s.from, s.merge]);
    }
    let out = match cmd.current_dir(repo).stdin(Stdio::null()).output() {
        Ok(o) => o,
        Err(e) => return Err(No::Refused(vec![format!("the land rule's binary {} did not start: {e}", bin.display())])),
    };
    let said = lines(&out.stdout);
    match out.status.code() {
        Some(0) => Ok(said),
        Some(1) if !said.is_empty() => Err(No::Refused(said)),
        code => {
            let how = code.map_or_else(|| "a signal".to_string(), |c| format!("exit {c}"));
            let last = lines(&out.stderr).pop().unwrap_or_default();
            Err(No::Refused(vec![format!("the land rule failed ({how}): {last}")]))
        }
    }
}

/// The master's periodic work, on one long-lived thread: [`tick`] and the wait it asks for, while
/// `on` (the rule is not switched off; off, it looks again a tick later).
pub fn start_ticks(repo: PathBuf, machine: String, on: Arc<dyn Fn() -> bool + Send + Sync>, publish: Arc<dyn Fn(Record) + Send + Sync>) {
    let _ = std::thread::Builder::new().name("devhub-gate-tick".into()).spawn(move || {
        let mut last = None;
        loop {
            if !on() {
                std::thread::sleep(TICK_DEFAULT);
                continue;
            }
            let said = tick(&bin(&repo), present(&repo), &repo, &mut last);
            for (kind, sev, text) in said.lines {
                publish(crate::proto::hub_record(&machine, "", 0, &kind, sev, text, String::new()));
            }
            std::thread::sleep(said.wait);
        }
    });
}

/// What one tick says: event lines (kind, severity, text) and the wait before the next.
#[derive(Debug, PartialEq)]
pub struct Said {
    pub lines: Vec<(String, Sev, String)>,
    pub wait: Duration,
}

const TICK_DEFAULT: Duration = Duration::from_secs(60);
const TICK_MIN: Duration = Duration::from_secs(10);
const TICK_MAX: Duration = Duration::from_secs(3600);
/// How long a tick that could not run waits (a missing binary, a failed start, a failed exit).
const TICK_RETRY: Duration = Duration::from_secs(300);

/// One tick: the binary's lines, or once per distinct trouble a warning (`last`: the trouble said
/// last). A checkout without the private one ticks quietly.
pub fn tick(bin: &Path, present: bool, repo: &Path, last: &mut Option<String>) -> Said {
    let mut trouble = |why: String| {
        let lines = if last.as_deref() == Some(why.as_str()) { Vec::new() } else { vec![("devhub".to_string(), Sev::Warn, why.clone())] };
        *last = Some(why);
        Said { lines, wait: TICK_RETRY }
    };
    if !bin.is_file() {
        if !present {
            return Said { lines: Vec::new(), wait: TICK_RETRY };
        }
        return trouble(format!("{} is not built yet: the master builds it with devhub", bin.display()));
    }
    let out = match Command::new(bin).arg("tick").current_dir(repo).stdin(Stdio::null()).output() {
        Ok(o) => o,
        Err(e) => return trouble(format!("{} tick did not start: {e}", bin.display())),
    };
    if !out.status.success() {
        return trouble(format!("{} tick failed ({}): {}", bin.display(), out.status, lines(&out.stderr).pop().unwrap_or_default()));
    }
    let mut said = Said { lines: Vec::new(), wait: TICK_DEFAULT };
    for line in lines(&out.stdout) {
        if let Some(secs) = line.strip_prefix("wait ").and_then(|s| s.trim().parse::<u64>().ok()) {
            said.wait = Duration::from_secs(secs).clamp(TICK_MIN, TICK_MAX);
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(kind), Some(sev), Some(text)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let kind_ok = !kind.is_empty() && kind.len() <= 16 && kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let sev = match sev {
            "info" => Sev::Info,
            "warn" => Sev::Warn,
            "err" => Sev::Err,
            _ => continue,
        };
        if kind_ok {
            said.lines.push((kind.to_string(), sev, text.to_string()));
        }
    }
    *last = None;
    said
}

/// `makepad devhub intake <words>`: the binary beside this one, with this terminal.
pub fn forward(words: &[&str]) -> i32 {
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).ok();
    let repo = exe.iter().flat_map(|e| e.ancestors()).find(|d| d.join("libs/devhub/Cargo.toml").is_file()).map(Path::to_path_buf);
    let Some(repo) = repo else {
        eprintln!("makepad devhub {WORD}: this devhub runs from no checkout");
        return 2;
    };
    let bin = bin(&repo);
    if !bin.is_file() {
        let how = if present(&repo) { "the master builds it with devhub" } else { "it builds from the private checkout, which this one lacks" };
        eprintln!("makepad devhub {WORD}: {} is not built ({how})", bin.display());
        return 2;
    }
    let mut command = Command::new(&bin);
    command.args(words);
    #[cfg(unix)]
    let e = std::os::unix::process::CommandExt::exec(&mut command);
    #[cfg(not(unix))]
    let e = match command.status() {
        Ok(status) => return status.code().unwrap_or(1),
        Err(e) => e,
    };
    eprintln!("makepad devhub {WORD}: {}: {e}", bin.display());
    1
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("rule");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_rule_fails_closed_and_passes_its_answers_through() {
        let d = std::env::temp_dir().join(format!("devhub-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let side = [Side { name: "makepad", dir: &d, from: "a", merge: "b" }];
        let ask = |bin: &Path| run_rule(bin, true, &d, Verb::Land, "rc-x", &side);
        let refused = |bin: &Path| match run_rule(bin, true, &d, Verb::Land, "rc-x", &side) {
            Err(No::Refused(why)) => why,
            other => panic!("refused, not {other:?}"),
        };
        assert!(matches!(ask(&d.join("none")), Err(No::Wait(_))), "not built yet with the private checkout: the landing waits");
        let missing = match run_rule(&d.join("none"), false, &d, Verb::Check, "rc-x", &side) {
            Err(No::Refused(why)) => why,
            other => panic!("refused, not {other:?}"),
        };
        assert!(missing[0].contains("missing"), "without the private checkout a missing binary refuses: {missing:?}");
        // the binary sees the contract's words
        let echo = script(&d, "echo \"$*\"; exit 0");
        assert_eq!(ask(&echo), Ok(vec![format!("land-rule land --branch rc-x --makepad {} a b", d.display())]), "exit 0 lands, its lines are notes");
        let refuse = script(&d, "echo 'it changes protected paths'; echo 'and more'; exit 1");
        assert_eq!(refused(&refuse), vec!["it changes protected paths".to_string(), "and more".to_string()], "exit 1 refuses with its lines");
        let silent = script(&d, "exit 1");
        assert!(refused(&silent)[0].contains("failed (exit 1)"), "exit 1 without a reason still refuses");
        let crash = script(&d, "echo 'ok'; echo boom >&2; exit 101");
        let why = refused(&crash);
        assert!(why[0].contains("exit 101") && why[0].contains("boom"), "any other exit refuses: {why:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_tick_says_its_lines_once_per_trouble_and_asks_its_wait() {
        let d = std::env::temp_dir().join(format!("devhub-tick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut last = None;
        assert_eq!(tick(&d.join("none"), false, &d, &mut last), Said { lines: Vec::new(), wait: TICK_RETRY }, "no private checkout: quiet");
        let first = tick(&d.join("none"), true, &d, &mut last);
        assert_eq!((first.lines.len(), first.wait), (1, TICK_RETRY), "a missing binary with the private checkout is said");
        assert!(tick(&d.join("none"), true, &d, &mut last).lines.is_empty(), "once");
        let ok = script(&d, "printf 'pull\\tinfo\\t2 taken\\nBad Kind\\tinfo\\tx\\npull\\tloud\\tx\\nwait 900\\n'");
        assert_eq!(tick(&ok, true, &d, &mut last), Said { lines: vec![("pull".to_string(), Sev::Info, "2 taken".to_string())], wait: Duration::from_secs(900) }, "only well-formed lines, and the wait");
        let fast = script(&d, "echo 'wait 0'");
        assert_eq!(tick(&fast, true, &d, &mut last).wait, TICK_MIN, "the wait is bounded");
        let failing = script(&d, "exit 3");
        assert_eq!(tick(&failing, true, &d, &mut last).lines.len(), 1, "a failing tick is said");
        let _ = std::fs::remove_dir_all(&d);
    }
}
