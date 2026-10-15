//! Collected graph encoding. Unlike a crate's parse record this preserves resolved IDs.
//! Only the exact compiler build and a validated input certificate may restore it.
use crate::crate_image::{Codec, Decode};
use std::sync::{Arc, OnceLock};
use relax_session::sync::Mutex;

/// An immutable image component whose first reader may need owned strings/maps. Keeping
/// its bytes mapped avoids eagerly rebuilding macro scopes that warm code never asks for.
pub struct Deferred<T> {
    bytes: Option<&'static [u8]>,
    value: OnceLock<T>,
}
impl<T: Default> Default for Deferred<T> {
    fn default() -> Self { Self { bytes: None, value: OnceLock::from(T::default()) } }
}
impl<T: Codec> Deferred<T> {
    pub fn mapped(bytes: &'static [u8]) -> Self { Self { bytes: Some(bytes), value: OnceLock::new() } }
    pub fn image(&self) -> Vec<u8> {
        match self.value.get() {
            Some(value) => crate::crate_image::encode(value),
            None => self.bytes.expect("mapped component").to_vec(),
        }
    }
    pub fn write_image(&self, image: &mut crate::image::Writer<'_>, id: u32) {
        match self.value.get() {
            Some(value) => image.add_bytes(id, crate::crate_image::encode(value)),
            None => image.add_mapped(id, self.bytes.expect("mapped component")),
        }
    }
}
impl<T: Codec> std::ops::Deref for Deferred<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value.get_or_init(|| crate::crate_image::decode(self.bytes.expect("mapped component"))
            .expect("checksummed collected component"))
    }
}
impl<T: Codec> std::ops::DerefMut for Deferred<T> {
    fn deref_mut(&mut self) -> &mut T {
        let _ = &**self;
        self.bytes = None;
        self.value.get_mut().unwrap()
    }
}

#[macro_export]
macro_rules! collected_fields {
    ($t:ty, $($f:ident),+ $(,)?) => {
        impl $crate::crate_image::Codec for $t {
            fn put(&self, w: &mut Vec<u8>) {
                $( $crate::crate_image::Codec::put(&self.$f, w); )+
            }
            fn get(r: &mut $crate::crate_image::Decode<'_>) -> Option<Self> {
                Some(Self { $( $f: $crate::crate_image::Codec::get(r)?, )+ })
            }
        }
    };
}

#[macro_export]
macro_rules! collected_enum {
    ($t:ty; $($tag:literal => $v:ident $(($($f:ident),+))?),+ $(,)?) => {
        impl $crate::crate_image::Codec for $t {
            fn put(&self, w: &mut Vec<u8>) {
                match self {
                    $(Self::$v $(($($f),+))? => {
                        $crate::crate_image::Codec::put(&($tag as u8), w);
                        $($( $crate::crate_image::Codec::put($f, w); )+)?
                    })+
                }
            }
            fn get(r: &mut $crate::crate_image::Decode<'_>) -> Option<Self> {
                Some(match <u8 as $crate::crate_image::Codec>::get(r)? {
                    $($tag => Self::$v $(($( { let $f = $crate::crate_image::Codec::get(r)?; $f } ),+))?,)+
                    _ => return None,
                })
            }
        }
    };
}

#[macro_export]
macro_rules! collected_id {
    ($($t:ty),+ $(,)?) => { $(
        impl $crate::crate_image::Codec for $t {
            fn put(&self, w: &mut Vec<u8>) { $crate::crate_image::Codec::put(&self.0, w); }
            fn get(r: &mut $crate::crate_image::Decode<'_>) -> Option<Self> {
                Some(Self($crate::crate_image::Codec::get(r)?))
            }
        }
    )+ };
}

impl Codec for () {
    fn put(&self, _: &mut Vec<u8>) {}
    fn get(_: &mut Decode<'_>) -> Option<Self> { Some(()) }
}
impl<A: Codec, B: Codec, C: Codec> Codec for (A, B, C) {
    fn put(&self, w: &mut Vec<u8>) { self.0.put(w); self.1.put(w); self.2.put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some((Codec::get(r)?, Codec::get(r)?, Codec::get(r)?)) }
}
impl<A: Codec, B: Codec, C: Codec, D: Codec> Codec for (A, B, C, D) {
    fn put(&self, w: &mut Vec<u8>) { self.0.put(w); self.1.put(w); self.2.put(w); self.3.put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some((Codec::get(r)?, Codec::get(r)?, Codec::get(r)?, Codec::get(r)?)) }
}
impl<T: Codec> Codec for OnceLock<T> {
    fn put(&self, w: &mut Vec<u8>) {
        self.get().is_some().put(w);
        if let Some(v) = self.get() { v.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(if bool::get(r)? { Self::from(T::get(r)?) } else { Self::new() })
    }
}
impl<T: Codec> Codec for Mutex<T> {
    fn put(&self, w: &mut Vec<u8>) { self.lock().unwrap_or_else(|e| e.into_inner()).put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self::new(Codec::get(r)?)) }
}
impl<T: Codec> Codec for Arc<[T]> {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        for v in self.iter() { v.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Vec::<T>::get(r)?.into()) }
}
impl<T: Codec + 'static> Codec for crate::arena::Arena<T> {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        for v in self.iter() { v.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Vec::<T>::get(r)?.into()) }
}

impl Codec for std::sync::atomic::AtomicU32 {
    fn put(&self, w: &mut Vec<u8>) { self.load(std::sync::atomic::Ordering::Relaxed).put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self::new(u32::get(r)?)) }
}
impl<T> Codec for crate::ast::List<T> {
    fn put(&self, w: &mut Vec<u8>) { self.lo.put(w); self.len.put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self::new(u32::get(r)?, u32::get(r)?)) }
}
impl<K: Codec + Ord + std::hash::Hash + Clone, V: Codec + Clone> Codec for crate::conc::MemoMap<K, V> {
    fn put(&self, w: &mut Vec<u8>) { self.snapshot().put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let out = Self::default();
        for (k, v) in <crate::fxhash::HashMap<K, V> as Codec>::get(r)? { out.insert(k, v); }
        Some(out)
    }
}
impl<K: Codec + Ord + std::hash::Hash + Clone, V: Codec + Clone> Codec for crate::conc::ShardMap<K, V> {
    fn put(&self, w: &mut Vec<u8>) { self.snapshot().put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let out = Self::default();
        for (k, v) in <crate::fxhash::HashMap<K, V> as Codec>::get(r)? { out.insert(k, v); }
        Some(out)
    }
}

crate::collected_id!(crate::program::DefId, crate::ast::TyId, crate::ast::ExprId);
crate::collected_enum!(crate::program::Prim;
    0 => Bool, 1 => Char, 2 => Str, 3 => I8, 4 => I16, 5 => I32, 6 => I64,
    7 => I128, 8 => Isize, 9 => U8, 10 => U16, 11 => U32, 12 => U64,
    13 => U128, 14 => Usize, 15 => F32, 16 => F64);
crate::collected_enum!(crate::program::DefKind;
    0 => Mod, 1 => Fn, 2 => Struct, 3 => Union, 4 => Enum, 5 => Variant,
    6 => Const, 7 => Static, 8 => Trait, 9 => Impl, 10 => TypeAlias,
    11 => AssocFn, 12 => AssocConst, 13 => AssocTy, 14 => ForeignFn,
    15 => ForeignStatic, 16 => MacroRules, 17 => Prim(p), 18 => AnonConst, 19 => OpaqueTy, 20 => Closure, 21 => ForeignTy, 22 => Field);
crate::collected_enum!(crate::ast::SegKind; 0 => Ident, 1 => SelfValue, 2 => SelfType, 3 => Super, 4 => Crate);
crate::collected_fields!(crate::ast::Path, segs, lo);
impl Codec for crate::ast::SegList {
    fn put(&self, w: &mut Vec<u8>) { self.lo.put(w); self.len().put(w); self.global().put(w); self.has_qself().put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let lo = u32::get(r)?;
        let len = u32::get(r)?;
        if len >= 1 << 30 { return None; }
        let global = bool::get(r)?;
        let segs = Self::new(lo, len, global);
        Some(if bool::get(r)? { segs.with_qself() } else { segs })
    }
}

// Snapshot lookup caches are transient; retain the existing image representation.
impl Codec for crate::macros::MacroTable {
    fn put(&self, w: &mut Vec<u8>) {
        self.defs.put(w); self.aliases.put(w); self.macro_use.put(w); self.renames.put(w);
        self.mod_names.put(w); self.crates.put(w); self.s2_fmt.put(w); self.exp_uses.put(w); self.sealed.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let mut table = Self::default();
        table.defs = Codec::get(r)?; table.aliases = Codec::get(r)?; table.macro_use = Codec::get(r)?;
        table.renames = Codec::get(r)?; table.mod_names = Codec::get(r)?; table.crates = Codec::get(r)?;
        table.s2_fmt = Codec::get(r)?; table.exp_uses = Codec::get(r)?; table.sealed = Codec::get(r)?;
        Some(table)
    }
}

impl Codec for crate::lang_items::LangItem {
    fn put(&self, w: &mut Vec<u8>) { self.name().to_string().put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Self::from_name(&String::get(r)?) }
}
crate::collected_fields!(crate::lang_items::LangItems, items, by_def);

/// One lazily decoded value per image entry: a zeroed array of pointers (null: not decoded),
/// so a fresh allocation's pages stay untouched until their entries are used.
pub struct LazySlots<T> {
    slots: Box<[std::sync::atomic::AtomicPtr<T>]>,
    _owns: std::marker::PhantomData<Box<T>>,
}
// As OnceLock<T>: any thread may decode a value that every thread then shares.
unsafe impl<T: Send> Send for LazySlots<T> {}
unsafe impl<T: Send + Sync> Sync for LazySlots<T> {}
impl<T> LazySlots<T> {
    pub fn new(n: usize) -> Self {
        // all-zero bytes are null pointers
        let slots = unsafe { Box::<[std::sync::atomic::AtomicPtr<T>]>::new_zeroed_slice(n).assume_init() };
        Self { slots, _owns: std::marker::PhantomData }
    }
    pub fn get(&self, i: usize) -> Option<&T> {
        let p = self.slots[i].load(std::sync::atomic::Ordering::Acquire);
        // SAFETY: a published pointer stays valid and unchanged while &self lives
        (!p.is_null()).then(|| unsafe { &*p })
    }
    pub fn get_or_init(&self, i: usize, f: impl FnOnce() -> T) -> &T {
        if let Some(v) = self.get(i) { return v; }
        let new = Box::into_raw(Box::new(f()));
        match self.slots[i].compare_exchange(std::ptr::null_mut(), new, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire) {
            Ok(_) => unsafe { &*new },
            // another thread decoded it first: keep theirs
            Err(theirs) => { drop(unsafe { Box::from_raw(new) }); unsafe { &*theirs } }
        }
    }
    pub fn take(&mut self, i: usize) -> Option<T> {
        let p = std::mem::replace(self.slots[i].get_mut(), std::ptr::null_mut());
        (!p.is_null()).then(|| *unsafe { Box::from_raw(p) })
    }
    pub fn decoded(&self) -> impl Iterator<Item = &T> + '_ { (0..self.slots.len()).filter_map(|i| self.get(i)) }
    pub fn decoded_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.slots.iter_mut().filter_map(|p| { let p = *p.get_mut(); (!p.is_null()).then(|| unsafe { &mut *p }) })
    }
}
impl<T> Drop for LazySlots<T> {
    fn drop(&mut self) {
        for p in self.slots.iter_mut() {
            let p = *p.get_mut();
            if !p.is_null() { drop(unsafe { Box::from_raw(p) }); }
        }
    }
}

/// Coalesce unchanged contiguous records into references to their immutable image.
/// Generated records keep their normal Codec bytes; no entry is decoded just to copy it.
#[derive(Default)]
struct ImageEntries {
    chunks: Vec<crate::image::Chunk<'static>>,
    generated: Vec<u8>,
    flushed: usize,
}
impl ImageEntries {
    fn len(&self) -> usize { self.flushed + self.generated.len() }
    fn raw(&mut self, base: &'static [u8], start: usize, end: usize) {
        self.flush();
        if let Some(crate::image::Chunk::Mapped(last)) = self.chunks.last_mut() {
            let first = (last.as_ptr() as usize).wrapping_sub(base.as_ptr() as usize);
            if first <= start && first + last.len() == start {
                *last = &base[first..end];
                self.flushed += end - start;
                return;
            }
        }
        self.chunks.push(crate::image::Chunk::Mapped(&base[start..end]));
        self.flushed += end - start;
    }
    fn flush(&mut self) {
        if !self.generated.is_empty() {
            self.flushed += self.generated.len();
            self.chunks.push(crate::image::Chunk::Owned(std::mem::take(&mut self.generated)));
        }
    }
    fn finish(mut self) -> Vec<crate::image::Chunk<'static>> { self.flush(); self.chunks }
}

/// Injective sort key of a table key in a collected image (DefId, (TyKey, Sym), ..).
pub trait ImageKey { fn image_key(&self) -> u128; }
impl ImageKey for u32 { fn image_key(&self) -> u128 { *self as u128 } }
impl ImageKey for crate::program::DefId { fn image_key(&self) -> u128 { self.0 as u128 } }
impl<A: ImageKey, B: ImageKey> ImageKey for (A, B) {
    // Both halves fit in 64 bits for every key type used here.
    fn image_key(&self) -> u128 { (self.0.image_key() << 64) | self.1.image_key() }
}

// The containing image guarantees 8-byte alignment; u128 may need 16.
#[derive(Clone, Copy)]
#[repr(C)]
struct KeyRow { lo: u64, hi: u64 }
unsafe impl crate::image::Pod for KeyRow {}
impl KeyRow { fn get(self) -> u128 { self.lo as u128 | ((self.hi as u128) << 64) } }

/// A declaration table of a restored collected graph, read in place: the image holds keys,
/// entry offsets, encoded (key, value) entries and a hash index; a lookup decodes only the
/// entry it finds, once. The first mutation turns it into an owned HashMap (live edits, prefix retain).
pub struct ImageMap<K, V> {
    owned: crate::fxhash::HashMap<K, V>,
    mapped: Option<MappedTable<K, V>>,
    /// write_image ran on this owned table and nothing has taken `&mut` access since: its
    /// entries are what the image holds (adopt). Interior state of a value (lazy cells) is
    /// derived and may differ.
    snap: std::sync::atomic::AtomicBool,
}
/// What `adopt` did with a table: the live heap table became a view of the image just written
/// from it, or why not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Adopt {
    /// the heap table was freed; n entries are read in place from now on
    Adopted(usize),
    /// already read in place (a warm restore, or adopted before)
    Mapped,
    /// empty: nothing to free
    Empty,
    /// mutated since the image was written from it (or never written)
    Dirty,
    /// the image does not hold this table's entries
    Mismatch,
}
pub struct MappedTable<K, V> {
    keys: &'static [KeyRow],
    /// open-addressed index over `keys` (entry = row + 1, 0 = empty; linear probing)
    index: &'static [u32],
    offsets: &'static [u32],
    entries: &'static [u8],
    slots: LazySlots<(K, V)>,
}
impl<K, V> Default for ImageMap<K, V> {
    fn default() -> Self { Self { owned: Default::default(), mapped: None, snap: Default::default() } }
}
impl<K: Codec, V: Codec> MappedTable<K, V> {
    fn entry(&self, i: usize) -> &(K, V) {
        self.slots.get_or_init(i, || self.decode(i))
    }
    fn decode(&self, i: usize) -> (K, V) {
        let bytes = self.entries.get(self.offsets[i] as usize..self.offsets[i + 1] as usize);
        bytes.and_then(crate::crate_image::decode).expect("checksummed collected table")
    }
    fn find(&self, key: u128) -> Option<usize> {
        let mask = self.index.len() - 1;
        let mut at = key_slot(key, self.index.len());
        loop {
            let e = *self.index.get(at)?;
            if e == 0 { return None; }
            if self.keys.get(e as usize - 1)?.get() == key { return Some(e as usize - 1); }
            at = (at + 1) & mask;
        }
    }
}
fn key_slot(key: u128, table: usize) -> usize {
    let h = ((key as u64) ^ ((key >> 64) as u64).rotate_left(29)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (h >> (64 - table.trailing_zeros())) as usize
}
impl<K: Codec + ImageKey + std::hash::Hash + Eq, V: Codec> ImageMap<K, V> {
    /// Sections of a collected image: keys, offsets, entries, hash index.
    pub fn write_image(&self, image: &mut crate::image::Writer<'_>, id: &mut u32) {
        self.snap.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut rows: Vec<(u128, Option<(u32, u32)>, Option<(&K, &V)>)> = match &self.mapped {
            // An entry nobody decoded is unchanged; a decoded one may have filled a lazy cell.
            Some(m) => (0..m.keys.len()).map(|i| match m.slots.get(i) {
                Some(kv) => (m.keys[i].get(), None, Some((&kv.0, &kv.1))),
                None => (m.keys[i].get(), Some((m.offsets[i], m.offsets[i + 1])), None),
            }).collect(),
            None => self.owned.iter().map(|(k, v)| (k.image_key(), None, Some((k, v)))).collect(),
        };
        rows.sort_unstable_by_key(|r| r.0);
        let mut keys = Vec::with_capacity(rows.len());
        let mut offsets = Vec::with_capacity(rows.len() + 1);
        let mut entries = ImageEntries::default();
        for (key, raw, kv) in rows {
            keys.push(KeyRow { lo: key as u64, hi: (key >> 64) as u64 });
            offsets.push(u32::try_from(entries.len()).expect("collected table size"));
            match (raw, kv) {
                (Some((start, end)), _) => entries.raw(self.mapped.as_ref().unwrap().entries, start as usize, end as usize),
                (None, Some((k, v))) => { k.put(&mut entries.generated); v.put(&mut entries.generated); }
                (None, None) => unreachable!(),
            }
        }
        offsets.push(u32::try_from(entries.len()).expect("collected table size"));
        let size = (keys.len() * 2).next_power_of_two().max(2);
        let mut index = vec![0u32; size];
        for (i, k) in keys.iter().enumerate() {
            let mut at = key_slot(k.get(), size);
            while index[at] != 0 { at = (at + 1) & (size - 1); }
            index[at] = i as u32 + 1;
        }
        image.add_owned(*id, &keys); image.add_owned(*id + 1, &offsets); image.add_chunks(*id + 2, entries.finish());
        image.add_owned(*id + 3, &index);
        *id += 4;
    }
    pub fn read_image(image: &crate::image::Reader, k: &mut usize, id: &mut u32) -> Option<Self> {
        let keys: &'static [KeyRow] = image.slice(*k, *id)?;
        let offsets: &'static [u32] = image.slice(*k + 1, *id + 1)?;
        let entries = image.bytes(*k + 2, *id + 2)?;
        let index: &'static [u32] = image.slice(*k + 3, *id + 3)?;
        // Entry bounds and the index were certified when the image was written; decoding and
        // probing stay bounds-checked.
        if offsets.len() != keys.len() + 1 || *offsets.last()? as usize != entries.len()
            || !index.len().is_power_of_two() || index.len() <= keys.len() { return None; }
        *k += 4; *id += 4;
        Some(Self { owned: Default::default(), mapped: Some(MappedTable { keys, index, offsets, entries, slots: LazySlots::new(keys.len()) }), snap: Default::default() })
    }

    /// Replace this heap table by `image` (read_image of the image `write_image` made from
    /// it), when it has not been mutated since: the same entries, now read in place, decoded on
    /// first use like a warm restore's. Needs exclusive access, as any mutation does.
    /// The freed heap table goes to `retired`: the caller drops it outside its locks.
    pub fn adopt(&mut self, image: Self, retired: &mut Vec<Box<dyn Send>>) -> Adopt where K: Send + 'static, V: Send + 'static {
        if self.mapped.is_some() { return Adopt::Mapped; }
        if self.owned.is_empty() { return Adopt::Empty; }
        if !*self.snap.get_mut() { return Adopt::Dirty; }
        let Some(m) = image.mapped else { return Adopt::Mismatch };
        if m.keys.len() != self.owned.len() { return Adopt::Mismatch; }
        let n = m.keys.len();
        retired.push(Box::new(std::mem::take(&mut self.owned)));
        self.mapped = Some(m);
        Adopt::Adopted(n)
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        match &self.mapped {
            None => self.owned.get(k),
            Some(m) => Some(&m.entry(m.find(k.image_key())?).1),
        }
    }
    pub fn contains_key(&self, k: &K) -> bool {
        match &self.mapped { None => self.owned.contains_key(k), Some(m) => m.find(k.image_key()).is_some() }
    }
    pub fn len(&self) -> usize { self.mapped.as_ref().map_or(self.owned.len(), |m| m.keys.len()) }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn capacity(&self) -> usize { self.mapped.as_ref().map_or(self.owned.capacity(), |m| m.keys.len()) }
    pub fn iter(&self) -> ImageMapIter<'_, K, V> {
        match &self.mapped {
            None => ImageMapIter::Owned(self.owned.iter()),
            Some(m) => ImageMapIter::Mapped(m, 0..m.keys.len()),
        }
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> + '_ { self.iter().map(|(k, _)| k) }
    pub fn values(&self) -> impl Iterator<Item = &V> + '_ { self.iter().map(|(_, v)| v) }

    /// Mutation needs the owned map: decode every entry once, keeping the decoded ones.
    pub fn owned(&mut self) -> &mut crate::fxhash::HashMap<K, V> {
        *self.snap.get_mut() = false;
        if let Some(mut m) = self.mapped.take() {
            self.owned.reserve(m.keys.len());
            for i in 0..m.keys.len() {
                let (k, v) = match m.slots.take(i) { Some(kv) => kv, None => m.decode(i) };
                self.owned.insert(k, v);
            }
        }
        &mut self.owned
    }
    pub fn insert(&mut self, k: K, v: V) -> Option<V> { self.owned().insert(k, v) }
    pub fn remove(&mut self, k: &K) -> Option<V> { self.owned().remove(k) }
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> { self.owned().get_mut(k) }
    pub fn entry(&mut self, k: K) -> std::collections::hash_map::Entry<'_, K, V> { self.owned().entry(k) }
    pub fn retain(&mut self, f: impl FnMut(&K, &mut V) -> bool) { self.owned().retain(f) }
    pub fn values_mut(&mut self) -> std::collections::hash_map::ValuesMut<'_, K, V> { self.owned().values_mut() }
    /// Whether entries are still read from the image (no mutation since the restore).
    pub fn is_mapped(&self) -> bool { self.mapped.is_some() }
    /// decoded_values_mut, read only
    pub fn decoded_values(&self) -> Box<dyn Iterator<Item = &V> + '_> {
        match &self.mapped {
            None => Box::new(self.owned.values()),
            Some(m) => Box::new(m.slots.decoded().map(|kv| &kv.1)),
        }
    }
    /// The values a reader can have changed through a lazy cell: all of an owned map, only
    /// the decoded entries of a mapped one (the others are as the image holds them).
    pub fn decoded_values_mut(&mut self) -> Box<dyn Iterator<Item = &mut V> + '_> {
        match &mut self.mapped {
            None => Box::new(self.owned.values_mut()),
            Some(m) => Box::new(m.slots.decoded_mut().map(|kv| &mut kv.1)),
        }
    }
    pub fn iter_mut(&mut self) -> std::collections::hash_map::IterMut<'_, K, V> { self.owned().iter_mut() }
    pub fn clear(&mut self) { *self.snap.get_mut() = false; self.mapped = None; self.owned.clear() }
}
impl<K: Codec + ImageKey + std::hash::Hash + Eq, V: Codec> Extend<(K, V)> for ImageMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, it: I) { self.owned().extend(it) }
}
impl<K: Codec + ImageKey + std::hash::Hash + Eq, V: Codec> FromIterator<(K, V)> for ImageMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(it: I) -> Self { Self { owned: it.into_iter().collect(), mapped: None, snap: Default::default() } }
}
impl<K: Codec + ImageKey + std::hash::Hash + Eq, V: Codec> std::ops::Index<&K> for ImageMap<K, V> {
    type Output = V;
    fn index(&self, k: &K) -> &V { self.get(k).expect("key in collected table") }
}
impl<'a, K: Codec + ImageKey + std::hash::Hash + Eq, V: Codec> IntoIterator for &'a ImageMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = ImageMapIter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter { self.iter() }
}
/// Entries in hash order (owned) or image key order (mapped); neither is semantic.
pub enum ImageMapIter<'a, K, V> {
    Owned(std::collections::hash_map::Iter<'a, K, V>),
    Mapped(&'a MappedTable<K, V>, std::ops::Range<usize>),
}
impl<'a, K: Codec, V: Codec> Iterator for ImageMapIter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Owned(it) => it.next(),
            Self::Mapped(m, r) => r.next().map(|i| { let kv = m.entry(i); (&kv.0, &kv.1) }),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self { Self::Owned(it) => it.size_hint(), Self::Mapped(_, r) => r.size_hint() }
    }
}
/// Same encoding as a HashMap (verification compares tables across restore and rebuild).
impl<K: Codec + ImageKey + Ord + std::hash::Hash + Eq, V: Codec> Codec for ImageMap<K, V> {
    fn put(&self, w: &mut Vec<u8>) {
        let mut v: Vec<_> = self.iter().collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v.len().put(w);
        for (k, v) in v { k.put(w); v.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Vec::<(K, V)>::get(r)?.into_iter().collect())
    }
}

/// A collected image's list of encoded records (modules, impls, imports), read in place:
/// an element is decoded on first access, once. The first mutation makes it an owned Vec.
pub struct ImageVec<T> {
    owned: Vec<T>,
    mapped: Option<MappedList<T>>,
    /// as ImageMap::snap
    snap: std::sync::atomic::AtomicBool,
}
pub struct MappedList<T> {
    offsets: &'static [u32],
    entries: &'static [u8],
    slots: LazySlots<T>,
}
impl<T> Default for ImageVec<T> {
    fn default() -> Self { Self { owned: Vec::new(), mapped: None, snap: Default::default() } }
}
impl<T: Codec> MappedList<T> {
    fn len(&self) -> usize { self.offsets.len() - 1 }
    fn get(&self, i: usize) -> Option<&T> {
        if i >= self.len() { return None; }
        Some(self.slots.get_or_init(i, || self.decode(i)))
    }
    fn decode(&self, i: usize) -> T {
        let bytes = self.entries.get(self.offsets[i] as usize..self.offsets[i + 1] as usize);
        bytes.and_then(crate::crate_image::decode).expect("checksummed collected list")
    }
}
impl<T: Codec> ImageVec<T> {
    /// Sections of a collected image: offsets, entries.
    pub fn write_image(&self, image: &mut crate::image::Writer<'_>, id: &mut u32) {
        self.snap.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut offsets = Vec::with_capacity(self.len() + 1);
        let mut entries = ImageEntries::default();
        for i in 0..self.len() {
            offsets.push(u32::try_from(entries.len()).expect("collected list size"));
            match &self.mapped {
                // An element nobody decoded is unchanged; copy its bytes.
                Some(m) if m.slots.get(i).is_none() =>
                    entries.raw(m.entries, m.offsets[i] as usize, m.offsets[i + 1] as usize),
                _ => self[i].put(&mut entries.generated),
            }
        }
        offsets.push(u32::try_from(entries.len()).expect("collected list size"));
        image.add_owned(*id, &offsets); image.add_chunks(*id + 1, entries.finish());
        *id += 2;
    }
    pub fn read_image(image: &crate::image::Reader, k: &mut usize, id: &mut u32) -> Option<Self> {
        let offsets: &'static [u32] = image.slice(*k, *id)?;
        let entries = image.bytes(*k + 1, *id + 1)?;
        if offsets.is_empty() || *offsets.last()? as usize != entries.len() { return None; }
        *k += 2; *id += 2;
        Some(Self { owned: Vec::new(), mapped: Some(MappedList { offsets, entries, slots: LazySlots::new(offsets.len() - 1) }), snap: Default::default() })
    }
    /// As ImageMap::adopt.
    pub fn adopt(&mut self, image: Self, retired: &mut Vec<Box<dyn Send>>) -> Adopt where T: Send + 'static {
        if self.mapped.is_some() { return Adopt::Mapped; }
        if self.owned.is_empty() { return Adopt::Empty; }
        if !*self.snap.get_mut() { return Adopt::Dirty; }
        let Some(m) = image.mapped else { return Adopt::Mismatch };
        if m.len() != self.owned.len() { return Adopt::Mismatch; }
        let n = m.len();
        retired.push(Box::new(std::mem::take(&mut self.owned)));
        self.mapped = Some(m);
        Adopt::Adopted(n)
    }
    pub fn len(&self) -> usize { self.mapped.as_ref().map_or(self.owned.len(), |m| m.len()) }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn get(&self, i: usize) -> Option<&T> {
        match &self.mapped { None => self.owned.get(i), Some(m) => m.get(i) }
    }
    pub fn last(&self) -> Option<&T> { self.len().checked_sub(1).and_then(|i| self.get(i)) }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator + '_ { (0..self.len()).map(|i| &self[i]) }
    /// Mutation needs the owned Vec: decode every element once, keeping the decoded ones.
    pub fn owned(&mut self) -> &mut Vec<T> {
        *self.snap.get_mut() = false;
        if let Some(mut m) = self.mapped.take() {
            self.owned = (0..m.len()).map(|i| match m.slots.take(i) {
                Some(x) => x,
                None => m.decode(i),
            }).collect();
        }
        &mut self.owned
    }
    /// owned elements' shallow heap bytes (mapped ones live in the image)
    pub fn heap_bytes(&self) -> usize { self.owned.capacity() * std::mem::size_of::<T>() }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> { self.owned().get_mut(i) }
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> { self.owned().iter_mut() }
    pub fn push(&mut self, x: T) { self.owned().push(x) }
    pub fn truncate(&mut self, n: usize) { self.owned().truncate(n) }
    pub fn retain(&mut self, f: impl FnMut(&T) -> bool) { self.owned().retain(f) }
}
impl<T: Codec> std::ops::Index<usize> for ImageVec<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T { self.get(i).expect("index in collected list") }
}
impl<T: Codec> std::ops::IndexMut<usize> for ImageVec<T> {
    fn index_mut(&mut self, i: usize) -> &mut T { &mut self.owned()[i] }
}
impl<T: Codec> Extend<T> for ImageVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, it: I) { self.owned().extend(it) }
}
impl<'a, T: Codec> IntoIterator for &'a ImageVec<T> {
    type Item = &'a T;
    type IntoIter = std::iter::Map<std::ops::Range<usize>, Box<dyn Fn(usize) -> &'a T + 'a>>;
    fn into_iter(self) -> Self::IntoIter { (0..self.len()).map(Box::new(move |i| &self[i])) }
}
impl<'a, T: Codec> IntoIterator for &'a mut ImageVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter { self.owned().iter_mut() }
}
impl<T: Codec> From<Vec<T>> for ImageVec<T> {
    fn from(owned: Vec<T>) -> Self { Self { owned, mapped: None, snap: Default::default() } }
}
/// Same encoding as a Vec.
impl<T: Codec> Codec for ImageVec<T> {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        for x in self.iter() { x.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Vec::<T>::get(r)?.into()) }
}

#[cfg(test)]
mod image_table_tests {
    use super::*;
    use crate::program::{DefId, Interner};

    // the store hands out 8-byte aligned payloads: leak an aligned copy
    fn mapped(bytes: Vec<u8>) -> &'static [u8] {
        let words: &'static mut [u64] = Box::leak(vec![0u64; bytes.len().div_ceil(8)].into_boxed_slice());
        let b: &'static mut [u8] = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, bytes.len()) };
        b.copy_from_slice(&bytes);
        b
    }
    fn image(write: impl FnOnce(&mut crate::image::Writer<'_>)) -> crate::image::Reader {
        let mut w = crate::image::Writer::default();
        write(&mut w);
        crate::image::Reader::new(mapped(w.finish())).unwrap()
    }

    #[test]
    fn image_map_reads_in_place_and_owns_on_first_mutation() {
        let mut owned: ImageMap<(u32, DefId), Vec<String>> = Default::default();
        for i in 0..500u32 { owned.insert((i % 7, DefId(i)), vec![format!("v{i}"); (i % 3) as usize]); }
        let r = image(|w| owned.write_image(w, &mut 0));
        let (mut k, mut id) = (0, 0);
        let mut m: ImageMap<(u32, DefId), Vec<String>> = ImageMap::read_image(&r, &mut k, &mut id).unwrap();
        assert_eq!((k, id), (4, 4));
        assert!(m.is_mapped());
        assert_eq!(m.len(), 500);
        assert_eq!(m.get(&(3, DefId(10))), Some(&vec!["v10".to_string()]));
        assert_eq!(m.get(&(4, DefId(10))), None);
        assert!(m.contains_key(&(2, DefId(499))) && !m.contains_key(&(3, DefId(500))));
        assert_eq!(m.decoded_values().count(), 1, "a lookup decodes only its entry");
        assert_eq!(crate::crate_image::encode(&m), crate::crate_image::encode(&owned));
        // re-recording copies undecoded entries and re-encodes decoded ones: same bytes
        let again = image(|w| m.write_image(w, &mut 0));
        let a: &[KeyRow] = again.slice(0, 0).unwrap();
        let b: &[KeyRow] = r.slice(0, 0).unwrap();
        assert!(a.iter().zip(b).all(|(x, y)| x.get() == y.get()) && a.len() == b.len());
        assert_eq!(again.bytes(2, 2), r.bytes(2, 2));
        m.get_mut(&(3, DefId(10))).unwrap().push("edited".into());
        assert!(!m.is_mapped());
        assert_eq!(m.len(), 500);
        assert_eq!(m.get(&(3, DefId(10))).unwrap().len(), 2);
        assert_eq!(m.get(&(1, DefId(1))), owned.get(&(1, DefId(1))));
    }

    #[test]
    fn heap_tables_adopt_their_own_image_unless_changed() {
        let mut retired: Vec<Box<dyn Send>> = Vec::new();
        // a map: written, read back, adopted: same entries, now in place, the heap table retired
        let mut owned: ImageMap<(u32, DefId), Vec<String>> = Default::default();
        for i in 0..500u32 { owned.insert((i % 7, DefId(i)), vec![format!("v{i}"); (i % 3) as usize]); }
        let want = crate::crate_image::encode(&owned);
        // never written: not adoptable
        let r0 = image(|w| owned.clone().write_image(w, &mut 0));
        assert_eq!(owned.adopt(ImageMap::read_image(&r0, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Dirty);
        let r = image(|w| owned.write_image(w, &mut 0));
        assert_eq!(owned.adopt(ImageMap::read_image(&r, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Adopted(500));
        assert_eq!(retired.len(), 1);
        assert!(owned.is_mapped() && owned.len() == 500);
        assert_eq!(crate::crate_image::encode(&owned), want);
        assert_eq!(owned.adopt(ImageMap::read_image(&r, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Mapped);
        // a mutation after the write keeps the heap table
        let mut edited: ImageMap<(u32, DefId), Vec<String>> = (0..50u32).map(|i| ((0, DefId(i)), vec![format!("e{i}")])).collect();
        let r = image(|w| edited.write_image(w, &mut 0));
        edited.insert((0, DefId(99)), vec![]);
        assert_eq!(edited.adopt(ImageMap::read_image(&r, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Dirty);
        assert!(!edited.is_mapped() && edited.len() == 51);
        // an image of another table is refused by its entry count
        let other: ImageMap<(u32, DefId), Vec<String>> = (0..10u32).map(|i| ((0, DefId(i)), vec![])).collect();
        let ro = image(|w| other.write_image(w, &mut 0));
        let mut same: ImageMap<(u32, DefId), Vec<String>> = (0..50u32).map(|i| ((0, DefId(i)), vec![])).collect();
        same.write_image(&mut crate::image::Writer::default(), &mut 0);
        assert_eq!(same.adopt(ImageMap::read_image(&ro, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Mismatch);
        // a list and a set
        let mut list: ImageVec<(u32, String)> = (0..300u32).map(|i| (i, format!("m{i}"))).collect::<Vec<_>>().into();
        let want = crate::crate_image::encode(&list);
        let r = image(|w| list.write_image(w, &mut 0));
        assert_eq!(list.adopt(ImageVec::read_image(&r, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Adopted(300));
        assert_eq!(crate::crate_image::encode(&list), want);
        let mut set: ImageSet<DefId> = Default::default();
        for i in 0..40u32 { set.insert(DefId(i)); }
        let r = image(|w| set.write_image(w, &mut 0));
        assert_eq!(set.adopt(ImageSet::read_image(&r, &mut 0, &mut 0).unwrap(), &mut retired), Adopt::Adopted(40));
        assert!(set.contains(&DefId(39)) && !set.contains(&DefId(40)));
        // the first mutation of an adopted table owns it again, as after a restore
        set.insert(DefId(77));
        assert!(set.contains(&DefId(77)) && set.contains(&DefId(0)) && set.len() == 41);
    }

    #[test]
    fn image_vec_reads_in_place_and_owns_on_first_mutation() {
        let owned: ImageVec<(u32, String)> = (0..300u32).map(|i| (i, format!("m{i}"))).collect::<Vec<_>>().into();
        let r = image(|w| owned.write_image(w, &mut 0));
        let mut v: ImageVec<(u32, String)> = ImageVec::read_image(&r, &mut 0, &mut 0).unwrap();
        assert_eq!(v.len(), 300);
        assert_eq!(v[299], (299, "m299".to_string()));
        assert!(v.get(300).is_none());
        assert_eq!(v.iter().map(|x| x.0).sum::<u32>(), (0..300).sum());
        assert_eq!(crate::crate_image::encode(&v), crate::crate_image::encode(&owned));
        v[5].1.push('!');
        v.push((300, "new".into()));
        assert_eq!((v.len(), v[5].1.as_str(), v[300].1.as_str()), (301, "m5!", "new"));
    }

    #[test]
    fn interner_image_keeps_ids_and_appends_after_them() {
        let mut syms = Interner::default();
        let names: Vec<String> = (0..1000).map(|i| format!("name{i}")).chain(["".to_string(), "self".to_string()]).collect();
        for n in &names { syms.intern(n); }
        let r = image(|w| syms.write_image(w, 0));
        let mut restored = Interner::from_image(&r, 0, 0).unwrap();
        assert!(restored.names().eq(syms.names()));
        for (i, n) in names.iter().enumerate() { assert_eq!(restored.get(n), Some(i as u32)); assert_eq!(restored.str(i as u32), n); }
        assert_eq!(restored.get("missing"), None);
        assert_eq!(restored.intern("name7"), 7);
        let fresh = restored.intern("fresh");
        assert_eq!(fresh, names.len() as u32);
        assert_eq!((restored.str(fresh), restored.get("fresh"), restored.len()), ("fresh", Some(fresh), names.len() + 1));
        let late = restored.intern_late("late one");
        assert_eq!((restored.str(late), restored.get("late one")), ("late one", Some(late)));
    }
}

impl<K: Codec + ImageKey + Clone + std::hash::Hash + Eq, V: Codec + Clone> Clone for ImageMap<K, V> {
    fn clone(&self) -> Self { self.iter().map(|(k, v)| (k.clone(), v.clone())).collect() }
}

/// A declaration set with the same demand-decoded representation as declaration maps.
pub struct ImageSet<K>(ImageMap<K, ()>);
impl<K: Codec + ImageKey + std::hash::Hash + Eq> ImageSet<K> {
    pub fn len(&self) -> usize { self.0.len() }
    pub fn insert(&mut self, key: K) -> bool { self.0.insert(key, ()).is_none() }
    pub fn remove(&mut self, key: &K) -> bool { self.0.remove(key).is_some() }
    pub fn contains(&self, key: &K) -> bool { self.0.contains_key(key) }
    pub fn retain(&mut self, mut f: impl FnMut(&K) -> bool) { self.0.retain(|k, _| f(k)); }
    pub fn iter(&self) -> impl Iterator<Item = &K> { self.0.keys() }
    pub fn write_image(&self, image: &mut crate::image::Writer<'_>, id: &mut u32) { self.0.write_image(image, id); }
    pub fn read_image(image: &crate::image::Reader, k: &mut usize, id: &mut u32) -> Option<Self> { Some(Self(ImageMap::read_image(image, k, id)?)) }
    pub fn adopt(&mut self, image: Self, retired: &mut Vec<Box<dyn Send>>) -> Adopt where K: Send + 'static { self.0.adopt(image.0, retired) }
}
impl<'a, K: Codec + ImageKey + std::hash::Hash + Eq> IntoIterator for &'a ImageSet<K> {
    type Item = &'a K;
    type IntoIter = std::iter::Map<ImageMapIter<'a, K, ()>, fn((&'a K, &'a ())) -> &'a K>;
    fn into_iter(self) -> Self::IntoIter { self.0.iter().map(|(k, _)| k) }
}
impl<K: Codec + ImageKey + Ord + std::hash::Hash + Eq> Codec for ImageSet<K> {
    fn put(&self, w: &mut Vec<u8>) { self.0.put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self(Codec::get(r)?)) }
}

impl<K> Default for ImageSet<K> { fn default() -> Self { Self(ImageMap::default()) } }
