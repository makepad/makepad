use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::CxSplashVmExt,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PopupMenuItemBase = #(PopupMenuItem::register_widget(vm))
    mod.widgets.PopupMenuBase = #(PopupMenu::script_component(vm))

    mod.widgets.PopupMenuItem = set_type_default() do mod.widgets.PopupMenuItemBase{
        width: Fill
        height: Fit
        align: Align{y: 0.5}
        padding: theme.mspace_1{left: 15.}

        draw_text +: {
            active: instance(0.0)
            hover: instance(0.0)
            disabled: instance(0.0)

            ink_centered: true

            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_active: uniform(theme.color_label_inner_active)
            color_disabled: uniform(theme.color_label_inner_disabled)

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }

            get_color: fn() {
                return self.color
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_disabled, self.disabled)
            }
        }

        draw_bg +: {
            active: instance(0.0)
            hover: instance(0.0)
            disabled: instance(0.0)

            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_dither: uniform(1.0)
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_u_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_active: uniform(theme.color_outset_active)
            color_disabled: uniform(theme.color_outset_disabled)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_outset_2_hover)
            color_2_active: uniform(theme.color_outset_2_active)
            color_2_disabled: uniform(theme.color_outset_2_disabled)

            border_color: uniform(theme.color_u_hidden)
            border_color_hover: uniform(theme.color_u_hidden)
            border_color_active: uniform(theme.color_u_hidden)
            border_color_disabled: uniform(theme.color_u_hidden)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_u_hidden)
            border_color_2_active: uniform(theme.color_u_hidden)
            border_color_2_disabled: uniform(theme.color_u_hidden)

            mark_color: uniform(theme.color_u_hidden)
            mark_color_active: uniform(theme.color_mark_active)
            mark_color_disabled: uniform(theme.color_mark_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )

                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_active = self.color_active
                let mut color_fill_disabled = self.color_disabled

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_fill = vec2(
                        self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                        self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                    )
                    let dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y
                    color_fill = mix(self.color, self.color_2, dir)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, dir)
                    color_fill_active = mix(self.color_active, self.color_2_active, dir)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, dir)
                }

                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_active = self.border_color_active
                let mut color_stroke_disabled = self.border_color_disabled

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_border = vec2(
                        self.pos.x + dither
                        self.pos.y + dither
                    )
                    let dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                    color_stroke_hover = mix(self.border_color_hover, self.border_color_2_hover, dir)
                    color_stroke_active = mix(self.border_color_active, self.border_color_2_active, dir)
                    color_stroke_disabled = mix(self.border_color_disabled, self.border_color_2_disabled, dir)
                }

                let fill = color_fill
                    .mix(color_fill_active, self.active)
                    .mix(color_fill_hover, self.hover)
                    .mix(color_fill_disabled, self.disabled)

                let stroke = color_stroke
                    .mix(color_stroke_active, self.active)
                    .mix(color_stroke_hover, self.hover)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                // Mark
                let sz = 3.
                let dx = 2.0
                let c = vec2(8.0, 0.5 * self.rect_size.y)
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx)
                sdf.line_to(c.x, c.y + sz)
                sdf.line_to(c.x + sz, c.y - sz)

                sdf.stroke(
                    self.mark_color
                        .mix(self.mark_color_active, self.active)
                        .mix(self.mark_color_disabled, self.disabled)
                    1.)

                return sdf.result
            }
        }

        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                    }
                }
            }

            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
        indent_width: 10.0
    }

    mod.widgets.PopupMenuItemGradientX = mod.widgets.PopupMenuItem{
        draw_bg +: {
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 1.0

            color: theme.color_u_hidden
            color_hover: theme.color_outset_1_hover
            color_active: theme.color_outset_1_active
            color_disabled: theme.color_outset_1_disabled

            color_2: theme.color_u_hidden
            color_2_hover: theme.color_outset_2_hover
            color_2_active: theme.color_outset_2_active
            color_2_disabled: theme.color_outset_2_disabled
        }
    }

    mod.widgets.PopupMenuItemGradientY = mod.widgets.PopupMenuItemGradientX{
        draw_bg +: {
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 0.0
        }
    }

    mod.widgets.PopupMenuFlat = set_type_default() do mod.widgets.PopupMenuBase{
        width: 150.
        height: Fit
        flow: Flow.Down
        padding: theme.mspace_1

        menu_item: mod.widgets.PopupMenuItem{}

        draw_bg +: {
            border_size: uniform(theme.beveling)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_fg_app)
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color: uniform(theme.color_bevel)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_dither: uniform(1.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = if self.color_2.x > -0.5 self.color_2 else self.color
                let border_color_2 = if self.border_color_2.x > -0.5 self.border_color_2 else self.border_color

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither
                    self.pos.y + dither
                )

                let gradient_border_dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                let gradient_fill_dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y

                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )

                sdf.fill_keep(mix(self.color, color_2, gradient_fill_dir))

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color, border_color_2, gradient_border_dir)
                        self.border_size
                    )
                }

                return sdf.result
            }
        }
    }

    mod.widgets.PopupMenu = mod.widgets.PopupMenuFlat{
        menu_item: mod.widgets.PopupMenuItem{}
        draw_bg +: {
            border_color: theme.color_bevel_outset_1
            border_color_2: theme.color_bevel_outset_2
        }
    }

    mod.widgets.PopupMenuGradientY = mod.widgets.PopupMenu{
        menu_item: mod.widgets.PopupMenuItemGradientY{}

        draw_bg +: {
            color: theme.color_fg_app
            color_2: theme.color_fg_app * 1.2
        }
    }

    mod.widgets.PopupMenuGradientX = mod.widgets.PopupMenuGradientY{
        menu_item: mod.widgets.PopupMenuItemGradientY{}

        draw_bg +: {
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 1.0
        }
    }
}

/// A menu item is a real widget: it has a uid and sits in the widget tree
/// under the dropdown that opened the menu, so the design tweaker can pick it
/// (in 2D and on its own plane in the exploded view) and style its template.
/// The menu still drives it directly — `handle_event_with` for actions,
/// `draw_item` for the label — the widget seams only add identity.
#[derive(Script, ScriptHook, Animator, Widget)]
pub struct PopupMenuItem {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    /// The label the menu hands in before each draw.
    #[rust]
    pub label: String,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,

    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,
    #[walk]
    walk: Walk,

    #[live]
    indent_width: f32,
    #[live]
    icon_walk: Walk,

    #[live]
    opened: f32,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script)]
pub struct PopupMenu {
    #[source]
    source: ScriptObjectRef,

    #[live]
    draw_list: DrawList2d,
    #[live]
    menu_item: ScriptValue,

    #[live]
    draw_bg: DrawQuad,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,
    #[live]
    items: Vec<String>,
    #[rust]
    first_tap: bool,
    /// Each item is a widget of its own, registered in the widget tree under
    /// `tree_parent` by its item id (the design tweaker picks them).
    #[rust]
    menu_items: ComponentMap<PopupMenuItemId, WidgetRef>,
    /// The widget-tree node the items hang under. The menu itself is not a
    /// tree node (a `DropDown` owns it by value through a global), so the
    /// dropdown hands in its own uid and the items read as `drop_down.<n>`.
    #[rust]
    pub tree_parent: WidgetUid,
    #[rust]
    init_select_item: Option<PopupMenuItemId>,

    #[rust]
    count: usize,
}

impl ScriptHook for PopupMenu {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // Apply menu_item template to existing items
        if !self.menu_item.is_nil() {
            for (_, node) in self.menu_items.iter_mut() {
                let Some(mut node) = node.borrow_mut::<PopupMenuItem>() else {
                    continue;
                };
                node.script_apply(vm, apply, scope, self.menu_item);
            }
        }
        self.draw_list.redraw(vm.cx_mut());
    }
}

pub enum PopupMenuItemAction {
    WasSweeped,
    WasSelected,
    MightBeSelected,
    None,
}

#[derive(Clone, Default)]
pub enum PopupMenuAction {
    WasSweeped(PopupMenuItemId),
    WasSelected(PopupMenuItemId),
    #[default]
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PopupMenuItemId(pub LiveId);

impl PopupMenuItem {
    pub fn draw_item(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuItemAction),
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.area().redraw(cx);
        }

        match event.hits_with_options(
            cx,
            self.draw_bg.area(),
            HitOptions::new().with_sweep_area(sweep_area),
        ) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                dispatch_action(cx, PopupMenuItemAction::WasSweeped);
                self.animator_play(cx, ids!(hover.on));
                self.animator_play(cx, ids!(active.on));
            }
            Hit::FingerUp(se) if se.is_primary_hit() => {
                if !se.is_sweep {
                    dispatch_action(cx, PopupMenuItemAction::WasSelected);
                } else {
                    self.animator_play(cx, ids!(hover.off));
                    self.animator_play(cx, ids!(active.off));
                }
            }
            _ => {}
        }
    }
}

impl Widget for PopupMenuItem {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
        // Driven by `PopupMenu::handle_event_with`, which needs the menu's
        // sweep area and action sink; nothing to do on the plain seam.
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        let label = std::mem::take(&mut self.label);
        self.draw_item(cx, &label);
        self.label = label;
        DrawStep::done()
    }
}

impl PopupMenu {
    fn item(&mut self, cx: &mut Cx, item_id: PopupMenuItemId) -> WidgetRef {
        let menu_item = self.menu_item;
        // `menu_item` is an object index in the popup's heap. A popup owned by a
        // Splash isolate must install that VM on Cx before the template is applied.
        let Some(vm_id) = cx.script_ref_vm_id(&self.source) else {
            return WidgetRef::empty();
        };
        self.menu_items
            .get_or_insert(cx, item_id, |cx| {
                cx.with_script_vm_id(vm_id, |vm| {
                    WidgetRef::script_from_value(vm, menu_item)
                })
            })
            .clone()
    }

    pub fn menu_contains_pos(&self, cx: &mut Cx, pos: Vec2d) -> bool {
        self.draw_bg.area().clipped_rect(cx).contains(pos)
    }

    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.draw_list.begin_overlay_reuse(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        self.draw_bg.begin(cx, self.walk, self.layout);
        self.count = 0;
    }

    pub fn end(&mut self, cx: &mut Cx2d, shift_area: Area, shift: Vec2d) {
        self.draw_bg.end(cx);

        cx.end_pass_sized_turtle_with_shift(shift_area, shift);
        self.draw_list.end(cx);
        self.menu_items.retain_visible();
        if let Some(init_select_item) = self.init_select_item.take() {
            self.select_item_state(cx, init_select_item);
        }
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_list.redraw(cx);
    }

    pub fn draw_item(&mut self, cx: &mut Cx2d, item_id: PopupMenuItemId, label: &str) {
        self.draw_item_dimmed(cx, item_id, label, false);
    }

    /// Draw a selectable item using the disabled palette. This is visual
    /// metadata only: dimmed entries still receive pointer and keyboard input.
    pub fn draw_item_dimmed(
        &mut self,
        cx: &mut Cx2d,
        item_id: PopupMenuItemId,
        label: &str,
        dimmed: bool,
    ) {
        self.count += 1;

        let item = self.item(cx, item_id);
        if let Some(mut menu_item) = item.borrow_mut::<PopupMenuItem>() {
            menu_item.label = label.to_string();
            menu_item.animator_cut(
                cx,
                if dimmed {
                    ids!(disabled.on)
                } else {
                    ids!(disabled.off)
                },
            );
        }
        // Re-seat on every draw: one menu instance is shared by every dropdown
        // that uses the same template, so the items belong to whichever
        // dropdown has the menu open right now.
        cx.widget_tree_insert_child(self.tree_parent, item_id.0, item.clone());
        // Through the widget seam, so the item counts as its own nesting level
        // and the design tweaker's plane pick lands on it.
        item.draw_all(cx, &mut Scope::empty());
    }

    /// The items as widgets, for whoever enumerates children (the DropDown).
    pub fn item_refs(&self) -> impl Iterator<Item = (LiveId, WidgetRef)> + '_ {
        self.menu_items
            .iter()
            .map(|(id, item)| (id.0, item.clone()))
    }

    pub fn init_select_item(&mut self, which_id: PopupMenuItemId) {
        self.init_select_item = Some(which_id);
        self.first_tap = true;
    }

    fn select_item_state(&mut self, cx: &mut Cx, which_id: PopupMenuItemId) {
        for (id, item) in &mut *self.menu_items {
            let Some(mut item) = item.borrow_mut::<PopupMenuItem>() else {
                continue;
            };
            if *id == which_id {
                item.animator_cut(cx, ids!(active.on));
                item.animator_cut(cx, ids!(hover.on));
            } else {
                item.animator_cut(cx, ids!(active.off));
                item.animator_cut(cx, ids!(hover.off));
            }
        }
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuAction),
    ) {
        let mut actions = Vec::new();
        for (item_id, node) in self.menu_items.iter_mut() {
            let Some(mut node) = node.borrow_mut::<PopupMenuItem>() else {
                continue;
            };
            node.handle_event_with(cx, event, sweep_area, &mut |_, e| {
                actions.push((*item_id, e))
            });
        }

        for (node_id, action) in actions {
            match action {
                PopupMenuItemAction::MightBeSelected => {
                    if self.first_tap {
                        self.first_tap = false;
                    } else {
                        self.select_item_state(cx, node_id);
                        dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                    }
                }
                PopupMenuItemAction::WasSweeped => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                }
                PopupMenuItemAction::WasSelected => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                }
                _ => (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_item_is_instantiated_in_the_popup_owners_vm() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let vm_id = cx.alloc_splash_vm();
        let mut popup = cx.with_script_vm_id(vm_id, PopupMenu::script_new_with_default);
        let popup_heap = popup.source.heap_key();

        let item = popup.item(&mut cx, PopupMenuItemId(LiveId(1)));
        let item = item
            .borrow::<PopupMenuItem>()
            .expect("popup template should instantiate a PopupMenuItem");

        assert_eq!(item.source.heap_key(), popup_heap);
    }
}
