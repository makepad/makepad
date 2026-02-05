use proc_macro::{TokenStream};

mod script;
mod derive_scriptable;
mod swizzle;
mod error;
use script::*;
use derive_scriptable::*;
use swizzle::*;
use error::*;

#[proc_macro]
pub fn script(input: TokenStream) -> TokenStream {
    script_impl(input)
}

#[proc_macro]
pub fn script_mod(input: TokenStream) -> TokenStream {
    script_mod_impl(input)
}

#[proc_macro]
pub fn script_apply_eval(input: TokenStream) -> TokenStream {
    script_apply_eval_impl(input)
}

#[proc_macro]
pub fn script_err_gen(input: TokenStream) -> TokenStream {
    script_err_gen_impl(input)
}


#[proc_macro_derive(Script, attributes(
    apply_default,
    source,
    new,
    live,
    rust,
    pick,
    splat,
    walk,
    layout,
    deref,
))]

pub fn derive_script(input: TokenStream) -> TokenStream {
    derive_script_impl(input)
}


#[proc_macro_derive(ScriptHook, attributes())]
pub fn derive_script_hook(input: TokenStream) -> TokenStream {
    derive_script_hook_impl(input)
}

#[proc_macro]
pub fn pod_swizzle_vec_match(input: TokenStream) -> TokenStream {
    pod_swizzle_vec_match_impl(input)
}

#[proc_macro]
pub fn pod_swizzle_vec_type(input: TokenStream) -> TokenStream {
    pod_swizzle_vec_type_impl(input)
}