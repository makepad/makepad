//! Interned lists of ids (rustc_middle/src/ty/list.rs `List<T>`, GenericArgsRef): a type's
//! argument lists live once in the interner's list arena and a kind names one by a u32, so
//! a TyKind holds no heap pointer (it is Copy and maps from a collected image as is).
use super::*;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use super::index::Index;

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
/// slices into them), so a list holds at most LIST_CHUNK - 1 words.
const LIST_CHUNK_BITS: u32 = 16;
const LIST_CHUNK: usize = 1 << LIST_CHUNK_BITS;
/// 2^29 words at most (a list id holds a position in its high 29 bits).
const LIST_MAX_CHUNKS: usize = 1 << 13;
/// A list id's low bits: its word count, or LONG when a length word precedes the words.
const LEN_BITS: u32 = 3;
const LONG: u32 = (1 << LEN_BITS) - 1;

/// The list arena: each list is [words...], or [len, words...] from LONG words on, named by
/// `position of its first word << 3 | word count` (LONG: read the length word): most lists
/// hold one or two ids, so the count is in the id rather than in a word. Positions below
/// `mapped.len()` read from a collected image; later ones from chunks (zeroed, so skipped
/// words read 0). Lookups are lock-free (a list is complete before its slot publishes it).
pub(super) struct Lists {
    pub(super) mapped: &'static [u32],
    chunks: Box<[AtomicPtr<u32>]>,
    /// the next free position
    end: AtomicU32,
    /// the intern table: a slot is `hash << 32 | id`, so a probe reads a list's words only when
    /// the hash matches and a doubling reads no list at all. Its lock serialises position
    /// assignment and slot claims (slots are claimed in position order).
    index: Index<u64>,
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
    /// The settle rebuild's top-up (Types::top_up_from): `self`, restored from an image of `live`
    /// and unused since, gets the lists `live` made past the image's end, their words at the same
    /// positions and their slots claimed in position order (as `intern` claims them). Nothing
    /// interns into either while it runs.
    pub(super) fn top_up_from(&mut self, live: &Lists) {
        let (e0, e1) = (self.end(), live.end());
        for p in e0..e1 {
            // (word_at: a chunk skipped over whole, after a mapped prefix, was never allocated)
            let w = live.word_at(p);
            if w != 0 {
                let c = self.chunk(p >> LIST_CHUNK_BITS);
                unsafe { *c.add(p & (LIST_CHUNK - 1)) = w };
            }
        }
        self.end.store(e1 as u32, Ordering::Release);
        // the new lists' slots: the live table's entries whose words start past the image's end
        let mut new: Vec<u64> = live.index.table().filled().filter(|&s| ((s as u32) >> LEN_BITS) as usize >= e0).collect();
        new.sort_unstable_by_key(|&s| (s as u32) >> LEN_BITS);
        let mut g = self.index.lock();
        for slot in new {
            let h = (slot >> 32) as u32;
            self.index.claim(&mut g, h, slot, || self.index.table().filled().map(|s| ((s >> 32) as u32, s)).chain(std::iter::once((h, slot))));
        }
    }

    pub(super) fn new() -> Self {
        Lists {
            mapped: &[],
            chunks: (0..LIST_MAX_CHUNKS).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect(),
            // position 0 is never a list's (id 0 is the empty list)
            end: AtomicU32::new(1),
            index: Index::new(),
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

    #[inline]
    fn find(&self, h: u32, w: &[u32]) -> Option<u32> {
        let s = self.index.find(h, |s| (s >> 32) as u32 == h && self.words(s as u32) == w)?;
        Some(s as u32)
    }

    /// Interns `w` (a list's words): looked up without a lock; a new list takes the table's lock
    /// and is complete before its slot publishes it.
    pub(super) fn intern(&self, w: &[u32]) -> u32 {
        if w.is_empty() {
            return 0;
        }
        use std::hash::{Hash, Hasher};
        let mut hasher = crate::fxhash::FxHasher::default();
        w.hash(&mut hasher);
        // the high half: the high bits of the multiplicative hash mix best
        let h = (hasher.finish() >> 32) as u32;
        if let Some(id) = self.find(h, w) {
            return id;
        }
        let mut g = self.index.lock();
        if let Some(id) = self.find(h, w) {
            return id;
        }
        let long = w.len() >= LONG as usize;
        let head_words = long as usize;
        let need = w.len() + head_words;
        assert!(need <= LIST_CHUNK, "relax: interned list of {} words", w.len());
        let mut p = self.end.load(Ordering::Relaxed) as usize;
        if (p & (LIST_CHUNK - 1)) + need > LIST_CHUNK {
            p = (p | (LIST_CHUNK - 1)) + 1;
        }
        assert!(p >> LIST_CHUNK_BITS < LIST_MAX_CHUNKS, "relax: list arena full");
        let c = self.chunk(p >> LIST_CHUNK_BITS);
        let at = unsafe { c.add(p & (LIST_CHUNK - 1)) };
        unsafe {
            if long {
                at.write(w.len() as u32);
            }
            std::ptr::copy_nonoverlapping(w.as_ptr(), at.add(head_words), w.len());
        }
        self.end.store((p + need) as u32, Ordering::Release);
        let id = (((p + head_words) as u32) << LEN_BITS) | (w.len() as u32).min(LONG);
        let slot = (h as u64) << 32 | id as u64;
        self.index.claim(&mut g, h, slot, || self.index.table().filled().map(|s| ((s >> 32) as u32, s)).chain(std::iter::once((h, slot))));
        id
    }

    /// Chunk `ci`, allocated (zeroed) by the first list that needs it (the table's lock is held,
    /// or the interner is not shared yet).
    fn chunk(&self, ci: usize) -> *mut u32 {
        let c = self.chunks[ci].load(Ordering::Acquire);
        if !c.is_null() {
            return c;
        }
        let new = unsafe { std::alloc::alloc_zeroed(Self::chunk_layout()) as *mut u32 };
        if new.is_null() {
            std::alloc::handle_alloc_error(Self::chunk_layout());
        }
        self.chunks[ci].store(new, Ordering::Release);
        new
    }

    /// Positions in use (the image's word count).
    pub(super) fn end(&self) -> usize {
        self.end.load(Ordering::Acquire) as usize
    }

    /// Frees the intern tables replaced by growth (`self` is not shared).
    pub(super) fn free_retired(&mut self) {
        self.index.free_retired();
    }

    /// Lists interned (memory report).
    pub(super) fn count(&self) -> usize {
        self.index.count()
    }

    /// Bytes held: allocated chunks and the intern table.
    pub(super) fn heap_bytes(&self) -> (usize, usize) {
        let chunks = self.chunks.iter().filter(|c| !c.load(Ordering::Relaxed).is_null()).count();
        (chunks * LIST_CHUNK * 4, self.index.bytes())
    }

    /// The word at position `p`; a skipped one as 0 (its chunk may never have been allocated).
    fn word_at(&self, p: usize) -> u32 {
        if p < self.mapped.len() { return self.mapped[p]; }
        let c = self.chunks[p >> LIST_CHUNK_BITS].load(Ordering::Acquire);
        if c.is_null() { 0 } else { unsafe { *c.add(p & (LIST_CHUNK - 1)) } }
    }

    /// The words from `from` to `end` as bytes (an image section's tail, taken once), asking
    /// `stop` every 64Ki words (None: it said yes)
    pub(super) fn tail_word_bytes(&self, from: usize, end: usize, stop: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        let mut b = Vec::with_capacity(end.saturating_sub(from) * 4);
        for p in from..end {
            if (p - from) % (1 << 16) == (1 << 16) - 1 && stop() { return None; }
            b.extend_from_slice(&self.word_at(p).to_ne_bytes());
        }
        Some(b)
    }

    /// The image's view of the table: the end position and the slots of the lists below it.
    /// Read under the lock, so every list below the end is in the table read; a list claimed
    /// since lies past the end and reads as empty, which cuts no other list's probe run (slots
    /// are claimed in position order). A table replaced meanwhile stays allocated.
    /// `stop`: as Table::snapshot.
    pub(super) fn image_slots(&self, stop: &dyn Fn() -> bool) -> Option<(usize, Vec<u64>)> {
        let (end, t) = {
            let _g = self.index.lock();
            (self.end(), self.index.table())
        };
        Some((end, t.snapshot(|s| ((s as u32) >> LEN_BITS) as usize >= 1 && (((s as u32) >> LEN_BITS) as usize) < end, stop)?))
    }

    /// A collected image's words and intern table; new lists follow the mapped words.
    pub(super) fn restored(mapped: &'static [u32], slots: &[u64]) -> Option<Self> {
        if mapped.first() != Some(&0) || mapped.len() >= LIST_MAX_CHUNKS * LIST_CHUNK {
            return None;
        }
        let index = Index::restored(slots, |s| s as u32 != 0 && (((s as u32) >> LEN_BITS) as usize) < mapped.len())?;
        let mut l = Lists::new();
        l.index = index;
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
