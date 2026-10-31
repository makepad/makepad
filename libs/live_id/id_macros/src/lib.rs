// The expansion logic lives in the library crate makepad-live-id-macros-impl (relax links it
// directly); these are the rustc proc-macro entry points.
use proc_macro::TokenStream;

#[proc_macro]
pub fn live_id(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::live_id(item)
}

#[proc_macro]
pub fn some_id(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::some_id(item)
}

#[proc_macro]
pub fn id(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::id(item)
}

#[proc_macro]
pub fn ids(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::ids(item)
}

#[proc_macro]
pub fn ids_array(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::ids_array(item)
}

#[proc_macro]
pub fn ids_list(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::ids_list(item)
}

#[proc_macro]
pub fn live_id_num(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::live_id_num(item)
}

#[proc_macro]
pub fn id_lut(item: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::id_lut(item)
}

#[proc_macro_derive(FromLiveId)]
pub fn derive_from_live_id(input: TokenStream) -> TokenStream {
    makepad_live_id_macros_impl::derive_from_live_id_impl(input)
}
