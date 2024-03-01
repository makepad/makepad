use crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        View,
        Image,
    };

live_design!{
    //import crate::base::View;

    DrawRadioButton = {{DrawRadioButton}} {}
    RadioButtonBase = {{RadioButton}} {}
    RadioButtonGroupBase = {{RadioButtonGroup }} {}
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawRadioButton {
    #[deref] draw_super: DrawQuad,
    #[live] radio_type: RadioType,
    #[live] hover: f32,
    #[live] focus: f32,
    #[live] selected: f32
}


#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum RadioType {
    #[pick] Round = shader_enum(1),
    Tab = shader_enum(2),
}

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum MediaType {
    Image,
    #[pick] Icon,
    None,
}

#[derive(Live, LiveHook, Widget)]
pub struct RadioButtonGroup {
    #[deref] frame: View
}

#[derive(Live, LiveHook, Widget)]
pub struct RadioButton {
    #[redraw] #[live] draw_radio: DrawRadioButton,
    #[live] draw_icon: DrawIcon,
    #[live] draw_text: DrawText,

    #[live] value: LiveValue,

    #[live] media: MediaType,
    
    #[live] icon_walk: Walk,
    #[walk] walk: Walk,

    #[live] image: Image,

    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] label: String,
    
    #[live] bind: String,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum RadioButtonAction {
    Clicked,
    None
}


impl RadioButtonGroup {
    pub fn draw_walk(&mut self, _cx: &mut Cx2d, _walk: Walk) {}
}

impl RadioButton {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_radio.begin(cx, walk, self.layout);
        match self.media {
            MediaType::Image => {
                let image_walk = self.image.walk(cx);
                let _ = self.image.draw_walk(cx, image_walk);
            }
            MediaType::Icon => {
                self.draw_icon.draw_walk(cx, self.icon_walk);
            }
            MediaType::None => {}
        }
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, &self.label);
        self.draw_radio.end(cx);
    }

}

impl Widget for RadioButtonGroup {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        //let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
              
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl Widget for RadioButton {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
                
        match event.hits(cx, self.draw_radio.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(selected.off)) {
                    self.animator_play(cx, id!(selected.on));
                    cx.widget_action(uid, &scope.path, RadioButtonAction::Clicked);
                }
            },
            Hit::FingerUp(_fe) => {
                                
            }
            Hit::FingerMove(_fe) => {
                                
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl RadioButtonRef{
    fn unselect(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.animator_play(cx, id!(selected.off));
        }
    }
}

impl RadioButtonSet{
    
    pub fn selected(&self, cx: &mut Cx, actions: &Actions)->Option<usize>{
        for action in actions{
            if let Some(action) = action.as_widget_action(){
                match action.cast(){
                    RadioButtonAction::Clicked => if let Some(index) = self.0.iter().position(|v| action.widget_uid == v.widget_uid()){
                        for (i, item) in self.0.iter().enumerate(){
                            if i != index{
                                RadioButtonRef(item).unselect(cx);
                            }
                        }
                        return Some(index);
                    }
                    _ => ()
                }
            }
        }
        None
    }
    
    pub fn selected_to_visible(&self, cx: &mut Cx, ui:&WidgetRef, actions: &Actions, paths:&[&[LiveId]] ) {
        // find a widget action that is in our radiogroup
        if let Some(index) = self.selected(cx, actions){
            // ok now we set visible
            for (i,path) in paths.iter().enumerate(){
                let widget = ui.widget(path);
                widget.apply_over(cx, live!{visible:(i == index)});
                widget.redraw(cx);
            }
        }
    }
}
