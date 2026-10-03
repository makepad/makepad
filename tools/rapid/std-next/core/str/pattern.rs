//! The string Pattern API: what `find`, `split`, `trim_matches`, ... search for.
//! Patterns: `char`, `&str`, `&&str`, `&String` (string.rs), `&[char]`, `[char; N]`/`&[char; N]`,
//! and `FnMut(char) -> bool` closures/fns. Same match semantics as real core (non-overlapping
//! leftmost matches forward, rightmost backward; an empty `&str` matches at every boundary).

use super::validations::{decode_at, decode_before};
use crate::option::Option::{self, None, Some};

/// One step of a search.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum SearchStep {
    Match(usize, usize),
    Reject(usize, usize),
    Done,
}

pub trait Searcher<'a> {
    fn haystack(&self) -> &'a str;
    fn next(&mut self) -> SearchStep;
    fn next_match(&mut self) -> Option<(usize, usize)> {
        loop {
            match self.next() {
                SearchStep::Match(a, b) => return Some((a, b)),
                SearchStep::Done => return None,
                _ => continue,
            }
        }
    }
    fn next_reject(&mut self) -> Option<(usize, usize)> {
        loop {
            match self.next() {
                SearchStep::Reject(a, b) => return Some((a, b)),
                SearchStep::Done => return None,
                _ => continue,
            }
        }
    }
}

pub trait ReverseSearcher<'a>: Searcher<'a> {
    fn next_back(&mut self) -> SearchStep;
    fn next_match_back(&mut self) -> Option<(usize, usize)> {
        loop {
            match self.next_back() {
                SearchStep::Match(a, b) => return Some((a, b)),
                SearchStep::Done => return None,
                _ => continue,
            }
        }
    }
    fn next_reject_back(&mut self) -> Option<(usize, usize)> {
        loop {
            match self.next_back() {
                SearchStep::Reject(a, b) => return Some((a, b)),
                SearchStep::Done => return None,
                _ => continue,
            }
        }
    }
}

/// Forward and backward searches find the same matches (char-like patterns).
pub trait DoubleEndedSearcher<'a>: ReverseSearcher<'a> {}

pub trait Pattern<'a>: Sized {
    type Searcher: Searcher<'a>;

    fn into_searcher(self, haystack: &'a str) -> Self::Searcher;

    fn is_contained_in(self, haystack: &'a str) -> bool {
        self.into_searcher(haystack).next_match().is_some()
    }

    fn is_prefix_of(self, haystack: &'a str) -> bool {
        matches!(self.into_searcher(haystack).next(), SearchStep::Match(0, _))
    }

    fn is_suffix_of(self, haystack: &'a str) -> bool
    where
        Self::Searcher: ReverseSearcher<'a>,
    {
        match self.into_searcher(haystack).next_back() {
            SearchStep::Match(_, j) => haystack.len() == j,
            _ => false,
        }
    }

    fn strip_prefix_of(self, haystack: &'a str) -> Option<&'a str> {
        match self.into_searcher(haystack).next() {
            SearchStep::Match(0, len) => Some(unsafe { super::sub(haystack, len, haystack.len()) }),
            _ => None,
        }
    }

    fn strip_suffix_of(self, haystack: &'a str) -> Option<&'a str>
    where
        Self::Searcher: ReverseSearcher<'a>,
    {
        match self.into_searcher(haystack).next_back() {
            SearchStep::Match(start, end) if end == haystack.len() => Some(unsafe { super::sub(haystack, 0, start) }),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- char

#[derive(Clone, Debug)]
pub struct CharSearcher<'a> {
    haystack: &'a str,
    finger: usize,
    finger_back: usize,
    needle: char,
    utf8_size: usize,
    utf8_encoded: [u8; 4],
}

impl<'a> Pattern<'a> for char {
    type Searcher = CharSearcher<'a>;
    fn into_searcher(self, haystack: &'a str) -> CharSearcher<'a> {
        let mut utf8_encoded = [0u8; 4];
        let utf8_size = crate::char::encode_utf8_raw(self as u32, &mut utf8_encoded);
        CharSearcher { haystack, finger: 0, finger_back: haystack.len(), needle: self, utf8_size, utf8_encoded }
    }
    fn is_contained_in(self, haystack: &'a str) -> bool {
        self.into_searcher(haystack).next_match().is_some()
    }
}

impl<'a> CharSearcher<'a> {
    fn matches_at(&self, i: usize) -> bool {
        let b = self.haystack.as_bytes();
        let mut k = 0;
        while k < self.utf8_size {
            if b[i + k] != self.utf8_encoded[k] {
                return false;
            }
            k += 1;
        }
        true
    }
}

impl<'a> Searcher<'a> for CharSearcher<'a> {
    fn haystack(&self) -> &'a str {
        self.haystack
    }
    fn next(&mut self) -> SearchStep {
        if self.finger >= self.finger_back {
            return SearchStep::Done;
        }
        let old = self.finger;
        let (cp, w) = decode_at(self.haystack.as_bytes(), old);
        self.finger += w;
        if cp == self.needle as u32 {
            SearchStep::Match(old, self.finger)
        } else {
            SearchStep::Reject(old, self.finger)
        }
    }
    fn next_match(&mut self) -> Option<(usize, usize)> {
        let n = self.utf8_size;
        let b = self.haystack.as_bytes();
        let first = self.utf8_encoded[0];
        let mut i = self.finger;
        while i + n <= self.finger_back {
            if b[i] == first && self.matches_at(i) {
                self.finger = i + n;
                return Some((i, i + n));
            }
            i += 1;
        }
        self.finger = self.finger_back;
        None
    }
}

impl<'a> ReverseSearcher<'a> for CharSearcher<'a> {
    fn next_back(&mut self) -> SearchStep {
        if self.finger >= self.finger_back {
            return SearchStep::Done;
        }
        let old = self.finger_back;
        let (cp, w) = decode_before(self.haystack.as_bytes(), old);
        self.finger_back -= w;
        if cp == self.needle as u32 {
            SearchStep::Match(self.finger_back, old)
        } else {
            SearchStep::Reject(self.finger_back, old)
        }
    }
    fn next_match_back(&mut self) -> Option<(usize, usize)> {
        let n = self.utf8_size;
        let b = self.haystack.as_bytes();
        let first = self.utf8_encoded[0];
        let mut end = self.finger_back;
        while end >= self.finger + n {
            let i = end - n;
            if b[i] == first && self.matches_at(i) {
                self.finger_back = i;
                return Some((i, end));
            }
            end -= 1;
        }
        self.finger_back = self.finger;
        None
    }
}

impl<'a> DoubleEndedSearcher<'a> for CharSearcher<'a> {}

// ---------------------------------------------------------------- &str

#[derive(Clone, Debug)]
pub struct StrSearcher<'a, 'b> {
    haystack: &'a str,
    needle: &'b str,
    position: usize,
    end: usize,
    is_match_fw: bool,
    is_match_bw: bool,
    is_finished: bool,
}

impl<'a, 'b> StrSearcher<'a, 'b> {
    pub(crate) fn new(haystack: &'a str, needle: &'b str) -> StrSearcher<'a, 'b> {
        StrSearcher {
            haystack,
            needle,
            position: 0,
            end: haystack.len(),
            is_match_fw: true,
            is_match_bw: true,
            is_finished: false,
        }
    }
    fn match_at(&self, i: usize) -> bool {
        let h = self.haystack.as_bytes();
        let n = self.needle.as_bytes();
        let mut k = 0;
        while k < n.len() {
            if h[i + k] != n[k] {
                return false;
            }
            k += 1;
        }
        true
    }
}

impl<'a, 'b> Pattern<'a> for &'b str {
    type Searcher = StrSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> StrSearcher<'a, 'b> {
        StrSearcher::new(haystack, self)
    }
    fn is_prefix_of(self, haystack: &'a str) -> bool {
        super::bytes_start_with(haystack.as_bytes(), self.as_bytes())
    }
    fn is_suffix_of(self, haystack: &'a str) -> bool {
        super::bytes_end_with(haystack.as_bytes(), self.as_bytes())
    }
    fn strip_prefix_of(self, haystack: &'a str) -> Option<&'a str> {
        if super::bytes_start_with(haystack.as_bytes(), self.as_bytes()) {
            Some(unsafe { super::sub(haystack, self.len(), haystack.len()) })
        } else {
            None
        }
    }
    fn strip_suffix_of(self, haystack: &'a str) -> Option<&'a str> {
        if super::bytes_end_with(haystack.as_bytes(), self.as_bytes()) {
            Some(unsafe { super::sub(haystack, 0, haystack.len() - self.len()) })
        } else {
            None
        }
    }
}

impl<'a, 'b, 'c> Pattern<'a> for &'c &'b str {
    type Searcher = StrSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> StrSearcher<'a, 'b> {
        StrSearcher::new(haystack, *self)
    }
    fn is_prefix_of(self, haystack: &'a str) -> bool {
        (*self).is_prefix_of(haystack)
    }
    fn is_suffix_of(self, haystack: &'a str) -> bool {
        (*self).is_suffix_of(haystack)
    }
    fn strip_prefix_of(self, haystack: &'a str) -> Option<&'a str> {
        (*self).strip_prefix_of(haystack)
    }
    fn strip_suffix_of(self, haystack: &'a str) -> Option<&'a str> {
        (*self).strip_suffix_of(haystack)
    }
}

impl<'a, 'b> Searcher<'a> for StrSearcher<'a, 'b> {
    fn haystack(&self) -> &'a str {
        self.haystack
    }
    fn next(&mut self) -> SearchStep {
        if self.needle.is_empty() {
            if self.is_finished {
                return SearchStep::Done;
            }
            let is_match = self.is_match_fw;
            self.is_match_fw = !self.is_match_fw;
            let pos = self.position;
            if is_match {
                return SearchStep::Match(pos, pos);
            }
            if pos >= self.haystack.len() {
                self.is_finished = true;
                return SearchStep::Done;
            }
            let (_, w) = decode_at(self.haystack.as_bytes(), pos);
            self.position += w;
            return SearchStep::Reject(pos, self.position);
        }
        if self.position >= self.end {
            return SearchStep::Done;
        }
        let pos = self.position;
        let n = self.needle.len();
        if pos + n <= self.end && self.match_at(pos) {
            self.position = pos + n;
            return SearchStep::Match(pos, pos + n);
        }
        let (_, w) = decode_at(self.haystack.as_bytes(), pos);
        self.position = pos + w;
        SearchStep::Reject(pos, pos + w)
    }
    fn next_match(&mut self) -> Option<(usize, usize)> {
        if self.needle.is_empty() {
            loop {
                match self.next() {
                    SearchStep::Match(a, b) => return Some((a, b)),
                    SearchStep::Done => return None,
                    _ => continue,
                }
            }
        }
        let n = self.needle.len();
        let first = self.needle.as_bytes()[0];
        let h = self.haystack.as_bytes();
        let mut i = self.position;
        while i + n <= self.end {
            if h[i] == first && self.match_at(i) {
                self.position = i + n;
                return Some((i, i + n));
            }
            i += 1;
        }
        self.position = self.end;
        None
    }
}

impl<'a, 'b> ReverseSearcher<'a> for StrSearcher<'a, 'b> {
    fn next_back(&mut self) -> SearchStep {
        if self.needle.is_empty() {
            if self.is_finished {
                return SearchStep::Done;
            }
            let is_match = self.is_match_bw;
            self.is_match_bw = !self.is_match_bw;
            let end = self.end;
            if is_match {
                return SearchStep::Match(end, end);
            }
            if end == 0 {
                self.is_finished = true;
                return SearchStep::Done;
            }
            let (_, w) = decode_before(self.haystack.as_bytes(), end);
            self.end -= w;
            return SearchStep::Reject(self.end, end);
        }
        if self.end <= self.position {
            return SearchStep::Done;
        }
        let end = self.end;
        let n = self.needle.len();
        if end >= self.position + n && self.match_at(end - n) {
            self.end = end - n;
            return SearchStep::Match(end - n, end);
        }
        let (_, w) = decode_before(self.haystack.as_bytes(), end);
        self.end = end - w;
        SearchStep::Reject(end - w, end)
    }
    fn next_match_back(&mut self) -> Option<(usize, usize)> {
        if self.needle.is_empty() {
            loop {
                match self.next_back() {
                    SearchStep::Match(a, b) => return Some((a, b)),
                    SearchStep::Done => return None,
                    _ => continue,
                }
            }
        }
        let n = self.needle.len();
        let first = self.needle.as_bytes()[0];
        let h = self.haystack.as_bytes();
        let mut end = self.end;
        while end >= self.position + n {
            let i = end - n;
            if h[i] == first && self.match_at(i) {
                self.end = i;
                return Some((i, end));
            }
            end -= 1;
        }
        self.end = self.position;
        None
    }
}

// ---------------------------------------------------------------- char predicates

/// Steps through the chars of a haystack from both ends.
#[derive(Clone, Debug)]
struct CharStepper<'a> {
    haystack: &'a str,
    front: usize,
    back: usize,
}

impl<'a> CharStepper<'a> {
    fn new(haystack: &'a str) -> CharStepper<'a> {
        CharStepper { haystack, front: 0, back: haystack.len() }
    }
    fn step(&mut self) -> Option<(usize, char, usize)> {
        if self.front >= self.back {
            return None;
        }
        let i = self.front;
        let (cp, w) = decode_at(self.haystack.as_bytes(), i);
        self.front += w;
        Some((i, unsafe { crate::char::from_u32_unchecked(cp) }, i + w))
    }
    fn step_back(&mut self) -> Option<(usize, char, usize)> {
        if self.front >= self.back {
            return None;
        }
        let end = self.back;
        let (cp, w) = decode_before(self.haystack.as_bytes(), end);
        self.back -= w;
        Some((end - w, unsafe { crate::char::from_u32_unchecked(cp) }, end))
    }
}

/// Searcher for `FnMut(char) -> bool` patterns (closures, fns, `CharPred`).
#[derive(Clone)]
pub struct CharPredicateSearcher<'a, F> {
    pred: F,
    steps: CharStepper<'a>,
}

impl<'a, F: FnMut(char) -> bool> CharPredicateSearcher<'a, F> {
    pub(crate) fn new(haystack: &'a str, pred: F) -> CharPredicateSearcher<'a, F> {
        CharPredicateSearcher { pred, steps: CharStepper::new(haystack) }
    }
}

impl<'a, F: FnMut(char) -> bool> Searcher<'a> for CharPredicateSearcher<'a, F> {
    fn haystack(&self) -> &'a str {
        self.steps.haystack
    }
    fn next(&mut self) -> SearchStep {
        match self.steps.step() {
            Some((a, c, b)) => {
                if (self.pred)(c) {
                    SearchStep::Match(a, b)
                } else {
                    SearchStep::Reject(a, b)
                }
            }
            None => SearchStep::Done,
        }
    }
}

impl<'a, F: FnMut(char) -> bool> ReverseSearcher<'a> for CharPredicateSearcher<'a, F> {
    fn next_back(&mut self) -> SearchStep {
        match self.steps.step_back() {
            Some((a, c, b)) => {
                if (self.pred)(c) {
                    SearchStep::Match(a, b)
                } else {
                    SearchStep::Reject(a, b)
                }
            }
            None => SearchStep::Done,
        }
    }
}

impl<'a, F: FnMut(char) -> bool> DoubleEndedSearcher<'a> for CharPredicateSearcher<'a, F> {}

/// A char predicate as a pattern without relying on the closure blanket impl.
#[derive(Clone, Copy)]
pub struct CharPred<F>(pub F);

impl<'a, F: FnMut(char) -> bool> Pattern<'a> for CharPred<F> {
    type Searcher = CharPredicateSearcher<'a, F>;
    fn into_searcher(self, haystack: &'a str) -> CharPredicateSearcher<'a, F> {
        CharPredicateSearcher::new(haystack, self.0)
    }
}

/// Closures and fns `FnMut(char) -> bool`. (Not in the rustc shim: there it would overlap
/// with the `char`/`&str` impls, which only core itself may write.)
#[cfg(not(rapid_check))]
impl<'a, F: FnMut(char) -> bool> Pattern<'a> for F {
    type Searcher = CharPredicateSearcher<'a, F>;
    fn into_searcher(self, haystack: &'a str) -> CharPredicateSearcher<'a, F> {
        CharPredicateSearcher::new(haystack, self)
    }
}

/// `char::is_whitespace` as a pattern (split_whitespace, trim).
#[derive(Clone, Copy)]
pub(crate) struct IsWhitespace;

impl<'a> Pattern<'a> for IsWhitespace {
    type Searcher = CharPredicateSearcher<'a, fn(char) -> bool>;
    fn into_searcher(self, haystack: &'a str) -> CharPredicateSearcher<'a, fn(char) -> bool> {
        let f: fn(char) -> bool = crate::char::is_whitespace;
        CharPredicateSearcher::new(haystack, f)
    }
}

// ---------------------------------------------------------------- char sets

#[derive(Clone)]
pub struct CharSliceSearcher<'a, 'b> {
    chars: &'b [char],
    steps: CharStepper<'a>,
}

fn slice_has(chars: &[char], c: char) -> bool {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == c {
            return true;
        }
        i += 1;
    }
    false
}

impl<'a, 'b> Pattern<'a> for &'b [char] {
    type Searcher = CharSliceSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> CharSliceSearcher<'a, 'b> {
        CharSliceSearcher { chars: self, steps: CharStepper::new(haystack) }
    }
}

impl<'a, 'b> Searcher<'a> for CharSliceSearcher<'a, 'b> {
    fn haystack(&self) -> &'a str {
        self.steps.haystack
    }
    fn next(&mut self) -> SearchStep {
        match self.steps.step() {
            Some((a, c, b)) => {
                if slice_has(self.chars, c) {
                    SearchStep::Match(a, b)
                } else {
                    SearchStep::Reject(a, b)
                }
            }
            None => SearchStep::Done,
        }
    }
}

impl<'a, 'b> ReverseSearcher<'a> for CharSliceSearcher<'a, 'b> {
    fn next_back(&mut self) -> SearchStep {
        match self.steps.step_back() {
            Some((a, c, b)) => {
                if slice_has(self.chars, c) {
                    SearchStep::Match(a, b)
                } else {
                    SearchStep::Reject(a, b)
                }
            }
            None => SearchStep::Done,
        }
    }
}

impl<'a, 'b> DoubleEndedSearcher<'a> for CharSliceSearcher<'a, 'b> {}

/// `[char; N]` patterns (needs const generics in Rapid: cfg rapid_const_generics).
#[cfg(any(rapid_check, rapid_const_generics))]
#[derive(Clone)]
#[cfg(any(rapid_check, rapid_const_generics))]
pub struct CharArraySearcher<'a, const N: usize> {
    chars: [char; N],
    steps: CharStepper<'a>,
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, const N: usize> Pattern<'a> for [char; N] {
    type Searcher = CharArraySearcher<'a, N>;
    fn into_searcher(self, haystack: &'a str) -> CharArraySearcher<'a, N> {
        CharArraySearcher { chars: self, steps: CharStepper::new(haystack) }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, 'b, const N: usize> Pattern<'a> for &'b [char; N] {
    type Searcher = CharSliceSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> CharSliceSearcher<'a, 'b> {
        CharSliceSearcher { chars: &self[..], steps: CharStepper::new(haystack) }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, const N: usize> Searcher<'a> for CharArraySearcher<'a, N> {
    fn haystack(&self) -> &'a str {
        self.steps.haystack
    }
    fn next(&mut self) -> SearchStep {
        match self.steps.step() {
            Some((a, c, b)) => {
                if slice_has(&self.chars, c) {
                    SearchStep::Match(a, b)
                } else {
                    SearchStep::Reject(a, b)
                }
            }
            None => SearchStep::Done,
        }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, const N: usize> ReverseSearcher<'a> for CharArraySearcher<'a, N> {
    fn next_back(&mut self) -> SearchStep {
        match self.steps.step_back() {
            Some((a, c, b)) => {
                if slice_has(&self.chars, c) {
                    SearchStep::Match(a, b)
                } else {
                    SearchStep::Reject(a, b)
                }
            }
            None => SearchStep::Done,
        }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, const N: usize> DoubleEndedSearcher<'a> for CharArraySearcher<'a, N> {}
