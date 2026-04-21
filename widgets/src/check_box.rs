use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    widget::*,
    widget_async::{CxWidgetToScriptCallExt, ScriptAsyncResult},
};

use crate::makepad_draw::DrawSvg;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CheckBoxBase = #(CheckBox::register_widget(vm))

    mod.widgets.CheckBoxFlat = set_type_default() do mod.widgets.CheckBoxBase{
        width: Fit
        height: Fit
        padding: theme.mspace_2
        align: Align{x: 0., y: 0.}

        label_walk: Walk{
            width: Fit
            height: Fit
            margin: theme.mspace_h_1{left: 13.}
        }

        draw_bg +: {
            disabled: instance(0.0)
            down: instance(0.0)
            hover: instance(0.0)
            focus: instance(0.0)
            active: instance(0.0)

            size: uniform(15.0)
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_down: uniform(theme.color_inset_down)
            color_active: uniform(theme.color_inset_active)
            color_focus: uniform(theme.color_inset_focus)
            color_disabled: uniform(theme.color_inset_disabled)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_down: uniform(theme.color_bevel_down)
            border_color_active: uniform(theme.color_bevel_active)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_disabled: uniform(theme.color_bevel_disabled)

            mark_size: uniform(0.65)
            mark_color: uniform(theme.color_u_hidden)
            mark_color_hover: uniform(theme.color_u_hidden)
            mark_color_down: uniform(theme.color_u_hidden)
            mark_color_active: uniform(theme.color_mark_active)
            mark_color_active_hover: uniform(theme.color_mark_active_hover)
            mark_color_focus: uniform(theme.color_mark_focus)
            mark_color_disabled: uniform(theme.color_mark_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let sz_px = self.size
                let center_px = vec2(sz_px * 0.5, self.rect_size.y * 0.5)
                let offset_px = vec2(0., center_px.y - sz_px * 0.5)

                //match self.check_type {
                //    CheckType.Check => {
                        // Draw background box
                        sdf.box(
                            offset_px.x + self.border_size
                            offset_px.y + self.border_size
                            sz_px - self.border_size * 2.
                            sz_px - self.border_size * 2.
                            self.border_radius * 0.5
                        )

                        let color_fill = self.color
                            .mix(self.color_focus, self.focus)
                            .mix(self.color_active, self.active)
                            .mix(self.color_hover, self.hover)
                            .mix(self.color_down, self.down)
                            .mix(self.color_disabled, self.disabled)

                        let color_stroke = self.border_color
                            .mix(self.border_color_focus, self.focus)
                            .mix(self.border_color_active, self.active)
                            .mix(self.border_color_hover, self.hover)
                            .mix(self.border_color_down, self.down)
                            .mix(self.border_color_disabled, self.disabled)

                        sdf.fill_keep(color_fill)
                        sdf.stroke(color_stroke, self.border_size)

                        // Draw checkmark
                        let mark_padding = 0.275 * self.size
                        sdf.move_to(mark_padding, center_px.y)
                        sdf.line_to(center_px.x, center_px.y + sz_px * 0.5 - mark_padding)
                        sdf.line_to(sz_px - mark_padding, offset_px.y + mark_padding)

                        let mark_color = self.mark_color
                            .mix(self.mark_color_hover, self.hover)
                            .mix(self.mark_color_active, self.active)
                            .mix(self.mark_color_disabled, self.disabled)

                        sdf.stroke(mark_color, self.size * 0.09)
                //    }

                //    CheckType.None => {
                //        sdf.fill(vec4(0., 0., 0., 0.))
                //    }
                //}
                return sdf.result
            }
        }

        draw_text +: {
            focus: instance(0.0)
            hover: instance(0.0)
            down: instance(0.0)
            active: instance(0.0)
            disabled: instance(0.0)

            color: theme.color_label_outer
            color_hover: uniform(theme.color_label_outer_hover)
            color_down: uniform(theme.color_label_outer_down)
            color_focus: uniform(theme.color_label_outer_focus)
            color_active: uniform(theme.color_label_outer_active)
            color_disabled: uniform(theme.color_label_outer_disabled)

            get_color: fn() {
                return self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)
            }
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
        }

        icon_walk: Walk{width: 14.0, height: Fit}

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
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.CheckBox = mod.widgets.CheckBoxFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_down: theme.color_bevel_inset_1_down
            border_color_active: theme.color_bevel_inset_1_active
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }

    mod.widgets.ToggleFlat = mod.widgets.CheckBoxFlat{
        label_walk +: {
            margin: theme.mspace_h_1{left: 27.}
        }

        draw_bg +: {
            mark_color: theme.color_label_outer
            mark_color_hover: theme.color_label_outer_active
            mark_color_down: theme.color_label_outer_down
            mark_color_active: theme.color_mark_active
            mark_color_active_hover: theme.color_mark_active_hover

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let sz_px = vec2(self.size * 1.6, self.size)
                let center_px = vec2(sz_px.x * 0.5, self.rect_size.y * 0.5)
                let offset_px = vec2(0., center_px.y - sz_px.y * 0.5)

                // Draw background pill
                sdf.box(
                    offset_px.x + self.border_size
                    offset_px.y + self.border_size
                    sz_px.x - self.border_size * 2.
                    sz_px.y - self.border_size * 2.
                    self.border_radius * self.size * 0.1
                )

                let color_fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)

                let color_stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_active, self.active)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_down, self.down)
                    .mix(self.border_color_disabled, self.disabled)

                sdf.fill_keep(color_fill)
                sdf.stroke(color_stroke, self.border_size)

                // Draw toggle mark
                let mark_padding = 1.5
                let mark_size = sz_px.y * 0.5 - self.border_size - mark_padding
                let mark_target_y = sz_px.y - sz_px.x + self.border_size + mark_padding
                let mark_pos_y = sz_px.y * 0.5 + self.border_size - mark_target_y * self.active

                // Draw ring when off, filled circle when on
                sdf.circle(mark_pos_y, center_px.y, mark_size)
                sdf.circle(mark_pos_y, center_px.y, mark_size * 0.45)
                sdf.subtract()

                sdf.circle(mark_pos_y, center_px.y, mark_size)
                sdf.blend(self.active)

                let mark_color = self.mark_color
                    .mix(self.mark_color_hover, self.hover)
                    .mix(self.mark_color_active, self.active)
                    .mix(self.mark_color_disabled, self.disabled)

                sdf.fill(mark_color)
                return sdf.result
            }
        }

        animator +: {
            active: {
                default: @off
                off: AnimatorState{
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.Toggle = mod.widgets.ToggleFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_down: theme.color_bevel_inset_1_down
            border_color_active: theme.color_bevel_inset_1_active
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }

    mod.widgets.CheckBoxCustom = mod.widgets.CheckBox{
        width: Fit
        height: Fit
        padding: theme.mspace_2
        align: Align{x: 0., y: 0.5}

        label_walk +: {
            margin: theme.mspace_h_2
        }

        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.fill(vec4(0., 0., 0., 0.))
                return sdf.result
            }
        }
    }
}

#[derive(Script, Widget, Animator)]
pub struct CheckBox {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[live]
    label_align: Align,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,

    #[live]
    draw_icon: DrawSvg,

    #[live]
    text: ArcStringMut,

    #[visible]
    #[live(true)]
    pub visible: bool,

    #[live(None)]
    pub active: Option<bool>,

    #[live]
    on_click: ScriptFnRef,

    #[live]
    bind: String,
    #[action_data]
    #[rust]
    action_data: WidgetActionData,
}

impl ScriptHook for CheckBox {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        if let Some(active) = self.active.take() {
            vm.with_cx_mut(|cx| {
                self.animator_toggle(cx, active, Animate::No, ids!(active.on), ids!(active.off));
            });
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum CheckBoxAction {
    Change(bool),
    #[default]
    None,
}

impl CheckBox {
    pub fn draw_check_box(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);

        self.draw_icon.draw_walk(cx, self.icon_walk);

        self.draw_text
            .draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    pub fn changed(&self, actions: &Actions) -> Option<bool> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let CheckBoxAction::Change(b) = item.cast() {
                return Some(b);
            }
        }
        None
    }

    pub fn active(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }

    pub fn set_active(&mut self, cx: &mut Cx, value: bool) {
        self.animator_toggle(cx, value, Animate::Yes, ids!(active.on), ids!(active.off));
    }

    pub fn debug_dump_animator(&self, heap: &ScriptHeap) -> String {
        self.animator.debug_dump(heap)
    }
}

impl Widget for CheckBox {
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

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(checked) {
            let is_active = vm.with_cx(|cx| self.animator_in_state(cx, ids!(active.on)));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(is_active));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.set_key_focus(cx);
                let new_active = if self.animator_in_state(cx, ids!(active.on)) {
                    self.animator_play(cx, ids!(active.off));
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        CheckBoxAction::Change(false),
                    );
                    false
                } else {
                    self.animator_play(cx, ids!(active.on));
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        CheckBoxAction::Change(true),
                    );
                    true
                };
                cx.widget_to_script_call(
                    uid,
                    NIL,
                    self.source.clone(),
                    self.on_click.clone(),
                    &[ScriptValue::from_bool(new_active)],
                );
            }
            Hit::FingerUp(_fe) => {}
            Hit::FingerMove(_fe) => {}
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_check_box(cx, walk)
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl CheckBoxRef {
    pub fn changed(&self, actions: &Actions) -> Option<bool> {
        self.borrow().and_then(|inner| inner.changed(actions))
    }

    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }

    pub fn active(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.active(cx)
        } else {
            false
        }
    }

    pub fn set_active(&self, cx: &mut Cx, value: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, value);
        }
    }

    pub fn debug_dump_animator(&self, heap: &ScriptHeap) -> String {
        if let Some(inner) = self.borrow() {
            inner.debug_dump_animator(heap)
        } else {
            "no borrow".to_string()
        }
    }
}
