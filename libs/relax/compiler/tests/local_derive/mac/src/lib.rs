use proc_macro::{TokenStream, TokenTree};

/// `#[derive(Named)]` on `struct X` / `enum X`: `impl crate::Named for X { fn name(&self) -> &'static str { "X" } }`.
#[proc_macro_derive(Named)]
pub fn derive_named(item: TokenStream) -> TokenStream {
    let mut it = item.into_iter();
    let mut name = String::new();
    while let Some(t) = it.next() {
        if let TokenTree::Ident(i) = &t {
            let s = i.to_string();
            if s == "struct" || s == "enum" {
                if let Some(TokenTree::Ident(n)) = it.next() { name = n.to_string(); }
                break;
            }
        }
    }
    format!("impl crate::Named for {name} {{ fn name(&self) -> &'static str {{ \"{name}\" }} }}").parse().unwrap()
}
