#![allow(unused)]
use {
    crate::{
        makepad_draw_2d::*,
        frame::*
    }
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    import makepad_component::theme::*;
    Label: {{Label}} {
        walk:{
            width:Fit
            height:Fit
        }
        label:{
            color:#8
        }
    }
}

#[derive(Live, LiveHook)]
#[live_register(frame_component!(Label))]
pub struct Label {
    label: DrawText,
    walk: Walk,
    
//    overflow: Overflow,
    align: Align,

    //margin: Margin,
    text: String,
}

impl FrameComponent for Label {
    fn get_walk(&self)->Walk{
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk:Walk, self_uid:FrameUid)->FrameDraw{
        let mut lines = self.text.split("\\n");
        for line in lines{
            self.label.draw_walk(cx, walk, self.align, line);
        }
        FrameDraw::done()
    }
}
