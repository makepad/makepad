//! Literal validation of the Rust scanner (rustc_parse `cook_lexer_literal`, rustc_literal_escaper).
use relax_lex::lexer::lex;

/// A file's first error where all of it is live: a lexer error, else the first literal error rustc
/// reports where it lowers the literal (the parser drops those of cfg-stripped code).
fn err(src: &str, edition: u16) -> Option<String> {
    let mut toks = Vec::new();
    if let Err(e) = lex(src.as_bytes(), edition, &mut toks) {
        return Some(e.msg.into_owned());
    }
    match relax_lex::lexer::validate_file(src.as_bytes(), &toks, edition) {
        Err(e) => Some(e.msg.into_owned()),
        Ok(lowers) => lowers.into_iter().next().map(|e| e.msg.into_owned()),
    }
}

#[test]
fn valid_literals_pass() {
    for s in [
        r#"let a = "a\n\t\\\"\'\0\x41\u{1F600}\u{00_41}";"#, "let b = b'\\xff'; let c = b\"\\xff\";", "let d = c\"\\xff\";",
        "let e = 0b1010_u8; let f = 0o777; let g = 0xdead_beefu64; let h = 1_000.5e-3_f64; let i = 1e10;",
        "let j = r#\"raw \\ \"# ; let k = '\\''; let l = 'a'; let m = \"line \\\n   continues\";",
        "let r#abstract = 1; let ñandú = 2; let 变量 = 3;", "let n = 340282366920938463463374607431768211455u128;",
    ] {
        assert_eq!(err(s, 2021), None, "{s}");
    }
}

#[test]
fn bad_escapes_and_digits_are_errors() {
    assert!(err("let a = \"\\q\";", 2021).unwrap().contains("unknown character escape"));
    assert!(err("let a = b\"\\q\";", 2021).unwrap().contains("unknown byte escape"));
    assert!(err("let a = \"\\xff\";", 2021).unwrap().contains("out of range hex escape"));
    assert!(err("let a = '\\x4';", 2021).unwrap().contains("too short"));
    assert!(err("let a = \"\\u{110000}\";", 2021).unwrap().contains("invalid unicode character escape"));
    assert!(err("let a = b\"\\u{41}\";", 2021).unwrap().contains("unicode escape in byte string"));
    assert!(err("let a = \"\\u{1234567}\";", 2021).unwrap().contains("overlong"));
    assert!(err("let a = 0b12;", 2021).unwrap().contains("invalid digit for a base 2 literal"));
    assert!(err("let a = 0o8;", 2021).unwrap().contains("invalid digit for a base 8 literal"));
    assert!(err("let a = 0x;", 2021).unwrap().contains("no valid digits"));
    assert!(err("let a = 1e;", 2021).unwrap().contains("exponent"));
    assert!(err("let a = 1is;", 2021).unwrap().contains("invalid suffix"));
    assert!(err("let a = 340282366920938463463374607431768211456;", 2021).unwrap().contains("too large"));
    assert!(err("let a = \"x\"y;", 2021).unwrap().contains("suffixes on string literals"));
    assert!(err("let a = \"x\"_;", 2021).unwrap().contains("underscore literal suffix"));
    assert!(err("let a = r#self;", 2021).unwrap().contains("raw identifier"));
    assert!(err("let a = b\"é\";", 2021).unwrap().contains("non-ASCII"));
    assert!(err("let 🦀 = 1;", 2021).unwrap().contains("Ferris"));
    assert!(err("let a = f\"x\";", 2021).unwrap().contains("is unknown"));
    assert_eq!(err("let a = f\"x\";", 2018), None);
}

#[test]
fn a_quoted_char_or_byte_holds_no_bare_newline_tab_cr_or_quote() {
    // rustc_literal_escaper `unescape_single`: EscapeOnlyChar and BareCarriageReturn. The flag of the
    // parser's diagnostics is never set in this file: every command is held to this
    let must_escape = |lit: &str, what: &str, shown: &str| {
        let want = format!("{what} constant must be escaped: `{shown}`");
        // a lexer rule: a macro's tokens and code that is configured out are held to it too
        for src in [format!("let a = {lit};"), format!("m!({lit});"), format!("macro_rules! n {{ () => {{ {lit} }} }}"), format!("#[cfg(any())] fn f() {{ {lit}; }}")] {
            assert_eq!(err(&src, 2021).as_deref(), Some(&*want), "{src:?}");
        }
    };
    must_escape("'\n'", "character", "\\n");
    must_escape("'\t'", "character", "\\t");
    must_escape("'''", "character", "'");
    must_escape("'\r'", "character", "\\r");
    must_escape("b'\n'", "byte", "\\n");
    must_escape("b'\t'", "byte", "\\t");
    // (BareCarriageReturn says "character" for a byte too: `UnescapeError::BareCr`)
    must_escape("b'\r'", "character", "\\r");
    for s in ["b''", "b'ab'", "b'\r\n'", "'\\q'", "'\\\n'", "'\\nx'", "'a'b"] {
        assert!(err(&format!("let a = {s};"), 2021).is_some(), "{s:?}");
    }
    // what they would have written
    for s in ["'\\n'", "'\\t'", "'\\''", "'\\r'", "b'\\n'", "b'\\''", "'\"'", "b'\"'", "\"a\nb\tc'\"", "\"a\r\nb\"", "r\"a\r\nb\""] {
        assert_eq!(err(&format!("let a = {s};"), 2021), None, "{s:?}");
    }
}

#[test]
fn neighbours_of_the_char_rule() {
    // rustc_literal_escaper `CStr::char2unit`
    assert!(err("let a = c\"\\u{0}\";", 2021).unwrap().contains("null characters"));
    // a raw string has a suffix as any other: `LitKind::descr` names the kind
    assert!(err("let a = r\"a\"b;", 2021).unwrap().contains("suffixes on string literals"));
    assert!(err("let a = r#\"a\"#b;", 2021).unwrap().contains("suffixes on string literals"));
    assert!(err("let a = br\"a\"b;", 2021).unwrap().contains("suffixes on byte string literals"));
    assert!(err("let a = cr\"a\"b;", 2021).unwrap().contains("suffixes on C string literals"));
    assert!(err("let a = b'a'b;", 2021).unwrap().contains("suffixes on byte literals"));
    assert!(err("let a = b\"a\"b;", 2021).unwrap().contains("suffixes on byte string literals"));
    assert!(err("let a = c\"a\"b;", 2021).unwrap().contains("suffixes on C string literals"));
    assert!(err("let a = r\"a\"_;", 2021).unwrap().contains("underscore literal suffix"));
    assert_eq!(err("m!(r\"a\"b);", 2021), None);
    // a macro or attribute rustc parses the arguments of, against one that keeps its tokens
    for s in ["let a = concat!(\"a\"b);", "#[doc = \"a\"b] fn f() {}", "#![doc = \"a\"b]", "#[cfg(feature = \"a\"b)] fn f() {}", "#[cfg(all(feature = \"a\"b))] fn f() {}", "let a = cfg!(feature = \"a\"b);"] {
        assert!(err(s, 2021).unwrap().contains("suffixes on string literals"), "{s}");
    }
    for s in ["#[a(x = \"a\"b)] fn f() {}", "#[cfg_attr(any(), a(x = \"a\"b))] fn f() {}", "#[doc = \"a\"] fn f() {}", "#[cfg(feature = \"a\")] fn f() {}"] {
        assert_eq!(err(s, 2021), None, "{s}");
    }
    assert_eq!(err("let a = r#\"a\"#;", 2021), None);
    // rustc_span source_map `read_file`
    let mut toks = Vec::new();
    let src = b"let a = \"\xff\";";
    assert!(lex(src, 2021, &mut toks).is_ok());
    assert!(relax_lex::lexer::validate_file(src, &toks, 2021).err().unwrap().msg.contains("valid UTF-8"));
    // what the parser cooks of an expansion
    assert!(relax_lex::lexer::validate_quoted(b"\"a\"b").unwrap_err().contains("suffixes on string literals"));
    assert!(relax_lex::lexer::validate_quoted(b"'a'b").unwrap_err().contains("suffixes on char literals"));
    assert!(relax_lex::lexer::validate_quoted(b"r#\"a\"#b").unwrap_err().contains("suffixes on string literals"));
    assert!(relax_lex::lexer::validate_quoted(b"b\"a\"b").unwrap_err().contains("suffixes on byte string literals"));
    for s in [&b"\"a\""[..], b"'a'", b"'\\''", b"b'a'", b"r#\"a\"#", b"br\"a\"", b"c\"a\""] {
        assert_eq!(relax_lex::lexer::validate_quoted(s), Ok(()));
    }
}

#[test]
fn literals_in_macro_token_trees_are_not_cooked_by_the_parser_checks() {
    // rustc judges a literal's suffix and size when the parser consumes it, never inside a macro
    // invocation's tokens (Splash colours: `#1a1b26`); the lexer's own errors still apply there
    for s in ["m!{ x: #1a1b26 }", "m!(1x, \"a\"b, 'c'd, 340282366920938463463374607431768211456)", "macro_rules! m { () => { 1x } }", "#[a(1x)] fn f() {}"] {
        assert_eq!(err(s, 2021), None, "{s}");
    }
    assert!(err("let a = 1x;", 2021).unwrap().contains("invalid suffix"));
    assert!(err("m!{ 0b102 }", 2021).unwrap().contains("invalid digit for a base 2 literal"));
    assert!(err("m!{ \"\\q\" }", 2021).unwrap().contains("unknown character escape"));
}

#[test]
fn std_macros_that_parse_expressions_cook_their_literals() {
    assert!(err("fn main() { println!(\"\"y); }", 2021).unwrap().contains("suffixes on string literals"));
    assert!(err("fn main() { let v = vec![1x]; }", 2021).unwrap().contains("invalid suffix"));
    assert_eq!(err("fn main() { m!(1x); }", 2021), None);
}
