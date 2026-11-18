//! The control panel's read view (`panel` on the master's control socket, read by
//! apps/devhub_panel): every node (its caps with their temperatures and hot sensors), every lease
//! with absolute times (queued, live, and the ended ones of the asked window), and each node's
//! pressure samples. One request line, `panel <since_ms>`, one answer; read-only. The ended
//! leases come from the scheduler's memory,
//! and from the ledger only for the part of the window the scheduler no longer holds, read
//! backwards from the ledger's end (a panel that keeps asking from its last answer never reads
//! it). Pressure samples are what the node's caps said every [`super::master`] caps round, with
//! the scheduler's reservation on the box at that moment; the scheduler keeps the last
//! [`PRESSURE_KEEP_MS`] of them in memory (a restarted master reads them back), and the master's
//! history ([`super::history`]) keeps every one, forever: `history` answers any range of it
//! reduced to columns, `exact` every sample of a range ([`serve_history`]). The leases, bucket
//! runs and landings of any range come from time indexes over the master's logs ([`serve_past`]:
//! `rows`, `buckets`, `landings`).

use super::history::{read_lines, History, TimeIndex};
use super::sched::{Lease, NodeView, Sched, State};
use super::{num, Cost, Kind};
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

pub const SERVICE: &str = "panel";

/// How long the scheduler keeps each node's pressure samples in memory: the `panel` answer's
/// window (what is older is in the history).
pub const PRESSURE_KEEP_MS: u64 = 3 * 3_600_000;

/// A sample's fields as the history stores them, in [`Sample::vals`]'s order.
pub const FIELDS: [&str; 8] = ["cpu_pct", "load1", "ram_used_gb", "res_cores", "res_ram_gb", "gpu_pct", "gpu_used_gb", "disk_free_gb"];

/// The most columns one `history` ask takes (the panel asks a few thousand at most).
pub const MAX_COLS: u64 = 65_536;

/// The ledger is appended at each lease's end, so it is in end order but for leases whose
/// ending raced; its backwards read stops this far past the window's start.
const LEDGER_SLACK_MS: u64 = 10 * 60_000;

/// One node's load at one moment: what its caps measured, and what the scheduler had reserved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub t_ms: u64,
    /// all cores' CPU use in percent (negative: not measured yet)
    pub cpu_pct: f32,
    pub load1: f32,
    pub ram_used_gb: f32,
    /// cores and RAM the box's running leases are charged
    pub res_cores: f32,
    pub res_ram_gb: f32,
    /// the busiest GPU's use in percent and the VRAM used on all GPUs (negative: no GPU)
    pub gpu_pct: f32,
    pub gpu_used_gb: f32,
    /// the disk the box's work goes to: GB free
    pub disk_free_gb: f32,
}

impl Sample {
    pub fn of(t_ms: u64, c: &super::Caps, reserved: Cost) -> Sample {
        let gpu_pct = c.gpu_busy.iter().map(|(u, _)| *u as f32).fold(-1.0f32, f32::max);
        Sample {
            t_ms,
            cpu_pct: c.cpu_pct.map_or(-1.0, |p| p as f32),
            load1: c.load1 as f32,
            ram_used_gb: (c.ram_gb - c.ram_avail_gb).max(0.0) as f32,
            res_cores: reserved.cores as f32,
            res_ram_gb: reserved.ram_gb as f32,
            gpu_pct,
            gpu_used_gb: c.gpu_busy.iter().map(|(_, m)| *m as f32).sum(),
            disk_free_gb: if c.disk_total_gb > 0.0 { c.disk_free_gb as f32 } else { -1.0 },
        }
    }

    /// The fields, in [`FIELDS`]' order.
    pub fn vals(&self) -> [f32; 8] {
        [self.cpu_pct, self.load1, self.ram_used_gb, self.res_cores, self.res_ram_gb, self.gpu_pct, self.gpu_used_gb, self.disk_free_gb]
    }

    pub fn of_vals(t_ms: u64, v: &[f32]) -> Sample {
        let f = |i: usize| v.get(i).copied().unwrap_or(-1.0);
        Sample { t_ms, cpu_pct: f(0), load1: f(1), ram_used_gb: f(2), res_cores: f(3), res_ram_gb: f(4), gpu_pct: f(5), gpu_used_gb: f(6), disk_free_gb: f(7) }
    }

    fn json(&self) -> String {
        format!("[{},{:.1},{:.2},{:.1},{:.1},{:.1},{:.0},{:.1},{:.0}]", self.t_ms, self.cpu_pct, self.load1, self.ram_used_gb, self.res_cores, self.res_ram_gb, self.gpu_pct, self.gpu_used_gb, self.disk_free_gb)
    }

    fn parse(v: &Value) -> Option<Sample> {
        let a = v.as_arr()?;
        let f = |i: usize| a.get(i).and_then(num).unwrap_or(-1.0) as f32;
        Some(Sample { t_ms: a.first().and_then(num)? as u64, cpu_pct: f(1), load1: f(2), ram_used_gb: f(3), res_cores: f(4), res_ram_gb: f(5), gpu_pct: f(6), gpu_used_gb: f(7), disk_free_gb: f(8) })
    }
}

/// A stretch a lease waited in the queue, and why (the normalised blocker).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wait {
    pub start_ms: u64,
    /// 0: still waiting
    pub end_ms: u64,
    pub blocker: String,
}

/// One lease as the panel draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub id: String,
    pub lane: String,
    pub agent: String,
    pub kind: String,
    /// `queued`, `starting`, `granted` or `ended`
    pub state: String,
    /// empty while queued
    pub node: String,
    pub asked_ms: u64,
    pub granted_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    /// how it ended (`released`, `expired`, ...)
    pub how: String,
    /// why it waits (queued), or why it ended
    pub reason: String,
    /// 1-based among the waiting leases of its kind
    pub pos: u32,
    /// an exclusive request's expected wait (0: none known)
    pub eta_ms: u64,
    pub priority: String,
    /// placement class (`small`, `medium`, `large`) and what it came from
    pub cost: String,
    pub cost_source: String,
    /// the class's measured run (None: not measured)
    pub measured_ms: Option<u64>,
    pub cores: f64,
    pub ram_gb: f64,
    pub gpus: u32,
    /// takes the whole box
    pub whole: bool,
    pub src: String,
    pub class: String,
    /// what the node says it runs now
    pub running: String,
    pub disk_mb: Option<f64>,
    pub waits: Vec<Wait>,
    /// a live session: open-ended, never measured against its class
    pub live: bool,
    /// the scheduler said it runs past twice its measured run (`stalled?`)
    pub stalled: bool,
    /// the holder's last beat, for a lease that expires without one (None: ended, or watched by
    /// its holder's pid instead)
    pub beat_ms: Option<u64>,
    /// queued behind its own lane's lease and nothing else: that lease ([`Sched::own_wait`];
    /// empty: none)
    pub own_wait: String,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn ms(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(num).filter(|n| *n >= 0.0).map(|n| n as u64)
}

fn waits_of(phases: &super::prof::Phases) -> Vec<Wait> {
    phases.0.iter().filter(|p| p.phase == "queued").map(|p| Wait { start_ms: p.start, end_ms: p.end, blocker: p.blocker.clone() }).collect()
}

impl Row {
    /// A lease the scheduler holds (queued, live, or ended in its memory).
    pub fn of_lease(l: &Lease) -> Row {
        let info = makepad_strict_json::parse(l.info.as_bytes()).ok();
        let measured_ms = match l.cost_source { super::sched::CostSource::Measured(ms) | super::sched::CostSource::Scaled(ms) => Some(ms), _ => None };
        Row {
            id: l.id.clone(),
            lane: l.req.lane.clone(),
            agent: l.req.agent.clone().unwrap_or_default(),
            kind: l.req.kind.as_str().into(),
            state: l.state.as_str().into(),
            node: l.node.clone().unwrap_or_default(),
            asked_ms: l.asked_ms,
            granted_ms: l.granted_ms,
            ended_ms: l.ended_ms,
            how: match &l.state { State::Ended(h) => h.clone(), _ => String::new() },
            reason: l.reason.clone(),
            pos: l.pos as u32,
            eta_ms: l.eta_ms,
            priority: l.priority.as_str().into(),
            cost: l.cost_class.as_str().into(),
            cost_source: l.cost_source.label(),
            measured_ms,
            cores: l.cost.cores,
            ram_gb: l.cost.ram_gb,
            gpus: l.cost.gpus,
            whole: l.req.whole,
            src: l.req.src.as_str().into(),
            class: l.req.class.clone(),
            running: info.as_ref().map(|i| s(i, "running").to_string()).unwrap_or_default(),
            disk_mb: info.as_ref().and_then(|i| i.get("disk_mb")).and_then(num),
            waits: waits_of(&l.profile()),
            live: l.req.live,
            stalled: false,
            beat_ms: (l.req.holder.is_none() && l.ended_ms.is_none()).then_some(l.beat_ms),
            own_wait: String::new(),
        }
    }

    /// A ledger line (`leases.jsonl`): an ended lease the scheduler no longer holds. Its cost is
    /// the allocation the ledger kept, else what it asked, else its kind's default.
    pub fn of_ledger(v: &Value) -> Option<Row> {
        let id = s(v, "id");
        let kind = Kind::parse(s(v, "kind"))?;
        if id.is_empty() {
            return None;
        }
        let req = v.get("req");
        let r = |k: &str| req.map(|r| s(r, k)).unwrap_or("").to_string();
        let def = Cost::default_for(kind, true);
        let alloc = v.get("alloc");
        let cost = |k: &str, d: f64| alloc.and_then(|a| a.get(k)).and_then(num).or_else(|| req.and_then(|r| r.get(k)).and_then(num)).unwrap_or(d);
        Some(Row {
            id: id.into(),
            lane: s(v, "lane").into(),
            agent: r("agent"),
            kind: kind.as_str().into(),
            state: "ended".into(),
            node: s(v, "node").into(),
            asked_ms: ms(v, "asked_ms").unwrap_or(0),
            granted_ms: ms(v, "granted_ms"),
            ended_ms: ms(v, "ended_ms"),
            how: s(v, "how").into(),
            reason: String::new(),
            pos: 0,
            eta_ms: 0,
            priority: r("priority"),
            cost: s(v, "cost").into(),
            cost_source: s(v, "cost_source").into(),
            measured_ms: ms(v, "measured_ms"),
            cores: cost("cores", def.cores),
            ram_gb: cost("ram_gb", def.ram_gb),
            gpus: alloc.and_then(|a| a.get("gpus")).and_then(num).map_or(def.gpus, |g| g as u32),
            whole: req.and_then(|r| r.get("whole")).and_then(Value::as_bool).unwrap_or(false),
            src: r("src"),
            class: r("class"),
            running: String::new(),
            disk_mb: None,
            waits: waits_of(&super::prof::Phases::parse(v.get("phases"))),
            live: req.and_then(|r| r.get("live")).and_then(Value::as_bool).unwrap_or(false),
            stalled: false,
            beat_ms: None,
            own_wait: String::new(),
        })
    }

    pub fn to_json(&self) -> String {
        let opt = |o: Option<u64>| o.map_or("null".to_string(), |v| v.to_string());
        let waits: Vec<String> = self.waits.iter().map(|w| format!("[{},{},{}]", w.start_ms, w.end_ms, json_str(&w.blocker))).collect();
        format!(
            "{{\"t\":\"lease\",\"id\":{},\"lane\":{},\"agent\":{},\"kind\":{},\"state\":{},\"node\":{},\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{},\"how\":{},\"reason\":{},\"pos\":{},\"eta_ms\":{},\"priority\":{},\"cost\":{},\"cost_source\":{},\"measured_ms\":{},\"cores\":{},\"ram_gb\":{},\"gpus\":{},\"whole\":{},\"src\":{},\"class\":{},\"running\":{},\"disk_mb\":{},\"waits\":[{}],\"live\":{},\"stalled\":{},\"beat_ms\":{},\"own_wait\":{}}}",
            json_str(&self.id),
            json_str(&self.lane),
            json_str(&self.agent),
            json_str(&self.kind),
            json_str(&self.state),
            json_str(&self.node),
            self.asked_ms,
            opt(self.granted_ms),
            opt(self.ended_ms),
            json_str(&self.how),
            json_str(&self.reason),
            self.pos,
            self.eta_ms,
            json_str(&self.priority),
            json_str(&self.cost),
            json_str(&self.cost_source),
            opt(self.measured_ms),
            self.cores,
            self.ram_gb,
            self.gpus,
            self.whole,
            json_str(&self.src),
            json_str(&self.class),
            json_str(&self.running),
            self.disk_mb.map_or("null".to_string(), |d| format!("{d:.0}")),
            waits.join(","),
            self.live,
            self.stalled,
            opt(self.beat_ms),
            json_str(&self.own_wait)
        )
    }

    pub fn parse(v: &Value) -> Option<Row> {
        let st = |k: &str| s(v, k).to_string();
        let waits = v.get("waits").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|w| {
            let a = w.as_arr()?;
            Some(Wait { start_ms: a.first().and_then(num)? as u64, end_ms: a.get(1).and_then(num)? as u64, blocker: a.get(2).and_then(Value::as_str).unwrap_or("").to_string() })
        });
        Some(Row {
            id: v.get("id")?.as_str()?.to_string(),
            lane: st("lane"),
            agent: st("agent"),
            kind: st("kind"),
            state: st("state"),
            node: st("node"),
            asked_ms: ms(v, "asked_ms").unwrap_or(0),
            granted_ms: ms(v, "granted_ms"),
            ended_ms: ms(v, "ended_ms"),
            how: st("how"),
            reason: st("reason"),
            pos: ms(v, "pos").unwrap_or(0) as u32,
            eta_ms: ms(v, "eta_ms").unwrap_or(0),
            priority: st("priority"),
            cost: st("cost"),
            cost_source: st("cost_source"),
            measured_ms: ms(v, "measured_ms"),
            cores: v.get("cores").and_then(num).unwrap_or(0.0),
            ram_gb: v.get("ram_gb").and_then(num).unwrap_or(0.0),
            gpus: ms(v, "gpus").unwrap_or(0) as u32,
            whole: v.get("whole").and_then(Value::as_bool).unwrap_or(false),
            src: st("src"),
            class: st("class"),
            running: st("running"),
            disk_mb: v.get("disk_mb").and_then(num),
            waits: waits.collect(),
            live: v.get("live").and_then(Value::as_bool).unwrap_or(false),
            stalled: v.get("stalled").and_then(Value::as_bool).unwrap_or(false),
            beat_ms: ms(v, "beat_ms"),
            own_wait: st("own_wait"),
        })
    }
}

/// The whole answer to `panel <since_ms>`.
#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub now_ms: u64,
    pub since_ms: u64,
    pub nodes: Vec<NodeView>,
    pub rows: Vec<Row>,
    /// per node, its samples since `since_ms`, oldest first
    pub pressure: Vec<(String, Vec<Sample>)>,
}

impl Answer {
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("{{\"t\":\"head\",\"now_ms\":{},\"since_ms\":{}}}", self.now_ms, self.since_ms)];
        out.extend(self.nodes.iter().map(|n| format!("{{\"t\":\"node\",\"v\":{}}}", n.to_json())));
        out.extend(self.rows.iter().map(Row::to_json));
        for (node, samples) in &self.pressure {
            let s: Vec<String> = samples.iter().map(Sample::json).collect();
            out.push(format!("{{\"t\":\"pressure\",\"node\":{},\"s\":[{}]}}", json_str(node), s.join(",")));
        }
        out
    }

    pub fn parse(lines: &[String]) -> Result<Answer, String> {
        let mut a = Answer::default();
        let mut head = false;
        for l in lines {
            let v = makepad_strict_json::parse(l.as_bytes()).map_err(|_| "panel: a bad line".to_string())?;
            if let Some(e) = v.get("err").and_then(Value::as_str) {
                return Err(e.to_string());
            }
            match s(&v, "t") {
                "head" => {
                    head = true;
                    a.now_ms = ms(&v, "now_ms").unwrap_or(0);
                    a.since_ms = ms(&v, "since_ms").unwrap_or(0);
                }
                "node" => a.nodes.extend(v.get("v").and_then(NodeView::parse)),
                "lease" => a.rows.extend(Row::parse(&v)),
                "pressure" => a.pressure.push((s(&v, "node").to_string(), v.get("s").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Sample::parse).collect())),
                _ => {}
            }
        }
        if !head {
            return Err("panel: no head line".into());
        }
        Ok(a)
    }
}

/// `panel <since_ms>`: the answer's lines. The scheduler is locked only to copy its view; the
/// ledger is read after, for the part of the window its memory no longer covers.
pub fn serve(sched: &Mutex<Sched>, ledger: &Path, line: &str, now: u64, out: &mut Vec<String>) {
    let Some(since) = line.split_whitespace().next().and_then(|w| w.parse::<u64>().ok()) else {
        out.push(format!("{{\"err\":{}}}", json_str("panel <since_ms>")));
        return;
    };
    // the widest window is the pressure ring's: an older `since` would read the whole ledger
    let since = since.clamp(now.saturating_sub(PRESSURE_KEEP_MS), now);
    let (nodes, mut rows, pressure, kept_from) = {
        let s = sched.lock().unwrap();
        let row = |l: &Lease| Row { live: s.is_live_job(l), stalled: s.is_stalled(&l.id), own_wait: s.own_wait(l).unwrap_or_default(), ..Row::of_lease(l) };
        let mut rows: Vec<Row> = s.view().iter().map(row).collect();
        rows.extend(s.recent_ended(usize::MAX).iter().filter(|l| l.ended_ms.unwrap_or(0) >= since).map(row));
        // the scheduler holds the leases that ended since the master started, up to a bound:
        // what ended before that is in the ledger only
        (s.node_view(now), rows, s.pressure_since(since), s.ended_from_ms)
    };
    if since < kept_from {
        let have: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
        match ledger_rows(ledger, since, &have) {
            Ok(old) => rows.extend(old),
            Err(e) => {
                out.push(format!("{{\"err\":{}}}", json_str(&format!("panel: ledger: {e}"))));
                return;
            }
        }
    }
    out.extend(Answer { now_ms: now, since_ms: since, nodes, rows, pressure }.lines());
}

/// The ledger's leases that ended at or after `since`, read backwards from its end (the newest
/// line of a lease wins); `skip`: leases the scheduler still holds.
pub fn ledger_rows(path: &Path, since: u64, skip: &HashSet<String>) -> Result<Vec<Row>, String> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut pos = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let mut seen: HashSet<String> = skip.clone();
    let mut rows = Vec::new();
    // bytes of a line cut at the chunk's start, completed by the next (earlier) chunk
    let mut carry: Vec<u8> = Vec::new();
    const CHUNK: u64 = 256 << 10;
    'chunks: while pos > 0 {
        let take = CHUNK.min(pos);
        pos -= take;
        f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; take as usize];
        f.read_exact(&mut buf).map_err(|e| e.to_string())?;
        buf.extend_from_slice(&carry);
        // the first line of the chunk may be cut (unless the chunk starts the file)
        let start = if pos == 0 { 0 } else { buf.iter().position(|b| *b == b'\n').map_or(buf.len(), |i| i + 1) };
        carry = buf[..start].to_vec();
        for line in buf[start..].split(|b| *b == b'\n').rev() {
            if line.is_empty() {
                continue;
            }
            let Ok(v) = makepad_strict_json::parse(line) else { continue };
            let ended = ms(&v, "ended_ms").unwrap_or(0);
            if ended + LEDGER_SLACK_MS < since {
                break 'chunks;
            }
            if ended < since {
                continue;
            }
            let id = s(&v, "id");
            if seen.contains(id) {
                continue;
            }
            if let Some(r) = Row::of_ledger(&v) {
                seen.insert(r.id.clone());
                rows.push(r);
            }
        }
    }
    Ok(rows)
}

/// A field's columns in a `history` answer: each column's min, max and mean (None: no sample).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cols {
    pub min: Vec<Option<f32>>,
    pub max: Vec<Option<f32>>,
    pub mean: Vec<Option<f32>>,
}

/// The answer to `history` or `exact`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hist {
    pub from_ms: u64,
    /// `history`: the column width and count, and the level the columns were read from;
    /// `exact`: 0, the range's length in ms, 0
    pub per_ms: u64,
    pub n: u64,
    pub level: usize,
    /// the oldest sample of any node (None: an empty history)
    pub first_ms: Option<u64>,
    /// per node and field (`history`)
    pub cols: Vec<(String, String, Cols)>,
    /// per node, its samples (`exact`)
    pub exact: Vec<(String, Vec<Sample>)>,
    /// the leases that ran in the range (`rows`)
    pub rows: Vec<Row>,
}

impl Hist {
    pub fn parse(lines: &[String]) -> Result<Hist, String> {
        let mut h = Hist::default();
        let mut head = false;
        for l in lines {
            let v = makepad_strict_json::parse(l.as_bytes()).map_err(|_| "history: a bad line".to_string())?;
            if let Some(e) = v.get("err").and_then(Value::as_str) {
                return Err(e.to_string());
            }
            let arr = |k: &str| -> Vec<Option<f32>> { v.get(k).and_then(Value::as_arr).unwrap_or(&[]).iter().map(|x| num(x).map(|x| x as f32)).collect() };
            match s(&v, "t") {
                "hist" | "exact" | "rows" => {
                    head = true;
                    h.from_ms = ms(&v, "from_ms").unwrap_or(0);
                    h.per_ms = ms(&v, "per_ms").unwrap_or(0);
                    h.n = ms(&v, "n").unwrap_or(0);
                    h.level = ms(&v, "level").unwrap_or(0) as usize;
                    h.first_ms = ms(&v, "first_ms");
                }
                "cols" => h.cols.push((s(&v, "node").to_string(), s(&v, "field").to_string(), Cols { min: arr("min"), max: arr("max"), mean: arr("mean") })),
                "pressure" => h.exact.push((s(&v, "node").to_string(), v.get("s").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Sample::parse).collect())),
                "lease" => h.rows.extend(Row::parse(&v)),
                _ => {}
            }
        }
        if !head {
            return Err("history: no head line".into());
        }
        Ok(h)
    }

    /// A node's field's columns.
    pub fn field(&self, node: &str, field: &str) -> Option<&Cols> {
        self.cols.iter().find(|(n, f, _)| n == node && f == field).map(|(_, _, c)| c)
    }
}

/// A column value as the answer writes it: whole numbers bare, else two decimals.
fn col_num(v: Option<f32>) -> String {
    match v {
        None => "null".into(),
        Some(v) if v == v.round() => format!("{v:.0}"),
        Some(v) => format!("{v:.2}"),
    }
}

/// `history <from_ms> <per_ms> <n> [field,...]`: every node's samples from the master's history
/// reduced to `n` columns of `per_ms` from `from_ms`, per field asked ([`FIELDS`]; all when none
/// is named) each column's min, max and mean. `exact <from_ms> <to_ms>`: every sample of the
/// range as taken, in the `panel` answer's pressure lines. Both heads say the oldest sample.
/// The cost is the columns', not the history's ([`super::history`]).
pub fn serve_history(history: &Mutex<History>, line: &str, out: &mut Vec<String>) {
    let w: Vec<&str> = line.split_whitespace().collect();
    let arg = |i: usize| w.get(i).and_then(|x| x.parse::<u64>().ok());
    let err = |out: &mut Vec<String>, e: &str| out.push(format!("{{\"err\":{}}}", json_str(e)));
    let mut h = history.lock().unwrap();
    let names = h.names();
    let first = names.iter().filter_map(|n| h.span(n).ok().flatten()).map(|(a, _)| a).min();
    let first = first.map_or("null".to_string(), |t| t.to_string());
    match w.first().copied() {
        Some("history") => {
            let (Some(from), Some(per), Some(n)) = (arg(1), arg(2).filter(|p| *p > 0), arg(3)) else {
                return err(out, "history <from_ms> <per_ms> <n> [field,...]");
            };
            if n > MAX_COLS {
                return err(out, &format!("history: {n} columns: at most {MAX_COLS} an ask (ask in parts)"));
            }
            let mut fields = Vec::new();
            for f in w.get(4).map_or(Vec::new(), |f| f.split(',').collect()) {
                match FIELDS.iter().position(|x| *x == f) {
                    Some(i) => fields.push(i),
                    None => return err(out, &format!("history: no field `{f}` (the fields: {})", FIELDS.join(","))),
                }
            }
            if fields.is_empty() {
                fields = (0..FIELDS.len()).collect();
            }
            out.push(format!("{{\"t\":\"hist\",\"from_ms\":{from},\"per_ms\":{per},\"n\":{n},\"level\":{},\"first_ms\":{first}}}", super::history::level_for(per)));
            for node in &names {
                let cols = match h.reduced(node, from, per, n as usize) {
                    Ok((_, c)) => c,
                    Err(e) => return err(out, &format!("history: {e}")),
                };
                for &i in &fields {
                    let list = |f: &dyn Fn(&super::history::Bucket) -> Option<f32>| cols.iter().map(|c| col_num(f(c))).collect::<Vec<_>>().join(",");
                    let (min, max, mean) = (list(&|c| (c.n[i] > 0).then(|| c.min[i])), list(&|c| (c.n[i] > 0).then(|| c.max[i])), list(&|c| c.mean(i)));
                    out.push(format!("{{\"t\":\"cols\",\"node\":{},\"field\":\"{}\",\"min\":[{min}],\"max\":[{max}],\"mean\":[{mean}]}}", json_str(node), FIELDS[i]));
                }
            }
        }
        Some("exact") => {
            let (Some(from), Some(to)) = (arg(1), arg(2)) else {
                return err(out, "exact <from_ms> <to_ms>");
            };
            out.push(format!("{{\"t\":\"exact\",\"from_ms\":{from},\"per_ms\":{},\"n\":0,\"level\":0,\"first_ms\":{first}}}", to.saturating_sub(from)));
            for node in &names {
                match h.exact(node, from, to) {
                    Ok(v) => {
                        let s: Vec<String> = v.iter().map(|(t, x)| Sample::of_vals(*t, x).json()).collect();
                        out.push(format!("{{\"t\":\"pressure\",\"node\":{},\"s\":[{}]}}", json_str(node), s.join(",")));
                    }
                    Err(e) => return err(out, &format!("exact: {e}")),
                }
            }
        }
        _ => err(out, "history <from_ms> <per_ms> <n> [field,...] | exact <from_ms> <to_ms>"),
    }
}

/// `rows <from_ms> <to_ms>`: every lease of the ledger that ran in the range (asked by its end,
/// ended at or after its start), found through the ledger's time index: the panel's bars for a
/// stretch before its window. Its cost is the range's leases, not the ledger's length.
/// The master's time indexes over its logs, beside them in its home: the lease ledger's, the
/// bucket ledger's and the landing train's history's ([`TimeIndex`]).
pub struct PastIndex {
    pub leases: Mutex<TimeIndex>,
    pub buckets: Mutex<TimeIndex>,
    pub landings: Mutex<TimeIndex>,
}

impl PastIndex {
    pub fn new(home: &Path) -> PastIndex {
        PastIndex {
            leases: Mutex::new(TimeIndex::leases(home.join("leases.idx"))),
            buckets: Mutex::new(TimeIndex::buckets(home.join("buckets.idx"))),
            landings: Mutex::new(TimeIndex::landings(home.join("landings.idx"))),
        }
    }
}

/// `rows <from_ms> <to_ms>`, `buckets <from_ms> <to_ms>` and `landings <from_ms> <to_ms>`: the
/// leases, bucket runs and landings of a range, from the logs in `home` through their indexes.
/// `live` is the landing train's live view, which its history does not hold yet.
pub fn serve_past(index: &PastIndex, home: &Path, live: &dyn Fn() -> Vec<crate::land::Entry>, line: &str, out: &mut Vec<String>) {
    match line.split_whitespace().next() {
        Some("buckets") => serve_buckets(&index.buckets, &home.join("buckets.jsonl"), line, out),
        Some("landings") => serve_landings(&index.landings, &home.join("history.jsonl"), &live(), line, out),
        _ => serve_rows(&index.leases, &home.join("leases.jsonl"), line, out),
    }
}

/// A range's ask (`<word> <from_ms> <to_ms>`), its lines' byte ranges in `log` and the lines, or
/// the error line said.
fn past_lines(index: &Mutex<TimeIndex>, log: &Path, line: &str, out: &mut Vec<String>) -> Option<(u64, u64, Vec<Vec<u8>>)> {
    let w: Vec<&str> = line.split_whitespace().collect();
    let arg = |i: usize| w.get(i).and_then(|x| x.parse::<u64>().ok());
    let word = w.first().copied().unwrap_or("rows");
    let (Some(from), Some(to)) = (arg(1), arg(2)) else {
        out.push(format!("{{\"err\":{}}}", json_str(&format!("{word} <from_ms> <to_ms>"))));
        return None;
    };
    let lines = index.lock().unwrap().lines(log, from, to).and_then(|r| read_lines(log, &r));
    match lines {
        Ok(lines) => Some((from, to, lines)),
        Err(e) => {
            out.push(format!("{{\"err\":{}}}", json_str(&format!("{word}: {}: {e}", log.display()))));
            None
        }
    }
}

/// The bucket runs that ran in a range, each as its name, verdict, failing units, tip and times
/// (a row's errors and landings stay in the ledger).
pub fn serve_buckets(index: &Mutex<TimeIndex>, ledger: &Path, line: &str, out: &mut Vec<String>) {
    let Some((from, to, lines)) = past_lines(index, ledger, line, out) else { return };
    let runs: Vec<String> = lines
        .iter()
        .filter_map(|l| {
            let v = makepad_strict_json::parse_depth(l, 16).ok()?;
            let text = |v: &Value, k: &str| json_str(v.get(k).and_then(Value::as_str).unwrap_or(""));
            let failing: Vec<String> = v.get("failing").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(json_str).collect();
            let tip = v.get("tip").map_or("null".to_string(), |t| format!("{{\"work\":{},\"main\":{}}}", text(t, "work"), text(t, "main")));
            Some(format!(
                "{{\"bucket\":{},\"ok\":{},\"failing\":[{}],\"tip\":{tip},\"started_ms\":{},\"ended_ms\":{}}}",
                json_str(v.get("bucket")?.as_str()?),
                v.get("ok").and_then(Value::as_bool).unwrap_or(false),
                failing.join(","),
                v.get("started_ms").and_then(Value::as_u64)?,
                v.get("ended_ms").and_then(Value::as_u64).unwrap_or(0)
            ))
        })
        .collect();
    out.push(format!("{{\"t\":\"buckets\",\"from_ms\":{from},\"to_ms\":{to},\"n\":{}}}", runs.len()));
    out.extend(runs);
}

/// The landings handed in or changed in a range: the train's history's (an entry as it left the
/// live view) and its live view's (`live`), the live one winning; each without its detail lines
/// and package lists (an entry's tip has its own ask).
pub fn serve_landings(index: &Mutex<TimeIndex>, history: &Path, live: &[crate::land::Entry], line: &str, out: &mut Vec<String>) {
    let Some((from, to, lines)) = past_lines(index, history, line, out) else { return };
    let mut by_id: std::collections::BTreeMap<u64, crate::land::Entry> = std::collections::BTreeMap::new();
    for l in &lines {
        let Some(e) = makepad_strict_json::parse_depth(l, 16).ok().and_then(|v| crate::land::Entry::parse(&v.get("record")?.to_json())) else { continue };
        by_id.insert(e.id, e);
    }
    for e in live.iter().filter(|e| e.changed_ms >= from && e.asked_ms <= to) {
        by_id.insert(e.id, e.clone());
    }
    out.push(format!("{{\"t\":\"landings\",\"from_ms\":{from},\"to_ms\":{to},\"n\":{}}}", by_id.len()));
    out.extend(by_id.into_values().map(|mut e| {
        (e.detail, e.touched, e.wide, e.set) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        e.to_json()
    }));
}

pub fn serve_rows(index: &Mutex<TimeIndex>, ledger: &Path, line: &str, out: &mut Vec<String>) {
    let Some((from, to, lines)) = past_lines(index, ledger, line, out) else { return };
    // a lease's newest line wins
    let mut rows: Vec<Row> = Vec::new();
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in &lines {
        let Some(r) = makepad_strict_json::parse(line).ok().as_ref().and_then(Row::of_ledger) else { continue };
        match at.get(&r.id) {
            Some(&i) => rows[i] = r,
            None => {
                at.insert(r.id.clone(), rows.len());
                rows.push(r);
            }
        }
    }
    out.push(format!("{{\"t\":\"rows\",\"from_ms\":{from},\"per_ms\":{},\"n\":{},\"level\":0,\"first_ms\":null}}", to.saturating_sub(from), rows.len()));
    out.extend(rows.iter().map(Row::to_json));
}

/// The samples of the last `keep_ms` before `now` from the history, per node: what a restarted
/// master's scheduler holds again.
pub fn recent(history: &mut History, now: u64, keep_ms: u64) -> Vec<(String, Vec<Sample>)> {
    history.names().into_iter().filter_map(|n| Some((n.clone(), history.exact(&n, now.saturating_sub(keep_ms), now.saturating_add(1)).ok()?.iter().map(|(t, v)| Sample::of_vals(*t, v)).collect()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::sched::Req;
    use crate::lease::{Caps, Role};

    #[test]
    fn node_temperatures_and_hot_sensors_reach_the_panel() {
        let mut s = Sched::new("master", 1);
        s.add_node("box-a", "192.0.2.1", None);
        let mut c = caps("box-a");
        c.temps = vec![crate::lease::Temp { sensor: "gpu0".into(), c: 86.0, limit: Some(85.0) }];
        c.hot = vec![crate::lease::HotTemp { sensor: "gpu0".into(), c: 86.0, limit: 85.0 }];
        s.set_caps("box-a", c.clone(), 1);
        let a = Answer { now_ms: 1, since_ms: 0, nodes: s.node_view(1), ..Default::default() };
        let read = Answer::parse(&a.lines()).unwrap();
        let got = read.nodes[0].caps.as_ref().unwrap();
        assert_eq!((&got.temps, &got.hot), (&c.temps, &c.hot), "the node view carries readings and alerts through the panel wire");
        assert!(read.nodes[0].caps_line().contains("HOT gpu0 86C (limit 85)"), "the panel's node description names the hot sensor");
    }

    fn caps(name: &str) -> Caps {
        Caps {
            node: name.into(),
            features: crate::lease::FEATURES.iter().map(|f| f.to_string()).collect(),
            native_trees: crate::lease::caps::NATIVE_TREES,
            arch: "x86_64".into(),
            os: "linux".into(),
            cores: 16,
            ram_gb: 64.0,
            ram_avail_gb: 40.0,
            disk_free_gb: 500.0,
            disk_total_gb: 1000.0,
            disk_floor_gb: 25.0,
            cpu_pct: Some(42.0),
            gpu_busy: vec![(30.0, 2.0)],
            gpus: vec![("GPU-A".into(), 24.0)],
            role: Some(Role::Pool),
            timed: true,
            ..Default::default()
        }
    }

    /// The answer the master sends is the answer the panel reads: nodes, live and queued leases
    /// with absolute times and their queue waits, the ledger's older leases, pressure samples.
    #[test]
    fn the_panel_reads_what_the_master_answers() {
        let dir = std::env::temp_dir().join(format!("devhub-panel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = dir.join("leases.jsonl");
        // an older lease only the ledger still has, twice (its later line wins), and one before the
        // window (its times 9 s and 8 s before its end: no end before 9 s, where u64 would wrap)
        let line = |id: &str, ended: u64, how: &str| format!("{{\"id\":\"{id}\",\"lane\":\"lane-a\",\"kind\":\"build\",\"node\":\"node-b\",\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{ended},\"how\":\"{how}\",\"cost\":\"large\",\"cost_source\":\"kind\",\"measured_ms\":null,\"alloc\":{{\"cores\":6,\"ram_gb\":10,\"gpus\":0}},\"phases\":[{{\"phase\":\"queued\",\"blocker\":\"budget cores\",\"boxes\":[],\"start_ms\":{},\"end_ms\":{}}}],\"req\":{{\"agent\":\"AGENT-A\",\"class\":\"build cargo\"}}}}\n", ended - 9000, ended - 8000, ended - 9000, ended - 8000);
        std::fs::write(&ledger, [line("L1", 10_000, "released"), line("L2", 50_000, "expired"), line("L2", 51_000, "released")].concat()).unwrap();
        let sched = Mutex::new(Sched::new("mac", 1));
        {
            let mut s = sched.lock().unwrap();
            // the master started after the ledger's leases ended
            s.ended_from_ms = 55_000;
            for n in ["node-a", "node-b"] {
                s.add_node(n, "192.0.2.1", None);
                s.set_caps(n, caps(n), 60_000);
            }
            let mut r = Req::new("lane-b", Kind::Test);
            r.agent = Some("AGENT-B".into());
            r.node = Some("node-a".into());
            let id = s.request(r, 60_000);
            s.tick(61_000);
            s.started(&id, "{}");
            s.set_caps("node-a", caps("node-a"), 70_000);
        }
        let mut out = Vec::new();
        serve(&sched, &ledger, "20000", 80_000, &mut out);
        let a = Answer::parse(&out).unwrap();
        assert_eq!((a.now_ms, a.since_ms), (80_000, 20_000), "head");
        assert_eq!(a.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["node-a", "node-b"], "every node");
        assert_eq!(a.nodes[0].caps.as_ref().map(|c| c.cores), Some(16), "a node's caps");
        let live = a.rows.iter().find(|r| r.lane == "lane-b").expect("the live lease");
        assert_eq!((live.agent.as_str(), live.asked_ms, live.granted_ms.is_some()), ("AGENT-B", 60_000, true), "live lease times");
        assert!(!live.live && !live.stalled, "a plain test lease: no live session, not stalled");
        assert!(live.beat_ms.is_some(), "a lease that expires without beats says its last one");
        let flagged = Row { live: true, stalled: true, beat_ms: None, ..live.clone() };
        assert_eq!(Row::parse(&makepad_strict_json::parse(flagged.to_json().as_bytes()).unwrap()).as_ref(), Some(&flagged), "the flags travel");
        let old: Vec<&Row> = a.rows.iter().filter(|r| r.lane == "lane-a").collect();
        assert_eq!(old.len(), 1, "one row per ledger lease in the window: {old:?}");
        assert_eq!((old[0].how.as_str(), old[0].cores, old[0].waits.len()), ("released", 6.0, 1), "the ledger's newest line, its allocation and its wait");
        assert_eq!(old[0].waits[0].blocker, "budget cores");
        let samples = &a.pressure.iter().find(|(n, _)| n == "node-a").unwrap().1;
        assert_eq!(samples.len(), 2, "a sample per caps round");
        assert_eq!((samples[1].cpu_pct, samples[1].ram_used_gb, samples[1].gpu_pct), (42.0, 24.0, 30.0), "measured load");
        assert!(samples[1].res_cores > 0.0, "the running lease's reservation: {:?}", samples[1]);
        // asking again from the last answer reads no ledger (a ledger that cannot be read would
        // answer an error): the old lease is not sent again
        let mut again = Vec::new();
        serve(&sched, &dir, "79000", 81_000, &mut again);
        assert!(again.iter().all(|l| !l.contains("\"err\"")), "no ledger read: {again:?}");
        let b = Answer::parse(&again).unwrap();
        assert!(b.rows.iter().all(|r| r.lane != "lane-a"), "nothing of the ledger past the asked time");
        assert!(b.pressure.iter().all(|(_, s)| s.is_empty()), "no sample since then");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `rows` answers the ledger's leases that ran in a range before the panel's window, through
    /// the ledger's time index: those inside it and one that began in it and ended a day later
    /// (a live session), none outside it; the lines appended since the last ask too; and a lost
    /// index is built again with the same answer.
    #[test]
    fn rows_of_a_past_range_come_from_the_ledgers_index() {
        let dir = std::env::temp_dir().join(format!("devhub-panel-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (ledger, idx) = (dir.join("leases.jsonl"), dir.join("leases.idx"));
        let (t0, min) = (1_800_000_000_000u64, 60_000u64);
        let line = |id: &str, asked: u64, ended: u64| format!("{{\"id\":\"{id}\",\"lane\":\"lane-a\",\"kind\":\"build\",\"node\":\"node-a\",\"asked_ms\":{asked},\"granted_ms\":{asked},\"ended_ms\":{ended},\"how\":\"released\"}}\n");
        // a day of leases, one every 10 min for 5 min, then one asked at 3 h that ended at 30 h
        let mut text: String = (0..144).map(|i| line(&format!("L{i}"), t0 + i * 10 * min, t0 + (i * 10 + 5) * min)).collect();
        text += &line("LONG", t0 + 180 * min, t0 + 1800 * min);
        std::fs::write(&ledger, &text).unwrap();
        let ask = |index: &Mutex<TimeIndex>| {
            let mut out = Vec::new();
            serve_rows(index, &ledger, &format!("rows {} {}", t0 + 120 * min, t0 + 240 * min), &mut out);
            let mut ids: Vec<String> = Hist::parse(&out).unwrap().rows.into_iter().map(|r| r.id).collect();
            ids.sort();
            ids
        };
        let mut want: Vec<String> = (12..=24).map(|i| format!("L{i}")).chain(["LONG".to_string()]).collect();
        want.sort();
        let index = Mutex::new(TimeIndex::leases(idx.clone()));
        assert_eq!(ask(&index), want, "the leases that ran in 2 h to 4 h, the long one among them");
        assert_eq!(std::fs::metadata(&idx).unwrap().len(), 16 + 145 * 32, "a record a ledger line");
        // a lease that ended after the last ask, asked in the range
        std::io::Write::write_all(&mut std::fs::OpenOptions::new().append(true).open(&ledger).unwrap(), line("LATE", t0 + 200 * min, t0 + 3000 * min).as_bytes()).unwrap();
        want.push("LATE".into());
        want.sort();
        assert_eq!(ask(&index), want, "the lines appended since are indexed at the next ask");
        // a lost index: built again from the ledger
        drop(index);
        std::fs::remove_file(&idx).unwrap();
        assert_eq!(ask(&Mutex::new(TimeIndex::leases(idx.clone()))), want, "a rebuilt index answers the same");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `buckets` and `landings` answer a range's bucket runs and landings through their indexes:
    /// the bucket ledger's rows in it (a full run that began before it among them), and the land
    /// section's entries of the state history, its other sections' lines passed over, the train's
    /// live view winning over the history for an entry both hold (11-17).
    #[test]
    fn buckets_and_landings_of_a_range_come_from_their_indexes() {
        let dir = std::env::temp_dir().join(format!("devhub-panel-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (t0, min) = (1_800_000_000_000u64, 60_000u64);
        let run = |name: &str, s: u64, e: u64, ok: bool| format!("{{\"bucket\":\"{name}\",\"base\":null,\"tip\":{{\"work\":\"a1\",\"main\":\"b1\"}},\"started_ms\":{},\"ended_ms\":{},\"ok\":{ok},\"failing\":[{}],\"errors\":{{}},\"landings\":[{{\"id\":1,\"land_to_verdict_ms\":5}}]}}\n", t0 + s * min, t0 + e * min, if ok { "" } else { "\"test p\"" });
        let text: String = (0..30).map(|i| run("fast", i * 10, i * 10 + 5, i != 13)).chain([run("full", 100, 300, false)]).collect();
        std::fs::write(dir.join("buckets.jsonl"), text).unwrap();
        let land = |id: u64, asked: u64, changed: u64, state: &str| format!("{{\"revision\":{id},\"section\":\"land\",\"record\":{{\"id\":{id},\"branch\":\"rc-l{id}\",\"state\":\"{state}\",\"asked_ms\":{},\"changed_ms\":{},\"detail\":[\"a long check log\"],\"touched\":[\"pkg\"]}}}}\n", t0 + asked * min, t0 + changed * min);
        let other = |rev: u64| format!("{{\"revision\":{rev},\"section\":\"lives\",\"record\":{{\"id\":\"W{rev}\",\"asked_ms\":{},\"changed_ms\":{}}}}}\n", t0 + 140 * min, t0 + 150 * min);
        let text: String = (0..30).map(|i| land(i, i * 10, i * 10 + 3, if i == 14 { "failed" } else { "landed" }) + &other(1000 + i)).collect();
        std::fs::write(dir.join("history.jsonl"), text).unwrap();
        let index = PastIndex::new(&dir);
        let live = || {
            let mut e = crate::land::Entry::parse(&format!("{{\"id\":14,\"branch\":\"rc-l14\",\"state\":\"landed\",\"asked_ms\":{},\"changed_ms\":{}}}", t0 + 140 * min, t0 + 149 * min)).unwrap();
            e.agent = "LIVE".into();
            vec![e]
        };
        let ask = |line: &str| {
            let mut out = Vec::new();
            serve_past(&index, &dir, &live, line, &mut out);
            out
        };
        let (from, to) = (t0 + 120 * min, t0 + 160 * min);
        let runs = ask(&format!("buckets {from} {to}"));
        let names: Vec<(String, u64)> = runs[1..].iter().map(|l| makepad_strict_json::parse(l.as_bytes()).unwrap()).map(|v| (v.get("bucket").and_then(Value::as_str).unwrap().to_string(), (v.get("started_ms").and_then(Value::as_u64).unwrap() - t0) / min)).collect();
        assert_eq!(names, [("fast", 120), ("fast", 130), ("fast", 140), ("fast", 150), ("fast", 160), ("full", 100)].map(|(n, s)| (n.to_string(), s)), "the runs that met 2 h to 2 h 40, the full run begun before it among them");
        assert!(runs[1..].iter().all(|l| !l.contains("errors") && !l.contains("landings")) && runs[2].contains("\"failing\":[\"test p\"]"), "a run's name, verdict, failing units, tip and times only: {runs:?}");
        let got = ask(&format!("landings {from} {to}"));
        let entries: Vec<crate::land::Entry> = got[1..].iter().map(|l| crate::land::Entry::parse(l).unwrap()).collect();
        assert_eq!(entries.iter().map(|e| e.id).collect::<Vec<_>>(), [12, 13, 14, 15, 16], "the land section's entries in the range; the other sections' lines passed over");
        let live_one = entries.iter().find(|e| e.id == 14).unwrap();
        assert_eq!((live_one.agent.as_str(), live_one.state.as_str()), ("LIVE", "landed"), "the live view wins over the history");
        assert!(entries.iter().all(|e| e.detail.is_empty() && e.touched.is_empty()), "no detail lines or package lists");
        assert!(ask("landings soon").iter().any(|l| l.contains("landings <from_ms> <to_ms>")), "a bad ask says its form");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `panel 0` answers the last three hours, not the whole ledger.
    #[test]
    fn an_old_since_answers_three_hours() {
        let dir = std::env::temp_dir().join(format!("devhub-panel-window-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = dir.join("leases.jsonl");
        let now = 10 * 3_600_000;
        let line = |id: &str, ended: u64| format!("{{\"id\":\"{id}\",\"lane\":\"lane-a\",\"kind\":\"test\",\"node\":\"node-a\",\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{ended},\"how\":\"released\"}}\n", ended - 2000, ended - 1000);
        std::fs::write(&ledger, [line("L1", now - 4 * 3_600_000), line("L2", now - 3_600_000)].concat()).unwrap();
        let sched = Mutex::new(Sched::new("mac", 1));
        sched.lock().unwrap().ended_from_ms = now - 10_000;
        let mut out = Vec::new();
        serve(&sched, &ledger, "0", now, &mut out);
        let a = Answer::parse(&out).unwrap();
        assert_eq!(a.since_ms, now - PRESSURE_KEEP_MS, "the window is clamped");
        assert_eq!(a.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["L2"], "only the leases of the last three hours");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
