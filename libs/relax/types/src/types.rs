// ported from N:rustc_middle/src/ty/erase_regions.rs @8d1a764

// ported from N:rustc_type_ir/src/ty_kind.rs @8d1a764
//! Interned types and their machine layout.

/// Immutable declaration identity for a lifetime in an owner query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LifetimeId { pub owner: crate::program::DefId, pub local: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoundRegionKind { Anon, Named(LifetimeId) }

use crate::consts::Const;
use crate::program::{DefId, Prim};

mod collected;

#[path = "types/fold.rs"]
pub mod fold;
#[path = "types/variance.rs"]
pub mod variance;
#[path = "types/flags.rs"]
mod flags;
pub use flags::TypeFlags;
#[path = "types/closure.rs"]
pub mod closure;
#[path = "types/user_type.rs"]
pub mod user_type;
pub use closure::ClosureArgs;
#[path = "types/list.rs"]
mod list;
pub use list::{List, ListElem, TyList};

/// Zero denotes the innermost binder, as in the existing region representation.
pub type DebruijnIndex = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UniverseIndex(pub u32);

impl UniverseIndex {
    pub const ROOT: Self = Self(0);
    pub fn can_name(self, other: Self) -> bool { self.0 >= other.0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoundVar(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoundTyKind {
    Anon,
    Param(DefId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoundTy {
    pub var: BoundVar,
    pub kind: BoundTyKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaceholderType {
    pub universe: UniverseIndex,
    pub bound: BoundVar,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TyId(pub u32);

/// Interned lifetime; zero is the erased region in every Types interner.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RegionId(pub u32);

impl RegionId {
    pub const ERASED: Self = Self(0);
}

/// rustc_type_ir::RegionKind. Names belong to source/diagnostics; bound variables
/// here are anonymous indices. Late parameters retain their defining scope.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RegionKind {
    ReEarlyParam(u32),
    ReBound(DebruijnIndex, BoundVar),
    ReLateParam(RegionScope, LateParamKind),
    ReStatic,
    ReVar(u32),
    RePlaceholder { universe: UniverseIndex, var: BoundVar },
    ReErased,
    ReError,
}

/// Body owners for liberated regions. Closures have expression identities in
/// Relax, rather than rustc's dedicated LocalDefId.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RegionScope {
    Item(DefId),
    Closure { owner: DefId, expr: u32 },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LateParamKind {
    Anon(u32),
    Named(DefId),
    ClosureEnv,
}

/// Interned lifetime arguments, in lifetime-parameter order. Type/const arguments
/// keep their existing compact indices (Param(i), subst); ReEarlyParam uses the
/// full declaration index, while this list is in lifetime-parameter order.
/// Keeping the list interned avoids enlarging
/// every TyKind by another Vec, including types without lifetime arguments.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RegionArgsId(pub u32);

impl RegionArgsId {
    pub const EMPTY: Self = Self(0);
}

/// The region interner: kinds and argument lists in append-only arenas whose published entries
/// are read without a lock, the lookup maps sharded (rustc's sharded interners). Interning a new
/// entry takes `intern` so ids stay dense. A single Mutex over all of it was the top contended
/// lock of a parallel `makepad check` (20% of Scope's cycles spinning on it).
pub(crate) struct Regions {
    pub(crate) kinds: crate::conc::AppendVec<RegionKind>,
    pub(crate) args: crate::conc::AppendVec<std::sync::Arc<[RegionId]>>,
    ids: crate::conc::ShardMap<RegionKind, RegionId>,
    arg_ids: crate::conc::ShardMap<std::sync::Arc<[RegionId]>, RegionArgsId>,
    intern: relax_session::sync::Mutex<()>,
    /// a restored image's lookup maps are built by the first interning
    unindexed: std::sync::atomic::AtomicBool,
}
const REGION_CAP: usize = 1 << 26;
impl Default for Regions {
    fn default() -> Self {
        Regions { kinds: crate::conc::AppendVec::with_capacity(REGION_CAP), args: crate::conc::AppendVec::with_capacity(REGION_CAP),
            ids: Default::default(), arg_ids: Default::default(), intern: Default::default(), unindexed: Default::default() }
    }
}
impl Regions {
    /// A restored image's entries (ids follow their order); the maps are built on first use.
    pub(crate) fn restored(kinds: Vec<RegionKind>, args: Vec<std::sync::Arc<[RegionId]>>) -> Self {
        let r = Regions::default();
        for k in kinds { r.kinds.push(k); }
        for a in args { r.args.push(a); }
        r.unindexed.store(true, std::sync::atomic::Ordering::Release);
        r
    }
    fn index(&self) {
        use std::sync::atomic::Ordering;
        if !self.unindexed.load(Ordering::Acquire) { return; }
        let _g = self.intern.lock().unwrap_or_else(|e| e.into_inner());
        if !self.unindexed.load(Ordering::Acquire) { return; }
        for (i, k) in self.kinds.iter().enumerate() { self.ids.insert(*k, RegionId(i as u32 + 1)); }
        for (i, a) in self.args.iter().enumerate() { self.arg_ids.insert(a.clone(), RegionArgsId(i as u32 + 1)); }
        self.unindexed.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum Safety { Safe, Unsafe }

/// ABI identity is independent of the calling convention used by the backend.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum Abi { Rust, RustCall, C, CUnwind, System, SystemUnwind, RelaxIntrinsic, SysV64, SysV64Unwind, Win64, Win64Unwind }

impl Abi {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust", Self::RustCall => "rust-call", Self::C => "C",
            Self::CUnwind => "C-unwind", Self::System => "system",
            Self::SystemUnwind => "system-unwind", Self::RelaxIntrinsic => "relax-intrinsic",
            Self::SysV64 => "sysv64", Self::SysV64Unwind => "sysv64-unwind",
            Self::Win64 => "win64", Self::Win64Unwind => "win64-unwind",
        }
    }

    /// rustc_abi ExternAbi::as_str: every ABI name rustc knows (ours: `relax-intrinsic`).
    pub fn is_rustc_name(name: &str) -> bool {
        matches!(name, "C" | "C-unwind" | "Rust" | "Swift" | "aapcs" | "aapcs-unwind" | "avr-interrupt" | "avr-non-blocking-interrupt"
            | "cdecl" | "cdecl-unwind" | "cmse-nonsecure-call" | "cmse-nonsecure-entry" | "custom" | "efiapi" | "fastcall" | "fastcall-unwind"
            | "gpu-kernel" | "msp430-interrupt" | "ptx-kernel" | "riscv-interrupt-m" | "riscv-interrupt-s" | "rust-call" | "rust-cold"
            | "rust-invalid" | "rust-preserve-none" | "stdcall" | "stdcall-unwind" | "system" | "system-unwind" | "sysv64" | "sysv64-unwind"
            | "thiscall" | "thiscall-unwind" | "unadjusted" | "vectorcall" | "vectorcall-unwind" | "win64" | "win64-unwind" | "x86-interrupt"
            | "relax-intrinsic")
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Rust" => Some(Self::Rust), "rust-call" => Some(Self::RustCall), "C" => Some(Self::C),
            "C-unwind" => Some(Self::CUnwind), "system" => Some(Self::System),
            "system-unwind" => Some(Self::SystemUnwind), "relax-intrinsic" => Some(Self::RelaxIntrinsic),
            "sysv64" => Some(Self::SysV64), "sysv64-unwind" => Some(Self::SysV64Unwind),
            "win64" => Some(Self::Win64), "win64-unwind" => Some(Self::Win64Unwind),
            _ => None,
        }
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Rust), 1 => Some(Self::RustCall), 2 => Some(Self::C),
            3 => Some(Self::CUnwind), 4 => Some(Self::System), 5 => Some(Self::SystemUnwind),
            6 => Some(Self::RelaxIntrinsic), 7 => Some(Self::SysV64), 8 => Some(Self::SysV64Unwind),
            9 => Some(Self::Win64), 10 => Some(Self::Win64Unwind), _ => None,
        }
    }

    /// rustc FnAbi::can_unwind for the C family: the `-unwind` ABIs let a panic cross the call.
    pub fn unwinds(self) -> bool {
        matches!(self, Self::CUnwind | Self::SystemUnwind | Self::SysV64Unwind | Self::Win64Unwind)
    }

    pub fn is_c_like(self) -> bool {
        matches!(self, Self::C | Self::CUnwind | Self::System | Self::SystemUnwind | Self::RelaxIntrinsic
            | Self::SysV64 | Self::SysV64Unwind | Self::Win64 | Self::Win64Unwind)
    }

    /// rustc_target::spec::abi_map: explicit x86-64 conventions are independent of the OS.
    pub fn is_supported_on_arch(self, arch: &str) -> bool {
        !matches!(self, Self::SysV64 | Self::SysV64Unwind | Self::Win64 | Self::Win64Unwind) || arch == "x86_64"
    }
}

/// rustc_type_ir/src/ty_kind.rs:917,1151: header survives folding and reification.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FnHeader {
    pub safety: Safety,
    pub abi: Abi,
    pub c_variadic: bool,
}

impl FnHeader {
    pub const RUST: Self = Self { safety: Safety::Safe, abi: Abi::Rust, c_variadic: false };

    pub fn is_fn_trait_compatible(self) -> bool {
        self.safety == Safety::Safe && self.abi == Abi::Rust && !self.c_variadic
    }

    /// rustc_hir_typeck/src/coercion.rs:1034: only safety can be weakened.
    pub fn can_coerce_to(self, target: Self) -> bool {
        self.abi == target.abi && self.c_variadic == target.c_variadic
            && (self.safety == target.safety || self.safety == Safety::Safe)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AliasTyKind {
    Projection { def_id: DefId },
    Inherent { def_id: DefId },
    Opaque { def_id: DefId },
    Free { def_id: DefId },
}

impl AliasTyKind {
    pub fn def_id(self) -> DefId {
        match self {
            Self::Projection { def_id } | Self::Inherent { def_id } | Self::Free { def_id } => def_id,
            Self::Opaque { def_id } => def_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AliasTy { pub kind: AliasTyKind, pub args: TyList, pub regions: RegionArgsId }

/// Plain ids only (lists are interned, `Types::list`): Copy, 28 bytes, and an interner entry
/// maps from a collected image as it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum TyKind {
    Bool,
    Char,
    Str,
    Int(Prim),
    Float(Prim),
    Never,
    Tuple(TyList),
    Array(TyId, Const),
    Slice(TyId),
    Ref(bool, TyId, RegionId),
    Ptr(bool, TyId),
    Adt(DefId, TyList, RegionArgsId),
    /// rustc_type_ir/src/ty_kind.rs:184: an opaque extern type has thin pointers.
    Foreign(DefId),
    /// fn pointer: (params, return, header, bound region count).
    /// Written signatures populate this binder; A3 handles inferred closure binders.
    FnPtr(TyList, TyId, FnHeader, u32),
    FnDef(DefId, TyList, RegionArgsId),
    /// generic parameter by index in the owner's generics list
    Param(u32),
    /// inference variable (only inside one body's type check)
    Infer(u32),
    IntVar(u32),
    FloatVar(u32),
    /// Closure identity and synthetic kind, signature and minimum-capture arguments.
    Closure(u32, u32, ClosureArgs),
    /// rustc_type_ir/src/ty_kind.rs:317: rigid and normalizable aliases share
    /// their definition identity and arguments.
    Alias(crate::consts::IsRigid, AliasTy),
    /// trait object `dyn Trait<Args, Assoc = T>`: (principal trait, its generic args without
    /// Self, associated type bindings, principal lifetime args, object region,
    /// principal bound-region count). Each binding is (associated item, term,
    /// its own bound-region count); principal args and binding terms are under
    /// their respective binders, while the object region is outside them.
    /// The final field is the sorted auto-trait set, part of object type identity.
    /// The bound-region count is a u16 so the variant fits the 24 bytes a closure takes.
    Dyn(DefId, TyList, List<(DefId, TyId, u32)>, RegionArgsId, RegionId, u16, List<DefId>),
    /// a const generic argument's value (`Foo<3>`, the N of `[T; N]` after substitution)
    Const(Const),
    Error,
    /// A type variable bound by the binder at this de Bruijn index.
    Bound(DebruijnIndex, BoundTy),
    /// A universally quantified type in a particular universe.
    Placeholder(PlaceholderType),
}

const F_INFER: u32 = 1;
const F_ERROR: u32 = 2;
const F_OPAQUE: u32 = 4;
/// a projection anywhere inside (normalize's fast path; conservative: every child counts)
const F_PROJ: u32 = 8;
/// a generic param anywhere `subst` looks (closure owners and array length types included):
/// without it `subst` is the identity
const F_PARAM: u32 = 16;
/// not `is_concrete` (a param, inference variable or error type, by is_concrete's walk)
const F_NONCONC: u32 = 32;
/// `has_assoc` (a projection, by has_assoc's walk: not inside closures or trait objects)
const F_ASSOC: u32 = 64;
/// `has_stuck_assoc` (a projection whose args are all concrete, by its walk)
const F_STUCK: u32 = 128;

/// One interned type: its kind and its flags (F_*), computed once when interned (rustc's
/// TypeFlags, rustc_type_ir/src/flags.rs): `has_infer` & co. are a lookup, not a walk.
/// 40 bytes, plain data: a collected image holds these rows and maps them as they are.
#[derive(Clone, Copy)]
#[repr(C)]
struct Entry {
    kind: TyKind,
    /// the next entry (id + 1, 0 = none) in this entry's hash bucket
    next: u32,
    /// Canonical free-region-erased type (self for already erased types).
    erased: TyId,
    /// flags (META_FLAGS), nesting depth and outer exclusive binder (`meta`)
    meta: u32,
}
const _: () = assert!(std::mem::size_of::<TyKind>() == 28 && std::mem::size_of::<Entry>() == 40);
// SAFETY: plain ids and C-like enums (no pointers, no Drop); same-build images only.
unsafe impl crate::image::Pod for Entry {}

/// `meta` bits 0..21: the type flags (F_* and TypeFlags, all below bit 21).
const META_FLAGS: u32 = (1 << 21) - 1;
/// bits 21..29: nesting depth, 1 + the deepest type inside, saturating at 255; it bounds
/// instantiation of ever-growing types (jit.rs INSTANCE_DEPTH_LIMIT = 128 < 255).
const META_DEPTH_SHIFT: u32 = 21;
const META_DEPTH_MAX: u32 = 255;
/// bits 29..32: outer exclusive binder; BINDER_SAT means "7 or more", recomputed from the
/// kind when asked (types escaping seven binders do not occur in practice).
const META_BINDER_SHIFT: u32 = 29;
const BINDER_SAT: u32 = 7;
const _: () = assert!(TypeFlags::ALL_BITS & !META_FLAGS == 0);

#[inline(always)]
fn meta(flags: u32, depth: u16, binder: DebruijnIndex) -> u32 {
    debug_assert!(flags & !META_FLAGS == 0);
    flags | ((depth as u32).min(META_DEPTH_MAX) << META_DEPTH_SHIFT) | (binder.min(BINDER_SAT) << META_BINDER_SHIFT)
}

/// Entries per arena chunk (chunks never move: `kind` hands out references into them).
const CHUNK_BITS: u32 = 14;
const CHUNK: usize = 1 << CHUNK_BITS;
/// Chunk table size: 2^26 types at most (a run uses ~10^5).
const MAX_CHUNKS: usize = 1 << 12;
/// Hash buckets of the intern table (fixed): a cold Stage interns ~1.1*10^6 types, which 2^19
/// buckets held in chains of ~2 (every lookup walked them: a third of the interner's time).
const BUCKET_BITS: u32 = 21;
/// Insert locks, by bucket.
const LOCKS: usize = 64;

/// The type interner. Shared by every thread that type-checks or lowers: `intern` takes
/// `&self` (rustc's sharded interners). Entries live in an append-only arena of fixed-size
/// chunks, so a `&TyKind` stays valid while other threads intern; the map from kind to id is
/// a chained hash table whose buckets and chains are read without locks (entries are complete
/// before a bucket head publishes them, Release/Acquire); inserts take one of a few bucket locks.
pub struct Types {
    image: Option<collected::MappedTypes>,
    lists: list::Lists,
    pub(crate) consts: relax_session::sync::Mutex<crate::consts::ConstInterner>,
    regions: Regions,
    free_regions: std::sync::atomic::AtomicBool,
    chunks: Box<[std::sync::atomic::AtomicPtr<Entry>]>,
    len: std::sync::atomic::AtomicU32,
    /// serialises chunk allocation and id assignment (ids stay dense)
    grow: relax_session::sync::Mutex<()>,
    buckets: Box<[std::sync::atomic::AtomicU32]>,
    locks: Box<[relax_session::sync::Mutex<()>]>,
    pub unit: TyId,
    pub bool_: TyId,
    pub char_: TyId,
    pub str_: TyId,
    pub never: TyId,
    pub error: TyId,
    pub i32_: TyId,
    pub i64_: TyId,
    pub u8_: TyId,
    pub u32_: TyId,
    pub u64_: TyId,
    pub usize_: TyId,
    pub isize_: TyId,
    pub f32_: TyId,
    pub f64_: TyId,
    pub str_ref: TyId,
    /// The target's pointer width in bytes (size_of::<usize>()): 8, or 4 for the wasm32 web
    /// target (Program::ptr_bytes, set when declarations are collected).
    pub ptr_bytes: u32,
}

// The arena's raw chunk pointers are owned by Types; entries are written once before their id
// is published and never mutated after.
unsafe impl Send for Types {}
unsafe impl Sync for Types {}

impl Drop for Types {
    fn drop(&mut self) {
        for c in self.chunks.iter_mut() {
            let p = *c.get_mut();
            if !p.is_null() {
                unsafe { std::alloc::dealloc(p as *mut u8, std::alloc::Layout::array::<Entry>(CHUNK).unwrap()) };
            }
        }
    }
}

impl Types {
    pub fn new() -> Types {
        let t = Types {
            image: None,
            lists: list::Lists::new(),
            consts: relax_session::sync::Mutex::new(crate::consts::ConstInterner::default()),
            regions: Regions::default(),
            free_regions: std::sync::atomic::AtomicBool::new(false),
            chunks: (0..MAX_CHUNKS).map(|_| std::sync::atomic::AtomicPtr::new(std::ptr::null_mut())).collect(),
            len: std::sync::atomic::AtomicU32::new(0),
            grow: relax_session::sync::Mutex::new(()),
            buckets: (0..1usize << BUCKET_BITS).map(|_| std::sync::atomic::AtomicU32::new(0)).collect(),
            locks: (0..LOCKS).map(|_| relax_session::sync::Mutex::new(())).collect(),
            unit: TyId(0),
            bool_: TyId(0),
            char_: TyId(0),
            str_: TyId(0),
            never: TyId(0),
            error: TyId(0),
            i32_: TyId(0),
            i64_: TyId(0),
            u8_: TyId(0),
            u32_: TyId(0),
            u64_: TyId(0),
            usize_: TyId(0),
            isize_: TyId(0),
            f32_: TyId(0),
            f64_: TyId(0),
            str_ref: TyId(0),
            ptr_bytes: 8,
        };
        let mut t = t;
        t.unit = t.intern(TyKind::Tuple(TyList::EMPTY));
        t.bool_ = t.intern(TyKind::Bool);
        t.char_ = t.intern(TyKind::Char);
        t.str_ = t.intern(TyKind::Str);
        t.never = t.intern(TyKind::Never);
        t.error = t.intern(TyKind::Error);
        t.i32_ = t.intern(TyKind::Int(Prim::I32));
        t.i64_ = t.intern(TyKind::Int(Prim::I64));
        t.u8_ = t.intern(TyKind::Int(Prim::U8));
        t.u32_ = t.intern(TyKind::Int(Prim::U32));
        t.u64_ = t.intern(TyKind::Int(Prim::U64));
        t.usize_ = t.intern(TyKind::Int(Prim::Usize));
        t.isize_ = t.intern(TyKind::Int(Prim::Isize));
        t.f32_ = t.intern(TyKind::Float(Prim::F32));
        t.f64_ = t.intern(TyKind::Float(Prim::F64));
        t.str_ref = t.intern(TyKind::Ref(false, t.str_, crate::types::RegionId::ERASED));
        t
    }

    #[inline(always)]
    fn entry(&self, t: TyId) -> &Entry {
        let i = t.0 as usize;
        if let Some(image) = &self.image {
            if i < image.len() { return image.entry(i); }
        }
        // Relaxed: whoever holds `t` got it after the entry and its chunk were published
        // (happens-before), so this load sees the chunk pointer (read-read coherence)
        assert!(i >> CHUNK_BITS < MAX_CHUNKS);
        let c = unsafe { self.chunks.get_unchecked(i >> CHUNK_BITS) }.load(std::sync::atomic::Ordering::Relaxed);
        debug_assert!(!c.is_null() && i < self.len.load(std::sync::atomic::Ordering::Acquire) as usize);
        unsafe { &*c.add(i & (CHUNK - 1)) }
    }

    #[inline(always)]
    fn fl(&self, t: TyId) -> u32 {
        self.entry(t).meta & META_FLAGS
    }

    #[inline]
    fn hash_of(k: &TyKind) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = crate::fxhash::FxHasher::default();
        k.hash(&mut h);
        h.finish()
    }

    #[inline]
    fn bucket_of(h: u64) -> usize {
        // the high bits of the multiplicative hash mix best
        (h >> (64 - BUCKET_BITS)) as usize
    }

    #[inline]
    fn find(&self, mut id1: u32, k: &TyKind) -> Option<TyId> {
        while id1 != 0 {
            let e = self.entry(TyId(id1 - 1));
            if e.kind == *k {
                return Some(TyId(id1 - 1));
            }
            id1 = e.next;
        }
        None
    }

    /// Number of interned types.
    pub fn len(&self) -> usize {
        self.len.load(std::sync::atomic::Ordering::Acquire) as usize
    }

    /// (interned regions, interned region lists): the settle swap compares them before and
    /// after the restore (type ids and region ids must come back identical).
    pub fn region_counts(&self) -> (usize, usize) {
        (self.regions.kinds.len(), self.regions.args.len())
    }

    /// Memory report lines: the arena, the list arena, and the part that holds inference
    /// variables (types only one body's check can use).
    pub fn mem_lines(&self) -> Vec<(String, u64)> {
        let n = self.len();
        let es = std::mem::size_of::<Entry>();
        let mapped = self.image.as_ref().map_or(0, |image| image.len());
        let chunks = self.chunks.iter().filter(|c| !c.load(std::sync::atomic::Ordering::Relaxed).is_null()).count();
        let mut by: [usize; 10] = [0; 10];
        let (mut inf_n, mut par_n) = (0usize, 0usize);
        for i in 0..n {
            let e = self.entry(TyId(i as u32));
            let ix = match &e.kind {
                TyKind::Bool => 0, TyKind::Tuple(_) => 1, TyKind::Adt(..) => 2, TyKind::FnDef(..) => 3,
                TyKind::Alias(..) => 4, TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => 5,
                TyKind::Ref(..) => 6, TyKind::Closure(..) => 7, TyKind::FnPtr(..) => 8, _ => 9,
            };
            by[ix] += 1;
            if e.meta & F_INFER != 0 {
                inf_n += 1;
            }
            if e.meta & F_PARAM != 0 {
                par_n += 1;
            }
        }
        let (list_chunks, list_buckets) = self.lists.heap_bytes();
        let mut out = vec![
            (format!("types: arena ({} types, {} mapped, {} B entries, {} chunks)", n, mapped, es, chunks), (chunks * CHUNK * es) as u64),
            (format!("types: list arena ({} lists, {} words, {} mapped)", self.lists.count(), self.lists.end(), self.lists.mapped.len()), list_chunks as u64),
            ("types: list buckets".to_string(), list_buckets as u64),
            (format!("types: holding inference vars ({}; entries)", inf_n), (inf_n * es) as u64),
            (format!("types: holding generic params ({}; entries)", par_n), (par_n * es) as u64),
            ("types: hash buckets".to_string(), (self.buckets.len() * 4) as u64),
        ];
        let names = ["bool", "tuple", "adt", "fndef", "assoc", "infer", "ref", "closure", "fnptr", "other"];
        for (i, c) in by.iter().enumerate() {
            if *c > 0 {
                out.push((format!("  types {} ({}; entries)", names[i], c), (c * es) as u64));
            }
        }
        let r = &self.regions;
        let rb = r.kinds.len() * std::mem::size_of::<RegionKind>() + r.ids.len() * (std::mem::size_of::<(RegionKind, RegionId)>() + 1) + r.args.len() * 16 + r.args.iter().map(|a| a.len() * 4 + 16).sum::<usize>() + r.arg_ids.len() * 17;
        out.push((format!("regions ({} kinds, {} arg lists)", r.kinds.len(), r.args.len()), rb as u64));
        // exact bytes in the labels (the report prints MB)
        out.into_iter().map(|(k, v)| (format!("{} [{} B]", k, v), v)).collect()
    }

    pub fn intern(&self, k: TyKind) -> TyId {
        use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
        let h = Self::hash_of(&k);
        let b = Self::bucket_of(h);
        if let Some(id) = self.find(self.buckets[b].load(Acquire), &k) {
            return id;
        }
        let (binder_flags, outer_exclusive_binder) = self.binder_flags_of(&k);
        let m = meta(self.flags_of(&k) | binder_flags, self.depth_of(&k), outer_exclusive_binder);
        // Before taking either type-interner lock: erasure may intern a new type.
        let erased = self.erased_kind(&k).map(|k| self.intern(k));
        if erased.is_some() {
            self.free_regions.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let _l = self.locks[b % LOCKS].lock().unwrap_or_else(|e| e.into_inner());
        let head = self.buckets[b].load(Acquire);
        if let Some(id) = self.find(head, &k) {
            return id;
        }
        let _g = self.grow.lock().unwrap_or_else(|e| e.into_inner());
        let i = self.len.load(Relaxed) as usize;
        let ci = i >> CHUNK_BITS;
        assert!(ci < MAX_CHUNKS, "makepad: type interner full");
        let mut c = self.chunks[ci].load(Relaxed);
        if c.is_null() {
            let layout = std::alloc::Layout::array::<Entry>(CHUNK).unwrap();
            c = unsafe { std::alloc::alloc(layout) as *mut Entry };
            if c.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            self.chunks[ci].store(c, Release);
        }
        unsafe { c.add(i & (CHUNK - 1)).write(Entry { kind: k, next: head, erased: erased.unwrap_or(TyId(i as u32)), meta: m }) };
        self.len.store(i as u32 + 1, Release);
        self.buckets[b].store(i as u32 + 1, Release);
        TyId(i as u32)
    }

    /// Nesting depth of type `t` (a type without type arguments is 1).
    pub fn depth(&self, t: TyId) -> u32 {
        (self.entry(t).meta >> META_DEPTH_SHIFT) & META_DEPTH_MAX
    }

    fn depth_of(&self, k: &TyKind) -> u16 {
        let d = |t: &TyId| self.depth(*t) as u16;
        let max = |v: &TyList| self.list(*v).iter().map(|t| d(t)).max().unwrap_or(0);
        let inner = match k {
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) | TyKind::Alias(_, AliasTy { args: v, .. }) => max(v),
            TyKind::Const(ct) => self.const_depth(*ct),
            TyKind::Array(e, l) => d(e).max(self.const_depth(*l)),
            TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => d(e),
            TyKind::FnPtr(ps, r, _, _) => max(ps).max(d(r)),
            TyKind::Dyn(_, v, bs, _, _, _, _) => max(v).max(self.list(*bs).iter().map(|x| d(&x.1)).max().unwrap_or(0)),
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => d(s).max(d(u)).max(d(o)).max(d(closure_kind_ty)),
            _ => 0,
        };
        inner.saturating_add(1)
    }

    fn flags_of(&self, k: &TyKind) -> u32 {
        self.flags_base(k) | self.flags_walk(k)
    }

    /// F_PARAM / F_NONCONC / F_ASSOC: the answers of subst's, is_concrete's and has_assoc's
    /// walks, each by its own rule (they differ on closures, trait objects and array lengths)
    fn flags_walk(&self, k: &TyKind) -> u32 {
        let f = |t: &TyId| self.fl(*t);
        let all = |v: &TyList| self.list(*v).iter().fold(0u32, |a, t| a | f(t));
        const W: u32 = F_PARAM | F_NONCONC | F_ASSOC | F_STUCK;
        match k {
            TyKind::Const(ct) => self.const_flags(*ct).0,
            TyKind::Param(_) => F_PARAM | F_NONCONC,
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error | TyKind::Bound(..) | TyKind::Placeholder(_) => F_NONCONC,
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. } | AliasTyKind::Inherent { .. } | AliasTyKind::Free { .. }, args: v, .. }) => {
                let a = all(v);
                let stuck = if a & F_NONCONC == 0 { F_STUCK } else { 0 };
                F_ASSOC | stuck | (a & W)
            }
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args: v, .. }) => all(v) & W,
            TyKind::Array(e, l) => {
                let lt = self.const_flags(*l).0;
                (f(e) & W) | lt
            }
            TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => f(e) & W,
            TyKind::FnPtr(ps, r, _, _) => (all(ps) | f(r)) & W,
            TyKind::Dyn(_, v, bs, _, _, _, _) => (all(v) | self.list(*bs).iter().fold(0u32, |a, x| a | f(&x.1))) & (F_PARAM | F_NONCONC),
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => ((f(s) | f(u) | f(closure_kind_ty)) & (F_PARAM | F_NONCONC)) | (f(o) & F_PARAM),
            _ => 0,
        }
    }

    fn flags_base(&self, k: &TyKind) -> u32 {
        let f = |t: &TyId| self.fl(*t);
        let all = |v: &TyList| self.list(*v).iter().fold(0u32, |a, t| a | f(t));
        (match k {
            TyKind::Const(ct) => self.const_flags(*ct).0,
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => F_INFER,
            TyKind::Error => F_ERROR,
            // has_opaque stops at the opaque; its args count for the others
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args: v, .. }) => F_OPAQUE | (all(v) & !F_OPAQUE),
            TyKind::Alias(_, alias) => F_PROJ | all(&alias.args),
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) => all(v),
            // only has_infer looks at a length that is a type
            TyKind::Array(e, l) => f(e) | self.const_flags(*l).0,
            TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => f(e),
            TyKind::FnPtr(ps, r, _, _) => all(ps) | f(r),
            TyKind::Dyn(_, v, bs, _, _, _, _) => all(v) | self.list(*bs).iter().fold(0u32, |a, x| a | f(&x.1)),
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u, .. }) => f(s) | f(u) | f(closure_kind_ty),
            _ => 0,
        }) & (F_INFER | F_ERROR | F_OPAQUE | F_PROJ)
    }

    /// An already interned type.
    pub fn lookup(&self, k: &TyKind) -> Option<TyId> {
        let h = Self::hash_of(k);
        self.find(self.buckets[Self::bucket_of(h)].load(std::sync::atomic::Ordering::Acquire), k)
    }

    #[inline(always)]
    pub fn kind(&self, t: TyId) -> &TyKind {
        &self.entry(t).kind
    }

    pub fn prim(&self, p: Prim) -> TyId {
        match p {
            Prim::Bool => self.bool_,
            Prim::Char => self.char_,
            Prim::Str => self.str_,
            Prim::F32 | Prim::F64 => self.intern(TyKind::Float(p)),
            _ => self.intern(TyKind::Int(p)),
        }
    }

    pub fn is_int(&self, t: TyId) -> bool {
        matches!(self.kind(t), TyKind::Int(_))
    }
    pub fn is_float(&self, t: TyId) -> bool {
        matches!(self.kind(t), TyKind::Float(_))
    }

    /// Instantiate a type for codegen, dropping free regions in the result.
    pub fn subst_erasing_regions(&self, t: TyId, args: &[TyId]) -> TyId {
        self.erase_regions(self.subst(t, args))
    }

    pub fn map_regions(&self, t: TyId, f: &impl Fn(RegionId, u32) -> RegionId, depth: u32) -> TyId {
        struct Regions<'a> { map: &'a dyn Fn(RegionId, u32) -> RegionId }
        impl fold::TypeFolder for Regions<'_> {
            fn fold_region(&mut self, _tys: &Types, r: RegionId, depth: u32) -> RegionId { (self.map)(r, depth) }
        }
        use fold::TypeFolder;
        Regions { map: f }.fold_ty(self, t, depth)
    }

    /// Replaces `Param(i)` by `args[i]`.
    pub fn subst(&self, t: TyId, args: &[TyId]) -> TyId {
        self.subst_at_depth(t, args, 0)
    }

    /// Substitute a payload whose enclosing binders have already been skipped.
    pub(crate) fn subst_at_depth(&self, t: TyId, args: &[TyId], depth: u32) -> TyId {
        if args.is_empty() || self.fl(t) & F_PARAM == 0 {
            return t;
        }
        let sv = |v: &TyList| -> TyList { self.mk_list(&self.list(*v).iter().map(|x| self.subst_at_depth(*x, args, depth)).collect::<Vec<_>>()) };
        match self.kind(t) {
            TyKind::Param(i) => {
                if (*i as usize) < args.len() {
                    let arg = args[*i as usize];
                    self.shift_in(arg, depth)
                } else {
                    t
                }
            }
            TyKind::Tuple(v) => self.intern(TyKind::Tuple(sv(v))),
            TyKind::Const(ct) => self.intern(TyKind::Const(self.subst_const(*ct, args, depth))),
            TyKind::Array(e, len) => self.intern(TyKind::Array(self.subst_at_depth(*e, args, depth), self.subst_const(*len, args, depth))),
            TyKind::Alias(rigid, alias) => self.intern(TyKind::Alias(*rigid, AliasTy { kind: alias.kind, args: sv(&alias.args), regions: alias.regions })),
            TyKind::Slice(e) => {
                let e2 = self.subst_at_depth(*e, args, depth);
                self.intern(TyKind::Slice(e2))
            }
            TyKind::Ref(m, e, region) => {
                let e2 = self.subst_at_depth(*e, args, depth);
                self.intern(TyKind::Ref(*m, e2, *region))
            }
            TyKind::Ptr(m, e) => {
                let e2 = self.subst_at_depth(*e, args, depth);
                self.intern(TyKind::Ptr(*m, e2))
            }
            TyKind::Adt(d, v, regions) => self.intern(TyKind::Adt(*d, sv(v), *regions)),
            TyKind::FnPtr(ps, r, c, bound) => {
                let n: Vec<_> = self.list(*ps).iter().map(|t| self.subst_at_depth(*t, args, depth + 1)).collect();
                let r2 = self.subst_at_depth(*r, args, depth + 1);
                self.intern(TyKind::FnPtr(self.mk_list(&n), r2, *c, *bound))
            }
            TyKind::FnDef(d, v, regions) => self.intern(TyKind::FnDef(*d, sv(v), *regions)),
            TyKind::Dyn(d, v, bs, regions, region, bound, autos) => {
                let n: Vec<_> = self.list(*v).iter().map(|t| self.subst_at_depth(*t, args, depth + 1)).collect();
                let nb: Vec<_> = self.list(*bs).iter().map(|(a, x, n)| (*a, self.subst_at_depth(*x, args, depth + 1), *n)).collect();
                self.intern(TyKind::Dyn(*d, self.mk_list(&n), self.mk_list(&nb), *regions, *region, *bound, *autos))
            }
            TyKind::Closure(f, e, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }) => {
                let kind = self.subst_at_depth(*closure_kind_ty, args, depth);
                let sig = self.subst_at_depth(*sig, args, depth);
                let up = self.subst_at_depth(*up, args, depth);
                let owner = self.subst_at_depth(*owner, args, depth);
                self.intern(TyKind::Closure(*f, *e, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: kind, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }))
            }
            _ => t,
        }
    }

    pub fn prim_name(p: Prim) -> &'static str {
        for (n, q) in crate::program::PRIMS {
            if q == p {
                return n;
            }
        }
        "?"
    }

    /// Integer width in bytes and signedness.
    /// (bytes, signed) of an integer type on this world's target.
    pub fn int_info(&self, p: Prim) -> (u32, bool) {
        Self::int_info_of(p, self.ptr_bytes)
    }

    /// (bytes, signed) of an integer type on a target with `ptr_bytes`-byte pointers.
    pub fn int_info_of(p: Prim, ptr_bytes: u32) -> (u32, bool) {
        match p {
            Prim::Isize => (ptr_bytes, true),
            Prim::Usize => (ptr_bytes, false),
            Prim::I8 => (1, true),
            Prim::I16 => (2, true),
            Prim::I32 => (4, true),
            Prim::I64 => (8, true),
            Prim::I128 => (16, true),
            Prim::U8 => (1, false),
            Prim::U16 => (2, false),
            Prim::U32 => (4, false),
            Prim::U64 => (8, false),
            Prim::U128 => (16, false),
            _ => (0, false),
        }
    }
}

impl Types {
    pub fn mk_array(&self, e: TyId, n: Const) -> TyId {
        self.intern(TyKind::Array(e, n))
    }

    /// Does the type contain an unresolved (error) type?
    pub fn has_error(&self, t: TyId) -> bool {
        self.fl(t) & F_ERROR != 0
    }

    /// Does the type contain an associated type projection on concrete types (one that no impl
    /// resolved)?
    pub fn has_stuck_assoc(&self, t: TyId) -> bool {
        self.fl(t) & F_STUCK != 0
    }

    #[allow(dead_code)]
    fn has_stuck_assoc_walk(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { .. }, args: v, regions: _ }) => self.list(*v).iter().all(|x| self.is_concrete(*x)) || self.list(*v).iter().any(|x| self.has_stuck_assoc(*x)),
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args: v, .. }) => self.list(*v).iter().any(|x| self.has_stuck_assoc(*x)),
            TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => self.has_stuck_assoc(*e),
            TyKind::FnPtr(ps, r, _, _) => self.list(*ps).iter().any(|x| self.has_stuck_assoc(*x)) || self.has_stuck_assoc(*r),
            _ => false,
        }
    }

    /// Does the type contain an associated-type projection?
    pub fn has_assoc(&self, t: TyId) -> bool {
        self.fl(t) & F_ASSOC != 0
    }

    #[allow(dead_code)]
    fn has_assoc_walk(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) => true,
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args: v, .. }) => self.list(*v).iter().any(|x| self.has_assoc_walk(*x)),
            TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => self.has_assoc_walk(*e),
            TyKind::FnPtr(ps, r, _, _) => self.list(*ps).iter().any(|x| self.has_assoc_walk(*x)) || self.has_assoc_walk(*r),
            _ => false,
        }
    }

    /// Does the type contain an inference variable?
    /// Can normalizing change the type (a projection somewhere inside)?
    pub fn may_project(&self, t: TyId) -> bool {
        self.fl(t) & F_PROJ != 0
    }

    pub fn has_infer(&self, t: TyId) -> bool {
        self.fl(t) & F_INFER != 0
    }

    /// Does the type contain generic params or inference variables?
    pub fn is_concrete(&self, t: TyId) -> bool {
        self.fl(t) & F_NONCONC == 0
    }

    #[allow(dead_code)]
    fn is_concrete_walk(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Param(_) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error | TyKind::Bound(..) | TyKind::Placeholder(_) => false,
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) | TyKind::Alias(_, AliasTy { args: v, regions: _, .. }) => {
                for x in self.list(*v) {
                    if !self.is_concrete(*x) {
                        return false;
                    }
                }
                true
            }
            TyKind::Const(ct) => self.const_flags(*ct).0 & F_NONCONC == 0,
            TyKind::Array(e, l) => self.is_concrete(*e) && self.const_flags(*l).0 & F_NONCONC == 0,
            TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => self.is_concrete(*e),
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up, .. }) => self.is_concrete(*sig) && self.is_concrete(*up) && self.is_concrete(*closure_kind_ty),
            TyKind::Dyn(_, v, bs, _, _, _, _) => self.list(*v).iter().all(|x| self.is_concrete(*x)) && self.list(*bs).iter().all(|x| self.is_concrete(x.1)),
            TyKind::FnPtr(ps, r, _, _) => {
                for x in self.list(*ps) {
                    if !self.is_concrete(*x) {
                        return false;
                    }
                }
                self.is_concrete(*r)
            }
            _ => true,
        }
    }

    /// Does the type contain a return-position `impl Trait`?
    pub fn has_opaque(&self, t: TyId) -> bool {
        self.fl(t) & F_OPAQUE != 0
    }

    /// The opaque types inside `t` (outermost first).
    pub fn opaques_in(&self, t: TyId, out: &mut Vec<TyId>) {
        match self.kind(t) {
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args: v, .. }) => {
                if !out.contains(&t) {
                    out.push(t);
                }
                for x in self.list(*v) {
                    self.opaques_in(*x, out);
                }
            }
            TyKind::Tuple(v) | TyKind::Adt(_, v, _) | TyKind::FnDef(_, v, _) | TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { .. }, args: v, regions: _ }) => self.list(*v).iter().for_each(|x| self.opaques_in(*x, out)),
            TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => self.opaques_in(*e, out),
            TyKind::FnPtr(ps, r, _, _) => {
                self.list(*ps).iter().for_each(|x| self.opaques_in(*x, out));
                self.opaques_in(*r, out)
            }
            TyKind::Dyn(_, v, bs, _, _, _, _) => {
                self.list(*v).iter().for_each(|x| self.opaques_in(*x, out));
                self.list(*bs).iter().for_each(|x| self.opaques_in(x.1, out))
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u, .. }) => {
                self.opaques_in(*closure_kind_ty, out);
                self.opaques_in(*s, out);
                self.opaques_in(*u, out)
            }
            _ => {}
        }
    }

    /// `t` with each `from` type replaced by its `to` (structurally, outermost match first).
    pub fn replace(&self, t: TyId, pairs: &[(TyId, TyId)]) -> TyId {
        if let Some(p) = pairs.iter().find(|p| p.0 == t) {
            return p.1;
        }
        let k = *self.kind(t);
        let r = |s: &Types, x: TyId| s.replace(x, pairs);
        let rl = |v: TyList| self.mk_list(&self.list(v).iter().map(|x| r(self, *x)).collect::<Vec<_>>());
        match k {
            TyKind::Tuple(v) => self.intern(TyKind::Tuple(rl(v))),
            TyKind::Adt(d, v, regions) => self.intern(TyKind::Adt(d, rl(v), regions)),
            TyKind::FnDef(d, v, regions) => self.intern(TyKind::FnDef(d, rl(v), regions)),
            TyKind::Alias(rigid, mut alias) => {
                alias.args = rl(alias.args);
                self.intern(TyKind::Alias(rigid, alias))
            }
            TyKind::Const(ct) => self.intern(TyKind::Const(self.replace_const(ct, pairs))),
            TyKind::Array(e, l) => self.intern(TyKind::Array(r(self, e), self.replace_const(l, pairs))),
            TyKind::Slice(e) => {
                let e = r(self, e);
                self.intern(TyKind::Slice(e))
            }
            TyKind::Ref(m, e, region) => {
                let e = r(self, e);
                self.intern(TyKind::Ref(m, e, region))
            }
            TyKind::Ptr(m, e) => {
                let e = r(self, e);
                self.intern(TyKind::Ptr(m, e))
            }
            TyKind::FnPtr(ps, ret, c, bound) => {
                let n = rl(ps);
                let ret = r(self, ret);
                self.intern(TyKind::FnPtr(n, ret, c, bound))
            }
            TyKind::Dyn(d, v, bs, regions, region, bound, autos) => {
                let nb: Vec<_> = self.list(bs).iter().map(|(a, x, n)| (*a, r(self, *x), *n)).collect();
                self.intern(TyKind::Dyn(d, rl(v), self.mk_list(&nb), regions, region, bound, autos))
            }
            TyKind::Closure(f, e, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }) => {
                let kind = r(self, closure_kind_ty);
                let sig = r(self, sig);
                let up = r(self, up);
                let owner = r(self, owner);
                self.intern(TyKind::Closure(f, e, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: kind, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }))
            }
            _ => t,
        }
    }
}

impl Types {
    pub fn intern_region(&self, kind: RegionKind) -> RegionId {
        if kind == RegionKind::ReErased { return RegionId::ERASED; }
        let r = &self.regions;
        r.index();
        if let Some(id) = r.ids.get(&kind) { return id; }
        let _g = r.intern.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = r.ids.get(&kind) { return id; }
        let id = RegionId(r.kinds.push(kind) as u32 + 1);
        r.ids.insert(kind, id);
        id
    }

    #[inline]
    pub fn region_kind(&self, id: RegionId) -> RegionKind {
        if id == RegionId::ERASED { return RegionKind::ReErased; }
        self.regions.kinds[id.0 as usize - 1]
    }

    pub fn intern_region_args(&self, args: &[RegionId]) -> RegionArgsId {
        if args.is_empty() { return RegionArgsId::EMPTY; }
        let r = &self.regions;
        r.index();
        if let Some(id) = r.arg_ids.get_by(args) { return id; }
        let _g = r.intern.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = r.arg_ids.get_by(args) { return id; }
        let args: std::sync::Arc<[RegionId]> = args.into();
        let id = RegionArgsId(r.args.push(args.clone()) as u32 + 1);
        r.arg_ids.insert(args, id);
        id
    }

    #[inline]
    pub fn region_args(&self, id: RegionArgsId) -> std::sync::Arc<[RegionId]> {
        if id == RegionArgsId::EMPTY { return std::sync::Arc::from([]); }
        self.regions.args[id.0 as usize - 1].clone()
    }

    /// rustc_middle/ty/erase_regions.rs: free regions are erased; bound regions
    /// survive (they affect trait selection and TypeId). Bound names are already
    /// anonymous in this representation. Each Entry caches its result, making all
    /// codegen/layout/cache boundary calls constant time, including the A1 fast path.
    #[inline]
    pub fn erase_regions(&self, t: TyId) -> TyId {
        self.entry(t).erased
    }

    /// Whether this interner has ever contained types requiring free-region erasure.
    pub fn has_free_regions(&self) -> bool {
        self.free_regions.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn erase_region(&self, r: RegionId) -> RegionId {
        if r == RegionId::ERASED || matches!(self.region_kind(r), RegionKind::ReBound(..)) {
            r
        } else {
            RegionId::ERASED
        }
    }

    fn erase_region_args(&self, rs: RegionArgsId) -> RegionArgsId {
        if rs == RegionArgsId::EMPTY { return rs; }
        let args = self.region_args(rs);
        let args: Vec<_> = args.iter().map(|r| self.erase_region(*r)).collect();
        // rustc_middle/src/ty/erase_regions.rs: erasure changes region values,
        // never the arity of the declaration's generic argument list.
        self.intern_region_args(&args)
    }

    fn erased_kind(&self, k: &TyKind) -> Option<TyKind> {
        let changed = |t: &TyId| self.erase_regions(*t) != *t;
        let list = |v: &TyList| self.list(*v).iter().any(changed);
        let needs = match k {
            TyKind::Ref(_, t, r) => changed(t) || self.erase_region(*r) != *r,
            TyKind::Adt(_, v, rs) | TyKind::FnDef(_, v, rs) | TyKind::Alias(_, AliasTy { args: v, regions: rs, .. }) => {
                list(v) || self.erase_region_args(*rs) != *rs
            }
            TyKind::Tuple(v) => list(v),
            TyKind::Const(n) => self.erase_const_regions(*n, false) != *n,
            TyKind::Array(t, n) => changed(t) || self.erase_const_regions(*n, false) != *n,
            TyKind::Slice(t) | TyKind::Ptr(_, t) => changed(t),
            TyKind::FnPtr(ps, r, _, _) => list(ps) || changed(r),
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => changed(s) || changed(u) || changed(o) || changed(closure_kind_ty),
            TyKind::Dyn(_, v, bs, rs, r, _, _) => list(v) || self.list(*bs).iter().any(|b| changed(&b.1)) || self.erase_region_args(*rs) != *rs || self.erase_region(*r) != *r,
            _ => false,
        };
        if !needs { return None; }
        let mut k = *k;
        let ty = |t: &mut TyId| *t = self.erase_regions(*t);
        let list = |v: &mut TyList| *v = self.mk_list(&self.list(*v).iter().map(|t| self.erase_regions(*t)).collect::<Vec<_>>());
        match &mut k {
            TyKind::Ref(_, t, r) => { ty(t); *r = self.erase_region(*r); }
            TyKind::Adt(_, v, rs) | TyKind::FnDef(_, v, rs) | TyKind::Alias(_, AliasTy { args: v, regions: rs, .. }) => {
                list(v); *rs = self.erase_region_args(*rs);
            }
            TyKind::Tuple(v) => list(v),
            TyKind::Array(t, n) => {
                ty(t);
                *n = self.erase_const_regions(*n, false);
            }
            TyKind::Const(n) => *n = self.erase_const_regions(*n, false),
            TyKind::Slice(t) | TyKind::Ptr(_, t) => ty(t),
            TyKind::FnPtr(ps, r, _, _) => { list(ps); ty(r); }
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => { ty(s); ty(u); ty(o); ty(closure_kind_ty); }
            TyKind::Dyn(_, v, bs, rs, r, _, _) => {
                list(v);
                *bs = self.mk_list(&self.list(*bs).iter().map(|b| (b.0, self.erase_regions(b.1), b.2)).collect::<Vec<_>>());
                *rs = self.erase_region_args(*rs); *r = self.erase_region(*r);
            }
            _ => {}
        }
        Some(k)
    }

    /// A lifetime-insensitive key (every region erased, binders dropped) for the
    /// pre-native structural matchers and diagnostics; codegen uses erase_regions,
    /// which preserves bound regions.
    pub fn erase_all_regions(&self, t: TyId) -> TyId {
        let f = self.flags(t);
        if !f.contains(TypeFlags::HAS_FREE_REGIONS) && !f.contains(TypeFlags::HAS_RE_BOUND) { return t; }
        let mut k = *self.kind(t);
        let ty = |t: &mut TyId| *t = self.erase_all_regions(*t);
        let list = |v: &mut TyList| *v = self.mk_list(&self.list(*v).iter().map(|t| self.erase_all_regions(*t)).collect::<Vec<_>>());
        match &mut k {
            TyKind::Ref(_, t, r) => { ty(t); *r = RegionId::ERASED; }
            TyKind::Adt(_, v, rs) | TyKind::FnDef(_, v, rs) | TyKind::Alias(_, AliasTy { args: v, regions: rs, .. }) => {
                list(v); *rs = self.intern_region_args(&vec![RegionId::ERASED; self.region_args(*rs).len()]);
            }
            TyKind::Tuple(v) => list(v),
            TyKind::Array(t, n) => {
                ty(t);
                *n = self.erase_const_regions(*n, true);
            }
            TyKind::Const(n) => *n = self.erase_const_regions(*n, true),
            TyKind::Slice(t) | TyKind::Ptr(_, t) => ty(t),
            TyKind::FnPtr(ps, r, _, n) => { list(ps); ty(r); *n = 0; }
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => { ty(s); ty(u); ty(o); ty(closure_kind_ty); }
            TyKind::Dyn(_, v, bs, rs, r, bound, _) => {
                list(v);
                *bs = self.mk_list(&self.list(*bs).iter().map(|b| (b.0, self.erase_all_regions(b.1), 0)).collect::<Vec<_>>());
                *bound = 0;
                *rs = self.intern_region_args(&vec![RegionId::ERASED; self.region_args(*rs).len()]); *r = RegionId::ERASED;
            }
            _ => {}
        }
        self.intern(k)
    }

    pub fn eq_mod_regions(&self, a: TyId, b: TyId) -> bool {
        a == b || self.erase_all_regions(a) == self.erase_all_regions(b)
    }
}

#[cfg(test)]
mod region_tests {
    use super::*;

    // A1 source typeck only creates erased regions, so UI tests cannot exercise
    // non-erased interning, erasure or preservation through structural folders.
    #[test]
    fn regions_are_interned_and_erased_structurally() {
        let tys = Types::new();
        let free = tys.intern_region(RegionKind::ReEarlyParam(0));
        assert_eq!(free, tys.intern_region(RegionKind::ReEarlyParam(0)));
        let bound = tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
        let args = tys.intern_region_args(&[free]);
        assert_eq!(args, tys.intern_region_args(&[free]));
        let r = tys.intern(TyKind::Ref(false, tys.i32_, free));
        let er = tys.intern(TyKind::Ref(false, tys.i32_, RegionId::ERASED));
        assert_ne!(r, er);
        assert_eq!(tys.erase_regions(r), er);
        assert_eq!(tys.erase_regions(er), er);
        let empty = tys.intern_region_args(&[RegionId::ERASED]);
        for (k, erased) in [
            (TyKind::Adt(DefId(1), tys.mk_list(&[r]), args), TyKind::Adt(DefId(1), tys.mk_list(&[er]), empty)),
            (TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: DefId(2) }, args: tys.mk_list(&[r]), regions: args }), TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: DefId(2) }, args: tys.mk_list(&[er]), regions: empty })),
            (TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Opaque { def_id: DefId(3) }, args: tys.mk_list(&[r]), regions: args }), TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Opaque { def_id: DefId(3) }, args: tys.mk_list(&[er]), regions: empty })),
            (TyKind::FnDef(DefId(5), tys.mk_list(&[r]), args), TyKind::FnDef(DefId(5), tys.mk_list(&[er]), empty)),
            (TyKind::Dyn(DefId(6), tys.mk_list(&[r]), tys.mk_list(&[(DefId(7), r, 0)]), args, free, 0, List::EMPTY), TyKind::Dyn(DefId(6), tys.mk_list(&[er]), tys.mk_list(&[(DefId(7), er, 0)]), empty, RegionId::ERASED, 0, List::EMPTY)),
            (TyKind::Tuple(tys.mk_list(&[r])), TyKind::Tuple(tys.mk_list(&[er]))),
            (TyKind::Array(r, tys.const_usize(3)), TyKind::Array(er, tys.const_usize(3))),
            (TyKind::Slice(r), TyKind::Slice(er)),
            (TyKind::Ptr(true, r), TyKind::Ptr(true, er)),
        ] {
            let t = tys.intern(k);
            assert_eq!(tys.erase_regions(t), tys.intern(erased));
        }
        let br = tys.intern(TyKind::Ref(false, tys.i32_, bound));
        let sig = tys.intern(TyKind::FnPtr(tys.mk_list(&[br, r]), r, crate::types::FnHeader::RUST, 1));
        let esig = tys.intern(TyKind::FnPtr(tys.mk_list(&[br, er]), er, crate::types::FnHeader::RUST, 1));
        assert_eq!(tys.erase_regions(sig), esig);
        assert_ne!(esig, tys.intern(TyKind::FnPtr(tys.mk_list(&[er, er]), er, crate::types::FnHeader::RUST, 0)));
        let owner = tys.intern(TyKind::FnDef(DefId(5), tys.mk_list(&[r]), args));
        let closure = tys.intern(TyKind::Closure(0, 1, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: r }));
        let eclosure = tys.intern(TyKind::Closure(0, 1, crate::types::ClosureArgs { parent_args: tys.erase_regions(owner), closure_kind_ty: tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: esig, tupled_upvars_ty: er }));
        assert_eq!(tys.erase_regions(closure), eclosure);
        // Type substitution/structural replacement must not silently erase regions.
        let param = tys.intern(TyKind::Param(0));
        let rp = tys.intern(TyKind::Ref(false, param, free));
        assert_eq!(tys.subst(rp, &[tys.i32_]), r);
        assert_eq!(tys.replace(rp, &[(param, tys.i32_)]), r);
        let ap = tys.intern(TyKind::Adt(DefId(1), tys.mk_list(&[rp]), args));
        let ai = tys.intern(TyKind::Adt(DefId(1), tys.mk_list(&[r]), args));
        assert_eq!(tys.subst(ap, &[tys.i32_]), ai);
        assert_eq!(tys.replace(ap, &[(param, tys.i32_)]), ai);
    }
}

#[cfg(test)]
#[path = "types/tests.rs"]
mod binder_tests;
