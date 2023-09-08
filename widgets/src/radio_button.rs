use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    DrawRadioButton = {{DrawRadioButton}} {}
    RadioButtonBase = {{RadioButton}} {}
}

#[derive(Live, LiveHook)]
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

#[derive(Live)]
pub struct RadioButton {
    #[live] draw_radio: DrawRadioButton,
    #[live] draw_icon: DrawIcon,
    #[live] draw_text: DrawText,
    
    #[live] icon_walk: Walk,
    #[walk] walk: Walk,
    
    #[live] value: LiveValue,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] label: String,
    
    #[live] bind: String,
}

impl LiveHook for RadioButton{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,RadioButton)
    }
}

#[derive(Clone, WidgetAction)]
pub enum RadioButtonAction {
    Clicked,
    None
}


impl RadioButton {
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, RadioButtonAction)) {
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
                    dispatch_action(cx, RadioButtonAction::Clicked);
                }
            },
            Hit::FingerUp(_fe) => {
                
            }
            Hit::FingerMove(_fe) => {
                
            }
            _ => ()
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_radio.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, &self.label);
        self.draw_radio.end(cx);
    }
}

impl Widget for RadioButton {
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_radio.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct RadioButtonRef(WidgetRef);

impl RadioButtonRef{
    fn unselect(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.animator_play(cx, id!(selected.off));
        }
    }
}

#[derive(Clone, WidgetSet)]
pub struct RadioButtonSet(WidgetSet);

impl RadioButtonSet{
    
    pub fn selected(&self, cx: &mut Cx, actions: &WidgetActions)->Option<usize>{
        for action in actions{
            match action.action() {
                RadioButtonAction::Clicked => if let Some(index) = self.0.iter().position(|v| v.widget_uid() == action.widget_uid){
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
        None
    }
    
    pub fn selected_to_visible(&self, cx: &mut Cx, ui:&WidgetRef, actions: &WidgetActions, paths:&[&[LiveId]] ) {
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
