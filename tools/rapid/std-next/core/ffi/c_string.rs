//! CString (an owned, nul-terminated C string). Physically in core like every alloc-level
//! type of Rapid's std; real paths `alloc::ffi::CString` / `std::ffi::CString`.

use crate::borrow::Borrow;
use crate::boxed::Box;
use crate::fmt;
use crate::ops::Deref;
use crate::string::String;
use crate::vec::Vec;

use super::{c_char, CStr};

/// `inner` holds the bytes plus the trailing nul, no interior nul.
#[derive(PartialEq, PartialOrd, Eq, Ord, Hash, Clone)]
pub struct CString {
    inner: Box<[u8]>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NulError(usize, Vec<u8>);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IntoStringError {
    inner: CString,
    error: crate::str::Utf8Error,
}

impl CString {
    pub fn new<T: Into<Vec<u8>>>(t: T) -> Result<CString, NulError> {
        let bytes: Vec<u8> = t.into();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0 {
                return Err(NulError(i, bytes));
            }
            i += 1;
        }
        Ok(unsafe { CString::from_vec_unchecked(bytes) })
    }

    /// # Safety: `v` has no nul byte.
    pub unsafe fn from_vec_unchecked(mut v: Vec<u8>) -> CString {
        v.reserve_exact(1);
        v.push(0);
        CString { inner: v.into_boxed_slice() }
    }

    /// # Safety: `ptr` came from `CString::into_raw`.
    pub unsafe fn from_raw(ptr: *mut c_char) -> CString {
        let len = super::strlen(ptr) + 1;
        let slice = crate::slice::from_raw_parts_mut(ptr as *mut u8, len);
        CString { inner: Box::from_raw(slice as *mut [u8]) }
    }

    pub fn into_raw(self) -> *mut c_char {
        Box::into_raw(self.into_inner()) as *mut u8 as *mut c_char
    }

    pub fn into_string(self) -> Result<String, IntoStringError> {
        match String::from_utf8(self.into_bytes()) {
            Ok(s) => Ok(s),
            Err(e) => {
                let error = e.utf8_error();
                Err(IntoStringError { inner: unsafe { CString::from_vec_unchecked(e.into_bytes()) }, error })
            }
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        let mut vec = self.into_inner().into_vec();
        vec.pop();
        vec
    }

    pub fn into_bytes_with_nul(self) -> Vec<u8> {
        self.into_inner().into_vec()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.inner[..self.inner.len() - 1]
    }

    pub fn as_bytes_with_nul(&self) -> &[u8] {
        &self.inner
    }

    pub fn as_c_str(&self) -> &CStr {
        unsafe { CStr::from_bytes_with_nul_unchecked(&self.inner) }
    }

    pub fn into_boxed_c_str(self) -> Box<CStr> {
        unsafe { Box::from_raw(Box::into_raw(self.into_inner()) as *mut CStr) }
    }

    fn into_inner(self) -> Box<[u8]> {
        let this = crate::mem::ManuallyDrop::new(self);
        unsafe { crate::ptr::read(&this.inner) }
    }
}

impl Drop for CString {
    fn drop(&mut self) {
        // real std zeroes the first byte so use-after-free of as_ptr reads ""
        if let Some(c) = self.inner.first_mut() {
            *c = 0;
        }
    }
}

impl Deref for CString {
    type Target = CStr;
    fn deref(&self) -> &CStr {
        self.as_c_str()
    }
}

impl fmt::Debug for CString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl From<CString> for Vec<u8> {
    fn from(s: CString) -> Vec<u8> {
        s.into_bytes()
    }
}

impl Default for CString {
    fn default() -> CString {
        unsafe { CString::from_vec_unchecked(Vec::new()) }
    }
}

impl Borrow<CStr> for CString {
    fn borrow(&self) -> &CStr {
        self
    }
}

impl AsRef<CStr> for CString {
    fn as_ref(&self) -> &CStr {
        self
    }
}

impl From<&CStr> for CString {
    fn from(s: &CStr) -> CString {
        s.to_owned()
    }
}

impl crate::borrow::ToOwned for CStr {
    type Owned = CString;
    fn to_owned(&self) -> CString {
        CString { inner: self.to_bytes_with_nul().into() }
    }
}

impl NulError {
    pub fn nul_position(&self) -> usize {
        self.0
    }
    pub fn into_vec(self) -> Vec<u8> {
        self.1
    }
}

impl fmt::Display for NulError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "nul byte found in provided data at position: {}", self.0)
    }
}

impl crate::error::Error for NulError {}

impl IntoStringError {
    pub fn into_cstring(self) -> CString {
        self.inner
    }
    pub fn utf8_error(&self) -> crate::str::Utf8Error {
        self.error
    }
}

impl fmt::Display for IntoStringError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("C string contained non-utf8 bytes")
    }
}

impl crate::error::Error for IntoStringError {}
