use proc_macro::TokenStream;

/// The item as it is. Named like the crate that re-exports it (lmi-dep), as `relax_api` is.
#[proc_macro_attribute]
pub fn lmi_dep(_: TokenStream, item: TokenStream) -> TokenStream {
    item
}
