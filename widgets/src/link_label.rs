use {
    crate::{
        makepad_derive_widget::*,
        widget::*,
        makepad_draw::*,
        button::{Button}
    }
};

live_design!{
    LinkLabelBase = {{LinkLabel}} {}
}

#[derive(Live)]
pub struct LinkLabel {
    #[deref] button: Button
}

impl LiveHook for LinkLabel {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, LinkLabel)
    }
}

impl Widget for LinkLabel {
       fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        self.button.handle_widget_event_with(cx,event,dispatch_action);
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.button.redraw(cx)
    }
    
    fn walk(&self) -> Walk {
        self.button.walk()
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.button.draw_walk_widget(cx, walk)
    }
    
    fn text(&self)->String{
        self.button.text()
    }
    
    fn set_text(&mut self, v:&str){
        self.button.set_text(v);
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct LinkLabelRef(WidgetRef);

impl LinkLabelRef {
}
