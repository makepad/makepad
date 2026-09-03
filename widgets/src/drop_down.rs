use {
    crate::{
        animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
        makepad_derive_widget::*,
        makepad_draw::*,
        popup_menu::{PopupMenu, PopupMenuAction},
        widget::*,
        widget_async::CxSplashVmExt,
    },
    std::cell::RefCell,
    std::rc::Rc,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawLabelTextBase = #(DrawLabelText::script_component(vm))
    mod.widgets.DropDownBase = #(DropDown::register_widget(vm))
    set_type_default() do #(DrawLabelText::script_shader(vm)){
        ..mod.draw.DrawText // splat in draw quad
    }
    /** The flat dropdown: a button face with a corner arrow, opening a popup menu. */
    mod.widgets.DropDownFlat = set_type_default() do mod.widgets.DropDownBase{
        width: Fit
        height: Fit
        align: TopLeft

        padding: theme.mspace_1{left: theme.space_2, right: 22.5}
        margin: theme.mspace_v_1{}

        /** The selected-item label ink, state-mixed with the face. */
        draw_text +: {
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)
            /** pressed mix 0..1 step 0.01 */
            down: instance(0.0)

            ink_centered: true

            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_focus: uniform(theme.color_label_inner_focus)
            color_down: uniform(theme.color_label_inner_down)
            color_disabled: uniform(theme.color_label_inner_disabled)

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }

            get_color: fn() {
                mix(
                    mix(
                        mix(
                            self.color
                            mix(
                                self.color_focus
                                self.color_hover
                                self.hover
                            )
                            self.focus
                        )
                        mix(
                            self.color_hover
                            self.color_down
                            self.down
                        )
                        self.hover
                    )
                    self.color_disabled
                    self.disabled
                )
            }
        }

        /** The dropdown face: an SDF box with a bevel stroke and a filled corner arrow. */
        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** pressed mix 0..1 step 0.01 */
            down: instance(0.0)
            /** popup-open mix 0..1 step 0.01 */
            active: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** bevel gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_border_horizontal: uniform(0.0)
            /** fill gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_fill_horizontal: uniform(0.0)
            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)

            /** dither the gradient fill to hide banding 0..1 step 1 */
            color_dither: uniform(1.0)

            color: uniform(theme.color_outset)
            color_hover: uniform(theme.color_outset_hover)
            color_focus: uniform(theme.color_outset_focus)
            color_down: uniform(theme.color_outset_down)
            color_disabled: uniform(theme.color_outset_disabled)

            /** fill gradient end stop; negative alpha means flat fill */
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_outset_2_hover)
            color_2_focus: uniform(theme.color_outset_2_focus)
            color_2_down: uniform(theme.color_outset_2_down)
            color_2_disabled: uniform(theme.color_outset_2_disabled)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_down: uniform(theme.color_bevel_down)
            border_color_disabled: uniform(theme.color_bevel_disabled)

            /** bevel gradient end stop; negative alpha means flat stroke */
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_bevel_outset_2_hover)
            border_color_2_focus: uniform(theme.color_bevel_outset_2_focus)
            border_color_2_down: uniform(theme.color_bevel_outset_2_down)
            border_color_2_disabled: uniform(theme.color_bevel_outset_2_disabled)

            /** the corner disclosure triangle ink */
            arrow_color: uniform(theme.color_label_inner)
            arrow_color_focus: uniform(theme.color_label_inner_focus)
            arrow_color_hover: uniform(theme.color_label_inner_hover)
            arrow_color_down: uniform(theme.color_label_inner_down)
            arrow_color_disabled: uniform(theme.color_label_inner_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither

                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_focus = self.color_focus
                let mut color_fill_down = self.color_down
                let mut color_fill_disabled = self.color_disabled

                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_focus = self.border_color_focus
                let mut color_stroke_down = self.border_color_down
                let mut color_stroke_disabled = self.border_color_disabled

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - /** arrow inset from right 4..24 step 0.5 */ 10.0, self.rect_size.y * 0.5)
                let sz = /** arrow half size 1..8 step 0.25 */ 2.5
                let offset = /** arrow vertical nudge -4..4 step 0.5 */ 1.
                let offset_x = /** arrow horizontal nudge -6..6 step 0.5 */ 2.

                sdf.move_to(c.x - sz - offset_x, c.y - sz + offset)
                sdf.line_to(c.x + sz - offset_x, c.y - sz + offset)
                sdf.line_to(c.x - offset_x, c.y + sz * /** arrow apex depth 0..1 step 0.05 */ 0.25 + offset)
                sdf.close_path()

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.arrow_color
                                self.arrow_color_focus
                                self.focus
                            )
                            mix(
                                self.arrow_color_hover
                                self.arrow_color_down
                                self.down
                            )
                            self.hover
                        )
                        self.arrow_color_disabled
                        self.disabled
                    )
                )

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

                if self.color_2.x > -0.5 {
                    color_fill = mix(self.color, self.color_2, gradient_fill_dir)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, gradient_fill_dir)
                    color_fill_focus = mix(self.color_focus, self.color_2_focus, gradient_fill_dir)
                    color_fill_down = mix(self.color_down, self.color_2_down, gradient_fill_dir)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, gradient_fill_dir)
                }

                if self.border_color_2.x > -0.5 {
                    color_stroke = mix(self.border_color, self.border_color_2, gradient_border_dir)
                    color_stroke_hover = mix(self.border_color_hover, self.border_color_2_hover, gradient_border_dir)
                    color_stroke_focus = mix(self.border_color_focus, self.border_color_2_focus, gradient_border_dir)
                    color_stroke_down = mix(self.border_color_down, self.border_color_2_down, gradient_border_dir)
                    color_stroke_disabled = mix(self.border_color_disabled, self.border_color_2_disabled, gradient_border_dir)
                }

                let fill = color_fill
                    .mix(color_fill_focus, self.focus)
                    .mix(color_fill_hover, self.hover)
                    .mix(color_fill_down, self.down * self.hover)
                    .mix(color_fill_disabled, self.disabled)

                let stroke = color_stroke
                    .mix(color_stroke_focus, self.focus)
                    .mix(color_stroke_hover, self.hover)
                    .mix(color_stroke_down, self.down * self.hover)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                sdf.result
            }
        }

        popup_menu: mod.widgets.PopupMenu{}

        selected_item: 0

        animator : Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {
                        all: Forward{duration: 0.1}
                        down: Forward{duration: 0.01}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}]}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}]}
                    }
                }

                down: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Arrow
                    from: {all: Forward{duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    /** The standard dropdown: the flat face plus the theme's outset bevel. */
    mod.widgets.DropDown = set_type_default() do mod.widgets.DropDownFlat{
        draw_bg +: {
            color: uniform(theme.color_outset)
            color_hover: uniform(theme.color_outset_hover)
            color_focus: uniform(theme.color_outset_focus)
            color_down: uniform(theme.color_outset_down)
            color_disabled: uniform(theme.color_u_hidden)

            border_color: uniform(theme.color_bevel_outset_1)
            border_color_hover: uniform(theme.color_bevel_outset_1_hover)
            border_color_focus: uniform(theme.color_bevel_outset_1_focus)
            border_color_down: uniform(theme.color_bevel_outset_1_down)
            border_color_disabled: uniform(theme.color_bevel_outset_1_disabled)

            border_color_2: uniform(theme.color_bevel_outset_2)
            border_color_2_hover: uniform(theme.color_bevel_outset_2_hover)
            border_color_2_focus: uniform(theme.color_bevel_outset_2_focus)
            border_color_2_down: uniform(theme.color_bevel_outset_2_down)
            border_color_2_disabled: uniform(theme.color_bevel_outset_2_disabled)
        }

        popup_menu: mod.widgets.PopupMenuFlat{}
    }

    mod.widgets.DropDownGradientY = mod.widgets.DropDown{
        popup_menu: mod.widgets.PopupMenuGradientY{}
        draw_bg +: {
            color: uniform(theme.color_outset_1)
            color_hover: uniform(theme.color_outset_1_hover)
            color_focus: uniform(theme.color_outset_1_focus)
            color_down: uniform(theme.color_outset_1_down)
            color_disabled: uniform(theme.color_outset_1_disabled)

            color_2: uniform(theme.color_outset_2)
        }
    }

    mod.widgets.DropDownGradientX = mod.widgets.DropDownGradientY{
        popup_menu: mod.widgets.PopupMenuGradientX{}

        draw_bg +: {
            gradient_border_horizontal: uniform(1.0)
            gradient_fill_horizontal: uniform(1.0)
        }
    }

}

#[derive(Script, ScriptHook, Clone, Copy)]
#[repr(C)]
pub enum PopupMenuPosition {
    #[pick]
    OnSelected,
    BelowInput,
}

/// Optional affine mapping for a dropdown drawn under a scaled/translated
/// parent draw list. Popup menus themselves are window-space overlays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupAnchorTransform {
    pub scale: f64,
    pub translation: DVec2,
}

impl PopupAnchorTransform {
    pub fn rect(self, rect: Rect) -> Rect {
        Rect {
            pos: rect.pos * self.scale + self.translation,
            size: rect.size * self.scale,
        }
    }

    fn point(self, point: DVec2) -> DVec2 {
        point * self.scale + self.translation
    }
}

fn transform_popup_event(event: &Event, transform: PopupAnchorTransform) -> Option<Event> {
    Some(match event {
        Event::MouseDown(e) => {
            let mut e = e.clone();
            e.abs = transform.point(e.abs);
            Event::MouseDown(e)
        }
        Event::MouseMove(e) => {
            let mut e = e.clone();
            e.abs = transform.point(e.abs);
            Event::MouseMove(e)
        }
        Event::MouseUp(e) => {
            let mut e = e.clone();
            e.abs = transform.point(e.abs);
            Event::MouseUp(e)
        }
        Event::MouseLeave(e) => {
            let mut e = e.clone();
            e.abs = transform.point(e.abs);
            Event::MouseLeave(e)
        }
        Event::Scroll(e) => {
            let mut e = e.clone();
            e.abs = transform.point(e.abs);
            Event::Scroll(e)
        }
        Event::LongPress(e) => {
            let mut e = e.clone();
            e.abs = transform.point(e.abs);
            Event::LongPress(e)
        }
        Event::TouchUpdate(e) => {
            let mut e = e.clone();
            for touch in &mut e.touches {
                touch.abs = transform.point(touch.abs);
            }
            Event::TouchUpdate(e)
        }
        _ => return None,
    })
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet, Animator)]
pub struct DropDown {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    // `redraw`/`walk`/`uid`/`action_data` are the `Widget` derive's helper
    // attributes; this widget hand-writes `WidgetNode` (for `children`), so
    // they are wired by hand below instead.
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawLabelText,

    #[walk]
    walk: Walk,

    #[live]
    bind: String,
    #[live]
    bind_enum: String,

    #[live]
    popup_menu: ScriptValue,

    #[live]
    labels: Vec<String>,

    /// Popup rows drawn with the disabled palette while remaining selectable.
    #[rust]
    dimmed_items: Vec<bool>,

    #[live]
    popup_menu_position: PopupMenuPosition,

    #[rust]
    is_active: bool,

    #[rust]
    popup_anchor_transform: Option<PopupAnchorTransform>,

    /// The menu store this dropdown draws from, kept so `children()` (which
    /// gets no `Cx`) can surface the open menu's items to the widget tree.
    #[rust]
    popup_global: PopupMenuGlobal,

    #[live]
    selected_item: usize,

    #[layout]
    layout: Layout,

    #[rust]
    action_data: WidgetActionData,
}

#[derive(Default, Clone)]
struct PopupMenuGlobal {
    map: Rc<RefCell<ComponentMap<PopupMenuKey, PopupMenu>>>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct PopupMenuKey {
    // ScriptValue only identifies an object within one heap. Popup menus are
    // Cx-global, so the heap is part of their identity too.
    heap: usize,
    template: ScriptValue,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawLabelText {
    #[deref]
    draw_super: DrawText,
    #[live]
    focus: f32,
    #[live]
    hover: f32,
}

impl ScriptHook for DropDown {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _obj: ScriptValue,
    ) {
        if self.popup_menu.is_nil() {
            return;
        }
        vm.with_cx_mut(|cx| {
            let global = cx.global::<PopupMenuGlobal>().clone();
            self.popup_global = global.clone();
            // Use try_borrow_mut to avoid panic if already borrowed (can happen during
            // nested on_after_apply calls when PopupMenu creation triggers another DropDown apply)
            let Ok(mut map) = global.map.try_borrow_mut() else {
                return;
            };

            let popup_menu_val = self.popup_menu;
            let key = self.popup_menu_key();
            let Some(vm_id) = cx.script_ref_vm_id(&self.source) else {
                return;
            };
            map.get_or_insert(cx, key, |cx| {
                cx.with_script_vm_id(vm_id, |vm| {
                    PopupMenu::script_from_value(vm, popup_menu_val)
                })
            });
        });
    }
}

#[derive(Clone, Debug, Default)]
pub enum DropDownAction {
    Select(usize),
    #[default]
    None,
}

impl DropDown {
    fn popup_menu_key(&self) -> PopupMenuKey {
        PopupMenuKey {
            heap: self.source.heap_key(),
            template: self.popup_menu,
        }
    }

    pub fn selected_item_index(&self) -> usize {
        self.selected_item
    }

    pub fn selected_item_label(&self) -> String {
        self.labels
            .get(self.selected_item)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_active(&mut self, cx: &mut Cx) {
        self.is_active = true;
        self.draw_bg.redraw(cx);
        let global = cx.global::<PopupMenuGlobal>().clone();
        let mut map = global.map.borrow_mut();
        let lb = map.get_mut(&self.popup_menu_key()).unwrap();
        let node_id = LiveId(self.selected_item as u64).into();
        lb.init_select_item(node_id);
        cx.sweep_lock(self.draw_bg.area());
    }

    pub fn set_closed(&mut self, cx: &mut Cx) {
        self.is_active = false;
        self.draw_bg.redraw(cx);
        cx.sweep_unlock(self.draw_bg.area());
    }

    pub fn draw_text(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);

        if let Some(val) = self.labels.get(self.selected_item) {
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align::default(), val);
        } else {
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align::default(), " ");
        }
        self.draw_bg.end(cx);

        cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());

        if self.is_active && !self.popup_menu.is_nil() {
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let popup_menu = map.get_mut(&self.popup_menu_key()).unwrap();

            // One menu instance serves every dropdown with the same template,
            // so claim its items for this dropdown while they draw.
            popup_menu.tree_parent = self.uid;
            popup_menu.begin(cx);

            match self.popup_menu_position {
                PopupMenuPosition::OnSelected => {
                    let mut item_pos = None;
                    for (i, item) in self.labels.iter().enumerate() {
                        let node_id = LiveId(i as u64).into();
                        if i == self.selected_item {
                            item_pos = Some(cx.turtle().pos());
                        }
                        popup_menu.draw_item_dimmed(
                            cx,
                            node_id,
                            &item,
                            self.dimmed_items.get(i).copied().unwrap_or(false),
                        );
                    }

                    let area = self.draw_bg.area().rect(cx);
                    let anchor = self
                        .popup_anchor_transform
                        .map(|transform| transform.rect(area))
                        .unwrap_or(area);
                    popup_menu.end(
                        cx,
                        self.draw_bg.area(),
                        anchor.pos - area.pos - item_pos.unwrap_or(dvec2(0.0, 0.0)),
                    );
                }
                PopupMenuPosition::BelowInput => {
                    for (i, item) in self.labels.iter().enumerate() {
                        let node_id = LiveId(i as u64).into();
                        popup_menu.draw_item_dimmed(
                            cx,
                            node_id,
                            &item,
                            self.dimmed_items.get(i).copied().unwrap_or(false),
                        );
                    }

                    let area = self.draw_bg.area().rect(cx);
                    let anchor = self
                        .popup_anchor_transform
                        .map(|transform| transform.rect(area))
                        .unwrap_or(area);
                    let shift = anchor.pos - area.pos + dvec2(0.0, anchor.size.y);

                    popup_menu.end(cx, self.draw_bg.area(), shift);
                }
            }
        }
    }
}

impl WidgetNode for DropDown {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn set_action_data(&mut self, action_data: std::sync::Arc<dyn ActionTrait>) {
        self.action_data.set_box(action_data)
    }

    fn action_data(&self) -> Option<std::sync::Arc<dyn ActionTrait>> {
        self.action_data.clone_data()
    }

    fn area(&self) -> Area {
        self.draw_bg.area()
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        // The open menu's items are widgets too (the design tweaker picks and
        // styles them); the menu that owns them is not a tree node, so they
        // surface here — only while this dropdown is the one showing them.
        if !self.is_active || self.popup_menu.is_nil() {
            return;
        }
        let Ok(map) = self.popup_global.map.try_borrow() else {
            return;
        };
        let Some(menu) = map.get(&self.popup_menu_key()) else {
            return;
        };
        for (id, item) in menu.item_refs() {
            visit(id, item);
        }
    }
}

impl Widget for DropDown {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        let uid = self.widget_uid();

        if self.is_active && !self.popup_menu.is_nil() {
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let menu = map.get_mut(&self.popup_menu_key()).unwrap();
            let mut close = false;
            let popup_event = self
                .popup_anchor_transform
                .and_then(|transform| transform_popup_event(event, transform));
            let popup_event = popup_event.as_ref().unwrap_or(event);
            menu.handle_event_with(
                cx,
                popup_event,
                self.draw_bg.area(),
                &mut |cx, action| match action {
                    PopupMenuAction::WasSweeped(_node_id) => {}
                    PopupMenuAction::WasSelected(node_id) => {
                        self.selected_item = node_id.0 .0 as usize;
                        cx.widget_action_with_data(
                            &self.action_data,
                            uid,
                            DropDownAction::Select(self.selected_item),
                        );
                        self.draw_bg.redraw(cx);
                        close = true;
                    }
                    _ => (),
                },
            );
            if close {
                self.set_closed(cx);
            }

            // check if we clicked outside of the popup menu
            if let Event::MouseDown(e) = popup_event {
                if !menu.menu_contains_pos(cx, e.abs) {
                    self.set_closed(cx);
                    self.animator_play(cx, ids!(hover.off));
                    return;
                }
            }
        }

        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.set_closed(cx);
                self.animator_play(cx, ids!(hover.off));
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::ArrowUp => {
                    if self.selected_item > 0 {
                        self.selected_item -= 1;
                        cx.widget_action_with_data(
                            &self.action_data,
                            uid,
                            DropDownAction::Select(self.selected_item),
                        );
                        self.set_closed(cx);
                        self.draw_bg.redraw(cx);
                    }
                }
                KeyCode::ArrowDown => {
                    if self.labels.len() > 0 && self.selected_item < self.labels.len() - 1 {
                        self.selected_item += 1;
                        cx.widget_action_with_data(
                            &self.action_data,
                            uid,
                            DropDownAction::Select(self.selected_item),
                        );
                        self.set_closed(cx);
                        self.draw_bg.redraw(cx);
                    }
                }
                _ => (),
            },
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(disabled.off)) {
                    cx.set_key_focus(self.draw_bg.area());
                    self.animator_play(cx, ids!(hover.down));
                    self.set_active(cx);
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if fe.is_over {
                    if fe.device.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    }
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => (),
        };
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl DropDownRef {
    pub fn set_popup_anchor_transform(
        &self,
        cx: &mut Cx,
        transform: Option<PopupAnchorTransform>,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.popup_anchor_transform != transform {
                inner.popup_anchor_transform = transform;
                inner.draw_bg.redraw(cx);
            }
        }
    }

    pub fn set_labels_with<F: FnMut(&mut String)>(&self, cx: &mut Cx, mut f: F) {
        if let Some(mut inner) = self.borrow_mut() {
            let mut i = 0;
            loop {
                if i >= inner.labels.len() {
                    inner.labels.push(String::new());
                }
                let s = &mut inner.labels[i];
                s.clear();
                f(s);
                if s.len() == 0 {
                    break;
                }
                i += 1;
            }
            inner.labels.truncate(i);
            inner.draw_bg.redraw(cx);
        }
    }

    pub fn set_labels(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.labels = labels;
            inner.draw_bg.redraw(cx);
        }
    }

    /// Set which popup rows use the disabled palette. These rows deliberately
    /// stay selectable (for example a hub model that will be acquired).
    pub fn set_dimmed_items(&self, cx: &mut Cx, dimmed_items: Vec<bool>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.dimmed_items = dimmed_items;
            inner.draw_bg.redraw(cx);
        }
    }

    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDownAction::Select(id) = item.cast() {
                return Some(id);
            }
        }
        None
    }

    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDownAction::Select(id) = item.cast() {
                return Some(id);
            }
        }
        None
    }

    pub fn changed_label(&self, actions: &Actions) -> Option<String> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDownAction::Select(id) = item.cast() {
                if let Some(inner) = self.borrow() {
                    return Some(inner.labels[id].clone());
                }
            }
        }
        None
    }

    pub fn set_selected_item(&self, cx: &mut Cx, item: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            let new_selected = item.min(inner.labels.len().max(1) - 1);
            if new_selected != inner.selected_item {
                inner.selected_item = new_selected;
                inner.draw_bg.redraw(cx);
            }
        }
    }

    pub fn selected_item(&self) -> usize {
        if let Some(inner) = self.borrow() {
            return inner.selected_item;
        }
        0
    }

    pub fn selected_label(&self) -> String {
        if let Some(inner) = self.borrow() {
            return inner.labels[inner.selected_item].clone();
        }
        "".to_string()
    }

    pub fn set_selected_by_label(&self, label: &str, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            if let Some(index) = inner.labels.iter().position(|v| v == label) {
                if inner.selected_item != index {
                    inner.selected_item = index;
                    inner.draw_bg.redraw(cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;

    #[test]
    fn popup_anchor_rect_applies_camera_scale_and_translation() {
        let rect = Rect {
            pos: dvec2(110.0, 70.0),
            size: dvec2(80.0, 24.0),
        };
        let half = PopupAnchorTransform {
            scale: 0.5,
            translation: dvec2(20.0, -5.0),
        }
        .rect(rect);
        assert_eq!(half.pos, dvec2(75.0, 30.0));
        assert_eq!(half.size, dvec2(40.0, 12.0));

        let double = PopupAnchorTransform {
            scale: 2.0,
            translation: dvec2(-30.0, 15.0),
        }
        .rect(rect);
        assert_eq!(double.pos, dvec2(190.0, 155.0));
        assert_eq!(double.size, dvec2(160.0, 48.0));
    }
}
