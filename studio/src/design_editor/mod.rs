use makepad_render::*;

pub mod design_editor;
pub mod live_widget;
pub mod live_editor;

pub fn live_register(cx: &mut Cx){
    crate::design_editor::live_widget::live_color_picker::live_register(cx);
    crate::design_editor::live_widget::registry::live_register(cx);
    crate::design_editor::live_editor::live_register(cx);
}
