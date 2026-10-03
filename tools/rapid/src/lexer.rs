//! Lexer: source bytes -> flat token list. Tokens carry byte spans only;
//! identifiers and literals are read back from the source when needed, so
//! lexing and parsing of different files share no state.
//! Doc comments are dropped (they are attributes with no meaning to us).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum T {
    Eof,
    Ident,
    /// raw Splash source of a `script_mod! { ... }` body (between the braces)
    Splash,
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
}

#[derive(Clone, Copy)]
pub struct Tok {
    pub kind: T,
    pub lo: u32,
    pub hi: u32,
}


pub struct LexError {
    pub pos: u32,
    pub msg: &'static str,
}

#[inline]
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}
#[inline]
fn is_ident_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
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
    let n = src.len();
    let mut i = 0usize;
    // BOM
    if n >= 3 && &src[0..3] == b"\xEF\xBB\xBF" {
        i = 3;
    }
    // shebang (not an inner attribute)
    if n >= i + 2 && src[i] == b'#' && src[i + 1] == b'!' {
        let mut j = i + 2;
        while j < n && (src[j] == b' ' || src[j] == b'\t') {
            j += 1;
        }
        if j >= n || src[j] != b'[' {
            while i < n && src[i] != b'\n' {
                i += 1;
            }
        }
    }
    let err = |pos: usize, msg: &'static str| LexError {
        pos: pos as u32,
        msg,
    };
    while i < n {
        let c = src[i];
        let lo = i;
        let kind = match c {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => {
                i += 1;
                continue;
            }
            b'/' => {
                if i + 1 < n && src[i + 1] == b'/' {
                    while i < n && src[i] != b'\n' {
                        i += 1;
                    }
                    continue;
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
                    continue;
                } else if i + 1 < n && src[i + 1] == b'=' {
                    i += 2;
                    T::SlashEq
                } else {
                    i += 1;
                    T::Slash
                }
            }
            b'"' => {
                i = lex_quoted(src, i + 1, b'"').ok_or_else(|| err(lo, "unterminated string"))?;
                i = lex_suffix(src, i);
                T::Str
            }
            b'\'' => {
                // char literal or lifetime
                if i + 1 < n && src[i + 1] == b'\\' {
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
                let (j, k) = lex_number(src, i);
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
                } else {
                    i += 1;
                    T::Colon
                }
            }
            b'#' => {
                i += 1;
                T::Pound
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
                T::At
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
                } else if i + 1 < n && src[i + 1] == b'-' {
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
            b'+' => op2(src, &mut i, T::Plus, T::PlusEq),
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
            _ if is_ident_start(c) => {
                // prefixed literals
                if c == b'b' && i + 1 < n && src[i + 1] == b'\'' {
                    i = lex_quoted(src, i + 2, b'\'').ok_or_else(|| err(lo, "bad byte"))?;
                    i = lex_suffix(src, i);
                    T::Byte
                } else if (c == b'b' || c == b'c') && i + 1 < n && src[i + 1] == b'"' {
                    i = lex_quoted(src, i + 2, b'"').ok_or_else(|| err(lo, "bad string"))?;
                    i = lex_suffix(src, i);
                    if c == b'b' {
                        T::ByteStr
                    } else {
                        T::CStr
                    }
                } else if let Some(j) = raw_string_start(src, i) {
                    i = lex_raw_string(src, j).ok_or_else(|| err(lo, "unterminated raw string"))?;
                    i = lex_suffix(src, i);
                    T::RawStr
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
                    if k == T::Ident && &src[i..j] == b"script_mod" {
                        // `script_mod! {`: hand the body to Splash untokenised
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
                                let end = skip_braced(src, q).ok_or_else(|| err(q, "unterminated script_mod! body"))?;
                                out.push(Tok { kind: T::Ident, lo: i as u32, hi: j as u32 });
                                out.push(Tok { kind: T::Not, lo: m as u32, hi: m as u32 + 1 });
                                out.push(Tok { kind: T::OpenBrace, lo: q as u32, hi: q as u32 + 1 });
                                out.push(Tok { kind: T::Splash, lo: q as u32 + 1, hi: end as u32 });
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
            _ => return Err(err(lo, "unexpected character")),
        };
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
fn lex_quoted(src: &[u8], mut i: usize, q: u8) -> Option<usize> {
    let n = src.len();
    while i < n {
        let c = src[i];
        if c == b'\\' {
            i += 2;
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

/// 1-based line and column of a byte position.
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
/// chars/lifetimes and comments follow the Rust lexer's rules (Splash bodies are
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
