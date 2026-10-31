// Parser/tokenizer workloads (many small calls per iteration, tokenizer and parser loops): makepad's Rust tokenizer (the code editor's highlighter lexer),
// the in-tree pulldown-cmark markdown parser, and makepad-html, each over a large in-repo file.
use crate::{fold, inputs};

fn text(rel: &str) -> String {
    String::from_utf8_lossy(&inputs::read(rel)).to_string()
}

fn tok_code(t: &makepad_rust_tokenizer::full_token::FullToken) -> u64 {
    use makepad_rust_tokenizer::full_token::FullToken as T;
    match t {
        T::Punct(id) => 1 ^ id.0,
        T::Ident(id) => 2 ^ id.0,
        T::Open(_) => 3,
        T::Close(_) => 4,
        T::String => 5,
        T::Bool(b) => 6 ^ *b as u64,
        T::Color(c) => 7 ^ *c as u64,
        T::Float(f) => 8 ^ f.to_bits(),
        T::Int(i) => 9 ^ *i as u64,
        T::OtherNumber => 10,
        T::Lifetime => 11,
        T::Comment => 12,
        T::Whitespace => 13,
        T::Unknown => 14,
    }
}

fn ev_code(e: &pulldown_cmark::Event) -> u64 {
    use pulldown_cmark::Event as E;
    match e {
        E::Start(_) => 1,
        E::End(_) => 2,
        E::Text(t) => 3 ^ (t.len() as u64) << 8,
        E::Code(t) => 4 ^ (t.len() as u64) << 8,
        E::Html(t) | E::InlineHtml(t) => 5 ^ (t.len() as u64) << 8,
        E::SoftBreak => 6,
        E::HardBreak => 7,
        E::Rule => 8,
        E::TaskListMarker(b) => 9 ^ *b as u64,
        _ => 10,
    }
}

/// makepad-rust-tokenizer over a large Rust source file: token stream of the whole file
pub fn rust_tokens_setup() -> Box<dyn FnMut() -> u64> {
    use makepad_rust_tokenizer::tokenizer::{Cursor, State};
    let src = text("libs/ai/llm/src/runtime.rs");
    let end = src.char_indices().nth(150_000).map_or(src.len(), |(i, _)| i);
    // as the code editor does: one cursor per line, the state carried from line to line
    let lines: Vec<Vec<char>> = src[..end].split('\n').map(|l| l.chars().collect()).collect();
    Box::new(move || {
        let mut scratch = String::new();
        let mut state = State::default();
        let (mut h, mut n) = (0u64, 0u64);
        for line in &lines {
            let mut cursor = Cursor::new(line, &mut scratch);
            loop {
                let (next, tok) = state.next(&mut cursor);
                state = next;
                match tok {
                    Some(t) => {
                        h = fold(h, tok_code(&t.token) ^ (t.len as u64) << 40);
                        n += 1;
                    }
                    None => break,
                }
            }
        }
        if std::env::var("RW_DEBUG").is_ok() {
            eprintln!("rust_tokenize: {} tokens", n);
        }
        fold(h, n)
    })
}

/// pulldown-cmark over two large in-repo markdown documents, all extensions on
pub fn markdown_setup() -> Box<dyn FnMut() -> u64> {
    use pulldown_cmark::{Options, Parser};
    let mut doc = text("AGENTS.md");
    doc.push_str("\n\n");
    doc.push_str(&text("libs/box3d/README.md"));
    Box::new(move || {
        let mut h = 0u64;
        let mut n = 0u64;
        for _ in 0..8 {
            for ev in Parser::new_ext(&doc, Options::all()) {
                n += 1;
                h = fold(h, ev_code(&ev));
            }
        }
        fold(h, n)
    })
}

/// makepad-html over a large in-repo HTML page
pub fn html_setup() -> Box<dyn FnMut() -> u64> {
    use makepad_html::{parse_html, HtmlNode};
    use makepad_live_id::InternLiveId;
    let page = text("libs/piano_model/piano-explained.html");
    Box::new(move || {
        let mut h = 0u64;
        let mut out = String::new();
        for _ in 0..4 {
            let doc = parse_html(&page, &mut None, InternLiveId::No);
            out.clear();
            for n in &doc.nodes {
                h = fold(h, match n {
                    HtmlNode::OpenTag { lc, .. } => 1 ^ lc.0,
                    HtmlNode::CloseTag { lc, .. } => 2 ^ lc.0,
                    HtmlNode::Attribute { lc, start, end, .. } => 3 ^ lc.0 ^ ((end - start) as u64) << 20,
                    HtmlNode::Text { start, end, all_ws } => 4 ^ ((end - start) as u64) << 8 ^ (*all_ws as u64),
                });
            }
            h = fold(h, doc.decoded.len() as u64);
        }
        h
    })
}
