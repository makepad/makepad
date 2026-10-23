// ported from compiler/rustc_middle/src/ty/generics.rs @8d1a764
//! Type context: item signatures, ADT fields, the inherent/trait impl index and
//! lowering of syntactic types (`ast::Ty`) to interned types.

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::ast::{self, GenericArg, GenericArgs, GenericParamKind, ImplItem, ItemKind, SegKind, TraitItem, TypeAliasItem, TyId as AstTy, VariantData};
use crate::program::{DefId, DefKind, Prim, Program, Sym, NO_DEF};
use crate::types::{Abi, FnHeader, Safety, TyId, TyKind, Types};
use crate::fxhash::HashMap;
use crate::collected::ImageMap;
use crate::lang_items::{LangItem, LangItems};

mod collected;
mod declarations;
mod regions;
pub mod arg_count;
pub(crate) mod lifetime_resolve;
pub(crate) mod resolve_bound_vars;
mod discriminant;
pub use discriminant::{Discr, VariantDiscr};
pub mod outlives;
mod env;
pub use env::{ParamEnvId, DefiningOpaquesId};
use regions::RegionEnv;
use crate::types::{RegionId, RegionKind, RegionArgsId};

/// Key for impl lookup: the outer type constructor.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TyKey {
    Prim(Prim),
    Adt(DefId),
    Slice,
    Array,
    Str,
    Ref,
    Ptr,
    Tuple,
    FnPtr,
    /// trait objects by principal trait (`impl Debug for dyn ActionTrait`)
    Dyn(DefId),
    Other,
}

#[derive(Clone)]
pub struct FnSigT {
    pub region_contract: outlives::RegionContract,
    pub generic_indices: crate::types::variance::GenericIndices,
    pub bound_regions: u32,
    pub params: Vec<TyId>,
    pub ret: TyId,
    /// 0 none, 1 self by value, 2 &self, 3 &mut self
    pub self_kind: u8,
    /// generic params of the owner impl/trait followed by the fn's own
    pub n_generics: u32,
    pub n_parent_generics: u32,
    pub is_const: bool,
    /// Explicit promotion permission, from the resolved definition's attribute.
    pub rustc_promotable: bool,
    /// `#[rustc_intrinsic]` (rustc `tcx.intrinsic(def_id)`): not coercible to a function pointer
    pub rustc_intrinsic: bool,
    pub header: FnHeader,
    /// `#[track_caller]`: takes the caller's `&'static Location` as an implicit last argument
    /// (rustc's implicit caller-location parameter)
    pub track_caller: bool,
    /// codegen_fn_attrs.target_features (with implied features) and safe_target_features:
    /// a safe `#[target_feature]` fn has an `unsafe` header (ast_lowering item.rs:1589-1597)
    pub target_features: Vec<crate::target_features::TargetFeature>,
    pub safe_target_features: bool,
}

#[derive(Clone)]
pub struct FieldT {
    pub name: Sym,
    pub ty: TyId,
    pub is_pub: bool,
    /// the module a restricted visibility (`pub`, `pub(in path)`, private) limits the
    /// field to; u32::MAX: `pub`
    pub vis_mod: u32,
}

#[derive(Clone)]
pub struct VariantT {
    pub def: DefId,
    pub name: Sym,
    /// 0 unit, 1 tuple, 2 struct
    pub shape: u8,
    pub fields: Vec<FieldT>,
    pub discr: VariantDiscr,
}

#[derive(Clone)]
pub struct AdtT {
    pub region_contract: outlives::RegionContract,
    pub generic_indices: crate::types::variance::GenericIndices,
    pub flags: AdtFlags,
    pub region_params: RegionArgsId,
    pub is_enum: bool,
    pub is_union: bool,
    pub n_generics: u32,
    pub variants: Vec<VariantT>,
    pub repr_c: bool,
    pub repr_int: Option<Prim>,
    /// `#[repr(align(N))]`: N (0 = none)
    pub repr_align: u32,
    /// `#[repr(packed)]` / `#[repr(packed(N))]`: N (0 = none)
    pub repr_packed: u32,
    /// `#[rustc_layout_scalar_valid_range_start(N)]` / `_end(N)`: the valid range of the
    /// struct's scalar (NonNull, NonZero: 1..)
    pub valid_start: Option<u128>,
    pub valid_end: Option<u128>,
    /// `#[rustc_layout_no_niche]` (Relax's mark for UnsafeCell / UnsafePinned, which rustc
    /// special-cases): its niches are hidden
    pub no_niche: bool,
    /// per generic type param (lifetimes excluded): bounded `?Sized`
    pub param_maybe_unsized: Vec<bool>,
    /// Live sessions (M10, hotreload.md section 13): fields from this index on were added by
    /// edits and live in the value's extension block (layout.rs ext word); u32::MAX: none.
    /// Fields before it keep the slots of the struct's first version (a removed one stays as a
    /// dead slot, renamed so no code names it).
    pub ext_from: u32,
    /// Live sessions (M10): (added field, dead base slot) for a retyped scalar field -- running
    /// values get the old value converted (`as`, lossless widening) instead of the default.
    pub ext_conv: Vec<(u32, u32)>,
}

/// rustc_middle/src/ty/adt.rs:33,340. Identity flags come only from collected attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdtFlags(pub u16);

impl AdtFlags {
    pub const IS_ENUM: Self = Self(1 << 0);
    pub const IS_UNION: Self = Self(1 << 1);
    pub const IS_STRUCT: Self = Self(1 << 2);
    pub const IS_BOX: Self = Self(1 << 3);
    pub const IS_PHANTOM_DATA: Self = Self(1 << 4);
    pub const IS_MANUALLY_DROP: Self = Self(1 << 5);
    pub const IS_UNSAFE_CELL: Self = Self(1 << 6);
    pub const IS_UNSAFE_PINNED: Self = Self(1 << 7);
    pub const HAS_CTOR: Self = Self(1 << 8);
    pub const IS_VARIANT_LIST_NON_EXHAUSTIVE: Self = Self(1 << 9);
    /// `#[repr(simd)]`: a vector type (layout: size rounded up to a power of two, aligned to it)
    pub const IS_SIMD: Self = Self(1 << 10);
    pub fn contains(self, flag: Self) -> bool { self.0 & flag.0 == flag.0 }
    fn insert(&mut self, flag: Self) { self.0 |= flag.0; }
}

impl AdtT {
    pub fn is_enum(&self) -> bool { self.flags.contains(AdtFlags::IS_ENUM) }
    pub fn is_union(&self) -> bool { self.flags.contains(AdtFlags::IS_UNION) }
    pub fn is_struct(&self) -> bool { self.flags.contains(AdtFlags::IS_STRUCT) }
    pub fn is_simd(&self) -> bool { self.flags.contains(AdtFlags::IS_SIMD) }
    pub fn is_box(&self) -> bool { self.flags.contains(AdtFlags::IS_BOX) }
    pub fn is_phantom_data(&self) -> bool { self.flags.contains(AdtFlags::IS_PHANTOM_DATA) }
    pub fn is_manually_drop(&self) -> bool { self.flags.contains(AdtFlags::IS_MANUALLY_DROP) }
    pub fn is_unsafe_cell(&self) -> bool { self.flags.contains(AdtFlags::IS_UNSAFE_CELL) }
    pub fn is_unsafe_pinned(&self) -> bool { self.flags.contains(AdtFlags::IS_UNSAFE_PINNED) }
    pub fn has_ctor(&self) -> bool { self.flags.contains(AdtFlags::HAS_CTOR) }
    pub fn is_variant_list_non_exhaustive(&self) -> bool { self.flags.contains(AdtFlags::IS_VARIANT_LIST_NON_EXHAUSTIVE) }
}

/// The associated drop function, not the impl or an instantiated method selection.
/// rustc_middle/src/ty/util.rs:376.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Destructor { pub did: DefId }

/// Generic parameter environment while lowering types.
#[derive(Clone, Default)]
pub struct GenEnv {
    pub sized_params: Vec<u32>,
    pub regions: RegionEnv,
    pub names: Vec<Sym>,
    pub self_ty: Option<TyId>,
    /// inside a trait (its items see `Self::Assoc` as projections)
    pub trait_def: Option<DefId>,
    /// inside an impl (its items see `Self::Assoc` as the impl's own types)
    pub impl_def: Option<DefId>,
    /// `impl Trait` in argument position: (ast type id, the anonymous generic param it is)
    pub impl_params: Vec<(u32, u32)>,
    /// trait bounds on generic params (param index, file, trait path): `T::Assoc` lookups
    pub bounds: Vec<(u32, u32, ast::Path)>,
    /// how many leading `bounds` an associated item inherits from its impl or trait: those
    /// resolve their lifetimes in the parent's bound-variable table
    pub parent_bounds: usize,
    /// const generic params: (param index, file, declared type)
    pub consts: Vec<(u32, u32, ast::TyId)>,
    /// lowering this fn's return type: `impl Trait` there is its opaque type
    pub opaque_owner: Option<DefId>,
    /// the item whose generics these are (rustc ribs: items of the owner's body blocks are
    /// innermost, so they shadow its type parameters)
    pub owner: Option<DefId>,
}

/// Expression id of an async fn's body coroutine: ASYNC_FN_BODY | the body block's id.
pub const ASYNC_FN_BODY: u32 = 0x8000_0000;

/// A return-position `impl Trait` (rustc's opaque type item): its bounds over the defining fn's
/// generics, which is all its users see (rustc item_bounds).
#[derive(Clone, Default)]
pub struct OpaqueDecl {
    pub region_contract: outlives::RegionContract,
    /// (trait, its args without Self)
    pub bounds: Vec<(DefId, Vec<TyId>)>,
}

pub struct Tcx {
    pub dcx: crate::infer::diagnostics::DiagCtxt,
    /// `adt_dtorck_constraint` per (ADT, recursion limit): a query in rustc; uncached, a chain of
    /// ADTs that each hold the previous one several times is exponential
    pub(crate) dtorck_constraints: std::sync::Mutex<crate::fxhash::HashMap<(DefId, usize), std::sync::Arc<crate::next_solver::dropck_outlives::DropckConstraint>>>,
    pub tys: Types,
    envs: env::Envs,
    /// Immutable entries; each declaration is lowered once by its first reader.
    sigs: ImageMap<DefId, std::sync::OnceLock<Box<SigParts>>>,
    /// tuple constructors as fn items (fields to the ADT): apart from `sigs`, which names
    /// only fns (code asks `sigs` whether a def is a fn); FnDef paths read fn_item_sig
    pub ctor_sigs: ImageMap<DefId, FnSigT>,
    pub adts: ImageMap<DefId, AdtT>,
    pub lang_items: LangItems,
    pub(crate) variances: std::sync::OnceLock<HashMap<DefId, Vec<crate::types::variance::V>>>,
    adt_destructors: ImageMap<DefId, Destructor>,
    destructor_impls: ImageMap<DefId, DefId>,
    /// One mapped store for lazy const/static declarations and fed anonymous types.
    const_tys: collected::ConstTypes,
    declarations: declarations::DeclarationQueries,
    pub nominal_region_contracts: ImageMap<DefId, outlives::RegionContract>,
    pub impl_self: ImageMap<DefId, TyId>,
    pub impl_trait: ImageMap<DefId, DefId>,
    pub inherent: ImageMap<TyKey, Vec<DefId>>,
    pub trait_impls: ImageMap<(TyKey, DefId), Vec<DefId>>,
    /// All impls of a trait, shared by candidate assembly and coherence.
    pub trait_impls_of: ImageMap<DefId, Vec<DefId>>,
    pub contracts_waiting: crate::collected::ImageSet<DefId>,
    /// trait impls by self type constructor
    pub impls_by_key: ImageMap<TyKey, Vec<DefId>>,
    /// trait arguments of each trait impl (in the impl's generic environment, defaults filled)
    pub impl_trait_args: ImageMap<DefId, Vec<TyId>>,
    pub impl_trait_regions: ImageMap<DefId, RegionArgsId>,
    pub alias_tys: crate::conc::MemoMap<(TypingEnv, DefId), TyId>,
    /// per impl: its trait bounds with their trait args (param, trait, args): inference through
    /// where-clauses once the impl is selected (`impl<A, B> PartialEq<Vec<B>> for Vec<A> where
    /// A: PartialEq<B>` fixes an integer literal B from A's impls, rustc's obligation processing)
    pub impl_trait_bounds: ImageMap<DefId, Vec<(u32, DefId, Vec<TyId>)>>,
    /// `core::<path>` items typeck asks for by path ("ops:Index:")
    pub core_path_items: crate::conc::MemoMap<String, Option<DefId>>,
    /// async blocks / async fn bodies (file, expr; an async fn body is ASYNC_FN_BODY | block):
    /// closure types that are coroutines -- they implement Future<Output = sig's return>; their
    /// storage record holds the state word and is extended by saved locals (coro_frames).
    pub coroutines: crate::conc::MemoMap<(u32, u32), ()>,
    /// `mir_coroutine_witnesses` reduced to what dropck asks (whether any local live across a suspension
    /// point needs drop), per coroutine (file, expr), set when borrowck has built the coroutine's MIR.
    pub coroutine_witness_needs_drop: crate::conc::MemoMap<(u32, u32), bool>,
    /// `coroutine_hidden_types` (rustc mir_coroutine_witnesses): the regions-erased types of the locals live
    /// across any suspension point, per coroutine (file, expr), in terms of its owner's generics.
    pub coroutine_hidden_types: crate::conc::MemoMap<(u32, u32), std::sync::Arc<Vec<TyId>>>,
    /// `TypeckResults::coroutine_stalled_predicates`, per typeck root: the goals that were stalled on a
    /// coroutine of that body (regions erased), proven once the coroutines' witnesses are known.
    pub coroutine_stalled_predicates: relax_session::sync::Mutex<crate::fxhash::HashMap<DefId, Vec<(crate::next_solver::Goal, u32)>>>,
    /// a coroutine type's saved locals after its upvars: (end offset, alignment), made when its
    /// poll function is lowered (coroutine_transform)
    pub coro_frames: crate::conc::MemoMap<TyId, (u32, u32)>,
    /// a coroutine type's state shape (M5): a hash of its suspension points' saved slots and
    /// values (offset, size), its size and alignment -- a body edit whose new poll code has the
    /// same shape and the same code up to its last await may resume suspended values (reload.rs)
    pub coro_shapes: crate::conc::MemoMap<TyId, u64>,
    /// a coroutine poll slot's coroutine type (set when it is lowered)
    pub coro_slot_ty: crate::conc::MemoMap<u32, TyId>,
    /// async fns: their declared output type (the signature returns an opaque Future of it)
    pub async_fns: crate::conc::MemoMap<DefId, TyId>,
    /// core::future::Future and its Output (found on first use)
    future_defs: std::sync::OnceLock<Option<(DefId, DefId)>>,
    /// Return-position `impl Trait` declarations keyed by their opaque DefId.
    pub opaques: crate::conc::ShardMap<DefId, OpaqueDecl>,
    /// rustc `inferred_outlives_of` (a query there): the outlives requirements an ADT's fields put on its own
    /// parameters, computed once per ADT; cleared whenever declarations change.
    pub inferred_outlives: crate::conc::ShardMap<DefId, std::sync::Arc<Vec<outlives::OutlivesPredicate>>>,
    /// each trait with its supertraits, transitively (a trait object implements all of them)
    pub trait_closure: ImageMap<DefId, Vec<DefId>>,
    pub queries: crate::type_queries::QueryCx,
    pub const_eval_provider: Option<crate::const_eval::ConstEvalProvider>,
    /// Bumped when collected declarations change; earlier body results are stale.
    pub collect_epoch: std::sync::atomic::AtomicU64,
    /// Initial collection precedes projection matching in impl trait arguments.
    /// core::marker::Copy
    pub copy_trait: Option<DefId>,
    /// errors of signature collection and type lowering (moved here from `err_sink` when a
    /// collection step ends)
    pub errors: Vec<String>,
    /// errors reported through `&self` (type lowering may run on any thread)
    err_sink: relax_session::sync::Mutex<Vec<String>>,
    // Diagnostics at the declaration checkpoint, before runtime/speculative queries.
    collected_errors: Option<Vec<String>>,
}



/// `f` over `items` on the boot's worker count (MAKEPAD_RUN_JOBS), results in item order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> where T: Copy {
    let jobs = crate::preload::jobs().min(items.len().max(1));
    if jobs < 2 {
        return items.iter().map(|x| f(x)).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    const CHUNK: usize = 64;
    let mut parts: Vec<(usize, Vec<R>)> = crate::pool::scope(|sc| {
        let hs: Vec<_> = (0..jobs)
            .map(|_| {
                sc.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(CHUNK, std::sync::atomic::Ordering::Relaxed);
                        if k >= items.len() {
                            break;
                        }
                        let end = (k + CHUNK).min(items.len());
                        out.push((k, items[k..end].iter().map(|x| f(x)).collect::<Vec<R>>()));
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    parts.sort_by_key(|p| p.0);
    parts.into_iter().flat_map(|p| p.1).collect()
}

/// One impl header's collection (Tcx::impl_head_captured), recorded in impl order.
struct ImplHead {
    self_ty: TyId,
    /// None: inherent impl; Some(None): a trait path that did not resolve
    trait_: Option<Option<(DefId, Vec<TyId>)>>,
    errors: Vec<String>,
    /// its lowering read the impl tables: recomputed in impl order after the earlier impls
    reads_impls: bool,
    trait_regions: RegionArgsId,
}

/// One ADT's collection, computed with `&self` (Tcx::adt_parts) and recorded by Tcx::apply_adt.
pub struct AdtParts {
    adt: Option<AdtT>,
    errors: Vec<String>,
}

/// One complete function declaration, published atomically for all signature readers.
#[derive(Default)]
pub(crate) struct SigParts {
    pub(crate) variances: std::sync::OnceLock<Vec<crate::types::variance::V>>,
    sig: Option<FnSigT>,
    self_sized: bool,
    trait_bounds: Vec<(u32, DefId, Vec<TyId>)>,
    ty_bounds: Vec<(TyId, DefId, Vec<TyId>)>,
    errors: Vec<String>,
}

/// One const/static item's declaration (Tcx::const_ty), published under its own OnceLock.
#[derive(Clone, Default)]
pub(crate) struct ConstDecl {
    ty: Option<TyId>,
    region_contract: Option<outlives::RegionContract>,
    errors: Vec<String>,
    /// (skipped module, name) misses of its lowering (the post-boot loader's)
    misses: Vec<(u32, String)>,
}

pub fn ty_key(tys: &Types, t: TyId) -> TyKey {
    match tys.kind(t) {
        TyKind::Bool => TyKey::Prim(Prim::Bool),
        TyKind::Char => TyKey::Prim(Prim::Char),
        TyKind::Int(p) | TyKind::Float(p) => TyKey::Prim(*p),
        TyKind::Str => TyKey::Str,
        TyKind::Adt(d, _, _) => TyKey::Adt(*d),
        TyKind::Slice(_) => TyKey::Slice,
        TyKind::Array(..) => TyKey::Array,
        TyKind::Ref(..) => TyKey::Ref,
        TyKind::Ptr(..) => TyKey::Ptr,
        TyKind::Tuple(_) => TyKey::Tuple,
        TyKind::FnPtr(..) => TyKey::FnPtr,
        TyKind::Dyn(d, ..) => TyKey::Dyn(*d),
        _ => TyKey::Other,
    }
}

impl Tcx {
    /// Memory report lines: the type interner and the shared memo tables typeck fills.
    pub fn mem_lines(&self) -> Vec<(String, u64)> {
        let mut out = self.tys.mem_lines();
        out.extend(self.queries.engine_mem_lines());
        let mut push = |name: &str, (n, b): (usize, usize)| out.push((format!("tcx {} ({})", name, n), b as u64));
        push("param envs", self.envs.mem_stats());
        push("alias_tys", self.alias_tys.mem_stats(|_, _| 0));
        push("core_path_items", self.core_path_items.mem_stats(|k, _| k.capacity()));
        out.extend(self.queries.mem_lines(&self.tys));
        out
    }

    pub fn new() -> Tcx { Self::with_types(Types::new()) }

    fn with_types(tys: Types) -> Tcx {
        Tcx {
            tys,
            dcx: Default::default(),
            dtorck_constraints: Default::default(),
            envs: env::Envs::new(),
            sigs: Default::default(),
            ctor_sigs: Default::default(),
            adts: Default::default(),
            const_tys: Default::default(),
            declarations: Default::default(),
            nominal_region_contracts: Default::default(),
            contracts_waiting: Default::default(),
            impl_self: Default::default(),
            impl_trait: Default::default(),
            inherent: Default::default(),
            trait_impls: Default::default(),
            trait_impls_of: Default::default(),
            impls_by_key: Default::default(),
            impl_trait_args: Default::default(),
            impl_trait_regions: Default::default(),
            alias_tys: Default::default(),
            impl_trait_bounds: Default::default(),
            core_path_items: Default::default(),
            lang_items: Default::default(),
            variances: Default::default(),
            adt_destructors: Default::default(),
            destructor_impls: Default::default(),
            coroutines: crate::conc::MemoMap::with_bits(10),
            coroutine_witness_needs_drop: crate::conc::MemoMap::with_bits(10),
            coroutine_hidden_types: crate::conc::MemoMap::new_kept(10),
            coroutine_stalled_predicates: Default::default(),
            coro_frames: Default::default(),
            coro_shapes: Default::default(),
            coro_slot_ty: Default::default(),
            async_fns: crate::conc::MemoMap::with_bits(10),
            future_defs: Default::default(),
            opaques: Default::default(),
            inferred_outlives: Default::default(),
            trait_closure: Default::default(),
            queries: crate::type_queries::QueryCx::new(),
            collect_epoch: Default::default(),
            const_eval_provider: None,
            copy_trait: None,
            errors: Vec::new(),
            err_sink: Default::default(),
            collected_errors: None,
        }
    }

    /// The errors collected so far (signature collection, type lowering).
    /// Moves the errors reported through `&self` into `errors` (end of a collection step).
    fn flush_errs(&mut self) {
        let v = std::mem::take(self.err_sink.get_mut().unwrap_or_else(|e| e.into_inner()));
        self.errors.extend(v);
        if !self.errors.is_empty() { self.dcx.note_reported_error(); }
    }

    /// rustc's orphan rules (coherence/orphan.rs, RFC 2451) for every impl outside the std
    /// crates: an inherent impl only for a type of this crate (E0116; primitives E0390); an impl
    /// of a foreign trait only when one of `Self, T1..Tn` is a local type (through the
    /// fundamental `&`, `&mut`, `Box`, `Pin`) and no uncovered type parameter comes before it
    /// (E0210), else E0117.
    /// The orphan errors (impl, position, message), unreported: the caller reports them
    /// (Tcx::check_coherence into the collected errors, the background coherence check of a
    /// run into the diagnostics context).
    pub fn orphan_errors(&self, prog: &Program) -> Vec<(DefId, u32, String)> {
        self.orphan_errors_of(prog, None)
    }

    pub fn orphan_errors_of(&self, prog: &Program, crates: Option<&[u32]>) -> Vec<(DefId, u32, String)> {
        let typing_env = TypingEnv::fully_monomorphized(prog);
        let mut errors = Vec::new();
        for im in &prog.impls {
            let krate = prog.def(im.def).krate;
            if crates.is_some_and(|c| !c.contains(&krate)) { continue; }
            let kname = prog.syms.str(prog.crates[krate as usize].name);
            if matches!(kname, "core" | "alloc" | "std" | "proc_macro") {
                continue;
            }
            let Some(&st) = self.impl_self.get(&im.def) else { continue };
            let raw_st = st;
            // inherent_impls.rs check_item: an inherent impl names a nominal type, not a projection, an opaque or a parameter
            if !self.impl_trait.contains_key(&im.def) && !prog.files[im.file as usize].ast.as_impl(&prog.files[im.file as usize].ast.item(im.item).kind).is_some_and(|i| i.trait_.is_some())
                && matches!(self.tys.kind(st), TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. } | AliasTyKind::Opaque { .. }, .. })) {
                errors.push((im.def, prog.files[im.file as usize].ast.item(im.item).lo, "E0118: no nominal type found for inherent implementation -- implement a trait or new type instead".to_string()));
                continue;
            }
            // (a projection self type is checked as what it normalizes to)
            let st = if self.tys.has_assoc(st) { self.try_normalize_erasing_regions(prog, typing_env, st).unwrap_or(st) } else { st };
            let item = prog.files[im.file as usize].ast.item(im.item);
            let pos = item.lo;
            let is_trait_impl = prog.files[im.file as usize].ast.as_impl(&item.kind).is_some_and(|i| i.trait_.is_some());
            match self.impl_trait.get(&im.def) {
                // a trait impl whose trait did not resolve: reported there
                None if is_trait_impl => {}
                None => {
                    if self.orphan_local(prog, st, krate) {
                        continue;
                    }
                    let shown = crate::typeck::ty_to_string(false, prog, self, st);
                    if matches!(self.tys.kind(st), TyKind::Adt(..) | TyKind::Dyn(..)) {
                        errors.push((im.def, pos, format!("E0116: cannot define inherent `impl` for a type outside of the crate where the type is defined (`{}`) -- define and implement a trait or a new type instead", shown)));
                    } else {
                        errors.push((im.def, pos, format!("E0390: cannot define inherent `impl` for primitive types (`{}`) -- use a trait instead", shown)));
                    }
                }
                Some(&td) => {
                    self.header_coherence_errors(prog, im.def, krate, td, raw_st, pos, &mut errors);
                    if prog.def(td).krate == krate {
                        continue;
                    }
                    // rustc_hir_analysis/src/coherence/orphan.rs:20-50: Proper first; a failure that
                    // Compat mode accepts is the UNCOVERED_PARAM_IN_PROJECTION future-compat lint.
                    use crate::next_solver::coherence::{OrphanCheckMode, OrphanCheckErr};
                    let verdict = match self.orphan_check_impl(prog, im.def, OrphanCheckMode::Proper) {
                        Ok(()) => Ok(()),
                        Err(proper) => match self.orphan_check_impl(prog, im.def, OrphanCheckMode::Compat) {
                            Ok(()) => {
                                assert!(matches!(proper, OrphanCheckErr::UncoveredTyParams { .. }), "orphanck: non-local input types in compat mode");
                                crate::lint::emit(prog, im.def, &crate::lint::UNCOVERED_PARAM_IN_PROJECTION, prog.def(im.def).file, pos,
                                    &format!("type parameter covered only by a projection in an impl of `{}`", prog.def_path(td)));
                                Ok(())
                            }
                            Err(compat) => Err(compat),
                        },
                    };
                    match verdict {
                        Ok(()) => {}
                        Err(OrphanCheckErr::UncoveredTyParams { .. }) => errors.push((im.def, pos, format!("E0210: type parameter must be covered by another type when it appears before the first local type in an impl of `{}` -- implement it for a local type or a local wrapper", prog.def_path(td)))),
                        Err(OrphanCheckErr::NonLocalInputType(_)) => errors.push((im.def, pos, format!("E0117: only traits defined in the current crate can be implemented for types defined outside of the crate (`impl {} for {}`) -- implement it for a local type or a local wrapper", prog.def_path(td), crate::typeck::ty_to_string(false, prog, self, st)))),
                    }
                }
            }
        }
        errors
    }

    /// coherence/mod.rs check_object_overlap (E0371) and coherence/orphan.rs `trait_is_auto` (E0321) of one trait impl.
    fn header_coherence_errors(&self, prog: &Program, imp: DefId, krate: u32, td: DefId, st: TyId, pos: u32, errors: &mut Vec<(DefId, u32, String)>) {
        use crate::next_solver::eval_ctxt::trait_is_auto;
        let shown = || crate::typeck::ty_to_string(false, prog, self, st);
        if let TyKind::Dyn(principal, _, _, _, _, _, autos) = self.tys.kind(st).clone() {
            let mut components = vec![principal];
            components.extend(self.tys.list(autos).iter().copied());
            for c in components {
                if !self.dyn_compatibility_violations(prog, c).is_empty() { continue; }
                // the trait and its supertraits (elaborate::supertrait_def_ids)
                let (mut seen, mut stack) = (vec![c], vec![c]);
                while let Some(t) = stack.pop() {
                    for s in self.trait_supers(prog, t) { if !seen.contains(&s) { seen.push(s); stack.push(s); } }
                }
                if seen.contains(&td) {
                    errors.push((imp, pos, format!("E0371: the object type `{}` automatically implements the trait `{}`", shown(), prog.def_path(td))));
                    return;
                }
            }
        }
        if !trait_is_auto(prog, td) { return; }
        // (local ok, non-local ok, what is problematic for a local trait)
        let local_trait = prog.def(td).krate == krate;
        let (disallow_local, nonlocal_ok, nonlocal_defined): (Option<&str>, bool, bool) = match self.tys.kind(st).clone() {
            TyKind::Adt(d, ..) | TyKind::Foreign(d) => { let own = prog.def(d).krate == krate; (None, own, !own) }
            TyKind::Dyn(..) => (Some("trait object"), false, false),
            TyKind::Param(i) => {
                let sized = self.impl_generics(prog, imp).map_or(true, |e| e.sized_params.contains(&i));
                (if sized { None } else { Some("generic type") }, false, false)
            }
            TyKind::Alias(_, a) => (Some(match a.kind { AliasTyKind::Projection { .. } => "associated type", AliasTyKind::Opaque { .. } => "opaque type", _ => "type alias" }), false, false),
            TyKind::Error => (None, true, false),
            _ => (None, false, false),
        };
        if local_trait {
            if let Some(kind) = disallow_local {
                errors.push((imp, pos, format!("E0321: traits with a default impl, like `{}`, cannot be implemented for {} `{}`", prog.def_path(td), kind, shown())));
            }
        } else if !nonlocal_ok {
            if nonlocal_defined {
                errors.push((imp, pos, format!("E0321: cross-crate traits with a default impl, like `{}`, can only be implemented for a struct/enum type defined in the current crate", prog.def_path(td))));
            } else {
                errors.push((imp, pos, format!("E0321: cross-crate traits with a default impl, like `{}`, can only be implemented for a struct/enum type, not `{}`", prog.def_path(td), shown())));
            }
        }
    }

    /// Coherence (N:rustc_hir_analysis/src/coherence/mod.rs:126-170 coherent_trait: orphan
    /// check, then the overlap check of specialization_graph_of; inherent_impls_overlap.rs)
    /// checked now, its errors reported as collection errors (orphan) and through the
    /// diagnostics context (overlap). Lead decision 53.2: coherence is not on a run's startup
    /// path -- `collect_declarations` leaves it out, a run checks it in the background after
    /// the first present (Tcx::report_coherence), `makepad check` and the other full collections
    /// (`collect`) here. Selection never reads its verdicts (nightly reports coherence errors
    /// and selection proceeds), so where it runs changes no other answer.
    pub fn check_coherence(&mut self, prog: &Program) {
        self.check_coherence_of(prog, None);
    }

    /// `check_coherence` for the impls of `crates` only (None: every crate) -- the app check,
    /// like `cargo check`, checks its own crates; their dependencies were checked as their own.
    pub fn check_coherence_of(&mut self, prog: &Program, crates: Option<&[u32]>) {
        for (imp, pos, message) in self.orphan_errors_of(prog, crates) {
            let m = prog.record_error(prog.def(imp).file, pos, "typeck", message);
            self.err_sink.get_mut().unwrap_or_else(|e| e.into_inner()).push(m);
        }
        self.flush_errs();
        self.coherence_check_of(prog, crates);
    }

    /// check_coherence for a run's background check: every error goes to the diagnostics
    /// context, which the run reports as it reports the other compile errors.
    /// `orphan_done`: the orphan errors were reported by an earlier, preempted attempt (the
    /// overlap queries are memoized: an attempt again reports nothing twice).
    pub fn report_coherence(&self, prog: &Program, orphan_done: &mut bool) {
        if !*orphan_done {
            let errors = self.orphan_errors(prog);
            for (imp, pos, message) in errors {
                self.dcx.report(crate::next_solver::fulfill::ObligationCause { body_id: imp, span: pos }, message);
            }
            *orphan_done = true;
        }
        self.coherence_check(prog);
    }

    /// A local type for the orphan rules: an ADT or trait object of crate `krate`, or one behind
    /// a fundamental type (`&`, `&mut`, `Box`, `Pin`).
    fn orphan_local(&self, prog: &Program, t: TyId, krate: u32) -> bool {
        match self.tys.kind(t) {
            TyKind::Adt(d, args, _) => {
                if prog.def(*d).krate == krate {
                    return true;
                }
                self.fundamental(prog, *d) && self.tys.list(*args).first().map_or(false, |a| self.orphan_local(prog, *a, krate))
            }
            TyKind::Dyn(td, ..) => prog.def(*td).krate == krate,
            TyKind::Ref(_, x, _) => self.orphan_local(prog, *x, krate),
            _ => false,
        }
    }

    /// `#[fundamental]` types: std's Box and Pin, and any type declared `#[fundamental]`.
    fn fundamental(&self, prog: &Program, d: DefId) -> bool {
        let k = prog.syms.str(prog.crates[prog.def(d).krate as usize].name);
        if matches!(prog.name(d), "Box" | "Pin") && matches!(k, "core" | "alloc" | "std") {
            return true;
        }
        let def = prog.def(d);
        let f = &prog.files[def.file as usize];
        f.ast.list(f.ast.item(def.item).attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "fundamental")
    }

    /// rustc_hir_analysis/src/hir_ty_lowering/mod.rs:3604 retains all header fields.
    fn lower_fn_header(&self, cx: &EvalCx<'_>, prog: &Program, file: u32, unsafe_: bool,
        abi: Option<Option<ast::Ident>>, c_variadic: bool) -> FnHeader {
        let abi = match abi {
            None => Abi::Rust,
            Some(None) => Abi::C,
            Some(Some(a)) => match Abi::from_name(&crate::lit::unescape_str(prog.files[file as usize].text(a))) {
                Some(abi) => {
                    let target = &prog.session.options.target;
                    let arch = if target.is_empty() { std::env::consts::ARCH } else { target.split('-').next().unwrap() };
                    if !abi.is_supported_on_arch(arch) {
                        self.err(cx, prog, file, a.lo, format!("E0570: \"{}\" is not a supported ABI for the current target", abi.name()));
                    }
                    abi
                }
                None => {
                    let written = crate::lit::unescape_str(prog.files[file as usize].text(a));
                    if Abi::is_rustc_name(&written) {
                        self.err(cx, prog, file, a.lo, format!("unsupported function ABI {}", prog.files[file as usize].text(a)));
                    } else {
                        self.err(cx, prog, file, a.lo, format!("E0703: invalid ABI: found `{}`", written));
                    }
                    Abi::Rust
                }
            },
        };
        FnHeader { safety: if unsafe_ { Safety::Unsafe } else { Safety::Safe }, abi, c_variadic }
    }

    /// Whether the errors of code in `file` are reported (a checked crate): the validation-only
    /// checks (generic argument counts, cast validation) skip every other crate -- std, provided
    /// and framework code -- so `makepad run` does no extra work there.
    #[inline]
    pub fn validates(&self, prog: &Program, file: u32) -> bool {
        prog.file_crate.get(file as usize).is_some_and(|&k| prog.crates[k as usize].validate)
    }

    pub(crate) fn err(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, pos: u32, msg: String) {
        let m = prog.record_error(file, pos, "typeck", msg);
        if let Some(m) = match cx.state.errors.borrow_mut().as_mut() {
            Some(v) => {
                v.push(m);
                None
            }
            None => Some(m),
        } {
            self.err_sink.lock().unwrap_or_else(|e| e.into_inner()).push(m);
        }
    }

    /// Reports an already formatted error again (one taken out of a with_errors capture).
    pub fn reraise_error(&self, cx: &crate::tcx::EvalCx<'_>, m: String) {
        if let Some(m) = match cx.state.errors.borrow_mut().as_mut() {
            Some(v) => {
                v.push(m);
                None
            }
            None => Some(m),
        } {
            self.err_sink.lock().unwrap_or_else(|e| e.into_inner()).push(m);
        }
    }

    /// Errors reported so far by this evaluation's captured step (None: not capturing).
    /// Runs `f` with this evaluation's errors captured; returns its result and the errors.
    /// with_errors in two halves, for a caller that needs `&mut self` meanwhile: starts
    /// capturing and returns what was capturing before (give it back to `end_errors`).
    pub fn begin_errors(&self, cx: &crate::tcx::EvalCx<'_>) -> Option<Vec<String>> {
        cx.state.errors.replace(Some(Vec::new()))
    }

    /// Ends a begin_errors capture: the errors reported since.
    pub fn end_errors(&self, cx: &crate::tcx::EvalCx<'_>, outer: Option<Vec<String>>) -> Vec<String> {
        cx.state.errors.replace(outer).unwrap_or_default()
    }

    pub fn with_errors<R>(&self, cx: &crate::tcx::EvalCx<'_>, f: impl FnOnce() -> R) -> (R, Vec<String>) {
        let outer = cx.state.errors.replace(Some(Vec::new()));
        let r = f();
        let errors = cx.state.errors.replace(outer).unwrap_or_default();
        (r, errors)
    }

    fn captured_len(&self, cx: &crate::tcx::EvalCx<'_>) -> Option<usize> {
        cx.state.errors.borrow().as_ref().map(|v| v.len())
    }

    /// Publish declaration changes under the exclusive front-end borrow.
    /// See QueryCx::retire_settled.
    pub fn retire_settled(&mut self) -> (usize, usize) { self.queries.retire_settled() }

    pub fn publish_collected(&mut self) {
        self.publish_declarations(None);
    }

    /// Lead decision 52 (R1): publish a declaration change that only the crates in
    /// `visible` (reload::crates_seeing_edit) can see; None: every crate. The owner
    /// queries of every other crate keep their results (type_queries.rs DECLARATIONS).
    pub fn publish_declarations(&mut self, visible: Option<&[bool]>) {
        self.declarations.invalidate_visible(visible);
        self.publish_declaration_inputs(visible);
    }

    // Collection already reset declaration cells before lowering. In particular, a
    // deferred load preserves successful cells even though solver inputs change.
    fn publish_declaration_inputs(&mut self, visible: Option<&[bool]>) {
        self.publish_declaration_inputs_with_solver(visible, false);
    }

    fn publish_declaration_inputs_with_solver(&mut self, visible: Option<&[bool]>, keep_solver: bool) {
        self.variances.take();
        for cell in self.sigs.decoded_values_mut() {
            if let Some(p) = cell.get_mut() {
                p.variances.take();
            }
        }
        let epoch = self.collect_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if keep_solver { self.queries.collected_with_solver(epoch, visible, true); }
        else { self.queries.collected(epoch, visible); }
    }

    /// Round-47 file input publication. Both version ids are invalidated so old
    /// dependencies cannot retain file-relative coordinates across edit/revert.
    pub fn publish_file_bodies(&mut self, files: &[u32]) {
        let epoch = self.collect_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.queries.file_bodies_changed(files, epoch);
    }

    /// collect_declarations, then coherence (check_coherence): every collection but a run's
    /// startup, which checks coherence in the background.
    pub fn collect(&mut self, prog: &Program) {
        self.collect_declarations(prog);
        self.check_coherence(prog);
        self.seal_collected_diagnostics();
    }

    /// Collects impls and ADTs; registers functions and constants for their declaration
    /// queries. Coherence is not checked (check_coherence / report_coherence).
    pub fn collect_declarations(&mut self, prog: &Program) {
        self.tys.ptr_bytes = prog.ptr_bytes;
        self.declarations = Default::default();
        self.queries.register_files(prog.files.len());
        self.queries.register_crates(prog.crates.len());
        let cx_state = crate::tcx::EvalState::new();
        let cx = &EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &cx_state);
        self.lang_items = LangItems::default();
        let mut defs = Vec::with_capacity(prog.defs.len());
        for i in 0..prog.defs.len() { defs.push(DefId(i as u32)); }
        self.lang_items.collect(prog, &defs, &mut self.errors);
        self.collect_all(cx, prog);
        self.collect_nominal_region_contracts(cx, prog, defs.iter().copied());
        self.collect_adt_destructors(prog);
        self.seal_collected_diagnostics();
        self.publish_collected();
    }

    fn collect_all(&mut self, cx: &crate::tcx::EvalCx<'_>, prog: &Program) {
        let _mem = crate::memstat::tag(crate::memstat::Tag::Sigs);
        let t0 = std::time::Instant::now();
        // impls first: their self types key the index. Each impl's header is lowered in
        // parallel (`&self`), then recorded in impl order with its errors and constant
        // requests, as the sequential loop did.
        let idx: Vec<u32> = (0..prog.impls.len() as u32).collect();
        let heads = par_map(&idx, |ii| {
            let cx_state = crate::tcx::EvalState::new();
            let cx = &EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &cx_state);
            cx.state.par_impls.set(true);
            let h = self.impl_head_captured(cx, prog, *ii as usize);
            cx.state.par_impls.set(false);
            h
        });
        for (ii, h) in heads.into_iter().enumerate() {
            let h = match h {
                // a header that read the impl tables is lowered again now: with exactly the
                // impls before it recorded, as the sequential collection saw them
                Some(h) if h.reads_impls => match self.impl_head_captured(cx, prog, ii) {
                    Some(h) => h,
                    None => continue,
                },
                Some(h) => h,
                None => continue,
            };
            let im = &prog.impls[ii];
            self.err_sink.get_mut().unwrap_or_else(|e| e.into_inner()).extend(h.errors);
            let st = h.self_ty;
            self.impl_self.insert(im.def, st);
            let key = ty_key(&self.tys, st);
            match h.trait_ {
                None => {
                    self.inherent.entry(key).or_default().push(im.def);
                }
                Some(None) => {}
                Some(Some((td, targs))) => {
                    self.impl_trait.insert(im.def, td);
                    self.trait_impls.entry((key, td)).or_default().push(im.def);
                    self.trait_impls_of.entry(td).or_default().push(im.def);
                    self.impls_by_key.entry(key).or_default().push(im.def);
                    self.impl_trait_args.insert(im.def, targs);
                }
            }
            self.impl_trait_regions.insert(im.def, h.trait_regions);
        }
        // trait args naming another impl's constant (`impl Tr<{ u64::BYTE_LEN }, 0> for X`,
        // the const coming from `impl Tr<8, 0> for u64`): with every header recorded now, those
        // left unevaluated are lowered once more (rustc evaluates them lazily, in any order)
        for ii in 0..prog.impls.len() {
            let d = prog.impls[ii].def;
            let bad = self.impl_trait_args.get(&d).map_or(false, |a| a.iter().any(|t| *t == self.tys.error));
            if !bad {
                continue;
            }
            if let Some(h) = self.impl_head_captured(cx, prog, ii) {
                if let Some(Some((td, targs))) = h.trait_ {
                    if self.impl_trait.get(&d) == Some(&td) && !targs.iter().any(|t| *t == self.tys.error) {
                        self.impl_trait_args.insert(d, targs);
                        self.impl_trait_regions.insert(d, h.trait_regions);
                    }
                }
            }
        }
        let t1 = std::time::Instant::now();
        for ii in 0..prog.impls.len() {
            let d = prog.impls[ii].def;
            self.resolve_impl_bounds(cx, d, prog);
        }
        let t1b = std::time::Instant::now();
        for di in 0..prog.defs.len() {
            let d = DefId(di as u32);
            if prog.def(d).kind == DefKind::Trait {
                if prog.def_path(d) == "core::marker::Copy" { self.copy_trait = Some(d); }
            }
        }
        for di in 0..prog.defs.len() {
            let d = DefId(di as u32);
            if prog.def(d).kind == DefKind::Trait {
                let mut all = Vec::new();
                let mut todo = vec![d];
                while let Some(x) = todo.pop() {
                    if !all.contains(&x) {
                        all.push(x);
                        todo.extend(self.trait_supers(prog, x));
                    }
                }
                self.trait_closure.insert(d, all);
            }
        }
        let t2 = std::time::Instant::now();
        let adts: Vec<DefId> = (0..prog.defs.len() as u32).map(DefId).filter(|d| matches!(prog.def(*d).kind, DefKind::Struct | DefKind::Enum | DefKind::Union)).collect();
        let parts = par_map(&adts, |d| self.adt_parts_captured(&EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &crate::tcx::EvalState::new()), prog, *d));
        for (d, p) in adts.into_iter().zip(parts) {
            self.apply_adt(d, p);
        }
        let t3 = std::time::Instant::now();
        // Allocate only the publication cells. Function and const/static declarations
        // are lowered by their first reader, after impl/ADT inputs are complete.
        for di in 0..prog.defs.len() {
            let d = DefId(di as u32);
            let def = prog.def(d);
            match def.kind {
                DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                    self.sigs.insert(d, Default::default());
                }
                DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic => {
                    self.const_tys.insert_declaration(d);
                }
                _ => {}
            }
        }
        if crate::preload::trace() {
            let t4 = std::time::Instant::now();
            let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1e3;
            eprintln!("boot-trace: signatures: impls {:.1}, impl bounds {:.1}, traits {:.1}, adts {:.1}, fn slots + consts {:.1} ms", ms(t0, t1), ms(t1, t1b), ms(t1b, t2), ms(t2, t3), ms(t3, t4));
        }
        self.flush_errs();
    }

    /// Hot reload: signatures, types and impl tables of defs a new file version added (the
    /// per-item steps of `collect`). Returns the errors they made.
    /// `own_crate`: the defs are a crate of their own added to the program (a run-time module).
    /// No cached answer about the program's types can change then -- the orphan rule keeps a new
    /// crate's impls to its own traits or its own types -- so the selection caches stay.
    pub fn collect_added(&mut self, prog: &Program, defs: &[DefId], own_crate: bool) -> Vec<String> {
        self.collect_added_in(prog, defs, own_crate, None)
    }

    /// collect_added for a reload whose edit only the crates in `visible` can see (lead
    /// decision 52 R1): the nominal region contracts of the other crates' items name only
    /// items of those crates' dependencies, unchanged, and their queries keep their results.
    pub fn collect_added_in(&mut self, prog: &Program, defs: &[DefId], own_crate: bool, visible: Option<&[bool]>) -> Vec<String> {
        self.queries.register_files(prog.files.len());
        self.queries.register_crates(prog.crates.len());
        let cx_state = crate::tcx::EvalState::new();
        let cx = &EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &cx_state);
        let lang_items = self.lang_items.items.clone();
        let r = self.collect_added_inner(cx, prog, defs, own_crate, false, visible);
        if self.lang_items.items != lang_items {
            // Language items are process-wide inputs, even when supplied by a new crate.
            self.publish_declaration_inputs(None);
        } else if own_crate {
            let mut own = vec![false; prog.crates.len()];
            for &d in defs { own[prog.def(d).krate as usize] = true; }
            // Concrete selection queries keep their upstream crate inputs. Anonymous
            // inference goals can still observe a new impl of an existing trait.
            self.publish_declaration_inputs_with_solver(Some(&own), self.additions_preserve_solver(prog, defs));
        } else {
            self.publish_declaration_inputs(visible);
        }
        r
    }

    /// collect_added_in for a deferred module load (the post-boot loader, lazy loading): the
    /// load adds `defs` to the crates and only the crates in `visible` see them. A load changes
    /// no existing item, so an existing item's nominal region contract changes only where its
    /// computation looked up a name in a module not loaded then (contracts_waiting) -- every
    /// other lookup answers as before; a reload's edit instead recomputes every contract its
    /// crates see. The contracts computed are the added items' and the waiting ones'.
    pub fn collect_loaded(&mut self, prog: &Program, defs: &[DefId], visible: &[bool]) -> Vec<String> {
        self.queries.register_files(prog.files.len());
        self.queries.register_crates(prog.crates.len());
        let cx_state = crate::tcx::EvalState::new();
        let cx = &EvalCx::new(prog, self, TypingEnv::fully_monomorphized(&prog), &cx_state);
        let lang_items = self.lang_items.items.clone();
        let r = self.collect_added_inner(cx, prog, defs, false, true, Some(visible));
        let keep_solver = self.additions_preserve_solver(prog, defs);
        if self.lang_items.items != lang_items {
            self.publish_declaration_inputs(None);
        } else {
            self.publish_declaration_inputs_with_solver(Some(visible), keep_solver);
        }
        r
    }

    fn additions_preserve_solver(&self, prog: &Program, defs: &[DefId]) -> bool {
        // rustc's solver reads trait_impls_of: a new impl of an existing trait can
        // change even a successful answer. New traits and inherent impls of new types
        // cannot be named by old goals except through a deferred miss.
        let added: crate::fxhash::HashSet<DefId> = defs.iter().copied().collect();
        if defs.iter().any(|&d| {
            let def = prog.def(d);
            matches!(def.kind, DefKind::AssocFn | DefKind::AssocTy | DefKind::AssocConst | DefKind::Field | DefKind::Variant)
                && !added.contains(&def.parent)
        }) { return false; }
        defs.iter().filter(|&&d| prog.def(d).kind == DefKind::Impl).all(|d| {
            if let Some(td) = self.impl_trait.get(d) { return added.contains(td); }
            // An unresolved trait path must not be mistaken for an inherent impl.
            let def = prog.def(*d);
            if !{ let a = &prog.files[def.file as usize].ast; a.as_impl(&a.item(def.item).kind).is_some_and(|i| i.trait_.is_none()) } { return false; }
            self.impl_self.get(d).is_some_and(|ty| matches!(self.tys.kind(*ty), TyKind::Adt(td, ..) if added.contains(td)))
        })
    }

    fn collect_added_inner(&mut self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, defs: &[DefId], own_crate: bool, loaded: bool, visible: Option<&[bool]>) -> Vec<String> {
        // errors reported before this collection (body checks since the last one) are not its
        // errors: they go to `errors` first, and only this collection's are returned
        self.flush_errs();
        let n0 = self.errors.len();
        self.reset_const_decls_with_misses();
        self.declarations.prepare(prog, loaded, own_crate, visible);
        self.declarations.invalidate(defs);
        self.lang_items.collect(prog, defs, &mut self.errors);
        let mut impls_added = Vec::new();
        for &d in defs {
            if prog.def(d).kind != DefKind::Impl {
                continue;
            }
            let ii = prog.def(d).sub as usize;
            let h = match self.impl_head_captured(cx, prog, ii) {
                Some(h) => h,
                None => continue,
            };
            self.err_sink.get_mut().unwrap_or_else(|e| e.into_inner()).extend(h.errors);
            let st = h.self_ty;
            self.impl_self.insert(d, st);
            let key = ty_key(&self.tys, st);
            match h.trait_ {
                None => self.inherent.entry(key).or_default().push(d),
                Some(None) => {}
                Some(Some((td, targs))) => {
                    self.impl_trait.insert(d, td);
                    self.trait_impls.entry((key, td)).or_default().push(d);
                    self.trait_impls_of.entry(td).or_default().push(d);
                    self.impls_by_key.entry(key).or_default().push(d);
                    self.impl_trait_args.insert(d, targs);
                }
            }
            self.impl_trait_regions.insert(d, h.trait_regions);
            impls_added.push(d);
        }
        for &d in &impls_added {
            self.resolve_impl_bounds(cx, d, prog);
        }
        let adts: Vec<DefId> = defs.iter().copied().filter(|d| matches!(prog.def(*d).kind, DefKind::Struct | DefKind::Enum | DefKind::Union)).collect();
        for d in adts {
            let p = self.adt_parts_captured(cx, prog, d);
            self.apply_adt(d, p);
        }
        for &d in defs {
            match prog.def(d).kind {
                DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                    self.sigs.insert(d, Default::default());
                }
                DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic => {
                    self.const_tys.insert_declaration(d);
                }
                _ => {}
            }
        }
        self.flush_errs();
        // A crate of its own (a run-time module): nothing in the program names its items, so
        // only its own contracts are new. A reload recomputes every contract (a changed trait,
        // alias or associated type feeds the contracts of the items that name it).
        let sees = |d: &DefId| visible.map_or(true, |v| v.get(prog.def(*d).krate as usize).copied().unwrap_or(true));
        if own_crate {
            self.collect_nominal_region_contracts(cx, prog, defs.iter().copied());
        } else if loaded {
            let mut items: Vec<DefId> = self.contracts_waiting.iter().copied().filter(sees).collect();
            items.extend_from_slice(defs);
            items.sort_unstable_by_key(|d| d.0);
            items.dedup();
            self.collect_nominal_region_contracts(cx, prog, items.into_iter());
        } else {
            self.collect_nominal_region_contracts(cx, prog, (0..prog.defs.len() as u32).map(DefId).filter(sees));
        }
        // A run-time module can only implement Drop for its own types (orphan rule, checked
        // before it is accepted): the host's destructors stay, its new ADTs are looked up.
        if own_crate {
            let adts: Vec<DefId> = defs.iter().copied().filter(|d| matches!(prog.def(*d).kind, DefKind::Struct | DefKind::Enum | DefKind::Union)).collect();
            self.collect_adt_destructors_of(prog, &adts);
        } else {
            self.collect_adt_destructors(prog);
        }
        self.errors[n0..].to_vec()
    }

    /// A single-writer check session discards answers tied to old bodies and definitions.
    /// The trait solver's goal and proof caches are dropped, as a declaration edit drops them
    /// (type_queries.rs `collected_with_solver`): everything in them is derived and refills on
    /// use. Returned to be freed outside the caller's locks.
    pub fn retire_solver_cache(&mut self) -> Box<dyn Send> {
        Box::new(std::mem::take(&mut self.queries.solver))
    }

    /// A commit added defs (a body edit's closures, anonymous constants, opaque types: created by
    /// def_collector at the commit, never "added items"): the per-def declaration table covers
    /// them. Without it the borrow checker of the edited fn read past the table's end ("index
    /// out of bounds: the len is N but the index is N+1": a closure added by a live patch).
    pub fn grow_to(&mut self, prog: &Program) {
        self.declarations.grow(prog.defs.len());
    }

    pub fn invalidate_check(&mut self, gone: &[DefId]) {
        self.declarations.invalidate(gone);
        self.alias_tys.clear();
        self.inferred_outlives.clear();
        if gone.is_empty() { self.publish_collected(); return; }
        let gone: crate::fxhash::HashSet<_> = gone.iter().copied().collect();
        self.sigs.retain(|d, _| !gone.contains(d));
        self.adts.retain(|d, _| !gone.contains(d));
        self.ctor_sigs.retain(|d, _| !gone.contains(d));
        self.const_tys.retain(|d| !gone.contains(d));
        self.nominal_region_contracts.retain(|d, _| !gone.contains(d));
        self.contracts_waiting.retain(|d| !gone.contains(d));
        self.impl_self.retain(|d, _| !gone.contains(d));
        self.impl_trait.retain(|d, _| !gone.contains(d));
        for impls in self.trait_impls_of.values_mut() { impls.retain(|d| !gone.contains(d)); }
        self.opaques.retain(|k, _| !gone.contains(k));
        self.trait_closure.retain(|d, _| !gone.contains(d));
        self.publish_collected();
    }

    /// Ordinary bodies do not change impl selection or signatures. An opaque return
    /// type is the exception: its inferred hidden type participates in those answers.
    pub fn invalidate_check_bodies(&mut self, prog: &Program, changed: &[DefId]) {
        let mut opaque = false;
        self.opaques.retain(|d, _| { opaque |= changed.contains(&prog.opaque_owner(*d)); true });
        if opaque {
            self.invalidate_check(&[]);
        } else {
        }
    }

    /// The metadata that collect_added does not need for hot code installation, but which
    /// a full checker must refresh after trait/item-set edits.
    pub fn collect_check(&mut self, prog: &Program, defs: &[DefId]) {
        self.collect_added(prog, defs, false);
        for &d in defs {
            let errors = self.signature_errors(prog, d).to_vec();
            self.errors.extend(errors);
            if prog.def(d).kind == DefKind::Trait {
                let mut all = Vec::new(); let mut todo = vec![d];
                while let Some(x) = todo.pop() { if !all.contains(&x) { all.push(x); todo.extend(self.trait_supers(prog, x)); } }
                self.trait_closure.insert(d, all);
            }
        }
        self.flush_errs();
        self.publish_collected();
        self.check_coherence(prog);
    }

    /// Hot reload: takes impls a refused version added out of the impl tables.
    pub fn uncollect_impls(&mut self, impls: &[DefId]) {
        if impls.is_empty() {
            return;
        }
        // a set: every impl list of the program is filtered against them (a slice scan per
        // entry was 2% of a `makepad check makepad-scope`)
        let impls: crate::fxhash::HashSet<DefId> = impls.iter().copied().collect();
        let impls = &impls;
        self.adt_destructors.retain(|adt, _| !self.destructor_impls.get(adt).is_some_and(|imp| impls.contains(imp)));
        self.destructor_impls.retain(|_, imp| !impls.contains(imp));
        for v in self.inherent.values_mut() {
            v.retain(|d| !impls.contains(d));
        }
        for v in self.trait_impls_of.values_mut() {
            v.retain(|d| !impls.contains(d));
        }
        for v in self.trait_impls.values_mut() {
            v.retain(|d| !impls.contains(d));
        }
        for v in self.impls_by_key.values_mut() {
            v.retain(|d| !impls.contains(d));
        }
        self.publish_collected();
    }

    /// rustc_hir_analysis/src/check/mod.rs:124; rustc_middle/src/ty/util.rs:376.
    /// E0120/E0367/E0366 applicability and coherence validation belong to T8.
    pub fn adt_destructor(&self, adt: DefId) -> Option<Destructor> {
        self.adt_destructors.get(&adt).copied()
    }

    fn collect_adt_destructors(&mut self, prog: &Program) {
        self.adt_destructors.clear();
        self.destructor_impls.clear();
        let adts: Vec<DefId> = self.adts.keys().copied().collect();
        self.collect_adt_destructors_of(prog, &adts);
    }

    fn collect_adt_destructors_of(&mut self, prog: &Program, adts: &[DefId]) {
        let Some(drop_trait) = self.lang_items.get(LangItem::Drop) else { return };
        for &adt in adts {
            let Some(impls) = self.trait_impls.get(&(TyKey::Adt(adt), drop_trait)) else { continue };
            for &imp in impls {
                let Some(&self_ty) = self.impl_self.get(&imp) else { continue };
                if !matches!(self.tys.kind(self_ty), TyKind::Adt(d, _, _) if *d == adt) { continue; }
                let data = &prog.impls[prog.def(imp).sub as usize];
                let Some(&did) = data.items.first() else { continue };
                if prog.def(did).kind == DefKind::AssocFn {
                    self.adt_destructors.insert(adt, Destructor { did });
                    self.destructor_impls.insert(adt, imp);
                }
            }
        }
    }

    /// Module scope used to resolve names inside an item.
    pub fn item_module(&self, prog: &Program, d: DefId) -> u32 {
        prog.def(d).scope
    }

    /// Generic environment of an item (impl/trait generics first, then the item's own).
    pub fn item_env(&self, prog: &Program, d: DefId) -> GenEnv {
        if matches!(prog.def(d).kind, DefKind::Prim(_) | DefKind::Mod | DefKind::MacroRules) { return GenEnv::default(); }
        let mut owner = d;
        while matches!(prog.def(owner).kind, DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Closure | DefKind::Field | DefKind::Variant) { owner = prog.def(owner).parent; }
        let mut env = self.item_generic_env(prog, d);
        env.regions = self.bound_region_env(prog, owner);
        env
    }

    pub(crate) fn item_generic_env(&self, prog: &Program, d: DefId) -> GenEnv {
        let def = prog.def(d);
        if matches!(def.kind, DefKind::Prim(_) | DefKind::Mod | DefKind::MacroRules) { return GenEnv::default(); }
        // rustc_hir_analysis/src/collect/generics_of.rs:96-100: MCG bodies
        // have no generic parent; repeat counts and inline consts retain theirs.
        if def.kind == DefKind::AnonConst && !prog.repeat_expr_counts.contains(&d)
            && matches!(prog.def_id_to_node_id.get(&d), Some((_, relax_front::def_collector::NodeId::AnonConst(_)))) {
            let parent = self.item_env(prog, def.parent);
            return GenEnv { self_ty: parent.self_ty, ..Default::default() };
        }
        if matches!(def.kind, DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Closure | DefKind::Field | DefKind::Variant) {
            return self.item_generic_env(prog, def.parent);
        }
        if def.kind == DefKind::Impl {
            if let Some(env) = self.impl_generics(prog, d) { return env; }
        }
        let mut env = GenEnv::default();
        if def.parent != NO_DEF {
            let pd = prog.def(def.parent);
            if pd.kind == DefKind::Impl {
                if let Some(e) = self.impl_generics(prog, def.parent) {
                    env = e;
                }
            } else if pd.kind == DefKind::Trait {
                let ast = &prog.files[pd.file as usize].ast;
                if let Some(TraitItem { generics, .. }) = ast.as_trait(&ast.item(pd.item).kind) {
                    // `Self` is param 0 inside traits
                    env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    env.regions.next_param += 1;
                    push_generics(prog, pd.file, generics, &mut env);
                    env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
                    env.trait_def = Some(def.parent);
                }
            }
        }
        env.parent_bounds = env.bounds.len();
        env.owner = Some(d);
        let ast = &prog.files[def.file as usize].ast;
        let it = ast.item(def.item);
        match &it.kind {
            ItemKind::Fn(sig, _, _) => {
                let sig = ast.fn_sig(*sig);
                push_generics(prog, def.file, &sig.generics, &mut env);
                // each `impl Trait` in a parameter type is an anonymous generic param, also nested
                // in generic args, tuples, slices, arrays (`&mut Image2d<u8, impl AsMut<[u8]>>`;
                // rustc: synthetic params in order of appearance)
                let mut found = Vec::new();
                for p in ast.list(sig.params) {
                    collect_argument_impl_traits(ast, p.ty, &mut found);
                }
                for t in found {
                    if let ast::Ty::ImplTrait(bs) = ast.ty(t) {
                        let pi = env.names.len() as u32;
                        env.impl_params.push((t.0, pi));
                        env.names.push(u32::MAX);
                        env.sized_params.push(pi);
                        // its bounds are the anonymous param's (`impl IntoIterator<Item =
                        // u32>` fixes `1..=3` to u32 like `I: IntoIterator<Item = u32>`)
                        for b in ast.list(*bs) {
                            if let ast::Bound::Trait { path, maybe, .. } = b {
                                if *maybe { env.sized_params.retain(|p| *p != pi); continue; }
                                env.bounds.push((pi, def.file, *path));
                            }
                        }
                    }
                }
            }
            ItemKind::Struct(_) | ItemKind::Enum(_) | ItemKind::Union(_) => {
                push_generics(prog, def.file, ast.kind_generics(&ast.item(def.item).kind).unwrap(), &mut env);
                // hir_ty_lowering/mod.rs:2270: Self denotes this declaration
                // instantiated with its identity arguments, also in field consts.
                let indices = crate::types::variance::GenericIndices::from_env(&env);
                let regions: Vec<_> = indices.lifetimes.iter().map(|&i| self.tys.intern_region(crate::types::RegionKind::ReEarlyParam(i))).collect();
                env.self_ty = Some(self.tys.intern(TyKind::Adt(d, self.tys.mk_list(&indices.identity_types_consts(&self.tys)), self.tys.intern_region_args(&regions))));
            }
            ItemKind::TypeAlias(r) => push_generics(prog, def.file, &ast.row(*r).generics, &mut env),
            // a trait's own parameters (defaults of `trait Tr<const N: u8 = 1, const M: u8 = N>`
            // name earlier ones): `Self` is param 0, as for the trait's items
            ItemKind::Trait(_) => {
                if let Some(TraitItem { generics, .. }) = ast.as_trait(&it.kind) {
                    env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    env.regions.next_param += 1;
                    push_generics(prog, def.file, generics, &mut env);
                    env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
                    env.trait_def = Some(d);
                }
            }
            ItemKind::Impl(r) => {
                // collect/generics_of.rs: impl parameters are available while
                // lowering the impl header, before its self type is collected.
                push_generics(prog, def.file, &ast.row(*r).generics, &mut env);
                env.impl_def = Some(d);
            }
            _ => {}
        }
        env.regions.enabled = false;
        env
    }

    /// An impl's generic environment, rebuilt from its declaration: the collected tables keep only
    /// the self type (`impl_self`), everything else here is a function of the impl's AST and its
    /// lifetime resolution. None when the impl is not collected.
    pub(crate) fn impl_generics(&self, prog: &Program, d: DefId) -> Option<GenEnv> {
        let st = *self.impl_self.get(&d)?;
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let generics = &ast.as_impl(&ast.item(def.item).kind)?.generics;
        let mut env = self.impl_header_env(prog, d, def.file, generics);
        env.self_ty = Some(st);
        Some(env)
    }

    /// Type and const parameter count of a collected impl (the parent generics of its items).
    fn impl_generic_count(&self, prog: &Program, d: DefId) -> u32 {
        if !self.impl_self.contains_key(&d) { return 0; }
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let Some(imp) = ast.as_impl(&ast.item(def.item).kind) else { return 0 };
        let generics = &imp.generics;
        ast.list(generics.params).iter().filter(|g| !matches!(g.kind, GenericParamKind::Lifetime(_))).count() as u32
    }

    /// The impl's parameters and lifetime resolution, before its self type is lowered.
    fn impl_header_env(&self, prog: &Program, d: DefId, file: u32, generics: &ast::Generics) -> GenEnv {
        let mut env = GenEnv::default();
        push_generics(prog, file, generics, &mut env);
        env.regions = self.bound_region_env(prog, d);
        env.impl_def = Some(d);
        env
    }

    /// One impl's header with this evaluation's errors captured; None for a non-impl item.
    fn impl_head_captured(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, ii: usize) -> Option<ImplHead> {
        let im = &prog.impls[ii];
        let ast = &prog.files[im.file as usize].ast;
        let (generics, trait_, self_ty) = match ast.as_impl(&ast.item(im.item).kind) {
            Some(ImplItem { generics, trait_, self_ty, .. }) => (generics, trait_, *self_ty),
            None => return None,
        };
        let outer = cx.state.errors.replace(Some(Vec::new()));
        let read0 = cx.state.impl_read.replace(false);
        let mut env = self.impl_header_env(prog, im.def, im.file, generics);
        let st = self.lower_ty(cx, prog, im.file, im.module, &env, self_ty);
        env.self_ty = Some(st);
        let trait_regions = trait_.as_ref().map(|p| self.lower_region_args(cx.infcx, prog, im.file, im.module, &env, p)).unwrap_or(RegionArgsId::EMPTY);
        let tr = trait_.as_ref().map(|tp| {
            self.resolve_trait_path(cx, prog, im.file, im.module, &env, tp).and_then(|td| {
                // rustc_hir_analysis/src/coherence/mod.rs:73-94. Freeze impls,
                // specialization-trait attrs and impl restrictions are feature-gated cuts.
                let trait_def = prog.def(td);
                let trait_ast = &prog.files[trait_def.file as usize].ast;
                if crate::lang_items::has_attr(prog, trait_def.file, trait_ast.list(trait_ast.item(trait_def.item).attrs), "rustc_deny_explicit_impl") {
                    self.err(cx, prog, im.file, tp.lo, format!("{}: explicit impls for the `{}` trait are not permitted", if self.lang_items.get(crate::lang_items::LangItem::Unsize) == Some(td) { "E0328" } else { "E0322" }, prog.name(td)));
                    return None;
                }
                // (an impl header's trait ref takes no associated item constraints)
                if let Some(last) = prog.files[im.file as usize].ast.segs(tp.segs).last() { self.prohibit_constraints(cx, prog, im.file, last.args()); }
                let explicit = self.lower_generic_args(cx, prog, im.file, im.module, &env, tp);
                Some((td, self.fill_trait_defaults(cx, prog, td, st, explicit)))
            })
        });
        let errors = cx.state.errors.replace(outer).unwrap_or_default();
        let reads_impls = cx.state.impl_read.replace(read0);
        Some(ImplHead { self_ty: st, trait_: tr, errors, reads_impls, trait_regions })
    }



    /// adt_parts with this evaluation's errors captured into the result.
    /// A struct's definition collected again from its current text, not recorded (a live layout
    /// edit reconciles it with the running one first, reload.rs layout_edit): the ADT and errors.
    pub fn adt_fresh(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId) -> (Option<AdtT>, Vec<String>) {
        let p = self.adt_parts_captured(cx, prog, d);
        (p.adt, p.errors)
    }

    fn adt_parts_captured(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId) -> AdtParts {
        let mut parts = AdtParts { adt: None, errors: Vec::new() };
        let outer = cx.state.errors.replace(Some(Vec::new()));
        self.adt_parts(cx, prog, d, &mut parts);
        parts.errors = cx.state.errors.replace(outer).unwrap_or_default();
        parts
    }

    /// Records one ADT's collection (derives and errors in their order).
    fn apply_adt(&mut self, d: DefId, p: AdtParts) {
        self.variances.take();
        self.err_sink.get_mut().unwrap_or_else(|e| e.into_inner()).extend(p.errors);
        if let Some(adt) = p.adt {
            // a tuple constructor is a fn item (rustc: its own FnDef type with the ADT's args):
            // its signature, fields to the ADT
            if !adt.is_union {
                let n = adt.n_generics;
                let pargs = adt.generic_indices.identity_types_consts(&self.tys);
                let ret = self.tys.intern(TyKind::Adt(d, self.tys.mk_list(&pargs), adt.region_params));
                for v in adt.variants.iter().filter(|v| v.shape == 1) {
                    self.ctor_sigs.insert(v.def, FnSigT {
                        region_contract: adt.region_contract.clone(),
                        generic_indices: adt.generic_indices.clone(),
                        bound_regions: 0,
                        params: v.fields.iter().map(|f| f.ty).collect(),
                        ret,
                        self_kind: 0,
                        n_generics: n,
                        n_parent_generics: if adt.is_enum { n } else { 0 },
                        is_const: false,
                        rustc_promotable: false,
                        rustc_intrinsic: false,
                        header: FnHeader::RUST,
                        track_caller: false,
                        target_features: Vec::new(),
                        safe_target_features: false,
                    });
                }
            }
            self.adts.insert(d, adt);
        }
    }

    fn adt_parts(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId, parts: &mut AdtParts) {
        let def = prog.def(d);
        let file = def.file;
        let module = self.item_module(prog, d);
        let mut env = self.item_env(prog, d);
        env.regions.enabled = true;
        let n_generics = env.names.len() as u32;
        let self_args = crate::types::variance::GenericIndices::from_env(&env).identity_types_consts(&self.tys);
        env.self_ty = Some(self.tys.intern(TyKind::Adt(d, self.tys.mk_list(&self_args), env.regions.identity_args(&self.tys))));
        let ast = &prog.files[file as usize].ast;
        let it = ast.item(def.item);
        let mut repr_c = false;
        let mut repr_int = None;
        let mut repr_simd = false;
        let (repr_align, repr_packed) = read_repr(prog, file, ast.list(it.attrs), &mut repr_c, &mut repr_int, &mut repr_simd);
        let (valid_start, valid_end, no_niche) = read_layout_attrs(prog, file, ast.list(it.attrs));
        let param_maybe_unsized = match &it.kind {
            k @ (ItemKind::Struct(_) | ItemKind::Enum(_) | ItemKind::Union(_)) => maybe_unsized_params(prog, file, ast.kind_generics(k).unwrap()),
            _ => Vec::new(),
        };
        let mut flags = match it.kind {
            ItemKind::Enum(..) => AdtFlags::IS_ENUM,
            ItemKind::Union(..) => AdtFlags::IS_UNION,
            _ => AdtFlags::IS_STRUCT,
        };
        if matches!(it.kind, ItemKind::Struct(r) if matches!(ast.row(r).data, VariantData::Unit | VariantData::Tuple(_))) {
            flags.insert(AdtFlags::HAS_CTOR);
        }
        if repr_simd && matches!(it.kind, ItemKind::Struct(..)) {
            flags.insert(AdtFlags::IS_SIMD);
        }
        if matches!(it.kind, ItemKind::Enum(..)) && crate::lang_items::has_attr(prog, file, ast.list(it.attrs), "non_exhaustive") {
            flags.insert(AdtFlags::IS_VARIANT_LIST_NON_EXHAUSTIVE);
        }
        if let Some(lang) = self.lang_items.by_def.get(&d) {
            let identity = match lang {
                LangItem::OwnedBox => AdtFlags::IS_BOX,
                LangItem::PhantomData => AdtFlags::IS_PHANTOM_DATA,
                LangItem::ManuallyDrop => AdtFlags::IS_MANUALLY_DROP,
                LangItem::UnsafeCell => AdtFlags::IS_UNSAFE_CELL,
                LangItem::UnsafePinned => AdtFlags::IS_UNSAFE_PINNED,
                _ => AdtFlags::default(),
            };
            flags.insert(identity);
        }
        let mut adt = AdtT { region_contract: self.region_contract(cx, prog, d, &env), generic_indices: crate::types::variance::GenericIndices::from_env(&env), flags, region_params: env.regions.identity_args(&self.tys), is_enum: false, is_union: false, n_generics, variants: Vec::new(), repr_c, repr_int, repr_align, repr_packed, valid_start, valid_end, no_niche, param_maybe_unsized, ext_from: u32::MAX, ext_conv: Vec::new() };
        match &it.kind {
            ItemKind::Struct(r) => {
                let (shape, fields) = self.lower_fields(cx, prog, file, module, &env, &ast.row(*r).data);
                adt.variants.push(VariantT { def: d, name: def.name, shape, fields, discr: VariantDiscr::Relative(0) });
            }
            ItemKind::Union(r) => {
                adt.is_union = true;
                let (_, f) = self.lower_fields(cx, prog, file, module, &env, &VariantData::Struct(ast.row(*r).fields));
                adt.variants.push(VariantT { def: d, name: def.name, shape: 2, fields: f, discr: VariantDiscr::Relative(0) });
            }
            ItemKind::Enum(r) => {
                let vars = &ast.row(*r).variants;
                adt.is_enum = true;
                // rustc_hir_analysis/src/collect.rs:1044-1059: declarations
                // retain anonymous constant identities; evaluation is a query.
                let mut distance_from_explicit = 0;
                let discr_ty = self.tys.intern(TyKind::Int(repr_int.unwrap_or(Prim::Isize)));
                for (vi, v) in ast.list(*vars).iter().enumerate() {
                    let (shape, fields) = self.lower_fields(cx, prog, file, module, &env, &v.data);
                    let discr = if let Some(e) = v.disc {
                        distance_from_explicit = 0;
                        let did = prog.node_id_to_def_id[&(file, relax_front::def_collector::NodeId::AnonConst(e))];
                        self.const_tys.feed(did, discr_ty);
                        VariantDiscr::Explicit(did)
                    } else {
                        VariantDiscr::Relative(distance_from_explicit)
                    };
                    distance_from_explicit += 1;
                    let vdef = prog.variants[def.sub as usize][vi];
                    adt.variants.push(VariantT { def: vdef, name: prog.def(vdef).name, shape, fields, discr });
                }
            }
            _ => {}
        }
        parts.adt = Some(adt);
    }

    fn lower_fields(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, data: &VariantData) -> (u8, Vec<FieldT>) {
        let mut out = Vec::new();
        let (shape, fields) = match data {
            VariantData::Unit => (0, None),
            VariantData::Tuple(f) => (1, Some(f)),
            VariantData::Struct(f) => (2, Some(f)),
        };
        if let Some(fs) = fields {
            for (i, f) in prog.files[file as usize].ast.list(*fs).iter().enumerate() {
                let ty = self.lower_ty(cx, prog, file, module, env, f.ty);
                let name = match f.name {
                    Some(n) => {
                        let s = prog.files[file as usize].text(n);
                        prog.syms.get(s).unwrap_or(u32::MAX)
                    }
                    None => i as u32 | 0x8000_0000,
                };
                let vis_mod = prog.vis_module(file, module, &f.vis).unwrap_or(u32::MAX);
                out.push(FieldT { name, ty, is_pub: !matches!(f.vis, ast::Vis::Private), vis_mod });
            }
        }
        (shape, out)
    }



    /// sig_parts with this evaluation's errors captured into the result.
    fn sig_parts_captured(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId) -> SigParts {
        let mut parts = SigParts::default();
        let outer = cx.state.errors.replace(Some(Vec::new()));
        self.sig_parts(cx, prog, d, &mut parts);
        parts.errors = cx.state.errors.replace(outer).unwrap_or_default();
        parts
    }

    /// Publish a complete declaration under its own OnceLock. Readers of unrelated
    /// functions never contend, and an initialized read takes no lock.
    #[inline]
    pub(crate) fn signature_parts(&self, prog: &Program, d: DefId) -> Option<&SigParts> {
        let cell = self.sigs.get(&d)?;
        Some(match cell.get() {
            Some(parts) => parts,
            None => self.init_signature(prog, d, cell),
        })
    }

    #[cold]
    fn init_signature<'a>(&'a self, prog: &Program, d: DefId, cell: &'a std::sync::OnceLock<Box<SigParts>>) -> &'a SigParts {
        // (computed outside the cell's Once: relax_base::query::init_once)
        let diagnostics = std::cell::Cell::new(None);
        let value: &Box<_> = crate::query::init_once(cell, || {
            if crate::why::enabled() && prog.files.get(prog.def(d).file as usize)
                .and_then(|f| f.ast.items.get(prog.def(d).item.0 as usize)).is_some() {
                let cause = prog.why_def(d, "signature-demand");
                crate::why::event("item-demand", &cause.file, cause.line, &cause.symbol, None);
            }
            let _mem = crate::memstat::tag(crate::memstat::Tag::Sigs);
            let state = EvalState::new();
            let mut typing_env = TypingEnv::fully_monomorphized(prog);
            typing_env.limit = RecursionLimit::for_item(prog, d);
            let cx = EvalCx::new(prog, self, typing_env, &state);
            let checkpoint = self.dcx.checkpoint();
            let parts = self.sig_parts_captured(&cx, prog, d);
            if !parts.errors.is_empty() { cx.errors().note_reported_error(); }
            diagnostics.set(Some(cx.errors().take_since(checkpoint)));
            Box::new(parts)
        }, |parts| {
            let (errors, warnings) = diagnostics.take().unwrap_or_default();
            for error in errors {
                let owner = if error.cause.body_id == NO_DEF { d } else { error.cause.body_id };
                parts.errors.push(prog.record_error(prog.def(owner).file, error.cause.span, "signature", error.message));
            }
            for warning in warnings { eprintln!("warning: {}", warning.message); }
        });
        value
    }

    /// fn_sig(d).header.abi without lowering the signature: lower_fn_header reads it from
    /// the written header alone (an unknown ABI is reported by the signature and is Rust
    /// there), so a reader that needs only the ABI (a slot's entry linkage) does not compute
    /// the signature and its owner's bound-variable table. None: no fn signature.
    pub fn fn_abi(&self, prog: &Program, d: DefId) -> Option<Abi> {
        let cell = self.sigs.get(&d)?;
        if let Some(parts) = cell.get() { return parts.sig.as_ref().map(|s| s.header.abi); }
        let def = prog.def(d);
        let file = &prog.files[def.file as usize];
        let ItemKind::Fn(sig, _, _) = &file.ast.item(def.item).kind else { return None };
        Some(match file.ast.fn_sig(*sig).abi {
            None => Abi::Rust,
            Some(None) => Abi::C,
            Some(Some(a)) => Abi::from_name(&crate::lit::unescape_str(file.text(a))).unwrap_or(Abi::Rust),
        })
    }

    #[inline]
    /// Signatures computed with errors (a type in a module the lazy selection had skipped) are
    /// computed again on next use: the post-boot loader calls this after it loads modules.
    /// Only errored ones: a correct signature never changes when items are added.
    pub fn forget_error_signatures(&mut self) -> usize {
        let errored: Vec<DefId> = self.sigs.iter().filter(|(_, cell)| cell.get().is_some_and(|p| {
            !p.errors.is_empty() || p.sig.as_ref().is_some_and(|sig| sig.params.iter().any(|t| self.tys.has_error(*t)) || self.tys.has_error(sig.ret))
        })).map(|(d, _)| *d).collect();
        for d in &errored { self.sigs.insert(*d, std::sync::OnceLock::new()); }
        errored.len()
    }

    pub fn fn_sig(&self, prog: &Program, d: DefId) -> Option<&FnSigT> {
        self.signature_parts(prog, d)?.sig.as_ref()
    }

    /// Diagnostics belong to the declaration and are available to every checker,
    /// including readers that arrive after another worker initialized it.
    /// A const/static item's are its declared type's (Tcx::const_ty).
    pub fn signature_errors(&self, prog: &Program, d: DefId) -> &[String] {
        if let Some(cell) = self.const_tys.declaration(&d) { return &self.const_decl(prog, d, cell).errors; }
        self.signature_parts(prog, d).map_or(&[], |p| p.errors.as_slice())
    }

    /// A full check requests declarations without bodies too (foreign and trait
    /// methods). Runtime loading only requests the declarations it uses.
    pub fn check_signature_errors(&self, prog: &Program, krate: u32) -> Vec<String> {
        let mut errors = Vec::new();
        for (i, def) in prog.defs.iter().enumerate() {
            if def.krate == krate {
                errors.extend(self.check_def_signature(prog, DefId(i as u32)));
            }
        }
        errors
    }

    /// One definition's share of `check_signature_errors` (a parallel check runs these on
    /// several threads: every part is a query over the shared context).
    pub fn check_def_signature(&self, prog: &Program, did: DefId) -> Vec<String> {
        let def = prog.def(did);
        let mut errors = Vec::new();
        if prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            errors.extend(self.check_item_type_defaults(prog, did));
        }
        errors.extend_from_slice(self.signature_errors(prog, did));
        if def.kind == DefKind::OpaqueTy && self.validates(prog, def.file) {
            for e in self.opaque_bound_errors(prog, did) { if !errors.contains(e) { errors.push(e.clone()); } }
        }
        if prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            errors.extend(crate::item_checks::duplicate_generic_params(prog, did));
            if self.validates(prog, def.file) { errors.extend(crate::item_checks::generic_param_rules(prog, did)); }
            errors.extend(crate::item_checks::repr_checks(prog, self, did));
            errors.extend(crate::item_checks::assoc_const_elision(prog, did));
            errors.extend(crate::item_checks::attribute_safety(prog, did));
            errors.extend(crate::item_checks::attribute_shapes(prog, did));
            if self.validates(prog, def.file) { errors.extend(crate::item_checks::precise_capture_params(prog, self, did)); }
            if def.kind == DefKind::AssocFn && self.validates(prog, def.file) && errors.is_empty() {
                errors.extend(self.check_method_receiver(prog, did));
            }
        }
        // predicates_of: the bounds and where-clauses of every item are lowered, used or not
        if prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            for e in self.own_clauses_errors(prog, did) {
                if !errors.contains(e) { errors.push(e.clone()); }
            }
            if def.kind == DefKind::AssocTy {
                for e in self.assoc_item_bounds_errors(prog, did) {
                    if !errors.contains(&e) { errors.push(e); }
                }
            }
        }
        // rustc_hir_analysis check_main_fn_ty: the crate's `main` takes no generic parameters
        if def.kind == DefKind::Fn && prog.name(did) == "main" && prog.item_defs.get(&(def.file, def.item.0)) == Some(&did)
            && def.parent == prog.mods[prog.crates[def.krate as usize].root_mod as usize].def
            && !self.generic_indices_of(prog, did).types_consts.is_empty() {
            errors.push(prog.record_error(def.file, prog.files[def.file as usize].ast.item(def.item).lo, "signature", "E0131: `main` function is not allowed to have generic parameters".into()));
        }
        if def.kind == DefKind::AssocFn && prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            // rustc_ast_passes ast_validation check_trait_fn_not_const (E0379)
            let ast = &prog.files[def.file as usize].ast;
            if let crate::ast::ItemKind::Fn(sig, _, _) = &ast.item(def.item).kind {
                if ast.fn_sig(*sig).konst {
                    let parent = prog.def(def.parent);
                    let which = match parent.kind {
                        DefKind::Trait => Some("traits"),
                        DefKind::Impl if self.impl_trait.contains_key(&def.parent) => Some("trait impls"),
                        _ => None,
                    };
                    if let Some(which) = which {
                        errors.push(prog.record_error(def.file, ast.item(def.item).lo, "typeck", format!("E0379: functions in {} cannot be declared const", which)));
                    }
                }
            }
        }
        if def.kind == DefKind::Enum {
            errors.extend(self.adts[&did].check_discriminants(self, prog));
        }
        if matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union) && prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            errors.extend(crate::item_checks::representability(prog, self, did));
            // (a definition with errors has no meaningful variances)
            errors.extend(self.check_type_defn_variances(prog, did));
            if def.kind == DefKind::Union { errors.extend(crate::item_checks::union_fields(prog, self, did)); }
        }
        if def.kind == DefKind::TypeAlias && prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            for e in self.check_type_alias(prog, did) { if !errors.contains(&e) { errors.push(e); } }
        }
        if prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            errors.extend(crate::item_checks::bound_assoc_cycles(prog, self, did));
        }
        if def.kind == DefKind::Trait && prog.item_defs.get(&(def.file, def.item.0)) == Some(&did) {
            errors.extend(crate::item_checks::supertrait_cycles(prog, self, did));
        }
        if def.kind == DefKind::Impl {
            let unconstrained = self.check_impl_params_constrained(prog, did);
            // (an impl with an unconstrained parameter has no sound item types to compare)
            let compare = unconstrained.is_empty();
            errors.extend(unconstrained);
            if compare { errors.extend(self.check_impl_items_against_trait(prog, did)); }
            errors.extend(crate::item_checks::copy_impl(prog, self, did));
        }
        errors
    }

    /// rustc_hir_analysis/src/check/check.rs:765-805 and collect.rs:1770-1798:
    /// lower defaults even when the item is unused, feeding real anon bodies
    /// their declared type before the body-owner check requests type_of.
    fn check_item_type_defaults(&self, prog: &Program, d: DefId) -> Vec<String> {
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let generics = match ast.kind_generics(&ast.item(def.item).kind) {
            Some(g) => *g,
            None => return Vec::new(),
        };
        let env = self.item_env(prog, d);
        let state = EvalState::new();
        state.errors.replace(Some(Vec::new()));
        let cx = EvalCx::new(prog, self, TypingEnv::fully_monomorphized(prog), &state);
        let mut param_wf = Vec::new();
        for param in ast.list(generics.params) {
            match param.kind {
                GenericParamKind::Type(_, Some(ty)) => { self.lower_ty(&cx, prog, def.file, def.scope, &env, ty); }
                GenericParamKind::Const(ty, default) => {
                    let ty = self.lower_ty(&cx, prog, def.file, def.scope, &env, ty);
                    if let Some(value) = default { self.lower_const_arg(prog, def.file, &env, value, ty); }
                    if self.validates(prog, def.file) {
                        param_wf.extend(self.const_param_ty_wf(prog, d, param.name.lo, ty));
                    }
                }
                _ => {}
            }
        }
        // a type alias's right-hand side is lowered where it is written (rustc type_of of the alias),
        // used or not
        if def.kind == DefKind::TypeAlias {
            if let Some(TypeAliasItem { ty: Some(t), .. }) = ast.as_alias(&ast.item(def.item).kind) {
                let mut env = self.item_env(prog, d);
                env.regions.enabled = true;
                self.lower_ty(&cx, prog, def.file, self.item_module(prog, d), &env, *t);
            }
        }
        // the types the where clauses bound name existing types (rustc resolves them whether or
        // not the item is used: late resolution of the generics' predicates)
        let mut found = state.errors.replace(Some(Vec::new())).unwrap_or_default();
        if matches!(def.kind, DefKind::Fn | DefKind::Impl) {
            for w in ast.list(generics.where_) {
                // a bound on a plain name (`where Missing: Trait`); a path through `Self` or a
                // projection waits for the item's own bounds
                if let ast::WherePred::Bound { ty, .. } = w {
                    if matches!(ast.ty(*ty), ast::Ty::Path(p) if p.segs.len() == 1 && p.no_qself() && !p.global()) {
                        self.lower_ty(&cx, prog, def.file, def.scope, &env, *ty);
                    }
                }
            }
        }
        found.extend(state.errors.take().unwrap_or_default());
        found.extend(param_wf);
        found
    }

    /// rustc_hir_analysis wfcheck.rs check_param_wf without `adt_const_params` /
    /// `min_adt_const_params` / `const_param_ty_unchecked`: a const parameter's type is an
    /// integer, `bool` or `char`. (Types carry no span: the error is at the parameter.)
    fn const_param_ty_wf(&self, prog: &Program, d: DefId, pos: u32, ty: TyId) -> Option<String> {
        let def = prog.def(d);
        let features = prog.crates[def.krate as usize].features(prog);
        if features.has("adt_const_params") || features.has("min_adt_const_params") || features.has("const_param_ty_unchecked") {
            return None;
        }
        let msg = match self.tys.kind(ty) {
            // (a parameter type that names another parameter is E0770, reported by resolution)
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Error | TyKind::Param(_) => return None,
            TyKind::FnPtr(..) => "using function pointers as const generic parameters is forbidden".to_string(),
            TyKind::Ptr(..) => "using raw pointers as const generic parameters is forbidden".to_string(),
            _ => {
                if self.tys.has_error(ty) { return None; }
                format!("`{}` is forbidden as the type of a const generic parameter", crate::typeck::ty_to_string(false, prog, self, ty))
            }
        };
        Some(prog.record_uncoded_error(def.file, pos, "signature", msg))
    }

    pub fn signature_count(&self) -> usize {
        self.sigs.values().filter(|cell| cell.get().is_some()).count()
    }

    pub fn signature_bytes(&self) -> usize {
        self.sigs.capacity() * (std::mem::size_of::<DefId>() + std::mem::size_of::<std::sync::OnceLock<Box<SigParts>>>() + 1)
            + self.signature_count() * std::mem::size_of::<SigParts>()
    }

    #[inline]
    pub fn fn_trait_bounds(&self, prog: &Program, d: DefId) -> Option<&Vec<(u32, DefId, Vec<TyId>)>> {
        self.signature_parts(prog, d).map(|p| &p.trait_bounds).filter(|v| !v.is_empty())
    }
    #[inline]
    pub fn fn_ty_bounds(&self, prog: &Program, d: DefId) -> Option<&Vec<(TyId, DefId, Vec<TyId>)>> {
        self.signature_parts(prog, d).map(|p| &p.ty_bounds).filter(|v| !v.is_empty())
    }
    #[inline]
    pub fn self_sized_method(&self, prog: &Program, d: DefId) -> bool {
        self.signature_parts(prog, d).is_some_and(|p| p.self_sized)
    }

    #[cfg(test)]
    pub(crate) fn insert_test_signature(&mut self, d: DefId, sig: FnSigT) {
        self.sigs.insert(d, std::sync::OnceLock::from(Box::new(SigParts { sig: Some(sig), ..Default::default() })));
    }

    fn sig_parts(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId, parts: &mut SigParts) {
        let def = prog.def(d);
        let file = def.file;
        let module = self.item_module(prog, d);
        let env = self.item_env(prog, d);
        let n_parent = match prog.def(def.parent).kind {
            DefKind::Impl => self.impl_generic_count(prog, def.parent),
            DefKind::Trait => 1 + self.trait_generic_count(prog, def.parent) as u32,
            _ => 0,
        };
        let ast = &prog.files[file as usize].ast;
        let sig = match &ast.item(def.item).kind {
            ItemKind::Fn(s, _, _) => ast.fn_sig(*s),
            _ => return,
        };
        let mut params = Vec::new();
        let mut self_kind = 0;
        if let Some(sp) = &sig.self_param {
            let st = env.self_ty.unwrap_or(self.tys.error);
            let t = match sp {
                ast::SelfParam::Value(_) => {
                    self_kind = 1;
                    st
                }
                ast::SelfParam::Ref(lt, m) => {
                    self_kind = if *m { 3 } else { 2 };
                    self.tys.intern(TyKind::Ref(*m, st, env.regions.resolve_key(cx.infcx, &self.tys, lt.map_or(resolve_bound_vars::LifetimeKey::SelfReference, |lt| resolve_bound_vars::LifetimeKey::Named(lt.lo, lt.hi)))))
                }
                ast::SelfParam::Typed(_, t) => {
                    self_kind = 1;
                    let lt = self.lower_ty(cx, prog, file, module, &env, *t);
                    if let TyKind::Ref(m, _, _) = self.tys.kind(lt) {
                        self_kind = if *m { 3 } else { 2 };
                    }
                    lt
                }
            };
            params.push(t);
        }
        for p in ast.list(sig.params) {
            params.push(self.lower_ty(cx, prog, file, module, &env, p.ty));
        }
        let bound_regions = env.regions.bound_count();
        let ret = match sig.ret {
            Some(r) => {
                let mut renv = env.clone();
                renv.opaque_owner = Some(d);
                self.lower_ty(cx, prog, file, module, &renv, r)
            }
            None => self.tys.unit,
        };
        // `async fn f() -> T` returns `impl Future<Output = T>` (rustc's async fn desugaring;
        // the hidden type is the body's coroutine)
        // (a foreign fn's `async` is rejected by ast validation and ignored by lowering: no opaque)
        let ret = if sig.asyncness && prog.def(d).kind != DefKind::ForeignFn { self.async_opaque(prog, d, ret) } else { ret };
        let mut callable_signatures = Vec::new();
        self.collect_callable_signatures(cx, prog, file, module, &env, &sig.generics, n_parent, &mut callable_signatures);
        // Associated items inherit their parent's predicates, including callable
        // Output equalities (rustc_middle/src/ty/generics.rs:431-441 @8d1a764).
        let parent = prog.def(def.parent);
        let mut parent_env = None;
        if matches!(parent.kind, DefKind::Impl | DefKind::Trait) {
            let parent_ast = &prog.files[parent.file as usize].ast;
            let (generics, offset) = match &parent_ast.item(parent.item).kind {
                ItemKind::Impl(r) => (&parent_ast.row(*r).generics, 0),
                ItemKind::Trait(r) => (&parent_ast.row(*r).generics, 1),
                _ => unreachable!(),
            };
            let mut penv = env.clone();
            penv.impl_params.clear();
            // The parent's bounds resolve their lifetimes and object lifetime defaults in the
            // parent's own resolve_bound_vars table (rustc lowers predicates_of(parent) in the
            // parent's ItemCtxt): the method's table has no entry for the parent's nodes, which
            // left `F: Fn() -> Box<dyn Tr>` on an impl indeterminate (E0228).
            penv.regions = self.bound_region_env(prog, def.parent);
            self.collect_callable_signatures(cx, prog, parent.file, module, &penv, generics, offset, &mut callable_signatures);
            parent_env = Some(penv);
        }
        let mut tb = Vec::new();
        for (bi, (pi, bfile, path)) in env.bounds.clone().into_iter().enumerate() {
            // the parent's own bounds lower in the parent's table too (same reason as above:
            // `F: Fn(usize, &(dyn Fn(usize) + Sync))` on an impl was E0228 in `FnExecutor::new`)
            let env = match &parent_env {
                Some(penv) if bi < env.parent_bounds => penv,
                _ => &env,
            };
            if let Some(td) = self.resolve_type_path_def(prog, bfile, module, &path) {
                if pi == 0 && n_parent > 0 && prog.name(td) == "Sized" && prog.def(def.parent).kind == DefKind::Trait {
                    parts.self_sized = true;
                }
                if prog.def(td).kind != DefKind::Trait {
                    continue;
                }
                let targs = self.lower_generic_args(cx, prog, bfile, module, &env, &path);
                // (default trait args filled: `D: OverlayEdgeData` is OverlayEdgeData<ShapeCountBoolean>,
                // as the signature's `D::Store` projection spells it, and a binding's projection too)
                let p = self.tys.intern(TyKind::Param(pi));
                let targs = self.fill_trait_defaults(cx, prog, td, p, targs);
                // `I: IntoIterator<Item = D>`: the binding's type is lowered for its
                // diagnostics; the projection clause itself is native (clauses_of)
                let bast = &prog.files[bfile as usize].ast;
                if let Some(GenericArgs::Angle(v)) = bast.segs(path.segs).last().and_then(|l| bast.gargs(l.args())) {
                    for g in bast.list(*v) {
                        if let GenericArg::Binding(n, _, ty) = g {
                            let sym = prog.syms.get(prog.files[bfile as usize].text(*n)).unwrap_or(u32::MAX);
                            if self.trait_assoc(prog, td, sym).is_some() { self.lower_ty(cx, prog, bfile, module, &env, *ty); }
                        }
                    }
                }
                tb.push((pi, td, targs));
            }
        }
        // a trait's method has the trait's own predicate `Self: Trait<P..>` (rustc
        // predicates_of a trait item): a call `From::from(5)` / `T::from(x)` selects the impl
        // through it, fixing the trait args from the unique applicable impl
        if prog.def(def.parent).kind == DefKind::Trait {
            let nt = self.trait_generic_count(prog, def.parent) as u32;
            let targs: Vec<TyId> = (1..=nt).map(|i| self.tys.intern(TyKind::Param(i))).collect();
            if !targs.is_empty() {
                tb.push((0, def.parent, targs));
            }
        }
        if let Some(sized) = self.lang_items.get(LangItem::Sized) {
            for &pi in &env.sized_params { tb.push((pi, sized, Vec::new())); }
        }
        parts.trait_bounds = tb;
        // where-clauses on a type that is not one of the params (rustc predicates_of)
        let mut tyb = Vec::new();
        let f = &prog.files[file as usize];
        // (and the enclosing impl's: `impl<A: Array> SmallVec<A> where A::Item: Clone`)
        let impl_where: &[ast::WherePred] = if prog.def(def.parent).kind == DefKind::Impl {
            let im = &prog.impls[prog.def(def.parent).sub as usize];
            match prog.files[im.file as usize].ast.as_impl(&prog.files[im.file as usize].ast.item(im.item).kind) {
                Some(ImplItem { generics, .. }) if im.file == file => f.ast.list(generics.where_),
                _ => &[],
            }
        } else {
            &[]
        };
        for w in f.ast.list(sig.generics.where_).iter().chain(impl_where.iter()) {
            if let ast::WherePred::Bound { ty, bounds, .. } = w {
                let simple = match f.ast.ty(*ty) {
                    ast::Ty::Path(p) => p.no_qself() && p.segs.len() == 1 && f.ast.segs(p.segs)[0].args().is_none(),
                    _ => false,
                };
                if simple {
                    continue;
                }
                let mut tds = Vec::new();
                for b in f.ast.list(*bounds) {
                    if let ast::Bound::Trait { path, maybe: false, .. } = b {
                        if let Some(td) = self.resolve_type_path_def(prog, file, module, path) {
                            if prog.def(td).kind == DefKind::Trait {
                                tds.push((td, path));
                            }
                        }
                    }
                }
                if tds.is_empty() {
                    continue;
                }
                // a self type Relax cannot lower here (`M::Item` through a where-clause
                // supertrait) only loses this inference aid: its errors are not the program's
                let nerr = self.captured_len(cx).unwrap_or(0);
                let st = self.lower_ty(cx, prog, file, module, &env, *ty);
                if st == self.tys.error || self.captured_len(cx).unwrap_or(0) != nerr {
                    {
                        if let Some(v) = cx.state.errors.borrow_mut().as_mut() {
                            v.truncate(nerr);
                        }
                     };
                    continue;
                }
                for (td, path) in tds {
                    let targs = self.lower_generic_args(cx, prog, file, module, &env, path);
                    tyb.push((st, td, targs));
                }
            }
        }
        parts.ty_bounds = tyb;
        let header = self.lower_fn_header(cx, prog, file, sig.unsafe_, sig.abi, sig.variadic);
        let target_features = self.target_feature_attrs(cx, prog, d, header);
        let header = if target_features.safe_target_features { FnHeader { safety: Safety::Unsafe, ..header } } else { header };
        let mut region_contract = self.region_contract(cx, prog, d, &env);
        region_contract.callable_signatures = callable_signatures;
        parts.sig = Some(FnSigT {
                region_contract, generic_indices: crate::types::variance::GenericIndices::from_env(&env),
                bound_regions,
                params,
                ret,
                self_kind,
                n_generics: env.names.len() as u32,
                n_parent_generics: n_parent,
                is_const: sig.konst,
                rustc_promotable: sig.konst && ast.list(ast.item(def.item).attrs).iter().any(|a| {
                    a.toks.hi > a.toks.lo && crate::cfg::tok_str(prog.src(file), a.toks.lo as usize) == b"rustc_promotable"
                }),
                rustc_intrinsic: ast.list(ast.item(def.item).attrs).iter().any(|a| {
                    a.toks.hi > a.toks.lo && crate::cfg::tok_str(prog.src(file), a.toks.lo as usize) == b"rustc_intrinsic"
                }),
                header,
                track_caller: {
                    let has = |d2: DefId| {
                        let dd = prog.def(d2);
                        let src = prog.src(dd.file);
                        prog.files[dd.file as usize].ast.list(prog.files[dd.file as usize].ast.item(dd.item).attrs).iter().any(|a| a.toks.hi > a.toks.lo && crate::cfg::tok_str(src, a.toks.lo as usize) == b"track_caller")
                    };
                    // (an `extern "C"` fn cannot take the implicit argument: C calls it); a trait
                    // method declared #[track_caller] makes its impls' methods tracked too (rustc
                    // codegen_fn_attrs inherits the attribute from the trait item)
                    !header.abi.is_c_like()
                        && (has(d)
                            || (prog.def(def.parent).kind == DefKind::Impl
                                && self.impl_trait.get(&def.parent).map_or(false, |td| prog.traits[prog.def(*td).sub as usize].iter().any(|&m| prog.def(m).name == def.name && prog.def(m).kind == DefKind::AssocFn && has(m)))))
                },
                target_features: target_features.target_features,
                safe_target_features: target_features.safe_target_features,
        });
    }

    /// TyCtxt::body_codegen_attrs(def).target_features (middle/ty/context.rs:857-873): fns have
    /// their own, consts none; a closure inherits its owner's (codegen_attrs.rs:339-346 -- Relax
    /// keeps no closure attributes, so an `#[inline(always)]` closure cannot opt out).
    pub fn body_target_features(&self, prog: &Program, mut def: DefId) -> Vec<crate::target_features::TargetFeature> {
        while prog.def(def).kind == DefKind::Closure { def = prog.def(def).parent; }
        match prog.def(def).kind {
            DefKind::Fn | DefKind::AssocFn => self.fn_item_sig(prog, def).map_or(Vec::new(), |s| s.target_features.clone()),
            _ => Vec::new(),
        }
    }

    /// TyCtxt::adjust_target_feature_sig (middle/ty/context.rs:1269-1284): `fun_def`'s signature
    /// is safe inside `caller` when the caller has every feature the callee enables.
    pub fn adjust_target_feature_sig(&self, prog: &Program, fun_def: DefId, caller: DefId) -> bool {
        let Some(sig) = self.fn_item_sig(prog, fun_def) else { return false };
        crate::target_features::is_target_feature_call_safe(&sig.target_features, &self.body_target_features(prog, caller))
    }

    /// L7: the `#[target_feature]` part of codegen_fn_attrs (codegen_ssa codegen_attrs.rs:102-133,
    /// 489-502; attr_parsing codegen_attrs.rs:574-587; target_features.rs:176-186) and the
    /// SafeTargetFeatures header rule (ast_lowering item.rs:1589-1597). Errors are reported once,
    /// when the signature is collected.
    fn target_feature_attrs(&self, cx: &EvalCx<'_>, prog: &Program, d: DefId, header: FnHeader) -> crate::target_features::TargetFeatureAttrs {
        use crate::target_features as tf;
        let def = prog.def(d);
        let file = def.file;
        let ast = &prog.files[file as usize].ast;
        let attrs = ast.list(ast.item(def.item).attrs);
        let mut errors = Vec::new();
        let (features, attr_pos) = tf::parse_target_feature_attrs(prog, file, attrs, &mut errors);
        let mut out = tf::TargetFeatureAttrs { attr_pos, ..Default::default() };
        if let Some(attr_pos) = attr_pos {
            let arch = tf::target_arch(prog);
            // ast_lowering: a safe fn with the attribute is unsafe in the type system
            out.safe_target_features = header.safety == Safety::Safe;
            if out.safe_target_features && matches!(def.kind, DefKind::AssocFn) {
                let parent = prog.def(def.parent).kind;
                if parent == DefKind::Trait || (parent == DefKind::Impl && self.impl_trait.contains_key(&def.parent)) {
                    errors.push((attr_pos, "`#[target_feature(..)]` cannot be applied to safe trait method".into()));
                }
            }
            tf::from_target_feature_attr(arch, &features, &mut out.target_features, &mut errors);
            // attr_parsing finalize_check: not on lang item functions (`#[panic_handler]` first)
            for kind in ["panic_handler", "lang"] {
                if crate::lang_items::has_attr(prog, file, attrs, kind) {
                    errors.push((attr_pos, format!("`#[target_feature]` cannot be applied to a {} function",
                        if kind == "panic_handler" { "`#[panic_handler]`" } else { "lang item" })));
                    break;
                }
            }
            let names: Vec<&str> = out.target_features.iter().map(|f| f.name).collect();
            if let Some(features) = tf::check_tied_features(arch, &names) {
                errors.push((attr_pos, format!("the target features {} must all be either enabled or disabled together", features.join(", "))));
            }
        }
        for (pos, message) in errors { self.err(cx, prog, file, pos, message); }
        out
    }

    /// Fn-family bounds on an item's generic params (inline, `where`, and `impl Fn(..)`).
    fn collect_callable_signatures(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, generics: &ast::Generics, n_parent: u32, callable_signatures: &mut Vec<callable::CallableSignature>) {
        let ast = &prog.files[file as usize].ast;
        let f = &prog.files[file as usize];
        let mut pending: Vec<(u32, &ast::Bound)> = Vec::new();
        let mut k = n_parent;
        for gp in ast.list(generics.params) {
            match &gp.kind {
                GenericParamKind::Type(bs, _) => {
                    for b in ast.list(*bs) {
                        pending.push((k, b));
                    }
                    k += 1;
                }
                GenericParamKind::Const(..) => k += 1,
                GenericParamKind::Lifetime(_) => {}
            }
        }
        for w in ast.list(generics.where_) {
            if let ast::WherePred::Bound { ty, bounds, .. } = w {
                if let ast::Ty::Path(p) = ast.ty(*ty) {
                    if p.segs.len() == 1 && prog.files[file as usize].ast.segs(p.segs)[0].args().is_none() {
                        let name = prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[0].name)).unwrap_or(u32::MAX - 1);
                        if let Some(i) = env.names.iter().rposition(|n| *n == name) {
                            if i as u32 >= n_parent {
                                for b in ast.list(*bounds) {
                                    pending.push((i as u32, b));
                                }
                            }
                        }
                    }
                }
            }
        }
        for &(t, i) in &env.impl_params {
            if let ast::Ty::ImplTrait(bs) = ast.ty(ast::TyId(t)) {
                for b in ast.list(*bs) {
                    pending.push((i, b));
                }
            }
        }
        for (i, b) in pending {
            if let ast::Bound::Trait { path, hrtb, .. } = b {
                if let Some(last) = prog.files[file as usize].ast.segs(path.segs).last() {
                    let n = f.text(last.name);
                    let is_async = matches!(n, "AsyncFn" | "AsyncFnMut" | "AsyncFnOnce");
                    if n == "Fn" || n == "FnMut" || n == "FnOnce" || is_async {
                        if let Some(a) = ast.gargs(last.args()) {
                            if let GenericArgs::Paren(ps, r) = a {
                                let fp = ast::FnPtr { hrtb: *hrtb, unsafe_: false, abi: None, params: *ps, ret: *r, variadic: false };
                                let bound = self.lower_fn_pointer(cx, prog, file, module, env, &fp, resolve_bound_vars::BinderId::Trait(path.segs.lo));
                                if let Some(trait_def) = self.resolve_type_path_def(prog, file, module, path) {
                                    callable_signatures.push(callable::CallableSignature { param: i, trait_def, signature: bound });
                                }
                            }
                        }
                    } else if let Some(td) = self.resolve_type_path_def(prog, file, module, path) {
                        // a trait whose supertrait is Fn sugar (`F: KeyFn<T, K>` with
                        // `trait KeyFn<T, K>: Fn(&T) -> K`): that signature (rustc elaborates the
                        // bound to its supertraits; closures passed for F are typed by it)
                        if prog.def(td).kind == DefKind::Trait && !matches!(n, "Fn" | "FnMut" | "FnOnce") {
                            let targs = self.lower_generic_args(cx, prog, file, module, env, path);
                            let p = self.tys.intern(TyKind::Param(i));
                            for (sd, sa) in self.trait_super_refs_of(cx, prog, td, p, &targs) {
                                if matches!(prog.name(sd), "Fn" | "FnMut" | "FnOnce") {
                                    if let Some(TyKind::Tuple(ps)) = sa.first().map(|t| self.tys.kind(*t)) {
                                        if let Some(output) = self.callable_output(sd) {
                                            let mut args = vec![p]; args.extend_from_slice(&sa);
                                            let projection = self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: output }, args: self.tys.mk_list(&args), regions: RegionArgsId::EMPTY }));
                                            for (lhs, ret) in self.trait_super_bindings(cx, prog, td, p, &targs) {
                                                if lhs == projection {
                                                    let sig = self.tys.intern(TyKind::FnPtr(ps.clone(), ret, FnHeader::RUST, 0));
                                                    callable_signatures.push(callable::CallableSignature { param: i, trait_def: sd, signature: sig });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// type_of for a const/static item (N:rustc_hir_analysis/src/collect/type_of.rs:228-262,
    /// ItemKind::Const/Static and TraitItem/ImplItem consts: the written type, lowered in the
    /// item's generic environment), with its region contract (what borrowck's universal
    /// regions of the constant's body read). On demand, like nightly's type_of query: the
    /// first reader lowers it; its errors stay with the declaration (signature_errors).
    /// None: not a const/static item (anonymous constants are fed, `ConstTypes`), or an
    /// item without a written type.
    pub fn const_ty(&self, prog: &Program, d: DefId) -> Option<TyId> {
        match self.const_tys.declaration(&d) {
            Some(cell) => self.const_decl(prog, d, cell).ty,
            None => self.const_tys.fed_type(&d),
        }
    }

    /// The region contract of a const/static item's declared type (None: not one, or no type).
    pub fn const_region_contract(&self, prog: &Program, d: DefId) -> Option<&outlives::RegionContract> {
        let cell = self.const_tys.declaration(&d)?;
        self.const_decl(prog, d, cell).region_contract.as_ref()
    }

    #[inline]
    fn const_decl<'a>(&'a self, prog: &Program, d: DefId, cell: &'a std::sync::OnceLock<Box<ConstDecl>>) -> &'a ConstDecl {
        let decl = match cell.get() {
            Some(decl) => decl,
            None => self.init_const_decl(prog, d, cell),
        };
        // the post-boot loader's misses travel with the declaration (as Body::deferred_misses):
        // a reader fails for the same skipped modules as the lowering, and the load that adds
        // them resets the declaration (reset_const_decls_with_misses)
        if !decl.misses.is_empty() { crate::program::note_misses(&decl.misses); }
        decl
    }

    /// A load (or reload) may add a name a constant's type missed: those declarations are
    /// lowered again by their next reader.
    fn reset_const_decls_with_misses(&mut self) {
        self.const_tys.reset_declarations_with_misses();
    }

    #[cold]
    fn init_const_decl<'a>(&'a self, prog: &Program, d: DefId, cell: &'a std::sync::OnceLock<Box<ConstDecl>>) -> &'a ConstDecl {
        // (computed outside the cell's Once: relax_base::query::init_once)
        let diagnostics = std::cell::Cell::new(None);
        let value: &Box<_> = crate::query::init_once(cell, || {
            let _mem = crate::memstat::tag(crate::memstat::Tag::Sigs);
            let state = EvalState::new();
            let cx = EvalCx::new(prog, self, TypingEnv::fully_monomorphized(prog), &state);
            let checkpoint = self.dcx.checkpoint();
            let outer = cx.state.errors.replace(Some(Vec::new()));
            let enclosing = crate::program::missing_save();
            let mut decl = self.const_decl_uncached(&cx, prog, d);
            decl.misses = crate::program::take_missing();
            crate::program::missing_restore(enclosing);
            decl.errors = cx.state.errors.replace(outer).unwrap_or_default();
            if !decl.errors.is_empty() { cx.errors().note_reported_error(); }
            diagnostics.set(Some(cx.errors().take_since(checkpoint)));
            Box::new(decl)
        }, |decl| {
            let (errors, warnings) = diagnostics.take().unwrap_or_default();
            for error in errors {
                let owner = if error.cause.body_id == NO_DEF { d } else { error.cause.body_id };
                decl.errors.push(prog.record_error(prog.def(owner).file, error.cause.span, "signature", error.message));
            }
            for warning in warnings { eprintln!("warning: {}", warning.message); }
        });
        value
    }

    fn const_decl_uncached(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId) -> ConstDecl {
        let def = prog.def(d);
        let file = def.file;
        let module = self.item_module(prog, d);
        let ast = &prog.files[file as usize].ast;
        let t = match &ast.item(def.item).kind {
            ItemKind::Const(t, _) if t.is_some() => t.get().unwrap(),
            ItemKind::Static(_, t, _) => *t,
            _ => return ConstDecl::default(),
        };
        let mut env = self.item_env(prog, d);
        env.regions.enabled = true;
        let ty = self.lower_ty(cx, prog, file, module, &env, t);
        ConstDecl { ty: Some(ty), region_contract: Some(self.region_contract(cx, prog, d, &env)), errors: Vec::new(), misses: Vec::new() }
    }

    // ------------------------------------------------------------ type lowering

    pub fn lower_ty(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, t: AstTy) -> TyId {
        let ast = &prog.files[file as usize].ast;
        match ast.ty(t) {
            ast::Ty::Tuple(v) => {
                let mut out = Vec::new();
                for x in ast.list(*v) {
                    out.push(self.lower_ty(cx, prog, file, module, env, *x));
                }
                self.tys.intern(TyKind::Tuple(self.tys.mk_list(&out)))
            }
            ast::Ty::Paren(x) => self.lower_ty(cx, prog, file, module, env, *x),
            ast::Ty::Never => self.tys.never,
            ast::Ty::Ref(lt, m, x) => {
                let r = env.regions.resolve_key(cx.infcx, &self.tys, lt.map_or(resolve_bound_vars::LifetimeKey::Reference(t.0), |lt| resolve_bound_vars::LifetimeKey::Named(lt.lo, lt.hi)));
                let inner = self.lower_ty(cx, prog, file, module, env, *x);
                self.tys.intern(TyKind::Ref(*m, inner, r))
            }
            ast::Ty::Ptr(m, x) => {
                let inner = self.lower_ty(cx, prog, file, module, env, *x);
                self.tys.intern(TyKind::Ptr(*m, inner))
            }
            ast::Ty::Slice(x) => {
                let inner = self.lower_ty(cx, prog, file, module, env, *x);
                self.tys.intern(TyKind::Slice(inner))
            }
            ast::Ty::Array(x, n) => {
                let inner = self.lower_ty(cx, prog, file, module, env, *x);
                if is_infer_len(ast, *n) {
                    let length = match cx.infcx {
                        Some(infcx) => infcx.next_const_var(),
                        None => {
                            // ItemCtxt::ct_infer: no `_` length in an item's signature either
                            if self.validates(prog, file) {
                                let pos = env.owner.filter(|o| prog.def(*o).file == file).map_or(0, |o| ast.item(prog.def(o).item).lo);
                                self.err(cx, prog, file, pos, "E0121: the placeholder `_` is not allowed within types on item signatures".to_string());
                            }
                            self.tys.intern_const(crate::consts::ConstKind::Error)
                        }
                    };
                    return self.tys.mk_array(inner, length);
                }
                let lt = self.lower_const_arg(prog, file, env, *n, self.tys.usize_);
                // `[u8; N]` with `const N: u64`: an array length is a usize (rustc E0308 "the
                // constant `N` is not of type `usize`")
                if let crate::consts::ConstKind::Param(crate::consts::ParamConst { index: i }) = self.tys.const_kind(lt) {
                    if let Some(&(_, cfile, cty)) = env.consts.iter().find(|c| c.0 == i) {
                        let cast = &prog.files[cfile as usize].ast;
                        if let ast::Ty::Path(cp) = cast.ty(cty) {
                            if cp.no_qself() && cp.segs.len() == 1 {
                                let tn = prog.files[cfile as usize].text(cast.segs(cp.segs)[0].name);
                                if crate::program::PRIMS.iter().any(|(pn, _)| *pn == tn) && tn != "usize" {
                                    let lo = ast.expr(*n).lo;
                                    let pn = env.names.get(i as usize).map_or(String::new(), |s| prog.syms.str(*s).to_string());
                                    self.err(cx, prog, file, lo, format!("E0308: the constant `{}` is not of type `usize` (it is `{}`)", pn, tn));
                                    return self.tys.error;
                                }
                            }
                        }
                    }
                }
                self.tys.mk_array(inner, lt)
            }
            ast::Ty::Fn(f) => self.lower_fn_pointer(cx, prog, file, module, env, ast.fn_ptr(*f), resolve_bound_vars::BinderId::FnPointer(t.0)),
            ast::Ty::Infer => match cx.infcx {
                Some(infcx) => infcx.next_ty_var(),
                None => {
                    // ItemCtxt::ty_infer: no `_` in an item's signature (types carry no position here: the item's)
                    if self.validates(prog, file) {
                        let pos = env.owner.filter(|o| prog.def(*o).file == file).map_or(0, |o| ast.item(prog.def(o).item).lo);
                        self.err(cx, prog, file, pos, "E0121: the placeholder `_` is not allowed within types on item signatures".to_string());
                    }
                    self.tys.error
                }
            },
            ast::Ty::Path(p) => self.lower_path_ty(cx, prog, file, module, env, p, Some(t)),
            ast::Ty::DynTrait(bounds, _) => self.lower_dyn(cx, prog, file, module, env, ast.list(*bounds), 0, Some(t)),
            ast::Ty::ImplTrait(_) if env.impl_params.iter().any(|x| x.0 == t.0) => {
                let i = env.impl_params.iter().find(|x| x.0 == t.0).unwrap().1;
                self.tys.intern(TyKind::Param(i))
            }
            ast::Ty::ImplTrait(bs) if env.opaque_owner.is_some() => self.lower_opaque(cx, prog, file, module, env, t, *bs),
            ast::Ty::Mac(m) => {
                let m = prog.files[file as usize].ast.row(*m);
                let lo = m.path.lo;
                let name = prog.files[file as usize].ast.segs(m.path.segs).last().map(|sg| prog.files[file as usize].text(sg.name).to_string()).unwrap_or_default();
                self.err(cx, prog, file, lo, format!("type macro `{}!` is not expanded here", name));
                self.tys.error
            }
            ast::Ty::ImplTrait(_) => {
                self.err(cx, prog, file, 0, "E0562: `impl Trait` is not allowed in paths or in this position".to_string());
                self.tys.error
            }
            _ => {
                self.err(cx, prog, file, 0, "unsupported type".to_string());
                self.tys.error
            }
        }
    }

    /// rustc_hir_analysis/src/hir_ty_lowering/mod.rs:2390; nontrivial bodies
    /// retain their definition and substitutions for const_eval_resolve.
    pub fn lower_const_arg(&self, prog: &Program, file: u32, env: &GenEnv, e: ast::ExprId, expected_ty: TyId) -> crate::consts::Const {
        use crate::consts::{AliasConst, AliasConstKind, ConstKind, IsRigid, ParamConst};
        let pf = &prog.files[file as usize];
        let expr = pf.ast.expr(e);
        // rustc_ast_lowering/src/lib.rs:3005-3013: one block unwrap,
        // not recursive evaluation of arbitrary generic const expressions.
        let unwrapped = match expr.kind {
            ast::ExprKind::Block(block, l) if l.is_none() => match pf.ast.stmts(block) {
                [ast::Stmt::Expr(inner, false)] => *inner,
                _ => e,
            },
            _ => e,
        };
        if let ast::ExprKind::Path(p) = pf.ast.expr(unwrapped).kind {
            if p.no_qself() && !p.global() && p.segs.len() == 1 {
                let name = prog.syms.get(pf.text(pf.ast.segs(p.segs)[0].name));
                for &(index, _, _) in &env.consts {
                    if env.names.get(index as usize).copied() == name {
                        return self.tys.intern_const(ConstKind::Param(ParamConst { index }));
                    }
                }
            }
        }
        // hir_ty_lowering/mod.rs:2416-2443: feed the expected parameter type
        // before evaluating the anonymous body; query inputs cannot contain inference.
        let expected_ty = if self.tys.has_infer(expected_ty) || self.tys.flags(expected_ty).intersects(crate::types::TypeFlags::HAS_TY_PARAM) { self.tys.error } else { expected_ty };
        let def_id = *prog.node_id_to_def_id.get(&(file, relax_front::def_collector::NodeId::AnonConst(e)))
            .unwrap_or_else(|| panic!("uncollected anonymous constant in {} at {}: {:?}", prog.file_paths[file as usize], expr.lo, expr.kind));
        // feed_anon_const_type (front-bck H5): a re-lowering whose expected type holds
        // inference/params (replaced by the error type above) never replaces the fed type.
        // A real type still replaces it: a live edit can change the parameter's type.
        if expected_ty != self.tys.error || self.const_tys.fed_type(&def_id).is_none() { self.const_tys.feed(def_id, expected_ty); }
        if expected_ty == self.tys.error { return self.tys.intern_const(ConstKind::Error); }
        // hir_ty_lowering/mod.rs:3083-3118 try_lower_anon_const_lit: a (negated) integer literal
        // whose kind matches the type (ty/consts/lit.rs:50-55 const_lit_matches_ty) is lowered
        // directly by lit_to_const, truncated to the type (thir/constant.rs:21-33,75-94; rustc's
        // deny-by-default overflowing_literals lint has no Relax counterpart). Anything else is
        // the anon const, whose typeck reports the mismatch (`Foo<3u16>` for a u8 parameter).
        let (literal, neg) = match pf.ast.expr(unwrapped).kind {
            ast::ExprKind::Unary(ast::UnOp::Neg, inner) if matches!(pf.ast.expr(inner).kind, ast::ExprKind::Lit(ast::LitKind::Int)) => (Some(inner), true),
            ast::ExprKind::Lit(ast::LitKind::Int) => (Some(unwrapped), false),
            _ => (None, false),
        };
        if let (Some(literal), TyKind::Int(prim)) = (literal, self.tys.kind(expected_ty)) {
            let le = pf.ast.expr(literal);
            let text = std::str::from_utf8(pf.bytes(le.lo, le.hi)).unwrap_or("");
            let (bytes, signed) = self.tys.int_info(*prim);
            let matches = match crate::const_fold::int_suffix(text) {
                None => signed || !neg,
                Some(suffix) => crate::program::PRIMS.iter().find(|(n, _)| *n == suffix)
                    .is_some_and(|(_, p)| self.tys.int_info(*p) == (bytes, signed) && (signed || !neg)),
            };
            if let (true, Some(value)) = (matches, crate::const_fold::parse_int_lit(text)) {
                let value = if neg { value.wrapping_neg() } else { value };
                let truncated = value & (u128::MAX >> (128 - bytes * 8));
                let scalar = crate::consts::ScalarInt::try_from_uint(truncated, bytes as u8).expect("truncated literal");
                return crate::consts::Const::new_value(&self.tys, crate::consts::ValTree::from_scalar_int(&self.tys, scalar), expected_ty);
            }
        }
        let args = if prog.repeat_expr_counts.contains(&def_id) {
            crate::types::variance::GenericIndices::from_env(env).identity_args(&self.tys)
        } else { Vec::new() };
        self.tys.intern_const(ConstKind::Alias(IsRigid::No, AliasConst { kind: AliasConstKind::Anon { def_id }, args: args.into() }))
    }

    /// The resolver's opaque definition inherits its enclosing generic arguments.
    /// rustc_ast_lowering/src/lib.rs:1853-1879; nested `Assoc = impl X` has its own DefId.
    fn lower_opaque(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, t: AstTy, bounds: ast::List<ast::Bound>) -> TyId {
        let bs = prog.files[file as usize].ast.list(bounds);
        let opaque_def = prog.node_id_to_def_id[&(file, relax_front::def_collector::NodeId::OpaqueTy(t))];
        let me = self.lower_opaque_ty(prog, opaque_def);
        if self.opaques.contains_key(&opaque_def) {
            return me;
        }
        self.opaques.insert(opaque_def, OpaqueDecl::default());
        let f = &prog.files[file as usize];
        let mut decl = OpaqueDecl::default();
        decl.region_contract = self.region_contract(cx,prog,opaque_def,env);
        if !bs.iter().any(|b| matches!(b, ast::Bound::Trait { maybe: true, .. })) {
            if let Some(sized) = self.lang_items.get(LangItem::Sized) { decl.bounds.push((sized, Vec::new())); }
        }
        for b in bs {
            let path = match b {
                ast::Bound::Trait { path, maybe: false, .. } => path,
                // (a relaxed bound names a trait too: unknown ones are E0405)
                ast::Bound::Trait { path, maybe: true, .. } => { if self.validates(prog, file) { self.resolve_trait_path(cx, prog, file, module, env, path); } continue; }
                _ => continue,
            };
            let td = match self.resolve_trait_path(cx, prog, file, module, env, path) {
                Some(d) if prog.def(d).kind == DefKind::Trait => d,
                _ => continue,
            };
            let last = f.ast.segs(path.segs).last();
            if let Some(GenericArgs::Paren(ps, r)) = last.and_then(|l| f.ast.gargs(l.args())) {
                let pts: Vec<TyId> = f.ast.list(*ps).iter().map(|x| self.lower_ty(cx, prog, file, module, env, *x)).collect();
                // the output is lowered for its diagnostics and nested opaques; the
                // declaration's clauses come from native collection (item_bounds)
                if let Some(r) = r { self.lower_ty(cx, prog, file, module, env, *r); }
                let args = vec![self.tys.intern(TyKind::Tuple(self.tys.mk_list(&pts)))];
                decl.bounds.push((td, args));
            } else {
                let explicit = self.lower_generic_args(cx, prog, file, module, env, path);
                let targs = self.fill_trait_defaults(cx, prog, td, me, explicit);
                if let Some(GenericArgs::Angle(v)) = last.and_then(|l| f.ast.gargs(l.args())) {
                    for g in f.ast.list(*v) {
                        if let GenericArg::Binding(n, _, ty) = g {
                            let sym = prog.syms.get(f.text(*n)).unwrap_or(u32::MAX);
                            if self.trait_assoc(prog, td, sym).is_some() {
                                self.lower_ty(cx, prog, file, module, env, *ty);
                            }
                        }
                    }
                }
                decl.bounds.push((td, targs));
            }
        }
        self.opaques.insert(opaque_def, decl);
        me
    }

    /// core::future::Future and its Output associated type.
    pub fn future_defs(&self, prog: &Program) -> Option<(DefId, DefId)> {
        if let Some(x) = self.future_defs.get() {
            return *x;
        }
        let mut r = None;
        for di in 0..prog.defs.len() {
            let d = DefId(di as u32);
            if prog.def(d).kind == DefKind::Trait && prog.def_path(d) == "core::future::Future" {
                let sym = prog.syms.get("Output").unwrap_or(u32::MAX);
                if let Some(ad) = self.trait_assoc(prog, d, sym) {
                    r = Some((d, ad));
                }
                break;
            }
        }
        let _ = self.future_defs.set(r);
        r
    }

    /// The opaque `impl Future<Output = out>` an async fn `d` returns.
    fn async_opaque(&self, prog: &Program, d: DefId, out: TyId) -> TyId {
        let opaque_def = prog.node_id_to_def_id[&(prog.def(d).file, relax_front::def_collector::NodeId::AsyncReturn(prog.def(d).item))];
        let me = self.lower_opaque_ty(prog, opaque_def);
        let TyKind::Alias(_, alias) = self.tys.kind(me) else { unreachable!() };
        let args = self.complete_args(prog, opaque_def, self.tys.list(alias.args), alias.regions);
        self.async_fns.insert(d, self.instantiate_ty(prog, opaque_def, &args, out));
        if let Some((fd, _)) = self.future_defs(prog) {
            let mut decl = OpaqueDecl::default();
            decl.bounds.push((fd, Vec::new()));
            if let Some(sized) = self.lang_items.get(LangItem::Sized) { decl.bounds.push((sized, Vec::new())); }
            self.opaques.insert(opaque_def, decl);
        }
        me
    }

    /// A coroutine (async block / async fn body) closure type.
    pub fn is_coroutine(&self, t: TyId) -> bool {
        matches!(self.tys.kind(t), TyKind::Closure(f, e, ..) if self.coroutines.contains(&(*f, *e)))
    }

    /// Does `t` mention a generic parameter?
    pub fn has_param(&self, t: TyId) -> bool {
        // rustc_type_ir/src/visit.rs: has_non_region_param includes rigid
        // aliases and const arguments just like every other generic type.
        self.tys.flags(t).contains(crate::types::TypeFlags::HAS_TY_PARAM)
    }

    /// Built-in impls of Copy / Clone for primitive and pointer-like types (rustc
    /// assemble_builtin_bound_candidates; core has no impl items for them in every case).
    /// `dyn Tr` implements Tr and its supertraits (rustc object candidates; the vtable has them).
    pub fn dyn_implements(&self, td: DefId, t: TyId) -> bool {
        match self.tys.kind(t) {
            TyKind::Dyn(p, _, _, _, _, _, autos) => self.tys.list(*autos).contains(&td) || self.trait_closure.get(p).map_or(*p == td, |v| v.contains(&td)),
            _ => false,
        }
    }

    /// `dyn Principal<..> + AutoTraits + 'a`
    fn lower_dyn(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, bounds: &[ast::Bound], lo: u32, source_ty: Option<ast::TyId>) -> TyId {
        let f = &prog.files[file as usize];
        let mut autos = Vec::new();
        for bound in bounds {
            if let ast::Bound::Trait { path, .. } = bound {
                if let Some(td) = self.resolve_type_path_def(prog, file, module, path) {
                    let d = prog.def(td);
                    if { let a = &prog.files[d.file as usize].ast; a.as_trait(&a.item(d.item).kind).is_some_and(|t| t.auto) } {
                        autos.push(td);
                    }
                }
            }
        }
        autos.sort_by_key(|d| d.0);
        autos.dedup();
        // hir_ty_lowering/dyn_trait.rs: at most one non-auto trait (E0225)
        let mut principals = 0;
        for b in bounds {
            if let ast::Bound::Trait { path, maybe: true, .. } = b {
                if self.validates(prog, file) {
                    self.resolve_trait_path(cx, prog, file, module, env, path);
                    self.err(cx, prog, file, path.lo, format!("{}relaxed bounds are not permitted in trait object types", relax_front::diagnostics::UNCODED_PREFIX));
                }
            }
            if let ast::Bound::Trait { path, maybe: false, .. } = b {
                let Some(td) = self.resolve_trait_path(cx, prog, file, module, env, path) else { continue };
                if autos.contains(&td) { continue; }
                principals += 1;
                if principals == 2 {
                    self.err(cx, prog, file, path.lo, "E0225: only auto traits can be used as additional traits in a trait object".to_string());
                    return self.tys.error;
                }
            }
        }
        for b in bounds {
            let (path, hrtb) = match b {
                ast::Bound::Trait { path, hrtb, .. } => (path, *hrtb),
                _ => continue,
            };
            let td = match self.resolve_type_path_def(prog, file, module, path) {
                Some(d) if prog.def(d).kind == DefKind::Trait => d,
                _ => continue,
            };
            if autos.contains(&td) { continue; }
            let outer_env = env;
            let mut predicate_env = env.clone();
            predicate_env.regions = env.regions.with_binder(Some(resolve_bound_vars::BinderId::Trait(path.segs.lo)));
            let env = &predicate_env;
            let regions = self.lower_region_args(cx.infcx, prog, file, module, env, path);
            let mut ambiguous = false;
            let region = self.object_region_with_bounds(cx.infcx, prog, file, outer_env, bounds, td, regions, source_ty, &mut ambiguous);
            if ambiguous && self.validates(prog, file) {
                self.err(cx, prog, file, lo, "E0227: ambiguous lifetime bound, explicit lifetime bound required".to_string());
            }
            self.report_indeterminate_object_default(cx, prog, file, outer_env, region, source_ty, lo);
            let last = f.ast.segs(path.segs).last().unwrap();
            if !matches!(f.ast.gargs(last.args()), Some(GenericArgs::Paren(..))) {
                let errs = if !self.validates(prog, file) { Vec::new() } else { self.check_generic_arg_count(prog, td, f.ast.gargs(last.args()), &f.ast, self.own_generic_counts(prog, td, None), arg_count::ArgPosition::Type, false, false) };
                if !errs.is_empty() {
                    for e in errs { self.err(cx, prog, file, last.name.lo, e); }
                    return self.tys.error;
                }
            }
            let mut args = Vec::new();
            let mut binds = Vec::new();
            let mut predicate_bound = env.regions.bound_count();
            match f.ast.gargs(last.args()) {
                Some(GenericArgs::Paren(ps, r)) => {
                    let sig = ast::FnPtr { hrtb, unsafe_: false, abi: None, params: *ps, ret: *r, variadic: false };
                    // rustc_ast_lowering/src/path.rs:517-573 and
                    // rustc_hir_analysis/src/hir_ty_lowering/dyn_trait.rs:348-419
                    // @8d1a764: Args and Output retain the trait's binder.
                    let sig = self.lower_fn_pointer(cx, prog, file, module, outer_env, &sig, resolve_bound_vars::BinderId::Trait(path.segs.lo));
                    let TyKind::FnPtr(ps, ret, _, bound) = self.tys.kind(sig) else { unreachable!() };
                    predicate_bound = *bound;
                    args.push(self.tys.intern(TyKind::Tuple(ps.clone())));
                    if let Some(output) = self.callable_output(td) { binds.push((output, *ret, 0)); }
                }
                Some(GenericArgs::Angle(v)) => {
                    for g in f.ast.list(*v) {
                        match g {
                            GenericArg::Type(t) => {
                                let arg = self.type_or_const_arg(cx, prog, file, module, env, *t);
                                let expected_const = self.declaration_env(prog, td).consts.iter().any(|c| c.0 as usize == args.len() + 1);
                                args.push(self.arg_of_param_kind(cx, prog, file, path.lo, expected_const, arg));
                            },
                            GenericArg::Const(e) => {
                                let declaration = self.declaration_env(prog, td);
                                // (the type of a const parameter naming its own trait: E0391)
                                let cycle = crate::cycle::enter(crate::cycle::Kind::ConstParamType, td.0, args.len() as u32 + 1);
                                if cycle.is_err() {
                                    self.err(cx, prog, file, lo, format!("E0391: cycle detected when computing type of the const parameter {} of `{}`", args.len() + 1, prog.def_path(td)));
                                    args.push(self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))));
                                    continue;
                                }
                                let expected = declaration.consts.iter().find(|c| c.0 as usize == args.len() + 1)
                                    .map(|&(_, cf, ct)| self.lower_ty(cx, prog, cf, prog.def(td).scope, &declaration, ct))
                                    .unwrap_or(self.tys.error);
                                drop(cycle);
                                args.push(self.tys.intern(TyKind::Const(self.lower_const_arg(prog, file, env, *e, expected))));
                            },
                            GenericArg::Binding(n, bargs, t) => {
                                let sym = prog.syms.get(f.text(*n)).unwrap_or(u32::MAX);
                                if let Some(ad) = self.trait_assoc(prog, td, sym) {
                                    // the binding's own arguments against the associated type's (`Y<1> = ..`)
                                    if self.validates(prog, file) {
                                        let errs = self.check_generic_arg_count(prog, ad, f.ast.gargs(*bargs), &f.ast, self.own_generic_counts(prog, ad, None), arg_count::ArgPosition::AssocType, false, false);
                                        if !errs.is_empty() || self.prohibit_constraints(cx, prog, file, *bargs) {
                                            for e in errs { self.err(cx, prog, file, n.lo, e); }
                                            return self.tys.error;
                                        }
                                    }
                                    // lower_assoc_item_constraint: an object type binds each associated item once (E0719)
                                    if binds.iter().any(|b| b.0 == ad) && self.validates(prog, file) {
                                        self.err(cx, prog, file, n.lo, format!("E0719: the value of the associated type `{}` in trait `{}` is already specified", f.text(*n), prog.def_path(prog.def(ad).parent)));
                                    }
                                    let bt = self.lower_ty(cx, prog, file, module, env, *t);
                                    binds.push((ad, bt, 0));
                                } else {
                                    self.err(cx, prog, file, n.lo, format!("E0220: associated type `{}` not found for `{}`", f.text(*n), prog.name(td)));
                                    return self.tys.error;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                None => {}
            }
            let bound = predicate_bound;
            for b in &mut binds { b.2 = bound; }
            binds.sort_by_key(|b| b.0 .0);
            // rustc dyn_trait.rs: the value of every associated type must be given, in the
            // object type or by a supertrait bound
            let missing = self.missing_dyn_assoc_types(prog, td, &binds);
            if !missing.is_empty() {
                let names: Vec<String> = missing.iter().map(|&m| format!("`{}`", prog.name(m))).collect();
                let list = match names.len() { 1 => names[0].clone(), n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]) };
                self.err(cx, prog, file, lo, format!("E0191: the value of the associated type{} {} in `{}` must be specified", if names.len() > 1 { "s" } else { "" }, list, prog.name(td)));
                return self.tys.error;
            }
            // omitted trait args take their defaults (`dyn Trait` with `trait Trait<const N: u8 =
            // 12>`); a default naming Self cannot be used in an object type, left as written
            let n = self.trait_generic_count(prog, td);
            if args.len() < n && !matches!(prog.name(td), "Fn" | "FnMut" | "FnOnce" | "AsyncFn" | "AsyncFnMut" | "AsyncFnOnce") {
                let filled = self.fill_trait_defaults(cx, prog, td, self.tys.error, args.clone());
                if filled.len() == n && filled[args.len()..].iter().all(|t| !self.tys.has_error(*t)) {
                    args = filled;
                }
            }
            // an argument list of the wrong length is E0107 / E0393; the object type still
            // carries one argument per declared parameter (error for the missing, none extra)
            // (a missing const argument is a const error, not a type: the object type's arguments keep
            // their parameters' kinds)
            let declaration = self.declaration_env(prog, td);
            while args.len() < n {
                let i = args.len() + 1;
                args.push(if declaration.consts.iter().any(|c| c.0 as usize == i) { self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))) } else { self.tys.error });
            }
            let bound = u16::try_from(bound).expect("dyn principal bound-region count");
            return self.tys.intern(TyKind::Dyn(td, self.tys.mk_list(&args), self.tys.mk_list(&binds), regions, region, bound, self.tys.mk_list(&autos)));
        }
        // only auto traits (`dyn Send`, `dyn Send + Sync`): a trait object without a principal
        // (rustc: its vtable is drop, size and align alone); the first auto trait stands for it
        if let Some(&td) = autos.first() {
            let region = self.object_region(cx.infcx, prog, file, env, bounds, source_ty);
            self.report_indeterminate_object_default(cx, prog, file, env, region, source_ty, lo);
            return self.tys.intern(TyKind::Dyn(td, crate::types::List::EMPTY, crate::types::List::EMPTY, RegionArgsId::EMPTY, region, 0, self.tys.mk_list(&autos)));
        }
        self.err(cx, prog, file, lo, "trait object without a principal trait".to_string());
        self.tys.error
    }

    /// rustc_hir_analysis/src/collect.rs:506-520 (ItemCtxt::re_infer): outside bodies an
    /// object type whose lifetime default is indeterminate (resolve_bound_vars.rs:927-944)
    /// and which has no bound of its own is E0228; bodies infer it.
    fn report_indeterminate_object_default(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, env: &GenEnv, region: RegionId, source_ty: Option<ast::TyId>, lo: u32) {
        if cx.infcx.is_none() && self.tys.region_kind(region) == RegionKind::ReError && env.regions.object_default_indeterminate(source_ty) {
            self.err(cx, prog, file, lo, "E0228: cannot deduce the lifetime bound for this trait object type from context".into());
        }
    }

    pub fn lower_path_ty(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path, source_ty: Option<ast::TyId>) -> TyId {
        let f = &prog.files[file as usize];
        // `<T as Trait>::Name`
        if let Some(q) = prog.files[file as usize].ast.qself(p) {
            // rustc_ast_lowering path.rs:41,88: `impl Trait` in a qualified path's self type or
            // trait path is ImplTraitPosition::Path (E0562)
            let self_t = if env.opaque_owner.is_some() || !env.impl_params.is_empty() {
                let mut path_env = env.clone();
                path_env.opaque_owner = None; path_env.impl_params.clear();
                self.lower_ty(cx, prog, file, module, &path_env, q.ty)
            } else { self.lower_ty(cx, prog, file, module, env, q.ty) };
            if let Some(tp) = &q.trait_path {
                if let Some(td) = self.resolve_type_path_def(prog, file, module, tp) {
                    let name = prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[p.segs.len() - 1].name)).unwrap_or(u32::MAX);
                    if let Some(ad) = self.trait_assoc(prog, td, name) {
                        if prog.def(ad).kind == DefKind::AssocTy && !self.check_assoc_ty_args(cx, prog, file, ad, p, source_ty.is_none()) { return self.tys.error; }
                        let tr = self.lower_region_args(cx.infcx, prog, file, module, env, tp);
                        let explicit = self.lower_generic_args(cx, prog, file, module, env, tp);
                        let mut args = vec![self_t];
                        args.extend(self.fill_trait_defaults(cx, prog, td, self_t, explicit));
                        let tr = if prog.def(ad).parent == td { tr } else { RegionArgsId::EMPTY };
                        let mut own_regions = self.written_lifetimes(cx.infcx, prog, file, env, p);
                        let mut own_args = self.lower_generic_args_of_item(cx, prog, file, module, env, p, Some(ad));
                        self.fit_assoc_own_args(prog, ad, &mut own_args, &mut own_regions);
                        args.extend(own_args);
                        let mut rs = self.tys.region_args(tr).to_vec();
                        rs.extend(own_regions);
                        return self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: self.tys.mk_list(&args), regions: self.tys.intern_region_args(&rs) }));
                    }
                }
            }
            // `<T>::Name`: the same lookup as `T::Name`
            if q.trait_path.is_none() {
                let ast = &prog.files[file as usize].ast;
                let name = prog.syms.get(f.text(ast.segs(p.segs)[p.segs.len() - 1].name)).unwrap_or(u32::MAX);
                if let TyKind::Param(pi) = self.tys.kind(self_t).clone() {
                    let in_trait_self = pi == 0 && env.trait_def.is_some() && env.names.first().is_some_and(|n| prog.syms.str(*n) == "Self");
                    let found = if in_trait_self {
                        let td = env.trait_def.unwrap();
                        let trait_ref = crate::next_solver::TraitRef { def_id: td, args: self.generic_indices_of(prog, td).identity_args(&self.tys) };
                        self.assoc_projection(prog, trait_ref, name)
                    } else { self.param_assoc(cx, prog, module, env, pi, name) };
                    if let Some(t) = found {
                        return self.lower_generic_args_of_assoc_item(cx, prog, file, module, env, p, t, source_ty);
                    }
                    self.err(cx, prog, file, p.lo, format!("E0220: associated type `{}` not found for `{}`", prog.syms.str(name), crate::typeck::ty_to_string(false, prog, self, self_t)));
                    return self.tys.error;
                }
                // `<Self>::Name` in an impl
                if let (Some(imp), true) = (env.impl_def, env.self_ty == Some(self_t)) {
                    if self.impl_trait.contains_key(&imp) {
                        let args = self.generic_indices_of(prog, imp).identity_args(&self.tys);
                        let trait_ref = self.impl_trait_ref(prog, imp, &args);
                        if let Some(t) = self.assoc_projection(prog, trait_ref, name) {
                            return self.lower_generic_args_of_assoc_item(cx, prog, file, module, env, p, t, source_ty);
                        }
                    } else if let Some(t) = self.impl_assoc_ty(cx, prog, imp, name) { return t; }
                }
                if !self.tys.has_error(self_t) {
                    self.err(cx, prog, file, p.lo, "E0223: ambiguous associated type".to_string());
                }
                return self.tys.error;
            }
            self.err(cx, prog, file, p.lo, "unsupported qualified type path".to_string());
            return self.tys.error;
        }
        // `Self::Name`: inside an impl the impl's own associated type, inside a trait a projection
        if p.segs.len() == 2 && prog.files[file as usize].ast.segs(p.segs)[0].kind() == SegKind::SelfType {

            let name = prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[1].name)).unwrap_or(u32::MAX);
            if let Some(imp) = env.impl_def {
                if self.impl_trait.contains_key(&imp) && env.self_ty.is_some() {
                    let args = self.generic_indices_of(prog, imp).identity_args(&self.tys);
                    let trait_ref = self.impl_trait_ref(prog, imp, &args);
                    if let Some(t) = self.assoc_projection(prog, trait_ref, name) {
                        // an enum variant of the same name: the variant has priority (ambiguous_associated_items)
                        if let Some(TyKind::Adt(a, ..)) = env.self_ty.map(|t| self.tys.kind(t).clone()) {
                            if self.validates(prog, file) && self.adts.get(&a).is_some_and(|adt| adt.is_enum() && adt.variants.iter().any(|v| v.name == name)) {
                                crate::lint::emit(prog, imp, &crate::lint::AMBIGUOUS_ASSOCIATED_ITEMS, file, p.lo, "ambiguous associated item");
                            }
                        }
                        return self.lower_generic_args_of_assoc_item(cx, prog, file, module, env, p, t, source_ty);
                    }
                } else if let Some(t) = self.impl_assoc_ty(cx, prog, imp, name) { return t; }
            }
            if let Some(td) = env.trait_def {
                let trait_ref = crate::next_solver::TraitRef { def_id: td, args: self.generic_indices_of(prog, td).identity_args(&self.tys) };
                // `Self::Name` in a trait: the trait and its supertraits that declare `Name` (more than one: E0221)
                if self.validates(prog, file) {
                    let predicate = crate::next_solver::Predicate::dummy(crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(
                        crate::next_solver::TraitClause { trait_ref: trait_ref.clone(), polarity: crate::next_solver::ClausePolarity::Positive })));
                    let mut declaring = self.transitive_bounds_that_define_assoc_item(prog, vec![predicate], name);
                    // (the same trait ref reached under different binders is one candidate: anonymize_bound_vars)
                    let mut kinds: Vec<crate::next_solver::PredicateKind> = Vec::new();
                    declaring.retain(|(_, p)| { if kinds.contains(&p.kind) { false } else { kinds.push(p.kind.clone()); true } });
                    if declaring.len() > 1 {
                        let traits: Vec<&str> = declaring.iter().map(|(item, _)| prog.name(prog.def(*item).parent)).collect();
                        self.err(cx, prog, file, p.lo, format!("E0221: ambiguous associated type `{}` in bounds of `Self` (from {})", prog.syms.str(name), traits.join(", ")));
                        return self.tys.error;
                    }
                }
                if let Some(t) = self.assoc_projection(prog, trait_ref, name) {
                    return self.lower_generic_args_of_assoc_item(cx, prog, file, module, env, p, t, source_ty);
                }
                // an associated type of a bound the item's where clauses put on `Self`
                // (`where Self: TryInto<T>` makes `Self::Error`; Self is param 0 of a trait)
                if let Some(t) = self.param_assoc(cx, prog, module, env, 0, name) {
                    return self.lower_generic_args_of_assoc_item(cx, prog, file, module, env, p, t, source_ty);
                }
            }
        }
        // `T::Name` on a generic param: the associated type of one of its bounds
        if p.no_qself() && p.segs.len() == 2 && prog.files[file as usize].ast.segs(p.segs)[0].kind() == SegKind::Ident && prog.files[file as usize].ast.segs(p.segs)[0].args().is_none() {
            let pn = prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[0].name)).unwrap_or(u32::MAX - 1);
            if let Some(pi) = env.names.iter().rposition(|n| *n == pn) {
                let name = prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[1].name)).unwrap_or(u32::MAX);
                if let Some(t) = self.param_assoc(cx, prog, module, env, pi as u32, name) {
                    return self.lower_generic_args_of_assoc_item(cx, prog, file, module, env, p, t, source_ty);
                }
                self.err(cx, prog, file, p.lo, format!("E0220: associated type `{}` not found for `{}`", f.text(prog.files[file as usize].ast.segs(p.segs)[1].name), f.text(prog.files[file as usize].ast.segs(p.segs)[0].name)));
                return self.tys.error;
            }
        }
        if p.no_qself() && p.segs.len() == 1 {
            let seg = &prog.files[file as usize].ast.segs(p.segs)[0];
            if seg.kind() == SegKind::SelfType {
                if source_ty.is_some() && self.prohibit_args_on(cx, prog, file, seg, "self type", "Self") { return self.tys.error; }
                if env.self_ty.is_none() && self.validates(prog, file) {
                    self.err(cx, prog, file, p.lo, "E0411: cannot find type `Self` in this scope".to_string());
                }
                return env.self_ty.unwrap_or(self.tys.error);
            }
            let name = f.text(seg.name);
            if let Some(s) = prog.syms.get(name) {
                // an item of the owner's body blocks shadows its type parameters (late.rs ribs)
                let shadowed = env.owner.is_some_and(|o| prog.lookup_in_body_blocks(module, o, s, true).is_some());
                for (i, n) in env.names.iter().enumerate().filter(|_| !shadowed) {
                    // a const parameter lives in the value namespace: a type path never names it
                    if *n == s && !env.consts.iter().any(|c| c.0 as usize == i) {
                        // a type parameter takes no generic arguments (E0109), no constraints (E0229)
                        if source_ty.is_some() && self.prohibit_args_on(cx, prog, file, seg, "type parameter", name) { return self.tys.error; }
                        return self.tys.intern(TyKind::Param(i as u32));
                    }
                }
            }
        }
        let relaxed = || -> Option<DefId> {
            if p.segs.len() != 1 || p.has_qself() || p.global() {
                return None;
            }
            let s = prog.syms.get(prog.files[file as usize].text(prog.files[file as usize].ast.segs(p.segs)[0].name))?;
            prog.relaxed_import(module, s, true, file, p.lo)
        };
        let mark = crate::program::missing_mark();
        let d = match self.resolve_type_path_def(prog, file, module, p).or_else(relaxed) {
            Some(d) => d,
            None => {
                // (lazy loading) the skipped files this lookup missed in may define it
                crate::program::missing_confirm(mark);
                // a single name that exists elsewhere in the program: where to import it from
                let help = if p.segs.len() == 1 && p.no_qself() {
                    let f = &prog.files[file as usize];
                    match prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[0].name)) {
                        Some(s) => crate::typeck::import_help(prog, module, s, true),
                        None => String::new(),
                    }
                } else {
                    String::new()
                };
                self.err(cx, prog, file, p.lo, format!("E0412: cannot find type `{}` in this scope{}", path_str(prog, file, p), help));
                return self.tys.error;
            }
        };
        if self.validates(prog, file) && p.segs.len() == 1 && p.no_qself() && !p.global() {
            if let Some(s) = prog.syms.get(prog.files[file as usize].text(prog.files[file as usize].ast.segs(p.segs)[0].name)) {
                if let Some(e) = prog.name_ambiguity(module, s, true, false) {
                    if let Some(m) = crate::lint::privacy_diag(prog, file, p.lo, &e) { self.err(cx, prog, file, p.lo, m); }
                }
            }
        }
        if self.validates(prog, file) && p.segs.len() >= 2 && p.no_qself() {
            let segs = crate::program::path_segs(prog, file, p);
            if let Some(e) = prog.path_privacy(module, p.global(), &segs, Some(true), false, false).filter(|_| p.lo < prog.files[file as usize].src_len) {
                if let Some(m) = crate::lint::privacy_diag(prog, file, p.lo, &e) { self.err(cx, prog, file, p.lo, m); }
            }
        }
        if source_ty.is_some() && self.prohibit_module_args(cx, prog, file, module, p) { return self.tys.error; }
        let regions = self.lower_region_args(cx.infcx, prog, file, module, env, p);
        let mut args = self.lower_generic_args(cx, prog, file, module, env, p);
        let def = prog.def(d);
        if source_ty.is_some() && matches!(def.kind, DefKind::Prim(_)) && p.segs.len() == 1 && p.no_qself() {
            let seg = &prog.files[file as usize].ast.segs(p.segs)[0];
            if self.prohibit_args_on(cx, prog, file, seg, "builtin type", f.text(seg.name)) { return self.tys.error; }
        }
        // a module named like a primitive (`use std::str;`, globs re-exporting std's modules)
        // does not hide the primitive type
        if def.kind == DefKind::Mod && p.segs.len() == 1 && p.no_qself() {
            let name = f.text(prog.files[file as usize].ast.segs(p.segs)[0].name);
            for (pn, pr) in crate::program::PRIMS {
                if pn == name {
                    return self.tys.prim(pr);
                }
            }
        }
        // hir_ty_lowering check_generic_arg_count (a written type; an expression path's prefix
        // infers what is left out): the error type for a wrong number of arguments
        if source_ty.is_some() && matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias) {
            if let Some(seg) = prog.files[file as usize].ast.segs(p.segs).last() {
                let ast = &prog.files[file as usize].ast;
                let errs = if !self.validates(prog, file) { Vec::new() } else { self.check_generic_arg_count(prog, d, ast.gargs(seg.args()), ast, self.own_generic_counts(prog, d, None), arg_count::ArgPosition::Type, false, false) };
                if !errs.is_empty() {
                    for e in errs { self.err(cx, prog, file, seg.name.lo, e); }
                    return self.tys.error;
                }
                if self.prohibit_constraints(cx, prog, file, seg.args()) { return self.tys.error; }
            }
        }
        match def.kind {
            DefKind::Prim(pr) => self.tys.prim(pr),
            DefKind::ForeignTy => self.tys.intern(TyKind::Foreign(d)),
            DefKind::Struct | DefKind::Enum | DefKind::Union => {
                // `HashMap<K, V>` with `S = RandomState` defaulted
                // (a missing parameter is E0107 in check_generic_arg_count; the argument is the error type)
                let _ = self.fill_type_defaults(cx, prog, d, &mut args, env.regions.enabled.then_some(regions));
                self.tys.intern(TyKind::Adt(d, self.tys.mk_list(&args), regions))
            }
            DefKind::TypeAlias => {
                let a = self.alias_ty(cx, prog, d);
                // (a missing parameter is E0107 in check_generic_arg_count; the argument is the error type)
                let _ = self.fill_type_defaults(cx, prog, d, &mut args, env.regions.enabled.then_some(regions));
                let rs = regions;
                let aenv = self.item_env(prog, d);
                let a = self.subst_decl_regions(a, &aenv, rs);
                let a = self.tys.subst(a, &args);
                if env.regions.enabled { a } else { self.tys.erase_all_regions(a) }
            }
            // a bare trait in a type (`&mut FnMut(&mut [u8])`): edition 2015's trait object
            // (rustc: `dyn` is implied there; later editions reject it, E0782)
            DefKind::Trait if prog.crates[prog.mods[module as usize].krate as usize].edition <= 2018 => {
                if crate::lint::enabled() && self.validates(prog, file) {
                    let at = crate::lint::def_at(prog, file, p.lo);
                    crate::lint::emit(prog, at, &crate::lint_early::BARE_TRAIT_OBJECTS, file, p.lo, "trait objects without an explicit `dyn` are deprecated");
                }
                let b = ast::Bound::Trait { hrtb: ast::List::EMPTY, maybe: false, konst: false, async_: false, path: p.clone() };
                self.lower_dyn(cx, prog, file, module, env, &[b], p.lo, source_ty)
            }
            _ => {
                {
                // rustc: a trait in type position without `dyn` (2021+) is E0782; any other
                // non-type item is E0573 "expected type, found <kind> `X`"
                let msg = match prog.def(d).kind {
                    DefKind::Trait => "E0782: expected a type, found a trait".to_string(),
                    k => format!("E0573: expected type, found {} `{}`", def_kind_descr(k), path_str(prog, file, p)),
                };
                self.err(cx, prog, file, p.lo, msg);
            }
                self.tys.error
            }
        }
    }

    /// Appends the defaults of type params left out of `args` (`struct HashMap<K, V, S =
    /// RandomState>` written `HashMap<u32, u64>`). Only when some args were given: `HashMap`
    /// alone in an expression path gets inference variables instead.
    pub fn fill_type_defaults(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId, args: &mut Vec<TyId>, regions: Option<RegionArgsId>) -> bool {
        let mut missing = false;
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let g = match &ast.item(def.item).kind {
            // `type Result<T, E = Error> = ..` written `Result<()>` (rustc: alias params default
            // like any other generic params)
            k @ (ItemKind::Struct(_) | ItemKind::Enum(_) | ItemKind::Union(_) | ItemKind::TypeAlias(_)) => *ast.kind_generics(k).unwrap(),
            _ => return false,
        };
        let params: Vec<&ast::GenericParam> = ast.list(g.params).iter().filter(|p| !matches!(p.kind, GenericParamKind::Lifetime(_))).collect();
        if args.len() >= params.len() {
            return false;
        }
        // no args at all: in a type, `Foo` with `struct Foo<A = (isize, char)>` is
        // `Foo<(isize, char)>` when every param has a default (else the args are inferred)
        // (item signatures have nothing to infer from: a missing list is E0107 there, error arguments)
        if args.is_empty() && cx.infcx.is_some() && !params.iter().all(|p| matches!(p.kind, GenericParamKind::Type(_, Some(_)) | GenericParamKind::Const(_, Some(_)))) {
            return false;
        }
        let mut env = self.item_env(prog, d);
        env.regions.enabled = regions.is_some();
        let module = self.item_module(prog, d);
        for i in args.len()..params.len() {
            if cx.state.default_inprog.borrow().contains(&(d, i)) {
                self.err(cx, prog, def.file, params[i].name.lo, format!("E0391: cycle detected when computing the default of generic parameter `{}`", prog.def_path(d)));
                args.push(self.tys.error);
                continue;
            }
            cx.state.default_inprog.borrow_mut().push((d, i));
            let t = match &params[i].kind {
                GenericParamKind::Type(_, Some(dt)) => {
                    let lt = self.lower_ty(cx, prog, def.file, module, &env, *dt);
                    let lt = regions.map_or(lt, |rs| self.subst_decl_regions(lt, &env, rs));
                    self.tys.subst(lt, args)
                }
                // a const param's default (`const M: usize = N`): its value, or an earlier param
                GenericParamKind::Const(ty, Some(de)) => {
                    let expected = self.lower_ty(cx, prog, def.file, module, &env, *ty);
                    let lt = self.tys.intern(TyKind::Const(self.lower_const_arg(prog, def.file, &env, *de, expected)));
                    self.tys.subst(lt, args)
                }
                _ => { missing = true; self.tys.error }
            };
            cx.state.default_inprog.borrow_mut().pop();
            args.push(t);
        }
        missing
    }

    /// `<Param(pi) as Bound>::name` for the unique bound of param `pi` (or a supertrait)
    /// that declares the associated type `name`.
    /// hir_ty_lowering/mod.rs:1247-1275,1351-1397 (probe_single_ty_param_bound_for_assoc_item ->
    /// probe_single_bound_for_assoc_item): every bound of the parameter and their supertraits
    /// (rustc_infer/src/traits/util.rs:111-140, deduplicated) that define the item; more than one
    /// is E0221 (supertrait_item_shadowing, which could pick the subtrait, is a cut feature).
    fn param_assoc(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, _module: u32, env: &GenEnv, pi: u32, name: Sym) -> Option<TyId> {
        let owner = env.regions.owner.expect("type parameter bounds have an item owner");
        let predicates = self.type_param_clauses(prog, owner, pi, name);
        let candidates = self.transitive_bounds_that_define_assoc_item(prog, predicates, name);
        match candidates.as_slice() {
            [] => None,
            [(item, predicate)] => {
                // FnCtxt::lower_assoc_item_path (fn_ctxt/mod.rs:384-407)
                // instantiates the binder; ItemCtxt (collect.rs:627) rejects it.
                let kind = if env.regions.infer_fresh {
                    cx.infcx.expect("body type lowering has inference").instantiate_predicate_binder_with_infer(predicate)
                } else if predicate.has_bound_vars(&self.tys) {
                    self.err(cx, prog, prog.def(owner).file, prog.files[prog.def(owner).file as usize].ast.item(prog.def(owner).item).lo,
                        "E0212: cannot use the associated type of a trait with uninferred generic parameters".into());
                    return Some(self.tys.error);
                } else { predicate.kind.clone() };
                let crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(tr)) = kind else { unreachable!() };
                Some(self.alias_ty_with_args(AliasTyKind::Projection { def_id: *item }, &tr.trait_ref.args))
            },
            [..] => {
                let def = prog.def(owner);
                let (file, lo) = (def.file, prog.files[def.file as usize].ast.item(def.item).lo);
                let traits: Vec<&str> = candidates.iter().map(|(_, p)| {
                    let crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(tr)) = &p.kind else { unreachable!() };
                    prog.name(tr.trait_ref.def_id)
                }).collect();
                self.err(cx, prog, file, lo, format!("E0221: ambiguous associated type `{}` in bounds of the type parameter (from {})", prog.syms.str(name), traits.join(", ")));
                Some(self.tys.error)
            }
        }
    }

    /// rustc_infer/src/traits/util.rs:111-140 filtered by probe_trait_that_defines_assoc_item:
    /// the deduplicated trait refs (bounds and their positive supertraits) defining `name`.
    fn transitive_bounds_that_define_assoc_item(&self, prog: &Program, mut stack: Vec<crate::next_solver::Predicate>, name: Sym) -> Vec<(DefId, crate::next_solver::Predicate)> {
        use crate::next_solver::{PredicateKind, ClauseKind, ClausePolarity};
        let mut seen = crate::fxhash::HashSet::default();
        stack.reverse();
        let mut result = Vec::new();
        while let Some(predicate) = stack.pop() {
            let PredicateKind::Clause(ClauseKind::Trait(tr)) = &predicate.kind else { continue; };
            if !seen.insert(predicate.anonymized()) { continue; }
            for bound in self.explicit_super_predicates_of(prog, tr.trait_ref.def_id) {
                let instantiated = self.instantiate_supertrait(prog, &bound, &predicate);
                if matches!(&instantiated.kind, PredicateKind::Clause(ClauseKind::Trait(t)) if t.polarity == ClausePolarity::Positive) { stack.push(instantiated); }
            }
            for &item in &prog.traits[prog.def(tr.trait_ref.def_id).sub as usize] {
                let d = prog.def(item);
                if d.name == name && d.kind == DefKind::AssocTy { result.push((item, predicate.clone())); break; }
            }
        }
        result
    }

    /// The type a type alias is declared as (not cached).
    pub(crate) fn lower_alias_rhs(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId) -> TyId {
        let def = prog.def(d);
        let module = self.item_module(prog, d);
        let mut env = self.item_env(prog, d);
        env.regions.enabled = true;
        let ast = &prog.files[def.file as usize].ast;
        match ast.as_alias(&ast.item(def.item).kind) {
            Some(TypeAliasItem { ty: Some(t), .. }) => self.lower_ty(cx, prog, def.file, module, &env, *t),
            _ => self.tys.error,
        }
    }

    pub fn alias_ty(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, d: DefId) -> TyId {
        if let Some(t) = self.alias_tys.get(&(cx.typing_env, d)) {
            return t;
        }
        let ik = (cx.typing_env, d);
        if cx.state.alias_inprog.borrow().contains(&ik) {
            return self.tys.error;
        }
        cx.state.alias_inprog.borrow_mut().insert(ik.clone());
        let read0 = cx.state.impl_read.replace(false);
        let t = self.lower_alias_rhs(cx, prog, d);
        cx.state.alias_inprog.borrow_mut().remove(&ik);
        let read = cx.state.impl_read.get();
        cx.state.impl_read.set(read0 || read);
        if !(read && cx.state.par_impls.get()) {
            self.alias_tys.insert((cx.typing_env, d), t);
        }
        t
    }

    /// The (type/const, lifetime) parameters a path's last segment can name for the type-like item
    /// `d` it resolves to; `None` for items whose last segment is not this item's own arguments
    /// (variants, functions, modules ...).
    pub(crate) fn own_param_counts(&self, prog: &Program, d: DefId) -> Option<(usize, usize)> {
        let def = prog.def(d);
        if !matches!(def.kind, DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Trait | DefKind::AssocTy) { return None; }
        let ast = &prog.files[def.file as usize].ast;
        let generics = ast.kind_generics(&ast.item(def.item).kind)?;
        let lifetimes = ast.list(generics.params).iter().filter(|p| matches!(p.kind, GenericParamKind::Lifetime(_))).count();
        Some((ast.list(generics.params).len() - lifetimes, lifetimes))
    }

    /// An associated type path's own arguments are exactly the declared parameters of the
    /// associated type (hir_ty_lowering/generics.rs: missing ones are error terms, extra ones
    /// are dropped; E0107 reports either).
    pub(crate) fn fit_assoc_own_args(&self, prog: &Program, ad: DefId, args: &mut Vec<TyId>, regions: &mut Vec<RegionId>) {
        let def = prog.def(ad);
        let ast = &prog.files[def.file as usize].ast;
        let Some(generics) = ast.kind_generics(&ast.item(def.item).kind) else { return; };
        let params = ast.list(generics.params);
        let lifetimes = params.iter().filter(|p| matches!(p.kind, GenericParamKind::Lifetime(_))).count();
        regions.resize(lifetimes, self.tys.intern_region(RegionKind::ReError));
        let mut own = params.iter().filter(|p| !matches!(p.kind, GenericParamKind::Lifetime(_)));
        let mut fitted = Vec::new();
        for _ in 0..params.len() - lifetimes {
            let param = own.next().unwrap();
            fitted.push(match args.get(fitted.len()) {
                Some(&t) => t,
                None if matches!(param.kind, GenericParamKind::Const(..)) => self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))),
                None => self.tys.error,
            });
        }
        *args = fitted;
    }

    /// hir_ty_lowering/generics.rs provided_kind: a type written for a const parameter, or a
    /// constant for a type parameter, is E0747 and lowers to the error term of the parameter's
    /// kind.
    pub(crate) fn arg_of_param_kind(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, lo: u32, expected_const: bool, arg: TyId) -> TyId {
        let is_const = matches!(self.tys.kind(arg), TyKind::Const(_));
        if is_const == expected_const { return arg; }
        // a parenthesized `_` is an inference variable of the parameter's kind
        if expected_const && matches!(self.tys.kind(arg), TyKind::Infer(_)) {
            if let Some(infcx) = cx.infcx { return self.tys.intern(TyKind::Const(infcx.next_const_var())); }
        }
        let message = match (expected_const, self.tys.has_error(arg)) {
            (true, true) => "E0747: unresolved item provided when a constant was expected",
            (true, false) => "E0747: type provided when a constant was expected",
            (false, _) => "E0747: constant provided when a type was expected",
        };
        self.err(cx, prog, file, lo, message.into());
        if expected_const { self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))) } else { self.tys.error }
    }

    pub fn lower_generic_args(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path) -> Vec<TyId> {
        self.lower_generic_args_of_item(cx, prog, file, module, env, p, self.resolve_type_path_def(prog, file, module, p))
    }

    /// The compact arguments a declaration's own parameters follow: `Self` and the trait's
    /// parameters for a trait or one of its associated types.
    fn inherited_arg_count(&self, prog: &Program, d: DefId) -> usize {
        match prog.def(d).kind {
            DefKind::Trait => 1,
            DefKind::AssocTy => 1 + self.trait_generic_count(prog, prog.def(d).parent),
            _ => 0,
        }
    }

    /// `lower_generic_args` for a path whose last segment's arguments belong to `item` (a path
    /// with a qualified self type resolves to nothing by itself).
    pub(crate) fn lower_generic_args_of_item(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path, item: Option<DefId>) -> Vec<TyId> {
        let declaration = item.map(|d| (d, self.declaration_env(prog, d)));
        let mut out = Vec::new();
        let ast = &prog.files[file as usize].ast;
        if let Some(last) = ast.segs(p.segs).last() {
            if let Some(GenericArgs::Paren(ps, _)) = ast.gargs(last.args()) {
                let mut args = Vec::new();
                for t in ast.list(*ps) { args.push(self.lower_ty(cx, prog, file, module, env, *t)); }
                return vec![self.tys.intern(TyKind::Tuple(self.tys.mk_list(&args)))];
            }
            // check_generic_arg_count of a trait path (bounds, trait refs, `<T as Trait>::x`)
            if let Some((d, _)) = &declaration {
                if prog.def(*d).kind == DefKind::Trait {
                    let errs = if !self.validates(prog, file) { Vec::new() } else { self.check_generic_arg_count(prog, *d, ast.gargs(last.args()), ast, self.own_generic_counts(prog, *d, None), arg_count::ArgPosition::Type, false, false) };
                    if !errs.is_empty() {
                        for e in errs { self.err(cx, prog, file, last.name.lo, e); }
                        return vec![self.tys.error; self.trait_generic_count(prog, *d)];
                    }
                }
            }
            if let Some(GenericArgs::Angle(v)) = ast.gargs(last.args()) {
                // generics.rs generic_arg_mismatch_err: a type, placeholder or constant written before a lifetime
                // argument is provided where the lifetime parameter (they come first) was expected
                if self.validates(prog, file) {
                    let mut first_other: Option<&'static str> = None;
                    for g in ast.list(*v) {
                        match g {
                            GenericArg::Type(t) => {
                                let mut t = t;
                                while let ast::Ty::Paren(inner) = ast.ty(*t) { t = inner; }
                                first_other.get_or_insert(if matches!(ast.ty(*t), ast::Ty::Infer) { "placeholder" } else { "type" });
                            }
                            GenericArg::Const(_) => { first_other.get_or_insert("constant"); }
                            GenericArg::Lifetime(_) => {
                                if let Some(what) = first_other {
                                    self.err(cx, prog, file, p.lo, format!("E0747: {} provided when a lifetime was expected", what));
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                for g in ast.list(*v) {
                    let index = out.len() + declaration.as_ref().map_or(0, |(d, _)| self.inherited_arg_count(prog, *d));
                    match g {
                        GenericArg::Type(t) => {
                            // `Foo<(_)>`: parentheses do not change what the argument is
                            let mut t = t;
                            while let ast::Ty::Paren(inner) = ast.ty(*t) { t = inner; }
                            if matches!(ast.ty(*t), ast::Ty::Infer) {
                                if let (Some(infcx), Some((_, declaration))) = (cx.infcx, &declaration) {
                                    let ty = if declaration.consts.iter().any(|c| c.0 as usize == index) {
                                        self.tys.intern(TyKind::Const(infcx.next_const_var()))
                                    } else { infcx.next_ty_var() };
                                    out.push(ty); continue;
                                }
                            }
                            let mut arg = self.type_or_const_arg(cx, prog, file, module, env, *t);
                            // rustc hir_ty_lowering generic_arg_mismatch_err (E0747): an argument of
                            // the other kind than its parameter is an error, recovered as an error value
                            if let Some((_, declaration)) = &declaration {
                                let wants_const = declaration.consts.iter().any(|c| c.0 as usize == index);
                                // a parenthesized `_` is an inference variable of the parameter's kind
                                if wants_const && matches!(self.tys.kind(arg), TyKind::Infer(_)) && cx.infcx.is_some() { arg = self.tys.intern(TyKind::Const(cx.infcx.unwrap().next_const_var())); }
                                let is_const = matches!(self.tys.kind(arg), TyKind::Const(_));
                                if wants_const && !is_const && index < declaration.names.len() {
                                    self.err(cx, prog, file, p.lo, "E0747: type provided when a constant was expected".into());
                                    out.push(self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))));
                                    continue;
                                }
                                if !wants_const && is_const && index < declaration.names.len() {
                                    self.err(cx, prog, file, p.lo, "E0747: constant provided when a type was expected".into());
                                    out.push(self.tys.error);
                                    continue;
                                }
                            }
                            out.push(arg);
                        }
                        GenericArg::Const(e) => {
                            let ct = if let Some((d, declaration)) = &declaration {
                                if let Some(&(_, cfile, ty)) = declaration.consts.iter().find(|c| c.0 as usize == index) {
                                    // `trait Foo<const N: Bar<2>>` with `trait Bar<const M: Foo<2>>`: the type of a
                                    // const parameter names itself through the other (E0391)
                                    let Ok(_cycle) = crate::cycle::enter(crate::cycle::Kind::ConstParamType, d.0, index as u32) else {
                                        self.err(cx, prog, file, p.lo, format!("E0391: cycle detected when computing type of the const parameter {} of `{}`", index, prog.def_path(*d)));
                                        out.push(self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))));
                                        continue;
                                    };
                                    let expected = self.lower_ty(cx, prog, cfile, prog.def(*d).scope, declaration, ty);
                                    if is_infer_len(ast, *e) { cx.infcx.map_or_else(|| self.tys.intern_const(crate::consts::ConstKind::Error), |infcx| infcx.next_const_var()) }
                                    else { self.lower_const_arg(prog, file, env, *e, expected) }
                                } else {
                                    // typeck-audit 1.30: as typeck reports it (generics.rs E0747)
                                    self.err(cx, prog, file, ast.expr(*e).lo, "E0747: constant provided when a type was expected".into());
                                    self.tys.intern_const(crate::consts::ConstKind::Error)
                                }
                            } else { self.tys.intern_const(crate::consts::ConstKind::Error) };
                            out.push(self.tys.intern(TyKind::Const(ct)));
                        }
                        _ => {}
                    }
                }
            }
        }
        // hir_ty_lowering/mod.rs lower_generic_args_of_path: arguments beyond the declared
        // parameters are not lowered (E0107 reports them); the lowered list never exceeds the
        // declaration.
        if let Some((count, _)) = declaration.as_ref().and_then(|(d, _)| self.own_param_counts(prog, *d)) {
            out.truncate(count);
        }
        out
    }

    /// A generic argument written as a type that may name a constant (`Foo<SIZE>`, `Foo<N>`:
    /// the parser cannot tell them from types).
    pub fn type_or_const_arg(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, t: ast::TyId) -> TyId {
        let ast = &prog.files[file as usize].ast;
        if let ast::Ty::Path(p) = ast.ty(t) {
            if p.no_qself() && p.segs.len() == 1 && prog.files[file as usize].ast.segs(p.segs)[0].args().is_none() {
                let name = prog.syms.get(prog.files[file as usize].text(prog.files[file as usize].ast.segs(p.segs)[0].name)).unwrap_or(u32::MAX);
                // late.rs: a name that is a type in scope is a type argument, whatever const parameter shares it
                let type_item = prog.lookup_in_scope(module, name, true).is_some()
                    && !env.names.iter().enumerate().any(|(i, n)| *n == name && !env.consts.iter().any(|c| c.0 as usize == i));
                for &(index, _, _) in env.consts.iter().filter(|_| !type_item) {
                    if env.names.get(index as usize) == Some(&name) {
                        let ct = self.tys.intern_const(crate::consts::ConstKind::Param(crate::consts::ParamConst { index }));
                        return self.tys.intern(TyKind::Const(ct));
                    }
                }
                if !env.names.contains(&name) && prog.lookup_in_scope(module, name, true).is_none() {
                    if let Some(d) = prog.lookup_in_scope(module, name, false) {
                        if prog.def(d).kind == DefKind::Const {
                            // hir_ty_lowering/mod.rs:2880-2897: a named constant
                            // retains its declared type until solver normalization.
                            // Kept redesign (typeck-audit 2.12, front-bck redesign 7): stable
                            // rustc lowers this path as an anon const whose DefId ast_lowering
                            // creates on the fly (lib.rs:3014-3022); Relax creates DefIds at
                            // collection, before it knows the path names a const, so it keeps
                            // the const item's own Free alias. Both normalize to the item's
                            // value under the same args; only the argument's own type check
                            // (ConstArgHasType against the param) differs: the item's declared
                            // type is checked here instead of an anon body's coercion.
                            let args = self.generic_indices_of(prog, d).identity_args(&self.tys);
                            let ct = self.tys.intern_const(crate::consts::ConstKind::Alias(crate::consts::IsRigid::No,
                                crate::consts::AliasConst { kind: crate::consts::AliasConstKind::Free { def_id: d }, args: args.into() }));
                            return self.tys.intern(TyKind::Const(ct));
                        }
                    }
                }
            }
        }
        self.lower_ty(cx, prog, file, module, env, t)
    }

    /// The trait a bound's path names. Reports what rustc_resolve does for a path in trait
    /// position: E0405 for a name that is nothing, E0404 for one that is not a trait.
    pub fn resolve_trait_path(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path) -> Option<DefId> {
        let single = p.segs.len() == 1 && p.no_qself() && !p.global();
        // a generic parameter shadows the trait of that name in the type namespace
        if single {
            let name = prog.files[file as usize].text(prog.files[file as usize].ast.segs(p.segs)[0].name);
            if prog.syms.get(name).is_some_and(|s| env.names.contains(&s)) && prog.files[file as usize].ast.segs(p.segs)[0].kind() == SegKind::Ident {
                self.err(cx, prog, file, p.lo, format!("E0404: expected trait, found type parameter `{}`", name));
                return None;
            }
        }
        let Some(td) = self.resolve_type_path_def(prog, file, module, p) else {
            if single {
                self.err(cx, prog, file, p.lo, format!("E0405: cannot find trait `{}` in this scope", path_str(prog, file, p)));
            } else if p.segs.len() > 1 && p.no_qself() && !p.global() {
                // the module exists, the name is not in it
                let ast = &prog.files[file as usize].ast;
                let segs = crate::program::path_segs(prog, file, p);
                if prog.resolve_mod_path(module, false, &segs[..segs.len() - 1], false, false).is_some() {
                    let last = prog.files[file as usize].text(ast.segs(p.segs)[p.segs.len() - 1].name);
                    let parent = prog.files[file as usize].text(ast.segs(p.segs)[p.segs.len() - 2].name);
                    self.err(cx, prog, file, p.lo, format!("E0405: cannot find trait `{}` in `{}`", last, parent));
                } else if ast.segs(p.segs)[0].kind() == SegKind::Ident {
                    // the path starts with a name that is nothing in scope: no module, no crate
                    let first = prog.files[file as usize].text(ast.segs(p.segs)[0].name);
                    let known = prog.syms.get(first).is_some_and(|s| env.names.contains(&s) || prog.lookup_in_scope(module, s, true).is_some() || prog.lookup_in_scope(module, s, false).is_some() || prog.crates[prog.mods[module as usize].krate as usize].deps.iter().any(|d| d.0 == s));
                    if !known {
                        self.err(cx, prog, file, p.lo, format!("E0433: failed to resolve: use of unresolved module or unlinked crate `{}`", first));
                    }
                }
            }
            return None;
        };
        if self.prohibit_module_args(cx, prog, file, module, p) { return None; }
        let k = prog.def(td).kind;
        if matches!(k, DefKind::Trait) { return Some(td); }
        // (a name another namespace or a misresolved module holds is left alone)
        if matches!(k, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::AssocTy | DefKind::AssocFn | DefKind::AssocConst | DefKind::Fn | DefKind::Const | DefKind::Static | DefKind::Prim(_) | DefKind::Mod) {
            self.err(cx, prog, file, p.lo, format!("E0404: expected trait, found {} `{}`", def_kind_descr(k), path_str(prog, file, p)));
        }
        None
    }

    /// prohibit_generic_args on a segment that names no generic definition (a type parameter,
    /// `Self`): arguments are E0109, associated item constraints E0229.
    pub(crate) fn prohibit_args_on(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, seg: &ast::PathSeg, what: &str, name: &str) -> bool {
        if !self.validates(prog, file) { return false; }
        let ast = &prog.files[file as usize].ast;
        if let Some(GenericArgs::Angle(v)) = ast.gargs(seg.args()) {
            let args = ast.list(*v);
            if let Some(a) = args.iter().find(|a| matches!(a, GenericArg::Lifetime(_) | GenericArg::Type(_) | GenericArg::Const(_))) {
                let kind = match a { GenericArg::Lifetime(_) => "lifetime", GenericArg::Const(_) => "const", _ => "type" };
                self.err(cx, prog, file, seg.name.lo, format!("E0109: {} arguments are not allowed on {} `{}`", kind, what, name));
                return true;
            }
        }
        self.prohibit_constraints(cx, prog, file, seg.args())
    }

    /// prohibit_generic_args on the segments of a path that name modules (`Mod::<i32>::Item`):
    /// E0109. Only a segment with written arguments is resolved.
    pub fn prohibit_module_args(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, p: &ast::Path) -> bool {
        if p.segs.len() < 2 || !p.no_qself() { return false; }
        let ast = &prog.files[file as usize].ast;
        // rustc_ast_lowering lower_qpath: parenthesized sugar belongs to the last segment of a trait path
        if self.validates(prog, file) {
            let segs = ast.segs(p.segs);
            if let Some(seg) = segs[..segs.len() - 1].iter().find(|s| matches!(ast.gargs(s.args()), Some(GenericArgs::Paren(..)))) {
                self.err(cx, prog, file, seg.name.lo, "E0214: parenthesized type parameters may only be used with a `Fn` trait".to_string());
                return true;
            }
        }
        let written = |seg: &ast::PathSeg| matches!(ast.gargs(seg.args()), Some(GenericArgs::Angle(v)) if ast.list(*v).iter().any(|a| matches!(a, GenericArg::Lifetime(_) | GenericArg::Type(_) | GenericArg::Const(_))));
        let segs = ast.segs(p.segs);
        if !segs[..segs.len() - 1].iter().any(written) || !self.validates(prog, file) { return false; }
        let resolved = crate::program::path_segs(prog, file, p);
        for (i, seg) in segs[..segs.len() - 1].iter().enumerate() {
            if !written(seg) { continue; }
            if let Some(m) = prog.resolve_mod_path(module, p.global(), &resolved[..=i], false, false) {
                if prog.def(m).kind == DefKind::Mod {
                    let name = prog.name(m).to_string();
                    return self.prohibit_args_on(cx, prog, file, seg, "module", &name);
                }
            }
        }
        false
    }

    /// prohibit_assoc_item_constraint: `Foo<Item = T>` where no trait is named (E0229).
    pub fn prohibit_constraints(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, args: ast::GenericArgsId) -> bool {
        if !self.validates(prog, file) { return false; }
        let ast = &prog.files[file as usize].ast;
        if let Some(GenericArgs::Angle(v)) = ast.gargs(args) {
            if let Some(c) = ast.list(*v).iter().find_map(|a| match a { GenericArg::Binding(n, _, _) | GenericArg::Constraint(n, _, _) => Some(*n), _ => None }) {
                self.err(cx, prog, file, c.lo, "E0229: associated item constraints are not allowed here".to_string());
                return true;
            }
        }
        false
    }

    /// Resolves a path in the type namespace to a def.
    pub fn resolve_type_path_def(&self, prog: &Program, file: u32, module: u32, p: &ast::Path) -> Option<DefId> {
        let segs = crate::program::path_segs(prog, file, p);
        if segs.is_empty() {
            return None;
        }
        if segs.len() == 1 && !p.global() {
            if segs[0].0 != SegKind::Ident {
                return prog.resolve_mod_path(module, false, &segs, false, false);
            }
            return prog.lookup_in_scope(module, segs[0].1, true);
        }
        if segs.len() == 1 {
            // `::Name`: 2015 names an item of the crate root (extern crates are root items), 2018
            // an extern-prelude crate
            let krate = prog.mods[module as usize].krate;
            let (kind, name) = segs[0];
            if kind != SegKind::Ident { return None; }
            let from_extern = || prog.extern_crate(module, name).map(|c| prog.mods[prog.crates[c as usize].root_mod as usize].def);
            if prog.crates[krate as usize].edition < 2018 {
                let root = prog.mods[prog.crates[krate as usize].root_mod as usize].def;
                return prog.lookup_in_container(root, name, true).or_else(from_extern);
            }
            return from_extern();
        }
        let parent = prog.resolve_mod_path(module, p.global(), &segs[..segs.len() - 1], false, false)?;
        prog.lookup_in_container(parent, segs[segs.len() - 1].1, true)
    }


}

/// Reads `#[repr(..)]`: C, the integer type, and returns (align(N), packed(N)) (0 = absent;
/// `packed` alone is packed(1)).
/// `#[rustc_layout_scalar_valid_range_start(N)]`, `_end(N)` and `#[rustc_layout_no_niche]`.
fn read_layout_attrs(prog: &Program, file: u32, attrs: &[ast::Attr]) -> (Option<u128>, Option<u128>, bool) {
    let f = &prog.files[file as usize];
    let (mut start, mut end, mut no_niche) = (None, None, false);
    for a in attrs {
        if a.toks.hi <= a.toks.lo {
            continue;
        }
        let name = f.tok_text(a.toks.lo);
        let arg = || -> Option<u128> {
            if a.toks.lo + 2 < a.toks.hi && f.tok_text(a.toks.lo + 1) == "(" {
                crate::const_fold::parse_int_lit(f.tok_text(a.toks.lo + 2)).map(|v| v as u128)
            } else {
                None
            }
        };
        match name {
            "rustc_layout_scalar_valid_range_start" => start = arg(),
            "rustc_layout_scalar_valid_range_end" => end = arg(),
            "rustc_layout_no_niche" => no_niche = true,
            _ => {}
        }
    }
    (start, end, no_niche)
}

/// Which type params of a generics list are `?Sized` (inline bound or where clause).
fn maybe_unsized_params(prog: &Program, file: u32, g: &ast::Generics) -> Vec<bool> {
    let f = &prog.files[file as usize];
    let mut names = Vec::new();
    let mut out = Vec::new();
    for p in f.ast.list(g.params) {
        if let GenericParamKind::Type(bounds, _) = &p.kind {
            names.push(f.text(p.name).to_string());
            out.push(f.ast.list(*bounds).iter().any(|b| matches!(b, ast::Bound::Trait { maybe: true, .. })));
        }
    }
    for w in f.ast.list(g.where_) {
        if let ast::WherePred::Bound { ty, bounds, .. } = w {
            if let ast::Ty::Path(pth) = f.ast.ty(*ty) {
                if pth.segs.len() == 1 && f.ast.list(*bounds).iter().any(|b| matches!(b, ast::Bound::Trait { maybe: true, .. })) {
                    let n = f.text(prog.files[file as usize].ast.segs(pth.segs)[0].name);
                    if let Some(i) = names.iter().position(|x| x == n) {
                        out[i] = true;
                    }
                }
            }
        }
    }
    out
}

fn read_repr(prog: &Program, file: u32, attrs: &[ast::Attr], repr_c: &mut bool, repr_int: &mut Option<Prim>, repr_simd: &mut bool) -> (u32, u32) {
    let f = &prog.files[file as usize];
    let (mut align, mut packed) = (0u32, 0u32);
    for a in attrs {
        if a.toks.hi <= a.toks.lo || f.tok_text(a.toks.lo) != "repr" {
            continue;
        }
        for i in a.toks.lo + 1..a.toks.hi {
            let t = f.tok_text(i);
            // repr(transparent) lays out as its one field: as repr(C) does for it (and it keeps
            // the type out of live layout edits, layout.rs live_editable)
            if t == "C" || t == "transparent" {
                *repr_c = true;
            }
            if t == "simd" {
                *repr_simd = true;
            }
            if t == "align" || t == "packed" {
                // `align(N)` / `packed(N)` / bare `packed`
                let n = if i + 3 < a.toks.hi && f.tok_text(i + 1) == "(" {
                    crate::const_fold::parse_int_lit(f.tok_text(i + 2)).map_or(0, |v| v.min(1 << 29) as u32)
                } else {
                    1
                };
                if t == "align" {
                    align = align.max(n);
                } else {
                    packed = if packed == 0 { n } else { packed.min(n) };
                }
            }
            for (n, p) in crate::program::PRIMS {
                if n == t {
                    *repr_int = Some(p);
                }
            }
        }
    }
    (align, packed)
}

pub fn push_generics(prog: &Program, file: u32, g: &ast::Generics, env: &mut GenEnv) {
    let f = &prog.files[file as usize];
    for p in f.ast.list(g.params) {
        let region_index = env.regions.next_param;
        env.regions.next_param += 1;
        match &p.kind {
            GenericParamKind::Type(bounds, _) => {
                let i = env.names.len() as u32;
                env.names.push(prog.syms.get(f.text(p.name)).unwrap_or(u32::MAX));
                env.sized_params.push(i);
                for b in f.ast.list(*bounds) {
                    if let ast::Bound::Trait { path, maybe, .. } = b {
                        if *maybe { env.sized_params.retain(|p| *p != i); continue; }
                        env.bounds.push((i, file, path.clone()));
                    }
                }
            }
            GenericParamKind::Const(ty, _) => {
                env.consts.push((env.names.len() as u32, file, *ty));
                env.names.push(prog.syms.get(f.text(p.name)).unwrap_or(u32::MAX));
            }
            GenericParamKind::Lifetime(_) => env.regions.names.push((f.text(p.name).to_owned(), RegionKind::ReEarlyParam(region_index))),
        }
    }
    // `where T: Trait` on one of the params
    for w in f.ast.list(g.where_) {
        if let ast::WherePred::Bound { ty, bounds, .. } = w {
            if let ast::Ty::Path(p) = f.ast.ty(*ty) {
                if p.no_qself() && p.segs.len() == 1 && prog.files[file as usize].ast.segs(p.segs)[0].args().is_none() {
                    let name = prog.syms.get(f.text(prog.files[file as usize].ast.segs(p.segs)[0].name)).unwrap_or(u32::MAX - 1);
                    if let Some(i) = env.names.iter().rposition(|n| *n == name) {
                        for b in f.ast.list(*bounds) {
                            if let ast::Bound::Trait { path, maybe, .. } = b {
                                if *maybe { env.sized_params.retain(|p| *p != i as u32); continue; }
                                env.bounds.push((i as u32, file, path.clone()));
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn path_str(prog: &Program, file: u32, p: &ast::Path) -> String {
    let f = &prog.files[file as usize];
    let mut s = String::new();
    for (i, seg) in prog.files[file as usize].ast.segs(p.segs).iter().enumerate() {
        if i > 0 {
            s.push_str("::");
        }
        s.push_str(f.text(seg.name));
    }
    s
}

// ---------------------------------------------------------------- traits

impl Tcx {
    pub fn trait_generic_count(&self, prog: &Program, td: DefId) -> usize {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        if let Some(TraitItem { generics, .. }) = ast.as_trait(&ast.item(pd.item).kind) {
            let mut n = 0;
            for g in ast.list(generics.params) {
                if !matches!(g.kind, GenericParamKind::Lifetime(_)) {
                    n += 1;
                }
            }
            return n;
        }
        0
    }

    /// Trait args with defaults filled (`Rhs = Self` etc.).
    pub fn fill_trait_defaults(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, td: DefId, self_t: TyId, explicit: Vec<TyId>) -> Vec<TyId> {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        let mut out = explicit;
        let generics = match ast.as_trait(&ast.item(pd.item).kind) {
            Some(t) => t.generics,
            None => return out,
        };
        // (type and const params in order: `trait Traitor<const N: u8 = 1, const M: u8 = N>`)
        let mut k = 0;
        for g in ast.list(generics.params) {
            let (tdef, cdef) = match &g.kind {
                GenericParamKind::Type(_, d) => (*d, None),
                GenericParamKind::Const(_, d) => (None, *d),
                GenericParamKind::Lifetime(_) => continue,
            };
            if k >= out.len() {
                // `trait Foo<X = Box<dyn Foo>>`: the default names the trait whose defaults it is
                // part of (E0391, cycle detected when computing the type of `Foo::X`)
                let _cycle = match crate::cycle::enter(crate::cycle::Kind::TraitDefault, td.0, k as u32) {
                    Ok(g) => g,
                    Err(()) => {
                        self.err(cx, prog, pd.file, ast.item(pd.item).lo, format!("E0391: cycle detected when computing type of `{}::{}`", prog.def_path(td), prog.files[pd.file as usize].text(g.name)));
                        out.push(if cdef.is_some() { self.tys.intern(TyKind::Const(self.tys.intern_const(crate::consts::ConstKind::Error))) } else { self.tys.error });
                        k += 1;
                        continue;
                    }
                };
                let t = if tdef.is_some() || cdef.is_some() {
                    let mut env = GenEnv::default();
                    env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    env.regions.next_param += 1;
                    push_generics(prog, pd.file, &generics, &mut env);
                    env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
                    let lt = match (tdef, cdef) {
                        (Some(dt), _) => self.lower_ty(cx, prog, pd.file, pd.scope, &env, dt),
                        (_, Some(de)) => {
                            let GenericParamKind::Const(ty, _) = g.kind else { unreachable!() };
                            let expected = self.lower_ty(cx, prog, pd.file, pd.scope, &env, ty);
                            self.tys.intern(TyKind::Const(self.lower_const_arg(prog, pd.file, &env, de, expected)))
                        },
                        _ => self.tys.error,
                    };
                    let mut sub = vec![self_t];
                    sub.extend(out.iter().copied());
                    self.tys.subst(lt, &sub)
                } else {
                    self.tys.error
                };
                out.push(t);
            }
            k += 1;
        }
        out
    }

    /// Associated type `name` of trait `td` or (transitively) its supertraits.
    pub fn trait_assoc(&self, prog: &Program, td: DefId, name: Sym) -> Option<DefId> {
        for &it in &prog.traits[prog.def(td).sub as usize] {
            let d = prog.def(it);
            if d.name == name && d.kind == DefKind::AssocTy {
                return Some(it);
            }
        }
        for sup in self.trait_supers(prog, td) {
            if sup != td {
                if let Some(x) = self.trait_assoc(prog, sup, name) {
                    return Some(x);
                }
            }
        }
        None
    }

    /// hir_ty_lowering/mod.rs:902-914,1990: append the associated item's own
    /// arguments to the selected parent trait's arguments, including lifetimes.
    /// check_generic_arg_count for the last segment of an associated type path (`<T as Tr>::Gat<..>`).
    fn check_assoc_ty_args(&self, cx: &EvalCx<'_>, prog: &Program, file: u32, item: DefId, p: &ast::Path, infer_args: bool) -> bool {
        let ast = &prog.files[file as usize].ast;
        let Some(seg) = ast.segs(p.segs).last() else { return true };
        let errs = if !self.validates(prog, file) { Vec::new() } else { self.check_generic_arg_count(prog, item, ast.gargs(seg.args()), ast, self.own_generic_counts(prog, item, None), arg_count::ArgPosition::AssocType, infer_args, false) };
        for e in &errs { self.err(cx, prog, file, seg.name.lo, e.clone()); }
        errs.is_empty() && !self.prohibit_constraints(cx, prog, file, seg.args())
    }

    fn lower_generic_args_of_assoc_item(&self, cx: &EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, path: &ast::Path, parent: TyId, source_ty: Option<ast::TyId>) -> TyId {
        let TyKind::Alias(rigid, alias) = self.tys.kind(parent) else { return parent; };
        if let AliasTyKind::Projection { def_id } = alias.kind {
            if !self.check_assoc_ty_args(cx, prog, file, def_id, path, source_ty.is_none()) { return self.tys.error; }
        }
        let mut args = self.tys.list(alias.args).to_vec();
        let mut own_regions = self.written_lifetimes(cx.infcx, prog, file, env, path);
        let mut own_args = self.lower_generic_args_of_item(cx, prog, file, module, env, path, Some(alias.kind.def_id()));
        self.fit_assoc_own_args(prog, alias.kind.def_id(), &mut own_args, &mut own_regions);
        args.extend(own_args);
        let mut regions = self.tys.region_args(alias.regions).to_vec();
        regions.extend(own_regions);
        self.tys.intern(TyKind::Alias(*rigid, AliasTy { kind: alias.kind, args: self.tys.mk_list(&args), regions: self.tys.intern_region_args(&regions) }))
    }

    /// rustc_hir_analysis/src/hir_ty_lowering/bounds.rs:416-425: search
    /// instantiated supertraits, keeping the defining trait's full arguments.
    fn assoc_projection(&self, prog: &Program, trait_ref: crate::next_solver::TraitRef, name: Sym) -> Option<TyId> {
        use crate::next_solver::{PredicateKind, ClauseKind};
        let predicate = crate::next_solver::collect::trait_predicate(trait_ref.def_id, trait_ref.args);
        let mut stack = vec![predicate];
        let mut visited = crate::fxhash::HashSet::default();
        while let Some(predicate) = stack.pop() {
            if !visited.insert(predicate.clone()) { continue; }
            let PredicateKind::Clause(ClauseKind::Trait(tr)) = &predicate.kind else { continue; };
            for &item in &prog.traits[prog.def(tr.trait_ref.def_id).sub as usize] {
                let d = prog.def(item);
                if d.name == name && d.kind == DefKind::AssocTy {
                    return Some(self.alias_ty_with_args(AliasTyKind::Projection { def_id: item }, &tr.trait_ref.args));
                }
            }
            // (only the supertraits that may define the item are lowered: the full list would
            // lower `trait T: AsRef<Self::Bar>` through this very lookup)
            for bound in self.explicit_supertraits_containing_assoc_item(prog, tr.trait_ref.def_id, name) {
                stack.push(self.instantiate_supertrait(prog, &bound, &predicate));
            }
        }
        None
    }

    /// Direct supertraits of a trait (resolved from its `: A + B` bounds).
    /// Does trait `td` require `Self: Sized` (a `Sized` supertrait, directly or through its
    /// The signature of a fn item type's def: a fn's, or a tuple constructor's.
    #[inline]
    pub fn fn_item_sig(&self, prog: &Program, d: DefId) -> Option<&FnSigT> {
        self.fn_sig(prog, d).or_else(|| self.ctor_sigs.get(&d))
    }

    /// dyn_trait.rs: the associated types of the principal and its supertraits that a trait
    /// object type leaves unspecified. A binding of the object type or of a supertrait bound
    /// (`trait Sub: Super<Item = u8>`) specifies one; so does `where Self: Sized` on the item.
    fn missing_dyn_assoc_types(&self, prog: &Program, td: DefId, binds: &[(DefId, TyId, u32)]) -> Vec<DefId> {
        let mut closure: Vec<DefId> = vec![td];
        let mut i = 0;
        while i < closure.len() && i < 64 {
            for s in self.trait_supers(prog, closure[i]) { if !closure.contains(&s) { closure.push(s); } }
            i += 1;
        }
        // bindings written on supertrait bounds, by name
        let mut bound_names: Vec<Sym> = Vec::new();
        for &t in &closure {
            let d = prog.def(t);
            let ast = &prog.files[d.file as usize].ast;
            let Some(tr) = ast.as_trait(&ast.item(d.item).kind) else { continue };
            let mut lists = vec![tr.supers];
            for w in ast.list(ast.kind_generics(&ast.item(d.item).kind).map(|g| g.where_).unwrap_or_default()) {
                if let ast::WherePred::Bound { bounds, .. } = w { lists.push(*bounds); }
            }
            for l in lists {
                for b in ast.list(l) {
                    let ast::Bound::Trait { path, .. } = b else { continue };
                    let Some(last) = ast.segs(path.segs).last() else { continue };
                    // `trait Sub: Fn(A) -> R` binds `FnOnce::Output`
                    if let Some(GenericArgs::Paren(..)) = ast.gargs(last.args()) {
                        if let Some(sym) = prog.syms.get("Output") { bound_names.push(sym); }
                    }
                    if let Some(GenericArgs::Angle(v)) = ast.gargs(last.args()) {
                        for g in ast.list(*v) {
                            if let GenericArg::Binding(n, _, bt) = g {
                                // a bound whose value mentions `Self` cannot stand for the object's
                                // value (dyn_trait.rs: `references_self`): it must be written out
                                if ty_mentions_self(ast, *bt) { continue; }
                                if let Some(sym) = prog.syms.get(prog.files[d.file as usize].text(*n)) { bound_names.push(sym); }
                            }
                        }
                    }
                }
            }
        }
        let mut missing = Vec::new();
        for &t in &closure {
            for &it in &prog.traits[prog.def(t).sub as usize] {
                let d = prog.def(it);
                if d.kind != DefKind::AssocTy || prog.impl_trait_in_trait_data.contains_key(&it) { continue; }
                if binds.iter().any(|b| b.0 == it) || bound_names.contains(&d.name) { continue; }
                if self.generics_require_sized_self(prog, it) { continue; }
                missing.push(it);
            }
        }
        missing
    }

    pub fn trait_supers(&self, prog: &Program, td: DefId) -> Vec<DefId> {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        let mut out = Vec::new();
        if let Some(TraitItem { supers, .. }) = ast.as_trait(&ast.item(pd.item).kind) {
            for b in ast.list(*supers) {
                if let ast::Bound::Trait { path, .. } = b {
                    if let Some(d) = self.resolve_type_path_def(prog, pd.file, pd.scope, path) {
                        if prog.def(d).kind == DefKind::Trait {
                            out.push(d);
                        }
                    }
                }
            }
        }
        out
    }

    /// The supertraits of `td<targs>` as trait refs: (trait, its args without Self), the
    /// bounds' args substituted (`trait Foo<T>: Bar<i32> + Bar<T>` for Foo<bool>: Bar<i32>,
    /// Bar<bool>); Self stands for the error type (a trait object's Self is not a type here).
    pub fn trait_super_refs(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, td: DefId, targs: &[TyId]) -> Vec<(DefId, Vec<TyId>)> {
        self.trait_super_refs_of(cx, prog, td, self.tys.error, targs)
    }

    /// The associated-type bindings of a trait's supertrait bounds with Self = `self_t`
    /// (`trait FloatNumber: Neg<Output = Self>` for T: `<T as Neg>::Output == T`): the
    /// projection and its value (rustc elaborates them as projection predicates).
    pub fn trait_super_bindings(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, td: DefId, self_t: TyId, targs: &[TyId]) -> Vec<(TyId, TyId)> {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        let paths: Vec<ast::Path> = match ast.as_trait(&ast.item(pd.item).kind) {
            Some(TraitItem { supers, .. }) => ast.list(*supers).iter().filter_map(|b| match b {
                ast::Bound::Trait { path, .. } => Some(path.clone()),
                _ => None,
            }).collect(),
            _ => return Vec::new(),
        };
        let mut env = GenEnv::default();
        env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    env.regions.next_param += 1;
        if let Some(TraitItem { generics, .. }) = ast.as_trait(&ast.item(pd.item).kind) {
            push_generics(prog, pd.file, generics, &mut env);
        }
        env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
        env.trait_def = Some(td);
        // (`P::Assoc` in a supertrait's arguments resolves through the trait's own bounds)
        env.regions.owner = Some(td);
        let module = self.item_module(prog, td);
        let mut sub = vec![self_t];
        sub.extend(targs.iter().copied());
        let mut out = Vec::new();
        for path in paths {
            if let Some(GenericArgs::Paren(_, ret)) = ast.segs(path.segs).last().and_then(|l| ast.gargs(l.args())) {
                if let Some(d) = self.resolve_type_path_def(prog, pd.file, pd.scope, &path) {
                    if let Some(output) = self.callable_output(d) {
                        let written = self.lower_generic_args(cx, prog, pd.file, module, &env, &path);
                        let mut args = vec![self_t];
                        for t in written { args.push(self.tys.subst(t, &sub)); }
                        let projection = self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: output }, args: self.tys.mk_list(&args), regions: RegionArgsId::EMPTY }));
                        let ret = match ret { Some(t) => self.lower_ty(cx, prog, pd.file, module, &env, *t), None => self.tys.unit };
                        out.push((projection, self.tys.subst(ret, &sub)));
                    }
                }
                continue;
            }
            let binds: Vec<(ast::Ident, AstTy)> = match ast.segs(path.segs).last().and_then(|l| ast.gargs(l.args())) {
                Some(GenericArgs::Angle(v)) => ast.list(*v).iter().filter_map(|g| match g {
                    GenericArg::Binding(n, _, t) => Some((*n, *t)),
                    _ => None,
                }).collect(),
                _ => continue,
            };
            if binds.is_empty() {
                continue;
            }
            let d = match self.resolve_type_path_def(prog, pd.file, pd.scope, &path) {
                Some(d) if prog.def(d).kind == DefKind::Trait => d,
                _ => continue,
            };
            let explicit = self.lower_generic_args(cx, prog, pd.file, module, &env, &path);
            let explicit: Vec<TyId> = explicit.iter().map(|x| self.tys.subst(*x, &sub)).collect();
            let args = self.fill_trait_defaults(cx, prog, d, self_t, explicit);
            for (n, t) in binds {
                let sym = match prog.syms.get(prog.files[pd.file as usize].text(n)) {
                    Some(x) => x,
                    None => continue,
                };
                if let Some(ad) = self.trait_assoc(prog, d, sym) {
                    let mut pa = vec![self_t];
                    pa.extend(args.iter().copied());
                    let proj = self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: self.tys.mk_list(&pa), regions: crate::types::RegionArgsId::EMPTY }));
                    let v = self.lower_ty(cx, prog, pd.file, module, &env, t);
                    out.push((proj, self.tys.subst(v, &sub)));
                }
            }
        }
        out
    }

    /// trait_super_refs with Self = `self_t` (`trait Try: FromResidual` is
    /// `FromResidual<<Self as Try>::Residual>`, `trait Ord: PartialOrd` is `PartialOrd<Self>`).
    pub fn trait_super_refs_of(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, td: DefId, self_t: TyId, targs: &[TyId]) -> Vec<(DefId, Vec<TyId>)> {
        let pd = prog.def(td);
        let ast = &prog.files[pd.file as usize].ast;
        let paths: Vec<ast::Path> = match ast.as_trait(&ast.item(pd.item).kind) {
            Some(TraitItem { supers, .. }) => ast
                .list(*supers)
                .iter()
                .filter_map(|b| match b {
                    ast::Bound::Trait { path, .. } => Some(path.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if paths.is_empty() {
            return Vec::new();
        }
        // the trait's own environment: Self is param 0, its params follow
        let mut env = GenEnv::default();
        env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    env.regions.next_param += 1;
        if let Some(TraitItem { generics, .. }) = ast.as_trait(&ast.item(pd.item).kind) {
            push_generics(prog, pd.file, generics, &mut env);
        }
        env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
        env.trait_def = Some(td);
        env.regions.owner = Some(td);
        let module = self.item_module(prog, td);
        let mut sub = vec![self_t];
        sub.extend(targs.iter().copied());
        let mut out = Vec::new();
        for path in paths {
            let d = match self.resolve_type_path_def(prog, pd.file, pd.scope, &path) {
                Some(d) if prog.def(d).kind == DefKind::Trait => d,
                _ => continue,
            };
            let explicit = self.lower_generic_args(cx, prog, pd.file, module, &env, &path);
            let explicit: Vec<TyId> = explicit.iter().map(|x| self.tys.subst(*x, &sub)).collect();
            // rustc_type_ir/src/elaborate.rs instantiates supertrait clauses without
            // normalizing them; obligations normalize when they are registered
            let args = self.fill_trait_defaults(cx, prog, d, self_t, explicit);
            out.push((d, args));
        }
        out
    }

    /// An impl's `type Name = T;` lowered in the impl's environment.
    pub fn impl_assoc_ty(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, imp: DefId, name: Sym) -> Option<TyId> {
        cx.state.impl_read.set(true);
        if imp == crate::program::NO_DEF {
            return None;
        }
        let ii = prog.def(imp).sub as usize;
        for &it in &prog.impls[ii].items {
            let d = prog.def(it);
            if d.name == name && d.kind == DefKind::AssocTy {
                let env = self.item_env(prog, it);
                let ast = &prog.files[d.file as usize].ast;
                if let Some(TypeAliasItem { ty: Some(t), .. }) = ast.as_alias(&ast.item(d.item).kind) {
                    return Some(self.lower_ty(cx, prog, d.file, d.scope, &env, *t));
                }
            }
        }
        None
    }

    /// The declared bounds of associated type `ad` (`type IntoIter: Iterator<Item = ..>;`), as
    /// (trait, trait args) with `args` (the projection's [Self, trait args..]) substituted: rustc
    /// item_bounds of the associated type.
    pub fn assoc_decl_bounds(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, ad: DefId, args: &[TyId]) -> Vec<(DefId, Vec<TyId>)> {
        self.assoc_decl_bounds_eqs(cx, prog, ad, args).0
    }

    /// assoc_decl_bounds and the bounds' bindings as projection equalities (`type IntoIter:
    /// Iterator<Item = Self::Item>`: `<<T as IntoIterator>::IntoIter as Iterator>::Item ==
    /// <T as IntoIterator>::Item`).
    pub fn assoc_decl_bounds_eqs(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, ad: DefId, args: &[TyId]) -> (Vec<(DefId, Vec<TyId>)>, Vec<(TyId, TyId)>) {
        let d = prog.def(ad);
        let td = d.parent;
        if td == crate::program::NO_DEF || prog.def(td).kind != DefKind::Trait {
            return (Vec::new(), Vec::new());
        }
        let ast = &prog.files[d.file as usize].ast;
        let paths: Vec<ast::Path> = match ast.as_alias(&ast.item(d.item).kind) {
            Some(TypeAliasItem { bounds, .. }) => ast
                .list(*bounds)
                .iter()
                .filter_map(|b| match b {
                    ast::Bound::Trait { maybe: false, path, .. } => Some(path.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let maybe_sized = match ast.as_alias(&ast.item(d.item).kind) {
            Some(&TypeAliasItem { bounds, .. }) => ast.list(bounds).iter().any(|b| matches!(b, ast::Bound::Trait { maybe: true, .. })),
            _ => false,
        };
        let proj = self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: self.tys.mk_list(args), regions: crate::types::RegionArgsId::EMPTY }));
        let mut eqs = Vec::new();
        let pd = prog.def(td);
        let tast = &prog.files[pd.file as usize].ast;
        let mut env = GenEnv::default();
        env.names.push(prog.syms.get("Self").unwrap_or(u32::MAX));
                    env.regions.next_param += 1;
        if let Some(TraitItem { generics, .. }) = tast.as_trait(&tast.item(pd.item).kind) {
            push_generics(prog, pd.file, generics, &mut env);
        }
        env.self_ty = Some(self.tys.intern(TyKind::Param(0)));
        env.trait_def = Some(td);
        env.regions.owner = Some(td);
        let module = self.item_module(prog, td);
        let mut out = Vec::new();
        if !maybe_sized { if let Some(sized) = self.lang_items.get(LangItem::Sized) { out.push((sized, Vec::new())); } }
        for path in paths {
            let bd = match self.resolve_type_path_def(prog, d.file, d.scope, &path) {
                Some(x) if prog.def(x).kind == DefKind::Trait => x,
                _ => continue,
            };
            let explicit = self.lower_generic_args(cx, prog, d.file, module, &env, &path);
            let explicit: Vec<TyId> = explicit.iter().map(|x| self.tys.subst(*x, args)).collect();
            let ba = self.fill_trait_defaults(cx, prog, bd, proj, explicit);
            if let Some(GenericArgs::Angle(v)) = ast.segs(path.segs).last().and_then(|l| ast.gargs(l.args())) {
                for g in ast.list(*v) {
                    if let GenericArg::Binding(n, _, ty) = g {
                        let sym = prog.syms.get(prog.files[d.file as usize].text(*n)).unwrap_or(u32::MAX);
                        if let Some(bad) = self.trait_assoc(prog, bd, sym) {
                            let v = self.lower_ty(cx, prog, d.file, module, &env, *ty);
                            let mut pa = vec![proj];
                            pa.extend(ba.iter().copied());
                            eqs.push((self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: bad }, args: self.tys.mk_list(&pa), regions: crate::types::RegionArgsId::EMPTY })), self.tys.subst(v, args)));
                        }
                    }
                }
            }
            out.push((bd, ba));
        }
        (out, eqs)
    }

    /// Written impl bounds for param-environment elaboration; proof goes through solve.
    fn resolve_impl_bounds(&mut self, cx: &EvalCx<'_>, imp: DefId, prog: &Program) {
        if let Some(env) = self.impl_generics(prog, imp) {
            let module = self.item_module(prog, imp);
            for (param, file, path) in &env.bounds {
                if let Some(td) = self.resolve_type_path_def(prog, *file, module, path) {
                    if prog.def(td).kind != DefKind::Trait { continue; }
                    let args = self.lower_generic_args(cx, prog, *file, module, &env, path);
                    self.impl_trait_bounds.entry(imp).or_default().push((*param, td, args));
                }
            }
        }
    }

}

/// Round-31 L1': rustc_interface/src/limits.rs:32-39 @8d1a764. Unlike
/// rustc's single-crate tcx, our property queries share multiple crate policies.
/// The value, rather than an owner identity, is part of each TypingEnv key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecursionLimit(pub usize);
impl Default for RecursionLimit { fn default() -> Self { Self(128) } }
impl RecursionLimit {
    pub fn for_crate(prog: &Program, krate: u32) -> Self {
        Self(prog.crates.get(krate as usize).map_or(128, |c| c.recursion_limit))
    }

    pub fn for_item(prog: &Program, def: DefId) -> Self {
        if def == crate::program::NO_DEF { Self::default() }
        else { Self::for_crate(prog, prog.def(def).krate) }
    }
}

/// Predicate environment and typing mode, following rustc_middle/src/ty/mod.rs:1050.
#[derive(Clone, Copy, Debug, Hash)]
pub struct TypingEnv {
    pub limit: RecursionLimit,
    pub param_env: ParamEnvId,
    pub mode: TypingMode,
}

pub use crate::infer::typing_mode::{TypingMode, TypingModeEqWrapper};
impl PartialEq for TypingEnv {
    fn eq(&self, other: &Self) -> bool {
        self.limit == other.limit && self.param_env == other.param_env && TypingModeEqWrapper(self.mode) == TypingModeEqWrapper(other.mode)
    }
}
impl Eq for TypingEnv {}

impl TypingEnv {
    /// rustc_middle/src/ty/mod.rs:1073: codegen has no predicates in scope.
    pub fn fully_monomorphized(prog: &Program) -> Self {
        Self { param_env: ParamEnvId::EMPTY, mode: TypingMode::PostAnalysis, limit: RecursionLimit::for_crate(prog, crate::layout::root_crate()) }
    }
}

/// One explicit query stack. Mutable traversal state is not part of cache equality
/// (rustc_infer/src/infer/mod.rs:244; rustc_trait_selection/src/traits/select/mod.rs:1354).
pub struct EvalCx<'p> {
    diagnostics: crate::infer::diagnostics::DiagCtxt,
    pub infcx: Option<&'p crate::infer::InferCtxt<'p>>,
    pub prog: &'p Program,
    pub typing_env: TypingEnv,
    pub state: &'p EvalState,
}

/// Selection stacks and constant frames follow one explicit evaluation. Constant inputs
/// mirror rustc_const_eval/src/interpret/eval_context.rs:27 (InterpCx and its typing env).
#[derive(Default)]
pub struct EvalState {
    pub(crate) impl_read: std::cell::Cell<bool>,
    par_impls: std::cell::Cell<bool>,
    pub(crate) errors: std::cell::RefCell<Option<Vec<String>>>,
    pub(crate) const_too_generic: std::cell::Cell<bool>,
    /// const_eval::fold_without_execution: named constants fold without the query cache
    pub(crate) fold_only: std::cell::Cell<bool>,
    pub(crate) const_generic_frame: std::cell::RefCell<Option<(GenEnv, Vec<TyId>)>>,
    alias_inprog: std::cell::RefCell<crate::fxhash::HashSet<(TypingEnv, DefId)>>,
    /// generic parameter defaults being lowered: (item, parameter position)
    default_inprog: std::cell::RefCell<Vec<(DefId, usize)>>,
    pub tc_depth: std::cell::Cell<u32>,
}

impl EvalState {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<'p> EvalCx<'p> {
    pub fn new(prog: &'p Program, tcx: &Tcx, typing_env: TypingEnv, state: &'p EvalState) -> Self {
        Self { prog, typing_env, state, infcx: None, diagnostics: tcx.dcx.clone() }
    }


    pub fn errors(&self) -> &crate::infer::diagnostics::DiagCtxt {
        match self.infcx { Some(infcx) => &infcx.errors, None => &self.diagnostics }
    }

    pub fn with_infcx<'a>(&'a self, infcx: &'a crate::infer::InferCtxt<'a>) -> EvalCx<'a> {
        EvalCx { prog: self.prog, typing_env: self.typing_env, state: self.state, infcx: Some(infcx), diagnostics: infcx.errors.clone() }
    }

    pub fn body(&self) -> Self {
        Self { prog: self.prog, typing_env: self.typing_env, state: self.state, infcx: None, diagnostics: self.errors().clone() }
    }


}

/// `_` (also parenthesized) as an array length / repeat count: inferred (rustc
/// generic_arg_infer, stable since 1.89).
pub fn is_infer_len(ast: &crate::ast::Ast, n: crate::ast::ExprId) -> bool {
    match &ast.expr(n).kind {
        crate::ast::ExprKind::Underscore => true,
        crate::ast::ExprKind::Paren(x) => is_infer_len(ast, *x),
        _ => false,
    }
}

/// Tcx and Program are shared across compile threads.
#[allow(dead_code)]
fn _assert_sync() {
    fn s<T: Sync + Send>() {}
    s::<Tcx>();
    s::<crate::program::Program>();
}

#[cfg(test)]
mod typing_tests;

mod callable;

// rustc_errors/src/lib.rs: flush delayed diagnostics at the session boundary.
impl Drop for Tcx {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            if !self.errors.is_empty() || !self.err_sink.get_mut().unwrap().is_empty() { self.dcx.note_reported_error(); }
            self.dcx.flush_delayed();
        }
    }
}

fn collect_argument_impl_traits(ast: &ast::Ast, t: ast::TyId, out: &mut Vec<ast::TyId>) {
    match ast.ty(t) {
        ast::Ty::Ref(_, _, x) | ast::Ty::Paren(x) | ast::Ty::Ptr(_, x) | ast::Ty::Slice(x) | ast::Ty::Array(x, _) => collect_argument_impl_traits(ast, *x, out),
        ast::Ty::Tuple(v) => ast.list(*v).iter().for_each(|x| collect_argument_impl_traits(ast, *x, out)),
        ast::Ty::ImplTrait(bs) => {
            out.push(t);
            // nested in its own bounds too (`impl IntoIterator<Item = impl Into<String>>`)
            for b in ast.list(*bs) {
                if let ast::Bound::Trait { path, .. } = b {
                    for seg in ast.segs(path.segs) {
                        if let Some(a) = ast.gargs(seg.args()) {
                            if let GenericArgs::Angle(v) = a {
                                for g in ast.list(*v) {
                                    if let GenericArg::Type(x) | GenericArg::Binding(_, _, x) = g {
                                        collect_argument_impl_traits(ast, *x, out);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        ast::Ty::Path(p) => {
            for seg in ast.segs(p.segs) {
                if let Some(a) = ast.gargs(seg.args()) {
                    if let GenericArgs::Angle(v) = a {
                        for g in ast.list(*v) {
                            match g {
                                GenericArg::Type(x) | GenericArg::Binding(_, _, x) => collect_argument_impl_traits(ast, *x, out),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// rustc's DefKind::descr for the kinds a type path can wrongly name (E0573).
pub(crate) fn def_kind_descr(k: DefKind) -> &'static str {
    match k {
        DefKind::Struct => "struct", DefKind::Enum => "enum", DefKind::Union => "union", DefKind::TypeAlias => "type alias",
        DefKind::Trait => "trait", DefKind::AssocTy => "associated type", DefKind::Prim(_) => "builtin type",
        DefKind::Fn => "function", DefKind::AssocFn => "associated function", DefKind::Const => "constant",
        DefKind::AssocConst => "associated constant", DefKind::Static => "static", DefKind::Mod => "module",
        DefKind::Variant => "variant", DefKind::MacroRules => "macro", DefKind::ForeignFn => "foreign function",
        DefKind::ForeignStatic => "foreign static", DefKind::Field => "field", _ => "item",
    }
}

/// Whether a written type names `Self` (also as the qualified-path self type).
fn ty_mentions_self(ast: &ast::Ast, t: ast::TyId) -> bool {
    match ast.ty(t) {
        ast::Ty::Path(p) => {
            if let Some(q) = ast.qself(p) { if ty_mentions_self(ast, q.ty) { return true; } }
            ast.segs(p.segs).first().is_some_and(|s| s.kind() == SegKind::SelfType)
        }
        ast::Ty::Ref(_, _, i) | ast::Ty::Ptr(_, i) | ast::Ty::Slice(i) | ast::Ty::Array(i, _) | ast::Ty::Paren(i) => ty_mentions_self(ast, *i),
        ast::Ty::Tuple(l) => ast.list(*l).iter().any(|&i| ty_mentions_self(ast, i)),
        _ => false,
    }
}
