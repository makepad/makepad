use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw_2d::*,
        widget::*,
        data_binding::DataBinding,
        frame::*,
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
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
            }
            return sdf.result
        }
    }
    
    CheckBox = {{CheckBox}} {
        label_text: {
            color: #9
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
        
        check_box: {
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
                        check_box: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        check_box: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        check_box: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        check_box: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {check_box: {selected: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {check_box: {selected: 1.0}}
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawCheckBox {
    draw_super: DrawQuad,
    check_type: CheckType,
    hover: f32,
    focus: f32,
    selected: f32
}

#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum CheckType {
    #[pick] Check = shader_enum(1),
    Radio = shader_enum(2),
    Toggle = shader_enum(3),
}

#[derive(Live, LiveHook)]
#[live_design_fn(widget_factory!(CheckBox))]
pub struct CheckBox {
    check_box: DrawCheckBox,
    
    walk: Walk,
    
    layout: Layout,
    state: State,
    
    label_walk: Walk,
    label_align: Align,
    label_text: DrawText,
    label: String,
    
    bind: String,
}

#[derive(Clone, WidgetAction)]
pub enum CheckBoxAction {
    Change(bool),
    None
}


impl CheckBox {
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, CheckBoxAction)) {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.check_box.area()) {
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
        self.check_box.begin(cx, walk, self.layout);
        self.label_text.draw_walk(cx, self.label_walk, self.label_align, &self.label);
        self.check_box.end(cx);
    }
}

impl Widget for CheckBox {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    
    fn bind_to(&mut self, cx: &mut Cx, db: &mut DataBinding, act: &WidgetActions, path: &[LiveId]) {
        match db {
            DataBinding::FromWidgets{nodes,..} => if let Some(item) = act.find_single_action(self.widget_uid()) {
                match item.action() {
                    CheckBoxAction::Change(v) => {
                        nodes.write_by_field_path(path, &[LiveNode::from_value(LiveValue::Bool(v))]);
                    }
                    _ => ()
                }
            }
            DataBinding::ToWidgets{nodes} => {
                if let Some(value) = nodes.read_by_field_path(path) {
                    if let Some(value) = value.as_bool() {
                        self.toggle_state(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
                    }
                }
            }
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.check_box.redraw(cx);
    }
    
    fn handle_widget_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_fn(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CheckBoxRef(WidgetRef);
