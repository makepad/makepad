use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    Label= {{Label}} {
        walk:{
            width:Fit
            height:Fit
        }
        draw_label:{
            color:#8
        }
    }
}

#[derive(Live, LiveHook)]
#[live_design_with(widget_factory!(cx,Label))]
pub struct Label {
    draw_label: DrawText,
    walk: Walk,
    
//  overflow: Overflow,
    align: Align,

    //margin: Margin,
    label: String,
}

impl Widget for Label {
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_label.redraw(cx)
    }
    
    fn get_walk(&self)->Walk{
        self.walk
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk:Walk)->WidgetDraw{
        let lines = self.label.split("\\n");
        for line in lines{
            // lets debugdraw the cliprect
            
            self.draw_label.draw_walk(cx, walk, self.align, line);
        }
        WidgetDraw::done()
    }
}


#[derive(Clone, PartialEq, WidgetRef)]
pub struct LabelRef(WidgetRef); 

impl LabelRef{
    pub fn set_text(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            inner.label.clear();
            inner.label.push_str(text);
        }
    }
}
