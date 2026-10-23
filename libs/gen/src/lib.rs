//! Generation for every app that offers it: the preset catalog (recipe documents that
//! the apps offer), the recipe checks as Flow nodes, the recipe digest that
//! is the one generation cache, the headless host a shell without a studio
//! runs presets on, and the `gen.*` tools every app registers with the same
//! text. A plain library: the hosts, the modelling runtime and the tool
//! registries call it directly. Hub jobs themselves run on the hub's
//! `GenJob` inside Flow's `Gen` node.

pub mod catalog;
pub mod character;
pub mod digest;
pub mod host;
pub mod nodes;
pub mod service;
pub mod tools;

pub use makepad_ai_hub;
