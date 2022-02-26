#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        frame_component::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    Label: {{Label}} {
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(Label))]
pub struct Label {
    label_text: DrawText,
    margin: Margin,
    text: String,
}

impl FrameComponent for Label {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> FrameComponentActionRef {
        None
    }

    fn get_walk(&self)->Walk{
        Walk::empty()
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk:Walk)->Result<LiveId,()>{
        self.draw_walk(cx);
        Err(())
    }
}

impl Label {
    pub fn draw_walk(&mut self, cx: &mut Cx2d) { 
        self.label_text.draw_walk_with_margin(cx, self.margin, &self.text);
    }
}
