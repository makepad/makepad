//! Reusable 3D design framework.
//!
//! The crate owns the document/scene model, area-based UI shell, navigation,
//! tools, realtime viewport, progressive ray-tracing seam, sheet UI, and tour
//! panel. Applications only compose this shell with one or more format
//! loaders implementing [`model::Loader`].

pub use makepad_widgets;

pub mod arch;
pub mod api;
pub mod document;
pub mod loader;
pub mod model;
pub mod nav;
pub mod providers;
pub mod render;
pub mod sheets;
pub mod theme;
pub mod tools;
pub mod tour;
pub mod ui;
pub mod viewport;

pub use api::*;
pub use document::{Document, DocumentBuilder, Edit};
pub use loader::LoadCoordinator;
pub use model::{DocumentProvider, Loader, SceneProvider};

use makepad_widgets::ScriptVm;

/// Register every framework widget and shader in dependency order.
pub fn script_mod(vm: &mut ScriptVm) {
    theme::script_mod(vm);
    ui::script_mod_kit(vm);
    viewport::script_mod(vm);
    nav::script_mod(vm);
    tools::script_mod(vm);
    sheets::script_mod(vm);
    render::script_mod(vm);
    tour::script_mod(vm);
    ui::script_mod(vm);
}
