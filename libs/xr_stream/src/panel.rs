//! The XR stream's control in an app's caption: [`XrCaptionButton`], a headset icon (grey: no
//! headset, amber: waiting for one, green: one streams) opening a dropdown with the stream's
//! numbers (the headset's and the server's lines, the last minute as bars and the last seconds
//! frame by frame) and the user's settings: the stream on/off, the room scan, the bandwidth, the
//! headset's frame rate, the anti-aliasing and VR or mixed reality. An app adds it with one line
//! in its caption (`xr := XrCaptionButton{app: "sandbox"}`), which also starts the app's XR server
//! (headsets find it; one linking gives the app its XR display). The settings are kept per app (`app`) in its data folder
//! (`home::app_data_dir`, file `xr-stream`) and applied at start and on every change.

use crate::auto::{Lever, XrAuto};
use crate::settings_types::{StreamAa, StreamView, STREAM_AA_MODES};
#[cfg(not(target_arch = "wasm32"))]
use crate::XrServer;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.XrHeadsetIconBase = #(XrHeadsetIcon::register_widget(vm))
    mod.widgets.XrHeadsetIcon = set_type_default() do mod.widgets.XrHeadsetIconBase {
        width: 26 height: 22
        icon +: { svg: crate_resource("self:resources/icons/headset.svg") }
        color_off: theme.color_text_meta
        color_waiting: #xE8A33A
        color_live: #x5BD38A
    }

    mod.widgets.XrStatsViewBase = #(XrStatsView::register_widget(vm))
    mod.widgets.XrStatsView = set_type_default() do mod.widgets.XrStatsViewBase {
        width: 560 height: 300
        draw_text +: { color: theme.color_text text_style: theme.font_code{font_size: 7.5} }
        draw_bar +: {}
    }

    mod.widgets.XrSettingRow = View{
        width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
    }
    mod.widgets.XrSettingLabel = Label{
        width: 110
        draw_text +: { color: theme.color_text_meta }
    }

    mod.widgets.XrCaptionButtonBase = #(XrCaptionButton::register_widget(vm))
    mod.widgets.XrCaptionButton = set_type_default() do mod.widgets.XrCaptionButtonBase {
        width: Fit height: Fit
        app: "makepad"
        popover: mod.widgets.Popover{
            width: Fit height: Fit
            trigger: mod.widgets.Click
            placement: mod.widgets.BottomStart
            icon := mod.widgets.XrHeadsetIcon{}
            content := View{
                width: 576 height: Fit flow: Down spacing: 6
                headline := Label{ text: "XR headset" draw_text +: { text_style: theme.font_bold{} } }
                stats := mod.widgets.XrStatsView{}
                streaming := CheckBox{ text: "Stream to the headset (the left menu button too)" }
                room_scan := CheckBox{ text: "Room scan (while the app uses the room)" }
                mirror := CheckBox{ text: "Mirror the headset on the desktop (a few frames a second)" }
                mod.widgets.XrSettingRow{
                    mod.widgets.XrSettingLabel{ text: "Bandwidth" }
                    bitrate := DropDown{ width: 180 labels: ["Auto", "40 Mbit/s", "60 Mbit/s", "80 Mbit/s", "100 Mbit/s", "120 Mbit/s", "150 Mbit/s"] }
                }
                mod.widgets.XrSettingRow{
                    mod.widgets.XrSettingLabel{ text: "Frame rate" }
                    fps := DropDown{ width: 180 labels: ["72 fps", "80 fps", "90 fps", "120 fps"] }
                    Label{ text: "relinks: a second without a picture" draw_text +: { color: theme.color_text_meta } }
                }
                mod.widgets.XrSettingRow{
                    mod.widgets.XrSettingLabel{ text: "Anti-aliasing" }
                    aa := DropDown{ width: 180 labels: ["Auto", "Off", "FXAA", "MSAA 2x", "MSAA 4x", "MSAA 8x", "MSAA 4x + FXAA", "Supersampled 1.25x", "Supersampled 1.5x", "Supersampled 2x"] }
                }
                mod.widgets.XrSettingRow{
                    mod.widgets.XrSettingLabel{ text: "Render scale" }
                    render_scale := DropDown{ width: 180 labels: ["Auto", "100%", "90%", "80%", "70%"] }
                    Label{ text: "of the streamed eye size" draw_text +: { color: theme.color_text_meta } }
                }
                mod.widgets.XrSettingRow{
                    mod.widgets.XrSettingLabel{ text: "View" }
                    view := DropDown{ width: 180 labels: ["The app decides", "Mixed reality", "VR"] }
                }
                mod.widgets.XrSettingRow{
                    mod.widgets.XrSettingLabel{ text: "Headset PIN" }
                    pin := TextInput{ width: 90 empty_text: "5 digits" }
                    pin_state := Label{ text: "" draw_text +: { color: theme.color_text_meta } }
                }
                auto_note := Label{ width: Fill text: "" draw_text +: { color: theme.color_text_meta } }
            }
        }
    }
}

/// The stream bitrates offered (kbit/s, all tiles), in the bandwidth dropdown's order.
pub const STREAM_BITRATES_KBPS: [u32; 6] = [40_000, 60_000, 80_000, 100_000, 120_000, 150_000];
/// The headset frame rates offered (Hz), in the frame rate dropdown's order.
pub const STREAM_FPS: [u32; 4] = [72, 80, 90, 120];
/// The render scales offered (percent), after the dropdown's Auto.
const STREAM_RENDER_SCALES: [u32; 4] = [100, 90, 80, 70];
/// The views offered, in the view dropdown's order.
const STREAM_VIEWS: [StreamView; 3] = [StreamView::App, StreamView::Mixed, StreamView::Vr];

/// The user's stream settings, kept per app. `None`: the setting is on Auto (`crate::auto`).
#[derive(Clone, Debug, PartialEq)]
pub struct XrStreamSettings {
    pub streaming: bool,
    pub room_scan: bool,
    pub kbps: Option<u32>,
    pub hz: u32,
    pub aa: Option<StreamAa>,
    pub render_scale_pct: Option<u32>,
    pub view: StreamView,
    /// The desktop's mirror of the headset's view repaints from its frames (else only when
    /// the desktop's own UI changes).
    pub mirror: bool,
}

impl Default for XrStreamSettings {
    fn default() -> Self {
        XrStreamSettings { streaming: true, room_scan: true, kbps: None, hz: 90, aa: None, render_scale_pct: None, view: StreamView::App, mirror: true }
    }
}

impl XrStreamSettings {
    fn path(app: &str) -> Option<std::path::PathBuf> {
        Some(makepad_widgets::makepad_platform::home::app_data_dir(app)?.join("xr-stream"))
    }

    /// The app's kept settings (defaults where none are kept).
    pub fn load(app: &str) -> Self {
        let mut s = XrStreamSettings::default();
        let Some(text) = Self::path(app).and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.trim().split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "streaming" => s.streaming = v != "off",
                "room_scan" => s.room_scan = v != "off",
                "mirror" => s.mirror = v != "off",
                "kbps" => s.kbps = v.parse().ok(),
                "hz" => s.hz = v.parse().unwrap_or(s.hz),
                "aa" => s.aa = StreamAa::from_id(v),
                "render_scale" => s.render_scale_pct = v.parse().ok(),
                "view" => s.view = StreamView::from_label(v).unwrap_or(s.view),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self, app: &str) {
        let Some(path) = Self::path(app) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let on = |b: bool| if b { "on" } else { "off" };
        let auto = |v: Option<String>| v.unwrap_or_else(|| "auto".to_string());
        let text = format!("streaming={}\nroom_scan={}\nkbps={}\nhz={}\naa={}\nrender_scale={}\nview={}\nmirror={}\n", on(self.streaming), on(self.room_scan), auto(self.kbps.map(|k| k.to_string())), self.hz, auto(self.aa.map(|a| a.id.to_string())), auto(self.render_scale_pct.map(|p| p.to_string())), self.view.label(), on(self.mirror));
        let _ = std::fs::write(path, text);
    }

    /// Whether the user left `lever` on Auto.
    pub fn auto(&self, lever: Lever) -> bool {
        match lever {
            Lever::Bitrate => self.kbps.is_none(),
            Lever::Aa => self.aa.is_none(),
            Lever::RenderScale => self.render_scale_pct.is_none(),
        }
    }

    /// To the stream server (live): the manual values, Auto's for the rest.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply(&self, auto: &XrAuto, server: &XrServer) {
        let a = auto.values;
        server.set_aa(self.aa.unwrap_or(a.aa));
        server.set_render_scale(self.render_scale_pct.unwrap_or(a.render_scale_pct));
        server.set_view(self.view);
        server.set_mirror(self.mirror);
        server.set_streaming(self.streaming);
        server.set_room_scan(self.room_scan);
        server.set_bitrate_kbps(self.kbps.unwrap_or(a.kbps));
        server.set_refresh_hz(self.hz);
    }
}

/// The headset's link as the icon shows it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum XrLinkState {
    /// This process serves no XR, or the stream is off.
    #[default]
    Off,
    /// The server listens; no headset streams.
    Waiting,
    /// A headset streams.
    Live,
}

fn link_state(cx: &mut Cx) -> XrLinkState {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(server) = XrServer::get(cx) else { return XrLinkState::Off };
        if !server.streaming() {
            return XrLinkState::Off;
        }
        if server.session_eye().is_some() {
            return XrLinkState::Live;
        }
        XrLinkState::Waiting
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = cx;
        XrLinkState::Off
    }
}

/// The caption's headset icon (the popover's anchor), coloured by the link's state.
#[derive(Script, ScriptHook, Widget)]
pub struct XrHeadsetIcon {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    icon: DrawSvg,
    #[live]
    color_off: Vec4f,
    #[live]
    color_waiting: Vec4f,
    #[live]
    color_live: Vec4f,
    #[rust]
    state: XrLinkState,
}

impl XrHeadsetIcon {
    fn set_state(&mut self, cx: &mut Cx, state: XrLinkState) {
        if state != self.state {
            self.state = state;
            self.icon.redraw(cx);
        }
    }
}

impl Widget for XrHeadsetIcon {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.walk_turtle(walk);
        let s = 16.0;
        self.icon.color = match self.state {
            XrLinkState::Off => self.color_off,
            XrLinkState::Waiting => self.color_waiting,
            XrLinkState::Live => self.color_live,
        };
        self.icon.draw_abs(cx, Rect { pos: dvec2(r.pos.x + ((r.size.x - s) * 0.5).floor(), r.pos.y + ((r.size.y - s) * 0.5).floor()), size: dvec2(s, s) });
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// The stream's numbers: the headset's last-second report and the server's line, the last
/// minute of pictures shown per second (green) and motion to photon (orange) as bars, and the
/// last seconds frame by frame (one tick per display frame: new dim, repeat yellow, skip red,
/// odd bright red, late purple, blank white).
#[derive(Script, ScriptHook, Widget)]
pub struct XrStatsView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bar: DrawColor,
    #[live]
    draw_text: DrawText,
}

impl Widget for XrStatsView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // without a headset one line
        #[cfg(not(target_arch = "wasm32"))]
        let numbers = XrServer::get(cx.cx).is_some_and(|s| !s.headset_reports().is_empty());
        #[cfg(target_arch = "wasm32")]
        let numbers = false;
        let walk = if numbers { walk } else { Walk { height: Size::Fixed(16.0), ..walk } };
        let rect = cx.walk_turtle(walk);
        #[cfg(not(target_arch = "wasm32"))]
        self.draw_numbers(cx, rect);
        #[cfg(target_arch = "wasm32")]
        let _ = rect;
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl XrStatsView {
    fn draw_numbers(&mut self, cx: &mut Cx2d, rect: Rect) {
        use makepad_desk_link::xr::report as r;
        let Some(server) = XrServer::get(cx.cx) else { return };
        let reports = server.headset_reports();
        let at = |k: usize| rect.pos + dvec2(0.0, k as f64 * 15.0);
        let Some(last) = reports.last() else {
            let line = match link_state(cx.cx) {
                XrLinkState::Off => "the stream is off: nothing drawn, encoded or sent".to_string(),
                _ => "waiting for a headset (the XR Remote app on the headset finds this computer)".to_string(),
            };
            self.draw_text.draw_abs(cx, at(0), &line);
            return;
        };
        let v = |i: usize| last.get(i).copied().unwrap_or(0.0);
        let mut lines = vec![
            format!("headset {:.0} Hz · shown {:.0}/s · BLANK {:.1}/s · skipped {:.1}/s · received {:.0}/s · battery {}{}", v(r::DISPLAY), v(r::SHOWN), v(r::BLANK), v(r::SKIPPED), v(r::RECEIVED),
                if v(r::BATTERY_PCT) < 0.0 { "?".to_string() } else { format!("{:.0}%", v(r::BATTERY_PCT)) }, if v(r::BATTERY_CHARGING) > 0.5 { " charging" } else { "" }),
            format!("repeats {:.1}/s: late {:.1} · server skipped {:.1} · queue cap {:.1} · no pose {:.1}", v(r::REPEATS), v(r::REPEAT_LATE), v(r::REPEAT_SERVER), v(r::REPEAT_DROPPED), v(r::REPEAT_NOPOSE)),
            format!("motion to photon {:.0} ms (net {:.1} decode {:.1} queue {:.1}) · lead {:.0} ms ({:.0} frames)", v(r::M2P_MS), v(r::NET_MS), v(r::DECODE_MS), v(r::WAIT_MS), v(r::LEAD_MS), v(r::AHEAD)),
            format!("uplink {:.0} poses/s ±{:.2} ms · env depth age {:.0} ms · headset GPU {:.1} of {:.1} ms, eye buffers {:.0}%", v(r::UPLINK), v(r::UPLINK_SD_MS), v(r::ENV_AGE_MS), v(r::CLIENT_GPU_MS), 1000.0 / v(r::DISPLAY).max(1.0), v(r::EYE_SCALE_PCT)),
            format!("{} · AA {}", server.server_line(), server.aa().name),
            if v(r::AUDIO_RECEIVED) > 0.0 {
                format!("audio {:.0} ms (net {:.1} buffer {:.0}/{:.0} device {:.0}; pictures {:.0}) · lost {:.0} · underruns {:.0} · drift {:+.0} ppm",
                    v(r::AUDIO_MS), v(r::AUDIO_NET_MS), v(r::AUDIO_BUFFER_MS), v(r::AUDIO_TARGET_MS), v(r::AUDIO_DEVICE_MS), v(r::AUDIO_VIDEO_MS), v(r::AUDIO_LOST) + v(r::AUDIO_LATE), v(r::AUDIO_UNDERRUNS), v(r::AUDIO_DRIFT_PPM))
            } else {
                "audio: the headset hears none".to_string()
            },
        ];
        let app = server.app_line();
        if !app.is_empty() {
            lines.push(app);
        }
        for (k, l) in lines.iter().enumerate() {
            self.draw_text.draw_abs(cx, at(k), l);
        }
        // two graphs of the last minute, a bar per second, each on its own labelled scale:
        // frames shown per second against the display rate (repeats yellow, blank white above
        // the bar), and motion to photon against 100 ms
        let full = v(r::DISPLAY).max(1.0) as f64;
        let width = rect.size.x.min(560.0) - 40.0;
        let bw = width / 60.0;
        let x0 = rect.pos.x + 40.0;
        let gh = 36.0;
        let mut y = rect.pos.y + lines.len() as f64 * 15.0 + 8.0;
        let line_at = |me: &mut Self, cx: &mut Cx2d, y: f64, color: Vec4f| {
            me.draw_bar.color = color;
            me.draw_bar.draw_abs(cx, Rect { pos: dvec2(x0, y), size: dvec2(width, 1.0) });
        };
        // (1) frames shown /s
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y), &format!("Frames shown /s (last minute; line: the display's {:.0} Hz; yellow: repeats, white: blank)", full));
        y += 14.0;
        let base = y + gh;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y - 2.0), &format!("{:.0}", full));
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, base - 10.0), "0");
        let scale = gh / (full * 1.1);
        line_at(self, cx, base - full * scale, vec4(0.6, 0.6, 0.6, 0.7));
        line_at(self, cx, base, vec4(0.35, 0.35, 0.35, 0.7));
        for (k, rep) in reports.iter().enumerate() {
            let x = x0 + k as f64 * bw;
            let get = |i: usize| rep.get(i).copied().unwrap_or(0.0) as f64;
            let shown = (get(r::SHOWN) * scale).clamp(0.0, gh);
            self.draw_bar.color = vec4(0.3, 0.85, 0.4, 0.9);
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, base - shown), size: dvec2(bw * 0.8, shown.max(1.0)) });
            let rep_h = (get(r::REPEATS) * scale * 4.0).clamp(0.0, 8.0);
            if rep_h > 0.0 {
                self.draw_bar.color = vec4(1.0, 0.9, 0.1, 1.0);
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, base - shown - rep_h), size: dvec2(bw * 0.8, rep_h) });
            }
            if get(r::BLANK) > 0.0 {
                self.draw_bar.color = vec4(1.0, 1.0, 1.0, 1.0);
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, base - gh), size: dvec2(bw * 0.8, 3.0) });
            }
        }
        // (2) motion to photon
        y = base + 8.0;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y), "Motion to photon, ms (last minute; line: 100 ms)");
        y += 14.0;
        let base = y + gh;
        let top_ms = 150.0;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y - 2.0), "150");
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, base - 10.0), "0");
        line_at(self, cx, base - 100.0 / top_ms * gh, vec4(0.6, 0.6, 0.6, 0.7));
        line_at(self, cx, base, vec4(0.35, 0.35, 0.35, 0.7));
        for (k, rep) in reports.iter().enumerate() {
            let x = x0 + k as f64 * bw;
            let m2p = (rep.get(r::M2P_MS).copied().unwrap_or(0.0) as f64 / top_ms).clamp(0.0, 1.0) * gh;
            self.draw_bar.color = vec4(1.0, 0.6, 0.2, 0.9);
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, base - m2p), size: dvec2(bw * 0.8, m2p.max(1.0)) });
        }
        // (3) the last ~5 s frame by frame, newest right, one tick per display frame
        let colors = [
            (r::FRAME_NEW, "new", vec4(0.3, 0.45, 0.35, 0.7), 0.4),
            (r::FRAME_REPEAT, "repeat", vec4(1.0, 0.9, 0.1, 1.0), 0.75),
            (r::FRAME_SKIP, "skip", vec4(1.0, 0.25, 0.2, 1.0), 0.75),
            (r::FRAME_ODD, "odd", vec4(1.0, 0.05, 0.05, 1.0), 1.0),
            (r::FRAME_LATE, "late", vec4(0.75, 0.3, 1.0, 1.0), 1.0),
            (r::FRAME_BLANK, "blank", vec4(1.0, 1.0, 1.0, 1.0), 1.0),
        ];
        y = base + 8.0;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y), "Each display frame, last seconds:");
        // the legend: a swatch and a word per code
        let mut lx = rect.pos.x + 200.0;
        for (_, name, color, _) in colors {
            self.draw_bar.color = color;
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(lx, y + 2.0), size: dvec2(8.0, 8.0) });
            self.draw_text.draw_abs(cx, dvec2(lx + 11.0, y), name);
            lx += 14.0 + name.len() as f64 * 6.0 + 8.0;
        }
        y += 14.0;
        let ticks = width as usize;
        let frames: Vec<u8> = {
            let mut all: Vec<u8> = Vec::new();
            for rep in reports.iter().rev().take(5).rev() {
                all.extend(r::frames(rep));
            }
            let n = all.len().saturating_sub(ticks);
            all.split_off(n)
        };
        let (strip_y, strip_h) = (y, 18.0);
        let sx = x0 + width - frames.len() as f64;
        let mut counts = [0u32; 7];
        for (k, c) in frames.iter().enumerate() {
            counts[(*c as usize).min(6)] += 1;
            let (color, h) = colors.iter().find(|e| e.0 == *c).map_or((vec4(0.2, 0.2, 0.2, 0.5), 0.2), |e| (e.2, e.3));
            self.draw_bar.color = color;
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(sx + k as f64, strip_y + strip_h * (1.0 - h)), size: dvec2(1.0, strip_h * h) });
        }
        let secs = (frames.len() as f64 / full).max(1.0);
        let per_s = |c: u8| counts[c as usize] as f64 / secs;
        let line = if frames.is_empty() {
            "per frame: the headset sends no frame codes".to_string()
        } else {
            format!("last {:.0} s per s: repeats {:.1} · skips {:.1} · odd {:.1} · late {:.1} · blank {:.1}", secs, per_s(r::FRAME_REPEAT), per_s(r::FRAME_SKIP), per_s(r::FRAME_ODD), per_s(r::FRAME_LATE), per_s(r::FRAME_BLANK))
        };
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, strip_y + strip_h + 4.0), &line);
    }
}

/// The caption control (see the module). `app`: the name the settings are kept under.
#[derive(Script, ScriptHook, Widget)]
pub struct XrCaptionButton {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[find]
    #[redraw]
    #[live]
    popover: WidgetRef,
    #[walk]
    walk: Walk,
    #[live]
    app: String,
    /// Whether this process serves XR was looked at (`start`).
    #[rust]
    checked: bool,
    #[rust]
    settings: Option<XrStreamSettings>,
    #[rust]
    auto: XrAuto,
    #[rust]
    timer: Timer,
    /// The stream switch's change count last seen (the headset's menu button flips it too).
    #[rust]
    streaming_seen: u32,
}

impl XrCaptionButton {
    /// Once, at the first event or draw: in a process that serves XR, the kept settings to the
    /// server and the once-a-second tick.
    fn start(&mut self, cx: &mut Cx) {
        if self.checked {
            return;
        }
        self.checked = true;
        // the app's XR server (none on the web): headsets find it from now on
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(server) = XrServer::install(cx, &self.app) else { return };
            let s = XrStreamSettings::load(&self.app);
            s.apply(&self.auto, &server);
            self.streaming_seen = server.streaming_changes();
            self.settings = Some(s);
            self.timer = cx.start_interval(1.0);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = cx;
    }

    fn content(&self, path: &[LiveId]) -> WidgetRef {
        self.popover.as_popover().content().child_by_path(path)
    }

    /// The controls to the settings (when the dropdown opens and after the headset's switch).
    fn show_settings(&self, cx: &mut Cx) {
        let Some(s) = &self.settings else { return };
        self.content(ids!(streaming)).as_check_box().set_active(cx, s.streaming, Animate::No);
        self.content(ids!(room_scan)).as_check_box().set_active(cx, s.room_scan, Animate::No);
        self.content(ids!(mirror)).as_check_box().set_active(cx, s.mirror, Animate::No);
        // the dropdowns' first entry is Auto
        let pick = |i: Option<usize>| i.map_or(0, |i| i + 1);
        let bitrate = pick(s.kbps.and_then(|kbps| STREAM_BITRATES_KBPS.iter().position(|k| *k == kbps)));
        self.content(ids!(bitrate)).as_drop_down().set_selected_item(cx, bitrate);
        let fps = STREAM_FPS.iter().position(|h| *h == s.hz).unwrap_or(2);
        self.content(ids!(fps)).as_drop_down().set_selected_item(cx, fps);
        let aa = pick(s.aa.and_then(|aa| STREAM_AA_MODES.iter().position(|m| *m == aa)));
        self.content(ids!(aa)).as_drop_down().set_selected_item(cx, aa);
        let scale = pick(s.render_scale_pct.and_then(|pct| STREAM_RENDER_SCALES.iter().position(|p| *p == pct)));
        self.content(ids!(render_scale)).as_drop_down().set_selected_item(cx, scale);
        let view = STREAM_VIEWS.iter().position(|m| *m == s.view).unwrap_or(0);
        self.content(ids!(view)).as_drop_down().set_selected_item(cx, view);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(p) = XrServer::get(cx).and_then(|s| s.pairing()) {
            self.content(ids!(pin)).as_text_input().set_text(cx, &p.pin);
        }
    }

    /// The PIN row's state: the headset paired, or looked for.
    fn pin_state(&self, cx: &mut Cx) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(server) = XrServer::get(cx) else { return "the XR server has not started".to_string() };
            let Some(p) = server.pairing() else { return "the XR server has not started".to_string() };
            let live = server.live_headset();
            if let Some(h) = &p.paired {
                let linked = live.as_ref().is_some_and(|l| l.instance == h.instance);
                return format!("paired with {} ({:05x}){}", h.name, h.instance & 0xfffff, if linked { ", linked" } else { ", not linked now" });
            }
            if p.clash {
                return "two headsets showed this PIN: one picks a new one, type that".to_string();
            }
            if let Some(h) = &p.matched {
                return format!("{} shows this PIN: linking", h.name);
            }
            if p.pin.is_empty() {
                return match live {
                    Some(l) => format!("{} linked without pairing", l.name),
                    None => "type the PIN the headset shows".to_string(),
                };
            }
            "looking for the headset showing this PIN".to_string()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = cx;
            String::new()
        }
    }

    fn headline(&self, cx: &mut Cx) -> String {
        match link_state(cx) {
            XrLinkState::Off => "XR headset · stream off".to_string(),
            XrLinkState::Waiting => "XR headset · waiting for a headset".to_string(),
            XrLinkState::Live => "XR headset · streaming".to_string(),
        }
    }

    /// What Auto holds and did last, and what it suggests.
    fn auto_line(&self) -> String {
        let Some(s) = &self.settings else { return String::new() };
        let a = self.auto.values;
        let mut on = Vec::new();
        if s.kbps.is_none() {
            on.push(format!("bitrate {} Mbit/s", a.kbps / 1000));
        }
        if s.aa.is_none() {
            on.push(format!("AA {}", a.aa.name));
        }
        if s.render_scale_pct.is_none() {
            on.push(format!("render scale {}%", a.render_scale_pct));
        }
        if on.is_empty() {
            return "Auto: off (every setting chosen)".to_string();
        }
        let mut line = format!("Auto: {}", on.join(" · "));
        if !self.auto.note.is_empty() {
            line.push_str(&format!("\nlast: {}", self.auto.note));
        }
        if !self.auto.suggestion.is_empty() {
            line.push_str(&format!("\nsuggests: {}", self.auto.suggestion));
        }
        line
    }

    fn change(&mut self, cx: &mut Cx, f: impl FnOnce(&mut XrStreamSettings)) {
        let Some(s) = self.settings.as_mut() else { return };
        f(s);
        s.save(&self.app);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(server) = XrServer::get(cx) {
            s.apply(&self.auto, &server);
            self.streaming_seen = server.streaming_changes();
        }
        #[cfg(target_arch = "wasm32")]
        let _ = cx;
    }

    fn settings_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(on) = self.content(ids!(streaming)).as_check_box().changed(actions) {
            self.change(cx, |s| s.streaming = on);
        }
        if let Some(on) = self.content(ids!(room_scan)).as_check_box().changed(actions) {
            self.change(cx, |s| s.room_scan = on);
        }
        if let Some(on) = self.content(ids!(mirror)).as_check_box().changed(actions) {
            self.change(cx, |s| s.mirror = on);
        }
        if let Some(k) = self.content(ids!(bitrate)).as_drop_down().changed(actions) {
            let kbps = k.checked_sub(1).and_then(|k| STREAM_BITRATES_KBPS.get(k).copied());
            self.change(cx, |s| s.kbps = kbps);
        }
        if let Some(k) = self.content(ids!(fps)).as_drop_down().changed(actions) {
            if let Some(hz) = STREAM_FPS.get(k).copied() {
                self.change(cx, |s| s.hz = hz);
            }
        }
        if let Some(k) = self.content(ids!(aa)).as_drop_down().changed(actions) {
            let aa = k.checked_sub(1).and_then(|k| STREAM_AA_MODES.get(k).copied());
            self.change(cx, |s| s.aa = aa);
        }
        if let Some(k) = self.content(ids!(render_scale)).as_drop_down().changed(actions) {
            let pct = k.checked_sub(1).and_then(|k| STREAM_RENDER_SCALES.get(k).copied());
            self.change(cx, |s| s.render_scale_pct = pct);
        }
        if let Some(k) = self.content(ids!(view)).as_drop_down().changed(actions) {
            if let Some(view) = STREAM_VIEWS.get(k).copied() {
                self.change(cx, |s| s.view = view);
            }
        }
        // the PIN: any edit unpairs the paired headset; five digits look for the headset
        // showing them
        if let Some(text) = self.content(ids!(pin)).as_text_input().changed(actions) {
            let pin: String = text.chars().filter(|c| c.is_ascii_digit()).take(5).collect();
            if pin != text {
                self.content(ids!(pin)).as_text_input().set_text(cx, &pin);
            }
            // any change unpairs at once (the headset is told, the link drops, both forget);
            // five digits then look for the headset showing them
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(server) = XrServer::get(cx) {
                server.set_pin(&pin);
            }
        }
    }

    /// Once a second: the icon's state, the headset's own switch (its menu button), the numbers
    /// while open.
    fn tick(&mut self, cx: &mut Cx) {
        let state = link_state(cx);
        if let Some(mut icon) = self.popover.child_by_path(ids!(icon)).borrow_mut::<XrHeadsetIcon>() {
            icon.set_state(cx, state);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(server) = XrServer::get(cx) {
            let changes = server.streaming_changes();
            if changes != self.streaming_seen {
                // the headset's menu button turned the stream on or off: kept like a click
                let on = server.streaming();
                self.change(cx, |s| s.streaming = on);
                self.show_settings(cx);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(server) = XrServer::get(cx) {
            if let (Some(signals), Some(s)) = (self.auto.signals(&server), self.settings.as_ref()) {
                let s = s.clone();
                if self.auto.step(&signals, |l| s.auto(l), std::time::Instant::now()) {
                    makepad_widgets::log!("xr auto: {}", self.auto.note);
                    s.apply(&self.auto, &server);
                }
            }
        }
        let popover = self.popover.as_popover();
        if popover.is_open() {
            let line = self.headline(cx);
            self.content(ids!(headline)).as_label().set_text(cx, &line);
            self.content(ids!(auto_note)).as_label().set_text(cx, &self.auto_line());
            let line = self.pin_state(cx);
            self.content(ids!(pin_state)).as_label().set_text(cx, &line);
            self.content(ids!(stats)).redraw(cx);
        }
    }
}

impl Widget for XrCaptionButton {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.start(cx.cx);
        if self.settings.is_none() {
            // no XR served here: the caption shows nothing of it
            return DrawStep::done();
        }
        cx.widget_tree_insert_child(self.uid, live_id!(popover), self.popover.clone());
        self.popover.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.start(cx);
        if self.settings.is_none() {
            return;
        }
        if self.timer.is_event(event).is_some() {
            self.tick(cx);
        }
        let actions = cx.capture_actions(|cx| self.popover.handle_event(cx, event, scope));
        if self.popover.as_popover().opened(&actions) {
            self.show_settings(cx);
            let line = self.headline(cx);
            self.content(ids!(headline)).as_label().set_text(cx, &line);
            let line = self.pin_state(cx);
            self.content(ids!(pin_state)).as_label().set_text(cx, &line);
        }
        self.settings_actions(cx, &actions);
    }
}
