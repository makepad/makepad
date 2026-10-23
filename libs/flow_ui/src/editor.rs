//! An application's flow editor. The host supplies immutable observations;
//! this widget never creates an engine, starts an instance or owns a device.

use makepad_flow::Graph;
use makepad_flowgraph::{CanvasEdit, FaceViewport, FlowCanvas, FlowCanvasAction, GraphView, NodeFaces, NodeFacesScope, NodeStyle, Selection, WirePainter, EdgeView, CanvasViewport, Camera, CanvasPick, CanvasNodeRegion, CanvasEventPolicy};
use makepad_widgets::*;
use makepad_widgets::makepad_platform::event::{MouseButton, MouseLeaveEvent, TouchState, TweakRayEvent};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::{cell::RefCell, rc::Rc};
use crate::node_picker::{FlowNodePicker, NodeChoice, NodePickerAction};

#[derive(Clone, Debug)]
pub struct DebugInstance {
    pub id: String,
    pub label: String,
    pub revision: u64,
    pub status: String,
    pub graph: Option<Arc<Graph>>,
}

/// An edit is a request to the application, qualified by instance, accepted
/// revision and nested occurrence. Only the application can admit/save it.
#[derive(Clone, Debug, Default)]
pub enum FlowEditorAction {
    #[default]
    None,
    Edit { instance: String, revision: u64, path: Vec<String>, edit: CanvasEdit },
    AddNode { instance: String, revision: u64, path: Vec<String>, type_name: String,
        at: (f64, f64), from: Option<(String, String)> },
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let Box = RoundedView{
        width: Fill height: Fill flow: Down spacing: 6 padding: 8
        draw_bg +: { color: theme.color_surface_container border_radius: 8.0 }
    }
    mod.widgets.FlowEditor = set_type_default() do #(FlowEditor::register_widget(vm)){
        width: Fill height: Fill flow: Down spacing: 6 padding: 6
        View{width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
            back := ButtonFlatter{text: "←"}
            location := Label{width: Fill text: "Flows"}
            add_node := ButtonFlatter{text: "+ Node"}
            fit := ButtonFlatter{text: "Fit"}
        }
        split := Splitter{
            axis: Horizontal align: FromA(160) min_vertical: 100 max_vertical: 120
                a: Splitter{
                axis: Vertical align: Weighted(0.6) min_horizontal: 70 max_horizontal: 60
                a: Box{
                    View{width: Fill height: Fit flow: Right spacing: 4
                        show_flows := ButtonFlatter{text: "Flows"}
                        show_nodes := ButtonFlatter{text: "Nodes"}
                    }
                    picker := FlowNodePicker{visible: false}
                    tree_panel := ScrollXYView{width: Fill height: Fill
                        tree := TreeView{
                            width: Fit height: Fit start_open: false
                            row_height: 22 indent: 14 spacing: 4
                            draw_text +: {color: theme.color_on_surface text_style: theme.font_regular{font_size: 10}}
                            draw_text_selected +: {text_style: theme.font_regular{font_size: 10}}
                            draw_fold +: {
                                color: theme.color_on_surface_variant
                                color_hover: theme.color_primary
                                // A solid disclosure triangle, right when
                                // closed and down when open. Its own hit
                                // zone belongs to TreeView, beside the name.
                                pixel: fn() {
                                    let p = (self.pos - vec2(0.5)) * self.rect_size
                                    let q = p.mix(vec2(p.y, -p.x), self.open)
                                    let arm = min(self.rect_size.x, self.rect_size.y) * 0.26
                                    let distance = max(-q.x - arm, (abs(q.y) + q.x * 0.5 - arm * 0.5) / 1.118)
                                    let covered = 1.0 - smoothstep(-0.6, 0.6, distance)
                                    return Pal.premul(self.color.mix(self.color_hover, self.hover)) * covered
                                }
                            }
                        }
                    }
                }
                b: Box{
                    Label{text: "PROPERTIES"}
                    ScrollYView{width: Fill height: Fill flow: Down spacing: 6
                        edit_notice := Label{width: Fill height: Fit visible: false
                            draw_text +: {color: theme.color_error}}
                        selection_preview := View{width: Fill height: Fit flow: Down visible: false}
                        properties := Label{width: Fill height: Fit text: "Select a node or wire."}
                    }
                }
            }
            b: RoundedView{
                width: Fill height: Fill flow: Down
                draw_bg +: {color: theme.color_surface_container_lowest border_radius: 8.0}
                canvas := FlowCanvas{
                    width: Fill height: Fill
                    // The canvas defaults copied its fallback aliases when
                    // flowgraph registered. Bind every paint here to the
                    // current theme, including the real translucent shadow.
                    // Component navigation reuses this same styled canvas.
                    draw_card +: {shadow_color: theme.color_elevation_1}
                    draw_title +: {color: theme.color_on_surface}
                    draw_meta +: {color: theme.color_on_surface_variant}
                    draw_port +: {color: theme.color_on_surface_variant}
                    draw_chip +: {color: theme.color_on_surface}
                    draw_error +: {color: theme.color_error}
                    card_color: theme.color_surface_container
                    card_color_hover: theme.color_surface_container_high
                    card_edge_color: theme.color_outline_variant
                    accent_color: theme.color_primary
                    highlight_color: theme.color_primary
                    color_input: theme.color_success
                    color_output: theme.color_success
                    color_chat: theme.color_info
                    color_gen: theme.color_warning
                    color_fn: theme.color_primary
                    color_http: theme.color_info
                    color_ask: theme.color_warning
                    color_flow: theme.color_on_surface_variant
                    color_port_text: theme.color_on_surface
                    color_port_image: theme.color_warning
                    color_port_audio: theme.color_info
                    color_port_video: theme.color_primary
                    color_port_mesh: theme.color_success
                    color_port_json: theme.color_warning
                    color_port_list: theme.color_primary
                    color_port_bytes: theme.color_on_surface_variant
                    color_state_running: theme.color_info
                    color_state_done: theme.color_success
                    color_state_failed: theme.color_error
                    color_state_waiting: theme.color_warning
                    color_state_inactive: theme.color_text_disabled
                    color_state_idle: theme.color_text_disabled
                    color_port_label_connected: theme.color_on_surface
                    color_port_label_open: theme.color_on_surface_variant
                    icon_check +: {color: theme.color_success}
                    icon_alert +: {color: theme.color_error}
                    icon_clock +: {color: theme.color_warning}
                    draw_bg +: {
                        color_a: theme.color_surface_container_lowest
                        color_b: mix(theme.color_surface_container_lowest, theme.color_surface_container_high, 0.15)
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct TreeTarget {
    instance: String,
    path: Vec<String>,
    node: Option<String>,
}

struct DisplayedFlow {
    instance: String,
    revision: u64,
    path: Vec<String>,
    /// The accepted root identifies a replacement even at the same revision.
    root: Arc<Graph>,
    graph: Arc<Graph>,
}

/// Presentation belongs to the host, independently of the executable graph.
/// A full-bleed face owns its port labels; the canvas still owns every socket,
/// wire and hit target. Its drawing scope contains the current `FaceViewport`.
pub struct DebugNodeFace {
    pub node: String,
    pub widget: WidgetRef,
    pub full_bleed: bool,
}

/// Host-supplied faces and overlay controls on the UI thread. The application
/// owns frame bindings and supplies widgets; the editor never reads media.
#[derive(Default)]
struct DebugFaces {
    event_order: Vec<String>,
    capture: Option<OverlayCapture>,
    hovered: Option<String>,
    wire_painter: Option<Rc<RefCell<dyn WirePainter>>>,
    overlays: HashMap<String, WidgetRef>,
    widgets: HashMap<String, WidgetRef>,
    full_bleed: HashSet<String>,
}

#[derive(Clone, Copy)]
enum OverlayPointer { Mouse(MouseButton), Touch(u64) }

#[derive(Clone)]
struct OverlayCapture { node: String, pointer: OverlayPointer }

impl NodeFaces for DebugFaces {
    fn face_measurement_pending(&self, node: &str) -> bool {
        self.full_bleed.contains(node) && !self.widgets.contains_key(node)
    }

    fn draw_wire(&mut self, cx: &mut Cx2d, edge: &EdgeView, route: &makepad_flowgraph::wire_route::WireRoute, viewport: &CanvasViewport) -> bool {
        self.wire_painter.as_ref().is_some_and(|painter| {
            painter.try_borrow_mut().is_ok_and(|mut painter| painter.draw_wire(cx, edge, route, viewport))
        })
    }

    fn draw_node_overlay(&mut self, cx: &mut Cx2d, node: &str, viewport: &FaceViewport) {
        let Some(widget) = self.overlays.get(node) else { return };
        let card = viewport.geometry.card;
        let origin = viewport.canvas.camera.world_to_render((card.pos.x, card.pos.y));
        let walk = Walk { abs_pos: Some(origin), width: Size::Fixed(card.size.x), height: Size::Fixed(card.size.y), ..Walk::default() };
        let mut viewport = viewport.clone();
        widget.draw_walk_all(cx, &mut Scope::with_data(&mut viewport), walk);
    }

    fn draw_face(&mut self, cx: &mut Cx2d, node: &str, mut walk: Walk, card_sized: bool) {
        let Some(widget) = self.widgets.get(node) else { return };
        if !card_sized {
            walk.height = match widget.walk(cx).height { Size::Fixed(height) => Size::Fixed(height), _ => Size::Fixed(100.0) };
        }
        widget.draw_walk_all(cx, &mut Scope::empty(), walk);
    }

    fn draw_face_in_viewport(&mut self, cx: &mut Cx2d, node: &str, mut walk: Walk, card_sized: bool, viewport: &FaceViewport) {
        let Some(widget) = self.widgets.get(node) else { return };
        if !card_sized {
            walk.height = match widget.walk(cx).height { Size::Fixed(height) => Size::Fixed(height), _ => Size::Fixed(100.0) };
        }
        // Snapshot only this dispatch's geometry; neither the scope nor any
        // borrowed canvas state escapes into a host widget.
        if self.full_bleed.contains(node) {
            // A media face fills the port-sized card, including control inputs.
            walk.height = Size::Fixed(viewport.geometry.card.size.y);
        }
        let mut viewport = viewport.clone();
        widget.draw_walk_all(cx, &mut Scope::with_data(&mut viewport), walk);
    }

    fn set_z_order(&mut self, order: &[String]) {
        if self.event_order != order { self.event_order = order.to_vec(); }
    }

    fn set_popup_anchor_transform(&mut self, _cx: &mut Cx, _transform: Option<PopupAnchorTransform>) {}
}

impl DebugFaces {
    fn roots(&self) -> Vec<(LiveId, WidgetRef)> {
        // Both presentations belong to the same node. The canvas uses that
        // association to exclude controls hidden underneath a raised card.
        self.widgets.iter().chain(self.overlays.iter())
            .map(|(node, widget)| (LiveId::from_str(node), widget.clone())).collect()
    }

    fn handle_overlays(&mut self, cx: &mut Cx, event: &Event, canvas: &WidgetRef) -> CanvasEventPolicy {
        if matches!(event, Event::Draw(_)) || self.overlays.is_empty() { return CanvasEventPolicy::ALL; }
        let Some((viewport, front)) = canvas.borrow::<FlowCanvas>().and_then(|canvas| {
            let viewport = canvas.viewport()?;
            let point = match event {
                Event::MouseDown(e) => Some(e.abs), Event::MouseMove(e) => Some(e.abs),
                Event::MouseUp(e) => Some(e.abs), Event::Scroll(e) => Some(e.abs),
                Event::LongPress(e) => Some(e.abs), Event::TweakRay(e) => Some(e.abs),
                Event::TouchUpdate(e) => e.touches.iter().find(|touch| touch.state == TouchState::Start)
                    .or_else(|| e.touches.first()).map(|touch| touch.abs),
                _ => None,
            };
            let front = point.and_then(|point| {
                if !viewport.clip.contains(point) { return None; }
                match canvas.pick(point) {
                    Some(CanvasPick::Node {node, region: CanvasNodeRegion::Content | CanvasNodeRegion::Body}) => Some(node),
                    _ => None,
                }
            });
            Some((viewport, front))
        }) else { return CanvasEventPolicy::ALL; };
        let press = match event {
            Event::MouseDown(e) => Some(OverlayPointer::Mouse(e.button)),
            Event::TouchUpdate(e) => e.touches.iter().find(|touch| touch.state == TouchState::Start)
                .map(|touch| OverlayPointer::Touch(touch.uid)),
            _ => None,
        };
        let pointer = matches!(event, Event::MouseDown(_) | Event::MouseMove(_) | Event::MouseUp(_)
            | Event::MouseLeave(_) | Event::Scroll(_) | Event::LongPress(_) | Event::TouchUpdate(_) | Event::TweakRay(_));
        let release = matches!(event, Event::WindowLostFocus(_)) || self.capture.as_ref().is_some_and(|capture| {
            match (capture.pointer, event) {
                (OverlayPointer::Mouse(button), Event::MouseUp(e)) => e.button == button,
                (OverlayPointer::Touch(uid), Event::TouchUpdate(e)) => e.touches.iter()
                    .any(|touch| touch.uid == uid && touch.state == TouchState::Stop),
                _ => false,
            }
        });
        let had_capture = self.capture.is_some();
        let owner = self.capture.as_ref().map(|capture| capture.node.clone()).or(front);
        // The old topmost card must receive an exit even when the pointer is
        // now over blank canvas or another card. Sending another ordinary
        // move could let a covered control claim hover again.
        if let Event::MouseMove(e) = event {
            if self.hovered != owner {
                if let Some(widget) = self.hovered.as_ref().and_then(|node| self.overlays.get(node)) {
                    widget.handle_event(cx, &Event::MouseLeave(MouseLeaveEvent {
                        abs: viewport.camera.screen_to_local(e.abs), window_id: e.window_id,
                        modifiers: e.modifiers, time: e.time, handled: Default::default(),
                    }), &mut Scope::empty());
                }
                self.hovered = owner.clone();
            }
        }
        let owner = if matches!(event, Event::MouseLeave(_)) {
            self.hovered.take().or(owner)
        } else { owner };
        if pointer && owner.is_none() { return CanvasEventPolicy::ALL; }
        let remapped = remap_overlay_event(event, &viewport.camera);
        let delivered = remapped.as_ref().unwrap_or(event);
        for node in self.event_order.iter().rev() {
            if pointer && owner.as_ref() != Some(node) { continue; }
            if let Some(widget) = self.overlays.get(node) {
                widget.handle_event(cx, delivered, &mut Scope::empty());
            }
        }
        if let Some(remapped) = &remapped { sync_overlay_handled(event, remapped, &viewport.camera); }
        if !had_capture && press.is_some() && owner.as_ref().and_then(|node| self.overlays.get(node))
            .is_some_and(|root| overlay_has_capture(cx, root)) {
            self.capture = owner.map(|node| OverlayCapture { node, pointer: press.unwrap() });
        }
        let control_pointer = had_capture || self.capture.is_some() || matches!(delivered,
            Event::Scroll(e) if e.handled_x.get() || e.handled_y.get());
        if release { self.capture = None; }
        let focus = cx.key_focus();
        let control_focus = !focus.is_empty() && self.overlays.values().any(|root| overlay_owns_area(root, focus));
        CanvasEventPolicy { pointer: !control_pointer, navigation: !control_pointer, keyboard: !control_focus }
    }
}

fn overlay_has_capture(cx: &Cx, root: &WidgetRef) -> bool {
    if cx.fingers.is_area_captured(root.area()) { return true; }
    let mut found = false;
    root.children(&mut |_, child| { found |= overlay_has_capture(cx, &child); });
    found
}

fn overlay_owns_area(root: &WidgetRef, area: Area) -> bool {
    if root.area() == area { return true; }
    let mut found = false;
    root.children(&mut |_, child| { found |= overlay_owns_area(&child, area); });
    found
}

#[derive(Script, ScriptHook, Widget)]
pub struct FlowEditor {
    #[deref] view: View,
    #[rust] instances: Vec<DebugInstance>,
    #[rust] selected: Option<String>,
    #[rust] path: Vec<String>,
    #[rust] targets: HashMap<LiveId, TreeTarget>,
    #[rust] displayed: Option<DisplayedFlow>,
    #[rust] faces: DebugFaces,
    #[rust] streaming: HashSet<String>,
    #[rust] preview_selection: Option<Selection>,
    #[rust] node_type_styles: HashMap<String, NodeStyle>,
    #[rust] face_view: Option<(String, Vec<String>)>,
    #[rust] has_fitted_faces: bool,
    #[rust] fit_pending: bool,
    #[rust] fit_sample: Option<[f64; 6]>,
    #[rust] fit_passes: u8,
    #[rust] fit_next: NextFrame,
    /// Only host-admitted layout edits, overlaid until the host's edit lane
    /// settles. Accepted observations remain immutable and authoritative.
    #[rust] pending_layout: HashMap<String, LayoutPreview>,
    #[rust] insertion: Option<NodeInsertion>,
}

struct NodeInsertion {
    key: (String, u64, Vec<String>),
    at: (f64, f64),
    from: Option<(String, String)>,
}

#[derive(Default)]
struct LayoutPreview {
    at: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
}

fn graph_at<'a>(root: &'a Graph, path: &[String]) -> Option<&'a Graph> {
    let mut graph = root;
    for occurrence in path {
        let node = graph.nodes.iter().find(|node| &node.id == occurrence && node.kind == "component")?;
        graph = &root.components.iter().find(|component| component.alias == node.type_name)?.graph;
    }
    Some(graph)
}

fn tree_id(instance: &str, path: &[String], node: Option<&str>) -> LiveId {
    LiveId::from_str(&format!("{instance}\0{}\0{}", path.join("/"), node.unwrap_or_default()))
}

impl FlowEditor {
    pub fn set_node_choices(&mut self, cx: &mut Cx, choices: Vec<NodeChoice>) {
        let widget = self.view.widget(cx, ids!(picker));
        if let Some(mut picker) = widget.borrow_mut::<FlowNodePicker>() {
            picker.set_choices(cx, choices);
        };
    }

    fn show_nodes(&mut self, cx: &mut Cx, insertion: Option<NodeInsertion>, ty: Option<makepad_flow::PortType>) {
        self.insertion = insertion;
        self.view.widget(cx, ids!(tree_panel)).set_visible(cx, false);
        let widget = self.view.widget(cx, ids!(picker));
        widget.set_visible(cx, true);
        if let Some(mut picker) = widget.borrow_mut::<FlowNodePicker>() {
            picker.set_source_type(cx, ty);
        };
        self.view.redraw(cx);
    }

    fn insert_choice(&mut self, cx: &mut Cx, type_name: String) {
        let widget = self.view.widget(cx, ids!(canvas));
        let at = if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
            canvas.armed_type = None;
            let camera = canvas.camera();
            let center = camera.screen_to_world(camera.view.pos + camera.view.size * 0.5);
            Some((center.0 - 80.0, center.1 - 30.0))
        } else { None };
        let Some(key) = self.view_key() else { return; };
        let (at, from) = if let Some(insertion) = self.insertion.take() {
            if insertion.key != key { return; }
            (insertion.at, insertion.from)
        } else {
            let Some(at) = at else { return; };
            (at, None)
        };
        cx.widget_action(self.widget_uid(), FlowEditorAction::AddNode {
            instance: key.0, revision: key.1, path: key.2, type_name, at, from,
        });
        self.show_nodes(cx, None, None);
    }

    /// Host-owned appearance by declared node type. This changes only the
    /// canvas projection's style key; graph ids, types and execution stay exact.
    pub fn set_node_type_styles(&mut self, cx: &mut Cx, styles: HashMap<String, NodeStyle>) {
        let keys_changed = self.node_type_styles.len() != styles.len()
            || styles.keys().any(|key| !self.node_type_styles.contains_key(key));
        self.node_type_styles = styles;
        let styles = self.node_type_styles.iter().map(|(name, style)| {
            (format!("type:{name}"), style.clone())
        }).collect();
        let projected = if keys_changed {
            self.displayed.as_ref().map(|displayed| self.project_graph(&displayed.graph))
        } else { None };
        let widget = self.view.widget(cx, ids!(canvas));
        if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
            canvas.set_node_styles(cx, styles);
            if let Some(projected) = projected { canvas.set_graph(cx, Some(projected)); }
        };
    }

    fn project_graph(&self, graph: &Graph) -> GraphView {
        let mut projected = crate::graph_view::view_of(graph);
        for node in &mut projected.nodes {
            if self.node_type_styles.contains_key(&node.type_name) {
                node.kind = format!("type:{}", node.type_name);
            }
            if self.faces.full_bleed.contains(&node.id) { node.full_bleed = true; }
            if let Some(layout) = self.pending_layout.get(&node.id) {
                if let Some(at) = layout.at { node.at = at; }
                if let Some(size) = layout.size { node.size = Some(size); }
            }
        }
        projected
    }

    /// Complete synchronous host admission of an edit. A refused gesture is
    /// rolled back without removing an earlier admitted edit still in flight.
    pub fn resolve_layout_edit(&mut self, cx: &mut Cx, instance: &str, path: &[String], edit: &CanvasEdit, admitted: bool) {
        if self.displayed.as_ref().is_none_or(|view| view.instance != instance || view.path != path) { return; }
        if admitted {
            match edit {
                CanvasEdit::Move { node, at } => self.pending_layout.entry(node.clone()).or_default().at = Some(*at),
                CanvasEdit::Resize { node, size } => self.pending_layout.entry(node.clone()).or_default().size = Some(*size),
                _ => {}
            }
        }
        self.restore_layout(cx);
    }

    /// Call after installing the latest accepted observations, once this
    /// instance's edit lane is idle, whether it succeeded or was refused.
    pub fn settle_layout_edits(&mut self, cx: &mut Cx, instance: &str) {
        if self.displayed.as_ref().is_none_or(|view| view.instance != instance) || self.pending_layout.is_empty() { return; }
        self.pending_layout.clear();
        self.restore_layout(cx);
    }

    fn restore_layout(&mut self, cx: &mut Cx) {
        let Some(displayed) = self.displayed.as_ref() else { return; };
        let projected = self.project_graph(&displayed.graph);
        let widget = self.view.widget(cx, ids!(canvas));
        if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
            for node in projected.nodes {
                canvas.set_node_geometry(cx, &node.id, node.at, node.size);
            }
        };
    }

    /// Identity of the graph currently drawn, including its nested occurrence.
    pub fn view_key(&self) -> Option<(String, u64, Vec<String>)> {
        let displayed = self.displayed.as_ref()?;
        Some((displayed.instance.clone(), displayed.revision, displayed.path.clone()))
    }

    /// The selected subflow, retained so a host can cheaply detect replacements.
    pub fn displayed_graph(&self) -> Option<Arc<Graph>> {
        self.displayed.as_ref().map(|displayed| displayed.graph.clone())
    }

    fn admits_presentation(&self, instance: &str, revision: u64, path: &[String], graph: &Arc<Graph>) -> bool {
        self.displayed.as_ref().is_some_and(|displayed| displayed.instance == instance
            && displayed.revision == revision && displayed.path == path && Arc::ptr_eq(&displayed.graph, graph))
    }

    /// The host owns media; the editor borrows its renderer on this UI thread.
    /// Replacements, including same-revision graph replacements, retire it.
    pub fn set_wire_painter(&mut self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], graph: &Arc<Graph>, painter: Option<Rc<RefCell<dyn WirePainter>>>) -> bool {
        if !self.admits_presentation(instance, revision, path, graph) { return false; }
        self.faces.wire_painter = painter;
        self.view.widget(cx, ids!(canvas)).redraw(cx);
        true
    }

    /// Host presentation and controls. They never create executable ports,
    /// edges or card measurements, and cannot survive their graph instance.
    pub fn set_node_overlays(&mut self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], graph: &Arc<Graph>, overlays: Vec<(String, WidgetRef)>) -> bool {
        if !self.admits_presentation(instance, revision, path, graph)
            || overlays.iter().any(|(id, _)| !graph.nodes.iter().any(|node| node.id == *id)) { return false; }
        let same = self.faces.overlays.len() == overlays.len() && overlays.iter().all(|(id, widget)| {
            self.faces.overlays.get(id).is_some_and(|old| old.widget_uid() == widget.widget_uid())
        });
        if !same {
            let survives = |node: &str| self.faces.overlays.get(node).is_some_and(|old| overlays.iter()
                .any(|(id, widget)| id == node && old.widget_uid() == widget.widget_uid()));
            if self.faces.capture.as_ref().is_some_and(|capture| !survives(&capture.node)) { self.faces.capture = None; }
            if self.faces.hovered.as_ref().is_some_and(|node| !survives(node)) { self.faces.hovered = None; }
            self.faces.overlays = overlays.into_iter().collect();
            let widget = self.view.widget(cx, ids!(canvas));
            if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
                canvas.set_face_roots(cx, self.faces.roots());
            };
        }
        self.view.widget(cx, ids!(canvas)).redraw(cx);
        true
    }

    /// Install passive widgets by local node id, only for this exact view.
    /// Call with an empty list to unmount previews before retiring their source.
    pub fn set_node_faces(&mut self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], faces: Vec<(String, WidgetRef)>) -> bool {
        let faces = faces.into_iter().map(|(node, widget)| DebugNodeFace {node, widget, full_bleed: false}).collect();
        self.set_presented_node_faces(cx, instance, revision, path, faces)
    }

    /// Install host-selected presentation with the same exact-view admission
    /// and retirement ordering as ordinary faces. This never changes a graph
    /// document, port positions or the camera.
    pub fn set_presented_node_faces(&mut self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], faces: Vec<DebugNodeFace>) -> bool {
        let Some(displayed) = self.displayed.as_ref() else { return false };
        if displayed.instance != instance || displayed.revision != revision || displayed.path != path
            || faces.iter().any(|face| !displayed.graph.nodes.iter().any(|node| node.id == face.node)) {
            return false;
        }
        let full_bleed: HashSet<_> = faces.iter().filter(|face| face.full_bleed).map(|face| face.node.clone()).collect();
        let presentation_changed = self.faces.full_bleed != full_bleed;
        let next: HashMap<_, _> = faces.into_iter().map(|face| (face.node, face.widget)).collect();
        if !presentation_changed && self.faces.widgets.len() == next.len() && next.iter().all(|(id, widget)| {
            self.faces.widgets.get(id).is_some_and(|old| old.widget_uid() == widget.widget_uid())
        }) { return true; }
        let roots = next.iter().chain(self.faces.overlays.iter()).map(|(id, widget)| (LiveId::from_str(id), widget.clone())).collect();
        let widget = self.view.widget(cx, ids!(canvas));
        let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() else { return false };
        // Remove the old canvas references while the previous host still owns
        // its widgets, before dropping any old bound face on this UI thread.
        canvas.set_face_roots(cx, roots);
        self.faces.widgets = next;
        self.faces.full_bleed = full_bleed;
        if presentation_changed {
            canvas.set_graph(cx, Some(self.project_graph(&displayed.graph)));
        }
        drop(canvas);
        // Revision/theme remounts replace widgets without changing the view
        // the user is navigating. Only its first real previews need a fit.
        if !self.faces.widgets.is_empty() && !self.has_fitted_faces {
            self.has_fitted_faces = true;
            self.request_fit(cx);
        }
        true
    }

    /// Continuous traffic observed by the host. Unchanged samples do no work.
    pub fn set_node_streaming(&mut self, cx: &mut Cx, node: &str, on: bool) {
        let Some(displayed) = self.displayed.as_ref() else { return };
        if !displayed.graph.nodes.iter().any(|candidate| candidate.id == node)
            || self.streaming.contains(node) == on { return; }
        let widget = self.view.widget(cx, ids!(canvas));
        if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
            canvas.set_streaming(cx, node, on);
            if on { self.streaming.insert(node.to_string()); }
            else { self.streaming.remove(node); }
        };
    }

    pub fn set_instances(&mut self, cx: &mut Cx, instances: Vec<DebugInstance>) {
        let changed = self.instances.len() != instances.len() || self.instances.iter().zip(&instances).any(|(a,b)| {
            a.id != b.id || a.revision != b.revision || a.label != b.label || a.status != b.status || match (&a.graph, &b.graph) {
                (Some(a), Some(b)) => !Arc::ptr_eq(a,b), (None,None) => false, _ => true,
            }
        });
        if !changed { return; }
        let selected_before = self.selected.clone();
        let old_revision = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref()).map(|row| row.revision);
        let old_graph = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref()).and_then(|row| row.graph.clone());
        let added: Vec<_> = instances.iter().filter(|row| !self.instances.iter().any(|old| old.id == row.id)).map(|row| row.id.clone()).collect();
        self.instances = instances;
        if !self.instances.iter().any(|row| Some(&row.id) == self.selected.as_ref()) {
            self.selected = self.instances.first().map(|row| row.id.clone());
            self.path.clear();
        }
        self.rebuild_tree(cx);
        let tree = self.view.tree_view(cx, ids!(tree));
        for id in added { tree.set_open(cx, tree_id(&id, &[], None), true); }
        let new_graph = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref()).and_then(|row| row.graph.clone());
        let graph_changed = match (&old_graph, &new_graph) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a,b), (None,None) => false, _ => true,
        };
        let new_revision = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref()).map(|row| row.revision);
        if selected_before != self.selected || graph_changed || old_revision != new_revision {
            self.show_graph(cx, selected_before != self.selected || old_graph.is_none());
        } else {
            self.update_location(cx);
            let widget = self.view.widget(cx, ids!(canvas));
            let selection = widget.borrow::<FlowCanvas>().and_then(|canvas| canvas.selection().cloned());
            self.describe(cx, selection.as_ref());
        }
        self.view.redraw(cx);
    }

    /// Show one listed instance's root graph, as choosing it in the tree
    /// does: how a host hands the editor an instance chosen elsewhere.
    /// `false` when the instance is not listed.
    pub fn show_instance(&mut self, cx: &mut Cx, instance: &str) -> bool {
        if !self.instances.iter().any(|row| row.id == instance) {
            return false;
        }
        if self.selected.as_deref() != Some(instance) || !self.path.is_empty() {
            self.selected = Some(instance.to_string());
            self.path.clear();
            self.show_graph(cx, true);
            self.view.redraw(cx);
        }
        true
    }

    fn show_graph(&mut self, cx: &mut Cx, fit: bool) {
        self.clear_selection_preview(cx);
        let row = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref());
        let graph = row.and_then(|row| row.graph.as_deref());
        while !self.path.is_empty() && graph.and_then(|root| graph_at(root, &self.path)).is_none() { self.path.pop(); }
        let logical_view = row.map(|row| (row.id.clone(), self.path.clone()));
        let logical_view_changed = logical_view.is_some() && self.face_view != logical_view;
        if logical_view_changed {
            self.pending_layout.clear();
            self.face_view = logical_view;
            self.has_fitted_faces = false;
        }
        let changed = match (self.displayed.as_ref(), row.filter(|row| row.graph.is_some())) {
            (Some(old), Some(row)) => old.instance != row.id || old.revision != row.revision
                || old.path != self.path || !Arc::ptr_eq(&old.root, row.graph.as_ref().unwrap()),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.insertion = None;
            let picker = self.view.widget(cx, ids!(picker));
            if let Some(mut picker) = picker.borrow_mut::<FlowNodePicker>() {
                picker.set_source_type(cx, None);
            };
            if logical_view_changed || graph.is_none() {
                self.pending_layout.clear();
                self.fit_pending = false;
                self.fit_sample = None;
            }
            let next = row.and_then(|row| {
                let root = row.graph.clone()?;
                let graph = if self.path.is_empty() { root.clone() }
                    else { Arc::new(graph_at(&root, &self.path)?.clone()) };
                Some(DisplayedFlow {instance: row.id.clone(), revision: row.revision, path: self.path.clone(), root, graph})
            });
            // Retiring media is not a change to card geometry. A revision can
            // be visible before its execution owner accepts the new sources;
            // dropping full-bleed here briefly changes ports and wire paths.
            // Preserve presentation only for the same occurrence and node type.
            if logical_view_changed || next.is_none() {
                self.faces.full_bleed.clear();
            } else {
                self.faces.full_bleed.retain(|id| {
                    let before = self.displayed.as_ref().and_then(|old| old.graph.nodes.iter().find(|node| node.id == *id));
                    let after = next.as_ref().and_then(|next| next.graph.nodes.iter().find(|node| node.id == *id));
                    matches!((before, after), (Some(a), Some(b)) if a.type_name == b.type_name)
                });
            }
            let projected = next.as_ref().map(|displayed| self.project_graph(&displayed.graph));
            let compatible = next.as_ref().map(|displayed| crate::graph_view::compatibility_of(&displayed.graph)).unwrap_or_default();
            let widget = self.view.widget(cx, ids!(canvas));
            if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
                canvas.set_face_roots(cx, Vec::new());
                canvas.armed_type = None;
                self.faces.wire_painter = None;
                self.faces.overlays.clear();
                self.faces.capture = None;
                self.faces.hovered = None;
                canvas.clear_run(cx);
                self.faces.widgets.clear();
                self.streaming.clear();
                self.displayed = next;
                let _ = canvas.set_zoom_range(cx, 0.05, 6.0);
                canvas.set_graph(cx, projected);
                canvas.set_compatible_ports(compatible);
            };
            if fit { self.request_fit(cx); }
        }
        self.update_location(cx);
        self.view.button(cx, ids!(back)).set_disabled(cx, self.path.is_empty());
        let widget = self.view.widget(cx, ids!(canvas));
        let selection = widget.borrow::<FlowCanvas>().and_then(|canvas| canvas.selection().cloned());
        self.describe(cx, selection.as_ref());
        self.view.redraw(cx);
    }

    fn request_fit(&mut self, cx: &mut Cx) {
        self.fit_pending = true;
        self.fit_sample = None;
        self.fit_passes = 0;
        self.view.redraw(cx);
    }

    /// A hidden editor has no usable layout. Fit only after actual canvas
    /// drawing and face measurement agree; at most four valid drawing passes
    /// are requested, with no background polling while the view is hidden.
    fn fit_after_layout(&mut self, cx: &mut Cx) {
        if !self.fit_pending { return; }
        let widget = self.view.widget(cx, ids!(canvas));
        let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() else { return };
        let Some(viewport) = canvas.viewport() else { return };
        if viewport.clip.size.x <= 1.0 || viewport.clip.size.y <= 1.0
            || viewport.camera.view.size.x <= 1.0 || viewport.camera.view.size.y <= 1.0 { return; }
        let Some(bounds) = canvas.graph_bounds() else {
            self.fit_pending = false;
            return;
        };
        let sample = [viewport.camera.view.size.x, viewport.camera.view.size.y,
            bounds.pos.x, bounds.pos.y, bounds.size.x, bounds.size.y];
        self.fit_passes += 1;
        if self.fit_sample == Some(sample) || self.fit_passes >= 4 {
            self.fit_pending = false;
            self.fit_sample = None;
            canvas.fit(cx);
        } else {
            self.fit_sample = Some(sample);
            self.fit_next = cx.new_next_frame();
        }
    }

    fn update_location(&mut self, cx: &mut Cx) {
        let label = self.flow_location();
        self.view.label(cx, ids!(location)).set_text(cx, &label);
    }

    fn flow_location(&self) -> String {
        let Some(row) = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref()) else { return "Flows".into() };
        let mut label = format!("{} · flow", row.label);
        if let Some(root) = row.graph.as_deref() {
            let mut graph = root;
            for occurrence in &self.path {
                let Some(node) = graph.nodes.iter().find(|node| &node.id == occurrence && node.kind == "component") else { break };
                label.push_str(&format!(" / {} · {}", node.id, node.type_name));
                let Some(component) = root.components.iter().find(|component| component.alias == node.type_name) else { break };
                graph = &component.graph;
            }
        }
        label
    }

    fn describe(&mut self, cx: &mut Cx, selection: Option<&Selection>) {
        if self.preview_selection.as_ref() != selection { self.clear_selection_preview(cx); }
        let row = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref());
        let graph = row.and_then(|row| row.graph.as_deref()).and_then(|root| graph_at(root, &self.path));
        let text = match (row, graph, selection) {
            (_, Some(graph), Some(Selection::Node(id))) => graph.nodes.iter().find(|node| &node.id == id).map(|node| {
                let mut text = format!("{}\n{}\n\nAuthored settings", node.id, node.type_name);
                for (name, value) in &node.params { text.push_str(&format!("\n{name}: {value:?}")); }
                text
            }).unwrap_or_default(),
            (_, _, Some(Selection::Edge{from_node, from_port, to_node, to_port, ..})) => format!("{from_node}.{from_port}\n→ {to_node}.{to_port}"),
            (Some(row), Some(graph), _) => format!("{}\n{} nodes · {} wires\nRevision {}\n{}", self.flow_location(), graph.nodes.len(), graph.edges.len(), row.revision, row.status),
            (Some(row), _, _) => format!("{}\nRevision {}\n{}", self.flow_location(), row.revision, row.status),
            _ => "No running flows".into(),
        };
        self.view.label(cx, ids!(properties)).set_text(cx, &text);
    }

    /// A host's passive observation of exactly the node still selected in
    /// this accepted view. Observations never mutate the executable graph.
    pub fn set_node_description(&mut self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], node: &str, text: &str) -> bool {
        let Some(displayed) = self.displayed.as_ref() else { return false };
        if displayed.instance != instance || displayed.revision != revision || displayed.path != path { return false }
        let widget = self.view.widget(cx, ids!(canvas));
        let selected = widget.borrow::<FlowCanvas>().is_some_and(|canvas| {
            matches!(canvas.selection(), Some(Selection::Node(selected)) if selected == node)
        });
        if !selected { return false }
        self.view.label(cx, ids!(properties)).set_text(cx, text);
        true
    }

    /// A host-owned passive probe for the exact currently selected node/wire.
    /// The editor does not interpret its data or mutate the running graph.
    pub fn set_selection_preview(&mut self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], selection: &Selection, preview: WidgetRef) -> bool {
        let Some(displayed) = self.displayed.as_ref() else { return false };
        if displayed.instance != instance || displayed.revision != revision || displayed.path != path { return false }
        let canvas = self.view.widget(cx, ids!(canvas));
        if !canvas.borrow::<FlowCanvas>().is_some_and(|canvas| canvas.selection() == Some(selection)) { return false }
        let target = self.view.view(cx, ids!(selection_preview));
        if let Some(mut view) = target.borrow_mut() {
            if view.children.first().is_none_or(|(_, child)| child.widget_uid() != preview.widget_uid()) {
                view.children.clear();
                view.children.push((live_id!(probe), preview));
                view.redraw(cx);
            }
        }
        target.set_visible(cx, true);
        self.preview_selection = Some(selection.clone());
        true
    }

    pub fn clear_selection_preview(&mut self, cx: &mut Cx) {
        self.preview_selection = None;
        let target = self.view.view(cx, ids!(selection_preview));
        if let Some(mut view) = target.borrow_mut() { view.children.clear(); }
        target.set_visible(cx, false);
    }

    fn open(&mut self, cx: &mut Cx, node: &str) {
        let is_component = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref())
            .and_then(|row| row.graph.as_deref()).and_then(|root| graph_at(root, &self.path))
            .is_some_and(|graph| graph.nodes.iter().any(|item| item.id == node && item.kind == "component"));
        if is_component { self.path.push(node.into()); self.show_graph(cx, true); }
    }

    fn node_items(root: &Graph, graph: &Graph, instance: &str, path: &mut Vec<String>, targets: &mut HashMap<LiveId, TreeTarget>) -> Vec<TreeItem> {
        let mut items = Vec::new();
        for node in &graph.nodes {
            if node.kind == "component" {
                if path.len() >= makepad_flow::document::closure::MAX_COMPONENT_DEPTH { continue; }
                let Some(component) = root.components.iter().find(|component| component.alias == node.type_name) else { continue };
                path.push(node.id.clone());
                let id = tree_id(instance, path, None);
                targets.insert(id, TreeTarget{instance: instance.into(), path: path.clone(), node: None});
                let children = Self::node_items(root, &component.graph, instance, path, targets);
                items.push(TreeItem::new(id, &format!("{} · {}", node.id, node.type_name)).with_children(children));
                path.pop();
            } else {
                let id = tree_id(instance, path, Some(&node.id));
                targets.insert(id, TreeTarget{instance: instance.into(), path: path.clone(), node: Some(node.id.clone())});
                items.push(TreeItem::new(id, &format!("{} · {}", node.id, node.type_name)));
            }
        }
        items
    }

    fn rebuild_tree(&mut self, cx: &mut Cx) {
        self.targets.clear();
        let mut items = Vec::new();
        for row in &self.instances {
            let id = tree_id(&row.id, &[], None);
            self.targets.insert(id, TreeTarget{instance: row.id.clone(), path: Vec::new(), node: None});
            let children = row.graph.as_deref().map(|graph| Self::node_items(graph, graph, &row.id, &mut Vec::new(), &mut self.targets)).unwrap_or_default();
            items.push(TreeItem::new(id, &format!("{} · flow", row.label)).with_children(children));
        }
        self.view.tree_view(cx, ids!(tree)).set_items(cx, &items);
    }
}

impl Widget for FlowEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.fit_pending && self.fit_next.is_event(event).is_some() { self.view.redraw(cx); }
        // A pending initial fit must not replace a camera the user is already
        // manipulating. An explicit navigation/fit action below re-arms it.
        if self.fit_pending {
            let widget = self.view.widget(cx, ids!(canvas));
            let viewport = widget.borrow::<FlowCanvas>().and_then(|canvas| canvas.viewport());
            if let Some(viewport) = viewport {
                let inside = match event {
                    Event::MouseDown(event) => viewport.clip.contains(event.abs),
                    Event::Scroll(event) => viewport.clip.contains(event.abs),
                    Event::TouchUpdate(event) => event.touches.iter().any(|touch| viewport.clip.contains(touch.abs)),
                    _ => false,
                };
                if inside { self.fit_pending = false; }
            }
        }
        let canvas = self.view.widget(cx, ids!(canvas));
        let policy = self.faces.handle_overlays(cx, event, &canvas);
        let mut faces = NodeFacesScope::new(&mut self.faces).with_canvas_policy(canvas.widget_uid(), policy);
        self.view.handle_event(cx, event, &mut Scope::with_data(&mut faces));
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(add_node)).clicked(actions)
                || self.view.button(cx, ids!(show_nodes)).clicked(actions) {
                self.show_nodes(cx, None, None);
            }
            if self.view.button(cx, ids!(show_flows)).clicked(actions) {
                self.insertion = None;
                self.view.widget(cx, ids!(picker)).set_visible(cx, false);
                self.view.widget(cx, ids!(tree_panel)).set_visible(cx, true);
                self.view.redraw(cx);
            }
            let picker_uid = self.view.widget(cx, ids!(picker)).widget_uid();
            for action in actions.filter_widget_actions_cast::<NodePickerAction>(picker_uid) {
                match action {
                    NodePickerAction::Arm(type_name) if self.view_key().is_some() => {
                        let widget = self.view.widget(cx, ids!(canvas));
                        if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
                            canvas.armed_type = Some(type_name);
                        };
                    }
                    NodePickerAction::Choose(type_name) => self.insert_choice(cx, type_name),
                    _ => {}
                }
            }
            if self.view.button(cx, ids!(back)).clicked(actions) && self.path.pop().is_some() { self.show_graph(cx, true); }
            if self.view.button(cx, ids!(fit)).clicked(actions) {
                self.request_fit(cx);
            }
            let tree = self.view.tree_view(cx, ids!(tree));
            if let Some(target) = tree.chosen(actions).and_then(|id| self.targets.get(&id).cloned()) {
                self.selected = Some(target.instance); self.path = target.path;
                self.show_graph(cx, true);
                if let Some(node) = target.node {
                    let selection = Selection::Node(node);
                    let widget = self.view.widget(cx, ids!(canvas));
                    if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() { canvas.set_selection(cx, Some(selection.clone())); }
                    self.describe(cx, Some(&selection));
                }
            }
            let uid = self.view.widget(cx, ids!(canvas)).widget_uid();
            for action in actions.filter_widget_actions_cast::<FlowCanvasAction>(uid) {
                match action {
                    FlowCanvasAction::Open{node} => self.open(cx, &node),
                    FlowCanvasAction::Select(selection) => self.describe(cx, selection.as_ref()),
                    FlowCanvasAction::OpenPalette { at, from_node, from_port, ty } => {
                        if let Some(key) = self.view_key() {
                            if let Some(ty) = makepad_flow::PortType::from_str(&ty) {
                                self.show_nodes(cx, Some(NodeInsertion { key, at,
                                    from: Some((from_node, from_port)) }), Some(ty));
                            }
                        }
                    }
                    FlowCanvasAction::Edit(edit) => {
                        if let Some(row) = self.instances.iter().find(|row| Some(&row.id) == self.selected.as_ref()) {
                            cx.widget_action(self.widget_uid(), FlowEditorAction::Edit{instance: row.id.clone(), revision: row.revision, path: self.path.clone(), edit});
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // This adapter never escapes this dispatch. The face host is separate
        // from the view/canvas being mutably drawn through it.
        let mut faces = NodeFacesScope::new(&mut self.faces);
        let step = self.view.draw_walk(cx, &mut Scope::with_data(&mut faces), walk);
        if step.is_done() { self.fit_after_layout(cx); }
        step
    }
}

impl FlowEditorRef {
    pub fn set_node_choices(&self, cx: &mut Cx, choices: Vec<NodeChoice>) {
        if let Some(mut editor) = self.borrow_mut() { editor.set_node_choices(cx, choices); }
    }

    /// A host's edit result stays visible independently of node selection.
    pub fn set_edit_notice(&self, cx: &mut Cx, text: Option<&str>) {
        if let Some(editor) = self.borrow() {
            let notice = editor.view.label(cx, ids!(edit_notice));
            let text = text.unwrap_or("");
            if notice.text() != text { notice.set_text(cx, text); }
            notice.set_visible(cx, !text.is_empty());
        }
    }

    pub fn set_selection_preview(&self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], selection: &Selection, preview: WidgetRef) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.set_selection_preview(cx, instance, revision, path, selection, preview))
    }

    pub fn clear_selection_preview(&self, cx: &mut Cx) {
        if let Some(mut editor) = self.borrow_mut() { editor.clear_selection_preview(cx); }
    }

    pub fn set_node_description(&self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], node: &str, text: &str) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.set_node_description(cx, instance, revision, path, node, text))
    }

    pub fn resolve_layout_edit(&self, cx: &mut Cx, instance: &str, path: &[String], edit: &CanvasEdit, admitted: bool) {
        if let Some(mut editor) = self.borrow_mut() { editor.resolve_layout_edit(cx, instance, path, edit, admitted); }
    }

    pub fn settle_layout_edits(&self, cx: &mut Cx, instance: &str) {
        if let Some(mut editor) = self.borrow_mut() { editor.settle_layout_edits(cx, instance); }
    }

    pub fn set_node_type_styles(&self, cx: &mut Cx, styles: HashMap<String, NodeStyle>) {
        if let Some(mut editor) = self.borrow_mut() { editor.set_node_type_styles(cx, styles); }
    }

    pub fn view_key(&self) -> Option<(String, u64, Vec<String>)> {
        self.borrow().and_then(|editor| editor.view_key())
    }

    pub fn displayed_graph(&self) -> Option<Arc<Graph>> {
        self.borrow().and_then(|editor| editor.displayed_graph())
    }

    pub fn set_wire_painter(&self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], graph: &Arc<Graph>, painter: Option<Rc<RefCell<dyn WirePainter>>>) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.set_wire_painter(cx, instance, revision, path, graph, painter))
    }

    pub fn set_node_overlays(&self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], graph: &Arc<Graph>, overlays: Vec<(String, WidgetRef)>) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.set_node_overlays(cx, instance, revision, path, graph, overlays))
    }

    pub fn set_node_faces(&self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], faces: Vec<(String, WidgetRef)>) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.set_node_faces(cx, instance, revision, path, faces))
    }

    pub fn set_presented_node_faces(&self, cx: &mut Cx, instance: &str, revision: u64, path: &[String], faces: Vec<DebugNodeFace>) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.set_presented_node_faces(cx, instance, revision, path, faces))
    }

    pub fn set_node_streaming(&self, cx: &mut Cx, node: &str, on: bool) {
        if let Some(mut editor) = self.borrow_mut() { editor.set_node_streaming(cx, node, on); }
    }

    pub fn show_instance(&self, cx: &mut Cx, instance: &str) -> bool {
        self.borrow_mut().is_some_and(|mut editor| editor.show_instance(cx, instance))
    }

    pub fn set_instances(&self, cx: &mut Cx, instances: Vec<DebugInstance>) {
        if let Some(mut editor) = self.borrow_mut() { editor.set_instances(cx, instances); }
    }
}

// Host controls draw in the same render-local coordinates as node faces.
fn remap_overlay_event(event: &Event, camera: &Camera) -> Option<Event> {
    Some(match event {
        Event::MouseDown(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseDown(e)
        }
        Event::MouseMove(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            e.lock_delta /= camera.scale;
            Event::MouseMove(e)
        }
        Event::MouseUp(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseUp(e)
        }
        Event::MouseLeave(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseLeave(e)
        }
        Event::Scroll(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            e.scroll /= camera.scale;
            Event::Scroll(e)
        }
        Event::LongPress(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::LongPress(e)
        }
        Event::TouchUpdate(e) => {
            let mut e = e.clone();
            for touch in &mut e.touches {
                touch.abs = camera.screen_to_local(touch.abs);
                touch.radius /= camera.scale;
            }
            Event::TouchUpdate(e)
        }
        Event::SelectionHandleDrag(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::SelectionHandleDrag(e)
        }
        Event::Drag(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::Drag(e)
        }
        Event::Drop(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::Drop(e)
        }
        Event::TweakRay(e) => Event::TweakRay(TweakRayEvent {
            abs: camera.screen_to_local(e.abs),
            window_id: e.window_id,
            modifiers: e.modifiers,
            time: e.time,
            dpi_factor: e.dpi_factor,
            hit_widget_uids: RefCell::new(e.hit_widget_uids.borrow().clone()),
            hit_rect: std::cell::Cell::new(e.hit_rect.get()),
        }),
        _ => return None,
    })
}

/// A hit the faces claimed on the mapped clone is a hit on the original.
fn sync_overlay_handled(original: &Event, remapped: &Event, camera: &Camera) {
    match (original, remapped) {
        (Event::MouseDown(a), Event::MouseDown(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::MouseMove(a), Event::MouseMove(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::MouseLeave(a), Event::MouseLeave(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::Scroll(a), Event::Scroll(b)) => {
            if b.handled_x.get() {
                a.handled_x.set(true);
            }
            if b.handled_y.get() {
                a.handled_y.set(true);
            }
        }
        (Event::TouchUpdate(a), Event::TouchUpdate(b)) => {
            for (x, y) in a.touches.iter().zip(b.touches.iter()) {
                if x.handled.get().is_empty() {
                    x.handled.set(y.handled.get());
                }
                if x.sweep_lock.get().is_empty() {
                    x.sweep_lock.set(y.sweep_lock.get());
                }
            }
        }
        (Event::TweakRay(a), Event::TweakRay(b)) => {
            *a.hit_widget_uids.borrow_mut() = b.hit_widget_uids.borrow().clone();
            a.hit_rect.set(b.hit_rect.get().map(|rect| Rect {
                pos: camera.local_to_screen(rect.pos),
                size: rect.size * camera.scale,
            }));
        }
        _ => {}
    }
}
