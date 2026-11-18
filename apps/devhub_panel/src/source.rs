//! Where the panel's fleet comes from: the running master (its control socket, read-only), or
//! the synthetic demo fleet. One worker thread asks and merges; the UI takes the newest
//! [`Fleet`] when the worker's signal arrives and never waits on it.
//!
//! What the master is asked, each on a channel kept open (the master runs a thread per open
//! channel, so channels are opened once per connection, not per ask):
//! - `panel <since>` once a second: the first ask after a (re)connect takes the whole window,
//!   every later one what changed since the last answer and the live leases and nodes (only the
//!   lines that differ from the last answer are parsed);
//! - `job list brief` every 5 s (the open jobs, no log read) and, while a job's detail is open,
//!   `job output <id> -16384` every 5 s (its log's last 16 KB, the most the pane shows);
//! - the landing train every 5 s (all of it once per connection, then what is open or recent);
//! - the inbox counts every 10 s and its unseen entries every 15 s;
//! - the bucket ledger every 30 s and the lanes' branches every 60 s;
//! - the shows (`show list`) every 5 s: a held one's row says why it stays at its pair;
//! - the history (`history`, `exact` and `rows` on the panel channel) when the timeline shows a
//!   stretch before the live window: that stretch's load, and its leases while the view spans a
//!   day or less ([`fetch_past`]); asked again only when the view leaves what was fetched or its
//!   column width changes, at once when the view moves there, not at the next round.
//!
//! A lost master (its restart) is redialled every 2 s, and the panel's header says so meanwhile.
//! The UI gets a new fleet only when something it shows changed (not the clock, not a holder's
//! beat): an idle fleet once a caps round. A hidden panel (its window closed to the tray) asks
//! every 5 s instead of every second, each cadence above five times as far apart, and goes back
//! to a second within one when it is shown again.

use crate::demo::Demo;
use crate::model::{Bucket, Fleet, InboxCounts, InboxEntry, JobBrief, JobDetail, LaneBrief, Link, Marks, Merge, Past, Side, ViewReq, ViewSink, EXACT_COL_MS, KEEP_MS, PAST_FIELDS, ROWS_SPAN_MS};
use makepad_devhub::land::{Entry as LandEntry, St};
use makepad_devhub::lease::panel::{Answer, Hist, Row, MAX_COLS};
use makepad_devhub::lease::sched::NodeView;
use makepad_remote::link::{Client, Target};
use makepad_remote::Event;
use makepad_widgets::{SignalToUI, ThreadOptions, ThreadSpawner};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one answer may take before the master counts as lost.
const ANSWER_WAIT: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_secs(1);
/// A hidden panel's round: its tray glyph only needs the fleet's state, not each second of it.
const HIDDEN_TICK: Duration = Duration::from_secs(5);
/// What a plain `land status` keeps of the landed: its last hour.
const LAND_RECENT_MS: u64 = 3_600_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Master,
    Demo,
}

impl Mode {
    /// `DEVHUB_PANEL_DEMO=1`: the synthetic fleet; else the master.
    pub fn from_env() -> Mode {
        if std::env::var("DEVHUB_PANEL_DEMO").is_ok_and(|v| v == "1") { Mode::Demo } else { Mode::Master }
    }
}

pub struct Source {
    shared: Arc<Ask>,
}

impl Drop for Source {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Source {
    pub fn start(spawner: &ThreadSpawner, mode: Mode) -> Result<Source, String> {
        let shared = Arc::new(Ask::default());
        let ask = shared.clone();
        spawner
            .spawn_worker(ThreadOptions::default(), move || match mode {
                Mode::Master => master(&ask),
                Mode::Demo => demo(&ask),
            })
            .map_err(|e| format!("devhub panel worker: {e:?}"))?
            .detach();
        Ok(Source { shared })
    }

    /// The newest fleet since the last take (None: nothing new).
    pub fn take(&self) -> Option<Arc<Fleet>> {
        self.shared.latest.lock().unwrap().take()
    }

    /// Asks for a job's output from the next answer on (None: no detail open).
    pub fn want_job(&self, id: Option<String>) {
        *self.shared.want.lock().unwrap() = id;
    }

    /// Where the timeline says what it shows.
    pub fn view_sink(&self) -> Arc<ViewSink> {
        self.shared.sink.clone()
    }

    /// The panel's window is closed to the tray (true) or shown: a hidden panel asks less often.
    pub fn set_hidden(&self, hidden: bool) {
        self.shared.hidden.store(hidden, Ordering::Relaxed);
    }
}

/// What the UI and the worker share.
#[derive(Default)]
struct Ask {
    latest: Mutex<Option<Arc<Fleet>>>,
    /// the job whose detail is open (its output is asked for while it is)
    want: Mutex<Option<String>>,
    hidden: AtomicBool,
    stop: AtomicBool,
    /// what the timeline shows, and the oldest time the live window holds
    sink: Arc<ViewSink>,
    live_from: AtomicU64,
}

impl Ask {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn publish(&self, f: Fleet) {
        *self.latest.lock().unwrap() = Some(Arc::new(f));
        SignalToUI::set_ui_signal();
    }

    /// Waits out the round that began at `t`: a second, or while the panel is hidden up to
    /// [`HIDDEN_TICK`] (a second at most once it is shown again); a view moved before the live
    /// window ends it at once.
    fn rest(&self, t: Instant) {
        while !self.stopped() && t.elapsed() < TICK {
            std::thread::sleep(TICK.saturating_sub(t.elapsed()).min(Duration::from_millis(50)));
            if self.sink.moved_before(self.live_from.load(Ordering::Relaxed)) {
                return;
            }
        }
        while self.hidden.load(Ordering::Relaxed) && !self.stopped() && t.elapsed() < HIDDEN_TICK {
            std::thread::sleep(TICK.min(HIDDEN_TICK.saturating_sub(t.elapsed())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line seen in the last answer gives the same lease or node as parsing it does, and only
    /// what the master still sends is kept.
    #[test]
    fn reparsed_answers_match_parsed_ones() {
        let start = 1_800_000_000_000;
        let demo = Demo::new(start);
        let mut r = Reparse::default();
        for now in [start, start + 1_000, start + 30_000] {
            let lines = demo.answer(now - 2_000, now).lines();
            let want = Answer::parse(&lines).unwrap();
            let got = r.answer(lines.clone()).unwrap();
            assert_eq!((got.now_ms, got.since_ms), (want.now_ms, want.since_ms));
            assert_eq!(got.rows, want.rows);
            assert_eq!(got.nodes.iter().map(|n| n.to_json()).collect::<Vec<_>>(), want.nodes.iter().map(|n| n.to_json()).collect::<Vec<_>>());
            assert_eq!(got.pressure, want.pressure);
            assert_eq!(r.last.len(), want.rows.len() + want.nodes.len());
        }
    }

    /// The landings and bucket runs are asked for the whole view, the live window's part too, once
    /// per stretch: a pan within the margin asks nothing, past it both again; a view that follows
    /// now asks again only once now passes the stretch; an older master's refusal is no entry.
    #[test]
    fn the_landings_and_bucket_runs_are_asked_once_per_stretch() {
        use std::cell::RefCell;
        let (t, h) = (1_800_000_000_000u64, 3_600_000u64);
        let asked = RefCell::new(Vec::<String>::new());
        let mut ask = |l: &str| -> Result<Vec<String>, String> {
            let w: Vec<&str> = l.split_whitespace().collect();
            asked.borrow_mut().push(w[0].to_string());
            let from: u64 = w[1].parse().unwrap();
            Ok(match w[0] {
                "landings" => vec![format!(r#"{{"t":"landings","from_ms":{from},"to_ms":0,"n":1}}"#), format!(r#"{{"id":7,"branch":"rc-a","state":"landed","asked_ms":{from},"changed_ms":{}}}"#, from + 1)],
                _ => vec![r#"{"err":"panel <since_ms>"}"#.to_string()],
            })
        };
        let mut p = PastState::default();
        let mut step = |p: &mut PastState, l: u64, r: u64| -> (bool, Vec<String>) {
            let changed = fetch_marks(&mut ask, p, Some(ViewReq { t_left: l, t_right: r, col_ms: 1.0 })).unwrap();
            (changed, asked.borrow_mut().drain(..).collect())
        };
        let both = vec!["landings".to_string(), "buckets".to_string()];
        assert_eq!(step(&mut p, t - 3 * h, t), (true, both.clone()), "a view: its landings and bucket runs");
        let m = p.marks.clone().unwrap();
        assert_eq!((m.land.iter().map(|e| e.id).collect::<Vec<_>>(), m.buckets.len()), (vec![7], 0), "the landing; a refused buckets ask is no run");
        assert_eq!((m.from_ms, m.to_ms), (t - 3 * h - 3 * h / 2, t + 3 * h / 2), "half a view's margin each side");
        assert_eq!(step(&mut p, t - 3 * h + h, t + h), (false, vec![]), "following now within the margin");
        assert_eq!(step(&mut p, t - 3 * h + 2 * h, t + 2 * h), (true, both), "past it: both again");
        p.unsupported = true;
        assert_eq!(step(&mut p, t, t + h), (true, vec![]), "a master with no history: nothing asked, the marks go");
    }

    /// The worker asks the history only for what the view needs and was not fetched: a view in
    /// the live window asks nothing, one before it its load and (a day or less wide) its leases
    /// once; a pan within the margin asks nothing, past it again; a zoom asks the load again at
    /// its new width; a wide view asks no leases and drops them; a close-up asks exact samples;
    /// back in the live window the past goes; an older master is asked nothing more.
    #[test]
    fn the_past_is_asked_once_per_stretch_and_width() {
        use std::cell::RefCell;
        let live_from = 1_800_000_000_000u64;
        let h = 3_600_000u64;
        let asked = RefCell::new(Vec::<String>::new());
        let older = RefCell::new(false);
        let mut ask = |l: &str| -> Result<Vec<String>, String> {
            asked.borrow_mut().push(l.split_whitespace().next().unwrap().to_string());
            if *older.borrow() {
                return Ok(vec![r#"{"err":"panel <since_ms>"}"#.to_string()]);
            }
            let w: Vec<u64> = l.split_whitespace().skip(1).take(3).filter_map(|x| x.parse().ok()).collect();
            Ok(match l.split_whitespace().next() {
                Some("history") => vec![
                    format!(r#"{{"t":"hist","from_ms":{},"per_ms":{},"n":{},"level":1,"first_ms":5}}"#, w[0], w[1], w[2]),
                    r#"{"t":"cols","node":"node-a","field":"cpu_pct","min":[1],"max":[2],"mean":[1.5]}"#.to_string(),
                ],
                Some("exact") => vec![format!(r#"{{"t":"exact","from_ms":{},"per_ms":1,"n":0,"level":0,"first_ms":5}}"#, w[0]), r#"{"t":"pressure","node":"node-a","s":[[7,50,1,2,3,4,5,6,7]]}"#.to_string()],
                _ => vec![format!(r#"{{"t":"rows","from_ms":{},"per_ms":1,"n":1,"level":0,"first_ms":null}}"#, w[0]), Row { id: "L1".into(), asked_ms: w[0], ..Row::default() }.to_json()],
            })
        };
        let mut p = PastState::default();
        let live = std::cell::Cell::new(live_from);
        let mut step = |p: &mut PastState, v: ViewReq| -> (bool, Vec<String>) {
            let changed = fetch_past(&mut ask, p, Some(v), live.get()).unwrap();
            (changed, asked.borrow_mut().drain(..).collect())
        };
        let view = |l: u64, r: u64| ViewReq { t_left: l, t_right: r, col_ms: (r - l) as f64 / 400.0 };
        assert_eq!(step(&mut p, view(live_from, live_from + h)), (false, vec![]), "all in the live window");
        let v = view(live_from - 2 * h, live_from + h);
        assert_eq!(step(&mut p, v), (true, vec!["history".into(), "rows".into()]), "a stretch before it");
        assert_eq!((p.first_ms, p.samples.as_ref().map(|s| s.per_ms)), (Some(5), Some(27_000)), "its width and the oldest sample");
        assert_eq!(step(&mut p, v), (false, vec![]), "the same view again");
        let pan = |v: ViewReq, by: u64| ViewReq { t_left: v.t_left - by, t_right: v.t_right - by, ..v };
        assert_eq!(step(&mut p, pan(v, h)), (false, vec![]), "a pan within the margin");
        assert_eq!(step(&mut p, pan(v, 3 * h)), (true, vec!["history".into(), "rows".into()]), "a pan past it");
        let zoomed = view(live_from - 5 * h + h / 4, live_from - 2 * h - h / 4);
        assert_eq!(step(&mut p, zoomed), (true, vec!["history".into()]), "a zoom: the load at its width, the leases fetched already");
        let mut f = Fleet { rows: vec![Arc::new(Row { id: "L1".into(), node: "live".into(), ..Row::default() })], ..Fleet::default() };
        p.onto(&mut f);
        assert_eq!(f.rows.iter().map(|r| r.node.as_str()).collect::<Vec<_>>(), ["live"], "a lease the live window has is its own");
        let day = view(live_from - 21 * h, live_from + 3 * h);
        assert_eq!(step(&mut p, day), (true, vec!["history".into(), "rows".into()]), "a day view");
        live.set(live_from + 60_000);
        assert_eq!(step(&mut p, pan(day, 0)), (false, vec![]), "following it: the live window's start moved a minute, nothing is asked");
        live.set(live_from);
        assert_eq!(step(&mut p, view(live_from - 72 * h, live_from)), (true, vec!["history".into()]), "a wide view: the load alone");
        assert!(p.rows.is_none(), "its leases are dropped");
        let close = view(live_from - h, live_from - h / 2);
        assert_eq!(step(&mut p, close), (true, vec!["exact".into(), "rows".into()]), "a close-up: exact samples");
        assert_eq!(p.samples.as_ref().unwrap().exact["node-a"][0].cpu_pct, 50.0);
        assert_eq!(step(&mut p, view(live_from, live_from + h)), (true, vec![]), "back in the live window: the past goes");
        assert!(p.samples.is_none() && p.rows.is_none());
        *older.borrow_mut() = true;
        assert_eq!(step(&mut p, v).1, vec!["history".to_string()]);
        assert!(p.unsupported, "an older master has no history");
        assert_eq!(step(&mut p, pan(v, 10 * h)), (false, vec![]), "and is not asked again");
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn demo(ask: &Ask) {
    let start = wall_ms();
    let d = Demo::new(start);
    let mut m = Merge::default();
    let mut side = Side::default();
    // the panel has watched for a while: what it tracks itself (self-update since) has a past
    m.apply(d.answer(start - KEEP_MS, start - crate::demo::WATCHED_MS));
    let mut first = true;
    // the month before the run, as a master's history answers it
    let mut past = PastState { first_ms: Some(d.first_ms()), ..PastState::default() };
    while !ask.stopped() {
        let t = Instant::now();
        let now = wall_ms();
        let mut changed = m.apply(d.answer(m.next_since(now.saturating_sub(KEEP_MS)), now));
        ask.live_from.store(m.from_ms, Ordering::Relaxed);
        changed |= fetch_past(&mut |l: &str| Ok(d.history(l, now)), &mut past, ask.sink.get(), m.from_ms).unwrap_or(false);
        changed |= fetch_marks(&mut |l: &str| Ok(d.history(l, now)), &mut past, ask.sink.get()).unwrap_or(false);
        let want = ask.want.lock().unwrap().clone();
        let before = side.clone();
        d.side(&mut side, now, want.as_deref());
        changed |= side != before;
        if changed || first {
            let mut f = m.fleet(Link::Demo);
            side.onto(&mut f);
            past.onto(&mut f);
            ask.publish(f);
            first = false;
        }
        ask.rest(t);
    }
}

/// One service channel of the master, kept open: one request line, its answer lines.
struct Chan(makepad_remote::Channel);

impl Chan {
    fn open(c: &Client, service: &str) -> Result<Chan, String> {
        c.open(service, "").map(Chan).map_err(|e| e.to_string())
    }

    fn ask(&self, line: &str) -> Result<Vec<String>, String> {
        self.0.send(line.as_bytes().to_vec()).map_err(|e| format!("link lost: {e}"))?;
        let mut got = Vec::new();
        loop {
            match self.0.recv_timeout(ANSWER_WAIT) {
                Ok(Event::Data(d)) => {
                    for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                        if l.starts_with("{\"end\":") {
                            return Ok(got);
                        }
                        got.push(l.to_string());
                    }
                }
                Ok(Event::Opened(_)) => {}
                Ok(Event::Closed(_, why)) => return Err(format!("link lost: {why}")),
                Err(true) => return Err(format!("no answer in {} s", ANSWER_WAIT.as_secs())),
                Err(false) => return Err("link lost".into()),
            }
        }
    }
}

impl Side {
    /// A plain `land status` answer (what is open, and what landed in its last hour) replaces
    /// all of that; the window's older landings, known from the connection's `status all`, stay.
    fn merge_land(&mut self, lines: &[String], now: u64) {
        let keep_from = now.saturating_sub(KEEP_MS);
        let hour_ago = now.saturating_sub(LAND_RECENT_MS);
        self.land.retain(|e| e.state == St::Landed && e.changed_ms >= keep_from && e.changed_ms < hour_ago);
        let old = self.land.len();
        for e in lines.iter().filter_map(|l| LandEntry::parse(l)) {
            if !self.land[..old].iter().any(|x| x.id == e.id) {
                self.land.push(e);
            }
        }
        self.land.sort_by_key(|e| e.id);
    }
}

fn master(ask: &Ask) {
    let mut m = Merge::default();
    let mut side = Side::default();
    let mut past = PastState::default();
    let mut lost: Option<(u64, String)> = None;
    let mut reconnected_ms = None;
    while !ask.stopped() {
        let (why, was_up) = match session(&mut m, &mut side, &mut past, ask, &mut reconnected_ms, lost.is_some()) {
            Ok(()) => return,
            Err(Ended::Older) => {
                lost = None;
                ask.publish(m.fleet(Link::Older));
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
            Err(Ended::Lost(why, was_up)) => (why, was_up),
        };
        // a link that was up and broke is a new loss; a redial that failed keeps the old one's time
        let since = match &lost {
            Some((s, _)) if !was_up => *s,
            _ => wall_ms(),
        };
        lost = Some((since, why.clone()));
        let mut f = m.fleet(if m.now_ms == 0 { Link::Connecting } else { Link::Lost { since_ms: since, why } });
        side.onto(&mut f);
        past.onto(&mut f);
        f.reconnected_ms = reconnected_ms;
        ask.publish(f);
        std::thread::sleep(Duration::from_secs(2));
    }
}

enum Ended {
    /// why, and whether the link had come up first
    Lost(String, bool),
    /// the master has no `panel` service
    Older,
}

/// The master sends every live lease and every node each second, nearly all of them in the
/// same lines as the last answer: a line the last answer had is not parsed again.
#[derive(Default)]
struct Reparse {
    last: HashMap<String, Item>,
}

#[derive(Clone)]
enum Item {
    Node(NodeView),
    Row(Row),
}

impl Reparse {
    fn answer(&mut self, lines: Vec<String>) -> Result<Answer, String> {
        let mut kept = HashMap::with_capacity(self.last.len());
        let (mut nodes, mut rows, mut rest) = (Vec::new(), Vec::new(), Vec::new());
        for l in lines {
            let item = match self.last.remove(&l) {
                Some(item) => item,
                None if l.starts_with(r#"{"t":"node""#) || l.starts_with(r#"{"t":"lease""#) => {
                    let v = makepad_strict_json::parse(l.as_bytes()).map_err(|_| "panel: a bad line".to_string())?;
                    let item = if l.starts_with(r#"{"t":"node""#) { v.get("v").and_then(NodeView::parse).map(Item::Node) } else { Row::parse(&v).map(Item::Row) };
                    let Some(item) = item else { continue };
                    item
                }
                // the head, pressure samples, an error: as the master's own parser reads them
                None => {
                    rest.push(l);
                    continue;
                }
            };
            match &item {
                Item::Node(n) => nodes.push(n.clone()),
                Item::Row(r) => rows.push(r.clone()),
            }
            kept.insert(l, item);
        }
        self.last = kept;
        let mut a = Answer::parse(&rest)?;
        a.nodes = nodes;
        a.rows = rows;
        Ok(a)
    }
}

/// One connection's life: asks until the link breaks.
fn session(m: &mut Merge, side: &mut Side, past: &mut PastState, ask: &Ask, reconnected_ms: &mut Option<u64>, was_lost: bool) -> Result<(), Ended> {
    let c = Client::new(Target::Unix(makepad_devhub::proto::control_socket()), "devhub-panel");
    c.conn().map_err(|e| Ended::Lost(e.to_string(), false))?;
    let has = |svc: &str| c.caps().iter().any(|s| s.split('/').next() == Some(svc));
    if !has(makepad_devhub::lease::panel::SERVICE) {
        return Err(Ended::Older);
    }
    let lost = |e: String| Ended::Lost(e, true);
    let panel = Chan::open(&c, makepad_devhub::lease::panel::SERVICE).map_err(lost)?;
    let land = Chan::open(&c, "land").map_err(lost)?;
    let inbox = Chan::open(&c, "inbox").map_err(lost)?;
    // the orchestration and sync services and the bucket ledger come with a master's checkout
    let open = |svc: &str| if has(svc) { Chan::open(&c, svc).map(Some).map_err(lost) } else { Ok(None) };
    let job = open("job")?;
    let lanes = open("lanes")?;
    let bucket = open("bucket")?;
    let show = open("show")?;
    if was_lost {
        *reconnected_ms = Some(wall_ms());
    }
    let mut n: u64 = 0;
    let mut wanted: Option<String> = None;
    let mut reparse = Reparse::default();
    // the history's oldest sample (an older master has no history: its stretch stays empty)
    past.unsupported = false;
    match Hist::parse(&panel.ask(&format!("history {} 60000 0 cpu_pct", wall_ms())).map_err(lost)?) {
        Ok(h) => past.first_ms = h.first_ms,
        Err(_) => past.unsupported = true,
    }
    while !ask.stopped() {
        let t = Instant::now();
        let now = wall_ms();
        let since = m.next_since(now.saturating_sub(KEEP_MS));
        let a = reparse.answer(panel.ask(&since.to_string()).map_err(lost)?).map_err(lost)?;
        let before = side.clone();
        let mut changed = m.apply(a) || n == 0;
        ask.live_from.store(m.from_ms, Ordering::Relaxed);
        changed |= fetch_past(&mut |l: &str| panel.ask(l), past, ask.sink.get(), m.from_ms).map_err(lost)?;
        changed |= fetch_marks(&mut |l: &str| panel.ask(l), past, ask.sink.get()).map_err(lost)?;
        if n % 5 == 0 {
            if n == 0 {
                // the whole train once: its landings older than the window are dropped here
                let keep_from = now.saturating_sub(KEEP_MS);
                side.land = land.ask("status all").map_err(lost)?.iter().filter_map(|l| LandEntry::parse(l)).filter(|e| e.state != St::Landed || e.changed_ms >= keep_from).collect();
            } else {
                let lines = land.ask("status").map_err(lost)?;
                side.merge_land(&lines, now);
            }
        }
        if n % 10 == 0 {
            side.inbox = inbox.ask("counts").map_err(lost)?.first().and_then(|l| InboxCounts::parse(l));
        }
        if n % 15 == 0 {
            side.inbox_list = inbox.ask("list").map_err(lost)?.iter().filter_map(|l| InboxEntry::parse(l)).collect();
        }
        // a detail just opened asks at once, not at the next round
        let want_now = ask.want.lock().unwrap().clone();
        let opened = want_now != wanted;
        wanted = want_now;
        if let Some(j) = job.as_ref().filter(|_| n % 5 == 0 || opened) {
            // an older master answers `brief` with an error: no job states then
            let lines = j.ask("job list brief").map_err(lost)?;
            side.jobs = (!lines.iter().any(|l| l.contains("\"err\""))).then(|| JobBrief::parse_list(&lines));
            side.detail = match &wanted {
                Some(id) => Some(JobDetail::from_output(id, &j.ask(&JobDetail::ask(id)).map_err(lost)?)),
                None => None,
            };
        }
        if let Some(l) = lanes.as_ref().filter(|_| n % 60 == 0) {
            side.lanes = l.ask("lanes list").map_err(lost)?.iter().filter_map(|l| LaneBrief::parse(l)).collect();
        }
        if let Some(s) = show.as_ref().filter(|_| n % 5 == 0) {
            let lines = s.ask("show list").map_err(lost)?;
            let list = lines.first().and_then(|l| makepad_strict_json::parse_depth(l.as_bytes(), 16).ok());
            side.shows = list.as_ref().and_then(|v| v.as_arr()).unwrap_or(&[]).iter().map(makepad_devhub::state::show_from).collect();
        }
        if let Some(b) = bucket.as_ref().filter(|_| n % 30 == 0) {
            let mut v: Vec<Bucket> = b.ask("status").map_err(lost)?.iter().filter_map(|l| Bucket::parse(l)).collect();
            v.sort_by_key(|b| b.ended_ms);
            side.buckets = v;
        }
        n += 1;
        // the UI draws only what changed: an idle fleet is redrawn once a caps round
        changed |= *side != before;
        if changed {
            let mut f = m.fleet(Link::Live);
            side.onto(&mut f);
            past.onto(&mut f);
            f.reconnected_ms = *reconnected_ms;
            ask.publish(f);
        }
        ask.rest(t);
    }
    Ok(())
}

/// What of the history the worker fetched for the view.
#[derive(Default)]
struct PastState {
    samples: Option<Arc<Past>>,
    /// the range asked and the leases that ran in it
    rows: Option<(u64, u64, Vec<Arc<Row>>)>,
    first_ms: Option<u64>,
    /// the master has no history (an older one)
    unsupported: bool,
    /// the landings and bucket runs of the stretch asked
    marks: Option<Arc<Marks>>,
}

impl PastState {
    /// The past onto a fleet: its load, and its leases beside the live window's (a lease both
    /// have is the live one).
    fn onto(&self, f: &mut Fleet) {
        f.past = self.samples.clone();
        f.first_ms = self.first_ms;
        f.marks = self.marks.clone();
        if let Some((_, _, rows)) = &self.rows {
            let have: HashSet<&str> = f.rows.iter().map(|r| r.id.as_str()).collect();
            let extra: Vec<Arc<Row>> = rows.iter().filter(|r| !have.contains(r.id.as_str())).cloned().collect();
            if !extra.is_empty() {
                f.rows.extend(extra);
                f.rows.sort_by(|a, b| a.asked_ms.cmp(&b.asked_ms).then_with(|| a.id.cmp(&b.id)));
            }
        }
    }
}

/// The landings and bucket runs of the whole view, asked from the master's history with half a
/// view's margin each side when the view leaves what was fetched: the live side holds only the
/// train since the window's start and each bucket's last runs. A view that follows now asks
/// again once now passes the stretch's end. An older master answers none, for that stretch.
fn fetch_marks(ask: &mut dyn FnMut(&str) -> Result<Vec<String>, String>, p: &mut PastState, v: Option<ViewReq>) -> Result<bool, String> {
    let Some(v) = v.filter(|_| !p.unsupported) else { return Ok(p.marks.take().is_some()) };
    if p.marks.as_ref().is_some_and(|m| m.from_ms <= v.t_left && m.to_ms >= v.t_right) {
        return Ok(false);
    }
    let span = v.t_right.saturating_sub(v.t_left).max(60_000);
    let (from, to) = (v.t_left.saturating_sub(span / 2), v.t_right.saturating_add(span / 2));
    let entries = |lines: Vec<String>| lines.into_iter().filter(|l| !l.starts_with("{\"t\"") && !l.starts_with("{\"err\""));
    let land = entries(ask(&format!("landings {from} {to}"))?).filter_map(|l| LandEntry::parse(&l)).collect();
    let mut buckets: Vec<Bucket> = entries(ask(&format!("buckets {from} {to}"))?).filter_map(|l| Bucket::parse(&l)).collect();
    buckets.sort_by_key(|b| b.started_ms);
    p.marks = Some(Arc::new(Marks { from_ms: from, to_ms: to, land, buckets }));
    Ok(true)
}

/// What of the view lies before the live window (`live_from`) and is not fetched yet, asked from
/// the master's history with half a view's margin each side: the load (exact samples for a column
/// under [`EXACT_COL_MS`], else columns of the view's width) and, while the view spans at most
/// [`ROWS_SPAN_MS`], the leases. The stretch runs into the live window (what both have is the
/// live one's), so a view that follows now asks again once the live window's start passes its
/// end, not each second as that start moves. A view all in the live window drops what was
/// fetched. Whether the past changed; an error only for the link.
fn fetch_past(ask: &mut dyn FnMut(&str) -> Result<Vec<String>, String>, p: &mut PastState, v: Option<ViewReq>, live_from: u64) -> Result<bool, String> {
    let Some(v) = v.filter(|v| !p.unsupported && live_from > 0 && v.t_left < live_from) else {
        let had = p.samples.take().is_some() | p.rows.take().is_some();
        return Ok(had);
    };
    let span = v.t_right.saturating_sub(v.t_left).max(60_000);
    let from = v.t_left.saturating_sub(span / 2);
    let to = v.t_right.saturating_add(span / 2);
    let need_to = live_from.min(v.t_right);
    let per = (v.col_ms.round() as u64).max(1);
    let exact = per < EXACT_COL_MS;
    let mut changed = false;
    let have = p.samples.as_ref().is_some_and(|s| (s.per_ms == 0) == exact && (exact || s.per_ms == per) && s.from_ms <= v.t_left && s.to_ms >= need_to);
    if !have {
        let (line, from) = if exact {
            (format!("exact {from} {to}"), from)
        } else {
            let from = from / per * per;
            let n = to.saturating_sub(from).div_ceil(per).min(MAX_COLS);
            (format!("history {from} {per} {n} {}", PAST_FIELDS.join(",")), from)
        };
        match Hist::parse(&ask(&line)?) {
            Ok(h) => {
                p.first_ms = h.first_ms.or(p.first_ms);
                p.samples = Some(Arc::new(Past {
                    from_ms: from,
                    to_ms: to,
                    per_ms: if exact { 0 } else { per },
                    exact: h.exact.into_iter().collect(),
                    cols: h.cols.into_iter().map(|(n, f, c)| ((n, f), c)).collect(),
                }));
            }
            // an older master: no history to show, nor asked again
            Err(_) => {
                p.unsupported = true;
                return Ok(false);
            }
        }
        changed = true;
    }
    if span <= ROWS_SPAN_MS {
        if !p.rows.as_ref().is_some_and(|(a, b, _)| *a <= v.t_left && *b >= need_to) {
            // (a master without `rows` answers none, and is not asked again for this stretch)
            let rows = Hist::parse(&ask(&format!("rows {from} {to}"))?).map(|h| h.rows).unwrap_or_default();
            p.rows = Some((from, to, rows.into_iter().map(Arc::new).collect()));
            changed = true;
        }
    } else if p.rows.take().is_some() {
        changed = true;
    }
    Ok(changed)
}
