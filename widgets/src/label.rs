use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};

live_design!{
    LabelBase = {{Label}} {}
}

#[derive(Live, LiveHook, LiveRegisterWidget)]
pub struct Label {
    #[live] draw_text: DrawText,
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live] padding: Padding,
    //margin: Margin,
    #[live] text: RcStringMut,
} 

impl Widget for Label {
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_text.redraw(cx)
    }
    
    fn walk(&mut self, _cx:&mut Cx)->Walk{
        self.walk
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut WidgetScope, walk:Walk)->WidgetDraw{
        self.draw_text.draw_walk(cx, walk.with_add_padding(self.padding), self.align, self.text.as_ref());
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
