use proc_macro::{TokenStream};

mod script;
mod derive_scriptable;
use script::*;
use derive_scriptable::*;

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