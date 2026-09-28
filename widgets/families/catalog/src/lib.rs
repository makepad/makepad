//! Alerts and banners, accordions, breadcrumbs, pagination, floating panels,
//! spinners, toasts, empty states, code blocks and the tour.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `catalog` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod accordion;
pub mod alert;
pub mod breadcrumb;
pub mod code_block;
pub mod empty_state;
pub mod floating_panel;
pub mod pagination;
pub mod spinner;
pub mod toast;
pub mod tour;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn catalog_mod(vm: &mut ScriptVm) {
    crate::alert::script_mod(vm);
    crate::accordion::script_mod(vm);
    crate::breadcrumb::script_mod(vm);
    crate::pagination::script_mod(vm);
    crate::floating_panel::script_mod(vm);
    crate::spinner::script_mod(vm);
    crate::toast::script_mod(vm);
    crate::empty_state::script_mod(vm);
    crate::code_block::script_mod(vm);
    crate::tour::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with this family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[catalog_mod]);
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
// crate's `catalog_mod`, and that is the text searched here.

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
        let fam_start = family.find("pub fn catalog_mod(").expect("catalog_mod");
        let fam_end = fam_start + family[fam_start..].find("\n}\n").expect("end of catalog_mod");
        format!("{}{}", &core[start..end], &family[fam_start..fam_end])
    })
}

#[cfg(test)]
mod breadcrumb_registration_tests {
    /// The trail registers after the view and label it draws with, and
    /// carries exactly one preset.
    #[test]
    fn test_breadcrumb_is_registered_after_its_bases() {
        let lib = crate::FAMILY_LIB;
        let calls = crate::widgets_mod_source();
        let breadcrumb = include_str!("breadcrumb.rs");
        assert!(lib.contains("pub mod breadcrumb;"));
        assert!(crate::FRONT_LIB.contains("breadcrumb::*"));
        let at = calls.find("crate::breadcrumb::script_mod(vm);").expect("breadcrumb registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(calls.find(base).expect(base) < at, "{base} must register before breadcrumb");
        }
        assert!(breadcrumb.contains("mod.widgets.BreadcrumbBase = #(Breadcrumb::register_widget(vm))"));
        assert_eq!(
            breadcrumb.matches("set_type_default() do mod.widgets.BreadcrumbBase").count(),
            1
        );
    }
}

#[cfg(test)]
mod pagination_registration_tests {
    /// The strip registers after the view and label it draws with, carries
    /// exactly one preset, and keeps its arithmetic a free function: the
    /// window is what the tests are about, the drawing is not.
    #[test]
    fn test_pagination_is_registered_after_its_bases() {
        let lib = crate::FAMILY_LIB;
        let calls = crate::widgets_mod_source();
        let pagination = include_str!("pagination.rs");
        assert!(lib.contains("pub mod pagination;"));
        assert!(crate::FRONT_LIB.contains("pagination::*"));
        let at = calls.find("crate::pagination::script_mod(vm);").expect("pagination registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(calls.find(base).expect(base) < at, "{base} must register before pagination");
        }
        assert!(pagination.contains("mod.widgets.PaginationBase = #(Pagination::register_widget(vm))"));
        assert_eq!(
            pagination.matches("set_type_default() do mod.widgets.PaginationBase").count(),
            1
        );
        assert!(pagination.contains("pub fn page_window("), "the arithmetic stays a free function");
    }
}

#[cfg(test)]
mod spinner_registration_tests {
    /// The spinner family registers after the button, label, view and
    /// glass modules it composes, and leaves `loading_spinner` untouched:
    /// that DSL-only view has shader parameters seven apps override by name.
    #[test]
    fn test_spinner_is_registered_after_its_bases() {
        let lib = crate::FAMILY_LIB;
        let calls = crate::widgets_mod_source();
        let spinner = include_str!("spinner.rs");
        assert!(lib.contains("pub mod spinner;"));
        assert!(crate::FRONT_LIB.contains("spinner::*"));
        let at = calls.find("crate::spinner::script_mod(vm);").expect("spinner registered");
        for base in [
            "crate::view::script_mod(vm);",
            "crate::label::script_mod(vm);",
            "crate::button::script_mod(vm);",
            "crate::gauss_view::script_mod(vm);",
            "crate::loading_spinner::script_mod(vm);",
        ] {
            assert!(calls.find(base).expect(base) < at, "{base} must register before spinner");
        }
        assert!(spinner.contains("mod.widgets.SpinnerBase = #(Spinner::register_widget(vm))"));
        assert!(spinner.contains("mod.widgets.SpinnerFlat = set_type_default()"));
        assert_eq!(spinner.matches("set_type_default() do mod.widgets.SpinnerBase").count(), 1);
        assert!(!spinner.contains("mod.widgets.LoadingSpinner"));
    }
}
