//! Buckets check landed tips on their own timers.
//! One run covers all landings since that bucket's last tip.
//! Failures name the lanes whose inputs changed; they never control landing.
//! A unit failing the way it failed at the bucket's last look at it is a break already on the
//! branch: it names who broke it then, not the later landings that changed its inputs.
//! A failed unit stays red until it passes: the next run of its bucket looks at it again whatever
//! that run's diff reaches, and a run that did not get to it is no pass.
//! A restarted master adopts its bucket's unfinished run instead of starting it again.
//! When a bucket is due comes from what outlives the master: its last start (its ledger, or the
//! runs of its kind it knows of) and the landings since. A master restart never moves it: CI was
//! due again at each of them (every ~30 min) and never ran for 8 h (11-02).
//! The CI bucket runs once a night ([`Timing::daily`]): a run every ~3 h of a moving tip proved
//! little a night's run of the day's tip does not, and took the fleet's day (user, 11-05).
//! A run whose lease was lost under it (expired, its node lost it) has no verdict: it runs once
//! more at once, and lost again it is no verdict on the landings, only a warn: the bucket takes the
//! tip again at its next turn.

use crate::land::{Entry, St};
use crate::proto::{hub_record, wall_ms, Sev};
use crate::state::{Pair, Store};
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
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
    /// Once a day at this hour of the local clock, after a landing it has not checked: in place
    /// of `every`, `quiet` and `cap`.
    pub daily: Option<u32>,
}

impl Timing {
    /// Due at once after the last landing, waiting `quiet` for the tip to settle, at most `cap`.
    pub fn after(quiet: Duration, cap: Duration) -> Timing {
        Timing { every: Duration::ZERO, quiet, cap, daily: None }
    }
}

/// When a bucket starts its next run: due at `every` after its last start and not before the
/// first landing it has not checked; then once the tip stayed `quiet` since it last changed, and
/// at the latest `cap` after it was due, quiet or not. All of them outlive the master.
fn starts_at(t: &Timing, last_start: u64, first_landed: u64, last_changed: u64) -> u64 {
    if let Some(hour) = t.daily {
        return daily_after(last_start.max(first_landed), hour, &|secs| makepad_civil_time::local_or_utc(secs).offset);
    }
    let ms = |d: Duration| d.as_millis() as u64;
    let due = last_start.saturating_add(ms(t.every)).max(first_landed);
    due.max(last_changed.saturating_add(ms(t.quiet))).min(due.saturating_add(ms(t.cap)))
}

/// The first `hour`:00 on the local clock after `after_ms` (`offset`: seconds east of UTC at an
/// instant), with the offset at that hour (a daylight saving change in between).
fn daily_after(after_ms: u64, hour: u32, offset: &dyn Fn(i64) -> i32) -> u64 {
    let after = (after_ms / 1000) as i64;
    let local = after + offset(after) as i64;
    let mut at = local.div_euclid(86_400) * 86_400 + hour as i64 * 3600;
    if at <= local {
        at += 86_400;
    }
    let utc = at - offset(at - offset(after) as i64) as i64;
    utc.max(0) as u64 * 1000
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
    /// why this run says nothing about the tip (its lease lost twice, a CI run that stopped before
    /// its legs ran): nothing here is a failure of the landings, and it runs again at its next turn
    pub no_verdict: Option<String>,
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

pub(crate) fn entry_pair(text: &str) -> Option<Pair> {
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
        // `tests <pkg>`: its test targets' one cargo check (land_gate.sh step_testcheck)
        (Some("check" | "present" | "cargo" | "tests"), Some(_)) => true,
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

/// A unit's package: `check <pkg>` without its `--also=` flips (one break shows under both).
fn unit_key(unit: &str) -> String {
    unit.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// A compile error line as `<file>: <message>`: rustc's `<path>:<line>:<col>: ` gives way to the
/// file's name (the node's tree path, or an edit shifting the line, is no new error).
fn error_key(line: &str) -> Option<String> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let mut from = 0;
    while let Some(c) = line[from..].find(':') {
        let at = from + c;
        let rest = &line[at + 1..];
        let row = digits(rest);
        if row > 0 && rest[row..].starts_with(':') {
            let col = &rest[row + 1..];
            let n = digits(col);
            if n > 0 && col[n..].starts_with(": ") {
                let path = &line[..at];
                return Some(format!("{}: {}", path.rsplit('/').next().unwrap_or(path), &col[n + 2..]));
            }
        }
        from = at + 1;
    }
    None
}

/// A run's failing units with their compile errors (each unit's own output: the lines after the
/// unit before it), and the units that passed, by package (a proven one passed at these inputs).
fn unit_lines(text: &str) -> (BTreeMap<String, Vec<String>>, BTreeSet<String>) {
    let (mut errors, mut passed, mut own) = (BTreeMap::new(), BTreeSet::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            match rest.split_once(' ') {
                Some(("fail", unit)) if !own.is_empty() => {
                    let mut e = std::mem::take(&mut own);
                    e.sort();
                    e.dedup();
                    errors.insert(unit.to_string(), e);
                }
                // (`gone`: its package left the tree, nothing is left to fail)
                Some(("ok" | "proven" | "gone", unit)) => { passed.insert(unit_key(unit)); }
                _ => {}
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else if let Some(e) = error_key(line) {
            own.push(e);
        }
    }
    (errors, passed)
}

/// A run's units with warnings of their own (`GATEUNIT warn <unit>`, after its warning lines), by
/// package: each warning's key and its line, and the units it took as proven, not compiled: such a
/// run says nothing of their warnings.
fn warn_lines(text: &str) -> (BTreeMap<String, Vec<(String, String)>>, BTreeSet<String>) {
    let (mut warned, mut proven, mut own) = (BTreeMap::new(), BTreeSet::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            match rest.split_once(' ') {
                Some(("warn", unit)) => {
                    let mut w: Vec<(String, String)> = std::mem::take(&mut own);
                    w.sort();
                    w.dedup_by(|a, b| a.0 == b.0);
                    warned.insert(unit_key(unit), w);
                }
                Some(("proven", unit)) => { proven.insert(unit_key(unit)); }
                _ => {}
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else if let Some(key) = error_key(line) {
            own.push((key, line.to_string()));
        }
    }
    (warned, proven)
}

/// Unit `key`'s warnings `now` that it did not have at the bucket's last look at it (a run that
/// took it as proven did not look), or None when no row says what it had: its first look, or a
/// look from before rows kept warnings. Such a warning is of unknown origin, no landing's: a unit
/// that warned for weeks would name whoever changed its inputs next. `rows`: newest first.
fn new_warnings(rows: &[Value], key: &str, now: &[String]) -> Option<Vec<String>> {
    let lists = |list: &str, row: &Value| row.get(list).and_then(Value::as_arr).is_some_and(|l| l.iter().any(|u| u.as_str().is_some_and(|u| unit_key(u) == key)));
    let of = |map: Option<&Value>| match map {
        Some(Value::Obj(m)) => m.iter().find(|(u, _)| unit_key(u) == key).map(|(_, w)| w.as_arr().unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()),
        _ => None,
    };
    for row in rows {
        let before = if row.get("warned").is_none() {
            if lists("passed", row) || lists("failing", row) { return None; }
            continue;
        } else if let Some(warned) = of(row.get("warned")) {
            warned
        } else if lists("proven", row) {
            continue;
        } else if lists("passed", row) {
            Vec::new()
        } else if let Some(errors) = of(row.get("errors")) {
            errors
        } else if lists("failing", row) {
            return None;
        } else {
            continue;
        };
        return Some(now.iter().filter(|w| !before.contains(w)).cloned().collect());
    }
    None
}

/// The failing units already broken at this bucket's last look at them, with who broke them: a
/// unit whose every error now was in its last failure, with no run of the bucket passing it
/// since. A later landing that changed such a unit's inputs did not break it (one landing broke a
/// phone host check, and each later devhub landing was named for it, 11-03). A unit with no
/// compile error (a present, a test run) failing again is the same break, and so is a failure in
/// a row that kept no errors to compare with: rechecked a day later, one was named for whatever
/// had just landed (11-04). `rows`: the bucket's earlier rows, newest first.
fn carried_since(rows: &[Value], failing: &[String], errors: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for unit in failing.iter().filter(|u| is_unit(u)) {
        let key = unit_key(unit);
        let now = errors.get(unit).map_or(&[][..], Vec::as_slice);
        for row in rows {
            if row.get("passed").and_then(Value::as_arr).is_some_and(|p| p.iter().any(|k| k.as_str() == Some(key.as_str()))) { break; }
            let failed = match row.get("errors") {
                Some(Value::Obj(failed)) => failed.iter().find(|(u, _)| unit_key(u) == key),
                _ => None,
            };
            let listed = row.get("failing").and_then(Value::as_arr).is_some_and(|f| f.iter().any(|u| u.as_str().is_some_and(|u| unit_key(u) == key)));
            if failed.is_none() && !listed { continue; }
            let before: Vec<&str> = failed.and_then(|(_, b)| b.as_arr()).unwrap_or_default().iter().filter_map(Value::as_str).collect();
            if row.get("errors").is_none() || now.iter().all(|e| before.contains(&e.as_str())) {
                out.insert(unit.clone(), origin(rows, &key));
            }
            break;
        }
    }
    out
}

/// Who broke unit `key`, failing with no pass since: what the first run to fail it said, else that
/// nobody knows, with that run's log. A later run's name is no better: it named who changed the
/// unit then. `rows`: the bucket's earlier rows, newest first.
fn origin(rows: &[Value], key: &str) -> String {
    let lists = |list: &str, row: &Value| row.get(list).and_then(Value::as_arr).is_some_and(|l| l.iter().any(|u| u.as_str().is_some_and(|u| unit_key(u) == key)));
    let failed = |row: &Value| lists("failing", row) || matches!(row.get("errors"), Some(Value::Obj(e)) if e.iter().any(|(u, _)| unit_key(u) == key));
    let first = rows.iter().take_while(|row| !lists("passed", row)).filter(|row| failed(row)).last();
    if let Some(who) = first.and_then(|row| row.get("broke")).and_then(|b| b.get(key)).and_then(Value::as_str) {
        return who.to_string();
    }
    let log = first.and_then(|row| row.get("log")).and_then(Value::as_str).and_then(|l| Path::new(l).file_name()).map(|l| l.to_string_lossy().into_owned());
    log.map_or(ORIGIN_UNKNOWN.to_string(), |log| format!("{ORIGIN_UNKNOWN}, first failed in {log}"))
}

/// A failure older than what the ledger can name a landing for.
pub const ORIGIN_UNKNOWN: &str = "origin unknown";

/// `<unit> still failing since <who>`, for a `who` from [`origin`] or a row.
pub fn still_since(who: &str) -> String {
    if who.starts_with(ORIGIN_UNKNOWN) { who.to_string() } else { format!("since {who}") }
}

/// The buckets whose next run looks again at what failed (land_gate.sh's GATE_RETRY). CI needs no
/// telling: a shard with no proof at its inputs runs; prebuild lists its legs under each new JIT.
const RERUNS: [&str; 2] = ["fast", "hot"];

/// Bucket `name`'s rows, newest first.
fn rows_of(home: &Path, name: &str) -> Vec<Value> {
    ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter(|v| v.get("bucket").and_then(Value::as_str) == Some(name)).collect()
}

/// The units whose last look ended in a failure, by package, with who broke them and the log of
/// that look. A run that never looked at a unit says nothing about it: one whose diff touched
/// nothing said ok and a real failure looked fixed (11-04). `rows`: the bucket's rows, newest first.
fn still_failing(rows: &[Value]) -> BTreeMap<String, (String, String, String)> {
    let (mut settled, mut out) = (BTreeSet::new(), BTreeMap::new());
    for (i, row) in rows.iter().enumerate() {
        for unit in row.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).filter(|u| is_unit(u)) {
            let key = unit_key(unit);
            if settled.contains(&key) { continue; }
            let who = origin(&rows[i..], &key);
            let log = row.get("log").and_then(Value::as_str).unwrap_or("").to_string();
            out.insert(key, (unit.to_string(), who, log));
        }
        settled.extend(out.keys().cloned());
        settled.extend(row.get("passed").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string));
    }
    out
}

/// What the next run of a bucket looks at whatever its diff reaches, as land_gate.sh's GATE_RETRY:
/// the units still failing and, after a step that failed with no unit to name (the JIT that did not
/// build), `steps`: the gate takes the JIT even if every unit is proven.
fn retry_env(rows: &[Value]) -> String {
    let mut units: Vec<String> = still_failing(rows).into_values().map(|(unit, _, _)| unit).collect();
    let step = rows.first().and_then(|r| r.get("failing")).and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).any(|u| !is_unit(u));
    if step { units.push("steps".to_string()); }
    format!("GATE_RETRY={}", units.join(";"))
}

/// `devhub bucket`'s lines: what each bucket still owes a pass, then the last runs, newest first.
pub fn status(home: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for name in RERUNS {
        let still = still_failing(&rows_of(home, name));
        if still.is_empty() { continue; }
        // and where each failed last: a green kept log beside a red bucket read as a miscount (11-05)
        out.push(obj(vec![("still_failing", s(name)), ("units", Value::Obj(still.values().map(|(unit, who, _)| (unit.clone(), s(who))).collect())),
            ("logs", Value::Obj(still.into_values().map(|(unit, _, log)| (unit, s(&log))).collect()))]).to_json());
    }
    out.extend(ledger(home, 10));
    out
}

/// Bucket `name`'s verdict on `tip`, from the outcome `run` reaches from the base (the last tip it
/// judged, else the oldest base of the last hour's landings): the landings since the base named
/// for the units they changed, a ledger row and a feed line (`emit`). Err: no verdict.
#[allow(clippy::too_many_arguments)]
fn judge(name: &str, home: &Path, repo: &Path, store: &Store, emit: &dyn Fn(Sev, String, String), started: u64, base: Option<Pair>, tip: &Pair,
    run: impl FnOnce(Option<&Pair>) -> Result<Outcome, String>) -> Result<(), String> {
    let history = store.history("land")?.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect::<Vec<_>>();
    let entries = || history.clone();
    let base = base.or_else(|| {
        let recent = Duration::from_secs(60 * 60).as_millis();
        let mut entries = entries();
        entries.sort_by_key(|e| (e.changed_ms, e.id));
        entries.iter().filter(|e| u128::from(started.saturating_sub(e.changed_ms)) <= recent).find_map(|e| entry_pair(&e.pre))
    });
    let mut outcome = run(base.as_ref())?;
    if let Some(why) = &outcome.no_verdict {
        emit(Sev::Warn, format!("bucket {name} {}: no verdict: {why} ({}); it runs again at its next turn", tip.short(), outcome.log), String::new());
        return Err(format!("no verdict on {}: {why}", tip.short()));
    }
    let ended = wall_ms();
    let (entries, attribution_error) = match &base {
        Some(base) => match landings(repo, base, tip, &entries()) {
            Ok(entries) => (entries, None),
            Err(e) => (Vec::new(), Some(format!("landing attribution: {e}"))),
        },
        None => (Vec::new(), None),
    };
    if let Some(e) = &attribution_error {
        outcome.summary.push_str(&format!("; {e}"));
    }
    let text = std::fs::read_to_string(&outcome.log).unwrap_or_default();
    let (errors, passed) = unit_lines(last_run(&text));
    let (warned, proven) = warn_lines(last_run(&text));
    let earlier = rows_of(home, name);
    let carried = carried_since(&earlier, &outcome.failing, &errors);
    // what this run did not get to (a gate that stopped at the JIT): red still, whatever else it says
    let still: BTreeMap<String, (String, String)> = if RERUNS.contains(&name) {
        still_failing(&earlier).into_iter().filter(|(key, _)| !passed.contains(key) && !outcome.failing.iter().any(|u| unit_key(u) == *key)).map(|(_, (unit, who, log))| (unit, (who, log))).collect()
    } else { BTreeMap::new() };
    outcome.ok &= still.is_empty();
    let blame: Vec<String> = outcome.failing.iter().filter(|u| !carried.contains_key(*u)).cloned().collect();
    let (units, mut ambiguous) = if blame.is_empty() { (vec![Vec::new(); entries.len()], false) }
        else { attribute(repo, &entries, &blame) };
    ambiguous |= attribution_error.is_some();
    // who broke each failing unit: for the next run of the bucket to name, should it still fail
    let mut broke: BTreeMap<String, String> = BTreeMap::new();
    for unit in outcome.failing.iter().filter(|u| is_unit(u)) {
        let who = carried.get(unit).cloned().unwrap_or_else(|| entries.iter().zip(&units).filter(|(_, us)| us.contains(unit))
            .map(|(e, _)| format!("{} ({}) #{}", e.label(), e.agent, e.id)).collect::<Vec<_>>().join(", "));
        if !who.is_empty() { broke.entry(unit_key(unit)).or_insert(who); }
    }
    // a warning a unit did not have at its last look: zero warnings is the rule, so a WARN to the
    // landings that changed the unit, the bucket green (a test target's unused const, 11-05)
    let mut fresh: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unknown: Vec<String> = Vec::new();
    for (key, lines) in &warned {
        let now: Vec<String> = lines.iter().map(|(k, _)| k.clone()).collect();
        match new_warnings(&earlier, key, &now) {
            Some(new) if new.is_empty() => {}
            Some(new) => { fresh.insert(key.clone(), new); }
            None => unknown.push(key.clone()),
        }
    }
    let warn_units: Vec<String> = fresh.keys().cloned().collect();
    let (warn_by, warn_ambiguous) = if warn_units.is_empty() || attribution_error.is_some() { (vec![Vec::new(); entries.len()], attribution_error.is_some()) }
        else { attribute(repo, &entries, &warn_units) };
    let mut warns: BTreeMap<String, String> = unknown.iter().map(|k| (k.clone(), ORIGIN_UNKNOWN.to_string())).collect();
    for key in &warn_units {
        let who = entries.iter().zip(&warn_by).filter(|(_, us)| us.contains(key))
            .map(|(e, _)| format!("{} ({}) #{}", e.label(), e.agent, e.id)).collect::<Vec<_>>().join(", ");
        warns.insert(key.clone(), if who.is_empty() { ORIGIN_UNKNOWN.to_string() } else { who });
    }
    let rows = entries.iter().zip(&units).map(|(e, units)| obj(vec![
        ("id", Value::Int(e.id as i64)), ("label", s(&e.label())), ("agent", s(&e.agent)),
        ("land_to_verdict_ms", Value::Int(ended.saturating_sub(e.changed_ms) as i64)),
        ("units", Value::Arr(units.iter().map(|u| s(u)).collect())),
    ])).collect();
    append(home, obj(vec![
        ("bucket", s(name)), ("base", base.as_ref().map_or(Value::Null, pair_value)), ("tip", pair_value(tip)),
        ("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(ended as i64)),
        ("ok", Value::Bool(outcome.ok)), ("failing", Value::Arr(outcome.failing.iter().map(|u| s(u)).collect())),
        ("summary", s(&outcome.summary)), ("log", s(&outcome.log)), ("landings", Value::Arr(rows)),
        ("ambiguous", Value::Bool(ambiguous)),
        ("errors", Value::Obj(errors.iter().map(|(u, e)| (u.clone(), Value::Arr(e.iter().map(s).collect()))).collect())),
        ("passed", Value::Arr(passed.iter().map(s).collect())),
        ("carried", Value::Obj(carried.iter().map(|(u, who)| (u.clone(), s(who))).collect())),
        ("still", Value::Obj(still.iter().map(|(u, (who, _))| (u.clone(), s(who))).collect())),
        ("broke", Value::Obj(broke.into_iter().map(|(k, who)| (k, s(who))).collect())),
        ("warned", Value::Obj(warned.iter().map(|(u, w)| (u.clone(), Value::Arr(w.iter().map(|(k, _)| s(k)).collect()))).collect())),
        ("proven", Value::Arr(proven.iter().map(s).collect())),
        ("warns", Value::Obj(warns.iter().map(|(u, who)| (u.clone(), s(who))).collect())),
    ])).map_err(|e| format!("bucket ledger: {e}"))?;
    if !warns.is_empty() {
        let named = warns.iter().map(|(u, who)| if who == ORIGIN_UNKNOWN { format!("{u}: {ORIGIN_UNKNOWN} (its first look)") } else { format!("{who} changed {u}") }).collect::<Vec<_>>().join("; ");
        // each new warning as rustc said it; one of unknown origin, all of its unit's
        let fresh = &fresh;
        let detail = warned.iter().filter(|(u, _)| warns.contains_key(*u)).flat_map(|(u, lines)| lines.iter()
            .filter(move |(k, _)| fresh.get(u).is_none_or(|new| new.contains(k))).map(|(_, l)| crate::proto::clean(l, 300))).collect::<Vec<_>>();
        emit(Sev::Warn, format!("bucket {name} {} WARN new warnings in {}: {named}{}", tip.short(), warns.keys().cloned().collect::<Vec<_>>().join(", "), if warn_ambiguous { " (ambiguous)" } else { "" }), detail.join("\n"));
    }
    let latency = entries.iter().map(|e| e.changed_ms).max().map(|landed| {
        format!("land->verdict {} s; ", ended.saturating_sub(landed) / 1000)
    }).unwrap_or_default();
    let (sev, line, detail) = if outcome.ok {
        (Sev::Info, format!("bucket {name} {} ok ({latency}{})", tip.short(), outcome.summary), String::new())
    } else {
        let blamed = entries.iter().zip(&units).flat_map(|(e, units)| units.iter().map(move |u| {
            format!("{} ({}) changed {u}", e.label(), e.agent)
        // each with the log of its last look: this run's when it failed here, else the run that last looked
        })).chain(carried.iter().map(|(u, who)| (u, who, &outcome.log)).chain(still.iter().map(|(u, (who, log))| (u, who, log)))
            .map(|(u, who, log)| format!("{u} still failing {} (log {log})", still_since(who)))).collect::<Vec<_>>().join("; ");
        let mut detail = crate::land::failure_lines(&text);
        if let Some(e) = attribution_error { detail.push(e); }
        (Sev::Err, format!("bucket {name} {} FAILED {}: {blamed}{} ({latency}{})", tip.short(), outcome.failing.join(", "), if ambiguous { " (ambiguous)" } else { "" }, outcome.summary), detail.join("\n"))
    };
    emit(sev, line, detail);
    Ok(())
}

/// The prebuild legs' test listings of a landed tip under the JIT it brought (orch/warm.rs), as
/// bucket `prebuild`'s verdict: a listing that fails there is a break on the branch, named for the
/// landings that changed its package (only a warm WARN before, 11-03). `outcome.log` holds each
/// listing's output and its `GATEUNIT` line; `base` is the previous prebuild's tip.
pub fn prebuild(home: &Path, repo: &Path, store: &Store, started: u64, base: Option<Pair>, tip: &Pair, outcome: Outcome) -> Result<(), String> {
    let emit = |sev: Sev, line: String, detail: String| store.event("bucket", sev, line, Some(detail));
    judge("prebuild", home, repo, store, &emit, started, base, tip, |_| Ok(outcome))
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
        let emit = |sev: Sev, line: String, detail: String| publish(hub_record(&machine, "", 0, "bucket", sev, line, detail));
        let record = |started: u64, base: Option<Pair>, tip: Pair, log: PathBuf| -> Result<Pair, String> {
            judge(&name, &home, &repo, &store, &emit, started, base, &tip, |base| match base {
                Some(base) => {
                    std::fs::create_dir_all(&logs).map_err(|e| format!("bucket logs: {e}"))?;
                    Ok(run(Some(base), &tip, &log))
                }
                None => Ok(Outcome { ok: true, failing: Vec::new(), log: String::new(), summary: "baseline".into(), no_verdict: None }),
            })?;
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

pub fn fast(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    Arc::new(move |base, tip, log| {
        let (units, why) = proofs.proven(&repo, tip);
        let retry = retry_env(&rows_of(&home, "fast"));
        let base = base.expect("fast bucket needs a base");
        // its presents share these cores (land_gate.sh step_qpresent): what the box sets aside for them
        let lease = ["lease", "gate", "--needs", GATE_NEEDS, "--cores", "8", "--lane", "landq-fast", "--prio", "0", "--rev", &tip.rev()];
        let proven = format!("GATE_PROVEN={}", units.join(";"));
        let gate = ["env", &proven, &retry, "bash", "tools/run/fixture/land_gate.sh", "fast", &base.work, &base.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &why, log, base, tip);
        proofs.record_log(&repo, tip, &text, &format!("bucket fast {}", tip.short()));
        verdict(status, &text, log)
    })
}

/// Boots the apps the landings touched on a Linux node's warm JIT (one that presents), right
/// after each landing. The Macs take only work that requires macOS.
pub fn hot(repo: PathBuf, home: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        let base = base.expect("hot bucket needs a base");
        let retry = retry_env(&rows_of(&home, "hot"));
        // a warm boot of one or two apps needs far less than a test lease's default 6 cores / 7 GB
        let needs = format!("{GATE_NEEDS},gui=1");
        let lease = ["lease", "test", "--needs", &needs, "--cost", "small", "--cores", "3", "--ram-gb", "5", "--priority", "interactive", "--lane", "landq-hot", "--rev", &tip.rev()];
        let gate = ["env", &retry, "bash", "tools/run/fixture/land_gate.sh", "hot", &base.work, &base.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &[], log, base, tip);
        verdict(status, &text, log)
    })
}

/// Runs land_gate.sh under a devhub lease, its output in `log` after the `why` lines; a failed
/// step's logs, which go with the node's scratch at the lease's end, in `<log>.keep` beside it
/// (`lease --keep`). A lost lease ([`crate::lease::client::LOST_EXIT`]) runs it once more in the
/// same log, after [`RERUN`]; the text is the last run's.
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
        let mut child = std::process::Command::new(&own[0]).args(&own[1..]).args(lease).arg("--keep").arg(log.with_extension("keep")).arg("--").args(gate)
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
    Outcome { ok, failing, log: log.display().to_string(), summary, no_verdict: lost_run.then(|| "its lease was lost under the gate twice (the fleet, not the landings)".to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_lease_runs_the_gate_once_more_and_lost_again_is_no_verdict() {
        let lost_exit = Ok(Some(crate::lease::client::LOST_EXIT));
        assert!(again(&lost_exit, "why\nL1 test@node-a: granted\n"));
        assert!(!again(&lost_exit, &format!("why\n{RERUN}\nL2 test@node-b: granted\n")), "only once, also for a run adopted after a master restart");
        assert!(!again(&Ok(Some(1)), "GATEUNIT fail check x\n"), "a failed gate is a verdict");
        // the verdict reads the last run only: the lost run's lines are gone with it
        let text = format!("why\nSTEP jit 9 s\n{RERUN}\nSTEP jit 3 s\nNODEGATE-OK\n");
        assert_eq!(last_run(&text), "STEP jit 3 s\nNODEGATE-OK\n");
        let o = verdict(Ok(Some(0)), last_run(&text), Path::new("x.log"));
        assert!(o.ok && o.no_verdict.is_none() && o.summary == "STEP jit 3 s", "{:?} {}", o.failing, o.summary);
        // lost twice: no verdict, nothing for the landings to answer for
        assert!(verdict(lost_exit, "STEP affected 0 s\n", Path::new("x.log")).no_verdict.is_some());
        assert!(verdict(Ok(Some(1)), "", Path::new("x.log")).no_verdict.is_none());
    }

    #[test]
    fn a_break_already_on_the_branch_names_who_broke_it_not_the_next_lander() {
        let err = |at: &str| format!("/n/tree/tools/x/src/boot.rs:{at}: E0433: failed to resolve: use of undeclared type `crate::logcat`");
        // the landing that broke it: its run's errors and who it named
        let first = format!("STEP qcheck 1 s\nm: 0 errors\nGATEUNIT ok check m\n{}\n{}\nx: 2 errors\nGATEUNIT fail check x\nSTEP qpresent 1 s\n", err("102:9"), err("110:5"));
        let (errors, passed) = unit_lines(&first);
        assert_eq!(errors["check x"], vec!["boot.rs: E0433: failed to resolve: use of undeclared type `crate::logcat`".to_string()], "path and line give way to the file");
        assert!(passed.contains("check m") && !passed.contains("check x"));
        let row = |errors: &BTreeMap<String, Vec<String>>, passed: &[&str], broke: &str| obj(vec![
            ("errors", Value::Obj(errors.iter().map(|(u, e)| (u.clone(), Value::Arr(e.iter().map(s).collect()))).collect())),
            ("passed", Value::Arr(passed.iter().map(|p| s(*p)).collect())),
            ("broke", obj(vec![("check x", s(broke))])),
        ]);
        let broke = row(&errors, &["check m"], "rc-a (A) #970");
        // a later run checks it as a flip of another lib's build, an edit moved the line: the same break
        let later = format!("{}\nx: 1 errors\nGATEUNIT fail check x --also=y\n", err("130:9"));
        let (now, _) = unit_lines(&later);
        let failing = |e: &BTreeMap<String, Vec<String>>| e.keys().cloned().collect::<Vec<_>>();
        let carried = carried_since(&[obj(vec![]), broke.clone()], &failing(&now), &now);
        assert_eq!(carried.get("check x --also=y").map(String::as_str), Some("rc-a (A) #970"), "rows that never looked at it are skipped");
        // a run that passed it since: a failure now is new
        assert!(carried_since(&[row(&BTreeMap::new(), &["check x"], ""), broke.clone()], &failing(&now), &now).is_empty());
        // an error it did not have: new, the landings that changed its inputs answer for it
        let more = format!("{}\n/n/tree/tools/x/src/main.rs:3:1: E0425: cannot find value `z`\nGATEUNIT fail check x\n", err("102:9"));
        let more = unit_lines(&more).0;
        assert!(carried_since(&[broke], &failing(&more), &more).is_empty());
        assert_eq!(error_key("no position: here"), None);
        assert_eq!(error_key("C:/t/a.rs:1:2: warning: unused").as_deref(), Some("a.rs: warning: unused"));
    }

    /// Zero warnings is the rule: a test target's new warning is a WARN to who brought it, the bucket
    /// green. One it had at its last look is nobody's news; a first look cannot say whose it is.
    #[test]
    fn a_new_warning_warns_once_and_the_bucket_stays_green() {
        let warn = |at: &str, what: &str| format!("/n/tree/apps/f/tests/common/p.rs:{at}: warning: {what}");
        let log = format!("STEP depcheck 1 s\n{}\n{}\nGATEUNIT warn tests f\nGATEUNIT ok tests f\nGATEUNIT proven tests g\nGATEUNIT ok tests h\n",
            warn("5:11", "constant `MACHINE` is never used"), warn("5:11", "constant `MACHINE` is never used"));
        let (warned, proven) = warn_lines(&log);
        assert_eq!(warned["tests f"].iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["p.rs: warning: constant `MACHINE` is never used"], "each once");
        assert!(warned["tests f"][0].1.contains("p.rs:5:11"), "its line as rustc said it");
        assert!(proven.contains("tests g") && !warned.contains_key("tests h"));
        let (errors, passed) = unit_lines(&log);
        assert!(errors.is_empty() && passed.contains("tests f"), "a warning is no failure");
        let had = vec!["p.rs: warning: constant `MACHINE` is never used".to_string()];
        let more = vec![had[0].clone(), "q.rs: warning: unused import".to_string()];
        let row = |warned: &[(&str, &[String])], passed: &[&str], proven: &[&str]| obj(vec![
            ("warned", Value::Obj(warned.iter().map(|(u, w)| (u.to_string(), Value::Arr(w.iter().map(s).collect()))).collect())),
            ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())),
            ("proven", Value::Arr(proven.iter().map(|u| s(*u)).collect())),
        ]);
        let looked = row(&[("tests f", &had)], &["tests f"], &[]);
        assert_eq!(new_warnings(&[looked.clone()], "tests f", &had), Some(Vec::new()), "it had it at its last look");
        assert_eq!(new_warnings(&[looked.clone()], "tests f", &more), Some(vec!["q.rs: warning: unused import".to_string()]));
        // a run that took it as proven did not look; one that never reached it says nothing either
        let skipped = [row(&[], &["tests f"], &["tests f"]), row(&[], &["tests z"], &[]), looked.clone()];
        assert_eq!(new_warnings(&skipped, "tests f", &had), Some(Vec::new()));
        // a clean look since: the warning is new
        assert_eq!(new_warnings(&[row(&[], &["tests f"], &[]), looked], "tests f", &had), Some(had.clone()));
        // its first look, or a look from before rows kept warnings: nobody's to name
        assert_eq!(new_warnings(&[], "tests f", &had), None);
        assert_eq!(new_warnings(&[obj(vec![("passed", Value::Arr(vec![s("tests f")]))])], "tests f", &had), None);

        // the bucket's verdict: the first look warns of unknown origin, green; the same warning again is quiet
        use crate::proto::Record;
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-warn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fed: Arc<Mutex<Vec<Record>>> = Arc::default();
        let feed = fed.clone();
        let store = Store::open(&home, "mac", Arc::new(move |r: Record| { feed.lock().unwrap().push(r); 0 })).unwrap();
        let tip = Pair::new("a1", "b1");
        let judged = |text: &str| {
            let log = home.join("fast.log");
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(0)), text, &log);
            assert!(outcome.ok, "{:?}", outcome.failing);
            judge("fast", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &tip, |_| Ok(outcome)).unwrap();
            let warns: Vec<(Sev, String, String)> = fed.lock().unwrap().drain(..).filter(|r| r.kind == "bucket" && r.sev == Sev::Warn)
                .map(|r| (r.sev, r.line, r.detail)).collect();
            warns
        };
        let first = judged(&format!("{log}NODEGATE-OK\n"));
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].1.contains("WARN new warnings in tests f: tests f: origin unknown"), "{}", first[0].1);
        assert!(first[0].2.contains("p.rs:5:11: warning: constant `MACHINE` is never used"), "{}", first[0].2);
        assert!(judged(&format!("{log}NODEGATE-OK\n")).is_empty(), "the same warning at its next look is no news");
        let again = judged(&format!("{}\n{}\nGATEUNIT warn tests f\nGATEUNIT ok tests f\nNODEGATE-OK\n", warn("5:11", "constant `MACHINE` is never used"), warn("9:1", "unused import: `x`")));
        assert_eq!(again.len(), 1, "{again:?}");
        assert!(again[0].2.contains("p.rs:9:1: warning: unused import") && !again[0].2.contains("MACHINE"), "only the new one: {}", again[0].2);
        let rows: Vec<Value> = rows_of(&home, "fast");
        assert!(rows.iter().all(|r| r.get("ok").and_then(Value::as_bool) == Some(true)), "a warning never turns it red");
        assert_eq!(rows[0].get("warns").and_then(|w| w.get("tests f")).and_then(Value::as_str), Some(ORIGIN_UNKNOWN), "no landing in its range to name");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_failed_unit_stays_red_until_it_passes() {
        let row = |failing: &[&str], passed: &[&str], broke: &[(&str, &str)]| obj(vec![
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())),
            ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())),
            ("broke", Value::Obj(broke.iter().map(|(k, who)| (k.to_string(), s(*who))).collect())),
        ]);
        let failed = row(&["check x --also=y", "present p", "devhub makepad-devhub", "tests t"], &["check m"], &[("check x", "rc-a (A) #970")]);
        // a later run that only looked at Calculator: its diff touched nothing, and it is no pass
        let quiet = row(&[], &["present makepad-app-calculator"], &[]);
        let still = still_failing(&[quiet.clone(), failed.clone()]);
        assert_eq!(still.keys().map(String::as_str).collect::<Vec<_>>(), ["check x", "devhub makepad-devhub", "present p", "tests t"]);
        assert_eq!((still["check x"].0.as_str(), still["check x"].1.as_str()), ("check x --also=y", "rc-a (A) #970"), "the unit as it failed, with its flips");
        assert_eq!(still["present p"].1, ORIGIN_UNKNOWN);
        assert_eq!(retry_env(&[quiet.clone(), failed.clone()]), "GATE_RETRY=check x --also=y;devhub makepad-devhub;present p;tests t");
        // a pass settles it, also under other flips; an older failure stays settled
        let fixed = row(&[], &["check x", "present p", "tests t"], &[]);
        assert_eq!(still_failing(&[fixed.clone(), quiet.clone(), failed.clone()]).keys().map(String::as_str).collect::<Vec<_>>(), ["devhub makepad-devhub"]);
        // failing again after a pass: open again
        assert!(still_failing(&[row(&["present p"], &[], &[]), fixed, failed.clone()]).contains_key("present p"));
        // a package that left the tree has nothing left to fail
        let (_, passed) = unit_lines("GATEUNIT gone check x --also=y\n");
        assert!(passed.contains("check x"));
        // a step's failure names no unit: the gate takes the JIT once more, and any verdict settles it
        assert_eq!(retry_env(&[row(&["build"], &[], &[])]), "GATE_RETRY=steps");
        assert_eq!(retry_env(&[quiet, row(&["build"], &[], &[])]), "GATE_RETRY=");
        // a present failing again is the same break: who broke it, not the landings since
        let now = ["present p".to_string()];
        assert_eq!(carried_since(&[failed], &now, &BTreeMap::new()).get("present p").map(String::as_str), Some(ORIGIN_UNKNOWN));
    }

    /// The first recheck of a failure from before rows kept errors named whatever had just landed,
    /// and each later run carried that name. Its first run's word stands, and it had none.
    #[test]
    fn a_failure_older_than_the_ledgers_names_is_of_unknown_origin() {
        let old = obj(vec![("failing", Value::Arr(vec![s("check z")])), ("log", s("/h/bucket-logs/fast-1.log"))]);
        let quiet = obj(vec![("failing", Value::Arr(vec![])), ("errors", obj(vec![])), ("passed", Value::Arr(vec![s("check m")]))]);
        let unknown = "origin unknown, first failed in fast-1.log";
        let now = BTreeMap::from([("check z".to_string(), vec!["a.rs: warning: unused".to_string()])]);
        let failing = ["check z".to_string()];
        assert_eq!(carried_since(&[quiet.clone(), old.clone()], &failing, &now).get("check z").map(String::as_str), Some(unknown), "no errors to compare with: the same break");
        // a wrong name a later run wrote does not stand either
        let named = obj(vec![("failing", Value::Arr(vec![s("check z")])), ("errors", Value::Obj([("check z".to_string(), Value::Arr(vec![s("a.rs: warning: unused")]))].into_iter().collect())), ("broke", obj(vec![("check z", s("rc-late (L) #9"))]))]);
        assert_eq!(carried_since(&[named.clone(), quiet.clone(), old.clone()], &failing, &now).get("check z").map(String::as_str), Some(unknown));
        assert_eq!(still_failing(&[named.clone(), quiet, old])["check z"].1, unknown);
        // where the first failure has a name, that is who
        assert_eq!(still_failing(&[named])["check z"].1, "rc-late (L) #9");
        assert_eq!(still_since(unknown), unknown);
        assert_eq!(still_since("rc-a (A) #9"), "since rc-a (A) #9");
    }

    /// A unit still failing names the log of its last look, in the bucket's FAILED line and in
    /// `devhub bucket`'s STILL FAILING: a kept log holding only a green present of the unit, beside
    /// a red bucket, read as a miscount (11-05).
    #[test]
    fn a_still_failing_unit_names_the_log_of_its_last_look() {
        use crate::proto::Record;
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fed: Arc<Mutex<Vec<Record>>> = Arc::default();
        let feed = fed.clone();
        let store = Store::open(&home, "mac", Arc::new(move |r: Record| { feed.lock().unwrap().push(r); 0 })).unwrap();
        let tip = Pair::new("a1", "b1");
        let judged = |n: u32, text: &str| {
            let log = home.join(format!("fast-{n}.log"));
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(if text.contains("NODEGATE-OK") { 0 } else { 1 })), text, &log);
            judge("fast", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &tip, |_| Ok(outcome)).unwrap();
            let red: Vec<String> = fed.lock().unwrap().drain(..).filter(|r| r.kind == "bucket" && r.sev == Sev::Err).map(|r| r.line).collect();
            (log.display().to_string(), red)
        };
        let failed = "/n/tree/a/src/s.rs:134:12: warning: method `s` is never used [dead_code]\nf: 1 warnings\nGATEUNIT fail check f\nNODEGATE-FAIL: f-warnings\n";
        let (first, _) = judged(1, failed);
        // a run that did not look at it: red, with the log of the run that did
        let (_, quiet) = judged(2, "GATEUNIT ok check c\nNODEGATE-OK\n");
        assert_eq!(quiet.len(), 1, "{quiet:?}");
        assert!(quiet[0].contains(&format!("check f still failing origin unknown, first failed in fast-1.log (log {first})")), "{}", quiet[0]);
        // failing again: this run's log
        let (third, again) = judged(3, failed);
        assert!(again[0].contains(&format!("check f still failing origin unknown, first failed in fast-1.log (log {third})")), "{}", again[0]);
        let status = status(&home);
        let still = status.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).find(|v| v.get("still_failing").is_some()).expect("a STILL FAILING line");
        assert_eq!(still.get("logs").and_then(|l| l.get("check f")).and_then(Value::as_str), Some(third.as_str()));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_bucket_starts_from_its_last_start_and_landings_and_the_cap_is_a_cap() {
        let (min, h) = (60_000u64, 3_600_000u64);
        let ci = Timing::after(Duration::from_secs(15 * 60), Duration::from_secs(3 * 3600));
        // last run at 0, a landing every 10 min from 10 min on: never 15 min quiet, so it starts
        // 3 h after that first landing (a master restart changes none of the inputs)
        let first_start = (10 * min..5 * h).step_by(min as usize).find(|now| *now >= starts_at(&ci, 0, 10 * min, now / (10 * min) * (10 * min)));
        assert_eq!(first_start, Some(10 * min + 3 * h));
        // quiet for 15 min: then
        assert_eq!(starts_at(&ci, 0, 10 * min, 40 * min), 55 * min);
        // a run with no ledger row and no run of its kind known: due since its first landing, long
        // past, so now
        assert!(starts_at(&ci, 0, 1, 1_000 * min) <= 3 * h + 1);
        let hot = Timing::after(Duration::ZERO, Duration::ZERO);
        assert_eq!(starts_at(&hot, 5, 9, 9), 9, "hot: at the landing");
        let fast = Timing::after(Duration::from_secs(60), Duration::from_secs(600));
        assert_eq!(starts_at(&fast, 0, 100, 100), 100 + min, "a minute after the last landing");
        assert_eq!(starts_at(&fast, 0, 100, 100 + 20 * min), 100 + 10 * min, "at most 10 min after the first");
        let every = Timing { every: Duration::from_secs(3600), quiet: Duration::ZERO, cap: Duration::ZERO, daily: None };
        assert_eq!(starts_at(&every, 1_000, 2_000, 2_000), 1_000 + h, "not before every after its last start");
        assert_eq!(log_started(Path::new("/x/bucket-logs/ci-1793673952236.log")), Some(1793673952236));
    }

    /// The night's run: at 02:00 local after the first landing it has not checked, once a day
    /// however many landings follow, whatever the zone and its daylight saving.
    #[test]
    fn the_nightly_run_starts_at_two_after_a_landing_once_a_day() {
        let (min, h, day) = (60_000u64, 3_600_000u64, 86_400_000u64);
        let cet = |_: i64| 3600;
        // 2026-11-05 00:00 UTC is 01:00 CET
        let midnight = 1_793_836_800_000u64;
        assert_eq!(daily_after(midnight, 2, &cet), midnight + h, "02:00 CET the same night");
        assert_eq!(daily_after(midnight + h, 2, &cet), midnight + h + day, "at 02:00 itself: the next night");
        assert_eq!(daily_after(midnight + 9 * h, 2, &cet), midnight + h + day, "a landing at 10:00: the next night");
        assert_eq!(daily_after(midnight + 23 * h, 2, &|_| 0), midnight + day + 2 * h, "in UTC");
        // a zone that leaves summer time before the night's 02:00: UTC+2 at the landing, UTC+1 then
        let dst = |secs: i64| if secs < (midnight / 1000) as i64 - 3600 { 7200 } else { 3600 };
        assert_eq!(daily_after(midnight - 6 * h, 2, &dst), midnight + h, "02:00 on the clock after the change");
        let nightly = Timing { every: Duration::ZERO, quiet: Duration::ZERO, cap: Duration::ZERO, daily: Some(2) };
        let at = starts_at(&nightly, midnight - day, midnight + 10 * min, midnight + 50 * min);
        assert_eq!(at % (h / 60), 0, "on the minute");
        assert!(at > midnight + 10 * min && at <= midnight + 10 * min + day, "within a day of the landing");
        assert_eq!(starts_at(&nightly, at, at + min, at + min), daily_after(at + min, 2, &|s| makepad_civil_time::local_or_utc(s).offset), "the next night, not again at once");
        assert_eq!(log_started(Path::new("/x/ci.log")), None);
    }
}
