//! What limits the stream, in plain words: the stage that keeps the headset from showing a new
//! picture every display frame, with the numbers that show it, and a hint the panel's own
//! controls can act on.
//!
//! The headset says why a frame repeated (`report::REPEAT_*`): the picture was late (in flight
//! or decoding), the server never sent it (it built fewer frames than the headset shows), or the
//! headset let it go at its queue cap (pictures came in a burst). The stage times say which part
//! is slow: the host app's frame (the app's draw and its GPU passes, split into the headset's
//! eyes, the desktop's windows and the stream's own passes), the encode, the send, the network
//! (its round trip against the idle link's), the headset's decode and its own render.
//!
//! The verdict is taken over a few seconds' mean ([`VerdictFilter`]), and a new one replaces the
//! one shown only after it held for a few seconds: it does not flicker.

use std::collections::VecDeque;

/// One second's numbers (the headset's report, the server's last window, the host's load).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Evidence {
    /// The headset: display rate, pictures shown and received (per s).
    pub display_hz: f64,
    pub shown: f64,
    pub received: f64,
    /// Repeats per second by cause: late, the server never sent it, dropped at the queue cap,
    /// no pose went up.
    pub repeat_late: f64,
    pub repeat_server: f64,
    pub repeat_cap: f64,
    pub repeat_nopose: f64,
    /// The headset's stages (ms): transit, decode, its own render (0: not measured).
    pub net_ms: f64,
    pub decode_ms: f64,
    pub client_gpu_ms: f64,
    /// The server: frames built per second, pose to frame built, the GPU's part, encode, send
    /// (ms; 0 before its first window).
    pub built_per_s: f64,
    pub render_ms: f64,
    pub gpu_ms: f64,
    pub encode_ms: f64,
    pub send_ms: f64,
    /// A tile's round trip and the idle link's (ms).
    pub trip_ms: f64,
    pub trip_base_ms: f64,
    /// Sent and the bandwidth cap (Mbit/s).
    pub sent_mbps: f64,
    pub cap_mbps: f64,
    /// The app waited for the headset (too many pictures in flight), per second.
    pub waited_per_s: f64,
    /// The host app's load (`Cx::take_pass_load`): GPU ms per second for the headset's eyes, the
    /// desktop's windows and the passes under no window, and the draws' CPU ms per second.
    /// `host_timed`: the GPU times are measured.
    pub eyes_gpu_ms_s: f64,
    pub desktop_gpu_ms_s: f64,
    pub other_gpu_ms_s: f64,
    pub draw_ms_s: f64,
    pub host_timed: bool,
}

/// The stage that limits the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Nothing measured yet.
    Waiting,
    /// Every display frame shows a new picture.
    None,
    HostRendering,
    Encoder,
    Network,
    Bandwidth,
    HeadsetDecode,
    HeadsetRendering,
    /// Frames are missed without one clear cause.
    Unclear,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Waiting => "waiting for numbers",
            Stage::None => "no bottleneck",
            Stage::HostRendering => "host app rendering",
            Stage::Encoder => "encoder",
            Stage::Network => "network",
            Stage::Bandwidth => "bandwidth cap",
            Stage::HeadsetDecode => "headset decode",
            Stage::HeadsetRendering => "headset rendering",
            Stage::Unclear => "unclear",
        }
    }
}

/// The verdict: the stage, the numbers behind it, a hint (empty: none).
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub stage: Stage,
    pub evidence: String,
    pub hint: String,
}

impl Verdict {
    /// "Limited by: host app rendering (…)", "No bottleneck (…)".
    pub fn line(&self) -> String {
        let head = match self.stage {
            Stage::None => "No bottleneck".to_string(),
            Stage::Waiting => "Bottleneck: waiting for numbers".to_string(),
            Stage::Unclear => "Frames missed, cause unclear".to_string(),
            s => format!("Limited by: {}", s.name()),
        };
        if self.evidence.is_empty() { head } else { format!("{head} ({})", self.evidence) }
    }
}

/// One stage's time for the bars: its name, ms per frame, and whether it is measured.
#[derive(Clone, Debug, PartialEq)]
pub struct StageTime {
    pub name: &'static str,
    pub ms: f64,
    pub measured: bool,
}

impl Evidence {
    pub fn budget_ms(&self) -> f64 {
        1000.0 / self.display_hz.max(1.0)
    }

    /// The host's GPU ms per frame it built (all its passes: the desktop's too share the GPU).
    fn per_built(&self, ms_per_s: f64) -> f64 {
        if self.built_per_s > 0.5 { ms_per_s / self.built_per_s } else { 0.0 }
    }

    /// The host app's frame (ms): the slowest of its draw (CPU) and its GPU passes per frame
    /// built when measured, else from the frames it built (behind the display rate) or its GPU
    /// part.
    pub fn host_ms(&self) -> f64 {
        if self.host_timed && self.built_per_s > 0.5 {
            let gpu = self.per_built(self.eyes_gpu_ms_s + self.desktop_gpu_ms_s + self.other_gpu_ms_s);
            let cpu = self.per_built(self.draw_ms_s);
            return gpu.max(cpu);
        }
        if self.built_per_s > 0.5 && self.built_per_s < self.display_hz * 0.95 {
            1000.0 / self.built_per_s
        } else {
            self.gpu_ms
        }
    }

    /// The stages for the bars, in the picture's order.
    pub fn stage_times(&self) -> Vec<StageTime> {
        let server = self.built_per_s > 0.0;
        vec![
            StageTime { name: "host app", ms: self.host_ms(), measured: server || self.host_timed },
            StageTime { name: "encode", ms: self.encode_ms, measured: server },
            StageTime { name: "send", ms: self.send_ms, measured: server },
            StageTime { name: "network", ms: self.net_ms, measured: self.net_ms > 0.0 },
            StageTime { name: "headset decode", ms: self.decode_ms, measured: self.decode_ms > 0.0 },
            StageTime { name: "headset render", ms: self.client_gpu_ms, measured: self.client_gpu_ms > 0.0 },
        ]
    }

    /// The host's frame split by what it draws for: "desktop view 9 ms + eyes 7 ms" (GPU per
    /// frame built, the larger first; the draw's CPU time when it is the longer), empty when
    /// not measured.
    pub fn host_split(&self) -> String {
        if !self.host_timed || self.built_per_s <= 0.5 {
            return String::new();
        }
        let mut parts = [("desktop view", self.per_built(self.desktop_gpu_ms_s)), ("eyes", self.per_built(self.eyes_gpu_ms_s)), ("stream passes", self.per_built(self.other_gpu_ms_s))];
        parts.sort_by(|a, b| b.1.total_cmp(&a.1));
        let gpu: Vec<String> = parts.iter().filter(|p| p.1 >= 0.5).map(|(n, ms)| format!("{n} {ms:.0} ms")).collect();
        let cpu = self.per_built(self.draw_ms_s);
        let gpu_total: f64 = parts.iter().map(|p| p.1).sum();
        if cpu > gpu_total {
            format!("app draw (CPU) {cpu:.0} ms")
        } else {
            gpu.join(" + ")
        }
    }

    /// The verdict of these numbers alone (no smoothing).
    pub fn verdict(&self) -> Verdict {
        let v = |stage: Stage, evidence: String, hint: &str| Verdict { stage, evidence, hint: hint.to_string() };
        if self.display_hz < 1.0 {
            return v(Stage::Waiting, String::new(), "");
        }
        let d = self.display_hz;
        let budget = self.budget_ms();
        let repeats = self.repeat_late + self.repeat_server + self.repeat_cap + self.repeat_nopose;
        let missed = (d - self.shown).max(0.0).max(repeats);
        let over_idle = (self.trip_ms - self.trip_base_ms).max(0.0);
        if missed < d * 0.03 {
            let slowest = self.stage_times().into_iter().filter(|s| s.measured).max_by(|a, b| a.ms.total_cmp(&b.ms));
            let evidence = match slowest {
                Some(s) => format!("{:.0} of {:.0} frames/s new; slowest stage {} {:.1} of {:.1} ms", self.shown, d, s.name, s.ms, budget),
                None => format!("{:.0} of {:.0} frames/s new", self.shown, d),
            };
            return v(Stage::None, evidence, "");
        }
        let causes = [self.repeat_server, self.repeat_late, self.repeat_cap, self.repeat_nopose];
        let top = (0..4).max_by(|a, b| causes[*a].total_cmp(&causes[*b])).unwrap();
        let of = |n: f64, what: &str| format!("{n:.0} of {repeats:.0} repeats {what}");
        let decode_slow = self.decode_ms > budget * 0.85;
        let link_queued = over_idle > 6.0;
        let at_cap = self.cap_mbps > 0.0 && self.sent_mbps >= self.cap_mbps * 0.9;
        if repeats < 0.5 {
            return v(Stage::Unclear, format!("{:.0} of {:.0} frames/s shown, no repeats counted", self.shown, d), "");
        }
        match top {
            // the server never sent those frames
            0 => {
                // held back by the headset (too many pictures in flight): what is downstream
                if self.waited_per_s > d * 0.1 {
                    if decode_slow {
                        return v(Stage::HeadsetDecode, format!("{}: the host waited for the headset {:.0}/s, decode {:.1} ms, budget {budget:.1} ms", of(self.repeat_server, "unsent"), self.waited_per_s, self.decode_ms), "lower Bandwidth or Frame rate");
                    }
                    if link_queued {
                        return v(Stage::Network, format!("{}: the host waited for the headset {:.0}/s, round trip {over_idle:.0} ms over idle", of(self.repeat_server, "unsent"), self.waited_per_s), "lower Bandwidth");
                    }
                }
                if self.encode_ms > budget * 0.85 {
                    return v(Stage::Encoder, format!("{}: encode {:.1} ms, budget {budget:.1} ms", of(self.repeat_server, "unsent"), self.encode_ms), "lower Frame rate");
                }
                let host = self.host_ms();
                let mut evidence = if self.built_per_s > 0.5 {
                    format!("{}: the host built {:.0} of {d:.0} frames/s, {:.1} ms each, budget {budget:.1} ms", of(self.repeat_server, "unsent"), self.built_per_s, 1000.0 / self.built_per_s)
                } else {
                    format!("{}: host frame {host:.1} ms, budget {budget:.1} ms", of(self.repeat_server, "unsent"))
                };
                let split = self.host_split();
                if !split.is_empty() {
                    evidence = format!("{evidence}: {split}");
                }
                let desktop = self.per_built(self.desktop_gpu_ms_s);
                let eyes = self.per_built(self.eyes_gpu_ms_s);
                let hint = if self.host_timed && desktop > eyes && desktop > 1.0 {
                    "turn off Desktop mirror, and close or pause other views of the app's desktop"
                } else if self.host_timed && self.per_built(self.draw_ms_s) > desktop + eyes {
                    "lower Frame rate"
                } else {
                    "lower Render scale or Anti-aliasing"
                };
                v(Stage::HostRendering, evidence, hint)
            }
            // late: in flight or decoding when its frame came
            1 => {
                if decode_slow {
                    v(Stage::HeadsetDecode, format!("{}: decode {:.1} ms, budget {budget:.1} ms", of(self.repeat_late, "late"), self.decode_ms), "lower Bandwidth or Frame rate")
                } else if at_cap && link_queued {
                    v(Stage::Bandwidth, format!("{}: sending {:.0} of {:.0} Mbit/s, round trip {over_idle:.0} ms over idle", of(self.repeat_late, "late"), self.sent_mbps, self.cap_mbps), "lower Bandwidth")
                } else {
                    v(Stage::Network, format!("{}: round trip {:.0} ms, {over_idle:.0} ms over idle", of(self.repeat_late, "late"), self.trip_ms), "lower Bandwidth")
                }
            }
            // let go at the headset's queue cap: pictures came in bursts
            2 => {
                if self.client_gpu_ms > budget * 0.9 {
                    v(Stage::HeadsetRendering, format!("{}: headset GPU {:.1} ms, budget {budget:.1} ms", of(self.repeat_cap, "dropped at the queue cap"), self.client_gpu_ms), "turn off Room scan or lower Frame rate")
                } else {
                    v(Stage::Network, format!("{}: pictures arrive in bursts, round trip {over_idle:.0} ms over idle", of(self.repeat_cap, "dropped at the queue cap")), "lower Bandwidth")
                }
            }
            // no pose went up for the frame: the headset's own frame
            _ => v(Stage::HeadsetRendering, format!("{}: headset GPU {:.1} ms, budget {budget:.1} ms", of(self.repeat_nopose, "without a pose"), self.client_gpu_ms), "turn off Room scan or lower Frame rate"),
        }
    }

    /// The mean of `all` (each field).
    pub fn mean(all: &[Evidence]) -> Evidence {
        let n = all.len().max(1) as f64;
        let m = |f: fn(&Evidence) -> f64| all.iter().map(f).sum::<f64>() / n;
        Evidence {
            display_hz: m(|e| e.display_hz),
            shown: m(|e| e.shown),
            received: m(|e| e.received),
            repeat_late: m(|e| e.repeat_late),
            repeat_server: m(|e| e.repeat_server),
            repeat_cap: m(|e| e.repeat_cap),
            repeat_nopose: m(|e| e.repeat_nopose),
            net_ms: m(|e| e.net_ms),
            decode_ms: m(|e| e.decode_ms),
            client_gpu_ms: m(|e| e.client_gpu_ms),
            built_per_s: m(|e| e.built_per_s),
            render_ms: m(|e| e.render_ms),
            gpu_ms: m(|e| e.gpu_ms),
            encode_ms: m(|e| e.encode_ms),
            send_ms: m(|e| e.send_ms),
            trip_ms: m(|e| e.trip_ms),
            trip_base_ms: m(|e| e.trip_base_ms),
            sent_mbps: m(|e| e.sent_mbps),
            cap_mbps: m(|e| e.cap_mbps),
            waited_per_s: m(|e| e.waited_per_s),
            eyes_gpu_ms_s: m(|e| e.eyes_gpu_ms_s),
            desktop_gpu_ms_s: m(|e| e.desktop_gpu_ms_s),
            other_gpu_ms_s: m(|e| e.other_gpu_ms_s),
            draw_ms_s: m(|e| e.draw_ms_s),
            host_timed: all.iter().any(|e| e.host_timed),
        }
    }
}

/// Seconds the verdict is taken over.
const MEAN_SECONDS: usize = 5;
/// Seconds a new verdict must hold before it replaces the one shown.
const HOLD_SECONDS: u32 = 3;

/// The verdict over the last seconds, changed only after a new one held (no flicker).
#[derive(Default)]
pub struct VerdictFilter {
    seconds: VecDeque<Evidence>,
    shown: Option<Verdict>,
    candidate: Option<(Stage, u32)>,
    mean: Evidence,
}

impl VerdictFilter {
    /// One second's numbers in; the verdict to show.
    pub fn push(&mut self, e: Evidence) -> &Verdict {
        if self.seconds.len() >= MEAN_SECONDS {
            self.seconds.pop_front();
        }
        self.seconds.push_back(e);
        self.mean = Evidence::mean(self.seconds.make_contiguous());
        let now = self.mean.verdict();
        match &self.shown {
            // the same stage: its numbers refresh
            Some(s) if s.stage == now.stage => {
                self.candidate = None;
                self.shown = Some(now);
            }
            // nothing shown yet, or a stage that is no verdict: at once
            Some(s) if s.stage != Stage::Waiting && now.stage != Stage::Waiting => {
                let held = match self.candidate {
                    Some((stage, n)) if stage == now.stage => n + 1,
                    _ => 1,
                };
                if held >= HOLD_SECONDS {
                    self.candidate = None;
                    self.shown = Some(now);
                } else {
                    self.candidate = Some((now.stage, held));
                }
            }
            _ => {
                self.candidate = None;
                self.shown = Some(now);
            }
        }
        self.shown.as_ref().unwrap()
    }

    /// The verdict shown (None before any numbers).
    pub fn shown(&self) -> Option<&Verdict> {
        self.shown.as_ref()
    }

    /// The mean the verdict was taken over (for the stage bars).
    pub fn mean(&self) -> &Evidence {
        &self.mean
    }

    pub fn clear(&mut self) {
        *self = VerdictFilter::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean stream at 90 Hz.
    fn healthy() -> Evidence {
        Evidence {
            display_hz: 90.0,
            shown: 90.0,
            received: 90.0,
            net_ms: 4.0,
            decode_ms: 4.0,
            client_gpu_ms: 5.0,
            built_per_s: 90.0,
            render_ms: 5.0,
            gpu_ms: 4.0,
            encode_ms: 4.0,
            send_ms: 0.3,
            trip_ms: 8.0,
            trip_base_ms: 7.0,
            sent_mbps: 60.0,
            cap_mbps: 80.0,
            ..Default::default()
        }
    }

    /// The user's screenshot: 87 Hz, 64 shown and received, 23 repeats (late 2, server skipped
    /// 18, the rest at the queue cap), encode 7.7 ms, 118 ms latency.
    fn users_case() -> Evidence {
        Evidence {
            display_hz: 87.0,
            shown: 64.0,
            received: 64.0,
            repeat_late: 2.0,
            repeat_server: 18.0,
            repeat_cap: 3.0,
            net_ms: 6.0,
            decode_ms: 5.0,
            client_gpu_ms: 6.0,
            built_per_s: 64.0,
            render_ms: 14.0,
            gpu_ms: 9.0,
            encode_ms: 7.7,
            send_ms: 0.4,
            trip_ms: 12.0,
            trip_base_ms: 9.0,
            sent_mbps: 0.0,
            cap_mbps: 80.0,
            ..Default::default()
        }
    }

    #[test]
    fn the_users_case_is_host_rendering() {
        let v = users_case().verdict();
        assert_eq!(v.stage, Stage::HostRendering, "{}", v.line());
        assert!(v.evidence.starts_with("18 of 23 repeats"), "{}", v.evidence);
        assert!(v.evidence.contains("64 of 87 frames/s") && v.evidence.contains("15.6 ms each") && v.evidence.contains("budget 11.5 ms"), "{}", v.evidence);
        assert!(!v.hint.is_empty());
        assert!(v.line().starts_with("Limited by: host app rendering ("), "{}", v.line());
    }

    #[test]
    fn host_rendering_split_names_the_desktop_view() {
        // the desktop's 3D view repaints during XR: 9 ms of GPU per frame built, the eyes 7
        let mut e = users_case();
        e.host_timed = true;
        e.desktop_gpu_ms_s = 9.0 * 64.0;
        e.eyes_gpu_ms_s = 7.0 * 64.0;
        e.other_gpu_ms_s = 0.3 * 64.0;
        e.draw_ms_s = 3.0 * 64.0;
        let v = e.verdict();
        assert_eq!(v.stage, Stage::HostRendering);
        assert!(v.evidence.ends_with(": desktop view 9 ms + eyes 7 ms"), "{}", v.evidence);
        assert!(v.hint.contains("Desktop mirror"), "{}", v.hint);
        assert!((e.host_ms() - 16.3).abs() < 0.05, "{}", e.host_ms());
        // the eyes the larger part: render scale
        e.desktop_gpu_ms_s = 1.0 * 64.0;
        e.eyes_gpu_ms_s = 14.0 * 64.0;
        let v = e.verdict();
        assert!(v.evidence.ends_with(": eyes 14 ms + desktop view 1 ms"), "{}", v.evidence);
        assert!(v.hint.contains("Render scale"), "{}", v.hint);
        // the app's draw (CPU) longer than its GPU passes
        e.draw_ms_s = 20.0 * 64.0;
        assert!(e.verdict().evidence.ends_with(": app draw (CPU) 20 ms"), "{}", e.verdict().evidence);
    }

    #[test]
    fn healthy_is_no_bottleneck() {
        let v = healthy().verdict();
        assert_eq!(v.stage, Stage::None, "{}", v.line());
        assert!(v.line().starts_with("No bottleneck"));
        assert!(v.hint.is_empty());
    }

    #[test]
    fn late_frames_are_the_network_or_the_cap_or_the_decode() {
        let mut e = healthy();
        e.shown = 75.0;
        e.repeat_late = 15.0;
        e.trip_ms = 47.0;
        let v = e.verdict();
        assert_eq!(v.stage, Stage::Network, "{}", v.line());
        assert!(v.evidence.contains("40 ms over idle"), "{}", v.evidence);
        // sending at the cap with the link queued: the bandwidth cap
        e.sent_mbps = 79.0;
        assert_eq!(e.verdict().stage, Stage::Bandwidth);
        // a slow decode wins
        e.decode_ms = 12.0;
        assert_eq!(e.verdict().stage, Stage::HeadsetDecode);
    }

    #[test]
    fn unsent_frames_with_a_slow_encode_are_the_encoder() {
        let mut e = healthy();
        e.shown = 70.0;
        e.repeat_server = 20.0;
        e.built_per_s = 70.0;
        e.encode_ms = 10.5;
        assert_eq!(e.verdict().stage, Stage::Encoder);
        // held back by a headset that decodes too slowly
        e.encode_ms = 4.0;
        e.waited_per_s = 30.0;
        e.decode_ms = 11.0;
        assert_eq!(e.verdict().stage, Stage::HeadsetDecode);
    }

    #[test]
    fn queue_cap_and_no_pose() {
        let mut e = healthy();
        e.shown = 80.0;
        e.repeat_cap = 10.0;
        assert_eq!(e.verdict().stage, Stage::Network);
        e.client_gpu_ms = 11.0;
        assert_eq!(e.verdict().stage, Stage::HeadsetRendering);
        let mut e = healthy();
        e.shown = 80.0;
        e.repeat_nopose = 10.0;
        assert_eq!(e.verdict().stage, Stage::HeadsetRendering);
    }

    #[test]
    fn nothing_yet_is_waiting() {
        assert_eq!(Evidence::default().verdict().stage, Stage::Waiting);
    }

    #[test]
    fn the_filter_does_not_flicker() {
        let mut f = VerdictFilter::default();
        assert_eq!(f.push(users_case()).stage, Stage::HostRendering);
        // one healthy second: the mean still misses frames, and a change must hold
        for _ in 0..2 {
            assert_eq!(f.push(healthy()).stage, Stage::HostRendering);
        }
        // a lasting change replaces it once the mean says so for three seconds
        let mut n = 0;
        while f.push(healthy()).stage != Stage::None {
            n += 1;
            assert!(n < 10, "never changed");
        }
        assert!(n >= 2, "changed after {n} more seconds");
        // a single bad second among healthy ones does not show
        for _ in 0..3 {
            f.push(healthy());
        }
        assert_eq!(f.push(users_case()).stage, Stage::None);
        assert_eq!(f.push(healthy()).stage, Stage::None);
    }

    #[test]
    fn stage_bars_name_the_long_pole() {
        let e = users_case();
        let bars = e.stage_times();
        let long = bars.iter().max_by(|a, b| a.ms.total_cmp(&b.ms)).unwrap();
        assert_eq!(long.name, "host app");
        assert!(long.ms > e.budget_ms());
    }
}
