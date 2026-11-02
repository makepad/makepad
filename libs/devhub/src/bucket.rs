//! Buckets check landed tips after a quiet minute, with a cap if merges keep coming.
//! One run covers all landings since that bucket's last tip.
//! Failures name the lanes whose inputs changed; they never control landing.

use crate::land::{Entry, St};
use crate::proto::{hub_record, wall_ms, Sev};
use crate::state::{Pair, Store};
use makepad_strict_json::{obj, s, Value};
use std::collections::{HashMap, HashSet, VecDeque};
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

pub struct Outcome {
    pub ok: bool,
    pub failing: Vec<String>,
    pub log: String,
    pub summary: String,
}

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

fn range(repo: &Path, base: &str, tip: &str) -> Result<HashSet<String>, String> {
    crate::orch::git(repo, &["rev-list", &format!("{base}..{tip}")]).map(|s| s.lines().map(str::to_string).collect())
}

fn landings(repo: &Path, base: &Pair, tip: &Pair, entries: &[Entry]) -> Result<Vec<Entry>, String> {
    let work = range(repo, &base.work, &tip.work)?;
    Ok(entries.iter().filter(|e| entry_pair(&e.landed).is_some_and(|p| work.contains(&p.work))).cloned().collect())
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
pub fn spawn(name: &str, timing: Timing, repo: PathBuf, home: PathBuf, publish: Publish, run: Run) {
    let name = name.to_string();
    let thread = std::thread::Builder::new().name(format!("devhub-bucket-{name}"));
    if let Err(e) = thread.spawn(move || {
        let machine = crate::proto::default_machine_name();
        let store = Store::open_quiet(&home, "");
        let latest = ledger(&home, usize::MAX).iter().find_map(|line| {
            let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
            (v.get("bucket")?.as_str()? == name).then_some(v)
        });
        let mut last = latest.as_ref().and_then(|v| read_pair(v.get("tip")?));
        let mut last_start = latest.as_ref().and_then(|v| v.get("started_ms")?.as_u64()).unwrap_or(0);
        let mut seen = last.clone();
        let mut last_merge = wall_ms();
        let mut due_since = None;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let Some(tip) = crate::orch::snapshot(&repo) else { continue };
            let now = wall_ms();
            if seen.as_ref() != Some(&tip) {
                seen = Some(tip.clone());
                last_merge = now;
            }
            if last.as_ref() == Some(&tip) {
                due_since = None;
                continue;
            }
            let due = *due_since.get_or_insert_with(|| last_start.saturating_add(timing.every.as_millis() as u64).max(now));
            if now < due || (Duration::from_millis(now.saturating_sub(last_merge)) < timing.quiet
                && Duration::from_millis(now.saturating_sub(due)) < timing.cap) { continue; }
            let started = now;
            last_start = started;
            due_since = None;
            let tick = || -> Result<Pair, String> {
                let entries = || store.read(|d| d.land.clone()).iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect::<Vec<_>>();
                let base = last.clone().or_else(|| {
                    let recent = Duration::from_secs(60 * 60).as_millis();
                    let mut entries = entries();
                    entries.sort_by_key(|e| (e.changed_ms, e.id));
                    entries.iter().filter(|e| u128::from(started.saturating_sub(e.changed_ms)) <= recent).find_map(|e| entry_pair(&e.pre))
                });
                let mut outcome = if base.is_some() {
                    let logs = home.join("bucket-logs");
                    std::fs::create_dir_all(&logs).map_err(|e| format!("bucket logs: {e}"))?;
                    run(base.as_ref(), &tip, &logs.join(format!("{name}-{started}.log")))
                } else {
                    Outcome { ok: true, failing: Vec::new(), log: String::new(), summary: "baseline".into() }
                };
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
                    ("units", Value::Arr(units.iter().map(|u| s(u)).collect())),
                ])).collect();
                append(&home, obj(vec![
                    ("bucket", s(&name)), ("base", base.as_ref().map_or(Value::Null, pair_value)), ("tip", pair_value(&tip)),
                    ("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(ended as i64)),
                    ("ok", Value::Bool(outcome.ok)), ("failing", Value::Arr(outcome.failing.iter().map(|u| s(u)).collect())),
                    ("summary", s(&outcome.summary)), ("log", s(&outcome.log)), ("landings", Value::Arr(rows)),
                    ("ambiguous", Value::Bool(ambiguous)),
                ])).map_err(|e| format!("bucket ledger: {e}"))?;
                let (sev, line, detail) = if outcome.ok {
                    (Sev::Info, format!("bucket {name} {} ok ({})", tip.short(), outcome.summary), String::new())
                } else {
                    let blamed = entries.iter().zip(&units).flat_map(|(e, units)| units.iter().map(move |u| {
                        format!("{} ({}) changed {u}", e.label(), e.agent)
                    })).collect::<Vec<_>>().join("; ");
                    let log = std::fs::read_to_string(&outcome.log).unwrap_or_default();
                    let mut detail = crate::land::failure_lines(&log);
                    if let Some(e) = attribution_error { detail.push(e); }
                    (Sev::Err, format!("bucket {name} {} FAILED {}: {blamed}{}", tip.short(), outcome.failing.join(", "), if ambiguous { " (ambiguous)" } else { "" }), detail.join("\n"))
                };
                publish(hub_record(&machine, "", 0, "bucket", sev, line, detail));
                Ok(tip)
            };
            match tick() {
                Ok(tip) => last = Some(tip),
                Err(e) => eprintln!("bucket {name}: {e}"),
            }
        }
    }) { eprintln!("bucket thread: {e}"); }
}

pub fn fast(repo: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    Arc::new(move |base, tip, log| {
        let (units, why) = proofs.proven(&repo, tip);
        let own = crate::cli::own_argv(&repo);
        let execute = || -> std::io::Result<std::process::ExitStatus> {
            let mut file = std::fs::File::create(log)?;
            for line in &why { writeln!(file, "{line}")?; }
            let err = file.try_clone()?;
            let base = base.expect("fast bucket needs a base");
            std::process::Command::new(&own[0]).args(&own[1..])
                .args(["lease", "gate", "--needs", GATE_NEEDS, "--lane", "landq-fast", "--prio", "0", "--rev", &tip.rev(), "--"])
                .args(["env", &format!("GATE_PROVEN={}", units.join(";")), "bash", "tools/run/fixture/land_gate.sh", "fast", &base.work, &base.main])
                .current_dir(&repo).env("DEVHUB_AGENT", "devhub-bucket")
                .stdin(std::process::Stdio::null()).stdout(file).stderr(err).status()
        };
        let status = execute();
        if let Err(e) = &status {
            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) { let _ = writeln!(file, "gate lease: {e}"); }
        }
        let text = std::fs::read_to_string(log).unwrap_or_default();
        proofs.record_log(&repo, tip, &text, &format!("bucket fast {}", tip.short()));
        let ok = status.as_ref().is_ok_and(|s| s.success()) && text.lines().any(|l| l.starts_with("NODEGATE-OK"));
        let mut failing: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("GATEUNIT fail ").map(str::to_string)).collect();
        if !ok && failing.is_empty() {
            for line in text.lines().filter_map(|l| l.strip_prefix("NODEGATE-FAIL")) {
                let steps = line.trim_start_matches(':').split(" [").next().unwrap_or("");
                failing.extend(steps.split_whitespace().map(str::to_string));
            }
            if failing.is_empty() {
                failing.push(match status { Ok(s) => format!("no verdict (exit {:?})", s.code()), Err(e) => format!("gate lease: {e}") });
            }
        }
        failing.sort();
        failing.dedup();
        let summary = text.lines().filter(|l| l.starts_with("STEP ")).collect::<Vec<_>>().join(", ");
        Outcome { ok, failing, log: log.display().to_string(), summary }
    })
}
