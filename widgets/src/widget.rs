use {
    crate::makepad_draw::*,
    std::fmt::{Formatter, Debug, Error},
    std::collections::BTreeMap,
    std::any::TypeId,
    std::cell::RefCell,
    std::rc::Rc,
    std::fmt
};
pub use crate::register_widget;

#[derive(Clone, Copy)]
pub enum WidgetCache {
    Yes,
    No,
    Clear
}

#[derive(Clone, Debug, Copy, PartialEq)]
pub struct WidgetUid(pub u64);

pub trait WidgetDesign {
}

pub trait WidgetNode: LiveApply{
    fn find_widgets(&mut self, _path: &[LiveId], _cached: WidgetCache, _results: &mut WidgetSet);
    fn walk(&mut self, _cx:&mut Cx) -> Walk;
    fn redraw(&mut self, _cx: &mut Cx);
}

pub trait Widget: WidgetNode {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
    }

    fn widget(&mut self, path: &[LiveId]) -> WidgetRef {
        let mut results = WidgetSet::default();
        self.find_widgets(path, WidgetCache::Yes, &mut results);
        return results.into_first()
    }
    
    fn widgets(&mut self, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        for path in paths {
            self.find_widgets(path, WidgetCache::Yes, &mut results);
        }
        results
    }
    
    // fn widget_uid(&self)->WidgetUid;
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as *const () as u64)}
    
    fn widget_to_data(&self, _cx: &mut Cx, _actions: &Actions, _nodes: &mut LiveNodeVec, _path: &[LiveId]) -> bool {false}
    fn data_to_widget(&mut self, _cx: &mut Cx, _nodes: &[LiveNode], _path: &[LiveId]) {}
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep;
    
    fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope) -> DrawStep{
        let walk = self.walk(cx);
        self.draw_walk(cx, scope, walk)
    }
    
    fn draw_walk_all(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk:Walk){
        while self.draw_walk(cx, scope, walk).is_step(){}
    }
    
    fn is_visible(&self) -> bool {
        true
    }

    fn draw_all(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        while self.draw(cx, scope).is_step() {};
    }
    
    fn text(&self) -> String {
        String::new()
    }
    
    fn set_text(&mut self, _v: &str) {
    }
    
    fn set_text_and_redraw(&mut self, cx: &mut Cx, v: &str) {
        self.set_text(v);
        self.redraw(cx);
    }
    /*
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
    }*/
    
    fn ref_cast_type_id(&self) -> LiveType where Self: 'static {LiveType::of::<Self>()}
}

#[derive(Clone, Copy)]
pub enum CreateAt {
    Template,
    Begin,
    After(LiveId),
    Before(LiveId),
    End
}


pub trait DrawStepApi {
    fn done() -> DrawStep {Result::Ok(())}
    fn make_step_here(arg: WidgetRef) -> DrawStep {Result::Err(arg)}
    fn make_step() -> DrawStep {Result::Err(WidgetRef::empty())}
    fn is_done(&self) -> bool;
    fn is_step(&self) -> bool;
    fn step(self) -> Option<WidgetRef>;
}

impl DrawStepApi for DrawStep {
    fn is_done(&self) -> bool {
        match *self {
            Result::Ok(_) => true,
            Result::Err(_) => false
        }
    }
    fn is_step(&self) -> bool {
        match *self {
            Result::Ok(_) => false,
            Result::Err(_) => true
        }
    }
    
    fn step(self) -> Option<WidgetRef> {
        match self {
            Result::Ok(_) => None,
            Result::Err(nd) => Some(nd)
        }
    }
}

pub type DrawStep = Result<(), WidgetRef>;

generate_any_trait_api!(Widget);

pub trait WidgetFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn Widget>;
}

#[derive(Default, LiveComponentRegistry)]
pub struct WidgetRegistry {
    pub map: BTreeMap<LiveType, (LiveComponentInfo, Box<dyn WidgetFactory>)>
}

pub struct WidgetRefInner{ 
    pub widget: Box<dyn Widget >,
}
#[derive(Clone, Default)]
pub struct WidgetRef(Rc<RefCell<Option<WidgetRefInner>>>);

impl Debug for WidgetRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "WidgetRef {}", self.widget_uid().0)
    }
}

#[derive(Clone)]
pub enum WidgetSet {
    Inline {
        set: [Option<WidgetRef>; 4],
        len: usize
    },
    Vec(Vec<WidgetRef>),
    Empty
}

impl std::fmt::Debug for WidgetSet {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        match self {
            Self::Inline {len, ..} => {
                let _ = write!(f, "WidgetSet::Inline: {}", len);
            },
            Self::Vec(vec) => {
                let _ = write!(f, "WidgetSet::Vec: {}", vec.len());
            },
            Self::Empty => {
                let _ = write!(f, "WidgetSet::Empty");
            }
        }
        Ok(())
    }
}

impl Default for WidgetSet {
    fn default() -> Self {Self::Empty}
}

impl WidgetSet {
    pub fn is_empty(&mut self) -> bool {
        if let Self::Empty = self {
            true
        }
        else {
            false
        }
    }
    
    pub fn push(&mut self, item: WidgetRef) {
        match self {
            Self::Empty => {
                *self = Self::Inline {
                    set: [Some(item), None, None, None],
                    len: 1
                }
            }
            Self::Inline {len, set} => {
                if *len == set.len() {
                    let mut vec = Vec::new();
                    for item in set {
                        vec.push(item.clone().unwrap());
                    }
                    vec.push(item);
                    *self = Self::Vec(vec);
                }
                else {
                    set[*len] = Some(item);
                    *len += 1;
                }
            }
            Self::Vec(vec) => {
                vec.push(item);
            }
        }
    }
    
    pub fn extend_from_set(&mut self, other: &WidgetSet) {
        for item in other.iter() {
            self.push(item);
        }
    }
    
    pub fn into_first(self) -> WidgetRef {
        match self {
            Self::Empty => {
                WidgetRef::empty()
            }
            Self::Inline {len: _, mut set} => {
                set[0].take().unwrap()
            }
            Self::Vec(mut vec) => {
                vec.remove(0)
            }
        }
    }
    
    pub fn widgets(&self, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        for widget in self.iter() {
            if let Some(inner) = widget.0.borrow_mut().as_mut() {
                for path in paths {
                    inner.widget.find_widgets(path, WidgetCache::Yes, &mut results);
                }
            }
        }
        results
    }
    
    pub fn contains(&self, widget: &WidgetRef) -> bool {
        for item in self.iter() {
            if item == *widget {
                return true
            }
        }
        false
    }
}

impl LiveHook for WidgetSet {}
impl LiveApply for WidgetSet {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        for inner in self.iter() {
            let mut inner = inner.0.borrow_mut();
            if let Some(component) = &mut *inner {
                return component.widget.apply(cx, apply, index, nodes)
            }
        }
        nodes.skip_node(index)
    }
}

impl WidgetSet {
    pub fn empty() -> Self {Self::Empty}
    pub fn iter(&self) -> WidgetSetIterator {
        WidgetSetIterator {
            widget_set: self,
            index: 0
        }
    }
    
    pub fn set_text(&self, v: &str) {
        for item in self.iter(){
            item.set_text(v)
        }
    }
    
    pub fn set_text_and_redraw(&self, cx: &mut Cx, v: &str) {
        for item in self.iter(){
            item.set_text_and_redraw(cx, v)
        }
    }
}

pub struct WidgetSetIterator<'a> {
    widget_set: &'a WidgetSet,
    index: usize
}

impl<'a> Iterator for WidgetSetIterator<'a> {
    // We can refer to this type using Self::Item
    type Item = WidgetRef;
    fn next(&mut self) -> Option<Self::Item> {
        match self.widget_set {
            WidgetSet::Empty => {
                return None;
            }
            WidgetSet::Inline {set, len} => {
                if self.index >= *len {
                    return None
                }
                let ret = set[self.index].as_ref().unwrap();
                self.index += 1;
                return Some(ret.clone())
            }
            WidgetSet::Vec(vec) => {
                if self.index >= vec.len() {
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
        Self (Rc::new(RefCell::new(Some(WidgetRefInner{
            widget,
        }))))
    }
    
    pub fn handle_event(&self, cx: &mut Cx, event: &Event, scope:&mut Scope){
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            // if we're in a draw event, do taht here
            if let Event::Draw(e) = event{
                let cx = &mut Cx2d::new(cx, e);
                return inner.widget.draw_all(cx, scope);
            }
            return inner.widget.handle_event(cx, event, scope)
        }
    }
    
    pub fn widget_uid(&self) -> WidgetUid {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget.widget_uid()
        }
        WidgetUid(0)
    }
    
    pub fn widget_to_data(&self, cx: &mut Cx, actions: &Actions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.widget_to_data(cx, actions, nodes, path);
        }
        false
    }
    
    pub fn data_to_widget(&self, cx: &mut Cx, nodes: &[LiveNode], path: &[LiveId]) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.data_to_widget(cx, nodes, path);
        }
    }
    
    pub fn find_widgets(
        &mut self,
        path: &[LiveId],
        cached: WidgetCache,
        results: &mut WidgetSet
    ) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.find_widgets(path, cached, results)
        }
    }
    
    pub fn widget(&self, path: &[LiveId]) -> WidgetRef {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.widget(path);
        }
        WidgetRef::empty()
    }
    
    pub fn widgets(&self, paths: &[&[LiveId]]) -> WidgetSet {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.widgets(paths);
        }
        WidgetSet::default()
    }
    
    pub fn draw_walk(&self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
           if let Some(nd) = inner.widget.draw_walk(cx, scope, walk).step() {
                if nd.is_empty() {
                    return DrawStep::make_step_here(self.clone())
                }
                return DrawStep::make_step_here(nd);
            }
        }
        DrawStep::done()
    }
    
    pub fn draw_walk_all(&self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk)  {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.draw_walk_all(cx, scope, walk)
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope) -> DrawStep{
        if let Some(inner) = self.0.borrow_mut().as_mut() {
        if let Some(nd) = inner.widget.draw(cx, scope).step() {
                if nd.is_empty() {
                    return DrawStep::make_step_here(self.clone())
                }
                return DrawStep::make_step_here(nd);
            }
        }
        DrawStep::done()
    }
    
    pub fn walk(&self, cx:&mut Cx) -> Walk {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.walk(cx)
        }
        Walk::default()
    }
    
    // forwarding Widget trait
    pub fn redraw(&self, cx: &mut Cx) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.redraw(cx)
        }
    }
    
    pub fn is_visible(&self) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget.is_visible()
        }
        true
    }
    
    pub fn draw_all(&self, cx: &mut Cx2d, scope:&mut Scope) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.draw_all(cx, scope)
        }
    }
    
    pub fn text(&self) -> String {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.text()
        }
        else {
            String::new()
        }
    }
    
    pub fn set_text(&self, v: &str) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.set_text(v)
        }
    }
    
    pub fn set_text_and_redraw(&self, cx: &mut Cx, v: &str) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.set_text_and_redraw(cx, v);
        }
    }
    
    pub fn borrow_mut<T: 'static + Widget>(&self) -> Option<std::cell::RefMut<'_, T >> {
        if let Ok(ret) = std::cell::RefMut::filter_map(self.0.borrow_mut(), | inner | {
            if let Some(inner) = inner.as_mut() {
                inner.widget.downcast_mut::<T>()
            }
            else {
                None
            }
        }) {
            Some(ret)
        }
        else {
            None
        }
    }
    
    pub fn borrow<T: 'static + Widget>(&self) -> Option<std::cell::Ref<'_, T >> {
        if let Ok(ret) = std::cell::Ref::filter_map(self.0.borrow(), | inner | {
            if let Some(inner) = inner.as_ref() {
                inner.widget.downcast_ref::<T>()
            }
            else {
                None
            }
        }) {
            Some(ret)
        }
        else {
            None
        }
    }
    
    pub fn apply_over(&self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, &mut ApplyFrom::Over.into(), 0, nodes);
    }

    pub fn apply_over_and_redraw(&self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, &mut ApplyFrom::Over.into(), 0, nodes);
        self.redraw(cx);
    }
    
    fn apply(&self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        let mut inner = self.0.borrow_mut();
        if let LiveValue::Class {live_type, ..} = nodes[index].value {
            if let Some(component) = &mut *inner {
                if component.widget.ref_cast_type_id() != live_type {
                    *inner = None; // type changed, drop old component
                    log!("TYPECHANGE {:?}", nodes[index]);
                }
                else {
                    return component.widget.apply(cx, apply, index, nodes);
                }
            }
            if let Some(component) = cx.live_registry.clone().borrow()
                .components.get::<WidgetRegistry>().new(cx, live_type) {
                    if cx.debug.marker() == 1{
                        panic!()
                    }
                *inner = Some(WidgetRefInner{
                    widget: component,
                });
                if let Some(component) = &mut *inner {
                    return component.widget.apply(cx, apply, index, nodes);
                }
            }
            else {
                cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, nodes[index].id);
            }
        }
        else if let Some(component) = &mut *inner {
            return component.widget.apply(cx, apply, index, nodes)
        }
        cx.apply_error_cant_find_target(live_error_origin!(), index, nodes, nodes[index].id);
        nodes.skip_node(index)
    }
}

impl LiveHook for WidgetRef {}
impl LiveApply for WidgetRef {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        <WidgetRef>::apply(self, cx, apply, index, nodes)
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

pub trait WidgetActionTrait: 'static {
    fn ref_cast_type_id(&self) -> TypeId;
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
    fn box_clone(&self) -> Box<dyn WidgetActionTrait>;
}

impl<T: 'static + ? Sized + Clone + Debug> WidgetActionTrait for T {
    fn ref_cast_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
        
    fn box_clone(&self) -> Box<dyn WidgetActionTrait> {
        Box::new((*self).clone())
    }
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        self.fmt(f)
    }
}

impl Debug for dyn WidgetActionTrait{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        self.debug_fmt(f)
    }
}

generate_any_trait_api!(WidgetActionTrait);

impl Clone for Box<dyn WidgetActionTrait> {
    fn clone(&self) -> Box<dyn WidgetActionTrait> {
        self.as_ref().box_clone()
    }
}

#[derive(Clone, Debug)]
pub struct WidgetAction {
    action: Box<dyn WidgetActionTrait>,
    pub widget_uid: WidgetUid,
    pub path: HeapLiveIdPath,
    pub group: Option<WidgetActionGroup>
}


#[derive(Clone, Debug)]
pub struct WidgetActionGroup{
    pub group_uid: WidgetUid,
    pub item_uid: WidgetUid,
}

pub trait WidgetActionCxExt {
    fn widget_action(&mut self, uid:WidgetUid, path:&HeapLiveIdPath, t:impl WidgetActionTrait);
    fn group_widget_actions<F, R>(&mut self, group_id:WidgetUid, item_id:WidgetUid, f:F) -> R where F: FnOnce(&mut Cx) -> R;
}

pub trait WidgetActionsApi {
    fn find_widget_action_cast<T: WidgetActionTrait + 'static >(&self, widget_uid: WidgetUid) -> T where T: Default + Clone;
    fn find_widget_action(&self, widget_uid: WidgetUid) -> Option<&WidgetAction>;
}

pub trait WidgetActionOptionApi{
    fn widget_uid_eq(&self, widget_uid: WidgetUid) -> Option<&WidgetAction>;
    fn cast<T: WidgetActionTrait + 'static >(&self) -> T where T: Default + Clone;
}

impl WidgetActionOptionApi for Option<&WidgetAction>{
    fn widget_uid_eq(&self, widget_uid: WidgetUid) -> Option<&WidgetAction>{
        if let Some(item) = self {
            if item.widget_uid == widget_uid{
                return Some(item)
                                
            }
        }
        None
    }
    
    fn cast<T: WidgetActionTrait + 'static >(&self) -> T where T: Default + Clone{
        if let Some(item) = self{
            if let Some(item) = item.action.downcast_ref::<T>(){
                return item.clone()
            }
        }
        T::default()
    }
}

pub trait WidgetActionCast {
    fn as_widget_action(&self) -> Option<&WidgetAction>;
}
    
impl WidgetActionCast for Action{
    fn as_widget_action(&self) -> Option<&WidgetAction>{
        self.downcast_ref::<WidgetAction>()
    }
}

impl WidgetActionsApi for Actions{
    fn find_widget_action(&self, widget_uid: WidgetUid) -> Option<&WidgetAction>{
        for action in self{
            if let Some(action) = action.downcast_ref::<WidgetAction>(){
                if action.widget_uid == widget_uid{
                    return Some(action)
                }
            }
        }
        None
    }
    
    fn find_widget_action_cast<T: WidgetActionTrait + 'static >(&self, widget_uid: WidgetUid) -> T where T: Default + Clone {
        if let Some(item) = self.find_widget_action(widget_uid) {
            if let Some(item) = item.action.downcast_ref::<T>(){
                return item.clone()
            }
        }
        T::default()
    }
    
}

impl WidgetActionCxExt for Cx {
    fn widget_action(&mut self, widget_uid:WidgetUid, path:&HeapLiveIdPath, t:impl WidgetActionTrait){
        self.action(WidgetAction{
            widget_uid,
            path: path.clone(),
            action: Box::new(t),
            group: None,
        })
    }
    
    fn group_widget_actions<F, R>(&mut self, group_uid:WidgetUid, item_uid:WidgetUid, f:F) -> R where 
    F: FnOnce(&mut Cx) -> R{
        self.map_actions(|cx| f(cx), |_cx, mut actions|{
            for action in &mut actions{
                if let Some(action) = action.downcast_mut::<WidgetAction>(){
                    if action.group.is_none(){
                        action.group = Some(WidgetActionGroup{
                            group_uid,
                            item_uid,
                        })
                    }
                }
            }
            actions
        })
    }
}

impl WidgetAction {
    pub fn cast<T: WidgetActionTrait + 'static >(&self) -> T where T: Default + Clone {
        if let Some(item) = self.action.downcast_ref::<T>(){
            return item.clone()
        }
        T::default()
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
    pub fn begin(&mut self, cx: &mut Cx2d, init: T) -> bool {
        if self.redraw_id != cx.redraw_id() {
            self.redraw_id = cx.redraw_id();
            self.state = Some(init);
            true
        }
        else {
            false
        }
    }
    
    pub fn begin_with<F, S>(&mut self, cx: &mut Cx2d, v: &S, init: F) -> bool where F: FnOnce(&mut Cx2d, &S) -> T {
        if self.redraw_id != cx.redraw_id() {
            self.redraw_id = cx.redraw_id();
            self.state = Some(init(cx, v));
            true
        }
        else {
            false
        }
    }
    
    pub fn get(&self) -> Option<T> {
        self.state.clone()
    }
    
    pub fn as_ref(&self) -> Option<&T> {
        self.state.as_ref()
    }
    
    pub fn as_mut(&mut self) -> Option<&mut T> {
        self.state.as_mut()
    }
    
    pub fn set(&mut self, value: T) {
        self.state = Some(value);
    }
    
    pub fn end(&mut self) {
        self.state = None;
    }
}

#[macro_export]
macro_rules!register_widget {
    ( $ cx: ident, $ ty: ty) => {
        {
            struct Factory();
            impl WidgetFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn Widget> {
                    Box::new(< $ ty>::new(cx))
                }
            }
            register_component_factory!( $ cx, WidgetRegistry, $ ty, Factory);
        }
    }
}
