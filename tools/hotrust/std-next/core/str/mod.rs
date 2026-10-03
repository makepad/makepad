//! str: UTF-8 validation, slicing with real core's panic messages, the Pattern API,
//! splitting, trimming, escapes and parsing. The `impl str` methods (str/inherent.rs, HotRust
//! only) forward to the free fns here, which the rustc shim tests against real std.

pub mod pattern;

mod iter;
mod lossy;
mod split;
mod validations;

#[cfg(not(hotrust_check))]
mod inherent;
#[cfg(not(hotrust_check))]
mod traits_impls;

pub use iter::{Bytes, CharIndices, Chars, EncodeUtf16, EscapeDebug, EscapeDefault, EscapeUnicode};
pub use lossy::{utf8_chunks, Utf8Chunk, Utf8Chunks};
pub use split::{
    Lines, MatchIndices, Matches, RMatchIndices, RMatches, RSplit, RSplitN, RSplitTerminator, Split,
    SplitAsciiWhitespace, SplitInclusive, SplitN, SplitTerminator, SplitWhitespace,
};
pub use validations::{utf8_char_width, Utf8Error};

pub(crate) use iter::write_escape_debug;
/// Free-fn forms of the str methods (the `impl str` in inherent.rs forwards to these).
pub use iter::{bytes, char_indices, chars, encode_utf16, escape_debug, escape_default, escape_unicode};
pub use split::split_ascii_whitespace;

use crate::fmt;
use crate::ops::{Range, RangeFrom, RangeFull, RangeInclusive, RangeTo, RangeToInclusive};
use crate::option::Option::{self, None, Some};
use crate::result::Result::{self, Err, Ok};
use pattern::{DoubleEndedSearcher, IsWhitespace, Pattern, ReverseSearcher, Searcher};
use split::SplitInternal;

// ---------------------------------------------------------------- raw helpers

/// `&s[a..b]` without checks (a <= b <= len, both char boundaries).
pub(crate) unsafe fn sub(s: &str, a: usize, b: usize) -> &str {
    let p = crate::intrinsics_mem::slice_ptr(s.as_bytes());
    crate::intrinsics_mem::str_from_raw(crate::intrinsics_mem::ptr_add(p, a), b - a)
}

pub(crate) unsafe fn sub_mut(s: &mut str, a: usize, b: usize) -> &mut str {
    let whole = as_bytes_mut(s);
    from_utf8_unchecked_mut(&mut whole[a..b])
}

pub(crate) fn bytes_start_with(h: &[u8], n: &[u8]) -> bool {
    if n.len() > h.len() {
        return false;
    }
    let mut i = 0;
    while i < n.len() {
        if h[i] != n[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub(crate) fn bytes_end_with(h: &[u8], n: &[u8]) -> bool {
    if n.len() > h.len() {
        return false;
    }
    let off = h.len() - n.len();
    let mut i = 0;
    while i < n.len() {
        if h[off + i] != n[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Decimal digits of `v` (what `{}` prints for an unsigned integer).
pub(crate) fn write_dec(v: usize, w: &mut dyn fmt::Write) -> fmt::Result {
    let mut buf = [0u8; 20];
    let mut i = 20;
    let mut x = v;
    loop {
        i -= 1;
        buf[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    w.write_str(unsafe { from_utf8_unchecked(&buf[i..]) })
}

// ---------------------------------------------------------------- conversions

pub fn from_utf8(v: &[u8]) -> Result<&str, Utf8Error> {
    match validations::run_utf8_validation(v) {
        Ok(()) => Ok(unsafe { from_utf8_unchecked(v) }),
        Err(e) => Err(e),
    }
}

pub fn from_utf8_mut(v: &mut [u8]) -> Result<&mut str, Utf8Error> {
    match validations::run_utf8_validation(v) {
        Ok(()) => Ok(unsafe { from_utf8_unchecked_mut(v) }),
        Err(e) => Err(e),
    }
}

pub unsafe fn from_utf8_unchecked(v: &[u8]) -> &str {
    crate::intrinsics_mem::str_from_raw(crate::intrinsics_mem::slice_ptr(v), v.len())
}

pub unsafe fn from_utf8_unchecked_mut(v: &mut [u8]) -> &mut str {
    crate::intrinsics_mem::transmute::<&mut [u8], &mut str>(v)
}

pub unsafe fn from_raw_parts<'a>(ptr: *const u8, len: usize) -> &'a str {
    crate::intrinsics_mem::str_from_raw(ptr, len)
}

pub unsafe fn as_bytes_mut(s: &mut str) -> &mut [u8] {
    crate::intrinsics_mem::transmute::<&mut str, &mut [u8]>(s)
}

// ---------------------------------------------------------------- boundaries and slicing

pub fn is_char_boundary(s: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    if index >= s.len() {
        index == s.len()
    } else {
        validations::is_utf8_char_boundary(s.as_bytes()[index])
    }
}

pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let b = s.as_bytes();
    let mut i = index;
    while !validations::is_utf8_char_boundary(b[i]) {
        i -= 1;
    }
    i
}

pub fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let b = s.as_bytes();
    let mut i = index;
    while !validations::is_utf8_char_boundary(b[i]) {
        i += 1;
        if i >= s.len() {
            break;
        }
    }
    i
}

fn char_at(s: &str, i: usize) -> char {
    let (cp, _) = validations::decode_at(s.as_bytes(), i);
    unsafe { crate::char::from_u32_unchecked(cp) }
}

/// Real core's message for a failed `&s[begin..end]`.
#[track_caller]
#[cold]
pub(crate) fn slice_error_fail(s: &str, begin: usize, end: usize) -> ! {
    let len = s.len();
    if begin > len {
        panic!("start byte index {} is out of bounds for string of length {}", begin, len);
    }
    if end > len {
        panic!("end byte index {} is out of bounds for string of length {}", end, len);
    }
    if begin > end {
        panic!("byte range starts at {} but ends at {}", begin, end);
    }
    if !is_char_boundary(s, begin) {
        let floor = floor_char_boundary(s, begin);
        let ceil = ceil_char_boundary(s, begin);
        let ch = char_at(s, floor);
        panic!(
            "start byte index {} is not a char boundary; it is inside {:?} (bytes {}..{} of string)",
            begin, ch, floor, ceil
        );
    }
    if !is_char_boundary(s, end) {
        let floor = floor_char_boundary(s, end);
        let ceil = ceil_char_boundary(s, end);
        let ch = char_at(s, floor);
        panic!(
            "end byte index {} is not a char boundary; it is inside {:?} (bytes {}..{} of string)",
            end, ch, floor, ceil
        );
    }
    panic!("end byte index {} is out of bounds for string of length {}", end, len);
}

/// What can index a str: the usize range types (real core's `SliceIndex<str>`).
pub trait StrIndex {
    fn get_str(self, s: &str) -> Option<&str>;
    fn get_str_mut(self, s: &mut str) -> Option<&mut str>;
    fn index_str(self, s: &str) -> &str;
    fn index_str_mut(self, s: &mut str) -> &mut str;
}

fn range_ok(s: &str, start: usize, end: usize) -> bool {
    start <= end && is_char_boundary(s, start) && is_char_boundary(s, end)
}

impl StrIndex for Range<usize> {
    fn get_str(self, s: &str) -> Option<&str> {
        if range_ok(s, self.start, self.end) {
            Some(unsafe { sub(s, self.start, self.end) })
        } else {
            None
        }
    }
    fn get_str_mut(self, s: &mut str) -> Option<&mut str> {
        if range_ok(s, self.start, self.end) {
            Some(unsafe { sub_mut(s, self.start, self.end) })
        } else {
            None
        }
    }
    #[track_caller]
    fn index_str(self, s: &str) -> &str {
        if range_ok(s, self.start, self.end) {
            unsafe { sub(s, self.start, self.end) }
        } else {
            slice_error_fail(s, self.start, self.end)
        }
    }
    #[track_caller]
    fn index_str_mut(self, s: &mut str) -> &mut str {
        if range_ok(s, self.start, self.end) {
            unsafe { sub_mut(s, self.start, self.end) }
        } else {
            slice_error_fail(s, self.start, self.end)
        }
    }
}

impl StrIndex for RangeTo<usize> {
    fn get_str(self, s: &str) -> Option<&str> {
        if is_char_boundary(s, self.end) {
            Some(unsafe { sub(s, 0, self.end) })
        } else {
            None
        }
    }
    fn get_str_mut(self, s: &mut str) -> Option<&mut str> {
        if is_char_boundary(s, self.end) {
            Some(unsafe { sub_mut(s, 0, self.end) })
        } else {
            None
        }
    }
    #[track_caller]
    fn index_str(self, s: &str) -> &str {
        if is_char_boundary(s, self.end) {
            unsafe { sub(s, 0, self.end) }
        } else {
            slice_error_fail(s, 0, self.end)
        }
    }
    #[track_caller]
    fn index_str_mut(self, s: &mut str) -> &mut str {
        if is_char_boundary(s, self.end) {
            unsafe { sub_mut(s, 0, self.end) }
        } else {
            slice_error_fail(s, 0, self.end)
        }
    }
}

impl StrIndex for RangeFrom<usize> {
    fn get_str(self, s: &str) -> Option<&str> {
        if is_char_boundary(s, self.start) {
            Some(unsafe { sub(s, self.start, s.len()) })
        } else {
            None
        }
    }
    fn get_str_mut(self, s: &mut str) -> Option<&mut str> {
        if is_char_boundary(s, self.start) {
            let len = s.len();
            Some(unsafe { sub_mut(s, self.start, len) })
        } else {
            None
        }
    }
    #[track_caller]
    fn index_str(self, s: &str) -> &str {
        if is_char_boundary(s, self.start) {
            unsafe { sub(s, self.start, s.len()) }
        } else {
            slice_error_fail(s, self.start, s.len())
        }
    }
    #[track_caller]
    fn index_str_mut(self, s: &mut str) -> &mut str {
        let len = s.len();
        if is_char_boundary(s, self.start) {
            unsafe { sub_mut(s, self.start, len) }
        } else {
            slice_error_fail(s, self.start, len)
        }
    }
}

impl StrIndex for RangeFull {
    fn get_str(self, s: &str) -> Option<&str> {
        Some(s)
    }
    fn get_str_mut(self, s: &mut str) -> Option<&mut str> {
        Some(s)
    }
    fn index_str(self, s: &str) -> &str {
        s
    }
    fn index_str_mut(self, s: &mut str) -> &mut str {
        s
    }
}

/// (start, exclusive end) of an inclusive range, as real core's `into_slice_range`.
fn inclusive_parts(r: &RangeInclusive<usize>) -> (usize, usize, bool) {
    let start = *r.start();
    let end = *r.end();
    // exhausted iff empty although start <= end
    let exhausted = r.is_empty() && start <= end;
    (start, end, exhausted)
}

impl StrIndex for RangeInclusive<usize> {
    fn get_str(self, s: &str) -> Option<&str> {
        let (start, end, exhausted) = inclusive_parts(&self);
        if end >= s.len() {
            return None;
        }
        let e = end + 1;
        let st = if exhausted { e } else { start };
        (st..e).get_str(s)
    }
    fn get_str_mut(self, s: &mut str) -> Option<&mut str> {
        let (start, end, exhausted) = inclusive_parts(&self);
        if end >= s.len() {
            return None;
        }
        let e = end + 1;
        let st = if exhausted { e } else { start };
        (st..e).get_str_mut(s)
    }
    #[track_caller]
    fn index_str(self, s: &str) -> &str {
        let (mut start, mut end, exhausted) = inclusive_parts(&self);
        if end < s.len() {
            end = end + 1;
            start = if exhausted { end } else { start };
            if range_ok(s, start, end) {
                return unsafe { sub(s, start, end) };
            }
        }
        slice_error_fail(s, start, end)
    }
    #[track_caller]
    fn index_str_mut(self, s: &mut str) -> &mut str {
        let (mut start, mut end, exhausted) = inclusive_parts(&self);
        if end < s.len() {
            end = end + 1;
            start = if exhausted { end } else { start };
            if range_ok(s, start, end) {
                return unsafe { sub_mut(s, start, end) };
            }
        }
        slice_error_fail(s, start, end)
    }
}

impl StrIndex for RangeToInclusive<usize> {
    fn get_str(self, s: &str) -> Option<&str> {
        (0..=self.end).get_str(s)
    }
    fn get_str_mut(self, s: &mut str) -> Option<&mut str> {
        (0..=self.end).get_str_mut(s)
    }
    #[track_caller]
    fn index_str(self, s: &str) -> &str {
        (0..=self.end).index_str(s)
    }
    #[track_caller]
    fn index_str_mut(self, s: &mut str) -> &mut str {
        (0..=self.end).index_str_mut(s)
    }
}

pub fn get<I: StrIndex>(s: &str, i: I) -> Option<&str> {
    i.get_str(s)
}

pub fn get_mut<I: StrIndex>(s: &mut str, i: I) -> Option<&mut str> {
    i.get_str_mut(s)
}

#[track_caller]
pub fn index<I: StrIndex>(s: &str, i: I) -> &str {
    i.index_str(s)
}

#[track_caller]
pub fn index_mut<I: StrIndex>(s: &mut str, i: I) -> &mut str {
    i.index_str_mut(s)
}

#[track_caller]
pub fn split_at(s: &str, mid: usize) -> (&str, &str) {
    match split_at_checked(s, mid) {
        None => slice_error_fail(s, 0, mid),
        Some(pair) => pair,
    }
}

pub fn split_at_checked(s: &str, mid: usize) -> Option<(&str, &str)> {
    if is_char_boundary(s, mid) {
        Some(unsafe { (sub(s, 0, mid), sub(s, mid, s.len())) })
    } else {
        None
    }
}

#[track_caller]
pub fn split_at_mut(s: &mut str, mid: usize) -> (&mut str, &mut str) {
    if is_char_boundary(s, mid) {
        unsafe { split_at_mut_unchecked(s, mid) }
    } else {
        slice_error_fail(s, 0, mid)
    }
}

pub fn split_at_mut_checked(s: &mut str, mid: usize) -> Option<(&mut str, &mut str)> {
    if is_char_boundary(s, mid) {
        Some(unsafe { split_at_mut_unchecked(s, mid) })
    } else {
        None
    }
}

unsafe fn split_at_mut_unchecked(s: &mut str, mid: usize) -> (&mut str, &mut str) {
    let len = s.len();
    let p = crate::intrinsics_mem::slice_ptr(s.as_bytes()) as *mut u8;
    let a = crate::intrinsics_mem::slice_from_raw_mut(p, mid);
    let b = crate::intrinsics_mem::slice_from_raw_mut(crate::intrinsics_mem::ptr_add_mut(p, mid), len - mid);
    (from_utf8_unchecked_mut(a), from_utf8_unchecked_mut(b))
}

// ---------------------------------------------------------------- pattern searches

pub fn contains<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> bool {
    pat.is_contained_in(s)
}

pub fn starts_with<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> bool {
    pat.is_prefix_of(s)
}

pub fn ends_with<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> bool
where
    P::Searcher: ReverseSearcher<'a>,
{
    pat.is_suffix_of(s)
}

pub fn find<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> Option<usize> {
    match pat.into_searcher(s).next_match() {
        Some((i, _)) => Some(i),
        None => None,
    }
}

pub fn rfind<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> Option<usize>
where
    P::Searcher: ReverseSearcher<'a>,
{
    match pat.into_searcher(s).next_match_back() {
        Some((i, _)) => Some(i),
        None => None,
    }
}

pub fn split<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> Split<'a, P> {
    Split(SplitInternal::new(s, pat, true))
}

pub fn rsplit<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> RSplit<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    RSplit(SplitInternal::new(s, pat, true))
}

pub fn split_inclusive<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> SplitInclusive<'a, P> {
    SplitInclusive(SplitInternal::new(s, pat, false))
}

pub fn split_terminator<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> SplitTerminator<'a, P> {
    SplitTerminator(SplitInternal::new(s, pat, false))
}

pub fn rsplit_terminator<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> RSplitTerminator<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    RSplitTerminator(SplitInternal::new(s, pat, false))
}

pub fn splitn<'a, P: Pattern<'a>>(s: &'a str, n: usize, pat: P) -> SplitN<'a, P> {
    SplitN { iter: SplitInternal::new(s, pat, true), count: n }
}

pub fn rsplitn<'a, P: Pattern<'a>>(s: &'a str, n: usize, pat: P) -> RSplitN<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    RSplitN { iter: SplitInternal::new(s, pat, true), count: n }
}

pub fn split_once<'a, P: Pattern<'a>>(s: &'a str, delimiter: P) -> Option<(&'a str, &'a str)> {
    let (start, end) = delimiter.into_searcher(s).next_match()?;
    Some(unsafe { (sub(s, 0, start), sub(s, end, s.len())) })
}

pub fn rsplit_once<'a, P: Pattern<'a>>(s: &'a str, delimiter: P) -> Option<(&'a str, &'a str)>
where
    P::Searcher: ReverseSearcher<'a>,
{
    let (start, end) = delimiter.into_searcher(s).next_match_back()?;
    Some(unsafe { (sub(s, 0, start), sub(s, end, s.len())) })
}

pub fn matches<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> Matches<'a, P> {
    Matches(pat.into_searcher(s))
}

pub fn rmatches<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> RMatches<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    RMatches(pat.into_searcher(s))
}

pub fn match_indices<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> MatchIndices<'a, P> {
    MatchIndices(pat.into_searcher(s))
}

pub fn rmatch_indices<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> RMatchIndices<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    RMatchIndices(pat.into_searcher(s))
}

pub fn lines(s: &str) -> Lines<'_> {
    Lines(split_inclusive(s, '\n'))
}

pub fn split_whitespace(s: &str) -> SplitWhitespace<'_> {
    SplitWhitespace { inner: split(s, IsWhitespace) }
}

pub fn trim_matches<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> &'a str
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    let mut i = 0;
    let mut j = 0;
    let mut matcher = pat.into_searcher(s);
    if let Some((a, b)) = matcher.next_reject() {
        i = a;
        j = b;
    }
    if let Some((_, b)) = matcher.next_reject_back() {
        j = b;
    }
    unsafe { sub(s, i, j) }
}

pub fn trim_start_matches<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> &'a str {
    let mut i = s.len();
    let mut matcher = pat.into_searcher(s);
    if let Some((a, _)) = matcher.next_reject() {
        i = a;
    }
    unsafe { sub(s, i, s.len()) }
}

pub fn trim_end_matches<'a, P: Pattern<'a>>(s: &'a str, pat: P) -> &'a str
where
    P::Searcher: ReverseSearcher<'a>,
{
    let mut j = 0;
    let mut matcher = pat.into_searcher(s);
    if let Some((_, b)) = matcher.next_reject_back() {
        j = b;
    }
    unsafe { sub(s, 0, j) }
}

pub fn strip_prefix<'a, P: Pattern<'a>>(s: &'a str, prefix: P) -> Option<&'a str> {
    prefix.strip_prefix_of(s)
}

pub fn strip_suffix<'a, P: Pattern<'a>>(s: &'a str, suffix: P) -> Option<&'a str>
where
    P::Searcher: ReverseSearcher<'a>,
{
    suffix.strip_suffix_of(s)
}

pub fn trim(s: &str) -> &str {
    trim_matches(s, IsWhitespace)
}

pub fn trim_start(s: &str) -> &str {
    trim_start_matches(s, IsWhitespace)
}

pub fn trim_end(s: &str) -> &str {
    trim_end_matches(s, IsWhitespace)
}

// ---------------------------------------------------------------- ASCII

pub fn is_ascii(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] >= 0x80 {
            return false;
        }
        i += 1;
    }
    true
}

fn ascii_lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' {
        b + 32
    } else {
        b
    }
}

fn ascii_upper(b: u8) -> u8 {
    if b >= b'a' && b <= b'z' {
        b - 32
    } else {
        b
    }
}

pub fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    let x = a.as_bytes();
    let y = b.as_bytes();
    if x.len() != y.len() {
        return false;
    }
    let mut i = 0;
    while i < x.len() {
        if ascii_lower(x[i]) != ascii_lower(y[i]) {
            return false;
        }
        i += 1;
    }
    true
}

pub fn make_ascii_uppercase(s: &mut str) {
    let b = unsafe { as_bytes_mut(s) };
    let mut i = 0;
    while i < b.len() {
        b[i] = ascii_upper(b[i]);
        i += 1;
    }
}

pub fn make_ascii_lowercase(s: &mut str) {
    let b = unsafe { as_bytes_mut(s) };
    let mut i = 0;
    while i < b.len() {
        b[i] = ascii_lower(b[i]);
        i += 1;
    }
}

fn is_ascii_ws_byte(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

pub fn trim_ascii_start(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && is_ascii_ws_byte(b[i]) {
        i += 1;
    }
    unsafe { sub(s, i, b.len()) }
}

pub fn trim_ascii_end(s: &str) -> &str {
    let b = s.as_bytes();
    let mut j = b.len();
    while j > 0 && is_ascii_ws_byte(b[j - 1]) {
        j -= 1;
    }
    unsafe { sub(s, 0, j) }
}

pub fn trim_ascii(s: &str) -> &str {
    trim_ascii_end(trim_ascii_start(s))
}

// ---------------------------------------------------------------- comparison

/// Byte-wise ordering of two strs (what str's Ord is).
pub fn cmp_str(a: &str, b: &str) -> crate::cmp::Ordering {
    let x = a.as_bytes();
    let y = b.as_bytes();
    let n = if x.len() < y.len() { x.len() } else { y.len() };
    let mut i = 0;
    while i < n {
        if x[i] != y[i] {
            return if x[i] < y[i] { crate::cmp::Ordering::Less } else { crate::cmp::Ordering::Greater };
        }
        i += 1;
    }
    if x.len() < y.len() {
        crate::cmp::Ordering::Less
    } else if x.len() > y.len() {
        crate::cmp::Ordering::Greater
    } else {
        crate::cmp::Ordering::Equal
    }
}

pub fn eq_str(a: &str, b: &str) -> bool {
    let x = a.as_bytes();
    let y = b.as_bytes();
    if x.len() != y.len() {
        return false;
    }
    let mut i = 0;
    while i < x.len() {
        if x[i] != y[i] {
            return false;
        }
        i += 1;
    }
    true
}

// ---------------------------------------------------------------- parsing

/// Parse a value from a string (`str::parse`).
pub trait FromStr: Sized {
    type Err;
    fn from_str(s: &str) -> Result<Self, Self::Err>;
}

pub fn parse<F: FromStr>(s: &str) -> Result<F, F::Err> {
    F::from_str(s)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseBoolError;

impl fmt::Display for ParseBoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("provided string was not `true` or `false`", f)
    }
}

#[cfg(not(hotrust_check))]
impl crate::error::Error for ParseBoolError {}

impl FromStr for bool {
    type Err = ParseBoolError;
    fn from_str(s: &str) -> Result<bool, ParseBoolError> {
        if eq_str(s, "true") {
            Ok(true)
        } else if eq_str(s, "false") {
            Ok(false)
        } else {
            Err(ParseBoolError)
        }
    }
}
