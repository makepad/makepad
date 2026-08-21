//! Per-isolate resource limits for Splash mini-apps, on the container model.
//!
//! A Splash isolate already cannot reach the filesystem, the network, or
//! another isolate's heap without the host handing it those things. What it
//! could still do was *cost* whatever it liked: the VM's budgets are per
//! ENTRY, so a script that arranges to be entered often — a fast interval, a
//! network callback, a chain of widget events — got a fresh 64ms and a fresh
//! 200k instructions every time, forever. Nothing capped an isolate's heap,
//! its timer count, or its share of the frame.
//!
//! # The rule
//!
//! **Nothing is limited while limiting it would buy nothing.** This is cgroup
//! v2's split, and it is the whole design:
//!
//! - a **weight** ([`SplashLimits::weight`], cgroup `cpu.weight`/`io.weight`)
//!   decides who yields to whom when isolates actually compete. It has no
//!   effect at all when they do not.
//! - a **max** (`cpu_max_ms`, `mem_max_slots`, …, cgroup `cpu.max`/
//!   `memory.max`) is an absolute ceiling that applies whatever else is
//!   happening. Every one of them is OFF or set to a runaway backstop by
//!   default, because an absolute cap on an idle system is a tax with no
//!   beneficiary.
//!
//! So one mini-app alone gets the machine. Five split it by weight. A quiet
//! app costs nothing and holds nothing back for anyone else. An app is only
//! ever trimmed while the pool it is drawing from is genuinely full AND it is
//! the one over its share of it.
//!
//! # Every contended resource works the same way
//!
//! CPU is time-multiplexed and memory, timers and requests are
//! space-multiplexed, but the rationing rule is identical
//! ([`over_share`]): compare the pool's total against its budget, and only
//! then compare this isolate against its weighted slice.
//!
//! - **CPU** — a wall-clock window. Over-share means shorter entry slices.
//! - **Memory** — live heap slots after a collection. Over-share is reported
//!   as pressure; the host collects that isolate harder and stops it only if
//!   it stays over (cgroup `memory.high` then `memory.max`).
//! - **Timers** and **in-flight requests** — slot pools. Over-share means the
//!   next one is refused.
//!
//! Storage is deliberately NOT on this model: disk is not reclaimed when
//! pressure passes, so a share of it would be a share of something nobody
//! gives back. It stays an absolute quota, enforced by the jail.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The window every CPU decision is made over.
const WINDOW: Duration = Duration::from_secs(1);

/// The frame interval the host is trying to hold, in seconds. Set by the
/// embedder ([`set_frame_target`]); 60Hz until it says otherwise.
const DEFAULT_FRAME_TARGET: f64 = 1.0 / 60.0;

/// How much of a window mini-apps must collectively be using before a missed
/// frame is treated as THEIR contention. Below this the machine is slow for
/// reasons trimming an app cannot fix — a software rasteriser, another
/// process, a cold cache — and squeezing apps would be punishing them for
/// someone else's problem.
const APP_BLAME_FRACTION: f64 = 0.2;

/// How far past the target the smoothed frame interval must drift before the
/// system counts as contended. Frames are noisy, and trimming apps over a
/// single late frame would be a controller chasing its own tail.
const CONTENTION_FACTOR: f64 = 1.5;

/// What one timer FIRE costs its isolate, beyond the script it then runs.
///
/// A wakeup is not free even when the callback is empty: the event reaches
/// the app through a full dispatch pass before any script executes, and that
/// pass is host cost the script-time accounting cannot see. Charging it is
/// what makes a 1ms timer expensive to the app that asked for it — which is
/// the honest version of the arbitrary "fastest timer" floor this replaced.
/// An estimate, deliberately: measuring the pre-hook dispatch per timer costs
/// more than the number is worth.
const WAKEUP_COST: Duration = Duration::from_micros(250);

/// Live heap slots ALL isolates together may hold before anyone is asked to
/// give some back, unless the host says otherwise ([`set_memory_pool`]). A
/// lone app may use the lot.
///
/// A default rather than a truth: how much memory there is to share is
/// something the embedder knows and this crate does not.
const DEFAULT_HEAP_POOL: usize = 24_000_000;

thread_local! {
    static HEAP_POOL: std::cell::Cell<usize> = const { std::cell::Cell::new(DEFAULT_HEAP_POOL) };
}

/// How many live heap slots all isolates together may hold. The host sets
/// this from what the machine actually has; the default is a guess and says
/// so.
pub fn set_memory_pool(slots: usize) {
    HEAP_POOL.with(|p| p.set(slots.max(1)));
}

fn heap_pool() -> usize {
    HEAP_POOL.with(|p| p.get())
}

/// Live script timers across every isolate before the pool starts rationing.
const GLOBAL_TIMERS: u32 = 512;

/// In-flight HTTP requests across every isolate before the pool rations.
const GLOBAL_INFLIGHT_HTTP: u32 = 64;

/// The container rule, in one function, for every contended resource.
///
/// `true` means "this isolate must yield": the pool is genuinely full AND
/// this isolate is holding more than its weighted slice of it. Either half
/// alone is not enough — a full pool where everyone is within their share is
/// simply a busy system working correctly, and an isolate over its share of a
/// pool with room to spare is costing nobody anything.
fn over_share(total: f64, budget: f64, mine: f64, fraction: f64) -> bool {
    total > budget && mine > budget * fraction
}

/// How much one Splash isolate may consume.
///
/// One `weight` governs every contended resource — an app is "important" or
/// it is not, and asking a user to rank it separately for processor, memory
/// and downloads is asking them to invent numbers. The `*_max` fields are the
/// absolute ceilings, off or set to runaway backstops by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplashLimits {
    /// This isolate's pull on every contended pool (cgroup `cpu.weight`).
    /// Relative, not absolute: two isolates at 4 and 1 split a contended
    /// resource 80/20, and either one alone is not limited at all.
    pub weight: u32,
    /// Wall-clock an isolate may spend in ONE entry into script. A latency
    /// bound rather than a share: it stops a single entry eating a frame.
    pub entry_time_ms: u64,
    /// Instructions an isolate may execute in ONE entry.
    pub entry_instructions: usize,
    /// Absolute wall-clock cap per window, whatever else is running
    /// (`cpu.max`). `None` — the default — means "share only".
    pub cpu_max_ms: Option<u64>,
    /// Live heap slots this isolate alone may hold, however quiet the system
    /// is (`memory.max`). A runaway backstop, not a working limit: memory is
    /// shared out by weight long before this. Defaults to the whole pool, so
    /// a lone app is never stopped for using memory nobody else wants — a
    /// backstop BELOW the pool would quietly make the sharing unreachable.
    pub mem_max_slots: usize,
    /// Live script timers it may hold (a sanity backstop; what those timers
    /// COST is already charged as CPU when their callbacks run).
    pub timers_max: u32,
    /// Concurrent in-flight HTTP requests it may hold.
    pub http_max: u32,
}

impl Default for SplashLimits {
    fn default() -> Self {
        Self {
            // Only ratios matter, so the number is arbitrary; it sits above 1
            // so a host can say "less than normal" without fractions.
            weight: 4,
            // The values the widgets crate has always applied, now nameable.
            entry_time_ms: 64,
            entry_instructions: 200_000,
            cpu_max_ms: None,
            mem_max_slots: DEFAULT_HEAP_POOL,
            timers_max: 256,
            http_max: 16,
        }
    }
}

impl SplashLimits {
    /// Limits for a surface that runs while the user is looking at something
    /// else — a home-screen tile, a preview.
    ///
    /// The difference is WEIGHT: a tile competes with eleven others and is not
    /// what the user is waiting on, so under contention it yields. With
    /// nothing to compete against it runs exactly as fast as anything else,
    /// which is the entire point of a share.
    pub fn background() -> Self {
        Self {
            weight: 1,
            entry_time_ms: 16,
            timers_max: 64,
            // A tile has no business ballooning even on an idle system.
            mem_max_slots: DEFAULT_HEAP_POOL / 3,
            http_max: 4,
            ..Self::default()
        }
    }
}

/// Which limit an isolate crossed. Reported to the host, which decides what a
/// repeat offender deserves — this module only measures and refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplashLimitKind {
    /// Held over its share while the thread was contended, or crossed its own
    /// hard ceiling.
    Cpu,
    /// Asked for more timers than it may hold.
    Timers,
    /// Holding more heap than its share of a system that has run out, or more
    /// than its own backstop.
    Memory,
    /// Too many HTTP requests in flight at once.
    Network,
}

impl SplashLimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SplashLimitKind::Cpu => "cpu",
            SplashLimitKind::Timers => "timers",
            SplashLimitKind::Memory => "memory",
            SplashLimitKind::Network => "network",
        }
    }
}

/// One limit crossing, for the host to act on.
#[derive(Clone, Debug)]
pub struct SplashLimitEvent {
    /// The isolate's heap key, which the host maps back to its own identity.
    pub heap_key: usize,
    pub kind: SplashLimitKind,
    /// What was asked for and what was allowed, for a message the user can
    /// understand ("wanted 4000 timers, allowed 256").
    pub wanted: u64,
    pub allowed: u64,
}

/// Per-isolate settings and the counts that are not about time.
#[derive(Default)]
struct IsolateState {
    limits: SplashLimits,
    /// Live timers this isolate holds, and in-flight requests — both slot
    /// pools, rationed by the same rule as CPU and memory.
    timers: u32,
    /// Live heap slots as of this isolate's last collection.
    heap_slots: usize,
    /// In-flight HTTP requests, as last reported by the stdlib.
    inflight_http: usize,
    /// Consecutive collections this isolate has finished over its share.
    /// Pressure first, a stop only if it will not come back down — cgroup
    /// `memory.high` before `memory.max`.
    heap_pressure: u32,
}

/// The shared CPU window, plus how the frame is actually doing. One window
/// for every isolate, or shares would not be comparable.
struct CpuWindow {
    started: Instant,
    /// Wall-clock spent in script this window, per heap.
    spent: HashMap<usize, Duration>,
    total: Duration,
    /// The interval the host wants to hold between frames.
    frame_target: f64,
    /// Smoothed frame interval, or `None` while nothing is drawing — which
    /// is not contention, it is quiet.
    frame_ema: Option<f64>,
    /// How much of their slice apps are currently allowed, 0..1. Falls while
    /// the frame is being missed and climbs back when it recovers, so apps
    /// give up exactly as much as the launcher needs and no more. There is no
    /// number here that anybody chose.
    pressure: f64,
}

impl Default for CpuWindow {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            spent: HashMap::new(),
            total: Duration::ZERO,
            frame_target: DEFAULT_FRAME_TARGET,
            frame_ema: None,
            pressure: 1.0,
        }
    }
}

impl CpuWindow {
    /// Whether the thing that owns the frame is losing it, AND the apps are
    /// why.
    ///
    /// Both halves are load-bearing. The first measures the LAUNCHER rather
    /// than the apps: it draws every app's pixels, so if it cannot make its
    /// deadline nothing else on screen matters — which is why it does not sit
    /// in the same weighted pool, and is not given a reserved slice either. It
    /// gets first call on the time it actually needs.
    ///
    /// The second stops that becoming a tax on apps for someone else's
    /// slowness. A machine can miss frames because the renderer is slow, the
    /// display is software-rasterised, or another process is thrashing — none
    /// of which an app can fix by being trimmed, and all of which would
    /// otherwise leave every app permanently squeezed. So apps are only
    /// blamed when they are actually using a meaningful part of the window.
    fn contended(&self) -> bool {
        let frames_late = match self.frame_ema {
            // Nothing is drawing. Nobody is being kept waiting.
            None => false,
            Some(ema) => ema > self.frame_target * CONTENTION_FACTOR,
        };
        frames_late && self.total.as_secs_f64() >= WINDOW.as_secs_f64() * APP_BLAME_FRACTION
    }

    fn note_frame(&mut self, interval_s: f64) {
        if !interval_s.is_finite() || interval_s <= 0.0 {
            return;
        }
        // A long gap means nothing was being drawn, not that a frame was
        // late; and one slow frame should not be a verdict, so a sample's
        // influence is capped as well as smoothed.
        let sample = interval_s.min(self.frame_target * 3.0);
        self.frame_ema = Some(match self.frame_ema {
            None => sample,
            Some(ema) => ema * 0.8 + sample * 0.2,
        });
        // Squeeze while the frame is being missed, release when it is not.
        // The floor is there so a squeezed app still makes progress, and the
        // ceiling is "not squeezed at all", which is where it sits whenever
        // the launcher is keeping up.
        self.pressure = if self.contended() {
            (self.pressure * 0.9).max(0.15)
        } else {
            (self.pressure * 1.05).min(1.0)
        };
    }
}

impl CpuWindow {
    fn roll(&mut self, now: Instant) {
        if now.duration_since(self.started) >= WINDOW {
            self.started = now;
            self.spent.clear();
            self.total = Duration::ZERO;
        }
    }

    fn spent_by(&self, heap_key: usize) -> Duration {
        self.spent.get(&heap_key).copied().unwrap_or(Duration::ZERO)
    }

    /// This heap's fraction of the contended thread, by weight, counting only
    /// isolates that have actually run this window.
    ///
    /// Demand-based on purpose: eleven idle tiles must not shrink the share of
    /// the one app doing something. An isolate that has spent nothing is not
    /// competing, so it holds nothing back.
    fn share_fraction(&self, heap_key: usize, states: &HashMap<usize, IsolateState>) -> (f64, usize) {
        let weight_of = |k: &usize| {
            states
                .get(k)
                .map(|s| s.limits.weight)
                .unwrap_or(SplashLimits::default().weight)
                .max(1)
        };
        let mut weights: u64 = 0;
        let mut count = 0usize;
        for (k, spent) in &self.spent {
            if !spent.is_zero() {
                weights += weight_of(k) as u64;
                count += 1;
            }
        }
        let mine = weight_of(&heap_key) as u64;
        // Count ourselves even on our first entry of the window, or a fresh
        // isolate would compute a share of zero and be trimmed instantly.
        if self.spent_by(heap_key).is_zero() {
            weights += mine;
            count += 1;
        }
        if weights == 0 {
            return (1.0, 1);
        }
        (mine as f64 / weights as f64, count)
    }

}

thread_local! {
    /// heap key -> that isolate's limits and non-time counters.
    static STATES: RefCell<HashMap<usize, IsolateState>> = RefCell::new(HashMap::new());
    /// The one CPU window every isolate is measured against.
    static WINDOW_STATE: RefCell<CpuWindow> = RefCell::new(CpuWindow::default());
    /// Crossings from every isolate, in order, awaiting [`take_limit_events`].
    static EVENTS: RefCell<Vec<SplashLimitEvent>> = const { RefCell::new(Vec::new()) };
}

/// Installs (or clears) an isolate's limits. Host-only: called from
/// `Splash::set_limits`, never reachable from script.
pub(crate) fn set_limits_for_heap(heap_key: usize, limits: Option<SplashLimits>) {
    STATES.with(|s| {
        let mut s = s.borrow_mut();
        match limits {
            Some(limits) => s.entry(heap_key).or_default().limits = limits,
            // Back to the defaults, and forget the counters with them: a host
            // clearing limits is not trying to keep a grudge.
            None => {
                s.remove(&heap_key);
            }
        }
    });
}

/// The limits in force for a heap — the defaults when the host set none.
pub fn limits_for_heap(heap_key: usize) -> SplashLimits {
    STATES.with(|s| s.borrow().get(&heap_key).map(|st| st.limits).unwrap_or_default())
}

/// Drops the limits and counters of reclaimed isolates. Called from
/// `gc_dead_splash_isolates` alongside the storage roots and bridge state.
pub(crate) fn gc_limits(dead_heaps: &[usize]) {
    STATES.with(|s| {
        let mut s = s.borrow_mut();
        for heap in dead_heaps {
            s.remove(heap);
        }
    });
    WINDOW_STATE.with(|w| {
        let mut w = w.borrow_mut();
        for heap in dead_heaps {
            if let Some(spent) = w.spent.remove(heap) {
                w.total = w.total.saturating_sub(spent);
            }
        }
    });
    EVENTS.with(|e| e.borrow_mut().retain(|ev| !dead_heaps.contains(&ev.heap_key)));
}

fn record(heap_key: usize, kind: SplashLimitKind, wanted: u64, allowed: u64) {
    let event = SplashLimitEvent { heap_key, kind, wanted, allowed };
    EVENTS.with(|e| {
        let mut e = e.borrow_mut();
        // A misbehaving isolate can cross a limit every frame; the host only
        // needs to know THAT it did, not four thousand times over.
        if e.len() < 64 {
            e.push(event);
        }
    });
}

/// Takes every limit crossing since the last call. The host drains this the
/// same way it drains the service bridge, and decides what to do about it.
pub fn take_limit_events() -> Vec<SplashLimitEvent> {
    EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

/// How much wall-clock this isolate's next entry may use.
///
/// Full speed unless BOTH of these are true: the mini-apps have collectively
/// saturated the window, and this isolate is the one over its share of it.
/// Everything else — an app alone, an app under its share, an app on an idle
/// system — runs unthrottled.
pub(crate) fn cpu_allowance(heap_key: usize) -> Option<Duration> {
    let now = Instant::now();
    STATES.with(|states| {
        let states = states.borrow();
        let limits = states.get(&heap_key).map(|s| s.limits).unwrap_or_default();
        let entry = Duration::from_millis(limits.entry_time_ms);

        WINDOW_STATE.with(|w| {
            let mut w = w.borrow_mut();
            w.roll(now);

            // An absolute ceiling does not care who else is running, which is
            // exactly why it is off by default.
            if let Some(cap) = limits.cpu_max_ms {
                if w.spent_by(heap_key) >= Duration::from_millis(cap) {
                    return Some(Duration::from_millis(4).max(entry / 8));
                }
            }
            // Frames are fine: nobody is waiting on anything, so trimming an
            // app would slow it down for no one's benefit. This is the case
            // for one app alone on an idle machine, and it is the common one.
            if !w.contended() {
                return Some(entry);
            }
            // The launcher is losing its frame. Two things decide the slice:
            // WEIGHT splits what the apps get between them, and PRESSURE says
            // how much that is in total — it falls while frames are missed and
            // climbs back when they recover, so apps give up as much as the
            // launcher needs and no more. Note the two multiply, which is why
            // one app alone is squeezed just as hard as five together: the
            // fractions across competing apps sum to one either way.
            let (fraction, _) = w.share_fraction(heap_key, &states);
            Some(
                entry
                    .mul_f64(fraction * w.pressure)
                    .max(Duration::from_millis(4)),
            )
        })
    })
}

/// Charges what an entry actually took against the shared window.
pub(crate) fn charge_cpu(heap_key: usize, spent: Duration) {
    let now = Instant::now();
    let (report, mine, fraction) = WINDOW_STATE.with(|w| {
        let mut w = w.borrow_mut();
        w.roll(now);
        *w.spent.entry(heap_key).or_default() += spent;
        w.total += spent;
        let mine = w.spent_by(heap_key);
        let (fraction, _) = STATES.with(|s| w.share_fraction(heap_key, &s.borrow()));
        // Worth telling the host about only when it means something: frames
        // are being missed AND this isolate is the one using more than its
        // weight of what the apps are collectively taking.
        let report = w.contended() && mine.as_secs_f64() > w.total.as_secs_f64() * fraction;
        (report, mine, fraction)
    });
    if report {
        record(
            heap_key,
            SplashLimitKind::Cpu,
            mine.as_millis() as u64,
            (mine.as_millis() as f64 * fraction) as u64,
        );
    }
}

/// Charges one timer FIRE to its isolate.
///
/// A wakeup costs the host a dispatch pass whether or not the callback does
/// anything, and that cost is invisible to script-time accounting. Charging
/// it here is what makes a fast timer expensive to the app that asked for
/// one — replacing an arbitrary floor on how fast a timer may tick with the
/// actual price of ticking that fast.
pub(crate) fn charge_wakeup(heap_key: usize) {
    charge_cpu(heap_key, WAKEUP_COST);
}

/// Tells the accounting how the frame is doing. The host calls this once per
/// rendered frame with the interval since the last one; without it, nothing
/// is ever considered contended and no app is ever trimmed.
pub fn note_frame(interval_s: f64) {
    WINDOW_STATE.with(|w| w.borrow_mut().note_frame(interval_s));
}

/// Sets the frame interval the host is trying to hold (default 60Hz).
pub fn set_frame_target(interval_s: f64) {
    if interval_s.is_finite() && interval_s > 0.0 {
        WINDOW_STATE.with(|w| w.borrow_mut().frame_target = interval_s);
    }
}

/// Whether the frame is currently being missed — exposed so a host can show
/// or log why apps are being trimmed.
pub fn is_contended() -> bool {
    WINDOW_STATE.with(|w| w.borrow().contended())
}

/// Instructions this isolate may execute in one entry.
pub(crate) fn entry_instructions(heap_key: usize) -> usize {
    limits_for_heap(heap_key).entry_instructions
}

/// Asks permission to create one more timer, and clamps its interval to the
/// isolate's floor.
///
/// Returns the interval to actually use, or `None` if the isolate already
/// holds as many timers as it may. The clamp is deliberately not a refusal: an
/// app asking for 1ms is usually just being greedy, and a 16ms timer still
/// works. A NEGATIVE or non-finite interval is not greed but nonsense — it
/// reaches `Duration::from_secs_f64` on several platform backends, which
/// panics — so it is clamped by the same path.
pub(crate) fn admit_timer(heap_key: usize, requested_s: f64) -> Option<f64> {
    let (refused, held, allowed) = STATES.with(|s| {
        let mut st = s.borrow_mut();
        st.entry(heap_key).or_default();
        let limits = st[&heap_key].limits;
        let held = st[&heap_key].timers;
        // Timer slots are a pool like any other: while the system is nowhere
        // near using them up, one app holding a lot of them costs nobody
        // anything. How FAST they tick is not rationed here at all — a wakeup
        // is charged to the app's processor share when it fires, which is the
        // price of ticking fast, rather than an arbitrary floor on how fast an
        // app is allowed to want to.
        let pool: u32 = st.values().map(|v| v.timers).sum();
        let weights: u64 = st
            .values()
            .filter(|v| v.timers > 0)
            .map(|v| v.limits.weight.max(1) as u64)
            .sum::<u64>()
            .max(limits.weight.max(1) as u64);
        let fraction = limits.weight.max(1) as f64 / weights as f64;
        let refused = held >= limits.timers_max
            || over_share(
                pool as f64 + 1.0,
                GLOBAL_TIMERS as f64,
                held as f64 + 1.0,
                fraction,
            );
        let allowed = if held >= limits.timers_max {
            limits.timers_max as u64
        } else {
            (GLOBAL_TIMERS as f64 * fraction) as u64
        };
        if !refused {
            st.get_mut(&heap_key).unwrap().timers += 1;
        }
        (refused, held, allowed)
    });
    if refused {
        record(heap_key, SplashLimitKind::Timers, held as u64 + 1, allowed);
        return None;
    }
    // The interval itself passes through untouched. Platform still refuses a
    // value that would panic a backend (negative, NaN, infinite) — that is a
    // validity check, not a policy about how often an app may work.
    Some(requested_s)
}

/// Gives back a timer slot when one is stopped or fires for the last time.
pub(crate) fn release_timer(heap_key: usize) {
    STATES.with(|s| {
        if let Some(state) = s.borrow_mut().get_mut(&heap_key) {
            state.timers = state.timers.saturating_sub(1);
        }
    });
}

/// Records a post-collection heap size and says whether the isolate may carry
/// on holding it.
///
/// Memory is shared the way CPU is: while every isolate together fits under
/// [`heap_pool()`], nobody is capped — a lone app on a quiet system may
/// use all of it. Past that watermark the isolate holding more than its
/// weighted share is the one reported, and its own backstop applies whatever
/// else is happening.
pub(crate) fn check_heap(heap_key: usize, live_slots: usize) -> bool {
    /// Collections an isolate may finish over its share before the launcher
    /// is told to stop it. Pressure first (cgroup `memory.high`), a stop only
    /// for something that will not come back down (`memory.max`).
    const PRESSURE_BEFORE_STOP: u32 = 3;

    let (verdict, live, allowed) = STATES.with(|s| {
        let mut st = s.borrow_mut();
        let state = st.entry(heap_key).or_default();
        state.heap_slots = live_slots;
        let limits = state.limits;

        // Its own backstop is absolute: a single isolate this large is a
        // runaway whatever the rest of the system is doing.
        if live_slots > limits.mem_max_slots {
            return (Some(limits.mem_max_slots), live_slots, limits.mem_max_slots);
        }

        let total: usize = st.values().map(|v| v.heap_slots).sum();
        let weights: u64 = st
            .values()
            .filter(|v| v.heap_slots > 0)
            .map(|v| v.limits.weight.max(1) as u64)
            .sum::<u64>()
            .max(limits.weight.max(1) as u64);
        let fraction = limits.weight.max(1) as f64 / weights as f64;
        let share = (heap_pool() as f64 * fraction) as usize;
        let over = over_share(
            total as f64,
            heap_pool() as f64,
            live_slots as f64,
            fraction,
        );

        let state = st.get_mut(&heap_key).unwrap();
        if !over {
            // Back inside its share: the pressure it was under is over.
            state.heap_pressure = 0;
            return (None, live_slots, share);
        }
        state.heap_pressure += 1;
        // Under pressure but not yet condemned: the host collects this
        // isolate harder, and an app that frees what it grabbed never
        // reaches the next step.
        if state.heap_pressure < PRESSURE_BEFORE_STOP {
            return (None, live_slots, share);
        }
        (Some(share), live_slots, share)
    });
    let Some(_) = verdict else {
        return true;
    };
    record(heap_key, SplashLimitKind::Memory, live as u64, allowed as u64);
    false
}

/// Whether this isolate is currently over its share of a full system, and so
/// should be collected harder than the round-robin would otherwise reach it.
/// The cgroup `memory.high` half: pressure, not a verdict.
pub(crate) fn under_memory_pressure(heap_key: usize) -> bool {
    STATES.with(|s| s.borrow().get(&heap_key).map(|st| st.heap_pressure > 0).unwrap_or(false))
}

/// Whether one more HTTP request may go out, given how many are already in
/// flight for this isolate.
pub(crate) fn admit_http(heap_key: usize, in_flight: usize) -> bool {
    let limits = limits_for_heap(heap_key);
    // The launcher tells us this isolate's own count; the pool is every
    // isolate's. Same rule again: a lone app downloading twenty things is
    // using capacity nobody else wants.
    let (pool, fraction) = STATES.with(|s| {
        let st = s.borrow();
        let pool: usize = st.values().map(|v| v.inflight_http).sum();
        let weights: u64 = st
            .values()
            .filter(|v| v.inflight_http > 0)
            .map(|v| v.limits.weight.max(1) as u64)
            .sum::<u64>()
            .max(limits.weight.max(1) as u64);
        (pool, limits.weight.max(1) as f64 / weights as f64)
    });
    STATES.with(|s| {
        s.borrow_mut().entry(heap_key).or_default().inflight_http = in_flight;
    });
    let refused = in_flight as u64 >= limits.http_max as u64
        || over_share(
            pool as f64 + 1.0,
            GLOBAL_INFLIGHT_HTTP as f64,
            in_flight as f64 + 1.0,
            fraction,
        );
    if !refused {
        return true;
    }
    let allowed = if in_flight as u64 >= limits.http_max as u64 {
        limits.http_max as u64
    } else {
        (GLOBAL_INFLIGHT_HTTP as f64 * fraction) as u64
    };
    record(heap_key, SplashLimitKind::Network, in_flight as u64 + 1, allowed);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: usize = 0xA;
    const B: usize = 0xB;

    fn reset() {
        STATES.with(|s| s.borrow_mut().clear());
        WINDOW_STATE.with(|w| *w.borrow_mut() = CpuWindow::default());
        EVENTS.with(|e| e.borrow_mut().clear());
        set_memory_pool(DEFAULT_HEAP_POOL);
    }

    fn full() -> Duration {
        Duration::from_millis(SplashLimits::default().entry_time_ms)
    }

    /// Frames arriving late enough, often enough, to count as contention —
    /// AND apps using enough of the window to be the reason.
    fn frames_are_slipping() {
        WINDOW_STATE.with(|w| {
            let mut w = w.borrow_mut();
            let blame = WINDOW.mul_f64(APP_BLAME_FRACTION * 1.5);
            if w.total < blame {
                w.total = blame;
            }
        });
        for _ in 0..20 {
            note_frame(DEFAULT_FRAME_TARGET * 4.0);
        }
    }

    fn frames_are_fine() {
        for _ in 0..20 {
            note_frame(DEFAULT_FRAME_TARGET);
        }
    }

    // ---- what contention even is ----------------------------------------

    /// Nothing drawing is not contention. It is quiet.
    #[test]
    fn silence_is_not_contention() {
        reset();
        assert!(!is_contended(), "no frames, no complaint");
        frames_are_fine();
        assert!(!is_contended(), "frames on time, nobody is waiting");
        frames_are_slipping();
        assert!(is_contended(), "the launcher is losing its frame to the apps");
    }

    /// A slow machine is not the apps' fault. Frames can be late because the
    /// renderer is slow or another process is thrashing, and trimming an app
    /// fixes none of that — it just punishes it for someone else's problem.
    #[test]
    fn missed_frames_alone_do_not_blame_the_apps() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        // Frames are dreadful, but the apps have barely run.
        charge_cpu(A, Duration::from_millis(5));
        for _ in 0..20 {
            note_frame(DEFAULT_FRAME_TARGET * 6.0);
        }
        assert!(!is_contended(), "the apps are not what is slow");
        assert_eq!(cpu_allowance(A), Some(full()), "so nothing is taken from them");
    }

    /// One late frame is not a verdict.
    #[test]
    fn a_single_slow_frame_does_not_trip_it() {
        reset();
        frames_are_fine();
        note_frame(DEFAULT_FRAME_TARGET * 8.0);
        assert!(!is_contended(), "frames are noisy; the signal is smoothed");
    }

    // ---- CPU -------------------------------------------------------------

    /// The headline property: an app is limited by nothing while the machine
    /// is keeping up, however much it uses.
    #[test]
    fn an_app_is_not_trimmed_while_the_frame_is_fine() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        frames_are_fine();
        for _ in 0..50 {
            assert_eq!(cpu_allowance(A), Some(full()), "nobody is waiting on anything");
            charge_cpu(A, full());
        }
        assert!(take_limit_events().is_empty(), "it has crossed nothing by running");
    }

    /// And it is trimmed exactly when the launcher starts losing frames.
    #[test]
    fn trimming_starts_when_frames_start_slipping() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        frames_are_fine();
        assert_eq!(cpu_allowance(A), Some(full()));
        frames_are_slipping();
        assert!(cpu_allowance(A).unwrap() < full(), "the launcher gets its frame back");
    }

    /// Five equally-weighted apps under contention get equal slices.
    #[test]
    fn five_apps_balance_out() {
        reset();
        let heaps = [1usize, 2, 3, 4, 5];
        for h in heaps {
            set_limits_for_heap(h, Some(SplashLimits::default()));
            charge_cpu(h, Duration::from_millis(1));
        }
        frames_are_slipping();
        let slices: Vec<_> = heaps.iter().map(|h| cpu_allowance(*h).unwrap()).collect();
        let (min, max) = (slices.iter().min().unwrap(), slices.iter().max().unwrap());
        assert_eq!(min, max, "five equals, five equal slices");
        assert!(*max < full(), "and all of them trimmed");
    }

    /// Weight decides who yields, and by how much.
    #[test]
    fn weight_decides_the_split() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits { weight: 16, ..Default::default() }));
        set_limits_for_heap(B, Some(SplashLimits { weight: 1, ..Default::default() }));
        charge_cpu(A, Duration::from_millis(1));
        charge_cpu(B, Duration::from_millis(1));
        frames_are_slipping();
        let (a, b) = (cpu_allowance(A).unwrap(), cpu_allowance(B).unwrap());
        assert!(a > b, "the heavyweight keeps the bigger slice: {a:?} vs {b:?}");
    }

    /// A quiet app holds nothing back for anyone else.
    #[test]
    fn quiet_apps_do_not_shrink_anyone_elses_share() {
        reset();
        for h in [1usize, 2, 3, 4, 5] {
            set_limits_for_heap(h, Some(SplashLimits::default()));
        }
        charge_cpu(1, Duration::from_millis(50));
        // Only just slipping: deep pressure would floor both slices and the
        // comparison below would be measuring the floor, not the share.
        WINDOW_STATE.with(|w| {
            let mut w = w.borrow_mut();
            let blame = WINDOW.mul_f64(APP_BLAME_FRACTION * 1.5);
            if w.total < blame {
                w.total = blame;
            }
        });
        frames_are_fine();
        for _ in 0..4 {
            note_frame(DEFAULT_FRAME_TARGET * 3.0);
        }
        // Same pressure either way, so this compares the SHARE and nothing
        // else: with four idle neighbours the busy app has the thread to
        // itself, and it only drops to a fifth once they are busy too.
        let alone_among_idlers = cpu_allowance(1).unwrap();
        for h in [2usize, 3, 4, 5] {
            charge_cpu(h, Duration::from_millis(50));
        }
        let sharing = cpu_allowance(1).unwrap();
        assert!(
            sharing < alone_among_idlers / 4,
            "idle neighbours must not count as competitors ({alone_among_idlers:?} -> {sharing:?})"
        );
    }

    /// `cpu.max`: absolute, and off unless a host asks for it.
    #[test]
    fn an_explicit_max_applies_even_on_an_idle_system() {
        reset();
        assert!(SplashLimits::default().cpu_max_ms.is_none(), "off by default");
        set_limits_for_heap(A, Some(SplashLimits { cpu_max_ms: Some(100), ..Default::default() }));
        frames_are_fine();
        charge_cpu(A, Duration::from_millis(120));
        assert!(cpu_allowance(A).unwrap() < full(), "a max does not care that frames are fine");
    }

    /// A wakeup costs its isolate, which is what replaced the arbitrary floor
    /// on how fast a timer may tick.
    #[test]
    fn a_wakeup_is_charged_to_whoever_asked_for_it() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        let before = WINDOW_STATE.with(|w| w.borrow().spent_by(A));
        charge_wakeup(A);
        let after = WINDOW_STATE.with(|w| w.borrow().spent_by(A));
        assert!(after > before, "waking the machine is not free");
    }

    /// The window forgets: an app trimmed a second ago starts even.
    #[test]
    fn the_window_rolls() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        charge_cpu(A, Duration::from_millis(500));
        WINDOW_STATE.with(|w| w.borrow_mut().started = Instant::now() - WINDOW);
        WINDOW_STATE.with(|w| assert!(w.borrow().spent_by(A) > Duration::ZERO));
        cpu_allowance(A);
        WINDOW_STATE.with(|w| assert!(w.borrow().spent_by(A).is_zero(), "clean start"));
    }

    // ---- memory ----------------------------------------------------------

    /// Same rule, space-multiplexed: a lone app may hold memory nobody else
    /// wants, far past any per-app number.
    #[test]
    fn one_app_may_hold_the_memory_nobody_is_using() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        assert!(check_heap(A, heap_pool() / 2));
        assert!(take_limit_events().is_empty(), "plenty spare, nothing to report");
    }

    /// The pool is the host's to size — this crate does not know how much
    /// memory the machine has.
    #[test]
    fn the_host_sizes_the_memory_pool() {
        reset();
        set_memory_pool(1000);
        set_limits_for_heap(A, Some(SplashLimits::default()));
        set_limits_for_heap(B, Some(SplashLimits::default()));
        check_heap(B, 400);
        // A is over half of a 1000-slot pool that is now full.
        assert!(check_heap(A, 900));
        assert!(check_heap(A, 900));
        assert!(!check_heap(A, 900), "pressure, then a verdict");
    }

    /// Over its share of a FULL system it gets pressure first, and is only
    /// given up on if it will not come back down.
    #[test]
    fn memory_pressure_comes_before_a_stop() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        set_limits_for_heap(B, Some(SplashLimits::default()));
        check_heap(B, heap_pool() / 4);
        let hog = heap_pool();
        assert!(check_heap(A, hog), "first collection over share: pressure");
        assert!(check_heap(A, hog), "second: still just pressure");
        assert!(!check_heap(A, hog), "third: it is not coming down");
        assert!(take_limit_events().iter().any(|e| e.kind == SplashLimitKind::Memory));
    }

    /// An app that frees what it grabbed never reaches the stop.
    #[test]
    fn giving_memory_back_clears_the_pressure() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        set_limits_for_heap(B, Some(SplashLimits::default()));
        check_heap(B, heap_pool() / 4);
        assert!(check_heap(A, heap_pool()));
        assert!(check_heap(A, 1000), "back inside its share");
        assert!(check_heap(A, heap_pool()), "pressure starts over, not where it left off");
        assert!(check_heap(A, heap_pool()));
    }

    /// `memory.max`: its own backstop is absolute and immediate.
    #[test]
    fn the_per_app_backstop_catches_a_runaway_at_once() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits { mem_max_slots: 1000, ..Default::default() }));
        assert!(!check_heap(A, 1001), "no pressure ladder for a single runaway isolate");
        assert_eq!(take_limit_events().len(), 1);
    }

    // ---- timers and requests --------------------------------------------

    /// An app may ask for any interval it likes; what it pays is the wakeups.
    #[test]
    fn intervals_are_not_second_guessed() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        assert_eq!(admit_timer(A, 0.001), Some(0.001), "1ms is the app's business");
        assert_eq!(admit_timer(A, 30.0), Some(30.0));
        assert!(take_limit_events().is_empty(), "wanting a fast timer is not a crossing");
    }

    /// A lone app may hold many timers; the cap is a backstop, not a budget.
    #[test]
    fn timers_are_pooled_not_rationed_per_app() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        for _ in 0..200 {
            assert!(admit_timer(A, 1.0).is_some(), "nobody else wants the slots");
        }
        assert!(take_limit_events().is_empty());
    }

    /// The per-app backstop still stops a hoarder.
    #[test]
    fn a_timer_hoarder_is_refused() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits { timers_max: 3, ..Default::default() }));
        for _ in 0..3 {
            assert!(admit_timer(A, 1.0).is_some());
        }
        assert_eq!(admit_timer(A, 1.0), None, "the fourth is refused");
        release_timer(A);
        assert!(admit_timer(A, 1.0).is_some(), "stopping one frees a slot");
    }

    /// Downloads are a pool too: alone, an app may have plenty in flight.
    #[test]
    fn requests_are_pooled() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        assert!(admit_http(A, 8), "eight at once is fine when nobody else is asking");
        assert!(!admit_http(A, SplashLimits::default().http_max as usize), "its own max binds");
    }

    // ---- housekeeping ----------------------------------------------------

    /// A dead isolate's limits, counters and pending events go with it — and
    /// its spend stops counting against everyone else's share.
    #[test]
    fn reclaiming_an_isolate_forgets_it() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits { timers_max: 1, ..Default::default() }));
        admit_timer(A, 1.0);
        admit_timer(A, 1.0); // refused, records an event
        charge_cpu(A, Duration::from_millis(500));
        gc_limits(&[A]);
        assert!(take_limit_events().is_empty(), "a dead isolate's events die with it");
        assert_eq!(limits_for_heap(A), SplashLimits::default());
        WINDOW_STATE.with(|w| assert!(w.borrow().total.is_zero(), "its spend leaves with it"));
    }

    /// A background surface yields under contention and is not otherwise
    /// second-class — no arbitrary ceiling for being a tile.
    #[test]
    fn background_yields_but_is_not_crippled() {
        let fg = SplashLimits::default();
        let bg = SplashLimits::background();
        assert!(bg.weight < fg.weight, "it yields when they compete");
        assert_eq!(bg.entry_instructions, fg.entry_instructions, "same work per entry");
        assert!(bg.cpu_max_ms.is_none(), "no absolute cap on a healthy system");
    }
}
