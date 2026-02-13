pub use crate::register_widget;
use {
    crate::makepad_draw::*,
    crate::widget_async::{ScriptAsyncId, ScriptAsyncResult},
    crate::widget_tree::CxWidgetExt,
    //crate::designer_data::DesignerDataToWidget,
    std::any::TypeId,
    std::cell::RefCell,
    std::collections::BTreeMap,
    std::fmt,
    std::fmt::{Debug, Error, Formatter},
    std::rc::Rc,
    std::sync::atomic::{AtomicU64, Ordering},
    std::sync::Arc,
};

static WIDGET_UID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WidgetUid(pub u64);

impl WidgetUid {
    pub fn new() -> Self {
        Self(WIDGET_UID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

pub trait WidgetDesign: WidgetNode {}

#[derive(Clone, Debug, Default)]
pub enum WidgetDesignAction {
    PickedBody,
    #[default]
    None,
}

pub trait WidgetNode: ScriptApply {
    fn widget_uid(&self) -> WidgetUid {
        WidgetUid(0)
    }
    fn widget_design(&mut self) -> Option<&mut dyn WidgetDesign> {
        return None;
    }
    /// Enumerate direct children for widget-tree indexing.
    fn children(&self, _visit: &mut dyn FnMut(LiveId, WidgetRef)) {}
    /// If true, global widget-tree search/flood will not traverse this node's descendants.
    /// The node is still indexed and can still be matched directly by name/path.
    fn skip_widget_tree_search(&self) -> bool {
        false
    }
    /// Find all widgets whose area contains the given point. Calls the closure for each found widget.
    fn find_widgets_from_point(&self, _cx: &Cx, _point: DVec2, _found: &mut dyn FnMut(&WidgetRef)) {
    }
    /// Find the first interactive widget at the given point, or None.
    fn find_interactive_widget_from_point(&self, cx: &Cx, point: DVec2) -> Option<WidgetRef> {
        let mut result = None;
        self.find_widgets_from_point(cx, point, &mut |widget| {
            if result.is_none() && widget.is_interactive() {
                result = Some(widget.clone());
            }
        });
        result
    }
    /// Whether this widget's area contains the given point. Override for widgets with
    /// multiple hit areas (e.g. TextFlowLink with drawn_areas).
    fn point_hits_area(&self, cx: &Cx, point: DVec2) -> bool {
        let area = self.area();
        area.is_valid(cx) && area.rect(cx).contains(point)
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk;
    fn area(&self) -> Area; //{return Area::Empty;}
    fn redraw(&mut self, _cx: &mut Cx);
    fn set_action_data(&mut self, _data: Arc<dyn ActionTrait>) {}
    fn action_data(&self) -> Option<Arc<dyn ActionTrait>> {
        None
    }

    fn set_visible(&mut self, _cx: &mut Cx, _visible: bool) {}
    fn visible(&self) -> bool {
        true
    }

    // Selection API - override for widgets that support text selection.
    // Containers should delegate to children (the derive macro does this
    // automatically for #[deref], #[wrap], and #[find] fields).
    fn selection_text_len(&self) -> usize {
        0
    }
    fn selection_point_to_char_index(&self, _cx: &Cx, _abs: DVec2) -> Option<usize> {
        None
    }
    fn selection_set(&mut self, _anchor: usize, _cursor: usize) {}
    fn selection_clear(&mut self) {}
    fn selection_select_all(&mut self) {}
    fn selection_get_text_for_range(&self, _start: usize, _end: usize) -> String {
        String::new()
    }
    fn selection_get_full_text(&self) -> String {
        String::new()
    }
}

pub trait Widget: WidgetNode {
    fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
        _sweep_area: Area,
    ) {
        self.handle_event(cx, event, scope)
    }
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn script_call(
        &mut self,
        _vm: &mut ScriptVm,
        _method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        ScriptAsyncResult::MethodNotFound
    }

    fn script_result(&mut self, _vm: &mut ScriptVm, _id: ScriptAsyncId, _result: ScriptValue) {}

    /// Whether this widget is interactive (wants mouse/touch events like hover, click).
    /// Defaults to true. Override to return false for non-interactive widgets.
    fn is_interactive(&self) -> bool {
        true
    }

    fn widget(&self, cx: &Cx, path: &[LiveId]) -> WidgetRef {
        let tree = cx.widget_tree();
        let uid = self.widget_uid();
        tree.find_within_from_borrowed(uid, path, |visit| self.children(visit))
    }

    fn widgets(&self, cx: &Cx, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        let tree = cx.widget_tree();
        let uid = self.widget_uid();
        tree.refresh_from_borrowed(uid, |visit| self.children(visit));
        for path in paths {
            results.0.extend(tree.find_all_within(uid, path));
        }
        results
    }

    /// Flood-fill search: find a widget by path, searching children first,
    /// then expanding outward through parents and their subtrees.
    fn widget_flood(&self, cx: &Cx, path: &[LiveId]) -> WidgetRef {
        let tree = cx.widget_tree();
        let uid = self.widget_uid();
        tree.find_flood_from_borrowed(uid, path, |visit| self.children(visit))
    }

    /// Flood-fill search returning all matches, ordered by proximity.
    fn widgets_flood(&self, cx: &Cx, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        let tree = cx.widget_tree();
        let uid = self.widget_uid();
        tree.refresh_from_borrowed(uid, |visit| self.children(visit));
        for path in paths {
            results.0.extend(tree.find_all_flood(uid, path));
        }
        results
    }

    fn draw_3d(&mut self, _cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d_all(&mut self, cx: &mut Cx3d, scope: &mut Scope) {
        while self.draw_3d(cx, scope).is_step() {}
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope) -> DrawStep {
        let walk = self.walk(cx);
        self.draw_walk(cx, scope, walk)
    }

    fn draw_walk_all(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) {
        while self.draw_walk(cx, scope, walk).is_step() {}
    }

    fn draw_all(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        while self.draw(cx, scope).is_step() {}
    }

    fn draw_unscoped(&mut self, cx: &mut Cx2d) -> DrawStep {
        self.draw(cx, &mut Scope::empty())
    }

    fn draw_all_unscoped(&mut self, cx: &mut Cx2d) {
        self.draw_all(cx, &mut Scope::empty());
    }

    fn text(&self) -> String {
        String::new()
    }

    fn set_text(&mut self, _cx: &mut Cx, _v: &str) {}

    fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.area())
    }

    fn key_focus(&self, cx: &Cx) -> bool {
        cx.has_key_focus(self.area())
    }

    fn set_disabled(&mut self, _cx: &mut Cx, _disabled: bool) {}

    fn disabled(&self, _cx: &Cx) -> bool {
        false
    }

    fn ref_cast_type_id(&self) -> TypeId
    where
        Self: 'static,
    {
        TypeId::of::<Self>()
    }

    fn ui_runner(&self) -> UiRunner<Self>
    where
        Self: Sized + 'static,
    {
        UiRunner::new(self.widget_uid().0 as usize)
    }
}

#[derive(Clone, Copy)]
pub enum CreateAt {
    Template,
    Begin,
    After(LiveId),
    Before(LiveId),
    End,
}

pub trait DrawStepApi {
    fn done() -> DrawStep {
        Result::Ok(())
    }
    fn make_step_here(arg: WidgetRef) -> DrawStep {
        Result::Err(arg)
    }
    fn make_step() -> DrawStep {
        Result::Err(WidgetRef::empty())
    }
    fn is_done(&self) -> bool;
    fn is_step(&self) -> bool;
    fn step(self) -> Option<WidgetRef>;
}

impl DrawStepApi for DrawStep {
    fn is_done(&self) -> bool {
        match *self {
            Result::Ok(_) => true,
            Result::Err(_) => false,
        }
    }
    fn is_step(&self) -> bool {
        match *self {
            Result::Ok(_) => false,
            Result::Err(_) => true,
        }
    }

    fn step(self) -> Option<WidgetRef> {
        match self {
            Result::Ok(_) => None,
            Result::Err(nd) => Some(nd),
        }
    }
}

pub type DrawStep = Result<(), WidgetRef>;

impl dyn Widget {
    pub fn is<T: Widget + 'static>(&self) -> bool {
        let t = std::any::TypeId::of::<T>();
        let concrete = self.ref_cast_type_id();
        t == concrete
    }
    pub fn downcast_ref<T: Widget + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const dyn Widget as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: Widget + 'static>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *const dyn Widget as *mut T) })
        } else {
            None
        }
    }
}

pub trait WidgetFactory: 'static {
    fn script_new(&self, vm: &mut ScriptVm) -> Box<dyn Widget>;
}

#[derive(Default)]
pub struct WidgetRegistry {
    pub map: BTreeMap<TypeId, (ComponentInfo, Box<dyn WidgetFactory>)>,
}

impl ComponentRegistry for WidgetRegistry {
    fn ref_cast_type_id(&self) -> TypeId {
        TypeId::of::<WidgetRegistry>()
    }

    fn component_type(&self) -> LiveId {
        live_id!(Widget)
    }

    fn get_component_info(&self, name: LiveId) -> Option<ComponentInfo> {
        self.map
            .values()
            .find(|(info, _)| info.name == name)
            .map(|(info, _)| info.clone())
    }
}

impl WidgetRegistry {
    pub fn can_script_new(&self, ty: TypeId) -> bool {
        self.map.contains_key(&ty)
    }

    pub fn script_new(&self, vm: &mut ScriptVm, ty: TypeId) -> Option<Box<dyn Widget>> {
        self.map.get(&ty).map(|(_, fac)| fac.script_new(vm))
    }
}

pub struct WidgetRefInner {
    pub widget: Box<dyn Widget>,
}
#[derive(Clone, Default)]
pub struct WidgetRef(Rc<RefCell<Option<WidgetRefInner>>>);

impl Debug for WidgetRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "WidgetRef {}", self.widget_uid().0)
    }
}

#[derive(Default, Clone, Debug)]
pub struct WidgetSet(pub SmallVec<[WidgetRef; 2]>);

impl WidgetSet {
    pub fn is_empty(&mut self) -> bool {
        self.0.len() == 0
    }

    pub fn push(&mut self, item: WidgetRef) {
        self.0.push(item);
    }

    pub fn extend_from_set(&mut self, other: &WidgetSet) {
        for item in other.iter() {
            self.0.push(item.clone())
        }
    }

    pub fn into_first(self) -> WidgetRef {
        for item in self.0 {
            return item;
        }
        WidgetRef::empty()
    }

    pub fn widgets(&self, cx: &Cx, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        let tree = cx.widget_tree();
        for widget in &self.0 {
            tree.seed_from_widget(widget.clone());
            let uid = widget.widget_uid();
            for path in paths {
                results.0.extend(tree.find_all_within(uid, path));
            }
        }
        results
    }

    pub fn contains(&self, widget: &WidgetRef) -> bool {
        for item in &self.0 {
            if *item == *widget {
                return true;
            }
        }
        false
    }
}
/*
impl LiveHook for WidgetSet {}
impl LiveApply for WidgetSet {
    fn apply(&mut self, cx: &mut Cx, apply: &Apply, index: usize, nodes: &[LiveNode]) -> usize {
        for inner in &self.0 {
            let mut inner = inner.0.borrow_mut();
            if let Some(component) = &mut *inner {
                return component.widget.apply(cx, apply, index, nodes);
            }
        }
        nodes.skip_node(index)
    }
}*/

impl WidgetSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn set_text(&self, cx: &mut Cx, v: &str) {
        for item in &self.0 {
            item.set_text(cx, v)
        }
    }

    pub fn iter(&self) -> WidgetSetIterator<'_> {
        return WidgetSetIterator {
            widget_set: self,
            index: 0,
        };
    }

    pub fn filter_actions<'a>(
        &'a self,
        actions: &'a Actions,
    ) -> impl Iterator<Item = &'a WidgetAction> {
        actions.filter_widget_actions_set(self)
    }
}

pub struct WidgetSetIterator<'a> {
    widget_set: &'a WidgetSet,
    index: usize,
}

impl<'a> Iterator for WidgetSetIterator<'a> {
    // We can refer to this type using Self::Item
    type Item = &'a WidgetRef;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.widget_set.0.len() {
            let idx = self.index;
            self.index += 1;
            return Some(&self.widget_set.0[idx]);
        }
        None
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
pub trait OptionWidgetRefExt {
    fn into_ref(self) -> WidgetRef;
}
impl OptionWidgetRefExt for Option<WidgetRef> {
    fn into_ref(self) -> WidgetRef {
        if let Some(v) = self {
            return v;
        } else {
            WidgetRef::empty()
        }
    }
}

impl WidgetRef {
    pub fn value_is_newable_widget(vm: &mut ScriptVm, value: ScriptValue) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };
        let Some(type_id) = vm.bx.heap.object_type_id(obj) else {
            return false;
        };

        vm.cx()
            .components
            .get::<WidgetRegistry>()
            .can_script_new(type_id)
    }

    pub fn into_option(self) -> Option<WidgetRef> {
        if self.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    pub fn empty() -> Self {
        Self(Rc::new(RefCell::new(None)))
    }

    pub fn is_empty(&self) -> bool {
        match self.0.try_borrow() {
            Ok(r) => r.as_ref().is_none(),
            Err(_) => false, // actively borrowed means not empty
        }
    }

    pub fn new_with_inner(widget: Box<dyn Widget>) -> Self {
        Self(Rc::new(RefCell::new(Some(WidgetRefInner { widget }))))
    }
    /// ## handle event with a sweep area
    ///
    /// this is used for the sweep event, this fn can help to pass the event into popup,
    /// the widget should implement the `handle_event_with` fn in `impl Widget for $Widget`
    ///
    /// ### Example
    /// ```rust
    /// impl Widget for Button {
    /// fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope, sweep_area: Area) {
    ///     let uid = self.widget_uid();
    ///
    ///     if self.animator_handle_event(cx, event).must_redraw() {
    ///         self.draw_button.redraw(cx);
    ///     }
    ///     match event.hits_with_options(cx, self.draw_button.area(), HitOptions::new().with_sweep_area(sweep_area) ) {
    ///         Hit::FingerDown(f_down) => {
    ///             if self.grab_key_focus {
    ///                  cx.set_key_focus(self.sweep_area);
    ///             }
    ///             cx.widget_action(uid, &scope.path, GButtonEvent::Pressed(f_down.modifiers));
    ///             self.animator_play(cx, ids!(hover.pressed));
    ///         }
    ///         _ =>()
    ///     }
    /// }
    /// ```
    /// ### Details
    /// See [Flexible Popup](https://palpus-rs.github.io/Gen-UI.github.io/makepad/code/widgets/flexible_popup.html)
    pub fn handle_event_with(
        &self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
        sweep_area: Area,
    ) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.handle_event_with(cx, event, scope, sweep_area)
        }
    }

    pub fn handle_event(&self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.handle_event(cx, event, scope);
        }
    }

    pub fn script_call(
        &self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.script_call(vm, method, args);
        }
        ScriptAsyncResult::MethodNotFound
    }

    pub fn script_result(&self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.script_result(vm, id, result);
        }
    }

    /// Returns the unique ID (UID) of this widget.
    ///
    /// Returns `WidgetUid(0)` if the widget is currently borrowed or is empty.
    pub fn widget_uid(&self) -> WidgetUid {
        self.try_widget_uid().unwrap_or(WidgetUid(0))
    }

    pub fn try_widget_uid(&self) -> Option<WidgetUid> {
        self.0
            .try_borrow()
            .ok()
            .and_then(|r| r.as_ref().map(|w| w.widget.widget_uid()))
    }

    pub fn area(&self) -> Area {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget.area();
        }
        Area::Empty
    }
    /*
       pub fn widget_to_data(
           &self,
           cx: &mut Cx,
           actions: &Actions,
           nodes: &mut LiveNodeVec,
           path: &[LiveId],
       ) -> bool {
           if let Some(inner) = self.0.borrow_mut().as_mut() {
               return inner.widget.widget_to_data(cx, actions, nodes, path);
           }
           false
       }
    */
    pub fn set_action_data<T: ActionTrait + PartialEq>(&self, data: T) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(v) = inner.widget.action_data() {
                if let Some(v) = v.downcast_ref::<T>() {
                    if v.ne(&data) {
                        inner.widget.set_action_data(Arc::new(data));
                    }
                }
            } else {
                inner.widget.set_action_data(Arc::new(data));
            }
        }
    }

    pub fn set_action_data_always<T: ActionTrait>(&self, data: T) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.set_action_data(Arc::new(data));
        }
    }
    /*
        pub fn data_to_widget(&self, cx: &mut Cx, nodes: &[LiveNode], path: &[LiveId]) {
            if let Some(inner) = self.0.borrow_mut().as_mut() {
                inner.widget.data_to_widget(cx, nodes, path);
            }
        }
    */
    pub fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        let _ = self.try_children(visit);
    }

    pub fn try_children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        let Ok(inner) = self.0.try_borrow() else {
            return false;
        };
        if let Some(inner) = inner.as_ref() {
            inner.widget.children(visit);
        }
        true
    }

    pub fn skip_widget_tree_search(&self) -> bool {
        self.0
            .try_borrow()
            .ok()
            .and_then(|r| r.as_ref().map(|w| w.widget.skip_widget_tree_search()))
            .unwrap_or(false)
    }

    pub fn find_widgets_from_point(
        &self,
        cx: &Cx,
        point: DVec2,
        found: &mut dyn FnMut(&WidgetRef),
    ) {
        if let Some(inner) = self.0.borrow().as_ref() {
            if inner.widget.point_hits_area(cx, point) {
                found(self);
            }
            inner.widget.find_widgets_from_point(cx, point, found)
        }
    }

    pub fn point_hits_area(&self, cx: &Cx, point: DVec2) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.point_hits_area(cx, point)
        } else {
            false
        }
    }

    pub fn find_interactive_widget_from_point(&self, cx: &Cx, point: DVec2) -> Option<WidgetRef> {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.find_interactive_widget_from_point(cx, point)
        } else {
            None
        }
    }

    pub fn is_interactive(&self) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.is_interactive()
        } else {
            false
        }
    }

    pub fn selection_text_len(&self) -> usize {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.selection_text_len()
        } else {
            0
        }
    }

    pub fn selection_point_to_char_index(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.selection_point_to_char_index(cx, abs)
        } else {
            None
        }
    }

    pub fn selection_set(&self, anchor: usize, cursor: usize) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.selection_set(anchor, cursor)
        }
    }

    pub fn selection_clear(&self) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.selection_clear()
        }
    }

    pub fn selection_select_all(&self) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.selection_select_all()
        }
    }

    pub fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.selection_get_text_for_range(start, end)
        } else {
            String::new()
        }
    }

    pub fn selection_get_full_text(&self) -> String {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.selection_get_full_text()
        } else {
            String::new()
        }
    }

    pub fn widget(&self, cx: &Cx, path: &[LiveId]) -> WidgetRef {
        let tree = cx.widget_tree();
        tree.seed_from_widget(self.clone());
        if let Ok(inner) = self.0.try_borrow() {
            if let Some(inner) = inner.as_ref() {
                let uid = inner.widget.widget_uid();
                if uid != WidgetUid(0) {
                    tree.refresh_from_borrowed(uid, |visit| inner.widget.children(visit));
                    return tree.find_within(uid, path);
                }
            } else {
                return WidgetRef::empty();
            }
        }
        tree.find_within(self.widget_uid(), path)
    }

    pub fn widgets(&self, cx: &Cx, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        let tree = cx.widget_tree();
        tree.seed_from_widget(self.clone());
        let mut uid = self.widget_uid();
        if let Ok(inner) = self.0.try_borrow() {
            if let Some(inner) = inner.as_ref() {
                let inner_uid = inner.widget.widget_uid();
                if inner_uid != WidgetUid(0) {
                    tree.refresh_from_borrowed(inner_uid, |visit| inner.widget.children(visit));
                    uid = inner_uid;
                }
            } else {
                return results;
            }
        }
        for path in paths {
            results.0.extend(tree.find_all_within(uid, path));
        }
        results
    }

    pub fn widget_set(&self, cx: &Cx, paths: &[&[LiveId]]) -> WidgetSet {
        self.widgets(cx, paths)
    }

    pub fn widget_flood(&self, cx: &Cx, path: &[LiveId]) -> WidgetRef {
        let tree = cx.widget_tree();
        tree.seed_from_widget(self.clone());
        if let Ok(inner) = self.0.try_borrow() {
            if let Some(inner) = inner.as_ref() {
                let uid = inner.widget.widget_uid();
                if uid != WidgetUid(0) {
                    tree.refresh_from_borrowed(uid, |visit| inner.widget.children(visit));
                    return tree.find_flood(uid, path);
                }
            } else {
                return WidgetRef::empty();
            }
        }
        tree.find_flood(self.widget_uid(), path)
    }

    pub fn widgets_flood(&self, cx: &Cx, paths: &[&[LiveId]]) -> WidgetSet {
        let mut results = WidgetSet::default();
        let tree = cx.widget_tree();
        tree.seed_from_widget(self.clone());
        let mut uid = self.widget_uid();
        if let Ok(inner) = self.0.try_borrow() {
            if let Some(inner) = inner.as_ref() {
                let inner_uid = inner.widget.widget_uid();
                if inner_uid != WidgetUid(0) {
                    tree.refresh_from_borrowed(inner_uid, |visit| inner.widget.children(visit));
                    uid = inner_uid;
                }
            } else {
                return results;
            }
        }
        for path in paths {
            results.0.extend(tree.find_all_flood(uid, path));
        }
        results
    }

    pub fn draw_walk(&self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(nd) = inner.widget.draw_walk(cx, scope, walk).step() {
                if nd.is_empty() {
                    return DrawStep::make_step_here(self.clone());
                }
                return DrawStep::make_step_here(nd);
            }
        }
        DrawStep::done()
    }

    pub fn draw_walk_all(&self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.draw_walk_all(cx, scope, walk)
        }
    }

    pub fn draw_3d_all(&self, cx: &mut Cx3d, scope: &mut Scope) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.draw_3d_all(cx, scope)
        }
    }

    pub fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(nd) = inner.widget.draw_3d(cx, scope).step() {
                if nd.is_empty() {
                    return DrawStep::make_step_here(self.clone());
                }
                return DrawStep::make_step_here(nd);
            }
        }
        DrawStep::done()
    }

    pub fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope) -> DrawStep {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(nd) = inner.widget.draw(cx, scope).step() {
                if nd.is_empty() {
                    return DrawStep::make_step_here(self.clone());
                }
                return DrawStep::make_step_here(nd);
            }
        }
        DrawStep::done()
    }

    pub fn draw_unscoped(&mut self, cx: &mut Cx2d) -> DrawStep {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            if let Some(nd) = inner.widget.draw(cx, &mut Scope::empty()).step() {
                if nd.is_empty() {
                    return DrawStep::make_step_here(self.clone());
                }
                return DrawStep::make_step_here(nd);
            }
        }
        DrawStep::done()
    }

    pub fn walk(&self, cx: &mut Cx) -> Walk {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.walk(cx);
        }
        Walk::default()
    }

    // forwarding Widget trait
    pub fn redraw(&self, cx: &mut Cx) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.redraw(cx);
        }
    }

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.set_visible(cx, visible);
        }
    }

    pub fn visible(&self) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget.visible();
        }
        true
    }

    pub fn text(&self) -> String {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.text()
        } else {
            String::new()
        }
    }

    pub fn set_text(&self, cx: &mut Cx, v: &str) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.set_text(cx, v)
        }
    }

    pub fn key_focus(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.key_focus(cx)
        } else {
            false
        }
    }

    pub fn set_key_focus(&self, cx: &mut Cx) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.widget.set_key_focus(cx)
        }
    }

    pub fn set_disabled(&self, cx: &mut Cx, disabled: bool) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.set_disabled(cx, disabled);
        }
    }

    pub fn disabled(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget.disabled(cx);
        }
        true
    }

    pub fn draw_all(&self, cx: &mut Cx2d, scope: &mut Scope) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.draw_all(cx, scope);
        }
    }

    pub fn action_data(&self) -> Option<Arc<dyn ActionTrait>> {
        if let Some(inner) = self.0.borrow().as_ref() {
            return inner.widget.action_data();
        }
        None
    }

    pub fn filter_actions<'a>(
        &'a self,
        actions: &'a Actions,
    ) -> impl Iterator<Item = &'a WidgetAction> {
        actions.filter_widget_actions(self.widget_uid())
    }

    pub fn draw_all_unscoped(&self, cx: &mut Cx2d) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            return inner.widget.draw_all_unscoped(cx);
        }
    }

    pub fn borrow_mut<T: 'static + Widget>(&self) -> Option<std::cell::RefMut<'_, T>> {
        if let Ok(ret) = std::cell::RefMut::filter_map(self.0.borrow_mut(), |inner| {
            if let Some(inner) = inner.as_mut() {
                inner.widget.downcast_mut::<T>()
            } else {
                None
            }
        }) {
            Some(ret)
        } else {
            None
        }
    }

    pub fn borrow<T: 'static + Widget>(&self) -> Option<std::cell::Ref<'_, T>> {
        if let Ok(ret) = std::cell::Ref::filter_map(self.0.borrow(), |inner| {
            if let Some(inner) = inner.as_ref() {
                inner.widget.downcast_ref::<T>()
            } else {
                None
            }
        }) {
            Some(ret)
        } else {
            None
        }
    }

    fn script_apply(
        &self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        let mut inner = self.0.borrow_mut();

        // Get the TypeId from the value if it's an object with a registered type
        let type_id = if let Some(obj) = value.as_object() {
            vm.bx.heap.object_type_id(obj)
        } else {
            None
        };

        // Check if we already have a widget
        if let Some(component) = &mut *inner {
            if let Some(type_id) = type_id {
                // We have a type_id from the value - check if it matches
                if component.widget.ref_cast_type_id() != type_id {
                    // Type changed, drop old component
                    *inner = None;
                } else {
                    // Type matches, apply to existing widget and redraw
                    component.widget.script_apply(vm, apply, scope, value);
                    let cx = vm.cx_mut();
                    component.widget.redraw(cx);
                    return;
                }
            } else {
                // No type_id in value, apply to existing widget anyway
                component.widget.script_apply(vm, apply, scope, value);
                let cx = vm.cx_mut();
                component.widget.redraw(cx);
                return;
            }
        }

        // If we have a type_id, create a new widget via the registry
        if let Some(type_id) = type_id {
            // Clone the components Rc to avoid borrow issues with vm
            let components = vm.cx_mut().components.clone();

            // Get the WidgetRegistry and create a new widget
            // Separate statement to ensure Ref is dropped before components
            let new_widget = components.get::<WidgetRegistry>().script_new(vm, type_id);

            if let Some(new_widget) = new_widget {
                *inner = Some(WidgetRefInner { widget: new_widget });

                // Apply value to the new widget and redraw
                if let Some(component) = &mut *inner {
                    component.widget.script_apply(vm, apply, scope, value);
                    let cx = vm.cx_mut();
                    component.widget.redraw(cx);
                }
            }
        }
    }
}

impl ScriptHook for WidgetRef {}
impl ScriptApply for WidgetRef {
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        <WidgetRef>::script_apply(self, vm, apply, scope, value)
    }

    fn script_source(&self) -> ScriptObject {
        if let Some(inner) = self.0.borrow().as_ref() {
            inner.widget.script_source()
        } else {
            ScriptObject::ZERO
        }
    }
}

impl ScriptNew for WidgetRef {
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Self(Rc::new(RefCell::new(None)))
    }

    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        // WidgetRef is a polymorphic container that can hold any widget type.
        // Accept nil (for empty widget refs) or any object.
        // The actual widget type validation happens at apply time when we
        // look up the type in the WidgetRegistry.
        value.is_nil() || value.is_object()
    }
}

pub trait WidgetActionTrait: 'static + Send + Sync {
    fn ref_cast_type_id(&self) -> TypeId;
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
    fn box_clone(&self) -> Box<dyn WidgetActionTrait>;
}

pub trait ActionDefault {
    fn default_ref(&self) -> Box<dyn WidgetActionTrait>;
}

impl<T: 'static + ?Sized + Clone + Debug + Send + Sync> WidgetActionTrait for T {
    fn ref_cast_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn box_clone(&self) -> Box<dyn WidgetActionTrait> {
        Box::new((*self).clone())
    }
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt(f)
    }
}

impl Debug for dyn WidgetActionTrait {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.debug_fmt(f)
    }
}

impl dyn WidgetActionTrait {
    pub fn is<T: WidgetActionTrait + 'static>(&self) -> bool {
        let t = std::any::TypeId::of::<T>();
        let concrete = self.ref_cast_type_id();
        t == concrete
    }
    pub fn downcast_ref<T: WidgetActionTrait + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const dyn WidgetActionTrait as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: WidgetActionTrait + 'static>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *const dyn WidgetActionTrait as *mut T) })
        } else {
            None
        }
    }
}

impl Clone for Box<dyn WidgetActionTrait> {
    fn clone(&self) -> Box<dyn WidgetActionTrait> {
        self.as_ref().box_clone()
    }
}

#[derive(Default)]
pub struct WidgetActionData {
    data: Option<Arc<dyn ActionTrait>>,
}

impl WidgetActionData {
    pub fn set(&mut self, data: impl ActionTrait) {
        self.data = Some(Arc::new(data));
    }

    pub fn set_box(&mut self, data: Arc<dyn ActionTrait>) {
        self.data = Some(data);
    }

    pub fn clone_data(&self) -> Option<Arc<dyn ActionTrait>> {
        self.data.clone()
    }
}

/// An action emitted by another widget via the `widget_action()` method.
#[derive(Clone, Debug)]
pub struct WidgetAction {
    /// Extra data that can be stored on a widget at draw time,
    /// and then cheaply cloned to be emitted as part of an action.
    ///
    /// To attach data to a widget action, use the `widget_action_with_data()` method.
    pub data: Option<Arc<dyn ActionTrait>>,
    /// The emitted action object itself, which acts as a dyn Any-like.
    pub action: Box<dyn WidgetActionTrait>,
    /// The UID of the widget that emitted this action.
    pub widget_uid: WidgetUid,
    /// Used by list-like widgets (e.g., PortalList) to mark a group-uid around item-actions.
    pub group: Option<WidgetActionGroup>,
}

#[derive(Clone, Debug)]
pub struct WidgetActionGroup {
    pub group_uid: WidgetUid,
    pub item_uid: WidgetUid,
}

pub trait WidgetActionCxExt {
    fn widget_action(&mut self, uid: WidgetUid, t: impl WidgetActionTrait);
    fn widget_action_with_data(
        &mut self,
        action_data: &WidgetActionData,
        widget_uid: WidgetUid,
        t: impl WidgetActionTrait,
    );
    fn group_widget_actions<F, R>(&mut self, group_id: WidgetUid, item_id: WidgetUid, f: F) -> R
    where
        F: FnOnce(&mut Cx) -> R;
}

pub trait WidgetActionsApi {
    fn find_widget_action_cast<T: WidgetActionTrait>(&self, widget_uid: WidgetUid) -> T
    where
        T: Default + Clone;
    fn find_widget_action(&self, widget_uid: WidgetUid) -> Option<&WidgetAction>;
    /// ## Filter all actions by widget uid
    /// this function use to filter all actions from `Event::Actions(actions)`,
    /// if multi actions in same widget may happened in the same time, this function will help you get all
    /// and back an Iter
    /// ### Attention
    /// **If you want to focus on target actions and need to cast directly use `filter_widget_action_cast`**
    /// ### Examples
    /// #### find and directly do target action without param
    /// you can `filter_widget_actions` and then do find to get target action you want,
    /// then do map to do want you what
    /// ```rust
    /// let actions = cx.capture_actions(|cx| self.super_widget.handle_event(cx, event, scope));
    ///
    /// self.gbutton(ids!(auto_connect)).borrow().map(|x| {
    ///     let mut actions = actions.filter_widget_actions(x.widget_uid());
    ///     actions.find(|action| {
    ///         if let GButtonEvent::Clicked(_) = action.cast(){
    ///             true
    ///         }else{
    ///             false
    ///         }
    ///     }).map(|action|{
    ///         dbg!(action);
    ///     });
    /// });
    /// ```
    /// #### find and cast
    /// ```rust
    /// let actions = cx.capture_actions(|cx| self.super_widget.handle_event(cx, event, scope));
    ///
    /// self.gbutton(ids!(auto_connect)).borrow().map(|x| {
    /// let actions = actions.filter_widget_actions(x.widget_uid());
    ///     actions.for_each(|action| {
    ///         if let GButtonEvent::Clicked(param) = action.cast(){
    ///             dbg!(param);
    ///         }
    ///     })
    /// });
    /// ```
    fn filter_widget_actions(&self, widget_uid: WidgetUid) -> impl Iterator<Item = &WidgetAction>;
    /// ## Filter widget actions by widget id and cast
    /// this function can help you cast the widget actions to the widget you want, the diff is:
    /// - try cast all widget actions (This method is not recommended when a large number of actions occur simultaneously)
    /// - back `Iterator<Item = T>` not `Iterator<Item = &T>`
    /// ### Example
    /// ```rust
    /// self.gbutton(ids!(auto_connect)).borrow().map(|x| {
    /// let actions = actions.filter_widget_actions_cast::<GButtonEvent>(x.widget_uid());
    ///     actions.for_each(|action| {
    ///         if let GButtonEvent::Clicked(param) = action{
    ///             dbg!(param);
    ///         }
    ///     })
    /// });
    /// ```
    fn filter_widget_actions_cast<T: WidgetActionTrait>(
        &self,
        widget_uid: WidgetUid,
    ) -> impl Iterator<Item = T>
    where
        T: Default + Clone;

    fn filter_actions_data<T: ActionTrait>(&self) -> impl Iterator<Item = &T>
    where
        T: Clone;

    fn filter_widget_actions_set(&self, set: &WidgetSet) -> impl Iterator<Item = &WidgetAction>;
}

pub trait WidgetActionOptionApi {
    fn widget_uid_eq(&self, widget_uid: WidgetUid) -> Option<&WidgetAction>;
    fn cast<T: WidgetActionTrait>(&self) -> T
    where
        T: Default + Clone;
    fn cast_ref<T: WidgetActionTrait + ActionDefaultRef>(&self) -> &T;
}

impl WidgetActionOptionApi for Option<&WidgetAction> {
    fn widget_uid_eq(&self, widget_uid: WidgetUid) -> Option<&WidgetAction> {
        if let Some(item) = self {
            if item.widget_uid == widget_uid {
                return Some(item);
            }
        }
        None
    }

    fn cast<T: WidgetActionTrait>(&self) -> T
    where
        T: Default + Clone,
    {
        if let Some(item) = self {
            if let Some(item) = item.action.downcast_ref::<T>() {
                return item.clone();
            }
        }
        T::default()
    }

    fn cast_ref<T: WidgetActionTrait + ActionDefaultRef>(&self) -> &T {
        if let Some(item) = self {
            if let Some(item) = item.action.downcast_ref::<T>() {
                return item;
            }
        }
        T::default_ref()
    }
}

pub trait WidgetActionCast {
    fn as_widget_action(&self) -> Option<&WidgetAction>;
}

impl WidgetActionCast for Action {
    fn as_widget_action(&self) -> Option<&WidgetAction> {
        self.downcast_ref::<WidgetAction>()
    }
}

impl WidgetActionsApi for Actions {
    fn find_widget_action(&self, widget_uid: WidgetUid) -> Option<&WidgetAction> {
        for action in self {
            if let Some(action) = action.downcast_ref::<WidgetAction>() {
                if action.widget_uid == widget_uid {
                    return Some(action);
                }
            }
        }
        None
    }

    fn find_widget_action_cast<T: WidgetActionTrait + 'static + Send>(
        &self,
        widget_uid: WidgetUid,
    ) -> T
    where
        T: Default + Clone,
    {
        if let Some(item) = self.find_widget_action(widget_uid) {
            if let Some(item) = item.action.downcast_ref::<T>() {
                return item.clone();
            }
        }
        T::default()
    }

    fn filter_widget_actions(&self, widget_uid: WidgetUid) -> impl Iterator<Item = &WidgetAction> {
        self.iter().filter_map(move |action| {
            action
                .downcast_ref::<WidgetAction>()
                .and_then(|action| (action.widget_uid == widget_uid).then_some(action))
        })
    }

    fn filter_widget_actions_cast<T: WidgetActionTrait>(
        &self,
        widget_uid: WidgetUid,
    ) -> impl Iterator<Item = T>
    where
        T: Default + Clone,
    {
        self.filter_widget_actions(widget_uid).map(|action| {
            if let Some(a) = action.action.downcast_ref::<T>() {
                a.clone()
            } else {
                T::default()
            }
        })
    }

    fn filter_actions_data<T: ActionTrait>(&self) -> impl Iterator<Item = &T> {
        self.iter().filter_map(move |action| {
            action.downcast_ref::<WidgetAction>().and_then(|action| {
                if let Some(a) = &action.data {
                    if let Some(a) = a.downcast_ref::<T>() {
                        Some(a)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
    }

    fn filter_widget_actions_set(&self, set: &WidgetSet) -> impl Iterator<Item = &WidgetAction> {
        self.iter().filter_map(move |action| {
            action.downcast_ref::<WidgetAction>().and_then(|action| {
                (set.iter().any(|w| action.widget_uid == w.widget_uid())).then_some(action)
            })
        })
    }
}

impl WidgetActionCxExt for Cx {
    fn widget_action(&mut self, widget_uid: WidgetUid, t: impl WidgetActionTrait) {
        self.action(WidgetAction {
            widget_uid,
            data: None,
            action: Box::new(t),
            group: None,
        })
    }

    fn widget_action_with_data(
        &mut self,
        action_data: &WidgetActionData,
        widget_uid: WidgetUid,
        t: impl WidgetActionTrait,
    ) {
        self.action(WidgetAction {
            widget_uid,
            data: action_data.clone_data(),
            action: Box::new(t),
            group: None,
        })
    }

    fn group_widget_actions<F, R>(&mut self, group_uid: WidgetUid, item_uid: WidgetUid, f: F) -> R
    where
        F: FnOnce(&mut Cx) -> R,
    {
        self.mutate_actions(
            |cx| f(cx),
            |actions| {
                for action in actions {
                    if let Some(action) = action.downcast_mut::<WidgetAction>() {
                        if action.group.is_none() {
                            action.group = Some(WidgetActionGroup {
                                group_uid,
                                item_uid,
                            })
                        }
                    }
                }
            },
        )
    }
}

impl WidgetAction {
    pub fn cast<T: WidgetActionTrait + 'static + Send>(&self) -> T
    where
        T: Default + Clone,
    {
        if let Some(item) = self.action.downcast_ref::<T>() {
            return item.clone();
        }
        T::default()
    }

    pub fn cast_ref<T: WidgetActionTrait + 'static + Send + ActionDefaultRef>(&self) -> &T {
        if let Some(item) = self.action.downcast_ref::<T>() {
            return item;
        }
        T::default_ref()
    }

    pub fn downcast_ref<T: WidgetActionTrait + Send + ActionDefaultRef>(&self) -> Option<&T> {
        self.action.downcast_ref::<T>()
    }
}

pub struct DrawStateWrap<T: Clone> {
    state: Option<T>,
    redraw_id: u64,
}

impl<T: Clone> Default for DrawStateWrap<T> {
    fn default() -> Self {
        Self {
            state: None,
            redraw_id: 0,
        }
    }
}

impl<T: Clone> DrawStateWrap<T> {
    pub fn begin(&mut self, cx: &mut CxDraw, init: T) -> bool {
        if self.redraw_id != cx.redraw_id() {
            self.redraw_id = cx.redraw_id();
            self.state = Some(init);
            true
        } else {
            false
        }
    }

    pub fn begin_with<F, S>(&mut self, cx: &mut CxDraw, v: &S, init: F) -> bool
    where
        F: FnOnce(&mut CxDraw, &S) -> T,
    {
        if self.redraw_id != cx.redraw_id() {
            self.redraw_id = cx.redraw_id();
            self.state = Some(init(cx, v));
            true
        } else {
            false
        }
    }

    pub fn begin_state(&mut self, cx: &mut Cx) -> Option<&mut Option<T>> {
        if self.redraw_id != cx.redraw_id() {
            self.redraw_id = cx.redraw_id();
            Some(&mut self.state)
        } else {
            None
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

pub trait WidgetRegister {
    fn register_widget(vm: &mut ScriptVm) -> ScriptValue;
}

#[macro_export]
macro_rules! register_widget {
    ( $ cx: expr, $ ty: ty) => {{
        struct Factory();
        impl WidgetFactory for Factory {
            fn script_new(&self, vm: &mut ScriptVm) -> Box<dyn Widget> {
                Box::new(<$ty>::script_new(vm))
            }
        }

        let cx = $cx;
        $crate::widget_async::ensure_widget_async_hooks_registered(cx);
        let type_id = std::any::TypeId::of::<$ty>();
        let name = $crate::LiveId::from_str_with_lut(stringify!($ty)).unwrap();

        cx.components
            .get_or_create::<$crate::WidgetRegistry>()
            .map
            .insert(
                type_id,
                ($crate::ComponentInfo { name }, Box::new(Factory())),
            );
    }};
}
