//! Video, the playback bar, level meters, the marquee, content placeholders
//! and media frames.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it
//! (the `media` feature). These widgets were the core's until they moved
//! here: few apps use them, and every app built on the core waited for them.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod level_meter;
pub mod marquee;
pub mod media;
pub mod placeholder;
pub mod playback_bar;
pub mod video;

/// Registers the family, after every widget of the core, in the order the
/// core registered these widgets before they moved here.
pub fn media_mod(vm: &mut ScriptVm) {
    crate::playback_bar::script_mod(vm);
    crate::level_meter::script_mod(vm);
    crate::marquee::script_mod(vm);
    crate::placeholder::script_mod(vm);
    crate::media::script_mod(vm);
    crate::video::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with this family.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(vm, WindowFamilies::default(), &[media_mod]);
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
// crate's `media_mod`, and that is the text searched here.

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
        let fam_start = family.find("pub fn media_mod(").expect("media_mod");
        let fam_end = fam_start + family[fam_start..].find("\n}\n").expect("end of media_mod");
        format!("{}{}", &core[start..end], &family[fam_start..fam_end])
    })
}

#[cfg(test)]
mod marquee_registration_tests {
    /// The marquee registers after the view it draws inside and carries
    /// exactly one preset. It draws text and no children on purpose, so it
    /// must not grow a `#[deref] view` and start redrawing child widgets
    /// several times a frame — see the module's own reasoning.
    #[test]
    fn test_marquee_is_registered_after_its_bases() {
        let lib = crate::FAMILY_LIB;
        let calls = crate::widgets_mod_source();
        let marquee = include_str!("marquee.rs");
        assert!(lib.contains("pub mod marquee;"));
        assert!(crate::FRONT_LIB.contains("marquee::*"));
        let at = calls.find("crate::marquee::script_mod(vm);").expect("marquee registered");
        for base in ["crate::view::script_mod(vm);", "crate::label::script_mod(vm);"] {
            assert!(calls.find(base).expect(base) < at, "{base} must register before marquee");
        }
        assert!(marquee.contains("mod.widgets.MarqueeBase = #(Marquee::register_widget(vm))"));
        assert_eq!(marquee.matches("set_type_default() do mod.widgets.MarqueeBase").count(), 1);
    }
}
