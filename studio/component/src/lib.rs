pub mod log_list;
pub mod log_icon;
pub mod file_tree;
pub mod shader_view;
pub use makepad_component;
pub use makepad_component::makepad_platform;

use makepad_platform::*;

pub fn live_register(cx:&mut Cx){
    makepad_component::live_register(cx);
    crate::log_list::live_register(cx);
    crate::log_icon::live_register(cx);
    crate::shader_view::live_register(cx);
    crate::file_tree::live_register(cx);
}
