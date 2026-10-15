//! `proc_macro` for proc-macro crates Relax runs in its JIT proc-macro host (src/pmhost.rs).
//!
//! The token model of rustc's proc_macro (TokenStream, TokenTree, Group, Ident, Punct, Literal,
//! Span, Delimiter, Spacing) as plain data in the macro's own heap. The host hands a macro its
//! input and takes its output as bytes (`__relax::bridge`), in the encoding of the host's twin
//! model src/pm.rs; observable behaviour (spacing, literal text, doc comments as #[doc], spans
//! with 1-based line/column) is rustc's.
//! `to_string` is inherent on every token type as well as through Display, so token
//! comparisons in macros (`ident.to_string() == "pub"`) need no formatting machinery.

use std::string::String;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delimiter {
    Parenthesis,
    Brace,
    Bracket,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spacing {
    Joint,
    Alone,
}

/// A line/column position (1-based, columns in chars). `span.start().line()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineColumn {
    pub line: usize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    lo: u32,
    hi: u32,
    sl: u32,
    sc: u32,
    el: u32,
    ec: u32,
    file: u32,
}

// per thread: the host runs expansions on every parse worker at once
std::thread_local! {
    static CALL_SITE: std::cell::Cell<Span> = const { std::cell::Cell::new(Span { lo: 0, hi: 0, sl: 1, sc: 1, el: 1, ec: 1, file: 0 }) };
}

impl Span {
    pub fn call_site() -> Span {
        CALL_SITE.with(|c| c.get())
    }
    pub fn mixed_site() -> Span {
        Span::call_site()
    }
    pub fn def_site() -> Span {
        Span::call_site()
    }
    pub fn start(&self) -> LineColumn {
        LineColumn { line: self.sl as usize, column: self.sc as usize }
    }
    pub fn end(&self) -> LineColumn {
        LineColumn { line: self.el as usize, column: self.ec as usize }
    }
    pub fn line(&self) -> usize {
        self.sl as usize
    }
    pub fn column(&self) -> usize {
        self.sc as usize
    }
    pub fn join(&self, other: Span) -> Option<Span> {
        if self.file != other.file {
            return None;
        }
        let (a, b) = if self.lo <= other.lo { (*self, other) } else { (other, *self) };
        if b.hi >= a.hi {
            Some(Span { lo: a.lo, hi: b.hi, sl: a.sl, sc: a.sc, el: b.el, ec: b.ec, ..a })
        } else {
            Some(a)
        }
    }
    pub fn resolved_at(&self, _other: Span) -> Span {
        *self
    }
    pub fn located_at(&self, other: Span) -> Span {
        other
    }
    /// The source text is not kept with a span here (rustc may also answer None).
    pub fn source_text(&self) -> Option<String> {
        None
    }
    /// The span's byte range in its source (expansion text past the file's end).
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.lo as usize..self.hi as usize
    }
}

/// A proc macro is running (the JIT host only calls macro code from a macro call).
pub fn is_available() -> bool {
    true
}

#[derive(Clone, Debug)]
pub struct Group {
    delim: Delimiter,
    stream: TokenStream,
    span: Span,
}

impl Group {
    pub fn new(delimiter: Delimiter, stream: TokenStream) -> Group {
        Group { delim: delimiter, stream, span: Span::call_site() }
    }
    pub fn delimiter(&self) -> Delimiter {
        self.delim
    }
    pub fn stream(&self) -> TokenStream {
        self.stream.clone()
    }
    /// The opening / closing delimiter's span (the group's own here: Relax keeps one span).
    pub fn span_open(&self) -> Span {
        self.span
    }
    pub fn span_close(&self) -> Span {
        self.span
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        print_group(self, &mut s);
        s
    }
}

#[derive(Clone, Debug)]
pub struct Ident {
    name: String,
    raw: bool,
    span: Span,
}

impl Ident {
    pub fn new(string: &str, span: Span) -> Ident {
        if string.len() > 2 && string.as_bytes()[0] == b'r' && string.as_bytes()[1] == b'#' {
            return Ident { name: String::from(&string[2..]), raw: true, span };
        }
        Ident { name: String::from(string), raw: false, span }
    }
    pub fn new_raw(string: &str, span: Span) -> Ident {
        Ident { name: String::from(string), raw: true, span }
    }
    pub fn span(&self) -> Span {
        self.span
    }
    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        if self.raw {
            s.push_str("r#");
        }
        s.push_str(&self.name);
        s
    }
}

#[derive(Debug)]
pub struct Punct {
    ch: char,
    spacing: Spacing,
    span: Span,
}

// by hand: std's char has no Clone impl yet (coord.md, builtins lane -> std lanes)
impl Clone for Punct {
    fn clone(&self) -> Punct {
        Punct { ch: self.ch, spacing: self.spacing, span: self.span }
    }
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
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        s.push(self.ch);
        s
    }
}

/// A literal, kept as its source text.
#[derive(Clone, Debug)]
pub struct Literal {
    text: String,
    span: Span,
}

fn push_u64(s: &mut String, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if n == 0 {
        s.push('0');
        return;
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    while i < buf.len() {
        s.push(buf[i] as char);
        i += 1;
    }
}

/// u128 in decimal by long division over 32-bit limbs (no 128-bit division needed).
fn push_u128(s: &mut String, n: u128) {
    if n <= u64::MAX as u128 {
        push_u64(s, n as u64);
        return;
    }
    let mut limbs = [(n >> 96) as u32 as u64, (n >> 64) as u32 as u64, (n >> 32) as u32 as u64, n as u32 as u64];
    let mut digits = [0u8; 40];
    let mut k = 0;
    loop {
        let mut rem = 0u64;
        let mut zero = true;
        let mut j = 0;
        while j < 4 {
            let cur = (rem << 32) | limbs[j];
            limbs[j] = cur / 10;
            rem = cur % 10;
            if limbs[j] != 0 {
                zero = false;
            }
            j += 1;
        }
        digits[k] = b'0' + rem as u8;
        k += 1;
        if zero {
            break;
        }
    }
    while k > 0 {
        k -= 1;
        s.push(digits[k] as char);
    }
}

fn push_i128(s: &mut String, n: i128) {
    if n < 0 {
        s.push('-');
        push_u128(s, (n as u128).wrapping_neg());
    } else {
        push_u128(s, n as u128);
    }
}

/// `{:?}` of a char inside a string literal (rustc's Literal::string escaping): `\0 \t \r \n
/// \\ \"`, other control chars as `\u{..}`.
fn push_escaped(s: &mut String, c: char, quote: char) {
    match c {
        '\0' => s.push_str("\\0"),
        '\t' => s.push_str("\\t"),
        '\r' => s.push_str("\\r"),
        '\n' => s.push_str("\\n"),
        '\\' => s.push_str("\\\\"),
        _ if c == quote => {
            s.push('\\');
            s.push(c);
        }
        _ if (c as u32) < 0x20 || c as u32 == 0x7f => {
            s.push_str("\\u{");
            let v = c as u32;
            let hex = b"0123456789abcdef";
            let mut started = false;
            let mut sh = 28i32;
            while sh >= 0 {
                let d = (v >> sh as u32) & 15;
                if d != 0 || started || sh == 0 {
                    s.push(hex[d as usize] as char);
                    started = true;
                }
                sh -= 4;
            }
            s.push('}');
        }
        _ => s.push(c),
    }
}

macro_rules! int_lits {
    ($($suf:ident $unsuf:ident $t:ty, $name:literal, $wide:ident;)*) => {
        $(
            pub fn $suf(n: $t) -> Literal {
                let mut s = String::new();
                $wide(&mut s, n as _);
                s.push_str($name);
                Literal::from_text(s)
            }
            pub fn $unsuf(n: $t) -> Literal {
                let mut s = String::new();
                $wide(&mut s, n as _);
                Literal::from_text(s)
            }
        )*
    };
}

impl Literal {
    fn from_text(text: String) -> Literal {
        Literal { text, span: Span::call_site() }
    }
    int_lits! {
        u8_suffixed u8_unsuffixed u8, "u8", push_u128;
        u16_suffixed u16_unsuffixed u16, "u16", push_u128;
        u32_suffixed u32_unsuffixed u32, "u32", push_u128;
        u64_suffixed u64_unsuffixed u64, "u64", push_u128;
        u128_suffixed u128_unsuffixed u128, "u128", push_u128;
        usize_suffixed usize_unsuffixed usize, "usize", push_u128;
        i8_suffixed i8_unsuffixed i8, "i8", push_i128;
        i16_suffixed i16_unsuffixed i16, "i16", push_i128;
        i32_suffixed i32_unsuffixed i32, "i32", push_i128;
        i64_suffixed i64_unsuffixed i64, "i64", push_i128;
        i128_suffixed i128_unsuffixed i128, "i128", push_i128;
        isize_suffixed isize_unsuffixed isize, "isize", push_i128;
    }
    fn float_text(mut repr: String) -> String {
        let mut has_dot = false;
        for b in repr.as_bytes() {
            if *b == b'.' || *b == b'e' || *b == b'i' || *b == b'N' {
                has_dot = true;
            }
        }
        if !has_dot {
            repr.push_str(".0");
        }
        repr
    }
    pub fn f32_unsuffixed(n: f32) -> Literal {
        Literal::from_text(Literal::float_text(n.to_string()))
    }
    pub fn f32_suffixed(n: f32) -> Literal {
        let mut s = Literal::float_text(n.to_string());
        s.push_str("f32");
        Literal::from_text(s)
    }
    pub fn f64_unsuffixed(n: f64) -> Literal {
        Literal::from_text(Literal::float_text(n.to_string()))
    }
    pub fn f64_suffixed(n: f64) -> Literal {
        let mut s = Literal::float_text(n.to_string());
        s.push_str("f64");
        Literal::from_text(s)
    }
    pub fn string(string: &str) -> Literal {
        let mut s = String::from("\"");
        for c in string.chars() {
            push_escaped(&mut s, c, '"');
        }
        s.push('"');
        Literal::from_text(s)
    }
    pub fn character(ch: char) -> Literal {
        let mut s = String::from("'");
        push_escaped(&mut s, ch, '\'');
        s.push('\'');
        Literal::from_text(s)
    }
    pub fn byte_character(byte: u8) -> Literal {
        let hex = b"0123456789abcdef";
        let mut s = String::from("b'");
        match byte {
            b'\t' => s.push_str("\\t"),
            b'\r' => s.push_str("\\r"),
            b'\n' => s.push_str("\\n"),
            b'\\' => s.push_str("\\\\"),
            b'\'' => s.push_str("\\'"),
            0x20..=0x7e => s.push(byte as char),
            _ => {
                s.push_str("\\x");
                s.push(hex[(byte >> 4) as usize] as char);
                s.push(hex[(byte & 15) as usize] as char);
            }
        }
        s.push('\'');
        Literal::from_text(s)
    }
    pub fn byte_string(bytes: &[u8]) -> Literal {
        let hex = b"0123456789abcdef";
        let mut s = String::from("b\"");
        for &b in bytes {
            match b {
                b'\t' => s.push_str("\\t"),
                b'\r' => s.push_str("\\r"),
                b'\n' => s.push_str("\\n"),
                b'\\' => s.push_str("\\\\"),
                b'"' => s.push_str("\\\""),
                0x20..=0x7e => s.push(b as char),
                _ => {
                    s.push_str("\\x");
                    s.push(hex[(b >> 4) as usize] as char);
                    s.push(hex[(b & 15) as usize] as char);
                }
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
    pub fn to_string(&self) -> String {
        self.text.clone()
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
            TokenTree::Group(t) => t.span,
            TokenTree::Ident(t) => t.span,
            TokenTree::Punct(t) => t.span,
            TokenTree::Literal(t) => t.span,
        }
    }
    pub fn set_span(&mut self, span: Span) {
        match self {
            TokenTree::Group(t) => t.span = span,
            TokenTree::Ident(t) => t.span = span,
            TokenTree::Punct(t) => t.span = span,
            TokenTree::Literal(t) => t.span = span,
        }
    }
    pub fn to_string(&self) -> String {
        match self {
            TokenTree::Group(t) => t.to_string(),
            TokenTree::Ident(t) => t.to_string(),
            TokenTree::Punct(t) => t.to_string(),
            TokenTree::Literal(t) => t.to_string(),
        }
    }
}

impl From<Group> for TokenTree {
    fn from(t: Group) -> TokenTree {
        TokenTree::Group(t)
    }
}
impl From<Ident> for TokenTree {
    fn from(t: Ident) -> TokenTree {
        TokenTree::Ident(t)
    }
}
impl From<Punct> for TokenTree {
    fn from(t: Punct) -> TokenTree {
        TokenTree::Punct(t)
    }
}
impl From<Literal> for TokenTree {
    fn from(t: Literal) -> TokenTree {
        TokenTree::Literal(t)
    }
}

#[derive(Clone, Debug)]
pub struct TokenStream {
    trees: Vec<TokenTree>,
}

impl Default for TokenStream {
    fn default() -> TokenStream {
        TokenStream::new()
    }
}

impl TokenStream {
    pub fn new() -> TokenStream {
        TokenStream { trees: Vec::new() }
    }
    pub fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        print_trees(&self.trees, &mut s);
        s
    }
}

pub mod token_stream {
    use super::TokenTree;
    /// Iterator over a stream's trees (moves them out; kept reversed so `next` is a pop).
    #[derive(Clone)]
    pub struct IntoIter {
        pub(crate) rev: std::vec::Vec<TokenTree>,
    }
    impl Iterator for IntoIter {
        type Item = TokenTree;
        fn next(&mut self) -> Option<TokenTree> {
            self.rev.pop()
        }
    }
}

impl IntoIterator for TokenStream {
    type Item = TokenTree;
    type IntoIter = token_stream::IntoIter;
    fn into_iter(mut self) -> token_stream::IntoIter {
        // mem::replace, not a field move: Relax drops a partially moved struct whole (boot B4)
        let mut v = std::mem::replace(&mut self.trees, Vec::new());
        let mut rev = Vec::with_capacity(v.len());
        while let Some(t) = v.pop() {
            rev.push(t);
        }
        token_stream::IntoIter { rev }
    }
}

impl From<TokenTree> for TokenStream {
    fn from(t: TokenTree) -> TokenStream {
        let mut v = Vec::new();
        v.push(t);
        TokenStream { trees: v }
    }
}

impl Extend<TokenTree> for TokenStream {
    fn extend<I: IntoIterator<Item = TokenTree>>(&mut self, iter: I) {
        for t in iter {
            self.trees.push(t);
        }
    }
}

impl Extend<TokenStream> for TokenStream {
    fn extend<I: IntoIterator<Item = TokenStream>>(&mut self, iter: I) {
        for mut s in iter {
            for t in std::mem::replace(&mut s.trees, Vec::new()) {
                self.trees.push(t);
            }
        }
    }
}

impl FromIterator<TokenTree> for TokenStream {
    fn from_iter<I: IntoIterator<Item = TokenTree>>(iter: I) -> TokenStream {
        let mut s = TokenStream::new();
        for t in iter {
            s.trees.push(t);
        }
        s
    }
}

impl FromIterator<TokenStream> for TokenStream {
    fn from_iter<I: IntoIterator<Item = TokenStream>>(iter: I) -> TokenStream {
        let mut s = TokenStream::new();
        for mut x in iter {
            for t in std::mem::replace(&mut x.trees, Vec::new()) {
                s.trees.push(t);
            }
        }
        s
    }
}

// ---------------------------------------------------------------- printing (rustc's rules)

fn is_reserved(s: &str) -> bool {
    match s {
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in"
        | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "self" | "Self" | "static" | "struct"
        | "super" | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await" | "dyn" | "abstract"
        | "become" | "box" | "do" | "final" | "macro" | "override" | "priv" | "typeof" | "unsized" | "virtual" | "yield" | "try"
        | "_" => true,
        _ => false,
    }
}

fn space_between(a: &TokenTree, b: &TokenTree) -> bool {
    let b_punct = match b {
        TokenTree::Punct(_) => true,
        _ => false,
    };
    let a_punct = match a {
        TokenTree::Punct(_) => true,
        _ => false,
    };
    if let TokenTree::Punct(p) = a {
        if p.ch == '.' && !b_punct {
            return false;
        }
        if p.ch == '$' {
            if let TokenTree::Ident(_) = b {
                return false;
            }
        }
        if p.ch == '#' {
            if let TokenTree::Group(g) = b {
                if g.delim == Delimiter::Bracket {
                    return false;
                }
            }
        }
    }
    if let TokenTree::Punct(p) = b {
        if (p.ch == ',' || p.ch == ';' || p.ch == '.') && !a_punct {
            return false;
        }
    }
    if let TokenTree::Ident(i) = a {
        if let TokenTree::Group(g) = b {
            if g.delim == Delimiter::Parenthesis && (i.raw || !is_reserved(&i.name) || i.name == "fn" || i.name == "Self" || i.name == "pub") {
                return false;
            }
        }
    }
    true
}

fn print_trees(trees: &[TokenTree], s: &mut String) {
    let mut i = 0;
    while i < trees.len() {
        let t = &trees[i];
        match t {
            TokenTree::Group(g) => print_group(g, s),
            TokenTree::Ident(x) => s.push_str(&x.to_string()),
            TokenTree::Punct(p) => s.push(p.ch),
            TokenTree::Literal(l) => s.push_str(&l.text),
        }
        if i + 1 < trees.len() {
            let joint = match t {
                TokenTree::Punct(p) => p.spacing == Spacing::Joint,
                _ => false,
            };
            if !joint && space_between(t, &trees[i + 1]) {
                s.push(' ');
            }
        }
        i += 1;
    }
}

fn print_group(g: &Group, s: &mut String) {
    let (o, c) = match g.delim {
        Delimiter::Parenthesis => ("(", ")"),
        Delimiter::Brace => ("{", "}"),
        Delimiter::Bracket => ("[", "]"),
        Delimiter::None => ("", ""),
    };
    s.push_str(o);
    if g.delim == Delimiter::Brace && !g.stream.trees.is_empty() {
        s.push(' ');
        print_trees(&g.stream.trees, s);
        s.push(' ');
    } else {
        print_trees(&g.stream.trees, s);
    }
    s.push_str(c);
}

/// `"1u8".parse::<Literal>()`: one literal token (a leading `-` belongs to it, as rustc).
impl std::str::FromStr for Literal {
    type Err = LexError;
    fn from_str(s: &str) -> Result<Literal, LexError> {
        let ts = __relax::parse(s).ok_or(LexError { _p: () })?;
        let mut it = ts.into_iter();
        match (it.next(), it.next(), it.next()) {
            (Some(TokenTree::Literal(l)), None, _) => Ok(l),
            (Some(TokenTree::Punct(p)), Some(TokenTree::Literal(l)), None) if p.as_char() == '-' => {
                let mut t = String::from("-");
                t.push_str(&l.text);
                Ok(Literal::from_text(t))
            }
            _ => Err(LexError { _p: () }),
        }
    }
}

impl PartialEq<char> for Punct {
    fn eq(&self, other: &char) -> bool {
        self.ch == *other
    }
}

/// `"..".parse::<TokenStream>()` failed: the text does not lex (rustc's `proc_macro::LexError`).
#[derive(Debug, Clone)]
pub struct LexError {
    _p: (),
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("cannot parse string into token stream")
    }
}

/// As rustc: lexed into token trees, every span the call site's. The host's lexer does the
/// lexing (`__relax::parse`), so the tokens are exactly the ones the host reads back.
impl std::str::FromStr for TokenStream {
    type Err = LexError;
    fn from_str(s: &str) -> Result<TokenStream, LexError> {
        __relax::parse(s).ok_or(LexError { _p: () })
    }
}

impl std::fmt::Display for TokenStream {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.to_string())
    }
}
impl std::fmt::Display for TokenTree {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.to_string())
    }
}
impl std::fmt::Display for Group {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.to_string())
    }
}
impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.to_string())
    }
}
impl std::fmt::Display for Punct {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.to_string())
    }
}
impl std::fmt::Display for Literal {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// The host side of a macro call (src/pmhost.rs). Not part of rustc's proc_macro API.
pub mod __relax {
    use super::*;

    /// One macro call: the macro fn, its input (and attribute input) as bytes; the output is
    /// written back as bytes owned by the macro heap (leaked; the host copies them).
    #[repr(C)]
    pub struct BridgeIo {
        pub f: usize,
        /// 0: derive / function-like `fn(TokenStream) -> TokenStream`;
        /// 1: attribute `fn(TokenStream, TokenStream) -> TokenStream`
        pub kind: u64,
        pub inp: *const u8,
        pub inp_len: usize,
        pub attr: *const u8,
        pub attr_len: usize,
        pub out: *mut u8,
        pub out_len: usize,
    }

    extern "C" {
        /// The host's lexer (pmhost.rs): `src` as an encoded span + stream in a host buffer
        /// that stays valid until this thread's next call; 0 on success.
        fn relax_pm_parse(src: *const u8, len: usize, out: *mut *const u8, out_len: *mut usize) -> i32;
    }

    /// `src` lexed into a stream with call-site spans, None when it does not lex.
    pub(crate) fn parse(src: &str) -> Option<TokenStream> {
        let mut out: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        if unsafe { relax_pm_parse(src.as_ptr(), src.len(), &mut out, &mut len) } != 0 || out.is_null() {
            return None;
        }
        let b = unsafe { std::slice::from_raw_parts(out, len) };
        let mut r = Reader { b, i: 0 };
        let _ = r.span();
        let mut ts = r.stream();
        respan(&mut ts.trees, Span::call_site());
        Some(ts)
    }

    fn respan(trees: &mut Vec<TokenTree>, sp: Span) {
        for t in trees.iter_mut() {
            match t {
                TokenTree::Group(g) => {
                    g.span = sp;
                    respan(&mut g.stream.trees, sp);
                }
                TokenTree::Ident(x) => x.span = sp,
                TokenTree::Punct(p) => p.span = sp,
                TokenTree::Literal(l) => l.span = sp,
            }
        }
    }

    pub fn bridge(io: *mut BridgeIo) {
        let io = unsafe { &mut *io };
        let inp = unsafe { std::slice::from_raw_parts(io.inp, io.inp_len) };
        let mut r = Reader { b: inp, i: 0 };
        let call = r.span();
        CALL_SITE.with(|c| c.set(call));
        let input = r.stream();
        let out = if io.kind == 1 {
            let ab = unsafe { std::slice::from_raw_parts(io.attr, io.attr_len) };
            let mut ar = Reader { b: ab, i: 0 };
            let _ = ar.span();
            let attr = ar.stream();
            let f: fn(TokenStream, TokenStream) -> TokenStream = unsafe { std::mem::transmute(io.f) };
            f(attr, input)
        } else {
            let f: fn(TokenStream) -> TokenStream = unsafe { std::mem::transmute(io.f) };
            f(input)
        };
        let mut w = Vec::new();
        write_stream(&mut w, &out.trees);
        io.out_len = w.len();
        io.out = w.as_mut_ptr();
        std::mem::forget(w);
    }

    struct Reader<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl<'a> Reader<'a> {
        fn u8(&mut self) -> u8 {
            let v = self.b[self.i];
            self.i += 1;
            v
        }
        fn u32(&mut self) -> u32 {
            let v = (self.b[self.i] as u32) | ((self.b[self.i + 1] as u32) << 8) | ((self.b[self.i + 2] as u32) << 16) | ((self.b[self.i + 3] as u32) << 24);
            self.i += 4;
            v
        }
        fn span(&mut self) -> Span {
            let lo = self.u32();
            let hi = self.u32();
            let sl = self.u32();
            let sc = self.u32();
            let el = self.u32();
            let ec = self.u32();
            let file = self.u32();
            Span { lo, hi, sl, sc, el, ec, file }
        }
        fn text(&mut self) -> String {
            let n = self.u32() as usize;
            let s = String::from_utf8_lossy(&self.b[self.i..self.i + n]).into_owned();
            self.i += n;
            s
        }
        fn stream(&mut self) -> TokenStream {
            let n = self.u32() as usize;
            let mut trees = Vec::with_capacity(n);
            let mut k = 0;
            while k < n {
                let tag = self.u8();
                let span = self.span();
                let t = match tag {
                    0 => {
                        let d = match self.u8() {
                            0 => Delimiter::Parenthesis,
                            1 => Delimiter::Brace,
                            2 => Delimiter::Bracket,
                            _ => Delimiter::None,
                        };
                        let stream = self.stream();
                        TokenTree::Group(Group { delim: d, stream, span })
                    }
                    1 => {
                        let raw = self.u8() != 0;
                        let name = self.text();
                        TokenTree::Ident(Ident { name, raw, span })
                    }
                    2 => {
                        let ch = char::from_u32(self.u32()).unwrap_or('?');
                        let spacing = if self.u8() == 0 { Spacing::Joint } else { Spacing::Alone };
                        TokenTree::Punct(Punct { ch, spacing, span })
                    }
                    _ => {
                        let text = self.text();
                        TokenTree::Literal(Literal { text, span })
                    }
                };
                trees.push(t);
                k += 1;
            }
            TokenStream { trees }
        }
    }

    fn w32(w: &mut Vec<u8>, v: u32) {
        w.push(v as u8);
        w.push((v >> 8) as u8);
        w.push((v >> 16) as u8);
        w.push((v >> 24) as u8);
    }

    fn wspan(w: &mut Vec<u8>, s: &Span) {
        w32(w, s.lo);
        w32(w, s.hi);
        w32(w, s.sl);
        w32(w, s.sc);
        w32(w, s.el);
        w32(w, s.ec);
        w32(w, s.file);
    }

    fn wtext(w: &mut Vec<u8>, s: &str) {
        w32(w, s.len() as u32);
        for b in s.as_bytes() {
            w.push(*b);
        }
    }

    fn write_stream(w: &mut Vec<u8>, trees: &[TokenTree]) {
        w32(w, trees.len() as u32);
        for t in trees {
            match t {
                TokenTree::Group(g) => {
                    w.push(0);
                    wspan(w, &g.span);
                    w.push(match g.delim {
                        Delimiter::Parenthesis => 0,
                        Delimiter::Brace => 1,
                        Delimiter::Bracket => 2,
                        Delimiter::None => 3,
                    });
                    write_stream(w, &g.stream.trees);
                }
                TokenTree::Ident(x) => {
                    w.push(1);
                    wspan(w, &x.span);
                    w.push(if x.raw { 1 } else { 0 });
                    wtext(w, &x.name);
                }
                TokenTree::Punct(p) => {
                    w.push(2);
                    wspan(w, &p.span);
                    w32(w, p.ch as u32);
                    w.push(if p.spacing == Spacing::Joint { 0 } else { 1 });
                }
                TokenTree::Literal(l) => {
                    w.push(3);
                    wspan(w, &l.span);
                    wtext(w, &l.text);
                }
            }
        }
    }
}
