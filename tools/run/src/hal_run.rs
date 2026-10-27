//! The permanent window of `makepad run`. Session 0 presents the boot document on
//! any native HAL backend; compilation and app domains are guests.
//! Only ending the run closes the physical window.
use crate::boot_progress::{History, Progress};
use makepad_hal::{domain_stop, open_flags, session::HOST, CmdWriter, Hal, Id, OpenReq};
use makepad_studio_protocol::KeyCode;
use relax_boot::{
    Action, Boot as HostWindow, Document, Environment, Event as BootEvent, Icon, LayerWant, RingAlert, RingWant,
};
use relax_compiler::{
    diag::Diagnostic,
    domain::{AppSpec, DomainState},
    postframe::Activity,
    run_state::{self, Mode, ParkedPanic, Phase, State},
};
use relax_session::sync::{Condvar, Mutex, MutexGuard};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

struct Run {
    window: Id,
    spec: Option<AppSpec>,
    domain: u32,
    pending: bool,
    stopping: bool,
    relaunch: Option<Instant>,
    baseline: u64,
    initial_failed: bool,
    retry: bool,
}

#[derive(Clone)]
enum Update {
    Compiler(run_state::Event),
    Domain(DomainState),
    Begin { warm: bool },
    Failure(String),
    Message(String),
    SourcesChanged,
    Document(Document),
    Environment(Environment),
}

enum Work {
    Save(PathBuf, String),
    OpenFile {
        file: String,
        line: u32,
        col: u32,
    },
    Resume {
        panic: String,
        choice: String,
        value: String,
    },
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);
static RETRY: Condvar = Condvar::new();
static UPDATES: Mutex<VecDeque<Update>> = Mutex::new(VecDeque::new());
static PUMP: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static QUIT: AtomicBool = AtomicBool::new(false);
static PANICS_CHANGED: AtomicBool = AtomicBool::new(false);
static EXIT: AtomicI32 = AtomicI32::new(0);
static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);
/// The UI thread began a JIT wait since the window last looked (postframe's hook): the window
/// wakes, and shows the UI block on the JIT ring once the waiting is sustained (BLOCK_AFTER).
static UI_WAIT_ARMED: AtomicBool = AtomicBool::new(false);
/// A true UI block: one episode of UI-thread JIT waits this long (postframe::BLOCK_SHOW_NS: one
/// long lazy compile, or a load's thousands of short ones; never a short hiccup). The JIT ring
/// shows only past one.
const BLOCK_AFTER: Duration = Duration::from_nanos(relax_compiler::postframe::BLOCK_SHOW_NS);
/// The UI has not been blocked on the JIT for this long: the warm-up is over.
const READY_QUIET_NS: u64 = 300_000_000;

fn ui_wait_began() {
    if !UI_WAIT_ARMED.swap(true, Ordering::AcqRel) {
        Hal::global().wake_session(HOST);
    }
}

/// The JIT ring shows only past a real wait: a UI block (BLOCK_AFTER), or makepad rebuilding
/// itself; first compiles and background tier-up alone never show it (JITLOOP-B). Once shown it
/// stays while the JIT works, and settles (fades out) once nothing has been built or run for
/// this long (queued builds nobody works on are not work).
const RING_SETTLE_AFTER: Duration = Duration::from_millis(500);
/// the build rate's smoothing (s)
const RATE_TAU: f32 = 0.5;
/// the run looks at the JIT's queues at most this often (the ring eases between looks)
const WATCH_EVERY: Duration = Duration::from_millis(100);
/// how often the run looks at the JIT while the app runs and no ring shows (new work is seen
/// within this; a UI wait wakes it at once)
const RING_LOOK: Duration = Duration::from_millis(500);

/// Whether this run shows the JIT ring: `MAKEPAD_RUN_SETTLE_HUD=0` never, `=1` always, else not
/// in hidden or benchmark runs (MAKEPAD_HIDE_WINDOWS, MAKEPAD_RUN_O3_PROGRESS,
/// MAKEPAD_RUN_BOOT_DUMP).
fn ring_on() -> bool {
    match std::env::var("MAKEPAD_RUN_SETTLE_HUD").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => {
            std::env::var("MAKEPAD_HIDE_WINDOWS").as_deref() != Ok("1")
                && std::env::var_os("MAKEPAD_RUN_O3_PROGRESS").is_none()
                && std::env::var_os("MAKEPAD_RUN_BOOT_DUMP").is_none()
        }
    }
}

/// What the run sees of the JIT for its ring: builds running now and finished since the last
/// look (the work rate), the queues, the UI thread's JIT duty cycle, a true UI block. Busy means
/// something was built or is building, never only that a queue is non-empty. Atomics and two
/// queue locks a look; the app's threads never wait for it.
struct Watch {
    shown: bool,
    busy_since: Option<Instant>,
    /// the last time anything was running or finished
    worked: Option<Instant>,
    /// the last look at the JIT (at most every WATCH_EVERY), what it showed then
    looked: Instant,
    a: Activity,
    /// the queues' builds finished when this busy spell began
    done_from: Option<u64>,
    /// a program thread's first compiles finished since the last look
    demand: bool,
    /// builds finished a second (smoothed), and of the last look's, warm links only
    rate: f32,
    linking: bool,
    duty: f32,
    /// the UI block shows (for the trace)
    blocked: bool,
    trace: bool,
    traced: Option<Instant>,
}

impl Watch {
    fn new(trace: bool) -> Self {
        Self {
            shown: false,
            busy_since: None,
            worked: None,
            looked: Instant::now(),
            a: relax_compiler::postframe::activity(),
            done_from: None,
            demand: false,
            rate: 0.0,
            linking: false,
            duty: 0.0,
            blocked: false,
            trace,
            traced: None,
        }
    }

    /// The ring's wish now (`None`: the JIT is settled). `clock`: the frame dump's clock (ms),
    /// for the trace.
    fn look(&mut self, clock: Option<u128>) -> Option<RingWant> {
        use relax_compiler::postframe::{activity, ui_blocked_ns};
        let now = Instant::now();
        let dt = now.duration_since(self.looked).as_secs_f32();
        if dt >= WATCH_EVERY.as_secs_f32() {
            let a = activity();
            let waited = a.ui_wait_ns.saturating_sub(self.a.ui_wait_ns) as f32 / 1e9;
            self.duty = (waited / dt).clamp(0.0, 1.0);
            let built = a.finished().saturating_sub(self.a.finished());
            self.linking = built > 0 && a.linked > self.a.linked && a.finished() - a.linked == self.a.finished() - self.a.linked;
            self.demand = a.demand() > self.a.demand();
            let k = 1.0 - (-dt / RATE_TAU).exp();
            self.rate += (built as f32 / dt - self.rate) * k;
            if built > 0 || a.running() || self.duty > 0.02 {
                self.worked = Some(now);
            }
            self.a = a;
            self.looked = now;
        }
        let a = self.a;
        let stalled = Duration::from_nanos(ui_blocked_ns());
        let block = stalled >= BLOCK_AFTER;
        // a program thread compiles lazily: the UI thread's JIT duty is up, or other threads'
        // first compiles finished
        let lazy = self.duty > 0.02 || self.demand;
        use relax_compiler::reload::host::{status, Phase};
        let rebuild = match status().0 {
            Phase::Building => 1,
            Phase::Restarting => 2,
            Phase::Idle | Phase::Failed => 0,
        };
        if block || rebuild != 0 {
            self.worked = Some(now);
        }
        let busy = self.worked.is_some_and(|t| now.duration_since(t) < RING_SETTLE_AFTER);
        if busy {
            self.done_from.get_or_insert(a.queue_done());
            self.busy_since.get_or_insert(now);
            self.shown |= block || rebuild != 0;
        } else {
            self.shown = false;
            self.busy_since = None;
            self.done_from = None;
            self.rate = 0.0;
        }
        if self.trace && block != self.blocked {
            if block {
                eprintln!("makepad-run: jit ring: UI block (the UI waited {:.0} ms on the JIT)", stalled.as_secs_f64() * 1000.0);
            } else {
                eprintln!("makepad-run: jit ring: UI block over");
            }
        }
        self.blocked = block;
        let want = RingWant {
            done: a.queue_done().saturating_sub(self.done_from.unwrap_or(a.queue_done())),
            pending: a.pending(),
            ahead: a.ahead,
            o2: a.o2,
            o3: a.o3 + a.o3_running,
            lazy,
            block,
            settling: a.settling,
            linking: self.linking,
            rate: self.rate,
            busy: self.busy_since.map_or(Duration::ZERO, |t| now.duration_since(t)),
            rebuild,
        };
        // the trace: what the ring was told, at most 4 times a second while it shows
        if self.trace && (self.shown || self.traced.is_some()) && self.traced.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(250)) {
            let at = clock.map_or(String::new(), |ms| format!(" @{ms}"));
            let state = if !self.shown {
                "settled"
            } else if want.ahead > 0 || lazy || block {
                "compiling"
            } else {
                "tier-up"
            };
            eprintln!(
                "makepad-run: jit ring{at}: {state} ({}, ui duty {:.2}){}",
                relax_boot::ring_summary(Some(want), None),
                self.duty,
                if block { " UI BLOCK" } else { "" }
            );
            self.traced = self.shown.then_some(now);
        }
        self.shown.then_some(want)
    }

    /// How soon to look again while the UI thread may be heading into a block (its JIT wait began).
    fn block_wake(&self) -> Option<Duration> {
        use relax_compiler::postframe::{ui_blocked_ns, ui_idle_ns};
        if !UI_WAIT_ARMED.load(Ordering::Acquire) {
            return None;
        }
        // once the UI thread is not waiting the window disarms (the next wait's hook wakes it)
        if ui_idle_ns() >= BLOCK_AFTER.as_nanos() as u64 {
            UI_WAIT_ARMED.store(false, Ordering::Release);
            return None;
        }
        let stalled = Duration::from_nanos(ui_blocked_ns());
        Some(BLOCK_AFTER.saturating_sub(stalled).max(Duration::from_millis(20)))
    }
}

/// A rebuild of makepad failed (its dot shows in the ring until the next rebuild).
fn rebuild_failed() -> bool {
    use relax_compiler::reload::host::{status, Phase};
    status().0 == Phase::Failed
}

/// Where the window layer over the app is in its sequence.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    /// no layer (the JIT ring may show)
    Running,
    /// the boot field, the app's first frames going under it
    Boot,
    /// after the first present, until the app is ready
    WarmUp,
    /// ready: the sharp app comes in, the galaxy goes
    Lift,
}

pub fn set_environment(environment: Environment) {
    *ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(environment);
    if state().is_some() {
        post(Update::Environment(environment));
    }
}

fn state() -> MutexGuard<'static, Option<Run>> {
    RUN.lock().unwrap_or_else(|e| e.into_inner())
}

impl Update {
    /// A copy built in the kept heap (the caller is in the kept phase). An update crosses from
    /// the compiler's threads to the window pump, which holds its parts for the whole run: none
    /// of it may live in the compiler arena, whose generation the settle swap destroys.
    fn kept(self) -> Update {
        match self {
            Update::Compiler(run_state::Event::Patch { domain, applied, records }) => Update::Compiler(run_state::Event::Patch {
                domain,
                applied,
                records: records.iter().cloned().collect::<Vec<_>>().into(),
            }),
            other => other.clone(),
        }
    }
}

fn post(update: Update) {
    let _kept = relax_compiler::zone_alloc::kept();
    let update = update.kept();
    UPDATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(update);
    Hal::global().wake_session(HOST);
}

/// MAKEPAD_RUN_QUIT_TRACE=1: each step of the run's end on stderr (thread, ms since the quit
/// began): where a quit that never exits stops.
fn quit_trace(step: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_QUIT_TRACE").is_some()) {
        return;
    }
    let ms = T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3;
    eprintln!("makepad-run quit: {step} (+{ms:.0} ms, thread `{}`)", std::thread::current().name().unwrap_or("?"));
}

fn panic_changed() {
    PANICS_CHANGED.store(true, Ordering::Release);
    Hal::global().wake_session(HOST);
}

pub fn run(mut compile: Box<dyn FnMut() -> i32 + Send>) -> i32 {
    // Java owns Android's UI loop; this boot thread compiles and runs domain 0.
    if cfg!(target_os = "android") {
        // the APK's boot app is the window manager when the host was built `--privileged`
        let privileged = relax_runner::privileged();
        return makepad_hal::with_caps(makepad_hal::Caps { privileged }, || compile());
    }
    makepad_hal::expect_main_loop();
    // the handle (thread record, result packet) is joined at the end of the run, after the settle swap
    // may have destroyed the compiler generation this frame runs in: it lives in the kept heap
    let thread = relax_compiler::zone_alloc::kept_with(move || std::thread::Builder::new()
        .name("run-compile".into())
        .stack_size(8 << 20)
        .spawn(move || {
            // this thread boots and, for an app without the HAL, runs its main (what the loader's
            // main thread did): a compiler phase, not a swap-quiesced compiler worker
            let _compiler = relax_compiler::zone_alloc::compiler();
            loop {
                relax_compiler::lifeline::clear_panic_at();
                // a panic in the compiler (exit 101): its message and position go on the load
                // screen, not only into a log nobody on the box sees
                let (code, panicked) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile())) {
                    Ok(code) => (code, None),
                    Err(p) => (101, Some(relax_compiler::lifeline::panic_line(&*p))),
                };
                let mut guard = state();
                let Some(run) = guard.as_mut() else {
                    EXIT.store(code, Ordering::Release);
                    QUIT.store(true, Ordering::Release);
                    makepad_hal::stop_main_loop();
                    return;
                };
                if code == 0 && run.spec.is_some() {
                    return;
                }
                if QUIT.load(Ordering::Acquire) {
                    return;
                }
                EXIT.store(code, Ordering::Release);
                if run.window.is_none() {
                    // No native window backend: there is no run UI to retry from.
                    QUIT.store(true, Ordering::Release);
                    Hal::global().wake_session(HOST);
                    if PUMP.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                        makepad_hal::stop_main_loop();
                    }
                    return;
                }
                {
                    // With a unit already installed, retries go through domain relaunch.
                    run.initial_failed = run.spec.is_none();
                    post(Update::Failure(match &panicked {
                        Some(line) => format!("Compilation failed: the compiler panicked: {line}"),
                        None => format!("Compilation failed (exit {code})"),
                    }));
                    if run.spec.is_some() {
                        return;
                    }
                    while !QUIT.load(Ordering::Acquire) && !guard.as_ref().is_some_and(|r| r.retry)
                    {
                        guard = RETRY.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                    if QUIT.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(run) = guard.as_mut() {
                        run.retry = false;
                        run.initial_failed = false;
                    }
                    drop(guard);
                    post(Update::Begin { warm: false });
                }
            }
        })
        .expect("the compiler thread"));
    // the app is a domain of this host: its platform installs no termination handler, so the
    // host answers SIGINT/SIGTERM/SIGHUP (quit_signal.rs): the cache's quit write, then the run
    // ends as closing its window ends it. Its worker thread (handle, name, result packet, closure)
    // waits for the whole run: it lives in the kept heap, not in the compiler generation the
    // settle swap destroys (it then faulted on its own packet as the signal woke it)
    relax_compiler::zone_alloc::kept_with(|| crate::quit_signal::install(|| {
        relax_compiler::run::quit_requested();
        QUIT.store(true, Ordering::Release);
        RETRY.notify_all();
        Hal::global().wake_session(HOST);
    }));
    {
        let _kept = relax_compiler::zone_alloc::kept();
        makepad_hal::run_main_loop();
    }
    quit_trace("the main loop returned");
    QUIT.store(true, Ordering::Release);
    RETRY.notify_all();
    Hal::global().wake_session(HOST);
    let mut code = None;
    if thread.join().is_err() {
        quit_trace("the compile thread panicked");
        code = Some(101);
    }
    let pump = PUMP.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(pump) = pump {
        quit_trace("the compile thread joined");
        if pump.join().is_err() {
            code = code.or(Some(101));
        }
    }
    // Every way the run ends comes here (window close, a signal's quit, a failed boot, no window
    // backend) and the process exits right after: the HAL's driver threads stop and are joined
    // first. exit() runs the driver's exit handlers (NVIDIA's EGL teardown), and a Vulkan worker
    // still in vkQueueSubmit faulted inside it (a SIGSEGV at a SIGTERM mid-frame).
    quit_trace("before_exit");
    Hal::global().before_exit();
    quit_trace("the run ends");
    code.unwrap_or_else(|| EXIT.load(Ordering::Acquire))
}

fn document(package_dir: &str) -> Result<Document, String> {
    let manifest = format!("{package_dir}/Cargo.toml");
    let text =
        relax_compiler::vfs::read_to_string(&manifest).map_err(|e| format!("{manifest}: {e}"))?;
    let manifest = relax_compiler::toml::parse_toml(&text).map_err(|e| e.to_string())?;
    let Some(value) = manifest.get_path(&["package", "metadata", "makepad", "bootloader"]) else {
        // the binary's own document: embedded, never the source tree's copy
        return Ok(Document::bundled());
    };
    let relative = value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("package.metadata.makepad.bootloader must name a Splash document")?;
    let path = PathBuf::from(package_dir).join(relative);
    let source = relax_compiler::vfs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Document {
        path,
        source: source.into(),
        live: true,
    })
}

fn identity(package: &str, package_dir: &str) -> (String, Option<Icon>) {
    let manifest = relax_compiler::vfs::read_to_string(format!("{package_dir}/Cargo.toml"))
        .ok()
        .and_then(|text| relax_compiler::toml::parse_toml(&text).ok());
    let metadata = |path: &[&str]| {
        manifest
            .as_ref()?
            .get_path(path)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let name = metadata(&["package", "metadata", "makepad", "android", "label"])
        .or_else(|| metadata(&["package", "metadata", "makepad", "name"]))
        .or_else(|| metadata(&["package", "metadata", "makepad", "desktop", "name"]))
        .unwrap_or_else(|| package.to_owned());
    if std::env::var_os("MAKEPAD_NO_ICON").is_some() {
        return (name, None);
    }
    let root = PathBuf::from(package_dir);
    let mut paths = Vec::new();
    for path in [
        &["package", "metadata", "makepad", "icon"][..],
        &["package", "metadata", "makepad", "desktop", "icon"][..],
    ] {
        if let Some(path) = metadata(path) {
            paths.push(root.join(path));
        }
    }
    // The Android packager's launcher resources, then its existing desktop PNG
    // overrides/resources. A missing/unsupported icon never blocks boot.
    for density in ["xxxhdpi", "xxhdpi", "xhdpi", "hdpi", "mdpi"] {
        paths.push(root.join(format!(
            "resources/android/res/mipmap-{density}/ic_launcher.png"
        )));
    }
    for size in [256, 512, 128, 64, 1024, 32] {
        if let Some(path) = std::env::var_os(format!("MAKEPAD_APP_ICON_{size}")) {
            paths.push(PathBuf::from(path));
        }
        paths.push(root.join(format!("resources/icon_{size}.png")));
    }
    let icon = paths.into_iter().find_map(|path| {
        let bytes = relax_compiler::vfs::read(&path).ok()?;
        Icon::decode(&bytes)
    });
    (name, icon)
}

/// Called after graph resolution, before build scripts or application sources.
pub fn open(package: &str, package_dir: &str) -> Result<(), String> {
    // JNI captures the Activity before compilation; the app opens the device when ready.
    if cfg!(target_os = "android") {
        return Ok(());
    }
    if state().is_some() {
        return Ok(());
    }
    let _kept = relax_compiler::zone_alloc::kept();
    let hal = Hal::global();
    let remote = relax_compiler::os::proc::args_bytes()
        .iter()
        .any(|a| a == b"--remote")
        || std::env::var_os("MAKEPAD_REMOTE").is_some();
    let flags = open_flags::RETAIN_WINDOWS
        | open_flags::HIDDEN_UNTIL_PRESENT
        | if remote { open_flags::NO_FOCUS } else { 0 };
    // Inside a makepad window manager the run is one of its windows: it is named by the app.
    #[cfg(target_os = "linux")]
    makepad_hal::wm_socket::set_client_name(package);
    // MAKEPAD_RUN_DPI: the scale of a hidden window (tests of how the boot window looks on a HiDPI screen)
    let dpi_hint = std::env::var("MAKEPAD_RUN_DPI").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let info = hal.open_host(&OpenReq::from_env(0, flags, dpi_hint));
    if info.backend == makepad_hal::backend_id::NONE {
        return Err("could not open the host HAL session".into());
    }
    let key = package
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ *b as u64).wrapping_mul(0x100000001b3)
        });
    let state_path = relax_compiler::dirs::relax_dir()
        .join("windows")
        .join(format!("{key:016x}.geom"));
    let geometry = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| {
            let values: Vec<f32> = s
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            let g: [f32; 4] = values.try_into().ok()?;
            (g.iter().all(|v| v.is_finite()) && g[2] > 0.0 && g[3] > 0.0).then_some(g)
        })
        .unwrap_or([f32::NAN, f32::NAN, 1280.0, 800.0]);
    // `MAKEPAD_WINDOW=WxH@scale` sizes the app's first window, which adopts this one: it is made
    // at that size from the start, exactly, and the size is not kept for the next run
    let launch = launch_window(info.dpi);
    let geometry = launch.unwrap_or(geometry);
    let history = History::load(
        relax_compiler::dirs::cache_dir()
            .join("boot")
            .join(format!("{key:016x}.timings")),
    );
    let document = document(package_dir).unwrap_or_else(|e| {
        post(Update::Message(e));
        Document::bundled()
    });
    *state() = Some(Run {
        window: Id::NONE,
        spec: None,
        domain: 0,
        pending: false,
        stopping: false,
        relaunch: None,
        baseline: 0,
        initial_failed: false,
        retry: false,
    });
    run_state::set_observer(|event| post(Update::Compiler(event)));
    relax_compiler::tui::track_for_host();
    relax_compiler::inspect::events::set_run_state_observer(panic_changed);
    relax_compiler::inspect::events::run_state_available(
        info.dialect != makepad_hal::dialect::NONE,
    );
    let (app, icon) = identity(package, package_dir);
    let initial = State {
        app,
        domain: "Compiling".into(),
        ..State::default()
    };
    let package_dir = PathBuf::from(package_dir);
    let watched_document = document.clone();
    let (ready, first) = std::sync::mpsc::sync_channel(1);
    *PUMP.lock().unwrap_or_else(|e| e.into_inner()) = Some(
        std::thread::Builder::new()
            .name("run-window".into())
            .stack_size(8 << 20)
            .spawn(move || {
                struct StopLoop;
                impl Drop for StopLoop {
                    fn drop(&mut self) {
                        quit_trace("the window pump ends: stop_main_loop");
                        makepad_hal::stop_main_loop();
                    }
                }
                let _stop = StopLoop;
                let _kept = relax_compiler::zone_alloc::kept();
                let mut host = match HostWindow::new(info, document, initial.clone(), geometry, launch.is_some()) {
                    Ok(host) => host,
                    Err(error) => {
                        let _ = ready.try_send(Err(error));
                        return;
                    }
                };
                let environment = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_default();
                host.environment(environment);
                if let Some(icon) = icon { host.icon(icon); }
                pump(
                    host,
                    initial,
                    history,
                    launch.is_none().then_some(state_path),
                    package_dir,
                    watched_document,
                    ready,
                );
            })
            .map_err(|e| e.to_string())?,
    );
    first.recv().map_err(|e| e.to_string())?
}

/// The boot window's geometry for `MAKEPAD_WINDOW=WxH@scale`: the native size that gives the app's
/// window a `W`x`H` layout at `scale` on a display running at `dpi` (the app's `--window` does the
/// same for a native launch: `makepad_platform::remote::app_clock::LaunchWindow::native_size`).
fn launch_window(dpi: f32) -> Option<[f32; 4]> {
    let value = std::env::var("MAKEPAD_WINDOW").ok()?;
    let (size, scale) = value.split_once('@')?;
    let (w, h) = size.split_once('x')?;
    let [w, h, scale] = [w, h, scale].map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0));
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi } else { 1.0 };
    let scale = scale?;
    Some([f32::NAN, f32::NAN, w? * scale / dpi, h? * scale / dpi])
}

pub fn start(spec: AppSpec) -> Result<(), String> {
    let _kept = relax_compiler::zone_alloc::kept();
    // the spec is built by the boot in the compiler phase; the run state keeps it for the whole run
    let spec = spec.clone();
    if QUIT.load(Ordering::Acquire) {
        return Err("run closed during compilation".into());
    }
    let launch =
        Hal::global().with_world(|w| w.start_domain(HOST, 0, spec.package.clone(), String::new()));
    {
        let mut state = state();
        let r = state.as_mut().ok_or("no host window")?;
        r.domain = launch.domain;
        r.spec = Some(spec);
    }
    relax_compiler::run::set_app_domain(launch.domain);
    crate::hal_host::start(launch)
}

pub fn spec(id: u32) -> Option<AppSpec> {
    state()
        .as_ref()
        .filter(|r| r.domain == id)
        .and_then(|r| r.spec.clone())
}

pub fn relaunch() -> Result<(), String> {
    let _kept = relax_compiler::zone_alloc::kept();
    let mut guard = state();
    let r = guard
        .as_mut()
        .filter(|r| r.domain != 0)
        .ok_or("the app has not started yet")?;
    if QUIT.load(Ordering::Acquire) {
        return Err("the run is closing".into());
    }
    if r.relaunch.is_none() {
        r.relaunch = Some(Instant::now());
        r.pending = true;
        r.stopping = true;
        post(Update::Begin { warm: true });
    }
    drop(guard);
    Hal::global().wake_session(HOST);
    Ok(())
}

pub fn restarting(id: u32) {
    let window = state()
        .as_ref()
        .filter(|r| r.domain == id)
        .map(|r| r.window);
    let Some(window) = window else { return };
    let baseline = Hal::global().with_world(|w| {
        w.session(HOST)
            .and_then(|s| s.windows.get(HOST, window).ok())
            .map_or(0, |w| w.val.presentations)
    });
    if let Some(r) = state().as_mut().filter(|r| r.domain == id) {
        if r.relaunch.is_none() {
            r.relaunch = Some(Instant::now());
            post(Update::Begin { warm: true });
        }
        r.stopping = false;
        r.baseline = baseline;
    }
}

pub fn observe(id: u32, next: &DomainState) {
    let guard = state();
    if let Some(r) = guard.as_ref().filter(|r| r.domain == id) {
        if !r.stopping || matches!(next, DomainState::Leaked { .. }) {
            post(Update::Domain(next.clone()));
        }
    }
}

fn retry() -> Result<(), String> {
    {
        let mut guard = state();
        let r = guard.as_mut().ok_or("no run window")?;
        if r.initial_failed {
            r.retry = true;
            RETRY.notify_one();
            return Ok(());
        }
    }
    relaunch()
}

fn work(rx: std::sync::mpsc::Receiver<Work>, package_dir: PathBuf, document: Document) {
    let _kept = relax_compiler::zone_alloc::kept();
    // only a package's own document is watched (the bundled one is the binary's)
    let document_path = if document.live { document.path.clone() } else { PathBuf::new() };
    let mut roots = Vec::new();
    if package_dir.is_dir() {
        roots.push(makepad_filesystem_watcher::WatchRoot {
            mount: "run".into(),
            path: package_dir.clone(),
        });
    }
    if let Some(parent) = document_path
        .parent()
        .filter(|p| p.is_dir() && !document_path.starts_with(&package_dir))
    {
        roots.push(makepad_filesystem_watcher::WatchRoot {
            mount: "boot".into(),
            path: parent.into(),
        });
    }
    let contents = std::sync::Arc::new(Mutex::new(document.source));
    let _watcher = if roots.is_empty() {
        None
    } else {
        use makepad_filesystem_watcher::FileSystemWatcher;
        FileSystemWatcher::start(roots, move |event| {
            // Watch roots catch atomic saves and backend rescan notifications too.
            if !document_path.as_os_str().is_empty() && (event.path == document_path || event.path.is_dir()) {
                match relax_compiler::vfs::read_to_string(&document_path) {
                    Ok(source) => {
                        let source: std::sync::Arc<str> = source.into();
                        let mut old = contents.lock().unwrap_or_else(|e| e.into_inner());
                        if *old != source {
                            *old = source.clone();
                            drop(old);
                            post(Update::Document(Document {
                                path: document_path.clone(),
                                source,
                                live: true,
                            }));
                        }
                    }
                    Err(error) => post(Update::Message(format!(
                        "{}: {error}",
                        document_path.display()
                    ))),
                }
            }
            if event.path.is_dir()
                || matches!(
                    event.path.extension().and_then(|e| e.to_str()),
                    Some("rs" | "toml")
                )
            {
                post(Update::SourcesChanged);
            }
        })
        .map_err(|e| eprintln!("makepad: run source watcher: {e}"))
        .ok()
    };
    let mut editor = crate::boot_editor::Editor::new();
    loop {
        for error in editor.poll() {
            post(Update::Message(error));
        }
        // the Studio connection is polled every 100 ms while there is one; without it the
        // worker sleeps until there is work
        let work = if editor.has_network() {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(work) => work,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(work) => work,
                Err(_) => break,
            }
        };
        match work {
            Work::Save(path, text) => {
                if let Err(e) = crate::boot_progress::save(&path, &text) {
                    eprintln!("makepad: cannot remember {}: {e}", path.display());
                }
            }
            Work::OpenFile { file, line, col } => {
                if let Err(error) = editor.open_file(file, line, col) {
                    post(Update::Message(error));
                }
            }
            Work::Resume {
                panic,
                choice,
                value,
            } => {
                // Reuse inspect's validation and its domain-restart / restartable-fn logic.
                let (reply, _) = makepad_remote::cmd::answer(
                    relax_compiler::inspect::events::cmd_resume,
                    &format!("resume {panic} {choice} {value} full=1"),
                );
                for line in reply.split(|b| *b == b'\n') {
                    if let Ok(json) = relax_compiler::json::parse(line) {
                        if let Some(error) = json.get("err") {
                            post(Update::Message(error.str().into()));
                        }
                    }
                }
                panic_changed();
            }
        }
    }
}

fn parked(domain: u32) -> Option<Option<ParkedPanic>> {
    for event in relax_compiler::inspect::events::parked_events(domain)? {
        let Ok(json) = relax_compiler::json::parse(event.as_bytes()) else {
            continue;
        };
        let Some(id) = json.get("id") else { continue };
        let id = id.str().to_string();
        let choices = json
            .get("more")
            .map(|j| j.arr())
            .unwrap_or(&[])
            .iter()
            .filter_map(|j| {
                let mut words = j.str().split_whitespace();
                (words.next() == Some("resume") && words.next() == Some(id.as_str()))
                    .then(|| words.next().map(str::to_string))
                    .flatten()
            })
            .collect();
        return Some(Some(ParkedPanic {
            id,
            message: json.get("message").map_or("", |j| j.str()).into(),
            location: json.get("location").map_or("", |j| j.str()).into(),
            choices,
            stale: json.get("kind").is_some_and(|k| k.str() == "stale access"),
        }));
    }
    Some(None)
}

impl Work {
    /// What the work is, for the log.
    fn kind(&self) -> String {
        match self {
            Work::Save(path, _) => format!("a save of {}", path.display()),
            Work::OpenFile { .. } => "an open-file request".into(),
            Work::Resume { .. } => "a resume".into(),
        }
    }
}

fn enqueue(queue: &mut VecDeque<Work>, work: Work) {
    // Geometry/timings are snapshots; a pending write of the same path can take
    // the newer value. User actions retain their order and are never dropped.
    match &work {
        Work::Save(path, text) => {
            for pending in queue.iter_mut().rev() {
                match pending {
                    Work::Save(old_path, old_text) if old_path == path => {
                        old_text.clone_from(text);
                        return;
                    }
                    _ => {}
                }
            }
        }
        Work::Resume { .. } | Work::OpenFile { .. } => {}
    }
    queue.push_back(work);
}

fn pump(
    mut host: HostWindow,
    mut data: State,
    history: History,
    geometry_path: Option<PathBuf>,
    package_dir: PathBuf,
    document: Document,
    ready: std::sync::mpsc::SyncSender<Result<(), String>>,
) {
    let hal = Hal::global();
    let (send, recv) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("run-run-actions".into())
        .spawn(move || work(recv, package_dir, document))
        .expect("run action worker");
    let mut work_pending = VecDeque::new();
    let mut work_full = false;
    let has_window = host.window().is_some();
    let mut probe = crate::boot_probe::Probe::from_env();
    if probe.is_some() {
        host.measure_input();
    }
    let mut ready = if has_window {
        Some(ready)
    } else {
        let _ = ready.try_send(Ok(()));
        None
    };
    let mut selected = 0usize;
    let mut input = crate::boot_input::Input::default();
    let mut return_entry: Option<(String, String)> = None;
    let mut progress = Progress::new(history);
    let mut failed = false;
    let mut patch_failed = false;
    let mut action_failed = false;
    let mut overlay_dismissed = false;
    let mut active = true;
    let mut relaunched = false;
    let mut restartable = true;
    let mut diagnostics = Vec::new();
    let mut diagnostic_keys = HashSet::new();
    let mut diagnostics_dirty = true;
    let mut document_errors = Vec::new();
    let mut saved_geometry = None;
    // the geometry last seen and since when: saved once it has held still (not every frame of
    // a drag, a resize or a window animation, which kept the action worker full)
    let mut settling_geometry: Option<([f32; 4], Instant)> = None;
    let mut next_frame = Instant::now();
    let mut next_status = Instant::now();
    let mut dirty = true;
    let mut stage = Stage::Running;
    let ring = ring_on();
    // the run's inspect view (the halted view over the app without a stop): opened by
    // `makepad inspect <app> session view open` (the JIT ring is display only)
    let mut inspect_open = false;
    // what the view over the app showed last (its `session view` answer and the log line)
    let mut view_shown: Option<String> = None;
    relax_compiler::inspect::run_view::set_wake(|| Hal::global().wake_session(HOST));
    let mut warm_up_at: Option<Instant> = None;
    // MAKEPAD_RUN_POSTFRAME_LOG (the responsiveness trace): the JIT ring's UI blocks too
    let mut watch = Watch::new(std::env::var_os("MAKEPAD_RUN_POSTFRAME_LOG").is_some());
    relax_compiler::postframe::on_ui_wait(ui_wait_began);
    loop {
        let (domain, window, relaunch, baseline, pending, stopping, initial_failed) = {
            let mut guard = state();
            let r = guard.as_mut().expect("host run");
            (
                r.domain,
                r.window,
                r.relaunch,
                r.baseline,
                std::mem::take(&mut r.pending),
                r.stopping,
                r.initial_failed,
            )
        };
        if let Some(probe) = &mut probe {
            probe.turn(active, &data);
        }
        if QUIT.load(Ordering::Acquire) {
            break;
        }
        if pending {
            let mut cmd = CmdWriter::new(HOST);
            if !window.is_none() {
                cmd.window_overlay(window, false, [0.0; 4], Id::NONE);
            }
            cmd.domain_relaunch(domain);
            hal.submit(&cmd.buf());
        }
        let updates = std::mem::take(&mut *UPDATES.lock().unwrap_or_else(|e| e.into_inner()));
        for update in updates {
            dirty = true;
            match update {
                Update::Begin { warm } => {
                    stage = Stage::Running;
                    progress.reset();
                    data.phase = Phase::Resolve;
                    data.done = None;
                    data.total = None;
                    data.current.clear();
                    data.warm = warm;
                    data.elapsed = 0.0;
                    data.completed_weight = 0.0;
                    data.phase_elapsed = 0.0;
                    data.parked = None;
                    data.domain = "Compiling".into();
                    diagnostics.clear();
                    diagnostic_keys.clear();
                    diagnostics_dirty = true;
                    failed = false;
                    patch_failed = false;
                    action_failed = false;
                    overlay_dismissed = false;
                    active = true;
                    relaunched |= warm;
                    restartable = true;
                }
                Update::Failure(message) if !has_window => {
                    // No window shows this failure and nobody can retry from it (a backend without
                    // a shader dialect: headless, a plain program): the run ends with it. A program
                    // that panicked in main used to leave the process running until it was killed.
                    eprintln!("makepad: {message}");
                    let _ = EXIT.compare_exchange(0, 101, Ordering::AcqRel, Ordering::Acquire);
                    QUIT.store(true, Ordering::Release);
                }
                Update::Failure(message) => {
                    diagnostics.push(Diagnostic::new("run", "", 0, 0, message));
                    diagnostics_dirty = true;
                    failed = true;
                    active = false;
                    data.domain = "Boot failed".into();
                    if let Some(r) = state().as_mut() {
                        r.relaunch = None;
                        r.stopping = false;
                    }
                }
                Update::Message(message) => {
                    diagnostics.push(Diagnostic::new("run", "", 0, 0, message));
                    diagnostics_dirty = true;
                    overlay_dismissed = false;
                    action_failed = true;
                }
                Update::Document(document) => host.reload(document),
                Update::Environment(environment) => host.environment(environment),
                Update::SourcesChanged => {
                    if failed && !stopping && restartable {
                        match retry() {
                            Ok(()) => failed = false,
                            Err(e) => post(Update::Message(e)),
                        }
                    }
                }
                Update::Domain(next) => match next {
                    DomainState::Compiling(name) => {
                        let phase = match name.as_str() {
                            "loading crates" | "load" => Phase::Parse,
                            "collect" => Phase::Typecheck,
                            "link" => Phase::Link,
                            "main" => Phase::Codegen,
                            _ => Phase::Resolve,
                        };
                        progress.phase(phase);
                        data.phase = phase;
                        data.done = None;
                        data.total = None;
                        data.current = name;
                        data.domain = "Compiling".into();
                    }
                    DomainState::Running => {
                        data.domain = "Starting".into();
                        progress.phase(Phase::FirstFrame);
                        data.phase = Phase::FirstFrame;
                        data.done = None;
                        data.total = None;
                    }
                    DomainState::Exited(code) if code == 0 || !has_window => {
                        EXIT.store(code, Ordering::Release);
                        QUIT.store(true, Ordering::Release);
                    }
                    DomainState::Exited(code) => {
                        post(Update::Failure(format!("App exited with status {code}")))
                    }
                    DomainState::Crashed(message) => post(Update::Failure(message)),
                    DomainState::Leaked { threads } => {
                        post(Update::Failure(format!(
                            "Shutdown timed out: {threads} threads remain"
                        )));
                        restartable = false;
                    }
                    DomainState::Stopped => {}
                },
                Update::Compiler(event) => match event {
                    run_state::Event::Progress {
                        domain: id,
                        phase,
                        done,
                        total,
                        item,
                    } if id.is_none() || id == Some(domain) => {
                        progress.phase(phase);
                        data.phase = phase;
                        data.done = done;
                        data.total = total;
                        data.current = item;
                    }
                    run_state::Event::Warm { domain: id, warm }
                        if id.is_none() || (id == Some(domain) && relaunched) =>
                    {
                        data.warm = warm
                    }
                    run_state::Event::Diagnostic { domain: id, record }
                        if id.is_none() || id == Some(domain) =>
                    {
                        if diagnostic_keys.insert(record.to_json()) {
                            if !active && record.severity == relax_compiler::diag::Severity::Error {
                                patch_failed = true;
                                overlay_dismissed = false;
                                data.domain = "Patch not applied · previous code is running".into();
                            }
                            diagnostics.push(record);
                            diagnostics_dirty = true;
                        }
                    }
                    run_state::Event::Patch {
                        domain: id,
                        applied,
                        records,
                    } if id.is_none() || id == Some(domain) => {
                        patch_failed = !applied;
                        action_failed = false;
                        if !applied {
                            overlay_dismissed = false;
                        }
                        if !active {
                            data.domain = if applied {
                                "Running"
                            } else {
                                "Patch not applied · previous code is running"
                            }
                            .into();
                        }
                        diagnostic_keys = records.iter().map(Diagnostic::to_json).collect();
                        diagnostics = records.to_vec();
                        diagnostics_dirty = true;
                    }
                    _ => {}
                },
            }
        }
        if QUIT.load(Ordering::Acquire) {
            break;
        }
        if PANICS_CHANGED.swap(false, Ordering::AcqRel) {
            if let Some(parked) = parked(domain) {
                if data.parked.as_ref().map(|p| &p.id) != parked.as_ref().map(|p| &p.id) {
                    if parked.is_some() {
                        overlay_dismissed = false;
                    }
                }
                data.parked = parked;
                dirty = true;
            } else {
                PANICS_CHANGED.store(true, Ordering::Release);
            }
        }
        if return_entry
            .as_ref()
            .is_some_and(|(id, _)| data.parked.as_ref().map(|p| &p.id) != Some(id))
        {
            return_entry = None;
            host.input(None);
        }
        let physical = hal.with_world(|w| {
            w.session(HOST)
                .and_then(|s| s.windows.get(HOST, window).ok())
                .map(|w| {
                    (
                        w.val.geom,
                        w.val.borrower_frame,
                        w.val.presentations,
                        w.val.presenter,
                        w.lent_to,
                    )
                })
        });
        let mut may_present = true;
        let mut app_frame = false;
        let mut app_frames = 0;
        let mut lent_to_app = false;
        let first_present = active;
        if let Some((geom, borrower_frame, presentations, presenter, lent)) = physical {
            app_frames = presentations;
            lent_to_app = lent == domain && domain != 0 && !stopping && !pending;
            app_frame = borrower_frame && lent == domain && domain != 0 && !stopping && !pending;
            may_present = !borrower_frame || lent == 0;
            if settling_geometry.is_none_or(|(g, _)| g != geom) {
                settling_geometry = Some((geom, Instant::now()));
            }
            let settled = settling_geometry.is_some_and(|(_, since)| since.elapsed() >= Duration::from_millis(500));
            if let Some(path) = geometry_path.as_ref().filter(|_| {
                settled
                    && saved_geometry != Some(geom)
                    && geom.iter().all(|v| v.is_finite())
                    && geom[2] > 0.0
                    && geom[3] > 0.0
            }) {
                saved_geometry = Some(geom);
                enqueue(
                    &mut work_pending,
                    Work::Save(
                        path.clone(),
                        format!("{} {} {} {}\n", geom[0], geom[1], geom[2], geom[3]),
                    ),
                );
            }
            if presentations > 0 {
                if let Some(ready) = ready.take() {
                    let _ = ready.try_send(Ok(()));
                }
            }
            if active
                && borrower_frame
                && lent == domain
                && !pending
                && !stopping
                && domain != 0
                && presenter == domain
                && presentations > baseline
            {
                active = false;
                failed = false;
                data.domain = "Running".into();
                EXIT.store(0, Ordering::Release);
                let (path, contents) = progress.finish(data.warm);
                enqueue(&mut work_pending, Work::Save(path, contents));
                if let Some(r) = state().as_mut() {
                    if let Some(t) = r.relaunch.take() {
                        eprintln!(
                            "relaunch: {:.1} ms to first frame (window kept)",
                            t.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
                dirty = true;
            }
        }
        match relax_compiler::inspect::run_view::take() {
            Some(true) => {
                inspect_open = true;
                overlay_dismissed = false;
                dirty = true;
            }
            Some(false) => {
                overlay_dismissed = true;
                if std::mem::take(&mut inspect_open) && data.domain.starts_with("JIT · ") {
                    data.domain = "Running".into();
                }
                dirty = true;
            }
            None => {}
        }
        // Errors reveal immediately. Otherwise a warm relaunch preserves the last app
        // image for 300 ms, then uses exactly the same document/context as cold boot.
        let overlay = has_window
            && app_frame
            && !overlay_dismissed
            && (patch_failed
                || action_failed
                || failed
                || inspect_open
                || !document_errors.is_empty()
                || data.parked.is_some());
        // what the view over the app shows and why: `session view`, and a line when it changes
        let why = overlay.then(|| {
            if data.parked.as_ref().is_some_and(|p| p.stale) {
                "paused on invalidated code"
            } else if data.parked.is_some() {
                "paused on a panic"
            } else if failed {
                "the app stopped"
            } else if patch_failed {
                "a live patch failed"
            } else if action_failed {
                "a run action failed"
            } else if !document_errors.is_empty() {
                "the boot document has errors"
            } else {
                "the inspect view (session view open)"
            }
        });
        let shown = format!(
            "\"over_app\":{},\"why\":{},\"dismissed\":{},\"pid\":{}",
            overlay,
            why.map_or("null".to_string(), |w| format!("\"{w}\"")),
            overlay_dismissed,
            std::process::id()
        );
        if view_shown.as_deref() != Some(shown.as_str()) {
            let was_over = view_shown.as_deref().is_some_and(|v| v.starts_with("\"over_app\":true"));
            match why {
                Some(why) => eprintln!(
                    "makepad-run: the run's view is over the app: {why}; Esc or `makepad inspect {} session view dismiss` dismisses it",
                    std::process::id()
                ),
                None if was_over => eprintln!("makepad-run: the run's view is off the app"),
                None => {}
            }
            relax_compiler::inspect::run_view::publish(shown.clone());
            view_shown = Some(shown);
        }
        let mode = if overlay { Mode::Overlay } else { Mode::Boot };
        if data.mode != mode {
            data.mode = mode;
            dirty = true;
        }
        let visible = overlay
            || (has_window
                && may_present
                && (failed
                    || data.parked.is_some()
                    || (active
                        && relaunch.is_none_or(|t| t.elapsed() >= Duration::from_millis(300)))));
        // the window layer (Metal): the boot field the app's first frames go under, the warm-up
        // after its first present (until the JIT's readiness signal). An opaque overlay, a failure
        // or a stop takes it away. (Later JIT work shows in the JIT ring, below.)
        // the boot stage begins before the app has the window, once the field has presented
        // (its first frame then never reaches the window before the layer)
        let layers = host.layers() && !overlay && !failed && data.parked.is_none() && !stopping && !pending;
        if !layers || (stage != Stage::Boot && !lent_to_app) {
            stage = Stage::Running;
        }
        stage = match stage {
            Stage::Running if layers && active && visible && host.field_mode() && app_frames > 0 => {
                Stage::Boot
            }
            // compile errors in the boot window show its halted view, not the field
            Stage::Boot if active && !host.field_mode() => Stage::Running,
            Stage::Boot if !active && first_present => {
                warm_up_at = Some(Instant::now());
                Stage::WarmUp
            }
            Stage::WarmUp if relax_compiler::postframe::ready(READY_QUIET_NS) => {
                if let Some(at) = warm_up_at.take() {
                    eprintln!(
                        "makepad-run: warm-up {:.0} ms after the first present ({})",
                        at.elapsed().as_secs_f64() * 1000.0,
                        if data.warm { "warm" } else { "cold" }
                    );
                }
                Stage::Lift
            }
            Stage::Lift if host.layer_done() => Stage::Running,
            stage => stage,
        };
        let mut wake_in = None;
        let want = match stage {
            Stage::Boot => Some(LayerWant::Boot),
            Stage::WarmUp => {
                let (done, queued) = relax_compiler::postframe::ahead_progress();
                Some(LayerWant::WarmUp { done, queued })
            }
            Stage::Lift => Some(LayerWant::Ready),
            Stage::Running => None,
        };
        host.layer(want, app_frames);
        // the JIT ring while the app runs: the JIT's work in flight, a UI block, what the program
        // is stopped on (while it is)
        let running = ring && stage == Stage::Running && lent_to_app && !active && !failed;
        let alert = data
            .parked
            .as_ref()
            .map(|p| if p.stale { RingAlert::Stale } else { RingAlert::Panic })
            .or_else(|| rebuild_failed().then_some(RingAlert::RebuildFailed));
        if running && host.badges() {
            let want = watch.look(host.dump_ms()).filter(|_| data.parked.is_none());
            if inspect_open {
                let summary = format!("JIT · {}", relax_boot::ring_summary(want, None));
                if data.domain != summary && data.parked.is_none() && data.diagnostics.is_empty() {
                    data.domain = summary;
                    dirty = true;
                }
            }
            let shown = host.ring(want, alert);
            for wait in [shown, watch.block_wake(), Some(RING_LOOK)].into_iter().flatten() {
                wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
            }
        } else if let Some(wait) = host.ring(None, None) {
            // a ring fading out
            wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
        }
        if running && host.dumping() {
            host.dump_app();
            wake_in = Some(wake_in.map_or(RING_LOOK, |w: Duration| w.min(RING_LOOK)));
        }
        let layered = host.layer_on();
        if active && (dirty || Instant::now() >= next_status) {
            progress.tick(&mut data);
            if matches!(data.phase, Phase::Codegen | Phase::FirstFrame) {
                if let Some(item) = relax_compiler::tui::current_item() {
                    data.current = item;
                }
            }
            next_status = Instant::now() + Duration::from_millis(100);
            dirty = true;
        }
        if diagnostics_dirty {
            let records = diagnostics.iter().cloned();
            let records = records.chain(document_errors.iter().cloned());
            // root causes first, a folded group's members together (the boot window's list order;
            // the selection, Copy and Open index the same list)
            let mut records = records.collect::<Vec<_>>();
            relax_boot::order_diagnostics(&mut records);
            data.diagnostics = records.into();
            diagnostics_dirty = false;
            dirty = true;
        }
        let count = data.diagnostics.len() + usize::from(data.parked.is_some());
        let next = selected.min(count.saturating_sub(1));
        if next != selected {
            selected = next;
            host.select(selected);
        }
        if dirty {
            data.can_retry = restartable && (initial_failed || (failed && domain != 0));
            data.can_restart = restartable && domain != 0 && !stopping;
            host.update(data.clone());
            dirty = false;
        }
        let hold = !visible && !layered && host.hold();
        host.offscreen(hold);
        let draw = (visible || hold || layered) && Instant::now() >= next_frame;
        let timeout = if visible || hold || layered {
            next_frame
                .saturating_duration_since(Instant::now())
                .as_nanos() as u64
        } else if let Some(wake_in) = wake_in {
            wake_in.as_nanos() as u64
        } else if (has_window && active)
            || !work_pending.is_empty()
            || PANICS_CHANGED.load(Ordering::Acquire)
        {
            16_666_667
        } else {
            u64::MAX
        };
        let alive = host.turn(timeout, draw);
        if window.is_none() {
            if let Some(window) = host.window() {
                if let Some(r) = state().as_mut() {
                    r.window = window;
                }
            }
        }
        if let Some(probe) = &mut probe {
            if let Some(window) = host.window() {
                probe.window(window);
            }
            probe.parts(host.turn_parts());
            if draw {
                probe.frame(&data);
            }
            probe.latencies(host.take_latencies(), &data);
        }
        if !alive {
            QUIT.store(true, Ordering::Release);
            break;
        }
        if draw {
            // frames on a fixed 60 Hz schedule: the time a turn takes is not added to every
            // interval; a turn that fell behind starts the schedule again from now
            let period = Duration::from_nanos(host.frame_period_ns());
            let now = Instant::now();
            next_frame += period;
            if next_frame <= now {
                next_frame = now + period;
            }
        }
        if !visible {
            input.cancel(&mut host);
        }
        let mut close = false;
        for event in host.events() {
            let action = match event {
                BootEvent::DocumentErrors(errors) => {
                    if !errors.is_empty() {
                        overlay_dismissed = false;
                    }
                    document_errors = errors;
                    diagnostics_dirty = true;
                    None
                }
                BootEvent::Text(text) if visible && return_entry.is_some() => {
                    if let Some((_, value)) = &mut return_entry {
                        value.push_str(&text.replace(['\r', '\n'], " "));
                        host.input(Some(value.clone()));
                    }
                    None
                }
                BootEvent::Input(raw, payload) if visible => raw
                    .decode(&payload)
                    .and_then(|event| input.event(&mut host, event)),
                BootEvent::KeyDown(raw) if visible => {
                    let Some(makepad_hal::HalEvent::KeyDown {
                        key, repeat, mods, ..
                    }) = raw.decode(&[])
                    else {
                        continue;
                    };
                    let key = KeyCode::from_index(key);
                    if return_entry.is_some() {
                        match key {
                            KeyCode::Escape => Some(Action::Cancel),
                            KeyCode::Backspace => {
                                if let Some((_, value)) = &mut return_entry {
                                    value.pop();
                                    host.input(Some(value.clone()));
                                }
                                None
                            }
                            KeyCode::ReturnKey | KeyCode::NumpadEnter if !repeat => {
                                Some(Action::Submit)
                            }
                            _ => None,
                        }
                    } else if mods
                        & (makepad_hal::input::MOD_CONTROL
                            | makepad_hal::input::MOD_ALT
                            | makepad_hal::input::MOD_LOGO)
                        != 0
                    {
                        None
                    } else {
                        match key {
                            KeyCode::Escape if !repeat => Some(Action::Dismiss),
                            KeyCode::KeyR if !repeat => Some(Action::Restart),
                            KeyCode::KeyC if !repeat => Some(Action::Copy),
                            KeyCode::KeyE if !repeat => Some(Action::Open),
                            KeyCode::ArrowUp => {
                                selected = selected.saturating_sub(1);
                                host.select(selected);
                                None
                            }
                            KeyCode::ArrowDown => {
                                let count =
                                    data.diagnostics.len() + usize::from(data.parked.is_some());
                                selected = (selected + 1).min(count.saturating_sub(1));
                                host.select(selected);
                                None
                            }
                            KeyCode::PageUp => {
                                host.scroll(-1.0);
                                None
                            }
                            KeyCode::PageDown => {
                                host.scroll(1.0);
                                None
                            }
                            key if !repeat => {
                                let index = match key {
                                    KeyCode::Key1 => Some(0),
                                    KeyCode::Key2 => Some(1),
                                    KeyCode::Key3 => Some(2),
                                    KeyCode::Key4 => Some(3),
                                    KeyCode::Key5 => Some(4),
                                    KeyCode::Key6 => Some(5),
                                    KeyCode::Key7 => Some(6),
                                    KeyCode::Key8 => Some(7),
                                    KeyCode::Key9 => Some(8),
                                    _ => None,
                                };
                                index.map(Action::Resume)
                            }
                            _ => None,
                        }
                    }
                }
                _ => None,
            };
            // Both input paths use these checks and the existing action worker.
            match action {
                Some(Action::Restart) if data.can_retry || data.can_restart => match retry() {
                    Ok(()) => failed = false,
                    Err(error) => post(Update::Message(error)),
                },
                Some(Action::Copy) => {
                    let text = data
                        .diagnostics
                        .get(selected)
                        .map(relax_boot::diagnostic_text)
                        .or_else(|| {
                            data.parked
                                .as_ref()
                                .map(|p| format!("{} {}\n{}", p.id, p.location, p.message))
                        });
                    if let Some(text) = text {
                        let mut cmd = CmdWriter::new(HOST);
                        cmd.clip_set(text.as_bytes());
                        hal.submit(&cmd.buf());
                    }
                }
                Some(Action::Open) => {
                    if let Some(d) = data
                        .diagnostics
                        .get(selected)
                        .filter(|d| !d.file.is_empty())
                    {
                        enqueue(
                            &mut work_pending,
                            Work::OpenFile {
                                file: d.file.clone(),
                                line: d.line,
                                col: d.col,
                            },
                        );
                    }
                }
                Some(Action::Dismiss) if overlay => {
                    overlay_dismissed = true;
                    if std::mem::take(&mut inspect_open) && data.domain.starts_with("JIT · ") {
                        data.domain = "Running".into();
                    }
                    dirty = true;
                }
                Some(Action::Select(index))
                    if index < data.diagnostics.len() + usize::from(data.parked.is_some()) =>
                {
                    selected = index;
                    host.choose(selected);
                }
                Some(Action::Close) => close = true,
                Some(action @ (Action::Minimize | Action::Maximize)) => host.window_control(action),
                Some(Action::Cancel) if return_entry.is_some() => {
                    return_entry = None;
                    host.input(None);
                }
                Some(Action::Submit) if return_entry.is_some() => {
                    let (panic, value) = return_entry.take().unwrap();
                    enqueue(
                        &mut work_pending,
                        Work::Resume {
                            panic,
                            choice: "return".into(),
                            value,
                        },
                    );
                    host.input(None);
                }
                Some(Action::Resume(index)) if return_entry.is_none() => {
                    if let Some(panic) = &data.parked {
                        if let Some(choice) = panic.choices.get(index) {
                            if choice == "return" {
                                return_entry = Some((panic.id.clone(), String::new()));
                                host.input(Some(String::new()));
                            } else {
                                enqueue(
                                    &mut work_pending,
                                    Work::Resume {
                                        panic: panic.id.clone(),
                                        choice: choice.clone(),
                                        value: String::new(),
                                    },
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if close {
            eprintln!("makepad-run: the boot window ends: its close button");
            QUIT.store(true, Ordering::Release);
            break;
        }
        while let Some(work) = work_pending.pop_front() {
            match send.try_send(work) {
                Ok(()) => {
                    work_full = false;
                }
                Err(std::sync::mpsc::TrySendError::Full(work)) => {
                    if !work_full {
                        eprintln!(
                            "makepad: run action worker busy; retaining {} for a later frame",
                            work.kind()
                        );
                        work_full = true;
                    }
                    work_pending.push_front(work);
                    break;
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(work)) => {
                    work_pending.push_front(work);
                    eprintln!("makepad: run action worker stopped; closing the run");
                    EXIT.store(101, Ordering::Release);
                    QUIT.store(true, Ordering::Release);
                    break;
                }
            }
        }
    }
    if let Some(probe) = &mut probe {
        probe.finish();
    }
    quit_trace("the window pump quits");
    relax_compiler::run::quitting();
    relax_compiler::inspect::events::run_state_available(false);
    RETRY.notify_all();
    if let Some(ready) = ready {
        let _ = ready.try_send(Err("run closed before its first frame".into()));
    }
    let (domain, window) = state()
        .as_ref()
        .map_or((0, Id::NONE), |r| (r.domain, r.window));
    let mut cmd = CmdWriter::new(HOST);
    if domain != 0 {
        cmd.domain_stop(domain, domain_stop::QUIT);
    }
    if !window.is_none() {
        cmd.window_close(window);
    }
    hal.submit(&cmd.buf());
    // The HAL runs the app's stop on a thread of its own (Hal::run_host_calls) and nobody waited
    // for it: the process exited while the app's threads still ran JIT code and drew, its heap
    // was being torn down and its HAL session freed (crashes at every quit). The run ends after
    // the stop: the app quit (or was given up as leaked), its threads joined, its heap gone.
    // Waited for here, while the main loop still serves the window system.
    if domain != 0 {
        quit_trace("waiting for the app's stop");
        if let Err(e) = relax_compiler::domain::stop(domain, relax_compiler::domain::Stop::Quit) {
            eprintln!("makepad: stopping the app at quit: {e}");
        }
    }
    // The UI has stopped: finish pending persistence/actions before ending this run.
    for work in work_pending {
        let _ = send.send(work);
    }
    drop(send);
    quit_trace("the app stopped; joining the action worker");
    for compile in makepad_script_compute::boundary::kernels_compiling() {
        quit_trace(&format!("still compiling at the app's stop: {compile}"));
    }
    if worker.join().is_err() {
        eprintln!("makepad: run action worker panicked");
    }
}
