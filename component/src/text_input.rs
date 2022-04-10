#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        button_logic::*,
        frame_component::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    use crate::theme::*;
    
    TextInput: {{TextInput}} {
        
        label_text:{
            instance hover: 0.0
            instance focus: 0.0
            text_style: FONT_CODE{}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #f,
                        self.hover
                    ),
                    #9,
                    self.focus
                )
            }
        }
        
        bg_quad: {
            instance hover: 0.0
            instance focus: 0.0
            
            const BORDER_RADIUS: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        
        walk: {
            width: Size::Fit,
            height: Size::Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            padding: {left: 0.0, top: 0.0, right: 4.0, bottom: 0.0}
        }
        
        state:{
            default =  {
                duration: 0.1,
                apply: {
                    bg_quad: {pressed: 0.0, hover: 0.0}
                    label_text: {pressed: 0.0, hover: 0.0}
                }
            }
            
            hover =  {
                from: {
                    all: Play::Forward {duration: 0.1}
                    pressed_state: Play::Forward {duration: 0.01}
                }
                apply: {
                    bg_quad: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    label_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                }
            }
                
            edit = {
                duration: 0.2,
                apply: {
                    bg_quad: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    label_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(TextInput))]
pub struct TextInput {
    state: State,
    
    bg_quad: DrawQuad,
    label_text: DrawText,
    
    walk: Walk,
    layout: Layout,
    
    pub value: String
}

impl FrameComponent for TextInput {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id: LiveId) -> FrameComponentActionRef {
        self.handle_event(cx, event).into()
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId> {
        self.draw_walk(cx, walk);
        Ok(())
    }
}

#[derive(Copy, Clone, PartialEq, FrameComponentAction)]
pub enum TextInputAction {
    None
}

impl TextInput {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> TextInputAction {
        TextInputAction::None
        /*
        self.animator_handle_event(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg_quad.draw_vars.area);
        
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, self.pressed_state),
            ButtonState::Default => self.animate_to(cx, self.default_state),
            ButtonState::Hover => self.animate_to(cx, self.hover_state),
            _ => ()
        };
        res.action*/
    }
    
    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.bg_quad.begin(cx, self.walk, self.layout);
        self.label_text.draw_walk(cx, label);
        self.bg_quad.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg_quad.begin(cx, walk, self.layout);
        self.label_text.draw_walk(cx, &self.value);
        self.bg_quad.end(cx);
    }
}
