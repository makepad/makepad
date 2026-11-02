//! Workflow wall time, clipped to the requested window. Lease phases are intervals so live
//! work and leases crossing the window count too; node samples count boxes, never leases.

use super::sched::Lease;
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::BufRead;
use std::path::Path;

pub const USAGE: &str = "prof [--hour|--day] [--lane L] [--kind K]";

fn n(v: &Value, key: &str) -> u64 { v.get(key).and_then(Value::as_u64).unwrap_or(0) }
fn s<'a>(v: &'a Value, key: &str) -> &'a str { v.get(key).and_then(Value::as_str).unwrap_or("") }

#[derive(Clone, Debug)]
pub struct Span {
    pub phase: String,
    pub blocker: String,
    pub boxes: Vec<String>,
    pub start: u64,
    /// Zero means this phase is still open.
    pub end: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Phases(pub Vec<Span>);

impl Phases {
    pub fn push(&mut self, phase: &str, start: u64, end: u64) {
        self.sample(phase, "", &[], start, end);
    }

    pub fn sample(&mut self, phase: &str, blocker: &str, boxes: &[String], start: u64, end: u64) {
        if end != 0 && end <= start { return; }
        if let Some(last) = self.0.last_mut().filter(|s| s.phase == phase && s.blocker == blocker && s.boxes == boxes && s.end >= start && s.start <= start && s.end != 0) {
            last.end = if end == 0 { 0 } else { last.end.max(end) };
        } else {
            self.0.push(Span { phase: phase.into(), blocker: blocker.into(), boxes: boxes.to_vec(), start, end });
        }
    }

    pub fn begin(&mut self, phase: &str, now: u64) {
        if !self.0.iter().any(|s| s.phase == phase && s.end == 0) { self.push(phase, now, 0); }
    }

    pub fn close(&mut self, phase: &str, now: u64) {
        for s in self.0.iter_mut().filter(|s| s.phase == phase && s.end == 0) { s.end = now.max(s.start); }
    }

    pub fn parse(v: Option<&Value>) -> Self {
        Self(v.and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|v| {
            let phase = s(v, "phase");
            if phase.is_empty() { return None; }
            Some(Span { phase: phase.into(), blocker: s(v, "blocker").into(), boxes: v.get("boxes").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect(), start: n(v, "start_ms"), end: n(v, "end_ms") })
        }).collect())
    }

    pub fn json(&self) -> String {
        format!("[{}]", self.0.iter().map(|s| format!("{{\"phase\":{},\"blocker\":{},\"boxes\":[{}],\"start_ms\":{},\"end_ms\":{}}}", json_str(&s.phase), json_str(&s.blocker), s.boxes.iter().map(|b| json_str(b)).collect::<Vec<_>>().join(","), s.start, s.end)).collect::<Vec<_>>().join(","))
    }

    /// Exclusive wall time, like a CPU profile: resolution/transfer wins over a concurrent
    /// run, and running wins over fetching another run's result. Never count a millisecond
    /// of one lease twice, including multiple concurrent runs.
    fn totals(&self, since: u64, now: u64) -> BTreeMap<String, u64> {
        let names = ["queued", "jit", "sync", "run", "report", "unattributed"];
        let mut events = Vec::new();
        for s in &self.0 {
            let end = if s.end == 0 { now } else { s.end.min(now) };
            let start = s.start.max(since);
            if let Some(p) = names.iter().position(|p| *p == s.phase) {
                if end > start { events.extend([(start, p, 1i32), (end, p, -1)]); }
            }
        }
        events.sort_unstable();
        let mut active = [0i32; 6];
        let mut totals = BTreeMap::new();
        let mut last = since;
        for (at, p, delta) in events {
            if at > last {
                if let Some(p) = active.iter().position(|n| *n > 0) { *totals.entry(names[p].into()).or_default() += at - last; }
            }
            active[p] += delta;
            last = at;
        }
        totals
    }

    pub fn fields(&self, now: u64) -> String {
        let totals = self.totals(0, now);
        let mut blocked = BTreeMap::<&str, u64>::new();
        for s in self.0.iter().filter(|s| s.phase == "queued") {
            *blocked.entry(&s.blocker).or_default() += (if s.end == 0 { now } else { s.end.min(now) }).saturating_sub(s.start);
        }
        let phases = ["queued", "sync", "jit", "run", "report"].map(|p| format!("\"{p}_ms\":{}", totals.get(p).copied().unwrap_or(0))).join(",");
        format!("{phases},\"blocked\":{{{}}},\"phases\":{}", blocked.iter().map(|(k, v)| format!("{}:{v}", json_str(k))).collect::<Vec<_>>().join(","), self.json())
    }
}

/// Normalize the scheduler's reasons, not the rendered lease tag or its eventual node.
pub fn blocker(reason: &str) -> String {
    let r = reason.split_once(": ").filter(|(prefix, _)| !prefix.contains(' ') && !matches!(*prefix, "budget" | "self-update" | "class")).map_or(reason, |(_, r)| r);
    if let Some(held) = r.strip_prefix("held by ") {
        let kind = held.split_once('(').map_or(held, |(_, k)| k).split_whitespace().next().unwrap_or("lease");
        return format!("held by {kind}");
    }
    if let Some(b) = r.strip_prefix("budget:").or_else(|| r.strip_prefix("budget ")) { return format!("budget {}", b.split_whitespace().next().unwrap_or("resources")); }
    for key in ["self-update", "hold for timed", "hold for whole", "behind timed", "behind whole", "held for timed", "held for whole", "held for interactive", "node off", "dev off", "node down", "node busy", "lane cap", "class", "disk floor"] {
        if r.starts_with(key) { return key.into(); }
    }
    if r.starts_with("scrubbing") { return "teardown".into(); }
    if r.contains("estimated finish") { return "class".into(); }
    if r.contains("protocol") { return "self-update".into(); }
    if r.contains("unreachable") || r.starts_with("node ") && r.ends_with(" down") { return "node down".into(); }
    if r.contains("not heard") || r == "no node heard yet" || r == "no fitting node up" { return "node unknown".into(); }
    if r.starts_with("every node is one you cancelled") { return "cancel exclusion".into(); }
    if r.contains("without input") { return "user present".into(); }
    if r.contains("toolchain cannot link") { return "toolchain".into(); }
    if r.is_empty() { return "unobserved".into(); }
    // Future reasons still group without lease ids, numbers or a lane suffix.
    r.split(" of ").next().unwrap_or(r).split_whitespace().filter(|w| !w.chars().any(|c| c.is_ascii_digit())).collect::<Vec<_>>().join(" ")
}

pub struct Query {
    hours: u64,
    lane: Option<String>,
    kind: Option<String>,
}

impl Query {
    pub fn parse(args: &[&str]) -> Result<Self, String> {
        let mut q = Self { hours: 1, lane: None, kind: None };
        let mut window = false;
        let mut i = 0;
        while i < args.len() {
            match args[i] {
                "--hour" | "--day" if !window => { q.hours = if args[i] == "--day" { 24 } else { 1 }; window = true; }
                "--lane" | "--kind" => {
                    let key = args[i];
                    i += 1;
                    let value = *args.get(i).filter(|v| !v.starts_with('-')).ok_or(USAGE)?;
                    if key == "--lane" { q.lane = Some(value.into()); }
                    else if super::Kind::parse(value).is_some() { q.kind = Some(value.into()); }
                    else { return Err(USAGE.into()); }
                }
                _ => return Err(USAGE.into()),
            }
            i += 1;
        }
        Ok(q)
    }

    fn matches(&self, v: &Value) -> bool {
        self.lane.as_deref().is_none_or(|l| s(v, "lane") == l) && self.kind.as_deref().is_none_or(|k| s(v, "kind") == k)
    }
}

fn read(path: &Path, mut take: impl FnMut(Value)) -> Result<(), String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) { take(v); }
    }
    Ok(())
}

#[derive(Default)]
struct Block {
    ms: u64,
    count: usize,
    max: u64,
    boxes: BTreeSet<String>,
}

#[derive(Default)]
struct BoxTime { observed: u64, busy: u64, idle: u64 }

pub fn report(home: &Path, leases: &[Lease], q: &Query, now: u64, summary: bool) -> Result<String, String> {
    let since = now.saturating_sub(q.hours * 3_600_000);
    let mut records = HashMap::new();
    read(&home.join("leases.jsonl"), |v| {
        let end = n(&v, "reported_ms").max(n(&v, "ended_ms").saturating_add(n(&v, "teardown_ms")));
        if end >= since && q.matches(&v) { records.insert(s(&v, "id").to_string(), v); }
    })?;
    for l in leases {
        // The final ledger includes result fetch; an older in-memory end must not replace it.
        if records.contains_key(&l.id) { continue; }
        if let Ok(v) = makepad_strict_json::parse(l.ledger_line().as_bytes()) {
            if q.matches(&v) { records.insert(l.id.clone(), v); }
        }
    }
    // Costs are written before the box is freed. If cleanup or the master died before the
    // final ledger append, retain that measured work; a JIT-build cost without a lease is
    // already inside its lease's JIT interval and must never be added again.
    read(&home.join("costs.jsonl"), |v| {
        let id = s(&v, "lease");
        if id.is_empty() || n(&v, "ended_ms") < since || !q.matches(&v) { return; }
        if let Some(old) = records.get_mut(id) {
            if let Value::Obj(fields) = old {
                if !fields.iter().any(|(k, _)| k == "class") { fields.push(("class".into(), Value::Str(s(&v, "class").into()))); }
                if let Some(peak) = v.get("peak_ram_gb") {
                    if !fields.iter().any(|(k, _)| k == "peak_ram_gb") { fields.push(("peak_ram_gb".into(), peak.clone())); }
                }
            }
        } else if v.get("phases").is_some() {
            records.insert(id.to_string(), v);
        }
    })?;
    let mut totals = BTreeMap::<String, u64>::new();
    let mut blocks = BTreeMap::<String, Block>::new();
    let mut classes = BTreeMap::<String, (u64, usize, Option<f64>)>::new();
    let mut legacy = 0;
    let mut count = 0;
    for v in records.values() {
        let mut phases = Phases::parse(v.get("phases"));
        if v.get("phases").is_none() {
            // Old ledgers know queue/setup/teardown, but cannot identify run or JIT time.
            let end = n(v, "ended_ms");
            let grant = v.get("granted_ms").and_then(Value::as_u64).unwrap_or(end);
            phases.sample("queued", "unobserved", &[], n(v, "asked_ms"), grant);
            let setup = grant.saturating_add(n(v, "setup_ms")).min(end);
            phases.push("sync", grant, setup);
            phases.push("unattributed", setup, end);
            phases.push("report", end, end.saturating_add(n(v, "teardown_ms")));
            legacy += 1;
        }
        let times = phases.totals(since, now);
        if times.is_empty() { continue; }
        count += 1;
        for (p, ms) in &times { *totals.entry(p.clone()).or_default() += ms; }
        if let Some(ms) = times.get("run") {
            let class = v.get("req").map(|r| s(r, "class")).filter(|c| !c.is_empty()).unwrap_or_else(|| s(v, "class"));
            let class = if class.is_empty() { s(v, "kind") } else { class };
            let c = classes.entry(class.into()).or_default();
            c.0 += ms;
            c.1 += 1;
            if let Some(peak) = v.get("peak_ram_gb").and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0) { c.2 = Some(c.2.unwrap_or(0.0).max(peak)); }
        }
        let mut waits = BTreeMap::<String, (u64, BTreeSet<String>)>::new();
        for span in phases.0.iter().filter(|s| s.phase == "queued") {
            let ms = (if span.end == 0 { now } else { span.end.min(now) }).saturating_sub(span.start.max(since));
            if ms == 0 { continue; }
            let w = waits.entry(span.blocker.clone()).or_default();
            w.0 += ms;
            w.1.extend(span.boxes.iter().cloned());
        }
        for (key, (ms, boxes)) in waits {
            let b = blocks.entry(key).or_default();
            b.ms += ms;
            b.count += 1;
            b.max = b.max.max(ms);
            b.boxes.extend(boxes);
        }
    }
    let mut samples = HashMap::new();
    let mut sample = |v: Value| {
        if n(&v, "end_ms") <= since { return; }
        let key = (s(&v, "node").to_string(), n(&v, "start_ms"));
        // Appending a completed interval and replacing profile.live may be interrupted by
        // a restart. The longer copy owns that interval, never both copies.
        let old = samples.entry(key).or_insert_with(|| v.clone());
        if n(&v, "end_ms") > n(old, "end_ms") { *old = v; }
    };
    read(&home.join("profile.jsonl"), &mut sample)?;
    read(&home.join("profile.live"), &mut sample)?;
    let mut boxes = BTreeMap::<String, BoxTime>::new();
    for v in samples.values() {
        let ms = n(v, "end_ms").min(now).saturating_sub(n(v, "start_ms").max(since));
        if ms == 0 { continue; }
        let b = boxes.entry(s(v, "node").into()).or_default();
        b.observed += ms;
        let any = |key| v.get(key).and_then(Value::as_arr).unwrap_or(&[]).iter().any(|l| q.matches(l));
        if any("busy") { b.busy += ms; }
        if any("idle-while-queued") { b.idle += ms; }
    }
    let mut blockers: Vec<_> = blocks.iter().collect();
    blockers.sort_by(|a, b| b.1.ms.cmp(&a.1.ms).then_with(|| a.0.cmp(b.0)));
    let mut jobs: Vec<_> = classes.iter().collect();
    jobs.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(b.0)));
    let min = |ms: u64| ms as f64 / 60_000.0;
    let time = |p: &str| min(totals.get(p).copied().unwrap_or(0));
    let idle = min(boxes.values().map(|b| b.idle).sum());
    let top = blockers.first().map_or("none 0.0 min".into(), |(key, b)| format!("{key} {:.1} min", min(b.ms)));
    if summary { return Ok(format!("last hour: {:.1} min queued (top blocker: {top}), {:.1} min running, {idle:.1} box-min idle while queued\n", time("queued"), time("run"))); }
    let filter = format!("{}{}", q.lane.as_ref().map_or(String::new(), |l| format!(" lane={l}")), q.kind.as_ref().map_or(String::new(), |k| format!(" kind={k}")));
    let mut out = format!("last {}h{filter}: {:.1} lease-min — queued {:.1}, sync {:.1}, jit {:.1}, run {:.1}, report {:.1} ({count} leases)\n", q.hours, min(totals.values().sum()), time("queued"), time("sync"), time("jit"), time("run"), time("report"));
    out += "blockers (min; n / mean / max min; boxes):\n";
    for (key, b) in blockers.iter().take(5) {
        out += &format!("  {key}: {:.1}; {} / {:.1} / {:.1}; {}\n", min(b.ms), b.count, min(b.ms) / b.count as f64, min(b.max), if b.boxes.is_empty() { "-".into() } else { b.boxes.iter().cloned().collect::<Vec<_>>().join(",") });
    }
    if blockers.is_empty() { out += "  none\n"; }
    if blockers.len() > 5 { out += &format!("  +{} blockers: {:.1} min\n", blockers.len() - 5, min(blockers.iter().skip(5).map(|(_, b)| b.ms).sum())); }
    out += "run classes (min; leases; measured peak RAM):\n";
    for (class, (ms, n, peak)) in &jobs { out += &format!("  {class}: {:.1}; {n}; {}\n", min(*ms), peak.map_or("unknown".into(), |n| format!("{n:.1} GB"))); }
    if jobs.is_empty() { out += "  none\n"; }
    out += "boxes (busy % of observed up time; idle-while-queued min; observed min):\n";
    for (node, b) in &boxes { out += &format!("  {node}: {:.0}%; {:.1}; {:.1}\n", 100.0 * b.busy as f64 / b.observed as f64, min(b.idle), min(b.observed)); }
    if boxes.is_empty() { out += "  no samples\n"; }
    if legacy > 0 { out += &format!("legacy: {legacy} leases lack phase detail ({:.1} min unattributed)\n", time("unattributed")); }
    out += &format!("focus: {top} queued; {idle:.1} box-min idle while queued\n");
    Ok(out)
}
