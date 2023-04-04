use {
    crate::makepad_draw::*,
    std::collections::BTreeMap,
    std::any::TypeId,
    std::cell::RefCell,
    std::rc::Rc
};
pub use crate::widget_factory;

#[derive(Clone, Copy)]
pub enum WidgetCache {
    Yes,
    No,
    Clear
}

#[derive(Clone, Debug, Copy, PartialEq)]
pub struct WidgetUid(pub u64);

pub trait WidgetDesign{
    
}

pub trait Widget: LiveApply {
    fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _fn_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {}
    
    fn handle_widget_event(&mut self, cx: &mut Cx, event: &Event) -> WidgetActions {
        let mut actions = Vec::new();
        self.handle_widget_event_with(cx, event, &mut | _, action | {
            actions.push(action);
        });
        actions
    }
    
    fn find_widgets(&mut self, _path: &[LiveId], _cached: WidgetCache, _results:&mut WidgetSet){
    }
    
   // fn widget_uid(&self)->WidgetUid;
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as *const () as u64)}
    
    fn widget_to_data(&self, _cx: &mut Cx, _actions:&WidgetActions, _nodes: &mut LiveNodeVec, _path: &[LiveId])->bool{false}
    fn data_to_widget(&mut self, _cx: &mut Cx, _nodes:&[LiveNode], _path: &[LiveId]){}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw;
    fn get_walk(&self) -> Walk;
    fn redraw(&mut self, _cx: &mut Cx);
    
    fn is_visible(&self)->bool{
        true
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d) -> WidgetDraw {
        self.draw_walk_widget(cx, self.get_walk())
    }
    
    fn create_child(
        &mut self,
        _cx: &mut Cx,
        _live_ptr: LivePtr,
        _at: CreateAt,
        _new_id: LiveId,
        _nodes: &[LiveNode]
    ) -> WidgetRef {
        WidgetRef::empty()
    }
    
    fn find_template(&self, _id: &[LiveId; 1]) -> Option<LivePtr> {
        None
    }
    
    fn type_id(&self) -> LiveType where Self: 'static {LiveType::of::<Self>()}
}

#[derive(Clone, Copy)]
pub enum CreateAt {
    Template,
    Begin,
    After(LiveId),
    Before(LiveId),
    End
}

pub trait WidgetDrawApi {
    fn done() -> WidgetDraw {Result::Ok(())}
    fn not_done(arg: WidgetRef) -> WidgetDraw {Result::Err(arg)}
    fn is_done(&self) -> bool;
    fn is_not_done(&self) -> bool;
    fn into_not_done(self) -> Option<WidgetRef>;
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
    fn into_not_done(self) -> Option<WidgetRef> {
        match self {
            Result::Ok(_) => None,
            Result::Err(nd) => Some(nd)
        }
    }
}

/*
pub type WidgetResult = Result<(), WidgetRef>;
pub trait WidgetResultApi {
    fn found(found: WidgetRef) -> WidgetResult {Result::Err(found)}
    fn not_found() -> WidgetResult {Result::Ok(())}
    fn into_found(self) -> Option<WidgetRef>;
}

impl WidgetResultApi for WidgetResult {
    fn into_found(self) -> Option<WidgetRef> {
        match self {
            Result::Ok(_) => None,
            Result::Err(found) => Some(found)
        }
    }
}*/

pub type WidgetDraw = Result<(), WidgetRef>;

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

#[derive(Clone)]
pub struct WidgetRef(Rc<RefCell<Option<Box<dyn Widget >> >>);

#[derive(Clone)]
pub enum WidgetSet{
    Inline{
        set:[Option<WidgetRef>;4],
        len: usize
    },
    Vec(Vec<WidgetRef>),
    Empty
}

impl std::fmt::Debug for WidgetSet{
    fn fmt(&self, f: &mut std::fmt::Formatter)->Result<(), std::fmt::Error>{
        match self{
            Self::Inline{len,..}=>{
                let _ = write!(f, "WidgetSet::Inline: {}", len);
            },
            Self::Vec(vec)=>{
                let _ = write!(f, "WidgetSet::Vec: {}", vec.len());
            },
            Self::Empty=>{
                let _ = write!(f, "WidgetSet::Empty");
            }
        }
        Ok(())
    }
}

impl Default for WidgetSet{
    fn default()->Self{Self::Empty}
}

impl WidgetSet{
    pub fn is_empty(&mut self)->bool{
        if let Self::Empty = self{
            true
        }
        else{
            false
        }
    }
    pub fn push(&mut self, item: WidgetRef){
        match self{
            Self::Empty=>{
                *self = Self::Inline{
                    set:[Some(item), None, None, None],
                    len: 1
                }
            }
            Self::Inline{len, set}=>{
                if *len == set.len(){
                    let mut vec = Vec::new();
                    for item in set{
                        vec.push(item.clone().unwrap());
                    }
                    vec.push(item);
                    *self = Self::Vec(vec);
                }
                else{
                    set[*len] = Some(item);
                    *len += 1;
                }
            }
            Self::Vec(vec)=>{
                vec.push(item);
            }
        }
    }
    
    pub fn extend_from_set(&mut self, other: &WidgetSet){
        for item in other.iter(){
            self.push(item);
        }
    }
    
    pub fn into_first(self)->WidgetRef{
        match self{
            Self::Empty=>{
                WidgetRef::empty()
            }
            Self::Inline{len:_, mut set}=>{
                set[0].take().unwrap()
            }
            Self::Vec(mut vec)=>{
                vec.remove(0)
            }
        }
    }
    
    pub fn get_widgets(&self, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        for widget in self.iter(){
            if let Some(inner) = widget.0.borrow_mut().as_mut() {
                for path in paths{
                    inner.find_widgets(path, WidgetCache::Yes, &mut results);
                }
            }
        }
        results
    }
}                     

impl WidgetSet{
    pub fn empty()->Self{Self::Empty}
    pub fn iter(&self)->WidgetSetIterator{
        WidgetSetIterator{
            widget_set: self,
            index: 0
        }
    }
}

pub struct WidgetSetIterator<'a>{
    widget_set: &'a WidgetSet,
    index: usize
}

impl<'a> Iterator for WidgetSetIterator<'a> {
    // We can refer to this type using Self::Item
    type Item = WidgetRef;
    fn next(&mut self)->Option<Self::Item>{
        match self.widget_set{
            WidgetSet::Empty=>{
                return None;
            }
            WidgetSet::Inline{set, len}=>{
                if self.index >= *len{
                    return None
                }
                let ret = set[self.index].as_ref().unwrap();
                self.index += 1;
                return Some(ret.clone())
            }
            WidgetSet::Vec(vec)=>{
                if self.index >= vec.len(){
                    return None
                }
                let ret = &vec[self.index];
                self.index += 1;
                return Some(ret.clone())
            }
        }
    }
}

impl PartialEq for WidgetRef {
    fn eq(&self, other: &WidgetRef) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
    
    fn ne(&self, other: &WidgetRef) -> bool {
        !Rc::ptr_eq(&self.0, &other.0)
    }
}

impl WidgetRef {
    pub fn empty() -> Self {Self (Rc::new(RefCell::new(None)))}
    
    pub fn is_empty(&self) -> bool {
        self.0.borrow().as_ref().is_none()
    }
    pub fn new_with_inner(widget: Box<dyn Widget>) -> Self {
        Self (Rc::new(RefCell::new(Some(widget))))
    }
    
    pub fn create_child(
        &self,
        cx: &mut Cx,
        live_ptr: LivePtr,
        at: CreateAt,
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> WidgetRef {
        let mut inner = self.0.borrow_mut();
        if let Some(inner) = &mut *inner {
            inner.create_child(cx, live_ptr, at, new_id, nodes)
        }
        else {
            WidgetRef::empty()
        }
    }
    
    pub fn handle_widget_event_with(&self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.handle_widget_event_with(cx, event, dispatch_action)
        }
    }
    
    pub fn handle_widget_event(&self, cx: &mut Cx, event: &Event) -> Vec<WidgetActionItem> {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.handle_widget_event(cx, event)
        }
        Vec::new()
    }
    
    pub fn widget_uid(&self) -> WidgetUid {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget_uid()
        }
        WidgetUid(0)
    }

    pub fn widget_to_data(&self, cx: &mut Cx, actions:&WidgetActions, nodes: &mut LiveNodeVec, path: &[LiveId])->bool{
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget_to_data(cx, actions, nodes, path);
        }
        false
    }
    
    pub fn data_to_widget(&self, cx: &mut Cx, nodes:&[LiveNode], path: &[LiveId]){
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.data_to_widget(cx, nodes, path);
        }
    }
    
    pub fn find_widgets(
        &mut self,
        path: &[LiveId],
        cached: WidgetCache,
        results: &mut WidgetSet
    ) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.find_widgets(path, cached, results)
        }
    }
    
    pub fn get_widget(&self, path: &[LiveId]) -> WidgetRef {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            let mut results = WidgetSet::default();
            inner.find_widgets(path, WidgetCache::Yes, &mut results);
            return results.into_first()
        }
        WidgetRef::empty()
    }
    
    pub fn get_widgets(&self, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            for path in paths{
                inner.find_widgets(path, WidgetCache::Yes, &mut results);
            }
        }
        results
    }

    pub fn find_template(&self, id: &[LiveId; 1]) -> Option<LivePtr> {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.find_template(id)
        }
        else {
            None
        }
    }
    
    pub fn draw_walk_widget(&self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            let ret = inner.draw_walk_widget(cx, walk);
            if let Some(nd) = ret.into_not_done() {
                if nd.is_empty() {
                    return WidgetDraw::not_done(self.clone())
                }
                return WidgetDraw::not_done(nd);
            }
        }
        WidgetDraw::done()
    }
    
    pub fn get_walk(&self) -> Walk {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.get_walk()
        }
        Walk::default()
    }
    
    // forwarding Widget trait
    pub fn redraw(&self, cx: &mut Cx) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.redraw(cx)
        }
    }


    pub fn is_visible(&self)->bool{
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.is_visible()
        }
        true
    }
    
    pub fn draw_widget(&self, cx: &mut Cx2d) -> WidgetDraw {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.draw_widget(cx)
        }
        WidgetDraw::done()
    }
    
    pub fn inner_mut<T: 'static + Widget>(&self) -> Option<std::cell::RefMut<'_, T >> {
        {
            if let Some(inner) = self.0.borrow_mut().as_mut() {
                if inner.type_id() != std::any::TypeId::of::<T>() {
                    return None
                }
            }
            else {
                return None
            }
        }
        Some(std::cell::RefMut::map(self.0.borrow_mut(), | inner | {
            inner.as_mut().unwrap().cast_mut::<T>().unwrap()
        }))
    }
    
    pub fn inner<T: 'static + Widget>(&self) -> Option<std::cell::Ref<'_, T >> {
        {
            // TODO this shouldnt borrow_mut but otherwise it doesnt compile for some reason
            if let Some(inner) = self.0.borrow_mut().as_mut() {
                if inner.type_id() != std::any::TypeId::of::<T>() {
                    return None
                }
            }
            else {
                return None
            }
        }
        Some(std::cell::Ref::map(self.0.borrow(), | inner | {
            inner.as_ref().unwrap().cast::<T>().unwrap()
        }))
    }
}

impl LiveHook for WidgetRef {}
impl LiveApply for WidgetRef {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut inner = self.0.borrow_mut();
        if let LiveValue::Class {live_type, ..} = nodes[index].value {
            if let Some(component) = &mut *inner {
                if component.type_id() != live_type {
                    *inner = None; // type changed, drop old component
                }
                else {
                    return component.apply(cx, from, index, nodes);
                }
            }
            if let Some(component) = cx.live_registry.clone().borrow()
                .components.get::<WidgetRegistry>().new(cx, live_type) {
                *inner = Some(component);
                if let Some(component) = &mut *inner {
                    return component.apply(cx, from, index, nodes);
                }
            }
            else {
                cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, nodes[index].id);
            }
        }
        else if let Some(component) = &mut *inner {
            return component.apply(cx, from, index, nodes)
        }
        cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, nodes[index].id);
        nodes.skip_node(index)
    }
}

impl LiveNew for WidgetRef {
    fn new(_cx: &mut Cx) -> Self {
        Self (Rc::new(RefCell::new(None)))
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

#[derive(Clone)]
pub struct WidgetActionItem {
    pub widget_uid: WidgetUid,
    pub action: Box<dyn WidgetAction>
}

pub type WidgetActions = Vec<WidgetActionItem>;

pub trait WidgetActionsApi {
    fn find_single_action(&self, widget_uid: WidgetUid) -> Option<&WidgetActionItem>;
    fn single_action<T: WidgetAction + 'static >(&self, widget_uid: WidgetUid) -> T where T: Default + Clone;
    fn not_empty(&self) -> bool;
}

impl WidgetActionsApi for WidgetActions {
    fn find_single_action(&self, widget_uid: WidgetUid) -> Option<&WidgetActionItem> {
        self.iter().find( | v | v.widget_uid == widget_uid)
    }

    fn single_action<T: WidgetAction + 'static >(&self, widget_uid: WidgetUid) -> T where T: Default + Clone {
        if let Some(item) = self.find_single_action(widget_uid){
            item.action.cast::<T>()
        }
        else{
            T::default()
        }
    }
    
    fn not_empty(&self) -> bool {
        self.len()>0
    }
}

impl WidgetActionItem {
    pub fn new(action: Box<dyn WidgetAction>, widget_uid:WidgetUid) -> Self {
        Self {
            widget_uid,
            action
        }
    }
    
    pub fn action<T: WidgetAction + 'static >(&self) -> T where T: Default + Clone {
        self.action.cast::<T>()
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
    
    pub fn get(&self) -> Option<T> {
        self.state.clone()
    }
    
    pub fn set(&mut self, value: T) {
        self.state = Some(value);
    }
    
    pub fn end(&mut self) {
        self.state = None;
    }
}

#[macro_export]
macro_rules!widget_factory {
    ( $cx:ident, $ ty: ty) => {
        {
            struct Factory();
            impl WidgetFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn Widget> {
                    Box::new(< $ ty>::new(cx))
                }
            }
            register_component_factory!($cx, WidgetRegistry, $ ty, Factory);
        }
    }
}
