//! Long JIT correctness legs, after input changes, one lease at a time. No gate waits for them.

use super::ci::Inputs;
use super::leg::Spec;
use super::verify::{self, Verdict};
use super::{snapshot, stamp, Ctx};
use crate::proto::{wall_ms, Sev};
use crate::state::{JitLegsRun, Leg, Pair, NOT_RUN};
use makepad_strict_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(60);

/// The warm leg: its verdicts are times, so it runs as a timed lease on the bench box.
const WARM: &str = "warm";

/// The warm leg runs at most once in this long, on the newest pair: run at every JIT-changing
/// pair it held the bench box 83 min a day, 56 of them while a lane's timed ask waited (11-10).
/// A pair it skips says so; once the window has passed, the newest pair gets it.
const WARM_EVERY_MS: u64 = 4 * 3600 * 1000;

/// What a warm leg the window skipped says, before when it last ran.
fn warm_skipped_why() -> String {
    format!("warm: at most once per {} h, on the newest pair;", WARM_EVERY_MS / 3_600_000)
}

/// The newest run whose warm leg ran or runs (not one the window skipped): its start and pair.
fn warm_ran(runs: &[JitLegsRun]) -> Option<(u64, Pair)> {
    runs.iter().rev().find(|r| r.legs.iter().any(|l| l.name == WARM && l.state != NOT_RUN)).map(|r| (r.started_ms, r.pair.clone()))
}

/// The run that gives the newest pair its warm leg once the window has passed (`now`), when the
/// newest run's was skipped: that run's other legs as they ended, its warm leg to run (its log
/// at `warm_log`). None: nothing owed.
fn warm_owed(runs: &[JitLegsRun], now: u64, warm_log: &dyn Fn(u64) -> String) -> Option<JitLegsRun> {
    let newest = runs.last().filter(|r| r.ended_ms > 0)?;
    newest.legs.iter().find(|l| l.name == WARM && l.state == NOT_RUN && l.summary.starts_with(&warm_skipped_why()))?;
    if warm_ran(runs).is_some_and(|(at, _)| now < at + WARM_EVERY_MS) {
        return None;
    }
    let legs = newest.legs.iter().map(|l| if l.name == WARM { Leg { name: l.name.clone(), state: "wait".into(), log: warm_log(now), ..Default::default() } } else { l.clone() }).collect();
    Some(JitLegsRun { pair: newest.pair.clone(), started_ms: now, ended_ms: 0, legs })
}

pub fn start(ctx: Arc<Ctx>) {
    let _ = std::thread::Builder::new().name("devhub-jitlegs".into()).spawn(move || {
        let mut next = Instant::now();
        let mut unchanged = None;
        loop {
            let (active, asked) = ctx.store.read(|d| (d.jitlegs.runs.last().filter(|r| r.ended_ms == 0).cloned(), d.jitlegs.asked));
            if let Some(r) = active {
                run(&ctx, r);
                continue;
            }
            // unrelated state changes wake us too; only a manual request skips the tick
            let now = Instant::now();
            if !asked && now < next {
                ctx.store.wait(next - now);
                continue;
            }
            next = now + TICK;
            let Some(pair) = snapshot(&ctx.repo) else {
                ctx.store.wait(TICK);
                continue;
            };
            let (known, last) = ctx.store.read(|d| (d.jitlegs.runs.iter().any(|r| r.pair == pair), d.jitlegs.runs.last().map(|r| r.pair.clone())));
            if !asked && (known || unchanged.as_ref() == Some(&pair)) {
                // no new pair: the newest one's warm leg, if the window skipped it and has passed
                let now_ms = wall_ms();
                let warm_log = |ms: u64| ctx.logs("jitlegs").join(stamp(ms)).join(format!("{WARM}.log")).to_string_lossy().into_owned();
                if let Some(r) = ctx.store.read(|d| warm_owed(&d.jitlegs.runs, now_ms, &warm_log)) {
                    let _ = std::fs::create_dir_all(ctx.logs("jitlegs").join(stamp(r.started_ms)));
                    ctx.store.update(|d| d.jitlegs.runs.push(r.clone())).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
                    ctx.store.event("jitlegs", Sev::Info, format!("jitlegs {}: its warm leg, at most once per {} h, on the newest pair", r.pair.short(), WARM_EVERY_MS / 3_600_000), None);
                    run(&ctx, r);
                }
                continue;
            }
            let arm = super::arm::classify(&ctx.repo, last.as_ref(), &pair, asked);
            if !asked && !arm.required && last.as_ref().is_some_and(|p| Inputs::between(&ctx.repo, p, &pair).is_some_and(|i| !i.jit_changed)) {
                ctx.store.event("jitlegs", Sev::Info, format!("jitlegs {}: {}; JIT inputs unchanged", pair.short(), arm.summary), None);
                unchanged = Some(pair);
                continue;
            }
            let started_ms = wall_ms();
            let dir = ctx.logs("jitlegs").join(stamp(started_ms));
            let _ = std::fs::create_dir_all(&dir);
            let runnable = verify::legs(arm.required);
            // (a run asked by hand runs every leg)
            let warm_recent = if asked { None } else { ctx.store.read(|d| warm_ran(&d.jitlegs.runs)) }.filter(|(at, _)| started_ms < at + WARM_EVERY_MS);
            let legs = verify::legs(true).into_iter().map(|d| {
                let log = dir.join(format!("{}.log", d.name));
                let skipped = !runnable.iter().any(|r| r.name == d.name);
                if skipped { let _ = std::fs::write(&log, format!("{}\n", arm.summary)); }
                if let Some((at, ran)) = warm_recent.as_ref().filter(|_| !skipped && d.name == WARM) {
                    let summary = format!("{} it ran on {} {} min ago", warm_skipped_why(), ran.short(), started_ms.saturating_sub(*at) / 60_000);
                    let _ = std::fs::write(&log, format!("{summary}\n"));
                    return Leg { log: log.to_string_lossy().into_owned(), name: d.name, state: NOT_RUN.into(), summary, ..Default::default() };
                }
                Leg { log: log.to_string_lossy().into_owned(), name: d.name, state: if skipped { "ok" } else { "wait" }.into(), summary: arm.summary.clone(), ..Default::default() }
            }).collect();
            let r = JitLegsRun { pair, started_ms, ended_ms: 0, legs };
            ctx.store.update(|d| {
                if asked {
                    d.jitlegs.asked = false;
                }
                d.jitlegs.runs.push(r.clone());
            }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
            ctx.store.event("jitlegs", Sev::Info, format!("jitlegs {} started; {}", r.pair.short(), arm.summary), None);
            run(&ctx, r);
        }
    });
}

fn run(ctx: &Ctx, r: JitLegsRun) {
    let dir = ctx.logs("jitlegs").join(stamp(r.started_ms));
    let _ = std::fs::create_dir_all(&dir);
    super::prune(&ctx.logs("jitlegs"), 20);
    // the lockfile the legs build with: the one the master resolved (shipped with each leg)
    if !dir.join("Cargo.lock").exists() {
        let _ = std::fs::copy(ctx.repo.join("Cargo.lock"), dir.join("Cargo.lock"));
    }
    let base = ctx.store.read(|d| d.verify.verified.last().map(|t| t.pair.clone()));
    let mut env = vec!["env".to_string(), "CARGO_INCREMENTAL=0".into(), "DIFF_SCOPE=all".into(), format!("FUZZ_FIXED={}", verify::FUZZ)];
    if let Some(l) = r.legs.iter().find(|l| l.name == "arm64" && l.summary.starts_with("arm64: required (")) {
        env.push(format!("DEVHUB_ARM64_VERDICT={}", l.summary));
    }
    if let Some(b) = base {
        env.push(format!("VERIFY_BASE={}", b.work));
        env.push(format!("VERIFY_CBASE={}", b.main));
    }
    for def in verify::legs(true) {
        let Some(leg) = r.legs.iter().find(|l| l.name == def.name).cloned() else { continue };
        if matches!(leg.state.as_str(), "ok" | "fail" | "incomplete" | NOT_RUN) {
            continue;
        }
        let os = def.needs.split(',').find_map(|kv| kv.strip_prefix("os=")).unwrap_or("linux");
        let needs = match super::fleet_rustc(ctx, os) {
            Some(v) => format!("{}{}rustc_eq={v}", def.needs, if def.needs.is_empty() { "" } else { "," }),
            None => def.needs.clone(),
        };
        let spec = Spec { since_ms: None, lane: format!("jitlegs-{}", def.name), agent: "devhub-jitlegs".into(), kind: def.kind, needs, prio: super::leg::BACKGROUND_PRIO, rev: r.pair.rev(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: def.node.clone() };
        let log = leg.log.clone();
        let verdict = verify::run_leg(ctx, &spec, &r.pair, &def, &env, Path::new(&log), Some(leg));
        let summary = ctx.store.read(|d| d.jitlegs.runs.last().and_then(|r| r.legs.iter().find(|l| l.name == def.name)).map(|l| l.summary.clone()).unwrap_or_default());
        let name = &def.name;
        match verdict {
            Verdict::Fail => ctx.store.event("jitlegs", Sev::Err, format!("jitlegs {}: {name} failed: {summary}", r.pair.short()), None),
            Verdict::Incomplete => ctx.store.event("jitlegs", Sev::Warn, format!("jitlegs {}: {name} incomplete: {summary}", r.pair.short()), None),
            Verdict::Ok => ctx.store.event("jitlegs", Sev::Info, format!("jitlegs {}: {name} ok: {summary}", r.pair.short()), None),
        }
    }
    let ended_ms = wall_ms();
    ctx.store.update(|d| {
        if let Some(last) = d.jitlegs.runs.last_mut().filter(|last| last.pair == r.pair && last.started_ms == r.started_ms) {
            last.ended_ms = ended_ms;
        }
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    let classified = r.legs.iter().find(|l| l.name == "arm64" && l.summary.starts_with("arm64: skipped (")).map_or(String::new(), |l| format!("; {}", l.summary));
    ctx.store.event("jitlegs", Sev::Info, format!("jitlegs {} ended (wall {}s){classified}", r.pair.short(), ended_ms.saturating_sub(r.started_ms) / 1000), None);
}

/// `jitlegs [status|run]`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let words: Vec<&str> = line.split_whitespace().skip(1).collect();
    match words.as_slice() {
        [] | ["status"] => Ok(ctx.store.read(|d| d.section(Some("jitlegs")).unwrap_or_default())),
        ["run"] => {
            ctx.store.update(|d| d.jitlegs.asked = true)?;
            ctx.store.event("jitlegs", Sev::Info, "jitlegs: asked for the newest landed pair".into(), None);
            Ok("{\"queued\":true}".into())
        }
        _ => Err("jitlegs [status|run]".into()),
    }
}

pub fn render(v: &Value) -> String {
    let Some(r) = v.get("runs").and_then(|a| a.as_arr()).and_then(|a| a.last()) else { return "jitlegs: no run yet\n".into() };
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |k: &str| r.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let pair = r.get("pair").unwrap_or(&Value::Null);
    let ended = n("ended_ms");
    let wall = (if ended == 0 { wall_ms() } else { ended }).saturating_sub(n("started_ms")) / 1000;
    let mut out = format!("jitlegs {},{} {} wall {}s\n", super::short(&s(pair, "work")), super::short(&s(pair, "main")), if ended == 0 { "running" } else { "ended" }, wall);
    for l in r.get("legs").and_then(|a| a.as_arr()).unwrap_or(&[]) {
        out.push_str(&format!("  {:<9} {:<10} {} {}\n", s(l, "name"), s(l, "state"), s(l, "node"), s(l, "summary")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legs_run(pair: &str, started_ms: u64, warm: &str, why: &str) -> JitLegsRun {
        let leg = |name: &str, state: &str, summary: &str| Leg { name: name.into(), state: state.into(), summary: summary.into(), ..Default::default() };
        JitLegsRun { pair: Pair::new(pair, "m"), started_ms, ended_ms: started_ms + 1, legs: vec![leg("full-a", "ok", ""), leg(WARM, warm, why)] }
    }

    /// The warm leg runs at most once per window: a pair inside it skips it and says when it ran;
    /// once the window has passed, the newest pair gets it, with its other legs as they ended,
    /// and nothing is owed after that run or for a pair the window did not skip.
    #[test]
    fn the_warm_leg_runs_once_per_window_and_then_on_the_newest_pair() {
        let skipped = format!("{} it ran on a 60 min ago", warm_skipped_why());
        let hour = 3_600_000;
        let mut runs = vec![legs_run("a", 0, "ok", ""), legs_run("b", hour, NOT_RUN, &skipped), legs_run("c", 2 * hour, NOT_RUN, &skipped)];
        let log = |ms: u64| format!("logs/{ms}/warm.log");
        assert_eq!(warm_ran(&runs), Some((0, Pair::new("a", "m"))), "the skipped runs are not when it ran");
        assert!(warm_owed(&runs, WARM_EVERY_MS - 1, &log).is_none(), "inside the window nothing is owed");
        let owed = warm_owed(&runs, WARM_EVERY_MS, &log).expect("past the window the newest pair is owed its warm leg");
        assert_eq!(owed.pair, Pair::new("c", "m"), "the newest pair");
        assert_eq!((owed.started_ms, owed.ended_ms), (WARM_EVERY_MS, 0));
        let warm = owed.legs.iter().find(|l| l.name == WARM).unwrap();
        assert_eq!((warm.state.as_str(), warm.log.as_str(), warm.summary.as_str()), ("wait", log(WARM_EVERY_MS).as_str(), ""), "its warm leg runs, logged in its own run");
        assert_eq!(owed.legs.iter().find(|l| l.name == "full-a").map(|l| l.state.as_str()), Some("ok"), "its other legs stay as they ended");
        runs.push(owed);
        assert!(warm_owed(&runs, WARM_EVERY_MS + 1, &log).is_none(), "a run going is owed nothing");
        runs.last_mut().unwrap().ended_ms = WARM_EVERY_MS + 1;
        assert_eq!(warm_ran(&runs).map(|(at, _)| at), Some(WARM_EVERY_MS), "it ran with that run");
        assert!(warm_owed(&runs, 3 * WARM_EVERY_MS, &log).is_none(), "the newest run's warm leg ran: nothing owed");
        runs.push(legs_run("d", 3 * WARM_EVERY_MS, NOT_RUN, "arm64-style skip"));
        assert!(warm_owed(&runs, 4 * WARM_EVERY_MS, &log).is_none(), "a warm leg not run for another reason is not the window's");
    }
}
