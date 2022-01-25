use crate::makepad_platform::*;

pub mod design_editor;
pub mod inline_widget;
pub mod live_editor;
pub mod inline_cache;

pub fn live_register(cx: &mut Cx){
    crate::design_editor::inline_widget::inline_color_picker::live_register(cx);
    crate::design_editor::inline_widget::inline_registry::live_register(cx);
    crate::design_editor::live_editor::live_register(cx);
}
