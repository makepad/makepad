pub mod log_list;
pub mod log_icon;
pub mod dock;
pub mod file_tree;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod color_picker;
pub mod splitter;

pub use makepad_component;
pub use makepad_component::makepad_platform;

use makepad_platform::*;

pub fn live_register(cx:&mut Cx){
    makepad_component::live_register(cx);
    crate::splitter::live_register(cx);
    crate::log_list::live_register(cx);
    crate::log_icon::live_register(cx);
    crate::file_tree::live_register(cx);
    crate::tab_close_button::live_register(cx);
    crate::tab::live_register(cx);
    crate::tab_bar::live_register(cx);
    crate::dock::live_register(cx);
    crate::color_picker::live_register(cx);
}
