//! The reflection and theme surface for tools OUTSIDE this crate.
//!
//! The Shift+F10 overlay (`tweaker.rs`) reads a widget's live state, the `/** */`
//! annotations behind its properties and the theme's tokens, and edits a
//! theme value with every draw buffer following. A catalogue app's Docs,
//! Controls and Theme panels, a theme editor or a test harness need exactly
//! those reads and that one write, and they must not get a second
//! implementation that can disagree with the overlay. So this module is a
//! door onto the tweaker's own functions, with signatures that are a
//! contract: the test at the bottom binds each one to its exact type.
//!
//! * [`reflect_flat`]: `(name, value, is_set)` per property of a widget's
//!   CURRENT state, one level of dotted nesting (`draw_bg.color`), `is_set`
//!   true where the DSL explicitly applied it. The Docs panel's property
//!   table and the Controls panel's default values.
//! * [`collect_row_docs`]: property name -> its `/** */` doc, gathered up
//!   the construction chain (dotted names for sub-objects). The Docs panel's
//!   third column.
//! * [`theme_values`] / [`ThemeVal`]: every colour and number in
//!   `mod.theme` as `(name, key, value, defined_at)`. The Theme panel's rows
//!   and the probe behind `/tweak/op?op=theme&name=`.
//! * [`resolve_widget_by_path`] / [`indexed_path`]: a readable path
//!   (`/window/body/Button.2`) to a widget and back. How a story names its
//!   subject and how an action log names its sender.
//! * [`theme_set_value`]: set one theme value live, through the same path
//!   the overlay's own gestures use.

use crate::makepad_draw::*;
pub use makepad_widgets_core::design_data::{
    pick::{indexed_path, resolve_widget_by_path}, props::{collect_row_docs, reflect_flat},
    theme::{theme_values, ThemeVal},
};

/// Set one theme value at runtime. `name` is a token of `mod.theme`
/// (`color_bg_app`, `space_factor`); `text` is a colour as `#rgb`,
/// `#rrggbb` or `#rrggbbaa`, a number, or `theme.other` to take that
/// token's current value. A colour changes on screen in the next paint,
/// app-wide, without a reload; a number reaches the script heap so
/// everything applied from now on uses it, and existing layout follows
/// when the edit lands in the source. The edit is ledgered in the tweaker
/// session under origin "editor" (visible in `/tweak/diff` and
/// `/tweak/final`) and is undoable there.
///
/// Errors name the problem in plain words: an unknown token, text that is
/// neither a colour nor a number for the token's kind.
pub fn theme_set_value(cx: &mut Cx, name: &str, text: &str) -> Result<(), String> {
    crate::tweaker::theme_apply(cx, name, text, "editor", Some(crate::tweaker::track_undo))
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::WidgetRef;
    use std::collections::HashMap;

    /// The surface is a contract: these bindings compile only while every
    /// item is public with exactly this signature.
    #[test]
    fn the_surface_keeps_the_signatures_tools_build_on() {
        let _: fn(&mut Cx, &WidgetRef) -> Vec<(String, String, bool)> = reflect_flat;
        let _: fn(&mut Cx, &WidgetRef) -> HashMap<String, String> = collect_row_docs;
        let _: fn(&mut Cx) -> Vec<(String, LiveId, ThemeVal, String)> = theme_values;
        let _: fn(&Cx, &str) -> Result<WidgetRef, String> = resolve_widget_by_path;
        let _: fn(&Cx, u64) -> String = indexed_path;
        let _: fn(&mut Cx, &str, &str) -> Result<(), String> = theme_set_value;
        assert_ne!(ThemeVal::Color(0xff5c39ff), ThemeVal::Num(1.0));
    }


}
