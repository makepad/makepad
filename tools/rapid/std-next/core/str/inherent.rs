//! `impl str` (Rapid only: real core owns str's inherent methods). Each method forwards to
//! the free fns in str/ and string.rs, which the rustc shim tests against real std.

use super::pattern::{DoubleEndedSearcher, Pattern, ReverseSearcher};
use super::*;
use crate::string::String;

impl str {
    pub const fn len(&self) -> usize {
        unsafe { crate::intrinsics_mem::slice_len(crate::intrinsics_mem::str_as_bytes(self)) }
    }
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub const fn as_bytes(&self) -> &[u8] {
        unsafe { crate::intrinsics_mem::str_as_bytes(self) }
    }
    pub unsafe fn as_bytes_mut(&mut self) -> &mut [u8] {
        as_bytes_mut(self)
    }
    pub const fn as_ptr(&self) -> *const u8 {
        unsafe { crate::intrinsics_mem::slice_ptr(crate::intrinsics_mem::str_as_bytes(self)) }
    }
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.as_ptr() as *mut u8
    }
    pub const fn as_str(&self) -> &str {
        self
    }

    pub fn is_char_boundary(&self, index: usize) -> bool {
        is_char_boundary(self, index)
    }
    pub fn floor_char_boundary(&self, index: usize) -> usize {
        floor_char_boundary(self, index)
    }
    pub fn ceil_char_boundary(&self, index: usize) -> usize {
        ceil_char_boundary(self, index)
    }

    pub fn get<I: StrIndex>(&self, i: I) -> Option<&str> {
        get(self, i)
    }
    pub fn get_mut<I: StrIndex>(&mut self, i: I) -> Option<&mut str> {
        get_mut(self, i)
    }
    pub unsafe fn get_unchecked<I: StrIndex>(&self, i: I) -> &str {
        index(self, i)
    }
    pub unsafe fn get_unchecked_mut<I: StrIndex>(&mut self, i: I) -> &mut str {
        index_mut(self, i)
    }
    pub unsafe fn slice_unchecked(&self, begin: usize, end: usize) -> &str {
        sub(self, begin, end)
    }
    #[track_caller]
    pub fn split_at(&self, mid: usize) -> (&str, &str) {
        split_at(self, mid)
    }
    #[track_caller]
    pub fn split_at_mut(&mut self, mid: usize) -> (&mut str, &mut str) {
        split_at_mut(self, mid)
    }
    pub fn split_at_checked(&self, mid: usize) -> Option<(&str, &str)> {
        split_at_checked(self, mid)
    }
    pub fn split_at_mut_checked(&mut self, mid: usize) -> Option<(&mut str, &mut str)> {
        split_at_mut_checked(self, mid)
    }

    pub fn chars(&self) -> Chars<'_> {
        chars(self)
    }
    pub fn char_indices(&self) -> CharIndices<'_> {
        char_indices(self)
    }
    pub fn bytes(&self) -> Bytes<'_> {
        bytes(self)
    }
    pub fn split_whitespace(&self) -> SplitWhitespace<'_> {
        split_whitespace(self)
    }
    pub fn split_ascii_whitespace(&self) -> SplitAsciiWhitespace<'_> {
        split_ascii_whitespace(self)
    }
    pub fn lines(&self) -> Lines<'_> {
        lines(self)
    }
    pub fn encode_utf16(&self) -> EncodeUtf16<'_> {
        encode_utf16(self)
    }

    pub fn contains<'a, P: Pattern<'a>>(&'a self, pat: P) -> bool {
        contains(self, pat)
    }
    pub fn starts_with<'a, P: Pattern<'a>>(&'a self, pat: P) -> bool {
        starts_with(self, pat)
    }
    pub fn ends_with<'a, P: Pattern<'a>>(&'a self, pat: P) -> bool
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        ends_with(self, pat)
    }
    pub fn find<'a, P: Pattern<'a>>(&'a self, pat: P) -> Option<usize> {
        find(self, pat)
    }
    pub fn rfind<'a, P: Pattern<'a>>(&'a self, pat: P) -> Option<usize>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rfind(self, pat)
    }
    pub fn split<'a, P: Pattern<'a>>(&'a self, pat: P) -> Split<'a, P> {
        split(self, pat)
    }
    pub fn split_inclusive<'a, P: Pattern<'a>>(&'a self, pat: P) -> SplitInclusive<'a, P> {
        split_inclusive(self, pat)
    }
    pub fn rsplit<'a, P: Pattern<'a>>(&'a self, pat: P) -> RSplit<'a, P>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rsplit(self, pat)
    }
    pub fn split_terminator<'a, P: Pattern<'a>>(&'a self, pat: P) -> SplitTerminator<'a, P> {
        split_terminator(self, pat)
    }
    pub fn rsplit_terminator<'a, P: Pattern<'a>>(&'a self, pat: P) -> RSplitTerminator<'a, P>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rsplit_terminator(self, pat)
    }
    pub fn splitn<'a, P: Pattern<'a>>(&'a self, n: usize, pat: P) -> SplitN<'a, P> {
        splitn(self, n, pat)
    }
    pub fn rsplitn<'a, P: Pattern<'a>>(&'a self, n: usize, pat: P) -> RSplitN<'a, P>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rsplitn(self, n, pat)
    }
    pub fn split_once<'a, P: Pattern<'a>>(&'a self, delimiter: P) -> Option<(&'a str, &'a str)> {
        split_once(self, delimiter)
    }
    pub fn rsplit_once<'a, P: Pattern<'a>>(&'a self, delimiter: P) -> Option<(&'a str, &'a str)>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rsplit_once(self, delimiter)
    }
    pub fn matches<'a, P: Pattern<'a>>(&'a self, pat: P) -> Matches<'a, P> {
        matches(self, pat)
    }
    pub fn rmatches<'a, P: Pattern<'a>>(&'a self, pat: P) -> RMatches<'a, P>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rmatches(self, pat)
    }
    pub fn match_indices<'a, P: Pattern<'a>>(&'a self, pat: P) -> MatchIndices<'a, P> {
        match_indices(self, pat)
    }
    pub fn rmatch_indices<'a, P: Pattern<'a>>(&'a self, pat: P) -> RMatchIndices<'a, P>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        rmatch_indices(self, pat)
    }

    pub fn trim(&self) -> &str {
        trim(self)
    }
    pub fn trim_start(&self) -> &str {
        trim_start(self)
    }
    pub fn trim_end(&self) -> &str {
        trim_end(self)
    }
    pub fn trim_left(&self) -> &str {
        trim_start(self)
    }
    pub fn trim_right(&self) -> &str {
        trim_end(self)
    }
    pub fn trim_matches<'a, P: Pattern<'a>>(&'a self, pat: P) -> &'a str
    where
        P::Searcher: DoubleEndedSearcher<'a>,
    {
        trim_matches(self, pat)
    }
    pub fn trim_start_matches<'a, P: Pattern<'a>>(&'a self, pat: P) -> &'a str {
        trim_start_matches(self, pat)
    }
    pub fn trim_end_matches<'a, P: Pattern<'a>>(&'a self, pat: P) -> &'a str
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        trim_end_matches(self, pat)
    }
    pub fn trim_left_matches<'a, P: Pattern<'a>>(&'a self, pat: P) -> &'a str {
        trim_start_matches(self, pat)
    }
    pub fn trim_right_matches<'a, P: Pattern<'a>>(&'a self, pat: P) -> &'a str
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        trim_end_matches(self, pat)
    }
    pub fn strip_prefix<'a, P: Pattern<'a>>(&'a self, prefix: P) -> Option<&'a str> {
        strip_prefix(self, prefix)
    }
    pub fn strip_suffix<'a, P: Pattern<'a>>(&'a self, suffix: P) -> Option<&'a str>
    where
        P::Searcher: ReverseSearcher<'a>,
    {
        strip_suffix(self, suffix)
    }
    pub fn trim_ascii_start(&self) -> &str {
        trim_ascii_start(self)
    }
    pub fn trim_ascii_end(&self) -> &str {
        trim_ascii_end(self)
    }
    pub fn trim_ascii(&self) -> &str {
        trim_ascii(self)
    }

    pub fn parse<F: FromStr>(&self) -> Result<F, F::Err> {
        parse(self)
    }

    pub fn is_ascii(&self) -> bool {
        is_ascii(self)
    }
    pub fn eq_ignore_ascii_case(&self, other: &str) -> bool {
        eq_ignore_ascii_case(self, other)
    }
    pub fn make_ascii_uppercase(&mut self) {
        make_ascii_uppercase(self)
    }
    pub fn make_ascii_lowercase(&mut self) {
        make_ascii_lowercase(self)
    }

    pub fn escape_debug(&self) -> EscapeDebug<'_> {
        escape_debug(self)
    }
    pub fn escape_default(&self) -> EscapeDefault<'_> {
        escape_default(self)
    }
    pub fn escape_unicode(&self) -> EscapeUnicode<'_> {
        escape_unicode(self)
    }

    // owned results (real alloc's str methods)
    pub fn to_lowercase(&self) -> String {
        crate::string::str_to_lowercase(self)
    }
    pub fn to_uppercase(&self) -> String {
        crate::string::str_to_uppercase(self)
    }
    pub fn to_ascii_uppercase(&self) -> String {
        crate::string::str_to_ascii_uppercase(self)
    }
    pub fn to_ascii_lowercase(&self) -> String {
        crate::string::str_to_ascii_lowercase(self)
    }
    #[track_caller]
    pub fn repeat(&self, n: usize) -> String {
        crate::string::str_repeat(self, n)
    }
    pub fn replace<'a, P: Pattern<'a>>(&'a self, from: P, to: &str) -> String {
        crate::string::str_replace(self, from, to)
    }
    pub fn replacen<'a, P: Pattern<'a>>(&'a self, pat: P, to: &str, count: usize) -> String {
        crate::string::str_replacen(self, pat, to, count)
    }
}
