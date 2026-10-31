//! `devhub verify`: the rolling full verifier (was tools/run/fixture/verify_tip.py and
//! verified-tips.txt). The master verifies the newest landed work + main pair, one run at a time,
//! at the lowest lease priority; its legs ([`legs`]) run on their own boxes at once:
//! - `full-a`, `full-b`: the two halves of land_gate.sh `full`, timed leases on two x64 pool nodes;
//! - `arm64`: arm64_leg.sh `--diff-fast`, a timed lease on an arm64 Mac;
//! - CI ([`super::ci::gate`]): its shards (the `check:` shards cover what the platforms leg did),
//!   its verdict folded in, so a push of a verified tip has passed CI too.
//!
//! The diff leg diffs only when the JIT, platform or diff programs changed since the last verified
//! tip (`DIFF_SCOPE=affected`), and every program once a day; the fuzzer runs 500 programs (the
//! script rotates the window with the tip). A passing pair is a verified tip. A failing one is
//! bisected over the landings since the newest verified tip (first-parent order, landing times
//! from the reflogs) to the first bad landed commit, kept as a failure and said as an `err` event;
//! that pair is not tried again, newer tips are. A started pair runs to its verdict and only then
//! the newest tip is picked: a landing never restarts a run, and neither does a master restart (the
//! run is state, its legs and CI shards go on where they are on their boxes). Incomplete evidence
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

/// One leg of a verify run: its own lease on its own box.
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
}

impl LegDef {
    pub fn new(name: &str, kind: Kind, needs: &str, marker: &str, argv: &[&str]) -> LegDef {
        LegDef { name: name.into(), kind, needs: needs.into(), marker: marker.into(), argv: argv.iter().map(|a| a.to_string()).collect(), node: None }
    }
}

/// The legs of a run (`_full`: the nightly run that diffs every program; legs that differ by it,
/// such as orch/ci.rs's platform legs, read it).
pub fn legs(_full: bool) -> Vec<LegDef> {
    #[cfg(test)]
    if let Ok(cmd) = std::env::var("DEVHUB_TEST_LEG") {
        return vec![LegDef::new("leg", Kind::Test, "", "TESTLEG", &[&cmd])];
    }
    vec![
        LegDef::new("full-a", Kind::Test, "arch=x64,os=linux,role=pool", "NODEGATE", &["bash", "tools/run/fixture/land_gate.sh", "full-a"]),
        LegDef::new("full-b", Kind::Test, "arch=x64,os=linux,role=pool", "NODEGATE", &["bash", "tools/run/fixture/land_gate.sh", "full-b"]),
        LegDef::new("arm64", Kind::Test, "arch=arm64,os=macos,rustc=1.97", "ARM64", &["bash", "tools/run/fixture/arm64_leg.sh", "--diff-fast"]),
    ]
}

/// How many error lines a leg keeps.
const LEG_ERRORS: usize = 20;

/// The lowest lease priority: landings and lanes go first.
pub const PRIO: i32 = super::leg::BACKGROUND_PRIO;
/// An older pair, asked about what failed on the tip: behind the tip's own run, and never owed.
pub const OLD_PAIR_PRIO: i32 = PRIO + 1;
/// The fuzzer's fixed-seed programs per run.
pub const FUZZ: u32 = 500;
/// Every program is diffed this often (the nightly full diff).
pub const FULL_EVERY_MS: u64 = 24 * 3600 * 1000;
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

pub fn start(ctx: Arc<Ctx>) {
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
            if let Some(r) = &started {
                ask.dry = r.dry;
            }
            let known = self.ctx.store.read(|d| d.verify.verified.iter().any(|t| t.pair == pair) || d.verify.failures.iter().any(|f| f.pair == pair));
            if started.is_none() && explicit.is_none() && known {
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
                None => ask.full || self.ctx.store.read(|d| d.verify.verified.is_empty() || wall_ms().saturating_sub(d.verify.last_full_ms) >= FULL_EVERY_MS),
            };
            let (verdict, legs) = self.gate(&pair, full, ask.dry, "tip", asked.is_some(), None);
            // (a run that goes on is still what was asked for)
            if asked.is_some() && self.ctx.store.read(|d| d.verify.run.is_none()) {
                self.pop_asked();
            }
            match verdict {
                Verdict::Ok => {
                    self.record_good(&pair, full, ask.dry);
                    self.ctx.store.event("verify", Sev::Info, format!("verify OK {} {n}/{n} legs and CI (diff {})", pair.short(), if full { "all" } else { "affected" }, n = self::legs(full).len()), None);
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

    /// The legs of `pair` at once, each on its own box; the run is state while it goes. The run
    /// of this pair that is in the state (a previous master's, or one that ended incomplete) goes
    /// on: what passed stands, what runs on a lease that came back is followed, the rest asks again.
    /// `only`: just the legs and CI shards of these names, on an older pair.
    fn gate(&self, pair: &Pair, full: bool, dry: bool, purpose: &str, asked: bool, only: Option<&[String]>) -> (Verdict, Vec<Leg>) {
        let (prior, stale) = self.ctx.store.update(|d| match d.verify.run.take() {
            Some(r) if r.pair == *pair && r.purpose == purpose => (Some(r), None),
            r => (None, r),
        });
        // (another run in the state has no one to go on: its leases go with it)
        for l in stale.iter().flat_map(|r| r.legs.iter()).filter(|l| super::leg::lease_held(&self.ctx, &l.lease)) {
            let _ = crate::cli::ask(&makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-verify"), None, "release", &l.lease);
        }
        let id = prior.as_ref().map_or_else(|| stamp(wall_ms()), |r| r.id.clone());
        let dir = self.ctx.logs("verify").join(&id);
        let _ = std::fs::create_dir_all(&dir);
        super::prune(&self.ctx.logs("verify"), 60);
        let base = self.ctx.store.read(|d| d.verify.verified.iter().rev().find(|t| &t.pair != pair).map(|t| t.pair.clone()));
        let mut env = vec!["env".to_string(), "CARGO_INCREMENTAL=0".into(), format!("DIFF_SCOPE={}", if full || base.is_none() { "all" } else { "affected" }), format!("FUZZ_FIXED={FUZZ}")];
        if let Some(b) = &base {
            env.push(format!("VERIFY_BASE={}", b.work));
            env.push(format!("VERIFY_CBASE={}", b.main));
        }
        let mut defs = legs(full);
        if let Some(only) = only {
            defs.retain(|d| only.contains(&d.name));
        }
        // CI goes along unless what is asked for is all legs
        let with_ci = only.is_none_or(|o| o.iter().any(|n| !defs.iter().any(|d| d.name == *n)));
        let leg_prio = if only.is_some() { OLD_PAIR_PRIO } else { PRIO };
        let earlier = |name: &str| prior.as_ref().and_then(|r| r.legs.iter().find(|l| l.name == name)).cloned();
        let legs: Vec<Leg> = defs.iter().map(|d| &d.name).map(|name| earlier(name).unwrap_or_else(|| Leg { name: name.to_string(), state: "wait".into(), log: dir.join(format!("{name}.log")).to_string_lossy().into_owned(), ..Default::default() })).collect();
        let tries = prior.as_ref().map_or(0, |r| r.tries);
        let run = Run { id: id.clone(), pair: pair.clone(), started_ms: prior.as_ref().map_or_else(wall_ms, |r| r.started_ms), diff: if full { "all" } else { "affected" }.into(), dry, purpose: purpose.into(), legs: legs.clone(), asked, tries };
        self.ctx.store.update(|d| d.verify.run = Some(run));
        let word = if prior.is_some() { "goes on" } else { "started" };
        self.ctx.store.event("verify", Sev::Info, format!("verify {purpose} {} {word} (run {id}, diff {})", pair.short(), if full { "all" } else { "affected" }), None);
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
                let spec = Spec { only: false, lane: format!("verify-{}", def.name), agent: "devhub-verify".into(), kind: def.kind, needs: def.needs.clone(), prio: leg_prio, rev: pair.rev(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: def.node.clone() };
                let held = super::leg::lease_held(&ctx, &leg.lease);
                if held && leg.run_n == 0 {
                    // its lease came back with no run started: asked again
                    let _ = Held::attach(&leg.lease, &leg.node);
                }
                let back = (held && leg.run_n > 0 && leg.state == "run").then_some(leg);
                let v = run_leg(&ctx, &spec, &pair, &def, &env, &log, back);
                results.lock().unwrap().push(v);
            }));
        }
        // CI next to the legs: its shards on their boxes, its verdict folded in
        let ci = with_ci.then(|| {
            let (ctx, pair, only) = (self.ctx.clone(), pair.clone(), only.map(<[String]>::to_vec));
            let c2 = self.ctx.clone();
            std::thread::spawn(move || super::ci::gate(&ctx, &pair, dry, only.as_deref(), Arc::new(move || !c2.store.read(|d| d.verify.on))))
        });
        for t in threads {
            let _ = t.join();
        }
        let (ci_verdict, ci_legs) = ci.map_or((Verdict::Ok, Vec::new()), |ci| ci.join().unwrap_or((Verdict::Incomplete, Vec::new())));
        let mut results = results.lock().unwrap().clone();
        results.push(ci_verdict);
        let ok = results.iter().filter(|v| **v == Verdict::Ok).count();
        let verdict = if results.contains(&Verdict::Fail) {
            Verdict::Fail
        } else if results.len() < defs.len() + 1 || results.contains(&Verdict::Incomplete) {
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
        legs.extend(ci_legs);
        let _ = std::fs::write(dir.join("pair"), format!("{} {} {ok}/{} (CI included)\n", pair.work, pair.main, defs.len() + 1));
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
        let Some((base, work, commercial)) = found else { return Bisected::Unresolved("no verified first-parent baseline".into()) };
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

/// One leg: lease, check the tree is at the pair, run, verdict; its state as it goes. `back`: a
/// previous master's run of it, on the lease this master took back, is followed instead.
fn run_leg(ctx: &Ctx, spec: &Spec, pair: &Pair, def: &LegDef, env: &[String], log: &Path, mut back: Option<Leg>) -> Verdict {
    let (name, marker) = (def.name.as_str(), def.marker.as_str());
    let set = |state: &str, lease: &str, node: &str, summary: &str| {
        ctx.store.update(|d| {
            if let Some(l) = d.verify.run.as_mut().and_then(|r| r.legs.iter_mut().find(|l| l.name == name)) {
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
            ctx.store.event("verify", Sev::Info, format!("verify leg {name} of {} on {} ({}): the master restarted, its run goes on", pair.short(), l.node, l.lease), None);
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
        let held = match Held::acquire(spec, &mut |l| set("wait", "", "", l), &|| false) {
            Ok(h) => h,
            Err(e) => {
                set("incomplete", "", "", &e);
                return Verdict::Incomplete;
            }
        };
        set("run", &held.id, &held.node, "");
        ctx.store.event("verify", Sev::Info, format!("verify leg {name} of {} on {} ({})", pair.short(), held.node, held.id), None);
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
                if let Some(l) = d.verify.run.as_mut().and_then(|r| r.legs.iter_mut().find(|l| l.name == name)) {
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
            if let Some(l) = d.verify.run.as_mut().and_then(|r| r.legs.iter_mut().find(|l| l.name == name)) {
                l.errors = errors;
            }
        });
    }
    ctx.store.event("verify", if v == Verdict::Fail { Sev::Warn } else { Sev::Info }, format!("verify leg {name} {} {}: {summary}", state.to_uppercase(), pair.short()), None);
    v
}

/// `verify [status|on|off|kick [--full]|pair <work> <main> [--dry] [--full]]`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let w: Vec<&str> = line.split_whitespace().skip(1).collect();
    let full = w.contains(&"--full");
    let dry = w.contains(&"--dry");
    match w.first().copied() {
        None | Some("status") => Ok(ctx.store.read(|d| d.section(Some("verify"))).unwrap_or_default()),
        Some(on @ ("on" | "off")) => {
            ctx.store.update(|d| d.verify.on = on == "on");
            ctx.store.event("verify", Sev::Info, format!("verify: rolling verification {on}"), None);
            Ok(format!("{{\"on\":{}}}", on == "on"))
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
