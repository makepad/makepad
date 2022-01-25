use {
    std::collections::HashMap,
    crate::{
        makepad_platform::*,
        frame_registry::*
    }
};

live_register!{
    Frame: {{Frame}} {
    }
}

pub struct Frame { // draw info per UI element
    pub view: Option<View>,
    pub live_ptr: Option<LivePtr>,
    pub components: HashMap<LiveId, Box<dyn FrameComponent >>,
    pub has_children_array: bool,
    pub children: Vec<LiveId>,
    pub create_order: Vec<LiveId>
}

impl LiveHook for Frame {}
impl LiveNew for Frame {
    fn new(_cx: &mut Cx) -> Self {
        Self {
            live_ptr: None,
            view: None,
            components: HashMap::new(),
            has_children_array: false,
            children: Vec::new(),
            create_order: Vec::new()
        }
    }
    
    fn live_register(cx: &mut Cx) {
        register_as_frame_component!(cx, Frame);
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo where Self: Sized + 'static {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            type_name: LiveId::from_str("Frame").unwrap(),
            // kind: LiveTypeKind::Class
        }
    }
}

impl LiveApply for Frame {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if let Some(file_id) = apply_from.file_id() {
            self.live_ptr = Some(LivePtr::from_index(file_id, start_index, cx.live_registry.borrow().file_id_to_file(file_id).generation));
        }
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, id!(Frame));
            return nodes.skip_node(start_index);
        }
        
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        if let ApplyFrom::ApplyClear = apply_from {
            self.create_order.clear();
        }
        let mut index = start_index + 1;
        while index<nodes.len() {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                id!(children) => {
                    self.has_children_array = true;
                    self.children.clear();
                    let mut node_iter = nodes.first_child(index);
                    while let Some(index) = node_iter {
                        if let LiveValue::Id(id) = nodes[index].value {
                            if self.components.get(&id).is_none() {
                                cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, id);
                            }
                            else {
                                self.children.push(id)
                            }
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(live_error_origin!(), index, nodes);
                        }
                        node_iter = nodes.next_child(index);
                    }
                }
                id!(layout) => {
                }
                component_id => {
                    if !nodes[index].value.is_structy_type() {
                        cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    }
                    else if let Some(component) = self.components.get_mut(&component_id) {
                        // exists
                        component.apply(cx, apply_from, index, nodes);
                        if let ApplyFrom::ApplyClear = apply_from {
                            self.create_order.push(component_id);
                        }
                    }
                    else if !apply_from.is_from_doc() { // not from doc. and doesnt exist.
                        if let LiveValue::Clone(target_id) = nodes[index].value {
                            // ok now we need to find
                            if let Some((start_nodes, start_index)) = live_registry.find_scope_item_via_class_parent(self.live_ptr.unwrap(), target_id) {
                                if let LiveValue::Class {live_type, ..} = start_nodes[start_index].value {
                                    // first spawn the component from the target
                                    //cx.profile_start(0);
                                    self.new_component(
                                        cx,
                                        ApplyFrom::NewFromDoc {file_id: start_nodes[start_index].origin.token_id().unwrap().file_id()},
                                        component_id,
                                        live_type,
                                        start_index,
                                        start_nodes
                                    );
                                    //cx.profile_end(0);
                                    // then apply our local data over it.
                                    self.components.get_mut(&component_id).as_mut().unwrap().apply(cx, apply_from, index, nodes);
                                    self.create_order.push(component_id);
                                }
                                else {
                                    cx.apply_error_wrong_type_for_value(live_error_origin!(), index, nodes);
                                }
                            }
                            else {
                                cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, target_id);
                            }
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(live_error_origin!(), index, nodes);
                        }
                    }
                    else { // apply or create component
                        if let LiveValue::Class {live_type, ..} = nodes[index].value {
                            self.new_component(cx, apply_from, component_id, live_type, index, nodes);
                            self.create_order.push(component_id);
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(live_error_origin!(), index, nodes);
                        }
                    }
                }
            }
            index = nodes.skip_node(index);
        }
        return index;
    }
}

impl FrameComponent for Frame {
    fn type_id(&self)->LiveType{LiveType::of::<Self>()}
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> OptionFrameComponentAction {
        self.handle_event(cx, event).into()
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d) {
        self.draw(cx);
    }
}

impl Frame {
    
    fn new_component(&mut self, cx: &mut Cx, apply_from: ApplyFrom, id: LiveId, live_type: LiveType, index: usize, nodes: &[LiveNode]) {
        if let Some(mut component) = cx.registries.clone().get::<CxFrameComponentRegistry>().new(cx, live_type) {
            component.apply(cx, apply_from, index, nodes);
            self.components.insert(id, component);
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> FrameActions {
        let mut actions = Vec::new();
        for id in if self.has_children_array {&self.children}else {&self.create_order} {
            let component = self.components.get_mut(id).unwrap();
            if let Some(action) = component.handle_component_event(cx, event) {
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
        if actions.len()>0 {
            FrameActions::Actions(actions)
        }
        else {
            FrameActions::None
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        for id in if self.has_children_array {&self.children}else {&self.create_order} {
            let component = self.components.get_mut(id).unwrap();
            component.draw_component(cx)
        }
    }
}
