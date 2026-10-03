//! Which Splash modules (`census`) and which top-level definitions of the
//! `script_mod!` blocks (`top_use`) an app used, learned by running it (a
//! collect run, a film's analysis): what the web build's optimiser strips.
//! Outside the VM crate: the VM only calls the recording census it is given
//! (makepad_script::census).

pub mod census;
pub mod top_use;

pub use census::{census_record, census_used, census_used_from, Census, ModuleUse};
pub use top_use::{top_statements, top_use, BlockUse, TopStatement};
