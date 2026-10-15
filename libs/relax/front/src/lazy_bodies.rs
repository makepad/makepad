//! Parser arrays over a published file, and the file's lazily parsed body segments.
//!
//! A part parse (ParsedFile::part) continues the file's token, text and position arrays.
//! It reads the file's own arrays in place (owned or mapped) and keeps only what it appends:
//! a `Seg` is a published base slice followed by the parse's own suffix, `Toks` adds the few
//! base tokens the parse changes in place (`>>` split by eat_gt, restored by a macro fragment
//! match). Every read sees exactly what one contiguous copy of base + suffix would hold.
use super::*;
use crate::packed::PackedToks;
use std::cmp::Ordering;
use std::ops::{Index, Range};

/// slice::binary_search_by over the array `at(0)..at(len)`, step for step as std does it: an
/// array read in two parts (or with changed elements) answers exactly as one contiguous copy
/// would, whatever its order. One contiguous slice is searched by std itself.
fn binary_search_at<'t, T: 't>(len: usize, at: impl Fn(usize) -> &'t T, mut f: impl FnMut(&T) -> Ordering) -> Result<usize, usize> {
    let mut size = len;
    if size == 0 {
        return Err(0);
    }
    let mut base = 0usize;
    while size > 1 {
        let half = size / 2;
        let mid = base + half;
        let cmp = f(at(mid));
        base = if cmp == Ordering::Greater { base } else { mid };
        size -= half;
    }
    let cmp = f(at(base));
    if cmp == Ordering::Equal { Ok(base) } else { Err(base + (cmp == Ordering::Less) as usize) }
}

fn partition_at<'t, T: 't>(len: usize, at: impl Fn(usize) -> &'t T, mut pred: impl FnMut(&T) -> bool) -> usize {
    binary_search_at(len, at, |x| if pred(x) { Ordering::Less } else { Ordering::Greater }).unwrap_or_else(|i| i)
}

/// A parser array: a published file's array (read in place), then what this parse appends.
pub struct Seg<'a, T> {
    base: &'a [T],
    own: Vec<T>,
}

impl<T> Default for Seg<'_, T> {
    fn default() -> Self { Self { base: &[], own: Vec::new() } }
}

impl<T> From<Vec<T>> for Seg<'_, T> {
    fn from(own: Vec<T>) -> Self { Self { base: &[], own } }
}

impl<'a, T> Seg<'a, T> {
    /// Continues `base` (a published array): pushes append past its end.
    pub fn on(base: &'a [T]) -> Self { Self { base, own: Vec::new() } }
    #[inline]
    pub fn len(&self) -> usize { self.base.len() + self.own.len() }
    #[inline]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    #[inline]
    pub fn get(&self, i: usize) -> Option<&T> {
        match i.checked_sub(self.base.len()) {
            Some(k) => self.own.get(k),
            None => self.base.get(i),
        }
    }
    pub fn last(&self) -> Option<&T> { self.own.last().or_else(|| self.base.last()) }
    #[inline]
    pub fn push(&mut self, v: T) { self.own.push(v) }
    pub fn iter(&self) -> std::iter::Chain<std::slice::Iter<'_, T>, std::slice::Iter<'_, T>> { self.base.iter().chain(self.own.iter()) }
    /// What this parse appended (the whole array when it continues nothing).
    pub fn own(&self) -> &[T] { &self.own }
    pub fn into_own(self) -> Vec<T> { self.own }
    pub fn view(&self) -> SegRef<'_, T> { SegRef { base: self.base, own: &self.own } }
    pub fn partition_point(&self, pred: impl FnMut(&T) -> bool) -> usize { self.view().partition_point(pred) }
    pub fn clear(&mut self) { self.base = &[]; self.own.clear(); }
}

impl<T: Clone> Seg<'_, T> {
    pub fn extend_from_slice(&mut self, v: &[T]) { self.own.extend_from_slice(v) }
}

impl<T> Extend<T> for Seg<'_, T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) { self.own.extend(iter) }
}

impl<T> Index<usize> for Seg<'_, T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        match i.checked_sub(self.base.len()) {
            Some(k) => &self.own[k],
            None => &self.base[i],
        }
    }
}

impl Seg<'_, u8> {
    /// Text within one segment (an expansion's text is appended whole).
    #[inline]
    pub fn slice(&self, lo: usize, hi: usize) -> &[u8] { self.view().slice(lo, hi) }
}

impl Index<Range<usize>> for Seg<'_, u8> {
    type Output = [u8];
    #[inline]
    fn index(&self, r: Range<usize>) -> &[u8] { self.slice(r.start, r.end) }
}

/// A borrowed `Seg`, or a plain slice (`own` empty).
pub struct SegRef<'a, T> {
    pub base: &'a [T],
    pub own: &'a [T],
}

impl<T> Clone for SegRef<'_, T> {
    fn clone(&self) -> Self { *self }
}
impl<T> Copy for SegRef<'_, T> {}

impl<'a, T> From<&'a [T]> for SegRef<'a, T> {
    fn from(base: &'a [T]) -> Self { Self { base, own: &[] } }
}

impl<'a, T> SegRef<'a, T> {
    #[inline]
    pub fn len(&self) -> usize { self.base.len() + self.own.len() }
    #[inline]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    #[inline]
    pub fn get(&self, i: usize) -> Option<&'a T> {
        match i.checked_sub(self.base.len()) {
            Some(k) => self.own.get(k),
            None => self.base.get(i),
        }
    }
    pub fn iter(&self) -> std::iter::Chain<std::slice::Iter<'a, T>, std::slice::Iter<'a, T>> { self.base.iter().chain(self.own.iter()) }
    /// slice::partition_point over `base` followed by `own`
    pub fn partition_point(&self, pred: impl FnMut(&T) -> bool) -> usize {
        if self.own.is_empty() { return self.base.partition_point(pred); }
        if self.base.is_empty() { return self.own.partition_point(pred); }
        partition_at(self.len(), |i| &self[i], pred)
    }
    /// slice::binary_search_by_key over `base` followed by `own`
    pub fn binary_search_by_key<B: Ord>(&self, b: &B, mut f: impl FnMut(&T) -> B) -> Result<usize, usize> {
        if self.own.is_empty() { return self.base.binary_search_by_key(b, f); }
        if self.base.is_empty() { return self.own.binary_search_by_key(b, f); }
        binary_search_at(self.len(), |i| &self[i], |k| f(k).cmp(b))
    }
}

impl<'a> SegRef<'a, u8> {
    /// Bytes [lo, hi) within one segment.
    #[inline]
    pub fn slice(&self, lo: usize, hi: usize) -> &'a [u8] {
        let nb = self.base.len();
        if lo >= nb && !(lo == nb && hi == nb) {
            &self.own[lo - nb..hi - nb]
        } else {
            assert!(hi <= nb, "parser text range {lo}..{hi} crosses a published file's expansion text ({nb} bytes)");
            &self.base[lo..hi]
        }
    }
    /// Bytes [lo, hi) as one buffer (copied only when they cross into `own`).
    pub fn window(&self, lo: usize, hi: usize) -> std::borrow::Cow<'a, [u8]> {
        let nb = self.base.len();
        if lo >= nb || hi <= nb {
            return std::borrow::Cow::Borrowed(self.slice(lo, hi));
        }
        let mut v = self.base[lo..].to_vec();
        v.extend_from_slice(&self.own[..hi - nb]);
        std::borrow::Cow::Owned(v)
    }
    /// All the bytes as one buffer (copied only when both segments hold some).
    pub fn contiguous(&self) -> std::borrow::Cow<'a, [u8]> {
        if self.own.is_empty() { return std::borrow::Cow::Borrowed(self.base); }
        if self.base.is_empty() { return std::borrow::Cow::Borrowed(self.own); }
        std::borrow::Cow::Owned([self.base, self.own].concat())
    }
}

impl<T> Index<usize> for SegRef<'_, T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        match i.checked_sub(self.base.len()) {
            Some(k) => &self.own[k],
            None => &self.base[i],
        }
    }
}

/// Token sequences the parser's helpers read: a slice, or the parser's `Toks`.
pub trait TokSeq {
    fn len(&self) -> usize;
    fn at(&self, i: usize) -> Tok;
    fn get(&self, i: usize) -> Option<Tok>;
    /// Tokens [lo, hi) (None: out of range, as slice::get)
    fn slice(&self, lo: usize, hi: usize) -> Option<std::borrow::Cow<'_, [Tok]>>;
}

impl TokSeq for [Tok] {
    #[inline]
    fn len(&self) -> usize { <[Tok]>::len(self) }
    #[inline]
    fn at(&self, i: usize) -> Tok { self[i] }
    #[inline]
    fn get(&self, i: usize) -> Option<Tok> { <[Tok]>::get(self, i).copied() }
    fn slice(&self, lo: usize, hi: usize) -> Option<std::borrow::Cow<'_, [Tok]>> { <[Tok]>::get(self, lo..hi).map(std::borrow::Cow::Borrowed) }
}

impl TokSeq for Vec<Tok> {
    #[inline]
    fn len(&self) -> usize { Vec::len(self) }
    #[inline]
    fn at(&self, i: usize) -> Tok { self[i] }
    #[inline]
    fn get(&self, i: usize) -> Option<Tok> { <[Tok]>::get(self, i).copied() }
    fn slice(&self, lo: usize, hi: usize) -> Option<std::borrow::Cow<'_, [Tok]>> { <[Tok]>::get(self, lo..hi).map(std::borrow::Cow::Borrowed) }
}

impl TokSeq for PackedToks {
    #[inline]
    fn len(&self) -> usize { PackedToks::len(self) }
    #[inline]
    fn at(&self, i: usize) -> Tok { PackedToks::at(self, i) }
    #[inline]
    fn get(&self, i: usize) -> Option<Tok> { PackedToks::get(self, i) }
    fn slice(&self, lo: usize, hi: usize) -> Option<std::borrow::Cow<'_, [Tok]>> { (lo <= hi && hi <= self.len()).then(|| std::borrow::Cow::Owned(self.range(lo, hi))) }
}

/// What a parse's token array continues: nothing, tokens of a working array, or a published
/// file's packed tokens (read in place).
#[derive(Clone, Copy, Default)]
enum TokBase<'a> {
    #[default]
    None,
    Slice(&'a [Tok]),
    Packed(&'a PackedToks),
}

impl TokBase<'_> {
    #[inline]
    fn len(&self) -> usize {
        match self {
            TokBase::None => 0,
            TokBase::Slice(s) => s.len(),
            TokBase::Packed(p) => p.len(),
        }
    }
    #[inline]
    fn at(&self, i: usize) -> Tok {
        match self {
            TokBase::None => unreachable!(),
            TokBase::Slice(s) => s[i],
            TokBase::Packed(p) => p.at(i),
        }
    }
}

/// The parser's token array: the tokens of a published file (or of a working array) it
/// continues, then the tokens this parse appended, plus the base tokens this parse changed in
/// place (index -> token; `patched` marks them, one bit per base token). Tokens are read by
/// value: a packed base has no `Tok` to lend.
#[derive(Default)]
pub struct Toks<'a> {
    base: TokBase<'a>,
    own: Vec<Tok>,
    patched: Vec<u64>,
    patches: crate::fxhash::HashMap<u32, Tok>,
}

impl From<Vec<Tok>> for Toks<'_> {
    fn from(v: Vec<Tok>) -> Self { Self { own: v, ..Default::default() } }
}

impl<'a> From<&'a [Tok]> for Toks<'a> {
    fn from(base: &'a [Tok]) -> Self { Self { base: TokBase::Slice(base), ..Default::default() } }
}

impl<'a> From<&'a PackedToks> for Toks<'a> {
    fn from(base: &'a PackedToks) -> Self { Self { base: TokBase::Packed(base), ..Default::default() } }
}

impl<'a> Toks<'a> {
    #[inline]
    pub fn len(&self) -> usize { self.base.len() + self.own.len() }
    #[inline]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    #[inline]
    pub fn get(&self, i: usize) -> Option<Tok> { (i < self.len()).then(|| self.at(i)) }
    pub fn last(&self) -> Option<Tok> { self.len().checked_sub(1).map(|i| self.at(i)) }
    #[inline]
    pub fn push(&mut self, t: Tok) { self.own.push(t) }
    pub fn base_len(&self) -> usize { self.base.len() }
    /// The tokens this parse appended (the whole array when it continues nothing).
    pub fn into_own(self) -> Vec<Tok> { self.own }
    /// The whole array as one slice: only a parse that continues no published file has it.
    pub fn as_slice(&self) -> &[Tok] {
        assert!(self.base.len() == 0, "Toks::as_slice on a parse continuing a published file");
        &self.own
    }
    /// Token `i`.
    #[inline]
    pub fn at(&self, i: usize) -> Tok {
        let nb = self.base.len();
        if i >= nb {
            return self.own[i - nb];
        }
        if !self.patches.is_empty() && self.is_patched(i) {
            return self.patches[&(i as u32)];
        }
        self.base.at(i)
    }
    /// Changes token `i` in place (a base token's original stays in the base).
    #[inline]
    pub fn edit(&mut self, i: usize, f: impl FnOnce(&mut Tok)) {
        let nb = self.base.len();
        if i >= nb {
            return f(&mut self.own[i - nb]);
        }
        if self.patched.is_empty() {
            self.patched = vec![0; nb.div_ceil(64)];
        }
        self.patched[i / 64] |= 1 << (i % 64);
        let base = self.base.at(i);
        f(self.patches.entry(i as u32).or_insert(base))
    }
    /// Tokens [lo, hi) (borrowed unless they include a base token or a changed base token).
    pub fn range(&self, lo: usize, hi: usize) -> std::borrow::Cow<'_, [Tok]> {
        let nb = self.base.len();
        if lo >= nb {
            return std::borrow::Cow::Borrowed(&self.own[lo - nb..hi - nb]);
        }
        if let TokBase::Slice(s) = self.base {
            if hi <= nb && (self.patches.is_empty() || !(lo..hi).any(|i| self.is_patched(i))) {
                return std::borrow::Cow::Borrowed(&s[lo..hi]);
            }
        }
        std::borrow::Cow::Owned((lo..hi).map(|i| self.at(i)).collect())
    }
    pub fn iter(&self) -> impl Iterator<Item = Tok> + '_ { (0..self.len()).map(move |i| self.at(i)) }
    /// slice::partition_point over the tokens
    pub fn partition_point(&self, mut pred: impl FnMut(&Tok) -> bool) -> usize {
        if self.base.len() == 0 { return self.own.partition_point(pred); }
        // std's search, step for step, over the tokens as they read now
        let mut size = self.len();
        if size == 0 { return 0; }
        let mut base = 0usize;
        while size > 1 {
            let half = size / 2;
            let mid = base + half;
            if pred(&self.at(mid)) { base = mid; }
            size -= half;
        }
        base + pred(&self.at(base)) as usize
    }
    #[inline]
    fn is_patched(&self, i: usize) -> bool { self.patched.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 != 0) }
}

impl TokSeq for Toks<'_> {
    #[inline]
    fn len(&self) -> usize { Toks::len(self) }
    #[inline]
    fn at(&self, i: usize) -> Tok { Toks::at(self, i) }
    #[inline]
    fn get(&self, i: usize) -> Option<Tok> { Toks::get(self, i) }
    fn slice(&self, lo: usize, hi: usize) -> Option<std::borrow::Cow<'_, [Tok]>> { (lo <= hi && hi <= self.len()).then(|| self.range(lo, hi)) }
}

/// The parser's tokens and text as cfg evaluation reads them (cfg::Src): plain slices while
/// the parse continues no published file, else through this view.
pub struct TokView<'b> {
    src: &'b [u8],
    toks: &'b Toks<'b>,
    ext: SegRef<'b, u8>,
}

impl<'b> TokView<'b> {
    pub fn new(src: &'b [u8], toks: &'b Toks<'_>, ext: &'b Seg<'_, u8>) -> Self {
        // Toks is covariant in its lifetime: the borrow shortens it
        let toks: &'b Toks<'b> = toks;
        Self { src, toks, ext: ext.view() }
    }
    pub fn src(&self) -> crate::cfg::Src<'_> {
        if self.toks.base.len() == 0 && self.ext.base.is_empty() {
            return crate::cfg::Src { src: self.src, plain: &self.toks.own, ext: self.ext.own, parts: None };
        }
        crate::cfg::Src { src: self.src, plain: &[], ext: &[], parts: Some(self) }
    }
}

impl crate::cfg::SourceTokens for TokView<'_> {
    fn token(&self, i: u32) -> Tok { self.toks.at(i as usize) }
    /// As cfg::tok_str reads a plain view: clamped, text past the source in `ext`.
    fn bytes(&self, lo: u32, hi: u32) -> &[u8] {
        let (lo, hi, b, n) = (lo as usize, hi as usize, self.src.len(), self.ext.len());
        if lo >= b && n != 0 {
            return self.ext.slice((lo - b).min(n), (hi - b).min(n));
        }
        &self.src[lo.min(b)..hi.min(b)]
    }
}

impl ParsedFile {
    pub fn parts(&self) -> impl Iterator<Item = &FilePart> {
        std::iter::successors(self.part.get().map(|p| &**p), |p| p.next.get().map(|p| &**p))
    }

    pub fn part_at(&self, pos: u32) -> Option<&FilePart> {
        self.parts().take_while(|p| p.src_base <= pos).last()
    }

}

impl crate::cfg::SourceTokens for ParsedFile {
    fn token(&self, i: u32) -> Tok { self.tok(i) }
    fn bytes(&self, lo: u32, hi: u32) -> &[u8] { self.bytes(lo, hi) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A split array answers searches as std answers them on one contiguous copy, sorted or not.
    #[test]
    fn split_searches_match_std() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % 9) as u32 };
        for len in 0..40 {
            for sorted in [true, false] {
                let mut v: Vec<(u32, u32, u32)> = (0..len).map(|_| (next(), 0, 0)).collect();
                if sorted { v.sort(); }
                for cut in 0..=len {
                    let r = SegRef { base: &v[..cut], own: &v[cut..] };
                    for key in 0..10 {
                        assert_eq!(r.partition_point(|x| x.0 <= key), v.partition_point(|x| x.0 <= key));
                        assert_eq!(r.binary_search_by_key(&key, |x| x.0), v.binary_search_by_key(&key, |x| x.0));
                    }
                }
                let toks: Vec<Tok> = v.iter().map(|x| Tok { kind: crate::lexer::T::Ident, lo: x.0, hi: x.0 + 1 }).collect();
                for cut in 0..=len {
                    // the base is a working slice, or the file's packed tokens
                    let packed = PackedToks::encode(&toks[..cut]);
                    for use_packed in [false, true] {
                        let mut t = if use_packed { Toks::from(&packed) } else { Toks::from(&toks[..cut]) };
                        for &x in &toks[cut..] { t.push(x); }
                        let mut whole = toks.clone();
                        if cut > 1 {
                            // a changed base token (eat_gt) is read back as changed
                            t.edit(cut / 2, |x| x.lo += 3);
                            whole[cut / 2].lo += 3;
                        }
                        for key in 0..12 {
                            assert_eq!(t.partition_point(|x| x.lo <= key), whole.partition_point(|x| x.lo <= key));
                        }
                        assert_eq!(&t.range(0, len)[..], &whole[..]);
                        assert_eq!(t.into_own(), toks[cut..].to_vec());
                    }
                }
            }
        }
    }
}
