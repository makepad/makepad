pub use apm_mac::generate;
pub mod public_module { pub use apm_mac::generate; }

// Like zerovec: an exported macro and private module share the crate's name.
mod apm_dep {}
#[macro_export]
macro_rules! apm_dep { () => { 17u32 }; }
