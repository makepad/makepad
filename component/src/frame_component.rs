use {
    std::any::TypeId,
    crate::makepad_platform::*,
    std::collections::BTreeMap,
};

pub trait FrameComponent: LiveApply {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> Option<Box<dyn FrameComponentAction >>;
    fn draw_component(&mut self, cx: &mut Cx2d);
    fn type_id(&self) -> LiveType where Self:'static {LiveType::of::<Self>()}
}

pub trait FrameComponentFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn FrameComponent>;
}

#[derive(Default, LiveComponentRegistry)]
pub struct FrameComponentRegistry {
    pub map: BTreeMap<LiveType, (LiveComponentInfo, Box<dyn FrameComponentFactory>)>
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

generate_clone_cast_api!(FrameComponentAction);

pub type OptionFrameComponentAction = Option<Box<dyn FrameComponentAction >>;

impl Clone for Box<dyn FrameComponentAction> {
    fn clone(&self) -> Box<dyn FrameComponentAction> {
        self.as_ref().box_clone()
    }
}

pub struct FrameComponentRef(Option<Box<dyn FrameComponent >>);

impl FrameComponentRef {
    pub fn _as_ref(&mut self) -> Option<&Box<dyn FrameComponent >> {
        self.0.as_ref()
    }
    pub fn as_mut(&mut self) -> Option<&mut Box<dyn FrameComponent >> {
        self.0.as_mut()
    }
}

impl LiveHook for FrameComponentRef {}
impl LiveApply for FrameComponentRef {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let LiveValue::Class {live_type, ..} = nodes[index].value {
            if let Some(component) = &mut self.0 {
                if component.type_id() != live_type {
                    self.0 = None; // type changed, drop old component
                }
                else {
                    return component.apply(cx, apply_from, index, nodes);
                }
            }
            if let Some(component) = cx.live_registry.clone().borrow()
                .components.get::<FrameComponentRegistry>().new(cx, live_type) {
                self.0 = Some(component);
                return self.0.as_mut().unwrap().apply(cx, apply_from, index, nodes);
            }
        }
        else if let Some(component) = &mut self.0 {
            return component.apply(cx, apply_from, index, nodes)
        }
        nodes.skip_node(index)
    }
}

impl LiveNew for FrameComponentRef {
    fn new(_cx: &mut Cx) -> Self {
        Self (None)
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<dyn FrameComponent>(),
            fields: Vec::new(),
            type_name: LiveId(0)
        }
    }
}

#[macro_export]
macro_rules!register_as_frame_component {
    ( $ ty: ty) => {
        | cx: &mut Cx | {
            struct Factory();
            impl FrameComponentFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn FrameComponent> {
                    Box::new( <$ ty>::new(cx))
                }
            }
            register_component_factory!(cx, FrameComponentRegistry, $ ty, Factory);
        }
    }
}

#[macro_export]
macro_rules!frame_component_impl {
    ( $ ty: ty) => {
        impl LiveHook for $ ty {
            fn before_apply(&mut self, _cx: &mut Cx, apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
                if let ApplyFrom::ApplyClear = apply_from {
                    self.create_order.clear();
                }
            }
            
            fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, _nodes: &[LiveNode]) {
                if let Some(file_id) = apply_from.file_id() {
                    self.live_ptr = Some(LivePtr::from_index(file_id, index, cx.live_registry.borrow().file_id_to_file(file_id).generation));
                }
            }
            
            fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
                if apply_from.is_from_doc() { //from doc
                    self.create_order.push(nodes[index].id);
                    return self.components.get_or_insert(cx, nodes[index].id, | cx | {FrameComponentRef::new(cx)})
                        .apply(cx, apply_from, index, nodes);
                }
                else {
                }
                nodes.skip_node(index)
            }
        }
        
        impl FrameComponent for $ ty {
            fn type_id(&self) -> LiveType where Self: 'static {LiveType::of::<Self>()}
            fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> OptionFrameComponentAction {
                self.handle_event(cx, event).into()
            }
            
            fn draw_component(&mut self, cx: &mut Cx2d) {
                self.draw(cx);
            }
        }
    }
}

#[macro_export]
macro_rules!frame_component_handle_event_impl {
    ( $ ty: ty) => {
        impl $ ty {
            pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> FrameActions {
                let mut actions = Vec::new();
                for id in &self.create_order {
                    if let Some(component) = self.components.get_mut(id).unwrap().as_mut() {
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
                }
                if actions.len()>0 {
                    FrameActions::Actions(actions)
                }
                else {
                    FrameActions::None
                }
            }
        }
    }
}
