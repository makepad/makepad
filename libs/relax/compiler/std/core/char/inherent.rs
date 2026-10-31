//! `impl char` (Relax only: real core owns char's inherent methods). Each method forwards
//! to the free fns in char/mod.rs, which the rustc shim tests against real std.

use super::*;

impl char {
    pub const MIN: char = '\0';
    pub const MAX: char = '\u{10FFFF}';
    pub const REPLACEMENT_CHARACTER: char = '\u{FFFD}';
    pub const UNICODE_VERSION: (u8, u8, u8) = crate::unicode::UNICODE_VERSION;

    pub const fn from_u32(i: u32) -> Option<char> {
        from_u32(i)
    }
    pub const unsafe fn from_u32_unchecked(i: u32) -> char {
        from_u32_unchecked(i)
    }
    pub const fn from_digit(num: u32, radix: u32) -> Option<char> {
        from_digit(num, radix)
    }
    pub fn decode_utf16<I: IntoIterator<Item = u16>>(iter: I) -> DecodeUtf16<I::IntoIter> {
        decode_utf16(iter)
    }

    pub const fn is_digit(self, radix: u32) -> bool {
        is_digit(self, radix)
    }
    pub const fn to_digit(self, radix: u32) -> Option<u32> {
        to_digit(self, radix)
    }
    pub const fn len_utf8(self) -> usize {
        len_utf8(self as u32)
    }
    pub const fn len_utf16(self) -> usize {
        len_utf16(self as u32)
    }
    #[track_caller]
    pub const fn encode_utf8(self, dst: &mut [u8]) -> &mut str {
        encode_utf8(self, dst)
    }
    #[track_caller]
    pub const fn encode_utf16(self, dst: &mut [u16]) -> &mut [u16] {
        encode_utf16(self, dst)
    }

    pub fn is_alphabetic(self) -> bool {
        is_alphabetic(self)
    }
    pub const fn is_lowercase(self) -> bool {
        is_lowercase(self)
    }
    pub const fn is_uppercase(self) -> bool {
        is_uppercase(self)
    }
    pub const fn is_whitespace(self) -> bool {
        is_whitespace(self)
    }
    pub fn is_alphanumeric(self) -> bool {
        is_alphanumeric(self)
    }
    pub const fn is_control(self) -> bool {
        is_control(self)
    }
    pub fn is_numeric(self) -> bool {
        is_numeric(self)
    }

    pub fn to_lowercase(self) -> ToLowercase {
        to_lowercase(self)
    }
    pub fn to_uppercase(self) -> ToUppercase {
        to_uppercase(self)
    }

    pub fn escape_unicode(self) -> EscapeUnicode {
        escape_unicode(self)
    }
    pub fn escape_debug(self) -> EscapeDebug {
        escape_debug(self)
    }
    pub fn escape_default(self) -> EscapeDefault {
        escape_default(self)
    }

    pub const fn is_ascii(&self) -> bool {
        (*self as u32) < 0x80
    }
    pub const fn as_ascii(&self) -> Option<u8> {
        if self.is_ascii() {
            Some(*self as u8)
        } else {
            None
        }
    }
    pub const fn to_ascii_uppercase(&self) -> char {
        to_ascii_uppercase(*self)
    }
    pub const fn to_ascii_lowercase(&self) -> char {
        to_ascii_lowercase(*self)
    }
    pub const fn eq_ignore_ascii_case(&self, other: &char) -> bool {
        eq_ignore_ascii_case(*self, *other)
    }
    pub const fn make_ascii_uppercase(&mut self) {
        *self = to_ascii_uppercase(*self);
    }
    pub const fn make_ascii_lowercase(&mut self) {
        *self = to_ascii_lowercase(*self);
    }
    pub const fn is_ascii_alphabetic(&self) -> bool {
        is_ascii_alphabetic(*self)
    }
    pub const fn is_ascii_uppercase(&self) -> bool {
        is_ascii_uppercase(*self)
    }
    pub const fn is_ascii_lowercase(&self) -> bool {
        is_ascii_lowercase(*self)
    }
    pub const fn is_ascii_alphanumeric(&self) -> bool {
        is_ascii_alphanumeric(*self)
    }
    pub const fn is_ascii_digit(&self) -> bool {
        is_ascii_digit(*self)
    }
    pub const fn is_ascii_octdigit(&self) -> bool {
        is_ascii_octdigit(*self)
    }
    pub const fn is_ascii_hexdigit(&self) -> bool {
        is_ascii_hexdigit(*self)
    }
    pub const fn is_ascii_punctuation(&self) -> bool {
        is_ascii_punctuation(*self)
    }
    pub const fn is_ascii_graphic(&self) -> bool {
        is_ascii_graphic(*self)
    }
    pub const fn is_ascii_whitespace(&self) -> bool {
        is_ascii_whitespace(*self)
    }
    pub const fn is_ascii_control(&self) -> bool {
        is_ascii_control(*self)
    }
}
