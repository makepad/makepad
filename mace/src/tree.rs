use {
    makepad_render::*,
    std::collections::{HashMap, HashSet},
};

pub struct Tree {
    nodes: HashMap<NodeId, Node>,
    animating_nodes: HashSet<NodeId>,
    nodes_by_area: HashMap<Area, NodeId>,
    count: usize,
    next_frame: NextFrame,
}

impl Tree {
    pub fn new() -> Tree {
        Tree {
            nodes: HashMap::new(),
            animating_nodes: HashSet::new(),
            nodes_by_area: HashMap::new(),
            count: 0,
            next_frame: NextFrame::default(),
        }
    }

    pub fn begin(&mut self) {
        self.nodes_by_area.clear();
        self.count = 0;
    }

    pub fn end(&mut self) {}

    pub fn begin_node(&mut self, node_id: NodeId) -> NodeInfo {
        let count = self.count;
        self.count += 1;
        let node = self.nodes.entry(node_id).or_default();
        NodeInfo {
            count,
            is_expanded_fraction: node.is_expanded.fraction,
        }
    }

    pub fn end_node(&mut self) {}

    pub fn forget_tree(&mut self) {
        self.nodes.clear();
        self.animating_nodes.clear();
    }

    pub fn forget_node(&mut self, node_id: NodeId) {
        self.nodes.remove(&node_id).unwrap();
        self.animating_nodes.remove(&node_id);
    }

    pub fn node_is_expanded(&mut self, node_id: NodeId) -> bool {
        let node = self.nodes.entry(node_id).or_default();
        node.is_expanded.value
    }

    pub fn set_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        node_id: NodeId,
        is_expanded: bool,
        should_animate: bool,
    ) -> bool {
        let node = self.nodes.entry(node_id).or_default();
        if node.is_expanded.value == is_expanded {
            return false;
        }
        node.is_expanded.set_value(is_expanded, should_animate);
        if should_animate {
            let is_animating = node.is_animating();
            self.update_animating_nodes(cx, node_id, is_animating);
            false
        } else {
            true
        }
    }

    pub fn toggle_node_is_expanded(&mut self, cx: &mut Cx, node_id: NodeId, should_animate: bool) -> bool {
        let is_expanded = self.node_is_expanded(node_id);
        self.set_node_is_expanded(cx, node_id, !is_expanded, should_animate)
    }

    fn update_animating_nodes(&mut self, cx: &mut Cx, node_id: NodeId, is_animating: bool) {
        if is_animating {
            self.animating_nodes.insert(node_id);
        } else {
            self.animating_nodes.remove(&node_id);
        }
        self.update_next_frame(cx);
    }

    fn update_next_frame(&mut self, cx: &mut Cx) {
        if self.animating_nodes.is_empty() {
            self.next_frame = NextFrame::default();
        } else {
            self.next_frame = cx.new_next_frame();
        }
    }

    pub fn set_node_area(&mut self, node_id: NodeId, area: Area) {
        self.nodes_by_area.insert(area, node_id);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(Action)) {
        match event {
            Event::NextFrame(_) if self.next_frame.is_active(cx) => {
                let mut new_animating_nodes = HashSet::new();
                for node_id in &self.animating_nodes {
                    let node = self.nodes.get_mut(node_id).unwrap();
                    node.update();
                    if node.is_animating() {
                        new_animating_nodes.insert(*node_id);
                    }
                }
                dispatch_action(Action::Redraw);
                self.animating_nodes = new_animating_nodes;
                self.update_next_frame(cx);
            }
            event => {
                for (area, node_id) in &self.nodes_by_area {
                    match event.hits(cx, *area, HitOpt::default()) {
                        Event::FingerDown(_) => {
                            dispatch_action(Action::ToggleNodeIsExpanded(*node_id, true));
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId(pub usize);

#[derive(Clone, Copy, Debug)]
pub struct NodeInfo {
    pub count: usize,
    pub is_expanded_fraction: f32,
}

impl NodeInfo {
    pub fn is_fully_collapsed(&self) -> bool {
        self.is_expanded_fraction == 0.0
    }

    pub fn is_fully_expanded(&self) -> bool {
        self.is_expanded_fraction == 1.0
    }
}

#[derive(Clone, Debug)]
struct Node {
    is_expanded: AnimatedBool,
}

impl Node {
    fn is_animating(&self) -> bool {
        self.is_expanded.is_animating()
    }

    fn update(&mut self) {
        self.is_expanded.update_fraction();
    }
}

impl Default for Node {
    fn default() -> Self {
        Self {
            is_expanded: AnimatedBool::new(true),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AnimatedBool {
    value: bool,
    fraction: f32,
}

impl AnimatedBool {
    fn new(value: bool) -> Self {
        Self {
            value,
            fraction: value as u32 as f32,
        }
    }

    fn is_animating(&self) -> bool {
        self.fraction != self.value as u32 as f32
    }

    fn set_value(&mut self, value: bool, should_animate: bool) {
        self.value = value;
        if !should_animate {
            self.fraction = value as u32 as f32;
        }
    }

    fn update_fraction(&mut self) {
        if self.value {
            self.fraction = 1.0 - 0.6 * (1.0 - self.fraction);
            if 1.0 - self.fraction < 1.0E-3 {
                self.fraction = 1.0;
            }
        } else {
            self.fraction = 0.6 * self.fraction;
            if self.fraction < 1.0E-3 {
                self.fraction = 0.0;
            }
        }
    }
}

pub enum Action {
    ToggleNodeIsExpanded(NodeId, bool),
    Redraw,
}