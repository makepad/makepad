//! A stand-in for proc-macro-hack 0.5's attribute on the implementation side: `#[proc_macro_hack]
//! pub fn name(..)` becomes the derive entry point `_proc_macro_hack_name`, which the declaration
//! crate imports by that name. And `cargo_set!()`: whether a proc macro runs with CARGO set, as
//! cargo runs them (sqlx's query macros run `$CARGO metadata`).
use proc_macro::{Ident, TokenStream, TokenTree};

#[proc_macro_attribute]
pub fn proc_macro_hack(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut trees: Vec<TokenTree> = item.into_iter().collect();
    let at = 1 + trees.iter().position(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "fn")).expect("#[proc_macro_hack] on a fn");
    let entry = format!("_proc_macro_hack_{}", trees[at]);
    trees[at] = TokenTree::Ident(Ident::new(&entry, trees[at].span()));
    let mut out: TokenStream = format!("#[proc_macro_derive({entry})]").parse().unwrap();
    out.extend(trees);
    out
}

#[proc_macro]
pub fn cargo_set(_input: TokenStream) -> TokenStream {
    std::env::var_os("CARGO").is_some().to_string().parse().unwrap()
}
