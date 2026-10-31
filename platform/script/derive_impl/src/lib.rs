//! The expansion logic of makepad-script-derive as a plain library: `fn(TokenStream) ->
//! TokenStream` over `makepad_micro_proc_macro::proc_macro`. The proc-macro crate
//! (platform/script/derive) wraps these for rustc; relax links them into its binary
//! (feature `relax-tokens`). Pattern: local/plans/fast-rust.md "Proc macros as little libraries".

mod derive_scriptable;
#[allow(unstable_name_collisions)]
mod error;
#[allow(unstable_name_collisions)]
mod script;
#[allow(unstable_name_collisions)]
mod swizzle;

pub use derive_scriptable::{derive_script_hook_impl, derive_script_impl};
pub use error::script_err_gen_impl;
pub use script::{script_apply_eval_impl, script_impl, script_mod_impl};
pub use swizzle::{pod_swizzle_vec_match_impl, pod_swizzle_vec_type_impl};
