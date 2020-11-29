use proc_macro::{TokenStream};
mod derive_draw; 
use crate::derive_draw::*;

#[path = "../../microserde/derive/src/macro_lib.rs"]
mod macro_lib; 

#[proc_macro_derive(DrawQuad, attributes(default_shader, custom_new))]
pub fn derive_draw_quad(input: TokenStream) -> TokenStream {
    derive_draw_impl(input, DrawType::DrawQuad)
}

#[proc_macro_derive(DrawText, attributes(default_shader, custom_new))]
pub fn derive_draw_text(input: TokenStream) -> TokenStream {
    derive_draw_impl(input, DrawType::DrawText)
}




