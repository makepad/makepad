use makepad_platform::*;

pub mod button_logic;
pub mod button;
pub mod desktop_button;
pub mod desktop_window;
pub mod scroll_shadow;
pub mod scroll_bar;
pub mod scroll_view;
pub mod component_map;
pub mod link_button;

#[macro_use]
pub mod frame_component;
pub mod frame;
pub mod window_menu;

pub use makepad_platform;

pub mod frame_template;
pub mod empty_template;
pub mod bare_window;
pub mod fold_button;
mod theme;

pub use crate::{
    component_map::ComponentMap,
    button_logic::{ButtonLogic, ButtonAction},
    button::{Button},
    link_button::{LinkButton},
    desktop_window::{DesktopWindow}, 
    scroll_view::{ScrollView},
    scroll_shadow::{ScrollShadow},
    frame::{Frame},
    frame_component::{FrameActions, FrameComponent, FrameComponentRegistry}
};

pub fn live_register(cx:&mut Cx){
    makepad_platform::live_cx::live_register(cx);
    crate::theme::live_register(cx);
    crate::frame::live_register(cx);
    crate::fold_button::live_register(cx);
    crate::link_button::live_register(cx);
    crate::scroll_shadow::live_register(cx);
    crate::button::live_register(cx);
    crate::desktop_button::live_register(cx);
    crate::desktop_window::live_register(cx);
    crate::bare_window::live_register(cx);
    crate::window_menu::live_register(cx);
    crate::scroll_view::live_register(cx);
    crate::scroll_bar::live_register(cx);
}
