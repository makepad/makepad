//! Buckets check landed tips on their own timers.
//! One run covers all landings since that bucket's last tip.
//! Failures name the lanes whose inputs changed; they never control landing.
//! A restarted master adopts its bucket's unfinished run instead of starting it again.
//! When a bucket is due comes from what outlives the master: its last start (its ledger, or the
//! runs of its kind it knows of) and the landings since. A master restart never moves it: CI was
//! due again at each of them (every ~30 min) and never ran for 8 h (11-02).
//! A run whose lease was lost under it (expired, its node lost it) has no verdict: it runs once
//! more at once, and lost again it is no verdict on the landings, only a warn: the bucket takes the
//! tip again at its next turn.

use crate::land::{Entry, St};
use crate::proto::{hub_record, wall_ms, Sev};
use crate::state::{Pair, Store};
use makepad_strict_json::{obj, s, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::land::Publish;

pub struct Timing {
    /// Least time between starts.
    pub every: Duration,
    /// Wait for the tip to stay unchanged this long.
    pub quiet: Duration,
    /// Start anyway after being due this long.
    pub cap: Duration,
}

/// When a bucket starts its next run: due at `every` after its last start and not before the
/// first landing it has not checked; then once the tip stayed `quiet` since it last changed, and
/// at the latest `cap` after it was due, quiet or not. All of them outlive the master.
fn starts_at(t: &Timing, last_start: u64, first_landed: u64, last_changed: u64) -> u64 {
    let ms = |d: Duration| d.as_millis() as u64;
    let due = last_start.saturating_add(ms(t.every)).max(first_landed);
    due.max(last_changed.saturating_add(ms(t.quiet))).min(due.saturating_add(ms(t.cap)))
}

/// A bucket's last run that its ledger may not have: the start and tip of the newest run of its
/// kind (CI's runs survive a master restart; the bucket that asked for one did not).
pub type Prior = fn(&crate::state::Doc) -> Option<(u64, Pair)>;

/// When each landing landed.
fn landed_ms(store: &Store) -> Vec<u64> {
    store.read(|d| d.land.iter().filter(|v| v.get("state").and_then(Value::as_str) == Some(St::Landed.as_str()))
        .filter_map(|v| v.get("changed_ms")?.as_u64()).collect())
}

/// The start a bucket run's log is named after (`<bucket>-<ms>.log`).
pub(crate) fn log_started(log: &Path) -> Option<u64> {
    log.file_stem()?.to_str()?.rsplit_once('-')?.1.parse().ok()
}

/// Notes a bucket run beside its log (`.log.run`), for a restarted master to adopt.
pub(crate) fn save_run(log: &Path, pid: u32, started: u64, base: &Pair, tip: &Pair) -> std::io::Result<()> {
    let sidecar = log.with_extension("log.run");
    let value = obj(vec![
        ("pid", Value::Int(pid as i64)), ("started_ms", Value::Int(started as i64)),
        ("base", pair_value(base)), ("tip", pair_value(tip)),
    ]);
    let temp = sidecar.with_extension("run.tmp");
    std::fs::write(&temp, value.to_json()).and_then(|_| std::fs::rename(&temp, &sidecar))
}

pub struct Outcome {
    pub ok: bool,
    pub failing: Vec<String>,
    pub log: String,
    pub summary: String,
    /// its lease was lost under it twice (the fleet, not the code): no verdict
    pub lost: bool,
}

/// The log's line before a bucket runs its gate once more after a lost lease; an adopted run
/// whose log has it runs no third time.
const RERUN: &str = "bucket: its lease was lost under the gate (the fleet, not the code): running it once more";

pub type Run = Arc<dyn Fn(Option<&Pair>, &Pair, &Path) -> Outcome + Send + Sync>;

const GATE_NEEDS: &str = "arch=x64,os=linux";
static LEDGER: Mutex<()> = Mutex::new(());

fn pair_value(pair: &Pair) -> Value {
    obj(vec![("work", s(&pair.work)), ("main", s(&pair.main))])
}

fn read_pair(v: &Value) -> Option<Pair> {
    let pair = Pair::new(v.get("work")?.as_str()?, v.get("main")?.as_str()?);
    pair.valid().then_some(pair)
}

fn read_run(path: &Path) -> Option<(u32, u64, Pair, Pair)> {
    let v = makepad_strict_json::parse(&std::fs::read(path).ok()?).ok()?;
    let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
    if pid == 0 { return None; }
    Some((pid, v.get("started_ms")?.as_u64()?, read_pair(v.get("base")?)?, read_pair(v.get("tip")?)?))
}

fn entry_pair(text: &str) -> Option<Pair> {
    let words: Vec<_> = text.split_whitespace().collect();
    let ["work", work, "main", main] = words.as_slice() else { return None };
    let pair = Pair::new(work, main);
    pair.valid().then_some(pair)
}

/// Newest first, across all buckets.
pub fn ledger(home: &Path, n: usize) -> Vec<String> {
    let _lock = LEDGER.lock().unwrap();
    let file = match std::fs::File::open(home.join("buckets.jsonl")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => { eprintln!("buckets ledger: {e}"); return Vec::new(); }
    };
    let mut lines = VecDeque::new();
    for line in BufReader::new(file).lines() {
        match line {
            Ok(line) if !line.is_empty() => {
                lines.push_back(line);
                if lines.len() > n { lines.pop_front(); }
            }
            Ok(_) => {}
            Err(e) => { eprintln!("buckets ledger: {e}"); break; }
        }
    }
    lines.into_iter().rev().collect()
}

fn append(home: &Path, value: Value) -> std::io::Result<()> {
    let _lock = LEDGER.lock().unwrap();
    std::fs::create_dir_all(home)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(home.join("buckets.jsonl"))?;
    let mut line = value.to_json();
    line.push('\n');
    file.write_all(line.as_bytes())
}

fn is_unit(unit: &str) -> bool {
    let mut words = unit.split_whitespace();
    match (words.next(), words.next()) {
        (Some("check" | "present"), Some(_)) => true,
        (Some("suite"), Some(_)) => words.next().is_none(),
        (Some("devhub"), Some("makepad-devhub")) => words.next().is_none(),
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn range(repo: &Path, base: &str, tip: &str) -> Result<std::sync::Arc<std::collections::HashSet<String>>, String> {
    crate::gitread::Pass::default().range(repo, base, tip)
}

/// The landings between two tips: a commercial-only landing moves main, not work.
fn landings(repo: &Path, base: &Pair, tip: &Pair, entries: &[Entry]) -> Result<Vec<Entry>, String> {
    let mut pass = crate::gitread::Pass::default();
    let work = pass.range(repo, &base.work, &tip.work)?;
    let main = pass.range(&repo.join("apps/commercial"), &base.main, &tip.main)?;
    Ok(entries.iter().filter(|e| entry_pair(&e.landed).is_some_and(|p| work.contains(&p.work) || main.contains(&p.main))).cloned().collect())
}

fn attribute(repo: &Path, entries: &[Entry], failing: &[String]) -> (Vec<Vec<String>>, bool) {
    let units: Vec<String> = failing.iter().filter(|u| is_unit(u)).cloned().collect();
    let mut keys = HashMap::new();
    for e in entries {
        for pair in [entry_pair(&e.pre), entry_pair(&e.landed)].into_iter().flatten() {
            keys.entry(pair.clone()).or_insert_with(|| crate::gateproof::keys(repo, &pair, &units));
        }
    }
    let mut attributed = vec![Vec::new(); entries.len()];
    let mut ambiguous = false;
    for unit in failing {
        let changed: Vec<usize> = if is_unit(unit) {
            entries.iter().enumerate().filter_map(|(i, e)| {
                let pre = entry_pair(&e.pre)?;
                let landed = entry_pair(&e.landed)?;
                let before = keys.get(&pre)?.get(unit)?;
                let after = keys.get(&landed)?.get(unit)?;
                (before != after).then_some(i)
            }).collect()
        } else { Vec::new() };
        if changed.is_empty() {
            ambiguous = true;
            for units in &mut attributed { units.push(unit.clone()); }
        } else {
            for i in changed { attributed[i].push(unit.clone()); }
        }
    }
    (attributed, ambiguous)
}

/// Each bucket owns one timer and runs synchronously on its own thread.
pub fn spawn(name: &str, timing: Timing, repo: PathBuf, home: PathBuf, publish: Publish, run: Run, prior: Prior) {
    let name = name.to_string();
    let thread = std::thread::Builder::new().name(format!("devhub-bucket-{name}"));
    if let Err(e) = thread.spawn(move || {
        let machine = makepad_devhub_conf::default_machine_name();
        let store = match Store::open_quiet(&home, "") { Ok(s) => s, Err(e) => { eprintln!("devhub bucket: {e}"); return; } };
        let latest = ledger(&home, usize::MAX).iter().find_map(|line| {
            let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
            (v.get("bucket")?.as_str()? == name).then_some(v)
        });
        let mut last = latest.as_ref().and_then(|v| read_pair(v.get("tip")?));
        let mut last_start = latest.as_ref().and_then(|v| v.get("started_ms")?.as_u64()).unwrap_or(0);
        let logs = home.join("bucket-logs");
        let record = |started: u64, base: Option<Pair>, tip: Pair, log: PathBuf| -> Result<Pair, String> {
            let history = store.history("land")?.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect::<Vec<_>>();
            let entries = || history.clone();
            let base = base.or_else(|| {
                let recent = Duration::from_secs(60 * 60).as_millis();
                let mut entries = entries();
                entries.sort_by_key(|e| (e.changed_ms, e.id));
                entries.iter().filter(|e| u128::from(started.saturating_sub(e.changed_ms)) <= recent).find_map(|e| entry_pair(&e.pre))
            });
            let mut outcome = if base.is_some() {
                std::fs::create_dir_all(&logs).map_err(|e| format!("bucket logs: {e}"))?;
                run(base.as_ref(), &tip, &log)
            } else {
                Outcome { ok: true, failing: Vec::new(), log: String::new(), summary: "baseline".into(), lost: false }
            };
            if outcome.lost {
                publish(hub_record(&machine, "", 0, "bucket", Sev::Warn, format!("bucket {name} {}: no verdict: its lease was lost under the gate twice (the fleet, not the landings; {}); it runs again at its next turn", tip.short(), outcome.log), String::new()));
                return Err(format!("no verdict on {}: lease lost twice", tip.short()));
            }
            let ended = wall_ms();
            let (entries, attribution_error) = match &base {
                Some(base) => match landings(&repo, base, &tip, &entries()) {
                    Ok(entries) => (entries, None),
                    Err(e) => (Vec::new(), Some(format!("landing attribution: {e}"))),
                },
                None => (Vec::new(), None),
            };
            if let Some(e) = &attribution_error {
                outcome.summary.push_str(&format!("; {e}"));
            }
            let (units, mut ambiguous) = if outcome.ok { (vec![Vec::new(); entries.len()], false) }
                else { attribute(&repo, &entries, &outcome.failing) };
            ambiguous |= attribution_error.is_some();
            let rows = entries.iter().zip(&units).map(|(e, units)| obj(vec![
                ("id", Value::Int(e.id as i64)), ("label", s(&e.label())), ("agent", s(&e.agent)),
                ("land_to_verdict_ms", Value::Int(ended.saturating_sub(e.changed_ms) as i64)),
                ("units", Value::Arr(units.iter().map(|u| s(u)).collect())),
            ])).collect();
            append(&home, obj(vec![
                ("bucket", s(&name)), ("base", base.as_ref().map_or(Value::Null, pair_value)), ("tip", pair_value(&tip)),
                ("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(ended as i64)),
                ("ok", Value::Bool(outcome.ok)), ("failing", Value::Arr(outcome.failing.iter().map(|u| s(u)).collect())),
                ("summary", s(&outcome.summary)), ("log", s(&outcome.log)), ("landings", Value::Arr(rows)),
                ("ambiguous", Value::Bool(ambiguous)),
            ])).map_err(|e| format!("bucket ledger: {e}"))?;
            let latency = entries.iter().map(|e| e.changed_ms).max().map(|landed| {
                format!("land->verdict {} s; ", ended.saturating_sub(landed) / 1000)
            }).unwrap_or_default();
            let (sev, line, detail) = if outcome.ok {
                (Sev::Info, format!("bucket {name} {} ok ({latency}{})", tip.short(), outcome.summary), String::new())
            } else {
                let blamed = entries.iter().zip(&units).flat_map(|(e, units)| units.iter().map(move |u| {
                    format!("{} ({}) changed {u}", e.label(), e.agent)
                })).collect::<Vec<_>>().join("; ");
                let log = std::fs::read_to_string(&outcome.log).unwrap_or_default();
                let mut detail = crate::land::failure_lines(&log);
                if let Some(e) = attribution_error { detail.push(e); }
                (Sev::Err, format!("bucket {name} {} FAILED {}: {blamed}{} ({latency}{})", tip.short(), outcome.failing.join(", "), if ambiguous { " (ambiguous)" } else { "" }, outcome.summary), detail.join("\n"))
            };
            publish(hub_record(&machine, "", 0, "bucket", sev, line, detail));
            Ok(tip)
        };
        let prefix = format!("{name}-");
        let unfinished = std::fs::read_dir(&logs).ok().into_iter().flatten().filter_map(|entry| {
            let entry = entry.ok()?;
            let file = entry.file_name();
            let started: u64 = file.to_str()?.strip_prefix(&prefix)?.strip_suffix(".log.run")?.parse().ok()?;
            (latest.is_none() || started > last_start).then_some((started, entry.path()))
        }).max_by_key(|(started, _)| *started);
        if let Some((ms, sidecar)) = unfinished {
            if let Some((pid, started, base, tip)) = read_run(&sidecar).filter(|(_, started, _, _)| *started == ms) {
                publish(hub_record(&machine, "", 0, "bucket", Sev::Info,
                    format!("bucket {name} adopts its run of {} from before the master restart (pid {pid})", tip.short()), String::new()));
                if let Err(e) = record(started, Some(base), tip.clone(), sidecar.with_extension("")) {
                    eprintln!("bucket {name}: {e}");
                }
                last = Some(tip);
                last_start = started;
            }
        }
        // a newer run of its kind than its ledger has (its asker died with a master) is its last
        if let Some((started, tip)) = store.read(prior).filter(|(started, _)| *started > last_start) {
            last_start = started;
            last = Some(tip);
        }
        // the tip at this thread's first look is no change; one it sees change later is (a push
        // that is no landing)
        let (mut seen, mut changed, mut unlanded) = (None::<Pair>, 0, None::<u64>);
        let poll = Duration::from_secs(if timing.quiet < Duration::from_secs(5) { 1 } else { 5 });
        loop {
            std::thread::sleep(poll);
            let Some(tip) = crate::orch::snapshot(&repo) else { continue };
            let now = wall_ms();
            if seen.as_ref().is_some_and(|s| *s != tip) { changed = now; }
            seen = Some(tip.clone());
            if last.as_ref() == Some(&tip) {
                unlanded = None;
                continue;
            }
            let landed = landed_ms(&store);
            let first = landed.iter().copied().filter(|ms| *ms > last_start).min().unwrap_or_else(|| *unlanded.get_or_insert(now));
            let last_changed = landed.iter().copied().max().unwrap_or(0).max(changed);
            if now < starts_at(&timing, last_start, first, last_changed) { continue; }
            let started = now;
            last_start = started;
            unlanded = None;
            match record(started, last.clone(), tip, logs.join(format!("{name}-{started}.log"))) {
                Ok(tip) => last = Some(tip),
                Err(e) => eprintln!("bucket {name}: {e}"),
            }
        }
    }) { eprintln!("bucket thread: {e}"); }
}

pub fn fast(repo: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    Arc::new(move |base, tip, log| {
        let (units, why) = proofs.proven(&repo, tip);
        let base = base.expect("fast bucket needs a base");
        // its presents share these cores (land_gate.sh step_qpresent): what the box sets aside for them
        let lease = ["lease", "gate", "--needs", GATE_NEEDS, "--cores", "8", "--lane", "landq-fast", "--prio", "0", "--rev", &tip.rev(), "--"];
        let proven = format!("GATE_PROVEN={}", units.join(";"));
        let gate = ["env", &proven, "bash", "tools/run/fixture/land_gate.sh", "fast", &base.work, &base.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &why, log, base, tip);
        proofs.record_log(&repo, tip, &text, &format!("bucket fast {}", tip.short()));
        verdict(status, &text, log)
    })
}

/// Boots the apps the landings touched on a Linux node's warm JIT (one that presents), right
/// after each landing. The Macs take only work that requires macOS.
pub fn hot(repo: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        let base = base.expect("hot bucket needs a base");
        // a warm boot of one or two apps needs far less than a test lease's default 6 cores / 7 GB
        let needs = format!("{GATE_NEEDS},gui=1");
        let lease = ["lease", "test", "--needs", &needs, "--cost", "small", "--cores", "3", "--ram-gb", "5", "--priority", "interactive", "--lane", "landq-hot", "--rev", &tip.rev(), "--"];
        let gate = ["bash", "tools/run/fixture/land_gate.sh", "hot", &base.work, &base.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &[], log, base, tip);
        verdict(status, &text, log)
    })
}

/// Runs land_gate.sh under a devhub lease, its output in `log` after the `why` lines. A lost
/// lease ([`crate::lease::client::LOST_EXIT`]) runs it once more in the same log, after
/// [`RERUN`]; the text is the last run's.
fn lease_run(repo: &Path, lease: &[&str], gate: &[&str], why: &[String], log: &Path, base: &Pair, tip: &Pair) -> (Result<Option<i32>, String>, String) {
    let sidecar = log.with_extension("log.run");
    let execute = |again: bool| -> Result<Option<i32>, String> {
        if sidecar.exists() {
            let (pid, _, _, _) = read_run(&sidecar).ok_or_else(|| format!("invalid bucket run {}", sidecar.display()))?;
            loop {
                if let Some(code) = crate::procinfo::exited(pid) { return Ok(code); }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let started = log_started(log).ok_or_else(|| format!("invalid bucket log {}", log.display()))?;
        let own = crate::cli::own_argv(repo);
        let mut file = if again { std::fs::OpenOptions::new().append(true).open(log) } else { std::fs::File::create(log) }.map_err(|e| e.to_string())?;
        if !again { for line in why { writeln!(file, "{line}").map_err(|e| e.to_string())?; } }
        let err = file.try_clone().map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(&own[0]).args(&own[1..]).args(lease).args(gate)
            .current_dir(repo).env("DEVHUB_AGENT", "devhub-bucket")
            .stdin(std::process::Stdio::null()).stdout(file).stderr(err).spawn().map_err(|e| e.to_string())?;
        let saved = save_run(log, child.id(), started, base, tip);
        let status = child.wait().map(|s| s.code()).map_err(|e| e.to_string());
        saved.map_err(|e| format!("bucket run: {e}"))?;
        status
    };
    let note = |line: &str| if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) { let _ = writeln!(file, "{line}"); };
    let mut status = execute(false);
    if again(&status, &std::fs::read_to_string(log).unwrap_or_default()) {
        note(RERUN);
        let _ = std::fs::remove_file(&sidecar);
        status = execute(true);
    }
    if let Err(e) = &status { note(&format!("gate lease: {e}")); }
    let text = std::fs::read_to_string(log).unwrap_or_default();
    (status, last_run(&text).to_string())
}

fn lost(status: &Result<Option<i32>, String>) -> bool {
    matches!(status, Ok(Some(code)) if *code == crate::lease::client::LOST_EXIT)
}

/// A lost lease's gate runs once more: not when its log says it already did.
fn again(status: &Result<Option<i32>, String>, log: &str) -> bool {
    lost(status) && !log.lines().any(|l| l == RERUN)
}

/// The last run's part of a log.
fn last_run(text: &str) -> &str {
    text.rsplit_once(&format!("{RERUN}\n")).map_or(text, |(_, last)| last)
}

/// A run passes on exit 0 with NODEGATE-OK and no failing unit.
fn verdict(status: Result<Option<i32>, String>, text: &str, log: &Path) -> Outcome {
    let lost_run = lost(&status);
    let mut failing: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("GATEUNIT fail ").map(str::to_string)).collect();
    let ok = failing.is_empty() && status.as_ref().is_ok_and(|s| *s == Some(0)) && text.lines().any(|l| l.starts_with("NODEGATE-OK"));
    if !ok && failing.is_empty() {
        for line in text.lines().filter_map(|l| l.strip_prefix("NODEGATE-FAIL")) {
            let steps = line.trim_start_matches(':').split(" [").next().unwrap_or("");
            failing.extend(steps.split_whitespace().map(str::to_string));
        }
        if failing.is_empty() {
            failing.push(match status { Ok(code) => format!("no verdict (exit {code:?})"), Err(e) => format!("gate lease: {e}") });
        }
    }
    failing.sort();
    failing.dedup();
    let summary = text.lines().filter(|l| l.starts_with("STEP ")).collect::<Vec<_>>().join(", ");
    Outcome { ok, failing, log: log.display().to_string(), summary, lost: lost_run }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_lease_runs_the_gate_once_more_and_lost_again_is_no_verdict() {
        let lost_exit = Ok(Some(crate::lease::client::LOST_EXIT));
        assert!(again(&lost_exit, "why\nL1 test@101: granted\n"));
        assert!(!again(&lost_exit, &format!("why\n{RERUN}\nL2 test@102: granted\n")), "only once, also for a run adopted after a master restart");
        assert!(!again(&Ok(Some(1)), "GATEUNIT fail check x\n"), "a failed gate is a verdict");
        // the verdict reads the last run only: the lost run's lines are gone with it
        let text = format!("why\nSTEP jit 9 s\n{RERUN}\nSTEP jit 3 s\nNODEGATE-OK\n");
        assert_eq!(last_run(&text), "STEP jit 3 s\nNODEGATE-OK\n");
        let o = verdict(Ok(Some(0)), last_run(&text), Path::new("x.log"));
        assert!(o.ok && !o.lost && o.summary == "STEP jit 3 s", "{:?} {}", o.failing, o.summary);
        // lost twice: no verdict, nothing for the landings to answer for
        assert!(verdict(lost_exit, "STEP affected 0 s\n", Path::new("x.log")).lost);
        assert!(!verdict(Ok(Some(1)), "", Path::new("x.log")).lost);
    }

    #[test]
    fn a_bucket_starts_from_its_last_start_and_landings_and_the_cap_is_a_cap() {
        let (min, h) = (60_000u64, 3_600_000u64);
        let ci = Timing { every: Duration::ZERO, quiet: Duration::from_secs(15 * 60), cap: Duration::from_secs(3 * 3600) };
        // last run at 0, a landing every 10 min from 10 min on: never 15 min quiet, so it starts
        // 3 h after that first landing (a master restart changes none of the inputs)
        let first_start = (10 * min..5 * h).step_by(min as usize).find(|now| *now >= starts_at(&ci, 0, 10 * min, now / (10 * min) * (10 * min)));
        assert_eq!(first_start, Some(10 * min + 3 * h));
        // quiet for 15 min: then
        assert_eq!(starts_at(&ci, 0, 10 * min, 40 * min), 55 * min);
        // a run with no ledger row and no run of its kind known: due since its first landing, long
        // past, so now
        assert!(starts_at(&ci, 0, 1, 1_000 * min) <= 3 * h + 1);
        let hot = Timing { every: Duration::ZERO, quiet: Duration::ZERO, cap: Duration::ZERO };
        assert_eq!(starts_at(&hot, 5, 9, 9), 9, "hot: at the landing");
        let fast = Timing { every: Duration::ZERO, quiet: Duration::from_secs(60), cap: Duration::from_secs(600) };
        assert_eq!(starts_at(&fast, 0, 100, 100), 100 + min, "a minute after the last landing");
        assert_eq!(starts_at(&fast, 0, 100, 100 + 20 * min), 100 + 10 * min, "at most 10 min after the first");
        let every = Timing { every: Duration::from_secs(3600), quiet: Duration::ZERO, cap: Duration::ZERO };
        assert_eq!(starts_at(&every, 1_000, 2_000, 2_000), 1_000 + h, "not before every after its last start");
        assert_eq!(log_started(Path::new("/x/bucket-logs/ci-1793673952236.log")), Some(1793673952236));
        assert_eq!(log_started(Path::new("/x/ci.log")), None);
    }
}
