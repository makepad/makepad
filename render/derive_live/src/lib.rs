#![feature(proc_macro_span)]
use proc_macro::{TokenStream};

mod derive_live;
use crate::derive_live::*;

mod live_register;
use crate::live_register::*;

mod apply;
use crate::apply::*;

#[path = "../../microserde/derive/src/macro_lib.rs"]
mod macro_lib;

#[path = "../../live_parser/src/id.rs"]
mod id;

#[proc_macro_derive(LiveComponent, attributes(local, live, hidden, default))]
pub fn derive_live(input: TokenStream) -> TokenStream {
    derive_live_component_impl(input)
}

#[proc_macro_derive(LiveComponentHooks)]
pub fn derive_live_component_hooks(input: TokenStream) -> TokenStream {
    derive_live_component_hooks_impl(input)
}

#[proc_macro]
pub fn nodes(input: TokenStream) -> TokenStream {
    nodes_impl(input)
}

#[proc_macro]
pub fn live_register(input: TokenStream) -> TokenStream {
    live_register_impl(input)

}



