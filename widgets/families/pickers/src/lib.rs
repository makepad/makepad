//! Number fields, range sliders, drop toggles, the combo box and DropDown2,
//! ratings, tag fields, radio groups, wheel pickers, avatars, forms, and the
//! column, tree and transfer pickers.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `pickers` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod avatar;
pub mod column_picker;
pub mod combo_box;
pub mod drop_down2;
pub mod drop_toggles;
pub mod form;
pub mod number_field;
pub mod picker_parts;
pub mod radio_group;
pub mod range_slider;
pub mod rating;
pub mod tag_field;
pub mod transfer;
pub mod tree_select;
pub mod wheel_picker;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn pickers_mod(vm: &mut ScriptVm) {
    crate::drop_down2::script_mod(vm);
    crate::range_slider::script_mod(vm);
    crate::drop_toggles::script_mod(vm);
    crate::combo_box::script_mod(vm);
    crate::number_field::script_mod(vm);
    crate::rating::script_mod(vm);
    crate::tag_field::script_mod(vm);
    crate::radio_group::script_mod(vm);
    crate::wheel_picker::script_mod(vm);
    crate::avatar::script_mod(vm);
    crate::form::script_mod(vm);
    crate::picker_parts::script_mod(vm);
    crate::column_picker::script_mod(vm);
    crate::tree_select::script_mod(vm);
    crate::transfer::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with this family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[pickers_mod]);
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

// Registration order, read off the source the way the core's registration
// tests read it. The family registers after every widget of the core, so
// the order an app gets is the core's `widgets_mod_with` followed by this
// crate's `pickers_mod`, and that is the text searched here.

/// The front crate's lib.rs, whose re-exports put each widget in the prelude.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) const FRONT_LIB: &str = include_str!("../../../src/lib.rs");

/// This crate's lib.rs, which declares the modules.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) const FAMILY_LIB: &str = include_str!("lib.rs");

/// The core's registration, then this family's, in the order they run.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn widgets_mod_source() -> &'static str {
    static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| {
        let core = include_str!("../../../core/src/lib.rs");
        let start = core.find("pub fn widgets_mod_with(").expect("widgets_mod_with");
        let end = core.find("pub fn script_mod(vm: &mut ScriptVm)").expect("script_mod");
        let family = include_str!("lib.rs");
        let fam_start = family.find("pub fn pickers_mod(").expect("pickers_mod");
        let fam_end = fam_start + family[fam_start..].find("\n}\n").expect("end of pickers_mod");
        format!("{}{}", &core[start..end], &family[fam_start..fam_end])
    })
}

#[cfg(test)]
mod field_row_tests {
    use makepad_widgets_core::desktop_style::{catalogue, install, StyleSheet};
    use makepad_widgets_core::makepad_script::trap::NoTrap;
    use makepad_widgets_core::*;

    /// A field's box metrics as the sheet leaves them: the minimum height,
    /// and the padding above and below the line.
    fn field_metrics(vm: &mut ScriptVm, name: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
        let widgets = vm.module(id!(widgets));
        let widget = vm
            .bx
            .heap
            .value(widgets, LiveId::from_str(name).into(), NoTrap)
            .as_object()
            .unwrap_or_else(|| panic!("the family has no widget named {name}"));
        let min = vm.bx.heap.value(widget, id!(min_height).into(), NoTrap).as_f64();
        let pad = vm.bx.heap.value(widget, id!(padding).into(), NoTrap).as_object();
        let side = |vm: &mut ScriptVm, key: LiveId| pad.and_then(|p| vm.bx.heap.value(p, key.into(), NoTrap).as_f64());
        (min, side(vm, id!(top)), side(vm, id!(bottom)))
    }

    /// The core's row test, for the fields that moved here with this family
    /// (see `desktop_style`'s tests): the Fit fields stand in the row the
    /// sheet's text box sets, whole; the number field states only the row's
    /// height and lays its own parts out, so no vertical padding moves its
    /// line off the box.
    #[test]
    fn the_picker_fields_stand_at_the_height_their_sheet_gives_the_text_box() {
        for entry in catalogue() {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.init_cx_os();
            cx.with_vm(|vm| {
                crate::script_mod(vm);
                install(vm, StyleSheet::load(entry));
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                let sheet = entry.id;
                assert!(vm.take_errors().is_empty(), "{sheet} does not evaluate");
                let row = field_metrics(vm, "TextInput");
                assert!(row.0.is_some(), "{sheet} states no field height for the row to stand at");
                for name in ["ComboBox", "DropDown2", "TagField"] {
                    assert_eq!(
                        field_metrics(vm, name),
                        row,
                        "{sheet}: {name} does not stand in the row its TextInput sets"
                    );
                }
                let field = field_metrics(vm, "NumberField");
                assert_eq!(field.0, row.0, "{sheet}: NumberField does not stand at the height of the row");
                assert!(
                    field.1.unwrap_or(0.0) == 0.0 && field.2.unwrap_or(0.0) == 0.0,
                    "{sheet}: NumberField lays its own parts out, so a vertical padding on it moves the line off the box"
                );
            });
        }
    }
}
