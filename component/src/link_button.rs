#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        button_logic::*,
        frame_registry::*,
        register_as_frame_component,
        button::Button
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    use crate::theme::*;
    
    LinkButton: {{LinkButton}} {
        button: {
            bg_quad: {
                const THICKNESS: 0.8
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    let offset_y = 1.0
                    sdf.move_to(0., self.rect_size.y - offset_y);
                    sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                    return sdf.stroke(mix(
                        COLOR_TEXT_DEFAULT,
                        COLOR_TEXT_META,
                        self.pressed
                    ), mix(0.0, THICKNESS, self.hover));
                }
            }
            label_text: {
                text_style:FONT_META{}
                fn get_color(self) -> vec4 {
                    return mix(
                        mix(
                            COLOR_TEXT_META,
                            COLOR_TEXT_DEFAULT,
                            self.hover
                        ),
                        COLOR_TEXT_META,
                        self.pressed
                    )
                }
            }
            
            layout: {
                walk: {
                    width: Width::Computed,
                    height: Height::Computed,
                    margin: Margin {left: 5.0, top: 4.0, right: 5.0}
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
