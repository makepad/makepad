#![allow(unused)]
use makepad_render::*;
use crate::button_logic::*;
use crate::frame_registry::*;
use crate::register_as_frame_component;

live_register!{
    use makepad_render::shader::std::*;

    LinkButton: {{LinkButton}} {
        bg_quad: {
            instance hover: 0.0
            instance pressed: 0.0
            
            const THICKNESS: 0.8
            
            fn pixel(self) -> vec4 {
                //return #f00
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);

                return sdf.stroke(#f, mix(0.0,THICKNESS,self.hover));
            }
        }
        
        layout: {
            align: {fx: 0.5, fy: 0.5},
            walk: {
                width: Width::Computed,
                height: Height::Computed,
                margin: {top: 5.0},
            }
            padding: {left: 1.0, top: 1.0, right: 1.0, bottom: 1.0}
        }
        
        default_state: {
            duration: 0.1,
            apply:{
                bg_quad: {pressed: 0.0, hover: 0.0}
                label_text: {color: #9}
            }
        }
        
        hover_state: {
            from: {
                all: Play::Forward {duration: 0.1}
                pressed_state: Play::Forward {duration: 0.01}
            }
            apply: {
                bg_quad: {
                    pressed: 0.0,
                    hover: [{time: 0.0, value: 1.0}],
                }
                label_text: {color: [{time: 0.0, value: #f}]}
            }
        }
        
        pressed_state: {
            duration: 0.2,
            apply: {
                bg_quad: {
                    pressed: [{time: 0.0, value: 1.0}],
                    hover: 1.0,
                }
                label_text: {color: [{time: 0.0, value: #c}]}
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct LinkButton {
    #[rust] pub button_logic: ButtonLogic,
    #[state(default_state)] pub animator: Animator,
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    bg_quad: DrawQuad,
    label_text: DrawText,
    layout: Layout,
    label: String
}

impl LinkButton {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        
        self.animator_handle_event(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg_quad.draw_vars.area);
        
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, self.pressed_state),
            ButtonState::Default => self.animate_to(cx, self.default_state),
            ButtonState::Hover => self.animate_to(cx, self.hover_state),
            _ => ()
        };
        res.action
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, label: Option<&str>) {
        self.bg_quad.begin(cx, self.layout);
        self.label_text.draw_walk(cx, label.unwrap_or(&self.label));
        self.bg_quad.end(cx);
    }
}
