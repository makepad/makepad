// In-housed TOML parser (Relax's crate resolution reads manifests and
// Cargo.lock with it), so the relax tree builds on its own.
mod parser;
pub use self::parser::*;
