use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt, held_events::HeldEvents, view::View};
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PageFlipBase = #(PageFlip::register_widget(vm))
    mod.widgets.PageFlip = mod.widgets.PageFlipBase{}
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct PageFlip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(false)]
    lazy_init: bool,
    #[imperative]
    #[live]
    #[apply_state]
    active_page: LiveId,
    #[live(true)]
    #[apply_state]
    visible: bool,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
    #[rust]
    templates: ComponentMap<LiveId, ScriptObjectRef>,
    #[rust]
    pages: ComponentMap<LiveId, WidgetRef>,
    /// What each page out of sight waits for (see `held_events`): a page
    /// that is not shown skips frame requests (delivered once it is shown)
    /// and actions, unless its root view sets `background_events: true`.
    #[rust]
    held: HashMap<LiveId, HeldEvents>,
}

impl ScriptHook for PageFlip {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Handle vec_key children from the object's vec (these are our page templates)
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if kv.key.as_id().is_some() {
                            if let Some(id) = kv.key.as_id() {
                                if let Some(template_obj) = kv.value.as_object() {
                                    self.templates
                                        .insert(id, vm.new_object_ref(template_obj));
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

        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl PageFlip {
    /// Returns the widget for the given page templated ID, creating it if necessary.
    pub fn page(&mut self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        if let Some(template_ref) = self.templates.get(&page_id) {
            let template_value: ScriptValue = template_ref.as_object().into();
            if !self.pages.contains_key(&page_id) {
                let page = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                self.pages.insert(page_id, page.clone());
                cx.widget_tree_insert_child_deep(self.uid, page_id, page);
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
    fn visible(&self) -> bool { self.visible }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible { self.visible = visible; self.redraw(cx); }
    }
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, page) in self.pages.iter() {
            visit(*id, page.clone());
        }
    }

    fn cancel_children_impl(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        if let Some(page) = self.pages.get(&self.active_page) {
            visit(self.active_page, page.clone());
        }
        true
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }
}

impl Widget for PageFlip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() { return; }
        let uid = self.widget_uid();
        let active_page = self.active_page;
        // The shown page first gets what it waited for while it was hidden.
        if let (Some(page), Some(held)) = (self.pages.get_mut(&active_page), self.held.get_mut(&active_page)) {
            if !held.is_empty() {
                let item_uid = page.widget_uid();
                for held in held.take() {
                    cx.group_widget_actions(uid, item_uid, |cx| page.handle_event(cx, &held, scope));
                }
            }
        }
        if event.requires_visibility() {
            if let Some(page) = self.pages.get_mut(&active_page) {
                let item_uid = page.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx| page.handle_event(cx, event, scope));
            }
        } else {
            for (page_id, page) in self.pages.iter() {
                if *page_id != active_page && !page.borrow::<View>().is_some_and(|view| view.background_events)
                    && self.held.entry(*page_id).or_default().hold(event) {
                    continue;
                }
                let item_uid = page.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx| page.handle_event(cx, event, scope));
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible { return DrawStep::done(); }
        let active_page = self.active_page;
        if let Some(page) = self.page(cx, active_page) {
            if self.draw_state.begin_with(cx, &(), |cx, _| page.walk(cx)) {
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
            // The page's retained draw lists missed any redraws while it was hidden.
            page_widget.redraw(cx);
        }
        Some(page_widget)
    }
}

impl PageFlip {
    /// Builds the page for `page_id` without making it the active page, so
    /// an app can prepare a page it is not showing yet. Idempotent: a page
    /// that already exists is left as it is. Returns whether the page exists
    /// afterwards (false: no such template).
    pub fn ensure_page(&mut self, cx: &mut Cx, page_id: LiveId) -> bool {
        if self.pages.contains_key(&page_id) {
            return true;
        }
        self.page(cx, page_id).is_some()
    }

    /// Whether the page for `page_id` has been built.
    pub fn has_page(&self, page_id: LiveId) -> bool {
        self.pages.contains_key(&page_id)
    }
}

impl PageFlipRef {
    /// See [`PageFlip::ensure_page()`].
    pub fn ensure_page(&self, cx: &mut Cx, page_id: LiveId) -> bool {
        match self.borrow_mut() {
            Some(mut inner) => inner.ensure_page(cx, page_id),
            None => false,
        }
    }

    /// See [`PageFlip::has_page()`].
    pub fn has_page(&self, page_id: LiveId) -> bool {
        self.borrow().is_some_and(|inner| inner.has_page(page_id))
    }

    /// See [`PageFlip::page()`].
    pub fn page(&self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        let mut inner = self.borrow_mut()?;
        inner.page(cx, page_id)
    }

    /// See [`PageFlip::set_active_page()`].
    pub fn set_active_page(&self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        let mut inner = self.borrow_mut()?;
        inner.set_active_page(cx, page_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn style_reapply_keeps_selected_page_and_visibility() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value=crate::script_eval!(vm,{use mod.widgets.* PageFlip{
                active_page: @one
                one := View{}
                two := View{}
            }});
            let mut pages=PageFlip::script_from_value(vm,value);
            pages.set_active_page(vm.cx_mut(),live_id!(two)).unwrap();
            pages.set_visible(vm.cx_mut(),false);
            pages.script_apply(vm,&Apply::ScriptReapply,&mut Scope::empty(),value);
            assert_eq!(pages.active_page,live_id!(two));
            assert!(!pages.visible);
            assert_eq!(pages.pages.len(),2);
        });
    }
}
