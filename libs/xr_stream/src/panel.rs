//! The XR stream's control in an app's caption: [`XrCaptionButton`], a headset icon (grey: off,
//! slate: paused from the headset, amber: waiting for a headset, green: streaming) opening a
//! compact panel. On top the stream switch (the real off: nothing listens, answers probes,
//! encodes or sends; `XrServer::set_enabled`), the state and the headset; below the quality
//! settings, the room scan and the desktop mirror; at the bottom the diagnostics, collapsed: key
//! numbers as tiles, the last minute as a sparkline, and under "Raw stats" every number the
//! stream has (the headset's and the server's lines, the bars, the last seconds frame by frame).
//!
//! An app adds it with one line in its caption (`xr := XrCaptionButton{app: "sandbox"}`), which
//! also installs the app's XR server (headsets find it; one linking gives the app its XR
//! display). The settings are kept per app (`app`) in its data folder (`home::app_data_dir`,
//! file `xr-stream`) and applied at start and on every change; the stream switch is on unless
//! the user turned it off, or the launch says off (`MAKEPAD_XR_STREAM=off`, `--xr-stream=off`).
//!
//! For automation and assistive technology the caption button is one switch node ("Stream to
//! headset", `ui act q='role=switch name="Stream to headset"' toggle`; `set_value v=off|on`;
//! its value is the state line), and the panel's own switch is the same control.

use crate::auto::{Lever, XrAuto};
use crate::settings_types::{StreamAa, StreamView, STREAM_AA_MODES};
#[cfg(not(target_arch = "wasm32"))]
use crate::XrServer;
use makepad_widgets::*;

/// The panel's own shaders (their own script module: the draw prelude).
pub(crate) mod shader {
    use makepad_widgets::*;

    script_mod! {
        use mod.pod.*
        use mod.math.*
        use mod.shader.*
        use mod.draw

        // The state dot: a filled circle and, while streaming, a soft ring growing out of it.
        mod.draw.DrawXrDot = mod.std.set_type_default() do #(DrawXrDot::script_shader(vm)){
            ..mod.draw.DrawQuad
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let r = 4.0
                let d = length(p - c)
                let core = 1.0 - smoothstep(r - 0.75, r + 0.75, d)
                let ring_r = r + self.pulse * 5.0
                let ring = (1.0 - smoothstep(0.6, 1.6, abs(d - ring_r))) * (1.0 - self.pulse) * self.pulsing * 0.55
                let a = max(core, ring) * self.color.w
                return vec4(self.color.xyz * a, a)
            }
        }

        // One segment of a sparkline: an anti-aliased line from seg_a to seg_b (the quad's
        // pixels), round-capped so consecutive segments join smoothly.
        mod.draw.DrawXrSpark = mod.std.set_type_default() do #(DrawXrSpark::script_shader(vm)){
            ..mod.draw.DrawQuad
            pixel: fn() {
                let p = self.pos * self.rect_size
                let pa = p - self.seg_a
                let ba = self.seg_b - self.seg_a
                let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
                let d = length(pa - ba * h)
                let a = (1.0 - smoothstep(self.thickness * 0.5 - 0.6, self.thickness * 0.5 + 0.6, d)) * self.color.w
                return vec4(self.color.xyz * a, a)
            }
        }

        // The disclosure's chevron: pointing right (closed) or down (open), two anti-aliased strokes.
        mod.draw.DrawXrChevron = mod.std.set_type_default() do #(DrawXrChevron::script_shader(vm)){
            ..mod.draw.DrawQuad
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let s = 3.0
                // the two strokes: from the tip back to either end (right, or down when open)
                let tip = mix(vec2(c.x + s * 0.5, c.y), vec2(c.x, c.y + s * 0.5), self.open)
                let e1 = mix(vec2(c.x - s * 0.5, c.y - s), vec2(c.x - s, c.y - s * 0.5), self.open)
                let e2 = mix(vec2(c.x - s * 0.5, c.y + s), vec2(c.x + s, c.y - s * 0.5), self.open)
                let a1 = p - tip
                let b1 = e1 - tip
                let d1 = length(a1 - b1 * clamp(dot(a1, b1) / dot(b1, b1), 0.0, 1.0))
                let b2 = e2 - tip
                let d2 = length(a1 - b2 * clamp(dot(a1, b2) / dot(b2, b2), 0.0, 1.0))
                let a = (1.0 - smoothstep(0.1, 1.3, min(d1, d2))) * self.color.w
                return vec4(self.color.xyz * a, a)
            }
        }
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct DrawXrDot {
        #[deref]
        pub draw_super: DrawQuad,
        #[live]
        pub color: Vec4f,
        #[live]
        pub pulse: f32,
        #[live]
        pub pulsing: f32,
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct DrawXrSpark {
        #[deref]
        pub draw_super: DrawQuad,
        #[live]
        pub seg_a: Vec2f,
        #[live]
        pub seg_b: Vec2f,
        #[live]
        pub color: Vec4f,
        #[live(1.5)]
        pub thickness: f32,
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct DrawXrChevron {
        #[deref]
        pub draw_super: DrawQuad,
        #[live]
        pub color: Vec4f,
        #[live]
        pub open: f32,
    }
}

use shader::{DrawXrChevron, DrawXrDot, DrawXrSpark};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // the state colours: one accent (live), the rest calm
    let xr_live = #x4CC38A
    let xr_waiting = #xE0A548
    let xr_paused = #x8EA0B8

    mod.widgets.XrHeadsetIconBase = #(XrHeadsetIcon::register_widget(vm))
    mod.widgets.XrHeadsetIcon = set_type_default() do mod.widgets.XrHeadsetIconBase {
        width: 26 height: 22
        icon +: { svg: crate_resource("self:resources/icons/headset.svg") }
        color_off: theme.color_text_meta
        color_paused: xr_paused
        color_waiting: xr_waiting
        color_live: xr_live
    }

    mod.widgets.XrStateDotBase = #(XrStateDot::register_widget(vm))
    mod.widgets.XrStateDot = set_type_default() do mod.widgets.XrStateDotBase {
        width: 20 height: 20
        color_off: theme.color_text_meta
        color_paused: xr_paused
        color_waiting: xr_waiting
        color_live: xr_live
    }

    mod.widgets.XrSparklineBase = #(XrSparkline::register_widget(vm))
    mod.widgets.XrSparkline = set_type_default() do mod.widgets.XrSparklineBase {
        width: Fill height: 40
        color_fps: xr_live
        color_m2p: xr_waiting
        draw_base +: { color: theme.color_outline_variant }
    }

    mod.widgets.XrDisclosureBase = #(XrDisclosure::register_widget(vm))
    mod.widgets.XrDisclosure = set_type_default() do mod.widgets.XrDisclosureBase {
        width: Fill height: 28
        color: theme.color_text_meta
        color_hover: theme.color_text
        draw_text +: { text_style: theme.font_bold{font_size: 8.5} }
    }

    mod.widgets.XrStatsViewBase = #(XrStatsView::register_widget(vm))
    mod.widgets.XrStatsView = set_type_default() do mod.widgets.XrStatsViewBase {
        width: Fill height: 0
        draw_text +: { color: theme.color_text_meta text_style: theme.font_code{font_size: 7} }
        draw_bar +: {}
    }

    // the panel's parts
    let XrSection = Label{
        padding: 0 margin: Inset{top: 10 bottom: 6}
        text: ""
        draw_text +: { color: theme.color_text_meta text_style: theme.font_bold{font_size: 8} }
    }
    let XrDivider = mod.widgets.Divider{ padding: Inset{top: 10 bottom: 0} }
    let XrFieldLabel = Label{
        width: 78 padding: 0
        draw_text +: { color: theme.color_text_meta text_style: theme.font_regular{font_size: 9} }
    }
    let XrField = View{
        width: 176 height: Fit flow: Right spacing: 0 align: Align{y: 0.5}
    }
    let XrPick = DropDown{ width: Fill }
    let XrSwitchRow = View{
        width: 176 height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
    }
    let XrSwitchText = View{ width: Fill height: Fit flow: Down spacing: 3 }
    let XrSwitchTitle = Label{
        padding: 0
        draw_text +: { color: theme.color_text text_style: theme.font_regular{font_size: 10} }
    }
    let XrSwitchHelp = Label{
        width: Fill padding: 0
        draw_text +: { color: theme.color_text_meta text_style: theme.font_regular{font_size: 8} }
    }
    // the switch alone (no label of its own: the row says what it is); the box holds the whole pill
    let XrSwitch = Toggle{
        text: "" width: 28 height: 18 padding: 0 label_walk +: { margin: 0 }
        draw_bg +: { color_active: xr_live }
    }
    let XrTile = RoundedView{
        width: Fill height: Fit flow: Down spacing: 1 padding: Inset{left: 8 right: 6 top: 7 bottom: 6}
        show_bg: true
        draw_bg +: { color: theme.color_inset border_radius: 6.0 border_size: 0.0 }
        value := Label{ padding: 0 text: "-" draw_text +: { color: theme.color_text text_style: theme.font_bold{font_size: 12} } }
        caption := Label{ padding: 0 text: "" draw_text +: { color: theme.color_text_meta text_style: theme.font_regular{font_size: 7.5} } }
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
                width: 392 height: Fit flow: Down padding: Inset{left: 16 right: 16 top: 12 bottom: 10}
                header := View{
                    width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                    dot := mod.widgets.XrStateDot{}
                    View{
                        width: Fill height: Fit flow: Down spacing: 4
                        title := Label{ padding: 0 text: "Stream to headset" draw_text +: { color: theme.color_text text_style: theme.font_bold{font_size: 11} } }
                        status := Label{ width: Fill padding: 0 text: "" draw_text +: { color: theme.color_text_meta text_style: theme.font_regular{font_size: 9} } }
                    }
                    streaming := Toggle{
                        text: "" width: 36 height: 24 padding: 0 label_walk +: { margin: 0 }
                        draw_bg +: { size: 22.0 color_active: xr_live }
                        // the caption button is this switch for assistive technology and automation (one node)
                        a11y: { hidden: true }
                    }
                }
                headset := View{
                    width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} margin: Inset{top: 8 left: 28}
                    chip := RoundedView{
                        width: Fit height: 24 padding: Inset{left: 10 right: 10} align: Align{y: 0.5}
                        show_bg: true draw_bg +: { color: theme.color_inset border_radius: 12.0 border_size: 0.0 }
                        chip_text := Label{ padding: 0 text: "No headset" draw_text +: { color: theme.color_text text_style: theme.font_regular{font_size: 9} } }
                    }
                    // the PIN, beside the headset: a click edits it (a headset showing the PIN typed pairs)
                    pin_tip := mod.widgets.PopoverHover{
                        width: Fit height: Fit placement: mod.widgets.BottomStart
                        pin_chip := ButtonFlat{
                            height: 24 padding: Inset{left: 10 right: 10}
                            text: "Set PIN"
                            a11y: { label: "Headset PIN" }
                            draw_bg +: { color: theme.color_inset border_radius: 12.0 border_size: 0.0 }
                            draw_text +: { text_style: theme.font_regular{font_size: 9} }
                        }
                        content := View{
                            width: Fit height: Fit padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
                            Label{ padding: 0 text: "Click to re-pair: type the PIN the headset shows" draw_text +: { color: theme.color_text text_style: theme.font_regular{font_size: 8.5} } }
                        }
                    }
                    pin := TextInput{ visible: false width: 64 height: 24 empty_text: "PIN" a11y: { label: "Headset PIN entry" } }
                    resume := ButtonFlat{
                        visible: false height: 24 padding: Inset{left: 10 right: 10}
                        text: "Resume"
                        draw_bg +: { border_radius: 12.0 }
                        draw_text +: { text_style: theme.font_bold{font_size: 9} }
                    }
                }
                body := ScrollYView{
                    width: Fill height: Fit
                    inner := View{
                        width: Fill height: Fit flow: Down
                        XrDivider{}
                        View{
                            width: Fill height: Fit flow: Right align: Align{y: 0.5}
                            XrSection{ text: "QUALITY" }
                            View{ width: Fill height: 1 }
                            auto_note := Label{ padding: 0 text: "" draw_text +: { color: theme.color_text_meta text_style: theme.font_regular{font_size: 8} } }
                        }
                        View{
                            width: Fill height: Fit flow: Right{wrap: true} spacing: 8
                            XrField{ XrFieldLabel{ text: "Bandwidth" } bitrate := XrPick{ labels: ["Auto", "40 Mbit/s", "60 Mbit/s", "80 Mbit/s", "100 Mbit/s", "120 Mbit/s", "150 Mbit/s"] } }
                            XrField{ XrFieldLabel{ text: "Frame rate" } fps := XrPick{ labels: ["72 fps", "80 fps", "90 fps", "120 fps"] } }
                            XrField{ XrFieldLabel{ text: "Anti-aliasing" } aa := XrPick{ labels: ["Auto", "Off", "FXAA", "MSAA 2x", "MSAA 4x", "MSAA 8x", "MSAA 4x+FXAA", "Super 1.25x", "Super 1.5x", "Super 2x"] } }
                            XrField{ XrFieldLabel{ text: "Render scale" } render_scale := XrPick{ labels: ["Auto", "100%", "90%", "80%", "70%"] } }
                            XrField{ XrFieldLabel{ text: "View" } view := XrPick{ labels: ["App decides", "Mixed reality", "VR"] } }
                        }
                        XrDivider{}
                        View{
                            width: Fill height: Fit flow: Right{wrap: true} spacing: 8 margin: Inset{top: 12}
                            XrSwitchRow{
                                XrSwitchText{ XrSwitchTitle{ text: "Room scan" } XrSwitchHelp{ text: "When the app uses it" } }
                                room_scan := XrSwitch{ a11y: { role: "switch" label: "Room scan" } }
                            }
                            XrSwitchRow{
                                XrSwitchText{ XrSwitchTitle{ text: "Desktop mirror" } XrSwitchHelp{ text: "The headset's view, live" } }
                                mirror := XrSwitch{ a11y: { role: "switch" label: "Desktop mirror" } }
                            }
                        }
                        XrDivider{}
                        diag_toggle := mod.widgets.XrDisclosure{ text: "DIAGNOSTICS" margin: Inset{top: 6} }
                        diagnostics := View{
                            visible: false
                            width: Fill height: Fit flow: Down spacing: 8
                            tiles := View{
                                width: Fill height: Fit flow: Right spacing: 6
                                tile_fps := XrTile{ caption +: { text: "fps shown" } }
                                tile_m2p := XrTile{ caption +: { text: "latency ms" } }
                                tile_rate := XrTile{ caption +: { text: "Mbit/s" } }
                                tile_encode := XrTile{ caption +: { text: "ms encode" } }
                                tile_late := XrTile{ caption +: { text: "late, lost" } }
                            }
                            spark := mod.widgets.XrSparkline{}
                            spark_legend := Label{ text: "last minute: frames shown (green), motion to photon (amber)" draw_text +: { color: theme.color_text_meta text_style: theme.font_regular{font_size: 7.5} } }
                            raw_toggle := mod.widgets.XrDisclosure{ text: "RAW STATS" }
                            raw := View{
                                visible: false
                                width: Fill height: Fit flow: Down spacing: 6
                                raw_text := Label{ width: Fill text: "" draw_text +: { color: theme.color_text_meta text_style: theme.font_code{font_size: 7} } }
                                stats := mod.widgets.XrStatsView{}
                            }
                        }
                    }
                }
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
/// The state dot's pulse steps a second.
const PULSE_HZ: f64 = 8.0;
/// The panel's widest (narrower when the window is).
const PANEL_WIDTH: f64 = 392.0;

/// The user's stream settings, kept per app. `None`: the setting is on Auto (`crate::auto`).
#[derive(Clone, Debug, PartialEq)]
pub struct XrStreamSettings {
    /// The stream switch (`XrServer::set_enabled`): off, the server does nothing at all.
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
                // (the switch's own key: an old `streaming=off` was the headset's pause, not an off)
                "stream" => s.streaming = v != "off",
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
        let text = format!("stream={}\nroom_scan={}\nkbps={}\nhz={}\naa={}\nrender_scale={}\nview={}\nmirror={}\n", on(self.streaming), on(self.room_scan), auto(self.kbps.map(|k| k.to_string())), self.hz, auto(self.aa.map(|a| a.id.to_string())), auto(self.render_scale_pct.map(|p| p.to_string())), self.view.label(), on(self.mirror));
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

    /// To the stream server (live): the manual values, Auto's for the rest, and the switch.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply(&self, auto: &XrAuto, server: &XrServer) {
        let a = auto.values;
        server.set_aa(self.aa.unwrap_or(a.aa));
        server.set_render_scale(self.render_scale_pct.unwrap_or(a.render_scale_pct));
        server.set_view(self.view);
        server.set_mirror(self.mirror);
        server.set_room_scan(self.room_scan);
        server.set_bitrate_kbps(self.kbps.unwrap_or(a.kbps));
        server.set_refresh_hz(self.hz);
        server.set_enabled(self.streaming);
    }
}

/// The stream's state as the icon, the dot and the switch node show it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum XrLinkState {
    /// This process serves no XR, or the stream is switched off: nothing runs.
    #[default]
    Off,
    /// The headset paused it (its menu button): linked, nothing streamed.
    Paused,
    /// The server listens; no headset is linked.
    Waiting,
    /// A headset is linked but sends no frames (taken off, asleep): nothing is held for it.
    Idle,
    /// A headset streams.
    Live,
}

fn link_state(cx: &mut Cx) -> XrLinkState {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(server) = XrServer::get(cx) else { return XrLinkState::Off };
        if !server.enabled() {
            return XrLinkState::Off;
        }
        if !server.streaming() {
            return XrLinkState::Paused;
        }
        if server.session_eye().is_some() {
            return if server.frames_flowing() { XrLinkState::Live } else { XrLinkState::Idle };
        }
        XrLinkState::Waiting
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = cx;
        XrLinkState::Off
    }
}

/// A value of the headset's last report.
#[cfg(not(target_arch = "wasm32"))]
fn report_value(report: &[f32], i: usize) -> f32 {
    report.get(i).copied().unwrap_or(0.0)
}

/// The caption's headset icon (the popover's anchor), coloured by the stream's state.
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
    color_paused: Vec4f,
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
            // off: the meta ink, dimmed further so it reads as "nothing runs"
            XrLinkState::Off => vec4(self.color_off.x, self.color_off.y, self.color_off.z, self.color_off.w * 0.6),
            XrLinkState::Paused | XrLinkState::Idle => self.color_paused,
            XrLinkState::Waiting => self.color_waiting,
            XrLinkState::Live => self.color_live,
        };
        self.icon.draw_abs(cx, Rect { pos: dvec2(r.pos.x + ((r.size.x - s) * 0.5).floor(), r.pos.y + ((r.size.y - s) * 0.5).floor()), size: dvec2(s, s) });
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// The panel's state dot, coloured like the icon; while streaming a soft ring pulses out of
/// it (`pulse`, driven by the panel while it is open).
#[derive(Script, ScriptHook, Widget)]
pub struct XrStateDot {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_dot: DrawXrDot,
    #[live]
    color_off: Vec4f,
    #[live]
    color_paused: Vec4f,
    #[live]
    color_waiting: Vec4f,
    #[live]
    color_live: Vec4f,
    #[rust]
    state: XrLinkState,
    #[rust]
    pulse: f32,
}

impl XrStateDot {
    fn set(&mut self, cx: &mut Cx, state: XrLinkState, pulse: f32) {
        if state != self.state || (state == XrLinkState::Live && pulse != self.pulse) {
            self.state = state;
            self.pulse = pulse;
            self.draw_dot.redraw(cx);
        }
    }
}

impl Widget for XrStateDot {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.walk_turtle(walk);
        self.draw_dot.color = match self.state {
            XrLinkState::Off => self.color_off,
            XrLinkState::Paused | XrLinkState::Idle => self.color_paused,
            XrLinkState::Waiting => self.color_waiting,
            XrLinkState::Live => self.color_live,
        };
        self.draw_dot.pulsing = if self.state == XrLinkState::Live { 1.0 } else { 0.0 };
        self.draw_dot.pulse = self.pulse;
        self.draw_dot.draw_abs(cx, r);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// The last minute in one small chart: the frames shown per second against the display rate
/// (green) and motion to photon against 150 ms (amber), a thin line each over a baseline.
#[derive(Script, ScriptHook, Widget)]
pub struct XrSparkline {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_line: DrawXrSpark,
    #[live]
    draw_base: DrawColor,
    #[live]
    color_fps: Vec4f,
    #[live]
    color_m2p: Vec4f,
    /// Per second: (shown / display rate, motion to photon / 150 ms), oldest first.
    #[rust]
    points: Vec<(f32, f32)>,
}

impl XrSparkline {
    fn set_points(&mut self, cx: &mut Cx, points: Vec<(f32, f32)>) {
        if points != self.points {
            self.points = points;
            self.draw_line.redraw(cx);
        }
    }

    fn series(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f, value: impl Fn(&(f32, f32)) -> f32) {
        let n = self.points.len();
        if n < 2 {
            return;
        }
        // the seconds there are, across the whole width
        let step = rect.size.x / (n - 1) as f64;
        let x0 = rect.pos.x;
        let at = |k: usize, v: f32| dvec2(x0 + k as f64 * step, rect.pos.y + rect.size.y * (1.0 - v.clamp(0.0, 1.1) as f64 / 1.1));
        self.draw_line.color = color;
        for k in 1..n {
            let (a, b) = (at(k - 1, value(&self.points[k - 1])), at(k, value(&self.points[k])));
            let pad = 2.0;
            let lo = dvec2(a.x.min(b.x) - pad, a.y.min(b.y) - pad);
            let hi = dvec2(a.x.max(b.x) + pad, a.y.max(b.y) + pad);
            self.draw_line.seg_a = vec2((a.x - lo.x) as f32, (a.y - lo.y) as f32);
            self.draw_line.seg_b = vec2((b.x - lo.x) as f32, (b.y - lo.y) as f32);
            self.draw_line.draw_abs(cx, Rect { pos: lo, size: hi - lo });
        }
    }
}

impl Widget for XrSparkline {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let plot = Rect { pos: rect.pos + dvec2(0.0, 3.0), size: rect.size - dvec2(0.0, 6.0) };
        self.draw_base.draw_abs(cx, Rect { pos: dvec2(plot.pos.x, plot.pos.y + plot.size.y), size: dvec2(plot.size.x, 1.0) });
        let (fps, m2p) = (self.color_fps, self.color_m2p);
        self.series(cx, plot, m2p, |p| p.1);
        self.series(cx, plot, fps, |p| p.0);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// A section that opens and closes: a chevron and a small-caps title, the whole row clickable,
/// brighter while hovered. A disclosure button for assistive technology.
#[derive(Script, ScriptHook, Widget)]
pub struct XrDisclosure {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_text: DrawText,
    #[live]
    draw_chevron: DrawXrChevron,
    #[live]
    text: ArcStringMut,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[rust]
    open: bool,
    #[rust]
    hover: bool,
    #[rust]
    #[area]
    area: Area,
}

#[derive(Clone, Debug, Default)]
pub enum XrDisclosureAction {
    Toggled(bool),
    #[default]
    None,
}

impl XrDisclosure {
    fn flip(&mut self, cx: &mut Cx) {
        self.open = !self.open;
        self.draw_text.redraw(cx);
        cx.widget_action(self.uid, XrDisclosureAction::Toggled(self.open));
    }
}

impl Widget for XrDisclosure {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let color = if self.hover { self.color_hover } else { self.color };
        self.draw_chevron.color = color;
        self.draw_chevron.open = if self.open { 1.0 } else { 0.0 };
        self.draw_chevron.draw_abs(cx, Rect { pos: dvec2(rect.pos.x - 2.0, rect.pos.y + (rect.size.y - 12.0) * 0.5), size: dvec2(12.0, 12.0) });
        self.draw_text.color = color;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x + 14.0, rect.pos.y + (rect.size.y - 11.0) * 0.5), self.text.as_ref());
        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.hover = true;
                self.draw_text.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = false;
                self.draw_text.redraw(cx);
            }
            Hit::FingerUp(f) if f.is_over && f.is_primary_hit() => self.flip(cx),
            _ => {}
        }
    }

    fn semantics(&self, _cx: &Cx, node: &mut a11y_tree::Node) {
        node.role = a11y_tree::Role::DisclosureButton;
        makepad_platform::a11y::set_string(&mut node.name, self.text.as_ref());
        node.actions = a11y_tree::Actions::PRESS | a11y_tree::Actions::EXPAND | a11y_tree::Actions::COLLAPSE;
        node.states.set(a11y_tree::States::EXPANDED, self.open);
        node.states.set(a11y_tree::States::COLLAPSED, !self.open);
    }

    fn handle_action(&mut self, cx: &mut Cx, action: &a11y_tree::ActionRequest, _scope: &mut Scope) -> a11y_tree::ActionResult {
        use a11y_tree::{ActionResult, Actions as A};
        let want = if action.action == A::PRESS {
            !self.open
        } else if action.action == A::EXPAND {
            true
        } else if action.action == A::COLLAPSE {
            false
        } else {
            return ActionResult::Unhandled;
        };
        if want != self.open {
            self.flip(cx);
        }
        ActionResult::Handled
    }
}

/// The raw numbers' graphs: the last minute of pictures shown per second (green) and motion to
/// photon (orange) as bars, and the last seconds frame by frame (one tick per display frame:
/// new dim, repeat yellow, skip red, odd bright red, late purple, blank white). Nothing without
/// a headset.
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

/// The graphs' height.
const STATS_GRAPHS_H: f64 = 190.0;

impl Widget for XrStatsView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        #[cfg(not(target_arch = "wasm32"))]
        let numbers = XrServer::get(cx.cx).is_some_and(|s| !s.headset_reports().is_empty());
        #[cfg(target_arch = "wasm32")]
        let numbers = false;
        let walk = Walk { height: Size::Fixed(if numbers { STATS_GRAPHS_H } else { 0.0 }), ..walk };
        let rect = cx.walk_turtle(walk);
        #[cfg(not(target_arch = "wasm32"))]
        if numbers {
            self.draw_graphs(cx, rect);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = rect;
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// Every number of the stream as text (the raw stats' lines).
#[cfg(not(target_arch = "wasm32"))]
fn raw_stats_text(server: &XrServer) -> String {
    use makepad_desk_link::xr::report as r;
    let reports = server.headset_reports();
    let mut lines = Vec::new();
    if let Some(last) = reports.last() {
        let v = |i: usize| report_value(last, i);
        lines.push(format!("headset {:.0} Hz · shown {:.0}/s · BLANK {:.1}/s · skipped {:.1}/s · received {:.0}/s · battery {}{}", v(r::DISPLAY), v(r::SHOWN), v(r::BLANK), v(r::SKIPPED), v(r::RECEIVED),
            if v(r::BATTERY_PCT) < 0.0 { "?".to_string() } else { format!("{:.0}%", v(r::BATTERY_PCT)) }, if v(r::BATTERY_CHARGING) > 0.5 { " charging" } else { "" }));
        lines.push(format!("repeats {:.1}/s: late {:.1} · server skipped {:.1} · queue cap {:.1} · no pose {:.1}", v(r::REPEATS), v(r::REPEAT_LATE), v(r::REPEAT_SERVER), v(r::REPEAT_DROPPED), v(r::REPEAT_NOPOSE)));
        lines.push(format!("motion to photon {:.0} ms (net {:.1} decode {:.1} queue {:.1}) · lead {:.0} ms ({:.0} frames)", v(r::M2P_MS), v(r::NET_MS), v(r::DECODE_MS), v(r::WAIT_MS), v(r::LEAD_MS), v(r::AHEAD)));
        lines.push(format!("uplink {:.0} poses/s ±{:.2} ms · env depth age {:.0} ms · headset GPU {:.1} of {:.1} ms, eye buffers {:.0}%", v(r::UPLINK), v(r::UPLINK_SD_MS), v(r::ENV_AGE_MS), v(r::CLIENT_GPU_MS), 1000.0 / v(r::DISPLAY).max(1.0), v(r::EYE_SCALE_PCT)));
        lines.push(format!("{} · AA {}", server.server_line(), server.aa().name));
        lines.push(if v(r::AUDIO_RECEIVED) > 0.0 {
            format!("audio {:.0} ms (net {:.1} buffer {:.0}/{:.0} device {:.0}; pictures {:.0}) · lost {:.0} · underruns {:.0} · drift {:+.0} ppm",
                v(r::AUDIO_MS), v(r::AUDIO_NET_MS), v(r::AUDIO_BUFFER_MS), v(r::AUDIO_TARGET_MS), v(r::AUDIO_DEVICE_MS), v(r::AUDIO_VIDEO_MS), v(r::AUDIO_LOST) + v(r::AUDIO_LATE), v(r::AUDIO_UNDERRUNS), v(r::AUDIO_DRIFT_PPM))
        } else {
            "audio: the headset hears none".to_string()
        });
    } else {
        lines.push(match server.port() {
            Some(port) if server.enabled() => format!("no headset linked · listening on port {port} · AA {}", server.aa().name),
            _ if server.enabled() => "no headset linked · binding a port".to_string(),
            _ => "switched off: no port, no pairing probe answered, nothing drawn, encoded or sent".to_string(),
        });
        let line = server.server_line();
        if !line.is_empty() {
            lines.push(format!("last session: {line}"));
        }
    }
    let app = server.app_line();
    if !app.is_empty() {
        lines.push(app);
    }
    lines.join("\n")
}

#[cfg(not(target_arch = "wasm32"))]
impl XrStatsView {
    fn draw_graphs(&mut self, cx: &mut Cx2d, rect: Rect) {
        use makepad_desk_link::xr::report as r;
        let Some(server) = XrServer::get(cx.cx) else { return };
        let reports = server.headset_reports();
        let Some(last) = reports.last() else { return };
        let v = |i: usize| report_value(last, i);
        // two graphs of the last minute, a bar per second, each on its own labelled scale:
        // frames shown per second against the display rate (repeats yellow, blank white above
        // the bar), and motion to photon against 100 ms
        let full = v(r::DISPLAY).max(1.0) as f64;
        let width = rect.size.x - 28.0;
        let bw = width / 60.0;
        let x0 = rect.pos.x + 28.0;
        let gh = 32.0;
        let mut y = rect.pos.y;
        let line_at = |me: &mut Self, cx: &mut Cx2d, y: f64, color: Vec4f| {
            me.draw_bar.color = color;
            me.draw_bar.draw_abs(cx, Rect { pos: dvec2(x0, y), size: dvec2(width, 1.0) });
        };
        // (1) frames shown /s
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y), &format!("Frames shown /s (line: {:.0} Hz; yellow repeats, white blank)", full));
        y += 13.0;
        let base = y + gh;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y - 2.0), &format!("{:.0}", full));
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, base - 10.0), "0");
        let scale = gh / (full * 1.1);
        line_at(self, cx, base - full * scale, vec4(0.6, 0.6, 0.6, 0.5));
        line_at(self, cx, base, vec4(0.35, 0.35, 0.35, 0.5));
        for (k, rep) in reports.iter().enumerate() {
            let x = x0 + k as f64 * bw;
            let get = |i: usize| report_value(rep, i) as f64;
            let shown = (get(r::SHOWN) * scale).clamp(0.0, gh);
            self.draw_bar.color = vec4(0.3, 0.76, 0.54, 0.9);
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
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y), "Motion to photon, ms (line: 100 ms)");
        y += 13.0;
        let base = y + gh;
        let top_ms = 150.0;
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y - 2.0), "150");
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, base - 10.0), "0");
        line_at(self, cx, base - 100.0 / top_ms * gh, vec4(0.6, 0.6, 0.6, 0.5));
        line_at(self, cx, base, vec4(0.35, 0.35, 0.35, 0.5));
        for (k, rep) in reports.iter().enumerate() {
            let x = x0 + k as f64 * bw;
            let m2p = (report_value(rep, r::M2P_MS) as f64 / top_ms).clamp(0.0, 1.0) * gh;
            self.draw_bar.color = vec4(0.88, 0.65, 0.28, 0.9);
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
        y += 13.0;
        // the legend: a swatch and a word per code
        let mut lx = rect.pos.x;
        for (_, name, color, _) in colors {
            self.draw_bar.color = color;
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(lx, y + 2.0), size: dvec2(7.0, 7.0) });
            self.draw_text.draw_abs(cx, dvec2(lx + 10.0, y), name);
            lx += 14.0 + name.len() as f64 * 5.0 + 8.0;
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
        let (strip_y, strip_h) = (y, 16.0);
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
    /// The once-a-second tick: only while the stream is on or the panel is open (off and
    /// closed, nothing of the stream wakes the app).
    #[rust]
    timer: Timer,
    /// The state dot's pulse while streaming and open: a few steps a second on its own timer (the
    /// desktop never renders at the headset's rate for it).
    #[rust]
    pulse_timer: Timer,
    /// The PIN's field shows (in place of its chip).
    #[rust]
    editing_pin: bool,
    #[rust]
    state: XrLinkState,
    /// The state line (the status label and the switch node's value).
    #[rust]
    status: String,
    /// The pause's change count last seen (the headset's menu button flips it).
    #[rust]
    streaming_seen: u32,
}

impl XrCaptionButton {
    /// Once, at the first event or draw: in a process that serves XR, the kept settings (the
    /// launch's switch over the kept one) to the server and the tick.
    fn start(&mut self, cx: &mut Cx) {
        if self.checked {
            return;
        }
        self.checked = true;
        // the app's XR server (none on the web): headsets find it from now on, unless off
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(server) = XrServer::install(cx, &self.app) else { return };
            let mut s = XrStreamSettings::load(&self.app);
            if let Some(on) = crate::server::launch_enabled() {
                s.streaming = on;
            }
            s.apply(&self.auto, &server);
            self.streaming_seen = server.streaming_changes();
            self.settings = Some(s);
            self.refresh(cx);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = cx;
    }

    fn content(&self, path: &[LiveId]) -> WidgetRef {
        self.popover.as_popover().content().child_by_path(path)
    }

    fn open(&self) -> bool {
        self.popover.as_popover().is_open()
    }

    /// The tick runs while it has something to follow.
    fn arm_timer(&mut self, cx: &mut Cx) {
        let want = self.settings.as_ref().is_some_and(|s| s.streaming) || self.open();
        if want && self.timer.is_empty() {
            self.timer = cx.start_interval(1.0);
        } else if !want && !self.timer.is_empty() {
            cx.stop_timer(self.timer);
            self.timer = Timer::empty();
        }
    }

    /// The controls to the settings (when the panel opens and after a change from elsewhere).
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

    /// The state line: off, paused, waiting, or streaming with its rate and latency.
    fn status_line(&self, cx: &mut Cx) -> String {
        match self.state {
            XrLinkState::Off => "Off · nothing listens, encodes or sends".to_string(),
            XrLinkState::Paused => "Paused from the headset · linked, nothing streamed".to_string(),
            XrLinkState::Waiting => "Waiting for a headset · open XR Remote on it".to_string(),
            XrLinkState::Idle => "Linked · the headset is idle (not worn or asleep)".to_string(),
            XrLinkState::Live => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(last) = XrServer::get(cx).and_then(|s| s.headset_reports().pop()) {
                    use makepad_desk_link::xr::report as r;
                    return format!("Streaming · {:.0} fps · {:.0} ms", report_value(&last, r::SHOWN), report_value(&last, r::M2P_MS));
                }
                let _ = cx;
                "Streaming".to_string()
            }
        }
    }

    /// The headset chip: the headset (paired, linked, or the one the PIN found), its battery and
    /// link, in a few words.
    fn headset_chip(&self, cx: &mut Cx) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use makepad_desk_link::xr::report as r;
            let Some(server) = XrServer::get(cx) else { return String::new() };
            let Some(p) = server.pairing() else { return String::new() };
            let live = server.live_headset();
            if p.clash {
                return "Two headsets show this PIN".to_string();
            }
            let name = live.as_ref().map(|l| l.name.clone()).or_else(|| p.paired.as_ref().map(|h| h.name.clone()));
            let Some(name) = name else {
                return match &p.matched {
                    Some(h) => format!("{} · linking", h.name),
                    None => "No headset".to_string(),
                };
            };
            let mut parts = vec![name];
            if let Some(last) = server.headset_reports().pop().filter(|_| live.is_some()) {
                let pct = report_value(&last, r::BATTERY_PCT);
                if pct >= 0.0 {
                    parts.push(format!("{pct:.0}%{}", if report_value(&last, r::BATTERY_CHARGING) > 0.5 { " charging" } else { "" }));
                }
            }
            parts.push(if live.is_some() { "linked".to_string() } else { "not linked".to_string() });
            parts.join(" · ")
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = cx;
            String::new()
        }
    }

    /// The PIN chip: the PIN typed here (the paired headset's), or none yet.
    fn pin_chip(&self, cx: &mut Cx) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(p) = XrServer::get(cx).and_then(|s| s.pairing()).filter(|p| !p.pin.is_empty()) {
            return format!("PIN {}", p.pin);
        }
        let _ = cx;
        "Set PIN".to_string()
    }

    /// The PIN chip or its field (editing: a click on the chip; back on Return, Escape, focus
    /// leaving or five digits).
    fn edit_pin(&mut self, cx: &mut Cx, on: bool) {
        self.editing_pin = on;
        self.content(ids!(pin_tip)).set_visible(cx, !on);
        let field = self.content(ids!(pin));
        field.set_visible(cx, on);
        if on {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(p) = XrServer::get(cx).and_then(|s| s.pairing()) {
                field.as_text_input().set_text(cx, &p.pin);
            }
            field.set_key_focus(cx);
        }
        self.fill(cx);
        self.popover.redraw(cx);
    }

    /// What Auto holds and did last, and what it suggests.
    fn auto_line(&self) -> String {
        let Some(s) = &self.settings else { return String::new() };
        let a = self.auto.values;
        let mut on = Vec::new();
        if s.kbps.is_none() {
            on.push(format!("{} Mbit/s", a.kbps / 1000));
        }
        if s.aa.is_none() {
            on.push(a.aa.name.to_string());
        }
        if s.render_scale_pct.is_none() {
            on.push(format!("{}%", a.render_scale_pct));
        }
        if on.is_empty() {
            return "Auto off".to_string();
        }
        let mut line = format!("Auto {}", on.join(" · "));
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
        self.refresh(cx);
    }

    /// The stream switch (the panel's, the switch node's): kept, applied, shown.
    pub fn set_streaming(&mut self, cx: &mut Cx, on: bool) {
        if self.settings.as_ref().is_some_and(|s| s.streaming == on) {
            return;
        }
        makepad_widgets::log!("xr panel: stream to headset {}", if on { "on" } else { "off" });
        self.change(cx, |s| s.streaming = on);
        self.content(ids!(streaming)).as_check_box().set_active(cx, on, Animate::Yes);
    }

    pub fn streaming(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| s.streaming)
    }

    fn settings_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(on) = self.content(ids!(streaming)).as_check_box().changed(actions) {
            self.set_streaming(cx, on);
        }
        if self.content(ids!(resume)).as_button().clicked(actions) {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(server) = XrServer::get(cx) {
                server.set_streaming(true);
            }
            self.refresh(cx);
        }
        if self.content(ids!(pin_chip)).as_button().clicked(actions) {
            self.content(ids!(pin_tip)).as_popover().close(cx);
            self.edit_pin(cx, true);
        }
        let field = self.content(ids!(pin)).as_text_input();
        if self.editing_pin && (field.returned(actions).is_some() || field.escaped(actions) || field.key_focus_lost(actions)) {
            self.edit_pin(cx, false);
        }
        for (toggle, section) in [(ids!(diag_toggle), ids!(diagnostics)), (ids!(raw_toggle), ids!(raw))] {
            let id = self.content(toggle).widget_uid();
            if let Some(XrDisclosureAction::Toggled(open)) = actions.find_widget_action(id).map(|a| a.cast()) {
                self.content(section).set_visible(cx, open);
                self.fill(cx);
                self.popover.redraw(cx);
            }
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
        // the PIN: any edit unpairs the paired headset (the headset is told, the link drops,
        // both forget); five digits then look for the headset showing them
        if let Some(text) = self.content(ids!(pin)).as_text_input().changed(actions) {
            let pin: String = text.chars().filter(|c| c.is_ascii_digit()).take(5).collect();
            if pin != text {
                self.content(ids!(pin)).as_text_input().set_text(cx, &pin);
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(server) = XrServer::get(cx) {
                server.set_pin(&pin);
            }
            if pin.len() == 5 {
                self.edit_pin(cx, false);
            } else {
                self.fill(cx);
            }
        }
    }

    /// The state now: the icon, the dot and the state line (and the tick armed for it).
    fn refresh(&mut self, cx: &mut Cx) {
        let state = link_state(cx);
        self.state = state;
        if let Some(mut icon) = self.popover.child_by_path(ids!(icon)).borrow_mut::<XrHeadsetIcon>() {
            icon.set_state(cx, state);
        }
        self.status = self.status_line(cx);
        self.arm_timer(cx);
        if self.open() {
            self.fill(cx);
        }
    }

    /// The open panel's texts and numbers.
    fn fill(&mut self, cx: &mut Cx) {
        if let Some(mut dot) = self.content(ids!(dot)).borrow_mut::<XrStateDot>() {
            let pulse = dot.pulse;
            dot.set(cx, self.state, pulse);
        }
        self.content(ids!(status)).as_label().set_text(cx, &self.status);
        let chip = self.headset_chip(cx);
        self.content(ids!(chip_text)).as_label().set_text(cx, &chip);
        let pin = self.pin_chip(cx);
        self.content(ids!(pin_chip)).as_button().set_text(cx, &pin);
        self.content(ids!(headset)).set_visible(cx, self.state != XrLinkState::Off);
        self.content(ids!(resume)).set_visible(cx, self.state == XrLinkState::Paused);
        let auto = self.auto_line();
        self.content(ids!(auto_note)).as_label().set_text(cx, auto.lines().next().unwrap_or(""));
        if self.content(ids!(diagnostics)).visible() {
            self.fill_diagnostics(cx);
        }
    }

    /// The tiles, the sparkline and (open) the raw stats.
    fn fill_diagnostics(&mut self, cx: &mut Cx) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use makepad_desk_link::xr::report as r;
            let Some(server) = XrServer::get(cx) else { return };
            let reports = server.headset_reports();
            let stats = server.stats();
            let live = server.session_eye().is_some();
            let tiles: [(&[LiveId], String); 5] = match reports.last().filter(|_| live) {
                Some(last) => {
                    let v = |i: usize| report_value(last, i);
                    let display = v(r::DISPLAY).max(1.0);
                    let late = (v(r::REPEATS) + v(r::SKIPPED) + v(r::BLANK)) / display * 100.0;
                    [
                        (ids!(tile_fps), format!("{:.0}/{:.0}", v(r::SHOWN), display)),
                        (ids!(tile_m2p), format!("{:.0}", v(r::M2P_MS))),
                        (ids!(tile_rate), if stats.window > 0 { format!("{:.0}", stats.sent_mbps) } else { "-".to_string() }),
                        (ids!(tile_encode), if stats.window > 0 { format!("{:.1}", stats.encode_ms) } else { "-".to_string() }),
                        (ids!(tile_late), format!("{late:.1}%")),
                    ]
                }
                None => [(ids!(tile_fps), "-".into()), (ids!(tile_m2p), "-".into()), (ids!(tile_rate), "-".into()), (ids!(tile_encode), "-".into()), (ids!(tile_late), "-".into())],
            };
            for (tile, value) in tiles {
                self.content(tile).child_by_path(ids!(value)).as_label().set_text(cx, &value);
            }
            let points = reports.iter().map(|rep| (report_value(rep, r::SHOWN) / report_value(rep, r::DISPLAY).max(1.0), report_value(rep, r::M2P_MS) / 150.0)).collect();
            if let Some(mut spark) = self.content(ids!(spark)).borrow_mut::<XrSparkline>() {
                spark.set_points(cx, points);
            }
            if self.content(ids!(raw)).visible() {
                let text = format!("{}\n{}", raw_stats_text(&server), self.auto_line());
                self.content(ids!(raw_text)).as_label().set_text(cx, &text);
                self.content(ids!(stats)).redraw(cx);
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = cx;
    }

    /// Once a second: the state, the headset's pause, Auto, the numbers while open.
    fn tick(&mut self, cx: &mut Cx) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(server) = XrServer::get(cx) {
            // the headset's menu button paused or resumed the stream: shown, not kept (the
            // switch stays the user's)
            let changes = server.streaming_changes();
            if changes != self.streaming_seen {
                self.streaming_seen = changes;
                makepad_widgets::log!("xr panel: the headset {} the stream", if server.streaming() { "resumed" } else { "paused" });
            }
            if let (Some(signals), Some(s)) = (self.auto.signals(&server), self.settings.as_ref()) {
                let s = s.clone();
                if self.auto.step(&signals, |l| s.auto(l), std::time::Instant::now()) {
                    makepad_widgets::log!("xr auto: {}", self.auto.note);
                    s.apply(&self.auto, &server);
                }
            }
        }
        self.refresh(cx);
    }

    /// The panel's size: never wider than the window, and its body (all but the header)
    /// scrolls once taller than the window allows (70% with the diagnostics open).
    fn fit_panel(&mut self, cx: &mut Cx2d) {
        let pass = cx.current_pass_size();
        let width = PANEL_WIDTH.min((pass.x - 24.0).max(240.0));
        let content = self.popover.as_popover().content().as_view();
        if content.borrow().is_some_and(|v| v.walk.width != Size::Fixed(width)) {
            content.set_walk(cx, Walk { width: Size::Fixed(width), height: Size::fit(), ..Walk::default() });
        }
        let header = self.content(ids!(header)).area().rect(cx).size.y + 60.0;
        let diag = self.content(ids!(diagnostics)).visible();
        let cap = if diag { pass.y * 0.7 } else { pass.y - 24.0 } - header;
        let inner = self.content(ids!(inner)).area().rect(cx).size.y;
        let height = if inner > cap && cap > 80.0 { Size::Fixed(cap) } else { Size::fit() };
        let body = self.content(ids!(body)).as_view();
        if body.borrow().is_some_and(|b| b.walk.height != height) {
            body.set_walk(cx, Walk { width: Size::fill(), height, ..Walk::default() });
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
        if self.open() {
            self.fit_panel(cx);
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
        if self.pulse_timer.is_event(event).is_some() {
            if self.open() && self.state == XrLinkState::Live {
                // a soft pulse every 1.6 s
                let pulse = ((cx.seconds_since_app_start() / 1.6).fract()) as f32;
                if let Some(mut dot) = self.content(ids!(dot)).borrow_mut::<XrStateDot>() {
                    dot.set(cx, XrLinkState::Live, pulse);
                }
            } else {
                cx.stop_timer(self.pulse_timer);
                self.pulse_timer = Timer::empty();
            }
        }
        let actions = cx.capture_actions(|cx| self.popover.handle_event(cx, event, scope));
        if self.popover.as_popover().opened(&actions) {
            self.show_settings(cx);
            self.refresh(cx);
        }
        if self.popover.as_popover().closed(&actions) {
            self.arm_timer(cx);
        }
        self.settings_actions(cx, &actions);
        if self.open() && self.state == XrLinkState::Live && self.pulse_timer.is_empty() {
            self.pulse_timer = cx.start_interval(1.0 / PULSE_HZ);
        }
    }

    fn semantics(&self, _cx: &Cx, node: &mut a11y_tree::Node) {
        node.role = a11y_tree::Role::Switch;
        makepad_platform::a11y::set_string(&mut node.name, "Stream to headset");
        makepad_platform::a11y::set_string(&mut node.value, &self.status);
        node.toggle = if self.streaming() { a11y_tree::Toggle::On } else { a11y_tree::Toggle::Off };
        node.actions = a11y_tree::Actions::TOGGLE | a11y_tree::Actions::SELECT | a11y_tree::Actions::SET_VALUE | a11y_tree::Actions::SHOW_MENU | a11y_tree::Actions::FOCUS;
        node.states = a11y_tree::States::FOCUSABLE | a11y_tree::States::HAS_POPUP;
        node.states.set(a11y_tree::States::EXPANDED, self.open());
    }

    fn handle_action(&mut self, cx: &mut Cx, action: &a11y_tree::ActionRequest, _scope: &mut Scope) -> a11y_tree::ActionResult {
        use a11y_tree::{ActionArg, ActionResult, Actions as A};
        if self.settings.is_none() {
            return ActionResult::Disabled;
        }
        let on = if action.action == A::TOGGLE {
            !self.streaming()
        } else if action.action == A::SELECT {
            true
        } else if action.action == A::SET_VALUE {
            match &action.arg {
                ActionArg::Number(v) => *v != 0.0,
                ActionArg::Text(t) => match t.trim().to_ascii_lowercase().as_str() {
                    "on" | "1" | "true" => true,
                    "off" | "0" | "false" => false,
                    _ => return ActionResult::Unsupported,
                },
                _ => return ActionResult::Unsupported,
            }
        } else if action.action == A::SHOW_MENU {
            self.popover.as_popover().open(cx);
            self.show_settings(cx);
            self.refresh(cx);
            return ActionResult::Handled;
        } else {
            return ActionResult::Unhandled;
        };
        self.set_streaming(cx, on);
        self.popover.redraw(cx);
        ActionResult::Handled
    }
}
