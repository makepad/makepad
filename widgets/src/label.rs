#![allow(unused)]
use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw_2d::*,
        frame::*,
        widget::*
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    Label= {{Label}} {
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
#[live_design_fn(widget_factory!(Label))]
pub struct Label {
    label: DrawText,
    walk: Walk,
    
//    overflow: Overflow,
    align: Align,

    //margin: Margin,
    text: String,
}

impl Widget for Label {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}

    fn redraw(&mut self, cx:&mut Cx){
        self.label.redraw(cx)
    }
    
    fn get_walk(&self)->Walk{
        self.walk
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk:Walk)->WidgetDraw{
        let mut lines = self.text.split("\\n");
        for line in lines{
            // lets debugdraw the cliprect
            
            self.label.draw_walk(cx, walk, self.align, line);
        }
        WidgetDraw::done()
    }
}


#[derive(Clone, PartialEq, WidgetRef)]
pub struct LabelRef(WidgetRef); 

impl LabelRef{
    pub fn set_text(&self, text:&str){
        if let Some(mut inner) = self.inner_mut(){
            inner.text.clear();
            inner.text.push_str(text);
        }
    }
}
