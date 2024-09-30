use crate::{
    makepad_derive_widget::*,
    widget::*,
    makepad_draw::*,
    button::Button,
};

live_design!{
    LinkLabelBase = {{LinkLabel}} {}
}

/// A clickable label widget that opens a URL when clicked.
///
/// This is a wrapper around (and derefs to) a [`Button`] widget.
#[derive(Live, LiveHook, Widget)]
pub struct LinkLabel {
    #[deref] button: Button,
    #[live] pub url: String,
    #[live] pub open_in_place: bool,
}

impl Widget for LinkLabel {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) {
        let actions = cx.capture_actions(|cx|{
            self.button.handle_event(cx, event, scope);
        });
        if self.url.len()>0 && self.clicked(&actions){
            cx.open_url(&self.url, if self.open_in_place{OpenUrlInPlace::Yes}else{OpenUrlInPlace::No});
        }
        cx.extend_actions(actions);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.button.draw_walk(cx, scope, walk)
    }
    
    fn text(&self)->String{
        self.button.text()
    }
    
    fn set_text(&mut self, v:&str){
        self.button.set_text(v);
    }
}

impl LinkLabelRef {
    /// See [`Button::clicked()`].
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.clicked(actions))
    }

    /// See [`Button::pressed()`].
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.pressed(actions))
    }

    /// See [`Button::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.released(actions))
    }

    /// See [`Button::clicked_modifiers()`].
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.clicked_modifiers(actions))
    }

    /// See [`Button::pressed_modifiers()`].
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.pressed_modifiers(actions))
    }

    /// See [`Button::released_modifiers()`].
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.released_modifiers(actions))
    }
}
