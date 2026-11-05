// An edition 2021 proc-macro crate used by an edition 2024 app. Tokens a macro makes (parsed
// strings, `Group::new`: `Span::call_site()`) have this crate's edition; tokens passed through
// from the input keep the app's (rustc ExpnData::edition, rustc_span hygiene.rs).
use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

/// A block whose tail makes a temporary: in 2021 it is dropped after the block's local.
#[proc_macro]
pub fn fresh_block(_: TokenStream) -> TokenStream {
    "{ let _local = crate::Noisy(\"local\"); crate::Noisy(\"temp\").get() }".parse().unwrap()
}

/// The input unchanged.
#[proc_macro]
pub fn carried(input: TokenStream) -> TokenStream {
    input
}

/// The input statements in braces made here: the block's span is the macro's.
#[proc_macro]
pub fn fresh_braces(input: TokenStream) -> TokenStream {
    TokenTree::Group(Group::new(Delimiter::Brace, input)).into()
}

/// `if let` with an `else`: in 2021 the scrutinee's temporary lives through the `else`.
#[proc_macro]
pub fn fresh_if_let(_: TokenStream) -> TokenStream {
    "if let Some(_) = crate::Noisy(\"scrutinee\").none() {} else { crate::log(\"else\") }".parse().unwrap()
}

/// `.into_iter()` on a `Box<[u8]>`: in 2021 the method call iterates by reference.
#[proc_macro]
pub fn fresh_into_iter(_: TokenStream) -> TokenStream {
    "crate::name_of(crate::boxed().into_iter().next().unwrap())".parse().unwrap()
}

/// A `macro_rules!` made here, its body a block with a tail temporary.
#[proc_macro]
pub fn fresh_rules(_: TokenStream) -> TokenStream {
    "macro_rules! made_tail { () => { { let _local = crate::Noisy(\"local\"); crate::Noisy(\"temp\").get() } }; }".parse().unwrap()
}
