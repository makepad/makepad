pub use makepad_draw::makepad_platform;
pub use makepad_draw;
pub use makepad_html;
pub use makepad_derive_widget;
pub use makepad_draw::*;
pub use makepad_derive_widget::*;

pub mod button;
pub mod label;
pub mod image;
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
pub mod vectorline;
pub mod fold_header;
pub mod fold_button;
pub mod multi_window;
pub mod designer;
pub mod dock;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod portal_list;
pub mod stack_navigation;
pub mod expandable_panel;
pub mod desktop_button;
pub mod window;
pub mod scroll_shadow;
pub mod window_menu;
pub mod html;
pub mod text_flow;
// Only available on Android at the moment
// #[cfg(target_os="android")]
pub mod video;
pub mod rotated_image;
pub mod slide_panel;
pub mod page_flip;
pub mod keyboard_view;
pub mod flat_list;

pub mod file_tree;
pub mod slides_view;
pub mod color_picker;

pub mod debug_view;
pub mod performance_view;
pub mod nav_control;

pub mod view;
pub mod widget;
pub mod widget_match_event;

pub mod touch_gesture;

#[macro_use]
pub mod data_binding;

mod base;
mod theme_desktop_dark;
pub mod image_cache;

pub use crate::{
    data_binding::{DataBindingStore, DataBindingMap},
    button::*,
    view::*,
    image::*,
    label::*,
    slider::*,
    check_box::*,
    drop_down::*,
    video::*,
    radio_button::*,
    text_input::*,
    link_label::*,
    portal_list::*,
    flat_list::*,
    page_flip::*,
    slide_panel::*,
    fold_button::*,
    dock::*,
    stack_navigation::*,
    expandable_panel::*,
    window::*,
    tab::TabClosable,
    scroll_bars::{ScrollBars},
    scroll_shadow::{DrawScrollShadow},
    scroll_bar::{ScrollBar},
    slides_view::{SlidesView},
    widget_match_event::WidgetMatchEvent,
    widget::{
        WidgetSet,
        WidgetSetIterator,
        WidgetUid,
        DrawStep,
        DrawStepApi,
        CreateAt,
        WidgetCache,
        WidgetActionCxExt,
        WidgetActionsApi,
        WidgetActionTrait,
        WidgetAction,
        WidgetActionCast,
        WidgetActionOptionApi,
        WidgetRef,
        Widget,
        WidgetNode,
        WidgetRegistry,
        WidgetFactory,
        DrawStateWrap,
    }
};


pub fn live_design(cx: &mut Cx) {
    makepad_draw::live_design(cx);
    crate::page_flip::live_design(cx);
    crate::debug_view::live_design(cx);
    crate::performance_view::live_design(cx);
    crate::fold_header::live_design(cx);
    crate::splitter::live_design(cx);
    crate::base::live_design(cx);
    crate::theme_desktop_dark::live_design(cx);
    crate::slider::live_design(cx);
    crate::label::live_design(cx);
    crate::nav_control::live_design(cx);
    crate::image::live_design(cx);
    crate::rotated_image::live_design(cx);
    crate::video::live_design(cx);
    crate::view::live_design(cx);
    crate::fold_button::live_design(cx);
    crate::text_input::live_design(cx);
    crate::link_label::live_design(cx);
    crate::scroll_shadow::live_design(cx);
    crate::button::live_design(cx);
    crate::desktop_button::live_design(cx);
    crate::window::live_design(cx);
    crate::window_menu::live_design(cx);
    crate::scroll_bar::live_design(cx);
    crate::scroll_bars::live_design(cx);
    crate::check_box::live_design(cx);
    crate::radio_button::live_design(cx);
    crate::popup_menu::live_design(cx);
    crate::drop_down::live_design(cx);
    crate::multi_window::live_design(cx);
    crate::designer::live_design(cx);
    crate::portal_list::live_design(cx);
    crate::flat_list::live_design(cx);
    crate::slide_panel::live_design(cx);
    crate::tab::live_design(cx);
    crate::tab_bar::live_design(cx);
    crate::dock::live_design(cx);
    crate::color_picker::live_design(cx);
    crate::file_tree::live_design(cx);
    crate::slides_view::live_design(cx);
    crate::tab_close_button::live_design(cx);
    crate::keyboard_view::live_design(cx);
    crate::vectorline::live_design(cx);
    crate::stack_navigation::live_design(cx);
    crate::expandable_panel::live_design(cx);
    crate::html::live_design(cx);
    crate::text_flow::live_design(cx);
}
