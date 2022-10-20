#![allow(unused)]
use {
    crate::{
        makepad_draw_2d::*,
        widget::*,
        button::{Button, ButtonAction}
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import crate::theme::*;
    
    LinkLabel = {{LinkLabel}} {
        button: {
            bg: {
                const THICKNESS = 0.8
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
            label: {
                text_style: <FONT_META> {}
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
            
            walk: {
                width: Fit,
                height: Fit,
                margin: {left: 5.0, top: 0.0, right: 0.0}
            }
            
            layout: {
                padding: {left: 1.0, top: 1.0, right: 1.0, bottom: 1.0}
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct LinkLabel {
    button: Button
}

impl LinkLabel {
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction),) {
        self.button.handle_event_fn(cx, event, dispatch_action)
    }
    
    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.button.draw_label(cx, &label)
    }
}
