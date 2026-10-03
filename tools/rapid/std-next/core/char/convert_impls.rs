//! Conversion trait impls between char and integers (Rapid only: in the rustc shim these
//! are real core's). Integer<->integer conversions are generated in num/.

use super::{try_from_u32, CharTryFromError, TryFromCharError};
use crate::convert::{From, TryFrom};
use crate::result::Result::{self, Err, Ok};

impl From<u8> for char {
    fn from(i: u8) -> char {
        i as char
    }
}

impl From<char> for u32 {
    fn from(c: char) -> u32 {
        c as u32
    }
}

impl From<char> for u64 {
    fn from(c: char) -> u64 {
        c as u64
    }
}

impl From<char> for u128 {
    fn from(c: char) -> u128 {
        c as u128
    }
}

impl TryFrom<u32> for char {
    type Error = CharTryFromError;
    fn try_from(i: u32) -> Result<char, CharTryFromError> {
        try_from_u32(i)
    }
}

impl TryFrom<char> for u8 {
    type Error = TryFromCharError;
    fn try_from(c: char) -> Result<u8, TryFromCharError> {
        let v = c as u32;
        if v <= 0xff {
            Ok(v as u8)
        } else {
            Err(TryFromCharError(()))
        }
    }
}

impl TryFrom<char> for u16 {
    type Error = TryFromCharError;
    fn try_from(c: char) -> Result<u16, TryFromCharError> {
        let v = c as u32;
        if v <= 0xffff {
            Ok(v as u16)
        } else {
            Err(TryFromCharError(()))
        }
    }
}
