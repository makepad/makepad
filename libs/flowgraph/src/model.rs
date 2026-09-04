use makepad_widgets::*;
use std::collections::{HashMap, HashSet};

/// The ordinary width used by automatic placement and new-card previews.
pub const NODE_WIDTH: f64 = 300.0;
/// The fallback position used by hosts when their source graph omits layout.
pub const FIRST_AT: (f64, f64) = (40.0, 120.0);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphView {
    pub nodes: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeView {
    pub id: String,
    pub title: String,
    pub type_name: String,
    /// Host-defined style key.
    pub kind: String,
    pub at: (f64, f64),
    pub size: Option<(f64, f64)>,
    pub flip: bool,
    pub inputs: Vec<PortView>,
    pub outputs: Vec<PortView>,
    pub full_bleed: bool,
    /// String-valued presentation parameters read by the canvas. At present
    /// this only lets an Output card advertise its selected port kind.
    pub params: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortView {
    pub name: String,
    /// Host-defined style and compatibility key.
    pub kind: String,
    pub connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeView {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
}

/// Targets keyed by the output that may connect to them. Keeping this beside
/// (rather than inside) `GraphView` leaves type-system policy with the host.
pub type CompatiblePorts =
    HashMap<(String, String), HashSet<(String, String)>>;

/// A host-defined port appearance, keyed by `PortView::kind`.
#[derive(Script, ScriptHook, Clone, Default)]
pub struct PortStyle {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub kind: String,
    #[live]
    pub color: Vec4f,
    #[live]
    pub icon: Option<ScriptHandleRef>,
}

impl PortStyle {
    pub fn new(color: Vec4f, icon: Option<ScriptHandleRef>) -> Self {
        Self {
            color,
            icon,
            ..Self::default()
        }
    }
}

/// A host-defined node appearance, keyed by `NodeView::kind`.
#[derive(Script, ScriptHook, Clone, Default)]
pub struct NodeStyle {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub kind: String,
    #[live]
    pub color: Vec4f,
    #[live]
    pub icon: Option<ScriptHandleRef>,
}

#[derive(Script)]
pub struct CanvasStyles {
    #[source]
    source: ScriptObjectRef,
    #[rust]
    node_styles: HashMap<String, NodeStyle>,
    #[rust]
    port_styles: HashMap<String, PortStyle>,
    #[rust]
    node_icons: HashMap<String, DrawSvg>,
    #[rust]
    port_icons: HashMap<String, DrawSvg>,
}

impl ScriptHook for CanvasStyles {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        let Some(object) = value.as_object() else {
            return;
        };
        let mut node_styles = HashMap::new();
        let mut port_styles = HashMap::new();
        vm.vec_with(object, |vm, entries| {
            for entry in entries {
                let Some(object) = entry.value.as_object() else {
                    continue;
                };
                if vm
                    .bx
                    .heap
                    .type_matches_id(object, NodeStyle::script_type_id_static())
                {
                    let style = NodeStyle::script_from_value(vm, entry.value);
                    node_styles.insert(style.kind.clone(), style);
                } else if vm
                    .bx
                    .heap
                    .type_matches_id(object, PortStyle::script_type_id_static())
                {
                    let style = PortStyle::script_from_value(vm, entry.value);
                    port_styles.insert(style.kind.clone(), style);
                }
            }
        });
        if !node_styles.is_empty() {
            self.node_styles = node_styles;
        }
        if !port_styles.is_empty() {
            self.port_styles = port_styles;
        }
        self.rebuild_icons(vm);
    }
}

impl CanvasStyles {
    fn rebuild_icons(&mut self, vm: &mut ScriptVm) {
        self.node_icons = icons_of(vm, &self.node_styles);
        self.port_icons = icons_of(vm, &self.port_styles);
    }

    pub(crate) fn set_nodes(&mut self, vm: &mut ScriptVm, styles: HashMap<String, NodeStyle>) {
        self.node_styles = styles;
        self.rebuild_icons(vm);
    }

    pub(crate) fn set_ports(&mut self, vm: &mut ScriptVm, styles: HashMap<String, PortStyle>) {
        self.port_styles = styles;
        self.rebuild_icons(vm);
    }

    pub(crate) fn node_color(&self, kind: &str) -> Option<Vec4f> {
        self.node_styles.get(kind).map(|style| style.color)
    }

    pub(crate) fn port_color(&self, kind: &str) -> Option<Vec4f> {
        self.port_styles.get(kind).map(|style| style.color)
    }

    pub(crate) fn node_icon(&mut self, kind: &str) -> Option<&mut DrawSvg> {
        let key = if self.node_icons.contains_key(kind) {
            kind
        } else {
            "flow"
        };
        self.node_icons.get_mut(key)
    }

    pub(crate) fn port_icon(&mut self, kind: &str) -> Option<&mut DrawSvg> {
        self.port_icons.get_mut(kind)
    }
}

fn icons_of<T>(vm: &mut ScriptVm, styles: &HashMap<String, T>) -> HashMap<String, DrawSvg>
where
    T: Style,
{
    styles
        .iter()
        .filter_map(|(kind, style)| {
            let svg = style.icon()?.clone();
            let mut draw = DrawSvg::script_new_with_default(vm);
            draw.svg = Some(svg);
            draw.color = style.color();
            Some((kind.clone(), draw))
        })
        .collect()
}

trait Style {
    fn color(&self) -> Vec4f;
    fn icon(&self) -> Option<&ScriptHandleRef>;
}

impl Style for NodeStyle {
    fn color(&self) -> Vec4f {
        self.color
    }

    fn icon(&self) -> Option<&ScriptHandleRef> {
        self.icon.as_ref()
    }
}

impl Style for PortStyle {
    fn color(&self) -> Vec4f {
        self.color
    }

    fn icon(&self) -> Option<&ScriptHandleRef> {
        self.icon.as_ref()
    }
}

/// The three face-host operations the canvas needs while drawing.
pub trait NodeFaces {
    fn draw_face(&mut self, cx: &mut Cx2d, node: &str, walk: Walk, card_sized: bool);
    fn set_z_order(&mut self, order: &[String]);
    fn set_popup_anchor_transform(
        &mut self,
        cx: &mut Cx,
        transform: Option<PopupAnchorTransform>,
    );
}

/// Concrete `Scope` payload for a borrowed [`NodeFaces`] implementation.
pub struct NodeFacesScope {
    faces: *mut dyn NodeFaces,
}

impl NodeFacesScope {
    pub fn new<T: NodeFaces + 'static>(faces: &mut T) -> Self {
        Self {
            faces: faces as *mut T as *mut dyn NodeFaces,
        }
    }

    pub(crate) fn faces(&mut self) -> &mut dyn NodeFaces {
        // SAFETY: the app creates this adapter immediately before dispatch;
        // neither `Scope` nor the canvas retains it after that call returns.
        unsafe { &mut *self.faces }
    }
}

impl NodeStyle {
    pub fn new(color: Vec4f, icon: Option<ScriptHandleRef>) -> Self {
        Self {
            color,
            icon,
            ..Self::default()
        }
    }
}

/// Node lookup and reverse adjacency shared by graph hosts and the canvas.
#[derive(Default)]
pub struct GraphIndex {
    nodes: HashMap<String, usize>,
    upstream: Vec<Vec<usize>>,
}

impl GraphIndex {
    pub fn new(graph: &GraphView) -> Self {
        Self::from_parts(
            graph.nodes.iter().map(|node| node.id.as_str()),
            graph
                .edges
                .iter()
                .map(|edge| (edge.from.as_str(), edge.to.as_str())),
        )
    }

    pub fn from_parts<'a>(
        nodes: impl IntoIterator<Item = &'a str>,
        edges: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let nodes: HashMap<String, usize> = nodes
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id.to_string(), index))
            .collect();
        let mut upstream = vec![Vec::new(); nodes.len()];
        for (from_id, to_id) in edges {
            let (Some(from), Some(to)) = (nodes.get(from_id), nodes.get(to_id)) else {
                continue;
            };
            upstream[*to].push(*from);
        }
        Self { nodes, upstream }
    }

    pub fn node(&self, id: &str) -> Option<usize> {
        self.nodes.get(id).copied()
    }

    pub fn ancestors(&self, node: usize) -> HashSet<usize> {
        let mut seen = HashSet::new();
        let mut stack = vec![node];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            stack.extend(self.upstream[node].iter().copied());
        }
        seen
    }

    pub fn ancestor_indices(&self, node: &str) -> HashSet<usize> {
        self.node(node)
            .map(|node| self.ancestors(node))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_index_follows_reverse_edges() {
        let index = GraphIndex::from_parts(["a", "b", "c"], [("a", "b"), ("b", "c")]);
        assert_eq!(index.node("b"), Some(1));
        assert_eq!(index.ancestor_indices("c"), HashSet::from([0, 1, 2]));
    }
}
