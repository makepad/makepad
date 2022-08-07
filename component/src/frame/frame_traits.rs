use {
    crate::makepad_draw_2d::*,
    std::collections::BTreeMap,
    std::any::TypeId
};
pub use crate::frame_component;

pub trait FrameComponent: LiveApply {
    fn handle_component_event(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _fn_action: &mut dyn FnMut(&mut Cx, FrameActionItem)
    ) {}
    
    fn handle_component_event_iter(&mut self, cx: &mut Cx, event: &Event) -> Vec<FrameActionItem> {
        let mut actions = Vec::new();
        self.handle_component_event(cx, event, &mut | _, action | {
            actions.push(action);
        });
        actions
    }
    
    fn frame_query(
        &mut self,
        _query: &FrameQuery,
        _callback: &mut Option<FrameQueryCb>
    ) -> FrameResult {
        return FrameResult::not_found()
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, self_uid: FrameUid) -> FrameDraw;
    fn get_walk(&self) -> Walk;
    
    // defaults
    fn redraw(&mut self, _cx: &mut Cx) {}
    
    fn draw_walk_component(&mut self, cx: &mut Cx2d, self_uid: FrameUid) -> FrameDraw {
        self.draw_component(cx, self.get_walk(), self_uid)
    }
    
    fn create_child(
        &mut self,
        _cx: &mut Cx,
        _live_ptr: LivePtr,
        _at: CreateAt,
        _new_id: LiveId,
        _nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn FrameComponent >> {
        None
    }
    
    fn template(
        &mut self,
        cx: &mut Cx,
        path: &[LiveId],
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn FrameComponent >> {
        // first we query the template
        if path.len() == 1{
            if let Some(live_ptr) = self.query_template(path[0]){
                return self.create_child(cx, live_ptr, CreateAt::Template, new_id, nodes)
            }
        }
        if let Some(FrameFound::Template(child, live_ptr)) = 
        self.frame_query(&FrameQuery::Path(path), &mut None).into_found() {
            child.create_child(cx, live_ptr, CreateAt::Template, new_id, nodes)
        }
        else {
            None
        }
    }
    
    fn query_template(&self, _id: LiveId) -> Option<LivePtr> {
        None
    }
    
    fn bind_read(&mut self, _cx:&mut Cx, _nodes:&[LiveNode]){
    }
    
    fn type_id(&self) -> LiveType where Self: 'static {LiveType::of::<Self>()}
}

pub struct FrameQueryCb<'a>{
    pub cx:&'a mut Cx,
    pub cb:&'a mut dyn FnMut(&mut Cx, FrameFound)
}

impl<'a> FrameQueryCb<'a>{
    pub fn call(&mut self, args:FrameFound){
        let cb = &mut self.cb;
        cb(self.cx, args)
    }
}

#[derive(Clone, Copy)]
pub enum CreateAt {
    Template,
    Begin,
    After(LiveId),
    Before(LiveId),
    End
}

pub enum FrameQuery<'a> {
    All,
    TypeId(std::any::TypeId),
    Path(&'a [LiveId]),
    Uid(FrameUid),
}

pub enum FrameFound<'a> {
    Child(&'a mut Box<dyn FrameComponent >),
    Template(&'a mut Box<dyn FrameComponent >, LivePtr)
}

pub type FrameResult<'a> = Result<(),FrameFound<'a>>;

pub trait FrameResultApi<'a>{
    fn child(value: &'a mut Box<dyn FrameComponent >) -> FrameResult<'a> {
       Result::Err(FrameFound::Child(value))
    }
    fn template(value: &'a mut Box<dyn FrameComponent >, live_ptr: LivePtr) -> FrameResult<'a> {
        Result::Err(FrameFound::Template(value, live_ptr))
    }    
    fn not_found()->FrameResult<'a>{FrameResult::Ok(())}
    fn found(arg:FrameFound)->FrameResult{FrameResult::Err(arg)}
    fn is_not_found(&self)->bool;
    fn is_found(&self)->bool;
    fn into_found(self)->Option<FrameFound<'a>>;
}

impl<'a> FrameResultApi<'a> for FrameResult<'a> {

    fn is_not_found(&self) -> bool {
        match *self {
            Result::Ok(_) => true,
            Result::Err(_) => false
        }
    }
    fn is_found(&self) -> bool {
        match *self {
            Result::Ok(_) => false,
            Result::Err(_) => true
        }
    }
    fn into_found(self)->Option<FrameFound<'a>>{
        match self {
            Result::Ok(_) => None,
            Result::Err(arg) => Some(arg)
        }
    }
}

/*
pub enum FrameResult<'a> {
    NotFound,
    Found(FrameFound<'a>)
}

impl<'a> FrameResult<'a> {
    pub fn child(value: &'a mut Box<dyn FrameComponent >) -> Self {
        Self::Found(FrameFound::Child(value))
    }
    pub fn template(value: &'a mut Box<dyn FrameComponent >, live_ptr: LivePtr) -> Self {
        Self::Found(FrameFound::Template(value, live_ptr))
    }
}

impl<'a> FromResidual for FrameResult<'a> {
    fn from_residual(residual: FrameFound<'a>) -> Self {
        Self::Found(residual)
    }
}

impl<'a> Try for FrameResult<'a> {
    type Output = ();
    type Residual = FrameFound<'a>;
    
    fn from_output(_: Self::Output) -> Self {
        FrameResult::NotFound
    }
    
    fn branch(self) -> ControlFlow<Self::Residual,
    Self::Output> {
        match self {
            Self::NotFound => ControlFlow::Continue(()),
            Self::Found(c) => ControlFlow::Break(c)
        }
    }
}*/

pub type FrameDraw = Result<(),FrameUid>;

pub trait FrameDrawApi{
    fn done()->FrameDraw{Result::Ok(())}
    fn not_done(arg:FrameUid)->FrameDraw{Result::Err(arg)}
    fn is_done(&self)->bool;
    fn is_not_done(&self)->bool;
    fn into_not_done(&self)->Option<FrameUid>;
}

impl FrameDrawApi for FrameDraw {
    fn is_done(&self) -> bool {
        match *self {
            Result::Ok(_) => true,
            Result::Err(_) => false
        }
    }
    fn is_not_done(&self) -> bool {
        match *self {
            Result::Ok(_) => false,
            Result::Err(_) => true
        }
    }
    fn into_not_done(&self)->Option<FrameUid>{
        match *self {
            Result::Ok(_) => None,
            Result::Err(uid) => Some(uid)
        }
    }
}

/*
pub enum FrameDraw {
    Done,
    FrameUid(FrameUid)
}

impl FrameDraw {
    pub fn is_done(&self) -> bool {
        match self {
            Self::Done => true,
            _ => false
        }
    }
    pub fn not_done(&self) -> bool {
        match self {
            Self::Done => false,
            _ => true
        }
    }
}

impl FromResidual for FrameDraw {
    fn from_residual(residual: FrameUid) -> Self {
        Self::FrameUid(residual)
    }
}

impl Try for FrameDraw {
    type Output = ();
    type Residual = FrameUid;
    
    fn from_output(_: Self::Output) -> Self {
        FrameDraw::Done
    }
    
    fn branch(self) -> ControlFlow<Self::Residual,
    Self::Output> {
        match self {
            Self::Done => ControlFlow::Continue(()),
            Self::FrameUid(c) => ControlFlow::Break(c)
        }
    }
}
*/


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
        if let Some(inner) = &self.0 {
            FrameUid::from_frame_component(inner)
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
    
    pub fn handle_component_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)) {
        if let Some(inner) = &mut self.0 {
            return inner.handle_component_event(cx, event, dispatch_action)
        }
    }
    
    pub fn handle_component_event_iter(&mut self, cx: &mut Cx, event: &Event) -> Vec<FrameActionItem> {
        if let Some(inner) = &mut self.0 {
            return inner.handle_component_event_iter(cx, event)
        }
        Vec::new()
    }
    
    pub fn frame_query(&mut self, query: &FrameQuery, callback: &mut Option<FrameQueryCb>) -> FrameResult {
        if let Some(inner) = &mut self.0 {
            match query {
                FrameQuery::All => {
                    if let Some(callback) = callback {
                        callback.call(FrameFound::Child(inner))
                    }
                    else {
                        return FrameResult::child(inner)
                    }
                },
                FrameQuery::TypeId(id) => {
                    if inner.type_id() == *id{
                        if let Some(callback) = callback {
                            callback.call(FrameFound::Child(inner))
                        }
                        else {
                            return FrameResult::child(inner)
                        }
                    }
                },
                FrameQuery::Uid(uid) => {
                    if *uid == FrameUid(&*inner as *const _ as u64) {
                        if let Some(callback) = callback {
                            callback.call(FrameFound::Child(inner))
                        }
                        else {
                            return FrameResult::child(inner)
                        }
                    }
                }
                FrameQuery::Path(path) => if path.len() == 1{
                    if let Some(live_ptr) = inner.query_template(path[0]) {
                        if let Some(callback) = callback {
                            callback.call(FrameFound::Template(inner, live_ptr))
                        }
                        else {
                            return FrameResult::template(inner, live_ptr)
                        }
                    }
                }
            }
            inner.frame_query(query, callback)
        }
        else {
            FrameResult::not_found()
        }
    }
    
    pub fn template(
        &mut self,
        cx: &mut Cx,
        path: &[LiveId],
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn FrameComponent >> {
        if let Some(inner) = &mut self.0 {
            return inner.template(cx, path, new_id, nodes);
        }
        else {
            None
        }
    }
    
    pub fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> FrameDraw {
        if let Some(inner) = &mut self.0 {
            return inner.draw_component(cx, walk, FrameUid(&*inner as *const _ as u64))
        }
        FrameDraw::done()
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
    
    pub fn draw_walk_component(&mut self, cx: &mut Cx2d) -> FrameDraw {
        if let Some(inner) = &mut self.0 {
            return inner.draw_walk_component(cx, FrameUid(&*inner as *const _ as u64))
        }
        FrameDraw::done()
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
            else{
                cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, nodes[index].id);
            }
        }
        else if let Some(component) = &mut self.0 {
            return component.apply(cx, from, index, nodes)
        }
        cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, nodes[index].id);
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
            live_ignore: true,
            type_name: LiveId(0)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub struct FrameUid(u64);

impl FrameUid{
    pub fn empty()->Self{Self::default()}
    pub fn is_empty(&self)->bool{self.0 == 0}
    pub fn from_frame_component(fc:&Box<dyn FrameComponent>)->Self{
        FrameUid(&*fc as *const _ as u64)
    }
}


pub struct FrameActionItem {
    pub uids: Vec<FrameUid>,
    pub ids: Vec<LiveId>,
    pub bind_delta: Option<Vec<LiveNode>>,
    pub action: Box<dyn FrameAction>
}

impl FrameActionItem {
    pub fn new(action: Box<dyn FrameAction>) -> Self {
        Self{
            uids: Vec::new(),
            ids: Vec::new(),
            bind_delta: None,
            action
        }
    }
    
    pub fn bind_delta(mut self, bind_delta: Vec<LiveNode>,) -> Self {
        if bind_delta.len()>0{self.bind_delta = Some(bind_delta)}
        self
    }
    
    pub fn action<T: FrameAction + 'static >(&self)->T where T:Default + Clone{
        self.action.cast::<T>()
    }
    
    pub fn id(&self)->LiveId{
        self.ids[0]
    }
    
    pub fn uid(&self)->FrameUid{
        self.uids[0]
    }
    
    pub fn has_bind_delta(&self)->bool{
        self.bind_delta.is_some()
    }
    
    pub fn mark(mut self, id: LiveId, uid: FrameUid) -> Self {
        self.uids.push(uid);
        self.ids.push(id);
        self
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
        if self.redraw_id != cx.redraw_id() {
            self.redraw_id = cx.redraw_id();
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
