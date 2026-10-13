use relax_lex::lexer::{lex, lex_with, LexOptions, LineIndex, Mode, TokenLexError, Tok, T};

fn scan(source: &str, mode: Mode, trivia: bool) -> Vec<Tok> {
    let mut out = Vec::new();
    lex_with(source.as_bytes(), &LexOptions { edition: 2021, mode, trivia }, &mut out).unwrap();
    let eof = out.last().unwrap();
    assert_eq!((eof.kind, eof.lo, eof.hi), (T::Eof, source.len() as u32, source.len() as u32));
    out
}

fn atoms(source: &str, mode: Mode, trivia: bool) -> Vec<(T, &str)> {
    scan(source, mode, trivia).into_iter().filter(|t| t.kind != T::Eof)
        .map(|t| (t.kind, &source[t.lo as usize..t.hi as usize])).collect()
}

fn error(source: &str, mode: Mode) -> TokenLexError {
    lex_with(source.as_bytes(), &LexOptions { edition: 2021, mode, trivia: false }, &mut Vec::new()).unwrap_err()
}

#[test]
fn colours_preserve_spelling_and_boundaries() {
    let source = "#abc #abcd #ABCDEF #01234567 #xabc #xabcd #xABCDEF #x01234567 #[ #name #z";
    for mode in [Mode::Declarative, Mode::Gpu] {
        assert_eq!(atoms(source, mode, false), vec![
            (T::Colour, "#abc"), (T::Colour, "#abcd"), (T::Colour, "#ABCDEF"),
            (T::Colour, "#01234567"), (T::Colour, "#xabc"), (T::Colour, "#xabcd"),
            (T::Colour, "#xABCDEF"), (T::Colour, "#x01234567"), (T::Pound, "#"),
            (T::OpenBracket, "["), (T::Pound, "#"), (T::Ident, "name"),
            (T::Pound, "#"), (T::Ident, "z"),
        ]);
    }
}

#[test]
fn invalid_colours_match_error_code_span_and_message() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        for (source, hi) in [("#a", 2), ("#ab", 3), ("#abcde", 6), ("#abcdefg", 7),
            ("#1234567", 8), ("#123456789", 10), ("#x", 2), ("#xyz", 2),
            ("#abc_", 4), ("#abcZ", 4), ("#abcé", 4)] {
            let e = error(source, mode);
            assert_eq!((e.code, e.lo, e.hi, e.msg),
                (2, 0, hi, format!("invalid colour: {}", &source[..hi as usize])), "{source}");
        }
        let e = error("ok #a", mode);
        assert_eq!((e.lo, e.hi), (3, 5));
    }
}

#[test]
fn durations_are_dialect_atoms_including_gpu() {
    let source = "250ms 1.5s 2e3ms 1.s 1.suffix 1.5f32 0xffms 1us 1msec";
    for mode in [Mode::Declarative, Mode::Gpu] {
        assert_eq!(atoms(source, mode, false), vec![
            (T::Duration, "250ms"), (T::Duration, "1.5s"), (T::Duration, "2e3ms"),
            (T::Int, "1"), (T::Dot, "."), (T::Ident, "s"),
            (T::Int, "1"), (T::Dot, "."), (T::Ident, "suffix"),
            (T::Float, "1.5f32"), (T::Int, "0xffms"), (T::Int, "1us"), (T::Int, "1msec"),
        ]);
    }
    assert_eq!(atoms("250ms 1.5s", Mode::Rust, false), vec![(T::Int, "250ms"), (T::Float, "1.5s")]);
}

#[test]
fn at_ids_include_raw_prefix_and_do_not_require_a_body_after_it() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        assert_eq!(atoms("@name @r#x @r# @ @7 @_ := +:", mode, false), vec![
            (T::AtId, "@name"), (T::AtId, "@r#x"), (T::AtId, "@r#"), (T::At, "@"),
            (T::At, "@"), (T::Int, "7"), (T::AtId, "@_"), (T::ColonEq, ":="), (T::PlusColon, "+:"),
        ]);
    }
    assert_eq!(atoms("@r#x := +:", Mode::Rust, false), vec![
        (T::At, "@"), (T::RawIdent, "r#x"), (T::Colon, ":"), (T::Eq, "="), (T::Plus, "+"), (T::Colon, ":"),
    ]);
}

#[test]
fn dialect_punctuation_includes_apostrophe_other_and_separate_left_arrow() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        assert_eq!(atoms("'a 'r#x <- ` \\ \0 \u{7f} \u{80} >>= ..= ::", mode, false), vec![
            (T::Apostrophe, "'"), (T::Ident, "a"), (T::Apostrophe, "'"), (T::RawIdent, "r#x"),
            (T::Lt, "<"), (T::Minus, "-"), (T::OtherPunct, "`"), (T::OtherPunct, "\\"),
            (T::OtherPunct, "\0"), (T::OtherPunct, "\u{7f}"), (T::OtherPunct, "\u{80}"),
            (T::ShrEq, ">>="), (T::DotDotEq, "..="), (T::PathSep, "::"),
        ]);
    }
}

#[test]
fn comments_and_doc_comments_with_and_without_trivia() {
    let source = "// ordinary\n/// doc\n//// ordinary\n//! inner\n/* nested /* x */ */ /** doc */ /*** ordinary */ /*! inner */ /**/ x";
    let expected = vec![
        (T::Comment, "// ordinary"), (T::DocComment, "/// doc"), (T::Comment, "//// ordinary"),
        (T::DocComment, "//! inner"), (T::Comment, "/* nested /* x */ */"),
        (T::DocComment, " /** doc */".trim_start()), (T::Comment, "/*** ordinary */"),
        (T::DocComment, "/*! inner */"), (T::DocComment, "/**/"), (T::Ident, "x"),
    ];
    for mode in [Mode::Rust, Mode::Declarative, Mode::Gpu] {
        assert_eq!(atoms(source, mode, true), expected);
        assert_eq!(atoms(source, mode, false), vec![(T::Ident, "x")]);
    }
}

#[test]
fn unterminated_comments_and_literals_have_reference_diagnostics() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        for (source, msg) in [
            (" /* nested /* */", "invalid comment: unterminated block comment"),
            (" r##\"x\"#", "invalid string: unterminated raw literal"),
            (" br\"x", "invalid string: unterminated raw literal"),
            (" cr#\"x", "invalid string: unterminated raw literal"),
            (" \"x", "invalid literal: unterminated quote"),
            (" b'x", "invalid literal: unterminated quote"),
            (" c\"x", "invalid literal: unterminated quote"),
            (" '", "invalid literal: unterminated quote"),
        ] {
            let e = error(source, mode);
            assert_eq!((e.code, e.lo, e.hi, e.msg.as_str()), (2, 1, source.len() as u32, msg));
        }
    }
}

#[test]
fn dialect_numbers_validate_exponents_and_preserve_permissive_suffixes() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        for (source, hi) in [("1e", 2), ("1e+", 3), ("1E-foo", 3), ("1efoo", 2)] {
            let e = error(source, mode);
            assert_eq!((e.code, e.lo, e.hi, e.msg.as_str()), (2, 0, hi, "invalid number: missing exponent"));
        }
        assert_eq!(atoms("1e_ 1e+_ 0baf 0oef 1f32 1. 1..2 1._x 1µs", mode, false), vec![
            (T::Float, "1e_"), (T::Float, "1e+_"), (T::Int, "0baf"), (T::Int, "0oef"),
            (T::Float, "1f32"), (T::Float, "1."), (T::Int, "1"), (T::DotDot, ".."),
            (T::Int, "2"), (T::Int, "1"), (T::Dot, "."), (T::Ident, "_x"), (T::Int, "1µs"),
        ]);
    }
}

#[test]
fn dialect_non_ascii_identifiers_are_rejected_as_whole_atoms() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        for ident in ["é", "abcé", "r#é", "@é", "@r#é", "🦀", "x\u{feff}"] {
            let source = format!("  {ident}!");
            let e = error(&source, mode);
            assert_eq!((e.code, e.lo, e.hi, e.msg), (15, 2, 2 + ident.len() as u32, format!("identifier {ident} must be ASCII")));
        }
    }
}

#[test]
fn dialect_whitespace_bom_shebang_and_raw_identifier_boundaries() {
    for mode in [Mode::Declarative, Mode::Gpu] {
        assert_eq!(atoms("\u{feff}a\u{a0}b\u{2003}c", mode, false), vec![(T::Ident, "a"), (T::Ident, "b"), (T::Ident, "c")]);
        assert_eq!(atoms("#!/bin/x\nx", mode, true), vec![(T::Comment, "#!/bin/x"), (T::Ident, "x")]);
        assert_eq!(atoms("#! [x]\ny", mode, true), vec![(T::Comment, "#! [x]"), (T::Ident, "y")]);
        assert_eq!(atoms("\u{feff}#!x", mode, true), vec![(T::Pound, "#"), (T::Not, "!"), (T::Ident, "x")]);
        assert_eq!(atoms("r#1 r#?x r# fn", mode, false), vec![(T::RawIdent, "r#1"), (T::RawIdent, "r#?x"), (T::RawIdent, "r# fn")]);
        // The old dialect lexer panics on a trailing r#; keep the available token.
        assert_eq!(atoms("r#", mode, false), vec![(T::RawIdent, "r#")]);
    }
}

#[test]
fn dialect_literals_end_at_the_quote_and_allow_multi_character_chars() {
    let source = r###""x"suffix b"x"s c"x"s r#"x"#s br"x"s cr"x"s 'ab' '7x' b'x' '\é'"###;
    let expected = vec![
        (T::Str, "\"x\""), (T::Ident, "suffix"), (T::ByteStr, "b\"x\""), (T::Ident, "s"),
        (T::CStr, "c\"x\""), (T::Ident, "s"), (T::RawStr, "r#\"x\"#"), (T::Ident, "s"),
        (T::ByteStr, "br\"x\""), (T::Ident, "s"), (T::CStr, "cr\"x\""), (T::Ident, "s"),
        (T::Char, "'ab'"), (T::Char, "'7x'"), (T::Byte, "b'x'"), (T::Char, "'\\é'"),
    ];
    for mode in [Mode::Declarative, Mode::Gpu] { assert_eq!(atoms(source, mode, false), expected); }
}

#[test]
fn rust_literals_lifetimes_and_shift_keep_relax_rules() {
    let source = r###"r#"a"# br"b" cr##"c"## 'a 'static 'r#raw 'x' '\n' b'7' "s"suffix >> <- 1e+ 1.5f32"###;
    assert_eq!(atoms(source, Mode::Rust, false), vec![
        (T::RawStr, "r#\"a\"#"), (T::ByteStr, "br\"b\""), (T::CStr, "cr##\"c\"##"),
        (T::Lifetime, "'a"), (T::Lifetime, "'static"), (T::Lifetime, "'r#raw"),
        (T::Char, "'x'"), (T::Char, "'\\n'"), (T::Byte, "b'7'"), (T::Str, "\"s\"suffix"),
        (T::Shr, ">>"), (T::LArrow, "<-"), (T::Int, "1e"), (T::Plus, "+"), (T::Float, "1.5f32"),
    ]);
}

#[test]
fn rust_keywords_follow_edition_in_every_mode() {
    for mode in [Mode::Rust, Mode::Declarative, Mode::Gpu] {
        for edition in [2015, 2018, 2021, 2024] {
            let mut out = Vec::new();
            lex_with(b"async await dyn try fn r#fn", &LexOptions { edition, mode, trivia: false }, &mut out).unwrap();
            let mut expected = if edition == 2015 { vec![T::Ident; 4] } else { vec![T::KwAsync, T::KwAwait, T::KwDyn, T::KwTry] };
            expected.extend([T::KwFn, T::RawIdent, T::Eof]);
            assert_eq!(out.iter().map(|t| t.kind).collect::<Vec<_>>(), expected);
        }
    }
}

#[test]
fn rust_entry_points_agree_including_errors_and_appending() {
    for source in ["", "\u{feff}#!/bin/x\nx", "#! [x]", "x /* broken", "x \"broken", "x '7", "'", "`", "é\u{a0}x", "r#x >>"] {
        let sentinel = Tok { kind: T::Interp, lo: 7, hi: 9 };
        let mut a = vec![sentinel];
        let mut b = vec![sentinel];
        let old = lex(source.as_bytes(), 2021, &mut a).err();
        let new = lex_with(source.as_bytes(), &LexOptions { edition: 2021, mode: Mode::Rust, trivia: false }, &mut b).err();
        assert_eq!(old.map(|e| (e.pos, e.msg.to_owned())), new.map(|e| (e.lo, e.msg)));
        let flatten = |ts: &[Tok]| ts.iter().map(|t| (t.kind, t.lo, t.hi)).collect::<Vec<_>>();
        assert_eq!(flatten(&a), flatten(&b));
    }
}

#[test]
fn dialect_rejects_invalid_utf8_without_touching_output() {
    let mut out = Vec::new();
    let e = lex_with(b"x\xff", &LexOptions { edition: 2021, mode: Mode::Gpu, trivia: true }, &mut out).unwrap_err();
    assert_eq!((e.code, e.lo, e.hi, e.msg.as_str()), (2, 1, 2, "invalid source: expected UTF-8"));
    assert!(out.is_empty());
}

#[test]
fn line_index_counts_characters_and_handles_crlf_eof_and_clamping() {
    for source in ["", "a\n", "\r\n\n", "\u{feff}é\t🦀\r\nx\n最後", "one long ascii line"] {
        let index = LineIndex::new(source.as_bytes());
        for pos in source.char_indices().map(|(p, _)| p).chain([source.len()]) {
            let prefix = &source[..pos];
            let start = prefix.rfind('\n').map_or(0, |p| p + 1);
            assert_eq!(index.line_col(pos as u32), (prefix.bytes().filter(|&b| b == b'\n').count() + 1, source[start..pos].chars().count() + 1));
        }
        assert_eq!(index.line_col(u32::MAX), index.line_col(source.len() as u32));
    }
    assert_eq!(relax_lex::lexer::line_col("éx".as_bytes(), 2), (1, 3));
    assert_eq!(LineIndex::new("éx".as_bytes()).line_col(2), (1, 2));
}

#[test]
fn token_layout_and_appended_variants_remain_stable() {
    assert_eq!(std::mem::size_of::<Tok>(), 9);
    assert_eq!(T::Eof as u8, 0);
    assert_eq!(T::RawBody as u8, 2);
    let appended = [T::Colour, T::Duration, T::AtId, T::ColonEq, T::PlusColon, T::Comment, T::DocComment, T::Apostrophe, T::OtherPunct];
    for (i, kind) in appended.iter().enumerate() { assert_eq!(*kind as u8, T::Interp as u8 + 1 + i as u8); }
}
