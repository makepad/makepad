use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

fn configured(item: &TokenStream) {
    for token in item.clone() {
        if let TokenTree::Group(g) = token {
            if g.delimiter() == Delimiter::Bracket {
                assert_ne!(g.stream().into_iter().next().unwrap().to_string(), "cfg_attr");
            }
        }
    }
}

// The fixture's traits have `async fn name(&self) -> T;` methods. Preserve the
// signature tokens except `async` and replace T with a boxed future of T.
fn boxed_methods(body: TokenStream, send: bool) -> TokenStream {
    let mut out = Vec::new();
    let mut tokens = body.into_iter();
    let mut asynchronous = false;
    while let Some(token) = tokens.next() {
        if matches!(&token, TokenTree::Ident(id) if id.to_string() == "async") {
            asynchronous = true;
            continue;
        }
        let arrow = asynchronous && matches!(&token, TokenTree::Punct(p) if p.as_char() == '>');
        out.push(token);
        if arrow {
            let mut output = Vec::new();
            while let Some(token) = tokens.next() {
                if matches!(&token, TokenTree::Punct(p) if p.as_char() == ';') { break; }
                output.push(token);
            }
            let output: TokenStream = output.into_iter().collect();
            let bound = if send { "+ Send" } else { "" };
            let boxed: TokenStream = format!(
                "::core::pin::Pin<Box<dyn ::core::future::Future<Output = {}> {} + '_>>;",
                output, bound,
            ).parse().unwrap();
            out.extend(boxed);
            asynchronous = false;
        }
    }
    out.into_iter().collect()
}

// On functions: m doubles the body; m(arg) adds one. On traits: box async
// method returns, accepting the same ?Send tokens as async_trait.
#[proc_macro_attribute]
pub fn m(attr: TokenStream, item: TokenStream) -> TokenStream {
    configured(&item);
    let args: String = attr.to_string().chars().filter(|c| !c.is_whitespace()).collect();
    assert!(args.is_empty() || args == "arg" || args == "?Send");
    let is_trait = item.clone().into_iter().any(|t| matches!(t, TokenTree::Ident(id) if id.to_string() == "trait"));
    item.into_iter().map(|token| match token {
        TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
            let body = if is_trait {
                boxed_methods(g.stream(), args != "?Send")
            } else {
                let expression = g.stream().to_string();
                let expression = if args == "arg" { format!("({}) + 1", expression) }
                    else { format!("2 * ({})", expression) };
                expression.parse().unwrap()
            };
            let mut body = Group::new(Delimiter::Brace, body);
            body.set_span(g.span());
            TokenTree::Group(body)
        }
        token => token,
    }).collect()
}

#[proc_macro_attribute]
pub fn boom(_attr: TokenStream, _item: TokenStream) -> TokenStream {
    panic!("inactive cfg_attr expanded");
}

#[proc_macro_attribute]
pub fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    m(attr, item)
}

#[proc_macro_attribute]
pub fn bench(attr: TokenStream, item: TokenStream) -> TokenStream {
    m(attr, item)
}

#[proc_macro_attribute]
pub fn check_attrs(attr: TokenStream, item: TokenStream) -> TokenStream {
    configured(&item);
    let mut paths = Vec::new();
    for token in item.clone() {
        if let TokenTree::Group(g) = token {
            if g.delimiter() == Delimiter::Bracket {
                let path: TokenStream = g.stream().into_iter().take_while(|t| match t {
                    TokenTree::Ident(_) => true,
                    TokenTree::Punct(p) => p.as_char() == ':',
                    _ => false,
                }).collect();
                paths.push(path.to_string());
            }
        }
    }
    let compact = |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    assert_eq!(compact(paths.join(",")), compact(attr.to_string()));
    item
}

#[proc_macro_derive(Marked)]
pub fn marked(item: TokenStream) -> TokenStream {
    configured(&item);
    let mut tokens = item.into_iter();
    while let Some(token) = tokens.next() {
        if matches!(token, TokenTree::Ident(id) if id.to_string() == "struct") {
            let name = tokens.next().unwrap();
            return format!("impl {} {{ pub fn derived() -> u32 {{ 17 }} }}", name).parse().unwrap();
        }
    }
    panic!("expected a struct");
}
