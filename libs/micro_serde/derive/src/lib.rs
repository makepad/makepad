extern crate proc_macro;
use proc_macro::TokenStream;

// The expansion logic lives in the library crate makepad-micro-serde-derive-impl (relax links
// it directly); these are the rustc proc-macro entry points.
use makepad_micro_serde_derive_impl::*;

#[proc_macro_derive(SerBin)]
pub fn derive_ser_bin(input: TokenStream) -> TokenStream {
    derive_ser_bin_impl(input)
}

#[proc_macro_derive(DeBin)]
pub fn derive_de_bin(input: TokenStream) -> TokenStream {
    derive_de_bin_impl(input)
}

#[proc_macro_derive(SerJson, attributes(rename))]
pub fn derive_ser_json(input: TokenStream) -> TokenStream {
    derive_ser_json_impl(input)
}

#[proc_macro_derive(DeJson, attributes(rename))]
pub fn derive_de_json(input: TokenStream) -> TokenStream {
    derive_de_json_impl(input)
}

#[proc_macro_derive(SerRon)]
pub fn derive_ser_ron(input: TokenStream) -> TokenStream {
    derive_ser_ron_impl(input)
}

#[proc_macro_derive(DeRon)]
pub fn derive_de_ron(input: TokenStream) -> TokenStream {
    derive_de_ron_impl(input)
}
