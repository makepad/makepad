//! `MAKEPAD_RUN_BOOT_PROBE=1`: how the run window answers while the app boots. A thread presses,
//! drags and releases on the window's caption every 8 ms and resizes it every 256 ms (the
//! events a user's window drag and resize put into session 0, pushed the way the window system
//! pushes them); the window pump measures how long each event waited for it, the gaps between
//! its turns and between the frames it presented. Stalls over 50 ms are printed as they happen
//! (with the boot phase), the summary when the boot ends (the app's first frame) or the run does
//! (`drag_queries`: the caption presses the window answered, `drag_max_ms` the longest answer):
//!
//! `makepad-run boot-probe: boot_s=.. events=.. event_max_ms=.. event_p99_ms=.. drag_queries=..
//! drag_max_ms=.. turn_max_ms=.. turn_p99_ms=.. frames=.. frame_max_ms=.. frame_p99_ms=.. stalls=..`
use makepad_hal::{clock_ns, event_tag, session::HOST, Hal, Id, RawEvent};
use relax_compiler::run_state::State;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// A gap the user feels: a dropped frame at 20 Hz.
const STALL_NS: u64 = 50_000_000;
const PRESS_EVERY: Duration = Duration::from_millis(8);
const RESIZE_EVERY: u32 = 32;

pub struct Probe {
    start: Instant,
    last_turn: Option<Instant>,
    last_frame: Option<Instant>,
    turns: Vec<u64>,
    frames: Vec<u64>,
    events: Vec<u64>,
    drag: Vec<u64>,
    stalls: usize,
    /// the last turn's pump overshoot and present (ns), named in stall lines
    parts: (u64, u64),
    stop: Arc<AtomicBool>,
    injector: Option<std::thread::JoinHandle<()>>,
    done: bool,
}

impl Probe {
    pub fn from_env() -> Option<Probe> {
        std::env::var_os("MAKEPAD_RUN_BOOT_PROBE")?;
        Some(Probe {
            start: Instant::now(),
            last_turn: None,
            last_frame: None,
            turns: Vec::new(),
            frames: Vec::new(),
            events: Vec::new(),
            drag: Vec::new(),
            stalls: 0,
            parts: (0, 0),
            stop: Arc::new(AtomicBool::new(false)),
            injector: None,
            done: false,
        })
    }

    /// The window exists: the presses start.
    pub fn window(&mut self, window: Id) {
        if self.injector.is_some() || self.done {
            return;
        }
        let stop = self.stop.clone();
        self.injector = std::thread::Builder::new()
            .name("run-boot-probe".into())
            .spawn(move || inject(window, &stop))
            .ok();
    }

    fn stall(&mut self, what: &str, ns: u64, state: &State) {
        if ns <= STALL_NS {
            return;
        }
        self.stalls += 1;
        eprintln!(
            "makepad-run boot-probe: stall {:.1} ms ({what}) at {:.2} s (last turn: pump +{:.1} ms, present {:.1} ms), phase {} {}",
            ns as f64 / 1e6,
            self.start.elapsed().as_secs_f64(),
            self.parts.0 as f64 / 1e6,
            self.parts.1 as f64 / 1e6,
            state.phase.name(),
            state.current
        );
    }

    /// The pump begins a turn; `booting` until the app presented its first frame.
    pub fn turn(&mut self, booting: bool, state: &State) {
        if self.done {
            return;
        }
        if !booting && self.injector.is_some() {
            self.finish();
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_turn.replace(now) {
            let ns = (now - last).as_nanos() as u64;
            self.turns.push(ns);
            self.stall("turn gap", ns, state);
        }
    }

    pub fn frame(&mut self, state: &State) {
        if self.done {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_frame.replace(now) {
            let ns = (now - last).as_nanos() as u64;
            self.frames.push(ns);
            self.stall("frame interval", ns, state);
        }
    }

    /// What the last turn of the boot window spent (`Boot::turn_parts`).
    pub fn parts(&mut self, parts: (u64, u64)) {
        self.parts = parts;
    }

    pub fn latencies(&mut self, latencies: Vec<(u16, u64)>, state: &State) {
        if self.done {
            return;
        }
        for (tag, ns) in latencies {
            self.events.push(ns);
            if tag == event_tag::DragQuery {
                self.drag.push(ns);
            }
            self.stall("event wait", ns, state);
        }
    }

    pub fn finish(&mut self) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        self.stop.store(true, Ordering::Release);
        if let Some(injector) = self.injector.take() {
            let _ = injector.join();
        }
        let ms = |v: f64| format!("{:.1}", v / 1e6);
        let (event_max, event_p99) = spread(&mut self.events);
        let (drag_max, _) = spread(&mut self.drag);
        let (turn_max, turn_p99) = spread(&mut self.turns);
        let (frame_max, frame_p99) = spread(&mut self.frames);
        eprintln!(
            "makepad-run boot-probe: boot_s={:.2} events={} event_max_ms={} event_p99_ms={} drag_queries={} drag_max_ms={} turn_max_ms={} turn_p99_ms={} frames={} frame_max_ms={} frame_p99_ms={} stalls={}",
            self.start.elapsed().as_secs_f64(),
            self.events.len(),
            ms(event_max),
            ms(event_p99),
            self.drag.len(),
            ms(drag_max),
            ms(turn_max),
            ms(turn_p99),
            self.frames.len(),
            ms(frame_max),
            ms(frame_p99),
            self.stalls
        );
    }
}

/// Largest and 99th percentile (ns).
fn spread(samples: &mut [u64]) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_unstable();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    (samples[samples.len() - 1] as f64, p99 as f64)
}

/// A press on the caption, two drag steps and the release, again and again; every
/// `RESIZE_EVERY` presses the window's size changes (as the window system reports a resize).
fn inject(window: Id, stop: &AtomicBool) {
    let hal = Hal::global();
    let stamped = |mut event: RawEvent| {
        event.time_ns = clock_ns();
        event
    };
    let mut cycle = 0u32;
    // the window's geometry is read once (then kept here): a read takes the HAL's lock, and an
    // event stamped only after waiting for it would hide that wait
    let mut known = None;
    while !stop.load(Ordering::Acquire) {
        if known.is_none() {
            known = hal.with_world(|w| {
                w.session(HOST)
                    .and_then(|s| s.windows.get(HOST, window).ok())
                    .map(|w| (w.val.geom, w.val.dpi.unwrap_or(1.0), w.val.state))
                    .filter(|(geom, _, _)| geom[2] > 0.0 && geom[3] > 0.0)
            });
        }
        let Some((mut geom, dpi, state)) = known else {
            std::thread::sleep(PRESS_EVERY);
            continue;
        };
        let (x, y) = (geom[2] as f64 * 0.5, 12.0);
        let step = cycle % 4;
        let event = match step {
            0 => {
                hal.inject(HOST, stamped(RawEvent::drag_query(window, x, y)));
                RawEvent::mouse_down(window, x, y, 1, 0)
            }
            1 | 2 => RawEvent::mouse_move(window, x + step as f64 * 4.0, y, 4.0, 0.0, 0),
            _ => RawEvent::mouse_up(window, x + 8.0, y, 1, 0),
        };
        hal.inject(HOST, stamped(event));
        if cycle % RESIZE_EVERY == RESIZE_EVERY - 1 {
            geom[2] += if (cycle / RESIZE_EVERY) % 2 == 0 { 48.0 } else { -48.0 };
            let resize = stamped(RawEvent::geom(window, geom[0] as f64, geom[1] as f64, geom[2] as f64, geom[3] as f64, dpi, state));
            hal.with_world(|w| w.window_event(HOST, resize, &[]));
            hal.wake_session(HOST);
            known = Some((geom, dpi, state));
        }
        cycle += 1;
        std::thread::sleep(PRESS_EVERY);
    }
}
