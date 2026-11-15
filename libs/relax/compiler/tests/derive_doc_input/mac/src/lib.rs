//! `#[derive(Echo)]` and `#[echo_attr]`: the item as the macro received it, with each group's
//! delimiter, as a string constant on the type.
use proc_macro::{Delimiter, TokenStream, TokenTree};

fn describe(ts: TokenStream) -> String {
    ts.into_iter().map(|t| match t {
        TokenTree::Group(g) => {
            let d = match g.delimiter() { Delimiter::None => "None", Delimiter::Parenthesis => "Paren", Delimiter::Brace => "Brace", Delimiter::Bracket => "Bracket" };
            format!("{d}[{}]", describe(g.stream()))
        }
        other => other.to_string(),
    }).collect::<Vec<_>>().join(" ")
}

fn type_name(item: &TokenStream) -> String {
    let mut next = false;
    for t in item.clone() {
        if next {
            return t.to_string();
        }
        next = matches!(&t, TokenTree::Ident(i) if i.to_string() == "struct");
    }
    String::new()
}

#[proc_macro_derive(Echo)]
pub fn echo(input: TokenStream) -> TokenStream {
    format!("impl {} {{ pub const INPUT: &'static str = {:?}; }}", type_name(&input), describe(input)).parse().unwrap()
}

#[proc_macro_attribute]
pub fn echo_attr(_args: TokenStream, item: TokenStream) -> TokenStream {
    let tail: TokenStream = format!("impl {} {{ pub const INPUT: &'static str = {:?}; }}", type_name(&item), describe(item.clone())).parse().unwrap();
    let mut out = item;
    out.extend(tail);
    out
}
