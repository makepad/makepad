use proc_macro::{TokenStream};

mod derive_widget;
use crate::derive_widget::*;

// move elsewhere
#[proc_macro_derive(WidgetAction)]
pub fn derive_frame_action(input: TokenStream) -> TokenStream {
    derive_frame_action_impl(input)
}
// move elsewhere
#[proc_macro_derive(Widget)]
pub fn derive_frame_component(input: TokenStream) -> TokenStream {
    derive_frame_component_impl(input)
}

