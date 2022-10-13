pub use makepad_draw_2d::makepad_platform;
pub use makepad_draw_2d::makepad_image_formats;
pub use makepad_draw_2d;

pub mod button_logic;
pub mod button;
pub mod label;
pub mod desktop_button;
pub mod desktop_window;
pub mod scroll_shadow;
pub mod scroll_bar;
pub mod scroll_bars;
pub mod link_label;
pub mod list_box;

pub mod file_tree;
pub mod slides_view;
pub mod log_list;
pub mod log_icon;

pub mod drop_down;

pub mod dock;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod color_picker;
pub mod text_input;
pub mod slider;

pub mod check_box;

pub mod popup_menu;

#[macro_use]
pub mod window_menu;

pub use makepad_derive_widget;
pub use makepad_platform::*;
pub use makepad_derive_widget::*;

pub mod bare_window;
pub mod fold_button;

pub mod splitter;
pub mod fold_header;

pub mod debug_view;

pub mod imgui;
pub mod frame;

pub mod nav_control;

pub mod widget;

mod theme;

pub use crate::{
    bare_window::BareWindow,
    button_logic::{button_logic_handle_event, ButtonAction},
    button::{Button},
    text_input::{TextInput},
    link_label::{LinkLabel},
    desktop_window::{DesktopWindow},
    scroll_bars::{ScrollBars},
    scroll_shadow::{ScrollShadow},
    scroll_bar::{ScrollBar},
    frame::{
        Frame,
    },
    widget::{
        WidgetDraw,
        WidgetDrawApi,
        WidgetUid,
        CreateAt,
        WidgetActionItem,
        WidgetRef,
        Widget,
        WidgetRegistry,
        WidgetFactory,
        WidgetAction,
    }
};

pub fn live_register(cx: &mut Cx) {
    makepad_draw_2d::live_register(cx);
    crate::log_list::live_register(cx);
    crate::log_icon::live_register(cx);
    crate::debug_view::live_register(cx);
    crate::fold_header::live_register(cx);
    crate::splitter::live_register(cx);
    crate::theme::live_register(cx);
    crate::slider::live_register(cx);
    crate::label::live_register(cx);
    crate::nav_control::live_register(cx);
    crate::frame::live_register(cx);
    crate::fold_button::live_register(cx);
    crate::text_input::live_register(cx);
    crate::link_label::live_register(cx);
    crate::scroll_shadow::live_register(cx);
    crate::button::live_register(cx);
    crate::desktop_button::live_register(cx);
    crate::desktop_window::live_register(cx);
    crate::bare_window::live_register(cx);
    crate::window_menu::live_register(cx);
    crate::scroll_bar::live_register(cx);
    crate::scroll_bars::live_register(cx);
    crate::check_box::live_register(cx);
    crate::tab_close_button::live_register(cx);
    crate::tab::live_register(cx);
    crate::tab_bar::live_register(cx);
    crate::dock::live_register(cx);
    crate::color_picker::live_register(cx);
    crate::file_tree::live_register(cx);
    crate::slides_view::live_register(cx);
    crate::list_box::live_register(cx);
    crate::popup_menu::live_register(cx);
    crate::drop_down::live_register(cx);
}
