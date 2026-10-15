//! Rust outlines. Tokens retain source offsets; no expression, function body,
//! macro expansion or derive is parsed to build an outline.
use crate::cfg::CfgSet;
use crate::lexer::{Tok, T};
use crate::rs_index::NameIndex;
use std::sync::Arc;

/// What declaration discovery keeps of a file: its name index. The tokens it is built from
/// (the file's lex, its cfg-active tokens) belong to the scan that needs them.
pub struct Outline {
    pub index: NameIndex,
}

impl Outline {
    pub fn build(src: &[u8], edition: u16, cfg: &CfgSet) -> Option<Self> {
        let mut tokens = Vec::with_capacity(src.len() / 4);
        crate::lexer::lex(src, edition, &mut tokens).ok()?;
        Self::of_tokens(src, &tokens, cfg).map(|(outline, _)| outline)
    }

    /// The outline of a file from its (complete, successful) lex, and its cfg-active tokens
    /// (the file's tokens with cfg'd-out items and attributes removed).
    pub fn of_tokens(src: &[u8], tokens: &[Tok], cfg: &CfgSet) -> Option<(Self, Vec<Tok>)> {
        // Validate delimiters before the cfg walker, whose inputs normally come
        // from a successfully lowered file. Collection reports malformed input.
        pairs(tokens, None)?;
        let active = crate::preload::configured_tokens(src, tokens, cfg);
        let mut declarations = Vec::new();
        surface(&active, &matching(&active)?, 0, active.len(), &mut declarations);
        let index = index(src, &declarations)?;
        Some((Self { index }, active))
    }
}

pub(crate) fn expanded_index(src: &[u8], tokens: &[Tok]) -> Option<NameIndex> {
    let mut declarations = Vec::new();
    surface(tokens, &matching(tokens)?, 0, tokens.len(), &mut declarations);
    index(src, &declarations)
}

fn index(src: &[u8], tokens: &[Tok]) -> Option<NameIndex> {
    // The name index reads no doc comments (an explicit #[doc = ...] attribute is a
    // token, not a comment): blank every comment between tokens and drop doc-comment
    // tokens, so building the token trees does not turn each doc line into an
    // attribute (positions are unchanged).
    let kept;
    let tokens = if tokens.iter().any(|t| t.kind == T::DocComment) {
        kept = tokens.iter().copied().filter(|t| t.kind != T::DocComment).collect::<Vec<_>>();
        &kept[..]
    } else { tokens };
    let mut copy = None::<Vec<u8>>;
    for w in tokens.windows(2) {
        let (lo, hi) = (w[0].hi as usize, w[1].lo as usize);
        if lo < hi && src[lo..hi].contains(&b'/') { copy.get_or_insert_with(|| src.to_vec())[lo..hi].fill(b' '); }
    }
    let src = copy.as_deref().unwrap_or(src);
    let stream = crate::pm_tokens::from_toks(src, tokens, &|lo, hi| crate::pm_tokens::Span {
        lo, hi, ..crate::pm_tokens::Span::DUMMY
    }).ok()?;
    Some(crate::rs_index::of_tokens(&stream))
}

/// The closing index of each opening delimiter (u32: a file's token indexes fit), None when the
/// delimiters do not pair.
fn matching(tokens: &[Tok]) -> Option<Vec<u32>> {
    let mut ends = vec![0u32; tokens.len()];
    pairs(tokens, Some(&mut ends))?;
    Some(ends)
}

/// Whether the delimiters pair; fills `ends` (open index -> close index) when given one.
fn pairs(tokens: &[Tok], mut ends: Option<&mut Vec<u32>>) -> Option<()> {
    let mut stack = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        match t.kind {
            T::OpenBrace | T::OpenParen | T::OpenBracket => stack.push((i, t.kind)),
            T::CloseBrace | T::CloseParen | T::CloseBracket => {
                let (open, kind) = stack.pop()?;
                if !matches!((kind, t.kind), (T::OpenBrace, T::CloseBrace)
                    | (T::OpenParen, T::CloseParen) | (T::OpenBracket, T::CloseBracket)) { return None; }
                if let Some(ends) = ends.as_deref_mut() { ends[open] = i as u32; }
            }
            _ => {}
        }
    }
    stack.is_empty().then_some(())
}

// Keep declaration headers, imports, attributes and inline module/foreign
// scopes. A brace group belonging to an item body is represented by its original
// delimiters. Macro definitions' order and attributes survive; their transcribers
// remain in `active` for the existing textual macro scanner and demand analysis.
fn surface(tokens: &[Tok], ends: &[u32], mut at: usize, hi: usize, out: &mut Vec<Tok>) {
    let mut module = false;
    let mut foreign = false;
    let mut import = false;
    let mut initializer = false;
    while at < hi {
        let t = tokens[at];
        match t.kind {
            T::KwMod => module = true,
            T::KwExtern => foreign = true,
            T::KwFn | T::KwCrate => { foreign = false; initializer = false; }
            T::KwConst | T::KwStatic => initializer = true,
            T::KwUse => import = true,
            T::Semi => { module = false; foreign = false; import = false; initializer = false; }
            T::OpenParen | T::OpenBracket => {
                let end = ends[at] as usize;
                out.extend_from_slice(&tokens[at..=end]);
                at = end + 1;
                continue;
            }
            T::OpenBrace => {
                let end = ends[at] as usize;
                out.push(t);
                if import { out.extend_from_slice(&tokens[at + 1..end]); }
                else if module || foreign || initializer { surface(tokens, ends, at + 1, end, out); }
                out.push(tokens[end]);
                module = false; foreign = false; import = false; initializer = false;
                at = end + 1;
                continue;
            }
            _ => {}
        }
        out.push(t);
        at += 1;
    }
}

/// Expand a macro-owned declaration stream in the caller's crate/module scope.
/// This read-only projection is also useful to embedders comparing a contract
/// against ordinary macro expansion; it never installs parse-reuse entries.
pub fn macro_declaration_index(input: &crate::pm::TokenStream, edition: u16,
    ctx: std::rc::Rc<crate::builtins::FileCtx>, table: Arc<crate::macros::MacroTable>,
    declaration_contract: bool,
) -> Result<NameIndex, String> {
    let input = crate::builtins::Expansion::from_stream(input, edition)?;
    let surface = if declaration_contract {
        crate::parser::rs_expand::expand_declarations(&input, edition, ctx, table)
    } else {
        let mut toks = crate::parser::lazy_bodies::Toks::from(&input.toks[..]);
        let end = input.text.len() as u32;
        toks.push(Tok { kind: T::Eof, lo: end, hi: end });
        crate::parser::rs_expand::expand(input.text.as_bytes(), toks, edition, ctx, table)
    };
    if !surface.unknown.is_empty() { return Err(format!("unsupported macro surface: {:?}", surface.unknown)); }
    expanded_index(&surface.text(input.text.as_bytes()), &surface.toks).ok_or_else(|| "invalid expanded declaration tokens".into())
}
