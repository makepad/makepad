//! The core's internal helpers the family crates build with.
//!
//! These are crate-private in the modules that own them, whose items the
//! prelude re-exports wholesale (`badge::*`, `menu_bar::*`, ...); making
//! them public there would put names like `measure` into every app's
//! `use makepad_widgets::*`. The families reach them here instead, a module
//! the prelude does not glob.
#![doc(hidden)]

use crate::{makepad_draw::*, widget::WidgetRef};

/// `badge`'s text measure: the width of one line of `text` in this text
/// style, plus the slack a drawn run needs.
pub fn measure(draw_text: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    crate::badge::measure(draw_text, cx, text)
}

/// `menu_bar`'s walk over a script array, whether it arrived as an array
/// value or as an object carrying a vec.
pub fn for_each_element(
    vm: &mut ScriptVm,
    value: ScriptValue,
    f: &mut dyn FnMut(&mut ScriptVm, ScriptValue),
) {
    crate::menu_bar::for_each_element(vm, value, f)
}

/// `menu_bar`'s field read of a script object (nil when absent).
pub fn obj_field(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> ScriptValue {
    crate::menu_bar::obj_field(vm, object, key)
}

/// `menu_bar`'s string field read of a script object.
pub fn obj_string(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> Option<String> {
    crate::menu_bar::obj_string(vm, object, key)
}

/// `menu_bar`'s bool field read of a script object.
pub fn obj_bool(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> Option<bool> {
    crate::menu_bar::obj_bool(vm, object, key)
}

/// `overlay_place`: leave sweep locks for the next event to release, from a
/// `Drop` that has no `Cx`.
pub fn orphan_sweep_locks(areas: &[Area]) {
    crate::overlay_place::orphan_sweep_locks(areas)
}

/// `overlay_place`: release every lock a dropped overlay left behind.
pub fn release_orphaned_sweep_locks(cx: &mut Cx) {
    crate::overlay_place::release_orphaned_sweep_locks(cx)
}

/// `modal`: release the scroll blocks a dropped modal left behind.
pub fn release_orphaned_scroll_blocks(cx: &mut Cx) {
    crate::modal::release_orphaned_scroll_blocks(cx)
}

/// `modal`: the handle a kept area has now, followed across the redraws
/// of its list the way the platform follows a lock's owner.
pub fn area_after_redraws(cx: &Cx, area: Area) -> Area {
    crate::modal::area_after_redraws(cx, area)
}

/// `tour`: a widget's area, or None for one that is mid-dispatch
/// (mutably borrowed) and cannot be asked.
pub fn askable_area(widget: &WidgetRef) -> Option<Area> {
    crate::tour::askable_area(widget)
}

/// `tour`: a dotted id path as the ids a widget lookup takes.
pub fn id_path(path: &str) -> Vec<LiveId> {
    crate::tour::id_path(path)
}
