//! An implementation crate as proc-macro-hack's users write one (unic-langid-macros-impl). Its
//! output is the body of the `proc_macro_call!` the derive defines, and says `$crate` for the
//! declaration crate: a proc macro's tokens are spanned at its call, inside that crate's macro.
use pmh_hack::proc_macro_hack;
use proc_macro::TokenStream;

#[proc_macro_hack]
pub fn answer(_input: TokenStream) -> TokenStream {
    "macro_rules! proc_macro_call { () => { $crate::ANSWER } }".parse().unwrap()
}
