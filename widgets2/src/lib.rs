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

// Core modules (used internally first)
pub mod widget;
pub mod widget_match_event;
pub mod animator;
pub mod theme_desktop_skeleton;
pub mod theme_desktop_dark;
pub mod theme_desktop_light;

// Modules ordered to match script_mod calls
pub mod scroll_bar;
pub mod scroll_bars;
pub mod view;
pub mod view_ui;

pub mod label;
pub mod link_label;
pub mod button;
pub mod check_box;
pub mod radio_button;
pub mod image_cache;
pub mod image;
pub mod image_blend;
pub mod icon;

pub mod adaptive_view;
pub mod desktop_button;
pub mod keyboard_view;
pub mod window_menu;
pub mod nav_control;
pub mod window;

pub mod popup_menu;
pub mod drop_down;
pub mod text_input;
pub mod slider;

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

pub mod tab_close_button;
pub mod tab;
pub mod tab_bar;
pub mod dock;

#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "markdown")]
pub mod markdown;

// Commented out modules (not yet converted)
// pub mod vectorline;
// pub mod multi_window;
// pub mod stack_navigation;
// pub mod expandable_panel;
// pub mod scroll_shadow;
// pub mod multi_image;
// pub mod modal;
// pub mod tooltip;
// pub mod popup_notification;
// pub mod web_view;
// pub mod video;
// pub mod rotated_image;
// pub mod slide_panel;
// pub mod page_flip;
// pub mod flat_list;
// pub mod file_tree;
// pub mod slides_view;
// pub mod color_picker;
// pub mod debug_view;
// pub mod performance_view;
// pub mod toggle_panel;
// pub mod command_text_input;
// pub mod touch_gesture;
// pub mod data_binding;
// pub mod designer;
// pub mod designer_dummy;
// pub mod designer_theme;
// pub mod designer_outline_tree;
// pub mod designer_view;
// pub mod designer_outline;
// pub mod designer_data;
// pub mod designer_toolbox;
// pub mod defer_with_redraw;


pub use crate::{
    // Ordered to match script_mod calls
    scroll_bar::{ScrollBar},
    scroll_bars::{ScrollBars},
    view::*,
    // view_ui - no public exports
    
    label::*,
    link_label::*,
    button::*,
    check_box::*,
    radio_button::*,
    image::*,
    image_cache::*,
    icon::*,
    
    adaptive_view::*,
    desktop_button::*,
    keyboard_view::*,
    window_menu::*,
    nav_control::*,
    window::*,
    
    popup_menu::*,
    drop_down::*,
    text_input::*,
    slider::*,
    
    splitter::*,
    
    fold_button::*,
    fold_header::*,
    
    // loading_spinner - no public exports
    
    bare_step::*,
    turtle_step::*,
    
    portal_list::*,
    text_flow::*,
    
    cached_widget::*,
    image_blend::*,
    root::*,
    
    tab_close_button::*,
    tab::*,
    tab_bar::*,
    dock::*,
    
    widget_match_event::WidgetMatchEvent,

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

#[cfg(feature = "html")]
pub use crate::html::*;

#[cfg(feature = "markdown")]
pub use crate::markdown::*;

pub fn script_mod(vm: &mut ScriptVm){
    makepad_draw2::script_mod(vm);
    
    vm.bx.heap.new_module(id!(prelude));
    vm.bx.heap.new_module(id!(themes));
    crate::theme_desktop_dark::script_mod(vm);
    crate::animator::script_mod(vm);
    // make the prelude for our own widgets
    {
        script_mod!{ 
            mod.prelude.widgets_internal = {
                ..mod.res,
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
    
    #[cfg(feature = "html")]
    crate::html::script_mod(vm);
    #[cfg(feature = "markdown")]
    crate::markdown::script_mod(vm);
        
        
    // make the prelude.widgetst with all our components
    
    {
        script_mod!{
            mod.prelude.widgets = {
                ..mod.res,
                ..mod.std,
                ..mod.pod,
                ..mod.math,
                ..mod.sdf,
                mod.theme,
                mod.draw,
                ..mod.animator,
                ..mod.animator.Play,
                ..mod.animator.Ease,
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
