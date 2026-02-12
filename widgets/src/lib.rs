pub extern crate makepad_derive_widget;
pub extern crate makepad_draw;
pub use makepad_derive_widget::*;
pub use makepad_draw::makepad_platform;
pub use makepad_draw::*;
pub use makepad_platform::log;
pub use makepad_platform::makepad_script;

pub use makepad_html;
#[cfg(feature = "pdf")]
pub use makepad_pdf_parse;

pub use makepad_zune_jpeg;

pub use makepad_zune_png;

// Core modules (used internally first)
pub mod animator;
pub mod theme_desktop_dark;
pub mod theme_desktop_light;
pub mod theme_desktop_skeleton;
pub mod widget;
pub mod widget_async;
pub mod widget_match_event;
pub mod widget_tree;

// Modules ordered to match script_mod calls
pub mod rubber_view;
pub mod scroll_bar;
pub mod scroll_bars;
pub mod view;
pub mod view_ui;

pub mod button;
pub mod check_box;
pub mod icon;
pub mod image;
pub mod image_blend;
pub mod image_cache;
pub mod label;
pub mod link_label;
pub mod radio_button;

pub mod adaptive_view;
pub mod desktop_button;
pub mod keyboard_view;
pub mod nav_control;
pub mod window;
pub mod window_menu;

pub mod drop_down;
pub mod popup_menu;
pub mod slider;
pub mod text_input;

pub mod splitter;

pub mod fold_button;
pub mod fold_header;

pub mod loading_spinner;

pub mod bare_step;
pub mod turtle_step;

pub mod portal_list;
pub mod text_flow;

pub mod cached_widget;
pub mod root;

pub mod dock;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;

pub mod html;
pub mod markdown;

#[cfg(feature = "maps")]
pub mod map;
pub mod math_view;
#[cfg(feature = "pdf")]
pub mod pdf_view;
pub mod splash;
pub mod svg;
pub mod vector;

// Touch gesture support (used by expandable_panel)
pub mod touch_gesture;

// Navigation and panels
pub mod expandable_panel;
pub mod scroll_shadow;
pub mod stack_navigation;

pub mod file_tree;
pub mod modal;
pub mod page_flip;
pub mod popup_notification;
pub mod slides_view;
pub mod tooltip;
#[cfg(target_os = "android")]
pub mod video;

pub mod command_text_input;
pub mod defer_with_redraw;
pub mod slide_panel;

pub mod flat_list;

pub mod chart;

// Commented out modules (not yet converted)
// lets depricate these for now
// pub mod toggle_panel;
// pub mod vectorline;
// pub mod web_view;
// pub mod rotated_image;
// pub mod color_picker;
// pub mod debug_view;
// pub mod performance_view;
// pub mod data_binding;
// pub mod designer;
// pub mod designer_dummy;
// pub mod designer_theme;
// pub mod designer_outline_tree;
// pub mod designer_view;
// pub mod designer_outline;
// pub mod designer_data;
// pub mod designer_toolbox;

pub use crate::{
    adaptive_view::*,
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl},
    // loading_spinner - no public exports
    bare_step::*,
    button::*,
    cached_widget::*,
    check_box::*,
    desktop_button::*,
    dock::*,

    drop_down::*,
    expandable_panel::*,
    file_tree::*,
    flat_list::*,

    fold_button::*,
    fold_header::*,

    icon::*,

    image::*,
    image_blend::*,
    image_cache::*,
    keyboard_view::*,
    // view_ui - no public exports
    label::*,
    link_label::*,
    modal::*,
    nav_control::*,
    page_flip::*,
    popup_menu::*,
    popup_notification::*,
    portal_list::*,
    radio_button::*,
    root::*,

    rubber_view::*,
    // Ordered to match script_mod calls
    scroll_bar::ScrollBar,
    scroll_bars::ScrollBars,
    scroll_shadow::*,
    slide_panel::*,
    slider::*,
    slides_view::*,

    splitter::*,

    stack_navigation::*,
    tab::*,
    tab_bar::*,
    tab_close_button::*,
    text_flow::*,

    text_input::*,
    tooltip::*,
    // Navigation and panels
    touch_gesture::*,
    turtle_step::*,

    view::*,
    widget::{
        CreateAt, DrawStateWrap, DrawStep, DrawStepApi, OptionWidgetRefExt, Widget, WidgetAction,
        WidgetActionCast, WidgetActionCxExt, WidgetActionOptionApi, WidgetActionTrait,
        WidgetActionsApi, WidgetFactory, WidgetNode, WidgetRef, WidgetRegister, WidgetRegistry,
        WidgetSet, WidgetSetIterator, WidgetUid,
    },
    widget_async::{
        set_widget_async_trace, CxWidgetToScriptCallExt, ScriptAsyncCalls, ScriptAsyncId,
        ScriptAsyncResult,
    },
    widget_match_event::WidgetMatchEvent,
    widget_tree::CxWidgetExt,

    window::*,

    window_menu::*,
};

pub use crate::html::*;

pub use crate::markdown::*;

#[cfg(feature = "maps")]
pub use crate::map::view::*;

pub use crate::math_view::*;

pub use crate::splash::*;

#[cfg(feature = "pdf")]
pub use crate::pdf_view::*;
pub use crate::svg::*;
pub use crate::vector::*;

pub use crate::chart::*;

#[cfg(target_os = "android")]
pub use crate::video::*;

pub fn script_mod(vm: &mut ScriptVm) {
    makepad_draw::script_mod(vm);

    vm.bx.heap.new_module(id!(prelude));
    vm.bx.heap.new_module(id!(themes));
    crate::theme_desktop_dark::script_mod(vm);
    crate::animator::script_mod(vm);
    // make the prelude for our own widgets
    {
        script_mod! {
            mod.prelude.widgets_internal = {
                ..mod.res,
                ..mod.helper,
                ..mod.animator,
                ..mod.animator.Play,
                ..mod.animator.Ease,
                ..mod.pod,
                ..mod.math,
                ..mod.sdf,
                ..mod.shader,
                ..mod.turtle,
                ..mod.turtle.Size,
                ..mod.turtle.Flow,
                ..mod.std
                theme:mod.theme,
                draw:mod.draw,
                MouseCursor:mod.draw.MouseCursor
            }
        }
        script_mod(vm);
    }

    vm.bx.heap.new_module(id!(widgets));

    crate::scroll_bar::script_mod(vm);
    crate::scroll_bars::script_mod(vm);
    crate::view::script_mod(vm);
    crate::view_ui::script_mod(vm);
    crate::rubber_view::script_mod(vm);

    crate::label::script_mod(vm);
    crate::link_label::script_mod(vm);
    crate::button::script_mod(vm);
    crate::check_box::script_mod(vm);
    crate::radio_button::script_mod(vm);
    crate::image::script_mod(vm);
    crate::image_blend::script_mod(vm);
    crate::icon::script_mod(vm);

    crate::adaptive_view::script_mod(vm);
    crate::desktop_button::script_mod(vm);
    crate::keyboard_view::script_mod(vm);
    crate::window_menu::script_mod(vm);
    crate::nav_control::script_mod(vm);
    crate::window::script_mod(vm);

    crate::popup_menu::script_mod(vm);
    crate::drop_down::script_mod(vm);
    crate::text_input::script_mod(vm);
    crate::slider::script_mod(vm);

    crate::splitter::script_mod(vm);

    crate::fold_button::script_mod(vm);
    crate::fold_header::script_mod(vm);

    crate::loading_spinner::script_mod(vm);

    crate::bare_step::script_mod(vm);
    crate::turtle_step::script_mod(vm);

    crate::portal_list::script_mod(vm);
    crate::text_flow::script_mod(vm);

    crate::cached_widget::script_mod(vm);
    crate::root::script_mod(vm);

    crate::tab_close_button::script_mod(vm);
    crate::tab::script_mod(vm);
    crate::tab_bar::script_mod(vm);
    crate::dock::script_mod(vm);

    // Navigation and panels
    crate::scroll_shadow::script_mod(vm);
    crate::stack_navigation::script_mod(vm);
    crate::expandable_panel::script_mod(vm);
    crate::modal::script_mod(vm);
    crate::tooltip::script_mod(vm);
    crate::popup_notification::script_mod(vm);
    #[cfg(target_os = "android")]
    crate::video::script_mod(vm);
    crate::page_flip::script_mod(vm);
    crate::file_tree::script_mod(vm);
    crate::flat_list::script_mod(vm);
    crate::slides_view::script_mod(vm);
    crate::slide_panel::script_mod(vm);

    crate::html::script_mod(vm);
    crate::markdown::script_mod(vm);

    crate::splash::script_mod(vm);
    #[cfg(feature = "pdf")]
    crate::pdf_view::script_mod(vm);
    crate::svg::script_mod(vm);
    crate::vector::script_mod(vm);
    crate::chart::script_mod(vm);
    #[cfg(feature = "maps")]
    crate::map::style::script_mod(vm);
    #[cfg(feature = "maps")]
    crate::map::view::script_mod(vm);
    crate::math_view::script_mod(vm);

    // make the prelude.widgetst with all our components

    {
        script_mod! {
            mod.prelude.widgets = {
                ..mod.res,
                ..mod.helper,
                ..mod.std,
                ..mod.pod,
                ..mod.math,
                ..mod.sdf,
                theme:mod.theme,
                draw:mod.draw,
                net:mod.net,
                ..mod.animator,
                ..mod.animator.Play,
                ..mod.animator.Ease,
                ..mod.shader,
                ..mod.widgets,
                ..mod.turtle,
                ..mod.turtle.Size,
                ..mod.turtle.Flow,
                ..mod.draw.MouseCursor
            }
        }
        script_mod(vm);
    }
    //crate::theme_desktop_dark::script_mod(vm);
    //makepad_fonts_emoji2::script_mod(vm);
    //makepad_fonts_chinese_regular2::script_mod(vm);
    //makepad_fonts_chinese_regular2_2::script_mod(vm);
    //makepad_fonts_chinese_bold2::script_mod(vm);
    //makepad_fonts_chinese_bold2_2::script_mod(vm);
}
