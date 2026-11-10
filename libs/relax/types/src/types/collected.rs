//! The interner's IDs are part of a collected graph, including its resolved array lengths.
//! Restore entries verbatim instead of interning them again (which would change those IDs).
use super::*;
use crate::crate_image::{Codec, Decode};
use std::sync::atomic::Ordering;

crate::collected_id!(UniverseIndex, BoundVar);

struct Semantic {
    types: *const Types,
    hashes: Vec<Option<u128>>,
}
crate::memo_local! {
    static SEMANTIC: std::cell::RefCell<Option<Semantic>>;
}
struct SemanticGuard(Option<Semantic>);
impl Drop for SemanticGuard {
    fn drop(&mut self) { SEMANTIC.with(|s| { s.replace(self.0.take()); }); }
}
impl Types {
    /// Verification-only encoding uses structural identities, including lifetimes and
    /// tagged const-generic array lengths. Parallel collection assigns nondeterministic IDs.
    pub fn with_semantic_ids<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = SemanticGuard(SEMANTIC.with(|s| s.replace(Some(Semantic {
            types: self, hashes: vec![None; self.len()],
        }))));
        f()
    }
}
impl Codec for TyId {
    fn put(&self, w: &mut Vec<u8>) {
        // Cache publication can run from atexit, after verification's TLS was destroyed.
        let semantic = SEMANTIC.try_with(|s| s.borrow().as_ref().map(|s| (s.types, s.hashes[self.0 as usize]))).ok().flatten();
        let Some((types, hash)) = semantic else { self.0.put(w); return; };
        let hash = hash.unwrap_or_else(|| {
            // The scoped guard holds a borrow of Types for every use of this pointer.
            let types = unsafe { &*types };
            let mut bytes = Vec::new();
            types.kind(*self).put(&mut bytes);
            types.fl(*self).put(&mut bytes); types.outer_exclusive_binder(*self).put(&mut bytes); types.depth(*self).put(&mut bytes);
            let mut h = crate::sip::Sip128::new(); h.write(&bytes); let hash = h.finish128();
            SEMANTIC.with(|s| s.borrow_mut().as_mut().unwrap().hashes[self.0 as usize] = Some(hash));
            hash
        });
        hash.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self(u32::get(r)?)) }
}
/// A list's id; under `with_semantic_ids` its elements (as the Vec it was).
impl<T: ListElem + Codec> Codec for List<T> {
    fn put(&self, w: &mut Vec<u8>) {
        let types = SEMANTIC.try_with(|s| s.borrow().as_ref().map(|s| s.types)).ok().flatten();
        if let Some(types) = types { unsafe { &*types }.list(*self).to_vec().put(w); } else { self.raw().put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self::from_raw(u32::get(r)?)) }
}
impl Codec for RegionId {
    fn put(&self, w: &mut Vec<u8>) {
        let types = SEMANTIC.try_with(|s| s.borrow().as_ref().map(|s| s.types)).ok().flatten();
        if let Some(types) = types { unsafe { &*types }.region_kind(*self).put(w); } else { self.0.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self(u32::get(r)?)) }
}
impl Codec for RegionArgsId {
    fn put(&self, w: &mut Vec<u8>) {
        let types = SEMANTIC.try_with(|s| s.borrow().as_ref().map(|s| s.types)).ok().flatten();
        if let Some(types) = types { unsafe { &*types }.region_args(*self).put(w); } else { self.0.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self(u32::get(r)?)) }
}
crate::collected_enum!(BoundTyKind; 0 => Anon, 1 => Param(d));
crate::collected_fields!(BoundTy, var, kind);
crate::collected_fields!(PlaceholderType, universe, bound);
crate::collected_enum!(LateParamKind; 0 => Anon(n), 1 => Named(d), 2 => ClosureEnv);
crate::collected_enum!(Safety; 0 => Safe, 1 => Unsafe);
crate::collected_enum!(Abi; 0 => Rust, 1 => RustCall, 2 => C, 3 => CUnwind,
    4 => System, 5 => SystemUnwind, 6 => RelaxIntrinsic,
    7 => SysV64, 8 => SysV64Unwind, 9 => Win64, 10 => Win64Unwind);
crate::collected_fields!(FnHeader, safety, abi, c_variadic);
crate::collected_fields!(ClosureArgs, parent_args, closure_kind_ty, closure_sig_as_fn_ptr_ty, tupled_upvars_ty);
crate::collected_fields!(AliasTy, kind, args, regions);
crate::collected_fields!(LifetimeId, owner, local);
crate::collected_enum!(BoundRegionKind; 0 => Anon, 1 => Named(d));
impl Codec for AliasTyKind {
    fn put(&self, w: &mut Vec<u8>) {
        (match self { Self::Projection { .. } => 0u8, Self::Inherent { .. } => 1, Self::Opaque { .. } => 2, Self::Free { .. } => 3 }).put(w);
        self.def_id().put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let tag = u8::get(r)?; let def_id = Codec::get(r)?;
        Some(match tag { 0 => Self::Projection { def_id }, 1 => Self::Inherent { def_id }, 2 => Self::Opaque { def_id }, 3 => Self::Free { def_id }, _ => return None })
    }
}
impl Codec for crate::consts::Const {
    fn put(&self, w: &mut Vec<u8>) {
        let types = SEMANTIC.try_with(|s| s.borrow().as_ref().map(|s| s.types)).ok().flatten();
        if let Some(types) = types { unsafe { &*types }.const_kind(*self).put(w); } else { self.0.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self(u32::get(r)?)) }
}
crate::collected_enum!(TyKind;
    0 => Bool, 1 => Char, 2 => Str, 3 => Int(p), 4 => Float(p), 5 => Never,
    6 => Tuple(v), 7 => Array(t,n), 8 => Slice(t), 9 => Ref(m,t,r), 10 => Ptr(m,t),
    11 => Adt(d,v,r), 12 => FnPtr(v,t,h,n), 13 => FnDef(d,v,r), 14 => Param(n),
    15 => Infer(n), 16 => Closure(f,e,c), 17 => Alias(r,a),
    18 => Dyn(d,v,b,a,r,n,auto), 19 => Const(n), 20 => Foreign(d),
    21 => Error, 22 => Bound(n,b), 23 => Placeholder(p), 24 => IntVar(n), 25 => FloatVar(n));
impl Codec for RegionScope {
    fn put(&self, w: &mut Vec<u8>) {
        match self {
            Self::Item(d) => { 0u8.put(w); d.put(w); }
            Self::Closure { owner, expr } => { 1u8.put(w); owner.put(w); expr.put(w); }
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(match u8::get(r)? {
            0 => Self::Item(Codec::get(r)?),
            1 => Self::Closure { owner: Codec::get(r)?, expr: Codec::get(r)? },
            _ => return None,
        })
    }
}
impl Codec for RegionKind {
    fn put(&self, w: &mut Vec<u8>) {
        match self {
            Self::ReEarlyParam(n) => { 0u8.put(w); n.put(w); }
            Self::ReBound(d,v) => { 1u8.put(w); d.put(w); v.put(w); }
            Self::ReLateParam(s,k) => { 2u8.put(w); s.put(w); k.put(w); }
            Self::ReStatic => 3u8.put(w),
            Self::ReVar(n) => { 4u8.put(w); n.put(w); }
            Self::RePlaceholder { universe, var } => { 5u8.put(w); universe.put(w); var.put(w); }
            Self::ReErased => 6u8.put(w),
            Self::ReError => 7u8.put(w),
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(match u8::get(r)? {
            0 => Self::ReEarlyParam(Codec::get(r)?),
            1 => Self::ReBound(Codec::get(r)?, Codec::get(r)?),
            2 => Self::ReLateParam(Codec::get(r)?, Codec::get(r)?),
            3 => Self::ReStatic,
            4 => Self::ReVar(Codec::get(r)?),
            5 => Self::RePlaceholder { universe: Codec::get(r)?, var: Codec::get(r)? },
            6 => Self::ReErased,
            7 => Self::ReError,
            _ => return None,
        })
    }
}

/// The image's interner rows, mapped as they are (Entry is plain data; same build only):
/// a warm boot reads a type without decoding or allocating it.
pub(super) struct MappedTypes {
    rows: &'static [Entry],
}
impl MappedTypes {
    #[inline(always)]
    pub(super) fn len(&self) -> usize { self.rows.len() }
    #[inline(always)]
    pub(super) fn entry(&self, i: usize) -> &Entry { unsafe { self.rows.get_unchecked(i) } }
}

impl Types {
    pub fn collected_image(&self) -> Option<Vec<u8>> {
        Some(self.collected_writer()?.finish())
    }

    /// The settle rebuild (relax-compiler settle_rebuild.rs): `self` was restored from an image of
    /// `live` encoded while compiles ran, so it holds the rows up to that encode's count. With
    /// every type check held off (a front write), the rows interned since are copied as they are (same ids)
    /// and the small tables -- regions, constants, the list arena, the intern tables -- are copied
    /// whole, so the restored interner equals the live one. Nothing of `live`'s heap is shared
    /// (it is dropped after the rebuild).
    /// None: `self` holds rows `live` does not.
    pub fn top_up_from(&mut self, live: &Types) -> Option<()> {
        let (n0, n1) = (self.len(), live.len());
        if n0 > n1 { return None; }
        let layout = std::alloc::Layout::array::<Entry>(CHUNK).ok()?;
        for i in n0..n1 {
            let row = *live.entry(TyId(i as u32));
            let c = self.chunks[i >> CHUNK_BITS].get_mut();
            if c.is_null() {
                *c = unsafe { std::alloc::alloc(layout) as *mut Entry };
                if c.is_null() { std::alloc::handle_alloc_error(layout); }
            }
            unsafe { c.add(i & (CHUNK - 1)).write(row) };
        }
        *self.len.get_mut() = n1 as u32;
        self.index = index::Index::copy_of(&live.index);
        let kinds: Vec<RegionKind> = live.regions.kinds.iter().copied().collect();
        let args: Vec<std::sync::Arc<[RegionId]>> = live.regions.args.iter().map(|a| std::sync::Arc::from(&a[..])).collect();
        self.regions = Regions::restored(kinds, args);
        // (through the codec: a constant's value tree is a fresh allocation)
        let mut w = Vec::new();
        live.consts.put(&mut w);
        self.consts = Codec::get(&mut Decode::new(&w))?;
        *self.free_regions.get_mut() = live.free_regions.load(Ordering::Relaxed);
        self.lists = list::Lists::copy_of(&live.lists);
        Some(())
    }

    /// The interner's image. Consistent while other threads intern (a mid-run encode runs
    /// with compiles running): the row count and the intern table are read first, under the table's
    /// lock, so every row below the count is in that table; every table read after them holds what
    /// those rows name. A row claimed since lies past the count and its slot is written empty,
    /// which cuts no other row's probe run (slots are claimed in id order).
    pub fn collected_writer(&self) -> Option<crate::image::Writer<'_>> {
        self.collected_writer_with(&|| false)
    }
    /// collected_writer, asking `stop` every arena chunk of rows and through the intern tables
    /// (None: it said yes)
    pub fn collected_writer_with(&self, stop: &dyn Fn() -> bool) -> Option<crate::image::Writer<'_>> {
        let (n, table) = {
            let _g = self.index.lock();
            (self.len(), self.index.table())
        };
        let mut meta = Vec::new();
        let kinds: Vec<RegionKind> = self.regions.kinds.iter().copied().collect();
        let args: Vec<std::sync::Arc<[RegionId]>> = self.regions.args.iter().cloned().collect();
        kinds.put(&mut meta); args.put(&mut meta);
        self.consts.put(&mut meta);
        self.free_regions.load(Ordering::Relaxed).put(&mut meta);
        for t in [self.unit, self.bool_, self.char_, self.str_, self.never, self.error,
            self.i32_, self.i64_, self.u8_, self.u32_, self.u64_, self.usize_, self.isize_,
            self.f32_, self.f64_, self.str_ref] { t.put(&mut meta); }
        if stop() { return None; }
        let mapped_rows = self.image.as_ref().map_or(&[][..], |image| image.rows);
        // the rows past the mapped ones, as the arena chunks hold them (no copy): a row below
        // `n` is written before `len` publishes it and never changes, and a chunk is freed only
        // with exclusive access (adopt_collected)
        let mut runs: Vec<&[Entry]> = Vec::new();
        let mut i = mapped_rows.len();
        while i < n {
            let end = n.min((i | (CHUNK - 1)) + 1);
            let c = self.chunks[i >> CHUNK_BITS].load(Ordering::Acquire);
            if c.is_null() { return None; }
            let run = unsafe { std::slice::from_raw_parts(c.add(i & (CHUNK - 1)) as *const Entry, end - i) };
            for r in run {
                if (r.erased.0 as usize) >= n { return None; }
            }
            runs.push(run);
            if stop() { return None; }
            i = end;
        }
        let slots = table.snapshot(|s| (s & ID_MASK) as usize <= n, stop)?;
        // the list table after the rows taken above: every list they name is below its end
        let (list_end, list_slots) = self.lists.image_slots(stop)?;
        let mapped_words = self.lists.mapped;
        let words = self.lists.tail_word_bytes(mapped_words.len(), list_end, stop)?;
        let mut image = crate::image::Writer::default();
        image.add_bytes(0, meta); image.add_parts(1, mapped_rows, runs);
        image.add_mapped_prefix(2, mapped_words, words);
        image.add_owned(3, &slots);
        image.add_owned(4, &list_slots);
        Some(image)
    }

    /// The running interner reads the rows of `bytes` (the image `collected_writer` made from
    /// it) in place, as a restored one does: the arena chunks that lie wholly below the image's
    /// row count are freed (type ids and the rows never change). Needs exclusive access, as any
    /// mutation does: no reader may hold a type's row. Mismatch: not this interner's image.
    pub fn adopt_collected(&mut self, bytes: &'static [u8]) -> crate::collected::Adopt {
        use crate::collected::Adopt;
        if self.image.is_some() { return Adopt::Mapped; }
        let Some(image) = crate::image::Reader::new(bytes) else { return Adopt::Mismatch };
        let Some(rows) = image.slice::<Entry>(1, 1) else { return Adopt::Mismatch };
        let n = rows.len();
        let same = |i: usize| {
            let (a, b) = (self.entry(TyId(i as u32)), &rows[i]);
            a.kind == b.kind && a.hash == b.hash && a.erased == b.erased && a.meta == b.meta
        };
        if n == 0 || n > self.len() || !same(0) || !same(n - 1) || !same(n / 2) { return Adopt::Mismatch; }
        let whole = n >> CHUNK_BITS;
        for c in 0..whole {
            let p = std::mem::replace(self.chunks[c].get_mut(), std::ptr::null_mut());
            if !p.is_null() { unsafe { std::alloc::dealloc(p.cast(), std::alloc::Layout::array::<Entry>(CHUNK).unwrap()) }; }
        }
        self.image = Some(MappedTypes { rows });
        self.index.free_retired();
        self.lists.free_retired();
        Adopt::Adopted(whole << CHUNK_BITS)
    }


    pub fn restore_collected(bytes: &'static [u8]) -> Option<Self> {
        let image = crate::image::Reader::new(bytes)?;
        let rows: &'static [Entry] = image.slice(1, 1)?;
        let words: &'static [u32] = image.slice(2, 2)?;
        let slots: &'static [u32] = image.slice(3, 3)?;
        let list_slots: &'static [u64] = image.slice(4, 4)?;
        let n = rows.len();
        // The checksummed, same-build snapshot is constructed from valid interner entries.
        // Row relationships were certified at publication; demand decoding still uses
        // bounds-checked slices, without a pass over every type on each warm boot.
        if n > MAX_CHUNKS * CHUNK { return None; }
        let mut r = Decode::new(image.bytes(0, 0)?);
        let region_kinds: Vec<RegionKind> = Codec::get(&mut r)?;
        let args: Vec<std::sync::Arc<[RegionId]>> = Codec::get(&mut r)?;
        // Zero is the implicit erased/empty value, not an element of either arena.
        if region_kinds.contains(&RegionKind::ReErased) || args.iter().any(|a| a.is_empty())
            || args.iter().flat_map(|a| a.iter()).any(|id| id.0 as usize > region_kinds.len()) { return None; }
        let mut t = Types::new();
        if n < t.len() { return None; }
        t.regions = Regions::restored(region_kinds, args);
        t.consts = Codec::get(&mut r)?;
        *t.free_regions.get_mut() = bool::get(&mut r)?;
        for dst in [&mut t.unit, &mut t.bool_, &mut t.char_, &mut t.str_, &mut t.never,
            &mut t.error, &mut t.i32_, &mut t.i64_, &mut t.u8_, &mut t.u32_, &mut t.u64_,
            &mut t.usize_, &mut t.isize_, &mut t.f32_, &mut t.f64_, &mut t.str_ref] {
            *dst = Codec::get(&mut r)?;
            if dst.0 as usize >= n { return None; }
        }
        if !r.bytes.is_empty() { return None; }
        t.index = index::Index::restored(slots, |s| s & ID_MASK != 0 && (s & ID_MASK) as usize <= n)?;
        t.lists = list::Lists::restored(words, list_slots)?;
        // Retire new()'s primitive arena. Mapped IDs need no Entry allocations;
        // subsequent interning starts after the image and retains the same append-only IDs.
        let first = *t.chunks[0].get_mut();
        unsafe { std::alloc::dealloc(first.cast(), std::alloc::Layout::array::<Entry>(CHUNK).ok()?); }
        *t.chunks[0].get_mut() = std::ptr::null_mut();
        t.image = Some(MappedTypes { rows });
        *t.len.get_mut() = n as u32;
        // `new`'s pre-interned variables are the image's first rows and regions (same build)
        if !t.preinterned_hold() { return None; }
        Some(t)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round38_mapped_native_types_keep_ids_and_semantics() {
        let tys = Types::new();
        let region = tys.intern_region(RegionKind::ReBound(1, BoundVar(2)));
        let reference = tys.intern(TyKind::Ref(false, tys.u8_, region));
        let signature = tys.intern(TyKind::FnPtr(tys.mk_list(&[reference]), reference, FnHeader::RUST, 3));
        let scalar = tys.const_usize(37);
        let array = tys.mk_array(reference, scalar);
        let tuple = tys.intern(TyKind::Tuple(tys.mk_list(&[array, signature])));
        let regions = tys.intern_region_args(&[region]);
        let alias = tys.intern(TyKind::Alias(crate::consts::IsRigid::No,
            AliasTy { kind: AliasTyKind::Opaque { def_id: crate::program::DefId(42) }, args: tys.mk_list(&[tuple]), regions }));
        let closure = tys.intern(TyKind::Closure(0, 7, ClosureArgs {
            parent_args: alias, closure_kind_ty: tys.i32_, closure_sig_as_fn_ptr_ty: signature, tupled_upvars_ty: tuple,
        }));
        let image = tys.collected_image().unwrap();
        let restored = Types::restore_collected(Box::leak(image.into_boxed_slice())).unwrap();
        for ty in [reference, signature, array, tuple, alias, closure] {
            assert!(restored.kind(ty) == tys.kind(ty));
            assert_eq!(restored.intern(*tys.kind(ty)), ty);
            assert_eq!(restored.with_semantic_ids(|| crate::crate_image::encode(&ty)),
                tys.with_semantic_ids(|| crate::crate_image::encode(&ty)));
        }
        assert_eq!(restored.const_usize(37), scalar);
        assert_eq!(restored.region_kind(region), tys.region_kind(region));
        assert_eq!(restored.region_args(regions), tys.region_args(regions));
        let new = restored.mk_array(reference, restored.const_usize(38));
        assert!(new.0 as usize >= tys.len());
        let second = restored.collected_image().unwrap();
        let again = Types::restore_collected(Box::leak(second.into_boxed_slice())).unwrap();
        assert!(again.kind(new) == restored.kind(new));
    }


    #[test]
    fn running_interner_adopts_its_image_and_keeps_interning() {
        use crate::collected::Adopt;
        let mut tys = Types::new();
        let u8_ref = tys.intern(TyKind::Ref(false, tys.u8_, tys.intern_region(RegionKind::ReStatic)));
        let arrays: Vec<TyId> = (0..40_000u64).map(|i| { let n = tys.const_usize(i); tys.mk_array(u8_ref, n) }).collect();
        assert!(tys.len() > 2 * CHUNK + 1000);
        let kinds: Vec<TyKind> = arrays.iter().step_by(997).map(|t| *tys.kind(*t)).collect();
        let before = tys.len();
        let bytes: &'static [u8] = Box::leak(tys.collected_image().unwrap().into_boxed_slice());
        // types made after the image was written stay in the arena
        let late = tys.mk_array(u8_ref, tys.const_usize(1_000_000));
        let a = tys.adopt_collected(bytes);
        let whole = before >> CHUNK_BITS;
        assert_eq!(a, Adopt::Adopted(whole << CHUNK_BITS), "{a:?}");
        assert!(whole >= 2 && (0..whole).all(|c| tys.chunks[c].load(Ordering::Relaxed).is_null()));
        assert!(!tys.chunks[whole].load(Ordering::Relaxed).is_null());
        assert_eq!(tys.adopt_collected(bytes), Adopt::Mapped);
        for (t, k) in arrays.iter().step_by(997).zip(&kinds) { assert!(tys.kind(*t) == k); }
        // interning finds the mapped rows and the late one, and appends after both
        assert_eq!(tys.mk_array(u8_ref, tys.const_usize(5)), arrays[5]);
        assert_eq!(tys.mk_array(u8_ref, tys.const_usize(1_000_000)), late);
        assert!(tys.len() >= before && tys.mk_array(u8_ref, tys.const_usize(2_000_000)).0 as usize >= before + 1);
        // the adopted interner's image (its mapped rows, then the arena's from inside a chunk)
        // restores to the same types
        let again: &'static [u8] = Box::leak(tys.collected_image().unwrap().into_boxed_slice());
        let restored = Types::restore_collected(again).unwrap();
        assert_eq!(restored.len(), tys.len());
        for t in arrays.iter().step_by(997).chain([&late]) { assert!(restored.kind(*t) == tys.kind(*t)); }
        // an image of other types at the same ids is refused
        let other = Types::new();
        let _ = other.intern(TyKind::Slice(other.u8_));
        let theirs: &'static [u8] = Box::leak(other.collected_image().unwrap().into_boxed_slice());
        let mut mine = Types::new();
        let _ = mine.intern(TyKind::Slice(mine.u32_));
        assert_eq!(mine.adopt_collected(theirs), Adopt::Mismatch);
    }

    /// The settle rebuild's restore: an image taken while the interner was still growing (here: the
    /// rows interned after it), topped up from the live interner, equals it -- same ids, lookups
    /// find the topped-up rows, lists and constants interned later resolve.
    #[test]
    fn restored_interner_topped_up_from_the_live_one() {
        let tys = Types::new();
        let r = tys.intern_region(RegionKind::ReStatic);
        let base = tys.intern(TyKind::Ref(false, tys.u8_, r));
        let early: Vec<TyId> = (0..3_000u64).map(|i| tys.mk_array(base, tys.const_usize(i))).collect();
        let image = tys.collected_image().unwrap();
        let late: Vec<TyId> = (3_000..9_000u64).map(|i| {
            let a = tys.mk_array(base, tys.const_usize(i));
            tys.intern(TyKind::Tuple(tys.mk_list(&[a, base])))
        }).collect();
        let late_region = tys.intern_region_args(&[r, tys.intern_region(RegionKind::ReEarlyParam(3))]);
        let mut restored = Types::restore_collected(Box::leak(image.into_boxed_slice())).unwrap();
        assert!(restored.len() < tys.len());
        restored.top_up_from(&tys).unwrap();
        assert_eq!(restored.len(), tys.len());
        assert_eq!(restored.region_counts(), tys.region_counts());
        assert_eq!(restored.list_const_counts(), tys.list_const_counts());
        for &t in early.iter().chain(&late) {
            assert!(restored.kind(t) == tys.kind(t));
            assert_eq!(restored.intern(*tys.kind(t)), t);
        }
        assert_eq!(restored.region_args(late_region), tys.region_args(late_region));
        assert_eq!(restored.const_usize(8_999), tys.const_usize(8_999));
        let last = *late.last().unwrap();
        let TyKind::Tuple(l) = *tys.kind(last) else { panic!() };
        assert_eq!(restored.mk_list(tys.list(l)), l);
        assert_eq!(restored.len(), tys.len(), "nothing interned twice");
    }

    /// The intern tables double while other threads intern and look up: every thread gets one
    /// id per kind and list, and an image taken meanwhile restores to an interner that finds
    /// every type and list it holds (none is interned again).
    #[test]
    fn intern_tables_double_under_concurrent_interning_and_images() {
        const KINDS: u32 = 60_000;
        let tys = Types::new();
        let id_of = |i: u32| tys.intern(TyKind::Tuple(tys.mk_list(&[tys.intern(TyKind::Infer(i)), tys.u8_])));
        let image = std::thread::scope(|s| {
            // every thread walks the same kinds from its own start, so most are raced
            let threads: Vec<_> = (0..8u32).map(|t| {
                let id_of = &id_of;
                s.spawn(move || (0..KINDS).map(|i| ((i + t * 7_001) % KINDS, id_of((i + t * 7_001) % KINDS))).collect::<Vec<_>>())
            }).collect();
            std::thread::sleep(std::time::Duration::from_millis(2));
            let image = tys.collected_image().unwrap();
            let ids: Vec<Vec<(u32, TyId)>> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            for thread in &ids[1..] {
                let mut by_kind: Vec<(u32, TyId)> = thread.clone();
                by_kind.sort_by_key(|p| p.0);
                for (i, id) in by_kind { assert_eq!(id, id_of(i)); }
            }
            image
        });
        let restored = Types::restore_collected(Box::leak(image.into_boxed_slice())).unwrap();
        let n = restored.len();
        assert!(restored.list_const_counts().0 <= tys.list_const_counts().0);
        for i in 0..n {
            let (t, k) = (TyId(i as u32), *restored.kind(TyId(i as u32)));
            assert!(k == *tys.kind(t));
            assert_eq!(restored.lookup(&k), Some(t), "type {i} is in the image's table");
            if let TyKind::Tuple(l) = k { assert_eq!(restored.mk_list(restored.list(l)), l); }
        }
        assert_eq!(restored.len(), n, "nothing interned twice");
        // a table that grew freed nothing a lookup could still read, and frees it when not shared
        let mut tys = tys;
        let before = tys.index.bytes();
        tys.index.free_retired();
        assert!(tys.index.bytes() < before);
        assert_eq!(tys.intern(*tys.kind(TyId(0))), TyId(0));
    }
}
