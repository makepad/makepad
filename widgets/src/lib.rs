pub extern crate makepad_derive_widget;
pub extern crate makepad_draw;
pub use makepad_derive_widget::*;
pub use makepad_draw::makepad_platform;
pub use makepad_draw::*;
pub use makepad_platform::log;
pub use makepad_platform::makepad_script;
pub use makepad_script::script_eval;
pub use makepad_script::{ScriptValue, ScriptVm};

pub use makepad_html;
#[cfg(feature = "pdf")]
pub use makepad_pdf_parse;

pub use makepad_draw::makepad_zune_jpeg;
pub use makepad_draw::makepad_zune_png;

// Core modules (used internally first)
pub mod animator;
pub mod font_policy;
pub mod theme_desktop_dark;
pub mod theme_desktop_light;
pub mod theme_desktop_skeleton;
pub mod theme_tokens;
pub mod widget;
pub mod widget_async;
pub mod splash_host;
pub mod splash_storage;
pub mod widget_match_event;
pub mod widget_tree;

// Modules ordered to match script_mod calls
pub mod rubber_view;
pub mod scroll_bar;
pub mod scroll_bars;
pub mod scroll_motion;
pub mod view;
pub mod view_ui;
pub mod grid;

pub mod animated_image_gif;
pub mod badge;
pub mod button_group;
pub mod chip;
pub mod menu;
pub mod select;
pub mod accordion;
pub mod dialog;
pub mod drawer;
pub mod toast;
pub mod breadcrumb;
pub mod browser;
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
pub mod alert;
pub mod divider;
pub mod desktop_button;
pub mod gauss_view;
pub mod keyboard_view;
pub mod nav_control;
pub mod tweaker;
pub mod reflect;
pub mod ai_slot;
#[cfg(feature = "voice")]
pub mod voice_wave;
pub mod window;
pub mod window_menu;
#[cfg(feature = "voice")]
mod window_voice_input;

pub mod combo_box;
pub mod drop_down;
pub mod drop_down2;
pub mod popup_menu;
pub mod slider;
pub mod text_input;
pub mod drop_slider;
pub mod drop_toggles;
pub mod overlay_place;
pub mod tip;
pub mod popover;
pub mod overlay_layers;
pub mod value_input;
pub mod fab_controls;
pub mod menu_bar;

pub mod splitter;

pub mod fold_button;
pub mod fold_header;

pub mod glass_panel;
pub mod loading_spinner;
pub mod progress;
pub mod marquee;
pub mod spinner;

pub mod bare_step;
pub mod turtle_step;

pub mod data_grid;
pub mod portal_list;
pub mod reorder_list;
pub mod text_flow;

pub mod cached_widget;
pub mod root;

pub mod dock;
pub mod tab;
pub mod tab_bar;
pub mod tabs;
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

pub mod callout_tooltip;
pub mod file_tree;
pub mod modal;
pub mod page_flip;
pub mod pagination;
pub mod placeholder;
pub mod popup_notification;
pub mod slides_view;
pub mod tooltip;
pub mod video;

pub mod command_text_input;
pub mod defer_with_redraw;
pub mod slide_panel;

pub mod flat_list;

pub mod chart;
pub mod perf_graph;
pub mod screen_cap;

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

pub use crate::{
    adaptive_view::*,
    alert::*,
    divider::*,
    animated_image_gif::*,
    badge::*,
    breadcrumb::*,
    button_group::*,
    chip::*,
    menu::*,
    select::*,
    accordion::*,
    dialog::*,
    drawer::*,
    toast::*,
    placeholder::*,
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    // loading_spinner - no public exports
    bare_step::*,
    button::*,
    cached_widget::*,
    callout_tooltip::*,
    check_box::*,
    combo_box::*,
    desktop_button::*,
    dock::*,

    drop_down::*,
    drop_down2::*,
    drop_toggles::*,
    overlay_place::*,
    popover::*,
    overlay_layers::*,
    expandable_panel::*,
    file_tree::*,
    flat_list::*,

    fold_button::*,
    fold_header::*,
    gauss_view::*,
    glass_panel::*,
    grid::*,

    icon::*,

    image::*,
    image_blend::*,
    image_cache::*,
    keyboard_view::*,
    // view_ui - no public exports
    label::*,
    link_label::*,
    menu_bar::*,
    modal::*,
    nav_control::*,
    page_flip::*,
    popup_menu::*,
    popup_notification::*,
    data_grid::*,
    portal_list::*,
    progress::*,
    reorder_list::*,
    radio_button::*,
    reflect::*,
    root::*,

    rubber_view::*,
    // Ordered to match script_mod calls
    scroll_bar::ScrollBar,
    scroll_bars::ScrollBars,
    scroll_shadow::*,
    slide_panel::*,
    slider::*,
    slides_view::*,
    marquee::*,
    spinner::*,

    splitter::*,

    stack_navigation::*,
    tab::*,
    tab_bar::*,
    tabs::*,
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
        enter_isolate, leave_isolate, set_widget_async_trace, CxSplashVmExt, CxWidgetToScriptCallExt,
        IsolateEntry, ScriptAsyncCalls, ScriptAsyncId, ScriptAsyncResult, SplashVmId, MAIN_SPLASH_VM_ID,
    },
    widget_match_event::WidgetMatchEvent,
    widget_tree::{set_ui_root, CxWidgetExt},

    window::*,

    window_menu::*,
};

#[cfg(feature = "cef")]
pub use crate::browser::*;

#[cfg(feature = "voice")]
pub use crate::voice_wave::*;

pub use crate::html::*;

pub use crate::markdown::*;

#[cfg(feature = "maps")]
pub use crate::map::overlay::{MapMarker, MapPuck, MapRouteOverlay};
#[cfg(feature = "maps")]
pub use crate::map::view::*;

pub use crate::math_view::*;

pub use crate::theme_tokens::*;

pub use crate::splash::*;

#[cfg(feature = "pdf")]
pub use crate::pdf_view::*;
pub use crate::svg::*;
pub use crate::vector::*;

pub use crate::chart::*;
pub use crate::perf_graph::*;
pub use crate::screen_cap::*;

pub use crate::video::*;

pub fn theme_mod(vm: &mut ScriptVm) {
    makepad_draw::script_mod(vm);
    if !vm.is_reload() {
        makepad_platform::ime::script_mod(vm);
    }

    vm.bx.heap.new_module(id!(prelude));
    vm.bx.heap.new_module(id!(themes));
    crate::animator::script_mod(vm);
    crate::theme_desktop_dark::script_mod(vm);
    crate::theme_desktop_light::script_mod(vm);
    crate::theme_desktop_skeleton::script_mod(vm);
    #[cfg(not(target_arch = "wasm32"))]
    script_eval!(vm, {
        mod.helper = {
            startup: |v|{
                mod.res.load_all_resources()
                //mod.gc.set_static(mod.prelude.widgets_header);
                //mod.gc.set_static(mod.prelude.widgets_internal);
                //mod.gc.set_static(mod.prelude.widgets);
                v
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    script_eval!(vm, {
        mod.helper = {
            startup: |v|{
                v
            }
        }
    });
    crate::font_policy::install_theme_fonts(vm);
    script_eval!(vm, {
        mod.prelude.widgets_header = {
            ..mod.res,
            ..mod.helper,
            ..mod.std,
            ..mod.pod,
            ..mod.math,
            ..mod.sdf,
            ..mod.animator,
            ..mod.turtle,
            ..mod.text,
            ..mod.ime,
            ..mod.shader,
            ..mod.animator.Play,
            ..mod.animator.Ease,
            draw:mod.draw,
            MouseCursor:mod.draw.MouseCursor
        }
        mod.theme = mod.themes.dark

    });
}

pub fn widgets_mod(vm: &mut ScriptVm) {
    // make the prelude for our own widgets
    script_eval!(vm, {
        mod.prelude.widgets_internal = {
            ..mod.prelude.widgets_header,
            theme:mod.theme,
        }
    });

    vm.bx.heap.new_module(id!(widgets));

    crate::scroll_bar::script_mod(vm);
    crate::scroll_bars::script_mod(vm);
    crate::view::script_mod(vm);
    crate::view_ui::script_mod(vm);
    crate::grid::script_mod(vm);
    crate::rubber_view::script_mod(vm);

    crate::label::script_mod(vm);
    crate::link_label::script_mod(vm);
    crate::button::script_mod(vm);
    crate::alert::script_mod(vm);
    crate::divider::script_mod(vm);
    #[cfg(feature = "cef")]
    crate::browser::script_mod(vm);
    crate::check_box::script_mod(vm);
    crate::radio_button::script_mod(vm);
    crate::image::script_mod(vm);
    crate::animated_image_gif::script_mod(vm);
    crate::image_blend::script_mod(vm);
    crate::icon::script_mod(vm);

    crate::adaptive_view::script_mod(vm);
    crate::desktop_button::script_mod(vm);
    crate::keyboard_view::script_mod(vm);
    #[cfg(feature = "voice")]
    crate::voice_wave::script_mod(vm);
    #[cfg(not(feature = "voice"))]
    script_eval!(vm, {
        use mod.widgets.View
        mod.widgets.VoiceWave = mod.widgets.View {
            visible: false
        }
    });
    crate::window_menu::script_mod(vm);
    crate::nav_control::script_mod(vm);
    crate::tweaker::script_mod(vm);
    crate::gauss_view::script_mod(vm);
    crate::screen_cap::script_mod(vm);
    // The AI slot before the window: its DSL names `AiChatSlot`.
    crate::ai_slot::script_mod(vm);
    crate::window::script_mod(vm);

    crate::popup_menu::script_mod(vm);
    crate::drop_down::script_mod(vm);
    crate::drop_down2::script_mod(vm);
    crate::text_input::script_mod(vm);
    crate::slider::script_mod(vm);
    crate::drop_slider::script_mod(vm);
    crate::drop_toggles::script_mod(vm);
    // The badge first: it owns the role palette and the intent names, and
    // the tooltip, the chip and the segmented control all read them.
    crate::badge::script_mod(vm);
    crate::tip::script_mod(vm);
    crate::popover::script_mod(vm);
    crate::value_input::script_mod(vm);
    crate::fab_controls::script_mod(vm);
    crate::menu_bar::script_mod(vm);
    crate::combo_box::script_mod(vm);

    crate::splitter::script_mod(vm);

    crate::fold_button::script_mod(vm);
    crate::fold_header::script_mod(vm);
    crate::accordion::script_mod(vm);

    crate::loading_spinner::script_mod(vm);
    crate::progress::script_mod(vm);
    crate::breadcrumb::script_mod(vm);
    crate::marquee::script_mod(vm);
    crate::spinner::script_mod(vm);
    crate::glass_panel::script_mod(vm);
    crate::chip::script_mod(vm);
    // The menu first: the group's split and menu buttons carry a
    // `MenuPlace`, and a block's `use` only sees what already exists.
    crate::menu::script_mod(vm);
    crate::button_group::script_mod(vm);
    crate::select::script_mod(vm);
    crate::toast::script_mod(vm);
    crate::placeholder::script_mod(vm);

    crate::bare_step::script_mod(vm);
    crate::turtle_step::script_mod(vm);

    crate::data_grid::script_mod(vm);
    crate::portal_list::script_mod(vm);
    crate::reorder_list::script_mod(vm);
    crate::text_flow::script_mod(vm);

    crate::cached_widget::script_mod(vm);
    crate::root::script_mod(vm);

    crate::tab_close_button::script_mod(vm);
    crate::tab::script_mod(vm);
    crate::tab_bar::script_mod(vm);
    crate::tabs::script_mod(vm);
    crate::dock::script_mod(vm);

    // Navigation and panels
    crate::scroll_shadow::script_mod(vm);
    crate::stack_navigation::script_mod(vm);
    crate::expandable_panel::script_mod(vm);
    crate::modal::script_mod(vm);
    crate::dialog::script_mod(vm);
    crate::drawer::script_mod(vm);
    crate::tooltip::script_mod(vm);
    crate::callout_tooltip::script_mod(vm);
    crate::popup_notification::script_mod(vm);
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
    crate::perf_graph::script_mod(vm);
    #[cfg(feature = "maps")]
    crate::map::style::script_mod(vm);
    #[cfg(feature = "maps")]
    crate::map::view::script_mod(vm);
    crate::math_view::script_mod(vm);

    // The overlay layer host registers LAST, after every layer it owns
    // (tip today; menu layer, toaster and dialog host as they land): a
    // widget deriving from another must register after it, and Window,
    // which registers far above tip and modal, cannot own these for the
    // same reason. Keep it the final registration in this function.
    crate::overlay_layers::script_mod(vm);

    // Safe area inset values (in Makepad layout points). Populated from the platform's
    // display_context which is set before Startup on iOS/Android. On desktop
    // platforms these remain 0.0. Updated at runtime on WindowGeomChange events.
    {
        use makepad_script::trap::NoTrap;
        let insets = vm.cx().display_context.safe_area_insets;
        let widgets = vm.module(id!(widgets));
        vm.bx.heap.set_value(
            widgets,
            id!(SAFE_INSET_PAD_TOP).into(),
            insets.top.into(),
            NoTrap,
        );
        vm.bx.heap.set_value(
            widgets,
            id!(SAFE_INSET_PAD_BOTTOM).into(),
            insets.bottom.into(),
            NoTrap,
        );
        vm.bx.heap.set_value(
            widgets,
            id!(SAFE_INSET_PAD_LEFT).into(),
            insets.left.into(),
            NoTrap,
        );
        vm.bx.heap.set_value(
            widgets,
            id!(SAFE_INSET_PAD_RIGHT).into(),
            insets.right.into(),
            NoTrap,
        );
    }

    script_eval!(vm, {
        mod.prelude.widgets = {
            ..mod.prelude.widgets_header,
            theme:mod.theme,
            ..mod.widgets,
        }
    });
}

pub fn script_mod(vm: &mut ScriptVm) {
    makepad_platform::startup_trace("widgets: theme_mod begin");
    theme_mod(vm);
    makepad_platform::startup_trace("widgets: theme_mod done");
    widgets_mod(vm);
    makepad_platform::startup_trace("widgets: widgets_mod done");
}

#[cfg(test)]
mod animated_image_gif_registration_tests {
    #[test]
    fn test_animated_image_gif_is_registered_separately_from_image() {
        let lib = include_str!("lib.rs");
        let gif = include_str!("animated_image_gif.rs");
        assert!(lib.contains("pub mod animated_image_gif;"));
        assert!(lib.contains("animated_image_gif::*"));
        assert!(lib.contains("crate::animated_image_gif::script_mod(vm);"));
        assert!(gif.contains(
            "mod.widgets.AnimatedImageGifBase = #(AnimatedImageGif::register_widget(vm))"
        ));
        assert!(gif.contains("mod.widgets.AnimatedImageGif = set_type_default()"));
        assert!(lib.contains("pub mod image;"));
        assert!(lib.contains("crate::image::script_mod(vm);"));
    }
}

#[cfg(test)]
mod progress_registration_tests {
    /// The progress family registers as one module, after the bases it
    /// draws with and its `Intent` enum splatted into `mod.widgets`; the
    /// loading spinner it sits beside stays a separate module.
    #[test]
    fn test_progress_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let progress = include_str!("progress.rs");
        assert!(lib.contains("pub mod progress;"));
        assert!(lib.contains("progress::*"));
        let at = lib.find("crate::progress::script_mod(vm);").expect("progress registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before progress");
        }
        assert!(lib.contains("crate::loading_spinner::script_mod(vm);"));
        assert!(progress.contains("mod.widgets.ProgressBarBase = #(ProgressBar::register_widget(vm))"));
        assert!(progress.contains("mod.widgets.ProgressBarFlat = set_type_default()"));
        assert!(progress.contains("mod.widgets.splat(mod.widgets.Intent)"));
        assert_eq!(progress.matches("set_type_default() do mod.widgets.ProgressBarBase").count(), 1);
    }
}

#[cfg(test)]
mod button_group_registration_tests {
    /// The group and the segmented control register after the button and
    /// chip modules they build on, each with one type default at the flat
    /// rung of the ladder.
    #[test]
    fn test_button_group_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let group = include_str!("button_group.rs");
        assert!(lib.contains("pub mod button_group;"));
        assert!(lib.contains("button_group::*"));
        let at = lib.find("crate::button_group::script_mod(vm);").expect("registered");
        for base in [
            "crate::view::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::chip::script_mod(vm);",
            "crate::menu::script_mod(vm);",
        ] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before the group");
        }
        assert!(group.contains("mod.widgets.ButtonGroupBase = #(ButtonGroup::register_widget(vm))"));
        assert!(group.contains("mod.widgets.SegmentedControlBase = #(SegmentedControl::register_widget(vm))"));
        assert!(group.contains("mod.widgets.SegmentedControlFlat = set_type_default()"));
        assert!(group.contains("mod.widgets.SegmentedControl = mod.widgets.SegmentedControlFlat{"));
        assert_eq!(group.matches("set_type_default() do mod.widgets.ButtonGroupBase").count(), 1);
        assert_eq!(group.matches("set_type_default() do mod.widgets.SegmentedControlBase").count(), 1);
    }
}

#[cfg(test)]
mod drawer_registration_tests {
    /// The drawer registers after the modal it is built on and the dialog
    /// it sits beside, with one type default and its two sheet presets.
    #[test]
    fn test_drawer_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let drawer = include_str!("drawer.rs");
        assert!(lib.contains("pub mod drawer;"));
        assert!(lib.contains("drawer::*"));
        let at = lib.find("crate::drawer::script_mod(vm);").expect("drawer registered");
        for base in [
            "crate::modal::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::label::script_mod(vm);",
        ] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before the drawer");
        }
        assert!(drawer.contains("mod.widgets.DrawerBase = #(Drawer::register_widget(vm))"));
        assert!(drawer.contains("mod.widgets.Drawer = set_type_default()"));
        assert!(drawer.contains("mod.widgets.BottomSheet = mod.widgets.Drawer{"));
        assert!(drawer.contains("mod.widgets.SideSheet = mod.widgets.Drawer{"));
        assert_eq!(drawer.matches("set_type_default() do mod.widgets.DrawerBase").count(), 1);
        // The widget derive takes any field type beginning with "Draw" for
        // a shader layer, which is why these two are not called DrawerSide
        // and DrawerSize.
        assert!(!drawer.contains("pub enum DrawerSide"));
        assert!(!drawer.contains("pub enum DrawerSize"));
    }
}

#[cfg(test)]
mod dialog_registration_tests {
    /// The dialog registers after the modal it is built on and the button
    /// and label its chrome uses, with one type default and the presets
    /// hanging off it.
    #[test]
    fn test_dialog_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let dialog = include_str!("dialog.rs");
        assert!(lib.contains("pub mod dialog;"));
        assert!(lib.contains("dialog::*"));
        let at = lib.find("crate::dialog::script_mod(vm);").expect("dialog registered");
        for base in [
            "crate::modal::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::label::script_mod(vm);",
        ] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before the dialog");
        }
        assert!(dialog.contains("mod.widgets.DialogBase = #(Dialog::register_widget(vm))"));
        assert!(dialog.contains("mod.widgets.Dialog = set_type_default()"));
        assert!(dialog.contains("mod.widgets.AlertDialog = mod.widgets.Dialog{"));
        assert!(dialog.contains("mod.widgets.ConfirmDialog = mod.widgets.Dialog{"));
        assert_eq!(dialog.matches("set_type_default() do mod.widgets.DialogBase").count(), 1);
    }
}

#[cfg(test)]
mod chip_registration_tests {
    /// The chip registers after the view, label and button modules whose
    /// shapes it borrows, and after the badge whose role palette it shares,
    /// with one type default at the flat rung of the ladder.
    #[test]
    fn test_chip_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let chip = include_str!("chip.rs");
        assert!(lib.contains("pub mod chip;"));
        assert!(lib.contains("chip::*"));
        let at = lib.find("crate::chip::script_mod(vm);").expect("chip registered");
        for base in [
            "crate::view::script_mod(vm);",
            "crate::label::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::badge::script_mod(vm);",
        ] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before chip");
        }
        assert!(chip.contains("mod.widgets.ChipBase = #(Chip::register_widget(vm))"));
        assert!(chip.contains("mod.widgets.ChipFlat = set_type_default()"));
        assert!(chip.contains("mod.widgets.Chip = mod.widgets.ChipFlat{"));
        assert!(chip.contains("mod.widgets.Tag = mod.widgets.ChipFlat{"));
        assert_eq!(chip.matches("set_type_default() do mod.widgets.ChipBase").count(), 1);
    }
}

#[cfg(test)]
mod breadcrumb_registration_tests {
    /// The trail registers after the view and label it draws with, and
    /// carries exactly one preset.
    #[test]
    fn test_breadcrumb_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let breadcrumb = include_str!("breadcrumb.rs");
        assert!(lib.contains("pub mod breadcrumb;"));
        assert!(lib.contains("breadcrumb::*"));
        let at = lib.find("crate::breadcrumb::script_mod(vm);").expect("breadcrumb registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before breadcrumb");
        }
        assert!(breadcrumb.contains("mod.widgets.BreadcrumbBase = #(Breadcrumb::register_widget(vm))"));
        assert_eq!(
            breadcrumb.matches("set_type_default() do mod.widgets.BreadcrumbBase").count(),
            1
        );
    }
}

#[cfg(test)]
mod marquee_registration_tests {
    /// The marquee registers after the view it draws inside and carries
    /// exactly one preset. It draws text and no children on purpose, so it
    /// must not grow a `#[deref] view` and start redrawing child widgets
    /// several times a frame — see the module's own reasoning.
    #[test]
    fn test_marquee_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let marquee = include_str!("marquee.rs");
        assert!(lib.contains("pub mod marquee;"));
        assert!(lib.contains("marquee::*"));
        let at = lib.find("crate::marquee::script_mod(vm);").expect("marquee registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before marquee");
        }
        assert!(marquee.contains("mod.widgets.MarqueeBase = #(Marquee::register_widget(vm))"));
        assert_eq!(marquee.matches("set_type_default() do mod.widgets.MarqueeBase").count(), 1);
    }
}

#[cfg(test)]
mod spinner_registration_tests {
    /// The spinner family registers after the button, label, view and
    /// glass modules it composes, and leaves `loading_spinner` untouched:
    /// that DSL-only view has shader parameters seven apps override by name.
    #[test]
    fn test_spinner_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let spinner = include_str!("spinner.rs");
        assert!(lib.contains("pub mod spinner;"));
        assert!(lib.contains("spinner::*"));
        let at = lib.find("crate::spinner::script_mod(vm);").expect("spinner registered");
        for base in [
            "crate::view::script_mod(vm);",
            "crate::label::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::gauss_view::script_mod(vm);",
            "crate::loading_spinner::script_mod(vm);",
        ] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before spinner");
        }
        assert!(spinner.contains("mod.widgets.SpinnerBase = #(Spinner::register_widget(vm))"));
        assert!(spinner.contains("mod.widgets.SpinnerFlat = set_type_default()"));
        assert_eq!(spinner.matches("set_type_default() do mod.widgets.SpinnerBase").count(), 1);
        assert!(!spinner.contains("mod.widgets.LoadingSpinner"));
    }
}

#[cfg(test)]
mod popover_registration_tests {
    /// The popover registers its base, its flat default and its bevelled
    /// variant, after the view and tip it builds on.
    #[test]
    fn test_popover_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let popover = include_str!("popover.rs");
        assert!(lib.contains("pub mod popover;"));
        assert!(lib.contains("popover::*"));
        assert!(lib.contains("crate::popover::script_mod(vm);"));
        let view_at = lib.find("crate::view::script_mod(vm);").unwrap();
        let tip_at = lib.find("crate::tip::script_mod(vm);").unwrap();
        let popover_at = lib.find("crate::popover::script_mod(vm);").unwrap();
        assert!(view_at < popover_at);
        assert!(tip_at < popover_at);
        assert!(popover.contains("mod.widgets.PopoverBase = #(Popover::register_widget(vm))"));
        assert!(popover.contains("mod.widgets.PopoverFlat = set_type_default() do mod.widgets.PopoverBase{"));
        assert!(popover.contains("mod.widgets.Popover = mod.widgets.PopoverFlat{"));
        assert_eq!(popover.matches("set_type_default() do mod.widgets.PopoverBase").count(), 1);
    }

    /// The presets build on the bevelled popover, the confirm popover has
    /// its own base and one default, and the hover card is what the info
    /// label is made of.
    #[test]
    fn test_popover_presets_and_confirm_are_registered() {
        let popover = include_str!("popover.rs");
        assert!(popover.contains("mod.widgets.PopoverArrow = mod.widgets.Popover{"));
        assert!(popover.contains("mod.widgets.PopoverHover = mod.widgets.Popover{"));
        assert!(popover.contains("mod.widgets.PopoverToggle = mod.widgets.Popover{"));
        assert!(popover.contains("mod.widgets.ConfirmPopoverBase = #(ConfirmPopover::register_widget(vm))"));
        assert!(popover.contains("mod.widgets.ConfirmPopover = set_type_default() do mod.widgets.ConfirmPopoverBase{"));
        assert_eq!(popover.matches("set_type_default() do mod.widgets.ConfirmPopoverBase").count(), 1);
        assert!(popover.contains("mod.widgets.InfoLabel = mod.widgets.PopoverHover{"));
        assert!(popover.contains("pub struct FocusTrap"));
    }
}

#[cfg(test)]
mod overlay_layers_registration_tests {
    /// The layer host registers after the tip layer it owns, and after
    /// every other widget: no registration follows it in `widgets_mod`.
    #[test]
    fn test_overlay_layers_is_registered_last() {
        let lib = include_str!("lib.rs");
        let layers = include_str!("overlay_layers.rs");
        assert!(lib.contains("pub mod overlay_layers;"));
        assert!(lib.contains("overlay_layers::*"));
        let call = "crate::overlay_layers::script_mod(vm);";
        let at = lib.find(call).unwrap();
        assert!(lib.find("crate::tip::script_mod(vm);").unwrap() < at);
        assert!(lib.find("crate::modal::script_mod(vm);").unwrap() < at);
        assert!(lib.find("crate::window::script_mod(vm);").unwrap() < at);
        // Nothing else registers between the host and the end of widgets_mod.
        let rest = &lib[at + call.len()..];
        let end = rest.find("pub fn script_mod(vm: &mut ScriptVm)").unwrap();
        assert!(!rest[..end].contains("::script_mod(vm);"));
        assert!(layers.contains("mod.widgets.OverlayLayersBase = #(OverlayLayers::register_widget(vm))"));
        assert!(layers.contains("mod.widgets.OverlayLayers = set_type_default() do mod.widgets.OverlayLayersBase{"));
        assert_eq!(layers.matches("set_type_default() do mod.widgets.OverlayLayersBase").count(), 1);
        assert!(layers.contains("tip_layer := TipLayer{}"));
    }
}
