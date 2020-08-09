extern crate proc_macro;
use proc_macro::{TokenStream};

mod macro_lib; 

mod derive_bin;
use crate::derive_bin::*;

mod derive_ron;
use crate::derive_ron::*;

mod derive_json;
use crate::derive_json::*;

#[proc_macro_derive(SerBin)]
pub fn derive_ser_bin(input: TokenStream) -> TokenStream {
    derive_ser_bin_impl(input)
}

#[proc_macro_derive(DeBin)]
pub fn derive_de_bin(input: TokenStream) -> TokenStream {
    derive_de_bin_impl(input)
}

#[proc_macro_derive(SerJson)]
pub fn derive_ser_json(input: TokenStream) -> TokenStream {
    derive_ser_json_impl(input)
}

#[proc_macro_derive(DeJson)]
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

