//! `FlowCanvas`: the graph of one instance — rounded node cards around
//! mounted faces, port circles with the port-type icon, bezier wires in one
//! `DrawVector` batch, drag-to-move, drag-to-connect, a continuously zoomable
//! camera (0.25×–3× by default) and a dark checkerboard behind it all.
//!
//! How the zoom works (decision of 2026-09-03, replacing the three discrete
//! sizes): everything the canvas and the faces draw is laid out ONCE in
//! canvas units inside the canvas's own draw list, and the camera is the
//! draw list's `view_transform` (scale + translate), so text and pictures
//! scale on the GPU and nothing re-flows. Two platform facts shape the code:
//!
//! * per-instance clipping (`draw_clip`) is written by the align-list walk in
//!   PRE-transform units and intersected with every ancestor, so a
//!   transformed child list under a clipped window body could never show
//!   content that lies outside the window in local units. The canvas draws
//!   inside an unclipped root turtle (the mechanism
//!   popups use): its range gets a fresh clip context, and the one clip it
//!   pushes is the inverse-transformed view rect, which after the transform is
//!   exactly the supplied ancestor intersection. Nested canvas transforms are
//!   finalized parent-first after drawing, preserving each absolute viewport.
//! * `Event::hits` compares the raw pointer position with those local rects,
//!   so the faces receive a cloned event whose positions went through the
//!   inverse camera (the host uses [`Camera`] to remap face events); the canvas's own
//!   hit tests (ports, cards) convert the other way. No platform change.
//!
//! Standalone cameras preserve the fixed LOCAL_ORIGIN mapping by default.
//! Embedded canvases and expanded zoom ranges rebase near the visible world
//! before the f32 GPU conversion. Hosts still bound visible depth and node work.

use crate::model::{
    CanvasStyles, CompatiblePorts, FacePort, FaceViewport, GraphView as Graph, NodeFacesScope, NodeStyle,
    NodeView as Node, PortIconOverrides, PortStyle, NODE_WIDTH,
};
use crate::wire_route::{
    self, Obstacle, Point, PortSide, RouteKind, RouteStyle, WireMode, WireRoute,
};
use makepad_widgets::fab_controls::FabValueInput;
use makepad_widgets::makepad_draw::DrawSvg;
use makepad_widgets::makepad_draw::vector::{LineCap, LineJoin};
use makepad_widgets::makepad_platform::event::TouchState;
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use std::any::TypeId;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Legacy local-space offset, used when Camera::render_origin is None.
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
/// The progress overlay on a cable: thicker than the 3 unit base stroke.
const WIRE_FLOW_WIDTH: f32 = 6.0;
const WIRE_FLOW_GLINT: f64 = 18.0;
const WIRE_FLOW_SPEED: f64 = 140.0;
const DRAG_THRESHOLD: f64 = 3.0;
const ZOOM_MIN: f64 = 0.25;
const ZOOM_MAX: f64 = 3.0;
/// Finite limits for the rebased renderer, not a promise of unlimited nesting.
pub const CANVAS_SCALE_MIN: f64 = 1.0 / 4096.0;
pub const CANVAS_SCALE_MAX: f64 = 4096.0;
// Rebase only after substantial screen travel, so ordinary pan frames reuse routes.
const REBASE_DISTANCE_PX: f64 = 4096.0;
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
        || type_id == TypeId::of::<Video>()
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
        (CARD_RADIUS as f64 * 2.0).max(CARD_HEADER_H + port_rows as f64 * PORT_ROW_H + CARD_PAD)
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
/// Public instance fields let other canvases share this renderer.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowGrid {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub cell: f32,
    #[live]
    pub origin: Vec2f,
    #[live]
    pub color_a: Vec4f,
    #[live]
    pub color_b: Vec4f,
}

/// One card, including its shadow, so the shadow follows the card's exact
/// transformed rectangle rather than a separately batched vector estimate.
/// Public instance fields let sibling canvases customize the same node chrome.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowCard {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub border_size: f32,
    #[live]
    pub border_radius: f32,
    #[live]
    pub outline_color: Vec4f,
    #[live]
    pub outline_size: f32,
    #[live]
    pub shadow_color: Vec4f,
    #[live]
    pub shadow_radius: f32,
    #[live]
    pub shadow_offset: Vec2f,
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
    /// Move one existing wire as a single graph revision. The old input and
    /// ordered entry key identify exactly the cable picked up by the hand.
    Reconnect {
        from_node: String,
        from_port: String,
        old_to_node: String,
        old_to_port: String,
        old_key: Option<u64>,
        to_node: String,
        to_port: String,
    },
    Disconnect {
        to_node: String,
        to_port: String,
        /// The entry to drop on an ordered input; `None` clears a one
        /// input (or every entry of an ordered one).
        key: Option<u64>,
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
        /// The entry key on an ordered input, so a selected wire is one
        /// wire even where two share their ends.
        key: Option<u64>,
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
    /// A node was double-clicked: the app decides whether it opens (a
    /// container's inner graph, say).
    Open {
        node: String,
    },
}

#[derive(Clone, Debug)]
enum Drag {
    Pan {
        start: DVec2,
        origin: DVec2,
        navigate: bool,
        select: bool,
    },
    Node {
        index: usize,
        start: DVec2,
        initial: (f64, f64),
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
        /// Kept in the graph until release; Escape or a graph replacement
        /// cancels pickup without ever publishing a destructive edit.
        picked_up: Option<EdgeIndex>,
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
/// space. Local units use `render_origin`, or the legacy LOCAL_ORIGIN offset.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub view: Rect,
    pub pan: DVec2,
    pub scale: f64,
    /// World origin of render-local coordinates. `None` preserves the original
    /// fixed LOCAL_ORIGIN mapping. Embedded/expanded-range canvases rebase this
    /// near the visible world; pan and public graph geometry stay world-based.
    pub render_origin: Option<DVec2>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            view: Rect::default(),
            pan: dvec2(0.0, 0.0),
            scale: 1.0,
            render_origin: None,
        }
    }
}

impl Camera {
    pub fn screen_to_local(&self, screen: DVec2) -> DVec2 {
        let world = (screen - self.view.pos - self.pan) / self.scale;
        world - self.origin()
    }

    pub fn local_to_screen(&self, local: DVec2) -> DVec2 {
        self.view.pos + self.pan + (local + self.origin()) * self.scale
    }

    /// Legacy fixed-origin conversion; use world_to_render for an active camera.
    pub fn world_to_local(world: (f64, f64)) -> DVec2 {
        dvec2(LOCAL_ORIGIN + world.0, LOCAL_ORIGIN + world.1)
    }

    /// Legacy fixed-origin conversion; use render_to_world for an active camera.
    pub fn local_to_world(local: DVec2) -> (f64, f64) {
        (local.x - LOCAL_ORIGIN, local.y - LOCAL_ORIGIN)
    }

    pub fn screen_to_world(&self, screen: DVec2) -> (f64, f64) {
        let world = (screen - self.view.pos - self.pan) / self.scale;
        (world.x, world.y)
    }

    fn origin(&self) -> DVec2 {
        self.render_origin
            .unwrap_or(dvec2(-LOCAL_ORIGIN, -LOCAL_ORIGIN))
    }

    /// Instance-aware conversion, including an optional render origin.
    pub fn world_to_render(&self, world: (f64, f64)) -> DVec2 {
        dvec2(world.0, world.1) - self.origin()
    }

    pub fn render_to_world(&self, local: DVec2) -> (f64, f64) {
        let world = local + self.origin();
        (world.x, world.y)
    }

    fn rebase_at(&mut self, screen: DVec2) {
        let visible = (screen - self.view.pos - self.pan) / self.scale;
        if self
            .render_origin
            .is_none_or(|origin| (visible - origin).length() * self.scale > REBASE_DISTANCE_PX)
        {
            self.render_origin = Some(visible);
        }
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
        let t = self.view.pos + self.pan + self.origin() * self.scale;
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
            translation: self.view.pos + self.pan + self.origin() * self.scale,
        }
    }
}

/// Absolute window-space camera and ancestor clip for one canvas instance.
#[derive(Clone, Copy, Debug)]
pub struct CanvasViewport {
    pub camera: Camera,
    pub clip: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasViewportError {
    NonFinite,
    ScaleOutOfRange,
    InvalidRect,
}

impl CanvasViewport {
    fn checked(self) -> Result<Self, CanvasViewportError> {
        for rect in [self.camera.view, self.clip] {
            if !finite_point(rect.pos)
                || !finite_point(rect.size)
                || !finite_point(rect.pos + rect.size)
            {
                return Err(CanvasViewportError::NonFinite);
            }
            if rect.size.x < 0.0 || rect.size.y < 0.0 {
                return Err(CanvasViewportError::InvalidRect);
            }
        }
        if !finite_point(self.camera.pan)
            || !self.camera.scale.is_finite()
            || self
                .camera
                .render_origin
                .is_some_and(|origin| !finite_point(origin))
        {
            return Err(CanvasViewportError::NonFinite);
        }
        if !(CANVAS_SCALE_MIN..=CANVAS_SCALE_MAX).contains(&self.camera.scale) {
            return Err(CanvasViewportError::ScaleOutOfRange);
        }
        Ok(Self {
            clip: intersect_rect(self.camera.view, self.clip),
            ..self
        })
    }
}

/// Measured geometry in graph-world units, independent of render rebasing.
#[derive(Clone, Copy, Debug)]
pub struct NodeGeometry {
    pub card: Rect,
    pub content: Rect,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CanvasEventPolicy {
    pub pointer: bool,
    pub keyboard: bool,
    pub navigation: bool,
}

impl CanvasEventPolicy {
    pub const ALL: Self = Self {
        pointer: true,
        keyboard: true,
        navigation: true,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasNodeRegion {
    Header,
    Content,
    Body,
    Resize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasPick {
    Background,
    Node {
        node: String,
        region: CanvasNodeRegion,
    },
    Port {
        node: String,
        port: String,
        output: bool,
    },
    Wire(Selection),
}

/// A window-coordinate child, associated with the card that owns its body.
/// Its id must be distinct from ordinary face roots. Do not also mount this
/// widget beneath a face root, which would inverse-map its events twice.
#[derive(Clone)]
pub struct EmbeddedCanvasRoot {
    pub id: LiveId,
    pub node: String,
    pub widget: WidgetRef,
}

/// A named viewport inlet/outlet continuing a parent port into a child graph.
/// `outer` and `slot` are window coordinates. `node` and `port` name the
/// child graph's boundary node and its one inner port; `output` is the
/// *container's* direction, so the inner port faces the other way. When the
/// node is a boundary node ([`FlowCanvas::set_boundary_nodes`]) the slot IS
/// that node: no card is drawn for it and the graph's real wires end at the
/// slot's inward socket.
#[derive(Clone, Debug)]
pub struct BoundaryLink {
    pub node: String,
    pub port: String,
    pub output: bool,
    pub on_right: bool,
    pub label: String,
    pub kind: String,
    pub outer: DVec2,
    pub slot: Rect,
}

/// Boundary links and boundary nodes one canvas takes. A host's component
/// interface is bounded well below this.
const MAX_BOUNDARY: usize = 256;

/// Restrained accents that pair an interface port outside a group with its
/// slot inside it. They sit beside the type colours, never instead of them:
/// a cable keeps its type colour and its status, the accent marks only the
/// boundary (the slot's edge and socket, the continuation, the outer
/// connector, the last stretch of a wire at the slot).
const BOUNDARY_ACCENTS: [[f32; 3]; 8] = [
    [0.96, 0.69, 0.26],
    [0.36, 0.78, 0.92],
    [0.84, 0.50, 0.90],
    [0.56, 0.86, 0.47],
    [0.97, 0.49, 0.45],
    [0.45, 0.62, 0.98],
    [0.93, 0.85, 0.40],
    [0.42, 0.85, 0.74],
];

/// The accent of every link, in order. A link's accent comes from its
/// interface identity (the boundary's name and direction), so it is the same
/// on every draw, in the child that draws the slot and in the parent that
/// draws the continuation. Two neighbours on one rail never share one: the
/// later of the two moves to the next accent.
fn boundary_accents(links: &[BoundaryLink]) -> Vec<Vec4f> {
    let mut indices: Vec<usize> = Vec::with_capacity(links.len());
    for (at, link) in links.iter().enumerate() {
        // FNV-1a: the same on every platform and toolchain.
        let mut hash = 0x811c_9dc5u32;
        for byte in link.node.bytes().chain([if link.output { b'>' } else { b'<' }]) {
            hash = (hash ^ byte as u32).wrapping_mul(0x0100_0193);
        }
        let mut index = hash as usize % BOUNDARY_ACCENTS.len();
        let neighbour = (0..at).rev().find(|before| links[*before].on_right == link.on_right).map(|before| indices[before]);
        if neighbour == Some(index) {
            index = (index + 1) % BOUNDARY_ACCENTS.len();
        }
        indices.push(index);
    }
    indices
        .into_iter()
        .map(|index| {
            let [r, g, b] = BOUNDARY_ACCENTS[index];
            vec4(r, g, b, 1.0)
        })
        .collect()
}

fn same_link(a: &BoundaryLink, b: &BoundaryLink) -> bool {
    a.node == b.node && a.port == b.port && a.output == b.output && a.on_right == b.on_right
        && a.label == b.label && a.kind == b.kind && a.outer == b.outer
        && a.slot.pos == b.slot.pos && a.slot.size == b.slot.size
}

fn finite_point(point: DVec2) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn intersect_rect(a: Rect, b: Rect) -> Rect {
    let pos = dvec2(a.pos.x.max(b.pos.x), a.pos.y.max(b.pos.y));
    let end = dvec2(
        (a.pos.x + a.size.x).min(b.pos.x + b.size.x),
        (a.pos.y + a.size.y).min(b.pos.y + b.size.y),
    );
    Rect {
        pos,
        size: dvec2((end.x - pos.x).max(0.0), (end.y - pos.y).max(0.0)),
    }
}

fn nonempty(rect: Rect) -> bool {
    rect.size.x > 0.0 && rect.size.y > 0.0
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
    key: Option<u64>,
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

/// The cable just connected from a drag, by the names its edge will have:
/// its preview route seeds the connected cable's cache in the next
/// `set_graph`, so connecting cannot change the shape the preview had.
struct PendingConnect {
    from: String,
    from_port: String,
    to: String,
    to_port: String,
    route: WireRoute,
}

/// `MAKEPAD_FLOW_ROUTE_TRACE`, read once: print every change of a cached
/// route or of the preview to stderr, for driving the canvas and comparing
/// the shapes it chose. Nothing is printed when it is unset or `0`.
fn route_trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| {
        std::env::var_os("MAKEPAD_FLOW_ROUTE_TRACE").is_some_and(|value| !value.is_empty() && value != "0")
    })
}

/// What one routing asks for. A graph edge and the wire being dragged both
/// build theirs through `route_request`, so a preview cannot take a shape
/// the connected cable then abandons for no reason but a different
/// obstacle set or corridor offset.
struct RouteRequest {
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: Vec<Obstacle>,
    offset: f64,
}

/// How a source node's progress shows on its outgoing cables.
#[derive(Clone, Copy, Debug)]
enum WireFlow {
    /// Filled from the source port up to this fraction of the route.
    Fill(f64),
    /// Running without a known fraction: a segment sweeps source to target.
    Sweep,
    /// Failed or cancelled: nothing travels further than the source port.
    Stopped,
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
    /// Last completed draw, before a pending Fit changes the next frame.
    #[rust]
    drawn_viewport: Option<CanvasViewport>,
    #[rust]
    embedded: bool,
    #[rust(0.25f64)]
    zoom_min: f64,
    #[rust(3.0f64)]
    zoom_max: f64,
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
    /// Host port-style keys for the icons of exact sockets, keyed by node id,
    /// port name and `true` for an output. Nothing but the socket icon reads it.
    #[rust]
    port_icon_overrides: PortIconOverrides,
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
    /// Preview geometry follows the same retained route policy as real wires.
    #[rust]
    preview_wire: Option<CachedWire>,
    #[rust]
    pending_connect: Option<PendingConnect>,
    /// The edge the preview was last routed for, so a release seeds the
    /// connected cable only from a preview drawn to that same target.
    #[rust]
    preview_edge: Option<EdgeIndex>,
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
    embedded_roots: Vec<EmbeddedCanvasRoot>,
    #[rust]
    boundary_links: Vec<BoundaryLink>,
    /// Nodes of the graph that are this viewport's boundary: shown as their
    /// slot, never as a card. Empty for an ordinary flat canvas.
    #[rust]
    boundary_nodes: HashSet<String>,
    /// A run attachment keeps the face visible beneath a subtle lock veil;
    /// the host separately disables the mounted controls.
    #[rust]
    faces_locked: bool,
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
            // A boundary node has no card, so its face is not drawn; the
            // area it kept from when it was one must not take events.
            if self.boundary_nodes.iter().any(|node| LiveId::from_str(node) == *id) {
                continue;
            }
            visit(*id, root.clone());
        }
        for root in &self.embedded_roots {
            visit(root.id, root.widget.clone());
        }
    }
    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        if !self.contains_point(point) || self.point_over_chrome(point) {
            return;
        }
        let front = match self.pick(point) {
            Some(CanvasPick::Node { node, region: CanvasNodeRegion::Content }) => Some(node),
            _ => None,
        };
        for root in &self.embedded_roots {
            if front.as_deref() == Some(root.node.as_str()) {
                root.widget.find_widgets_from_point(cx, point, found);
            }
        }
        let local = self.interaction_camera().screen_to_local(point);
        for (id, root) in &self.face_roots {
            // Named node roots use the same node-id convention as set_z_order.
            // Unassociated host roots retain their previous discovery behavior.
            if self.node_index.keys().any(|node| LiveId::from_str(node) == *id)
                && front.as_ref().is_none_or(|node| LiveId::from_str(node) != *id)
            {
                continue;
            }
            if self.boundary_nodes.iter().any(|node| LiveId::from_str(node) == *id) {
                continue;
            }
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

    /// The completed frame's viewport, including any render-origin adjustment.
    pub fn viewport(&self) -> Option<CanvasViewport> {
        self.drawn_viewport
    }

    fn interaction_camera(&self) -> Camera {
        self.drawn_viewport.map_or(self.camera, |view| view.camera)
    }

    fn contains_point(&self, point: DVec2) -> bool {
        let clip = self
            .drawn_viewport
            .map_or(self.camera.view, |view| view.clip);
        nonempty(clip) && clip.contains(point)
    }

    pub fn has_canvas_drag(&self) -> bool {
        self.drag.is_some()
    }

    /// Abandon a gesture without publishing an edit. The owner must still
    /// deliver its eventual release so platform pointer capture can retire.
    pub fn cancel_gesture(&mut self, cx: &mut Cx) {
        self.drag = None;
        self.preview_wire = None;
        self.armed_type = None;
        self.compatible.clear();
        self.wire_cache_dirty = true;
        self.redraw(cx);
    }

    /// Register only mounted embedded canvases, with unique ids. This does not
    /// draw or route events to them; the host owns hierarchy and gesture routing.
    pub fn set_embedded_roots(&mut self, cx: &mut Cx, roots: Vec<EmbeddedCanvasRoot>) {
        self.embedded_roots = roots;
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    /// Expanded ranges opt into render rebasing. Defaults remain 0.25..3.
    /// Limits bound numeric inputs, not a validated arbitrary nesting depth.
    pub fn set_zoom_range(
        &mut self,
        cx: &mut Cx,
        min: f64,
        max: f64,
    ) -> Result<(), CanvasViewportError> {
        if !min.is_finite() || !max.is_finite() {
            return Err(CanvasViewportError::NonFinite);
        }
        if min > max || min < CANVAS_SCALE_MIN || max > CANVAS_SCALE_MAX {
            return Err(CanvasViewportError::ScaleOutOfRange);
        }
        self.zoom_min = min;
        self.zoom_max = max;
        self.target_scale = self.target_scale.clamp(min, max);
        self.camera.scale = self.camera.scale.clamp(min, max);
        self.redraw(cx);
        Ok(())
    }

    pub fn node_geometry(&self, node: &str) -> Option<NodeGeometry> {
        let graph = self.graph.as_ref()?;
        let index = *self.node_index.get(node)?;
        Some(self.geometry(graph, index))
    }

    /// Refresh only presentation, preserving the camera, selection, mounted
    /// faces and any current pointer capture. The node order cannot change.
    pub fn update_graph_layout(&mut self, cx: &mut Cx, next: &Graph) -> bool {
        let Some(current) = self.graph.as_ref() else { return false; };
        if current.nodes.len() != next.nodes.len() { return false; }
        let mut projection = current.clone();
        for (node, next) in projection.nodes.iter_mut().zip(&next.nodes) {
            node.at = next.at;
            node.size = next.size;
            node.flip = next.flip;
        }
        if projection != *next { return false; }
        for node in &next.nodes {
            self.set_node_layout(cx, &node.id, node.at, node.size, node.flip);
        }
        true
    }

    /// Refresh one existing card without disturbing an ongoing gesture.
    pub fn set_node_layout(&mut self, cx: &mut Cx, node: &str, at: (f64, f64), size: Option<(f64, f64)>, flip: bool) {
        let Some(index) = self.node_index.get(node).copied() else { return; };
        let Some(graph) = self.graph.as_mut() else { return; };
        let node = &mut graph.nodes[index];
        if node.at == at && node.size == size && node.flip == flip { return; }
        node.at = at;
        node.size = size;
        node.flip = flip;
        self.wire_cache_dirty = true;
        self.auto_flip_pending = false;
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    /// Reconcile a move or resize without replacing the canvas's display facing.
    pub fn set_node_geometry(&mut self, cx: &mut Cx, node: &str, at: (f64, f64), size: Option<(f64, f64)>) {
        let Some(index) = self.node_index.get(node).copied() else { return; };
        let Some(flip) = self.graph.as_ref().map(|graph| graph.nodes[index].flip) else { return; };
        self.set_node_layout(cx, node, at, size, flip);
    }

    /// Host-owned inlet/outlet rails for an embedded viewport. Set before
    /// draw_embedded; ordinary flat canvases leave this list empty.
    pub fn set_boundary_links(&mut self, links: Vec<BoundaryLink>) {
        let mut seen = HashSet::new();
        let links: Vec<BoundaryLink> = links.into_iter().filter(|link| {
            let Some(node) = self.graph.as_ref().and_then(|graph| {
                graph.nodes.iter().find(|node| node.id == link.node)
            }) else { return false; };
            let ports = if link.output { &node.inputs } else { &node.outputs };
            self.boundary_nodes.contains(&link.node)
                && ports.iter().any(|port| port.name == link.port)
                && finite_point(link.outer) && finite_point(link.slot.pos)
                && finite_point(link.slot.size) && nonempty(link.slot)
                && seen.insert((link.node.clone(), link.port.clone(), link.output))
        }).take(MAX_BOUNDARY).collect();
        // A slot is a wire endpoint: when one moves, appears, changes side or
        // goes, the routes that end there are stale.
        let same = links.len() == self.boundary_links.len()
            && links.iter().zip(&self.boundary_links).all(|(a, b)| same_link(a, b));
        if !same {
            self.wire_cache_dirty = true;
        }
        self.boundary_links = links;
    }

    /// The nodes of the current graph that are this viewport's boundary: a
    /// component's interface inputs and outputs. Call it after `set_graph`
    /// and before reading `graph_bounds`; the links' geometry may follow
    /// later. Such a node is represented by its slot alone: it has no card,
    /// no face, no label, cannot be dragged or resized, is no obstacle, does
    /// not count for bounds or fit and is never auto-flipped. It stays the
    /// logical node it is: its real wires end at the slot's inward socket,
    /// its real port is hit there, and pressing the slot selects it. With no
    /// boundary nodes a canvas draws Input and Output cards as ever.
    pub fn set_boundary_nodes(&mut self, cx: &mut Cx, nodes: Vec<String>) {
        let nodes: HashSet<String> = nodes.into_iter().take(MAX_BOUNDARY).collect();
        if nodes == self.boundary_nodes {
            return;
        }
        // A gesture or a hover may hold the index of a card that is about
        // to stop being one.
        self.cancel_gesture(cx);
        self.hover = None;
        self.hover_wire = None;
        self.boundary_nodes = nodes;
        let links = std::mem::take(&mut self.boundary_links);
        self.set_boundary_links(links);
        if let Some(graph) = self.graph.as_ref() {
            for (index, node) in graph.nodes.iter().enumerate() {
                if self.boundary_nodes.contains(&node.id) {
                    // Its retained card list is not recorded again.
                    if let Some(list) = self.card_draw_lists.get_mut(index) {
                        *list = None;
                    }
                }
            }
        }
        self.wire_cache.iter_mut().for_each(|cached| *cached = None);
        self.wire_cache_dirty = true;
        self.auto_flip_pending = self.graph.is_some();
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    fn is_boundary(&self, graph: &Graph, index: usize) -> bool {
        !self.boundary_nodes.is_empty() && self.boundary_nodes.contains(&graph.nodes[index].id)
    }

    /// The link that stands for `port` of boundary node `index`, matched
    /// exactly: node, port name, and the direction (a container input is the
    /// inner node's output and the other way round).
    fn boundary_link(&self, graph: &Graph, index: usize, port: usize, output: bool) -> Option<&BoundaryLink> {
        if !self.is_boundary(graph, index) {
            return None;
        }
        let node = &graph.nodes[index];
        let name = if output { &node.outputs.get(port)?.name } else { &node.inputs.get(port)?.name };
        self.boundary_links
            .iter()
            .find(|link| link.node == node.id && link.port == *name && link.output != output)
    }

    /// A slot's socket in this canvas's local units: the middle of the edge
    /// that faces into the graph.
    fn boundary_socket(&self, link: &BoundaryLink) -> DVec2 {
        self.interaction_camera().screen_to_local(dvec2(
            if link.on_right { link.slot.pos.x } else { link.slot.pos.x + link.slot.size.x },
            link.slot.pos.y + link.slot.size.y * 0.5,
        ))
    }

    /// Which way a port faces: a slot on the right rail faces left into the
    /// graph and one on the left rail faces right, whatever the node's flip;
    /// every other port follows its card.
    fn side_at_flip(&self, graph: &Graph, index: usize, port: usize, output: bool, flip: f64) -> PortSide {
        match self.boundary_link(graph, index, port, output) {
            Some(link) if link.on_right => PortSide::Left,
            Some(_) => PortSide::Right,
            None => Self::port_side_at_flip(output, flip),
        }
    }

    fn side_of(&self, graph: &Graph, index: usize, port: usize, output: bool) -> PortSide {
        let flip = self
            .flip_positions
            .get(index)
            .copied()
            .unwrap_or(if graph.nodes[index].flip { 1.0 } else { 0.0 });
        self.side_at_flip(graph, index, port, output, flip)
    }

    /// What a routed wire must go round, in this canvas's local units, with
    /// the ordinary clearance: every card that is drawn, and every boundary
    /// slot. A slot is a body on the rail like a card is on the canvas; a
    /// wire that ends at its socket leaves through that socket under the
    /// router's endpoint exemption, exactly as a wire leaves a card's port,
    /// and never crosses the slot. Slots come in through the same camera as
    /// their sockets, so obstacles and anchors agree. One collector for the
    /// route cache, the auto-flip scoring and the drag preview, so the
    /// three never drift apart.
    ///
    /// Slots stand in rows on a rail, often closer together than the
    /// clearance. A slot's envelope is therefore inflated by the full
    /// clearance sideways but above and below only by half the gap to the
    /// nearest slot whose horizontal span overlaps its own (never less than
    /// nothing, never more than the clearance). So the envelopes of two
    /// slots that do not touch never contain each other's socket, and each
    /// socket has exactly one owner; slots whose bodies really overlap stay
    /// obstructed, with no exemption. Each slot keeps its raw body, so the
    /// router's fixed deflations for its narrower tiers cannot open the
    /// thin envelope into the slot itself. Cards are inflated as ever.
    fn route_obstacles(&self, graph: &Graph) -> Vec<Obstacle> {
        const CARD_CLEARANCE: f64 = 12.0;
        let mut obstacles: Vec<Obstacle> = (0..graph.nodes.len())
            .filter(|index| !self.is_boundary(graph, *index))
            .map(|index| {
                let rect = self.card_rect(graph, index);
                Obstacle::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y).inflate(CARD_CLEARANCE)
            })
            .collect();
        let camera = self.interaction_camera();
        if !camera.scale.is_finite() || camera.scale <= 0.0 {
            return obstacles;
        }
        let slots: Vec<Rect> = self
            .boundary_links
            .iter()
            .map(|link| Rect { pos: camera.screen_to_local(link.slot.pos), size: link.slot.size / camera.scale })
            .filter(|slot| finite_point(slot.pos) && finite_point(slot.size))
            .collect();
        for (at, slot) in slots.iter().enumerate() {
            let (top, bottom) = (slot.pos.y, slot.pos.y + slot.size.y);
            let gap = slots
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != at)
                .filter(|(_, other)| other.pos.x < slot.pos.x + slot.size.x && slot.pos.x < other.pos.x + other.size.x)
                .map(|(_, other)| {
                    let (other_top, other_bottom) = (other.pos.y, other.pos.y + other.size.y);
                    if other_top >= bottom { other_top - bottom } else if other_bottom <= top { top - other_bottom } else { 0.0 }
                })
                .fold(f64::INFINITY, f64::min);
            let vertical = (gap * 0.5).clamp(0.0, CARD_CLEARANCE);
            obstacles.push(
                Obstacle::from_xywh(slot.pos.x, slot.pos.y, slot.size.x, slot.size.y)
                    .with_body()
                    .inflate_xy(CARD_CLEARANCE, vertical),
            );
        }
        obstacles
    }

    /// An edge with a boundary end whose slot has no geometry (yet): it has
    /// nowhere to go and is not routed or drawn.
    fn edge_unplaced(&self, graph: &Graph, edge: EdgeIndex) -> bool {
        (self.is_boundary(graph, edge.from) && self.boundary_link(graph, edge.from, edge.from_port, true).is_none())
            || (self.is_boundary(graph, edge.to) && self.boundary_link(graph, edge.to, edge.to_port, false).is_none())
    }

    fn geometry(&self, graph: &Graph, index: usize) -> NodeGeometry {
        let local = self.card_rect(graph, index);
        let node = &graph.nodes[index];
        let content = card_content_rect(local, Self::full_bleed(node), Self::port_rows(node)).rect;
        let origin = self.camera.origin();
        NodeGeometry {
            card: Rect {
                pos: local.pos + origin,
                ..local
            },
            content: Rect {
                pos: content.pos + origin,
                ..content
            },
        }
    }

    /// Wire tip/notch in graph-world coordinates, including animated facing.
    pub fn port_anchor(&self, node: &str, port: &str, output: bool) -> Option<DVec2> {
        let graph = self.graph.as_ref()?;
        let index = *self.node_index.get(node)?;
        let ports = if output {
            &graph.nodes[index].outputs
        } else {
            &graph.nodes[index].inputs
        };
        let port = ports.iter().position(|item| item.name == port)?;
        Some(self.wire_anchor(graph, index, port, output) + self.camera.origin())
    }

    pub fn graph_bounds(&self) -> Option<Rect> {
        let graph = self.graph.as_ref()?;
        if graph.nodes.is_empty() {
            return None;
        }
        // A group of nothing but its boundary (a pass-through) has no cards
        // and so no bounds, like an empty graph.
        if (0..graph.nodes.len()).all(|index| self.is_boundary(graph, index)) {
            return None;
        }
        let mut min = dvec2(f64::MAX, f64::MAX);
        let mut max = dvec2(f64::MIN, f64::MIN);
        for index in 0..graph.nodes.len() {
            if self.is_boundary(graph, index) {
                continue;
            }
            let card = self.geometry(graph, index).card;
            min.x = min.x.min(card.pos.x);
            min.y = min.y.min(card.pos.y - LABEL_H);
            max.x = max.x.max(card.pos.x + card.size.x);
            max.y = max.y.max(card.pos.y + card.size.y);
        }
        Some(Rect {
            pos: min,
            size: max - min,
        })
    }

    pub fn pick(&self, point: DVec2) -> Option<CanvasPick> {
        if !self.contains_point(point) || self.point_over_chrome(point) {
            return None;
        }
        let Some(graph) = &self.graph else {
            return Some(CanvasPick::Background);
        };
        if let Some(hit) = self.port_at(point) {
            let node = &graph.nodes[hit.node];
            let ports = if hit.output {
                &node.outputs
            } else {
                &node.inputs
            };
            return Some(CanvasPick::Port {
                node: node.id.clone(),
                port: ports[hit.port].name.clone(),
                output: hit.output,
            });
        }
        if let Some(index) = self.node_index_at(point) {
            let geometry = self.geometry(graph, index);
            let world = self.interaction_camera().screen_to_world(point);
            let region = if self.resize_at(point) == Some(index) {
                CanvasNodeRegion::Resize
            } else if geometry.content.contains(dvec2(world.0, world.1)) {
                CanvasNodeRegion::Content
            } else if world.1 < geometry.card.pos.y + CARD_HEADER_H {
                CanvasNodeRegion::Header
            } else {
                CanvasNodeRegion::Body
            };
            return Some(CanvasPick::Node {
                node: graph.nodes[index].id.clone(),
                region,
            });
        }
        if let Some(node) = self.boundary_at(point) {
            return Some(CanvasPick::Node { node: node.to_string(), region: CanvasNodeRegion::Body });
        }
        if let Some(selection) = self
            .wire_index_at(point)
            .and_then(|index| self.edge_selection(graph, index))
        {
            return Some(CanvasPick::Wire(selection));
        }
        Some(CanvasPick::Background)
    }
    pub fn wire_mode(&self) -> WireMode {
        self.wire_mode
    }

    pub fn set_wire_mode(&mut self, cx: &mut Cx, mode: WireMode) {
        if self.wire_mode == mode {
            return;
        }
        self.wire_mode = mode;
        self.preview_wire = None;
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

    /// Replaces the per-socket icon keys. A key names a port style; a socket
    /// without a key, or whose style has no icon, keeps its kind's icon.
    /// Keys for sockets absent from the current graph are dropped, and a
    /// replacement graph drops those it no longer has.
    pub fn set_port_icon_overrides(&mut self, cx: &mut Cx, overrides: PortIconOverrides) {
        if self.port_icon_overrides == overrides {
            return;
        }
        self.port_icon_overrides = overrides;
        self.prune_port_icon_overrides();
        self.redraw(cx);
    }

    /// The face roots the app mounted for the bound instance; cleared before
    /// the app frees that isolate.
    pub fn set_face_roots(&mut self, cx: &mut Cx, roots: Vec<(LiveId, WidgetRef)>) {
        self.face_roots = roots;
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    pub fn set_faces_locked(&mut self, cx: &mut Cx, locked: bool) {
        if self.faces_locked != locked {
            self.faces_locked = locked;
            self.redraw(cx);
        }
    }

    pub fn set_graph(&mut self, cx: &mut Cx, graph: Option<Graph>) {
        // Drag indices belong to the previous projection and cannot survive
        // a replacement, even when a same-named node moves to another index.
        self.cancel_gesture(cx);
        // Geometry belongs to the old projection; a host supplies the new
        // slots before drawing the replacement graph.
        self.boundary_links.clear();
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
                        key: edge.key,
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
        self.parallel_offsets = edges
            .iter()
            .enumerate()
            .map(|(index, edge)| Self::lane_offset(&edges, index, edge))
            .collect();
        // A Connect's preview is consumed by whichever graph follows it,
        // even one that did not take the edge: it never seeds a later,
        // unrelated edit.
        let mut pending = self.pending_connect.take();
        self.wire_cache = if edges == old_edges {
            old_wire_cache
        } else {
            // A cable that survives the edit keeps its cached route, by its
            // full identity including an ordered input's entry key, so it
            // is re-routed from the shape it had rather than from nothing.
            // Only a genuinely new cable with no survivor of its own takes
            // the preview it was connected from.
            let identity = |graph: &Graph, edge: &EdgeIndex| {
                let from = graph.nodes.get(edge.from)?;
                let to = graph.nodes.get(edge.to)?;
                Some((
                    from.id.clone(),
                    from.outputs.get(edge.from_port)?.name.clone(),
                    to.id.clone(),
                    to.inputs.get(edge.to_port)?.name.clone(),
                    edge.key,
                ))
            };
            let mut survivors: HashMap<(String, String, String, String, Option<u64>), CachedWire> = old
                .as_ref()
                .map(|old| {
                    old_edges
                        .iter()
                        .zip(old_wire_cache)
                        .filter_map(|(edge, cached)| Some((identity(old, edge)?, cached?)))
                        .collect()
                })
                .unwrap_or_default();
            edges
                .iter()
                .map(|edge| {
                    let identity = graph.as_ref().and_then(|graph| identity(graph, edge))?;
                    if let Some(cached) = survivors.remove(&identity) {
                        return Some(cached);
                    }
                    let (from, from_port, to, to_port, _) = &identity;
                    let seeds = pending.as_ref().is_some_and(|pending| {
                        pending.from == *from && pending.from_port == *from_port && pending.to == *to && pending.to_port == *to_port
                    });
                    seeds.then(|| CachedWire { key: 0, route: pending.take().expect("checked just above").route })
                })
                .collect()
        };
        self.wire_cache_dirty = true;
        self.heights = heights;
        self.flip_positions = flip_positions;
        self.edges = edges;
        self.card_draw_lists = card_draw_lists;
        self.node_index = node_index;
        self.graph = graph;
        self.prune_port_icon_overrides();
        if route_trace() {
            let nodes = self.graph.as_ref().map_or(0, |graph| graph.nodes.len());
            let full_bleed = self
                .graph
                .as_ref()
                .map_or(0, |graph| graph.nodes.iter().filter(|node| Self::full_bleed(node)).count());
            // Nodes that arrived, or whose full-bleed flag changed.
            let changed: Vec<String> = self
                .graph
                .as_ref()
                .map(|graph| {
                    graph
                        .nodes
                        .iter()
                        .filter(|node| {
                            old_index
                                .get(&node.id)
                                .and_then(|index| old.as_ref()?.nodes.get(*index))
                                .is_none_or(|before| Self::full_bleed(before) != Self::full_bleed(node))
                        })
                        .map(|node| format!("{}:{}", node.id, if Self::full_bleed(node) { "full" } else { "framed" }))
                        .collect()
                })
                .unwrap_or_default();
            eprintln!(
                "flow graph nodes {nodes} edges {} full_bleed {full_bleed} changed [{}]",
                self.edges.len(),
                changed.join(" "),
            );
        }
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
        if !self.contains_point(abs) || self.point_over_chrome(abs) { return; }
        let node = self.node_index_at(abs).and_then(|index| {
            self.graph
                .as_ref()
                .and_then(|graph| graph.nodes.get(index))
                .map(|node| node.id.clone())
        });
        let node = node.or_else(|| self.boundary_at(abs).map(str::to_string));
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
        let mut interactive = false;
        let mut handled_widget_found = handled.is_empty();
        self.find_widgets_from_point(cx, abs, &mut |widget| {
                handled_widget_found |= widget.area() == handled;
                if !interactive
                    && widget
                        .widget_type_id()
                        .is_some_and(is_interactive_face_type)
                {
                    interactive = true;
                }
        });
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
        self.interaction_camera()
    }

    /// Put the view where `camera` says, at once: pan and scale, no ease,
    /// no pending fit. The view rect stays the widget's own. An app that
    /// keeps one camera per graph it shows restores it with this.
    pub fn set_camera(&mut self, cx: &mut Cx, camera: Camera) {
        if !finite_point(camera.pan) || !camera.scale.is_finite()
            || !(CANVAS_SCALE_MIN..=CANVAS_SCALE_MAX).contains(&camera.scale)
        { return; }
        self.camera.pan = camera.pan;
        self.camera.scale = camera.scale;
        self.target_pan = camera.pan;
        self.target_scale = camera.scale;
        self.fit_pending = 0;
        // The same notification a gesture ends with, so whatever mirrors
        // the zoom follows a restored camera too.
        cx.widget_action(self.uid, FlowCanvasAction::Camera { scale: camera.scale });
        self.redraw(cx);
    }

    /// Select a node or a wire (or nothing) as if the user had: what an
    /// app restores when it shows a graph again.
    pub fn set_selection(&mut self, cx: &mut Cx, selection: Option<Selection>) {
        if let Some(node) = selection.as_ref().and_then(Selection::node) {
            let node = node.to_string();
            self.raise_node(&node);
        }
        self.selected = selection;
        self.redraw(cx);
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
        if self.embedded { return; }
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
        let cards = (0..graph.nodes.len()).filter(|index| !self.is_boundary(graph, *index)).count();
        if cards == 0 || view.size.x <= 0.0 {
            self.target_pan = dvec2(0.0, 0.0);
            self.target_scale = 1.0;
            self.next_frame = cx.new_next_frame();
            return;
        }
        let mut min = dvec2(f64::MAX, f64::MAX);
        let mut max = dvec2(f64::MIN, f64::MIN);
        for (index, node) in graph.nodes.iter().enumerate() {
            if self.is_boundary(graph, index) {
                continue;
            }
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
            .clamp(self.zoom_min, self.zoom_max.min(1.0).max(self.zoom_min));
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
        if self.embedded || !scale.is_finite() || !finite_point(anchor) { return; }
        let scale = scale.clamp(self.zoom_min, self.zoom_max);
        // The world point under the anchor stays under it at the end of
        // the ease.
        let target = Camera {
            pan: self.target_pan,
            scale: self.target_scale,
            ..self.camera
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
                key,
            } => self.graph.as_ref().is_some_and(|graph| {
                graph.edges.iter().any(|edge| {
                    edge.from == *from_node
                        && edge.from_port == *from_port
                        && edge.to == *to_node
                        && edge.to_port == *to_port
                        && edge.key == *key
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
        let pos = self.camera.world_to_render(self.node_at(graph, index));
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
        if !self.contains_point(abs) { return None; }
        let graph = self.graph.as_ref()?;
        let camera = self.interaction_camera();
        let local = camera.screen_to_local(abs);
        let grip = RESIZE_GRIP / camera.scale.min(1.0);
        let index = self.node_index_at(abs)?;
        let rect = self.card_rect(graph, index);
        Rect {
            pos: rect.pos + rect.size - dvec2(grip, grip),
            size: dvec2(grip, grip),
        }.contains(local).then_some(index)
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
        node.inputs.len().max(node.outputs.len())
    }

    fn port_local_at_flip(
        &self,
        graph: &Graph,
        index: usize,
        port: usize,
        output: bool,
        flip: f64,
    ) -> DVec2 {
        // A boundary port is its slot's socket, not a row of a card.
        if let Some(link) = self.boundary_link(graph, index, port, output) {
            return self.boundary_socket(link);
        }
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
        // The socket has no disc to leave from: the wire meets it directly.
        if self.boundary_link(graph, index, port, output).is_some() {
            return p;
        }
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
        if !self.contains_point(abs) || self.point_over_chrome(abs) { return None; }
        let graph = self.graph.as_ref()?;
        let camera = self.interaction_camera();
        let local = camera.screen_to_local(abs);
        let r = PORT_HIT_R / camera.scale.min(1.0);
        let mut nearest: Option<(f64, PortHit)> = None;
        for id in self.z_order.iter().rev() {
            let index = *self.node_index.get(id)?;
            let node = &graph.nodes[index];
            // Cards are drawn above the rails. A card covering a socket
            // must win the hit even if the boundary node was raised.
            if self.is_boundary(graph, index) { continue; }
            for port in 0..node.inputs.len() {
                let pos = self.port_local(graph, index, port, false);
                let distance = (pos - local).length();
                if distance <= r && nearest.as_ref().is_none_or(|(best, _)| distance < *best) {
                    nearest = Some((distance, PortHit {
                        node: index,
                        port,
                        output: false,
                    }));
                }
            }
            for port in 0..node.outputs.len() {
                let pos = self.port_local(graph, index, port, true);
                let distance = (pos - local).length();
                if distance <= r && nearest.as_ref().is_none_or(|(best, _)| distance < *best) {
                    nearest = Some((distance, PortHit {
                        node: index,
                        port,
                        output: true,
                    }));
                }
            }
            let mut card = self.card_rect(graph, index);
            card.pos.y -= LABEL_H;
            card.size.y += LABEL_H;
            // Overlapping padded hit areas choose the closest socket, with
            // draw order breaking ties. The first actual card body still
            // occludes every lower card and boundary socket at this point.
            if card.contains(local) { return nearest.map(|(_, hit)| hit); }
        }
        for link in self.boundary_links.iter().rev() {
            let distance = (self.boundary_socket(link) - local).length();
            if distance > r || nearest.as_ref().is_some_and(|(best, _)| distance >= *best) { continue; }
            let index = *self.node_index.get(&link.node)?;
            let node = &graph.nodes[index];
            let output = !link.output;
            let ports = if output { &node.outputs } else { &node.inputs };
            let port = ports.iter().position(|port| port.name == link.port)?;
            nearest = Some((distance, PortHit { node: index, port, output }));
        }
        nearest.map(|(_, hit)| hit)
    }

    fn node_index_at(&self, abs: DVec2) -> Option<usize> {
        let graph = self.graph.as_ref()?;
        let local = self.interaction_camera().screen_to_local(abs);
        self.z_order.iter().rev().find_map(|id| {
            let index = *self.node_index.get(id)?;
            // No phantom hit area where a boundary node was authored.
            if self.is_boundary(graph, index) {
                return None;
            }
            let mut rect = self.card_rect(graph, index);
            rect.pos.y -= LABEL_H;
            rect.size.y += LABEL_H;
            rect.contains(local).then_some(index)
        })
    }

    fn wire_index_at(&self, abs: DVec2) -> Option<usize> {
        let picked_up = match &self.drag { Some(Drag::Wire { picked_up, .. }) => *picked_up, _ => None };
        let camera = self.interaction_camera();
        let local = camera.screen_to_local(abs);
        let point = Self::route_point(local);
        let threshold = WIRE_HIT_PX / camera.scale.max(0.01);
        self.wire_cache
            .iter()
            .enumerate()
            .filter_map(|(index, cached)| {
                if picked_up.is_some_and(|edge| self.edges.get(index) == Some(&edge)) { return None; }
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
            key: edge.key,
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

    fn route_cache_key(edge: EdgeIndex, request: &RouteRequest) -> u64 {
        let RouteRequest { from, source_side, to, target_side, obstacles, offset } = request;
        let mut hash = DefaultHasher::new();
        edge.from.hash(&mut hash);
        edge.from_port.hash(&mut hash);
        edge.to.hash(&mut hash);
        edge.to_port.hash(&mut hash);
        source_side.hash(&mut hash);
        target_side.hash(&mut hash);
        for value in [from.x, from.y, to.x, to.y, *offset] {
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
            // A body changes what a deflation may do: it is part of the key.
            obstacle.body().is_some().hash(&mut hash);
            if let Some((min, max)) = obstacle.body() {
                for value in [min.x, min.y, max.x, max.y] {
                    value.to_bits().hash(&mut hash);
                }
            }
        }
        hash.finish()
    }

    /// The one routing policy: the cards near this cable, in local
    /// coordinates, and its corridor offset. `all_obstacles` is
    /// `route_obstacles`. The cards are those near the endpoints and near
    /// the shape being retained: a retained detour can leave the endpoints'
    /// box, and the cards it runs past belong in its request and its key.
    #[allow(clippy::too_many_arguments)]
    fn route_request(
        &self,
        from: Point,
        source_side: PortSide,
        to: Point,
        target_side: PortSide,
        all_obstacles: &[Obstacle],
        offset: f64,
        previous: Option<&WireRoute>,
    ) -> RouteRequest {
        let style = RouteStyle::default();
        let obstacles = if self.wire_mode == WireMode::Routed {
            let mut min = Point::new(from.x.min(to.x), from.y.min(to.y));
            let mut max = Point::new(from.x.max(to.x), from.y.max(to.y));
            if let Some((low, high)) = previous.map(WireRoute::bounds) {
                min = Point::new(min.x.min(low.x), min.y.min(low.y));
                max = Point::new(max.x.max(high.x), max.y.max(high.y));
            }
            wire_route::obstacles_in_corridor(
                min,
                max,
                all_obstacles,
                style.port_stub + style.corner_radius * 2.0 + offset.abs(),
            )
        } else {
            Vec::new()
        };
        RouteRequest { from, source_side, to, target_side, obstacles, offset }
    }

    /// The corridor offset one cable of a bundle between two cards has: the
    /// bundle is centred and its lanes are in port order, then an ordered
    /// input's entry key, then the order the edges were listed. Not
    /// insertion order alone, so the wire being dragged can know its lane.
    fn lane_offset(edges: &[EdgeIndex], index: usize, edge: &EdgeIndex) -> f64 {
        let bundle = edges
            .iter()
            .enumerate()
            .filter(|(_, other)| other.from == edge.from && other.to == edge.to);
        let total = bundle.clone().count();
        let position = bundle
            .filter(|(other_index, other)| {
                (other.from_port, other.to_port, other.key, *other_index) < (edge.from_port, edge.to_port, edge.key, index)
            })
            .count();
        (position as f64 - (total as f64 - 1.0) * 0.5) * RouteStyle::default().cable_spacing
    }

    /// The lane the wire being dragged will have once it is an edge, by the
    /// same order as `lane_offset`; a lone cable is at zero, as it will be.
    /// A single input takes one cable, so connecting replaces the one it
    /// has and that one leaves the bundle; an ordered input appends, and a
    /// new entry's key sorts after the entries it has.
    fn preview_offset(&self, graph: &Graph, from: usize, from_port: usize, to: usize, to_port: usize) -> f64 {
        let many = graph.nodes[to].inputs.get(to_port).is_some_and(|port| port.many);
        let bundle = self
            .edges
            .iter()
            .filter(|edge| edge.from == from && edge.to == to && (many || edge.to_port != to_port));
        let total = bundle.clone().count() + 1;
        let position = bundle
            .filter(|edge| (edge.from_port, edge.to_port) <= (from_port, to_port))
            .count();
        (position as f64 - (total as f64 - 1.0) * 0.5) * RouteStyle::default().cable_spacing
    }

    /// Route one request. A route that strays into a card outside the
    /// request's local set is routed again against every card, so no shape
    /// is ever kept over a card the corridor did not ask about.
    fn route_wire(&self, request: &RouteRequest, previous: Option<&WireRoute>, all_obstacles: &[Obstacle]) -> WireRoute {
        let route = self.route_with(request, &request.obstacles, previous);
        if self.wire_mode != WireMode::Routed {
            return route;
        }
        let strays: Vec<Obstacle> = all_obstacles
            .iter()
            .filter(|obstacle| !request.obstacles.contains(obstacle))
            .copied()
            .collect();
        if route.crosses_obstacles(&strays) {
            return self.route_with(request, all_obstacles, previous);
        }
        route
    }

    /// Keep routing's grid and retained spines in graph-world coordinates.
    /// The camera's render origin is only an f32 drawing aid, not layout.
    fn route_with(&self, request: &RouteRequest, obstacles: &[Obstacle], previous: Option<&WireRoute>) -> WireRoute {
        let origin = Self::route_point(self.camera.origin());
        let world = |point: Point| Point::new(point.x + origin.x, point.y + origin.y);
        let obstacles: Vec<_> = obstacles.iter().map(|obstacle| obstacle.translated(origin)).collect();
        let mut previous = previous.cloned();
        if let Some(previous) = previous.as_mut() { previous.translate(origin); }
        let mut route = wire_route::route_wire_sticky_in_mode(
            self.wire_mode, world(request.from), request.source_side, world(request.to), request.target_side,
            &obstacles, RouteStyle::default(), request.offset, previous.as_ref(),
        );
        route.translate(Point::new(-origin.x, -origin.y));
        route
    }

    fn ensure_wire_routes(&mut self, graph: &Graph) {
        // Slot and camera changes dirty this cache just like card movement.
        if !self.wire_cache_dirty && self.wire_cache.iter().all(Option::is_some) {
            return;
        }
        let all_obstacles = self.route_obstacles(graph);
        for index in 0..self.edges.len() {
            let edge = self.edges[index];
            if self.edge_unplaced(graph, edge) {
                self.wire_cache[index] = None;
                continue;
            }
            let from = Self::route_point(self.wire_anchor(graph, edge.from, edge.from_port, true));
            let to = Self::route_point(self.wire_anchor(graph, edge.to, edge.to_port, false));
            let source_side = self.side_of(graph, edge.from, edge.from_port, true);
            let target_side = self.side_of(graph, edge.to, edge.to_port, false);
            let offset = self.parallel_offsets.get(index).copied().unwrap_or(0.0);
            let previous = self.wire_cache[index]
                .as_ref()
                .map(|cached| &cached.route);
            let request = self.route_request(from, source_side, to, target_side, &all_obstacles, offset, previous);
            let key = Self::route_cache_key(edge, &request);
            if self.wire_cache.get(index).and_then(Option::as_ref).is_some_and(|cached| cached.key == key)
            {
                continue;
            }
            let route = self.route_wire(&request, previous, &all_obstacles);
            if route_trace() && previous != Some(&route) {
                let (source, target) = (&graph.nodes[edge.from], &graph.nodes[edge.to]);
                eprintln!(
                    "flow route {}.{} -> {}.{} key {:?} from ({:.1}, {:.1}) to ({:.1}, {:.1}) lane {offset:.1} cards {}: {}",
                    source.id,
                    source.outputs[edge.from_port].name,
                    target.id,
                    target.inputs[edge.to_port].name,
                    edge.key,
                    from.x,
                    from.y,
                    to.x,
                    to.y,
                    request.obstacles.len(),
                    route.describe(),
                );
            }
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
                let request = self.route_request(
                    from,
                    self.side_at_flip(graph, edge.from, edge.from_port, true, from_flip),
                    to,
                    self.side_at_flip(graph, edge.to, edge.to_port, false, to_flip),
                    obstacles,
                    self.parallel_offsets.get(edge_index).copied().unwrap_or(0.0),
                    None,
                );
                self.route_wire(&request, None, obstacles)
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
            for port in 0..count {
                let side = self.side_at_flip(graph, node_index, port, output, flip);
                let direction = if side == PortSide::Right { 1.0 } else { -1.0 };
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
            let request = self.route_request(
                from,
                self.side_at_flip(graph, edge.from, edge.from_port, true, from_flip),
                to,
                self.side_at_flip(graph, edge.to, edge.to_port, false, to_flip),
                obstacles,
                self.parallel_offsets.get(edge_index).copied().unwrap_or(0.0),
                None,
            );
            candidate_routes[edge_index] = self.route_wire(&request, None, obstacles);
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
            || self.edges.iter().any(|edge| self.edge_unplaced(graph, *edge))
        {
            return;
        }
        let obstacles = self.route_obstacles(graph);
        let original_facings: Vec<bool> = graph.nodes.iter().map(|node| node.flip).collect();
        let mut facings = original_facings.clone();
        for _ in 0..AUTO_FLIP_MAX_PASSES {
            let current_routes = self.routes_for_facings(graph, &facings, &obstacles);
            let mut pass_changes = Vec::new();
            for node_index in 0..graph.nodes.len() {
                // A slot faces into the graph from its rail; it has no
                // facing to choose.
                if self.is_boundary(graph, node_index) {
                    continue;
                }
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
        &mut self,
        graph: &Graph,
        from_index: usize,
        from_port: usize,
        pointer: DVec2,
        target: Option<(usize, usize)>,
    ) -> WireRoute {
        let from = Self::route_point(self.wire_anchor(graph, from_index, from_port, true));
        let source_side = self.side_of(graph, from_index, from_port, true);
        let (to, target_side) = target.map_or(
            (Self::route_point(pointer), PortSide::Left),
            |(node, port)| {
                (
                    Self::route_point(self.wire_anchor(graph, node, port, false)),
                    self.side_of(graph, node, port, false),
                )
            },
        );
        // The same request a graph edge makes: with a target, the lane and
        // the corridor the connected cable will have; a free pointer routes
        // as a lone cable to where it is. The previous preview's shape is
        // retained across the pointer's motion and into a target.
        let all_obstacles = self.route_obstacles(graph);
        let offset = target.map_or(0.0, |(node, port)| self.preview_offset(graph, from_index, from_port, node, port));
        let previous = self.preview_wire.as_ref().map(|cached| &cached.route);
        let request = self.route_request(from, source_side, to, target_side, &all_obstacles, offset, previous);
        let (to_node, to_port) = target.unwrap_or((usize::MAX, usize::MAX));
        let edge = EdgeIndex { from: from_index, from_port, to: to_node, to_port, key: None };
        let key = Self::route_cache_key(edge, &request);
        if let Some(cached) = self.preview_wire.as_ref().filter(|cached| cached.key == key) {
            return cached.route.clone();
        }
        let route = self.route_wire(&request, previous, &all_obstacles);
        if route_trace() && previous != Some(&route) {
            let source = &graph.nodes[from_index];
            let target = target.map_or_else(
                || "pointer".to_string(),
                |(node, port)| format!("{}.{}", graph.nodes[node].id, graph.nodes[node].inputs[port].name),
            );
            eprintln!(
                "flow preview {}.{} -> {target} from ({:.1}, {:.1}) to ({:.1}, {:.1}) lane {offset:.1} cards {}: {}",
                source.id,
                source.outputs[from_port].name,
                from.x,
                from.y,
                to.x,
                to.y,
                request.obstacles.len(),
                route.describe(),
            );
        }
        self.preview_wire = Some(CachedWire { key, route: route.clone() });
        self.preview_edge = Some(edge);
        route
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
        // Reduce the checker phase in f64 before sending it to the shader.
        let period = 2.0 * self.draw_bg.cell as f64;
        let origin = -self.camera.origin();
        self.draw_bg.origin = vec2(origin.x.rem_euclid(period) as f32, origin.y.rem_euclid(period) as f32);
        self.draw_bg.draw_abs(cx, local_view);
    }

    /// The boundary node whose slot is under `point`. A slot is the node:
    /// pressing it selects the node. Its wires are the graph's real wires and
    /// are hit as wires.
    fn boundary_at(&self, point: DVec2) -> Option<&str> {
        self.boundary_links.iter().find(|link| link.slot.contains(point)).map(|link| link.node.as_str())
    }

    /// Inlet/outlet slots sit below the wires and the node cards. A slot is
    /// the whole picture of its boundary node: a plate on the rail with the
    /// port's name and type, an edge in the accent it shares with the outer
    /// connector, and one socket on the side that faces into the graph,
    /// where the graph's own wires end. All geometry is reduced to this
    /// canvas's rebased local space before f32.
    fn draw_boundaries(&mut self, cx: &mut Cx2d, _graph: &Graph) {
        if self.boundary_links.is_empty() { return; }
        let links = self.boundary_links.clone();
        let accents = boundary_accents(&links);
        self.draw_vec.begin();
        for (link, accent) in links.iter().zip(&accents) {
            let selected = self.selected.as_ref().and_then(Selection::node) == Some(link.node.as_str());
            let pos = self.camera.screen_to_local(link.slot.pos);
            let size = link.slot.size / self.camera.scale;
            let radius = (size.y * 0.2) as f32;
            Self::set_color(&mut self.draw_vec, self.card_color, 1.0);
            self.draw_vec.rounded_rect(pos.x as f32, pos.y as f32, size.x as f32, size.y as f32, radius);
            self.draw_vec.fill();
            Self::set_color(&mut self.draw_vec, if selected { self.accent_color } else { *accent }, if selected { 1.0 } else { 0.9 });
            self.draw_vec.rounded_rect(pos.x as f32, pos.y as f32, size.x as f32, size.y as f32, radius);
            self.draw_vec.stroke((size.y * if selected { 0.07 } else { 0.035 }).max(1.0) as f32);
            // The socket: a filled accent bead on the inward edge, ringed in
            // the port's type colour so the type still reads at the slot.
            let socket = self.boundary_socket(link);
            let bead = (size.y * 0.16).clamp(2.0, PORT_RX);
            let kind_color = self.port_color(&link.kind);
            Self::set_color(&mut self.draw_vec, kind_color, 1.0);
            self.draw_vec.circle(socket.x as f32, socket.y as f32, (bead + 1.5) as f32);
            self.draw_vec.fill();
            Self::set_color(&mut self.draw_vec, *accent, 1.0);
            self.draw_vec.circle(socket.x as f32, socket.y as f32, bead as f32);
            self.draw_vec.fill();
        }
        self.draw_vec.end(cx);
        let font_size = self.draw_port.text_style.font_size;
        for link in &links {
            if link.slot.size.y < 15.0 { continue; }
            let pos = self.camera.screen_to_local(link.slot.pos);
            let size = link.slot.size / self.camera.scale;
            cx.push_clip_rect(Rect { pos, size });
            self.draw_port.color = self.color_port_label_connected;
            self.draw_port.text_style.font_size = (size.y * 0.25) as f32;
            self.draw_port.draw_abs(cx, pos + dvec2(size.y * 0.18, size.y * 0.10), &link.label);
            self.draw_port.color = self.port_color(&link.kind);
            self.draw_port.text_style.font_size = (size.y * 0.20) as f32;
            self.draw_port.draw_abs(cx, pos + dvec2(size.y * 0.18, size.y * 0.57), &link.kind);
            cx.pop_clip_rect();
        }
        self.draw_port.text_style.font_size = font_size;
    }

    fn child_boundary_links(&self, node: &str) -> Vec<(BoundaryLink, Vec4f)> {
        self.embedded_roots.iter()
            .filter(|root| root.node == node)
            .filter_map(|root| root.widget.borrow::<FlowCanvas>())
            .filter(|child| child.drawn_viewport.is_some_and(|viewport| nonempty(viewport.clip)))
            .flat_map(|child| {
                // The child's own accents, so both sides of the rail agree.
                let accents = boundary_accents(&child.boundary_links);
                child.boundary_links.iter().cloned().zip(accents).collect::<Vec<_>>()
            })
            .collect()
    }

    /// Continue each child rail to the outer connector in the parent's
    /// retained card list. The child itself remains clipped to the content.
    fn draw_boundary_continuations(&mut self, cx: &mut Cx2d, graph: &Graph, node: usize) {
        let links = self.child_boundary_links(&graph.nodes[node].id);
        if links.is_empty() { return; }
        let card = self.card_rect(graph, node);
        cx.push_clip_rect(Rect { pos: card.pos - dvec2(20.0, 0.0), size: card.size + dvec2(40.0, 0.0) });
        self.draw_over.begin();
        for (link, accent) in links {
            let outer = self.camera.screen_to_local(link.outer);
            let socket = self.camera.screen_to_local(dvec2(
                if link.on_right { link.slot.pos.x + link.slot.size.x } else { link.slot.pos.x },
                link.slot.pos.y + link.slot.size.y * 0.5,
            ));
            if !finite_point(outer) || !finite_point(socket) {
                continue;
            }
            // A fixed part of the container, not an edge of a graph: one
            // shape, decided by which rail the slot is on and which way the
            // value flows, never by comparing two nearly equal positions
            // (which tipped the router between equal-cost shapes while
            // zooming). On the right rail the outer connector lies beyond
            // the slot's outward edge, on the left rail before it.
            let toward_outer = if link.on_right { PortSide::Right } else { PortSide::Left };
            let toward_slot = if link.on_right { PortSide::Left } else { PortSide::Right };
            let (from, source_side, to, target_side) = if link.output {
                (socket, toward_outer, outer, toward_slot)
            } else {
                (outer, toward_slot, socket, toward_outer)
            };
            let route = wire_route::fixed_route_in_mode(self.wire_mode,
                Self::route_point(from), source_side, Self::route_point(to), target_side,
                RouteStyle::default());
            // The cable keeps its type colour, with a thin matching accent.
            // The port itself is accented in draw_overlays at its real centre.
            let color = self.port_color(&link.kind);
            Self::set_color(&mut self.draw_over, color, 0.85);
            Self::draw_route(&mut self.draw_over, &route);
            self.draw_over.stroke(2.5);
            Self::set_color(&mut self.draw_over, accent, 0.95);
            Self::draw_route(&mut self.draw_over, &route);
            self.draw_over.stroke(1.0);
        }
        self.draw_over.end(cx);
        cx.pop_clip_rect();
    }

    /// Wires are one vector batch in the canvas list, below every card list.
    fn draw_wires(&mut self, cx: &mut Cx2d, scope: &mut Scope, graph: &Graph) {
        let dragging_wire = matches!(self.drag, Some(Drag::Wire { .. }));
        let picked_up = match &self.drag { Some(Drag::Wire { picked_up, .. }) => *picked_up, _ => None };
        let time = self.time;
        self.ensure_wire_routes(graph);
        let selected_wire = self.selected_edge_index(graph);
        // Host materials use precisely the path the canvas hit-tests. Drawing
        // data cannot feed back into routing or card measurements. This pass
        // sits under the thin centreline and selection affordances.
        let mut observed = vec![false; self.edges.len()];
        if let (Some(host), Some(viewport)) = (scope.data.get_mut::<NodeFacesScope>(), self.drawn_viewport) {
            for (index, edge) in self.edges.iter().enumerate() {
                if picked_up == Some(*edge) { continue; }
                let Some(cached) = self.wire_cache[index].as_ref() else { continue };
                let source = &graph.nodes[edge.from];
                let target = &graph.nodes[edge.to];
                let view = crate::model::EdgeView {
                    from: source.id.clone(), from_port: source.outputs[edge.from_port].name.clone(),
                    to: target.id.clone(), to_port: target.inputs[edge.to_port].name.clone(), key: edge.key,
                };
                observed[index] = host.faces().draw_wire(cx, &view, &cached.route, &viewport);
            }
        }
        self.draw_vec.begin();
        // Wires, under the cards.
        for (index, edge) in self.edges.iter().copied().enumerate() {
            if picked_up == Some(edge) { continue; }
            let kind = &graph.nodes[edge.from].outputs[edge.from_port].kind;
            let color = self.port_color(kind);
            let node = &graph.nodes[edge.from].id;
            let streaming = self.streaming.contains(node) && !observed[index];
            let carrying = self.carrying.contains(node);
            let selected = selected_wire == Some(index);
            let hovered = self.hover_wire == Some(index);
            // The source node's progress continues into its cables, in the
            // same colour as the bar on the card.
            let flow = self.statuses.get(node).and_then(|status| {
                let state = status.state.as_str();
                let color = if state == "running" {
                    self.kind_color(&graph.nodes[edge.from].kind)
                } else {
                    self.state_color(state)
                };
                match state {
                    "running" if !status.has_progress => Some((WireFlow::Sweep, color)),
                    "running" | "done" => Some((WireFlow::Fill(status.shown.clamp(0.0, 1.0)), color)),
                    "failed" | "cancelled" => Some((WireFlow::Stopped, color)),
                    _ => None,
                }
            });
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
            self.draw_vec.stroke(if observed[index] {
                if selected || hovered { 1.5 } else { 0.75 }
            } else if selected {
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
            if let Some((flow, flow_color)) = flow.filter(|_| !dragging_wire && !observed[index]) {
                let length = route.length();
                match flow {
                    WireFlow::Fill(fraction) => {
                        let filled = length * fraction;
                        if filled > 0.5 {
                            Self::set_color(&mut self.draw_vec, flow_color, 0.18);
                            Self::draw_clamped_route_slice(&mut self.draw_vec, route, 0.0, filled);
                            self.draw_vec.stroke_opts(WIRE_FLOW_WIDTH + 6.0, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                            Self::set_color(&mut self.draw_vec, flow_color, 0.95);
                            Self::draw_clamped_route_slice(&mut self.draw_vec, route, 0.0, filled);
                            self.draw_vec.stroke_opts(WIRE_FLOW_WIDTH, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                        }
                        // While the value is still being made, a glint runs
                        // down the filled part towards the consumer.
                        if fraction < 1.0 && filled > WIRE_FLOW_GLINT {
                            let head = (time * WIRE_FLOW_SPEED) % (filled + WIRE_FLOW_GLINT);
                            Self::set_color(&mut self.draw_vec, vec4(1.0, 1.0, 1.0, 1.0), 0.45);
                            Self::draw_clamped_route_slice(
                                &mut self.draw_vec,
                                route,
                                head - WIRE_FLOW_GLINT,
                                head.min(filled),
                            );
                            self.draw_vec.stroke_opts(WIRE_FLOW_WIDTH - 2.0, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                        }
                    }
                    WireFlow::Sweep => {
                        // Indeterminate: the card's sweeping segment, carried
                        // on from the source port to the target port.
                        // It leaves the port as the card's segment reaches
                        // the end of the bar (0.28 of the bar long).
                        let segment = (length * 0.28).max(WIRE_FLOW_GLINT);
                        let head = (time * 0.8 - 1.0 / 1.28).rem_euclid(1.0) * (length + segment);
                        Self::set_color(&mut self.draw_vec, flow_color, 0.18);
                        Self::draw_clamped_route_slice(&mut self.draw_vec, route, head - segment, head);
                        self.draw_vec.stroke_opts(WIRE_FLOW_WIDTH + 6.0, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                        Self::set_color(&mut self.draw_vec, flow_color, 0.95);
                        Self::draw_clamped_route_slice(&mut self.draw_vec, route, head - segment, head);
                        self.draw_vec.stroke_opts(WIRE_FLOW_WIDTH, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                    }
                    WireFlow::Stopped => {
                        // Nothing left this node: a short stub at the source
                        // port in the state's colour, the rest stays a cable.
                        Self::set_color(&mut self.draw_vec, flow_color, 0.95);
                        Self::draw_clamped_route_slice(
                            &mut self.draw_vec,
                            route,
                            0.0,
                            (length * 0.25).min(WIRE_FLOW_GLINT),
                        );
                        self.draw_vec.stroke_opts(WIRE_FLOW_WIDTH, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                    }
                }
            }
            for pulse in self.pulses.iter().filter(|pulse| pulse.node == *node && !observed[index]) {
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
            if self.camera.scale >= 0.5 && !observed[index] {
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
        // A wire that ends at a slot wears the slot's accent on its last
        // stretch: the cable's own type colour and status stay as they are.
        if !self.boundary_links.is_empty() {
            const ACCENT_LENGTH: f64 = 22.0;
            let accents = boundary_accents(&self.boundary_links);
            for (index, edge) in self.edges.iter().copied().enumerate() {
                let Some(route) = self.wire_cache[index].as_ref().map(|cached| &cached.route) else {
                    continue;
                };
                let length = route.length();
                // Which link each end is, by position: no reference into
                // `self` is held while the vector batch is written.
                let link_at = |node: usize, port: usize, output: bool| {
                    self.boundary_link(graph, node, port, output)
                        .and_then(|link| self.boundary_links.iter().position(|other| std::ptr::eq(other, link)))
                };
                let ends = [
                    (link_at(edge.from, edge.from_port, true), 0.0, ACCENT_LENGTH.min(length * 0.5)),
                    (link_at(edge.to, edge.to_port, false), (length - ACCENT_LENGTH).max(length * 0.5), length),
                ];
                for (at, start, end) in ends {
                    let Some(at) = at else { continue };
                    Self::set_color(&mut self.draw_vec, accents[at], if dragging_wire { 0.35 } else { 1.0 });
                    Self::draw_clamped_route_slice(&mut self.draw_vec, route, start, end);
                    self.draw_vec.stroke(3.0);
                }
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
        }) = self.drag.clone()
        {
            let b = self.camera.screen_to_local(pos);
            let color = self.port_color(&ty);
            let route = self.preview_route(graph, from, from_port, b, target);
            Self::set_color(&mut self.draw_vec, color, 1.0);
            Self::draw_route(&mut self.draw_vec, &route);
            self.draw_vec.stroke(3.0);
            // A slot's socket that can take the wire says so, as a card's
            // input does: it has no card overlay to say it for it.
            let sockets: Vec<DVec2> = self
                .compatible
                .iter()
                .filter_map(|(node, port)| self.boundary_link(graph, *node, *port, false))
                .map(|link| self.boundary_socket(link))
                .collect();
            for socket in sockets {
                Self::set_color(&mut self.draw_vec, self.accent_color, 0.9);
                self.draw_vec.circle(socket.x as f32, socket.y as f32, (PORT_RX + 3.0) as f32);
                self.draw_vec.stroke(1.5);
            }
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

    /// The icon inside one socket: the host's key for it when that style has
    /// an icon, else the port kind's.
    fn socket_icon(&mut self, node: &str, port: &str, output: bool, kind: &str) -> Option<&mut DrawSvg> {
        let key = if self.port_icon_overrides.is_empty() {
            None
        } else {
            self.port_icon_overrides
                .get(&(node.to_string(), port.to_string(), output))
                .filter(|key| self.styles.has_port_icon(key))
                .cloned()
        };
        self.styles.port_icon(key.as_deref().unwrap_or(kind))
    }

    fn prune_port_icon_overrides(&mut self) {
        let Some(graph) = self.graph.as_ref() else {
            self.port_icon_overrides.clear();
            return;
        };
        let node_index = &self.node_index;
        self.port_icon_overrides.retain(|(node, port, output), _| {
            node_index
                .get(node)
                .and_then(|index| graph.nodes.get(*index))
                .is_some_and(|view| {
                    let ports = if *output { &view.outputs } else { &view.inputs };
                    ports.iter().any(|candidate| candidate.name == *port)
                })
        });
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

    /// The faces, each in a turtle at its card's content rect. Measurements
    /// are applied after the complete frame, so cards, ports and wires all
    /// consume the same geometry snapshot.
    fn draw_faces(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        graph: &Graph,
        indices: &[usize],
    ) -> Vec<(usize, f64)> {
        let mut measured = Vec::new();
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
                    ..Default::default()
                },
                Layout {
                    flow: Flow::Down,
                    clip_x: true,
                    clip_y: true,
                    ..Layout::default()
                },
            );
            let face_viewport = FaceViewport {
                canvas: self.drawn_viewport.expect("canvas drawing has a viewport"),
                geometry: self.geometry(graph, index),
                content_clip: intersect_rect(
                    self.drawn_viewport.unwrap().clip,
                    Rect {
                        pos: self.camera.local_to_screen(content.rect.pos),
                        size: content.rect.size * self.camera.scale,
                    },
                ),
                ports: node.inputs.iter().enumerate().map(|(port, input)| FacePort {
                    name: input.name.clone(), kind: Self::input_kind(node, port).to_string(), output: false,
                    anchor: self.camera.local_to_screen(self.wire_anchor(graph, index, port, false)),
                }).chain(node.outputs.iter().enumerate().map(|(port, output)| FacePort {
                    name: output.name.clone(), kind: output.kind.clone(), output: true,
                    anchor: self.camera.local_to_screen(self.wire_anchor(graph, index, port, true)),
                })).collect(),
            };
            if let Some(faces) = scope.data.get_mut::<NodeFacesScope>() {
                faces.faces().draw_face_in_viewport(
                    cx,
                    &node.id,
                    if fixed_height.is_some() {
                        Walk::fill()
                    } else {
                        Walk::fill_fit()
                    },
                    fixed_height.is_some(),
                    &face_viewport,
                );
            }
            let rect = cx.end_turtle();
            if let Some(faces) = scope.data.get_mut::<NodeFacesScope>() {
                faces.faces().draw_node_overlay(cx, &node.id, &face_viewport);
            }
            cx.pop_clip_rect();
            let mut height = rect.size.y + content.pad_top + content.pad_bottom;
            if has_error {
                height += 20.0;
            }
            let height = height.max(min_card_height(full_bleed, Self::port_rows(node)));
            let pending_face = scope.data.get_mut::<NodeFacesScope>()
                .is_some_and(|faces| faces.faces().face_measurement_pending(&node.id));
            if !pending_face && node.size.is_none() && (self.heights[index] - height).abs() > 0.5 {
                measured.push((index, height));
            }
        }
        measured
    }

    fn draw_locked_face_overlay(&mut self, cx: &mut Cx2d, graph: &Graph, indices: &[usize]) {
        if !self.faces_locked {
            return;
        }
        self.draw_over.begin();
        self.draw_over.set_color(0.0, 0.0, 0.0, 0.06);
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let card = self.card_rect(graph, index);
            let content =
                card_content_rect(card, Self::full_bleed(node), Self::port_rows(node));
            self.draw_over.rounded_rect(
                content.rect.pos.x as f32,
                content.rect.pos.y as f32,
                content.rect.size.x as f32,
                content.rect.size.y as f32,
                4.0,
            );
            self.draw_over.fill();
        }
        self.draw_over.end(cx);
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
            let boundary_links = self.child_boundary_links(&node.id);
            let r = self.card_rect(graph, index);
            // Progress bar: the top strip of a running card.
            if let Some(status) = self.statuses.get(&node.id) {
                let state = status.state.as_str();
                let show = matches!(state, "running" | "waiting" | "queued")
                    || (matches!(state, "done" | "failed") && status.shown < 1.0 - 1e-6)
                    || matches!(state, "done" | "failed");
                if show {
                    // Inset well past the card's 16 px corners so the bar
                    // sits inside the rounded box, and centred in the header
                    // strip between the label row and the first port row.
                    let inset = CARD_RADIUS + 8.0;
                    let bx = r.pos.x as f32 + inset;
                    let by = (r.pos.y + (CARD_HEADER_H - PROGRESS_H) * 0.5) as f32;
                    let bw = r.size.x as f32 - 2.0 * inset;
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
                if let Some((_, accent)) = boundary_links.iter().find(|(link, _)| {
                    !link.output && link.node == node.inputs[port].name
                }) {
                    Self::set_color(&mut self.draw_over, *accent, if ok { 1.0 } else { 0.25 });
                    Self::shaped_port(&mut self.draw_over, centre, grow - 3.5, direction, true);
                    self.draw_over.stroke(1.5);
                }
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
                if let Some((_, accent)) = boundary_links.iter().find(|(link, _)| {
                    link.output && link.node == output.name
                }) {
                    Self::set_color(&mut self.draw_over, *accent, 1.0);
                    Self::shaped_port(&mut self.draw_over, centre, -3.5, direction, false);
                    self.draw_over.stroke(1.5);
                }
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
                let kind = Self::input_kind(node, port);
                if let Some(icon) = self.socket_icon(&node.id, &node.inputs[port].name, false, kind) {
                    icon.draw_abs(cx, rect);
                }
            }
            for (port, output) in node.outputs.iter().enumerate() {
                let p = self.port_local(graph, index, port, true);
                let rect = Rect {
                    pos: p + dvec2(direction * PORT_ICON_SHIFT_OUT - 4.75, -4.75),
                    size: dvec2(9.5, 9.5),
                };
                if let Some(icon) = self.socket_icon(&node.id, &output.name, true, &output.kind) {
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
            picked_up,
        } = drag
        else {
            return;
        };
        let Some(graph) = self.graph.as_ref() else {
            return;
        };
        let from_node = graph.nodes[from].id.clone();
        let from_port_index = from_port;
        let from_port = graph.nodes[from].outputs[from_port].name.clone();
        if let Some((to, to_port)) = target {
            if picked_up.is_some_and(|old| old.to == to && old.to_port == to_port) {
                self.compatible.clear();
                return;
            }
            let drawn = EdgeIndex { from, from_port: from_port_index, to, to_port, key: None };
            let to_node = graph.nodes[to].id.clone();
            let to_port = graph.nodes[to].inputs[to_port].name.clone();
            // The preview was routed by the connected cable's own policy:
            // it seeds that cable's route once the edit lands, if it was
            // last drawn to this target rather than to an earlier one.
            if let Some(preview) = self.preview_wire.take().filter(|_| self.preview_edge == Some(drawn)) {
                self.pending_connect = Some(PendingConnect {
                    from: from_node.clone(),
                    from_port: from_port.clone(),
                    to: to_node.clone(),
                    to_port: to_port.clone(),
                    route: preview.route,
                });
            }
            let edit = if let Some(old) = picked_up {
                CanvasEdit::Reconnect {
                    from_node,
                    from_port,
                    old_to_node: graph.nodes[old.to].id.clone(),
                    old_to_port: graph.nodes[old.to].inputs[old.to_port].name.clone(),
                    old_key: old.key,
                    to_node,
                    to_port,
                }
            } else {
                CanvasEdit::Connect {
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                }
            };
            cx.widget_action(self.uid, FlowCanvasAction::Edit(edit));
        } else if self.contains_point(pos) && !self.point_over_chrome(pos)
            && self.node_index_at(pos).is_none()
        {
            if let Some(old) = picked_up {
                cx.widget_action(self.uid, FlowCanvasAction::Edit(CanvasEdit::Disconnect {
                    to_node: graph.nodes[old.to].id.clone(),
                    to_port: graph.nodes[old.to].inputs[old.to_port].name.clone(),
                    key: old.key,
                }));
                self.compatible.clear();
                return;
            }
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
            || (!self.embedded && self.drag.is_none()
                && ((self.camera.pan - self.target_pan).length() > 0.05
                    || (self.camera.scale - self.target_scale).abs() > 1e-4))
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

impl FlowCanvas {
    /// Draw in an absolute window-space viewport while consuming one parent
    /// walk. No fitting, easing or camera actions occur in embedded mode.
    /// Keep the supplied viewport stable throughout a captured gesture.
    pub fn draw_embedded(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
        viewport: CanvasViewport,
    ) -> Result<DrawStep, CanvasViewportError> {
        let mut viewport = viewport.checked()?;
        if viewport.camera.render_origin.is_none() && self.embedded {
            viewport.camera.render_origin = self.camera.render_origin;
        }
        // A projected group body can start far off screen at deep zoom.
        // Rebase at the visible intersection, not at that distant body corner.
        viewport.camera.rebase_at(viewport.clip.pos);
        if !viewport
            .camera
            .matrix()
            .v
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(CanvasViewportError::NonFinite);
        }
        cx.walk_turtle(walk);
        // Area::Rect is window-space and does not inherit the draw matrix.
        // Isolate its align range from the parent's graph-coordinate turtle.
        cx.begin_unclipped_root_turtle(viewport.camera.view.size, Layout::flow_overlay());
        cx.push_clip_rect(viewport.clip);
        cx.add_rect_area(&mut self.area, viewport.clip);
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle_no_clip();
        self.embedded = true;
        self.target_pan = viewport.camera.pan;
        self.target_scale = viewport.camera.scale;
        self.fit_pending = 0;
        Ok(self.draw_viewport(cx, scope, viewport))
    }

    /// Restore absolute transforms in parent-before-child order after the
    /// parent has recursively assigned its matrix. Registered direct canvas
    /// roots are finalized recursively, including retained child draw lists.
    /// Call after any external ancestor transform assignment as well.
    pub fn finalize_draw_transform(&self, cx: &mut Cx) {
        if let (Some(list), Some(viewport)) = (&self.draw_list, self.drawn_viewport) {
            list.set_view_transform(cx, &viewport.camera.matrix());
        }
        for root in &self.embedded_roots {
            if let Some(canvas) = root.widget.borrow::<FlowCanvas>() {
                canvas.finalize_draw_transform(cx);
            }
        }
    }

    fn draw_viewport(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        viewport: CanvasViewport,
    ) -> DrawStep {
        let origin_shift = self.camera.origin() - viewport.camera.origin();
        if origin_shift != dvec2(0.0, 0.0) {
            let offset = Self::route_point(origin_shift);
            for cached in self.wire_cache.iter_mut().flatten() {
                cached.route.translate(offset);
            }
            if let Some(preview) = self.preview_wire.as_mut() { preview.route.translate(offset); }
            if let Some(pending) = self.pending_connect.as_mut() { pending.route.translate(offset); }
        }
        if self.camera.render_origin != viewport.camera.render_origin
            || (!self.boundary_nodes.is_empty()
                && (self.camera.pan != viewport.camera.pan
                    || self.camera.scale != viewport.camera.scale
                    || self.camera.view.pos != viewport.camera.view.pos
                    || self.camera.view.size != viewport.camera.view.size))
        {
            self.wire_cache_dirty = true;
        }
        self.camera = viewport.camera;
        self.drawn_viewport = Some(viewport);
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        let mut draw_list = self.draw_list.take().unwrap();
        draw_list.begin_always(cx);
        cx.begin_unclipped_root_turtle(dvec2(ROOT_SIZE, ROOT_SIZE), Layout::flow_overlay());
        let local_clip = Rect {
            pos: self.camera.screen_to_local(viewport.clip.pos),
            size: viewport.clip.size / self.camera.scale,
        };
        cx.push_clip_rect(local_clip);
        if nonempty(viewport.clip) {
            self.draw_background(cx, self.camera.local_view());
        }
        let mut measured_heights = Vec::new();
        if nonempty(viewport.clip) {
            if let Some(mut graph) = self.graph.take() {
                self.draw_boundaries(cx, &graph);
                self.draw_wires(cx, scope, &graph);
                let z_order = std::mem::take(&mut self.z_order);
                if let Some(faces) = scope.data.get_mut::<NodeFacesScope>() {
                    // A boundary node has no card and so no face to order.
                    if self.boundary_nodes.is_empty() {
                        faces.faces().set_z_order(&z_order);
                    } else {
                        let cards: Vec<String> = z_order.iter().filter(|id| !self.boundary_nodes.contains(*id)).cloned().collect();
                        faces.faces().set_z_order(&cards);
                    }
                }
                for id in &z_order {
                    let Some(index) = self.node_index.get(id).copied() else {
                        continue;
                    };
                    // Its slot, drawn with the boundaries above, is all of it.
                    if self.is_boundary(&graph, index) {
                        self.card_draw_lists[index] = None;
                        continue;
                    }
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
                    measured_heights.extend(self.draw_faces(cx, scope, &graph, one));
                    self.draw_boundary_continuations(cx, &graph, index);
                    self.draw_locked_face_overlay(cx, &graph, one);
                    self.draw_overlays(cx, &graph, one);
                    self.draw_labels(cx, &graph, one);
                    card_list.end(cx);
                    self.card_draw_lists[index] = Some(card_list);
                }
                self.z_order = z_order;
                self.draw_top_overlay(cx);
                if measured_heights.is_empty() {
                    self.maybe_auto_flip(cx, &mut graph);
                }
                self.graph = Some(graph);
            }
        }
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle_no_clip();
        draw_list.end(cx);
        self.draw_list = Some(draw_list);
        self.finalize_draw_transform(cx);
        let heights_changed = !measured_heights.is_empty();
        if heights_changed {
            for (index, height) in measured_heights { self.heights[index] = height; }
            // Wires, ports and outlines were drawn against last frame's heights.
            self.wire_cache_dirty = true;
            self.area.redraw(cx);
        }
        if !self.embedded && self.fit_pending > 0 && !heights_changed {
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
        self.embedded = false;
        let mut camera = self.camera;
        if self.zoom_min < ZOOM_MIN || self.zoom_max > ZOOM_MAX || camera.render_origin.is_some() {
            camera.rebase_at(view.pos);
        }
        self.draw_viewport(cx, scope, CanvasViewport { camera, clip: view })
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let policy = scope.data.get_mut::<NodeFacesScope>()
            .and_then(|faces| faces.policy_for(self.uid))
            .unwrap_or(CanvasEventPolicy::ALL);
        self.handle_event_routed(cx, event, scope, policy);
    }
}

impl FlowCanvas {
    /// Route each new pointer gesture to exactly one level. Suppressing input
    /// does not suppress animation/status maintenance or release of our drag.
    /// Navigation-only presses pan without selecting the container beneath.
    pub fn handle_event_routed(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope,
        policy: CanvasEventPolicy,
    ) {
        let navigation = policy.navigation && !self.embedded;
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
            if moved && !self.embedded && self.drag.is_none() {
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
        if policy.pointer {
            if let Event::MouseUp(e) = event {
                if let Some(type_name) = self.armed_type.take() {
                    if self.contains_point(e.abs) && !self.point_over_chrome(e.abs) {
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
        }
        if !policy.pointer && (self.hover.take().is_some() | self.hover_wire.take().is_some()) {
            self.area.redraw(cx);
        }
        if policy.pointer {
            if let Event::MouseMove(e) = event {
                self.cursor = e.abs;
                let over_chrome = self.point_over_chrome(e.abs);
                if self.armed_type.is_some() && self.contains_point(e.abs) && !over_chrome {
                    self.area.redraw(cx);
                }
                if self.drag.is_none() {
                    let hit = if self.contains_point(e.abs) && !over_chrome {
                        prioritize_canvas_hit(self.node_index_at(e.abs), self.wire_index_at(e.abs))
                    } else {
                        CanvasHit::Empty
                    };
                    let (hover, hover_wire) = match hit {
                        CanvasHit::Card(index) => (Some(index), None),
                        CanvasHit::Wire(index) => (None, Some(index)),
                        CanvasHit::Empty => (None, None),
                    };
                    let boundary_hover = hover.is_none() && self.contains_point(e.abs)
                        && !over_chrome && self.boundary_at(e.abs).is_some();
                    cx.set_cursor(if hover_wire.is_some() || boundary_hover {
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
        }
        // Avoid calling hits at all for a suppressed press: even an ignored
        // FingerDown result can acquire capture before the match below.
        let accepts_hit = match event {
            Event::MouseDown(_)
            | Event::MouseMove(_)
            | Event::MouseUp(_)
            | Event::TouchUpdate(_) => policy.pointer || navigation || self.drag.is_some(),
            Event::Scroll(_) => navigation && self.drag.is_none(),
            Event::KeyDown(_) | Event::KeyUp(_) => policy.keyboard,
            _ => false,
        };
        if !accepts_hit {
            return;
        }
        if matches!(event, Event::MouseDown(press)
            if !self.contains_point(press.abs) || self.point_over_chrome(press.abs))
        {
            return;
        }
        // Faces receive events first. Co-capture display-only face presses so
        // a click can still open a picture, then promote the canvas capture
        // once movement crosses the card-drag threshold. Interactive face
        // controls retain exclusive capture.
        let capture_display_press = policy.pointer
            && match event {
                Event::MouseDown(event) => {
                    self.contains_point(event.abs)
                        && self.node_index_at(event.abs).is_some()
                        && !self.interactive_face_widget_at(cx, event.abs, event.handled.get())
                }
                Event::TouchUpdate(event) => event.touches.iter().any(|touch| {
                    touch.state == TouchState::Start
                        && self.contains_point(touch.abs)
                        && self.node_index_at(touch.abs).is_some()
                        && !self.interactive_face_widget_at(cx, touch.abs, touch.handled.get())
                }),
                _ => false,
            };
        match event.hits_with_capture_overload(cx, self.area, capture_display_press) {
            Hit::FingerScroll(fs)
                if navigation
                    && !scroll_is_handled
                    && self.contains_point(fs.abs)
                    && !self.point_over_chrome(fs.abs) =>
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
            Hit::FingerDown(fd)
                if self.contains_point(fd.abs) && !self.point_over_chrome(fd.abs) =>
            {
                // Stop an outstanding camera ease at the displayed frame.
                self.camera = self.interaction_camera();
                self.target_pan = self.camera.pan;
                self.target_scale = self.camera.scale;
                if policy.keyboard {
                    cx.set_key_focus(self.area);
                }
                if !policy.pointer {
                    self.drag = Some(Drag::Pan {
                        start: fd.abs,
                        origin: self.camera.pan,
                        navigate: navigation,
                        select: false,
                    });
                } else if let Some(hit) = self.port_at(fd.abs) {
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
                            picked_up: None,
                        });
                        self.compatible = compatible;
                    } else {
                        // An input with a wire: pick the wire up again from
                        // its source; a bare one does nothing.
                        let input = &node.inputs[hit.port];
                        if input.connected {
                            // On an ordered input the wire picked up is the
                            // last entry (the newest, drawn on top).
                            let source = self
                                .edges
                                .iter()
                                .filter(|edge| edge.to == hit.node && edge.to_port == hit.port)
                                .last()
                                .map(|edge| (edge.from, edge.from_port, edge.key));
                            if let Some((from, from_port, key)) = source {
                                let ty = graph.nodes[from].outputs[from_port].kind.clone();
                                let compatible = self.compatible_for(graph, from, from_port);
                                self.drag = Some(Drag::Wire {
                                    from,
                                    from_port,
                                    ty,
                                    pos: fd.abs,
                                    target: None,
                                    picked_up: Some(EdgeIndex { from, from_port, to: hit.node, to_port: hit.port, key }),
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
                    if fd.tap_count >= 2 {
                        cx.widget_action(self.uid, FlowCanvasAction::Open { node: id.clone() });
                    }
                    self.drag = Some(Drag::Node {
                        index,
                        start: fd.abs,
                        initial: origin,
                        origin,
                        moved: false,
                    });
                } else if let Some(node) = self.boundary_at(fd.abs).map(str::to_string) {
                    let selection = Some(Selection::Node(node));
                    if self.selected != selection {
                        self.selected = selection.clone();
                        cx.widget_action(self.uid, FlowCanvasAction::Select(selection));
                    }
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
                        navigate: navigation,
                        select: true,
                    });
                    if navigation {
                        cx.set_cursor(MouseCursor::Grabbing);
                    }
                }
                self.area.redraw(cx);
            }
            Hit::FingerMove(fm) => {
                let s = self.camera.scale;
                match self.drag.clone() {
                    Some(Drag::Pan {
                        start,
                        origin,
                        navigate,
                        ..
                    }) => {
                        if navigate {
                            self.camera.pan = origin + (fm.abs - start);
                            self.target_pan = self.camera.pan;
                        }
                    }
                    Some(Drag::Node {
                        index,
                        start,
                        initial,
                        origin,
                        moved,
                    }) => {
                        let delta = fm.abs - start;
                        let moved = moved || delta.length() > DRAG_THRESHOLD;
                        if moved {
                            cx.promote_finger_capture_over(self.area);
                        }
                        let origin = if moved {
                            (initial.0 + delta.x / s, initial.1 + delta.y / s)
                        } else {
                            origin
                        };
                        self.drag = Some(Drag::Node {
                            index,
                            start,
                            initial,
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
                        picked_up,
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
                            picked_up,
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
                    Some(Drag::Pan { start, select, .. }) => {
                        // A press taken away is no click: nothing deselects.
                        if select
                            && !fu.cancelled
                            && (fu.abs - start).length() <= DRAG_THRESHOLD
                            && self.selected.is_some()
                        {
                            self.selected = None;
                            cx.widget_action(self.uid, FlowCanvasAction::Select(None));
                        }
                    }
                    Some(Drag::Node {
                        index,
                        origin,
                        moved,
                        ..
                    }) => {
                        if moved {
                            if let Some(node) =
                                self.graph.as_mut().and_then(|graph| graph.nodes.get_mut(index))
                            {
                                node.at = origin;
                                self.wire_cache_dirty = true;
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
                        if let Some(node) =
                            self.graph.as_mut().and_then(|graph| graph.nodes.get_mut(index))
                        {
                            node.size = Some(size);
                            self.wire_cache_dirty = true;
                            cx.widget_action(
                                self.uid,
                                FlowCanvasAction::Edit(CanvasEdit::Resize {
                                    node: node.id.clone(),
                                    size,
                                }),
                            );
                        }
                    }
                    Some(Drag::Wire {
                        from,
                        from_port,
                        ty,
                        picked_up,
                        ..
                    }) if fu.cancelled => {
                        // Taken away (a list or the host took the finger): no
                        // connect, reconnect or disconnect — a picked-up wire
                        // stays where it was, nothing was edited yet.
                        let _ = (from, from_port, ty, picked_up);
                        self.compatible.clear();
                    }
                    Some(Drag::Wire {
                        from,
                        from_port,
                        ty,
                        picked_up,
                        ..
                    }) => {
                        // Release may arrive outside the clip without a final
                        // move; never commit the last in-bounds hover target.
                        let target = self.port_at(fu.abs).and_then(|hit| {
                            (!hit.output && self.compatible.contains(&(hit.node, hit.port)))
                                .then_some((hit.node, hit.port))
                        });
                        self.finish_wire_drag(
                            cx,
                            Drag::Wire {
                                from,
                                from_port,
                                ty,
                                pos: fu.abs,
                                target,
                                picked_up,
                            },
                        );
                    }
                    None => {}
                }
                self.preview_wire = None;
                self.area.redraw(cx);
            }
            Hit::KeyDown(ke) if policy.keyboard => match ke.key_code {
                KeyCode::Delete | KeyCode::Backspace => {
                    if let Some(selection) = self.selected.take() {
                        let edit = match selection {
                            Selection::Node(node) => CanvasEdit::Delete { node },
                            Selection::Edge {
                                to_node,
                                to_port,
                                key,
                                ..
                            } => CanvasEdit::Disconnect {
                                to_node,
                                to_port,
                                key,
                            },
                        };
                        cx.widget_action(self.uid, FlowCanvasAction::Edit(edit));
                        cx.widget_action(self.uid, FlowCanvasAction::Select(None));
                        self.area.redraw(cx);
                    }
                }
                KeyCode::Home if navigation && self.drag.is_none() => self.fit(cx),
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
