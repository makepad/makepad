//! The CI status for other programs: `GET /ci/v1/status`, read-only JSON, served by the master from
//! its verify state (was tools/ci's status endpoint on the CI mini; same route, same version-1
//! document, so Scope's CI strip reads it with only its `url` changed).
//!
//! One branch, `work`: its tip is the pair under test (a `tip` run) or else the newest judged pair;
//! `last_pass` the newest verified tip. Each verify leg is one "script" with one step, whose detail
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
            || t.starts_with("CIERR ");
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

/// The version-1 status document of the verify and CI state (`now` in seconds; `tail` reads a
/// leg's log). A run in progress shows first (CI's, then verify's tip run); else the newest of the
/// verified tips, the failures and the finished CI runs.
pub fn status_value(d: &Doc, now: u64, tail: &dyn Fn(&str) -> String) -> Value {
    let v = &d.verify;
    let mut note = String::new();
    let red_if = |legs: &[Leg]| if legs.iter().any(|l| l.state == "fail") { "red" } else { "running" };
    let live = if let Some(r) = &d.ci.run {
        let legs = ci_legs(r);
        Some(Shown { tip: r.pair.clone(), verdict: red_if(&legs), finished: 0, detail: format!("ci run {} ({} shards)", r.id, r.shards.len()), legs, at_ms: r.started_ms })
    } else if let Some(r) = v.run.as_ref().filter(|r| r.purpose == "tip") {
        let legs = r.legs.clone();
        Some(Shown { tip: r.pair.clone(), verdict: red_if(&legs), finished: 0, detail: format!("verify run {} (diff {})", r.id, r.diff), legs, at_ms: r.started_ms })
    } else {
        None
    };
    if live.is_none() {
        if let Some(r) = &v.run {
            note = format!("verify {} at {}", r.purpose, r.pair.short());
        }
    }
    let mut done: Vec<Shown> = Vec::new();
    if let Some(g) = v.verified.iter().max_by_key(|t| t.at_ms) {
        done.push(Shown { tip: g.pair.clone(), verdict: "green", finished: g.at_ms / 1000, detail: format!("verified (diff {})", g.diff), legs: Vec::new(), at_ms: g.at_ms });
    }
    if let Some(f) = v.failures.iter().max_by_key(|f| f.at_ms) {
        done.push(Shown { tip: f.pair.clone(), verdict: "red", finished: f.at_ms / 1000, detail: failure_detail(f), legs: f.legs.clone(), at_ms: f.at_ms });
    }
    if let Some(r) = d.ci.runs.last() {
        let verdict = match r.verdict.as_str() {
            "ok" => "green",
            "fail" => "red",
            _ => "waiting",
        };
        done.push(Shown { tip: r.pair.clone(), verdict, finished: r.ended_ms / 1000, detail: format!("ci run {} {}: {}", r.id, r.verdict, r.critical), legs: ci_legs(r), at_ms: r.ended_ms });
    }
    let shown = live.or_else(|| done.into_iter().max_by_key(|x| x.at_ms));
    let last_pass = v
        .verified
        .iter()
        .map(|t| (t.at_ms, &t.pair.work))
        .chain(d.ci.runs.iter().filter(|r| r.verdict == "ok").map(|r| (r.ended_ms, &r.pair.work)))
        .max_by_key(|(at, _)| *at)
        .map_or(Value::Null, |(_, w)| s(w));
    let shown = shown.unwrap_or(Shown { tip: Pair::default(), verdict: "waiting", finished: 0, detail: String::new(), legs: Vec::new(), at_ms: 0 });
    if shown.tip.work.is_empty() && note.is_empty() {
        note = "no run yet".into();
    }
    let branch = obj(vec![
        ("name", s("work")),
        ("tip", s(&shown.tip.work)),
        ("commercial", s(&shown.tip.main)),
        ("verdict", s(shown.verdict)),
        ("finished", Value::Int(shown.finished as i64)),
        ("detail", s(&shown.detail)),
        ("last_pass", last_pass),
        ("scripts", Value::Arr(shown.legs.iter().map(|l| script(l, tail)).collect())),
    ]);
    obj(vec![("version", Value::Int(VERSION)), ("generated", Value::Int(now as i64)), ("note", s(note)), ("branches", Value::Arr(vec![branch]))])
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
        Leg { name: name.into(), state: state.into(), lease: "L1".into(), node: "101".into(), summary: format!("CI-{state}"), errors: Vec::new(), log: log.into() }
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
        let br = get(&v, &["branches", "0"]);
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
    }

    #[test]
    fn a_tip_run_is_running_until_a_leg_fails_and_a_bisect_keeps_the_judged_tip() {
        let mut d = Doc::default();
        assert_eq!(get(&status_value(&d, 1, &|_| String::new()), &["note"]).as_str(), Some("no run yet"));
        let a = Pair::new(&"a".repeat(40), &"c".repeat(40));
        d.verify.verified.push(Tip { pair: a.clone(), at_ms: 1_000, diff: "affected".into() });
        let t = Pair::new(&"d".repeat(40), &"c".repeat(40));
        d.verify.run = Some(Run { id: "R".into(), pair: t.clone(), started_ms: 1, diff: "affected".into(), dry: false, purpose: "tip".into(), legs: vec![leg("ci-workspace", "run", "")] });
        let v = status_value(&d, 1, &|_| String::new());
        assert_eq!(get(&v, &["branches", "0", "verdict"]).as_str(), Some("running"));
        assert_eq!(get(&v, &["branches", "0", "tip"]).as_str(), Some(t.work.as_str()));
        d.verify.run.as_mut().unwrap().legs.push(leg("ci-cross", "fail", ""));
        assert_eq!(get(&status_value(&d, 1, &|_| String::new()), &["branches", "0", "verdict"]).as_str(), Some("red"));
        d.verify.run.as_mut().unwrap().purpose = "bisect 1/3".into();
        let v = status_value(&d, 1, &|_| String::new());
        assert_eq!(get(&v, &["branches", "0", "verdict"]).as_str(), Some("green"));
        assert_eq!(get(&v, &["branches", "0", "tip"]).as_str(), Some(a.work.as_str()));
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
}
