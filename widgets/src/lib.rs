//! `makepad-widgets`: the name every app depends on.
//!
//! The library is `makepad-widgets-core` (views, windows, text, buttons,
//! inputs, lists, tabs, popups and the rest that is always there) plus one
//! crate per widget family under `widgets/families`. Neither the core nor a
//! family crate has features, so each compiles once per profile whatever
//! families the apps sharing a target dir pick. This crate is the front: its
//! features choose the family crates, it re-exports each under the module
//! path it always had (`makepad_widgets::markdown::Markdown`, the prelude's
//! glob items), and its `script_mod` registers the core with the chosen
//! families in the order the widgets build on each other.

pub use makepad_widgets_core::*;

#[cfg(feature = "fab")]
pub use makepad_widgets_fab::{
    fab_controls, menu_bar, menu_bar::*, property_inspector, property_inspector::*, tween_inspector,
};

#[cfg(feature = "editors")]
pub use makepad_widgets_editors::{
    ease_editor, ease_editor::*, gizmo, sequencer, sequencer::*,
};

#[cfg(feature = "hotkeys")]
pub use makepad_widgets_hotkeys::{
    hotkey_editor, hotkey_editor::*, hotkeys, hotkeys::*, keyboard_map, keyboard_map::*,
};

#[cfg(feature = "media")]
pub use makepad_widgets_media::{
    level_meter, level_meter::*, marquee, marquee::*, media, media::*, placeholder, placeholder::*,
    playback_bar, playback_bar::*, video, video::*,
};

#[cfg(feature = "collections")]
pub use makepad_widgets_collections::{
    carousel, carousel::*, conceding_row, corner_cap_view, corner_cap_view::*, item_grid,
    item_grid::*, item_selection, item_selection::*, kanban, kanban::*, list_item, list_item::*,
    masonry, masonry::*, scroll_marks, scroll_marks::*, svg_select, svg_select::*, table, table::*,
    tile_list, tile_list::*,
};

#[cfg(feature = "pickers")]
pub use makepad_widgets_pickers::{
    avatar, avatar::*, column_picker, column_picker::*, combo_box, combo_box::*, drop_down2,
    drop_down2::*, drop_toggles, drop_toggles::*, form, form::*, number_field, number_field::*,
    picker_parts, picker_parts::*, radio_group, radio_group::*, range_slider, range_slider::*,
    rating, rating::*, tag_field, tag_field::*, transfer, transfer::*, tree_select,
    tree_select::*, wheel_picker, wheel_picker::*,
};

#[cfg(feature = "catalog")]
pub use makepad_widgets_catalog::{
    accordion, accordion::*, alert, alert::*, breadcrumb, breadcrumb::*, code_block, code_block::*,
    empty_state, empty_state::*, floating_panel, floating_panel::*, pagination, pagination::*,
    spinner, spinner::*, toast, toast::*, tour, tour::*,
};

#[cfg(feature = "data")]
pub use makepad_widgets_data::{
    data_grid, data_grid::*, data_grid_columns, data_grid_columns::*, file_tree, file_tree::*,
    tree, tree::*,
};

#[cfg(feature = "dock")]
pub use makepad_widgets_data::{dock, dock::*};

#[cfg(feature = "rich_text")]
pub use makepad_widgets_rich_text::{
    html, html::*, log_list, log_list::*, makepad_html, markdown, markdown::*, math_view,
    math_view::*, rich_text, rich_text::*, text_flow, text_flow::*,
};

#[cfg(feature = "nav_menus")]
pub use makepad_widgets_menus::{
    floating_action, floating_action::*, hamburger_menu, hamburger_menu::*, line_menu,
    line_menu::*, pill_nav, pill_nav::*, radial_menu, radial_menu::*,
};

#[cfg(feature = "command_palette")]
pub use makepad_widgets_menus::{command_palette, command_palette::*};

#[cfg(feature = "charts")]
pub use makepad_widgets_extras::{
    chart, chart::*, chart_shapes, chart_shapes::*, waveform, waveform::*,
};

#[cfg(feature = "color")]
pub use makepad_widgets_extras::{color, color::*, gradient_editor, gradient_editor::*};

#[cfg(feature = "dates")]
pub use makepad_widgets_extras::{
    calendar, calendar::*, date_picker, date_picker::*, time_picker, time_picker::*,
};

#[cfg(feature = "chat")]
pub use makepad_widgets_extras::{chat, chat::*};

#[cfg(feature = "dropzone")]
pub use makepad_widgets_extras::{dropzone, dropzone::*};

#[cfg(feature = "vector")]
pub use makepad_widgets_extras::{vector, vector::*};

#[cfg(feature = "glass")]
pub use makepad_widgets_extras::{glass_panel, glass_panel::*};

#[cfg(feature = "maps")]
pub use makepad_widgets_maps::{
    map,
    map::overlay::{MapMarker, MapPuck, MapRouteOverlay},
    map::view::*,
};

#[cfg(feature = "pdf")]
pub use makepad_widgets_pdf::{makepad_pdf_parse, pdf_view, pdf_view::*};

#[cfg(feature = "voice")]
pub use makepad_widgets_voice::{voice_wave, voice_wave::*};

#[cfg(feature = "cef")]
pub use makepad_widgets_cef::{browser, browser::*};

/// The AI chat slot. The requests global is the core's and is here whether
/// or not the slot is linked (the window manager and the screen recorder
/// read it); the slot itself comes with the `ai` feature.
pub mod ai_slot {
    #[cfg(feature = "ai")]
    pub use makepad_widgets_ai::ai_slot::*;
    pub use makepad_widgets_core::widget_hooks::{AiSelectedRegion, AiSlotRequests};
}

/// The families a Window names, registered where it expects them; the ones
/// this build lacks are empty hidden views.
///
/// The design overlay is not one of this crate's families: the app links
/// `makepad-widgets-tweaker` itself and calls its `link` before this
/// crate's `script_mod`, which leaves the registration in the `Cx` global
/// [`WindowFamilies`] read here. So no crate built on this one waits for
/// the overlay, and an app's release build leaves it out by not linking it.
fn window_families(vm: &mut ScriptVm) -> WindowFamilies {
    // Every Splash isolate the host allocates from here on registers the
    // families this build picked after the core's widgets.
    vm.cx_mut().global::<makepad_widgets_core::widget_hooks::IsolateFamilies>().0 = families(true);
    #[allow(unused_mut)]
    let mut window = vm
        .cx_mut()
        .get_global_ref::<WindowFamilies>()
        .copied()
        .unwrap_or_default();
    #[cfg(feature = "voice")]
    {
        window.voice = Some(makepad_widgets_voice::voice_mod);
    }
    #[cfg(feature = "ai")]
    {
        window.ai = Some(makepad_widgets_ai::ai_mod);
    }
    window
}

/// The other families, after the core's widgets, in the order they build on
/// each other: the colour family before the charts that read it, the data
/// family before the dock. The rarer widgets that moved out of the core
/// (fab ... catalog) come first, nearest the core they were part of; none
/// of them names another family's widgets.
///
/// `isolate` is the list a Splash isolate registers: the families that
/// reach past the host service bridge (the browser and the map) stay out.
fn families(isolate: bool) -> Vec<fn(&mut ScriptVm)> {
    #[allow(unused_mut)]
    let mut families: Vec<fn(&mut ScriptVm)> = Vec::new();
    #[cfg(feature = "fab")]
    families.push(makepad_widgets_fab::fab_mod);
    #[cfg(feature = "editors")]
    families.push(makepad_widgets_editors::editors_mod);
    #[cfg(feature = "hotkeys")]
    families.push(makepad_widgets_hotkeys::hotkeys_mod);
    #[cfg(feature = "media")]
    families.push(makepad_widgets_media::media_mod);
    #[cfg(feature = "collections")]
    families.push(makepad_widgets_collections::collections_mod);
    #[cfg(feature = "pickers")]
    families.push(makepad_widgets_pickers::pickers_mod);
    #[cfg(feature = "catalog")]
    families.push(makepad_widgets_catalog::catalog_mod);
    #[cfg(feature = "cef")]
    if !isolate {
        families.push(makepad_widgets_cef::cef_mod);
    }
    #[cfg(feature = "glass")]
    families.push(makepad_widgets_extras::glass_mod);
    #[cfg(feature = "nav_menus")]
    families.push(makepad_widgets_menus::nav_menus_mod);
    #[cfg(feature = "data")]
    families.push(makepad_widgets_data::data_mod);
    #[cfg(feature = "dates")]
    families.push(makepad_widgets_extras::dates_mod);
    #[cfg(feature = "color")]
    families.push(makepad_widgets_extras::color_mod);
    #[cfg(feature = "charts")]
    families.push(makepad_widgets_extras::charts_mod);
    #[cfg(feature = "chat")]
    families.push(makepad_widgets_extras::chat_mod);
    #[cfg(feature = "dropzone")]
    families.push(makepad_widgets_extras::dropzone_mod);
    #[cfg(feature = "command_palette")]
    families.push(makepad_widgets_menus::command_palette_mod);
    #[cfg(feature = "rich_text")]
    families.push(makepad_widgets_rich_text::rich_text_mod);
    #[cfg(feature = "dock")]
    families.push(makepad_widgets_data::dock_mod);
    #[cfg(feature = "pdf")]
    families.push(makepad_widgets_pdf::pdf_mod);
    #[cfg(feature = "vector")]
    families.push(makepad_widgets_extras::vector_mod);
    #[cfg(feature = "maps")]
    if !isolate {
        families.push(makepad_widgets_maps::maps_mod);
    }
    families
}

/// The widgets of the core and of this build's families (the theme must be
/// in place: `theme_mod`).
pub fn widgets_mod(vm: &mut ScriptVm) {
    let window = window_families(vm);
    makepad_widgets_core::widgets_mod_with(vm, window, &families(false));
}

/// The theme, the widgets of the core and of this build's families, and the
/// desktop style over them: what every app's `script_mod` calls first.
pub fn script_mod(vm: &mut ScriptVm) {
    let window = window_families(vm);
    makepad_widgets_core::script_mod_with(vm, window, &families(false));
}
