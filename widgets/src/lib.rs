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
pub use makepad_tween;
#[cfg(feature = "pdf")]
pub use makepad_pdf_parse;

pub use makepad_draw::makepad_zune_jpeg;
pub use makepad_draw::makepad_zune_png;

// Core modules (used internally first)
pub mod animator;
pub mod tween;
pub mod tween_script;
pub mod font_policy;
pub mod desktop_style;
pub mod app_icon;
pub mod theme_desktop_dark;
pub mod theme_desktop_light;
pub mod theme_desktop_skeleton;
pub mod theme_tokens;
pub mod theme_store;
pub mod theme_lab;
pub mod conceding_row;
pub mod widget;
pub mod width_override;
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
pub mod pill_nav;
pub mod chip;
pub mod menu;
pub mod select;
pub mod accordion;
pub mod dialog;
pub mod toast;
pub mod breadcrumb;
pub mod browser;
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
pub mod tweaker;
pub mod designer;
pub mod reflect;
pub mod ai_slot;
#[cfg(feature = "voice")]
pub mod voice_wave;
pub mod window;
pub mod cursor;
pub mod window_menu;
#[cfg(feature = "voice")]
mod window_voice_input;

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
pub mod overlay_layers;
pub mod value_input;
pub mod ease_editor;
pub mod fab_controls;
pub mod menu_bar;

pub mod splitter;

pub mod fold_button;
pub mod fold_header;

pub mod glass_panel;
pub mod loading_spinner;
pub mod progress;
pub mod playback_bar;
pub mod level_meter;
pub mod marquee;
pub mod spinner;

pub mod bare_step;
pub mod turtle_step;

pub mod data_grid;
pub mod calendar;
pub mod date_picker;
pub mod time_picker;
pub mod rating;
pub mod tag_field;
pub mod radio_group;
pub mod kbd;
pub mod hotkeys;
pub mod hotkey_editor;
pub mod keyboard_map;
pub mod typography;
pub mod tree;
pub mod list_item;
pub mod item_selection;
pub mod avatar;
pub mod card;
pub mod media;
pub mod table;
pub mod empty_state;
pub mod timeline;
pub mod waveform;
pub mod chat;
pub mod code_block;
pub mod carousel;
pub mod dropzone;
pub mod form;
pub mod color;
pub mod gradient_editor;
pub mod column_picker;
pub mod picker_parts;
pub mod tree_select;
pub mod transfer;
pub mod command_palette;
pub mod radial_menu;
pub mod floating_action;
pub mod hamburger_menu;
pub mod property_inspector;
pub mod chart_shapes;
pub mod gizmo;
pub mod toolbar;
mod column_fit;
pub mod masonry;
pub mod tile_list;
pub mod item_grid;
pub mod kanban;
pub mod svg_select;
pub mod rich_text;
pub mod scroll_marks;
pub mod scroll_fade;
pub mod line_menu;
pub mod tour;
pub mod wheel_picker;
pub mod portal_list;
pub mod reorder_list;
pub mod text_flow;
pub mod log_list;

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

pub mod chart;
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
    pill_nav::*,
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
    data_grid::*,
    calendar::*,
    date_picker::*,
    time_picker::*,
    rating::*,
    tag_field::*,
    radio_group::*,
    kbd::*,
    hotkeys::*,
    hotkey_editor::*,
    keyboard_map::*,
    typography::*,
    tree::*,
    list_item::*,
    avatar::*,
    card::*,
    media::*,
    table::*,
    empty_state::*,
    timeline::*,
    waveform::*,
    chat::*,
    code_block::*,
    carousel::*,
    dropzone::*,
    form::*,
    color::*,
    gradient_editor::*,
    column_picker::*,
    picker_parts::*,
    tree_select::*,
    transfer::*,
    command_palette::*,
    radial_menu::*,
    floating_action::*,
    hamburger_menu::*,
    property_inspector::*,
    chart_shapes::*,
    toolbar::*,
    masonry::*,
    tile_list::*,
    item_grid::*,
    kanban::*,
    svg_select::*,
    rich_text::*,
    scroll_marks::*,
    scroll_fade::*,
    line_menu::*,
    tour::*,
    wheel_picker::*,
    portal_list::*,
    progress::*,
    playback_bar::*,
    level_meter::*,
    reorder_list::*,
    radio_button::*,
    reflect::*,
    root::*,

    rubber_view::*,
    // Ordered to match script_mod calls
    scroll_bar::ScrollBar,
    scroll_bars::{ScrollBars, ScrollExtent},
    scroll_shadow::*,
    slide_panel::*,
    number_field::*,
    ease_editor::*,
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
    text_flow::*,
    log_list::*,

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

/// The blend `theme_mod` emits over the base theme, whole, as the script that
/// makes it current. `None` is the ordinary case: no mix, and `theme_mod` ends
/// on the base theme exactly as it did before there was a mix.
///
/// On the Cx and not in the module, for the same reason the base theme is: a
/// module run is what would otherwise lose it. Evaluating a blend once into
/// `mod.theme` moves the library's own theme and nothing an APP built off it,
/// because an app's templates bake `theme.color_x` into a literal when the
/// app's own module block runs, and only re-running THAT rebuilds them -- which
/// is `cx.request_style_reload()`, which is a module run, which would throw the
/// blend away again. Held here it survives, and the reload that makes an app
/// wear the mix is the same reload that re-emits it.
#[derive(Default)]
struct ThemeMixChoice(Option<String>);

/// The blend in force, if any: the script `theme_mod` re-emits on every run.
pub fn theme_mix(cx: &mut Cx) -> Option<String> {
    cx.global::<ThemeMixChoice>().0.clone()
}

/// Put a blend in force, or take the standing one off with `None`.
///
/// Read by `theme_mod`, so like `set_base_theme` it takes effect on the next
/// module run and no sooner: a caller follows it with
/// `cx.request_style_reload()`. That is not a detail of this call -- it is the
/// whole point of it. See [`crate::theme_lab::ThemeLab::apply`].
pub fn set_theme_mix(cx: &mut Cx, code: Option<String>) {
    cx.global::<ThemeMixChoice>().0 = code;
}

/// A person's own edits to the theme in force, token by token, as the text
/// each was set to. Re-emitted on every module run, after the base theme,
/// the sheet and the mix, so an edit outlives the rebuild that used to
/// forget it: the heap held the value, and the next reload put the theme
/// file's own back.
#[derive(Default)]
struct ThemeEdits(std::collections::BTreeMap<String, String>);

/// The edits standing over the theme, in token order.
pub fn theme_edits(cx: &mut Cx) -> Vec<(String, String)> {
    cx.global::<ThemeEdits>().0.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Put one edit in force, or take it off with `None`. Like the mix it is
/// read on the next module run: a caller follows it with
/// `cx.request_style_reload()`, behind whatever settle its gesture wants.
pub fn set_theme_edit(cx: &mut Cx, name: &str, text: Option<String>) {
    let edits = &mut cx.global::<ThemeEdits>().0;
    match text {
        Some(text) => {
            edits.insert(name.to_string(), text);
        }
        None => {
            edits.remove(name);
        }
    }
}

/// Every edit off. Picking a theme does this: the edits were made to the
/// theme that was up, and would otherwise pin its values over the next.
pub fn clear_theme_edits(cx: &mut Cx) {
    cx.global::<ThemeEdits>().0.clear();
}

/// Choose the base theme. It is read by `theme_mod`, so it takes effect on the
/// next `script_mod` run and no sooner: a caller switching a running app
/// follows this with `cx.request_style_reload()`, which re-runs `script_mod`
/// and then re-applies the tree with `Apply::ScriptReapply` (typed text and
/// running animations survive, which `Apply::Reload` would not).
///
/// A standing mix comes off. Picking a theme is somebody saying which theme
/// the app should be wearing, and a mix stands over the whole app rather than
/// over the panel that made it -- so the pick wins. Without this the blend
/// `theme_mod` re-emits on every run would go straight back over the top of
/// the theme just chosen, and a picker that had worked for years would sit
/// there doing nothing for as long as a mix was up.
///
/// The lab notices it has been stood down and opens again on the theme now in
/// force; see [`crate::theme_lab::ThemeLab::apply`]. The one caller that must
/// not be caught by this is the lab's own install, which sets no base theme --
/// it writes the blend and asks for the reload, and nothing else.
pub fn set_base_theme(cx: &mut Cx, theme: BaseTheme) {
    cx.global::<BaseThemeChoice>().0 = theme;
    set_theme_mix(cx, None);
    clear_theme_edits(cx);
}

pub fn theme_mod(vm: &mut ScriptVm) {
    makepad_draw::script_mod(vm);
    if !vm.is_reload() {
        makepad_platform::ime::script_mod(vm);
    }

    vm.bx.heap.new_module(id!(prelude));
    vm.bx.heap.new_module(id!(themes));
    crate::animator::script_mod(vm);
    // `mod.tween` (GSAP-style tweens from script), once per VM.
    crate::tween_script::script_mod(vm);
    crate::theme_desktop_dark::script_mod(vm);
    crate::theme_desktop_light::script_mod(vm);
    crate::theme_desktop_skeleton::script_mod(vm);
    // A person's edit to a GLOBAL -- `space_factor`, `font_size_base` --
    // builds the base theme again from its own source with that literal in
    // place, so every rung derived from it moves. Under the base's own name:
    // the `mod.theme` below and a sheet's first line both point at it by
    // name. The edits go on again later as pins, in `widgets_mod`; for a
    // global that is nothing, and for one a base's file does not spell out
    // it is the only way the value lands at all.
    let globals: Vec<(String, crate::theme_tokens::TokenValue)> = theme_edits(vm.cx_mut())
        .into_iter()
        .filter(|(name, _)| crate::theme_tokens::is_global_key(name))
        .map(|(name, text)| (name, crate::theme_tokens::TokenValue::Raw(text)))
        .collect();
    if !globals.is_empty() {
        let scheme = match base_theme(vm.cx_mut()) {
            BaseTheme::Dark => crate::theme_tokens::Scheme::Dark,
            BaseTheme::Light => crate::theme_tokens::Scheme::Light,
            BaseTheme::Skeleton => crate::theme_tokens::Scheme::Skeleton,
        };
        vm.eval(makepad_platform::ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
            module_path: "theme_globals".to_string(),
            file: format!("theme_{}_rederived.splash", scheme.theme_name()),
            line: 0,
            column: 0,
            code: crate::theme_tokens::theme_rederived_script(scheme, &globals),
            values: vec![],
        });
    }
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
            tween:mod.tween,
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
    // ...and the blend over it, if one is in force. Here, and not in
    // `widgets_mod`, because this is the seam a mix is seen from: after the
    // themes exist, and before a widget template bakes `theme.color_x` into a
    // literal it will not evaluate again.
    if let Some(code) = theme_mix(vm.cx_mut()) {
        vm.eval(makepad_platform::ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
            module_path: "theme_lab".to_string(),
            file: format!("{}.splash", crate::theme_lab::MIX_NAME),
            line: 0,
            column: 0,
            code,
            values: vec![],
        });
    }
}

pub fn widgets_mod(vm: &mut ScriptVm) {
    let host_io_only = vm.cx().script_data.std.host_io_only();
    widgets_mod_with_host_io(vm, host_io_only);
}

pub(crate) fn widgets_mod_with_host_io(vm: &mut ScriptVm, host_io_only: bool) {
    crate::desktop_style::apply_theme(vm);
    // ...and the person's own edits over everything -- base, sheet or mix.
    // (A global has already rebuilt the base in `theme_mod`; its pin here
    // is the same value again, and the belt for a base that never named it.)
    // Here and not beside the mix: a sheet's first line points `mod.theme`
    // at its own base, so an edit written before `apply_theme` is gone by
    // now; and before the prelude captures `theme: mod.theme` for every
    // template, which is the last moment a token is still a token.
    let edits = theme_edits(vm.cx_mut());
    if !edits.is_empty() {
        let mut code = String::from("mod.themes.edited = mod.theme{");
        for (name, text) in &edits {
            code.push(' ');
            code.push_str(name);
            code.push_str(": ");
            code.push_str(text);
        }
        // The last statement of a script is swallowed; `true` takes the fall.
        code.push_str(" }
mod.theme = mod.themes.edited
true
");
        vm.eval(makepad_platform::ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
            module_path: "theme_edits".to_string(),
            file: "theme_edits.splash".to_string(),
            line: 0,
            column: 0,
            code,
            values: vec![],
        });
    }
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
    if !host_io_only { crate::browser::script_mod(vm); }
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
    // A guest must not replace the app's own menu bar.
    if !host_io_only { crate::window_menu::script_mod(vm); }
    crate::nav_control::script_mod(vm);
    crate::tweaker::script_mod(vm);
    crate::gauss_view::script_mod(vm);
    if !host_io_only { crate::screen_cap::script_mod(vm); }
    // The AI slot before the window: its DSL names `AiChatSlot`.
    crate::ai_slot::script_mod(vm);
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
    crate::ease_editor::script_mod(vm);
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
    crate::glass_panel::script_mod(vm);
    crate::chip::script_mod(vm);
    // The menu first: the group's split and menu buttons carry a
    // `MenuPlace`, and a block's `use` only sees what already exists.
    crate::menu::script_mod(vm);
    crate::button_group::script_mod(vm);
    // After the popover, whose trigger and placement enums it takes.
    crate::pill_nav::script_mod(vm);
    crate::select::script_mod(vm);
    crate::toast::script_mod(vm);
    crate::placeholder::script_mod(vm);
    // Only needs a View to derive from; the ladder it uses is Rust.
    crate::conceding_row::script_mod(vm);

    crate::bare_step::script_mod(vm);
    crate::turtle_step::script_mod(vm);

    crate::data_grid::script_mod(vm);
    crate::portal_list::script_mod(vm);
    crate::calendar::script_mod(vm);
    crate::date_picker::script_mod(vm);
    crate::time_picker::script_mod(vm);
    crate::rating::script_mod(vm);
    crate::tag_field::script_mod(vm);
    crate::radio_group::script_mod(vm);
    crate::kbd::script_mod(vm);
    crate::wheel_picker::script_mod(vm);
    crate::typography::script_mod(vm);
    crate::tree::script_mod(vm);
    crate::list_item::script_mod(vm);
    crate::avatar::script_mod(vm);
    crate::card::script_mod(vm);
    crate::media::script_mod(vm);
    crate::table::script_mod(vm);
    crate::empty_state::script_mod(vm);
    crate::timeline::script_mod(vm);
    crate::waveform::script_mod(vm);
    crate::chat::script_mod(vm);
    crate::code_block::script_mod(vm);
    crate::carousel::script_mod(vm);
    crate::dropzone::script_mod(vm);
    crate::form::script_mod(vm);
    crate::color::script_mod(vm);
    crate::gradient_editor::script_mod(vm);
    // Before the three that draw with its panel and row surfaces.
    crate::picker_parts::script_mod(vm);
    crate::column_picker::script_mod(vm);
    crate::tree_select::script_mod(vm);
    crate::transfer::script_mod(vm);
    crate::command_palette::script_mod(vm);
    // After TextInput, Button, KbdGroup and the tip they build on.
    crate::hotkey_editor::script_mod(vm);
    crate::keyboard_map::script_mod(vm);
    crate::radial_menu::script_mod(vm);
    crate::property_inspector::script_mod(vm);
    crate::tour::script_mod(vm);
    crate::chart_shapes::script_mod(vm);
    crate::gizmo::script_mod(vm);
    crate::toolbar::script_mod(vm);
    crate::floating_action::script_mod(vm);
    crate::masonry::script_mod(vm);
    crate::tile_list::script_mod(vm);
    crate::item_grid::script_mod(vm);
    crate::kanban::script_mod(vm);
    crate::scroll_marks::script_mod(vm);
    crate::scroll_fade::script_mod(vm);
    // After the pill nav, whose surface shader draws its card, and the nav
    // list, whose ground is its hit rect.
    crate::line_menu::script_mod(vm);
    crate::svg_select::script_mod(vm);
    crate::rich_text::script_mod(vm);
    crate::reorder_list::script_mod(vm);
    crate::text_flow::script_mod(vm);
    crate::log_list::script_mod(vm);

    // Its singletons would let a guest reach widgets the host or other isolates cached.
    if !host_io_only { crate::cached_widget::script_mod(vm); }
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
    // After the dialog: the menu is built from a drawer, which is a dialog
    // on an edge, a popover, a nav list and a burger button, and this is
    // the last of the four to land.
    crate::hamburger_menu::script_mod(vm);
    crate::tooltip::script_mod(vm);
    crate::callout_tooltip::script_mod(vm);
    crate::popup_notification::script_mod(vm);
    crate::video::script_mod(vm);
    crate::page_flip::script_mod(vm);
    crate::hosted_view::script_mod(vm);
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
    crate::corner_cap_view::script_mod(vm);
    #[cfg(feature = "maps")]
    crate::map::style::script_mod(vm);
    #[cfg(feature = "maps")]
    if !host_io_only { crate::map::view::script_mod(vm); }
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
    crate::desktop_style::apply_widgets(vm);
    makepad_platform::startup_trace("widgets: widgets_mod done");
}

#[cfg(test)]
mod module_rebuild_tests {
    use super::*;

    /// A module rebuild -- a theme switch, a live edit, every settle of a
    /// drag on the theme lab -- must leave the live heap where it found it.
    /// It did not: every run registered every type into a fresh slot, and
    /// each slot rooted its template tree, so a rebuild kept some thousands
    /// of objects alive for good. Measured through the collector's own
    /// count, after a collection, so garbage the collector would take
    /// anyway is not mistaken for a leak.
    #[test]
    fn a_module_rebuild_leaves_the_live_heap_where_it_was() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        set_base_theme(&mut cx, BaseTheme::Dark);
        cx.with_vm(|vm| script_mod(vm));
        let live_after = |cx: &mut Cx| {
            cx.with_vm(|vm| {
                vm.with_reload(script_mod);
                vm.gc();
                (vm.bx.heap.gc_live_len(), vm.bx.heap.root_counts())
            })
        };
        // Two runs to settle: the first reload pays for what the first run
        // only lazily made.
        live_after(&mut cx);
        let (mut live, roots) = live_after(&mut cx);
        let named = ["type slots", "type defaults", "pod types", "object refs", "array refs", "handles"];
        let mut steps = Vec::new();
        for _ in 0..5 {
            let (now, roots_now) = live_after(&mut cx);
            // Every root but the pod types holds still. Those still grow, by
            // the vertex structs a run spells out, and the obvious fix -- an
            // equal slot taking the new definition over -- is not open: a
            // compiled shader holds a pod type INDEX, so handing a slot back
            // to the free list hands a later struct an index some shader is
            // still reading its geometry through, and a quad stops finding
            // its own `pos`. It waits for a way to retire a slot that knows
            // who is still holding it.
            for (i, name) in named.iter().enumerate() {
                if i == 2 {
                    continue;
                }
                assert_eq!(roots_now[i], roots[i], "a rebuild grew a collector root: {name}");
            }
            steps.push(now as i64 - live as i64);
            live = now;
        }
        // Not zero, for the reason above: a run's pod definitions stay rooted
        // by the slots they were filed under. What matters is that the cost
        // is the SAME every rebuild and small -- a leak accelerates, and this
        // is what a leak of some 2,970 objects a rebuild was brought down to.
        let worst = *steps.iter().max().unwrap();
        assert!(
            worst <= 192,
            "a rebuild left {worst} more live objects than the one before it; per-rebuild steps were {steps:?}"
        );
        assert!(
            steps.iter().all(|s| (s - steps[0]).abs() <= 32),
            "the cost of a rebuild is not settling: {steps:?}"
        );
    }
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

/// Asserts `call` is registered after every one of `bases`, and directly
/// after the first of them, so each new widget keeps the slot its bases
/// give it and no later edit slides another registration in between.
#[cfg(test)]
fn assert_registered_after(call: &str, bases: &[&str]) {
    let calls = widgets_mod_source();
    let at = calls.find(call).unwrap_or_else(|| panic!("{call} is not registered"));
    for base in bases {
        let base_at = calls.find(base).unwrap_or_else(|| panic!("{base} is not registered"));
        assert!(base_at < at, "{base} must register before {call}");
    }
    let first = bases[0];
    let after_first = &calls[calls.find(first).unwrap() + first.len()..];
    let next = after_first
        .lines()
        .map(str::trim)
        .find(|line| line.ends_with("::script_mod(vm);"))
        .unwrap_or_default();
    assert_eq!(next, call, "{call} must follow {first} directly");
}

#[cfg(test)]
mod radial_menu_registration_tests {
    /// The ring menu registers after the glass it samples and the badge
    /// whose text measure it uses, with one type default: `PieMenu` and the
    /// other presets derive from it and must not take the default with them.
    /// Neither base is its neighbour, so the order is read here rather than
    /// through `assert_registered_after`, which also pins the slot.
    #[test]
    fn test_radial_menu_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let radial = include_str!("radial_menu.rs");
        assert!(lib.contains("\npub mod radial_menu;"));
        assert!(lib.contains("\n    radial_menu::*,"));
        let calls = crate::widgets_mod_source();
        let call = "crate::radial_menu::script_mod(vm);";
        let at = calls.find(call).unwrap_or_else(|| panic!("{call} is not registered"));
        for base in ["crate::gauss_view::script_mod(vm);", "crate::badge::script_mod(vm);"] {
            let base_at = calls.find(base).unwrap_or_else(|| panic!("{base} is not registered"));
            assert!(base_at < at, "{base} must register before {call}");
        }
        assert!(radial.contains("mod.widgets.RadialMenuBase = #(RadialMenu::register_widget(vm))"));
        assert_eq!(radial.matches("set_type_default() do mod.widgets.RadialMenuBase").count(), 1);
        assert!(radial.contains("mod.widgets.PieMenu = mod.widgets.RadialMenu{"), "the ring in its field is a preset, not a type");
    }
}

#[cfg(test)]
mod floating_action_registration_tests {
    /// The floating action registers directly after the toolbar whose
    /// floating slot can host it, after the button its faces are, with one
    /// type default for the action and one for an item.
    #[test]
    fn test_floating_action_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let floating = include_str!("floating_action.rs");
        assert!(lib.contains("\npub mod floating_action;"));
        assert!(lib.contains("\n    floating_action::*,"));
        crate::assert_registered_after(
            "crate::floating_action::script_mod(vm);",
            &["crate::toolbar::script_mod(vm);", "crate::button::script_mod(vm);"],
        );
        assert!(floating.contains("mod.widgets.FloatingActionBase = #(FloatingAction::register_widget(vm))"));
        assert!(floating.contains("mod.widgets.FloatingActionItemBase = #(FloatingActionItem::register_widget(vm))"));
        assert_eq!(floating.matches("set_type_default() do mod.widgets.FloatingActionBase").count(), 1);
        assert_eq!(floating.matches("set_type_default() do mod.widgets.FloatingActionItemBase").count(), 1);
    }
}

#[cfg(test)]
mod hamburger_menu_registration_tests {
    /// The menu is composed of a drawer, a popover, a nav list and a burger
    /// button, so it registers after all four, directly after the dialog a
    /// drawer is a preset of, which lands last.
    #[test]
    fn test_hamburger_menu_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let hamburger = include_str!("hamburger_menu.rs");
        assert!(lib.contains("\npub mod hamburger_menu;"));
        assert!(lib.contains("\n    hamburger_menu::*,"));
        crate::assert_registered_after(
            "crate::hamburger_menu::script_mod(vm);",
            &[
                "crate::dialog::script_mod(vm);",
                "crate::popover::script_mod(vm);",
                "crate::nav_list::script_mod(vm);",
                "crate::button::script_mod(vm);",
            ],
        );
        assert!(hamburger.contains("mod.widgets.HamburgerMenuBase = #(HamburgerMenu::register_widget(vm))"));
        assert_eq!(hamburger.matches("set_type_default() do mod.widgets.HamburgerMenuBase").count(), 1);
    }
}

#[cfg(test)]
mod pill_nav_registration_tests {
    /// The bar registers directly after the segmented control whose sliding
    /// pill it reuses, and after the popover whose trigger and placement
    /// words it takes, with one type default.
    #[test]
    fn test_pill_nav_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let pill = include_str!("pill_nav.rs");
        assert!(lib.contains("\npub mod pill_nav;"));
        assert!(lib.contains("\n    pill_nav::*,"));
        crate::assert_registered_after(
            "crate::pill_nav::script_mod(vm);",
            &["crate::button_group::script_mod(vm);", "crate::popover::script_mod(vm);"],
        );
        assert!(pill.contains("mod.widgets.PillNavBase = #(PillNav::register_widget(vm))"));
        assert_eq!(pill.matches("set_type_default() do mod.widgets.PillNavBase").count(), 1);
    }
}

#[cfg(test)]
mod line_menu_registration_tests {
    /// The stack registers directly after the scroll views it follows, and
    /// after the nav list and pill nav whose ground and surface it draws
    /// with, with one type default.
    #[test]
    fn test_line_menu_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let line = include_str!("line_menu.rs");
        assert!(lib.contains("\npub mod line_menu;"));
        assert!(lib.contains("\n    line_menu::*,"));
        crate::assert_registered_after(
            "crate::line_menu::script_mod(vm);",
            &[
                "crate::scroll_fade::script_mod(vm);",
                "crate::nav_list::script_mod(vm);",
                "crate::pill_nav::script_mod(vm);",
            ],
        );
        assert!(line.contains("mod.widgets.LineMenuBase = #(LineMenu::register_widget(vm))"));
        assert_eq!(line.matches("set_type_default() do mod.widgets.LineMenuBase").count(), 1);
    }
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
