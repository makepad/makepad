#![cfg_attr(feature = "nightly", feature(proc_macro_span))]

use proc_macro::{TokenStream};

mod derive_live;
use crate::derive_live::*;

mod derive_live_atomic;
use crate::derive_live_atomic::*;

mod derive_live_hook;
use crate::derive_live_hook::*;

mod derive_live_read;
use crate::derive_live_read::*;


mod live_register_macro;
use crate::live_register_macro::*;

mod live_macro;
use crate::live_macro::*;

mod generate_cast;
use crate::generate_cast::*;

mod derive_live_registry;
use crate::derive_live_registry::*;

#[path = "../../live_tokenizer/src/colorhex.rs"]
mod colorhex;

#[proc_macro_derive(Live, attributes(
    alias,
    calc,
    live,
    rust,
    pick,
    state,
    live_register,
    live_ignore,
    live_debug
))]
pub fn derive_live(input: TokenStream) -> TokenStream {
    derive_live_impl(input)
}

#[proc_macro_derive(LiveHook)]
pub fn derive_live_apply(input: TokenStream) -> TokenStream {
    derive_live_hook_impl(input)
}

#[proc_macro_derive(LiveRead)]
pub fn derive_live_read(input: TokenStream) -> TokenStream {
    derive_live_read_impl(input)
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
pub fn generate_ref_cast_api(input: TokenStream) -> TokenStream {
    generate_ref_cast_api_impl(input)
}

#[proc_macro]
pub fn generate_clone_cast_api(input: TokenStream) -> TokenStream {
    generate_clone_cast_api_impl(input)
}

#[proc_macro]
pub fn live_register(input: TokenStream) -> TokenStream {
    live_register_impl(input)
}

#[proc_macro_derive(LiveComponentRegistry)]
pub fn derive_live_component_registry(input: TokenStream) -> TokenStream {
    derive_live_component_registry_impl(input)
}

#[proc_macro_derive(LiveAtomic)]
pub fn derive_live_atomic(input: TokenStream) -> TokenStream {
    derive_live_atomic_impl(input)
}

