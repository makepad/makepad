use {
    crate::{id::GenId, id_map::GenIdMap},
    makepad_render::*,
    std::collections::{HashMap, HashSet},
};

const MIN_DRAG_DISTANCE: f32 = 10.0;

#[derive(Default)]
pub struct TreeLogic {
    nodes_by_node_id: GenIdMap<NodeId, Node>,
    node_ids_by_area: HashMap<Area, NodeId>,
    animating_node_ids: HashSet<NodeId>,
    hovered_node_id: Option<NodeId>,
    selected_node_ids: HashSet<NodeId>,
    dragging_node_id: Option<NodeId>,
    next_frame: NextFrame,
}

impl TreeLogic {
    pub fn new() -> TreeLogic {
        TreeLogic::default()
    }

    pub fn begin(&mut self) {
        self.node_ids_by_area.clear();
    }

    pub fn end(&mut self) {}

    pub fn begin_node(&mut self, node_id: NodeId) -> NodeInfo {
        let node = self.get_or_create_node(node_id);
        NodeInfo {
            is_expanded_fraction: node.is_expanded.fraction,
            is_hovered: self
                .hovered_node_id
                .map_or(false, |hovered_node_id| hovered_node_id == node_id),
            is_selected: self.selected_node_ids.contains(&node_id),
        }
    }

    pub fn end_node(&mut self) {}

    fn get_or_create_node(&mut self, node_id: NodeId) -> &mut Node {
        if !self.nodes_by_node_id.contains(node_id) {
            self.nodes_by_node_id.insert(node_id, Node::default());
        }
        &mut self.nodes_by_node_id[node_id]
    }

    pub fn forget(&mut self) {
        self.nodes_by_node_id.clear();
        self.animating_node_ids.clear();
    }

    pub fn forget_node(&mut self, node_id: NodeId) {
        self.nodes_by_node_id.remove(node_id);
        self.animating_node_ids.remove(&node_id);
    }

    pub fn set_node_area(&mut self, cx: &mut Cx, node_id: NodeId, area: Area) {
        let node = &mut self.nodes_by_node_id[node_id];
        cx.update_area_refs(node.area, area);
        self.node_ids_by_area.insert(area, node_id);
        node.area = area;
    }

    pub fn node_is_expanded(&mut self, node_id: NodeId) -> bool {
        let node = self.get_or_create_node(node_id);
        node.is_expanded.value
    }

    pub fn set_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        node_id: NodeId,
        is_expanded: bool,
        should_animate: bool,
    ) -> bool {
        let node = self.get_or_create_node(node_id);
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

    pub fn hovered_node_id(&mut self) -> Option<NodeId> {
        self.hovered_node_id
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

    pub fn start_dragging_node(&mut self, cx: &mut Cx, node_id: NodeId, dragged_item: DraggedItem) {
        self.dragging_node_id = Some(node_id);
        cx.start_dragging(dragged_item);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(TreeAction),
    ) {
        match event {
            Event::NextFrame(_) if self.next_frame.is_active(cx) => {
                let mut new_animating_node_ids = HashSet::new();
                for node_id in &self.animating_node_ids {
                    let node = &mut self.nodes_by_node_id[*node_id];
                    node.update();
                    if node.is_animating() {
                        new_animating_node_ids.insert(*node_id);
                    }
                }
                dispatch_action(TreeAction::TreeWasAnimated);
                self.animating_node_ids = new_animating_node_ids;
                self.update_next_frame(cx);
            }
            Event::DragEnd => self.dragging_node_id = None,
            event => {
                for (area, node_id) in &self.node_ids_by_area {
                    match event.hits(cx, *area, HitOpt::default()) {
                        Event::FingerHover(event) => {
                            cx.set_hover_mouse_cursor(MouseCursor::Hand);
                            match event.hover_state {
                                HoverState::In => {
                                    dispatch_action(TreeAction::NodeWasEntered(*node_id));
                                }
                                HoverState::Out => {
                                    dispatch_action(TreeAction::NodeWasExited(*node_id));
                                }
                                _ => {}
                            }
                        }
                        Event::FingerMove(event) => {
                            if self.dragging_node_id.is_none()
                                && event.abs.distance(&event.abs_start) >= MIN_DRAG_DISTANCE
                            {
                                dispatch_action(TreeAction::NodeShouldStartDragging(*node_id));
                            }
                        }
                        Event::FingerUp(event) => {
                            if area.get_rect(cx).contains(event.abs_start) {
                                dispatch_action(TreeAction::NodeWasClicked(*node_id));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId(pub GenId);

impl AsRef<GenId> for NodeId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}

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
    area: Area,
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
            is_expanded: AnimatedBool::new(false),
            area: Area::Empty,
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

pub enum TreeAction {
    TreeWasAnimated,
    NodeWasEntered(NodeId),
    NodeWasExited(NodeId),
    NodeWasClicked(NodeId),
    NodeShouldStartDragging(NodeId),
}
