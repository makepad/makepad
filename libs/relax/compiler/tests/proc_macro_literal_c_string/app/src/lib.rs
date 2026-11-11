//! `proc_macro::Literal::c_string` (1.79): the literal a macro returns is the C string it was
//! given, and its text is what rustc writes: valid UTF-8 as `char::escape_debug` gives it with a
//! `'` left alone, any other byte as `\x..`.

#[test]
fn a_c_string_literal_from_a_proc_macro() {
    let (lit, text): (&std::ffi::CStr, &str) = clit_macro::c_lit!();
    assert_eq!(lit.to_bytes(), b"a\"b'\n\t\\\x01\xc3\xa9\xff");
    assert_eq!(text, r#"c"a\"b'\n\t\\\u{1}é\xff""#);
}
