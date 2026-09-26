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
        // Centred, not TopLeft: a face carrying an icon beside its label
        // gives the shorter of the two the slack, and top-aligning it put
        // the icon three points above the words it belongs to. The popup's
        // own rows already centre, so this is the two halves of one control
        // agreeing. A label-only face has one walk and no slack, which is
        // every call site that predates icons — the change cannot move them.
        align: Align{x: 0.0, y: 0.5}

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

        /** The selected item's icon ink. Flat rather than state-mixed like
        `draw_text`, because a chrome glyph has to stay readable over the
        pressed face; the face itself already carries the state. */
        draw_icon +: {
            color: theme.color_label_inner
        }

        /** The icon sits in front of the label with a small gap, which
        `icon_only` drops again since there is then nothing to separate from.
        Only walked for a dropdown that was given `icons`, so a dropdown
        without them keeps exactly its old metrics. */
        icon_walk: Walk{
            width: 10
            height: Fit
            margin: Inset{right: 5.0}
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
            e.lock_delta *= transform.scale;
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
            e.scroll *= transform.scale;
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
                touch.radius *= transform.scale;
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
    /// Re-pointed at the selected item's icon on every draw, and left empty for
    /// a dropdown that was given none — an empty `DrawSvg` is never walked, so
    /// the face keeps its old metrics.
    #[live]
    draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,

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

    /// Icons parallel to `labels`, one per item. Short is fine: an item past
    /// the end of this list simply draws no icon, so a caller can give icons to
    /// the few entries that have one and leave the rest as words.
    #[live]
    icons: Vec<DropDownIcon>,

    /// Draw the closed face as the selected item's icon alone, for a console
    /// too narrow for words. The popup still shows icon and label both — the
    /// list is where the operator reads what an item means.
    #[live]
    icon_only: bool,

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

    /// The key this dropdown last took up in the menu store, so a change of
    /// template -- every rebuild is one -- lets the old key go.
    #[rust]
    popup_key: Option<PopupMenuKey>,

    #[imperative]
    #[live]
    #[apply_state]
    selected_item: usize,

    #[layout]
    layout: Layout,

    #[rust]
    action_data: WidgetActionData,
}

#[derive(Default, Clone)]
struct PopupMenuGlobal {
    map: Rc<RefCell<ComponentMap<PopupMenuKey, PopupMenu>>>,
    /// How many dropdowns stand on each key. A module rebuild hands every
    /// dropdown a fresh template object, and so a fresh key; the menu built
    /// for the old key has nobody left to draw it, and it goes with the last
    /// dropdown that lets the key go. Without this the cache kept one menu
    /// per rebuild for the life of the process -- each holding its item
    /// template's whole tree and a draw list -- and nothing ever asked for
    /// them again.
    users: Rc<RefCell<std::collections::HashMap<PopupMenuKey, usize>>>,
}

impl PopupMenuGlobal {
    fn take_up(&self, key: PopupMenuKey) {
        *self.users.borrow_mut().entry(key).or_insert(0) += 1;
    }

    /// One dropdown fewer on the key; the last one out takes the menu with
    /// it. The map can be mid-borrow when this runs from a nested apply,
    /// in which case the entry waits for the next release, or for the heap
    /// to be retired.
    fn let_go(&self, key: PopupMenuKey) {
        let last = {
            let mut users = self.users.borrow_mut();
            match users.get_mut(&key) {
                Some(n) if *n > 1 => {
                    *n -= 1;
                    false
                }
                Some(_) => {
                    users.remove(&key);
                    true
                }
                None => false,
            }
        };
        if last {
            if let Ok(mut map) = self.map.try_borrow_mut() {
                map.retain(|k, _| *k != key);
            }
        }
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct PopupMenuKey {
    // ScriptValue only identifies an object within one heap. Popup menus are
    // Cx-global, so the heap is part of their identity too.
    heap: usize,
    template: ScriptValue,
}

/// One entry of `DropDown::icons`.
///
/// A `Vec` field only accepts elements the script layer marks as derivable, so
/// the bare `Option<ScriptHandleRef>` that `DrawSvg::svg` uses cannot be the
/// element type directly. Wrapping it in a derived struct earns that marker;
/// `on_custom_apply` then takes the resource handle a `crate_resource(...)` in
/// the DSL array evaluates to, so the call site stays a flat list of handles
/// rather than a list of one-field objects.
#[derive(Script)]
pub struct DropDownIcon {
    #[live]
    svg: Option<ScriptHandleRef>,
}

impl ScriptHook for DropDownIcon {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_nil() || value.as_handle().is_some()
    }

    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        if value.as_handle().is_none() {
            return false;
        }
        self.svg.script_apply(vm, apply, scope, value);
        true
    }
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
            if self.popup_key != Some(key) {
                drop(map);
                if let Some(old) = self.popup_key.take() {
                    global.let_go(old);
                }
                global.take_up(key);
                self.popup_key = Some(key);
                map = match global.map.try_borrow_mut() {
                    Ok(map) => map,
                    Err(_) => return,
                };
            }
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

impl Drop for DropDown {
    fn drop(&mut self) {
        if let Some(key) = self.popup_key.take() {
            self.popup_global.let_go(key);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DropDownAction {
    Select(usize),
    #[default]
    None,
}

impl DropDown {
    /// Remove every cached popup minted by one script heap before that heap's
    /// isolate is freed. PopupMenu objects retain script refs (including their
    /// item template), so allowing them to outlive the heap is both a leak and
    /// an identity hazard when an allocator later reuses the heap address.
    pub fn retire_popup_menus_for_heap(cx: &mut Cx, heap: usize) -> usize {
        if heap == 0 {
            return 0;
        }
        let global = cx.global::<PopupMenuGlobal>().clone();
        let mut map = global.map.borrow_mut();
        let before = map.len();
        map.retain(|key, _| key.heap != heap);
        before - map.len()
    }

    /// Test/diagnostic visibility for isolate lifecycle assertions.
    #[doc(hidden)]
    pub fn popup_menu_cache_len(cx: &mut Cx) -> usize {
        cx.global::<PopupMenuGlobal>().map.borrow().len()
    }

    fn popup_menu_key(&self) -> PopupMenuKey {
        PopupMenuKey {
            heap: self.source.heap_key(),
            template: self.popup_menu,
        }
    }

    /// Whether the shared menu on screen is THIS dropdown's: whether this is
    /// the dropdown that last drew it. Every dropdown on one template shares
    /// one menu instance, so the items in it belong to one of them at a time
    /// and the rest must not read them. See `handle_event`.
    pub fn owns_the_menu(&self, cx: &mut Cx) -> bool {
        let global = cx.global::<PopupMenuGlobal>().clone();
        let Ok(map) = global.map.try_borrow() else {
            return false;
        };
        map.get(&self.popup_menu_key()).is_some_and(|menu| menu.tree_parent == self.widget_uid())
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

    /// The icon for item `index`, or `None` where the caller gave fewer icons
    /// than labels (or none at all).
    fn icon_at(&self, index: usize) -> Option<&ScriptHandleRef> {
        self.icons.get(index).and_then(|icon| icon.svg.as_ref())
    }

    /// Point `draw_icon` at item `index`'s icon and report whether there is one.
    /// Only writes on an actual change: a `ScriptHandleRef` is a GC root, and
    /// re-seating one every frame churns the root table for nothing.
    fn seat_icon(&mut self, index: usize) -> bool {
        let want = self.icons.get(index).and_then(|icon| icon.svg.as_ref());
        if self.draw_icon.svg.as_ref().map(|s| s.as_handle()) != want.map(|s| s.as_handle()) {
            self.draw_icon.svg = want.cloned();
        }
        self.draw_icon.svg.is_some()
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);

        let has_icon = self.seat_icon(self.selected_item);
        if has_icon {
            // The gap in `icon_walk` exists to hold the label off the glyph;
            // with the label gone there is nothing to hold off, and a collapsed
            // face should be as narrow as the icon.
            let icon_walk = if self.icon_only {
                Walk {
                    margin: Inset::default(),
                    ..self.icon_walk
                }
            } else {
                self.icon_walk
            };
            self.draw_icon.draw_walk(cx, icon_walk);
        }

        // `icon_only` lets the icon stand in for the words. An item that was
        // given no icon has nothing to stand in for them, so it keeps them
        // rather than leaving the operator a blank face.
        if !(self.icon_only && has_icon) {
            if let Some(val) = self.labels.get(self.selected_item) {
                // A label set to ellipsize takes the room the face leaves it
                // (its padding keeps the chevron clear) and ends in "…".
                let walk = if self.draw_text.text_overflow == crate::makepad_draw::shader::draw_text::TextOverflow::Ellipsis {
                    Walk { width: Size::fill(), height: Size::fit(), ..Walk::fit() }
                } else {
                    Walk::fit()
                };
                self.draw_text.draw_walk(cx, walk, Align::default(), val);
            } else {
                self.draw_text
                    .draw_walk(cx, Walk::fit(), Align::default(), " ");
            }
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
            popup_menu.icon_column = !self.icons.is_empty();
            popup_menu.begin(cx);

            match self.popup_menu_position {
                PopupMenuPosition::OnSelected => {
                    let mut item_pos = None;
                    for (i, item) in self.labels.iter().enumerate() {
                        let node_id = LiveId(i as u64).into();
                        if i == self.selected_item {
                            item_pos = Some(cx.turtle().pos());
                        }
                        popup_menu.draw_item_full(
                            cx,
                            node_id,
                            &item,
                            self.icon_at(i),
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
                        popup_menu.draw_item_full(
                            cx,
                            node_id,
                            &item,
                            self.icon_at(i),
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

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        Some(self.selected_item_label())
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        let uid = self.widget_uid();

        // ONE menu instance serves every dropdown on the same template, and
        // it holds the items of whichever dropdown last DREW it
        // (`tree_parent`, claimed in `draw_walk`). Between this dropdown
        // being pressed and its own first draw, the items standing in that
        // menu are somebody else's: they are somewhere else on the screen,
        // they say something else, and they answer to another list. Reading
        // them here turns a press anywhere over that other menu's rows into a
        // choice made HERE -- a `Select` carrying the other list's row
        // number, sent under this dropdown's uid, which is a value nobody
        // chose arriving at a control nobody touched.
        //
        // So the menu is this dropdown's to read only once it has drawn it.
        // Until then there is nothing of ours on the screen to be pressed,
        // and the press belongs to whoever drew last. Closing still works by
        // the plain routes -- key focus lost, and the dropdown's own hits --
        // and the next draw makes the menu ours again.
        if self.is_active && !self.popup_menu.is_nil() && self.owns_the_menu(cx) {
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
                    // The press that closes the list is the list's. What was
                    // walked before this saw the lock and nothing else; what
                    // is walked after would see no lock, now it is released,
                    // and take the press as its own: a tab beside the page
                    // switched as the list closed. Marked on the event as it
                    // came in, which `transform_popup_event` copied.
                    if let Event::MouseDown(e) = event {
                        if e.handled.get().is_empty() {
                            e.handled.set(self.draw_bg.area());
                        }
                    }
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

    /// Collapse the closed face to the selected item's icon, or spell it out
    /// again. The host drives this off its own width, so it fires on every
    /// resize step — hence the guard against redrawing for an unchanged value.
    pub fn set_icon_only(&self, cx: &mut Cx, icon_only: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.icon_only != icon_only {
                inner.icon_only = icon_only;
                inner.draw_bg.redraw(cx);
            }
        }
    }

    pub fn icon_only(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.icon_only;
        }
        false
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
mod outside_press_tests {
    use super::*;
    use crate::button::ButtonAction;
    use crate::combo_box::ComboBoxWidgetRefExt;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: DVec2 = DVec2 { x: 800.0, y: 600.0 };

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn release(abs: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    /// A press and its release through the whole page, in tree order, and
    /// whether the button heard the press.
    fn click(cx: &mut Cx, root: &WidgetRef, at: DVec2, button: &WidgetRef) -> bool {
        let mut heard = false;
        for event in [press(at), release(at)] {
            let actions = cx.capture_actions(|cx| root.handle_event(cx, &event, &mut Scope::empty()));
            heard |= actions
                .iter()
                .filter_map(|action| action.as_widget_action())
                .any(|action| {
                    action.widget_uid == button.widget_uid()
                        && matches!(action.cast::<ButtonAction>(), ButtonAction::Pressed(_))
                });
        }
        heard
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> DVec2 {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    /// An open list, a drop-down's or a combo box's, closes on a press
    /// outside it, and the press goes no further: a button walked after the
    /// list does not hear it, though the list's lock is gone by the time
    /// the button is walked. The next press reaches the button.
    ///
    /// Each list has a button of its own, and each opens from Rust: with no
    /// event loop here to end a capture on release, a widget that once took
    /// a press would take every later one.
    #[test]
    fn a_press_outside_an_open_list_closes_it_and_reaches_nothing_after_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 20.
                    // The lists before the buttons, as a page is walked
                    // before the side panel beside it.
                    event_order: EventOrder.Down
                    pick := DropDown{width: 150.}
                    combo := ComboBox{width: 150.}
                    after_pick := Button{width: 150. height: 40. margin: Inset{top: 200.} text: "after"}
                    after_combo := Button{width: 150. height: 40. text: "after"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let labels = || vec!["One".to_string(), "Two".to_string(), "Three".to_string()];
        root.drop_down(&cx, ids!(pick)).set_labels(&mut cx, labels());
        let combo = root.widget(&cx, ids!(combo)).as_combo_box();
        combo.set_labels(&mut cx, labels());
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let pick = root.widget(&cx, ids!(pick));
        let is_active = |pick: &WidgetRef| pick.borrow::<DropDown>().unwrap().is_active;

        let after = root.widget(&cx, ids!(after_pick));
        let on_after = middle(&cx, &after);
        pick.borrow_mut::<DropDown>().unwrap().set_active(&mut cx);
        target.draw(&mut cx, &root);
        assert!(is_active(&pick));
        assert!(!click(&mut cx, &root, on_after, &after), "the button heard the press that closed the drop-down's list");
        assert!(!is_active(&pick), "the press outside closed the drop-down's list");
        assert_eq!(cx.sweep_lock_area(), None);
        target.draw(&mut cx, &root);
        assert!(click(&mut cx, &root, on_after, &after), "with the list closed the button hears its press");

        let after = root.widget(&cx, ids!(after_combo));
        let on_after = middle(&cx, &after);
        combo.open_list(&mut cx);
        target.draw(&mut cx, &root);
        assert!(combo.is_open());
        assert!(!click(&mut cx, &root, on_after, &after), "the button heard the press that closed the combo box's list");
        assert!(!combo.is_open(), "the press outside closed the combo box's list");
        assert_eq!(cx.sweep_lock_area(), None);
        target.draw(&mut cx, &root);
        assert!(click(&mut cx, &root, on_after, &after), "with the list closed the button hears its press");
    }
}

#[cfg(test)]
mod popup_cache_tests {
    use super::*;
    use crate::makepad_script::script;
    use crate::makepad_platform::ScriptApply;

    /// A module rebuild hands every dropdown a fresh popup template, and so
    /// a fresh cache key. The menu built for the old key went on living in
    /// the cache with its item template's tree and its draw list -- one more
    /// per rebuild, for the life of the process. Now the last dropdown off a
    /// key takes the menu with it, and a dropped dropdown does the same.
    #[test]
    fn a_dropdown_lets_go_of_the_menu_its_old_template_earned() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| crate::script_mod(vm));
        let build = |cx: &mut Cx| {
            cx.with_vm(|vm| {
                vm.eval(script! {
                    use mod.prelude.widgets.*
                    DropDown{ popup_menu: PopupMenuFlat{} }
                })
            })
        };
        let first = build(&mut cx);
        let mut widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, first));
        assert_eq!(DropDown::popup_menu_cache_len(&mut cx), 1, "the first template earned no menu");
        // The same widget applied from a second template object, which is
        // what a rebuild does to every dropdown in the tree.
        let second = build(&mut cx);
        assert_ne!(first.as_object(), second.as_object(), "the second build is the same object; the test proves nothing");
        cx.with_vm(|vm| widget.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), second));
        assert_eq!(DropDown::popup_menu_cache_len(&mut cx), 1, "the old template's menu stayed in the cache");
        drop(widget);
        assert_eq!(DropDown::popup_menu_cache_len(&mut cx), 0, "a dropped dropdown left its menu behind");
    }

    /// Two dropdowns on one template share one menu, and it stays as long as
    /// either of them stands on it.
    #[test]
    fn a_shared_menu_stays_until_the_last_dropdown_lets_go() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| crate::script_mod(vm));
        let (a, b) = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                View{ one := DropDown{} two := DropDown{} }
            });
            let root = WidgetRef::script_from_value(vm, value);
            (root.clone(), root)
        });
        assert_eq!(DropDown::popup_menu_cache_len(&mut cx), 1, "two dropdowns on one template should share one menu");
        drop(a);
        assert_eq!(DropDown::popup_menu_cache_len(&mut cx), 1, "the menu went while a dropdown still stood on it");
        drop(b);
        assert_eq!(DropDown::popup_menu_cache_len(&mut cx), 0);
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;
    use crate::makepad_script::script;
    use crate::makepad_platform::event::{ScrollEvent, ScrollPhase};
    use std::cell::Cell;

    fn popup(cx: &mut Cx) -> PopupMenu {
        cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                PopupMenu{}
            });
            PopupMenu::script_from_value(vm, value)
        })
    }

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

        for transform in [
            PopupAnchorTransform {
                scale: 0.5,
                translation: dvec2(20.0, -5.0),
            },
            PopupAnchorTransform {
                scale: 2.0,
                translation: dvec2(-30.0, 15.0),
            },
        ] {
            let event = Event::Scroll(ScrollEvent {
                window_id: WindowId(1, 1),
                scroll: dvec2(8.0, -4.0),
                abs: dvec2(110.0, 70.0),
                modifiers: KeyModifiers::default(),
                handled_x: Cell::new(false),
                handled_y: Cell::new(false),
                is_mouse: true,
                time: 0.0,
                phase: ScrollPhase::None,
            });
            assert!(matches!(
                transform_popup_event(&event, transform),
                Some(Event::Scroll(event))
                    if event.abs == transform.point(dvec2(110.0, 70.0))
                        && event.scroll == dvec2(8.0, -4.0) * transform.scale
            ));
        }
    }

    #[test]
    fn popup_cache_can_retire_one_isolate_heap() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let global = cx.global::<PopupMenuGlobal>().clone();
        let first = popup(&mut cx);
        let second = popup(&mut cx);
        {
            let mut map = global.map.borrow_mut();
            map.insert(
                PopupMenuKey {
                    heap: 11,
                    template: ScriptValue::NIL,
                },
                first,
            );
            map.insert(
                PopupMenuKey {
                    heap: 22,
                    template: ScriptValue::NIL,
                },
                second,
            );
        }
        assert_eq!(DropDown::retire_popup_menus_for_heap(&mut cx, 11), 1);
        let map = global.map.borrow();
        assert!(!map.keys().any(|key| key.heap == 11));
        assert!(map.keys().any(|key| key.heap == 22));
    }
}
