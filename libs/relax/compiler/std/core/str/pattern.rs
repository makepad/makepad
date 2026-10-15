//! Port of real core's Two-Way string search (MIT/Apache-2.0).
//! The string Pattern API: what `find`, `split`, `trim_matches`, ... search for.
//! Patterns: `char`, `&str`, `&&str`, `&String` (string.rs), `&[char]`, `[char; N]`/`&[char; N]`,
//! and `FnMut(char) -> bool` closures/fns. Same match semantics as real core (non-overlapping
//! leftmost matches forward, rightmost backward; an empty `&str` matches at every boundary).

use crate::cmp;
use crate::slice::memchr::{memchr, memrchr};

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
        let last = self.utf8_encoded[n - 1];
        while self.finger < self.finger_back {
            let found = memchr(last, &b[self.finger..self.finger_back]);
            match found {
                Some(i) => self.finger += i + 1,
                None => { self.finger = self.finger_back; return None; }
            }
            if self.finger >= n {
                let start = self.finger - n;
                if self.matches_at(start) { return Some((start, self.finger)); }
            }
        }
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
        let last = self.utf8_encoded[n - 1];
        while self.finger_back >= self.finger + n {
            match memrchr(last, &b[self.finger + n - 1..self.finger_back]) {
                Some(i) => {
                    let end = self.finger + n + i;
                    if self.matches_at(end - n) {
                        self.finger_back = end - n;
                        return Some((end - n, end));
                    }
                    self.finger_back = end - 1;
                }
                None => break,
            }
        }
        self.finger_back = self.finger;
        None
    }
}

impl<'a> DoubleEndedSearcher<'a> for CharSearcher<'a> {}

// ---------------------------------------------------------------- &str

#[derive(Clone, Debug)]
/// Associated type for `<&str as Pattern>::Searcher<'a>`.
pub struct StrSearcher<'a, 'b> {
    haystack: &'a str,
    needle: &'b str,

    searcher: StrSearcherImpl,
}

#[derive(Clone, Debug)]
enum StrSearcherImpl {
    Empty(EmptyNeedle),
    TwoWay(TwoWaySearcher),
}

#[derive(Clone, Debug)]
struct EmptyNeedle {
    position: usize,
    end: usize,
    is_match_fw: bool,
    is_match_bw: bool,
    // Needed in case of an empty haystack, see #85462
    is_finished: bool,
}

impl<'a, 'b> StrSearcher<'a, 'b> {
    pub(crate) fn new(haystack: &'a str, needle: &'b str) -> StrSearcher<'a, 'b> {
        if needle.is_empty() {
            StrSearcher {
                haystack,
                needle,
                searcher: StrSearcherImpl::Empty(EmptyNeedle {
                    position: 0,
                    end: haystack.len(),
                    is_match_fw: true,
                    is_match_bw: true,
                    is_finished: false,
                }),
            }
        } else {
            StrSearcher {
                haystack,
                needle,
                searcher: StrSearcherImpl::TwoWay(TwoWaySearcher::new(
                    needle.as_bytes(),
                    haystack.len(),
                )),
            }
        }
    }
}

impl<'a, 'b> Searcher<'a> for StrSearcher<'a, 'b> {
    #[inline]
    fn haystack(&self) -> &'a str {
        self.haystack
    }

    #[inline]
    fn next(&mut self) -> SearchStep {
        match self.searcher {
            StrSearcherImpl::Empty(ref mut searcher) => {
                if searcher.is_finished {
                    return SearchStep::Done;
                }
                // empty needle rejects every char and matches every empty string between them
                let is_match = searcher.is_match_fw;
                searcher.is_match_fw = !searcher.is_match_fw;
                let pos = searcher.position;
                match self.haystack[pos..].chars().next() {
                    _ if is_match => SearchStep::Match(pos, pos),
                    None => {
                        searcher.is_finished = true;
                        SearchStep::Done
                    }
                    Some(ch) => {
                        searcher.position += ch.len_utf8();
                        SearchStep::Reject(pos, searcher.position)
                    }
                }
            }
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                // TwoWaySearcher produces valid *Match* indices that split at char boundaries
                // as long as it does correct matching and that haystack and needle are
                // valid UTF-8
                // *Rejects* from the algorithm can fall on any indices, but we will walk them
                // manually to the next character boundary, so that they are utf-8 safe.
                if searcher.position == self.haystack.len() {
                    return SearchStep::Done;
                }
                let is_long = searcher.memory == usize::MAX;
                match searcher.next::<RejectAndMatch>(
                    self.haystack.as_bytes(),
                    self.needle.as_bytes(),
                    is_long,
                ) {
                    SearchStep::Reject(a, mut b) => {
                        // skip to next char boundary
                        while !self.haystack.is_char_boundary(b) {
                            b += 1;
                        }
                        searcher.position = cmp::max(b, searcher.position);
                        SearchStep::Reject(a, b)
                    }
                    otherwise => otherwise,
                }
            }
        }
    }

    #[inline]
    fn next_match(&mut self) -> Option<(usize, usize)> {
        match self.searcher {
            StrSearcherImpl::Empty(..) => loop {
                match self.next() {
                    SearchStep::Match(a, b) => return Some((a, b)),
                    SearchStep::Done => return None,
                    SearchStep::Reject(..) => {}
                }
            },
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                let is_long = searcher.memory == usize::MAX;
                // write out `true` and `false` cases to encourage the compiler
                // to specialize the two cases separately.
                if is_long {
                    searcher.next::<MatchOnly>(
                        self.haystack.as_bytes(),
                        self.needle.as_bytes(),
                        true,
                    )
                } else {
                    searcher.next::<MatchOnly>(
                        self.haystack.as_bytes(),
                        self.needle.as_bytes(),
                        false,
                    )
                }
            }
        }
    }
}

impl<'a, 'b> ReverseSearcher<'a> for StrSearcher<'a, 'b> {
    #[inline]
    fn next_back(&mut self) -> SearchStep {
        match self.searcher {
            StrSearcherImpl::Empty(ref mut searcher) => {
                if searcher.is_finished {
                    return SearchStep::Done;
                }
                let is_match = searcher.is_match_bw;
                searcher.is_match_bw = !searcher.is_match_bw;
                let end = searcher.end;
                match self.haystack[..end].chars().next_back() {
                    _ if is_match => SearchStep::Match(end, end),
                    None => {
                        searcher.is_finished = true;
                        SearchStep::Done
                    }
                    Some(ch) => {
                        searcher.end -= ch.len_utf8();
                        SearchStep::Reject(searcher.end, end)
                    }
                }
            }
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                if searcher.end == 0 {
                    return SearchStep::Done;
                }
                let is_long = searcher.memory == usize::MAX;
                match searcher.next_back::<RejectAndMatch>(
                    self.haystack.as_bytes(),
                    self.needle.as_bytes(),
                    is_long,
                ) {
                    SearchStep::Reject(mut a, b) => {
                        // skip to next char boundary
                        while !self.haystack.is_char_boundary(a) {
                            a -= 1;
                        }
                        searcher.end = cmp::min(a, searcher.end);
                        SearchStep::Reject(a, b)
                    }
                    otherwise => otherwise,
                }
            }
        }
    }

    #[inline]
    fn next_match_back(&mut self) -> Option<(usize, usize)> {
        match self.searcher {
            StrSearcherImpl::Empty(..) => loop {
                match self.next_back() {
                    SearchStep::Match(a, b) => return Some((a, b)),
                    SearchStep::Done => return None,
                    SearchStep::Reject(..) => {}
                }
            },
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                let is_long = searcher.memory == usize::MAX;
                // write out `true` and `false`, like `next_match`
                if is_long {
                    searcher.next_back::<MatchOnly>(
                        self.haystack.as_bytes(),
                        self.needle.as_bytes(),
                        true,
                    )
                } else {
                    searcher.next_back::<MatchOnly>(
                        self.haystack.as_bytes(),
                        self.needle.as_bytes(),
                        false,
                    )
                }
            }
        }
    }
}

/// The internal state of the two-way substring search algorithm.
#[derive(Clone, Debug)]
struct TwoWaySearcher {
    // constants
    /// critical factorization index
    crit_pos: usize,
    /// critical factorization index for reversed needle
    crit_pos_back: usize,
    period: usize,
    /// `byteset` is an extension (not part of the two way algorithm);
    /// it's a 64-bit "fingerprint" where each set bit `j` corresponds
    /// to a (byte & 63) == j present in the needle.
    byteset: u64,

    // variables
    position: usize,
    end: usize,
    /// index into needle before which we have already matched
    memory: usize,
    /// index into needle after which we have already matched
    memory_back: usize,
}

// Crochemore and Perrin, Two-way string-matching (1991).
impl TwoWaySearcher {
    fn new(needle: &[u8], end: usize) -> TwoWaySearcher {
        let (crit_pos_false, period_false) = TwoWaySearcher::maximal_suffix(needle, false);
        let (crit_pos_true, period_true) = TwoWaySearcher::maximal_suffix(needle, true);

        let (crit_pos, period) = if crit_pos_false > crit_pos_true {
            (crit_pos_false, period_false)
        } else {
            (crit_pos_true, period_true)
        };

        // A particularly readable explanation of what's going on here can be found
        // in Crochemore and Rytter's book "Text Algorithms", ch 13. Specifically
        // see the code for "Algorithm CP" on p. 323.
        //
        // What's going on is we have some critical factorization (u, v) of the
        // needle, and we want to determine whether u is a suffix of
        // &v[..period]. If it is, we use "Algorithm CP1". Otherwise we use
        // "Algorithm CP2", which is optimized for when the period of the needle
        // is large.
        if crate::slice::slice_eq(&needle[..crit_pos], &needle[period..period + crit_pos]) {
            // short period case -- the period is exact
            // compute a separate critical factorization for the reversed needle
            // x = u' v' where |v'| < period(x).
            //
            // This is sped up by the period being known already.
            // Note that a case like x = "acba" may be factored exactly forwards
            // (crit_pos = 1, period = 3) while being factored with approximate
            // period in reverse (crit_pos = 2, period = 2). We use the given
            // reverse factorization but keep the exact period.
            let crit_pos_back = needle.len()
                - cmp::max(
                    TwoWaySearcher::reverse_maximal_suffix(needle, period, false),
                    TwoWaySearcher::reverse_maximal_suffix(needle, period, true),
                );

            TwoWaySearcher {
                crit_pos,
                crit_pos_back,
                period,
                byteset: Self::byteset_create(&needle[..period]),

                position: 0,
                end,
                memory: 0,
                memory_back: needle.len(),
            }
        } else {
            // long period case -- we have an approximation to the actual period,
            // and don't use memorization.
            //
            // Approximate the period by lower bound max(|u|, |v|) + 1.
            // The critical factorization is efficient to use for both forward and
            // reverse search.

            TwoWaySearcher {
                crit_pos,
                crit_pos_back: crit_pos,
                period: cmp::max(crit_pos, needle.len() - crit_pos) + 1,
                byteset: Self::byteset_create(needle),

                position: 0,
                end,
                memory: usize::MAX, // Dummy value to signify that the period is long
                memory_back: usize::MAX,
            }
        }
    }

    #[inline]
    fn byteset_create(bytes: &[u8]) -> u64 {
        let mut set = 0u64;
        let mut i = 0;
        while i < bytes.len() {
            set |= 1u64 << (bytes[i] & 0x3f);
            i += 1;
        }
        set
    }

    #[inline]
    fn byteset_contains(&self, byte: u8) -> bool {
        (self.byteset >> ((byte & 0x3f) as usize)) & 1 != 0
    }

    // One of the main ideas of Two-Way is that we factorize the needle into
    // two halves, (u, v), and begin trying to find v in the haystack by scanning
    // left to right. If v matches, we try to match u by scanning right to left.
    // How far we can jump when we encounter a mismatch is all based on the fact
    // that (u, v) is a critical factorization for the needle.
    #[inline]
    fn next<S>(&mut self, haystack: &[u8], needle: &[u8], long_period: bool) -> S::Output
    where
        S: TwoWayStrategy,
    {
        // `next()` uses `self.position` as its cursor
        let old_pos = self.position;
        let needle_last = needle.len() - 1;
        'search: loop {
            // Check that we have room to search in
            // position + needle_last can not overflow if we assume slices
            // are bounded by isize's range.
            let tail_byte = match haystack.get(self.position + needle_last) {
                Some(&b) => b,
                None => {
                    self.position = haystack.len();
                    return S::rejecting(old_pos, self.position);
                }
            };

            if S::use_early_reject() && old_pos != self.position {
                return S::rejecting(old_pos, self.position);
            }

            // Quickly skip by large portions unrelated to our substring
            if !self.byteset_contains(tail_byte) {
                self.position += needle.len();
                if !long_period {
                    self.memory = 0;
                }
                continue 'search;
            }

            // See if the right part of the needle matches
            let start =
                if long_period { self.crit_pos } else { cmp::max(self.crit_pos, self.memory) };
            for i in start..needle.len() {
                if needle[i] != haystack[self.position + i] {
                    self.position += i - self.crit_pos + 1;
                    if !long_period {
                        self.memory = 0;
                    }
                    continue 'search;
                }
            }

            // See if the left part of the needle matches
            let start = if long_period { 0 } else { self.memory };
            for i in (start..self.crit_pos).rev() {
                if needle[i] != haystack[self.position + i] {
                    self.position += self.period;
                    if !long_period {
                        self.memory = needle.len() - self.period;
                    }
                    continue 'search;
                }
            }

            // We have found a match!
            let match_pos = self.position;

            // Note: add self.period instead of needle.len() to have overlapping matches
            self.position += needle.len();
            if !long_period {
                self.memory = 0; // set to needle.len() - self.period for overlapping matches
            }

            return S::matching(match_pos, match_pos + needle.len());
        }
    }

    // Follows the ideas in `next()`.
    //
    // The definitions are symmetrical, with period(x) = period(reverse(x))
    // and local_period(u, v) = local_period(reverse(v), reverse(u)), so if (u, v)
    // is a critical factorization, so is (reverse(v), reverse(u)).
    //
    // For the reverse case we have computed a critical factorization x = u' v'
    // (field `crit_pos_back`). We need |u| < period(x) for the forward case and
    // thus |v'| < period(x) for the reverse.
    //
    // To search in reverse through the haystack, we search forward through
    // a reversed haystack with a reversed needle, matching first u' and then v'.
    #[inline]
    fn next_back<S>(&mut self, haystack: &[u8], needle: &[u8], long_period: bool) -> S::Output
    where
        S: TwoWayStrategy,
    {
        // `next_back()` uses `self.end` as its cursor -- so that `next()` and `next_back()`
        // are independent.
        let old_end = self.end;
        'search: loop {
            // Check that we have room to search in
            // end - needle.len() will wrap around when there is no more room,
            // but due to slice length limits it can never wrap all the way back
            // into the length of haystack.
            let front_byte = match haystack.get(self.end.wrapping_sub(needle.len())) {
                Some(&b) => b,
                None => {
                    self.end = 0;
                    return S::rejecting(0, old_end);
                }
            };

            if S::use_early_reject() && old_end != self.end {
                return S::rejecting(self.end, old_end);
            }

            // Quickly skip by large portions unrelated to our substring
            if !self.byteset_contains(front_byte) {
                self.end -= needle.len();
                if !long_period {
                    self.memory_back = needle.len();
                }
                continue 'search;
            }

            // See if the left part of the needle matches
            let crit = if long_period {
                self.crit_pos_back
            } else {
                cmp::min(self.crit_pos_back, self.memory_back)
            };
            for i in (0..crit).rev() {
                if needle[i] != haystack[self.end - needle.len() + i] {
                    self.end -= self.crit_pos_back - i;
                    if !long_period {
                        self.memory_back = needle.len();
                    }
                    continue 'search;
                }
            }

            // See if the right part of the needle matches
            let needle_end = if long_period { needle.len() } else { self.memory_back };
            for i in self.crit_pos_back..needle_end {
                if needle[i] != haystack[self.end - needle.len() + i] {
                    self.end -= self.period;
                    if !long_period {
                        self.memory_back = self.period;
                    }
                    continue 'search;
                }
            }

            // We have found a match!
            let match_pos = self.end - needle.len();
            // Note: sub self.period instead of needle.len() to have overlapping matches
            self.end -= needle.len();
            if !long_period {
                self.memory_back = needle.len();
            }

            return S::matching(match_pos, match_pos + needle.len());
        }
    }

    // Compute the maximal suffix of `arr`.
    //
    // The maximal suffix is a possible critical factorization (u, v) of `arr`.
    //
    // Returns (`i`, `p`) where `i` is the starting index of v and `p` is the
    // period of v.
    //
    // `order_greater` determines if lexical order is `<` or `>`. Both
    // orders must be computed -- the ordering with the largest `i` gives
    // a critical factorization.
    //
    // For long period cases, the resulting period is not exact (it is too short).
    #[inline]
    fn maximal_suffix(arr: &[u8], order_greater: bool) -> (usize, usize) {
        let mut left = 0; // Corresponds to i in the paper
        let mut right = 1; // Corresponds to j in the paper
        let mut offset = 0; // Corresponds to k in the paper, but starting at 0
        // to match 0-based indexing.
        let mut period = 1; // Corresponds to p in the paper

        while let Some(&a) = arr.get(right + offset) {
            // `left` will be inbounds when `right` is.
            let b = arr[left + offset];
            if (a < b && !order_greater) || (a > b && order_greater) {
                // Suffix is smaller, period is entire prefix so far.
                right += offset + 1;
                offset = 0;
                period = right - left;
            } else if a == b {
                // Advance through repetition of the current period.
                if offset + 1 == period {
                    right += offset + 1;
                    offset = 0;
                } else {
                    offset += 1;
                }
            } else {
                // Suffix is larger, start over from current location.
                left = right;
                right += 1;
                offset = 0;
                period = 1;
            }
        }
        (left, period)
    }

    // Compute the maximal suffix of the reverse of `arr`.
    //
    // The maximal suffix is a possible critical factorization (u', v') of `arr`.
    //
    // Returns `i` where `i` is the starting index of v', from the back;
    // returns immediately when a period of `known_period` is reached.
    //
    // `order_greater` determines if lexical order is `<` or `>`. Both
    // orders must be computed -- the ordering with the largest `i` gives
    // a critical factorization.
    //
    // For long period cases, the resulting period is not exact (it is too short).
    fn reverse_maximal_suffix(arr: &[u8], known_period: usize, order_greater: bool) -> usize {
        let mut left = 0; // Corresponds to i in the paper
        let mut right = 1; // Corresponds to j in the paper
        let mut offset = 0; // Corresponds to k in the paper, but starting at 0
        // to match 0-based indexing.
        let mut period = 1; // Corresponds to p in the paper
        let n = arr.len();

        while right + offset < n {
            let a = arr[n - (1 + right + offset)];
            let b = arr[n - (1 + left + offset)];
            if (a < b && !order_greater) || (a > b && order_greater) {
                // Suffix is smaller, period is entire prefix so far.
                right += offset + 1;
                offset = 0;
                period = right - left;
            } else if a == b {
                // Advance through repetition of the current period.
                if offset + 1 == period {
                    right += offset + 1;
                    offset = 0;
                } else {
                    offset += 1;
                }
            } else {
                // Suffix is larger, start over from current location.
                left = right;
                right += 1;
                offset = 0;
                period = 1;
            }
            if period == known_period {
                break;
            }
        }
        debug_assert!(period <= known_period);
        left
    }
}

// TwoWayStrategy allows the algorithm to either skip non-matches as quickly
// as possible, or to work in a mode where it emits Rejects relatively quickly.
trait TwoWayStrategy {
    type Output;
    fn use_early_reject() -> bool;
    fn rejecting(a: usize, b: usize) -> Self::Output;
    fn matching(a: usize, b: usize) -> Self::Output;
}

/// Skip to match intervals as quickly as possible
enum MatchOnly {}

impl TwoWayStrategy for MatchOnly {
    type Output = Option<(usize, usize)>;

    #[inline]
    fn use_early_reject() -> bool {
        false
    }
    #[inline]
    fn rejecting(_a: usize, _b: usize) -> Self::Output {
        None
    }
    #[inline]
    fn matching(a: usize, b: usize) -> Self::Output {
        Some((a, b))
    }
}

/// Emit Rejects regularly
enum RejectAndMatch {}

impl TwoWayStrategy for RejectAndMatch {
    type Output = SearchStep;

    #[inline]
    fn use_early_reject() -> bool {
        true
    }
    #[inline]
    fn rejecting(a: usize, b: usize) -> Self::Output {
        SearchStep::Reject(a, b)
    }
    #[inline]
    fn matching(a: usize, b: usize) -> Self::Output {
        SearchStep::Match(a, b)
    }
}

impl<'a, 'b> Pattern<'a> for &'b str {
    type Searcher = StrSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> StrSearcher<'a, 'b> {
        StrSearcher::new(haystack, self)
    }
    fn is_contained_in(self, haystack: &'a str) -> bool {
        if self.is_empty() { return true; }
        if self.len() >= haystack.len() {
            return crate::slice::slice_eq(self.as_bytes(), haystack.as_bytes());
        }
        if self.len() == 1 {
            return memchr(self.as_bytes()[0], haystack.as_bytes()).is_some();
        }
        // Scalar fallback from real core's short-needle contains path. Bound
        // this by the same short window; larger inputs retain Two-Way's linear
        // worst-case behavior (including highly repetitive needles).
        if self.len() <= 32 && haystack.len() < 16 + self.len() - 1 {
            let needle = self.as_bytes();
            let bytes = haystack.as_bytes();
            let mut i = 0;
            while i <= bytes.len() - needle.len() {
                if crate::slice::slice_eq(&bytes[i..i + needle.len()], needle) {
                    return true;
                }
                i += 1;
            }
            return false;
        }
        self.into_searcher(haystack).next_match().is_some()
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
#[cfg(not(relax_check))]
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

/// `[char; N]` patterns.
#[derive(Clone)]
pub struct CharArraySearcher<'a, const N: usize> {
    chars: [char; N],
    steps: CharStepper<'a>,
}

impl<'a, const N: usize> Pattern<'a> for [char; N] {
    type Searcher = CharArraySearcher<'a, N>;
    fn into_searcher(self, haystack: &'a str) -> CharArraySearcher<'a, N> {
        CharArraySearcher { chars: self, steps: CharStepper::new(haystack) }
    }
}

impl<'a, 'b, const N: usize> Pattern<'a> for &'b [char; N] {
    type Searcher = CharSliceSearcher<'a, 'b>;
    fn into_searcher(self, haystack: &'a str) -> CharSliceSearcher<'a, 'b> {
        CharSliceSearcher { chars: &self[..], steps: CharStepper::new(haystack) }
    }
}

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

impl<'a, const N: usize> DoubleEndedSearcher<'a> for CharArraySearcher<'a, N> {}
