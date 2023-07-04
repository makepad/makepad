use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
    DrawCheckBox = {{DrawCheckBox}} {
        uniform size: 7.0;
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            match self.check_type {
                CheckType::Check => {
                    let left = 3;
                    let sz = self.size;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.box(left, c.y - sz, sz * 2.0, sz * 2.0, 3.0); // rounding = 3rd value
                    sdf.fill_keep(mix(mix(#x00000077, #x00000044, pow(self.pos.y, 1.)), mix(#x000000AA, #x00000066, pow(self.pos.y, 1.0)), self.hover))
                    sdf.stroke(#x888, 1.0) // outline
                    let szs = sz * 0.5;
                    let dx = 1.0;
                    sdf.move_to(left + 4.0, c.y);
                    sdf.line_to(c.x, c.y + szs);
                    sdf.line_to(c.x + szs, c.y - szs);
                    sdf.stroke(mix(#fff0, #f, self.selected), 1.25);
                }
                CheckType::Radio => {
                    let sz = self.size;
                    let left = sz + 1.;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.circle(left, c.y, sz);
                    sdf.fill(#2);
                    let isz = sz * 0.5;
                    sdf.circle(left, c.y, isz);
                    sdf.fill(mix(#fff0, #f, self.selected));
                }
                CheckType::Toggle => {
                    let sz = self.size;
                    let left = sz + 1.;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);
                    sdf.fill(#2);
                    let isz = sz * 0.5;
                    sdf.circle(left + sz + self.selected * sz, c.y, isz);
                    sdf.circle(left + sz + self.selected * sz, c.y, 0.5 * isz);
                    sdf.subtract();
                    sdf.circle(left + sz + self.selected * sz, c.y, isz);
                    sdf.blend(self.selected)
                    sdf.fill(#f);
                }
                CheckType::None=>{
                    return #0000
                }
            }
            return sdf.result
        }
    }
    
    CheckBox = {{CheckBox}} {
        
        draw_label: {
            color: #9,
            instance focus: 0.0
            instance selected: 0.0
            instance hover: 0.0
            text_style: {
                font: {
                    //path: d"resources/IBMPlexSans-SemiBold.ttf"
                }
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #fff6,
                        #fff6,
                        self.hover
                    ),
                    #fff6,
                    self.selected
                )
            }
        }
        
        draw_icon:{
            instance focus: 0.0
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #f,
                    self.selected
                )
            }
        }
        
        walk: {
            width: Fit,
            height: Fit
        }
        label_walk: {
            margin: {left: 20.0, top: 8, bottom: 8, right: 10}
            width: Fit,
            height: Fit,
        }
        
        draw_check: {
        }
        
        label_align: {
            y: 0.0
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_check: {hover: 0.0}
                        draw_label: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {hover: 1.0}
                        draw_label: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 0.0}
                        draw_label: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 1.0}
                        draw_label: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 0.0},
                        draw_label: {selected: 0.0},
                        draw_icon: {selected: 0.0},
                    }
                 }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_check: {selected: 1.0}
                        draw_label: {selected: 1.0}
                        draw_icon: {selected: 1.0},
                    }
                }
            }
        }
    }
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
    None
}

#[derive(Live)]
pub struct CheckBox {
    
    #[live] walk: Walk,
    #[live] icon_walk: Walk,

    #[live] layout: Layout,
    #[state] state: LiveState,
    
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    
    #[live] draw_check: DrawCheckBox,
    #[live] draw_label: DrawText,
    #[live] draw_icon: DrawIcon,

    #[live] label: String,
    
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
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.draw_check.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                if self.state.is_in_state(cx, id!(selected.on)) {
                    self.animate_state(cx, id!(selected.off));
                    dispatch_action(cx, CheckBoxAction::Change(false));
                }
                else {
                    self.animate_state(cx, id!(selected.on));
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
        self.draw_label.draw_walk(cx, self.label_walk, self.label_align, &self.label);
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
                self.toggle_state(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
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
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CheckBoxRef(WidgetRef);

impl CheckBoxRef {
    pub fn set_label_text(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            inner.label.clear();
            inner.label.push_str(text);
        }
    }

    pub fn set_selected(&self, cx: &mut Cx, value:bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.toggle_state(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
        }
    }
}
