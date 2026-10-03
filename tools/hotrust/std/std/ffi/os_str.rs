//! OsStr / OsString: arbitrary bytes on unix. OsStr is an unsized newtype over [u8]
//! (coord.md F-OS3).

use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::ops::Deref;

#[repr(transparent)]
pub struct OsStr {
    inner: [u8],
}

#[derive(Clone, Default)]
pub struct OsString {
    inner: Vec<u8>,
}

impl OsStr {
    pub fn new<S: AsRef<OsStr> + ?Sized>(s: &S) -> &OsStr {
        s.as_ref()
    }
    pub(crate) fn from_raw_bytes(b: &[u8]) -> &OsStr {
        unsafe { &*(b as *const [u8] as *const OsStr) }
    }
    pub(crate) fn from_bytes_mut(b: &mut [u8]) -> &mut OsStr {
        unsafe { &mut *(b as *mut [u8] as *mut OsStr) }
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.inner
    }
    pub unsafe fn from_encoded_bytes_unchecked(bytes: &[u8]) -> &OsStr {
        OsStr::from_raw_bytes(bytes)
    }
    pub fn as_encoded_bytes(&self) -> &[u8] {
        &self.inner
    }
    pub fn to_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.inner).ok()
    }
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.inner)
    }
    pub fn to_os_string(&self) -> OsString {
        OsString { inner: self.inner.to_vec() }
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    pub fn into_os_string(self: Box<OsStr>) -> OsString {
        let b: Box<[u8]> = unsafe { Box::from_raw(Box::into_raw(self) as *mut [u8]) };
        OsString { inner: b.into_vec() }
    }
    pub fn to_ascii_lowercase(&self) -> OsString {
        OsString { inner: self.inner.to_ascii_lowercase() }
    }
    pub fn to_ascii_uppercase(&self) -> OsString {
        OsString { inner: self.inner.to_ascii_uppercase() }
    }
    pub fn eq_ignore_ascii_case<S: AsRef<OsStr>>(&self, other: S) -> bool {
        self.inner.eq_ignore_ascii_case(&other.as_ref().inner)
    }
    pub fn display(&self) -> Display<'_> {
        Display { os_str: self }
    }
}

impl OsString {
    pub fn new() -> OsString {
        OsString { inner: Vec::new() }
    }
    pub fn with_capacity(capacity: usize) -> OsString {
        OsString { inner: Vec::with_capacity(capacity) }
    }
    pub(crate) fn from_bytes_vec(v: Vec<u8>) -> OsString {
        OsString { inner: v }
    }
    pub(crate) fn into_bytes_vec(self) -> Vec<u8> {
        self.inner
    }
    pub(crate) fn vec_mut(&mut self) -> &mut Vec<u8> {
        &mut self.inner
    }
    pub unsafe fn from_encoded_bytes_unchecked(bytes: Vec<u8>) -> OsString {
        OsString { inner: bytes }
    }
    pub fn into_encoded_bytes(self) -> Vec<u8> {
        self.inner
    }
    pub fn as_os_str(&self) -> &OsStr {
        OsStr::from_raw_bytes(&self.inner)
    }
    pub fn into_string(self) -> Result<String, OsString> {
        match String::from_utf8(self.inner) {
            Ok(s) => Ok(s),
            Err(e) => Err(OsString { inner: e.into_bytes() }),
        }
    }
    pub fn push<T: AsRef<OsStr>>(&mut self, s: T) {
        self.inner.extend_from_slice(&s.as_ref().inner)
    }
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
    pub fn clear(&mut self) {
        self.inner.clear()
    }
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional)
    }
    pub fn into_boxed_os_str(self) -> Box<OsStr> {
        let b = self.inner.into_boxed_slice();
        unsafe { Box::from_raw(Box::into_raw(b) as *mut OsStr) }
    }
    pub fn display(&self) -> Display<'_> {
        self.as_os_str().display()
    }
}

impl Deref for OsString {
    type Target = OsStr;
    fn deref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl core::ops::DerefMut for OsString {
    fn deref_mut(&mut self) -> &mut OsStr {
        OsStr::from_bytes_mut(&mut self.inner)
    }
}

impl From<String> for OsString {
    fn from(s: String) -> OsString {
        OsString { inner: s.into_bytes() }
    }
}

impl From<&str> for OsString {
    fn from(s: &str) -> OsString {
        OsString { inner: s.as_bytes().to_vec() }
    }
}

impl From<&OsStr> for OsString {
    fn from(s: &OsStr) -> OsString {
        s.to_os_string()
    }
}

impl From<OsString> for Box<OsStr> {
    fn from(s: OsString) -> Box<OsStr> {
        s.into_boxed_os_str()
    }
}

impl Borrow<OsStr> for OsString {
    fn borrow(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl ToOwned for OsStr {
    type Owned = OsString;
    fn to_owned(&self) -> OsString {
        self.to_os_string()
    }
}

impl AsRef<OsStr> for OsStr {
    fn as_ref(&self) -> &OsStr {
        self
    }
}
impl AsRef<OsStr> for OsString {
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}
impl AsRef<OsStr> for str {
    fn as_ref(&self) -> &OsStr {
        OsStr::from_raw_bytes(self.as_bytes())
    }
}
impl AsRef<OsStr> for String {
    fn as_ref(&self) -> &OsStr {
        OsStr::from_raw_bytes(self.as_bytes())
    }
}

impl PartialEq for OsStr {
    fn eq(&self, other: &OsStr) -> bool {
        self.inner == other.inner
    }
}
impl Eq for OsStr {}
impl PartialEq<str> for OsStr {
    fn eq(&self, other: &str) -> bool {
        &self.inner == other.as_bytes()
    }
}
impl PartialEq<OsStr> for str {
    fn eq(&self, other: &OsStr) -> bool {
        self.as_bytes() == &other.inner
    }
}
impl PartialOrd for OsStr {
    fn partial_cmp(&self, other: &OsStr) -> Option<Ordering> {
        Some(self.inner.cmp(&other.inner))
    }
}
impl Ord for OsStr {
    fn cmp(&self, other: &OsStr) -> Ordering {
        self.inner.cmp(&other.inner)
    }
}
impl Hash for OsStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}

impl PartialEq for OsString {
    fn eq(&self, other: &OsString) -> bool {
        self.inner == other.inner
    }
}
impl Eq for OsString {}
impl PartialEq<str> for OsString {
    fn eq(&self, other: &str) -> bool {
        self.inner.as_slice() == other.as_bytes()
    }
}
impl PartialEq<&str> for OsString {
    fn eq(&self, other: &&str) -> bool {
        self.inner.as_slice() == other.as_bytes()
    }
}
impl PartialOrd for OsString {
    fn partial_cmp(&self, other: &OsString) -> Option<Ordering> {
        Some(self.inner.cmp(&other.inner))
    }
}
impl Ord for OsString {
    fn cmp(&self, other: &OsString) -> Ordering {
        self.inner.cmp(&other.inner)
    }
}
impl Hash for OsString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}

/// Real std's unix OsStr Debug (Utf8Chunks debug): chars via escape_debug, invalid bytes
/// as `\xNN` (upper-case hex).
pub(crate) fn debug_os_bytes(bytes: &[u8], f: &mut fmt::Formatter) -> fmt::Result {
    f.write_str("\"")?;
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c == '\'' {
                f.write_str("'")?;
            } else {
                write!(f, "{}", c.escape_debug())?;
            }
        }
        for b in chunk.invalid() {
            write!(f, "\\x{:02X}", b)?;
        }
    }
    f.write_str("\"")
}

impl fmt::Debug for OsStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        debug_os_bytes(&self.inner, f)
    }
}

impl fmt::Debug for OsString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        debug_os_bytes(&self.inner, f)
    }
}

/// `OsStr::display()`: lossy UTF-8 (U+FFFD for each invalid sequence).
pub struct Display<'a> {
    os_str: &'a OsStr,
}

impl<'a> fmt::Display for Display<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let bytes = &self.os_str.inner;
        if let Ok(s) = core::str::from_utf8(bytes) {
            return f.pad(s);
        }
        for chunk in bytes.utf8_chunks() {
            f.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                f.write_str("\u{FFFD}")?;
            }
        }
        Ok(())
    }
}

impl<'a> fmt::Debug for Display<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.os_str, f)
    }
}
