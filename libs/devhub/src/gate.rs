//! The land rule's hook. The rule is a private binary beside devhub's own (`target/host/` of the
//! checkout), built with devhub by the master's self-update from the private checkout
//! ([`CRATE_DIR`]). devhub knows only this contract with it:
//! - `<bin> land-rule check|land --branch <branch> --makepad <dir> <from> <merge>
//!   [--commercial <dir> <from> <merge>]`: exit 0, the landing may land (stdout: notes to say, a
//!   line each); exit 1, it may not (stdout: why, a line each). `check` asks while the queue
//!   prepares the landing, `land` at the fast-forward itself (the rule keeps its own record).
//!   Both get the last landing's ledger witness in `DEVHUB_LAND_WITNESS` (`-`: none yet), and a
//!   `land` that lands prints the new one on a line `witness\t<text>` ([`witness_of`]): the master
//!   keeps it in that landing's entry and hands it to the next run, so the rule refuses when its
//!   ledgers lost or changed what they held. An older binary ignores the variable.
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
pub fn land_rule(repo: &Path, verb: Verb, branch: &str, sides: &[Side], witness: &str, before: &str) -> Result<Vec<String>, No> {
    run_rule(&bin(repo), present(repo), repo, verb, branch, sides, witness, before)
}

/// The variable the rule gets the last landing's witness in, and the line it prints a new one on.
pub const WITNESS_ENV: &str = "DEVHUB_LAND_WITNESS";
/// With no witness (the rule switched off and on), the last landed entry's, for the rule to say
/// what the ledgers lost since.
pub const WITNESS_LAST_ENV: &str = "DEVHUB_LAND_WITNESS_LAST";
const WITNESS_LINE: &str = "witness\t";

/// A `land`'s lines split into its notes and the witness it printed (None: an older binary, or
/// none it could take).
pub fn witness_of(lines: Vec<String>) -> (Vec<String>, Option<String>) {
    let mut witness = None;
    let notes = lines.into_iter().filter(|l| match l.strip_prefix(WITNESS_LINE) {
        Some(w) if !w.trim().is_empty() => {
            witness = Some(w.trim().to_string());
            false
        }
        _ => true,
    });
    (notes.collect(), witness)
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

#[allow(clippy::too_many_arguments)]
fn run_rule(bin: &Path, present: bool, repo: &Path, verb: Verb, branch: &str, sides: &[Side], witness: &str, before: &str) -> Result<Vec<String>, No> {
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
    cmd.env(WITNESS_ENV, if witness.is_empty() { "-" } else { witness });
    if witness.is_empty() && !before.is_empty() {
        cmd.env(WITNESS_LAST_ENV, before);
    } else {
        cmd.env_remove(WITNESS_LAST_ENV);
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

/// The binary as a review card's gate ([`crate::ask::Gate`]): `<bin> card <items>` gives the card's
/// lines of items in doubt, a JSON object a line; `<bin> decide` takes a press the master holds,
/// its nonce on stdin and nowhere else, and answers one JSON line. Both run like a tick: in the
/// checkout, with no terminal.
pub struct Binary {
    repo: PathBuf,
}

impl Binary {
    pub fn new(repo: PathBuf) -> Self {
        Self { repo }
    }

    fn run(&self, words: &[&str], stdin: Option<&str>) -> Result<Vec<String>, String> {
        run_lines(&bin(&self.repo), &self.repo, words, stdin)
    }
}

fn run_lines(bin: &Path, repo: &Path, words: &[&str], stdin: Option<&str>) -> Result<Vec<String>, String> {
    use std::io::Write;
    if !bin.is_file() {
        return Err(format!("{} is not built (the master builds it with devhub)", bin.display()));
    }
    let mut child = Command::new(bin)
        .args(words)
        .current_dir(repo)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} did not start: {e}", bin.display()))?;
    // a binary may end without reading its stdin: what it said and how it ended are its answer,
    // whichever of the two comes first, so a pipe it closed is no error of its own
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        match pipe.write_all(format!("{text}\n").as_bytes()) {
            Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{}: its stdin: {e}", bin.display()));
            }
            _ => {}
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("{}: {e}", bin.display()))?;
    let said = lines(&out.stdout);
    if said.is_empty() {
        return Err(format!("{} {} said nothing ({}): {}", bin.display(), words.first().unwrap_or(&""), out.status, lines(&out.stderr).pop().unwrap_or_default()));
    }
    Ok(said)
}

fn json_lines(lines: Vec<String>) -> Result<Vec<makepad_strict_json::Value>, String> {
    lines.iter().map(|l| makepad_strict_json::parse(l.as_bytes()).map_err(|e| format!("the gate's line is no JSON: {e}"))).collect()
}

impl crate::ask::Gate for Binary {
    fn card(&self, tags: &[String]) -> Result<Vec<makepad_strict_json::Value>, String> {
        let mut words = vec!["card"];
        words.extend(tags.iter().map(String::as_str));
        json_lines(self.run(&words, None)?)
    }

    fn decide(&self, nonce: &str) -> Result<makepad_strict_json::Value, String> {
        json_lines(self.run(&["decide"], Some(nonce))?)?.pop().ok_or_else(|| "the gate's decide said nothing".to_string())
    }
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

    /// A script at `dir/rule`, written by a `sh` of its own: a file this process held open for
    /// writing is refused as busy (ETXTBSY) when run while a child that another test forks still
    /// has that descriptor, before its exec closes it. A spawn so refused failed the review gate's
    /// test in the fast bucket once (11-19).
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::io::Write;
        let p = dir.join("rule");
        let mut sh = Command::new("sh").args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"]).arg(&p).stdin(Stdio::piped()).spawn().unwrap();
        sh.stdin.take().unwrap().write_all(format!("#!/bin/sh\n{body}\n").as_bytes()).unwrap();
        assert!(sh.wait().unwrap().success(), "the script {} is written", p.display());
        p
    }

    #[test]
    fn the_rule_fails_closed_and_passes_its_answers_through() {
        let d = std::env::temp_dir().join(format!("devhub-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let side = [Side { name: "makepad", dir: &d, from: "a", merge: "b" }];
        let ask = |bin: &Path| run_rule(bin, true, &d, Verb::Land, "rc-x", &side, "", "");
        let refused = |bin: &Path| match run_rule(bin, true, &d, Verb::Land, "rc-x", &side, "", "") {
            Err(No::Refused(why)) => why,
            other => panic!("refused, not {other:?}"),
        };
        assert!(matches!(ask(&d.join("none")), Err(No::Wait(_))), "not built yet with the private checkout: the landing waits");
        let missing = match run_rule(&d.join("none"), false, &d, Verb::Check, "rc-x", &side, "", "") {
            Err(No::Refused(why)) => why,
            other => panic!("refused, not {other:?}"),
        };
        assert!(missing[0].contains("missing"), "without the private checkout a missing binary refuses: {missing:?}");
        // the binary sees the contract's words
        let echo = script(&d, "echo \"$*\"; exit 0");
        assert_eq!(ask(&echo), Ok(vec![format!("land-rule land --branch rc-x --makepad {} a b", d.display())]), "exit 0 lands, its lines are notes");
        // the last landing's witness goes in the environment (`-`: none yet), and a land's new one
        // comes back on its own line, apart from the notes
        let witnessed = script(&d, "echo \"saw $DEVHUB_LAND_WITNESS ${DEVHUB_LAND_WITNESS_LAST:-none}\"; printf 'witness\\tports.jsonl=2:ab\\n'; exit 0");
        let said = run_rule(&witnessed, true, &d, Verb::Land, "rc-x", &side, "ports.jsonl=1:aa", "ports.jsonl=0:aa").unwrap();
        assert_eq!(witness_of(said), (vec!["saw ports.jsonl=1:aa none".to_string()], Some("ports.jsonl=2:ab".to_string())), "the witness in, and out; the last entry's only without one");
        assert_eq!(run_rule(&witnessed, true, &d, Verb::Check, "rc-x", &side, "", "").map(|l| l[0].clone()), Ok("saw - none".to_string()), "none yet");
        assert_eq!(run_rule(&witnessed, true, &d, Verb::Check, "rc-x", &side, "", "ports.jsonl=0:aa").map(|l| l[0].clone()), Ok("saw - ports.jsonl=0:aa".to_string()), "after an off and on, the last landed entry's"); 
        assert_eq!(witness_of(vec!["a note".into()]), (vec!["a note".to_string()], None), "an older binary prints none");
        let refuse = script(&d, "echo 'it changes protected paths'; echo 'and more'; exit 1");
        assert_eq!(refused(&refuse), vec!["it changes protected paths".to_string(), "and more".to_string()], "exit 1 refuses with its lines");
        let silent = script(&d, "exit 1");
        assert!(refused(&silent)[0].contains("failed (exit 1)"), "exit 1 without a reason still refuses");
        let crash = script(&d, "echo 'ok'; echo boom >&2; exit 101");
        let why = refused(&crash);
        assert!(why[0].contains("exit 101") && why[0].contains("boom"), "any other exit refuses: {why:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The review card's gate: `card` gives its JSON lines, and `decide` gets the press's nonce on
    /// its stdin, never as an argument; a gate that says nothing is an error.
    #[test]
    fn the_review_gate_gives_card_lines_and_decides_with_the_nonce_on_stdin() {
        let d = std::env::temp_dir().join(format!("devhub-gate-card-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let bin = script(&d, r#"case "$1" in
card) shift; for t in "$@"; do printf '{"skip":"%s","why":"not in doubt"}\n' "$t"; done;;
decide) read n; [ "$#" -eq 1 ] && printf '{"ok":"cleared","row":"%s"}\n' "$n";;
esac"#);
        let card = json_lines(run_lines(&bin, &d, &["card", "relax#1", "relax#2"], None).unwrap()).unwrap();
        assert_eq!(card.iter().filter_map(|v| v.get("skip").and_then(|s| s.as_str())).collect::<Vec<_>>(), ["relax#1", "relax#2"], "a line per item named");
        assert_eq!(run_lines(&bin, &d, &["decide"], Some("ab12")), Ok(vec![r#"{"ok":"cleared","row":"ab12"}"#.to_string()]), "the nonce reaches decide on its stdin alone");
        let silent = script(&d, "echo boom >&2; exit 1");
        let said = run_lines(&silent, &d, &["decide"], Some("ab12"));
        assert!(said.as_ref().is_err_and(|e| e.contains("said nothing") && e.contains("boom")), "a gate that says nothing is an error: {said:?}");
        // a gate that ends without reading its stdin answers by what it said and how it ended,
        // whether it ends before the nonce is written or after: a stdin past what a pipe holds is
        // written only once it ended, every time
        let long = "n".repeat(1 << 20);
        let said = run_lines(&silent, &d, &["decide"], Some(&long));
        assert!(said.as_ref().is_err_and(|e| e.contains("said nothing") && e.contains("boom")), "its closed stdin is no error of its own: {said:?}");
        let unread = script(&d, r#"printf '{"ok":"cleared"}\n'"#);
        assert_eq!(run_lines(&unread, &d, &["decide"], Some(&long)), Ok(vec![r#"{"ok":"cleared"}"#.to_string()]), "and what it said is its answer");
        assert!(run_lines(&d.join("none"), &d, &["card"], None).is_err(), "a missing binary is an error");
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
