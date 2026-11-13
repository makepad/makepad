//! `literals!()`: each of proc_macro's Literal constructors on text that needs escaping, as the
//! text a macro sees (`TEXTS`, from `to_string`) and as tokens the program reads back (`VALUES`).
extern crate proc_macro;
use proc_macro::{Delimiter, Group, Literal, Punct, Spacing, TokenStream, TokenTree};
use std::ffi::CStr;

#[proc_macro]
pub fn literals(_: TokenStream) -> TokenStream {
    let c = CStr::from_bytes_with_nul(b"tab\there \"q\" 'a' \\ \xe2\x82\xac \x7f \xff\0").unwrap();
    let lits = [
        Literal::c_string(c),
        Literal::c_string(c"plain"),
        Literal::string("a\"b'c\\\n\t\0\u{301}\u{e9}\u{7f}"),
        Literal::character('\''),
        Literal::character('"'),
        Literal::character('\u{301}'),
        Literal::byte_character(b'\''),
        Literal::byte_character(b'"'),
        Literal::byte_character(0),
        Literal::byte_character(0xff),
        Literal::byte_string(b"q\"'\0\xff\x7f"),
    ];
    let mut out = String::from("pub const TEXTS: &[&str] = &[");
    for l in &lits {
        out.push_str(&format!("{:?}, ", l.to_string()));
    }
    out.push_str("]; pub const VALUES: (&::core::ffi::CStr, &::core::ffi::CStr, &str, char, char, char, u8, u8, u8, u8, &[u8]) = ");
    let mut ts: TokenStream = out.parse().unwrap();
    let mut tuple = Vec::new();
    for l in lits {
        tuple.push(TokenTree::Literal(l));
        tuple.push(TokenTree::Punct(Punct::new(',', Spacing::Alone)));
    }
    ts.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, tuple.into_iter().collect())), TokenTree::Punct(Punct::new(';', Spacing::Alone))]);
    ts
}
