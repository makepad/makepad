pub use makepad_draw::makepad_platform;
pub use makepad_platform::makepad_script;
pub use makepad_draw2 as makepad_draw;
pub use makepad_derive_widget2 as makepad_derive_widget;
pub use makepad_draw::*;
pub use makepad_derive_widget::*;
pub use makepad_platform::log;

#[cfg(feature = "html")]
pub use makepad_html;

#[cfg(feature = "jpg")]
pub use makepad_zune_jpeg;

#[cfg(feature = "png")]
pub use makepad_zune_png;

pub mod animator;
// pub mod button;
// pub mod cached_widget;
// pub mod label;
// pub mod image;
// pub mod image_blend;
// pub mod icon;
// pub mod link_label;
// pub mod drop_down;
// pub mod popup_menu;
// pub mod check_box;
// pub mod radio_button;
// pub mod text_input;
// pub mod slider;
pub mod scroll_bar;
pub mod scroll_bars;
// pub mod splitter;
// pub mod vectorline;
// pub mod fold_header;
// pub mod fold_button;
// pub mod multi_window;
// pub mod dock;
// pub mod tab;
// pub mod tab_bar;
// pub mod tab_close_button;
// pub mod portal_list;
// pub mod portal_list2;
// pub mod stack_navigation;
// pub mod expandable_panel;
// pub mod desktop_button;
pub mod window;
// pub mod scroll_shadow;
// pub mod window_menu;
// pub mod html;
// pub mod markdown;
// pub mod text_flow;
// pub mod multi_image;
// pub mod modal;
// pub mod tooltip;
// pub mod popup_notification;
// pub mod loading_spinner;
// pub mod web_view;

// Only available on Android at the moment
// #[cfg(target_os="android")]
// pub mod video;
// pub mod rotated_image;
// pub mod slide_panel;
// pub mod page_flip;
// pub mod keyboard_view;
// pub mod flat_list;
// pub mod file_tree;
// pub mod slides_view;
// pub mod color_picker;
// pub mod root;

// pub mod debug_view;
// pub mod performance_view;
// pub mod nav_control;

pub mod view;

// pub mod adaptive_view;
pub mod view_ui;

// pub mod toggle_panel;
// pub mod command_text_input;

pub mod widget;
pub mod widget_match_event;

// pub mod touch_gesture;
// #[macro_use]
// pub mod data_binding;

pub mod theme_desktop_skeleton;
pub mod theme_desktop_dark;
pub mod theme_desktop_light;

// pub mod image_cache;
// pub mod bare_step;
// pub mod turtle_step;
/*
pub mod designer;
pub mod designer_dummy;
pub mod designer_theme;
pub mod designer_outline_tree;
pub mod designer_view;
pub mod designer_outline;
pub mod designer_data;
pub mod designer_toolbox;
*/
//pub mod defer_with_redraw;


pub use crate::{
    
//    data_binding::{DataBindingStore, DataBindingMap},
//    button::*,
//    cached_widget::*,
    view::*,
//    adaptive_view::*,
//    image::*,
//    image_blend::*,
//    icon::*,
//    label::*,
//    slider::*,
//    root::*,
//    text_flow::*,
//    markdown::*,
//    html::*,
//    check_box::*,
//    drop_down::*,
//    modal::*,
//    tooltip::*,
//    popup_notification::*,
//    video::*,
//    radio_button::*,
//    text_input::*,
//    link_label::*,
//    portal_list::*,
//    portal_list2::*,
//    flat_list::*,
//    page_flip::*,
//    slide_panel::*,
//    fold_button::*,
//    dock::*,
//    stack_navigation::*,
//    expandable_panel::*,
//    command_text_input::*,
    window::*,
//    multi_window::*,
//    web_view::*,
    scroll_bars::{ScrollBars},
//    scroll_shadow::{DrawScrollShadow},
scroll_bar::{ScrollBar},
//    slides_view::{SlidesView},
//    widget_match_event::WidgetMatchEvent,
//    toggle_panel::*,
//    defer_with_redraw::*,

    widget::{
        WidgetSet,
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
        WidgetRegister,
        OptionWidgetRefExt,
        WidgetRef,
        Widget,
        WidgetNode,
        WidgetRegistry,
        WidgetFactory,
        WidgetSetIterator,
        DrawStateWrap,
    }
};

pub fn script_mod(vm: &mut ScriptVm){
    makepad_draw2::script_mod(vm);
    
    vm.heap.new_module(id!(prelude));
    vm.heap.new_module(id!(themes));
    crate::theme_desktop_dark::script_mod(vm);
    crate::animator::script_mod(vm);
    {
        script_mod!{ 
            mod.prelude.widgets_internal = {
                ..mod.pod,
                ..mod.math,
                ..mod.sdf,
                ..mod.shader,
                ..mod.turtle,
                ..mod.turtle.Size,
                ..mod.turtle.Flow,
                theme:mod.theme,
                draw:mod.draw,
                MouseCursor:mod.draw.MouseCursor
            }                
        }
        script_mod(vm);    
    }
            
    vm.heap.new_module(id!(widgets));
    crate::scroll_bar::script_mod(vm);
    crate::scroll_bars::script_mod(vm);
    crate::view::script_mod(vm);
    crate::view_ui::script_mod(vm);
    crate::window::script_mod(vm);
    
    {
        script_mod!{
            mod.prelude.widgets = {
                ..mod.pod,
                ..mod.math,
                ..mod.sdf,
                mod.theme,
                mod.draw,
                ..mod.shader,
                ..mod.widgets,
                ..mod.turtle,
                ..mod.turtle.Size,
                ..mod.turtle.Flow,
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

/*
pub fn live_design(cx: &mut Cx) {
    cx.link(live_id!(theme), live_id!(theme_desktop_dark));
    if cx.in_makepad_studio() {
        cx.link(live_id!(designer), live_id!(designer_real));
    }
    else{
        cx.link(live_id!(designer), live_id!(designer_dummy));
    }
    //makepad_fonts_emoji::live_design(cx);
    //makepad_fonts_chinese_regular::live_design(cx);
    //makepad_fonts_chinese_regular_2::live_design(cx);
    //makepad_fonts_chinese_bold::live_design(cx);
    //makepad_fonts_chinese_bold_2::live_design(cx);
    
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
*/