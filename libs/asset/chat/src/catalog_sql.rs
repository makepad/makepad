//! Compatibility shim for the Asset Store-owned bounded catalog query.
//!
//! `makepad-asset-store` currently depends on this crate for the broker, so
//! a crate-level re-export would create a dependency cycle. Pointing this
//! private module at the single moved source keeps the broker compiling until
//! it is deleted without duplicating the implementation or its safety tests.

#[path = "../../store/src/host/assets_query.rs"]
mod assets_query;

pub use assets_query::*;
