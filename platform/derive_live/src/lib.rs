#![feature(proc_macro_span)]
use proc_macro::{TokenStream};

mod derive_live;
use crate::derive_live::*;

mod live_register_macro;
use crate::live_register_macro::*;

mod live_macro;
use crate::live_macro::*;

mod derive_live_registry;
use crate::derive_live_registry::*;

mod derive_into_fcaction;
use crate::derive_into_fcaction::*;


#[path = "../../micro_serde/derive/src/macro_lib.rs"]
mod macro_lib;

#[path = "../../live_tokenizer/src/live_id.rs"]
mod live_id;

#[path = "../../live_tokenizer/src/colorhex.rs"]
mod colorhex;


#[proc_macro_derive(Live, attributes(
    calc,
    live,
    rust,
    pick,
    state,
    live_register,
))]
pub fn derive_live(input: TokenStream) -> TokenStream {
    derive_live_impl(input)
}

#[proc_macro_derive(LiveHook)]
pub fn derive_live_apply(input: TokenStream) -> TokenStream {
    derive_live_hook_impl(input)
}

#[proc_macro_derive(FromLiveId)]
pub fn derive_from_live_id(input: TokenStream) -> TokenStream {
    derive_from_live_id_impl(input)
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

// move elsewhere
#[proc_macro_derive(IntoFrameComponentAction)]
pub fn derive_into_frame_component_action(input: TokenStream) -> TokenStream {
    derive_into_frame_component_action_impl(input)
}

