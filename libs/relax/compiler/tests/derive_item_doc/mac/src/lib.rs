//! `#[derive(Attrs)]`: an `attrs()` that lists the outer attributes the derive input carries, the
//! item's own (doc comments among them, as `#[doc = ".."]`) and its fields', as the macro saw them.
extern crate proc_macro;
use proc_macro::{Delimiter, TokenStream, TokenTree};

fn attrs_of(trees: &[TokenTree], out: &mut Vec<String>) {
    let mut i = 0;
    while i < trees.len() {
        if let (TokenTree::Punct(p), Some(TokenTree::Group(g))) = (&trees[i], trees.get(i + 1)) {
            if p.as_char() == '#' && g.delimiter() == Delimiter::Bracket {
                out.push(g.stream().to_string());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
}

#[proc_macro_derive(Attrs)]
pub fn attrs(input: TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = input.into_iter().collect();
    let mut name = String::new();
    let mut seen_kw = false;
    let mut item = Vec::new();
    let mut fields = Vec::new();
    for t in &trees {
        match t {
            TokenTree::Ident(i) if seen_kw && name.is_empty() => name = i.to_string(),
            TokenTree::Ident(i) if matches!(i.to_string().as_str(), "struct" | "enum") => seen_kw = true,
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                attrs_of(&inner, &mut fields);
            }
            _ => {}
        }
    }
    let head: Vec<TokenTree> = trees.iter().take_while(|t| !matches!(t, TokenTree::Ident(i) if matches!(i.to_string().as_str(), "struct" | "enum" | "pub"))).cloned().collect();
    attrs_of(&head, &mut item);
    let text = format!("item [{}] fields [{}]", item.join(" | "), fields.join(" | "));
    format!("impl {name} {{ pub fn attrs() -> &'static str {{ {text:?} }} }}").parse().unwrap()
}
