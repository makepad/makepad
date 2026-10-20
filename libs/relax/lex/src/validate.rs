//! Literal and identifier validation of the Rust scanner, after rustc_parse's lexer cooking
//! (`cook_lexer_literal`, `unescape_error_reporting`) and rustc_literal_escaper's unescape
//! rules: bad escapes, bare CRs, non-ASCII in byte literals, wrong digits for a base, empty
//! exponents, unknown prefixes and non-identifier characters. Only the first error is kept.

use crate::lexer::{Tok, T};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Char,
    Byte,
    Str,
    ByteStr,
    CStr,
}

impl Mode {
    fn is_byte(self) -> bool {
        matches!(self, Mode::Byte | Mode::ByteStr)
    }
    fn allow_high_bytes(self) -> bool {
        matches!(self, Mode::Byte | Mode::ByteStr | Mode::CStr)
    }
    fn is_single(self) -> bool {
        matches!(self, Mode::Char | Mode::Byte)
    }
}

type VErr = (u32, Cow<'static, str>);

fn err<T>(pos: usize, msg: impl Into<Cow<'static, str>>) -> Result<T, VErr> {
    Err((pos as u32, msg.into()))
}

fn char_at(src: &[u8], i: usize) -> Option<char> {
    let n = match src.get(i)? {
        0..=0x7f => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        _ => 4,
    };
    std::str::from_utf8(src.get(i..i + n)?).ok()?.chars().next()
}

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Literal validation is a diagnostic: `makepad check` turns it on; a run never pays for it.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// rustc_parse lexer `cook_doc_comment`: a doc comment holds no bare CR (one not followed by LF).
/// The comments are the gaps between tokens (and after the last one).
fn doc_comment_crs(src: &[u8], toks: &[Tok]) -> Result<(), VErr> {
    let mut prev_hi = 0usize;
    let check = |lo: usize, hi: usize| -> Result<(), VErr> {
        let hi = hi.min(src.len());
        let g = &src[lo.min(hi)..hi];
        let mut i = 0;
        while i < g.len() {
            match g[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                b'/' if g.get(i + 1) == Some(&b'/') => {
                    let doc = (g.get(i + 2) == Some(&b'/') && g.get(i + 3) != Some(&b'/')) || g.get(i + 2) == Some(&b'!');
                    let start = i;
                    while i < g.len() && g[i] != b'\n' { i += 1; }
                    if doc {
                        if let Some(c) = (start..i).find(|&k| g[k] == b'\r' && g.get(k + 1) != Some(&b'\n')) {
                            return err(lo + c, "bare CR not allowed in doc-comment");
                        }
                    }
                }
                b'/' if g.get(i + 1) == Some(&b'*') => {
                    let doc = (g.get(i + 2) == Some(&b'*') && !matches!(g.get(i + 3), Some(b'*') | Some(b'/'))) || g.get(i + 2) == Some(&b'!');
                    let start = i;
                    let mut depth = 0;
                    while i < g.len() {
                        if g[i] == b'/' && g.get(i + 1) == Some(&b'*') { depth += 1; i += 2; }
                        else if g[i] == b'*' && g.get(i + 1) == Some(&b'/') { depth -= 1; i += 2; if depth == 0 { break; } }
                        else { i += 1; }
                    }
                    if doc {
                        if let Some(c) = (start..i.min(g.len())).find(|&k| g[k] == b'\r' && g.get(k + 1) != Some(&b'\n')) {
                            return err(lo + c, "bare CR not allowed in doc-comment");
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
        Ok(())
    };
    for t in toks {
        check(prev_hi, t.lo as usize)?;
        prev_hi = t.hi as usize;
    }
    check(prev_hi, src.len())
}

/// Validates the tokens `toks[from..]` of `src`; the first error is returned.
pub fn validate(src: &[u8], toks: &[Tok], edition: u16) -> Result<(), VErr> {
    doc_comment_crs(src, toks)?;
    // Delimiter stack: true inside a macro invocation/definition or an attribute, whose tokens are
    // not literals yet. rustc cooks a literal's value and suffix when the parser consumes it
    // (`report_lit_error`), so those checks are skipped there; the lexer's own (escapes, digits of a
    // base, exponents, `_` suffix, token starts) apply everywhere.
    let mut stack: Vec<bool> = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        let (lo, hi) = (t.lo as usize, t.hi as usize);
        let parse_level = !stack.last().copied().unwrap_or(false);
        match t.kind {
            T::OpenParen | T::OpenBracket | T::OpenBrace => {
                let prev = |n: usize| if k >= n { Some(toks[k - n].kind) } else { None };
                let bang = prev(1) == Some(T::Not) || (prev(2) == Some(T::Not) && matches!(prev(1), Some(T::Ident)));
                let attr = t.kind == T::OpenBracket && (prev(1) == Some(T::Pound) || (prev(1) == Some(T::Not) && prev(2) == Some(T::Pound)));
                // (the std macros that parse their arguments as expressions consume the literals)
                let parsed_args = bang && prev(1) == Some(T::Not) && k >= 2 && toks[k - 2].kind == T::Ident && matches!(&src[toks[k - 2].lo as usize..toks[k - 2].hi as usize],
                    b"println" | b"print" | b"eprintln" | b"eprint" | b"format" | b"format_args" | b"write" | b"writeln" | b"panic" | b"assert" | b"assert_eq" | b"assert_ne"
                    | b"debug_assert" | b"debug_assert_eq" | b"debug_assert_ne" | b"vec" | b"unreachable" | b"todo" | b"unimplemented" | b"dbg");
                stack.push(!parse_level || (bang && !parsed_args) || attr);
            }
            T::CloseParen | T::CloseBracket | T::CloseBrace => {
                stack.pop();
            }
            _ => {}
        }
        match t.kind {
            T::Str => quoted(src, lo, hi, 0, b'"', Mode::Str, parse_level)?,
            T::ByteStr if src[lo + 1] == b'"' => quoted(src, lo, hi, 1, b'"', Mode::ByteStr, parse_level)?,
            T::CStr if src[lo + 1] == b'"' => quoted(src, lo, hi, 1, b'"', Mode::CStr, parse_level)?,
            T::ByteStr | T::CStr | T::RawStr => raw(src, lo, hi, t.kind)?,
            T::Char => quoted(src, lo, hi, 0, b'\'', Mode::Char, parse_level)?,
            T::Byte => quoted(src, lo, hi, 1, b'\'', Mode::Byte, parse_level)?,
            T::Int | T::Float => number(src, lo, hi, t.kind == T::Float, parse_level)?,
            T::Ident | T::RawIdent => {
                if src[lo..hi].is_ascii() {
                    // an ASCII identifier directly followed by a quote is a reserved prefix
                    // (rustc_lexer `UnknownPrefix`; an error from edition 2021)
                    if edition >= 2021 && t.kind == T::Ident {
                        if let Some(n) = toks.get(k + 1) {
                            if n.lo as usize == hi && matches!(n.kind, T::Str | T::Char) {
                                return err(lo, format!("prefix `{}` is unknown", String::from_utf8_lossy(&src[lo..hi])));
                            }
                        }
                    }
                } else {
                    ident(src, lo, hi)?;
                }
                if t.kind == T::RawIdent && matches!(&src[lo + 2..hi], b"self" | b"super" | b"crate" | b"Self" | b"_") {
                    return err(lo, format!("`{}` cannot be a raw identifier", String::from_utf8_lossy(&src[lo + 2..hi])));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn ident(src: &[u8], lo: usize, hi: usize) -> Result<(), VErr> {
    let mut i = lo;
    while i < hi {
        let c = match char_at(src, i) {
            Some(c) => c,
            None => return err(i, "invalid UTF-8"),
        };
        // (no Unicode property tables here: only what is certainly not an identifier character is
        // rejected -- the emoji blocks, whitespace, control and format characters)
        if !c.is_ascii() && (is_emoji_approx(c) || c.is_whitespace() || c.is_control() || matches!(c as u32, 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFEFF)) {
            if c == '\u{1F980}' {
                return err(i, "Ferris cannot be used as an identifier");
            }
            if is_emoji_approx(c) {
                return err(i, format!("identifiers cannot contain emoji: `{}`", String::from_utf8_lossy(&src[lo..hi])));
            }
            return err(i, format!("unknown start of token: \\u{{{:x}}}", c as u32));
        }
        i += c.len_utf8();
    }
    Ok(())
}

fn is_emoji_approx(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2190..=0x21FF | 0x2300..=0x23FF | 0x2460..=0x24FF | 0x25A0..=0x27BF
        | 0x2900..=0x297F | 0x2B00..=0x2BFF | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122
        | 0x2139 | 0xFE0F | 0x20E3 | 0xE0020..=0xE007F)
}

const NUMBER_SUFFIXES: &[&str] = &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f16", "f32", "f64", "f128"];

fn number(src: &[u8], lo: usize, hi: usize, float: bool, parse_level: bool) -> Result<(), VErr> {
    let b = &src[lo..hi];
    // the digits of the literal (suffix excluded) and, for an integer, its value
    let body_end;
    let mut radix = 10u32;
    if b.len() >= 2 && b[0] == b'0' && matches!(b[1], b'b' | b'o' | b'x') {
        radix = match b[1] {
            b'b' => 2,
            b'o' => 8,
            _ => 16,
        };
        let mut j = 2;
        let mut digits = 0;
        while j < b.len() && (b[j] == b'_' || b[j].is_ascii_digit() || (radix == 16 && b[j].is_ascii_hexdigit())) {
            if b[j] != b'_' {
                if b[j].is_ascii_digit() && (b[j] - b'0') as u32 >= radix {
                    return err(lo + j, format!("invalid digit for a base {radix} literal"));
                }
                digits += 1;
            }
            j += 1;
        }
        if digits == 0 {
            return err(lo, "E0768: no valid digits found for number");
        }
        body_end = j;
    } else {
        // decimal: digits, optional fraction and exponent, then the suffix; a suffix that starts
        // with `e`/`E` is an exponent without digits
        let mut j = 0;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'_') {
            j += 1;
        }
        if j < b.len() && b[j] == b'.' {
            j += 1;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'_') {
                j += 1;
            }
        }
        if j < b.len() && matches!(b[j], b'e' | b'E') {
            let mut k = j + 1;
            if k < b.len() && matches!(b[k], b'+' | b'-') {
                k += 1;
            }
            while k < b.len() && b[k] == b'_' {
                k += 1;
            }
            if !(k < b.len() && b[k].is_ascii_digit()) {
                return err(lo + j, "expected at least one digit in exponent");
            }
            while k < b.len() && (b[k].is_ascii_digit() || b[k] == b'_') {
                k += 1;
            }
            j = k;
        }
        body_end = j;
    }
    // (an integer's digits are all the literal holds before the suffix)
    let suffix = std::str::from_utf8(&b[body_end..]).unwrap_or("?");
    if parse_level && !suffix.is_empty() && !NUMBER_SUFFIXES.contains(&suffix) {
        return err(lo + body_end, format!("invalid suffix `{suffix}` for {} literal", if float { "float" } else { "number" }));
    }
    if parse_level && !float {
        let digits = if radix == 10 { &b[..body_end] } else { &b[2..body_end] };
        let mut v: u128 = 0;
        for &c in digits {
            if c == b'_' {
                continue;
            }
            let d = (c as char).to_digit(radix).unwrap_or(0) as u128;
            match v.checked_mul(radix as u128).and_then(|x| x.checked_add(d)) {
                Some(x) => v = x,
                None => return err(lo, "integer literal is too large"),
            }
        }
    }
    Ok(())
}

fn raw(src: &[u8], lo: usize, hi: usize, kind: T) -> Result<(), VErr> {
    // prefix: r | br | cr, then #*, then the quote
    let mut i = lo;
    while src[i] != b'#' && src[i] != b'"' {
        i += 1;
    }
    let mut hashes = 0;
    while src[i] == b'#' {
        hashes += 1;
        i += 1;
    }
    if hashes > 255 {
        return err(lo, format!("too many `#` symbols: raw strings may be delimited by up to 255 `#` symbols, but found {hashes}"));
    }
    let body_lo = i + 1;
    // the closing quote: the last quote of the token followed by the hashes
    let mut end = hi;
    while end > body_lo && src[end - 1] != b'"' {
        end -= 1;
    }
    let body_hi = end.saturating_sub(1).max(body_lo);
    let mut j = body_lo;
    while j < body_hi {
        let c = src[j];
        if c == b'\r' && src.get(j + 1) != Some(&b'\n') {
            return err(j, "bare CR not allowed in raw string");
        }
        if c >= 0x80 && kind == T::ByteStr {
            let ch = char_at(src, j).unwrap_or('\u{fffd}');
            return err(j, "non-ASCII character in raw byte string literal".to_string() + &format!(" ({ch:?})"));
        }
        if c == 0 && kind == T::CStr {
            return err(j, "null characters in C string literals are not supported");
        }
        j += 1;
    }
    Ok(())
}

fn quoted(src: &[u8], lo: usize, hi: usize, prefix: usize, q: u8, mode: Mode, parse_level: bool) -> Result<(), VErr> {
    let body_lo = lo + prefix + 1;
    // the closing quote is the last one of the token (a suffix holds no quotes)
    let mut end = hi;
    while end > body_lo && src[end - 1] != q {
        end -= 1;
    }
    let body_hi = end.saturating_sub(1).max(body_lo);
    if end < hi {
        let suffix = String::from_utf8_lossy(&src[end..hi]).into_owned();
        if suffix == "_" {
            return err(end, "underscore literal suffix is not allowed");
        }
        // (any other suffix on a quoted literal is judged when the parser consumes it)
        if parse_level {
            return err(end, format!("suffixes on {} literals are invalid", if mode.is_single() { "char" } else { "string" }));
        }
    }
    // fast path: nothing to check in plain ASCII text without escapes
    let body = &src[body_lo..body_hi];
    if !mode.is_single() && !body.iter().any(|&c| c == b'\\' || c == b'\r' || c >= 0x80 || c == 0) {
        return Ok(());
    }
    let mut i = body_lo;
    let mut units = 0;
    if body.is_empty() && mode.is_single() {
        return err(lo, "empty character literal");
    }
    while i < body_hi {
        let c = src[i];
        let start = i;
        match c {
            b'\\' => {
                i += 1;
                let Some(&e) = src.get(i).filter(|_| i < body_hi) else {
                    return err(start, "invalid trailing slash in literal");
                };
                i += 1;
                match e {
                    b'"' | b'\'' | b'n' | b'r' | b't' | b'\\' => {}
                    b'0' => {
                        if mode == Mode::CStr {
                            return err(start, "null characters in C string literals are not supported");
                        }
                    }
                    b'x' => {
                        let mut v = 0u32;
                        for k in 0..2 {
                            let Some(&d) = src.get(i).filter(|_| i < body_hi) else {
                                return err(start, "numeric character escape is too short");
                            };
                            let Some(dv) = (d as char).to_digit(16) else {
                                let ch = char_at(src, i).unwrap_or('?');
                                return err(i, format!("invalid character in numeric character escape: `{ch}`"));
                            };
                            v = v * 16 + dv;
                            i += 1;
                            let _ = k;
                        }
                        if !mode.allow_high_bytes() && v > 0x7f {
                            return err(start, "out of range hex escape");
                        }
                        if mode == Mode::CStr && v == 0 {
                            return err(start, "null characters in C string literals are not supported");
                        }
                    }
                    b'u' => {
                        if src.get(i) != Some(&b'{') || i >= body_hi {
                            return err(start, "incorrect unicode escape sequence");
                        }
                        i += 1;
                        let mut value = 0u32;
                        let mut n_digits = 0;
                        let mut closed = false;
                        let mut first = true;
                        while i < body_hi {
                            let d = src[i];
                            if d == b'}' {
                                if first {
                                    return err(start, "empty unicode escape");
                                }
                                i += 1;
                                closed = true;
                                break;
                            }
                            if d == b'_' {
                                if first {
                                    return err(start, "invalid start of unicode escape: `_`");
                                }
                                i += 1;
                                continue;
                            }
                            first = false;
                            let Some(dv) = (d as char).to_digit(16) else {
                                let ch = char_at(src, i).unwrap_or('?');
                                return err(i, format!("invalid character in unicode escape: `{ch}`"));
                            };
                            n_digits += 1;
                            if n_digits > 6 {
                                return err(start, "overlong unicode escape");
                            }
                            value = value * 16 + dv;
                            i += 1;
                        }
                        if !closed {
                            return err(start, "unterminated unicode escape");
                        }
                        if mode.is_byte() {
                            return err(start, "unicode escape in byte string");
                        }
                        if (0xD800..=0xDFFF).contains(&value) || value > 0x10FFFF {
                            return err(start, "invalid unicode character escape");
                        }
                    }
                    b'\n' if !mode.is_single() => {
                        while i < body_hi && matches!(src[i], b' ' | b'\n' | b'\t' | b'\r') {
                            i += 1;
                        }
                    }
                    b'\r' if !mode.is_single() && src.get(i) == Some(&b'\n') => {
                        while i < body_hi && matches!(src[i], b' ' | b'\n' | b'\t' | b'\r') {
                            i += 1;
                        }
                    }
                    _ => {
                        let ch = char_at(src, i - 1).unwrap_or('?');
                        let shown = match ch {
                            '\n' => "\\n".to_string(),
                            '\r' => "\\r".to_string(),
                            '\t' => "\\t".to_string(),
                            c => c.to_string(),
                        };
                        let what = if mode.is_byte() { "unknown byte escape" } else { "unknown character escape" };
                        return err(start, format!("{what}: `{shown}`"));
                    }
                }
            }
            b'\r' => {
                if src.get(i + 1) == Some(&b'\n') && !mode.is_single() {
                    i += 1;
                } else if mode.is_single() {
                    return err(i, "character constant must be escaped: `\\r`");
                } else {
                    return err(i, "bare CR not allowed in string, use `\\r` instead");
                }
            }
            b'\n' | b'\t' if mode.is_single() => {
                return err(i, format!("{} constant must be escaped: `{}`", if mode == Mode::Byte { "byte" } else { "character" }, if c == b'\n' { "\\n" } else { "\\t" }));
            }
            0 if mode == Mode::CStr => return err(i, "null characters in C string literals are not supported"),
            c if c >= 0x80 => {
                let ch = char_at(src, i).unwrap_or('\u{fffd}');
                if mode.is_byte() {
                    return err(i, format!("non-ASCII character in {}", if mode == Mode::Byte { "byte literal" } else { "byte string literal" }));
                }
                i += ch.len_utf8();
            }
            _ => i += 1,
        }
        units += 1;
        if mode.is_single() && units == 1 && i < body_hi {
            return err(lo, "character literal may only contain one codepoint");
        }
    }
    Ok(())
}
