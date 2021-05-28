use {
    makepad_render::*,
    std::collections::{HashMap, HashSet},
};

pub struct Tree {
    nodes_by_node_id: HashMap<NodeId, Node>,
    node_ids_by_area: HashMap<Area, NodeId>,
    animating_node_ids: HashSet<NodeId>,
    next_frame: NextFrame,
    needs_redraw: bool,
}

impl Tree {
    pub fn new() -> Tree {
        Tree {
            nodes_by_node_id: HashMap::new(),
            node_ids_by_area: HashMap::new(),
            animating_node_ids: HashSet::new(),
            needs_redraw: false,
            next_frame: NextFrame::default(),
        }
    }

    pub fn begin(&mut self) {
        self.needs_redraw = false;
    }

    pub fn end(&mut self) {}

    pub fn begin_node(&mut self, node_id: NodeId) -> NodeInfo {
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        NodeInfo {
            is_expanded_fraction: node.is_expanded.fraction,
            is_selected: node.is_selected,
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

    pub fn node_is_selected(&mut self, node_id: NodeId) -> bool {
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        node.is_expanded.value
    }

    pub fn set_node_is_selected(&mut self, node_id: NodeId, is_selected: bool) {
        let node = self.nodes_by_node_id.entry(node_id).or_default();
        if node.is_selected == is_selected {
            return;
        }
        node.is_selected = is_selected;
        self.needs_redraw = true;
    }

    pub fn toggle_node_is_selected(&mut self, node_id: NodeId) {
        let is_selected = self.node_is_selected(node_id);
        self.set_node_is_selected(node_id, !is_selected);
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

    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
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
                    self.toggle_node_is_expanded(cx, node_id, true);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId(pub usize);

#[derive(Clone, Copy, Debug)]
pub struct NodeInfo {
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
    is_selected: bool,
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
            is_selected: false,
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
            self.fraction = 1.0 - 0.5 * (1.0 - self.fraction);
            if 1.0 - self.fraction < 1.0E-3 {
                self.fraction = 1.0;
            }
        } else {
            self.fraction = 0.5 * self.fraction;
            if self.fraction < 1.0E-3 {
                self.fraction = 0.0;
            }
        }
    }
}
