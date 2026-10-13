//! The host side of the proc-macro token API: rustc's `proc_macro` model (TokenStream,
//! TokenTree, Group, Ident, Punct, Literal, Span) as plain data, in which Relax's built-in macros,
//! derive inputs and the JIT proc-macro host's encoding (pm.rs) work. Macros themselves see the
//! standard `proc_macro` crate of Relax's std set (std/proc_macro); pm.rs converts between the two.
//!
//! Semantics follow rustc's proc_macro where macros can observe them: punctuation is split into
//! single-char `Punct`s (Joint when the next token is punctuation directly after it), a lifetime
//! is `'` (Joint) + ident, doc comments are `#[doc = "..."]`.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Delimiter {
    Parenthesis,
    Brace,
    Bracket,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Spacing {
    Joint,
    Alone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineColumn {
    /// 1-based
    pub line: usize,
    /// 1-based, in chars
    pub column: usize,
}

impl LineColumn {
    pub fn line(&self) -> usize {
        self.line
    }
    pub fn column(&self) -> usize {
        self.column
    }
}

/// A source range: byte offsets (into the parser's position space) plus line/column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
    pub start: LineColumn,
    pub end: LineColumn,
    /// The source file, as an id its producer's host assigns . 0 is "the current file":
    /// Relax's own spans are positions in its parser's position space and keep 0.
    pub file: u32,
}

thread_local! {
    /// The span of the macro invocation being expanded (`Span::call_site()`).
    static CALL_SITE: std::cell::Cell<Span> = std::cell::Cell::new(Span::DUMMY);
}

impl Span {
    pub const DUMMY: Span = Span { lo: 0, hi: 0, start: LineColumn { line: 1, column: 1 }, end: LineColumn { line: 1, column: 1 }, file: 0 };

    pub fn call_site() -> Span {
        CALL_SITE.with(|c| c.get())
    }
    pub fn mixed_site() -> Span {
        Span::call_site()
    }
    pub fn def_site() -> Span {
        Span::call_site()
    }
    pub fn set_call_site(s: Span) -> Span {
        CALL_SITE.with(|c| c.replace(s))
    }
    pub fn start(&self) -> LineColumn {
        self.start
    }
    pub fn end(&self) -> LineColumn {
        self.end
    }
    pub fn join(&self, other: Span) -> Option<Span> {
        if self.file != other.file {
            return None;
        }
        let (a, b) = if self.lo <= other.lo { (*self, other) } else { (other, *self) };
        Some(Span { lo: a.lo, hi: b.hi.max(a.hi), start: a.start, end: if b.hi >= a.hi { b.end } else { a.end }, file: a.file })
    }
    pub fn resolved_at(&self, _other: Span) -> Span {
        *self
    }
    pub fn located_at(&self, other: Span) -> Span {
        other
    }
    pub fn unwrap(self) -> Span {
        self
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    #[doc(hidden)]
    pub delim: Delimiter,
    #[doc(hidden)]
    pub stream: TokenStream,
    #[doc(hidden)]
    pub span: Span,
}

impl Group {
    pub fn new(delim: Delimiter, stream: TokenStream) -> Group {
        Group { delim, stream, span: Span::call_site() }
    }
    pub fn delimiter(&self) -> Delimiter {
        self.delim
    }
    pub fn stream(&self) -> TokenStream {
        self.stream.clone()
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn span_open(&self) -> Span {
        Span { hi: self.span.lo + 1, end: LineColumn { line: self.span.start.line, column: self.span.start.column + 1 }, ..self.span }
    }
    pub fn span_close(&self) -> Span {
        Span { lo: self.span.hi.saturating_sub(1), start: LineColumn { line: self.span.end.line, column: self.span.end.column.saturating_sub(1) }, ..self.span }
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

#[derive(Clone, Debug)]
pub struct Ident {
    #[doc(hidden)]
    pub name: String,
    #[doc(hidden)]
    pub raw: bool,
    #[doc(hidden)]
    pub span: Span,
}

impl Ident {
    pub fn new(name: &str, span: Span) -> Ident {
        match name.strip_prefix("r#") {
            Some(r) => Ident { name: r.to_string(), raw: true, span },
            None => Ident { name: name.to_string(), raw: false, span },
        }
    }
    pub fn new_raw(name: &str, span: Span) -> Ident {
        Ident { name: name.to_string(), raw: true, span }
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

#[derive(Clone, Debug)]
pub struct Punct {
    #[doc(hidden)]
    pub ch: char,
    #[doc(hidden)]
    pub spacing: Spacing,
    #[doc(hidden)]
    pub span: Span,
}

impl Punct {
    pub fn new(ch: char, spacing: Spacing) -> Punct {
        Punct { ch, spacing, span: Span::call_site() }
    }
    pub fn as_char(&self) -> char {
        self.ch
    }
    pub fn spacing(&self) -> Spacing {
        self.spacing
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

/// A literal, kept as its source text (rustc prints literals as written).
#[derive(Clone, Debug)]
pub struct Literal {
    #[doc(hidden)]
    pub text: String,
    #[doc(hidden)]
    pub span: Span,
}

macro_rules! lit_ctors {
    ($($suf:ident $unsuf:ident $t:ty, $s:literal;)*) => {
        $(
            pub fn $suf(n: $t) -> Literal {
                Literal::from_text(format!(concat!("{}", $s), n))
            }
            pub fn $unsuf(n: $t) -> Literal {
                Literal::from_text(format!("{}", n))
            }
        )*
    };
}

impl Literal {
    pub fn from_text(text: String) -> Literal {
        Literal { text, span: Span::call_site() }
    }
    pub fn with_span(text: String, span: Span) -> Literal {
        Literal { text, span }
    }
    lit_ctors! {
        u8_suffixed u8_unsuffixed u8, "u8";
        u16_suffixed u16_unsuffixed u16, "u16";
        u32_suffixed u32_unsuffixed u32, "u32";
        u64_suffixed u64_unsuffixed u64, "u64";
        u128_suffixed u128_unsuffixed u128, "u128";
        usize_suffixed usize_unsuffixed usize, "usize";
        i8_suffixed i8_unsuffixed i8, "i8";
        i16_suffixed i16_unsuffixed i16, "i16";
        i32_suffixed i32_unsuffixed i32, "i32";
        i64_suffixed i64_unsuffixed i64, "i64";
        i128_suffixed i128_unsuffixed i128, "i128";
        isize_suffixed isize_unsuffixed isize, "isize";
    }
    fn float_repr(f: f64, shortest: String) -> String {
        if !f.is_finite() {
            panic!("Invalid float literal {}", f);
        }
        let mut repr = shortest;
        if !repr.contains('.') {
            repr.push_str(".0");
        }
        repr
    }
    pub fn f32_unsuffixed(f: f32) -> Literal {
        Literal::from_text(Literal::float_repr(f as f64, f.to_string()))
    }
    pub fn f32_suffixed(f: f32) -> Literal {
        Literal::from_text(format!("{}f32", Literal::float_repr(f as f64, f.to_string())))
    }
    pub fn f64_unsuffixed(f: f64) -> Literal {
        Literal::from_text(Literal::float_repr(f, f.to_string()))
    }
    pub fn f64_suffixed(f: f64) -> Literal {
        Literal::from_text(format!("{}f64", Literal::float_repr(f, f.to_string())))
    }
    pub fn string(s: &str) -> Literal {
        Literal::from_text(format!("{:?}", s))
    }
    pub fn character(c: char) -> Literal {
        Literal::from_text(format!("{:?}", c))
    }
    pub fn byte_character(b: u8) -> Literal {
        Literal::from_text(format!("b'{}'", std::ascii::escape_default(b)))
    }
    pub fn byte_string(bytes: &[u8]) -> Literal {
        let mut s = String::from("b\"");
        for &b in bytes {
            for c in std::ascii::escape_default(b) {
                s.push(c as char);
            }
        }
        s.push('"');
        Literal::from_text(s)
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

impl std::str::FromStr for Literal {
    type Err = LexError;
    fn from_str(s: &str) -> Result<Literal, LexError> {
        let ts: TokenStream = s.parse()?;
        let mut it = ts.into_iter();
        match (it.next(), it.next()) {
            (Some(TokenTree::Literal(l)), None) => Ok(l),
            _ => Err(LexError),
        }
    }
}

#[derive(Clone, Debug)]
pub enum TokenTree {
    Group(Group),
    Ident(Ident),
    Punct(Punct),
    Literal(Literal),
}

impl TokenTree {
    pub fn span(&self) -> Span {
        match self {
            TokenTree::Group(g) => g.span,
            TokenTree::Ident(i) => i.span,
            TokenTree::Punct(p) => p.span,
            TokenTree::Literal(l) => l.span,
        }
    }
    pub fn set_span(&mut self, span: Span) {
        match self {
            TokenTree::Group(g) => g.span = span,
            TokenTree::Ident(i) => i.span = span,
            TokenTree::Punct(p) => p.span = span,
            TokenTree::Literal(l) => l.span = span,
        }
    }
}

impl From<Group> for TokenTree {
    fn from(g: Group) -> TokenTree {
        TokenTree::Group(g)
    }
}
impl From<Ident> for TokenTree {
    fn from(g: Ident) -> TokenTree {
        TokenTree::Ident(g)
    }
}
impl From<Punct> for TokenTree {
    fn from(g: Punct) -> TokenTree {
        TokenTree::Punct(g)
    }
}
impl From<Literal> for TokenTree {
    fn from(g: Literal) -> TokenTree {
        TokenTree::Literal(g)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TokenStream {
    #[doc(hidden)]
    pub trees: Vec<TokenTree>,
}

#[derive(Debug)]
pub struct LexError;

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("cannot lex the string as Rust tokens")
    }
}

impl TokenStream {
    pub fn new() -> TokenStream {
        TokenStream { trees: Vec::new() }
    }
    pub fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }
}

pub mod token_stream {
    pub type IntoIter = std::vec::IntoIter<super::TokenTree>;
}

impl IntoIterator for TokenStream {
    type Item = TokenTree;
    type IntoIter = token_stream::IntoIter;
    fn into_iter(self) -> token_stream::IntoIter {
        self.trees.into_iter()
    }
}

impl From<TokenTree> for TokenStream {
    fn from(t: TokenTree) -> TokenStream {
        TokenStream { trees: vec![t] }
    }
}

impl FromIterator<TokenTree> for TokenStream {
    fn from_iter<I: IntoIterator<Item = TokenTree>>(it: I) -> TokenStream {
        TokenStream { trees: it.into_iter().collect() }
    }
}

impl FromIterator<TokenStream> for TokenStream {
    fn from_iter<I: IntoIterator<Item = TokenStream>>(it: I) -> TokenStream {
        let mut v = Vec::new();
        for s in it {
            v.extend(s.trees);
        }
        TokenStream { trees: v }
    }
}

impl Extend<TokenTree> for TokenStream {
    fn extend<I: IntoIterator<Item = TokenTree>>(&mut self, it: I) {
        self.trees.extend(it);
    }
}

impl Extend<TokenStream> for TokenStream {
    fn extend<I: IntoIterator<Item = TokenStream>>(&mut self, it: I) {
        for s in it {
            self.trees.extend(s.trees);
        }
    }
}

/// Lexer for `str::parse::<TokenStream>()`: the host compiler installs one (`set_lexer`).
static LEXER: std::sync::OnceLock<fn(&str) -> Option<TokenStream>> = std::sync::OnceLock::new();

/// Installs the lexer behind `"..".parse::<TokenStream>()` (once per process).
#[doc(hidden)]
pub fn set_lexer(f: fn(&str) -> Option<TokenStream>) {
    let _ = LEXER.set(f);
}

/// Parses Rust source text into tokens (spans: call site).
impl std::str::FromStr for TokenStream {
    type Err = LexError;
    fn from_str(s: &str) -> Result<TokenStream, LexError> {
        match LEXER.get() {
            Some(f) => f(s).ok_or(LexError),
            None => Err(LexError),
        }
    }
}

// ---------------------------------------------------------------- printing (rustc's rules)

/// rustc's `space_between` (rustc_ast_pretty): no space in `x.y`, `$e`, `foo,`, `f(`, `#[`.
fn space_between(a: &TokenTree, b: &TokenTree) -> bool {
    let is_punct = |t: &TokenTree| matches!(t, TokenTree::Punct(_));
    match (a, b) {
        (TokenTree::Punct(p), t) if p.ch == '.' && !is_punct(t) => false,
        (TokenTree::Punct(p), TokenTree::Ident(_)) if p.ch == '$' => false,
        (t, TokenTree::Punct(p)) if matches!(p.ch, ',' | ';' | '.') && !is_punct(t) => false,
        (TokenTree::Ident(i), TokenTree::Group(g))
            if g.delim == Delimiter::Parenthesis && (i.raw || !is_reserved(&i.name) || matches!(i.name.as_str(), "fn" | "Self" | "pub")) =>
        {
            false
        }
        (TokenTree::Punct(p), TokenTree::Group(g)) if p.ch == '#' && g.delim == Delimiter::Bracket => false,
        _ => true,
    }
}

fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in"
            | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "self" | "Self" | "static" | "struct"
            | "super" | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await" | "dyn" | "abstract"
            | "become" | "box" | "do" | "final" | "macro" | "override" | "priv" | "typeof" | "unsized" | "virtual" | "yield" | "try"
            | "_"
    )
}

/// Printed like rustc's `TokenStream::to_string`: a space between trees unless the first is a
/// Joint punct or rustc's spacing rules say none.
fn print_trees(trees: &[TokenTree], f: &mut fmt::Formatter) -> fmt::Result {
    for (i, t) in trees.iter().enumerate() {
        fmt::Display::fmt(t, f)?;
        if let Some(n) = trees.get(i + 1) {
            let joint = matches!(t, TokenTree::Punct(p) if p.spacing == Spacing::Joint);
            if !joint && space_between(t, n) {
                f.write_str(" ")?;
            }
        }
    }
    Ok(())
}

impl fmt::Display for TokenStream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        print_trees(&self.trees, f)
    }
}

impl fmt::Display for TokenTree {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TokenTree::Group(g) => fmt::Display::fmt(g, f),
            TokenTree::Ident(i) => fmt::Display::fmt(i, f),
            TokenTree::Punct(p) => fmt::Display::fmt(p, f),
            TokenTree::Literal(l) => fmt::Display::fmt(l, f),
        }
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (o, c) = match self.delim {
            Delimiter::Parenthesis => ("(", ")"),
            Delimiter::Brace => ("{", "}"),
            Delimiter::Bracket => ("[", "]"),
            Delimiter::None => ("", ""),
        };
        f.write_str(o)?;
        if self.delim == Delimiter::Brace && !self.stream.is_empty() {
            f.write_str(" ")?;
            print_trees(&self.stream.trees, f)?;
            f.write_str(" ")?;
        } else {
            print_trees(&self.stream.trees, f)?;
        }
        f.write_str(c)
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.raw {
            f.write_str("r#")?;
        }
        f.write_str(&self.name)
    }
}

impl fmt::Display for Punct {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.ch)
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.text)
    }
}


// ---------------------------------------------------------------- flat tokens -> token trees

use crate::lexer::{Tok, T};

/// The source range of an unmatched delimiter (or an unlexable raw macro body).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unbalanced {
    pub lo: u32,
    pub hi: u32,
}

impl fmt::Display for Unbalanced {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unbalanced token stream at {}..{}", self.lo, self.hi)
    }
}

impl std::error::Error for Unbalanced {}

/// Build token trees from source-backed tokens, stopping at `Eof` if present.
/// Punctuation spacing, lifetimes, raw bodies and doc-comment gaps follow Relax's
/// `builtins::to_stream`. No leading/trailing gap outside the supplied token slice
/// is read. Explicit comment tokens are also accepted; docs become attributes.
/// Token byte ranges must index `src`, as they do for `lexer::lex`/`lex_with`.
pub fn from_toks(
    src: &[u8], toks: &[Tok], span_of: &dyn Fn(u32, u32) -> Span,
) -> Result<TokenStream, Unbalanced> {
    // stack of (delimiter, open token span lo, trees)
    let mut stack: Vec<(Delimiter, u32, Vec<TokenTree>)> = vec![(Delimiter::None, 0, Vec::new())];
    for i in 0..toks.len() {
        let t = toks[i];
        if t.kind == T::Eof {
            break;
        }
        // doc comments in the gap before this token (same buffer only)
        if i > 0 {
            let prev = toks[i - 1];
            if prev.hi < t.lo {
                let gap = &src[prev.hi as usize..t.lo as usize];
                if gap.windows(2).any(|w| w == b"//" || w == b"/*") {
                    let top = &mut stack.last_mut().unwrap().2;
                    gap_docs(gap, prev.hi, span_of, top);
                }
            }
        }
        let text = &src[t.lo as usize..t.hi as usize];
        match t.kind {
            T::Comment => {}
            T::DocComment => gap_docs(text, t.lo, span_of, &mut stack.last_mut().unwrap().2),
            T::AtId => {
                let mut at = Punct::new('@', Spacing::Alone);
                at.set_span(span_of(t.lo, t.lo + 1));
                let top = &mut stack.last_mut().unwrap().2;
                top.push(TokenTree::Punct(at));
                top.push(TokenTree::Ident(Ident::new(
                    std::str::from_utf8(&text[1..]).unwrap_or("?"), span_of(t.lo + 1, t.hi),
                )));
            }
            T::OpenParen | T::OpenBracket | T::OpenBrace => {
                let d = match t.kind {
                    T::OpenParen => Delimiter::Parenthesis,
                    T::OpenBracket => Delimiter::Bracket,
                    _ => Delimiter::Brace,
                };
                stack.push((d, t.lo, Vec::new()));
            }
            T::CloseParen | T::CloseBracket | T::CloseBrace => {
                if stack.len() < 2 {
                    return Err(Unbalanced { lo: t.lo, hi: t.hi });
                }
                let (d, open_lo, trees) = stack.pop().unwrap();
                let want = match t.kind {
                    T::CloseParen => Delimiter::Parenthesis,
                    T::CloseBracket => Delimiter::Bracket,
                    _ => Delimiter::Brace,
                };
                if d != want {
                    return Err(Unbalanced { lo: t.lo, hi: t.hi });
                }
                let mut g = Group::new(d, TokenStream { trees });
                g.set_span(span_of(open_lo, t.hi));
                stack.last_mut().unwrap().2.push(TokenTree::Group(g));
            }
            T::Ident | T::RawIdent => {
                let s = std::str::from_utf8(text).unwrap_or("?");
                stack.last_mut().unwrap().2.push(TokenTree::Ident(Ident::new(s, span_of(t.lo, t.hi))));
            }
            T::Lifetime => {
                let s = std::str::from_utf8(&text[1..]).unwrap_or("?");
                let mut q = Punct::new('\'', Spacing::Joint);
                q.set_span(span_of(t.lo, t.lo + 1));
                let top = &mut stack.last_mut().unwrap().2;
                top.push(TokenTree::Punct(q));
                top.push(TokenTree::Ident(Ident::new(s, span_of(t.lo + 1, t.hi))));
            }
            T::RawBody => {
                // a raw macro body inside other tokens: lex it as Rust
                let mut sub = Vec::new();
                if crate::lexer::lex(text, 2021, &mut sub).is_err() {
                    return Err(Unbalanced { lo: t.lo, hi: t.hi });
                }
                for s in sub.iter_mut() {
                    s.lo += t.lo;
                    s.hi += t.lo;
                }
                // Positions now index the parent source.
                let inner = from_toks(src, &sub, span_of)?;
                stack.last_mut().unwrap().2.extend(inner.trees);
            }
            k if is_keyword(k) => {
                let s = std::str::from_utf8(text).unwrap_or("?");
                stack.last_mut().unwrap().2.push(TokenTree::Ident(Ident::new(s, span_of(t.lo, t.hi))));
            }
            k if is_literal(k) => {
                let s = String::from_utf8_lossy(text).into_owned();
                stack.last_mut().unwrap().2.push(TokenTree::Literal(Literal::with_span(s, span_of(t.lo, t.hi))));
            }
            _ => {
                // punctuation: one Punct per char; Joint inside, and on the last char when
                // punctuation follows immediately (rustc's spacing)
                let next_joint = i + 1 < toks.len() && toks[i + 1].lo == t.hi && is_punct_kind(toks[i + 1].kind);
                let next_joint = next_joint || (t.kind == T::Apostrophe
                    && toks.get(i + 1).is_some_and(|next| next.lo == t.hi
                        && (matches!(next.kind, T::Ident | T::RawIdent) || is_keyword(next.kind))));
                let n = text.len();
                let top = &mut stack.last_mut().unwrap().2;
                for (j, c) in std::str::from_utf8(text).unwrap_or("?").char_indices() {
                    let sp = if j + c.len_utf8() < n || next_joint { Spacing::Joint } else { Spacing::Alone };
                    let mut p = Punct::new(c, sp);
                    p.set_span(span_of(t.lo + j as u32, t.lo + (j + c.len_utf8()) as u32));
                    top.push(TokenTree::Punct(p));
                }
            }
        }
    }
    if stack.len() != 1 {
        let lo = stack.last().unwrap().1;
        return Err(Unbalanced { lo, hi: lo + 1 });
    }
    Ok(TokenStream { trees: stack.pop().unwrap().2 })
}

fn is_punct_kind(k: T) -> bool {
    !matches!(
        k,
        T::Eof
            | T::Ident
            | T::RawBody
            | T::RawIdent
            | T::Lifetime
            | T::Int
            | T::Float
            | T::Str
            | T::ByteStr
            | T::CStr
            | T::RawStr
            | T::Char
            | T::Byte
            | T::Colour
            | T::Duration
            | T::AtId
            | T::Comment
            | T::DocComment
            | T::OpenParen
            | T::CloseParen
            | T::OpenBracket
            | T::CloseBracket
            | T::OpenBrace
            | T::CloseBrace
    ) && !is_keyword(k)
}

fn is_keyword(k: T) -> bool {
    (k as u8) >= (T::KwAs as u8) && (k as u8) <= (T::KwTry as u8)
}

fn is_literal(k: T) -> bool {
    matches!(k, T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::Colour | T::Duration)
}

/// A doc comment found in a gap between tokens: byte range in the gap, inner (`//!`, `/*!`),
/// and the text after the comment opener (and before a block comment's `*/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapDoc<'a> {
    pub lo: usize,
    pub hi: usize,
    pub inner: bool,
    pub text: &'a [u8],
}

/// The doc comments of a gap between tokens (the lexer drops comments), in order. `///x` (not
/// `////`), `//!x`, `/**x*/` (not `/***`, not `/**/`) and `/*!x*/` are docs; other comments
/// are skipped (block comments nest).
pub fn for_each_gap_doc<'a>(gap: &'a [u8], mut f: impl FnMut(GapDoc<'a>)) {
    let n = gap.len();
    let mut i = 0;
    while i < n {
        if gap[i] == b'/' && i + 1 < n && gap[i + 1] == b'/' {
            let s = i;
            while i < n && gap[i] != b'\n' {
                i += 1;
            }
            let c = &gap[s..i];
            let (doc, inner) = if c.len() >= 3 && c[2] == b'/' && !(c.len() >= 4 && c[3] == b'/') {
                (true, false)
            } else if c.len() >= 3 && c[2] == b'!' {
                (true, true)
            } else {
                (false, false)
            };
            if doc {
                let mut text = &c[3..];
                if text.last() == Some(&b'\r') {
                    text = &text[..text.len() - 1];
                }
                f(GapDoc { lo: s, hi: i, inner, text });
            }
        } else if gap[i] == b'/' && i + 1 < n && gap[i + 1] == b'*' {
            let s = i;
            let mut d = 1;
            i += 2;
            while d > 0 && i < n {
                if gap[i] == b'/' && i + 1 < n && gap[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if gap[i] == b'*' && i + 1 < n && gap[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let c = &gap[s..i];
            if c.len() >= 5 {
                let body = &c[3..c.len() - 2];
                if c[2] == b'*' && c[3] != b'*' {
                    f(GapDoc { lo: s, hi: i, inner: false, text: body });
                } else if c[2] == b'!' {
                    f(GapDoc { lo: s, hi: i, inner: true, text: body });
                }
            }
        } else {
            i += 1;
        }
    }
}

/// A doc comment as macro input shows it to `macro_rules!`: `/// foo` is `#[doc = r"foo"]`,
/// `//! foo` is `#![doc = r"foo"]`, the raw string with as many `#` as its text needs.
/// rustc_ast tokenstream.rs:698 `desugar_doc_comments`, `desugared_tts` (:736-775).
pub fn desugared_doc_attr(inner: bool, text: &[u8]) -> String {
    let text = String::from_utf8_lossy(text);
    // the longest `"#*` run in the text (tokenstream.rs:741-751)
    let mut hashes = 0;
    let mut count = 0;
    for ch in text.chars() {
        count = match ch {
            '"' => 1,
            '#' if count > 0 => count + 1,
            _ => 0,
        };
        hashes = hashes.max(count);
    }
    let h = "#".repeat(hashes);
    format!("#{}[doc = r{h}\"{text}\"{h}]", if inner { "!" } else { "" })
}

/// Doc comments in a gap between tokens, as rustc's proc_macro shows them: `#` [`!`]
/// `[doc = "text"]` with the comment's span.
fn gap_docs(gap: &[u8], base: u32, span_of: &dyn Fn(u32, u32) -> Span, out: &mut Vec<TokenTree>) {
    for_each_gap_doc(gap, |d| push_doc(d.text, d.inner, span_of(base + d.lo as u32, base + d.hi as u32), out));
}

fn push_doc(text: &[u8], inner: bool, sp: Span, out: &mut Vec<TokenTree>) {
    let text = String::from_utf8_lossy(text);
    let mut esc = String::from("\"");
    for ch in text.chars() {
        esc.extend(ch.escape_debug());
    }
    esc.push('"');
    let mut p = Punct::new('#', Spacing::Alone);
    p.set_span(sp);
    out.push(TokenTree::Punct(p));
    if inner {
        let mut b = Punct::new('!', Spacing::Alone);
        b.set_span(sp);
        out.push(TokenTree::Punct(b));
    }
    let mut eq = Punct::new('=', Spacing::Alone);
    eq.set_span(sp);
    let inner_ts = TokenStream { trees: vec![TokenTree::Ident(Ident::new("doc", sp)), TokenTree::Punct(eq), TokenTree::Literal(Literal::with_span(esc, sp))] };
    let mut g = Group::new(Delimiter::Bracket, inner_ts);
    g.set_span(sp);
    out.push(TokenTree::Group(g));
}
