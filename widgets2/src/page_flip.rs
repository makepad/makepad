use crate::{
    widget::*,
    makepad_derive_widget::*,
    makepad_draw::*,
};

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.PageFlipBase = #(PageFlip::register_widget(vm))
    mod.widgets.PageFlip = mod.widgets.PageFlipBase{}
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct PageFlip {
    #[source] source: ScriptObjectRef,
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[live(false)] lazy_init: bool,
    #[live] active_page: LiveId,
    #[rust] draw_state: DrawStateWrap<Walk>,
    #[rust] templates: ComponentMap<LiveId, ScriptObjectRef>,
    #[rust] pages: ComponentMap<LiveId, WidgetRef>,
}

impl ScriptHook for PageFlip {
    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }
    
    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        // Handle $prop children from the object's vec (these are our page templates)
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if kv.key.is_prefixed_id() {  // $prop children are our page templates
                            if let Some(id) = kv.key.as_id() {
                                if let Some(template_obj) = kv.value.as_object() {
                                    self.templates.insert(id, vm.bx.heap.new_object_ref(template_obj));
                                }
                                
                                // If we already have this page instantiated, apply updates to it
                                if let Some(page) = self.pages.get_mut(&id) {
                                    page.script_apply(vm, apply, scope, kv.value);
                                }
                            }
                        }
                    }
                });
            }
        }
        
        // If not lazy_init, create all pages upfront
        if !self.lazy_init && (apply.is_new() || apply.is_reload()) {
            for (page_id, template_ref) in self.templates.iter() {
                if !self.pages.contains_key(page_id) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    let page = WidgetRef::script_from_value_scoped(vm, scope, template_value);
                    self.pages.insert(*page_id, page);
                }
            }
        }
    }
}

impl PageFlip {
    /// Returns the widget for the given page templated ID, creating it if necessary.
    pub fn page(&mut self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        if let Some(template_ref) = self.templates.get(&page_id) {
            let template_value: ScriptValue = template_ref.as_object().into();
            if !self.pages.contains_key(&page_id) {
                let page = cx.with_vm(|vm| {
                    WidgetRef::script_from_value(vm, template_value)
                });
                self.pages.insert(page_id, page);
            }
            self.pages.get(&page_id).cloned()
        } else {
            error!("Template not found: {page_id}. Did you add it to the <PageFlip> instance in `live_design!{{}}`?");
            None
        }
    }
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area);
    }
}

impl WidgetNode for PageFlip {
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    
    fn area(&self) -> Area {
        self.area
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }
        
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        if let Some(page) = self.pages.get(&path[0]) {
            if path.len() == 1 {
                results.push(page.clone());
            } else {
                page.find_widgets(&path[1..], cached, results);
            }
        }
        for page in self.pages.values() {
            page.find_widgets(path, cached, results);
        }
    }
    
    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        for page in self.pages.values() {
            let x = page.uid_to_widget(uid);
            if !x.is_empty() {
                return x
            }
        }
        WidgetRef::empty()
    }

    fn widget_tree_walk(&self, nodes: &mut Vec<WidgetTreeNode>) {
        for (id, page) in self.pages.iter() {
            page.widget_tree_walk_named(*id, nodes);
        }
    }
}        

impl Widget for PageFlip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if event.requires_visibility() {
            if let Some(page) = self.pages.get_mut(&self.active_page) {
                let item_uid = page.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx| {
                    page.handle_event(cx, event, scope)
                });
            }
        } else {
            for page in self.pages.values() {
                let item_uid = page.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx| {
                    page.handle_event(cx, event, scope)
                });
            }
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(page) = self.page(cx, self.active_page) {
            if self.draw_state.begin_with(cx, &(), |cx, _| {
                page.walk(cx)
            }) {
                self.begin(cx, walk);
            }
            if let Some(walk) = self.draw_state.get() {
                page.draw_walk(cx, scope, walk)?;
            }
            self.end(cx);
        } else {
            self.begin(cx, walk);
            self.end(cx);
        }
        DrawStep::done()
    }
}

impl PageFlip {
    /// Sets the active page of the PageFlip widget, creating it if necessary.
    ///
    /// Returns `None` if the `page_id` template was not found in the `<PageFlip>` widget DSL.
    pub fn set_active_page(&mut self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        let page_widget = self.page(cx, page_id)?;
        if self.active_page != page_id {
            self.active_page = page_id;
            self.redraw(cx);
        }
        Some(page_widget)
    }
}

impl PageFlipRef {
    /// See [`PageFlip::set_active_page()`].
    pub fn set_active_page(&self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        let mut inner = self.borrow_mut()?;
        inner.set_active_page(cx, page_id)
    }
}
