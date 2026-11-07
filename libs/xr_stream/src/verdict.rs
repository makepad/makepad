//! What limits the stream, in plain words: the stage that keeps the headset from showing a new
//! picture every display frame, with the numbers that show it, and a hint the panel's own
//! controls can act on (only when that stage takes the budget).
//!
//! The headset says why a frame repeated (`report::REPEAT_*`): the picture was late (in flight
//! or decoding), the server never sent it, or the headset let it go at its queue cap (pictures
//! came in a burst). The link is reliable, so a picture never sent is the host's; a late one is
//! the host's or the link's by which of them had pictures a frame late (the server counts both).
//! The stage times say which part is slow: the host app's frame (its UI thread's whole time per
//! frame, the draw and what is outside it: events, the frame build, the submit; its GPU passes,
//! split into the headset's eyes, the desktop's windows and the stream's own passes), the
//! encode, the send, the headset's decode and its own render. Motion to photon is shown part by
//! part ([`Evidence::m2p_rows`]): every millisecond of it in a row.
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
    /// The headset's stages (ms): transit (both ways), decode, its own render (0: not measured).
    pub net_ms: f64,
    pub decode_ms: f64,
    pub client_gpu_ms: f64,
    /// The headset: motion to photon, a picture's wait from decoded to its display frame (the
    /// queue), how far ahead the poses are predicted (ms; 0: not measured).
    pub m2p_ms: f64,
    pub queue_ms: f64,
    pub lead_ms: f64,
    /// The server: the headset's poses that came and the frames built for them (per s), pose to
    /// frame built and of it the pose's wait for the app to ask for it, the GPU's part, encode,
    /// send (ms; 0 before its first window).
    pub poses_per_s: f64,
    pub built_per_s: f64,
    pub render_ms: f64,
    pub app_wait_ms: f64,
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
    /// Pictures a frame or more later than the median, per second: on the host (pose to sent)
    /// and on the link (round trip over the idle link's).
    pub host_late_per_s: f64,
    pub link_late_per_s: f64,
    /// The host app's load (`Cx::take_pass_load`): GPU ms per second for the headset's eyes, the
    /// desktop's windows and the passes under no window, the draws' CPU ms per second, and the
    /// UI thread's ms per second outside its idle wait with the frame builds and submits in it.
    /// `host_timed`: the GPU times are measured.
    pub eyes_gpu_ms_s: f64,
    pub desktop_gpu_ms_s: f64,
    pub other_gpu_ms_s: f64,
    pub draw_ms_s: f64,
    pub busy_ms_s: f64,
    pub build_ms_s: f64,
    pub submit_ms_s: f64,
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

/// One row of the bars: its name, ms, and whether it is measured.
#[derive(Clone, Debug, PartialEq)]
pub struct StageTime {
    pub name: &'static str,
    pub ms: f64,
    pub measured: bool,
}

/// The repeats per second by where they come from (`Evidence::repeat_split`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RepeatSplit {
    /// The host never sent the picture (it built fewer frames than the poses that came, or let
    /// one go): the link is reliable, nothing sent is lost on it.
    pub host_unsent: f64,
    /// Late, by the pictures a frame late on the host (built late) and on the link.
    pub host_late: f64,
    pub link_late: f64,
    /// Late with neither the host's nor the link's pictures a frame late (spread, or both
    /// unmeasured).
    pub late_unexplained: f64,
    pub cap: f64,
    pub nopose: f64,
}

impl RepeatSplit {
    pub fn host(&self) -> f64 {
        self.host_unsent + self.host_late
    }
}

impl Evidence {
    pub fn budget_ms(&self) -> f64 {
        1000.0 / self.display_hz.max(1.0)
    }

    /// Per frame the host built (ms per second of a load over the frames built).
    fn per_built(&self, ms_per_s: f64) -> f64 {
        if self.built_per_s > 0.5 { ms_per_s / self.built_per_s } else { 0.0 }
    }

    /// The UI thread's whole time per frame built (events, the draw, the frame build, the
    /// submit; 0: not measured).
    pub fn host_ui_ms(&self) -> f64 {
        self.per_built(self.busy_ms_s)
    }

    /// The host's GPU time per frame built, all its passes (the desktop's share the GPU; 0: not
    /// measured).
    pub fn host_gpu_ms(&self) -> f64 {
        if self.host_timed { self.per_built(self.eyes_gpu_ms_s + self.desktop_gpu_ms_s + self.other_gpu_ms_s) } else { 0.0 }
    }

    /// The host app's frame (ms): the longer of its UI thread's time and its GPU's per frame
    /// built when measured, else the frames' interval while it builds fewer than the display
    /// shows, else the server's GPU stage.
    pub fn host_ms(&self) -> f64 {
        let measured = self.host_ui_ms().max(self.host_gpu_ms());
        if measured > 0.0 {
            return measured;
        }
        if self.built_per_s > 0.5 && self.built_per_s < self.display_hz * 0.95 {
            1000.0 / self.built_per_s
        } else {
            self.gpu_ms
        }
    }

    /// The host's frame interval (ms) while it builds fewer frames than the headset asks for
    /// (it missed some: its stalls, though its mean frame fits the budget); 0 while it builds
    /// them all.
    pub fn host_interval_ms(&self) -> f64 {
        let asked = if self.poses_per_s > 0.5 { self.poses_per_s } else { self.display_hz };
        if self.built_per_s > 0.5 && self.built_per_s < asked * 0.95 { 1000.0 / self.built_per_s } else { 0.0 }
    }

    /// The UI thread's time per frame built outside the app's draw, by part, the larger first:
    /// "submit 8.3 ms, frame build 0.8 ms, events 0.5 ms" (parts of 0.5 ms or more).
    pub fn outside_draw(&self) -> (f64, String) {
        let ui = self.host_ui_ms();
        if ui <= 0.0 {
            return (0.0, String::new());
        }
        let draw = self.per_built(self.draw_ms_s);
        let (submit, build) = (self.per_built(self.submit_ms_s), self.per_built(self.build_ms_s));
        let mut parts = [("submit", submit), ("frame build", build), ("events", (ui - draw - submit - build).max(0.0))];
        parts.sort_by(|a, b| b.1.total_cmp(&a.1));
        let named: Vec<String> = parts.iter().filter(|p| p.1 >= 0.5).map(|(n, ms)| format!("{n} {ms:.1} ms")).collect();
        ((ui - draw).max(0.0), named.join(", "))
    }

    /// The per-frame rows, in the picture's order: each stage works on its own picture, so each
    /// has the whole frame budget (they do not add).
    pub fn stage_times(&self) -> Vec<StageTime> {
        let server = self.built_per_s > 0.0;
        let ui = self.host_ui_ms();
        vec![
            // what limits the host: its frames' interval while it misses frames, else its frame
            StageTime { name: "host app", ms: if ui > 0.0 { ui.max(self.host_interval_ms()) } else { self.host_ms() }, measured: server },
            StageTime { name: "host GPU", ms: self.host_gpu_ms(), measured: server && self.host_timed },
            StageTime { name: "encode", ms: self.encode_ms, measured: server },
            StageTime { name: "send", ms: self.send_ms, measured: server },
            StageTime { name: "headset decode", ms: self.decode_ms, measured: self.decode_ms > 0.0 },
            StageTime { name: "headset render", ms: self.client_gpu_ms, measured: self.client_gpu_ms > 0.0 },
        ]
    }

    /// Motion to photon, part by part, in order (they add up to `m2p_ms`): the pose's wait for
    /// the app, the app's frame, the GPU and the pack, the encode, the send, the network both
    /// ways, the decode, the headset's queue, and the display (the rest: the headset's frame from
    /// taking the picture to its light, and the pose on its way out).
    pub fn m2p_rows(&self) -> Vec<StageTime> {
        if self.m2p_ms <= 0.0 {
            return Vec::new();
        }
        let server = self.built_per_s > 0.0;
        let mut rows = vec![
            StageTime { name: "waits for app", ms: self.app_wait_ms, measured: server },
            StageTime { name: "app frame", ms: (self.render_ms - self.app_wait_ms).max(0.0), measured: server },
            StageTime { name: "GPU, pack", ms: self.gpu_ms, measured: server },
            StageTime { name: "encode", ms: self.encode_ms, measured: server },
            StageTime { name: "send", ms: self.send_ms, measured: server },
            StageTime { name: "network", ms: self.net_ms, measured: true },
            StageTime { name: "decode", ms: self.decode_ms, measured: true },
            StageTime { name: "headset queue", ms: self.queue_ms, measured: true },
        ];
        let parts: f64 = rows.iter().filter(|r| r.measured).map(|r| r.ms).sum();
        rows.push(StageTime { name: "display", ms: (self.m2p_ms - parts).max(0.0), measured: true });
        rows
    }

    /// The repeats by where they come from. The headset's "never sent" are the host's (the
    /// link is reliable: what is sent arrives); its late ones go to the host and the link by
    /// their pictures a frame late (the host's: built late; the link's: in transit), to the link
    /// alone when it queues without counts, else unexplained.
    pub fn repeat_split(&self) -> RepeatSplit {
        let late = self.repeat_late;
        let (host, link) = (self.host_late_per_s, self.link_late_per_s);
        let (host_late, link_late, late_unexplained) = if host + link > 0.0 {
            let explained = late.min(host + link);
            (explained * host / (host + link), explained * link / (host + link), late - explained)
        } else if self.trip_ms - self.trip_base_ms > self.budget_ms() * 0.5 {
            (0.0, late, 0.0)
        } else {
            (0.0, 0.0, late)
        };
        RepeatSplit { host_unsent: self.repeat_server, host_late, link_late, late_unexplained, cap: self.repeat_cap, nopose: self.repeat_nopose }
    }

    /// The host's evidence: what it built, its frame on the UI thread and the GPU, the time
    /// outside drawing by part, and (its means inside the budget) its stalls.
    fn host_evidence(&self, split: &RepeatSplit, repeats: f64) -> String {
        let budget = self.budget_ms();
        let mut out = format!("{:.0} of {repeats:.0} repeats from the host", split.host());
        if split.host_late >= 0.5 {
            out += &format!(" ({:.0} unsent, {:.0} built late)", split.host_unsent, split.host_late);
        }
        if self.built_per_s > 0.5 {
            let poses = if self.poses_per_s > 0.5 { self.poses_per_s } else { self.display_hz };
            out += &format!(": it built {:.0} of {poses:.0} frames/s", self.built_per_s);
        }
        let ui = self.host_ui_ms();
        if ui > 0.0 {
            let (outside, parts) = self.outside_draw();
            out += &format!(", {:.1} ms per frame outside drawing", outside);
            if !parts.is_empty() {
                out += &format!(": {parts}");
            }
            out += &format!(" (UI thread {ui:.1} ms per frame");
            let gpu = self.host_gpu_ms();
            if gpu > 0.0 {
                out += &format!(", GPU {gpu:.1} ms");
            }
            out += &format!(", budget {budget:.1} ms)");
            if ui.max(gpu) <= budget * 0.85 {
                out += &format!("; within the budget on average: it stalls now and then (poses waited {:.1} ms for it)", self.app_wait_ms);
            }
        } else if self.built_per_s > 0.5 {
            out += &format!(", {:.1} ms each, budget {budget:.1} ms", 1000.0 / self.built_per_s);
        }
        out
    }

    /// A hint for the host only when one of its parts takes the budget: its GPU (the desktop's
    /// views or the eyes), or its UI thread.
    fn host_hint(&self) -> &'static str {
        let budget = self.budget_ms();
        let gpu = self.host_gpu_ms();
        if gpu > budget * 0.85 {
            return if self.per_built(self.desktop_gpu_ms_s) > self.per_built(self.eyes_gpu_ms_s) {
                "turn off Desktop mirror, and close or pause other views of the app's desktop"
            } else {
                "lower Render scale or Anti-aliasing"
            };
        }
        if self.host_ui_ms() > budget * 0.85 {
            return "lower Frame rate";
        }
        ""
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
        if repeats < 0.5 {
            return v(Stage::Unclear, format!("{:.0} of {:.0} frames/s shown, no repeats counted", self.shown, d), "");
        }
        let split = self.repeat_split();
        let of = |n: f64, what: &str| format!("{n:.0} of {repeats:.0} repeats {what}");
        let decode_slow = self.decode_ms > budget * 0.85;
        let link_queued = over_idle > budget * 0.5;
        let at_cap = self.cap_mbps > 0.0 && self.sent_mbps >= self.cap_mbps * 0.9;
        let causes = [split.host(), split.link_late, split.cap, split.nopose, split.late_unexplained];
        let top = (0..causes.len()).max_by(|a, b| causes[*a].total_cmp(&causes[*b])).unwrap();
        match top {
            // the host: frames never built or sent, or built late
            0 => {
                // held back by the headset (too many pictures in flight): what is downstream
                if self.waited_per_s > d * 0.1 {
                    if decode_slow {
                        return v(Stage::HeadsetDecode, format!("{}: the host waited for the headset {:.0}/s, decode {:.1} ms, budget {budget:.1} ms", of(split.host(), "from the host"), self.waited_per_s, self.decode_ms), "lower Bandwidth or Frame rate");
                    }
                    if link_queued {
                        return v(Stage::Network, format!("{}: the host waited for the headset {:.0}/s, round trip {over_idle:.0} ms over idle", of(split.host(), "from the host"), self.waited_per_s), "lower Bandwidth");
                    }
                }
                if self.encode_ms > budget * 0.85 {
                    return v(Stage::Encoder, format!("{}: encode {:.1} ms, budget {budget:.1} ms", of(split.host(), "from the host"), self.encode_ms), "lower Frame rate");
                }
                v(Stage::HostRendering, self.host_evidence(&split, repeats), self.host_hint())
            }
            // late in transit
            1 => {
                if decode_slow {
                    v(Stage::HeadsetDecode, format!("{}: decode {:.1} ms, budget {budget:.1} ms", of(split.link_late, "late"), self.decode_ms), "lower Bandwidth or Frame rate")
                } else if at_cap && link_queued {
                    v(Stage::Bandwidth, format!("{}: sending {:.0} of {:.0} Mbit/s, round trip {over_idle:.0} ms over idle", of(split.link_late, "late on the link"), self.sent_mbps, self.cap_mbps), "lower Bandwidth")
                } else {
                    let spikes = if self.link_late_per_s > 0.0 { format!("{:.0} pictures/s a frame late in transit, ", self.link_late_per_s) } else { String::new() };
                    v(Stage::Network, format!("{}: {spikes}round trip {:.0} ms, {over_idle:.0} ms over idle", of(split.link_late, "late on the link"), self.trip_ms), if link_queued { "lower Bandwidth" } else { "" })
                }
            }
            // let go at the headset's queue cap: pictures came in bursts
            2 => {
                if self.client_gpu_ms > budget * 0.9 {
                    v(Stage::HeadsetRendering, format!("{}: headset GPU {:.1} ms, budget {budget:.1} ms", of(split.cap, "dropped at the queue cap"), self.client_gpu_ms), "turn off Room scan or lower Frame rate")
                } else {
                    v(Stage::Network, format!("{}: pictures arrive in bursts, round trip {over_idle:.0} ms over idle", of(split.cap, "dropped at the queue cap")), if link_queued { "lower Bandwidth" } else { "" })
                }
            }
            // no pose went up for the frame: the headset's own frame
            3 => v(Stage::HeadsetRendering, format!("{}: headset GPU {:.1} ms, budget {budget:.1} ms", of(split.nopose, "without a pose"), self.client_gpu_ms), "turn off Room scan or lower Frame rate"),
            // late, and neither the host's pictures nor the link's were a frame late
            _ => {
                if decode_slow {
                    return v(Stage::HeadsetDecode, format!("{}: decode {:.1} ms, budget {budget:.1} ms", of(split.late_unexplained, "late"), self.decode_ms), "lower Bandwidth or Frame rate");
                }
                v(Stage::Unclear, format!("{}, no stage a frame late: round trip {:.0} ms ({over_idle:.0} over idle), host {:.1} ms per frame", of(split.late_unexplained, "late"), self.trip_ms, self.host_ms()), "")
            }
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
            m2p_ms: m(|e| e.m2p_ms),
            queue_ms: m(|e| e.queue_ms),
            lead_ms: m(|e| e.lead_ms),
            poses_per_s: m(|e| e.poses_per_s),
            built_per_s: m(|e| e.built_per_s),
            render_ms: m(|e| e.render_ms),
            app_wait_ms: m(|e| e.app_wait_ms),
            gpu_ms: m(|e| e.gpu_ms),
            encode_ms: m(|e| e.encode_ms),
            send_ms: m(|e| e.send_ms),
            trip_ms: m(|e| e.trip_ms),
            trip_base_ms: m(|e| e.trip_base_ms),
            sent_mbps: m(|e| e.sent_mbps),
            cap_mbps: m(|e| e.cap_mbps),
            waited_per_s: m(|e| e.waited_per_s),
            host_late_per_s: m(|e| e.host_late_per_s),
            link_late_per_s: m(|e| e.link_late_per_s),
            eyes_gpu_ms_s: m(|e| e.eyes_gpu_ms_s),
            desktop_gpu_ms_s: m(|e| e.desktop_gpu_ms_s),
            other_gpu_ms_s: m(|e| e.other_gpu_ms_s),
            draw_ms_s: m(|e| e.draw_ms_s),
            busy_ms_s: m(|e| e.busy_ms_s),
            build_ms_s: m(|e| e.build_ms_s),
            submit_ms_s: m(|e| e.submit_ms_s),
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
        assert!(v.evidence.starts_with("18 of 23 repeats from the host"), "{}", v.evidence);
        assert!(v.evidence.contains("64 of 87 frames/s") && v.evidence.contains("15.6 ms each") && v.evidence.contains("budget 11.5 ms"), "{}", v.evidence);
        assert!(v.line().starts_with("Limited by: host app rendering ("), "{}", v.line());
    }

    /// The user's stream at 20:38 (72 Hz): "14 of 21 repeats unsent: the host built 67 of 72
    /// frames/s, 15.0 ms each: desktop view 2 ms + eyes 1 ms", the host bar at 2.3 ms. The UI
    /// thread's split is the same show's recording in a dip: per second 728 ms busy, 559 of
    /// them submitting, 51 building frames, 84 drawing.
    fn host_limited_72() -> Evidence {
        Evidence {
            display_hz: 72.0,
            shown: 51.0,
            received: 67.0,
            repeat_late: 7.0,
            repeat_server: 14.0,
            net_ms: 3.6,
            decode_ms: 4.9,
            client_gpu_ms: 4.7,
            poses_per_s: 72.0,
            built_per_s: 67.0,
            render_ms: 9.0,
            app_wait_ms: 4.0,
            gpu_ms: 3.0,
            encode_ms: 0.9,
            send_ms: 1.0,
            trip_ms: 4.0,
            trip_base_ms: 2.0,
            sent_mbps: 17.0,
            cap_mbps: 80.0,
            eyes_gpu_ms_s: 1.0 * 67.0,
            desktop_gpu_ms_s: 2.0 * 67.0,
            other_gpu_ms_s: 0.2 * 67.0,
            draw_ms_s: 84.4,
            busy_ms_s: 727.9,
            build_ms_s: 51.2,
            submit_ms_s: 559.4,
            host_timed: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_users_72_hz_case_names_the_time_outside_drawing() {
        let e = host_limited_72();
        let v = e.verdict();
        assert_eq!(v.stage, Stage::HostRendering, "{}", v.line());
        assert!(v.evidence.starts_with("14 of 21 repeats from the host: it built 67 of 72 frames/s"), "{}", v.evidence);
        assert!(v.evidence.contains("9.6 ms per frame outside drawing: submit 8.3 ms, frame build 0.8 ms"), "{}", v.evidence);
        assert!(v.evidence.contains("UI thread 10.9 ms per frame, GPU 3.2 ms, budget 13.9 ms"), "{}", v.evidence);
        // on average inside the budget: stalls, and no hint (the desktop's 2 ms of GPU is no limit)
        assert!(v.evidence.contains("it stalls now and then"), "{}", v.evidence);
        assert!(v.hint.is_empty(), "{}", v.hint);
        // the host row: the frames' interval while it misses frames (red: over the budget)
        let host = &e.stage_times()[0];
        assert_eq!(host.name, "host app");
        assert!((host.ms - 1000.0 / 67.0).abs() < 0.01 && host.ms > e.budget_ms(), "{}", host.ms);
        // building them all, the UI thread's whole frame (not the draw alone)
        let mut all = e;
        all.built_per_s = 72.0;
        all.busy_ms_s = 10.86 * 72.0;
        assert!((all.stage_times()[0].ms - 10.86).abs() < 0.01, "{}", all.stage_times()[0].ms);
        // in a dip (41 built): the UI thread takes the budget, the hint is the frame rate
        let mut dip = e;
        dip.built_per_s = 41.0;
        dip.eyes_gpu_ms_s = 1.0 * 41.0;
        dip.desktop_gpu_ms_s = 2.0 * 41.0;
        dip.other_gpu_ms_s = 0.2 * 41.0;
        let v = dip.verdict();
        assert_eq!(v.stage, Stage::HostRendering);
        assert!(v.evidence.contains("UI thread 17.8 ms per frame"), "{}", v.evidence);
        assert_eq!(v.hint, "lower Frame rate");
        assert!(dip.stage_times()[0].ms > dip.budget_ms());
    }

    #[test]
    fn host_hints_only_for_the_part_that_takes_the_budget() {
        let mut e = host_limited_72();
        // the desktop's views take the GPU: the mirror hint
        e.desktop_gpu_ms_s = 11.0 * 67.0;
        assert!(e.verdict().hint.contains("Desktop mirror"), "{}", e.verdict().hint);
        // the eyes take it: render scale
        e.desktop_gpu_ms_s = 1.0 * 67.0;
        e.eyes_gpu_ms_s = 12.0 * 67.0;
        assert!(e.verdict().hint.contains("Render scale"), "{}", e.verdict().hint);
    }

    /// The user's stream at 20:40 (72 Hz): "network (17 of 24 repeats late: round trip 4 ms,
    /// 2 ms over idle)", motion to photon 91 ms of which the bars showed 17.
    fn late_72() -> Evidence {
        Evidence {
            display_hz: 72.0,
            shown: 48.0,
            received: 66.0,
            repeat_late: 17.0,
            repeat_server: 7.0,
            net_ms: 3.4,
            decode_ms: 4.3,
            client_gpu_ms: 4.6,
            m2p_ms: 91.0,
            queue_ms: 30.0,
            lead_ms: 69.0,
            poses_per_s: 72.0,
            built_per_s: 66.0,
            render_ms: 9.0,
            app_wait_ms: 3.0,
            gpu_ms: 4.0,
            encode_ms: 0.9,
            send_ms: 1.0,
            trip_ms: 4.0,
            trip_base_ms: 2.0,
            sent_mbps: 19.0,
            cap_mbps: 80.0,
            ..Default::default()
        }
    }

    #[test]
    fn late_pictures_are_the_network_only_with_the_links_evidence() {
        // a 2 ms longer round trip and no picture a frame late anywhere: not the network
        let v = late_72().verdict();
        assert_eq!(v.stage, Stage::Unclear, "{}", v.line());
        assert!(v.evidence.starts_with("17 of 24 repeats late, no stage a frame late: round trip 4 ms (2 over idle)"), "{}", v.evidence);
        assert!(v.hint.is_empty());
        // the host built 12 pictures/s a frame late: those late repeats are the host's
        let mut e = late_72();
        e.host_late_per_s = 12.0;
        let v = e.verdict();
        assert_eq!(v.stage, Stage::HostRendering, "{}", v.line());
        assert!(v.evidence.starts_with("19 of 24 repeats from the host (7 unsent, 12 built late)"), "{}", v.evidence);
        // pictures a frame late in transit: the network, without a bandwidth hint on a link that
        // does not queue
        let mut e = late_72();
        e.link_late_per_s = 12.0;
        let v = e.verdict();
        assert_eq!(v.stage, Stage::Network, "{}", v.line());
        assert!(v.evidence.starts_with("12 of 24 repeats late on the link: 12 pictures/s a frame late in transit"), "{}", v.evidence);
        assert!(v.hint.is_empty(), "{}", v.hint);
    }

    #[test]
    fn motion_to_photon_rows_add_up() {
        let e = late_72();
        let rows = e.m2p_rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name).collect();
        assert_eq!(names, ["waits for app", "app frame", "GPU, pack", "encode", "send", "network", "decode", "headset queue", "display"]);
        let sum: f64 = rows.iter().map(|r| r.ms).sum();
        assert!((sum - 91.0).abs() < 1e-9, "{sum}");
        assert!((rows[1].ms - 6.0).abs() < 1e-9, "app frame = render less its wait");
        assert!((rows[8].ms - 38.4).abs() < 1e-9, "display = the rest: {}", rows[8].ms);
        assert!(Evidence::default().m2p_rows().is_empty());
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
