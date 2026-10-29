//! Auto: the stream's seamless levers set from what the stream measures. Only levers that never
//! drop the picture: the bitrate cap, the anti-aliasing and the server's eye render scale (the
//! encoded size stays). The frame rate is never changed (a re-link); when the server cannot keep
//! up at the lowest settings, Auto suggests a lower one. One change at a time, then a dwell; a
//! lever lowered for a problem is not raised again for a minute, and a raise needs the whole
//! window clean with room to spare (no flapping). The user's manual value of a setting wins
//! (only settings left on Auto move).
//!
//! The signals, per 10 s window (two server windows, ten headset reports): the server's pictures
//! built per second against the display rate and its GPU time (render + pack) against the frame
//! budget; the headset's repeats by cause (late: the link or the decode; server skipped: the
//! server's build; queue cap), pictures shown; the link's round trip against its idle one.

use crate::settings_types::{StreamAa, MIN_RENDER_SCALE_PCT, STREAM_AA_MODES};
use std::time::{Duration, Instant};

/// Seconds after a change before the next one.
const DWELL: Duration = Duration::from_secs(15);
/// Seconds a lever lowered for a problem stays down at least.
const HOLD_DOWN: Duration = Duration::from_secs(60);
/// The window the signals are taken over (headset reports, one a second).
const WINDOW: usize = 10;
/// The bitrates Auto moves between (kbit/s): never above the link's measured sweet spot.
pub const AUTO_BITRATES_KBPS: [u32; 4] = [40_000, 60_000, 80_000, 100_000];
/// Auto's eye render scales (percent).
const RENDER_SCALES: [u32; 4] = [MIN_RENDER_SCALE_PCT, 80, 90, 100];
/// Auto's anti-aliasing ladder (indices into `STREAM_AA_MODES`, cheapest first): off, FXAA,
/// MSAA 2x, MSAA 4x (the default; supersampling and 8x only by hand).
const AUTO_AA: [usize; 4] = [0, 1, 2, 3];

fn aa_step(aa: StreamAa) -> usize {
    let i = STREAM_AA_MODES.iter().position(|m| *m == aa).unwrap_or(3);
    AUTO_AA.iter().position(|k| *k == i).unwrap_or(AUTO_AA.len() - 1)
}

/// Which lever a setting left on Auto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lever {
    Bitrate,
    Aa,
    RenderScale,
}

/// The levers' values Auto holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoValues {
    pub kbps: u32,
    pub aa: StreamAa,
    pub render_scale_pct: u32,
}

impl Default for AutoValues {
    fn default() -> Self {
        AutoValues { kbps: 80_000, aa: crate::settings_types::STREAM_AA_DEFAULT, render_scale_pct: 100 }
    }
}

/// One window's signals.
#[derive(Clone, Copy, Debug, Default)]
pub struct Signals {
    pub display_hz: f64,
    pub shown: f64,
    pub repeat_late: f64,
    pub repeat_server: f64,
    pub repeat_dropped: f64,
    pub built_per_s: f64,
    pub gpu_ms: f64,
    pub render_ms: f64,
    pub trip_ms: f64,
    pub trip_base_ms: f64,
    pub sent_mbps: f64,
    pub rate_mbps: f64,
}

impl Signals {
    fn budget_ms(&self) -> f64 {
        1000.0 / self.display_hz.max(1.0)
    }

    /// The server cannot build every frame (its GPU or its pacing).
    fn server_short(&self) -> bool {
        self.repeat_server > 1.0 || self.built_per_s < self.display_hz * 0.95 || self.gpu_ms > self.budget_ms() * 0.8
    }

    /// The link carries less than asked or queues.
    fn link_short(&self) -> bool {
        self.trip_ms > self.trip_base_ms + 6.0 || self.repeat_late > 1.5
    }

    /// Room on the server's GPU for more pixels.
    fn server_room(&self) -> bool {
        self.repeat_server < 0.2 && self.built_per_s >= self.display_hz * 0.98 && self.gpu_ms < self.budget_ms() * 0.5
    }

    /// The link is clean at the current rate.
    fn link_room(&self) -> bool {
        self.trip_ms <= self.trip_base_ms + 2.0 && self.repeat_late < 0.3
    }
}

/// The controller (the caption's XR control steps it once a second).
pub struct XrAuto {
    pub values: AutoValues,
    last_change: Option<Instant>,
    lowered: [Option<Instant>; 3],
    /// What Auto did last and why (for the panel); empty until it changed something.
    pub note: String,
    /// A suggestion Auto does not act on itself (a lower frame rate).
    pub suggestion: String,
    seen_window: u64,
}

impl Default for XrAuto {
    fn default() -> Self {
        XrAuto { values: AutoValues::default(), last_change: None, lowered: [None; 3], note: String::new(), suggestion: String::new(), seen_window: 0 }
    }
}

fn index(l: Lever) -> usize {
    match l {
        Lever::Bitrate => 0,
        Lever::Aa => 1,
        Lever::RenderScale => 2,
    }
}

impl XrAuto {
    /// One step with this window's signals; `auto(l)`: the user left lever `l` on Auto. Returns
    /// true when a value changed (to be applied).
    pub fn step(&mut self, s: &Signals, auto: impl Fn(Lever) -> bool, now: Instant) -> bool {
        if self.last_change.is_some_and(|t| now - t < DWELL) {
            return false;
        }
        let budget = s.budget_ms();
        // problems first: the server, then the link
        if s.server_short() {
            if auto(Lever::Aa) && aa_step(self.values.aa) > 0 {
                self.values.aa = STREAM_AA_MODES[AUTO_AA[aa_step(self.values.aa) - 1]];
                return self.changed(Lever::Aa, true, format!("anti-aliasing down to {}: the server built {:.0} of {:.0} frames/s, GPU {:.1} of {:.1} ms", self.values.aa.name, s.built_per_s, s.display_hz, s.gpu_ms, budget), now);
            }
            if auto(Lever::RenderScale) && self.values.render_scale_pct > MIN_RENDER_SCALE_PCT {
                let i = RENDER_SCALES.iter().position(|p| *p >= self.values.render_scale_pct).unwrap_or(RENDER_SCALES.len() - 1);
                self.values.render_scale_pct = RENDER_SCALES[i.saturating_sub(1)];
                return self.changed(Lever::RenderScale, true, format!("render scale down to {}%: the server built {:.0} of {:.0} frames/s, GPU {:.1} of {:.1} ms", self.values.render_scale_pct, s.built_per_s, s.display_hz, s.gpu_ms, budget), now);
            }
            if s.display_hz > 72.5 {
                self.suggestion = format!("the server keeps up at no lower settings: 72 fps would give it {:.1} ms a frame (now {:.1})", 1000.0 / 72.0, budget);
            }
            return false;
        }
        self.suggestion.clear();
        if s.link_short() {
            if auto(Lever::Bitrate) && self.values.kbps > AUTO_BITRATES_KBPS[0] {
                let i = AUTO_BITRATES_KBPS.iter().position(|k| *k >= self.values.kbps).unwrap_or(AUTO_BITRATES_KBPS.len() - 1);
                self.values.kbps = AUTO_BITRATES_KBPS[i.saturating_sub(1)];
                return self.changed(Lever::Bitrate, true, format!("bitrate down to {} Mbit/s: link trip {:.1} ms (idle {:.1}), {:.1} late repeats/s", self.values.kbps / 1000, s.trip_ms, s.trip_base_ms, s.repeat_late), now);
            }
            return false;
        }
        // room: one notch up, the picture's sharpness first (render scale, then AA), then the rate
        let can_raise = |me: &Self, l: Lever| auto(l) && !me.lowered[index(l)].is_some_and(|t| now - t < HOLD_DOWN);
        if s.server_room() {
            if can_raise(self, Lever::RenderScale) && self.values.render_scale_pct < 100 {
                let i = RENDER_SCALES.iter().position(|p| *p >= self.values.render_scale_pct).unwrap_or(0);
                self.values.render_scale_pct = RENDER_SCALES[(i + 1).min(RENDER_SCALES.len() - 1)];
                return self.changed(Lever::RenderScale, false, format!("render scale up to {}%: GPU {:.1} of {:.1} ms", self.values.render_scale_pct, s.gpu_ms, budget), now);
            }
            // a notch of AA up only with the GPU well inside the frame
            let i = aa_step(self.values.aa);
            if can_raise(self, Lever::Aa) && i + 1 < AUTO_AA.len() && s.gpu_ms < budget * 0.45 {
                self.values.aa = STREAM_AA_MODES[AUTO_AA[i + 1]];
                return self.changed(Lever::Aa, false, format!("anti-aliasing up to {}: GPU {:.1} of {:.1} ms", self.values.aa.name, s.gpu_ms, budget), now);
            }
        }
        if s.link_room() && can_raise(self, Lever::Bitrate) && s.sent_mbps * 1000.0 > self.values.kbps as f64 * 0.85 {
            if let Some(next) = AUTO_BITRATES_KBPS.iter().copied().find(|k| *k > self.values.kbps) {
                self.values.kbps = next;
                return self.changed(Lever::Bitrate, false, format!("bitrate up to {} Mbit/s: the link stays at its idle trip ({:.1} ms)", next / 1000, s.trip_ms), now);
            }
        }
        false
    }

    fn changed(&mut self, l: Lever, down: bool, why: String, now: Instant) -> bool {
        self.last_change = Some(now);
        if down {
            self.lowered[index(l)] = Some(now);
        }
        self.note = why;
        true
    }

    /// This window's signals: the server's last window and the headset's last reports. None while
    /// no headset streams or before a new server window (Auto waits for fresh numbers).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn signals(&mut self, srv: &crate::XrServer) -> Option<Signals> {
        use makepad_desk_link::xr::report as r;
        let server = srv.stats();
        if srv.session_eye().is_none() || server.window == 0 || server.window == self.seen_window {
            return None;
        }
        self.seen_window = server.window;
        let reports = srv.headset_reports();
        if reports.len() < WINDOW {
            return None;
        }
        let tail = &reports[reports.len() - WINDOW..];
        let mean = |i: usize| tail.iter().map(|rep| rep.get(i).copied().unwrap_or(0.0) as f64).sum::<f64>() / WINDOW as f64;
        Some(Signals {
            display_hz: mean(r::DISPLAY),
            shown: mean(r::SHOWN),
            repeat_late: mean(r::REPEAT_LATE),
            repeat_server: mean(r::REPEAT_SERVER),
            repeat_dropped: mean(r::REPEAT_DROPPED),
            built_per_s: server.built_per_s,
            gpu_ms: server.gpu_ms,
            render_ms: server.render_ms,
            trip_ms: server.trip_ms,
            trip_base_ms: server.trip_base_ms,
            sent_mbps: server.sent_mbps,
            rate_mbps: server.rate_mbps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Signals {
        Signals { display_hz: 90.0, shown: 90.0, repeat_late: 0.0, repeat_server: 0.0, repeat_dropped: 0.0, built_per_s: 90.0, gpu_ms: 3.0, render_ms: 4.0, trip_ms: 6.0, trip_base_ms: 5.0, sent_mbps: 79.0, rate_mbps: 80.0 }
    }

    #[test]
    fn one_change_per_dwell_and_no_flap() {
        let mut a = XrAuto::default();
        a.values.render_scale_pct = 90;
        let t0 = Instant::now();
        // room: up once
        assert!(a.step(&healthy(), |_| true, t0));
        assert_eq!(a.values.render_scale_pct, 100);
        // within the dwell nothing
        assert!(!a.step(&healthy(), |_| true, t0 + Duration::from_secs(5)));
        // the server falls short: AA down
        let mut short = healthy();
        short.repeat_server = 3.0;
        short.gpu_ms = 10.0;
        assert!(a.step(&short, |_| true, t0 + Duration::from_secs(16)));
        assert_eq!(a.values.aa, STREAM_AA_MODES[2]);
        // room again but AA held down for a minute: the render scale is at 100, so the bitrate
        let t = t0 + Duration::from_secs(32);
        assert!(a.step(&healthy(), |_| true, t));
        assert_eq!(a.values.aa, STREAM_AA_MODES[2]);
        assert_eq!(a.values.kbps, 100_000);
        // after the hold AA may come back
        assert!(a.step(&healthy(), |_| true, t0 + Duration::from_secs(80)));
        assert_eq!(a.values.aa, STREAM_AA_MODES[3]);
    }

    #[test]
    fn manual_settings_never_move_and_fps_is_only_suggested() {
        let mut a = XrAuto::default();
        let mut short = healthy();
        short.built_per_s = 60.0;
        short.gpu_ms = 12.0;
        // AA manual: the render scale goes down instead
        assert!(a.step(&short, |l| l != Lever::Aa, Instant::now()));
        assert_eq!(a.values.aa, STREAM_AA_MODES[3]);
        assert_eq!(a.values.render_scale_pct, 90);
        // nothing on Auto: nothing moves, 72 fps suggested
        let mut b = XrAuto::default();
        assert!(!b.step(&short, |_| false, Instant::now()));
        assert!(!b.suggestion.is_empty());
    }
}
