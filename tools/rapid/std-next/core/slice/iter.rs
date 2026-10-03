//! Slice iterators (general path; Rapid fuses `for x in s.iter()` chains into loops).

use crate::fmt;
use crate::iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, Iterator};

fn take_mut<'a, T>(s: &mut &'a mut [T]) -> &'a mut [T] {
    crate::mem::replace(s, &mut [])
}

// ---------------------------------------------------------------- Iter / IterMut

pub struct Iter<'a, T> {
    s: &'a [T],
}

pub fn iter<T>(s: &[T]) -> Iter<'_, T> {
    Iter { s }
}

impl<'a, T> Iter<'a, T> {
    pub(crate) fn new(s: &'a [T]) -> Iter<'a, T> {
        Iter { s }
    }
    pub fn as_slice(&self) -> &'a [T] {
        self.s
    }
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter { s: self.s }
    }
}

impl<'a, T> Default for Iter<'a, T> {
    fn default() -> Iter<'a, T> {
        Iter { s: &[] }
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for Iter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Iter").field(&self.s).finish()
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        match self.s.split_first() {
            Some((first, rest)) => {
                self.s = rest;
                Some(first)
            }
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.s.len(), Some(self.s.len()))
    }
    fn count(self) -> usize {
        self.s.len()
    }
    fn nth(&mut self, n: usize) -> Option<&'a T> {
        if n >= self.s.len() {
            self.s = &[];
            None
        } else {
            let r = &self.s[n];
            self.s = &self.s[n + 1..];
            Some(r)
        }
    }
    fn last(self) -> Option<&'a T> {
        self.s.last()
    }
    fn position<P: FnMut(&'a T) -> bool>(&mut self, predicate: P) -> Option<usize> {
        let mut predicate = predicate;
        let mut i = 0;
        while let Some(x) = self.next() {
            if predicate(x) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        match self.s.split_last() {
            Some((last, rest)) => {
                self.s = rest;
                Some(last)
            }
            None => None,
        }
    }
    fn nth_back(&mut self, n: usize) -> Option<&'a T> {
        let len = self.s.len();
        if n >= len {
            self.s = &[];
            None
        } else {
            let r = &self.s[len - 1 - n];
            self.s = &self.s[..len - 1 - n];
            Some(r)
        }
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}
impl<'a, T> FusedIterator for Iter<'a, T> {}

pub struct IterMut<'a, T> {
    s: &'a mut [T],
}

pub fn iter_mut<T>(s: &mut [T]) -> IterMut<'_, T> {
    IterMut { s }
}

impl<'a, T> IterMut<'a, T> {
    pub(crate) fn new(s: &'a mut [T]) -> IterMut<'a, T> {
        IterMut { s }
    }
    pub fn into_slice(self) -> &'a mut [T] {
        self.s
    }
    pub fn as_slice(&self) -> &[T] {
        self.s
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for IterMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IterMut").field(&self.as_slice()).finish()
    }
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        let s = take_mut(&mut self.s);
        match s.split_first_mut() {
            Some((first, rest)) => {
                self.s = rest;
                Some(first)
            }
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.s.len(), Some(self.s.len()))
    }
    fn count(self) -> usize {
        self.s.len()
    }
    fn nth(&mut self, n: usize) -> Option<&'a mut T> {
        let s = take_mut(&mut self.s);
        if n >= s.len() {
            None
        } else {
            let (_, rest) = s.split_at_mut(n);
            let (x, rest) = match rest.split_first_mut() {
                Some(p) => p,
                None => return None,
            };
            self.s = rest;
            Some(x)
        }
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    fn next_back(&mut self) -> Option<&'a mut T> {
        let s = take_mut(&mut self.s);
        match s.split_last_mut() {
            Some((last, rest)) => {
                self.s = rest;
                Some(last)
            }
            None => None,
        }
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {}
impl<'a, T> FusedIterator for IterMut<'a, T> {}

// ---------------------------------------------------------------- Windows / Chunks

#[derive(Debug)]
pub struct Windows<'a, T> {
    v: &'a [T],
    size: usize,
}

pub fn windows<T>(v: &[T], size: usize) -> Windows<'_, T> {
    if size == 0 {
        crate::panicking::panic_str("window size must be non-zero");
    }
    Windows { v, size }
}

impl<'a, T> Clone for Windows<'a, T> {
    fn clone(&self) -> Windows<'a, T> {
        Windows { v: self.v, size: self.size }
    }
}

impl<'a, T> Iterator for Windows<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.size > self.v.len() {
            None
        } else {
            let r = &self.v[..self.size];
            self.v = &self.v[1..];
            Some(r)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.size > self.v.len() { 0 } else { self.v.len() - self.size + 1 };
        (n, Some(n))
    }
    fn nth(&mut self, n: usize) -> Option<&'a [T]> {
        let end = self.size.checked_add(n);
        match end {
            Some(end) if end <= self.v.len() => {
                let r = &self.v[n..end];
                self.v = &self.v[n + 1..];
                Some(r)
            }
            _ => {
                self.v = &[];
                None
            }
        }
    }
}

impl<'a, T> DoubleEndedIterator for Windows<'a, T> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.size > self.v.len() {
            None
        } else {
            let len = self.v.len();
            let r = &self.v[len - self.size..];
            self.v = &self.v[..len - 1];
            Some(r)
        }
    }
}

impl<'a, T> ExactSizeIterator for Windows<'a, T> {}
impl<'a, T> FusedIterator for Windows<'a, T> {}

#[derive(Debug)]
pub struct Chunks<'a, T> {
    v: &'a [T],
    chunk_size: usize,
}

pub fn chunks<T>(v: &[T], chunk_size: usize) -> Chunks<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    Chunks { v, chunk_size }
}

impl<'a, T> Clone for Chunks<'a, T> {
    fn clone(&self) -> Chunks<'a, T> {
        Chunks { v: self.v, chunk_size: self.chunk_size }
    }
}

impl<'a, T> Iterator for Chunks<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.v.is_empty() {
            None
        } else {
            let sz = if self.v.len() < self.chunk_size { self.v.len() } else { self.chunk_size };
            let (fst, snd) = self.v.split_at(sz);
            self.v = snd;
            Some(fst)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.v.is_empty() { 0 } else { (self.v.len() + self.chunk_size - 1) / self.chunk_size };
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for Chunks<'a, T> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.v.is_empty() {
            None
        } else {
            let remainder = self.v.len() % self.chunk_size;
            let chunksz = if remainder != 0 { remainder } else { self.chunk_size };
            let (fst, snd) = self.v.split_at(self.v.len() - chunksz);
            self.v = fst;
            Some(snd)
        }
    }
}

impl<'a, T> ExactSizeIterator for Chunks<'a, T> {}
impl<'a, T> FusedIterator for Chunks<'a, T> {}

#[derive(Debug)]
pub struct ChunksMut<'a, T> {
    v: &'a mut [T],
    chunk_size: usize,
}

pub fn chunks_mut<T>(v: &mut [T], chunk_size: usize) -> ChunksMut<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    ChunksMut { v, chunk_size }
}

impl<'a, T> Iterator for ChunksMut<'a, T> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<&'a mut [T]> {
        if self.v.is_empty() {
            None
        } else {
            let sz = if self.v.len() < self.chunk_size { self.v.len() } else { self.chunk_size };
            let v = take_mut(&mut self.v);
            let (head, tail) = v.split_at_mut(sz);
            self.v = tail;
            Some(head)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.v.is_empty() { 0 } else { (self.v.len() + self.chunk_size - 1) / self.chunk_size };
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for ChunksMut<'a, T> {
    fn next_back(&mut self) -> Option<&'a mut [T]> {
        if self.v.is_empty() {
            None
        } else {
            let remainder = self.v.len() % self.chunk_size;
            let sz = if remainder != 0 { remainder } else { self.chunk_size };
            let v = take_mut(&mut self.v);
            let len = v.len();
            let (head, tail) = v.split_at_mut(len - sz);
            self.v = head;
            Some(tail)
        }
    }
}

impl<'a, T> ExactSizeIterator for ChunksMut<'a, T> {}

#[derive(Debug)]
pub struct ChunksExact<'a, T> {
    v: &'a [T],
    rem: &'a [T],
    chunk_size: usize,
}

pub fn chunks_exact<T>(v: &[T], chunk_size: usize) -> ChunksExact<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let rem_len = v.len() % chunk_size;
    let fst_len = v.len() - rem_len;
    let (fst, snd) = v.split_at(fst_len);
    ChunksExact { v: fst, rem: snd, chunk_size }
}

impl<'a, T> ChunksExact<'a, T> {
    pub fn remainder(&self) -> &'a [T] {
        self.rem
    }
}

impl<'a, T> Clone for ChunksExact<'a, T> {
    fn clone(&self) -> ChunksExact<'a, T> {
        ChunksExact { v: self.v, rem: self.rem, chunk_size: self.chunk_size }
    }
}

impl<'a, T> Iterator for ChunksExact<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.v.len() < self.chunk_size {
            None
        } else {
            let (fst, snd) = self.v.split_at(self.chunk_size);
            self.v = snd;
            Some(fst)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size;
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for ChunksExact<'a, T> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.v.len() < self.chunk_size {
            None
        } else {
            let (fst, snd) = self.v.split_at(self.v.len() - self.chunk_size);
            self.v = fst;
            Some(snd)
        }
    }
}

impl<'a, T> ExactSizeIterator for ChunksExact<'a, T> {}
impl<'a, T> FusedIterator for ChunksExact<'a, T> {}

#[derive(Debug)]
pub struct ChunksExactMut<'a, T> {
    v: &'a mut [T],
    rem: &'a mut [T],
    chunk_size: usize,
}

pub fn chunks_exact_mut<T>(v: &mut [T], chunk_size: usize) -> ChunksExactMut<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let rem_len = v.len() % chunk_size;
    let fst_len = v.len() - rem_len;
    let (fst, snd) = v.split_at_mut(fst_len);
    ChunksExactMut { v: fst, rem: snd, chunk_size }
}

impl<'a, T> ChunksExactMut<'a, T> {
    pub fn into_remainder(self) -> &'a mut [T] {
        self.rem
    }
}

impl<'a, T> Iterator for ChunksExactMut<'a, T> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<&'a mut [T]> {
        if self.v.len() < self.chunk_size {
            None
        } else {
            let v = take_mut(&mut self.v);
            let (head, tail) = v.split_at_mut(self.chunk_size);
            self.v = tail;
            Some(head)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size;
        (n, Some(n))
    }
}

impl<'a, T> ExactSizeIterator for ChunksExactMut<'a, T> {}

#[derive(Debug)]
pub struct RChunks<'a, T> {
    v: &'a [T],
    chunk_size: usize,
}

pub fn rchunks<T>(v: &[T], chunk_size: usize) -> RChunks<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    RChunks { v, chunk_size }
}

impl<'a, T> Iterator for RChunks<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.v.is_empty() {
            None
        } else {
            let len = self.v.len();
            let sz = if len < self.chunk_size { len } else { self.chunk_size };
            let (fst, snd) = self.v.split_at(len - sz);
            self.v = fst;
            Some(snd)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.v.is_empty() { 0 } else { (self.v.len() + self.chunk_size - 1) / self.chunk_size };
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for RChunks<'a, T> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.v.is_empty() {
            None
        } else {
            let remainder = self.v.len() % self.chunk_size;
            let sz = if remainder != 0 { remainder } else { self.chunk_size };
            let (fst, snd) = self.v.split_at(sz);
            self.v = snd;
            Some(fst)
        }
    }
}

impl<'a, T> ExactSizeIterator for RChunks<'a, T> {}

// ---------------------------------------------------------------- Split family

pub struct Split<'a, T, P> {
    v: &'a [T],
    pred: P,
    finished: bool,
}

pub fn split<T, P: FnMut(&T) -> bool>(v: &[T], pred: P) -> Split<'_, T, P> {
    Split { v, pred, finished: false }
}

impl<'a, T, P: FnMut(&T) -> bool> Split<'a, T, P> {
    fn finish(&mut self) -> Option<&'a [T]> {
        if self.finished {
            None
        } else {
            self.finished = true;
            Some(self.v)
        }
    }
    pub fn as_slice(&self) -> &'a [T] {
        if self.finished {
            &[]
        } else {
            self.v
        }
    }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for Split<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                let ret = &self.v[..i];
                self.v = &self.v[i + 1..];
                return Some(ret);
            }
            i += 1;
        }
        self.finish()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.finished {
            (0, Some(0))
        } else {
            (1, Some(self.v.len() + 1))
        }
    }
}

impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for Split<'a, T, P> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        let mut i = self.v.len();
        while i > 0 {
            if (self.pred)(&self.v[i - 1]) {
                let ret = &self.v[i..];
                self.v = &self.v[..i - 1];
                return Some(ret);
            }
            i -= 1;
        }
        self.finish()
    }
}

impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for Split<'a, T, P> {}

pub struct SplitInclusive<'a, T, P> {
    v: &'a [T],
    pred: P,
    finished: bool,
}

pub fn split_inclusive<T, P: FnMut(&T) -> bool>(v: &[T], pred: P) -> SplitInclusive<'_, T, P> {
    let finished = v.is_empty();
    SplitInclusive { v, pred, finished }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitInclusive<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        let mut idx = self.v.len();
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                idx = i + 1;
                break;
            }
            i += 1;
        }
        if idx == self.v.len() {
            self.finished = true;
        }
        let ret = &self.v[..idx];
        self.v = &self.v[idx..];
        Some(ret)
    }
}

impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for SplitInclusive<'a, T, P> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        // the last element is never a split point going backwards
        let remainder = if self.v.is_empty() { &[][..] } else { &self.v[..self.v.len() - 1] };
        let mut idx = 0;
        let mut i = remainder.len();
        while i > 0 {
            if (self.pred)(&remainder[i - 1]) {
                idx = i;
                break;
            }
            i -= 1;
        }
        if idx == 0 {
            self.finished = true;
        }
        let ret = &self.v[idx..];
        self.v = &self.v[..idx];
        Some(ret)
    }
}

pub struct SplitMut<'a, T, P> {
    v: &'a mut [T],
    pred: P,
    finished: bool,
}

pub fn split_mut<T, P: FnMut(&T) -> bool>(v: &mut [T], pred: P) -> SplitMut<'_, T, P> {
    SplitMut { v, pred, finished: false }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<&'a mut [T]> {
        if self.finished {
            return None;
        }
        let mut found = None;
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                found = Some(i);
                break;
            }
            i += 1;
        }
        let v = take_mut(&mut self.v);
        match found {
            None => {
                self.finished = true;
                Some(v)
            }
            Some(idx) => {
                let (head, tail) = v.split_at_mut(idx);
                self.v = &mut tail[1..];
                Some(head)
            }
        }
    }
}

/// splitn / rsplitn: at most `count` pieces.
pub struct SplitN<'a, T, P> {
    inner: Split<'a, T, P>,
    count: usize,
}

pub fn splitn<T, P: FnMut(&T) -> bool>(v: &[T], n: usize, pred: P) -> SplitN<'_, T, P> {
    SplitN { inner: split(v, pred), count: n }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitN<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        match self.count {
            0 => None,
            1 => {
                self.count -= 1;
                self.inner.finish()
            }
            _ => {
                self.count -= 1;
                self.inner.next()
            }
        }
    }
}

pub struct RSplit<'a, T, P> {
    inner: Split<'a, T, P>,
}

pub fn rsplit<T, P: FnMut(&T) -> bool>(v: &[T], pred: P) -> RSplit<'_, T, P> {
    RSplit { inner: split(v, pred) }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for RSplit<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        self.inner.next_back()
    }
}

impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for RSplit<'a, T, P> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        self.inner.next()
    }
}

pub struct RSplitN<'a, T, P> {
    inner: Split<'a, T, P>,
    count: usize,
}

pub fn rsplitn<T, P: FnMut(&T) -> bool>(v: &[T], n: usize, pred: P) -> RSplitN<'_, T, P> {
    RSplitN { inner: split(v, pred), count: n }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for RSplitN<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        match self.count {
            0 => None,
            1 => {
                self.count -= 1;
                self.inner.finish()
            }
            _ => {
                self.count -= 1;
                self.inner.next_back()
            }
        }
    }
}

#[cfg(not(rapid_check))]
impl<'a, T> crate::iter::IntoIterator for &'a [T] {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        Iter::new(self)
    }
}

#[cfg(not(rapid_check))]
impl<'a, T> crate::iter::IntoIterator for &'a mut [T] {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> IterMut<'a, T> {
        IterMut::new(self)
    }
}
