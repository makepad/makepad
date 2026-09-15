//! Asset creation through reusable Flow graphs and isolated run instances.
//!
//! [`flow::CreatorFlow`] attaches to a remote flow-server or owns an embedded
//! host. [`engine`] translates existing named pipeline work orders into Flow
//! graphs and observes their results; Flow owns scheduling and cancellation.
//! [`runner`] submits single-generation graphs, then validates and publishes
//! completed assets through asset-client. All blocking calls belong on workers.
//!
//! Keep the Flow session alive for the app's lifetime. Dropping a remote
//! attachment leaves its server and submitted instances intact. Explicit
//! cancellation affects only the invocation owned by that caller.

pub mod engine;
pub mod character;
pub mod composite;
pub mod pipeline;
pub mod runner;
pub mod tools;
pub mod presets;
#[cfg(not(target_arch="wasm32"))]
pub mod flow;

pub use makepad_ai_hub;
pub use makepad_strict_json;

#[cfg(not(target_arch="wasm32"))]
mod flow_graph;
