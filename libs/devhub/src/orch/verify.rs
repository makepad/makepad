//! `devhub verify`: the correctness-leg verifier of the newest landed work + main pair. The long
//! JIT correctness legs (`legs()`) run independently in [`super::jitlegs`]. Full verification runs
//! the Mac correctness legs, bypassing arm64 classification. Full CI belongs to the CI bucket.
//!
//! A passing pair is a verified tip. A failing one is bisected over the landings since the newest
//! verified tip (first-parent order, landing times from the reflogs) to the first bad landed
//! commit, kept as a failure and said as an `err` event;
//! that pair is not tried again, newer tips are. A started pair runs to its verdict and only then
//! the newest tip is picked: a landing never restarts a run, and neither does a master restart (the
//! run is state, its legs go on where they are on their boxes). Incomplete evidence
//! (no verdict, a node lost) goes on a minute later on the same pair, what passed standing, up to
//! [`TRIES`] times.

use super::leg::{Held, Spec};
use super::{age, git, snapshot, stamp, Ctx};
use crate::lease::Kind;
use crate::proto::{wall_ms, Sev};
use crate::state::{Asked, Blame, Failure, Leg, Pair, Run, Tip};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A gate's evidence: all legs passed, one failed, or not enough to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Fail,
    Incomplete,
}

/// One correctness leg, with its own lease.
#[derive(Clone, Debug, PartialEq)]
pub struct LegDef {
    pub name: String,
    pub kind: Kind,
    /// `arch=x64,os=linux,role=pool`
    pub needs: String,
    /// its verdict lines: `<marker>-OK` / `<marker>-FAIL`
    pub marker: String,
    /// the command, from the tree's root (the run's env words go before it)
    pub argv: Vec<String>,
    /// a preferred node
    pub node: Option<String>,
    /// its verdict is reported, never the run's: a failure or a missing verdict is an event and the
    /// leg's state, and the run counts it as passed (a leg on its way to gating)
    pub report_only: bool,
}

impl LegDef {
    pub fn new(name: &str, kind: Kind, needs: &str, marker: &str, argv: &[&str]) -> LegDef {
        LegDef { name: name.into(), kind, needs: needs.into(), marker: marker.into(), argv: argv.iter().map(|a| a.to_string()).collect(), node: None, report_only: false }
    }

    /// The leg reports its verdict without deciding the run's (`report_only`).
    pub fn report_only(mut self) -> LegDef {
        self.report_only = true;
        self
    }
}

/// The long correctness legs; `arm` includes the Mac legs, the environment sets the diff scope.
pub(crate) fn legs(arm: bool) -> Vec<LegDef> {
    #[cfg(test)]
    if let Ok(cmd) = std::env::var("DEVHUB_TEST_LEG") {
        return vec![LegDef::new("leg", Kind::Test, "", "TESTLEG", &[&cmd])];
    }
    let mut legs = vec![
        LegDef::new("full-a", Kind::Test, "arch=x64,os=linux,role=pool", "NODEGATE", &["bash", "tools/run/fixture/land_gate.sh", "full-a"]),
        LegDef::new("full-b", Kind::Test, "arch=x64,os=linux,role=pool", "NODEGATE", &["bash", "tools/run/fixture/land_gate.sh", "full-b"]),
        LegDef::new("arm64", Kind::Test, "arch=arm64,os=macos,rustc=1.97", "ARM64", &["bash", "tools/run/fixture/arm64_leg.sh", "--diff-fast"]),
        // makepad run's no-codegen mode (iOS's constraint: every function interpreted) on an arm64
        // Mac: diff_fast's nocodegen tier, agree.sh, app boots. Report-only until the Macs'
        // toolchains agree (agree's rustc-side q_object_default_type_relative on rustc 1.97)
        LegDef::new("nocodegen", Kind::Test, "arch=arm64,os=macos,rustc=1.97", "NOCODEGEN", &["bash", "tools/run/fixture/nocodegen_leg.sh"]).report_only(),
    ];
    legs.retain(|d| arm || !d.needs.contains("arch=arm64,os=macos"));
    legs
}

/// How many error lines a leg keeps.
const LEG_ERRORS: usize = 20;

/// The lowest lease priority: landings and lanes go first.
pub const PRIO: i32 = super::leg::BACKGROUND_PRIO;
/// An older pair, asked about what failed on the tip: behind the tip's own run, and never owed.
pub const OLD_PAIR_PRIO: i32 = PRIO + 1;
/// The fuzzer's fixed-seed programs per run.
pub const FUZZ: u32 = 500;
const IDLE: Duration = Duration::from_secs(60);
/// How often a pair may end incomplete before the newest tip takes its place.
const TRIES: u32 = 3;

/// A leg's verdict from its log and exit code (verify_tip.py's rule): the last `<MARKER>-OK|FAIL`
/// line decides; none or a nonzero exit after OK is incomplete.
pub fn leg_verdict(_name: &str, marker: &str, text: &str, exit: Option<i32>) -> (Verdict, String) {
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with(&format!("{marker}-"))).collect();
    let last = lines.iter().rev().find(|l| l.starts_with(&format!("{marker}-OK")) || l.starts_with(&format!("{marker}-FAIL")));
    let summary = last.map_or_else(|| format!("no verdict (exit {})", exit.map_or("lost".into(), |c| c.to_string())), |l| crate::proto::clean(l, 400));
    let v = match last {
        None => Verdict::Incomplete,
        Some(l) if l.starts_with(&format!("{marker}-FAIL")) => Verdict::Fail,
        Some(_) if exit == Some(0) => Verdict::Ok,
        Some(_) => Verdict::Incomplete,
    };
    (v, summary)
}

/// The first-parent commits after `base` up to `tip`, oldest first.
pub fn first_parent(repo: &Path, base: &str, tip: &str) -> Result<Vec<String>, String> {
    let chain: Vec<String> = git(repo, &["rev-list", "--first-parent", tip])?.lines().map(str::to_string).collect();
    let at = chain.iter().position(|c| c == base).ok_or_else(|| format!("baseline {} is not on the first-parent history of {}", super::short(base), super::short(tip)))?;
    Ok(chain[..at].iter().rev().cloned().collect())
}

/// When each commit of `chain` became visible on `refname` (`git reflog show --format='%H %gd'
/// --date=raw`, newest first): `(seconds, side, index, sha)`. A fast-forward can carry several
/// merges; they keep their first-parent order. Commit dates are not landing dates.
pub fn landing_events(reflog: &str, chain: &[String], side: u8) -> Result<Vec<(u64, u8, usize, String)>, String> {
    let mut out = Vec::new();
    if chain.is_empty() {
        return Ok(out);
    }
    let mut covered: i64 = -1;
    for row in reflog.lines().rev() {
        let mut w = row.split_whitespace();
        let Some(sha) = w.next() else { continue };
        let Some(pos) = chain.iter().position(|c| c == sha) else { continue };
        // `refname@{<secs> <tz>}`
        let Some(secs) = row.split("@{").nth(1).and_then(|t| t.split_whitespace().next()).and_then(|t| t.parse::<u64>().ok()) else { continue };
        if pos as i64 > covered {
            for i in (covered + 1) as usize..=pos {
                out.push((secs, side, i, chain[i].clone()));
            }
            covered = pos as i64;
        }
    }
    if covered != chain.len() as i64 - 1 {
        return Err("the reflog does not cover the first-parent range".into());
    }
    Ok(out)
}

/// The pairs from `base` to `tip` in landing order: one step per landed commit, work before
/// commercial at equal times (like the landing train).
pub fn steps(base: &Pair, mut events: Vec<(u64, u8, usize, String)>) -> Vec<Pair> {
    events.sort();
    let mut pairs = vec![base.clone()];
    for (_, side, _, sha) in events {
        let mut p = pairs.last().unwrap().clone();
        if side == 0 {
            p.work = sha;
        } else {
            p.main = sha;
        }
        pairs.push(p);
    }
    pairs
}

/// A verify run this old raises an error, again each time this long passes.
const VERIFY_STALLED_MS: u64 = 2 * 60 * 60 * 1000;

/// Report a stalled regression run and what it waits for.
fn watch_stall(ctx: Arc<Ctx>) {
    let mut said = 0u64;
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let line = ctx.store.read(|d| {
            let r = d.verify.run.as_ref()?;
            let age = wall_ms().saturating_sub(r.started_ms);
            if age < VERIFY_STALLED_MS || wall_ms().saturating_sub(said) < VERIFY_STALLED_MS {
                return None;
            }
            let legs: Vec<String> = r.legs.iter().map(|l| format!("{} {}", l.name, l.state)).collect();
            Some(format!("verify {}: not concluded after {} min: {}", r.pair.short(), age / 60_000, legs.join(", ")))
        });
        if let Some(line) = line {
            said = wall_ms();
            ctx.store.event("verify", Sev::Err, line, None);
        }
    }
}

pub fn start(ctx: Arc<Ctx>) {
    let watched = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-verify-stall".into()).spawn(move || watch_stall(watched));
    let _ = std::thread::Builder::new().name("devhub-verify".into()).spawn(move || Verifier { ctx }.main());
}

struct Verifier {
    ctx: Arc<Ctx>,
}

/// What a failing pair's bisect found.
enum Bisected {
    /// everything that failed fails on this verified pair too
    OldOnly(Pair, Vec<String>),
    Blame(Blame),
    Unresolved(String),
}

impl Verifier {
    fn main(&self) {
        let mut retry_at = 0u64;
        loop {
            // a bisect a previous master was in goes on
            if let Some(f) = self.ctx.store.read(|d| d.verify.failing.clone()) {
                self.failed(&f.pair, false, f.legs);
                continue;
            }
            let (on, asked, started) = self.ctx.store.read(|d| (d.verify.on, d.verify.asked.first().cloned(), d.verify.run.clone().filter(|r| r.purpose == "tip")));
            // a started pair runs to its verdict; what was asked for meanwhile waits for it
            let asked = if started.as_ref().is_some_and(|r| !r.asked) { None } else { asked };
            if !on && asked.is_none() {
                self.ctx.store.wait(IDLE);
                continue;
            }
            let explicit = asked.as_ref().and_then(|a| a.pair.clone());
            let Some(pair) = started.as_ref().map(|r| r.pair.clone()).or_else(|| explicit.clone()).or_else(|| snapshot(&self.ctx.repo)) else {
                self.ctx.store.wait(IDLE);
                continue;
            };
            let mut ask = asked.clone().unwrap_or_default();
            let weekly = on && started.is_none() && asked.is_none() && self.ctx.store.read(|d| super::arm::weekly_due(wall_ms(), d.verify.last_full_ms));
            ask.full |= weekly;
            if let Some(r) = &started {
                ask.dry = r.dry;
            }
            let known = self.ctx.store.read(|d| d.verify.verified.iter().any(|t| t.pair == pair) || d.verify.failures.iter().any(|f| f.pair == pair));
            if started.is_none() && explicit.is_none() && known && !ask.full {
                if asked.is_some() {
                    self.pop_asked();
                    self.ctx.store.event("verify", Sev::Info, format!("verify: {} is already {}", pair.short(), self.ctx.store.read(|d| if d.verify.verified.iter().any(|t| t.pair == pair) { "verified" } else { "failed" })), None);
                }
                self.ctx.store.wait(IDLE);
                continue;
            }
            if (asked.is_none() || started.is_some()) && wall_ms() < retry_at {
                self.ctx.store.wait(Duration::from_millis(retry_at - wall_ms()));
                continue;
            }
            let full = match &started {
                Some(r) => r.diff == "all",
                // `--full` sets the diff scope and forces the weekly Mac correctness sweep
                None => ask.full,
            };
            let (verdict, legs) = self.gate(&pair, full, ask.dry, "tip", asked.is_some(), None);
            // `verify skip`: this pair is given up, the newest one is next
            // (the state already says so: `verify skip` dropped the run)
            if SKIP.swap(false, std::sync::atomic::Ordering::AcqRel) {
                self.ctx.store.update(|d| d.verify.run = None);
                continue;
            }
            // (a run that goes on is still what was asked for)
            if asked.is_some() && self.ctx.store.read(|d| d.verify.run.is_none()) {
                self.pop_asked();
            }
            match verdict {
                Verdict::Ok => {
                    self.record_good(&pair, full, ask.dry);
                    let classified = legs.iter().find(|l| l.name == "arm64" && l.summary.starts_with("arm64: skipped (")).map_or(String::new(), |l| format!("; {}", l.summary));
                    self.ctx.store.event("verify", Sev::Info, format!("verify OK {}: correctness legs (diff {}){classified}", pair.short(), if full { "all" } else { "affected" }), None);
                }
                Verdict::Fail => self.failed(&pair, ask.dry, legs),
                Verdict::Incomplete => {
                    retry_at = wall_ms() + 60_000;
                    let why: Vec<String> = legs.iter().filter(|l| l.state == "incomplete").map(|l| format!("{}: {}", l.name, l.summary)).collect();
                    let next = if self.ctx.store.read(|d| d.verify.run.is_some()) { "goes on in 1 min, what passed stands" } else { "the newest tip in 1 min" };
                    self.ctx.store.event("verify", Sev::Warn, format!("verify INCOMPLETE {}: {}; {next}", pair.short(), why.join("; ")), None);
                }
            }
        }
    }

    /// A pair failed. What failed on the newest verified pair too is an old failure: reported, and
    /// a pair with nothing else failing is verified. The rest is bisected to its first bad landing
    /// (the bisect is state: a restarted master goes on with it), and kept as a failure.
    fn failed(&self, pair: &Pair, dry: bool, legs: Vec<Leg>) {
        let (blame, unresolved) = if dry {
            (None, "dry run: not bisected".to_string())
        } else {
            let known = self.ctx.store.read(|d| d.verify.failing.clone().filter(|f| f.pair == *pair));
            let failing = crate::state::Failing { pair: pair.clone(), legs: legs.clone(), gated: known.as_ref().map_or(Vec::new(), |f| f.gated.clone()), old: known.and_then(|f| f.old) };
            self.ctx.store.update(|d| d.verify.failing = Some(failing));
            match self.bisect(pair) {
                Bisected::OldOnly(base, old) => {
                    self.record_good(pair, false, false);
                    self.ctx.store.update(|d| d.verify.failing = None);
                    self.ctx.store.event("verify", Sev::Warn, format!("verify OK {} with old failures, they fail on {} too: {}", pair.short(), base.short(), old.join(" ")), None);
                    return;
                }
                Bisected::Blame(b) => (Some(b), String::new()),
                Bisected::Unresolved(why) => (None, why),
            }
        };
        let line = match &blame {
            Some(b) => format!("verify FAIL {}: first bad {}:{} \"{}\"", pair.short(), b.repo, super::short(&b.sha), b.subject),
            None => format!("verify FAIL {}: {unresolved}", pair.short()),
        };
        let failure = Failure { pair: pair.clone(), at_ms: wall_ms(), blame, unresolved, legs };
        let detail = crate::state::Doc { verify: crate::state::Verify { failures: vec![failure.clone()], ..Default::default() }, ..Default::default() }.section(Some("verify"));
        if !dry {
            self.ctx.store.update(|d| {
                d.verify.failures.push(failure);
                d.verify.failing = None;
            });
        }
        self.ctx.store.event("verify", Sev::Err, line, detail);
        // a skip that gave this bisect up is spent: the next pair's run is not stopped by it
        SKIP.store(false, std::sync::atomic::Ordering::Release);
    }

    /// What failed the pair being bisected and did not fail on the verified pair too.
    fn still_failing(&self) -> Vec<String> {
        self.ctx.store.read(|d| {
            let Some(f) = d.verify.failing.as_ref() else { return Vec::new() };
            let old = f.old.as_deref().unwrap_or(&[]);
            f.legs.iter().filter(|l| l.state == "fail" && !old.contains(&l.name)).map(|l| l.name.clone()).collect()
        })
    }

    /// What the failing pair's failures do on the verified pair `base`, in today's environment:
    /// those that fail there too; None: no verdict.
    fn old_failures(&self, base: &Pair) -> Option<Vec<String>> {
        if let Some(old) = self.ctx.store.read(|d| d.verify.failing.as_ref().and_then(|f| f.old.clone())) {
            return Some(old);
        }
        if SKIP.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let (verdict, legs) = self.gate(base, false, false, "baseline", false, Some(&self.still_failing()));
        if verdict == Verdict::Incomplete {
            return None;
        }
        let old: Vec<String> = legs.into_iter().filter(|l| l.state == "fail").map(|l| l.name).collect();
        self.ctx.store.update(|d| {
            if let Some(f) = d.verify.failing.as_mut() {
                f.old = Some(old.clone());
            }
        });
        Some(old)
    }

    /// A pair of the bisect, its verdict kept: a restarted master does not gate it twice.
    fn step(&self, pair: &Pair, purpose: &str) -> Verdict {
        let word = |v: Verdict| match v {
            Verdict::Ok => "ok",
            Verdict::Fail => "fail",
            Verdict::Incomplete => "incomplete",
        };
        let known = self.ctx.store.read(|d| d.verify.failing.as_ref().and_then(|f| f.gated.iter().find(|(p, _)| p == pair).map(|(_, v)| v.clone())));
        if let Some(v) = known {
            return [Verdict::Ok, Verdict::Fail, Verdict::Incomplete].into_iter().find(|x| word(*x) == v).unwrap_or(Verdict::Incomplete);
        }
        if SKIP.load(std::sync::atomic::Ordering::Acquire) {
            return Verdict::Incomplete;
        }
        let v = self.gate(pair, false, false, purpose, false, Some(&self.still_failing())).0;
        self.ctx.store.update(|d| {
            if let Some(f) = d.verify.failing.as_mut() {
                f.gated.push((pair.clone(), word(v).to_string()));
            }
        });
        v
    }

    fn pop_asked(&self) {
        self.ctx.store.update(|d| {
            if !d.verify.asked.is_empty() {
                d.verify.asked.remove(0);
            }
        });
    }

    fn record_good(&self, pair: &Pair, full: bool, dry: bool) {
        if dry {
            return;
        }
        self.ctx.store.update(|d| {
            if full {
                d.verify.last_full_ms = wall_ms();
            }
            if !d.verify.verified.iter().any(|t| &t.pair == pair) {
                d.verify.verified.push(Tip { pair: pair.clone(), at_ms: wall_ms(), diff: if full { "all" } else { "affected" }.into() });
            }
        });

    }

    /// Correctness legs of `pair`; a previous master's run goes on, what passed standing.
    /// `only`: just the legs of these names, on an older pair.
    fn gate(&self, pair: &Pair, full: bool, dry: bool, purpose: &str, asked: bool, only: Option<&[String]>) -> (Verdict, Vec<Leg>) {
        let (prior, stale) = self.ctx.store.update(|d| match d.verify.run.take() {
            Some(r) if r.pair == *pair && r.purpose == purpose => (Some(r), None),
            r => (None, r),
        });
        // an older master's JIT legs no longer belong to verify
        for l in stale.iter().chain(prior.iter()).flat_map(|r| r.legs.iter()).filter(|l| super::leg::lease_held(&self.ctx, &l.lease)) {
            let _ = crate::cli::ask(&makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-verify"), None, "release", &l.lease);
        }
        let id = prior.as_ref().map_or_else(|| stamp(wall_ms()), |r| r.id.clone());
        let dir = self.ctx.logs("verify").join(&id);
        let _ = std::fs::create_dir_all(&dir);
        super::prune(&self.ctx.logs("verify"), 60);
        let base = self.ctx.store.read(|d| d.verify.verified.iter().rev().find(|t| &t.pair != pair).map(|t| t.pair.clone()));
        let macs = full || only.is_some_and(|o| o.iter().any(|n| n == "arm64" || n == "nocodegen"));
        let arm = prior.as_ref().and_then(|_| super::arm::Decision::read(&dir.join("arm64-classification.log"))).unwrap_or_else(|| super::arm::classify(&self.ctx.repo, base.as_ref(), pair, macs));
        self.ctx.store.event("verify", Sev::Info, format!("verify {}: {}", pair.short(), arm.summary), None);
        let _ = std::fs::write(dir.join("arm64-classification.log"), format!("{}\n", arm.summary));
        if full && prior.is_none() && purpose == "tip" {
            self.ctx.store.update(|d| {
                // Record the attempt too: a failed Sunday sweep must not loop all day.
                if !dry { d.verify.last_full_ms = wall_ms(); }
            });
        }
        let mut env = vec!["env".to_string(), "CARGO_INCREMENTAL=0".into(), format!("DIFF_SCOPE={}", if full || base.is_none() { "all" } else { "affected" }), format!("FUZZ_FIXED={FUZZ}")];
        env.push(format!("DEVHUB_ARM64_VERDICT={}", arm.summary));
        if let Some(b) = &base {
            env.push(format!("VERIFY_BASE={}", b.work));
            env.push(format!("VERIFY_CBASE={}", b.main));
        }
        let mut defs: Vec<LegDef> = if macs { legs(arm.required).into_iter().filter(|d| d.needs.contains("arch=arm64,os=macos")).collect() } else { Vec::new() };
        if let Some(only) = only {
            defs.retain(|d| only.contains(&d.name));
        }
        let job_units: Vec<super::ci::Unit> = defs.iter().map(|d| super::ci::Unit { dir: "tools/run".into(), section: format!("job.{}", d.name) }).collect();
        let job_digests = if job_units.is_empty() { None } else {
            let lock = prior.as_ref().and_then(|_| std::fs::read_to_string(dir.join("Cargo.lock")).ok()).map(Ok)
                .unwrap_or_else(|| std::fs::read_to_string(self.ctx.repo.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}")));
            match lock.and_then(|lock| super::ci::Tree::at(&self.ctx.repo, pair).and_then(|t| super::ci::Digests::with_lock(&t, &job_units, lock))) {
                Ok(d) => Some(d),
                Err(e) => {
                    self.ctx.store.event("verify", Sev::Err, format!("job proofs: {e}"), None);
                    return (Verdict::Incomplete, Vec::new());
                }
            }
        };
        if let Some(dg) = &job_digests {
            let lock = dir.join("Cargo.lock");
            if let Err(e) = std::fs::write(lock, &dg.lock) {
                self.ctx.store.event("verify", Sev::Err, format!("job lock snapshot: {e}"), None);
                return (Verdict::Incomplete, Vec::new());
            }
        }
        let leg_prio = if only.is_some() { OLD_PAIR_PRIO } else { PRIO };
        let earlier = |name: &str| prior.as_ref().and_then(|r| r.legs.iter().find(|l| l.name == name)).cloned();
        let legs: Vec<Leg> = defs.iter().map(|d| &d.name).map(|name| earlier(name).unwrap_or_else(|| Leg { name: name.to_string(), state: "wait".into(), log: dir.join(format!("{name}.log")).to_string_lossy().into_owned(), ..Default::default() })).collect();
        let tries = prior.as_ref().map_or(0, |r| r.tries);
        let run = Run { id: id.clone(), pair: pair.clone(), started_ms: prior.as_ref().map_or_else(wall_ms, |r| r.started_ms), diff: if full { "all" } else { "affected" }.into(), dry, purpose: purpose.into(), legs: legs.clone(), asked, tries };
        self.ctx.store.update(|d| d.verify.run = Some(run));
        let word = if prior.is_some() { "goes on" } else { "started" };
        self.ctx.store.event("verify", Sev::Info, format!("verify {purpose} {} {word} (run {id}, diff {}); {}", pair.short(), if full { "all" } else { "affected" }, arm.summary), None);
        let results: Arc<Mutex<Vec<Verdict>>> = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for (def, leg) in defs.iter().cloned().zip(legs.iter().cloned()) {
            // what a leg already said stands
            match leg.state.as_str() {
                "ok" => {
                    results.lock().unwrap().push(Verdict::Ok);
                    continue;
                }
                "fail" => {
                    results.lock().unwrap().push(Verdict::Fail);
                    continue;
                }
                _ => {}
            }
            let (ctx, pair, results, env) = (self.ctx.clone(), pair.clone(), results.clone(), env.clone());
            let log = dir.join(format!("{}.log", def.name));
            threads.push(std::thread::spawn(move || {
                // (the fleet's toolchain only)
                let os = def.needs.split(',').find_map(|kv| kv.strip_prefix("os=")).unwrap_or("linux");
                let needs = match super::fleet_rustc(&ctx, os) {
                    Some(v) => format!("{}{}rustc_eq={v}", def.needs, if def.needs.is_empty() { "" } else { "," }),
                    None => def.needs.clone(),
                };
                let spec = Spec { since_ms: None, only: false, lane: format!("verify-{}", def.name), agent: "devhub-verify".into(), kind: def.kind, needs, prio: leg_prio, rev: pair.rev(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: def.node.clone() };
                let v = run_leg(&ctx, &spec, &pair, &def, &env, &log, Some(leg));
                results.lock().unwrap().push(v);
            }));
        }
        for t in threads {
            let _ = t.join();
        }
        let mut results = results.lock().unwrap().clone();
        if let Some(dg) = job_digests {
            let mut updates = super::ci::Updates::new();
            let passed = self.ctx.store.read(|d| d.verify.run.as_ref().map(|r| r.legs.clone()).unwrap_or_default());
            for (def, unit) in defs.iter().zip(job_units) {
                if !passed.iter().any(|l| l.name == def.name && l.state == "ok") { continue; }
                let os = super::ci::Os::of(def.needs.split(',').find_map(|s| s.strip_prefix("os=")).unwrap_or("linux"));
                if os == super::ci::Os::Any { continue; }
                updates.insert((unit.clone(), os), super::ci::Proof { digest: dg.of(&unit, os).into(), pair: pair.clone(), capture: String::new(), date: super::ci::date(wall_ms()) });
            }
            if let Err(e) = super::ci::commit_proofs(&self.ctx, pair, &updates) {
                self.ctx.store.event("verify", Sev::Err, format!("job proofs: {e}"), None);
                results.push(Verdict::Incomplete);
            }
        }
        let ok = results.iter().filter(|v| **v == Verdict::Ok).count();
        let verdict = if results.contains(&Verdict::Fail) {
            Verdict::Fail
        } else if results.len() < defs.len() || results.contains(&Verdict::Incomplete) {
            Verdict::Incomplete
        } else {
            Verdict::Ok
        };
        // an incomplete tip stays the run: it goes on, what passed standing
        let goes_on = verdict == Verdict::Incomplete && purpose == "tip" && tries + 1 < TRIES;
        let mut legs = self.ctx.store.update(|d| {
            if !goes_on {
                return d.verify.run.take().map(|r| r.legs);
            }
            let r = d.verify.run.as_mut()?;
            let legs = r.legs.clone();
            r.tries += 1;
            for l in r.legs.iter_mut().filter(|l| l.state != "ok") {
                l.state = "wait".into();
                l.lease.clear();
                l.node.clear();
                l.run_n = 0;
            }
            Some(legs)
        }).unwrap_or_default();
        if !arm.required {
            for name in ["arm64", "nocodegen"] {
                legs.push(Leg { name: name.into(), state: "ok".into(), summary: arm.summary.clone(), log: dir.join("arm64-classification.log").to_string_lossy().into_owned(), ..Default::default() });
            }
        }
        let _ = std::fs::write(dir.join("pair"), format!("{} {} {ok}/{}; {}\n", pair.work, pair.main, defs.len(), arm.summary));
        (verdict, legs)
    }

    /// The first bad landed commit between the newest verified tip on both first-parent
    /// histories and `tip` (verify_tip.py's bisect: one good -> bad transition is assumed).
    fn bisect(&self, tip: &Pair) -> Bisected {
        let (repo, com) = (self.ctx.repo.clone(), self.ctx.commercial());
        let verified: Vec<Pair> = self.ctx.store.read(|d| d.verify.verified.iter().map(|t| t.pair.clone()).collect());
        let mut found = None;
        for p in verified.iter().rev() {
            if let (Ok(w), Ok(c)) = (first_parent(&repo, &p.work, &tip.work), first_parent(&com, &p.main, &tip.main)) {
                found = Some((p.clone(), w, c));
                break;
            }
        }
        let Some((base, work, commercial)) = found else {
            // work was re-pointed since (`devhub promote`): no landing to blame across it, but the
            // newest verified pair still says what is an old failure
            let Some(base) = verified.last().cloned() else { return Bisected::Unresolved("no verified baseline".into()) };
            let Some(old) = self.old_failures(&base) else { return Bisected::Unresolved(format!("no verdict on the baseline {}", base.short())) };
            return if self.still_failing().is_empty() && !old.is_empty() {
                Bisected::OldOnly(base, old)
            } else {
                Bisected::Unresolved(format!("no first-parent baseline (work was re-pointed after {})", base.short()))
            };
        };
        let events = (|| -> Result<_, String> {
            let wl = git(&repo, &["reflog", "show", "--format=%H %gd", "--date=raw", "work"])?;
            let cl = git(&com, &["reflog", "show", "--format=%H %gd", "--date=raw", "main"])?;
            let mut e = landing_events(&wl, &work, 0)?;
            e.extend(landing_events(&cl, &commercial, 1)?);
            Ok(e)
        })();
        let events = match events {
            Ok(e) => e,
            Err(e) => return Bisected::Unresolved(e),
        };
        let mut sorted = events.clone();
        sorted.sort();
        let pairs = steps(&base, events);
        if pairs.last() != Some(tip) || pairs.len() < 2 {
            return Bisected::Unresolved("empty or inconsistent first-parent range".into());
        }
        // the failures on the baseline in today's environment before blaming code
        let Some(old) = self.old_failures(&base) else { return Bisected::Unresolved(format!("no verdict on the baseline {}", base.short())) };
        if self.still_failing().is_empty() {
            return if old.is_empty() { Bisected::Unresolved("it failed with no failing leg named".into()) } else { Bisected::OldOnly(base, old) };
        }
        let (mut low, mut high) = (0usize, pairs.len() - 1);
        let mut step = 0;
        let total = (pairs.len() as f64).log2().ceil() as usize;
        while high - low > 1 {
            step += 1;
            let mid = (low + high) / 2;
            match self.step(&pairs[mid], &format!("bisect {step}/{total}")) {
                Verdict::Incomplete => return Bisected::Unresolved(format!("incomplete bisect pair {}", pairs[mid].short())),
                // (only what failed ran: it passes here, the pair is not verified by that)
                Verdict::Ok => low = mid,
                Verdict::Fail => high = mid,
            }
        }
        let (_, side, _, sha) = &sorted[high - 1];
        let (repo_name, dir) = if *side == 0 { ("work", &repo) } else { ("commercial", &com) };
        let subject = git(dir, &["show", "-s", "--format=%s", sha]).unwrap_or_default();
        Bisected::Blame(Blame { repo: repo_name.into(), sha: sha.clone(), subject, good: pairs[low].clone(), bad: pairs[high].clone() })
    }
}

/// `verify skip`: the run going gives up (legs not started do not start; running steps finish)
/// and the newest landed pair is verified next.
static SKIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A leg's command is its lease's third run, after the two `git rev-parse` of the tree check.
const LEG_RUN: u64 = 3;

fn active_leg<'a>(d: &'a mut crate::state::Doc, job: &str, pair: &Pair, name: &str) -> Option<&'a mut Leg> {
    let legs = if job == "jitlegs" {
        &mut d.jitlegs.runs.last_mut().filter(|r| r.pair == *pair && r.ended_ms == 0)?.legs
    } else {
        &mut d.verify.run.as_mut().filter(|r| r.pair == *pair)?.legs
    };
    legs.iter_mut().find(|l| l.name == name)
}

/// One leg: lease, check the tree is at the pair, run, verdict; its state as it goes. `back`: a
/// previous master's run of it, on the lease this master took back, is followed instead.
pub(crate) fn run_leg(ctx: &Ctx, spec: &Spec, pair: &Pair, def: &LegDef, env: &[String], log: &Path, mut back: Option<Leg>) -> Verdict {
    let (name, marker) = (def.name.as_str(), def.marker.as_str());
    let job = if spec.agent == "devhub-jitlegs" { "jitlegs" } else { "verify" };
    back = back.and_then(|mut l| {
        if !super::leg::lease_held(ctx, &l.lease) {
            return None;
        }
        if l.state == "run" && l.run_n > 0 {
            return Some(l);
        }
        // the node may have started the command before the master noted its run number
        let h = Held::attach(&l.lease, &l.node);
        if l.state == "run" && h.has_run(LEG_RUN) == Ok(true) {
            l.run_n = LEG_RUN;
            h.keep();
            Some(l)
        } else {
            None
        }
    });
    let set = |state: &str, lease: &str, node: &str, summary: &str| {
        ctx.store.update(|d| {
            if let Some(l) = active_leg(d, job, pair, name) {
                l.state = state.into();
                if !lease.is_empty() {
                    l.lease = lease.into();
                    l.node = node.into();
                    l.run_n = 0;
                }
                l.summary = summary.into();
            }
        })
    };
    // a background leg: preempted by lane work, it asks again (a fresh log)
    let (r, exit) = loop {
        if let Some(l) = back.take() {
            let off = std::fs::metadata(log).map_or(0, |m| m.len());
            let held = Held::attach(&l.lease, &l.node);
            ctx.store.event(job, Sev::Info, format!("{job} leg {name} of {} on {} ({}): the master restarted, its run goes on", pair.short(), l.node, l.lease), None);
            let r = match std::fs::OpenOptions::new().append(true).create(true).open(log) {
                Ok(mut f) => held.follow(l.run_n, off, &mut f),
                Err(e) => Err(format!("{}: {e}", log.display())),
            };
            let exit = r.as_ref().ok().copied();
            held.release(exit);
            if r.is_err() && super::leg::was_preempted(&l.lease) {
                set("wait", "", "", "preempted by lane work; asks again");
                continue;
            }
            break (r, exit);
        }
        let mut file = match std::fs::File::create(log) {
            Ok(f) => f,
            Err(e) => {
                set("incomplete", "", "", &format!("{}: {e}", log.display()));
                return Verdict::Incomplete;
            }
        };
        let held = match Held::acquire(spec, &mut |l| set("wait", "", "", l), &|| job == "verify" && SKIP.load(std::sync::atomic::Ordering::Acquire)) {
            Ok(h) => h,
            Err(e) => {
                set("incomplete", "", "", &e);
                return Verdict::Incomplete;
            }
        };
        if let Err(e) = super::ci::ship_lock(&held, log.parent().unwrap_or_else(|| Path::new(".")), false) {
            held.release(None);
            set("incomplete", "", "", &e);
            return Verdict::Incomplete;
        }
        set("run", &held.id, &held.node, "");
        ctx.store.event(job, Sev::Info, format!("{job} leg {name} of {} on {} ({})", pair.short(), held.node, held.id), None);
        // the tree must hold the pair before any gate command
        let at = held.output(&["git", "rev-parse", "HEAD"]).and_then(|(_, w)| held.output(&["git", "-C", "apps/commercial", "rev-parse", "HEAD"]).map(|(_, c)| (w, c)));
        match at {
            Ok((w, c)) if w == pair.work && c == pair.main => {}
            Ok((w, c)) => {
                let why = format!("the lease's tree is at {},{} not {}", super::short(&w), super::short(&c), pair.short());
                held.release(None);
                set("incomplete", "", "", &why);
                return Verdict::Incomplete;
            }
            Err(e) => {
                held.release(None);
                set("incomplete", "", "", &e);
                return Verdict::Incomplete;
            }
        }
        let argv: Vec<String> = env.iter().cloned().chain(def.argv.iter().cloned()).collect();
        let id = held.id.clone();
        let r = held.run_noted(&argv, &mut file, &mut |n| {
            ctx.store.update(|d| {
                if let Some(l) = active_leg(d, job, pair, name) {
                    l.run_n = n;
                }
            });
        });
        let exit = r.as_ref().ok().copied();
        held.release(exit);
        if r.is_err() && super::leg::was_preempted(&id) {
            set("wait", "", "", "preempted by lane work; asks again");
            continue;
        }
        break (r, exit);
    };
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let (v, mut summary) = leg_verdict(name, marker, &text, exit);
    if let Err(e) = r {
        summary = format!("{summary}; {e}");
    }
    let state = match v {
        Verdict::Ok => "ok",
        Verdict::Fail => "fail",
        Verdict::Incomplete => "incomplete",
    };
    set(state, "", "", &summary);
    if v != Verdict::Ok {
        let errors: Vec<String> = crate::land::failure_lines(&text).into_iter().take(LEG_ERRORS).collect();
        ctx.store.update(|d| {
            if let Some(l) = active_leg(d, job, pair, name) {
                l.errors = errors;
            }
        });
    }
    if job == "verify" {
        ctx.store.event("verify", if v == Verdict::Fail { Sev::Warn } else { Sev::Info }, format!("verify leg {name} {}{} {}: {summary}", state.to_uppercase(), if def.report_only && v != Verdict::Ok { " (report-only)" } else { "" }, pair.short()), None);
        if def.report_only {
            return Verdict::Ok;
        }
    }
    v
}

/// `verify [status|on|off|kick [--full]|pair <work> <main> [--dry] [--full]]`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let w: Vec<&str> = line.split_whitespace().skip(1).collect();
    let full = w.contains(&"--full");
    let dry = w.contains(&"--dry");
    match w.first().copied() {
        None | Some("status") => Ok(ctx.store.read(|d| d.to_value().get("verify").cloned().unwrap_or(makepad_strict_json::Value::Null).to_json())),
        Some(on @ ("on" | "off")) => {
            ctx.store.update(|d| d.verify.on = on == "on");
            ctx.store.event("verify", Sev::Info, format!("verify: rolling verification {on}"), None);
            Ok(format!("{{\"on\":{}}}", on == "on"))
        }
        Some("skip") => {
            // in the state at once (a master restart before the run gives up goes to the newest pair)
            let pair = ctx.store.update(|d| {
                // (a bisect of a failure is given up too)
                let failing = d.verify.failing.take().map(|f| f.pair.short());
                let pair = d.verify.run.take().map(|r| r.pair.short()).or(failing)?;
                d.verify.asked.clear();
                d.verify.asked.push(Asked { pair: None, dry: false, full: false });
                Some(pair)
            }).ok_or("verify skip: no run is going")?;
            SKIP.store(true, std::sync::atomic::Ordering::Release);
            ctx.store.event("verify", Sev::Warn, format!("verify {pair}: skipped by hand; the newest landed pair is next"), None);
            ctx.store.kick();
            Ok(format!("{{\"skipping\":{}}}", makepad_remote::json_str(&pair)))
        }
        Some("kick") => {
            let n = ctx.store.update(|d| {
                d.verify.asked.push(Asked { pair: None, dry: false, full });
                d.verify.asked.len()
            });
            ctx.store.event("verify", Sev::Info, format!("verify: asked for the newest landed pair{}", if full { " (full diff)" } else { "" }), None);
            Ok(format!("{{\"queued\":{n}}}"))
        }
        Some("pair") => {
            let resolve = |dir: &Path, r: &str| git(dir, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")]);
            let (Some(w), Some(c)) = (w.get(1), w.get(2)) else { return Err("verify pair <work sha> <main sha> [--dry] [--full]".into()) };
            let pair = Pair { work: resolve(&ctx.repo, w)?, main: resolve(&ctx.commercial(), c)? };
            let n = ctx.store.update(|d| {
                d.verify.asked.push(Asked { pair: Some(pair.clone()), dry, full });
                d.verify.asked.len()
            });
            ctx.store.event("verify", Sev::Info, format!("verify: asked for {}{}", pair.short(), if dry { " (dry)" } else { "" }), None);
            Ok(format!("{{\"queued\":{n},\"pair\":{}}}", makepad_remote::json_str(&pair.short())))
        }
        Some(x) => Err(format!("verify: unknown word `{x}` (status on off kick pair)")),
    }
}

/// The terse view: one head line, then the run's legs, then the newest failure.
pub fn render(v: &makepad_strict_json::Value) -> String {
    let s = |v: Option<&makepad_strict_json::Value>, k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |v: Option<&makepad_strict_json::Value>, k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_u64()).unwrap_or(0);
    let pair = |p: Option<&makepad_strict_json::Value>| format!("{},{}", super::short(&s(p, "work")), super::short(&s(p, "main")));
    let verified = v.get("verified").and_then(|a| a.as_arr()).and_then(|a| a.last());
    let failure = v.get("failures").and_then(|a| a.as_arr()).and_then(|a| a.last());
    let run = v.get("run").filter(|r| !r.is_null());
    let mut out = format!(
        "verify on={} verified={} ({} ago) running={} asked={} failed={}\n",
        v.get("on").and_then(|x| x.as_bool()).unwrap_or(true) as u8,
        verified.map_or("-".into(), |t| pair(t.get("pair"))),
        age(n(verified, "at_ms")),
        run.map_or("-".into(), |r| format!("{} {}", pair(r.get("pair")), s(Some(r), "purpose"))),
        v.get("asked").and_then(|a| a.as_arr()).map_or(0, |a| a.len()),
        failure.map_or("-".into(), |f| pair(f.get("pair"))),
    );
    if let Some(r) = run {
        for l in r.get("legs").and_then(|a| a.as_arr()).unwrap_or(&[]) {
            let o = Some(l);
            out.push_str(&format!("  {:<9} {:<10} {}{} {}\n", s(o, "name"), s(o, "state"), s(o, "lease"), if s(o, "node").is_empty() { String::new() } else { format!("@{}", s(o, "node")) }, s(o, "summary")));
        }
    }
    if let Some(f) = failure {
        let b = f.get("blame").filter(|b| !b.is_null());
        match b {
            Some(b) => out.push_str(&format!("  last failure {} ({} ago): first bad {}:{} {}\n", pair(f.get("pair")), age(n(Some(f), "at_ms")), s(Some(b), "repo"), super::short(&s(Some(b), "sha")), s(Some(b), "subject"))),
            None => out.push_str(&format!("  last failure {} ({} ago): {}\n", pair(f.get("pair")), age(n(Some(f), "at_ms")), s(Some(f), "unresolved"))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_follow_the_last_marker_line() {
        assert_eq!(leg_verdict("full-a", "NODEGATE", "x\nNODEGATE-OK fuzz:500\n", Some(0)).0, Verdict::Ok);
        assert_eq!(leg_verdict("full-a", "NODEGATE", "NODEGATE-FAIL: fuzz\n", Some(1)), (Verdict::Fail, "NODEGATE-FAIL: fuzz".to_string()));
        assert_eq!(leg_verdict("full-a", "NODEGATE", "NODEGATE-OK\n", Some(2)).0, Verdict::Incomplete);
        assert_eq!(leg_verdict("full-a", "NODEGATE", "build ok\n", Some(0)), (Verdict::Incomplete, "no verdict (exit 0)".to_string()));
        assert_eq!(leg_verdict("arm64", "ARM64", "", None).1, "no verdict (exit lost)");
    }

    fn sha(c: char) -> String {
        c.to_string().repeat(40)
    }

    #[test]
    fn bisect_steps_follow_landing_times() {
        // work landed a then b in one fast-forward at t=100, commercial x at t=100 and y at t=200
        let work = vec![sha('a'), sha('b')];
        let reflog = format!("{} work@{{200 +0000}}\n{} work@{{100 +0000}}\n{} work@{{50 +0000}}\n", sha('9'), sha('b'), sha('0'));
        let ev = landing_events(&reflog, &work, 0).unwrap();
        assert_eq!(ev.iter().map(|e| (e.0, e.2)).collect::<Vec<_>>(), vec![(100, 0), (100, 1)]);
        let com = vec![sha('c'), sha('d')];
        let creflog = format!("{} main@{{200 +0000}}\n{} main@{{100 +0000}}\n", sha('d'), sha('c'));
        let mut all = ev;
        all.extend(landing_events(&creflog, &com, 1).unwrap());
        let base = Pair { work: sha('0'), main: sha('1') };
        let p = steps(&base, all);
        assert_eq!(p.len(), 5);
        assert_eq!(p[1], Pair { work: sha('a'), main: sha('1') });
        assert_eq!(p[2], Pair { work: sha('b'), main: sha('1') });
        assert_eq!(p[3], Pair { work: sha('b'), main: sha('c') });
        assert_eq!(p[4], Pair { work: sha('b'), main: sha('d') });
        // a range the reflog lost
        assert!(landing_events(&format!("{} work@{{1 +0000}}\n", sha('a')), &work, 0).is_err());
    }
}
