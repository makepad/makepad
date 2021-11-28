use makepad_render::*;
use std::collections::HashMap;

live_register!{
    Frame: {{Frame}} {
    }
}

#[derive(Clone)]
pub struct FrameActionItem {
    pub id: LiveId,
    pub action: Box<dyn AnyAction>
}

#[derive(Clone, IntoAnyAction)]
pub enum FrameActions {
    None,
    Actions(Vec<FrameActionItem>)
}

pub struct FrameItem { // draw info per UI element
    component: Box<dyn LiveApply>
}

pub struct Frame { // draw info per UI element
    pub view: Option<View>,
    pub live_ptr: Option<LivePtr>,
    pub components: HashMap<LiveId, FrameItem>,
    pub has_children_array: bool,
    pub children: Vec<LiveId>,
    pub create_order: Vec<LiveId>
}

impl LiveHook for Frame {
    fn to_frame_component(&mut self) -> Option<&mut dyn FrameComponent> {
        return Some(self);
    }
}

impl FrameComponent for Frame {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> OptionAnyAction {
        self.handle_event(cx, event).into()
    }
    
    fn draw_dyn(&mut self, cx: &mut Cx) {
        self.draw(cx);
    }
}

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
        struct Factory();
        impl LiveFactory for Factory {
            fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveApply> where Self: Sized {
                Box::new(Frame::new(cx))
            }
        }
        cx.register_factory(Self::live_type(), Box::new(Factory()));
    }
    
    fn live_type_info() -> LiveTypeInfo where Self: Sized + 'static {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: Self::live_type(),
            fields: Vec::new(),
            type_name: LiveId::from_str("Frame").unwrap(),
            kind: LiveTypeKind::Class
        }
    }
    
}

impl Frame {
    fn create_component(&mut self, cx: &mut Cx, apply_from: ApplyFrom, id: LiveId, live_type: LiveType, index: usize, nodes: &[LiveNode]) {
        let factories = cx.live_factories.clone();
        let factories_cp = factories.borrow();
        if let Some(factory) = factories_cp.get(&live_type) {
            let mut component = factory.new_component(cx);
            component.apply(cx, apply_from, index, nodes);
            self.components.insert(id, FrameItem {component});
        }
    }
}

impl LiveApply for Frame {
    fn type_id(&self) -> std::any::TypeId {std::any::TypeId::of::<Self>()}
    
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if let Some(file_id) = apply_from.file_id() {
            self.live_ptr = Some(LivePtr::from_index(file_id, start_index));
        }
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(apply_from, start_index, nodes, id!(Frame));
            return nodes.skip_node(start_index);
        }
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        if let ApplyFrom::ApplyClear = apply_from{
            self.create_order.truncate(0);
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
                    self.children.truncate(0);
                    let mut node_iter = nodes.first_child(index);
                    while let Some(index) = node_iter {
                        if let LiveValue::Id(id) = nodes[index].value {
                            if self.components.get(&id).is_none() {
                                cx.apply_error_cant_find_target(apply_from, index, nodes, id);
                            }
                            else {
                                self.children.push(id)
                            }
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                        }
                        node_iter = nodes.next_child(index);
                    }
                }
                id!(layout) => {
                }
                component_id => {
                    if !nodes[index].value.is_structy_type() {
                        cx.apply_error_no_matching_field(apply_from, index, nodes);
                    }
                    else if let Some(item) = self.components.get_mut(&component_id) {
                        // exists
                        item.component.apply(cx, apply_from, index, nodes);
                        if let ApplyFrom::ApplyClear = apply_from{
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
                                    self.create_component(
                                        cx,
                                        ApplyFrom::NewFromDoc {file_id: start_nodes[start_index].token_id.unwrap().file_id()},
                                        component_id,
                                        live_type,
                                        start_index,
                                        start_nodes
                                    );
                                    //cx.profile_end(0);
                                    // then apply our local data over it.
                                    self.components.get_mut(&component_id).as_mut().unwrap().component.apply(cx, apply_from, index, nodes);
                                    self.create_order.push(component_id);
                                }
                                else {
                                    cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                                }
                            }
                            else {
                                cx.apply_error_cant_find_target(apply_from, index, nodes, target_id);
                            }
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                        }
                    }
                    else { // apply or create component
                        if let LiveValue::Class {live_type, ..} = nodes[index].value {
                            self.create_component(cx, apply_from, component_id, live_type, index, nodes);
                            self.create_order.push(component_id);
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                        }
                    }
                }
            }
            index = nodes.skip_node(index);
        }
        return index;
    }
}


impl Frame {
    pub fn get_component(&mut self, id: LiveId) -> Option<&mut Box<dyn LiveApply >> {
        if let Some(comp) = self.components.get_mut(&id) {
            return Some(&mut comp.component)
        }
        else {
            None
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> FrameActions {
        let mut actions = Vec::new();
        for id in if self.has_children_array {&self.children}else {&self.create_order} {
            let item = self.components.get_mut(id).unwrap();
            if let Some(fc) = item.component.to_frame_component() {
                if let Some(action) = fc.handle_event_dyn(cx, event) {
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
    
    pub fn draw(&mut self, cx: &mut Cx) {
        for id in if self.has_children_array {&self.children}else {&self.create_order} {
            let item = self.components.get_mut(id).unwrap();
            if let Some(fc) = item.component.to_frame_component() {
                fc.draw_dyn(cx)
            }
        }
    }
}

impl Default for FrameActions {
    fn default() -> Self {Self::None}
}

pub struct FrameActionsIterator {
    iter: Option<std::vec::IntoIter<FrameActionItem >>
}

impl Iterator for FrameActionsIterator {
    type Item = FrameActionItem;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(iter) = self.iter.as_mut() {
            return iter.next()
        }
        else {
            None
        }
    }
}

// and we'll implement IntoIterator
impl IntoIterator for FrameActions {
    type Item = FrameActionItem;
    type IntoIter = FrameActionsIterator;
    
    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::None => FrameActionsIterator {iter: None},
            Self::Actions(actions) => FrameActionsIterator {iter: Some(actions.into_iter())},
        }
    }
}