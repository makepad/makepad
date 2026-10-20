// ported from N:rustc_ty_utils/src/layout.rs @8d1a764
//! Machine layout of types: size, alignment, field offsets and the scalar
//! "leaves" a small aggregate is split into when it lives in registers.

mod closure;
pub use closure::ClosureStorage;

use crate::next_solver::TraitRef;
use crate::consts::GenericArg;
use crate::program::Prim;
use relax_backend::rir::{Cls, Mem};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Leaf {
    pub off: u32,
    pub mem: Mem,
    pub cls: Cls,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Layout {
    pub size: u32,
    pub align: u32,
    /// struct/tuple field offsets (variant 0 for structs), in declaration order
    pub fields: Vec<u32>,
    /// enums: the tag field's offset/width (rustc's tag scalar, also for niche encodings)
    pub tag: Option<(u32, Mem)>,
    /// how the tag field encodes the variant (rustc_abi TagEncoding / Variants::Single)
    pub enc: TagEnc,
    /// enums: per-variant field offsets
    pub variant_fields: Vec<Vec<u32>>,
    /// Some when the value can live in registers (<= MAX_LEAVES scalars, no arrays); leaves in
    /// declaration order of the fields
    pub leaves: Option<Vec<Leaf>>,
    /// the largest niche (rustc_abi LayoutData::largest_niche): a scalar with invalid values
    pub niche: Option<Niche>,
    /// no value of the type exists (`!`, empty enums, a field of such a type)
    pub uninhabited: bool,
    /// Live sessions (M10): the offset of the hidden extension word of a live-editable struct
    /// (a pointer to the fields edits added, null until one is used; hotreload.md section 13).
    /// Fields in its extension block have offset u32::MAX in `fields`.
    pub ext: Option<u32>,
}

/// A scalar of `bytes` at `off` whose valid values are `start..=end` (wrapping; rustc_abi
/// Niche with its WrappingRange).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Niche {
    pub off: u32,
    pub bytes: u32,
    pub start: u128,
    pub end: u128,
}

impl Niche {
    fn max(&self) -> u128 {
        if self.bytes >= 16 {
            u128::MAX
        } else {
            (1u128 << (self.bytes * 8)) - 1
        }
    }

    /// Values outside the valid range (rustc_abi Niche::available).
    pub fn available(&self) -> u128 {
        let m = self.max();
        self.start.wrapping_sub(self.end.wrapping_add(1)) & m
    }

    /// Reserves `count` invalid values: (the first, the niche with the reduced invalid range)
    /// (rustc_abi Niche::reserve, its bound selection included).
    fn reserve(&self, count: u128) -> Option<(u128, Niche)> {
        let m = self.max();
        let (vs, ve) = (self.start, self.end);
        let available = vs.wrapping_sub(ve).wrapping_sub(1) & m;
        if count == 0 || count > available {
            return None;
        }
        let move_start = || {
            let start = vs.wrapping_sub(count) & m;
            Some((start, Niche { start, ..*self }))
        };
        let move_end = || {
            let start = ve.wrapping_add(1) & m;
            let end = ve.wrapping_add(count) & m;
            Some((start, Niche { end, ..*self }))
        };
        let distance_end_zero = m - ve;
        let sext = |x: u128| -> i128 {
            let bits = self.bytes * 8;
            if bits >= 128 {
                x as i128
            } else {
                let sh = 128 - bits;
                ((x << sh) as i128) >> sh
            }
        };
        if count == 1 && !(vs == 0 && ve == 1) {
            let next_up = sext(ve.wrapping_add(1) & m).unsigned_abs();
            let next_down = sext(vs.wrapping_sub(1) & m).unsigned_abs();
            if next_down <= next_up {
                move_start()
            } else {
                move_end()
            }
        } else if vs > ve {
            move_end()
        } else if vs <= distance_end_zero {
            if count <= vs {
                move_start()
            } else {
                move_end()
            }
        } else {
            let end = ve.wrapping_add(count) & m;
            let overshot_zero = end >= 1 && end <= ve;
            if overshot_zero {
                move_start()
            } else {
                move_end()
            }
        }
    }

    fn some(self) -> Option<Niche> {
        if self.available() > 0 && self.bytes <= 8 {
            Some(self)
        } else {
            None
        }
    }
}

/// How an enum's tag field encodes the variant (rustc_abi TagEncoding).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TagEnc {
    /// the tag holds the discriminant (or: not an enum)
    Direct,
    /// variants `lo..=hi` (except `untagged`) are tag values `start + (v - lo)` (wrapping);
    /// any other value is the untagged variant
    Niche { untagged: u32, lo: u32, hi: u32, start: u128 },
    /// one variant only (rustc Variants::Single): no tag
    Single(u32),
}

pub const MAX_LEAVES: usize = 8;

/// One computed layout; `too_big`: the type is 4 GiB or larger (layouts are u32, their sizes
/// saturate and every use is an error in lowering, never a silent wrap).
#[derive(PartialEq, Eq, Hash)]
struct LEntry {
    layout: Layout,
    too_big: bool,
}

const LCHUNK_BITS: u32 = 12;
const LCHUNK: usize = 1 << LCHUNK_BITS;
const LMAX_CHUNKS: usize = 1 << 14;

/// A slot word: bits 0..30 the layout's entry index + 1 (0: not computed yet), bits 30..32 the
/// is_unsized answer (0 unknown, 1 sized, 2 unsized).
const SLOT_IDX: u32 = (1 << 30) - 1;
const SLOT_UNSZ_SHIFT: u32 = 30;

/// Per-TyId slots (TyIds are dense): the index of the computed layout and the is_unsized
/// answer, 4 bytes per type. Readable without locks from every thread; the entry is complete
/// before its slot publishes it. The entries themselves are shared (`Layouts::ents`): a few
/// thousand distinct layouts serve every type.
struct LTable {
    chunks: Box<[std::sync::atomic::AtomicPtr<std::sync::atomic::AtomicU32>]>,
}

impl LTable {
    fn new() -> Box<LTable> {
        Box::new(LTable { chunks: (0..LMAX_CHUNKS).map(|_| std::sync::atomic::AtomicPtr::new(std::ptr::null_mut())).collect() })
    }

    /// The slot of `t` (its chunk allocated on first use).
    fn slot(&self, t: TyId) -> &std::sync::atomic::AtomicU32 {
        use std::sync::atomic::{AtomicU32, Ordering};
        let i = t.0 as usize;
        let ci = i >> LCHUNK_BITS;
        assert!(ci < LMAX_CHUNKS, "makepad: layout table full");
        let mut c = self.chunks[ci].load(Ordering::Acquire);
        if c.is_null() {
            let fresh: Box<[AtomicU32]> = (0..LCHUNK).map(|_| AtomicU32::new(0)).collect();
            let p = Box::into_raw(fresh) as *mut AtomicU32;
            match self.chunks[ci].compare_exchange(std::ptr::null_mut(), p, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => c = p,
                Err(other) => {
                    unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(p, LCHUNK))) };
                    c = other;
                }
            }
        }
        unsafe { &*c.add(i & (LCHUNK - 1)) }
    }
}

impl Drop for LTable {
    fn drop(&mut self) {
        for c in self.chunks.iter_mut() {
            let p = *c.get_mut();
            if !p.is_null() {
                unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(p, LCHUNK))) };
            }
        }
    }
}

/// The distinct layouts: an append-only array (entries never move) plus a hash chain index over
/// it, so a layout equal to one already made is stored once.
#[derive(Default)]
struct LPool {
    /// layout hash -> the first entry index with it
    head: std::collections::HashMap<u64, u32, std::hash::BuildHasherDefault<crate::fxhash::FxHasher>>,
    /// entry index -> the next entry index with the same hash (u32::MAX: none)
    next: Vec<u32>,
}

/// Per-evaluation layout effects, propagated through nested layouts instead of TLS.
/// Normalization uses PostAnalysis, as in rustc_middle/src/ty/layout.rs and
/// rustc_middle/src/ty/normalize_erasing_regions.rs:41.
pub struct LayoutCx<'p> {
    pub prog: &'p crate::program::Program,
    overflowed: std::cell::Cell<bool>,
    pub missing: std::cell::RefCell<Vec<TyId>>,
}

impl<'p> LayoutCx<'p> {
    pub fn new(prog: &'p crate::program::Program) -> Self {
        Self { prog, overflowed: std::cell::Cell::new(false), missing: Default::default() }
    }
}

/// Type layouts, shared by every thread that lowers: computed once per type, then read without
/// locks (`of` hands out references valid for the Layouts' life; `clear` retires the table
/// instead of freeing it, since references into it may still be held).
pub struct Layouts {
    closure_storage: crate::conc::MemoMap<TyId, ClosureStorage>,
    table: std::sync::atomic::AtomicPtr<LTable>,
    retired: relax_session::sync::Mutex<Vec<Box<LTable>>>,
    /// the distinct layouts, never freed before the Layouts (`of` hands out 'static references)
    ents: crate::conc::AppendVec<LEntry>,
    pool: relax_session::sync::Mutex<LPool>,
    /// provided crates' opaque types (provided.rs): rustc's size and align, no fields Relax
    /// may read; set once when the unit is made
    pub opaque: std::collections::HashMap<crate::program::DefId, (u32, u32)>,
    /// a field type that does not normalize stays as written (transmute's size skeletons erase
    /// parameters to `()`, so `PhantomData<<() as Tr>::Assoc>` need not project)
    lenient_fields: bool,
}

unsafe impl Send for Layouts {}
unsafe impl Sync for Layouts {}

impl Drop for Layouts {
    fn drop(&mut self) {
        let t = *self.table.get_mut();
        if !t.is_null() {
            unsafe { std::mem::drop(Box::from_raw(t)) };
        }
    }
}

fn plain(size: u32, align: u32, leaves: Option<Vec<Leaf>>) -> Layout {
    Layout { size, align, fields: Vec::new(), tag: None, enc: TagEnc::Direct, variant_fields: Vec::new(), leaves, niche: None, uninhabited: false, ext: None }
}

fn scalar(size: u32, mem: Mem, cls: Cls) -> Layout {
    plain(size, size, Some(vec![Leaf { off: 0, mem, cls }]))
}

/// A field as univariant layout sees it.
#[derive(Clone, Copy)]
struct FieldL {
    size: u32,
    align: u32,
    niche: Option<Niche>,
    uninhabited: bool,
}

impl FieldL {
    fn is_1zst(&self) -> bool {
        self.size == 0 && self.align == 1
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    AlwaysSized,
    MaybeUnsized,
    /// enum variants after the tag: (tag size, prefix alignment)
    Prefixed(u32, u32),
}

#[derive(Clone, Copy, PartialEq)]
enum Bias {
    Start,
    End,
}

/// One variant's / struct's layout (rustc_abi univariant).
#[derive(Clone)]
struct Uni {
    offsets: Vec<u32>,
    size: u32,
    align: u32,
    niche: Option<Niche>,
    uninhabited: bool,
}

/// Field-order and size options of a type's `repr`.
#[derive(Clone, Copy)]
struct Repr {
    pack: u32,
    align: u32,
    reorder: bool,
}

/// rustc_abi Integer::for_align: the integer whose size is this alignment.
fn int_for_align(a: u32) -> Option<u32> {
    match a {
        1 | 2 | 4 | 8 | 16 => Some(a),
        _ => None,
    }
}

fn fit_unsigned(x: u128) -> u32 {
    if x <= u8::MAX as u128 {
        1
    } else if x <= u16::MAX as u128 {
        2
    } else if x <= u32::MAX as u128 {
        4
    } else if x <= u64::MAX as u128 {
        8
    } else {
        16
    }
}

fn fit_signed(x: i128) -> u32 {
    if x >= i8::MIN as i128 && x <= i8::MAX as i128 {
        1
    } else if x >= i16::MIN as i128 && x <= i16::MAX as i128 {
        2
    } else if x >= i32::MIN as i128 && x <= i32::MAX as i128 {
        4
    } else if x >= i64::MIN as i128 && x <= i64::MAX as i128 {
        8
    } else {
        16
    }
}

fn tz(x: u64) -> u64 {
    if x == 0 {
        64
    } else {
        x.trailing_zeros() as u64
    }
}

impl Layouts {
    pub fn new() -> Layouts {
        Layouts { closure_storage: Default::default(), table: std::sync::atomic::AtomicPtr::new(Box::into_raw(LTable::new())), retired: Default::default(), ents: crate::conc::AppendVec::with_capacity(1 << 24), pool: Default::default(), opaque: Default::default(), lenient_fields: false }
    }

    /// Layouts of types whose parameters were erased (see `lenient_fields`).
    pub fn erased_generics() -> Layouts {
        let mut l = Layouts::new();
        l.lenient_fields = true;
        l
    }

    fn field_ty(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, d: crate::program::DefId, variant: u32, field: u32, args: &[TyId]) -> TyId {
        let env = crate::tcx::TypingEnv::fully_monomorphized(lcx.prog);
        if !self.lenient_fields { return tcx.field_ty(lcx.prog, env, d, variant, field, args); }
        let decl = tcx.adts[&d].variants[variant as usize].fields[field as usize].ty;
        let ty = tcx.tys.subst(decl, args);
        tcx.try_normalize_erasing_regions(lcx.prog, env, ty).unwrap_or(ty)
    }

    pub fn closure_storage(&self, prog: &crate::program::Program, tcx: &Tcx, ty: TyId) -> ClosureStorage {
        let ty = tcx.tys.erase_regions(ty);
        if let Some(storage) = self.closure_storage.get(&ty) { return storage; }
        let TyKind::Closure(_, ce, crate::types::ClosureArgs { parent_args: owner, .. }) = tcx.tys.kind(ty) else { panic!("closure storage for non-closure") };
        let TyKind::FnDef(def, args, _) = tcx.tys.kind(*owner) else { panic!("closure owner") };
        let body = crate::typeck::codegen_body(prog, tcx, *def, tcx.tys.list(*args));
        let storage = ClosureStorage::for_codegen(prog, tcx, &body, *ce);
        self.closure_storage.insert(ty, storage.clone());
        storage
    }

    pub fn closure_storage_layout(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, ty: TyId) -> Layout {
        let storage = self.closure_storage(lcx.prog, tcx, ty);
        let kind = if storage.fields.is_empty() { Kind::AlwaysSized } else { Kind::MaybeUnsized };
        self.struct_layout(lcx, tcx, &storage.fields, Repr { pack: 0, align: 0, reorder: true }, kind)
    }

    /// Memory report (MAKEPAD_RUN_MEM_REPORT): exact bytes of the slot tables (live and retired) and of
    /// the shared layout entries with their vectors.
    pub fn mem_lines(&self) -> Vec<(String, u64)> {
        use std::sync::atomic::Ordering;
        let mut out = Vec::new();
        let ret = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        let mut tables: Vec<(&str, &LTable)> = vec![("live", self.table())];
        for t in ret.iter() { tables.push(("retired", t)); }
        for (name, t) in tables {
            let (mut chunks, mut used) = (0u64, 0u64);
            for c in t.chunks.iter() {
                let p = c.load(Ordering::Acquire);
                if p.is_null() { continue; }
                chunks += 1;
                for k in 0..LCHUNK {
                    used += (unsafe { &*p.add(k) }.load(Ordering::Acquire) & SLOT_IDX != 0) as u64;
                }
            }
            let bytes = chunks * (LCHUNK * 4) as u64 + (LMAX_CHUNKS * 8) as u64;
            out.push((format!("layout {} table: slots ({} chunks of {} B; {} types with a layout)", name, chunks, LCHUNK * 4, used), bytes));
        }
        let (mut vb, mut nf, mut nvf, mut nl) = (0u64, 0u64, 0u64, 0u64);
        for e in self.ents.iter() {
            let l = &e.layout;
            vb += (l.fields.capacity() * 4 + l.leaves.as_ref().map_or(0, |v| v.capacity() * std::mem::size_of::<Leaf>()) + l.variant_fields.capacity() * 24
                + l.variant_fields.iter().map(|v| v.capacity() * 4).sum::<usize>()) as u64;
            nf += !l.fields.is_empty() as u64;
            nvf += !l.variant_fields.is_empty() as u64;
            nl += l.leaves.is_some() as u64;
        }
        let n = self.ents.len() as u64;
        out.push((format!("layout shared entries ({}; LEntry {} B; with fields {}, variant_fields {}, leaves {})", n, std::mem::size_of::<LEntry>(), nf, nvf, nl), n * std::mem::size_of::<LEntry>() as u64));
        out.push(("layout shared entries: vector heap".to_string(), vb));
        out
    }

    /// The entry made for `l` (equal layouts share one), `ents` index + 1.
    fn intern(&self, l: Layout, too_big: bool) -> u32 {
        use std::hash::{Hash, Hasher};
        let e = LEntry { layout: l, too_big };
        let mut h = crate::fxhash::FxHasher::default();
        e.hash(&mut h);
        let h = h.finish();
        let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        let mut at = pool.head.get(&h).copied();
        while let Some(i) = at {
            if self.ents[i as usize] == e {
                return i + 1;
            }
            at = Some(pool.next[i as usize]).filter(|n| *n != u32::MAX);
        }
        let i = self.ents.push(e) as u32;
        assert!(i < SLOT_IDX, "makepad: layout entries full");
        let first = pool.head.insert(h, i);
        pool.next.push(first.unwrap_or(u32::MAX));
        i + 1
    }

    /// The entry of slot word `w`'s layout index (`w & SLOT_IDX` is not 0).
    #[inline]
    fn entry(&self, w: u32) -> &'static LEntry {
        unsafe { &*(&self.ents[((w & SLOT_IDX) - 1) as usize] as *const LEntry) }
    }

    fn table(&self) -> &LTable {
        unsafe { &*self.table.load(std::sync::atomic::Ordering::Acquire) }
    }

    /// Forgets every cached layout (a type definition was re-collected). The old table stays
    /// allocated until the Layouts are dropped: references into it may still be held.
    pub fn clear(&self) {
        self.closure_storage.clear();
        let fresh = Box::into_raw(LTable::new());
        let old = self.table.swap(fresh, std::sync::atomic::Ordering::AcqRel);
        self.retired.lock().unwrap_or_else(|e| e.into_inner()).push(unsafe { Box::from_raw(old) });
    }

    /// `clear`, keeping (copied into the fresh table) the layouts of the types `keep` accepts:
    /// a declaration edit that only some crates can see keeps the layouts of types made of
    /// the other crates' items (lead decision 52 R1). The caller holds every lowering out.
    pub fn clear_except(&self, keep: &dyn Fn(TyId) -> bool) {
        use std::sync::atomic::Ordering;
        self.closure_storage.clear();
        let fresh = LTable::new();
        let current = self.table.load(Ordering::Acquire);
        if !current.is_null() {
            let current = unsafe { &*current };
            for (ci, c) in current.chunks.iter().enumerate() {
                let p = c.load(Ordering::Acquire);
                if p.is_null() { continue; }
                for k in 0..LCHUNK {
                    let w = unsafe { &*p.add(k) }.load(Ordering::Acquire);
                    if w & SLOT_IDX == 0 { continue; }
                    let t = TyId(((ci << LCHUNK_BITS) | k) as u32);
                    if !keep(t) { continue; }
                    fresh.slot(t).store(w, Ordering::Release);
                }
            }
        }
        let old = self.table.swap(Box::into_raw(fresh), Ordering::AcqRel);
        if !old.is_null() {
            self.retired.lock().unwrap_or_else(|e| e.into_inner()).push(unsafe { Box::from_raw(old) });
        }
    }

    /// The layout of `t`. 'static: entries are never freed before the Layouts (the unit) are
    /// dropped (`clear` retires tables), so callers may hold one across their own changes.
    pub fn of(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, t: TyId) -> &'static Layout {
        let t = tcx.tys.erase_regions(t);
        let _mem = crate::memstat::tag(crate::memstat::Tag::Layout);
        use std::sync::atomic::Ordering;
        let slot = self.table().slot(t);
        let w = slot.load(Ordering::Acquire);
        if w & SLOT_IDX != 0 {
            let e = self.entry(w);
            if e.too_big {
                lcx.overflowed.set(true);
            }
            return &e.layout;
        }
        // a type whose layout needs itself (`struct Foo<T: A>(T::Assoc)` with `Assoc = Foo<()>`): rustc's
        // layout query cycle, E0391, not unbounded recursion
        let _cycle = match crate::cycle::enter(crate::cycle::Kind::Layout, t.0, 2) {
            Ok(g) => g,
            Err(()) => {
                if let TyKind::Adt(d, ..) = tcx.tys.kind(t) {
                    let def = lcx.prog.def(*d);
                    tcx.dcx.report(crate::next_solver::fulfill::ObligationCause { body_id: *d, span: lcx.prog.files[def.file as usize].ast.item(def.item).lo },
                        format!("E0391: cycle detected when computing layout of `{}`", lcx.prog.def_path(*d)));
                }
                return Box::leak(Box::new(plain(0, 1, None)));
            }
        };
        let saved = lcx.overflowed.get();
        lcx.overflowed.set(false);
        let m0 = lcx.missing.borrow().len();
        let l = self.compute(lcx, tcx, t);
        let ov = lcx.overflowed.get();
        lcx.overflowed.set(saved || ov);
        if lcx.missing.borrow().len() > m0 {
            // a coroutine inside whose saved locals are not known yet: not cached
            // (Unit::layout_of lowers it and asks again)
            return Box::leak(Box::new(l));
        }
        let idx = self.intern(l, ov);
        // publish; another thread may have computed the same type meanwhile (same layout, same entry)
        let mut cur = slot.load(Ordering::Acquire);
        loop {
            if cur & SLOT_IDX != 0 {
                return &self.entry(cur).layout;
            }
            match slot.compare_exchange(cur, cur | idx, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return &self.entry(idx).layout,
                Err(now) => cur = now,
            }
        }
    }

    /// Live sessions (M10, hotreload.md section 13): a struct in the application's scope that
    /// edits may give fields in place -- named fields, no repr other than Rust's, not Copy (a
    /// bitwise copy would share the extension block).
    pub fn live_editable(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, d: crate::program::DefId, adt: &crate::tcx::AdtT, t: TyId) -> bool {
        if !LIVE_SESSION.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        let prog = lcx.prog;
        if !crate::borrowck::event::app_crate(prog, prog.def(d).krate) || adt.is_enum || adt.is_union {
            return false;
        }
        if adt.repr_c || adt.repr_packed != 0 || adt.repr_align != 0 || adt.repr_int.is_some() || adt.no_niche || adt.valid_start.is_some() || adt.valid_end.is_some() {
            return false;
        }
        if adt.variants.first().map_or(true, |v| v.shape != 2) {
            return false;
        }
        match tcx.copy_trait {
            Some(c) => !tcx.type_implements_trait(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), TraitRef { def_id: c, args: vec![GenericArg::Type(t)] }).may_apply(),
            None => false,
        }
    }

    /// Live sessions (M10): an enum of the program's own crate with Rust's repr, which variants
    /// may be added to in place (tagged layout, no niche).
    pub fn live_editable_enum(&self, lcx: &LayoutCx<'_>, d: crate::program::DefId, adt: &crate::tcx::AdtT) -> bool {
        if !LIVE_SESSION.load(std::sync::atomic::Ordering::Relaxed) || !adt.is_enum {
            return false;
        }
        let prog = lcx.prog;
        prog.def(d).krate == root_crate() && !adt.repr_c && adt.repr_int.is_none() && adt.repr_align == 0 && adt.repr_packed == 0 && !adt.no_niche
    }

    /// Is `t` 4 GiB or larger (its layout saturated)?
    pub fn is_too_big(&self, t: TyId) -> bool {
        let w = self.table().slot(t).load(std::sync::atomic::Ordering::Acquire);
        w & SLOT_IDX != 0 && self.entry(w).too_big
    }

    fn rup(&self, lcx: &LayoutCx<'_>, x: u32, a: u32) -> u32 {
        if a > 1 && x as u64 + a as u64 - 1 > u32::MAX as u64 {
            lcx.overflowed.set(true);
        }
        round_up(x, a)
    }

    fn add(&self, lcx: &LayoutCx<'_>, a: u32, b: u32) -> u32 {
        match a.checked_add(b) {
            Some(x) => x,
            None => {
                lcx.overflowed.set(true);
                u32::MAX
            }
        }
    }

    fn compute(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, t: TyId) -> Layout {
        let k = tcx.tys.kind(t).clone();
        let ptr = Mem::Int(8, false);
        let nonnull = |off: u32| Some(Niche { off, bytes: 8, start: 1, end: u64::MAX as u128 });
        match k {
            TyKind::Bool => {
                let mut l = scalar(1, Mem::Int(1, false), Cls::I);
                l.niche = Some(Niche { off: 0, bytes: 1, start: 0, end: 1 });
                l
            }
            TyKind::Char => {
                let mut l = scalar(4, Mem::Int(4, false), Cls::I);
                l.niche = Some(Niche { off: 0, bytes: 4, start: 0, end: 0x10FFFF });
                l
            }
            TyKind::Int(p) => {
                let (b, s) = crate::types::Types::int_info(p);
                if b == 16 {
                    // i128/u128: two words (low, high), lowered as word pairs
                    return plain(16, 16, Some(vec![Leaf { off: 0, mem: Mem::Int(8, false), cls: Cls::I }, Leaf { off: 8, mem: Mem::Int(8, s), cls: Cls::I }]));
                }
                scalar(b, Mem::Int(b as u8, s), Cls::I)
            }
            TyKind::Float(Prim::F32) => scalar(4, Mem::F32, Cls::F32),
            TyKind::Float(_) => scalar(8, Mem::F64, Cls::F64),
            TyKind::Ref(_, inner, _) | TyKind::Ptr(_, inner) => {
                let is_ref = matches!(k, TyKind::Ref(..));
                let mut l = if self.is_unsized(lcx, tcx, inner) && !matches!(tcx.tys.kind(self.struct_tail_for_codegen(lcx, tcx, inner)), TyKind::Foreign(_)) {
                    let mut l = plain(16, 8, Some(vec![Leaf { off: 0, mem: ptr, cls: Cls::I }, Leaf { off: 8, mem: ptr, cls: Cls::I }]));
                    l.fields = vec![0, 8];
                    // A dyn pointer's vtable is non-null even when its data pointer is raw.
                    // Slice metadata is a length and has no invalid values.
                    if matches!(tcx.tys.kind(self.struct_tail_for_codegen(lcx, tcx, inner)), TyKind::Dyn(..)) {
                        l.niche = nonnull(8);
                    }
                    l
                } else {
                    scalar(8, ptr, Cls::I)
                };
                // references are never null (rustc: the data pointer's valid range starts at 1;
                // of a fat reference's two scalars the first wins a tie)
                if is_ref {
                    l.niche = nonnull(0);
                }
                l
            }
            TyKind::FnPtr(..) => {
                let mut l = scalar(8, ptr, Cls::I);
                l.niche = nonnull(0);
                l
            }
            TyKind::Closure(file, ce, ..) => {
                let storage = self.closure_storage(lcx.prog, tcx, t);
                let mut l = self.closure_storage_layout(lcx, tcx, t);
                // a coroutine: its upvars and state word, then the saved locals of its suspension
                // points (rustc's coroutine layout: promoted fields, then the variants overlapping)
                if tcx.coroutines.contains(&(file, ce)) {
                    match tcx.coro_frames.get(&t) {
                        Some((end, align)) => {
                            if end > l.size || align > l.align {
                                let a = align.max(l.align);
                                l.size = self.rup(lcx, end.max(l.size), a);
                                l.align = a;
                                l.leaves = None;
                            }
                        }
                        None => lcx.missing.borrow_mut().push(t),
                    }
                }
                // an FnOnce closure's state holds drop flags its body clears: it lives in memory
                // (one copy), never in registers
                if storage.drop_flags {
                    l.leaves = None;
                }
                l
            }
            TyKind::FnDef(..) => plain(0, 1, Some(Vec::new())),
            TyKind::Never => {
                let mut l = plain(0, 1, Some(Vec::new()));
                l.uninhabited = true;
                l
            }
            // a tuple's last element may be unsized: it stays last (rustc StructKind::MaybeUnsized)
            TyKind::Tuple(v) => {
                let kind = if v.is_empty() { Kind::AlwaysSized } else { Kind::MaybeUnsized };
                self.struct_layout(lcx, tcx, tcx.tys.list(v), Repr { pack: 0, align: 0, reorder: true }, kind)
            }
            // a length that is still a const param / variable has no layout (never instantiated)
            TyKind::Array(_, n) if n.try_to_target_usize(&tcx.tys).is_none() => plain(0, 1, None),
            TyKind::Array(e, n) => {
                let n = n.try_to_target_usize(&tcx.tys).unwrap();
                let el = self.of(lcx, tcx, e);
                let stride = self.rup(lcx, el.size, el.align);
                let full = (stride as u64).saturating_mul(n);
                // rustc_abi obj_size_bound: 2^61 bytes on a 64-bit target; a larger value type has no
                // layout (E0080 "values of the type `..` are too big for the target architecture")
                if (stride as u128) * (n as u128) >= 1u128 << 61 {
                    tcx.dcx.report(crate::next_solver::fulfill::ObligationCause { body_id: crate::program::NO_DEF, span: 0 },
                        format!("E0080: values of the type `{}` are too big for the target architecture", crate::typeck::ty_to_string(false, lcx.prog, tcx, t)));
                }
                if full > u32::MAX as u64 {
                    lcx.overflowed.set(true);
                }
                let size = full.min(u32::MAX as u64) as u32;
                let mut l = plain(size, el.align, None);
                // arrays of N > 0 keep their element's niche and uninhabitedness (rustc_abi)
                if n > 0 {
                    l.niche = el.niche;
                    l.uninhabited = el.uninhabited;
                }
                l
            }
            TyKind::Adt(d, args, _) => {
                if args.is_empty() {
                    if let Some(&(size, align)) = self.opaque.get(&d) {
                        return plain(size, align, None);
                    }
                }
                let adt = match tcx.adts.get(&d) {
                    Some(a) => a.clone(),
                    None => return plain(0, 1, Some(Vec::new())),
                };
                let repr = Repr { pack: adt.repr_packed, align: adt.repr_align, reorder: !adt.repr_c && adt.repr_int.is_none() };
                if !adt.is_enum {
                    let mut fts = Vec::new();
                    for fi in 0..adt.variants[0].fields.len() {
                        fts.push(self.field_ty(lcx, tcx, d, 0, fi as u32, tcx.tys.list(args)));
                    }
                    // a live-editable struct (M10): its first version's fields, then the hidden
                    // extension word; the fields edits added live in the extension block
                    if !adt.is_union && self.live_editable(lcx, tcx, d, &adt, t) {
                        let n_base = (adt.ext_from as usize).min(fts.len());
                        let mut l = self.struct_layout(lcx, tcx, &fts[..n_base], repr, Kind::AlwaysSized);
                        let ext = self.rup(lcx, l.size, 8);
                        l.align = l.align.max(8);
                        l.size = self.rup(lcx, ext + 8, l.align);
                        l.ext = Some(ext);
                        l.leaves = None;
                        l.fields.resize(fts.len(), u32::MAX);
                        return l;
                    }
                    if adt.is_union {
                        let mut size = 0;
                        let mut align = 1;
                        for f in &fts {
                            let l = self.of(lcx, tcx, *f);
                            size = size.max(l.size);
                            align = align.max(packed_align(l.align, adt.repr_packed));
                        }
                        let align = align.max(adt.repr_align);
                        let n = fts.len();
                        let size = self.rup(lcx, size, align);
                        let mut l = plain(size, align, None);
                        l.fields = vec![0; n];
                        return l;
                    }
                    let kind = if fts.is_empty() || !self.decl_maybe_unsized(lcx, tcx, d, 0) { Kind::AlwaysSized } else { Kind::MaybeUnsized };
                    // `S<dyn Tr>` where S's tail parameter is Sized: rustc rejects the type (E0277);
                    // its sized instances may be reordered differently, so say so loudly
                    if kind == Kind::AlwaysSized && repr.reorder && fts.len() > 1 && self.is_unsized(lcx, tcx, fts[fts.len() - 1]) {
                        let name = lcx.prog.def_path(d);
                        eprintln!("makepad: layout of `{}` with an unsized last field, but its definition requires that field sized (rustc: E0277); declare the parameter `?Sized`", name);
                    }
                    let mut l = self.struct_layout(lcx, tcx, &fts, repr, kind);
                    if adt.is_simd() {
                        // rustc's vector layout (x86-64, AArch64): the lanes' size rounded up to a
                        // power of two, aligned to that size; passed in memory here
                        let size = l.size.max(1).next_power_of_two();
                        l.size = size;
                        l.align = size;
                        l.leaves = None;
                    }
                    if adt.no_niche {
                        l.niche = None;
                    } else if adt.valid_start.is_some() || adt.valid_end.is_some() {
                        // `rustc_layout_scalar_valid_range_*`: the range of the struct's scalar
                        if let Some(first) = l.leaves.as_ref().and_then(|v| v.first().copied()) {
                            if first.off == 0 && matches!(first.mem, Mem::Int(..)) {
                                let bytes = match first.mem {
                                    Mem::Int(b, _) => b as u32,
                                    _ => 8,
                                };
                                let m = if bytes >= 16 { u128::MAX } else { (1u128 << (bytes * 8)) - 1 };
                                l.niche = Niche { off: 0, bytes, start: adt.valid_start.unwrap_or(0) & m, end: adt.valid_end.unwrap_or(m) & m }.some();
                            }
                        }
                    }
                    return l;
                }
                // a live-editable enum (M10): always tagged, no niche given to outer types, so a
                // variant added at the end keeps every value's representation
                if self.live_editable_enum(lcx, d, &adt) {
                    let mut l = self.enum_layout(lcx, tcx, d, &adt, tcx.tys.list(args), repr, true);
                    l.niche = None;
                    return l;
                }
                self.enum_layout(lcx, tcx, d, &adt, tcx.tys.list(args), repr, false)
            }
            TyKind::Foreign(_) => plain(0, 1, None),
            TyKind::Str => plain(0, 1, None),
            TyKind::Slice(e) => {
                let el = self.of(lcx, tcx, e);
                plain(0, el.align, None)
            }
            // a trait object's alignment is only known from its vtable; struct tails of dyn type
            // are placed at an 8-aligned offset (the vtable's align beyond 8 is not supported)
            TyKind::Dyn(..) => plain(0, 8, None),
            // rustc_ty_utils::layout bugs on an escaping bound type. Placeholders
            // are TooGeneric there; here they use the existing generic fallback.
            TyKind::Bound(..) => unreachable!("layout of an escaping bound type"),
            TyKind::Param(_) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Alias(..) |
            TyKind::Const(_) | TyKind::Error | TyKind::Placeholder(_) => plain(0, 1, Some(Vec::new())),
        }
    }

    fn field_ls(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, fts: &[TyId]) -> (Vec<FieldL>, Vec<&'static Layout>) {
        let mut fl = Vec::new();
        let mut ls = Vec::new();
        for f in fts {
            let l = self.of(lcx, tcx, *f);
            fl.push(FieldL { size: l.size, align: l.align, niche: l.niche, uninhabited: l.uninhabited });
            ls.push(l);
        }
        (fl, ls)
    }

    /// A struct / tuple: rustc_abi univariant, its leaves in declaration order.
    fn struct_layout(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, fts: &[TyId], repr: Repr, kind: Kind) -> Layout {
        let (fl, ls) = self.field_ls(lcx, tcx, fts);
        let u = self.univariant(lcx, &fl, repr, kind);
        let leaves = self.leaves_of(&ls, &u.offsets);
        Layout { size: u.size, align: u.align, fields: u.offsets, tag: None, enc: TagEnc::Direct, variant_fields: Vec::new(), leaves, niche: u.niche, uninhabited: u.uninhabited, ext: None }
    }

    fn leaves_of(&self, ls: &[&'static Layout], offsets: &[u32]) -> Option<Vec<Leaf>> {
        let mut acc = Vec::new();
        for (l, off) in ls.iter().zip(offsets) {
            for x in l.leaves.as_ref()? {
                acc.push(Leaf { off: off + x.off, mem: x.mem, cls: x.cls });
            }
        }
        if acc.len() > MAX_LEAVES {
            return None;
        }
        Some(acc)
    }

    /// rustc_abi univariant: the Start-biased layout, or the End-biased one when it moves the
    /// niche closer to an edge.
    fn univariant(&self, lcx: &LayoutCx<'_>, fields: &[FieldL], repr: Repr, kind: Kind) -> Uni {
        let l = self.univariant_biased(lcx, fields, repr, kind, Bias::Start);
        if kind != Kind::MaybeUnsized {
            if let Some(n) = l.niche {
                let head = n.off as u64;
                let len = n.bytes as u64;
                let tail = (l.size as u64).saturating_sub(head + len);
                if fields.len() > 1 && head != 0 && tail > 0 {
                    let alt = self.univariant_biased(lcx, fields, repr, kind, Bias::End);
                    if let Some(an) = alt.niche {
                        let ahead = an.off as u64;
                        let alen = an.bytes as u64;
                        let _atail = (alt.size as u64).saturating_sub(ahead + alen);
                        if ahead > head && ahead > tail {
                            return alt;
                        }
                    }
                }
            }
        }
        l
    }

    fn univariant_biased(&self, lcx: &LayoutCx<'_>, fields: &[FieldL], repr: Repr, kind: Kind, bias: Bias) -> Uni {
        let n = fields.len();
        let mut order: Vec<usize> = (0..n).collect();
        let end = if kind == Kind::MaybeUnsized { n.saturating_sub(1) } else { n };
        if repr.reorder && n > 1 {
            let excl = &fields[..end];
            let max_field_align = excl.iter().map(|f| f.align as u64).max().unwrap_or(1);
            let largest_niche_size = excl.iter().filter_map(|f| f.niche).map(|x| x.available()).max().unwrap_or(0);
            let group = |f: &FieldL| -> u64 {
                if repr.pack > 0 {
                    (f.align.min(repr.pack)) as u64
                } else {
                    let align = f.align as u64;
                    let size = f.size as u64;
                    let niche_size = f.niche.map_or(0, |x| x.available());
                    let size_as_align = tz(align.max(size));
                    if largest_niche_size > 0 {
                        match bias {
                            Bias::Start => tz(max_field_align).min(size_as_align),
                            Bias::End if niche_size == largest_niche_size => tz(align),
                            Bias::End => size_as_align,
                        }
                    } else {
                        size_as_align
                    }
                }
            };
            match kind {
                Kind::AlwaysSized | Kind::MaybeUnsized => {
                    order[..end].sort_by_key(|&x| {
                        let f = &fields[x];
                        let niche_size = f.niche.map_or(0, |n| n.available());
                        let niche_size_key = match bias {
                            Bias::Start => !niche_size,
                            Bias::End => niche_size,
                        };
                        let inner = match bias {
                            Bias::Start => f.niche.map_or(0, |n| n.off as u64),
                            Bias::End => f.niche.map_or(0, |n| !((f.size as u64).wrapping_sub(n.bytes as u64).wrapping_sub(n.off as u64))),
                        };
                        (std::cmp::Reverse(group(f)), niche_size_key, inner)
                    });
                }
                Kind::Prefixed(..) => {
                    order[..end].sort_by_key(|&x| {
                        let f = &fields[x];
                        (group(f), f.niche.map_or(0, |n| n.available()))
                    });
                }
            }
        }
        let mut offsets = vec![0u32; n];
        let mut offset = 0u32;
        let mut align = 1u32;
        let mut niche: Option<Niche> = None;
        let mut best = 0u128;
        if let Kind::Prefixed(ps, pa) = kind {
            let pa = if repr.pack > 0 { pa.min(repr.pack) } else { pa };
            align = align.max(pa);
            offset = self.rup(lcx, ps, pa);
        }
        for &i in &order {
            let f = &fields[i];
            let fa = if repr.pack > 0 { f.align.min(repr.pack) } else { f.align };
            offset = self.rup(lcx, offset, fa);
            align = align.max(fa);
            offsets[i] = offset;
            if let Some(mut nn) = f.niche {
                let av = nn.available();
                let prefer = match bias {
                    Bias::Start => av > best,
                    Bias::End => av >= best,
                };
                if prefer {
                    best = av;
                    nn.off += offset;
                    niche = Some(nn);
                }
            }
            offset = self.add(lcx, offset, f.size);
        }
        if repr.align > 0 {
            align = align.max(repr.align);
        }
        let size = self.rup(lcx, offset, align);
        Uni { offsets, size, align, niche, uninhabited: fields.iter().any(|f| f.uninhabited) }
    }

    /// rustc_abi layout_of_struct_or_enum for enums: a single present variant is laid out like
    /// a struct; else the smaller of the niche-filling and the tagged layout (layout_of_enum).
    fn enum_layout(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, d: crate::program::DefId, adt: &crate::tcx::AdtT, args: &[TyId], repr: Repr, live: bool) -> Layout {
        let inhibit = adt.repr_c || adt.repr_int.is_some() || live;
        let nv = adt.variants.len();
        let mut vfl: Vec<Vec<FieldL>> = Vec::new();
        let mut vls: Vec<Vec<&'static Layout>> = Vec::new();
        for (vi, v) in adt.variants.iter().enumerate() {
            let mut fts = Vec::new();
            for fi in 0..v.fields.len() {
                fts.push(self.field_ty(lcx, tcx, d, vi as u32, fi as u32, args));
            }
            let (fl, ls) = self.field_ls(lcx, tcx, &fts);
            vfl.push(fl);
            vls.push(ls);
        }
        let absent = |fl: &Vec<FieldL>| fl.iter().any(|f| f.uninhabited) && fl.iter().all(|f| f.is_1zst());
        let present: Vec<usize> = (0..nv).filter(|&i| inhibit || !absent(&vfl[i])).collect();
        if present.is_empty() {
            // no inhabited variant: `!`
            let mut l = plain(0, 1, Some(Vec::new()));
            l.uninhabited = true;
            l.variant_fields = vfl.iter().map(|f| vec![0; f.len()]).collect();
            l.enc = TagEnc::Single(0);
            return l;
        }
        if present.len() == 1 && !inhibit {
            // a struct of its one present variant (Variants::Single)
            let v = present[0];
            let u = self.univariant(lcx, &vfl[v], repr, Kind::AlwaysSized);
            let leaves = self.leaves_of(&vls[v], &u.offsets);
            let mut vfs: Vec<Vec<u32>> = vfl.iter().map(|f| vec![0; f.len()]).collect();
            vfs[v] = u.offsets.clone();
            return Layout { size: u.size, align: u.align, fields: Vec::new(), tag: None, enc: TagEnc::Single(v as u32), variant_fields: vfs, leaves, niche: u.niche, uninhabited: u.uninhabited, ext: None };
        }
        let niche_l = if inhibit { None } else { self.niche_filling(lcx, &vfl, &vls, repr) };
        let tagged = self.tagged(lcx, tcx, adt, &vfl, &vls, repr);
        match niche_l {
            Some(nl) => {
                let ns = |l: &Layout| l.niche.map_or(0, |n| n.available());
                if tagged.size > nl.size || (tagged.size == nl.size && ns(&tagged) < ns(&nl)) {
                    nl
                } else {
                    tagged
                }
            }
            None => tagged,
        }
    }

    fn niche_filling(&self, lcx: &LayoutCx<'_>, vfl: &[Vec<FieldL>], vls: &[Vec<&'static Layout>], repr: Repr) -> Option<Layout> {
        let nv = vfl.len();
        if nv < 2 {
            return None;
        }
        let mut align = 1u32;
        let mut us: Vec<Uni> = Vec::new();
        for fl in vfl {
            let u = self.univariant(lcx, fl, repr, Kind::AlwaysSized);
            align = align.max(u.align);
            us.push(u);
        }
        // the last of the largest (Iterator::max_by_key)
        let mut largest = 0;
        for i in 0..nv {
            if us[i].size >= us[largest].size {
                largest = i;
            }
        }
        let absent = |fl: &Vec<FieldL>| fl.iter().any(|f| f.uninhabited) && fl.iter().all(|f| f.is_1zst());
        let needs: Vec<usize> = (0..nv).filter(|&i| i != largest && !absent(&vfl[i])).collect();
        let (lo, hi) = (*needs.first()?, *needs.last()?);
        let count = (hi - lo) as u128 + 1;
        let niche = us[largest].niche?;
        let (start, reserved) = niche.reserve(count)?;
        let size = self.rup(lcx, us[largest].size, align);
        let mut vfs: Vec<Vec<u32>> = Vec::new();
        for i in 0..nv {
            let mut offs = us[i].offsets.clone();
            if i != largest && us[i].size > niche.off {
                let this_off = self.rup(lcx, niche.off + niche.bytes, us[i].align);
                if this_off as u64 + us[i].size as u64 > size as u64 {
                    return None;
                }
                for o in offs.iter_mut() {
                    *o += this_off;
                }
                us[i].size += this_off;
            }
            vfs.push(offs);
        }
        let others_zst = (0..nv).all(|i| i == largest || us[i].size == 0);
        let same = size == us[largest].size && align == us[largest].align;
        let leaves = if others_zst && same { self.leaves_of(&vls[largest], &us[largest].offsets) } else { None };
        let mem = Mem::Int(niche.bytes as u8, false);
        Some(Layout {
            size,
            align,
            fields: Vec::new(),
            tag: Some((niche.off, mem)),
            enc: TagEnc::Niche { untagged: largest as u32, lo: lo as u32, hi: hi as u32, start },
            variant_fields: vfs,
            leaves,
            niche: reserved.some(),
            uninhabited: us.iter().all(|u| u.uninhabited),
            ext: None,
        })
    }

    fn tagged(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, adt: &crate::tcx::AdtT, vfl: &[Vec<FieldL>], vls: &[Vec<&'static Layout>], repr: Repr) -> Layout {
        let nv = vfl.len();
        // the discriminants of inhabited variants, and the largest gap between them (wrapping)
        let mut ds: Vec<i128> = (0..nv).filter(|&i| adt.repr_c || !vfl[i].iter().any(|f| f.uninhabited)).map(|i| adt.discriminant_for_variant(tcx, lcx.prog, i as u32).expect("existing layout variant").signed_value()).collect();
        ds.sort();
        ds.dedup();
        let discr_signed = match adt.repr_int {
            Some(p) => crate::types::Types::int_info(p).1,
            None => true,
        };
        let discr_bytes = match adt.repr_int {
            Some(p) => crate::types::Types::int_info(p).0,
            None => 8,
        };
        let dmax = if discr_bytes >= 16 { u128::MAX } else { (1u128 << (discr_bytes * 8)) - 1 };
        let smax: u128 = if discr_bytes >= 16 { i128::MAX as u128 } else { (1u128 << (discr_bytes * 8 - 1)) - 1 };
        let mut best: Option<(i128, i128, u128)> = None;
        for k in 0..ds.len() {
            let (s, e) = (ds[k], ds[(k + 1) % ds.len()]);
            let dist = if s > e {
                let d = s.wrapping_sub(e);
                if discr_signed { smax.wrapping_sub(d as u128) } else { dmax.wrapping_sub(d as u128) }
            } else {
                e.wrapping_sub(s) as u128
            };
            // max_by_key: the last of the largest
            if best.map_or(true, |b| dist >= b.2) {
                best = Some((s, e, dist));
            }
        }
        let (max, min) = best.map_or((0, 0), |b| (b.0, b.1));
        // rustc_middle discr_range_of_repr
        let (min_bytes, signed) = match adt.repr_int {
            Some(p) => {
                let (b, s) = crate::types::Types::int_info(p);
                (b, s)
            }
            None => {
                let at_least = if adt.repr_c { 4 } else { 1 };
                let uf = fit_unsigned((min as u128).max(max as u128));
                let sf = fit_signed(min).max(fit_signed(max));
                if uf <= sf {
                    (uf.max(at_least), false)
                } else {
                    (sf.max(at_least), true)
                }
            }
        };
        let mut prefix_align = min_bytes;
        if adt.repr_c {
            for fl in vfl {
                for f in fl {
                    prefix_align = prefix_align.max(f.align);
                }
            }
        }
        let mut align = 1u32;
        let mut size = 0u32;
        let mut start_align = 256u32;
        let mut us: Vec<Uni> = Vec::new();
        for fl in vfl {
            let u = self.univariant(lcx, fl, repr, Kind::Prefixed(min_bytes, prefix_align));
            // the first field (by offset) that is not a 1-ZST
            let mut idx: Vec<usize> = (0..fl.len()).collect();
            idx.sort_by_key(|&i| u.offsets[i]);
            for i in idx {
                if !fl[i].is_1zst() {
                    start_align = start_align.min(fl[i].align);
                    break;
                }
            }
            size = size.max(u.size);
            align = align.max(u.align);
            us.push(u);
        }
        size = self.rup(lcx, size, align);
        let mut ity = if adt.repr_c || adt.repr_int.is_some() { min_bytes } else { int_for_align(start_align).unwrap_or(min_bytes) };
        if ity <= min_bytes {
            ity = min_bytes;
        } else {
            ity = ity.min(8);
            for u in us.iter_mut() {
                for o in u.offsets.iter_mut() {
                    if *o <= min_bytes {
                        *o = ity;
                    }
                }
                if u.size <= min_bytes {
                    u.size = ity;
                }
            }
        }
        let mask = if ity >= 16 { u128::MAX } else { (1u128 << (ity * 8)) - 1 };
        let tag_niche = Niche { off: 0, bytes: ity, start: (min as u128) & mask, end: (max as u128) & mask };
        let mem = Mem::Int(ity as u8, signed);
        let leaves = if ity != size { None } else if ity == 16 {
            // The scalar tag uses the same two-word ABI as i128/u128.
            Some(vec![Leaf { off: 0, mem: Mem::Int(8, false), cls: Cls::I }, Leaf { off: 8, mem: Mem::Int(8, signed), cls: Cls::I }])
        } else { Some(vec![Leaf { off: 0, mem, cls: Cls::I }]) };
        let _ = vls;
        Layout {
            size,
            align,
            fields: Vec::new(),
            tag: Some((0, mem)),
            enc: TagEnc::Direct,
            variant_fields: us.iter().map(|u| u.offsets.clone()).collect(),
            leaves,
            niche: tag_niche.some(),
            uninhabited: us.iter().all(|u| u.uninhabited),
            ext: None,
        }
    }

    /// Whether ADT `d`'s declared last field may be unsized (rustc: the tail's type is not Sized
    /// for the definition's own params): such a struct's tail is never reordered.
    fn decl_maybe_unsized(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, d: crate::program::DefId, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        let adt = match tcx.adts.get(&d) {
            Some(a) if !a.is_enum && !a.is_union => a.clone(),
            _ => return false,
        };
        let last = match adt.variants.first().and_then(|v| v.fields.last()) {
            Some(f) => f.ty,
            None => return false,
        };
        self.ty_maybe_unsized(lcx, tcx, last, &adt.param_maybe_unsized, depth)
    }

    fn ty_maybe_unsized(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, t: TyId, params: &[bool], depth: u32) -> bool {
        match tcx.tys.kind(t).clone() {
            TyKind::Foreign(_) | TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => true,
            TyKind::Param(i) => params.get(i as usize).copied().unwrap_or(false),
            TyKind::Tuple(v) => match tcx.tys.list(v).last() {
                Some(l) => self.ty_maybe_unsized(lcx, tcx, *l, params, depth + 1),
                None => false,
            },
            TyKind::Adt(d2, a2, _) => {
                let adt2 = match tcx.adts.get(&d2) {
                    Some(a) if !a.is_enum && !a.is_union => a.clone(),
                    _ => return false,
                };
                let lt = match adt2.variants.first().and_then(|v| v.fields.last()) {
                    Some(f) => f.ty,
                    None => return false,
                };
                match tcx.tys.kind(lt).clone() {
                    TyKind::Param(j) => {
                        adt2.param_maybe_unsized.get(j as usize).copied().unwrap_or(false)
                            && tcx.tys.list(a2).get(j as usize).map_or(false, |x| self.ty_maybe_unsized(lcx, tcx, *x, params, depth + 1))
                    }
                    _ => {
                        if !self.decl_maybe_unsized(lcx, tcx, d2, depth + 1) {
                            return false;
                        }
                        let s = self.field_ty(lcx, tcx, d2, 0, adt2.variants[0].fields.len() as u32 - 1, tcx.tys.list(a2));
                        self.ty_maybe_unsized(lcx, tcx, s, params, depth + 1)
                    }
                }
            }
            _ => false,
        }
    }

    /// Dynamically sized: str, slices, trait objects, and structs/tuples whose last field is.
    pub fn is_unsized(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, t: TyId) -> bool {
        let t = tcx.tys.erase_regions(t);
        use std::sync::atomic::Ordering;
        let slot = self.table().slot(t);
        match slot.load(Ordering::Relaxed) >> SLOT_UNSZ_SHIFT {
            1 => return false,
            2 => return true,
            _ => {}
        }
        // `struct Foo(<&'static Foo as Deref>::Target)`: the tail is the struct itself through a
        // projection -- rustc's layout query cycle (E0391)
        let _cycle = match crate::cycle::enter(crate::cycle::Kind::UnsizedTail, t.0, 0) {
            Ok(g) => g,
            Err(()) => {
                if let TyKind::Adt(d, ..) = tcx.tys.kind(t) {
                    let def = lcx.prog.def(*d);
                    tcx.dcx.report(crate::next_solver::fulfill::ObligationCause { body_id: *d, span: lcx.prog.files[def.file as usize].ast.item(def.item).lo },
                        format!("E0391: cycle detected when computing layout of `{}`", lcx.prog.def_path(*d)));
                }
                return false;
            }
        };
        // by type structure only (no layouts: pointee layouts may be recursive)
        let r = match tcx.tys.kind(t) {
            TyKind::Foreign(_) | TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => true,
            TyKind::Tuple(v) => match tcx.tys.list(*v).last() {
                Some(l) => self.is_unsized(lcx, tcx, *l),
                None => false,
            },
            TyKind::Adt(d, args, _) => match tcx.adts.get(d) {
                // a one-field union is a wrapper too (Relax's ManuallyDrop<T: ?Sized>)
                Some(adt) if !adt.is_enum && (!adt.is_union || adt.variants[0].fields.len() == 1) => match adt.variants[0].fields.last() {
                    Some(_) => {
                        // a projection tail (`A::Dyn` with `type Dyn = DynRef`) normalized first
                        let ft = self.field_ty(lcx, tcx, *d, 0, adt.variants[0].fields.len() as u32 - 1, tcx.tys.list(*args));
                        self.is_unsized(lcx, tcx, ft)
                    }
                    None => false,
                },
                _ => false,
            },
            _ => false,
        };
        slot.fetch_or((if r { 2 } else { 1 }) << SLOT_UNSZ_SHIFT, Ordering::Relaxed);
        r
    }

    /// rustc_middle/src/ty/util.rs:216-226,256-315: struct tails retain the metadata
    /// kind of their last field, including the thin metadata of an extern type.
    pub fn struct_tail_for_codegen(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, mut ty: TyId) -> TyId {
        for _ in 0..crate::next_solver::search_graph::SearchGraphConfig::default().recursion_limit {
            ty = match tcx.tys.kind(ty) {
                TyKind::Tuple(fields) => match tcx.tys.list(*fields).last() { Some(&last) => last, None => return ty },
                TyKind::Adt(def, args, _) => {
                    let Some(adt) = tcx.adts.get(def) else { return ty };
                    if adt.is_enum || adt.is_union || adt.variants[0].fields.is_empty() { return ty; }
                    self.field_ty(lcx, tcx, *def, 0, adt.variants[0].fields.len() as u32 - 1, tcx.tys.list(*args))
                }
                _ => return ty,
            };
        }
        panic!("recursion limit reached computing struct tail");
    }

    /// The innermost dynamically sized tail of `t` and its offset from the start of `t`
    /// (`(0, t)` for str/slice/dyn themselves); None for sized types.
    pub fn unsized_tail(&self, lcx: &LayoutCx<'_>, tcx: &Tcx, t: TyId) -> Option<(u32, TyId)> {
        if !self.is_unsized(lcx, tcx, t) {
            return None;
        }
        match tcx.tys.kind(t).clone() {
            TyKind::Foreign(_) | TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => Some((0, t)),
            TyKind::Tuple(v) => {
                let last = *tcx.tys.list(v).last()?;
                let (o, tail) = self.unsized_tail(lcx, tcx, last)?;
                let l = self.of(lcx, tcx, t);
                Some((l.fields[tcx.tys.list(v).len() - 1] + o, tail))
            }
            TyKind::Adt(d, args, _) => {
                let adt = tcx.adts.get(&d)?;
                if adt.is_enum || (adt.is_union && adt.variants[0].fields.len() != 1) {
                    return None;
                }
                adt.variants[0].fields.last()?;
                let ft = self.field_ty(lcx, tcx, d, 0, adt.variants[0].fields.len() as u32 - 1, tcx.tys.list(args));
                let (o, tail) = self.unsized_tail(lcx, tcx, ft)?;
                let l = self.of(lcx, tcx, t);
                Some((l.fields[adt.variants[0].fields.len() - 1] + o, tail))
            }
            _ => None,
        }
    }
}

pub fn round_up(x: u32, a: u32) -> u32 {
    if a <= 1 {
        x
    } else {
        ((x as u64 + a as u64 - 1) / a as u64 * a as u64).min(u32::MAX as u64) as u32
    }
}

/// A field's alignment inside a `repr(packed(n))` type (n = 0: not packed).
fn packed_align(align: u32, packed: u32) -> u32 {
    if packed == 0 {
        align
    } else {
        align.min(packed)
    }
}

/// A live session (`makepad run` of a working tree watches and patches the program's sources).
pub static LIVE_SESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The crate of the program's main (the disk cache's startup manifest is per program).
static ROOT_CRATE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The program's own crate (the package `makepad run` / the loader runs).
pub fn root_crate() -> u32 {
    ROOT_CRATE.load(std::sync::atomic::Ordering::Relaxed)
}

/// `makepad live`: the crate under test is the program's own crate (live_editable).
pub fn set_root_crate(c: u32) {
    ROOT_CRATE.store(c, std::sync::atomic::Ordering::Relaxed);
}
