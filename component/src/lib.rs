#![feature(try_trait_v2)]
use makepad_platform::*;

pub mod button_logic;
pub mod button;
pub mod label;
pub mod desktop_button;
pub mod desktop_window;
pub mod scroll_shadow;
pub mod scroll_bar;
pub mod scroll_view;
pub mod component_map;
pub mod link_button;

pub mod dock;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod color_picker;
pub mod text_input;
pub mod slider;

#[macro_use]
pub mod frame_traits;
pub mod frame;
pub mod window_menu;

pub use makepad_platform;

pub mod bare_window;
pub mod fold_button;

pub mod splitter;
pub mod fold_header;

mod theme;

pub use crate::{
    bare_window::BareWindow,
    component_map::ComponentMap,
    button_logic::{button_logic_handle_event, ButtonAction},
    button::{Button},
    text_input::{TextInput},
    link_button::{LinkButton},
    desktop_window::{DesktopWindow},
    scroll_view::{ScrollView},
    scroll_shadow::{ScrollShadow},
    scroll_bar::{ScrollBar},
    frame::{Frame},
    frame_traits::{
        CreateAt,
        FramePath,
        FrameRef,
        FrameComponent,
        FrameComponentRegistry,
        FrameComponentFactory,
        FrameAction,
    }
};

pub fn live_register(cx: &mut Cx) {
    makepad_platform::live_cx::live_register(cx);
    crate::fold_header::live_register(cx);
    crate::splitter::live_register(cx);
    crate::theme::live_register(cx);
    crate::slider::live_register(cx);
    crate::label::live_register(cx);
    crate::frame::live_register(cx);
    crate::fold_button::live_register(cx);
    crate::text_input::live_register(cx);
    crate::link_button::live_register(cx);
    crate::scroll_shadow::live_register(cx);
    crate::button::live_register(cx);
    crate::desktop_button::live_register(cx);
    crate::desktop_window::live_register(cx);
    crate::bare_window::live_register(cx);
    crate::window_menu::live_register(cx);
    crate::scroll_view::live_register(cx);
    crate::scroll_bar::live_register(cx);
    crate::tab_close_button::live_register(cx);
    crate::tab::live_register(cx);
    crate::tab_bar::live_register(cx);
    crate::dock::live_register(cx);
    crate::color_picker::live_register(cx);
}
