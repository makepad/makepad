//! Token-building / parsing helpers for makepad's proc macros, over a switchable token crate:
//! `makepad_micro_proc_macro::proc_macro` is rustc's `proc_macro` normally, and a plain-data
//! `proc_macro`-compatible model with the `relax-tokens` feature (for proc-macro libraries linked into
//! relax). Proc-macro libraries name the token types through this re-export.

#[cfg(not(feature = "relax-tokens"))]
pub extern crate proc_macro;

#[allow(dead_code)]
pub mod macro_lib;
pub use macro_lib::*;

#[cfg(feature = "relax-tokens")]
pub use relax_lex::tokens as proc_macro;

#[cfg(feature = "relax-tokens")]
mod declaration;
#[cfg(feature = "relax-tokens")]
pub use declaration::declaration_surface;
