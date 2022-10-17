use proc_macro::{TokenStream};

mod derive_widget;
use crate::derive_widget::*;

#[proc_macro_derive(WidgetAction)]
pub fn derive_widget_action(input: TokenStream) -> TokenStream {
    derive_widget_action_impl(input)
}

#[proc_macro_derive(Widget)]
pub fn derive_widget(input: TokenStream) -> TokenStream {
    derive_widget_impl(input)
}

#[proc_macro_derive(WidgetRef)]
pub fn derive_widget_ref(input: TokenStream) -> TokenStream {
    derive_widget_ref_impl(input)
}
