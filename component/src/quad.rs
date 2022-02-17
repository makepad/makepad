#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        frame_component::*,
        register_as_frame_component
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    Quad: {{Quad}} {
        bg: {
            color: #f00
        }
        walk: {
            width: Width::Fixed(40),
            height: Height::Fixed(40),
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(Quad))]
pub struct Quad {
    bg: DrawColor,
    walk: Walk,
    label: String
}

impl FrameComponent for Quad {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> OptionFrameComponentAction {
        None
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d) {
        self.draw(cx, None);
    }
}

impl Quad {
    
    pub fn draw(&mut self, cx: &mut Cx2d, label: Option<&str>) {
        self.bg.draw_walk(cx, self.walk);
    }
}
