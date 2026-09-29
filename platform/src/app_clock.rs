//! Opt-in deterministic application time, for frame-exact captures.
//!
//! `--virtual-clock` (or `MAKEPAD_VIRTUAL_CLOCK=1`) puts one clock behind
//! everything an app reads as time: `seconds_since_app_start`,
//! `Cx::time_now`, the `NextFrame` and `Draw` stamps, pass uniforms and the
//! `start_timeout` / `start_interval` timers. It starts at zero and moves
//! only when the remote bridge's `/step` runs a frame, by exactly one frame
//! period. Without the flag none of this is consulted beyond one relaxed
//! load. Transport deadlines (HTTP, GPU watchdogs) keep using `Instant`.
//!
//! The same launch configuration carries `--window WxH@scale` or `MAKEPAD_WINDOW` (the first
//! window's logical size and dpi scale) and `--seed N` / `MAKEPAD_SEED`
//! (Splash `random`), the other two inputs a reproducible capture pins.
// No remote bridge on these targets, so nothing steps the clock there.
#![cfg_attr(
    any(target_arch = "wasm32", target_os = "android", target_env = "ohos"),
    allow(dead_code)
)]
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

/// `Cx::time_now` under the virtual clock: seconds since the Unix epoch of
/// 2026-01-01T00:00:00Z plus the virtual time, so a date an app formats is
/// plausible and the same on every run.
pub const VIRTUAL_EPOCH: f64 = 1_767_225_600.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchWindow {
    /// Logical (layout) size in points.
    pub width: f64,
    pub height: f64,
    /// Layout dpi scale: the window is `width * scale` by `height * scale`
    /// physical pixels.
    pub scale: f64,
}

impl LaunchWindow {
    /// The native (OS point) inner size that gives this window its layout
    /// size once `scale` overrides a display running at `native_dpi`.
    pub fn native_size(&self, native_dpi: f64) -> (f64, f64) {
        let native_dpi = if native_dpi.is_finite() && native_dpi > 0.0 { native_dpi } else { 1.0 };
        (self.width * self.scale / native_dpi, self.height * self.scale / native_dpi)
    }
}

/// `WxH@scale`, every part finite and positive.
pub fn parse_window(value: &str) -> Option<LaunchWindow> {
    let (size, scale) = value.split_once('@')?;
    let (width, height) = size.split_once('x')?;
    let window = LaunchWindow {
        width: width.parse().ok()?,
        height: height.parse().ok()?,
        scale: scale.parse().ok()?,
    };
    [window.width, window.height, window.scale]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0)
        .then_some(window)
}

#[derive(Default, Debug, PartialEq)]
struct Configuration {
    virtual_clock: bool,
    window: Option<LaunchWindow>,
    seed: Option<u64>,
}

fn parse_configuration(
    args: impl Iterator<Item = String>,
    virtual_env: Option<String>,
    seed_env: Option<String>,
) -> Configuration {
    let mut config = Configuration {
        virtual_clock: virtual_env.is_some_and(|v| v == "1"),
        seed: seed_env.and_then(|v| v.trim().parse().ok()),
        window: None,
    };
    let mut args = args.skip(1);
    while let Some(arg) = args.next() {
        if arg == "--virtual-clock" {
            config.virtual_clock = true;
        } else if arg == "--window" {
            config.window = args.next().as_deref().and_then(parse_window);
        } else if let Some(value) = arg.strip_prefix("--window=") {
            config.window = parse_window(value);
        } else if arg == "--seed" {
            config.seed = args.next().and_then(|v| v.parse().ok());
        } else if let Some(value) = arg.strip_prefix("--seed=") {
            config.seed = value.parse().ok();
        }
    }
    config
}

fn configuration() -> &'static Configuration {
    static CONFIG: OnceLock<Configuration> = OnceLock::new();
    CONFIG.get_or_init(|| {
        #[cfg(target_arch = "wasm32")]
        {
            Configuration::default()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut config = parse_configuration(
                std::env::args(),
                std::env::var("MAKEPAD_VIRTUAL_CLOCK").ok(),
                std::env::var("MAKEPAD_SEED").ok(),
            );
            // MAKEPAD_WINDOW=WxH@scale: `--window` for launchers that pass
            // env but not arguments.
            if config.window.is_none() {
                config.window = std::env::var("MAKEPAD_WINDOW").ok().as_deref().and_then(parse_window);
            }
            // Only the remote bridge's `/step` moves the clock: without it
            // (a child that inherited MAKEPAD_VIRTUAL_CLOCK, a forgotten
            // --remote) the app would freeze, so the clock stays real.
            if config.virtual_clock && !super::requested() {
                crate::log!("--virtual-clock / MAKEPAD_VIRTUAL_CLOCK ignored: it needs --remote (only /step advances it)");
                config.virtual_clock = false;
            }
            config
        }
    })
}

static TIME: AtomicU64 = AtomicU64::new(0);
static FRAME: AtomicU64 = AtomicU64::new(0);
static FPS: AtomicU64 = AtomicU64::new(60);
/// The window `--window` was given to, once one claimed it (id + 1).
static LAUNCH_WINDOW_CLAIM: AtomicUsize = AtomicUsize::new(0);

pub fn enabled() -> bool {
    configuration().virtual_clock
}

/// The virtual time in seconds, when the clock is on.
pub fn now() -> Option<f64> {
    enabled().then(|| f64::from_bits(TIME.load(Ordering::Acquire)))
}

/// `Cx::time_now` under the virtual clock (see [`VIRTUAL_EPOCH`]).
pub fn epoch_now() -> Option<f64> {
    now().map(|time| VIRTUAL_EPOCH + time)
}

/// Frames `/step` has run.
pub fn frame() -> u64 {
    FRAME.load(Ordering::Acquire)
}

/// The rate of the last step.
pub fn fps() -> u32 {
    FPS.load(Ordering::Acquire) as u32
}

pub fn seed() -> Option<u64> {
    configuration().seed
}

/// `--window` for this window: the first window to ask claims it, and only
/// that one gets it (a second top-level window keeps its own size).
pub fn launch_window_for(window_id: usize) -> Option<LaunchWindow> {
    let window = configuration().window?;
    let claim = window_id + 1;
    match LAUNCH_WINDOW_CLAIM.compare_exchange(0, claim, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(window),
        Err(owner) if owner == claim => Some(window),
        Err(_) => None,
    }
}

/// Advance by one frame at `fps`: the same sum of frame periods on every
/// run, and a rate change continues from where the previous rate left off.
pub(crate) fn advance(fps: u32) -> (u64, f64) {
    let fps = fps.max(1);
    let previous = f64::from_bits(TIME.load(Ordering::Relaxed));
    let time = previous + 1.0 / fps as f64;
    TIME.store(time.to_bits(), Ordering::Release);
    FPS.store(fps as u64, Ordering::Release);
    (FRAME.fetch_add(1, Ordering::AcqRel) + 1, time)
}

#[derive(Clone, Copy)]
struct Timer {
    due: f64,
    interval: f64,
    repeats: bool,
}

/// The app's timers while the virtual clock runs. They fire inside `/step`
/// frames, in due order, with the virtual time.
#[derive(Default)]
pub struct AppClock {
    timers: BTreeMap<u64, Timer>,
    /// True while `/step` dispatches a frame: the only time a `NextFrame` is
    /// delivered under the virtual clock.
    pub(crate) dispatching_frame: bool,
}

impl AppClock {
    pub(crate) fn start_timer(&mut self, id: u64, interval: f64, repeats: bool, now: f64) {
        let interval = if interval.is_finite() { interval.max(0.0) } else { 0.0 };
        self.timers.insert(id, Timer { due: now + interval, interval, repeats });
    }

    pub(crate) fn stop_timer(&mut self, id: u64) {
        self.timers.remove(&id);
    }

    /// Is any timer due at or before `time`?
    pub(crate) fn due_within(&self, time: f64) -> bool {
        self.timers.values().any(|timer| timer.due <= time + DUE_SLACK)
    }

    /// Every timer due by `now`, in due order (ties by id), each at most
    /// once: a repeating timer moves to its first due time after `now`
    /// (catch-up repeats collapse into this one), a zero interval to the
    /// next frame. The batch is taken before any handler runs, so a timer a
    /// handler starts or re-arms fires no earlier than the next frame, as a
    /// real event loop delivers it on a later turn; a zero-delay timer that
    /// re-arms itself therefore runs once per frame instead of forever.
    pub(crate) fn take_due(&mut self, now: f64) -> Vec<u64> {
        let mut due: Vec<(f64, u64)> = self
            .timers
            .iter()
            .filter(|(_, timer)| timer.due <= now + DUE_SLACK)
            .map(|(id, timer)| (timer.due, *id))
            .collect();
        due.sort_by(|(ta, a), (tb, b)| ta.total_cmp(tb).then(a.cmp(b)));
        for (_, id) in &due {
            let timer = self.timers.get_mut(id).unwrap();
            if !timer.repeats {
                self.timers.remove(id);
            } else if timer.interval > 0.0 {
                let interval = timer.interval.max(MIN_INTERVAL);
                let behind = ((now + DUE_SLACK - timer.due) / interval).floor().max(0.0);
                timer.due += (behind + 1.0) * interval;
            } else {
                timer.due = now;
            }
        }
        due.into_iter().map(|(_, id)| id).collect()
    }
}

/// Frame times are sums of `1/fps`; a timer due at exactly a frame boundary
/// must not miss it by a rounding error.
const DUE_SLACK: f64 = 1e-9;
/// A repeating interval smaller than this repeats at this rate.
const MIN_INTERVAL: f64 = 1e-6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_finite_and_positive() {
        assert_eq!(
            parse_window("360x640@3"),
            Some(LaunchWindow { width: 360.0, height: 640.0, scale: 3.0 })
        );
        for value in ["0x2@1", "1x2@NaN", "1xinf@1", "1x2", "-1x2@3", "axb@1"] {
            assert!(parse_window(value).is_none(), "{value}");
        }
    }

    #[test]
    fn launch_window_native_size_divides_out_the_display_scale() {
        let window = parse_window("360x640@3").unwrap();
        assert_eq!(window.native_size(2.0), (540.0, 960.0));
        assert_eq!(window.native_size(0.0), (1080.0, 1920.0));
    }

    #[test]
    fn arguments_and_environment_configure_the_launch() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        let config = parse_configuration(
            args(&["app", "--remote", "--virtual-clock", "--window", "360x640@3", "--seed", "7"]),
            None,
            None,
        );
        assert_eq!(
            config,
            Configuration {
                virtual_clock: true,
                window: parse_window("360x640@3"),
                seed: Some(7)
            }
        );
        let config = parse_configuration(
            args(&["app", "--seed=9", "--window=10x20@1"]),
            Some("1".into()),
            Some("3".into()),
        );
        assert!(config.virtual_clock);
        assert_eq!(config.seed, Some(9));
        assert_eq!(config.window, parse_window("10x20@1"));
        let config = parse_configuration(args(&["app"]), Some("0".into()), Some(" 5 ".into()));
        assert!(!config.virtual_clock);
        assert_eq!(config.seed, Some(5));
        // the program name is never an argument
        assert!(!parse_configuration(args(&["--virtual-clock"]), None, None).virtual_clock);
    }

    #[test]
    fn timer_order_and_cancellation_follow_virtual_time() {
        let mut clock = AppClock::default();
        clock.start_timer(2, 0.1, true, 0.0);
        clock.start_timer(1, 0.1, false, 0.0);
        assert!(!clock.due_within(0.09));
        assert_eq!(clock.take_due(0.1), vec![1, 2]);
        assert_eq!(clock.take_due(0.1), Vec::<u64>::new());
        clock.stop_timer(2);
        assert!(!clock.due_within(10.0));
    }

    #[test]
    fn timers_on_a_frame_boundary_fire_on_that_frame() {
        let mut clock = AppClock::default();
        clock.start_timer(1, 0.5, true, 0.0);
        let mut time = 0.0;
        let mut fired = Vec::new();
        for frame in 1..=60 {
            time += 1.0 / 60.0;
            for _ in clock.take_due(time) {
                fired.push(frame);
            }
        }
        assert_eq!(fired, vec![30, 60]);
    }

    #[test]
    fn a_zero_interval_repeats_once_per_frame() {
        let mut clock = AppClock::default();
        clock.start_timer(1, 0.0, true, 0.0);
        let mut count = 0;
        let mut time = 0.0;
        for _ in 0..10 {
            time += 1.0 / 60.0;
            count += clock.take_due(time).len();
        }
        assert_eq!(count, 10);
    }

    #[test]
    fn a_timer_rearmed_by_its_handler_waits_for_the_next_frame() {
        // A zero-delay timeout whose handler starts it again (a Splash
        // `set_timeout(f, 0)` loop): one batch per frame, so it fires once
        // per frame and the dispatch of a frame always ends.
        let mut clock = AppClock::default();
        clock.start_timer(1, 0.0, false, 0.0);
        let mut fired = 0;
        for frame in 1..=3 {
            let time = frame as f64 / 60.0;
            for id in clock.take_due(time) {
                fired += 1;
                clock.start_timer(id, 0.0, false, time);
            }
        }
        assert_eq!(fired, 3);
    }

    #[test]
    fn tiny_and_overdue_intervals_fire_once_per_frame() {
        let mut clock = AppClock::default();
        clock.start_timer(1, 1e-12, true, 0.0);
        clock.start_timer(2, 0.001, true, 0.0);
        assert_eq!(clock.take_due(1.0), vec![1, 2]);
        assert_eq!(clock.take_due(1.0), Vec::<u64>::new());
        assert_eq!(clock.take_due(1.0 + 1.0 / 60.0).len(), 2);
    }
}
