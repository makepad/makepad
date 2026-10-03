//! String: an owned, growable UTF-8 string over `Vec<u8>`, and the str methods that return
//! owned strings (to_lowercase, replace, repeat, ...), with real alloc's behaviour and
//! panic messages.

use crate::fmt;
use crate::iter::{DoubleEndedIterator, Extend, FromIterator, FusedIterator, IntoIterator, Iterator};
use crate::ops::{self, Add, AddAssign, Bound, Deref, DerefMut, RangeBounds};
use crate::option::Option::{self, None, Some};
use crate::result::Result::{self, Err, Ok};
use crate::str::pattern::{Pattern, StrSearcher};
use crate::str::{Chars, StrIndex, Utf8Error};

pub struct String {
    vec: Vec<u8>,
}

// ---------------------------------------------------------------- errors

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FromUtf8Error {
    bytes: Vec<u8>,
    error: Utf8Error,
}

impl FromUtf8Error {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..]
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
    pub fn utf8_error(&self) -> Utf8Error {
        self.error
    }
    /// The bytes with invalid sequences replaced by U+FFFD.
    pub fn into_utf8_lossy(self) -> String {
        lossy_string(&self.bytes)
    }
}

impl fmt::Display for FromUtf8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

#[derive(Debug)]
pub struct FromUtf16Error(());

impl fmt::Display for FromUtf16Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("invalid utf-16: lone surrogate found", f)
    }
}

#[cfg(not(rapid_check))]
impl crate::error::Error for FromUtf8Error {}
#[cfg(not(rapid_check))]
impl crate::error::Error for FromUtf16Error {}

/// `bytes` with each invalid sequence replaced by U+FFFD (String::from_utf8_lossy's text).
pub fn lossy_string(bytes: &[u8]) -> String {
    let mut res = String::with_capacity(bytes.len());
    let mut it = crate::str::utf8_chunks(bytes);
    while let Some(chunk) = it.next() {
        res.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            res.push_str("\u{FFFD}");
        }
    }
    res
}

/// Resolves a range against `len` like real core's `slice::range` (same panics).
#[track_caller]
fn slice_range<R: RangeBounds<usize>>(range: &R, len: usize) -> (usize, usize) {
    let end = match range.end_bound() {
        Bound::Included(&end) if end >= len => slice_index_fail(0, end, len),
        Bound::Included(&end) => end + 1,
        Bound::Excluded(&end) if end > len => slice_index_fail(0, end, len),
        Bound::Excluded(&end) => end,
        Bound::Unbounded => len,
    };
    let start = match range.start_bound() {
        Bound::Excluded(&start) if start >= end => slice_index_fail(start, end, len),
        Bound::Excluded(&start) => start + 1,
        Bound::Included(&start) if start > end => slice_index_fail(start, end, len),
        Bound::Included(&start) => start,
        Bound::Unbounded => 0,
    };
    (start, end)
}

#[track_caller]
#[cold]
fn slice_index_fail(start: usize, end: usize, len: usize) -> ! {
    if start > len {
        panic!("range start index {} out of range for slice of length {}", start, len);
    }
    if end > len {
        panic!("range end index {} out of range for slice of length {}", end, len);
    }
    if start > end {
        panic!("slice index starts at {} but ends at {}", start, end);
    }
    panic!("range end index {} out of range for slice of length {}", end, len);
}

// ---------------------------------------------------------------- String

impl String {
    pub const fn new() -> String {
        String { vec: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> String {
        String { vec: Vec::with_capacity(capacity) }
    }

    pub fn from_utf8(vec: Vec<u8>) -> Result<String, FromUtf8Error> {
        match crate::str::from_utf8(&vec[..]) {
            Ok(..) => Ok(String { vec }),
            Err(e) => Err(FromUtf8Error { bytes: vec, error: e }),
        }
    }

    pub unsafe fn from_utf8_unchecked(bytes: Vec<u8>) -> String {
        String { vec: bytes }
    }

    pub fn from_utf16(v: &[u16]) -> Result<String, FromUtf16Error> {
        let mut ret = String::with_capacity(v.len());
        let mut it = crate::char::decode_utf16(v.iter().cloned());
        while let Some(c) = it.next() {
            match c {
                Ok(c) => ret.push(c),
                Err(_) => return Err(FromUtf16Error(())),
            }
        }
        Ok(ret)
    }

    pub fn from_utf16_lossy(v: &[u16]) -> String {
        let mut ret = String::with_capacity(v.len());
        let mut it = crate::char::decode_utf16(v.iter().cloned());
        while let Some(c) = it.next() {
            match c {
                Ok(c) => ret.push(c),
                Err(_) => ret.push('\u{FFFD}'),
            }
        }
        ret
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.vec
    }

    pub fn as_str(&self) -> &str {
        unsafe { crate::str::from_utf8_unchecked(&self.vec[..]) }
    }

    pub fn as_mut_str(&mut self) -> &mut str {
        unsafe { crate::str::from_utf8_unchecked_mut(&mut self.vec[..]) }
    }

    pub fn push_str(&mut self, string: &str) {
        self.vec.extend_from_slice(string.as_bytes())
    }

    #[track_caller]
    pub fn extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) {
        let (start, end) = slice_range(&src, self.len());
        assert!(self.is_char_boundary(start));
        assert!(self.is_char_boundary(end));
        let n = end - start;
        self.vec.reserve(n);
        let len = self.vec.len();
        unsafe {
            let p = self.vec.as_mut_ptr();
            crate::intrinsics_mem::copy_nonoverlapping(
                crate::intrinsics_mem::ptr_add(p as *const u8, start),
                crate::intrinsics_mem::ptr_add_mut(p, len),
                n,
            );
            self.vec.set_len(len + n);
        }
    }

    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    pub fn reserve(&mut self, additional: usize) {
        self.vec.reserve(additional)
    }

    pub fn reserve_exact(&mut self, additional: usize) {
        self.vec.reserve_exact(additional)
    }

    pub fn shrink_to_fit(&mut self) {
        self.vec.shrink_to_fit()
    }

    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.vec.shrink_to(min_capacity)
    }

    pub fn push(&mut self, ch: char) {
        let code = ch as u32;
        if code < 0x80 {
            self.vec.push(code as u8);
        } else {
            let mut buf = [0u8; 4];
            let n = crate::char::encode_utf8_raw(code, &mut buf);
            self.vec.extend_from_slice(&buf[..n]);
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.vec[..]
    }

    #[track_caller]
    pub fn truncate(&mut self, new_len: usize) {
        if new_len <= self.len() {
            assert!(self.is_char_boundary(new_len));
            self.vec.truncate(new_len)
        }
    }

    pub fn pop(&mut self) -> Option<char> {
        let ch = self.as_str().chars().next_back()?;
        let newlen = self.len() - ch.len_utf8();
        unsafe {
            self.vec.set_len(newlen);
        }
        Some(ch)
    }

    #[track_caller]
    pub fn remove(&mut self, idx: usize) -> char {
        let ch = match crate::str::index(self.as_str(), idx..).chars().next() {
            Some(ch) => ch,
            None => panic!("cannot remove a char from the end of a string"),
        };
        let next = idx + ch.len_utf8();
        let len = self.len();
        unsafe {
            let p = self.vec.as_mut_ptr();
            crate::intrinsics_mem::copy(
                crate::intrinsics_mem::ptr_add(p as *const u8, next),
                crate::intrinsics_mem::ptr_add_mut(p, idx),
                len - next,
            );
            self.vec.set_len(len - (next - idx));
        }
        ch
    }

    pub fn retain<F: FnMut(char) -> bool>(&mut self, mut f: F) {
        let len = self.len();
        let mut idx = 0;
        let mut del_bytes = 0;
        while idx < len {
            let ch = unsafe { crate::str::sub(self.as_str(), idx, len) }.chars().next().unwrap();
            let ch_len = ch.len_utf8();
            if !f(ch) {
                del_bytes += ch_len;
            } else if del_bytes > 0 {
                unsafe {
                    let p = self.vec.as_mut_ptr();
                    crate::intrinsics_mem::copy(
                        crate::intrinsics_mem::ptr_add(p as *const u8, idx),
                        crate::intrinsics_mem::ptr_add_mut(p, idx - del_bytes),
                        ch_len,
                    );
                }
            }
            idx += ch_len;
        }
        unsafe {
            self.vec.set_len(len - del_bytes);
        }
    }

    #[track_caller]
    pub fn insert(&mut self, idx: usize, ch: char) {
        assert!(self.is_char_boundary(idx));
        let mut bits = [0u8; 4];
        let n = crate::char::encode_utf8_raw(ch as u32, &mut bits);
        unsafe {
            self.insert_bytes(idx, &bits[..n]);
        }
    }

    unsafe fn insert_bytes(&mut self, idx: usize, bytes: &[u8]) {
        let len = self.len();
        let amt = bytes.len();
        self.vec.reserve(amt);
        let p = self.vec.as_mut_ptr();
        crate::intrinsics_mem::copy(
            crate::intrinsics_mem::ptr_add(p as *const u8, idx),
            crate::intrinsics_mem::ptr_add_mut(p, idx + amt),
            len - idx,
        );
        crate::intrinsics_mem::copy_nonoverlapping(
            crate::intrinsics_mem::slice_ptr(bytes),
            crate::intrinsics_mem::ptr_add_mut(p, idx),
            amt,
        );
        self.vec.set_len(len + amt);
    }

    #[track_caller]
    pub fn insert_str(&mut self, idx: usize, string: &str) {
        assert!(self.is_char_boundary(idx));
        unsafe {
            self.insert_bytes(idx, string.as_bytes());
        }
    }

    pub unsafe fn as_mut_vec(&mut self) -> &mut Vec<u8> {
        &mut self.vec
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[track_caller]
    pub fn split_off(&mut self, at: usize) -> String {
        assert!(self.is_char_boundary(at));
        let other = Vec::from(&self.vec[at..]);
        self.vec.truncate(at);
        String { vec: other }
    }

    pub fn clear(&mut self) {
        self.vec.clear()
    }

    #[track_caller]
    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_> {
        let (start, end) = slice_range(&range, self.len());
        assert!(self.is_char_boundary(start));
        assert!(self.is_char_boundary(end));
        let self_ptr = self as *mut String;
        let chars_iter = crate::str::chars(unsafe { crate::str::sub(&*self_ptr, start, end) });
        Drain { start, end, iter: chars_iter, string: self_ptr }
    }

    #[track_caller]
    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &str) {
        let (start, end) = slice_range(&range, self.len());
        assert!(self.is_char_boundary(start), "start of range should be a character boundary");
        assert!(self.is_char_boundary(end), "end of range should be a character boundary");
        let tail = Vec::from(&self.vec[end..]);
        self.vec.truncate(start);
        self.vec.extend_from_slice(replace_with.as_bytes());
        self.vec.extend_from_slice(&tail[..]);
    }

    pub fn leak<'a>(self) -> &'a mut str {
        let mut me = crate::mem::ManuallyDrop::new(self);
        let len = me.vec.len();
        unsafe { crate::str::from_utf8_unchecked_mut(crate::intrinsics_mem::slice_from_raw_mut(me.vec.as_mut_ptr(), len)) }
    }

    fn is_char_boundary(&self, idx: usize) -> bool {
        crate::str::is_char_boundary(self.as_str(), idx)
    }
}

/// `String::from_utf8_lossy` (borrows when the bytes are valid).
#[cfg(not(rapid_check))]
impl String {
    pub fn from_utf8_lossy(v: &[u8]) -> crate::borrow::Cow<'_, str> {
        let mut iter = crate::str::utf8_chunks(v);
        let chunk = match iter.next() {
            Some(c) => c,
            None => return crate::borrow::Cow::Borrowed(""),
        };
        if chunk.invalid().is_empty() {
            return crate::borrow::Cow::Borrowed(chunk.valid());
        }
        crate::borrow::Cow::Owned(lossy_string(v))
    }
}

// ---------------------------------------------------------------- Drain

pub struct Drain<'a> {
    string: *mut String,
    start: usize,
    end: usize,
    iter: Chars<'a>,
}

impl<'a> Drain<'a> {
    pub fn as_str(&self) -> &str {
        self.iter.as_str()
    }
}

impl<'a> Drop for Drain<'a> {
    fn drop(&mut self) {
        unsafe {
            let vec = &mut (*self.string).vec;
            let len = vec.len();
            if self.start <= self.end && self.end <= len {
                let p = vec.as_mut_ptr();
                crate::intrinsics_mem::copy(
                    crate::intrinsics_mem::ptr_add(p as *const u8, self.end),
                    crate::intrinsics_mem::ptr_add_mut(p, self.start),
                    len - self.end,
                );
                vec.set_len(len - (self.end - self.start));
            }
        }
    }
}

impl<'a> Iterator for Drain<'a> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
    fn last(self) -> Option<char> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a> DoubleEndedIterator for Drain<'a> {
    fn next_back(&mut self) -> Option<char> {
        self.iter.next_back()
    }
}

impl<'a> FusedIterator for Drain<'a> {}

impl<'a> fmt::Debug for Drain<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Drain").field(&self.as_str()).finish()
    }
}

// ---------------------------------------------------------------- str -> String

/// `str::to_lowercase`, including the final-sigma rule.
pub fn str_to_lowercase(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = crate::str::char_indices(s);
    while let Some((i, c)) = it.next() {
        if c == 'Σ' {
            out.push(map_uppercase_sigma(s, i));
        } else {
            push_mapping(&mut out, crate::unicode::to_lower(c));
        }
    }
    out
}

fn push_mapping(out: &mut String, m: [char; 3]) {
    out.push(m[0]);
    if m[1] != '\0' {
        out.push(m[1]);
        if m[2] != '\0' {
            out.push(m[2]);
        }
    }
}

fn map_uppercase_sigma(from: &str, i: usize) -> char {
    let before = unsafe { crate::str::sub(from, 0, i) };
    let after = unsafe { crate::str::sub(from, i + 2, from.len()) };
    let is_word_final = case_ignorable_then_cased(before.chars().rev()) && !case_ignorable_then_cased(after.chars());
    if is_word_final {
        'ς'
    } else {
        'σ'
    }
}

fn case_ignorable_then_cased<I: Iterator<Item = char>>(iter: I) -> bool {
    let mut iter = iter;
    loop {
        match iter.next() {
            Some(c) if crate::unicode::is_case_ignorable(c) => continue,
            Some(c) => return crate::unicode::is_cased(c),
            None => return false,
        }
    }
}

pub fn str_to_uppercase(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = crate::str::chars(s);
    while let Some(c) = it.next() {
        push_mapping(&mut out, crate::unicode::to_upper(c));
    }
    out
}

pub fn str_to_ascii_uppercase(s: &str) -> String {
    let mut out = String::from(s);
    crate::str::make_ascii_uppercase(out.as_mut_str());
    out
}

pub fn str_to_ascii_lowercase(s: &str) -> String {
    let mut out = String::from(s);
    crate::str::make_ascii_lowercase(out.as_mut_str());
    out
}

#[track_caller]
pub fn str_repeat(s: &str, n: usize) -> String {
    let total = match s.len().checked_mul(n) {
        Some(t) => t,
        None => panic!("capacity overflow"),
    };
    let mut out = String::with_capacity(total);
    let mut i = 0;
    while i < n {
        out.push_str(s);
        i += 1;
    }
    out
}

pub fn str_replace<'a, P: Pattern<'a>>(s: &'a str, from: P, to: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut last_end = 0;
    let mut it = crate::str::match_indices(s, from);
    while let Some((start, part)) = it.next() {
        result.push_str(unsafe { crate::str::sub(s, last_end, start) });
        result.push_str(to);
        last_end = start + part.len();
    }
    result.push_str(unsafe { crate::str::sub(s, last_end, s.len()) });
    result
}

pub fn str_replacen<'a, P: Pattern<'a>>(s: &'a str, pat: P, to: &str, count: usize) -> String {
    let mut result = String::with_capacity(32);
    let mut last_end = 0;
    let mut it = crate::str::match_indices(s, pat);
    let mut n = 0;
    while n < count {
        match it.next() {
            Some((start, part)) => {
                result.push_str(unsafe { crate::str::sub(s, last_end, start) });
                result.push_str(to);
                last_end = start + part.len();
            }
            None => break,
        }
        n += 1;
    }
    result.push_str(unsafe { crate::str::sub(s, last_end, s.len()) });
    result
}

// ---------------------------------------------------------------- traits

impl Clone for String {
    fn clone(&self) -> String {
        String { vec: self.vec.clone() }
    }
    fn clone_from(&mut self, source: &String) {
        self.vec.clear();
        self.vec.extend_from_slice(&source.vec[..]);
    }
}

impl Default for String {
    fn default() -> String {
        String::new()
    }
}

impl Deref for String {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl DerefMut for String {
    fn deref_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl fmt::Display for String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl fmt::Debug for String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Write for String {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }
    fn write_char(&mut self, c: char) -> fmt::Result {
        self.push(c);
        Ok(())
    }
}

impl PartialEq for String {
    fn eq(&self, other: &String) -> bool {
        crate::str::eq_str(self.as_str(), other.as_str())
    }
}
impl Eq for String {}

impl PartialOrd for String {
    fn partial_cmp(&self, other: &String) -> Option<crate::cmp::Ordering> {
        Some(crate::str::cmp_str(self.as_str(), other.as_str()))
    }
}

impl Ord for String {
    fn cmp(&self, other: &String) -> crate::cmp::Ordering {
        crate::str::cmp_str(self.as_str(), other.as_str())
    }
}

impl PartialEq<str> for String {
    fn eq(&self, other: &str) -> bool {
        crate::str::eq_str(self.as_str(), other)
    }
}
impl<'a> PartialEq<&'a str> for String {
    fn eq(&self, other: &&'a str) -> bool {
        crate::str::eq_str(self.as_str(), *other)
    }
}
impl PartialEq<String> for str {
    fn eq(&self, other: &String) -> bool {
        crate::str::eq_str(self, other.as_str())
    }
}
impl<'a> PartialEq<String> for &'a str {
    fn eq(&self, other: &String) -> bool {
        crate::str::eq_str(*self, other.as_str())
    }
}

impl<'a> Add<&'a str> for String {
    type Output = String;
    fn add(mut self, other: &str) -> String {
        self.push_str(other);
        self
    }
}

impl<'a> AddAssign<&'a str> for String {
    fn add_assign(&mut self, other: &str) {
        self.push_str(other);
    }
}

impl<I: StrIndex> ops::Index<I> for String {
    type Output = str;
    #[track_caller]
    fn index(&self, index: I) -> &str {
        index.index_str(self.as_str())
    }
}

impl<I: StrIndex> ops::IndexMut<I> for String {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut str {
        index.index_str_mut(self.as_mut_str())
    }
}

impl AsRef<str> for String {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsMut<str> for String {
    fn as_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl AsRef<[u8]> for String {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<'a> From<&'a str> for String {
    fn from(s: &'a str) -> String {
        let mut v = Vec::with_capacity(s.len());
        v.extend_from_slice(s.as_bytes());
        String { vec: v }
    }
}

impl<'a> From<&'a mut str> for String {
    fn from(s: &'a mut str) -> String {
        String::from(&*s)
    }
}

impl<'a> From<&'a String> for String {
    fn from(s: &'a String) -> String {
        s.clone()
    }
}

impl From<char> for String {
    fn from(c: char) -> String {
        let mut s = String::new();
        s.push(c);
        s
    }
}

impl From<String> for Vec<u8> {
    fn from(string: String) -> Vec<u8> {
        string.into_bytes()
    }
}

impl crate::str::FromStr for String {
    type Err = crate::convert::Infallible;
    fn from_str(s: &str) -> Result<String, crate::convert::Infallible> {
        Ok(String::from(s))
    }
}

impl FromIterator<char> for String {
    fn from_iter<I: IntoIterator<Item = char>>(iter: I) -> String {
        let mut buf = String::new();
        buf.extend(iter);
        buf
    }
}

impl<'a> FromIterator<&'a char> for String {
    fn from_iter<I: IntoIterator<Item = &'a char>>(iter: I) -> String {
        let mut buf = String::new();
        buf.extend(iter);
        buf
    }
}

impl<'a> FromIterator<&'a str> for String {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> String {
        let mut buf = String::new();
        buf.extend(iter);
        buf
    }
}

impl FromIterator<String> for String {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> String {
        let mut it = iter.into_iter();
        match it.next() {
            None => String::new(),
            Some(mut buf) => {
                buf.extend(it);
                buf
            }
        }
    }
}

impl Extend<char> for String {
    fn extend<I: IntoIterator<Item = char>>(&mut self, iter: I) {
        let it = iter.into_iter();
        let (lower_bound, _) = it.size_hint();
        self.reserve(lower_bound);
        let mut it = it;
        while let Some(c) = it.next() {
            self.push(c);
        }
    }
}

impl<'a> Extend<&'a char> for String {
    fn extend<I: IntoIterator<Item = &'a char>>(&mut self, iter: I) {
        let mut it = iter.into_iter();
        while let Some(c) = it.next() {
            self.push(*c);
        }
    }
}

impl<'a> Extend<&'a str> for String {
    fn extend<I: IntoIterator<Item = &'a str>>(&mut self, iter: I) {
        let mut it = iter.into_iter();
        while let Some(s) = it.next() {
            self.push_str(s);
        }
    }
}

impl Extend<String> for String {
    fn extend<I: IntoIterator<Item = String>>(&mut self, iter: I) {
        let mut it = iter.into_iter();
        while let Some(s) = it.next() {
            self.push_str(&s);
        }
    }
}

impl<'a, 'b> Pattern<'a> for &'b String {
    type Searcher = StrSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> StrSearcher<'a, 'b> {
        StrSearcher::new(haystack, self.as_str())
    }
    fn is_prefix_of(self, haystack: &'a str) -> bool {
        self.as_str().is_prefix_of(haystack)
    }
    fn is_suffix_of(self, haystack: &'a str) -> bool {
        self.as_str().is_suffix_of(haystack)
    }
    fn strip_prefix_of(self, haystack: &'a str) -> Option<&'a str> {
        self.as_str().strip_prefix_of(haystack)
    }
    fn strip_suffix_of(self, haystack: &'a str) -> Option<&'a str> {
        self.as_str().strip_suffix_of(haystack)
    }
}

// ---------------------------------------------------------------- ToString, ToOwned, Cow

pub trait ToString {
    fn to_string(&self) -> String;
}

/// Every Display type gets `to_string` (one of the blanket impls core may write).
#[cfg(not(rapid_check))]
impl<T: fmt::Display + ?Sized> ToString for T {
    fn to_string(&self) -> String {
        let mut buf = String::new();
        let mut formatter = fmt::Formatter::new(&mut buf);
        if fmt::Display::fmt(self, &mut formatter).is_err() {
            panic!("a Display implementation returned an error unexpectedly");
        }
        buf
    }
}

#[cfg(not(rapid_check))]
mod borrow_impls {
    use super::String;
    use crate::borrow::{Borrow, BorrowMut, Cow, ToOwned};
    use crate::ops::Add;

    impl Borrow<str> for String {
        fn borrow(&self) -> &str {
            self.as_str()
        }
    }

    impl BorrowMut<str> for String {
        fn borrow_mut(&mut self) -> &mut str {
            self.as_mut_str()
        }
    }

    impl ToOwned for str {
        type Owned = String;
        fn to_owned(&self) -> String {
            String::from(self)
        }
        fn clone_into(&self, target: &mut String) {
            target.clear();
            target.push_str(self);
        }
    }

    impl<'a> From<&'a str> for Cow<'a, str> {
        fn from(s: &'a str) -> Cow<'a, str> {
            Cow::Borrowed(s)
        }
    }

    impl<'a> From<String> for Cow<'a, str> {
        fn from(s: String) -> Cow<'a, str> {
            Cow::Owned(s)
        }
    }

    impl<'a> From<&'a String> for Cow<'a, str> {
        fn from(s: &'a String) -> Cow<'a, str> {
            Cow::Borrowed(s.as_str())
        }
    }

    impl<'a> From<Cow<'a, str>> for String {
        fn from(s: Cow<'a, str>) -> String {
            s.into_owned()
        }
    }

    impl<'a> PartialEq<Cow<'a, str>> for String {
        fn eq(&self, other: &Cow<'a, str>) -> bool {
            crate::str::eq_str(self.as_str(), &**other)
        }
    }

    impl<'a> PartialEq<String> for Cow<'a, str> {
        fn eq(&self, other: &String) -> bool {
            crate::str::eq_str(&**self, other.as_str())
        }
    }

    impl<'a, 'b> PartialEq<&'b str> for Cow<'a, str> {
        fn eq(&self, other: &&'b str) -> bool {
            crate::str::eq_str(&**self, *other)
        }
    }

    impl<'a> PartialEq<str> for Cow<'a, str> {
        fn eq(&self, other: &str) -> bool {
            crate::str::eq_str(&**self, other)
        }
    }

    impl<'a> Add<&'a str> for Cow<'a, str> {
        type Output = Cow<'a, str>;
        fn add(self, rhs: &'a str) -> Cow<'a, str> {
            if self.is_empty() {
                Cow::Borrowed(rhs)
            } else if rhs.is_empty() {
                self
            } else {
                let mut s = self.into_owned();
                s.push_str(rhs);
                Cow::Owned(s)
            }
        }
    }

    impl<'a> Extend<Cow<'a, str>> for String {
        fn extend<I: IntoIterator<Item = Cow<'a, str>>>(&mut self, iter: I) {
            let mut it = iter.into_iter();
            while let Some(s) = it.next() {
                self.push_str(&s);
            }
        }
    }

    impl<'a> FromIterator<Cow<'a, str>> for String {
        fn from_iter<I: IntoIterator<Item = Cow<'a, str>>>(iter: I) -> String {
            let mut buf = String::new();
            buf.extend(iter);
            buf
        }
    }

    impl crate::hash::Hash for String {
        fn hash<H: crate::hash::Hasher>(&self, hasher: &mut H) {
            crate::hash::Hash::hash(self.as_str(), hasher)
        }
    }
}

/// rustc shim only: lets tests compare/print this String with real std's.
#[cfg(rapid_check)]
mod check_bridge {
    use super::String;

    impl std::fmt::Debug for String {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Debug::fmt(self.as_str(), f)
        }
    }

    impl std::fmt::Display for String {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Display::fmt(self.as_str(), f)
        }
    }

    impl PartialEq<std::string::String> for String {
        fn eq(&self, other: &std::string::String) -> bool {
            self.as_str() == other.as_str()
        }
    }

    impl PartialEq<String> for std::string::String {
        fn eq(&self, other: &String) -> bool {
            self.as_str() == other.as_str()
        }
    }
}
