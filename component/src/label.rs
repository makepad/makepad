#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        frame_component::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    Label: {{Label}} {
        walk:{
            width:Fit
            height:Fit
        }
        label_text:{
            color:#8
        }
    }
}

#[derive(Live, LiveHook)]
#[live_register(frame_component!(Label))]
pub struct Label {
    label_text: DrawText,
    walk: Walk,
    
//    overflow: Overflow,
    align: Align,

    //margin: Margin,
    text: String,
}

impl FrameComponent for Label {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id:LiveId) -> FrameComponentActionRef {
        None
    }

    fn get_walk(&self)->Walk{
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk:Walk)->Result<(),LiveId>{
        let mut lines = self.text.split("\\n");
        for line in lines{
            self.label_text.draw_walk(cx, walk, self.align, line);
        }
        Ok(())
    }
}
