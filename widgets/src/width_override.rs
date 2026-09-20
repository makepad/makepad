//! A named override block, parsed down to what can change a width.
//!
//! A widget carries its compact face as a named block beside its ordinary
//! properties:
//!
//! ```text
//! inspect := ButtonFlat{ text: "Inspect"  give_up := 1  tight: { text: "<>" } }
//! ```
//!
//! The block is declared once on the base with `:=`, which puts it in the
//! instance's vec rather than its map. Two things follow, and both are load
//! bearing. The colon form at the use site is legal, because the type-checked
//! path looks in the vec before refusing a key. And the widget's own apply never
//! sees it, because `value_for_apply` reads only the map -- so a block sits
//! there inert until somebody decides to put it on.

use crate::makepad_draw::*;

/// The part of a named override block that can change a width.
///
/// Parsed once, when the block is collected, so that pricing a rung never
/// applies it: a row asks each child what it *would* be, and the child stays as
/// it is. That is what lets a row settle before it draws rather than one rung
/// per draw.
///
/// The set of keys is closed on purpose. Anything outside it sets [`Self::opaque`],
/// and an opaque rung is walked the old way -- taken blind, measured on the draw
/// that follows. Guessing instead would price the rung wrong, and a rung priced
/// wrong is one the ladder never gives back.
#[derive(Clone, Debug, Default)]
pub struct WidthOverride {
    pub visible: Option<bool>,
    pub text: Option<String>,
    pub width: Option<Size>,
    pub margin: Option<Inset>,
    pub padding: Option<Inset>,
    pub spacing: Option<f64>,
    /// The block names something this cannot price.
    pub opaque: bool,
}

impl WidthOverride {
    /// Read a block into the keys that can move a width, leaving it untouched.
    pub fn parse(vm: &mut ScriptVm, block: ScriptObject) -> Self {
        let mut o = Self::default();
        let mut read = |vm: &mut ScriptVm, map: &mut ScriptObjectMap| {
            for (key, map_value) in map.iter() {
                let v = map_value.value;
                let Some(k) = key.as_id() else {
                    o.opaque = true;
                    continue;
                };
                if k == id!(visible) {
                    o.visible = v.as_bool();
                } else if k == id!(text) {
                    o.text = Some(vm.bx.heap.temp_string_with(|heap, out| {
                        heap.cast_to_string(v, out);
                        out.to_string()
                    }));
                } else if k == id!(width) {
                    o.width = Some(<Size as ScriptNew>::script_from_value(vm, v));
                } else if k == id!(margin) {
                    o.margin = Some(<Inset as ScriptNew>::script_from_value(vm, v));
                } else if k == id!(padding) {
                    o.padding = Some(<Inset as ScriptNew>::script_from_value(vm, v));
                } else if k == id!(spacing) {
                    o.spacing = v.as_number();
                } else {
                    // Anything else may or may not move a width. Refuse to guess.
                    o.opaque = true;
                }
            }
        };
        vm.proto_map_iter_mut_with(block, &mut read);
        o
    }

    /// Whether this rung takes the widget off the row entirely. A hidden child
    /// contributes neither its width nor a gap beside it.
    pub fn hides(&self) -> bool {
        self.visible == Some(false)
    }
}
