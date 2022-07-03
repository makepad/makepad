//#![feature(proc_macro_span)]
use proc_macro::{TokenStream};


mod derive_wasm_bridge;
use derive_wasm_bridge::*;

#[proc_macro_derive(ToWasm)]
pub fn derive_to_wasm(input: TokenStream) -> TokenStream {
    derive_to_wasm_impl(input)
}

#[proc_macro_derive(FromWasm)]
pub fn derive_de_wasm_msg(input: TokenStream) -> TokenStream {
    derive_from_wasm_impl(input)
}
