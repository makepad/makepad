//! split / rsplit / splitn / split_terminator / split_inclusive / lines / matches / match_indices.
//! `SplitInternal` is real core's state machine, so edge cases (empty pieces, trailing
//! empties, mixed front/back iteration) behave identically.

use super::pattern::{DoubleEndedSearcher, IsWhitespace, Pattern, ReverseSearcher, Searcher};
use super::sub;
use crate::iter::{DoubleEndedIterator, FusedIterator, Iterator};
use crate::option::Option::{self, None, Some};

pub(crate) struct SplitInternal<'a, P: Pattern<'a>> {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) matcher: P::Searcher,
    pub(crate) allow_trailing_empty: bool,
    pub(crate) finished: bool,
}

impl<'a, P: Pattern<'a>> Clone for SplitInternal<'a, P>
where
    P::Searcher: Clone,
{
    fn clone(&self) -> SplitInternal<'a, P> {
        SplitInternal {
            start: self.start,
            end: self.end,
            matcher: self.matcher.clone(),
            allow_trailing_empty: self.allow_trailing_empty,
            finished: self.finished,
        }
    }
}

impl<'a, P: Pattern<'a>> SplitInternal<'a, P> {
    pub(crate) fn new(haystack: &'a str, pat: P, allow_trailing_empty: bool) -> SplitInternal<'a, P> {
        SplitInternal {
            start: 0,
            end: haystack.len(),
            matcher: pat.into_searcher(haystack),
            allow_trailing_empty,
            finished: false,
        }
    }

    fn get_end(&mut self) -> Option<&'a str> {
        if !self.finished {
            self.finished = true;
            if self.allow_trailing_empty || self.end - self.start > 0 {
                return Some(unsafe { sub(self.matcher.haystack(), self.start, self.end) });
            }
        }
        None
    }

    fn next(&mut self) -> Option<&'a str> {
        if self.finished {
            return None;
        }
        let haystack = self.matcher.haystack();
        match self.matcher.next_match() {
            Some((a, b)) => {
                let elt = unsafe { sub(haystack, self.start, a) };
                self.start = b;
                Some(elt)
            }
            None => self.get_end(),
        }
    }

    fn next_inclusive(&mut self) -> Option<&'a str> {
        if self.finished {
            return None;
        }
        let haystack = self.matcher.haystack();
        match self.matcher.next_match() {
            Some((_, b)) => {
                let elt = unsafe { sub(haystack, self.start, b) };
                self.start = b;
                Some(elt)
            }
            None => self.get_end(),
        }
    }

    fn remainder(&self) -> Option<&'a str> {
        if self.finished {
            return None;
        }
        Some(unsafe { sub(self.matcher.haystack(), self.start, self.end) })
    }
}

impl<'a, P: Pattern<'a>> SplitInternal<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        if self.finished {
            return None;
        }
        if !self.allow_trailing_empty {
            self.allow_trailing_empty = true;
            match self.next_back() {
                Some(elt) if !elt.is_empty() => return Some(elt),
                _ => {
                    if self.finished {
                        return None;
                    }
                }
            }
        }
        let haystack = self.matcher.haystack();
        match self.matcher.next_match_back() {
            Some((a, b)) => {
                let elt = unsafe { sub(haystack, b, self.end) };
                self.end = a;
                Some(elt)
            }
            None => {
                self.finished = true;
                Some(unsafe { sub(haystack, self.start, self.end) })
            }
        }
    }

    fn next_back_inclusive(&mut self) -> Option<&'a str> {
        if self.finished {
            return None;
        }
        if !self.allow_trailing_empty {
            self.allow_trailing_empty = true;
            match self.next_back_inclusive() {
                Some(elt) if !elt.is_empty() => return Some(elt),
                _ => {
                    if self.finished {
                        return None;
                    }
                }
            }
        }
        let haystack = self.matcher.haystack();
        match self.matcher.next_match_back() {
            Some((_, b)) => {
                let elt = unsafe { sub(haystack, b, self.end) };
                self.end = b;
                Some(elt)
            }
            None => {
                self.finished = true;
                Some(unsafe { sub(haystack, self.start, self.end) })
            }
        }
    }
}

// ---------------------------------------------------------------- Split / RSplit

pub struct Split<'a, P: Pattern<'a>>(pub(crate) SplitInternal<'a, P>);
pub struct RSplit<'a, P: Pattern<'a>>(pub(crate) SplitInternal<'a, P>);
pub struct SplitTerminator<'a, P: Pattern<'a>>(pub(crate) SplitInternal<'a, P>);
pub struct RSplitTerminator<'a, P: Pattern<'a>>(pub(crate) SplitInternal<'a, P>);
pub struct SplitInclusive<'a, P: Pattern<'a>>(pub(crate) SplitInternal<'a, P>);

impl<'a, P: Pattern<'a>> Iterator for Split<'a, P> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        self.0.next()
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for Split<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        self.0.next_back()
    }
}
impl<'a, P: Pattern<'a>> FusedIterator for Split<'a, P> {}
impl<'a, P: Pattern<'a>> Split<'a, P> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.0.remainder()
    }
}
impl<'a, P: Pattern<'a>> Clone for Split<'a, P>
where
    P::Searcher: Clone,
{
    fn clone(&self) -> Split<'a, P> {
        Split(self.0.clone())
    }
}

impl<'a, P: Pattern<'a>> Iterator for RSplit<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        self.0.next_back()
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for RSplit<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        self.0.next()
    }
}
impl<'a, P: Pattern<'a>> FusedIterator for RSplit<'a, P> where P::Searcher: ReverseSearcher<'a> {}
impl<'a, P: Pattern<'a>> RSplit<'a, P> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.0.remainder()
    }
}
impl<'a, P: Pattern<'a>> Clone for RSplit<'a, P>
where
    P::Searcher: Clone,
{
    fn clone(&self) -> RSplit<'a, P> {
        RSplit(self.0.clone())
    }
}

impl<'a, P: Pattern<'a>> Iterator for SplitTerminator<'a, P> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        self.0.next()
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for SplitTerminator<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        self.0.next_back()
    }
}
impl<'a, P: Pattern<'a>> FusedIterator for SplitTerminator<'a, P> {}
impl<'a, P: Pattern<'a>> SplitTerminator<'a, P> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.0.remainder()
    }
}

impl<'a, P: Pattern<'a>> Iterator for RSplitTerminator<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        self.0.next_back()
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for RSplitTerminator<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        self.0.next()
    }
}

impl<'a, P: Pattern<'a>> Iterator for SplitInclusive<'a, P> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        self.0.next_inclusive()
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for SplitInclusive<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        self.0.next_back_inclusive()
    }
}
impl<'a, P: Pattern<'a>> FusedIterator for SplitInclusive<'a, P> {}
impl<'a, P: Pattern<'a>> SplitInclusive<'a, P> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.0.remainder()
    }
}
impl<'a, P: Pattern<'a>> Clone for SplitInclusive<'a, P>
where
    P::Searcher: Clone,
{
    fn clone(&self) -> SplitInclusive<'a, P> {
        SplitInclusive(self.0.clone())
    }
}

// ---------------------------------------------------------------- SplitN / RSplitN

pub struct SplitN<'a, P: Pattern<'a>> {
    pub(crate) iter: SplitInternal<'a, P>,
    pub(crate) count: usize,
}

pub struct RSplitN<'a, P: Pattern<'a>> {
    pub(crate) iter: SplitInternal<'a, P>,
    pub(crate) count: usize,
}

impl<'a, P: Pattern<'a>> Iterator for SplitN<'a, P> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        match self.count {
            0 => None,
            1 => {
                self.count = 0;
                self.iter.get_end()
            }
            _ => {
                self.count -= 1;
                self.iter.next()
            }
        }
    }
}
impl<'a, P: Pattern<'a>> FusedIterator for SplitN<'a, P> {}
impl<'a, P: Pattern<'a>> SplitN<'a, P> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.iter.remainder()
    }
}

impl<'a, P: Pattern<'a>> Iterator for RSplitN<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        match self.count {
            0 => None,
            1 => {
                self.count = 0;
                self.iter.get_end()
            }
            _ => {
                self.count -= 1;
                self.iter.next_back()
            }
        }
    }
}
impl<'a, P: Pattern<'a>> RSplitN<'a, P> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.iter.remainder()
    }
}

// ---------------------------------------------------------------- matches

pub struct MatchIndices<'a, P: Pattern<'a>>(pub(crate) P::Searcher);
pub struct RMatchIndices<'a, P: Pattern<'a>>(pub(crate) P::Searcher);
pub struct Matches<'a, P: Pattern<'a>>(pub(crate) P::Searcher);
pub struct RMatches<'a, P: Pattern<'a>>(pub(crate) P::Searcher);

impl<'a, P: Pattern<'a>> Iterator for MatchIndices<'a, P> {
    type Item = (usize, &'a str);
    fn next(&mut self) -> Option<(usize, &'a str)> {
        match self.0.next_match() {
            Some((a, b)) => Some((a, unsafe { sub(self.0.haystack(), a, b) })),
            None => None,
        }
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for MatchIndices<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<(usize, &'a str)> {
        match self.0.next_match_back() {
            Some((a, b)) => Some((a, unsafe { sub(self.0.haystack(), a, b) })),
            None => None,
        }
    }
}

impl<'a, P: Pattern<'a>> Iterator for RMatchIndices<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    type Item = (usize, &'a str);
    fn next(&mut self) -> Option<(usize, &'a str)> {
        match self.0.next_match_back() {
            Some((a, b)) => Some((a, unsafe { sub(self.0.haystack(), a, b) })),
            None => None,
        }
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for RMatchIndices<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<(usize, &'a str)> {
        match self.0.next_match() {
            Some((a, b)) => Some((a, unsafe { sub(self.0.haystack(), a, b) })),
            None => None,
        }
    }
}

impl<'a, P: Pattern<'a>> Iterator for Matches<'a, P> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        match self.0.next_match() {
            Some((a, b)) => Some(unsafe { sub(self.0.haystack(), a, b) }),
            None => None,
        }
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for Matches<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        match self.0.next_match_back() {
            Some((a, b)) => Some(unsafe { sub(self.0.haystack(), a, b) }),
            None => None,
        }
    }
}

impl<'a, P: Pattern<'a>> Iterator for RMatches<'a, P>
where
    P::Searcher: ReverseSearcher<'a>,
{
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        match self.0.next_match_back() {
            Some((a, b)) => Some(unsafe { sub(self.0.haystack(), a, b) }),
            None => None,
        }
    }
}
impl<'a, P: Pattern<'a>> DoubleEndedIterator for RMatches<'a, P>
where
    P::Searcher: DoubleEndedSearcher<'a>,
{
    fn next_back(&mut self) -> Option<&'a str> {
        match self.0.next_match() {
            Some((a, b)) => Some(unsafe { sub(self.0.haystack(), a, b) }),
            None => None,
        }
    }
}

// ---------------------------------------------------------------- lines / whitespace

/// `str::lines`: split_inclusive('\n') with one trailing "\n" or "\r\n" removed per line.
#[derive(Clone)]
pub struct Lines<'a>(pub(crate) SplitInclusive<'a, char>);

fn strip_line_end(line: &str) -> &str {
    let b = line.as_bytes();
    let n = b.len();
    if n == 0 || b[n - 1] != b'\n' {
        return line;
    }
    if n >= 2 && b[n - 2] == b'\r' {
        unsafe { sub(line, 0, n - 2) }
    } else {
        unsafe { sub(line, 0, n - 1) }
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        match self.0.next() {
            Some(l) => Some(strip_line_end(l)),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
    fn last(self) -> Option<&'a str> {
        let mut s = self;
        s.next_back()
    }
}
impl<'a> DoubleEndedIterator for Lines<'a> {
    fn next_back(&mut self) -> Option<&'a str> {
        match self.0.next_back() {
            Some(l) => Some(strip_line_end(l)),
            None => None,
        }
    }
}
impl<'a> FusedIterator for Lines<'a> {}
impl<'a> Lines<'a> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.0.remainder()
    }
}

/// `str::split_whitespace`: Unicode whitespace separated, empty pieces skipped.
#[derive(Clone)]
pub struct SplitWhitespace<'a> {
    pub(crate) inner: Split<'a, IsWhitespace>,
}

impl<'a> Iterator for SplitWhitespace<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        loop {
            match self.inner.next() {
                Some(s) if s.is_empty() => continue,
                other => return other,
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
    fn last(self) -> Option<&'a str> {
        let mut s = self;
        s.next_back()
    }
}
impl<'a> DoubleEndedIterator for SplitWhitespace<'a> {
    fn next_back(&mut self) -> Option<&'a str> {
        loop {
            match self.inner.next_back() {
                Some(s) if s.is_empty() => continue,
                other => return other,
            }
        }
    }
}
impl<'a> FusedIterator for SplitWhitespace<'a> {}
impl<'a> SplitWhitespace<'a> {
    pub fn remainder(&self) -> Option<&'a str> {
        self.inner.remainder()
    }
}

/// `str::split_ascii_whitespace`
#[derive(Clone)]
pub struct SplitAsciiWhitespace<'a> {
    s: &'a str,
    front: usize,
    back: usize,
}

pub fn split_ascii_whitespace(s: &str) -> SplitAsciiWhitespace<'_> {
    SplitAsciiWhitespace { s, front: 0, back: s.len() }
}

fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

impl<'a> Iterator for SplitAsciiWhitespace<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        let b = self.s.as_bytes();
        while self.front < self.back && is_ascii_ws(b[self.front]) {
            self.front += 1;
        }
        if self.front >= self.back {
            return None;
        }
        let start = self.front;
        while self.front < self.back && !is_ascii_ws(b[self.front]) {
            self.front += 1;
        }
        Some(unsafe { sub(self.s, start, self.front) })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}
impl<'a> DoubleEndedIterator for SplitAsciiWhitespace<'a> {
    fn next_back(&mut self) -> Option<&'a str> {
        let b = self.s.as_bytes();
        while self.back > self.front && is_ascii_ws(b[self.back - 1]) {
            self.back -= 1;
        }
        if self.back <= self.front {
            return None;
        }
        let end = self.back;
        while self.back > self.front && !is_ascii_ws(b[self.back - 1]) {
            self.back -= 1;
        }
        Some(unsafe { sub(self.s, self.back, end) })
    }
}
impl<'a> FusedIterator for SplitAsciiWhitespace<'a> {}
impl<'a> SplitAsciiWhitespace<'a> {
    pub fn remainder(&self) -> Option<&'a str> {
        let b = self.s.as_bytes();
        let mut f = self.front;
        while f < self.back && is_ascii_ws(b[f]) {
            f += 1;
        }
        if f >= self.back {
            None
        } else {
            Some(unsafe { sub(self.s, f, self.back) })
        }
    }
}
