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
        
            width:Fit
            height:Fit
        
        draw_text:{
            color:#8,
            wrap: Word
        }
    }
}

#[derive(Live)]
pub struct Label {
    #[live] draw_text: DrawText,
    #[walk] walk: Walk,
    #[live] align: Align,

    //margin: Margin,
    #[live] text: RcStringMut,
} 

impl LiveHook for Label{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,Label)
    }
}

impl Widget for Label {
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_text.redraw(cx)
    }
    
    fn walk(&self)->Walk{
        self.walk
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk:Walk)->WidgetDraw{
        self.draw_text.draw_walk(cx, walk, self.align, self.text.as_ref());
        WidgetDraw::done()
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, v:&str){
        self.text.as_mut_empty().push_str(v);
    }
}


#[derive(Clone, PartialEq, WidgetRef)]
pub struct LabelRef(WidgetRef); 

impl LabelRef{
  
}
