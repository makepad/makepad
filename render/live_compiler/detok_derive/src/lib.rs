use proc_macro::{TokenStream};
mod derive_detok; 
use crate::derive_detok::*;

#[path = "../../../microserde/derive/src/macro_lib.rs"]
mod macro_lib; 

#[proc_macro_derive(DeTok)]
pub fn derive_de_tok(input: TokenStream) -> TokenStream {
    derive_de_tok_impl(input)
}

#[proc_macro_derive(DeTokSplat)]
pub fn derive_de_tok_splat(input: TokenStream) -> TokenStream {
    derive_de_tok_splat_impl(input)
}

