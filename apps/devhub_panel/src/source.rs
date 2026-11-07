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
//! - the shows (`show list`) every 5 s: a held one's row says why it stays at its pair.
//!
//! A lost master (its restart) is redialled every 2 s, and the panel's header says so meanwhile.
//! The UI gets a new fleet only when something it shows changed (not the clock, not a holder's
//! beat): an idle fleet once a caps round. A hidden panel (its window closed to the tray) asks
//! every 5 s instead of every second, each cadence above five times as far apart, and goes back
//! to a second within one when it is shown again.

use crate::demo::Demo;
use crate::model::{Bucket, Fleet, InboxCounts, InboxEntry, JobBrief, JobDetail, LaneBrief, Link, Merge, Side, KEEP_MS};
use makepad_devhub::land::{Entry as LandEntry, St};
use makepad_devhub::lease::panel::{Answer, Row};
use makepad_devhub::lease::sched::NodeView;
use makepad_remote::link::{Client, Target};
use makepad_remote::Event;
use makepad_widgets::{SignalToUI, ThreadOptions, ThreadSpawner};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// [`HIDDEN_TICK`] (a second at most once it is shown again).
    fn rest(&self, t: Instant) {
        std::thread::sleep(TICK.saturating_sub(t.elapsed()));
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
    while !ask.stopped() {
        let t = Instant::now();
        let now = wall_ms();
        let mut changed = m.apply(d.answer(m.next_since(now.saturating_sub(KEEP_MS)), now));
        let want = ask.want.lock().unwrap().clone();
        let before = side.clone();
        d.side(&mut side, now, want.as_deref());
        changed |= side != before;
        if changed || first {
            let mut f = m.fleet(Link::Demo);
            side.onto(&mut f);
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
    let mut lost: Option<(u64, String)> = None;
    let mut reconnected_ms = None;
    while !ask.stopped() {
        let (why, was_up) = match session(&mut m, &mut side, ask, &mut reconnected_ms, lost.is_some()) {
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
fn session(m: &mut Merge, side: &mut Side, ask: &Ask, reconnected_ms: &mut Option<u64>, was_lost: bool) -> Result<(), Ended> {
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
    while !ask.stopped() {
        let t = Instant::now();
        let now = wall_ms();
        let since = m.next_since(now.saturating_sub(KEEP_MS));
        let a = reparse.answer(panel.ask(&since.to_string()).map_err(lost)?).map_err(lost)?;
        let before = side.clone();
        let mut changed = m.apply(a) || n == 0;
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
            f.reconnected_ms = *reconnected_ms;
            ask.publish(f);
        }
        ask.rest(t);
    }
    Ok(())
}
