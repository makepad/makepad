#![allow(unused)]
use {
    crate::{
        makepad_derive_frame::*,
        makepad_platform::*,
        button_logic::*,
        frame_traits::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    use crate::theme::*;
    
    TextInput: {{TextInput}} {
        
        cursor: {
        }
        
        label: {
            instance hover: 0.0
            instance focus: 0.0
            instance selected: 1.0
            
            text_style: FONT_LABEL {}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #b,
                        self.hover
                    ),
                    mix(
                        #9,
                        #f,
                        self.selected
                    ),
                    self.focus
                )
            }
        }
        
        select: {
            instance hover: 0.0
            instance focus: 0.0
            
            const BORDER_RADIUS: 2.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    BORDER_RADIUS
                )
                sdf.fill(#0000)
                return sdf.result
            }
        }
        
        bg:{
            shape:Box
            color: #5
            radius:2
        },
        walk: {
            width: Fit,
            height: Fill,
            margin: {left: 1.0, right: 5.0, top: 0.0, bottom: 2.0},
        }
        label_walk: {
            width: Fit,
            height: Fill,
            margin: {left: 5.0, right: 5.0, top: 2.0, bottom: 2.0},
        }
        align: {
            y: 0.5
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        select: {hover: 0.0}
                        label: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Text,
                    from: {all: Play::Snap}
                    apply: {
                        select: {hover: 1.0}
                        label: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        select: {focus: 0.0}
                        label: {focus: 0.0}
                    }
                }
                on = {
                    cursor: Text,
                    from: {all: Play::Snap}
                    apply: {
                        select: {focus: 1.0}
                        label: {focus: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(TextInput))]
pub struct TextInput {
    state: State,
    
    bg: DrawShape,
    select: DrawQuad,
    cursor: DrawColor,
    label: DrawText,
    
    walk: Walk,
    align: Align,
    layout: Layout,
    
    label_walk: Walk,
    
    pub text: String,
    
    #[rust] cursor_pos: usize
}
impl LiveHook for TextInput {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        //nodes.debug_print(index,100);
        
        None
    }
}

#[derive(Copy, Clone, PartialEq, FrameAction)]
pub enum TextInputAction {
    None
}

impl TextInput {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, TextInputAction)) {
        self.state_handle_event(cx, event);
        match event.hits(cx, self.bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animate_state(cx, ids!(focus.off));
            }
            Hit::KeyFocus(_) => {
                self.animate_state(cx, ids!(focus.on));
            }
            Hit::KeyDown(ke) if ke.arrow_left()=>{
                 self.cursor_pos -= 1;
            }
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            },
            Hit::FingerDown(fe) => {
                cx.set_key_focus(self.bg.area());
                // ok so we need to calculate where we put the cursor down.
                //elf.
                if let Some(pos) = self.label.closest_offset(cx, fe.abs){
                    self.cursor_pos = pos;
                    self.label.redraw(cx);
                }
            },
            Hit::FingerUp(fe) => {
                self.animate_state(cx, ids!(drag.off));
                if fe.is_over && fe.input_type.has_hovers() {
                    self.animate_state(cx, ids!(hover.on));
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
            }
            Hit::FingerMove(fe) => {
            }
            _ => ()
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg.begin(cx, walk, self.layout);
        
        self.label.draw_walk(cx, self.label_walk, self.align, &self.text);
        
        // ok now we can query the geometry of the text we output
        if let Some(rect) = self.label.character_rect(cx, self.cursor_pos){
            // draw the cursor
            
        }
        
        self.bg.end(cx);
        // ok next problem.
        // how will we get the text geom
        //self.bg_quad.end(cx);
    }
}
