use proc_macro::{Literal, Spacing, TokenStream, TokenTree};

/// A string literal naming every top-level punct of the input with its spacing.
#[proc_macro]
pub fn spacings(input: TokenStream) -> TokenStream {
    let mut out = Vec::new();
    for tt in input {
        if let TokenTree::Punct(p) = tt {
            let s = match p.spacing() {
                Spacing::Joint => "Joint",
                Spacing::Alone => "Alone",
            };
            out.push(format!("'{}' {}", p.as_char(), s));
        }
    }
    TokenTree::Literal(Literal::string(&out.join(", "))).into()
}
