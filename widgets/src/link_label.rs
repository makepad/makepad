use crate::{
    makepad_derive_widget::*,
    widget::*,
    makepad_draw::*,
    button::Button,
};

live_design!{
    LinkLabelBase = {{LinkLabel}} {}
}

#[derive(Live, LiveHook, Widget)]
pub struct LinkLabel {
    #[deref] button: Button
}

impl Widget for LinkLabel {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) {
        self.button.handle_event(cx, event, scope)
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

impl LinkLabel {
    pub fn clicked(&self, actions:&Actions) -> bool {
        self.button.clicked(actions)
    }

    pub fn pressed(&self, actions:&Actions) -> bool {
        self.button.pressed(actions)
    }

    pub fn released(&self, actions:&Actions) -> bool {
        self.button.released(actions)
    }
}

impl LinkLabelRef {
    pub fn clicked(&self, actions:&Actions) -> bool {
        if let Some(inner) = self.borrow(){ 
            inner.clicked(actions)
        } else {
            false
        }
    }
    
    pub fn pressed(&self, actions:&Actions) -> bool {
        if let Some(inner) = self.borrow(){ 
            inner.pressed(actions)
        } else {
            false
        }
    }

    pub fn released(&self, actions:&Actions) -> bool {
        if let Some(inner) = self.borrow(){ 
            inner.released(actions)
        } else {
            false
        }
    }
}
