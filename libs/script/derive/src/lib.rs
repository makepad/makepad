use proc_macro::{TokenStream};

mod script;
mod derive_scriptable;
mod swizzle;
use script::*;
use derive_scriptable::*;
use swizzle::*;

#[proc_macro]
pub fn script(input: TokenStream) -> TokenStream {
    script_impl(input)
}

#[proc_macro_derive(Script, attributes(
    script,
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