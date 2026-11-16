use proc_macro::TokenStream;

// The expansion lives in relax-api-macro-impl (Relax links it directly); this is the
// rustc proc-macro entry point.
use relax_api_macro_impl::*;

#[proc_macro_attribute]
pub fn relax_api(attr: TokenStream, item: TokenStream) -> TokenStream {
    relax_api_impl(attr, item)
}

/// Export layouts for Options spelled in unmarked items of a provided crate.
#[proc_macro]
pub fn option_layouts(input: TokenStream) -> TokenStream {
    option_layouts_impl(input)
}
