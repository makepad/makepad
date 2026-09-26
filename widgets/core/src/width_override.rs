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

    /// Lay `other` over this one: every field it states wins, every field it
    /// leaves alone keeps what was here.
    ///
    /// A row prices a level by laying the rungs in force over the authored
    /// face, in order, and asking the child what it would be under the result.
    /// Doing it this way rather than reading the child is what keeps a price
    /// independent of the face the child happens to be wearing -- otherwise the
    /// first concession makes every later measurement a measurement of the
    /// concession, and the row can never price its way back up.
    pub fn overlay(&mut self, other: &Self) {
        if other.visible.is_some() {
            self.visible = other.visible;
        }
        if other.text.is_some() {
            self.text = other.text.clone();
        }
        if other.width.is_some() {
            self.width = other.width;
        }
        if other.margin.is_some() {
            self.margin = other.margin;
        }
        if other.padding.is_some() {
            self.padding = other.padding;
        }
        if other.spacing.is_some() {
            self.spacing = other.spacing;
        }
        self.opaque |= other.opaque;
    }

    /// Every key this block names, width-relevant or not.
    ///
    /// A row snapshots exactly these off the child before it puts any face on,
    /// so that taking a face off again is applying a block like any other.
    pub fn keys(vm: &mut ScriptVm, block: ScriptObject) -> Vec<LiveId> {
        let mut keys = Vec::new();
        let mut read = |_vm: &mut ScriptVm, map: &mut ScriptObjectMap| {
            for (key, _) in map.iter() {
                if let Some(k) = key.as_id() {
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
        };
        vm.proto_map_iter_mut_with(block, &mut read);
        keys
    }
}
