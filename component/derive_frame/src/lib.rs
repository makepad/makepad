#![feature(proc_macro_span)]
use proc_macro::{TokenStream};

mod derive_frame;
use crate::derive_frame::*;

// move elsewhere
#[proc_macro_derive(FrameAction)]
pub fn derive_frame_action(input: TokenStream) -> TokenStream {
    derive_frame_action_impl(input)
}
// move elsewhere
#[proc_macro_derive(FrameComponent)]
pub fn derive_frame_component(input: TokenStream) -> TokenStream {
    derive_frame_component_impl(input)
}

