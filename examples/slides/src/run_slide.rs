use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};
live_design!{
    RunSlide = {{RunSlide}} {}
}

#[derive(Clone, WidgetAction)]
pub enum RunSlideAction {
    None,
}

#[derive(Live)]
pub struct RunSlide {
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[deref] run_view: RunView
}

impl LiveHook for RunSlide{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, Button)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.file_system.init(cx);
        self.build_manager.init(cx);
    }
}

impl Widget for RunSlide{
   fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(),uid));
        });
    }

    fn walk(&mut self, _cx:&mut Cx)->Walk{
        self.walk
    }
    
    fn redraw(&mut self, cx:&mut Cx){
        self.area.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, v:&str){
        self.text.as_mut_empty().push_str(v);
    }
}

impl RunSlide {
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)) {
    }
    /*
    pub fn draw_text(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }*/
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
    }
}

#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct RunSlideRef(WidgetRef); 

impl RunSlideRef {
}

