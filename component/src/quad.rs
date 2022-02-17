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
        color: #f00
        walk: {
            width: 40,
            height: 40,
            margin: 10.0,
        }
    }
}

#[derive(Live)]
#[live_register(register_as_frame_component!(Quad))]
pub struct Quad {
    bg_quad: DrawColor,
    walk: Walk,
    label: String
}

// allow color to be set on the root
impl LiveHook for Quad{
    fn apply_value_unknown(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match nodes[index].id{
            id!(color)=>self.bg_quad.color.apply(cx, from, index, nodes),
            id!(width)=>self.walk.width.apply(cx, from, index, nodes),
            id!(height)=>self.walk.height.apply(cx, from, index, nodes),
            id!(margin)=>self.walk.margin.apply(cx, from, index, nodes),
            _=> {
                cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                nodes.skip_node(index)
            }
        }
    }
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
        self.bg_quad.draw_walk(cx, self.walk);
    }
}
