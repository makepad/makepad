//! An implementation crate as proc-macro-hack's users write one (unic-langid-macros-impl).
use pmh_hack::proc_macro_hack;
use proc_macro::TokenStream;

#[proc_macro_hack]
pub fn answer(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}
