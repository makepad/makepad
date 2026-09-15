//! The frame clock, measured: `MAKEPAD_TRACE=frames` prints a histogram every
//! two seconds of what paced each tick and how evenly frames land, on every
//! desktop backend (the sources differ per backend, the gaps read the same).
//!
//! Three gaps matter for smooth animation and each has its own clock: the
//! gap between the ticks that DISPATCH `NextFrame` (the app steps its
//! animations here), the gap between `Present`s (what the compositor
//! receives), and — for a beat-driven tick — how far ahead of the wake the
//! flip it is aimed at lies. Every tick also says what woke it: a window's
//! frame-latency waitable, the wait timing out, an input message, a timer,
//! or the drain-then-paint fallback when nothing is being composited. A
//! judder that shows as an even present histogram and an uneven next-frame
//! histogram is the app clock's; the reverse is the compositor's.
//!
//! Off, this is one `bool` check per tick.

use crate::makepad_error_log::trace_enabled;

/// What woke a paint tick.
#[derive(Clone, Copy)]
pub enum TickSource {
    /// A window's DXGI frame-latency waitable fired: a present retired.
    Waitable,
    /// The beat wait timed out: nothing was being composited.
    Timeout,
    /// An input message arrived while in `Wait` flow and a paint followed it.
    Message,
    /// A Win32 timer (resize / drag-drop heartbeat).
    Timer,
    /// No beat source to wait on: the drain-then-paint fallback.
    Drain,
    /// A display link fired (macOS): the flip's target timestamp is known.
    Link,
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
    enabled: bool,
    last_print: Option<f64>,
    sources: [u32; 6],
    next_frame: Hist,
    present: Hist,
    /// How far ahead of the wake the beat's target flip lies.
    flip_lead: Hist,
    last_next_frame: Option<f64>,
    last_present: Option<f64>,
    /// Presents the compositor refused (`DXGI_STATUS_OCCLUDED` and kin).
    occluded: u32,
}

impl FrameTrace {
    pub fn new() -> Self {
        FrameTrace {
            enabled: trace_enabled("frames"),
            last_print: None,
            sources: [0; 6],
            next_frame: Hist::default(),
            present: Hist::default(),
            flip_lead: Hist::default(),
            last_next_frame: None,
            last_present: None,
            occluded: 0,
        }
    }

    /// A tick woke; `wake` and `flip` are app seconds (the flip is the beat's
    /// target present time, `None` for an unpaced tick).
    pub fn tick(&mut self, source: TickSource, wake: f64, flip: Option<f64>) {
        if !self.enabled {
            return;
        }
        self.sources[source as usize] += 1;
        if let Some(flip) = flip {
            self.flip_lead.add((flip - wake) * 1000.0);
        }
    }

    /// A beat's target flip, once the window's frame statistics gave it.
    pub fn flip_lead(&mut self, wake: f64, flip: f64) {
        if !self.enabled {
            return;
        }
        self.flip_lead.add((flip - wake) * 1000.0);
    }

    /// The app clock stepped: `NextFrame` was dispatched with this time.
    pub fn next_frame(&mut self, time: f64) {
        if !self.enabled {
            return;
        }
        if let Some(prev) = self.last_next_frame {
            self.next_frame.add((time - prev) * 1000.0);
        }
        self.last_next_frame = Some(time);
    }

    /// A frame was handed to `Present` at this wall time.
    pub fn present(&mut self, now: f64) {
        if !self.enabled {
            return;
        }
        if let Some(prev) = self.last_present {
            self.present.add((now - prev) * 1000.0);
        }
        self.last_present = Some(now);
    }

    /// A present the compositor refused: the window is not reaching glass.
    pub fn present_occluded(&mut self) {
        if !self.enabled {
            return;
        }
        self.occluded += 1;
    }

    /// Print and reset every two seconds of wall time.
    pub fn maybe_print(&mut self, now: f64) {
        if !self.enabled {
            return;
        }
        let Some(start) = self.last_print else {
            self.last_print = Some(now);
            return;
        };
        if now - start < 2.0 {
            return;
        }
        let [waitable, timeout, message, timer, drain, link] = self.sources;
        let occluded = self.occluded;
        crate::trace!(
            "frames",
            "ticks/2s: waitable={waitable} timeout={timeout} message={message} timer={timer} drain={drain} link={link} occluded_presents={occluded}"
        );
        crate::trace!("frames", "{}", self.next_frame.line("next_frame gap"));
        crate::trace!("frames", "{}", self.present.line("present gap"));
        crate::trace!("frames", "{}", self.flip_lead.line("flip lead"));
        self.sources = [0; 6];
        self.next_frame = Hist::default();
        self.present = Hist::default();
        self.flip_lead = Hist::default();
        self.occluded = 0;
        self.last_print = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
