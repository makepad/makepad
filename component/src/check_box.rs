use {
    crate::{
        makepad_derive_frame::*,
        makepad_draw_2d::*,
        frame::*,
    }
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    DrawCheckBox: {{DrawCheckBox}} {
        instance hover: float
        instance focus: float
        instance drag: float
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            return sdf.result
        }
    }
    
    CheckBox: {{CheckBox}} {
        label_text: {
            color: #9
        }
        
        label_walk: {
            margin: {left: 4.0, top: 3.0}
            width: Fill,
            height: Fill
        }
        
        label_align: {
            y: 0.0
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        check_box: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        check_box: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        check_box: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        check_box: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {check_box: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Play::Snap}
                    apply: {check_box: {drag: 1.0}}
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawCheckBox {
    draw_super: DrawQuad,
}

#[derive(Live, LiveHook)]
#[live_register(frame_component!(CheckBox))]
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

#[derive(Clone, FrameAction)]
pub enum CheckBoxAction {
    None
}


impl CheckBox {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, &mut Self, CheckBoxAction)) {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.check_box.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                
            },
            Hit::FingerUp(_fe) => {

            }
            Hit::FingerMove(_fe) => {
                
            }
            _ => ()
        }
    }
    
    pub fn draw_walk(&mut self, _cx: &mut Cx2d, _walk: Walk) {
    }
}

impl FrameComponent for CheckBox {
    fn bind_read(&mut self, _cx: &mut Cx, _nodes: &[LiveNode]) {
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.check_box.redraw(cx);
    }
    
    fn handle_component_event(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)) {
        self.handle_event(cx, event, &mut | _cx, _checkbox, _action | {
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, _self_uid: FrameUid) -> FrameDraw {
        self.draw_walk(cx, walk);
        FrameDraw::done()
    }
}
