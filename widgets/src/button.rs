use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};
live_design!{
    import makepad_draw::shader::std::*;
    
    Button= {{Button}} {
    }
}

#[derive(Clone, WidgetAction)]
pub enum ButtonAction {
    None,
    Clicked,
    Pressed,
    Released
}

#[derive(Live)]
pub struct Button {
    #[animator] animator: Animator,

    #[live] draw_bg: DrawQuad,
    #[live] draw_text: DrawText,
    #[live] draw_icon: DrawIcon,
    #[live] icon_walk: Walk,
    #[live] label_walk: Walk,
    #[walk] walk: Walk,
    
    #[layout] layout: Layout,

    #[live] pub text: RcStringMut,
}

impl LiveHook for Button{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, Button)
    }
}

impl Widget for Button{
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

    fn walk(&self)->Walk{
        self.walk
    }
    
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_bg.redraw(cx)
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

impl Button {
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)) {
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                dispatch_action(cx, ButtonAction::Pressed);
                self.animator_play(cx, id!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                 self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => if fe.is_over {
                dispatch_action(cx, ButtonAction::Clicked);
                if fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else{
                    self.animator_play(cx, id!(hover.off));
                }
            }
            else {
                dispatch_action(cx, ButtonAction::Released);
                self.animator_play(cx, id!(hover.off));
            }
            _ => ()
        };
    }
    /*
    pub fn draw_text(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }*/
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_text.draw_walk(cx, self.label_walk, Align::default(), self.text.as_ref());
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct ButtonRef(WidgetRef); 

impl ButtonRef {
    pub fn set_text(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }
    
    pub fn clicked(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let ButtonAction::Clicked = item.action() {
                return true
            }
        }
        false
    }

    pub fn pressed(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let ButtonAction::Pressed = item.action() {
                return true
            }
        }
        false
    }

}

#[derive(Clone, WidgetSet)]
pub struct ButtonSet(WidgetSet);
impl ButtonSet{
    pub fn clicked(&self, actions: &WidgetActions)->bool{
        for button in self.iter(){
            if button.clicked(actions){
                return true
            }
        }
        false
    }
    pub fn pressed(&self, actions: &WidgetActions)->bool{
        for button in self.iter(){
            if button.clicked(actions){
                return true
            }
        }
        false
    }
}

