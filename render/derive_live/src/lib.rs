#![feature(proc_macro_span)]
use proc_macro::{TokenStream};

mod derive_live;
use crate::derive_live::*;

mod live_register_macro;
use crate::live_register_macro::*;

mod live_macro;
use crate::live_macro::*;

#[path = "../../microserde/derive/src/macro_lib.rs"]
mod macro_lib;

#[path = "../../live_compiler/src/id.rs"]
mod id;

#[proc_macro_derive(LiveComponent, attributes(calc, live, hide, pick))]
pub fn derive_live(input: TokenStream) -> TokenStream {
    derive_live_component_impl(input)
}

#[proc_macro_derive(LiveComponentHooks)]
pub fn derive_live_component_hooks(input: TokenStream) -> TokenStream {
    derive_live_component_hooks_impl(input)
}

#[proc_macro_derive(LiveAnimate)]
pub fn derive_live_animate(input: TokenStream) -> TokenStream {
    derive_live_animate_impl(input)
}

#[proc_macro]
pub fn live(input: TokenStream) -> TokenStream {
    live_impl(input)
}

#[proc_macro]
pub fn live_array(input: TokenStream) -> TokenStream {
    live_array_impl(input)
}

#[proc_macro]
pub fn live_object(input: TokenStream) -> TokenStream {
    live_object_impl(input)
}


#[proc_macro]
pub fn live_register(input: TokenStream) -> TokenStream {
    live_register_impl(input)
}



