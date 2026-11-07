//! The fleet as the panel draws it: the master's `panel` answers merged into one model (every
//! node, every lease of the window with absolute times, each node's pressure samples), with the
//! landing train, the last post-land buckets and the inbox counts beside it. The worker merges;
//! the UI gets an immutable [`Fleet`] per answer and derives what it draws from it.

use makepad_devhub::land::Entry as LandEntry;
use makepad_devhub::lease::panel::{Answer, Row, Sample};
use makepad_devhub::lease::sched::NodeView;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// The widest window the panel shows (the master keeps pressure samples this long).
pub const KEEP_MS: u64 = makepad_devhub::lease::panel::PRESSURE_KEEP_MS;

/// A queue wait past this stands out.
pub const LONG_WAIT_MS: u64 = 10 * 60_000;

/// How the panel reaches its master.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Link {
    #[default]
    Connecting,
    Live,
    /// the master stopped answering at `since_ms` (a restart, most often): the panel redials
    Lost { since_ms: u64, why: String },
    /// the master answers but has no `panel` service: it comes with the master's next restart
    Older,
    /// a synthetic fleet built in code (`DEVHUB_PANEL_DEMO=1`)
    Demo,
}

/// One post-land bucket run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bucket {
    pub name: String,
    pub ok: bool,
    pub failing: Vec<String>,
    /// `work,main` short shas of the checked tips
    pub tip: String,
    pub started_ms: u64,
    pub ended_ms: u64,
}

impl Bucket {
    pub fn parse(line: &str) -> Option<Bucket> {
        use makepad_strict_json::Value;
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let short = |x: String| x[..x.len().min(8)].to_string();
        let tip = v.get("tip").map(|t| format!("{},{}", short(s(t, "work")), short(s(t, "main")))).unwrap_or_default();
        Some(Bucket {
            name: v.get("bucket")?.as_str()?.to_string(),
            ok: v.get("ok").and_then(Value::as_bool).unwrap_or(false),
            failing: v.get("failing").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect(),
            tip,
            started_ms: v.get("started_ms").and_then(Value::as_u64).unwrap_or(0),
            ended_ms: v.get("ended_ms").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

/// The inbox's unseen entries by severity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InboxCounts {
    pub unseen: u64,
    pub err: u64,
    pub warn: u64,
    pub req: u64,
}

impl InboxCounts {
    pub fn parse(line: &str) -> Option<InboxCounts> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        v.get("unseen")?;
        Some(InboxCounts { unseen: n("unseen"), err: n("err"), warn: n("warn"), req: n("req") })
    }
}

/// An open job as the master's store has it (`job list brief`): what a lease runs for a lane,
/// and whether its program sits on a parked panic (`paused`: the box stays held).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobBrief {
    pub id: String,
    pub lane: String,
    pub agent: String,
    pub kind: String,
    pub lease: String,
    pub node: String,
    pub state: String,
    pub cmd: String,
    pub started_ms: u64,
    /// the parked panic: its id (`P1`), message, location, when
    pub panic: Option<JobPanic>,
    /// when an agent last acted on it while paused
    pub touched_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobPanic {
    pub id: String,
    pub message: String,
    pub location: String,
    pub at_ms: u64,
}

impl JobBrief {
    pub fn parse(v: &makepad_strict_json::Value) -> Option<JobBrief> {
        use makepad_strict_json::Value;
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        let cmd: Vec<&str> = v.get("cmd").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect();
        let panic = v.get("panic").filter(|p| p.get("message").is_some()).map(|p| JobPanic { id: s(p, "id"), message: s(p, "message"), location: s(p, "location"), at_ms: n(p, "at_ms") });
        Some(JobBrief {
            id: v.get("id")?.as_str()?.to_string(),
            lane: s(v, "lane"),
            agent: s(v, "agent"),
            kind: s(v, "kind"),
            lease: s(v, "lease"),
            node: s(v, "node"),
            state: s(v, "state"),
            cmd: cmd.join(" "),
            started_ms: n(v, "started_ms"),
            panic,
            touched_ms: n(v, "touched_ms"),
        })
    }

    /// A `[..]` answer of jobs.
    pub fn parse_list(lines: &[String]) -> Vec<JobBrief> {
        lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).flat_map(|v| v.as_arr().map(|a| a.to_vec()).unwrap_or_default()).filter_map(|v| JobBrief::parse(&v)).collect()
    }
}

/// One job with its output's last lines and the error lines among them (`job output` from the
/// log's end, asked while its detail is open).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobDetail {
    pub job: JobBrief,
    pub errors: Vec<String>,
    pub tail: Vec<String>,
    /// why the master gave no output (an older master takes no offset from the end)
    pub err: String,
}

/// What a detail asks of its job's log: its end, no more than the pane shows.
pub const TAIL_BYTES: usize = 16 << 10;
/// The lines a detail keeps of that end.
const TAIL_LINES: usize = 15;
const ERROR_LINES: usize = 5;

impl JobDetail {
    /// The detail of `job` from the kept end of its log.
    pub fn of_text(job: JobBrief, text: &str) -> JobDetail {
        let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
        let is_err = |l: &str| {
            let t = l.trim_start();
            t.starts_with("error") || t.contains("panicked at") || t.contains("FAILED") || t.contains(": error:")
        };
        let errors: Vec<String> = lines.iter().filter(|l| is_err(l)).map(|l| l.to_string()).collect();
        JobDetail {
            job,
            errors: errors[errors.len().saturating_sub(ERROR_LINES)..].to_vec(),
            tail: lines[lines.len().saturating_sub(TAIL_LINES)..].iter().map(|l| l.to_string()).collect(),
            err: String::new(),
        }
    }

    /// The `job output <id> -TAIL_BYTES` request.
    pub fn ask(id: &str) -> String {
        format!("job output {id} -{TAIL_BYTES}")
    }

    /// The detail of job `id` from an answer to [`Self::ask`].
    pub fn from_output(id: &str, lines: &[String]) -> JobDetail {
        use makepad_strict_json::Value;
        let v = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok());
        let job = v.as_ref().and_then(|v| v.get("job")).and_then(JobBrief::parse).unwrap_or_else(|| JobBrief { id: id.to_string(), ..Default::default() });
        let data = match v.as_ref().and_then(|v| v.get("data")).and_then(Value::as_str) {
            Some(d) => makepad_devhub::lease::master::b64_decode(d).unwrap_or_default(),
            None => {
                let err = v.as_ref().and_then(|v| v.get("err")).and_then(Value::as_str).unwrap_or("no answer");
                return JobDetail { job, err: err.to_string(), ..Default::default() };
            }
        };
        let text = String::from_utf8_lossy(&data);
        // a full read from the end starts mid-line: that line goes
        let text = if data.len() >= TAIL_BYTES { text.split_once('\n').map_or("", |(_, rest)| rest) } else { &text };
        JobDetail::of_text(job, text)
    }
}

/// A lane's branch as the master's sync loop last saw it (`lanes list`): when the lane last did
/// anything of its own (a commit, a lease, a job).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneBrief {
    pub branch: String,
    /// up-to-date, landed, merged, conflict, dirty, held, failed, waiting, dormant
    pub result: String,
    pub active_ms: u64,
}

impl LaneBrief {
    pub fn parse(line: &str) -> Option<LaneBrief> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(LaneBrief { branch: v.get("branch")?.as_str()?.to_string(), result: s("result"), active_ms: v.get("active_ms").and_then(|x| x.as_u64()).unwrap_or(0) })
    }
}

/// An unseen inbox entry (`inbox list`): its newest time and its stream line
/// (`15:04:51Z WARN node subject: text`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxEntry {
    pub id: String,
    pub count: u64,
    pub last_ms: u64,
    pub line: String,
}

impl InboxEntry {
    pub fn parse(line: &str) -> Option<InboxEntry> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(InboxEntry { id: v.get("id")?.as_str()?.to_string(), count: v.get("count").and_then(|x| x.as_u64()).unwrap_or(1), last_ms: v.get("last_ms").and_then(|x| x.as_u64()).unwrap_or(0), line: s("line") })
    }

    /// `ERR`, `WARN`, `INFO` or `REQ`: the line's second word.
    pub fn sev(&self) -> &str {
        self.line.split_whitespace().nth(1).unwrap_or("")
    }
}

/// What the UI draws: one immutable view of the fleet.
#[derive(Clone, Debug, Default)]
pub struct Fleet {
    pub link: Link,
    /// the master's clock at its last answer
    pub now_ms: u64,
    /// the oldest time this model holds leases for
    pub from_ms: u64,
    pub nodes: Vec<NodeView>,
    /// every lease of the window, by asked time (shared with the model: a fleet a second
    /// copies pointers, not leases)
    pub rows: Vec<Arc<Row>>,
    pub pressure: HashMap<String, Arc<Vec<Sample>>>,
    pub land: Vec<LandEntry>,
    pub buckets: Vec<Bucket>,
    pub inbox: Option<InboxCounts>,
    /// the open jobs (None: this master answers no brief job list)
    pub jobs: Option<Vec<JobBrief>>,
    pub lanes: Vec<LaneBrief>,
    /// the unseen inbox entries, newest first
    pub inbox_list: Vec<InboxEntry>,
    /// the job whose detail is open, with its output's tail
    pub detail: Option<JobDetail>,
    /// the apps shown on the person's devices (`devhub shows`): the running ones and the recent
    pub shows: Vec<makepad_devhub::state::Show>,
    /// since when each node has said it self-updates (seen by this panel)
    pub updating_since: HashMap<String, u64>,
    /// when the panel last found its master again after losing it (a master restart)
    pub reconnected_ms: Option<u64>,
}

impl Fleet {
    /// A lease's queue wait: request to grant, or to now while it waits.
    pub fn wait_ms(&self, r: &Row) -> u64 {
        r.granted_ms.unwrap_or(if r.state == "queued" { self.now_ms } else { r.ended_ms.unwrap_or(self.now_ms) }).saturating_sub(r.asked_ms)
    }

    /// What a lease waited on, summed per blocker in first-seen order (the master logs a wait
    /// each time its blocker changes): "budget cores 1m20s, lane cap 35 s"; empty without waits.
    pub fn waited_on(&self, r: &Row) -> String {
        let mut by: Vec<(&str, u64)> = Vec::new();
        for w in &r.waits {
            let ms = (if w.end_ms == 0 { self.now_ms } else { w.end_ms }).saturating_sub(w.start_ms);
            match by.iter_mut().find(|(b, _)| *b == w.blocker) {
                Some(e) => e.1 += ms,
                None => by.push((&w.blocker, ms)),
            }
        }
        by.iter().map(|(b, ms)| format!("{} {}", if b.is_empty() { "unobserved" } else { b }, crate::fmt::dur(*ms))).collect::<Vec<_>>().join(", ")
    }

    /// Where a lease's bar ends: its end, or now while it runs.
    pub fn end_ms(&self, r: &Row) -> u64 {
        r.ended_ms.unwrap_or(self.now_ms)
    }

    /// The waiting requests, in queue order (interactive first, then by position and age).
    pub fn queued(&self) -> Vec<&Row> {
        let mut q: Vec<&Row> = self.rows.iter().map(|r| &**r).filter(|r| r.state == "queued").collect();
        q.sort_by(|a, b| (a.priority != "interactive").cmp(&(b.priority != "interactive")).then(a.asked_ms.cmp(&b.asked_ms)));
        q
    }

    pub fn running(&self) -> usize {
        self.rows.iter().filter(|r| matches!(r.state.as_str(), "starting" | "granted")).count()
    }

    /// Queue waits that overlap `[t0, t1]`: (p50, p90, max, count) in ms.
    pub fn wait_stats(&self, t0: u64, t1: u64) -> Option<(u64, u64, u64, usize)> {
        let mut w: Vec<u64> = self.rows.iter().filter(|r| {
            let end = r.granted_ms.unwrap_or(if r.state == "queued" { self.now_ms } else { r.ended_ms.unwrap_or(self.now_ms) });
            end >= t0 && r.asked_ms <= t1
        }).map(|r| self.wait_ms(r)).collect();
        if w.is_empty() {
            return None;
        }
        w.sort_unstable();
        let q = |p: f64| w[((w.len() - 1) as f64 * p).round() as usize];
        Some((q(0.5), q(0.9), *w.last().unwrap(), w.len()))
    }

    /// What held the queue in `[t0, t1]`: blocker -> waited ms, most first. The prebuilds wait
    /// behind timed work by design, so their waits are a blocker of their own.
    pub fn blockers(&self, t0: u64, t1: u64) -> Vec<(String, u64)> {
        let mut by: BTreeMap<String, u64> = BTreeMap::new();
        for r in &self.rows {
            for w in &r.waits {
                let end = if w.end_ms == 0 { self.now_ms } else { w.end_ms };
                let ms = end.min(t1).saturating_sub(w.start_ms.max(t0));
                if ms > 0 {
                    *by.entry(blocker_key(r, &w.blocker)).or_default() += ms;
                }
            }
        }
        let mut v: Vec<(String, u64)> = by.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    pub fn node(&self, name: &str) -> Option<&NodeView> {
        self.nodes.iter().find(|n| n.name == name)
    }

    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().map(|r| &**r).find(|r| r.id == id)
    }

    /// The open job a lease runs.
    pub fn job_of(&self, lease: &str) -> Option<&JobBrief> {
        self.jobs.as_ref()?.iter().find(|j| j.lease == lease)
    }
}

/// A wait's blocker as the panel groups it (the master already normalises the reason).
pub fn blocker_key(r: &Row, blocker: &str) -> String {
    let b = if blocker.is_empty() { "unobserved" } else { blocker };
    if r.lane.starts_with(makepad_devhub::lease::PREBUILD_LANE_PREFIX) { format!("prebuild {b}") } else { b.to_string() }
}

/// The worker's side: answers merged into one model.
#[derive(Default)]
pub struct Merge {
    rows: HashMap<String, Arc<Row>>,
    nodes: Vec<NodeView>,
    /// the nodes as the last answer had them without their gauges (caps, latencies), to tell
    /// a change
    nodes_seen: Vec<String>,
    /// and with them: each node's caps round moves its gauges at its own moment
    gauges_seen: Vec<String>,
    /// gauges or pressure samples moved since the UI last got a fleet, and when it last did
    /// for them
    gauges_pending: bool,
    gauges_shown_ms: u64,
    /// the live leases whose beat was stale at the last answer
    stale: HashSet<String>,
    pressure: HashMap<String, Arc<Vec<Sample>>>,
    updating_since: HashMap<String, u64>,
    pub now_ms: u64,
    pub from_ms: u64,
}

/// The master's other services, as they last answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    pub shows: Vec<makepad_devhub::state::Show>,
    pub land: Vec<LandEntry>,
    pub buckets: Vec<Bucket>,
    pub inbox: Option<InboxCounts>,
    pub inbox_list: Vec<InboxEntry>,
    pub jobs: Option<Vec<JobBrief>>,
    pub lanes: Vec<LaneBrief>,
    pub detail: Option<JobDetail>,
}

impl Side {
    pub fn onto(&self, f: &mut Fleet) {
        f.land = self.land.clone();
        f.buckets = self.buckets.clone();
        f.inbox = self.inbox;
        f.inbox_list = self.inbox_list.clone();
        f.jobs = self.jobs.clone();
        f.lanes = self.lanes.clone();
        f.detail = self.detail.clone();
        f.shows = self.shows.clone();
    }
}

/// How often gauges and pressure samples alone make a new fleet: one caps round.
const GAUGES_MS: u64 = 10_000;

impl Merge {
    /// The `since` of the next ask: a little before the last answer, so nothing that changed
    /// around it is missed.
    pub fn next_since(&self, first: u64) -> u64 {
        if self.now_ms == 0 { first } else { self.now_ms.saturating_sub(2_000) }
    }

    /// Merges an answer; whether it changed what a fleet shows. A lease's beat counts only when
    /// it goes stale or fresh again (holders beat every 2 s), the clock not at all: an idle
    /// fleet changes once a caps round, with its samples.
    pub fn apply(&mut self, a: Answer) -> bool {
        if self.from_ms == 0 || a.since_ms < self.from_ms {
            self.from_ms = a.since_ms;
        }
        self.now_ms = a.now_ms;
        let seen: Vec<String> = a.nodes.iter().map(|n| NodeView { caps: None, latency: String::new(), ..n.clone() }.to_json()).collect();
        let mut changed = seen != self.nodes_seen;
        self.nodes_seen = seen;
        let gauges: Vec<String> = a.nodes.iter().map(|n| n.to_json()).collect();
        self.gauges_pending |= gauges != self.gauges_seen;
        self.gauges_seen = gauges;
        self.nodes = a.nodes;
        let now = a.now_ms;
        self.updating_since.retain(|n, _| self.nodes.iter().any(|v| &v.name == n && v.updating.is_some()));
        for n in self.nodes.iter().filter(|n| n.updating.is_some()) {
            self.updating_since.entry(n.name.clone()).or_insert(now);
        }
        let keep_from = a.now_ms.saturating_sub(KEEP_MS);
        let present: HashSet<String> = a.rows.iter().map(|r| r.id.clone()).collect();
        // a queued or live lease the master no longer lists is gone (it ended in the answer's
        // window, or a restarted master no longer has it)
        let had = self.rows.len();
        self.rows.retain(|id, r| r.state == "ended" || present.contains(id));
        changed |= self.rows.len() != had;
        // (a lease that ended before the window is not taken, so it is not dropped again below)
        for r in a.rows.into_iter().filter(|r| r.ended_ms.is_none_or(|e| e >= keep_from)) {
            // an unchanged lease keeps the copy the last fleets share
            let differs = match self.rows.get(&r.id) {
                None => Some(true),
                Some(have) if **have == r => None,
                Some(have) => Some(have.beat_ms == r.beat_ms || Row { beat_ms: have.beat_ms, ..r.clone() } != **have),
            };
            if let Some(d) = differs {
                changed |= d;
                self.rows.insert(r.id.clone(), Arc::new(r));
            }
        }
        let stale: HashSet<String> = self.rows.values().filter(|r| r.beat_ms.is_some_and(|b| now.saturating_sub(b) >= crate::needs::STALE_BEAT_MS)).map(|r| r.id.clone()).collect();
        changed |= stale != self.stale;
        self.stale = stale;
        // a node's samples are copied only when they change (a sample per caps round)
        for (node, samples) in a.pressure {
            let have = self.pressure.entry(node).or_default();
            let last = have.last().map_or(0, |s| s.t_ms);
            if samples.last().is_some_and(|s| s.t_ms > last) {
                Arc::make_mut(have).extend(samples.into_iter().filter(|s| s.t_ms > last));
                self.gauges_pending = true;
            }
        }
        let had = self.rows.len();
        self.rows.retain(|_, r| r.ended_ms.is_none_or(|e| e >= keep_from));
        changed |= self.rows.len() != had;
        for s in self.pressure.values_mut() {
            let cut = s.partition_point(|x| x.t_ms < keep_from);
            if cut > 0 {
                Arc::make_mut(s).drain(..cut);
            }
        }
        self.from_ms = self.from_ms.max(keep_from);
        // gauges and samples alone show once a caps round, else with the next change
        if self.gauges_pending && (changed || now.saturating_sub(self.gauges_shown_ms) >= GAUGES_MS) {
            changed = true;
            self.gauges_pending = false;
            self.gauges_shown_ms = now;
        }
        changed
    }

    pub fn fleet(&self, link: Link) -> Fleet {
        let mut rows: Vec<Arc<Row>> = self.rows.values().cloned().collect();
        rows.sort_by(|a, b| a.asked_ms.cmp(&b.asked_ms).then_with(|| a.id.cmp(&b.id)));
        Fleet { link, now_ms: self.now_ms, from_ms: self.from_ms, nodes: self.nodes.clone(), rows, pressure: self.pressure.clone(), updating_since: self.updating_since.clone(), ..Default::default() }
    }
}

/// Packs spans into rows: each takes the first row free from its start on. `spans` are
/// (start, end) in time order of start; the answer is each span's row.
pub fn pack(spans: &[(u64, u64)]) -> Vec<usize> {
    let mut ends: Vec<u64> = Vec::new();
    spans
        .iter()
        .map(|&(start, end)| match ends.iter().position(|e| *e <= start) {
            Some(i) => {
                ends[i] = end.max(start + 1);
                i
            }
            None => {
                ends.push(end.max(start + 1));
                ends.len() - 1
            }
        })
        .collect()
}

/// Packs spans into at most `cap` rows, in the order given (the first get their places first:
/// a lane's flagged and running leases before its history); a span that fits in no row is None.
pub fn pack_capped(spans: &[(u64, u64)], cap: usize) -> Vec<Option<usize>> {
    let mut rows: Vec<Vec<(u64, u64)>> = vec![Vec::new(); cap];
    spans
        .iter()
        .map(|&(start, end)| {
            let end = end.max(start + 1);
            let i = rows.iter().position(|r| r.iter().all(|&(a, b)| end <= a || b <= start))?;
            rows[i].push((start, end));
            Some(i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer that says nothing new changes nothing (an idle panel does not redraw), nor
    /// does a holder's beat alone; a lease that changes does.
    #[test]
    fn only_a_changed_answer_changes_the_fleet() {
        let start = 1_800_000_000_000;
        let demo = crate::demo::Demo::new(start);
        let mut m = Merge::default();
        let a = demo.answer(start - KEEP_MS, start);
        assert_eq!(a.rows.iter().map(|r| r.id.as_str()).collect::<HashSet<_>>().len(), a.rows.len(), "each lease once in an answer");
        assert!(m.apply(a.clone()), "the first answer");
        assert!(!m.apply(a.clone()), "the same answer again");
        let mut beat = a.clone();
        let i = beat.rows.iter().position(|r| r.beat_ms.is_some_and(|b| start - b < 2_000)).expect("a lease with a fresh beat");
        beat.rows[i].beat_ms = beat.rows[i].beat_ms.map(|b| b + 1_000);
        assert!(!m.apply(beat.clone()), "a beat alone");
        let mut gauge = beat.clone();
        gauge.nodes[0].latency.push_str(" p50 1s");
        assert!(!m.apply(gauge.clone()), "a node's gauges alone, within a caps round");
        let mut ended = gauge.clone();
        ended.rows[i].state = "ended".into();
        ended.rows[i].ended_ms = Some(start);
        assert!(m.apply(ended.clone()), "a lease that ended");
        let mut later = ended;
        later.nodes[0].latency.push_str(" p90 2s");
        assert!(!m.apply(later.clone()), "gauges again: they just went out with the ended lease");
        later.now_ms += GAUGES_MS;
        assert!(m.apply(later), "gauges a caps round later");
    }

    /// A job's detail is the end of its log: its last lines and the error lines among them; a
    /// master that cannot answer says why.
    #[test]
    fn a_detail_is_its_logs_end() {
        let job = r#"{"id":"J7","lane":"x","state":"running","cmd":["makepad","check"]}"#;
        let ok = format!(r#"{{"job":{job},"data":"YnVpbGRpbmcKZXJyb3JbRTAzMDhdOiBtaXNtYXRjaGVkIHR5cGVzCnRoZSBsYXN0IGxpbmUK","off":53}}"#);
        let d = JobDetail::from_output("J7", &[ok]);
        assert_eq!(JobDetail::ask("J7"), "job output J7 -16384");
        assert_eq!(d.job.id, "J7");
        assert_eq!(d.job.cmd, "makepad check");
        assert_eq!(d.tail, ["building", "error[E0308]: mismatched types", "the last line"]);
        assert_eq!(d.errors, ["error[E0308]: mismatched types"]);
        assert!(d.err.is_empty());
        let old = JobDetail::from_output("J7", &[r#"{"err":"job output: invalid offset"}"#.to_string()]);
        assert_eq!((old.job.id.as_str(), old.err.as_str()), ("J7", "job output: invalid offset"));
        assert!(old.tail.is_empty());
    }

    /// A capped packing places the first spans first, never overlaps two in a row, and says
    /// which did not fit.
    #[test]
    fn capped_packing_keeps_the_first_and_overflows_the_rest() {
        // two running (10..100, 20..100), then history that fits around them or not
        let spans = [(10, 100), (20, 100), (0, 9), (5, 30), (101, 120)];
        assert_eq!(pack_capped(&spans, 2), vec![Some(0), Some(1), Some(0), None, Some(0)]);
    }

    /// One synthetic fleet: the merged model keeps each lease once with its newest state, the
    /// queue is the waiting ones, packing never overlaps two leases in a row, and the stats see
    /// the waits and their blockers.
    #[test]
    fn a_synthetic_fleet_merges_packs_and_counts() {
        let start = 1_800_000_000_000;
        let demo = crate::demo::Demo::new(start);
        let mut m = Merge::default();
        m.apply(demo.answer(start - KEEP_MS, start));
        m.apply(demo.answer(m.next_since(0), start + 60_000));
        let f = m.fleet(Link::Demo);
        let ids: HashSet<&str> = f.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), f.rows.len(), "each lease once");
        assert!(f.nodes.len() >= 8 && f.nodes.iter().any(|n| !n.up), "every node, a down one too");
        assert!(f.rows.iter().all(|r| r.state == "queued" || f.node(&r.node).is_some()), "every placed lease is on a known node");
        let q = f.queued();
        assert!(!q.is_empty() && q.iter().all(|r| r.node.is_empty() && r.granted_ms.is_none()), "the queue is the waiting requests");
        for n in &f.nodes {
            let mut mine: Vec<&Row> = f.rows.iter().map(|r| &**r).filter(|r| r.node == n.name).collect();
            mine.sort_by_key(|r| r.asked_ms);
            let spans: Vec<(u64, u64)> = mine.iter().map(|r| (r.asked_ms, f.end_ms(r))).collect();
            let rows = pack(&spans);
            for (i, a) in spans.iter().enumerate() {
                for (j, b) in spans.iter().enumerate().skip(i + 1) {
                    assert!(rows[i] != rows[j] || a.1 <= b.0 || b.1 <= a.0, "{} packs two overlapping leases into row {}", n.name, rows[i]);
                }
            }
        }
        let (p50, p90, max, n) = f.wait_stats(start - 3_600_000, start + 60_000).expect("waits in the last hour");
        assert!(n > 0 && p50 <= p90 && p90 <= max, "wait quantiles ordered: {p50} {p90} {max}");
        assert!(!f.blockers(start - 3_600_000, start + 60_000).is_empty(), "the waits name their blockers");
        let samples = f.pressure.values().map(|s| s.len()).max().unwrap_or(0);
        assert!(samples > 300, "pressure samples for the whole window: {samples}");
    }
}
