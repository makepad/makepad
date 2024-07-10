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

#[derive(Clone, Debug, DefaultNone)]
pub enum LabelAction {
    HoverIn(Rect),
    HoverOut,
    None
}


#[derive(Live, LiveHook, Widget)]
pub struct Label {
    #[redraw] #[live] draw_text: DrawText,
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live] padding: Padding,
    #[rust] area: Area,
    //margin: Margin,
    #[live] text: RcStringMut,
} 

impl Widget for Label {

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        let walk = walk.with_add_padding(self.padding);
        cx.begin_turtle(walk, Layout::default());
        self.draw_text.draw_walk(cx, walk, self.align, self.text.as_ref());
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, v:&str){
        self.text.as_mut_empty().push_str(v);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fh) => {
                cx.widget_action(uid, &scope.path, LabelAction::HoverIn(fh.rect));
            }
            Hit::FingerHoverOut(_) => {
                cx.widget_action(uid, &scope.path, LabelAction::HoverOut);
            },
            _ => ()
        }
    }
}

impl LabelRef {
    pub fn hover_in(&self, actions:&Actions)->Option<Rect>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                LabelAction::HoverIn(rect) => Some(rect),
                _=> None
            }
        } else {
            None
        }
    }

    pub fn hover_out(&self, actions:&Actions)->bool{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                LabelAction::HoverOut => true,
                _=> false
            }
        } else {
            false
        }
    }
}
