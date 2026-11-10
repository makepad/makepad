//! The frame clock, measured: `MAKEPAD_TRACE=frames` prints a histogram every
//! two seconds of what paced each beat of the HAL loop and how evenly frames
//! land, on every backend (the gaps read the same on each).
//!
//! Three gaps matter for smooth animation and each has its own clock: the
//! gap between the beats that DISPATCH `NextFrame` (the app steps its
//! animations here), the gap between window frames the backend presented
//! (what the compositor receives), and — for a display-paced beat — how far
//! ahead of the beat the flip it is aimed at lies. Every beat also says what
//! woke it: a window's display announcing a frame, the loop's own period
//! when no display paces it, or a host's turn. A judder that shows as an even
//! present histogram and an uneven next-frame histogram is the app clock's;
//! the reverse is the compositor's.
//!
//! A present's gap counts only time a frame was due. An idle app presents
//! nothing, and that is no stall: a rest runs from a turn of the loop that left
//! no work for a frame and none on its way to the screen, to when the loop next
//! started on something that left one (an input it took, or its wake), and the
//! gap leaves it out. A handler's time is not rest. The line says so.
//!
//! Off, each call is one atomic load.

use crate::makepad_error_log::trace_enabled;
use std::sync::atomic::{AtomicU64, Ordering};

/// Window frames handed to the compositor since start, always counted (one
/// relaxed add per present). An app's frame clock can run on beats whose
/// repaint the GPU backpressure gate withheld: comparing this with its own
/// drawn frames is how an app sees it is GPU-bound (and scales its render
/// resolution) without a governor inside the platform.
static PRESENTED: AtomicU64 = AtomicU64::new(0);

/// A window frame was presented (backends call this at the present).
pub fn note_presented() {
    PRESENTED.fetch_add(1, Ordering::Relaxed);
}

/// Window frames presented since start, all windows.
pub fn presented_frames() -> u64 {
    PRESENTED.load(Ordering::Relaxed)
}

/// Draw events dispatched since start (one per drawn frame, every platform): the frame clock
/// input record / replay and the flight recorder pin input events to.
static DRAWN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn note_drawn() {
    DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Draw events dispatched since start.
pub fn drawn_frames() -> u64 {
    DRAWN.load(Ordering::Relaxed)
}

/// What woke a beat of the HAL loop.
#[derive(Clone, Copy)]
pub enum TickSource {
    /// A window's display announced a frame (`FrameReady`): the flip it aims at is known.
    Display,
    /// The loop's own period: no display paces this app (offscreen windows, none yet).
    Cadence,
    /// A host's turn: the window manager's `Tick` for a hosted app, an embedder's turn.
    Host,
}

const EDGES_MS: [f64; 9] = [4.0, 6.0, 8.0, 10.0, 12.0, 15.0, 18.0, 25.0, 40.0];

#[derive(Default, Clone, Copy)]
struct Hist {
    buckets: [u32; 10],
    count: u32,
    sum_ms: f64,
    max_ms: f64,
}

impl Hist {
    fn add(&mut self, ms: f64) {
        let slot = EDGES_MS.iter().position(|e| ms < *e).unwrap_or(EDGES_MS.len());
        self.buckets[slot] += 1;
        self.count += 1;
        self.sum_ms += ms;
        if ms > self.max_ms {
            self.max_ms = ms;
        }
    }

    fn line(&self, name: &str) -> String {
        if self.count == 0 {
            return format!("{name}: none");
        }
        let mut s = format!(
            "{name}: n={} mean={:.2}ms max={:.1}ms |",
            self.count,
            self.sum_ms / self.count as f64,
            self.max_ms
        );
        let mut lo = 0.0;
        for (i, b) in self.buckets.iter().enumerate() {
            let label = if i < EDGES_MS.len() {
                format!("{lo:.0}-{:.0}", EDGES_MS[i])
            } else {
                format!("{lo:.0}+")
            };
            if *b > 0 {
                s.push_str(&format!(" {label}:{b}"));
            }
            if i < EDGES_MS.len() {
                lo = EDGES_MS[i];
            }
        }
        s
    }
}

pub struct FrameTrace {
    last_print: Option<f64>,
    sources: [u32; 3],
    next_frame: Hist,
    present: Hist,
    /// How far ahead of the wake the beat's target flip lies.
    flip_lead: Hist,
    last_next_frame: Option<f64>,
    last_present: Option<f64>,
    /// When the loop started on this turn's work: its oldest input, or its wake.
    turn_from: Option<f64>,
    /// No frame has been due since this time.
    resting: Option<f64>,
    /// Seconds of rest since the last present.
    rested: f64,
}

pub(crate) fn on() -> bool {
    trace_enabled("frames")
}

impl FrameTrace {
    pub fn new() -> Self {
        FrameTrace {
            last_print: None,
            sources: [0; 3],
            next_frame: Hist::default(),
            present: Hist::default(),
            flip_lead: Hist::default(),
            last_next_frame: None,
            last_present: None,
            turn_from: None,
            resting: None,
            rested: 0.0,
        }
    }

    /// A beat woke; `wake` and `flip` are seconds of one clock (the flip is the
    /// beat's target present time, `None` for a beat no display paced).
    pub fn tick(&mut self, source: TickSource, wake: f64, flip: Option<f64>) {
        if !on() {
            return;
        }
        self.sources[source as usize] += 1;
        if let Some(flip) = flip {
            self.flip_lead.add((flip - wake) * 1000.0);
        }
    }

    /// The app clock stepped: `NextFrame` was dispatched with this time.
    pub fn next_frame(&mut self, time: f64) {
        if !on() {
            return;
        }
        if let Some(prev) = self.last_next_frame {
            self.next_frame.add((time - prev) * 1000.0);
        }
        self.last_next_frame = Some(time);
    }

    /// The loop started on something at this time: an input it took, or a wake. A turn's oldest
    /// counts.
    pub fn work_from(&mut self, at: f64) {
        if on() {
            self.turn_from.get_or_insert(at);
        }
    }

    /// The loop's turn is over at `now`; `due`: it leaves work for a frame, or a frame on its way
    /// to the screen. A rest ends when the loop started on what made a frame due.
    pub fn turn_over(&mut self, due: bool, now: f64) {
        let from = self.turn_from.take().unwrap_or(now);
        if !due {
            self.resting.get_or_insert(now);
        } else if let Some(rest) = self.resting.take() {
            let rest = self.last_present.map_or(rest, |prev| rest.max(prev));
            self.rested += (from - rest).max(0.0);
        }
    }

    /// The gap in ms this present counts: since the one before, less the rests between them.
    fn gap(&mut self, now: f64) -> Option<f64> {
        let rested = std::mem::take(&mut self.rested);
        let prev = self.last_present.replace(now)?;
        Some(((now - prev - rested) * 1000.0).max(0.0))
    }

    /// A window frame was presented at this time (seconds of the backend's clock).
    pub fn present(&mut self, now: f64) {
        if !on() {
            return;
        }
        if let Some(ms) = self.gap(now) {
            self.present.add(ms);
        }
    }

    /// Print and reset every two seconds of wall time.
    pub fn maybe_print(&mut self, now: f64) {
        if !on() {
            return;
        }
        let Some(start) = self.last_print else {
            self.last_print = Some(now);
            return;
        };
        if now - start < 2.0 {
            return;
        }
        let [display, cadence, host] = self.sources;
        crate::trace!("frames", "ticks/2s: display={display} cadence={cadence} host={host}");
        crate::trace!("frames", "{}", self.next_frame.line("next_frame gap"));
        crate::trace!("frames", "{} (only while a frame was due)", self.present.line("present gap"));
        crate::trace!("frames", "{}", self.flip_lead.line("flip lead"));
        self.sources = [0; 3];
        self.next_frame = Hist::default();
        self.present = Hist::default();
        self.flip_lead = Hist::default();
        self.last_print = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(got: Option<f64>, ms: f64) -> bool {
        got.is_some_and(|g| (g - ms).abs() < 1e-6)
    }

    #[test]
    fn a_present_gap_leaves_out_a_rest_and_nothing_else() {
        let mut t = FrameTrace::new();
        assert_eq!(t.gap(1.0), None, "the first has nothing to count from");
        // frames that run on count whole
        t.turn_over(true, 1.002);
        assert!(near(t.gap(1.008), 8.0));
        // a rest, a wake that asked for nothing, then an input: from the input
        t.turn_over(false, 1.008);
        t.turn_over(false, 2.0);
        t.turn_from = Some(4.0);
        t.turn_over(true, 4.001);
        assert!(near(t.gap(4.04), 40.0));
        // a stall with a frame due counts whole
        t.turn_over(true, 5.0);
        assert!(near(t.gap(8.04), 4000.0));
        // a stall in the handler the rest ended for counts whole
        t.turn_over(false, 8.04);
        t.turn_from = Some(9.0);
        t.turn_over(true, 12.0);
        assert!(near(t.gap(12.01), 3010.0));
        // a wake's work counts from the wake
        t.turn_over(false, 12.01);
        t.turn_from = Some(13.0);
        t.turn_over(true, 13.1);
        assert!(near(t.gap(13.11), 110.0));
    }

    #[test]
    fn a_present_gap_adds_up_its_rests_and_never_rests_before_the_present() {
        let mut t = FrameTrace::new();
        let _ = t.gap(1.0);
        // two rests between two presents
        t.turn_over(false, 1.0);
        t.turn_from = Some(2.0);
        t.turn_over(true, 2.0);
        t.turn_over(false, 2.1);
        t.turn_from = Some(3.1);
        t.turn_over(true, 3.1);
        assert!(near(t.gap(3.12), 120.0));
        // a present heard after a turn found nothing due: the rest is from the present
        t.turn_over(false, 3.11);
        assert!(near(t.gap(3.13), 10.0));
        t.turn_from = Some(5.0);
        t.turn_over(true, 5.0);
        assert!(near(t.gap(5.01), 10.0));
    }

    #[test]
    fn gaps_fall_in_their_buckets_and_print_the_populated_ones() {
        let mut h = Hist::default();
        for ms in [3.0, 8.3, 8.4, 16.7, 50.0] {
            h.add(ms);
        }
        assert_eq!(h.count, 5);
        assert_eq!(h.buckets[0], 1, "3 ms is under 4");
        assert_eq!(h.buckets[3], 2, "8.3 and 8.4 are in 8-10");
        assert_eq!(h.buckets[6], 1, "16.7 is in 15-18");
        assert_eq!(h.buckets[9], 1, "50 is 40+");
        let line = h.line("x");
        assert!(line.contains("n=5") && line.contains("0-4:1") && line.contains("8-10:2") && line.contains("40+:1"), "{line}");
        assert_eq!(Hist::default().line("y"), "y: none");
    }
}
