//! Offline discrete-event scheduler simulation. Only `take` contacts the master.

use super::master::CostRecords;
use super::sched::{Action, Lease, Req, Sched, State, TESTS_LANE};
use super::{Caps, Kind, Priority};
use makepad_strict_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

const HOUR: u64 = 3_600_000;
const STEP: u64 = 10_000;
const CATEGORIES: [&str; 8] = ["gate", "tests", "timed", "live", "hub", "upkeep", "interactive", "background"];

fn category(req: &Req) -> &'static str {
    if req.kind == Kind::Gate { "gate" }
    else if req.lane == TESTS_LANE { "tests" }
    else if req.kind == Kind::Timed { "timed" }
    else if req.live { "live" }
    else if req.hub_build() { "hub" }
    else if req.upkeep() { "upkeep" }
    else if req.priority == Priority::Interactive { "interactive" }
    else { "background" }
}

fn timestamp(text: &str) -> Result<u64, String> {
    let bad = || format!("invalid UTC time `{text}`: expected YYYY-MM-DDTHH:MM:SSZ at or after 1970");
    let b = text.as_bytes();
    if b.len() != 20 || !text.is_ascii() || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' { return Err(bad()); }
    let day = makepad_civil_time::parse_iso(&text[..10]).filter(|d| *d >= 0).ok_or_else(bad)?;
    let number = |range: std::ops::Range<usize>, max: u64| -> Result<u64, String> {
        let s = &text[range];
        if !s.bytes().all(|b| b.is_ascii_digit()) { return Err(bad()); }
        s.parse::<u64>().ok().filter(|n| *n <= max).ok_or_else(bad)
    };
    Ok((day as u64 * 86_400 + number(11..13, 23)? * 3600 + number(14..16, 59)? * 60 + number(17..19, 59)?) * 1000)
}

fn window(from: &str, to: &str) -> Result<(u64, u64), String> {
    let (from, to) = (timestamp(from)?, timestamp(to)?);
    if from > to { return Err("replay: from must not be after to".into()); }
    Ok((from, to))
}

fn ms(v: &Value, key: &str) -> Option<u64> { v.get(key).and_then(Value::as_u64) }
fn string<'a>(v: &'a Value, key: &str) -> &'a str { v.get(key).and_then(Value::as_str).unwrap_or("") }
fn io(e: std::io::Error) -> String { e.to_string() }

/// Streams the ledger, retaining the original bytes (including the line ending). A whole line that
/// is no record is passed over and its number returned: the master's ledger holds 324 such lines
/// from before 11-02 (two records on one line, L3920's at 3888), and `take` refused every window
/// over them (11-11).
fn lines(path: &Path, mut visit: impl FnMut(&Value, &[u8]) -> Result<(), String>) -> Result<Vec<usize>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut number = 0;
    let mut unreadable = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(io)? == 0 { break; }
        number += 1;
        let v = match makepad_strict_json::parse(&line) {
            Ok(v) => v,
            // The master may currently be appending the final record.
            Err(_) if !line.ends_with(b"\n") => break,
            Err(_) => { unreadable.push(number); continue; }
        };
        visit(&v, &line)?;
    }
    Ok(unreadable)
}

/// [`lines`] of a replay's own input, which `take` wrote record by record: a line that is no record
/// is an error there.
fn records(path: &Path, visit: impl FnMut(&Value, &[u8]) -> Result<(), String>) -> Result<(), String> {
    match lines(path, visit)?.first() {
        Some(n) => Err(format!("{}:{n}: not a record", path.display())),
        None => Ok(()),
    }
}

fn take(dir: &Path, from: u64, to: u64) -> Result<String, String> {
    let c = super::client::master()?;
    let nodes = crate::cli::ask(&c, None, "nodes", "nodes")?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let mut report = String::new();
    for (name, field) in [("leases.jsonl", "asked_ms"), ("costs.jsonl", "ended_ms")] {
        // Do not truncate an earlier capture, or a ledger if the caller names its directory.
        let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(dir.join(name)).map_err(io)?);
        let mut count = 0;
        let unreadable = lines(&crate::proto::home().join(name), |v, raw| {
            if ms(v, field).is_some_and(|t| (from.saturating_sub(26 * HOUR)..=to).contains(&t)) {
                out.write_all(raw).map_err(io)?;
                count += 1;
            }
            Ok(())
        })?;
        out.flush().map_err(io)?;
        report.push_str(&format!("{name}: {count} lines\n"));
        if let Some(first) = unreadable.first() {
            report.push_str(&format!("{name}: {} unreadable lines passed over (the first at {first}, the last at {})\n", unreadable.len(), unreadable[unreadable.len() - 1]));
        }
    }
    let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(dir.join("nodes.jsonl")).map_err(io)?);
    for line in &nodes { writeln!(out, "{line}").map_err(io)?; }
    out.flush().map_err(io)?;
    report.push_str(&format!("nodes.jsonl: {} lines\n", nodes.len()));
    Ok(report)
}

pub fn cli(args: &[&str]) -> i32 {
    let result = match args {
        ["take", from, to, dir] => window(from, to).and_then(|(from, to)| take(Path::new(dir), from, to)),
        ["run", dir, from, to] => window(from, to).and_then(|(a, b)| simulate(Path::new(dir), a, b).map(|s| s.report(from, to))),
        _ => Err("usage: makepad devhub replay take <from> <to> <out-dir> | replay run <dir> <from> <to> (UTC YYYY-MM-DDTHH:MM:SSZ)".into()),
    };
    match result {
        Ok(text) => { print!("{text}"); 0 }
        Err(e) => { eprintln!("makepad devhub replay: {e}"); 1 }
    }
}

struct Ask {
    id: String,
    req: Req,
    asked: u64,
    /// When it entered the queue: its grant (or end) less its `queued_ms`. A leg devhub runs asks at
    /// its run's place (`asked`) and enters the queue only once its previous leg is done: entered at
    /// its place, jitlegs' timed leg queued 18 min longer in the replay than it did (L812015, 11-10).
    request: u64,
    /// Its box and granted cores on the fleet: the real idle measure's.
    node: Option<String>,
    alloc_cores: f64,
    real: Option<u64>,
    ended: u64,
    teardown: u64,
    counted: bool,
    replay: Option<String>,
    grant: Option<u64>,
    ended_replay: Option<u64>,
    cancelled: bool,
    cores: Option<f64>,
    ram: Option<f64>,
    costs: Vec<Value>,
}

impl Ask {
    fn wait(&self) -> Option<u64> {
        if !self.counted || self.cancelled || self.real.is_none() { return None; }
        self.grant.map(|t| t.saturating_sub(self.request))
    }

    fn real_wait(&self) -> Option<u64> {
        if !self.counted || self.cancelled { return None; }
        self.real.map(|t| t.saturating_sub(self.request))
    }
}

enum Event {
    Ask(usize),
    Cancel(usize),
    Release(usize),
    Clean(String, String, u64),
    Learn(Value),
}

struct Simulation {
    asks: Vec<Ask>,
    skipped: BTreeMap<String, usize>,
    lead_skipped: BTreeMap<String, usize>,
    ends: BTreeMap<String, usize>,
    overtakes: BTreeMap<String, usize>,
    head: Option<(String, u64)>,
    idle: f64,
    real_idle: f64,
    /// Still queued at the deadline: id, category, lane, its last reason.
    stuck: Vec<(String, &'static str, String, String)>,
    queued: BTreeMap<String, usize>,
    deadline: bool,
    cancelled_grants: usize,
}

fn live(l: &Lease) -> bool { matches!(l.state, State::Starting | State::Granted) }
fn count(map: &mut BTreeMap<String, usize>, key: &str) { *map.entry(key.into()).or_default() += 1; }

fn simulate(dir: &Path, from: u64, to: u64) -> Result<Simulation, String> {
    let start = from.saturating_sub(2 * HOUR);
    let limit = to + 12 * HOUR;
    let mut result = Simulation {
        asks: Vec::new(), skipped: BTreeMap::new(), lead_skipped: BTreeMap::new(), ends: BTreeMap::new(),
        overtakes: BTreeMap::new(), head: None, idle: 0.0, real_idle: 0.0, stuck: Vec::new(), queued: BTreeMap::new(), deadline: false, cancelled_grants: 0,
    };
    let mut sched = Sched::new("replay", 1);
    let mut boxes = BTreeMap::new();
    records(&dir.join("nodes.jsonl"), |v, _| {
        if let Some(mut caps) = v.get("caps").filter(|c| !c.is_null()).and_then(|c| Caps::parse(&c.to_json())) {
            let name = string(v, "node");
            if name.is_empty() { return Err("nodes record with caps has no node".into()); }
            // What the box said of the moment `take` asked (a person at it, its hub's build, a
            // deployment's apply, a pre-warm, memory stalls, its sickness): the ledger keeps none
            // of it over time, so the replay holds none. A morning replay took a box's "user present"
            // and hub build from the night's take and gave the box nothing all morning (11-11).
            caps.user = None;
            (caps.applying, caps.hub_building, caps.prewarming) = (false, false, false);
            caps.sick.clear();
            caps.stalls.clear();
            caps.trouble.clear();
            (caps.mem_pressure, caps.mem_stall) = (0, 0.0);
            (caps.gpu_busy, caps.mac_gpu) = (Vec::new(), None);
            sched.add_node(name, "", Some(true));
            boxes.insert(name.to_string(), caps);
        }
        Ok(())
    })?;
    let mut ids = BTreeSet::new();
    records(&dir.join("leases.jsonl"), |v, _| {
        let Some(asked) = ms(v, "asked_ms").filter(|t| (start..=to).contains(t)) else { return Ok(()) };
        let counted = asked >= from;
        let req = match Req::parse(&v.get("req").unwrap_or(&Value::Null).to_json()) {
            Ok(req) => req,
            Err(e) => { count(if counted { &mut result.skipped } else { &mut result.lead_skipped }, &e); return Ok(()); }
        };
        let id = string(v, "id").to_string();
        if id.is_empty() || !ids.insert(id.clone()) { return Err(format!("missing or duplicate lease id `{id}`")); }
        let ended = ms(v, "ended_ms").ok_or_else(|| format!("{id}: missing ended_ms"))?;
        let real = ms(v, "granted_ms");
        let request = ms(v, "queued_ms").and_then(|q| real.unwrap_or(ended).checked_sub(q)).map_or(asked, |r| r.max(asked));
        let alloc_cores = v.get("alloc").and_then(|a| a.get("cores")).and_then(super::num).unwrap_or(0.0);
        let node = v.get("node").and_then(Value::as_str).map(str::to_string);
        result.asks.push(Ask {
            id, req, asked, request, node, alloc_cores, real, ended, teardown: ms(v, "teardown_ms").unwrap_or(0), counted,
            replay: None, grant: None, ended_replay: None, cancelled: false, cores: None, ram: None, costs: Vec::new(),
        });
        Ok(())
    })?;
    result.asks.sort_by(|a, b| (a.asked, &a.id).cmp(&(b.asked, &b.id)));
    let ledger: BTreeMap<_, _> = result.asks.iter().enumerate().map(|(i, a)| (a.id.clone(), i)).collect();
    let mut warm = CostRecords::default();
    let mut events: BTreeMap<u64, Vec<Event>> = BTreeMap::new();
    records(&dir.join("costs.jsonl"), |v, _| {
        let Some(ended) = ms(v, "ended_ms") else { return Ok(()) };
        if (from.saturating_sub(26 * HOUR)..start).contains(&ended) { warm.push(v); }
        else if (start..=to).contains(&ended) {
            if let Some(&i) = ledger.get(string(v, "lease")) {
                let ask = &mut result.asks[i];
                // Calls and JIT builds teach the scheduler, but are not whole-lease occupancy samples.
                if !matches!(string(v, "how"), "call") && string(v, "class") != "jit-build" {
                    if let Some(wall) = v.get("wall_ms").and_then(super::num).filter(|n| n.is_finite() && *n > 0.0) {
                        ask.cores = v.get("cpu_seconds").and_then(super::num).map(|cpu| cpu * 1000.0 / wall).filter(|n| n.is_finite() && *n >= 0.0).or(ask.cores);
                    }
                    ask.ram = v.get("peak_ram_gb").and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0).or(ask.ram);
                }
                ask.costs.push(v.clone());
            } else if string(v, "lease").is_empty() {
                // Per-node JIT builds have no lease: their recorded completion is an external event.
                events.entry(ended).or_default().push(Event::Learn(v.clone()));
            }
        }
        Ok(())
    })?;
    warm.apply(&mut sched);
    for (i, ask) in result.asks.iter().enumerate() {
        events.entry(ask.request).or_default().push(Event::Ask(i));
        if ask.real.is_none() { events.entry(ask.ended.max(ask.request)).or_default().push(Event::Cancel(i)); }
    }
    let mut replay_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut scheduled = BTreeSet::new();
    let mut heads: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut now = start;
    let mut sample = start;
    loop {
        // End the previous head's interval before changing queues at this timestamp.
        for (id, since) in heads.values() {
            let a = &result.asks[replay_ids[id]];
            if a.counted { result.note_head(&a.id.clone(), now - since); }
        }
        while let Some(at) = events.remove(&now) {
            for event in at {
                match event {
                    Event::Ask(i) => {
                        let a = &mut result.asks[i];
                        let id = sched.request(a.req.clone(), now);
                        replay_ids.insert(id.clone(), i);
                        a.replay = Some(id);
                    }
                    Event::Cancel(i) => {
                        let a = &mut result.asks[i];
                        if let Some(id) = &a.replay {
                            if sched.get(id).is_some_and(|l| l.state == State::Queued) {
                                scheduled.insert(id.clone());
                                sched.cancel(id, now);
                                a.ended_replay = Some(now);
                                a.cancelled = true;
                            }
                        }
                    }
                    Event::Release(i) => {
                        let a = &mut result.asks[i];
                        if a.ended_replay.is_none() {
                            let id = a.replay.as_ref().unwrap();
                            scheduled.insert(id.clone());
                            let node = sched.get(id).and_then(|l| l.node.clone());
                            sched.release(id, "released", now);
                            if let Some(node) = node {
                                events.entry(now + a.teardown).or_default().push(Event::Clean(node, id.clone(), a.teardown));
                            }
                            a.ended_replay = Some(now);
                            let mut costs = CostRecords::default();
                            for v in &a.costs { costs.push(v); }
                            costs.apply(&mut sched);
                        }
                    }
                    Event::Clean(node, id, teardown) => sched.cleaned(&node, &id, teardown),
                    Event::Learn(v) => {
                        let mut costs = CostRecords::default();
                        costs.push(&v);
                        costs.apply(&mut sched);
                    }
                }
            }
        }
        let view = sched.view();
        if now == sample {
            for (name, base) in &boxes {
                let mut caps = base.clone();
                let mut ram = 0.0;
                caps.busy_leases.clear();
                caps.busy_outside.clear();
                for l in view.iter().filter(|l| live(l) && l.node.as_ref() == Some(name)) {
                    let a = &result.asks[replay_ids[&l.id]];
                    caps.busy_leases.push((l.id.clone(), a.cores.unwrap_or(l.cost.cores)));
                    ram += a.ram.unwrap_or(l.cost.ram_gb);
                }
                let cores: f64 = caps.busy_leases.iter().map(|(_, c)| c).sum();
                caps.cpu_pct = Some(if caps.cores == 0 { 0.0 } else { (100.0 * cores / caps.cores as f64).min(100.0) });
                caps.ram_avail_gb = (caps.ram_gb - ram).max(0.0);
                sched.set_caps(name, caps, now);
            }
        }
        // Queued asks need heartbeats too: the live master beats their holders.
        for l in &view { sched.beat(&l.id, now); }
        let mut queued: Vec<Lease> = view.into_iter().filter(|l| l.state == State::Queued).collect();
        let actions = sched.tick(now);
        let mut retick = false;
        for action in actions {
            match action {
                Action::Grant { id, node } => {
                    let i = replay_ids[&id];
                    let a = &result.asks[i];
                    if a.counted {
                        let granted = sched.get(&id).unwrap();
                        // Count a grant once, attributing it to the oldest eligible overtaken ask.
                        let overtaken = queued.iter().filter(|o| {
                            let older = &result.asks[replay_ids[&o.id]];
                            if !older.counted || older.asked >= a.asked || o.id == id { return false; }
                            let priority = sched.get(&o.id).map_or(o.priority, |l| l.priority);
                            if priority == Priority::Background && granted.priority == Priority::Interactive { return false; }
                            if priority == granted.priority && o.req.prio > granted.req.prio { return false; }
                            let c = &boxes[&node];
                            let cost = sched.cost_on(&node, o);
                            cost.cores <= c.cores_for(o.req.kind) as f64 && cost.ram_gb <= o.req.ram_budget(c)
                                && sched.candidates(o).is_ok_and(|ns| ns.contains(&&node))
                        }).min_by(|a, b| (a.asked_ms, &a.id).cmp(&(b.asked_ms, &b.id)));
                        if let Some(o) = overtaken { count(&mut result.overtakes, category(&o.req)); }
                    }
                    queued.retain(|l| l.id != id);
                    let a = &mut result.asks[i];
                    a.grant = Some(now);
                    let occupancy = match a.real {
                        Some(granted) => a.ended.saturating_sub(granted).max(1000),
                        // No measured run exists: keep it until its recorded cancellation, at least 1 s.
                        None => { result.cancelled_grants += usize::from(a.counted); a.ended.saturating_sub(now).max(1000) }
                    };
                    events.entry(now + occupancy).or_default().push(Event::Release(i));
                }
                Action::ParkLive { leases, .. } => {
                    // The master asks live sessions to release; here there is no call to drain.
                    for id in leases {
                        if sched.release(&id, "parked", now) { retick = true; }
                    }
                }
                Action::End { id, node, how } => {
                    queued.retain(|l| l.id != id);
                    let a = &mut result.asks[replay_ids[&id]];
                    if !scheduled.remove(&id) {
                        count(&mut result.ends, &how);
                        a.ended_replay = Some(now);
                        if let Some(node) = node {
                            if a.teardown == 0 { sched.cleaned(&node, &id, 0); retick = true; }
                            else { events.entry(now + a.teardown).or_default().push(Event::Clean(node, id, a.teardown)); }
                        }
                    }
                }
                _ => {}
            }
        }
        let view = sched.view();
        let current: BTreeMap<_, _> = boxes.keys().filter_map(|name| sched.first_timed(name).map(|l| {
            let since = heads.get(name).filter(|(id, _)| id == &l.id).map_or(now, |(_, since)| *since);
            (name.clone(), (l.id.clone(), since))
        })).collect();
        heads = current;
        let waiting = view.iter().any(|l| l.state == State::Queued && result.asks[replay_ids[&l.id]].counted);
        if retick { continue; }
        if now == sample {
            // (in the window only, each lease at its granted cores: as the real measure counts)
            if waiting && now < to {
                for (name, caps) in &boxes {
                    let used: f64 = view.iter().filter(|l| live(l) && l.node.as_ref() == Some(name)).map(|l| l.cost.cores).sum();
                    result.idle += (caps.cores as f64 - used).max(0.0) * STEP as f64 / 60_000.0;
                }
            }
            sample += STEP;
        }
        if result.asks.iter().filter(|a| a.counted).all(|a| a.grant.is_some() || a.cancelled) { break; }
        if now == limit {
            result.deadline = true;
            for l in view.iter().filter(|l| l.state == State::Queued && result.asks[replay_ids[&l.id]].counted) {
                count(&mut result.queued, category(&l.req));
                result.stuck.push((l.id.clone(), category(&l.req), l.req.lane.clone(), l.reason.clone()));
            }
            break;
        }
        now = events.first_key_value().map_or(sample, |(t, _)| sample.min(*t)).min(limit);
    }
    result.real_idle = real_idle(&result.asks, &boxes, from, to);
    Ok(result)
}

/// The fleet's own idle core-minutes while a counted ask waited, as the replay counts its own:
/// every [`STEP`] in the window, each box's cores less the granted cores of the leases it ran then.
fn real_idle(asks: &[Ask], boxes: &BTreeMap<String, Caps>, from: u64, to: u64) -> f64 {
    let mut idle = 0.0;
    let mut t = from;
    while t < to {
        if asks.iter().any(|a| a.counted && a.request <= t && t < a.real.unwrap_or(a.ended)) {
            for (name, caps) in boxes {
                let used: f64 = asks.iter().filter(|a| a.node.as_deref() == Some(name.as_str()) && a.real.is_some_and(|g| g <= t) && t < a.ended).map(|a| a.alloc_cores).sum();
                idle += (caps.cores as f64 - used).max(0.0) * STEP as f64 / 60_000.0;
            }
        }
        t += STEP;
    }
    idle
}

fn duration(ms: u64) -> String { format!("{}m{:02}s", ms / 60_000, ms / 1000 % 60) }
fn counts(map: &BTreeMap<String, usize>) -> String {
    if map.is_empty() { "none".into() } else { map.iter().map(|(k, n)| format!("{k}: {n}")).collect::<Vec<_>>().join(", ") }
}
fn quantiles(mut waits: Vec<u64>) -> String {
    if waits.is_empty() { return "- - - 0".into(); }
    waits.sort_unstable();
    let q = |p: usize| duration(waits[(waits.len() * p).div_ceil(100) - 1]);
    let total: u64 = waits.iter().sum();
    format!("{} {} {} {:.0}", q(50), q(95), duration(*waits.last().unwrap()), total as f64 / 60_000.0)
}

impl Simulation {
    fn note_head(&mut self, id: &str, wait: u64) {
        if self.head.as_ref().is_none_or(|(old, best)| wait > *best || wait == *best && id < old.as_str()) {
            self.head = Some((id.into(), wait));
        }
    }

    fn report(&self, from: &str, to: &str) -> String {
        let skipped: usize = self.skipped.values().sum();
        let n = self.asks.iter().filter(|a| a.counted).count() + skipped;
        let mut text = format!("replay {from}..{to}: {n} asks ({skipped} skipped: {}), {} ends not scheduled ({})\n", counts(&self.skipped), self.ends.values().sum::<usize>(), counts(&self.ends));
        text.push_str("waits from each ask's entry to its grant; total in minutes\n");
        text.push_str("category       n  replay p50 p95 max total          real p50 p95 max total\n");
        for cat in CATEGORIES {
            let asks: Vec<_> = self.asks.iter().filter(|a| a.counted && !a.cancelled && a.real.is_some() && category(&a.req) == cat).collect();
            let replay = asks.iter().filter_map(|a| a.wait()).collect();
            let real = asks.iter().filter_map(|a| a.real_wait()).collect();
            text.push_str(&format!("{cat:<12} {:>3}  {:<28} {}\n", asks.len(), quantiles(replay), quantiles(real)));
        }
        text.push_str("longest waits (replay):\n");
        let mut longest: Vec<_> = self.asks.iter().filter_map(|a| a.wait().map(|w| (a, w))).collect();
        longest.sort_by(|(a, wa), (b, wb)| wb.cmp(wa).then_with(|| a.id.cmp(&b.id)));
        for (a, w) in longest.into_iter().take(5) {
            text.push_str(&format!("  {} {} {} {} (real {})\n", a.id, category(&a.req), a.req.lane, duration(w), duration(a.real_wait().unwrap_or(0))));
        }
        let timed = |wait: fn(&Ask) -> Option<u64>| self.asks.iter().filter(|a| category(&a.req) == "timed").filter_map(|a| wait(a).map(|w| (w, a.id.as_str()))).max_by(|x, y| x.0.cmp(&y.0).then_with(|| y.1.cmp(x.1)))
            .map_or("none".to_string(), |(w, id)| format!("{id} {}", duration(w)));
        text.push_str(&format!("timed longest wait: replay {}, real {}\n", timed(Ask::wait), timed(Ask::real_wait)));
        text.push_str(&format!("timed head (replay: longest first in a box's timed queue): {}\n", self.head.as_ref().map_or("none".into(), |(id, w)| format!("{id} {}", duration(*w)))));
        text.push_str(&format!("overtakes: {} ({})\n", self.overtakes.values().sum::<usize>(), counts(&self.overtakes)));
        text.push_str(&format!("idle core-minutes while an ask waited, in the window, at granted cores: replay {:.0}, real {:.0}\n", self.idle, self.real_idle));
        if !self.lead_skipped.is_empty() { text.push_str(&format!("lead-in skipped: {}\n", counts(&self.lead_skipped))); }
        if self.cancelled_grants > 0 { text.push_str(&format!("{} never-granted real asks granted in replay: held until recorded cancellation, at least 1s; excluded from wait categories\n", self.cancelled_grants)); }
        if self.deadline {
            text.push_str(&format!("stopped at to + 12h: {} still queued ({})\n", self.queued.values().sum::<usize>(), counts(&self.queued)));
            for (id, cat, lane, reason) in self.stuck.iter().take(8) { text.push_str(&format!("  {id} {cat} {lane}: {reason}\n")); }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Role;

    /// The master's ledger has lines that are no record (two on one line): `take` passes over them
    /// and names them; a replay's own input refuses them by line; a last line still being written
    /// ends the read.
    #[test]
    fn a_ledger_line_that_is_no_record_is_passed_over_by_number() {
        let path = std::env::temp_dir().join(format!("devhub-replay-lines-{}", std::process::id()));
        std::fs::write(&path, "{\"id\":\"L1\"}\n{\"id\":\"L2\"}{\"id\":\"L3\"}\n{\"id\":\"L4\"}\n{\"id\":").unwrap();
        let mut seen = Vec::new();
        assert_eq!(lines(&path, |v, _| { seen.push(string(v, "id").to_string()); Ok(()) }), Ok(vec![2]), "the glued line by its number");
        assert_eq!(seen, ["L1", "L4"], "the records around it, and not the line being written");
        assert_eq!(records(&path, |_, _| Ok(())), Err(format!("{}:2: not a record", path.display())));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replay_occupancy_cancellation_and_determinism() {
        let dir = std::env::temp_dir().join(format!("devhub-replay-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        let from = timestamp("2026-01-02T12:00:00Z").unwrap();
        let to = from + 60_000;
        let mut nodes = String::new();
        for (name, cores) in [("box-a", 16), ("box-b", 8)] {
            let mut caps = Caps {
                node: name.into(), cores, os: "linux".into(), arch: "x86_64".into(), ram_gb: 64.0,
                ram_avail_gb: 64.0, disk_free_gb: 500.0, disk_total_gb: 1000.0, timed: true,
                role: Some(Role::Pool), native_trees: super::super::caps::NATIVE_TREES,
                features: super::super::FEATURES.iter().map(|f| f.to_string()).collect(), ..Default::default()
            };
            // (box-b was out of rotation when its state was taken: the replay holds none of that)
            if name == "box-b" { caps.sick = "git fails on its stores".into(); }
            nodes.push_str(&format!("{{\"node\":\"{name}\",\"caps\":{}}}\n", caps.to_json()));
        }
        std::fs::write(dir.join("nodes.jsonl"), nodes).unwrap();
        let mut leases = String::new();
        for i in 0..9 {
            let mut req = Req::new(&format!("lane-{i}"), if i == 2 { Kind::Timed } else { Kind::Test });
            req.node = Some(if i < 3 { "box-a" } else { "box-b" }.into());
            req.whole = i < 3;
            req.cores = Some(1.0);
            req.ram_gb = Some(1.0);
            let asked = from + if i == 2 { 1000 } else { 0 };
            let granted = if i == 2 { "null".into() } else { (from + match i { 1 => 60_000, 8 => 50_000, _ => 0 }).to_string() };
            let ended = from + if i == 2 { 2000 } else if i == 1 { 120_000 } else { 60_000 };
            // (the leg: asked at its run's place, in the queue its last 10 s before its grant)
            let queued = if i == 8 { ",\"queued_ms\":10000" } else { "" };
            leases.push_str(&format!("{{\"id\":\"L{i}\",\"asked_ms\":{asked},\"granted_ms\":{granted},\"ended_ms\":{ended}{queued},\"node\":\"{}\",\"alloc\":{{\"cores\":1}},\"req\":{}}}\n", if i < 3 { "box-a" } else { "box-b" }, req.to_json()));
        }
        std::fs::write(dir.join("leases.jsonl"), leases).unwrap();
        std::fs::write(dir.join("costs.jsonl"), format!("{{\"lease\":\"L0\",\"ended_ms\":{},\"class\":\"test\",\"kind\":\"test\",\"node\":\"box-a\",\"how\":\"released\",\"wall_ms\":60000,\"cpu_seconds\":60,\"peak_ram_gb\":1}}\n", from + 60_000)).unwrap();
        let first = simulate(&dir, from, to).unwrap();
        assert_eq!(first.asks.iter().find(|a| a.id == "L1").unwrap().wait(), Some(60_000), "whole box waits for the first occupancy");
        assert_eq!(first.asks.iter().find(|a| a.id == "L0").unwrap().cores, Some(1.0), "cost sample supplies measured use");
        let leg = first.asks.iter().find(|a| a.id == "L8").unwrap();
        // (its box runs its five other lanes' trees in turns: when it starts there is the box's)
        assert_eq!((leg.request, leg.real_wait()), (from + 40_000, Some(10_000)), "a leg enters at its grant less its queued time, its real wait counted from there");
        assert!(leg.grant.is_some_and(|g| g >= leg.request) && leg.wait() == leg.grant.map(|g| g - leg.request), "its replay wait counts from its entry too: {:?}", leg.grant);
        let cancelled = first.asks.iter().find(|a| a.id == "L2").unwrap();
        assert!(cancelled.cancelled && cancelled.wait().is_none(), "cancelled ask contributes no category wait");
        let report = first.report("2026-01-02T12:00:00Z", "2026-01-02T12:01:00Z");
        assert!(report.lines().find(|l| l.starts_with("timed ")).unwrap().split_whitespace().nth(1) == Some("0"), "cancelled ask has no category count");
        assert!(report.contains("idle core-minutes while an ask waited, in the window, at granted cores: replay "), "{report}");
        assert_eq!(report, simulate(&dir, from, to).unwrap().report("2026-01-02T12:00:00Z", "2026-01-02T12:01:00Z"), "repeat replay is byte-identical");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
