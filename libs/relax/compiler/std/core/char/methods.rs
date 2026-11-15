//! The free functions `char`'s methods and core's `str` are made of (char/mod.rs `mod methods`).
use super::*;

pub(crate) fn to_digit(c: char, radix: u32) -> Option<u32> {
    assert!(radix >= 2 && radix <= 36, "to_digit: invalid radix -- radix must be in the range 2 to 36 inclusive");
    let value = if c > '9' && radix > 10 {
        ((c as u32).wrapping_sub('A' as u32) & !0b0010_0000u32) + 10
    } else {
        (c as u32).wrapping_sub('0' as u32)
    };
    if value < radix {
        Some(value)
    } else {
        None
    }
}

pub(crate) fn is_digit(c: char, radix: u32) -> bool {
    to_digit(c, radix).is_some()
}

pub(crate) const fn len_utf8(code: u32) -> usize {
    if code < 0x80 {
        1
    } else if code < 0x800 {
        2
    } else if code < 0x10000 {
        3
    } else {
        4
    }
}

pub(crate) const fn len_utf16(code: u32) -> usize {
    if (code & 0xFFFF) == code {
        1
    } else {
        2
    }
}

/// `char::encode_utf8`: the encoded bytes as a `&mut str` inside `dst`.
#[track_caller]
pub(crate) fn encode_utf8(c: char, dst: &mut [u8]) -> &mut str {
    let n = encode_utf8_raw(c as u32, dst);
    unsafe { crate::str::from_utf8_unchecked_mut(&mut dst[..n]) }
}

#[track_caller]
pub(crate) fn encode_utf16(c: char, dst: &mut [u16]) -> &mut [u16] {
    let n = encode_utf16_raw(c as u32, dst);
    &mut dst[..n]
}

pub(crate) fn is_alphabetic(c: char) -> bool {
    match c {
        'a'..='z' | 'A'..='Z' => true,
        _ => c > '\x7f' && crate::unicode::is_alphabetic(c),
    }
}

pub(crate) fn is_lowercase(c: char) -> bool {
    match c {
        'a'..='z' => true,
        _ => c > '\x7f' && crate::unicode::is_lowercase(c),
    }
}

pub(crate) fn is_uppercase(c: char) -> bool {
    match c {
        'A'..='Z' => true,
        _ => c > '\x7f' && crate::unicode::is_uppercase(c),
    }
}

pub(crate) fn is_whitespace(c: char) -> bool {
    match c {
        ' ' | '\x09'..='\x0d' => true,
        _ => c > '\x7f' && crate::unicode::is_white_space(c),
    }
}

pub(crate) fn is_numeric(c: char) -> bool {
    match c {
        '0'..='9' => true,
        _ => c > '\x7f' && crate::unicode::is_numeric(c),
    }
}

pub(crate) fn is_alphanumeric(c: char) -> bool {
    is_alphabetic(c) || is_numeric(c)
}

/// General category Cc.
pub(crate) fn is_control(c: char) -> bool {
    let u = c as u32;
    u < 0x20 || (u >= 0x7f && u <= 0x9f)
}

pub(crate) fn is_ascii(c: char) -> bool {
    (c as u32) < 0x80
}

pub(crate) fn is_ascii_alphabetic(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z')
}

pub(crate) fn is_ascii_uppercase(c: char) -> bool {
    matches!(c, 'A'..='Z')
}

pub(crate) fn is_ascii_lowercase(c: char) -> bool {
    matches!(c, 'a'..='z')
}

pub(crate) fn is_ascii_alphanumeric(c: char) -> bool {
    matches!(c, '0'..='9' | 'A'..='Z' | 'a'..='z')
}

pub(crate) fn is_ascii_digit(c: char) -> bool {
    matches!(c, '0'..='9')
}

pub(crate) fn is_ascii_octdigit(c: char) -> bool {
    matches!(c, '0'..='7')
}

pub(crate) fn is_ascii_hexdigit(c: char) -> bool {
    matches!(c, '0'..='9' | 'A'..='F' | 'a'..='f')
}

pub(crate) fn is_ascii_punctuation(c: char) -> bool {
    matches!(c, '!'..='/' | ':'..='@' | '['..='`' | '{'..='~')
}

pub(crate) fn is_ascii_graphic(c: char) -> bool {
    matches!(c, '!'..='~')
}

pub(crate) fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

pub(crate) fn is_ascii_control(c: char) -> bool {
    matches!(c, '\0'..='\x1F' | '\x7F')
}

pub(crate) fn to_ascii_uppercase(c: char) -> char {
    if is_ascii_lowercase(c) {
        ((c as u8) - 32) as char
    } else {
        c
    }
}

pub(crate) fn to_ascii_lowercase(c: char) -> char {
    if is_ascii_uppercase(c) {
        ((c as u8) + 32) as char
    } else {
        c
    }
}

pub(crate) fn eq_ignore_ascii_case(a: char, b: char) -> bool {
    to_ascii_lowercase(a) == to_ascii_lowercase(b)
}

pub(crate) fn to_lowercase(c: char) -> ToLowercase {
    ToLowercase(CaseMappingIter::new(crate::unicode::to_lower(c)))
}

pub(crate) fn to_uppercase(c: char) -> ToUppercase {
    ToUppercase(CaseMappingIter::new(crate::unicode::to_upper(c)))
}

pub(crate) fn escape_debug(c: char) -> EscapeDebug {
    EscapeDebug(escape_debug_ext(c, ESCAPE_ALL))
}

pub(crate) fn escape_default(c: char) -> EscapeDefault {
    EscapeDefault(escape_default_iter(c))
}

pub(crate) const fn escape_unicode(c: char) -> EscapeUnicode {
    EscapeUnicode(EscapeIter::unicode(c))
}

/// `str::parse::<char>`
pub(crate) fn parse_char(s: &str) -> Result<char, ParseCharError> {
    let mut chars = crate::str::chars(s);
    match (chars.next(), chars.next()) {
        (None, _) => Err(ParseCharError { kind: CharErrorKind::EmptyString }),
        (Some(c), None) => Ok(c),
        _ => Err(ParseCharError { kind: CharErrorKind::TooManyChars }),
    }
}

/// `char::try_from(u32)`
pub(crate) fn try_from_u32(i: u32) -> Result<char, CharTryFromError> {
    match from_u32(i) {
        Some(c) => Ok(c),
        None => Err(CharTryFromError(())),
    }
}
