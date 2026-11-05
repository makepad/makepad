//! `devhub allapps`: `makepad check` of every app, example and WM package of the newest landed
//! pair (tools/run/fixture/allapps_check.sh; makepad-app-wm-all is not checked), a scheduled
//! master job (was allapps.sh and allapps.last.txt): hourly, only while no lease waits in the
//! queue, at the lowest priority on one x64 pool box, and only for a pair it has not checked.
//! `devhub allapps kick` runs it now. The result is state and an event. A master restart ends no
//! run: the new master follows it on its lease, or, its lease gone, asks again at once with the
//! run's age.

use super::leg::{Held, Spec};
use super::{age, snapshot, stamp, Ctx};
use crate::lease::sched::State;
use crate::lease::Kind;
use crate::proto::{wall_ms, Sev};
use crate::state::{AllappsRun, Pair, PkgErrors};
use std::sync::Arc;
use std::time::Duration;

pub const EVERY_MS: u64 = 3600 * 1000;
const TICK: Duration = Duration::from_secs(60);

/// allapps_check.sh's output: `ERR <n> <pkg>` with up to 3 indented error lines, `?? <pkg> (no
/// summary line)`, `BUILD FAILED`, and `allapps done` at the end.
pub fn parse(text: &str) -> (String, Vec<PkgErrors>) {
    let mut errs: Vec<PkgErrors> = Vec::new();
    let (mut done, mut build) = (false, false);
    for l in text.lines() {
        let mut w = l.split_whitespace();
        match w.next() {
            Some("ERR") => {
                let n = w.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                errs.push(PkgErrors { pkg: w.next().unwrap_or("").to_string(), errors: n, first: Vec::new() });
            }
            Some("??") => errs.push(PkgErrors { pkg: w.next().unwrap_or("").to_string(), errors: 0, first: vec!["no summary line".into()] }),
            _ if l.starts_with("    ") => {
                if let Some(e) = errs.last_mut().filter(|e| e.first.len() < 3) {
                    e.first.push(crate::proto::clean(l.trim(), 240));
                }
            }
            _ if l.trim() == "allapps done" => done = true,
            _ if l.trim() == "BUILD FAILED" => build = true,
            _ => {}
        }
    }
    let verdict = if build {
        "build-failed"
    } else if !done {
        "incomplete"
    } else if errs.is_empty() {
        "ok"
    } else {
        "errors"
    };
    (verdict.to_string(), errs)
}

pub fn start(ctx: Arc<Ctx>) {
    // the run a previous master did not see end: its record names its lease, so the restore keeps
    // that lease for it ([`super::go`])
    let mut back = ctx.store.read(|d| d.allapps.running.clone());
    let _ = std::thread::Builder::new().name("devhub-allapps".into()).spawn(move || loop {
        if let Some(r) = back.take() {
            run(&ctx, r.pair.clone(), Some(r));
        } else if let Some(pair) = due(&ctx) {
            run(&ctx, pair, None);
        }
        ctx.store.wait(TICK);
    });
}

/// What it leases and runs ([`DEVHUB_TEST_ALLAPPS`]: a test's command, on any box).
fn command(ctx: &Ctx, pair: &Pair, since_ms: Option<u64>) -> (Spec, Vec<String>) {
    let spec = Spec { since_ms, lane: "allapps".into(), agent: "devhub-allapps".into(), kind: Kind::Long, needs: "arch=x64,os=linux,role=pool".into(), prio: super::verify::PRIO, rev: pair.rev(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: None };
    #[cfg(test)]
    if let Ok(cmd) = std::env::var("DEVHUB_TEST_ALLAPPS") {
        return (Spec { needs: String::new(), ..spec }, vec![cmd]);
    }
    (spec, ["env", "CARGO_INCREMENTAL=0", "bash", "tools/run/fixture/allapps_check.sh"].iter().map(|s| s.to_string()).collect())
}

/// The run of its lease (one per lease).
const RUN_N: u64 = 1;

/// The pair to check now: asked, or an hour since the last run, a pair it has not checked (or
/// checked incompletely), and nobody waiting for a lease.
fn due(ctx: &Ctx) -> Option<Pair> {
    let (on, asked, last) = ctx.store.read(|d| (d.allapps.on, d.allapps.asked, d.allapps.last.clone()));
    if !on && !asked {
        return None;
    }
    let pair = snapshot(&ctx.repo)?;
    if asked {
        return Some(pair);
    }
    // incomplete evidence: the same pair again, an hour later
    if last.as_ref().is_some_and(|l| (l.pair == pair && l.verdict != "incomplete") || wall_ms().saturating_sub(l.at_ms) < EVERY_MS) {
        return None;
    }
    let busy = ctx.leases.view().iter().any(|l| l.state == State::Queued);
    (!busy).then_some(pair)
}

/// One run of `pair`; `back`: a previous master's run of it, followed on its lease when that is
/// still held, asked for again at once (with its age) when not.
fn run(ctx: &Ctx, pair: Pair, back: Option<AllappsRun>) {
    let dir = ctx.logs("allapps");
    let _ = std::fs::create_dir_all(&dir);
    super::prune(&dir, 30);
    let resumed = back.is_some();
    let mut r = back.unwrap_or_else(|| {
        let started = wall_ms();
        AllappsRun { pair: pair.clone(), at_ms: started, verdict: "running".into(), log: dir.join(format!("{}.log", stamp(started))).to_string_lossy().into_owned(), ..Default::default() }
    });
    let started = r.at_ms;
    let log = std::path::PathBuf::from(&r.log);
    let save = |r: &AllappsRun| ctx.store.update(|d| d.allapps.running = Some(r.clone())).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    ctx.store.update(|d| {
        d.allapps.asked = false;
        d.allapps.running = Some(r.clone());
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    let (spec, argv) = command(ctx, &pair, Some(started));
    // its lease, taken back from the previous master: its run is followed from where the log
    // stops (the node may have started it before that master noted it), or started on it
    let mut kept = if resumed && super::leg::lease_held(ctx, &r.lease) { Some(Held::attach(&r.lease, &r.node)) } else { None };
    let mut following = kept.as_ref().is_some_and(|h| r.run_n > 0 || h.has_run(RUN_N) == Ok(true));
    if resumed {
        let how = match &kept {
            Some(_) if following => "its run goes on",
            Some(_) => "its lease is kept: the run starts on it",
            None => "its lease ended: it asks again at once, with its age",
        };
        ctx.store.event("allapps", Sev::Info, format!("allapps {}: the master restarted; {how}", pair.short()), None);
    } else {
        ctx.store.event("allapps", Sev::Info, format!("allapps {} started", pair.short()), None);
    }
    let outcome = (|| -> Result<(), String> {
        // preempted by lane work, it asks again (a fresh log)
        loop {
            let held = match kept.take() {
                Some(h) => h,
                None => {
                    (r.run_n, following) = (0, false);
                    Held::acquire(&spec, &mut |_| {}, &|| false)?
                }
            };
            r.lease = held.id.clone();
            r.node = held.node.clone();
            save(&r);
            let res = if following {
                let off = std::fs::metadata(&log).map_or(0, |m| m.len());
                let mut file = std::fs::OpenOptions::new().append(true).create(true).open(&log).map_err(|e| format!("{}: {e}", log.display()))?;
                held.follow(RUN_N, off, &mut file)
            } else {
                let mut file = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
                held.run_asked("allapps", ".", &argv, &mut file, &mut |n| {
                    r.run_n = n;
                    save(&r);
                })
            };
            let id = held.id.clone();
            held.release(res.as_ref().ok().copied());
            if res.is_err() && super::leg::was_preempted(&id) {
                (r.run_n, following) = (0, false);
                continue;
            }
            return res.map(|_| ());
        }
    })();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let (verdict, errors) = parse(&text);
    r.verdict = verdict;
    r.errors = errors;
    r.secs = wall_ms().saturating_sub(started) / 1000;
    if let Err(e) = &outcome {
        if r.verdict == "incomplete" {
            r.errors.push(PkgErrors { pkg: "-".into(), errors: 0, first: vec![crate::proto::clean(e, 240)] });
        }
    }
    let line = match r.verdict.as_str() {
        "ok" => format!("allapps OK {}: every package checks clean ({} s on {})", pair.short(), r.secs, r.node),
        "errors" => format!("allapps ERRORS {}: {}", pair.short(), r.errors.iter().map(|e| format!("{} ({})", e.pkg, e.errors)).collect::<Vec<_>>().join(", ")),
        "build-failed" => format!("allapps BUILD FAILED {} on {}", pair.short(), r.node),
        _ => format!("allapps INCOMPLETE {}: {}", pair.short(), outcome.err().unwrap_or_else(|| "no `allapps done` line".into())),
    };
    let sev = match r.verdict.as_str() {
        "ok" => Sev::Info,
        "incomplete" => Sev::Warn,
        _ => Sev::Err,
    };
    let done = r.clone();
    ctx.store.update(|d| {
        d.allapps.running = None;
        d.allapps.last = Some(done);
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    ctx.store.event("allapps", sev, line, None);
}

/// `allapps [status|on|off|kick]`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    match line.split_whitespace().nth(1) {
        None | Some("status") => Ok(ctx.store.read(|d| d.section(Some("allapps"))).unwrap_or_default()),
        Some(on @ ("on" | "off")) => {
            ctx.store.update(|d| d.allapps.on = on == "on")?;
            ctx.store.event("allapps", Sev::Info, format!("allapps: hourly runs {on}"), None);
            Ok(format!("{{\"on\":{}}}", on == "on"))
        }
        Some("kick") => {
            ctx.store.update(|d| d.allapps.asked = true)?;
            ctx.store.event("allapps", Sev::Info, "allapps: asked to run now".into(), None);
            Ok("{\"asked\":true}".into())
        }
        Some(x) => Err(format!("allapps: unknown word `{x}` (status on off kick)")),
    }
}

/// The terse view.
pub fn render(v: &makepad_strict_json::Value) -> String {
    let s = |v: Option<&makepad_strict_json::Value>, k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let pair = |r: Option<&makepad_strict_json::Value>| {
        let p = r.and_then(|r| r.get("pair"));
        format!("{},{}", super::short(&s(p, "work")), super::short(&s(p, "main")))
    };
    let last = v.get("last").filter(|x| !x.is_null());
    let running = v.get("running").filter(|x| !x.is_null());
    let mut out = format!(
        "allapps on={} last={} {} ({} ago) running={}\n",
        v.get("on").and_then(|x| x.as_bool()).unwrap_or(true) as u8,
        last.map_or("-".into(), |l| pair(Some(l))),
        s(last, "verdict"),
        age(last.and_then(|l| l.get("at_ms")).and_then(|x| x.as_u64()).unwrap_or(0)),
        running.map_or("-".into(), |r| format!("{} {}@{}", pair(Some(r)), s(Some(r), "lease"), s(Some(r), "node"))),
    );
    for e in last.and_then(|l| l.get("errors")).and_then(|a| a.as_arr()).unwrap_or(&[]) {
        out.push_str(&format!("  ERR {} {}\n", e.get("errors").and_then(|x| x.as_u64()).unwrap_or(0), s(Some(e), "pkg")));
        for f in e.get("first").and_then(|a| a.as_arr()).unwrap_or(&[]) {
            out.push_str(&format!("      {}\n", f.as_str().unwrap_or("")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn allapps_output() {
        let t = "ERR 2 makepad-app-x\n    error[E0425]: cannot find `y`\n    error: z\n?? makepad-example-q (no summary line)\nallapps done\n";
        let (v, e) = super::parse(t);
        assert_eq!(v, "errors");
        assert_eq!((e[0].pkg.as_str(), e[0].errors, e[0].first.len()), ("makepad-app-x", 2, 2));
        assert_eq!((e[1].pkg.as_str(), e[1].errors), ("makepad-example-q", 0));
        assert_eq!(super::parse("allapps done\n").0, "ok");
        assert_eq!(super::parse("x\nBUILD FAILED\n").0, "build-failed");
        assert_eq!(super::parse("ERR 1 a\n").0, "incomplete");
    }
}
