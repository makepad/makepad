//! Flow's bounded, retained component views. The document remains flat at each
//! level; zoom changes presentation, never the open definition or its source.

use crate::{faces::FaceHost, graph_edit, graph_view, values::ValueCache};
use makepad_flow::{
    document::{closure::Closure, Registry},
    ComponentDef, Graph, Node, NodeTypeCatalog, ValueSlotRef,
};
use makepad_flowgraph::{
    BoundaryLink, Camera, CanvasEventPolicy, CanvasNodeRegion, CanvasPick, CanvasViewport, EmbeddedCanvasRoot,
    FaceViewport, FlowCanvas, FlowCanvasAction, GraphView, NodeFacesScope, NodeStatus, PortView,
    CANVAS_SCALE_MIN,
};
use makepad_widgets::makepad_script::{Apply, ScriptObject};
use makepad_widgets::*;
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

const MAX_DEPTH: usize = 3;
const MAX_LEVELS: usize = 12;
const MAX_NODES: usize = 256;
const REVEAL_SCALE: f64 = 0.24;
const HIDE_SCALE: f64 = 0.18;
const GROUP_SIZE: (f64, f64) = (520.0, 360.0);
const INNER_NODE_SIZE: (f64, f64) = (300.0, 240.0);
const BOUNDARY_RAIL: f64 = 78.0;

pub type Status = (String, u16, bool, String, Option<String>);

struct Config {
    root: Graph,
    catalog: Vec<NodeTypeCatalog>,
    registry: Arc<Registry>,
    closure: Closure,
    instance: String,
    /// The canvas every level's canvas is styled after: the app's root
    /// canvas, whose source the main VM refreshes on every reload.
    canvas: WidgetRef,
    /// That canvas's source as last taken; new levels are minted from it.
    style: RefCell<ScriptObjectRef>,
    budget: RefCell<(usize, usize)>,
}

struct Level {
    node: String,
    path: Vec<String>,
    alias: String,
    graph: Graph,
    boundary_nodes: Vec<String>,
    canvas: WidgetRef,
    faces: Option<Box<FaceHost>>,
    bounds: Rect,
    revealed: bool,
    requested: bool,
    values: HashMap<(String, String), ValueSlotRef>,
    statuses: HashMap<String, Status>,
}

pub struct Hierarchy {
    config: Rc<Config>,
    depth: usize,
    levels: Vec<Level>,
    /// The style object this hierarchy's canvases were last applied from.
    applied_style: ScriptObject,
}

/// Same domain-then-type join as `faces::catalog_entry_for_node`.
fn catalog_entry_for_node<'a>(
    node: &Node,
    catalog: &'a [NodeTypeCatalog],
) -> Option<&'a NodeTypeCatalog> {
    node.domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
        .and_then(|domain| {
            catalog
                .iter()
                .find(|entry| entry.domain.as_deref() == Some(domain))
        })
        .or_else(|| {
            catalog
                .iter()
                .find(|entry| entry.type_name == node.type_name)
        })
}

/// Empty or `NodeFace` catalog faces have no controls. Unknown types keep
/// the rich fallback. Explicit `ui:` / authored size always win later.
fn is_ports_only(node: &Node, catalog: &[NodeTypeCatalog]) -> bool {
    if node.kind == "component" || node.face_src.is_some() {
        return false;
    }
    let Some(entry) = catalog_entry_for_node(node, catalog) else {
        return false;
    };
    let face = entry.face.trim();
    face.is_empty() || face == "NodeFace"
}

fn ports_only_size(node: &Node) -> (f64, f64) {
    let in_len = node.inputs.iter().map(|input| input.port.chars().count()).max().unwrap_or(0);
    let out_len = node.outputs.iter().map(|output| output.name.chars().count()).max().unwrap_or(0);
    let width = (68.0 + 7.0 * (in_len + out_len) as f64).clamp(180.0, 320.0);
    let rows = node.inputs.len().max(node.outputs.len());
    (width, 46.0 + 24.0 * rows as f64)
}

/// Presentation default for a missing `size`. Does not write the graph.
fn presentation_size(node: &Node, catalog: &[NodeTypeCatalog], nested: bool) -> (f64, f64) {
    if node.kind == "component" {
        GROUP_SIZE
    } else if is_ports_only(node, catalog) {
        ports_only_size(node)
    } else if nested {
        INNER_NODE_SIZE
    } else {
        graph_edit::default_node_size(node)
    }
}

fn apply_presentation_sizes(
    view: &mut GraphView,
    graph: &Graph,
    catalog: &[NodeTypeCatalog],
    nested: bool,
) {
    for node in &mut view.nodes {
        if node.size.is_some() {
            continue;
        }
        let Some(source) = graph.nodes.iter().find(|source| source.id == node.id) else {
            continue;
        };
        if source.size.is_some() {
            node.size = source.size;
            continue;
        }
        // Root rich/custom/unknown faces stay unsized so the canvas measures
        // mounted controls. Auto-placement still uses default_node_size.
        if nested || source.kind == "component" || is_ports_only(source, catalog) {
            node.size = Some(presentation_size(source, catalog, nested));
        }
    }
}

fn expand_component_inputs(view: &mut GraphView, graph: &Graph) {
    for node in &mut view.nodes {
        if node.kind != "component" {
            continue;
        }
        let Some(definition) = graph.components.iter().find(|definition| definition.alias == node.type_name) else {
            continue;
        };
        // Runtime projections only list supplied `in:` entries. The
        // editor also shows omitted/defaulted component input slots.
        let supplied = std::mem::take(&mut node.inputs);
        node.inputs = definition.inputs.iter().map(|port| {
            supplied.iter().find(|input| input.name == port.name).cloned()
                .unwrap_or_else(|| PortView { name: port.name.clone(), kind: port.ty.as_str().to_string(),
                    connected: false, many: port.many.is_some() })
        }).collect();
        node.inputs.extend(supplied.into_iter().filter(|input| {
            !definition.inputs.iter().any(|port| port.name == input.name)
        }));
    }
}

fn project_view(graph: &Graph, catalog: &[NodeTypeCatalog], nested: bool) -> GraphView {
    let mut graph = graph.clone();
    if nested {
        auto_place_inner(&mut graph, catalog);
    } else {
        auto_place(&mut graph, catalog);
    }
    let mut view = graph_view::view_of(&graph);
    apply_presentation_sizes(&mut view, &graph, catalog, nested);
    expand_component_inputs(&mut view, &graph);
    view
}

/// Fixed group bodies are presentation defaults. Authored layout is untouched.
pub fn view_of(graph: &Graph, catalog: &[NodeTypeCatalog]) -> GraphView {
    project_view(graph, catalog, false)
}

/// The root/remount projection uses the same default container bounds
/// as its canvas. This only fills missing positions on the supplied copy.
pub fn auto_place(graph: &mut Graph, catalog: &[NodeTypeCatalog]) {
    graph_edit::auto_place_with_sizes(graph, |node| presentation_size(node, catalog, false));
}

fn auto_place_inner(graph: &mut Graph, catalog: &[NodeTypeCatalog]) {
    graph_edit::auto_place_with_sizes(graph, |node| presentation_size(node, catalog, true));
}

/// Only interface boundaries become viewport slots. Connectivity is irrelevant:
/// scalar defaults and empty ordered inputs still expose their declared slot.
/// Keep an inconsistent/missing endpoint visible as a card rather than hiding it.
fn component_boundary_nodes(definition: &ComponentDef) -> Vec<String> {
    definition.inputs.iter().filter_map(|port| {
        definition.graph.nodes.iter().find(|node| node.id == port.name && node.kind == "input"
            && node.outputs.first().is_some_and(|output| output.shape() == port.shape()))
            .map(|node| node.id.clone())
    }).chain(definition.outputs.iter().filter_map(|port| {
        definition.graph.nodes.iter().find(|node| node.id == port.name && node.kind == "output"
            && node.inputs.iter().find(|input| input.port == "value").is_some_and(|input| input.shape() == port.shape()))
            .map(|node| node.id.clone())
    })).collect()
}

pub fn prefix(path: &[String]) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{}/", path.join("/"))
    }
}

/// Component pins change when their source layout changes. Every executable
/// field must still agree before retaining the mounted hierarchy.
fn same_layout_tree(before: &Graph, after: &Graph) -> bool {
    if before.components.len() != after.components.len() { return false; }
    for (old, new) in before.components.iter().zip(&after.components) {
        if old.alias != new.alias || old.path != new.path || old.inputs != new.inputs
            || old.outputs != new.outputs || !same_layout_tree(&old.graph, &new.graph) {
            return false;
        }
    }
    let mut projection = before.clone();
    projection.components.clone_from(&after.components);
    makepad_flow::live::same_root_layout_execution(&projection, after)
}

impl Hierarchy {
    /// Refresh all occurrences after a layout save, including updated module
    /// pins used by children that have not mounted yet. Existing view transforms
    /// and fit bounds remain stable: saving a drag never reframes the contents.
    /// Return false for a semantic change; the caller should remount then.
    pub fn update_layouts(&mut self, cx: &mut Cx, root: &Graph, closure: &Closure) -> bool {
        if !same_layout_tree(&self.config.root, root) { return false; }
        let config = Rc::new(Config {
            root: root.clone(), catalog: self.config.catalog.clone(),
            registry: self.config.registry.clone(), closure: closure.clone(),
            instance: self.config.instance.clone(), canvas: self.config.canvas.clone(),
            style: RefCell::new(self.config.style.borrow().clone()),
            budget: RefCell::new(*self.config.budget.borrow()),
        });
        self.refresh_layouts(cx, config);
        true
    }

    fn refresh_layouts(&mut self, cx: &mut Cx, config: Rc<Config>) {
        self.config = config.clone();
        for level in &mut self.levels {
            if let Some(definition) = config.root.components.iter().find(|definition| definition.alias == level.alias) {
                level.boundary_nodes = component_boundary_nodes(definition);
                let mut graph = definition.graph.clone();
                graph.components = config.root.components.clone();
                // Unplaced nodes retain their initial projection. Browsing or
                // saving a sibling must not run automatic placement again.
                for node in &mut graph.nodes {
                    if node.at.is_none() {
                        node.at = level.graph.nodes.iter().find(|old| old.id == node.id).and_then(|old| old.at);
                    }
                }
                auto_place_inner(&mut graph, &config.catalog);
                let view = project_view(&graph, &config.catalog, true);
                if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                    canvas.set_boundary_nodes(cx, level.boundary_nodes.clone());
                    for node in view.nodes {
                        canvas.set_node_layout(cx, &node.id, node.at, node.size, node.flip);
                    }
                }
                level.graph = graph;
            }
            if let Some(hierarchy) = level.faces.as_mut().and_then(|faces| faces.hierarchy.as_mut()) {
                hierarchy.refresh_layouts(cx, config.clone());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cx: &mut Cx,
        canvas: &WidgetRef,
        graph: &Graph,
        root: &Graph,
        path: &[String],
        catalog: &[NodeTypeCatalog],
        registry: Arc<Registry>,
        closure: Closure,
        instance: String,
    ) -> Self {
        let source = canvas.script_source();
        let style = cx.with_vm(|vm| vm.bx.heap_mut().new_object_ref(source));
        let config = Rc::new(Config {
            root: root.clone(),
            catalog: catalog.to_vec(),
            registry,
            closure,
            instance,
            canvas: canvas.clone(),
            style: RefCell::new(style),
            budget: RefCell::new((0, 0)),
        });
        Self::build(cx, canvas, graph, path, config, 0)
    }

    fn build(
        cx: &mut Cx,
        parent: &WidgetRef,
        graph: &Graph,
        path: &[String],
        config: Rc<Config>,
        depth: usize,
    ) -> Self {
        let mut levels = Vec::new();
        if depth < MAX_DEPTH {
            for node in graph.nodes.iter().filter(|node| node.kind == "component") {
                let Some(definition) = config
                    .root
                    .components
                    .iter()
                    .find(|def| def.alias == node.type_name)
                else {
                    continue;
                };
                {
                    let mut budget = config.budget.borrow_mut();
                    if budget.0 >= MAX_LEVELS || budget.1 + definition.graph.nodes.len() > MAX_NODES
                    {
                        continue;
                    }
                    budget.0 += 1;
                    budget.1 += definition.graph.nodes.len();
                }
                let mut graph = definition.graph.clone();
                let boundary_nodes = component_boundary_nodes(definition);
                graph.components = config.root.components.clone();
                auto_place_inner(&mut graph, &config.catalog);
                let style = config.style.borrow().as_object();
                let canvas = cx.with_vm(|vm| {
                    WidgetRef::script_from_value(vm, style.into())
                });
                // Nested fit is stable before faces mount and while reveal
                // changes. Authored dimensions win; generic cards scroll.
                let view = project_view(&graph, &config.catalog, true);
                let bounds = if let Some(mut inner) = canvas.borrow_mut::<FlowCanvas>() {
                    inner.set_graph(cx, Some(view));
                    inner.set_boundary_nodes(cx, boundary_nodes.clone());
                    inner.set_compatible_ports(graph_view::compatibility_of(&graph));
                    inner.graph_bounds().unwrap_or(Rect {
                        pos: dvec2(0.0, 0.0),
                        size: dvec2(300.0, 240.0),
                    })
                } else {
                    continue;
                };
                let mut child_path = path.to_vec();
                child_path.push(node.id.clone());
                levels.push(Level {
                    node: node.id.clone(),
                    path: child_path,
                    alias: node.type_name.clone(),
                    graph,
                    boundary_nodes,
                    canvas,
                    faces: None,
                    bounds,
                    revealed: false,
                    requested: false,
                    values: HashMap::new(),
                    statuses: HashMap::new(),
                });
            }
        }
        if let Some(mut parent) = parent.borrow_mut::<FlowCanvas>() {
            parent.set_embedded_roots(
                cx,
                levels
                    .iter()
                    .map(|level| EmbeddedCanvasRoot {
                        id: LiveId::from_str(&format!("embedded_{}", level.node)),
                        node: level.node.clone(),
                        widget: level.canvas.clone(),
                    })
                    .collect(),
            );
        }
        let applied_style = config.style.borrow().as_object();
        Self {
            config,
            depth,
            levels,
            applied_style,
        }
    }

    /// Bring the nested canvases and every mounted host below them to what
    /// the main VM shows. Outside draw, after the app's own re-apply: the
    /// root canvas's source is the reloaded one then, so a level's canvas is
    /// re-applied from it in place (`Rebake` keeps the graph, camera,
    /// selection and capture, which are Rust state) and a level mounted
    /// later is minted from it. Each host decides for its own isolate by
    /// appearance epoch.
    pub fn apply_appearance(&mut self, cx: &mut Cx) {
        let source = self.config.canvas.script_source();
        if source != ScriptObject::ZERO && source != self.applied_style {
            if self.config.style.borrow().as_object() != source {
                let style = cx.with_vm(|vm| vm.bx.heap_mut().new_object_ref(source));
                *self.config.style.borrow_mut() = style;
            }
            for level in &mut self.levels {
                cx.with_vm(|vm| {
                    level.canvas.script_apply(vm, &Apply::Rebake, &mut Scope::empty(), source.into());
                });
            }
            self.applied_style = source;
        }
        for level in &mut self.levels {
            if let Some(faces) = level.faces.as_mut() {
                faces.apply_appearance(cx);
            }
        }
    }

    /// At most one isolate is evaluated per preparation tick, never in draw.
    pub fn prepare_one(&mut self, cx: &mut Cx) -> bool {
        for level in &mut self.levels {
            if level.requested && level.faces.is_none() {
                let Some(module) = self.config.closure.module(&level.alias) else {
                    continue;
                };
                let mut faces = FaceHost::mount_scoped_with_registry(
                    cx,
                    level.canvas.widget_uid(),
                    &self.config.instance,
                    &module.path,
                    &module.source,
                    &level.graph,
                    &self.config.catalog,
                    &self.config.closure,
                    Some(&level.alias),
                    &self.config.registry,
                    &level.path,
                );
                faces.set_locked(cx, true);
                let roots = faces
                    .faces
                    .iter()
                    .map(|(id, face)| (LiveId::from_str(id), face.root.clone()))
                    .collect();
                if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                    canvas.set_face_roots(cx, roots);
                    canvas.set_face_errors(cx, faces.face_errors(&level.graph));
                }
                faces.hierarchy = Some(Self::build(
                    cx,
                    &level.canvas,
                    &level.graph,
                    &level.path,
                    self.config.clone(),
                    self.depth + 1,
                ));
                level.faces = Some(Box::new(faces));
                level.canvas.redraw(cx);
                return true;
            }
            if let Some(hierarchy) = level
                .faces
                .as_mut()
                .and_then(|faces| faces.hierarchy.as_mut())
            {
                if hierarchy.prepare_one(cx) {
                    return true;
                }
            }
        }
        false
    }

    /// Called only from the owning card's face callback. No VM evaluation,
    /// projection, route admission, or source loading occurs on this path.
    pub fn draw(
        &mut self,
        cx: &mut Cx2d,
        node: &str,
        walk: Walk,
        parent: &FaceViewport,
        face_engaged: bool,
    ) -> bool {
        let Some(level) = self.levels.iter_mut().find(|level| level.node == node) else {
            return false;
        };
        let content = parent.geometry.content;
        let padding = 12.0;
        let rails = if parent.ports.is_empty() { 0.0 } else { BOUNDARY_RAIL };
        let fit = ((content.size.x - padding * 2.0 - rails * 2.0) / level.bounds.size.x.max(1.0))
            .min((content.size.y - padding * 2.0) / level.bounds.size.y.max(1.0))
            .clamp(0.0001, 1.0);
        let scale = parent.canvas.camera.scale * fit;
        let child_engaged = level.faces.as_ref().is_some_and(|faces| {
            faces.owns_key_focus(cx)
                || faces.has_open_popup()
                || faces
                    .hierarchy
                    .as_ref()
                    .is_some_and(|hierarchy| hierarchy.has_engaged_face(cx))
        });
        level.revealed = if level.revealed {
            scale >= HIDE_SCALE || child_engaged
        } else {
            scale >= REVEAL_SCALE && !face_engaged
        };
        level.requested |=
            level.revealed && parent.content_clip.size.x > 0.0 && parent.content_clip.size.y > 0.0;
        let parent_camera = parent.canvas.camera;
        let view = Rect {
            pos: parent_camera
                .local_to_screen(parent_camera.world_to_render((content.pos.x, content.pos.y))),
            size: content.size * parent_camera.scale,
        };
        let child_translation = parent_camera.view.pos
            + parent_camera.pan
            + (content.pos + dvec2(padding + rails, padding) - level.bounds.pos * fit)
                * parent_camera.scale;
        let camera = Camera {
            view,
            pan: child_translation - view.pos,
            scale: scale.max(CANVAS_SCALE_MIN),
            render_origin: None,
        };
        let clip = if level.revealed {
            parent.content_clip
        } else {
            Rect {
                size: dvec2(0.0, 0.0),
                ..view
            }
        };
        let mut scope_data;
        let mut scope = if let Some(faces) = level.faces.as_mut() {
            scope_data = NodeFacesScope::new(faces.as_mut());
            Scope::with_data(&mut scope_data)
        } else {
            Scope::empty()
        };
        if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
            let mut links = Vec::new();
            if level.revealed {
                let mut rows = [0usize; 2];
                let mut totals = [0usize; 2];
                let midpoint = view.pos.x + view.size.x * 0.5;
                for port in &parent.ports { totals[usize::from(port.anchor.x >= midpoint)] += 1; }
                for port in &parent.ports {
                    let Some(boundary) = level.graph.nodes.iter().find(|node| node.id == port.name
                        && level.boundary_nodes.contains(&node.id)
                        && node.kind == if port.output { "output" } else { "input" }) else { continue; };
                    let inner_port = if port.output { boundary.inputs.iter().find(|port| port.port == "value").map(|port| port.port.clone()) }
                        else { boundary.outputs.first().map(|port| port.name.clone()) };
                    let Some(inner_port) = inner_port else { continue; };
                    let on_right = port.anchor.x >= midpoint;
                    let side = usize::from(on_right);
                    let pitch = 36.0f64.min((content.size.y - 16.0) / totals[side].max(1) as f64).max(4.0);
                    let slot = Rect {
                        pos: view.pos + dvec2(
                            if on_right { content.size.x - BOUNDARY_RAIL + 4.0 } else { 4.0 },
                            8.0 + rows[side] as f64 * pitch,
                        ) * parent_camera.scale,
                        size: dvec2(BOUNDARY_RAIL - 8.0, pitch - 2.0) * parent_camera.scale,
                    };
                    rows[side] += 1;
                    let many = if port.output { boundary.inputs.iter().find(|port| port.port == "value").and_then(|port| port.many) }
                        else { boundary.outputs.first().and_then(|port| port.many) };
                    let label = if many.is_some() { format!("{}[]", port.name) } else { port.name.clone() };
                    links.push(BoundaryLink { node: boundary.id.clone(), port: inner_port,
                        output: port.output, on_right, label, kind: port.kind.clone(), outer: port.anchor, slot });
                }
            }
            canvas.set_boundary_links(links);
            let consumed = if level.revealed {
                walk
            } else {
                Walk::fixed(0.0, 0.0)
            };
            if canvas
                .draw_embedded(cx, &mut scope, consumed, CanvasViewport { camera, clip })
                .is_err()
            {
                level.revealed = false;
            }
        }
        level.revealed
    }

    pub fn target(&self, parent: &FlowCanvas, point: DVec2) -> Option<Vec<String>> {
        let Some(CanvasPick::Node {
            node,
            region: CanvasNodeRegion::Content,
        }) = parent.pick(point)
        else {
            return None;
        };
        let level = self
            .levels
            .iter()
            .find(|level| level.node == node && level.revealed)?;
        let canvas = level.canvas.borrow::<FlowCanvas>()?;
        canvas.pick(point)?;
        level
            .faces
            .as_ref()
            .and_then(|faces| faces.hierarchy.as_ref())
            .and_then(|hierarchy| hierarchy.target(&canvas, point))
            .or_else(|| Some(level.path.clone()))
    }

    pub fn canvas(&self, path: &[String]) -> Option<WidgetRef> {
        for level in &self.levels {
            if level.path == path {
                return Some(level.canvas.clone());
            }
            if let Some(canvas) = level
                .faces
                .as_ref()
                .and_then(|faces| faces.hierarchy.as_ref())
                .and_then(|hierarchy| hierarchy.canvas(path))
            {
                return Some(canvas);
            }
        }
        None
    }

    pub fn graph(&self, path: &[String]) -> Option<Graph> {
        for level in &self.levels {
            if level.path == path {
                return Some(level.graph.clone());
            }
            if let Some(graph) = level
                .faces
                .as_ref()
                .and_then(|faces| faces.hierarchy.as_ref())
                .and_then(|hierarchy| hierarchy.graph(path))
            {
                return Some(graph);
            }
        }
        None
    }

    pub fn restore_readonly(&self, cx: &mut Cx, path: &[String]) {
        if let (Some(graph), Some(widget)) = (self.graph(path), self.canvas(path)) {
            let view = project_view(&graph, &self.config.catalog, true);
            if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
                canvas.set_graph(cx, Some(view));
                canvas.set_boundary_nodes(cx, self.boundaries(path).unwrap_or_default());
            }
        }
    }

    fn boundaries(&self, path: &[String]) -> Option<Vec<String>> {
        for level in &self.levels {
            if level.path == path { return Some(level.boundary_nodes.clone()); }
            if let Some(nodes) = level.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref())
                .and_then(|hierarchy| hierarchy.boundaries(path)) {
                return Some(nodes);
            }
        }
        None
    }

    pub fn focused_path(&self, cx: &Cx) -> Option<Vec<String>> {
        for level in &self.levels {
            if let Some(faces) = &level.faces {
                if let Some(path) = faces
                    .hierarchy
                    .as_ref()
                    .and_then(|hierarchy| hierarchy.focused_path(cx))
                {
                    return Some(path);
                }
                if faces.owns_key_focus(cx) || faces.has_open_popup() {
                    return Some(level.path.clone());
                }
            }
            if !cx.key_focus().is_empty() && level.canvas.area() == cx.key_focus() {
                return Some(level.path.clone());
            }
        }
        None
    }

    fn has_engaged_face(&self, cx: &Cx) -> bool {
        self.levels.iter().any(|level| {
            level.faces.as_ref().is_some_and(|faces| {
                faces.owns_key_focus(cx)
                    || faces.has_open_popup()
                    || faces
                        .hierarchy
                        .as_ref()
                        .is_some_and(|hierarchy| hierarchy.has_engaged_face(cx))
            })
        })
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, target: Option<&[String]>) {
        for level in &mut self.levels {
            let selected = target == Some(level.path.as_slice());
            let camera = level
                .canvas
                .borrow::<FlowCanvas>()
                .map(|canvas| canvas.camera());
            if let Some(faces) = level.faces.as_mut() {
                if selected || !is_input(event) {
                    faces.handle_event(cx, event, &mut Scope::empty(), camera.as_ref());
                }
                if let Some(hierarchy) = faces.hierarchy.as_mut() {
                    hierarchy.handle_event(cx, event, target);
                }
            }
            if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                if matches!(event, Event::WindowLostFocus(_)) {
                    canvas.cancel_gesture(cx);
                }
                if selected {
                    if let Event::MouseDown(event) = event {
                        canvas.select_at(cx, event.abs);
                    }
                }
                canvas.handle_event_routed(
                    cx,
                    event,
                    &mut Scope::empty(),
                    CanvasEventPolicy {
                        pointer: selected,
                        keyboard: selected,
                        navigation: false,
                    },
                );
            }
        }
    }

    pub fn actions(&self, actions: &Actions, out: &mut Vec<(Vec<String>, FlowCanvasAction)>) {
        for level in &self.levels {
            for action in actions
                .iter()
                .filter_map(|action| action.as_widget_action())
                .filter(|action| action.widget_uid == level.canvas.widget_uid())
            {
                out.push((level.path.clone(), action.cast()));
            }
            if let Some(hierarchy) = level
                .faces
                .as_ref()
                .and_then(|faces| faces.hierarchy.as_ref())
            {
                hierarchy.actions(actions, out);
            }
        }
    }

    pub fn sync(
        &mut self,
        cx: &mut Cx,
        outputs: &HashMap<String, Vec<(String, ValueSlotRef)>>,
        statuses: &HashMap<String, Status>,
        values: &mut ValueCache,
    ) {
        for level in &mut self.levels {
            let Some(faces) = level.faces.as_mut() else {
                continue;
            };
            let prefix = prefix(&level.path);
            if level.values.keys().any(|(node, port)| {
                outputs
                    .get(&format!("{prefix}{node}"))
                    .is_none_or(|ports| !ports.iter().any(|(name, _)| name == port))
            }) {
                faces.clear_values(cx);
                level.values.clear();
            }
            if level
                .statuses
                .keys()
                .any(|node| !statuses.contains_key(&format!("{prefix}{node}")))
            {
                if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                    canvas.clear_run(cx);
                }
                level.statuses.clear();
            }
            for node in &level.graph.nodes {
                let qualified = format!("{prefix}{}", node.id);
                if let Some(status) = statuses.get(&qualified) {
                    if level.statuses.get(&node.id) != Some(status) {
                        if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                            canvas.set_status(
                                cx,
                                &node.id,
                                NodeStatus::new(
                                    &status.0,
                                    status.1,
                                    status.2,
                                    &status.3,
                                    status.4.clone(),
                                ),
                            );
                        }
                        faces.push_state(cx, &node.id, &status.0);
                        level.statuses.insert(node.id.clone(), status.clone());
                    }
                }
                if let Some(ports) = outputs.get(&qualified) {
                    for (port, slot) in ports {
                        let key = (node.id.clone(), port.clone());
                        if node.kind == "input" {
                            if let ValueSlotRef::One(value) = slot {
                                if value.ty == makepad_flow::PortType::Text {
                                    let bytes = values.get(&value.digest);
                                    let text = bytes
                                        .as_ref()
                                        .map(|bytes| {
                                            String::from_utf8_lossy(&bytes.bytes).into_owned()
                                        })
                                        .or_else(|| match &value.preview {
                                            Some(makepad_flow::Literal::Str(text)) => {
                                                Some(text.clone())
                                            }
                                            _ => None,
                                        });
                                    if let Some(text) = text {
                                        faces.fill_input_value(cx, &node.id, port, &text);
                                    }
                                    if bytes.is_none() && !faces.wanted.contains(&value.digest) {
                                        faces.wanted.push(value.digest.clone());
                                    }
                                }
                            }
                        }
                        if level.values.get(&key) == Some(slot) {
                            continue;
                        }
                        match slot {
                            ValueSlotRef::One(value) => {
                                faces.push_value(
                                    cx,
                                    &node.id,
                                    port,
                                    value,
                                    values.get(&value.digest).as_ref(),
                                );
                            }
                            ValueSlotRef::Seq(sequence) => {
                                faces.push_sequence(cx, &node.id, port, sequence)
                            }
                        }
                        if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                            canvas.pulse(cx, &node.id, true);
                        }
                        level.values.insert(key, slot.clone());
                    }
                }
            }
            if let Some(hierarchy) = faces.hierarchy.as_mut() {
                hierarchy.sync(cx, outputs, statuses, values);
            }
        }
    }

    pub fn wanted(&self, out: &mut Vec<String>) {
        for level in &self.levels {
            if let Some(faces) = &level.faces {
                out.extend(faces.wanted_values());
            }
        }
    }

    /// Bind every mounted descendant to the same provider set as the root.
    /// Call after prepare_one mounted a face or a live generation became
    /// ready, never from Draw. FaceHost paths qualify errors and endpoints.
    pub fn bind_frames(&self, cx: &mut Cx, sources: &crate::frames::FrameSources) -> Vec<String> {
        let mut errors = Vec::new();
        for level in &self.levels {
            if let Some(faces) = &level.faces {
                errors.extend(sources.bind_host(cx, faces));
                if let Some(hierarchy) = &faces.hierarchy {
                    errors.extend(hierarchy.bind_frames(cx, sources));
                }
            }
        }
        errors
    }

    /// Retained hierarchies can outlive their previous instance generation.
    /// Clear provider bindings and displayed textures before that transition.
    pub fn clear_frames(&self, cx: &mut Cx) {
        for level in &self.levels {
            if let Some(faces) = &level.faces {
                crate::frames::clear_host(cx, faces);
                if let Some(hierarchy) = &faces.hierarchy {
                    hierarchy.clear_frames(cx);
                }
            }
        }
    }

    pub fn deliver_bytes(&mut self, cx: &mut Cx, values: &mut ValueCache, digest: &str) {
        for level in &mut self.levels {
            if let Some(faces) = level.faces.as_mut() {
                faces.deliver_bytes(cx, values, digest);
            }
        }
    }

    pub fn open_requests(&self, actions: &Actions, out: &mut Vec<(String, String)>) {
        for level in &self.levels {
            if let Some(faces) = &level.faces {
                out.extend(faces.open_requests(actions));
            }
        }
    }

    pub fn free(self, cx: &mut Cx) {
        for level in self.levels {
            if let Some(mut canvas) = level.canvas.borrow_mut::<FlowCanvas>() {
                canvas.set_face_roots(cx, Vec::new());
                canvas.set_embedded_roots(cx, Vec::new());
            }
            if let Some(faces) = level.faces {
                crate::frames::clear_host(cx, &faces);
                faces.free(cx);
            }
        }
    }
}

pub fn pointer(event: &Event) -> Option<DVec2> {
    match event {
        Event::MouseDown(e) => Some(e.abs),
        Event::MouseMove(e) => Some(e.abs),
        Event::MouseUp(e) => Some(e.abs),
        Event::Scroll(e) => Some(e.abs),
        Event::TouchUpdate(e) => e.touches.first().map(|touch| touch.abs),
        _ => None,
    }
}

pub fn is_input(event: &Event) -> bool {
    pointer(event).is_some()
        || matches!(
            event,
            Event::KeyDown(_)
                | Event::KeyUp(_)
                | Event::TextInput(_)
                | Event::TextRangeReplace(_)
                | Event::TextCut(_)
                | Event::ImeAction(_)
                | Event::LongPress(_)
                | Event::SelectionHandleDrag(_)
        )
}
