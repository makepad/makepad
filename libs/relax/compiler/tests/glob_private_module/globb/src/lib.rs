// A private module named like an extern crate, its items re-exported by glob (the shape of
// makepad_micro_serde: `mod serde_json; pub use crate::serde_json::*;`). The module itself is not
// exported, so a glob of this crate does not bind the name `realj`.
mod realj {
    pub struct Marker;
}
pub use crate::realj::*;
