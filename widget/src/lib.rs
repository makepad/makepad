use makepad_render::*;
mod button_logic;
mod button;
mod desktop_button;
mod desktop_window;
mod scroll_shadow;
mod scroll_bar;
mod scroll_view;
mod frame;
mod window_menu;

pub mod dock;
pub mod file_tree;
pub mod splitter;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod genid;
pub mod bare_window;

pub use crate::{
    genid::{GenId, GenIdMap, GenIdAllocator},
    button_logic::{ButtonLogic, ButtonAction},
    button::{Button},
    desktop_window::{DesktopWindow}, 
    scroll_view::{ScrollView},
    scroll_shadow::{ScrollShadow},
    frame::{Frame, FrameActions}
};

pub fn live_register(cx:&mut Cx){
    crate::scroll_shadow::live_register(cx);
    crate::button::live_register(cx);
    crate::desktop_button::live_register(cx);
    crate::desktop_window::live_register(cx);
    crate::bare_window::live_register(cx);
    crate::window_menu::live_register(cx);
    crate::frame::live_register(cx);
    crate::scroll_view::live_register(cx);
    crate::scroll_bar::live_register(cx);
    crate::file_tree::live_register(cx);
    crate::splitter::live_register(cx);
    crate::tab_close_button::live_register(cx);
    crate::tab::live_register(cx);
    crate::tab_bar::live_register(cx);
    crate::dock::live_register(cx);
}
