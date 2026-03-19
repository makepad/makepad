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
#[cfg(feature = "voice")]
pub mod voice_wave;
pub mod window;
pub mod window_menu;
#[cfg(feature = "voice")]
mod window_voice_input;

pub mod drop_down;
pub mod popup_menu;
pub mod slider;
pub mod text_input;

pub mod splitter;

pub mod fold_button;
pub mod fold_header;

pub mod glass_panel;
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
#[cfg(feature = "3d")]
#[path = "3d/mod.rs"]
pub mod widgets_3d;

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
pub mod popup_notification;
pub mod slides_view;
pub mod tooltip;
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

pub use crate::{
    adaptive_view::*,
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    // loading_spinner - no public exports
    bare_step::*,
    button::*,
    cached_widget::*,
    callout_tooltip::*,
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
    widget_tree::{set_ui_root, CxWidgetExt},

    window::*,

    window_menu::*,
};

#[cfg(feature = "voice")]
pub use crate::voice_wave::*;

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
#[cfg(feature = "3d")]
pub use crate::widgets_3d::*;

pub use crate::chart::*;

pub use crate::video::*;

const WIDGET_THEME_REGISTRY_MODULE: LiveId = live_id!(makepad_widgets_theme_registered);
const WIDGET_NAMESPACE_REGISTRY_MODULE: LiveId = live_id!(makepad_widgets_namespace_registered);
const WIDGET_REGISTRY_MODULE: LiveId = live_id!(makepad_widgets_registered);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetModule {
    ScrollBar,
    ScrollBars,
    View,
    ViewUi,
    RubberView,
    Label,
    LinkLabel,
    Button,
    CheckBox,
    RadioButton,
    Image,
    ImageBlend,
    Icon,
    AdaptiveView,
    DesktopButton,
    KeyboardView,
    VoiceWave,
    WindowMenu,
    NavControl,
    Window,
    PopupMenu,
    DropDown,
    TextInput,
    Slider,
    Splitter,
    FoldButton,
    FoldHeader,
    LoadingSpinner,
    GlassPanel,
    BareStep,
    TurtleStep,
    PortalList,
    TextFlow,
    CachedWidget,
    Root,
    CommandTextInput,
    TabCloseButton,
    Tab,
    TabBar,
    Dock,
    ScrollShadow,
    StackNavigation,
    ExpandablePanel,
    Modal,
    Tooltip,
    CalloutTooltip,
    PopupNotification,
    Video,
    PageFlip,
    FileTree,
    FlatList,
    SlidesView,
    SlidePanel,
    Html,
    Markdown,
    Splash,
    Svg,
    Vector,
    Chart,
    MathView,
    PdfView,
    Widgets3d,
    MapStyle,
    MapView,
}

impl WidgetModule {
    fn marker_id(self) -> LiveId {
        match self {
            WidgetModule::ScrollBar => live_id!(scroll_bar),
            WidgetModule::ScrollBars => live_id!(scroll_bars),
            WidgetModule::View => live_id!(view),
            WidgetModule::ViewUi => live_id!(view_ui),
            WidgetModule::RubberView => live_id!(rubber_view),
            WidgetModule::Label => live_id!(label),
            WidgetModule::LinkLabel => live_id!(link_label),
            WidgetModule::Button => live_id!(button),
            WidgetModule::CheckBox => live_id!(check_box),
            WidgetModule::RadioButton => live_id!(radio_button),
            WidgetModule::Image => live_id!(image),
            WidgetModule::ImageBlend => live_id!(image_blend),
            WidgetModule::Icon => live_id!(icon),
            WidgetModule::AdaptiveView => live_id!(adaptive_view),
            WidgetModule::DesktopButton => live_id!(desktop_button),
            WidgetModule::KeyboardView => live_id!(keyboard_view),
            WidgetModule::VoiceWave => live_id!(voice_wave),
            WidgetModule::WindowMenu => live_id!(window_menu),
            WidgetModule::NavControl => live_id!(nav_control),
            WidgetModule::Window => live_id!(window),
            WidgetModule::PopupMenu => live_id!(popup_menu),
            WidgetModule::DropDown => live_id!(drop_down),
            WidgetModule::TextInput => live_id!(text_input),
            WidgetModule::Slider => live_id!(slider),
            WidgetModule::Splitter => live_id!(splitter),
            WidgetModule::FoldButton => live_id!(fold_button),
            WidgetModule::FoldHeader => live_id!(fold_header),
            WidgetModule::LoadingSpinner => live_id!(loading_spinner),
            WidgetModule::GlassPanel => live_id!(glass_panel),
            WidgetModule::BareStep => live_id!(bare_step),
            WidgetModule::TurtleStep => live_id!(turtle_step),
            WidgetModule::PortalList => live_id!(portal_list),
            WidgetModule::TextFlow => live_id!(text_flow),
            WidgetModule::CachedWidget => live_id!(cached_widget),
            WidgetModule::Root => live_id!(root),
            WidgetModule::CommandTextInput => live_id!(command_text_input),
            WidgetModule::TabCloseButton => live_id!(tab_close_button),
            WidgetModule::Tab => live_id!(tab),
            WidgetModule::TabBar => live_id!(tab_bar),
            WidgetModule::Dock => live_id!(dock),
            WidgetModule::ScrollShadow => live_id!(scroll_shadow),
            WidgetModule::StackNavigation => live_id!(stack_navigation),
            WidgetModule::ExpandablePanel => live_id!(expandable_panel),
            WidgetModule::Modal => live_id!(modal),
            WidgetModule::Tooltip => live_id!(tooltip),
            WidgetModule::CalloutTooltip => live_id!(callout_tooltip),
            WidgetModule::PopupNotification => live_id!(popup_notification),
            WidgetModule::Video => live_id!(video),
            WidgetModule::PageFlip => live_id!(page_flip),
            WidgetModule::FileTree => live_id!(file_tree),
            WidgetModule::FlatList => live_id!(flat_list),
            WidgetModule::SlidesView => live_id!(slides_view),
            WidgetModule::SlidePanel => live_id!(slide_panel),
            WidgetModule::Html => live_id!(html),
            WidgetModule::Markdown => live_id!(markdown),
            WidgetModule::Splash => live_id!(splash),
            WidgetModule::Svg => live_id!(svg),
            WidgetModule::Vector => live_id!(vector),
            WidgetModule::Chart => live_id!(chart),
            WidgetModule::MathView => live_id!(math_view),
            WidgetModule::PdfView => live_id!(pdf_view),
            WidgetModule::Widgets3d => live_id!(widgets_3d),
            WidgetModule::MapStyle => live_id!(map_style),
            WidgetModule::MapView => live_id!(map_view),
        }
    }

    fn dependencies(self) -> &'static [WidgetModule] {
        match self {
            WidgetModule::AdaptiveView => &[WidgetModule::View],
            WidgetModule::CommandTextInput => &[
                WidgetModule::Label,
                WidgetModule::PortalList,
                WidgetModule::TextInput,
                WidgetModule::ViewUi,
            ],
            WidgetModule::Dock => &[WidgetModule::Tab, WidgetModule::TabBar],
            WidgetModule::DropDown => &[WidgetModule::PopupMenu],
            WidgetModule::FileTree => &[WidgetModule::ScrollBars],
            WidgetModule::FlatList => &[WidgetModule::ScrollBars],
            WidgetModule::GlassPanel => &[WidgetModule::ViewUi],
            WidgetModule::KeyboardView => &[WidgetModule::ViewUi],
            WidgetModule::LoadingSpinner => &[WidgetModule::ViewUi],
            WidgetModule::PortalList => &[WidgetModule::ScrollBar],
            WidgetModule::ScrollBars => &[WidgetModule::ScrollBar],
            WidgetModule::SlidesView => &[WidgetModule::Label, WidgetModule::ViewUi],
            WidgetModule::TabBar => &[WidgetModule::Tab],
            WidgetModule::ViewUi => &[WidgetModule::ScrollBars, WidgetModule::View],
            WidgetModule::VoiceWave => &[WidgetModule::View],
            WidgetModule::Window => &[
                WidgetModule::DesktopButton,
                WidgetModule::KeyboardView,
                WidgetModule::Label,
                WidgetModule::NavControl,
                WidgetModule::ViewUi,
                WidgetModule::VoiceWave,
                WidgetModule::WindowMenu,
            ],
            _ => &[],
        }
    }

    fn register_script_mod(self, vm: &mut ScriptVm) {
        match self {
            WidgetModule::ScrollBar => {
                crate::scroll_bar::script_mod(vm);
            }
            WidgetModule::ScrollBars => {
                crate::scroll_bars::script_mod(vm);
            }
            WidgetModule::View => {
                crate::view::script_mod(vm);
            }
            WidgetModule::ViewUi => {
                crate::view_ui::script_mod(vm);
            }
            WidgetModule::RubberView => {
                crate::rubber_view::script_mod(vm);
            }
            WidgetModule::Label => {
                crate::label::script_mod(vm);
            }
            WidgetModule::LinkLabel => {
                crate::link_label::script_mod(vm);
            }
            WidgetModule::Button => {
                crate::button::script_mod(vm);
            }
            WidgetModule::CheckBox => {
                crate::check_box::script_mod(vm);
            }
            WidgetModule::RadioButton => {
                crate::radio_button::script_mod(vm);
            }
            WidgetModule::Image => {
                crate::image::script_mod(vm);
            }
            WidgetModule::ImageBlend => {
                crate::image_blend::script_mod(vm);
            }
            WidgetModule::Icon => {
                crate::icon::script_mod(vm);
            }
            WidgetModule::AdaptiveView => {
                crate::adaptive_view::script_mod(vm);
            }
            WidgetModule::DesktopButton => {
                crate::desktop_button::script_mod(vm);
            }
            WidgetModule::KeyboardView => {
                crate::keyboard_view::script_mod(vm);
            }
            WidgetModule::VoiceWave => {
                #[cfg(feature = "voice")]
                crate::voice_wave::script_mod(vm);
                #[cfg(not(feature = "voice"))]
                script_eval!(vm, {
                    use mod.widgets.View
                    mod.widgets.VoiceWave = mod.widgets.View {
                        visible: false
                    }
                });
            }
            WidgetModule::WindowMenu => {
                crate::window_menu::script_mod(vm);
            }
            WidgetModule::NavControl => {
                crate::nav_control::script_mod(vm);
            }
            WidgetModule::Window => {
                crate::window::script_mod(vm);
            }
            WidgetModule::PopupMenu => {
                crate::popup_menu::script_mod(vm);
            }
            WidgetModule::DropDown => {
                crate::drop_down::script_mod(vm);
            }
            WidgetModule::TextInput => {
                crate::text_input::script_mod(vm);
            }
            WidgetModule::Slider => {
                crate::slider::script_mod(vm);
            }
            WidgetModule::Splitter => {
                crate::splitter::script_mod(vm);
            }
            WidgetModule::FoldButton => {
                crate::fold_button::script_mod(vm);
            }
            WidgetModule::FoldHeader => {
                crate::fold_header::script_mod(vm);
            }
            WidgetModule::LoadingSpinner => {
                crate::loading_spinner::script_mod(vm);
            }
            WidgetModule::GlassPanel => {
                crate::glass_panel::script_mod(vm);
            }
            WidgetModule::BareStep => {
                crate::bare_step::script_mod(vm);
            }
            WidgetModule::TurtleStep => {
                crate::turtle_step::script_mod(vm);
            }
            WidgetModule::PortalList => {
                crate::portal_list::script_mod(vm);
            }
            WidgetModule::TextFlow => {
                crate::text_flow::script_mod(vm);
            }
            WidgetModule::CachedWidget => {
                crate::cached_widget::script_mod(vm);
            }
            WidgetModule::Root => {
                crate::root::script_mod(vm);
            }
            WidgetModule::CommandTextInput => {
                crate::command_text_input::script_mod(vm);
            }
            WidgetModule::TabCloseButton => {
                crate::tab_close_button::script_mod(vm);
            }
            WidgetModule::Tab => {
                crate::tab::script_mod(vm);
            }
            WidgetModule::TabBar => {
                crate::tab_bar::script_mod(vm);
            }
            WidgetModule::Dock => {
                crate::dock::script_mod(vm);
            }
            WidgetModule::ScrollShadow => {
                crate::scroll_shadow::script_mod(vm);
            }
            WidgetModule::StackNavigation => {
                crate::stack_navigation::script_mod(vm);
            }
            WidgetModule::ExpandablePanel => {
                crate::expandable_panel::script_mod(vm);
            }
            WidgetModule::Modal => {
                crate::modal::script_mod(vm);
            }
            WidgetModule::Tooltip => {
                crate::tooltip::script_mod(vm);
            }
            WidgetModule::CalloutTooltip => {
                crate::callout_tooltip::script_mod(vm);
            }
            WidgetModule::PopupNotification => {
                crate::popup_notification::script_mod(vm);
            }
            WidgetModule::Video => {
                crate::video::script_mod(vm);
            }
            WidgetModule::PageFlip => {
                crate::page_flip::script_mod(vm);
            }
            WidgetModule::FileTree => {
                crate::file_tree::script_mod(vm);
            }
            WidgetModule::FlatList => {
                crate::flat_list::script_mod(vm);
            }
            WidgetModule::SlidesView => {
                crate::slides_view::script_mod(vm);
            }
            WidgetModule::SlidePanel => {
                crate::slide_panel::script_mod(vm);
            }
            WidgetModule::Html => {
                crate::html::script_mod(vm);
            }
            WidgetModule::Markdown => {
                crate::markdown::script_mod(vm);
            }
            WidgetModule::Splash => {
                crate::splash::script_mod(vm);
            }
            WidgetModule::Svg => {
                crate::svg::script_mod(vm);
            }
            WidgetModule::Vector => {
                crate::vector::script_mod(vm);
            }
            WidgetModule::Chart => {
                crate::chart::script_mod(vm);
            }
            WidgetModule::MathView => {
                crate::math_view::script_mod(vm);
            }
            WidgetModule::PdfView => {
                #[cfg(feature = "pdf")]
                crate::pdf_view::script_mod(vm);
            }
            WidgetModule::Widgets3d => {
                #[cfg(feature = "3d")]
                crate::widgets_3d::script_mod(vm);
            }
            WidgetModule::MapStyle => {
                #[cfg(feature = "maps")]
                crate::map::style::script_mod(vm);
            }
            WidgetModule::MapView => {
                #[cfg(feature = "maps")]
                crate::map::view::script_mod(vm);
            }
        }
    }
}

fn registry_module(vm: &mut ScriptVm, marker: LiveId) -> ScriptObject {
    let existing = vm.bx.heap.value(vm.bx.heap.modules, marker.into(), NoTrap);
    if let Some(module) = existing.as_object() {
        module
    } else {
        vm.new_module(marker)
    }
}

fn theme_mod_registered(vm: &mut ScriptVm) -> bool {
    vm.bx
        .heap
        .value(vm.bx.heap.modules, WIDGET_THEME_REGISTRY_MODULE.into(), NoTrap)
        .as_object()
        .is_some()
}

fn mark_theme_mod_registered(vm: &mut ScriptVm) {
    if !theme_mod_registered(vm) {
        vm.new_module(WIDGET_THEME_REGISTRY_MODULE);
    }
}

fn widgets_namespace_registered(vm: &mut ScriptVm) -> bool {
    vm.bx
        .heap
        .value(
            vm.bx.heap.modules,
            WIDGET_NAMESPACE_REGISTRY_MODULE.into(),
            NoTrap,
        )
        .as_object()
        .is_some()
}

fn mark_widgets_namespace_registered(vm: &mut ScriptVm) {
    if !widgets_namespace_registered(vm) {
        vm.new_module(WIDGET_NAMESPACE_REGISTRY_MODULE);
    }
}

fn widget_registered(vm: &mut ScriptVm, module: WidgetModule) -> bool {
    let registry = registry_module(vm, WIDGET_REGISTRY_MODULE);
    vm.bx
        .heap
        .value(registry, module.marker_id().into(), NoTrap)
        .as_object()
        .is_some()
}

fn mark_widget_registered(vm: &mut ScriptVm, module: WidgetModule) {
    let registry = registry_module(vm, WIDGET_REGISTRY_MODULE);
    vm.bx
        .heap
        .set_value_def(registry, module.marker_id().into(), registry.into());
}

fn ensure_widgets_namespace(vm: &mut ScriptVm) {
    theme_mod(vm);
    if widgets_namespace_registered(vm) {
        return;
    }

    script_eval!(vm, {
        mod.prelude.widgets_internal = {
            ..mod.prelude.widgets_header,
            theme:mod.theme,
        }
    });
    if vm
        .bx
        .heap
        .value(vm.bx.heap.modules, id!(widgets).into(), NoTrap)
        .as_object()
        .is_none()
    {
        vm.new_module(id!(widgets));
    }
    refresh_widgets_prelude(vm);
    mark_widgets_namespace_registered(vm);
}

fn refresh_widgets_prelude(vm: &mut ScriptVm) {
    script_eval!(vm, {
        mod.prelude.widgets = {
            ..mod.prelude.widgets_header,
            theme:mod.theme,
            ..mod.widgets,
        }
    });
}

fn register_widget_recursive(vm: &mut ScriptVm, module: WidgetModule) -> bool {
    if widget_registered(vm, module) {
        return false;
    }

    for dependency in module.dependencies() {
        register_widget_recursive(vm, *dependency);
    }

    module.register_script_mod(vm);
    mark_widget_registered(vm, module);
    true
}

pub fn register_widgets(vm: &mut ScriptVm, modules: &[WidgetModule]) {
    ensure_widgets_namespace(vm);
    let mut changed = false;
    for module in modules {
        changed |= register_widget_recursive(vm, *module);
    }
    if changed {
        refresh_widgets_prelude(vm);
    }
}

pub fn register_all_widgets(vm: &mut ScriptVm) {
    register_widgets(
        vm,
        &[
            WidgetModule::ScrollBar,
            WidgetModule::ScrollBars,
            WidgetModule::View,
            WidgetModule::ViewUi,
            WidgetModule::RubberView,
            WidgetModule::Label,
            WidgetModule::LinkLabel,
            WidgetModule::Button,
            WidgetModule::CheckBox,
            WidgetModule::RadioButton,
            WidgetModule::Image,
            WidgetModule::ImageBlend,
            WidgetModule::Icon,
            WidgetModule::AdaptiveView,
            WidgetModule::DesktopButton,
            WidgetModule::KeyboardView,
            WidgetModule::VoiceWave,
            WidgetModule::WindowMenu,
            WidgetModule::NavControl,
            WidgetModule::Window,
            WidgetModule::PopupMenu,
            WidgetModule::DropDown,
            WidgetModule::TextInput,
            WidgetModule::Slider,
            WidgetModule::Splitter,
            WidgetModule::FoldButton,
            WidgetModule::FoldHeader,
            WidgetModule::LoadingSpinner,
            WidgetModule::GlassPanel,
            WidgetModule::BareStep,
            WidgetModule::TurtleStep,
            WidgetModule::PortalList,
            WidgetModule::TextFlow,
            WidgetModule::CachedWidget,
            WidgetModule::Root,
            WidgetModule::CommandTextInput,
            WidgetModule::TabCloseButton,
            WidgetModule::Tab,
            WidgetModule::TabBar,
            WidgetModule::Dock,
            WidgetModule::ScrollShadow,
            WidgetModule::StackNavigation,
            WidgetModule::ExpandablePanel,
            WidgetModule::Modal,
            WidgetModule::Tooltip,
            WidgetModule::CalloutTooltip,
            WidgetModule::PopupNotification,
            WidgetModule::Video,
            WidgetModule::PageFlip,
            WidgetModule::FileTree,
            WidgetModule::FlatList,
            WidgetModule::SlidesView,
            WidgetModule::SlidePanel,
            WidgetModule::Html,
            WidgetModule::Markdown,
            WidgetModule::Splash,
            WidgetModule::Svg,
            WidgetModule::Vector,
            WidgetModule::Chart,
            WidgetModule::MathView,
            WidgetModule::PdfView,
            WidgetModule::Widgets3d,
            WidgetModule::MapStyle,
            WidgetModule::MapView,
        ],
    );
}

pub fn theme_mod(vm: &mut ScriptVm) {
    if theme_mod_registered(vm) {
        return;
    }
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
    #[cfg(target_arch = "wasm32")]
    script_eval!(vm, {
        use mod.text.*
        use mod.res.*

        mod.themes.dark = mod.themes.dark{
            font_label: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_regular: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_italic: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_italic: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_regular_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_italic_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_italic_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
        }

        mod.themes.light = mod.themes.light{
            font_label: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_regular: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_italic: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_italic: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_regular_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_italic_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_italic_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
        }

        mod.themes.skeleton = mod.themes.skeleton{
            font_label: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_regular: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_italic: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_italic: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_regular_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_italic_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
            font_bold_italic_i18n: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
                line_spacing: 1.2
            }
        }
    });
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
            ..mod.ime,
            ..mod.shader,
            ..mod.animator.Play,
            ..mod.animator.Ease,
            draw:mod.draw,
            MouseCursor:mod.draw.MouseCursor
        }
        mod.theme = mod.themes.dark

    });
    mark_theme_mod_registered(vm);
}

pub fn widgets_mod(vm: &mut ScriptVm) {
    register_all_widgets(vm);
}

pub fn script_mod(vm: &mut ScriptVm) {
    theme_mod(vm);
    widgets_mod(vm);
}
