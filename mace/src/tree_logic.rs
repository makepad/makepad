use {
    makepad_render::*,
    std::collections::{HashMap, HashSet},
};

pub struct TreeLogic {
    node_ids_by_node_id: HashMap<NodeId, Node>,
    node_ids_by_area: HashMap<Area, NodeId>,
    animating_node_ids: HashSet<NodeId>,
    hovered_node_id: Option<NodeId>,
    selected_node_ids: HashSet<NodeId>,
    next_frame: NextFrame,
}

impl TreeLogic {
    pub fn new() -> TreeLogic {
        TreeLogic {
            node_ids_by_node_id: HashMap::new(),
            node_ids_by_area: HashMap::new(),
            animating_node_ids: HashSet::new(),
            hovered_node_id: None,
            selected_node_ids: HashSet::new(),
            next_frame: NextFrame::default(),
        }
    }

    pub fn begin(&mut self) {
        self.node_ids_by_area.clear();
    }

    pub fn end(&mut self) {}

    pub fn begin_node(&mut self, node_id: NodeId) -> NodeInfo {
        let node = self.node_ids_by_node_id.entry(node_id).or_default();
        NodeInfo {
            is_expanded_fraction: node.is_expanded.fraction,
            is_hovered: self
                .hovered_node_id
                .map_or(false, |hovered_node_id| hovered_node_id == node_id),
            is_selected: self.selected_node_ids.contains(&node_id),
        }
    }

    pub fn end_node(&mut self) {}

    pub fn forget(&mut self) {
        self.node_ids_by_node_id.clear();
        self.animating_node_ids.clear();
    }

    pub fn forget_node(&mut self, node_id: NodeId) {
        self.node_ids_by_node_id.remove(&node_id).unwrap();
        self.animating_node_ids.remove(&node_id);
    }

    pub fn set_node_area(&mut self, node_id: NodeId, area: Area) {
        self.node_ids_by_area.insert(area, node_id);
    }

    pub fn node_is_expanded(&mut self, node_id: NodeId) -> bool {
        let node = self.node_ids_by_node_id.entry(node_id).or_default();
        node.is_expanded.value
    }

    pub fn set_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        node_id: NodeId,
        is_expanded: bool,
        should_animate: bool,
    ) -> bool {
        let node = self.node_ids_by_node_id.entry(node_id).or_default();
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

    pub fn toggle_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        node_id: NodeId,
        should_animate: bool,
    ) -> bool {
        let is_expanded = self.node_is_expanded(node_id);
        self.set_node_is_expanded(cx, node_id, !is_expanded, should_animate)
    }

    fn update_animating_nodes(&mut self, cx: &mut Cx, node_id: NodeId, is_animating: bool) {
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

    pub fn set_hovered_node_id(&mut self, node_id: Option<NodeId>) -> bool {
        if self.hovered_node_id == node_id {
            return false;
        }
        self.hovered_node_id = node_id;
        true
    }

    pub fn set_selected_node_id(&mut self, node_id: NodeId) -> bool {
        if self.selected_node_ids.len() == 1 && self.selected_node_ids.contains(&node_id) {
            return false;
        }
        self.selected_node_ids.clear();
        self.selected_node_ids.insert(node_id);
        true
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(Action),
    ) {
        match event {
            Event::NextFrame(_) if self.next_frame.is_active(cx) => {
                let mut new_animating_node_ids = HashSet::new();
                for node_id in &self.animating_node_ids {
                    let node = self.node_ids_by_node_id.get_mut(node_id).unwrap();
                    node.update();
                    if node.is_animating() {
                        new_animating_node_ids.insert(*node_id);
                    }
                }
                dispatch_action(Action::Redraw);
                self.animating_node_ids = new_animating_node_ids;
                self.update_next_frame(cx);
            }
            event => {
                for (area, node_id) in &self.node_ids_by_area {
                    match event.hits(cx, *area, HitOpt::default()) {
                        Event::FingerHover(fe) => {
                            cx.set_hover_mouse_cursor(MouseCursor::Hand);
                            match fe.hover_state {
                                HoverState::In => {
                                    dispatch_action(Action::SetHoveredNodeId(Some(*node_id)));
                                }
                                HoverState::Out => {
                                    dispatch_action(Action::SetHoveredNodeId(None));
                                }
                                _ => {}
                            }
                        }
                        Event::FingerDown(_) => {
                            dispatch_action(Action::ToggleNodeIsExpanded(*node_id, true));
                            dispatch_action(Action::SetSelectedNodeId(*node_id));
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
    pub is_expanded_fraction: f32,
    pub is_hovered: bool,
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
    SetHoveredNodeId(Option<NodeId>),
    SetSelectedNodeId(NodeId),
    Redraw,
}
