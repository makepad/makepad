//! The core's internal helpers the family crates build with.
//!
//! These are crate-private in the modules that own them, whose items the
//! prelude re-exports wholesale (`badge::*`, `modal::*`, ...); making
//! them public there would put names like `measure` into every app's
//! `use makepad_widgets::*`. The families reach them here instead, a module
//! the prelude does not glob.
#![doc(hidden)]

use crate::{makepad_draw::*, widget::WidgetRef};
use crate::makepad_script::trap::NoTrap;

/// `badge`'s text measure: the width of one line of `text` in this text
/// style, plus the slack a drawn run needs.
pub fn measure(draw_text: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    crate::badge::measure(draw_text, cx, text)
}

/// `badge`'s walk sizing: a `Fit` walk becomes the size the widget worked
/// out; a fixed one is left alone.
pub fn sized(walk: Walk, w: f64, h: f64) -> Walk {
    crate::badge::sized(walk, w, h)
}

// Reading the script tree: a script array whichever way it arrived, and
// the fields of a script object. The menu bar, the hotkeys and the
// navigation menus all read their `#[live] ScriptValue` tables with these.

/// Walk a script array, whether it arrived as an array value or as an
/// object carrying a vec (both spellings reach a `#[live] ScriptValue`).
pub fn for_each_element(
    vm: &mut ScriptVm,
    value: ScriptValue,
    f: &mut dyn FnMut(&mut ScriptVm, ScriptValue),
) {
    if let Some(array) = value.as_array() {
        let len = vm.bx.heap.array_len(array);
        for i in 0..len {
            let item = vm.bx.heap.array_index_unchecked(array, i);
            f(vm, item);
        }
        return;
    }
    if let Some(object) = value.as_object() {
        let len = vm.bx.heap.vec_len(object);
        for i in 0..len {
            if let Some(item) = vm.bx.heap.vec_value_if_exist(object, i) {
                f(vm, item);
            }
        }
    }
}

pub fn obj_field(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> ScriptValue {
    let value = vm.bx.heap.value(object, key.into(), NoTrap);
    if value.is_err() {
        ScriptValue::NIL
    } else {
        value
    }
}

pub fn obj_string(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> Option<String> {
    let value = obj_field(vm, object, key);
    if value.is_nil() {
        return None;
    }
    vm.string_with(value, |_, s| s.to_string())
}

pub fn obj_bool(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> Option<bool> {
    obj_field(vm, object, key).as_bool()
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

/// `modal`: whether `event` carries the dismissal a closing host sends
/// straight through its descendants (so a drag or an open popup inside it
/// lets go).
pub fn is_modal_dismissal(event: &Event) -> bool {
    crate::modal::ModalAction::is_dismissal(event)
}

/// `modal`: the handle a kept area has now, followed across the redraws
/// of its list the way the platform follows a lock's owner.
pub fn area_after_redraws(cx: &Cx, area: Area) -> Area {
    crate::modal::area_after_redraws(cx, area)
}

// The tour's lookups, which the line menu shares.

/// A widget's area, or nothing when the widget cannot be asked for one.
///
/// A widget that is mid-dispatch is mutably borrowed, and `area()` on a
/// borrowed widget panics rather than answering. Everything between a tour
/// and the root is mid-dispatch the whole time the tour is running code, so
/// the borrow has to be tested before the question is asked — and
/// `try_widget_uid` answers None for exactly the widgets that cannot be
/// asked anything else either.
pub fn askable_area(widget: &WidgetRef) -> Option<Area> {
    widget.try_widget_uid()?;
    Some(widget.area())
}

/// Turn a dotted id path into the ids a widget lookup takes.
pub fn id_path(path: &str) -> Vec<LiveId> {
    path.split('.')
        .filter(|part| !part.is_empty())
        .map(LiveId::from_str)
        .collect()
}
