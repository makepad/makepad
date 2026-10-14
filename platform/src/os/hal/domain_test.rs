//! A real-Cx check of domains on the headless HAL: a parent Cx (privileged, the "wm") starts a
//! child Cx as a domain on its own thread, receives its window as `Event::ChildWindow`, draws it
//! through `Texture::from_hal_lent`, forwards a click, relaunches the child, lists and kills it.

use crate::{
    cx::Cx,
    cx_api::CxOsApi,
    event::*,
    makepad_math::dvec2,
    texture::Texture,
    window::WindowHandle,
};
use makepad_hal::{with_caps, with_launch, Caps, DomainHost, DomainLaunch, Hal};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Both tests use the process's one HAL: they run one after the other.
static ONE_HAL: Mutex<()> = Mutex::new(());

/// Takes the process's one HAL for a test, on the headless backend (a unit test has no window
/// system: on a machine without a display the default backend opens no window at all).
fn take_headless_hal() -> std::sync::MutexGuard<'static, ()> {
    let one = ONE_HAL.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MAKEPAD_HAL_BACKEND", "headless");
    one
}

#[derive(Default)]
struct Log {
    /// (child, window, surface, w, h) of every ChildWindow the parent saw
    child_windows: Vec<(u32, LentId, LentId, u32, u32)>,
    closed: Vec<u32>,
    states: Vec<(u32, DomainStatus)>,
    listed: Vec<Vec<(u32, DomainStatus)>>,
    /// clicks the child Cx received (x, y)
    clicks: Vec<(f64, f64)>,
    child_starts: u32,
    child_scales: Vec<f64>,
    child_surface_scales: Vec<f64>,
    relaunched: bool,
    parent_done: bool,
}

struct Host {
    log: Arc<Mutex<Log>>,
    threads: Mutex<std::collections::HashMap<u32, std::thread::JoinHandle<()>>>,
}

fn child_main(log: Arc<Mutex<Log>>) {
    log.lock().unwrap().child_starts += 1;
    let handler_log = log.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx: &mut Cx, event: &Event| match event {
        Event::Startup => {
            let _window = WindowHandle::new(cx);
        }
        Event::WindowGeomChange(e) => handler_log.lock().unwrap().child_scales.push(e.new_geom.dpi_factor),
        Event::MouseDown(e) => {
            handler_log.lock().unwrap().clicks.push((e.abs.x, e.abs.y));
            // This Cx fixture has no draw tree; present an empty frame to acknowledge the click.
            let w = &mut cx.hal.frames.w;
            w.frame_begin(1, makepad_hal::FrameMeta::default());
            w.pass(makepad_hal::Pass::default());
            w.frame_end(crate::event::hal_intake::hal_window_id(e.window_id), makepad_hal::target::WINDOW);
        }
        _ => {}
    }))));
    cx.borrow_mut().init_cx_os();
    Cx::event_loop(cx);
}

impl DomainHost for Host {
    fn start(&self, launch: DomainLaunch) -> Result<(), String> {
        let log = self.log.clone();
        let domain = launch.domain;
        let t = std::thread::spawn(move || with_launch(&launch, || child_main(log)));
        self.threads.lock().unwrap().insert(domain, t);
        Ok(())
    }

    // a quit-requested or killed child ends its own run (the HAL tells it); the host waits for that
    fn stop(&self, domain: u32, _mode: u32) {
        let t = self.threads.lock().unwrap().remove(&domain);
        if let Some(t) = t {
            t.join().unwrap();
        }
    }

    fn relaunch(&self, launch: DomainLaunch) -> Result<(), String> {
        self.start(launch)
    }
}

fn wait(what: &str, log: &Arc<Mutex<Log>>, ok: impl Fn(&Log) -> bool) {
    let t0 = Instant::now();
    while !ok(&log.lock().unwrap()) {
        assert!(t0.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Wm {
    log: Arc<Mutex<Log>>,
    texture: Option<Texture>,
    first_surface: Option<LentId>,
    killed: bool,
    child: Option<(u32, LentId)>,
    started: bool,
}

impl Wm {
    fn event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Startup => {
                let _window = WindowHandle::new(cx);
            }
            Event::WindowGeomChange(e) if !self.started => {
                self.started = true;
                // Model a real parent window on a 2x display, independently of Cx's default hint.
                Hal::global().with_world(|world| {
                    let sid = world.session_ids().into_iter().find(|sid| *sid != 0).unwrap();
                    let g = &e.new_geom;
                    world.window_geom(sid, crate::event::hal_intake::hal_window_id(e.window_id),
                        [g.position.x as f32, g.position.y as f32, g.inner_size.x as f32, g.inner_size.y as f32],
                        2.0, makepad_hal::window_state::RESTORE);
                });
                cx.hal_start_domain("child", &[]);
            }
            Event::DomainState(e) => self.log.lock().unwrap().states.push((e.domain, e.status)),
            Event::ChildWindow(e) => {
                self.log.lock().unwrap().child_windows.push((e.child, e.window, e.texture, e.width, e.height));
                self.log.lock().unwrap().child_surface_scales.push(e.dpi);
                match &self.texture {
                    Some(tex) => tex.set_hal_lent(cx, e.texture),
                    None => self.texture = Some(Texture::from_hal_lent(cx, e.texture)),
                }
                self.child = Some((e.child, e.window));
                // a window is announced again when its title or size changes: tell runs apart by surface
                if self.first_surface.is_none() {
                    self.first_surface = Some(e.texture);
                    assert_eq!(e.dpi, 2.0);
                    cx.hal_child_geom(e.child, e.window, dvec2(300.0, 150.0), 1.0);
                    cx.hal_focus_child(e.child);
                    cx.hal_forward_mouse_down(e.child, e.window, dvec2(12.0, 34.0), 1, 0);
                } else if self.first_surface != Some(e.texture) && !self.log.lock().unwrap().relaunched {
                    assert_eq!((e.dpi, e.width, e.height), (1.0, 300, 150));
                    self.first_surface = Some(e.texture);
                } else if self.first_surface != Some(e.texture) && !self.killed {
                    // the relaunched child's window: list, then kill
                    self.killed = true;
                    cx.hal_list_domains();
                    cx.hal_stop_domain(e.child, true);
                }
            }
            Event::ChildWindowClosed(e) => self.log.lock().unwrap().closed.push(e.child),
            Event::DomainList(e) => {
                let mut l = self.log.lock().unwrap();
                l.listed.push(e.domains.iter().map(|d| (d.domain, d.status)).collect());
                l.parent_done = true;
                drop(l);
                cx.quit();
            }
            Event::ChildFrame(e) => {
                // The child presents after handling the click; no timer is needed to observe it.
                assert_eq!(self.child, Some((e.child, e.window)));
                assert_eq!(self.first_surface, Some(e.texture));
                assert_eq!(e.frame, 1);
                let mut l = self.log.lock().unwrap();
                if let (false, false, Some((child, _))) = (l.relaunched, l.clicks.is_empty(), self.child) {
                    l.relaunched = true;
                    drop(l);
                    cx.hal_relaunch_domain(child);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn a_parent_cx_embeds_a_child_cx() {
    let _one = take_headless_hal();
    let log = Arc::new(Mutex::new(Log::default()));
    let host = Arc::new(Host { log: log.clone(), threads: Mutex::new(Default::default()) });
    Hal::global().set_domain_host(host.clone());

    let wm_log = log.clone();
    let parent = std::thread::spawn(move || {
        with_caps(Caps { privileged: true }, || {
            let mut wm = Wm { log: wm_log, texture: None, first_surface: None, killed: false, child: None, started: false };
            let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx: &mut Cx, event: &Event| wm.event(cx, event)))));
            cx.borrow_mut().init_cx_os();
            Cx::event_loop(cx);
        })
    });

    wait("the child's window", &log, |l| !l.child_windows.is_empty());
    {
        let l = log.lock().unwrap();
        let (child, _, surface, w, h) = l.child_windows[0];
        assert!(surface.index >= makepad_hal::LENT_BASE, "the surface is a HAL-lent id");
        assert!(w > 0 && h > 0);
        assert!(l.states.iter().any(|s| s.0 == child && s.1 == DomainStatus::Running), "{:?}", l.states);
    }
    wait("the parent to finish", &log, |l| l.parent_done);
    parent.join().unwrap();
    let l = log.lock().unwrap();
    assert_eq!(l.clicks, [(12.0, 34.0)], "the click reached the child, in its own window space");
    assert!(l.child_scales.contains(&2.0) && l.child_scales.contains(&1.0), "{:?}", l.child_scales);
    let mut scales = l.child_surface_scales.clone();
    scales.dedup();
    assert_eq!(&scales[..2], &[2.0, 1.0]);
    assert_eq!(l.child_starts, 2, "started once, relaunched once");
    let mut surfaces: Vec<LentId> = l.child_windows.iter().map(|w| w.2).collect();
    surfaces.dedup();
    assert!(surfaces.len() >= 2 && surfaces[0] != *surfaces.last().unwrap(), "a new surface after the relaunch");
    assert!(l.closed.contains(&l.child_windows[0].0), "the first window closed when the child was freed");
    assert_eq!(l.listed.len(), 1);
    assert_eq!(l.listed[0].len(), 1);
    drop(l);
    // the killed child hears that its session ended and ends its run; the HAL carries out the kill
    // after the parent's last batch, so the stop may still be joining it
    let t0 = Instant::now();
    while !host.threads.lock().unwrap().is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(30), "the killed child's run ended and was joined by stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    Hal::global().with_world(|w| assert_eq!(w.session_ids(), [0], "the parent's Quit and the kill freed every session"));
}

/// What the deleted macOS backend did around every event, on the HAL client: a panic in an
/// event handler is contained (the Cx is put back in order and redraws), and futures spawned
/// through `cx.spawner()` run.
#[test]
fn a_panic_is_contained_and_spawned_futures_run() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let _one = take_headless_hal();
    static SPAWNED_RAN: AtomicU32 = AtomicU32::new(0);
    static AFTER_PANIC: AtomicU32 = AtomicU32::new(0);
    let thread = std::thread::spawn(|| {
        let mut panicked = false;
        let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx: &mut Cx, event: &Event| match event {
            Event::Startup => {
                let _window = WindowHandle::new(cx);
                cx.spawner().spawn(async { SPAWNED_RAN.fetch_add(1, Ordering::SeqCst); }).unwrap();
                cx.redraw_all();
            }
            Event::Draw(_) if !panicked => {
                panicked = true;
                panic!("a handler panic the HAL client must contain");
            }
            Event::Draw(_) => {
                AFTER_PANIC.fetch_add(1, Ordering::SeqCst);
                cx.quit();
            }
            _ => {}
        }))));
        cx.borrow_mut().init_cx_os();
        Cx::event_loop(cx);
    });
    let t0 = Instant::now();
    while AFTER_PANIC.load(Ordering::SeqCst) == 0 && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(SPAWNED_RAN.load(Ordering::SeqCst), 1, "the spawned future ran");
    assert!(AFTER_PANIC.load(Ordering::SeqCst) >= 1, "the app went on drawing after the panic");
    thread.join().unwrap();
}

/// What the WM's desk tells a covered domain tile (`HostedVisible` as an app message to the
/// child's window): the child holds its frames -- no next-frame callbacks, no draws -- while its
/// timers and input go on, and resumes when shown again. The same rule as a hosted process child.
#[test]
fn a_covered_child_cx_holds_its_frames() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    static STARTS: AtomicU32 = AtomicU32::new(0);
    /// 0 shown, 1 covered, 2 done: what the test asks the parent to tell the child
    static WANT: AtomicU32 = AtomicU32::new(0);
    static TOLD: AtomicU32 = AtomicU32::new(u32::MAX);
    /// the test asks the parent to relaunch the (covered) child
    static RELAUNCH: AtomicU32 = AtomicU32::new(0);

    fn frame_child() {
        STARTS.fetch_add(1, Ordering::SeqCst);
        let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx: &mut Cx, event: &Event| match event {
            Event::Startup => {
                let _window = WindowHandle::new(cx);
                let _ = cx.new_next_frame();
            }
            // a game: every frame asks for the next
            Event::NextFrame(_) => {
                FRAMES.fetch_add(1, Ordering::SeqCst);
                let _ = cx.new_next_frame();
            }
            _ => {}
        }))));
        cx.borrow_mut().init_cx_os();
        Cx::event_loop(cx);
    }
    struct FrameHost { threads: Mutex<std::collections::HashMap<u32, std::thread::JoinHandle<()>>> }
    impl DomainHost for FrameHost {
        fn start(&self, launch: DomainLaunch) -> Result<(), String> {
            let domain = launch.domain;
            let t = std::thread::spawn(move || with_launch(&launch, frame_child));
            self.threads.lock().unwrap().insert(domain, t);
            Ok(())
        }
        fn stop(&self, domain: u32, _mode: u32) {
            let t = self.threads.lock().unwrap().remove(&domain);
            if let Some(t) = t { t.join().unwrap(); }
        }
        fn relaunch(&self, launch: DomainLaunch) -> Result<(), String> { self.start(launch) }
    }

    let _one = take_headless_hal();
    let host = Arc::new(FrameHost { threads: Mutex::new(Default::default()) });
    Hal::global().set_domain_host(host.clone());
    let parent = std::thread::spawn(move || {
        with_caps(Caps { privileged: true }, || {
            let mut child: Option<(u32, LentId)> = None;
            // the covered state reached the child's current run (the desk tile's `hidden_told`)
            let mut told_run = false;
            let mut started = false;
            let mut poll = Timer::empty();
            let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx: &mut Cx, event: &Event| match event {
                Event::Startup => {
                    let _window = WindowHandle::new(cx);
                    poll = cx.start_interval(0.02);
                }
                Event::WindowGeomChange(_) if !started => {
                    started = true;
                    cx.hal_start_domain("child", &[]);
                }
                Event::ChildWindow(e) => child = Some((e.child, e.window)),
                Event::Timer(e) if poll.is_timer(e).is_some() => {
                    let Some((domain, window)) = child else { return };
                    let want = WANT.load(Ordering::SeqCst);
                    if RELAUNCH.swap(0, Ordering::SeqCst) == 1 {
                        told_run = false;
                        child = None;
                        cx.hal_relaunch_domain(domain);
                    } else if want == 2 {
                        cx.hal_stop_domain(domain, true);
                        cx.quit();
                    // as the desk's domain tile does: a covered tile's new (relaunched) window is told again
                    } else if TOLD.load(Ordering::SeqCst) != want || (want == 1 && !told_run) {
                        told_run = want == 1;
                        let json = makepad_studio_protocol::HostedVisible { visible: want == 0 }.to_json();
                        let data = makepad_hal::Range { off: 0, len: json.len() as u32 };
                        cx.hal_forward(domain, window, &makepad_hal::RawEvent::app_message(makepad_hal::Id::NONE, data), json.as_bytes());
                        TOLD.store(want, Ordering::SeqCst);
                    }
                }
                _ => {}
            }))));
            cx.borrow_mut().init_cx_os();
            Cx::event_loop(cx);
        })
    });
    let wait_for = |what: &str, ok: &dyn Fn() -> bool| {
        let t0 = Instant::now();
        while !ok() {
            assert!(t0.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait_for("the shown child's frames", &|| FRAMES.load(Ordering::SeqCst) >= 10);
    WANT.store(1, Ordering::SeqCst);
    wait_for("the parent to say covered", &|| TOLD.load(Ordering::SeqCst) == 1);
    // the message is in flight for a moment; then nothing more
    std::thread::sleep(Duration::from_millis(200));
    let held = FRAMES.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(500));
    let after = FRAMES.load(Ordering::SeqCst);
    assert!(after - held <= 1, "a covered child drew {} frames in 0.5 s", after - held);
    // relaunched while covered: the new run is told again and holds too
    RELAUNCH.store(1, Ordering::SeqCst);
    wait_for("the relaunched child", &|| STARTS.load(Ordering::SeqCst) == 2);
    std::thread::sleep(Duration::from_millis(500));
    let held = FRAMES.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(500));
    let after = FRAMES.load(Ordering::SeqCst);
    assert!(after - held <= 1, "a covered relaunched child drew {} frames in 0.5 s", after - held);
    WANT.store(0, Ordering::SeqCst);
    wait_for("the child to resume when shown", &|| FRAMES.load(Ordering::SeqCst) >= after + 10);
    WANT.store(2, Ordering::SeqCst);
    parent.join().unwrap();
    // the kill is carried out by the HAL after the parent's last batch: the next test starts clean
    wait_for("the child's run to end", &|| host.threads.lock().unwrap().is_empty());
    wait_for("every session freed", &|| Hal::global().with_world(|w| w.session_ids() == [0]));
}
