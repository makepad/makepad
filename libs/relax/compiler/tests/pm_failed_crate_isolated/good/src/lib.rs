use proc_macro::TokenStream;

/// `answer!()`: 42.
#[proc_macro]
pub fn answer(_: TokenStream) -> TokenStream {
    "42".parse().unwrap()
}
