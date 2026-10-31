//! Projection shared by opt-in macro declaration contracts. Types retain their
//! fields for subsequent attributes/derives; function and impl bodies are opaque
//! to the file-level name/impl-head index. Macro definitions remain executable
//! token templates because another declaration can invoke them.
use crate::proc_macro::{Delimiter, Group, TokenStream, TokenTree};

pub fn declaration_surface(input: TokenStream) -> TokenStream {
    let mut output = TokenStream::new();
    let mut item = String::new();
    let mut macro_item = false;
    for token in input {
        match token {
            TokenTree::Ident(ref name) if item.is_empty() || item == "const" && name.to_string() == "fn" => {
                let name = name.to_string();
                if matches!(name.as_str(), "fn" | "impl" | "mod" | "struct" | "enum" | "union" | "trait" | "const" | "static" | "use" | "type" | "macro_rules" | "macro") {
                    item = name;
                }
                output.extend([token]);
            }
            TokenTree::Punct(ref punct) if punct.as_char() == '!' => {
                macro_item = true;
                output.extend([token]);
            }
            TokenTree::Punct(ref punct) if punct.as_char() == ';' => {
                item.clear();
                macro_item = false;
                output.extend([token]);
            }
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => {
                let body = if macro_item { group.stream() } else {
                    match item.as_str() {
                        "fn" | "impl" => TokenStream::new(),
                        "mod" | "const" | "static" => declaration_surface(group.stream()),
                        _ => group.stream(),
                    }
                };
                let mut projected = Group::new(Delimiter::Brace, body);
                projected.set_span(group.span());
                output.extend([TokenTree::Group(projected)]);
                item.clear();
                macro_item = false;
            }
            other => output.extend([other]),
        }
    }
    output
}
