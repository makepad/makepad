use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*,};

live_design! {
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    
    pub ButtonBase = {{Button}} {}
    pub Button = <ButtonBase> {
        // TODO: NEEDS FOCUS STATE
        
        width: Fit, height: Fit,
        spacing: 7.5,
        align: {x: 0.5, y: 0.5},
        padding: <THEME_MSPACE_2> {}
        label_walk: { width: Fit, height: Fit },
        
        draw_text: {
            instance hover: 0.0,
            instance down: 0.0,

            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_down: (THEME_COLOR_TEXT_PRESSED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down)
            }
        }
        
        icon_walk: {
            width: (THEME_DATA_ICON_WIDTH), height: Fit,
        }
        
        draw_icon: {
            instance hover: 0.0
            instance down: 0.0

            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_down: (THEME_COLOR_TEXT_PRESSED)

            fn get_color(self) -> vec4 {
                return mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down)
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance down: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color: (THEME_COLOR_CTRL_DEFAULT)
            uniform color_hover: (THEME_COLOR_CTRL_HOVER)
            uniform color_down: (THEME_COLOR_CTRL_PRESSED)

            uniform outline_color_top: (THEME_COLOR_BEVEL_LIGHT)
            uniform outline_color_top_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform outline_color_top_down: (THEME_COLOR_BEVEL_SHADOW)

            uniform outline_color_bottom: (THEME_COLOR_BEVEL_SHADOW)
            uniform outline_color_bottom_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform outline_color_bottom_down: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down);
                
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(mix(self.outline_color_top, self.outline_color_top_hover, self.hover), self.outline_color_top_down, self.down),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(mix(self.outline_color_bottom, self.outline_color_bottom_hover, self.hover), self.outline_color_bottom_down, self.down),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    THEME_BEVELING
                )
                
                return sdf.result
            }
        }
        
        animator: {
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
        }
                
        animator: {
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
        }
    }
    
    pub ButtonIcon = <Button> {
        icon_walk: {
            width: 12.
            margin: { left: 0. }
        }
    }
    
    pub ButtonFlat = <ButtonIcon> {
        height: Fit, width: Fit,
        padding: <THEME_MSPACE_2> {}
        margin: 0.
        align: { x: 0.5, y: 0.5 }
        icon_walk: { width: 12. }
        
        draw_text: {
            color: (THEME_COLOR_TEXT_DEFAULT)
            color_hover: (THEME_COLOR_TEXT_HOVER)
            color_down: (THEME_COLOR_TEXT_PRESSED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down)
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance down: 0.0

            border_radius: (THEME_CORNER_RADIUS)

            color: (THEME_COLOR_U_HIDDEN)
            color_hover: (THEME_COLOR_CTRL_HOVER)
            color_down: (THEME_COLOR_CTRL_PRESSED)

            outline_color_top: (THEME_COLOR_U_HIDDEN)
            outline_color_top_hover: (THEME_COLOR_BEVEL_LIGHT)
            outline_color_top_down: (THEME_COLOR_BEVEL_SHADOW)

            outline_color_bottom: (THEME_COLOR_U_HIDDEN)
            outline_color_bottom_hover: (THEME_COLOR_BEVEL_SHADOW)
            outline_color_bottom_down: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down);
                
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(mix(self.outline_color_top, self.outline_color_top_hover, self.hover), self.outline_color_top_down, self.down),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(mix(self.outline_color_bottom, self.outline_color_bottom_hover, self.hover), self.outline_color_bottom_down, self.down),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    THEME_BEVELING
                )
                
                return sdf.result
            }
        }
        
    }
    
    pub ButtonFlatter = <ButtonIcon> {
        height: Fit, width: Fit,
        padding: <THEME_MSPACE_2> {},
        margin: <THEME_MSPACE_2> {},
        align: { x: 0.5, y: 0.5 },
        icon_walk: { width: 12. },

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.fill(#f00)
                return sdf.result
            }
        }
        
        draw_text: {
            color: (THEME_COLOR_TEXT_DEFAULT)
            color_hover: (THEME_COLOR_TEXT_HOVER)
            color_down: (THEME_COLOR_TEXT_PRESSED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down)
            }
        }
        
        draw_bg: {
            border_radius: (THEME_CORNER_RADIUS)
            
            color: (THEME_COLOR_U_HIDDEN)
            color_hover: (THEME_COLOR_U_HIDDEN)
            color_down: (THEME_COLOR_U_HIDDEN)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.color, self.color_hover, self.hover), self.color_down, self.down);
                
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_D_HIDDEN, self.down),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_D_HIDDEN, self.down),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    THEME_BEVELING
                )
                
                return sdf.result
            }
        }
        
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
    visible: bool,

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
        
        if self.visible {
            // The button only handles hits when it's visible and enabled.
            // If it's not enabled, we still show the button, but we set
            // the NotAllowed mouse cursor upon hover instead of the Hand cursor.
            match event.hits(cx, self.draw_bg.area()) {
                Hit::FingerDown(fe) if self.enabled && fe.is_primary_hit() => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.draw_bg.area());
                    }
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::Pressed(fe.modifiers));
                    self.animator_play(cx, id!(hover.down));
                }
                Hit::FingerHoverIn(_) => {
                    if self.enabled {
                        cx.set_cursor(MouseCursor::Hand);
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        cx.set_cursor(MouseCursor::NotAllowed);
                    }
                }
                Hit::FingerHoverOut(_) if self.enabled => {
                    self.animator_play(cx, id!(hover.off));
                }
                Hit::FingerLongPress(_lp) if self.enabled => {
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, ButtonAction::LongPressed);
                }
                Hit::FingerUp(fe) if self.enabled && fe.is_primary_hit() => {
                    if fe.is_over && fe.was_tap() {
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
