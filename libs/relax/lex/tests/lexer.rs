use relax_lex::lexer::{lex, lex_with, LexOptions, LineIndex, Tok, T};

fn scan(source: &str, trivia: bool) -> Vec<Tok> {
    let mut out = Vec::new();
    lex_with(source.as_bytes(), &LexOptions { edition: 2021, trivia }, &mut out).unwrap();
    let eof = out.last().unwrap();
    assert_eq!((eof.kind, eof.lo, eof.hi), (T::Eof, source.len() as u32, source.len() as u32));
    out
}

fn atoms(source: &str, trivia: bool) -> Vec<(T, &str)> {
    scan(source, trivia).into_iter().filter(|t| t.kind != T::Eof)
        .map(|t| (t.kind, &source[t.lo as usize..t.hi as usize])).collect()
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
    assert_eq!(atoms(source, true), expected);
    assert_eq!(atoms(source, false), vec![(T::Ident, "x")]);
}

#[test]
fn rust_literals_lifetimes_and_shift_keep_relax_rules() {
    let source = r###"r#"a"# br"b" cr##"c"## 'a 'static 'r#raw 'x' '\n' b'7' "s"suffix >> <- 1e+ 1.5f32"###;
    assert_eq!(atoms(source, false), vec![
        (T::RawStr, "r#\"a\"#"), (T::ByteStr, "br\"b\""), (T::CStr, "cr##\"c\"##"),
        (T::Lifetime, "'a"), (T::Lifetime, "'static"), (T::Lifetime, "'r#raw"),
        (T::Char, "'x'"), (T::Char, "'\\n'"), (T::Byte, "b'7'"), (T::Str, "\"s\"suffix"),
        (T::Shr, ">>"), (T::LArrow, "<-"), (T::Int, "1e"), (T::Plus, "+"), (T::Float, "1.5f32"),
    ]);
}

#[test]
fn rust_keywords_follow_edition() {
    for edition in [2015, 2018, 2021, 2024] {
        let mut out = Vec::new();
        lex_with(b"async await dyn try fn r#fn", &LexOptions { edition, trivia: false }, &mut out).unwrap();
        let mut expected = if edition == 2015 { vec![T::Ident; 4] } else { vec![T::KwAsync, T::KwAwait, T::KwDyn, T::KwTry] };
        expected.extend([T::KwFn, T::RawIdent, T::Eof]);
        assert_eq!(out.iter().map(|t| t.kind).collect::<Vec<_>>(), expected);
    }
}

#[test]
fn rust_entry_points_agree_including_errors_and_appending() {
    for source in ["", "\u{feff}#!/bin/x\nx", "#! [x]", "x /* broken", "x \"broken", "x '7", "'", "`", "é\u{a0}x", "r#x >>"] {
        let sentinel = Tok { kind: T::Interp, lo: 7, hi: 9 };
        let mut a = vec![sentinel];
        let mut b = vec![sentinel];
        let old = lex(source.as_bytes(), 2021, &mut a).err();
        let new = lex_with(source.as_bytes(), &LexOptions { edition: 2021, trivia: false }, &mut b).err();
        assert_eq!(old.map(|e| (e.pos, e.msg.into_owned())), new.map(|e| (e.lo, e.msg)));
        let flatten = |ts: &[Tok]| ts.iter().map(|t| (t.kind, t.lo, t.hi)).collect::<Vec<_>>();
        assert_eq!(flatten(&a), flatten(&b));
    }
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
fn token_layout_and_trivia_variants_remain_stable() {
    assert_eq!(std::mem::size_of::<Tok>(), 9);
    assert_eq!(T::Eof as u8, 0);
    assert_eq!(T::RawBody as u8, 2);
    let appended = [T::Comment, T::DocComment];
    for (i, kind) in appended.iter().enumerate() { assert_eq!(*kind as u8, T::Interp as u8 + 1 + i as u8); }
}
