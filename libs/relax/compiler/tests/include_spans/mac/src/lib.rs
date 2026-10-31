use proc_macro::{Span, TokenStream, TokenTree};

fn describe(input: TokenStream, out: &mut String) {
    for token in input {
        let span = token.span();
        out.push_str(&format!(
            "{} {}:{}-{}:{}\n",
            token,
            span.start().line(),
            span.start().column(),
            span.end().line(),
            span.end().column(),
        ));
        if let TokenTree::Group(group) = token {
            describe(group.stream(), out);
        }
    }
}

#[proc_macro]
pub fn spans(input: TokenStream) -> TokenStream {
    let mut out = String::new();
    describe(input, &mut out);
    format!("{:?}", out).parse().unwrap()
}

// Preserve authored spans through a second proc-macro invocation.
#[proc_macro]
pub fn forward(input: TokenStream) -> TokenStream {
    input
}

// file! must use the token's file, including through generated output.
#[proc_macro]
pub fn authored_file(input: TokenStream) -> TokenStream {
    let span = input.into_iter().next().unwrap().span();
    let output: TokenStream = "file!()".parse().unwrap();
    output.into_iter().map(|mut t| { t.set_span(span); t }).collect()
}

#[proc_macro]
pub fn call_location(_input: TokenStream) -> TokenStream {
    let span = Span::call_site();
    format!("({}usize, {}usize)", span.start().line(), span.start().column()).parse().unwrap()
}

#[proc_macro_attribute]
pub fn attribute_spans(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut out = String::new();
    describe(item, &mut out);
    format!("const ATTRIBUTE: &str = {:?};", out).parse().unwrap()
}

#[proc_macro_derive(DeriveSpans)]
pub fn derive_spans(item: TokenStream) -> TokenStream {
    let mut out = String::new();
    describe(item, &mut out);
    format!("const DERIVED: &str = {:?};", out).parse().unwrap()
}
