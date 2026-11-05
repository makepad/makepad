use proc_macro::{TokenStream, TokenTree};
use std::collections::HashMap;

/// `#[derive(Probe)]` on `struct X`: `impl crate::Probe for X { fn fields() -> usize { n } }`. It
/// keeps its names in a HashMap, as serde_derive does: the macro world's thread-locals (std's hash
/// keys, proc_macro's call site) get values on the thread that expands it.
#[proc_macro_derive(Probe)]
pub fn derive_probe(item: TokenStream) -> TokenStream {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut name = String::new();
    let mut it = item.into_iter().peekable();
    while let Some(t) = it.next() {
        match &t {
            TokenTree::Ident(i) if i.to_string() == "struct" => {
                if let Some(TokenTree::Ident(n)) = it.next() { name = n.to_string(); }
            }
            TokenTree::Group(g) => {
                for f in g.stream() {
                    if let TokenTree::Ident(i) = f {
                        let n = seen.len();
                        seen.entry(i.to_string()).or_insert(n);
                    }
                }
            }
            _ => {}
        }
    }
    let fields = seen.keys().filter(|k| k.as_str() != "u32" && k.as_str() != "String").count();
    format!("impl crate::Probe for {name} {{ fn fields() -> usize {{ {fields} }} }}").parse().unwrap()
}
