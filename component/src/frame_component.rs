use {
    std::ops::{ControlFlow, Try, FromResidual},
    std::any::TypeId,
    crate::makepad_platform::*,
    std::collections::BTreeMap,
};
pub use crate::frame_component;

#[derive(Clone, Copy)]
pub struct FrameUid(u64);

#[derive(Default)]
pub struct FramePath {
    pub uids: Vec<FrameUid>,
    pub ids: Vec<LiveId>
}

impl FramePath {
    pub fn empty()->Self{
        Self::default()
    }
    pub fn add(mut self, id: LiveId, uid: FrameUid) -> Self {
        self.uids.push(uid);
        self.ids.push(id);
        Self {
            uids: self.uids,
            ids: self.ids
        }
    }
}

pub struct FrameActionItem{
    pub path: FramePath,
    pub action: Box<dyn FrameAction>
}


impl FrameActionItem{
    pub fn id(&self)->LiveId{
        self.path.ids[0]
    }
}

pub trait FrameComponent: LiveApply {
    fn handle_component_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        fn_action: &mut dyn FnMut(&mut Cx, FramePath, Box<dyn FrameAction>)
    ){}
    
    fn handle_component_event_vec(&mut self, cx: &mut Cx, event: &mut Event) -> Vec<FrameActionItem> {
        let mut actions = Vec::new();
        self.handle_component_event(cx, event, &mut |_, path, action|{
            actions.push(FrameActionItem{
                action: action,
                path
            });
        });
        actions
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId>;
    fn get_walk(&self) -> Walk;
    
    // defaults
    fn redraw(&mut self, _cx: &mut Cx) {}
    
    fn draw_walk_component(&mut self, cx: &mut Cx2d) -> Result<(), LiveId> {
        self.draw_component(cx, self.get_walk())
    }
    
    fn create_child(&mut self, _cx: &mut Cx, _at: CreateAt, _id: LiveId, _path: &[LiveId], _nodes: &[LiveNode]) -> ChildResult {
        NoChild
    }
    
    fn add_child(&mut self, cx: &mut Cx, id: LiveId, path: &[LiveId], nodes: &[LiveNode]) -> ChildResult {
        self.create_child(cx, CreateAt::End, id, path, nodes) ?;
        NoChild
    }
    
    fn find_child(&mut self, _id: &[LiveId]) -> ChildResult {
        NoChild
    }
    
    fn apply_child(&mut self, cx: &mut Cx, id: &[LiveId], nodes: &[LiveNode]) {
        if let Child(child) = self.find_child(id) {
            child.apply(cx, ApplyFrom::ApplyOver, 0, nodes);
        }
    }
    
    fn type_id(&self) -> LiveType where Self: 'static {LiveType::of::<Self>()}
}

generate_ref_cast_api!(FrameComponent);

pub trait FrameComponentFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn FrameComponent>;
}

#[derive(Default, LiveComponentRegistry)]
pub struct FrameComponentRegistry {
    pub map: BTreeMap<LiveType, (LiveComponentInfo, Box<dyn FrameComponentFactory>)>
}

pub trait FrameAction: 'static {
    fn type_id(&self) -> TypeId;
    fn box_clone(&self) -> Box<dyn FrameAction>;
}

impl<T: 'static + ? Sized + Clone> FrameAction for T {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    
    fn box_clone(&self) -> Box<dyn FrameAction> {
        Box::new((*self).clone())
    }
}

generate_clone_cast_api!(FrameAction);

impl Clone for Box<dyn FrameAction> {
    fn clone(&self) -> Box<dyn FrameAction> {
        self.as_ref().box_clone()
    }
}

pub struct FrameRef(Option<Box<dyn FrameComponent >>);

impl FrameRef {
    pub fn empty() -> Self {Self (None)}
    
    pub fn as_uid(&self) -> FrameUid {
        if let Some(ptr) = &self.0 {
            FrameUid(&*ptr as *const _ as u64)
        }
        else {
            FrameUid(0)
        }
    }
    
    pub fn as_ref(&self) -> Option<&Box<dyn FrameComponent >> {
        self.0.as_ref()
    }
    pub fn as_mut(&mut self) -> Option<&mut Box<dyn FrameComponent >> {
        self.0.as_mut()
    }
    
    pub fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, FramePath, Box<dyn FrameAction>)) {
        if let Some(inner) = &mut self.0 {
            return inner.handle_component_event(cx, event, dispatch_action)
        }
    }
    
    pub fn handle_component_event_vec(&mut self, cx: &mut Cx, event: &mut Event) -> Vec<FrameActionItem> {
        if let Some(inner) = &mut self.0 {
            return inner.handle_component_event_vec(cx, event)
        }
        Vec::new()
    }
    
    pub fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId> {
        if let Some(inner) = &mut self.0 {
            return inner.draw_component(cx, walk)
        }
        Ok(())
    }
    
    pub fn get_walk(&mut self) -> Walk {
        if let Some(inner) = &mut self.0 {
            return inner.get_walk()
        }
        Walk::default()
    }
    
    // forwarding FrameComponent trait
    pub fn redraw(&mut self, cx: &mut Cx) {
        if let Some(inner) = &mut self.0 {
            return inner.redraw(cx)
        }
    }
    
    pub fn draw_walk_component(&mut self, cx: &mut Cx2d) -> Result<(), LiveId> {
        if let Some(inner) = &mut self.0 {
            return inner.draw_walk_component(cx)
        }
        Ok(())
    }
    
    pub fn create_child(&mut self, cx: &mut Cx, at: CreateAt, id: LiveId, path: &[LiveId], nodes: &[LiveNode]) -> ChildResult {
        if let Some(inner) = &mut self.0 {
            return inner.create_child(cx, at, id, path, nodes)
        }
        NoChild
    }
    
    pub fn add_child(&mut self, cx: &mut Cx, id: LiveId, path: &[LiveId], nodes: &[LiveNode]) -> ChildResult {
        if let Some(inner) = &mut self.0 {
            return inner.add_child(cx, id, path, nodes)
        }
        NoChild
    }
    
    pub fn find_child(&mut self, id: &[LiveId]) -> ChildResult {
        if let Some(inner) = &mut self.0 {
            return inner.find_child(id)
        }
        NoChild
    }
    
    pub fn apply_child(&mut self, cx: &mut Cx, id: &[LiveId], nodes: &[LiveNode]) {
        if let Some(inner) = &mut self.0 {
            return inner.apply_child(cx, id, nodes)
        }
    }
}

impl LiveHook for FrameRef {}
impl LiveApply for FrameRef {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let LiveValue::Class {live_type, ..} = nodes[index].value {
            if let Some(component) = &mut self.0 {
                if component.type_id() != live_type {
                    self.0 = None; // type changed, drop old component
                }
                else {
                    return component.apply(cx, from, index, nodes);
                }
            }
            if let Some(component) = cx.live_registry.clone().borrow()
                .components.get::<FrameComponentRegistry>().new(cx, live_type) {
                self.0 = Some(component);
                return self.0.as_mut().unwrap().apply(cx, from, index, nodes);
            }
        }
        else if let Some(component) = &mut self.0 {
            return component.apply(cx, from, index, nodes)
        }
        nodes.skip_node(index)
    }
}

impl LiveNew for FrameRef {
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

pub struct DrawStateWrap<T: Clone> {
    state: Option<T>,
    redraw_id: u64
}

impl<T: Clone> Default for DrawStateWrap<T> {
    fn default() -> Self {
        Self {
            state: None,
            redraw_id: 0
        }
    }
}

impl<T: Clone> DrawStateWrap<T> {
    pub fn begin(&mut self, cx: &Cx2d, init: T) -> bool {
        if self.redraw_id != cx.redraw_id {
            self.redraw_id = cx.redraw_id;
            self.state = Some(init);
            true
        }
        else {
            false
        }
    }
    
    pub fn get(&self) -> T {
        self.state.clone().unwrap()
    }
    
    pub fn set(&mut self, value: T) {
        self.state = Some(value);
    }
    
    pub fn end(&mut self) {
        self.state = None;
    }
}

pub enum ChildResult<'a> {
    NoChild,
    Child(&'a mut Box<dyn FrameComponent >)
}
pub use ChildResult::*;

impl<'a> FromResidual for ChildResult<'a> {
    fn from_residual(residual: &'a mut Box<dyn FrameComponent>) -> Self {
        ChildResult::Child(residual)
    }
}

impl<'a> Try for ChildResult<'a> {
    type Output = ();
    type Residual = &'a mut Box<dyn FrameComponent >;
    
    fn from_output(_: Self::Output) -> Self {
        ChildResult::NoChild
    }
    
    fn branch(self) -> ControlFlow<Self::Residual,
    Self::Output> {
        match self {
            Self::NoChild => ControlFlow::Continue(()),
            Self::Child(c) => ControlFlow::Break(c)
        }
    }
}

#[derive(Clone, Copy)]
pub enum CreateAt {
    Begin,
    After(LiveId),
    Before(LiveId),
    End
}

#[macro_export]
macro_rules!frame_component {
    ( $ ty: ty) => {
        | cx: &mut Cx | {
            struct Factory();
            impl FrameComponentFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn FrameComponent> {
                    Box::new(< $ ty>::new(cx))
                }
            }
            register_component_factory!(cx, FrameComponentRegistry, $ ty, Factory);
        }
    }
}
