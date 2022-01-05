use makepad_render::*;

mod button_logic;
mod button;
mod desktop_button;
mod desktop_window;
mod scroll_shadow;
mod scroll_bar;
mod scroll_view;
mod component_map;

#[macro_use]
mod frame_registry;
mod frame;
mod window_menu;

pub use makepad_render;

pub mod frame_template;
pub mod empty_template;
pub mod fold_list;
pub mod color_picker;
pub mod dock;
pub mod file_tree;
pub mod splitter;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod bare_window;
pub mod fold_button;

pub use crate::{
    component_map::ComponentMap,
    button_logic::{ButtonLogic, ButtonAction},
    button::{Button},
    desktop_window::{DesktopWindow}, 
    scroll_view::{ScrollView},
    scroll_shadow::{ScrollShadow},
    frame::{Frame},
    frame_registry::{FrameActions, CxFrameComponentRegistry, FrameComponent, FrameComponentRegistry}
};

pub fn live_register(cx:&mut Cx){
    crate::frame::live_register(cx);
    crate::fold_button::live_register(cx);
    crate::color_picker::live_register(cx);
    crate::scroll_shadow::live_register(cx);
    crate::button::live_register(cx);
    crate::desktop_button::live_register(cx);
    crate::desktop_window::live_register(cx);
    crate::bare_window::live_register(cx);
    crate::window_menu::live_register(cx);
    crate::scroll_view::live_register(cx);
    crate::scroll_bar::live_register(cx);
    crate::file_tree::live_register(cx);
    crate::splitter::live_register(cx);
    crate::tab_close_button::live_register(cx);
    crate::tab::live_register(cx);
    crate::tab_bar::live_register(cx);
    crate::dock::live_register(cx);
    crate::frame_registry::live_register(cx);
}
