//! Two function-like macros, as time-macros has `format_description` and
//! `serde_format_description`.
use proc_macro::TokenStream;

#[proc_macro]
pub fn describe(_input: TokenStream) -> TokenStream {
    "\"plain\"".parse().unwrap()
}

#[proc_macro]
pub fn tagged_describe(input: TokenStream) -> TokenStream {
    // (its input is another grammar: a name, a comma, a literal)
    assert_eq!(input.into_iter().count(), 3, "tagged_describe takes `name, literal`");
    "\"tagged\"".parse().unwrap()
}
