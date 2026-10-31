//! Literal validation of the Rust scanner (rustc_parse `cook_lexer_literal`, rustc_literal_escaper).
use relax_lex::lexer::lex;

fn err(src: &str, edition: u16) -> Option<String> {
    relax_lex::lexer::set_validation(true);
    let mut toks = Vec::new();
    lex(src.as_bytes(), edition, &mut toks).map_err(|e| e.msg.into_owned()).and_then(|_| relax_lex::lexer::validate_file(src.as_bytes(), &toks, edition).map_err(|e| e.msg.into_owned())).err()
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
