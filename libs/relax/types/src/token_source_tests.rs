//! A lowered module's type errors must name the authored source, including imported input.
use crate::pm_tokens::{Delimiter, Group, Ident, Punct, Spacing, Span, TokenStream, TokenTree};
use crate::program::{DefId, Program};
use std::sync::Arc;

#[test]
fn token_file_type_error_names_imported_source() {
    let sess = Arc::new(crate::Session::default());
    let cfg = crate::cfg::CfgSet::host(&sess, &[]);
    let mut p = Program::new(&sess);
    let root = std::env::temp_dir().join(format!("relax-token-source-{}.rs", std::process::id()));
    std::fs::write(&root, "").unwrap();
    let krate = p.load_crate("token_test", root.to_str().unwrap(), 2021, cfg.clone(), Vec::new()).unwrap();
    std::fs::remove_file(root).unwrap();

    let path = "/fake/token-module.rx";
    let other = "/fake/imported-source.rx";
    let authored = "// imported\néé authored_boolean_expression\n";
    let lo = authored.find("authored_boolean_expression").unwrap() as u32;
    let hi = authored.trim_end().len() as u32;
    p.sources.register(path, Arc::from(&b"an authored module construct"[..])).unwrap();
    let id = p.sources.register(other, Arc::from(authored.as_bytes())).unwrap();
    assert_eq!(p.source_id(other), id);
    let current = Span { lo: 0, hi: 28, ..Span::DUMMY };
    let imported = Span { file: id, lo, hi, ..Span::DUMMY };
    let ident = |name: &str, span| TokenTree::Ident(Ident::new(name, span));
    let punct = |ch, spacing| TokenTree::Punct(Punct { ch, spacing, span: current });
    let group = |delim, trees| TokenTree::Group(Group { delim, stream: TokenStream { trees }, span: current });
    // Every tree is built by hand. The boolean spelling is four bytes; its authored span
    // covers a whole expression, and the source is not a Rust module to re-lex.
    let stream = TokenStream { trees: vec![
        ident("struct", current), TokenTree::Ident(Ident::new_raw("type", current)), punct(';', Spacing::Alone),
        ident("fn", current), ident("broken", current), group(Delimiter::Parenthesis, vec![]),
        group(Delimiter::Brace, vec![
            ident("let", current), ident("value", current), punct(':', Spacing::Alone), ident("u32", current),
            punct('=', Spacing::Alone), ident("true", imported), punct(';', Spacing::Alone),
        ]),
    ] };
    let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("token_test", path, "token_test", &cfg));
    let (pf, _) = crate::parser::parse_token_stream(stream, path, 2021, None, ctx, p.sources.clone(), false).unwrap();
    assert!(pf.token_input);
    assert_eq!(pf.ast.root_items.len(), 2);
    assert_eq!(pf.text(pf.ast.item(pf.ast.root_items[0]).name), "type");
    let at = pf.toks.iter().find(|t| t.kind == crate::lexer::T::KwTrue).unwrap().lo;
    assert_eq!(pf.source_span(at), crate::source::SourceSpan { file: id, lo, hi });
    let module = p.crates[krate as usize].root_mod;
    let file = p.add_generated_file(path, pf, krate, module).unwrap();
    p.resolve_imports();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&p);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let def = (0..p.defs.len()).map(|i| DefId(i as u32)).find(|d| p.name(*d) == "broken").unwrap();
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&p, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&p), &state);
    let body = crate::typeck::instantiate_body(&cx, &p, &tcx, def, &[]);
    assert_eq!(body.errors.len(), 1, "{:?}", body.errors);
    let d = crate::diag::from_legacy(&body.errors[0], "typeck");
    assert_eq!((d.file.as_str(), d.line, d.col), (other, 2, 6));
    assert!(d.message.contains("mismatched types"), "{}", d.message);
    let d = p.diagnostic(file, at, "typeck", "mismatched types".into());
    assert_eq!((d.end_line, d.end_col), (2, 33));
    assert_eq!(p.source_location(file, at).span.hi, hi);
}
