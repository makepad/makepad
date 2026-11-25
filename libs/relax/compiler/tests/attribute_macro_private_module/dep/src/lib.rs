pub use apm_mac::generate;
pub mod public_module { pub use apm_mac::generate; }

// Like zerovec: an exported macro and private module share the crate's name.
mod apm_dep {}
#[macro_export]
macro_rules! apm_dep { () => { 17u32 }; }

// An attribute re-exported beside a private module of its name: a use of the name from another
// crate is the attribute. `hidden` has no macro, and `secret`'s is imported privately: a use of
// either from another crate is E0603.
mod service {}
pub use apm_mac::service;
mod hidden {}
mod secret {}
#[allow(unused_imports)]
use apm_mac::service as secret;
