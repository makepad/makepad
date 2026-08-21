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

/// What all mini-apps together may spend of a window before ANYONE is
/// trimmed. Below this there is headroom, so limiting an isolate would slow
/// it down without speeding anything else up.
///
/// Not 100%: the rest is the launcher's own — drawing, input, the frame it
/// owes the user. An app going flat out should not be able to stop the
/// window it is drawn in from being painted.
const COLLECTIVE_BUDGET: Duration = Duration::from_millis(800);

/// Live heap slots ALL isolates together may hold before anyone is asked to
/// give some back. A lone app may use the lot.
const GLOBAL_HEAP_SLOTS: usize = 24_000_000;

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
    /// Shortest interval/timeout it may ask for, in seconds. Also the floor a
    /// nonsense value (negative, NaN, infinite) is clamped to.
    pub min_timer_interval_s: f64,
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
            mem_max_slots: GLOBAL_HEAP_SLOTS,
            timers_max: 256,
            min_timer_interval_s: 0.016,
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
            min_timer_interval_s: 0.1,
            // A tile has no business ballooning even on an idle system.
            mem_max_slots: GLOBAL_HEAP_SLOTS / 3,
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
    /// Asked for a timer faster than the floor (the request was clamped, not
    /// refused — a clamped timer is still a working app).
    TimerInterval,
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
            SplashLimitKind::TimerInterval => "timer-interval",
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

/// The shared CPU window. One window for every isolate, or shares would not
/// be comparable.
struct CpuWindow {
    started: Instant,
    /// Wall-clock spent in script this window, per heap.
    spent: HashMap<usize, Duration>,
    total: Duration,
}

impl Default for CpuWindow {
    fn default() -> Self {
        Self { started: Instant::now(), spent: HashMap::new(), total: Duration::ZERO }
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

    /// This heap's slice of the collective budget, by weight.
    fn share_of(&self, heap_key: usize, states: &HashMap<usize, IsolateState>) -> Duration {
        COLLECTIVE_BUDGET.mul_f64(self.share_fraction(heap_key, states).0)
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
            let mine = w.spent_by(heap_key);
            let (fraction, active) = w.share_fraction(heap_key, &states);

            // The trimmed slice. Two jobs at once: leave the launcher enough
            // of the window to draw the frame this app is being drawn in, and
            // keep the weights meaningful while doing it — a heavyweight over
            // budget should still run faster than a lightweight over budget,
            // or the weight stops mattering exactly when it matters most.
            // `fraction * active` is 1.0 for an even split, so an even split
            // gives everyone the same trim.
            let scale = fraction * active.max(1) as f64;
            let trimmed = (entry / 8)
                .mul_f64(scale)
                .max(Duration::from_millis(4))
                .min(entry);

            // An absolute ceiling does not care who else is running, which is
            // exactly why it is off by default.
            if let Some(cap) = limits.cpu_max_ms {
                if mine >= Duration::from_millis(cap) {
                    return Some(trimmed);
                }
            }
            if over_share(
                w.total.as_secs_f64(),
                COLLECTIVE_BUDGET.as_secs_f64(),
                mine.as_secs_f64(),
                fraction,
            ) {
                return Some(trimmed);
            }
            // Either there is headroom or this isolate is inside its share:
            // both mean trimming it would slow it down for nobody's benefit.
            Some(entry)
        })
    })
}

/// Charges what an entry actually took against the shared window.
pub(crate) fn charge_cpu(heap_key: usize, spent: Duration) {
    let now = Instant::now();
    let (yielded, mine, share) = WINDOW_STATE.with(|w| {
        let mut w = w.borrow_mut();
        w.roll(now);
        *w.spent.entry(heap_key).or_default() += spent;
        w.total += spent;
        let mine = w.spent_by(heap_key);
        let (fraction, _) = STATES.with(|s| w.share_fraction(heap_key, &s.borrow()));
        let yielded = over_share(
            w.total.as_secs_f64(),
            COLLECTIVE_BUDGET.as_secs_f64(),
            mine.as_secs_f64(),
            fraction,
        );
        (yielded, mine, COLLECTIVE_BUDGET.mul_f64(fraction))
    });
    // Only worth telling the host about when it means something: the thread
    // was full AND this isolate was the one over its share of it.
    if yielded {
        record(
            heap_key,
            SplashLimitKind::Cpu,
            mine.as_millis() as u64,
            share.as_millis() as u64,
        );
    }
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
    let (refused, limits, held, allowed) = STATES.with(|s| {
        let mut st = s.borrow_mut();
        st.entry(heap_key).or_default();
        let limits = st[&heap_key].limits;
        let held = st[&heap_key].timers;
        // Timer slots are a pool like any other: while the system as a whole
        // is nowhere near using them up, one app holding a lot of them costs
        // nobody anything.
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
        (refused, limits, held, allowed)
    });
    if refused {
        record(heap_key, SplashLimitKind::Timers, held as u64 + 1, allowed);
        return None;
    }
    let floor = limits.min_timer_interval_s;
    let clamped = if requested_s.is_finite() && requested_s > floor {
        requested_s
    } else {
        floor
    };
    if clamped != requested_s {
        record(
            heap_key,
            SplashLimitKind::TimerInterval,
            (requested_s.max(0.0) * 1000.0) as u64,
            (floor * 1000.0) as u64,
        );
    }
    Some(clamped)
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
/// [`GLOBAL_HEAP_SLOTS`], nobody is capped — a lone app on a quiet system may
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
        let share = (GLOBAL_HEAP_SLOTS as f64 * fraction) as usize;
        let over = over_share(
            total as f64,
            GLOBAL_HEAP_SLOTS as f64,
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
    }

    fn full() -> Duration {
        Duration::from_millis(SplashLimits::default().entry_time_ms)
    }

    // ---- the rule itself -------------------------------------------------

    /// Both halves are required. A full pool where everyone is inside their
    /// share is a busy system working; an isolate over its share of a pool
    /// with room to spare is costing nobody anything.
    #[test]
    fn nothing_yields_unless_the_pool_is_full_and_it_is_over() {
        assert!(!over_share(50.0, 100.0, 50.0, 0.1), "room to spare, no matter who holds it");
        assert!(!over_share(150.0, 100.0, 5.0, 0.5), "full, but this one is well inside its share");
        assert!(over_share(150.0, 100.0, 80.0, 0.5), "full AND over: yield");
    }

    // ---- CPU -------------------------------------------------------------

    /// The headline property: one mini-app on its own is never trimmed for
    /// being the only one running.
    #[test]
    fn one_app_alone_gets_the_machine() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        // Far more than any static per-app quota would ever have allowed.
        for _ in 0..10 {
            assert_eq!(cpu_allowance(A), Some(full()), "nothing to share with");
            charge_cpu(A, full());
        }
        assert!(take_limit_events().is_empty(), "it has crossed nothing by running");
    }

    /// ...but the launcher still gets to draw: past the collective budget
    /// even a lone app yields some of the window back.
    #[test]
    fn the_launcher_keeps_its_own_air() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        let mut spent = Duration::ZERO;
        while spent < COLLECTIVE_BUDGET {
            charge_cpu(A, full());
            spent += full();
        }
        assert!(cpu_allowance(A).unwrap() < full(), "a runaway is trimmed even alone");
    }

    /// Five equally-weighted apps, all busy, end up level with each other.
    #[test]
    fn five_apps_balance_out() {
        reset();
        let heaps = [1usize, 2, 3, 4, 5];
        for h in heaps {
            set_limits_for_heap(h, Some(SplashLimits::default()));
        }
        let mut spent = [Duration::ZERO; 5];
        for _ in 0..100 {
            for (i, h) in heaps.iter().enumerate() {
                let slice = cpu_allowance(*h).unwrap();
                charge_cpu(*h, slice);
                spent[i] += slice;
            }
        }
        let (min, max) = (spent.iter().min().unwrap(), spent.iter().max().unwrap());
        assert!(
            *max - *min < full(),
            "five equals should stay level; got {min:?}..{max:?}"
        );
        // And they are actually being rationed rather than running free.
        assert!(cpu_allowance(1).unwrap() < full());
    }

    /// Weight is what decides who yields, and only while they compete.
    #[test]
    fn weight_decides_who_yields() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default())); // weight 4
        set_limits_for_heap(B, Some(SplashLimits::background())); // weight 1
        // Both busy, window full, split evenly so far.
        charge_cpu(A, COLLECTIVE_BUDGET.mul_f64(0.5));
        charge_cpu(B, COLLECTIVE_BUDGET.mul_f64(0.5));
        assert_eq!(cpu_allowance(A), Some(full()), "4/5 share: still inside it");
        assert!(cpu_allowance(B).unwrap() < full(), "1/5 share: over it, yields");
    }

    /// A trimmed heavyweight still outruns a trimmed lightweight, or the
    /// weight stops meaning anything exactly when it matters most.
    #[test]
    fn trimming_stays_proportional() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits { weight: 16, ..Default::default() }));
        set_limits_for_heap(B, Some(SplashLimits { weight: 1, ..Default::default() }));
        charge_cpu(A, COLLECTIVE_BUDGET);
        charge_cpu(B, COLLECTIVE_BUDGET);
        let (a, b) = (cpu_allowance(A).unwrap(), cpu_allowance(B).unwrap());
        assert!(a > b, "over budget, the heavyweight still gets the bigger slice: {a:?} vs {b:?}");
    }

    /// An idle isolate holds nothing back for anyone else.
    #[test]
    fn quiet_apps_do_not_shrink_anyone_elses_share() {
        reset();
        for h in [1usize, 2, 3, 4, 5] {
            set_limits_for_heap(h, Some(SplashLimits::default()));
        }
        charge_cpu(1, COLLECTIVE_BUDGET.mul_f64(0.9));
        assert_eq!(
            cpu_allowance(1),
            Some(full()),
            "four registered-but-quiet apps must not cost the busy one its share"
        );
    }

    /// `cpu.max`: absolute, and off unless a host asks for it.
    #[test]
    fn an_explicit_max_applies_even_on_an_idle_system() {
        reset();
        assert!(SplashLimits::default().cpu_max_ms.is_none(), "off by default");
        set_limits_for_heap(A, Some(SplashLimits { cpu_max_ms: Some(100), ..Default::default() }));
        charge_cpu(A, Duration::from_millis(120));
        assert!(cpu_allowance(A).unwrap() < full(), "a max does not care that the system is idle");
    }

    /// The window forgets: an app trimmed a second ago starts even.
    #[test]
    fn the_window_rolls() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        charge_cpu(A, COLLECTIVE_BUDGET);
        WINDOW_STATE.with(|w| w.borrow_mut().started = Instant::now() - WINDOW);
        assert_eq!(cpu_allowance(A), Some(full()), "a new window is a clean start");
    }

    // ---- memory ----------------------------------------------------------

    /// Same rule, space-multiplexed: a lone app may hold memory nobody else
    /// wants, far past any per-app number.
    #[test]
    fn one_app_may_hold_the_memory_nobody_is_using() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        assert!(check_heap(A, GLOBAL_HEAP_SLOTS / 2));
        assert!(take_limit_events().is_empty(), "plenty spare, so nothing to report");
    }

    /// Over its share of a FULL system it gets pressure first, and is only
    /// given up on if it will not come back down.
    #[test]
    fn memory_pressure_comes_before_a_stop() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        set_limits_for_heap(B, Some(SplashLimits::default()));
        // Between them they have filled the pool; A holds far more.
        check_heap(B, GLOBAL_HEAP_SLOTS / 4);
        let hog = GLOBAL_HEAP_SLOTS;
        assert!(check_heap(A, hog), "first collection over share: pressure, not a stop");
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
        check_heap(B, GLOBAL_HEAP_SLOTS / 4);
        assert!(check_heap(A, GLOBAL_HEAP_SLOTS));
        assert!(check_heap(A, 1000), "back inside its share");
        assert!(check_heap(A, GLOBAL_HEAP_SLOTS), "pressure starts over, not where it left off");
        assert!(check_heap(A, GLOBAL_HEAP_SLOTS));
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

    /// A lone app may hold many timers; the cap is a backstop, not a budget.
    #[test]
    fn timers_are_pooled_not_rationed_per_app() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits::default()));
        for _ in 0..64 {
            assert!(admit_timer(A, 1.0).is_some(), "nobody else wants the slots");
        }
        assert!(take_limit_events().is_empty());
    }

    /// Nonsense intervals are clamped rather than passed to a platform
    /// backend that would panic on them.
    #[test]
    fn intervals_are_floored() {
        reset();
        set_limits_for_heap(A, Some(SplashLimits { min_timer_interval_s: 0.05, ..Default::default() }));
        assert_eq!(admit_timer(A, 1.0), Some(1.0), "a sane interval is untouched");
        assert_eq!(admit_timer(A, 0.001), Some(0.05), "too fast is floored");
        assert_eq!(admit_timer(A, -1.0), Some(0.05), "negative is floored, not passed on");
        assert_eq!(admit_timer(A, f64::NAN), Some(0.05));
        assert_eq!(admit_timer(A, f64::INFINITY), Some(0.05));
        assert!(take_limit_events().iter().any(|e| e.kind == SplashLimitKind::TimerInterval));
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
        assert!(!admit_http(A, SplashLimits::default().http_max as usize), "its own max still binds");
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
        charge_cpu(A, COLLECTIVE_BUDGET);
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
        assert!(bg.cpu_max_ms.is_none(), "no absolute cap on an idle system");
    }
}
