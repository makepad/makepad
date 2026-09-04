//! `FlowCanvas`: the graph of one instance — rounded node cards around
//! mounted faces, port circles with the port-type icon, bezier wires in one
//! `DrawVector` batch, drag-to-move, drag-to-connect, a continuously zoomable
//! camera (0.25×–3×) and a dark checkerboard behind it all.
//!
//! How the zoom works (decision of 2026-09-03, replacing the three discrete
//! sizes): everything the canvas and the faces draw is laid out ONCE in
//! canvas units — the node at world `(x, y)` sits at local `(ORIGIN + x,
//! ORIGIN + y)` inside the canvas's own draw list — and the camera is the
//! draw list's `view_transform` (scale + translate), so text and pictures
//! scale on the GPU and nothing re-flows. Two platform facts shape the code:
//!
//! * per-instance clipping (`draw_clip`) is written by the align-list walk in
//!   PRE-transform units and intersected with every ancestor, so a
//!   transformed child list under a clipped window body could never show
//!   content that lies outside the window in local units. The canvas draws
//!   inside `begin_root_turtle` / `end_pass_sized_turtle` (the mechanism
//!   popups use): its range gets a fresh clip context, and the one clip it
//!   pushes is the inverse-transformed view rect, which after the transform is
//!   exactly the view. `LOCAL_ORIGIN` keeps every local coordinate positive
//!   inside that root.
//! * `Event::hits` compares the raw pointer position with those local rects,
//!   so the faces receive a cloned event whose positions went through the
//!   inverse camera (the host uses [`Camera`] to remap face events); the canvas's own
//!   hit tests (ports, cards) convert the other way. No platform change.

use crate::model::{
    CanvasStyles, CompatiblePorts, GraphView as Graph, NodeFacesScope, NodeStyle,
    NodeView as Node, PortStyle, FIRST_AT, NODE_WIDTH,
};
use crate::wire_route::{
    self, Obstacle, Point, PortSide, RouteKind, RouteStyle, WireMode, WireRoute,
};
use makepad_widgets::fab_controls::FabValueInput;
use makepad_widgets::makepad_draw::DrawSvg;
use makepad_widgets::makepad_platform::event::TouchState;
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use std::any::TypeId;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Local-space offset of the world origin: keeps every local coordinate
/// positive inside the root turtle's `(0, 0)..ROOT_SIZE` clip.
pub const LOCAL_ORIGIN: f64 = 32768.0;
const ROOT_SIZE: f64 = 65536.0;

const CARD_RADIUS: f32 = 16.0;
const CARD_OUTLINE_PX: f64 = 2.0;
const CARD_HOVER_OUTLINE_ALPHA: f32 = 0.42;
/// The icon-and-title row above every card.
const LABEL_H: f64 = 26.0;
/// Top inset occupied by the card's in-body header/chrome before its ports.
const CARD_HEADER_H: f64 = 14.0;
const PORT_ROW_H: f64 = 24.0;
const PORT_R: f64 = 11.0;
/// The disc is a slight oval along the flow axis (`PORT_RX` across,
/// `PORT_R` tall) so the type icon reads centred beside the point.
const PORT_RX: f64 = 13.0;
/// The label starts this far past the oval's edge.
const PORT_LABEL_GAP: f64 = 7.0;
/// An output disc ends in a point on its cable side, an input disc has a
/// small notch the point would fit into: the shape reads the flow direction.
const PORT_TIP: f64 = 4.5;
const PORT_TIP_HALF_ANGLE: f64 = 0.72;
const PORT_DENT: f64 = 3.0;
const PORT_NOTCH_HALF_ANGLE: f64 = 0.40;
/// The type icon shifts a little toward the point (outputs) or away from
/// the notch (inputs) so it sits visually centred in the shape.
const PORT_ICON_SHIFT_OUT: f64 = 1.5;
const PORT_ICON_SHIFT_IN: f64 = -0.75;
const PORT_HIT_R: f64 = 16.0;
const WIRE_HIT_PX: f64 = 6.0;
const CARD_PAD: f64 = 14.0;
const PROGRESS_H: f64 = 4.0;
const DRAG_THRESHOLD: f64 = 3.0;
const ZOOM_MIN: f64 = 0.25;
const ZOOM_MAX: f64 = 3.0;
const GRID_CELL: f64 = 24.0;
/// The checker never shows cells smaller than this on screen; past it the
/// spacing doubles ("hops up a level").
const GRID_MIN_PX: f64 = 14.0;
const FIT_MARGIN: f64 = 32.0;
const MIN_NODE_WIDTH: f64 = 160.0;
const MIN_TEXT_LINE_H: f64 = 18.0;
const RESIZE_GRIP: f64 = 18.0;
const FLIP_SECONDS: f64 = 0.2;
const AUTO_FLIP_RATIO: f64 = 0.8;
const AUTO_FLIP_SETTLE_SECONDS: f64 = 0.25;
const AUTO_FLIP_MAX_PASSES: usize = 3;
const CROSSING_COST: f64 = 400.0;
const BEND_COST: f64 = 120.0;
const LOOP_COST: f64 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct CardOutlineGeometry {
    outer_rect: Rect,
    radius: f32,
    stroke_width: f32,
}

/// The shader strokes the body's own SDF shape. Its outer edge is therefore
/// exactly this much larger than the body while retaining `CARD_RADIUS`.
fn card_outline_geometry(body_rect: Rect, zoom: f64) -> CardOutlineGeometry {
    let stroke_width = CARD_OUTLINE_PX / zoom.max(0.01);
    CardOutlineGeometry {
        outer_rect: Rect {
            pos: body_rect.pos - dvec2(stroke_width, stroke_width),
            size: body_rect.size + dvec2(stroke_width * 2.0, stroke_width * 2.0),
        },
        radius: CARD_RADIUS,
        stroke_width: stroke_width as f32,
    }
}

fn is_interactive_face_type(type_id: TypeId) -> bool {
    type_id == TypeId::of::<TextInput>()
        || type_id == TypeId::of::<FabValueInput>()
        || type_id == TypeId::of::<DropDown>()
        || type_id == TypeId::of::<DropDown2>()
        || type_id == TypeId::of::<Button>()
        || type_id == TypeId::of::<FoldHeader>()
        || type_id == TypeId::of::<FoldButton>()
        || type_id == TypeId::of::<Slider>()
        || type_id == TypeId::of::<CheckBox>()
        || type_id == TypeId::of::<RadioButton>()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CardContentRect {
    rect: Rect,
    pad_top: f64,
    pad_bottom: f64,
}

fn card_content_rect(card: Rect, full_bleed: bool, port_rows: usize) -> CardContentRect {
    if full_bleed {
        return CardContentRect {
            rect: card,
            pad_top: 0.0,
            pad_bottom: 0.0,
        };
    }
    let pad_top = CARD_HEADER_H + port_rows as f64 * PORT_ROW_H;
    let pad_bottom = CARD_PAD;
    CardContentRect {
        rect: Rect {
            pos: card.pos + dvec2(CARD_PAD, pad_top),
            size: dvec2(
                (card.size.x - 2.0 * CARD_PAD).max(1.0),
                (card.size.y - pad_top - pad_bottom).max(1.0),
            ),
        },
        pad_top,
        pad_bottom,
    }
}

fn min_card_height(full_bleed: bool, port_rows: usize) -> f64 {
    if full_bleed {
        (CARD_RADIUS as f64 * 2.0).max(MIN_TEXT_LINE_H)
    } else {
        CARD_HEADER_H + port_rows as f64 * PORT_ROW_H + CARD_PAD + MIN_TEXT_LINE_H
    }
}

fn clamp_card_size(size: DVec2, full_bleed: bool, port_rows: usize) -> DVec2 {
    dvec2(
        size.x.max(MIN_NODE_WIDTH),
        size.y.max(min_card_height(full_bleed, port_rows)),
    )
}

fn declared_output_kind(node: &Node) -> Option<&str> {
    if node.type_name != "Output" {
        return None;
    }
    node.params
        .iter()
        .find_map(|(name, value)| (name == "type").then_some(value.as_str()))
        .or_else(|| node.inputs.first().map(|input| input.kind.as_str()))
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Standalone consumers get the standard theme through these fallbacks.
    // A host may replace the flow tokens before defining its canvas style.
    mod.theme.flow_grid_a = theme.color_bg_app
    mod.theme.flow_grid_b = theme.color_bg_container
    mod.theme.flow_surface = theme.color_bg_container
    mod.theme.flow_surface_hover = theme.color_bg_highlight
    mod.theme.flow_edge = theme.color_bevel
    mod.theme.flow_shadow = theme.color_shadow
    mod.theme.flow_text = theme.color_text
    mod.theme.flow_text_muted = theme.color_text_meta
    mod.theme.flow_text_port = theme.color_label_outer
    mod.theme.flow_text_chip = theme.color_text
    mod.theme.flow_error = theme.color_error
    mod.theme.flow_accent = theme.color_highlight
    mod.theme.flow_highlight = theme.color_highlight
    mod.theme.flow_input = theme.color_highlight
    mod.theme.flow_success = theme.color_highlight
    mod.theme.flow_chat = theme.color_highlight
    mod.theme.flow_generation = theme.color_highlight
    mod.theme.flow_function = theme.color_highlight
    mod.theme.flow_http = theme.color_highlight
    mod.theme.flow_waiting = theme.color_highlight
    mod.theme.flow_port_text = theme.color_label_outer
    mod.theme.flow_port_image = theme.color_label_outer
    mod.theme.flow_port_audio = theme.color_label_outer
    mod.theme.flow_port_video = theme.color_label_outer
    mod.theme.flow_port_mesh = theme.color_label_outer
    mod.theme.flow_port_json = theme.color_label_outer
    mod.theme.flow_port_list = theme.color_label_outer
    mod.theme.flow_port_bytes = theme.color_label_outer
    mod.theme.flow_state_running = theme.color_highlight
    mod.theme.flow_state_idle = theme.color_text_meta
    mod.theme.flow_text_port_connected = theme.color_label_outer
    mod.theme.flow_text_port_open = theme.color_text_meta

    set_type_default() do #(DrawFlowGrid::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size + self.rect_pos - self.origin
            let c = floor(p / self.cell)
            let parity = fract((c.x + c.y) * 0.5) * 2.0
            return mix(self.color_a, self.color_b, parity)
        }
    }

    set_type_default() do #(DrawFlowCard::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: theme.flow_surface
        border_color: theme.flow_edge
        border_size: 1.0
        border_radius: 16.0
        outline_color: #0000
        outline_size: 0.0
        shadow_color: theme.flow_shadow
        shadow_radius: 12.0
        shadow_offset: vec2(0.0, 0.0)

        rect_size2: varying(vec2(0.0))
        rect_size3: varying(vec2(0.0))
        rect_pos2: varying(vec2(0.0))
        rect_shift: varying(vec2(0.0))
        sdf_rect_pos: varying(vec2(0.0))
        sdf_rect_size: varying(vec2(0.0))

        vertex: fn() {
            let min_offset = min(self.shadow_offset, vec2(0.0, 0.0))
            self.rect_size2 = self.rect_size + 2.0 * vec2(self.shadow_radius)
            self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
            self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
            self.sdf_rect_size = self.rect_size2
                - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
            self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius)
            self.rect_shift = -min_offset
            return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
        }

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
            sdf.box(
                self.sdf_rect_pos.x,
                self.sdf_rect_pos.y,
                self.sdf_rect_size.x,
                self.sdf_rect_size.y,
                self.border_radius
            )
            if sdf.shape > -1.0 {
                let m = self.shadow_radius
                let o = self.shadow_offset + self.rect_shift
                let v = GaussShadow.rounded_box_shadow(
                    vec2(m) + o,
                    self.rect_size2 + o,
                    self.pos * (self.rect_size3 + vec2(m)),
                    m * 0.5,
                    self.border_radius * 2.0
                )
                sdf.clear(self.shadow_color * v)
            }
            sdf.fill_keep(self.color)
            if self.border_size > 0.0 {
                sdf.stroke_keep(self.border_color, self.border_size)
            }
            if self.outline_size > 0.0 {
                sdf.stroke(self.outline_color, self.outline_size)
            }
            return sdf.result
        }
    }

    let KindIcon = mod.draw.DrawSvg{}

    mod.widgets.FlowPortStyle = #(PortStyle::script_component(vm))
    mod.widgets.FlowNodeStyle = #(NodeStyle::script_component(vm))
    mod.widgets.FlowCanvasStyles = #(CanvasStyles::script_component(vm))

    mod.widgets.FlowCanvasBase = #(FlowCanvas::register_widget(vm))
    mod.widgets.FlowCanvas = set_type_default() do mod.widgets.FlowCanvasBase{
        width: Fill
        height: Fill
        styles: mod.widgets.FlowCanvasStyles{}
        draw_bg +: {
            cell: 24.0
            origin: vec2(32768.0, 32768.0)
            color_a: theme.flow_grid_a
            color_b: theme.flow_grid_b
        }
        draw_card +: {}
        draw_title +: {
            text_style: theme.font_bold{font_size: 10.5}
            color: theme.flow_text
        }
        draw_meta +: {
            text_style: theme.font_regular{font_size: 9.5}
            color: theme.flow_text_muted
        }
        draw_port +: {
            text_style: theme.font_regular{font_size: 8.5}
            color: theme.flow_text_port
        }
        draw_chip +: {
            text_style: theme.font_bold{font_size: 8.5}
            color: theme.flow_text_chip
        }
        draw_error +: {
            text_style: theme.font_regular{font_size: 8.5}
            color: theme.flow_error
        }
        card_color: theme.flow_surface
        card_color_hover: theme.flow_surface_hover
        card_edge_color: theme.flow_edge
        accent_color: theme.flow_accent
        highlight_color: theme.flow_highlight
        color_input: theme.flow_input
        color_output: theme.flow_success
        color_chat: theme.flow_chat
        color_gen: theme.flow_generation
        color_fn: theme.flow_function
        color_http: theme.flow_http
        color_ask: theme.flow_waiting
        color_flow: theme.flow_text_port

        color_port_text: theme.flow_port_text
        color_port_image: theme.flow_port_image
        color_port_audio: theme.flow_port_audio
        color_port_video: theme.flow_port_video
        color_port_mesh: theme.flow_port_mesh
        color_port_json: theme.flow_port_json
        color_port_list: theme.flow_port_list
        color_port_bytes: theme.flow_port_bytes
        color_state_running: theme.flow_state_running
        color_state_done: theme.flow_success
        color_state_failed: theme.flow_error
        color_state_waiting: theme.flow_waiting
        color_state_inactive: theme.flow_text_muted
        color_state_idle: theme.flow_state_idle
        color_port_label_connected: theme.flow_text_port_connected
        color_port_label_open: theme.flow_text_port_open

        icon_check: KindIcon{ color: theme.flow_success svg: crate_resource("self:resources/icons/check.svg") }
        icon_alert: KindIcon{ color: theme.flow_error svg: crate_resource("self:resources/icons/alert.svg") }
        icon_clock: KindIcon{ color: theme.flow_waiting svg: crate_resource("self:resources/icons/clock.svg") }
    }
}

/// The dark checkerboard: two greys a few percent apart, cell size and
/// origin in local units so the pattern scales with the camera.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowGrid {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    cell: f32,
    #[live]
    origin: Vec2f,
    #[live]
    color_a: Vec4f,
    #[live]
    color_b: Vec4f,
}

/// One card, including its shadow, so the shadow follows the card's exact
/// transformed rectangle rather than a separately batched vector estimate.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowCard {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    outline_color: Vec4f,
    #[live]
    outline_size: f32,
    #[live]
    shadow_color: Vec4f,
    #[live]
    shadow_radius: f32,
    #[live]
    shadow_offset: Vec2f,
}

#[derive(Clone, Debug)]
pub enum CanvasEdit {
    Move {
        node: String,
        at: (f64, f64),
    },
    Resize {
        node: String,
        size: (f64, f64),
    },
    Flip {
        node: String,
    },
    Connect {
        from_node: String,
        from_port: String,
        to_node: String,
        to_port: String,
    },
    Disconnect {
        to_node: String,
        to_port: String,
    },
    Delete {
        node: String,
    },
    AddType {
        type_name: String,
        at: (f64, f64),
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Node(String),
    Edge {
        from_node: String,
        from_port: String,
        to_node: String,
        to_port: String,
    },
}

impl Selection {
    pub fn node(&self) -> Option<&str> {
        match self {
            Self::Node(node) => Some(node),
            Self::Edge { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum FlowCanvasAction {
    #[default]
    None,
    Select(Option<Selection>),
    Edit(CanvasEdit),
    /// A wire was dropped on empty canvas: open the palette filtered to
    /// types with an input of this type; `at` is the world position.
    OpenPalette {
        at: (f64, f64),
        from_node: String,
        from_port: String,
        ty: String,
    },
    /// The camera moved (pan or zoom); the app mirrors it in the toolbar.
    Camera {
        scale: f64,
    },
    /// Facing changes chosen by the router. The app coalesces these into one
    /// graph PUT on its 250 ms settle tick.
    AutoFlip(Vec<(String, bool)>),
}

#[derive(Clone, Debug)]
enum Drag {
    Pan {
        start: DVec2,
        origin: DVec2,
    },
    Node {
        index: usize,
        start: DVec2,
        origin: (f64, f64),
        moved: bool,
    },
    Resize {
        index: usize,
        start: DVec2,
        origin: (f64, f64),
        size: (f64, f64),
    },
    Wire {
        from: usize,
        from_port: usize,
        ty: String,
        pos: DVec2,
        target: Option<(usize, usize)>,
    },
}

/// A node's run state as the canvas draws it: the chip text is formatted
/// when the state changes, never per frame.
#[derive(Clone, Debug, Default)]
pub struct NodeStatus {
    pub state: String,
    pub permille: u16,
    /// A `node.progress` event arrived: the bar is determinate.
    pub has_progress: bool,
    pub stage: String,
    pub error: Option<String>,
    chip: String,
    /// The bar's eased on-screen fraction.
    shown: f64,
}

impl NodeStatus {
    pub fn new(state: &str, permille: u16, has_progress: bool, stage: &str, error: Option<String>) -> Self {
        let chip = match state {
            "running" if has_progress => format!("{}%", permille / 10),
            other => other.to_string(),
        };
        Self {
            state: state.to_string(),
            permille,
            has_progress,
            stage: stage.to_string(),
            error,
            chip,
            shown: 0.0,
        }
    }
}

/// The camera: pan in screen pixels, scale, and the view rect in window
/// space. Local = canvas units offset by `LOCAL_ORIGIN`.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub view: Rect,
    pub pan: DVec2,
    pub scale: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            view: Rect::default(),
            pan: dvec2(0.0, 0.0),
            scale: 1.0,
        }
    }
}

impl Camera {
    pub fn screen_to_local(&self, screen: DVec2) -> DVec2 {
        dvec2(LOCAL_ORIGIN, LOCAL_ORIGIN) + (screen - self.view.pos - self.pan) / self.scale
    }

    pub fn local_to_screen(&self, local: DVec2) -> DVec2 {
        self.view.pos + self.pan + (local - dvec2(LOCAL_ORIGIN, LOCAL_ORIGIN)) * self.scale
    }

    pub fn world_to_local(world: (f64, f64)) -> DVec2 {
        dvec2(LOCAL_ORIGIN + world.0, LOCAL_ORIGIN + world.1)
    }

    pub fn local_to_world(local: DVec2) -> (f64, f64) {
        (local.x - LOCAL_ORIGIN, local.y - LOCAL_ORIGIN)
    }

    pub fn screen_to_world(&self, screen: DVec2) -> (f64, f64) {
        Self::local_to_world(self.screen_to_local(screen))
    }

    /// The view rect in local units: what the clip and the background cover.
    pub fn local_view(&self) -> Rect {
        Rect {
            pos: self.screen_to_local(self.view.pos),
            size: self.view.size / self.scale,
        }
    }

    fn matrix(&self) -> Mat4f {
        let s = self.scale as f32;
        let t = self.view.pos + self.pan - dvec2(LOCAL_ORIGIN, LOCAL_ORIGIN) * self.scale;
        let mut m = Mat4f::default();
        m.v[0] = s;
        m.v[5] = s;
        m.v[12] = t.x as f32;
        m.v[13] = t.y as f32;
        m
    }

    pub fn popup_anchor_transform(&self) -> PopupAnchorTransform {
        PopupAnchorTransform {
            scale: self.scale,
            translation: self.view.pos + self.pan
                - dvec2(LOCAL_ORIGIN, LOCAL_ORIGIN) * self.scale,
        }
    }
}

#[derive(Clone, Copy)]
struct PortHit {
    node: usize,
    port: usize,
    output: bool,
}

/// One edge resolved to indices at `set_graph` time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EdgeIndex {
    from: usize,
    from_port: usize,
    to: usize,
    to_port: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CanvasHit {
    Card(usize),
    Wire(usize),
    Empty,
}

fn prioritize_canvas_hit(card: Option<usize>, wire: Option<usize>) -> CanvasHit {
    if let Some(card) = card {
        CanvasHit::Card(card)
    } else if let Some(wire) = wire {
        CanvasHit::Wire(wire)
    } else {
        CanvasHit::Empty
    }
}

struct CachedWire {
    key: u64,
    route: WireRoute,
}

struct WirePulse {
    node: String,
    /// Filled from the canvas's one animation clock on the next frame. This
    /// avoids comparing a newly arrived event with a clock that was idle.
    started: Option<f64>,
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct FlowCanvas {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawFlowGrid,
    #[live]
    draw_card: DrawFlowCard,
    #[live]
    draw_vec: DrawVector,
    #[live]
    draw_over: DrawVector,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_meta: DrawText,
    #[live]
    draw_port: DrawText,
    #[live]
    draw_chip: DrawText,
    #[live]
    draw_error: DrawText,
    #[live]
    card_color: Vec4f,
    #[live]
    card_color_hover: Vec4f,
    #[live]
    card_edge_color: Vec4f,
    #[live]
    accent_color: Vec4f,
    #[live]
    highlight_color: Vec4f,
    #[live]
    color_input: Vec4f,
    #[live]
    color_output: Vec4f,
    #[live]
    color_chat: Vec4f,
    #[live]
    color_gen: Vec4f,
    #[live]
    color_fn: Vec4f,
    #[live]
    color_http: Vec4f,
    #[live]
    color_ask: Vec4f,
    #[live]
    color_flow: Vec4f,
    #[live]
    color_port_text: Vec4f,
    #[live]
    color_port_image: Vec4f,
    #[live]
    color_port_audio: Vec4f,
    #[live]
    color_port_video: Vec4f,
    #[live]
    color_port_mesh: Vec4f,
    #[live]
    color_port_json: Vec4f,
    #[live]
    color_port_list: Vec4f,
    #[live]
    color_port_bytes: Vec4f,
    #[live]
    color_state_running: Vec4f,
    #[live]
    color_state_done: Vec4f,
    #[live]
    color_state_failed: Vec4f,
    #[live]
    color_state_waiting: Vec4f,
    #[live]
    color_state_inactive: Vec4f,
    #[live]
    color_state_idle: Vec4f,
    #[live]
    color_port_label_connected: Vec4f,
    #[live]
    color_port_label_open: Vec4f,
    #[live]
    icon_check: DrawSvg,
    #[live]
    icon_alert: DrawSvg,
    #[live]
    icon_clock: DrawSvg,

    #[rust]
    area: Area,
    #[rust]
    draw_list: Option<DrawList2d>,
    /// One retained list per graph node. Keeping a card's background, text,
    /// ports and face subtree in one list prevents draw-call batching from
    /// interleaving the contents of overlapping cards.
    #[rust]
    card_draw_lists: Vec<Option<DrawList2d>>,
    #[rust]
    camera: Camera,
    /// Screen-space chrome that `Fit` leaves clear around the graph.
    #[rust]
    fit_insets: Inset,
    /// Floating toolbar/panel rectangles. In particular, scroll events do not
    /// carry pointer capture, so the canvas must explicitly ignore them here.
    #[rust]
    chrome_rects: Vec<Rect>,
    #[rust]
    target_pan: DVec2,
    #[rust(1.0f64)]
    target_scale: f64,
    #[rust]
    graph: Option<Graph>,
    /// Graph lookup rebuilt only when the graph changes.
    #[rust]
    node_index: HashMap<String, usize>,
    #[rust]
    compatible_ports: CompatiblePorts,
    #[live]
    styles: CanvasStyles,
    /// Back-to-front card order; the back of this vector is screen-front.
    #[rust]
    z_order: Vec<String>,
    /// Card heights per node index, measured from the faces last frame.
    #[rust]
    heights: Vec<f64>,
    /// Zero is the ordinary left-to-right facing and one is mirrored. Values
    /// between them exist only during the 200 ms port-slide animation.
    #[rust]
    flip_positions: Vec<f64>,
    /// A hand-set facing is local UI state and deliberately never serialized.
    #[rust]
    flip_lock: HashSet<String>,
    #[rust]
    auto_flip_pending: bool,
    #[rust]
    auto_flip_settle_until: f64,
    #[rust]
    edges: Vec<EdgeIndex>,
    /// Geometry is retained until an endpoint card, another card, or the
    /// graph changes. Drawing and animation only consume this cache.
    #[rust]
    wire_cache: Vec<Option<CachedWire>>,
    #[rust]
    wire_cache_dirty: bool,
    #[rust]
    wire_mode: WireMode,
    #[rust]
    parallel_offsets: Vec<f64>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    hover_wire: Option<usize>,
    #[rust]
    selected: Option<Selection>,
    #[rust]
    highlight: Option<String>,
    #[rust]
    pub statuses: HashMap<String, NodeStatus>,
    #[rust]
    pub streaming: HashSet<String>,
    #[rust]
    carrying: HashSet<String>,
    #[rust]
    pulses: Vec<WirePulse>,
    /// A face that failed to evaluate, by node id, shown in that card only.
    #[rust]
    face_errors: HashMap<String, String>,
    #[rust]
    pub armed_type: Option<String>,
    #[rust]
    cursor: DVec2,
    #[rust]
    face_roots: Vec<(LiveId, WidgetRef)>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    last_time: f64,
    /// Frames left before the first fit: heights come from a draw, so the
    /// camera fits once the faces have been measured.
    #[rust]
    fit_pending: u8,
    #[rust]
    compatible: Vec<(usize, usize)>,
}

impl ScriptHook for FlowCanvas {}

impl WidgetNode for FlowCanvas {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, root) in &self.face_roots {
            visit(*id, root.clone());
        }
    }
    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        let local = self.camera.screen_to_local(point);
        for (_, root) in &self.face_roots {
            root.find_widgets_from_point(cx, local, found);
        }
    }
}

fn ease(current: f64, target: f64, dt: f64) -> f64 {
    let k = 1.0 - (-dt * 14.0).exp();
    let next = current + (target - current) * k;
    if (next - target).abs() < 1e-3 {
        target
    } else {
        next
    }
}

fn should_auto_flip(current: f64, flipped: f64, cables: usize, locked: bool) -> bool {
    cables > 0
        && !locked
        && current.is_finite()
        && flipped.is_finite()
        && flipped < current * AUTO_FLIP_RATIO
}

fn routing_cost(subjects: &[usize], routes: &[WireRoute]) -> f64 {
    let mut subject = vec![false; routes.len()];
    for index in subjects {
        subject[*index] = true;
    }
    let length = routes
        .iter()
        .enumerate()
        .filter(|(index, _)| subject[*index])
        .map(|(_, route)| route.length())
        .sum::<f64>();
    let bends = routes
        .iter()
        .enumerate()
        .filter(|(index, _)| subject[*index])
        .map(|(_, route)| route.bends())
        .sum::<usize>();
    let loops = routes
        .iter()
        .enumerate()
        .filter(|(index, route)| subject[*index] && route.is_loop())
        .count();
    let mut crossings = 0;
    for left in 0..routes.len() {
        if !subject[left] {
            continue;
        }
        for right in 0..routes.len() {
            if left == right || (subject[right] && right < left) {
                continue;
            }
            crossings += routes[left].crossings_with(&routes[right]);
        }
    }
    length
        + crossings as f64 * CROSSING_COST
        + bends as f64 * BEND_COST
        + loops as f64 * LOOP_COST
}

impl FlowCanvas {
    // -- the app's view of the canvas ------------------------------------------

    pub fn wire_mode(&self) -> WireMode {
        self.wire_mode
    }

    pub fn set_wire_mode(&mut self, cx: &mut Cx, mode: WireMode) {
        if self.wire_mode == mode {
            return;
        }
        self.wire_mode = mode;
        self.wire_cache.iter_mut().for_each(|cached| *cached = None);
        self.wire_cache_dirty = true;
        self.auto_flip_pending = mode == WireMode::Routed && self.graph.is_some();
        if self.auto_flip_pending {
            self.auto_flip_settle_until = self.time + AUTO_FLIP_SETTLE_SECONDS;
            self.next_frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }

    pub fn set_node_styles(&mut self, cx: &mut Cx, styles: HashMap<String, NodeStyle>) {
        cx.with_vm(|vm| self.styles.set_nodes(vm, styles));
        self.redraw(cx);
    }

    pub fn set_port_styles(&mut self, cx: &mut Cx, styles: HashMap<String, PortStyle>) {
        cx.with_vm(|vm| self.styles.set_ports(vm, styles));
        self.redraw(cx);
    }

    pub fn set_compatible_ports(&mut self, compatible: CompatiblePorts) {
        self.compatible_ports = compatible;
    }

    /// The face roots the app mounted for the bound instance; cleared before
    /// the app frees that isolate.
    pub fn set_face_roots(&mut self, cx: &mut Cx, roots: Vec<(LiveId, WidgetRef)>) {
        self.face_roots = roots;
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    pub fn set_graph(&mut self, cx: &mut Cx, graph: Option<Graph>) {
        // Keep the measured heights of the nodes that survive.
        let old = self.graph.take();
        let old_edges = std::mem::take(&mut self.edges);
        let old_wire_cache = std::mem::take(&mut self.wire_cache);
        let old_index: HashMap<String, usize> = old
            .as_ref()
            .map(|graph| {
                graph
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, node)| (node.id.clone(), index))
                    .collect()
            })
            .unwrap_or_default();
        let mut old_lists = std::mem::take(&mut self.card_draw_lists);
        let mut heights = Vec::new();
        let mut flip_positions = Vec::new();
        let mut edges = Vec::new();
        let mut card_draw_lists = Vec::new();
        let mut node_index = HashMap::new();
        if let Some(next) = graph.as_ref() {
            node_index.reserve(next.nodes.len());
            card_draw_lists.reserve(next.nodes.len());
            for (index, node) in next.nodes.iter().enumerate() {
                node_index.insert(node.id.clone(), index);
                let old_position = old_index.get(&node.id).copied();
                let height = old_position
                    .and_then(|index| self.heights.get(index).copied())
                    .unwrap_or(0.0);
                heights.push(height);
                flip_positions.push(
                    old_position
                        .and_then(|index| self.flip_positions.get(index).copied())
                        .unwrap_or(if node.flip { 1.0 } else { 0.0 }),
                );
                card_draw_lists.push(
                    old_position
                        .and_then(|index| old_lists.get_mut(index))
                        .and_then(Option::take),
                );
            }
            for edge in &next.edges {
                let from = node_index.get(&edge.from).copied();
                let to = node_index.get(&edge.to).copied();
                let (Some(from), Some(to)) = (from, to) else {
                    continue;
                };
                let from_port = next.nodes[from]
                    .outputs
                    .iter()
                    .position(|p| p.name == edge.from_port);
                let to_port = next.nodes[to]
                    .inputs
                    .iter()
                    .position(|p| p.name == edge.to_port);
                if let (Some(from_port), Some(to_port)) = (from_port, to_port) {
                    edges.push(EdgeIndex {
                        from,
                        from_port,
                        to,
                        to_port,
                    });
                }
            }
        }
        self.z_order.retain(|id| node_index.contains_key(id));
        let mut ordered: HashSet<String> = self.z_order.iter().cloned().collect();
        for node in graph.iter().flat_map(|graph| &graph.nodes) {
            if ordered.insert(node.id.clone()) {
                self.z_order.push(node.id.clone());
            }
        }
        let mut totals = HashMap::<(usize, usize), usize>::new();
        for edge in &edges {
            *totals.entry((edge.from, edge.to)).or_default() += 1;
        }
        let mut seen = HashMap::<(usize, usize), usize>::new();
        let spacing = RouteStyle::default().cable_spacing;
        self.parallel_offsets = edges
            .iter()
            .map(|edge| {
                let pair = (edge.from, edge.to);
                let position = seen.entry(pair).or_default();
                let offset = (*position as f64 - (totals[&pair] as f64 - 1.0) * 0.5) * spacing;
                *position += 1;
                offset
            })
            .collect();
        self.wire_cache = if edges == old_edges {
            old_wire_cache
        } else {
            std::iter::repeat_with(|| None).take(edges.len()).collect()
        };
        self.wire_cache_dirty = true;
        self.heights = heights;
        self.flip_positions = flip_positions;
        self.edges = edges;
        self.card_draw_lists = card_draw_lists;
        self.node_index = node_index;
        self.graph = graph;
        self.flip_lock.retain(|id| self.node_index.contains_key(id));
        self.auto_flip_pending = self.graph.is_some();
        self.auto_flip_settle_until = if self.time > 0.0 {
            self.time + AUTO_FLIP_SETTLE_SECONDS
        } else {
            f64::INFINITY
        };
        if self.auto_flip_pending {
            self.next_frame = cx.new_next_frame();
        }
        self.hover = None;
        self.hover_wire = None;
        if let Some(selected) = self.selected.clone() {
            if !self.has_selection(&selected) {
                self.selected = None;
                cx.widget_action(self.uid, FlowCanvasAction::Select(None));
            }
        }
        self.redraw(cx);
    }

    /// A different flow opened: fit it once its faces have been measured.
    pub fn reset_view(&mut self, cx: &mut Cx) {
        self.flip_lock.clear();
        self.auto_flip_pending = false;
        self.auto_flip_settle_until = 0.0;
        self.fit_pending = 2;
        self.redraw(cx);
    }

    /// Pin a user-chosen facing until they choose another facing by hand.
    pub fn lock_flip(&mut self, node: &str) {
        self.flip_lock.insert(node.to_string());
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_ref().and_then(Selection::node)
    }

    pub fn selection(&self) -> Option<&Selection> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, cx: &mut Cx, node: Option<String>) {
        if let Some(node) = node.as_deref() {
            self.raise_node(node);
        }
        self.selected = node.map(Selection::Node);
        self.redraw(cx);
    }

    /// Select a card without claiming the pointer event. The app calls this
    /// before dispatching into a face so an interactive child both acts and
    /// selects its owning node.
    pub fn select_at(&mut self, cx: &mut Cx, abs: DVec2) {
        let node = self.node_index_at(abs).and_then(|index| {
            self.graph
                .as_ref()
                .and_then(|graph| graph.nodes.get(index))
                .map(|node| node.id.clone())
        });
        let selection = node.clone().map(Selection::Node);
        if node.is_some() && self.selected != selection {
            self.raise_node(node.as_deref().unwrap());
            self.selected = selection.clone();
            cx.widget_action(self.uid, FlowCanvasAction::Select(selection));
            self.redraw(cx);
        }
    }

    fn raise_node(&mut self, node: &str) {
        raise_to_front(&mut self.z_order, node);
    }

    fn compatible_for(
        &self,
        graph: &Graph,
        from: usize,
        from_port: usize,
    ) -> Vec<(usize, usize)> {
        let from_id = &graph.nodes[from].id;
        let port_name = &graph.nodes[from].outputs[from_port].name;
        self.compatible_ports
            .get(&(from_id.clone(), port_name.clone()))
            .into_iter()
            .flatten()
            .filter_map(|(node, port)| {
                let node = self.node_index.get(node).copied()?;
                let port = graph.nodes[node]
                    .inputs
                    .iter()
                    .position(|input| input.name == *port)?;
                Some((node, port))
            })
            .collect()
    }

    pub fn is_resize_handle_at(&self, abs: DVec2) -> bool {
        self.resize_at(abs).is_some()
    }

    /// Display-only face widgets deliberately do not block a card drag. The
    /// allowlist mirrors the controls that own presses inside a card face.
    fn interactive_face_widget_at(&self, cx: &Cx, abs: DVec2, handled: Area) -> bool {
        let local = self.camera.screen_to_local(abs);
        let mut interactive = false;
        let mut handled_widget_found = handled.is_empty();
        for (_, root) in &self.face_roots {
            root.find_widgets_from_point(cx, local, &mut |widget| {
                handled_widget_found |= widget.area() == handled;
                if !interactive
                    && widget
                        .widget_type_id()
                        .is_some_and(is_interactive_face_type)
                {
                    interactive = true;
                }
            });
            if interactive {
                break;
            }
        }
        // Scroll bars are stored inside a View rather than as WidgetRefs. A
        // face-owned capture with no matching widget is therefore an opaque
        // interactive control and must keep the press.
        interactive || !handled_widget_found
    }

    pub fn set_highlight(&mut self, cx: &mut Cx, node: Option<String>) {
        if self.highlight != node {
            self.highlight = node;
            self.redraw(cx);
        }
    }

    pub fn set_status(&mut self, cx: &mut Cx, node: &str, status: NodeStatus) {
        let shown = self.statuses.get(node).map(|old| old.shown).unwrap_or(0.0);
        let mut status = status;
        status.shown = shown;
        self.statuses.insert(node.to_string(), status);
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    pub fn clear_run(&mut self, cx: &mut Cx) {
        self.statuses.clear();
        self.streaming.clear();
        self.carrying.clear();
        self.pulses.clear();
        self.redraw(cx);
    }

    /// Start one value pulse on every outgoing cable. Pulses share the
    /// canvas clock and are bounded so a very chatty stream cannot grow an
    /// unbounded animation queue.
    pub fn pulse(&mut self, cx: &mut Cx, node: &str, carrying: bool) {
        if carrying {
            self.carrying.insert(node.to_string());
        }
        let clock_was_live = self.animating();
        let too_soon = self.pulses.iter().rev().find(|pulse| pulse.node == node).is_some_and(
            |pulse| pulse.started.is_none() || self.time - pulse.started.unwrap_or(self.time) < 0.08,
        );
        if !too_soon {
            self.pulses.push(WirePulse {
                node: node.to_string(),
                started: clock_was_live.then_some(self.time),
            });
            if self.pulses.len() > 64 {
                self.pulses.remove(0);
            }
        }
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    pub fn set_streaming(&mut self, cx: &mut Cx, node: &str, on: bool) {
        if on {
            if self.streaming.insert(node.to_string()) {
                self.next_frame = cx.new_next_frame();
            }
        } else {
            self.streaming.remove(node);
        }
        self.redraw(cx);
    }

    pub fn set_face_errors(&mut self, cx: &mut Cx, errors: HashMap<String, String>) {
        self.face_errors = errors;
        self.redraw(cx);
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    pub fn zoom(&self) -> f64 {
        self.target_scale
    }

    pub fn set_fit_insets(&mut self, insets: Inset) {
        self.fit_insets = insets;
    }

    pub fn set_chrome_rects(&mut self, rects: Vec<Rect>) {
        self.chrome_rects = rects;
    }

    /// Zoom by a factor around the view centre (menu / toolbar).
    pub fn zoom_by(&mut self, cx: &mut Cx, factor: f64) {
        let centre = self.camera.view.pos + self.camera.view.size * 0.5;
        self.zoom_to(cx, centre, self.target_scale * factor);
    }

    pub fn zoom_reset(&mut self, cx: &mut Cx) {
        let centre = self.camera.view.pos + self.camera.view.size * 0.5;
        self.zoom_to(cx, centre, 1.0);
    }

    /// Fit every node into the view.
    pub fn fit(&mut self, cx: &mut Cx) {
        let Some(graph) = self.graph.as_ref() else {
            return;
        };
        let full_view = self.camera.view;
        let view = Rect {
            pos: full_view.pos + dvec2(self.fit_insets.left, self.fit_insets.top),
            size: dvec2(
                (full_view.size.x - self.fit_insets.left - self.fit_insets.right).max(1.0),
                (full_view.size.y - self.fit_insets.top - self.fit_insets.bottom).max(1.0),
            ),
        };
        if graph.nodes.is_empty() || view.size.x <= 0.0 {
            self.target_pan = dvec2(0.0, 0.0);
            self.target_scale = 1.0;
            self.next_frame = cx.new_next_frame();
            return;
        }
        let mut min = dvec2(f64::MAX, f64::MAX);
        let mut max = dvec2(f64::MIN, f64::MIN);
        for (index, node) in graph.nodes.iter().enumerate() {
            let (x, y) = node.at;
            let size = self.node_size(graph, index);
            min.x = min.x.min(x);
            min.y = min.y.min(y - LABEL_H);
            max.x = max.x.max(x + size.x);
            max.y = max.y.max(y + size.y);
        }
        let span = max - min;
        let scale = ((view.size.x - 2.0 * FIT_MARGIN) / span.x)
            .min((view.size.y - 2.0 * FIT_MARGIN) / span.y)
            .clamp(ZOOM_MIN, 1.0);
        self.target_scale = scale;
        self.target_pan = view.pos - full_view.pos
            + dvec2(
                (view.size.x - span.x * scale) * 0.5 - min.x * scale,
                (view.size.y - span.y * scale) * 0.5 - min.y * scale,
            );
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    fn zoom_to(&mut self, cx: &mut Cx, anchor: DVec2, scale: f64) {
        let scale = scale.clamp(ZOOM_MIN, ZOOM_MAX);
        // The world point under the anchor stays under it at the end of
        // the ease.
        let target = Camera {
            view: self.camera.view,
            pan: self.target_pan,
            scale: self.target_scale,
        };
        let world = target.screen_to_world(anchor);
        self.target_scale = scale;
        self.target_pan = dvec2(
            anchor.x - self.camera.view.pos.x - world.0 * scale,
            anchor.y - self.camera.view.pos.y - world.1 * scale,
        );
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    fn has_node(&self, id: &str) -> bool {
        self.graph
            .as_ref()
            .is_some_and(|graph| graph.nodes.iter().any(|node| node.id == id))
    }

    fn has_selection(&self, selection: &Selection) -> bool {
        match selection {
            Selection::Node(node) => self.has_node(node),
            Selection::Edge {
                from_node,
                from_port,
                to_node,
                to_port,
            } => self.graph.as_ref().is_some_and(|graph| {
                graph.edges.iter().any(|edge| {
                    edge.from == *from_node
                        && edge.from_port == *from_port
                        && edge.to == *to_node
                        && edge.to_port == *to_port
                })
            }),
        }
    }

    // -- geometry (local units) -----------------------------------------------

    /// The node's world position, with a live drag applied.
    fn node_at(&self, graph: &Graph, index: usize) -> (f64, f64) {
        let node = &graph.nodes[index];
        let mut at = node.at;
        if let Some(Drag::Node {
            index: dragged,
            origin,
            moved: true,
            ..
        }) = &self.drag
        {
            if *dragged == index {
                at = *origin;
            }
        }
        at
    }

    fn card_rect(&self, graph: &Graph, index: usize) -> Rect {
        let pos = Camera::world_to_local(self.node_at(graph, index));
        Rect {
            pos,
            size: self.node_size(graph, index),
        }
    }

    fn node_size(&self, graph: &Graph, index: usize) -> DVec2 {
        let node = &graph.nodes[index];
        let min_height = min_card_height(Self::full_bleed(node), Self::port_rows(node));
        if let Some(Drag::Resize {
            index: resized,
            size,
            ..
        }) = &self.drag
        {
            if *resized == index {
                return clamp_card_size(
                    dvec2(size.0, size.1),
                    Self::full_bleed(node),
                    Self::port_rows(node),
                );
            }
        }
        node
            .size
            .map(|(w, h)| clamp_card_size(dvec2(w, h), Self::full_bleed(node), Self::port_rows(node)))
            .unwrap_or_else(|| {
                dvec2(
                    NODE_WIDTH,
                    self.heights
                        .get(index)
                        .copied()
                        .unwrap_or(0.0)
                        .max(min_height),
                )
            })
    }

    fn point_over_chrome(&self, abs: DVec2) -> bool {
        self.chrome_rects.iter().any(|rect| rect.contains(abs))
    }

    fn resize_at(&self, abs: DVec2) -> Option<usize> {
        let graph = self.graph.as_ref()?;
        let local = self.camera.screen_to_local(abs);
        let grip = RESIZE_GRIP / self.camera.scale.min(1.0);
        (0..graph.nodes.len()).rev().find(|index| {
            let rect = self.card_rect(graph, *index);
            Rect {
                pos: rect.pos + rect.size - dvec2(grip, grip),
                size: dvec2(grip, grip),
            }
            .contains(local)
        })
    }

    fn full_bleed(node: &Node) -> bool {
        node.full_bleed
    }

    fn input_kind(node: &Node, port: usize) -> &str {
        if port == 0 {
            declared_output_kind(node).unwrap_or(&node.inputs[port].kind)
        } else {
            &node.inputs[port].kind
        }
    }

    fn port_rows(node: &Node) -> usize {
        if Self::full_bleed(node) {
            0
        } else {
            node.inputs.len().max(node.outputs.len())
        }
    }

    fn port_local_at_flip(
        &self,
        graph: &Graph,
        index: usize,
        port: usize,
        output: bool,
        flip: f64,
    ) -> DVec2 {
        let rect = self.card_rect(graph, index);
        let y = rect.pos.y + CARD_HEADER_H + (port as f64 + 0.5) * PORT_ROW_H;
        let side = if output { 1.0 - flip } else { flip };
        dvec2(rect.pos.x + rect.size.x * side, y)
    }

    /// Where a wire meets the port: the tip of an output's point, the apex
    /// of an input's notch — never the disc centre, so a wire visibly leaves
    /// the point along its axis.
    fn wire_anchor_at_flip(
        &self,
        graph: &Graph,
        index: usize,
        port: usize,
        output: bool,
        flip: f64,
    ) -> DVec2 {
        let p = self.port_local_at_flip(graph, index, port, output, flip);
        let direction = if Self::port_side_at_flip(true, flip) == PortSide::Right {
            1.0
        } else {
            -1.0
        };
        let offset = if output {
            direction * (PORT_RX + PORT_TIP)
        } else {
            -direction * (PORT_RX - PORT_DENT)
        };
        dvec2(p.x + offset, p.y)
    }

    fn wire_anchor(&self, graph: &Graph, index: usize, port: usize, output: bool) -> DVec2 {
        let flip = self
            .flip_positions
            .get(index)
            .copied()
            .unwrap_or(if graph.nodes[index].flip { 1.0 } else { 0.0 });
        self.wire_anchor_at_flip(graph, index, port, output, flip)
    }

    fn port_local(&self, graph: &Graph, index: usize, port: usize, output: bool) -> DVec2 {
        let flip = self
            .flip_positions
            .get(index)
            .copied()
            .unwrap_or(if graph.nodes[index].flip { 1.0 } else { 0.0 });
        self.port_local_at_flip(graph, index, port, output, flip)
    }

    fn port_side_at_flip(output: bool, flip: f64) -> PortSide {
        let on_right = if output { flip < 0.5 } else { flip >= 0.5 };
        if on_right {
            PortSide::Right
        } else {
            PortSide::Left
        }
    }

    fn port_side(&self, graph: &Graph, index: usize, output: bool) -> PortSide {
        let flip = self
            .flip_positions
            .get(index)
            .copied()
            .unwrap_or(if graph.nodes[index].flip { 1.0 } else { 0.0 });
        Self::port_side_at_flip(output, flip)
    }

    fn port_at(&self, abs: DVec2) -> Option<PortHit> {
        let graph = self.graph.as_ref()?;
        let local = self.camera.screen_to_local(abs);
        let r = PORT_HIT_R / self.camera.scale.min(1.0);
        for id in self.z_order.iter().rev() {
            let index = *self.node_index.get(id)?;
            let node = &graph.nodes[index];
            for port in 0..node.inputs.len() {
                let pos = self.port_local(graph, index, port, false);
                if (pos - local).length() <= r {
                    return Some(PortHit {
                        node: index,
                        port,
                        output: false,
                    });
                }
            }
            for port in 0..node.outputs.len() {
                let pos = self.port_local(graph, index, port, true);
                if (pos - local).length() <= r {
                    return Some(PortHit {
                        node: index,
                        port,
                        output: true,
                    });
                }
            }
        }
        None
    }

    fn node_index_at(&self, abs: DVec2) -> Option<usize> {
        let graph = self.graph.as_ref()?;
        let local = self.camera.screen_to_local(abs);
        self.z_order.iter().rev().find_map(|id| {
            let index = *self.node_index.get(id)?;
            let mut rect = self.card_rect(graph, index);
            rect.pos.y -= LABEL_H;
            rect.size.y += LABEL_H;
            rect.contains(local).then_some(index)
        })
    }

    fn wire_index_at(&self, abs: DVec2) -> Option<usize> {
        let local = self.camera.screen_to_local(abs);
        let point = Self::route_point(local);
        let threshold = WIRE_HIT_PX / self.camera.scale.max(0.01);
        self.wire_cache
            .iter()
            .enumerate()
            .filter_map(|(index, cached)| {
                let distance = cached.as_ref()?.route.distance_to_point(point);
                (distance <= threshold).then_some((index, distance))
            })
            .min_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.0.cmp(&right.0))
            })
            .map(|(index, _)| index)
    }

    fn edge_selection(&self, graph: &Graph, index: usize) -> Option<Selection> {
        let edge = *self.edges.get(index)?;
        Some(Selection::Edge {
            from_node: graph.nodes.get(edge.from)?.id.clone(),
            from_port: graph.nodes.get(edge.from)?.outputs.get(edge.from_port)?.name.clone(),
            to_node: graph.nodes.get(edge.to)?.id.clone(),
            to_port: graph.nodes.get(edge.to)?.inputs.get(edge.to_port)?.name.clone(),
        })
    }

    fn selected_edge_index(&self, graph: &Graph) -> Option<usize> {
        let selection = self.selected.as_ref()?;
        self.edges.iter().enumerate().find_map(|(index, _)| {
            (self.edge_selection(graph, index).as_ref() == Some(selection)).then_some(index)
        })
    }

    fn port_color(&self, kind: &str) -> Vec4f {
        if let Some(color) = self.styles.port_color(kind) {
            return color;
        }
        match kind {
            "text" => self.color_port_text,
            "image" => self.color_port_image,
            "audio" => self.color_port_audio,
            "video" => self.color_port_video,
            "mesh" => self.color_port_mesh,
            "json" => self.color_port_json,
            "list" => self.color_port_list,
            "bytes" => self.color_port_bytes,
            _ => self.color_flow,
        }
    }

    fn kind_color(&self, kind: &str) -> Vec4f {
        if let Some(color) = self.styles.node_color(kind) {
            return color;
        }
        match kind {
            "input" => self.color_input,
            "output" => self.color_output,
            "chat" => self.color_chat,
            "gen" => self.color_gen,
            "fn" => self.color_fn,
            "http" => self.color_http,
            "ask" => self.color_ask,
            _ => self.color_flow,
        }
    }

    fn state_color(&self, state: &str) -> Vec4f {
        match state {
            "running" | "ready" | "queued" => self.color_state_running,
            "done" => self.color_state_done,
            "failed" => self.color_state_failed,
            "waiting" => self.color_state_waiting,
            "skipped" | "cancelled" => self.color_state_inactive,
            _ => self.color_state_idle,
        }
    }

    fn set_color(v: &mut DrawVector, c: Vec4f, alpha: f64) {
        v.set_color(c.x, c.y, c.z, c.w * alpha as f32);
    }

    fn route_point(point: DVec2) -> Point {
        Point::new(point.x, point.y)
    }

    fn draw_route(v: &mut DrawVector, route: &WireRoute) {
        v.move_to(route.from.x as f32, route.from.y as f32);
        match &route.kind {
            RouteKind::Cubic { control_1, control_2 } => v.bezier_to(
                control_1.x as f32,
                control_1.y as f32,
                control_2.x as f32,
                control_2.y as f32,
                route.to.x as f32,
                route.to.y as f32,
            ),
            RouteKind::Orthogonal { points, radius } => {
                for index in 1..points.len() - 1 {
                    let before = points[index - 1];
                    let corner = points[index];
                    let after = points[index + 1];
                    let in_len = ((before.x - corner.x).powi(2) + (before.y - corner.y).powi(2)).sqrt();
                    let out_len = ((after.x - corner.x).powi(2) + (after.y - corner.y).powi(2)).sqrt();
                    let r = radius.min(in_len * 0.5).min(out_len * 0.5);
                    let entry = Point::new(
                        corner.x + (before.x - corner.x) * r / in_len,
                        corner.y + (before.y - corner.y) * r / in_len,
                    );
                    let exit = Point::new(
                        corner.x + (after.x - corner.x) * r / out_len,
                        corner.y + (after.y - corner.y) * r / out_len,
                    );
                    v.line_to(entry.x as f32, entry.y as f32);
                    v.quad_to(corner.x as f32, corner.y as f32, exit.x as f32, exit.y as f32);
                }
                v.line_to(route.to.x as f32, route.to.y as f32);
            }
        }
    }

    fn draw_chevron(v: &mut DrawVector, centre: Point, tangent: Point, size: f64) {
        let normal = Point::new(-tangent.y, tangent.x);
        let tip = Point::new(
            centre.x + tangent.x * size * 0.5,
            centre.y + tangent.y * size * 0.5,
        );
        let tail = Point::new(
            centre.x - tangent.x * size * 0.5,
            centre.y - tangent.y * size * 0.5,
        );
        for side in [-1.0, 1.0] {
            v.move_to(
                (tail.x + normal.x * size * 0.45 * side) as f32,
                (tail.y + normal.y * size * 0.45 * side) as f32,
            );
            v.line_to(tip.x as f32, tip.y as f32);
        }
    }

    /// A port disc whose cable side carries the flow direction: an output
    /// ends in a sharp `>` point, an input has a small `>` notch the point
    /// would fit into. `grow` widens the oval (hover, the selected halo);
    /// `direction` is the card's flow direction (+1 left-to-right).
    fn shaped_port(v: &mut DrawVector, centre: Point, grow: f64, direction: f64, input: bool) {
        let rx = PORT_RX + grow;
        let ry = PORT_R + grow;
        let cable_side = if input { -direction } else { direction };
        let (apex_r, half_angle) = if input {
            (rx - PORT_DENT, PORT_NOTCH_HALF_ANGLE)
        } else {
            (rx + PORT_TIP, PORT_TIP_HALF_ANGLE)
        };
        let base = if cable_side < 0.0 {
            std::f64::consts::PI
        } else {
            0.0
        };
        let steps = 40;
        let start = base + half_angle;
        let sweep = std::f64::consts::TAU - 2.0 * half_angle;
        for step in 0..=steps {
            let angle = start + sweep * step as f64 / steps as f64;
            let x = (centre.x + rx * angle.cos()) as f32;
            let y = (centre.y + ry * angle.sin()) as f32;
            if step == 0 {
                v.move_to(x, y);
            } else {
                v.line_to(x, y);
            }
        }
        v.line_to((centre.x + cable_side * apex_r) as f32, centre.y as f32);
        v.close();
    }

    fn draw_points(v: &mut DrawVector, points: &[Point]) {
        let Some(first) = points.first() else {
            return;
        };
        v.move_to(first.x as f32, first.y as f32);
        for point in &points[1..] {
            v.line_to(point.x as f32, point.y as f32);
        }
    }

    fn draw_route_slice(v: &mut DrawVector, route: &WireRoute, start: f64, end: f64) {
        if start >= 0.0 && end <= route.length() {
            Self::draw_points(v, &route.slice(start, end));
            return;
        }
        if start < 0.0 {
            Self::draw_points(v, &route.slice(0.0, end));
            Self::draw_points(v, &route.slice(route.length() + start, route.length()));
        } else {
            Self::draw_points(v, &route.slice(start, route.length()));
            Self::draw_points(v, &route.slice(0.0, end - route.length()));
        }
    }

    fn draw_clamped_route_slice(v: &mut DrawVector, route: &WireRoute, start: f64, end: f64) {
        Self::draw_points(
            v,
            &route.slice(start.max(0.0), end.min(route.length())),
        );
    }

    fn route_cache_key(
        edge: EdgeIndex,
        from: Point,
        source_side: PortSide,
        to: Point,
        target_side: PortSide,
        obstacles: &[Obstacle],
        offset: f64,
    ) -> u64 {
        let mut hash = DefaultHasher::new();
        edge.from.hash(&mut hash);
        edge.from_port.hash(&mut hash);
        edge.to.hash(&mut hash);
        edge.to_port.hash(&mut hash);
        source_side.hash(&mut hash);
        target_side.hash(&mut hash);
        for value in [from.x, from.y, to.x, to.y, offset] {
            value.to_bits().hash(&mut hash);
        }
        for obstacle in obstacles {
            for value in [
                obstacle.min.x,
                obstacle.min.y,
                obstacle.max.x,
                obstacle.max.y,
            ] {
                value.to_bits().hash(&mut hash);
            }
        }
        hash.finish()
    }

    fn ensure_wire_routes(&mut self, graph: &Graph) {
        const CARD_CLEARANCE: f64 = 12.0;
        if !self.wire_cache_dirty && self.wire_cache.iter().all(Option::is_some) {
            return;
        }
        let card_rects: Vec<Rect> = (0..graph.nodes.len())
            .map(|index| self.card_rect(graph, index))
            .collect();
        let all_obstacles: Vec<Obstacle> = card_rects
            .iter()
            .map(|rect| {
                Obstacle::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y)
                    .inflate(CARD_CLEARANCE)
            })
            .collect();
        for index in 0..self.edges.len() {
            let edge = self.edges[index];
            let from = Self::route_point(self.wire_anchor(graph, edge.from, edge.from_port, true));
            let to = Self::route_point(self.wire_anchor(graph, edge.to, edge.to_port, false));
            let source_side = self.port_side(graph, edge.from, true);
            let target_side = self.port_side(graph, edge.to, false);
            let offset = self.parallel_offsets.get(index).copied().unwrap_or(0.0);
            let style = RouteStyle::default();
            let obstacles = if self.wire_mode == WireMode::Routed {
                wire_route::obstacles_in_corridor(
                    from,
                    to,
                    &all_obstacles,
                    style.port_stub + style.corner_radius * 2.0 + offset.abs(),
                )
            } else {
                Vec::new()
            };
            let key = Self::route_cache_key(
                edge,
                from,
                source_side,
                to,
                target_side,
                &obstacles,
                offset,
            );
            if self.wire_cache.get(index).and_then(Option::as_ref).is_some_and(|cached| cached.key == key)
            {
                continue;
            }
            let previous = self.wire_cache[index]
                .as_ref()
                .map(|cached| &cached.route);
            let route = wire_route::route_wire_sticky_in_mode(
                self.wire_mode,
                from,
                source_side,
                to,
                target_side,
                &obstacles,
                style,
                offset,
                previous,
            );
            self.wire_cache[index] = Some(CachedWire { key, route });
        }
        self.wire_cache_dirty = false;
    }

    fn flip_animation_active(&self, graph: &Graph) -> bool {
        graph.nodes.iter().enumerate().any(|(index, node)| {
            let target = if node.flip { 1.0 } else { 0.0 };
            self.flip_positions
                .get(index)
                .is_some_and(|position| (position - target).abs() > 1e-4)
        })
    }

    fn routes_for_facings(
        &self,
        graph: &Graph,
        facings: &[bool],
        obstacles: &[Obstacle],
    ) -> Vec<WireRoute> {
        self.edges
            .iter()
            .copied()
            .enumerate()
            .map(|(edge_index, edge)| {
                let from_flip = if facings[edge.from] { 1.0 } else { 0.0 };
                let to_flip = if facings[edge.to] { 1.0 } else { 0.0 };
                let from = Self::route_point(self.wire_anchor_at_flip(
                    graph,
                    edge.from,
                    edge.from_port,
                    true,
                    from_flip,
                ));
                let to = Self::route_point(self.wire_anchor_at_flip(
                    graph,
                    edge.to,
                    edge.to_port,
                    false,
                    to_flip,
                ));
                wire_route::route_wire(
                    from,
                    Self::port_side_at_flip(true, from_flip),
                    to,
                    Self::port_side_at_flip(false, to_flip),
                    obstacles,
                    RouteStyle::default(),
                    self.parallel_offsets.get(edge_index).copied().unwrap_or(0.0),
                )
            })
            .collect()
    }

    fn port_stubs_at_flip(
        &self,
        graph: &Graph,
        node_index: usize,
        flip: f64,
    ) -> Vec<(Point, Point)> {
        let node = &graph.nodes[node_index];
        let mut stubs = Vec::with_capacity(node.inputs.len() + node.outputs.len());
        let stub_length = RouteStyle::default().port_stub;
        for (output, count) in [(false, node.inputs.len()), (true, node.outputs.len())] {
            let side = Self::port_side_at_flip(output, flip);
            let direction = if side == PortSide::Right { 1.0 } else { -1.0 };
            for port in 0..count {
                let from = Self::route_point(self.wire_anchor_at_flip(
                    graph,
                    node_index,
                    port,
                    output,
                    flip,
                ));
                stubs.push((from, Point::new(from.x + direction * stub_length, from.y)));
            }
        }
        stubs
    }

    fn scored_routes_for_card(
        &self,
        node_index: usize,
        routes: &[WireRoute],
        stubs: &[(Point, Point)],
    ) -> Vec<usize> {
        let mut scored: Vec<usize> = self
            .edges
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| {
                (edge.from == node_index || edge.to == node_index).then_some(index)
            })
            .collect();
        for (index, route) in routes.iter().enumerate() {
            if !scored.contains(&index)
                && stubs
                    .iter()
                    .any(|(from, to)| route.intersects_segment(*from, *to))
            {
                scored.push(index);
            }
        }
        scored
    }

    fn facing_costs(
        &self,
        graph: &Graph,
        node_index: usize,
        facings: &[bool],
        current_routes: &[WireRoute],
        obstacles: &[Obstacle],
    ) -> (f64, f64, usize) {
        let cable_count = self
            .edges
            .iter()
            .filter(|edge| edge.from == node_index || edge.to == node_index)
            .count();
        let current_flip = if facings[node_index] { 1.0 } else { 0.0 };
        let current_subjects = self.scored_routes_for_card(
            node_index,
            current_routes,
            &self.port_stubs_at_flip(graph, node_index, current_flip),
        );

        let mut candidate_facings = facings.to_vec();
        candidate_facings[node_index] = !candidate_facings[node_index];
        let mut candidate_routes = current_routes.to_vec();
        for (edge_index, edge) in self.edges.iter().enumerate() {
            if edge.from != node_index && edge.to != node_index {
                continue;
            }
            let from_flip = if candidate_facings[edge.from] { 1.0 } else { 0.0 };
            let to_flip = if candidate_facings[edge.to] { 1.0 } else { 0.0 };
            let from = Self::route_point(self.wire_anchor_at_flip(
                graph,
                edge.from,
                edge.from_port,
                true,
                from_flip,
            ));
            let to = Self::route_point(self.wire_anchor_at_flip(
                graph,
                edge.to,
                edge.to_port,
                false,
                to_flip,
            ));
            candidate_routes[edge_index] = wire_route::route_wire(
                from,
                Self::port_side_at_flip(true, from_flip),
                to,
                Self::port_side_at_flip(false, to_flip),
                obstacles,
                RouteStyle::default(),
                self.parallel_offsets.get(edge_index).copied().unwrap_or(0.0),
            );
        }
        let candidate_flip = if candidate_facings[node_index] { 1.0 } else { 0.0 };
        let candidate_subjects = self.scored_routes_for_card(
            node_index,
            &candidate_routes,
            &self.port_stubs_at_flip(graph, node_index, candidate_flip),
        );
        (
            routing_cost(&current_subjects, current_routes),
            routing_cost(&candidate_subjects, &candidate_routes),
            cable_count,
        )
    }

    /// Evaluate settled geometry in bounded whole-graph passes. A pass uses
    /// one route snapshot so cards cannot observe a half-applied result.
    fn maybe_auto_flip(&mut self, cx: &mut Cx, graph: &mut Graph) {
        if self.wire_mode == WireMode::Bezier {
            self.auto_flip_pending = false;
            return;
        }
        if !self.auto_flip_pending
            || self.time < self.auto_flip_settle_until
            || self.drag.is_some()
            || self.flip_animation_active(graph)
        {
            return;
        }
        const CARD_CLEARANCE: f64 = 12.0;
        let card_rects: Vec<Rect> = (0..graph.nodes.len())
            .map(|index| self.card_rect(graph, index))
            .collect();
        let obstacles: Vec<Obstacle> = card_rects
            .iter()
            .map(|rect| {
                Obstacle::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y)
                    .inflate(CARD_CLEARANCE)
            })
            .collect();
        let original_facings: Vec<bool> = graph.nodes.iter().map(|node| node.flip).collect();
        let mut facings = original_facings.clone();
        for _ in 0..AUTO_FLIP_MAX_PASSES {
            let current_routes = self.routes_for_facings(graph, &facings, &obstacles);
            let mut pass_changes = Vec::new();
            for node_index in 0..graph.nodes.len() {
                let locked = self.flip_lock.contains(&graph.nodes[node_index].id);
                let (current, flipped, cables) = self.facing_costs(
                    graph,
                    node_index,
                    &facings,
                    &current_routes,
                    &obstacles,
                );
                if should_auto_flip(current, flipped, cables, locked) {
                    pass_changes.push(node_index);
                }
            }
            if pass_changes.is_empty() {
                break;
            }
            for node_index in pass_changes {
                facings[node_index] = !facings[node_index];
            }
        }
        self.auto_flip_pending = false;
        let changes: Vec<(String, bool)> = graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                (facings[index] != original_facings[index])
                    .then_some((node.id.clone(), facings[index]))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        for (id, flip) in &changes {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == *id) {
                node.flip = *flip;
            }
        }
        self.wire_cache_dirty = true;
        self.next_frame = cx.new_next_frame();
        cx.widget_action(self.uid, FlowCanvasAction::AutoFlip(changes));
        self.redraw(cx);
    }

    fn preview_route(
        &self,
        graph: &Graph,
        from_index: usize,
        from_port: usize,
        pointer: DVec2,
        target: Option<(usize, usize)>,
    ) -> WireRoute {
        const CARD_CLEARANCE: f64 = 12.0;
        let from = Self::route_point(self.wire_anchor(graph, from_index, from_port, true));
        let source_side = self.port_side(graph, from_index, true);
        let (to, target_side) = target.map_or(
            (Self::route_point(pointer), PortSide::Left),
            |(node, port)| {
                (
                    Self::route_point(self.wire_anchor(graph, node, port, false)),
                    self.port_side(graph, node, false),
                )
            },
        );
        let obstacles: Vec<Obstacle> = if self.wire_mode == WireMode::Routed {
            (0..graph.nodes.len())
                .map(|index| {
                    let rect = self.card_rect(graph, index);
                    Obstacle::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y)
                        .inflate(CARD_CLEARANCE)
                })
                .collect()
        } else {
            Vec::new()
        };
        wire_route::route_wire_in_mode(
            self.wire_mode,
            from,
            source_side,
            to,
            target_side,
            &obstacles,
            RouteStyle::default(),
            0.0,
        )
    }

    fn text_width(&self, cx: &mut Cx2d, draw: &DrawText, text: &str) -> f64 {
        draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text)
            .size_in_lpxs
            .width as f64
    }

    // -- drawing ---------------------------------------------------------------

    fn draw_background(&mut self, cx: &mut Cx2d, local_view: Rect) {
        // Grid level of detail: the cell doubles (or halves) so it stays
        // between GRID_MIN_PX and 2 × GRID_MIN_PX on screen.
        let level = (GRID_MIN_PX / (GRID_CELL * self.camera.scale))
            .log2()
            .ceil()
            .clamp(-2.0, 6.0);
        self.draw_bg.cell = (GRID_CELL * 2f64.powf(level)) as f32;
        self.draw_bg.origin = vec2(LOCAL_ORIGIN as f32, LOCAL_ORIGIN as f32);
        self.draw_bg.draw_abs(cx, local_view);
    }

    /// Wires are one vector batch in the canvas list, below every card list.
    fn draw_wires(&mut self, cx: &mut Cx2d, graph: &Graph) {
        let dragging_wire = matches!(self.drag, Some(Drag::Wire { .. }));
        let time = self.time;
        self.ensure_wire_routes(graph);
        let selected_wire = self.selected_edge_index(graph);
        self.draw_vec.begin();
        // Wires, under the cards.
        for (index, edge) in self.edges.iter().copied().enumerate() {
            let kind = &graph.nodes[edge.from].outputs[edge.from_port].kind;
            let color = self.port_color(kind);
            let node = &graph.nodes[edge.from].id;
            let streaming = self.streaming.contains(node);
            let carrying = self.carrying.contains(node);
            let selected = selected_wire == Some(index);
            let hovered = self.hover_wire == Some(index);
            let Some(route) = self.wire_cache[index].as_ref().map(|cached| &cached.route) else {
                continue;
            };
            if streaming {
                Self::set_color(&mut self.draw_vec, color, 0.22);
                Self::draw_route(&mut self.draw_vec, route);
                self.draw_vec.stroke(10.0);
            }
            if selected {
                Self::set_color(&mut self.draw_vec, self.accent_color, 0.22);
                Self::draw_route(&mut self.draw_vec, route);
                self.draw_vec.stroke(13.0);
            }
            Self::set_color(
                &mut self.draw_vec,
                color,
                if selected || hovered {
                    1.0
                } else if dragging_wire {
                    0.35
                } else if carrying {
                    1.0
                } else {
                    0.95
                },
            );
            Self::draw_route(&mut self.draw_vec, route);
            self.draw_vec.stroke(if selected {
                5.0
            } else if hovered {
                3.75
            } else {
                3.0
            });
            if carrying && !dragging_wire {
                Self::set_color(&mut self.draw_vec, color, 0.75);
                Self::draw_route(&mut self.draw_vec, route);
                self.draw_vec.stroke(1.25);
            }
            if streaming {
                let spacing = (route.length() / 3.0).max(32.0);
                for k in 0..3 {
                    let start = (time * 48.0 + k as f64 * spacing) % route.length().max(1.0);
                    Self::set_color(&mut self.draw_vec, color, 0.9);
                    Self::draw_route_slice(&mut self.draw_vec, route, start, start + 18.0);
                    self.draw_vec.stroke(4.5);
                }
            }
            for pulse in self.pulses.iter().filter(|pulse| pulse.node == *node) {
                let elapsed = pulse.started.map_or(0.0, |started| (time - started).max(0.0));
                let centre = wire_route::pulse_progress(elapsed) * route.length();
                Self::set_color(&mut self.draw_vec, color, 0.20);
                Self::draw_clamped_route_slice(
                    &mut self.draw_vec,
                    route,
                    centre - 24.0,
                    centre + 24.0,
                );
                self.draw_vec.stroke(13.0);
                Self::set_color(&mut self.draw_vec, color, 1.0);
                Self::draw_clamped_route_slice(
                    &mut self.draw_vec,
                    route,
                    centre - 20.0,
                    centre + 20.0,
                );
                self.draw_vec.stroke(5.0);
            }
            if self.camera.scale >= 0.5 {
                let midpoint = route.length() * 0.5;
                let pulse_brightness = self
                    .pulses
                    .iter()
                    .filter(|pulse| pulse.node == *node)
                    .map(|pulse| {
                        let elapsed = pulse.started.map_or(0.0, |started| (time - started).max(0.0));
                        let centre = wire_route::pulse_progress(elapsed) * route.length();
                        (1.0 - (centre - midpoint).abs() / 24.0).clamp(0.0, 1.0)
                    })
                    .fold(0.0, f64::max);
                Self::set_color(
                    &mut self.draw_vec,
                    color,
                    if selected || hovered {
                        1.0
                    } else if dragging_wire {
                        0.35
                    } else {
                        0.8 + pulse_brightness * 0.2
                    },
                );
                let (point, tangent) = route.midpoint_tangent();
                Self::draw_chevron(&mut self.draw_vec, point, tangent, 8.0);
                self.draw_vec.stroke(1.5);
            }
        }
        // The wire being dragged.
        if let Some(Drag::Wire {
            from,
            from_port,
            ty,
            pos,
            target,
            ..
        }) = &self.drag
        {
            let b = self.camera.screen_to_local(*pos);
            let color = self.port_color(ty);
            let route = self.preview_route(graph, *from, *from_port, b, *target);
            Self::set_color(&mut self.draw_vec, color, 1.0);
            Self::draw_route(&mut self.draw_vec, &route);
            self.draw_vec.stroke(3.0);
        }
        self.draw_vec.end(cx);
    }

    /// Card shadow, body and its shared-geometry selection/hover outline.
    fn draw_card(&mut self, cx: &mut Cx2d, graph: &Graph, indices: &[usize]) {
        self.draw_card.shadow_radius = (12.0 / self.camera.scale.max(0.01)) as f32;
        self.draw_card.shadow_offset = vec2(0.0, 0.0);
        let hover = self.hover;
        let selected = self.selected.as_ref().and_then(Selection::node);
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let rect = self.card_rect(graph, index);
            let outline = card_outline_geometry(rect, self.camera.scale);
            let highlighted = self.highlight.as_deref() == Some(node.id.as_str());
            let waiting = self.statuses.get(&node.id).map(|s| s.state.as_str()) == Some("waiting");
            self.draw_card.color = if hover == Some(index) {
                self.card_color_hover
            } else {
                self.card_color
            };
            self.draw_card.border_color = if highlighted {
                self.highlight_color
            } else if waiting {
                self.state_color("waiting")
            } else {
                self.card_edge_color
            };
            self.draw_card.border_size = if highlighted || waiting { 2.0 } else { 1.0 };
            self.draw_card.border_radius = outline.radius;
            let outline_alpha = if selected == Some(node.id.as_str()) {
                1.0
            } else if hover == Some(index) {
                CARD_HOVER_OUTLINE_ALPHA
            } else {
                0.0
            };
            self.draw_card.outline_color = vec4(
                self.accent_color.x,
                self.accent_color.y,
                self.accent_color.z,
                self.accent_color.w * outline_alpha,
            );
            self.draw_card.outline_size = if outline_alpha > 0.0 {
                outline.stroke_width
            } else {
                0.0
            };
            self.draw_card.draw_abs(cx, rect);
        }
    }

    fn kind_icon(&mut self, kind: &str) -> Option<&mut DrawSvg> {
        self.styles.node_icon(kind)
    }

    fn port_icon(&mut self, kind: &str) -> Option<&mut DrawSvg> {
        self.styles.port_icon(kind)
    }

    /// Labels above the cards, port names, error lines.
    fn draw_labels(&mut self, cx: &mut Cx2d, graph: &Graph, indices: &[usize]) {
        let phase = ((self.time * 2.5) as usize) % 4;
        const DOTS: [&str; 4] = ["·", "··", "···", "····"];
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let r = self.card_rect(graph, index);
            let label_y = r.pos.y - LABEL_H;
            // Kind icon + id (bold) + type (muted).
            let icon_rect = Rect {
                pos: dvec2(r.pos.x + 2.0, label_y + 5.0),
                size: dvec2(15.0, 15.0),
            };
            if let Some(kind) = declared_output_kind(node) {
                if let Some(icon) = self.port_icon(kind) {
                    icon.draw_abs(cx, icon_rect);
                }
            } else if let Some(icon) = self.kind_icon(&node.kind) {
                icon.draw_abs(cx, icon_rect);
            }
            let id_w = self.text_width(cx, &self.draw_title, &node.title);
            self.draw_title
                .draw_abs(cx, dvec2(r.pos.x + 23.0, label_y + 6.0), &node.title);
            self.draw_meta
                .draw_abs(cx, dvec2(r.pos.x + 29.0 + id_w, label_y + 7.0), &node.type_name);
            // State chip at the right of the label row.
            if let Some(status) = self.statuses.get(&node.id) {
                let state = status.state.as_str();
                let color = self.state_color(state);
                let text: &str = if state == "running" && !status.has_progress {
                    DOTS[phase]
                } else {
                    status.chip.as_str()
                };
                let w = self.text_width(cx, &self.draw_chip, text);
                let mut x = r.pos.x + r.size.x - w - 2.0;
                self.draw_chip.color = color;
                self.draw_chip.draw_abs(cx, dvec2(x, label_y + 7.5), text);
                let icon_rect = Rect {
                    pos: dvec2(x - 16.0, label_y + 6.5),
                    size: dvec2(12.0, 12.0),
                };
                match state {
                    "done" => self.icon_check.draw_abs(cx, icon_rect),
                    "failed" => self.icon_alert.draw_abs(cx, icon_rect),
                    "waiting" => self.icon_clock.draw_abs(cx, icon_rect),
                    _ => x += 16.0,
                }
                if !status.stage.is_empty() && state == "running" {
                    let stage_w = self.text_width(cx, &self.draw_meta, &status.stage);
                    self.draw_meta.draw_abs(
                        cx,
                        dvec2(x - 24.0 - stage_w, label_y + 7.5),
                        &status.stage,
                    );
                }
            }
            // Port names inside the port strip.
            if !Self::full_bleed(node) {
                for (port, input) in node.inputs.iter().enumerate() {
                    let p = self.port_local(graph, index, port, false);
                    self.draw_port.color = if input.connected {
                        self.color_port_label_connected
                    } else {
                        self.color_port_label_open
                    };
                    let w = self.text_width(cx, &self.draw_port, &input.name);
                    let x = match self.port_side(graph, index, false) {
                        PortSide::Left => p.x + PORT_RX + PORT_LABEL_GAP,
                        PortSide::Right => p.x - PORT_RX - PORT_LABEL_GAP - w,
                    };
                    self.draw_port.draw_abs(cx, dvec2(x, p.y - 6.0), &input.name);
                }
                for (port, output) in node.outputs.iter().enumerate() {
                    let p = self.port_local(graph, index, port, true);
                    let w = self.text_width(cx, &self.draw_port, &output.name);
                    self.draw_port.color = self.color_port_label_connected;
                    let x = match self.port_side(graph, index, true) {
                        PortSide::Left => p.x + PORT_RX + PORT_LABEL_GAP,
                        PortSide::Right => p.x - PORT_RX - PORT_LABEL_GAP - w,
                    };
                    self.draw_port.draw_abs(cx, dvec2(x, p.y - 6.0), &output.name);
                }
            }
            // Errors: the face's own, or the node's run error, one line.
            let error = self
                .face_errors
                .get(&node.id)
                .map(String::as_str)
                .or_else(|| {
                    self.statuses
                        .get(&node.id)
                        .and_then(|status| status.error.as_deref())
                });
            if let Some(error) = error {
                let line = error.lines().next().unwrap_or(error);
                self.draw_error.draw_abs(
                    cx,
                    dvec2(r.pos.x + CARD_PAD, r.pos.y + r.size.y - 18.0),
                    line,
                );
            }
        }
    }

    /// The faces, each in a turtle at its card's content rect; measures the
    /// card heights for the next frame.
    fn draw_faces(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        graph: &Graph,
        indices: &[usize],
    ) -> bool {
        let mut changed = false;
        if let Some(faces) = scope.data.get_mut::<NodeFacesScope>() {
            faces
                .faces()
                .set_popup_anchor_transform(cx, Some(self.camera.popup_anchor_transform()));
        }
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let r = self.card_rect(graph, index);
            let full_bleed = Self::full_bleed(node);
            let content = card_content_rect(r, full_bleed, Self::port_rows(node));
            let has_error = self.face_errors.contains_key(&node.id)
                || self
                    .statuses
                    .get(&node.id)
                    .is_some_and(|status| status.error.is_some());
            let is_resizing = matches!(
                self.drag.as_ref(),
                Some(Drag::Resize {
                    index: resized,
                    ..
                }) if *resized == index
            );
            let fixed_height = (node.size.is_some() || is_resizing).then_some(content.rect.size.y);
            // Generic draw clipping is rectangular. Full-bleed media also use
            // the card's rounded SDF in their own shader; together these keep
            // every face inside the card body at any camera transform.
            cx.push_clip_rect(r);
            cx.begin_turtle(
                Walk {
                    abs_pos: Some(content.rect.pos),
                    margin: Inset::default(),
                    width: Size::Fixed(content.rect.size.x),
                    height: fixed_height.map(Size::Fixed).unwrap_or_else(Size::fit),
                    metrics: Metrics::default(),
                },
                Layout {
                    flow: Flow::Down,
                    clip_x: true,
                    clip_y: true,
                    ..Layout::default()
                },
            );
            if let Some(faces) = scope.data.get_mut::<NodeFacesScope>() {
                faces.faces().draw_face(
                    cx,
                    &node.id,
                    if fixed_height.is_some() {
                        Walk::fill()
                    } else {
                        Walk::fill_fit()
                    },
                    fixed_height.is_some(),
                );
            }
            let rect = cx.end_turtle();
            cx.pop_clip_rect();
            let mut height = rect.size.y + content.pad_top + content.pad_bottom;
            if has_error {
                height += 20.0;
            }
            let height = height.max(min_card_height(full_bleed, Self::port_rows(node)));
            if node.size.is_none() && (self.heights[index] - height).abs() > 0.5 {
                self.heights[index] = height;
                changed = true;
            }
        }
        changed
    }

    /// Ports and progress bars: the second batch, above the faces (a picture
    /// fills its card to the edges the ports sit on).
    fn draw_overlays(&mut self, cx: &mut Cx2d, graph: &Graph, indices: &[usize]) {
        let compatible_active = matches!(self.drag, Some(Drag::Wire { .. }));
        let wire_target = match &self.drag {
            Some(Drag::Wire { target, .. }) => *target,
            _ => None,
        };
        let selected_edge = self
            .selected_edge_index(graph)
            .and_then(|index| self.edges.get(index).copied());
        let time = self.time;
        self.draw_over.begin();
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let r = self.card_rect(graph, index);
            // Progress bar: the top strip of a running card.
            if let Some(status) = self.statuses.get(&node.id) {
                let state = status.state.as_str();
                let show = matches!(state, "running" | "waiting" | "queued")
                    || (matches!(state, "done" | "failed") && status.shown < 1.0 - 1e-6)
                    || matches!(state, "done" | "failed");
                if show {
                    let bx = r.pos.x as f32 + 12.0;
                    let by = r.pos.y as f32 + 8.0;
                    let bw = r.size.x as f32 - 24.0;
                    self.draw_over.set_color(1.0, 1.0, 1.0, 0.08);
                    self.draw_over
                        .rounded_rect(bx, by, bw, PROGRESS_H as f32, 2.0);
                    self.draw_over.fill();
                    let color = if state == "running" {
                        self.kind_color(&node.kind)
                    } else {
                        self.state_color(state)
                    };
                    if state == "running" && !status.has_progress {
                        // Indeterminate: a sweeping segment.
                        let seg = bw * 0.28;
                        let t = (time * 0.8).fract() as f32;
                        let sx = bx + (bw + seg) * t - seg;
                        let x0 = sx.max(bx);
                        let x1 = (sx + seg).min(bx + bw);
                        if x1 > x0 {
                            Self::set_color(&mut self.draw_over, color, 0.95);
                            self.draw_over
                                .rounded_rect(x0, by, x1 - x0, PROGRESS_H as f32, 2.0);
                            self.draw_over.fill();
                        }
                    } else {
                        let fraction = status.shown.clamp(0.0, 1.0) as f32;
                        if fraction > 0.0 {
                            Self::set_color(&mut self.draw_over, color, 0.95);
                            self.draw_over.rounded_rect(
                                bx,
                                by,
                                (bw * fraction).max(PROGRESS_H as f32),
                                PROGRESS_H as f32,
                                2.0,
                            );
                            self.draw_over.fill();
                        }
                    }
                }
            }
            // Ports: a dark disc with a ring in the port-type colour. The
            // disc's cable side is shaped: outputs point, inputs are dimpled.
            let direction = if self.port_side(graph, index, true) == PortSide::Right {
                1.0
            } else {
                -1.0
            };
            for port in 0..node.inputs.len() {
                let p = self.port_local(graph, index, port, false);
                let centre = Point::new(p.x, p.y);
                let ok = !compatible_active || self.compatible.contains(&(index, port));
                let hot = wire_target == Some((index, port));
                let grow = if hot { 3.0 } else { 0.0 };
                if selected_edge.is_some_and(|edge| edge.to == index && edge.to_port == port) {
                    Self::set_color(&mut self.draw_over, self.accent_color, 1.0);
                    Self::shaped_port(&mut self.draw_over, centre, 4.0, direction, true);
                    self.draw_over.stroke(2.0);
                }
                Self::set_color(&mut self.draw_over, self.card_color, 1.0);
                Self::shaped_port(&mut self.draw_over, centre, grow, direction, true);
                self.draw_over.fill();
                let color = self.port_color(Self::input_kind(node, port));
                Self::set_color(&mut self.draw_over, color, if ok { 1.0 } else { 0.25 });
                Self::shaped_port(&mut self.draw_over, centre, grow - 1.0, direction, true);
                self.draw_over.stroke(if hot { 3.0 } else { 2.0 });
            }
            for (port, output) in node.outputs.iter().enumerate() {
                let p = self.port_local(graph, index, port, true);
                let centre = Point::new(p.x, p.y);
                if selected_edge.is_some_and(|edge| edge.from == index && edge.from_port == port) {
                    Self::set_color(&mut self.draw_over, self.accent_color, 1.0);
                    Self::shaped_port(&mut self.draw_over, centre, 4.0, direction, false);
                    self.draw_over.stroke(2.0);
                }
                Self::set_color(&mut self.draw_over, self.card_color, 1.0);
                Self::shaped_port(&mut self.draw_over, centre, 0.0, direction, false);
                self.draw_over.fill();
                let color = self.port_color(&output.kind);
                Self::set_color(&mut self.draw_over, color, 1.0);
                Self::shaped_port(&mut self.draw_over, centre, -1.0, direction, false);
                self.draw_over.stroke(2.0);
            }
        }
        self.draw_over.end(cx);
        // The port-type icons inside the discs.
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let direction = if self.port_side(graph, index, true) == PortSide::Right {
                1.0
            } else {
                -1.0
            };
            for port in 0..node.inputs.len() {
                let p = self.port_local(graph, index, port, false);
                let rect = Rect {
                    pos: p + dvec2(-direction * PORT_ICON_SHIFT_IN - 4.75, -4.75),
                    size: dvec2(9.5, 9.5),
                };
                if let Some(icon) = self.port_icon(Self::input_kind(node, port)) {
                    icon.draw_abs(cx, rect);
                }
            }
            for (port, output) in node.outputs.iter().enumerate() {
                let p = self.port_local(graph, index, port, true);
                let rect = Rect {
                    pos: p + dvec2(direction * PORT_ICON_SHIFT_OUT - 4.75, -4.75),
                    size: dvec2(9.5, 9.5),
                };
                if let Some(icon) = self.port_icon(&output.kind) {
                    icon.draw_abs(cx, rect);
                }
            }
        }
        // The grip is last inside the body: shadow/body/face/ports/icons/grip.
        self.draw_over.begin();
        Self::set_color(&mut self.draw_over, self.draw_meta.color, 0.55);
        for index in indices.iter().copied() {
            let r = self.card_rect(graph, index);
            for inset in [5.0, 9.0, 13.0] {
                self.draw_over.move_to(
                    (r.pos.x + r.size.x - inset) as f32,
                    (r.pos.y + r.size.y - 3.0) as f32,
                );
                self.draw_over.line_to(
                    (r.pos.x + r.size.x - 3.0) as f32,
                    (r.pos.y + r.size.y - inset) as f32,
                );
                self.draw_over.stroke(1.0);
            }
        }
        self.draw_over.end(cx);
    }

    /// Placement ghost above the retained card lists.
    fn draw_top_overlay(&mut self, cx: &mut Cx2d) {
        self.draw_over.begin();
        if self.armed_type.is_some() && self.camera.view.contains(self.cursor) {
            let local = self.camera.screen_to_local(self.cursor);
            let x = (local.x - NODE_WIDTH * 0.5) as f32;
            let y = local.y as f32;
            self.draw_over.set_color(1.0, 1.0, 1.0, 0.06);
            self.draw_over
                .rounded_rect(x, y, NODE_WIDTH as f32, 120.0, CARD_RADIUS);
            self.draw_over.fill();
            Self::set_color(&mut self.draw_over, self.accent_color, 0.9);
            self.draw_over
                .rounded_rect(x, y, NODE_WIDTH as f32, 120.0, CARD_RADIUS);
            self.draw_over.stroke(1.5);
        }
        self.draw_over.end(cx);
    }

    fn finish_wire_drag(&mut self, cx: &mut Cx, drag: Drag) {
        let Drag::Wire {
            from,
            from_port,
            ty,
            pos,
            target,
        } = drag
        else {
            return;
        };
        let Some(graph) = self.graph.as_ref() else {
            return;
        };
        let from_node = graph.nodes[from].id.clone();
        let from_port = graph.nodes[from].outputs[from_port].name.clone();
        if let Some((to, to_port)) = target {
            let to_node = graph.nodes[to].id.clone();
            let to_port = graph.nodes[to].inputs[to_port].name.clone();
            cx.widget_action(
                self.uid,
                FlowCanvasAction::Edit(CanvasEdit::Connect {
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                }),
            );
        } else if self.node_index_at(pos).is_none() {
            let at = self.camera.screen_to_world(pos);
            cx.widget_action(
                self.uid,
                FlowCanvasAction::OpenPalette {
                    at,
                    from_node,
                    from_port,
                    ty,
                },
            );
        }
        self.compatible.clear();
    }

    fn animating(&self) -> bool {
        self.auto_flip_pending
            || !self.streaming.is_empty()
            || !self.pulses.is_empty()
            || self.statuses.values().any(|status| {
                matches!(status.state.as_str(), "running" | "waiting" | "queued")
                    || (status.shown - status.target_fraction()).abs() > 1e-3
            })
            || (self.camera.pan - self.target_pan).length() > 0.05
            || (self.camera.scale - self.target_scale).abs() > 1e-4
            || self
                .graph
                .as_ref()
                .is_some_and(|graph| self.flip_animation_active(graph))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_selection_moves_the_whole_retained_card_to_the_front() {
        let mut order = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        raise_to_front(&mut order, "b");
        assert_eq!(order, vec!["a", "c", "b"]);
        raise_to_front(&mut order, "b");
        assert_eq!(order, vec!["a", "c", "b"]);
    }

    #[test]
    fn card_hit_wins_over_wire_hit() {
        assert_eq!(prioritize_canvas_hit(Some(3), Some(7)), CanvasHit::Card(3));
        assert_eq!(prioritize_canvas_hit(None, Some(7)), CanvasHit::Wire(7));
        assert_eq!(prioritize_canvas_hit(None, None), CanvasHit::Empty);
    }

    #[test]
    fn card_outline_is_body_geometry_grown_by_screen_space_stroke() {
        let body = Rect {
            pos: dvec2(100.0, 200.0),
            size: dvec2(300.0, 220.0),
        };
        let outline = card_outline_geometry(body, 0.5);
        assert_eq!(outline.radius, CARD_RADIUS);
        assert_eq!(outline.stroke_width, 4.0);
        assert_eq!(outline.outer_rect.pos, dvec2(96.0, 196.0));
        assert_eq!(outline.outer_rect.size, dvec2(308.0, 228.0));
    }

    #[test]
    fn face_press_classification_keeps_controls_interactive_and_displays_draggable() {
        for interactive in [
            TypeId::of::<TextInput>(),
            TypeId::of::<FabValueInput>(),
            TypeId::of::<DropDown>(),
            TypeId::of::<Button>(),
            TypeId::of::<FoldHeader>(),
            TypeId::of::<Slider>(),
        ] {
            assert!(is_interactive_face_type(interactive));
        }
        for display in [
            TypeId::of::<View>(),
            TypeId::of::<Image>(),
            TypeId::of::<Label>(),
            TypeId::of::<Markdown>(),
        ] {
            assert!(!is_interactive_face_type(display));
        }
    }

    #[test]
    fn card_content_rect_subtracts_header_ports_and_padding() {
        let card = Rect {
            pos: dvec2(100.0, 200.0),
            size: dvec2(300.0, 220.0),
        };
        let content = card_content_rect(card, false, 2);
        assert_eq!(content.pad_top, CARD_HEADER_H + 2.0 * PORT_ROW_H);
        assert_eq!(content.pad_bottom, CARD_PAD);
        assert_eq!(content.rect.pos, dvec2(114.0, 262.0));
        assert_eq!(content.rect.size, dvec2(272.0, 144.0));

        assert_eq!(card_content_rect(card, true, 0).rect, card);
    }

    #[test]
    fn card_size_clamp_keeps_one_content_line() {
        let min_height = CARD_HEADER_H + 2.0 * PORT_ROW_H + CARD_PAD + MIN_TEXT_LINE_H;
        assert_eq!(min_card_height(false, 2), min_height);
        assert_eq!(
            clamp_card_size(dvec2(20.0, 20.0), false, 2),
            dvec2(MIN_NODE_WIDTH, min_height)
        );
        assert_eq!(
            clamp_card_size(dvec2(240.0, 180.0), false, 2),
            dvec2(240.0, 180.0)
        );
    }

    #[test]
    fn auto_flip_shortens_the_image_to_below_left_picture_route() {
        let style = RouteStyle::default();
        let obstacles = [
            Obstacle::from_xywh(1340.0, 180.0, 400.0, 400.0).inflate(12.0),
            Obstacle::from_xywh(300.0, 580.0, 900.0, 650.0).inflate(12.0),
        ];
        let from = Point::new(1740.0, 206.0);
        let current = wire_route::route_wire(
            from,
            PortSide::Right,
            Point::new(300.0, 606.0),
            PortSide::Left,
            &obstacles,
            style,
            0.0,
        );
        let flipped = wire_route::route_wire(
            from,
            PortSide::Right,
            Point::new(1200.0, 606.0),
            PortSide::Right,
            &obstacles,
            style,
            0.0,
        );
        assert!(
            should_auto_flip(current.length(), flipped.length(), 1, false),
            "current={} flipped={}",
            current.length(),
            flipped.length()
        );
    }

    #[test]
    fn routed_expand_geometry_respects_auto_flip_hysteresis() {
        let style = RouteStyle::default();
        let obstacles = [
            Obstacle::from_xywh(590.0, 110.0, 440.0, 230.0).inflate(12.0),
            Obstacle::from_xywh(680.0, 530.0, 420.0, 330.0).inflate(12.0),
            Obstacle::from_xywh(110.0, 920.0, 430.0, 400.0).inflate(12.0),
        ];
        let prompt = Point::new(1030.0, 136.0);
        let add_style = Point::new(680.0, 556.0);
        let current = vec![
            wire_route::route_wire(
                prompt,
                PortSide::Right,
                Point::new(540.0, 946.0),
                PortSide::Right,
                &obstacles,
                style,
                0.0,
            ),
            wire_route::route_wire(
                Point::new(110.0, 946.0),
                PortSide::Left,
                add_style,
                PortSide::Left,
                &obstacles,
                style,
                0.0,
            ),
        ];
        let unflipped = vec![
            wire_route::route_wire(
                prompt,
                PortSide::Right,
                Point::new(110.0, 946.0),
                PortSide::Left,
                &obstacles,
                style,
                0.0,
            ),
            wire_route::route_wire(
                Point::new(540.0, 946.0),
                PortSide::Right,
                add_style,
                PortSide::Left,
                &obstacles,
                style,
                0.0,
            ),
        ];
        let current_cost = routing_cost(&[0, 1], &current);
        let unflipped_cost = routing_cost(&[0, 1], &unflipped);
        assert!(current[0].crossings_with(&current[1]) > 0);
        assert_eq!(unflipped[0].crossings_with(&unflipped[1]), 0);
        assert!(
            !should_auto_flip(current_cost, unflipped_cost, 2, false),
            "current={current_cost} unflipped={unflipped_cost}"
        );
    }

    #[test]
    fn auto_flip_uses_strict_twenty_percent_hysteresis() {
        assert!(should_auto_flip(100.0, 79.99, 1, false));
        assert!(!should_auto_flip(100.0, 80.0, 1, false));
        assert!(!should_auto_flip(100.0, 10.0, 0, false));
    }

    #[test]
    fn hand_flip_lock_blocks_an_otherwise_winning_auto_flip() {
        let mut locks = HashSet::new();
        locks.insert("picture".to_string());
        assert!(!should_auto_flip(
            1000.0,
            100.0,
            1,
            locks.contains("picture")
        ));
    }
}

fn raise_to_front(order: &mut Vec<String>, node: &str) {
    if let Some(index) = order.iter().position(|id| id == node) {
        let id = order.remove(index);
        order.push(id);
    }
}

impl NodeStatus {
    fn target_fraction(&self) -> f64 {
        match self.state.as_str() {
            "done" | "failed" | "skipped" | "cancelled" => 1.0,
            "running" if self.has_progress => self.permille as f64 / 1000.0,
            _ => 0.0,
        }
    }
}

impl Widget for FlowCanvas {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Reserve the rect in the parent and keep the hit area in window
        // space; everything else lives in the transformed child list.
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        let first_layout = self.camera.view.size.x <= 0.0;
        self.camera.view = view;
        if first_layout {
            self.target_pan = self.camera.pan;
            self.target_scale = self.camera.scale;
        }
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        let mut draw_list = self.draw_list.take().unwrap();
        draw_list.begin_always(cx);
        cx.begin_root_turtle(dvec2(ROOT_SIZE, ROOT_SIZE), Layout::flow_overlay());
        let local_view = self.camera.local_view();
        cx.push_clip_rect(local_view);
        self.draw_background(cx, local_view);
        let mut heights_changed = false;
        if let Some(mut graph) = self.graph.take() {
            self.draw_wires(cx, &graph);
            let z_order = std::mem::take(&mut self.z_order);
            if let Some(faces) = scope.data.get_mut::<NodeFacesScope>() {
                faces.faces().set_z_order(&z_order);
            }
            for id in &z_order {
                let Some(index) = self.node_index.get(id).copied() else {
                    continue;
                };
                let mut card_list = self.card_draw_lists[index]
                    .take()
                    .unwrap_or_else(|| DrawList2d::new(cx));
                card_list.begin_always(cx);
                let one = std::slice::from_ref(&index);
                // One retained list fixes the visual order per card. The
                // outline is part of the body shader, so face content and
                // then ports/icons/grip necessarily cover it; the label is
                // emitted last and remains above the complete card.
                self.draw_card(cx, &graph, one);
                heights_changed |= self.draw_faces(cx, scope, &graph, one);
                self.draw_overlays(cx, &graph, one);
                self.draw_labels(cx, &graph, one);
                card_list.end(cx);
                self.card_draw_lists[index] = Some(card_list);
            }
            self.z_order = z_order;
            self.draw_top_overlay(cx);
            if !heights_changed {
                self.maybe_auto_flip(cx, &mut graph);
            }
            self.graph = Some(graph);
        }
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle();
        draw_list.end(cx);
        draw_list.set_view_transform(cx, &self.camera.matrix());
        self.draw_list = Some(draw_list);
        if heights_changed {
            // Wires, ports and outlines were drawn against last frame's heights.
            self.wire_cache_dirty = true;
            self.area.redraw(cx);
        }
        if self.fit_pending > 0 && !heights_changed {
            self.fit_pending -= 1;
            if self.fit_pending == 0 {
                self.fit(cx);
                self.camera.pan = self.target_pan;
                self.camera.scale = self.target_scale;
                cx.widget_action(
                    self.uid,
                    FlowCanvasAction::Camera {
                        scale: self.camera.scale,
                    },
                );
            }
            self.area.redraw(cx);
        }
        if self.animating() {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Popup scroll views run before the canvas and claim the axis they
        // consume. Scroll hits themselves do not consult those flags, so keep
        // the graph still whenever any popup has already taken the wheel.
        let scroll_is_handled = matches!(
            event,
            Event::Scroll(event) if event.handled_x.get() || event.handled_y.get()
        );
        if let Some(nf) = self.next_frame.is_event(event) {
            let dt = (nf.time - self.last_time).clamp(0.0, 0.1);
            self.last_time = nf.time;
            self.time = nf.time;
            if self.auto_flip_pending && !self.auto_flip_settle_until.is_finite() {
                self.auto_flip_settle_until = nf.time + AUTO_FLIP_SETTLE_SECONDS;
            }
            let moved = (self.camera.pan - self.target_pan).length() > 0.05
                || (self.camera.scale - self.target_scale).abs() > 1e-4;
            if moved {
                self.camera.pan.x = ease(self.camera.pan.x, self.target_pan.x, dt);
                self.camera.pan.y = ease(self.camera.pan.y, self.target_pan.y, dt);
                self.camera.scale = ease(self.camera.scale, self.target_scale, dt);
                cx.widget_action(
                    self.uid,
                    FlowCanvasAction::Camera {
                        scale: self.camera.scale,
                    },
                );
            }
            for status in self.statuses.values_mut() {
                let target = status.target_fraction();
                status.shown = ease(status.shown, target, dt);
            }
            if let Some(graph) = self.graph.as_ref() {
                let step = dt / FLIP_SECONDS;
                let mut changed = false;
                for (index, node) in graph.nodes.iter().enumerate() {
                    let target = if node.flip { 1.0 } else { 0.0 };
                    let Some(position) = self.flip_positions.get_mut(index) else {
                        continue;
                    };
                    let next = if *position < target {
                        (*position + step).min(target)
                    } else {
                        (*position - step).max(target)
                    };
                    changed |= (next - *position).abs() > f64::EPSILON;
                    *position = next;
                }
                if changed {
                    // Endpoint coordinates change continuously. Route keys keep
                    // every non-incident cable cached.
                    self.wire_cache_dirty = true;
                }
            }
            for pulse in &mut self.pulses {
                if pulse.started.is_none() {
                    pulse.started = Some(nf.time);
                }
            }
            self.pulses
                .retain(|pulse| nf.time - pulse.started.unwrap_or(nf.time) <= 0.6);
            if self.animating() {
                self.next_frame = cx.new_next_frame();
            }
            self.area.redraw(cx);
        }
        // A palette type armed by a press elsewhere lands on release here.
        if let Event::MouseUp(e) = event {
            if let Some(type_name) = self.armed_type.take() {
                if self.camera.view.contains(e.abs) && !self.point_over_chrome(e.abs) {
                    let world = self.camera.screen_to_world(e.abs);
                    cx.widget_action(
                        self.uid,
                        FlowCanvasAction::Edit(CanvasEdit::AddType {
                            type_name,
                            at: (world.0 - NODE_WIDTH * 0.5, world.1),
                        }),
                    );
                }
                self.area.redraw(cx);
            }
        }
        if let Event::MouseMove(e) = event {
            self.cursor = e.abs;
            let over_chrome = self.point_over_chrome(e.abs);
            if self.armed_type.is_some() && self.camera.view.contains(e.abs) && !over_chrome {
                self.area.redraw(cx);
            }
            if self.drag.is_none() {
                let hit = if self.camera.view.contains(e.abs) && !over_chrome {
                    prioritize_canvas_hit(self.node_index_at(e.abs), self.wire_index_at(e.abs))
                } else {
                    CanvasHit::Empty
                };
                let (hover, hover_wire) = match hit {
                    CanvasHit::Card(index) => (Some(index), None),
                    CanvasHit::Wire(index) => (None, Some(index)),
                    CanvasHit::Empty => (None, None),
                };
                cx.set_cursor(if hover_wire.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if hover != self.hover || hover_wire != self.hover_wire {
                    self.hover = hover;
                    self.hover_wire = hover_wire;
                    self.area.redraw(cx);
                }
            }
        }
        // Faces receive events first. Co-capture display-only face presses so
        // a click can still open a picture, then promote the canvas capture
        // once movement crosses the card-drag threshold. Interactive face
        // controls retain exclusive capture.
        let capture_display_press = match event {
            Event::MouseDown(event) => {
                self.node_index_at(event.abs).is_some()
                    && !self.interactive_face_widget_at(cx, event.abs, event.handled.get())
            }
            Event::TouchUpdate(event) => event.touches.iter().any(|touch| {
                touch.state == TouchState::Start
                    && self.node_index_at(touch.abs).is_some()
                    && !self.interactive_face_widget_at(cx, touch.abs, touch.handled.get())
            }),
            _ => false,
        };
        match event.hits_with_capture_overload(cx, self.area, capture_display_press) {
            Hit::FingerScroll(fs)
                if !scroll_is_handled && !self.point_over_chrome(fs.abs) =>
            {
                // Wheel = zoom anchored at the cursor; a horizontal wheel pans.
                if fs.scroll.x.abs() > fs.scroll.y.abs() * 1.5 {
                    self.target_pan.x -= fs.scroll.x;
                    self.camera.pan.x = self.target_pan.x;
                    self.area.redraw(cx);
                } else {
                    let factor = (-fs.scroll.y * 0.0035).exp();
                    self.zoom_to(cx, fs.abs, self.target_scale * factor);
                }
            }
            Hit::FingerDown(fd) if !self.point_over_chrome(fd.abs) => {
                cx.set_key_focus(self.area);
                if let Some(hit) = self.port_at(fd.abs) {
                    let graph = self.graph.as_ref().unwrap();
                    let node = &graph.nodes[hit.node];
                    let hit_id = node.id.clone();
                    if hit.output {
                        let compatible = self.compatible_for(graph, hit.node, hit.port);
                        self.drag = Some(Drag::Wire {
                            from: hit.node,
                            from_port: hit.port,
                            ty: node.outputs[hit.port].kind.clone(),
                            pos: fd.abs,
                            target: None,
                        });
                        self.compatible = compatible;
                    } else {
                        // An input with a wire: pick the wire up again from
                        // its source; a bare one does nothing.
                        let input = &node.inputs[hit.port];
                        if input.connected {
                            let source = self
                                .edges
                                .iter()
                                .find(|edge| edge.to == hit.node && edge.to_port == hit.port)
                                .map(|edge| (edge.from, edge.from_port));
                            if let Some((from, from_port)) = source {
                                let ty = graph.nodes[from].outputs[from_port].kind.clone();
                                let compatible = self.compatible_for(graph, from, from_port);
                                let to_node = node.id.clone();
                                let to_port = input.name.clone();
                                cx.widget_action(
                                    self.uid,
                                    FlowCanvasAction::Edit(CanvasEdit::Disconnect { to_node, to_port }),
                                );
                                self.drag = Some(Drag::Wire {
                                    from,
                                    from_port,
                                    ty,
                                    pos: fd.abs,
                                    target: None,
                                });
                                self.compatible = compatible;
                            }
                        }
                    }
                    self.raise_node(&hit_id);
                } else if let Some(index) = self.resize_at(fd.abs) {
                    let graph = self.graph.as_ref().unwrap();
                    let id = graph.nodes[index].id.clone();
                    let size = self.node_size(graph, index);
                    self.raise_node(&id);
                    let selection = Selection::Node(id.clone());
                    if self.selected.as_ref() != Some(&selection) {
                        self.selected = Some(selection.clone());
                        cx.widget_action(self.uid, FlowCanvasAction::Select(Some(selection)));
                    }
                    self.drag = Some(Drag::Resize {
                        index,
                        start: fd.abs,
                        origin: (size.x, size.y),
                        size: (size.x, size.y),
                    });
                } else if let Some(index) = self.node_index_at(fd.abs) {
                    let graph = self.graph.as_ref().unwrap();
                    let node = &graph.nodes[index];
                    let id = node.id.clone();
                    let origin = node.at;
                    self.raise_node(&id);
                    let selection = Selection::Node(id.clone());
                    if self.selected.as_ref() != Some(&selection) {
                        self.selected = Some(selection.clone());
                        cx.widget_action(self.uid, FlowCanvasAction::Select(Some(selection)));
                    }
                    self.drag = Some(Drag::Node {
                        index,
                        start: fd.abs,
                        origin,
                        moved: false,
                    });
                } else if let Some(index) = self.wire_index_at(fd.abs) {
                    if let Some(selection) = self
                        .graph
                        .as_ref()
                        .and_then(|graph| self.edge_selection(graph, index))
                    {
                        if self.selected.as_ref() != Some(&selection) {
                            self.selected = Some(selection.clone());
                            cx.widget_action(self.uid, FlowCanvasAction::Select(Some(selection)));
                        }
                        self.hover = None;
                        self.hover_wire = Some(index);
                        cx.set_cursor(MouseCursor::Hand);
                    }
                } else {
                    self.drag = Some(Drag::Pan {
                        start: fd.abs,
                        origin: self.camera.pan,
                    });
                    cx.set_cursor(MouseCursor::Grabbing);
                }
                self.area.redraw(cx);
            }
            Hit::FingerMove(fm) => {
                let s = self.camera.scale;
                match self.drag.clone() {
                    Some(Drag::Pan { start, origin }) => {
                        self.camera.pan = origin + (fm.abs - start);
                        self.target_pan = self.camera.pan;
                    }
                    Some(Drag::Node {
                        index,
                        start,
                        origin,
                        moved,
                    }) => {
                        let delta = fm.abs - start;
                        let moved = moved || delta.length() > DRAG_THRESHOLD;
                        if moved {
                            cx.promote_finger_capture_over(self.area);
                        }
                        let graph_at = self
                            .graph
                            .as_ref()
                            .and_then(|graph| graph.nodes.get(index))
                            .map(|node| node.at)
                            .unwrap_or(FIRST_AT);
                        let origin = if moved {
                            (graph_at.0 + delta.x / s, graph_at.1 + delta.y / s)
                        } else {
                            origin
                        };
                        self.drag = Some(Drag::Node {
                            index,
                            start,
                            origin,
                            moved,
                        });
                        self.wire_cache_dirty = true;
                    }
                    Some(Drag::Resize {
                        index,
                        start,
                        origin,
                        ..
                    }) => {
                        let delta = (fm.abs - start) / s;
                        let min_height = self
                            .graph
                            .as_ref()
                            .and_then(|graph| graph.nodes.get(index))
                            .map(|node| {
                                min_card_height(Self::full_bleed(node), Self::port_rows(node))
                            })
                            .unwrap_or(MIN_TEXT_LINE_H);
                        self.drag = Some(Drag::Resize {
                            index,
                            start,
                            origin,
                            size: (
                                (origin.0 + delta.x).max(MIN_NODE_WIDTH),
                                (origin.1 + delta.y).max(min_height),
                            ),
                        });
                        self.wire_cache_dirty = true;
                    }
                    Some(Drag::Wire {
                        from,
                        from_port,
                        ty,
                        ..
                    }) => {
                        let target = self.port_at(fm.abs).and_then(|hit| {
                            if hit.output {
                                return None;
                            }
                            self.compatible
                                .contains(&(hit.node, hit.port))
                                .then_some((hit.node, hit.port))
                        });
                        self.drag = Some(Drag::Wire {
                            from,
                            from_port,
                            ty,
                            pos: fm.abs,
                            target,
                        });
                    }
                    None => {}
                }
                self.area.redraw(cx);
            }
            Hit::FingerUp(fu) => {
                cx.set_cursor(if self.hover_wire.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                match self.drag.take() {
                    Some(Drag::Pan { start, .. }) => {
                        if (fu.abs - start).length() <= DRAG_THRESHOLD && self.selected.is_some() {
                            self.selected = None;
                            cx.widget_action(self.uid, FlowCanvasAction::Select(None));
                        }
                    }
                    Some(Drag::Node {
                        index, origin, moved, ..
                    }) => {
                        if moved {
                            if let Some(node) = self
                                .graph
                                .as_ref()
                                .and_then(|graph| graph.nodes.get(index))
                            {
                                cx.widget_action(
                                    self.uid,
                                    FlowCanvasAction::Edit(CanvasEdit::Move {
                                        node: node.id.clone(),
                                        at: origin,
                                    }),
                                );
                            }
                        }
                    }
                    Some(Drag::Resize { index, size, .. }) => {
                        if let Some(node) = self
                            .graph
                            .as_ref()
                            .and_then(|graph| graph.nodes.get(index))
                        {
                            cx.widget_action(
                                self.uid,
                                FlowCanvasAction::Edit(CanvasEdit::Resize {
                                    node: node.id.clone(),
                                    size,
                                }),
                            );
                        }
                    }
                    Some(drag @ Drag::Wire { .. }) => self.finish_wire_drag(cx, drag),
                    None => {}
                }
                self.area.redraw(cx);
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::Delete | KeyCode::Backspace => {
                    if let Some(selection) = self.selected.take() {
                        let edit = match selection {
                            Selection::Node(node) => CanvasEdit::Delete { node },
                            Selection::Edge {
                                to_node, to_port, ..
                            } => CanvasEdit::Disconnect { to_node, to_port },
                        };
                        cx.widget_action(self.uid, FlowCanvasAction::Edit(edit));
                        cx.widget_action(self.uid, FlowCanvasAction::Select(None));
                        self.area.redraw(cx);
                    }
                }
                KeyCode::Home => self.fit(cx),
                KeyCode::Escape => {
                    self.drag = None;
                    self.armed_type = None;
                    self.compatible.clear();
                    self.area.redraw(cx);
                }
                _ => {}
            },
            _ => {}
        }
    }
}
