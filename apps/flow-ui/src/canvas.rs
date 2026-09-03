//! `FlowCanvas`: the graph of one instance — node frames around mounted
//! faces, port dots, bezier wires in one `DrawVector` batch, drag-to-move,
//! drag-to-connect, a pan camera and three discrete sizes.
//!
//! Zoom decision (the F3 spike, 2026-09-03): the draw-list `view_transform`
//! scales what is drawn but NOT what is hit — `Area::clipped_rect`
//! (platform/src/area.rs) reads `rect_pos`/`rect_size` straight from the
//! instance buffer and only applies `view_shift`/`view_clip`, and
//! `Event::hits_with_options_and_test` (platform/src/event/finger.rs) tests
//! the raw pointer `abs` against that rect; nothing on the event path calls
//! `map_point_to_local`. Verified live under the bridge: with this list under
//! a 1.5× transform the prompt face's `TextInput` drew at 1.5× its layout
//! rect, a `/click` + `/t` on the drawn box typed nothing, and a `/click` on
//! the untransformed rect (empty screen) typed into it. Remapping `abs` for
//! the faces would still leave `/snap`, the tweaker, IME cursor rects and
//! popup menus (drawn in untransformed overlay lists) in the wrong space, so
//! v1 zoom is three discrete sizes (frame width, header, ports, wires) with a
//! pan-only camera: every widget rect stays in window space, every hit is
//! exact, and the faces keep their natural size inside a wider or narrower
//! frame.

use crate::faces::FaceHost;
use crate::graph_edit::{self, NODE_WIDTH};
use makepad_flow::{Graph, NodeInputValue, PortType};
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};

const HEADER_H: f64 = 26.0;
const PORT_ROW_H: f64 = 18.0;
const FRAME_PAD: f64 = 6.0;
const DOT_R: f64 = 5.0;
const DOT_HIT_R: f64 = 11.0;
const DRAG_THRESHOLD: f64 = 3.0;
const ZOOM_SCALES: [f64; 3] = [0.7, 1.0, 1.3];
const WHEEL_STEP: f64 = 40.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FlowCanvasBase = #(FlowCanvas::register_widget(vm))
    mod.widgets.FlowCanvas = set_type_default() do mod.widgets.FlowCanvasBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: theme.color_bg_app
        }
        draw_frame +: {
            color: theme.color_bg_container
        }
        draw_outline +: {
            color: theme.color_makepad
        }
        draw_header +: {
            color: theme.color_d_3
        }
        draw_text +: {
            text_style: theme.font_bold{font_size: 9.5}
            color: theme.color_label_inner
        }
        draw_port_text +: {
            text_style: theme.font_regular{font_size: 8.5}
            color: theme.color_text_meta
        }
        draw_chip_text +: {
            text_style: theme.font_code{font_size: 8}
            color: theme.color_label_inner
        }
    }
}

#[derive(Clone, Debug)]
pub enum CanvasEdit {
    Move {
        node: String,
        at: (f64, f64),
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
}

#[derive(Clone, Debug)]
enum Drag {
    Pan {
        start: DVec2,
        origin: DVec2,
    },
    Node {
        id: String,
        start: DVec2,
        origin: (f64, f64),
        moved: bool,
    },
    Wire {
        from_node: String,
        from_port: String,
        ty: PortType,
        pos: DVec2,
        target: Option<(String, String)>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct NodeStatus {
    pub state: String,
    pub permille: u16,
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
struct PortHit {
    node: usize,
    port: usize,
    output: bool,
    pos: DVec2,
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
    draw_bg: DrawColor,
    #[live]
    draw_frame: DrawColor,
    #[live]
    draw_outline: DrawColor,
    #[live]
    draw_header: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_port_text: DrawText,
    #[live]
    draw_chip_text: DrawText,
    #[live]
    draw_wire: DrawVector,

    #[rust]
    area: Area,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    view_rect: Rect,
    #[rust]
    graph: Option<Graph>,
    #[rust]
    pan: DVec2,
    #[rust(1usize)]
    zoom: usize,
    #[rust]
    wheel_acc: f64,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    selected: Option<String>,
    #[rust]
    highlight: Option<String>,
    #[rust]
    heights: HashMap<String, f64>,
    #[rust]
    pub statuses: HashMap<String, NodeStatus>,
    #[rust]
    pub streaming: HashSet<String>,
    #[rust]
    chips: HashMap<(String, String), String>,
    #[rust]
    pub armed_type: Option<String>,
    /// The mounted face roots, so the widget tree (and `/snap`) reaches
    /// them through this canvas; the app keeps it in step with its FaceHost.
    #[rust]
    face_roots: Vec<(LiveId, WidgetRef)>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    fitted: bool,
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
        for (_, root) in &self.face_roots {
            root.find_widgets_from_point(cx, point, found);
        }
    }
}

impl FlowCanvas {
    /// The face roots the app mounted for the bound instance; cleared before
    /// the app frees that isolate.
    pub fn set_face_roots(&mut self, cx: &mut Cx, roots: Vec<(LiveId, WidgetRef)>) {
        self.face_roots = roots;
        // The widget tree re-reads this node's children on its next lookup,
        // so `/snap` and `ids!` paths reach the faces.
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    pub fn set_graph(&mut self, cx: &mut Cx, graph: Option<Graph>) {
        let mut graph = graph;
        if let Some(graph) = graph.as_mut() {
            graph_edit::auto_place(graph);
        }
        self.graph = graph;
        if let Some(selected) = self.selected.clone() {
            if !self.has_node(&selected) {
                self.selected = None;
                cx.widget_action(self.uid, FlowCanvasAction::Select(None));
            }
        }
        if !self.fitted && self.view_rect.size.x > 0.0 {
            self.fit();
        }
        self.redraw(cx);
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, cx: &mut Cx, node: Option<String>) {
        self.selected = node;
        self.redraw(cx);
    }

    pub fn set_highlight(&mut self, cx: &mut Cx, node: Option<String>) {
        if self.highlight != node {
            self.highlight = node;
            self.redraw(cx);
        }
    }

    pub fn set_status(&mut self, cx: &mut Cx, node: &str, status: NodeStatus) {
        self.statuses.insert(node.to_string(), status);
        self.redraw(cx);
    }

    pub fn clear_run(&mut self, cx: &mut Cx) {
        self.statuses.clear();
        self.streaming.clear();
        self.chips.clear();
        self.redraw(cx);
    }

    pub fn set_chip(&mut self, cx: &mut Cx, node: &str, port: &str, text: String) {
        let mut text: String = text.chars().take(28).collect();
        text = text.replace('\n', " ");
        self.chips.insert((node.to_string(), port.to_string()), text);
        self.redraw(cx);
    }

    pub fn set_streaming(&mut self, cx: &mut Cx, node: &str, on: bool) {
        if on {
            self.streaming.insert(node.to_string());
            self.next_frame = cx.new_next_frame();
        } else {
            self.streaming.remove(node);
        }
        self.redraw(cx);
    }

    fn has_node(&self, id: &str) -> bool {
        self.graph
            .as_ref()
            .is_some_and(|graph| graph.nodes.iter().any(|node| node.id == id))
    }

    fn scale(&self) -> f64 {
        ZOOM_SCALES[self.zoom.min(ZOOM_SCALES.len() - 1)]
    }

    fn to_screen(&self, world: (f64, f64)) -> DVec2 {
        let s = self.scale();
        dvec2(
            self.view_rect.pos.x + self.pan.x + world.0 * s,
            self.view_rect.pos.y + self.pan.y + world.1 * s,
        )
    }

    fn to_world(&self, screen: DVec2) -> (f64, f64) {
        let s = self.scale();
        (
            (screen.x - self.view_rect.pos.x - self.pan.x) / s,
            (screen.y - self.view_rect.pos.y - self.pan.y) / s,
        )
    }

    /// Fit every node into the view at the normal size.
    pub fn fit(&mut self) {
        let Some(graph) = self.graph.as_ref() else {
            return;
        };
        if graph.nodes.is_empty() {
            self.pan = dvec2(0.0, 0.0);
            self.zoom = 1;
            self.fitted = true;
            return;
        }
        let mut min = dvec2(f64::MAX, f64::MAX);
        let mut max = dvec2(f64::MIN, f64::MIN);
        for node in &graph.nodes {
            let (x, y) = node.at.unwrap_or(graph_edit::FIRST_AT);
            let h = self.heights.get(&node.id).copied().unwrap_or(200.0);
            min.x = min.x.min(x);
            min.y = min.y.min(y);
            max.x = max.x.max(x + NODE_WIDTH);
            max.y = max.y.max(y + h);
        }
        let span = max - min;
        let zoom = if self.view_rect.size.x <= 0.0 {
            1
        } else {
            let fits = |s: f64| {
                span.x * s + 80.0 <= self.view_rect.size.x && span.y * s + 80.0 <= self.view_rect.size.y
            };
            if fits(ZOOM_SCALES[1]) {
                1
            } else {
                0
            }
        };
        self.zoom = zoom;
        let s = self.scale();
        self.pan = dvec2(
            (self.view_rect.size.x - span.x * s) * 0.5 - min.x * s,
            (self.view_rect.size.y - span.y * s) * 0.5 - min.y * s,
        );
        if span.x * s + 80.0 > self.view_rect.size.x {
            self.pan.x = 40.0 - min.x * s;
        }
        if span.y * s + 80.0 > self.view_rect.size.y {
            self.pan.y = 40.0 - min.y * s;
        }
        self.fitted = true;
    }

    fn zoom_step(&mut self, cursor: DVec2, delta: i32) {
        let next = (self.zoom as i32 + delta).clamp(0, ZOOM_SCALES.len() as i32 - 1) as usize;
        if next == self.zoom {
            return;
        }
        // The world point under the cursor stays under the cursor.
        let world = self.to_world(cursor);
        self.zoom = next;
        let s = self.scale();
        self.pan = dvec2(
            cursor.x - self.view_rect.pos.x - world.0 * s,
            cursor.y - self.view_rect.pos.y - world.1 * s,
        );
    }

    fn node_rect(&self, index: usize) -> Option<Rect> {
        let graph = self.graph.as_ref()?;
        let node = graph.nodes.get(index)?;
        let s = self.scale();
        let at = self.node_at(index)?;
        let pos = self.to_screen(at);
        let height = self.heights.get(&node.id).copied().unwrap_or(120.0);
        Some(Rect {
            pos,
            size: dvec2(NODE_WIDTH * s, height),
        })
    }

    /// The node's world position, with a live drag applied.
    fn node_at(&self, index: usize) -> Option<(f64, f64)> {
        let graph = self.graph.as_ref()?;
        let node = graph.nodes.get(index)?;
        let mut at = node.at.unwrap_or(graph_edit::FIRST_AT);
        if let Some(Drag::Node {
            id, start, origin, ..
        }) = &self.drag
        {
            if *id == node.id {
                let _ = start;
                at = *origin;
            }
        }
        Some(at)
    }

    fn port_rows(&self, index: usize) -> usize {
        self.graph
            .as_ref()
            .and_then(|graph| graph.nodes.get(index))
            .map(|node| node.inputs.len().max(node.outputs.len()))
            .unwrap_or(0)
    }

    fn port_pos(&self, index: usize, port: usize, output: bool) -> Option<DVec2> {
        let rect = self.node_rect(index)?;
        let s = self.scale();
        let y = rect.pos.y + HEADER_H * s + (port as f64 + 0.5) * PORT_ROW_H * s;
        Some(if output {
            dvec2(rect.pos.x + rect.size.x, y)
        } else {
            dvec2(rect.pos.x, y)
        })
    }

    fn port_at(&self, abs: DVec2) -> Option<PortHit> {
        let graph = self.graph.as_ref()?;
        let r = DOT_HIT_R * self.scale().max(1.0);
        for (index, node) in graph.nodes.iter().enumerate() {
            for (port, _) in node.inputs.iter().enumerate() {
                let pos = self.port_pos(index, port, false)?;
                if (pos - abs).length() <= r {
                    return Some(PortHit {
                        node: index,
                        port,
                        output: false,
                        pos,
                    });
                }
            }
            for (port, _) in node.outputs.iter().enumerate() {
                let pos = self.port_pos(index, port, true)?;
                if (pos - abs).length() <= r {
                    return Some(PortHit {
                        node: index,
                        port,
                        output: true,
                        pos,
                    });
                }
            }
        }
        None
    }

    fn node_index_at(&self, abs: DVec2) -> Option<usize> {
        let graph = self.graph.as_ref()?;
        (0..graph.nodes.len())
            .rev()
            .find(|index| self.node_rect(*index).is_some_and(|rect| rect.contains(abs)))
    }

    fn wire_color(ty: PortType) -> Vec4f {
        match ty {
            PortType::Text => vec4(0.55, 0.75, 1.0, 1.0),
            PortType::Image => vec4(1.0, 0.65, 0.35, 1.0),
            PortType::Audio => vec4(0.75, 0.55, 1.0, 1.0),
            PortType::Video => vec4(1.0, 0.5, 0.7, 1.0),
            PortType::Mesh => vec4(0.6, 0.9, 0.6, 1.0),
            PortType::Json => vec4(0.95, 0.85, 0.4, 1.0),
            PortType::List => vec4(0.8, 0.9, 0.5, 1.0),
            PortType::Bytes => vec4(0.7, 0.7, 0.7, 1.0),
        }
    }

    fn state_color(state: &str) -> Vec4f {
        match state {
            "running" | "ready" => vec4(0.35, 0.65, 1.0, 1.0),
            "done" => vec4(0.25, 0.8, 0.4, 1.0),
            "failed" => vec4(0.95, 0.3, 0.3, 1.0),
            "waiting" => vec4(1.0, 0.75, 0.2, 1.0),
            "skipped" | "cancelled" => vec4(0.6, 0.6, 0.6, 1.0),
            _ => vec4(0.45, 0.45, 0.5, 1.0),
        }
    }

    fn bezier(&mut self, from: DVec2, to: DVec2) {
        let dx = ((to.x - from.x).abs() * 0.5).max(40.0);
        self.draw_wire.move_to(from.x as f32, from.y as f32);
        self.draw_wire.bezier_to(
            (from.x + dx) as f32,
            from.y as f32,
            (to.x - dx) as f32,
            to.y as f32,
            to.x as f32,
            to.y as f32,
        );
    }

    fn bezier_point(from: DVec2, to: DVec2, t: f64) -> DVec2 {
        let dx = ((to.x - from.x).abs() * 0.5).max(40.0);
        let c1 = dvec2(from.x + dx, from.y);
        let c2 = dvec2(to.x - dx, to.y);
        let u = 1.0 - t;
        dvec2(
            u * u * u * from.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * to.x,
            u * u * u * from.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * to.y,
        )
    }

    fn draw_wires(&mut self, cx: &mut Cx2d) {
        let Some(graph) = self.graph.clone() else {
            return;
        };
        let s = self.scale();
        let index_of: HashMap<&str, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        self.draw_wire.begin();
        let dragging = matches!(self.drag, Some(Drag::Wire { .. }));
        for edge in &graph.edges {
            let (Some(&from), Some(&to)) = (
                index_of.get(edge.from_node.as_str()),
                index_of.get(edge.to_node.as_str()),
            ) else {
                continue;
            };
            let from_port = graph.nodes[from]
                .outputs
                .iter()
                .position(|port| port.name == edge.from_port);
            let to_port = graph.nodes[to]
                .inputs
                .iter()
                .position(|input| input.port == edge.to_port);
            let (Some(from_port), Some(to_port)) = (from_port, to_port) else {
                continue;
            };
            let ty = graph.nodes[from].outputs[from_port].ty;
            let (Some(a), Some(b)) = (
                self.port_pos(from, from_port, true),
                self.port_pos(to, to_port, false),
            ) else {
                continue;
            };
            let color = Self::wire_color(ty);
            let alpha = if dragging { 0.35 } else { 0.9 };
            self.draw_wire.set_color(color.x, color.y, color.z, alpha);
            self.bezier(a, b);
            self.draw_wire.stroke((2.0 * s) as f32);
            if self.streaming.contains(&edge.from_node) {
                for k in 0..3 {
                    let t = ((self.time * 0.6 + k as f64 / 3.0) % 1.0).clamp(0.0, 1.0);
                    let p = Self::bezier_point(a, b, t);
                    self.draw_wire.set_color(1.0, 1.0, 1.0, 0.9);
                    self.draw_wire
                        .circle(p.x as f32, p.y as f32, (3.0 * s) as f32);
                    self.draw_wire.fill();
                }
            }
        }
        if let Some(Drag::Wire {
            from_node,
            from_port,
            ty,
            pos,
            ..
        }) = self.drag.clone()
        {
            if let Some(&from) = index_of.get(from_node.as_str()) {
                if let Some(port) = graph.nodes[from]
                    .outputs
                    .iter()
                    .position(|port| port.name == from_port)
                {
                    if let Some(a) = self.port_pos(from, port, true) {
                        let color = Self::wire_color(ty);
                        self.draw_wire.set_color(color.x, color.y, color.z, 1.0);
                        self.bezier(a, pos);
                        self.draw_wire.stroke((2.0 * s) as f32);
                    }
                }
            }
        }
        self.draw_wire.end(cx);
    }

    fn draw_nodes(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        let Some(graph) = self.graph.clone() else {
            return;
        };
        let s = self.scale();
        let compatible: Option<HashSet<(String, String)>> =
            if let Some(Drag::Wire {
                from_node,
                from_port,
                ..
            }) = &self.drag
            {
                Some(
                    graph_edit::compatible_inputs(&graph, from_node, from_port)
                        .into_iter()
                        .collect(),
                )
            } else {
                None
            };
        let wire_target = match &self.drag {
            Some(Drag::Wire { target, .. }) => target.clone(),
            _ => None,
        };
        let mut new_heights = HashMap::new();
        let time = self.time;
        for index in 0..graph.nodes.len() {
            let node = &graph.nodes[index];
            let Some(at) = self.node_at(index) else {
                continue;
            };
            let pos = self.to_screen(at);
            let width = NODE_WIDTH * s;
            let rows = self.port_rows(index);
            let status = self.statuses.get(&node.id).cloned().unwrap_or_default();
            let selected = self.selected.as_deref() == Some(node.id.as_str());
            let highlighted = self.highlight.as_deref() == Some(node.id.as_str());
            let height_guess = self.heights.get(&node.id).copied().unwrap_or(0.0);
            if (selected || highlighted) && height_guess > 0.0 {
                self.draw_outline.color = if selected {
                    vec4(1.0, 0.36, 0.22, 1.0)
                } else {
                    vec4(0.35, 0.65, 1.0, 0.8)
                };
                self.draw_outline.draw_abs(
                    cx,
                    Rect {
                        pos: pos - dvec2(2.0, 2.0),
                        size: dvec2(width + 4.0, height_guess + 4.0),
                    },
                );
            }
            // The frame quad is patched to the turtle's final rect on end().
            self.draw_frame.begin(
                cx,
                Walk {
                    abs_pos: Some(pos),
                    margin: Inset::default(),
                    width: Size::Fixed(width),
                    height: Size::fit(),
                    metrics: Metrics::default(),
                },
                Layout {
                    flow: Flow::Down,
                    padding: Inset {
                        left: FRAME_PAD,
                        right: FRAME_PAD,
                        top: 0.0,
                        bottom: FRAME_PAD,
                    },
                    clip_x: false,
                    clip_y: false,
                    ..Layout::default()
                },
            );
            // Header.
            let header = cx.walk_turtle(Walk::fixed(width - 2.0 * FRAME_PAD, HEADER_H * s));
            self.draw_header.draw_abs(
                cx,
                Rect {
                    pos: dvec2(pos.x, header.pos.y),
                    size: dvec2(width, header.size.y),
                },
            );
            self.draw_text.text_style.font_size = (9.5 * s) as f32;
            self.draw_text.draw_abs(
                cx,
                dvec2(header.pos.x + 4.0, header.pos.y + 6.0 * s),
                &format!("{} · {}", node.id, node.type_name),
            );
            let state = if status.state.is_empty() {
                None
            } else {
                Some(status.state.as_str())
            };
            if let Some(state) = state {
                let chip = if state == "running" && status.permille > 0 {
                    format!("{state} {}%", status.permille / 10)
                } else if state == "running" {
                    let phase = ((time * 2.0) as usize) % 4;
                    format!("{state} {}", ["·", "··", "···", "····"][phase])
                } else {
                    state.to_string()
                };
                let chip_w = 6.0 * s * chip.len() as f64 + 8.0;
                self.draw_chip_text.text_style.font_size = (8.0 * s) as f32;
                self.draw_chip_text.color = Self::state_color(state);
                self.draw_chip_text.draw_abs(
                    cx,
                    dvec2(header.pos.x + header.size.x - chip_w, header.pos.y + 7.0 * s),
                    &chip,
                );
            }
            // Ports.
            let ports = cx.walk_turtle(Walk::fixed(
                width - 2.0 * FRAME_PAD,
                rows as f64 * PORT_ROW_H * s,
            ));
            self.draw_port_text.text_style.font_size = (8.5 * s) as f32;
            for (port, input) in node.inputs.iter().enumerate() {
                let y = ports.pos.y + (port as f64 + 0.5) * PORT_ROW_H * s;
                let dot = dvec2(pos.x, y);
                let dim = compatible
                    .as_ref()
                    .is_some_and(|set| !set.contains(&(node.id.clone(), input.port.clone())));
                let hot = wire_target
                    .as_ref()
                    .is_some_and(|(n, p)| *n == node.id && *p == input.port);
                let mut color = Self::wire_color(input.ty);
                if dim {
                    color.w = 0.25;
                }
                let r = if hot { DOT_R * 1.6 } else { DOT_R } * s;
                self.draw_wire.begin();
                self.draw_wire.set_color(color.x, color.y, color.z, color.w);
                self.draw_wire.circle(dot.x as f32, dot.y as f32, r as f32);
                self.draw_wire.fill();
                self.draw_wire.end(cx);
                let connected = matches!(input.value, NodeInputValue::Edge(_));
                let name = if connected {
                    format!("{} ←", input.port)
                } else {
                    input.port.clone()
                };
                self.draw_port_text.color = if dim {
                    vec4(0.5, 0.5, 0.5, 0.4)
                } else {
                    vec4(0.75, 0.75, 0.8, 1.0)
                };
                self.draw_port_text
                    .draw_abs(cx, dvec2(pos.x + 10.0 * s, y - 6.0 * s), &name);
            }
            for (port, output) in node.outputs.iter().enumerate() {
                let y = ports.pos.y + (port as f64 + 0.5) * PORT_ROW_H * s;
                let dot = dvec2(pos.x + width, y);
                let color = Self::wire_color(output.ty);
                self.draw_wire.begin();
                self.draw_wire.set_color(color.x, color.y, color.z, 1.0);
                self.draw_wire
                    .circle(dot.x as f32, dot.y as f32, (DOT_R * s) as f32);
                self.draw_wire.fill();
                self.draw_wire.end(cx);
                let chip = self.chips.get(&(node.id.clone(), output.name.clone()));
                let label = match chip {
                    Some(chip) => format!("{} · {chip}", output.name),
                    None => output.name.clone(),
                };
                let text_w = (label.chars().count() as f64 * 5.2 * s).min(width - 20.0 * s);
                self.draw_port_text.color = vec4(0.75, 0.75, 0.8, 1.0);
                self.draw_port_text.draw_abs(
                    cx,
                    dvec2(pos.x + width - 10.0 * s - text_w, y - 6.0 * s),
                    &label,
                );
            }
            // The face, mounted in the instance's isolate.
            if let Some(faces) = scope.data.get_mut::<FaceHost>() {
                let error = faces
                    .face(&node.id)
                    .and_then(|face| face.error.clone())
                    .or_else(|| faces.error.clone());
                if let Some(error) = error {
                    self.draw_port_text.color = vec4(0.95, 0.35, 0.35, 1.0);
                    let text: String = error.chars().take(70).collect();
                    let rect = cx.walk_turtle(Walk::fixed(width - 2.0 * FRAME_PAD, 30.0));
                    self.draw_port_text.draw_abs(cx, rect.pos, &text);
                }
                faces.draw_face(
                    cx,
                    &node.id,
                    Walk {
                        abs_pos: None,
                        margin: Inset::default(),
                        width: Size::fill(),
                        height: Size::fit(),
                        metrics: Metrics::default(),
                    },
                );
                if let Some(error) = &status.error {
                    self.draw_port_text.color = vec4(0.95, 0.35, 0.35, 1.0);
                    let text: String = error.chars().take(70).collect();
                    let rect = cx.walk_turtle(Walk::fixed(width - 2.0 * FRAME_PAD, 20.0));
                    self.draw_port_text.draw_abs(cx, rect.pos, &text);
                }
            }
            self.draw_frame.end(cx);
            let rect = self.draw_frame.draw_vars.area.rect(cx);
            let height = rect.size.y.max(HEADER_H * s + 2.0 * FRAME_PAD);
            new_heights.insert(node.id.clone(), height);
        }
        let changed = new_heights
            .iter()
            .any(|(id, h)| (self.heights.get(id).copied().unwrap_or(-1.0) - h).abs() > 0.5);
        self.heights = new_heights;
        if changed {
            // Wires and outlines were drawn against last frame's heights.
            self.area.redraw(cx);
        }
        if !self.fitted && self.view_rect.size.x > 0.0 {
            self.fit();
            self.area.redraw(cx);
        }
    }

    fn finish_wire_drag(&mut self, cx: &mut Cx, drag: Drag) {
        let Drag::Wire {
            from_node,
            from_port,
            ty,
            pos,
            target,
        } = drag
        else {
            return;
        };
        if let Some((to_node, to_port)) = target {
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
            let at = self.to_world(pos);
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
    }
}

impl Widget for FlowCanvas {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        let mut draw_list = self.draw_list.take().unwrap();
        draw_list.begin_always(cx);
        cx.begin_turtle(
            walk,
            Layout {
                clip_x: true,
                clip_y: true,
                ..self.layout
            },
        );
        self.view_rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, self.view_rect);
        self.draw_wires(cx);
        self.draw_nodes(cx, scope);
        if let Some(type_name) = self.armed_type.clone() {
            self.draw_port_text.color = vec4(1.0, 0.8, 0.4, 1.0);
            self.draw_port_text.draw_abs(
                cx,
                self.view_rect.pos + dvec2(12.0, 12.0),
                &format!("release to place {type_name}"),
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        draw_list.end(cx);
        self.draw_list = Some(draw_list);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(nf) = self.next_frame.is_event(event) {
            self.time = nf.time;
            if !self.streaming.is_empty()
                || self.statuses.values().any(|status| status.state == "running")
            {
                self.next_frame = cx.new_next_frame();
                self.area.redraw(cx);
            }
        }
        // A palette type armed by a press elsewhere lands on release here.
        if let Event::MouseUp(e) = event {
            if let Some(type_name) = self.armed_type.take() {
                if self.view_rect.contains(e.abs) {
                    let at = self.to_world(e.abs);
                    cx.widget_action(
                        self.uid,
                        FlowCanvasAction::Edit(CanvasEdit::AddType {
                            type_name,
                            at: (at.0 - 20.0, at.1 - 12.0),
                        }),
                    );
                }
                self.area.redraw(cx);
            }
        }
        if let Event::MouseMove(e) = event {
            if self.armed_type.is_some() && self.view_rect.contains(e.abs) {
                self.area.redraw(cx);
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerScroll(fs) => {
                if fs.scroll.x.abs() > fs.scroll.y.abs() * 1.2 {
                    self.pan.x -= fs.scroll.x;
                } else {
                    self.wheel_acc += fs.scroll.y;
                    if self.wheel_acc.abs() >= WHEEL_STEP {
                        let delta = if self.wheel_acc > 0.0 { -1 } else { 1 };
                        self.wheel_acc = 0.0;
                        self.zoom_step(fs.abs, delta);
                    }
                }
                self.area.redraw(cx);
            }
            Hit::FingerDown(fd) => {
                cx.set_key_focus(self.area);
                if let Some(hit) = self.port_at(fd.abs) {
                    let graph = self.graph.as_ref().unwrap();
                    let node = &graph.nodes[hit.node];
                    if hit.output {
                        let port = &node.outputs[hit.port];
                        self.drag = Some(Drag::Wire {
                            from_node: node.id.clone(),
                            from_port: port.name.clone(),
                            ty: port.ty,
                            pos: hit.pos,
                            target: None,
                        });
                    } else {
                        // An input dot with a wire: pick the wire up again
                        // from its source; a bare one does nothing.
                        let input = &node.inputs[hit.port];
                        if let NodeInputValue::Edge(edge) = &input.value {
                            let to_node = node.id.clone();
                            let to_port = input.port.clone();
                            let ty = graph_edit::output_port_type(graph, &edge.from_node, &edge.from_port)
                                .unwrap_or(input.ty);
                            let from_node = edge.from_node.clone();
                            let from_port = edge.from_port.clone();
                            cx.widget_action(
                                self.uid,
                                FlowCanvasAction::Edit(CanvasEdit::Disconnect { to_node, to_port }),
                            );
                            self.drag = Some(Drag::Wire {
                                from_node,
                                from_port,
                                ty,
                                pos: hit.pos,
                                target: None,
                            });
                        }
                    }
                } else if let Some(index) = self.node_index_at(fd.abs) {
                    let graph = self.graph.as_ref().unwrap();
                    let node = &graph.nodes[index];
                    let id = node.id.clone();
                    let origin = node.at.unwrap_or(graph_edit::FIRST_AT);
                    if self.selected.as_deref() != Some(id.as_str()) {
                        self.selected = Some(id.clone());
                        cx.widget_action(self.uid, FlowCanvasAction::Select(Some(id.clone())));
                    }
                    self.drag = Some(Drag::Node {
                        id,
                        start: fd.abs,
                        origin,
                        moved: false,
                    });
                } else {
                    self.drag = Some(Drag::Pan {
                        start: fd.abs,
                        origin: self.pan,
                    });
                    cx.set_cursor(MouseCursor::Grabbing);
                }
                self.area.redraw(cx);
            }
            Hit::FingerMove(fm) => {
                let s = self.scale();
                match self.drag.clone() {
                    Some(Drag::Pan { start, origin }) => {
                        self.pan = origin + (fm.abs - start);
                    }
                    Some(Drag::Node {
                        id,
                        start,
                        origin,
                        moved,
                    }) => {
                        let delta = fm.abs - start;
                        let moved = moved || delta.length() > DRAG_THRESHOLD;
                        let graph_at = self
                            .graph
                            .as_ref()
                            .and_then(|graph| graph.nodes.iter().find(|node| node.id == id))
                            .and_then(|node| node.at)
                            .unwrap_or(graph_edit::FIRST_AT);
                        let origin = if moved {
                            (graph_at.0 + delta.x / s, graph_at.1 + delta.y / s)
                        } else {
                            origin
                        };
                        self.drag = Some(Drag::Node {
                            id,
                            start,
                            origin,
                            moved,
                        });
                    }
                    Some(Drag::Wire {
                        from_node,
                        from_port,
                        ty,
                        ..
                    }) => {
                        let target = self.port_at(fm.abs).and_then(|hit| {
                            if hit.output {
                                return None;
                            }
                            let graph = self.graph.as_ref()?;
                            let node = &graph.nodes[hit.node];
                            let port = &node.inputs[hit.port];
                            let ok = graph_edit::compatible_inputs(graph, &from_node, &from_port)
                                .iter()
                                .any(|(n, p)| *n == node.id && *p == port.port);
                            ok.then(|| (node.id.clone(), port.port.clone()))
                        });
                        self.drag = Some(Drag::Wire {
                            from_node,
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
                        id, origin, moved, ..
                    }) => {
                        if moved {
                            cx.widget_action(
                                self.uid,
                                FlowCanvasAction::Edit(CanvasEdit::Move { node: id, at: origin }),
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
                KeyCode::Home => {
                    self.fit();
                    self.area.redraw(cx);
                }
                KeyCode::Escape => {
                    self.drag = None;
                    self.armed_type = None;
                    self.area.redraw(cx);
                }
                _ => {}
            },
            _ => {}
        }
    }
}
