use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};
live_design!{
    ButtonBase = {{Button}} {}
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

    #[live(true)] grab_key_focus: bool,

    #[live] pub text: RcStringMut,
}

impl LiveHook for Button{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, Button)
    }
}

impl Widget for Button{
   fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut WidgetScope)->WidgetActions {
       let mut actions = WidgetActions::new();
       let uid = self.widget_uid();
       self.animator_handle_event(cx, event);
       match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.grab_key_focus{
                    cx.set_key_focus(self.draw_bg.area());
                }
                actions.push_single(uid, &scope.path, ButtonAction::Pressed);
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
                actions.push_single(uid, &scope.path, ButtonAction::Clicked);
                if fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else{
                    self.animator_play(cx, id!(hover.off));
                }
            }
            else {
                actions.push_single(uid, &scope.path, ButtonAction::Released);
                self.animator_play(cx, id!(hover.off));
            }
            _ => ()
        }
        actions
    }

    fn walk(&mut self, _cx:&mut Cx)->Walk{
        self.walk
    }
    
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_bg.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _scope: &mut WidgetScope, walk: Walk) -> WidgetDraw {
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
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_text.draw_walk(cx, self.label_walk, Align::default(), self.text.as_ref());
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
    }
}

#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct ButtonRef(WidgetRef); 

impl ButtonRef {
    
    pub fn clicked(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let ButtonAction::Clicked = item.cast() {
                return true
            }
        }
        false
    }

    pub fn pressed(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let ButtonAction::Pressed = item.cast() {
                return true
            }
        }
        false
    }

}

#[derive(Clone, Debug, WidgetSet)]
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
            if button.pressed(actions){
                return true
            }
        }
        false
    }
}

