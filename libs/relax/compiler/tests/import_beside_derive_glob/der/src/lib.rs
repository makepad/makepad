use proc_macro::TokenStream;

/// A derive named like the trait it goes with: a glob import of this crate puts `Item` in the
/// macro namespace of the importing module.
#[proc_macro_derive(Item)]
pub fn item(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}
