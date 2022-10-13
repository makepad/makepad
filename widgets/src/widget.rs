use {
    crate::makepad_draw_2d::*,
    std::collections::BTreeMap,
    std::any::TypeId
};
pub use crate::widget;

pub trait Widget: LiveApply {
    fn handle_widget_event(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _fn_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {}
    
    fn handle_widget_event_iter(&mut self, cx: &mut Cx, event: &Event) -> Vec<WidgetActionItem> {
        let mut actions = Vec::new();
        self.handle_widget_event(cx, event, &mut | _, action | {
            actions.push(action);
        });
        actions
    }
    
    fn widget_query(
        &mut self,
        _query: &WidgetQuery,
        _callback: &mut Option<WidgetQueryCb>
    ) -> WidgetResult {
        return WidgetResult::not_found()
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk, self_uid: WidgetUid) -> WidgetDraw;
    fn get_walk(&self) -> Walk;
    
    // defaults
    fn redraw(&mut self, _cx: &mut Cx) {}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, self_uid: WidgetUid) -> WidgetDraw {
        self.draw_widget(cx, self.get_walk(), self_uid)
    }
    
    fn create_child(
        &mut self,
        _cx: &mut Cx,
        _live_ptr: LivePtr,
        _at: CreateAt,
        _new_id: LiveId,
        _nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn Widget >> {
        None
    }
    
    fn template(
        &mut self,
        cx: &mut Cx,
        path: &[LiveId],
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn Widget >> {
        // first we query the template
        if path.len() == 1{
            if let Some(live_ptr) = self.query_template(path[0]){
                return self.create_child(cx, live_ptr, CreateAt::Template, new_id, nodes)
            }
        }
        if let Some(WidgetFound::Template(child, live_ptr)) = 
        self.widget_query(&WidgetQuery::Path(path), &mut None).into_found() {
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

pub struct WidgetQueryCb<'a>{
    pub cx:&'a mut Cx,
    pub cb:&'a mut dyn FnMut(&mut Cx, WidgetFound)
}

impl<'a> WidgetQueryCb<'a>{
    pub fn call(&mut self, args:WidgetFound){
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

pub enum WidgetQuery<'a> {
    All,
    TypeId(std::any::TypeId),
    Path(&'a [LiveId]),
    Uid(WidgetUid),
}

pub enum WidgetFound<'a> {
    Child(&'a mut Box<dyn Widget >),
    Template(&'a mut Box<dyn Widget >, LivePtr)
}

pub type WidgetResult<'a> = Result<(),WidgetFound<'a>>;

pub trait WidgetResultApi<'a>{
    fn child(value: &'a mut Box<dyn Widget >) -> WidgetResult<'a> {
       Result::Err(WidgetFound::Child(value))
    }
    fn template(value: &'a mut Box<dyn Widget >, live_ptr: LivePtr) -> WidgetResult<'a> {
        Result::Err(WidgetFound::Template(value, live_ptr))
    }    
    fn not_found()->WidgetResult<'a>{WidgetResult::Ok(())}
    fn found(arg:WidgetFound)->WidgetResult{WidgetResult::Err(arg)}
    fn is_not_found(&self)->bool;
    fn is_found(&self)->bool;
    fn into_found(self)->Option<WidgetFound<'a>>;
}

impl<'a> WidgetResultApi<'a> for WidgetResult<'a> {

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
    fn into_found(self)->Option<WidgetFound<'a>>{
        match self {
            Result::Ok(_) => None,
            Result::Err(arg) => Some(arg)
        }
    }
}

/*
pub enum WidgetResult<'a> {
    NotFound,
    Found(WidgetFound<'a>)
}

impl<'a> WidgetResult<'a> {
    pub fn child(value: &'a mut Box<dyn Widget >) -> Self {
        Self::Found(WidgetFound::Child(value))
    }
    pub fn template(value: &'a mut Box<dyn Widget >, live_ptr: LivePtr) -> Self {
        Self::Found(WidgetFound::Template(value, live_ptr))
    }
}

impl<'a> FromResidual for WidgetResult<'a> {
    fn from_residual(residual: WidgetFound<'a>) -> Self {
        Self::Found(residual)
    }
}

impl<'a> Try for WidgetResult<'a> {
    type Output = ();
    type Residual = WidgetFound<'a>;
    
    fn from_output(_: Self::Output) -> Self {
        WidgetResult::NotFound
    }
    
    fn branch(self) -> ControlFlow<Self::Residual,
    Self::Output> {
        match self {
            Self::NotFound => ControlFlow::Continue(()),
            Self::Found(c) => ControlFlow::Break(c)
        }
    }
}*/

pub type WidgetDraw = Result<(),WidgetUid>;

pub trait WidgetDrawApi{
    fn done()->WidgetDraw{Result::Ok(())}
    fn not_done(arg:WidgetUid)->WidgetDraw{Result::Err(arg)}
    fn is_done(&self)->bool;
    fn is_not_done(&self)->bool;
    fn into_not_done(&self)->Option<WidgetUid>;
}

impl WidgetDrawApi for WidgetDraw {
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
    fn into_not_done(&self)->Option<WidgetUid>{
        match *self {
            Result::Ok(_) => None,
            Result::Err(uid) => Some(uid)
        }
    }
}

/*
pub enum WidgetDraw {
    Done,
    WidgetUid(WidgetUid)
}

impl WidgetDraw {
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

impl FromResidual for WidgetDraw {
    fn from_residual(residual: WidgetUid) -> Self {
        Self::WidgetUid(residual)
    }
}

impl Try for WidgetDraw {
    type Output = ();
    type Residual = WidgetUid;
    
    fn from_output(_: Self::Output) -> Self {
        WidgetDraw::Done
    }
    
    fn branch(self) -> ControlFlow<Self::Residual,
    Self::Output> {
        match self {
            Self::Done => ControlFlow::Continue(()),
            Self::WidgetUid(c) => ControlFlow::Break(c)
        }
    }
}
*/


generate_ref_cast_api!(Widget);

pub trait WidgetFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn Widget>;
}

#[derive(Default, LiveComponentRegistry)]
pub struct WidgetRegistry {
    pub map: BTreeMap<LiveType, (LiveComponentInfo, Box<dyn WidgetFactory>)>
}

pub trait WidgetAction: 'static {
    fn type_id(&self) -> TypeId;
    fn box_clone(&self) -> Box<dyn WidgetAction>;
}

impl<T: 'static + ? Sized + Clone> WidgetAction for T {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    
    fn box_clone(&self) -> Box<dyn WidgetAction> {
        Box::new((*self).clone())
    }
}

generate_clone_cast_api!(WidgetAction);

impl Clone for Box<dyn WidgetAction> {
    fn clone(&self) -> Box<dyn WidgetAction> {
        self.as_ref().box_clone()
    }
}

pub struct WidgetRef(Option<Box<dyn Widget >>);

impl WidgetRef {
    pub fn empty() -> Self {Self (None)}
    
    pub fn as_uid(&self) -> WidgetUid {
        if let Some(inner) = &self.0 {
            WidgetUid::from_frame_component(inner)
        }
        else {
            WidgetUid(0)
        }
    }
    
    pub fn as_ref(&self) -> Option<&Box<dyn Widget >> {
        self.0.as_ref()
    }
    pub fn as_mut(&mut self) -> Option<&mut Box<dyn Widget >> {
        self.0.as_mut()
    }
    
    pub fn handle_widget_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        if let Some(inner) = &mut self.0 {
            return inner.handle_widget_event(cx, event, dispatch_action)
        }
    }
    
    pub fn handle_widget_event_iter(&mut self, cx: &mut Cx, event: &Event) -> Vec<WidgetActionItem> {
        if let Some(inner) = &mut self.0 {
            return inner.handle_widget_event_iter(cx, event)
        }
        Vec::new()
    }
    
    pub fn widget_query(&mut self, query: &WidgetQuery, callback: &mut Option<WidgetQueryCb>) -> WidgetResult {
        if let Some(inner) = &mut self.0 {
            match query {
                WidgetQuery::All => {
                    if let Some(callback) = callback {
                        callback.call(WidgetFound::Child(inner))
                    }
                    else {
                        return WidgetResult::child(inner)
                    }
                },
                WidgetQuery::TypeId(id) => {
                    if inner.type_id() == *id{
                        if let Some(callback) = callback {
                            callback.call(WidgetFound::Child(inner))
                        }
                        else {
                            return WidgetResult::child(inner)
                        }
                    }
                },
                WidgetQuery::Uid(uid) => {
                    if *uid == WidgetUid(&*inner as *const _ as u64) {
                        if let Some(callback) = callback {
                            callback.call(WidgetFound::Child(inner))
                        }
                        else {
                            return WidgetResult::child(inner)
                        }
                    }
                }
                WidgetQuery::Path(path) => if path.len() == 1{
                    if let Some(live_ptr) = inner.query_template(path[0]) {
                        if let Some(callback) = callback {
                            callback.call(WidgetFound::Template(inner, live_ptr))
                        }
                        else {
                            return WidgetResult::template(inner, live_ptr)
                        }
                    }
                }
            }
            inner.widget_query(query, callback)
        }
        else {
            WidgetResult::not_found()
        }
    }
    
    pub fn template(
        &mut self,
        cx: &mut Cx,
        path: &[LiveId],
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn Widget >> {
        if let Some(inner) = &mut self.0 {
            return inner.template(cx, path, new_id, nodes);
        }
        else {
            None
        }
    }
    
    pub fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if let Some(inner) = &mut self.0 {
            return inner.draw_widget(cx, walk, WidgetUid(&*inner as *const _ as u64))
        }
        WidgetDraw::done()
    }
    
    pub fn get_walk(&mut self) -> Walk {
        if let Some(inner) = &mut self.0 {
            return inner.get_walk()
        }
        Walk::default()
    }
    
    // forwarding Widget trait
    pub fn redraw(&mut self, cx: &mut Cx) {
        if let Some(inner) = &mut self.0 {
            return inner.redraw(cx)
        }
    }
    
    pub fn draw_walk_widget(&mut self, cx: &mut Cx2d) -> WidgetDraw {
        if let Some(inner) = &mut self.0 {
            return inner.draw_walk_widget(cx, WidgetUid(&*inner as *const _ as u64))
        }
        WidgetDraw::done()
    }
}

impl LiveHook for WidgetRef {}
impl LiveApply for WidgetRef {
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
                .components.get::<WidgetRegistry>().new(cx, live_type) {
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

impl LiveNew for WidgetRef {
    fn new(_cx: &mut Cx) -> Self {
        Self (None)
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<dyn Widget>(),
            fields: Vec::new(),
            live_ignore: true,
            type_name: LiveId(0)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub struct WidgetUid(u64);

impl WidgetUid{
    pub fn empty()->Self{Self::default()}
    pub fn is_empty(&self)->bool{self.0 == 0}
    pub fn from_frame_component(fc:&Box<dyn Widget>)->Self{
        WidgetUid(&*fc as *const _ as u64)
    }
}


pub struct WidgetActionItem {
    pub uids: Vec<WidgetUid>,
    pub ids: Vec<LiveId>,
    pub bind_delta: Option<Vec<LiveNode>>,
    pub action: Box<dyn WidgetAction>
}

impl WidgetActionItem {
    pub fn new(action: Box<dyn WidgetAction>) -> Self {
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
    
    pub fn action<T: WidgetAction + 'static >(&self)->T where T:Default + Clone{
        self.action.cast::<T>()
    }
    
    pub fn id(&self)->LiveId{
        self.ids[0]
    }
    
    pub fn uid(&self)->WidgetUid{
        self.uids[0]
    }
    
    pub fn has_bind_delta(&self)->bool{
        self.bind_delta.is_some()
    }
    
    pub fn mark(mut self, id: LiveId, uid: WidgetUid) -> Self {
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
macro_rules!widget {
    ( $ ty: ty) => {
        | cx: &mut Cx | {
            struct Factory();
            impl WidgetFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn Widget> {
                    Box::new(< $ ty>::new(cx))
                }
            }
            register_component_factory!(cx, WidgetRegistry, $ ty, Factory);
        }
    }
}
