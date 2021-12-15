use makepad_render::*;
use std::collections::HashMap;
use std::any::TypeId;

live_register!{
    Frame: {{Frame}} {
    }
}

pub trait FrameComponentNew {
    fn new_component(&self, cx: &mut Cx) -> Box<dyn FrameComponent>;
}

pub trait FrameComponent: LiveApply {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> Option<Box<dyn FrameComponentAction >>;
    fn draw_dyn(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply_over(cx, nodes);
        self.draw_dyn(cx);
    }
}

#[derive(Clone)]
pub struct FrameActionItem {
    pub id: LiveId,
    pub action: Box<dyn FrameComponentAction>
}

#[derive(Clone, IntoFrameComponentAction)]
pub enum FrameActions {
    None,
    Actions(Vec<FrameActionItem>)
}

pub struct FrameItem { // draw info per UI element
    component: Box<dyn FrameComponent>
}

pub struct Frame { // draw info per UI element
    pub view: Option<View>,
    pub live_ptr: Option<LivePtr>,
    pub components: HashMap<LiveId, FrameItem>,
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
    
    fn register_factories(cx: &mut Cx) {
        struct Factory();
        impl FrameComponentNew for Factory {
            fn new_component(&self, cx: &mut Cx) -> Box<dyn FrameComponent> where Self: Sized {
                Box::new(Frame::new(cx))
            }
        }
        
        cx.registries.register_frame_component::<Self>(Box::new(Factory()));
    }
    
    fn live_type_info() -> LiveTypeInfo where Self: Sized + 'static {
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
            self.live_ptr = Some(LivePtr::from_index(file_id, start_index));
        }
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), apply_from, start_index, nodes, id!(Frame));
            return nodes.skip_node(start_index);
        }
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        if let ApplyFrom::ApplyClear = apply_from {
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
                                cx.apply_error_cant_find_target(live_error_origin!(), apply_from, index, nodes, id);
                            }
                            else {
                                self.children.push(id)
                            }
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(live_error_origin!(), apply_from, index, nodes);
                        }
                        node_iter = nodes.next_child(index);
                    }
                }
                id!(layout) => {
                }
                component_id => {
                    if !nodes[index].value.is_structy_type() {
                        cx.apply_error_no_matching_field(live_error_origin!(), apply_from, index, nodes);
                    }
                    else if let Some(item) = self.components.get_mut(&component_id) {
                        // exists
                        item.component.apply(cx, apply_from, index, nodes);
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
                                    self.create_component(
                                        cx,
                                        ApplyFrom::NewFromDoc {file_id: start_nodes[start_index].origin.token_id().unwrap().file_id()},
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
                                    cx.apply_error_wrong_type_for_value(live_error_origin!(), apply_from, index, nodes);
                                }
                            }
                            else {
                                cx.apply_error_cant_find_target(live_error_origin!(), apply_from, index, nodes, target_id);
                            }
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(live_error_origin!(), apply_from, index, nodes);
                        }
                    }
                    else { // apply or create component
                        if let LiveValue::Class {live_type, ..} = nodes[index].value {
                            self.create_component(cx, apply_from, component_id, live_type, index, nodes);
                            self.create_order.push(component_id);
                        }
                        else {
                            cx.apply_error_wrong_type_for_value(live_error_origin!(), apply_from, index, nodes);
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
    
    fn create_component(&mut self, cx: &mut Cx, apply_from: ApplyFrom, id: LiveId, live_type: LiveType, index: usize, nodes: &[LiveNode]) {
        if let Some(mut component) = CxRegistries::create_frame_component(cx, live_type) {
            component.apply(cx, apply_from, index, nodes);
            self.components.insert(id, FrameItem {component});
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> FrameActions {
        let mut actions = Vec::new();
        for id in if self.has_children_array {&self.children}else {&self.create_order} {
            let item = self.components.get_mut(id).unwrap();
            if let Some(action) = item.component.handle_event_dyn(cx, event) {
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
    
    pub fn draw(&mut self, cx: &mut Cx) {
        for id in if self.has_children_array {&self.children}else {&self.create_order} {
            let item = self.components.get_mut(id).unwrap();
            item.component.draw_dyn(cx)
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

pub trait FrameComponentAction: 'static {
    fn type_id(&self) -> TypeId;
    fn box_clone(&self) -> Box<dyn FrameComponentAction>;
}

impl<T: 'static + ? Sized + Clone> FrameComponentAction for T {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    
    fn box_clone(&self) -> Box<dyn FrameComponentAction> {
        Box::new((*self).clone())
    }
}

impl dyn FrameComponentAction {
    pub fn is<T: FrameComponentAction >(&self) -> bool {
        let t = TypeId::of::<T>();
        let concrete = self.type_id();
        t == concrete
    }
    pub fn cast<T: FrameComponentAction + Default + Clone>(&self) -> T {
        if self.is::<T>() {
            unsafe {&*(self as *const dyn FrameComponentAction as *const T)}.clone()
        } else {
            T::default()
        }
    }
    
    pub fn cast_id<T: FrameComponentAction + Default + Clone>(&self, id: LiveId) -> (LiveId, T) {
        if self.is::<T>() {
            (id, unsafe {&*(self as *const dyn FrameComponentAction as *const T)}.clone())
        } else {
            (id, T::default())
        }
    }
}

pub type OptionFrameComponentAction = Option<Box<dyn FrameComponentAction >>;

impl Clone for Box<dyn FrameComponentAction> {
    fn clone(&self) -> Box<dyn FrameComponentAction> {
        self.as_ref().box_clone()
    }
}

// The registry
pub fn add_frame_component_registry(cx: &mut Cx) {
    cx.registries.add_registry(
        TypeId::of::<Registry>(),
        Box::new(Registry {component_new: HashMap::new()})
    )
}

impl FrameComponent for Frame {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> OptionFrameComponentAction {
        self.handle_event(cx, event).into()
    }
    
    fn draw_dyn(&mut self, cx: &mut Cx) {
        self.draw(cx);
    }
}

impl FrameComponentNew for Frame {
    fn new_component(&self, cx: &mut Cx) -> Box<dyn FrameComponent> {
        Box::new(Self::new(cx))
    }
}


struct Registry {
    component_new: HashMap<TypeId, Box<dyn FrameComponentNew >>
}

pub trait CxRegistriesExt{
    fn register_frame_component<T: 'static>(&mut self, component: Box<dyn FrameComponentNew>);
    fn create_frame_component(cx:&mut Cx, live_type: LiveType) -> Option<Box<dyn FrameComponent >>;
}

impl CxRegistriesExt for CxRegistries {
    fn register_frame_component<T: 'static>(&mut self, component: Box<dyn FrameComponentNew>) {
        self.0.borrow_mut()
            .get_mut(&TypeId::of::<Registry>()).unwrap()
            .downcast_mut::<Registry>().unwrap()
            .component_new.insert(TypeId::of::<T>(), component);
    }
    
    fn create_frame_component(cx:&mut Cx, live_type: LiveType) -> Option<Box<dyn FrameComponent >> {
        let registries_cp = cx.registries.clone();
        let registries = registries_cp.0.borrow();
        registries
            .get(&TypeId::of::<Registry>()).unwrap()
            .downcast_ref::<Registry>().unwrap()
            .component_new.get(&live_type)
            .and_then(|cnew| Some(cnew.new_component(cx)) )
    }
}

