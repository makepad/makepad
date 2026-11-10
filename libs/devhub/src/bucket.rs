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
//! The full bucket ([`full`]) runs every package's tests with no proof reused, once a day before
//! the daily push review and on demand (`devhub bucket run full`): the tests bucket runs what the
//! landings reach and takes passes as proofs, and a red no landing's reach caught stayed unseen
//! (makepad-script's, 11-07). It reports; it gates nothing.
//! `devhub bucket run <bucket>` starts a bucket's run at once, of the tip then ([`kick`]).

use crate::land::{Entry, St};
use crate::lease::Pin;
use crate::proto::{hub_record, wall_ms, Sev};
use crate::state::{Pair, Store};
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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

    /// The fast bucket's: pipelined, a run of the newest tip at the first landing it has not
    /// checked, and the next one as a run ends. It waited for the landings to pause a minute (at
    /// most 10): at 11 landings in 90 min a quiet minute was rare, and a Mac show held for it
    /// (11-07).
    pub fn pipelined() -> Timing {
        Timing::after(Duration::ZERO, Duration::ZERO)
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
pub(crate) fn save_run(log: &Path, pid: u32, started: u64, base: &Pin, tip: &Pin) -> std::io::Result<()> {
    let sidecar = log.with_extension("log.run");
    let value = obj(vec![
        ("pid", Value::Int(pid as i64)), ("started_ms", Value::Int(started as i64)),
        ("base", tip_value(base)), ("tip", tip_value(tip)),
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

/// A bucket's run of `tip` (every followed repository's commit, [`crate::orch::snapshot_pin`])
/// since `base`, into its log.
pub type Run = Arc<dyn Fn(Option<&Pin>, &Pin, &Path) -> Outcome + Send + Sync>;

const GATE_NEEDS: &str = "arch=x64,os=linux";
static LEDGER: Mutex<()> = Mutex::new(());
/// One tests or full run's shards at a time: a full run's three shards beside a tests run's took
/// every pool box, and landing gates waited behind the six (11-07). A run that waits starts on
/// what the other proved.
static SHARD_RUNS: Mutex<()> = Mutex::new(());

/// A bucket row's tip: work, main, and each other nested repository's commit (`nested`, by name).
fn tip_value(tip: &Pin) -> Value {
    let pair = tip.pair();
    let others: Vec<(String, Value)> = tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED).map(|(n, sha)| (n.clone(), s(sha))).collect();
    let mut fields = vec![("work", s(&pair.work)), ("main", s(&pair.main))];
    if !others.is_empty() { fields.push(("nested", Value::Obj(others.into_iter().collect()))); }
    obj(fields)
}

/// [`tip_value`] read back; a row with no `nested` looked at no other nested repository.
fn read_tip(v: &Value) -> Option<Pin> {
    let pair = Pair::new(v.get("work")?.as_str()?, v.get("main")?.as_str()?);
    if !pair.valid() { return None; }
    let mut pin = pin_of(&pair);
    if let Some(Value::Obj(nested)) = v.get("nested") {
        for (name, sha) in nested.iter() {
            let sha = sha.as_str().filter(|s| crate::state::hex40(s))?;
            if !crate::nested::valid(name) { return None; }
            pin.nested.insert(name.clone(), sha.to_string());
        }
    }
    Some(pin)
}

/// The pin of a pair: makepad and the paired repository.
pub(crate) fn pin_of(pair: &Pair) -> Pin {
    Pin::at(&pair.work).with(crate::state::PAIRED, &pair.main)
}

/// The lease `--rev` of a bucket's tip: work, commercial main, and each other nested repository
/// at the branch its checkout in `repo` is on ([`crate::nested::branches`]).
fn tip_rev(repo: &Path, tip: &Pin) -> Result<String, String> {
    let branches = crate::nested::branches(repo);
    let mut rev = tip.pair().rev();
    for (name, sha) in tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED) {
        let branch = branches.iter().find(|(n, _)| n == name).map(|(_, b)| b).ok_or_else(|| format!("{name} is on no branch in {}: the run cannot pin it", repo.display()))?;
        rev.push_str(&format!(",{name}={branch}@{sha}"));
    }
    Ok(rev)
}

/// The gate's bases of the other nested repositories (`GATE_NESTED_BASES=<name>=<sha|->,..`):
/// each one `tip` names at its `base` commit, `-` where the base had none of it (every file of it
/// is new to the bucket).
fn nested_bases(base: &Pin, tip: &Pin) -> String {
    let sides: Vec<String> = tip.nested.keys().filter(|n| *n != crate::state::PAIRED).map(|n| format!("{n}={}", base.nested.get(n).map_or("-", String::as_str))).collect();
    format!("GATE_NESTED_BASES={}", sides.join(","))
}

/// `<work 10>,<main 10>[,<name>=<sha 10>]..`: a bucket tip as its lines say it.
fn tip_short(tip: &Pin) -> String {
    tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED).fold(tip.pair().short(), |k, (n, sha)| format!("{k},{n}={}", crate::state::short(sha)))
}

/// An outcome that says nothing of the tip.
fn no_verdict(why: String) -> Outcome {
    Outcome { ok: false, failing: Vec::new(), log: String::new(), summary: String::new(), no_verdict: Some(why) }
}

fn read_run(path: &Path) -> Option<(u32, u64, Pin, Pin)> {
    let v = makepad_strict_json::parse(&std::fs::read(path).ok()?).ok()?;
    let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
    if pid == 0 { return None; }
    Some((pid, v.get("started_ms")?.as_u64()?, read_tip(v.get("base")?)?, read_tip(v.get("tip")?)?))
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
        // `tests <pkg>`: its test targets' one cargo check (land_gate.sh step_testcheck); `test
        // <pkg>`: its tests run under the JIT (the tests bucket, land_gate.sh testrun); `guard
        // <pkg>`: its GPU guard of a user-visible effect (the hot bucket, step_guards); `wasm <pkg>`:
        // its wasm32 check (the fast bucket, step_wasmcheck)
        (Some("check" | "present" | "cargo" | "tests" | "test" | "guard" | "wasm"), Some(_)) => true,
        (Some("suite"), Some(_)) => words.next().is_none(),
        (Some("devhub"), Some("makepad-devhub")) => words.next().is_none(),
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn range(repo: &Path, base: &str, tip: &str) -> Result<std::sync::Arc<std::collections::HashSet<String>>, String> {
    crate::gitread::Pass::default().range(repo, base, tip)
}

/// The landings between two tips: a commercial-only landing moves main, not work. One that
/// moved nothing (its branch was in work already: just before is what it landed) is none. A side
/// a landing did not move is no sign of it: the commercial-only landings after a work landing
/// left work at its commit, and the bucket that ran at that work landing named them too (#2543
/// and #2544 in the range of #2542, 11-07).
fn landings(repo: &Path, base: &Pair, tip: &Pair, entries: &[Entry]) -> Result<Vec<Entry>, String> {
    let mut pass = crate::gitread::Pass::default();
    let work = pass.range(repo, &base.work, &tip.work)?;
    let main = pass.range(&repo.join("apps/commercial"), &base.main, &tip.main)?;
    Ok(entries.iter().filter(|e| e.pre != e.landed).filter(|e| {
        let Some(landed) = entry_pair(&e.landed) else { return false };
        let pre = entry_pair(&e.pre);
        let moved = |side: fn(&Pair) -> &String| pre.as_ref().is_none_or(|p| side(p) != side(&landed));
        (moved(|p| &p.work) && work.contains(&landed.work)) || (moved(|p| &p.main) && main.contains(&landed.main))
    }).cloned().collect())
}

fn attribute(repo: &Path, entries: &[Entry], failing: &[String]) -> (Vec<Vec<String>>, bool) {
    let changed = changed_by(repo, entries, failing);
    let mut attributed = vec![Vec::new(); entries.len()];
    let mut ambiguous = false;
    for unit in failing {
        let by = changed.get(unit).map_or(&[][..], Vec::as_slice);
        if by.is_empty() {
            ambiguous = true;
            for units in &mut attributed { units.push(unit.clone()); }
        } else {
            for &i in by { attributed[i].push(unit.clone()); }
        }
    }
    (attributed, ambiguous)
}

/// The landings among `entries` whose change moved each unit's inputs (its gateproof keys), by
/// index: none for a unit no landing changed, or for what is no unit ([`named_changers`]).
fn changed_by(repo: &Path, entries: &[Entry], failing: &[String]) -> BTreeMap<String, Vec<usize>> {
    let units: Vec<String> = failing.iter().filter(|u| is_unit(u)).cloned().collect();
    let mut keys = HashMap::new();
    for e in entries {
        for pair in [entry_pair(&e.pre), entry_pair(&e.landed)].into_iter().flatten() {
            keys.entry(pair.clone()).or_insert_with(|| crate::gateproof::blame_keys(repo, &pair, &units));
        }
    }
    failing.iter().map(|unit| (unit.clone(), if is_unit(unit) {
        named_changers(&entries.iter().enumerate().filter_map(|(i, e)| {
            let pre = entry_pair(&e.pre)?;
            let landed = entry_pair(&e.landed)?;
            let before = keys.get(&pre)?.get(unit)?;
            let after = keys.get(&landed)?.get(unit)?;
            (before != after).then(|| (i, before.as_ref().map(|(_, own)| own) != after.as_ref().map(|(_, own)| own)))
        }).collect::<Vec<_>>())
    } else { Vec::new() })).collect()
}

/// Of the landings that moved a unit's key (index, and whether it moved the unit's own inputs and
/// not only the gate's scripts), the ones a row names. One that changed only the gate's scripts
/// moved every unit's key, and was named for each red of its range beside the landing whose
/// change the red's own crate shows (#2542, a gate script, for a Printability call the toolpath
/// change broke, 11-07): it is named only when no landing changed the unit's own inputs.
fn named_changers(moved: &[(usize, bool)]) -> Vec<usize> {
    let own: Vec<usize> = moved.iter().filter(|(_, own)| *own).map(|(i, _)| *i).collect();
    if own.is_empty() { moved.iter().map(|(i, _)| *i).collect() } else { own }
}

/// A landing as a bucket names it: `rc-x+rc-x (AGENT) #12`.
fn who_of(e: &Entry) -> String {
    format!("{} ({}) #{}", e.label(), e.agent, e.id)
}

/// A unit's package: `check <pkg>` without its `--also=` flips (one break shows under both).
pub fn unit_key(unit: &str) -> String {
    unit.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// A compile error line as `<file>: <message>`: rustc's `<path>:<line>:<col>: ` gives way to the
/// file's name (the node's tree path, or an edit shifting the line, is no new error). The message
/// ends at a tab: a present's summary line carries the error as a tab-separated record whose next
/// field changes from run to run, and each run after a break named the landing of its own bucket
/// as the breaker (a missing field broke Fab's build at #1926, then #1927-#1931 were named, 11-06).
fn error_key(line: &str) -> Option<String> {
    let (path, said) = location(line)?;
    Some(format!("{}: {}", path.rsplit('/').next().unwrap_or(path), message(said)))
}

/// rustc's `<path>:<line>:<col>: <message>`, as its path and message.
fn location(line: &str) -> Option<(&str, &str)> {
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
                return Some((&line[..at], &col[n + 2..]));
            }
        }
        from = at + 1;
    }
    None
}

/// An error's message as [`error_key`] keeps it: up to a tab. Rows from before kept what followed.
fn message(text: &str) -> &str {
    text.split('\t').next().unwrap_or(text).trim_end()
}

/// A run's failing units with their compile errors (each unit's own output: the lines after the
/// unit before it), or the tests a hung unit still ran (land_gate.sh's `HUNG: ` line), and the
/// units that passed, by package (a proven one passed at these inputs).
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
        } else if line.starts_with("HUNG: ") {
            own.push(crate::proto::clean(line, 400));
        } else if let Some(e) = error_key(line) {
            own.push(e);
        }
    }
    (errors, passed)
}

/// A run's failing tests by unit, as `makepad test` names each (`FAILED <test>`: the unit's own
/// lines, those after the unit before it), and the units it ran that passed, by name (a proven one
/// passed at these inputs; a gone one has nothing left to fail).
fn test_lines(text: &str) -> (BTreeMap<String, Vec<String>>, BTreeSet<String>) {
    let (mut failed, mut ok, mut own) = (BTreeMap::new(), BTreeSet::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            match rest.split_once(' ') {
                Some(("fail", unit)) if !own.is_empty() => {
                    let mut tests = std::mem::take(&mut own);
                    tests.sort();
                    tests.dedup();
                    failed.insert(unit.to_string(), tests);
                }
                Some(("ok" | "proven" | "gone", unit)) => { ok.insert(unit.to_string()); }
                _ => {}
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else if let Some(test) = line.strip_prefix("FAILED ").filter(|t| !t.is_empty() && !t.contains(char::is_whitespace)) {
            own.push(test.to_string());
        }
    }
    (failed, ok)
}

/// What a unit's failing tests' blame starts with ([`tests_blame`]).
pub const BY_TEST: &str = "by test: ";

/// The tests a blame names at most per breaker (`a, b, c and 2 more since X`).
const TESTS_NAMED: usize = 3;

/// What a bucket row saw of a test.
#[derive(Debug, PartialEq)]
enum Look {
    /// it failed there, in that unit
    Failed(String),
    /// it passed: its unit or its whole package passed, or its unit failed naming other tests
    Passed,
    /// no unit that holds it ran, or one failed naming none (a build that stopped before its tests,
    /// or a whole package's run that failed elsewhere, which may have stopped before its target)
    Not,
}

/// A row as [`test_blame`] reads it: one from before rows named their tests (11-07) with what its
/// kept log says ([`test_lines`]), and who changed each failing unit by its landings' units (none
/// for a unit its ambiguous run spread over every landing). As it is with its log gone.
fn named_row(row: &Value) -> Value {
    if row.get("ran_ok").is_some() { return row.clone(); }
    let Some(text) = row.get("log").and_then(Value::as_str).and_then(|l| std::fs::read_to_string(l).ok()) else { return row.clone() };
    let (failed, ok) = test_lines(last_run(&text));
    let landings = row.get("landings").and_then(Value::as_arr).unwrap_or_default();
    let spread = row.get("ambiguous").and_then(Value::as_bool) == Some(true);
    let said = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let changed: Vec<(String, Value)> = failed.keys().filter_map(|unit| {
        let by: Vec<String> = landings.iter().filter(|e| e.get("units").and_then(Value::as_arr).is_some_and(|us| us.iter().any(|u| u.as_str() == Some(unit.as_str()))))
            .map(|e| format!("{} ({}) #{}", said(e, "label"), said(e, "agent"), e.get("id").and_then(Value::as_u64).unwrap_or(0))).collect();
        (!by.is_empty() && !(spread && by.len() == landings.len())).then(|| (unit.clone(), s(&by.join(", "))))
    }).collect();
    let mut row = row.clone();
    if let Value::Obj(fields) = &mut row {
        fields.push(("tests_failed".into(), Value::Obj(failed.iter().map(|(u, t)| (u.clone(), Value::Arr(t.iter().map(s).collect()))).collect())));
        fields.push(("ran_ok".into(), Value::Arr(ok.iter().map(s).collect())));
        fields.push(("changed".into(), Value::Obj(changed)));
    }
    row
}

/// What `row` saw of `test` of `unit`. None: a row from before rows named their tests (11-07)
/// whose log is gone ([`named_row`]).
fn looked(row: &Value, unit: &str, test: &str) -> Option<Look> {
    let ok = row.get("ran_ok").and_then(Value::as_arr)?;
    let Some(Value::Obj(failed)) = row.get("tests_failed") else { return None };
    let mut seen = false;
    for (ran, tests) in failed.iter().filter(|(ran, _)| ran == unit || *ran == unit_key(unit)) {
        let tests = tests.as_arr().unwrap_or_default();
        if tests.iter().any(|t| t.as_str() == Some(test)) { return Some(Look::Failed(ran.clone())); }
        seen |= ran == unit && !tests.is_empty();
    }
    seen |= ok.iter().filter_map(Value::as_str).any(|ran| ran == unit || ran == unit_key(unit));
    Some(if seen { Look::Passed } else { Look::Not })
}

/// A row's log, by its file name.
fn log_name(row: &Value) -> Option<String> {
    row.get("log").and_then(Value::as_str).and_then(|l| Path::new(l).file_name()).map(|l| l.to_string_lossy().into_owned())
}

/// Who broke `test` of `unit`: the landings that changed its unit in the first run where it
/// failed after a run where it passed. With no such pair in the ledger nobody knows, and no
/// landing is named: why (no green baseline: it failed in every run back to the first log, and
/// the ledger ends there or the run before it has no log to read). All nine red tests of the
/// first run that named its tests said only `origin unknown` (11-07). A unit's breaker is no test's: the
/// landings that changed a package were named for every test failing in it, and a pass of any of
/// its targets ended the look back, so whatever landed next was named (one package's tests for a
/// devhub-only landing, another's for two that changed only its tests' support, 11-07).
/// `rows`: newest first, from a run where it failed.
fn test_blame(rows: &[Value], unit: &str, test: &str) -> String {
    let mut first: Option<(&Value, String)> = None;
    for row in rows {
        match looked(row, unit, test) {
            Some(Look::Failed(ran)) => first = Some((row, ran)),
            Some(Look::Not) => {}
            Some(Look::Passed) => {
                let Some((row, ran)) = first else { return ORIGIN_UNKNOWN.to_string() };
                return match row.get("changed").and_then(|c| c.get(&ran)).and_then(Value::as_str).filter(|w| !w.is_empty()) {
                    Some(who) => who.to_string(),
                    None => format!("{ORIGIN_UNKNOWN}: no landing of its first failing run ({}) changed {ran}", log_name(row).unwrap_or_default()),
                };
            }
            None => {
                let before = log_name(row).map_or("its ledger row names none".to_string(), |l| format!("{l} is gone"));
                return match first.and_then(|(row, _)| log_name(row)) {
                    Some(log) => format!("{ORIGIN_UNKNOWN}: no green baseline, it failed in every run back to {log}, and the log of the run before ({before}) is not there to read its tests from"),
                    None => ORIGIN_UNKNOWN.to_string(),
                };
            }
        }
    }
    match first.and_then(|(row, _)| log_name(row)) {
        Some(log) => format!("{ORIGIN_UNKNOWN}: no green baseline, it failed in every run the ledger keeps, the first {log}"),
        None => ORIGIN_UNKNOWN.to_string(),
    }
}

/// Who broke each failing test of one package's units ([`test_blame`]), those with one breaker
/// together: `by test: a, b since X; c origin unknown, first failed in L`.
fn tests_blame(rows: &[Value], named: &[(String, Vec<String>)]) -> String {
    let mut by: Vec<(String, Vec<&str>)> = Vec::new();
    for (unit, tests) in named {
        for test in tests {
            let who = test_blame(rows, unit, test);
            match by.iter_mut().find(|(w, _)| *w == who) {
                Some((_, them)) => them.push(test.as_str()),
                None => by.push((who, vec![test.as_str()])),
            }
        }
    }
    let said: Vec<String> = by.iter().map(|(who, them)| {
        let more = them.len().saturating_sub(TESTS_NAMED);
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        format!("{}{more} {}", them[..them.len().min(TESTS_NAMED)].join(", "), still_since(who))
    }).collect();
    format!("{BY_TEST}{}", said.join("; "))
}

/// A row's failing units of package `key` with the tests each named.
fn named_tests(row: &Value, key: &str) -> Vec<(String, Vec<String>)> {
    let Some(Value::Obj(failed)) = row.get("tests_failed") else { return Vec::new() };
    failed.iter().filter(|(unit, _)| unit_key(unit) == key)
        .map(|(unit, tests)| (unit.clone(), tests.as_arr().unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()))
        .filter(|(_, tests)| !tests.is_empty()).collect()
}

/// The new warnings a bucket row keeps per unit (`warn_lines`), as rustc said them.
const WARN_LINES_KEPT: usize = 5;

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

/// The command each unit of a run said shows it alone (`GATEREPRO <unit> :: <command>`: a unit
/// that warned or failed), by package, for the lane that owns it to run: a unit's name is no
/// command (the members package's `tests` unit took the warnings of Fab's tests, and its own
/// `cargo check --tests` compiled none of them, 11-06).
fn repro_lines(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|l| l.strip_prefix("GATEREPRO ")?.split_once(" :: ")).map(|(unit, cmd)| (unit_key(unit), cmd.trim().to_string())).collect()
}

/// A unit's command, as `bucket status` and the bucket's WARN and FAILED lines say it.
pub fn repro_said(unit: &str, cmd: &str) -> String {
    format!("{unit}: shown alone by `{cmd}`")
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
            let before: Vec<&str> = failed.and_then(|(_, b)| b.as_arr()).unwrap_or_default().iter().filter_map(Value::as_str).map(message).collect();
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
    // a unit whose last failing run named its tests: who broke each of them
    if let Some(i) = rows.iter().position(|row| failed(row)) {
        let named = named_tests(&rows[i], key);
        // as its run said it, from the rows and logs it read ([`named_row`])
        if let Some(who) = rows[i].get("broke").and_then(|b| b.get(key)).and_then(Value::as_str).filter(|w| w.starts_with(BY_TEST)).filter(|_| !named.is_empty()) {
            return who.to_string();
        }
        if !named.is_empty() { return tests_blame(&rows[i..], &named); }
    }
    let first = rows.iter().take_while(|row| !lists("passed", row)).filter(|row| failed(row)).last();
    if let Some(who) = first.and_then(|row| row.get("broke")).and_then(|b| b.get(key)).and_then(Value::as_str) {
        return who.to_string();
    }
    let log = first.and_then(|row| row.get("log")).and_then(Value::as_str).and_then(|l| Path::new(l).file_name()).map(|l| l.to_string_lossy().into_owned());
    log.map_or(ORIGIN_UNKNOWN.to_string(), |log| format!("{ORIGIN_UNKNOWN}, first failed in {log}"))
}

/// A failure older than what the ledger can name a landing for.
pub const ORIGIN_UNKNOWN: &str = "origin unknown";

/// A new warning in a file no landing of the bucket's range touched.
pub const FROM_BEFORE: &str = "from before this range";

/// Who brought a unit's new warnings, by their `files` (rustc's paths: the tree's own, or under
/// the node's tree): the landings of the range whose change touched one (`touched`, each one's
/// files; None: its diff could not be read, so it is not cleared), else none of them, and the row
/// says the warnings are from before this range with the commit that `last` touched each file.
/// A landing that changed the unit's other inputs touched no warning: every landing of a range
/// was named for an unused function in a Fab test file none of them changed (#2239 and #2240,
/// then #2242-#2244, for FABPRINTERS' simulated_profiles.rs, 11-06).
fn warn_blame(files: &[&str], entries: &[Entry], touched: &[Option<Vec<String>>], last: &dyn Fn(&str) -> Option<String>) -> String {
    let who: Vec<String> = entries.iter().zip(touched).filter(|(_, t)| t.as_ref().is_none_or(|t| t.iter().any(|p| files.iter().any(|f| touches(f, p)))))
        .map(|(e, _)| format!("{} ({}) #{}", e.label(), e.agent, e.id)).collect();
    if !who.is_empty() { return who.join(", "); }
    let mut commits: Vec<String> = Vec::new();
    for c in files.iter().filter_map(|f| last(f)) {
        if !commits.contains(&c) { commits.push(c); }
    }
    if commits.is_empty() { FROM_BEFORE.to_string() } else { format!("{FROM_BEFORE} (last touched by {})", commits.join(", ")) }
}

/// The files landing `e` changed, the nested commercial checkout's under `apps/commercial/`.
/// None: a side's commits are not in the repository.
fn landing_files(repo: &Path, e: &Entry) -> Option<Vec<String>> {
    let (pre, landed) = (entry_pair(&e.pre)?, entry_pair(&e.landed)?);
    let mut files = crate::gitread::files_differing(repo, &pre.work, &landed.work)?;
    files.extend(crate::gitread::files_differing(&repo.join("apps/commercial"), &pre.main, &landed.main)?.into_iter().map(|f| format!("apps/commercial/{f}")));
    Some(files)
}

/// The commit that last touched the file rustc named `file` at `at`: its longest tail that is a
/// file of the tree, the nested commercial checkout's under `apps/commercial/`.
fn last_touch(repo: &Path, at: &Pair, file: &str) -> Option<String> {
    let parts: Vec<&str> = file.split('/').filter(|p| !p.is_empty()).collect();
    (0..parts.len()).find_map(|i| {
        let rel = parts[i..].join("/");
        match rel.strip_prefix("apps/commercial/") {
            Some(p) => crate::gitread::last_touch(&repo.join("apps/commercial"), &at.main, p),
            None => crate::gitread::last_touch(repo, &at.work, &rel),
        }
    })
}

/// Whether `file`, as rustc or the JIT names it (the tree's path, or one under a node's tree), is
/// `p`, a file of the tree.
fn touches(file: &str, p: &str) -> bool {
    file == p || file.strip_suffix(p).is_some_and(|head| head.ends_with('/'))
}

/// Each failing unit's error files, as its errors name them, in the unit's own lines as
/// [`unit_lines`] reads them. A warning's file is not one. A present's summary names a file after
/// the text before it: its path is the last word.
fn error_files(text: &str) -> BTreeMap<String, Vec<String>> {
    let (mut files, mut own) = (BTreeMap::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            if let Some(("fail", unit)) = rest.split_once(' ').filter(|_| !own.is_empty()) {
                let mut f = std::mem::take(&mut own);
                f.sort();
                f.dedup();
                files.insert(unit.to_string(), f);
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else if let Some((path, _)) = location(line).filter(|(_, said)| !said.starts_with("warning")) {
            own.push(path.rsplit(char::is_whitespace).next().unwrap_or(path).to_string());
        }
    }
    files
}

/// The tip of the last of `rows` (the bucket's earlier rows, newest first) that passed unit `key`.
fn last_pass(rows: &[Value], key: &str) -> Option<Pair> {
    rows.iter().find(|row| row.get("passed").and_then(Value::as_arr).is_some_and(|p| p.iter().any(|k| k.as_str() == Some(key))))
        .and_then(|row| row.get("tip")).and_then(read_tip).map(|t| t.pair())
}

/// Who broke a failing unit whose errors sit in `files`: of `since`, the landings since the unit
/// last passed, each one whose change touched one of those files (`touched`, each landing's
/// files; None: its diff could not be read). Each is named with the commit of it that changed the
/// file (`commit`, from the file's history at the landing): `rc-x (X) #12 (0123456789 changed vm.rs)`.
/// None of them touched a file: no one, and the landings that changed the unit's inputs answer for
/// it (a change elsewhere, a callee's, broke a caller's file). A run's own landings were named
/// before. A JIT break in vm.rs from #2593's 572669339f was named for whoever landed next with a
/// change to the apps' inputs: two other lanes (11-07).
fn file_blame(files: &[String], since: &[Entry], touched: &[Option<&[String]>], commit: &dyn Fn(&Entry, &str) -> Option<String>) -> Vec<(u64, String)> {
    since.iter().zip(touched).filter_map(|(e, t)| {
        let hit = t.as_ref()?.iter().find(|p| files.iter().any(|f| touches(f, p)))?;
        let name = hit.rsplit('/').next().unwrap_or(hit);
        Some((e.id, match commit(e, hit).as_deref().and_then(|c| c.split_whitespace().next()) {
            Some(c) => format!("{} ({c} changed {name})", who_of(e)),
            None => format!("{} (it changed {name})", who_of(e)),
        }))
    }).collect()
}

/// `<unit> still failing since <who>`, for a `who` from [`origin`] or a row.
pub fn still_since(who: &str) -> String {
    if who.starts_with(ORIGIN_UNKNOWN) || who.starts_with(BY_TEST) { who.to_string() } else { format!("since {who}") }
}

/// The buckets whose next run looks again at what failed (land_gate.sh's GATE_RETRY). CI needs no
/// telling: a shard with no proof at its inputs runs; prebuild lists its legs under each new JIT;
/// the full bucket runs everything.
const RERUNS: [&str; 3] = ["fast", "hot", "tests"];

/// The full bucket's name ([`full`]).
pub const FULL: &str = "full";

/// The buckets `devhub bucket run` starts ([`kick`]).
const KICKED: [&str; 5] = ["fast", "tests", "hot", "ci", FULL];

/// The file that starts a bucket's run at its next look ([`spawn`]).
fn kick_file(home: &Path, name: &str) -> PathBuf {
    home.join("bucket-logs").join(format!("{name}.now"))
}

/// `devhub bucket run <bucket>`: its run starts at once, of the tip then, whether or not a landing
/// came since its last. The service's answer: `{"said":..}` or `{"error":..}`.
pub fn kick(home: &Path, name: &str) -> String {
    let file = kick_file(home, name);
    let done = if !KICKED.contains(&name) { Err(format!("no bucket `{}`: {}", crate::proto::clean(name, 64), KICKED.join(", "))) }
        else { std::fs::create_dir_all(home.join("bucket-logs")).and_then(|_| std::fs::write(&file, "")).map_err(|e| format!("{}: {e}", file.display())) };
    match done {
        Ok(()) => obj(vec![("said", s(&format!("bucket {name}: its run starts at its next look (`devhub bucket` says its verdict)")))]).to_json(),
        Err(e) => obj(vec![("error", s(&e))]).to_json(),
    }
}

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
    for name in RERUNS.into_iter().chain([FULL]) {
        let still = still_failing(&rows_of(home, name));
        if still.is_empty() { continue; }
        // and where each failed last: a green kept log beside a red bucket read as a miscount (11-05)
        out.push(obj(vec![("still_failing", s(name)), ("units", Value::Obj(still.values().map(|(unit, who, _)| (unit.clone(), s(who))).collect())),
            ("logs", Value::Obj(still.into_values().map(|(unit, _, log)| (unit, s(&log))).collect()))]).to_json());
    }
    out.extend(ledger(home, 10));
    out
}

/// What the buckets know of the checkout's tip, for the shows that take only proven tips
/// (orch/show.rs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TipState {
    /// the fast bucket's newest run: the tip it looked at and its log
    pub fast: Option<(Pair, String)>,
    /// a step that run stopped at (the JIT that did not build): its units did not run, so it says
    /// nothing of them
    pub stopped: Option<String>,
    /// the units red after that run, by package (`check <pkg>`), with who broke each
    pub red: BTreeMap<String, String>,
    /// the fast bucket's run in flight: when it started and the tip it looks at (the next run
    /// takes the newest tip as it ends)
    pub running: Option<(u64, Pair)>,
    /// the units the hot bucket's newest run of the tip failed
    pub hot_failing: Vec<String>,
}

/// The buckets' word on `tip` (one read of the ledger).
pub fn tip_state(home: &Path, tip: &Pair) -> TipState {
    let rows: Vec<Value> = ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let of = |name: &str| rows.iter().filter(|v| v.get("bucket").and_then(Value::as_str) == Some(name)).cloned().collect::<Vec<_>>();
    let (fast, hot) = (of("fast"), of("hot"));
    let newest = fast.first();
    let failing = |row: &Value| row.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
    let started = newest.and_then(|r| r.get("started_ms")?.as_u64()).unwrap_or(0);
    let prefix = "fast-";
    let running = std::fs::read_dir(home.join("bucket-logs")).ok().into_iter().flatten().filter_map(|entry| {
        let entry = entry.ok()?;
        let ms: u64 = entry.file_name().to_str()?.strip_prefix(prefix)?.strip_suffix(".log.run")?.parse().ok()?;
        let (_, at, _, run_tip) = read_run(&entry.path())?;
        (ms > started && at == ms).then_some((ms, run_tip.pair()))
    }).max_by_key(|(ms, _)| *ms);
    TipState {
        fast: newest.and_then(|r| Some((read_tip(r.get("tip")?)?.pair(), r.get("log").and_then(Value::as_str).unwrap_or("").to_string()))),
        stopped: newest.and_then(|r| failing(r).into_iter().find(|u| !is_unit(u))),
        red: still_failing(&fast).into_iter().map(|(key, (_, who, _))| (key, who)).collect(),
        running,
        hot_failing: hot.iter().find(|r| r.get("tip").and_then(read_tip).map(|t| t.pair()).as_ref() == Some(tip)).map(failing).unwrap_or_default(),
    }
}

/// Where a box that takes only proven tips goes ([`proven_tip`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Proven {
    /// it moves to this tip (every repository the fast bucket's run of it looked at)
    Move(Pin),
    /// it stays where it is: why
    Stay(String),
}

/// Where a box that takes only proven tips moves (the user's WM node, sync/autosync.rs), by the
/// fast bucket's runs: to the newest tip a run looked at that did not stop at a step and has no
/// check, cargo or present unit red after it that was not red after the run of the newest tip the
/// box holds (`holds(tip)`: its deployment has it; never back). A box that holds none of the tips
/// (it took every tip unproven until then) starts at the newest run that did not stop, red and all.
pub fn proven_tip(home: &Path, holds: &dyn Fn(&Pin) -> bool) -> Proven {
    let fast = rows_of(home, "fast");
    let tip = |r: &Value| r.get("tip").and_then(read_tip);
    let stopped = |r: &Value| r.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).find(|u| !is_unit(u)).map(str::to_string);
    let log = |r: &Value| r.get("log").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(base) = fast.iter().position(|r| tip(r).is_some_and(|t| holds(&t))) else {
        return match fast.iter().filter(|r| stopped(r).is_none()).find_map(tip) {
            Some(t) => Proven::Move(t),
            None => Proven::Stay("no run of the fast bucket looked at a tip yet".to_string()),
        };
    };
    let took = still_failing(&fast[base..]);
    let mut why = None;
    for (i, row) in fast[..base].iter().enumerate() {
        let Some(t) = tip(row) else { continue };
        if let Some(step) = stopped(row) {
            why.get_or_insert_with(|| format!("tip {} is not proven: the fast bucket's run of it stopped at its {step} step (log {})", tip_short(&t), log(row)));
            continue;
        }
        let red = still_failing(&fast[i..]);
        let new = red.iter().find(|(key, _)| matches!(key.split_whitespace().next(), Some("check" | "cargo" | "present")) && !took.contains_key(*key));
        match new {
            Some((_, (unit, who, log))) => {
                why.get_or_insert_with(|| format!("tip {} has {unit} failing {} (log {log})", tip_short(&t), still_since(who)));
            }
            None => return Proven::Move(t),
        }
    }
    Proven::Stay(why.unwrap_or_else(|| "waits for the fast bucket to look at a newer tip".to_string()))
}

/// No verdict finishes the attempt without a ledger row; other errors leave it adoptable.
#[derive(Debug)]
enum JudgeError {
    NoVerdict(String),
    Other(String),
}

impl From<String> for JudgeError {
    fn from(error: String) -> Self { Self::Other(error) }
}

impl std::fmt::Display for JudgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::NoVerdict(why) | Self::Other(why) => f.write_str(why) }
    }
}

/// Both a newly run and an adopted attempt retire their sidecar when no verdict is possible.
fn finish_run(log: &Path, result: Result<(), JudgeError>) -> Result<(), JudgeError> {
    if matches!(&result, Err(JudgeError::NoVerdict(_))) {
        let sidecar = log.with_extension("log.run");
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(JudgeError::Other(format!("removing bucket run {}: {e}", sidecar.display()))),
        }
    }
    result
}

/// Bucket `name`'s verdict on `tip`, from the outcome `run` reaches from the base (the last tip it
/// judged, else the oldest base of the last hour's landings): the landings since the base named
/// for the units they changed, a ledger row and a feed line (`emit`). Err: no verdict.
#[allow(clippy::too_many_arguments)]
fn judge(name: &str, home: &Path, repo: &Path, store: &Store, emit: &dyn Fn(Sev, String, String), started: u64, base: Option<Pin>, tip: &Pin,
    run: impl FnOnce(Option<&Pin>) -> Result<Outcome, String>) -> Result<(), JudgeError> {
    let history = store.history("land")?.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect::<Vec<_>>();
    let entries = || history.clone();
    let base = base.or_else(|| {
        let recent = Duration::from_secs(60 * 60).as_millis();
        let mut entries = entries();
        entries.sort_by_key(|e| (e.changed_ms, e.id));
        entries.iter().filter(|e| u128::from(started.saturating_sub(e.changed_ms)) <= recent).find_map(|e| entry_pair(&e.pre)).map(|p| pin_of(&p))
    });
    let mut outcome = run(base.as_ref())?;
    // the landings' ledger and attribution read work and main
    let base_pin = base.clone();
    let (base, tip_pin, tip) = (base.map(|b| b.pair()), tip, &tip.pair());
    if let Some(why) = &outcome.no_verdict {
        emit(Sev::Warn, format!("bucket {name} {}: no verdict: {why} ({}); it runs again at its next turn", tip_short(tip_pin), outcome.log), String::new());
        return Err(JudgeError::NoVerdict(format!("no verdict on {}: {why}", tip_short(tip_pin))));
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
    let (tests_failed, ran_ok) = test_lines(last_run(&text));
    let (warned, proven) = warn_lines(last_run(&text));
    let repro = repro_lines(last_run(&text));
    let secs = unit_secs(last_run(&text));
    let mb = unit_mb(last_run(&text));
    let earlier = rows_of(home, name);
    // a failing unit that names its failing tests is blamed by test ([`test_blame`]), the rest by unit
    let unnamed: Vec<String> = outcome.failing.iter().filter(|u| !tests_failed.contains_key(*u)).cloned().collect();
    let carried = carried_since(&earlier, &unnamed, &errors);
    // what this run did not get to (a gate that stopped at the JIT): red still, whatever else it says
    let still: BTreeMap<String, (String, String)> = if RERUNS.contains(&name) {
        still_failing(&earlier).into_iter().filter(|(key, _)| !passed.contains(key) && !outcome.failing.iter().any(|u| unit_key(u) == *key)).map(|(_, (unit, who, log))| (unit, (who, log))).collect()
    } else { BTreeMap::new() };
    outcome.ok &= still.is_empty();
    let blame: Vec<String> = unnamed.iter().filter(|u| !carried.contains_key(*u)).cloned().collect();
    // a unit whose errors sit in a file a landing changed since the unit last passed: that landing,
    // in this run's range or before it ([`file_blame`]); the rest, the landings that changed its inputs
    let located = error_files(last_run(&text));
    let (mut since, mut diffs): (HashMap<Option<Pair>, Vec<Entry>>, HashMap<u64, Option<Vec<String>>>) = (HashMap::new(), HashMap::new());
    let mut by_file: BTreeMap<String, Vec<(u64, String)>> = BTreeMap::new();
    for unit in &blame {
        let Some(files) = located.get(unit) else { continue };
        let from = last_pass(&earlier, &unit_key(unit)).or_else(|| base.clone());
        let landed = since.entry(from.clone()).or_insert_with(|| from.as_ref().and_then(|from| landings(repo, from, tip, &history).ok()).unwrap_or_default());
        for e in landed.iter() { diffs.entry(e.id).or_insert_with(|| landing_files(repo, e)); }
        let touched: Vec<Option<&[String]>> = landed.iter().map(|e| diffs[&e.id].as_deref()).collect();
        let who = file_blame(files, landed, &touched, &|e, p| entry_pair(&e.landed).and_then(|at| last_touch(repo, &at, p)));
        if !who.is_empty() { by_file.insert(unit.clone(), who); }
    }
    let blame: Vec<String> = blame.into_iter().filter(|u| !by_file.contains_key(u)).collect();
    let by_file: BTreeMap<String, (Vec<u64>, String)> = by_file.into_iter()
        .map(|(u, who)| (u, (who.iter().map(|(id, _)| *id).collect(), who.into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(", ")))).collect();
    let (units, mut ambiguous) = if blame.is_empty() { (vec![Vec::new(); entries.len()], false) }
        else { attribute(repo, &entries, &blame) };
    ambiguous |= attribution_error.is_some();
    // the landings that changed each unit with named tests, kept for the runs after to name should
    // a test first fail here ([`test_blame`]); and who broke each of those tests
    let named: Vec<String> = outcome.failing.iter().filter(|u| tests_failed.contains_key(*u)).cloned().collect();
    let changed: BTreeMap<String, String> = if named.is_empty() { BTreeMap::new() } else {
        changed_by(repo, &entries, &named).into_iter().map(|(u, by)| (u, by.iter().map(|&i| who_of(&entries[i])).collect::<Vec<_>>().join(", "))).filter(|(_, who)| !who.is_empty()).collect()
    };
    let tests_value = Value::Obj(tests_failed.iter().map(|(u, t)| (u.clone(), Value::Arr(t.iter().map(s).collect()))).collect());
    let ran_value = Value::Arr(ran_ok.iter().map(s).collect());
    let changed_value = Value::Obj(changed.iter().map(|(u, who)| (u.clone(), s(who))).collect());
    let looked_back: Vec<Value> = std::iter::once(obj(vec![("log", s(&outcome.log)), ("tests_failed", tests_value.clone()), ("ran_ok", ran_value.clone()), ("changed", changed_value.clone())]))
        .chain(earlier.iter().take(TIMES_ROWS).map(named_row)).collect();
    let mut by_key: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for unit in &named { by_key.entry(unit_key(unit)).or_default().push((unit.clone(), tests_failed[unit].clone())); }
    let by_test: BTreeMap<String, String> = by_key.iter().map(|(key, named)| (key.clone(), tests_blame(&looked_back, named))).collect();
    // who broke each failing unit: for the next run of the bucket to name, should it still fail
    let mut broke: BTreeMap<String, String> = by_test.clone();
    for unit in unnamed.iter().filter(|u| is_unit(u)) {
        let who = carried.get(unit).or_else(|| by_file.get(unit).map(|(_, who)| who)).cloned().unwrap_or_else(|| entries.iter().zip(&units).filter(|(_, us)| us.contains(unit))
            .map(|(e, _)| who_of(e)).collect::<Vec<_>>().join(", "));
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
    // who brought them: the landings that touched a new warning's file (without a range, nobody knows)
    let touched: Vec<Option<Vec<String>>> = if fresh.is_empty() || attribution_error.is_some() { Vec::new() } else { entries.iter().map(|e| landing_files(repo, e)).collect() };
    let warn_ambiguous = attribution_error.is_some() || touched.iter().any(Option::is_none);
    let mut warns: BTreeMap<String, String> = unknown.iter().map(|k| (k.clone(), ORIGIN_UNKNOWN.to_string())).collect();
    for (key, new) in &fresh {
        let Some(base) = base.as_ref().filter(|_| attribution_error.is_none()) else {
            warns.insert(key.clone(), ORIGIN_UNKNOWN.to_string());
            continue;
        };
        let mut files: Vec<&str> = warned.get(key).into_iter().flatten().filter(|(k, _)| new.contains(k)).filter_map(|(_, l)| location(l).map(|(path, _)| path)).collect();
        files.sort_unstable();
        files.dedup();
        warns.insert(key.clone(), warn_blame(&files, &entries, &touched, &|f| last_touch(repo, base, f)));
    }
    // each new warning as rustc said it (file:line), by unit; one of unknown origin, all of its unit's
    let new_lines: BTreeMap<String, Vec<String>> = warned.iter().filter(|(u, _)| warns.contains_key(*u)).map(|(u, lines)| (u.clone(), lines.iter()
        .filter(|(k, _)| fresh.get(u).is_none_or(|new| new.contains(k))).map(|(_, l)| crate::proto::clean(l, 300)).collect())).collect();
    // the command that shows each new warning's or failing unit's own lines alone
    let repro: BTreeMap<String, String> = repro.into_iter().filter(|(u, _)| warns.contains_key(u) || outcome.failing.iter().any(|f| unit_key(f) == *u)).collect();
    let rows = entries.iter().zip(&units).map(|(e, units)| obj(vec![
        ("id", Value::Int(e.id as i64)), ("label", s(&e.label())), ("agent", s(&e.agent)),
        ("land_to_verdict_ms", Value::Int(ended.saturating_sub(e.changed_ms) as i64)),
        ("units", Value::Arr(units.iter().chain(by_file.iter().filter(|(_, (ids, _))| ids.contains(&e.id)).map(|(u, _)| u)).map(|u| s(u)).collect())),
    ])).collect();
    append(home, obj(vec![
        ("bucket", s(name)), ("base", base_pin.as_ref().map_or(Value::Null, tip_value)), ("tip", tip_value(tip_pin)),
        ("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(ended as i64)),
        ("ok", Value::Bool(outcome.ok)), ("failing", Value::Arr(outcome.failing.iter().map(|u| s(u)).collect())),
        ("summary", s(&outcome.summary)), ("log", s(&outcome.log)), ("landings", Value::Arr(rows)),
        ("ambiguous", Value::Bool(ambiguous)),
        ("errors", Value::Obj(errors.iter().map(|(u, e)| (u.clone(), Value::Arr(e.iter().map(s).collect()))).collect())),
        ("passed", Value::Arr(passed.iter().map(s).collect())),
        ("carried", Value::Obj(carried.iter().map(|(u, who)| (u.clone(), s(who))).collect())),
        // each failing unit's failing tests, the units that passed and who changed each named unit:
        // what a test's blame reads back ([`test_blame`])
        ("tests_failed", tests_value),
        ("ran_ok", ran_value),
        ("changed", changed_value),
        ("still", Value::Obj(still.iter().map(|(u, (who, _))| (u.clone(), s(who))).collect())),
        ("broke", Value::Obj(broke.into_iter().map(|(k, who)| (k, s(who))).collect())),
        ("warned", Value::Obj(warned.iter().map(|(u, w)| (u.clone(), Value::Arr(w.iter().map(|(k, _)| s(k)).collect()))).collect())),
        ("proven", Value::Arr(proven.iter().map(s).collect())),
        ("warns", Value::Obj(warns.iter().map(|(u, who)| (u.clone(), s(who))).collect())),
        // the first few, for `bucket status` to show the owner without a rerun
        ("warn_lines", Value::Obj(new_lines.iter().map(|(u, l)| (u.clone(), Value::Arr(l.iter().take(WARN_LINES_KEPT).map(s).collect()))).collect())),
        ("repro", Value::Obj(repro.iter().map(|(u, cmd)| (u.clone(), s(cmd))).collect())),
        // each unit's seconds, which the next runs deal the slowest out by first (tests)
        ("unit_s", Value::Obj(secs.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect())),
        ("unit_mb", Value::Obj(mb.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect())),
    ])).map_err(|e| format!("bucket ledger: {e}"))?;
    if !warns.is_empty() {
        let named = warns.iter().map(|(u, who)| if who == ORIGIN_UNKNOWN { format!("{u}: {ORIGIN_UNKNOWN} (its first look)") }
            else if who.starts_with(FROM_BEFORE) { format!("{u}: {who}") } else { format!("{who} changed {u}") }).collect::<Vec<_>>().join("; ");
        let detail: Vec<String> = new_lines.iter().flat_map(|(u, lines)| lines.iter().cloned().chain(repro.get(u).map(|cmd| repro_said(u, cmd)))).collect();
        emit(Sev::Warn, format!("bucket {name} {} WARN new warnings in {}: {named}{}", tip_short(tip_pin), warns.keys().cloned().collect::<Vec<_>>().join(", "), if warn_ambiguous { " (ambiguous)" } else { "" }), detail.join("\n"));
    }
    let latency = entries.iter().map(|e| e.changed_ms).max().map(|landed| {
        format!("land->verdict {} s; ", ended.saturating_sub(landed) / 1000)
    }).unwrap_or_default();
    let (sev, line, detail) = if outcome.ok {
        (Sev::Info, format!("bucket {name} {} ok ({latency}{})", tip_short(tip_pin), outcome.summary), String::new())
    } else {
        let blamed = entries.iter().zip(&units).flat_map(|(e, units)| units.iter().map(move |u| {
            format!("{} ({}) changed {u}", e.label(), e.agent)
        // each with the log of its last look: this run's when it failed here, else the run that last looked
        })).chain(carried.iter().map(|(u, who)| (u, who, &outcome.log)).chain(still.iter().map(|(u, (who, log))| (u, who, log)))
            .map(|(u, who, log)| format!("{u} still failing {} (log {log})", still_since(who))))
            .chain(by_file.iter().map(|(u, (_, who))| format!("{u} broken by {who}")))
            .chain(by_test.iter().map(|(key, who)| format!("{key} failing {who} (log {})", outcome.log))).collect::<Vec<_>>().join("; ");
        let mut detail = crate::land::failure_lines(&text);
        detail.extend(repro.iter().filter(|(u, _)| !warns.contains_key(*u)).map(|(u, cmd)| repro_said(u, cmd)));
        if let Some(e) = attribution_error { detail.push(e); }
        (Sev::Err, format!("bucket {name} {} FAILED {}: {blamed}{} ({latency}{})", tip_short(tip_pin), outcome.failing.join(", "), if ambiguous { " (ambiguous)" } else { "" }, outcome.summary), detail.join("\n"))
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
    judge("prebuild", home, repo, store, &emit, started, base.as_ref().map(pin_of), &pin_of(tip), |_| Ok(outcome)).map_err(|e| e.to_string())
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
        let mut last = latest.as_ref().and_then(|v| read_tip(v.get("tip")?));
        let mut last_start = latest.as_ref().and_then(|v| v.get("started_ms")?.as_u64()).unwrap_or(0);
        let logs = home.join("bucket-logs");
        let kick = kick_file(&home, &name);
        let emit = |sev: Sev, line: String, detail: String| publish(hub_record(&machine, "", 0, "bucket", sev, line, detail));
        let record = |started: u64, base: Option<Pin>, tip: Pin, log: PathBuf| -> Result<Pin, JudgeError> {
            let result = judge(&name, &home, &repo, &store, &emit, started, base, &tip, |base| match base {
                Some(base) => {
                    std::fs::create_dir_all(&logs).map_err(|e| format!("bucket logs: {e}"))?;
                    Ok(run(Some(base), &tip, &log))
                }
                None => Ok(Outcome { ok: true, failing: Vec::new(), log: String::new(), summary: "baseline".into(), no_verdict: None }),
            });
            finish_run(&log, result)?;
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
                    format!("bucket {name} adopts its run of {} from before the master restart (pid {pid})", tip_short(&tip)), String::new()));
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
            last = Some(pin_of(&tip));
        }
        // the tip at this thread's first look is no change; one it sees change later is (a push
        // that is no landing)
        let (mut seen, mut changed, mut unlanded) = (None::<Pin>, 0, None::<u64>);
        let poll = Duration::from_secs(if timing.quiet < Duration::from_secs(5) { 1 } else { 5 });
        loop {
            std::thread::sleep(poll);
            let Some(tip) = crate::orch::snapshot_pin(&repo) else { continue };
            let now = wall_ms();
            if seen.as_ref().is_some_and(|s| *s != tip) { changed = now; }
            seen = Some(tip.clone());
            // asked for (`devhub bucket run`): now, of this tip, whatever its timing says
            if std::fs::remove_file(&kick).is_err() {
                if last.as_ref() == Some(&tip) {
                    unlanded = None;
                    continue;
                }
                let landed = landed_ms(&store);
                let first = landed.iter().copied().filter(|ms| *ms > last_start).min().unwrap_or_else(|| *unlanded.get_or_insert(now));
                let last_changed = landed.iter().copied().max().unwrap_or(0).max(changed);
                if now < starts_at(&timing, last_start, first, last_changed) { continue; }
            }
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
        let (units, why) = proofs.proven(&repo, &tip.pair());
        let retry = retry_env(&rows_of(&home, "fast"));
        let base = base.expect("fast bucket needs a base");
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        // its presents share these cores (land_gate.sh step_qpresent): what the box sets aside for them.
        // Interactive, as the hot gate: every hand-in waits on its verdict. At background priority
        // a lane's whole asks held two pool boxes against it while its 8 cores waited on a tests
        // shard on the third, every landing behind it 15 min, and it ran at a background CPU
        // weight beside live calls (L657290, 11-08).
        let lease = ["lease", "gate", "--needs", GATE_NEEDS, "--cores", "8", "--priority", "interactive", "--lane", "landq-fast", "--prio", "0", "--rev", &rev];
        let proven = format!("GATE_PROVEN={}", units.join(";"));
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let gate = ["env", &proven, &retry, &nested, "bash", "tools/run/fixture/land_gate.sh", "fast", &bases.work, &bases.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &why, log, (base, tip), &|| true);
        proofs.record_log(&repo, &tip.pair(), &text, &format!("bucket fast {}", tip_short(tip)));
        verdict(status, &text, log)
    })
}

/// Runs the tests of the changed packages and the gate packages the landings reach, the smoke set
/// and JIT checks too when they reach the JIT (affected.py --runs: the fast bucket's selection, both
/// repos; every user of a JIT or platform change was 547 units and held most of the pool's boxes for
/// 78-106 min a run, 11-08; the full bucket runs every package's daily) under the tree's JIT on the
/// Linux pool boxes, after landing and at background priority: it gates nothing. One unit `test <pkg>` each, skipped while its inputs are proven, run
/// again while red until it passes (land_gate.sh testrun). The units are one queue, slowest first
/// by their last measures ([`times_env`]), dealt a unit at a time to one shard per box
/// ([`test_boxes`]) as each has a slot free ([`Dealer`]), each shard its own lease and log (a full
/// reach, 363 packages two at a time on one box, ran 45 min; 11-07), and the run's log is theirs,
/// one after the other.
pub fn tests(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    test_run(repo, home, proofs, false)
}

/// The full bucket's run: every package's tests as the tests bucket runs them, no proof reused
/// and none of its retries needed (`GATE_FULL=1`), dealt by the units' measures of both buckets.
pub fn full(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    test_run(repo, home, proofs, true)
}

/// The tests and full buckets' rows, newest first: the units' measures of either deal the next.
fn test_rows(home: &Path) -> Vec<Value> {
    ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter(|v| v.get("bucket").and_then(Value::as_str).is_some_and(|b| b == "tests" || b == FULL)).collect()
}

fn test_run(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>, full: bool) -> Run {
    Arc::new(move |base, tip, log| {
        let _one = SHARD_RUNS.lock().unwrap_or_else(|e| e.into_inner());
        let (units, why) = if full { (Vec::new(), vec![format!("full: every package's tests at {}, no proof reused", tip_short(tip))]) } else { proofs.proven(&repo, &tip.pair()) };
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let retry = if full { "GATE_RETRY=".to_string() } else { retry_env(&rows_of(&home, "tests")) };
        let times = times_env(&test_rows(&home));
        let peaks = peaks_env(&test_rows(&home));
        let floor = ram_floor(&test_rows(&home));
        let reach = format!("GATE_FULL={}", if full { "1" } else { "" });
        let base = base.expect("tests bucket needs a base");
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        // a run a restarted master adopts is dealt as it was (its shards' leases run on, each
        // with its sidecar beside its log); a new one over the pool boxes there are now
        let adopted = log.with_extension("log.run").exists().then(|| run_shape(log)).flatten();
        let (shards, cores) = match adopted {
            Some(shape) => shape,
            None => {
                let shape = pool_boxes();
                if let Err(e) = std::fs::write(log, format!("{SHARDS}{} {}\n", shape.0, shape.1)) {
                    return verdict(Err(format!("bucket log {}: {e}", log.display())), "", log);
                }
                if let Some(started) = log_started(log) {
                    let _ = save_run(log, std::process::id(), started, base, tip);
                }
                shape
            }
        };
        let proven = format!("GATE_PROVEN={}", units.join(";"));
        let (ask, class) = (cores.to_string(), shard_class(cores, full));
        // the run's one queue, dealt as the shards' slots free ([`Dealer`])
        let dealer = Mutex::new(Dealer::new(shards));
        let ended: Vec<AtomicBool> = (0..shards).map(|_| AtomicBool::new(false)).collect();
        let ran: Vec<(PathBuf, (Result<Option<i32>, String>, String))> = std::thread::scope(|scope| {
            scope.spawn(|| deal(&dealer, &ended, log));
            let shard = |i: usize| {
                let (repo, proven, retry, times, peaks, reach, ask, class, dealer, ended, rev, bases, nested, floor) = (&repo, &proven, &retry, &times, &peaks, &reach, &ask, &class, &dealer, &ended, &rev, &bases, &nested, &floor);
                move || {
                    let _ended = Ends(&ended[i - 1]);
                    let shard_log = shard_log(log, i);
                    // at most a run per 4 of its cores (land_gate.sh TEST_CORES); a shape not
                    // measured yet is a large lease (20-40 min on its cores), not its kind's
                    // small: its three shards ran small until their class had measures (11-07)
                    // never resized below its largest unit's last peak: a unit runs alone at most
                    // (land_gate.sh ram_room) and starts within the RAM its lease got
                    let mut lease = vec!["lease", "test", "--needs", GATE_NEEDS, "--cores", ask, "--cost", "large", "--class", class, "--priority", "background", "--lane", crate::lease::sched::TESTS_LANE, "--rev", rev];
                    if let Some(gb) = floor { lease.extend(["--ram-floor-gb", gb.as_str()]); }
                    let gate = ["env", proven, retry, times, peaks, reach, nested, "GATE_QUEUE=1", "bash", "tools/run/fixture/land_gate.sh", "testrun", &bases.work, &bases.main];
                    let run = lease_run(repo, &lease, &gate, &[], &shard_log, (base, tip), &|| dealer.lock().unwrap_or_else(|e| e.into_inner()).needed());
                    (shard_log, run)
                }
            };
            let handles: Vec<_> = (1..=shards).map(|i| scope.spawn(shard(i))).collect();
            handles.into_iter().enumerate().map(|(i, h)| h.join().unwrap_or_else(|_| (shard_log(log, i + 1), (Err("its shard's thread panicked".to_string()), String::new())))).collect()
        });
        let text = merged(&why, (shards, cores), &ran);
        if let Err(e) = std::fs::write(log, &text) {
            return verdict(Err(format!("bucket log {}: {e}", log.display())), &text, log);
        }
        proofs.record_log(&repo, &tip.pair(), &text, &format!("bucket tests {}", tip_short(tip)));
        verdict(worst(&ran), &text, log)
    })
}

/// A tests run's log's first line: its shards and the cores each asks (`SHARDS <n> <cores>`), how
/// a restarted master that adopts the run deals it again (a master just restarted may not know
/// every box yet).
const SHARDS: &str = "SHARDS ";

/// The shards and cores the run's log says ([`SHARDS`]).
fn run_shape(log: &Path) -> Option<(usize, u32)> {
    let text = std::fs::read_to_string(log).ok()?;
    let (n, cores) = text.lines().find_map(|l| l.strip_prefix(SHARDS))?.split_once(' ')?;
    Some((n.parse().ok().filter(|n| *n > 0)?, cores.trim().parse().ok()?))
}

/// [`test_boxes`], once there is one: a run waits for a pool box as a lease waits for a node.
fn pool_boxes() -> (usize, u32) {
    let mut said = false;
    loop {
        match test_boxes() {
            Ok(shape) => return shape,
            Err(e) if !said => {
                eprintln!("bucket tests: waits for its boxes: {e}");
                said = true;
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}

/// The boxes the tests bucket deals its units over: the up pool boxes, dev on, that meet
/// [`GATE_NEEDS`], one shard each (a box with room takes a second beside it, in the one tree
/// they share at the run's pin); and the cores each shard takes, the most a test lease gets on
/// the smallest of them.
fn test_boxes() -> Result<(usize, u32), String> {
    let c = crate::cli::control()?;
    pool_shape(&crate::cli::ask(&c, None, "nodes", "nodes")?)
}

/// The most cores a tests shard asks for (three units at once, land_gate.sh TEST_CORES): a whole
/// box's 16, asked at background priority, waited behind lane work on every box (L564088 queued
/// 6 min and more, 11-07). 8 ran two at once; 12, three, is one measured run's try (11-07): back
/// to 8 when contention eats the third unit's gain.
const SHARD_CORES: u32 = 12;

/// The RAM a box needs per tests shard: a 12-core shard's three units at once peaked at 27-29 GB
/// (11-07), charged that and a fifth ([`shard_class`]), up to its box's whole budget. A 30 GB pool
/// box never took one, and a 60 GB box takes one at a time: a shard dealt beyond that ran only
/// after a sibling's box freed, two rounds, 40-60 min of wall.
const SHARD_RAM_GB: f64 = 40.0;

/// A tests shard's measured class, its shape (its cores) in it: a shard's wall and peaks come from
/// what it runs at once, and another shape's are wrong data for it. An 8-core shard's 37.9 GB peak
/// charged the 12-core shards 45.5 GB each, where they peaked at 27-29 GB, and one waited for RAM
/// on a box that held it (11-07). The full bucket's shards ([`FULL`]: every package's tests, no
/// proof reused) are a class of their own: theirs ran 26-39 min against the tests bucket's 9
/// (L767799-801), and in one class the tests bucket's next shards asked large, measured 27 min,
/// with 18-30 GB each, and waited 10 min for RAM (11-10).
fn shard_class(cores: u32, full: bool) -> String {
    format!("test bash land_gate.sh testrun{} {cores} cores @{}", if full { " full" } else { "" }, crate::lease::sched::TESTS_LANE)
}

/// [`test_boxes`] of the master's `nodes` answer: how many shards, and the cores each asks: their
/// boxes' least test cores (a box's shared CPUs when its lease.conf pins them, else its cores), at
/// most [`SHARD_CORES`]. A box takes as many shards as it has those cores and [`SHARD_RAM_GB`]
/// for (none on a 30 GB box): every shard of a run starts at once, none waits for a sibling's box.
fn pool_shape(nodes: &[String]) -> Result<(usize, u32), String> {
    // each box: its test cores and how many shards its RAM holds
    let mut boxes: Vec<(u32, u32)> = Vec::new();
    for v in nodes.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
        let yes = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
        let Some(caps) = v.get("caps").filter(|_| yes("up") && yes("dev") && !yes("draining") && v.get("role").and_then(Value::as_str) == Some("pool")) else { continue };
        let text = |k: &str| caps.get(k).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        if text("os") != "linux" || !matches!(text("arch").as_str(), "x86_64" | "x64") { continue; }
        let by_ram = caps.get("ram_gb").and_then(crate::lease::num).map_or(1, |g| (g / SHARD_RAM_GB) as u32);
        let cores = caps.get("cpus_shared").and_then(Value::as_u64).filter(|n| *n > 0).or_else(|| caps.get("cores").and_then(Value::as_u64)).unwrap_or(0) as u32;
        if cores == 0 || by_ram == 0 { continue; }
        boxes.push((cores, by_ram));
    }
    let ask = boxes.iter().map(|(c, _)| *c).min().map(|least| least.min(SHARD_CORES)).ok_or_else(|| format!("no up pool box takes {GATE_NEEDS} leases"))?;
    Ok((boxes.iter().map(|(c, by_ram)| (c / ask).min(*by_ram) as usize).sum(), ask))
}

/// Shard `i`'s log beside the run's (`tests-<ms>.log`: `tests-s<i>-<ms>.log`): named after the
/// run's start as a bucket log is, and no bucket run of its own to a restarted master.
fn shard_log(log: &Path, i: usize) -> PathBuf {
    let stem = log.file_stem().and_then(|s| s.to_str()).unwrap_or("tests");
    let (name, ms) = stem.rsplit_once('-').unwrap_or((stem, "0"));
    log.with_file_name(format!("{name}-s{i}-{ms}.log"))
}

/// The run's log: its shape ([`SHARDS`]), its why lines, then each shard's run.
fn merged(why: &[String], (shards, cores): (usize, u32), ran: &[(PathBuf, (Result<Option<i32>, String>, String))]) -> String {
    let mut text = format!("{SHARDS}{shards} {cores}\n");
    text.extend(why.iter().map(|l| format!("{l}\n")));
    for (i, (shard, (_, part))) in ran.iter().enumerate() {
        text.push_str(&format!("== shard {}/{}: {}\n{part}", i + 1, ran.len(), shard.display()));
        if !part.ends_with('\n') { text.push('\n'); }
    }
    text
}

/// The run's status from its shards': a lease error, else a lost lease, else a failing exit,
/// else a run ended without one (a shard that exited 0 without its NODEGATE-OK too), else 0. A
/// shard the queue no longer needed ([`NOT_NEEDED`]) passes.
fn worst(ran: &[(PathBuf, (Result<Option<i32>, String>, String))]) -> Result<Option<i32>, String> {
    let said = |(status, part): &(Result<Option<i32>, String>, String)| match status {
        Ok(Some(0)) if !part.lines().any(|l| l.starts_with("NODEGATE-OK") || l == NOT_NEEDED) => Ok(None),
        other => other.clone(),
    };
    let all: Vec<Result<Option<i32>, String>> = ran.iter().map(|(_, r)| said(r)).collect();
    all.iter().find(|s| s.is_err()).cloned()
        .or_else(|| all.iter().find(|s| lost(s)).cloned())
        .or_else(|| all.iter().find(|s| matches!(s, Ok(Some(c)) if *c != 0)).cloned())
        .or_else(|| all.iter().find(|s| matches!(s, Ok(None))).cloned())
        .unwrap_or(Ok(Some(0)))
}

/// A tests run's shard's log line once it ended unneeded: the run's queue was dealt out before its
/// gate started (queued for its box, or in its setup), and the run's verdict waits for no box.
const NOT_NEEDED: &str = "bucket: the run's queue was dealt out before this shard's gate started: not needed";

/// How often the dealer looks at its shards' logs: each unit waits for its look once (474 units
/// over nine slots, a look of 0.25 s: 13 s of a run's 30 min).
const DEAL_LOOK: Duration = Duration::from_millis(250);

/// A tests run's one queue of units, dealt to its shards a unit at a time as each has a slot free
/// (land_gate.sh GATE_QUEUE: `GATEQUEUE <lease> <units> <digest>` once its queue is made, every
/// shard's the same, the slowest unit first; then `GATEWANT <lease> <n>` per free slot, answered in
/// the lease's `$DEVHUB_TOLD`, `<n> <index> <digest>` or `<n> -`: dealt out). A shard on a slow or
/// busy box takes fewer units: a deal fixed at the start left the run's wall to the shard beside
/// lane work, 1.20x its share (11-07). Each answer is in the run's `.log.dealt` before it is told,
/// so a restarted master deals on from it and the shards' logs.
#[derive(Default)]
struct Dealer {
    /// the queue's size and digest, as the first shard to make it said them
    queue: Option<(usize, String)>,
    /// the queue's first unit never dealt
    next: usize,
    /// units of a lease that ended without its verdict, dealt again first (the slowest first)
    back: BTreeSet<usize>,
    /// each want's answer, by its lease and number: a unit's index, or none (dealt out)
    dealt: BTreeMap<(String, u32), Option<usize>>,
    /// the lease each dealt unit last went to
    owner: BTreeMap<usize, String>,
    /// each shard's lease now (its log's last GATEQUEUE) and whether it said its verdict
    shards: Vec<(Option<String>, bool)>,
}

/// An answer to a want: the lease, the line its run is told, and its `.log.dealt` record.
#[derive(Debug, Clone, PartialEq)]
struct Told {
    lease: String,
    line: String,
    record: String,
}

impl Dealer {
    fn new(shards: usize) -> Dealer {
        Dealer { shards: vec![(None, false); shards], ..Dealer::default() }
    }

    /// The run's answers before a master's restart (its `.log.dealt`, `<lease> <n> <index|->`).
    fn replay(&mut self, text: &str) {
        for l in text.lines() {
            let [lease, n, what] = l.split_whitespace().collect::<Vec<_>>()[..] else { continue };
            let Ok(n) = n.parse::<u32>() else { continue };
            let index = what.parse::<usize>().ok();
            if let Some(i) = index { self.took(i, lease); }
            self.dealt.insert((lease.to_string(), n), index);
        }
    }

    fn took(&mut self, i: usize, lease: &str) {
        self.back.remove(&i);
        self.next = self.next.max(i + 1);
        self.owner.insert(i, lease.to_string());
    }

    fn told(lease: &str, n: u32, index: Option<usize>, digest: &str) -> Told {
        let (line, record) = match index {
            Some(i) => (format!("{n} {i} {digest}"), format!("{lease} {n} {i}")),
            None => (format!("{n} -"), format!("{lease} {n} -")),
        };
        Told { lease: lease.to_string(), line, record }
    }

    /// A line of shard `shard`'s log: a want not answered yet is answered.
    fn line(&mut self, shard: usize, line: &str) -> Option<Told> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["GATEQUEUE", lease, units, digest] => {
                let units = units.parse().ok()?;
                if self.queue.is_none() { self.queue = Some((units, digest.to_string())); }
                *self.shards.get_mut(shard)? = (Some(lease.to_string()), false);
                None
            }
            ["GATEWANT", lease, n] => {
                let n: u32 = n.parse().ok()?;
                let (units, digest) = self.queue.clone()?;
                if self.dealt.contains_key(&(lease.to_string(), n)) { return None; }
                let index = self.back.pop_first().or_else(|| (self.next < units).then_some(self.next));
                if let Some(i) = index { self.took(i, lease); }
                self.dealt.insert((lease.to_string(), n), index);
                Some(Self::told(lease, n, index, &digest))
            }
            _ if line.starts_with("NODEGATE-OK") || line.starts_with("NODEGATE-FAIL") => {
                if let Some((_, verdict)) = self.shards.get_mut(shard) { *verdict = true; }
                None
            }
            _ if line == RERUN => {
                self.over(shard);
                None
            }
            _ => None,
        }
    }

    /// Shard `shard`'s lease ended (its run did, or its log runs it again): one that said no verdict
    /// ran its units for nothing (their lines come at its end), so the ones it was the last to get
    /// are dealt again first.
    fn over(&mut self, shard: usize) {
        let Some((Some(lease), verdict)) = self.shards.get_mut(shard).map(std::mem::take) else { return };
        if !verdict {
            self.back.extend(self.owner.iter().filter(|(_, l)| **l == lease).map(|(i, _)| *i));
        }
    }

    /// Units left to deal, or none known yet: a shard whose gate has not started is still needed.
    fn needed(&self) -> bool {
        self.queue.as_ref().is_none_or(|(units, _)| self.next < *units) || !self.back.is_empty()
    }

    /// The answers of the shards' leases now: told again by a restarted master (a run reads its
    /// first answer to a want, so one told twice is harmless), the old master's last unsent too.
    fn answers_now(&self) -> Vec<Told> {
        let digest = self.queue.as_ref().map_or("", |(_, d)| d.as_str());
        let live: BTreeSet<&str> = self.shards.iter().filter_map(|(l, _)| l.as_deref()).collect();
        self.dealt.iter().filter(|((lease, _), _)| live.contains(lease.as_str())).map(|((lease, n), index)| Self::told(lease, *n, *index, digest)).collect()
    }

    fn live(&self, lease: &str) -> bool {
        self.shards.iter().any(|(l, _)| l.as_deref() == Some(lease))
    }
}

/// The lines a log grew by since `seen` (its offset and its unfinished last line); a log that
/// shrank was made again, read from its start.
fn grown(path: &Path, seen: &mut (u64, String)) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    if len < seen.0 { *seen = (0, String::new()); }
    let mut more = Vec::new();
    if f.seek(SeekFrom::Start(seen.0)).is_err() || f.read_to_end(&mut more).is_err() { return Vec::new(); }
    seen.0 += more.len() as u64;
    seen.1.push_str(&String::from_utf8_lossy(&more));
    let Some(cut) = seen.1.rfind('\n') else { return Vec::new() };
    let rest = seen.1.split_off(cut + 1);
    std::mem::replace(&mut seen.1, rest).lines().map(str::to_string).collect()
}

/// Says a shard's run ended when its thread does, a panic too: the dealer waits for them all.
struct Ends<'a>(&'a AtomicBool);

impl Drop for Ends<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The run's dealer ([`Dealer`]) while its shards run: each shard's log read as it grows, each want
/// answered through its lease's node ([`crate::lease::client::tell`]), its record in the run's
/// `.log.dealt` first. An answer not recorded or not told goes again at the next look, until its
/// lease ends.
fn deal(dealer: &Mutex<Dealer>, ended: &[AtomicBool], log: &Path) {
    let lock = || dealer.lock().unwrap_or_else(|e| e.into_inner());
    let dealt = log.with_extension("log.dealt");
    let earlier = std::fs::read_to_string(&dealt).unwrap_or_default();
    lock().replay(&earlier);
    let mut seen: Vec<(u64, String)> = vec![(0, String::new()); ended.len()];
    // answers not told yet, and whether each is recorded
    let mut unsent: Vec<(Told, bool)> = Vec::new();
    let mut adopted = !earlier.is_empty();
    let mut client = None;
    let mut said = BTreeSet::new();
    loop {
        let last = ended.iter().all(|e| e.load(Ordering::Acquire));
        for (i, end) in ended.iter().enumerate() {
            let over = end.load(Ordering::Acquire);
            let lines = grown(&shard_log(log, i + 1), &mut seen[i]);
            let mut d = lock();
            unsent.extend(lines.iter().filter_map(|l| d.line(i, l)).map(|t| (t, false)));
            if over { d.over(i); }
        }
        if std::mem::take(&mut adopted) {
            unsent.extend(lock().answers_now().into_iter().map(|t| (t, true)));
        }
        unsent.retain_mut(|(t, recorded)| {
            if !lock().live(&t.lease) { return false; }
            if !*recorded {
                let wrote = std::fs::OpenOptions::new().create(true).append(true).open(&dealt).and_then(|mut f| writeln!(f, "{}", t.record));
                if let Err(e) = wrote {
                    if said.insert(format!("record {e}")) { eprintln!("bucket tests: its queue's record {}: {e}", dealt.display()); }
                    return true;
                }
                *recorded = true;
            }
            let c = match client.take() {
                Some(c) => c,
                None => match crate::cli::control() {
                    Ok(c) => c,
                    Err(e) => {
                        if said.insert(format!("control {e}")) { eprintln!("bucket tests: its queue's dealer: {e}"); }
                        return true;
                    }
                },
            };
            match crate::lease::client::tell(&c, &t.lease, &t.line) {
                Ok(()) => {
                    client = Some(c);
                    false
                }
                Err(e) => {
                    if said.insert(format!("{} {e}", t.lease)) { eprintln!("bucket tests: telling {} its unit: {e}", t.lease); }
                    true
                }
            }
        });
        if last { return; }
        std::thread::sleep(DEAL_LOOK);
    }
}

/// The runs the tests bucket's units' last measures come from (land_gate.sh's GATETIME and
/// GATEPEAK lines, kept in each row's `unit_s` and `unit_mb`).
const TIMES_ROWS: usize = 30;

/// A run's units' measures by land_gate.sh's `<what> <unit> <n>` lines.
fn unit_measures(text: &str, what: &str) -> BTreeMap<String, u64> {
    text.lines().filter_map(|l| {
        let (unit, n) = l.strip_prefix(what)?.strip_prefix(' ')?.rsplit_once(' ')?;
        Some((unit.to_string(), n.trim().parse().ok()?))
    }).collect()
}

/// A run's units' seconds (land_gate.sh's `GATETIME <unit> <s>`).
fn unit_secs(text: &str) -> BTreeMap<String, u64> {
    unit_measures(text, "GATETIME")
}

/// A run's units' peak RSS in MB (land_gate.sh's `GATEPEAK <unit> <MB>`: the most its processes
/// held at once, their RSS summed at its watch's looks or one's own peak between them).
fn unit_mb(text: &str) -> BTreeMap<String, u64> {
    unit_measures(text, "GATEPEAK")
}

/// Each unit's last measured seconds in the bucket's last [`TIMES_ROWS`] runs, as land_gate.sh's
/// GATE_TIMES (`<unit>=<s>` joined by `;`), which its shards deal the slowest out by first.
/// `rows`: newest first.
fn times_env(rows: &[Value]) -> String {
    format!("GATE_TIMES={}", last_measures(rows, "unit_s").iter().map(|(u, s)| format!("{u}={s}")).collect::<Vec<_>>().join(";"))
}

/// Each unit's last measured peak RSS (MB) in the bucket's last [`TIMES_ROWS`] runs, as
/// land_gate.sh's GATE_PEAKS (`<unit>=<MB>` joined by `;`): a shard starts a unit it was dealt
/// once its running units' peaks and that one's fit the RAM its lease got (`DEVHUB_RAM_MB`).
fn peaks_env(rows: &[Value]) -> String {
    format!("GATE_PEAKS={}", last_measures(rows, "unit_mb").iter().map(|(u, mb)| format!("{u}={mb}")).collect::<Vec<_>>().join(";"))
}

/// A tests shard's RAM floor (`--ram-floor-gb`, GB): the largest of its units' last peaks
/// ([`peaks_env`]), the most one unit dealt to it alone needs; none before any is measured.
fn ram_floor(rows: &[Value]) -> Option<String> {
    last_measures(rows, "unit_mb").values().max().map(|mb| format!("{:.1}", *mb as f64 / 1024.0))
}

/// Each unit's newest measure of a row's `field` in the bucket's last [`TIMES_ROWS`] runs
/// (`rows`: newest first).
fn last_measures(rows: &[Value], field: &str) -> BTreeMap<String, u64> {
    let mut measures: BTreeMap<String, u64> = BTreeMap::new();
    for row in rows.iter().take(TIMES_ROWS) {
        if let Some(Value::Obj(units)) = row.get(field) {
            for (unit, n) in units {
                if let Some(n) = n.as_u64() { measures.entry(unit.clone()).or_insert(n); }
            }
        }
    }
    measures
}

/// How many times its last measure a tests unit runs before the check-in lists it ([`slow_units`]).
const SLOW_TIMES: u64 = 3;
/// And how long at least: a unit measured at seconds is no news at three times them.
const SLOW_FLOOR_S: u64 = 10 * 60;
/// On a box whose load is over [`OVER_LOAD`] times its cores, a unit past this many times its
/// measure is listed whatever its seconds, as the box oversubscribed.
const OVER_TIMES: u64 = 5;
const OVER_LOAD: f64 = 2.0;

/// A tests shard's box as the check-in sees it ([`slow_units`]): its node, whether its hub
/// answers, its load and cores.
pub struct ShardBox {
    pub node: String,
    pub up: bool,
    pub load: f64,
    pub cores: u32,
}

/// For the manager's check-in (`lane quiet`): the units of the tests run in flight that run past
/// [`SLOW_TIMES`] their last measure (and [`SLOW_FLOOR_S`]), by the live `GATESTART` and `GATEEND`
/// lines land_gate.sh's at_once says in each shard's log, each with what its box says
/// (`shard_box`: by the shard's lease, None once it ended): a hub that does not answer, a load
/// past [`OVER_LOAD`] times its cores (a unit past [`OVER_TIMES`] its measure there is listed
/// whatever its seconds), else where to look for a hang. A unit that hangs busy is no HUNG
/// (land_gate.sh's watch stops a unit only when its tests make no progress for 9 min, never one at
/// work), so this list is what says it: two render tests polled stalled GStreamer
/// pipelines at 4 % of a core for 73 min and held their shard at 98 min while the others took
/// 21 to 33, and nothing said so (11-07). Three units said 27x their measure on a box whose hub
/// had stopped answering, read as the box oversubscribed, while the other shards' units ran at
/// 1.0x theirs (11-07).
/// A unit whose tests gave no new result far past its measure (the watch's `GATEBUSY`: a spin,
/// which its watch never stops) is said as that instead, with the stack sample the watch took
/// then: six threads in `while !go` and a test thread spinning in a budget callback would have
/// held a shard and its box for ever with nothing said (11-09). It is a report, not a cap: main
/// or the unit's owner decides.
pub fn slow_units(home: &Path, now_ms: u64, shard_box: &dyn Fn(&str) -> Option<ShardBox>) -> Vec<String> {
    let rows = rows_of(home, "tests");
    let logs = home.join("bucket-logs");
    // the newest run with no row yet: the bucket runs one at a time, and a run's sidecar stays
    // after its verdict
    let Some(ms) = std::fs::read_dir(&logs).ok().into_iter().flatten().filter_map(|entry| {
        entry.ok()?.file_name().to_str()?.strip_prefix("tests-")?.strip_suffix(".log.run")?.parse::<u64>().ok()
    }).max().filter(|ms| !rows.iter().any(|r| r.get("started_ms").and_then(Value::as_u64) == Some(*ms))) else {
        return Vec::new();
    };
    let measures = last_measures(&rows, "unit_s");
    let log = logs.join(format!("tests-{ms}.log"));
    let mut out = Vec::new();
    for i in 1.. {
        let Ok(text) = std::fs::read_to_string(shard_log(&log, i)) else { break };
        let (lease, running, busy) = running_in(&text);
        let at = match lease {
            Some(lease) => match shard_box(lease) { Some(b) => Some((lease, b)), None => continue },
            None => None,
        };
        let over = at.as_ref().is_some_and(|(_, b)| b.up && b.load > OVER_LOAD * f64::from(b.cores));
        let who = at.as_ref().map_or("its lease".to_string(), |(lease, b)| format!("{lease} on {}", b.node));
        for (unit, b) in &busy {
            let measured = measures.get(*unit).map_or("not measured yet".to_string(), |m| format!("measured {}", took(*m)));
            let keep = shard_log(&log, i).with_extension("keep");
            let get = at.as_ref().map_or(String::new(), |(lease, _)| format!("`devhub get {} tmp/{}/{} <dir> --lease {lease}` now, ", crate::lease::sched::TESTS_LANE, crate::lease::KEEP_DIR, b.sample));
            let says = if b.says.is_empty() { String::new() } else { format!(": {}", b.says) };
            out.push(format!("tests run {ms} shard {i} ({who}): `{unit}` busy, no new result for {} ({measured}); nothing stops it: main or its owner decides. Its stack sample: {get}{} once its lease ends{says}", took((now_ms / 1000).saturating_sub(b.since)), keep.join(b.sample).display()));
        }
        for (unit, ran, measure) in slow_in(&running, &measures, now_ms / 1000, over) {
            if busy.contains_key(unit.as_str()) { continue; }
            let why = match &at {
                Some((_, b)) if !b.up => format!("{} does not answer: the seconds are its silence, not the unit's (its lease is lost if it stays so, and the shard runs again)", b.node),
                Some((_, b)) if over => format!("{} is oversubscribed: load {:.0} on {} cores", b.node, b.load, b.cores),
                _ => {
                    let node = at.as_ref().map_or("<node>", |(_, b)| b.node.as_str());
                    format!("its watch fails it HUNG, its state kept, once its tests make no progress for 9 min (no result, no CPU, no I/O; never while it builds or works): `devhub top {node}` shows its processes, `devhub inspect {node}/<pid> why-hung` where they wait")
                }
            };
            out.push(format!("tests run {ms} shard {i} ({who}): `{unit}` running {} against its measured {} ({}x); {why}", took(ran), took(measure), ran / measure.max(1)));
        }
    }
    out
}

/// A running unit its watch said stalled busy (unit_watch_body.sh's `GATEBUSY <unit> :: <since>
/// <sample> <says>`): since when (epoch s) its tests gave no new result, its stack sample's path
/// in the shard's keep dir, and what the sample says (the JIT's cause, the busiest thread).
#[derive(Debug, PartialEq)]
struct Busy<'a> {
    since: u64,
    sample: &'a str,
    says: &'a str,
}

/// A shard's log's lease (its last `GATEQUEUE <lease> ..`: a shard that lost its lease runs again
/// on a new one, and the lost one's units ended with it), the units it started since
/// (`GATESTART <unit> <epoch s>`) and did not end (`GATEEND <unit>`), by when each started, and
/// those of them its watch said stalled busy ([`Busy`]; `GATEBUSY <unit> :: 0`: a new result since).
fn running_in(text: &str) -> (Option<&str>, BTreeMap<&str, u64>, BTreeMap<&str, Busy<'_>>) {
    let (mut lease, mut running, mut busy) = (None, BTreeMap::new(), BTreeMap::new());
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("GATEQUEUE ") {
            lease = rest.split_whitespace().next();
            running.clear();
            busy.clear();
        } else if let Some((unit, at)) = l.strip_prefix("GATESTART ").and_then(|r| r.rsplit_once(' ')) {
            if let Ok(at) = at.trim().parse() { running.insert(unit, at); }
        } else if let Some(unit) = l.strip_prefix("GATEEND ") {
            running.remove(unit.trim_end());
            busy.remove(unit.trim_end());
        } else if let Some((unit, said)) = l.strip_prefix("GATEBUSY ").and_then(|r| r.split_once(" :: ")) {
            let mut w = said.splitn(3, ' ');
            match (w.next().and_then(|n| n.parse::<u64>().ok()), w.next()) {
                (Some(since), Some(sample)) if since > 0 && running.contains_key(unit) => {
                    busy.insert(unit, Busy { since, sample, says: w.next().unwrap_or("").trim() });
                }
                _ => { busy.remove(unit); }
            }
        }
    }
    (lease, running, busy)
}

/// The running units ([`running_in`]) the check-in lists by `now_s`: past [`SLOW_TIMES`] their
/// measure and [`SLOW_FLOOR_S`], or on a box oversubscribed (`over`) past [`OVER_TIMES`] it: each
/// unit, its seconds, its measure. A unit not measured yet is not judged.
fn slow_in(running: &BTreeMap<&str, u64>, measures: &BTreeMap<String, u64>, now_s: u64, over: bool) -> Vec<(String, u64, u64)> {
    running.iter().filter_map(|(unit, at)| {
        let (ran, measure) = (now_s.saturating_sub(*at), *measures.get(*unit)?);
        let slow = ran >= SLOW_FLOOR_S && ran >= SLOW_TIMES * measure;
        (slow || (over && ran >= OVER_TIMES * measure.max(1))).then(|| (unit.to_string(), ran, measure))
    }).collect()
}

/// Seconds as a check-in says them.
fn took(s: u64) -> String {
    if s >= 120 { format!("{} min", s / 60) } else { format!("{s} s") }
}

/// Boots the apps the landings touched on a Linux node's warm JIT (one that presents), right
/// after each landing. The Macs take only work that requires macOS.
pub fn hot(repo: PathBuf, home: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        let base = base.expect("hot bucket needs a base");
        let retry = retry_env(&rows_of(&home, "hot"));
        // a warm boot of one or two apps needs far less than a test lease's default 6 cores / 7 GB
        let needs = format!("{GATE_NEEDS},gui=1");
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let lease = ["lease", "test", "--needs", &needs, "--cost", "small", "--cores", "3", "--ram-gb", "5", "--priority", "interactive", "--lane", "landq-hot", "--rev", &rev];
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let gate = ["env", &retry, &nested, "bash", "tools/run/fixture/land_gate.sh", "hot", &bases.work, &bases.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &[], log, (base, tip), &|| true);
        verdict(status, &text, log)
    })
}

/// Runs land_gate.sh under a devhub lease, its output in `log` after the `why` lines; a failed
/// step's logs, which go with the node's scratch at the lease's end, in `<log>.keep` beside it
/// (`lease --keep`). A lost lease ([`crate::lease::client::LOST_EXIT`]) runs it once more in the
/// same log, after [`RERUN`]; the text is the last run's. Once `needed` says no, a lease whose
/// gate has not started (its log's run has no GATEQUEUE line yet: queued, or in its setup) ends,
/// its holder stopped by its pid, and the run passes [`NOT_NEEDED`]: a tests run's verdict waited
/// for a box the queue no longer needed.
///
/// The holder writes its own exit beside the log as it ends (`lease --exit-to`, `<log>.exit`), so
/// a master that adopts the run after a restart has it whether or not it is the holder's parent: an
/// exec keeps the children, a crash or a switch-over does not, and a shard that had ended before
/// the restart is no process to wait for at all. Its exit is the file's; one stopped as not needed
/// before the restart is said so in its log. A tests run adopted at 16:51Z (11-08) read a shard the
/// old master had stopped as not needed as one with no exit status: no verdict on the run.
fn lease_run(repo: &Path, lease: &[&str], gate: &[&str], why: &[String], log: &Path, (base, tip): (&Pin, &Pin), needed: &dyn Fn() -> bool) -> (Result<Option<i32>, String>, String) {
    let sidecar = log.with_extension("log.run");
    let exit = log.with_extension("log.exit");
    let note = |line: &str| if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) { let _ = writeln!(file, "{line}"); };
    let unneeded = || !needed() && !last_run(&std::fs::read_to_string(log).unwrap_or_default()).lines().any(|l| l.starts_with("GATEQUEUE "));
    let not_needed = || -> Result<Option<i32>, String> {
        note(NOT_NEEDED);
        Ok(Some(0))
    };
    let execute = |again: bool| -> Result<Option<i32>, String> {
        if sidecar.exists() {
            let (pid, _, _, _) = read_run(&sidecar).ok_or_else(|| format!("invalid bucket run {}", sidecar.display()))?;
            let mut stopped = false;
            let stopped_before = || last_run(&std::fs::read_to_string(log).unwrap_or_default()).lines().any(|l| l == NOT_NEEDED);
            loop {
                // (the holder writes its file before it exits: read once it is gone)
                if let Some(code) = crate::procinfo::exited(pid) {
                    return match crate::lease::client::read_exit(&exit, pid) {
                        _ if stopped => not_needed(),
                        Some(code) => Ok(Some(code)),
                        None if stopped_before() => Ok(Some(0)),
                        None => Ok(code),
                    };
                }
                if !stopped && unneeded() {
                    crate::os::signal_one(pid, true);
                    stopped = true;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let started = log_started(log).ok_or_else(|| format!("invalid bucket log {}", log.display()))?;
        let own = crate::cli::own_argv(repo);
        let mut file = if again { std::fs::OpenOptions::new().append(true).open(log) } else { std::fs::File::create(log) }.map_err(|e| e.to_string())?;
        if !again { for line in why { writeln!(file, "{line}").map_err(|e| e.to_string())?; } }
        let err = file.try_clone().map_err(|e| e.to_string())?;
        // (a run again in the same log: the last holder's exit is not this one's)
        match std::fs::remove_file(&exit) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", exit.display())),
            _ => {}
        }
        let mut child = std::process::Command::new(&own[0]).args(&own[1..]).args(lease).arg("--keep").arg(log.with_extension("keep")).arg("--exit-to").arg(&exit).arg("--").args(gate)
            .current_dir(repo).env("DEVHUB_AGENT", crate::lease::sched::BUCKET_AGENT)
            .stdin(std::process::Stdio::null()).stdout(file).stderr(err).spawn().map_err(|e| e.to_string())?;
        let saved = save_run(log, child.id(), started, base, tip);
        let mut stopped = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(_)) if stopped => break not_needed(),
                Ok(Some(s)) => break Ok(s.code()),
                Ok(None) if !stopped && unneeded() => {
                    let _ = child.kill();
                    stopped = true;
                }
                Ok(None) => std::thread::sleep(Duration::from_secs(1)),
                Err(e) => break Err(e.to_string()),
            }
        };
        saved.map_err(|e| format!("bucket run: {e}"))?;
        status
    };
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

/// A run passes on exit 0 with NODEGATE-OK and no failing unit. One that ended at exit 0 or
/// without an exit status and says neither NODEGATE-OK nor NODEGATE-FAIL nor a failing unit is no
/// verdict on the landings: what is missing and where its log ended are said, and the bucket
/// takes the tip again (a gate the restarted master adopted had been killed by the Mac's reboot,
/// still queued for its lease, and read as a failed run of #2259, 11-06).
fn verdict(status: Result<Option<i32>, String>, text: &str, log: &Path) -> Outcome {
    let lost_run = lost(&status);
    let mut failing: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("GATEUNIT fail ").map(str::to_string)).collect();
    let ok = failing.is_empty() && status.as_ref().is_ok_and(|s| *s == Some(0)) && text.lines().any(|l| l.starts_with("NODEGATE-OK"));
    let mut unsaid = None;
    if !ok && failing.is_empty() {
        for line in text.lines().filter_map(|l| l.strip_prefix("NODEGATE-FAIL")) {
            let steps = line.trim_start_matches(':').split(" [").next().unwrap_or("");
            failing.extend(steps.split_whitespace().map(str::to_string));
        }
        if failing.is_empty() {
            let end = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map_or("nothing".to_string(), |l| format!("`{}`", crate::proto::clean(l, 160)));
            match status {
                Ok(Some(0)) => unsaid = Some(format!("the gate exited 0 without a NODEGATE-OK or NODEGATE-FAIL line; its log ends at {end}")),
                Ok(None) => unsaid = Some(format!("the gate's process ended without an exit status (a signal, or a run adopted after a master restart that ended unwatched: a reboot) before its verdict; its log ends at {end}")),
                Ok(code) => failing.push(format!("no verdict (exit {code:?})")),
                Err(e) => failing.push(format!("gate lease: {e}")),
            }
        }
    }
    failing.sort();
    failing.dedup();
    let summary = text.lines().filter(|l| l.starts_with("STEP ")).collect::<Vec<_>>().join(", ");
    let no_verdict = if lost_run { Some("its lease was lost under the gate twice (the fleet, not the landings)".to_string()) } else { unsaid };
    Outcome { ok, failing, log: log.display().to_string(), summary, no_verdict }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_no_verdict_run_retires_its_sidecar_but_other_errors_keep_it() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-no-verdict-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let log = home.join("ci-1.log");
        let sidecar = log.with_extension("log.run");
        let store = Store::open_quiet(&home, "").unwrap();
        let tip = Pair::new(&"a".repeat(40), "");
        save_run(&log, 1, 1, &pin_of(&tip), &pin_of(&tip)).unwrap();
        let error = "no verdict on a: unavailable".to_string();
        assert!(finish_run(&log, Err(JudgeError::Other(error))).is_err());
        assert!(sidecar.exists(), "an ordinary error keeps the run even with no-verdict text");
        let outcome = Outcome { ok: false, failing: Vec::new(), log: log.display().to_string(), summary: "incomplete".into(), no_verdict: Some("unavailable".into()) };
        let result = judge("ci", &home, &home, &store, &|_, _, _| {}, 1, None, &pin_of(&tip), |_| Ok(outcome));
        assert!(matches!(finish_run(&log, result), Err(JudgeError::NoVerdict(_))));
        assert!(!sidecar.exists(), "a no-verdict end must never be adopted again");
        assert!(rows_of(&home, "ci").is_empty(), "no verdict writes no ledger row");
        assert!(matches!(finish_run(&log, Err(JudgeError::NoVerdict("unavailable".into()))), Err(JudgeError::NoVerdict(_))), "an absent sidecar is already retired");
        std::fs::remove_dir_all(&home).unwrap();
    }

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
        // ended at exit 0, or with no exit status (adopted, killed with the Mac), before any verdict
        // line: no verdict on the landings, and what is missing is said
        let queued = "why\nL5 gate: 1st in queue (node-a: waits for room)\n";
        for status in [Ok(Some(0)), Ok(None)] {
            let o = verdict(status, queued, Path::new("x.log"));
            assert!(!o.ok && o.failing.is_empty(), "{:?}", o.failing);
            let why = o.no_verdict.unwrap();
            assert!(why.contains("its log ends at `L5 gate: 1st in queue (node-a: waits for room)`"), "{why}");
        }
        assert!(verdict(Ok(Some(0)), "", Path::new("x.log")).no_verdict.unwrap().contains("NODEGATE-OK or NODEGATE-FAIL"));
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
        // a present's summary carries the error in a tab-separated record whose next field
        // changes per run: the same break, run after run, and against a row that kept the field
        let present = |n: u32| format!("app: no present (failed before its first frame: error\t/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\t{n} )error\t/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\t{n} ; its last log lines:\n/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\nGATEUNIT fail present app\n");
        let (first, _) = unit_lines(&present(7435));
        assert_eq!(first["present app"], vec!["sla.rs: E0063: missing field `stack`".to_string()]);
        let old = obj(vec![("errors", obj(vec![("present app", Value::Arr(vec![s("sla.rs: E0063: missing field `stack`\t7435 )error\t/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\t7435 ; its last log lines:")]))])), ("broke", obj(vec![("present app", s("rc-b (B) #1926"))]))]);
        let (next, _) = unit_lines(&present(1030));
        assert_eq!(carried_since(&[old], &failing(&next), &next).get("present app").map(String::as_str), Some("rc-b (B) #1926"), "the breaker, not the next lander");
    }

    /// The same present failure keys the same in two runs. The runs differ in the JIT's [t=N ms],
    /// the pid, the thread id, an address, the tree, the lease's tmp, and which log line comes
    /// first. The gate's settle_errors.sh drops what varies before it cuts each record. While the
    /// cut fell elsewhere in each run, the same JIT break in vm.rs was named for the next lander (11-07).
    #[test]
    fn a_present_failure_keys_the_same_in_every_run() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/settle_errors.sh");
        let root = std::env::temp_dir().join(format!("devhub-bucket-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let run = |tree: &str, t: u32, pid: u32, lease: &str, parked_first: bool| -> String {
            let (tree, tmp) = (root.join(tree), root.join(lease));
            let s = tmp.join("landgate.x").join("hrun-app");
            std::fs::create_dir_all(&tree).unwrap();
            std::fs::create_dir_all(&s).unwrap();
            let (tree, tmp) = (tree.canonicalize().unwrap(), tmp.canonicalize().unwrap());
            let fun = "makepad_script_std::vm::ScriptVmCall::resume_if_panicked";
            let at = format!("{}/platform/script/std/src/vm.rs:157:36", tree.display());
            let compile = format!("makepad-run: [t={t} ms] compile error in {fun}: {at}: `<makepad_script_std::vm::ScriptVmCall>::resume_if_panicked` calls `Box::from_raw` of a provided crate at 0x7f{pid:x}0038: the JIT cannot take its frame");
            let parked = format!("makepad: compile error parked on thread main ({pid}) at its call of code that did not compile, or stop: makepad inspect {pid} panics (log {}/makepad-run-session-{pid}-52951.log)", tmp.display());
            let log = if parked_first { format!("{parked}\n{compile}\n") } else { format!("{compile}\n{parked}\n") };
            std::fs::write(s.join("out.log"), log).unwrap();
            std::fs::write(s.join("report"), format!("present 0\nerror\tcompile error in {fun}: {at}: `<makepad_script_std::vm::ScriptVmCall>::resume_if_panicked` calls\t{pid}\n")).unwrap();
            let o = std::process::Command::new("bash").arg(&script).arg(&s).current_dir(&tree).env("PWD", &tree).env("TMPDIR", &tmp).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            // as land_gate.sh's present says it
            format!("app: no present (failed before its first frame: {}); its last log lines:\nGATEUNIT fail present app\n", String::from_utf8_lossy(&o.stdout))
        };
        let first = run("ci-tree", 12657, 649131, "L618033", false);
        let next = run("native-lane", 8122, 2955436, "L7", true);
        let (a, b) = (unit_lines(&first).0, unit_lines(&next).0);
        assert_eq!(a["present app"], b["present app"], "{first}\n{next}");
        let key = &a["present app"][0];
        assert!(key.starts_with("vm.rs: `<makepad_script_std::vm::ScriptVmCall>::resume_if_panicked` calls `Box::"), "{key}");
        assert!(!key.contains("649131") && !key.contains("2955436") && !key.contains("0x7f"), "{key}");
        // the summary names the tree's file, not the box's path to it
        assert!(first.contains(" platform/script/std/src/vm.rs:157:36: ") && !first.contains("ci-tree") && !first.contains("L618033"), "{first}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A unit whose process parks fails HUNG at the gate's next look, with the park's text and the
    /// compile error it parked at, and its processes are stopped. Nobody resumes a park in a gate:
    /// an app a test started parked at a compile error and held its unit 64 min (11-07). A panic
    /// that parks for less than a look goes on by itself and is no hang.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_whose_process_parks_is_hung_at_the_next_look() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/unit_watch.sh");
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        let ended = |child: &mut std::process::Child, looks: u32| (0..looks * 10).any(|_| {
            if child.try_wait().unwrap().is_some() { return true; }
            std::thread::sleep(Duration::from_millis(100));
            false
        });
        // (the unit's output, the session log of a JIT app of it) -> its .hung, and whether its process was stopped
        let watch = |own: &str, app: &str| -> (Option<String>, bool) {
            // the unit in a session of its own, as the gate's `watched` starts it
            let mut unit = std::process::Command::new("setsid").args(["sleep", "300"]).spawn().unwrap();
            let sid = unit.id();
            // (once setsid is in place: the spawn returns before the child's exec)
            let leads = || std::fs::read_to_string(format!("/proc/{sid}/stat")).ok()
                .is_some_and(|stat| stat.rsplit(") ").next().and_then(|s| s.split_whitespace().nth(3)) == Some(sid.to_string().as_str()));
            assert!((0..50).any(|_| leads() || { std::thread::sleep(Duration::from_millis(20)); false }), "the unit leads its session");
            let out = root.join(format!("unit-{sid}"));
            std::fs::write(&out, own).unwrap();
            std::fs::write(root.join("tmp").join(format!("makepad-run-session-{sid}-1.log")), app).unwrap();
            let mut w = std::process::Command::new("bash").arg(&script).arg(sid.to_string()).arg(&out)
                .env("HUNG_LOOK", "1").env("HUNG_QUIET", "600").env("TMPDIR", root.join("tmp")).env("B", "true").spawn().unwrap();
            // (a look, then a second of its threads' CPU and the capture)
            if !ended(&mut w, 4) {
                let _ = std::process::Command::new("kill").arg(w.id().to_string()).status();
                let _ = w.wait();
            }
            let stopped = ended(&mut unit, 1);
            if !stopped { let _ = unit.kill(); let _ = unit.wait(); }
            assert!(std::fs::metadata(format!("{}.peak", out.display())).is_ok(), "its peak, hung or not");
            (std::fs::read_to_string(format!("{}.hung", out.display())).ok(), stopped)
        };
        let compile = "makepad-run: compile error in lazy_typeck_dep::broken: libs/dep/src/lib.rs:3:5: mismatched types\n\
            makepad: compile error parked on thread main at its call of code that did not compile: edit the code (a live edit that makes it compile resumes the call), or stop: makepad inspect 7 panics (resume P1 stop)\n";
        let (hung, stopped) = watch("running 1 test\n", compile);
        let hung = hung.expect("a parked compile error is a hang");
        assert!(hung.starts_with("== a process of it parked, which nobody resumes in a gate: makepad-run: compile error in lazy_typeck_dep::broken: libs/dep/src/lib.rs:3:5: mismatched types; makepad: compile error parked on thread main"), "{hung}");
        assert!(stopped, "its processes are stopped");
        // a panic parked as watched, in the unit's own output
        let (hung, _) = watch("running 1 test\nmakepad: panic parked on thread main (index out of bounds) for 600 s -- attach: makepad inspect 7 panics (resume P1 panic)\n", "");
        assert!(hung.is_some_and(|h| h.contains("panic parked on thread main (index out of bounds) for 600 s")));
        // a brief park goes on by itself
        let (hung, stopped) = watch("running 1 test\n", "makepad: panic parked on thread main (boom) for 1 s -- attach: makepad inspect 7 panics (resume P1 panic)\n");
        assert!(hung.is_none() && !stopped, "{hung:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A unit whose processes only move bytes between themselves (a test polling its app's remote:
    /// socket traffic, under 1% of a core) does nothing, and is hung after its quiet. A test polled
    /// its parked app every 50 ms, read as busy, and held its unit 64 min (11-07). A unit that
    /// works on a core is busy.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_that_only_talks_to_itself_is_hung_after_its_quiet() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/unit_watch.sh");
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-quiet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // whether the watch said HUNG within 6 looks of a quiet of 3 s
        let hung = |program: &str| -> bool {
            let mut unit = std::process::Command::new("setsid").args(["python3", "-I", "-c", program]).spawn().unwrap();
            let sid = unit.id();
            let leads = || std::fs::read_to_string(format!("/proc/{sid}/stat")).ok()
                .is_some_and(|stat| stat.rsplit(") ").next().and_then(|s| s.split_whitespace().nth(3)) == Some(sid.to_string().as_str()));
            assert!((0..50).any(|_| leads() || { std::thread::sleep(Duration::from_millis(20)); false }), "the unit leads its session");
            let out = root.join(format!("unit-{sid}"));
            std::fs::write(&out, "running 1 test\n").unwrap();
            let mut w = std::process::Command::new("bash").arg(&script).arg(sid.to_string()).arg(&out)
                .env("HUNG_LOOK", "1").env("HUNG_QUIET", "3").env("TMPDIR", &root).env("B", "true").spawn().unwrap();
            let ended = (0..60).any(|_| w.try_wait().unwrap().is_some() || { std::thread::sleep(Duration::from_millis(100)); false });
            if !ended {
                let _ = std::process::Command::new("kill").arg(w.id().to_string()).status();
                let _ = w.wait();
            }
            let _ = unit.kill();
            let _ = unit.wait();
            std::fs::read_to_string(format!("{}.hung", out.display())).is_ok_and(|h| h.starts_with("== its processes did nothing"))
        };
        // 2.6 MB/s through a socket pair, back and forth in one process
        let chatter = "import socket, time\na, b = socket.socketpair()\nbuf = b'x' * 32768\nwhile True:\n    for _ in range(8):\n        a.sendall(buf)\n        n = 0\n        while n < len(buf):\n            n += len(b.recv(65536))\n    time.sleep(0.1)\n";
        assert!(hung(chatter), "socket traffic is no work");
        assert!(!hung("while True:\n    pass\n"), "a core's work is");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A unit is hung only when its tests make no progress for the quiet: no new test result and its
    /// processes doing nothing, counted while a target's tests run. A build is never a hang (a cold
    /// JIT's compile, its wait for the box's JIT store), nor is a unit at work however long it takes:
    /// a bound from its warm measure failed a cold JIT's `makepad-fab --bin` HUNG at 672 s against a
    /// warm 66 s, its first frame at 471 s and a test thread at a core in JIT code (11-09). The watch's
    /// body reads a fake /proc and a fake output on a fake clock here: a real spinning unit on a
    /// loaded box's tests got under a tenth of a core (11-08). Its peak (GATEPEAK) is the most it held
    /// at once, never its processes' own peaks summed: those read a force_mode unit's 12 GB as 30 GB
    /// (11-10).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_is_hung_only_when_its_tests_make_no_progress() {
        let body = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/unit_watch_body.sh");
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // its session's one process, a pid past Linux's limit (2^22): none the watch's kill reaches
        let pid = 4_194_401u32;
        // the watch on a fake clock, a wait taking no time: second t's /proc is <case>/proc/<t>, its
        // processes `mem(t)`'s (pid, command line, VmHWM and VmRSS in KB), the session's own at `cpu(t)`
        // ticks; its output is `outs`' text from the second each is given on; a stall is said after 5 s
        // with no new result. What it kept: the first lines of <out>.hung and <out>.busy, <out>.peak,
        // what it said live (its fd 4) and <out>.peak.by
        let watch = |case: &str, cpu: &dyn Fn(u64) -> u64, outs: &[(u64, &str)], mem: &dyn Fn(u64) -> Vec<(u32, &'static str, u64, u64)>| -> (String, String, String, String, String) {
            let dir = root.join(case);
            let proc = dir.join("proc");
            for t in 0..=40u64 {
                for (p, cmd, hwm, rss) in mem(t) {
                    let task = proc.join(t.to_string()).join(p.to_string());
                    let thread = task.join("task").join(p.to_string());
                    std::fs::create_dir_all(&thread).unwrap();
                    let comm = cmd.split(' ').next().unwrap();
                    let stat = format!("{p} ({comm}) R 1 {p} {pid} 0 -1 4194304 100 0 0 0 {} 0 0 0 20 0 1 0\n", if p == pid { cpu(t) } else { 0 });
                    for dir in [&task, &thread] {
                        std::fs::write(dir.join("stat"), &stat).unwrap();
                        std::fs::write(dir.join("wchan"), "0").unwrap();
                    }
                    std::fs::write(task.join("io"), "read_bytes: 0\nwrite_bytes: 0\n").unwrap();
                    std::fs::write(task.join("status"), format!("VmPeak:\t{hwm} kB\nVmHWM:\t{hwm} kB\nVmRSS:\t{rss} kB\nVmData:\t{rss} kB\n")).unwrap();
                    std::fs::write(task.join("cmdline"), cmd.split(' ').map(|a| format!("{a}\0")).collect::<String>()).unwrap();
                }
            }
            let out = dir.join("unit");
            std::fs::write(&out, outs.iter().find(|(t, _)| *t == 0).map_or("", |(_, text)| *text)).unwrap();
            for (t, text) in outs {
                std::fs::write(format!("{}.at{t}", out.display()), text).unwrap();
            }
            let script = dir.join("fake_watch.sh");
            std::fs::write(&script, format!(
                "now=0 nap=\"\" proc={0}/0\nwait_s() {{ now=$((now + $1)); proc={0}/$now; [ ! -f \"$out.at$now\" ] || cp \"$out.at$now\" \"$out\"; }}\nread_gpu() {{ :; }}\nwall() {{ echo $((1794000000 + now)); }}\nexec 4> \"$2.live\"\n. {1}\n",
                proc.display(), body.display())).unwrap();
            assert!(std::process::Command::new("bash").arg(&script).arg(pid.to_string()).arg(&out)
                .env("HUNG_LOOK", "1").env("HUNG_QUIET", "2").env("BUSY_S", "5").env("BUSY_UNIT", "test p").env("DEVHUB_KEEP", dir.join("keep"))
                .env("TMPDIR", &dir).env("B", "true").status().unwrap().success());
            let kept = |ext: &str| std::fs::read_to_string(format!("{}.{ext}", out.display())).unwrap_or_default();
            let first = |ext: &str| kept(ext).lines().next().unwrap_or("").to_string();
            if !kept("busy").is_empty() {
                assert_eq!(std::fs::read_to_string(dir.join("keep").join(case).join("unit.busy")).unwrap(), kept("busy"), "{case}: its sample in the lease's keep dir");
            }
            (first("hung"), kept("peak"), first("busy"), kept("live"), kept("peak.by"))
        };
        let one = |_: u64| vec![(pid, "python3", 10_240u64, 10_240u64)];
        const GB: u64 = 1 << 20;
        // the test's process at 4 GB from 2 s to 5 s, a build it set off at half a GB and then at 3 GB from
        // 5 s, gone at 8 s: 4.5 GB at once at most, against 7 GB of their own peaks summed
        let two = |t: u64| {
            let mut ps = vec![(pid, "python3 -I -c spin", if t >= 2 { 4 * GB } else { GB }, if (2..5).contains(&t) { 4 * GB } else { GB })];
            if t < 8 { ps.push((pid + 1, "rustc --crate-name dep", if t >= 5 { 3 * GB } else { GB / 2 }, if t >= 5 { 3 * GB } else { GB / 2 })); }
            ps
        };
        let hung_at = |s: u64| format!("== its processes did nothing (no test result, under 5% of a core, no I/O) for 2 s while its tests ran, {s} s into the unit; their state");
        // a core's work each second for 20 s, far past any measure, one result at 8 s, then nothing
        let (hung, peak, busy, live, by) = watch("busy", &|t| 100 * t.min(20), &[(0, "running 2 tests\n"), (8, "running 2 tests\ntest a ... ok\n")], &two);
        assert_eq!(hung, hung_at(22), "busy: a unit at work is no hang, however long it works");
        assert_eq!(peak.trim(), "4608", "busy: its peak in MB is the most its processes held at once, not their own peaks summed (7168)");
        assert_eq!(by.trim(), "4.0 GB python3 -I -c spin; 0.5 GB rustc --crate-name dep", "busy: its processes at its peak, largest first, by command");
        let top = "the busiest thread: 4194401/task/4194401 python3 R 100% wchan 0";
        assert_eq!(busy, format!("== busy: no new test result for 5 s while its tests ran, 13 s into the unit; nothing stops it; {top}; their state"), "busy: its last stall's sample");
        assert_eq!(live, format!("GATEBUSY test p :: 1794000000 busy/unit.busy {top}\nGATEBUSY test p :: 0\nGATEBUSY test p :: 1794000008 busy/unit.busy {top}\n"),
            "busy: each stall said once with its last result's time, a new result clearing it");
        // nothing for 10 s before its target's tests start (its build waits), then nothing
        // its process at 2 GB for a moment between two looks, held at 0.1 GB at each
        let spike = |t: u64| vec![(pid, "python3", if t >= 3 { 2 * GB } else { GB / 10 }, GB / 10)];
        let (hung, peak, busy, live, by) = watch("build", &|_| 0, &[(0, ""), (10, "running 1 test\n")], &spike);
        assert_eq!(hung, hung_at(11), "build: a build is never a hang; the quiet counts from its tests' start");
        assert_eq!((busy.as_str(), live.as_str()), ("", ""), "build: a build is no stall");
        assert_eq!((peak.trim(), by.trim()), ("2048", "2.0 GB python3 (its own peak, between looks)"), "build: one process's own peak between looks counts");
        // its processes idle, a test result every 2 s
        let (hung, peak, busy, live, _) = watch("results", &|_| 0, &[(0, "running 3 tests\n"), (2, "running 3 tests\ntest a ... ok\n"),
            (4, "running 3 tests\ntest a ... ok\ntest b ... ok\n"), (6, "running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n")], &one);
        assert_eq!(peak.trim(), "10", "results: its peak in MB");
        assert_eq!(hung, hung_at(8), "results: a new test result is progress");
        assert_eq!((busy.as_str(), live.as_str()), ("", ""), "results: no stall");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A shard's class names its shape and its bucket: another shape's or the full bucket's
    /// measures are not its own.
    #[test]
    fn a_shards_class_is_its_shape_and_its_bucket() {
        assert_eq!(shard_class(12, false), format!("test bash land_gate.sh testrun 12 cores @{}", crate::lease::sched::TESTS_LANE));
        assert_ne!(shard_class(8, false), shard_class(12, false), "another shape measures apart");
        assert_eq!(shard_class(12, true), format!("test bash land_gate.sh testrun full 12 cores @{}", crate::lease::sched::TESTS_LANE), "the full bucket's measure apart");
    }

    /// The tests bucket takes a shard per SHARD_CORES of test cores on each up Linux x64 pool box
    /// with dev on and the RAM for one (its shared CPUs when pinned), each shard with the test
    /// cores of the smallest, at most SHARD_CORES; a bench, a user's box, a Mac, a down, draining
    /// or small-RAM box takes none.
    #[test]
    fn the_tests_bucket_takes_a_shard_per_pool_box() {
        let node_ram = |name: &str, role: &str, up: bool, dev: bool, draining: bool, os: &str, arch: &str, cores: u32, shared: u32, ram: f64| format!(
            r#"{{"node":"{name}","up":{up},"dev":{dev},"role":"{role}","draining":{draining},"caps":{{"os":"{os}","arch":"{arch}","cores":{cores},"cpus_shared":{shared},"ram_gb":{ram}}}}}"#);
        let node = |name: &str, role: &str, up: bool, dev: bool, draining: bool, os: &str, arch: &str, cores: u32, shared: u32| node_ram(name, role, up, dev, draining, os, arch, cores, shared, 62.0);
        let nodes = vec![
            node("boxa", "pool", true, true, false, "linux", "x86_64", 16, 0),
            node("boxb", "pool", true, true, false, "linux", "x86_64", 32, 16),
            node("boxc", "pool", true, true, false, "linux", "x86_64", 32, 0),
            node("bench", "bench", true, true, false, "linux", "x86_64", 8, 0),
            node("userbox", "user-wm", true, true, false, "linux", "x86_64", 8, 0),
            node("mac", "pool", true, true, false, "macos", "aarch64", 8, 0),
            node("win", "pool", true, true, false, "windows", "x86_64", 8, 0),
            node("down", "pool", false, true, false, "linux", "x86_64", 8, 0),
            node("off", "pool", true, false, false, "linux", "x86_64", 8, 0),
            node("leaving", "pool", true, true, true, "linux", "x86_64", 8, 0),
        ];
        // boxa 1, boxb (pinned to 16 of 32) 1, boxc (32 cores, RAM for one) 1
        assert_eq!(pool_shape(&nodes), Ok((3, SHARD_CORES)));
        let lean = node_ram("lean", "pool", true, true, false, "linux", "x86_64", 16, 0, 30.0);
        assert_eq!(pool_shape(&[nodes[0].clone(), lean]), Ok((1, SHARD_CORES)), "a box short of a shard's RAM takes none");
        let big = node_ram("big", "pool", true, true, false, "linux", "x86_64", 32, 0, 96.0);
        assert_eq!(pool_shape(&[nodes[0].clone(), big]), Ok((3, SHARD_CORES)), "the cores and RAM for two: two");
        assert_eq!(pool_shape(&[node("small", "pool", true, true, false, "linux", "x86_64", 4, 0)]), Ok((1, 4)), "a smaller box: its cores");
        assert!(pool_shape(&nodes[3..]).is_err(), "no box: no run");
    }

    /// The run's one queue: each want gets the next unit, the slowest first; a lease that ends
    /// without its verdict gives its units back, dealt first; dealt out, a want hears none and a
    /// shard not started is no longer needed. A restarted master deals on from its record and the
    /// shards' logs: no want answered twice, the answers of the leases now told again.
    #[test]
    fn a_tests_runs_queue_deals_a_unit_per_free_slot_and_a_lost_leases_again() {
        let mut d = Dealer::new(3);
        assert!(d.needed(), "no queue known yet: every shard is needed");
        assert_eq!(d.line(0, "GATEWANT L1 1"), None, "no want before its queue");
        let mut record = Vec::new();
        let mut want = |d: &mut Dealer, shard: usize, lease: &str, n: u32| {
            let t = d.line(shard, &format!("GATEWANT {lease} {n}")).expect(lease);
            record.push(t.record);
            t.line
        };
        assert_eq!(d.line(0, "GATEQUEUE L1 4 3f2a"), None);
        assert_eq!(want(&mut d, 0, "L1", 1), "1 0 3f2a");
        assert_eq!(want(&mut d, 0, "L1", 2), "2 1 3f2a");
        d.line(1, "GATEQUEUE L2 4 3f2a");
        assert_eq!(want(&mut d, 1, "L2", 1), "1 2 3f2a");
        assert_eq!(d.line(1, "GATEWANT L2 1"), None, "a want is answered once");
        // L1 is lost before its verdict: its two units go back, dealt first, the slowest first
        d.line(0, RERUN);
        assert_eq!(want(&mut d, 1, "L2", 2), "2 0 3f2a");
        d.line(0, "GATEQUEUE L3 4 3f2a");
        assert_eq!(want(&mut d, 0, "L3", 1), "1 1 3f2a");
        assert!(d.needed());
        assert_eq!(want(&mut d, 0, "L3", 2), "2 3 3f2a");
        assert!(!d.needed(), "dealt out: a shard not started is not needed");
        assert_eq!(want(&mut d, 1, "L2", 3), "3 -");
        // a lease that said its verdict gives nothing back
        d.line(1, "NODEGATE-OK testrun 2 packages");
        d.over(1);
        assert!(!d.needed());
        assert_eq!(record, ["L1 1 0", "L1 2 1", "L2 1 2", "L2 2 0", "L3 1 1", "L3 2 3", "L2 3 -"]);
        let mut again = Dealer::new(3);
        again.replay(&record.join("\n"));
        for (shard, l) in [(0, "GATEQUEUE L1 4 3f2a"), (0, "GATEWANT L1 1"), (0, "GATEWANT L1 2"), (1, "GATEQUEUE L2 4 3f2a"), (1, "GATEWANT L2 1"), (0, RERUN), (1, "GATEWANT L2 2"), (0, "GATEQUEUE L3 4 3f2a"), (0, "GATEWANT L3 1"), (0, "GATEWANT L3 2"), (1, "GATEWANT L2 3")] {
            assert_eq!(again.line(shard, l), None, "{l}");
        }
        assert!(!again.needed(), "L1's units were dealt again before the restart: none back");
        assert_eq!(again.answers_now().iter().map(|t| t.line.as_str()).collect::<Vec<_>>(), ["1 2 3f2a", "2 0 3f2a", "3 -", "1 1 3f2a", "2 3 3f2a"]);
        assert_eq!(again.line(1, "GATEWANT L2 4").map(|t| t.line).as_deref(), Some("4 -"));
        // lost and not dealt again before the restart: back after it too
        let mut d = Dealer::new(2);
        d.replay("L5 1 0\nL5 2 1\n");
        for l in ["GATEQUEUE L5 2 aa", "GATEWANT L5 1", "GATEWANT L5 2", RERUN] {
            assert_eq!(d.line(0, l), None, "{l}");
        }
        assert!(d.needed());
        d.line(1, "GATEQUEUE L6 2 aa");
        assert_eq!(d.line(1, "GATEWANT L6 1").map(|t| t.line).as_deref(), Some("1 0 aa"));
        assert!(!d.live("L5") && d.live("L6"));
    }

    /// The dealer reads a shard's log as it grows: whole lines once, a log made again from its start.
    #[test]
    fn a_growing_log_is_read_a_whole_line_once() {
        let path = std::env::temp_dir().join(format!("devhub-grown-{}.log", std::process::id()));
        let mut seen = (0, String::new());
        assert!(grown(&path, &mut seen).is_empty(), "no log yet");
        std::fs::write(&path, "GATEQUEUE L1 3 aa\nGATEWA").unwrap();
        assert_eq!(grown(&path, &mut seen), ["GATEQUEUE L1 3 aa"]);
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"NT L1 1\nGATEWANT L1 2\n").unwrap();
        assert_eq!(grown(&path, &mut seen), ["GATEWANT L1 1", "GATEWANT L1 2"]);
        assert!(grown(&path, &mut seen).is_empty());
        std::fs::write(&path, "L2 test\n").unwrap();
        assert_eq!(grown(&path, &mut seen), ["L2 test"]);
        let _ = std::fs::remove_file(&path);
    }

    /// Shards log beside their run, named so a restarted master adopts the run, not them; the
    /// run's log is its why lines and each shard's; its status the worst of theirs.
    #[test]
    fn a_sharded_run_logs_each_shard_and_takes_the_worst_status() {
        let log = Path::new("/n/home/bucket-logs/tests-1794010109906.log");
        assert_eq!(shard_log(log, 2), Path::new("/n/home/bucket-logs/tests-s2-1794010109906.log"));
        assert_eq!(log_started(&shard_log(log, 2)), Some(1794010109906), "its lease_run reads its start");
        let ok = |n: u32| (shard_log(log, n as usize), (Ok(Some(0)), format!("GATEUNIT ok test p{n}\nNODEGATE-OK testrun 1 packages\n")));
        let failed = (shard_log(log, 3), (Ok(Some(1)), "GATEUNIT fail test p3\nNODEGATE-FAIL: test:p3".to_string()));
        let text = merged(&["why".to_string()], (2, 16), &[ok(1), failed.clone()]);
        assert!(text.starts_with("SHARDS 2 16\nwhy\n== shard 1/2: /n/home/bucket-logs/tests-s1-1794010109906.log\nGATEUNIT ok test p1\n"), "{text}");
        assert!(text.ends_with("NODEGATE-FAIL: test:p3\n"), "{text}");
        assert_eq!(worst(&[ok(1), ok(2)]), Ok(Some(0)));
        assert_eq!(worst(&[ok(1), failed.clone()]), Ok(Some(1)));
        let silent = (shard_log(log, 4), (Ok(Some(0)), "GATEUNIT ok test p4\n".to_string()));
        assert_eq!(worst(&[ok(1), silent]), Ok(None), "a shard that exited 0 without its NODEGATE-OK is no pass");
        let lost_one = (shard_log(log, 5), (Ok(Some(crate::lease::client::LOST_EXIT)), String::new()));
        assert_eq!(worst(&[failed.clone(), lost_one]), Ok(Some(crate::lease::client::LOST_EXIT)));
        let error = (shard_log(log, 6), (Err("no lease".to_string()), String::new()));
        assert_eq!(worst(&[ok(1), failed, error]), Err("no lease".to_string()));
        let unneeded = (shard_log(log, 7), (Ok(Some(0)), format!("L9 test: 1st in queue (a: busy)\n{NOT_NEEDED}\n")));
        assert_eq!(worst(&[ok(1), unneeded]), Ok(Some(0)), "a shard the queue no longer needed passes");
        let outcome = verdict(worst(&[ok(1), ok(2)]), &merged(&[], (2, 16), &[ok(1), ok(2)]), log);
        assert!(outcome.ok && outcome.failing.is_empty());
    }

    /// A run a restarted master adopts gives its verdict whoever reaped its holder: the holder's own
    /// exit (`lease --exit-to`), written as it ended, before the restart (the old image reaped it)
    /// or after it (a holder this process never started); a shard the old master stopped as not
    /// needed passes; one with neither ended by a signal, its exit not known (16:51Z, 11-08).
    #[cfg(unix)]
    #[test]
    fn an_adopted_run_takes_its_holders_own_exit_whoever_reaped_it() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-adopt-exit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tip = pin_of(&Pair::new(&"a".repeat(40), &"f".repeat(40)));
        let log = dir.join("tests-s1-5.log");
        let exit = log.with_extension("log.exit");
        let adopt = |pid: u32, text: &str| {
            std::fs::write(&log, text).unwrap();
            save_run(&log, pid, 5, &tip, &tip).unwrap();
            lease_run(&dir, &[], &[], &[], &log, (&tip, &tip), &|| true).0
        };
        // ended before the restart: the old master reaped it; nobody can now
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        let gone = ended.id();
        ended.wait().unwrap();
        std::fs::write(&exit, format!("{gone} 3\n")).unwrap();
        assert_eq!(adopt(gone, "L1 test@a: granted\n"), Ok(Some(3)), "its own exit, as it ended");
        std::fs::write(&exit, format!("{} 3\n", gone + 1)).unwrap();
        assert_eq!(adopt(gone, "L1 test@a: granted\n"), Ok(None), "another holder's exit is not its own: not known");
        std::fs::remove_file(&exit).unwrap();
        assert_eq!(adopt(gone, &format!("L1 test: 1st in queue (a: busy)\n{NOT_NEEDED}\n")), Ok(Some(0)), "stopped as not needed before the restart");
        // runs on after the restart and is not this process's child (a crash's or a switch-over's
        // master started it): its file once it ends
        let script = format!("sleep 1; echo \"$$ 7\" > '{0}.tmp' && mv '{0}.tmp' '{0}'", exit.display());
        let out = std::process::Command::new("sh").args(["-c", &format!("sh -c '{}' >/dev/null 2>&1 </dev/null & echo $!", script.replace('\'', "'\\''"))]).output().unwrap();
        let orphan: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert_eq!(adopt(orphan, "L2 test@a: granted\n"), Ok(Some(7)), "a holder it did not start: its exit when it ends");
        assert_eq!(crate::lease::client::read_exit(&exit, orphan), Some(7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A restarted master deals a run it adopts as the run was dealt (its log's SHARDS line), not
    /// over the boxes it knows just after its start; a log without one is no shape.
    #[test]
    fn an_adopted_tests_run_keeps_its_shards() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-shape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("tests-1794010109906.log");
        std::fs::write(&log, merged(&["why".to_string()], (4, 16), &[])).unwrap();
        assert_eq!(run_shape(&log), Some((4, 16)));
        std::fs::write(&log, "L1: granted\n").unwrap();
        assert_eq!(run_shape(&log), None);
        std::fs::write(&log, "SHARDS 0 16\n").unwrap();
        assert_eq!(run_shape(&log), None, "no shards is no run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `devhub bucket run` starts a bucket's run at its next look (its kick file, which the run
    /// takes); an unknown bucket is said. The full bucket's measures deal the tests bucket's next
    /// run as its own do, newest first; another bucket's do not.
    #[test]
    fn a_bucket_runs_on_demand_and_the_full_buckets_measures_deal_the_tests_runs() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-kick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kick(&dir, FULL).starts_with(r#"{"said":"bucket full: its run starts"#), "{}", kick(&dir, FULL));
        assert!(kick_file(&dir, FULL).exists());
        assert_eq!(kick(&dir, "nope"), r#"{"error":"no bucket `nope`: fast, tests, hot, ci, full"}"#);
        assert!(!kick_file(&dir, "nope").exists());
        let row = |bucket: &str, units: &[(&str, i64)]| obj(vec![("bucket", s(bucket)), ("unit_s", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        for r in [row("tests", &[("test a", 10)]), row(FULL, &[("test a", 20), ("test b", 5)]), row("fast", &[("test c", 1)])] {
            append(&dir, r).unwrap();
        }
        assert_eq!(times_env(&test_rows(&dir)), "GATE_TIMES=test a=20;test b=5", "the full run is the newest measure of `test a`");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each unit's seconds come from its run's GATETIME line, and the next run deals by the
    /// newest measure of each.
    #[test]
    fn the_tests_bucket_deals_by_each_units_last_measure() {
        let secs = unit_secs("GATETIME test a 12\ntest a: ok\nGATETIME test b 140\nGATETIME junk\n");
        assert_eq!(secs.into_iter().collect::<Vec<_>>(), [("test a".to_string(), 12), ("test b".to_string(), 140)]);
        let row = |units: &[(&str, i64)]| obj(vec![("unit_s", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        let rows = [row(&[("test a", 9)]), row(&[("test a", 12), ("test b", 140)]), obj(vec![])];
        assert_eq!(times_env(&rows), "GATE_TIMES=test a=9;test b=140", "the newest measure of each");
        assert_eq!(times_env(&[]), "GATE_TIMES=");
    }

    /// Each unit's peak RSS comes from its run's GATEPEAK line, and the next run's shards start
    /// the units they are dealt by the newest peak of each.
    #[test]
    fn the_tests_bucket_tells_its_shards_each_units_last_peak() {
        let mb = unit_mb("GATEPEAK test a --lib 2100\nGATETIME test a --lib 12\nGATEPEAK test b 30500\nGATEPEAK junk\nGATEPEAKS x 1\n");
        assert_eq!(mb.into_iter().collect::<Vec<_>>(), [("test a --lib".to_string(), 2100), ("test b".to_string(), 30500)]);
        let row = |units: &[(&str, i64)]| obj(vec![("unit_mb", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        let rows = [row(&[("test a --lib", 1800)]), row(&[("test a --lib", 2100), ("test b", 30500)])];
        assert_eq!(peaks_env(&rows), "GATE_PEAKS=test a --lib=1800;test b=30500", "the newest measure of each");
        assert_eq!(ram_floor(&rows).as_deref(), Some("29.8"), "a shard's floor: the largest unit's last peak, GB");
        assert_eq!(ram_floor(&[]), None, "none measured: no floor");
        assert_eq!(times_env(&rows), "GATE_TIMES=", "a peak is no time");
    }

    /// A unit that runs far over its measure is said while it runs: one still running past three
    /// times its measure and 10 min, not one that ended, one within its measure's reach, one
    /// measured at seconds and not yet running 10 min, or one never measured. On a box past twice
    /// its cores in load, one past five times its measure is said too, the box oversubscribed; on
    /// a box whose hub does not answer, the seconds are said as its silence; a shard's lease that
    /// ended took its units with it, and a shard that runs again on a new lease starts afresh.
    #[test]
    fn a_unit_running_far_over_its_measure_is_said_while_it_runs() {
        let measures: BTreeMap<String, u64> = [("test slow", 540), ("test done", 60), ("test near", 600), ("test quick", 5)].iter().map(|(u, s)| (u.to_string(), *s)).collect();
        let now = 10_000;
        let shard = "testrun: 4 packages\nGATEQUEUE L7 5 0123456789ab\nGATESTART test slow 5000\nGATESTART test done 5000\nGATEEND test done\nGATESTART test near 9000\nGATESTART test quick 9500\nGATESTART test new 1000\nGATESTART junk\n";
        let (lease, running, busy) = running_in(shard);
        assert_eq!(lease, Some("L7"));
        assert!(busy.is_empty());
        assert_eq!(slow_in(&running, &measures, now, false), [("test slow".to_string(), 5000, 540)]);
        assert_eq!(slow_in(&running, &measures, 5100, false), [], "not yet 10 min");
        assert_eq!(slow_in(&running, &measures, now, true), [("test quick".to_string(), 500, 5), ("test slow".to_string(), 5000, 540)], "oversubscribed: past five times its measure");
        let again = format!("{shard}LEASE LOST on box-a (L7): link lost\nGATEQUEUE L8 5 0123456789ab\nGATESTART test quick 9900\n");
        assert_eq!(running_in(&again), (Some("L8"), [("test quick", 9900)].into_iter().collect(), BTreeMap::new()), "the lost lease's units ended with it");
        let dir = std::env::temp_dir().join(format!("devhub-slow-units-{}", std::process::id()));
        let logs = dir.join("bucket-logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("tests-1794013801204.log.run"), "{}").unwrap();
        std::fs::write(logs.join("tests-s1-1794013801204.log"), shard).unwrap();
        let row = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(1)), ("unit_s", Value::Obj(measures.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect()))]);
        std::fs::write(dir.join("buckets.jsonl"), format!("{}\n", row.to_json())).unwrap();
        let at = |up: bool, load: f64| move |lease: &str| (lease == "L7").then(|| ShardBox { node: "box-a".into(), up, load, cores: 16 });
        let said = slow_units(&dir, now * 1000, &at(true, 20.0));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("tests run 1794013801204 shard 1 (L7 on box-a): `test slow` running 83 min against its measured 9 min (9x); its watch fails it HUNG"), "{said:?}");
        assert!(said[0].contains("`devhub top box-a`"), "{said:?}");
        let said = slow_units(&dir, now * 1000, &at(true, 40.0));
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].starts_with("tests run 1794013801204 shard 1 (L7 on box-a): `test quick` running 8 min against its measured 5 s (100x); box-a is oversubscribed: load 40 on 16 cores"), "{said:?}");
        let said = slow_units(&dir, now * 1000, &at(false, 40.0));
        assert_eq!(said.len(), 1, "a box that does not answer is not judged oversubscribed by its last load: {said:?}");
        assert!(said[0].ends_with("(9x); box-a does not answer: the seconds are its silence, not the unit's (its lease is lost if it stays so, and the shard runs again)"), "{said:?}");
        assert_eq!(slow_units(&dir, now * 1000, &|_| None), Vec::<String>::new(), "its lease ended");
        // a unit its watch said stalled busy is said as that with its sample, not as slow; one with a
        // new result since is slow again; one never started is nothing
        let stalled = "GATEQUEUE L9 3 0123456789ab\nGATESTART test spin 1000\nGATEBUSY test spin :: 3000 landgate.x/testrun-spin.busy the busiest thread: 7/task/8 worker R 99% wchan 0\nGATESTART test slow 5000\nGATEBUSY test slow :: 4000 landgate.x/testrun-slow.busy\nGATEBUSY test slow :: 0\nGATEBUSY test gone :: 4000 landgate.x/testrun-gone.busy\n";
        std::fs::write(logs.join("tests-s2-1794013801204.log"), stalled).unwrap();
        let both = |lease: &str| ["L7", "L9"].contains(&lease).then(|| ShardBox { node: "box-a".into(), up: true, load: 20.0, cores: 16 });
        let said = slow_units(&dir, now * 1000, &both);
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(said[1], format!("tests run 1794013801204 shard 2 (L9 on box-a): `test spin` busy, no new result for 116 min (not measured yet); nothing stops it: main or its owner decides. Its stack sample: `devhub get landq-tests tmp/keep/landgate.x/testrun-spin.busy <dir> --lease L9` now, {} once its lease ends: the busiest thread: 7/task/8 worker R 99% wchan 0", logs.join("tests-s2-1794013801204.keep/landgate.x/testrun-spin.busy").display()));
        assert!(said[2].starts_with("tests run 1794013801204 shard 2 (L9 on box-a): `test slow` running 83 min against its measured 9 min (9x)"), "{said:?}");
        std::fs::remove_file(logs.join("tests-s2-1794013801204.log")).unwrap();
        // a run with its row is over: its sidecar stays
        let ended = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(1794013801204))]);
        std::fs::write(dir.join("buckets.jsonl"), format!("{}\n{}\n", ended.to_json(), row.to_json())).unwrap();
        assert_eq!(slow_units(&dir, now * 1000, &at(true, 20.0)), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hung test unit (land_gate.sh testrun stopped it) fails with the tests it still ran, which
    /// `bucket status` says under the row, and the command that runs them alone.
    #[test]
    fn a_hung_unit_fails_with_the_tests_it_still_ran() {
        let log = "test a: ok (1 passed, 0 failed, 1 targets)\nGATEUNIT ok test a\ntest b: HUNG (its processes did nothing (under 5% of a core, no I/O) for 540 s: stopped; 3 passed, 0 failed before it)\n\
            HUNG: m::t::waits_on_eos; its stacks: testrun-b.hung in the run's kept logs\n{\"verdict\":\"blocked\",\"why\":\"main is blocked\"}\n\
            GATEREPRO test b :: makepad test b -- m::t::waits_on_eos --exact\nGATEUNIT fail test b\nNODEGATE-FAIL: test:b\n";
        let (errors, passed) = unit_lines(log);
        assert_eq!(errors["test b"], ["HUNG: m::t::waits_on_eos; its stacks: testrun-b.hung in the run's kept logs"]);
        assert!(passed.contains("test a") && !errors.contains_key("test a"));
        assert_eq!(repro_lines(log)["test b"], "makepad test b -- m::t::waits_on_eos --exact");
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
            judge("fast", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &pin_of(&tip), |_| Ok(outcome)).unwrap();
            let warns: Vec<(Sev, String, String)> = fed.lock().unwrap().drain(..).filter(|r| r.kind == "bucket" && r.sev == Sev::Warn)
                .map(|r| (r.sev, r.line, r.detail)).collect();
            warns
        };
        let first = judged(&format!("{log}NODEGATE-OK\n"));
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].1.contains("WARN new warnings in tests f: tests f: origin unknown"), "{}", first[0].1);
        assert!(first[0].2.contains("p.rs:5:11: warning: constant `MACHINE` is never used"), "{}", first[0].2);
        assert!(judged(&format!("{log}NODEGATE-OK\n")).is_empty(), "the same warning at its next look is no news");
        let again = judged(&format!("{}\n{}\nGATEREPRO tests f :: cargo check --release --tests -p f\nGATEUNIT warn tests f\nGATEUNIT ok tests f\nNODEGATE-OK\n", warn("5:11", "constant `MACHINE` is never used"), warn("9:1", "unused import: `x`")));
        assert_eq!(again.len(), 1, "{again:?}");
        assert!(again[0].2.contains("p.rs:9:1: warning: unused import") && !again[0].2.contains("MACHINE"), "only the new one: {}", again[0].2);
        // with the command that shows it alone, for the lane that owns it
        assert!(again[0].2.ends_with("\ntests f: shown alone by `cargo check --release --tests -p f`"), "{}", again[0].2);
        let rows: Vec<Value> = rows_of(&home, "fast");
        assert!(rows.iter().all(|r| r.get("ok").and_then(Value::as_bool) == Some(true)), "a warning never turns it red");
        assert_eq!(rows[0].get("warns").and_then(|w| w.get("tests f")).and_then(Value::as_str), Some(ORIGIN_UNKNOWN), "no landing in its range to name");
        // the row keeps the new one with its file and line, for `bucket status`
        let kept: Vec<&str> = rows[0].get("warn_lines").and_then(|w| w.get("tests f")).and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).collect();
        assert_eq!(kept, ["/n/tree/apps/f/tests/common/p.rs:9:1: warning: unused import: `x`"]);
        assert_eq!(rows[0].get("repro").and_then(|r| r.get("tests f")).and_then(Value::as_str), Some("cargo check --release --tests -p f"));
        assert!(rows[1].get("repro").is_some_and(|r| r.get("tests f").is_none()), "a warning it had before says no command");
        // a failing unit's, by its package (its flips aside)
        let repro = repro_lines("GATEREPRO check w --also=x :: cd libs/w && makepad check --also=x w\nGATEUNIT fail check w --also=x\nGATEREPRO test t :: makepad test t\n");
        assert_eq!(repro.get("check w").map(String::as_str), Some("cd libs/w && makepad check --also=x w"));
        assert_eq!(repro.get("test t").map(String::as_str), Some("makepad test t"));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A new warning names the landing of the range that touched its file; none touched it: the row
    /// says it is from before this range, with the commit that last touched the file, and names no
    /// landing (one that changed only the unit's other inputs was named before).
    #[test]
    fn a_new_warning_is_from_before_this_range_unless_a_landing_touched_its_file() {
        let g = |dir: &Path, args: &[&str]| -> String {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let d = std::env::temp_dir().join(format!("devhub-bucket-warn-blame-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(d.join("apps/f/tests/common")).unwrap();
        std::fs::create_dir_all(c.join("fab/tests")).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        g(&c, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "apps/commercial/\n").unwrap();
        std::fs::write(d.join("apps/f/tests/common/p.rs"), "fn model() {}\n").unwrap();
        g(&d, &["add", "-A"]);
        g(&d, &["commit", "-q", "-m", "fab: printer profiles"]);
        let profiles = g(&d, &["rev-parse", "HEAD"]);
        std::fs::write(c.join("fab/tests/q.rs"), "fn unused() {}\n").unwrap();
        g(&c, &["add", "-A"]);
        g(&c, &["commit", "-q", "-m", "fab: a test helper"]);
        let helper = g(&c, &["rev-parse", "HEAD"]);
        // the range: a landing that changed another file of the unit
        std::fs::write(d.join("libs.rs"), "\n").unwrap();
        g(&d, &["add", "-A"]);
        g(&d, &["commit", "-q", "-m", "elsewhere"]);
        let elsewhere = g(&d, &["rev-parse", "HEAD"]);
        let entry = |id: u64, pre: &str, landed: &str| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), landed: format!("work {landed} main {helper}"), pre: format!("work {pre} main {helper}"), detail: Vec::new(),
            agent: format!("L{id}"), proof: false, wire: false, asked_ms: 0, changed_ms: 0 };
        let base = Pair::new(&profiles, &helper);
        let entries = vec![entry(1, &profiles, &elsewhere)];
        let touched: Vec<Option<Vec<String>>> = entries.iter().map(|e| landing_files(&d, e)).collect();
        assert_eq!(touched, [Some(vec!["libs.rs".to_string()])]);
        let last = |f: &str| last_touch(&d, &base, f);
        // rustc's path under the node's tree, and one of the nested commercial checkout
        let line = "/n/tree/apps/f/tests/common/p.rs:1:4: warning: function `model` is never used";
        let file = location(line).map(|(path, _)| path).unwrap();
        assert_eq!(warn_blame(&[file], &entries, &touched, &last), format!("from before this range (last touched by {} fab: printer profiles)", &profiles[..10]));
        assert_eq!(warn_blame(&["apps/commercial/fab/tests/q.rs"], &entries, &touched, &last), format!("from before this range (last touched by {} fab: a test helper)", &helper[..10]));
        // a landing that touched the file is named, and only it
        std::fs::write(d.join("apps/f/tests/common/p.rs"), "fn model() {}\nfn more() {}\n").unwrap();
        g(&d, &["commit", "-q", "-am", "fab: another profile"]);
        let entries = vec![entries[0].clone(), entry(2, &elsewhere, &g(&d, &["rev-parse", "HEAD"]))];
        let touched: Vec<Option<Vec<String>>> = entries.iter().map(|e| landing_files(&d, e)).collect();
        assert_eq!(warn_blame(&[file], &entries, &touched, &last), "rc-l2 (L2) #2");
        // a landing whose diff cannot be read is not cleared
        assert_eq!(warn_blame(&[file], &entries[..1], &[None], &last), "rc-l1 (L1) #1");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A unit that fails with errors in a file is named for the landing that changed that file
    /// since the unit last passed, with that landing's commit that changed it. This holds even when
    /// the landing came before this run's range. When no landing since the pass touched the file,
    /// the unit's inputs answer for it as before.
    #[test]
    fn a_failure_is_named_for_the_landing_that_changed_its_errors_file() {
        let g = |dir: &Path, args: &[&str]| -> String {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let d = std::env::temp_dir().join(format!("devhub-bucket-file-blame-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(d.join("platform/script/std/src")).unwrap();
        std::fs::create_dir_all(d.join("apps/fab/src")).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        g(&c, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "apps/commercial/\n").unwrap();
        std::fs::write(c.join("m"), "m\n").unwrap();
        g(&c, &["add", "-A"]);
        g(&c, &["commit", "-q", "-m", "m"]);
        let m = g(&c, &["rev-parse", "HEAD"]);
        let commit = |file: &str, text: &str, msg: &str| -> String {
            std::fs::write(d.join(file), text).unwrap();
            g(&d, &["add", "-A"]);
            g(&d, &["commit", "-q", "-m", msg]);
            g(&d, &["rev-parse", "HEAD"])
        };
        commit("platform/script/std/src/vm.rs", "fn with_vm() {}\n", "script std: the vm");
        let w0 = commit("apps/fab/src/main.rs", "fn main() {}\n", "fab");
        // the break: a lane's two commits, the second changing the function the JIT cannot take
        commit("platform/script/std/src/lib.rs", "\n", "script std: a module");
        let w1 = commit("platform/script/std/src/vm.rs", "fn with_vm() {}\nfn resume_if_panicked() {}\n", "script std: a panic in a with_vm closure no longer loses the VM");
        // what landed after it, each changing the apps' inputs elsewhere
        let w2 = commit("apps/fab/src/main.rs", "fn main() { tree() }\n", "fab: trees");
        let w3 = commit("platform/script/std/src/lib.rs", "\n\n", "script std: more");
        let entry = |id: u64, pre: &str, landed: &str| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), landed: format!("work {landed} main {m}"), pre: format!("work {pre} main {m}"), detail: Vec::new(),
            agent: format!("L{id}"), proof: false, wire: false, asked_ms: 0, changed_ms: 0 };
        let history = vec![entry(1, &w0, &w1), entry(2, &w1, &w2), entry(3, &w2, &w3)];
        // the present's summary, under a node's tree, and a rustc error in a test file of its own
        let log = "app: no present (failed before its first frame: makepad-run: compile error in makepad_script_std::vm::ScriptVmCall::resume_if_panicked: platform/script/std/src/vm.rs:2:4: `resume_if_panicked` calls\terror\tcompile error)\n\
            /n/tree/apps/fab/src/main.rs:1:13: warning: unused\nGATEUNIT fail present fab\n/n/tree/apps/fab/src/main.rs:1:13: E0425: cannot find function `tree`\nGATEUNIT fail check fab\n";
        let files = error_files(log);
        assert_eq!(files["present fab"], ["platform/script/std/src/vm.rs"], "the summary's file, not the warning's");
        assert_eq!(files["check fab"], ["/n/tree/apps/fab/src/main.rs"]);
        // the present last passed before the break; this run's range holds only the landings after it
        let rows = [obj(vec![("tip", tip_value(&pin_of(&Pair::new(&w2, &m)))), ("passed", Value::Arr(vec![s("check fab")]))]),
            obj(vec![("tip", tip_value(&pin_of(&Pair::new(&w0, &m)))), ("passed", Value::Arr(vec![s("present fab")]))])];
        let tip = Pair::new(&w3, &m);
        let from = last_pass(&rows, "present fab").unwrap();
        assert_eq!(from, Pair::new(&w0, &m));
        let since = landings(&d, &from, &tip, &history).unwrap();
        assert_eq!(since.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2, 3]);
        let diffs: Vec<Option<Vec<String>>> = since.iter().map(|e| landing_files(&d, e)).collect();
        let touched: Vec<Option<&[String]>> = diffs.iter().map(Option::as_deref).collect();
        let commit_of = |e: &Entry, p: &str| entry_pair(&e.landed).and_then(|at| last_touch(&d, &at, p));
        assert_eq!(file_blame(&files["present fab"], &since, &touched, &commit_of), [(1, format!("rc-l1 (L1) #1 ({} changed vm.rs)", &w1[..10]))]);
        // the same with the box's path to the tree
        assert_eq!(file_blame(&["/n/tree/platform/script/std/src/vm.rs".to_string()], &since, &touched, &commit_of).len(), 1);
        // a landing whose diff cannot be read is not named for it
        assert!(file_blame(&files["present fab"], &since, &[None, None, None], &commit_of).is_empty());
        // the check last passed at w2: since then only #3, which did not touch main.rs (the callee changed elsewhere)
        let from = last_pass(&rows, "check fab").unwrap();
        let since = landings(&d, &from, &tip, &history).unwrap();
        let diffs: Vec<Option<Vec<String>>> = since.iter().map(|e| landing_files(&d, e)).collect();
        let touched: Vec<Option<&[String]>> = diffs.iter().map(Option::as_deref).collect();
        assert!(file_blame(&files["check fab"], &since, &touched, &commit_of).is_empty(), "the landings that changed its inputs answer for it");
        assert!(last_pass(&rows, "guard x").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A landing that changed only the gate's scripts is named for a red only when no landing of
    /// its range changed the red unit's own inputs.
    #[test]
    fn a_gate_script_landing_is_named_only_when_nothing_else_changed_the_unit() {
        assert_eq!(named_changers(&[(0, false), (1, true), (3, true)]), [1, 3]);
        assert_eq!(named_changers(&[(0, false), (2, false)]), [0, 2]);
        assert!(named_changers(&[]).is_empty());
    }

    /// A bucket's range holds the landings whose moved side its tip brought: a commercial-only
    /// landing after a work landing left work at that landing's commit, and is no landing of the
    /// range that ends there.
    #[test]
    fn a_commercial_landing_is_not_in_the_range_of_the_work_landing_before_it() {
        let g = |dir: &Path, args: &[&str]| -> String {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let d = std::env::temp_dir().join(format!("devhub-bucket-landings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        g(&c, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "apps/commercial/\n").unwrap();
        let commit = |dir: &Path, file: &str| -> String {
            std::fs::write(dir.join(file), format!("{file}\n")).unwrap();
            g(dir, &["add", "-A"]);
            g(dir, &["commit", "-q", "-m", file]);
            g(dir, &["rev-parse", "HEAD"])
        };
        let (w0, m0) = (commit(&d, "w0"), commit(&c, "m0"));
        let w1 = commit(&d, "w1");
        let m1 = commit(&c, "m1");
        let m2 = commit(&c, "m2");
        let entry = |id: u64, pre: (&str, &str), landed: (&str, &str)| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), landed: format!("work {} main {}", landed.0, landed.1), pre: format!("work {} main {}", pre.0, pre.1), detail: Vec::new(),
            agent: format!("L{id}"), proof: false, wire: false, asked_ms: 0, changed_ms: 0 };
        let entries = [entry(1, (&w0, &m0), (&w1, &m0)), entry(2, (&w1, &m0), (&w1, &m1)), entry(3, (&w1, &m1), (&w1, &m2))];
        let ids = |base: Pair, tip: Pair| landings(&d, &base, &tip, &entries).unwrap().iter().map(|e| e.id).collect::<Vec<_>>();
        // the run at the work landing: its tip has work at w1, main where it was
        assert_eq!(ids(Pair::new(&w0, &m0), Pair::new(&w1, &m0)), [1]);
        // the next run brought both commercial landings
        assert_eq!(ids(Pair::new(&w1, &m0), Pair::new(&w1, &m2)), [2, 3]);
        let _ = std::fs::remove_dir_all(&d);
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
        assert_eq!(retry_env(&[quiet.clone(), row(&["build"], &[], &[])]), "GATE_RETRY=");
        // the tests bucket's run of a package's tests is a unit of its own, red until it passes
        let tests = row(&["test t", "tests t"], &[], &[]);
        assert_eq!(retry_env(&[quiet.clone(), tests.clone()]), "GATE_RETRY=test t;tests t");
        assert_eq!(still_failing(&[row(&[], &["test t"], &[]), tests]).keys().map(String::as_str).collect::<Vec<_>>(), ["tests t"]);
        // and the hot bucket's GPU guard of a package: a unit, so its break is named, and red until it passes
        assert_eq!(retry_env(&[quiet.clone(), row(&["guard g"], &[], &[])]), "GATE_RETRY=guard g");
        // and the fast bucket's wasm32 check of a package
        assert_eq!(retry_env(&[quiet, row(&["wasm w"], &[], &[])]), "GATE_RETRY=wasm w");
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

    /// A failing unit is blamed by test: each failing test names who changed its unit in the first
    /// run where it failed after a run where it passed, or that nobody knows, naming no landing. A
    /// pass of another target of its package is no pass of the test (one package's tests, failing
    /// since before, named a devhub-only landing; another's two landings that changed only its
    /// tests' support, 11-07).
    #[test]
    fn a_failing_test_names_who_changed_its_unit_where_it_first_failed() {
        let log = "GATEUNIT ok test p --test other\ntest p --lib: FAILED (exit 101; 7 passed, 2 failed)\nFAILED live::tests::a\n  panicked at p/src/live.rs:1:2 -- no\nFAILED live::tests::b\nmakepad test: 0 warnings\ntest result: FAILED. 7 passed; 2 failed\nGATEUNIT fail test p --lib\nerror: could not compile\nGATEUNIT fail test q\n";
        let (failed, ok) = test_lines(log);
        assert_eq!(failed, BTreeMap::from([("test p --lib".to_string(), vec!["live::tests::a".to_string(), "live::tests::b".to_string()])]), "a unit that ran no test names none");
        assert_eq!(ok, BTreeSet::from(["test p --test other".to_string()]));
        let row = |log: &str, failed: &[(&str, &[&str])], ok: &[&str], changed: &[(&str, &str)]| obj(vec![
            ("failing", Value::Arr(failed.iter().map(|(u, _)| s(*u)).collect())),
            ("log", s(&format!("/h/bucket-logs/{log}"))),
            ("tests_failed", Value::Obj(failed.iter().map(|(u, t)| (u.to_string(), Value::Arr(t.iter().map(|t| s(*t)).collect()))).collect())),
            ("ran_ok", Value::Arr(ok.iter().map(|u| s(*u)).collect())),
            ("changed", Value::Obj(changed.iter().map(|(u, w)| (u.to_string(), s(*w))).collect())),
        ]);
        let unit = "test p --lib";
        let (a, b) = ("live::tests::a", "live::tests::b");
        let now = row("tests-4.log", &[(unit, &[a, b][..])], &["test p --test other"], &[(unit, "rc-late (L) #4")]);
        // b failed first in run 3, where a passed; run 2's pass of a sibling target looked at neither
        let three = row("tests-3.log", &[(unit, &[b][..])], &[], &[(unit, "rc-b (B) #3")]);
        let two = row("tests-2.log", &[], &["test p --test other"], &[]);
        let one = row("tests-1.log", &[], &[unit], &[]);
        let rows = [now.clone(), three.clone(), two, one.clone()];
        assert_eq!(test_blame(&rows, unit, b), "rc-b (B) #3");
        assert_eq!(test_blame(&rows, unit, a), "rc-late (L) #4", "a failed first now");
        assert_eq!(tests_blame(&rows, &[(unit.to_string(), vec![a.to_string(), b.to_string()])]), "by test: live::tests::a since rc-late (L) #4; live::tests::b since rc-b (B) #3");
        // a whole package's pass is a pass of each of its targets' tests
        assert_eq!(test_blame(&[three.clone(), row("tests-2.log", &[], &["test p"], &[])], unit, b), "rc-b (B) #3");
        // no pass among the runs that named their tests: nobody knows
        let old = obj(vec![("failing", Value::Arr(vec![s(unit)])), ("log", s("/h/bucket-logs/tests-0.log"))]);
        assert_eq!(test_blame(&[now.clone(), three.clone(), old], unit, b), "origin unknown: no green baseline, it failed in every run back to tests-3.log, and the log of the run before (tests-0.log is gone) is not there to read its tests from");
        assert_eq!(test_blame(&[now.clone(), three.clone()], unit, b), "origin unknown: no green baseline, it failed in every run the ledger keeps, the first tests-3.log");
        // its first failing run changed nothing of its unit: no landing is named
        let quiet = row("tests-3.log", &[(unit, &[b][..])], &[], &[]);
        assert_eq!(test_blame(&[now, quiet, one.clone()], unit, b), "origin unknown: no landing of its first failing run (tests-3.log) changed test p --lib");
        // `devhub bucket` says the same, past three tests counted
        let many = ["t0", "t1", "t2", "t3", "t4"];
        let wide = row("tests-5.log", &[(unit, &many[..])], &[], &[]);
        let said = "by test: t0, t1, t2 and 2 more origin unknown: no landing of its first failing run (tests-5.log) changed test p --lib";
        assert_eq!(still_failing(&[wide, one])["test p"].1, said);
        assert_eq!(still_since(said), said);
    }

    /// A row from before rows named their tests reads its kept log for them, and its landings'
    /// units for who changed each (none where its ambiguous run spread a unit over every landing):
    /// the first run that named its tests blamed all nine red ones `origin unknown` (11-07).
    #[test]
    fn a_row_from_before_tests_were_named_reads_its_kept_log() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-test-named-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let unit = "test p --lib";
        let log = |name: &str, text: &str| { let at = dir.join(name); std::fs::write(&at, text).unwrap(); at.display().to_string() };
        let landing = |id: i64, units: &[&str]| obj(vec![("id", Value::Int(id)), ("label", s("rc-x+rc-x")), ("agent", s("X")), ("units", Value::Arr(units.iter().map(|u| s(*u)).collect()))]);
        let old = |log: String, landings: Vec<Value>, ambiguous: bool| obj(vec![("failing", Value::Arr(vec![s(unit)])), ("log", s(&log)), ("landings", Value::Arr(landings)), ("ambiguous", Value::Bool(ambiguous))]);
        let failed = log("tests-2.log", "test p --lib: FAILED (exit 101)\nFAILED t::b\nGATEUNIT fail test p --lib\n");
        let passed = log("tests-1.log", "GATEUNIT ok test p --lib\n");
        let two = named_row(&old(failed.clone(), vec![landing(7, &[unit]), landing(8, &[])], false));
        assert_eq!(looked(&two, unit, "t::b"), Some(Look::Failed(unit.to_string())));
        assert_eq!(two.get("changed").and_then(|c| c.get(unit)).and_then(Value::as_str), Some("rc-x+rc-x (X) #7"));
        let one = named_row(&obj(vec![("log", s(&passed)), ("landings", Value::Arr(Vec::new()))]));
        assert_eq!(looked(&one, unit, "t::b"), Some(Look::Passed));
        let now = obj(vec![("log", s("/h/tests-3.log")), ("tests_failed", Value::Obj(vec![(unit.to_string(), Value::Arr(vec![s("t::b")]))])), ("ran_ok", Value::Arr(Vec::new())), ("changed", Value::Obj(Vec::new()))]);
        assert_eq!(test_blame(&[now.clone(), two, one.clone()], unit, "t::b"), "rc-x+rc-x (X) #7");
        // spread over every landing by an ambiguous run: no landing changed it
        let spread = named_row(&old(failed, vec![landing(7, &[unit]), landing(8, &[unit])], true));
        assert!(spread.get("changed").and_then(|c| c.get(unit)).is_none());
        assert_eq!(test_blame(&[now, spread, one], unit, "t::b"), "origin unknown: no landing of its first failing run (tests-2.log) changed test p --lib");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tests run's FAILED line blames a unit that names its failing tests by test, and its row
    /// keeps what the next runs read back.
    #[test]
    fn a_unit_that_names_its_failing_tests_is_blamed_by_test() {
        use crate::proto::Record;
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-bytest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fed: Arc<Mutex<Vec<Record>>> = Arc::default();
        let feed = fed.clone();
        let store = Store::open(&home, "mac", Arc::new(move |r: Record| { feed.lock().unwrap().push(r); 0 })).unwrap();
        let tip = Pair::new("a1", "b1");
        let judged = |n: u32, text: &str| {
            let log = home.join(format!("tests-{n}.log"));
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(if text.contains("NODEGATE-OK") { 0 } else { 1 })), text, &log);
            judge("tests", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &pin_of(&tip), |_| Ok(outcome)).unwrap();
            let red: Vec<String> = fed.lock().unwrap().drain(..).filter(|r| r.kind == "bucket" && r.sev == Sev::Err).map(|r| r.line).collect();
            (log.display().to_string(), red)
        };
        let (_, green) = judged(1, "GATEUNIT ok test p --lib\nNODEGATE-OK\n");
        assert!(green.is_empty(), "{green:?}");
        let (log, red) = judged(2, "test p --lib: FAILED (exit 101; 1 passed, 1 failed)\nFAILED t::a\nGATEUNIT fail test p --lib\nNODEGATE-FAIL: tests\n");
        assert_eq!(red.len(), 1, "{red:?}");
        assert!(red[0].contains(&format!("FAILED test p --lib: test p failing by test: t::a origin unknown: no landing of its first failing run (tests-2.log) changed test p --lib (log {log})")), "{}", red[0]);
        let row = rows_of(&home, "tests").into_iter().next().unwrap();
        assert_eq!(looked(&row, "test p --lib", "t::a"), Some(Look::Failed("test p --lib".to_string())));
        let _ = std::fs::remove_dir_all(&home);
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
            judge("fast", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &pin_of(&tip), |_| Ok(outcome)).unwrap();
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
        let tests = Timing::after(Duration::from_secs(60), Duration::from_secs(600));
        assert_eq!(starts_at(&tests, 0, 100, 100), 100 + min, "a minute after the last landing");
        assert_eq!(starts_at(&tests, 0, 100, 100 + 20 * min), 100 + 10 * min, "at most 10 min after the first");
        let fast = Timing::pipelined();
        assert_eq!(starts_at(&fast, 0, 100, 100 + 20 * min), 100, "at the first landing it has not checked, quiet or not");
        let every = Timing { every: Duration::from_secs(3600), quiet: Duration::ZERO, cap: Duration::ZERO, daily: None };
        assert_eq!(starts_at(&every, 1_000, 2_000, 2_000), 1_000 + h, "not before every after its last start");
        assert_eq!(log_started(Path::new("/x/bucket-logs/ci-1793673952236.log")), Some(1793673952236));
    }

    /// A steady stream of landings 30 s apart never starves the pipelined fast bucket: each run
    /// starts as the one before ends, of the newest tip, so a landing is looked at by the run
    /// after the one in flight when it landed. Waiting for a quiet minute (at most 10) left the
    /// box idle 8 min a run under such a stream, a Mac show held all the while (11-07).
    #[test]
    fn a_pipelined_bucket_runs_back_to_back_through_a_landing_stream() {
        let (sec, run) = (1_000u64, 150_000u64);
        let landed: Vec<u64> = (1..=40).map(|i| i * 30 * sec).collect();
        // the spawn loop's decision each poll (1 s), a run taking `run`: (start, end, landings looked at)
        let runs_of = |timing: &Timing| {
            let (mut last_start, mut busy_until, mut looked) = (0u64, 0u64, 0usize);
            let mut runs = Vec::new();
            for now in (0..landed[39] + 3 * run).step_by(sec as usize) {
                let tip = landed.iter().filter(|ms| **ms <= now).count();
                if now < busy_until || tip == looked {
                    continue;
                }
                let first = landed.iter().copied().filter(|ms| *ms > last_start).min().unwrap_or(now);
                if now < starts_at(timing, last_start, first, landed[tip - 1]) {
                    continue;
                }
                (last_start, busy_until, looked) = (now, now + run, tip);
                runs.push((now, now + run, tip));
            }
            runs
        };
        let runs = runs_of(&Timing::pipelined());
        assert_eq!(runs[0].0, landed[0], "the first landing starts a run at once");
        for w in runs.windows(2) {
            assert_eq!(w[1].0, w[0].1, "the next run starts as one ends");
        }
        for (i, ms) in landed.iter().enumerate() {
            let end = runs.iter().find(|r| r.2 > i).map(|r| r.1).unwrap();
            assert!(end <= ms + 2 * run, "landing {i} at {ms} is looked at by {end}");
        }
        assert_eq!(runs.last().map(|r| r.2), Some(landed.len()), "the stream's last tip");
        let quiet = runs_of(&Timing::after(Duration::from_secs(60), Duration::from_secs(600)));
        assert!(quiet.windows(2).all(|w| w[1].0 - w[0].1 >= 7 * 60 * sec), "the quiet minute never came: {quiet:?}");
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

    #[test]
    fn a_tips_state_is_the_newest_fast_run_with_what_stays_red_and_the_hot_runs_of_that_tip() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-tip-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bucket-logs")).unwrap();
        let pair = |c: char| Pair::new(&c.to_string().repeat(40), &"f".repeat(40));
        let (t0, t1, t2) = (pair('a'), pair('b'), pair('c'));
        let row = |bucket: &str, tip: &Pair, started: i64, failing: &[&str], passed: &[&str], log: &str| obj(vec![
            ("bucket", s(bucket)), ("tip", tip_value(&pin_of(tip))), ("started_ms", Value::Int(started)),
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())), ("log", s(log)),
        ]);
        append(&home, row("fast", &t0, 10, &["check pkg-a"], &[], "l0")).unwrap();
        append(&home, row("hot", &t1, 15, &["present makepad-foo"], &[], "h1")).unwrap();
        // its JIT did not build: its units did not run, and pkg-a is red still
        append(&home, row("fast", &t1, 20, &["jit"], &["check pkg-b"], "l1")).unwrap();
        append(&home, row("hot", &t2, 25, &[], &[], "h2")).unwrap();
        save_run(&home.join("bucket-logs/fast-30.log"), 7, 30, &pin_of(&t1), &pin_of(&t2)).unwrap();
        save_run(&home.join("bucket-logs/fast-5.log"), 7, 5, &pin_of(&t0), &pin_of(&t0)).unwrap();
        let at = tip_state(&home, &t1);
        assert_eq!(at.fast, Some((t1.clone(), "l1".to_string())));
        assert_eq!(at.stopped.as_deref(), Some("jit"));
        assert_eq!(at.red.keys().cloned().collect::<Vec<_>>(), vec!["check pkg-a".to_string()]);
        assert_eq!(at.hot_failing, vec!["present makepad-foo".to_string()]);
        assert_eq!(at.running, Some((30, t2.clone())), "the run going, of another tip");
        let at = tip_state(&home, &t2);
        assert_eq!((at.running, at.hot_failing.len()), (Some((30, t2.clone())), 0));
        // a sidecar older than the newest row is a finished run
        std::fs::remove_file(home.join("bucket-logs/fast-30.log.run")).unwrap();
        assert_eq!(tip_state(&home, &t0).running, None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_box_that_takes_only_proven_tips_moves_to_the_newest_tip_with_nothing_new_red() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-proven-tip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let pair = |c: char| pin_of(&Pair::new(&c.to_string().repeat(40), &"f".repeat(40)));
        let tips: Vec<Pin> = "abcdef".chars().map(pair).collect();
        let row = |tip: &Pin, failing: &[&str], passed: &[&str], log: &str| obj(vec![
            ("bucket", s("fast")), ("tip", tip_value(tip)), ("started_ms", Value::Int(1)),
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())), ("log", s(log)),
        ]);
        // a box holds the tips up to the one it is at (each tip has the ones before it)
        let at = |n: usize| {
            let held = tips[..=n].to_vec();
            move |t: &Pin| held.contains(t)
        };
        assert_eq!(proven_tip(&home, &|_: &Pin| false), Proven::Stay("no run of the fast bucket looked at a tip yet".into()));
        // a: pkg-a red already
        append(&home, row(&tips[0], &["check pkg-a"], &[], "l0")).unwrap();
        // b: its JIT did not build
        append(&home, row(&tips[1], &["jit"], &[], "l1")).unwrap();
        assert_eq!(proven_tip(&home, &|_: &Pin| false), Proven::Move(tips[0].clone()), "new to the rule: the newest run that did not stop, red and all");
        let stay = proven_tip(&home, &at(0));
        assert!(matches!(&stay, Proven::Stay(why) if why.contains("stopped at its jit step (log l1)")), "{stay:?}");
        // c: pkg-a still red, pkg-b green: taken (pkg-a was red where the box is)
        append(&home, row(&tips[2], &["check pkg-a"], &["check pkg-b"], "l2")).unwrap();
        assert_eq!(proven_tip(&home, &at(0)), Proven::Move(tips[2].clone()));
        // d: pkg-b breaks, and a test, which no show runs
        append(&home, row(&tips[3], &["check pkg-b", "tests pkg-c"], &[], "l3")).unwrap();
        assert_eq!(proven_tip(&home, &at(0)), Proven::Move(tips[2].clone()), "the newest tip with nothing new red");
        let stay = proven_tip(&home, &at(2));
        assert!(matches!(&stay, Proven::Stay(why) if why.starts_with(&format!("tip {} has check pkg-b failing", tip_short(&tips[3]))) && why.ends_with("(log l3)")), "{stay:?}");
        assert_eq!(proven_tip(&home, &at(3)), Proven::Stay("waits for the fast bucket to look at a newer tip".into()), "never back");
        // e: pkg-b fixed, pkg-a too
        append(&home, row(&tips[4], &[], &["check pkg-a", "check pkg-b"], "l4")).unwrap();
        assert_eq!(proven_tip(&home, &at(2)), Proven::Move(tips[4].clone()));
        // f: a present breaks
        append(&home, row(&tips[5], &["present makepad-foo"], &[], "l5")).unwrap();
        let stay = proven_tip(&home, &at(4));
        assert!(matches!(&stay, Proven::Stay(why) if why.contains("has present makepad-foo failing")), "{stay:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A bucket's tip pins each other nested repository it follows: its row says it, its gate
    /// diffs it from the base's commit (all of it where the base had none), and a red unit of it
    /// holds a box that takes only proven tips like any other.
    #[test]
    fn a_tip_carries_the_other_nested_repositories_and_their_red_units_hold() {
        let (a, b, f, e, e2) = ("a".repeat(40), "b".repeat(40), "f".repeat(40), "e".repeat(40), "d".repeat(40));
        let tip = pin_of(&Pair::new(&b, &f)).with("repo-b", &e);
        let row = tip_value(&tip);
        assert_eq!(read_tip(&row), Some(tip.clone()), "round-trips");
        assert_eq!(read_tip(&tip_value(&pin_of(&Pair::new(&a, &f)))), Some(pin_of(&Pair::new(&a, &f))), "a row with no other nested repository");
        assert_eq!(tip_short(&tip), format!("{},{},repo-b={}", &b[..10], &f[..10], &e[..10]));
        assert_eq!(nested_bases(&pin_of(&Pair::new(&a, &f)), &tip), "GATE_NESTED_BASES=repo-b=-", "new to the bucket: all of it");
        assert_eq!(nested_bases(&tip, &pin_of(&Pair::new(&a, &f)).with("repo-b", &e2)), format!("GATE_NESTED_BASES=repo-b={e}"));
        assert!(tip_rev(Path::new("/nonexistent-devhub-repo"), &tip).is_err_and(|e| e.contains("repo-b is on no branch")), "a repository the run cannot pin: no verdict");
        assert_eq!(tip_rev(Path::new("/nonexistent-devhub-repo"), &pin_of(&Pair::new(&a, &f))), Ok(format!("work@{a},commercial=main@{f}")));
        let home = std::env::temp_dir().join(format!("devhub-bucket-nested-tip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fast = |tip: &Pin, failing: &[&str]| obj(vec![("bucket", s("fast")), ("tip", tip_value(tip)), ("started_ms", Value::Int(1)),
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("passed", Value::Arr(Vec::new())), ("log", s("l"))]);
        let base = pin_of(&Pair::new(&a, &f)).with("repo-b", &e2);
        append(&home, fast(&base, &[])).unwrap();
        append(&home, fast(&tip, &["check repo-b-app"])).unwrap();
        let stay = proven_tip(&home, &|t: &Pin| *t == base);
        assert!(matches!(&stay, Proven::Stay(why) if why.contains(&format!("tip {} has check repo-b-app failing", tip_short(&tip)))), "{stay:?}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
