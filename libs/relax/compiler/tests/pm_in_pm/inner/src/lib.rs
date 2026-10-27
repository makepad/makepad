//! `#[derive(Hello)]`: `impl Hello for <name>` returning the type's name (no syn: token trees only).
use proc_macro::{TokenStream, TokenTree};

#[proc_macro_derive(Hello)]
pub fn derive_hello(input: TokenStream) -> TokenStream {
    let mut it = input.into_iter();
    let mut name = None;
    while let Some(t) = it.next() {
        if let TokenTree::Ident(i) = &t {
            if i.to_string() == "struct" || i.to_string() == "enum" {
                if let Some(TokenTree::Ident(n)) = it.next() {
                    name = Some(n.to_string());
                }
                break;
            }
        }
    }
    let name = name.expect("derive(Hello) on a struct or enum");
    format!("impl ::pmpm_hello::Hello for {name} {{ fn hello() -> &'static str {{ \"{name}\" }} }}").parse().unwrap()
}
