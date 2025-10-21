use proc_macro::{TokenStream};

mod script;
mod derive_scriptable;
use script::*;
use derive_scriptable::*;

#[proc_macro]
pub fn script(input: TokenStream) -> TokenStream {
    script_impl(input)
}

#[proc_macro_derive(Scriptable, attributes(
    script,
    rust,
    pick,
    walk,
    layout,
    deref,
))]

pub fn derive_scriptable(input: TokenStream) -> TokenStream {
    derive_scriptable_impl(input)
}
