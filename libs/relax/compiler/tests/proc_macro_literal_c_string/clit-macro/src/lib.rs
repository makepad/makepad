use proc_macro::{Delimiter, Group, Literal, Punct, Spacing, TokenStream, TokenTree};

/// `(<C string literal>, "<its text>")`: what `Literal::c_string` makes of a C string with a
/// quote of each kind, escapes, a control byte, a non-ASCII character and a byte that is not UTF-8.
#[proc_macro]
pub fn c_lit(_input: TokenStream) -> TokenStream {
    let lit = Literal::c_string(c"a\"b'\n\t\\\x01é\xff");
    let text = Literal::string(&lit.to_string());
    let pair: TokenStream =
        [TokenTree::Literal(lit), TokenTree::Punct(Punct::new(',', Spacing::Alone)), TokenTree::Literal(text)].into_iter().collect();
    TokenTree::Group(Group::new(Delimiter::Parenthesis, pair)).into()
}
