use relax_lex::lexer::{lex, register_raw_body, skip_braced, T};
use relax_lex::tokens::{from_toks, Span};

#[test]
fn registered_raw_bodies_stay_opaque_in_rust_and_expand_in_token_trees() {
    register_raw_body(&["opaque"]);
    let src = br##"opaque! { r#"}"#; {1} }"##;
    let mut toks = Vec::new();
    assert!(lex(src, 2021, &mut toks).is_ok());
    assert_eq!(toks.iter().map(|t| t.kind).collect::<Vec<_>>(), vec![T::Ident, T::Not, T::OpenBrace, T::RawBody, T::CloseBrace, T::Eof]);
    let body = toks[3];
    assert_eq!(&src[body.lo as usize..body.hi as usize], br##" r#"}"#; {1} "##);
    assert_eq!(skip_braced(src, 8), Some(src.len() - 1));
    let ts = from_toks(src, &toks, &|lo, hi| Span { lo, hi, file: 17, ..Span::DUMMY }).unwrap();
    assert_eq!(ts.to_string(), "opaque ! { r#\"}\"#; { 1 } }");
    let mut malformed = Vec::new();
    assert!(lex(b"opaque! {", 2021, &mut malformed).is_err());
    let mut bad_body = Vec::new();
    assert!(lex(b"opaque! {`}", 2021, &mut bad_body).is_ok());
    assert!(from_toks(b"opaque! {`}", &bad_body, &|_, _| Span::DUMMY).is_err());
}
