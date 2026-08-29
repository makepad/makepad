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

    mod.widgets.ButtonBase = #(Button::register_widget(vm))

    /** The flat button: the standard face with no outset gradient; the
     * other button variants inherit from it. */
    mod.widgets.ButtonFlat = set_type_default() do mod.widgets.ButtonBase{
        /** the label text */
        text: "Button"
        width: Fit
        height: Fit
        /** gap between icon and label 0..24 step 1 */
        spacing: theme.space_2
        align: Center
        /** inner face padding around icon and label */
        padding: theme.mspace_1{left: theme.space_2, right: theme.space_2}
        /** outer gap to neighbouring widgets */
        margin: theme.mspace_v_1
        /** the label's box inside the face */
        label_walk: Walk{width: Fit, height: Fit}

        /** The button label ink, state-mixed with the face. */
        draw_text +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: 0.0
            /** pressed mix 0..1 step 0.01 */
            down: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            // A button face is a box with one line of text in it: center the
            // ink, not the line box, or the label reads as sitting high.
            /** center the ink, not the line box */
            ink_centered: true

            /** label ink at rest */
            color: theme.color_label_inner
            /** label ink under the pointer */
            color_hover: theme.color_label_inner_hover
            /** label ink while held */
            color_down: uniform(theme.color_label_inner_down)
            /** label ink when key-focused */
            color_focus: uniform(theme.color_label_inner_focus)
            /** label ink when disabled */
            color_disabled: uniform(theme.color_label_inner_disabled)

            /** the label typeface */
            text_style: theme.font_regular{
                /** label type size in points 6..32 step 0.5 */
                font_size: theme.font_size_p
            }
            /** ink mix order: focus, hover, down, disabled */
            get_color: fn() {
                return self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)
            }
        }

        /** the icon's box inside the face; draw_icon paints the SVG into it */
        icon_walk: Walk{/** icon width in pixels 8..64 step 1 */ width: 22.0, height: Fit}

        /** The button face material: an SDF box with a bevel stroke and an
         * optional two-stop gradient fill, state-mixed by the animator's
         * hover/down/focus/disabled instances. */
        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** pressed mix 0..1 step 0.01 */
            down: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)

            /** dither the gradient fill to hide banding 0..1 step 1 */
            color_dither: uniform(1.0)
            /** bevel gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_border_horizontal: uniform(0.0)
            /** fill gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_fill_horizontal: uniform(0.0)

            /** face fill at rest */
            color: uniform(theme.color_outset)
            /** face fill under the pointer */
            color_hover: uniform(theme.color_outset_hover)
            /** face fill while held */
            color_down: uniform(theme.color_outset_down)
            /** face fill when key-focused */
            color_focus: uniform(theme.color_outset_focus)
            /** face fill when disabled */
            color_disabled: uniform(theme.color_outset_disabled)

            /** fill gradient end stop; negative alpha means flat fill */
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            /** fill end stop under the pointer */
            color_2_hover: uniform(theme.color_outset_2_hover)
            /** fill end stop while held */
            color_2_down: uniform(theme.color_outset_2_down)
            /** fill end stop when key-focused */
            color_2_focus: uniform(theme.color_outset_2_focus)
            /** fill end stop when disabled */
            color_2_disabled: uniform(theme.color_outset_2_disabled)

            /** bevel stroke at rest */
            border_color: uniform(theme.color_bevel)
            /** bevel stroke under the pointer */
            border_color_hover: uniform(theme.color_bevel_hover)
            /** bevel stroke while held */
            border_color_down: uniform(theme.color_bevel_down)
            /** bevel stroke when key-focused */
            border_color_focus: uniform(theme.color_bevel_focus)
            /** bevel stroke when disabled */
            border_color_disabled: uniform(theme.color_bevel_disabled)

            /** bevel gradient end stop; negative alpha means flat stroke */
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            /** bevel end stop under the pointer */
            border_color_2_hover: uniform(theme.color_bevel_outset_2_hover)
            /** bevel end stop while held */
            border_color_2_down: uniform(theme.color_bevel_outset_2_down)
            /** bevel end stop when key-focused */
            border_color_2_focus: uniform(theme.color_bevel_outset_2_focus)
            /** bevel end stop when disabled */
            border_color_2_disabled: uniform(theme.color_bevel_outset_2_disabled)

            /** the face: rounded SDF box, gradient fill, bevel stroke */
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
                let mut color_fill_down = self.color_down
                let mut color_fill_focus = self.color_focus
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
                    color_fill_down = mix(self.color_down, self.color_2_down, dir)
                    color_fill_focus = mix(self.color_focus, self.color_2_focus, dir)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, dir)
                }

                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_down = self.border_color_down
                let mut color_stroke_focus = self.border_color_focus
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
                    color_stroke_down = mix(self.border_color_down, self.border_color_2_down, dir)
                    color_stroke_focus = mix(self.border_color_focus, self.border_color_2_focus, dir)
                    color_stroke_disabled = mix(self.border_color_disabled, self.border_color_2_disabled, dir)
                }

                let fill = color_fill
                    .mix(color_fill_focus, self.focus)
                    .mix(color_fill_hover, self.hover)
                    .mix(color_fill_down, self.down)
                    .mix(color_fill_disabled, self.disabled)

                let stroke = color_stroke
                    .mix(color_stroke_focus, self.focus)
                    .mix(color_stroke_hover, self.hover)
                    .mix(color_stroke_down, self.down)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)
                return sdf.result
            }
        }

        /** the state machine driving the face and label mixes */
        animator: Animator{
            /** enabled/disabled track: drives the disabled mix on face and label */
            disabled: {
                default: @off
                /** enabled: face and label at full strength, cut instantly */
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                /** disabled: 0.2s fade of face and label to the disabled colors */
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            /** free-running clock track for animated faces */
            time: {
                default: @off
                /** clock stopped: nothing applied */
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                    }
                }
                /** clock running: ramps draw_bg.anim_time 0..1 once per second, forever */
                on: AnimatorState{
                    from: {all: Loop {duration: 1.0, end: 1000000000.0}}
                    apply: {
                        draw_bg: {anim_time: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
                    }
                }
            }
            /** pointer track: drives both the hover and down mixes */
            hover: {
                default: @off
                /** pointer away: 0.1s fade of hover and down back to 0 */
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                /** pointer over: hover snaps to 1, down releases in 0.01s */
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

                /** held down: down snaps to 1 with hover held on under it */
                down: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            /** keyboard-focus track: drives the focus mix on face and label */
            focus: {
                default: @off
                /** focus lost: 0.2s fade of the focus mix back to 0 */
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                /** focused: focus mix on immediately, arrow cursor */
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

    /** The borderless button: face and bevel hidden until disabled. */
    mod.widgets.ButtonFlatter = mod.widgets.ButtonFlat{
        draw_bg +: {
            color: theme.color_u_hidden
            color_hover: theme.color_u_hidden
            color_down: theme.color_u_hidden
            /** the one state that paints a face: disabled */
            color_disabled: theme.color_outset_disabled

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_down: theme.color_u_hidden
            border_color_focus: theme.color_u_hidden
            border_color_disabled: theme.color_u_hidden
        }
    }

    /** The standard button: the flat face plus the outset gradient fill
     * and bevel colors from the theme. */
    mod.widgets.Button = mod.widgets.ButtonFlat{
        draw_bg +: {
            border_color: theme.color_bevel_outset_1
            border_color_hover: theme.color_bevel_outset_1_hover
            border_color_down: theme.color_bevel_outset_1_down
            border_color_focus: theme.color_bevel_outset_1_focus
            border_color_disabled: theme.color_bevel_outset_1_disabled

            /** second bevel stop: turns the flat stroke into a gradient */
            border_color_2: theme.color_bevel_outset_2
            border_color_2_hover: theme.color_bevel_outset_2_hover
            border_color_2_down: theme.color_bevel_outset_2_down
            border_color_2_focus: theme.color_bevel_outset_2_focus
            border_color_2_disabled: theme.color_bevel_outset_2_disabled
        }
    }

    /** The gradient button: the standard face filled with a two-stop
     * vertical gradient instead of a flat color. */
    mod.widgets.ButtonGradientX = mod.widgets.Button{
        draw_bg +: {
            /** gradient start stop at rest */
            color: theme.color_outset_1
            /** gradient start stop under the pointer */
            color_hover: theme.color_outset_1_hover
            /** gradient start stop while held */
            color_down: theme.color_outset_1_down
            /** gradient start stop when key-focused */
            color_focus: theme.color_outset_1_focus
            /** gradient start stop when disabled */
            color_disabled: theme.color_outset_1_disabled

            /** second fill stop: a positive alpha here switches the fill to a gradient */
            color_2: theme.color_outset_2
        }
    }

    /** The gradient button turned sideways: the same fill run left to right. */
    mod.widgets.ButtonGradientY = mod.widgets.ButtonGradientX{
        draw_bg.gradient_fill_horizontal: /** fill gradient axis: 1 horizontal 0..1 step 1 */ 1.0
    }

    /** The standard button carrying only an icon: no label, no gap. */
    mod.widgets.ButtonIcon = mod.widgets.Button{
        /** no gap: there is no label to sit beside the icon 0..24 step 1 */
        spacing: 0.
        /** icon only: the label is empty */
        text: ""
    }

    /** The vertical-gradient button carrying only an icon. */
    mod.widgets.ButtonGradientXIcon = mod.widgets.ButtonGradientX{
        /** no gap: there is no label to sit beside the icon 0..24 step 1 */
        spacing: 0.
        /** icon only: the label is empty */
        text: ""
    }

    /** The horizontal-gradient button carrying only an icon. */
    mod.widgets.ButtonGradientYIcon = mod.widgets.ButtonGradientY{
        /** no gap: there is no label to sit beside the icon 0..24 step 1 */
        spacing: 0.
        /** icon only: the label is empty */
        text: ""
    }

    /** The flat button carrying only an icon. */
    mod.widgets.ButtonFlatIcon = mod.widgets.ButtonFlat{
        /** no gap: there is no label to sit beside the icon 0..24 step 1 */
        spacing: 0.
        /** icon only: the label is empty */
        text: ""
    }

    /** The borderless button carrying only an icon: a bare clickable mark. */
    mod.widgets.ButtonFlatterIcon = mod.widgets.ButtonFlatter{
        draw_bg.color_focus: theme.color_u_hidden
        /** no gap: there is no label to sit beside the icon 0..24 step 1 */
        spacing: 0.
        /** icon only: the label is empty */
        text: ""
    }
}

/// Actions emitted by a button widget, including the key modifiers
/// that were active when the action occurred.
///
/// The sequence of actions emitted by a button is as follows:
/// 1. `ButtonAction::Pressed` when the button is pressed.
/// 2. `ButtonAction::LongPressed` when the button has been pressed for a long time.
///    * This only occurs on platforms that support a *native* long press, e.g., mobile.
/// 3. Then, either one of the following, but not both:
///    * `ButtonAction::Clicked` when the mouse/finger is lifted up while over the button area.
///    * `ButtonAction::Released` when the mouse/finger is lifted up while *not* over the button area.
#[derive(Clone, Debug, Default)]
pub enum ButtonAction {
    #[default]
    None,
    /// The button was pressed (a "down" event).
    Pressed(KeyModifiers),
    /// The button was pressed for a long time (only occurs on mobile platforms).
    LongPressed,
    /// The button was clicked (an "up" event).
    Clicked(KeyModifiers),
    /// The button was released (an "up" event), but should not be considered clicked
    /// because the mouse/finger was not over the button area when released.
    Released(KeyModifiers),
}

/// A clickable button widget that emits actions when pressed, and when either released or clicked.
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct Button {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    /// Public so a host can recolour the mark directly. Going through a
    /// script apply instead re-applies the whole object and drops the loaded
    /// document, which renders the icon as a white silhouette.
    pub draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[walk]
    walk: Walk,

    #[layout]
    layout: Layout,

    #[live(true)]
    grab_key_focus: bool,

    #[live(true)]
    enabled: bool,

    #[live(true)]
    #[visible]
    visible: bool,

    /// Set the long-press handling behavior of this button.
    /// * If `false` (default), the button will ignore long-press events
    ///   and will never emit [`ButtonAction::LongPressed`].
    ///   * Also, the button logic will *not* call [`FingerUpEvent::was_tap()`]
    ///     to check if the button press was a short tap.
    ///     This means that this button will consider itself to be clicked
    ///     (and thus emit a [`ButtonAction::Clicked`] event)
    ///     if the finger-up/release event occurs within the button area,
    ///     *regardless* of how long the button was pressed down before it was released.
    /// * If `true`, the button will respond to a long-press event
    ///   by emitting [`ButtonAction::LongPressed`], which can only occur on
    ///   mobile platforms that support a *native* long press event.
    ///   * Also, the button will only consider itself to be clicked
    ///     (and thus emit [`ButtonAction::Clicked`]) if [`FingerUpEvent::was_tap()`] returns `true`,
    ///     meaning that a long press did *not* occur and that the button was released over the button area
    ///     within a short time frame (~0.5 seconds) after the initial down press.
    #[live]
    pub enable_long_press: bool,

    /// It indicates if the hover state will be reset when the button is clicked.
    /// This could be useful for buttons that disappear when clicked, where the hover state
    /// should not be preserved.
    #[live]
    reset_hover_on_click: bool,

    #[live]
    pub text: ArcStringMut,

    #[live]
    on_click: ScriptFnRef,

    #[live]
    on_press: ScriptFnRef,

    /// Legacy compatibility flag that fires `on_click` on press instead of click.
    #[live]
    trigger_on_press: bool,

    #[action_data]
    #[rust]
    action_data: WidgetActionData,
}

impl Widget for Button {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(text) {
            let str_val = vm.bx.heap.new_string_from_str(self.text.as_ref());
            return ScriptAsyncResult::Return(str_val.into());
        }
        if method == live_id!(set_text) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(new_text) = vm
                        .bx
                        .heap
                        .cast_to_owned_string(value, "copying button text")
                    {
                        vm.with_cx_mut(|cx| {
                            self.set_text(cx, &new_text);
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(on_click) {
            let uid = self.widget_uid();
            vm.with_cx_mut(|cx| {
                cx.widget_to_script_call(uid, NIL, self.source.clone(), self.on_click.clone(), &[]);
            });
            return ScriptAsyncResult::Return(TRUE);
        }
        if method == live_id!(on_press) {
            let uid = self.widget_uid();
            vm.with_cx_mut(|cx| {
                cx.widget_to_script_call(uid, NIL, self.source.clone(), self.on_press.clone(), &[]);
            });
            return ScriptAsyncResult::Return(TRUE);
        }
        ScriptAsyncResult::MethodNotFound
    }

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
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        if let Event::ClearHover = event {
            self.animator_cut(cx, ids!(hover.off));
        }

        // The button only handles hits when it's visible and enabled.
        // If it's not enabled, we still show the button, but we set
        // the NotAllowed mouse cursor upon hover instead of the Hand cursor.
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if self.enabled && fe.is_primary_hit() => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_bg.area());
                }
                cx.widget_action_with_data(
                    &self.action_data,
                    uid,
                    ButtonAction::Pressed(fe.modifiers),
                );
                cx.widget_to_script_call(uid, NIL, self.source.clone(), self.on_press.clone(), &[]);
                if self.trigger_on_press {
                    cx.widget_to_script_call(
                        uid,
                        NIL,
                        self.source.clone(),
                        self.on_click.clone(),
                        &[],
                    );
                }
                self.animator_play(cx, ids!(hover.down));
                self.set_key_focus(cx);
            }
            Hit::FingerHoverIn(_) => {
                if self.enabled {
                    cx.set_cursor(MouseCursor::Hand);
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    cx.set_cursor(MouseCursor::NotAllowed);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerLongPress(_lp) if self.enabled && self.enable_long_press => {
                cx.widget_action_with_data(&self.action_data, uid, ButtonAction::LongPressed);
            }
            Hit::FingerUp(fe) if self.enabled && fe.is_primary_hit() => {
                let was_clicked = fe.is_over
                    && if self.enable_long_press {
                        fe.was_tap()
                    } else {
                        true
                    };
                if was_clicked {
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        ButtonAction::Clicked(fe.modifiers),
                    );
                    if !self.trigger_on_press {
                        cx.widget_to_script_call(
                            uid,
                            NIL,
                            self.source.clone(),
                            self.on_click.clone(),
                            &[],
                        );
                    }
                    if self.reset_hover_on_click {
                        self.animator_cut(cx, ids!(hover.off));
                    } else if fe.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                } else {
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        ButtonAction::Released(fe.modifiers),
                    );
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text
            .draw_walk(cx, self.label_walk, Align::default(), self.text.as_ref());
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text.as_ref() == v { return }
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl Button {
    pub fn draw_button(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text
            .draw_walk(cx, self.label_walk, Align::default(), label);
        self.draw_bg.end(cx);
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Returns `true` if this button was clicked.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.clicked_modifiers(actions).is_some()
    }

    /// Returns `true` if this button was pressed down.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.pressed_modifiers(actions).is_some()
    }

    /// Returns `true` if this button was long-pressed on.
    ///
    /// Note that this does not mean the button has been released yet.
    /// See [`ButtonAction`] for more details.
    pub fn long_pressed(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), ButtonAction::LongPressed)
        } else {
            false
        }
    }

    /// Returns `true` if this button was released, which is *not* considered to be clicked.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn released(&self, actions: &Actions) -> bool {
        self.released_modifiers(actions).is_some()
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was clicked.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ButtonAction::Clicked(m) = item.cast() {
                return Some(m);
            }
        }
        None
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was pressed down.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ButtonAction::Pressed(m) = item.cast() {
                return Some(m);
            }
        }
        None
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was released,
    /// which is *not* considered to be clicked.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ButtonAction::Released(m) = item.cast() {
                return Some(m);
            }
        }
        None
    }
}

impl ButtonRef {
    /// See [`Button::clicked()`].
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.clicked(actions))
    }

    /// See [`Button::pressed()`].
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.pressed(actions))
    }

    /// See [`Button::long_pressed()`].
    pub fn long_pressed(&self, actions: &Actions) -> bool {
        self.borrow()
            .is_some_and(|inner| inner.long_pressed(actions))
    }

    /// See [`Button::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.released(actions))
    }

    /// See [`Button::clicked_modifiers()`].
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow()
            .and_then(|inner| inner.clicked_modifiers(actions))
    }

    /// See [`Button::pressed_modifiers()`].
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow()
            .and_then(|inner| inner.pressed_modifiers(actions))
    }

    /// See [`Button::released_modifiers()`].
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow()
            .and_then(|inner| inner.released_modifiers(actions))
    }

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.visible = visible;
            inner.redraw(cx);
        }
    }

    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.enabled = enabled;
            inner.redraw(cx);
        }
    }

    /// Resets the hover state of this button.
    ///
    /// This is useful in certain cases where the hover state should be reset
    /// (cleared) regardelss of whether the mouse is over it.
    pub fn reset_hover(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_cut(cx, ids!(hover.off));
        }
    }
}

impl ButtonSet {
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.clicked(actions))
    }
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.pressed(actions))
    }
    pub fn released(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.released(actions))
    }

    pub fn reset_hover(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.reset_hover(cx)
        }
    }

    pub fn which_clicked_modifiers(&self, actions: &Actions) -> Option<(usize, KeyModifiers)> {
        for (index, btn) in self.iter().enumerate() {
            if let Some(km) = btn.clicked_modifiers(actions) {
                return Some((index, km));
            }
        }
        None
    }

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        for item in self.iter() {
            item.set_visible(cx, visible)
        }
    }
    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        for item in self.iter() {
            item.set_enabled(cx, enabled)
        }
    }
}
