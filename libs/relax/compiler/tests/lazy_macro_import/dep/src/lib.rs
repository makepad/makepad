//! A crate that re-exports an attribute macro of its own name: `use lmi_dep::lmi_dep;` binds a
//! macro only, and `lmi_dep::sync::Mutex` beside it goes through the crate.
pub use lmi_mac::lmi_dep;

pub mod sync {
    pub use std::sync::Mutex;
}
