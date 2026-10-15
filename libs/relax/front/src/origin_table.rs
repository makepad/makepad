//! Position side tables of a parsed file (macro expansion regions, fragment and macro-body
//! origins, token provenance) in one compact encoding.
//!
//! A table is a list of rows sorted by a `u32` key (the row's position). Rows are stored in
//! blocks of 64: a fixed-width block index (first key, byte offset) for the binary search, then
//! per row the key (delta from the previous row; the first row of a block takes it from the
//! index) and the other fields relative to the previous row, as LEB128 varints (one byte for
//! the common small value). The parser works on plain `Vec`s of rows while it appends (it
//! reads them at random); a finished table is encoded once and the same bytes are what the
//! image stores and maps. Cold (just encoded) and warm (mapped) tables are the same type.
//!
//! Layout (little endian): `n: u32`, `flags: u32` (bit 0: keys non-decreasing, bit 1: keys
//! strictly increasing), `ceil(n / 64)` index entries of `(first key: u32, offset: u32)`, then
//! the row stream. An empty table has no bytes at all. Searches by key are exact for sorted
//! tables; a table that is not sorted answers every search the way `slice::partition_point` /
//! `binary_search_by_key` would over the plain rows (step for step, by random access).
use crate::arena::Arena;
use crate::source::{SourceSpan, TokenOrigin};
use std::cmp::Ordering;
use std::marker::PhantomData;

pub const BLOCK: usize = 64;
const HEAD: usize = 8;
const SORTED: u32 = 1;
const STRICT: u32 = 2;

#[inline]
fn zz(v: u32) -> u64 { (((v as i32) << 1) ^ ((v as i32) >> 31)) as u32 as u64 }
#[inline]
fn unzz(v: u64) -> u32 { let v = v as u32; (v >> 1) ^ (v & 1).wrapping_neg() }

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A cursor over a row stream. Reads are bounds checked: a damaged table panics, it does not
/// read out of bounds.
pub struct Cur<'a> { b: &'a [u8], i: usize }

impl Cur<'_> {
    #[inline]
    fn varint(&mut self) -> u64 {
        let b = self.b[self.i];
        self.i += 1;
        if b < 0x80 { return b as u64; }
        self.varint_more(b)
    }
    #[cold]
    fn varint_more(&mut self, first: u8) -> u64 {
        let mut v = (first & 0x7f) as u64;
        let mut shift = 7;
        loop {
            let b = self.b[self.i];
            self.i += 1;
            v |= ((b & 0x7f) as u64) << shift;
            if b < 0x80 { return v; }
            shift += 7;
            assert!(shift < 64, "damaged position table");
        }
    }
    #[inline]
    fn u32(&mut self) -> u32 { self.varint() as u32 }
}

/// The row shape of one kind of table.
pub trait Kind: 'static {
    type Item: Copy + PartialEq + std::fmt::Debug + Default + Send + Sync + 'static;
    fn key(row: &Self::Item) -> u32;
    /// The row relative to the previous one (a zero row at a block start, where `first` is
    /// set and the key comes from the block index). `sorted`: the table's keys never decrease.
    fn put(prev: &Self::Item, cur: &Self::Item, first: bool, sorted: bool, out: &mut Vec<u8>);
    fn get(prev: &Self::Item, first_key: Option<u32>, sorted: bool, r: &mut Cur) -> Self::Item;
}

#[inline]
fn put_key(prev: u32, key: u32, sorted: bool, out: &mut Vec<u8>) {
    put_varint(out, if sorted { key.wrapping_sub(prev) as u64 } else { zz(key.wrapping_sub(prev)) });
}

#[inline]
fn get_key(prev: u32, sorted: bool, r: &mut Cur) -> u32 {
    prev.wrapping_add(if sorted { r.varint() as u32 } else { unzz(r.varint()) })
}

/// `(lo, hi, call)`-shaped rows keyed by `lo` (`exp_map`: an expansion's text range and its call
/// site; `rules_exp`: a macro_rules expansion's range and its definition position).
pub struct Regions;
/// `(position, origin, length)` rows keyed by position (`tok_origin`, `body_origin`).
pub struct Maps;
/// A token's whole authored span (`token_origins`), keyed by `lo`.
pub struct Origins;

impl Kind for Regions {
    type Item = (u32, u32, u32);
    #[inline]
    fn key(r: &Self::Item) -> u32 { r.0 }
    fn put(prev: &Self::Item, c: &Self::Item, first: bool, sorted: bool, out: &mut Vec<u8>) {
        if !first { put_key(prev.0, c.0, sorted, out); }
        put_varint(out, zz(c.1.wrapping_sub(c.0)));
        put_varint(out, zz(c.2.wrapping_sub(prev.2)));
    }
    #[inline]
    fn get(prev: &Self::Item, first_key: Option<u32>, sorted: bool, r: &mut Cur) -> Self::Item {
        let key = match first_key { Some(k) => k, None => get_key(prev.0, sorted, r) };
        let hi = key.wrapping_add(unzz(r.varint()));
        let call = prev.2.wrapping_add(unzz(r.varint()));
        (key, hi, call)
    }
}

impl Kind for Maps {
    type Item = (u32, u32, u32);
    #[inline]
    fn key(r: &Self::Item) -> u32 { r.0 }
    fn put(prev: &Self::Item, c: &Self::Item, first: bool, sorted: bool, out: &mut Vec<u8>) {
        if !first { put_key(prev.0, c.0, sorted, out); }
        put_varint(out, zz(c.1.wrapping_sub(prev.1)));
        put_varint(out, c.2 as u64);
    }
    #[inline]
    fn get(prev: &Self::Item, first_key: Option<u32>, sorted: bool, r: &mut Cur) -> Self::Item {
        let key = match first_key { Some(k) => k, None => get_key(prev.0, sorted, r) };
        let origin = prev.1.wrapping_add(unzz(r.varint()));
        (key, origin, r.u32())
    }
}

impl Kind for Origins {
    type Item = TokenOrigin;
    #[inline]
    fn key(r: &Self::Item) -> u32 { r.lo }
    /// head = len << 4 | adjacent (lo is the previous row's hi) << 3 | span length equals the
    /// token's << 2 | same authored span as the previous row << 1 | file changed; then, only
    /// when set: the gap, the file, the span's start delta, the span length difference.
    fn put(prev: &Self::Item, c: &Self::Item, first: bool, _sorted: bool, out: &mut Vec<u8>) {
        let len = c.hi.wrapping_sub(c.lo);
        let gap = c.lo.wrapping_sub(prev.hi);
        let adjacent = first || gap == 0;
        let same = c.source == prev.source;
        let slen = c.source.hi.wrapping_sub(c.source.lo);
        let flat = !same && slen == len;
        let file = !same && c.source.file != prev.source.file;
        put_varint(out, (len as u64) << 4 | (adjacent as u64) << 3 | (flat as u64) << 2 | (same as u64) << 1 | file as u64);
        if !adjacent { put_varint(out, zz(gap)); }
        if file { put_varint(out, c.source.file as u64); }
        if !same {
            put_varint(out, zz(c.source.lo.wrapping_sub(prev.source.lo)));
            if !flat { put_varint(out, zz(slen.wrapping_sub(len))); }
        }
    }
    #[inline]
    fn get(prev: &Self::Item, first_key: Option<u32>, _sorted: bool, r: &mut Cur) -> Self::Item {
        let head = r.varint();
        let len = (head >> 4) as u32;
        let lo = match first_key {
            Some(k) => k,
            None if head & 8 != 0 => prev.hi,
            None => prev.hi.wrapping_add(unzz(r.varint())),
        };
        if head & 2 != 0 {
            return TokenOrigin { lo, hi: lo.wrapping_add(len), source: prev.source };
        }
        let file = if head & 1 != 0 { r.u32() } else { prev.source.file };
        let slo = prev.source.lo.wrapping_add(unzz(r.varint()));
        let slen = if head & 4 != 0 { len } else { len.wrapping_add(unzz(r.varint())) };
        TokenOrigin { lo, hi: lo.wrapping_add(len), source: SourceSpan { file, lo: slo, hi: slo.wrapping_add(slen) } }
    }
}

pub struct Table<K: Kind> {
    bytes: Arena<u8>,
    _kind: PhantomData<fn() -> K>,
}

impl<K: Kind> Default for Table<K> {
    fn default() -> Self { Self { bytes: Arena::Owned(Vec::new()), _kind: PhantomData } }
}

impl<K: Kind> Clone for Table<K> {
    fn clone(&self) -> Self { Self { bytes: self.bytes.clone(), _kind: PhantomData } }
}

impl<K: Kind> PartialEq for Table<K> {
    /// The encoding is a function of the rows.
    fn eq(&self, other: &Self) -> bool { self.bytes[..] == other.bytes[..] }
}

impl<K: Kind> std::fmt::Debug for Table<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_list().entries(self.iter()).finish() }
}

impl<K: Kind> Table<K> {
    /// Encodes `rows`, exactly sized.
    pub fn from_rows(rows: &[K::Item]) -> Self {
        if rows.is_empty() { return Self::default(); }
        let n = rows.len();
        let sorted = rows.windows(2).all(|w| K::key(&w[0]) <= K::key(&w[1]));
        let strict = rows.windows(2).all(|w| K::key(&w[0]) < K::key(&w[1]));
        let flags = if sorted { SORTED } else { 0 } | if strict { STRICT } else { 0 };
        let nb = n.div_ceil(BLOCK);
        let mut index: Vec<(u32, u32)> = Vec::with_capacity(nb);
        let mut stream: Vec<u8> = Vec::with_capacity(n * 3);
        let mut prev = K::Item::default();
        for (i, row) in rows.iter().enumerate() {
            let key = K::key(row);
            let first = i % BLOCK == 0;
            if first {
                index.push((key, stream.len() as u32));
                prev = K::Item::default();
            }
            K::put(&prev, row, first, sorted, &mut stream);
            prev = *row;
        }
        let mut out = Vec::with_capacity(HEAD + nb * 8 + stream.len());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        for (k, o) in index {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&stream);
        Self { bytes: Arena::Owned(out), _kind: PhantomData }
    }

    /// A table stored in a mapped image (checked for the shape the readers rely on).
    pub fn mapped(bytes: &'static [u8]) -> Option<Self> {
        if !bytes.is_empty() {
            if bytes.len() < HEAD { return None; }
            let n = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
            let nb = n.div_ceil(BLOCK);
            if n == 0 || bytes.len() < HEAD + nb * 8 { return None; }
            let stream = bytes.len() - HEAD - nb * 8;
            let last = u32::from_le_bytes(bytes[HEAD + (nb - 1) * 8 + 4..HEAD + nb * 8].try_into().ok()?) as usize;
            if last > stream { return None; }
        }
        Some(Self { bytes: Arena::Mapped(bytes), _kind: PhantomData })
    }

    pub fn bytes(&self) -> &[u8] { &self.bytes }
    pub fn heap_bytes(&self) -> usize { self.bytes.heap_bytes() }
    pub fn is_mapped(&self) -> bool { self.bytes.is_mapped() }
    pub fn shrink_to_fit(&mut self) { self.bytes.shrink_to_fit() }

    #[inline]
    pub fn len(&self) -> usize {
        match self.bytes.get(0..4) {
            Some(b) => u32::from_le_bytes(b.try_into().unwrap()) as usize,
            None => 0,
        }
    }
    #[inline]
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }
    #[inline]
    fn flags(&self) -> u32 { if self.bytes.is_empty() { SORTED | STRICT } else { u32::from_le_bytes(self.bytes[4..8].try_into().unwrap()) } }
    /// Keys never decrease (an empty table is sorted).
    pub fn sorted(&self) -> bool { self.flags() & SORTED != 0 }
    /// Keys strictly increase.
    pub fn strict(&self) -> bool { self.flags() & STRICT != 0 }
    #[inline]
    fn blocks(&self) -> usize { self.len().div_ceil(BLOCK) }
    #[inline]
    fn index(&self, b: usize) -> (u32, usize) {
        let at = HEAD + b * 8;
        (u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap()),
         u32::from_le_bytes(self.bytes[at + 4..at + 8].try_into().unwrap()) as usize)
    }
    #[inline]
    fn stream_at(&self, off: usize) -> usize { HEAD + self.blocks() * 8 + off }

    pub fn iter(&self) -> Iter<'_, K> { self.iter_from(0) }

    /// The rows from row `i` on.
    pub fn iter_from(&self, i: usize) -> Iter<'_, K> {
        let mut it = Iter { t: self, next: 0, n: self.len(), sorted: self.sorted(), cur: Cur { b: &self.bytes, i: 0 }, prev: K::Item::default() };
        if i >= it.n { it.next = it.n; return it; }
        let b = i / BLOCK;
        it.next = b * BLOCK;
        it.cur.i = self.stream_at(self.index(b).1);
        for _ in it.next..i { it.next(); }
        it
    }

    /// The rows from the last to the first.
    pub fn iter_rev(&self) -> RevIter<'_, K> {
        RevIter { t: self, block: self.blocks(), buf: Vec::new() }
    }

    pub fn get(&self, i: usize) -> Option<K::Item> {
        if i >= self.len() { return None; }
        self.iter_from(i).next()
    }

    pub fn last(&self) -> Option<K::Item> {
        let n = self.len();
        if n == 0 { None } else { self.iter_from(n - 1).next() }
    }

    /// The last row whose key is at most `key` and its index. For a sorted table (callers check
    /// `sorted()`): the block search finds the block, a scan of at most 64 rows the row.
    pub fn floor(&self, key: u32) -> Option<(usize, K::Item)> {
        let nb = self.blocks();
        let (mut lo, mut hi) = (0, nb);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.index(mid).0 <= key { lo = mid + 1 } else { hi = mid }
        }
        let b = lo.checked_sub(1)?;
        let mut it = self.iter_from(b * BLOCK);
        let end = ((b + 1) * BLOCK).min(self.len());
        let mut found = None;
        for i in b * BLOCK..end {
            let row = it.next().unwrap();
            if K::key(&row) > key { break; }
            found = Some((i, row));
        }
        found
    }
}

impl<K: Kind> Table<K> {
    /// How many rows have a key of at most `key` (a sorted table).
    pub fn count_le(&self, key: u32) -> usize { self.floor(key).map_or(0, |(i, _)| i + 1) }
}

impl Table<Origins> {
    /// The table with every non-zero source file id replaced.
    pub fn remap_files(&self, mut f: impl FnMut(u32) -> u32) -> Self {
        let rows: Vec<TokenOrigin> = self.iter().map(|mut o| { if o.source.file != 0 { o.source.file = f(o.source.file); } o }).collect();
        Self::from_rows(&rows)
    }
}

pub struct Iter<'a, K: Kind> {
    t: &'a Table<K>,
    next: usize,
    n: usize,
    sorted: bool,
    cur: Cur<'a>,
    prev: K::Item,
}

impl<K: Kind> Iterator for Iter<'_, K> {
    type Item = K::Item;
    #[inline]
    fn next(&mut self) -> Option<K::Item> {
        let i = self.next;
        if i >= self.n { return None; }
        let first_key = if i % BLOCK == 0 {
            self.prev = K::Item::default();
            Some(self.t.index(i / BLOCK).0)
        } else {
            None
        };
        let row = K::get(&self.prev, first_key, self.sorted, &mut self.cur);
        self.prev = row;
        self.next = i + 1;
        Some(row)
    }
    fn size_hint(&self) -> (usize, Option<usize>) { let r = self.n - self.next; (r, Some(r)) }
}

pub struct RevIter<'a, K: Kind> {
    t: &'a Table<K>,
    /// blocks before this one are not loaded yet
    block: usize,
    /// the loaded block's remaining rows, last first at the end
    buf: Vec<K::Item>,
}

impl<K: Kind> Iterator for RevIter<'_, K> {
    type Item = K::Item;
    fn next(&mut self) -> Option<K::Item> {
        if self.buf.is_empty() {
            let b = self.block.checked_sub(1)?;
            self.block = b;
            let end = ((b + 1) * BLOCK).min(self.t.len());
            self.buf.extend(self.t.iter_from(b * BLOCK).take(end - b * BLOCK));
        }
        self.buf.pop()
    }
}

/// A parser's table: a published table (a file's, read in place) followed by the rows this
/// parse appends. Reads see exactly the rows of one contiguous table.
pub struct OSeg<'a, K: Kind> {
    base: Option<&'a Table<K>>,
    own: Vec<K::Item>,
    last: Option<u32>,
    sorted: bool,
    strict: bool,
}

impl<K: Kind> Default for OSeg<'_, K> {
    fn default() -> Self { Self { base: None, own: Vec::new(), last: None, sorted: true, strict: true } }
}

impl<K: Kind> From<Vec<K::Item>> for OSeg<'_, K> {
    fn from(own: Vec<K::Item>) -> Self {
        let mut s = Self::default();
        for row in own { s.push(row); }
        s
    }
}

impl<'a, K: Kind> OSeg<'a, K> {
    /// Continues `base`: pushes append past its end.
    pub fn on(base: &'a Table<K>) -> Self {
        Self { base: Some(base), own: Vec::new(), last: base.last().map(|r| K::key(&r)), sorted: base.sorted(), strict: base.strict() }
    }
    #[inline]
    pub fn len(&self) -> usize { self.base.map_or(0, |b| b.len()) + self.own.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn push(&mut self, row: K::Item) {
        let key = K::key(&row);
        if let Some(last) = self.last {
            self.sorted &= last <= key;
            self.strict &= last < key;
        }
        self.last = Some(key);
        self.own.push(row);
    }
    pub fn clear(&mut self) { *self = Self::default(); }
    /// What this parse appended (the whole table when it continues nothing).
    pub fn own(&self) -> &[K::Item] { &self.own }
    pub fn into_own(self) -> Vec<K::Item> { self.own }
    pub fn view(&self) -> OSegRef<'_, K> { OSegRef { base: self.base, own: &self.own, sorted: self.sorted, strict: self.strict } }
    pub fn get(&self, i: usize) -> Option<K::Item> { self.view().get(i) }
    pub fn last(&self) -> Option<K::Item> { self.own.last().copied().or_else(|| self.base.and_then(|b| b.last())) }
    pub fn iter(&self) -> impl Iterator<Item = K::Item> + '_ { self.view().iter() }
    pub fn iter_rev(&self) -> impl Iterator<Item = K::Item> + '_ { self.view().iter_rev() }
    pub fn floor(&self, key: u32) -> Option<(usize, K::Item)> { self.view().floor(key) }
}

impl<K: Kind> Extend<K::Item> for OSeg<'_, K> {
    fn extend<I: IntoIterator<Item = K::Item>>(&mut self, iter: I) { for row in iter { self.push(row); } }
}

/// A borrowed `OSeg`, or a plain table.
pub struct OSegRef<'a, K: Kind> {
    base: Option<&'a Table<K>>,
    own: &'a [K::Item],
    sorted: bool,
    strict: bool,
}

impl<K: Kind> Clone for OSegRef<'_, K> {
    fn clone(&self) -> Self { *self }
}
impl<K: Kind> Copy for OSegRef<'_, K> {}
impl<K: Kind> Default for OSegRef<'_, K> {
    fn default() -> Self { Self { base: None, own: &[], sorted: true, strict: true } }
}

impl<'a, K: Kind> From<&'a Table<K>> for OSegRef<'a, K> {
    fn from(t: &'a Table<K>) -> Self { Self { base: Some(t), own: &[], sorted: t.sorted(), strict: t.strict() } }
}

impl<'a, K: Kind> OSegRef<'a, K> {
    fn base_len(&self) -> usize { self.base.map_or(0, |b| b.len()) }
    pub fn len(&self) -> usize { self.base_len() + self.own.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn get(&self, i: usize) -> Option<K::Item> {
        match i.checked_sub(self.base_len()) {
            Some(k) => self.own.get(k).copied(),
            None => self.base.and_then(|b| b.get(i)),
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = K::Item> + 'a {
        self.base.into_iter().flat_map(|b| b.iter()).chain(self.own.iter().copied())
    }
    pub fn iter_rev(&self) -> impl Iterator<Item = K::Item> + 'a {
        self.own.iter().rev().copied().chain(self.base.into_iter().flat_map(|b| b.iter_rev()))
    }

    /// The last row whose key is at most `key`, and its index: what
    /// `partition_point(|x| key(x) <= key)` finds, over the table and the appended rows as one array.
    pub fn floor(&self, key: u32) -> Option<(usize, K::Item)> {
        if !self.sorted {
            let i = partition_at(self.len(), |i| K::key(&self.get(i).unwrap()), |k| k <= key);
            return i.checked_sub(1).map(|i| (i, self.get(i).unwrap()));
        }
        let nb = self.base_len();
        if self.own.is_empty() || (nb > 0 && K::key(&self.own[0]) > key) {
            return self.base.and_then(|b| b.floor(key));
        }
        let i = self.own.partition_point(|x| K::key(x) <= key);
        i.checked_sub(1).map(|i| (nb + i, self.own[i]))
    }

    /// The row `slice::binary_search_by_key(&key, ..)` lands on, as the origin lookups use it:
    /// the matching row, else the one before the insertion point.
    pub fn floor_exact(&self, key: u32) -> Option<(usize, K::Item)> {
        if self.strict { return self.floor(key); }
        let r = binary_search_at(self.len(), |i| K::key(&self.get(i).unwrap()).cmp(&key));
        let i = match r { Ok(i) => i, Err(i) => i.checked_sub(1)? };
        Some((i, self.get(i).unwrap()))
    }
}

/// slice::binary_search_by over `0..len`, step for step as std does it (an array that is not
/// sorted answers as the std search would).
fn binary_search_at(len: usize, mut f: impl FnMut(usize) -> Ordering) -> Result<usize, usize> {
    let mut size = len;
    if size == 0 {
        return Err(0);
    }
    let mut base = 0usize;
    while size > 1 {
        let half = size / 2;
        let mid = base + half;
        let cmp = f(mid);
        base = if cmp == Ordering::Greater { base } else { mid };
        size -= half;
    }
    let cmp = f(base);
    if cmp == Ordering::Equal { Ok(base) } else { Err(base + (cmp == Ordering::Less) as usize) }
}

/// slice::partition_point over the keys of `0..len`, step for step.
fn partition_at(len: usize, key: impl Fn(usize) -> u32, mut pred: impl FnMut(u32) -> bool) -> usize {
    binary_search_at(len, |i| if pred(key(i)) { Ordering::Less } else { Ordering::Greater }).unwrap_or_else(|i| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 16) as u32
        }
    }

    fn regions(rng: &mut Rng, n: usize, sorted: bool) -> Vec<(u32, u32, u32)> {
        let mut key = 0u32;
        (0..n).map(|_| {
            if sorted { key += rng.next() % 40 + (rng.next() % 3 == 0) as u32 * 100_000; } else { key = rng.next() % 5000; }
            (key, key.wrapping_add(rng.next() % 50), if rng.next() % 4 == 0 { u32::MAX } else { rng.next() % 9000 })
        }).collect()
    }

    fn origins(rng: &mut Rng, n: usize, sorted: bool) -> Vec<TokenOrigin> {
        let mut lo = 0u32;
        (0..n).map(|_| {
            if sorted { lo += rng.next() % 6; } else { lo = rng.next() % 5000; }
            let len = rng.next() % 12;
            let slo = rng.next() % 100_000;
            TokenOrigin { lo, hi: lo + len, source: SourceSpan { file: rng.next() % 3, lo: slo, hi: slo + rng.next() % 12 } }
        }).collect()
    }

    fn check_roundtrip<K: Kind>(rows: &[K::Item]) -> Table<K> {
        let t = Table::<K>::from_rows(rows);
        assert_eq!(t.len(), rows.len());
        assert_eq!(t.iter().collect::<Vec<_>>(), rows);
        assert_eq!(t.iter_rev().collect::<Vec<_>>(), rows.iter().rev().copied().collect::<Vec<_>>());
        for i in [0, 1, 63, 64, 65, 127, 128, 500].into_iter().chain(rows.len().checked_sub(1)) {
            assert_eq!(t.get(i), rows.get(i).copied());
            assert_eq!(t.iter_from(i).collect::<Vec<_>>(), rows.get(i..).unwrap_or(&[]));
        }
        assert_eq!(t.last(), rows.last().copied());
        assert_eq!(t.sorted(), rows.windows(2).all(|w| K::key(&w[0]) <= K::key(&w[1])));
        let leaked: &'static [u8] = Box::leak(t.bytes().to_vec().into_boxed_slice());
        assert!(Table::<K>::mapped(leaked).unwrap() == t);
        t
    }

    fn check_search<K: Kind>(rows: &[K::Item], upto: u32) {
        let t = check_roundtrip::<K>(rows);
        let r: OSegRef<K> = (&t).into();
        for key in 0..upto {
            let want = rows.partition_point(|x| K::key(x) <= key).checked_sub(1).map(|i| (i, rows[i]));
            assert_eq!(r.floor(key), want, "floor {key}");
            let want = match rows.binary_search_by_key(&key, |x| K::key(x)) { Ok(i) => Some(i), Err(i) => i.checked_sub(1) }.map(|i| (i, rows[i]));
            assert_eq!(r.floor_exact(key), want, "floor_exact {key}");
        }
    }

    #[test]
    fn tables_round_trip_and_search_like_slices() {
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for n in [0usize, 1, 2, 63, 64, 65, 200, 1000] {
            for sorted in [true, false] {
                check_search::<Regions>(&regions(&mut rng, n, sorted), 5200);
                check_search::<Maps>(&regions(&mut rng, n, sorted), 5200);
                check_search::<Origins>(&origins(&mut rng, n, sorted), 5200);
            }
        }
        // equal keys, a zero row, extreme values
        let rows = vec![(0, 0, 0), (0, 5, 1), (5, 5, u32::MAX), (5, 0, 2), (u32::MAX, u32::MAX, u32::MAX)];
        check_search::<Regions>(&rows, 10);
        check_search::<Maps>(&rows, 10);
    }

    #[test]
    fn appended_rows_search_like_one_array() {
        let mut rng = Rng(12345);
        for (nb, no) in [(0usize, 0usize), (0, 10), (130, 0), (130, 70), (64, 64)] {
            for shuffled in [false, true] {
                let mut rows = regions(&mut rng, nb + no, !shuffled);
                if !shuffled { rows.sort(); rows.dedup_by_key(|r| r.0); }
                let nb = nb.min(rows.len());
                let base = Table::<Regions>::from_rows(&rows[..nb]);
                let mut seg = OSeg::on(&base);
                for r in &rows[nb..] { seg.push(*r); }
                assert_eq!(seg.len(), rows.len());
                assert_eq!(seg.iter().collect::<Vec<_>>(), rows);
                assert_eq!(seg.iter_rev().collect::<Vec<_>>(), rows.iter().rev().copied().collect::<Vec<_>>());
                assert_eq!(seg.last(), rows.last().copied());
                let v = seg.view();
                for key in (0..rows.last().map_or(10, |r| r.0 + 5)).step_by(7).chain(0..30) {
                    let want = rows.partition_point(|x| x.0 <= key).checked_sub(1).map(|i| (i, rows[i]));
                    assert_eq!(v.floor(key), want);
                    let want = match rows.binary_search_by_key(&key, |x| x.0) { Ok(i) => Some(i), Err(i) => i.checked_sub(1) }.map(|i| (i, rows[i]));
                    assert_eq!(v.floor_exact(key), want);
                }
            }
        }
    }

    #[test]
    fn remap_changes_only_files() {
        let mut rng = Rng(7);
        let rows = origins(&mut rng, 300, true);
        let t = Table::<Origins>::from_rows(&rows).remap_files(|f| f + 10);
        let want: Vec<_> = rows.iter().map(|o| { let mut o = *o; if o.source.file != 0 { o.source.file += 10; } o }).collect();
        assert_eq!(t.iter().collect::<Vec<_>>(), want);
    }

    /// ns per lookup, plain rows (the old `origin_at` partition_point) against the table:
    /// `cargo test --release -p relax-front origin_table::tests::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_lookup() {
        let mut rng = Rng(99);
        // macro expansion shaped rows: runs of copied fragment tokens, then call-site spans
        let mut rows = Vec::new();
        let mut lo = 0u32;
        let mut slo = 0u32;
        while rows.len() < 2_000_000 {
            let len = rng.next() % 9 + 1;
            if rng.next() % 5 == 0 { slo = rng.next() % 3_000_000; }
            slo += len + rng.next() % 2;
            rows.push(TokenOrigin { lo, hi: lo + len, source: SourceSpan { file: 0, lo: slo, hi: slo + len } });
            lo += len + rng.next() % 2;
        }
        let t = Table::<Origins>::from_rows(&rows);
        println!("rows {} plain {} B encoded {} B ({:.2} B/row)", rows.len(), rows.len() * 20, t.bytes().len(), t.bytes().len() as f64 / rows.len() as f64);
        let probes: Vec<u32> = (0..2_000_000).map(|_| rng.next() % lo).collect();
        let r: OSegRef<Origins> = (&t).into();
        for _ in 0..2 {
            let t0 = std::time::Instant::now();
            let mut acc = 0u64;
            for &p in &probes {
                let i = rows.partition_point(|o| o.lo <= p);
                if let Some(i) = i.checked_sub(1) { acc += rows[i].source.lo as u64; }
            }
            let plain = t0.elapsed();
            let t0 = std::time::Instant::now();
            let mut acc2 = 0u64;
            for &p in &probes {
                if let Some((_, o)) = r.floor(p) { acc2 += o.source.lo as u64; }
            }
            let packed = t0.elapsed();
            assert_eq!(acc, acc2);
            println!("lookup: plain {:.0} ns, table {:.0} ns", plain.as_nanos() as f64 / probes.len() as f64, packed.as_nanos() as f64 / probes.len() as f64);
        }
        let t0 = std::time::Instant::now();
        let n = t.iter().count();
        println!("sequential decode: {:.1} ns/row ({n} rows)", t0.elapsed().as_nanos() as f64 / n as f64);
        let t0 = std::time::Instant::now();
        let t2 = Table::<Origins>::from_rows(&rows);
        println!("encode: {:.1} ns/row ({} B)", t0.elapsed().as_nanos() as f64 / rows.len() as f64, t2.bytes().len());
    }
}
