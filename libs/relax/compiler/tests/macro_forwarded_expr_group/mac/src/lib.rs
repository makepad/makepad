//! Two macros that read their input as syn reads an expression: through an invisible group
//! (`Delimiter::None`, syn's `Expr::Group`), never through parentheses (`Expr::Paren`).
//! sqlx's expand_query! reads `source = "sql" + "sql"` and an argument's `as _` this way.
use proc_macro::{Delimiter, TokenStream, TokenTree};

/// The trees of `ts`, looked into while they are one invisible group.
fn peel(ts: TokenStream) -> Vec<TokenTree> {
    let trees: Vec<TokenTree> = ts.into_iter().collect();
    match trees.as_slice() {
        [TokenTree::Group(g)] if g.delimiter() == Delimiter::None => peel(g.stream()),
        _ => trees,
    }
}

/// `joined!(source = "a" + "b")`: the string literals, concatenated, as one string literal.
#[proc_macro]
pub fn joined(input: TokenStream) -> TokenStream {
    let mut it = input.into_iter();
    assert_eq!(it.next().map(|t| t.to_string()).as_deref(), Some("source"));
    assert_eq!(it.next().map(|t| t.to_string()).as_deref(), Some("="));
    let mut out = String::new();
    for (k, t) in peel(it.collect()).into_iter().enumerate() {
        // a `tt` forwarded inside the expression is the literal itself
        let t = match peel(TokenStream::from(t)).as_slice() { [one] => one.clone(), _ => panic!("expected string literal") };
        match (k % 2, t) {
            (0, TokenTree::Literal(l)) if l.to_string().starts_with('"') => out.push_str(l.to_string().trim_matches('"')),
            (1, TokenTree::Punct(p)) if p.as_char() == '+' => {}
            _ => panic!("expected string literal"),
        }
    }
    format!("{:?}", out).parse().unwrap()
}

/// `inferred!(x as _)`: whether the expression is a cast to `_` (the cast is the whole of it).
#[proc_macro]
pub fn inferred(input: TokenStream) -> TokenStream {
    let trees = peel(input);
    let n = trees.len();
    let cast = n >= 3 && trees[n - 2].to_string() == "as" && trees[n - 1].to_string() == "_";
    format!("{cast}").parse().unwrap()
}

/// `same!(..)`: its input as it came (an invisible group in it stays one).
#[proc_macro]
pub fn same(input: TokenStream) -> TokenStream {
    input
}

fn describe(ts: TokenStream) -> String {
    ts.into_iter().map(|t| match t {
        TokenTree::Group(g) => {
            let d = match g.delimiter() { Delimiter::None => "None", Delimiter::Parenthesis => "Paren", Delimiter::Brace => "Brace", Delimiter::Bracket => "Bracket" };
            format!("{d}[{}]", describe(g.stream()))
        }
        other => other.to_string(),
    }).collect::<Vec<_>>().join(" ")
}

/// `shape!(..)`: how its input reached the macro, as a string literal: a group's delimiter and
/// contents (`None[..]`, `Paren[..]`), else the token.
#[proc_macro]
pub fn shape(input: TokenStream) -> TokenStream {
    format!("{:?}", describe(input)).parse().unwrap()
}
