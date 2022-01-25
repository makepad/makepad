use crate::{
    makepad_platform::*,
};

live_register!{
    EmptyTemplate: {{EmptyTemplate}} {
    }
}

#[derive(Live, LiveHook)]
pub struct EmptyTemplate {
}

pub enum EmptyTemplateAction {
}

impl EmptyTemplate {
    
    pub fn draw(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_event(
        &mut self,
        _cx: &mut Cx,
        _event: &mut Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, EmptyTemplateAction),
    ) {
    }
}