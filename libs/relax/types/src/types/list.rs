//! Interned lists of ids (rustc_middle/src/ty/list.rs `List<T>`, GenericArgsRef): a type's
//! argument lists live once in the interner's list arena and a kind names one by a u32, so
//! a TyKind holds no heap pointer (it is Copy and maps from a collected image as is).
use super::*;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

/// An interned list of `T` (u32-sized ids: TyId, DefId, or a trait object's (item, term,
/// bound regions) triples): the arena position of its first word and, below 7, its word count
/// (`Lists`). 0 is the empty list.
#[repr(transparent)]
pub struct List<T>(u32, std::marker::PhantomData<fn() -> T>);
pub type TyList = List<TyId>;

impl<T> List<T> {
    pub const EMPTY: Self = List(0, std::marker::PhantomData);
    /// Every empty list is EMPTY (interning returns it), so this needs no arena read.
    #[inline]
    pub fn is_empty(self) -> bool { self.0 == 0 }
    pub(crate) fn from_raw(id: u32) -> Self { List(id, std::marker::PhantomData) }
    pub(crate) fn raw(self) -> u32 { self.0 }
}
impl<T> Clone for List<T> { #[inline] fn clone(&self) -> Self { *self } }
impl<T> Copy for List<T> {}
impl<T> PartialEq for List<T> { #[inline] fn eq(&self, o: &Self) -> bool { self.0 == o.0 } }
impl<T> Eq for List<T> {}
impl<T> Default for List<T> { fn default() -> Self { Self::EMPTY } }
impl<T> std::hash::Hash for List<T> { fn hash<H: std::hash::Hasher>(&self, h: &mut H) { self.0.hash(h) } }
impl<T> std::fmt::Debug for List<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "List({})", self.0) }
}

/// Element types a list holds: plain u32 words, `WORDS` per element.
/// SAFETY: implementors are exactly `WORDS` u32s (size, alignment, no padding, any value
/// this build wrote reads back as itself).
pub unsafe trait ListElem: Copy + 'static { const WORDS: usize; }
unsafe impl ListElem for TyId { const WORDS: usize = 1; }
unsafe impl ListElem for DefId { const WORDS: usize = 1; }
unsafe impl ListElem for (DefId, TyId, u32) { const WORDS: usize = 3; }
const _: () = {
    assert!(std::mem::size_of::<TyId>() == 4 && std::mem::align_of::<TyId>() == 4);
    assert!(std::mem::size_of::<DefId>() == 4 && std::mem::align_of::<DefId>() == 4);
    assert!(std::mem::size_of::<(DefId, TyId, u32)>() == 12 && std::mem::align_of::<(DefId, TyId, u32)>() == 4);
    assert!(std::mem::offset_of!((DefId, TyId, u32), 0) == 0);
    assert!(std::mem::offset_of!((DefId, TyId, u32), 1) == 4);
    assert!(std::mem::offset_of!((DefId, TyId, u32), 2) == 8);
};

/// Words per arena chunk; a list never crosses a chunk (chunks never move: `list` hands out
/// slices into them), so a list holds at most LIST_CHUNK - 2 words.
const LIST_CHUNK_BITS: u32 = 16;
const LIST_CHUNK: usize = 1 << LIST_CHUNK_BITS;
/// 2^29 words at most (a list id holds a position in its high 29 bits).
const LIST_MAX_CHUNKS: usize = 1 << 13;
/// A list id's low bits: its word count, or LONG when a length word precedes the words.
const LEN_BITS: u32 = 3;
const LONG: u32 = (1 << LEN_BITS) - 1;
const LIST_BUCKET_BITS: u32 = 18;

/// The list arena: each list is [next, words...], or [next, len, words...] from LONG words on
/// (next: the following list in its hash bucket, 0 = none), named by `position of its first
/// word << 3 | word count` (LONG: read the length word): most lists hold one or two ids, so
/// the count is in the id rather than in a word. Positions below `mapped.len()` read from a
/// collected image; later ones from chunks (zeroed, so skipped words read 0).
/// Lookups and inserts are lock-free (a list is complete before its bucket head publishes it).
pub(super) struct Lists {
    pub(super) mapped: &'static [u32],
    chunks: Box<[AtomicPtr<u32>]>,
    /// the next free position
    end: AtomicU32,
    buckets: Box<[AtomicU32]>,
}

// Raw chunk pointers are owned here; words are written once before publication.
unsafe impl Send for Lists {}
unsafe impl Sync for Lists {}

impl Drop for Lists {
    fn drop(&mut self) {
        for c in self.chunks.iter_mut() {
            let p = *c.get_mut();
            if !p.is_null() {
                unsafe { std::alloc::dealloc(p.cast(), Self::chunk_layout()) };
            }
        }
    }
}

impl Lists {
    pub(super) fn new() -> Self {
        Lists {
            mapped: &[],
            chunks: (0..LIST_MAX_CHUNKS).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect(),
            // position 0 is never a list's (id 0 is the empty list)
            end: AtomicU32::new(1),
            buckets: (0..1usize << LIST_BUCKET_BITS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    fn chunk_layout() -> std::alloc::Layout {
        std::alloc::Layout::array::<u32>(LIST_CHUNK).unwrap()
    }

    #[inline(always)]
    fn word(&self, p: usize) -> u32 {
        if p < self.mapped.len() {
            return unsafe { *self.mapped.get_unchecked(p) };
        }
        // Relaxed: whoever holds a list id got it after its words were published
        let c = unsafe { self.chunks.get_unchecked(p >> LIST_CHUNK_BITS) }.load(Ordering::Relaxed);
        unsafe { *c.add(p & (LIST_CHUNK - 1)) }
    }

    /// The words of list `id` (its elements).
    #[inline(always)]
    pub(super) fn words(&self, id: u32) -> &[u32] {
        if id == 0 {
            return &[];
        }
        let p = (id >> LEN_BITS) as usize;
        let n = match id & LONG {
            LONG => self.word(p - 1) as usize,
            n => n as usize,
        };
        if p < self.mapped.len() {
            return &self.mapped[p..p + n];
        }
        let c = unsafe { self.chunks.get_unchecked(p >> LIST_CHUNK_BITS) }.load(Ordering::Relaxed);
        unsafe { std::slice::from_raw_parts(c.add(p & (LIST_CHUNK - 1)), n) }
    }

    /// The next list in `id`'s hash bucket (0: none).
    #[inline]
    fn next(&self, id: u32) -> u32 {
        let p = (id >> LEN_BITS) as usize;
        self.word(if id & LONG == LONG { p - 2 } else { p - 1 })
    }

    fn find(&self, mut id: u32, w: &[u32]) -> Option<u32> {
        while id != 0 {
            if self.words(id) == w {
                return Some(id);
            }
            id = self.next(id);
        }
        None
    }

    /// Interns `w` (a list's words). Lock-free: the words go to space reserved by advancing
    /// `end`, and the bucket head publishes the list (a lost race re-reads the bucket, which
    /// then holds any equal list; the reserved words stay unused).
    pub(super) fn intern(&self, w: &[u32]) -> u32 {
        if w.is_empty() {
            return 0;
        }
        use std::hash::{Hash, Hasher};
        let mut h = crate::fxhash::FxHasher::default();
        w.hash(&mut h);
        let bucket = &self.buckets[(h.finish() >> (64 - LIST_BUCKET_BITS)) as usize];
        let mut head = bucket.load(Ordering::Acquire);
        if let Some(id) = self.find(head, w) {
            return id;
        }
        let long = w.len() >= LONG as usize;
        let head_words = 1 + long as usize;
        let need = w.len() + head_words;
        assert!(need <= LIST_CHUNK, "makepad: interned list of {} words", w.len());
        let mut end = self.end.load(Ordering::Relaxed);
        let p = loop {
            let mut p = end as usize;
            if (p & (LIST_CHUNK - 1)) + need > LIST_CHUNK {
                p = (p | (LIST_CHUNK - 1)) + 1;
            }
            assert!(p >> LIST_CHUNK_BITS < LIST_MAX_CHUNKS, "makepad: list arena full");
            match self.end.compare_exchange_weak(end, (p + need) as u32, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break p,
                Err(e) => end = e,
            }
        };
        let c = self.chunk(p >> LIST_CHUNK_BITS);
        let at = unsafe { c.add(p & (LIST_CHUNK - 1)) };
        unsafe {
            if long {
                at.add(1).write(w.len() as u32);
            }
            std::ptr::copy_nonoverlapping(w.as_ptr(), at.add(head_words), w.len());
        }
        let id = (((p + head_words) as u32) << LEN_BITS) | (w.len() as u32).min(LONG);
        loop {
            unsafe { at.write(head) };
            match bucket.compare_exchange(head, id, Ordering::Release, Ordering::Acquire) {
                Ok(_) => return id,
                Err(now) => {
                    if let Some(other) = self.find(now, w) {
                        return other;
                    }
                    head = now;
                }
            }
        }
    }

    /// Chunk `ci`, allocated (zeroed) by the first thread that needs it.
    fn chunk(&self, ci: usize) -> *mut u32 {
        let c = self.chunks[ci].load(Ordering::Acquire);
        if !c.is_null() {
            return c;
        }
        let new = unsafe { std::alloc::alloc_zeroed(Self::chunk_layout()) as *mut u32 };
        if new.is_null() {
            std::alloc::handle_alloc_error(Self::chunk_layout());
        }
        match self.chunks[ci].compare_exchange(std::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => new,
            Err(other) => {
                unsafe { std::alloc::dealloc(new.cast(), Self::chunk_layout()) };
                other
            }
        }
    }

    /// Positions in use (the image's word count).
    pub(super) fn end(&self) -> usize {
        self.end.load(Ordering::Acquire) as usize
    }

    /// Lists in the buckets (memory report).
    pub(super) fn count(&self) -> usize {
        self.buckets.iter().map(|b| {
            let (mut id, mut n) = (b.load(Ordering::Acquire), 0);
            while id != 0 {
                n += 1;
                id = self.next(id);
            }
            n
        }).sum()
    }

    /// Bytes held: allocated chunks and the bucket table.
    pub(super) fn heap_bytes(&self) -> (usize, usize) {
        let chunks = self.chunks.iter().filter(|c| !c.load(Ordering::Relaxed).is_null()).count();
        (chunks * LIST_CHUNK * 4, self.buckets.len() * 4)
    }

    /// The words from position `from` to the end (image tail), skipped words as 0.
    pub(super) fn tail_words(&self, from: usize) -> Vec<u32> {
        (from..self.end()).map(|p| {
            if p < self.mapped.len() { return self.mapped[p]; }
            let c = self.chunks[p >> LIST_CHUNK_BITS].load(Ordering::Acquire);
            if c.is_null() { 0 } else { unsafe { *c.add(p & (LIST_CHUNK - 1)) } }
        }).collect()
    }

    pub(super) fn bucket_pairs(&self) -> Vec<u32> {
        self.buckets.iter().enumerate().filter_map(|(i, head)| {
            let head = head.load(Ordering::Acquire);
            (head != 0).then_some([i as u32, head])
        }).flatten().collect()
    }

    /// A collected image's words and bucket heads; new lists follow the mapped words.
    pub(super) fn restored(mapped: &'static [u32], buckets: &[u32]) -> Option<Self> {
        if mapped.first() != Some(&0) || buckets.len() % 2 != 0 || mapped.len() >= LIST_MAX_CHUNKS * LIST_CHUNK {
            return None;
        }
        let mut l = Lists::new();
        for pair in buckets.chunks_exact(2) {
            if pair[0] as usize >= l.buckets.len() || pair[1] == 0 || (pair[1] >> LEN_BITS) as usize >= mapped.len() {
                return None;
            }
            *l.buckets[pair[0] as usize].get_mut() = pair[1];
        }
        l.mapped = mapped;
        *l.end.get_mut() = mapped.len() as u32;
        Some(l)
    }
}

impl Types {
    /// The elements of an interned list.
    #[inline(always)]
    pub fn list<T: ListElem>(&self, l: List<T>) -> &[T] {
        let w = self.lists.words(l.0);
        // SAFETY: lists of T were interned from &[T] (ListElem: T is WORDS plain u32s)
        unsafe { std::slice::from_raw_parts(w.as_ptr() as *const T, w.len() / T::WORDS) }
    }

    /// Interns a list (`List::EMPTY` for an empty one).
    pub fn mk_list<T: ListElem>(&self, v: &[T]) -> List<T> {
        // SAFETY: as `list`
        let w = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u32, v.len() * T::WORDS) };
        List(self.lists.intern(w), std::marker::PhantomData)
    }
}
