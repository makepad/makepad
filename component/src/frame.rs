use {
    crate::{
        makepad_platform::*,
        component_map::*,
        frame_component::*
    }
};

live_register!{
    Frame: {{Frame}} {
    }
}

#[derive(Live)]
pub struct Frame { // draw info per UI element
    #[rust] live_ptr: Option<LivePtr>,
    #[rust] components: ComponentMap<LiveId, FrameComponentRef>,
    #[rust] create_order: Vec<LiveId>
}

frame_component_impl!(Frame);
frame_component_handle_event_impl!(Frame);

impl Frame {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        for id in &self.create_order {
            if let Some(component) = self.components.get_mut(id).unwrap().as_mut() {
                component.draw_component(cx);
            }
        }
    }
}
