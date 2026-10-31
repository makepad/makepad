use proc_macro::TokenStream;

// The expansion lives in makepad-relax-api-macro-impl (Relax links it directly); this is the
// rustc proc-macro entry point.
use makepad_relax_api_macro_impl::*;

#[proc_macro_attribute]
pub fn relax_api(attr: TokenStream, item: TokenStream) -> TokenStream {
    relax_api_impl(attr, item)
}
