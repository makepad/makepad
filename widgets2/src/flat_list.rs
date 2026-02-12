use crate::{
    makepad_derive_widget::*, makepad_draw::*, scroll_bars::ScrollBars, widget::*,
    widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FlatListBase = #(FlatList::register_widget(vm))

    mod.widgets.FlatList = set_type_default() do mod.widgets.FlatListBase {
        width: Fill
        height: Fill
        capture_overload: true
        scroll_bars: mod.widgets.ScrollBars {show_scroll_x: false, show_scroll_y: true}
        flow: Down
    }
}

#[derive(Clone, Default)]
pub enum FlatListAction {
    Scroll,
    #[default]
    None,
}

pub struct WidgetItem {
    pub widget: WidgetRef,
    pub template: LiveId,
}

#[derive(Script, Widget)]
pub struct FlatList {
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live(0.2)]
    flick_scroll_minimum: f64,
    #[live(80.0)]
    flick_scroll_maximum: f64,
    #[live(0.005)]
    flick_scroll_scaling: f64,
    #[live(0.98)]
    flick_scroll_decay: f64,
    #[live(0.2)]
    swipe_drag_duration: f64,
    #[live(100.0)]
    max_pull_down: f64,
    #[live(true)]
    align_top_when_empty: bool,
    #[live(false)]
    grab_key_focus: bool,
    #[live(true)]
    drag_scrolling: bool,

    #[rust(Vec2Index::X)]
    vec_index: Vec2Index,
    #[redraw]
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    capture_overload: bool,
    #[rust]
    draw_state: DrawStateWrap<()>,

    // Templates stored as rooted ScriptObjectRef - populated in on_after_apply
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    pub items: ComponentMap<LiveId, WidgetItem>,
}

impl ScriptHook for FlatList {
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
        // Collect templates from the object's vec - only vec key IDs (name) end up in the vec
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        // Templates use vec key ids (name) - they end up in the vec
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                // Root the template object so it survives GC
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }

        // Update existing items if templates changed
        if apply.is_reload() {
            for (_, item) in self.items.iter_mut() {
                if let Some(template_ref) = self.templates.get(&item.template) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    item.widget.script_apply(vm, apply, scope, template_value);
                }
            }
        }

        // Set vec_index based on flow
        if let Flow::Down = self.layout.flow {
            self.vec_index = Vec2Index::Y;
        } else {
            self.vec_index = Vec2Index::X;
        }
    }
}

impl FlatList {
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.scroll_bars.end(cx);
    }

    pub fn space_left(&self, cx: &mut Cx2d) -> f64 {
        let view_total = cx.turtle().used();
        let rect_now = cx.turtle().rect();
        rect_now.size.y - view_total.y
    }

    pub fn item(&mut self, cx: &mut Cx, id: LiveId, template: LiveId) -> Option<WidgetRef> {
        use std::collections::hash_map::Entry;

        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            match self.items.entry(id) {
                Entry::Occupied(occ) => Some(occ.get().widget.clone()),
                Entry::Vacant(vac) => {
                    let widget_ref =
                        cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                    vac.insert(WidgetItem {
                        template,
                        widget: widget_ref.clone(),
                    });
                    Some(widget_ref)
                }
            }
        } else {
            warning!("Template not found: {template}. Did you add it to the FlatList instance?");
            None
        }
    }
}

impl Widget for FlatList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.scroll_bars.handle_event(cx, event, scope);

        for (item_id, item) in self.items.iter_mut() {
            let item_uid = item.widget.widget_uid();
            cx.with_node(item_uid, *item_id, item.widget.clone(), |cx| {
                cx.group_widget_actions(uid, item_uid, |cx| {
                    item.widget.handle_event(cx, event, scope)
                });
            });
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            self.begin(cx, walk);
            return DrawStep::make_step();
        }
        self.end(cx);
        self.draw_state.end();
        DrawStep::done()
    }
}

impl FlatListRef {
    pub fn item(&self, cx: &mut Cx, entry_id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.item(cx, entry_id, template)
        } else {
            None
        }
    }

    pub fn items_with_actions(&self, actions: &Actions) -> Vec<(LiveId, WidgetRef)> {
        let mut set = Vec::new();
        self.items_with_actions_vec(actions, &mut set);
        set
    }

    fn items_with_actions_vec(&self, actions: &Actions, set: &mut Vec<(LiveId, WidgetRef)>) {
        let uid = self.widget_uid();
        for action in actions {
            if let Some(action) = action.downcast_ref::<WidgetAction>() {
                if let Some(group) = &action.group {
                    if group.group_uid == uid {
                        if let Some(inner) = self.borrow() {
                            for (item_id, item) in inner.items.iter() {
                                if group.item_uid == item.widget.widget_uid() {
                                    set.push((*item_id, item.widget.clone()))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl FlatListSet {
    pub fn items_with_actions(&self, actions: &Actions) -> Vec<(LiveId, WidgetRef)> {
        let mut set = Vec::new();
        for list in self.iter() {
            list.items_with_actions_vec(actions, &mut set)
        }
        set
    }
}
