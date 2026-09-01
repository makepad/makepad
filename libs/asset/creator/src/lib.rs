//! makepad-asset-creator: the pipeline library (aicore.md §9).
//!
//! **The store stores. The client creates.** A creator app does not author
//! graphs — it picks a named pipeline from this library, parameterises it,
//! and consumes the result:
//!
//! ```text
//!   creator app (vj / sandbox / fab / asset-ui / a headless runner)
//!     ├── asset-creator ── the pipelines, by name, + the engine that runs one
//!     │        ├── ai-hub ──▶ every stage executes here
//!     │        └── combines stage results into a finished asset
//!     └── asset-client ──▶ upload ──▶ asset-server
//! ```
//!
//! Laws carried over from the systems this replaces:
//! - **State is never stored, it is derived** ([`pipeline::derive_state`]):
//!   a run's state is computed from its stage states on every read, so the
//!   record "can no more disagree with its jobs than a sum can disagree with
//!   its addends" (the store's own v6 pipeline law, kept verbatim).
//! - **Stage weights live in ONE place** — here — "precisely so two clients
//!   cannot disagree" (the other store law, finally true by construction).
//! - **Entropy is pinned at submission**: a stage re-picked onto another node
//!   must regenerate identical content, so seeds are part of the spec, never
//!   drawn at execution time.
//! - The run lives with the creating app. A run that must outlive a window
//!   is a client that does not close (a headless runner / the machine node),
//!   never a scheduler in the database.

pub mod engine;
pub mod pipeline;
pub mod runner;
pub mod tools;
pub mod presets;

pub use makepad_ai_hub;
pub use makepad_strict_json;
