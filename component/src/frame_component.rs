use {
    std::ops::{ControlFlow, Try, FromResidual},
    std::any::TypeId,
    crate::makepad_platform::*,
    std::collections::BTreeMap,
};
pub use crate::frame_component;


pub trait FrameComponent: LiveApply {
    // to implement
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id: LiveId) -> FrameComponentActionRef;
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId>;
    fn get_walk(&self) -> Walk;
 
    // defaults
    fn redraw(&mut self, _cx:&mut Cx){}
    fn draw_walk_component(&mut self, cx: &mut Cx2d) -> Result<(), LiveId>{self.draw_component(cx, self.get_walk())}

    fn create_child(&mut self, _cx:&mut Cx, _at:CreateAt,  _id:LiveId, _path: &[LiveId], _nodes:&[LiveNode]) -> ChildResult {
        NoChild
    }

    fn add_child(&mut self, cx:&mut Cx, id:LiveId, path: &[LiveId], nodes:&[LiveNode]) -> ChildResult {
        self.create_child(cx, CreateAt::End, id, path, nodes)?;
        NoChild
    }
    
    fn find_child(&mut self, _id: &[LiveId]) -> ChildResult {NoChild}

    fn apply_child(&mut self, cx:&mut Cx, id: &[LiveId], nodes:&[LiveNode]) {
        if let Child(child) = self.find_child(id){
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


#[derive(Clone)]
pub struct FrameActionItem {
    pub id: LiveId,
    pub parent: [LiveId; 4],
    pub action: Box<dyn FrameComponentAction>
}

pub trait FrameActionItemVec {
    fn merge(&mut self, id: LiveId, action: FrameComponentActionRef);
}

impl FrameActionItemVec for Vec<FrameActionItem> {
    fn merge(&mut self, id: LiveId, action: FrameComponentActionRef) {
        if let Some(action) = action {
            if let FrameActions::Actions(other_actions) = action.cast() {
                for action in other_actions {
                    self.push(action.with_parent_id(id));
                }
            }
            else {
                self.push(FrameActionItem::new(id, Some(action)));
            }
        }
    }
}

impl FrameActionItem {
    pub fn new(id: LiveId, action: FrameComponentActionRef) -> Self {
        Self {
            id,
            parent: [LiveId(0); 4],
            action: action.unwrap()
        }
    }
    
    pub fn with_parent_id(mut self, id: LiveId) -> Self {
        for i in 0..self.parent.len() {
            if self.parent[i] == LiveId(0) {
                self.parent[i] = id;
                break;
            }
        }
        self
    }
}

#[derive(Clone, FrameComponentAction)]
pub enum FrameActions {
    None,
    Actions(Vec<FrameActionItem>)
}

impl FrameActions {
    pub fn from_vec(actions: Vec<FrameActionItem>) -> Self {
        if actions.len()>0 {
            FrameActions::Actions(actions)
        }
        else {
            FrameActions::None
        }
    }
    
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

pub type FrameComponentActionRef = Option<Box<dyn FrameComponentAction >>;

impl Clone for Box<dyn FrameComponentAction> {
    fn clone(&self) -> Box<dyn FrameComponentAction> {
        self.as_ref().box_clone()
    }
}

pub struct FrameComponentRef(Option<Box<dyn FrameComponent >>);

impl FrameComponentRef {
    pub fn empty()->Self{Self(None)}
    
    pub fn as_ref(&self) -> Option<&Box<dyn FrameComponent >> {
        self.0.as_ref()
    }
    pub fn as_mut(&mut self) -> Option<&mut Box<dyn FrameComponent >> {
        self.0.as_mut()
    }

    pub fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id: LiveId) -> FrameComponentActionRef{
        if let Some(inner) = &mut self.0{
            return inner.handle_component_event(cx, event, self_id)
        }
        None
    }
    
    pub fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId>{
        if let Some(inner) = &mut self.0{
            return inner.draw_component(cx, walk)
        }
        Ok(())
    }
    
    pub fn get_walk(&mut self) -> Walk{
        if let Some(inner) = &mut self.0{
            return inner.get_walk()
        }
        Walk::default()
    }

    // forwarding FrameComponent trait
    pub fn redraw(&mut self, cx:&mut Cx){
        if let Some(inner) = &mut self.0{
            return inner.redraw(cx)
        }
    }
    
    pub fn draw_walk_component(&mut self, cx: &mut Cx2d) -> Result<(), LiveId>{
        if let Some(inner) = &mut self.0{
            return inner.draw_walk_component(cx)
        }
        Ok(())
    }

    pub fn create_child(&mut self, cx:&mut Cx, at:CreateAt,  id:LiveId, path: &[LiveId], nodes:&[LiveNode]) -> ChildResult {
        if let Some(inner) = &mut self.0{
            return inner.create_child(cx, at, id, path, nodes)
        }
        NoChild
    }

    pub fn add_child(&mut self, cx:&mut Cx, id:LiveId, path: &[LiveId], nodes:&[LiveNode]) -> ChildResult {
        if let Some(inner) = &mut self.0{
            return inner.add_child(cx, id, path, nodes)
        }
        NoChild
    }
    
    pub fn find_child(&mut self, id: &[LiveId]) -> ChildResult {
        if let Some(inner) = &mut self.0{
            return inner.find_child(id)
        }
        NoChild
    }

    pub fn apply_child(&mut self, cx:&mut Cx, id: &[LiveId], nodes:&[LiveNode]) {
        if let Some(inner) = &mut self.0{
            return inner.apply_child(cx, id, nodes)
        }
    }
}

impl LiveHook for FrameComponentRef {}
impl LiveApply for FrameComponentRef {
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

pub enum ChildResult<'a>{
    NoChild,
    Child(&'a mut Box<dyn FrameComponent >)
}
pub use ChildResult::*;

impl<'a> FromResidual for ChildResult<'a> {
    fn from_residual(residual:&'a mut Box<dyn FrameComponent>) -> Self{
        ChildResult::Child(residual)
    }
}

impl<'a> Try for ChildResult<'a>{
    type Output = ();
    type Residual = &'a mut Box<dyn FrameComponent >;

    fn from_output(_: Self::Output) -> Self{
        ChildResult::NoChild
    }
    
    fn branch(self) -> ControlFlow<Self::Residual, Self::Output>{
        match self{
            Self::NoChild=>ControlFlow::Continue(()),
            Self::Child(c)=>ControlFlow::Break(c)
        }
    }
}

#[derive(Clone,Copy)]
pub enum CreateAt{
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
