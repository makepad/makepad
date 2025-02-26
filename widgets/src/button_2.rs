use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*,};

live_design! {
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    
    pub Button2Base = {{Button2}} {}
    pub Button2 = <Button2Base> {
        // TODO: NEEDS FOCUS STATE
        
        width: Fit, height: Fit,
        spacing: 7.5,
        align: {x: 0.5, y: 0.5},
        padding: <THEME_MSPACE_2> {}
        label_walk: { width: Fit, height: Fit },
        label_align: { x: 0.5, y: 0.5 },
        
        draw_text: {
            instance hover: 0.0,
            instance pressed: 0.0,
            color: (THEME_COLOR_TEXT_DEFAULT)
            text_style: {
                font_family: (THEME_FONT_FAMILY_REGULAR),
                font_size: (THEME_FONT_SIZE_P)
            }
        }
        
        icon_walk: {
            width: (THEME_DATA_ICON_WIDTH), height: Fit,
        }
        
        draw_icon: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        mix(self.color, #f, 0.5),
                        self.hover
                    ),
                    self.color * 0.75,
                    self.pressed
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_CTRL_DEFAULT)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_CTRL_PRESSED, self.pressed);
                
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_SHADOW, self.pressed),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pressed),
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
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
    }
    
    pub Button2Icon = <Button2> {
        icon_walk: {
            width: 12.
            margin: { left: 0. }
        }
    }
    
    pub Button2Flat = <Button2Icon> {
        height: Fit, width: Fit,
        padding: <THEME_MSPACE_2> {}
        margin: 0.
        align: { x: 0.5, y: 0.5 }
        icon_walk: { width: 12. }
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.fill(#f00)
                return sdf.result
            }
        }
        
        draw_text: {
            instance hover: 0.0,
            instance pressed: 0.0,
            text_style: {
                font_family: (THEME_FONT_FAMILY_REGULAR),
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_U_HIDDEN)
            instance bodybottom: (THEME_COLOR_CTRL_HOVER)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_CTRL_PRESSED, self.pressed);
                
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_BEVEL_SHADOW, self.pressed),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_BEVEL_LIGHT, self.pressed),
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
    
    pub Button2Flatter = <Button2Icon> {
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
            instance hover: 0.0,
            instance pressed: 0.0,
            text_style: {
                font_family: (THEME_FONT_FAMILY_REGULAR),
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: (THEME_CORNER_RADIUS)
            instance bodytop: (THEME_COLOR_U_HIDDEN)
            instance bodybottom: (THEME_COLOR_U_HIDDEN)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 2.0;
                let body = mix(mix(self.bodytop, self.bodybottom, self.hover), THEME_COLOR_D_HIDDEN, self.pressed);
                
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(
                    body_transp,
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_D_HIDDEN, self.pressed),
                    max(0.0, grad_top - sdf.pos.y) / grad_top
                );
                let bot_gradient = mix(
                    mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_D_HIDDEN, self.pressed),
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
/// 1. `Button2Action::Pressed` when the button is pressed.
/// 2. Then, either one of the following, but not both:
///    * `Button2Action::Clicked` when the mouse/finger is lifted up while over the button area.
///    * `Button2Action::Released` when the mouse/finger is lifted up while *not* over the button area.
#[derive(Clone, Debug, DefaultNone)]
pub enum Button2Action {
    None,
    /// The button was pressed (a "down" event).
    Pressed(KeyModifiers),
    /// The button was clicked (an "up" event).
    Clicked(KeyModifiers),
    /// The button was released (an "up" event), but should not be considered clicked
    /// because the mouse/finger was not over the button area when released.
    Released(KeyModifiers),
}

/// A clickable button widget that emits actions when pressed, and when either released or clicked.
#[derive(Live, LiveHook, Widget)]
pub struct Button2 {
    #[animator]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText2,
    #[live]
    draw_icon: DrawIcon,
    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[live]
    label_align: Align,
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

impl Widget for Button2 {
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
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, Button2Action::Pressed(fe.modifiers));
                    self.animator_play(cx, id!(hover.pressed));
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
                Hit::FingerUp(fe) if self.enabled && fe.is_primary_hit() => {
                    if fe.is_over {
                        cx.widget_action_with_data(&self.action_data, uid, &scope.path, Button2Action::Clicked(fe.modifiers));
                        if self.reset_hover_on_click {
                            self.animator_cut(cx, id!(hover.off));
                        } else if fe.has_hovers() {
                            self.animator_play(cx, id!(hover.on));
                        } else {
                            self.animator_play(cx, id!(hover.off));
                        }
                    } else {
                        cx.widget_action_with_data(&self.action_data, uid, &scope.path, Button2Action::Released(fe.modifiers));
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
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
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

impl Button2 {
        
    pub fn draw_button(&mut self, cx: &mut Cx2d, label:&str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, label);
        self.draw_bg.end(cx);
    }
    
    /// Returns `true` if this button was clicked.
    ///
    /// See [`Button2Action`] for more details.
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.clicked_modifiers(actions).is_some()
    }

    /// Returns `true` if this button was pressed down.
    ///
    /// See [`Button2Action`] for more details.
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.pressed_modifiers(actions).is_some()
    }

    /// Returns `true` if this button was released, which is *not* considered to be clicked.
    ///
    /// See [`Button2Action`] for more details.
    pub fn released(&self, actions: &Actions) -> bool {
        self.released_modifiers(actions).is_some()
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was clicked.
    ///
    /// See [`Button2Action`] for more details.
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Button2Action::Clicked(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else {
            None
        }
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was pressed down.
    ///
    /// See [`Button2Action`] for more details.
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Button2Action::Pressed(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else {
            None
        }
    }

    /// Returns `Some` (with active keyboard modifiers) if this button was released,
    /// which is *not* considered to be clicked.
    ///
    /// See [`Button2Action`] for more details.
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        if let Button2Action::Released(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else {
            None
        }
    }
}

impl Button2Ref {
    /// See [`Button2::clicked()`].
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.clicked(actions))
    }

    /// See [`Button2::pressed()`].
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.pressed(actions))
    }

    /// See [`Button2::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.released(actions))
    }

    /// See [`Button2::clicked_modifiers()`].
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|inner| inner.clicked_modifiers(actions))
    }

    /// See [`Button2::pressed_modifiers()`].
    pub fn pressed_modifiers(&self, actions: &Actions) ->  Option<KeyModifiers> {
        self.borrow().and_then(|inner| inner.pressed_modifiers(actions))
    }

    /// See [`Button2::released_modifiers()`].
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

impl Button2Set {
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
