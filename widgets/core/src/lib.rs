pub extern crate makepad_derive_widget;
pub extern crate makepad_draw;
pub use makepad_derive_widget::*;
pub use makepad_draw::makepad_platform;
pub use makepad_draw::*;
pub use makepad_platform::log;
pub use makepad_platform::makepad_script;
pub use makepad_script::script_eval;
pub use makepad_script::{ScriptValue, ScriptVm};

pub use makepad_draw::makepad_zune_jpeg;
pub use makepad_draw::makepad_zune_png;

// Core modules (used internally first)
pub mod animator;
pub mod font_policy;
pub mod desktop_style;
pub mod app_icon;
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
pub mod widget_hooks;
pub mod family_api;
pub mod test_cx;

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
pub mod toast;
pub mod breadcrumb;
pub mod button;
pub mod check_box;
pub mod icon;
pub mod image;
pub mod image_blend;
pub mod image_cache;
pub mod image_slice;
pub mod label;
pub mod link_label;
pub mod radio_button;

pub mod adaptive_view;
pub mod alert;
pub mod divider;
pub mod desktop_button;
pub mod gauss_view;
pub mod gauss_chain;
mod gauss_stack;
pub mod backdrop;
pub mod keyboard_view;
pub mod nav_control;
pub mod nav_list;
pub mod relief;
pub mod window;
pub mod cursor;
pub mod window_menu;

pub mod combo_box;
pub mod field_well;
pub mod drop_down;
pub mod drop_down2;
pub mod popup_menu;
pub mod number_field;
pub mod range_slider;
pub mod slider;
pub mod text_input;
pub mod drop_slider;
pub mod drop_toggles;
pub mod overlay_place;
pub mod tip;
pub mod popover;
pub mod value_input;
pub mod fab_controls;
pub mod menu_bar;

pub mod splitter;

pub mod fold_button;
pub mod fold_header;

pub mod loading_spinner;
pub mod progress;
pub mod playback_bar;
pub mod level_meter;
pub mod marquee;
pub mod spinner;

pub mod bare_step;
pub mod turtle_step;

pub mod rating;
pub mod tag_field;
pub mod radio_group;
pub mod kbd;
pub mod typography;
pub mod list_item;
pub mod item_selection;
pub mod avatar;
pub mod card;
pub mod media;
pub mod table;
pub mod empty_state;
pub mod timeline;
pub mod code_block;
pub mod carousel;
pub mod form;
pub mod column_picker;
pub mod picker_parts;
pub mod tree_select;
pub mod transfer;
pub mod property_inspector;
pub mod toolbar;
mod column_fit;
pub mod masonry;
pub mod tile_list;
pub mod item_grid;
pub mod kanban;
pub mod svg_select;
pub mod scroll_marks;
pub mod scroll_fade;
pub mod tour;
pub mod wheel_picker;
pub mod portal_list;
pub mod reorder_list;

pub mod cached_widget;
pub mod root;

pub mod tab;
pub mod tab_bar;
pub mod tabs;
pub mod tab_close_button;


pub mod splash;
pub mod svg;

// Touch gesture support (used by expandable_panel)
pub mod touch_gesture;

// Navigation and panels
pub mod expandable_panel;
pub mod scroll_shadow;
pub mod stack_navigation;

pub mod callout_tooltip;
pub mod floating_panel;
pub mod modal;
pub mod page_flip;
pub mod pagination;
pub mod placeholder;
pub mod hosted_view;
pub mod popup_notification;
pub mod slides_view;
pub mod tooltip;
pub mod video;

pub mod command_text_input;
pub mod defer_with_redraw;
pub mod slide_panel;

pub mod flat_list;

pub mod perf_graph;
pub mod corner_cap_view;
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
    field_well::*,
    desktop_button::*,

    drop_down::*,
    drop_down2::*,
    drop_toggles::*,
    overlay_place::*,
    popover::*,
    expandable_panel::*,
    flat_list::*,

    fold_button::*,
    fold_header::*,
    gauss_view::*,
    relief::*,
    grid::*,

    icon::*,

    image::*,
    image_blend::*,
    image_cache::*,
    image_slice::*,
    keyboard_view::*,
    // view_ui - no public exports
    label::*,
    link_label::*,
    menu_bar::*,
    floating_panel::*,
    modal::*,
    nav_control::*,
    nav_list::*,
    page_flip::*,
    pagination::*,
    hosted_view::*,
    popup_menu::*,
    popup_notification::*,
    rating::*,
    tag_field::*,
    radio_group::*,
    kbd::*,
    list_item::*,
    avatar::*,
    card::*,
    media::*,
    table::*,
    empty_state::*,
    timeline::*,
    code_block::*,
    carousel::*,
    form::*,
    column_picker::*,
    picker_parts::*,
    tree_select::*,
    transfer::*,
    property_inspector::*,
    toolbar::*,
    masonry::*,
    tile_list::*,
    item_grid::*,
    kanban::*,
    svg_select::*,
    scroll_marks::*,
    scroll_fade::*,
    tour::*,
    wheel_picker::*,
    portal_list::*,
    progress::*,
    playback_bar::*,
    level_meter::*,
    reorder_list::*,
    radio_button::*,
    root::*,

    rubber_view::*,
    // Ordered to match script_mod calls
    scroll_bar::ScrollBar,
    scroll_bars::{ScrollBars, ScrollExtent},
    scroll_shadow::*,
    slide_panel::*,
    number_field::*,
    range_slider::*,
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

    text_input::*,
    tooltip::*,
    // Navigation and panels
    touch_gesture::*,
    turtle_step::*,

    view::*,
    widget::{
        CreateAt, DrawStateWrap, DrawStep, DrawStepApi, OptionWidgetRefExt, SnapshotPart, Widget, WidgetAction,
        WidgetActionCast, WidgetActionCxExt, WidgetActionOptionApi, WidgetActionTrait,
        WidgetActionsApi, WidgetFactory, WidgetNode, WidgetRef, WidgetRegister, WidgetRegistry,
        WidgetSet, WidgetSetIterator, WidgetUid,
    },
    widget_async::{
        enter_isolate, leave_isolate, set_splash_theme, set_widget_async_trace, CxSplashVmExt,
        CxWidgetToScriptCallExt, IsolateEntry, ScriptAsyncCalls, ScriptAsyncId, ScriptAsyncResult,
        SplashTheme, SplashVmId, MAIN_SPLASH_VM_ID,
    },
    widget_match_event::WidgetMatchEvent,
    widget_tree::{set_ui_root, CxWidgetExt},
    widget_hooks::{WidgetHooks, WindowFamilies},

    window::*,

    window_menu::*,
};

















pub use crate::theme_tokens::*;

pub use crate::splash::*;

pub use crate::svg::*;

pub use crate::perf_graph::*;
pub use crate::item_selection::*;
pub use crate::corner_cap_view::*;
pub use crate::screen_cap::*;

pub use crate::video::*;

/// Which of the three themes the library is written in `theme_mod` leaves in
/// `mod.theme`. A `desktop_style` sheet is laid OVER one of these rather than
/// replacing it, so the two are separate choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BaseTheme {
    #[default]
    Dark,
    Light,
    Skeleton,
}

impl BaseTheme {
    pub const ALL: [Self; 3] = [Self::Dark, Self::Light, Self::Skeleton];
    /// What a settings file or a remote surface calls it.
    pub fn id(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Skeleton => "skeleton",
        }
    }
    /// What a picker shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Skeleton => "Skeleton",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.id() == s)
    }
}

/// One value, not one per heap: every heap's `theme_mod` has to end on the
/// same base or two windows of one app disagree about what light means.
/// `desktop_style::Styles` is per-heap because a sheet is installed into the
/// heap that evaluates it; this is a preference, and nothing about it is
/// heap-shaped.
#[derive(Default)]
struct BaseThemeChoice(BaseTheme);

/// The base theme `theme_mod` will emit. `Dark` until somebody says otherwise,
/// which is exactly what the module hard-coded before there was a choice.
pub fn base_theme(cx: &mut Cx) -> BaseTheme {
    cx.global::<BaseThemeChoice>().0
}

/// Choose the base theme. It is read by `theme_mod`, so it takes effect on the
/// next `script_mod` run and no sooner: a caller switching a running app
/// follows this with `cx.request_style_reload()`, which re-runs `script_mod`
/// and then re-applies the tree with `Apply::ScriptReapply` (typed text and
/// running animations survive, which `Apply::Reload` would not).
pub fn set_base_theme(cx: &mut Cx, theme: BaseTheme) {
    cx.global::<BaseThemeChoice>().0 = theme;
}

/// The `widgets/` directory: the front crate `makepad-widgets`, whose
/// `themes/` and `resources/` belong to the whole library. This crate lives
/// in `widgets/core`, so it is the parent of this manifest.
pub fn widgets_dir() -> &'static str {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let core = env!("CARGO_MANIFEST_DIR").trim_start_matches(r"\\?\");
        std::path::Path::new(core)
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_else(|| core.to_string())
    })
}

/// Resources are named `makepad_widgets/resources/...` (the font policy,
/// the packagers, `crate_resource("makepad_widgets:...")`), which resolves
/// through the manifest registered for `makepad_widgets`. The script modules
/// that register one by being evaluated are all in `makepad_widgets_core`
/// and the family crates now, so the name is registered here, before the
/// theme's fonts ask for it.
fn register_widgets_resource_crate(vm: &mut ScriptVm) {
    vm.bx
        .code
        .crate_manifests
        .borrow_mut()
        .insert("makepad_widgets".to_string(), widgets_dir().to_string());
}

pub fn theme_mod(vm: &mut ScriptVm) {
    register_widgets_resource_crate(vm);
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
    });
    // The base theme, last, so everything after it reads the one that was
    // chosen. `Dark` is the default, so an app that never calls
    // `set_base_theme` gets precisely what this used to say outright.
    match base_theme(vm.cx_mut()) {
        BaseTheme::Dark => {
            script_eval!(vm, {
                mod.theme = mod.themes.dark
            });
        }
        BaseTheme::Light => {
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
        }
        BaseTheme::Skeleton => {
            script_eval!(vm, {
                mod.theme = mod.themes.skeleton
            });
        }
    }
}

/// The core's widgets alone. `makepad_widgets::widgets_mod` is the one an
/// app calls: it passes the families its features picked.
pub fn widgets_mod(vm: &mut ScriptVm) {
    widgets_mod_with(vm, WindowFamilies::default(), &[]);
}

/// The core's widgets, `window`'s families where the Window names them, and
/// then `families` in order, before the prelude is made from `mod.widgets`.
pub fn widgets_mod_with(
    vm: &mut ScriptVm,
    window: WindowFamilies,
    families: &[fn(&mut ScriptVm)],
) {
    let host_io_only = vm.cx().script_data.std.host_io_only();
    widgets_mod_with_io(vm, window, families, host_io_only);
}

/// The core's widgets for a Splash isolate being built: `host_io_only` is
/// the restriction it will run under (its std is restricted only after this).
pub(crate) fn widgets_mod_with_host_io(vm: &mut ScriptVm, host_io_only: bool) {
    widgets_mod_with_io(vm, WindowFamilies::default(), &[], host_io_only);
}

/// With `host_io_only` (a guest held to the host service bridge) the modules
/// that reach past it stay out: the window and its menu bar (a guest must not
/// replace the app's), screen capture, and cached widgets (their singletons
/// would reach widgets the host or other isolates cached).
fn widgets_mod_with_io(
    vm: &mut ScriptVm,
    window: WindowFamilies,
    families: &[fn(&mut ScriptVm)],
    host_io_only: bool,
) {
    crate::desktop_style::apply_theme(vm);
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
    crate::relief::script_mod(vm);
    crate::grid::script_mod(vm);
    crate::rubber_view::script_mod(vm);

    crate::label::script_mod(vm);
    crate::link_label::script_mod(vm);
    crate::button::script_mod(vm);
    crate::alert::script_mod(vm);
    crate::divider::script_mod(vm);
    crate::check_box::script_mod(vm);
    crate::radio_button::script_mod(vm);
    crate::image::script_mod(vm);
    crate::animated_image_gif::script_mod(vm);
    crate::image_blend::script_mod(vm);
    crate::icon::script_mod(vm);

    crate::adaptive_view::script_mod(vm);
    crate::desktop_button::script_mod(vm);
    crate::keyboard_view::script_mod(vm);
    // Every Window names a `VoiceWave`, a `Tweaker` and an `AiChatSlot`,
    // so their families register here; without one the name is an empty
    // hidden view.
    match window.voice {
        Some(register) => register(vm),
        None => {
            script_eval!(vm, {
                use mod.widgets.View
                mod.widgets.VoiceWave = mod.widgets.View {
                    visible: false
                }
            });
        }
    }
    if !host_io_only { crate::window_menu::script_mod(vm); }
    crate::nav_control::script_mod(vm);
    match window.tweaker {
        Some(register) => register(vm),
        None => {
            script_eval!(vm, {
                use mod.widgets.View
                mod.widgets.Tweaker = mod.widgets.View {
                    visible: false
                }
            });
        }
    }
    crate::gauss_view::script_mod(vm);
    if !host_io_only { crate::screen_cap::script_mod(vm); }
    // The AI slot before the window: its DSL names `AiChatSlot`.
    match window.ai {
        Some(register) => register(vm),
        None => {
            script_eval!(vm, {
                use mod.widgets.View
                mod.widgets.AiChatSlot = mod.widgets.View {
                    visible: false
                }
            });
        }
    }
    crate::app_icon::script_mod(vm);
    crate::cursor::script_mod(vm);
    if !host_io_only { crate::window::script_mod(vm); }

    crate::popup_menu::script_mod(vm);
    crate::drop_down::script_mod(vm);
    crate::drop_down2::script_mod(vm);
    crate::text_input::script_mod(vm);
    crate::slider::script_mod(vm);
    crate::range_slider::script_mod(vm);
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
    crate::field_well::script_mod(vm);
    crate::number_field::script_mod(vm);

    crate::splitter::script_mod(vm);

    crate::fold_button::script_mod(vm);
    crate::fold_header::script_mod(vm);
    crate::accordion::script_mod(vm);

    crate::loading_spinner::script_mod(vm);
    crate::progress::script_mod(vm);
    crate::playback_bar::script_mod(vm);
    crate::level_meter::script_mod(vm);
    crate::breadcrumb::script_mod(vm);
    crate::pagination::script_mod(vm);
    crate::nav_list::script_mod(vm);
    crate::floating_panel::script_mod(vm);
    crate::marquee::script_mod(vm);
    crate::spinner::script_mod(vm);
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

    crate::portal_list::script_mod(vm);
    crate::rating::script_mod(vm);
    crate::tag_field::script_mod(vm);
    crate::radio_group::script_mod(vm);
    crate::kbd::script_mod(vm);
    crate::wheel_picker::script_mod(vm);
    crate::typography::script_mod(vm);
    crate::list_item::script_mod(vm);
    crate::avatar::script_mod(vm);
    crate::card::script_mod(vm);
    crate::media::script_mod(vm);
    crate::table::script_mod(vm);
    crate::empty_state::script_mod(vm);
    crate::timeline::script_mod(vm);
    crate::code_block::script_mod(vm);
    crate::carousel::script_mod(vm);
    crate::form::script_mod(vm);
    // Before the three that draw with its panel and row surfaces.
    crate::picker_parts::script_mod(vm);
    crate::column_picker::script_mod(vm);
    crate::tree_select::script_mod(vm);
    crate::transfer::script_mod(vm);
    crate::property_inspector::script_mod(vm);
    crate::tour::script_mod(vm);
    crate::toolbar::script_mod(vm);
    crate::masonry::script_mod(vm);
    crate::tile_list::script_mod(vm);
    crate::item_grid::script_mod(vm);
    crate::kanban::script_mod(vm);
    crate::scroll_marks::script_mod(vm);
    crate::scroll_fade::script_mod(vm);
    crate::svg_select::script_mod(vm);
    crate::reorder_list::script_mod(vm);

    if !host_io_only { crate::cached_widget::script_mod(vm); }
    crate::root::script_mod(vm);

    crate::tab_close_button::script_mod(vm);
    crate::tab::script_mod(vm);
    crate::tab_bar::script_mod(vm);
    crate::tabs::script_mod(vm);

    // Navigation and panels
    crate::scroll_shadow::script_mod(vm);
    crate::stack_navigation::script_mod(vm);
    crate::expandable_panel::script_mod(vm);
    crate::modal::script_mod(vm);
    crate::dialog::script_mod(vm);
    crate::tooltip::script_mod(vm);
    crate::callout_tooltip::script_mod(vm);
    crate::popup_notification::script_mod(vm);
    crate::video::script_mod(vm);
    crate::page_flip::script_mod(vm);
    crate::hosted_view::script_mod(vm);
    crate::flat_list::script_mod(vm);
    crate::slides_view::script_mod(vm);
    crate::slide_panel::script_mod(vm);


    crate::splash::script_mod(vm);
    crate::svg::script_mod(vm);
    crate::perf_graph::script_mod(vm);
    crate::corner_cap_view::script_mod(vm);

    // The families, after every core widget they may build on.
    for register in families {
        register(vm);
    }

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
    script_mod_with(vm, WindowFamilies::default(), &[]);
}

/// The theme, the widgets with the given families, and the desktop style
/// laid over them: what `makepad_widgets::script_mod` runs.
pub fn script_mod_with(
    vm: &mut ScriptVm,
    window: WindowFamilies,
    families: &[fn(&mut ScriptVm)],
) {
    makepad_platform::startup_trace("widgets: theme_mod begin");
    theme_mod(vm);
    makepad_platform::startup_trace("widgets: theme_mod done");
    widgets_mod_with(vm, window, families);
    crate::desktop_style::apply_widgets(vm);
    makepad_platform::startup_trace("widgets: widgets_mod done");
}

// The pooled test contexts (test_cx.rs), registered with the core alone.
#[cfg(test)]
pub(crate) use crate::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    crate::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    crate::test_cx::on_test_cx(f)
}

#[cfg(test)]
mod base_theme_tests {

    use super::*;

    /// The base theme is a choice now, and `dark` is still what an app that
    /// never makes one gets.
    #[test]
    fn theme_mod_emits_the_chosen_base_and_still_defaults_to_dark() {
        fn bg(vm: &mut ScriptVm) -> Option<u32> {
            let theme = vm.module(id!(theme));
            vm.bx
                .heap
                .value(theme, id!(color_bg_app).into(), NoTrap)
                .as_color()
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let dark = bg(vm);
            assert!(dark.is_some());
            assert_eq!(base_theme(vm.cx_mut()), BaseTheme::Dark);

            set_base_theme(vm.cx_mut(), BaseTheme::Light);
            vm.with_reload(crate::script_mod);
            let light = bg(vm);
            assert!(light.is_some() && light != dark, "{light:?} vs {dark:?}");
            assert!(vm.take_errors().is_empty());

            set_base_theme(vm.cx_mut(), BaseTheme::Skeleton);
            vm.with_reload(crate::script_mod);
            let skeleton = bg(vm);
            assert!(skeleton.is_some());
            assert!(vm.take_errors().is_empty());

            // And back: the choice is a setting, not a one-way door.
            set_base_theme(vm.cx_mut(), BaseTheme::Dark);
            vm.with_reload(crate::script_mod);
            assert_eq!(bg(vm), dark);
            assert!(vm.take_errors().is_empty());
        });
    }

    #[test]
    fn every_base_theme_has_a_name_and_parses_back() {
        for base in BaseTheme::ALL {
            assert_eq!(BaseTheme::parse(base.id()), Some(base));
            assert!(!base.label().is_empty());
        }
        assert_eq!(BaseTheme::parse("omarchy"), None);
        assert_eq!(BaseTheme::default(), BaseTheme::Dark);
    }
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
        // The drawer and the two sheets are the dialog on an edge: presets
        // of it, with no type and so no type default of their own.
        assert!(dialog.contains("mod.widgets.Drawer = mod.widgets.Dialog{"));
        assert!(dialog.contains("mod.widgets.SideSheet = mod.widgets.Drawer{"));
        assert!(dialog.contains("mod.widgets.BottomSheet = mod.widgets.Drawer{"));
        // The widget derive takes any field type beginning with "Draw" for
        // a shader layer, which is why the side is not called DrawerSide.
        assert!(!dialog.contains("pub enum DrawerSide"));
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
mod nav_list_registration_tests {
    /// The nav list registers after the radio button its rows must be
    /// shaped like, and carries exactly one preset plus the two flows.
    #[test]
    fn test_nav_list_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let nav = include_str!("nav_list.rs");
        assert!(lib.contains("pub mod nav_list;"));
        assert!(lib.contains("nav_list::*"));
        let at = lib.find("crate::nav_list::script_mod(vm);").expect("nav_list registered");
        for base in ["crate::view::script_mod(vm);", "crate::radio_button::script_mod(vm);"] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before nav_list");
        }
        assert!(nav.contains("mod.widgets.NavListBase = #(NavList::register_widget(vm))"));
        assert_eq!(nav.matches("set_type_default() do mod.widgets.NavListBase").count(), 1);
        // A rail and a bar are presets over the one list, not two widgets.
        assert!(nav.contains("mod.widgets.NavRail = mod.widgets.NavList{"));
        assert!(nav.contains("mod.widgets.NavBar = mod.widgets.NavList{"));
    }
}

#[cfg(test)]
mod pagination_registration_tests {
    /// The strip registers after the view and label it draws with, carries
    /// exactly one preset, and keeps its arithmetic a free function: the
    /// window is what the tests are about, the drawing is not.
    #[test]
    fn test_pagination_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let pagination = include_str!("pagination.rs");
        assert!(lib.contains("pub mod pagination;"));
        assert!(lib.contains("pagination::*"));
        let at = lib.find("crate::pagination::script_mod(vm);").expect("pagination registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(lib.find(base).expect(base) < at, "{base} must register before pagination");
        }
        assert!(pagination.contains("mod.widgets.PaginationBase = #(Pagination::register_widget(vm))"));
        assert_eq!(
            pagination.matches("set_type_default() do mod.widgets.PaginationBase").count(),
            1
        );
        assert!(pagination.contains("pub fn page_window("), "the arithmetic stays a free function");
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

    /// The presets build on the bevelled popover, and the confirm popover
    /// has its own base and one default.
    #[test]
    fn test_popover_presets_and_confirm_are_registered() {
        let popover = include_str!("popover.rs");
        assert!(popover.contains("mod.widgets.PopoverArrow = mod.widgets.Popover{"));
        assert!(popover.contains("mod.widgets.PopoverHover = mod.widgets.Popover{"));
        assert!(popover.contains("mod.widgets.PopoverToggle = mod.widgets.Popover{"));
        assert!(popover.contains("mod.widgets.ConfirmPopoverBase = #(ConfirmPopover::register_widget(vm))"));
        assert!(popover.contains("mod.widgets.ConfirmPopover = set_type_default() do mod.widgets.ConfirmPopoverBase{"));
        assert_eq!(popover.matches("set_type_default() do mod.widgets.ConfirmPopoverBase").count(), 1);
        assert!(popover.contains("pub struct FocusTrap"));
    }
}

/// The text of `widgets_mod`, where registration order is decided. The new
/// widgets' tests read order from here rather than from the whole file: the
/// tests themselves name every call, so a search of the whole file would
/// still find a call that had gone missing from the function.
#[cfg(test)]
fn widgets_mod_source() -> &'static str {
    let lib = include_str!("lib.rs");
    let start = lib.find("pub fn widgets_mod(vm: &mut ScriptVm)").expect("widgets_mod");
    let end = lib.find("pub fn script_mod(vm: &mut ScriptVm)").expect("script_mod");
    &lib[start..end]
}

#[cfg(test)]
mod image_slice_registration_tests {
    /// The slicing arithmetic is a module of its own beside the image cache
    /// whose fit it extends. It has no script module: its enums are exported
    /// to the DSL by the image widget, next to `ImageFit`.
    #[test]
    fn test_image_slice_is_a_module_without_a_registration() {
        let lib = include_str!("lib.rs");
        assert!(lib.contains("\npub mod image_slice;"));
        assert!(lib.contains("\n    image_slice::*,"));
        assert!(!crate::widgets_mod_source().contains("crate::image_slice::"));
    }
}
