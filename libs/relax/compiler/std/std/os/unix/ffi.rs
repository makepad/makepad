//! OsStrExt / OsStringExt: unix OS strings are bytes.

use alloc::vec::Vec;

use crate::ffi::{OsStr, OsString};

pub trait OsStrExt {
    fn from_bytes(slice: &[u8]) -> &Self;
    fn as_bytes(&self) -> &[u8];
}

impl OsStrExt for OsStr {
    fn from_bytes(slice: &[u8]) -> &OsStr {
        OsStr::from_raw_bytes(slice)
    }
    fn as_bytes(&self) -> &[u8] {
        self.bytes()
    }
}

pub trait OsStringExt {
    fn from_vec(vec: Vec<u8>) -> Self;
    fn into_vec(self) -> Vec<u8>;
}

impl OsStringExt for OsString {
    fn from_vec(vec: Vec<u8>) -> OsString {
        OsString::from_bytes_vec(vec)
    }
    fn into_vec(self) -> Vec<u8> {
        OsString::into_bytes_vec(self)
    }
}
