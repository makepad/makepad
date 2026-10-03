//! core::ffi: the C scalar type aliases, c_void and CStr.
//! CStr is an unsized newtype over [c_char] like real core (needs the frontend's
//! transparent-DST-newtype support, coord.md F-OS3).

use crate::fmt;
use crate::fmt::Write as _;

mod c_string;
pub use self::c_string::{CString, IntoStringError, NulError};

pub type c_schar = i8;
pub type c_uchar = u8;
pub type c_short = i16;
pub type c_ushort = u16;
pub type c_int = i32;
pub type c_uint = u32;
pub type c_long = i64;
pub type c_ulong = u64;
pub type c_longlong = i64;
pub type c_ulonglong = u64;
pub type c_float = f32;
pub type c_double = f64;
pub type c_size_t = usize;
pub type c_ssize_t = isize;
pub type c_ptrdiff_t = isize;

/// `char` is unsigned on Linux/Android aarch64, signed on Apple aarch64 and on x86_64.
#[cfg(all(target_arch = "aarch64", not(target_os = "macos"), not(target_os = "ios"), not(target_os = "windows")))]
pub type c_char = u8;
#[cfg(not(all(target_arch = "aarch64", not(target_os = "macos"), not(target_os = "ios"), not(target_os = "windows"))))]
pub type c_char = i8;

#[repr(u8)]
pub enum c_void {
    __variant1,
    __variant2,
}

impl fmt::Debug for c_void {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("c_void")
    }
}

extern "C" {
    fn strlen(s: *const c_char) -> usize;
}

/// A borrowed, nul-terminated C string. `inner` includes the trailing nul.
#[repr(transparent)]
pub struct CStr {
    inner: [c_char],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FromBytesWithNulError {
    InteriorNul { position: usize },
    NotNulTerminated,
}

impl fmt::Display for FromBytesWithNulError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FromBytesWithNulError::InteriorNul { position } => {
                write!(f, "data provided contains an interior nul byte at byte position {}", position)
            }
            FromBytesWithNulError::NotNulTerminated => f.write_str("data provided is not nul terminated"),
        }
    }
}

impl crate::error::Error for FromBytesWithNulError {}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FromBytesUntilNulError(());

impl fmt::Display for FromBytesUntilNulError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("data provided does not contain a nul")
    }
}

impl crate::error::Error for FromBytesUntilNulError {}

fn memchr0(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

impl CStr {
    /// # Safety: `ptr` points to a nul-terminated string valid for 'a.
    pub unsafe fn from_ptr<'a>(ptr: *const c_char) -> &'a CStr {
        let len = strlen(ptr);
        let bytes = crate::slice::from_raw_parts(ptr as *const u8, len + 1);
        CStr::from_bytes_with_nul_unchecked(bytes)
    }
    pub fn from_bytes_with_nul(bytes: &[u8]) -> Result<&CStr, FromBytesWithNulError> {
        match memchr0(bytes) {
            Some(nul) if nul + 1 == bytes.len() => Ok(unsafe { CStr::from_bytes_with_nul_unchecked(bytes) }),
            Some(position) => Err(FromBytesWithNulError::InteriorNul { position }),
            None => Err(FromBytesWithNulError::NotNulTerminated),
        }
    }
    pub fn from_bytes_until_nul(bytes: &[u8]) -> Result<&CStr, FromBytesUntilNulError> {
        match memchr0(bytes) {
            Some(nul) => Ok(unsafe { CStr::from_bytes_with_nul_unchecked(&bytes[..nul + 1]) }),
            None => Err(FromBytesUntilNulError(())),
        }
    }
    /// # Safety: `bytes` ends with its only nul.
    pub const unsafe fn from_bytes_with_nul_unchecked(bytes: &[u8]) -> &CStr {
        &*(bytes as *const [u8] as *const CStr)
    }
    pub const fn as_ptr(&self) -> *const c_char {
        self.inner.as_ptr()
    }
    pub fn count_bytes(&self) -> usize {
        self.inner.len() - 1
    }
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 1
    }
    pub fn to_bytes(&self) -> &[u8] {
        let b = self.to_bytes_with_nul();
        &b[..b.len() - 1]
    }
    pub fn to_bytes_with_nul(&self) -> &[u8] {
        unsafe { &*(&self.inner as *const [c_char] as *const [u8]) }
    }
    pub fn to_str(&self) -> Result<&str, crate::str::Utf8Error> {
        crate::str::from_utf8(self.to_bytes())
    }
    pub fn to_string_lossy(&self) -> crate::borrow::Cow<'_, str> {
        crate::string::String::from_utf8_lossy(self.to_bytes())
    }
}

impl PartialEq for CStr {
    fn eq(&self, other: &CStr) -> bool {
        self.to_bytes() == other.to_bytes()
    }
}
impl Eq for CStr {}
impl PartialOrd for CStr {
    fn partial_cmp(&self, other: &CStr) -> Option<crate::cmp::Ordering> {
        Some(self.to_bytes().cmp(other.to_bytes()))
    }
}
impl Ord for CStr {
    fn cmp(&self, other: &CStr) -> crate::cmp::Ordering {
        self.to_bytes().cmp(other.to_bytes())
    }
}
impl crate::hash::Hash for CStr {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) {
        self.to_bytes_with_nul().hash(state);
    }
}
impl AsRef<CStr> for CStr {
    fn as_ref(&self) -> &CStr {
        self
    }
}

impl fmt::Debug for CStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("\"")?;
        debug_bytes(self.to_bytes(), f)?;
        f.write_str("\"")
    }
}

/// Real core's ByteStr Debug body: valid UTF-8 chars escaped as `char::escape_debug`
/// (ASCII via `u8::escape_ascii`, NUL as `\0`), invalid bytes as `\xNN`.
pub(crate) fn debug_bytes(bytes: &[u8], f: &mut fmt::Formatter) -> fmt::Result {
    let mut rest = bytes;
    while !rest.is_empty() {
        let (valid, bad) = match crate::str::from_utf8(rest) {
            Ok(_) => (rest.len(), 0),
            Err(e) => {
                let v = e.valid_up_to();
                let n = match e.error_len() {
                    Some(n) => n,
                    None => rest.len() - v,
                };
                (v, n)
            }
        };
        let s = unsafe { crate::str::from_utf8_unchecked(&rest[..valid]) };
        for c in s.chars() {
            if c == '\0' {
                f.write_str("\\0")?;
            } else if (c as u32) < 0x80 {
                escape_ascii_byte(c as u8, f)?;
            } else {
                write!(f, "{}", c.escape_debug())?;
            }
        }
        let mut i = valid;
        while i < valid + bad {
            escape_ascii_byte(rest[i], f)?;
            i += 1;
        }
        rest = &rest[valid + bad..];
    }
    Ok(())
}

fn escape_ascii_byte(b: u8, f: &mut fmt::Formatter) -> fmt::Result {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    match b {
        b'\t' => f.write_str("\\t"),
        b'\r' => f.write_str("\\r"),
        b'\n' => f.write_str("\\n"),
        b'\\' => f.write_str("\\\\"),
        b'\'' => f.write_str("\\'"),
        b'"' => f.write_str("\\\""),
        0x20..=0x7e => f.write_char(b as char),
        _ => {
            f.write_str("\\x")?;
            f.write_char(HEX[(b >> 4) as usize] as char)?;
            f.write_char(HEX[(b & 15) as usize] as char)
        }
    }
}
