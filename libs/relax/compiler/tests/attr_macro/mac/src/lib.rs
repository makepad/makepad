use proc_macro::{TokenStream, TokenTree, Delimiter};

/// `#[add_const(NAME = 7)] fn ...`: keeps the item and adds `const NAME: u32 = 7;`.
#[proc_macro_attribute]
pub fn add_const(attr: TokenStream, item: TokenStream) -> TokenStream {
    let a: Vec<TokenTree> = attr.into_iter().collect();
    let name = a[0].to_string();
    let val = a[2].to_string();
    let mut s = item.to_string();
    s.push_str(&format!("\npub const {}: u32 = {};", name, val));
    s.parse().unwrap()
}

/// `#[double_body]` on `fn f() -> u32 { e }`: the fn returns 2 * e (item rebuilt from tokens).
#[proc_macro_attribute]
pub fn double_body(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut out: Vec<TokenTree> = Vec::new();
    for t in item {
        match t {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                let inner = g.stream().to_string();
                let body: TokenStream = format!("{{ 2 * ({}) }}", inner).parse().unwrap();
                out.extend(body);
            }
            t => out.push(t),
        }
    }
    out.into_iter().collect()
}
