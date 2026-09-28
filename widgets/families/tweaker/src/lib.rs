//! The design overlay (Shift+F10) a Window carries, with the reflection and the saved-theme store it reads and writes.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! and the data family (the dock and file tree the panel is made of), has no
//! features of its own, and compiles once whatever mix of families an app
//! picks. Unlike the other families, `makepad-widgets` does not link it:
//! an app that carries the overlay depends on this crate (through its own
//! `tweaker` feature, on for development builds and off in the Builder's
//! releases) and calls [`link`] before `makepad_widgets::script_mod`:
//!
//! ```ignore
//! fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
//!     #[cfg(feature = "tweaker")]
//!     makepad_widgets_tweaker::link(vm);
//!     makepad_widgets::script_mod(vm);
//!     // ...
//! }
//! ```
//!
//! with, in its Cargo.toml, `tweaker = ["dep:makepad-widgets-tweaker", "makepad-widgets/fab",
//! "makepad-widgets/data", "makepad-widgets/dock"]` (the panel is made of
//! the fab family's controls, the data family's file tree and the dock).

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;
use makepad_widgets_data::{dock, file_tree};
use makepad_widgets_fab::{fab_controls, tween_inspector};

pub mod designer;
pub mod reflect;
pub mod theme_builder;
pub mod theme_combinations;
pub mod theme_lab;
pub mod theme_store;
pub mod tweaker;

/// Links the overlay into this `Cx`: every later `makepad_widgets::script_mod`
/// and `widgets_mod` (the first one and each theme rebuild) registers it
/// where the Window names it. Call it before the first `script_mod`.
pub fn link(vm: &mut ScriptVm) {
    vm.cx_mut().global::<WindowFamilies>().tweaker = Some(tweaker_mod);
}

/// Registers the overlay where the Window names it
/// ([`WindowFamilies::tweaker`]) and hands the core its hooks.
pub fn tweaker_mod(vm: &mut ScriptVm) {
    crate::tweaker::install_tweaker_hooks(vm.cx_mut());
    crate::tweaker::script_mod(vm);
}

// The whole registration the tests build on:
// the core with the overlay and the fab, data and dock families it opens.
#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(
        vm,
        WindowFamilies { tweaker: Some(tweaker_mod), ..Default::default() },
        &[makepad_widgets_fab::fab_mod, makepad_widgets_data::data_mod, makepad_widgets_data::dock_mod],
    );
}
