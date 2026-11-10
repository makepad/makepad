//! The control panel's read view (`panel` on the master's control socket, read by
//! apps/devhub_panel): every node, every lease with absolute times (queued, live, and the ended
//! ones of the asked window), and each node's pressure samples. One request line,
//! `panel <since_ms>`, one answer; read-only. The ended leases come from the scheduler's memory,
//! and from the ledger only for the part of the window the scheduler no longer holds, read
//! backwards from the ledger's end (a panel that keeps asking from its last answer never reads
//! it). Pressure samples are what the node's caps said every [`super::master`] caps round, with
//! the scheduler's reservation on the box at that moment; they live in the scheduler's memory
//! for [`PRESSURE_KEEP_MS`].

use super::sched::{Lease, NodeView, Sched, State};
use super::{num, Cost, Kind};
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

pub const SERVICE: &str = "panel";

/// How long each node's pressure samples are kept: the panel's widest window.
pub const PRESSURE_KEEP_MS: u64 = 3 * 3_600_000;

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
        }
    }

    fn json(&self) -> String {
        format!("[{},{:.1},{:.2},{:.1},{:.1},{:.1},{:.0},{:.1}]", self.t_ms, self.cpu_pct, self.load1, self.ram_used_gb, self.res_cores, self.res_ram_gb, self.gpu_pct, self.gpu_used_gb)
    }

    fn parse(v: &Value) -> Option<Sample> {
        let a = v.as_arr()?;
        let f = |i: usize| a.get(i).and_then(num).unwrap_or(-1.0) as f32;
        Some(Sample { t_ms: a.first().and_then(num)? as u64, cpu_pct: f(1), load1: f(2), ram_used_gb: f(3), res_cores: f(4), res_ram_gb: f(5), gpu_pct: f(6), gpu_used_gb: f(7) })
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
        let measured_ms = match l.cost_source { super::sched::CostSource::Measured(ms) => Some(ms), _ => None };
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
        })
    }

    pub fn to_json(&self) -> String {
        let opt = |o: Option<u64>| o.map_or("null".to_string(), |v| v.to_string());
        let waits: Vec<String> = self.waits.iter().map(|w| format!("[{},{},{}]", w.start_ms, w.end_ms, json_str(&w.blocker))).collect();
        format!(
            "{{\"t\":\"lease\",\"id\":{},\"lane\":{},\"agent\":{},\"kind\":{},\"state\":{},\"node\":{},\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{},\"how\":{},\"reason\":{},\"pos\":{},\"eta_ms\":{},\"priority\":{},\"cost\":{},\"cost_source\":{},\"measured_ms\":{},\"cores\":{},\"ram_gb\":{},\"gpus\":{},\"whole\":{},\"src\":{},\"class\":{},\"running\":{},\"disk_mb\":{},\"waits\":[{}],\"live\":{},\"stalled\":{},\"beat_ms\":{}}}",
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
            opt(self.beat_ms)
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
        let row = |l: &Lease| Row { live: s.is_live_job(l), stalled: s.is_stalled(&l.id), ..Row::of_lease(l) };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::sched::Req;
    use crate::lease::{Caps, Role};

    fn caps(name: &str) -> Caps {
        Caps {
            node: name.into(),
            hub_proto: crate::lease::HUB_PROTO,
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
        // an older lease only the ledger still has, twice (its later line wins), and one before the window
        let line = |id: &str, ended: u64, how: &str| format!("{{\"id\":\"{id}\",\"lane\":\"lane-a\",\"kind\":\"build\",\"node\":\"node-b\",\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{ended},\"how\":\"{how}\",\"cost\":\"large\",\"cost_source\":\"kind\",\"measured_ms\":null,\"alloc\":{{\"cores\":6,\"ram_gb\":10,\"gpus\":0}},\"phases\":[{{\"phase\":\"queued\",\"blocker\":\"budget cores\",\"boxes\":[],\"start_ms\":{},\"end_ms\":{}}}],\"req\":{{\"agent\":\"AGENT-A\",\"class\":\"build cargo\"}}}}\n", ended - 9000, ended - 8000, ended - 9000, ended - 8000);
        std::fs::write(&ledger, [line("L1", 1_000, "released"), line("L2", 50_000, "expired"), line("L2", 51_000, "released")].concat()).unwrap();
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
