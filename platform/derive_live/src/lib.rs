#![cfg_attr(lines, feature(proc_macro_span))]

use proc_macro::{TokenStream};

mod derive_live;
use crate::derive_live::*;

mod derive_live_atomic;
use crate::derive_live_atomic::*;

mod derive_live_hook;
use crate::derive_live_hook::*;

mod derive_live_read;
use crate::derive_live_read::*;


mod live_design_macro;
use crate::live_design_macro::*;

mod live_macro;
use crate::live_macro::*;

mod generate_cast;
use crate::generate_cast::*;

mod derive_live_registry;
use crate::derive_live_registry::*;

//#[path = "../../live_tokenizer/src/colorhex.rs"]
//mod colorhex;

#[proc_macro_derive(Live, attributes(
    calc,
    live,
    rust,
    pick,
    animator,
    walk,
    layout,
    deref,
    designable,
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

#[proc_macro_derive(LiveRegister)]
pub fn derive_live_register(input: TokenStream) -> TokenStream {
    derive_live_register_impl(input)
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
pub fn generate_any_trait_api(input: TokenStream) -> TokenStream {
    generate_any_trait_api_impl(input)
}

#[proc_macro]
pub fn generate_any_send_trait_api(input: TokenStream) -> TokenStream {
    generate_any_send_trait_api_impl(input)
}

#[proc_macro]
pub fn live_design(input: TokenStream) -> TokenStream {
    live_design_impl(input)
}

#[proc_macro_derive(LiveComponentRegistry)]
pub fn derive_live_component_registry(input: TokenStream) -> TokenStream {
    derive_live_component_registry_impl(input)
}

#[proc_macro_derive(LiveAtomic)]
pub fn derive_live_atomic(input: TokenStream) -> TokenStream {
    derive_live_atomic_impl(input)
}

#[proc_macro_derive(DefaultNone)] 
pub fn derive_widget_action(input: TokenStream) -> TokenStream {
    derive_default_none_impl(input)
}

