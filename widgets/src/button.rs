use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*,};
live_design! {
    ButtonBase = {{Button}} {}
}

/// Actions emitted by a button widget, including the key modifiers
/// that were active when the action occurred.
///
/// The sequence of actions emitted by a button is as follows:
/// 1. `ButtonAction::Pressed` when the button is pressed.
/// 2. Then, either one of the following, but not both:
///    * `ButtonAction::Clicked` when the mouse/finger is lifted up while over the button area.
///    * `ButtonAction::Released` when the mouse/finger is lifted up while *not* over the button area.
#[derive(Clone, Debug, DefaultNone)]
pub enum ButtonAction {
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
}

impl Widget for Button {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        
        match event.hit_designer(cx, self.draw_bg.area()){
            HitDesigner::DesignerPick(_e)=>{
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _=>()
        }
        
        if self.visible {
            // The button only handles hits when it's visible and enabled.
            // If it's not enabled, we still show the button, but we set
            // the NotAllowed mouse cursor upon hover instead of the Hand cursor.
            match event.hits(cx, self.draw_bg.area()) {
                Hit::FingerDown(fe) if self.enabled => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.draw_bg.area());
                    }
                    cx.widget_action(uid, &scope.path, ButtonAction::Pressed(fe.modifiers));
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
                Hit::FingerUp(fe) if self.enabled => {
                    if fe.is_over {
                        cx.widget_action(uid, &scope.path, ButtonAction::Clicked(fe.modifiers));
                        if self.reset_hover_on_click {
                            self.animator_cut(cx, id!(hover.off));
                        } else if fe.device.has_hovers() {
                            self.animator_play(cx, id!(hover.on));
                            self.animator_play(cx, id!(hover.on));
                        } else {
                            self.animator_play(cx, id!(hover.off));
                        }
                    } else {
                        cx.widget_action(uid, &scope.path, ButtonAction::Released(fe.modifiers));
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

    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
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
        self.borrow().map_or(false, |inner| inner.clicked(actions))
    }

    /// See [`Button::pressed()`].
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |inner| inner.pressed(actions))
    }

    /// See [`Button::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |inner| inner.released(actions))
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

    pub fn set_visible(&self, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.visible = visible;
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.enabled = enabled;
        }
    }

    /// Resets the hover state of this button. This is useful in certain cases the
    /// hover state should be reseted in a specific way that is not the default behavior
    /// which is based on the mouse cursor position and movement.
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

    pub fn set_visible(&self, visible: bool) {
        for item in self.iter() {
            item.set_visible(visible)
        }
    }
    pub fn set_enabled(&self, enabled: bool) {
        for item in self.iter() {
            item.set_enabled(enabled)
        }
    }
}
