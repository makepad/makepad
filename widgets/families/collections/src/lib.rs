//! Carousels, list items, tables, masonry, tile lists, item grids, kanban
//! boards, scroll marks, SVG selection, conceding rows and corner caps.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `collections` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod carousel;
mod column_fit;
pub mod conceding_row;
pub mod corner_cap_view;
pub mod item_grid;
pub mod item_selection;
pub mod kanban;
pub mod list_item;
pub mod masonry;
pub mod scroll_marks;
pub mod svg_select;
pub mod table;
pub mod tile_list;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn collections_mod(vm: &mut ScriptVm) {
    crate::conceding_row::script_mod(vm);
    crate::list_item::script_mod(vm);
    crate::table::script_mod(vm);
    crate::carousel::script_mod(vm);
    crate::masonry::script_mod(vm);
    crate::tile_list::script_mod(vm);
    crate::item_grid::script_mod(vm);
    crate::kanban::script_mod(vm);
    crate::scroll_marks::script_mod(vm);
    crate::svg_select::script_mod(vm);
    crate::corner_cap_view::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with this family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[collections_mod]);
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}
