//! Durable JIT telemetry. Node ledgers survive short test runs and disconnected
//! hubs; the master imports them by cursor and deduplicates by record identity.

use makepad_remote::hot::{cause_key, Outcome, Record};
use makepad_remote::json_str;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

pub struct Ledger {
    path: PathBuf,
    records: Vec<Record>,
    ids: HashSet<String>,
}

impl Ledger {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let mut this = Self { path, records: Vec::new(), ids: HashSet::new() };
        match std::fs::File::open(&this.path) {
            Ok(file) => for line in std::io::BufReader::new(file).lines() {
                let line = line.map_err(|e| e.to_string())?;
                if line.is_empty() { continue; }
                let Some(record) = Record::parse(&line) else {
                    // An interrupted append remains in the journal. Its outbox
                    // file was not acknowledged and will supply the whole record.
                    eprintln!("devhub hot: incomplete journal append retained; awaiting outbox replay");
                    continue;
                };
                if this.ids.insert(record.id.clone()) { this.records.push(record); }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.to_string()),
        }
        Ok(this)
    }

    pub fn add(&mut self, r: Record) -> Result<bool, String> {
        if self.ids.contains(&r.id) { return Ok(false); }
        if let Some(parent) = self.path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).map_err(|e| e.to_string())?;
        // The leading separator also terminates a previous interrupted append;
        // the journal never has to truncate or rewrite its history.
        writeln!(file, "\n{}", r.to_json()).and_then(|_| file.sync_data()).map_err(|e| e.to_string())?;
        self.ids.insert(r.id.clone());
        self.records.push(r);
        Ok(true)
    }
}

static LEDGER: OnceLock<Mutex<Option<Ledger>>> = OnceLock::new();

fn with<T>(f: impl FnOnce(&mut Ledger) -> Result<T, String>) -> Result<T, String> {
    let mut lock = LEDGER.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
    if lock.is_none() { *lock = Some(Ledger::open(crate::proto::home().join("hot-failures.jsonl"))?); }
    f(lock.as_mut().unwrap())
}

pub fn ingest(r: Record) -> Result<bool, String> { with(|l| l.add(r)) }

/// Reconstruct inbox delivery after a hub died between journal append and
/// publication. Inbox identities and clear tombstones make replay idempotent.
pub fn replay_failures(publish: &dyn Fn(crate::proto::Record)) -> Result<(), String> {
    let records = with(|l| Ok(l.records.iter().filter(|r| r.outcome.failure()
        || matches!(r.outcome, Outcome::RestartFailed | Outcome::FirstFrameTimeout | Outcome::NoWindow)).cloned().collect::<Vec<_>>()))?;
    for record in records { publish(crate::proto::from_hot(&record)); }
    Ok(())
}

/// The discovery worker owns this work. No filesystem or serialization is added
/// to an app's UI loop. A failed import leaves the outbox file for the next scan.
pub fn collect(node: &str, publish: &dyn Fn(crate::proto::Record)) {
    let Ok(files) = std::fs::read_dir(makepad_remote::hot::outbox()) else { return };
    for file in files.flatten() {
        let path = file.path();
        if path.extension().is_none_or(|x| x != "json") { continue; }
        let result = (|| -> Result<(), String> {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let mut r = Record::parse(&text).ok_or("invalid JIT outbox record")?;
            r.node = node.into();
            ingest(r.clone())?;
            publish(crate::proto::from_hot(&r));
            std::fs::remove_file(&path).map_err(|e| e.to_string())
        })();
        if let Err(e) = result { eprintln!("devhub hot: {e}; retaining record for retry"); }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Counts { loads: usize, failures: usize, restart_errors: usize }

fn counts(records: &[Record], since: u64, app: Option<&str>) -> Counts {
    let mut loads = HashSet::new();
    let mut failed = HashSet::new();
    let mut restart_errors = 0;
    for r in records.iter().filter(|r| r.at_ms >= since && app.is_none_or(|a| r.app == a)) {
        if r.outcome == Outcome::Verified || r.outcome.failure() { loads.insert(&r.load); }
        if r.outcome.failure() { failed.insert(&r.load); }
        if r.outcome == Outcome::RestartFailed { restart_errors += 1; }
    }
    Counts { loads: loads.len(), failures: failed.len(), restart_errors }
}

fn counts_json(c: &Counts) -> String {
    format!("{{\"loads\":{},\"failures\":{},\"rate\":{},\"restart_errors\":{}}}", c.loads, c.failures,
        if c.loads == 0 { "null".into() } else { (c.failures as f64 / c.loads as f64).to_string() }, c.restart_errors)
}

fn summary(records: &[Record], now: u64) -> String {
    let day = now.saturating_sub(24 * 60 * 60 * 1000);
    let mut causes: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    let mut apps = std::collections::BTreeSet::new();
    for r in records {
        // Startup alerts reach the inbox but do not enter the hot-load ledger's statistics.
        if matches!(r.outcome, Outcome::FirstFrameTimeout | Outcome::NoWindow) { continue; }
        apps.insert(r.app.as_str());
        if r.outcome.failure() || r.outcome == Outcome::RestartFailed {
            causes.entry(cause_key(&r.reason)).or_default().push(r);
        }
    }
    let causes = causes.into_iter().map(|(cause, rows)| {
        let newest = rows.iter().max_by_key(|r| r.at_ms).unwrap();
        let for_window = |since, app: Option<&str>| {
            let selected: Vec<_> = rows.iter().filter(|r| r.at_ms >= since && app.is_none_or(|a| r.app == a)).collect();
            let failed: HashSet<_> = selected.iter().filter(|r| r.outcome.failure()).map(|r| &r.load).collect();
            counts_json(&Counts { loads: counts(records, since, app).loads, failures: failed.len(), restart_errors: selected.iter().filter(|r| r.outcome == Outcome::RestartFailed).count() })
        };
        let by_app = apps.iter().filter(|app| rows.iter().any(|r| r.app == **app)).map(|app| {
            format!("{{\"app\":{},\"all_time\":{},\"last_24h\":{}}}", json_str(app), for_window(0, Some(app)), for_window(day, Some(app)))
        }).collect::<Vec<_>>().join(",");
        format!("{{\"cause\":{},\"count\":{},\"recent_count\":{},\"newest\":{},\"example\":{},\"all_time\":{},\"last_24h\":{},\"apps\":[{}]}}",
            json_str(&cause), rows.len(), rows.iter().filter(|r| r.at_ms >= day).count(), json_str(&newest.id), json_str(&newest.reason), for_window(0, None), for_window(day, None), by_app)
    }).collect::<Vec<_>>().join(",");
    let apps = apps.into_iter().map(|app| format!("{{\"app\":{},\"all_time\":{},\"last_24h\":{}}}", json_str(app), counts_json(&counts(records, 0, Some(app))), counts_json(&counts(records, day, Some(app))))).collect::<Vec<_>>().join(",");
    format!("{{\"all_time\":{},\"last_24h\":{},\"causes\":[{}],\"apps\":[{}]}}", counts_json(&counts(records, 0, None)), counts_json(&counts(records, day, None)), causes, apps)
}

pub fn register(ep: &makepad_remote::Endpoint) {
    ep.register("hot", 1, makepad_remote::lines::service("hot", Arc::new(|line, out| {
        let result = with(|l| {
            if let Some(after) = line.strip_prefix("records ") {
                let after: usize = after.parse().map_err(|_| "invalid hot cursor")?;
                out.extend(l.records.iter().skip(after).map(Record::to_json));
                out.push(format!("{{\"cursor\":{}}}", l.records.len()));
            } else {
                out.push(summary(&l.records, crate::proto::wall_ms()));
            }
            Ok(())
        });
        if let Err(e) = result { out.push(format!("{{\"err\":{}}}", json_str(&e))); }
    })));
}

pub fn render(json: &str, brief: bool) -> String {
    let Ok(v) = makepad_strict_json::parse(json.as_bytes()) else { return json.into() };
    let number = |v: &makepad_strict_json::Value, key| v.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    let rate = |v: &makepad_strict_json::Value| {
        let (n, d) = (number(v, "failures"), number(v, "loads"));
        format!("{n}/{d}{}", if d == 0 { String::new() } else { format!(" ({:.2}%)", n as f64 * 100.0 / d as f64) })
    };
    let Some(all) = v.get("all_time") else { return json.into() };
    let mut out = format!("hot failures: 24h {}; all time {}; warm restart errors {}\n", rate(v.get("last_24h").unwrap_or(all)), rate(all), number(all, "restart_errors"));
    if brief { return out; }
    if let Some(apps) = v.get("apps").and_then(|v| v.as_arr()) {
        for app in apps {
            out.push_str(&format!("  {}: 24h {}; all time {}\n", app.get("app").and_then(|v| v.as_str()).unwrap_or(""), rate(app.get("last_24h").unwrap()), rate(app.get("all_time").unwrap())));
        }
    }
    if let Some(causes) = v.get("causes").and_then(|v| v.as_arr()) {
        for cause in causes {
            let s = |key| cause.get(key).and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!("  {}: count={} 24h={} all={} newest={} example={}\n", s("cause"), number(cause, "count"), rate(cause.get("last_24h").unwrap()), rate(cause.get("all_time").unwrap()), s("newest"), s("example")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_counts_loads_once_and_keeps_recovery_errors_separate() {
        let path = std::env::temp_dir().join(format!("devhub-hot-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut l = Ledger::open(path.clone()).unwrap();
        let mut r = Record { id: "one".into(), load: "load-a".into(), at_ms: 1, app: "test-app".into(), node: "test-node".into(), pid: 1, outcome: Outcome::Verified, reason: String::new(), failure: String::new(), units: vec![], logs: vec![] };
        assert!(l.add(r.clone()).unwrap());
        assert!(!l.add(r.clone()).unwrap());
        r.id = "two".into(); r.outcome = Outcome::Panic; r.reason = "internal compiler error applying an edit".into();
        l.add(r.clone()).unwrap();
        r.id = "three".into(); r.outcome = Outcome::RestartFailed;
        l.add(r.clone()).unwrap();
        r.id = "four".into(); r.load = "load-b".into(); r.app = "other-app".into(); r.at_ms = 100; r.outcome = Outcome::Verified;
        l.add(r).unwrap();
        let l = Ledger::open(path.clone()).unwrap();
        assert_eq!(counts(&l.records, 0, None), Counts { loads: 2, failures: 1, restart_errors: 1 });
        assert_eq!(counts(&l.records, 2, None), Counts { loads: 1, failures: 0, restart_errors: 0 });
        assert_eq!(counts(&l.records, 0, Some("test-app")).loads, 1);
        std::fs::remove_file(path).unwrap();
    }
}
