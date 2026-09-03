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
//!   inverse camera ([`Camera::remap_event`] in `faces.rs`); the canvas's own
//!   hit tests (ports, cards) convert the other way. No platform change.

use crate::faces::FaceHost;
use crate::graph_edit::{self, GraphIndex, NODE_WIDTH};
use makepad_flow::{Graph, Literal, Node, NodeInputValue, PortType};
use makepad_widgets::makepad_draw::DrawSvg;
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};

/// Local-space offset of the world origin: keeps every local coordinate
/// positive inside the root turtle's `(0, 0)..ROOT_SIZE` clip.
pub const LOCAL_ORIGIN: f64 = 32768.0;
const ROOT_SIZE: f64 = 65536.0;

const CARD_RADIUS: f32 = 16.0;
/// The icon-and-title row above every card.
const LABEL_H: f64 = 26.0;
/// Top inset occupied by the card's in-body header/chrome before its ports.
const CARD_HEADER_H: f64 = 14.0;
const PORT_ROW_H: f64 = 24.0;
const PORT_R: f64 = 11.0;
const PORT_HIT_R: f64 = 16.0;
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortIcon {
    Text,
    Image,
    Audio,
    Video,
    Mesh,
    Json,
    Bytes,
}

impl PortIcon {
    pub(crate) fn for_type(ty: PortType) -> Self {
        match ty {
            PortType::Text => Self::Text,
            PortType::Image => Self::Image,
            PortType::Audio => Self::Audio,
            PortType::Video => Self::Video,
            PortType::Mesh => Self::Mesh,
            PortType::Json | PortType::List => Self::Json,
            PortType::Bytes => Self::Bytes,
        }
    }

    #[cfg(test)]
    fn path(self) -> &'static str {
        match self {
            Self::Text => "resources/icons/text.svg",
            Self::Image => "resources/icons/image.svg",
            Self::Audio => "resources/icons/audio.svg",
            Self::Video => "resources/icons/video.svg",
            Self::Mesh => "resources/icons/mesh.svg",
            Self::Json => "resources/icons/json.svg",
            Self::Bytes => "resources/icons/bytes.svg",
        }
    }
}

pub(crate) fn declared_output_type(node: &Node) -> Option<PortType> {
    if node.type_name != "Output" {
        return None;
    }
    node.params
        .iter()
        .find_map(|(name, value)| {
            if name != "type" {
                return None;
            }
            match value {
                Literal::Id(name) | Literal::Str(name) => PortType::from_str(name),
                _ => None,
            }
        })
        .or_else(|| node.inputs.first().map(|input| input.ty))
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

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
                sdf.stroke(self.border_color, self.border_size)
            }
            return sdf.result
        }
    }

    let KindIcon = mod.draw.DrawSvg{}

    mod.widgets.FlowCanvasBase = #(FlowCanvas::register_widget(vm))
    mod.widgets.FlowCanvas = set_type_default() do mod.widgets.FlowCanvasBase{
        width: Fill
        height: Fill
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

        icon_input: KindIcon{ color: theme.flow_input svg: crate_resource("self:resources/icons/input.svg") }
        icon_output: KindIcon{ color: theme.flow_success svg: crate_resource("self:resources/icons/output.svg") }
        icon_chat: KindIcon{ color: theme.flow_chat svg: crate_resource("self:resources/icons/chat.svg") }
        icon_gen: KindIcon{ color: theme.flow_generation svg: crate_resource("self:resources/icons/gen.svg") }
        icon_fn: KindIcon{ color: theme.flow_function svg: crate_resource("self:resources/icons/fn.svg") }
        icon_http: KindIcon{ color: theme.flow_http svg: crate_resource("self:resources/icons/http.svg") }
        icon_ask: KindIcon{ color: theme.flow_waiting svg: crate_resource("self:resources/icons/ask.svg") }
        icon_flow: KindIcon{ color: theme.flow_text_port svg: crate_resource("self:resources/icons/flow.svg") }

        icon_text: KindIcon{ color: theme.flow_port_text svg: crate_resource("self:resources/icons/text.svg") }
        icon_image: KindIcon{ color: theme.flow_port_image svg: crate_resource("self:resources/icons/image.svg") }
        icon_audio: KindIcon{ color: theme.flow_port_audio svg: crate_resource("self:resources/icons/audio.svg") }
        icon_video: KindIcon{ color: theme.flow_port_video svg: crate_resource("self:resources/icons/video.svg") }
        icon_mesh: KindIcon{ color: theme.flow_port_mesh svg: crate_resource("self:resources/icons/mesh.svg") }
        icon_json: KindIcon{ color: theme.flow_port_json svg: crate_resource("self:resources/icons/json.svg") }
        icon_bytes: KindIcon{ color: theme.flow_port_bytes svg: crate_resource("self:resources/icons/bytes.svg") }

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

#[derive(Clone, Debug, Default)]
pub enum FlowCanvasAction {
    #[default]
    None,
    Select(Option<String>),
    Edit(CanvasEdit),
    /// A wire was dropped on empty canvas: open the palette filtered to
    /// types with an input of this type; `at` is the world position.
    OpenPalette {
        at: (f64, f64),
        from_node: String,
        from_port: String,
        ty: PortType,
    },
    /// The camera moved (pan or zoom); the app mirrors it in the toolbar.
    Camera {
        scale: f64,
    },
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
        ty: PortType,
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
#[derive(Clone, Copy)]
struct EdgeIndex {
    from: usize,
    from_port: usize,
    to: usize,
    to_port: usize,
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
    icon_input: DrawSvg,
    #[live]
    icon_output: DrawSvg,
    #[live]
    icon_chat: DrawSvg,
    #[live]
    icon_gen: DrawSvg,
    #[live]
    icon_fn: DrawSvg,
    #[live]
    icon_http: DrawSvg,
    #[live]
    icon_ask: DrawSvg,
    #[live]
    icon_flow: DrawSvg,
    #[live]
    icon_text: DrawSvg,
    #[live]
    icon_image: DrawSvg,
    #[live]
    icon_audio: DrawSvg,
    #[live]
    icon_video: DrawSvg,
    #[live]
    icon_mesh: DrawSvg,
    #[live]
    icon_json: DrawSvg,
    #[live]
    icon_bytes: DrawSvg,
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
    graph_index: GraphIndex,
    /// Back-to-front card order; the back of this vector is screen-front.
    #[rust]
    z_order: Vec<String>,
    /// Card heights per node index, measured from the faces last frame.
    #[rust]
    heights: Vec<f64>,
    #[rust]
    edges: Vec<EdgeIndex>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    selected: Option<String>,
    #[rust]
    highlight: Option<String>,
    #[rust]
    pub statuses: HashMap<String, NodeStatus>,
    #[rust]
    pub streaming: HashSet<String>,
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

impl FlowCanvas {
    // -- the app's view of the canvas ------------------------------------------

    /// The face roots the app mounted for the bound instance; cleared before
    /// the app frees that isolate.
    pub fn set_face_roots(&mut self, cx: &mut Cx, roots: Vec<(LiveId, WidgetRef)>) {
        self.face_roots = roots;
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    pub fn set_graph(&mut self, cx: &mut Cx, graph: Option<Graph>) {
        let mut graph = graph;
        if let Some(graph) = graph.as_mut() {
            graph_edit::auto_place(graph);
        }
        // Keep the measured heights of the nodes that survive.
        let old = self.graph.take();
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
                card_draw_lists.push(
                    old_position
                        .and_then(|index| old_lists.get_mut(index))
                        .and_then(Option::take),
                );
            }
            for edge in &next.edges {
                let from = node_index.get(&edge.from_node).copied();
                let to = node_index.get(&edge.to_node).copied();
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
                    .position(|p| p.port == edge.to_port);
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
        self.heights = heights;
        self.edges = edges;
        self.card_draw_lists = card_draw_lists;
        self.node_index = node_index;
        self.graph_index = graph
            .as_ref()
            .map(graph_edit::GraphIndex::new)
            .unwrap_or_default();
        self.graph = graph;
        self.hover = None;
        if let Some(selected) = self.selected.clone() {
            if !self.has_node(&selected) {
                self.selected = None;
                cx.widget_action(self.uid, FlowCanvasAction::Select(None));
            }
        }
        self.redraw(cx);
    }

    /// A different flow opened: fit it once its faces have been measured.
    pub fn reset_view(&mut self, cx: &mut Cx) {
        self.fit_pending = 2;
        self.redraw(cx);
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, cx: &mut Cx, node: Option<String>) {
        if let Some(node) = node.as_deref() {
            self.raise_node(node);
        }
        self.selected = node;
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
        if node.is_some() && self.selected != node {
            self.raise_node(node.as_deref().unwrap());
            self.selected = node.clone();
            cx.widget_action(self.uid, FlowCanvasAction::Select(node));
            self.redraw(cx);
        }
    }

    fn raise_node(&mut self, node: &str) {
        raise_to_front(&mut self.z_order, node);
    }

    fn compatible_for(&self, graph: &Graph, from: usize, from_port: usize) -> Vec<(usize, usize)> {
        let from_id = &graph.nodes[from].id;
        let port_name = &graph.nodes[from].outputs[from_port].name;
        self.graph_index.compatible_inputs(graph, from_id, port_name)
    }

    pub fn is_resize_handle_at(&self, abs: DVec2) -> bool {
        self.resize_at(abs).is_some()
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
            let (x, y) = node.at.unwrap_or(graph_edit::FIRST_AT);
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

    // -- geometry (local units) -----------------------------------------------

    /// The node's world position, with a live drag applied.
    fn node_at(&self, graph: &Graph, index: usize) -> (f64, f64) {
        let node = &graph.nodes[index];
        let mut at = node.at.unwrap_or(graph_edit::FIRST_AT);
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
        match node.kind.as_str() {
            "output" => declared_output_type(node)
                .or_else(|| node.inputs.first().map(|input| input.ty))
                == Some(PortType::Image),
            "input" => node.outputs.first().is_some_and(|port| port.ty == PortType::Image),
            "gen" => node.outputs.first().is_some_and(|port| port.ty == PortType::Image),
            _ => false,
        }
    }

    fn input_type(node: &Node, port: usize) -> PortType {
        if port == 0 {
            declared_output_type(node).unwrap_or(node.inputs[port].ty)
        } else {
            node.inputs[port].ty
        }
    }

    fn port_rows(node: &Node) -> usize {
        if Self::full_bleed(node) {
            0
        } else {
            node.inputs.len().max(node.outputs.len())
        }
    }

    fn port_local(&self, graph: &Graph, index: usize, port: usize, output: bool) -> DVec2 {
        let rect = self.card_rect(graph, index);
        let y = rect.pos.y + CARD_HEADER_H + (port as f64 + 0.5) * PORT_ROW_H;
        if output {
            dvec2(rect.pos.x + rect.size.x, y)
        } else {
            dvec2(rect.pos.x, y)
        }
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

    fn port_color(&self, ty: PortType) -> Vec4f {
        match ty {
            PortType::Text => self.color_port_text,
            PortType::Image => self.color_port_image,
            PortType::Audio => self.color_port_audio,
            PortType::Video => self.color_port_video,
            PortType::Mesh => self.color_port_mesh,
            PortType::Json => self.color_port_json,
            PortType::List => self.color_port_list,
            PortType::Bytes => self.color_port_bytes,
        }
    }

    fn kind_color(&self, kind: &str) -> Vec4f {
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

    fn wire_ctrl(from: DVec2, to: DVec2) -> f64 {
        ((to.x - from.x).abs() * 0.5).max(48.0)
    }

    fn bezier(v: &mut DrawVector, from: DVec2, to: DVec2) {
        let dx = Self::wire_ctrl(from, to);
        v.move_to(from.x as f32, from.y as f32);
        v.bezier_to(
            (from.x + dx) as f32,
            from.y as f32,
            (to.x - dx) as f32,
            to.y as f32,
            to.x as f32,
            to.y as f32,
        );
    }

    fn bezier_point(from: DVec2, to: DVec2, t: f64) -> DVec2 {
        let dx = Self::wire_ctrl(from, to);
        let c1 = dvec2(from.x + dx, from.y);
        let c2 = dvec2(to.x - dx, to.y);
        let u = 1.0 - t;
        dvec2(
            u * u * u * from.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * to.x,
            u * u * u * from.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * to.y,
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
        self.draw_vec.begin();
        // Wires, under the cards.
        for edge in self.edges.iter().copied() {
            let a = self.port_local(graph, edge.from, edge.from_port, true);
            let b = self.port_local(graph, edge.to, edge.to_port, false);
            let ty = graph.nodes[edge.from].outputs[edge.from_port].ty;
            let color = self.port_color(ty);
            let streaming = self.streaming.contains(&graph.nodes[edge.from].id);
            if streaming {
                Self::set_color(&mut self.draw_vec, color, 0.22);
                Self::bezier(&mut self.draw_vec, a, b);
                self.draw_vec.stroke(10.0);
            }
            Self::set_color(&mut self.draw_vec, color, if dragging_wire { 0.35 } else { 0.95 });
            Self::bezier(&mut self.draw_vec, a, b);
            self.draw_vec.stroke(3.0);
            if streaming {
                for k in 0..3 {
                    let t = (time * 0.55 + k as f64 / 3.0).fract();
                    let p = Self::bezier_point(a, b, t);
                    self.draw_vec.set_color(1.0, 1.0, 1.0, 0.85);
                    self.draw_vec.circle(p.x as f32, p.y as f32, 3.5);
                    self.draw_vec.fill();
                }
            }
        }
        // The wire being dragged.
        if let Some(Drag::Wire {
            from,
            from_port,
            ty,
            pos,
            ..
        }) = self.drag
        {
            let a = self.port_local(graph, from, from_port, true);
            let b = self.camera.screen_to_local(pos);
            let color = self.port_color(ty);
            Self::set_color(&mut self.draw_vec, color, 1.0);
            Self::bezier(&mut self.draw_vec, a, b);
            self.draw_vec.stroke(3.0);
        }
        self.draw_vec.end(cx);
    }

    /// Card shadow and body. Selection is deliberately drawn later in the
    /// canvas list so it remains above every card child.
    fn draw_card(&mut self, cx: &mut Cx2d, graph: &Graph, indices: &[usize]) {
        self.draw_card.shadow_radius = (12.0 / self.camera.scale.max(0.01)) as f32;
        self.draw_card.shadow_offset = vec2(0.0, 0.0);
        let hover = self.hover;
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            let rect = self.card_rect(graph, index);
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
            self.draw_card.draw_abs(cx, rect);
        }
    }

    fn kind_icon(&mut self, kind: &str) -> &mut DrawSvg {
        match kind {
            "input" => &mut self.icon_input,
            "output" => &mut self.icon_output,
            "chat" => &mut self.icon_chat,
            "gen" => &mut self.icon_gen,
            "fn" => &mut self.icon_fn,
            "http" => &mut self.icon_http,
            "ask" => &mut self.icon_ask,
            _ => &mut self.icon_flow,
        }
    }

    fn port_icon(&mut self, ty: PortType) -> &mut DrawSvg {
        match PortIcon::for_type(ty) {
            PortIcon::Text => &mut self.icon_text,
            PortIcon::Image => &mut self.icon_image,
            PortIcon::Audio => &mut self.icon_audio,
            PortIcon::Video => &mut self.icon_video,
            PortIcon::Mesh => &mut self.icon_mesh,
            PortIcon::Json => &mut self.icon_json,
            PortIcon::Bytes => &mut self.icon_bytes,
        }
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
            if let Some(ty) = declared_output_type(node) {
                self.port_icon(ty).draw_abs(cx, icon_rect);
            } else {
                self.kind_icon(&node.kind).draw_abs(cx, icon_rect);
            }
            let id_w = self.text_width(cx, &self.draw_title, &node.id);
            self.draw_title
                .draw_abs(cx, dvec2(r.pos.x + 23.0, label_y + 6.0), &node.id);
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
                    let connected = matches!(input.value, NodeInputValue::Edge(_));
                    self.draw_port.color = if connected {
                        self.color_port_label_connected
                    } else {
                        self.color_port_label_open
                    };
                    self.draw_port
                        .draw_abs(cx, dvec2(p.x + PORT_R + 8.0, p.y - 6.0), &input.port);
                }
                for (port, output) in node.outputs.iter().enumerate() {
                    let p = self.port_local(graph, index, port, true);
                    let w = self.text_width(cx, &self.draw_port, &output.name);
                    self.draw_port.color = self.color_port_label_connected;
                    self.draw_port
                        .draw_abs(cx, dvec2(p.x - PORT_R - 8.0 - w, p.y - 6.0), &output.name);
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
        if let Some(faces) = scope.data.get_mut::<FaceHost>() {
            faces.set_popup_anchor_transform(cx, Some(self.camera.popup_anchor_transform()));
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
            if let Some(faces) = scope.data.get_mut::<FaceHost>() {
                faces.draw_face(
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
            // Ports: a dark disc with a ring in the port-type colour.
            for port in 0..node.inputs.len() {
                let p = self.port_local(graph, index, port, false);
                let ok = !compatible_active || self.compatible.contains(&(index, port));
                let hot = wire_target == Some((index, port));
                let radius = if hot { PORT_R + 3.0 } else { PORT_R } as f32;
                Self::set_color(&mut self.draw_over, self.card_color, 1.0);
                self.draw_over.circle(p.x as f32, p.y as f32, radius);
                self.draw_over.fill();
                let color = self.port_color(Self::input_type(node, port));
                Self::set_color(&mut self.draw_over, color, if ok { 1.0 } else { 0.25 });
                self.draw_over.circle(p.x as f32, p.y as f32, radius - 1.0);
                self.draw_over.stroke(if hot { 3.0 } else { 2.0 });
            }
            for (port, output) in node.outputs.iter().enumerate() {
                let p = self.port_local(graph, index, port, true);
                Self::set_color(&mut self.draw_over, self.card_color, 1.0);
                self.draw_over.circle(p.x as f32, p.y as f32, PORT_R as f32);
                self.draw_over.fill();
                let color = self.port_color(output.ty);
                Self::set_color(&mut self.draw_over, color, 1.0);
                self.draw_over
                    .circle(p.x as f32, p.y as f32, (PORT_R - 1.0) as f32);
                self.draw_over.stroke(2.0);
            }
            // Three unobtrusive diagonals, in the card's own local space.
            Self::set_color(&mut self.draw_over, self.draw_meta.color, 0.55);
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
        // The port-type icons inside the discs.
        for index in indices.iter().copied() {
            let node = &graph.nodes[index];
            for port in 0..node.inputs.len() {
                let p = self.port_local(graph, index, port, false);
                let rect = Rect {
                    pos: p - dvec2(5.5, 5.5),
                    size: dvec2(11.0, 11.0),
                };
                self.port_icon(Self::input_type(node, port)).draw_abs(cx, rect);
            }
            for (port, output) in node.outputs.iter().enumerate() {
                let p = self.port_local(graph, index, port, true);
                let rect = Rect {
                    pos: p - dvec2(5.5, 5.5),
                    size: dvec2(11.0, 11.0),
                };
                self.port_icon(output.ty).draw_abs(cx, rect);
            }
        }
    }

    /// Placement ghost and selection ring draw after every card list.
    fn draw_top_overlay(&mut self, cx: &mut Cx2d, graph: &Graph) {
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
        if let Some(index) = self
            .selected
            .as_ref()
            .and_then(|id| self.node_index.get(id))
            .copied()
        {
            let rect = self.card_rect(graph, index);
            Self::set_color(&mut self.draw_over, self.accent_color, 1.0);
            self.draw_over.rounded_rect(
                rect.pos.x as f32,
                rect.pos.y as f32,
                rect.size.x as f32,
                rect.size.y as f32,
                CARD_RADIUS,
            );
            self.draw_over.stroke(2.0);
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
            let to_port = graph.nodes[to].inputs[to_port].port.clone();
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
        !self.streaming.is_empty()
            || self.statuses.values().any(|status| {
                matches!(status.state.as_str(), "running" | "waiting" | "queued")
                    || (status.shown - status.target_fraction()).abs() > 1e-3
            })
            || (self.camera.pan - self.target_pan).length() > 0.05
            || (self.camera.scale - self.target_scale).abs() > 1e-4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_port_type_icon_exists() {
        let types = [
            PortType::Text,
            PortType::Image,
            PortType::Audio,
            PortType::Video,
            PortType::Mesh,
            PortType::Json,
            PortType::List,
            PortType::Bytes,
        ];
        for ty in types {
            let path = PortIcon::for_type(ty).path();
            assert!(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(path)
                    .is_file(),
                "missing icon for {}: {path}",
                ty.as_str()
            );
        }
    }

    #[test]
    fn card_selection_moves_the_whole_retained_card_to_the_front() {
        let mut order = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        raise_to_front(&mut order, "b");
        assert_eq!(order, vec!["a", "c", "b"]);
        raise_to_front(&mut order, "b");
        assert_eq!(order, vec!["a", "c", "b"]);
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
        if let Some(graph) = self.graph.take() {
            self.draw_wires(cx, &graph);
            let z_order = std::mem::take(&mut self.z_order);
            if let Some(faces) = scope.data.get_mut::<FaceHost>() {
                faces.set_z_order(&z_order);
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
                self.draw_card(cx, &graph, one);
                self.draw_labels(cx, &graph, one);
                heights_changed |= self.draw_faces(cx, scope, &graph, one);
                self.draw_overlays(cx, &graph, one);
                card_list.end(cx);
                self.card_draw_lists[index] = Some(card_list);
            }
            self.z_order = z_order;
            self.draw_top_overlay(cx, &graph);
            self.graph = Some(graph);
        }
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle();
        draw_list.end(cx);
        draw_list.set_view_transform(cx, &self.camera.matrix());
        self.draw_list = Some(draw_list);
        if heights_changed {
            // Wires, ports and outlines were drawn against last frame's heights.
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
        if let Some(nf) = self.next_frame.is_event(event) {
            let dt = (nf.time - self.last_time).clamp(0.0, 0.1);
            self.last_time = nf.time;
            self.time = nf.time;
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
                let hover = if self.camera.view.contains(e.abs) && !over_chrome {
                    self.node_index_at(e.abs)
                } else {
                    None
                };
                if hover != self.hover {
                    self.hover = hover;
                    self.area.redraw(cx);
                }
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerScroll(fs) if !self.point_over_chrome(fs.abs) => {
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
                            ty: node.outputs[hit.port].ty,
                            pos: fd.abs,
                            target: None,
                        });
                        self.compatible = compatible;
                    } else {
                        // An input with a wire: pick the wire up again from
                        // its source; a bare one does nothing.
                        let input = &node.inputs[hit.port];
                        if let NodeInputValue::Edge(edge) = &input.value {
                            let from = self.node_index.get(&edge.from_node).copied();
                            let from_port = from.and_then(|from| {
                                graph.nodes[from]
                                    .outputs
                                    .iter()
                                    .position(|p| p.name == edge.from_port)
                            });
                            if let (Some(from), Some(from_port)) = (from, from_port) {
                                let ty = graph.nodes[from].outputs[from_port].ty;
                                let compatible = self.compatible_for(graph, from, from_port);
                                let to_node = node.id.clone();
                                let to_port = input.port.clone();
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
                    if self.selected.as_deref() != Some(id.as_str()) {
                        self.selected = Some(id.clone());
                        cx.widget_action(self.uid, FlowCanvasAction::Select(Some(id)));
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
                    let origin = node.at.unwrap_or(graph_edit::FIRST_AT);
                    self.raise_node(&id);
                    if self.selected.as_deref() != Some(id.as_str()) {
                        self.selected = Some(id.clone());
                        cx.widget_action(self.uid, FlowCanvasAction::Select(Some(id)));
                    }
                    self.drag = Some(Drag::Node {
                        index,
                        start: fd.abs,
                        origin,
                        moved: false,
                    });
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
                        let graph_at = self
                            .graph
                            .as_ref()
                            .and_then(|graph| graph.nodes.get(index))
                            .and_then(|node| node.at)
                            .unwrap_or(graph_edit::FIRST_AT);
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
                cx.set_cursor(MouseCursor::Default);
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
                    if let Some(node) = self.selected.take() {
                        cx.widget_action(
                            self.uid,
                            FlowCanvasAction::Edit(CanvasEdit::Delete { node }),
                        );
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
