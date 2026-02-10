pub use makepad_derive_widget;
pub use makepad_derive_widget::*;
pub use makepad_draw;
pub use makepad_draw::makepad_platform;
pub use makepad_draw::*;
pub use makepad_html;
pub use makepad_platform::log;
pub use makepad_zune_jpeg;
pub use makepad_zune_png;

pub mod button;
pub mod cached_widget;
pub mod check_box;
pub mod desktop_button;
pub mod dock;
pub mod drop_down;
pub mod expandable_panel;
pub mod fold_button;
pub mod fold_header;
pub mod html;
pub mod icon;
pub mod image;
pub mod image_blend;
pub mod label;
pub mod link_label;
pub mod loading_spinner;
pub mod markdown;
pub mod modal;
pub mod multi_image;
pub mod multi_window;
pub mod popup_menu;
pub mod popup_notification;
pub mod portal_list;
pub mod portal_list2;
pub mod radio_button;
pub mod scroll_bar;
pub mod scroll_bars;
pub mod scroll_shadow;
pub mod slider;
pub mod splitter;
pub mod stack_navigation;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod text_flow;
pub mod text_input;
pub mod tooltip;
pub mod vectorline;
pub mod web_view;
pub mod window;
pub mod window_menu;

// Only available on Android at the moment
// #[cfg(target_os="android")]
pub mod color_picker;
pub mod file_tree;
pub mod flat_list;
pub mod keyboard_view;
pub mod page_flip;
pub mod root;
pub mod rotated_image;
pub mod slide_panel;
pub mod slides_view;
pub mod video;

pub mod debug_view;
pub mod nav_control;
pub mod performance_view;

pub mod adaptive_view;
pub mod command_text_input;
pub mod toggle_panel;
pub mod view;
pub mod view_ui;
pub mod widget;
pub mod widget_match_event;

pub mod touch_gesture;

#[macro_use]
pub mod data_binding;

pub mod bare_step;
pub mod image_cache;
pub mod theme_desktop_dark;
pub mod theme_desktop_light;
pub mod theme_desktop_skeleton;
pub mod theme_mobile_dark;
pub mod theme_mobile_light;
pub mod turtle_step;

pub mod designer;
pub mod designer_data;
pub mod designer_dummy;
pub mod designer_outline;
pub mod designer_outline_tree;
pub mod designer_theme;
pub mod designer_toolbox;
pub mod designer_view;

pub mod defer_with_redraw;

pub use crate::{
    adaptive_view::*,
    button::*,
    cached_widget::*,
    check_box::*,
    command_text_input::*,
    data_binding::{DataBindingMap, DataBindingStore},
    defer_with_redraw::*,
    dock::*,
    drop_down::*,
    expandable_panel::*,
    flat_list::*,
    fold_button::*,
    html::*,
    icon::*,
    image::*,
    image_blend::*,
    label::*,
    link_label::*,
    markdown::*,
    modal::*,
    multi_window::*,
    page_flip::*,
    popup_notification::*,
    portal_list::*,
    portal_list2::*,
    radio_button::*,
    root::*,
    scroll_bar::ScrollBar,
    scroll_bars::ScrollBars,
    scroll_shadow::DrawScrollShadow,
    slide_panel::*,
    slider::*,
    slides_view::SlidesView,
    stack_navigation::*,
    text_flow::*,
    text_input::*,
    toggle_panel::*,
    tooltip::*,
    video::*,
    view::*,
    web_view::*,
    widget::{
        CreateAt, DrawStateWrap, DrawStep, DrawStepApi, OptionWidgetRefExt, Widget, WidgetAction,
        WidgetActionCast, WidgetActionCxExt, WidgetActionOptionApi, WidgetActionTrait,
        WidgetActionsApi, WidgetCache, WidgetFactory, WidgetNode, WidgetRef, WidgetRegistry,
        WidgetSet, WidgetSetIterator, WidgetUid,
    },
    widget_match_event::WidgetMatchEvent,
    window::*,
};

pub fn live_design(cx: &mut Cx) {
    cx.link(live_id!(theme), live_id!(theme_desktop_dark));
    if cx.in_makepad_studio() {
        cx.link(live_id!(designer), live_id!(designer_real));
    } else {
        cx.link(live_id!(designer), live_id!(designer_dummy));
    }
    makepad_fonts_emoji::live_design(cx);
    makepad_fonts_chinese_regular::live_design(cx);
    makepad_fonts_chinese_regular_2::live_design(cx);
    makepad_fonts_chinese_bold::live_design(cx);
    makepad_fonts_chinese_bold_2::live_design(cx);
    makepad_draw::live_design(cx);
    crate::page_flip::live_design(cx);
    crate::debug_view::live_design(cx);
    crate::performance_view::live_design(cx);
    crate::fold_header::live_design(cx);
    crate::splitter::live_design(cx);
    crate::theme_desktop_skeleton::live_design(cx);
    crate::theme_desktop_dark::live_design(cx);
    crate::theme_desktop_light::live_design(cx);
    crate::theme_mobile_dark::live_design(cx);
    crate::theme_mobile_light::live_design(cx);
    crate::slider::live_design(cx);
    crate::label::live_design(cx);
    crate::nav_control::live_design(cx);
    crate::image::live_design(cx);
    crate::multi_image::live_design(cx);
    crate::image_blend::live_design(cx);
    crate::icon::live_design(cx);
    crate::rotated_image::live_design(cx);
    crate::modal::live_design(cx);
    crate::tooltip::live_design(cx);
    crate::popup_notification::live_design(cx);
    crate::video::live_design(cx);
    crate::view::live_design(cx);
    crate::adaptive_view::live_design(cx);
    crate::view_ui::live_design(cx);
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
    crate::portal_list::live_design(cx);
    crate::portal_list2::live_design(cx);
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
    crate::text_flow::live_design(cx);
    crate::markdown::live_design(cx);
    crate::html::live_design(cx);
    crate::root::live_design(cx);
    crate::bare_step::live_design(cx);
    crate::turtle_step::live_design(cx);
    crate::toggle_panel::live_design(cx);
    crate::cached_widget::live_design(cx);
    crate::command_text_input::live_design(cx);
    crate::loading_spinner::live_design(cx);
    crate::web_view::live_design(cx);

    crate::designer_theme::live_design(cx);
    crate::designer::live_design(cx);
    crate::designer_dummy::live_design(cx);
    crate::designer_view::live_design(cx);
    crate::designer_outline::live_design(cx);
    crate::designer_outline_tree::live_design(cx);
    crate::designer_toolbox::live_design(cx);
}
