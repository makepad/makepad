pub use makepad_draw_2d::makepad_platform;
pub use makepad_draw_2d::makepad_image_formats;
pub use makepad_draw_2d;

pub use makepad_derive_widget;
pub use makepad_platform::*;
pub use makepad_derive_widget::*;

pub mod button;
pub mod label;
pub mod link_label;
pub mod drop_down;
pub mod popup_menu;
pub mod check_box;
pub mod radio_button;
pub mod text_input;
pub mod slider;
pub mod scroll_bar;
pub mod scroll_bars;
pub mod splitter;
pub mod fold_header;
pub mod fold_button;
pub mod dock;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod bare_window;

pub mod desktop_button;
pub mod desktop_window;
pub mod scroll_shadow;

pub mod list_box;
pub mod file_tree;
pub mod slides_view;
pub mod log_list;
pub mod log_icon;
pub mod color_picker;

#[macro_use]
pub mod window_menu;
pub mod debug_view;
pub mod nav_control;

pub mod frame;
pub mod widget;

#[macro_use]
pub mod data_binding;

mod theme;

pub use crate::{
    data_binding::{DataBinding},
    bare_window::BareWindow,
    button::*,
    frame::*,
    label::*,
    slider::*,
    check_box::*,
    drop_down::*,
    radio_button::*,
    text_input::{TextInput},
    link_label::{LinkLabel},
    desktop_window::{DesktopWindow},
    scroll_bars::{ScrollBars},
    scroll_shadow::{ScrollShadow},
    scroll_bar::{ScrollBar},
    widget::{
        WidgetUid,
        WidgetDraw,
        WidgetDrawApi,
        CreateAt,
        WidgetActions,
        WidgetActionsApi,
        WidgetActionItem,
        WidgetRef,
        Widget,
        WidgetRegistry,
        WidgetFactory,
        WidgetAction,
    }
};


pub fn live_design(cx: &mut Cx) {
    makepad_draw_2d::live_design(cx);
    crate::log_list::live_design(cx);
    crate::log_icon::live_design(cx);
    crate::debug_view::live_design(cx);
    crate::fold_header::live_design(cx);
    crate::splitter::live_design(cx);
    crate::theme::live_design(cx);
    crate::slider::live_design(cx);
    crate::label::live_design(cx);
    crate::nav_control::live_design(cx);
    crate::frame::live_design(cx);
    crate::fold_button::live_design(cx);
    crate::text_input::live_design(cx);
    crate::link_label::live_design(cx);
    crate::scroll_shadow::live_design(cx);
    crate::button::live_design(cx);
    crate::desktop_button::live_design(cx);
    crate::desktop_window::live_design(cx);
    crate::bare_window::live_design(cx);
    crate::window_menu::live_design(cx);
    crate::scroll_bar::live_design(cx);
    crate::scroll_bars::live_design(cx);
    crate::check_box::live_design(cx);
    crate::radio_button::live_design(cx);
    crate::tab_close_button::live_design(cx);
    crate::tab::live_design(cx);
    crate::tab_bar::live_design(cx);
    crate::dock::live_design(cx);
    crate::color_picker::live_design(cx);
    crate::file_tree::live_design(cx);
    crate::slides_view::live_design(cx);
    crate::list_box::live_design(cx);
    crate::popup_menu::live_design(cx);
    crate::drop_down::live_design(cx);
}
