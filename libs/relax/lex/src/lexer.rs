//! Lexer: source bytes -> flat token list. Tokens carry byte spans only;
//! identifiers and literals are read back from the source when needed, so
//! lexing and parsing of different files share no state.
//! Rust's default entry drops comments; `lex_with` can retain comment trivia.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum T {
    Eof,
    Ident,
    /// the untokenised body of a registered raw-body macro `name! { ... }` (between the braces)
    RawBody,
    RawIdent,
    Lifetime,
    Int,
    Float,
    Str,
    ByteStr,
    CStr,
    RawStr,
    Char,
    Byte,
    // keywords
    KwAs,
    KwAsync,
    KwAwait,
    KwBreak,
    KwConst,
    KwContinue,
    KwCrate,
    KwDyn,
    KwElse,
    KwEnum,
    KwExtern,
    KwFalse,
    KwFn,
    KwFor,
    KwIf,
    KwImpl,
    KwIn,
    KwLet,
    KwLoop,
    KwMatch,
    KwMod,
    KwMove,
    KwMut,
    KwPub,
    KwRef,
    KwReturn,
    KwSelfValue,
    KwSelfType,
    KwStatic,
    KwStruct,
    KwSuper,
    KwTrait,
    KwTrue,
    KwType,
    KwUnsafe,
    KwUse,
    KwWhere,
    KwWhile,
    KwYield,
    KwBox,
    KwTry,
    // delimiters
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    // punctuation
    Semi,
    Comma,
    Dot,
    DotDot,
    DotDotDot,
    DotDotEq,
    Colon,
    PathSep,
    RArrow,
    FatArrow,
    Pound,
    Dollar,
    Question,
    At,
    Tilde,
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    ShlEq,
    ShrEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Not,
    And,
    Or,
    AndAnd,
    OrOr,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    CaretEq,
    AndEq,
    OrEq,
    LArrow,
    /// an already-parsed expression spliced into a built-in macro expansion (fmtmac.rs);
    /// lo/hi are the expression's span, the id is in `Parser::interp`
    Interp,
    // Append only: Relax stores the discriminants of the preceding variants.
    Colour,
    Duration,
    AtId,
    ColonEq,
    PlusColon,
    Comment,
    DocComment,
    Apostrophe,
    OtherPunct,
}

/// 9 bytes, packed (12 with padding): files keep their tokens (attributes, macro bodies,
/// lazily parsed fn bodies), ~6 per source line. Fields are read by value only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, packed)]
pub struct Tok {
    pub kind: T,
    pub lo: u32,
    pub hi: u32,
}


pub struct LexError {
    pub pos: u32,
    pub msg: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Rust,
    Declarative,
    Gpu,
}

#[derive(Clone, Copy, Debug)]
pub struct LexOptions {
    pub edition: u16,
    pub mode: Mode,
    pub trivia: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenLexError {
    pub code: u16,
    pub lo: u32,
    pub hi: u32,
    pub msg: String,
}

/// Append tokens, including a final `Eof`. Rust without trivia is exactly `lex`.
/// The dialect modes require UTF-8 and preserve shader_ast's boundaries and errors.
pub fn lex_with(src: &[u8], opts: &LexOptions, out: &mut Vec<Tok>) -> Result<(), TokenLexError> {
    if opts.mode == Mode::Rust {
        let result = if opts.trivia {
            scan::<false, true, true, LexError>(src, "", opts.edition, out)
        } else {
            lex(src, opts.edition, out)
        };
        return result.map_err(|e| TokenLexError {
            code: 2,
            lo: e.pos,
            hi: if matches!(e.msg, "unexpected character" | "bad char literal" | "bad quote") {
                (e.pos as usize + src.get(e.pos as usize).map_or(0, |&c| utf8_len(c)))
                    .min(src.len()) as u32
            } else {
                src.len() as u32
            },
            msg: e.msg.into(),
        });
    }
    let source = std::str::from_utf8(src).map_err(|e| TokenLexError {
        code: 2,
        lo: e.valid_up_to() as u32,
        hi: (e.valid_up_to() + e.error_len().unwrap_or(src.len() - e.valid_up_to())) as u32,
        msg: "invalid source: expected UTF-8".into(),
    })?;
    // shader_ast currently has identical Declarative and Gpu lexical rules,
    // including durations in both modes. The parsers impose further restrictions.
    if opts.trivia {
        scan::<true, true, true, TokenLexError>(src, source, opts.edition, out)
    } else {
        scan::<true, false, true, TokenLexError>(src, source, opts.edition, out)
    }
}


impl T {
    /// Identifiers, raw identifiers and edition-classified keywords.
    #[inline]
    pub fn is_ident_like(self) -> bool {
        matches!(self, Self::Ident | Self::RawIdent)
            || (self as u8 >= Self::KwAs as u8 && self as u8 <= Self::KwTry as u8)
    }

    #[inline]
    pub fn is_number(self) -> bool {
        matches!(self, Self::Int | Self::Float)
    }

    /// Quoted literals; numbers and dialect atoms have their own classifications.
    #[inline]
    pub fn is_literal(self) -> bool {
        matches!(self, Self::Str | Self::ByteStr | Self::CStr | Self::RawStr | Self::Char | Self::Byte)
    }

    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, Self::Comment | Self::DocComment)
    }
}

impl Tok {
    /// (inline: called per token in every scan of another crate; the release profile has no LTO)
    #[inline]
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.lo as usize..self.hi as usize]
    }
}

/// Scan a declarative item before its opaque Rust islands have been separated.
/// Uses the same scanner and token kinds as `lex_with`, retaining trivia and EOF,
/// but leaves ASCII identifier, colour and exponent validation to the parser.
/// String/comment termination is still checked. Rust islands must subsequently
/// be lexed in Rust mode; remaining dialect atoms must be validated with lex_with.
pub fn lex_deferred(src: &[u8], edition: u16, out: &mut Vec<Tok>) -> Result<(), TokenLexError> {
    let source = std::str::from_utf8(src).map_err(|e| TokenLexError {
        code: 2,
        lo: e.valid_up_to() as u32,
        hi: (e.valid_up_to() + e.error_len().unwrap_or(src.len() - e.valid_up_to())) as u32,
        msg: "invalid source: expected UTF-8".into(),
    })?;
    scan::<true, true, false, TokenLexError>(src, source, edition, out)
}

/// Whether a complete colour atom has a supported width and only hex digits.
pub fn valid_colour(text: &str) -> bool {
    let digits = text.strip_prefix('#').unwrap_or(text);
    let digits = digits.strip_prefix('x').unwrap_or(digits);
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

// Specialize the error representation too: Relax's hot path still returns its
// small, static LexError and never constructs a String or a dialect diagnostic.
trait ScanError {
    fn ordinary(lo: usize, hi: usize, msg: &'static str) -> Self;
    fn dialect(code: u16, lo: usize, hi: usize, msg: String) -> Self;
}

impl ScanError for LexError {
    fn ordinary(lo: usize, _hi: usize, msg: &'static str) -> Self {
        Self { pos: lo as u32, msg }
    }

    fn dialect(_code: u16, _lo: usize, _hi: usize, _msg: String) -> Self {
        unreachable!("dialect errors are compiled out of the Rust scanner")
    }
}

impl ScanError for TokenLexError {
    fn ordinary(lo: usize, hi: usize, msg: &'static str) -> Self {
        let msg = match msg {
            "unterminated block comment" => "invalid comment: unterminated block comment",
            "unterminated raw string" => "invalid string: unterminated raw literal",
            _ => "invalid literal: unterminated quote",
        };
        Self::dialect(2, lo, hi, msg.into())
    }

    fn dialect(code: u16, lo: usize, hi: usize, msg: String) -> Self {
        Self { code, lo: lo as u32, hi: hi as u32, msg }
    }
}

/// Indexed 1-based locations. Columns count Unicode characters, unlike the
/// legacy byte-column `line_col`. Queries take logarithmic time without rescans.
#[derive(Clone, Debug)]
pub struct LineIndex {
    len: u32,
    lines: Vec<u32>,
    continuations: Vec<u32>,
}

impl LineIndex {
    pub fn new(src: &[u8]) -> Self {
        let mut lines = vec![0];
        let mut continuations = Vec::new();
        for (i, &c) in src.iter().enumerate() {
            if c == b'\n' {
                lines.push(i as u32 + 1);
            } else if c & 0xc0 == 0x80 {
                continuations.push(i as u32);
            }
        }
        Self { len: src.len() as u32, lines, continuations }
    }

    #[inline]
    pub fn line_col(&self, pos: u32) -> (usize, usize) {
        let pos = pos.min(self.len);
        let line = self.lines.partition_point(|&start| start <= pos);
        let start = self.lines[line - 1];
        let extra = self.continuations.partition_point(|&p| p < pos)
            - self.continuations.partition_point(|&p| p < start);
        (line, (pos - start) as usize - extra + 1)
    }
}

// Preserve Relax's byte classes, including its permissive non-ASCII handling.
// A shared 256-byte table keeps the identifier loop compact in every scanner
// specialization instead of depending on the optimizer's register allocation.
const IDENT_BYTES: [u8; 256] = {
    let mut classes = [0; 256];
    let mut i = 0;
    while i < 256 {
        let c = i as u8;
        classes[i] = if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            3
        } else if c.is_ascii_digit() {
            2
        } else {
            0
        };
        i += 1;
    }
    classes
};

#[inline]
fn is_ident_start(c: u8) -> bool {
    IDENT_BYTES[c as usize] & 1 != 0
}

static RAW_BODY: std::sync::OnceLock<&'static [&'static str]> = std::sync::OnceLock::new();

/// Extension point: fn-like macros whose `name! { .. }` bodies are not Rust (an embedder's
/// own language); the lexer keeps such a body as one RawBody token, which the macro's
/// registered expansion reads as text. Call once, before anything is lexed.
pub fn register_raw_body(names: &'static [&'static str]) {
    if RAW_BODY.set(names).is_err() {
        panic!("lexer::register_raw_body called twice");
    }
}

#[inline]
fn is_raw_body(ident: &[u8]) -> bool {
    match RAW_BODY.get() {
        Some(names) => names.iter().any(|n| n.as_bytes() == ident),
        None => false,
    }
}

#[inline]
fn is_ident_cont(c: u8) -> bool {
    IDENT_BYTES[c as usize] & 2 != 0
}

pub fn keyword(s: &[u8], edition: u16) -> Option<T> {
    let k = match s {
        b"as" => T::KwAs,
        b"break" => T::KwBreak,
        b"const" => T::KwConst,
        b"continue" => T::KwContinue,
        b"crate" => T::KwCrate,
        b"else" => T::KwElse,
        b"enum" => T::KwEnum,
        b"extern" => T::KwExtern,
        b"false" => T::KwFalse,
        b"fn" => T::KwFn,
        b"for" => T::KwFor,
        b"if" => T::KwIf,
        b"impl" => T::KwImpl,
        b"in" => T::KwIn,
        b"let" => T::KwLet,
        b"loop" => T::KwLoop,
        b"match" => T::KwMatch,
        b"mod" => T::KwMod,
        b"move" => T::KwMove,
        b"mut" => T::KwMut,
        b"pub" => T::KwPub,
        b"ref" => T::KwRef,
        b"return" => T::KwReturn,
        b"self" => T::KwSelfValue,
        b"Self" => T::KwSelfType,
        b"static" => T::KwStatic,
        b"struct" => T::KwStruct,
        b"super" => T::KwSuper,
        b"trait" => T::KwTrait,
        b"true" => T::KwTrue,
        b"type" => T::KwType,
        b"unsafe" => T::KwUnsafe,
        b"use" => T::KwUse,
        b"where" => T::KwWhere,
        b"while" => T::KwWhile,
        b"yield" => T::KwYield,
        b"box" => T::KwBox,
        b"async" if edition >= 2018 => T::KwAsync,
        b"await" if edition >= 2018 => T::KwAwait,
        b"dyn" if edition >= 2018 => T::KwDyn,
        b"try" if edition >= 2018 => T::KwTry,
        _ => return None,
    };
    Some(k)
}

pub fn lex(src: &[u8], edition: u16, out: &mut Vec<Tok>) -> Result<(), LexError> {
    scan::<false, false, true, LexError>(src, "", edition, out)
}

fn scan<const RX: bool, const TRIVIA: bool, const VALIDATE: bool, E: ScanError>(
    src: &[u8], source: &str, edition: u16, out: &mut Vec<Tok>,
) -> Result<(), E> {
    let n = src.len();
    let mut i = 0usize;
    // BOM
    if n >= 3 && &src[0..3] == b"\xEF\xBB\xBF" {
        i = 3;
    }
    // shebang (not an inner attribute)
    if (!RX || i == 0) && n >= i + 2 && src[i] == b'#' && src[i + 1] == b'!' {
        let mut j = i + 2;
        while !RX && j < n && (src[j] == b' ' || src[j] == b'\t') {
            j += 1;
        }
        if j >= n || src[j] != b'[' {
            let lo = i;
            while i < n && src[i] != b'\n' {
                i += 1;
            }
            if TRIVIA {
                out.push(Tok { kind: T::Comment, lo: lo as u32, hi: i as u32 });
            }
        }
    }
    let err = |pos: usize, msg: &'static str| E::ordinary(pos, n, msg);
    while i < n {
        let c = src[i];
        let lo = i;
        if RX && c >= 0x80 && rx_char(source, i).is_some_and(char::is_whitespace) {
            i += utf8_len(c);
            continue;
        }
        let kind = match c {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => {
                i += 1;
                // Indentation and other whitespace runs need no token dispatch
                // per byte. The accepted bytes are unchanged in every mode.
                while i < n && matches!(src[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
                    i += 1;
                }
                continue;
            }
            b'/' => {
                if i + 1 < n && src[i + 1] == b'/' {
                    while i < n && src[i] != b'\n' {
                        i += 1;
                    }
                    if !TRIVIA {
                        continue;
                    }
                    comment_kind(&src[lo..i])
                } else if i + 1 < n && src[i + 1] == b'*' {
                    let mut depth = 1;
                    i += 2;
                    while depth > 0 {
                        if i + 1 >= n {
                            return Err(err(lo, "unterminated block comment"));
                        }
                        if src[i] == b'/' && src[i + 1] == b'*' {
                            depth += 1;
                            i += 2;
                        } else if src[i] == b'*' && src[i + 1] == b'/' {
                            depth -= 1;
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if !TRIVIA {
                        continue;
                    }
                    comment_kind(&src[lo..i])
                } else if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::SlashEq
                } else {
                    i += 1;
                    T::Slash
                }
            }
            b'"' => {
                i = lex_quoted_mode::<RX>(src, i + 1, b'"').ok_or_else(|| err(lo, "unterminated string"))?;
                if !RX { i = lex_suffix(src, i); }
                T::Str
            }
            b'\'' => {
                // char literal or lifetime
                if RX {
                    let end = rx_suffix(source, i + 1);
                    if rx_char(source, i + 1).is_some_and(rx_start) && src.get(end) != Some(&b'\'') {
                        i += 1;
                        T::Apostrophe
                    } else {
                        i = lex_quoted_mode::<true>(src, i + 1, b'\'').ok_or_else(|| err(lo, "bad char"))?;
                        T::Char
                    }
                } else if i + 1 < n && src[i + 1] == b'\\' {
                    i = lex_quoted(src, i + 1, b'\'').ok_or_else(|| err(lo, "bad char"))?;
                    i = lex_suffix(src, i);
                    T::Char
                } else {
                    // one utf8 char
                    let mut j = i + 1;
                    if j >= n {
                        return Err(err(lo, "bad quote"));
                    }
                    j += utf8_len(src[j]);
                    if j < n && src[j] == b'\'' {
                        i = lex_suffix(src, j + 1);
                        T::Char
                    } else if is_ident_start(src[i + 1]) {
                        let mut j = i + 1;
                        // raw lifetime 'r#a
                        if j + 2 < n && src[j] == b'r' && src[j + 1] == b'#' && is_ident_start(src[j + 2]) {
                            j += 2;
                        }
                        while j < n && is_ident_cont(src[j]) {
                            j += 1;
                        }
                        i = j;
                        T::Lifetime
                    } else {
                        return Err(err(lo, "bad char literal"));
                    }
                }
            }
            b'0'..=b'9' => {
                let (j, k) = if RX {
                    lex_rx_number::<VALIDATE, E>(source, i)?
                } else {
                    lex_number(src, i)
                };
                i = j;
                k
            }
            b'(' => {
                i += 1;
                T::OpenParen
            }
            b')' => {
                i += 1;
                T::CloseParen
            }
            b'[' => {
                i += 1;
                T::OpenBracket
            }
            b']' => {
                i += 1;
                T::CloseBracket
            }
            b'{' => {
                i += 1;
                T::OpenBrace
            }
            b'}' => {
                i += 1;
                T::CloseBrace
            }
            b';' => {
                i += 1;
                T::Semi
            }
            b',' => {
                i += 1;
                T::Comma
            }
            b'.' => {
                if i + 1 < n && src[i + 1] == b'.' {
                    if i + 2 < n && src[i + 2] == b'.' {
                        i += 3;
                        T::DotDotDot
                    } else if i + 2 < n && src[i + 2] == b'=' {
                        i += 3;
                        T::DotDotEq
                    } else {
                        i += 2;
                        T::DotDot
                    }
                } else {
                    i += 1;
                    T::Dot
                }
            }
            b':' => {
                if i + 1 < n && src[i + 1] == b':' {
                    i += 2;
                    T::PathSep
                } else if RX && src.get(i + 1) == Some(&b'=') {
                    i += 2;
                    T::ColonEq
                } else {
                    i += 1;
                    T::Colon
                }
            }
            b'#' => {
                if RX && src.get(i + 1).is_some_and(|b| b.is_ascii_hexdigit() || *b == b'x') {
                    i += 1;
                    if src[i] == b'x' { i += 1; }
                    let digits = i;
                    while i < n && src[i].is_ascii_hexdigit() { i += 1; }
                    if VALIDATE && (!matches!(i - digits, 3 | 4 | 6 | 8) || rx_char(source, i).is_some_and(rx_cont)) {
                        return Err(E::dialect(2, lo, i, format!("invalid colour: {}", &source[lo..i])));
                    }
                    if !VALIDATE { i = rx_suffix(source, i); }
                    T::Colour
                } else {
                    i += 1;
                    T::Pound
                }
            }
            b'$' => {
                i += 1;
                T::Dollar
            }
            b'?' => {
                i += 1;
                T::Question
            }
            b'@' => {
                i += 1;
                if RX && rx_char(source, i).is_some_and(rx_start) {
                    if source[i..].starts_with("r#") { i += 2; }
                    i = rx_suffix(source, i);
                    T::AtId
                } else {
                    T::At
                }
            }
            b'~' => {
                i += 1;
                T::Tilde
            }
            b'=' => {
                if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::EqEq
                } else if i + 1 < n && src[i + 1] == b'>' {
                    i += 2;
                    T::FatArrow
                } else {
                    i += 1;
                    T::Eq
                }
            }
            b'!' => {
                if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::Ne
                } else {
                    i += 1;
                    T::Not
                }
            }
            b'<' => {
                if i + 1 < n && src[i + 1] == b'<' {
                    if i + 2 < n && src[i + 2] == b'=' {
                        i += 3;
                        T::ShlEq
                    } else {
                        i += 2;
                        T::Shl
                    }
                } else if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::Le
                } else if !RX && i + 1 < n && src[i + 1] == b'-' {
                    i += 2;
                    T::LArrow
                } else {
                    i += 1;
                    T::Lt
                }
            }
            b'>' => {
                if i + 1 < n && src[i + 1] == b'>' {
                    if i + 2 < n && src[i + 2] == b'=' {
                        i += 3;
                        T::ShrEq
                    } else {
                        i += 2;
                        T::Shr
                    }
                } else if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::Ge
                } else {
                    i += 1;
                    T::Gt
                }
            }
            b'-' => {
                if i + 1 < n && src[i + 1] == b'>' {
                    i += 2;
                    T::RArrow
                } else if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::MinusEq
                } else {
                    i += 1;
                    T::Minus
                }
            }
            b'+' => {
                if RX && src.get(i + 1) == Some(&b':') {
                    i += 2;
                    T::PlusColon
                } else {
                    op2(src, &mut i, T::Plus, T::PlusEq)
                }
            }
            b'*' => op2(src, &mut i, T::Star, T::StarEq),
            b'%' => op2(src, &mut i, T::Percent, T::PercentEq),
            b'^' => op2(src, &mut i, T::Caret, T::CaretEq),
            b'&' => {
                if i + 1 < n && src[i + 1] == b'&' {
                    i += 2;
                    T::AndAnd
                } else {
                    op2(src, &mut i, T::And, T::AndEq)
                }
            }
            b'|' => {
                if i + 1 < n && src[i + 1] == b'|' {
                    i += 2;
                    T::OrOr
                } else {
                    op2(src, &mut i, T::Or, T::OrEq)
                }
            }
            _ if if RX { rx_char(source, i).is_some_and(rx_start) } else { is_ident_start(c) } => {
                // prefixed literals
                if c == b'b' && i + 1 < n && src[i + 1] == b'\'' {
                    i = lex_quoted_mode::<RX>(src, i + 2, b'\'').ok_or_else(|| err(lo, "bad byte"))?;
                    if !RX { i = lex_suffix(src, i); }
                    T::Byte
                } else if (c == b'b' || c == b'c') && i + 1 < n && src[i + 1] == b'"' {
                    i = lex_quoted_mode::<RX>(src, i + 2, b'"').ok_or_else(|| err(lo, "bad string"))?;
                    if !RX { i = lex_suffix(src, i); }
                    if c == b'b' {
                        T::ByteStr
                    } else {
                        T::CStr
                    }
                } else if let Some(j) = raw_string_start(src, i) {
                    i = lex_raw_string(src, j).ok_or_else(|| err(lo, "unterminated raw string"))?;
                    if !RX { i = lex_suffix(src, i); }
                    // br".." is a raw byte string, cr".." a raw C string
                    match c {
                        b'b' => T::ByteStr,
                        b'c' => T::CStr,
                        _ => T::RawStr,
                    }
                } else if RX {
                    let raw = c == b'r' && src.get(i + 1) == Some(&b'#');
                    if raw { i += 2; }
                    // shader_ast consumes the first character after r# even if
                    // it is not an identifier start. At EOF it used to panic;
                    // retain the available r# as a RawIdent instead.
                    if i < n { i += utf8_len(src[i]); }
                    i = rx_suffix(source, i);
                    if raw { T::RawIdent } else { keyword(&src[lo..i], edition).unwrap_or(T::Ident) }
                } else if c == b'r' && i + 2 < n && src[i + 1] == b'#' && is_ident_start(src[i + 2]) {
                    let mut j = i + 2;
                    while j < n && is_ident_cont(src[j]) {
                        j += 1;
                    }
                    i = j;
                    T::RawIdent
                } else {
                    let mut j = i + 1;
                    while j < n && is_ident_cont(src[j]) {
                        j += 1;
                    }
                    let k = keyword(&src[i..j], edition).unwrap_or(T::Ident);
                    if k == T::Ident && is_raw_body(&src[i..j]) {
                        // `name! {` of a registered raw-body macro: hand the body over untokenised
                        let mut m = j;
                        while m < n && (src[m] == b' ' || src[m] == b'\t' || src[m] == b'\n' || src[m] == b'\r') {
                            m += 1;
                        }
                        if m < n && src[m] == b'!' {
                            let mut q = m + 1;
                            while q < n && (src[q] == b' ' || src[q] == b'\t' || src[q] == b'\n' || src[q] == b'\r') {
                                q += 1;
                            }
                            if q < n && src[q] == b'{' {
                                let end = skip_braced(src, q).ok_or_else(|| err(q, "unterminated macro body"))?;
                                out.push(Tok { kind: T::Ident, lo: i as u32, hi: j as u32 });
                                out.push(Tok { kind: T::Not, lo: m as u32, hi: m as u32 + 1 });
                                out.push(Tok { kind: T::OpenBrace, lo: q as u32, hi: q as u32 + 1 });
                                out.push(Tok { kind: T::RawBody, lo: q as u32 + 1, hi: end as u32 });
                                out.push(Tok { kind: T::CloseBrace, lo: end as u32, hi: end as u32 + 1 });
                                i = end + 1;
                                continue;
                            }
                        }
                    }
                    i = j;
                    k
                }
            }
            _ if RX => {
                i += utf8_len(c);
                T::OtherPunct
            }
            _ => return Err(err(lo, "unexpected character")),
        };
        if RX && VALIDATE && matches!(kind, T::Ident | T::RawIdent | T::AtId) && !src[lo..i].is_ascii() {
            return Err(E::dialect(15, lo, i, format!("identifier {} must be ASCII", &source[lo..i])));
        }
        out.push(Tok {
            kind,
            lo: lo as u32,
            hi: i as u32,
        });
    }
    out.push(Tok {
        kind: T::Eof,
        lo: n as u32,
        hi: n as u32,
    });
    Ok(())
}

#[inline]
fn op2(src: &[u8], i: &mut usize, a: T, b: T) -> T {
    if *i + 1 < src.len() && src[*i + 1] == b'=' {
        *i += 2;
        b
    } else {
        *i += 1;
        a
    }
}

fn utf8_len(c: u8) -> usize {
    if c < 0x80 {
        1
    } else if c >> 5 == 0b110 {
        2
    } else if c >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// From just after the opening quote; returns index after the closing quote.
fn lex_quoted(src: &[u8], i: usize, q: u8) -> Option<usize> {
    lex_quoted_mode::<false>(src, i, q)
}

fn lex_quoted_mode<const RX: bool>(src: &[u8], mut i: usize, q: u8) -> Option<usize> {
    let n = src.len();
    while i < n {
        let c = src[i];
        if c == b'\\' {
            i += if RX { 1 + src.get(i + 1).map_or(0, |&c| utf8_len(c)) } else { 2 };
        } else if c == q {
            return Some(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

fn lex_suffix(src: &[u8], mut i: usize) -> usize {
    if i < src.len() && is_ident_start(src[i]) {
        while i < src.len() && is_ident_cont(src[i]) {
            i += 1;
        }
    }
    i
}

/// r"..", r#".."#, br"..", cr"..": returns the index of the first `#` or `"` after the prefix.
fn raw_string_start(src: &[u8], i: usize) -> Option<usize> {
    let n = src.len();
    let j = if src[i] == b'r' {
        i + 1
    } else if (src[i] == b'b' || src[i] == b'c') && i + 1 < n && src[i + 1] == b'r' {
        i + 2
    } else {
        return None;
    };
    let mut k = j;
    while k < n && src[k] == b'#' {
        k += 1;
    }
    if k < n && src[k] == b'"' {
        Some(j)
    } else {
        None
    }
}

fn lex_raw_string(src: &[u8], mut i: usize) -> Option<usize> {
    let n = src.len();
    let mut hashes = 0;
    while src[i] == b'#' {
        hashes += 1;
        i += 1;
    }
    i += 1; // quote
    while i < n {
        if src[i] == b'"' {
            let mut k = 0;
            while k < hashes && i + 1 + k < n && src[i + 1 + k] == b'#' {
                k += 1;
            }
            if k == hashes {
                return Some(i + 1 + hashes);
            }
        }
        i += 1;
    }
    None
}

// Keep this in the Rust loop even with both trivia specializations: otherwise
// LLVM can outline a helper that Relax's single entry used to inline.
#[inline(always)]
fn lex_number(src: &[u8], mut i: usize) -> (usize, T) {
    let n = src.len();
    let mut float = false;
    if src[i] == b'0' && i + 1 < n && matches!(src[i + 1], b'x' | b'o' | b'b') {
        let hex = src[i + 1] == b'x';
        i += 2;
        while i < n && (src[i].is_ascii_digit() || src[i] == b'_' || (hex && src[i].is_ascii_hexdigit())) {
            i += 1;
        }
        return (lex_suffix(src, i), T::Int);
    }
    while i < n && (src[i].is_ascii_digit() || src[i] == b'_') {
        i += 1;
    }
    if i < n && src[i] == b'.' {
        let next = if i + 1 < n { src[i + 1] } else { 0 };
        if next.is_ascii_digit() {
            float = true;
            i += 1;
            while i < n && (src[i].is_ascii_digit() || src[i] == b'_') {
                i += 1;
            }
        } else if next != b'.' && !is_ident_start(next) {
            // `1.` is a float
            return (i + 1, T::Float);
        }
    }
    if i < n && (src[i] == b'e' || src[i] == b'E') {
        let mut j = i + 1;
        if j < n && (src[j] == b'+' || src[j] == b'-') {
            j += 1;
        }
        while j < n && src[j] == b'_' {
            j += 1;
        }
        if j < n && src[j].is_ascii_digit() {
            float = true;
            i = j;
            while i < n && (src[i].is_ascii_digit() || src[i] == b'_') {
                i += 1;
            }
        }
    }
    let s = i;
    let i = lex_suffix(src, i);
    if i > s && src[s] == b'f' {
        float = true;
    }
    (i, if float { T::Float } else { T::Int })
}

fn comment_kind(text: &[u8]) -> T {
    if (text.starts_with(b"///") && !text.starts_with(b"////"))
        || (text.starts_with(b"/**") && !text.starts_with(b"/***"))
        || text.starts_with(b"//!")
        || text.starts_with(b"/*!")
    {
        T::DocComment
    } else {
        T::Comment
    }
}

fn rx_char(source: &str, i: usize) -> Option<char> {
    source[i..].chars().next()
}

fn rx_start(c: char) -> bool {
    c == '_' || c.is_alphabetic() || (!c.is_ascii() && !c.is_whitespace() && !c.is_control())
}

fn rx_cont(c: char) -> bool {
    rx_start(c) || c.is_ascii_digit()
}

fn rx_suffix(source: &str, mut i: usize) -> usize {
    while let Some(c) = rx_char(source, i).filter(|&c| rx_cont(c)) {
        i += c.len_utf8();
    }
    i
}

// The dialect consumes exponents even when they are malformed, validates them,
// and uses character-based identifier continuations for numeric suffixes.
fn lex_rx_number<const VALIDATE: bool, E: ScanError>(source: &str, lo: usize) -> Result<(usize, T), E> {
    let src = source.as_bytes();
    let mut i = lo + 1;
    let radix = src[lo] == b'0' && src.get(i).is_some_and(|b| matches!(b, b'x' | b'o' | b'b'));
    let mut float = false;
    if radix {
        i += 1;
        while src.get(i).is_some_and(|b| b.is_ascii_hexdigit() || *b == b'_') {
            i += 1;
        }
    } else {
        while src.get(i).is_some_and(|b| b.is_ascii_digit() || *b == b'_') {
            i += 1;
        }
        if src.get(i) == Some(&b'.') && src.get(i + 1) != Some(&b'.')
            && !rx_char(source, i + 1).is_some_and(rx_start)
        {
            float = true;
            i += 1;
            while src.get(i).is_some_and(|b| b.is_ascii_digit() || *b == b'_') {
                i += 1;
            }
        }
        if src.get(i).is_some_and(|b| matches!(b, b'e' | b'E')) {
            float = true;
            i += 1;
            if src.get(i).is_some_and(|b| matches!(b, b'+' | b'-')) { i += 1; }
            let exponent = i;
            while src.get(i).is_some_and(|b| b.is_ascii_digit() || *b == b'_') {
                i += 1;
            }
            if VALIDATE && i == exponent {
                return Err(E::dialect(2, lo, i, "invalid number: missing exponent".into()));
            }
        }
    }
    let suffix = i;
    i = rx_suffix(source, i);
    let kind = if !radix && matches!(&source[suffix..i], "s" | "ms") {
        T::Duration
    } else if !radix && (float || src.get(suffix) == Some(&b'f')) {
        T::Float
    } else {
        T::Int
    };
    Ok((i, kind))
}

/// 1-based line and column of a byte position.
/// Byte offsets where each line of `src` starts (0 first): with line_col_at, line_col without
/// scanning the source for every position (a file's macro sites asked it of the whole prefix each:
/// quadratic in a 1.7 MB file).
pub fn line_starts(src: &[u8]) -> Vec<u32> {
    let mut v = vec![0u32];
    for (i, &c) in src.iter().enumerate() {
        if c == b'\n' {
            v.push(i as u32 + 1);
        }
    }
    v
}

/// line_col(src, pos) from `src`'s line_starts (`len` = src.len()): the same 1-based line and
/// byte column.
pub fn line_col_at(lines: &[u32], len: usize, pos: u32) -> (usize, usize) {
    let pos = pos.min(len as u32);
    let line = lines.partition_point(|&start| start <= pos);
    (line, (pos - lines[line - 1]) as usize + 1)
}

pub fn line_col(src: &[u8], pos: u32) -> (usize, usize) {
    let pos = (pos as usize).min(src.len());
    let mut line = 1;
    let mut col = 1;
    for &c in &src[..pos] {
        if c == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// From an opening `{`, returns the index of its matching `}`. Strings, raw strings,
/// chars/lifetimes and comments follow the Rust lexer's rules (raw bodies are
/// Rust-lexable today, so the boundaries are exactly rustc's).
pub fn skip_braced(src: &[u8], open: usize) -> Option<usize> {
    let n = src.len();
    let mut depth = 0i32;
    let mut i = open;
    while i < n {
        let c = src[i];
        match c {
            b'{' | b'(' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b')' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return if c == b'}' { Some(i) } else { None };
                }
                i += 1;
            }
            b'/' if i + 1 < n && src[i + 1] == b'/' => {
                while i < n && src[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && src[i + 1] == b'*' => {
                let mut d = 1;
                i += 2;
                while d > 0 && i + 1 < n {
                    if src[i] == b'/' && src[i + 1] == b'*' {
                        d += 1;
                        i += 2;
                    } else if src[i] == b'*' && src[i + 1] == b'/' {
                        d -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'"' => i = lex_quoted(src, i + 1, b'"')?,
            b'\'' => {
                if i + 1 < n && src[i + 1] == b'\\' {
                    i = lex_quoted(src, i + 1, b'\'')?;
                } else if i + 2 < n && src[i + 1 + utf8_len(src[i + 1]).min(n - i - 2)] == b'\'' {
                    i += 1 + utf8_len(src[i + 1]) + 1;
                } else {
                    // lifetime-like: skip the quote only
                    i += 1;
                }
            }
            b'r' | b'b' | b'c' => {
                if (i == 0 || !is_ident_cont(src[i - 1])) && raw_string_start(src, i).is_some() {
                    let j = raw_string_start(src, i)?;
                    i = lex_raw_string(src, j)?;
                } else if (c == b'b' || c == b'c') && (i == 0 || !is_ident_cont(src[i - 1])) && i + 1 < n && src[i + 1] == b'"' {
                    i = lex_quoted(src, i + 2, b'"')?;
                } else {
                    // identifier
                    while i < n && is_ident_cont(src[i]) {
                        i += 1;
                    }
                }
            }
            _ if is_ident_start(c) => {
                while i < n && is_ident_cont(src[i]) {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    None
}
