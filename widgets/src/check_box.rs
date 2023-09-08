use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    DrawCheckBox = {{DrawCheckBox}} {}
    CheckBoxBase = {{CheckBox}} {}
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawCheckBox {
    #[deref] draw_super: DrawQuad,
    #[live] check_type: CheckType,
    #[live] hover: f32,
    #[live] focus: f32,
    #[live] selected: f32
}

#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum CheckType {
    #[pick] Check = shader_enum(1),
    Radio = shader_enum(2),
    Toggle = shader_enum(3),
    None = shader_enum(4),
}

#[derive(Live)]
pub struct CheckBox {
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[animator] animator: Animator,

    #[live] icon_walk: Walk,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    
    #[live] draw_check: DrawCheckBox,
    #[live] draw_text: DrawText,
    #[live] draw_icon: DrawIcon,

    #[live] text: RcStringMut,
    
    #[live] bind: String,
}

impl LiveHook for CheckBox{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,CheckBox)
    }
}

#[derive(Clone, WidgetAction)]
pub enum CheckBoxAction {
    Change(bool),
    None
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLabelText {
    #[deref] draw_super: DrawText,
    #[live] hover: f32,
    #[live] pressed: f32,
}

impl CheckBox {
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, CheckBoxAction)) {
        self.animator_handle_event(cx, event);
        
        match event.hits(cx, self.draw_check.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(selected.on)) {
                    self.animator_play(cx, id!(selected.off));
                    dispatch_action(cx, CheckBoxAction::Change(false));
                }
                else {
                    self.animator_play(cx, id!(selected.on));
                    dispatch_action(cx, CheckBoxAction::Change(true));
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
        self.draw_check.begin(cx, walk, self.layout);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_check.end(cx);
    }
}

impl Widget for CheckBox {

    fn widget_to_data(&self, _cx: &mut Cx, actions:&WidgetActions, nodes: &mut LiveNodeVec, path: &[LiveId])->bool{
        match actions.single_action(self.widget_uid()) {
            CheckBoxAction::Change(v) => {
                nodes.write_field_value(path, LiveValue::Bool(v));
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes:&[LiveNode], path: &[LiveId]){
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(value) = value.as_bool() {
                self.animator_toggle(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
            }
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_check.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self,v:&str){
        self.text.as_mut_empty().push_str(v);
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CheckBoxRef(WidgetRef);

impl CheckBoxRef {
    pub fn changed(&self, actions: &WidgetActions)->Option<bool>{
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let CheckBoxAction::Change(b) = item.action() {
                return Some(b)
            }
        }
        None
    }

    pub fn set_text(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }

    pub fn selected(&self, cx: &Cx)->bool {
        if let Some(inner) = self.borrow(){
            inner.animator_in_state(cx, id!(selected.on))
        }
        else{
            false
        }
    }

    pub fn set_selected(&self, cx: &mut Cx, value:bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.animator_toggle(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
        }
    }
}
