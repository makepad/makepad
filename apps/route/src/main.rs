//! AI route/trip planner — voice-driven navigation copilot.
//!
//! M1: tool broker + typed chat, cloud brain first (route.md §6). A Claude
//! backend drives the tool loop end-to-end: geo_search, route_plan/along,
//! map camera/markers, weather_now radar nowcast. TripModel is the source
//! of truth; every agent action is mirrored on the map. Voice, the local
//! Qwen dispatcher and EV charge planning land in M2+.
//!
//! Needs: `local/maps/*` nav data (see examples/map), ANTHROPIC_API_KEY as
//! env var or a file of that name at the repo root. Run from the repo root.

pub use ::makepad_widgets;

#[cfg(all(feature = "native", feature = "demo"))]
compile_error!("features `native` and `demo` are mutually exclusive");
#[cfg(not(any(feature = "native", feature = "demo")))]
compile_error!("enable either feature `native` or feature `demo`");

#[cfg(feature = "native")]
use makepad_converse::agent_seam::*;
use makepad_widgets::*;

#[cfg(feature = "native")]
mod broker;
#[cfg(feature = "native")]
mod claude_agent;
#[cfg(feature = "native")]
mod ddg;
#[cfg(feature = "native")]
mod history;
#[cfg(feature = "native")]
mod layers;
#[cfg(feature = "native")]
mod local_agent;
#[cfg(feature = "native")]
mod nav;
#[cfg(any(feature = "demo", test))]
mod nav_api;
#[cfg(feature = "native")]
mod nav_data;
mod provisioner;
mod side_panel;
#[cfg(feature = "native")]
mod testmap;
#[cfg(feature = "native")]
mod tools;
mod trip;
#[cfg(feature = "native")]
mod voice;

#[cfg(feature = "demo")]
mod demo_app;

#[cfg(feature = "native")]
use broker::{MarkerLegend, ToolCtx};
#[cfg(feature = "native")]
use ddg::{DdgEvent, DdgState};
#[cfg(feature = "native")]
use history::DriveLog;
#[cfg(feature = "native")]
use layers::{LayerState, TerrainUpdate, WindUpdate};
#[cfg(feature = "native")]
use nav::{ActiveNav, NavAction, NavTick};
#[cfg(feature = "native")]
use nav_data::{NavData, NavLoad, RadarData};
#[cfg(feature = "native")]
use makepad_converse::SpeechOutput;
#[cfg(feature = "native")]
use provisioner::MapProvisioner;
#[cfg(feature = "native")]
use side_panel::{PanelAction, PanelController};
#[cfg(feature = "native")]
use testmap::Stage as TestMapStage;
#[cfg(feature = "native")]
use trip::TripModel;
#[cfg(feature = "native")]
use voice::{GateResult, VoiceGate};

#[cfg(feature = "native")]
app_main!(App);
#[cfg(feature = "demo")]
use demo_app::App as DemoApp;
#[cfg(feature = "demo")]
app_main!(DemoApp);

/// Dam square, the point the map opens on and where a fresh test map
/// lands.
pub(crate) const AMSTERDAM_CENTER: (f64, f64) = (4.8952, 52.3702);

#[cfg(feature = "native")]
const SYSTEM_PROMPT: &str = "\
You are the route assistant inside a live map app (Netherlands detail, Europe-wide places), \
a conversational replacement for a car GPS. The user sees a full-screen map; you act only \
through tools and short replies.

Rules:
- Mirror everything on the map: plan trips with route_plan, drop markers for candidates you \
mention, fly the camera to places you talk about. The user must always see what you did.
- Replies are 1-3 short sentences, conversational, no coordinate dumps, no markdown. Tool \
digests are already shown in the transcript — summarize outcomes, don't repeat them.
- Stops and legs have stable ids (stop_2, leg_1); use them for changes and references.
- Coordinates are always lon,lat (WGS84). Waypoints accept place names, 'lon,lat', or 'here'.
- Each user message ends with an [app state] block (map center, trip digest) — trust it.
- If a tool reports data still loading or out of coverage, say so briefly; don't guess.";

#[cfg(feature = "native")]
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.TiltShiftLayerBase = #(TiltShiftLayer::register_widget(vm))

    let PanelText = Label{
        width: Fill
        draw_text +: {
            color: #x22303c
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
            color: #x223038
            color_hover: #x000000
            color_down: #x000000
            color_active: #x223038
            color_focus: #x223038
            text_style: theme.font_regular{font_size: 11}
        }
    }

    let AppButton = Button{
        draw_text +: {
            color: #x223038
            color_hover: #x000000
            color_focus: #x223038
            color_down: #x000000
            text_style: theme.font_regular{font_size: 12}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(3400, 2050)
                pass.clear_color: vec4(0.08, 0.10, 0.12, 1.0)
                body +: {
                    View{
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
                            overlay_mbtiles_paths: "local/maps/ocean-low.mbtiles;local/maps/ocean-high.mbtiles"
                            buildings_3d: true
                        }

                        // --- Tilt-shift blur (above the map, below all UI) ---
                        tilt_shift := mod.widgets.TiltShiftLayerBase{
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
                                    tint_color: #xf8fbff
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
                                    draw_text +: { color_disabled: #x8a8f98 }
                                }
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
                            layers_button := AppButton{
                                margin: Inset{left: 14, bottom: 16}
                                padding: Inset{left: 16, right: 16, top: 12, bottom: 12}
                                text: "▤"
                            }
                        }

                        // Selected at the feature seam: AI/voice on native,
                        // hosted trip planner in the demo profile.
                        RouteSidePanel{}

                        // --- First-run test map (centered, over everything) ---
                        View{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            testmap_panel := mod.widgets.glass.Panel{
                                visible: false
                                flow: Down
                                width: 520
                                height: Fit
                                padding: Inset{left: 24, right: 24, top: 20, bottom: 20}
                                spacing: 8
                                draw_bg +: {
                                    corner_radius: 14.0
                                    tint_color: #xf8fbff
                                    tint_alpha: 0.36
                                }
                                Label{
                                    draw_text +: {
                                        color: #x223038
                                        text_style: theme.font_bold{font_size: 13}
                                    }
                                    text: "Amsterdam test map"
                                }
                                testmap_headline := Label{
                                    width: Fill
                                    draw_text +: {
                                        color: #x223038
                                        text_style: theme.font_bold{font_size: 10.5}
                                    }
                                }
                                // Track and fill: the fill's width is set
                                // from the bake fraction each frame.
                                RoundedView{
                                    width: 470
                                    height: 10
                                    margin: Inset{top: 2, bottom: 2}
                                    draw_bg +: {
                                        color: #x22303820
                                        border_radius: 5.0
                                    }
                                    testmap_bar := RoundedView{
                                        width: 0
                                        height: Fill
                                        draw_bg +: {
                                            color: #x1d4ed8
                                            border_radius: 5.0
                                        }
                                    }
                                }
                                testmap_status := PanelText{}
                                testmap_log := Label{
                                    width: Fill
                                    draw_text +: {
                                        color: #x6b7784
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
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
#[cfg(feature = "native")]
pub enum EntryKind {
    User,
    Assistant,
    Tool,
    Info,
}

/// One transcript row; `trip` indexes into `ChatState::trips` and renders
/// with a `>` re-apply button.
#[cfg(feature = "native")]
pub struct ChatEntry {
    pub kind: EntryKind,
    pub text: String,
    pub trip: Option<usize>,
}

/// Shared with `TranscriptList`/`TiltShiftLayer` via `Scope::with_data`.
#[derive(Default)]
#[cfg(feature = "native")]
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
#[cfg(feature = "native")]
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

#[cfg(feature = "native")]
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

#[cfg(feature = "native")]
impl Widget for TiltShiftLayer {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
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
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, self.layout);
        if on && strength > 0.01 {
            let snapshot = request_window_gauss(cx);
            self.bind_snapshot(cx, snapshot);
            self.draw_bg
                .draw_vars
                .set_uniform(cx, live_id!(strength), &[strength]);
            self.draw_bg.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size,
                },
            );
        }
        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }
}

/// Per-kind transcript text color for the current UI theme.
#[cfg(feature = "native")]
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
#[cfg(feature = "native")]
pub struct TranscriptList {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
}

#[cfg(feature = "native")]
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

#[derive(Script, ScriptHook)]
#[cfg(feature = "native")]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
    #[rust]
    panel: PanelController,
    /// Last pushed disabled-state of the Space-warp row (None = never
    /// pushed): the row grays out whenever the camera leaves the
    /// near-first-person regime and re-enables when it returns.
    #[rust]
    warp_check_disabled: Option<bool>,
    #[rust]
    chat: ChatState,
    #[rust]
    drive_log: DriveLog,
    #[rust]
    agent: Option<Box<dyn Agent>>,
    #[rust]
    session: Option<SessionId>,
    /// Claude escalation agent behind the cloud_ask tool (None = offline).
    #[rust]
    cloud_agent: Option<Box<dyn Agent>>,
    #[rust]
    cloud_session: Option<SessionId>,
    /// In-flight cloud_ask: (local tool_use_id, accumulated answer).
    #[rust]
    pending_cloud: Option<(String, String)>,
    /// Turn timing shared with the LocalAgent worker.
    #[rust]
    local_timing: Option<std::sync::Arc<std::sync::Mutex<String>>>,
    #[rust]
    busy: bool,
    #[rust]
    nav_rx: ToUIReceiver<NavLoad>,
    #[rust]
    radar_rx: ToUIReceiver<RadarData>,
    #[rust]
    nav: Option<NavData>,
    #[rust]
    radar: Option<RadarData>,
    #[rust]
    trip: TripModel,
    #[rust]
    markers: MarkerLegend,
    /// Latest GPS fix from the platform geo service.
    #[rust]
    position: Option<LocationUpdateEvent>,
    #[rust]
    had_first_fix: bool,
    #[rust]
    layers: LayerState,
    #[rust]
    layers_panel_open: bool,
    /// First-run map acquisition (download + bake). Idle on a machine that
    /// already has map data.
    #[rust]
    testmap: MapProvisioner,
    /// Route assistant popover (bottom-right button). Closed on every
    /// launch — nothing persisted.
    #[rust]
    assistant_panel_open: bool,
    /// Routed legs with maneuvers (for turn-by-turn).
    #[rust]
    leg_routes: Vec<makepad_map_nav::graph::Route>,
    #[rust]
    active_nav: Option<ActiveNav>,
    #[rust]
    nav_frame: NextFrame,
    #[rust]
    ddg: DdgState,
    /// Continuous voice loop active (mic button).
    #[rust]
    mic_on: bool,
    #[rust]
    whisper_warmed: bool,
    #[rust]
    ai_llm_ready: bool,
    #[rust]
    ai_gate_ready: bool,
    #[rust]
    voice_gate: Option<VoiceGate>,
    /// SEND instructions that arrived while the agent was busy.
    #[rust]
    voice_queue: Vec<String>,
    #[rust]
    wind_rx: ToUIReceiver<WindUpdate>,
    #[rust]
    terrain_rx: ToUIReceiver<TerrainUpdate>,
    /// Kokoro voice output (🔊 button). None until first startup.
    #[rust]
    speech: Option<SpeechOutput>,
    /// Last nav banner instruction spoken, so each maneuver is announced once.
    #[rust]
    last_spoken_banner: String,
    /// In-flight dispatcher prompt, for user-override cancellation.
    #[rust]
    current_prompt: Option<PromptId>,
    /// Tool calls executed for the current prompt (loop budget).
    #[rust]
    tool_rounds: usize,
    /// Previous (name, args) this turn — breaks identical-call loops.
    #[rust]
    last_tool_call: Option<(String, String)>,
}

/// The machine's UTC offset in seconds, read once. The platform has no
/// timezone database, so we ask the system's own `date` — which knows about
/// DST — instead of guessing. Same house pattern as
/// `apps/files/src/model.rs::local_utc_offset_secs`.
#[cfg(feature = "native")]
fn local_utc_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let Ok(out) = std::process::Command::new("date").arg("+%z").output() else {
                return 0;
            };
            return parse_utc_offset(String::from_utf8_lossy(&out.stdout).trim());
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        0
    })
}

/// `+0200` / `-0730` -> seconds east of UTC.
#[cfg(feature = "native")]
fn parse_utc_offset(text: &str) -> i64 {
    let bytes = text.as_bytes();
    if bytes.len() < 5 || (bytes[0] != b'+' && bytes[0] != b'-') {
        return 0;
    }
    let Ok(hours) = text[1..3].parse::<i64>() else {
        return 0;
    };
    let Ok(minutes) = text[3..5].parse::<i64>() else {
        return 0;
    };
    let magnitude = hours * 3600 + minutes * 60;
    if bytes[0] == b'-' {
        -magnitude
    } else {
        magnitude
    }
}

/// The current local hour-of-day (0..24), wall clock + system UTC offset.
#[cfg(feature = "native")]
fn local_hour_now() -> u32 {
    let epoch_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local_secs = epoch_secs + local_utc_offset_secs();
    (local_secs.rem_euclid(86_400) / 3600) as u32
}

/// Civil-twilight approximation for the startup theme, no location lookup:
/// night from 19:00 through 06:59, light from 07:00 through 18:59.
#[cfg(feature = "native")]
fn theme_name_for_hour(hour: u32) -> &'static str {
    if hour >= 19 || hour < 7 {
        "night"
    } else {
        "light"
    }
}

/// Pull "ctx USED/MAX" out of the local timing status line.
#[cfg(feature = "native")]
fn parse_ctx_usage(timing: &str) -> Option<(usize, usize)> {
    let at = timing.rfind("ctx ")?;
    let rest = &timing[at + 4..];
    let (used, max) = rest.trim().split_once('/')?;
    Some((
        used.trim().parse().ok()?,
        max.trim()
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?,
    ))
}

#[cfg(feature = "native")]
fn read_secret(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::fs::read_to_string(name) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

#[cfg(feature = "native")]
impl App {
    /// Idempotent init; also re-runs after a script hot-reload wipes
    /// `#[rust]` state (same guard pattern as examples/map).
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        start_memory_watchdog(None);
        // Civil-twilight default (route.md follow-up): night 19:00-06:59,
        // light 07:00-18:59, local wall clock, no location lookup. Same
        // set_theme_name + apply_layers path the "Night theme" checkbox
        // uses, so the checkbox, the chrome and the map all agree; the
        // user can still flip it manually afterwards.
        let _ = self.layers.set_theme_name(theme_name_for_hour(local_hour_now()));
        self.layers.dirty = false;
        // Applies the theme above (chrome + map + checkboxes) and reflects
        // the rest of the LayerState defaults (e.g. tilt-shift on) in the
        // layers popover.
        self.apply_layers(cx);
        self.adopt_map_source(cx);
        nav_data::start_radar_worker(self.radar_rx.sender());
        cx.start_location_updates();
        // Kokoro af_heart when weights are in reach (this process, the machine
        // node, a LAN box), else the OS voice — the hub decides.
        let speech = SpeechOutput::new("af_heart");
        speech.install_audio_output(cx, 0);
        self.speech = Some(speech);
        self.init_agent(cx);
        self.update_ai_status(cx);
    }

    /// Point the map and the nav plane at whatever this machine has:
    /// the production archives, else a baked test map, else nothing — in
    /// which case the first-run popup offers to build one.
    fn adopt_map_source(&mut self, cx: &mut Cx) {
        let nav_basename = nav_data::nav_basename();
        if let Some(basename) = nav_basename.clone() {
            nav_data::start_nav_load(self.nav_rx.sender(), basename);
        }
        let map = self.ui.map_view(cx, ids!(map));
        if let Some(basename) = self.testmap.ensure_source(cx, &map) {
            if nav_basename.is_none() {
                nav_data::start_nav_load(self.nav_rx.sender(), basename);
            }
        }
        self.refresh_testmap_ui(cx);
    }

    /// Mirror the build state into the popup.
    fn refresh_testmap_ui(&mut self, cx: &mut Cx) {
        let active = self.testmap.is_active();
        self.ui
            .widget(cx, ids!(testmap_panel))
            .set_visible(cx, active);
        if !active {
            return;
        }
        self.ui
            .label(cx, ids!(testmap_headline))
            .set_text(cx, &self.testmap.headline);
        self.ui
            .label(cx, ids!(testmap_status))
            .set_text(cx, &self.testmap.status_line());
        self.ui
            .label(cx, ids!(testmap_log))
            .set_text(cx, &self.testmap.log.join("\n"));
        // The bar is a plain fill inside a fixed 470px track.
        let width = (470.0 * self.testmap.fraction.clamp(0.0, 1.0) as f64).round();
        let mut bar = self.ui.widget(cx, ids!(testmap_bar));
        script_apply_eval!(cx, bar, {
            width: #(width)
        });
        let running = self.testmap.is_running();
        let done = matches!(self.testmap.stage, TestMapStage::Done);
        self.ui
            .button(cx, ids!(testmap_start))
            .set_visible(cx, !running && !done);
        self.ui.button(cx, ids!(testmap_start)).set_text(
            cx,
            if matches!(self.testmap.stage, TestMapStage::Failed(_)) {
                "Try again"
            } else {
                "Build test map"
            },
        );
        self.ui.button(cx, ids!(testmap_dismiss)).set_text(
            cx,
            match self.testmap.stage {
                TestMapStage::Fetching { .. } => "Cancel",
                TestMapStage::Done => "Start driving",
                _ => "Not now",
            },
        );
        // The bake owns the machine for a minute and has no cancel; the
        // popup stays put rather than offering a button that does nothing.
        self.ui
            .button(cx, ids!(testmap_dismiss))
            .set_visible(cx, !matches!(self.testmap.stage, TestMapStage::Baking));
        self.ui.redraw(cx);
    }

    fn make_claude(api_key: String) -> Box<dyn Agent> {
        let model = std::env::var("MAKEPAD_ROUTE_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-5".to_string());
        Box::new(claude_agent::ClaudeAgent::new(model, api_key))
    }

    /// Dispatcher: the in-process local model by default (pure-Rust ggml,
    /// no external processes); MAKEPAD_ROUTE_CLOUD=1 keeps Claude as the
    /// dispatcher instead. Claude, when a key exists, otherwise serves as
    /// the cloud_ask escalation tool.
    fn init_agent(&mut self, cx: &mut Cx) {
        // AI on by default again (perf campaign over); MAKEPAD_NO_AI=1
        // keeps the GPU clear of the 9B/4B/whisper chain when profiling
        // the map renderer.
        if std::env::var_os("MAKEPAD_NO_AI").is_some() {
            return;
        }
        let api_key = read_secret("ANTHROPIC_API_KEY");
        let cloud_dispatch = std::env::var("MAKEPAD_ROUTE_CLOUD").is_ok() && api_key.is_some();

        let mut agent: Box<dyn Agent> = if cloud_dispatch {
            Self::make_claude(api_key.clone().unwrap())
        } else {
            let model_path = std::env::var("MAKEPAD_ROUTE_LOCAL_MODEL")
                .unwrap_or_else(|_| local_agent::DEFAULT_LOCAL_MODEL.to_string());
            let timing = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            self.local_timing = Some(timing.clone());
            Box::new(local_agent::LocalAgent::new(model_path, timing))
        };
        let session = agent.create_session(
            cx,
            SessionConfig {
                system_prompt: Some(SYSTEM_PROMPT.to_string()),
                tools: broker::tool_definitions(),
                ..Default::default()
            },
        );
        self.session = Some(session);
        self.agent = Some(agent);

        // Escalation valve: only when a key exists and Claude isn't
        // already the dispatcher.
        if !cloud_dispatch {
            if let Some(api_key) = api_key {
                let mut cloud = Self::make_claude(api_key);
                let cloud_session = cloud.create_session(
                    cx,
                    SessionConfig {
                        system_prompt: Some(
                            "You answer knowledge questions for a navigation assistant \
                             (sights, reviews, rankings, world knowledge). Be concise: a short \
                             paragraph or compact list, no markdown."
                                .to_string(),
                        ),
                        ..Default::default()
                    },
                );
                self.cloud_session = Some(cloud_session);
                self.cloud_agent = Some(cloud);
            }
        }
    }

    /// Aggregate boot status: the whole AI pipeline loads at startup
    /// (9B dispatcher, 4B voice gate, whisper via the voice worker).
    fn update_ai_status(&mut self, cx: &mut Cx) {
        if self.local_timing.is_none() {
            self.set_status(cx, "AI loading — cloud dispatcher · voice gate…");
            return;
        }
        let text = if self.ai_llm_ready && self.ai_gate_ready {
            "AI ready — dispatcher 9B ✓ · voice gate 4B ✓ · whisper warm".to_string()
        } else {
            format!(
                "AI loading — dispatcher 9B {} · voice gate 4B {} · whisper…",
                if self.ai_llm_ready { "✓" } else { "…" },
                if self.ai_gate_ready { "✓" } else { "…" },
            )
        };
        self.set_status(cx, &text);
    }

    fn set_status(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn render_transcript(&mut self, cx: &mut Cx) {
        let list = self.ui.portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.redraw(cx);
    }

    fn push_entry(&mut self, cx: &mut Cx, kind: EntryKind, text: &str) {
        self.chat.entries.push(ChatEntry {
            kind,
            text: text.to_string(),
            trip: None,
        });
        self.render_transcript(cx);
    }

    fn push_line(&mut self, cx: &mut Cx, line: &str) {
        self.push_entry(cx, EntryKind::Info, line);
    }

    /// Push a snapshot of the current trip as a re-applyable chat row.
    fn push_trip_entry(&mut self, cx: &mut Cx) {
        if !self.trip.is_routed() {
            return;
        }
        let first = self.trip.stops.first().map(|s| s.name.clone()).unwrap_or_default();
        let last = self.trip.stops.last().map(|s| s.name.clone()).unwrap_or_default();
        let vias = self.trip.stops.len().saturating_sub(2);
        let text = format!(
            "{first} → {last}{} — {:.1} km, {}",
            if vias > 0 { format!(" (+{vias} stop{})", if vias > 1 { "s" } else { "" }) } else { String::new() },
            self.trip.total_distance_m() / 1000.0,
            trip::fmt_duration(self.trip.total_duration_s()),
        );
        let index = self.chat.trips.len();
        self.chat.trips.push(self.trip.clone());
        self.chat.entries.push(ChatEntry {
            kind: EntryKind::Assistant,
            text,
            trip: Some(index),
        });
        self.drive_log.log_trip(&self.trip.digest());
        self.render_transcript(cx);
    }

    fn commit_pending(&mut self, cx: &mut Cx) {
        let text = std::mem::take(&mut self.chat.pending);
        if text.trim().is_empty() {
            return;
        }
        let text = text.trim().to_string();
        self.push_entry(cx, EntryKind::Assistant, &text);
    }

    /// `/tool_name {json args}` — direct tool console, no LLM involved.
    /// Works without an API key; also the manual test path for the broker.
    fn run_local_command(&mut self, cx: &mut Cx, text: &str) {
        let body = &text[1..];
        let (name, args) = match body.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.trim(), rest.trim()),
            None => (body.trim(), ""),
        };
        if name.is_empty() || name == "help" {
            let names: Vec<String> = broker::tool_definitions()
                .iter()
                .map(|d| d.name.clone())
                .collect();
            self.push_line(cx, &format!("tools: {}", names.join(", ")));
            return;
        }
        self.push_entry(cx, EntryKind::Tool, &format!("⚙ {name} {args}"));
        if name == "images_search" {
            let query = broker::parse_field(args, "query").unwrap_or_default();
            match self.ddg.start(cx, &query, None) {
                Ok(()) => self.push_entry(cx, EntryKind::Info, &format!("🔎 images: {query}")),
                Err(error) => self.push_entry(cx, EntryKind::Tool, &format!("⚠ {error}")),
            }
            return;
        }
        match self.execute_tool(cx, name, args) {
            Ok(text) => {
                self.push_entry(cx, EntryKind::Assistant, &text);
                if matches!(name, "route_plan" | "route_add_stop" | "route_remove_stop") {
                    self.push_trip_entry(cx);
                }
            }
            Err(error) => self.push_entry(cx, EntryKind::Tool, &format!("⚠ {error}")),
        }
    }

    fn send_user_prompt(&mut self, cx: &mut Cx, text: &str) {
        if text.starts_with('/') {
            self.run_local_command(cx, text);
            return;
        }
        if self.agent.is_none() {
            self.push_line(cx, "⚠ no agent available");
            return;
        }
        // A new turn obsoletes whatever the voice was still saying — and a
        // typed command while the dispatcher runs is always an override.
        if self.busy {
            self.interrupt_turn(cx);
        }
        if let Some(speech) = &mut self.speech {
            speech.stop();
        }
        if self.busy {
            self.set_status(cx, "still thinking — wait for the current answer");
            return;
        }
        self.push_entry(cx, EntryKind::User, text);
        let map = self.ui.map_view(cx, ids!(map));
        let (lon, lat) = map.center().unwrap_or((4.8952, 52.3702));
        let zoom = map.map_zoom().unwrap_or(13.0);
        let gps = match &self.position {
            Some(p) => format!("{:.5},{:.5} (±{:.0}m)", p.lon, p.lat, p.accuracy_m),
            None => "no fix — 'here' falls back to map center".to_string(),
        };
        let prompt = format!(
            "{text}\n\n[app state]\ngps: {gps}\nmap_center: {lon:.5},{lat:.5} zoom {zoom:.1}\nnav_data: {}\ntrip:\n{}",
            if self.nav.is_some() { "ready" } else { "loading" },
            self.trip.digest()
        );
        let (agent, session) = (self.agent.as_mut().unwrap(), self.session.unwrap());
        let prompt_id = agent.send_prompt(cx, session, &prompt);
        self.current_prompt = Some(prompt_id);
        self.tool_rounds = 0;
        self.last_tool_call = None;
        self.busy = true;
        self.set_status(cx, "thinking…");
    }

    /// User override: cancel the in-flight dispatcher turn NOW. The worker
    /// stops per-token, closes the assistant turn and drops its tool calls.
    fn interrupt_turn(&mut self, cx: &mut Cx) {
        if !self.busy {
            return;
        }
        if let (Some(agent), Some(prompt_id)) = (self.agent.as_mut(), self.current_prompt) {
            agent.cancel_prompt(cx, prompt_id);
        }
        self.busy = false;
        self.voice_queue.clear();
        if let Some(speech) = &mut self.speech {
            speech.stop();
        }
        self.push_entry(cx, EntryKind::Info, "⏹ interrupted");
        self.set_status(cx, "ready");
    }

    /// Does this utterance override the current activity? Only the LEADING
    /// words count — "add a charging stop" must not match on "stop".
    fn is_override_command(text: &str) -> bool {
        let lower = text.to_lowercase();
        lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .take(2)
            .any(|w| {
                matches!(
                    w,
                    "stop" | "cancel" | "nevermind" | "never" | "forget" | "wait" | "actually"
                )
            })
    }

    /// "stop" / "nevermind" with no follow-up command: just halt.
    fn is_pure_stop(text: &str) -> bool {
        let words: Vec<&str> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        words.len() <= 3
            && words.iter().all(|w| {
                matches!(
                    w.to_lowercase().as_str(),
                    "ok" | "okay" | "no" | "stop" | "cancel" | "nevermind" | "never" | "mind"
                        | "it" | "that" | "forget" | "computer"
                )
            })
    }

    fn on_agent_event(&mut self, cx: &mut Cx, event: AgentEvent) {
        match event {
            AgentEvent::SessionReady { .. } => {
                self.ai_llm_ready = true;
                // Eager but SERIALIZED: the gate's 4B loads after the 9B is
                // resident so startup peaks don't stack (iPad jetsam kills
                // on peak footprint, not steady state).
                if self.voice_gate.is_none() {
                    self.voice_gate = Some(VoiceGate::new());
                }
                // Chain whisper after the LLMs (eager but serialized).
                self.ui.voice_wave(cx, ids!(mic_wave)).prewarm(cx);
                self.update_ai_status(cx);
            }
            AgentEvent::SessionError { error, .. } => {
                self.busy = false;
                self.push_line(cx, &format!("⚠ session error: {error}"));
            }
            AgentEvent::TextDelta { text, .. } => {
                self.chat.pending.push_str(&text);
                if let Some(speech) = &mut self.speech {
                    speech.feed(&text);
                }
                self.render_transcript(cx);
            }
            AgentEvent::ToolRequest {
                tool_use_id,
                tool_name,
                tool_input,
                ..
            } => {
                self.commit_pending(cx);
                self.run_tool(cx, &tool_use_id, &tool_name, &tool_input);
            }
            AgentEvent::TurnComplete { .. } => {
                self.commit_pending(cx);
                if let Some(speech) = &mut self.speech {
                    speech.flush();
                }
                self.busy = false;
                self.current_prompt = None;
                let timing = self
                    .local_timing
                    .as_ref()
                    .and_then(|t| t.lock().ok().map(|s| s.clone()))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "ready".to_string());
                // The session is append-only: once the context is nearly
                // full it cannot recover — restart with a fresh session
                // (mmap makes the reload cheap; chat history stays in the
                // transcript, the model just loses conversational memory).
                if let Some((used, max)) = parse_ctx_usage(&timing) {
                    if used * 10 > max * 9 {
                        self.push_entry(
                            cx,
                            EntryKind::Info,
                            &format!("⚠ context {used}/{max} — restarting local session"),
                        );
                        self.agent = None;
                        self.session = None;
                        self.init_agent(cx);
                    }
                }
                self.set_status(cx, &timing);
            }
            AgentEvent::PromptError { error, .. } => {
                self.commit_pending(cx);
                self.busy = false;
                self.current_prompt = None;
                self.push_line(cx, &format!("⚠ {error}"));
                self.set_status(cx, "error — try again");
            }
        }
    }

    /// Execute one broker tool and apply any layer/nav changes it made.
    fn execute_tool(&mut self, cx: &mut Cx, name: &str, input: &str) -> Result<String, String> {
        let map = self.ui.map_view(cx, ids!(map));
        let mut nav_action = None;
        let result = {
            let mut tool_ctx = ToolCtx {
                cx,
                map: &map,
                trip: &mut self.trip,
                nav: self.nav.as_mut(),
                radar: self.radar.as_ref(),
                markers: &mut self.markers,
                position: self.position.as_ref().map(|p| (p.lon, p.lat)),
                layers: &mut self.layers,
                leg_routes: &mut self.leg_routes,
                nav_action: &mut nav_action,
            };
            broker::execute(&mut tool_ctx, name, input)
        };
        if self.layers.dirty {
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        match nav_action {
            Some(NavAction::Start { simulate }) => self.start_nav(cx, simulate),
            Some(NavAction::Stop) => self.stop_nav(cx),
            None => {}
        }
        result
    }

    // --- Turn-by-turn navigation -------------------------------------------

    fn start_nav(&mut self, cx: &mut Cx, simulate: bool) {
        let Some(mut nav) = ActiveNav::new(self.leg_routes.clone(), simulate) else {
            self.push_line(cx, "⚠ nav: no routed legs");
            return;
        };
        let map = self.ui.map_view(cx, ids!(map));
        if let Some(start) = nav.start_point() {
            map.fly_to(cx, start.lon, start.lat, 17.0);
        }
        // Driving view: tilted follow camera (also arms tilt-shift).
        map.set_tilt(cx, 42.0);
        self.ui.view(cx, ids!(banner)).set_visible(cx, true);
        self.ui
            .label(cx, ids!(banner_text))
            .set_text(cx, "Starting navigation…");
        self.ui.label(cx, ids!(banner_dist)).set_text(cx, "");
        if simulate {
            nav.sim_last_tick = Some(std::time::Instant::now());
            self.nav_frame = cx.new_next_frame();
        }
        self.drive_log.log_trip(&format!(
            "nav_start ({})\n{}",
            if simulate { "sim" } else { "gps" },
            self.trip.digest()
        ));
        self.active_nav = Some(nav);
        self.push_line(
            cx,
            if simulate { "▶ navigating (simulated drive)" } else { "▶ navigating (live GPS)" },
        );
    }

    fn stop_nav(&mut self, cx: &mut Cx) {
        if self.active_nav.take().is_none() {
            return;
        }
        let map = self.ui.map_view(cx, ids!(map));
        map.set_rotation(cx, 0.0);
        map.set_tilt(cx, 0.0);
        self.ui.view(cx, ids!(banner)).set_visible(cx, false);
        self.push_line(cx, "■ navigation ended");
    }

    fn apply_nav_tick(&mut self, cx: &mut Cx, tick: NavTick) {
        let map = self.ui.map_view(cx, ids!(map));
        map.set_puck(
            cx,
            Some(MapPuck::new(
                tick.position.lon,
                tick.position.lat,
                tick.heading,
                12.0,
            )),
        );
        map.set_center(cx, tick.position.lon, tick.position.lat);
        map.set_rotation(cx, tick.rotation);
        map.set_route_progress(cx, tick.progress_index);
        if !tick.banner.is_empty() {
            self.ui.label(cx, ids!(banner_text)).set_text(cx, &tick.banner);
            self.ui
                .label(cx, ids!(banner_dist))
                .set_text(cx, &tick.banner_dist);
            // Announce each maneuver once, when it becomes the current banner.
            if tick.banner != self.last_spoken_banner {
                self.last_spoken_banner = tick.banner.clone();
                if let Some(speech) = &self.speech {
                    speech.enqueue(&tick.banner);
                }
            }
        }

        if tick.arrived {
            let dest = self
                .trip
                .stops
                .last()
                .map(|s| s.name.clone())
                .unwrap_or_default();
            self.push_line(cx, &format!("🏁 arrived at {dest}"));
            if let Some(speech) = &self.speech {
                speech.enqueue(&format!("You have arrived at {dest}."));
            }
            self.active_nav = None;
            let map = self.ui.map_view(cx, ids!(map));
            map.set_rotation(cx, 0.0);
            return;
        }
        if tick.finished_leg {
            if let Some(nav) = &mut self.active_nav {
                let reached = self
                    .trip
                    .stops
                    .get(nav.leg_index + 1)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                nav.advance_leg();
                self.push_line(cx, &format!("● reached {reached} — continuing"));
            }
        }
        if tick.needs_reroute {
            self.reroute_nav(cx);
        }
    }

    /// Off-route with real GPS: recompute the current leg from here.
    fn reroute_nav(&mut self, cx: &mut Cx) {
        let (Some(nav_data), Some(nav)) = (self.nav.as_mut(), self.active_nav.as_mut()) else {
            return;
        };
        if nav.simulate {
            return;
        }
        let (Some(pos), Some(next_stop)) = (nav.position, self.trip.stops.get(nav.leg_index + 1))
        else {
            return;
        };
        let to = makepad_map_nav::geo::LonLat {
            lon: next_stop.lon,
            lat: next_stop.lat,
        };
        let mode = match self.trip.mode {
            trip::TripMode::Car => makepad_map_nav::graph::TravelMode::Car,
            trip::TripMode::Bike => makepad_map_nav::graph::TravelMode::Bike,
            trip::TripMode::Foot => makepad_map_nav::graph::TravelMode::Foot,
        };
        if let Some(route) = nav_data.route_pair(pos, to, mode) {
            nav.routes[nav.leg_index] = route.clone();
            nav.session = makepad_map_nav::nav::NavSession::new(route);
            // Redraw the route line: completed legs + fresh current + later.
            let points: Vec<(f64, f64)> = nav
                .routes
                .iter()
                .flat_map(|r| r.points.iter().map(|p| (p.lon, p.lat)))
                .collect();
            self.ui.map_view(cx, ids!(map)).set_route(cx, &points);
            self.push_line(cx, "↻ rerouting");
        }
    }

    fn tick_nav_sim(&mut self, cx: &mut Cx) {
        let Some(nav) = &mut self.active_nav else {
            return;
        };
        let Some(tick) = nav.tick_sim() else {
            return;
        };
        self.apply_nav_tick(cx, tick);
        if self.active_nav.is_some() {
            self.nav_frame = cx.new_next_frame();
        }
    }

    /// Events from the Claude escalation agent: stream into the pending
    /// cloud_ask, then hand the answer back to the dispatcher as the tool
    /// result (visible in the transcript per route.md — cloud is explicit).
    fn on_cloud_event(&mut self, cx: &mut Cx, event: AgentEvent) {
        match event {
            AgentEvent::TextDelta { text, .. } => {
                if let Some((_, accum)) = &mut self.pending_cloud {
                    accum.push_str(&text);
                }
            }
            AgentEvent::TurnComplete { .. } => {
                if let Some((tool_use_id, answer)) = self.pending_cloud.take() {
                    let preview: String = answer.chars().take(240).collect();
                    self.push_line(cx, &format!("☁ cloud: {}", preview.trim()));
                    if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
                        agent.send_tool_result(cx, session, &tool_use_id, answer.trim(), false);
                    }
                }
            }
            AgentEvent::PromptError { error, .. } | AgentEvent::SessionError { error, .. } => {
                if let Some((tool_use_id, _)) = self.pending_cloud.take() {
                    self.push_line(cx, &format!("☁ cloud error: {error}"));
                    if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
                        agent.send_tool_result(
                            cx,
                            session,
                            &tool_use_id,
                            &format!("cloud unavailable: {error}"),
                            true,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Mic toggle: starts/stops the continuous voice loop (VAD → STT →
    /// attention gate → dispatcher). Pipeline wiring lands with voice.rs.
    fn toggle_mic(&mut self, cx: &mut Cx) {
        self.mic_on = !self.mic_on;
        let mut button = self.ui.button(cx, ids!(mic_button));
        if self.mic_on {
            button.set_text(cx, "🔴");
            script_apply_eval!(cx, button, {
                draw_text +: {
                    color: #(vec4(0.86, 0.20, 0.20, 1.0))
                }
            });
            self.push_line(
                cx,
                if self.whisper_warmed {
                    "🎤 listening — say 'computer, …' or address me directly"
                } else {
                    "🎤 arming — loading whisper (first time takes a few seconds), then say 'computer, …'"
                },
            );
            self.whisper_warmed = true;
        } else {
            button.set_text(cx, "🎤");
            self.push_line(cx, "🎤 mic off");
        }
        self.sync_voice_state(cx);
    }

    /// Start/stop the VoiceWave capture and lazily spawn the gate worker.
    fn sync_voice_state(&mut self, cx: &mut Cx) {
        if self.mic_on && self.voice_gate.is_none() {
            self.voice_gate = Some(VoiceGate::new());
        }
        let wave = self.ui.voice_wave(cx, ids!(mic_wave));
        wave.set_enabled(cx, self.mic_on);
        log!(
            "voice: mic toggle requested={} widget_enabled={}",
            self.mic_on,
            wave.is_enabled()
        );
    }

    /// Last N user/assistant lines for the gate's in-context judgement.
    fn recent_dialog(&self) -> Vec<String> {
        self.chat
            .entries
            .iter()
            .filter_map(|e| match e.kind {
                EntryKind::User => Some(format!("user: {}", e.text)),
                EntryKind::Assistant if e.trip.is_none() => {
                    Some(format!("assistant: {}", e.text))
                }
                _ => None,
            })
            .rev()
            .take(voice::RECENT_DIALOG_LINES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    fn on_gate_result(&mut self, cx: &mut Cx, result: GateResult) {
        match result {
            GateResult::Ready { secs } => {
                self.push_entry(
                    cx,
                    EntryKind::Info,
                    &format!("voice gate ready (4B loaded + warmed in {secs:.1}s)"),
                );
                self.ai_gate_ready = true;
                self.update_ai_status(cx);
                return;
            }
            GateResult::Send { raw, instruction } => {
                let _ = &raw;
                self.push_entry(cx, EntryKind::Info, "→ directed");
                if self.busy && Self::is_override_command(&instruction) {
                    // "ok nevermind, stop, go do X now" — cancel the turn;
                    // send the rest unless it was a bare stop.
                    self.interrupt_turn(cx);
                    if !Self::is_pure_stop(&instruction) {
                        self.send_user_prompt(cx, &instruction);
                    }
                } else if self.busy {
                    self.push_entry(cx, EntryKind::Info, "(queued until current turn ends)");
                    self.voice_queue.push(instruction);
                } else if Self::is_pure_stop(&instruction) {
                    // Nothing running — just quiet the voice.
                    if let Some(speech) = &mut self.speech {
                        speech.stop();
                    }
                } else {
                    self.send_user_prompt(cx, &instruction);
                }
            }
            GateResult::Skip { raw, reason } => {
                let _ = &raw;
                self.push_entry(cx, EntryKind::Info, &format!("— skipped ({})", reason.trim()));
            }
        }
    }

    fn on_ddg_event(&mut self, cx: &mut Cx, event: DdgEvent) {
        match event {
            DdgEvent::Thumb(slot, data) => {
                let image_ids: [&[LiveId]; ddg::MAX_CARDS] =
                    [ids!(img_0), ids!(img_1), ids!(img_2), ids!(img_3)];
                if let Some(id) = image_ids.get(slot) {
                    let image = self.ui.image(cx, id);
                    if image.load_image_from_data(cx, &data).is_ok() {
                        self.ui.view(cx, ids!(images_row)).set_visible(cx, true);
                        self.ui.view(cx, ids!(images_row)).redraw(cx);
                    }
                }
            }
            DdgEvent::Done(tool_use_id, digest, is_error) => {
                self.push_entry(
                    cx,
                    if is_error { EntryKind::Tool } else { EntryKind::Assistant },
                    &if is_error { format!("⚠ {digest}") } else { digest.clone() },
                );
                if let Some(tool_use_id) = tool_use_id {
                    if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
                        agent.send_tool_result(cx, session, &tool_use_id, &digest, is_error);
                    }
                }
            }
        }
    }

    fn run_tool(&mut self, cx: &mut Cx, tool_use_id: &str, name: &str, input: &str) {
        let compact: String = input.chars().take(120).collect();
        self.push_entry(cx, EntryKind::Tool, &format!("⚙ {name} {compact}"));
        // Loop breakers: greedy decoding can wedge the dispatcher into
        // re-issuing the same call forever (seen: identical geo_search
        // spam). Repeats and over-budget turns get a corrective tool
        // result instead of execution.
        self.tool_rounds += 1;
        let this_call = (name.to_string(), input.to_string());
        let repeated = self.last_tool_call.as_ref() == Some(&this_call);
        self.last_tool_call = Some(this_call);
        if repeated || self.tool_rounds > 10 {
            let nudge = if repeated {
                "Error: identical tool call repeated — you already have this result. \
                 Do NOT call this tool again; answer the user now with what you know."
            } else {
                "Error: tool budget for this request is exhausted. \
                 Stop calling tools and answer the user now with what you have."
            };
            self.push_entry(cx, EntryKind::Info, "⛔ tool loop broken");
            if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
                agent.send_tool_result(cx, session, tool_use_id, nudge, true);
            }
            return;
        }
        // images_search runs async over cx.http_request; the tool result is
        // sent when the thumbnails land.
        if name == "images_search" {
            let query = broker::parse_field(input, "query").unwrap_or_default();
            match self.ddg.start(cx, &query, Some(tool_use_id.to_string())) {
                Ok(()) => {
                    self.push_entry(cx, EntryKind::Info, &format!("🔎 images: {query}"));
                }
                Err(error) => {
                    if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
                        agent.send_tool_result(cx, session, tool_use_id, &error, true);
                    }
                }
            }
            return;
        }
        // cloud_ask escalates asynchronously through the Claude side-agent;
        // the tool result is sent when that turn completes.
        if name == "cloud_ask" && self.cloud_agent.is_some() && self.pending_cloud.is_none() {
            let question = broker::parse_question(input);
            self.push_line(cx, "☁ asking cloud…");
            let (cloud, cloud_session) = (
                self.cloud_agent.as_mut().unwrap(),
                self.cloud_session.unwrap(),
            );
            cloud.send_prompt(cx, cloud_session, &question);
            self.pending_cloud = Some((tool_use_id.to_string(), String::new()));
            return;
        }
        let result = self.execute_tool(cx, name, input);
        let (text, is_error) = match result {
            Ok(t) => (t, false),
            Err(e) => (e, true),
        };
        if is_error {
            self.push_entry(cx, EntryKind::Tool, &format!("⚠ {text}"));
        } else if matches!(name, "route_plan" | "route_add_stop" | "route_remove_stop") {
            self.push_trip_entry(cx);
        }
        if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
            agent.send_tool_result(cx, session, tool_use_id, &text, is_error);
        }
    }

    /// Redraw route/markers/camera from the current TripModel (used by the
    /// `>` re-apply button; tools do this themselves).
    fn resync_trip_display(&mut self, cx: &mut Cx) {
        let map = self.ui.map_view(cx, ids!(map));
        let mut nav_action = None;
        let mut tool_ctx = ToolCtx {
            cx,
            map: &map,
            trip: &mut self.trip,
            nav: self.nav.as_mut(),
            radar: self.radar.as_ref(),
            markers: &mut self.markers,
            position: self.position.as_ref().map(|p| (p.lon, p.lat)),
            layers: &mut self.layers,
            leg_routes: &mut self.leg_routes,
            nav_action: &mut nav_action,
        };
        tools::map::sync_trip_display(&mut tool_ctx, true);
    }

    /// Mirror LayerState into the popover checkboxes (agent tools and the
    /// UI share one state).
    fn sync_layer_checkboxes(&mut self, cx: &mut Cx) {
        let overlay_ids = [
            ids!(layer_chargers),
            ids!(layer_transit),
            ids!(layer_nature),
            ids!(layer_districts),
            ids!(layer_buildings),
            ids!(layer_demographics),
        ];
        for (i, id) in overlay_ids.iter().enumerate() {
            self.ui
                .check_box(cx, *id)
                .set_active(cx, self.layers.overlay_on[i], Animate::No);
        }
        self.ui
            .check_box(cx, ids!(layer_rain))
            .set_active(cx, self.layers.rain, Animate::No);
        self.ui
            .check_box(cx, ids!(layer_wind))
            .set_active(cx, self.layers.wind, Animate::No);
        self.ui
            .check_box(cx, ids!(layer_terrain))
            .set_active(cx, self.layers.terrain, Animate::No);
        self.ui
            .check_box(cx, ids!(tilt_check))
            .set_active(cx, self.layers.tilt_shift, Animate::No);
        let warp_on = self.ui.map_view(cx, ids!(map)).space_warp();
        self.ui
            .check_box(cx, ids!(warp_check))
            .set_active(cx, warp_on, Animate::No);
        self.ui
            .check_box(cx, ids!(theme_night))
            .set_active(cx, self.layers.theme == 1, Animate::No);
        self.ui
            .check_box(cx, ids!(theme_circuit))
            .set_active(cx, self.layers.theme == 2, Animate::No);
    }

    /// Restyle the app chrome to match the map theme (light vs night/circuit).
    fn apply_ui_theme(&mut self, cx: &mut Cx) {
        let dark = self.layers.theme != 0;
        self.chat.dark = dark;
        let panel_bg = if dark { vec4(0.075, 0.085, 0.105, 0.95) } else { vec4(1.0, 1.0, 1.0, 0.95) };
        let panel_border = if dark { vec4(1.0, 1.0, 1.0, 0.10) } else { vec4(0.0, 0.0, 0.0, 0.13) };
        let text_main = if dark { vec4(0.87, 0.90, 0.93, 1.0) } else { vec4(0.13, 0.19, 0.22, 1.0) };
        let text_dim = if dark { vec4(0.52, 0.56, 0.61, 1.0) } else { vec4(0.13, 0.19, 0.24, 1.0) };

        // Glass panels: theme via tint (they sample the gauss backdrop).
        let (tint, tint_alpha) = if dark {
            (vec4(0.04, 0.06, 0.09, 1.0), 0.42f32)
        } else {
            (vec4(0.97, 0.98, 1.0, 1.0), 0.30f32)
        };
        let _ = (panel_bg, panel_border);
        for id in [ids!(assistant_panel), ids!(layers_panel)] {
            let mut panel = self.ui.widget(cx, id);
            script_apply_eval!(cx, panel, {
                draw_bg +: {
                    tint_color: #(tint)
                    tint_alpha: #(tint_alpha)
                }
            });
        }
        for id in [ids!(header_label), ids!(intro_label), ids!(status_label)] {
            let mut label = self.ui.label(cx, id);
            let color = if id == ids!(header_label) { text_main } else { text_dim };
            script_apply_eval!(cx, label, {
                draw_text +: {
                    color: #(color)
                }
            });
        }
        let check_ids = [
            ids!(layer_rain),
            ids!(layer_wind),
            ids!(layer_terrain),
            ids!(tilt_check),
            ids!(layer_chargers),
            ids!(layer_transit),
            ids!(layer_nature),
            ids!(layer_districts),
            ids!(layer_buildings),
            ids!(layer_demographics),
            ids!(theme_night),
            ids!(theme_circuit),
        ];
        // Every interaction state: hover/focus/down otherwise keep their
        // DSL (light-panel) colors and go unreadable on dark.
        let text_hot = if dark { vec4(1.0, 1.0, 1.0, 1.0) } else { vec4(0.0, 0.0, 0.0, 1.0) };
        for id in check_ids {
            let mut check = self.ui.check_box(cx, id);
            script_apply_eval!(cx, check, {
                draw_text +: {
                    color: #(text_main)
                    color_active: #(text_main)
                    color_hover: #(text_hot)
                    color_down: #(text_hot)
                    color_focus: #(text_main)
                }
            });
        }
        let input_text = if dark { vec4(0.88, 0.91, 0.94, 1.0) } else { vec4(0.09, 0.13, 0.16, 1.0) };
        let placeholder = if dark { vec4(0.50, 0.54, 0.58, 1.0) } else { vec4(0.42, 0.47, 0.52, 1.0) };
        let mut input = self.ui.text_input(cx, ids!(prompt_input));
        script_apply_eval!(cx, input, {
            draw_text +: {
                color: #(input_text)
                color_hover: #(input_text)
                color_focus: #(input_text)
                color_empty: #(placeholder)
                color_empty_hover: #(placeholder)
                color_empty_focus: #(placeholder)
            }
        });
        self.ui.redraw(cx);
    }

    /// Push the layer/theme state to the MapView; lazily starts workers.
    fn apply_layers(&mut self, cx: &mut Cx) {
        self.sync_layer_checkboxes(cx);
        self.apply_ui_theme(cx);
        let map = self.ui.map_view(cx, ids!(map));
        map.set_overlay_paths(cx, &self.layers.overlay_paths());
        map.set_theme(cx, self.layers.theme);

        let bbox = nav_data::radar_display_bbox();
        if self.layers.rain {
            if let Some(radar) = &self.radar {
                if !radar.display_frames.is_empty() {
                    map.set_rain_frames(
                        cx,
                        radar.display_frames.clone(),
                        radar.display_width,
                        radar.display_height,
                        bbox,
                    );
                    map.set_rain_now_hires(cx, radar.now_hires.clone());
                }
            }
        } else {
            map.set_rain_frames(cx, Vec::new(), 0, 0, bbox);
        }

        if self.layers.wind {
            if !self.layers.wind_worker_started {
                self.layers.wind_worker_started = true;
                layers::start_wind_worker(self.wind_rx.sender());
            }
            if let Some(update) = &self.layers.wind_cache {
                map.set_wind_field(
                    cx,
                    update.nx,
                    update.ny,
                    update.u.clone(),
                    update.v.clone(),
                    update.bbox,
                );
            }
        } else {
            map.set_wind_field(cx, 0, 0, Vec::new(), Vec::new(), (0.0, 0.0, 0.0, 0.0));
        }

        if self.layers.terrain {
            if self.layers.terrain_tx.is_none() {
                self.layers.terrain_tx = Some(layers::start_terrain_worker(self.terrain_rx.sender()));
            }
            self.layers.last_terrain_key = None;
            layers::request_terrain(cx, &map, &mut self.layers);
        } else if self.layers.terrain_tx.is_some() {
            map.set_terrain_overlay(cx, TerrainOverlayData::default());
        }
    }
}

#[cfg(feature = "native")]
impl MatchEvent for App {
    // The test-map download rides the platform's HTTP stack, which reports
    // progress as the body streams. (DDG image search reads the same
    // responses through its own state machine and ignores ids that are not
    // its own, so both can listen.)
    fn handle_http_progress(&mut self, cx: &mut Cx, request_id: LiveId, progress: &HttpProgress) {
        if self.testmap.handle_http_progress(request_id, progress) {
            self.refresh_testmap_ui(cx);
        }
    }

    fn handle_http_response(&mut self, cx: &mut Cx, request_id: LiveId, response: &HttpResponse) {
        if self.testmap.handle_http_response(request_id, response) {
            self.refresh_testmap_ui(cx);
        }
    }

    fn handle_http_request_error(&mut self, cx: &mut Cx, request_id: LiveId, err: &HttpError) {
        if self.testmap.handle_http_error(request_id, &err.message) {
            self.refresh_testmap_ui(cx);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.testmap.is_active() {
            if self.testmap.can_start() && self.ui.button(cx, ids!(testmap_start)).clicked(actions)
            {
                self.testmap.start(cx);
                self.refresh_testmap_ui(cx);
            }
            if self.ui.button(cx, ids!(testmap_dismiss)).clicked(actions) {
                self.testmap.dismiss();
                self.refresh_testmap_ui(cx);
            }
        }
        for panel_action in self.panel.actions(cx, &self.ui, actions) {
            if let PanelAction::Search(text) = panel_action {
                self.ui.text_input(cx, ids!(prompt_input)).set_text(cx, "");
                self.send_user_prompt(cx, &text);
            }
        }
        if self.ui.button(cx, ids!(mic_button)).clicked(actions) {
            self.toggle_mic(cx);
        }
        if self.ui.button(cx, ids!(speaker_button)).clicked(actions) {
            if let Some(speech) = &mut self.speech {
                let muted = !speech.is_muted();
                speech.set_muted(muted);
                self.ui
                    .button(cx, ids!(speaker_button))
                    .set_text(cx, if muted { "🔇" } else { "🔊" });
                self.push_line(cx, if muted { "🔇 voice off" } else { "🔊 voice on" });
            }
        }
        // Endpointed transcripts from the mic → attention gate.
        let mic_uid = self.ui.widget(cx, ids!(mic_wave)).widget_uid();
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            if action.widget_uid != mic_uid {
                continue;
            }
            match action.cast::<VoiceWaveAction>() {
                VoiceWaveAction::VoiceActivity(active) => {
                    // Barge-in: the user talking mutes the assistant NOW —
                    // they are not waiting for it to finish. (Assistant echo
                    // can also trigger this; acceptable — the gate still
                    // decides what the words meant.)
                    if active {
                        if let Some(speech) = &mut self.speech {
                            if speech.is_speaking() {
                                speech.stop();
                            }
                        }
                    }
                }
                VoiceWaveAction::InjectText(text) => {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        // Raw transcript, immediately visible; the gate's
                        // verdict follows as its own line. Assistant echo is
                        // NOT filtered here — the gate skips non-directed
                        // lines, and blocking the mic while speaking made
                        // interruptions impossible.
                        self.push_entry(cx, EntryKind::Info, &format!("🎤 “{text}”"));
                        if let Some(gate) = &self.voice_gate {
                            gate.submit(text, self.recent_dialog());
                        } else {
                            self.push_entry(cx, EntryKind::Info, "(gate still loading)");
                        }
                    }
                }
                VoiceWaveAction::RecordVoice(on) => {
                    if on != self.mic_on {
                        // widget-side toggle (click on the wave / F1)
                        self.mic_on = on;
                        self.sync_voice_state(cx);
                    }
                }
                _ => {}
            }
        }
        if self.ui.button(cx, ids!(layers_button)).clicked(actions) {
            self.layers_panel_open = !self.layers_panel_open;
            self.ui
                .widget(cx, ids!(layers_panel))
                .set_visible(cx, self.layers_panel_open);
        }
        if self.ui.button(cx, ids!(assistant_button)).clicked(actions) {
            self.assistant_panel_open = !self.assistant_panel_open;
            self.ui
                .widget(cx, ids!(assistant_panel))
                .set_visible(cx, self.assistant_panel_open);
        }
        let layer_checks: [(&[LiveId], &str); 10] = [
            (ids!(layer_rain), "rain"),
            (ids!(layer_wind), "wind"),
            (ids!(layer_terrain), "terrain"),
            (ids!(tilt_check), "tiltshift"),
            (ids!(layer_chargers), "chargers"),
            (ids!(layer_transit), "transit"),
            (ids!(layer_nature), "nature"),
            (ids!(layer_districts), "districts"),
            (ids!(layer_buildings), "buildings_age"),
            (ids!(layer_demographics), "demographics"),
        ];
        for (id, name) in layer_checks {
            if let Some(on) = self.ui.check_box(cx, id).changed(actions) {
                let _ = self.layers.set_layer(name, on);
                self.layers.dirty = false;
                self.apply_layers(cx);
            }
        }
        // The Inception fold: a live rendering mode on the map itself, not
        // a data layer — MapView owns the tween and the close-3D gating
        // (the setting remembers intent while the camera is elsewhere).
        // Grayed = inert: CheckBox still fires Change while disabled, so
        // outside the regime the mark snaps back and nothing arms.
        if let Some(on) = self.ui.check_box(cx, ids!(warp_check)).changed(actions) {
            let map = self.ui.map_view(cx, ids!(map));
            if map.space_warp_available() {
                map.set_space_warp(cx, on);
            } else {
                self.ui
                    .check_box(cx, ids!(warp_check))
                    .set_active(cx, map.space_warp(), Animate::No);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(theme_night)).changed(actions) {
            let _ = self.layers.set_theme_name(if on { "night" } else { "light" });
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(theme_circuit)).changed(actions) {
            let _ = self.layers.set_theme_name(if on { "circuit" } else { "light" });
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        let map = self.ui.map_view(cx, ids!(map));
        if let Some(id) = map.marker_clicked(actions) {
            if let Some(name) = self.markers.name_of(id) {
                let name = name.to_string();
                self.push_line(cx, &format!("map: {name}"));
            }
        }
        if let Some((lon, lat)) = map.long_pressed(actions) {
            self.push_line(cx, &format!("map: long-press at {lon:.5}, {lat:.5}"));
        }
        if map.viewport_changed(actions).is_some() && self.layers.terrain {
            layers::request_terrain(cx, &map, &mut self.layers);
        }
        // '>' on a trip row: re-apply that snapshot.
        let list = self.ui.portal_list(cx, ids!(list));
        for (item_id, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(apply_btn)).clicked(actions) {
                let re_applied = self
                    .chat
                    .entries
                    .get(item_id)
                    .and_then(|e| e.trip)
                    .and_then(|i| self.chat.trips.get(i).cloned());
                if let Some(trip) = re_applied {
                    self.trip = trip;
                    self.resync_trip_display(cx);
                    self.drive_log.log_trip(&self.trip.digest());
                    self.push_line(cx, "↩ trip re-applied");
                }
            }
        }
        if let Some((lon, lat, info)) = map.pin_tapped(actions) {
            let summary: Vec<String> = info
                .iter()
                .filter(|(k, _)| matches!(k.as_str(), "name" | "operator" | "max_kw" | "kind" | "city"))
                .map(|(k, v)| format!("{k}: {v}"))
                .collect();
            let text = if summary.is_empty() {
                format!("map: pin at {lon:.5},{lat:.5}")
            } else {
                format!("map: pin — {}", summary.join(", "))
            };
            self.push_line(cx, &text);
        }
    }
}

#[cfg(feature = "native")]
impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // Whisper stays on the F16 default (ggml-large-v3-turbo.bin): the
        // voice Metal library has no quantized matmul kernels, so q5_0/q8_0
        // models fail every GPU op. Port the kernels before re-quantizing.
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        crate::side_panel::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ensure_started(cx);
        // The Space-warp row tracks the camera live: grayed (but visible,
        // so it stays discoverable) outside the near-first-person regime,
        // re-enabled the moment tilt + zoom qualify. Cached so the
        // animator only toggles on transitions.
        let warp_avail = self.ui.map_view(cx, ids!(map)).space_warp_available();
        if self.warp_check_disabled != Some(!warp_avail) {
            self.warp_check_disabled = Some(!warp_avail);
            self.ui
                .check_box(cx, ids!(warp_check))
                .set_disabled(cx, !warp_avail);
        }
        match event {
            Event::Shutdown => {
                self.drive_log.close();
            }
            Event::AudioDevices(devices) => {
                // TTS playback device; mic input selection lives inside the
                // VoiceWave's own handler.
                cx.use_audio_outputs(&devices.default_output());
            }
            Event::LocationUpdate(fix) => {
                self.drive_log.log_fix(fix);
                let map = self.ui.map_view(cx, ids!(map));
                map.set_puck(
                    cx,
                    Some(MapPuck::new(fix.lon, fix.lat, fix.heading_deg, fix.accuracy_m)),
                );
                if !self.had_first_fix {
                    self.had_first_fix = true;
                    map.fly_to(cx, fix.lon, fix.lat, 14.0);
                    self.push_line(
                        cx,
                        &format!("gps: fix acquired (±{:.0}m)", fix.accuracy_m),
                    );
                }
                self.position = Some(fix.clone());
                // Live turn-by-turn: feed the fix into the session.
                let tick = self.active_nav.as_mut().and_then(|nav| {
                    if nav.simulate {
                        return None;
                    }
                    let now = std::time::Instant::now();
                    let dt = nav
                        .sim_last_tick
                        .map(|last| now.duration_since(last).as_secs_f64())
                        .unwrap_or(1.0)
                        .clamp(0.05, 5.0);
                    nav.sim_last_tick = Some(now);
                    let pos = makepad_map_nav::geo::LonLat {
                        lon: fix.lon,
                        lat: fix.lat,
                    };
                    Some(nav.feed(pos, fix.heading_deg, dt))
                });
                if let Some(tick) = tick {
                    self.apply_nav_tick(cx, tick);
                }
            }
            Event::NetworkResponses(responses) => {
                let mut ddg_events = Vec::new();
                for response in responses.iter() {
                    ddg_events.extend(self.ddg.handle_response(cx, response));
                }
                for event in ddg_events {
                    self.on_ddg_event(cx, event);
                }
            }
            Event::LocationError(error) => {
                let text = match error {
                    LocationErrorEvent::PermissionDenied => {
                        "gps: permission denied — using map center as position".to_string()
                    }
                    LocationErrorEvent::Unavailable(msg) => format!("gps: unavailable ({msg})"),
                };
                self.push_line(cx, &text);
            }
            _ => (),
        }
        // Esc closes the assistant popover from anywhere, regardless of
        // which widget currently owns key focus.
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::Escape && self.assistant_panel_open {
                self.assistant_panel_open = false;
                self.ui
                    .widget(cx, ids!(assistant_panel))
                    .set_visible(cx, false);
            }
        }
        if self.nav_frame.is_event(event).is_some() {
            self.tick_nav_sim(cx);
        }
        if let Some(gate) = &mut self.voice_gate {
            let results = gate.poll();
            for result in results {
                self.on_gate_result(cx, result);
            }
        }
        if !self.busy && !self.voice_queue.is_empty() {
            let next = self.voice_queue.remove(0);
            self.send_user_prompt(cx, &next);
        }
        // The provisioner owns polling and adoption of the native bake.
        let map = self.ui.map_view(cx, ids!(map));
        let provisioner_update = self.testmap.handle_event(cx, &map);
        if let Some(basename) = provisioner_update.nav_basename {
            nav_data::start_nav_load(self.nav_rx.sender(), basename);
            self.push_entry(
                cx,
                EntryKind::Info,
                "test map ready: Amsterdam tiles, routing graph and search index",
            );
        }
        if provisioner_update.changed {
            self.refresh_testmap_ui(cx);
        }
        while let Ok(load) = self.nav_rx.try_recv() {
            match load {
                NavLoad::Ready { data, stats } => {
                    self.nav = Some(*data);
                    self.push_entry(cx, EntryKind::Info, &stats);
                }
                NavLoad::Failed { error } => {
                    self.set_status(cx, &format!("nav data failed: {error}"));
                }
            }
        }
        while let Ok(radar) = self.radar_rx.try_recv() {
            self.radar = Some(radar);
            if self.layers.rain {
                self.apply_layers(cx);
            }
        }
        while let Ok(update) = self.wind_rx.try_recv() {
            self.layers.wind_cache = Some(update);
            if self.layers.wind {
                self.apply_layers(cx);
            }
        }
        while let Ok(update) = self.terrain_rx.try_recv() {
            if self.layers.terrain {
                self.ui.map_view(cx, ids!(map)).set_terrain_overlay(
                    cx,
                    TerrainOverlayData {
                        texels: update.texels,
                        width: update.width,
                        height: update.height,
                        elev_texels: update.elev_texels,
                        elev: update.elev,
                        elev_width: update.elev_width,
                        elev_height: update.elev_height,
                        bbox: update.bbox,
                    },
                );
            }
        }
        if let Some(mut agent) = self.agent.take() {
            let events = agent.handle_event(cx, event);
            self.agent = Some(agent);
            for agent_event in events {
                self.on_agent_event(cx, agent_event);
            }
        }
        if let Some(mut cloud) = self.cloud_agent.take() {
            let events = cloud.handle_event(cx, event);
            self.cloud_agent = Some(cloud);
            for cloud_event in events {
                self.on_cloud_event(cx, cloud_event);
            }
        }
        self.match_event(cx, event);
        // Feed the tilt-shift layer: on when the checkbox is set AND the
        // map is actually tilted (strength ramps with tilt angle).
        self.chat.tilt_shift_on = self.layers.tilt_shift;
        let tilt = self.ui.map_view(cx, ids!(map)).tilt() as f32;
        self.chat.tilt_strength = ((tilt - 5.0) / 50.0).clamp(0.0, 1.0);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.chat));
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    /// 19:00 through 06:59 is night; 07:00 through 18:59 is light — the
    /// boundary hours (6/7/18/19) are the ones a fencepost bug would miss.
    #[test]
    fn theme_for_hour_matches_civil_twilight_rule() {
        assert_eq!(theme_name_for_hour(19), "night");
        assert_eq!(theme_name_for_hour(7), "light");
        assert_eq!(theme_name_for_hour(6), "night");
        assert_eq!(theme_name_for_hour(18), "light");
        assert_eq!(theme_name_for_hour(0), "night");
        assert_eq!(theme_name_for_hour(23), "night");
        assert_eq!(theme_name_for_hour(12), "light");
        assert_eq!(theme_name_for_hour(20), "night");
    }

    #[test]
    fn utc_offset_parses_sign_and_magnitude() {
        assert_eq!(parse_utc_offset("+0200"), 7200);
        assert_eq!(parse_utc_offset("-0730"), -27000);
        assert_eq!(parse_utc_offset("+0000"), 0);
        assert_eq!(parse_utc_offset("garbage"), 0);
    }

    #[test]
    fn http_archive_configuration_keeps_detail_and_local_sidecars() {
        let config = TileSourceConfig::HttpArchive {
            root_url: "https://tiles.example/world.mkmap".to_string(),
            detail_root_url: "https://tiles.example/detail.mkmap".to_string(),
            overlay_mbtiles_paths: "local/maps/ocean.mbtiles".to_string(),
            bridge_dz_path: "local/maps/bridge-dz.mbtiles".to_string(),
        };
        let TileSourceConfig::HttpArchive {
            detail_root_url,
            overlay_mbtiles_paths,
            bridge_dz_path,
            ..
        } = config
        else {
            unreachable!()
        };
        assert_eq!(detail_root_url, "https://tiles.example/detail.mkmap");
        assert_eq!(overlay_mbtiles_paths, "local/maps/ocean.mbtiles");
        assert_eq!(bridge_dz_path, "local/maps/bridge-dz.mbtiles");
    }
}
