// A second `subtag!`, of this module only, re-exported from a private module by a path whose
// first segment could have been a crate: an import that is no reason to leave the `subtag!`
// of unicode.rs and transform.rs unresolved.
mod other;

pub use other::{subtag, Subtag};

pub const X: Subtag = subtag!("x");
