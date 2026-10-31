use proc_macro::TokenStream;

// The expansion logic lives in the library crate makepad-script-derive-impl (Rapid links it
// directly); these are the rustc proc-macro entry points.
use makepad_script_derive_impl::*;

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

#[proc_macro_derive(
    Script,
    attributes(
        apply_default,
        apply_state,
        source,
        new,
        live,
        imperative,
        rust,
        pick,
        script_splat,
        walk,
        layout,
        deref,
        uid,
    )
)]

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
