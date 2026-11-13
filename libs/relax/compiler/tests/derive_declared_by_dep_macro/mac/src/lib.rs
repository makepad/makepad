use proc_macro::{TokenStream, TokenTree};

// The derive's entry point is the expansion of a dependency's macro (malloc_size_of_derive:
// `decl_derive!([MallocSizeOf, attributes(..)] => malloc_size_of_derive);`).
ddm_decl::decl_named!([Named, attributes(named_skip)] => named_derive);

/// `#[derive(Named)]` on `struct X`: `impl crate::NamedTrait for X { fn name(&self) -> &'static str { "X" } }`.
fn named_derive(item: TokenStream) -> TokenStream {
    let mut it = item.into_iter();
    let mut name = String::new();
    while let Some(t) = it.next() {
        if let TokenTree::Ident(i) = &t {
            if i.to_string() == "struct" {
                if let Some(TokenTree::Ident(n)) = it.next() { name = n.to_string(); }
                break;
            }
        }
    }
    format!("impl crate::NamedTrait for {name} {{ fn name(&self) -> &'static str {{ \"{name}\" }} }}").parse().unwrap()
}
