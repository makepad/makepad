use crate::{
    button::{Button, ButtonAction},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget_async::ScriptAsyncResult,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LinkLabelBase = #(LinkLabel::register_widget(vm))

    mod.widgets.LinkLabel = set_type_default() do mod.widgets.LinkLabelBase{
        width: Fit
        height: Fit
        spacing: theme.space_2
        align: Center
        padding: 0.
        margin: theme.mspace_v_2
        label_walk: Walk{width: Fit, height: Fit}

        draw_text +: {
            hover: instance(0.0)
            down: instance(0.0)
            focus: instance(0.0)
            disabled: instance(0.0)

            color_dither: uniform(1.0)
            gradient_fill_horizontal: uniform(0.0)

            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_down: uniform(theme.color_label_inner_down)
            color_focus: uniform(theme.color_label_inner_focus)
            color_disabled: uniform(theme.color_label_inner_disabled)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_label_inner_hover)
            color_2_down: uniform(theme.color_label_inner_down)
            color_2_focus: uniform(theme.color_label_inner_focus)
            color_2_disabled: uniform(theme.color_label_inner_disabled)

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }

            get_color: fn() {
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let mut c2 = self.color
                let mut c2_hover = self.color_hover
                let mut c2_down = self.color_down
                let mut c2_focus = self.color_focus
                let mut c2_disabled = self.color_disabled

                if self.color_2.x > -0.5 {
                    c2 = self.color_2
                    c2_hover = self.color_2_hover
                    c2_down = self.color_2_down
                    c2_focus = self.color_2_focus
                    c2_disabled = self.color_2_disabled
                }

                let mut gradient_dir = self.pos.y + dither
                if self.gradient_fill_horizontal > 0.5 {
                    gradient_dir = self.pos.x + dither
                }

                return mix(self.color, c2, gradient_dir)
                    .mix(mix(self.color_focus, c2_focus, gradient_dir), self.focus)
                    .mix(mix(self.color_hover, c2_hover, gradient_dir), self.hover)
                    .mix(mix(self.color_down, c2_down, gradient_dir), self.down)
                    .mix(mix(self.color_disabled, c2_disabled, gradient_dir), self.disabled)
            }
        }

        icon_walk: Walk{width: theme.font_size_p, height: Fit}

        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            down: instance(0.0)
            disabled: instance(0.0)

            color_dither: uniform(1.0)
            gradient_fill_horizontal: uniform(0.0)

            color: uniform(theme.color_label_inner)
            color_hover: uniform(theme.color_label_inner_hover)
            color_down: uniform(theme.color_label_inner_down)
            color_focus: uniform(theme.color_label_inner_focus)
            color_disabled: uniform(theme.color_label_inner_disabled)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_label_inner_hover)
            color_2_down: uniform(theme.color_label_inner_down)
            color_2_focus: uniform(theme.color_label_inner_focus)
            color_2_disabled: uniform(theme.color_label_inner_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let offset_y = 1.0

                let mut c2 = self.color
                let mut c2_hover = self.color_hover
                let mut c2_down = self.color_down
                let mut c2_focus = self.color_focus
                let mut c2_disabled = self.color_disabled

                if self.color_2.x > -0.5 {
                    c2 = self.color_2
                    c2_hover = self.color_2_hover
                    c2_down = self.color_2_down
                    c2_focus = self.color_2_focus
                    c2_disabled = self.color_2_disabled
                }

                let mut gradient_dir = self.pos.y + dither
                if self.gradient_fill_horizontal > 0.5 {
                    gradient_dir = self.pos.x + dither
                }

                sdf.move_to(0., self.rect_size.y - offset_y)
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y)

                let stroke_color = mix(self.color, c2, gradient_dir)
                    .mix(mix(self.color_focus, c2_focus, gradient_dir), self.focus)
                    .mix(mix(self.color_hover, c2_hover, gradient_dir), self.hover)
                    .mix(mix(self.color_down, c2_down, gradient_dir), self.down)
                    .mix(mix(self.color_disabled, c2_disabled, gradient_dir), self.disabled)

                return sdf.stroke(stroke_color, mix(0.7, 1., self.hover))
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
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: snap(1.0)}
                        draw_text: {down: 0.0, hover: snap(1.0)}
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
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Arrow
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.LinkLabelIcon = mod.widgets.LinkLabel{
        padding: Inset{bottom: 2.}
        align: Align{x: 0.0, y: 0.0}
        label_walk: Walk{margin: Inset{left: theme.space_2}}
    }
}

/// A clickable label widget that opens a URL when clicked.
///
/// This is a wrapper around (and derefs to) a [`Button`] widget.
#[derive(Script, ScriptHook, Widget)]
pub struct LinkLabel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[deref]
    button: Button,
    #[live]
    pub url: String,
    #[live]
    pub open_in_place: bool,
}

impl Widget for LinkLabel {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(text) {
            let str_val = vm.bx.heap.new_string_from_str(self.button.text.as_ref());
            return ScriptAsyncResult::Return(str_val.into());
        }
        if method == live_id!(set_text) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    let new_text = vm.bx.heap.temp_string_with(|heap, out| {
                        heap.cast_to_string(value, out);
                        out.to_string()
                    });
                    vm.with_cx_mut(|cx| {
                        self.set_text(cx, &new_text);
                    });
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| {
            self.button.handle_event(cx, event, scope);
        });
        if self.url.len() > 0 && self.clicked(&actions) {
            cx.open_url(
                &self.url,
                if self.open_in_place {
                    OpenUrlInPlace::Yes
                } else {
                    OpenUrlInPlace::No
                },
            );
        }
        cx.extend_actions(actions);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.button.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.button.text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.button.set_text(cx, v);
    }
}

impl LinkLabel {
    /// Returns `true` if this link label was clicked.
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.clicked_modifiers(actions).is_some()
    }

    /// Returns `true` if this link label was pressed down.
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.pressed_modifiers(actions).is_some()
    }

    /// Returns `true` if this link label was released.
    pub fn released(&self, actions: &Actions) -> bool {
        self.released_modifiers(actions).is_some()
    }

    /// Returns `Some` (with active keyboard modifiers) if this link label was clicked.
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ButtonAction::Clicked(m) = item.cast() {
                return Some(m);
            }
        }
        None
    }

    /// Returns `Some` (with active keyboard modifiers) if this link label was pressed down.
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ButtonAction::Pressed(m) = item.cast() {
                return Some(m);
            }
        }
        None
    }

    /// Returns `Some` (with active keyboard modifiers) if this link label was released.
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ButtonAction::Released(m) = item.cast() {
                return Some(m);
            }
        }
        None
    }
}

impl LinkLabelRef {
    /// See [`LinkLabel::clicked()`].
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.clicked(actions))
    }

    /// See [`LinkLabel::pressed()`].
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.pressed(actions))
    }

    /// See [`LinkLabel::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.released(actions))
    }

    /// See [`LinkLabel::clicked_modifiers()`].
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.clicked_modifiers(actions))
    }

    /// See [`LinkLabel::pressed_modifiers()`].
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.pressed_modifiers(actions))
    }

    /// See [`LinkLabel::released_modifiers()`].
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.released_modifiers(actions))
    }

    pub fn set_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    pub fn set_url(&self, url: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.url = url.to_string();
        }
    }
}
