use proc_macro::{TokenStream};

mod script;
use script::*;

#[proc_macro]
pub fn script(input: TokenStream) -> TokenStream {
    script_impl(input)
}