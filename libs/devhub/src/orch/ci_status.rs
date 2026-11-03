//! The CI status for other programs: `GET /ci/v1/status`, read-only JSON, served by the master from
//! its verify state (was tools/ci's status endpoint on the CI mini; same route, same version-1
//! document, so Scope's CI strip reads it with only its `url` changed).
//!
//! The branch `work` twice: CI's own result first, then verify's, each with its tip (the pair
//! under test, or else its newest judged pair) and its own `last_pass`. Each verify leg is one "script" with one step, whose detail
//! carries the leg's verdict line and the error lines of its log, the way Scope cuts them
//! (`ci_link::error_lines`: from the first `error` line). It listens on loopback only
//! ([`LISTEN`]; `MAKEPAD_DEVHUB_CI_LISTEN` overrides it, `off` turns it off).

use super::Ctx;
use crate::proto::Sev;
use crate::state::{Doc, Leg, Pair};
use makepad_strict_json::{obj, s, Value};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

pub const VERSION: i64 = 1;
pub const LISTEN: &str = "127.0.0.1:7717";
pub const ROUTE: &str = "/ci/v1/status";
/// How much of a leg's log end is searched for error lines.
const LOG_TAIL: u64 = 256 * 1024;
/// Error lines per leg.
const MAX_ERRORS: usize = 40;

pub fn start(ctx: Arc<Ctx>) {
    let address = std::env::var("MAKEPAD_DEVHUB_CI_LISTEN").unwrap_or_else(|_| LISTEN.into());
    if address.is_empty() || address == "off" {
        return;
    }
    let listener = match TcpListener::bind(&address) {
        Ok(l) => l,
        Err(e) => {
            ctx.store.event("verify", Sev::Warn, format!("ci status: {address}: {e}; GET {ROUTE} is not served"), None);
            return;
        }
    };
    let _ = std::thread::Builder::new().name("devhub-ci-status".into()).spawn(move || {
        for mut stream in listener.incoming().flatten() {
            answer(&mut stream, &ctx);
        }
    });
}

fn answer(stream: &mut TcpStream, ctx: &Ctx) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&request);
    let mut words = line.lines().next().unwrap_or("").split_whitespace();
    let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let (status, body) = if method == "GET" && path.split('?').next() == Some(ROUTE) {
        ("200 OK", ctx.store.read(|d| status_value(d, crate::proto::wall_ms() / 1000, &read_tail)).to_json())
    } else {
        ("404 Not Found", obj(vec![("error", s(format!("only GET {ROUTE}")))]).to_json())
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The end of a leg's log (empty when it cannot be read).
fn read_tail(path: &str) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL)));
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn jit_diagnostic(t: &str) -> bool {
    let Some((_, rest)) = t.split_once(".rs:") else { return false };
    let Some((line, rest)) = rest.split_once(':') else { return false };
    let Some((col, rest)) = rest.split_once(':') else { return false };
    let Some((code, _)) = rest.trim_start().strip_prefix('E').and_then(|s| s.split_once(':')) else { return false };
    line.parse::<u64>().is_ok() && col.parse::<u64>().is_ok()
        && !code.is_empty() && code.bytes().all(|b| b.is_ascii_digit())
}

/// The lines of a log that say why it failed: cargo's `error` lines with the few lines under each
/// (the `-->` pointer, the code), and test failures and panics.
pub fn error_lines(log: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut under = 0;
    for line in log.lines() {
        let t = line.trim_start();
        let starts = (t.starts_with("error") && !t.starts_with("error: could not compile") && !t.starts_with("error: aborting"))
            || t.contains("panicked at")
            || (t.starts_with("test ") && t.ends_with(" FAILED"))
            || t.starts_with("CIERR ")
            // cargo's short format: `path:line:col: error[E0659]: ...`
            || t.contains(": error[")
            || t.contains(": error: ")
            // the JIT's short format: path:line:col: E0277: ...
            || jit_diagnostic(t);
        if starts {
            under = 3;
            out.push(crate::proto::clean(line, 300));
        } else if under > 0 && !t.is_empty() {
            under -= 1;
            out.push(crate::proto::clean(line, 300));
        } else {
            under = 0;
        }
        if out.len() >= MAX_ERRORS {
            break;
        }
    }
    out
}

fn script_verdict(leg: &str) -> &'static str {
    match leg {
        "ok" => "green",
        "fail" => "red",
        "run" => "running",
        _ => "waiting",
    }
}

fn step_state(leg: &str) -> &'static str {
    match leg {
        "ok" => "passed",
        "fail" => "failed",
        "run" => "running",
        _ => "waiting",
    }
}

fn script(leg: &Leg, tail: &dyn Fn(&str) -> String) -> Value {
    let errors = match &leg.errors {
        e if !e.is_empty() => e.clone(),
        _ if leg.state == "fail" => error_lines(&tail(&leg.log)),
        _ => Vec::new(),
    };
    let mut detail = leg.summary.clone();
    for e in &errors {
        detail.push('\n');
        detail.push_str(e);
    }
    let where_ = if leg.node.is_empty() { String::new() } else { format!(" on {}", leg.node) };
    let step = obj(vec![
        ("name", s(format!("{}{where_}", leg.name))),
        ("state", s(step_state(&leg.state))),
        ("seconds", Value::Null),
        ("command", s(&leg.lease)),
        ("detail", s(&detail)),
    ]);
    obj(vec![
        ("name", s(&leg.name)),
        ("verdict", s(script_verdict(&leg.state))),
        ("previous", Value::Null),
        ("seconds", Value::Null),
        ("failed_at", Value::Null),
        ("detail", s(&leg.summary)),
        ("steps", Value::Arr(vec![step])),
    ])
}

/// What the status shows: a pair, its verdict, when it finished, why, and its legs (with the error
/// lines a CI shard kept).
struct Shown {
    tip: Pair,
    verdict: &'static str,
    finished: u64,
    detail: String,
    legs: Vec<Leg>,
    at_ms: u64,
}

fn ci_legs(r: &crate::state::CiRun) -> Vec<Leg> {
    r.shards.iter().map(|s| s.leg()).collect()
}

/// CI's own result: its run in progress, else its newest run with a verdict (a newer incomplete
/// one is a note, not a verdict). Verify never stands in for it: its legs never build what CI
/// builds (the Windows JIT; run 129's red was hidden by a verify green within the hour, 11-02).
fn ci_shown(d: &Doc, notes: &mut Vec<String>) -> Option<Shown> {
    let red_if = |legs: &[Leg]| if legs.iter().any(|l| l.state == "fail") { "red" } else { "running" };
    if let Some(r) = &d.ci.run {
        let legs = ci_legs(r);
        return Some(Shown { tip: r.pair.clone(), verdict: red_if(&legs), finished: 0, detail: format!("ci run {} ({} shards)", r.id, r.shards.len()), legs, at_ms: r.started_ms });
    }
    let runs: Vec<_> = d.ci.runs.iter().filter(|r| !r.sweep).collect();
    let judged = runs.iter().rev().find(|r| matches!(r.verdict.as_str(), "ok" | "fail"));
    if let Some(r) = runs.last().filter(|r| judged.is_none_or(|j| j.id != r.id)) {
        notes.push(format!("ci run {} at {} {}", r.id, r.pair.short(), r.verdict));
    }
    judged.map(|r| Shown {
        tip: r.pair.clone(),
        verdict: if r.verdict == "ok" { "green" } else { "red" },
        finished: r.ended_ms / 1000,
        detail: format!("ci run {} {}: {}", r.id, r.verdict, r.critical),
        legs: ci_legs(r),
        at_ms: r.ended_ms,
    })
}

/// Verify's result: its tip run in progress, else the newer of its verified tips and failures.
fn verify_shown(d: &Doc, notes: &mut Vec<String>) -> Option<Shown> {
    let v = &d.verify;
    let red_if = |legs: &[Leg]| if legs.iter().any(|l| l.state == "fail") { "red" } else { "running" };
    if let Some(r) = v.run.as_ref().filter(|r| r.purpose == "tip") {
        let legs = r.legs.clone();
        return Some(Shown { tip: r.pair.clone(), verdict: red_if(&legs), finished: 0, detail: format!("verify run {} (diff {})", r.id, r.diff), legs, at_ms: r.started_ms });
    }
    if let Some(r) = &v.run {
        notes.push(format!("verify {} at {}", r.purpose, r.pair.short()));
    }
    let green = v.verified.iter().max_by_key(|t| t.at_ms).map(|g| Shown { tip: g.pair.clone(), verdict: "green", finished: g.at_ms / 1000, detail: format!("verified (diff {})", g.diff), legs: Vec::new(), at_ms: g.at_ms });
    let red = v.failures.iter().max_by_key(|f| f.at_ms).map(|f| Shown { tip: f.pair.clone(), verdict: "red", finished: f.at_ms / 1000, detail: failure_detail(f), legs: f.legs.clone(), at_ms: f.at_ms });
    green.into_iter().chain(red).max_by_key(|x| x.at_ms)
}

fn branch(source: &str, shown: Option<Shown>, last_pass: Option<&String>, tail: &dyn Fn(&str) -> String) -> Value {
    let shown = shown.unwrap_or(Shown { tip: Pair::default(), verdict: "waiting", finished: 0, detail: format!("no {source} run yet"), legs: Vec::new(), at_ms: 0 });
    obj(vec![
        ("name", s("work")),
        ("source", s(source)),
        ("tip", s(&shown.tip.work)),
        ("commercial", s(&shown.tip.main)),
        ("verdict", s(shown.verdict)),
        ("finished", Value::Int(shown.finished as i64)),
        ("detail", s(&shown.detail)),
        ("last_pass", last_pass.map_or(Value::Null, |w| s(w))),
        ("scripts", Value::Arr(shown.legs.iter().map(|l| script(l, tail)).collect())),
    ])
}

/// The version-1 status document (`now` in seconds; `tail` reads a leg's log): `work` twice, CI's
/// own result first (`"source":"ci"`, what a one-branch reader such as Scope's caption shows), then
/// verify's (`"source":"verify"`). Each has its own last pass: a verify green never replaces a CI
/// red.
pub fn status_value(d: &Doc, now: u64, tail: &dyn Fn(&str) -> String) -> Value {
    let mut notes = Vec::new();
    let ci = ci_shown(d, &mut notes);
    let verify = verify_shown(d, &mut notes);
    let ci_pass = d.ci.runs.iter().filter(|r| !r.sweep && r.verdict == "ok").max_by_key(|r| r.ended_ms).map(|r| &r.pair.work);
    let verify_pass = d.verify.verified.iter().max_by_key(|t| t.at_ms).map(|t| &t.pair.work);
    let branches = vec![branch("ci", ci, ci_pass, tail), branch("verify", verify, verify_pass, tail)];
    obj(vec![("version", Value::Int(VERSION)), ("generated", Value::Int(now as i64)), ("note", s(notes.join("; "))), ("branches", Value::Arr(branches))])
}

fn failure_detail(f: &crate::state::Failure) -> String {
    match &f.blame {
        Some(b) => format!("first bad {}:{} \"{}\"", b.repo, crate::state::short(&b.sha), b.subject),
        None => f.unresolved.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Blame, Failure, Run, Tip};

    fn leg(name: &str, state: &str, log: &str) -> Leg {
        Leg { name: name.into(), state: state.into(), lease: "L1".into(), node: "101".into(), summary: format!("CI-{state}"), errors: Vec::new(), log: log.into(), run_n: 0 }
    }

    fn get<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
        path.iter().fold(v, |v, k| match k.parse::<usize>() {
            Ok(i) => &v.as_arr().unwrap()[i],
            Err(_) => v.get(k).unwrap(),
        })
    }

    #[test]
    fn a_failed_pair_is_red_with_its_legs_and_error_lines_and_the_last_pass() {
        let mut d = Doc::default();
        let a = Pair::new(&"a".repeat(40), &"c".repeat(40));
        let b = Pair::new(&"b".repeat(40), &"c".repeat(40));
        d.verify.verified.push(Tip { pair: a.clone(), at_ms: 1_000, diff: "all".into() });
        d.verify.failures.push(Failure {
            pair: b.clone(),
            at_ms: 5_000,
            blame: Some(Blame { repo: "work".into(), sha: "b".repeat(40), subject: "x".into(), good: a.clone(), bad: b.clone() }),
            unresolved: String::new(),
            legs: vec![leg("ci-cross", "fail", "cross.log"), leg("ci-apple", "ok", "apple.log")],
        });
        let tail = |p: &str| if p == "cross.log" { "Compiling a\nerror[E0425]: cannot find value `x`\n --> src/a.rs:1:1\n  |\nwarning: w\nerror: could not compile `a`\nCI-FAIL wasm32\n".to_string() } else { String::new() };
        let v = status_value(&d, 9, &tail);
        assert_eq!(get(&v, &["version"]).as_i64(), Some(1));
        assert_eq!(get(&v, &["branches", "0", "verdict"]).as_str(), Some("waiting"), "no CI run: CI waits");
        let br = get(&v, &["branches", "1"]);
        assert_eq!(get(br, &["source"]).as_str(), Some("verify"));
        assert_eq!(get(br, &["verdict"]).as_str(), Some("red"));
        assert_eq!(get(br, &["tip"]).as_str(), Some(b.work.as_str()));
        assert_eq!(get(br, &["last_pass"]).as_str(), Some(a.work.as_str()));
        assert_eq!(get(br, &["finished"]).as_i64(), Some(5));
        assert!(get(br, &["detail"]).as_str().unwrap().starts_with("first bad work:bbbbbbbbbb"));
        let cross = get(br, &["scripts", "0"]);
        assert_eq!(get(cross, &["verdict"]).as_str(), Some("red"));
        let detail = get(cross, &["steps", "0", "detail"]).as_str().unwrap();
        assert!(detail.contains("error[E0425]") && detail.contains("--> src/a.rs") && !detail.contains("could not compile"), "{detail}");
        assert_eq!(get(cross, &["steps", "0", "state"]).as_str(), Some("failed"));
        assert_eq!(get(br, &["scripts", "1", "verdict"]).as_str(), Some("green"));
        assert_eq!(error_lines("src/a.rs:1:2: E0308: mismatched types\n"), vec!["src/a.rs:1:2: E0308: mismatched types"]);
    }

    #[test]
    fn a_tip_run_is_running_until_a_leg_fails_and_a_bisect_keeps_the_judged_tip() {
        let mut d = Doc::default();
        assert_eq!(get(&status_value(&d, 1, &|_| String::new()), &["branches", "0", "detail"]).as_str(), Some("no ci run yet"));
        let a = Pair::new(&"a".repeat(40), &"c".repeat(40));
        d.verify.verified.push(Tip { pair: a.clone(), at_ms: 1_000, diff: "affected".into() });
        let t = Pair::new(&"d".repeat(40), &"c".repeat(40));
        d.verify.run = Some(Run { id: "R".into(), pair: t.clone(), started_ms: 1, diff: "affected".into(), dry: false, purpose: "tip".into(), legs: vec![leg("ci-workspace", "run", "")], asked: false, tries: 0 });
        let v = status_value(&d, 1, &|_| String::new());
        assert_eq!(get(&v, &["branches", "1", "verdict"]).as_str(), Some("running"));
        assert_eq!(get(&v, &["branches", "1", "tip"]).as_str(), Some(t.work.as_str()));
        d.verify.run.as_mut().unwrap().legs.push(leg("ci-cross", "fail", ""));
        assert_eq!(get(&status_value(&d, 1, &|_| String::new()), &["branches", "1", "verdict"]).as_str(), Some("red"));
        d.verify.run.as_mut().unwrap().purpose = "bisect 1/3".into();
        let v = status_value(&d, 1, &|_| String::new());
        assert_eq!(get(&v, &["branches", "1", "verdict"]).as_str(), Some("green"));
        assert_eq!(get(&v, &["branches", "1", "tip"]).as_str(), Some(a.work.as_str()));
        assert!(get(&v, &["note"]).as_str().unwrap().starts_with("verify bisect 1/3"));
    }

    #[test]
    fn a_ci_run_shows_its_shards_with_their_kept_errors_and_a_passing_one_is_the_last_pass() {
        use crate::state::{CiRun, CiShard};
        let mut d = Doc::default();
        let p = Pair::new(&"e".repeat(40), &"c".repeat(40));
        let shard = |id: &str, state: &str, errors: Vec<String>| CiShard { id: id.into(), state: state.into(), node: "106".into(), summary: "x".into(), errors, ..Default::default() };
        d.ci.run = Some(CiRun { id: "R".into(), pair: p.clone(), started_ms: 10_000, verdict: "running".into(), shards: vec![shard("smoke:apps/calc", "fail", vec!["error[E1]: boom".into()]), shard("jit:01", "run", vec![])], ..Default::default() });
        let v = status_value(&d, 1, &|_| String::new());
        let br = get(&v, &["branches", "0"]);
        assert_eq!(get(br, &["verdict"]).as_str(), Some("red"));
        assert_eq!(get(br, &["scripts", "0", "name"]).as_str(), Some("smoke:apps/calc"));
        assert!(get(br, &["scripts", "0", "steps", "0", "detail"]).as_str().unwrap().contains("error[E1]: boom"));
        let mut r = d.ci.run.take().unwrap();
        r.verdict = "ok".into();
        r.ended_ms = 20_000;
        d.ci.runs.push(r);
        let v = status_value(&d, 1, &|_| String::new());
        assert_eq!(get(&v, &["branches", "0", "verdict"]).as_str(), Some("green"));
        assert_eq!(get(&v, &["branches", "0", "last_pass"]).as_str(), Some(p.work.as_str()));
    }

    #[test]
    fn a_verify_green_never_replaces_a_ci_red_and_both_show() {
        use crate::state::{CiRun, CiShard};
        let mut d = Doc::default();
        let (old, red, newer) = (Pair::new(&"1".repeat(40), &"c".repeat(40)), Pair::new(&"2".repeat(40), &"c".repeat(40)), Pair::new(&"3".repeat(40), &"c".repeat(40)));
        let shard = |state: &str| CiShard { id: "smoke:windows:apps/x".into(), state: state.into(), node: "105".into(), summary: "x".into(), errors: vec!["E0433: failed to resolve".into()], ..Default::default() };
        let run = |id: &str, pair: &Pair, verdict: &str, ended_ms: u64| CiRun { id: id.into(), pair: pair.clone(), started_ms: ended_ms - 1, ended_ms, verdict: verdict.into(), shards: vec![shard(if verdict == "ok" { "ok" } else { "fail" })], ..Default::default() };
        d.ci.runs.push(run("128", &old, "ok", 1_000));
        d.ci.runs.push(run("129", &red, "fail", 2_000));
        // an incomplete run after the red is no verdict either
        d.ci.runs.push(run("130", &newer, "incomplete", 3_000));
        d.verify.verified.push(Tip { pair: newer.clone(), at_ms: 4_000, diff: "affected".into() });
        let v = status_value(&d, 9, &|_| String::new());
        let ci = get(&v, &["branches", "0"]);
        assert_eq!((get(ci, &["source"]).as_str(), get(ci, &["verdict"]).as_str()), (Some("ci"), Some("red")));
        assert_eq!(get(ci, &["tip"]).as_str(), Some(red.work.as_str()));
        assert_eq!(get(ci, &["last_pass"]).as_str(), Some(old.work.as_str()), "CI's last pass is its own");
        assert!(get(ci, &["scripts", "0", "steps", "0", "detail"]).as_str().unwrap().contains("E0433"));
        let verify = get(&v, &["branches", "1"]);
        assert_eq!((get(verify, &["source"]).as_str(), get(verify, &["verdict"]).as_str()), (Some("verify"), Some("green")));
        assert_eq!(get(verify, &["last_pass"]).as_str(), Some(newer.work.as_str()));
        assert!(get(&v, &["note"]).as_str().unwrap().contains("ci run 130"), "{:?}", get(&v, &["note"]).as_str());
    }
}
