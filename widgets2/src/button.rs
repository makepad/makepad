use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    animator::{Animator, AnimatorImpl, Animate, AnimatorAction},
};

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.ButtonBase = #(Button::register_widget(vm))
    
    mod.widgets.ButtonFlat = mod.std.set_type_default() do mod.widgets.ButtonBase{
        text: "Button"
        width: Fit
        height: Fit
        spacing: theme.space_2
        align: Center
        padding: theme.mspace_1{left: theme.space_2, right: theme.space_2}
        margin: theme.mspace_v_1
        label_walk: Walk{width: Fit, height: Fit}

        draw_text +: {
            hover: instance(0.0)
            down: instance(0.0)
            focus: instance(0.0)
            disabled: instance(0.0)

            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_down: uniform(theme.color_label_inner_down)
            color_focus: uniform(theme.color_label_inner_focus)
            color_disabled: uniform(theme.color_label_inner_disabled)

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            get_color: fn() {
                return mix(
                    mix(
                        mix(
                            mix(self.color, self.color_focus, self.focus)
                            self.color_hover
                            self.hover
                        )
                        self.color_down
                        self.down
                    )
                    self.color_disabled
                    self.disabled
                )
            }
        }
        
        // TODO: icon_walk and draw_icon not yet available (DrawIcon missing from draw2)
        
        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            down: instance(0.0)
            enabled: instance(1.0)
            disabled: instance(1.0)

            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)

            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color: uniform(theme.color_outset)
            color_hover: uniform(theme.color_outset_hover)
            color_down: uniform(theme.color_outset_down)
            color_focus: uniform(theme.color_outset_focus)
            color_disabled: uniform(theme.color_outset_disabled)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_outset_2_hover)
            color_2_down: uniform(theme.color_outset_2_down)
            color_2_focus: uniform(theme.color_outset_2_focus)
            color_2_disabled: uniform(theme.color_outset_2_disabled)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_down: uniform(theme.color_bevel_down)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_disabled: uniform(theme.color_bevel_disabled)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_bevel_outset_2_hover)
            border_color_2_down: uniform(theme.color_bevel_outset_2_down)
            border_color_2_focus: uniform(theme.color_bevel_outset_2_focus)
            border_color_2_disabled: uniform(theme.color_bevel_outset_2_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                
                let mut color_2 = self.color
                let mut color_2_hover = self.color_hover
                let mut color_2_down = self.color_down
                let mut color_2_focus = self.color_focus
                let mut color_2_disabled = self.color_disabled

                let mut border_color_2 = self.border_color
                let mut border_color_2_hover = self.border_color_hover
                let mut border_color_2_down = self.border_color_down
                let mut border_color_2_focus = self.border_color_focus
                let mut border_color_2_disabled = self.border_color_disabled

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                    color_2_hover = self.color_2_hover
                    color_2_down = self.color_2_down
                    color_2_focus = self.color_2_focus
                    color_2_disabled = self.color_2_disabled
                }

                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                    border_color_2_hover = self.border_color_2_hover
                    border_color_2_down = self.border_color_2_down
                    border_color_2_focus = self.border_color_2_focus
                    border_color_2_disabled = self.border_color_2_disabled
                }
                
                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither
                    self.pos.y + dither
                )

                let mut gradient_border_dir = gradient_border.y
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = gradient_border.x
                }

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

                let mut gradient_fill_dir = gradient_fill.y
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = gradient_fill.x
                }

                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                
                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color, color_2, gradient_fill_dir)
                                    mix(self.color_focus, color_2_focus, gradient_fill_dir)
                                    self.focus
                                )
                                mix(self.color_hover, color_2_hover, gradient_fill_dir)
                                self.hover
                            )
                            mix(self.color_down, color_2_down, gradient_fill_dir)
                            self.down
                        )
                        mix(self.color_disabled, color_2_disabled, gradient_fill_dir)
                        self.disabled
                    )
                )
                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color, border_color_2, gradient_border_dir)
                                    mix(self.border_color_focus, border_color_2_focus, gradient_border_dir)
                                    self.focus
                                )
                                mix(self.border_color_hover, border_color_2_hover, gradient_border_dir)
                                self.hover
                            )
                            mix(self.border_color_down, border_color_2_down, gradient_border_dir)
                            self.down
                        )
                        mix(self.border_color_disabled, border_color_2_disabled, gradient_border_dir)
                        self.disabled
                    )
                    self.border_size
                )
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
            time: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                    }
                }
                on: AnimatorState{
                    from: {all: Loop {duration: 1.0, end: 1000000000.0}}
                    apply: {
                        draw_bg: {anim_time: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
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

    mod.widgets.ButtonFlatter = mod.widgets.ButtonFlat{
        draw_bg +: {
            color: theme.color_u_hidden
            color_hover: theme.color_u_hidden
            color_down: theme.color_u_hidden
            color_disabled: theme.color_outset_disabled

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_down: theme.color_u_hidden
            border_color_focus: theme.color_u_hidden
            border_color_disabled: theme.color_u_hidden
        }
    }

    mod.widgets.Button = mod.widgets.ButtonFlat{
        draw_bg +: {
            border_color: theme.color_bevel_outset_1
            border_color_hover: theme.color_bevel_outset_1_hover
            border_color_down: theme.color_bevel_outset_1_down
            border_color_focus: theme.color_bevel_outset_1_focus
            border_color_disabled: theme.color_bevel_outset_1_disabled

            border_color_2: theme.color_bevel_outset_2
            border_color_2_hover: theme.color_bevel_outset_2_hover
            border_color_2_down: theme.color_bevel_outset_2_down
            border_color_2_focus: theme.color_bevel_outset_2_focus
            border_color_2_disabled: theme.color_bevel_outset_2_disabled
        }
    }
 
    mod.widgets.ButtonGradientX = mod.widgets.Button{
        draw_bg +: {
            color: theme.color_outset_1
            color_hover: theme.color_outset_1_hover
            color_down: theme.color_outset_1_down
            color_focus: theme.color_outset_1_focus
            color_disabled: theme.color_outset_1_disabled

            color_2: theme.color_outset_2
        }
    }

    mod.widgets.ButtonGradientY = mod.widgets.ButtonGradientX{
        draw_bg +: {
            gradient_fill_horizontal: 1.0
        } 
    }
  
    mod.widgets.ButtonIcon = mod.widgets.Button{
        spacing: 0.
        text: ""
    }
    
    mod.widgets.ButtonGradientXIcon = mod.widgets.ButtonGradientX{
        spacing: 0.
        text: ""
    }
    
    mod.widgets.ButtonGradientYIcon = mod.widgets.ButtonGradientY{
        spacing: 0.
        text: ""
    }
    
    mod.widgets.ButtonFlatIcon = mod.widgets.ButtonFlat{
        spacing: 0.
        text: ""
    }
    
    mod.widgets.ButtonFlatterIcon = mod.widgets.ButtonFlatter{
        draw_bg +: {color_focus: theme.color_u_hidden}
        spacing: 0.
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
    #[source] source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    // TODO: DrawIcon not yet available in draw2
    // #[live]
    // draw_icon: DrawIcon,
    // #[live]
    // icon_walk: Walk,
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
    #[visible] visible: bool,

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
    
    #[action_data] #[rust] action_data: WidgetActionData,
}

impl Widget for Button {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }
                
    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        
        match event.hit_designer(cx, self.draw_bg.area()) {
            HitDesigner::DesignerPick(_e) => {
                cx.widget_action_with_data(&self.action_data, uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _ => ()
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
                cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Pressed(fe.modifiers));
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
                cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::LongPressed);
            }
            Hit::FingerUp(fe) if self.enabled && fe.is_primary_hit() => {
                let was_clicked = fe.is_over && if self.enable_long_press { fe.was_tap() } else { true };
                if was_clicked {
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Clicked(fe.modifiers));
                    if self.reset_hover_on_click {
                        self.animator_cut(cx, ids!(hover.off));
                    } else if fe.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                } else {
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Released(fe.modifiers));
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
        // TODO: draw_icon not yet available (DrawIcon missing from draw2)
        // self.draw_icon.draw_walk(cx, self.icon_walk);
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
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl Button {
        
    pub fn draw_button(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        // TODO: draw_icon not yet available (DrawIcon missing from draw2)
        // self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text
            .draw_walk(cx, self.label_walk, Align::default(), label);
        self.draw_bg.end(cx);
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
        self.borrow().is_some_and(|inner| inner.long_pressed(actions))
    }

    /// See [`Button::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.released(actions))
    }

    /// See [`Button::clicked_modifiers()`].
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|inner| inner.clicked_modifiers(actions))
    }

    /// See [`Button::pressed_modifiers()`].
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|inner| inner.pressed_modifiers(actions))
    }

    /// See [`Button::released_modifiers()`].
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|inner| inner.released_modifiers(actions))
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
                return Some((index, km))
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
