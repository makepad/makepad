use {
    crate::{
        makepad_platform::*,
        component_map::*,
        frame_component::*
    }
};

live_register!{
    HBox: {{HBox}} {
    }
}

#[derive(Live)]
#[live_register(register_as_frame_component!(HBox))]
pub struct HBox { // draw info per UI element
    #[rust] live_ptr: Option<LivePtr>,
    #[rust] components: ComponentMap<LiveId, FrameComponentRef>,
    #[rust] create_order: Vec<LiveId>
}

impl HBox {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        // ok lets start a horizontal turtle
        // we either have computed height or a fixed height
        // and a computed width or a fixed width
        // and we have alignment rules for our child components
        // essentially we have the layout cases per item
        // so we have a box. and then we draw things
        // what i think we need is:
        // horizontal alignment
        // vertical alignment
        // 'optionally item packing'
        // fill av space when you know itemsize
        
        
        for id in &self.create_order {
            if let Some(component) = self.components.get_mut(id).unwrap().as_mut() {
                component.draw_component(cx);
            }
        }
    }
}

frame_component_impl!(HBox);
frame_component_handle_event_impl!(HBox);
