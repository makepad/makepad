//! The chrome every profile draws: the map with the tilt-shift layer, the
//! turn banner, the layers popover, the location controls, the side panel
//! and the first-run test-map card — one splash template
//! (`mod.widgets.RouteChrome`) — and the small pieces of state the widgets
//! in it share: the location opt-in machine, the theme preference, the
//! transcript (`ChatState`) the panel and the blur read from the draw
//! scope.
//!
//! `RouteView` (view.rs) wraps the template with the native data plane and
//! the assistant seam; the hosted demo (nav/api.rs) puts the same template
//! in its own window and drives it from the site API.

use makepad_widgets::makepad_platform::storage::{StorageHandle, StorageRequestId, StorageResult};
use makepad_widgets::*;

use crate::trip::TripModel;

pub const AMSTERDAM_CENTER: (f64, f64) = (4.8952, 52.3702);

pub const THEME_STORAGE: &str = "route";
pub const THEME_KEY: &str = "theme";
pub const LOCATION_FIX_TIMEOUT_SECONDS: f64 = 20.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LocationState {
    #[default]
    Idle,
    Waiting,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocationClick {
    Start,
    Recenter,
    Ignore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocationFix {
    First,
    Update,
    Ignore,
}

impl LocationState {
    pub(crate) fn clicked(&mut self) -> LocationClick {
        match self {
            Self::Idle => {
                *self = Self::Waiting;
                LocationClick::Start
            }
            Self::Waiting => LocationClick::Ignore,
            Self::Active => LocationClick::Recenter,
        }
    }

    pub(crate) fn received_fix(&mut self) -> LocationFix {
        match self {
            Self::Idle => LocationFix::Ignore,
            Self::Waiting => {
                *self = Self::Active;
                LocationFix::First
            }
            Self::Active => LocationFix::Update,
        }
    }

    pub(crate) fn failed(&mut self) -> bool {
        if *self == Self::Idle {
            return false;
        }
        *self = Self::Idle;
        true
    }

    pub(crate) fn is_waiting(self) -> bool {
        self == Self::Waiting
    }

    pub(crate) fn stop(&mut self) -> bool {
        self.failed()
    }
}

pub(crate) fn show_location_status(cx: &mut Cx, ui: &WidgetRef, text: &str) {
    ui.label(cx, ids!(location_status_text)).set_text(cx, text);
    ui.widget(cx, ids!(location_status)).set_visible(cx, true);
}

pub(crate) fn location_error_status(error: &LocationErrorEvent) -> &'static str {
    match error {
        LocationErrorEvent::PermissionDenied => "Permission denied — tap to retry",
        LocationErrorEvent::Unavailable(message)
            if message.to_ascii_lowercase().contains("timeout")
                || message.to_ascii_lowercase().contains("timed out") =>
        {
            "Location timed out — tap to retry"
        }
        LocationErrorEvent::Unavailable(_) => "Location unavailable — tap to retry",
    }
}

/// The theme choice, kept in the instance's storage: the app's own
/// namespace standalone, the host's jail for a module.
#[derive(Default)]
pub struct ThemePreference {
    load: Option<StorageRequestId>,
}

impl ThemePreference {
    pub fn start(&mut self, cx: &mut Cx, storage: &StorageHandle) {
        self.load = Some(storage.get(cx, THEME_KEY));
    }

    pub fn restored(&mut self, event: &Event) -> Option<u32> {
        let Event::Storage(responses) = event else {
            return None;
        };
        let request_id = self.load?;
        let response = responses
            .iter()
            .find(|response| response.request_id == request_id)?;
        self.load = None;
        let Ok(StorageResult::Value(Some(bytes))) = &response.result else {
            return None;
        };
        match bytes.as_slice() {
            b"light" => Some(0),
            b"night" => Some(1),
            b"circuit" => Some(2),
            _ => None,
        }
    }

    pub fn save(&mut self, cx: &mut Cx, storage: &StorageHandle, theme: u32) {
        self.load = None;
        let name = ["light", "night", "circuit"][theme.min(2) as usize];
        storage.set(cx, THEME_KEY, name.as_bytes().to_vec());
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.TiltShiftLayerBase = #(TiltShiftLayer::register_widget(vm))

    let PanelText = Label{
        width: Fill
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 9}
        }
    }

    // Light panel + touch sizing: the desktop theme's label text is white,
    // so pin dark colors; bigger text/padding for finger targets.
    let LayerCheck = CheckBox{
        padding: Inset{top: 8, bottom: 8, left: 4, right: 10}
        label_walk: Walk{
            width: Fit
            height: Fit
            margin: Inset{left: 22}
        }
        draw_text +: {
            color: theme.color_text
            color_hover: theme.color_text_hover
            color_down: theme.color_text_hover
            color_active: theme.color_text
            color_focus: theme.color_text
            text_style: theme.font_regular{font_size: 11}
        }
    }

    let AppButton = Button{
        draw_text +: {
            color: theme.color_text
            color_hover: theme.color_text_hover
            color_focus: theme.color_text
            color_down: theme.color_text_hover
            text_style: theme.font_regular{font_size: 12}
        }
    }

    // The whole surface, map to popup, as one template: the native view
    // and the hosted demo both instantiate it.
    mod.widgets.RouteChrome = View{
        width: Fill
        height: Fill
        flow: Overlay

        map := MapView{
            width: Fill
            height: Fill
            // GPU-opt benchmark scene: AMS side view at the
            // lowest zoom that shows 3D geometry.
            center_lon: 4.8952
            center_lat: 52.3702
            zoom: 15.6
            tilt: 60.0
            min_zoom: 3.0
            mbtiles_path: "local/maps/world.mkmap"
            detail_mbtiles_path: "local/maps/world.mkmap"
            bridge_dz_mbtiles_path: "local/maps/nl-bridge-dz.mbtiles"
            buildings_3d: true
            // The @cam readout is a workbench tool: off in the app,
            // on through the layers popover\'s "Camera readout".
            debug_cam: false
        }

        // --- Tilt-shift blur (above the map, below all UI) ---
        tilt_shift := mod.widgets.TiltShiftLayerBase{
            width: Fill
            height: Fill
            draw_bg +: {
                scene_texture: texture_2d(float)
                mip0_texture: texture_2d(float)
                mip1_texture: texture_2d(float)
                mip2_texture: texture_2d(float)
                mip3_texture: texture_2d(float)
                mip4_texture: texture_2d(float)
                mip5_texture: texture_2d(float)
                has_gauss: uniform(0.0)
                source_y_flip: uniform(0.0)
                strength: uniform(0.0)
                focus_y: uniform(0.55)
                band: uniform(0.13)

                // Bicubic B-spline reconstruction (4 bilinear taps) — same as
                // GaussRoundedView. Single-tap bilinear of a low-res mip shows
                // its texel lattice on the map's thin high-contrast lines.
                bicubic_h: fn(uv: vec2, size: vec2) -> vec4 {
                    let tc = uv * size - 0.5
                    let f = fract(tc)
                    let tc0 = floor(tc)
                    let f2 = f * f
                    let f3 = f2 * f
                    let omf = 1.0 - f
                    let w1 = (f3 * 3.0 - f2 * 6.0 + 4.0) / 6.0
                    let g0 = omf * omf * omf / 6.0 + w1
                    let h0 = clamp((tc0 - 0.5 + w1 / g0) / size, vec2(0.0, 0.0), vec2(1.0, 1.0))
                    let h1 = clamp((tc0 + 1.5 + (f3 / 6.0) / (1.0 - g0)) / size, vec2(0.0, 0.0), vec2(1.0, 1.0))
                    return vec4(h0.x, h0.y, h1.x, h1.y)
                }

                bicubic_g0: fn(uv: vec2, size: vec2) -> vec2 {
                    let f = fract(uv * size - 0.5)
                    let f2 = f * f
                    let omf = 1.0 - f
                    return omf * omf * omf / 6.0 + (f2 * f * 3.0 - f2 * 6.0 + 4.0) / 6.0
                }

                sample_at: fn(uv: vec2, idx: float) -> vec4 {
                    if idx < 0.5 {
                        return self.scene_texture.sample_as_bgra(uv)
                    }
                    if idx < 1.5 {
                        let size = max(self.mip0_texture.size(), vec2(1.0, 1.0))
                        let h = self.bicubic_h(uv, size)
                        let g0 = self.bicubic_g0(uv, size)
                        let g1 = 1.0 - g0
                        return self.mip0_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                            + self.mip0_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                            + self.mip0_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                            + self.mip0_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                    }
                    if idx < 2.5 {
                        let size = max(self.mip1_texture.size(), vec2(1.0, 1.0))
                        let h = self.bicubic_h(uv, size)
                        let g0 = self.bicubic_g0(uv, size)
                        let g1 = 1.0 - g0
                        return self.mip1_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                            + self.mip1_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                            + self.mip1_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                            + self.mip1_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                    }
                    if idx < 3.5 {
                        let size = max(self.mip2_texture.size(), vec2(1.0, 1.0))
                        let h = self.bicubic_h(uv, size)
                        let g0 = self.bicubic_g0(uv, size)
                        let g1 = 1.0 - g0
                        return self.mip2_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                            + self.mip2_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                            + self.mip2_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                            + self.mip2_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                    }
                    if idx < 4.5 {
                        let size = max(self.mip3_texture.size(), vec2(1.0, 1.0))
                        let h = self.bicubic_h(uv, size)
                        let g0 = self.bicubic_g0(uv, size)
                        let g1 = 1.0 - g0
                        return self.mip3_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                            + self.mip3_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                            + self.mip3_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                            + self.mip3_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                    }
                    if idx < 5.5 {
                        let size = max(self.mip4_texture.size(), vec2(1.0, 1.0))
                        let h = self.bicubic_h(uv, size)
                        let g0 = self.bicubic_g0(uv, size)
                        let g1 = 1.0 - g0
                        return self.mip4_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                            + self.mip4_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                            + self.mip4_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                            + self.mip4_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                    }
                    let size = max(self.mip5_texture.size(), vec2(1.0, 1.0))
                    let h = self.bicubic_h(uv, size)
                    let g0 = self.bicubic_g0(uv, size)
                    let g1 = 1.0 - g0
                    return self.mip5_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                        + self.mip5_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                        + self.mip5_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                        + self.mip5_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                }

                pixel: fn() {
                    if self.has_gauss < 0.5 {
                        return vec4(0.0, 0.0, 0.0, 0.0)
                    }
                    let uv = self.pos
                    let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                    let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                    // Distance out of the focus band, 0..1 to the edge.
                    let d = abs(uv.y - self.focus_y)
                    let t = clamp((d - self.band) / 0.45, 0.0, 1.0)
                    // Linear circle-of-confusion: blur RADIUS grows linearly with
                    // distance (like a real lens), so the pyramid level is its log2.
                    // A linear LEVEL ramp doubles the radius per step and packs half
                    // the total blur growth into the last stretch before the screen
                    // edge — it reads as a hard blur "band" at the top.
                    //
                    // The growth RATE is a constant: tilt strength only raises the
                    // level ceiling. Scaling the rate with strength (radius factor
                    // exp2(6*strength)) slams full tilt into deep levels within a few
                    // percent of screen past the focus band — a hard band again.
                    let level = clamp(log2(1.0 + t * 21.0), 0.0, 6.0 * self.strength)
                    let base_idx = floor(level)
                    let frac = level - base_idx
                    let a = self.sample_at(safe_uv, base_idx)
                    let b = self.sample_at(safe_uv, base_idx + 1.0)
                    let c = a.mix(b, frac)
                    // Slight saturation lift sells the miniature look.
                    let gray = (c.x + c.y + c.z) / 3.0
                    let sat = vec3(gray, gray, gray).mix(c.xyz, 1.12)
                    return vec4(sat, 1.0)
                }
            }
        }

        // --- All UI hoists above the tilt-shift layer ---
        ui_layer := mod.widgets.glass.Layer{

        // --- Turn banner (top-center) ---
        View{
            width: Fill
            height: Fit
            align: Align{x: 0.5 y: 0.0}
            banner := RoundedView{
                visible: false
                flow: Down
                width: Fit
                height: Fit
                margin: Inset{top: 12}
                padding: Inset{left: 18, right: 18, top: 10, bottom: 10}
                align: Align{x: 0.5 y: 0.0}
                draw_bg +: {
                    color: #x1a7a3cf0
                    border_radius: 9.0
                }
                banner_text := Label{
                    draw_text +: {
                        color: #xffffff
                        text_style: theme.font_bold{font_size: 13}
                    }
                }
                banner_dist := Label{
                    draw_text +: {
                        color: #xd8f2e2
                        text_style: theme.font_regular{font_size: 10}
                    }
                }
            }
        }

        // --- Layers popover (bottom-left, touch) ---
        View{
            width: Fill
            height: Fill
            flow: Down
            align: Align{x: 0.0 y: 1.0}
            layers_panel := mod.widgets.glass.Panel{
                visible: false
                flow: Down
                width: Fit
                height: Fit
                margin: Inset{left: 14, bottom: 6}
                padding: Inset{left: 18, right: 22, top: 14, bottom: 14}
                spacing: 2
                draw_bg +: {
                    corner_radius: 12.0
                    tint_color: theme.color_bg_container
                    tint_alpha: 0.30
                }
                // View-effects group first: these two act
                // on the CAMERA/rendering, not on map data
                // — the divider separates them from the
                // content layers below (same unlabeled
                // hairline idiom as the theme group).
                tilt_check := LayerCheck{text: "Tilt-shift"}
                // Grayed (not hidden) outside the
                // near-first-person regime — the stock
                // disabled label washes out on the glass
                // popover, so keep it readable.
                warp_check := LayerCheck{
                    text: "Space warp"
                    draw_text +: { color_disabled: theme.color_text_disabled }
                }
                // The map\'s @cam line, the command that recreates the
                // view: a dev switch, off until asked for.
                cam_check := LayerCheck{text: "Camera readout"}
                Hr{
                    height: 16
                }
                layer_rain := LayerCheck{text: "Rain radar"}
                layer_wind := LayerCheck{text: "Wind"}
                layer_terrain := LayerCheck{text: "Terrain"}
                layer_chargers := LayerCheck{text: "EV chargers"}
                layer_transit := LayerCheck{text: "Transit"}
                layer_nature := LayerCheck{text: "Nature"}
                layer_districts := LayerCheck{text: "Districts"}
                layer_buildings := LayerCheck{text: "Building age"}
                layer_demographics := LayerCheck{text: "Population"}
                Hr{
                    height: 16
                }
                theme_night := LayerCheck{text: "Night theme"}
                theme_circuit := LayerCheck{text: "Circuit City"}
            }
            location_status := RoundedView{
                visible: false
                width: Fit
                height: Fit
                margin: Inset{left: 14, bottom: 6}
                padding: Inset{left: 12, right: 12, top: 7, bottom: 7}
                draw_bg +: {
                    color: theme.color_bg_container
                    border_radius: 8.0
                }
                location_status_text := Label{
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: 10}
                    }
                }
            }
            location_controls := View{
                width: Fit
                height: Fit
                flow: Right
                spacing: 8
                margin: Inset{left: 14, bottom: 16}
                layers_button := AppButton{
                    padding: Inset{left: 16, right: 16, top: 12, bottom: 12}
                    text: "▤"
                }
                location_button := AppButton{
                    padding: Inset{left: 14, right: 14, top: 12, bottom: 12}
                    text: "Fetch current location"
                }
            }
        }

        // One side panel in every profile; only its services differ.
        RouteSidePanel{}

        // --- First-run test map (centered, over everything) ---
        View{
            width: Fill
            height: Fill
            align: Align{x: 0.5 y: 0.5}
            testmap_panel := mod.widgets.glass.Panel{
                visible: false
                flow: Down
                width: Fill
                max_width: 520
                height: Fit
                padding: Inset{left: 24, right: 24, top: 20, bottom: 20}
                spacing: 8
                draw_bg +: {
                    corner_radius: 14.0
                    tint_color: theme.color_bg_container
                    tint_alpha: 0.36
                }
                Label{
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_bold{font_size: 13}
                    }
                    text: "Amsterdam test map"
                }
                testmap_headline := Label{
                    width: Fill
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_bold{font_size: 10.5}
                    }
                }
                // Track and fill: the fill's width is set
                // from the bake fraction each frame.
                RoundedView{
                    width: Fill
                    height: 10
                    margin: Inset{top: 2, bottom: 2}
                    draw_bg +: {
                        color: theme.color_bevel_inset_2
                        border_radius: 5.0
                    }
                    testmap_bar := RoundedView{
                        width: 0
                        height: Fill
                        draw_bg +: {
                            color: theme.color_focus
                            border_radius: 5.0
                        }
                    }
                }
                testmap_status := PanelText{}
                testmap_log := Label{
                    width: Fill
                    draw_text +: {
                        color: theme.color_text_disabled
                        text_style: theme.font_regular{font_size: 8.5}
                    }
                }
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 8
                    margin: Inset{top: 6}
                    testmap_start := AppButton{
                        padding: Inset{left: 14, right: 14, top: 8, bottom: 8}
                        text: "Build test map"
                    }
                    testmap_dismiss := AppButton{
                        padding: Inset{left: 14, right: 14, top: 8, bottom: 8}
                        text: "Not now"
                    }
                }
            }
        }

        } // ui_layer
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EntryKind {
    User,
    Assistant,
    Tool,
    Info,
}

/// One transcript row; `trip` indexes into `ChatState::trips` and renders
/// with a `>` re-apply button.
pub struct ChatEntry {
    pub kind: EntryKind,
    pub text: String,
    pub trip: Option<usize>,
}

/// Shared with `TranscriptList`/`TiltShiftLayer` via `Scope::with_data`.
#[derive(Default)]
pub struct ChatState {
    pub entries: Vec<ChatEntry>,
    pub trips: Vec<TripModel>,
    /// Streaming assistant text of the in-flight turn.
    pub pending: String,
    /// UI follows the map theme (night/circuit = dark panels).
    pub dark: bool,
    /// Tilt-shift enabled (settings checkbox) — the layer still gates on
    /// the map actually being tilted via `tilt_strength`.
    pub tilt_shift_on: bool,
    /// 0..1 from the current map tilt (0 = flat, no blur).
    pub tilt_strength: f32,
}

/// Full-window tilt-shift blur over the base scene: samples the window
/// gauss pyramid per pixel — sharp focus band, mip level rising towards
/// the top/bottom edges. Hoists into the overlay draw list (like glass),
/// so panels drawn as glass stay sharp above it.
#[derive(Script, ScriptHook, Widget)]
pub struct TiltShiftLayer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
}

impl TiltShiftLayer {
    fn bind_snapshot(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let draw = &mut self.draw_bg.draw_vars;
        if let Some(snapshot) = snapshot {
            draw.set_texture(0, &snapshot.scene_texture);
            for slot in 1..=GAUSS_VIEW_LEVELS {
                if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                    draw.set_texture(slot, texture);
                } else {
                    draw.empty_texture(slot);
                }
            }
            draw.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
            draw.set_uniform(cx, live_id!(has_gauss), &[1.0]);
        } else {
            for slot in 0..=GAUSS_VIEW_LEVELS {
                draw.empty_texture(slot);
            }
            draw.set_uniform(cx, live_id!(has_gauss), &[0.0]);
        }
    }
}

impl Widget for TiltShiftLayer {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let (on, strength) = scope
            .data
            .get_mut::<ChatState>()
            .map(|chat| (chat.tilt_shift_on, chat.tilt_strength))
            .unwrap_or((false, 0.0));
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        // ALWAYS claim the overlay slot, even when inactive: overlay lists
        // keep their first-appended position, so activating later would
        // put the blur ABOVE UI layers that registered earlier.
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        // The layer's rect is its place in the tree — the window at the
        // root, the host's tile when the view is hosted — never the pass:
        // an overlay draw list changes draw order, not coordinates. The
        // blur samples the scene by quad-local uv, which is exact wherever
        // the chrome fills its pass (a window, a host's capture of the tile).
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        if on && strength > 0.01 {
            let snapshot = request_window_gauss(cx);
            self.bind_snapshot(cx, snapshot);
            self.draw_bg
                .draw_vars
                .set_uniform(cx, live_id!(strength), &[strength]);
            self.draw_bg.draw_abs(cx, rect);
        }
        cx.end_turtle();
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }
}

/// Per-kind transcript text color for the current UI theme.
fn entry_color(kind: EntryKind, is_trip: bool, dark: bool) -> Vec4 {
    if is_trip {
        return if dark { vec4(0.38, 0.65, 0.98, 1.0) } else { vec4(0.11, 0.31, 0.85, 1.0) };
    }
    match (kind, dark) {
        (EntryKind::User, false) => vec4(0.06, 0.09, 0.13, 1.0),
        (EntryKind::User, true) => vec4(0.91, 0.93, 0.96, 1.0),
        (EntryKind::Assistant, false) => vec4(0.16, 0.21, 0.25, 1.0),
        (EntryKind::Assistant, true) => vec4(0.77, 0.81, 0.85, 1.0),
        (EntryKind::Tool, false) => vec4(0.48, 0.53, 0.58, 1.0),
        (EntryKind::Tool, true) => vec4(0.47, 0.51, 0.55, 1.0),
        (EntryKind::Info, false) => vec4(0.58, 0.63, 0.68, 1.0),
        (EntryKind::Info, true) => vec4(0.42, 0.46, 0.51, 1.0),
    }
}

/// PortalList-backed chat transcript; rows come from the `ChatState` in
/// the event/draw scope.
#[derive(Script, ScriptHook, Widget)]
pub struct TranscriptList {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
}

impl Widget for TranscriptList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                let Some(chat) = scope.data.get_mut::<ChatState>() else {
                    continue;
                };
                let extra = if chat.pending.is_empty() { 0 } else { 1 };
                let total = chat.entries.len() + extra;
                list.set_item_range(cx, 0, total);
                while let Some(idx) = list.next_visible_item(cx) {
                    // next_visible_item fills the viewport past the range —
                    // out-of-range ids must draw nothing.
                    if idx >= total {
                        continue;
                    }
                    let (text, template) = if let Some(entry) = chat.entries.get(idx) {
                        let template = if entry.trip.is_some() {
                            id!(TripLine)
                        } else {
                            match entry.kind {
                                EntryKind::User => id!(UserLine),
                                EntryKind::Assistant => id!(AssistantLine),
                                EntryKind::Tool => id!(ToolLine),
                                EntryKind::Info => id!(InfoLine),
                            }
                        };
                        (entry.text.clone(), template)
                    } else {
                        (chat.pending.clone(), id!(AssistantLine))
                    };
                    let is_trip = chat
                        .entries
                        .get(idx)
                        .map(|e| e.trip.is_some())
                        .unwrap_or(false);
                    let kind = chat
                        .entries
                        .get(idx)
                        .map(|e| e.kind)
                        .unwrap_or(EntryKind::Assistant);
                    let color = entry_color(kind, is_trip, chat.dark);
                    let item = list.item(cx, idx, template);
                    let mut label = item.label(cx, ids!(line_label));
                    label.set_text(cx, &text);
                    script_apply_eval!(cx, label, {
                        draw_text +: {
                            color: #(color)
                        }
                    });
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}
