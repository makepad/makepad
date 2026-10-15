use relax_lex::lexer::{lex, lex_with, LexOptions, LineIndex, Mode, Tok, T};
use relax_lex::tokens::{from_toks, Delimiter, LineColumn, Span, Spacing, TokenStream, TokenTree};

fn span(index: &LineIndex, lo: u32, hi: u32) -> Span {
    let (line, column) = index.line_col(lo);
    let start = LineColumn { line, column };
    let (line, column) = index.line_col(hi);
    Span { lo, hi, start, end: LineColumn { line, column }, file: 42 }
}

fn stream(source: &str, trivia: bool) -> TokenStream {
    let index = LineIndex::new(source.as_bytes());
    let mut toks = Vec::new();
    lex_with(source.as_bytes(), &LexOptions { edition: 2021, mode: Mode::Rust, trivia }, &mut toks).unwrap();
    from_toks(source.as_bytes(), &toks, &|lo, hi| span(&index, lo, hi)).unwrap()
}

fn atoms(source: &str) -> Vec<(T, String)> {
    let mut toks = Vec::new();
    assert!(lex(source.as_bytes(), 2021, &mut toks).is_ok());
    toks.into_iter().filter(|t| t.kind != T::Eof)
        .map(|t| (t.kind, source[t.lo as usize..t.hi as usize].into())).collect()
}

#[test]
fn rust_token_stream_round_trips() {
    for source in ["", "fn r#type<'a>(x: &'a str) -> Vec<Vec<u8>> { let y = [1, 2.0f32]; x::f(y); }",
        r###"r#"a"# br"b" cr##"c"## "s"suffix 'x' b'7'"###,
        "a >>= 2; b <<= 1; x..=y; x...y; x&&y || x!=y; $x:expr; @r#type",
        "&'r#async str",
    ] {
        let rendered = stream(source, false).to_string();
        assert_eq!(atoms(source), atoms(&rendered), "{source} -> {rendered}");
    }
}

#[test]
fn punctuation_spacing_and_lifetime_spans_match_relax() {
    let source = ">>=+= > > &'a 'r#raw";
    let ts = stream(source, false);
    let puncts: Vec<_> = ts.trees.iter().filter_map(|t| match t {
        TokenTree::Punct(p) => Some((p.as_char(), p.spacing(), p.span().lo, p.span().hi)),
        _ => None,
    }).collect();
    assert_eq!(puncts, vec![
        ('>', Spacing::Joint, 0, 1), ('>', Spacing::Joint, 1, 2), ('=', Spacing::Joint, 2, 3),
        ('+', Spacing::Joint, 3, 4), ('=', Spacing::Alone, 4, 5),
        ('>', Spacing::Alone, 6, 7), ('>', Spacing::Alone, 8, 9), ('&', Spacing::Alone, 10, 11),
        ('\'', Spacing::Joint, 11, 12), ('\'', Spacing::Joint, 14, 15),
    ]);
    let TokenTree::Ident(id) = &ts.trees[9] else { panic!("lifetime ident") };
    assert_eq!(id.to_string(), "a");
    assert_eq!((id.span().lo, id.span().hi), (12, 13));
    let TokenTree::Ident(id) = ts.trees.last().unwrap() else { panic!("raw lifetime ident") };
    assert!(id.raw);
    assert_eq!(id.to_string(), "r#raw");
}

#[test]
fn groups_and_each_token_keep_the_host_span() {
    let source = "fn f() {\n [é, 3]\n}";
    let ts = stream(source, false);
    let index = LineIndex::new(source.as_bytes());
    fn visit(ts: &TokenStream, index: &LineIndex) {
        for t in &ts.trees {
            let actual = t.span();
            assert_eq!(actual, span(index, actual.lo, actual.hi));
            if let TokenTree::Group(g) = t { visit(&g.stream, index); }
        }
    }
    visit(&ts, &index);
    let TokenTree::Group(g) = &ts.trees[3] else { panic!("body") };
    assert_eq!(g.delimiter(), Delimiter::Brace);
    assert_eq!(g.span().lo, 7);
    assert_eq!(g.span().hi, source.len() as u32);
    assert_eq!(g.span_open().file, 42);
    assert_eq!(g.span_close().end(), g.span().end());
}

#[test]
fn delimiter_mismatches_report_the_offending_range() {
    for (source, expected) in [("(]", (1, 2)), (")", (0, 1)), ("{[", (1, 2)), ("(x", (0, 1))] {
        let mut toks = Vec::new();
        assert!(lex(source.as_bytes(), 2021, &mut toks).is_ok());
        let e = from_toks(source.as_bytes(), &toks, &|_, _| Span::DUMMY).unwrap_err();
        assert_eq!((e.lo, e.hi), expected);
    }
}

#[test]
fn eof_is_optional_and_terminates_the_supplied_slice() {
    let src = b"x )";
    let toks = [Tok { kind: T::Ident, lo: 0, hi: 1 }, Tok { kind: T::Eof, lo: 1, hi: 1 }, Tok { kind: T::CloseParen, lo: 2, hi: 3 }];
    for ts in [&toks[..1], &toks[..]] {
        assert_eq!(from_toks(src, ts, &|_, _| Span::DUMMY).unwrap().to_string(), "x");
    }
}

#[test]
fn doc_gaps_are_attributes_and_explicit_trivia_does_not_duplicate_them() {
    let source = "{ /// outer\r\n x /*!inner*/ y /**block*/ z ////ordinary\n /**/ }";
    let without = stream(source, false).to_string();
    let with = stream(source, true).to_string();
    assert_eq!(without, with);
    assert_eq!(without, "{ #[doc = \" outer\"] x # ! [doc = \"inner\"] y #[doc = \"block\"] z }");
    // Relax's builder only examines gaps between tokens before Eof. Keep that
    // slice-local behavior; opt-in trivia can represent leading/trailing docs.
    assert_eq!(stream("///leading\nx ///trailing", false).to_string(), "x");
    assert_eq!(stream("///leading\nx ///trailing", true).to_string(), "#[doc = \"leading\"] x #[doc = \"trailing\"]");
}

#[test]
fn dialect_atoms_and_unicode_punctuation_keep_their_text() {
    let source = "@r#x #abc 250ms 'a \u{80}";
    let mut toks = Vec::new();
    lex_with(source.as_bytes(), &LexOptions { edition: 2021, mode: Mode::Declarative, trivia: false }, &mut toks).unwrap();
    let index = LineIndex::new(source.as_bytes());
    let ts = from_toks(source.as_bytes(), &toks, &|lo, hi| span(&index, lo, hi)).unwrap();
    assert_eq!(ts.to_string(), "@ r#x #abc 250ms 'a \u{80}");
    let TokenTree::Punct(p) = ts.trees.last().unwrap() else { panic!("punctuation") };
    assert_eq!(p.as_char(), '\u{80}');
    assert_eq!(p.span().hi - p.span().lo, 2);
}

#[test]
fn span_join_rejects_distinct_files_and_keeps_outer_endpoints() {
    let index = LineIndex::new(b"a\nbc\ndef");
    let a = span(&index, 0, 2);
    let b = span(&index, 3, 8);
    assert_eq!(a.join(b), Some(span(&index, 0, 8)));
    assert_eq!(b.join(a), a.join(b));
    let outer = span(&index, 0, 8);
    assert_eq!(outer.join(a), Some(outer));
    assert_eq!(b.join(Span { file: 7, ..a }), None);
    assert_eq!(b.join(Span { file: 0, ..a }), None);
    assert_eq!(Span::DUMMY.file, 0);
}
