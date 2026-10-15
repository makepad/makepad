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
crate::gen_local! {
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

    pub fn collected_writer(&self) -> Option<crate::image::Writer<'_>> {
        let mut meta = Vec::new();
        let kinds: Vec<RegionKind> = self.regions.kinds.iter().copied().collect();
        let args: Vec<std::sync::Arc<[RegionId]>> = self.regions.args.iter().cloned().collect();
        kinds.put(&mut meta); args.put(&mut meta);
        self.consts.lock().unwrap_or_else(|e| e.into_inner()).put(&mut meta);
        self.free_regions.load(Ordering::Relaxed).put(&mut meta);
        for t in [self.unit, self.bool_, self.char_, self.str_, self.never, self.error,
            self.i32_, self.i64_, self.u8_, self.u32_, self.u64_, self.usize_, self.isize_,
            self.f32_, self.f64_, self.str_ref] { t.put(&mut meta); }
        let mapped_rows = self.image.as_ref().map_or(&[][..], |image| image.rows);
        let rows: Vec<Entry> = (mapped_rows.len()..self.len()).map(|i| *self.entry(TyId(i as u32))).collect();
        let n = mapped_rows.len() + rows.len();
        if !rows.iter().all(|r| r.next as usize <= n && (r.erased.0 as usize) < n) { return None; }
        let buckets: Vec<u32> = self.buckets.iter().enumerate().filter_map(|(i, head)| {
            let head = head.load(Ordering::Relaxed);
            (head != 0).then_some([i as u32, head])
        }).flatten().collect();
        // list bucket heads before the words: every head (and the rows taken above) names a list
        // published before the words are copied, so the image never holds a head past its words
        let list_buckets = self.lists.bucket_pairs();
        let mapped_words = self.lists.mapped;
        let words = self.lists.tail_words(mapped_words.len());
        let mut image = crate::image::Writer::default();
        image.add_bytes(0, meta); image.add_mapped_prefix(1, mapped_rows, &rows);
        image.add_mapped_prefix(2, mapped_words, &words);
        image.add_owned(3, &buckets);
        image.add_owned(4, &list_buckets);
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
            a.kind == b.kind && a.next == b.next && a.erased == b.erased && a.meta == b.meta
        };
        if n == 0 || n > self.len() || !same(0) || !same(n - 1) || !same(n / 2) { return Adopt::Mismatch; }
        let whole = n >> CHUNK_BITS;
        for c in 0..whole {
            let p = std::mem::replace(self.chunks[c].get_mut(), std::ptr::null_mut());
            if !p.is_null() { unsafe { std::alloc::dealloc(p.cast(), std::alloc::Layout::array::<Entry>(CHUNK).unwrap()) }; }
        }
        self.image = Some(MappedTypes { rows });
        Adopt::Adopted(whole << CHUNK_BITS)
    }

    pub fn restore_collected(bytes: &'static [u8]) -> Option<Self> {
        let image = crate::image::Reader::new(bytes)?;
        let rows: &'static [Entry] = image.slice(1, 1)?;
        let words: &'static [u32] = image.slice(2, 2)?;
        let buckets: &'static [u32] = image.slice(3, 3)?;
        let list_buckets: &'static [u32] = image.slice(4, 4)?;
        let n = rows.len();
        // The checksummed, same-build snapshot is constructed from valid interner entries.
        // Row relationships were certified at publication; demand decoding still uses
        // bounds-checked slices, without a pass over every type on each warm boot.
        if n > MAX_CHUNKS * CHUNK || buckets.len() % 2 != 0 { return None; }
        let mut r = Decode::new(image.bytes(0, 0)?);
        let region_kinds: Vec<RegionKind> = Codec::get(&mut r)?;
        let args: Vec<std::sync::Arc<[RegionId]>> = Codec::get(&mut r)?;
        // Zero is the implicit erased/empty value, not an element of either arena.
        if region_kinds.contains(&RegionKind::ReErased) || args.iter().any(|a| a.is_empty())
            || args.iter().flat_map(|a| a.iter()).any(|id| id.0 as usize > region_kinds.len()) { return None; }
        let mut t = Types::new();
        if n < t.len() { return None; }
        t.regions = Regions::restored(region_kinds, args);
        *t.consts.get_mut().ok()? = Codec::get(&mut r)?;
        *t.free_regions.get_mut() = bool::get(&mut r)?;
        for dst in [&mut t.unit, &mut t.bool_, &mut t.char_, &mut t.str_, &mut t.never,
            &mut t.error, &mut t.i32_, &mut t.i64_, &mut t.u8_, &mut t.u32_, &mut t.u64_,
            &mut t.usize_, &mut t.isize_, &mut t.f32_, &mut t.f64_, &mut t.str_ref] {
            *dst = Codec::get(&mut r)?;
            if dst.0 as usize >= n { return None; }
        }
        if !r.bytes.is_empty() { return None; }
        for b in &mut t.buckets { *b.get_mut() = 0; }
        for pair in buckets.chunks_exact(2) {
            if pair[0] as usize >= t.buckets.len() || pair[1] as usize > n { return None; }
            *t.buckets[pair[0] as usize].get_mut() = pair[1];
        }
        t.lists = list::Lists::restored(words, list_buckets)?;
        // Retire new()'s primitive arena. Mapped IDs need no Entry allocations;
        // subsequent interning starts after the image and retains the same append-only IDs.
        let first = *t.chunks[0].get_mut();
        unsafe { std::alloc::dealloc(first.cast(), std::alloc::Layout::array::<Entry>(CHUNK).ok()?); }
        *t.chunks[0].get_mut() = std::ptr::null_mut();
        t.image = Some(MappedTypes { rows });
        *t.len.get_mut() = n as u32;
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
        // an image of other types at the same ids is refused
        let other = Types::new();
        let _ = other.intern(TyKind::Slice(other.u8_));
        let theirs: &'static [u8] = Box::leak(other.collected_image().unwrap().into_boxed_slice());
        let mut mine = Types::new();
        let _ = mine.intern(TyKind::Slice(mine.u32_));
        assert_eq!(mine.adopt_collected(theirs), Adopt::Mismatch);
    }
}
