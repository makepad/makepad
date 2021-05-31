use {
    makepad_render::*,
    std::collections::{HashMap, HashSet},
};

pub struct Tree {
    nodes_by_node_id: HashMap<NodeId, Node>,
    animators_by_node_id: HashMap<NodeId, Animator>,
    animating_node_ids: HashSet<NodeId>,
    selected_node_ids: HashSet<NodeId>,
    node_ids_by_area: HashMap<Area, NodeId>,
    count: usize,
    needs_redraw: bool,
    next_frame: NextFrame,
}

impl Tree {
    pub fn new() -> Tree {
        Tree {
            nodes_by_node_id: HashMap::new(),
            animators_by_node_id: HashMap::new(),
            animating_node_ids: HashSet::new(),
            selected_node_ids: HashSet::new(),
            node_ids_by_area: HashMap::new(),
            count: 0,
            needs_redraw: false,
            next_frame: NextFrame::default(),
        }
    }

    pub fn begin(&mut self) {
        self.count = 0;
        self.needs_redraw = false;
    }

    pub fn end(&mut self) {}

    pub fn begin_node(&mut self, node_id: NodeId) -> NodeInfo {
        let count = self.count;
        self.count += 1;
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        NodeInfo {
            count,
            is_expanded_fraction: node.is_expanded.fraction,
            is_selected: self.selected_node_ids.contains(&node_id)
        }
    }

    pub fn end_node(&mut self) {}

    pub fn forget_tree(&mut self) {
        self.nodes_by_node_id.clear();
        self.node_ids_by_area.clear();
        self.animating_node_ids.clear();
    }

    pub fn forget_node(&mut self, node_id: NodeId) {
        let node = self.nodes_by_node_id.remove(&node_id).unwrap();
        if !node.area.is_empty() {
            self.node_ids_by_area.remove(&node.area);
        }
        self.animating_node_ids.remove(&node_id);
    }

    pub fn node_animator(&mut self, node_id: NodeId) -> &mut Animator {
        self.animators_by_node_id.entry(node_id).or_default()
    }

    pub fn set_node_area(&mut self, node_id: NodeId, area: Area) {
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        if !node.area.is_empty() {
            self.node_ids_by_area.remove(&node.area);
        }
        self.node_ids_by_area.insert(area, node_id);
        node.area = area;
    }

    pub fn node_is_expanded(&mut self, node_id: NodeId) -> bool {
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        node.is_expanded.value
    }

    pub fn set_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        node_id: NodeId,
        is_expanded: bool,
        should_animate: bool,
    ) {
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        if node.is_expanded.value == is_expanded {
            return;
        }
        node.is_expanded.set_value(is_expanded, should_animate);
        if should_animate {
            let is_animating = node.is_animating();
            self.update_animating_node_ids(cx, node_id, is_animating);
        } else {
            self.needs_redraw = true;
        }
    }

    pub fn toggle_node_is_expanded(&mut self, cx: &mut Cx, node_id: NodeId, should_animate: bool) {
        let is_expanded = self.node_is_expanded(node_id);
        self.set_node_is_expanded(cx, node_id, !is_expanded, should_animate);
    }

    fn update_animating_node_ids(&mut self, cx: &mut Cx, node_id: NodeId, is_animating: bool) {
        if is_animating {
            self.animating_node_ids.insert(node_id);
        } else {
            self.animating_node_ids.remove(&node_id);
        }
        self.update_next_frame(cx);
    }

    fn update_next_frame(&mut self, cx: &mut Cx) {
        if self.animating_node_ids.is_empty() {
            self.next_frame = NextFrame::default();
        } else {
            self.next_frame = cx.new_next_frame();
        }
    }

    pub fn set_node_is_selected(&mut self, node_id: NodeId, is_selected: bool) {
        if is_selected {
            if self.selected_node_ids.insert(node_id) {
                self.needs_redraw = true;
            }
        } else {
            if self.selected_node_ids.remove(&node_id) {
                self.needs_redraw = true;
            }
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(Action)) {
        match event {
            Event::NextFrame(_) if self.next_frame.is_active(cx) => {
                let mut new_animating_node_ids = HashSet::new();
                for node_id in &self.animating_node_ids {
                    let node = self.nodes_by_node_id.get_mut(node_id).unwrap();
                    node.update();
                    if node.is_animating() {
                        new_animating_node_ids.insert(*node_id);
                    }
                }
                self.needs_redraw = true;
                self.animating_node_ids = new_animating_node_ids;
                self.update_next_frame(cx);
            }
            event => {
                let mut clicked_node_ids = Vec::new();
                for (area, node_id) in &self.node_ids_by_area {
                    match event.hits(cx, *area, HitOpt::default()) {
                        Event::FingerDown(_) => {
                            clicked_node_ids.push(*node_id);
                        }
                        _ => {}
                    }
                }
                for node_id in clicked_node_ids {
                    dispatch_action(Action::ToggleNodeIsExpanded(node_id, true))
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
    pub is_selected: bool,
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
    area: Area,
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
            area: Area::Empty,
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
    ToggleNodeIsExpanded(NodeId, bool)
}