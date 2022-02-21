use {
    crate::{
        makepad_platform::*,
        component_map::*,
        frame_component::*
    }
};

live_register!{
    Frame: {{Frame}} {
        color: #0000
    }
}

// ClipFrame
// ScrollFrame
// Frame

#[derive(Live)]
#[live_register(register_as_frame_component!(Frame))]
pub struct Frame { // draw info per UI element
    bg_quad: DrawColor,
    layout: Layout2,
    pub walk: Walk2,
    #[rust] live_ptr: Option<LivePtr>,
    #[rust] children: ComponentMap<LiveId, FrameComponentRef>,
    #[rust] create_order: Vec<LiveId>
}

impl LiveHook for Frame {
    fn before_apply(&mut self, _cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) -> Option<usize> {
        if let ApplyFrom::ApplyClear = from {
            self.create_order.clear();
        }
        None
    }
    
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, _nodes: &[LiveNode]) {
        if let Some(file_id) = from.file_id() {
            self.live_ptr = Some(LivePtr::from_index(file_id, index, cx.live_registry.borrow().file_id_to_file(file_id).generation));
        }
    }
    
    fn apply_value_unknown(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match nodes[index].id {
            id!(color) => self.bg_quad.color.apply(cx, from, index, nodes),
            id!(width) => self.walk.width.apply(cx, from, index, nodes),
            id!(height) => self.walk.height.apply(cx, from, index, nodes),
            id!(margin) => self.walk.margin.apply(cx, from, index, nodes),
            id!(padding) => self.layout.padding.apply(cx, from, index, nodes),
            id!(spacing) => self.layout.spacing.apply(cx, from, index, nodes),
            id => {
                if id.is_capitalised(){
                    self.create_order.push(nodes[index].id);
                    return self.children.get_or_insert(cx, nodes[index].id, | cx | {FrameComponentRef::new(cx)})
                        .apply(cx, from, index, nodes);
                }
                else{
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    nodes.skip_node(index)
                }
            }
        }
    }
}

impl FrameComponent for Frame {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> OptionFrameComponentAction {
        self.handle_event(cx, event).into()
    }

    fn get_walk(&self)->Walk2{
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2da, walk:Walk2) {
        self.draw(cx, walk);
    }
}

impl Frame {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> FrameActions {
        let mut actions = Vec::new();
        for id in &self.create_order {
            if let Some(child) = self.children.get_mut(id).unwrap().as_mut() {
                if let Some(action) = child.handle_component_event(cx, event) {
                    if let FrameActions::Actions(other_actions) = action.cast() {
                        actions.extend(other_actions);
                    }
                    else {
                        actions.push(FrameActionItem {
                            id: *id,
                            action: action
                        });
                    }
                }
            }
        }
        if actions.len()>0 {
            FrameActions::Actions(actions)
        }
        else {
            FrameActions::None
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2da, walk:Walk2) {
        let has_bg = self.bg_quad.color.w > 0.0;
        if has_bg{
            self.bg_quad.begin2(cx, walk, self.layout);
        }
        else{
            cx.begin_turtle(walk, self.layout);
        }
        
        // lets make a defer list for fill items
        let mut fills = Vec::new();
        for id in &self.create_order {
            if let Some(child) = self.children.get_mut(id).unwrap().as_mut() {
                let walk = child.get_walk();
                if let Some(fw) = cx.fill_walk(walk){
                    fills.push((id, fw));
                }
                else{
                    child.draw_component(cx, walk);
                }
            }
        }
        
        // the fill-items
        for (id, fw) in fills{
            if let Some(child) = self.children.get_mut(id).unwrap().as_mut() {
                let walk = cx.resolve_fill(fw);
                child.draw_component(cx, walk);
            }
        }
        
        if has_bg{
            self.bg_quad.end2(cx);
        }
        else{
            cx.end_turtle();
        }
    }
}
