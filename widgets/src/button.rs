use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*,};

live_design! {
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    
    pub ButtonBase = {{Button}} {}
    pub Button = <ButtonBase> {
        text: ""
        width: Fit, height: Fit,
        spacing: (THEME_SPACE_2),
        align: {x: 0.5, y: 0.5},
        padding: <THEME_MSPACE_1> { left: (THEME_SPACE_2), right: (THEME_SPACE_2) }
        margin: <THEME_MSPACE_V_1> {}
        label_walk: { width: Fit, height: Fit },
        
        draw_text: {
            instance hover: 0.0,
            instance down: 0.0,
            instance focus: 0.0,
            instance disabled: 0.0

            color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            mix(self.color, self.color_focus, self.focus),
                            self.color_hover,
                            self.hover
                        ),
                        self.color_down,
                        self.down
                    ),
                    self.color_disabled,
                    self.disabled
                )
            }
        }
        
        icon_walk: {
            width: (THEME_DATA_ICON_WIDTH), height: Fit,
        }
        
        draw_icon: {
            instance hover: 0.0
            instance down: 0.0
            instance focus: 0.0
            instance disabled: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            mix(self.color, self.color_focus, self.focus),
                            self.color_hover,
                            self.hover
                        ),
                        self.color_down,
                        self.down
                    ),
                    self.color_disabled,
                    self.disabled
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance down: 0.0
            instance enabled: 1.0
            instance disabled: 0.0
            instance focus: 0.0

            uniform color_dither: 1.0

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color: (THEME_COLOR_OUTSET)
            uniform color_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform color_down: (THEME_COLOR_OUTSET_DOWN)
            uniform color_focus: (THEME_COLOR_OUTSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_OUTSET_DISABLED)

            uniform border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
            uniform border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

            uniform border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
            uniform border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color, self.color_focus, self.focus),
                                self.color_hover,
                                self.hover
                            ),
                            self.color_down,
                            self.down
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                )
                
                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                self.hover
                            ),
                            mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                            self.down
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )
                return sdf.result;
            }
        }
        
        animator: {
            disabled = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_icon: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
            time = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        //draw_bg: {anim_time: 0.0}
                    }
                }
                on = {
                    from: {all: Loop {duration: 1.0, end:1000000000.0}}
                    apply: {
                        draw_bg: {anim_time: [{time: 0.0, value: 0.0},{time:1.0, value:1.0}]}
                    }
                }
            }
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_icon: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }
    
    pub ButtonGradientX = <Button> {
        draw_bg: {
            instance hover: 0.0
            instance down: 0.0
            instance enabled: 1.0
            instance disabled: 1.0

            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS)

            color_dither: 1.0

            uniform color_1: (THEME_COLOR_OUTSET_1)
            uniform color_1_hover: (THEME_COLOR_OUTSET_1_HOVER)
            uniform color_1_down: (THEME_COLOR_OUTSET_1_DOWN)
            uniform color_1_focus: (THEME_COLOR_OUTSET_1_FOCUS)
            uniform color_1_disabled: (THEME_COLOR_OUTSET_1_DISABLED)

            uniform color_2: (THEME_COLOR_OUTSET_2)
            uniform color_2_hover: (THEME_COLOR_OUTSET_2_HOVER)
            uniform color_2_down: (THEME_COLOR_OUTSET_2_DOWN)
            uniform color_2_focus: (THEME_COLOR_OUTSET_2_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_OUTSET_2_DISABLED)

            border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
            border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
            border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
            border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)
            border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
            border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
            border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2_FOCUS)
            border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.x),
                                    mix(self.color_1_focus, self.color_2_focus, gradient_fill.x),
                                    self.focus
                                ),
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.x),
                                self.hover
                            ),
                            mix(self.color_1_down, self.color_2_down, gradient_fill.x),
                            self.down
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.x),
                        self.disabled
                    )
                )

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                self.hover
                            ),
                            mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                            self.down
                            ),
                            mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                            self.disabled
                        ), self.border_size
                )
                return sdf.result
            }
        }
    }

    pub ButtonGradientY = <ButtonGradientX> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.y),
                                    mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                                    self.focus
                                ),
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                self.hover
                            ),
                            mix(self.color_1_down, self.color_2_down, gradient_fill.y),
                            self.down
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
                        self.disabled
                    )
                )

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix((THEME_COLOR_BEVEL_OUTSET_1), self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                self.hover
                            ),
                            mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                            self.down
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                return sdf.result
            }
        }
    }

    pub ButtonFlat = <Button> {
        draw_bg: {
            color: (THEME_COLOR_OUTSET)
            color_hover: (THEME_COLOR_OUTSET_HOVER)
            color_down: (THEME_COLOR_OUTSET_DOWN)
            color_disabled: (THEME_COLOR_OUTSET_DISABLED)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_1_disabled: (THEME_COLOR_BEVEL_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_2_disabled: (THEME_COLOR_BEVEL_DISABLED)

        }
        
    }
    
    pub ButtonFlatter = <ButtonFlat> {
        draw_bg: {
            
            color: (THEME_COLOR_U_HIDDEN)
            color_hover: (THEME_COLOR_U_HIDDEN)
            color_down: (THEME_COLOR_U_HIDDEN)
            color_disabled: (THEME_COLOR_OUTSET_DISABLED)

            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_down: (THEME_COLOR_U_HIDDEN)
            border_color_1_focus: (THEME_COLOR_U_HIDDEN)
            border_color_1_disabled: (THEME_COLOR_U_HIDDEN)

            border_color_2: (THEME_COLOR_U_HIDDEN)
            border_color_2_hover: (THEME_COLOR_U_HIDDEN)
            border_color_2_down: (THEME_COLOR_U_HIDDEN)
            border_color_2_focus: (THEME_COLOR_U_HIDDEN)
            border_color_2_disabled: (THEME_COLOR_U_HIDDEN)
        }
    }
    
    pub ButtonIcon = <Button> {
        spacing: 0.
        text: ""
    }
    
    pub ButtonGradientXIcon = <ButtonGradientX> {
        spacing: 0.
        text: ""
    }
    
    pub ButtonGradientYIcon = <ButtonGradientY> {
        spacing: 0.
        text: ""
    }
    
    pub ButtonFlatIcon = <ButtonFlat> {
        spacing: 0.
        text: ""
    }
    
    pub ButtonFlatterIcon = <ButtonFlatter> {
        draw_bg: { color_focus: (THEME_COLOR_U_HIDDEN)}        
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
#[derive(Clone, Debug, DefaultNone)]
pub enum ButtonAction {
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
#[derive(Live, LiveHook, Widget)]
pub struct Button {
    #[animator]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_icon: DrawIcon,
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
    fn set_disabled(&mut self, cx:&mut Cx, disabled:bool){
        self.animator_toggle(cx, disabled, Animate::Yes, id!(disabled.on), id!(disabled.off));
    }
                
    fn disabled(&self, cx:&Cx) -> bool {
        self.animator_in_state(cx, id!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        
        match event.hit_designer(cx, self.draw_bg.area()){
            HitDesigner::DesignerPick(_e)=>{
                cx.widget_action_with_data(&self.action_data, uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _=>()
        }
        
        
        // The button only handles hits when it's visible and enabled.
        // If it's not enabled, we still show the button, but we set
        // the NotAllowed mouse cursor upon hover instead of the Hand cursor.
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if self.enabled && fe.is_primary_hit() => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_bg.area());
                }
                cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Pressed(fe.modifiers));
                    self.animator_play(cx, id!(hover.down));
                    self.set_key_focus(cx);
            }
            Hit::FingerHoverIn(_) => {
                if self.enabled {
                    cx.set_cursor(MouseCursor::Hand);
                    self.animator_play(cx, id!(hover.on));
                } else {
                    cx.set_cursor(MouseCursor::NotAllowed);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerLongPress(_lp) if self.enabled && self.enable_long_press => {
                cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::LongPressed);
            }
            Hit::FingerUp(fe) if self.enabled && fe.is_primary_hit() => {
                let was_clicked = fe.is_over && if self.enable_long_press { fe.was_tap() } else { true };
                if was_clicked {
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Clicked(fe.modifiers));
                    if self.reset_hover_on_click {
                        self.animator_cut(cx, id!(hover.off));
                    } else if fe.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        self.animator_play(cx, id!(hover.off));
                    }
                } else {
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Released(fe.modifiers));
                    self.animator_play(cx, id!(hover.off));
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
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx:&mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl Button {
        
    pub fn draw_button(&mut self, cx: &mut Cx2d, label:&str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
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
        matches!(
            actions.find_widget_action(self.widget_uid()).cast_ref(),
            ButtonAction::LongPressed,
        )
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
        if let ButtonAction::Clicked(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else {
            None
        }
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was pressed down.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let ButtonAction::Pressed(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else {
            None
        }
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was released,
    /// which is *not* considered to be clicked.
    ///
    /// See [`ButtonAction`] for more details.
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let ButtonAction::Released(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else {
            None
        }
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
    pub fn pressed_modifiers(&self, actions: &Actions) ->  Option<KeyModifiers> {
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
            inner.animator_cut(cx, id!(hover.off));
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
    
    pub fn which_clicked_modifiers(&self, actions: &Actions) -> Option<(usize,KeyModifiers)> {
        for (index,btn) in self.iter().enumerate(){
            if let Some(km) = btn.clicked_modifiers(actions){
                return Some((index, km))
            }
        }
        None
    }

    pub fn set_visible(&self, cx:&mut Cx, visible: bool) {
        for item in self.iter() {
            item.set_visible(cx, visible)
        }
    }
    pub fn set_enabled(&self, cx:&mut Cx, enabled: bool) {
        for item in self.iter() {
            item.set_enabled(cx, enabled)
        }
    }
}
