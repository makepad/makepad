//! The ease-curve editor, the sequencer timeline, and the 3D gizmo and view
//! cube.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `editors` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod curve_editor;
pub mod ease_editor;
pub mod gizmo;
pub mod sequencer;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn editors_mod(vm: &mut ScriptVm) {
    crate::ease_editor::script_mod(vm);
    crate::curve_editor::script_mod(vm);
    crate::sequencer::script_mod(vm);
    crate::gizmo::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with this family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[editors_mod]);
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
// crate's `editors_mod`, and that is the text searched here.

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
        let fam_start = family.find("pub fn editors_mod(").expect("editors_mod");
        let fam_end = fam_start + family[fam_start..].find("\n}\n").expect("end of editors_mod");
        format!("{}{}", &core[start..end], &family[fam_start..fam_end])
    })
}

#[cfg(test)]
mod curve_editor_registration_tests {
    /// The curve editor registers directly after the ease editor it sits
    /// beside; the view, button, radio button and badge it is built from
    /// are the core's, and the core registers before every family. One
    /// type default.
    #[test]
    fn test_curve_editor_is_registered_after_its_bases() {
        let editor = include_str!("curve_editor.rs");
        assert!(crate::FAMILY_LIB.contains("pub mod curve_editor;"));
        assert!(crate::FRONT_LIB.contains(" curve_editor::*,"));
        let start = crate::FAMILY_LIB.find("pub fn editors_mod(").expect("editors_mod");
        let family = &crate::FAMILY_LIB[start..];
        let at = |call: &str| {
            family.find(call).unwrap_or_else(|| panic!("{call} is not in editors_mod"))
        };
        assert!(
            at("crate::ease_editor::script_mod(vm);") < at("crate::curve_editor::script_mod(vm);"),
            "the curve editor registers after the ease editor"
        );
        let core = include_str!("../../../core/src/lib.rs");
        let core_start = core.find("pub fn widgets_mod_with(").expect("widgets_mod_with");
        let core_end = core.find("pub fn script_mod(vm: &mut ScriptVm)").expect("script_mod");
        for base in [
            "crate::view::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::radio_button::script_mod(vm);",
            "crate::badge::script_mod(vm);",
        ] {
            assert!(core[core_start..core_end].contains(base), "{base} is not the core's");
        }
        assert!(editor.contains("mod.widgets.CurveEditorBase = #(CurveEditor::register_widget(vm))"));
        assert_eq!(editor.matches("set_type_default() do mod.widgets.CurveEditorBase").count(), 1);
    }
}
