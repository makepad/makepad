// `late_helper` runs only for the input `1`, which the program's own expansion never passes.
extern crate proc_macro;
use proc_macro::TokenStream;

#[inline(never)]
fn late_helper() -> TokenStream {
    "42u32".parse().unwrap()
}

#[proc_macro]
pub fn value(input: TokenStream) -> TokenStream {
    if input.to_string() == "1" { late_helper() } else { "0u32".parse().unwrap() }
}
