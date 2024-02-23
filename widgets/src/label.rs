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

#[derive(Live, LiveHook, Widget)]
pub struct Label {
    #[redraw] #[live] draw_text: DrawText,
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live] padding: Padding,
    //margin: Margin,
    #[live] text: RcStringMut,
} 

impl Widget for Label {

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        log!("Drawing label {}, align: {:?}, walk {:?}", self.text.as_ref(), self.align, self.walk);
        self.draw_text.draw_walk(cx, walk.with_add_padding(self.padding), self.align, self.text.as_ref());
        DrawStep::done()
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, v:&str){
        self.text.as_mut_empty().push_str(v);
    }
}
