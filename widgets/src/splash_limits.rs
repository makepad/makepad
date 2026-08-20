//! Per-isolate resource limits for Splash mini-apps.
//!
//! A Splash isolate already cannot reach the filesystem, the network, or
//! another isolate's heap without the host handing it those things. What it
//! could still do was *cost* whatever it liked: the VM's budgets are per
//! ENTRY, so a script that arranges to be entered often — a fast interval, a
//! network callback, a chain of widget events — got a fresh 64ms and a fresh
//! 200k instructions every time, forever. Nothing capped an isolate's heap,
//! its timer count, or its share of the frame.
//!
//! This module holds the numbers that answer "how much", per isolate, and the
//! accounting to enforce them. It follows the same shape as the storage jail
//! (`splash_storage`) and the host bridge (`splash_host`): host-assigned state
//! in a thread-local keyed by the isolate's heap, which script cannot read or
//! retarget, cleaned up when the isolate is reclaimed.
//!
//! Defaults are chosen to be invisible to an app doing its job — they sit
//! roughly an order of magnitude above what the mini-apps in this repo's own
//! sample set use — while a runaway one hits them in well under a second. A
//! host that wants different numbers sets them per isolate; a host that says
//! nothing gets [`SplashLimits::default`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How much one Splash isolate may consume.
///
/// Every field is a hard number rather than a level ("strict"/"relaxed"),
/// because the right value depends on what the surface is for: a foreground
/// app the user is looking at deserves more of the frame than a home-screen
/// tile that has to share it with eleven others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplashLimits {
    /// Wall-clock an isolate may spend in ONE entry into script.
    pub entry_time_ms: u64,
    /// Instructions an isolate may execute in ONE entry.
    pub entry_instructions: usize,
    /// The window over which cumulative CPU is measured.
    pub cpu_window: Duration,
    /// Wall-clock the isolate may spend in script per `cpu_window`, summed
    /// across every entry. This is the limit that per-entry budgets cannot
    /// express: ten cheap entries a frame are ten times the cost of one.
    pub cpu_per_window_ms: u64,
    /// Live script timers the isolate may hold at once.
    pub max_timers: u32,
    /// Shortest interval/timeout it may ask for, in seconds. Also the floor a
    /// nonsense value (negative, NaN, infinite) is clamped to.
    pub min_timer_interval_s: f64,
    /// Live heap slots (objects + arrays + strings + pods + handles) the
    /// isolate may hold after a collection. Slots rather than bytes because
    /// that is what the GC already counts; a script that keeps allocating
    /// without releasing crosses it, one that reuses its memory never does.
    pub live_heap_slots: usize,
    /// Concurrent in-flight HTTP requests. Only meaningful for an isolate the
    /// host gave network to at all.
    pub max_inflight_http: u32,
}

impl Default for SplashLimits {
    fn default() -> Self {
        Self {
            // The values the widgets crate has always applied, now nameable.
            entry_time_ms: 64,
            entry_instructions: 200_000,
            // A quarter of one core, averaged over a second. An app that
            // redraws on a timer sits far below this; a busy loop that bails
            // and immediately re-enters sits far above it.
            cpu_window: Duration::from_secs(1),
            cpu_per_window_ms: 250,
            // The busiest sample app holds three timers; the fastest asks for
            // 100ms. Both defaults are an order of magnitude clear of that.
            max_timers: 32,
            min_timer_interval_s: 0.016,
            // ~10x the largest heap any sample app settles at.
            live_heap_slots: 2_000_000,
            max_inflight_http: 8,
        }
    }
}

impl SplashLimits {
    /// Limits for a surface that runs while the user is looking at something
    /// else — a home-screen tile, a preview. Same rules, smaller share: such a
    /// surface competes with every other tile for one frame, and is by
    /// definition not what the user is waiting on.
    pub fn background() -> Self {
        Self {
            entry_time_ms: 16,
            cpu_per_window_ms: 60,
            max_timers: 8,
            min_timer_interval_s: 0.1,
            live_heap_slots: 500_000,
            max_inflight_http: 2,
            ..Self::default()
        }
    }
}

/// Which limit an isolate crossed. Reported to the host, which decides what a
/// repeat offender deserves — this module only measures and refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplashLimitKind {
    /// Burned its whole CPU allowance for the current window.
    Cpu,
    /// Asked for more timers than it may hold.
    Timers,
    /// Asked for a timer faster than the floor (the request was clamped, not
    /// refused — a clamped timer is still a working app).
    TimerInterval,
    /// Still holding more heap than allowed after a collection.
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
    /// understand ("wanted 4000 timers, allowed 32").
    pub wanted: u64,
    pub allowed: u64,
}

/// Per-isolate limits and the running counts they are checked against.
#[derive(Default)]
struct LimitState {
    limits: SplashLimits,
    /// Start of the current CPU window and what has been spent in it.
    window_start: Option<Instant>,
    cpu_spent: Duration,
    /// Live timers this isolate holds.
    timers: u32,
    /// Crossings not yet collected by the host.
    events: Vec<SplashLimitEvent>,
}

thread_local! {
    /// heap key -> that isolate's limits and counters. Keyed by heap for the
    /// same reason the storage jail is: a script has no way to name, read or
    /// retarget it.
    static LIMITS: RefCell<HashMap<usize, LimitState>> = RefCell::new(HashMap::new());
    /// Crossings from every isolate, in order, awaiting [`take_limit_events`].
    static EVENTS: RefCell<Vec<SplashLimitEvent>> = const { RefCell::new(Vec::new()) };
}

/// Installs (or clears) an isolate's limits. Host-only: called from
/// `Splash::set_limits`, never reachable from script.
pub(crate) fn set_limits_for_heap(heap_key: usize, limits: Option<SplashLimits>) {
    LIMITS.with(|l| {
        let mut l = l.borrow_mut();
        match limits {
            Some(limits) => l.entry(heap_key).or_default().limits = limits,
            // Back to the defaults, and forget the counters with them: a host
            // clearing limits is not trying to keep a grudge.
            None => {
                l.remove(&heap_key);
            }
        }
    });
}

/// The limits in force for a heap — the defaults when the host set none.
pub fn limits_for_heap(heap_key: usize) -> SplashLimits {
    LIMITS.with(|l| {
        l.borrow()
            .get(&heap_key)
            .map(|s| s.limits)
            .unwrap_or_default()
    })
}

/// Drops the limits and counters of reclaimed isolates. Called from
/// `gc_dead_splash_isolates` alongside the storage roots and bridge state.
pub(crate) fn gc_limits(dead_heaps: &[usize]) {
    LIMITS.with(|l| {
        let mut l = l.borrow_mut();
        for heap in dead_heaps {
            l.remove(heap);
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

/// Whether this isolate has any CPU allowance left in the current window, and
/// how much of one entry's wall-clock it may use.
///
/// Returns `None` when the isolate is out of allowance — the caller skips the
/// entry entirely. That is the point of a cumulative budget: a script that
/// spends its second can be refused the frame, not merely trimmed.
pub(crate) fn cpu_allowance(heap_key: usize) -> Option<Duration> {
    LIMITS.with(|l| {
        let mut l = l.borrow_mut();
        let state = l.entry(heap_key).or_default();
        let limits = state.limits;
        let now = Instant::now();
        let started = *state.window_start.get_or_insert(now);
        if now.duration_since(started) >= limits.cpu_window {
            state.window_start = Some(now);
            state.cpu_spent = Duration::ZERO;
        }
        let budget = Duration::from_millis(limits.cpu_per_window_ms);
        if state.cpu_spent >= budget {
            return None;
        }
        // Never hand out more than one entry's worth, nor more than what is
        // left of the window.
        let entry = Duration::from_millis(limits.entry_time_ms);
        Some(entry.min(budget - state.cpu_spent))
    })
}

/// Charges what an entry actually took against the isolate's window.
pub(crate) fn charge_cpu(heap_key: usize, spent: Duration) {
    LIMITS.with(|l| {
        let mut l = l.borrow_mut();
        let Some(state) = l.get_mut(&heap_key) else {
            return;
        };
        state.cpu_spent += spent;
        let budget = Duration::from_millis(state.limits.cpu_per_window_ms);
        if state.cpu_spent >= budget {
            let spent_ms = state.cpu_spent.as_millis() as u64;
            let allowed = state.limits.cpu_per_window_ms;
            drop(l);
            record(heap_key, SplashLimitKind::Cpu, spent_ms, allowed);
        }
    });
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
    LIMITS.with(|l| {
        let mut l = l.borrow_mut();
        let state = l.entry(heap_key).or_default();
        let limits = state.limits;
        if state.timers >= limits.max_timers {
            let wanted = state.timers as u64 + 1;
            drop(l);
            record(heap_key, SplashLimitKind::Timers, wanted, limits.max_timers as u64);
            return None;
        }
        state.timers += 1;
        let floor = limits.min_timer_interval_s;
        let clamped = if requested_s.is_finite() && requested_s > floor {
            requested_s
        } else {
            floor
        };
        if clamped != requested_s {
            drop(l);
            record(
                heap_key,
                SplashLimitKind::TimerInterval,
                (requested_s.max(0.0) * 1000.0) as u64,
                (floor * 1000.0) as u64,
            );
        }
        Some(clamped)
    })
}

/// Gives back a timer slot when one is stopped or fires for the last time.
pub(crate) fn release_timer(heap_key: usize) {
    LIMITS.with(|l| {
        if let Some(state) = l.borrow_mut().get_mut(&heap_key) {
            state.timers = state.timers.saturating_sub(1);
        }
    });
}

/// Checks a post-collection heap size against the isolate's ceiling.
/// `live_slots` is what the GC just counted. Reports a crossing; the caller
/// decides whether to stop the isolate.
pub(crate) fn check_heap(heap_key: usize, live_slots: usize) -> bool {
    let limits = limits_for_heap(heap_key);
    if live_slots <= limits.live_heap_slots {
        return true;
    }
    record(
        heap_key,
        SplashLimitKind::Memory,
        live_slots as u64,
        limits.live_heap_slots as u64,
    );
    false
}

/// Whether one more HTTP request may go out, given how many are already in
/// flight for this isolate.
pub(crate) fn admit_http(heap_key: usize, in_flight: usize) -> bool {
    let limits = limits_for_heap(heap_key);
    if (in_flight as u64) < limits.max_inflight_http as u64 {
        return true;
    }
    record(
        heap_key,
        SplashLimitKind::Network,
        in_flight as u64 + 1,
        limits.max_inflight_http as u64,
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: usize = 0xBEEF;

    fn reset() {
        LIMITS.with(|l| l.borrow_mut().clear());
        EVENTS.with(|e| e.borrow_mut().clear());
    }

    /// An isolate with no host-set limits still has limits.
    #[test]
    fn defaults_apply_without_a_host() {
        reset();
        let l = limits_for_heap(H);
        assert_eq!(l, SplashLimits::default());
        assert!(l.max_timers > 0 && l.cpu_per_window_ms > 0);
    }

    /// The cumulative window is the thing per-entry budgets cannot express:
    /// enough entries and the isolate is refused the next one outright.
    #[test]
    fn cpu_runs_out_across_many_entries() {
        reset();
        set_limits_for_heap(H, Some(SplashLimits::default()));
        let entry = Duration::from_millis(64);
        let mut entries = 0;
        while cpu_allowance(H).is_some() {
            charge_cpu(H, entry);
            entries += 1;
            assert!(entries < 100, "the window must close");
        }
        // 250ms of allowance at 64ms an entry.
        assert_eq!(entries, 4);
        let events = take_limit_events();
        assert!(events.iter().any(|e| e.kind == SplashLimitKind::Cpu));
    }

    /// One entry never gets more than its own share, even with a full window.
    #[test]
    fn one_entry_cannot_take_the_whole_window() {
        reset();
        let allowance = cpu_allowance(H).expect("fresh window");
        assert_eq!(allowance, Duration::from_millis(64));
    }

    /// Timers are capped, and a nonsense interval is clamped rather than
    /// passed to a platform backend that would panic on it.
    #[test]
    fn timers_are_capped_and_intervals_floored() {
        reset();
        let limits = SplashLimits { max_timers: 3, min_timer_interval_s: 0.05, ..Default::default() };
        set_limits_for_heap(H, Some(limits));

        assert_eq!(admit_timer(H, 1.0), Some(1.0), "a sane interval is untouched");
        assert_eq!(admit_timer(H, 0.001), Some(0.05), "too fast is floored");
        assert_eq!(admit_timer(H, -1.0), Some(0.05), "negative is floored, not passed on");
        assert_eq!(admit_timer(H, 1.0), None, "the fourth timer is refused");

        release_timer(H);
        assert_eq!(admit_timer(H, 2.0), Some(2.0), "stopping one frees a slot");

        let kinds: Vec<_> = take_limit_events().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&SplashLimitKind::Timers));
        assert!(kinds.contains(&SplashLimitKind::TimerInterval));
    }

    /// NaN and infinity are the other two shapes of nonsense.
    #[test]
    fn non_finite_intervals_are_floored() {
        reset();
        assert_eq!(admit_timer(H, f64::NAN), Some(0.016));
        assert_eq!(admit_timer(H, f64::INFINITY), Some(0.016));
        assert_eq!(admit_timer(H, f64::NEG_INFINITY), Some(0.016));
    }

    /// The heap ceiling is checked against what a collection actually left.
    #[test]
    fn heap_ceiling_reports_when_crossed() {
        reset();
        set_limits_for_heap(H, Some(SplashLimits { live_heap_slots: 1000, ..Default::default() }));
        assert!(check_heap(H, 999));
        assert!(take_limit_events().is_empty());
        assert!(!check_heap(H, 1001));
        assert_eq!(take_limit_events().len(), 1);
    }

    /// A dead isolate's limits, counters and pending events go with it.
    #[test]
    fn reclaiming_an_isolate_forgets_it() {
        reset();
        set_limits_for_heap(H, Some(SplashLimits { max_timers: 1, ..Default::default() }));
        admit_timer(H, 1.0);
        admit_timer(H, 1.0); // refused, records an event
        gc_limits(&[H]);
        assert!(take_limit_events().is_empty(), "a dead isolate's events die with it");
        assert_eq!(limits_for_heap(H), SplashLimits::default());
    }

    /// A background surface gets a smaller share than a foreground one, which
    /// is the whole reason these are numbers and not a flag.
    #[test]
    fn background_is_stricter_than_foreground() {
        let fg = SplashLimits::default();
        let bg = SplashLimits::background();
        assert!(bg.cpu_per_window_ms < fg.cpu_per_window_ms);
        assert!(bg.max_timers < fg.max_timers);
        assert!(bg.min_timer_interval_s > fg.min_timer_interval_s);
        assert!(bg.live_heap_slots < fg.live_heap_slots);
    }
}
