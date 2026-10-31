use makepad_micro_proc_macro::{error, TokenBuilder};
use proc_macro::{Delimiter, Ident, TokenStream, TokenTree};

#[proc_macro_attribute]
pub fn makepad_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_makepad_test(attr, item).unwrap_or_else(error)
}

fn ident(token: Option<&TokenTree>, name: &str) -> bool {
    matches!(token, Some(TokenTree::Ident(id)) if id.to_string() == name)
}

fn punct(token: Option<&TokenTree>, ch: char) -> bool {
    matches!(token, Some(TokenTree::Punct(p)) if p.as_char() == ch)
}

fn expand_makepad_test(attr: TokenStream, item: TokenStream) -> Result<TokenStream, &'static str> {
    if !attr.is_empty() {
        return Err("#[makepad_test] does not accept arguments");
    }
    let tokens: Vec<_> = item.into_iter().collect();
    let mut pos = 0;
    let mut wrapper_attrs = TokenStream::new();
    let mut inner_attrs = TokenStream::new();
    while punct(tokens.get(pos), '#') {
        let Some(TokenTree::Group(group)) = tokens.get(pos + 1) else {
            return Err("expected an attribute");
        };
        if group.delimiter() != Delimiter::Bracket {
            return Err("expected an attribute");
        }
        let contents: Vec<_> = group.stream().into_iter().collect();
        let wrapper_only = (ident(contents.first(), "ignore")
            || ident(contents.first(), "should_panic"))
            && !punct(contents.get(1), ':');
        wrapper_attrs.extend(tokens[pos..pos + 2].iter().cloned());
        if !wrapper_only {
            inner_attrs.extend(tokens[pos..pos + 2].iter().cloned());
        }
        pos += 2;
    }
    let signature_start = pos;
    let vis_start = pos;
    if ident(tokens.get(pos), "pub") {
        pos += 1;
        if matches!(tokens.get(pos), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) {
            pos += 1;
        }
    }
    let visibility: TokenStream = tokens[vis_start..pos].iter().cloned().collect();
    // Preserve unsafe/extern qualifiers on the inner function, as before.
    while !ident(tokens.get(pos), "fn") {
        match tokens.get(pos) {
            Some(TokenTree::Ident(id)) if id.to_string() == "async" => {
                return Err("#[makepad_test] only supports synchronous tests");
            }
            Some(TokenTree::Ident(id)) if id.to_string() == "const" => {
                return Err("#[makepad_test] does not support const functions");
            }
            Some(TokenTree::Ident(id)) if matches!(id.to_string().as_str(), "unsafe" | "extern") => {}
            Some(TokenTree::Literal(_)) if ident(tokens.get(pos.wrapping_sub(1)), "extern") => {}
            _ => return Err("#[makepad_test] expects a function"),
        }
        pos += 1;
    }
    pos += 1;
    let Some(TokenTree::Ident(name)) = tokens.get(pos) else {
        return Err("expected a test function name");
    };
    let name_pos = pos;
    pos += 1;
    if punct(tokens.get(pos), '<') {
        // Rust accepts an empty generic parameter list; syn did too.
        if !punct(tokens.get(pos + 1), '>') {
            return Err("#[makepad_test] does not support generic test functions");
        }
        pos += 2;
    }
    let Some(TokenTree::Group(args)) = tokens.get(pos) else {
        return Err("expected test function arguments");
    };
    if args.delimiter() != Delimiter::Parenthesis {
        return Err("expected test function arguments");
    }
    let args: Vec<_> = args.stream().into_iter().collect();
    let mut end = args.len();
    if punct(args.last(), ',') { end -= 1; }
    if end == 0 {
        return Err("#[makepad_test] expects exactly one TestApp argument");
    }
    let mut depth = 0;
    for (i, token) in args[..end].iter().enumerate() {
        if let TokenTree::Punct(p) = token {
            match p.as_char() {
                '<' => depth += 1,
                '>' if !punct(i.checked_sub(1).and_then(|j| args.get(j)), '-') => depth -= 1,
                ',' if depth == 0 => return Err("#[makepad_test] expects exactly one TestApp argument"),
                _ => {}
            }
        }
    }
    let mut arg_pos = 0;
    // Parameter attributes are preserved in the original token stream.
    while punct(args.get(arg_pos), '#') {
        if !matches!(args.get(arg_pos + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket) {
            return Err("expected a parameter attribute");
        }
        arg_pos += 2;
    }
    if ident(args.get(arg_pos), "ref") { arg_pos += 1; }
    if ident(args.get(arg_pos), "mut") { arg_pos += 1; }
    if ident(args.get(arg_pos), "self") || punct(args.get(arg_pos), '&') {
        return Err("#[makepad_test] does not support methods");
    }
    if !matches!(args.get(arg_pos), Some(TokenTree::Ident(id)) if id.to_string() != "_")
        || !(punct(args.get(arg_pos + 1), ':') || punct(args.get(arg_pos + 1), '@'))
    {
        return Err("#[makepad_test] requires an identifier pattern for the TestApp argument");
    }
    if !matches!(tokens.last(), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace) {
        return Err("expected a test function body");
    }

    let inner_name = Ident::new(
        &format!("__makepad_test_inner_{}", name.to_string().trim_start_matches("r#")),
        name.span(),
    );
    let mut out = TokenBuilder::new();
    out.stream(Some(inner_attrs));
    for (i, token) in tokens.iter().enumerate().skip(signature_start) {
        out.extend(if i == name_pos { TokenTree::Ident(inner_name.clone()) } else { token.clone() });
    }
    out.stream(Some(wrapper_attrs))
        .add("#[test]")
        .stream(Some(visibility))
        .add("fn").extend(TokenTree::Ident(name.clone())).add("() {")
        .add("::makepad_test::__private::run_current_package_test(")
        .add("env!(").string("CARGO_PKG_NAME").add("),")
        .add("env!(").string("CARGO_MANIFEST_DIR").add("),")
        .add("module_path!(), stringify!(").extend(TokenTree::Ident(name.clone())).add("),")
        .extend(TokenTree::Ident(inner_name)).add("); }");
    Ok(out.end())
}
