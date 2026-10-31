//! Domains on the headless HAL: a privileged "wm" session starts two child domains through a
//! `DomainHost` (a fake host that runs a tiny fake app on its own thread per domain), composites
//! their embedded windows, forwards input, relaunches, stops, lists. Every session pumps and submits
//! from its own thread.

use makepad_hal::buffers::{pack_domains, pack_forward, read_domains, read_strs, unpack_forward, DomainInfo};
use makepad_hal::ledger::Kind;
use makepad_hal::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);
const WIN: Id = Id::new(0, 1);

fn open_req(key: u64, parent: u32) -> OpenReq {
    OpenReq { backend: backend_id::HEADLESS, app_key: key, flags: 0, dpi_hint: 0.0, parent }
}

// ------------------------------------------------------------------ what a session sees

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    ChildWindow { child: u32, window: Id, texture: Id, w: u32, h: u32 },
    ChildClosed { child: u32, window: Id },
    State { domain: u32, state: u32, req: u32, app: String, msg: String },
    Listed { req: u32, list: Vec<DomainInfo> },
    Other(String),
}

fn collect(hal: &Hal, store: &mut EventStore, sid: u32, out: &mut Vec<Ev>, timeout_ns: u64) {
    let mut buf = store.buf();
    let n = hal.pump(timeout_ns, &mut buf);
    for (_, e) in store.reader(n, buf.arena_len) {
        out.push(match e {
            HalEvent::ChildWindow { window, child, texture, w, h, .. } => Ev::ChildWindow { child, window, texture, w, h },
            HalEvent::ChildWindowClosed { window, child } => Ev::ChildClosed { child, window },
            HalEvent::DomainState { domain, state, req, text, .. } => {
                let strs = read_strs(text, 2);
                Ev::State { domain, state, req, app: strs[0].clone(), msg: strs[1].clone() }
            }
            HalEvent::DomainListed { req, count, list } => Ev::Listed { req, list: read_domains(list, count) },
            other => Ev::Other(format!("{other:?}")),
        });
    }
    let _ = sid;
}

/// Pumps `sid` until `pred` holds for everything seen so far.
fn wait_for(hal: &Hal, store: &mut EventStore, sid: u32, out: &mut Vec<Ev>, pred: impl Fn(&[Ev]) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !pred(out) {
        assert!(Instant::now() < deadline, "timed out; saw {out:#?}");
        collect(hal, store, sid, out, 20_000_000);
    }
}

fn windows_of(evs: &[Ev], child: u32) -> Vec<(Id, u32, u32)> {
    evs.iter().filter_map(|e| if let Ev::ChildWindow { child: c, texture, w, h, .. } = e { (*c == child).then_some((*texture, *w, *h)) } else { None }).collect()
}

fn state_of(evs: &[Ev], domain: u32) -> Option<u32> {
    evs.iter().rev().find_map(|e| if let Ev::State { domain: d, state, .. } = e { (*d == domain).then_some(*state) } else { None })
}

// ------------------------------------------------------------------ the fake host and its apps

struct Running {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

struct Fake {
    hal: Weak<Hal>,
    apps: Mutex<HashMap<u32, Running>>,
    logs: Mutex<HashMap<u32, Arc<Mutex<Vec<String>>>>>,
    starts: AtomicU32,
    stops: Mutex<Vec<(u32, u32)>>,
    /// a relaunch stops the old app and waits here for `release` before starting the new one
    hold: AtomicBool,
    held: Mutex<Option<DomainLaunch>>,
}

impl Fake {
    fn new(hal: &Arc<Hal>) -> Arc<Fake> {
        Arc::new(Fake {
            hal: Arc::downgrade(hal),
            apps: Mutex::new(HashMap::new()),
            logs: Mutex::new(HashMap::new()),
            starts: AtomicU32::new(0),
            stops: Mutex::new(Vec::new()),
            hold: AtomicBool::new(false),
            held: Mutex::new(None),
        })
    }

    fn log(&self, domain: u32) -> Arc<Mutex<Vec<String>>> {
        self.logs.lock().unwrap().entry(domain).or_default().clone()
    }

    fn saw(&self, domain: u32, what: &str) -> bool {
        self.log(domain).lock().unwrap().iter().any(|l| l.contains(what))
    }

    /// The app runs on its own thread: wait until its log has `what`.
    fn wait_saw(&self, domain: u32, what: &str) {
        let t0 = Instant::now();
        while !self.saw(domain, what) {
            assert!(t0.elapsed() < WAIT, "{what} never reached domain {domain}: {:?}", self.log(domain).lock().unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn spawn(&self, l: DomainLaunch) {
        let hal = self.hal.upgrade().expect("hal");
        let stop = Arc::new(AtomicBool::new(false));
        let log = self.log(l.domain);
        let flag = stop.clone();
        let domain = l.domain;
        let join = std::thread::spawn(move || fake_app(hal, l, flag, log));
        self.apps.lock().unwrap().insert(domain, Running { stop, join });
    }

    fn halt(&self, domain: u32) {
        let r = self.apps.lock().unwrap().remove(&domain);
        if let Some(r) = r {
            r.stop.store(true, Ordering::SeqCst);
            r.join.join().unwrap();
        }
    }

    fn halt_all(&self) {
        let ids: Vec<u32> = self.apps.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.halt(id);
        }
    }

    /// Stops and relaunches reach the host on a thread of the HAL's own: wait until it was told.
    fn wait_stopped(&self, domain: u32, mode: u32) {
        let t0 = Instant::now();
        while !self.stops.lock().unwrap().contains(&(domain, mode)) {
            assert!(t0.elapsed() < WAIT, "the host was never told to stop domain {domain} (mode {mode}): {:?}", self.stops.lock().unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Starts the relaunch the host is holding, once it reached the host.
    fn release(&self) {
        let t0 = Instant::now();
        let l = loop {
            if let Some(l) = self.held.lock().unwrap().take() {
                break l;
            }
            assert!(t0.elapsed() < WAIT, "no relaunch reached the host");
            std::thread::sleep(Duration::from_millis(5));
        };
        self.spawn(l);
    }
}

impl DomainHost for Fake {
    fn start(&self, launch: DomainLaunch) -> Result<(), String> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.spawn(launch);
        Ok(())
    }

    fn stop(&self, domain: u32, mode: u32) {
        self.stops.lock().unwrap().push((domain, mode));
        if mode == domain_stop::KILL {
            self.halt(domain);
        }
    }

    fn relaunch(&self, launch: DomainLaunch) -> Result<(), String> {
        self.halt(launch.domain);
        if self.hold.load(Ordering::SeqCst) {
            *self.held.lock().unwrap() = Some(launch);
        } else {
            self.spawn(launch);
        }
        Ok(())
    }
}

/// A tiny app: a window, a texture and a pipeline of its own, a frame; a frame more per click;
/// quits when asked to.
fn fake_app(hal: Arc<Hal>, l: DomainLaunch, stop: Arc<AtomicBool>, log: Arc<Mutex<Vec<String>>>) {
    let info = hal.open(&open_req(l.app_key, l.parent));
    assert_eq!(info.session, l.domain, "the app opens the session the parent asked for");
    let sid = info.session;
    let mut w = CmdWriter::new(sid);
    let mut frame = 1u64;
    w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 200.0, 100.0], false);
    w.texture_create(Id::new(0, 1), usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    let blob = CmdWriter::pipeline_blob(PipelineDesc { name: 1, ..Default::default() }, b"code", &[], &[]);
    w.pipeline_create(Id::new(0, 1), dialect::MSL, Attr::default(), &blob);
    w.frame_begin(frame, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
    w.frame_end(WIN, target::WINDOW);
    hal.submit(&w.buf());
    w.clear();
    let mut store = EventStore::new(sid, 64, 8192);
    while !stop.load(Ordering::SeqCst) {
        let mut buf = store.buf();
        let n = hal.pump(20_000_000, &mut buf);
        let mut clicked = false;
        let mut quit = false;
        for (_, e) in store.reader(n, buf.arena_len) {
            if !matches!(e, HalEvent::Wake {}) {
                log.lock().unwrap().push(format!("{e:?}"));
            }
            clicked |= matches!(e, HalEvent::MouseDown { .. });
            quit |= matches!(e, HalEvent::Lifecycle { kind } if kind == lifecycle::QUIT_REQUESTED);
        }
        if clicked {
            frame += 1;
            w.frame_begin(frame, FrameMeta::default());
            w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
            w.frame_end(WIN, target::WINDOW);
            hal.submit(&w.buf());
            w.clear();
        }
        if quit {
            w.quit();
            hal.submit(&w.buf());
            return;
        }
    }
}

/// A frame of the wm that draws the given textures (one draw each) with a pipeline of its own.
fn composite(w: &mut CmdWriter, frame: u64, win: Id, textures: &[Id]) {
    w.frame_begin(frame, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
    for t in textures {
        let tex = w.put_ids(&[*t]);
        w.draw(Draw { pipeline: Id::new(0, 1), tex, ..Default::default() });
    }
    w.frame_end(win, target::WINDOW);
}

struct Rig {
    hal: Arc<Hal>,
    fake: Arc<Fake>,
    wm: u32,
    store: EventStore,
    evs: Vec<Ev>,
    w: CmdWriter,
    frame: u64,
}

impl Rig {
    fn new() -> Rig {
        let hal = Hal::new();
        let fake = Fake::new(&hal);
        hal.set_domain_host(fake.clone());
        hal.with_world(|w| w.trace_exec = true);
        let wm = hal.open_domain(&open_req(1, 0), Caps { privileged: true }).session;
        assert_ne!(wm, 0);
        let mut w = CmdWriter::new(wm);
        w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 800.0, 600.0], false);
        let blob = CmdWriter::pipeline_blob(PipelineDesc { name: 1, ..Default::default() }, b"code", &[], &[]);
        w.pipeline_create(Id::new(0, 1), dialect::MSL, Attr::default(), &blob);
        hal.submit(&w.buf());
        w.clear();
        let store = EventStore::new(wm, 256, 1 << 16);
        Rig { hal, fake, wm, store, evs: Vec::new(), w, frame: 0 }
    }

    fn submit(&mut self) {
        self.hal.submit(&self.w.buf());
        self.w.clear();
    }

    fn wait(&mut self, pred: impl Fn(&[Ev]) -> bool) {
        wait_for(&self.hal, &mut self.store, self.wm, &mut self.evs, pred);
    }

    fn stats(&self, sid: u32) -> makepad_hal::session::Stats {
        self.hal.with_world(|w| w.session(sid).expect("session").stats)
    }

    fn rows(&self, sid: u32) -> (usize, usize, usize, usize) {
        self.hal.with_world(|w| {
            let s = w.session(sid).expect("session");
            (s.textures.len(), s.buffers.len(), s.pipelines.len(), s.ledger.len())
        })
    }

    /// The wm draws the textures; returns the exec record of that frame.
    fn composite(&mut self, textures: &[Id]) -> ExecRecord {
        self.frame += 1;
        composite(&mut self.w, self.frame, WIN, textures);
        self.submit();
        let (wm, frame) = (self.wm, self.frame);
        self.hal.with_world(|w| w.exec_log.iter().rev().find(|r| r.session == wm && r.frame == frame).cloned().expect("the wm frame ran"))
    }
}

// ------------------------------------------------------------------ tests

#[test]
fn a_wm_runs_two_child_domains() {
    let mut rig = Rig::new();
    let hal = rig.hal.clone();
    let wm = rig.wm;

    // an ordinary session cannot start domains, nor open itself under the wm
    let plain = hal.open(&open_req(2, 0)).session;
    let mut pw = CmdWriter::new(plain);
    pw.str_def(1, b"calc");
    pw.domain_start(5, 1, 0);
    pw.domain_list(6);
    hal.submit(&pw.buf());
    assert_eq!(rig.stats(plain).denied, 2, "start and list from an unprivileged session");
    assert_eq!(rig.fake.starts.load(Ordering::SeqCst), 0);
    assert_eq!(hal.open(&open_req(0xdead_beef, wm)).session, 0, "a parent claim needs the key the wm's DomainStart produced");
    assert_eq!(hal.open(&open_req(0, wm)).session, 0);

    // the wm starts two apps
    rig.w.str_def(1, b"calc");
    rig.w.str_def(2, b"clock");
    rig.w.domain_start(11, 1, 0);
    rig.w.domain_start(12, 2, 0);
    rig.submit();
    assert_eq!(rig.fake.starts.load(Ordering::SeqCst), 2);
    rig.wait(|e| e.iter().filter(|e| matches!(e, Ev::ChildWindow { .. })).count() >= 2 && e.iter().filter(|e| matches!(e, Ev::State { state: domain_state::RUNNING, .. })).count() >= 2);
    let started: Vec<(u32, u32, String)> = rig.evs.iter().filter_map(|e| if let Ev::State { domain, req, app, state: domain_state::COMPILING, .. } = e { Some((*domain, *req, app.clone())) } else { None }).collect();
    assert_eq!(started.len(), 2);
    let (a, b) = (started.iter().find(|s| s.1 == 11).unwrap().0, started.iter().find(|s| s.1 == 12).unwrap().0);
    assert_eq!((started.iter().find(|s| s.0 == a).unwrap().2.as_str(), started.iter().find(|s| s.0 == b).unwrap().2.as_str()), ("calc", "clock"));
    assert!(a != b && a != wm && b != wm);
    let (tex_a, tex_b) = (windows_of(&rig.evs, a)[0], windows_of(&rig.evs, b)[0]);
    assert_eq!((tex_a.1, tex_a.2), (200, 100));
    assert!(tex_a.0.index >= LENT_BASE && tex_b.0.index >= LENT_BASE && tex_a.0 != tex_b.0);

    // both children rendered a frame before the wm composites them
    for d in [a, b] {
        assert!(hal.await_frame_done(d, 1, WAIT));
    }
    let rec = rig.composite(&[tex_a.0, tex_b.0]);
    assert_eq!(rec.deps.len(), 2, "{rec:?}");
    assert_eq!(rig.stats(wm).skipped_draws, 0);
    // ordering: each child frame the wm sampled ran before the wm frame, and no newer one in between
    let log = hal.with_world(|w| w.exec_log.clone());
    for (child, window, frame) in &rec.deps {
        let at = log.iter().position(|r| r.session == *child && r.frame == *frame && r.target == *window).expect("the sampled child frame ran");
        assert!(log[at].seq < rec.seq);
        assert!(!log[at + 1..rec.seq as usize].iter().any(|r| r.session == *child && r.frame > *frame), "a newer child frame ran in between and was not the one sampled");
    }

    // input: a click forwarded to a reaches a and not b; a answers with a new frame
    let click = pack_forward(&RawEvent::mouse_down(Id::NONE, 10.0, 20.0, 1, 0), &[]);
    rig.w.forward(WIN, a, &click);
    rig.submit();
    rig.fake.wait_saw(a, "MouseDown");
    assert!(hal.await_frame_done(a, 2, WAIT));
    std::thread::sleep(Duration::from_millis(100));
    assert!(!rig.fake.saw(b, "MouseDown"));
    assert_eq!(rig.stats(a).forwarded, 1);
    assert_eq!(rig.stats(b).forwarded, 0);
    let rec = rig.composite(&[tex_a.0, tex_b.0]);
    assert!(rec.deps.contains(&(a, WIN, 2)) && rec.deps.contains(&(b, WIN, 1)), "{rec:?}");
    // a session cannot forward into one that is not its child
    let mut pw = CmdWriter::new(plain);
    pw.forward(WIN, a, &click);
    pw.child_focus(a);
    hal.submit(&pw.buf());
    assert_eq!(rig.stats(plain).denied, 4);
    assert_eq!(rig.stats(a).forwarded, 1);

    // focus: keyboard / IME / cursor / clipboard requests of a child count only while it has it
    let ask = |hal: &Hal, d: u32| {
        let mut cw = CmdWriter::new(d);
        cw.ime_show(WIN, [0.0; 4], 0);
        cw.cursor(WIN, 1);
        cw.clip_set(b"x");
        hal.submit(&cw.buf());
    };
    ask(&hal, a);
    ask(&hal, b);
    assert_eq!((rig.stats(a).input_applied, rig.stats(a).input_dropped), (0, 3));
    rig.w.child_focus(a);
    rig.submit();
    ask(&hal, a);
    ask(&hal, b);
    assert_eq!((rig.stats(a).input_applied, rig.stats(a).input_dropped), (3, 3));
    assert_eq!((rig.stats(b).input_applied, rig.stats(b).input_dropped), (0, 6));
    rig.w.child_focus(b);
    rig.submit();
    ask(&hal, a);
    ask(&hal, b);
    assert_eq!((rig.stats(a).input_applied, rig.stats(a).input_dropped), (3, 6));
    assert_eq!((rig.stats(b).input_applied, rig.stats(b).input_dropped), (3, 6));
    rig.fake.wait_saw(a, "Focus { window: Id { index: 0, gen: 1 }, focused: true }");
    rig.fake.wait_saw(a, "focused: false");

    // ChildGeom resizes: a new texture for the window, the old one becomes stale
    rig.w.child_geom(WIN, a, [300.0, 150.0], 2.0);
    rig.submit();
    rig.wait(|e| windows_of(e, a).len() >= 2);
    let resized = *windows_of(&rig.evs, a).last().unwrap();
    assert_eq!((resized.1, resized.2), (600, 300));
    assert_ne!(resized.0, tex_a.0);
    let before = rig.stats(wm);
    let rec = rig.composite(&[tex_a.0, tex_b.0, resized.0]);
    let after = rig.stats(wm);
    assert_eq!((after.stale_lent_draws - before.stale_lent_draws, after.skipped_draws - before.skipped_draws), (1, 1));
    assert_eq!(rec.deps.len(), 2, "the new surface and b's");
    assert_eq!(hal.with_world(|w| w.session(a).unwrap().ledger.entries().filter(|e| e.kind == Kind::Texture).count()), 2, "own texture + resized surface, not both surfaces");

    // relaunch a: its entries are freed, the wm's references to its surface go stale, b is untouched
    let b_rows = rig.rows(b);
    let b_report = hal.report(b, 0).unwrap().total_bytes;
    rig.fake.hold.store(true, Ordering::SeqCst);
    rig.w.domain_relaunch(a);
    rig.submit();
    rig.wait(|e| e.iter().any(|e| matches!(e, Ev::ChildClosed { child, .. } if *child == a)) && state_of(e, a) == Some(domain_state::COMPILING));
    assert_eq!(rig.rows(a), (0, 0, 0, 0), "everything a owned is gone while the new app has not opened yet");
    assert_eq!(hal.with_world(|w| w.session(a).unwrap().windows.len()), 0);
    assert_eq!(rig.rows(b), b_rows);
    assert_eq!(hal.report(b, 0).unwrap().total_bytes, b_report);
    let before = rig.stats(wm);
    let rec = rig.composite(&[resized.0, tex_b.0]);
    let after = rig.stats(wm);
    assert_eq!((after.stale_lent_draws - before.stale_lent_draws, after.skipped_draws - before.skipped_draws), (1, 1));
    assert_eq!(rec.deps.len(), 1);
    let windows_seen = windows_of(&rig.evs, a).len();
    rig.fake.release();
    rig.wait(|e| windows_of(e, a).len() > windows_seen && state_of(e, a) == Some(domain_state::RUNNING));
    let again = *windows_of(&rig.evs, a).last().unwrap();
    assert_eq!((again.1, again.2), (200, 100));
    assert!(again.0 != resized.0 && again.0 != tex_a.0);
    assert!(hal.await_frame_done(a, 1, WAIT));
    assert_eq!(rig.rows(a), (2, 0, 1, 3), "the new app's own entries, not added to the old ones");
    let rec = rig.composite(&[again.0, tex_b.0]);
    assert_eq!(rec.deps.len(), 2);

    // stop b by asking it to quit: it quits itself, the wm hears ChildWindowClosed and EXITED
    rig.w.domain_stop(b, domain_stop::QUIT);
    rig.submit();
    rig.wait(|e| state_of(e, b) == Some(domain_state::EXITED) && e.iter().any(|e| matches!(e, Ev::ChildClosed { child, .. } if *child == b)));
    assert!(hal.with_world(|w| w.session(b).is_none()));
    let before = rig.stats(wm);
    rig.composite(&[again.0, tex_b.0]);
    assert_eq!(rig.stats(wm).stale_lent_draws - before.stale_lent_draws, 1);

    // list: a running, b exited
    rig.w.domain_list(77);
    rig.submit();
    rig.wait(|e| e.iter().any(|e| matches!(e, Ev::Listed { req: 77, .. })));
    let Some(Ev::Listed { list, .. }) = rig.evs.iter().find(|e| matches!(e, Ev::Listed { req: 77, .. })).cloned() else { unreachable!() };
    assert_eq!(list.iter().map(|d| (d.domain, d.state, d.app.as_str())).collect::<Vec<_>>(), [(a, domain_state::RUNNING, "calc"), (b, domain_state::EXITED, "clock")]);
    assert!(list.iter().all(|d| d.parent == wm));

    // kill a: freed at once, the host is told
    rig.w.domain_stop(a, domain_stop::KILL);
    rig.submit();
    assert!(hal.with_world(|w| w.session(a).is_none()));
    rig.fake.wait_stopped(a, domain_stop::KILL);
    rig.wait(|e| state_of(e, a) == Some(domain_state::EXITED));
    assert_eq!(hal.with_world(|w| w.count(Kind::Texture)), 0, "no surface or alias outlives its domain");

    // closing the wm leaves only the host and the unrelated session
    rig.fake.halt_all();
    assert!(hal.close(wm));
    assert_eq!(hal.with_world(|w| w.session_ids()), [0, plain]);
}

#[test]
fn closing_a_parent_closes_its_children_and_asks_the_host_to_stop_them() {
    let mut rig = Rig::new();
    rig.w.str_def(1, b"calc");
    rig.w.domain_start(1, 1, 0);
    rig.submit();
    rig.wait(|e| !e.is_empty() && e.iter().any(|e| matches!(e, Ev::ChildWindow { .. })));
    let child = rig.evs.iter().find_map(|e| if let Ev::State { domain, .. } = e { Some(*domain) } else { None }).unwrap();
    assert!(rig.hal.close(rig.wm));
    assert!(rig.hal.with_world(|w| w.session(child).is_none()));
    rig.fake.wait_stopped(child, domain_stop::KILL);
    rig.fake.halt_all();
    assert_eq!(rig.hal.with_world(|w| w.count(Kind::Texture) + w.count(Kind::Window)), 0);
}

#[test]
fn a_start_without_a_host_or_a_failing_host_reports_a_crash() {
    let hal = Hal::new();
    let wm = hal.open_domain(&open_req(1, 0), Caps { privileged: true }).session;
    let mut store = EventStore::new(wm, 64, 4096);
    let mut evs = Vec::new();
    let mut w = CmdWriter::new(wm);
    w.str_def(1, b"calc");
    w.domain_start(3, 1, 0);
    hal.submit(&w.buf());
    wait_for(&hal, &mut store, wm, &mut evs, |e| e.iter().any(|e| matches!(e, Ev::State { state: domain_state::CRASHED, req: 3, .. })));
    let msg = evs.iter().find_map(|e| if let Ev::State { state: domain_state::CRASHED, msg, .. } = e { Some(msg.clone()) } else { None }).unwrap();
    assert!(msg.contains("no domain host"), "{msg}");
    // the crashed domain's session is freed, its record stays for DomainList
    assert_eq!(hal.with_world(|w| w.session_ids()), [0, wm]);
    w.clear();
    w.domain_list(4);
    hal.submit(&w.buf());
    wait_for(&hal, &mut store, wm, &mut evs, |e| e.iter().any(|e| matches!(e, Ev::Listed { req: 4, .. })));
    let Some(Ev::Listed { list, .. }) = evs.iter().find(|e| matches!(e, Ev::Listed { .. })) else { unreachable!() };
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].state, domain_state::CRASHED);
}

#[test]
fn sessions_pump_and_wake_independently_from_their_own_threads() {
    let hal = Hal::new();
    let a = hal.open(&open_req(1, 0)).session;
    let b = hal.open(&open_req(2, 0)).session;
    let run = |sid: u32, hal: Arc<Hal>| {
        std::thread::spawn(move || {
            let mut store = EventStore::new(sid, 16, 256);
            let mut buf = store.buf();
            hal.pump(0, &mut buf); // Startup
            let t0 = Instant::now();
            let mut buf = store.buf();
            let n = hal.pump(2_000_000_000, &mut buf);
            let evs: Vec<String> = store.reader(n, buf.arena_len).map(|(_, e)| format!("{e:?}")).collect();
            (t0.elapsed(), evs)
        })
    };
    let (ta, tb) = (run(a, hal.clone()), run(b, hal.clone()));
    std::thread::sleep(Duration::from_millis(150));
    hal.wake_session(a);
    let (took_a, evs_a) = ta.join().unwrap();
    let (took_b, evs_b) = tb.join().unwrap();
    assert_eq!(evs_a.len(), 1);
    assert!(evs_a[0].starts_with("Wake"));
    assert!(took_a < Duration::from_millis(1500), "{took_a:?}");
    assert!(evs_b.is_empty() && took_b >= Duration::from_millis(1800), "b stays blocked: {evs_b:?} {took_b:?}");
    // the global wake reaches both, and a wake before a pump is not lost
    hal.wake();
    for sid in [a, b] {
        let mut store = EventStore::new(sid, 16, 256);
        let mut buf = store.buf();
        let n = hal.pump(5_000_000_000, &mut buf);
        assert_eq!(n, 1);
        assert!(matches!(store.reader(n, buf.arena_len).next().unwrap().1, HalEvent::Wake {}));
    }
    // commands of one session are executed on whichever thread submits them
    let hal2 = hal.clone();
    std::thread::spawn(move || {
        let mut w = CmdWriter::new(b);
        w.texture_create(Id::new(1, 1), usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
        hal2.submit(&w.buf());
    })
    .join()
    .unwrap();
    assert_eq!(hal.with_world(|w| w.session(b).unwrap().textures.len()), 1);
}

/// Turns of `sid`'s pump so far.
fn turns(hal: &Hal, sid: u32) -> PumpCounts {
    hal.pump_counts().into_iter().find(|(id, _)| *id == sid).map(|(_, c)| c).unwrap_or_default()
}

#[test]
fn an_idle_wm_with_domains_takes_no_pump_turns() {
    // a WM and three child domains, each pumping from its own thread with nothing to do
    let hal = Hal::new();
    let wm = hal.open(&open_req(201, 0)).session;
    let mut sids = vec![wm];
    for key in 202..205 {
        sids.push(hal.with_world(|w| w.open_session(key, wm, false, 1.0)));
    }
    for sid in &sids {
        let mut store = EventStore::new(*sid, 16, 256);
        let mut buf = store.buf();
        hal.pump(0, &mut buf); // Startup
    }
    let before: Vec<PumpCounts> = sids.iter().map(|sid| turns(&hal, *sid)).collect();
    let idle = Duration::from_millis(400);
    let threads: Vec<_> = sids.iter().map(|sid| {
        let (hal, sid) = (hal.clone(), *sid);
        std::thread::spawn(move || {
            let mut store = EventStore::new(sid, 16, 256);
            let mut buf = store.buf();
            hal.pump(idle.as_nanos() as u64, &mut buf)
        })
    }).collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), 0, "nothing was due");
    }
    for (sid, b) in sids.iter().zip(before) {
        let a = turns(&hal, *sid);
        // one turn finds nothing, the wait ends at the deadline, the next turn returns
        assert_eq!(a.calls - b.calls, 1);
        assert!(a.turns - b.turns <= 2, "session {sid} turned {} times while idle: {a:?}", a.turns - b.turns);
        assert_eq!((a.by_event - b.by_event, a.by_all - b.by_all, a.by_wake - b.by_wake), (0, 0, 0), "session {sid}: {a:?}");
    }
}

#[test]
fn an_idle_session_takes_no_turn_for_two_seconds() {
    // a WM and two domains blocked in their pumps (Cx's idle loop: no timeout of its own)
    let hal = Hal::new();
    let wm = hal.open(&open_req(221, 0)).session;
    let sids = [wm, hal.with_world(|w| w.open_session(222, wm, false, 1.0)), hal.with_world(|w| w.open_session(223, wm, false, 1.0))];
    for sid in sids {
        let mut store = EventStore::new(sid, 16, 256);
        let mut buf = store.buf();
        hal.pump(0, &mut buf); // Startup
    }
    let threads: Vec<_> = sids.iter().map(|sid| {
        let (hal, sid) = (hal.clone(), *sid);
        std::thread::spawn(move || {
            let mut store = EventStore::new(sid, 16, 256);
            let mut buf = store.buf();
            let n = hal.pump(u64::MAX, &mut buf);
            store.reader(n, buf.arena_len).map(|(_, e)| format!("{e:?}")).collect::<Vec<_>>()
        })
    }).collect();
    std::thread::sleep(Duration::from_millis(100));
    let blocked: Vec<PumpCounts> = sids.iter().map(|sid| turns(&hal, *sid)).collect();
    std::thread::sleep(Duration::from_secs(2));
    for (sid, b) in sids.iter().zip(&blocked) {
        assert_eq!(turns(&hal, *sid), *b, "session {sid} turned while idle");
    }
    // each one's own wake ends its wait, and only its
    for (i, sid) in sids.iter().enumerate() {
        hal.wake_session(*sid);
        std::thread::sleep(Duration::from_millis(20));
        for (j, other) in sids.iter().enumerate().skip(i + 1) {
            assert_eq!(turns(&hal, *other).turns, blocked[j].turns, "session {other} turned for {sid}'s wake");
        }
    }
    for t in threads {
        let evs = t.join().unwrap();
        assert!(evs.len() == 1 && evs[0].starts_with("Wake"), "{evs:?}");
    }
}

#[test]
fn news_for_one_session_does_not_turn_another_sessions_pump() {
    let hal = Hal::new();
    let wm = hal.open(&open_req(211, 0)).session;
    let busy = hal.with_world(|w| w.open_session(212, wm, false, 1.0));
    let quiet = hal.with_world(|w| w.open_session(213, wm, false, 1.0));
    for sid in [wm, busy, quiet] {
        let mut store = EventStore::new(sid, 16, 256);
        let mut buf = store.buf();
        hal.pump(0, &mut buf); // Startup
    }
    let mut w = CmdWriter::new(busy);
    w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 200.0, 100.0], false);
    hal.submit(&w.buf());
    let before = turns(&hal, quiet);
    let blocked = {
        let hal = hal.clone();
        std::thread::spawn(move || {
            let mut store = EventStore::new(quiet, 64, 4096);
            let mut buf = store.buf();
            let t0 = Instant::now();
            let n = hal.pump(600_000_000, &mut buf);
            (n, t0.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    // the busy child presents (its own FrameDone, the WM's ChildFrame), gets input forwarded
    // and injected; the WM submits
    for frame in 1..=20u64 {
        let mut w = CmdWriter::new(busy);
        w.frame_begin(frame, FrameMeta::default());
        w.pass(Pass::default());
        w.frame_end(WIN, target::WINDOW);
        hal.submit(&w.buf());
        hal.inject(busy, RawEvent::default());
        hal.submit(&CmdWriter::new(wm).buf());
        std::thread::sleep(Duration::from_millis(5));
    }
    let (n, took) = blocked.join().unwrap();
    assert_eq!(n, 0, "the quiet session had nothing");
    assert!(took >= Duration::from_millis(550), "{took:?}");
    let after = turns(&hal, quiet);
    assert!(after.turns - before.turns <= 2, "the quiet pump turned for others' news: {before:?} -> {after:?}");
    assert_eq!(after.by_event - before.by_event, 0);
    // and the busy sessions did get theirs
    let mut store = EventStore::new(wm, 256, 65536);
    let mut buf = store.buf();
    let n = hal.pump(0, &mut buf);
    assert!(store.reader(n, buf.arena_len).any(|(_, e)| matches!(e, HalEvent::ChildFrame { .. })));
}

#[test]
fn forward_and_domain_payloads_round_trip() {
    let raw = RawEvent::scroll(Id::new(1, 2), 1.5, 2.5, -3.0, 4.0, 3, true, 2);
    let packed = pack_forward(&raw, b"payload");
    let (back, bytes) = unpack_forward(&packed).unwrap();
    assert_eq!(bytes, b"payload");
    assert_eq!((back.tag, back.a, back.b, back.time_ns), (raw.tag, raw.a, raw.b, raw.time_ns));
    assert!(unpack_forward(&packed[..10]).is_none());
    let list = vec![
        DomainInfo { domain: 3, parent: 1, state: domain_state::RUNNING, phase: 2, pct: 0.5, app: "calc".into() },
        DomainInfo { domain: 4, parent: 1, state: domain_state::CRASHED, phase: 0, pct: 0.0, app: String::new() },
    ];
    assert_eq!(read_domains(&pack_domains(&list), 2), list);
    assert_eq!(read_domains(&pack_domains(&list)[..30], 2), list[..1], "a cut record ends the list");
}

#[test]
fn hal_open_takes_the_launch_the_host_gave_the_thread() {
    // the global HAL, through the op the apps call
    struct Host(Mutex<Vec<JoinHandle<u32>>>);
    impl DomainHost for Host {
        fn start(&self, launch: DomainLaunch) -> Result<(), String> {
            let t = std::thread::spawn(move || with_launch(&launch, || hal_open(&open_req(0, 0)).session));
            self.0.lock().unwrap().push(t);
            Ok(())
        }
        fn stop(&self, _: u32, _: u32) {}
        fn relaunch(&self, _: DomainLaunch) -> Result<(), String> {
            Ok(())
        }
    }
    let hal = Hal::global();
    let host = Arc::new(Host(Mutex::new(Vec::new())));
    hal.set_domain_host(host.clone());
    let wm = hal.open_domain(&open_req(0x77, 0), Caps { privileged: true }).session;
    let mut w = CmdWriter::new(wm);
    w.str_def(1, b"calc");
    w.domain_start(1, 1, 0);
    hal.submit(&w.buf());
    let opened = host.0.lock().unwrap().pop().unwrap().join().unwrap();
    assert_ne!(opened, 0);
    assert_eq!(hal.with_world(|w| w.session(opened).map(|s| (s.parent, s.privileged, s.attached))), Some((wm, false, true)));
    // outside a launch the same request is a parent claim without the key: refused
    assert_eq!(hal_open(&open_req(5, wm)).session, 0);
    // and an app cannot open itself privileged: the op takes no capabilities
    assert!(!hal.with_world(|w| w.session(opened).unwrap().privileged));
    assert!(hal.close(wm));
}

#[test]
fn child_frames_notify_the_parent_and_coalesce_per_window() {
    let hal = Hal::new();
    let parent = hal.open(&open_req(101, 0)).session;
    let child = hal.with_world(|w| w.open_session(102, parent, false, 1.0));
    let other = hal.with_world(|w| w.open_session(103, parent, false, 1.0));
    let plain = hal.open(&open_req(104, 0)).session;
    for sid in [child, other, plain] {
        let mut w = CmdWriter::new(sid);
        w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 200.0, 100.0], false);
        // Deliberately reuse the window id in the texture namespace.
        w.texture_create(WIN, usage::RENDER_TARGET, format::RGBA8, 8, 8, [1, 1], Attr::default());
        hal.submit(&w.buf());
    }
    let texture = hal.with_world(|w| w.session(child).unwrap().windows.get(child, WIN).unwrap().val.embedded.unwrap().alias);
    let other_texture = hal.with_world(|w| w.session(other).unwrap().windows.get(other, WIN).unwrap().val.embedded.unwrap().alias);
    let mut store = EventStore::new(parent, 64, 4096);
    let pump = |store: &mut EventStore| {
        let mut buf = store.buf();
        let n = hal.pump(0, &mut buf);
        store.reader(n, buf.arena_len).filter_map(|(_, e)| match e {
            HalEvent::ChildFrame { child, window, texture, frame } => Some((child, window, texture, frame)),
            _ => None,
        }).collect::<Vec<_>>()
    };
    assert!(pump(&mut store).is_empty());
    let present = |sid, frame, kind| {
        let mut w = CmdWriter::new(sid);
        w.frame_begin(frame, FrameMeta::default());
        w.pass(Pass::default());
        w.frame_end(WIN, kind);
        hal.submit(&w.buf());
    };
    present(child, 1, target::WINDOW);
    assert_eq!(pump(&mut store), [(child, WIN, texture, 1)]);
    let mut child_store = EventStore::new(child, 64, 4096);
    let mut buf = child_store.buf();
    let n = hal.pump(0, &mut buf);
    assert_eq!(child_store.reader(n, buf.arena_len).filter(|(_, e)| matches!(e, HalEvent::FrameDone { window: WIN, frame: 1, .. })).count(), 1);
    present(child, 2, target::WINDOW);
    present(other, 1, target::WINDOW);
    present(child, 3, target::WINDOW);
    assert_eq!(pump(&mut store), [(other, WIN, other_texture, 1), (child, WIN, texture, 3)]);
    assert!(pump(&mut store).is_empty());
    present(plain, 1, target::WINDOW);
    present(child, 4, target::TEXTURE);
    assert!(pump(&mut store).is_empty(), "neither an ordinary window nor a texture target notifies the parent");
    present(child, 5, target::WINDOW);
    hal.with_world(|w| assert!(w.resize_embedded(child, WIN, [300.0, 150.0], 1.0, 0)));
    present(child, 6, target::WINDOW);
    let mut buf = store.buf();
    let n = hal.pump(0, &mut buf);
    let events: Vec<_> = store.reader(n, buf.arena_len).map(|(_, e)| e).collect();
    assert_eq!(events.len(), 2, "the old frame was replaced after the resize announcement");
    let HalEvent::ChildWindow { texture: resized, .. } = events[0] else { panic!("resize must precede the frame") };
    assert_ne!(resized, texture);
    assert_eq!(events[1], HalEvent::ChildFrame { child, window: WIN, texture: resized, frame: 6 });
    assert!(hal.close(parent));
    assert!(hal.close(plain));
}

#[test]
fn child_scale_follows_the_parent_window_and_child_geom() {
    let hal = Hal::new();
    let parent = hal.open_domain(&open_req(201, 0), Caps { privileged: true }).session;
    let mut pw = CmdWriter::new(parent);
    pw.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 800.0, 600.0], false);
    hal.submit(&pw.buf());
    hal.with_world(|w| {
        w.window_geom(parent, WIN, [0.0, 0.0, 800.0, 600.0], 2.0, window_state::RESTORE);
        // A session default must not select the scale of an already placed window.
        w.session_mut(parent).unwrap().dpi = 1.0;
    });
    pw.clear();
    pw.window_create(Id::new(1, 1), window_kind::NORMAL, Id::NONE, [0.0, 0.0, 200.0, 100.0], false);
    hal.submit(&pw.buf());
    hal.with_world(|w| w.window_geom(parent, Id::new(1, 1), [0.0, 0.0, 200.0, 100.0], 1.0, window_state::RESTORE));
    let launch = hal.with_world(|w| w.start_domain(parent, 1, "child".into(), String::new()));
    assert_eq!(hal.with_world(|w| w.session(launch.domain).unwrap().dpi), 2.0);
    let mut req = open_req(launch.app_key, parent);
    req.dpi_hint = 1.0; // Cx's default hint must not override the compositor.
    let info = hal.open_domain(&req, Caps { privileged: false });
    assert_eq!(info.session, launch.domain);
    assert_eq!(info.dpi, 2.0);
    let child = info.session;
    let mut cw = CmdWriter::new(child);
    cw.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 300.0, 150.0], false);
    hal.submit(&cw.buf());
    let check_surface = |dpi, pixels| hal.with_world(|w| {
        let s = w.session(child).unwrap();
        assert_eq!(s.dpi, dpi);
        let em = s.windows.get(child, WIN).unwrap().val.embedded.unwrap();
        assert_eq!(em.dpi, dpi);
        let t = &s.textures.get(child, em.tex).unwrap().val;
        assert_eq!((t.w, t.h), pixels);
    });
    let check_events = |dpi, pixels| {
        let mut child_store = EventStore::new(child, 64, 4096);
        let mut buf = child_store.buf();
        let n = hal.pump(0, &mut buf);
        let mut display = false;
        let mut geom = false;
        for (_, e) in child_store.reader(n, buf.arena_len) {
            match e {
                HalEvent::Display { window, dpi: d, .. } if window == WIN => { assert_eq!(d, dpi); display = true; }
                HalEvent::Geom { window, dpi: d, w, h, .. } if window == WIN => {
                    assert!(display, "Display must precede geometry");
                    assert_eq!((d, w, h), (dpi, 300.0, 150.0));
                    geom = true;
                }
                _ => {}
            }
        }
        assert!(display && geom);
        let mut parent_store = EventStore::new(parent, 64, 4096);
        let mut buf = parent_store.buf();
        let n = hal.pump(0, &mut buf);
        assert!(parent_store.reader(n, buf.arena_len).any(|(_, e)| matches!(e,
            HalEvent::ChildWindow { child: c, dpi: d, w, h, .. }
                if c == child && d == dpi && (w, h) == pixels)));
    };
    check_surface(2.0, (600, 300));
    check_events(2.0, (600, 300));
    pw.clear();
    pw.child_geom(WIN, child, [300.0, 150.0], 1.0);
    hal.submit(&pw.buf());
    check_surface(1.0, (300, 150));
    check_events(1.0, (300, 150));
    assert!(hal.close(parent));
}
