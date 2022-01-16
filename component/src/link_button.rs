#![allow(unused)]
use {
    crate::{
        makepad_render::*,
        button_logic::*,
        frame_registry::*,
        register_as_frame_component,
        button::Button
    }
};

live_register!{
    use makepad_render::shader::std::*;
    
    LinkButton: {{LinkButton}} {
        button: {
            bg_quad: {
                const THICKNESS: 0.8
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    let offset_y = 1.0
                    sdf.move_to(0., self.rect_size.y - offset_y);
                    sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                    return sdf.stroke(#f, mix(0.0, THICKNESS, self.hover));
                }
            }
            
            layout: {
                walk: {
                    width: Width::Computed,
                    height: Height::Computed,
                    margin: Margin {left: 5.0, top: 5.0, right: 5.0}
                }
                padding: {left: 1.0, top: 1.0, right: 1.0, bottom: 1.0}
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct LinkButton {
    button: Button
}

impl LinkButton {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        self.button.handle_event(cx, event)
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, label: Option<&str>) {
        self.button.draw(cx, label)
    }
}
