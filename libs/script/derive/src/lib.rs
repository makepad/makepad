extern crate proc_macro;
use proc_macro::TokenStream;

#[proc_macro_derive(Script)]
pub fn derive_script(_input: TokenStream) -> TokenStream {
    Default::default()
}
