use {
    crate::makepad_draw::*,
    std::fmt::{Formatter, Debug, Error},
    std::collections::BTreeMap,
    std::any::TypeId,
    std::cell::RefCell,
    std::rc::Rc
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
    
    fn find_widgets(&mut self, _path: &[LiveId], _cached: WidgetCache, _results: &mut WidgetSet) {
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
    
    fn widget_to_data(&self, _cx: &mut Cx, _actions: &WidgetActions, _nodes: &mut LiveNodeVec, _path: &[LiveId]) -> bool {false}
    fn data_to_widget(&mut self, _cx: &mut Cx, _nodes: &[LiveNode], _path: &[LiveId]) {}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw;
    fn walk(&mut self, _cx:&mut Cx) -> Walk {Walk::default()}
    fn redraw(&mut self, _cx: &mut Cx);
    
    fn is_visible(&self) -> bool {
        true
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d) -> WidgetDraw {
        let walk = self.walk(cx);
        self.draw_walk_widget(cx, walk)
    }
    
    fn draw_widget_all(&mut self, cx: &mut Cx2d) {
        while self.draw_widget(cx).is_hook() {};
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
    fn hook(arg: WidgetRef) -> WidgetDraw {Result::Err(arg)}
    fn hook_above() -> WidgetDraw {Result::Err(WidgetRef::empty())}
    fn is_done(&self) -> bool;
    fn is_hook(&self) -> bool;
    fn hook_widget(self) -> Option<WidgetRef>;
}

impl WidgetDrawApi for WidgetDraw {
    fn is_done(&self) -> bool {
        match *self {
            Result::Ok(_) => true,
            Result::Err(_) => false
        }
    }
    fn is_hook(&self) -> bool {
        match *self {
            Result::Ok(_) => false,
            Result::Err(_) => true
        }
    }
    fn hook_widget(self) -> Option<WidgetRef> {
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

#[derive(Clone, Default)]
pub struct WidgetRef(Rc<RefCell<Option<Box<dyn Widget >> >>);

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
                    inner.find_widgets(path, WidgetCache::Yes, &mut results);
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
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        for inner in self.iter() {
            let mut inner = inner.0.borrow_mut();
            if let Some(component) = &mut *inner {
                return component.apply(cx, from, index, nodes)
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
        Self (Rc::new(RefCell::new(Some(widget))))
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
    
    pub fn widget_to_data(&self, cx: &mut Cx, actions: &WidgetActions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget_to_data(cx, actions, nodes, path);
        }
        false
    }
    
    pub fn data_to_widget(&self, cx: &mut Cx, nodes: &[LiveNode], path: &[LiveId]) {
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
    
    pub fn widget(&self, path: &[LiveId]) -> WidgetRef {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget(path);
        }
        WidgetRef::empty()
    }
    
    pub fn widgets(&self, paths: &[&[LiveId]]) -> WidgetSet {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widgets(paths);
        }
        WidgetSet::default()
    }
    
    pub fn draw_walk_widget(&self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(nd) = inner.draw_walk_widget(cx, walk).hook_widget() {
                if nd.is_empty() {
                    return WidgetDraw::hook(self.clone())
                }
                return WidgetDraw::hook(nd);
            }
        }
        WidgetDraw::done()
    }
    
    pub fn walk(&self, cx:&mut Cx) -> Walk {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.walk(cx)
        }
        Walk::default()
    }
    
    // forwarding Widget trait
    pub fn redraw(&self, cx: &mut Cx) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.redraw(cx)
        }
    }
    
    
    pub fn is_visible(&self) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.is_visible()
        }
        true
    }
    
    pub fn draw_widget_all(&self, cx: &mut Cx2d) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.draw_widget_all(cx)
        }
    }
    
    pub fn draw_widget(&self, cx: &mut Cx2d) -> WidgetDraw {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(nd) = inner.draw_widget(cx).hook_widget() {
                if nd.is_empty() {
                    return WidgetDraw::hook(self.clone())
                }
                return WidgetDraw::hook(nd);
            }
        }
        WidgetDraw::done()
    }
    
    pub fn text(&self) -> String {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.text()
        }
        else {
            String::new()
        }
    }
    
    pub fn set_text(&self, v: &str) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.set_text(v)
        }
    }
    
    pub fn set_text_and_redraw(&self, cx: &mut Cx, v: &str) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.set_text_and_redraw(cx, v);
        }
    }
    
    pub fn borrow_mut<T: 'static + Widget>(&self) -> Option<std::cell::RefMut<'_, T >> {
        if let Ok(ret) = std::cell::RefMut::filter_map(self.0.borrow_mut(), | inner | {
            if let Some(inner) = inner.as_mut() {
                inner.cast_mut::<T>()
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
                inner.cast::<T>()
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
        self.apply(cx, ApplyFrom::ApplyOver, 0, nodes);
    }

    pub fn apply_over_and_redraw(&self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, ApplyFrom::ApplyOver, 0, nodes);
        self.redraw(cx);
    }
    
    fn apply(&self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut inner = self.0.borrow_mut();
        if let LiveValue::Class {live_type, ..} = nodes[index].value {
            if let Some(component) = &mut *inner {
                if component.type_id() != live_type {
                    *inner = None; // type changed, drop old component
                    log!("TYPECHANGE");
                }
                else {
                    return component.apply(cx, from, index, nodes);
                }
            }
            if let Some(component) = cx.live_registry.clone().borrow()
                .components.get::<WidgetRegistry>().new(cx, live_type) {
                    if cx.debug.marker() == 1{
                        panic!()
                    }
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

impl LiveHook for WidgetRef {}
impl LiveApply for WidgetRef {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        <WidgetRef>::apply(self, cx, from, index, nodes)
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
    pub container_uid: WidgetUid,
    pub item_uid: WidgetUid,
    pub action: Box<dyn WidgetAction>
}

impl Debug for WidgetActionItem {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        write!(f, "WidgetActionItem{{wiget_uid: {:?}, container_uid: {:?}, item_uid:{:?}}}", self.widget_uid, self.container_uid, self.item_uid)
    }
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
        if let Some(item) = self.find_single_action(widget_uid) {
            item.action.cast::<T>()
        }
        else {
            T::default()
        }
    }
    
    fn not_empty(&self) -> bool {
        self.len()>0
    }
}

impl WidgetActionItem {
    pub fn new(action: Box<dyn WidgetAction>, widget_uid: WidgetUid) -> Self {
        Self {
            container_uid: WidgetUid(0),
            item_uid: WidgetUid(0),
            widget_uid,
            action
        }
    }
    pub fn with_container(self, container_uid: WidgetUid) -> Self {
        Self {
            container_uid,
            ..self
        }
    }
    
    pub fn with_item(self, item_uid: WidgetUid) -> Self {
        Self {
            item_uid,
            ..self
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
