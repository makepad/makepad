//! The widget catalogue.
//!
//! Every component of the widget library, organised by category, documented
//! in place and previewed live. A story is one named DSL template under
//! `mod.stories` plus a record in the registry that says where it belongs,
//! when it was added, what it documents and which of its properties the
//! controls panel may drive. The app is a navigator over that registry, a
//! canvas that instantiates one story at a time, and panels around it.
pub use makepad_widgets;

pub mod actions;
pub mod app;
pub mod canvas;
pub mod controls;
pub mod coverage;
pub mod docs;
pub mod knob;
pub mod navigator;
pub mod registry;
pub mod synonyms;
pub mod remote;
pub mod settings;
pub mod shell;
pub mod stories;
pub mod theme;
pub mod theme_panel;
