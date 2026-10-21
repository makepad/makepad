// ported from N:rustc_hir_typeck/src/callee.rs @8d1a764
// ported from N:rustc_hir_typeck/src/method/mod.rs @8d1a764
// ported from N:rustc_hir_typeck/src/fn_ctxt/_impl.rs @8d1a764
// ported from N:rustc_hir_typeck/src/lib.rs @8d1a764
// ported from N:rustc_hir_typeck/src/upvar.rs @8d1a764
// ported from N:rustc_hir_typeck/src/closure.rs @8d1a764
// ported from compiler/rustc_trait_selection/src/traits/project.rs @8d1a764
//! Definition-owned type checking under the declared parameter environment.

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::next_solver::{Goal, Predicate, PredicateKind, ClauseKind, AliasTerm, Term};
use crate::next_solver::select::ImplSource;
use crate::ast::*;
use crate::program::{DefId, DefKind, Prim, Program, Sym, NO_DEF};
use crate::tcx::{ty_key, FnSigT, GenEnv, Tcx, TyKey};
use crate::types::{Abi, FnHeader, Safety, TyId, TyKind};
use crate::fxhash::HashMap;

pub(crate) mod probe;
mod closure;
mod writeback;
mod opaque_types;
mod call;
mod checks;
mod cast;
mod coerce_unsized;
mod pat_bindings;
mod instantiate;
mod side_table;
mod place_op;
pub mod adjustments;
pub mod user_types;
pub use side_table::{BitSet, SideTable, ThinVec};
pub use call::CallTrait;
pub use closure::ClosureSignatures;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Res {
    Local(u32),
    Def(DefId),
    /// a const generic param used as a value (`N`): index into the instance args
    ConstParam(u32),
    Err,
}

#[derive(Clone, Copy)]
pub struct MethodRes {
    pub def: DefId,
    /// number of derefs applied to the receiver before the call
    pub derefs: u32,
    /// 0 none, 1 `&`, 2 `&mut`
    pub autoref: u8,
    /// full generic args (impl params then method params)
    pub args: crate::types::TyList,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coerce {
    /// fn item -> fn pointer
    ReifyFn,
    /// non-capturing closure -> fn pointer
    ClosureFn,
    /// safe fn pointer -> unsafe fn pointer (rustc coercion.rs:1034).
    UnsafeFn,
    /// &[T; N] -> &[T]
    Unsize,
    /// &mut T -> &T, *mut T -> *const T
    MutToConst,
    /// Deref followed by Borrow(RawPtr): the reference itself is not consumed.
    RefToRaw(bool),
    /// `!` -> any
    Never,
    /// pointer to a sized type -> pointer to `dyn Trait` (&T, *T, Box<T>, Rc<T>, ..):
    /// (source pointee, dyn type)
    ToDyn(TyId, TyId),
    /// `&A -> &B` through n autoderef steps (`&String -> &str`, `&Vec<T> -> &[T]`); overloaded
    /// steps are recorded in ov_derefs under (expr | DEREF_COERCE_KEY, step)
    DerefRef(usize),
    /// trait object upcasting `&dyn Sub -> &dyn Super` (also Box/Rc/raw pointers): the vtable
    /// pointer becomes Sub's stored vtable for Super (rustc vtable_trait_upcasting_coercion_new_vptr_slot):
    /// (source dyn type, target dyn type)
    Upcast(TyId, TyId),
}

pub const DEREF_COERCE_KEY: u32 = 0x8000_0000;
/// res_args of a path pattern are recorded under (pat id | PAT_KEY): pattern and expression
/// ids are separate arenas (`<i32 as Foo>::NUM =>` needs Self = i32 to pick the impl's const)
pub const PAT_KEY: u32 = 0x4000_0000;
/// `pat_local` key of an async fn parameter whose pattern is matched inside the body coroutine: the parameter's
/// pattern id with this bit gives the local holding the whole argument.
pub const ASYNC_ARG_KEY: u32 = 0x2000_0000;

#[derive(Clone)]
pub struct Local {
    pub name: Sym,
    pub ty: TyId,
    pub mutable: bool,
    pub pat: u32,
}

#[derive(Clone, Copy)]
pub enum ExpectedClosureKind {
    Fn,
    FnMut,
    FnOnce,
}

/// Results of type checking a definition, including its nested closures.
#[derive(Clone)]
pub struct Body {
    pub def: DefId,
    pub args: ThinVec<TyId>,
    pub file: u32,
    pub expr_lo: u32,
    pub expr_ty: ThinVec<TyId>,
    /// Presence in nightly TypeckResults::node_types: arena holes are not error types.
    pub expr_typed: BitSet,
    pub pat_lo: u32,
    pub pat_ty: ThinVec<TyId>,
    pub pat_typed: BitSet,
    /// a lazily parsed body's nodes (ParsedFile::part) follow the item's own range in
    /// expr_ty / pat_ty: their first ids (u32::MAX: none) and the item range's lengths
    pub expr_lo2: u32,
    pub expr_n1: u32,
    pub pat_lo2: u32,
    pub pat_n1: u32,
    pub res: SideTable<u32, Res>,
    pub res_args: SideTable<u32, crate::types::TyList>,
    pub pat_res: SideTable<u32, Res>,
    pub pat_local: SideTable<u32, u32>,
    pub methods: SideTable<u32, MethodRes>,
    /// binary operators on non-primitive types: expr -> operator trait method call
    pub binops: SideTable<u32, MethodRes>,
    /// The adjustments of each expression node, one list (typeck/adjustments.rs): coercions,
    /// reference reborrows (rustc coerce_to_ref, including equal-type ones) and autoderef steps,
    /// overloaded ones with their `Deref::deref` call.
    pub adjustments: SideTable<u32, adjustments::Adjustments>,
    /// rustc `user_provided_types`: the canonical user type annotations (written lifetimes kept,
    /// holes canonical variables) of let annotations, casts, generic paths, struct literals and
    /// patterns, method calls with explicit arguments.
    pub user_provided_types: SideTable<user_types::UserTypeKey, std::sync::Arc<crate::types::user_type::CanonicalUserType>>,
    /// rustc `user_provided_sigs`: the signature a closure expression wrote, canonicalized before
    /// it is merged with the expected one; keyed by the closure expression.
    pub user_provided_sigs: SideTable<u32, std::sync::Arc<crate::next_solver::canonical::Canonical<TyId>>>,
    pub min_captures: SideTable<u32, crate::upvar::ClosureCaptures>,
    pub hidden_types: SideTable<DefId, TyId>,
    pub field_idx: SideTable<u32, (u32, u32)>,
    /// every struct field the body names, resolved: (ADT, field index) -- a live layout edit's
    /// dependents (reload.rs: the fns using a removed or renamed field, not its name)
    pub field_uses: ThinVec<(DefId, u32)>,
    /// every `.await` of the body: (the enclosing coroutine's expression, the awaited operand's
    /// type, position) -- the by-value edges of async recursion (E0733)
    pub awaits: ThinVec<(u32, TyId, u32)>,
    /// Resolved definitions read while checking; deduplicated by the warm session.
    pub dependencies: ThinVec<DefId>,
    /// overloaded indexing: expr -> `Index::index` with [Self, Idx]
    pub ov_index: SideTable<u32, (DefId, crate::types::TyList)>,
    /// `IndexMut::index_mut` with [Self, Idx] for an overloaded index used as a mutable place
    /// (place_op.rs): rustc replaces the type-dependent def; `ov_index` keeps naming `Index::index`.
    pub mutable_index: SideTable<u32, (DefId, crate::types::TyList)>,
    /// `a[range]` on an array, slice or `str`: the std `Index` impl typeck leaves to the builtin
    /// slicing (codegen lowers it natively, so `ov_index` does not name it); THIR reads it as the
    /// overloaded place rustc has.
    pub std_index: SideTable<u32, (DefId, crate::types::TyList)>,
    /// Every checked `for` head -> `Iterator::next` with [Iter].
    pub for_next: SideTable<u32, (DefId, crate::types::TyList)>,
    /// Every checked `for` head -> (`IntoIterator::into_iter`, [Self], iterator type).
    /// The head is passed by value with identity adjustment (no method receiver coercion).
    pub for_into: SideTable<u32, (DefId, crate::types::TyList, TyId)>,
    /// Fn-family selection in the generic body, before instance substitution.
    pub call_traits: SideTable<u32, CallTrait>,
    /// calls of `#[rustc_legacy_const_generics]` fns: the argument positions that are const
    /// generic args (not passed as values)
    pub legacy_args: SideTable<u32, Vec<u32>>,
    /// match ergonomics: pattern -> references peeled before matching it
    pub pat_derefs: SideTable<u32, usize>,
    /// identifier patterns bound by reference through the default binding mode (1 ref, 2 ref mut)
    pub pat_bind_ref: SideTable<u32, u8>,
    /// Semantic discriminant reads of constructor patterns, including irrefutable
    /// patterns of enums with uninhabited variants (ExprUseVisitor::walk_pat).
    /// These are capture uses even when runtime lowering needs no discriminant test.
    pub pat_discriminant_reads: SideTable<u32, ()>,
    /// casts that are coercions (rustc `set_coercion_cast`); recorded only while the lint sink is on
    pub coercion_casts: SideTable<u32, ()>,
    /// type-relative value paths (`Ty::item`, `<Ty>::item`) that resolved to an item of a trait, not an inherent one; recorded only while the lint sink is on (prelude collision lints)
    pub trait_item_paths: SideTable<u32, DefId>,
    /// The two lang-item calls of the `?` desugaring, with their inferred types.
    pub try_branch: SideTable<u32, (DefId, crate::types::TyList, TyId)>,
    pub try_from_residual: SideTable<u32, (DefId, crate::types::TyList)>,
    pub locals: ThinVec<Local>,
    pub param_pats: ThinVec<PatId>,
    pub ret: TyId,
    pub body: Option<BlockId>,
    /// (macro expr, named arg) -> resolution of `{name}` in a format string
    pub fmt_named: SideTable<(u32, Sym), Res>,
    /// closures that capture nothing (can become plain functions)
    pub closures: ThinVec<u32>,
    pub closure_locals: SideTable<u32, Vec<u32>>,
    /// Kind deduced from the closure's expected type, before capture inference.
    pub closure_expected_kinds: SideTable<u32, ExpectedClosureKind>,
    /// PolyFnSig and the signature used inside each closure body.
    pub closure_sigs: SideTable<u32, ClosureSignatures>,
    pub inline_const_types: SideTable<u32, TyId>,
    /// `S { a: 1, .. }` (bare `..`): struct literal expr -> (field index, the const holding the
    /// field's declared default) for every omitted field (rustc default_field_values)
    pub struct_defaults: SideTable<u32, Vec<(u32, DefId)>>,
    pub errors: ThinVec<String>,
    /// A failed check's lookups that missed in modules the lazy Rust selection skipped
    /// (program.rs note_deferred_miss): every reader of the cached result notes them again, so
    /// the build that fails on it defers to the post-boot loader (jit.rs DEFERRED_RETRY) like
    /// the run that computed it. Loading a module republishes declarations, which recomputes it.
    pub deferred_misses: ThinVec<(u32, String)>,
    /// Clauses for this body's parameter space: the declaration's environment
    /// after typeck, or the caller's after instantiation for analysis.
    pub param_env: Option<crate::tcx::ParamEnvId>,
}

impl Body {
    /// A body with nothing in it (compiler-generated functions lowered without source).
    pub fn empty(def: DefId, args: Vec<TyId>, file: u32, unit: TyId) -> Body {
        Body {
            def,
            args: args.into(),
            file,
            expr_lo: 0,
            expr_ty: ThinVec::new(),
            expr_typed: BitSet::default(),
            pat_lo: 0,
            pat_ty: ThinVec::new(),
            pat_typed: BitSet::default(),
            expr_lo2: u32::MAX,
            expr_n1: 0,
            pat_lo2: u32::MAX,
            pat_n1: 0,
            res: SideTable::new(),
            res_args: SideTable::new(),
            pat_res: SideTable::new(),
            pat_local: SideTable::new(),
            methods: SideTable::new(),
            binops: SideTable::new(),
            adjustments: SideTable::new(),
            user_provided_types: SideTable::new(),
            user_provided_sigs: SideTable::new(),
            min_captures: SideTable::new(),
            hidden_types: SideTable::new(),
            field_idx: SideTable::new(),
            field_uses: ThinVec::new(),
            awaits: ThinVec::new(),
            dependencies: ThinVec::new(),
            ov_index: SideTable::new(),
            mutable_index: SideTable::new(),
            std_index: SideTable::new(),
            for_next: SideTable::new(),
            for_into: SideTable::new(),
            call_traits: SideTable::new(),
            legacy_args: SideTable::new(),
            pat_derefs: SideTable::new(),
            pat_bind_ref: SideTable::new(),
            pat_discriminant_reads: SideTable::new(),
            coercion_casts: SideTable::new(),
            trait_item_paths: SideTable::new(),
            try_branch: SideTable::new(),
            try_from_residual: SideTable::new(),
            locals: ThinVec::new(),
            param_pats: ThinVec::new(),
            ret: unit,
            body: None,
            fmt_named: SideTable::new(),
            closures: ThinVec::new(),
            closure_locals: SideTable::new(),
            closure_expected_kinds: SideTable::new(),
            closure_sigs: SideTable::new(),
            inline_const_types: SideTable::new(),
            struct_defaults: SideTable::new(),
            errors: ThinVec::new(),
            deferred_misses: ThinVec::new(),
            param_env: None,
        }
    }

    #[inline]
    pub fn ty(&self, e: ExprId) -> TyId {
        self.expr_ty[self.expr_ix(e)]
    }
    #[inline]
    pub fn pty(&self, p: PatId) -> TyId {
        self.pat_ty[self.pat_ix(p)]
    }
    /// Index of expression `e` in expr_ty (not checked against its length).
    #[inline]
    pub fn expr_ix(&self, e: ExprId) -> usize {
        if e.0 >= self.expr_lo2 {
            (e.0 - self.expr_lo2 + self.expr_n1) as usize
        } else {
            e.0.wrapping_sub(self.expr_lo) as usize
        }
    }
    #[inline]
    pub fn pat_ix(&self, p: PatId) -> usize {
        if p.0 >= self.pat_lo2 {
            (p.0 - self.pat_lo2 + self.pat_n1) as usize
        } else {
            p.0.wrapping_sub(self.pat_lo) as usize
        }
    }
    /// The expression at expr_ty index `i`.
    #[inline]
    pub fn expr_at(&self, i: usize) -> ExprId {
        if (i as u32) < self.expr_n1 || self.expr_lo2 == u32::MAX {
            ExprId(self.expr_lo + i as u32)
        } else {
            ExprId(self.expr_lo2 + i as u32 - self.expr_n1)
        }
    }
    /// Expression `e` is one this body typed.
    #[inline]
    pub fn has_expr(&self, e: ExprId) -> bool {
        self.expr_ix(e) < self.expr_ty.len()
    }

    /// Heap bytes this body owns (memory report: tables and arrays at their capacity, nested
    /// vectors followed one level), without the struct itself.
    pub fn heap_bytes(&self) -> usize {
        let mut acc = std::collections::BTreeMap::new();
        self.mem_breakdown(&mut acc);
        (acc.values().map(|x| x.1).sum::<u64>() - std::mem::size_of::<Body>() as u64) as usize
    }

    /// The results are final: every table and array holds exactly its entries.
    pub fn freeze(&mut self) {
        macro_rules! shrink { ($($f:ident),*) => { $( self.$f.shrink_to_fit(); )* } }
        shrink!(res, res_args, pat_res, pat_local, methods, binops, adjustments, user_provided_types, user_provided_sigs, min_captures, hidden_types,
            field_idx, ov_index, mutable_index, std_index, for_next, for_into, call_traits, legacy_args,
            pat_derefs, pat_bind_ref, pat_discriminant_reads, try_branch, try_from_residual, fmt_named, closure_locals,
            closure_expected_kinds, closure_sigs, inline_const_types, struct_defaults,
            args, expr_ty, expr_typed, pat_ty, pat_typed, field_uses, awaits, dependencies, locals, param_pats, closures, errors);
    }

    /// Memory report (MAKEPAD_RUN_MEM_REPORT): per table (entries, heap bytes incl. nested vectors),
    /// accumulated into `acc` by name.
    /// Memory report: the bytes these contents took in the previous layout, counted as that
    /// layout's report counted them: a 1304 B struct; HashMap side tables grown by insertion
    /// (4, 8, else the next power of two of n * 8 / 7 buckets of entry + 1 control byte, + 16)
    /// with their entries' previous sizes and Vec<TyId> payloads; Vec fields at their growth
    /// capacity (exact where they were built at their length).
    pub fn prev_layout_bytes(&self, tys: &crate::types::Types) -> usize {
        fn m(n: usize, entry: usize) -> usize {
            if n == 0 {
                return 0;
            }
            let buckets = if n < 4 { 4 } else if n < 8 { 8 } else { (n * 8 / 7).next_power_of_two() };
            buckets * (entry + 1) + 16
        }
        fn t<K, V>(x: &SideTable<K, V>) -> usize {
            m(x.len(), std::mem::size_of::<(K, V)>())
        }
        fn grown<T>(x: &ThinVec<T>) -> usize {
            if x.is_empty() { 0 } else { x.len().next_power_of_two().max(4) * std::mem::size_of::<T>() }
        }
        fn v<T>(x: &Vec<T>) -> usize {
            x.capacity() * std::mem::size_of::<T>()
        }
        let l = |x: crate::types::TyList| tys.list(x).len() * 4;
        let mut n = 1304;
        n += t(&self.res) + t(&self.pat_res) + t(&self.pat_local) + t(&self.hidden_types) + t(&self.call_traits) + t(&self.pat_derefs) + t(&self.pat_bind_ref) + t(&self.pat_discriminant_reads) + t(&self.fmt_named) + t(&self.closure_expected_kinds) + t(&self.closure_sigs) + t(&self.inline_const_types) + t(&self.legacy_args) + t(&self.closure_locals) + t(&self.struct_defaults) + t(&self.min_captures);
        // entries that held a Vec<TyId> (24 B) or a usize where they now hold a list id or a u32
        n += m(self.res_args.len(), 32) + m(self.methods.len(), 48) + m(self.binops.len(), 48) + m(self.ov_index.len(), 40) + m(self.for_next.len(), 40) + m(self.for_into.len(), 40) + m(self.try_branch.len(), 40) + m(self.try_from_residual.len(), 40) + m(self.field_idx.len(), 24);
        n += self.res_args.values().map(|&x| l(x)).sum::<usize>() + self.methods.values().chain(self.binops.values()).map(|x| l(x.args)).sum::<usize>();
        n += self.ov_index.values().chain(self.for_next.values()).chain(self.try_from_residual.values()).map(|x| l(x.1)).sum::<usize>();
        n += self.for_into.values().chain(self.try_branch.values()).map(|x| l(x.1)).sum::<usize>();
        n += self.legacy_args.values().map(v).sum::<usize>() + self.closure_locals.values().map(v).sum::<usize>() + self.struct_defaults.values().map(v).sum::<usize>();
        n += self.min_captures.values().map(|c| v(&c.captures) + v(&c.fake_reads) + v(&c.errors) + c.captures.iter().map(|x| v(&x.place.projections)).sum::<usize>() + c.fake_reads.iter().map(|x| v(&x.0.projections)).sum::<usize>()).sum::<usize>();
        n += (self.args.len() + self.expr_ty.len() + self.pat_ty.len()) * 4 + self.expr_typed.len() + self.pat_typed.len();
        n += grown(&self.field_uses) + grown(&self.dependencies) + grown(&self.locals) + grown(&self.param_pats) + grown(&self.closures) + grown(&self.errors) + grown(&self.deferred_misses);
        n += self.errors.iter().map(|e| e.capacity()).sum::<usize>();
        n
    }

    pub fn mem_breakdown(&self, acc: &mut std::collections::BTreeMap<&'static str, (u64, u64)>) {
        fn v<T>(x: &Vec<T>) -> usize {
            x.capacity() * std::mem::size_of::<T>()
        }
        fn t<T>(x: &ThinVec<T>) -> usize {
            x.heap_bytes()
        }
        fn m<K, V>(x: &SideTable<K, V>) -> usize {
            x.heap_bytes()
        }
        let mut add = |name: &'static str, n: usize, b: usize| {
            let e = acc.entry(name).or_default();
            e.0 += n as u64;
            e.1 += b as u64;
        };
        add("struct", 1, std::mem::size_of::<Body>());
        add("args", self.args.len(), t(&self.args));
        add("expr_ty", self.expr_ty.len(), t(&self.expr_ty));
        add("expr_typed", self.expr_typed.len(), self.expr_typed.heap_bytes());
        add("pat_ty", self.pat_ty.len(), t(&self.pat_ty));
        add("pat_typed", self.pat_typed.len(), self.pat_typed.heap_bytes());
        macro_rules! maps { ($($f:ident),*) => { $( add(stringify!($f), self.$f.len(), m(&self.$f)); )* } }
        maps!(res, res_args, methods, binops, ov_index, mutable_index, std_index, for_next, for_into, try_branch, try_from_residual, pat_res, pat_local, hidden_types, field_idx, call_traits, pat_derefs, pat_bind_ref, pat_discriminant_reads, fmt_named, closure_expected_kinds, closure_sigs, inline_const_types);
        add("adjustments", self.adjustments.len(), m(&self.adjustments) + self.adjustments.values().map(|a| a.heap_bytes()).sum::<usize>());
        add("user_provided_types", self.user_provided_types.len(), m(&self.user_provided_types) + self.user_provided_types.values().map(|user| {
            std::mem::size_of_val(&**user) + user.var_kinds.capacity() * std::mem::size_of::<crate::next_solver::canonical::CanonicalVarKind>()
        }).sum::<usize>());
        add("user_provided_sigs", self.user_provided_sigs.len(), m(&self.user_provided_sigs) + self.user_provided_sigs.values().map(|sig| std::mem::size_of_val(&**sig) + sig.var_kinds.capacity() * std::mem::size_of::<crate::next_solver::canonical::CanonicalVarKind>()).sum::<usize>());
        add("legacy_args", self.legacy_args.len(), m(&self.legacy_args) + self.legacy_args.values().map(v).sum::<usize>());
        add("closure_locals", self.closure_locals.len(), m(&self.closure_locals) + self.closure_locals.values().map(v).sum::<usize>());
        add("struct_defaults", self.struct_defaults.len(), m(&self.struct_defaults) + self.struct_defaults.values().map(v).sum::<usize>());
        add("min_captures", self.min_captures.len(), m(&self.min_captures) + self.min_captures.values().map(|c| v(&c.captures) + v(&c.fake_reads) + v(&c.errors) + c.captures.iter().map(|x| v(&x.place.projections)).sum::<usize>() + c.fake_reads.iter().map(|x| v(&x.0.projections)).sum::<usize>()).sum::<usize>());
        add("field_uses", self.field_uses.len(), t(&self.field_uses));
        add("dependencies", self.dependencies.len(), t(&self.dependencies));
        add("locals", self.locals.len(), t(&self.locals));
        add("param_pats", self.param_pats.len(), t(&self.param_pats));
        add("closures", self.closures.len(), t(&self.closures));
        add("errors", self.errors.len(), t(&self.errors) + self.errors.iter().map(|e| e.capacity()).sum::<usize>());
        add("deferred_misses", self.deferred_misses.len(), t(&self.deferred_misses));
    }
}

struct Loop {
    /// (name, position of the label: hygiene)
    label: Option<(Sym, u32)>,
    break_ty: Option<TyId>,
    kind: LoopKind,
}

/// The source of a loop-like break target (hir::LoopSource, plus labeled blocks).
#[derive(Clone, Copy, PartialEq)]
enum LoopKind {
    Loop,
    While,
    For,
    Block,
}

struct FcxSnapshot {
    inference: crate::infer::snapshot::CombinedSnapshot,
}

pub struct Fcx<'a> {
    cx: crate::tcx::EvalCx<'a>,
    prog: &'a Program,
    tcx: &'a Tcx,
    file: u32,
    module: u32,
    env: GenEnv,
    b: Body,
    scopes: Vec<(Sym, u32)>,
    /// per local (by index): its hygiene context and the source position of its binding
    /// (u32::MAX context: `self`, matched by any use)
    local_hyg: Vec<(Option<crate::parser::HygCtxt>, u32)>,
    /// the attributes of the match arm whose pattern is being checked (lint levels)
    arm_attrs: Vec<Attr>,
    infcx: crate::infer::InferCtxt<'a>,
    fulfillment_cx: crate::next_solver::fulfill::FulfillmentCtxt,
    /// the pending obligations outside the open snapshots (innermost last): a snapshot works on
    /// a fresh context, but what was pending before it is still visible to `type_var_is_sized`
    suspended_fulfillment: Vec<crate::next_solver::fulfill::FulfillmentCtxt>,
    param_env: crate::next_solver::ParamEnvId,
    loops: Vec<Loop>,
    in_loop_condition: bool,
    /// the item-holding block modules being checked, with how many locals were in scope when each
    /// began: its items sit inside those locals' ribs and outside the later ones
    item_blocks: Vec<(u32, usize)>,
    diverges: bool,
    ret_stack: Vec<TyId>,
    closure_depth: Vec<(u32, usize)>, // (closure expr, scope depth at entry)
    /// checking a const / static initializer or an anonymous constant (no `return` outside a closure)
    in_const: bool,
    /// `b.locals.len()` where each enclosing inline `const { }` block began: an outer local is no value there (E0435)
    inline_const_floor: Vec<u32>,
    /// checking an anonymous constant that is a typeck root of its own (array length, const argument)
    in_anon_const: bool,
    /// > 0 inside an async block or async fn body (where `.await` is allowed); a closure
    /// inside resets it
    async_depth: u32,
    /// `.await`s of the coroutine body being checked (its suspension points)
    await_count: u32,
    cur_binop: u32,
    /// resolve without defaulting `{integer}`/`{float}` (mid-check normalisation)
    keep_literal_vars: bool,
    /// Decided once per body, before any work: user-written types are saved only for crates that
    /// will be borrow-checked (event::user_types_wanted) -- they cost canonicalization otherwise.
    keep_user_types: bool,
    /// default binding mode while checking a pattern: 0 move, 1 ref, 2 ref mut
    bind_mode: u8,
    /// nesting of check_pat (0: a pattern's root, where its bindings are checked as a set)
    pat_depth: u32,
    /// a path pattern's resolution, made before its adjust mode is chosen (rustc pat.rs
    /// resolve_pat_path / calc_adjust_mode): (the resolution, its type)
    pat_path_pre: Option<(Res, TyId)>,
    /// calls whose bound-driven inference is retried after each statement
    /// calls whose Fn-bounded argument's type was still open when the call was checked
    /// (`parallel_for(tasks)` with `tasks` collected later): their Fn bounds are applied again
    /// at the end (rustc keeps the `F: FnOnce() -> R` obligation pending)
    /// type of a synthesized trailing method argument (consumed by the next method check)
    extra_arg: Option<TyId>,
    /// Postorder closure analysis, after body inference and before upvar tuples are fixed.
    pending_captures: Vec<crate::upvar::PendingCapture>,
    deferred_call_resolutions: Vec<call::DeferredCallResolution>,
    deferred_sized_obligations: Vec<(TyId, u32)>,
    /// checking the pattern of a `let` without `else` (rustc PatInfo.decl_origin = LocalDecl { els: None })
    pat_decl_irrefutable: bool,
    checked_opaque_types_storage_entries: Option<crate::infer::opaque_types::OpaqueTypeStorageEntries>,
    expected_closure_kinds: HashMap<u32, crate::upvar::ClosureKind>,
    /// an async closure's deduced `AsyncFnOnce::Output` term as a signature (bound), set by deduction
    async_expected_inner: Option<TyId>,
    /// async closure expr -> its coroutine's liberated expected return type (rustc coroutine-closure `return_ty`)
    async_body_expected: HashMap<u32, TyId>,
    /// the expectation of the expression checked next is a mere hint (the other branch's type),
    /// not a coercion target (rustc Expectation::ExpectHasType vs. an if/match's derived hint)
    soft: bool,
    /// position of the expression checked last (patterns have no spans of their own)
    last_lo: u32,
    /// traits of the body's generic bounds (and its trait), with supertraits: in scope for
    /// method calls (computed on first use)
    /// trait args of the next trait method call, fixed by a generic bound
    known_method_args: Option<Vec<crate::consts::GenericArg>>,
    /// The written lifetime arguments of the value path being resolved.
    value_path_lifetimes: Vec<crate::types::RegionId>,
    /// The lifetimes written on the segment before the last (`Enum::<'a>::Variant`).
    value_path_enum_lifetimes: Vec<crate::types::RegionId>,
    /// The impl's lifetime args of an inherent associated value path (confirm.rs
    /// fresh_receiver_args): the item's FnDef carries them, not fresh ones.
    value_path_parent_regions: Vec<crate::types::RegionId>,
    /// `[x; N]` expressions whose `Copy` check waits for the end of the body (rustc
    /// deferred_repeat_expr_checks): (element, element type, the array type)
    deferred_repeats: Vec<(ExprId, TyId, TyId)>,
    /// goals drained as waiting for this body's coroutines (`TypeckResults::coroutine_stalled_predicates`)
    stalled_coroutine_goals: Vec<crate::next_solver::fulfill::PredicateObligation>,
    /// the probe picked a by-value method of `&U` for receiver U: autoref it
    forced_autoref: Option<u8>,
    /// the expected type of the method call being checked (check_method's expectation)
    method_expected: Option<TyId>,
    /// coercions to a `dyn` pointer whose source pointee was still open: (expr, actual,
    /// expected, pos), decided in `finish`
    deferred_dyn: Vec<(ExprId, TyId, TyId, u32)>,
    /// Reference-to-pointer casts and casts whose target pointee was open.
    deferred_casts: Vec<(ExprId, TyId, TyId)>,
    /// every `e as T` (cast expression's position), validated after fallback (cast.rs)
    cast_checks: Vec<(ExprId, TyId, TyId, u32)>,
    /// how many patterns enclose the one being checked
    pat_nest: u32,
    /// inference variables a `!` value was coerced to (rustc diverging type variables): left
    /// unconstrained, they fall back to `()` (`!` from edition 2024) at the end
    diverging: Vec<TyId>,
}

/// Codegen shares identity bodies; lowering erases regions and normalizes on use,
/// as rustc_codegen_ssa::mir::FunctionCx::monomorphize does. Analysis consumers
/// requesting fully normalized tables use instantiate_body instead.
pub fn codegen_body(prog: &Program, tcx: &Tcx, def: DefId, args: &[TyId]) -> std::sync::Arc<Body> {
    try_codegen_body(prog, tcx, def, args).expect("failed to normalize body for codegen")
}

/// codegen_body for a caller that has a fallback: an instance whose types do not normalize
/// (an ill-formed program, e.g. `bop::<dyn Bop>` with a `Self: Sized` associated type) is None.
pub fn try_codegen_body(prog: &Program, tcx: &Tcx, def: DefId, args: &[TyId]) -> Option<std::sync::Arc<Body>> {
    let _cost_typeck = relax_codegen::fncost::phase(1);
    let _mem = crate::memstat::tag(crate::memstat::Tag::Typeck);
    let body = tcx.typeck(prog, def);
    // an instance with the wrong number of type/const arguments (`fn main<T>()`, E0131) has no
    // body to monomorphize (instantiating it trips complete_args' assertion)
    if *body.args != *args && args.len() != tcx.generic_indices_of(prog, def).types_consts.len() {
        return None;
    }
    instantiate::try_instantiate_mir_and_normalize_erasing_regions(prog, tcx, body, args).ok()
}

pub fn instantiate_body(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, def: DefId, args: &[TyId]) -> Body {
    try_instantiate_body(cx, prog, tcx, def, args).expect("failed to normalize body for codegen")
}

/// rustc_middle/src/ty/normalize_erasing_regions.rs:172-186: normalization failure
/// is a result for consumers that cannot require a fully monomorphic instance.
pub(crate) fn try_instantiate_body(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, def: DefId, args: &[TyId]) -> Result<Body, crate::next_solver::NoSolution> {
    // Diagnostic only: sample one in N calls, including queries outside JIT scopes.
    // Keep this off for timing runs; symbolizing a backtrace is deliberately expensive.
    static SAMPLE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sample = *SAMPLE.get_or_init(|| std::env::var("MAKEPAD_RUN_TYPECK_BACKTRACE").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
    if sample != 0 && CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % sample == 0 {
        let trace = format!("makepad-run typeck-backtrace: {} def={} args={:?} origin={} time={}\n{}\nmakepad-run typeck-backtrace-end\n",
            prog.def_path(def), def.0, args, crate::clock::TYS_ORIGIN.with(|o| o.get()),
            relax_codegen::fncost::mono(), std::backtrace::Backtrace::force_capture());
        // The app may write directly to fd 2 without Rust's stderr lock. Format
        // first, so its startup messages cannot split the diagnostic header.
        use std::io::Write;
        let _ = std::io::stderr().lock().write_all(trace.as_bytes());
    }
    let _census = relax_codegen::exec_census::Record::body(relax_codegen::exec_census::instance(def.0,
        || args.iter().map(|t| t.0).collect(), || {
            let d = prog.def(def);
            relax_codegen::exec_census::Metadata {
                name: format!("{}<{args:?}>", prog.def_path(def)),
                krate: prog.syms.str(prog.crates[d.krate as usize].name).to_owned(),
                module: prog.def_path(prog.mods[prog.mods[d.scope as usize].normal as usize].def),
                initializer: match d.kind { DefKind::Static => "static-initializer", DefKind::Const | DefKind::AssocConst => "const-initializer", _ => "" },
            }
        }));
    let _cost_typeck = relax_codegen::fncost::phase(1);
    let cost_start = if relax_codegen::fncost::enabled() { (relax_codegen::fncost::mono(), relax_backend::cpu_ns()) } else { (0, 0) };
    let _mem = crate::memstat::tag(crate::memstat::Tag::Typeck);
    let body = tcx.typeck(prog, def);
    // Analysis uses the supplied environment; codegen supplies a monomorphic one.
    let b = instantiate::try_normalize_body(prog, tcx, cx.typing_env, &body, args)?;
    if relax_codegen::fncost::enabled() {
        relax_codegen::fncost::type_detail(def.0, &args.iter().map(|a| a.0).collect::<Vec<_>>(), b.expr_ty.len(), &prog.def_path(def), cost_start.0, cost_start.1);
    }
    Ok(b)
}

/// rustc_hir_typeck/src/lib.rs:110-148: one inference context for the root body.
pub(crate) fn check_fn(prog: &Program, tcx: &Tcx, def: DefId) -> Body {
    let _mem = crate::memstat::tag(crate::memstat::Tag::Typeck);
    let args = tcx.generic_indices_of(prog, def).identity_types_consts(&tcx.tys);
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
    static ST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let st = *ST.get_or_init(|| std::env::var("MAKEPAD_RUN_TYSTATS").is_ok());
    let t0 = if st { relax_backend::cpu_ns() } else { 0 };
    let _warm = relax_codegen::warm::enabled().then(|| relax_codegen::warm::Scope::body(def.0, &args.iter().map(|t| t.0).collect::<Vec<_>>(), crate::clock::TYS_ORIGIN.with(|o| o.get()) == "gandalf", || {
        let args: Vec<_> = args.iter().map(|t| ty_to_string(false, prog, tcx, *t)).collect();
        if args.is_empty() { prog.def_path(def) } else { format!("{}<{}>", prog.def_path(def), args.join(", ")) }
    }));
    let b = check_fn_body(&cx, prog, tcx, def, &args);
    if st {
        eprintln!("TYS\t{}\t{}\t{}\t{}\t{}\t{:?}\t{}", def.0, args.len(), relax_backend::cpu_ns() - t0, b.expr_ty.len(), prog.def_path(def), args.iter().map(|a| a.0).collect::<Vec<u32>>(), crate::clock::TYS_ORIGIN.with(|o| o.get()));
    }
    b
}

/// MAKEPAD_RUN_LAZY_STATS: the fn defs whose bodies were type-checked (what a lazy parse would parse)
pub static CHECKED_DEFS: relax_session::sync::Mutex<Option<std::collections::HashSet<u32>>> = relax_session::sync::Mutex::new(None);

fn check_fn_body(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, def: DefId, args: &[TyId]) -> Body {
    let _why = crate::why::enabled().then(|| {
        let cause = prog.why_def(def, "typeck-caller");
        crate::why::event("typeck", &cause.file, cause.line, &cause.symbol, None);
        crate::why::enter(cause)
    });
    static LAZY_STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *LAZY_STATS.get_or_init(|| std::env::var_os("MAKEPAD_RUN_LAZY_STATS").is_some()) {
        CHECKED_DEFS.lock().unwrap().get_or_insert_with(Default::default).insert(def.0);
    }
    let d = prog.def(def);
    let file = d.file;
    let (ranges, ranges2) = match prog.body_ranges(file, d.item) {
        Ok(r) => r,
        Err(e) => {
            let mut b = Body::empty(def, args.to_vec(), file, tcx.tys.unit);
            b.errors.push(e);
            return b;
        }
    };
    let env = tcx.item_env(prog, def);
    let sig = match tcx.fn_sig(prog, def) {
        Some(s) => s.clone(),
        None => FnSigT { region_contract: Default::default(), generic_indices: Default::default(), bound_regions: 0,
            params: Vec::new(),
            ret: tcx.tys.unit,
            self_kind: 0,
            n_generics: 0,
            n_parent_generics: 0,
            is_const: false,
            rustc_promotable: false,
            rustc_intrinsic: false,
            header: FnHeader::RUST,
            track_caller: false,
            target_features: Vec::new(),
            safe_target_features: false,
        },
    };
    let n_expr1 = ranges[1] - ranges[0];
    let n_pat1 = ranges[3] - ranges[2];
    let n_expr = (n_expr1 + ranges2.map_or(0, |r| r[1] - r[0])) as usize;
    let n_pat = (n_pat1 + ranges2.map_or(0, |r| r[3] - r[2])) as usize;
    let error = tcx.tys.error;
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::Typeck { defining_opaque_types_and_generators: tcx.opaque_types_defined_by(prog, def) }, def);

    let mut fcx = Fcx {
        cx: cx.body(),
        prog,
        tcx,
        file,
        module: d.scope,
        env,
        b: Body {
            def,
            args: ThinVec::from_slice(args),
            file,
            expr_lo: ranges[0],
            expr_ty: vec![error; n_expr].into(),
            expr_typed: BitSet::new(n_expr),
            pat_lo: ranges[2],
            pat_ty: vec![error; n_pat].into(),
            pat_typed: BitSet::new(n_pat),
            expr_lo2: ranges2.map_or(u32::MAX, |r| r[0]),
            expr_n1: n_expr1,
            pat_lo2: ranges2.map_or(u32::MAX, |r| r[2]),
            pat_n1: n_pat1,
            res: SideTable::new(),
            res_args: SideTable::new(),
            pat_res: SideTable::new(),
            pat_local: SideTable::new(),
            methods: SideTable::new(),
            binops: SideTable::new(),
            adjustments: SideTable::new(),
            user_provided_types: SideTable::new(),
            user_provided_sigs: SideTable::new(),
            min_captures: SideTable::new(),
            hidden_types: SideTable::new(),
            field_idx: SideTable::new(),
            field_uses: ThinVec::new(),
            awaits: ThinVec::new(),
            dependencies: ThinVec::new(),
            ov_index: SideTable::new(),
            mutable_index: SideTable::new(),
            std_index: SideTable::new(),
            for_next: SideTable::new(),
            for_into: SideTable::new(),
            call_traits: SideTable::new(),
            legacy_args: SideTable::new(),
            try_branch: SideTable::new(),
            try_from_residual: SideTable::new(),
            pat_derefs: SideTable::new(),
            pat_bind_ref: SideTable::new(),
            pat_discriminant_reads: SideTable::new(),
            coercion_casts: SideTable::new(),
            trait_item_paths: SideTable::new(),
            locals: ThinVec::new(),
            param_pats: ThinVec::new(),
            ret: error,
            body: None,
            fmt_named: SideTable::new(),
            closures: ThinVec::new(),
            closure_locals: SideTable::new(),
            closure_expected_kinds: SideTable::new(),
            closure_sigs: SideTable::new(),
            inline_const_types: SideTable::new(),
            struct_defaults: SideTable::new(),
            errors: ThinVec::from_slice(tcx.signature_errors(prog, def)),
            deferred_misses: ThinVec::new(),
            param_env: None,
        },
        scopes: Vec::new(),
        local_hyg: Vec::new(),
        arm_attrs: Vec::new(),
        fulfillment_cx: crate::next_solver::fulfill::FulfillmentCtxt::new(&infcx),
        suspended_fulfillment: Vec::new(),
        infcx,
        param_env: Default::default(),
        loops: Vec::new(),
        in_loop_condition: false,
        item_blocks: Vec::new(),
        diverges: false,
        ret_stack: Vec::new(),
        closure_depth: Vec::new(),
        in_const: false,
        inline_const_floor: Vec::new(),
        in_anon_const: false,
        async_depth: 0,
        await_count: 0,
        cur_binop: 0,
        keep_literal_vars: true,
        keep_user_types: crate::borrowck::event::user_types_wanted(prog, prog.def(def).krate),
        diverging: Vec::new(),
        deferred_dyn: Vec::new(),
        deferred_casts: Vec::new(),
        cast_checks: Vec::new(),
        pat_nest: 0,
        bind_mode: 0,
        pat_depth: 0,
        pat_path_pre: None,
        extra_arg: None,
        pending_captures: Vec::new(),
        deferred_call_resolutions: Vec::new(),
        deferred_sized_obligations: Vec::new(),
        pat_decl_irrefutable: false,
        checked_opaque_types_storage_entries: None,
        expected_closure_kinds: HashMap::default(),
        async_expected_inner: None,
        async_body_expected: HashMap::default(),
        soft: false,
        last_lo: 0,
        known_method_args: None,
        value_path_lifetimes: Vec::new(),
        value_path_enum_lifetimes: Vec::new(),
        value_path_parent_regions: Vec::new(),
        deferred_repeats: Vec::new(),
        stalled_coroutine_goals: Vec::new(),
        forced_autoref: None,
        method_expected: None,
    };
    fcx.init_body_regions();
    let penv = tcx.param_env_id(prog, def);
    fcx.param_env = penv;
    // (also an empty one: a body checked inside another's check never sees the outer's env)
    fcx.cx.typing_env = crate::tcx::TypingEnv { limit: fcx.infcx.recursion_limit, param_env: penv, mode: crate::tcx::TypingMode::Typeck { defining_opaque_types_and_generators: tcx.opaque_types_defined_by(prog, def) } };
    if penv != crate::tcx::ParamEnvId::EMPTY {
        fcx.b.param_env = Some(penv);
    }
    let (params, ret) = fcx.body_signature(&sig, args);
    if params.iter().any(|p| fcx.tcx.tys.has_error(*p)) || fcx.tcx.tys.has_error(ret) {
        // the signature's own error is the root cause; checking the body would only cascade
        let lo = prog.files[file as usize].ast.item(d.item).lo;
        fcx.err(lo, "signature has unresolved types -- fix the signature errors first".to_string());
        return fcx.into_body();
    }
    // the body defines its own opaque return types: checked against inference variables
    // whose values become the hidden types (rustc check_fn / replace_opaque_types_with_inference_vars)
    let ret = fcx.norm(ret);
    fcx.b.ret = ret;
    fcx.ret_stack.push(ret);
    let ast = &prog.files[file as usize].ast;
    if d.kind == DefKind::AnonConst {
        // rustc_hir_typeck/src/lib.rs: check the anonymous constant's own body.
        let init = crate::const_eval::const_body(prog, def).expect("anonymous const body");
        let ct = tcx.tys.subst(tcx.type_of(prog, def).expect("anonymous const type"), args);
        // hir_typeck lib.rs:194-206: normalized, well formed and Sized (SizedConstOrStatic)
        let ct = fcx.norm(ct);
        fcx.register_wf(ct, ast.expr(init).lo);
        if let Some(sized) = fcx.tcx.lang_items.get(crate::lang_items::LangItem::Sized) {
            let goal = fcx.trait_goal(sized, ct, &[]);
            fcx.register_goal(goal, ast.expr(init).lo);
        }
        fcx.b.ret = ct;
        fcx.ret_stack.push(ct);
        fcx.in_const = true;
        fcx.in_anon_const = !prog.repeat_expr_counts.contains(&def);
        let t = fcx.check_expr(init, Some(ct));
        fcx.coerce_or_unify(t, ct, Some(init), ast.expr(init).lo);
        fcx.finish();
        return fcx.into_body();
    }
    let (fsig, body) = match &ast.item(d.item).kind {
        ItemKind::Fn(s, b, _) => (ast.fn_sig(*s), b.or_else(|| prog.fn_body(file, d.item).ok().flatten())),
        ItemKind::Const(_, init) | ItemKind::Static(_, _, init) if init.is_some() => {
            let init = &prog.const_body(file, d.item).expect("body_ranges validated initializer").unwrap_or(init.get().unwrap());
            // const/static initializer: checked against the declared type
            let ct = fcx.tcx.const_ty(prog, def).unwrap_or(error);
            let ct = fcx.tcx.tys.subst(ct, args);
            // hir_typeck lib.rs:194-198: the normalized expected type is well formed
            let ct = fcx.norm(ct);
            fcx.register_wf(ct, ast.expr(*init).lo);
            fcx.b.ret = ct;
            fcx.ret_stack.push(ct);
            fcx.in_const = true;
            // rustc_builtin_macros global_allocator: the static's methods are called as `GlobalAlloc`'s
            if d.kind == DefKind::Static && tcx.validates(prog, file) && crate::lang_items::has_attr(prog, file, ast.list(ast.item(d.item).attrs), "global_allocator") {
                if let Some(alloc) = fcx.lang_item(&["alloc", "GlobalAlloc"]) {
                    let goal = fcx.trait_goal(alloc, ct, &[]);
                    fcx.register_goal(goal, ast.expr(*init).lo);
                }
            }
            let t = fcx.check_expr(*init, Some(ct));
            fcx.coerce_or_unify(t, ct, Some(*init), ast.expr(*init).lo);
            fcx.finish();
            return fcx.into_body();
        }
        _ => return fcx.into_body(),
    };
    // rustc_hir_typeck check_fn / gather_locals: every parameter's type is Sized (SizedArgumentType),
    // unless `unsized_fn_params`; checked by `makepad check` only (the run never pays for it)
    if tcx.validates(prog, file) && !prog.crates[d.krate as usize].features(prog).has("unsized_fn_params") {
        for &p in &params { fcx.require_type_is_sized_deferred(p, ast.item(d.item).lo); }
    }
    let mut pi = 0;
    if fsig.self_param.is_some() {
        let mutable = matches!(fsig.self_param, Some(SelfParam::Value(true)) | Some(SelfParam::Typed(true, _)));
        let s = prog.syms.get("self").unwrap_or(0);
        let li = fcx.b.locals.len() as u32;
        fcx.b.locals.push(Local { name: s, ty: params[0], mutable, pat: u32::MAX });
        // `self` is hygienic like any local: a macro defined before the fn does not see it
        fcx.local_hyg.resize(li as usize + 1, (Some(Vec::new()), 0));
        let at = prog.files[file as usize].self_param_pos(ast.item(d.item).lo);
        fcx.local_hyg[li as usize] = (Some(prog.files[file as usize].hyg_ctxt(at)), at);
        fcx.scopes.push((s, li));
        pi = 1;
    }
    if fcx.tcx.validates(prog, fcx.file) {
        let param_pats: Vec<PatId> = ast.list(fsig.params).iter().map(|p| p.pat).collect();
        fcx.check_param_names(&param_pats);
    }
    // An `async fn` moves each argument whole into the coroutine, which matches the pattern there
    // (rustc_ast_lowering: `let <pat> = __arg;` first in the body). A parameter that is not a plain binding gets a
    // local for the whole argument here, and its pattern is checked in the coroutine's scope below.
    let async_fn_body = matches!((body, fsig.asyncness), (Some(_), true));
    let mut matched_in_coroutine: Vec<(PatId, TyId)> = Vec::new();
    for p in ast.list(fsig.params) {
        let t = if pi < params.len() { params[pi] } else { fcx.tcx.tys.error };
        let plain = matches!(ast.pat(p.pat), Pat::Ident { by_ref: false, sub: None, .. });
        if async_fn_body && !plain {
            let li = fcx.b.locals.len() as u32;
            fcx.b.locals.push(Local { name: prog.syms.get("_").or_else(|| prog.syms.get("self")).unwrap_or(0), ty: t, mutable: true, pat: u32::MAX - 1 });
            fcx.local_hyg.resize(li as usize + 1, (Some(Vec::new()), 0));
            fcx.local_hyg[li as usize] = (None, 0);
            fcx.b.pat_local.insert(p.pat.0 | ASYNC_ARG_KEY, li);
            matched_in_coroutine.push((p.pat, t));
        } else {
            fcx.check_pat(p.pat, t);
        }
        fcx.b.param_pats.push(p.pat);
        // fn_ctxt check_fn: every parameter is Sized (unless unsized_fn_params), judged for the
        // crates whose errors are reported
        if tcx.validates(prog, file) && !prog.crates[d.krate as usize].features(prog).has("unsized_fn_params") {
            let lo = ast.item(d.item).lo;
            fcx.require_type_is_sized_deferred(t, lo);
        }
        pi += 1;
    }
    if let (Some(bid), true) = (body, fsig.asyncness) {
        // `async fn`: the body is a coroutine owning every parameter (rustc moves them all into
        // the future); the declared return type is its output, the opaque's hidden type the
        // coroutine
        fcx.b.body = Some(bid);
        // The declared output is under the signature's binder: liberate its late-bound
        // regions as the inputs are (closure.rs:201-231 liberated_sig), then normalize.
        let out = match fcx.tcx.async_fns.get(&def) {
            Some(o) => fcx.liberated_signature_output(&sig, o, args),
            None => error,
        };
        // `async fn f() -> impl Tr`: the output's own opaque types are defined by this body too
        let out = fcx.norm(out);
        let ce = crate::tcx::ASYNC_FN_BODY | bid.0;
        // what the coroutine owns from outside: self, the plain parameters, the whole arguments of the others
        let caps: Vec<u32> = (0..fcx.b.locals.len() as u32).collect();
        let saved = fcx.scopes.len();
        fcx.closure_depth.push((ce, saved));
        // (the bindings of those patterns are the coroutine's own locals, not captures)
        for (pat, t) in matched_in_coroutine { fcx.check_pat(pat, t); }
        fcx.ret_stack.push(out);
        fcx.async_depth += 1;
        let t = fcx.check_block(bid, Some(out));
        if !fcx.diverges {
            fcx.coerce_or_unify(t, out, None, ast.block(bid).hi);
        }
        fcx.async_depth -= 1;
        fcx.ret_stack.pop();
        fcx.closure_depth.pop();
        fcx.diverges = false;
        let points = fcx.await_count;
        fcx.b.closure_locals.insert(ce, caps.clone());
        let ct = fcx.coroutine_type(ce, bid, caps, false, false, out, points);
        fcx.eq(ret, ct);
    } else if let Some(bid) = body {
        fcx.b.body = Some(bid);
        let t = fcx.check_block(bid, Some(ret));
        if !fcx.diverges {
            fcx.coerce_or_unify(t, ret, None, ast.block(bid).hi);
        } else {
            // a tail after a diverging statement still types the return (rustc coerces the
            // tail; `panic!(); MyStruct` defines `impl MyTrait` as MyStruct)
            let st = fcx.shallow(t);
            if !matches!(fcx.tcx.tys.kind(st), TyKind::Never) {
                let save = fcx.start_snapshot();
                if !fcx.try_coerce(t, ret, None) { fcx.rollback_to(save); } else { fcx.commit_from(save); }
            }
            // a body that only diverges (`fn f() -> impl Sized { todo!() }`): the return type's
            // still-open variable falls back like any variable a `!` flowed into
            let sr = fcx.shallow(ret);
            if matches!(fcx.tcx.tys.kind(sr), TyKind::Infer(_)) {
                fcx.diverging.push(sr);
            }
        }
    }
    fcx.finish();
    fcx.into_body()
}

impl<'a> Fcx<'a> {
    fn into_body(mut self) -> Body {
        if !self.b.errors.is_empty() { self.infcx.errors.note_reported_error(); }
        let (errors, warnings) = self.infcx.errors.take_since(self.infcx.diagnostic_checkpoint);
        for error in errors {
            let file = if error.cause.body_id == crate::program::NO_DEF { self.file } else { self.prog.def(error.cause.body_id).file };
            self.b.errors.push(self.prog.record_error(file, error.cause.span, "typeck", error.message));
        }
        for warning in warnings { eprintln!("warning: {}", warning.message); }
        if self.b.errors.is_empty() && self.infcx.tainted_by_errors() {
            self.b.errors.push(self.prog.record_error(self.file, self.last_lo, "typeck", "type checking failed due to previous errors".into()));
        }
        self.b.freeze();
        self.b
    }
    fn ast(&self) -> &'a Ast {
        &self.prog.files[self.file as usize].ast
    }

    fn err(&mut self, pos: u32, msg: String) {
        self.infcx.set_tainted_by_errors();
        if let Some(error) = self.infcx.errors.report_to_buffer(crate::next_solver::fulfill::ObligationCause { body_id: self.b.def, span: pos }, msg) {
            self.b.errors.push(self.prog.record_error(self.file, pos, "typeck", error.message));
        }
    }

    fn text(&self, id: Ident) -> &'a str {
        let f = &self.prog.files[self.file as usize];
        let s = std::str::from_utf8(f.bytes(id.lo, id.hi)).unwrap_or("?");
        match s.strip_prefix("r#") {
            Some(x) => x,
            None => s,
        }
    }

    fn sym(&self, id: Ident) -> Sym {
        self.prog.syms.get(self.text(id)).unwrap_or(u32::MAX)
    }

    // ------------------------------------------------------------ inference variables

    // rustc_infer/src/infer/mod.rs commit_if_ok and InferOk obligations:
    // speculative goals live with the snapshot; only commit publishes them.
    fn start_snapshot(&mut self) -> FcxSnapshot {
        let inference = self.infcx.start_snapshot();
        let local = crate::next_solver::fulfill::FulfillmentCtxt::new(&self.infcx);
        let outer = std::mem::replace(&mut self.fulfillment_cx, local);
        self.suspended_fulfillment.push(outer);
        FcxSnapshot { inference }
    }
    fn rollback_to(&mut self, snapshot: FcxSnapshot) {
        self.fulfillment_cx = self.suspended_fulfillment.pop().expect("open snapshot");
        self.infcx.rollback_to(snapshot.inference);
    }
    fn commit_from(&mut self, snapshot: FcxSnapshot) {
        let obligations = self.fulfillment_cx.pending_obligations();
        self.fulfillment_cx = self.suspended_fulfillment.pop().expect("open snapshot");
        self.infcx.commit_from(snapshot.inference);
        for obligation in obligations { self.fulfillment_cx.register_predicate_obligation(&self.infcx, obligation); }
    }

    fn fresh(&mut self, kind: u8) -> TyId {
        match kind { 0 => self.infcx.next_ty_var(), 1 => self.infcx.next_int_var(), 2 => self.infcx.next_float_var(), _ => unreachable!() }
    }

    fn shallow(&mut self, t: TyId) -> TyId {
        let t = self.infcx.shallow_resolve(t);
        if matches!(self.tcx.tys.kind(t), TyKind::Alias(_, _)) { self.normalize_body_type(t) } else { t }
    }

    fn eq(&mut self, a: TyId, b: TyId) -> bool {
        match self.infcx.at(self.prog, self.param_env.clone()).eq(a, b) {
            Ok(goals) => { for goal in goals { self.register_goal(goal, self.last_lo); } true }
            Err(_) => false,
        }
    }

    /// rustc_hir_typeck/src/coercion.rs:164-211: reject failed lazy
    /// normalization before committing a coercion, and retain unresolved goals.
    fn unify_raw(&mut self, a: TyId, b: TyId, force_leak_check: bool) -> bool {
        self.unify_coercion(a, b, false, force_leak_check).is_some()
    }

    fn unify_coercion(&mut self, a: TyId, b: TyId, use_lub: bool, force_leak_check: bool) -> Option<TyId> {
        let result = self.infcx.commit_if_ok(|snapshot| {
            let outer_universe = self.infcx.universe();
            let at = self.infcx.at(self.prog, self.param_env.clone());
            let (ty, goals) = if use_lub { at.lub(b, a)? } else { (b, at.sub(a, b)?) };
            let mut engine = crate::next_solver::fulfill::FulfillmentCtxt::new(&self.infcx);
            for goal in goals { engine.register_predicate_obligation(&self.infcx, crate::next_solver::fulfill::PredicateObligation { goal, recursion_depth: 0, cause: crate::next_solver::fulfill::ObligationCause { span: self.last_lo, body_id: self.b.def } }); }
            if !engine.try_evaluate_obligations(&self.infcx, self.prog).is_empty() { return Err(crate::infer::relate::TypeError::Mismatch); }
            // coercion.rs:164-211,1034-1055: only fn-item/pointer coercions
            // force the extra leak check; other relations keep their own checks.
            if force_leak_check { self.infcx.leak_check(outer_universe, Some(snapshot))?; }
            Ok((ty, engine.pending_obligations()))
        });
        match result {
            Ok((ty, obligations)) => { for obligation in obligations { self.fulfillment_cx.register_predicate_obligation(&self.infcx, obligation); } Some(ty) },
            Err(_) => None,
        }
    }

    fn has_open_infer(&mut self, t: TyId) -> bool {
        self.tcx.tys.has_infer(self.infcx.deeply_resolve_ignoring_regions(t))
    }

    /// `&T -> &dyn Tr`, `*T -> *dyn Tr`, `P<T> -> P<dyn Tr>` when T implements Tr.
    /// Array-to-slice unsizing (rustc coerce_unsized with `[T; N]: Unsize<[T]>`): `&[T; N]` ->
    /// `&[T]` / `*const [T]`, `*mut [T; N]` -> `*mut [T]`, and smart pointers (`Box<[T; N]>` ->
    /// `Box<[T]>`, Rc, Arc, ..; same shape as `dyn_coercion`). Inference variables are only bound
    /// when the coercion applies.
    /// ADT coercions and unsized tails require structs (rustc_hir_analysis/src/coherence/
    /// builtin.rs:870-871); enums/unions must relate their generic arguments normally.
    fn unsize_coercion(&mut self, ka: &TyKind, kx: &TyKind) -> bool {
        let save = self.start_snapshot();
        let ok = match (ka, kx) {
            (TyKind::Ref(m1, a, _), TyKind::Ref(m2, d, _)) | (TyKind::Ref(m1, a, _), TyKind::Ptr(m2, d)) | (TyKind::Ptr(m1, a), TyKind::Ptr(m2, d)) if *m1 || !*m2 => self.array_to_slice(*a, *d) || self.struct_unsize_goal(*a, *d),
            (TyKind::Adt(d1, xs, _), TyKind::Adt(d2, ys, _)) if d1 == d2 && self.prog.def(*d1).kind == DefKind::Struct && self.tcx.tys.list(*xs).len() == self.tcx.tys.list(*ys).len() => {
                let (xs, ys) = (self.tcx.tys.list(*xs), self.tcx.tys.list(*ys));
                let mut found = None;
                for i in 0..xs.len() {
                    let (x, y) = (self.shallow(xs[i]), self.shallow(ys[i]));
                    // `Box<[T; N]>` -> `Box<[T]>`, also through a struct tail (`Box<S<[T; N]>>`
                    // -> `Box<S<[T]>>`)
                    if matches!(self.tcx.tys.kind(x), TyKind::Array(..)) && matches!(self.tcx.tys.kind(y), TyKind::Slice(..))
                        || x != y && matches!((self.tcx.tys.kind(x), self.tcx.tys.kind(y)), (TyKind::Adt(a1, _, _), TyKind::Adt(a2, _, _)) if a1 == a2) && self.has_slice(y)
                    {
                        found = Some(i);
                    }
                }
                match found {
                    Some(i) => (0..xs.len()).all(|j| j == i || self.eq(xs[j], ys[j])) && self.array_to_slice(xs[i], ys[i]),
                    None => false,
                }
            }
            _ => false,
        };
        if !ok { self.rollback_to(save); } else { self.commit_from(save); }
        ok
    }

    /// A struct to the same struct with other arguments (`Foo<i32, Count<N>>` -> `Foo<u32, ()>`): the
    /// solver's `S<..>: Unsize<S<..>>` (the last-field rule, rustc coerce_unsized).
    fn struct_unsize_goal(&mut self, a: TyId, d: TyId) -> bool {
        let (a, d) = (self.shallow(a), self.shallow(d));
        let (TyKind::Adt(d1, xs, _), TyKind::Adt(d2, ys, _)) = (self.tcx.tys.kind(a).clone(), self.tcx.tys.kind(d).clone()) else { return false };
        if d1 != d2 || a == d || self.prog.def(d1).kind != DefKind::Struct || self.tcx.tys.list(xs).is_empty() || self.tcx.tys.list(xs).len() != self.tcx.tys.list(ys).len() { return false; }
        let Some(unsize) = self.tcx.lang_items.get(crate::lang_items::LangItem::Unsize) else { return false };
        // types that can be equal are related, not unsized (`W<?x>` -> `W<T>`)
        let probe = self.start_snapshot();
        let same = self.eq(a, d);
        self.rollback_to(probe);
        if same { return false; }
        matches!(self.select_trait(unsize, a, &[d]), Ok(Some(_)))
    }

    /// `[T; N]` -> `[T]`, also as the tail of a struct (`Holder<[T; N]>` -> `Holder<[T]>`, rustc
    /// Unsize for structs whose last field's type is the only one mentioning the parameter).
    fn array_to_slice(&mut self, a: TyId, d: TyId) -> bool {
        let (a, d) = (self.shallow(a), self.shallow(d));
        match (self.tcx.tys.kind(a).clone(), self.tcx.tys.kind(d).clone()) {
            (TyKind::Array(ea, _), TyKind::Slice(es)) => self.eq(ea, es),
            (TyKind::Adt(d1, xs, _), TyKind::Adt(d2, ys, _)) if d1 == d2 && self.prog.def(d1).kind == DefKind::Struct && self.tcx.tys.list(xs).len() == self.tcx.tys.list(ys).len() && !xs.is_empty() => {
                let (xs, ys) = (self.tcx.tys.list(xs), self.tcx.tys.list(ys));
                let i = match (0..xs.len()).find(|&i| {
                    let (x, y) = (self.shallow(xs[i]), self.shallow(ys[i]));
                    x != y && !matches!(self.tcx.tys.kind(y), TyKind::Infer(_))
                }) {
                    Some(i) => i,
                    None => return false,
                };
                // the solver's last-field rule: the parameter may not be mentioned by another
                // field (`Foo<T: ?Sized>(Box<T>, T)` does not unsize `Foo<[u8; 1]>`)
                if !matches!(self.tcx.lang_items.get(crate::lang_items::LangItem::Unsize), Some(unsize) if matches!(self.select_trait(unsize, a, &[d]), Ok(Some(_)))) { return false; }
                (0..xs.len()).all(|j| j == i || self.eq(xs[j], ys[j])) && self.array_to_slice(xs[i], ys[i])
            }
            _ => false,
        }
    }

    /// coercion.rs type_var_is_sized: a `$0: Sized` obligation is pending. Only then is `$0: Unsize<dyn
    /// Trait>` pending too; a bare variable (`ptr as *mut _`) is unified with the pointee instead.
    fn type_var_is_sized(&mut self, var: TyId) -> bool {
        let Some(sized) = self.tcx.lang_items.get(crate::lang_items::LangItem::Sized) else { return true };
        let v = self.shallow(var);
        std::iter::once(&self.fulfillment_cx).chain(self.suspended_fulfillment.iter()).any(|cx| cx.pending_obligations().iter().any(|o| {
            matches!(&o.goal.predicate.kind, PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(t))
                if t.trait_ref.def_id == sized && matches!(t.trait_ref.args.first(), Some(crate::consts::GenericArg::Type(a)) if self.infcx.shallow_resolve(*a) == v))
        }))
    }

    /// Is item `d` visible from this body's module (`pub`, or private and defined in this module
    /// or one of its ancestors)?
    fn item_visible(&self, d: DefId) -> bool {
        let def = self.prog.def(d);
        // (another crate's private item is never visible: its home module is not an ancestor here;
        // rustc probe.rs keeps such candidates for diagnostics only, so an accessible trait item wins)
        if def.is_pub {
            // `pub`, `pub(super)`, `pub(in path)`: inside that module only
            let vis = &self.prog.files[def.file as usize].ast.item(def.item).vis;
            // (no written visibility on a pub def: inherited -- a trait's items, a variant)
            return match vis {
                crate::ast::Vis::Pub | crate::ast::Vis::Private => true,
                _ => match self.prog.vis_module(def.file, def.scope, vis) {
                    Some(m) => self.prog.mod_within(self.module, m),
                    None => true,
                },
            };
        }
        let home = self.prog.mods[def.scope as usize].normal;
        let mut m = Some(self.prog.mods[self.module as usize].normal);
        let mut guard = 0;
        while let Some(x) = m {
            if x == home {
                return true;
            }
            guard += 1;
            if guard > 64 {
                break;
            }
            m = self.prog.mods[x as usize].parent_mod.map(|p| self.prog.mods[p as usize].normal);
        }
        false
    }

    /// rustc_hir_typeck/src/coercion.rs:1134-1164: a failed coercion returns
    /// failure without reporting. Only the demanding caller emits a diagnostic.
    fn try_coerce(&mut self, a: TyId, x: TyId, e: Option<ExprId>) -> bool {
        self.try_coerce_inner(a, x, e, self.last_lo, false).is_some()
    }
    fn try_coerce_inner(&mut self, a: TyId, x: TyId, e: Option<ExprId>, pos: u32, use_lub: bool) -> Option<TyId> {
        let snap = self.start_snapshot();
        let n_dd = self.deferred_dyn.len();
        let old = e.map(|e| self.save_value_adjustments(e.0));
        if let Some(target) = self.coerce(a, x, e, pos, use_lub) {
            self.commit_from(snap);
            return Some(target);
        }
        self.rollback_to(snap);
        self.deferred_dyn.truncate(n_dd);
        if let (Some(e), Some(old)) = (e, old) { self.restore_value_adjustments(e.0, old); }
        None
    }

    fn is_literal_var(&self, t: TyId) -> bool {
        match self.tcx.tys.kind(t) {
            TyKind::IntVar(_) | TyKind::FloatVar(_) => true,
            _ => false,
        }
    }

    /// The fn pointer type a fn item / capture-free closure / fn pointer coerces to.
    fn fn_ptr_of(&mut self, t: TyId) -> Option<TyId> {
        let t = self.shallow(t);
        match self.tcx.tys.kind(t).clone() {
            TyKind::FnPtr(..) => Some(t),
            TyKind::FnDef(d, args, regions) => {
                let sig = self.tcx.fn_item_sig(self.prog, d).cloned()?;
                // coercion.rs:1259-1264: intrinsics have no addressable body.
                if sig.header.abi == Abi::RelaxIntrinsic || sig.rustc_intrinsic { return None; }
                // coercion.rs:1270-1282 sig_for_fn_def_coercion: a safe #[target_feature] fn is a safe
                // signature where the body has its features
                let mut header = sig.header;
                if sig.safe_target_features && self.tcx.adjust_target_feature_sig(self.prog, d, self.b.def) {
                    header.safety = Safety::Safe;
                }
                let poly = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&sig.params), sig.ret, header, sig.bound_regions));
                let args = self.tcx.complete_args(self.prog, d, self.tcx.tys.list(args), regions);
                Some(self.tcx.instantiate_ty(self.prog, d, &args, poly))
            }
            TyKind::Closure(file, ce, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => {
                // rustc sig_for_closure_coercion: capture inference has not run
                // yet, so the upvar tuple is still an inference variable. Use
                // mentioned roots, as for coercion to an expected fn pointer.
                let captures = file != self.file || self.b.closure_locals.get(&ce).is_some_and(|v| !v.is_empty());
                if captures {
                    None
                } else {
                    Some(sig)
                }
            }
            _ => None,
        }
    }

    /// rustc_hir_typeck/src/coercion.rs:1328-1493: all arms contribute to
    /// the lattice result; callable items reify only if their own LUB fails.
    fn coerce_many(&mut self, prev: &[ExprId], prev_ty: TyId, new_e: ExprId, new_ty: TyId, lo: u32) -> TyId {
        match self.try_find_coercion_lub(prev, prev_ty, new_e, new_ty) {
            Some(ty) => ty,
            None => {
                let expected = self.ty_str(prev_ty);
                let found = self.ty_str(new_ty);
                self.err(lo, format!("E0308: mismatched types: expected `{expected}`, found `{found}`"));
                self.tcx.tys.error
            }
        }
    }

    fn try_find_coercion_lub(&mut self, prev: &[ExprId], prev_ty: TyId, new_e: ExprId, new_ty: TyId) -> Option<TyId> {
        self.retry_goals(false);
        let prev_ty = self.infcx.deeply_resolve_ignoring_regions(prev_ty);
        let new_ty = self.infcx.deeply_resolve_ignoring_regions(new_ty);
        if prev_ty == new_ty { return Some(prev_ty); }
        let pair = (self.tcx.tys.kind(prev_ty).clone(), self.tcx.tys.kind(new_ty).clone());
        let callable_pair = match pair {
            (TyKind::FnDef(..), TyKind::FnDef(..)) | (TyKind::Closure(..), TyKind::Closure(..)) => {
                if let Some(ty) = self.unify_coercion(prev_ty, new_ty, true, true) { return Some(ty); }
                true
            }
            (TyKind::FnDef(..), TyKind::Closure(..)) | (TyKind::Closure(..), TyKind::FnDef(..)) => true,
            _ => false,
        };
        if callable_pair {
            let mut a = self.fn_ptr_of(prev_ty)?;
            let mut b = self.fn_ptr_of(new_ty)?;
            let unsafe_ = matches!(self.tcx.tys.kind(a), TyKind::FnPtr(_, _, h, _) if h.safety == Safety::Unsafe)
                || matches!(self.tcx.tys.kind(b), TyKind::FnPtr(_, _, h, _) if h.safety == Safety::Unsafe);
            if unsafe_ {
                for ty in [&mut a, &mut b] {
                    let TyKind::FnPtr(ps, ret, header, bound) = self.tcx.tys.kind(*ty).clone() else { unreachable!() };
                    *ty = self.tcx.tys.intern(TyKind::FnPtr(ps, ret, FnHeader { safety: Safety::Unsafe, ..header }, bound));
                }
            }
            a = self.norm(a);
            b = self.norm(b);
            let ty = self.unify_coercion(a, b, true, false)?;
            let adjustment = if matches!(self.tcx.tys.kind(prev_ty), TyKind::Closure(..)) { Coerce::ClosureFn } else { Coerce::ReifyFn };
            for e in prev { self.set_coerce(e.0, adjustment, ty); }
            self.set_coerce(new_e.0, if matches!(self.tcx.tys.kind(new_ty), TyKind::Closure(..)) { Coerce::ClosureFn } else { Coerce::ReifyFn }, ty);
            return Some(ty);
        }
        if !self.b.coerce(new_e.0).is_some() {
            if let Some(ty) = self.try_coerce_inner(new_ty, prev_ty, Some(new_e), self.last_lo, true) { return Some(ty); }
        }
        // A single coercion determines the adjustments for all previous arms.
        // Copy the overloaded deref calls too: lowering looks them up per expression.
        let first = prev.first().copied();
        let ty = self.try_coerce_inner(prev_ty, new_ty, first, self.last_lo, true)?;
        if let Some(first) = first {
            for e in &prev[1..] { self.copy_value_adjustments(first.0, e.0); }
        }
        Some(ty)
    }

    /// Trait object upcasting (stable since 1.86), `dyn Tr + Auto` -> `dyn Tr`, `dyn Tr` -> `dyn Auto`:
    /// the pointee pair is unsized by the solver's `dyn A: Unsize<dyn B>` candidates (rustc
    /// consider_builtin_dyn_upcast_candidates); pointer shapes as for `dyn_coercion`.
    fn dyn_upcast(&mut self, ka: &TyKind, kx: &TyKind) -> Option<(TyId, TyId)> {
        let (a, d, outer_a, outer_d) = self.unsize_pointee_pair(ka, kx, true)?;
        let (a, d) = (self.shallow(a), self.shallow(d));
        if a == d || !matches!(self.tcx.tys.kind(a), TyKind::Dyn(..)) || !matches!(self.tcx.tys.kind(d), TyKind::Dyn(..)) { return None; }
        // the same object type up to lifetimes is subtyping, not an upcast
        // (a pointer keeps the coercion: extending a trait object's lifetime must prove `'a: 'b`
        // through the Unsize goal; a smart pointer's clone of the same type is plain subtyping)
        if self.tcx.tys.erase_regions(a) == self.tcx.tys.erase_regions(d) && !matches!(ka, TyKind::Ref(..) | TyKind::Ptr(..)) { return None; }
        let unsize = self.tcx.lang_items.get(crate::lang_items::LangItem::Unsize)?;
        // through a struct tail the whole pointee pair must unsize (rustc: `S<dyn A>: Unsize<S<dyn B>>`,
        // the last-field rule), not just the trait objects at its end
        let (oa, od) = (self.shallow(outer_a), self.shallow(outer_d));
        if oa != a { self.select_trait(unsize, oa, &[od]).ok()?; }
        self.select_trait(unsize, a, &[d]).ok()?;
        Some((a, d))
    }

    /// The pointee pair a pointer coercion unsizes: `&A` / `*A` / `P<..A..>` against `&B` / `*B` /
    /// `P<..B..>`, then down a struct tail (`S<A>` against `S<B>`, rustc coerce_unsized: `S<T>:
    /// Unsize<S<U>>` when the last field's type does) to the types that become / are trait objects.
    /// `upcast`: the source pointee is itself a trait object (dyn -> dyn).
    fn unsize_pointee_pair(&mut self, ka: &TyKind, kx: &TyKind, upcast: bool) -> Option<(TyId, TyId, TyId, TyId)> {
        let (src, dy) = match (ka, kx) {
            (TyKind::Ref(m1, a, _), TyKind::Ref(m2, d, _)) | (TyKind::Ref(m1, a, _), TyKind::Ptr(m2, d)) | (TyKind::Ptr(m1, a), TyKind::Ptr(m2, d)) if *m1 || !*m2 => (*a, *d),
            (TyKind::Adt(d1, xs, _), TyKind::Adt(d2, ys, _)) if d1 == d2 && self.prog.def(*d1).kind == DefKind::Struct && self.tcx.tys.list(*xs).len() == self.tcx.tys.list(*ys).len() => {
                let (xs, ys) = (self.tcx.tys.list(*xs), self.tcx.tys.list(*ys));
                let mut found = None;
                for i in 0..xs.len() {
                    let y = self.shallow(ys[i]);
                    let x = self.shallow(xs[i]);
                    // a literal pointee (`Box::new(1)` as `Box<dyn Any>`) is decided below: the one
                    // numeric impl, or the literal fallback's
                    let open = match self.tcx.tys.kind(x) {
                        TyKind::Infer(_) => true,
                        TyKind::Dyn(..) => !upcast,
                        _ => false,
                    };
                    if x != y && self.has_dyn(y) && !open {
                        found = Some(i);
                    }
                }
                let i = found?;
                for j in 0..xs.len() {
                    if j != i && !self.eq(xs[j], ys[j]) {
                        return None;
                    }
                }
                (xs[i], ys[i])
            }
            _ => return None,
        };
        // `P<S<..T..>>` -> `P<S<..dyn Tr..>>` (also `&S<T>` -> `&S<dyn Tr>`): struct tail
        // unsizing (rustc coerce_unsized: `S<T>: Unsize<S<U>>` when the last field's type does),
        // down to the type that becomes the trait object
        let (outer_src, outer_dy) = (src, dy);
        let (src, dy) = {
                let (mut x, mut y) = (self.shallow(src), self.shallow(dy));
                loop {
                    if matches!(self.tcx.tys.kind(y), TyKind::Dyn(..)) {
                        break (x, y);
                    }
                    match (self.tcx.tys.kind(x).clone(), self.tcx.tys.kind(y).clone()) {
                        (TyKind::Adt(a1, xa, _), TyKind::Adt(a2, ya, _)) if a1 == a2 && self.tcx.tys.list(xa).len() == self.tcx.tys.list(ya).len() && self.tcx.adts.get(&a1).map_or(false, |ad| !ad.is_enum && !ad.is_union) => {
                            let (xa, ya) = (self.tcx.tys.list(xa), self.tcx.tys.list(ya));
                            let mut k = None;
                            for j in 0..xa.len() {
                                let yj = self.shallow(ya[j]);
                                if self.has_dyn(yj) && self.shallow(xa[j]) != yj {
                                    k = Some(j);
                                } else if !self.eq(xa[j], ya[j]) {
                                    return None;
                                }
                            }
                            let k = k?;
                            x = self.shallow(xa[k]);
                            y = self.shallow(ya[k]);
                        }
                        _ => return None,
                    }
                }
        };
        Some((src, dy, outer_src, outer_dy))
    }

    fn dyn_coercion(&mut self, ka: &TyKind, kx: &TyKind) -> Option<(TyId, TyId)> {
        let (src, dy, _, _) = self.unsize_pointee_pair(ka, kx, false)?;
        let src = self.shallow(src);
        let dy = self.shallow(dy);
        // An unresolved pointee may itself become `dyn Tr` (for example the `_`
        // in `object_ref as *const _`). Let the deferred coercion decide after
        // the cast resolves it; selecting Unsize now can unify it with `dy` and
        // incorrectly record a thin-to-fat conversion for an already-fat value.
        if !matches!(self.tcx.tys.kind(dy), TyKind::Dyn(..)) || matches!(self.tcx.tys.kind(src), TyKind::Dyn(..) | TyKind::Infer(_)) { return None; }
        // coercion.rs:683-698: Unsize selection retains the object's binders,
        // associated-type equalities and outlives obligations together.
        let unsize = self.tcx.lang_items.get(crate::lang_items::LangItem::Unsize)?;
        if self.select_trait(unsize, src, &[dy]).is_err() { return None; }
        Some((src, dy))
    }

    /// method/probe.rs:assemble_inherent_candidates_from_param: read the
    /// native ParamEnv and an alias's instantiated item bounds, never re-lower AST bounds.
    fn param_bounds(&mut self, t: TyId) -> Vec<(DefId, Vec<TyId>)> {
        let mut clauses = self.tcx.param_env_by_id(self.param_env).clauses.to_vec();
        clauses.extend(self.alias_bound_clauses(t));
        self.trait_bounds_for_self(t, &clauses)
    }

    fn trait_bounds_for_self(&self, t: TyId, clauses: &[Predicate]) -> Vec<(DefId, Vec<TyId>)> {
        let mut out = Vec::new();
        for p in clauses {
            let PredicateKind::Clause(ClauseKind::Trait(c)) = &p.kind else { continue };
            let Some(crate::consts::GenericArg::Type(self_ty)) = c.trait_ref.args.first() else { continue };
            if !self.tcx.tys.eq_mod_regions(*self_ty, t) { continue; }
            let mut args = Vec::new();
            for a in &c.trait_ref.args[1..] {
                match a {
                    crate::consts::GenericArg::Type(t) => args.push(*t),
                    crate::consts::GenericArg::Const(c) => args.push(self.tcx.tys.intern(TyKind::Const(*c))),
                    crate::consts::GenericArg::Lifetime(_) => {}
                }
            }
            let entry = (c.trait_ref.def_id, args);
            if !out.contains(&entry) { out.push(entry); }
        }
        out
    }

    /// Is `t` dynamically sized (str, slice, dyn, or a struct whose last field is)?
    fn is_unsized_ty(&mut self, t: TyId) -> bool {
        let t = self.shallow(t);
        match self.tcx.tys.kind(t).clone() {
            TyKind::Dyn(..) | TyKind::Str | TyKind::Slice(_) => true,
            TyKind::Adt(d, args, _) => {
                let last = match self.tcx.adts.get(&d) {
                    Some(a) if !a.is_enum && !a.is_union => a.variants.first().and_then(|v| v.fields.last()).map(|f| f.ty),
                    _ => None,
                };
                // `struct Foo<T: A>(T::Assoc)` with `type Assoc = Foo<()>`: the tail is the struct
                // itself -- the layout query cycle (E0391)
                let Ok(_cycle) = crate::cycle::enter(crate::cycle::Kind::UnsizedTail, t.0, 1) else {
                    let ts = self.ty_str(t);
                    self.err(self.last_lo, format!("E0391: cycle detected when computing layout of `{}`", ts));
                    return false;
                };
                match last {
                    Some(ft) => {
                        let ft = self.tcx.tys.subst(ft, self.tcx.tys.list(args));
                        self.is_unsized_ty(ft)
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Does `t` contain a trait object type (as a generic argument, transitively)?
    fn has_dyn(&mut self, t: TyId) -> bool {
        let t = self.shallow(t);
        match self.tcx.tys.kind(t).clone() {
            TyKind::Dyn(..) => true,
            TyKind::Adt(_, v, _) => self.tcx.tys.list(v).iter().any(|x| self.has_dyn(*x)),
            _ => false,
        }
    }

    fn has_slice(&mut self, t: TyId) -> bool {
        let t = self.shallow(t);
        match self.tcx.tys.kind(t).clone() {
            TyKind::Slice(..) => true,
            TyKind::Adt(_, v, _) => self.tcx.tys.list(v).iter().any(|x| self.has_slice(*x)),
            _ => false,
        }
    }

    /// `&u8` -> `u8` when the referent is a primitive number/bool/char; other types unchanged
    /// (core implements the operators for `T` and `&T` operands, not for `&&T`).
    fn peel_prim_refs(&mut self, t: TyId) -> TyId {
        let mut cur = self.shallow(t);
        let mut n = 0;
        if let TyKind::Ref(_, inner, _) = self.tcx.tys.kind(cur).clone() {
            cur = self.shallow(inner);
            n += 1;
        }
        if n > 0 && (self.is_numeric(cur) || matches!(self.tcx.tys.kind(cur), TyKind::Bool | TyKind::Char)) {
            cur
        } else {
            t
        }
    }


    /// Autoderef steps from `a` to `x` (both pointees of references), if any.
    fn deref_coercion(&mut self, a: TyId, x: TyId) -> Option<Vec<(Option<adjustments::OverloadedDeref>, TyId)>> {
        let xs = self.shallow(x);
        // an open target takes the source as is; an integer / float variable is no reference,
        // so `&&{integer}` to `&{integer}` derefs (rustc coerce_borrowed_pointer autoderefs
        // until the pointees unify)
        if matches!(self.tcx.tys.kind(xs), TyKind::Infer(_)) && !self.is_literal_var(xs) {
            return None;
        }
        let mut cur = self.shallow(a);
        let mut derefs = Vec::new();
        for _ in 0..8usize {
            let (next, overloaded) = self.deref_step_raw(cur)?;
            cur = next;
            derefs.push((overloaded, cur));
            let save = self.start_snapshot();
            if self.eq(cur, xs) {
                self.commit_from(save);
                return Some(derefs);
            }
            self.rollback_to(save);
        }
        None
    }

    pub fn is_fn_trait(&self, td: DefId) -> bool {
        let d = self.prog.def(td);
        Some(d.krate) == self.prog.prelude_crate && matches!(self.prog.name(td), "Fn" | "FnMut" | "FnOnce")
    }

    fn ty_str(&mut self, t: TyId) -> String {
        let t = self.infcx.deeply_resolve_ignoring_regions(t);
        ty_to_string(false, self.prog, self.tcx, t)
    }

    /// rustc_hir_typeck demand_eqtype / demand_suptype: the types are the same, or E0308.
    fn demand_eq(&mut self, expected: TyId, actual: TyId, pos: u32) -> bool {
        if self.eq(expected, actual) { return true; }
        let (x, a) = (self.infcx.deeply_resolve_ignoring_regions(expected), self.infcx.deeply_resolve_ignoring_regions(actual));
        if self.tcx.tys.has_error(x) || self.tcx.tys.has_error(a) { return false; }
        let (sx, sa) = (ty_to_string(false, self.prog, self.tcx, x), ty_to_string(false, self.prog, self.tcx, a));
        let (sx, sa) = if sx == sa { (ty_to_string_paths(self.prog, self.tcx, x), ty_to_string_paths(self.prog, self.tcx, a)) } else { (sx, sa) };
        self.err(pos, format!("E0308: mismatched types: expected `{}`, found `{}`", sx, sa));
        false
    }

    /// rustc_hir_typeck expr.rs check_lhs_assignable: the left side of an assignment is a place expression
    /// (a local or static, a field, an index, a dereference); else E0070 (E0067 for `op=`).
    fn check_lhs_assignable(&mut self, a: ExprId, compound: bool) {
        if !self.tcx.validates(self.prog, self.file) { return; }
        let mut e = a;
        loop {
            match self.ast().expr(e).kind {
                ExprKind::Paren(i) => e = i,
                _ => break,
            }
        }
        let ok = match &self.ast().expr(e).kind {
            ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) | ExprKind::Unary(UnOp::Deref, _) => true,
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                Some(Res::Local(_)) | Some(Res::Err) | None => true,
                // (a unit struct or variant is a destructuring-assignment pattern)
                Some(Res::Def(d)) => matches!(self.prog.def(*d).kind, DefKind::Static | DefKind::ForeignStatic | DefKind::Struct | DefKind::Variant),
                Some(_) => false,
            },
            ExprKind::Underscore => true,
            _ => false,
        };
        if !ok {
            let pos = self.ast().expr(a).lo;
            if compound { self.err(pos, "E0067: invalid left-hand side of compound assignment".to_string()); }
            else { self.err(pos, "E0070: invalid left-hand side of assignment".to_string()); }
        }
    }

    /// A `&`/`&mut` pattern against a scrutinee of another shape: E0308.
    fn demand_pat_eq(&mut self, scrutinee: TyId, m: bool) {
        let x = self.infcx.deeply_resolve_ignoring_regions(scrutinee);
        let sx = ty_to_string(false, self.prog, self.tcx, x);
        self.err(self.last_lo, format!("E0308: mismatched types: expected `{}`, found `&{}_`", sx, if m { "mut " } else { "" }));
    }

    /// rustc_hir_typeck/src/demand.rs:230-270: report only after the coercion
    /// is required and failed; speculative coercions never emit and retract errors.
    fn coerce_or_unify(&mut self, actual: TyId, expected: TyId, e: Option<ExprId>, pos: u32) -> bool {
        if self.try_coerce_inner(actual, expected, e, pos, false).is_some() { return true; }
        let (a, x) = (self.shallow(actual), self.shallow(expected));
        let sa = self.ty_str(a);
        let sx = self.ty_str(x);
        // same printed name for different types: show where each is defined
        let (sx, sa) = if sx == sa {
            let (x, a2) = (self.infcx.deeply_resolve_ignoring_regions(expected), self.infcx.deeply_resolve_ignoring_regions(actual));
            (ty_to_string_paths(self.prog, self.tcx, x), ty_to_string_paths(self.prog, self.tcx, a2))
        } else {
            (sx, sa)
        };
        self.err(pos, format!("E0308: mismatched types: expected `{}`, found `{}`", sx, sa));
        false
    }

    fn coerce(&mut self, actual: TyId, expected: TyId, e: Option<ExprId>, pos: u32, use_lub: bool) -> Option<TyId> {
        let a = self.shallow(actual);
        let x = self.shallow(expected);
        if a == x {
            if let (Some(e), TyKind::Ref(true, _, _)) = (e, self.tcx.tys.kind(x)) {
                self.set_reborrow(e.0, true, x);
            }
            return Some(x);
        }
        let ka = self.tcx.tys.kind(a).clone();
        let kx = self.tcx.tys.kind(x).clone();
        if let TyKind::Never = ka {
            if let Some(e) = e {
                self.set_coerce(e.0, Coerce::Never, x);
            }
            if matches!(kx, TyKind::Infer(_)) {
                self.diverging.push(x);
            }
            return Some(x);
        }
        // coercion.rs coerce: unsizing is tried first
        if let Some(kind) = self.coerce_unsized(a, x) {
            if let Some(e) = e {
                if let (TyKind::Ref(..), TyKind::Ref(m, _, _) | TyKind::Ptr(m, _)) = (&ka, &kx) { let b = self.borrow_target(a, *m); self.set_reborrow(e.0, *m, b); }
                match kind {
                    coerce_unsized::UnsizeKind::ToDyn(src, dy) => { self.set_coerce(e.0, Coerce::ToDyn(src, dy), x); }
                    coerce_unsized::UnsizeKind::Upcast(sd, dd) => { self.set_coerce(e.0, Coerce::Upcast(sd, dd), x); }
                    coerce_unsized::UnsizeKind::Deferred => self.deferred_dyn.push((e, a, x, pos)),
                    coerce_unsized::UnsizeKind::Unsize => { self.set_coerce(e.0, Coerce::Unsize, x); }
                }
            }
            return Some(x);
        }
        if let (TyKind::Ref(m1, ia, region), TyKind::Ref(m2, ix, _), Some(ex)) = (&ka, &kx, e) {
            if *m1 || !*m2 {
                let save = self.start_snapshot();
                let reborrow = self.tcx.tys.intern(TyKind::Ref(*m2, *ia, *region));
                if let Some(target) = self.unify_coercion(reborrow, x, use_lub, false) {
                    self.commit_from(save);
                    if *m1 { self.set_reborrow(ex.0, *m2, reborrow); }
                    return Some(target);
                }
                self.rollback_to(save);
                if let Some(derefs) = self.deref_coercion(*ia, *ix) {
                    self.set_deref_ref(ex.0, derefs, Some((*m2, x)));
                    return Some(x);
                }
            }
        }
        match (&ka, &kx) {
            (TyKind::FnDef(d, _, _), TyKind::FnPtr(_, _, target, _)) => {
                if let Some(pointer) = self.fn_ptr_of(a) {
                    let TyKind::FnPtr(ps, ret, mut header, bound) = self.tcx.tys.kind(pointer).clone() else { unreachable!() };
                    // coercion.rs:1270-1282: a safe #[target_feature] fn is a safe pointer only
                    // where the body has its features (else TargetFeatureCast for a safe target)
                    if self.tcx.fn_item_sig(self.prog, *d).is_some_and(|s| s.safe_target_features)
                        && self.tcx.adjust_target_feature_sig(self.prog, *d, self.b.def) {
                        header.safety = Safety::Safe;
                    }
                    if header.can_coerce_to(*target) {
                        let pointer = self.tcx.tys.intern(TyKind::FnPtr(ps, ret, *target, bound));
                        if let Some(target) = self.unify_coercion(pointer, x, use_lub, true) {
                            if let Some(e) = e { self.set_coerce(e.0, Coerce::ReifyFn, target); }
                            return Some(target);
                        }
                    }
                }
            }
            (TyKind::FnPtr(ps, r, header, bound), TyKind::FnPtr(_, _, target, _))
                if header.safety == Safety::Safe && target.safety == Safety::Unsafe => {
                let unsafe_ty = self.tcx.tys.intern(TyKind::FnPtr(ps.clone(), *r,
                    FnHeader { safety: Safety::Unsafe, ..*header }, *bound));
                if let Some(target) = self.unify_coercion(unsafe_ty, x, use_lub, true) {
                    if let Some(e) = e { self.set_coerce(e.0, Coerce::UnsafeFn, target); }
                    return Some(target);
                }
            }
            (TyKind::Closure(file, ce, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }), TyKind::FnPtr(_, _, target, _)) => {
                // coercion.rs:1309: before capture inference, use mentioned
                // roots to decide whether function-pointer coercion is possible.
                let captures = *file != self.file || self.b.closure_locals.get(ce).is_some_and(|v| !v.is_empty());
                let pointer = match self.tcx.tys.kind(*sig).clone() {
                    TyKind::FnPtr(ps, r, header, bound) if header.can_coerce_to(*target) =>
                        self.tcx.tys.intern(TyKind::FnPtr(ps, r, *target, bound)),
                    _ => *sig,
                };
                if !captures { if let Some(target) = self.unify_coercion(pointer, x, use_lub, false) {
                    if let Some(e) = e {
                        self.set_coerce(e.0, Coerce::ClosureFn, target);
                    }
                    return Some(target);
                } }
            }
            (TyKind::Ref(m1, ia, _), TyKind::Ref(m2, ix, _)) => {
                let ia = self.shallow(*ia);
                let ix = self.shallow(*ix);
                if *m1 && !*m2 && self.unify_raw(ia, ix, false) {
                    if let Some(e) = e {
                        self.set_coerce(e.0, Coerce::MutToConst, x);
                    }
                    return Some(x);
                }
            }
            // `&mut T -> *mut T`, `&T -> *const T`, `&mut T -> *const T` (same bits)
            (TyKind::Ref(m1, ia, _), TyKind::Ptr(m2, _)) if *m1 || !*m2 => {
                let ia = *ia;
                let source = self.tcx.tys.intern(TyKind::Ptr(*m2, ia));
                if let Some(target) = self.unify_coercion(source, x, use_lub, false) {
                    if let Some(e) = e { self.set_coerce(e.0, Coerce::RefToRaw(*m2), target); }
                    return Some(target);
                }
            }
            (TyKind::Ptr(true, ia), TyKind::Ptr(false, ix)) => {
                let ia = *ia;
                if self.unify_raw(ia, *ix, false) {
                    if let Some(e) = e {
                        self.set_coerce(e.0, Coerce::MutToConst, x);
                    }
                    return Some(x);
                }
            }
            _ => {}
        }
        if let Some(target) = self.unify_coercion(actual, expected, use_lub, false) {
            return Some(target);
        }
        None
    }

    /// Casts use coercion adjustments too (rustc cast.rs: try_coercion_cast).
    /// Record their borrow before capture analysis; MIR lowering reads this same
    /// adjustment instead of independently deciding whether a cast moves a reference.
    fn record_cast_borrow(&mut self, e: ExprId, source: TyId, target: TyId) {
        let (source, target) = (self.shallow(source), self.shallow(target));
        let TyKind::Ref(sm, inner, _) = self.tcx.tys.kind(source).clone() else { return };
        match self.tcx.tys.kind(target).clone() {
            TyKind::Ptr(tm, pointee) if sm || !tm => {
                if self.b.coerce(e.0).is_some() {
                    // Unsizing / trait-object coercions borrow before the pointer conversion.
                    let b = self.borrow_target(source, tm);
                    self.set_reborrow(e.0, tm, b);
                } else {
                    let pointee = self.shallow(pointee);
                    if matches!(self.tcx.tys.kind(pointee), TyKind::Infer(_)) { return; }
                    let snap = self.start_snapshot();
                    if self.eq(inner, pointee) {
                        self.commit_from(snap);
                        self.set_coerce(e.0, Coerce::RefToRaw(tm), target);
                    } else {
                        self.rollback_to(snap);
                        // rustc check_ref_cast: &[T; N] -> *const T first coerces
                        // to a pointer to the array, using the source mutability.
                        let inner = self.shallow(inner);
                        if let TyKind::Array(element, _) = self.tcx.tys.kind(inner).clone() {
                            let snap = self.start_snapshot();
                            if self.eq(element, pointee) {
                                self.commit_from(snap);
                                self.set_coerce(e.0, Coerce::RefToRaw(sm), target);
                            } else { self.rollback_to(snap); }
                        }
                    }
                }
            }
            // rustc coerce_borrowed_pointer: a cast of a reference place to a reference type reborrows (and
            // then unsizes), so a `&mut` place is not moved out by `place as &mut dyn Trait`.
            TyKind::Ref(tm, _, _) if sm || !tm => {
                if sm || self.b.coerce(e.0).is_some() {
                    let b = self.borrow_target(source, tm);
                    self.set_reborrow(e.0, tm, b);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ writeback

    fn alias_bound_clauses(&self, t: TyId) -> Vec<Predicate> {
        let TyKind::Alias(_, alias) = self.tcx.tys.kind(t) else { return Vec::new(); };
        let def = alias.kind.def_id();
        let args = self.tcx.complete_args(self.prog, def, self.tcx.tys.list(alias.args), alias.regions);
        let mut result = Vec::new();
        for p in self.tcx.item_bounds(self.prog, def) {
            let p = self.tcx.instantiate_predicate(self.prog, def, &args, &p);
            for p in self.tcx.supertraits(self.prog, p) {
                result.push(p);
            }
        }
        result
    }

    /// rustc_hir_typeck/src/callee.rs:368-452 and method/mod.rs:345-478.
    /// M7 represents async closures as ordinary closures returning futures.
    fn try_overloaded_call_traits(&mut self, callee: TyId, nargs: Option<usize>) -> Option<(DefId, TyId, Vec<TyId>, TyId)> {
        use crate::lang_items::LangItem;
        for item in [LangItem::Fn, LangItem::FnMut, LangItem::FnOnce, LangItem::AsyncFn, LangItem::AsyncFnMut, LangItem::AsyncFnOnce] {
            let Some(td) = self.tcx.lang_items.get(item) else { continue };
            let tuple = match nargs {
                Some(n) => self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&(0..n).map(|_| self.infcx.next_ty_var()).collect::<Vec<_>>()))),
                None => self.infcx.next_ty_var(),
            };
            let goal = self.trait_goal(td, callee, &[tuple]);
            if !self.infcx.root_goal_may_hold_opaque_types_jank(self.prog, goal.clone()) { continue; }
            let method = self.prog.traits[self.prog.def(td).sub as usize].iter().copied()
                .find(|d| self.prog.def(*d).kind == DefKind::AssocFn).expect("call trait method");
            let full = self.infer_item_args(method, &[callee, tuple]);
            let sig = self.tcx.fn_sig(self.prog, method).expect("call trait signature").clone();
            let (params, ret) = self.instantiate_call_signature(method, &full, &sig);
            let params: Vec<_> = params.into_iter().map(|t| self.normalize_body_type(t)).collect();
            let ret = self.normalize_body_type(ret);
            self.apply_fn_bounds(method, &full, self.last_lo);
            for &ty in params.iter().chain(std::iter::once(&ret)) {
                self.register_goal(Goal { param_env: self.param_env.clone(), predicate: Predicate::dummy(
                    PredicateKind::Clause(crate::next_solver::ClauseKind::WellFormed(crate::next_solver::Term::Ty(ty)))) }, self.last_lo);
            }
            let tuple = self.infcx.shallow_resolve(params[1]);
            let inputs = match self.tcx.tys.kind(tuple) { TyKind::Tuple(ps) => self.tcx.tys.list(*ps).to_vec(), TyKind::Error => Vec::new(), _ => {
                self.err(self.last_lo, "call trait arguments must be a tuple".into());
                Vec::new()
            } };
            return Some((td, tuple, inputs, ret));
        }
        None
    }

    /// A diverging scrutinee (`match return 1 { 2 => .. }`): rustc types it against a fresh
    /// variable (demand_scrutinee_type), so `!` reaches the patterns as NeverToAny to that
    /// variable, which falls back like any diverging one.
    fn never_scrutinee(&mut self, x: ExprId, t: TyId) -> TyId {
        let s = self.shallow(t);
        if !matches!(self.tcx.tys.kind(s), TyKind::Never) {
            return t;
        }
        let v = self.fresh(0);
        self.set_coerce(x.0, Coerce::Never, v);
        self.diverging.push(v);
        v
    }

    /// A pending pointer-to-dyn coercion (`deferred_dyn`), once its pointee is known.
    fn finish_dyn_coercion(&mut self, ex: ExprId, a: TyId, x: TyId, pos: u32) {
        let (sa, sx) = (self.shallow(a), self.shallow(x));
        let (ka, kx) = (self.tcx.tys.kind(sa).clone(), self.tcx.tys.kind(sx).clone());
        if let Some((src, dy)) = self.dyn_coercion(&ka, &kx) {
            self.set_coerce(ex.0, Coerce::ToDyn(src, dy), x);
        } else if let Some((sd, dd)) = self.dyn_upcast(&ka, &kx) {
            self.set_coerce(ex.0, Coerce::Upcast(sd, dd), x);
        } else if !self.eq(sa, sx) {
            let (p, q) = (self.ty_str(sx), self.ty_str(sa));
            self.err(pos, format!("E0308: mismatched types: expected `{}`, found `{}`", p, q));
        }
    }

    /// rustc_hir_typeck/src/upvar.rs:457-465,625-626: equate kind and minimum
    /// capture types after processing nested bodies, before retrying obligations.
    fn analyze_closures(&mut self) {
        if self.pending_captures.is_empty() || !self.b.errors.is_empty() { return; }
        // The visitor's type queries must see this inference context's assignments.
        for i in 0..self.b.locals.len() { self.b.locals[i].ty = self.norm(self.b.locals[i].ty); }
        for i in 0..self.b.expr_ty.len() { if self.b.expr_typed.get(i) { self.b.expr_ty[i] = self.norm(self.b.expr_ty[i]); } }
        for i in 0..self.b.pat_ty.len() { if self.b.pat_typed.get(i) { self.b.pat_ty[i] = self.norm(self.b.pat_ty[i]); } }
        for k in self.b.methods.keys().copied().collect::<Vec<_>>() {
            let args = self.tcx.tys.list(self.b.methods[&k].args).iter().map(|&t| self.norm(t)).collect::<Vec<_>>();
            self.b.methods.get_mut(&k).unwrap().args = self.tcx.tys.mk_list(&args);
        }
        for key in self.b.adjustments.keys().copied().collect::<Vec<_>>() {
            let mut adj = self.b.adjustments[&key].clone();
            for a in adj.operand.iter_mut().chain(adj.value.iter_mut()) {
                if let adjustments::Adjust::Deref(Some((method, args))) = a.kind {
                    let args = self.tcx.tys.list(args).iter().map(|&t| self.norm(t)).collect::<Vec<_>>();
                    a.kind = adjustments::Adjust::Deref(Some((method, self.tcx.tys.mk_list(&args))));
                }
            }
            self.b.adjustments.insert(key, adj);
        }
        for pending in std::mem::take(&mut self.pending_captures) {
            crate::upvar::analyze(&self.cx.with_infcx(&self.infcx), self.prog, self.tcx, &mut self.b, &pending);
            let ce = pending.closure;
            // rustc check_closure: a kind deduced from the expected bound IS the closure's kind (its
            // `closure_kind_ty` is that type, not an inference variable); upvar analysis only infers it otherwise.
            if let (Some(kind), Some(result)) = (self.expected_closure_kinds.get(&ce).copied(), self.b.min_captures.get_mut(&ce)) { if result.errors.is_empty() { result.kind = kind; } }
            let result = &self.b.min_captures[&ce];
            if !result.errors.is_empty() {
                let errors = result.errors.clone();
                for (expr, reason) in errors {
                    let lo = if expr & crate::tcx::ASYNC_FN_BODY == 0 { self.ast().expr(ExprId(expr)).lo } else { self.last_lo };
                    self.err(lo, format!("capture analysis incomplete: {}", reason));
                }
                continue;
            }
            let kind = self.tcx.tys.from_closure_kind(result.kind);
            let mut fields = Vec::new();
            for capture in &result.captures {
                let ty = capture.place.ty();
                fields.push(match capture.info.kind {
                    crate::upvar::CaptureKind::ByValue => ty,
                    crate::upvar::CaptureKind::ByRef(kind) => self.tcx.tys.intern(TyKind::Ref(
                        kind != crate::expr_use::BorrowKind::Imm, ty, crate::types::RegionId::ERASED)),
                });
            }
            let up = self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&fields)));
            self.eq(pending.upvars, up);
            if matches!(self.tcx.tys.kind(self.infcx.shallow_resolve(pending.kind)), TyKind::Infer(_)) { self.eq(pending.kind, kind); }
        }
    }

    fn resolve_capture_place(&mut self, place: &mut crate::expr_use::Place) {
        place.root_ty = self.norm(place.root_ty);
        for (_, ty) in &mut place.projections { *ty = self.norm(*ty); }
    }

    fn finish(&mut self) {
        // Used foreign declarations have no body of their own to report signature
        // diagnostics. Keep those errors with each body that depends on them.
        for &d in &self.b.dependencies {
            for error in self.tcx.signature_errors(self.prog, d) {
                if !self.b.errors.contains(error) { self.b.errors.push(error.clone()); }
            }
        }
        self.retry_goals(false);
        self.check_repeat_exprs();
        // rustc_hir_typeck/src/lib.rs:226-230: defining uses may guide inference
        // before fallback.
        self.try_handle_opaque_type_uses_next();
        self.retry_goals(false);
        // diverging fallback (rustc fallback.rs): a variable only a `!` value flowed into is `()`
        // before edition 2024 and `!` from 2024 (`f(|_| panic!())` instantiates R = ())
        if !self.diverging.is_empty() {
            let ed = self.prog.crates[self.prog.def(self.b.def).krate as usize].edition;
            let fb = if ed >= 2024 { self.tcx.tys.never } else { self.tcx.tys.unit };
            for v in std::mem::take(&mut self.diverging) {
                let s = self.shallow(v);
                if matches!(self.tcx.tys.kind(s), TyKind::Infer(_)) {
                    self.eq(s, fb);
                }
            }
        }
        // integer/float literal fallback happens once, at the end (rustc fallback.rs)
        self.keep_literal_vars = false;
        let (ints, floats) = { let mut inner = self.infcx.inner.borrow_mut(); (inner.int_unification_table().len(), inner.float_unification_table().len()) };
        for v in 0..ints as u32 {
            let t = self.tcx.tys.intern(TyKind::IntVar(v));
            if matches!(self.tcx.tys.kind(self.infcx.shallow_resolve(t)), TyKind::IntVar(_)) { self.eq(t, self.tcx.tys.i32_); }
        }
        for v in 0..floats as u32 {
            let t = self.tcx.tys.intern(TyKind::FloatVar(v));
            if matches!(self.tcx.tys.kind(self.infcx.shallow_resolve(t)), TyKind::FloatVar(_)) { self.eq(t, self.tcx.tys.f64_); }
        }
        self.retry_goals(false);
        for (x, xt, target) in std::mem::take(&mut self.deferred_casts) {
            // cast.rs:check/try_coercion_cast and check_ref_cast:1215-1251:
            // fallback precedes coercion, then the array-to-element exception.
            if self.try_coerce(xt, target, Some(x)) { continue; }
            let sx = self.shallow(xt);
            let st = self.shallow(target);
            if let (TyKind::Ref(source_mut, inner, _), TyKind::Ptr(target_mut, pointee)) =
                (self.tcx.tys.kind(sx).clone(), self.tcx.tys.kind(st).clone()) {
                if source_mut || !target_mut {
                    // cast.rs check_ref_cast: the referent is structurally resolved (an alias of an array)
                    let inner = self.norm(inner);
                    if let TyKind::Array(element, _) = self.tcx.tys.kind(self.shallow(inner)).clone() {
                        let snapshot = self.start_snapshot();
                        if self.eq(element, pointee) {
                            self.commit_from(snapshot);
                            self.set_coerce(x.0, Coerce::RefToRaw(source_mut), target);
                        } else { self.rollback_to(snapshot); }
                    }
                }
                // (a reference that is no array pointer cast is the cast check's E0606)
            }
            self.record_cast_borrow(x, xt, target);
        }
        for (x, xt, target, lo) in std::mem::take(&mut self.cast_checks) { self.check_cast_final(x, xt, target, lo); }
        // pending pointer-to-dyn coercions, now that their pointees are known
        for (ex, a, x, pos) in std::mem::take(&mut self.deferred_dyn) {
            self.finish_dyn_coercion(ex, a, x, pos);
        }
        self.finish_mutable_places();
        self.analyze_closures();
        for call in std::mem::take(&mut self.deferred_call_resolutions) { call.resolve(self); }
        // rustc_hir_typeck/src/lib.rs:244-247: closure analysis precedes
        // normalization and registration of fn-item Sized obligations.
        for (ty, lo) in std::mem::take(&mut self.deferred_sized_obligations) {
            let ty = self.norm(ty);
            if self.tcx.tys.has_error(ty) { continue; }
            if let Some(sized) = self.tcx.lang_items.get(crate::lang_items::LangItem::Sized) {
                let goal = self.trait_goal(sized, ty, &[]);
                self.register_goal(goal, lo);
            }
        }
        self.retry_goals(false);
        self.handle_opaque_type_uses_next();
        self.finish_closure_signatures();
        // Capture types may contain an inner closure's formerly open upvar
        // tuple. Resolve these through the same context as expression/local types.
        let mut captures = std::mem::take(&mut self.b.min_captures);
        for c in captures.values_mut() {
            for capture in &mut c.captures { self.resolve_capture_place(&mut capture.place); }
            for (place, _, _) in &mut c.fake_reads { self.resolve_capture_place(place); }
        }
        self.b.min_captures = captures;
        crate::upvar::coroutine_closure_views(self.prog, self.tcx, &mut self.b);
        crate::upvar::dump_captures(self.prog, &self.b);
        self.drain_stalled_coroutine_obligations();
        self.retry_goals(true);
        self.resolve_type_vars_in_body();
        for i in 0..self.b.expr_ty.len() {
            let e = ExprId(if i < self.b.expr_n1 as usize { self.b.expr_lo + i as u32 } else { self.b.expr_lo2 + i as u32 - self.b.expr_n1 });
            // rustc_hir_typeck/src/writeback.rs:152-165: scalar operators use
            // builtin lowering after their trait obligations have been checked.
            let scalar = |t: TyId| matches!(self.tcx.tys.kind(t), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::FnDef(..));
            match self.ast().expr(e).kind {
                ExprKind::Unary(UnOp::Neg | UnOp::Not, inner) => {
                    if scalar(self.b.ty(inner)) { self.b.binops.remove(&e.0); }
                }
                // writeback.rs:162-176: a comparison whose (formerly inferred) operands are scalars
                ExprKind::Binary(op, l, r) if matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) => {
                    if scalar(self.b.ty(l)) && scalar(self.b.ty(r)) { self.b.binops.remove(&e.0); }
                }
                _ => {}
            }
        }
        self.record_stalled_coroutine_goals();
        self.detect_opaque_types_added_during_writeback();
    }

    // ------------------------------------------------------------ scopes

    /// The local named `s` that an identifier at `pos` refers to, with macro_rules hygiene
    /// (rustc: locals and labels resolve by name and SyntaxContext; walking out of the scopes
    /// a macro body sees, past the macro's definition, drops the expansion's mark): a binding
    /// of the same context, or -- for an identifier of a macro body -- a binding of the root
    /// context made before the macro's definition (in this file).
    fn lookup_local(&self, s: Sym, pos: u32) -> Option<u32> {
        let pf = &self.prog.files[self.file as usize];
        let c = pf.hyg_ctxt(pos);
        let mut i = self.scopes.len();
        while i > 0 {
            i -= 1;
            if self.scopes[i].0 == s {
                // a local declared before an item-holding block began is outside that block's
                // item rib: the block's item of that name shadows it (late.rs ribs)
                if self.item_blocks.iter().any(|&(m, entry)| entry > i && self.prog.lookup_in_module(m, s, false, &mut Vec::new()).is_some()) {
                    return None;
                }
                let li = self.scopes[i].1;
                let (bc, bpos) = match self.local_hyg.get(li as usize) {
                    Some(x) => x,
                    None => return Some(li),
                };
                let bc = match bc {
                    Some(b) => b,
                    None => return Some(li),
                };
                // the binding's context is the use's minus marks whose macros were defined
                // after the binding (in this file)
                if c.len() >= bc.len() && c[..bc.len()] == bc[..] && c[bc.len()..].iter().all(|m| {
                    let d = pf.hyg_def_pos(*m);
                    d != u32::MAX && pf.pos_before(*bpos, d)
                }) {
                    return Some(li);
                }
            }
        }
        None
    }

    fn set_ty(&mut self, e: ExprId, t: TyId) {
        let i = self.b.expr_ix(e);
        if i < self.b.expr_ty.len() {
            self.b.expr_ty[i] = t;
            self.b.expr_typed.set(i);
            // rustc_hir_typeck/src/fn_ctxt/_impl.rs:194: write_ty taints on error types.
            if self.tcx.tys.flags(t).intersects(crate::types::TypeFlags::HAS_ERROR) { self.infcx.set_tainted_by_errors(); }
        }
    }

    // ------------------------------------------------------------ patterns

    pub fn check_pat(&mut self, p: PatId, expected: TyId) {
        // a whole pattern (not a part of one): its bindings are checked once
        if self.pat_nest == 0 && self.tcx.validates(self.prog, self.file) { self.check_pattern_bindings(p); }
        self.pat_nest += 1;
        self.check_pat_nested(p, expected);
        self.pat_nest -= 1;
    }

    fn check_pat_nested(&mut self, p: PatId, expected: TyId) {
        if self.pat_depth == 0 && self.tcx.validates(self.prog, self.file) {
            self.pat_bindings(p, true);
        }
        self.pat_depth += 1;
        let ast = self.ast();
        // the pattern's type is the matched value's (lowering peels pat_derefs itself)
        let pi = self.b.pat_ix(p);
        if pi < self.b.pat_ty.len() {
            self.b.pat_ty[pi] = expected;
            self.b.pat_typed.set(pi);
            if self.tcx.tys.flags(expected).intersects(crate::types::TypeFlags::HAS_ERROR) { self.infcx.set_tainted_by_errors(); }
        }
        // match ergonomics: a structural pattern against references matches through them and
        // switches the default binding mode to `ref` / `ref mut`
        // literal and range patterns peel references too, except string / byte-string
        // literals, which match `&str` / `&[u8]` themselves (rustc pat.rs calc_adjust_mode:
        // AdjustMode::Peel for PatKind::Expr unless the literal's type is a reference)
        let peel_lit = match ast.pat(p) {
            Pat::Lit(e) => !matches!(ast.expr(*e).kind, ExprKind::Lit(LitKind::Str) | ExprKind::Lit(LitKind::RawStr) | ExprKind::Lit(LitKind::ByteStr) | ExprKind::Lit(LitKind::CStr)),
            Pat::Range(..) => true,
            _ => false,
        };
        // (a constant pattern passes the expected type unpeeled: its own type may be a
        // reference, `const Z: &() = ..; match &() { Z => .. }`; rustc pat.rs AdjustMode::Pass)
        // a path pattern resolves first: a const (or assoc const, `i32::C`, `Self::C`) keeps the
        // expected type as it is, an ADT constructor peels references
        let mut pre_const = false;
        if let Pat::Path(path) = ast.pat(p) {
            let (r, t) = self.resolve_value_path(path, p.0 | PAT_KEY, true);
            pre_const = matches!(r, Res::Def(d) if matches!(self.prog.def(d).kind, DefKind::Const | DefKind::AssocConst));
            self.pat_path_pre = Some((r, t));
        }
        let structural = peel_lit
            || matches!(ast.pat(p), Pat::Tuple(_) | Pat::TupleStruct(..) | Pat::Struct(..) | Pat::Slice(_))
            || matches!(ast.pat(p), Pat::Path(_) if !pre_const)
            || matches!(ast.pat(p), Pat::Ident { sub: None, by_ref: false, mutbl: false, .. } if self.ident_pat_is_path(p) && !self.ident_pat_is_const(p));
        let mut expected = expected;
        let saved_mode = self.bind_mode;
        if structural {
            let mut n = 0usize;
            loop {
                let s = self.shallow(expected);
                match self.tcx.tys.kind(s).clone() {
                    TyKind::Ref(m, inner, _) => {
                        n += 1;
                        self.bind_mode = if !m || self.bind_mode == 1 { 1 } else { 2 };
                        expected = inner;
                    }
                    _ => break,
                }
            }
            if n > 0 {
                self.b.pat_derefs.insert(p.0, n);
            }
        }
        if let Pat::Ref(..) = ast.pat(p) {
            self.bind_mode = 0;
        }
        self.check_pat_inner(p, expected);
        // rustc_hir_typeck/expr_use_visitor.rs:943-947, 1858: capture semantics
        // depend on the variant list, never on which variants are inhabited.
        // Non-exhaustiveness counts in the defining crate as well as other crates.
        let constructor = matches!(ast.pat(p), Pat::TupleStruct(..) | Pat::Struct(..))
            || matches!(self.b.pat_res.get(&p.0), Some(Res::Def(d))
                if matches!(self.prog.def(*d).kind, DefKind::Variant | DefKind::Struct));
        if constructor {
            let t = self.shallow(expected);
            if let TyKind::Adt(d, _, _) = self.tcx.tys.kind(t) {
                let multiple = self.tcx.adts.get(d).map_or(false, |a| a.variants.len() > 1);
                let def = self.prog.def(*d);
                let pf = &self.prog.files[def.file as usize];
                let non_exhaustive = def.kind == DefKind::Enum
                    && pf.ast.list(pf.ast.item(def.item).attrs).iter().any(|a| {
                        a.toks.lo < a.toks.hi && pf.tok_text(a.toks.lo) == "non_exhaustive"
                    });
                if multiple || non_exhaustive {
                    self.b.pat_discriminant_reads.insert(p.0, ());
                }
            }
        }
        self.bind_mode = saved_mode;
        self.pat_depth -= 1;
    }

    /// The names a pattern binds (rustc_resolve late.rs `resolve_pattern_top` / `check_consistent_bindings`):
    /// E0416 when one alternative binds a name twice, E0408 when the alternatives of an
    /// or-pattern bind different names. An identifier naming a unit struct / variant / const is
    /// not a binding.
    fn pat_bindings(&mut self, p: PatId, report: bool) -> Vec<(Sym, u32)> {
        let ast = self.ast();
        let mut out: Vec<(Sym, u32)> = Vec::new();
        // (names written by different macro expansions are different bindings)
        let same = |this: &Self, a: (Sym, u32), b: (Sym, u32)| a.0 == b.0 && {
            let pf = &this.prog.files[this.file as usize];
            pf.hyg_ctxt(a.1) == pf.hyg_ctxt(b.1)
        };
        let add = |this: &mut Self, out: &mut Vec<(Sym, u32)>, more: Vec<(Sym, u32)>| {
            for (n, pos) in more {
                if out.iter().any(|m| same(this, *m, (n, pos))) {
                    if report {
                        this.err(pos, format!("E0416: identifier `{}` is bound more than once in the same pattern", this.prog.syms.str(n)));
                    }
                } else {
                    out.push((n, pos));
                }
            }
        };
        match ast.pat(p) {
            Pat::Ident { name, sub, .. } => {
                if !(sub.is_none() && self.ident_pat_is_path(p)) {
                    out.push((self.sym(*name), name.lo));
                }
                if let Some(sp) = sub {
                    let more = self.pat_bindings(*sp, report);
                    add(self, &mut out, more);
                }
            }
            Pat::TupleStruct(_, l) | Pat::Tuple(l) | Pat::Slice(l) => {
                // `..` may appear once (rustc_ast_passes check_pat)
                let rests = ast.list(*l).iter().filter(|c| match ast.pat(**c) {
                    Pat::Rest => true,
                    Pat::Ident { sub: Some(s), .. } => matches!(ast.pat(*s), Pat::Rest),
                    _ => false,
                }).count();
                if report && rests > 1 {
                    let what = match ast.pat(p) { Pat::Tuple(_) => "tuple", Pat::Slice(_) => "slice", _ => "tuple struct" };
                    let lo = ast.list(*l).iter().find_map(|c| if let Pat::Ident { name, .. } = ast.pat(*c) { Some(name.lo) } else { None }).unwrap_or(0);
                    self.err(lo, format!("`..` can only be used once per {what} pattern"));
                }
                // ast_lowering lower_pat_tuple: `binding @ ..` is only allowed in a slice pattern
                if report && !matches!(ast.pat(p), Pat::Slice(_)) {
                    let what = if matches!(ast.pat(p), Pat::Tuple(_)) { "tuple" } else { "tuple struct" };
                    for &c in ast.list(*l) {
                        if let Pat::Ident { name, sub: Some(s), .. } = ast.pat(c) {
                            if matches!(ast.pat(*s), Pat::Rest) {
                                let shown = self.prog.syms.str(self.sym(*name)).to_string();
                                self.err(name.lo, format!("`{shown} @` is not allowed in a {what}"));
                            }
                        }
                    }
                }
                for &c in ast.list(*l) {
                    let more = self.pat_bindings(c, report);
                    add(self, &mut out, more);
                }
            }
            Pat::Struct(_, fields, _) => {
                for fp in ast.list(*fields) {
                    let more = self.pat_bindings(fp.pat, report);
                    add(self, &mut out, more);
                }
            }
            Pat::Ref(_, c) | Pat::Paren(c) | Pat::Box(c) => out = self.pat_bindings(*c, report),
            Pat::Or(alts) => {
                let sets: Vec<Vec<(Sym, u32)>> = ast.list(*alts).iter().map(|&a| self.pat_bindings(a, report)).collect();
                for (i, set) in sets.iter().enumerate() {
                    for &(n, pos) in set {
                        if !out.iter().any(|m| same(self, *m, (n, pos))) {
                            if report && sets.iter().enumerate().any(|(j, other)| j != i && !other.iter().any(|m| same(self, *m, (n, pos)))) {
                                self.err(pos, format!("E0408: variable `{}` is not bound in all patterns", self.prog.syms.str(n)));
                            }
                            out.push((n, pos));
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// E0415: a parameter name bound more than once in one parameter list.
    pub(crate) fn check_param_names(&mut self, pats: &[PatId]) {
        let mut seen: Vec<(Sym, u32)> = Vec::new();
        for &p in pats {
            for (n, pos) in self.pat_bindings(p, false) {
                let pf = &self.prog.files[self.file as usize];
                if seen.iter().any(|(m, mp)| *m == n && pf.hyg_ctxt(*mp) == pf.hyg_ctxt(pos)) {
                    self.err(pos, format!("E0415: identifier `{}` is bound more than once in this parameter list", self.prog.syms.str(n)));
                } else {
                    seen.push((n, pos));
                }
            }
        }
    }

    /// 0 unit, 1 tuple, 2 struct: the shape of a struct or variant's constructor.
    fn value_ctor_shape(&self, d: DefId) -> Option<u8> {
        let def = self.prog.def(d);
        let (adt, vi) = match def.kind {
            DefKind::Struct => (d, 0),
            DefKind::Variant => (def.parent, def.sub as usize),
            _ => return None,
        };
        self.tcx.adts.get(&adt)?.variants.get(vi).map(|v| v.shape)
    }

    fn ident_pat_is_const(&self, p: PatId) -> bool {
        if let Pat::Ident { name, .. } = self.ast().pat(p) {
            let s = self.sym(*name);
            if let Some(d) = self.prog.lookup_in_scope(self.module, s, false) {
                return matches!(self.prog.def(d).kind, DefKind::Const | DefKind::AssocConst);
            }
        }
        false
    }

    /// An identifier pattern naming a unit struct / unit variant / const (not a binding).
    fn ident_pat_is_path(&self, p: PatId) -> bool {
        if let Pat::Ident { name, .. } = self.ast().pat(p) {
            let s = self.sym(*name);
            if let Some(d) = self.prog.lookup_in_scope(self.module, s, false) {
                return matches!(self.prog.def(d).kind, DefKind::Const | DefKind::Variant | DefKind::Struct | DefKind::AssocConst);
            }
        }
        false
    }

    fn check_pat_inner(&mut self, p: PatId, expected: TyId) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident { by_ref, mutbl, name, sub } => {
                let s = self.sym(*name);
                // rustc_resolve late.rs try_resolve_as_non_binding: a binding may not shadow a
                // static, a tuple struct / variant constructor, or (written `ref x` / `mut x` /
                // `x @ ..`, so not a path pattern) a unit struct / variant / const
                // (statics, constants and unit structs / variants are reported below)
                if self.tcx.validates(self.prog, self.file) && !self.scopes.iter().any(|(sy, _)| *sy == s) {
                    if let Some(d) = self.prog.lookup_in_scope(self.module, s, false) {
                        let simple = sub.is_none() && !*by_ref && !*mutbl;
                        let what = match self.prog.def(d).kind {
                            DefKind::Struct | DefKind::Variant => match self.value_ctor_shape(d) {
                                Some(1) => Some(if self.prog.def(d).kind == DefKind::Struct { "tuple structs" } else { "tuple variants" }),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some(what) = what {
                            self.err(name.lo, format!("E0530: bindings cannot shadow {what}"));
                        } else if simple {
                            // an identifier pattern naming something ambiguous is an error, not a fresh binding
                            if let Some(e) = self.prog.name_ambiguity(self.module, s, false, false) {
                                if let Some(m) = crate::lint::privacy_diag(self.prog, self.file, name.lo, &e) { self.err(name.lo, m); }
                            }
                        }
                    }
                }
                // a unit struct / unit variant / const in scope is a path pattern, not a binding
                if sub.is_none() && !*by_ref && !*mutbl {
                    if let Some(d) = self.prog.lookup_in_scope(self.module, s, false) {
                        let k = self.prog.def(d).kind;
                        if matches!(k, DefKind::Const | DefKind::Variant | DefKind::Struct | DefKind::AssocConst) {
                            self.b.pat_res.insert(p.0, Res::Def(d));
                            let t = self.value_def_ty(d, None, p.0 | PAT_KEY);
                            self.coerce_or_unify(t, expected, None, name.lo);
                            return;
                        }
                    }
                }
                // a binding cannot take the name of a static or a tuple struct/variant (E0530)
                if self.tcx.validates(self.prog, self.file) {
                    if self.env.consts.iter().any(|c| self.env.names.get(c.0 as usize) == Some(&s)) {
                        self.err(name.lo, "E0530: match bindings cannot shadow const parameters".to_string());
                    }
                    if let Some(d) = self.prog.lookup_in_scope(self.module, s, false) {
                        let what = match self.prog.def(d).kind {
                            DefKind::Static | DefKind::ForeignStatic => Some("statics"),
                            // (reached only with `mut`, `ref` or `@`: otherwise the name is a path)
                            DefKind::Const | DefKind::AssocConst => Some("constants"),
                            DefKind::Variant | DefKind::Struct => Some("unit variants or structs"),
                            _ => None,
                        };
                        if let Some(what) = what {
                            self.err(name.lo, format!("E0530: match bindings cannot shadow {}", what));
                        }
                    }
                }
                let ty = if *by_ref {
                    self.tcx.tys.intern(TyKind::Ref(*mutbl, expected, crate::types::RegionId::ERASED))
                } else if self.bind_mode != 0 && !*mutbl {
                    // bound by reference through the default binding mode (a `mut x` binding
                    // resets it to move, rustc pat.rs check_pat_ident, editions <= 2021)
                    self.b.pat_bind_ref.insert(p.0, self.bind_mode);
                    self.tcx.tys.intern(TyKind::Ref(self.bind_mode == 2, expected, crate::types::RegionId::ERASED))
                } else {
                    expected
                };
                let li = self.b.locals.len() as u32;
                // rustc GatherLocalsVisitor (gather_locals.rs): every binding's type is Sized
                // (VariableType); checked by `makepad check` only (the run never pays for it)
                if crate::lint::enabled() { self.require_type_is_sized_deferred(ty, name.lo); }
                self.b.locals.push(Local { name: s, ty, mutable: *mutbl && !*by_ref, pat: p.0 });
                self.b.pat_local.insert(p.0, li);
                let pf = &self.prog.files[self.file as usize];
                let h = (Some(pf.hyg_ctxt(name.lo)), name.lo);
                self.local_hyg.resize(li as usize + 1, (Some(Vec::new()), 0));
                self.local_hyg[li as usize] = h;
                self.scopes.push((s, li));
                if let Some(sp) = sub {
                    self.check_pat(*sp, expected);
                }
            }
            Pat::Lit(e) => {
                let t = self.check_expr(*e, Some(expected));
                // string literal patterns against &str, byte strings against &[u8]
                self.coerce_or_unify(t, expected, Some(*e), ast.expr(*e).lo);
            }
            Pat::Range(a, b, _) => {
                // rustc pat.rs check_pat_range: each end has the scrutinee's type (demand_eqtype_pat)
                for end in [a, b].into_iter().flatten() {
                    let t = self.check_expr(*end, Some(expected));
                    let pos = self.ast().expr(*end).lo;
                    self.demand_eq(expected, t, pos);
                }
            }
            Pat::Tuple(v) => {
                let et = self.shallow(expected);
                let elems = match *self.tcx.tys.kind(et) {
                    TyKind::Tuple(x) => self.tcx.tys.list(x).to_vec(),
                    _ => {
                        let mut x = Vec::new();
                        for _ in 0..v.len() {
                            x.push(self.fresh(0));
                        }
                        let tt = self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&x)));
                        // rustc pat.rs check_pat_tuple: with a `..` the scrutinee's type must be known
                        // (structurally_resolve_type), and it must be a tuple (demand_eqtype_pat)
                        let has_rest = self.ast().list(*v).iter().any(|&q| is_rest_pat(self.ast(), q));
                        if has_rest && matches!(self.tcx.tys.kind(et), TyKind::Infer(_)) {
                            self.err(self.last_lo, "E0282: type annotations needed".to_string());
                        } else if !self.eq(tt, et) && !matches!(self.tcx.tys.kind(et), TyKind::Error) {
                            let shown = self.ty_str(et);
                            self.err(self.last_lo, format!("E0308: mismatched types: expected `{}`, found a tuple", shown));
                        }
                        x
                    }
                };
                self.check_seq_pats(self.ast().list(*v), &elems);
            }
            Pat::Paren(x) => self.check_pat(*x, expected),
            Pat::Ref(m, x) => {
                // rustc pat.rs check_pat_ref: the scrutinee is a reference of the same mutability
                // (an open type variable becomes one)
                let expected = self.deeply_resolve_ignoring_regions_with_obligations(expected);
                let et = self.shallow(expected);
                let inner = match self.tcx.tys.kind(et).clone() {
                    TyKind::Ref(em, i, _) => {
                        if em != *m { self.demand_pat_eq(expected, *m); }
                        i
                    }
                    TyKind::Infer(_) => {
                        let i = self.fresh(0);
                        let r = self.tcx.tys.intern(TyKind::Ref(*m, i, crate::types::RegionId::ERASED));
                        self.eq(et, r);
                        i
                    }
                    TyKind::Error => self.tcx.tys.error,
                    _ => { self.demand_pat_eq(expected, *m); self.tcx.tys.error }
                };
                self.check_pat(*x, inner);
            }
            Pat::Or(v) => {
                // every alternative binds the same names to the same variables (rustc: one
                // binding per name, the alternatives' types must agree)
                let mark = self.scopes.len();
                let mut first: Vec<(Sym, u32)> = Vec::new();
                for (i, x) in self.ast().list(*v).iter().enumerate() {
                    let m = self.scopes.len();
                    self.check_pat(*x, expected);
                    if i == 0 {
                        first = self.scopes[mark..].to_vec();
                        continue;
                    }
                    let added: Vec<(Sym, u32)> = self.scopes[m..].to_vec();
                    self.scopes.truncate(m);
                    for (s, li) in added {
                        if let Some(&(_, l0)) = first.iter().find(|b| b.0 == s) {
                            let (t0, t1) = (self.b.locals[l0 as usize].ty, self.b.locals[li as usize].ty);
                            self.eq(t1, t0);
                            // every binding that resolved to this alternative's local (a nested or-pattern's
                            // other alternatives did) is the first alternative's variable
                            let pp = self.b.locals[li as usize].pat;
                            self.b.pat_local.insert(pp, l0);
                            for v in self.b.pat_local.values_mut() { if *v == li { *v = l0; } }
                        }
                    }
                }
            }
            Pat::Slice(v) => {
                // pat.rs check_pat_slice: resolve with pending obligations; an irrefutable pattern
                // against a still-open variable fixes it to an array of the pattern's length
                // (rustc try_resolve_slice_ty_to_array_ty, issue #76342)
                let expected = self.deeply_resolve_ignoring_regions_with_obligations(expected);
                if self.pat_decl_irrefutable && matches!(self.tcx.tys.kind(self.shallow(expected)), TyKind::Infer(_)) && !self.ast().list(*v).iter().any(|x| is_rest_pat(ast, *x)) {
                    let inner = self.fresh(0);
                    let arr = self.tcx.tys.intern(TyKind::Array(inner, self.tcx.tys.const_usize(self.ast().list(*v).len() as u64)));
                    self.eq(expected, arr);
                }
                let et = self.shallow(expected);
                let has_rest = self.ast().list(*v).iter().any(|x| is_rest_pat(ast, *x));
                let fixed = self.ast().list(*v).iter().filter(|x| !is_rest_pat(ast, **x)).count() as u64;
                // pat.rs check_pat_slice / check_array_pat_len: the type of the `..` / `name @ ..` pattern
                let (elem, slice_ty) = match self.tcx.tys.kind(et).clone() {
                    TyKind::Array(e, len) => (e, self.check_array_pat_len(et, e, len, has_rest, fixed)),
                    TyKind::Slice(e) => (e, Some(et)),
                    _ => {
                        if !matches!(self.tcx.tys.kind(et), TyKind::Error | TyKind::Infer(_)) {
                            let ts = self.ty_str(et);
                            self.err(self.last_lo, format!("E0529: expected an array or slice, found `{}`", ts));
                        }
                        (self.tcx.tys.error, Some(self.tcx.tys.error))
                    }
                };
                for x in self.ast().list(*v) {
                    if let Pat::Rest = ast.pat(*x) {
                        continue;
                    }
                    if let Pat::Ident { sub: Some(s), .. } = ast.pat(*x) {
                        if let Pat::Rest = ast.pat(*s) {
                            // `name @ ..`: the subslice / subarray
                            let st = slice_ty.unwrap_or(self.tcx.tys.error);
                            self.check_pat(*x, st);
                            continue;
                        }
                    }
                    self.check_pat(*x, elem);
                }
            }
            Pat::Path(path) => {
                let (r, t) = match self.pat_path_pre.take() {
                    Some(x) => x,
                    None => self.resolve_value_path(path, p.0 | PAT_KEY, true),
                };
                self.b.pat_res.insert(p.0, r);
                if let Res::Def(pd) = r {
                    if self.tcx.validates(self.prog, self.file) && matches!(self.prog.def(pd).kind, DefKind::Static | DefKind::ForeignStatic) {
                        self.err(path.lo, format!("E0532: expected unit struct, unit variant or constant, found static `{}`", crate::tcx::path_str(self.prog, self.file, path)));
                    }
                }
                self.coerce_or_unify(t, expected, None, path.lo);
            }
            Pat::TupleStruct(path, v) => {
                let (r, _) = self.resolve_value_path(path, p.0 | PAT_KEY, true);
                self.b.pat_res.insert(p.0, r);
                let fields = self.ctor_fields_with(r, expected, path.lo, Some(p.0 | PAT_KEY), None);
                // check_pat_tuple_struct: the sub-patterns cover the fields exactly, or a `..`
                // stands for the rest (E0023)
                if let Res::Def(cd) = r {
                    let ast = self.ast();
                    let subs = ast.list(*v);
                    let has_rest = subs.iter().any(|x| matches!(ast.pat(*x), Pat::Rest));
                    let n = subs.len() - has_rest as usize;
                    let tuple_ctor = match self.prog.def(cd).kind {
                        DefKind::Variant => Some(("variant", self.prog.def(cd).parent, self.prog.def(cd).sub as usize)),
                        DefKind::Struct => Some(("struct", cd, 0)),
                        _ => None,
                    };
                    if let Some((what, adt, vi)) = tuple_ctor {
                        if let Some(variant) = self.tcx.adts.get(&adt).and_then(|a| a.variants.get(vi)) {
                            if variant.shape != 1 && self.tcx.validates(self.prog, self.file) {
                                let found = match (variant.shape, what) { (0, "variant") => "unit variant", (0, _) => "unit struct", (_, "variant") => "struct variant", _ => "struct" };
                                self.err(path.lo, format!("E0532: expected tuple struct or tuple variant, found {} `{}`", found, crate::tcx::path_str(self.prog, self.file, path)));
                            }
                            if variant.shape == 1 && !(if has_rest { n <= variant.fields.len() } else { n == variant.fields.len() }) {
                                let want = variant.fields.len();
                                let msg = format!("E0023: this pattern has {} field{}, but the corresponding tuple {} has {} field{}", n, if n == 1 { "" } else { "s" }, what, want, if want == 1 { "" } else { "s" });
                                self.err(path.lo, msg);
                            }
                        }
                    }
                }
                self.check_seq_pats(self.ast().list(*v), &fields);
            }
            Pat::Struct(path, fields, rest) => {
                let d = self.resolve_struct_path(path);
                self.b.pat_res.insert(p.0, d);
                if let Res::Def(dd) = d {
                    let (adt_ty, vi) = self.adt_of_ctor(dd);
                    let adt_ty = self.instantiate_adt(adt_ty);
                    let adt_ty = if self.prog.def(dd).kind == DefKind::Variant { self.variant_path_ty(path, dd, adt_ty) } else { self.struct_path_ty(path, adt_ty) };
                    self.write_adt_user_type(p.0 | PAT_KEY, adt_ty);
                    self.coerce_or_unify(adt_ty, expected, None, path.lo);
                    let et = self.shallow(expected);
                    let mut mentioned: Vec<u32> = Vec::new();
                    let mut bad = false;
                    for fp in self.ast().list(*fields) {
                        let fname = self.sym(fp.name);
                        // `S { 0: a, 1: b }` (a tuple struct's fields by index)
                        let fname = if fp.name.hi > fp.name.lo && self.text(fp.name).as_bytes()[0].is_ascii_digit() {
                            crate::ast::tuple_index(self.text(fp.name)).map_or(u32::MAX - 1, |n| n | 0x8000_0000)
                        } else {
                            fname
                        };
                        let ft = self.field_ty(et, vi, fname);
                        match ft {
                            Some((fi, t)) => {
                                if mentioned.contains(&fi) {
                                    self.err(fp.name.lo, format!("E0025: field `{}` bound multiple times in the pattern", self.text(fp.name)));
                                }
                                mentioned.push(fi);
                                self.check_field_privacy(adt_ty, vi, fi, fp.name.lo);
                                self.check_pat(fp.pat, t)
                            }
                            None => {
                                bad = true;
                                let what = self.adt_field_owner(adt_ty, vi);
                                self.err(fp.name.lo, format!("E0026: {} does not have a field named `{}`", what, self.text(fp.name)))
                            }
                        }
                    }
                    // without `..` every field is named (rustc E0027; a union names one)
                    if !*rest && !bad && self.prog.def(dd).kind != DefKind::Union {
                        if let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(self.shallow(et)).clone() {
                            if let Some(v) = self.tcx.adts.get(&ad).and_then(|a| a.variants.get(vi as usize).cloned()) {
                                // (a field a live edit removed is a dead slot no code names, M10)
                                let missing: Vec<String> = v.fields.iter().enumerate().filter(|(i, f)| !mentioned.contains(&(*i as u32)) && !(f.name & 0x8000_0000 == 0 && self.prog.syms.str(f.name).starts_with("__relax_dead_"))).map(|(_, f)| {
                                    if f.name & 0x8000_0000 != 0 { (f.name & 0x7fff_ffff).to_string() } else { self.prog.syms.str(f.name).to_string() }
                                }).collect();
                                if !missing.is_empty() {
                                    self.err(path.lo, format!("E0027: pattern does not mention field{} `{}` -- name {} or add `..`", if missing.len() > 1 { "s" } else { "" }, missing.join("`, `"), if missing.len() > 1 { "them" } else { "it" }));
                                }
                            }
                        }
                    }
                }
            }
            Pat::Box(x) => self.check_pat(*x, expected),
            Pat::Mac(m) => self.err(self.ast().row(*m).path.lo, "macro in pattern position".to_string()),
        }
    }

    /// rustc_hir_typeck/src/pat.rs check_array_pat_len: the length of an array pattern against the
    /// array type `arr` (E0527 / E0528 / E0730); the type of the `..` pattern if there is one.
    fn check_array_pat_len(&mut self, arr: TyId, e: TyId, len: crate::consts::Const, has_rest: bool, fixed: u64) -> Option<TyId> {
        let lenv = self.tcx.tys.intern(TyKind::Const(len));
        let lenv = self.normalize_body_type(lenv);
        let len = self.tcx.tys.expect_const(lenv).try_to_target_usize(&self.tcx.tys);
        let plural = if fixed == 1 { "" } else { "s" };
        match len {
            Some(len) => {
                if !has_rest {
                    if fixed != len {
                        self.err(self.last_lo, format!("E0527: pattern requires {} element{} but array has {}", fixed, plural, len));
                    }
                    None
                } else if let Some(rest) = len.checked_sub(fixed) {
                    Some(self.tcx.tys.intern(TyKind::Array(e, self.tcx.tys.const_usize(rest))))
                } else {
                    self.err(self.last_lo, format!("E0528: pattern requires at least {} element{} but array has {}", fixed, plural, len));
                    Some(self.tcx.tys.error)
                }
            }
            None if !has_rest => {
                // a fixed-length pattern fixes an inferred array length
                let updated = self.tcx.tys.intern(TyKind::Array(e, self.tcx.tys.const_usize(fixed)));
                if !self.eq(updated, arr) { self.err(self.last_lo, "array pattern length mismatch".into()); }
                None
            }
            None => {
                self.err(self.last_lo, "E0730: cannot pattern-match on an array without a fixed length".to_string());
                Some(self.tcx.tys.error)
            }
        }
    }

    /// The assignee of a destructuring assignment against the assigned type `t`.
    fn check_assignee(&mut self, a: ExprId, t: TyId, lo: u32) {
        let ast = self.ast();
        let is_rest = |x: ExprId| matches!(ast.expr(x).kind, ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false));
        match ast.expr(a).kind.clone() {
            ExprKind::Paren(x) if is_rest(x) => {}
            ExprKind::Paren(x) => {
                self.check_assignee(x, t, lo);
            }
            ExprKind::Underscore => {}
            ExprKind::Tuple(elems) => {
                let st = self.shallow(t);
                let tys = match *self.tcx.tys.kind(st) {
                    TyKind::Tuple(v) => self.tcx.tys.list(v).to_vec(),
                    _ => {
                        if self.ast().list(elems).iter().any(|x| is_rest(*x)) {
                            self.err(lo, "destructuring assignment with `..` needs a tuple of known length".to_string());
                            return;
                        }
                        let v: Vec<TyId> = self.ast().list(elems).iter().map(|_| self.fresh(0)).collect();
                        let tt = self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&v)));
                        self.demand_eq(st, tt, lo);
                        v
                    }
                };
                let r = self.ast().list(elems).iter().position(|x| is_rest(*x));
                let (n, m) = (elems.len(), tys.len());
                for (i, x) in self.ast().list(elems).iter().enumerate() {
                    if Some(i) == r {
                        continue;
                    }
                    let fi = match r {
                        Some(r) if i > r => m.wrapping_sub(n - i),
                        _ => i,
                    };
                    match tys.get(fi) {
                        Some(ft) => self.check_assignee(*x, *ft, lo),
                        None => self.err(lo, format!("E0308: mismatched types: the tuple has {} elements", m)),
                    }
                }
            }
            ExprKind::Array(elems) => {
                let st = self.shallow(t);
                let rests = self.ast().list(elems).iter().filter(|x| is_rest(**x)).count();
                if rests > 1 { self.err(lo, "`..` can only be used once per slice pattern".to_string()); }
                let fixed = (elems.len() - rests) as u64;
                let et = match self.tcx.tys.kind(st).clone() {
                    TyKind::Array(e, len) => { self.check_array_pat_len(st, e, len, rests > 0, fixed); e }
                    TyKind::Slice(e) => e,
                    _ => {
                        let e = self.fresh(0);
                        let at = self.tcx.tys.intern(TyKind::Array(e, self.tcx.tys.const_usize(elems.len() as u64)));
                        self.eq(st, at);
                        e
                    }
                };
                for x in self.ast().list(elems).iter().copied() {
                    if !is_rest(x) {
                        self.check_assignee(x, et, lo);
                    }
                }
            }
            ExprKind::Call(f, args) if matches!(ast.expr(f).kind, ExprKind::Path(_)) => {
                // `TupleStruct(x, y) = v` / `Variant(x, ..) = v`: fields by position
                let path = match &ast.expr(f).kind {
                    ExprKind::Path(p) => p.clone(),
                    _ => return,
                };
                // the path resolves like a tuple-struct pattern's (aliases keep their written arguments)
                let (r, _) = self.resolve_value_path(&path, a.0 | PAT_KEY, true);
                self.b.res.insert(a.0, r);
                // the constructor path is pattern syntax: region analysis reads its resolution
                self.b.res.insert(f.0, r);
                let ftys = self.ctor_fields_with(r, t, path.lo, Some(a.0 | PAT_KEY), Some(a.0));
                let n = args.len();
                let rest = self.ast().list(args).iter().position(|x| is_rest(*x));
                let nf = ftys.len();
                for (i, x) in self.ast().list(args).iter().enumerate() {
                    if Some(i) == rest {
                        continue;
                    }
                    let fi = match rest {
                        Some(r) if i > r => nf.wrapping_sub(n - i),
                        _ => i,
                    };
                    if let Some(ft) = ftys.get(fi) {
                        self.check_assignee(*x, *ft, lo);
                    }
                }
            }
            ExprKind::Struct(row) => {
                let crate::ast::StructRow { path, fields, .. } = *self.ast().row(row);
                let r = self.resolve_struct_path(&path);
                self.b.res.insert(a.0, r);
                if let Res::Def(d) = r {
                    let (adt_ty, vi) = self.adt_of_ctor(d);
                    let adt_ty = self.instantiate_adt(adt_ty);
                    self.write_adt_user_type(a.0, adt_ty);
                    self.eq(adt_ty, t);
                    for fe in self.ast().list(fields) {
                        let fname = self.sym(fe.name);
                        let fname = if fe.name.hi > fe.name.lo && self.text(fe.name).as_bytes()[0].is_ascii_digit() {
                            crate::ast::tuple_index(self.text(fe.name)).map_or(u32::MAX - 1, |n| n | 0x8000_0000)
                        } else {
                            fname
                        };
                        match self.field_ty(adt_ty, vi, fname) {
                            Some((_, ft)) => self.check_assignee(fe.expr, ft, lo),
                            None => { let what = self.adt_field_owner(adt_ty, vi); self.err(fe.name.lo, format!("E0026: {} does not have a field named `{}`", what, self.text(fe.name))) }
                        }
                    }
                }
            }
            _ => {
                let ta = self.check_expr(a, None);
                self.check_lhs_assignable(a, false);
                self.coerce_or_unify(t, ta, None, lo);
                return;
            }
        }
        self.set_ty(a, t);
    }

    /// Records the std `Index` impl of a range index of an array, slice or `str` in `std_index`.
    fn record_std_index(&mut self, node: u32, base: TyId, index: TyId) {
        let Some(td) = self.lang_item(&["ops", "Index"]) else { return };
        let (base, index) = (self.infcx.deeply_resolve_ignoring_regions(base), self.infcx.deeply_resolve_ignoring_regions(index));
        // an array indexes through its slice impl
        let slice = match self.tcx.tys.kind(base).clone() {
            TyKind::Array(el, _) => self.tcx.tys.intern(TyKind::Slice(el)),
            _ => base,
        };
        let Some(m) = self.trait_fn(td, "index") else { return };
        let self_ty = if matches!(self.tcx.tys.kind(base), TyKind::Array(..)) { base } else { slice };
        // rustc registers `Base: Index<Idx>`: selection types an integer-literal range (`..2`) as usize
        let goal = self.trait_goal(td, self_ty, &[index]);
        self.register_goal(goal, self.last_lo);
        self.b.std_index.insert(node, (m, self.tcx.tys.mk_list(&[self_ty, index])));
    }

    fn check_seq_pats(&mut self, v: &[PatId], elems: &[TyId]) {
        let ast = self.ast();
        let mut rest_at = None;
        for (i, x) in v.iter().enumerate() {
            if let Pat::Rest = ast.pat(*x) {
                rest_at = Some(i);
            }
        }
        match rest_at {
            None => {
                for i in 0..v.len() {
                    let t = if i < elems.len() { elems[i] } else { self.tcx.tys.error };
                    self.check_pat(v[i], t);
                }
            }
            Some(r) => {
                for i in 0..r {
                    let t = if i < elems.len() { elems[i] } else { self.tcx.tys.error };
                    self.check_pat(v[i], t);
                }
                let after = v.len() - r - 1;
                for j in 0..after {
                    let ei = elems.len() as isize - after as isize + j as isize;
                    let t = if ei >= 0 { elems[ei as usize] } else { self.tcx.tys.error };
                    self.check_pat(v[r + 1 + j], t);
                }
            }
        }
    }

    // ------------------------------------------------------------ ADT helpers

    /// For a struct/variant def: (adt type with Param args, variant index).
    fn adt_of_ctor(&mut self, d: DefId) -> (TyId, u32) {
        let def = self.prog.def(d);
        let (adt_def, vi) = match def.kind {
            DefKind::Variant => (def.parent, def.sub),
            _ => (d, 0),
        };
        let args = self.tcx.generic_indices_of(self.prog, adt_def).identity_types_consts(&self.tcx.tys);
        (self.tcx.tys.intern(TyKind::Adt(adt_def, self.tcx.tys.mk_list(&args), self.tcx.adts.get(&adt_def).map_or(crate::types::RegionArgsId::EMPTY, |a| a.region_params))), vi)
    }

    /// Replace Param args of an ADT type by fresh variables.
    /// The type a struct literal / struct pattern path names: explicit generic args
    /// (`S::<u16> { .. }`) and type aliases (`type S4<U> = S3<U, char>; S4::<u8> { .. }`) fix
    /// args; the rest are fresh (rustc instantiates the path's type, alias substituted).
    /// `T::A { a, b }` / `Self::Out { .. }`: a struct named through an associated type of a
    /// param (rustc resolves the path as a type, then takes its ADT and args).
    fn assoc_struct_path(&mut self, path: &Path) -> Option<TyId> {
        let n = path.segs.len();
        if n < 2 || path.has_qself() {
            return None;
        }
        let s0 = self.ast().segs(path.segs)[0].clone();
        if s0.kind() != SegKind::SelfType && !self.env.names.contains(&self.sym(s0.name)) {
            return None;
        }
        if let Some(d) = self.tcx.resolve_type_path_def(self.prog, self.file, self.module, path) {
            if matches!(self.prog.def(d).kind, DefKind::Struct | DefKind::Union | DefKind::Variant | DefKind::TypeAlias) {
                return None;
            }
        }
        // a probe: `Self::Variant { .. }` is not a type (resolve_struct_path then finds the
        // variant), so its lowering errors are kept only when the path is a struct here. They
        // used to reach the shared error sink, where the next collection (a live edit adding
        // items) returned them as that edit's errors: E0412 `cannot find type Self::ExpDecay`.
        let outer = self.tcx.begin_errors(&self.cx);
        let t = self.type_of_prefix(path, n);
        let errs = self.tcx.end_errors(&self.cx, outer);
        let t = self.shallow(t?);
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, _, _) if matches!(self.prog.def(*d).kind, DefKind::Struct | DefKind::Union) => {
                for e in errs {
                    self.tcx.reraise_error(&self.cx, e);
                }
                Some(t)
            }
            _ => None,
        }
    }

    /// The lifetime arguments written on segment `i` of `path`.
    fn seg_lifetimes(&mut self, path: &Path, i: usize) -> Vec<crate::types::RegionId> {
        let mut out = Vec::new();
        let Some(GenericArgs::Angle(v)) = self.ast().segs(path.segs).get(i).and_then(|s| self.ast().gargs(s.args())) else { return out };
        for a in self.ast().list(*v) {
            if let GenericArg::Lifetime(lt) = a {
                out.push(self.env.regions.resolve(Some(&self.infcx), &self.tcx.tys, self.prog, self.file, Some(*lt)));
            }
        }
        out
    }

    /// hir_ty_lowering/generics.rs: the lifetimes written on a struct path (`S::<'a>`) or on a
    /// variant path (`E::V::<'a>`, `E::<'a>::V`) instantiate the ADT's lifetime parameters, in
    /// place of the fresh ones; a wrong count is E0107 (item checks) and leaves them fresh.
    fn written_adt_regions(&mut self, path: &Path, variant: bool, fresh: crate::types::RegionArgsId) -> crate::types::RegionArgsId {
        let n = path.segs.len();
        let mut written = self.seg_lifetimes(path, n.wrapping_sub(1));
        if written.is_empty() && variant { written = self.seg_lifetimes(path, n.wrapping_sub(2)); }
        if written.is_empty() || written.len() != self.tcx.tys.region_args(fresh).len() { return fresh; }
        self.tcx.tys.intern_region_args(&written)
    }

    /// `Enum::<A>::Variant { .. }` / `Enum::Variant::<A> { .. }`: the enum's generic arguments written
    /// on the enum segment or on the variant segment (rustc: the variant path's generic args).
    fn variant_path_ty(&mut self, path: &Path, variant: DefId, fresh_adt: TyId) -> TyId {
        let TyKind::Adt(d, fa, regions) = self.tcx.tys.kind(fresh_adt).clone() else { return fresh_adt };
        let regions = self.written_adt_regions(path, true, regions);
        let fresh_adt = self.tcx.tys.intern(TyKind::Adt(d, fa, regions));
        let n = path.segs.len();
        // `Alias::<A>::Variant { .. }`: the type alias instantiated with its written (else fresh)
        // arguments is the enum type (rustc resolves the alias path as a type)
        if n >= 2 && !path.has_qself() {
            let segs = crate::program::path_segs(self.prog, self.file, path);
            let alias = if n == 2 && segs[0].0 == SegKind::Ident { self.prog.lookup_in_scope(self.module, segs[0].1, true) } else { None };
            if let Some(alias) = alias {
                if self.prog.def(alias).kind == DefKind::TypeAlias {
                    let at = self.tcx.alias_ty(&self.cx.with_infcx(&self.infcx), self.prog, alias);
                    let count = self.tcx.item_env(self.prog, alias).names.len();
                    let mut args = self.seg_args(path, n - 2, alias, 0).unwrap_or_default();
                    while args.len() < count { args.push(self.fresh_arg(alias, args.len())); }
                    args.truncate(count);
                    let nregions = self.tcx.generic_indices_of(self.prog, alias).lifetimes.len();
                    let regions: Vec<_> = (0..nregions).map(|_| self.fresh_region()).collect();
                    let regions = self.tcx.tys.intern_region_args(&regions);
                    let args = self.tcx.complete_args(self.prog, alias, &args, regions);
                    let t = self.tcx.instantiate_ty(self.prog, alias, &args, at);
                    if matches!(self.tcx.tys.kind(t), TyKind::Adt(a, _, _) if *a == d) {
                        // the fresh arguments made for the enum are this type's
                        self.eq(fresh_adt, t);
                        return t;
                    }
                }
            }
        }
        let written = if n >= 2 { self.seg_args(path, n - 2, d, 0) } else { None };
        let Some(a) = written.or_else(|| self.explicit_args(path, variant, 0)) else { return fresh_adt };
        if a.is_empty() || a.len() > self.tcx.tys.list(fa).len() { return fresh_adt; }
        let mut args = a;
        args.extend_from_slice(&self.tcx.tys.list(fa)[args.len()..]);
        self.tcx.tys.intern(TyKind::Adt(d, self.tcx.tys.mk_list(&args), regions))
    }

    fn struct_path_ty(&mut self, path: &Path, fresh_adt: TyId) -> TyId {
        let prog = self.prog;
        if let Some(t) = self.assoc_struct_path(path) {
            return t;
        }
        // `Self { .. }` is the impl's self type itself (not a fresh instance of the same ADT)
        if path.segs.len() == 1 && !path.has_qself() && self.ast().segs(path.segs)[0].kind() == SegKind::SelfType {
            if let Some(st) = self.env.self_ty {
                let st = self.shallow(st);
                if matches!(self.tcx.tys.kind(st), TyKind::Adt(d, _, _) if matches!(self.prog.def(*d).kind, DefKind::Struct | DefKind::Union)) { return st; }
            }
        }
        let segs = crate::program::path_segs(prog, self.file, path);
        if segs.len() == 1 {
            if let Some(d) = prog.lookup_in_scope(self.module, segs[0].1, true) {
                if prog.def(d).kind == DefKind::TypeAlias {
                    let at = self.tcx.alias_ty(&self.cx.with_infcx(&self.infcx), prog, d);
                    let n = self.tcx.item_env(prog, d).names.len();
                    let mut args = self.explicit_args(path, d, 0).unwrap_or_default();
                    while args.len() < n {
                        args.push(self.fresh_arg(d, args.len()));
                    }
                    args.truncate(n);
                    let args = self.infer_item_args(d, &args);
                    let t = self.tcx.instantiate_ty(prog, d, &args, at);
                    if matches!(self.tcx.tys.kind(t), TyKind::Adt(..)) {
                        return t;
                    }
                    return fresh_adt;
                }
            }
        }
        if let TyKind::Adt(d, fa, regions) = self.tcx.tys.kind(fresh_adt).clone() {
            let regions = self.written_adt_regions(path, false, regions);
            let written = self.explicit_args(path, d, 0).filter(|a| !a.is_empty() && a.len() <= self.tcx.tys.list(fa).len());
            let args = match written {
                Some(mut args) => { args.extend_from_slice(&self.tcx.tys.list(fa)[args.len()..]); self.tcx.tys.mk_list(&args) }
                None => fa,
            };
            return self.tcx.tys.intern(TyKind::Adt(d, args, regions));
        }
        fresh_adt
    }

    fn instantiate_adt(&mut self, t: TyId) -> TyId {
        if let TyKind::Adt(d, _, _) = self.tcx.tys.kind(t).clone() {
            let full = self.infcx.fresh_args_for_item(self.prog, d);
            let mut args = Vec::new(); let mut regions = Vec::new();
            for arg in full { match arg {
                crate::consts::GenericArg::Type(t) => args.push(t),
                crate::consts::GenericArg::Const(c) => args.push(self.tcx.tys.intern(TyKind::Const(c))),
                crate::consts::GenericArg::Lifetime(r) => regions.push(r),
            } }
            return self.tcx.tys.intern(TyKind::Adt(d, self.tcx.tys.mk_list(&args), self.tcx.tys.intern_region_args(&regions)));
        }
        t
    }

    /// `S { a: 1, .. }`: the omitted fields of `adt_ty`'s variant `vi` take their declared
    /// defaults, the anonymous consts the parser made (derive.rs default_consts).
    fn struct_defaults(&mut self, e: ExprId, ctor: DefId, adt_ty: TyId, vi: u32, given: &[u32], lo: u32) {
        let t = self.shallow(adt_ty);
        let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(t).clone() else { return };
        let Some(adt) = self.tcx.adts.get(&ad) else { return };
        let Some(v) = adt.variants.get(vi as usize) else { return };
        let ty_name = self.prog.syms.str(self.prog.def(ad).name).to_string();
        let variant = if self.prog.def(ctor).kind == DefKind::Variant { Some(self.prog.syms.str(self.prog.def(ctor).name).to_string()) } else { None };
        let scope = self.prog.def(ad).scope;
        let mut out = Vec::new();
        for (i, f) in v.fields.iter().enumerate() {
            if given.contains(&(i as u32)) {
                continue;
            }
            let fname = self.prog.syms.str(f.name).to_string();
            // a dead slot of a live layout edit (M10) and the fields an edit added (the extension
            // block gives them their Default) need no value
            if fname.starts_with("__relax_dead_") || adt.ext_from != u32::MAX && i as u32 >= adt.ext_from {
                continue;
            }
            let cname = crate::parser::derive::default_const_name(&ty_name, variant.as_deref(), &fname);
            match self.prog.syms.get(&cname).and_then(|s| self.prog.lookup_in_scope(scope, s, false)) {
                Some(cd) => out.push((i as u32, cd)),
                // rustc: E0063 missing field (a bare `..` fills only fields with a default value)
                None => self.err(lo, format!("E0063: missing field `{}` in initializer of `{}`; `..` fills only fields declared with a default value (`{}: T = value`){}", fname, ty_name, fname, if adt.n_generics > 0 { "; default values of generic types are not supported yet" } else { "" })),
            }
        }
        self.b.struct_defaults.insert(e.0, out);
    }

    /// `S { a: 1 }` without `..` or a base: every field needs a value (rustc_hir_typeck
    /// expr.rs check_expr_struct_fields, StructTailExpr::None: E0063 report_missing_fields,
    /// field names sorted, three named then "and N other fields"); a union literal names
    /// exactly one field (E0784).
    fn missing_fields(&mut self, adt_ty: TyId, vi: u32, given: &[u32], n_fields: usize, lo: u32) {
        let t = self.shallow(adt_ty);
        let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(t).clone() else { return };
        let Some(adt) = self.tcx.adts.get(&ad) else { return };
        let Some(v) = adt.variants.get(vi as usize) else { return };
        if adt.is_union {
            if n_fields != 1 { self.err(lo, "E0784: union expressions should have exactly one field".to_string()); }
            return;
        }
        let mut missing: Vec<String> = Vec::new();
        for (i, f) in v.fields.iter().enumerate() {
            if given.contains(&(i as u32)) { continue; }
            let fname = self.prog.syms.str(f.name).to_string();
            // a dead slot of a live layout edit (M10) and the fields an edit added (the extension
            // block gives them their Default) need no value
            if fname.starts_with("__relax_dead_") || adt.ext_from != u32::MAX && i as u32 >= adt.ext_from { continue; }
            missing.push(fname);
        }
        if missing.is_empty() { return; }
        missing.sort();
        let len = missing.len();
        let names = match &missing[..] {
            [a] => format!("`{}`", a),
            [a, b] => format!("`{}` and `{}`", a, b),
            [a, b, c] => format!("`{}`, `{}` and `{}`", a, b, c),
            _ => format!("{} and {} other field{}", missing[..3].iter().map(|n| format!("`{}`", n)).collect::<Vec<_>>().join(", "), len - 3, if len - 3 == 1 { "" } else { "s" }),
        };
        let ty_name = self.ty_str(t);
        self.err(lo, format!("E0063: missing field{} {} in initializer of `{}`", if len == 1 { "" } else { "s" }, names, ty_name));
    }

    /// rustc's name of the struct / variant / union a field list belongs to: "struct `S`",
    /// "variant `E::V`", "union `U`" (E0026, E0559, E0560).
    fn adt_field_owner(&mut self, adt_ty: TyId, vi: u32) -> String {
        let t = self.shallow(adt_ty);
        let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(t).clone() else { return format!("`{}`", self.ty_str(t)) };
        let Some(adt) = self.tcx.adts.get(&ad) else { return format!("`{}`", self.ty_str(t)) };
        let name = self.prog.syms.str(self.prog.def(ad).name).to_string();
        if adt.is_enum {
            let v = adt.variants.get(vi as usize).map_or(String::new(), |v| self.prog.syms.str(v.name).to_string());
            format!("variant `{}::{}`", name, v)
        } else if adt.is_union {
            format!("union `{}`", name)
        } else {
            format!("struct `{}`", name)
        }
    }

    /// rustc's Ty::prefix_string before a type in E0599: "struct ", "enum ", "union ",
    /// "type parameter ", else "type ".
    fn ty_prefix(&mut self, t: TyId) -> &'static str {
        let t = self.shallow(t);
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, _, _) => match self.tcx.adts.get(d) {
                Some(a) if a.is_enum => "enum ",
                Some(a) if a.is_union => "union ",
                _ => "struct ",
            },
            TyKind::Param(_) => "type parameter ",
            _ => "type ",
        }
    }

    fn field_ty(&mut self, adt_ty: TyId, vi: u32, name: Sym) -> Option<(u32, TyId)> {
        let t = self.shallow(adt_ty);
        if let TyKind::Adt(d, args, regions) = self.tcx.tys.kind(t).clone() {
            let adt = self.tcx.adts.get(&d)?;
            let v = adt.variants.get(vi as usize)?;
            for (i, f) in v.fields.iter().enumerate() {
                if f.name == name {
                    let ft = self.instantiate_field_type(f.ty, adt.region_params, regions, self.tcx.tys.list(args));
                    if !adt.is_enum && !self.b.field_uses.contains(&(d, i as u32)) {
                        self.b.field_uses.push((d, i as u32));
                        self.b.dependencies.push(d);
                    }
                    return Some((i as u32, ft));
                }
            }
        }
        None
    }

    /// Field types of a tuple-struct / tuple-variant pattern or constructor, with the path's generic args (`let Foo::<i32>(a, b) = ..`: the
    /// resolution recorded them under `key`).
    fn ctor_fields_with(&mut self, r: Res, expected: TyId, pos: u32, key: Option<u32>, user_node: Option<u32>) -> Vec<TyId> {
        if let Res::Def(d) = r {
            let (adt_ty, vi) = self.adt_of_ctor(d);
            let given = key.and_then(|k| self.b.res_args.get(&k).cloned());
            let adt_ty = self.instantiate_adt(adt_ty);
            let adt_ty = match (self.tcx.tys.kind(adt_ty).clone(), given) {
                (TyKind::Adt(ad, gen, regions), Some(a)) if self.tcx.tys.list(a).len() == self.tcx.tys.list(gen).len() => self.tcx.tys.intern(TyKind::Adt(ad, a, regions)),
                _ => adt_ty,
            };
            // the annotation of the constructor path (an assignee `T::<u8>(a, ..) = t`)
            if let Some(node) = user_node { self.write_adt_user_type(node, adt_ty); }
            self.coerce_or_unify(adt_ty, expected, None, pos);
            let t = self.shallow(adt_ty);
            if let TyKind::Adt(ad, args, regions) = self.tcx.tys.kind(t).clone() {
                if let Some(adt) = self.tcx.adts.get(&ad) {
                    if let Some(v) = adt.variants.get(vi as usize) {
                        let mut out = Vec::new();
                        for f in &v.fields {
                            out.push(self.instantiate_field_type(f.ty, adt.region_params, regions, self.tcx.tys.list(args)));
                        }
                        return out;
                    }
                }
            }
        }
        Vec::new()
    }

    // ------------------------------------------------------------ paths

    fn resolve_struct_path(&mut self, path: &Path) -> Res {
        let prog = self.prog;
        let segs = crate::program::path_segs(prog, self.file, path);
        if segs.len() == 1 && segs[0].0 == SegKind::SelfType {
            if let Some(st) = self.env.self_ty {
                if let TyKind::Adt(d, _, _) = self.tcx.tys.kind(st) {
                    return Res::Def(*d);
                }
            }
            return Res::Err;
        }
        if segs.len() == 1 {
            // a generic parameter shadows an item of the same name
            if segs[0].0 == SegKind::Ident && !path.global() && self.env.names.iter().take(self.b.args.len()).any(|n| *n == segs[0].1) {
                let msg = format!("E0574: expected struct, variant or union type, found type parameter `{}`", prog.syms.str(segs[0].1));
                self.err(path.lo, msg);
                return Res::Err;
            }
            if let Some(d) = prog.lookup_in_scope(self.module, segs[0].1, true) {
                if self.tcx.validates(prog, self.file) && segs[0].0 == SegKind::Ident && !path.global() && path.no_qself() {
                    if let Some(e) = prog.name_ambiguity(self.module, segs[0].1, true, false) { if let Some(m) = crate::lint::privacy_diag(prog, self.file, path.lo, &e) { self.err(path.lo, m); } }
                }
                if matches!(prog.def(d).kind, DefKind::Struct | DefKind::Union) {
                    return Res::Def(d);
                }
                if prog.def(d).kind == DefKind::TypeAlias {
                    let t = self.tcx.alias_ty(&self.cx.with_infcx(&self.infcx), prog, d);
                    if let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(t) {
                        return Res::Def(*ad);
                    }
                }
            }
            if let Some(d) = prog.lookup_in_scope(self.module, segs[0].1, false) {
                if prog.def(d).kind == DefKind::Variant {
                    return Res::Def(d);
                }
            }
            self.err(path.lo, format!("E0422: cannot find struct, variant or union type `{}` in this scope", crate::tcx::path_str(prog, self.file, path)));
            return Res::Err;
        }
        // module::Struct, then Enum::Variant / Self::Variant
        if let Some(d) = self.tcx.resolve_type_path_def(prog, self.file, self.module, path) {
            if matches!(prog.def(d).kind, DefKind::Struct | DefKind::Union | DefKind::Variant) {
                return Res::Def(d);
            }
            // `module::Alias { .. }` (a type alias to a struct, as for a one-segment path)
            if prog.def(d).kind == DefKind::TypeAlias {
                let t = self.tcx.alias_ty(&self.cx.with_infcx(&self.infcx), prog, d);
                if let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(t) {
                    return Res::Def(*ad);
                }
            }
        }
        if let Some(t) = self.assoc_struct_path(path) {
            if let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(t) {
                return Res::Def(*ad);
            }
        }
        // `Alias::Variant { .. }`: the variant of the enum a type alias names (rustc resolves the
        // alias path as a type, then the variant type-relatively)
        if path.segs.len() == 2 && !path.has_qself() {
            let segs = crate::program::path_segs(prog, self.file, path);
            if segs[0].0 == SegKind::Ident {
                if let Some(alias) = prog.lookup_in_scope(self.module, segs[0].1, true).filter(|a| prog.def(*a).kind == DefKind::TypeAlias) {
                    let at = self.tcx.alias_ty(&self.cx.with_infcx(&self.infcx), prog, alias);
                    if let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(at) {
                        if prog.def(*ad).kind == DefKind::Enum {
                            if let Some(v) = prog.lookup_in_container(*ad, segs[1].1, false).filter(|v| prog.def(*v).kind == DefKind::Variant) {
                                return Res::Def(v);
                            }
                        }
                    }
                }
            }
        }
        let (r, _) = self.resolve_value_path(path, u32::MAX, false);
        // rustc_resolve E0574: only a struct, variant or union names a struct literal / pattern
        if let Res::Def(d) = r {
            if !matches!(self.prog.def(d).kind, DefKind::Struct | DefKind::Union | DefKind::Variant) {
                self.err(path.lo, format!("E0574: expected struct, variant or union type, found {} `{}`", crate::tcx::def_kind_descr(self.prog.def(d).kind), crate::tcx::path_str(prog, self.file, path)));
                return Res::Err;
            }
        }
        r
    }

    /// Type of a value def used as an expression (fn items get fresh generic args).
    fn value_def_ty(&mut self, d: DefId, explicit: Option<Vec<TyId>>, key: u32) -> TyId {
        self.value_def_ty_with_self(d, explicit, key, None)
    }

    /// `user_self_ty`: the written receiver of an inherent associated item (`Foo::<'a>::f`), which
    /// has no Self argument to carry it (rustc fn_ctxt/_impl.rs:1034-1094 UserSelfTy).
    fn value_def_ty_with_self(&mut self, d: DefId, explicit: Option<Vec<TyId>>, key: u32, user_self_ty: Option<crate::types::user_type::UserSelfTy>) -> TyId {
        self.b.dependencies.push(d);
        let parent_regions = std::mem::take(&mut self.value_path_parent_regions);
        let def = self.prog.def(d);
        let ty = match def.kind {
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                let n = self.tcx.fn_sig(self.prog, d).map_or(0, |s| s.n_generics);
                let mut args = Vec::new();
                match explicit {
                    Some(v) => {
                        for i in 0..n as usize {
                            if i < v.len() {
                                args.push(v[i]);
                            } else {
                                args.push(self.fresh_arg(d, args.len()));
                            }
                        }
                    }
                    None => {
                        for _ in 0..n {
                            args.push(self.fresh_arg(d, args.len()));
                        }
                    }
                }
                self.b.res_args.insert(key, self.tcx.tys.mk_list(&args));
                // fn_ctxt/_impl.rs:1180-1205 (instantiate_value_path's lifetime
                // args) with hir_ty_lowering/generics.rs:520-580: written lifetimes
                // instantiate the fn's own early-bound lifetimes; with late-bound
                // lifetimes present they are ignored (late_bound_lifetime_arguments)
                // and inferred.
                let written = std::mem::take(&mut self.value_path_lifetimes);
                let indices = self.tcx.generic_indices_of(self.prog, d);
                let parent = self.prog.def(d).parent;
                let parent_lifetimes = if def.kind == DefKind::AssocFn { self.tcx.generic_indices_of(self.prog, parent).lifetimes.len() } else { 0 };
                let own = indices.lifetimes.len() - parent_lifetimes;
                let late_bound = self.tcx.fn_item_sig(self.prog, d).map_or(0, |s| s.bound_regions) > 0;
                let use_written = !written.is_empty() && !late_bound && written.len() == own;
                let parent_regions = parent_regions;
                let mut regions = Vec::new();
                for i in 0..indices.lifetimes.len() {
                    regions.push(match parent_regions.get(i) {
                        Some(&r) if i < parent_lifetimes => r,
                        _ if use_written && i >= parent_lifetimes => written[i - parent_lifetimes],
                        _ => self.fresh_region(),
                    });
                }
                let regions = self.tcx.tys.intern_region_args(&regions);
                // fn_ctxt/_impl.rs:1293-1301 (instantiate_value_path ->
                // add_required_obligations_for_hir): the path's args are created once
                // and its predicates registered with exactly those args.
                let full = self.tcx.complete_args(self.prog, d, &args, regions);
                self.write_user_type_annotation_from_args(key, d, &full, user_self_ty);
                self.apply_fn_bounds(d, &full, self.last_lo);
                self.tcx.tys.intern(TyKind::FnDef(d, self.tcx.tys.mk_list(&args), regions))
            }
            DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic => {
                let t = self.tcx.const_ty(self.prog, d).unwrap_or(self.tcx.tys.error);
                match explicit {
                    Some(mut v) => {
                        let expected = self.tcx.generic_indices_of(self.prog, d).types_consts.len();
                        v.truncate(expected);
                        for _ in v.len()..expected { v.push(self.fresh_arg(d, v.len())); }
                        // the args pick the impl of a trait's const at evaluation (`T::ZERO`)
                        self.b.res_args.insert(key, self.tcx.tys.mk_list(&v));
                        // typeck-audit 1.9 / fn_ctxt/_impl.rs:1293-1301: every value path
                        // registers its item's predicates with the path's args
                        // (`<T as Tr>::C` proves `T: Tr`)
                        if def.kind == DefKind::AssocConst {
                            let indices = self.tcx.generic_indices_of(self.prog, d);
                            // the parent's regions: the written trait lifetimes, or the impl's (inherent)
                            let regions: Vec<_> = indices.lifetimes.iter().enumerate().map(|(i, _)| parent_regions.get(i).copied().unwrap_or_else(|| self.fresh_region())).collect();
                            let regions = self.tcx.tys.intern_region_args(&regions);
                            let full = self.tcx.complete_args(self.prog, d, &v, regions);
                            self.write_user_type_annotation_from_args(key, d, &full, user_self_ty);
                            self.apply_fn_bounds(d, &full, self.last_lo);
                        }
                        self.tcx.tys.subst(t, &v)
                    }
                    None => t,
                }
            }
            DefKind::Variant | DefKind::Struct => {
                let (adt_ty, vi) = self.adt_of_ctor(d);
                let adt_def = match self.tcx.tys.kind(adt_ty) {
                    TyKind::Adt(a, _, _) => *a,
                    _ => NO_DEF,
                };
                let adt = match self.tcx.adts.get(&adt_def) {
                    Some(a) => a,
                    None => return self.tcx.tys.error,
                };
                let mut args = Vec::new();
                let mut regions = Vec::new();
                for _ in &adt.generic_indices.lifetimes { regions.push(self.fresh_region()); }
                // the lifetimes written on the constructor path (`S::<'a>(..)`, `E::V::<'a>(..)`,
                // `E::<'a>::V(..)`) instantiate the ADT's
                let written = if !self.value_path_lifetimes.is_empty() { self.value_path_lifetimes.clone() }
                    else if self.prog.def(d).kind == DefKind::Variant { self.value_path_enum_lifetimes.clone() } else { Vec::new() };
                if written.len() == regions.len() { regions = written; }
                let regions = self.tcx.tys.intern_region_args(&regions);
                match explicit {
                    Some(v) => {
                        args = v;
                        // hir_ty_lowering/generics.rs: parameters left out of explicit arguments take
                        // their defaults (`Uwu::<N>` for `Uwu<const N: u32 = 1, const M: u32 = N>`)
                        if args.len() < adt.n_generics as usize {
                            let cx = self.cx.with_infcx(&self.infcx);
                            let _ = self.tcx.fill_type_defaults(&cx, self.prog, adt_def, &mut args, Some(regions));
                        }
                        // a written list of the wrong length is E0107 (item checks); the ADT still
                        // takes exactly its declared parameters
                        args.truncate(adt.n_generics as usize);
                        for _ in args.len()..adt.n_generics as usize { args.push(self.fresh_arg(d, args.len())); }
                    }
                    None => {
                        for _ in 0..adt.n_generics {
                            args.push(self.fresh_arg(d, args.len()));
                        }
                    }
                }
                self.b.res_args.insert(key, self.tcx.tys.mk_list(&args));
                let full = self.tcx.complete_args(self.prog, adt_def, &args, regions);
                self.write_user_type_annotation_from_args(key, d, &full, user_self_ty);
                self.apply_fn_bounds(adt_def, &full, self.last_lo);
                let inst = self.tcx.tys.intern(TyKind::Adt(adt_def, self.tcx.tys.mk_list(&args), regions));
                let v = &adt.variants[vi as usize];
                if v.shape == 1 && !adt.is_union {
                    // tuple ctor: a fn item of its own (rustc: FnDef with the ADT's args, which
                    // keeps the written lifetimes; it reifies to `fn(fields..) -> Adt` on demand)
                    self.tcx.tys.intern(TyKind::FnDef(d, self.tcx.tys.mk_list(&args), regions))
                } else {
                    inst
                }
            }
            _ => self.tcx.tys.error,
        };
        // rustc check_expr_path only: a constructor named in a pattern has no Sized inputs
        if key & PAT_KEY == 0 { self.defer_fn_item_sized_obligations(ty, self.last_lo); }
        ty
    }

    /// `<T>::item` (no trait): on `dyn Trait`, the trait's (or a supertrait's) item with
    /// Self = the trait object (a virtual call); otherwise T's inherent item.
    fn resolve_qself_no_trait(&mut self, self_t: TyId, name: Sym, key: u32, lo: u32, path: &Path) -> (Res, TyId) {
        let error = self.tcx.tys.error;
        let st = self.shallow(self_t);
        // `<Self>::Variant(..)`, `Alias::Variant`: an enum's variant (rustc
        // resolve_fully_qualified_call looks up enum variants before probing: they win over inherent items)
        if let TyKind::Adt(ad, aargs, _) = self.tcx.tys.kind(st).clone() {
            if self.prog.def(ad).kind == DefKind::Enum {
                if let Some(v) = self.prog.lookup_in_container(ad, name, false) {
                    if self.prog.def(v).kind == DefKind::Variant {
                        let t = self.value_def_ty(v, Some(self.tcx.tys.list(aargs).to_vec()), key);
                        return (Res::Def(v), t);
                    }
                }
            }
        }
        if let TyKind::Dyn(td, targs, _, _, _, _, _) = self.tcx.tys.kind(st).clone() {
            let item = match self.dyn_method(td, name, lo) {
                Ok(item) => item,
                Err(()) => return (Res::Err, error),
            };
            if let Some(item) = item {
                let mut args = vec![st];
                let parent = self.prog.def(item).parent;
                if parent == td {
                    args.extend(self.tcx.tys.list(targs).iter().copied());
                } else if let Some(sa) = self.dyn_super_args(td, self.tcx.tys.list(targs), st, parent) {
                    args.extend(sa);
                }
                let sig = self.tcx.fn_sig(self.prog, item).cloned();
                let np = sig.as_ref().map_or(1, |s| s.n_parent_generics) as usize;
                while args.len() < np {
                    args.push(self.fresh_arg(item, args.len()));
                }
                let own = sig.map_or(0, |s| s.n_generics - s.n_parent_generics);
                let given = self.explicit_args(path, item, args.len()).unwrap_or_default();
                for i in 0..own as usize {
                    args.push(given.get(i).copied().unwrap_or_else(|| self.fresh_arg(item, args.len())));
                }
                let t = self.value_def_ty(item, Some(args), key);
                return (Res::Def(item), t);
            }
        }
        if let Some((item, imp)) = self.assemble_inherent_impl_probe(st, name) {
            let mut args = self.impl_args_for_value_path(imp, st);
            let own = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_generics - s.n_parent_generics);
            let given = self.explicit_args(path, item, args.len()).unwrap_or_default();
            for i in 0..own as usize {
                args.push(given.get(i).copied().unwrap_or_else(|| self.fresh_arg(item, args.len())));
            }
            let user_self = crate::types::user_type::UserSelfTy { impl_def_id: imp, self_ty: self_t };
            let t = self.value_def_ty_with_self(item, Some(args), key, Some(user_self));
            return (Res::Def(item), t);
        }
        // `<*mut Object>::encode()`: like `T::item`, items of traits in scope count too
        // (rustc_hir_typeck method/mod.rs resolve_fully_qualified_call probes the traits in scope)
        let cands = self.scoped_trait_methods(st, name);
        if cands.iter().any(|c| self.prog.def(c.0).parent != self.prog.def(cands[0].0).parent) {
            self.err(lo, "E0034: multiple applicable items in scope".into()); return (Res::Err, error);
        }
        if let Some(&(item, imp)) = cands.first() {
            let ambiguous = cands.iter().filter(|c| c.0 == item).count() > 1;
            let imp = if ambiguous { crate::program::NO_DEF } else { imp };
            let offset = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_parent_generics) as usize;
            let given = self.explicit_args(path, item, offset).unwrap_or_default();
            if crate::lint::enabled() { self.b.trait_item_paths.insert(key, item); }
            let args = self.trait_call_args(item, imp, st, &given);
            let t = self.value_def_ty(item, Some(args), key);
            return (Res::Def(item), t);
        }
        // a rigid projection or param of a generic body (`<A as Alloc<T>>::Mem::default()`):
        // through its bounds
        if matches!(self.tcx.tys.kind(st), TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) | TyKind::Param(_)) {
            if let Some((item, td_args)) = self.trait_item_for(st, name) {
                let mut args = vec![st];
                args.extend(td_args);
                let own = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_generics - s.n_parent_generics);
            let given = self.explicit_args(path, item, args.len()).unwrap_or_default();
                for i in 0..own as usize {
                    args.push(given.get(i).copied().unwrap_or_else(|| self.fresh_arg(item, args.len())));
                }
                let t = self.value_def_ty(item, Some(args), key);
                return (Res::Def(item), t);
            }
        }
        // `<i32>::ONE`, `MyType::CONST`: an associated const of a trait in scope that the type
        // implements (the probe covers all associated items, not only methods)
        if let Some(r) = self.scoped_assoc_const(st, name, key) {
            return r;
        }
        let ts = self.ty_str(st);
        let kind = self.ty_prefix(st);
        self.err(lo, format!("E0599: no function or associated item named `{}` found for {}`{}` in the current scope -- name the trait: `<{} as Trait>::{}`", self.prog.syms.str(name), kind, ts, ts, self.prog.syms.str(name)));
        (Res::Err, error)
    }

    /// `<i32>::ONE`, `Option::DEFAULT`: an associated const of a trait in scope with exactly
    /// one impl that can apply to `st`.
    fn scoped_assoc_const(&mut self, st: TyId, name: Sym, key: u32) -> Option<(Res, TyId)> {
        if !matches!(self.tcx.tys.kind(st), TyKind::Infer(_) | TyKind::Error) {
            let mut traits = self.traits_in_scope();
            traits.extend(self.param_bound_traits(st));
            let mut seen = crate::fxhash::HashSet::default();
            traits.retain(|t| seen.insert(*t));
            let mut hits: Vec<(DefId, DefId)> = Vec::new();
            for td in traits {
                let it = self.prog.traits[self.prog.def(td).sub as usize].iter().copied().find(|&it| self.prog.def(it).name == name && self.prog.def(it).kind == DefKind::AssocConst);
                if let Some(it) = it {
                    let mark = self.start_snapshot();
                    let selected = self.select_unspecified_trait(td, st);
                    self.rollback_to(mark);
                    let imp = match selected {
                        Ok(Some(selected)) => match &selected { ImplSource::UserDefined { impl_def_id, .. } => *impl_def_id, _ => NO_DEF },
                        Ok(None) => crate::program::NO_DEF,
                        Err(_) => continue,
                    };
                    hits.push((it, imp));
                }
            }
            if hits.len() == 1 {
                let (item, imp) = hits[0];
                let args = self.trait_call_args(item, imp, st, &[]);
                let full = self.infer_item_args(item, &args);
                self.register_method_obligation(item, &full);
                let t = self.value_def_ty(item, Some(args), key);
                return Some((Res::Def(item), t));
            }
        }
        None
    }

    /// Resolves a value path. `key`: expr or pat id for recording generic args.
    fn resolve_value_path(&mut self, path: &Path, key: u32, is_pat: bool) -> (Res, TyId) {
        let r = self.resolve_value_path_inner(path, key, is_pat);
        if let Res::Def(d) = r.0 {
            self.check_ctor_privacy(d, path.lo);
            if matches!(self.prog.def(d).kind, DefKind::AssocFn | DefKind::AssocConst) {
                self.check_legal_trait_for_method_call(d, path.lo);
            }
        }
        r
    }

    /// A tuple struct's constructor is as private as its most private field (rustc_resolve
    /// build_reduced_graph: ctor visibility; a path to it is E0603, a call E0423).
    fn check_ctor_privacy(&mut self, d: DefId, pos: u32) {
        if !self.tcx.validates(self.prog, self.file) { return; }
        if pos >= self.prog.files[self.file as usize].src_len { return; }
        if self.prog.def(d).kind != DefKind::Struct || self.value_ctor_shape(d) != Some(1) {
            return;
        }
        let Some(adt) = self.tcx.adts.get(&d) else { return };
        let hidden = adt.variants.first().map_or(false, |v| v.fields.iter().any(|f| !self.field_visible(d, f)));
        if hidden {
            let msg = format!("E0603: tuple struct constructor `{}` is private", self.prog.name(d));
            self.err(pos, msg);
        }
    }

    /// rustc_hir_typeck/callee.rs check_legal_trait_for_method_call: the `Drop` trait's method is never callable.
    fn check_legal_trait_for_method_call(&mut self, item: DefId, pos: u32) {
        let parent = self.prog.def(item).parent;
        if parent == NO_DEF { return; }
        let Some(drop) = self.tcx.lang_items.get(crate::lang_items::LangItem::Drop) else { return };
        let trait_id = match self.prog.def(parent).kind {
            DefKind::Trait => parent,
            DefKind::Impl => match self.tcx.impl_trait.get(&parent) { Some(t) => *t, None => return },
            _ => return,
        };
        if trait_id == drop {
            self.err(pos, "E0040: explicit use of destructor method".to_string());
        }
    }

    fn resolve_value_path_inner(&mut self, path: &Path, key: u32, is_pat: bool) -> (Res, TyId) {
        let prog = self.prog;
        // hir_ty_lowering/generics.rs lower_generic_args: the item's own written
        // lifetime arguments (last segment).
        self.value_path_lifetimes = self.seg_lifetimes(path, path.segs.len().wrapping_sub(1));
        self.value_path_enum_lifetimes = self.seg_lifetimes(path, path.segs.len().wrapping_sub(2));
        let segs = crate::program::path_segs(prog, self.file, path);
        let error = self.tcx.tys.error;
        // an anonymous constant cannot name a generic `Self`
        if self.in_anon_const && self.tcx.validates(prog, self.file) && segs.first().is_some_and(|s| s.0 == SegKind::SelfType) {
            if let Some(st) = self.env.self_ty {
                let generic = match self.tcx.tys.kind(st) {
                    TyKind::Adt(_, args, regions) => !self.tcx.tys.list(*args).is_empty() || !self.tcx.tys.region_args(*regions).is_empty(),
                    _ => self.tcx.tys.flags(st).contains(crate::types::TypeFlags::HAS_TY_PARAM),
                };
                if generic {
                    self.err(path.lo, "generic `Self` types are currently not permitted in anonymous constants".to_string());
                    return (Res::Err, error);
                }
            }
        }
        if self.tcx.validates(prog, self.file) && segs.len() == 1 && path.no_qself() && !path.global() && segs[0].0 == SegKind::Ident && !self.scopes.iter().any(|(sy, _)| *sy == segs[0].1) && !self.env.names.contains(&segs[0].1) {
            if let Some(e) = prog.name_ambiguity(self.module, segs[0].1, false, false) {
                if let Some(m) = crate::lint::privacy_diag(prog, self.file, path.lo, &e) { self.err(path.lo, m); }
            }
        }
        {
            let cx = self.cx.with_infcx(&self.infcx);
            if self.tcx.prohibit_module_args(&cx, prog, self.file, self.module, path) { return (Res::Err, error); }
        }
        if let Some(q) = self.ast().qself(path) {
            // `<T as Trait>::item`
            let self_t = self.lower_ty(q.ty);
            let tp = match &q.trait_path {
                Some(tp) => tp,
                None => return self.resolve_qself_no_trait(self_t, segs[segs.len() - 1].1, key, path.lo, path),
            };
            // `<T as Trait>::Assoc::item`: the qualified part names a type (a projection),
            // the last segment an item on it, resolved type-relatively as `<Ty>::item` (rustc
            // resolve_qpath with a partial resolution: the unresolved segments go to
            // hir_ty_lowering's associated-path lowering)
            if segs.len() >= 2 {
                let sub = Path { segs: path.segs.prefix(path.segs.len() - 1), lo: path.lo };
                let env = self.env.clone();
                let t = self.tcx.lower_path_ty(&self.cx.with_infcx(&self.infcx), prog, self.file, self.module, &env, &sub, None);
                // (a written qualified self type must be well formed, like any written type)
                if self.tcx.validates(self.prog, self.file) && !self.tcx.tys.has_error(t) { self.register_wf(t, path.lo); }
                let t = self.norm(t);
                if matches!(self.tcx.tys.kind(t), TyKind::Error) {
                    return (Res::Err, error);
                }
                return self.resolve_qself_no_trait(t, segs[segs.len() - 1].1, key, path.lo, path);
            }
            let td = match self.tcx.resolve_type_path_def(prog, self.file, self.module, tp) {
                Some(d) => d,
                None => return (Res::Err, error),
            };
            // the trait path of `<T as a::b::Trait>::item` is privacy-checked like any path
            if self.tcx.validates(prog, self.file) && tp.segs.len() >= 2 && tp.no_qself() && tp.lo < prog.files[self.file as usize].src_len {
                let tsegs = crate::program::path_segs(prog, self.file, tp);
                if let Some(e) = prog.path_privacy(self.module, tp.global(), &tsegs, Some(true), false, false) {
                    if let Some(m) = crate::lint::privacy_diag(prog, self.file, tp.lo, &e) { self.err(tp.lo, m); }
                }
            }
            let name = segs[segs.len() - 1].1;
            let mut item = None;
            for &it in &prog.traits[prog.def(td).sub as usize] {
                if prog.def(it).name == name {
                    item = Some(it);
                }
            }
            let item = match item {
                Some(i) => i,
                None => {
                    self.err(path.lo, "no such trait item".to_string());
                    return (Res::Err, error);
                }
            };
            let env = self.env.clone();
            let explicit = self.tcx.lower_generic_args(&self.cx.with_infcx(&self.infcx), prog, self.file, self.module, &env, tp);
            // `<T as Trait<_>>`: an inferred trait argument is an inference variable
            let written: Vec<crate::ast::TyId> = match self.ast().segs(tp.segs).last().and_then(|s| self.ast().gargs(s.args())) {
                Some(GenericArgs::Angle(v)) => self.ast().list(*v).iter().filter_map(|g| match g {
                    GenericArg::Type(t) => Some(*t),
                    _ => None,
                }).collect(),
                _ => Vec::new(),
            };
            let mut explicit2 = Vec::new();
            for (i, e) in explicit.into_iter().enumerate() {
                let infer = written.get(i).map_or(false, |w| matches!(self.ast().ty(*w), crate::ast::Ty::Infer));
                if infer {
                    let v = self.fresh_arg(td, i + 1);
                    explicit2.push(v);
                } else {
                    explicit2.push(e);
                }
            }
            let targs = self.tcx.fill_trait_defaults(&self.cx.with_infcx(&self.infcx), prog, td, self_t, explicit2);
            let mut args = vec![self_t];
            args.extend(targs);
            // the item's own args from the path (`<S as T<123>>::l::<true>()`)
            let given = self.explicit_args(path, item, args.len()).unwrap_or_default();
            let own = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_generics - s.n_parent_generics);
            for i in 0..own as usize {
                args.push(if i < given.len() { given[i] } else { self.fresh_arg(item, args.len()) });
            }
            // `<T as Trait<'a>>::item`: the trait's written lifetimes are the item's parent regions
            let trait_regions = self.tcx.lower_region_args(Some(&self.infcx), prog, self.file, self.module, &env, tp);
            self.value_path_parent_regions = self.tcx.tys.region_args(trait_regions).to_vec();
            let t = self.value_def_ty(item, Some(args), key);
            return (Res::Def(item), t);
        }
        if segs.len() == 1 && !path.global() {
            let (k, s) = segs[0];
            if k == SegKind::SelfValue || k == SegKind::Ident {
                if !is_pat {
                    if let Some(li) = self.lookup_local(s, self.ast().segs(path.segs)[0].name.lo) {
                        // prohibit_generic_args: a local variable takes no arguments (E0109)
                        let seg = &self.ast().segs(path.segs)[0];
                        let cx = self.cx.with_infcx(&self.infcx);
                        if self.tcx.prohibit_args_on(&cx, prog, self.file, seg, "local variable", self.text(seg.name)) { return (Res::Err, error); }
                        if self.inline_const_floor.last().is_some_and(|&f| li < f) && self.tcx.validates(prog, self.file) {
                            self.err(path.lo, "E0435: attempt to use a non-constant value in a constant".to_string());
                            return (Res::Err, error);
                        }
                        self.note_capture(li, key);
                        let t = self.b.locals[li as usize].ty;
                        return (Res::Local(li), t);
                    }
                }
                // rustc_resolve/src/late.rs value ribs: block items precede
                // enclosing item parameters (normal module items follow them).
                let mut scope = self.module;
                while let Some(parent) = prog.mods[scope as usize].lexical_parent {
                    if let Some(d) = prog.lookup_in_module(scope, s, false, &mut Vec::new()) {
                        let explicit = self.explicit_args(path, d, 0);
                        let t = self.value_def_ty(d, explicit, key);
                        return (Res::Def(d), t);
                    }
                    scope = parent;
                }
                // a const generic param as a value: the instance's const arg, its declared type
                if let Some(&(i, cfile, cty)) = self.env.consts.iter().find(|c| self.env.names.get(c.0 as usize) == Some(&s)) {
                    let env = self.env.clone();
                    let t = self.tcx.lower_ty(&self.cx.with_infcx(&self.infcx), prog, cfile, self.module, &env, cty);
                    return (Res::ConstParam(i), t);
                }
            }
            if k == SegKind::SelfType {
                // `Self` as a unit/tuple struct ctor
                if let Some(st) = self.env.self_ty {
                    if let TyKind::Adt(d, args, _) = self.tcx.tys.kind(st).clone() {
                        let seg = &self.ast().segs(path.segs)[0];
                        let cx = self.cx.with_infcx(&self.infcx);
                        if self.tcx.prohibit_args_on(&cx, prog, self.file, seg, "self constructor", "Self") { return (Res::Err, error); }
                        let t = self.value_def_ty(d, Some(self.tcx.tys.list(args).to_vec()), key);
                        return (Res::Def(d), t);
                    }
                } else if let Some(imp) = self.env.owner.and_then(|o| outer_impl(prog, o)) {
                    // Res::SelfCtor of an enclosing impl, reached from a nested item (rustc never
                    // validates it in resolve; hir_typeck fn_ctxt/_impl.rs: an error when the
                    // impl's self type has parameters, else the self_constructor_from_outer_item lint)
                    if let Some(&st) = self.tcx.impl_self.get(&imp) {
                        if self.tcx.has_param(st) {
                            self.err(path.lo, "E0401: can't reference `Self` constructor from outer item".to_string());
                            return (Res::Err, error);
                        }
                        if let TyKind::Adt(d, args, _) = self.tcx.tys.kind(st).clone() {
                            let t = self.value_def_ty(d, Some(self.tcx.tys.list(args).to_vec()), key);
                            return (Res::Def(d), t);
                        }
                    }
                }
            }
            if let Some(d) = prog.lookup_in_scope(self.module, s, false) {
                let explicit = self.explicit_args(path, d, 0);
                let t = self.value_def_ty(d, explicit, key);
                return (Res::Def(d), t);
            }
            if path.segs.len() == 1 && !path.global() {
                if let Some(d) = prog.relaxed_import(self.module, s, false, self.file, path.lo) {
                    let explicit = self.explicit_args(path, d, 0);
                    let t = self.value_def_ty(d, explicit, key);
                    return (Res::Def(d), t);
                }
            }
            if path.segs.len() == 1 && !path.global() && prog.syms.str(s) == "self" {
                // `self` where no method's receiver is visible (a macro's `self` is hygienic)
                // (reported where the macro body wrote it)
                let at = self.prog.files[self.file as usize].lint_pos(path.lo);
                self.err(at, "E0424: expected value, found module `self`".to_string());
                return (Res::Err, error);
            }
            let help = import_help(prog, self.module, s, false);
            self.err(path.lo, format!("E0425: cannot find value `{}` in this scope{}", crate::tcx::path_str(prog, self.file, path), help));
            return (Res::Err, error);
        }
        let n = segs.len();
        // `::f` (2015: a path from the crate root)
        if n == 1 && path.global() && prog.crates[prog.mods[self.module as usize].krate as usize].edition < 2018 {
            let krate = prog.mods[self.module as usize].krate;
            let root = prog.mods[prog.crates[krate as usize].root_mod as usize].def;
            if let Some(d) = prog.lookup_in_container(root, segs[0].1, false) {
                let explicit = self.explicit_args(path, d, 0);
                let t = self.value_def_ty(d, explicit, key);
                return (Res::Def(d), t);
            }
        }
        if self.tcx.validates(prog, self.file) && path.no_qself() {
            if let Some(e) = prog.path_privacy(self.module, path.global(), &segs, Some(false), false, false).filter(|_| path.lo < prog.files[self.file as usize].src_len) {
                if let Some(m) = crate::lint::privacy_diag(prog, self.file, path.lo, &e) { self.err(path.lo, m); }
            }
        }
        // container path (module / enum / trait)
        let parent = if segs[0].0 == SegKind::SelfType && n == 2 {
            None
        } else {
            prog.resolve_mod_path(self.module, path.global(), &segs[..n - 1], false, false)
        };
        if let Some(pd) = parent {
            let pk = prog.def(pd).kind;
            if matches!(pk, DefKind::Mod | DefKind::Enum | DefKind::Trait) {
                if let Some(d) = prog.lookup_in_container(pd, segs[n - 1].1, false) {
                    let offset = if pk == DefKind::Trait { self.tcx.fn_sig(self.prog, d).map_or(0, |s| s.n_parent_generics) as usize } else { 0 };
                    let last_args = self.explicit_args(path, d, offset);
                    // enum variant: generic args may sit on the enum segment
                    let explicit = if pk == DefKind::Enum {
                        self.seg_args(path, n - 2, pd, if pk == DefKind::Trait { 1 } else { 0 }).or(last_args)
                    } else if pk == DefKind::Trait && matches!(prog.def(d).kind, DefKind::AssocFn) && (last_args.is_some() || self.seg_args(path, n - 2, pd, if pk == DefKind::Trait { 1 } else { 0 }).is_some()) {
                        // `Trait::<A>::m::<B>`: [Self (open), the trait's args, the fn's own]
                        let nt = self.tcx.trait_generic_count(prog, pd);
                        let ta = self.seg_args(path, n - 2, pd, if pk == DefKind::Trait { 1 } else { 0 }).unwrap_or_default();
                        let mut v = vec![self.fresh(0)];
                        for i in 0..nt {
                            v.push(if i < ta.len() { ta[i] } else { self.fresh_arg(pd, v.len()) });
                        }
                        v.extend(last_args.unwrap_or_default());
                        Some(v)
                    } else if pk == DefKind::Trait && matches!(prog.def(d).kind, DefKind::AssocConst) {
                        // `Trait::CONST`: [Self (open, inferred from the use), the trait's args];
                        // the impl's const is picked once Self is known (rustc: a projection)
                        let nt = self.tcx.trait_generic_count(prog, pd);
                        let ta = self.seg_args(path, n - 2, pd, if pk == DefKind::Trait { 1 } else { 0 }).unwrap_or_default();
                        let mut v = vec![self.fresh(0)];
                        for i in 0..nt {
                            v.push(if i < ta.len() { ta[i] } else { self.fresh_arg(pd, v.len()) });
                        }
                        Some(v)
                    } else {
                        last_args
                    };
                    let t = self.value_def_ty(d, explicit, key);
                    return (Res::Def(d), t);
                }
                // `str::to_owned` with a module `str` in scope (a glob of std): a first segment
                // naming a primitive type falls back to the type when the module lacks the item
                // (rustc_resolve late.rs resolve_qpath, "required for backward compatibility")
                let prim_first = n == 2 && !path.global() && crate::program::PRIMS.iter().any(|(nm, _)| prog.syms.get(nm) == Some(segs[0].1));
                if pk != DefKind::Enum && pk != DefKind::Trait && !prim_first {
                    self.err(path.lo, format!("E0425: cannot find value `{}` in this scope", crate::tcx::path_str(prog, self.file, path)));
                    return (Res::Err, error);
                }
            }
        }
        // type-relative: <Type>::item
        let self_ty = match self.type_of_prefix(path, n - 1) {
            Some(t) => t,
            None => {
                // a missing first segment: where it exists (E0433, rustc's "failed to resolve")
                let help = if n == 2 && !path.global() { import_help(prog, self.module, self.sym(self.ast().segs(path.segs)[0].name), true) } else { String::new() };
                let first = crate::tcx::path_str(prog, self.file, path);
                let first = first.rsplit_once("::").map_or(first.as_str(), |(a, _)| a).to_string();
                self.err(path.lo, format!("E0433: failed to resolve: use of undeclared type `{}`{}", first, help));
                return (Res::Err, error);
            }
        };
        // (rustc probe_for_name / resolve_fully_qualified_call: the self type is structurally resolved --
        // an alias such as `type Foo<T> = <Option<T> as Mirror>::Assoc` names the type it normalizes to)
        let self_ty = self.shallow(self_ty);
        // the next solver proves the self type well formed before the lookup (its WF goal may constrain the
        // inference variables the written type mentions: `Alias::foo()` with `Foo<?t, <() as Constrain<?t>>::Assoc>`)
        if self.tcx.tys.has_infer(self.infcx.deeply_resolve_ignoring_regions(self_ty)) {
            self.register_wf(self_ty, path.lo);
            self.retry_goals(false);
        }
        let name = segs[n - 1].1;
        // enum variant through a type alias / Self
        if let TyKind::Adt(ad, aargs, _) = self.tcx.tys.kind(self_ty).clone() {
            if prog.def(ad).kind == DefKind::Enum {
                if let Some(v) = prog.lookup_in_container(ad, name, false) {
                    let t = self.value_def_ty(v, Some(self.tcx.tys.list(aargs).to_vec()), key);
                    return (Res::Def(v), t);
                }
            }
        }
        // `T::item` with T a generic param (or `Self` in a trait body): resolved through the
        // param's bounds only, as rustc does on the generic body; an inherent item of the type
        // a given instance substitutes is never seen (`U::from(x)` with U: From<T> calls
        // From::from even when the instance's U has an inherent `from`).
        // rustc_hir_analysis hir_ty_lowering lower_assoc_path / probe_single_bound_for_assoc_item
        let prefix_is_param = n == 2
            && path.no_qself()
            && match self.ast().segs(path.segs)[0].kind() {
                SegKind::SelfType => self.env.trait_def.is_some(),
                _ => {
                    let s0 = self.sym(self.ast().segs(path.segs)[0].name);
                    self.env.names.iter().take(self.b.args.len()).any(|nm| *nm == s0)
                }
            };
        if prefix_is_param {
            if let Some((item, td_args)) = self.trait_item_for(self_ty, name) {
                let mut args = vec![self_ty];
                args.extend(td_args);
                let offset = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_parent_generics) as usize;
                let la = self.explicit_args(path, item, offset).unwrap_or_default();
                let sig = self.tcx.fn_sig(self.prog, item).cloned();
                let own = sig.map_or(0, |s| s.n_generics - s.n_parent_generics);
                for i in 0..own as usize {
                    args.push(if i < la.len() { la[i] } else { self.fresh_arg(item, args.len()) });
                }
                if crate::lint::enabled() { self.b.trait_item_paths.insert(key, item); }
                let t = self.value_def_ty(item, Some(args), key);
                return (Res::Def(item), t);
            }
        }
        match self.assemble_inherent_impl_probe(self_ty, name) {
            Some((item, imp)) => {
                self.check_assoc_privacy(item, path.lo);
                // impl generic args from matching the impl self type against self_ty
                let impl_args = self.impl_args_for_value_path(imp, self_ty);
                let own = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_generics - s.n_parent_generics);
                let mut args = impl_args;
                let offset = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_parent_generics) as usize;
                let la = self.explicit_args(path, item, offset).unwrap_or_default();
                for i in 0..own as usize {
                    if i < la.len() {
                        args.push(la[i]);
                    } else {
                        args.push(self.fresh_arg(item, args.len()));
                    }
                }
                let user_self = crate::types::user_type::UserSelfTy { impl_def_id: imp, self_ty };
                let t = self.value_def_ty_with_self(item, Some(args), key, Some(user_self));
                (Res::Def(item), t)
            }
            None => {
                // a trait method implemented for this type
                let cands = self.extension_candidates(self_ty, name, DefKind::AssocFn);
                if cands.iter().any(|c| self.prog.def(c.0).parent != self.prog.def(cands[0].0).parent) {
                    self.err(path.lo, "E0034: multiple applicable items in scope".into()); return (Res::Err, error);
                }
                if let Some(&(item, imp)) = cands.first() {
                    let offset = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_parent_generics) as usize;
                let la = self.explicit_args(path, item, offset).unwrap_or_default();
                    // several impls of the trait for this type (`Rc::from` from &str, String,
                    // Box..): leave the trait args open; the impl is chosen once they are known
                    let ambiguous = cands.iter().filter(|c| c.0 == item).count() > 1;
                    let imp = if ambiguous { crate::program::NO_DEF } else { imp };
                    if crate::lint::enabled() { self.b.trait_item_paths.insert(key, item); }
                    let args = self.trait_call_args(item, imp, self_ty, &la);
                    let t = self.value_def_ty(item, Some(args), key);
                    return (Res::Def(item), t);
                }
                // an associated const / fn of a trait: through the body's bounds (`T::ZERO` with
                // T: Zeroable, checked as the instance type), else a trait in scope implemented
                // for the type
                if let Some((item, td_args)) = self.trait_item_for(self_ty, name) {
                    let mut args = vec![self_ty];
                    args.extend(td_args);
                    let offset = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_parent_generics) as usize;
                let la = self.explicit_args(path, item, offset).unwrap_or_default();
                    let sig = self.tcx.fn_sig(self.prog, item).cloned();
                    let own = sig.map_or(0, |s| s.n_generics - s.n_parent_generics);
                    for i in 0..own as usize {
                        args.push(if i < la.len() { la[i] } else { self.fresh_arg(item, args.len()) });
                    }
                    let t = self.value_def_ty(item, Some(args), key);
                    return (Res::Def(item), t);
                }
                if let Some(r) = self.scoped_assoc_const(self_ty, name, key) {
                    return r;
                }
                let ts = self.ty_str(self_ty);
                let kind = self.ty_prefix(self_ty);
                self.err(path.lo, format!("E0599: no function or associated item named `{}` found for {}`{}` in the current scope", prog.syms.str(name), kind, ts));
                (Res::Err, error)
            }
        }
    }

    /// rustc_hir_typeck/src/method/probe.rs:1772-1843,2371-2406:
    /// collect applicable candidates and collapse only a shared trait container.
    fn trait_item_for(&mut self, t: TyId, name: Sym) -> Option<(DefId, Vec<TyId>)> {
        let mut candidates = Vec::new();
        for (td, args) in self.param_bounds(t) {
            for &item in &self.prog.traits[self.prog.def(td).sub as usize] {
                // value namespace: an associated type of the same name is another item
                if self.prog.def(item).name == name && matches!(self.prog.def(item).kind, DefKind::AssocFn | DefKind::AssocConst) {
                    let c = (item, args.clone());
                    if !candidates.contains(&c) { candidates.push(c); }
                }
            }
        }
        if candidates.is_empty() {
            for td in self.traits_in_scope() {
                for &item in &self.prog.traits[self.prog.def(td).sub as usize] {
                    if self.prog.def(item).name != name || !matches!(self.prog.def(item).kind, DefKind::AssocFn | DefKind::AssocConst) { continue; }
                    let save = self.start_snapshot();
                    let mut args = Vec::new();
                    for i in 1..=self.tcx.trait_generic_count(self.prog, td) { args.push(self.fresh_arg(td, i as usize)); }
                    let applicable = self.select_trait(td, t, &args).is_ok();
                    self.rollback_to(save);
                    if applicable && !candidates.iter().any(|c| c.0 == item) { candidates.push((item, Vec::new())); }
                }
            }
        }
        let first = candidates.first()?.0;
        if candidates.iter().any(|c| self.prog.def(c.0).parent != self.prog.def(first).parent) {
            self.err(self.last_lo, "E0034: multiple applicable items in scope".into()); return None;
        }
        let (_, mut args) = candidates.pop().unwrap();
        if !candidates.is_empty() || args.is_empty() {
            args.clear();
            let td = self.prog.def(first).parent;
            for i in 1..=self.tcx.trait_generic_count(self.prog, td) { args.push(self.fresh_arg(td, i as usize)); }
        }
        Some((first, args))
    }

    /// rustc_ast_lowering/src/expr.rs:1774-1784,1826-1838: every head is the
    /// by-value argument of IntoIterator::into_iter, followed by next(&mut iter).
    /// UFCS does not autoderef or autoref the head; its adjustment is identity.
    fn for_into_iter_item(&mut self, it: u32, s: TyId) -> Option<TyId> {
        // rustc_ast_lowering/src/expr.rs: IntoIterator::into_iter(head),
        // then Iterator::next(&mut iter); read both results from their signatures.
        let into = self.tcx.lang_items.get(crate::lang_items::LangItem::IntoIterIntoIter)?;
        let full = self.infer_item_args(into, &[s]);
        let sig = self.tcx.fn_sig(self.prog, into)?.clone();
        let (_, iter_t) = self.instantiate_call_signature(into, &full, &sig);
        self.register_method_obligation(into, &full);
        self.apply_fn_bounds(into, &full, self.last_lo);
        let iter_t = self.normalize_signature(iter_t);
        let item = self.iterator_item(it, iter_t)?;
        self.b.dependencies.push(into);
        self.b.for_into.insert(it, (into, self.tcx.tys.mk_list(&[s]), iter_t));
        Some(item)
    }

    /// Trait methods `s` callable on `t` here: only traits in scope at the call (rustc's rule;
    /// `rc.borrow_mut()` is RefCell's through Deref unless BorrowMut is imported). Generic
    /// parameters and inference variables keep every candidate (bounds decide).
    fn scoped_trait_methods(&mut self, t: TyId, s: Sym) -> Vec<(DefId, DefId)> {
        let mut tm = self.extension_candidates(t, s, DefKind::AssocFn);
        // a param / rigid projection of a generic body: its bounds' methods, supertraits
        // included (rustc assemble_inherent_candidates_from_param: where-clause candidates)
        if matches!(self.tcx.tys.kind(t), TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. })) {
            for (td, _) in self.param_bounds(t) {
                for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
                    if self.prog.def(it).name == s && self.prog.def(it).kind == DefKind::AssocFn && !tm.iter().any(|x| x.0 == it) {
                        tm.push((it, crate::program::NO_DEF));
                    }
                }
            }
        }
        tm
    }

    /// The traits a method probe considers: those in scope at the call (rustc
    /// resolve's traits_in_scope, including the trait of the enclosing impl and
    /// the body's bound traits).
    /// method/probe.rs:926-928 + assemble_inherent_candidates_from_param: a type parameter's own
    /// where-clauses (elaborated through supertraits; `Self` in a trait has `Self: Trait`) are
    /// candidates for it; any other self type sees only the traits in scope (extension
    /// candidates, probe.rs:1092-1175).
    /// rustc_resolve/src/late.rs:5515-5522 + lib.rs:2111-2129 (traits_in_scope): inside a trait
    /// impl the implemented trait itself is in scope, besides the module's imports.
    fn traits_in_scope(&self) -> Vec<DefId> {
        let mut traits: Vec<DefId> = self.prog.traits_in_scope(self.module).iter().copied().collect();
        let parent = self.prog.def(self.b.def).parent;
        if parent != NO_DEF && self.prog.def(parent).kind == DefKind::Impl {
            if let Some(&td) = self.tcx.impl_trait.get(&parent) { traits.push(td); }
        }
        traits
    }

    fn param_bound_traits(&mut self, t: TyId) -> Vec<DefId> {
        if !matches!(self.tcx.tys.kind(t), TyKind::Param(_)) { return Vec::new(); }
        self.param_bounds(t).into_iter().map(|(td, _)| td).collect()
    }

    /// method/probe.rs:1092-1175 (assemble_extension_candidates_for_traits_in_scope)
    /// and :2087-2161 (consider_probe, TraitCandidate): every trait in scope with an
    /// item `name` of `kind` is a candidate when `t: Trait<fresh args>` may hold --
    /// the obligation is evaluated in a probe and only a true error rejects it
    /// (ambiguity is a match). A rigid opaque or param self type is proven through its
    /// alias bounds / where-clauses by the solver like any other type.
    fn extension_candidates(&mut self, t: TyId, name: Sym, kind: DefKind) -> Vec<(DefId, DefId)> {
        // (Mode::Path probes an unresolved self type too -- `<_>::try_from(x)`: probe_op has no
        // bad-type check there, every trait in scope with the item may apply)
        if matches!(self.tcx.tys.kind(t), TyKind::Error) { return Vec::new(); }
        let mut traits = self.traits_in_scope();
        traits.extend(self.param_bound_traits(t));
        traits.sort_by_key(|d| d.0);
        traits.dedup();
        let mut out = Vec::new();
        for td in traits {
            if self.prog.def(td).kind != DefKind::Trait { continue; }
            let Some(item) = self.prog.traits[self.prog.def(td).sub as usize].iter().copied().find(|&it| self.prog.def(it).name == name && self.prog.def(it).kind == kind) else { continue };
            let snapshot = self.start_snapshot();
            let outer_universe = self.infcx.universe();
            let mut args = Vec::new();
            let indices = self.tcx.generic_indices_of(self.prog, td);
            let full = self.infcx.fresh_args_for_item(self.prog, td);
            for &i in &indices.types_consts[1..] {
                args.push(match full[i as usize] { crate::consts::GenericArg::Type(t) => t, crate::consts::GenericArg::Const(c) => self.tcx.tys.intern(TyKind::Const(c)), _ => unreachable!() });
            }
            let goal = self.trait_goal(td, t, &args);
            self.register_goal(goal, self.last_lo);
            let mut matches = self.fulfillment_cx.try_evaluate_obligations(&self.infcx, self.prog).is_empty();
            // probe.rs:2271-2275: the probe's leak check.
            if matches && self.infcx.leak_check(outer_universe, Some(&snapshot.inference)).is_err() { matches = false; }
            self.rollback_to(snapshot);
            if matches { out.push((item, crate::program::NO_DEF)); }
        }
        out
    }

    /// method/probe.rs: object candidates include every elaborated supertrait;
    /// a path through dyn Self must diagnose ambiguity just like a method call.
    fn dyn_method(&mut self, td: DefId, name: Sym, lo: u32) -> Result<Option<DefId>, ()> {
        if td == NO_DEF { return Ok(None); }
        let args = self.tcx.generic_indices_of(self.prog, td).identity_args(&self.tcx.tys);
        let p = crate::next_solver::collect::trait_predicate(td, args);
        let mut candidates = Vec::new();
        for p in self.tcx.supertraits(self.prog, p) {
            let PredicateKind::Clause(ClauseKind::Trait(tr)) = p.kind else { continue; };
            for &item in &self.prog.traits[self.prog.def(tr.trait_ref.def_id).sub as usize] {
                if self.prog.def(item).name == name && self.prog.def(item).kind == DefKind::AssocFn && !candidates.contains(&item) { candidates.push(item); }
            }
        }
        if candidates.len() > 1 { self.err(lo, "E0034: multiple applicable items in scope".into()); return Err(()); }
        Ok(candidates.pop())
    }

    /// The args of supertrait `target` reached from trait object principal `td<targs>` (Self =
    /// `dyn_t`): `dyn Bar` with `trait Bar: for<'a> Foo<&'a ()>` gives Foo's `[&()]`. None when no
    /// or several references reach it (rustc: the object's elaborated supertrait predicates).
    fn dyn_super_args(&self, td: DefId, targs: &[TyId], dyn_t: TyId, target: DefId) -> Option<Vec<TyId>> {
        let mut found: Vec<Vec<TyId>> = Vec::new();
        let mut queue: Vec<(DefId, Vec<TyId>, u32)> = vec![(td, targs.to_vec(), 0)];
        let mut seen = 0;
        while let Some((d, a, depth)) = queue.pop() {
            seen += 1;
            if seen > 64 || depth > 8 {
                return None;
            }
            for (sd, sa) in self.tcx.trait_super_refs_of(&self.cx.with_infcx(&self.infcx), self.prog, d, dyn_t, &a) {
                if sd == target {
                    if !found.contains(&sa) {
                        found.push(sa.clone());
                    }
                } else {
                    queue.push((sd, sa, depth + 1));
                }
            }
        }
        if found.len() == 1 {
            found.pop()
        } else {
            None
        }
    }

    fn trait_call_args(&mut self, item: DefId, imp: DefId, t: TyId, explicit_own: &[TyId]) -> Vec<TyId> {
        // trait objects (NO_DEF) and #[derive]d impls have no impl item: trait args stay open
        if imp == crate::program::NO_DEF {
            // trait object receiver: [dyn, the principal's args when the method is its own, own..]
            let mut args = vec![t];
            let np_known = self.tcx.fn_sig(self.prog, item).map_or(1, |s| s.n_parent_generics) as usize;
            if let (Some(full), TyKind::Dyn(..)) = (&self.known_method_args, self.tcx.tys.kind(t)) {
                // confirm.rs ObjectPick: the principal's trait ref with its binder instantiated
                // by the pick (the object's own arguments still hold the bound regions)
                let indices = self.tcx.generic_indices_of(self.prog, item);
                for &idx in &indices.types_consts[1..np_known] {
                    args.push(match full[idx as usize] { crate::consts::GenericArg::Type(t) => t, crate::consts::GenericArg::Const(c) => self.tcx.tys.intern(TyKind::Const(c)), _ => unreachable!() });
                }
            } else if let TyKind::Dyn(td, targs, _, _, _, _, _) = self.tcx.tys.kind(t).clone() {
                let parent = self.prog.def(item).parent;
                if parent == td {
                    args.extend(self.tcx.tys.list(targs).iter().copied());
                } else if let Some(sa) = self.dyn_super_args(td, self.tcx.tys.list(targs), t, parent) {
                    args.extend(sa);
                }
            }
            let sig = self.tcx.fn_sig(self.prog, item).cloned();
            let np = sig.as_ref().map_or(1, |s| s.n_parent_generics) as usize;
            while args.len() < np {
                args.push(self.fresh_arg(item, args.len()));
            }
            let own = sig.map_or(0, |s| s.n_generics - s.n_parent_generics);
            for i in 0..own as usize {
                if i < explicit_own.len() {
                    args.push(explicit_own[i]);
                } else {
                    args.push(self.fresh_arg(item, args.len()));
                }
            }
            return args;
        }
        let impl_args = self.match_impl_full_args(imp, t);
        let targs = self.tcx.impl_trait_args.get(&imp).cloned().unwrap_or_default();
        let mut args = vec![t];
        for ta in targs {
            args.push(self.tcx.instantiate_ty(self.prog, imp, &impl_args, ta));
        }
        let own = self.tcx.fn_sig(self.prog, item).map_or(0, |s| s.n_generics - s.n_parent_generics);
        for i in 0..own as usize {
            if i < explicit_own.len() {
                args.push(explicit_own[i]);
            } else {
                args.push(self.fresh_arg(item, args.len()));
            }
        }
        args
    }

    /// One autoderef step from `t`: built-in for references and raw pointers, through
    /// `Deref` for other types. Records the step in the operand adjustments of node `e`.
    fn deref_step(&mut self, e: u32, step: usize, t: TyId) -> Option<TyId> {
        // rustc autoderef.rs: the recursion limit ends the walk with E0055
        if step >= self.infcx.recursion_limit(self.prog) && e != u32::MAX - 1 {
            let ts = self.ty_str(t);
            self.err(self.last_lo, format!("E0055: reached the recursion limit while auto-dereferencing `{}`", ts));
            return None;
        }
        let (next, overloaded) = self.deref_step_raw(t)?;
        self.set_operand_deref(e, step, overloaded, next);
        Some(next)
    }

    /// The step itself: the target type, and the `Deref::deref` call of an overloaded one.
    fn deref_step_raw(&mut self, t: TyId) -> Option<(TyId, Option<adjustments::OverloadedDeref>)> {
        let t = self.shallow(t);
        // a variable that is an opaque's hidden type in its defining scope derefs through the
        // opaque's item bounds (autoderef.rs overloaded_deref_ty on a pending `?h: Deref`)
        let hidden_of_opaque = matches!(self.tcx.tys.kind(t), TyKind::Infer(v) if self.infcx.has_opaques_with_sub_unified_hidden_type(*v));
        match self.tcx.tys.kind(t).clone() {
            TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) => Some((self.shallow(i), None)),
            TyKind::Infer(_) if hidden_of_opaque => {
                let td = self.tcx.lang_items.get(crate::lang_items::LangItem::Deref)?;
                let dt = self.infcx.deeply_resolve_ignoring_regions(t);
                self.select_trait(td, dt, &[]).ok()?;
                let target = self.tcx.trait_assoc(self.prog, td, self.prog.syms.get("Target")?)?;
                let proj = self.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: target }, args: self.tcx.tys.mk_list(&[dt]), regions: crate::types::RegionArgsId::EMPTY }));
                let r = self.norm(proj);
                let m = self.trait_fn(td, "deref")?;
                Some((r, Some((m, self.tcx.tys.mk_list(&[dt])))))
            }
            // (a param / rigid projection with a Deref bound derefs to its `Target` binding,
            // `D: DerefMut<Target = DrawVars>`)
            // (an opaque type derefs through its item bounds: `impl Deref<Target = Path>`,
            // rustc autoderef.rs overloaded_deref_ty selects the bound's candidate)
            TyKind::Adt(..) | TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) => {
                let td = self.tcx.lang_items.get(crate::lang_items::LangItem::Deref)?;
                let dt = self.infcx.deeply_resolve_ignoring_regions(t);
                match self.select_trait(td, dt, &[]) {
                    Err(_) => return None,
                    Ok(Some(_)) | Ok(None) => {},
                };
                let target = self.tcx.trait_assoc(self.prog, td, self.prog.syms.get("Target")?)?;
                let proj = self.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: target }, args: self.tcx.tys.mk_list(&[dt]), regions: crate::types::RegionArgsId::EMPTY }));
                let r = self.norm(proj);
                let m = self.trait_fn(td, "deref")?;
                Some((r, Some((m, self.tcx.tys.mk_list(&[dt])))))
            }
            _ => None,
        }
    }

    /// The parameter names of fn `d` (its declaration's patterns, `_` for others), without self.
    fn param_names(&self, d: DefId) -> Vec<String> {
        let def = self.prog.def(d);
        let f = &self.prog.files[def.file as usize];
        match &f.ast.item(def.item).kind {
            crate::ast::ItemKind::Fn(sig, _, _) => f.ast.list(f.ast.fn_sig(*sig).params)
                .iter()
                .map(|p| match f.ast.pat(p.pat) {
                    Pat::Ident { name, .. } => f.text(*name).to_string(),
                    _ => "_".to_string(),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Help for E0061 (too few / too many arguments): which parameters are missing, by name and
    /// type (matched against the given arguments' types in order: the first parameter each
    /// argument fits), or which arguments are extra.
    fn arg_count_help(&mut self, callee: Option<DefId>, params: &[TyId], args: &[ExprId], skip: usize) -> String {
        let names = callee.map(|d| self.param_names(d)).unwrap_or_default();
        let name_of = |i: usize| names.get(i + skip).cloned().unwrap_or_else(|| format!("#{}", i + 1));
        if args.len() > params.len() {
            return format!(" -- remove the extra argument{}", if args.len() - params.len() == 1 { "" } else { "s" });
        }
        let missing: Vec<usize> = (args.len()..params.len()).collect();
        let list: Vec<String> = missing.iter().map(|&i| format!("`{}: {}`", name_of(i), self.ty_str(params[i]))).collect();
        match list.len() {
            0 => String::new(),
            1 => format!(" -- missing argument {}", list[0]),
            _ => format!(" -- missing arguments {}", list.join(", ")),
        }
    }

    /// The methods that exist on `t` and its autoderef targets (inherent ones, and those of
    /// every trait implemented for the type anywhere in the program): (name, the trait when it
    /// comes from one).
    fn methods_of(&mut self, t: TyId) -> Vec<(Sym, Option<DefId>)> {
        let mut out: Vec<(Sym, Option<DefId>)> = Vec::new();
        let mut cur = self.shallow(t);
        for _ in 0..6usize {
            let key = crate::tcx::ty_key(&self.tcx.tys, cur);
            if let Some(list) = self.tcx.inherent.get(&key) {
                for &imp in list {
                    for &it in &self.prog.impls[self.prog.def(imp).sub as usize].items {
                        let d = self.prog.def(it);
                        if d.kind == DefKind::AssocFn && self.tcx.fn_sig(self.prog, it).map_or(false, |sg| sg.self_kind != 0) && !out.iter().any(|x| x.0 == d.name) {
                            out.push((d.name, None));
                        }
                    }
                }
            }
            if let Some(list) = self.tcx.impls_by_key.get(&key) {
                for &imp in list {
                    let Some(&td) = self.tcx.impl_trait.get(&imp) else { continue };
                    for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
                        let d = self.prog.def(it);
                        if d.kind == DefKind::AssocFn && self.tcx.fn_sig(self.prog, it).map_or(false, |sg| sg.self_kind != 0) && !out.iter().any(|x| x.0 == d.name) {
                            out.push((d.name, Some(td)));
                        }
                    }
                }
            }
            match self.deref_step_raw(cur) {
                Some((n, _)) => cur = self.shallow(n),
                None => break,
            }
        }
        out
    }

    /// Help for E0599: the trait to import when the method exists in a trait implemented for the
    /// type but not in scope; else the type's methods closest to the name (real names, not
    /// guesses from other types).
    fn method_help(&mut self, t: TyId, name: Sym) -> String {
        let ms = self.methods_of(t);
        let scope = self.prog.traits_in_scope(self.module);
        if let Some((_, Some(td))) = ms.iter().find(|m| m.0 == name) {
            if !scope.contains(td) {
                let tn = self.prog.syms.str(self.prog.def(*td).name);
                let paths = self.prog.import_suggestions(self.module, self.prog.def(*td).name, true);
                return match paths.first() {
                    Some(p) => format!(" -- the method is in trait `{}`, which is not in scope: `use {};`", tn, p),
                    None => format!(" -- the method is in trait `{}`, which is not in scope", tn),
                };
            }
        }
        let want = self.prog.syms.str(name).to_string();
        let mut close: Vec<(usize, String)> = ms
            .iter()
            .map(|m| self.prog.syms.str(m.0).to_string())
            .filter_map(|n| {
                let d = edit_distance(&want, &n);
                let lim = (want.len().max(n.len()) / 3).max(1);
                let sub = n.contains(want.as_str()) || want.contains(n.as_str());
                if d <= lim || (sub && n.len() > 2) {
                    Some((d, n))
                } else {
                    None
                }
            })
            .collect();
        close.sort();
        close.dedup();
        match close.len() {
            0 => String::new(),
            _ => format!(" -- methods with a similar name on this type: {}", close.iter().take(4).map(|x| format!("`{}`", x.1)).collect::<Vec<_>>().join(", ")),
        }
    }

    fn trait_fn(&self, td: DefId, name: &str) -> Option<DefId> {
        let s = self.prog.syms.get(name)?;
        for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
            if self.prog.def(it).name == s {
                return Some(it);
            }
        }
        None
    }

    /// Normalizes associated-type projections whose Self is known.
    fn norm(&mut self, t: TyId) -> TyId {
        let d = self.infcx.deeply_resolve_ignoring_regions(t);
        let n = self.normalize_body_type(d);
        n
    }

    fn normalize_body_type(&mut self, ty: TyId) -> TyId {
        let (ty, goals) = self.infcx.normalize_ty(self.prog, &self.param_env, ty);
        for goal in goals { self.register_goal(goal, self.last_lo); }
        ty
    }

    /// rustc_hir_typeck/src/method/probe.rs:981,2034: instantiate an impl's
    /// declaration arguments and test the relation in a rollback probe.
    fn assemble_inherent_impl_probe(&mut self, ty: TyId, name: Sym) -> Option<(DefId, DefId)> {
        self.cx.state.impl_read.set(true);
        let key = ty_key(&self.tcx.tys, ty);
        let mut found = None;
        for &imp in self.tcx.inherent.get(&key)? {
            if !self.prog.impls[self.prog.def(imp).sub as usize].items.iter().any(|&item| self.prog.def(item).name == name && self.item_visible(item)) { continue; }
            let applicable = self.infcx.probe(|_| {
                let full = self.infcx.fresh_args_for_item(self.prog, imp);
                let self_ty = self.tcx.instantiate_ty(self.prog, imp, &full, self.tcx.impl_self[&imp]);
                let mut ocx = crate::next_solver::fulfill::ObligationCtxt::new(&self.infcx, self.prog);
                if ocx.eq(&self.param_env, self_ty, ty).is_err() { return false; }
                for clause in self.tcx.clauses_of(self.prog, imp) {
                    ocx.register_obligation(Goal { param_env: self.param_env.clone(), predicate: self.tcx.instantiate_predicate(self.prog, imp, &full, &clause) });
                }
                ocx.engine.try_evaluate_obligations(&self.infcx, self.prog).is_empty()
            });
            if !applicable { continue; }
            for &item in &self.prog.impls[self.prog.def(imp).sub as usize].items {
                if self.prog.def(item).name == name && self.item_visible(item) {
                    if found.is_some() { self.err(self.last_lo, "E0034: multiple applicable items in scope".into()); return None; }
                    found = Some((item, imp));
                }
            }
        }
        found
    }

    /// Full generic args of impl `imp` (lifetimes included) such that its self type equals `t`.
    fn match_impl_full_args(&mut self, imp: DefId, t: TyId) -> Vec<crate::consts::GenericArg> {
        let full = self.infcx.fresh_args_for_item(self.prog, imp);
        if let Some(&st) = self.tcx.impl_self.get(&imp) {
            let inst = self.tcx.instantiate_ty(self.prog, imp, &full, st);
            self.eq(inst, t);
        }
        full
    }

    /// An inherent associated value path's impl args (fn_ctxt/_impl.rs
    /// instantiate_value_path with confirm.rs fresh_receiver_args): the type/const args
    /// for the item's FnDef, its lifetime args kept for value_def_ty.
    fn impl_args_for_value_path(&mut self, imp: DefId, t: TyId) -> Vec<TyId> {
        let full = self.match_impl_full_args(imp, t);
        let mut args = Vec::new();
        self.value_path_parent_regions.clear();
        for arg in full {
            match arg {
                crate::consts::GenericArg::Type(t) => args.push(t),
                crate::consts::GenericArg::Const(c) => args.push(self.tcx.tys.intern(TyKind::Const(c))),
                crate::consts::GenericArg::Lifetime(r) => self.value_path_parent_regions.push(r),
            }
        }
        args
    }

    fn explicit_args(&mut self, path: &Path, def: DefId, offset: usize) -> Option<Vec<TyId>> {
        let n = path.segs.len();
        self.seg_args(path, n - 1, def, offset)
    }

    /// hir_ty_lowering check_generic_arg_count of a written segment against `def`'s own generics.
    /// False (reported) when the number of arguments is wrong.
    fn check_seg_arg_count(&mut self, args: GenericArgsId, def: DefId, lo: u32, method: bool) -> bool {
        use crate::tcx::arg_count::ArgPosition;
        let prog = self.prog;
        // (only a crate whose errors are reported validates; a call without written arguments has nothing to count)
        if !matches!(self.ast().gargs(args), Some(GenericArgs::Angle(_))) || !self.tcx.validates(prog, self.file) { return true; }
        let mut def = def;
        let mut kind = prog.def(def).kind;
        if kind == DefKind::Variant { def = prog.def(def).parent; kind = prog.def(def).kind; }
        let (pos, early, late_bound) = match kind {
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                let indices = self.tcx.generic_indices_of(prog, def);
                let parent = prog.def(def).parent;
                let parent_lifetimes = if kind == DefKind::AssocFn { self.tcx.generic_indices_of(prog, parent).lifetimes.len() } else { 0 };
                let own = indices.lifetimes.len() - parent_lifetimes;
                let late = self.tcx.fn_item_sig(prog, def).map_or(0, |s| s.bound_regions) > 0;
                (ArgPosition::Value, Some(own), late)
            }
            DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::Trait => (ArgPosition::Type, None, false),
            _ => return true,
        };
        let ast = self.ast();
        // prohibit_explicit_late_bound_lifetimes
        if late_bound {
            if let Some(GenericArgs::Angle(v)) = ast.gargs(args) {
                let lts: Vec<Ident> = ast.list(*v).iter().filter_map(|a| if let GenericArg::Lifetime(l) = *a { Some(l) } else { None }).collect();
                if let Some(first) = lts.first() {
                    let msg = "cannot specify lifetime arguments explicitly if late bound lifetime parameters are present";
                    if !method && Some(lts.len()) != early {
                        self.err(first.lo, format!("E0794: {}", msg));
                    } else {
                        crate::lint::emit(self.prog, self.b.def, &crate::lint::LATE_BOUND_LIFETIME_ARGUMENTS, self.file, first.lo, msg);
                    }
                }
            }
        }
        let counts = self.tcx.own_generic_counts(prog, def, early);
        let errs = self.tcx.check_generic_arg_count(prog, def, ast.gargs(args), ast, counts, pos, !matches!(ast.gargs(args), Some(GenericArgs::Angle(_))), late_bound);
        for e in &errs { self.err(lo, e.clone()); }
        let mut ok = errs.is_empty();
        // associated item constraints belong to trait paths only
        if let Some(GenericArgs::Angle(v)) = ast.gargs(args) {
            if ast.list(*v).iter().any(|a| matches!(a, GenericArg::Binding(..) | GenericArg::Constraint(..))) {
                self.err(lo, "E0229: associated item constraints are not allowed here".to_string());
                ok = false;
            }
        }
        ok
    }

    fn seg_args(&mut self, path: &Path, i: usize, def: DefId, offset: usize) -> Option<Vec<TyId>> {
        let seg = self.ast().segs(path.segs).get(i)?;
        let a = self.ast().gargs(seg.args())?;
        let (args_id, lo) = (seg.args(), seg.name.lo);
        if !self.check_seg_arg_count(args_id, def, lo, false) { return None; }
        let mut out = Vec::new();
        if let GenericArgs::Angle(v) = a {
            // hir_ty_lowering/generics.rs: arguments beyond the declared parameters are not
            // lowered (E0107 reports them)
            let declared = self.tcx.generic_indices_of(self.prog, def).types_consts.len();
            for g in self.ast().list(*v) {
                if offset + out.len() >= declared && !matches!(g, GenericArg::Lifetime(_) | GenericArg::Binding(..) | GenericArg::Constraint(..)) {
                    break;
                }
                if let Some(lt) = self.generic_arg(g, def, offset + out.len()) {
                    out.push(lt);
                }
            }
        }
        Some(out)
    }

    /// A type or const generic argument inside the definition body.
    fn generic_arg(&mut self, g: &GenericArg, def: DefId, index: usize) -> Option<TyId> {
        let prog = self.prog;
        let env = self.env.clone();
        match g {
            GenericArg::Type(t) => {
                if let Ty::Infer = prog.files[self.file as usize].ast.ty(*t) {
                    return Some(self.fresh_arg(def, index));
                }
                let (lt, errors) = self.tcx.with_errors(&self.cx.with_infcx(&self.infcx), ||
                    self.tcx.type_or_const_arg(&self.cx.with_infcx(&self.infcx), prog, self.file, self.module, &env, *t));
                let clean = errors.is_empty();
                for error in errors { if !self.b.errors.contains(&error) { self.b.errors.push(error); } }
                // FnCtxt::lower_ty: every explicit type argument is well formed
                if self.tcx.validates(self.prog, self.file) && clean && !matches!(self.tcx.tys.kind(lt), TyKind::Const(_)) && !self.tcx.tys.has_error(lt) { self.register_wf(lt, self.last_lo); }
                // hir_ty_lowering/generics.rs:21-136,244-310: kind mismatch
                // recovery keeps the parameter's kind, including unresolved args.
                let declaration = self.tcx.declaration_env(prog, def);
                let expected_const = declaration.consts.iter().any(|c| c.0 as usize == index);
                if expected_const != matches!(self.tcx.tys.kind(lt), TyKind::Const(_)) {
                    let message = if expected_const {
                        if self.tcx.tys.has_error(lt) { "E0747: unresolved item provided when a constant was expected" }
                        else { "E0747: type provided when a constant was expected" }
                    } else { "E0747: constant provided when a type was expected" };
                    self.err(self.last_lo, message.into());
                    return Some(if expected_const { self.tcx.tys.intern(TyKind::Const(self.tcx.tys.intern_const(crate::consts::ConstKind::Error))) } else { self.tcx.tys.error });
                }
                Some(lt)
            }
            GenericArg::Const(e) => {
                let declaration = self.tcx.declaration_env(prog, def);
                let Some(&(_, file, ty)) = declaration.consts.iter().find(|c| c.0 as usize == index) else {
                    self.err(self.last_lo, "E0747: constant provided when a type was expected".into());
                    return Some(self.tcx.tys.error);
                };
                let expected = self.tcx.lower_ty(&self.cx.with_infcx(&self.infcx), prog, file, prog.def(def).scope, &declaration, ty);
                let ct = if crate::tcx::is_infer_len(self.ast(), *e) { self.infcx.next_const_var() }
                    else { self.tcx.lower_const_arg(prog, self.file, &env, *e, expected) };
                let lt = self.tcx.tys.intern(TyKind::Const(ct));
                Some(lt)
            }
            _ => None,
        }
    }

    /// Lowers a syntactic type inside the definition body.
    pub fn lower_ty(&mut self, t: crate::ast::TyId) -> TyId {
        let prog = self.prog;
        let env = self.env.clone();
        if let Ty::Infer = prog.files[self.file as usize].ast.ty(t) {
            return self.fresh(0);
        }
        // (errors of the annotation itself belong to this body: `let m: HashMap<..>` without the
        // import is E0412 here, not a later "type not fully known")
        let (lt, errs) = self.tcx.with_errors(&self.cx.with_infcx(&self.infcx), || self.tcx.lower_ty(&self.cx.with_infcx(&self.infcx), prog, self.file, self.module, &env, t));
        let lowered_clean = errs.is_empty();
        for e in errs {
            if !self.b.errors.contains(&e) {
                self.b.errors.push(e);
            }
        }
        type_dependencies(self.tcx, lt, &mut self.b.dependencies);
        // rustc_hir_typeck/src/fn_ctxt/mod.rs lower_ty: every written type must be well formed
        if self.tcx.validates(self.prog, self.file) && lowered_clean && !self.tcx.tys.has_error(lt) { self.register_wf(lt, self.last_lo); }
        lt
    }

    /// Type named by the first `n` segments of a path (for `Type::item`).
    fn type_of_prefix(&mut self, path: &Path, n: usize) -> Option<TyId> {
        let prog = self.prog;
        if n == 1 && self.ast().segs(path.segs)[0].kind() == SegKind::SelfType {
            let st = self.env.self_ty?;
            return Some(st);
        }
        // generic param `T::item`
        if n == 1 {
            let s = self.sym(self.ast().segs(path.segs)[0].name);
            for (i, nm) in self.env.names.iter().enumerate() {
                if *nm == s && i < self.b.args.len() {
                    return Some(self.b.args[i]);
                }
            }
        }
        let sub = Path { segs: path.segs.prefix(n).without_qself(), lo: path.lo };
        // `Self::Bits::EMPTY` / `T::Assoc::f`: the prefix is an associated-type projection,
        // lowered as a type path and normalized for this instance
        if n >= 2 && (self.ast().segs(path.segs)[0].kind() == SegKind::SelfType || self.env.names.contains(&self.sym(self.ast().segs(path.segs)[0].name))) {
            let env = self.env.clone();
            // (an expression path may elide a generic associated type's lifetimes: rustc
            // reports E0107 only in types)
            let (t, errs) = self.tcx.with_errors(&self.cx.with_infcx(&self.infcx), || self.tcx.lower_path_ty(&self.cx.with_infcx(&self.infcx), prog, self.file, self.module, &env, &sub, None));
            for e in errs {
                if !e.contains("E0107: missing generics for associated type") {
                    self.tcx.reraise_error(&self.cx.with_infcx(&self.infcx), e);
                }
            }
            let t = self.normalize_body_type(t);
            // (a projection that stays one is rigid in a generic body: `A::Mem::default()`
            // resolves through the associated type's bounds)
            if !matches!(self.tcx.tys.kind(t), TyKind::Error) && (!matches!(self.tcx.tys.kind(t), TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. })) || !self.tcx.tys.has_infer(t)) {
                return Some(t);
            }
        }
        let d = self.tcx.resolve_type_path_def(prog, self.file, self.module, &sub);
        // a primitive type's name that resolves to a module (or nothing): the primitive type
        // (rustc_resolve late.rs resolve_qpath fallback)
        if n == 1 && !path.global() && d.map_or(true, |d| prog.def(d).kind == DefKind::Mod) {
            let s = self.sym(self.ast().segs(path.segs)[0].name);
            if let Some((_, p)) = crate::program::PRIMS.iter().find(|(nm, _)| prog.syms.get(nm) == Some(s)) {
                return Some(self.tcx.tys.prim(*p));
            }
        }
        // relaxed imports: `HashMap::new()` with `HashMap` unique in the program
        let d = match d {
            Some(d) => d,
            None if n == 1 && !path.global() => prog.relaxed_import(self.module, self.sym(self.ast().segs(path.segs)[0].name), true, self.file, path.lo)?,
            None => return None,
        };
        let args = self.seg_args(path, n - 1, d, 0);
        match prog.def(d).kind {
            DefKind::Prim(p) => Some(self.tcx.tys.prim(p)),
            DefKind::Struct | DefKind::Enum | DefKind::Union => {
                let ng = self.tcx.adts.get(&d).map_or(0, |a| a.n_generics);
                let mut a = args.unwrap_or_default();
                while a.len() < ng as usize {
                    a.push(self.fresh_arg(d, a.len()));
                }
                let mut regions = Vec::new();
                for _ in &self.tcx.generic_indices_of(prog, d).lifetimes { regions.push(self.fresh_region()); }
                // the lifetimes written on the prefix segment (`A::<'a>::new`)
                let written = self.seg_lifetimes(path, n - 1);
                if written.len() == regions.len() { regions = written; }
                Some(self.tcx.tys.intern(TyKind::Adt(d, self.tcx.tys.mk_list(&a), self.tcx.tys.intern_region_args(&regions))))
            }
            DefKind::TypeAlias => {
                let t = self.tcx.alias_ty(&self.cx.with_infcx(&self.infcx), prog, d);
                let mut a = args.unwrap_or_default();
                // `Alias::f` without generic args: the alias's parameters are inferred like a
                // struct's (`FxHashSet::default()`)
                let need = self.tcx.item_env(prog, d).names.len();
                while a.len() < need {
                    a.push(self.fresh_arg(d, a.len()));
                }
                let t = self.tcx.tys.subst(t, &a);
                // the alias's lifetime parameters: the ones written on the segment, else inferred
                // (`S2::shl` for `type S2<'a> = S<'a, 'a>`)
                let params = self.tcx.generic_indices_of(prog, d).lifetimes.clone();
                if params.is_empty() { return Some(t); }
                let written = self.seg_lifetimes(path, n - 1);
                let regions: Vec<crate::types::RegionId> = if written.len() == params.len() { written } else { params.iter().map(|_| self.fresh_region()).collect() };
                Some(self.tcx.tys.map_regions(t, &|r, depth| match self.tcx.tys.region_kind(r) {
                    crate::types::RegionKind::ReBound(b, _) if b < depth => r,
                    crate::types::RegionKind::ReEarlyParam(i) => params.iter().position(|&p| p == i).map_or(r, |k| regions[k]),
                    _ => r,
                }, 0))
            }
            _ => None,
        }
    }

    fn note_capture(&mut self, li: u32, _key: u32) {
        // a local declared outside a closure is captured by it (and by every closure between
        // it and the local's scope, so nested closures can reach it)
        for k in (0..self.closure_depth.len()).rev() {
            let (ce, depth) = self.closure_depth[k];
            let mut declared_inside = false;
            for i in depth..self.scopes.len() {
                if self.scopes[i].1 == li {
                    declared_inside = true;
                }
            }
            if declared_inside {
                break;
            }
            let caps = self.b.closure_locals.get_or_insert_with(ce, Vec::new);
            if !caps.contains(&li) {
                caps.push(li);
            }
        }
    }

    // ------------------------------------------------------------ blocks

    /// rustc_hir_typeck check_block_no_value: a loop body has type `()` (a `!` body coerces to it);
    /// the tail expression is a coercion site for the unit expectation.
    fn check_block_no_value(&mut self, b: BlockId) {
        let unit = self.tcx.tys.unit;
        let ty = self.check_block(b, Some(unit));
        let st = self.shallow(ty);
        if matches!(self.tcx.tys.kind(st), TyKind::Never) { return; }
        let blk = self.ast().block(b);
        let tail = match self.ast().list(blk.stmts).last() { Some(Stmt::Expr(e, false)) => Some(*e), _ => None };
        let pos = match tail { Some(e) => self.ast().expr(e).lo, None => blk.lo };
        self.coerce_or_unify(ty, unit, tail, pos);
    }

    fn check_block(&mut self, b: BlockId, expected: Option<TyId>) -> TyId {
        self.check_block_soft(b, expected, false)
    }

    /// `soft`: the expectation is a hint only (the tail is not required to coerce to it).
    fn check_block_soft(&mut self, b: BlockId, expected: Option<TyId>, soft: bool) -> TyId {
        let ast = self.ast();
        let blk = ast.block(b);
        let saved_scope = self.scopes.len();
        let saved_module = self.module;
        let saved_blocks = self.item_blocks.len();
        if let Some(&m) = self.prog.block_mods.get(&(self.file, b.0)) {
            self.module = m;
            self.item_blocks.push((m, saved_scope));
        }
        let mut result = self.tcx.tys.unit;
        let n = blk.stmts.len();
        let mut diverged = false;
        for (i, s) in self.ast().list(blk.stmts).iter().enumerate() {
            if i > 0 {
                self.retry_goals(false);
            }
            match s {
                Stmt::Let(r) => {
                    let crate::ast::LetStmt { attrs, pat, ty, init, else_ } = self.ast().row(*r);
                    if !self.active(*attrs) {
                        continue;
                    }
                    // the annotation normalized in this body's inference context (rustc
                    // gather_locals: FnCtxt::lower_ty -> LoweredTy::from_raw normalizes):
                    // `&<(Wrap<?0>,) as Indirect>::Out` is `&?1` with that projection pending
                    let decl = match ty {
                        Some(t) => {
                            let d = self.lower_let_ty_saving_user_provided_ty(*t);
                            Some(if self.tcx.tys.has_assoc(d) { self.norm(d) } else { d })
                        }
                        None => None,
                    };
                    let t = match init {
                        Some(e) => {
                            let it = self.check_expr(*e, decl);
                            match decl {
                                Some(d) => {
                                    self.coerce_or_unify(it, d, Some(*e), ast.expr(*e).lo);
                                    d
                                }
                                // rustc check_decl_initializer coerces the initializer to the local's
                                // fresh variable: a diverging one leaves it open (diverging fallback)
                                None if matches!(self.tcx.tys.kind(self.shallow(it)), TyKind::Never) => {
                                    let v = self.fresh(0);
                                    self.coerce_or_unify(it, v, Some(*e), ast.expr(*e).lo);
                                    v
                                }
                                // (and the local's variable is instantiated by the coercion, as a generalization
                                // of the initializer type, when that has inference variables inside: relating
                                // the vars of two types is deferred, not unified)
                                None if self.tcx.tys.has_infer(it) && !matches!(self.tcx.tys.kind(self.shallow(it)), TyKind::Infer(_)) => {
                                    let v = self.fresh(0);
                                    self.coerce_or_unify(it, v, Some(*e), ast.expr(*e).lo);
                                    v
                                }
                                None => it,
                            }
                        }
                        None => match decl {
                            Some(d) => d,
                            None => self.fresh(0),
                        },
                    };
                    if self.diverges {
                        diverged = true;
                    }
                    if let Some(eb) = else_ {
                        let save = self.diverges;
                        self.diverges = false;
                        let et = self.check_block(*eb, None);
                        // rustc check_decl_local: the `else` block must have type `!`
                        let es = self.shallow(et);
                        if es != self.tcx.tys.never && !matches!(self.tcx.tys.kind(es), TyKind::Infer(_) | TyKind::Error) {
                            let pos = match self.ast().stmts(*eb).last() { Some(Stmt::Expr(e, false)) => self.ast().expr(*e).lo, _ => self.ast().block(*eb).hi };
                            self.err(pos, "E0308: `else` clause of `let...else` does not diverge".to_string());
                        }
                        self.diverges = save;
                    }
                    let saved = std::mem::replace(&mut self.pat_decl_irrefutable, else_.is_none());
                    self.check_pat(*pat, t);
                    self.pat_decl_irrefutable = saved;
                }
                // stripped at load (program::strip_cfg_stmts)
                Stmt::Item(_) | Stmt::Attrs(..) => {}
                Stmt::Expr(e, semi) => {
                    let is_tail = i == n - 1 && !*semi;
                    let t = self.check_expr(*e, if is_tail { expected } else { None });
                    if self.diverges {
                        diverged = true;
                    }
                    if is_tail {
                        result = t;
                        // the tail is a coercion site for the block's expected type
                        if let Some(x) = expected {
                            let (st, sx) = (self.shallow(t), self.shallow(x));
                            // an unsized expectation is only a hint (rustc Expectation::rvalue_hint,
                            // ExpectRvalueLikeUnsized): `RefCell::new({ .. closure })` for a
                            // `RefCell<dyn FnMut()>` keeps the closure type
                            let unsized_x = self.is_unsized_ty(sx);
                            if st == sx && matches!(self.tcx.tys.kind(st), TyKind::Ref(true, _, _)) {
                                self.set_reborrow(e.0, true, st);
                            }
                            if !unsized_x && st != sx && !matches!(self.tcx.tys.kind(st), TyKind::Never | TyKind::Infer(_)) && !matches!(self.tcx.tys.kind(sx), TyKind::Infer(_)) {
                                // the expectation may be a mere hint (an if's other branch):
                                // when no coercion applies the block keeps its own type and the
                                // consumer (let, return, CoerceMany) decides
                                if self.try_coerce(t, x, Some(*e)) {
                                    result = x;
                                } else if !soft {
                                    // rustc check_block_with_expected: under ExpectHasType the tail is
                                    // coerced to the block's expected type, or E0308
                                    let pos = self.ast().expr(*e).lo;
                                    self.coerce_or_unify(t, x, Some(*e), pos);
                                    result = x;
                                }
                            }
                        }
                    } else if !*semi {
                        // block-like statement without `;` must be unit (or diverge)
                        let st = self.shallow(t);
                        if st != self.tcx.tys.unit && st != self.tcx.tys.never {
                            let unit = self.tcx.tys.unit;
                            let pos = self.ast().expr(*e).lo;
                            self.demand_eq(unit, st, pos);
                        }
                    }
                }
                Stmt::Empty => {}
            }
        }
        self.scopes.truncate(saved_scope);
        self.module = saved_module;
        self.item_blocks.truncate(saved_blocks);
        if diverged {
            self.diverges = true;
            let tail_is_expr = n > 0 && matches!(self.ast().list(blk.stmts)[n - 1], Stmt::Expr(_, false));
            if !tail_is_expr {
                return self.tcx.tys.never;
            }
        }
        result
    }

    fn active(&self, attrs: List<Attr>) -> bool {
        if attrs.is_empty() {
            return true;
        }
        let krate = self.prog.file_crate[self.file as usize];
        crate::cfg::active(self.prog.src(self.file), self.prog.files[self.file as usize].ast.list(attrs), &self.prog.crates[krate as usize].cfg)
    }

    // ------------------------------------------------------------ expressions

    /// `asm!` operands (Reference [asm.operand-type]): inputs are values, outputs places, a
    /// `const` an integer constant; `sym` paths are resolved by lowering. The block is `()`, or
    /// `!` with `options(noreturn)`. Register classes and operand types are checked when the
    /// block is assembled (lowering), against the target's registers.
    fn check_asm(&mut self, m: &MacCall) -> TyId {
        let ast = self.ast();
        let margs = ast.mac_arg(m.args);
        let Some(spec) = margs.asm else { return self.tcx.tys.error };
        for op in ast.list(spec.ops) {
            match op.dir {
                AsmDir::Sym | AsmDir::Const => {
                    if let Some(x) = op.input {
                        self.check_expr(x, None);
                    }
                }
                _ => {
                    if let Some(x) = op.input {
                        let t = self.check_expr(x, None);
                        if let Some(y) = op.output {
                            if y != x {
                                let u = self.check_expr(y, Some(t));
                                let _ = u;
                            }
                        }
                    } else if let Some(y) = op.output {
                        self.check_expr(y, None);
                    }
                }
            }
        }
        if spec.options & ASM_NORETURN != 0 {
            self.tcx.tys.never
        } else {
            self.tcx.tys.unit
        }
    }

    pub fn check_expr(&mut self, e: ExprId, expected: Option<TyId>) -> TyId {
        crate::preempt::point();
        let _why = crate::why::enabled().then(|| crate::why::enter(self.prog.why_expr(
            self.file, self.prog.files[self.file as usize].ast.expr(e).lo, self.b.def, "typeck-expression")));
        let save_div = self.diverges;
        self.diverges = false;
        let t = self.check_expr_inner(e, expected);
        let ts = self.shallow(t);
        if ts == self.tcx.tys.never {
            self.diverges = true;
        }
        self.diverges = self.diverges || save_div;
        self.set_ty(e, t);
        t
    }

    fn check_expr_inner(&mut self, e: ExprId, expected: Option<TyId>) -> TyId {
        let ast = self.ast();
        let ex = ast.expr(e);
        let lo = ex.lo;
        self.last_lo = lo;
        let unit = self.tcx.tys.unit;
        let error = self.tcx.tys.error;
        let soft = std::mem::take(&mut self.soft);
        match &ex.kind {
            ExprKind::Lit(k) => self.check_lit(e, *k, expected),
            ExprKind::Path(path) => {
                let (r, t) = self.resolve_value_path(path, e.0, false);
                self.b.res.insert(e.0, r);
                // expr.rs check_expr_path: a struct-like variant is no value (E0533); a struct is one for resolve (E0423)
                if let Res::Def(d) = r {
                    if self.tcx.validates(self.prog, self.file) && matches!(self.value_ctor_shape(d), Some(s) if s > 1) {
                        let what = if self.prog.def(d).kind == DefKind::Variant { ("E0533", "struct variant") } else { ("E0423", "struct") };
                        self.err(lo, format!("{}: expected value, found {} `{}`", what.0, what.1, crate::tcx::path_str(self.prog, self.file, path)));
                        return error;
                    }
                }
                t
            }
            ExprKind::Paren(x) => self.check_expr(*x, expected),
            ExprKind::Unary(op, x) => match op {
                // rustc_hir_typeck/src/expr.rs check_expr_unary and op.rs:960-1080.
                UnOp::Neg | UnOp::Not => {
                    let t = self.check_expr(*x, expected);
                    let st = self.shallow(t);
                    let builtin = match (op, self.tcx.tys.kind(st)) {
                        (UnOp::Neg, TyKind::Int(_) | TyKind::Float(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)) => true,
                        (UnOp::Not, TyKind::Int(_) | TyKind::IntVar(_) | TyKind::Bool) => true,
                        _ => false,
                    };
                    let (tr, method, symbol) = if *op == UnOp::Neg { ("Neg", "neg", "-") } else { ("Not", "not", "!") };
                    let result = self.check_unop_trait(e, st, tr, method);
                    if result.is_none() && !self.tcx.tys.has_error(st) {
                        let name = self.ty_str(st);
                        self.err(lo, format!("E0600: cannot apply unary operator `{symbol}` to type `{name}`"));
                    }
                    if builtin { t } else { result.unwrap_or(error) }
                }
                UnOp::Deref => {
                    let t = self.check_expr(*x, None);
                    // (a built-in `*` is no adjustment; an overloaded one is the operand's Deref::deref)
                    match self.deref_step_raw(t) {
                        Some((i, overloaded)) => {
                            if overloaded.is_some() { self.set_operand_deref(e.0, 0, overloaded, i); }
                            i
                        }
                        None => {
                            let s = self.ty_str(t);
                            self.err(lo, format!("E0614: type `{}` cannot be dereferenced", s));
                            error
                        }
                    }
                }
            },
            ExprKind::Binary(op, a, b) => self.check_binary(e, *op, *a, *b, expected, lo),
            ExprKind::Assign(a, b) => {
                if crate::lit::is_assignee_pattern(self.ast(), *a) {
                    // destructuring assignment: the right side's type is matched by the
                    // assignee like a pattern (rustc lowers it to `let` + assignments)
                    let tb = self.check_expr(*b, None);
                    self.check_assignee(*a, tb, lo);
                    return unit;
                }
                let ta = self.check_expr(*a, None);
                self.check_lhs_assignable(*a, false);
                // rustc check_expr_assign: the assigned place is Sized (AssignmentLhsSized)
                if self.tcx.validates(self.prog, self.file) { let apos = self.ast().expr(*a).lo; self.require_type_is_sized_deferred(ta, apos); }
                let tb = self.check_expr(*b, Some(ta));
                self.coerce_or_unify(tb, ta, Some(*b), lo);
                unit
            }
            ExprKind::AssignOp(op, a, b) => {
                let ta = self.check_expr(*a, None);
                self.check_lhs_assignable(*a, true);
                let sa = self.shallow(ta);
                // rustc_hir_typeck/src/op.rs:29-44: parameters and aliases keep
                // the overloaded decision, even when an instance is primitive.
                if matches!(self.tcx.tys.kind(sa), TyKind::Adt(..) | TyKind::Ref(..) | TyKind::Tuple(..) | TyKind::Array(..) | TyKind::Param(_) | TyKind::Alias(..)) {
                    // `a op= b` on a non-primitive: core::ops::<Op>Assign
                    self.cur_binop = e.0;
                    self.check_op_trait(*op, sa, *b, lo, true);
                    return unit;
                }
                let is_shift = matches!(op, BinOp::Shl | BinOp::Shr);
                let tb = self.check_expr(*b, if is_shift { None } else { Some(ta) });
                // `sum += &i` with i still open and sum a literal: only core's primitive
                // `OpAssign<&T> for T` impls take a reference here, so the referent is sum's type
                let tb = match self.tcx.tys.kind(self.shallow(tb)).clone() {
                    TyKind::Ref(false, inner, _) if !is_shift && self.is_literal_var(sa) && matches!(self.tcx.tys.kind(self.shallow(inner)), TyKind::Infer(_)) => inner,
                    _ => tb,
                };
                let tb = self.peel_prim_refs(tb);
                if !is_shift {
                    self.coerce_or_unify(tb, ta, Some(*b), lo);
                }
                unit
            }
            ExprKind::Cast(x, t) => {
                let deferred_before = self.deferred_casts.len();
                let target = self.lower_ty_saving_user_provided_ty(*t);
                let st = self.shallow(target);
                // ExpectCastableToType: an unsuffixed integer literal takes the cast's integer
                // type, `u8` for `char`, `usize` for pointers and fn pointers (check_lit)
                let mut inner = *x;
                loop {
                    match self.ast().expr(inner).kind {
                        ExprKind::Paren(i) | ExprKind::Unary(UnOp::Neg, i) => inner = i,
                        _ => break,
                    }
                }
                let hint = if matches!(self.ast().expr(inner).kind, ExprKind::Lit(LitKind::Int)) {
                    match self.tcx.tys.kind(st).clone() {
                        TyKind::Int(_) => Some(st),
                        TyKind::Char => Some(self.tcx.tys.u8_),
                        TyKind::Ptr(..) | TyKind::FnDef(..) | TyKind::FnPtr(..) => Some(self.tcx.tys.usize_),
                        _ => None,
                    }
                } else if matches!(self.ast().expr(inner).kind, ExprKind::Closure(_)) {
                    // ExpectCastableToType reaches the closure's signature deduction (`to_option`):
                    // `(|_| {}) as for<'a> fn(&'a ())` is higher-ranked.
                    Some(st)
                } else { None };
                let xt = self.check_expr(*x, hint);
                if self.tcx.validates(self.prog, self.file) { self.cast_checks.push((*x, xt, target, lo)); }
                // `b as Box<dyn Tr>` / `&x as &dyn Tr`: an unsizing coercion
                let sx = self.shallow(xt);
                let (kx, kt) = (self.tcx.tys.kind(sx).clone(), self.tcx.tys.kind(st).clone());
                if matches!((&kx, &kt), (TyKind::Ref(..), TyKind::Ptr(..))) {
                    self.deferred_casts.push((*x, xt, target));
                } else if let Some((src, dy)) = self.dyn_coercion(&kx, &kt) {
                    self.set_coerce(x.0, Coerce::ToDyn(src, dy), target);
                } else if let Some((sd, dd)) = self.dyn_upcast(&kx, &kt) {
                    self.set_coerce(x.0, Coerce::Upcast(sd, dd), target);
                } else if self.unsize_coercion(&kx, &kt) {
                    // rustc check_cast: a cast that is a coercion is one (`&mut [1] as *mut [i32]`)
                    self.set_coerce(x.0, Coerce::Unsize, target);
                } else if let (TyKind::Ref(..) | TyKind::Ptr(..), TyKind::Ptr(..), true) = (&kx, &kt, {
                    // an open source pointee against a known target pointee (`&data as *const
                    // *const ()` with data: *const ?T): the cast is the coercion, T = ()
                    let tp = match &kt { TyKind::Ptr(_, p) => self.shallow(*p), _ => st };
                    let sp = match &kx { TyKind::Ref(_, p, _) | TyKind::Ptr(_, p) => *p, _ => sx };
                    !matches!(self.tcx.tys.kind(tp), TyKind::Infer(_)) && self.has_open_infer(sp)
                }) {
                    // Check inner casts first: in `r as *mut _ as *mut ()`,
                    // the reference coercion determines `_`, not the outer cast.
                    self.deferred_casts.push((*x, xt, target));
                } else if matches!((&kx, &kt), (TyKind::Ref(..) | TyKind::Ptr(..), TyKind::Ptr(_, _))) && {
                    let tp = match &kt { TyKind::Ptr(_, p) => *p, _ => st };
                    let tp = self.shallow(tp);
                    matches!(self.tcx.tys.kind(tp), TyKind::Infer(_))
                } {
                    // `p as *mut _`: decided once the target pointee is known (an unsizing cast
                    // when it becomes a slice)
                    self.deferred_casts.push((*x, xt, target));
                } else if matches!(kt, TyKind::Infer(_)) {
                    // `v as _`: the target is known only from later uses (a closure's return
                    // through `bool::then`, a `let` annotation); rustc checks every cast at the
                    // end of typeck (check_casts), where `Arc::new(v) as _` into
                    // `Arc<dyn Send + Sync>` is the unsizing coercion
                    self.deferred_casts.push((*x, xt, target));
                } else if matches!(kt, TyKind::FnPtr(..)) && matches!(kx, TyKind::FnDef(..) | TyKind::Closure(..) | TyKind::FnPtr(..)) {
                    // (a failed coercion is the cast check's E0605)
                    self.try_coerce(xt, target, Some(*x));
                } else if matches!(kt, TyKind::Adt(..) | TyKind::Tuple(_) | TyKind::Array(..)) {
                    // no primitive cast has these targets: only a coercion (a trivial cast `Some(4u32)
                    // as Option<u32>`, `(1, 2) as (u32, u32)` types its literals); when none applies
                    // the cast check reports E0605
                    self.try_coerce(xt, target, Some(*x));
                } else if let (TyKind::Ref(m1, a, _), TyKind::Ref(m2, bt, _)) = (&kx, &kt) {
                    // a cast is first a coercion when one applies (rustc cast.rs
                    // try_coercion_cast): `&s as &str` is a deref coercion of &String
                    let (sa, sb) = (self.shallow(*a), self.shallow(*bt));
                    if (*m1 || !*m2) && sa != sb && !matches!(self.tcx.tys.kind(sb), TyKind::Infer(_) | TyKind::Dyn(..)) {
                        if let Some(derefs) = self.deref_coercion(sa, sb) {
                            self.set_deref_ref(x.0, derefs, None);
                        }
                    }
                }
                if self.deferred_casts.len() == deferred_before { self.record_cast_borrow(*x, xt, target); }
                target
            }
            ExprKind::Block(b, label) => {
                if label.is_some() {
                    let l = label.map(|x| self.sym_lifetime(x));
                    self.loops.push(Loop { label: l, break_ty: expected, kind: LoopKind::Block });
                    let t = self.check_block_soft(*b, expected, soft);
                    let lp = self.loops.pop().unwrap();
                    if let Some(bt) = lp.break_ty {
                        // a body that diverges (`'b: { break 'b v; }`) has the breaks' type
                        if self.shallow(t) == self.tcx.tys.never {
                            self.diverges = false;
                            return bt;
                        }
                        self.eq(bt, t);
                    }
                    self.diverges = false;
                    return t;
                }
                self.check_block_soft(*b, expected, soft)
            }
            ExprKind::Unsafe(b) => self.check_block_soft(*b, expected, soft),
            ExprKind::ConstBlock(b) => {
                self.inline_const_floor.push(self.b.locals.len() as u32);
                let t = self.check_block(*b, expected);
                self.inline_const_floor.pop();
                self.b.inline_const_types.insert(e.0, t);
                t
            },
            ExprKind::If(c, then, els) => {
                let bool_ = self.tcx.tys.bool_;
                // `if let` bindings live in the condition and the then-block only (F23)
                let saved = self.scopes.len();
                let ct = self.check_expr(*c, Some(bool_));
                self.coerce_or_unify(ct, bool_, None, lo);
                // rustc_hir_typeck _match.rs check_then_else: the condition's divergence holds for
                // the whole `if`; the branches start from `Maybe`
                let cond_div = self.diverges;
                self.diverges = false;
                let tt = self.check_block_soft(*then, expected, soft);
                let then_div = self.diverges;
                self.scopes.truncate(saved);
                self.diverges = false;
                match &els.get() {
                    None => {
                        self.diverges = cond_div;
                        if !then_div {
                            let s = self.shallow(tt);
                            if s != unit && s != self.tcx.tys.never {
                                let pos = match self.ast().stmts(*then).last() { Some(crate::ast::Stmt::Expr(t2, false)) => self.ast().expr(*t2).lo, _ => lo };
                                self.demand_eq(unit, s, pos);
                            }
                        }
                        unit
                    }
                    Some(x) => {
                        // a diverging then-branch (`!`) is no hint for the else branch (F26)
                        let stt = self.shallow(tt);
                        // (a fn item / closure type is no hint: branches meet at a fn pointer)
                        let fnish = matches!(self.tcx.tys.kind(stt), TyKind::FnDef(..) | TyKind::Closure(..));
                        let hint = if stt == self.tcx.tys.never || fnish { expected } else { expected.or(Some(tt)) };
                        self.soft = soft || expected.is_none();
                        let et = self.check_expr(*x, hint);
                        let else_div = self.diverges;
                        self.diverges = cond_div || (then_div && else_div);
                        let ts = self.shallow(tt);
                        let es = self.shallow(et);
                        if ts == self.tcx.tys.never {
                            // the `!` then-branch is `NeverToAny` to the else type
                            if let Some(crate::ast::Stmt::Expr(t2, false)) = self.ast().stmts(*then).last() { self.set_coerce(t2.0, Coerce::Never, et); }
                            return et;
                        }
                        if es == self.tcx.tys.never {
                            self.set_coerce(x.0, Coerce::Never, tt);
                            return tt;
                        }
                        // the then-branch's value is its block's tail expression
                        let tail = match self.ast().stmts(*then).last() {
                            Some(crate::ast::Stmt::Expr(t2, false)) => vec![*t2],
                            _ => Vec::new(),
                        };
                        self.coerce_many(&tail, tt, *x, et, lo)
                    }
                }
            }
            ExprKind::Let(p, x) => {
                let t = self.check_expr(*x, None);
                // `if let true = return`: as a match scrutinee (rustc types the initializer
                // against a fresh variable, check_decl_initializer)
                let t = self.never_scrutinee(*x, t);
                self.check_pat(*p, t);
                self.tcx.tys.bool_
            }
            ExprKind::Match(x, arms) => {
                let mut st = self.check_expr(*x, None);
                if !arms.is_empty() {
                    st = self.never_scrutinee(*x, st);
                }
                // arms coerce to a known expected type (rustc _match.rs check_match:
                // CoerceMany starts from `ExpectHasType(ety)` unless it is `()`)
                let target = match expected {
                    Some(et) => {
                        let s = self.shallow(et);
                        if matches!(self.tcx.tys.kind(s), TyKind::Infer(_) | TyKind::Error) || s == unit || self.is_unsized_ty(s) {
                            None
                        } else {
                            Some(et)
                        }
                    }
                    None => None,
                };
                let mut result: Option<TyId> = target;
                let mut any_value = false;
                let mut all_div = !arms.is_empty();
                let mut seen: Vec<ExprId> = Vec::new();
                let mut never_arms: Vec<ExprId> = Vec::new();
                // _match.rs check_match: every arm's pattern is checked against the scrutinee
                // before any guard or body (a later arm's pattern types an earlier guard's binding)
                let mut arm_bindings: Vec<Vec<(Sym, u32)>> = Vec::new();
                for arm in self.ast().list(*arms) {
                    if !self.active(arm.attrs) {
                        arm_bindings.push(Vec::new());
                        continue;
                    }
                    let saved = self.scopes.len();
                    self.arm_attrs = self.ast().list(arm.attrs).to_vec();
                    self.check_pat(arm.pat, st);
                    self.arm_attrs.clear();
                    arm_bindings.push(self.scopes[saved..].to_vec());
                    self.scopes.truncate(saved);
                }
                for (arm_index, arm) in self.ast().list(*arms).iter().enumerate() {
                    if !self.active(arm.attrs) {
                        continue;
                    }
                    let saved = self.scopes.len();
                    self.scopes.extend(arm_bindings[arm_index].iter().copied());
                    if let Some(g) = arm.guard {
                        let bool_ = self.tcx.tys.bool_;
                        self.check_expr(g, Some(bool_));
                    }
                    self.diverges = false;
                    // (an earlier arm's fn item / closure type is no hint: arms meet at a fn pointer)
                    let rhint = match result {
                        Some(r) => {
                            let sr = self.shallow(r);
                            if matches!(self.tcx.tys.kind(sr), TyKind::FnDef(..) | TyKind::Closure(..)) { None } else { Some(r) }
                        }
                        None => None,
                    };
                    self.soft = soft || expected.is_none();
                    let bt = self.check_expr(arm.body, expected.or(rhint));
                    let div = self.diverges;
                    all_div = all_div && div;
                    let bs = self.shallow(bt);
                    if bs == self.tcx.tys.never { never_arms.push(arm.body); }
                    if bs != self.tcx.tys.never {
                        any_value = true;
                        match result {
                            None => result = Some(bt),
                            Some(r) => {
                                let prev = seen.clone();
                                result = Some(self.coerce_many(&prev, r, arm.body, bt, ast.expr(arm.body).lo));
                            }
                        }
                        seen.push(arm.body);
                    }
                    self.scopes.truncate(saved);
                }
                self.diverges = all_div;
                // coercion.rs CoerceMany: a `!` arm is `NeverToAny` to the match's type
                if let (true, Some(r)) = (any_value, result) {
                    for body in never_arms { self.set_coerce(body.0, Coerce::Never, r); }
                }
                match result {
                    Some(r) if any_value => r,
                    _ => self.tcx.tys.never,
                }
            }
            ExprKind::While(row) => {
                let crate::ast::WhileRow { cond: c, body: b, label } = &self.ast().row(*row).clone();
                let (c, b) = (c, b);
                let label = &crate::ast::OptIdent::from(*label);
                let bool_ = self.tcx.tys.bool_;
                let saved = self.scopes.len();
                let l = label.map(|x| self.sym_lifetime(x));
                // rustc_ast_lowering puts both the condition and body in the loop scope.
                self.loops.push(Loop { label: l, break_ty: None, kind: LoopKind::While });
                let was_in_condition = std::mem::replace(&mut self.in_loop_condition, true);
                let ct = self.check_expr(*c, Some(bool_));
                let cpos = self.ast().expr(*c).lo;
                self.coerce_or_unify(ct, bool_, Some(*c), cpos);
                self.in_loop_condition = false;
                // a loop body has type `()`: an open tail type is unit (`for _ in r { zeroed() }`)
                self.check_block_no_value(*b);
                self.loops.pop();
                self.in_loop_condition = was_in_condition;
                self.scopes.truncate(saved);
                self.diverges = false;
                unit
            }
            ExprKind::Loop(b, label) => {
                let l = label.map(|x| self.sym_lifetime(x));
                self.loops.push(Loop { label: l, break_ty: None, kind: LoopKind::Loop });
                let was_in_condition = std::mem::replace(&mut self.in_loop_condition, false);
                // a loop body has type `()`: an open tail type is unit (`for _ in r { zeroed() }`)
                self.check_block_no_value(*b);
                let lp = self.loops.pop().unwrap();
                self.in_loop_condition = was_in_condition;
                match lp.break_ty {
                    Some(t) => {
                        self.diverges = false;
                        t
                    }
                    None => {
                        // a loop without `break` never yields
                        if self.loop_has_break(*b, label.map(|x| self.sym_lifetime(x))) {
                            self.diverges = false;
                            unit
                        } else {
                            self.diverges = true;
                            self.tcx.tys.never
                        }
                    }
                }
            }
            ExprKind::For(row) => {
                let crate::ast::ForRow { pat: p, iter: it, body: b, label } = self.ast().row(*row);
                let saved = self.scopes.len();
                let item = self.check_for_iter(*it, lo);
                self.check_pat(*p, item);
                let l = label.map(|x| self.sym_lifetime(x));
                self.loops.push(Loop { label: l, break_ty: None, kind: LoopKind::For });
                let was_in_condition = std::mem::replace(&mut self.in_loop_condition, false);
                // a loop body has type `()`: an open tail type is unit (`for _ in r { zeroed() }`)
                self.check_block_no_value(*b);
                self.loops.pop();
                self.in_loop_condition = was_in_condition;
                self.scopes.truncate(saved);
                self.diverges = false;
                unit
            }
            ExprKind::Break(label, x) => {
                // rustc_hir_typeck/loops.rs require_label_in_labeled_block: an unlabeled break directly in a labeled block
                if label.is_none() && matches!(self.loops.last(), Some(Loop { kind: LoopKind::Block, .. })) {
                    if let Some(v) = x.get() { self.check_expr(v, None); }
                    self.err(lo, "E0695: unlabeled `break` inside of a labeled block".to_string());
                    return self.tcx.tys.never;
                }
                if self.in_loop_condition && label.is_none() {
                    self.err(lo, "E0590: `break` or `continue` with no label in the condition of a `while` loop".to_string());
                }
                let l = label.map(|x| self.sym_lifetime(x));
                if x.get().is_some() {
                    if let Some(i) = self.loop_index(l) {
                        // loops.rs BreakNonLoop: `break` with a value is only for `loop` (and labeled blocks)
                        if matches!(self.loops[i].kind, LoopKind::While | LoopKind::For) {
                            let w = if self.loops[i].kind == LoopKind::While { "while" } else { "for" };
                            self.err(lo, format!("E0571: `break` with value from a `{}` loop", w));
                        }
                    }
                }
                let t = match &x.get() {
                    Some(v) => {
                        let target = self.loop_index(l);
                        let exp = match target {
                            Some(i) => self.loops[i].break_ty,
                            None => None,
                        };
                        let vt = self.check_expr(*v, exp);
                        Some(vt)
                    }
                    None => None,
                };
                if let Some(i) = self.loop_index(l) {
                    let bt = match t {
                        Some(v) => v,
                        None => unit,
                    };
                    if matches!(self.loops[i].kind, LoopKind::Loop | LoopKind::Block) {
                        match self.loops[i].break_ty {
                            Some(prev) => {
                                // rustc check_expr_break: every `break` coerces to the loop's type
                                let bpos = match x.get() { Some(v) => self.ast().expr(v).lo, None => lo };
                                self.demand_eq(prev, bt, bpos);
                            }
                            None => self.loops[i].break_ty = Some(bt),
                        }
                    }
                } else if let Some((_, _)) = l {
                    self.err(lo, format!("E0426: use of undeclared label `{}`", self.text(label.get().unwrap())));
                } else {
                    self.err(lo, "E0268: `break` outside of a loop or labeled block".to_string());
                }
                self.tcx.tys.never
            }
            ExprKind::Continue(label) => {
                if label.is_none() && matches!(self.loops.last(), Some(Loop { kind: LoopKind::Block, .. })) {
                    self.err(lo, "E0695: unlabeled `continue` inside of a labeled block".to_string());
                    return self.tcx.tys.never;
                }
                if let Some(i) = label.get().and_then(|_| self.loop_index(label.map(|x| self.sym_lifetime(x)))) {
                    if self.loops[i].kind == LoopKind::Block {
                        self.err(lo, "E0696: `continue` pointing to a labeled block".to_string());
                        return self.tcx.tys.never;
                    }
                }
                if self.in_loop_condition && label.is_none() {
                    self.err(lo, "E0590: `break` or `continue` with no label in the condition of a `while` loop".to_string());
                }
                let l = label.map(|x| self.sym_lifetime(x));
                if self.loop_index(l).is_none() {
                    match label.get() {
                        Some(x) => self.err(lo, format!("E0426: use of undeclared label `{}`", self.text(x))),
                        None => self.err(lo, "E0268: `continue` outside of a loop".to_string()),
                    }
                }
                self.tcx.tys.never
            }
            ExprKind::Return(x) => {
                if self.in_const && self.closure_depth.is_empty() {
                    self.err(lo, "E0572: return statement outside of function body".to_string());
                }
                let rt = *self.ret_stack.last().unwrap();
                match &x.get() {
                    Some(v) => {
                        let t = self.check_expr(*v, Some(rt));
                        self.coerce_or_unify(t, rt, Some(*v), lo);
                    }
                    None => {
                        // coerce_forced_unit: `return;` where the return type is not `()`
                        if !self.eq(rt, unit) && !self.tcx.tys.has_error(rt) {
                            self.err(lo, "E0069: `return;` in a function whose return type is not `()`".to_string());
                        }
                    }
                }
                self.tcx.tys.never
            }
            ExprKind::Tuple(v) => {
                let exp_elems = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Tuple(x) if self.tcx.tys.list(x).len() == v.len() => Some(x),
                            _ => None,
                        }
                    }
                    None => None,
                };
                let mut ts = Vec::new();
                for (i, x) in self.ast().list(*v).iter().enumerate() {
                    let ex = match &exp_elems {
                        Some(es) => Some(self.tcx.tys.list(*es)[i]),
                        None => None,
                    };
                    let t = self.check_expr(*x, ex);
                    // expr.rs check_expr_tuple: every element is coerced to the expected element type
                    // or to a fresh variable (`(loop {},)` is `(?T,)`, its `!` coerced to the variable)
                    // (to a variable only a diverging element differs from the element's own type)
                    let et = match ex {
                        Some(et) => et,
                        None if matches!(self.tcx.tys.kind(self.shallow(t)), TyKind::Never) => self.fresh(0),
                        None => t,
                    };
                    if et != t { self.coerce_or_unify(t, et, Some(*x), lo); }
                    ts.push(et);
                }
                self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&ts)))
            }
            ExprKind::Array(v) => {
                let elem_exp = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Array(e, _) | TyKind::Slice(e) => Some(e),
                            _ => None,
                        }
                    }
                    None => None,
                };
                // an expected element type that is still open is no constraint yet
                let elem_open = elem_exp.map_or(false, |t| {
                    let st = self.shallow(t);
                    matches!(self.tcx.tys.kind(st), TyKind::Infer(_)) && !self.is_literal_var(st)
                });
                let elem = match elem_exp.filter(|_| !elem_open) {
                    Some(t) => t,
                    None => {
                        // no expected element type: the elements meet like match arms
                        // (rustc CoerceMany: `[foo, bar]` of two fn items is `[fn(); 2]`)
                        let mut res: Option<TyId> = None;
                        let mut seen: Vec<ExprId> = Vec::new();
                        for x in self.ast().list(*v) {
                            let hint = res.filter(|r| {
                                let sr = self.shallow(*r);
                                !matches!(self.tcx.tys.kind(sr), TyKind::FnDef(..) | TyKind::Closure(..))
                            });
                            let t = self.check_expr(*x, hint);
                            res = Some(match res {
                                None => t,
                                Some(r) => {
                                    let prev = seen.clone();
                                    self.coerce_many(&prev, r, *x, t, lo)
                                }
                            });
                            seen.push(*x);
                        }
                        let e = res.unwrap_or_else(|| self.fresh(0));
                        if let Some(x) = elem_exp {
                            self.eq(x, e);
                        }
                        return self.tcx.tys.intern(TyKind::Array(e, self.tcx.tys.const_usize(v.len() as u64)));
                    }
                };
                for x in self.ast().list(*v) {
                    let t = self.check_expr(*x, Some(elem));
                    self.coerce_or_unify(t, elem, Some(*x), lo);
                }
                self.tcx.tys.intern(TyKind::Array(elem, self.tcx.tys.const_usize(v.len() as u64)))
            }
            ExprKind::Repeat(x, n) => {
                let elem_exp = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Array(e, _) => Some(e),
                            _ => None,
                        }
                    }
                    None => None,
                };
                let mut t = self.check_expr(*x, elem_exp);
                // the operand is a coercion site for the expected element type (`[&[]; 4]` as
                // `[&[i16]; 4]`: &[i16; 0] -> &[i16]), as an array literal's elements are
                if let Some(et) = elem_exp {
                    let se = self.shallow(et);
                    let open = matches!(self.tcx.tys.kind(se), TyKind::Infer(_));
                    if !open && self.coerce_or_unify(t, et, Some(*x), self.ast().expr(*x).lo) {
                        t = et;
                    }
                }
                // `[x; _]`: the length is inferred (rustc generic_arg_infer)
                if crate::tcx::is_infer_len(self.ast(), *n) {
                    let arr = self.tcx.tys.mk_array(t, self.infcx.next_const_var());
                    // expr.rs:1775: the array type is well formed
                    self.register_wf(arr, lo);
                    self.deferred_repeats.push((*x, t, arr));
                    return arr;
                }
                let usize_ = self.tcx.tys.usize_;
                self.check_expr(*n, Some(usize_));
                // hir_ty_lowering/mod.rs:2390: retain the repeat body's identity;
                // fulfillment evaluates it through the same query as array types.
                let len = self.tcx.lower_const_arg(self.prog, self.file, &self.env, *n, usize_);
                let len = self.tcx.tys.intern(TyKind::Const(len));
                let len = self.normalize_body_type(len);
                let arr = self.tcx.tys.mk_array(t, self.tcx.tys.expect_const(len));
                self.register_wf(arr, lo);
                self.deferred_repeats.push((*x, t, arr));
                arr
            }
            ExprKind::Field(x, name) => self.check_field(e, *x, *name, lo),
            ExprKind::TupleField(x, idx) => {
                let t = self.check_expr(*x, None);
                let mut cur = self.shallow(t);
                let mut derefs = 0usize;
                loop {
                    // field access autoderef never goes through a raw pointer (Autoderef without include_raw_pointers)
                    if matches!(self.tcx.tys.kind(cur), TyKind::Ptr(..)) { break; }
                    match self.tcx.tys.kind(cur).clone() {
                        TyKind::Tuple(v) => {
                            self.b.field_idx.insert(e.0, (derefs as u32, *idx));
                            self.set_operand_derefs(e.0, derefs);
                            return self.tcx.tys.list(v).get(*idx as usize).copied().unwrap_or(error);
                        }
                        TyKind::Adt(d, args, regions) => {
                            let adt = self.tcx.adts.get(&d);
                            if let Some(adt) = adt {
                                if !adt.is_enum {
                                    if let Some(f) = adt.variants[0].fields.get(*idx as usize) {
                                        if self.field_visible(d, f) {
                                            self.b.field_idx.insert(e.0, (derefs as u32, *idx));
                                            self.set_operand_derefs(e.0, derefs);
                                            return self.instantiate_field_type(f.ty, adt.region_params, regions, self.tcx.tys.list(args));
                                        }
                                    }
                                }
                            }
                            match self.deref_step(e.0, derefs, cur) {
                                Some(i) => {
                                    cur = i;
                                    derefs += 1;
                                }
                                None => break,
                            }
                        }
                        _ => match self.deref_step(e.0, derefs, cur) {
                            Some(i) => {
                                cur = i;
                                derefs += 1;
                            }
                            None => break,
                        },
                    }
                }
                let s = self.ty_str(t);
                self.err(lo, format!("E0609: no field `{}` on type `{}`", idx, s));
                error
            }
            ExprKind::Index(a, i) => {
                let ta = self.check_expr(*a, None);
                let ti = self.check_expr(*i, None);
                let mut cur = self.shallow(ta);
                let mut derefs = 0usize;
                let tis = self.shallow(ti);
                let idx_is_range = matches!(self.tcx.tys.kind(tis), TyKind::Adt(..));
                // an integer literal index stays open until the indexing is known: `f[1]` with
                // `impl Index<isize> for Foo` is isize (rustc: the literal is typed by the
                // selected Index impl); builtin array/slice indexing makes it usize
                let lit_index = self.is_literal_var(tis);
                // an index whose type is still open (`tree[ix]`, ix from an Option filled later)
                // is fixed by the indexing selected below, not assumed usize up front (rustc
                // lookup_indexing: `T: Index<?I>`)
                let open_index = !lit_index && matches!(self.tcx.tys.kind(tis), TyKind::Infer(_));
                let mut idx_mismatch = false;
                if !idx_is_range && !lit_index && !open_index {
                    let usize_ = self.tcx.tys.usize_;
                    idx_mismatch = !self.eq(tis, usize_) && !matches!(self.tcx.tys.kind(tis), TyKind::Error);
                }
                loop {
                    match self.tcx.tys.kind(cur).clone() {
                        TyKind::Array(el, _) | TyKind::Slice(el) => {
                            self.set_operand_derefs(e.0, derefs);
                            // an index type of the program's own (`impl<T> Index<TableIndex> for
                            // [T]`): the overloaded Index of the array, else of the slice it
                            // unsizes to (rustc method/probe.rs lookup_indexing: unsize step)
                            // (a generic index of a polymorphic body -- `I: SliceIndex<[T]>` --
                            // goes through std's `impl<T, I: SliceIndex<[T]>> Index<I> for [T]`:
                            // the result is `<I as SliceIndex<[T]>>::Output`)
                            let std_range = match self.tcx.tys.kind(tis) {
                                TyKind::Adt(d, _, _) => matches!(self.prog.name(*d), "Range" | "RangeFrom" | "RangeTo" | "RangeFull" | "RangeInclusive" | "RangeToInclusive"),
                                TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) => false,
                                _ => true,
                            };
                            if !std_range {
                                if let Some(td) = self.lang_item(&["ops", "Index"]) {
                                    let ti2 = self.infcx.deeply_resolve_ignoring_regions(ti);
                                    let slice = self.tcx.tys.intern(TyKind::Slice(el));
                                    for st in [cur, slice] {
                                        let ds = self.infcx.deeply_resolve_ignoring_regions(st);
                                        if !matches!(self.select_trait(td, ds, &[ti2]), Err(_)) {
                                            let m = self.trait_fn(td, "index").unwrap();
                                            self.b.ov_index.insert(e.0, (m, self.tcx.tys.mk_list(&[ds, ti2])));
                                            let out = self.tcx.trait_assoc(self.prog, td, self.prog.syms.get("Output").unwrap()).unwrap();
                                            let r = self.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: out }, args: self.tcx.tys.mk_list(&[ds, ti2]), regions: crate::types::RegionArgsId::EMPTY }));
                                            return self.norm_keep(r);
                                        }
                                    }
                                }
                            }
                            if idx_mismatch {
                                // rustc lookup_indexing: no `Index<I>` impl for the slice
                                let (sa, si) = (self.ty_str(self.tcx.tys.intern(TyKind::Slice(el))), self.ty_str(tis));
                                self.err(lo, format!("E0277: the type `{}` cannot be indexed by `{}`", sa, si));
                                return el;
                            }
                            if idx_is_range {
                                self.record_std_index(e.0, cur, ti);
                                return self.tcx.tys.intern(TyKind::Slice(el));
                            }
                            if lit_index || open_index {
                                let usize_ = self.tcx.tys.usize_;
                                self.eq(tis, usize_);
                            }
                            return el;
                        }
                        TyKind::Str if idx_is_range => {
                            self.set_operand_derefs(e.0, derefs);
                            self.record_std_index(e.0, cur, ti);
                            return self.tcx.tys.str_;
                        }
                        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) => {
                            cur = self.shallow(x);
                            self.set_operand_deref(e.0, derefs, None, cur);
                            derefs += 1;
                        }
                        TyKind::Adt(..) | TyKind::Param(_) | TyKind::Alias(..) | TyKind::Dyn(..) => {
                            // `Index<Idx>` impl on the type, else autoderef through Deref
                            if let Some(td) = self.lang_item(&["ops", "Index"]) {
                                let dc = self.infcx.deeply_resolve_ignoring_regions(cur);
                                let ti2 = self.infcx.deeply_resolve_ignoring_regions(ti);
                                let mut found = self.select_trait(td, dc, &[ti2]);
                                let mut coerced_idx: Option<TyId> = None;
                                if matches!(found, Err(_)) && !idx_is_range && !lit_index {
                                    // the index coerces to the impl's index type (`S[bar]` with
                                    // `impl Index<fn()> for S`: rustc coerces the index operand to
                                    // the input type of the selected impl)
                                    let wild = self.fresh(0);
                                    if let Ok(Some(selected)) = self.select_trait(td, dc, &[wild]) {
                                        let (imp, iargs) = match selected { ImplSource::UserDefined { impl_def_id, args, .. } => (impl_def_id, args), _ => (NO_DEF, Vec::new()) };
                                        if let Some(&ia) = self.tcx.impl_trait_args.get(&imp).and_then(|v| v.first()) {
                                            let want = self.tcx.instantiate_ty(self.prog, imp, &iargs, ia);
                                            let snap = self.start_snapshot();
                                            if !self.tcx.has_param(want) && self.try_coerce(ti, want, Some(*i)) {
                                                self.commit_from(snap);
                                                coerced_idx = Some(want);
                                                found = self.select_trait(td, dc, &[want]);
                                            } else {
                                                self.rollback_to(snap);
                                            }
                                        }
                                    }
                                }
                                if !matches!(found, Err(_)) {
                                    self.set_operand_derefs(e.0, derefs);
                                    let m = self.trait_fn(td, "index").unwrap();
                                    let index = coerced_idx.unwrap_or(ti2);
                                    self.b.ov_index.insert(e.0, (m, self.tcx.tys.mk_list(&[dc, index])));
                                    let output = self.tcx.trait_assoc(self.prog, td, self.prog.syms.get("Output").unwrap()).unwrap();
                                    let projection = self.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: output }, args: self.tcx.tys.mk_list(&[dc, index]), regions: crate::types::RegionArgsId::EMPTY }));
                                    return self.norm_keep(projection);
                                }
                            }
                            match self.deref_step(e.0, derefs, cur) {
                                Some(i) => {
                                    cur = i;
                                    derefs += 1;
                                }
                                None => break,
                            }
                        }
                        _ => break,
                    }
                }
                let s = self.ty_str(ta);
                self.err(lo, format!("E0608: cannot index into a value of type `{}`", s));
                error
            }
            ExprKind::AddrOf(raw, m, x) => {
                let exp_inner = match expected {
                    Some(t) => {
                        let s = self.infcx.shallow_resolve(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Ref(_, i, _) => Some(i),
                            _ => None,
                        }
                    }
                    None => None,
                };
                // `&[a, b]` against `&[T]`: check the array with the slice's element. Otherwise an
                // unsized referent (`&str`, `&dyn T`) is only a hint for a value operand (rustc
                // check_expr_addr_of: rvalue_hint -> ExpectRvalueLikeUnsized); the coercion applies
                // to the whole `&expr` (`&if c { a } else { b }` with String branches)
                let exp_inner = match exp_inner {
                    Some(i) => {
                        let si = self.shallow(i);
                        let unsized_ref = matches!(self.tcx.tys.kind(si), TyKind::Str | TyKind::Dyn(..) | TyKind::Slice(..));
                        let operand = &self.ast().expr(*x).kind;
                        let value_operand = !matches!(operand, ExprKind::Path(_) | ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) | ExprKind::Unary(UnOp::Deref, _) | ExprKind::Paren(_) | ExprKind::Array(..) | ExprKind::Repeat(..));
                        // a closure still deduces its signature from the hint (rustc closure.rs
                        // check_expr_closure: `expected.to_option()` includes RvalueLikeUnsized)
                        let closure = matches!(operand, ExprKind::Closure { .. });
                        if unsized_ref && value_operand && !closure { None } else { Some(i) }
                    }
                    None => None,
                };
                let t = self.check_expr(*x, exp_inner);
                if *raw {
                    // expr.rs check_named_place_expr: `&raw const <temporary>` (E0745)
                    if self.tcx.validates(self.prog, self.file) && !self.is_named_place(*x) {
                        self.err(self.ast().expr(*x).lo, "E0745: cannot take address of a temporary".to_string());
                    }
                    self.tcx.tys.intern(TyKind::Ptr(*m, t))
                } else {
                    self.tcx.tys.intern(TyKind::Ref(*m, t, self.fresh_region()))
                }
            }
            ExprKind::Call(f, args) => self.check_call(e, *f, self.ast().list(*args), expected, lo),
            ExprKind::MethodCall(recv, row) => {
                let crate::ast::MethodCallRow { name, turbofish, args } = self.ast().row(*row);
                self.method_expected = expected;
                self.check_method(e, *recv, *name, *turbofish, self.ast().list(*args), lo)
            }
            ExprKind::Struct(row) => {
                let crate::ast::StructRow { path, fields, base, rest } = self.ast().row(*row);
                let r = self.resolve_struct_path(path);
                self.b.res.insert(e.0, r);
                let d = match r {
                    Res::Def(d) => d,
                    _ => return error,
                };
                let (adt_ty, vi) = self.adt_of_ctor(d);
                let adt_ty = self.instantiate_adt(adt_ty);
                let adt_ty = if self.prog.def(d).kind == DefKind::Variant { self.variant_path_ty(path, d, adt_ty) } else { self.struct_path_ty(path, adt_ty) };
                self.write_adt_user_type(e.0, adt_ty);
                // N:rustc_hir_typeck/src/expr.rs:1823 and fn_ctxt/_impl.rs:1293-1301:
                // obligations belong to the instantiated path, including explicit args.
                self.register_wf(adt_ty, lo);
                // an unsized expectation (`Wrap<dyn Debug>` for `Box::new(Wrap { .. })`) is no
                // type to check against (rustc only_has_type of ExpectRvalueLikeUnsized)
                if let Some(x) = expected {
                    if !self.is_unsized_ty(x) {
                        self.eq(adt_ty, x);
                    }
                }
                let base = base.get();
                let mut given: Vec<u32> = Vec::new();
                for fe in self.ast().list(*fields).iter() {
                    let fname = self.sym(fe.name);
                    let fname = if fe.name.hi > fe.name.lo && self.text(fe.name).as_bytes()[0].is_ascii_digit() {
                        crate::ast::tuple_index(self.text(fe.name)).map_or(u32::MAX - 1, |n| n | 0x8000_0000)
                    } else {
                        fname
                    };
                    let fx = fe.expr;
                    match self.field_ty(adt_ty, vi, fname) {
                        Some((fi, ft)) => {
                            // rustc check_expr_struct_fields: E0062 for a field given twice
                            if given.contains(&fi) {
                                self.err(fe.name.lo, format!("E0062: field `{}` specified more than once", self.text(fe.name)));
                            }
                            given.push(fi);
                            self.check_field_privacy(adt_ty, vi, fi, fe.name.lo);
                            let t = self.check_expr(fx, Some(ft));
                            self.coerce_or_unify(t, ft, Some(fx), fe.name.lo);
                        }
                        None => {
                            let what = self.adt_field_owner(adt_ty, vi);
                            let code = if what.starts_with("variant") { "E0559" } else { "E0560" };
                            self.err(fe.name.lo, format!("{}: {} has no field named `{}`", code, what, self.text(fe.name)));
                        }
                    }
                }
                if let Some(bx) = base {
                    // check_expr_struct_fields: `..base` is for structs
                    if self.prog.def(d).kind == DefKind::Variant && self.tcx.validates(self.prog, self.file) {
                        let pos = self.ast().expr(bx).lo;
                        self.err(pos, "E0436: functional record update syntax requires a struct".to_string());
                    }
                    let t = self.check_expr(bx, Some(adt_ty));
                    // expr.rs check_expr_struct_fields: a base of the same ADT with other
                    // generic arguments is the unstable type_changing_struct_update
                    let (bt, at) = (self.shallow(t), self.shallow(adt_ty));
                    if let (TyKind::Adt(bd, ..), TyKind::Adt(ad, ..)) = (self.tcx.tys.kind(bt).clone(), self.tcx.tys.kind(at).clone()) {
                        let krate = self.prog.def(self.b.def).krate;
                        if bd == ad && !self.can_eq(bt, at) && !self.prog.crates[krate as usize].features(self.prog).has("type_changing_struct_update") {
                            let pos = self.ast().expr(bx).lo;
                            self.err(pos, "E0658: type changing struct updating is experimental".to_string());
                        }
                    }
                    let pos = self.ast().expr(bx).lo;
                    self.coerce_or_unify(t, adt_ty, Some(bx), pos);
                    // the fields taken from the base must be visible too (rustc_hir_typeck
                    // check_expr_struct_fields: private fields of a functional update)
                    if let TyKind::Adt(ad, _, _) = self.tcx.tys.kind(self.shallow(adt_ty)).clone() {
                        if let Some(n) = self.tcx.adts.get(&ad).and_then(|a| a.variants.get(vi as usize)).map(|v| v.fields.len()) {
                            for fi in 0..n as u32 {
                                if !given.contains(&fi) {
                                    self.check_field_privacy(adt_ty, vi, fi, lo);
                                }
                            }
                        }
                    }
                }
                if *rest {
                    self.struct_defaults(e, d, adt_ty, vi, &given, lo);
                } else if base.is_none() {
                    let n = self.ast().list(*fields).len();
                    self.missing_fields(adt_ty, vi, &given, n, lo);
                }
                adt_ty
            }
            ExprKind::Range(a, b, incl) => {
                let elem = self.fresh(0);
                if let Some(a) = a.get() {
                    let t = self.check_expr(a, Some(elem));
                    self.eq(t, elem);
                }
                if let Some(b) = b.get() {
                    let t = self.check_expr(b, Some(elem));
                    self.eq(t, elem);
                }
                let name = match (a.is_some(), b.is_some(), *incl) {
                    (true, true, false) => "Range",
                    (true, true, true) => "RangeInclusive",
                    (true, false, _) => "RangeFrom",
                    (false, true, false) => "RangeTo",
                    (false, true, true) => "RangeToInclusive",
                    (false, false, _) => "RangeFull",
                };
                match self.lang_item(&["ops", name]) {
                    Some(d) => {
                        let n = self.tcx.adts.get(&d).map_or(0, |a| a.n_generics);
                        let args = if n == 0 { Vec::new() } else { vec![elem] };
                        self.tcx.tys.intern(TyKind::Adt(d, self.tcx.tys.mk_list(&args), crate::types::RegionArgsId::EMPTY))
                    }
                    None => {
                        self.err(lo, format!("core::ops::{} not found", name));
                        error
                    }
                }
            }
            ExprKind::Closure(row) => { let crate::ast::ClosureRow { params, ret, body, .. } = self.ast().row(*row); self.check_closure(e, self.ast().list(*params), *ret, *body, expected) }
            ExprKind::Mac(m) => self.check_mac(e, self.ast().row(*m), expected, lo),
            ExprKind::Try(x) => self.check_try(e, *x, lo),
            ExprKind::Async(mode, b) => self.check_async_block(e, *mode, *b),
            ExprKind::Await(x) => self.check_await(*x, lo),
            ExprKind::Yield(_) | ExprKind::Box(_) => {
                self.err(lo, "unsupported expression (yield, box)".to_string());
                error
            }
            ExprKind::Underscore => self.fresh(0),
        }
    }

    fn sym_lifetime(&self, id: Ident) -> (Sym, u32) {
        // lifetimes are not interned as idents; use the text without the quote
        let t = self.text(id);
        let t = t.trim_start_matches('\'');
        (self.prog.syms.get(t).unwrap_or(u32::MAX - 1), id.lo)
    }

    fn loop_index(&self, label: Option<(Sym, u32)>) -> Option<usize> {
        let mut i = self.loops.len();
        while i > 0 {
            i -= 1;
            match label {
                None => {
                    // unlabeled break targets the innermost real loop (not a labeled block)
                    return Some(i);
                }
                Some(l) => {
                    // labels are hygienic like locals (rustc)
                    if let Some(d) = self.loops[i].label {
                        if d.0 == l.0 && self.prog.files[self.file as usize].hyg_visible(d.1, l.1) {
                            return Some(i);
                        }
                    }
                }
            }
        }
        None
    }

    fn loop_has_break(&self, b: BlockId, label: Option<(Sym, u32)>) -> bool {
        // conservative syntactic scan: any `break` that targets this loop
        let ast = self.ast();
        let blk = ast.block(b);
        let mut found = false;
        for s in ast.list(blk.stmts) {
            match s {
                Stmt::Expr(e, _) => found = found || self.expr_has_break(*e, 0, label),
                Stmt::Let(r) => if let Some(e) = &ast.row(*r).init { found = found || self.expr_has_break(*e, 0, label) },
                _ => {}
            }
        }
        found
    }

    fn expr_has_break(&self, e: ExprId, depth: u32, label: Option<(Sym, u32)>) -> bool {
        let ast = self.ast();
        match &ast.expr(e).kind {
            ExprKind::Break(l, x) => {
                let target_ok = match &l.get() {
                    None => depth == 0,
                    Some(li) => label.map_or(false, |l| self.sym_lifetime(*li).0 == l.0),
                };
                target_ok || x.map_or(false, |v| self.expr_has_break(v, depth, label))
            }
            ExprKind::Loop(b, _) => self.block_has_break(*b, depth + 1, label),
            ExprKind::While(row) => self.block_has_break(self.ast().row(*row).body, depth + 1, label),
            ExprKind::For(row) => self.block_has_break(self.ast().row(*row).body, depth + 1, label),
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => self.block_has_break(*b, depth, label),
            ExprKind::If(c, t, el) => {
                self.expr_has_break(*c, depth, label) || self.block_has_break(*t, depth, label) || el.map_or(false, |x| self.expr_has_break(x, depth, label))
            }
            ExprKind::Match(x, arms) => {
                let mut r = self.expr_has_break(*x, depth, label);
                for a in self.ast().list(*arms) {
                    r = r || self.expr_has_break(a.body, depth, label);
                }
                r
            }
            ExprKind::Paren(x) => self.expr_has_break(*x, depth, label),
            _ => false,
        }
    }

    fn block_has_break(&self, b: BlockId, depth: u32, label: Option<(Sym, u32)>) -> bool {
        let ast = self.ast();
        for s in ast.stmts(b) {
            let r = match s {
                Stmt::Expr(e, _) => self.expr_has_break(*e, depth, label),
                Stmt::Let(r) => ast.row(*r).init.map_or(false, |e| self.expr_has_break(e, depth, label)),
                _ => false,
            };
            if r {
                return true;
            }
        }
        false
    }

    /// rustc_ast_lowering/src/expr.rs:1945-2033 @8d1a764: call Try::branch,
    /// match the ControlFlow lang-item variants, and return FromResidual on Break.
    /// The typed AST records the two desugared calls for its lowering consumers.
    fn check_try(&mut self, e: ExprId, x: ExprId, lo: u32) -> TyId {
        use crate::lang_items::LangItem;
        let t = self.check_expr(x, None);
        let Some(branch) = self.tcx.lang_items.get(LangItem::TryTraitBranch) else {
            self.err(lo, "missing `branch` lang item".into()); return self.tcx.tys.error;
        };
        let Some(from_residual) = self.tcx.lang_items.get(LangItem::TryTraitFromResidual) else {
            self.err(lo, "missing `from_residual` lang item".into()); return self.tcx.tys.error;
        };
        let full = self.infer_item_args(branch, &[t]);
        let sig = self.tcx.fn_sig(self.prog, branch).unwrap().clone();
        let (ps, flow) = self.instantiate_call_signature(branch, &full, &sig);
        self.coerce_or_unify(t, ps[0], Some(x), lo);
        self.register_method_obligation(branch, &full);
        self.apply_fn_bounds(branch, &full, lo);
        let flow = self.normalize_signature(flow);
        let TyKind::Adt(def, flow_args, regions) = self.tcx.tys.kind(flow).clone() else { return self.tcx.tys.error; };
        let adt = &self.tcx.adts[&def];
        let continue_def = self.tcx.lang_items.get(LangItem::ControlFlowContinue);
        let break_def = self.tcx.lang_items.get(LangItem::ControlFlowBreak);
        let output = adt.variants.iter().find(|v| Some(v.def) == continue_def).unwrap().fields[0].ty;
        let residual = adt.variants.iter().find(|v| Some(v.def) == break_def).unwrap().fields[0].ty;
        let flow_full = self.tcx.complete_args(self.prog, def, self.tcx.tys.list(flow_args), regions);
        let output = self.tcx.instantiate_ty(self.prog, def, &flow_full, output);
        let residual = self.tcx.instantiate_ty(self.prog, def, &flow_full, residual);
        let ret = *self.ret_stack.last().unwrap_or(&self.tcx.tys.error);
        let full = self.infer_item_args(from_residual, &[ret, residual]);
        self.register_method_obligation(from_residual, &full);
        self.apply_fn_bounds(from_residual, &full, lo);
        self.b.dependencies.push(branch);
        self.b.dependencies.push(from_residual);
        self.b.try_branch.insert(e.0, (branch, self.tcx.tys.mk_list(&[t]), flow));
        self.b.try_from_residual.insert(e.0, (from_residual, self.tcx.tys.mk_list(&[ret, residual])));
        output
    }

    pub fn lang_item(&self, path: &[&str]) -> Option<DefId> {
        let mut key = String::with_capacity(32);
        for p in path {
            key.push_str(p);
            key.push(':');
        }
        if let Some(r) = self.tcx.core_path_items.get(&key) {
            return r;
        }
        let r = self.lang_item_uncached(path);
        self.tcx.core_path_items.insert(key, r);
        r
    }

    fn lang_item_uncached(&self, path: &[&str]) -> Option<DefId> {
        // core::<path>
        let prog = self.prog;
        let core = prog.syms.get("core")?;
        let mut cur = None;
        for c in 0..prog.crates.len() {
            if prog.crates[c].name == core {
                cur = Some(prog.mods[prog.crates[c].root_mod as usize].def);
            }
        }
        let mut cur = cur?;
        for p in path {
            let s = prog.syms.get(p)?;
            cur = match prog.lookup_in_container(cur, s, true) {
                Some(d) => d,
                None => prog.lookup_in_container(cur, s, false)?,
            };
        }
        Some(cur)
    }

    fn is_numeric(&self, t: TyId) -> bool {
        match self.tcx.tys.kind(t) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Error => true,
            TyKind::IntVar(_) | TyKind::FloatVar(_) => true,
            _ => false,
        }
    }

    fn check_lit(&mut self, e: ExprId, k: LitKind, expected: Option<TyId>) -> TyId {
        let f = &self.prog.files[self.file as usize];
        let ex = f.ast.expr(e);
        let text = std::str::from_utf8(f.bytes(ex.lo, ex.hi)).unwrap_or("");
        match k {
            LitKind::Int | LitKind::Float => {
                if let Some(suf) = crate::const_fold::int_suffix(text) {
                    for (n, p) in crate::program::PRIMS {
                        if n == suf {
                            return self.tcx.tys.prim(p);
                        }
                    }
                }
                let want_float = k == LitKind::Float;
                if let Some(x) = expected {
                    let s = self.shallow(x);
                    let ok = match self.tcx.tys.kind(s) {
                        // rustc_hir_typeck check_lit: an unsuffixed integer literal takes an integer
                        // expectation only, a float literal a float one
                        TyKind::Int(_) => !want_float,
                        TyKind::Float(_) => want_float,
                        TyKind::IntVar(_) => !want_float,
                        TyKind::FloatVar(_) => want_float,
                        _ => false,
                    };
                    if ok {
                        return s;
                    }
                }
                self.fresh(if want_float { 2 } else { 1 })
            }
            LitKind::Bool(_) => self.tcx.tys.bool_,
            LitKind::Char => self.tcx.tys.char_,
            LitKind::Byte => self.tcx.tys.u8_,
            LitKind::Str | LitKind::RawStr => self.tcx.tys.str_ref,
            LitKind::ByteStr => {
                let n = crate::lit::unescape_bytes(text).len() as u64;
                let arr = self.tcx.tys.intern(TyKind::Array(self.tcx.tys.u8_, self.tcx.tys.const_usize(n)));
                self.tcx.tys.intern(TyKind::Ref(false, arr, crate::types::RegionId::ERASED))
            }
            // `c"..."`: &'static core::ffi::CStr (nul-terminated)
            LitKind::CStr => match self.lang_item(&["ffi", "CStr"]) {
                Some(d) => {
                    let t = self.tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
                    self.tcx.tys.intern(TyKind::Ref(false, t, crate::types::RegionId::ERASED))
                }
                None => self.tcx.tys.error,
            },
        }
    }

    fn check_binary(&mut self, e: ExprId, op: BinOp, a: ExprId, b: ExprId, expected: Option<TyId>, lo: u32) -> TyId {
        let bool_ = self.tcx.tys.bool_;
        match op {
            BinOp::And | BinOp::Or => {
                // both operands are coerced to bool (rustc op.rs check_overloaded_binop for
                // lazy ops: demand_coerce to bool), which also fixes a generic return
                // (`v.is_empty() && msg_send![..]`)
                let ta = self.check_expr(a, Some(bool_));
                self.coerce_or_unify(ta, bool_, Some(a), self.ast().expr(a).lo);
                // `let` chains bind in the left operand; keep those scopes for the right
                let tb = self.check_expr(b, Some(bool_));
                self.coerce_or_unify(tb, bool_, Some(b), self.ast().expr(b).lo);
                self.diverges = false;
                bool_
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let ta = self.check_expr(a, None);
                // non-primitive operands compare through PartialEq / PartialOrd (`v == w` on
                // Vecs calls Vec's eq, `s == "x"` with s: String uses PartialEq<&str>)
                let sa = self.shallow(ta);
                // (`&str` orders through str's PartialOrd: only its equality is built in)
                let ordering = !matches!(op, BinOp::Eq | BinOp::Ne);
                let prim_ref = match self.tcx.tys.kind(sa).clone() {
                    TyKind::Ref(_, i, _) => {
                        let i = self.shallow(i);
                        self.is_numeric(i) || matches!(self.tcx.tys.kind(i), TyKind::Bool | TyKind::Char | TyKind::FnPtr(..)) || (!ordering && matches!(self.tcx.tys.kind(i), TyKind::Str))
                    }
                    _ => false,
                };
                // a generic param or a rigid projection compares through its PartialEq /
                // PartialOrd bound too: only scalars are built in (rustc op.rs check_overloaded_binop,
                // by_ref_binop: both operands autoref'd). Typing it as a built-in comparison made the
                // capture analysis of `|kv| kv.0 == k` consume `k` (by value) in a generic body but
                // borrow it in an instance -- two closure layouts for one closure
                // unsized places compare through their impls too (`*l == *r` with l: &[T] is
                // `<[T] as PartialEq>::eq(&*l, &*r)`, rustc op.rs: only scalars are built in)
                // a type-variable left operand is an operator trait call too: rustc op.rs:41 takes the
                // builtin path only when neither operand is a type variable, and check_overloaded_binop
                // records the method for `?T: PartialEq<?R>` while it is ambiguous. Typing it as a
                // built-in comparison unified both sides and made the capture analysis of
                // `|p| *p == path` (p: &?T) consume `path` (a false E0382 in Scope's code_graph).
                if !prim_ref && matches!(self.tcx.tys.kind(sa), TyKind::Adt(..) | TyKind::Ref(..) | TyKind::Tuple(..) | TyKind::Array(..) | TyKind::Slice(_) | TyKind::Str | TyKind::Dyn(..) | TyKind::Param(_) | TyKind::Infer(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. })) {
                    if let Some(r) = self.check_cmp_trait(e, op, sa, b, lo, None) {
                        return r;
                    }
                }
                let tb = self.check_expr(b, Some(ta));
                // `&str != String`, `&u8 == u8`: not the built-in comparison of equal types but
                // `<&str as PartialEq<String>>` (rustc op.rs: an operator trait lookup with the
                // operand types as written)
                if prim_ref {
                    let (x, y) = (self.shallow(ta), self.shallow(tb));
                    if x != y && !matches!(self.tcx.tys.kind(y), TyKind::Infer(_)) {
                        let save = self.start_snapshot();
                        let same = self.eq(x, y);
                        self.rollback_to(save);
                        if !same {
                            if let Some(r) = self.check_cmp_trait(e, op, sa, b, lo, Some(tb)) {
                                return r;
                            }
                        }
                    }
                }
                // raw and fn pointers compare only through their `PartialEq for Self` impls, so
                // rustc's `L: PartialEq<?R>` selects ?R = L before the rhs is checked and the rhs
                // coerces to it (`fn_ptr == fn_item`, `raw == &x`, `*const T == *mut T`)
                let ptr_lhs = matches!(self.tcx.tys.kind(sa), TyKind::Ptr(..) | TyKind::FnPtr(..));
                if ptr_lhs {
                    let save = self.start_snapshot();
                    if self.eq(ta, tb) {
                        self.commit_from(save);
                    } else {
                        self.rollback_to(save);
                        self.coerce_or_unify(tb, ta, Some(b), self.ast().expr(b).lo);
                    }
                } else if !self.eq(ta, tb) {
                    let (sa, sb) = (self.ty_str(ta), self.ty_str(tb));
                    self.err(lo, format!("E0308: mismatched types: expected `{}`, found `{}`", sa, sb));
                }
                bool_
            }
            BinOp::Shl | BinOp::Shr => {
                let ta = self.check_expr(a, expected);
                // `&x >> k` on primitives: core's forward-ref Shl/Shr impls, Output = the
                // primitive (rustc op.rs: only primitive operands are built in); a non-primitive
                // left operand is an operator trait call
                let sa0 = self.shallow(ta);
                let ta = self.peel_prim_refs(sa0);
                let sa = self.shallow(ta);
                // parameters and aliases are operator trait calls (rustc op.rs is_builtin_binop:
                // only primitive operands are built in), as for `a op= b` below
                if matches!(self.tcx.tys.kind(sa), TyKind::Adt(..) | TyKind::Ref(..) | TyKind::Tuple(..) | TyKind::Array(..) | TyKind::Param(_) | TyKind::Alias(..)) {
                    self.cur_binop = e.0;
                    return self.check_op_trait(op, sa, b, lo, false);
                }
                self.check_expr(b, None);
                ta
            }
            _ => {
                let exp = match expected {
                    Some(x) => {
                        let s = self.shallow(x);
                        if self.is_numeric(s) {
                            Some(s)
                        } else {
                            None
                        }
                    }
                    None => None,
                };
                let ta = self.check_expr(a, exp);
                let sa0 = self.shallow(ta);
                // `&x op y` on primitives: built in, through the references (core's
                // forward-ref operator impls, without dispatch)
                let ta = self.peel_prim_refs(sa0);
                let sa0 = self.shallow(ta);
                // a parameter or alias left operand is an operator trait call even when an
                // instance is primitive (rustc op.rs is_builtin_binop): `p * 2.0` with
                // `P: Mul<f32, Output = P>` is `<P as Mul<f32>>::mul`, not `P == f32`
                // a type-variable left operand is an operator trait call too (rustc op.rs:41 takes
                // the builtin path only when neither operand is a type variable): `?S: Add<?R>`
                // stays ambiguous, it is not `?S == {integer}`
                let sa0 = if matches!(self.tcx.tys.kind(sa0), TyKind::Infer(_)) { self.retry_goals(false); self.shallow(sa0) } else { sa0 };
                if matches!(self.tcx.tys.kind(sa0), TyKind::Adt(..) | TyKind::Ref(..) | TyKind::Tuple(..) | TyKind::Array(..) | TyKind::Param(_) | TyKind::Alias(..) | TyKind::Infer(_)) {
                    self.cur_binop = e.0;
                    return self.check_op_trait(op, sa0, b, lo, false);
                }
                let tb0 = self.check_expr(b, Some(ta));
                let sb0 = self.shallow(tb0);
                // a primitive left operand with a non-primitive right one is an operator trait
                // call too (`3u32 * duration`: `impl Mul<Duration> for u32`; rustc is_builtin_binop
                // needs both sides primitive)
                if matches!(self.tcx.tys.kind(sb0), TyKind::Adt(..) | TyKind::Param(_) | TyKind::Alias(..) | TyKind::Infer(_)) {
                    let sa = self.shallow(ta);
                    if let Some(r) = self.prim_lhs_op(op, e, sa, sb0, lo) {
                        return r;
                    }
                }
                let tb = self.peel_prim_refs(tb0);
                let sa = self.shallow(ta);
                let sb = self.shallow(tb);
                // primitive arithmetic: both sides the same type
                if !self.eq(sa, sb) {
                    let (x, y) = (self.ty_str(sa), self.ty_str(sb));
                    // rustc: the operand's mismatch (E0308), then the unmet `X: Op<Y>` (E0277, core's
                    // on_unimplemented message for the operator)
                    self.err(lo, format!("E0308: mismatched types: expected `{}`, found `{}`", x, y));
                    self.err(lo, format!("E0277: {}", binop_unimplemented(op, &x, &y)));
                } else {
                    // rustc op.rs is_builtin_binop: arithmetic is built in for numbers, bit operators for
                    // integers and bool; any other primitive has no operator trait impl (E0369)
                    let k = self.tcx.tys.kind(sa).clone();
                    let arith = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem);
                    let bad = match k {
                        TyKind::Bool => arith,
                        TyKind::Float(_) | TyKind::FloatVar(_) => !arith,
                        TyKind::Char | TyKind::Str | TyKind::Ptr(..) | TyKind::FnPtr(..) => true,
                        _ => false,
                    };
                    if bad {
                        let x = self.ty_str(sa);
                        self.err(lo, format!("E0369: {}", binop_unimplemented(op, &x, &x)));
                    }
                }
                ta
            }
        }
    }

    /// The normalized <Iter as Iterator>::Item of the desugared next call.
    /// rustc_ast_lowering/src/expr.rs:1774-1784.
    fn iterator_item(&mut self, key: u32, t: TyId) -> Option<TyId> {
        use crate::lang_items::LangItem;
        let next = self.tcx.lang_items.get(LangItem::IteratorNext)?;
        let full = self.infer_item_args(next, &[t]);
        let sig = self.tcx.fn_sig(self.prog, next)?.clone();
        let (_, option) = self.instantiate_call_signature(next, &full, &sig);
        self.register_method_obligation(next, &full);
        self.apply_fn_bounds(next, &full, self.last_lo);
        let option = self.normalize_signature(option);
        let TyKind::Adt(def, args, regions) = self.tcx.tys.kind(option).clone() else { return None; };
        let some = self.tcx.lang_items.get(LangItem::OptionSome)?;
        let variant = self.tcx.adts.get(&def)?.variants.iter().find(|v| v.def == some)?;
        let item = self.tcx.instantiate_ty(self.prog, def, &self.tcx.complete_args(self.prog, def, self.tcx.tys.list(args), regions), variant.fields[0].ty);
        self.b.dependencies.push(next);
        self.b.for_next.insert(key, (next, self.tcx.tys.mk_list(&[t])));
        Some(item)
    }

    fn check_for_iter(&mut self, it: ExprId, lo: u32) -> TyId {
        let t = self.check_expr(it, None);
        if let Some(item) = self.for_into_iter_item(it.0, t) {
            return item;
        }
        let ts = self.ty_str(t);
        self.err(lo, format!("`for` over `{}`: no IntoIterator impl", ts));
        self.tcx.tys.error
    }

    fn check_field(&mut self, e: ExprId, x: ExprId, name: Ident, lo: u32) -> TyId {
        let t = self.check_expr(x, None);
        let s = self.sym(name);
        let resolved = self.deeply_resolve_ignoring_regions_with_obligations(t);
        let mut cur = self.shallow(resolved);
        let mut derefs = 0usize;
        loop {
            if matches!(self.tcx.tys.kind(cur), TyKind::Ptr(..)) { break; }
            if let TyKind::Adt(d, args, regions) = self.tcx.tys.kind(cur).clone() {
                if let Some(adt) = self.tcx.adts.get(&d) {
                    if !adt.is_enum {
                        for (i, f) in adt.variants[0].fields.iter().enumerate() {
                            // a private field of a type from elsewhere is not visible: autoderef
                            // continues (`rc.0` on Rc<Shared> is Shared's field)
                            if f.name == s && self.field_visible(d, f) {
                                self.b.field_idx.insert(e.0, (derefs as u32, i as u32));
                                self.set_operand_derefs(e.0, derefs);
                                if !self.b.field_uses.contains(&(d, i as u32)) {
                                    self.b.field_uses.push((d, i as u32));
                                    self.b.dependencies.push(d);
                                }
                                return self.instantiate_field_type(f.ty, adt.region_params, regions, self.tcx.tys.list(args));
                            }
                        }
                    }
                }
            }
            // references, raw pointers, then Deref impls (Box<T>, wrappers)
            match self.deref_step(e.0, derefs, cur) {
                Some(i) => {
                    cur = i;
                    derefs += 1;
                }
                None => break,
            }
        }
        let ts = self.ty_str(t);
        // expr.rs check_expr_field: a method of that name is E0615, a primitive has no fields (E0610)
        if !self.tcx.tys.has_error(cur) && matches!(self.pick_all_method(e, t, s, lo), Ok(Some(_))) {
            self.err(lo, format!("E0615: attempted to take value of method `{}` on type `{}`", self.text(name), ts));
        } else if matches!(self.tcx.tys.kind(self.shallow(t)), TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)) {
            self.err(lo, format!("E0610: `{}` is a primitive type and therefore doesn't have fields", ts));
        } else {
            self.err(lo, format!("E0609: no field `{}` on type `{}`", self.text(name), ts));
        }
        self.tcx.tys.error
    }

    /// E0624: an item of an inherent impl is private to the impl's module.
    fn check_assoc_privacy(&mut self, item: DefId, pos: u32) {
        if !self.tcx.validates(self.prog, self.file) { return; }
        if pos >= self.prog.files[self.file as usize].src_len { return; }
        let def = self.prog.def(item);
        if self.prog.def(def.parent).kind != DefKind::Impl || !self.prog.own_crates.get(def.krate as usize).copied().unwrap_or(false) {
            return;
        }
        let vis = self.prog.def_item_vis(item);
        if self.prog.vis_visible(vis, self.module) {
            return;
        }
        let what = match def.kind {
            DefKind::AssocFn if self.tcx.fn_sig(self.prog, item).map_or(false, |s| s.self_kind != 0) => "method",
            DefKind::AssocFn => "associated function",
            DefKind::AssocConst => "associated constant",
            _ => "associated item",
        };
        let msg = format!("E0624: {} `{}` is private", what, self.prog.syms.str(def.name));
        self.err(pos, msg);
    }

    /// E0451: a field named in a struct expression or pattern must be visible here.
    fn check_field_privacy(&mut self, adt_ty: TyId, vi: u32, fi: u32, pos: u32) {
        if !self.tcx.validates(self.prog, self.file) { return; }
        // (macro output is held to the macro's own privacy, not the call site's)
        if pos >= self.prog.files[self.file as usize].src_len { return; }
        let st = self.shallow(adt_ty);
        let TyKind::Adt(d, _, _) = self.tcx.tys.kind(st).clone() else { return };
        let Some(adt) = self.tcx.adts.get(&d) else { return };
        let Some(f) = adt.variants.get(vi as usize).and_then(|v| v.fields.get(fi as usize)) else { return };
        if adt.is_enum || self.field_visible(d, f) {
            return;
        }
        let what = match self.prog.def(d).kind { DefKind::Union => "union", _ => "struct" };
        let fname = if f.name & 0x8000_0000 != 0 { (f.name & 0x7fff_ffff).to_string() } else { self.prog.syms.str(f.name).to_string() };
        let msg = format!("E0451: field `{}` of {} `{}` is private", fname, what, self.prog.name(d));
        self.err(pos, msg);
    }

    /// Is a field of ADT `d` visible here: `pub`, or we are inside the ADT's module (or below it).
    fn field_visible(&self, d: DefId, f: &crate::tcx::FieldT) -> bool {
        // (`pub` / `pub(in path)`: inside that module only)
        if f.vis_mod != u32::MAX && f.is_pub {
            return self.prog.mod_within(self.module, f.vis_mod);
        }
        if f.is_pub {
            return true;
        }
        let target = self.prog.def(d).scope;
        let mut m = Some(self.module);
        let mut guard = 0;
        while let Some(cur) = m {
            if cur == target || self.prog.mods[cur as usize].normal == target {
                return true;
            }
            let md = &self.prog.mods[cur as usize];
            m = md.lexical_parent.or(md.parent_mod);
            guard += 1;
            if guard > 256 {
                break;
            }
        }
        false
    }

    fn check_call(&mut self, e: ExprId, f: ExprId, args: &[ExprId], expected: Option<TyId>, lo: u32) -> TyId {
        let legacy = self.prog.legacy_const_call(self.b.def, self.file, e).cloned();
        let ft = if let Some(split) = &legacy {
            let ty = self.value_def_ty(split.callee, None, f.0);
            self.b.res.insert(f.0, Res::Def(split.callee));
            self.set_ty(f, ty);
            ty
        } else { self.check_expr(f, None) };
        let mut fs = self.deeply_resolve_ignoring_regions_with_obligations(ft);
        // rustc_ast_lowering/src/expr.rs:594-679: collection already resolved
        // the callee and published the const/value argument split.
        let args = if let Some(split) = &legacy {
            if split.invalid {
                self.err(lo, "invalid legacy const generic argument".into());
                return self.tcx.tys.error;
            }
            if let TyKind::FnDef(d, gargs, _) = self.tcx.tys.kind(fs).clone() {
                let declaration = self.tcx.declaration_env(self.prog, d);
                let np = self.tcx.fn_item_sig(self.prog, d).map_or(0, |s| s.n_parent_generics) as usize;
                let env = self.env.clone();
                for (j, &a) in split.const_args.iter().enumerate() {
                    if let Some(&g) = self.tcx.tys.list(gargs).get(np + j) {
                        let expected = declaration.consts.iter().find(|c| c.0 as usize == np + j)
                            .map(|&(_, cf, ct)| self.tcx.lower_ty(&self.cx.with_infcx(&self.infcx), self.prog, cf, self.prog.def(d).scope, &declaration, ct))
                            .unwrap_or(self.tcx.tys.error);
                        let c = self.tcx.tys.intern(TyKind::Const(self.tcx.lower_const_arg(self.prog, self.file, &env, a, expected)));
                        self.eq(g, c);
                    }
                }
            }
            self.b.legacy_args.insert(e.0, split.positions.clone());
            split.value_args.as_slice()
        } else { args };
        // rustc_hir_typeck/src/callee.rs:113-120,253-366: probe at each
        // autoderef step, retaining the chosen trait in the generic body.
        let mut steps = 0usize;
        let mut overloaded = None;
        loop {
            fs = self.deeply_resolve_ignoring_regions_with_obligations(fs);
            match self.tcx.tys.kind(fs) {
                TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Closure(..) | TyKind::Error => break,
                TyKind::Ref(..) if steps == 0 => {},
                TyKind::Infer(var) if !self.infcx.has_opaques_with_sub_unified_hidden_type(*var) => {
                    self.err(lo, "type must be known at this call".into());
                    break;
                }
                _ => {
                    overloaded = self.try_overloaded_call_traits(fs, Some(args.len()))
                        .or_else(|| self.try_overloaded_call_traits(fs, None));
                    if overloaded.is_some() { break; }
                }
            }
            match self.deref_step(e.0, steps, fs) {
                Some(n) => { fs = n; steps += 1; }
                None => break,
            }
        }
        if steps > 0 { self.set_operand_derefs(e.0, steps); }
        if let Some((td, tuple, _, _)) = &overloaded {
            self.record_call_trait(e, *td, fs, steps, *tuple);
        }
        let error = self.tcx.tys.error;
        let mut variadic = false;
        let (params, ret) = match self.tcx.tys.kind(fs).clone() {
            TyKind::FnDef(d, gargs, regions) => match self.tcx.fn_item_sig(self.prog, d).cloned() {
                Some(sig) if sig.params.iter().any(|p| self.tcx.tys.has_error(*p)) || self.tcx.tys.has_error(sig.ret) => {
                    let n = self.prog.def_path(d);
                    self.err(lo, format!("`{}` has unresolved types in its signature -- fix its signature errors first", n));
                    for a in args {
                        self.check_expr(*a, None);
                    }
                    return error;
                }
                Some(sig) => {
                    let full = self.tcx.complete_args(self.prog, d, self.tcx.tys.list(gargs), regions);
                    let (ps, r) = self.instantiate_call_signature(d, &full, &sig);
                    variadic = sig.header.c_variadic;
                    (ps, r)
                }
                None => (Vec::new(), error),
            },
            TyKind::FnPtr(_, _, header, n) => {
                variadic = header.c_variadic;
                let vars: Vec<_> = (0..n).map(|_| self.fresh_region()).collect();
                let sig = self.instantiate_signature(fs, &vars);
                let TyKind::FnPtr(ps, r, _, _) = *self.tcx.tys.kind(sig) else { unreachable!() };
                (self.tcx.tys.list(ps).to_vec(), r)
            },
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => {
                let TyKind::FnPtr(_, _, _, n) = self.tcx.tys.kind(sig) else { unreachable!() };
                let vars: Vec<_> = (0..*n).map(|_| self.fresh_region()).collect();
                let sig = self.instantiate_signature(sig, &vars);
                let TyKind::FnPtr(ps, r, _, _) = *self.tcx.tys.kind(sig) else { unreachable!() };
                (self.tcx.tys.list(ps).to_vec(), r)
            },
            _ if overloaded.is_some() => { let (_, _, ps, ret) = overloaded.unwrap(); (ps, ret) },
            TyKind::Error => {
                for a in args {
                    self.check_expr(*a, None);
                }
                return error;
            }
            _ => {
                let s = self.ty_str(fs);
                self.err(lo, format!("E0618: expected function, found `{}`", s));
                return error;
            }
        };
        // callee.rs:254,1214-1278: deferring also known kinds preserves the
        // same signature equality and receiver adjustment after capture analysis.
        if matches!(self.tcx.tys.kind(fs), TyKind::Closure(..)) {
            let tuple = self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&params)));
            self.deferred_call_resolutions.push(call::DeferredCallResolution { call_expr: e, closure_ty: fs, derefs: steps, tuple, output: ret });
        }
        if params.len() != args.len() && !(variadic && args.len() >= params.len()) {
            let callee = match self.tcx.tys.kind(fs).clone() {
                TyKind::FnDef(d, _, _) => Some(d),
                _ => None,
            };
            let help = self.arg_count_help(callee, &params, args, 0);
            self.err(lo, format!("E0061: this function takes {} argument{} but {} {} supplied{}", params.len(), if params.len() == 1 { "" } else { "s" }, args.len(), if args.len() == 1 { "was" } else { "were" }, help));
        }
        for (&ty, &arg) in params.iter().zip(args) {
            self.register_wf(ty, self.ast().expr(arg).lo);
        }
        // An unsized expectation reaches a call only as rvalue_hint's ExpectRvalueLikeUnsized
        // (a call is never a place expression), whose only_has_type is None: no expected inputs
        // (expectation.rs:76-88, 113-118; fn_ctxt/checks.rs check_argument_types)
        let expected = expected.filter(|x| !self.is_unsized_ty(*x));
        let exp_inputs = expected.and_then(|x| self.expected_inputs_for_expected_output(ret, x, &params));
        // rustc check_argument_types: non-closures first, then select their obligations
        // before deriving closure signatures from the call's Fn bounds.
        for check_closures in [false, true] {
            if check_closures {
                self.select_before_closures();
            }
            for (i, a) in args.iter().enumerate() {
                if matches!(self.ast().expr(*a).kind, ExprKind::Closure { .. }) != check_closures {
                    continue;
                }
                if variadic && i >= params.len() {
                    let t = self.check_expr(*a, None);
                    // check_argument_types: C promotes these before the call (E0617)
                    if self.tcx.validates(self.prog, self.file) {
                        let t = self.shallow(t);
                        let lo = self.ast().expr(*a).lo;
                        let small = |p: Prim| matches!(p, Prim::I8 | Prim::I16 | Prim::U8 | Prim::U16);
                        match self.tcx.tys.kind(t).clone() {
                            TyKind::Float(Prim::F32) => self.err(lo, "E0617: can't pass `f32` to variadic function".to_string()),
                            TyKind::Int(p) if small(p) => self.err(lo, format!("E0617: can't pass `{}` to variadic function", crate::typeck::ty_to_string(false, self.prog, self.tcx, t))),
                            TyKind::Bool => self.err(lo, "E0617: can't pass `bool` to variadic function".to_string()),
                            TyKind::FnDef(..) => self.err(lo, "E0617: can't pass a function item to a variadic function".to_string()),
                            _ => {}
                        }
                    }
                    continue;
                }
                let pt = if i < params.len() { self.normalize_signature(params[i]) } else { error };
                if let Some(ei) = exp_inputs.as_ref().and_then(|v| v.get(i).copied().flatten()) {
                    // rvalue_hint: an unsized expected input is only a hint (closure signatures),
                    // and the argument coerces to the formal type; a sized one is coerced to, then
                    // the formal type must equal it (checks.rs demand_compatible)
                    let unsized_hint = self.is_unsized_ty(ei);
                    let t = self.check_expr(*a, Some(ei));
                    let ei = self.deeply_resolve_ignoring_regions_with_obligations(ei);
                    let pt = self.deeply_resolve_ignoring_regions_with_obligations(pt);
                    if unsized_hint {
                        self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
                    } else {
                        self.coerce_or_unify(t, ei, Some(*a), self.ast().expr(*a).lo);
                        self.eq(pt, ei);
                    }
                    continue;
                }
                let t = self.check_expr(*a, Some(pt));
                let pt = self.deeply_resolve_ignoring_regions_with_obligations(pt);
                self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
            }
        }
        let _ = (e, lo);
        self.normalize_signature(ret)
    }

    /// `async [move] { .. }`: a coroutine -- a closure type with no parameters whose state
    /// holds the captures and a frame word, and which implements Future<Output = the block's
    /// type> (rustc lowers async blocks to coroutines; captures as for closures).
    fn check_async_block(&mut self, e: ExprId, mode: u8, b: BlockId) -> TyId {
        let is_move = mode == crate::ast::ASYNC_MOVE;
        let _ = self.tcx.future_defs(self.prog);
        let saved = self.scopes.len();
        let expected_out = if mode == crate::ast::ASYNC_CLOSURE_BODY {
            self.closure_depth.last().and_then(|(parent, _)| self.async_body_expected.get(parent).copied())
        } else { None };
        self.closure_depth.push((e.0, saved));
        let out = expected_out.unwrap_or_else(|| self.fresh(0));
        self.ret_stack.push(out);
        self.async_depth += 1;
        let save_awaits = std::mem::take(&mut self.await_count);
        let save_loops = std::mem::take(&mut self.loops);
        let save_condition = std::mem::replace(&mut self.in_loop_condition, false);
        let bt = self.check_block(b, Some(out));
        self.loops = save_loops;
        self.in_loop_condition = save_condition;
        let points = std::mem::replace(&mut self.await_count, save_awaits);
        if !self.diverges {
            self.coerce_or_unify(bt, out, None, self.ast().block(b).hi);
        } else {
            // a body that only diverges (`async { todo!() }`): the return type's still-open
            // variable falls back like any variable a `!` flowed into
            let so = self.shallow(out);
            if matches!(self.tcx.tys.kind(so), TyKind::Infer(_)) {
                self.diverging.push(so);
            }
        }
        self.async_depth -= 1;
        self.ret_stack.pop();
        self.closure_depth.pop();
        self.scopes.truncate(saved);
        self.diverges = false;
        let mut force_args = Vec::new();
        let mut force_move_if_consumes = false;
        let mut is_move = is_move;
        if mode == crate::ast::ASYNC_CLOSURE_BODY {
            if let Some(&(parent, _)) = self.closure_depth.last() {
                if let ExprKind::Closure(prow) = self.ast().expr(ExprId(parent)).kind {
                    let parent_move = self.ast().row(prow).is_move;
                    force_move_if_consumes = parent_move;
                    is_move |= parent_move && self.expected_closure_kinds.get(&parent) == Some(&crate::upvar::ClosureKind::FnOnce);
                }
                // Parameter bindings are explicitly transferred to the coroutine,
                // including unused / Copy arguments (rustc upvar.rs:286-321).
                force_args = self.scopes[self.closure_depth.last().unwrap().1..].iter().map(|p| p.1).collect();
            }
        }
        let caps = self.b.closure_locals.get_or_insert_with(e.0, Vec::new);
        for &li in &force_args { if !caps.contains(&li) { caps.push(li); } }
        self.coroutine_type(e.0, b, force_args, is_move, force_move_if_consumes, out, points)
    }

    /// rustc's `defining_opaque_types_and_generators` holds the body's coroutines: goals about them wait for
    /// the body's MIR (`try_stall_coroutine`). Relax's coroutines have no definitions of their own, so the
    /// body's own definition stands for them, added when the body first has one.
    fn stall_coroutines_of_body(&mut self) {
        let crate::tcx::TypingMode::Typeck { defining_opaque_types_and_generators: defining } = self.infcx.typing_mode.get() else { return };
        if self.tcx.defining_opaques(defining).contains(&self.b.def) { return; }
        let mut owners = self.tcx.defining_opaques(defining).to_vec();
        owners.push(self.b.def);
        let mode = crate::tcx::TypingMode::Typeck { defining_opaque_types_and_generators: self.tcx.intern_defining_opaques(owners) };
        self.infcx.typing_mode.set(mode);
        self.cx.typing_env.mode = mode;
    }

    /// Leave the environment tuple open until closure_analyze has processed the body.
    fn coroutine_type(&mut self, ce: u32, block: BlockId, force_args: Vec<u32>, is_move: bool, force_move_if_consumes: bool, out: TyId, points: u32) -> TyId {
        self.b.closures.push(ce);
        let up = self.fresh(0);
        let kind = self.fresh(0);
        self.pending_captures.push(crate::upvar::PendingCapture {
            closure: ce, upvars: up, kind, source: crate::upvar::CaptureSource::Block(block),
            is_move, force_args, force_move_if_consumes, points: Some(points),
        });
        self.tcx.coroutines.insert((self.file, ce), ());
        self.stall_coroutines_of_body();
        let sig = self.tcx.tys.intern(TyKind::FnPtr(crate::types::List::EMPTY, out, FnHeader::RUST, 0));
        let owner = self.tcx.tys.intern(TyKind::FnDef(self.b.def, self.tcx.tys.mk_list(&self.b.args), self.env.regions.identity_args(&self.tcx.tys)));
        self.tcx.tys.intern(TyKind::Closure(self.file, ce, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: kind, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }))
    }

    /// `x.await`: `IntoFuture::into_future(x)` polled to completion; its type is
    /// `<X as IntoFuture>::Output` (rustc lowering of `.await`, Reference await-expr.md).
    fn check_await(&mut self, x: ExprId, lo: u32) -> TyId {
        let t = self.check_expr(x, None);
        if self.async_depth == 0 {
            self.err(lo, "`await` is only allowed inside `async` functions and blocks (E0728)".to_string());
            return self.tcx.tys.error;
        }
        self.await_count += 1;
        if let Some(&(coroutine, _)) = self.closure_depth.last() { self.b.awaits.push((coroutine, t, lo)); }
        // an async block / async fn body: its output is the coroutine's return type, known
        // without selection (rustc: the coroutine's Future::Output, so `A[async { 1 }.await]`
        // keeps the literal open for the index)
        let st = self.shallow(t);
        if let TyKind::Closure(f, ce, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) = self.tcx.tys.kind(st).clone() {
            if self.tcx.coroutines.contains(&(f, ce)) {
                if let TyKind::FnPtr(_, out, _, _) = self.tcx.tys.kind(sig).clone() {
                    return out;
                }
            }
        }
        let ft = self.infcx.deeply_resolve_ignoring_regions(t);
        let ad = match self.lang_item(&["future", "IntoFuture"]) {
            Some(td) => {
                let sym = self.prog.syms.get("Output").unwrap_or(u32::MAX);
                self.tcx.trait_assoc(self.prog, td, sym)
            }
            None => None,
        };
        let ad = match ad {
            Some(a) => a,
            None => {
                self.err(lo, "core::future::IntoFuture not found".to_string());
                return self.tcx.tys.error;
            }
        };
        let proj = self.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: self.tcx.tys.mk_list(&[ft]), regions: crate::types::RegionArgsId::EMPTY }));
        let n = self.norm(proj);
        // rustc_ast_lowering/src/expr.rs:1179-1197 lowers await through
        // IntoFuture::into_future. Its output may remain a rigid projection in
        // a generic body; the trait obligation, not alias shape, proves validity.
        let td = self.prog.def(ad).parent;
        let goal = self.trait_goal(td, ft, &[]);
        self.register_goal(goal, lo);
        n
    }

    fn check_closure(&mut self, e: ExprId, params: &[ClosureParam], ret: Option<crate::ast::TyId>, body: ExprId, expected: Option<TyId>) -> TyId {
        // rustc_hir_typeck::closure::deduce_closure_signature: retain the kind
        // alongside the signature. FnPtr is also used for signature-only hints
        // from generic Fn bounds here, so it cannot supply a closure kind.
        // Borrowck must check a dyn Fn closure as Fn
        // even when moving a capture would otherwise infer FnOnce.
        if let Some(expected) = expected {
            let expected = self.shallow(expected);
            let kind = match self.tcx.tys.kind(expected) {
                TyKind::Dyn(td, ..) if self.lang_item(&["ops", "Fn"]) == Some(*td) => Some(ExpectedClosureKind::Fn),
                TyKind::Dyn(td, ..) if self.lang_item(&["ops", "FnMut"]) == Some(*td) => Some(ExpectedClosureKind::FnMut),
                TyKind::Dyn(td, ..) if self.lang_item(&["ops", "FnOnce"]) == Some(*td) => Some(ExpectedClosureKind::FnOnce),
                _ => None,
            };
            if let Some(kind) = kind {
                self.b.closure_expected_kinds.insert(e.0, kind);
            }
        }
        let sigs = self.closure_signatures(e, params, ret, expected);
        let (ps, rt) = match self.tcx.tys.kind(sigs.liberated).clone() {
            TyKind::FnPtr(ps, r, _, _) => (ps, r),
            _ => unreachable!(),
        };
        self.b.closure_sigs.insert(e.0, sigs);
        let saved = self.scopes.len();
        self.closure_depth.push((e.0, saved));
        let save_async = std::mem::take(&mut self.async_depth);
        let save_awaits = std::mem::take(&mut self.await_count);
        if self.tcx.validates(self.prog, self.file) {
            let closure_pats: Vec<PatId> = params.iter().map(|p| p.pat).collect();
            self.check_param_names(&closure_pats);
        }
        for (p, t) in params.iter().zip(self.tcx.tys.list(ps).iter().copied()) {
            self.check_pat(p.pat, t);
        }
        self.ret_stack.push(rt);
        let save_loops = std::mem::take(&mut self.loops);
        let save_condition = std::mem::replace(&mut self.in_loop_condition, false);
        let bt = self.check_expr(body, Some(rt));
        self.loops = save_loops;
        self.in_loop_condition = save_condition;
        self.coerce_or_unify(bt, rt, Some(body), self.ast().expr(body).lo);
        // a coerced body keeps its own type: lowering converts it to the return type
        if !self.b.coerce(body.0).is_some() {
            self.set_ty(body, rt);
        }
        self.ret_stack.pop();
        self.closure_depth.pop();
        self.async_depth = save_async;
        self.await_count = save_awaits;
        self.scopes.truncate(saved);
        self.diverges = false;
        self.b.closures.push(e.0);
        let sig = self.async_closure_bound_signature(e);
        let is_move = matches!(self.ast().expr(e).kind, ExprKind::Closure(row) if self.ast().row(row).is_move);
        let up = self.fresh(0);
        let kind = match self.expected_closure_kinds.get(&e.0).copied() {
            Some(kind) => self.tcx.tys.from_closure_kind(kind),
            None => self.fresh(0),
        };
        self.pending_captures.push(crate::upvar::PendingCapture {
            closure: e.0, upvars: up, kind, source: crate::upvar::CaptureSource::Expr(body),
            is_move, force_args: Vec::new(), force_move_if_consumes: false, points: None,
        });
        let owner = self.tcx.tys.intern(TyKind::FnDef(self.b.def, self.tcx.tys.mk_list(&self.b.args), self.env.regions.identity_args(&self.tcx.tys)));
        self.tcx.tys.intern(TyKind::Closure(self.file, e.0, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: kind, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }))
    }

    /// The deferred `Copy` checks of `[x; N]` (rustc fn_ctxt/checks.rs check_repeat_exprs,
    /// run after the body and before fallback): a const item / const block operand needs none;
    /// the count must be resolved by now (all counts first, so a `Copy` side effect cannot
    /// make another repeat's count known); `Copy` is required unless the count is 0 or 1.
    fn check_repeat_exprs(&mut self) {
        let mut todo = Vec::new();
        for (x, et, arr) in std::mem::take(&mut self.deferred_repeats) {
            let mut y = x;
            while let ExprKind::Paren(i) = self.ast().expr(y).kind.clone() {
                y = i;
            }
            if matches!(self.ast().expr(y).kind, ExprKind::ConstBlock(..)) {
                continue;
            }
            if matches!(self.b.res.get(&y.0), Some(Res::Def(d)) if matches!(self.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst)) {
                continue;
            }
            let arr = self.infcx.deeply_resolve_ignoring_regions(arr);
            let n = match self.tcx.tys.kind(arr).clone() {
                TyKind::Array(_, n) => n,
                _ => continue,
            };
            // fn_ctxt/checks.rs:111-125 + _impl.rs:1510-1597 structurally_resolve_const: the
            // normalized count with obligations; still inferred -> E0282; erroneous -> skipped
            let count = self.tcx.tys.intern(TyKind::Const(n));
            let count = self.normalize_body_type(count);
            let count = self.deeply_resolve_ignoring_regions_with_obligations(count);
            let count = self.tcx.tys.expect_const(count);
            if matches!(self.tcx.tys.const_kind(count), crate::consts::ConstKind::Infer(_)) {
                if self.b.errors.is_empty() { self.err(self.ast().expr(x).lo, "E0282: type annotations needed".into()); }
                continue;
            }
            if matches!(self.tcx.tys.const_kind(count), crate::consts::ConstKind::Error) || self.tcx.tys.has_error(self.tcx.tys.intern(TyKind::Const(count))) { continue; }
            todo.push((et, count));
        }
        let Some(td) = self.lang_item(&["marker", "Copy"]) else { return };
        // checks.rs:164-185: a value count of 0 or 1 copies nothing; a parameter or an
        // unevaluated (rigid) count may be > 1 after monomorphization
        for (et, count) in todo {
            let enforce = match self.tcx.tys.const_kind(count) {
                crate::consts::ConstKind::Value(_) => count.try_to_target_usize(&self.tcx.tys).map_or(true, |c| c > 1),
                crate::consts::ConstKind::Param(_) | crate::consts::ConstKind::Placeholder(_) | crate::consts::ConstKind::Alias(..) => true,
                k => unreachable!("repeat count {:?}", k),
            };
            if enforce {
                let goal = self.trait_goal(td, et, &[]);
                self.register_goal(goal, self.last_lo);
            }
        }
    }

    fn check_method(&mut self, e: ExprId, recv: ExprId, name: Ident, turbofish: GenericArgsId, args: &[ExprId], lo: u32) -> TyId {
        // this call's expectation only (nested calls in the receiver set their own)
        let method_expected = self.method_expected.take();
        let s = self.sym(name);
        self.check_method_sym(e, recv, s, turbofish, args, None, lo, method_expected)
    }

    /// A method call by name; `extra` is the type of one more argument after `args` that has no
    /// expression (built-in macros: `write!(w, ..)` is `w.write_fmt(format_args!(..))`).
    fn check_method_sym(&mut self, e: ExprId, recv: ExprId, s: Sym, turbofish: GenericArgsId, args: &[ExprId], extra: Option<TyId>, lo: u32, method_expected: Option<TyId>) -> TyId {
        self.extra_arg = extra;
        let rt = self.check_expr(recv, None);
        let error = self.tcx.tys.error;
        self.retry_goals(false);
        let pick = match self.pick_all_method(e, rt, s, lo) {
            Ok(Some(pick)) => pick,
            Ok(None) => {
                let ts = self.ty_str(rt);
                let help = self.method_help(rt, s);
                self.err(lo, format!("E0599: no method named `{}` found for `{}` in the current scope{}", self.prog.syms.str(s), ts, help));
                for a in args { self.check_expr(*a, None); }
                return error;
            }
            Err(()) => { for a in args { self.check_expr(*a, None); } return error; }
        };
        let (item, imp, self_ty, derefs) = (pick.item, pick.imp, pick.self_ty, pick.autoderefs);
        self.check_legal_trait_for_method_call(item, lo);
        self.forced_autoref = Some(pick.autoref);
        if imp == NO_DEF {
            self.known_method_args = Some(pick.args);
            return self.check_trait_method_call(e, item, imp, self_ty, derefs, turbofish, args, lo, method_expected);
        }
        let sig = self.tcx.fn_sig(self.prog, item).cloned().unwrap();
        let indices = self.tcx.generic_indices_of(self.prog, imp);
        let impl_full = pick.args[..indices.types_consts.len() + indices.lifetimes.len()].to_vec();
        let mut gargs = Vec::new();
        for arg in &impl_full { match *arg { crate::consts::GenericArg::Type(t) => gargs.push(t), crate::consts::GenericArg::Const(c) => gargs.push(self.tcx.tys.intern(TyKind::Const(c))), _ => {} } }
        let own = sig.n_generics - sig.n_parent_generics;
        let mut tf = Vec::new();
        let turbofish_ok = self.check_seg_arg_count(turbofish, item, lo, true);
        if let (true, Some(GenericArgs::Angle(v))) = (turbofish_ok, self.ast().gargs(turbofish)) {
            for g in self.ast().list(*v) {
                if let Some(lt) = self.generic_arg(g, item, self.tcx.fn_sig(self.prog, item).unwrap().n_parent_generics as usize + tf.len()) {
                    tf.push(lt);
                }
            }
        }
        let item_indices = self.tcx.generic_indices_of(self.prog, item);
        for i in 0..own as usize {
            let arg = pick.args[item_indices.types_consts[gargs.len()] as usize];
            let ty = match arg { crate::consts::GenericArg::Type(t) => t, crate::consts::GenericArg::Const(c) => self.tcx.tys.intern(TyKind::Const(c)), _ => unreachable!() };
            if let Some(&explicit) = tf.get(i) { self.eq(ty, explicit); }
            gargs.push(ty);
        }
        let autoref = self.forced_autoref.take().unwrap_or(match sig.self_kind { 2 => 1, 3 => 2, _ => 0 });
        self.b.dependencies.push(item);
        self.b.methods.insert(e.0, MethodRes { def: item, derefs: derefs as u32, autoref, args: self.tcx.tys.mk_list(&gargs) });
        if sig.self_kind == 0 {
            self.err(lo, format!("`{}` is an associated function, not a method", self.prog.syms.str(s)));
            return error;
        }
        // method/confirm.rs:357-372 (fresh_receiver_args): the impl's args are the
        // ones its self type was related with, lifetimes included; the method's own
        // lifetimes are fresh.
        let mut full = self.infer_item_args(item, &gargs);
        full[..impl_full.len()].copy_from_slice(&impl_full);
        self.write_method_user_type(e.0, item, &full);
        let (ps, ret) = self.instantiate_call_signature(item, &full, &sig);
        // method/confirm.rs:150-160 (add_obligations): registered once with the
        // instantiated args, before the arguments are checked.
        self.apply_fn_bounds(item, &full, lo);
        let extra = self.extra_arg.take();
        let n_extra = extra.is_some() as usize;
        if ps.len() != args.len() + 1 + n_extra {
            let want = ps.len().saturating_sub(1);
            let help = if n_extra == 0 { let rest: Vec<TyId> = ps.iter().skip(1).copied().collect(); self.arg_count_help(Some(item), &rest, args, 0) } else { String::new() };
            self.err(lo, format!("E0061: this method takes {} argument{} but {} {} supplied{}", want, if want == 1 { "" } else { "s" }, args.len() + n_extra, if args.len() + n_extra == 1 { "was" } else { "were" }, help));
        }
        if let (Some(x), Some(&pt)) = (extra, ps.get(args.len() + 1)) {
            let pt = self.norm(pt);
            self.coerce_or_unify(x, pt, None, lo);
        }
        // rustc expected_inputs_for_expected_output, as for calls: the expected result type
        // (`let _: Option<fn()> = Foo.foo(main)`) gives the arguments' expected types; a sized
        // one is coerced to, then the formal type must equal it
        // Per input (rustc keeps the resolved formal inputs as expectations; an input that still
        // holds the callee's own variables is no hint): `opt.map_or(&[], |x| x.list())` into
        // `&[String]` checks `&[]` against `&[String]`, then unsizes `&[String; 0]`. The formal
        // output may be the method's own parameter (`U` of map_or) when the expectation is a
        // known type.
        let exp_inputs = self.method_expected_inputs(ret, &ps, method_expected);
        self.check_method_args(item, &ps, args, exp_inputs);
        // projections in the result normalize now that the call fixed the args (rustc
        // normalizes a method's output: `opt.as_deref()` is Option<&str>, not
        // Option<&<String as Deref>::Target>, so a pattern binding gets `&str`)
        self.normalize_signature(ret)
    }

    /// `a op b` on a non-primitive left operand: core::ops trait call, result `Output`.
    /// `a == b` / `a < b` on non-primitives: `PartialEq::eq(&a, &b)` / `PartialOrd::lt(..)`.
    /// None when no impl exists (tuples/arrays of primitives compare structurally).
    fn check_cmp_trait(&mut self, e: ExprId, op: BinOp, ta: TyId, b: ExprId, lo: u32, checked_b: Option<TyId>) -> Option<TyId> {
        let (tr, m) = match op {
            BinOp::Eq => ("PartialEq", "eq"),
            BinOp::Ne => ("PartialEq", "ne"),
            BinOp::Lt => ("PartialOrd", "lt"),
            BinOp::Le => ("PartialOrd", "le"),
            BinOp::Gt => ("PartialOrd", "gt"),
            _ => ("PartialOrd", "ge"),
        };
        let td = self.lang_item(&["cmp", tr])?;
        let item = self.trait_fn(td, m)?;
        // integer/float literal variables stay open while the impl is selected (rustc falls
        // back to i32/f64 only at the end of type checking; a where-clause of the selected impl
        // may still fix them)
        let save_keep = self.keep_literal_vars;
        self.keep_literal_vars = true;
        let da = self.infcx.deeply_resolve_ignoring_regions(ta);
        // the right operand's expected type: the single impl's Rhs, else Self
        // (rustc checks the right operand against a fresh variable: the left type is only a
        // hint here, and none when it names a generic param -- `x == &Unicode(y.as_ref())`
        // with x: &Unicode<S1> compares against Unicode<&str> through
        // `impl<S1, S2> PartialEq<Unicode<S2>> for Unicode<S1>`)
        // In an instance the left type is concrete but still no more than that: only an impl
        // selection that fixes `L: Cmp<?R>` gives the right operand a type (`Option<u64> ==
        // Some(10)`, one impl); several impls (`&A: PartialEq<&B>` / `<&mut B>`, String's) leave
        // it open, as rustc's fresh variable does -- `x == &Unicode(y.as_ref())` with x:
        // &Unicode<CowStr> is `Unicode<&str>`, not `Unicode<CowStr>` (op.rs check_overloaded_binop)
        let hint = if self.tcx.has_param(da) {
            None
        } else {
            let rhs = self.fresh(0);
            let mark = self.start_snapshot();
            let hint = match self.select_trait(td, da, &[rhs]) {
                Ok(Some(_)) => {
                    let h = self.infcx.deeply_resolve_ignoring_regions(rhs);
                    if self.tcx.tys.has_infer(h) { None } else { Some(h) }
                }
                Ok(None) => None,
                Err(_) => Some(ta),
            };
            self.rollback_to(mark);
            hint
        };
        let tb = match checked_b {
            Some(t) => t,
            None => self.check_expr(b, hint),
        };
        let db = self.infcx.deeply_resolve_ignoring_regions(tb);
        self.keep_literal_vars = save_keep;
        let selection = self.select_trait(td, da, &[db]);
        let rhs = if selection.is_ok() {
            db
        } else if !matches!(self.select_trait(td, da, &[da]), Err(_)) {
            self.coerce_or_unify(tb, ta, Some(b), lo);
            da
        } else {
            let (x, y) = (self.ty_str(da), self.ty_str(db));
            self.err(lo, format!("no `{}` impl to compare `{}` with `{}` -- derive or impl {}", tr, x, y, tr));
            return Some(self.tcx.tys.bool_);
        };
        self.b.binops.insert(e.0, MethodRes { def: item, derefs: 0, autoref: 1, args: self.tcx.tys.mk_list(&[da, rhs]) });
        Some(self.tcx.tys.bool_)
    }

    /// `a op b` (or `a op= b` with `assign`) through core::ops::<Op>[Assign]
    /// `!x` / `-x` on a type with a `Not` / `Neg` impl: the trait call,
    /// recorded like a binary operator (`binops`, one argument).
    fn check_unop_trait(&mut self, e: ExprId, t: TyId, tr: &str, m: &str) -> Option<TyId> {
        let td = self.lang_item(&["ops", tr])?;
        match self.select_trait(td, t, &[]) { Ok(Some(_)) | Ok(None) => {}, Err(_) => return None };
        let ms = self.prog.syms.get(m)?;
        let item = self.prog.traits[self.prog.def(td).sub as usize].iter().copied().find(|&it| self.prog.def(it).name == ms)?;
        let args = vec![t];
        self.b.binops.insert(e.0, MethodRes { def: item, derefs: 0, autoref: 0, args: self.tcx.tys.mk_list(&args) });
        let sig = self.tcx.fn_sig(self.prog, item).cloned()?;
        let r = self.tcx.tys.subst(sig.ret, &args);
        Some(self.norm(r))
    }

    fn check_op_trait(&mut self, op: BinOp, ta: TyId, b: ExprId, lo: u32, assign: bool) -> TyId {
        let (tr0, m0) = match op {
            BinOp::Add => ("Add", "add"),
            BinOp::Sub => ("Sub", "sub"),
            BinOp::Mul => ("Mul", "mul"),
            BinOp::Div => ("Div", "div"),
            BinOp::Rem => ("Rem", "rem"),
            BinOp::BitAnd => ("BitAnd", "bitand"),
            BinOp::BitOr => ("BitOr", "bitor"),
            BinOp::BitXor => ("BitXor", "bitxor"),
            BinOp::Shl => ("Shl", "shl"),
            BinOp::Shr => ("Shr", "shr"),
            _ => ("", ""),
        };
        let (trs, ms_) = if assign { (format!("{}Assign", tr0), format!("{}_assign", m0)) } else { (tr0.to_string(), m0.to_string()) };
        let (tr, m) = (trs.as_str(), ms_.as_str());
        let error = self.tcx.tys.error;
        let me = self.cur_binop;
        let td = match self.lang_item(&["ops", tr]) {
            Some(d) => d,
            None => {
                self.err(lo, format!("core::ops::{} not found", tr));
                return error;
            }
        };
        let rhs = self.infcx.next_ty_var();
        let goal = self.trait_goal(td, ta, &[rhs]);
        let exp_b = self.infcx.fudge_inference_if_ok(|| {
            let mut ocx = crate::next_solver::fulfill::ObligationCtxt::new(&self.infcx, self.prog);
            ocx.register_obligation(goal);
            ocx.evaluate_obligations_error_on_ambiguity()?;
            let hint = self.infcx.deeply_resolve_ignoring_regions(rhs);
            if self.tcx.tys.has_infer(hint) { Err(crate::next_solver::NoSolution) } else { Ok(hint) }
        }).ok();
        let mut tb = self.check_expr(b, exp_b);
        if let Some(x) = exp_b {
            // the right operand is a coercion site for the impl's Rhs (`s + &string`: &String
            // -> &str); the operator's argument is the coerced value
            if self.coerce_or_unify(tb, x, Some(b), lo) {
                tb = x;
            }
        }
        self.op_trait_call(td, m, me, ta, tb, lo)
    }

    /// The operator trait call `<ta as Op<tb>>::m` (operands already checked).
    fn op_trait_call(&mut self, td: DefId, m: &str, me: u32, ta: TyId, tb: TyId, lo: u32) -> TyId {
        let error = self.tcx.tys.error;
        let tr = self.prog.name(td).to_string();
        let tb = self.infcx.deeply_resolve_ignoring_regions(tb);
        let ms = self.prog.syms.get(m).unwrap_or(u32::MAX);
        let mut item = None;
        for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
            if self.prog.def(it).name == ms {
                item = Some(it);
            }
        }
        let item = match item {
            Some(i) => i,
            None => return error,
        };
        let ta = self.infcx.deeply_resolve_ignoring_regions(ta);
        if matches!(self.select_trait(td, ta, &[tb]), Err(_)) {
            let (x, y) = (self.ty_str(ta), self.ty_str(tb));
            self.err(lo, op_trait_unimplemented(&tr, &x, &y));
            return error;
        }
        let args = vec![ta, tb];
        self.b.binops.insert(me, MethodRes { def: item, derefs: 0, autoref: 0, args: self.tcx.tys.mk_list(&args) });
        let sig = self.tcx.fn_sig(self.prog, item).cloned().unwrap();
        let r = self.tcx.tys.subst(sig.ret, &args);
        self.norm(r)
    }

    /// `prim op adt`: the operator trait impl of the primitive for the right type. A literal
    /// left operand takes the type of the single primitive impl with that right type.
    fn prim_lhs_op(&mut self, op: BinOp, e: ExprId, sa: TyId, sb: TyId, lo: u32) -> Option<TyId> {
        let (tr, m) = match op {
            BinOp::Add => ("Add", "add"),
            BinOp::Sub => ("Sub", "sub"),
            BinOp::Mul => ("Mul", "mul"),
            BinOp::Div => ("Div", "div"),
            BinOp::Rem => ("Rem", "rem"),
            BinOp::BitAnd => ("BitAnd", "bitand"),
            BinOp::BitOr => ("BitOr", "bitor"),
            BinOp::BitXor => ("BitXor", "bitxor"),
            _ => return None,
        };
        let td = self.lang_item(&["ops", tr])?;
        let db = self.infcx.deeply_resolve_ignoring_regions(sb);
        if !self.is_numeric(sa) && !matches!(self.tcx.tys.kind(sa), TyKind::Infer(_)) { return None; }
        if matches!(self.select_trait(td, sa, &[db]), Err(_)) { return None; }
        Some(self.op_trait_call(td, m, e.0, sa, db, lo))
    }

    #[allow(clippy::too_many_arguments)]
    /// rustc expected_inputs_for_expected_output for a method call: per input, the type the
    /// expected result gives it (None: no hint). See check_method_sym.
    /// An unsized expectation is rvalue_hint's ExpectRvalueLikeUnsized: no hint (as for calls).
    fn method_expected_inputs(&mut self, ret: TyId, ps: &[TyId], expected: Option<TyId>) -> Option<Vec<Option<TyId>>> {
        let expected = expected.filter(|x| !self.is_unsized_ty(*x));
        expected.and_then(|x| self.expected_inputs_for_expected_output(ret, x, &ps[1..]))
    }

    /// The arguments of a method call (receiver excluded), checked against their expected
    /// inputs when the expected result gave them, else against the formal types.
    fn check_method_args(&mut self, _item: DefId, ps: &[TyId], args: &[ExprId], exp_inputs: Option<Vec<Option<TyId>>>) {
        for (&ty, &arg) in ps[1..].iter().zip(args) {
            self.register_wf(ty, self.ast().expr(arg).lo);
        }
        let error = self.tcx.tys.error;
        // rustc check_argument_types: non-closures first, then select their obligations
        // before deriving closure signatures from the call's Fn bounds.
        for check_closures in [false, true] {
            if check_closures {
                self.select_before_closures();
            }
            for (i, a) in args.iter().enumerate() {
                if matches!(self.ast().expr(*a).kind, ExprKind::Closure { .. }) != check_closures {
                    continue;
                }
                let pt = if i + 1 < ps.len() { ps[i + 1] } else { error };
                if let Some(ei) = exp_inputs.as_ref().and_then(|v| v.get(i).copied().flatten()) {
                    // a closure takes its signature from the registered bound unless the
                    // expected input is a fn pointer: then from that, and it coerces to it
                    // (`.filter(|(_, c)| ..)` into `Filter<_, fn(&(usize, &str)) -> bool>`)
                    let ei = self.deeply_resolve_ignoring_regions_with_obligations(ei);
                    let to_fn_ptr = matches!(self.tcx.tys.kind(self.shallow(ei)), TyKind::FnPtr(..));
                    if to_fn_ptr || !matches!(self.ast().expr(*a).kind, ExprKind::Closure { .. }) {
                        let t = self.check_expr(*a, Some(ei));
                        let ei = self.deeply_resolve_ignoring_regions_with_obligations(ei);
                        let pt = self.deeply_resolve_ignoring_regions_with_obligations(pt);
                        let snap = self.start_snapshot();
                        if self.try_coerce(t, ei, Some(*a)) && self.eq(pt, ei) {
                            self.commit_from(snap);
                            continue;
                        }
                        self.rollback_to(snap);
                        self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
                        continue;
                    }
                }
                let xt = self.normalize_signature(pt);
                let t = self.check_expr(*a, Some(xt));
                let pt = self.deeply_resolve_ignoring_regions_with_obligations(pt);
                self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
            }
        }
    }

    fn check_trait_method_call(&mut self, e: ExprId, item: DefId, imp: DefId, self_ty: TyId, derefs: usize, turbofish: GenericArgsId, args: &[ExprId], lo: u32, method_expected: Option<TyId>) -> TyId {
        let error = self.tcx.tys.error;
        let mut tf = Vec::new();
        let turbofish_ok = self.check_seg_arg_count(turbofish, item, lo, true);
        if let (true, Some(GenericArgs::Angle(v))) = (turbofish_ok, self.ast().gargs(turbofish)) {
            for g in self.ast().list(*v) {
                if let Some(lt) = self.generic_arg(g, item, self.tcx.fn_sig(self.prog, item).unwrap().n_parent_generics as usize + tf.len()) {
                    tf.push(lt);
                }
            }
        }
        let gargs = self.trait_call_args(item, imp, self_ty, &tf);
        let known_full = self.known_method_args.take();
        if let Some(full) = &known_full {
            let indices = self.tcx.generic_indices_of(self.prog, item);
            for (i, &idx) in indices.types_consts.iter().enumerate() {
                let t = match full[idx as usize] { crate::consts::GenericArg::Type(t) => t, crate::consts::GenericArg::Const(c) => self.tcx.tys.intern(TyKind::Const(c)), _ => unreachable!() };
                self.eq(gargs[i], t);
            }
        }
        let sig = self.tcx.fn_sig(self.prog, item).cloned().unwrap();
        let autoref = match self.forced_autoref.take() {
            Some(a) => a,
            None => match sig.self_kind {
                2 => 1,
                3 => 2,
                _ => 0,
            },
        };
        self.b.dependencies.push(item);
        self.b.methods.insert(e.0, MethodRes { def: item, derefs: derefs as u32, autoref, args: self.tcx.tys.mk_list(&gargs) });
        let mut full = self.infer_item_args(item, &gargs);
        if let Some(known) = known_full {
            for (arg, known) in full.iter_mut().zip(known) {
                if matches!(known, crate::consts::GenericArg::Lifetime(_)) { *arg = known; }
            }
        }
        self.write_method_user_type(e.0, item, &full);
        let (ps, r) = self.instantiate_call_signature(item, &full, &sig);
        // A probe only establishes that the trait may hold, even when it found
        // one impl. Confirm the pick by retaining its trait obligation (nightly
        // rustc_hir_typeck/src/method/probe.rs:2142-2144, confirm.rs:632-647),
        // and the method's own predicates, once with these args (confirm.rs:150-160).
        self.register_method_obligation(item, &full);
        self.apply_fn_bounds(item, &full, lo);
        let ps: Vec<_> = ps.into_iter().map(|t| self.normalize_signature(t)).collect();
        let extra = self.extra_arg.take();
        let n_extra = extra.is_some() as usize;
        if ps.len() != args.len() + 1 + n_extra {
            let want = ps.len().saturating_sub(1);
            let help = if n_extra == 0 { let rest: Vec<TyId> = ps.iter().skip(1).copied().collect(); self.arg_count_help(Some(item), &rest, args, 0) } else { String::new() };
            self.err(lo, format!("E0061: this method takes {} argument{} but {} {} supplied{}", want, if want == 1 { "" } else { "s" }, args.len() + n_extra, if args.len() + n_extra == 1 { "was" } else { "were" }, help));
        }
        if let (Some(x), Some(&pt)) = (extra, ps.get(args.len() + 1)) {
            self.coerce_or_unify(x, pt, None, lo);
        }
        // the probe reads the formal result as written: normalizing it before the arguments are
        // checked would select an impl for a projection whose trait args are still open
        // (`allocs.index(heading_ix)` with several Index impls)
        let exp_inputs = if method_expected.is_some() { self.method_expected_inputs(r, &ps, method_expected) } else { None };
        self.check_method_args(item, &ps, args, exp_inputs);
        // several impls of the trait for the self type and its args still open (`v.extend(it)`
        // on Vec<String>: Extend<String> and Extend<&String>): the impls whose where-clauses
        // can hold (`T: Copy` rules out the second) -- one left fixes the args (rustc selects
        // once a single candidate remains)
        if imp == crate::program::NO_DEF {
            // the expected type comes first (rustc unifies the expectation before it selects:
            // `SetCursor(cursor.into())` with `impl Into<Shape> for MouseCursor` and the blanket
            // `Into<U> for T where U: From<T>`): a result that is an open trait arg takes a
            // plain expected type
            if let Some(x) = method_expected {
                let (rs, xs) = (self.shallow(r), self.shallow(x));
                let plain = matches!(self.tcx.tys.kind(xs), TyKind::Adt(..) | TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Tuple(_) | TyKind::Array(..)) && !self.has_dyn(xs);
                if plain && matches!(self.tcx.tys.kind(rs), TyKind::Infer(_)) && !self.is_literal_var(rs) && gargs[1..].contains(&rs) {
                    let save = self.start_snapshot();
                    if !self.eq(rs, xs) { self.rollback_to(save); } else { self.commit_from(save); }
                }
            }
        }
        let _ = error;
        self.normalize_signature(r)
    }

    /// The trait ref of a trait item's full args is their parent prefix.
    fn register_method_obligation(&mut self, item: DefId, full: &[crate::consts::GenericArg]) {
        let td = self.prog.def(item).parent;
        if self.prog.def(td).kind != DefKind::Trait { return; }
        let indices = self.tcx.generic_indices_of(self.prog, td);
        let n = indices.types_consts.len() + indices.lifetimes.len();
        let predicate = crate::next_solver::collect::trait_predicate(td, full[..n].to_vec());
        self.register_goal(Goal { param_env: self.param_env.clone(), predicate }, self.last_lo);
    }

    /// `norm` that leaves unresolved literal variables open.
    fn norm_keep(&mut self, t: TyId) -> TyId {
        let save = self.keep_literal_vars;
        self.keep_literal_vars = true;
        let r = self.norm(t);
        self.keep_literal_vars = save;
        r
    }

    /// checks.rs:265-291: before closure arguments, the obligations registered
    /// so far are selected where possible (no literal fallback).
    fn select_before_closures(&mut self) {
        self.retry_goals(false);
    }

    /// Registers `d`'s predicates instantiated with its full args.
    fn apply_fn_bounds(&mut self, d: DefId, args: &[crate::consts::GenericArg], lo: u32) {
        for p in self.tcx.clauses_of(self.prog, d) {
            let predicate = self.tcx.instantiate_predicate(self.prog, d, args, &p);
            self.register_goal(Goal { param_env: self.param_env.clone(), predicate }, lo);
        }
    }

    fn fresh_arg(&self, def: DefId, compact: usize) -> TyId {
        let indices = self.tcx.generic_indices_of(self.prog, def);
        if indices.consts.contains(&indices.types_consts[compact]) {
            self.tcx.tys.intern(TyKind::Const(self.infcx.next_const_var()))
        } else { self.infcx.next_ty_var() }
    }

    fn infer_item_args(&self, def: DefId, compact: &[TyId]) -> Vec<crate::consts::GenericArg> {
        let mut args = self.infcx.fresh_args_for_item(self.prog, def);
        let indices = self.tcx.generic_indices_of(self.prog, def);
        for (i, ty) in compact.iter().enumerate() {
            args[indices.types_consts[i] as usize] = match self.tcx.tys.kind(*ty) {
                TyKind::Const(ct) => crate::consts::GenericArg::Const(*ct),
                _ => crate::consts::GenericArg::Type(*ty),
            };
        }
        args
    }

    fn trait_goal(&self, td: DefId, ty: TyId, args: &[TyId]) -> Goal {
        let mut compact = vec![ty]; compact.extend_from_slice(args);
        let args = self.infer_item_args(td, &compact);
        Goal { param_env: self.param_env.clone(), predicate: crate::next_solver::collect::trait_predicate(td, args) }
    }

    fn select_unspecified_trait(&mut self, td: DefId, ty: TyId) -> Result<Option<ImplSource>, crate::next_solver::NoSolution> {
        let mut args = Vec::new();
        let indices = self.tcx.generic_indices_of(self.prog, td);
        let full = self.infcx.fresh_args_for_item(self.prog, td);
        for &i in &indices.types_consts[1..] {
            args.push(match full[i as usize] { crate::consts::GenericArg::Type(t) => t, crate::consts::GenericArg::Const(c) => self.tcx.tys.intern(TyKind::Const(c)), _ => unreachable!() });
        }
        self.select_trait(td, ty, &args)
    }

    fn select_trait(&mut self, td: DefId, ty: TyId, args: &[TyId]) -> Result<Option<ImplSource>, crate::next_solver::NoSolution> {
        let goal = self.trait_goal(td, ty, args);
        let result = self.infcx.select_in_new_trait_solver(self.prog, goal.clone());
        match &result {
            Ok(Some(source)) => {
                let nested = match source { ImplSource::UserDefined { nested, .. } | ImplSource::Builtin(_, nested) | ImplSource::Param(nested) => nested };
                for goal in nested { self.register_goal(goal.clone(), self.last_lo); }
            }
            Ok(None) => self.register_goal(goal, self.last_lo),
            Err(_) => {}
        }
        result
    }

    /// rustc `drain_stalled_coroutine_obligations`: the goals waiting for this body's coroutines are not
    /// ambiguities; they move to the body's `coroutine_stalled_predicates`, proven after its MIR is built.
    fn drain_stalled_coroutine_obligations(&mut self) {
        self.retry_goals(false);
        let stalled = self.fulfillment_cx.drain_stalled_obligations_for_coroutines(&self.infcx);
        self.stalled_coroutine_goals.extend(stalled);
    }

    /// The drained goals, fully resolved and with their regions erased (typeck writeback), recorded for the
    /// borrow checker of this body.
    fn record_stalled_coroutine_goals(&mut self) {
        if self.stalled_coroutine_goals.is_empty() { return; }
        // (only a checked crate's body is proven later: `makepad run` has no use for the verdict)
        if !self.tcx.validates(self.prog, self.file) { self.stalled_coroutine_goals.clear(); return; }
        let mut resolver = crate::infer::resolve::DeepVariableResolver::new(&self.infcx);
        let mut goals = Vec::new();
        for obligation in std::mem::take(&mut self.stalled_coroutine_goals) {
            let predicate = crate::next_solver::fold::fold_predicate(&self.tcx.tys, &obligation.goal.predicate, &mut resolver);
            let predicate = crate::next_solver::fold::fold_predicate(&self.tcx.tys, &predicate, &mut EraseFreeRegions);
            goals.push((Goal { param_env: obligation.goal.param_env, predicate }, obligation.cause.span));
        }
        self.tcx.coroutine_stalled_predicates.lock().unwrap_or_else(|e| e.into_inner()).insert(self.b.def, goals);
    }

    fn register_goal(&mut self, goal: Goal, lo: u32) {
        self.fulfillment_cx.register_predicate_obligation(&self.infcx, crate::next_solver::fulfill::PredicateObligation { goal, recursion_depth: 0, cause: crate::next_solver::fulfill::ObligationCause { span: lo, body_id: self.b.def } });
    }

    fn retry_goals(&mut self, final_: bool) {
        use crate::next_solver::fulfill::NextSolverError;
        let errors = if final_ { self.fulfillment_cx.evaluate_obligations_error_on_ambiguity(&self.infcx, self.prog) }
            else { self.fulfillment_cx.try_evaluate_obligations(&self.infcx, self.prog) };
        for error in errors {
            let (code, obligation) = match error {
                NextSolverError::TrueError(o) => ("E0277", o),
                NextSolverError::Ambiguity(o) => { if !self.b.errors.is_empty() { continue; } ("E0283", o) },
                NextSolverError::Overflow(o) => ("E0275", o),
            };
            let predicate = &obligation.goal.predicate;
            if crate::wfcheck::obligations::predicate_mentions_error(&self.infcx, predicate) { continue; }
            // rustc's wording (rustc_trait_selection report_selection_error and core's
            // #[rustc_on_unimplemented] messages) for an unmet trait bound
            if let PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(p)) = &predicate.kind {
                let (prog, tcx) = (self.prog, self.tcx);
                let msg = trait_error_message(prog, tcx, code, p, &mut |t| self.ty_str(t));
                self.err(obligation.cause.span, format!("{code}: {msg}"));
                continue;
            }
            // rustc maybe_report_ambiguity: an ambiguous subtype / coercion predicate relates two still unknown
            // types (the first is named): "type annotations needed" (E0282), not an unsatisfied obligation.
            if code == "E0283" && matches!(predicate.kind, PredicateKind::Subtype(_) | PredicateKind::Coerce(_)) {
                self.err(obligation.cause.span, "E0282: type annotations needed".to_string());
                continue;
            }
            if let Some(msg) = crate::wfcheck::obligations::fulfillment_message(&self.infcx, self.prog, code, &obligation.goal) {
                self.err(obligation.cause.span, msg);
                continue;
            }
            let detail = format!("{predicate:?}");
            self.err(obligation.cause.span, format!("{code}: unsatisfied obligation {detail}"));
        }
    }

    // ------------------------------------------------------------ macros

    /// rustc_builtin_macros cfg.rs parse_cfg + rustc_attr_parsing cfg.rs parse_cfg_entry: the argument of `cfg!`
    /// is one cfg predicate (`name`, `name = "str"`, `not(p)`, `any(..)`, `all(..)`, `version(..)`, `target(..)`).
    fn check_cfg_macro(&mut self, lo_tok: usize, hi_tok: usize, pos: u32) {
        use crate::lexer::T;
        let p = self.prog.src(self.file);
        let kind = |i: usize| p.tok(i).kind;
        // the top-level comma-separated runs of lo..hi
        let split = |lo: usize, hi: usize| -> Vec<(usize, usize)> {
            let (mut v, mut depth, mut start) = (Vec::new(), 0i32, lo);
            for i in lo..hi {
                match kind(i) {
                    T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                    T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                    T::Comma if depth == 0 => { v.push((start, i)); start = i + 1; }
                    _ => {}
                }
            }
            if start < hi { v.push((start, hi)); }
            v
        };
        // Err(code) for a malformed predicate in lo..hi
        fn entry(p: crate::cfg::Src<'_>, lo: usize, hi: usize, split: &dyn Fn(usize, usize) -> Vec<(usize, usize)>, depth: u32) -> Result<(), &'static str> {
            use crate::lexer::T;
            if depth > 64 { return Ok(()); }
            if lo >= hi { return Err("E0539"); }
            let first = p.tok(lo).kind;
            // a bare literal is no meta item
            if matches!(first, T::Str | T::RawStr | T::Int | T::Float | T::Char | T::Byte | T::ByteStr | T::CStr) { return Err("E0539"); }
            let name = crate::cfg::tok_str(p, lo);
            if lo + 1 == hi { return Ok(()); }
            match p.tok(lo + 1).kind {
                T::Eq => {
                    // `name = "value"`
                    if hi == lo + 3 && matches!(p.tok(lo + 2).kind, T::Str | T::RawStr) { Ok(()) } else { Err("E0539") }
                }
                T::OpenParen if p.tok(hi - 1).kind == T::CloseParen => {
                    let args = split(lo + 2, hi - 1);
                    match name {
                        b"not" => {
                            if args.len() != 1 { return Err("E0805"); }
                            entry(p, args[0].0, args[0].1, split, depth + 1)
                        }
                        b"any" | b"all" => {
                            for (a, b) in args { entry(p, a, b, split, depth + 1)?; }
                            Ok(())
                        }
                        b"version" | b"target" => Ok(()),
                        _ => Err("E0537"),
                    }
                }
                _ => Err("E0539"),
            }
        }
        if lo_tok >= hi_tok {
            self.err(pos, "macro requires a cfg-pattern as an argument".to_string());
            return;
        }
        let runs = split(lo_tok, hi_tok);
        if runs.len() > 1 {
            // `cfg!(false, false)`: one pattern (a trailing comma is allowed)
            self.err(pos, "expected 1 cfg-pattern".to_string());
            return;
        }
        if let Err(code) = entry(p, lo_tok, hi_tok, &split, 0) {
            let what = match code { "E0805" => "malformed `cfg` macro input", "E0537" => "invalid predicate", _ => "malformed `cfg` macro input" };
            self.err(pos, format!("{}: {}", code, what));
        }
    }

    /// rustc_hir `Expr::is_place_expr` with `check_named_place_expr`'s projection rule: a place based on a local or
    /// static, or containing a dereference.
    fn is_named_place(&self, e: ExprId) -> bool {
        match self.ast().expr(e).kind {
            ExprKind::Paren(x) => self.is_named_place(x),
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                Some(Res::Local(_)) | Some(Res::Err) | None => true,
                Some(Res::Def(d)) => matches!(self.prog.def(*d).kind, DefKind::Static | DefKind::ForeignStatic),
                Some(Res::ConstParam(_)) => false,
            },
            ExprKind::Unary(UnOp::Deref, _) => true,
            ExprKind::Field(b, _) | ExprKind::TupleField(b, _) => self.b.field_idx.get(&e.0).is_some_and(|f| f.0 > 0) || self.is_named_place(b),
            ExprKind::Index(b, _) => self.is_named_place(b),
            _ => false,
        }
    }

    fn check_mac(&mut self, e: ExprId, m: &MacCall, expected: Option<TyId>, lo: u32) -> TyId {
        let ast = self.ast();
        let name = match ast.segs(m.path.segs).last() {
            Some(s) => self.text(s.name),
            None => "",
        };
        let unit = self.tcx.tys.unit;
        let never = self.tcx.tys.never;
        let bool_ = self.tcx.tys.bool_;
        if name == "cfg" {
            if self.tcx.validates(self.prog, self.file) && m.toks.hi >= m.toks.lo {
                self.check_cfg_macro(m.toks.lo as usize, m.toks.hi as usize, lo);
            }
            return bool_;
        }
        // `compile_error!("msg")` (what a failed built-in or derive leaves at its call): the
        // message is the error, as rustc reports it
        // inline assembly is assembled unless `--no-asm` (user decision 2026-10-21)
        if matches!(name, "asm" | "global_asm" | "naked_asm") {
            if self.prog.session.options.no_asm {
                self.err(lo, relax_session::options::asm_blocked(&self.prog.file_crate_name(self.file)));
                return self.tcx.tys.error;
            }
            if name == "global_asm" || m.args == NO_MAC_ARGS {
                // global_asm! as an expression: not where rustc accepts it
                self.err(lo, format!("`{}!` is not allowed here", name));
                return self.tcx.tys.error;
            }
            // naked_asm! (the body of a naked fn, lowering checks where it is) never returns
            let t = self.check_asm(m);
            return if name == "naked_asm" { self.tcx.tys.never } else { t };
        }
        if name == "compile_error" && m.toks.hi > m.toks.lo {
            let f = &self.prog.files[self.file as usize];
            let msg = crate::lit::unescape_str(f.tok_text(m.toks.lo));
            self.err(lo, msg);
            return never;
        }
        if m.args == NO_MAC_ARGS {
            self.err(lo, format!("macro `{}!` is not supported", name));
            return self.tcx.tys.error;
        }
        let margs = ast.mac_arg(m.args);
        let mexprs = ast.list(margs.exprs);
        match name {
            "assert" | "debug_assert" => {
                if let Some(&c) = mexprs.first() {
                    let t = self.check_expr(c, Some(bool_));
                    self.coerce_or_unify(t, bool_, None, lo);
                }
                self.check_fmt_args(e, &mexprs[1.min(mexprs.len())..]);
                self.diverges = false;
                unit
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                if mexprs.len() >= 2 {
                    let ta = self.check_expr(mexprs[0], None);
                    let tb = self.check_expr(mexprs[1], Some(ta));
                    if !self.eq(ta, tb) {
                        let (sa, sb) = (self.ty_str(ta), self.ty_str(tb));
                        self.err(lo, format!("assert_eq of `{}` and `{}`", sa, sb));
                    }
                }
                self.check_fmt_args(e, &mexprs[2.min(mexprs.len())..]);
                self.diverges = false;
                unit
            }
            "print" | "println" | "eprint" | "eprintln" => {
                self.check_fmt_args(e, mexprs);
                unit
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                self.check_fmt_args(e, mexprs);
                never
            }
            "matches" | "assert_matches" | "debug_assert_matches" => {
                if let (Some(&x), Some(p)) = (mexprs.first(), margs.pat) {
                    let t = self.check_expr(x, None);
                    let saved = self.scopes.len();
                    self.check_pat(p, t);
                    if let Some(g) = margs.guard {
                        self.check_expr(g, Some(bool_));
                    }
                    self.scopes.truncate(saved);
                }
                if name == "matches" {
                    bool_
                } else {
                    self.check_fmt_args(e, &mexprs[1.min(mexprs.len())..]);
                    self.diverges = false;
                    unit
                }
            }
            _ => {
                let _ = expected;
                self.err(lo, format!("macro `{}!` is not supported yet", name));
                for x in mexprs {
                    self.check_expr(*x, None);
                }
                self.tcx.tys.error
            }
        }
    }

    /// Format string + args: checks args and resolves inline `{name}` arguments.
    fn check_fmt_args(&mut self, e: ExprId, exprs: &[ExprId]) {
        if exprs.is_empty() {
            return;
        }
        let ast = self.ast();
        let first = exprs[0];
        let f = &self.prog.files[self.file as usize];
        let fe = ast.expr(first);
        if let ExprKind::Lit(LitKind::Str) | ExprKind::Lit(LitKind::RawStr) = fe.kind {
            let text = std::str::from_utf8(f.bytes(fe.lo, fe.hi)).unwrap_or("");
            let s = crate::lit::unescape_str(text);
            let pieces = crate::lit::parse_fmt(&s);
            for p in &pieces {
                if let crate::lit::FmtPiece::Arg { named: Some(n), .. } = p {
                    if let Some(sy) = self.prog.syms.get(n) {
                        match self.lookup_local(sy, fe.lo) {
                            Some(li) => {
                                self.b.fmt_named.insert((e.0, sy), Res::Local(li));
                            }
                            None => match self.prog.lookup_in_scope(self.module, sy, false) {
                                Some(d) => {
                                    self.b.fmt_named.insert((e.0, sy), Res::Def(d));
                                }
                                None => self.err(fe.lo, format!("cannot find `{}` for format string", n)),
                            },
                        }
                    } else {
                        self.err(fe.lo, format!("cannot find `{}` for format string", n));
                    }
                }
            }
            self.check_expr(first, None);
        } else {
            self.check_expr(first, None);
        }
        for x in &exprs[1..] {
            self.check_expr(*x, None);
        }
    }
}

/// Like ty_to_string with ADTs as full definition paths plus their crate and def id
/// (disambiguates "expected `LiveId`, found `LiveId`").
/// rustc's wording (rustc_trait_selection report_selection_error and core's
/// #[rustc_on_unimplemented] messages) for an unmet or ambiguous trait bound.
/// Replaces every region not bound inside the folded value by the erased region.
struct EraseFreeRegions;
impl crate::types::fold::TypeFolder for EraseFreeRegions {
    fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId { crate::types::fold::super_fold_ty(tys, ty, self, depth) }
    fn fold_region(&mut self, tys: &crate::types::Types, r: crate::types::RegionId, depth: u32) -> crate::types::RegionId {
        if matches!(tys.region_kind(r), crate::types::RegionKind::ReBound(db, _) if db < depth) { r } else { crate::types::RegionId::ERASED }
    }
}

pub(crate) fn trait_error_message(prog: &Program, tcx: &Tcx, code: &str, p: &crate::next_solver::TraitClause, ty_str: &mut dyn FnMut(TyId) -> String) -> String {
    let mut args = Vec::new();
    for arg in &p.trait_ref.args {
        args.push(match arg { crate::consts::GenericArg::Type(t) => ty_str(*t), crate::consts::GenericArg::Const(c) => tcx.tys.const_to_string(*c), crate::consts::GenericArg::Lifetime(r) => format!("{r:?}") });
    }
    let td = p.trait_ref.def_id;
    let name = prog.name(td).to_string();
    let core = Some(prog.def(td).krate) == prog.prelude_crate || matches!(prog.syms.str(prog.crates[prog.def(td).krate as usize].name), "core" | "alloc" | "std");
    let self_ty = args.first().cloned().unwrap_or_default();
    let tr = if args.len() > 1 { format!("{}<{}>", name, args[1..].join(", ")) } else { name.clone() };
    match code {
        "E0277" if tcx.lang_items.get(crate::lang_items::LangItem::Sized) == Some(td) => format!("the size for values of type `{}` cannot be known at compilation time", self_ty),
        "E0277" if core && name == "Send" => format!("`{}` cannot be sent between threads safely", self_ty),
        "E0277" if core && name == "Sync" => format!("`{}` cannot be shared between threads safely", self_ty),
        "E0277" if core && name == "Iterator" => format!("`{}` is not an iterator", self_ty),
        "E0277" => format!("the trait bound `{}: {}` is not satisfied", self_ty, tr),
        "E0283" => format!("type annotations needed: cannot satisfy `{}: {}`", self_ty, tr),
        _ => format!("overflow evaluating the requirement `{}: {}`", self_ty, tr),
    }
}

pub fn ty_to_string_paths(prog: &Program, tcx: &Tcx, t: TyId) -> String {
    match tcx.tys.kind(t) {
        TyKind::Adt(d, args, _) => {
            let krate = prog.syms.str(prog.crates[prog.def(*d).krate as usize].name);
            let mut s = format!("{}::{} (crate {}, def {})", krate, prog.def_path(*d), prog.def(*d).krate, d.0);
            if !args.is_empty() {
                let a: Vec<String> = tcx.tys.list(*args).iter().map(|x| ty_to_string_paths(prog, tcx, *x)).collect();
                s.push_str(&format!("<{}>", a.join(", ")));
            }
            s
        }
        TyKind::Ref(m, e, _) => format!("&{}{}", if *m { "mut " } else { "" }, ty_to_string_paths(prog, tcx, *e)),
        _ => ty_to_string(false, prog, tcx, t),
    }
}

/// `core::any::type_name` (rustc's printing: full paths, fn items as their path, closures as
/// `path::{{closure}}`, arguments recursively).
/// Levenshtein distance (bytes) between two names.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let c = if a[i - 1].eq_ignore_ascii_case(&b[j - 1]) { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + c);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// `-- import it: `use a::b::Name;`` when the program has items of that name (the paths that
/// exist, shortest first; at most 3), else nothing.
pub fn import_help(prog: &Program, module: u32, name: crate::program::Sym, ns_type: bool) -> String {
    let v = prog.import_suggestions(module, name, ns_type);
    match v.len() {
        0 => String::new(),
        1 => format!(" -- import it: `use {};`", v[0]),
        _ => format!(" -- it exists as {}", v.iter().take(3).map(|p| format!("`use {};`", p)).collect::<Vec<_>>().join(", ")),
    }
}

pub fn type_name(prog: &Program, tcx: &Tcx, t: TyId) -> String {
    let list = |v: &[TyId]| v.iter().map(|x| type_name(prog, tcx, *x)).collect::<Vec<_>>().join(", ");
    match tcx.tys.kind(t) {
        TyKind::Adt(d, args, _) => {
            let mut s = item_path(prog, tcx, *d);
            if !args.is_empty() {
                s.push_str(&format!("<{}>", list(tcx.tys.list(*args))));
            }
            s
        }
        TyKind::Tuple(v) => {
            if tcx.tys.list(*v).len() == 1 {
                format!("({},)", list(tcx.tys.list(*v)))
            } else {
                format!("({})", list(tcx.tys.list(*v)))
            }
        }
        TyKind::Array(e, n) => format!("[{}; {}]", type_name(prog, tcx, *e), tcx.tys.const_to_string(*n)),
        TyKind::Slice(e) => format!("[{}]", type_name(prog, tcx, *e)),
        TyKind::Ref(m, e, _) => format!("&{}{}", if *m { "mut " } else { "" }, type_name(prog, tcx, *e)),
        TyKind::Ptr(m, e) => format!("*{} {}", if *m { "mut" } else { "const" }, type_name(prog, tcx, *e)),
        TyKind::FnPtr(ps, r, header, _) => {
            let safety = if header.safety == Safety::Unsafe { "unsafe " } else { "" };
            let abi = if header.abi == Abi::Rust { String::new() } else { format!("extern \"{}\" ", header.abi.name()) };
            let mut params = list(tcx.tys.list(*ps));
            if header.c_variadic {
                if !params.is_empty() { params.push_str(", "); }
                params.push_str("...");
            }
            let unit = matches!(tcx.tys.kind(*r), TyKind::Tuple(v) if v.is_empty());
            if unit {
                format!("{}{}fn({})", safety, abi, params)
            } else {
                format!("{}{}fn({}) -> {}", safety, abi, params, type_name(prog, tcx, *r))
            }
        }
        TyKind::FnDef(d, args, _) => {
            let mut s = item_path(prog, tcx, *d);
            if !args.is_empty() {
                s.push_str(&format!("<{}>", list(tcx.tys.list(*args))));
            }
            s
        }
        TyKind::Closure(file, e, crate::types::ClosureArgs { parent_args: owner, .. }) => {
            // the enclosing fn's path, then one `{{closure}}` per closure / async block around
            // this one and itself (rustc: `krate::main::{{closure}}::{{closure}}` for an async
            // closure's future)
            let owner_def = match tcx.tys.kind(*owner) {
                TyKind::FnDef(d, _, _) => Some(*d),
                _ => None,
            };
            let mut s = match owner_def {
                Some(d) => item_path(prog, tcx, d),
                None => String::new(),
            };
            let mut depth = 1;
            if *e & crate::tcx::ASYNC_FN_BODY == 0 {
                if let (Some(d), Some(pf)) = (owner_def, prog.files.get(*file as usize)) {
                    let it = pf.ast.item(prog.def(d).item);
                    let me = pf.ast.expr(crate::ast::ExprId(*e));
                    let _ = it;
                    for i in prog.item_expr_ranges(*file, prog.def(d).item).into_iter().flat_map(|(a, b)| a..b) {
                        if i == *e || !pf.ast.has_expr(crate::ast::ExprId(i)) {
                            continue;
                        }
                        let x = pf.ast.expr(crate::ast::ExprId(i));
                        if matches!(x.kind, crate::ast::ExprKind::Closure { .. } | crate::ast::ExprKind::Async(..)) && x.lo <= me.lo && me.hi <= x.hi && (x.lo, x.hi) != (me.lo, me.hi) {
                            depth += 1;
                        }
                    }
                }
            }
            for _ in 0..depth {
                if !s.is_empty() {
                    s.push_str("::");
                }
                s.push_str("{{closure}}");
            }
            s
        }
        TyKind::Dyn(d, args, bindings, _, _, _, autos) => {
            let (head, autos) = dyn_trait_names(*d, tcx.tys.list(*autos), |a| item_path(prog, tcx, a));
            let mut s = format!("dyn {}", head);
            // the Fn family as rustc prints it: `Fn(u32) -> u32`
            let fn_family = matches!(prog.name(*d), "Fn" | "FnMut" | "FnOnce") && Some(prog.def(*d).krate) == prog.prelude_crate;
            match (fn_family, tcx.tys.list(*args).first().map(|a| tcx.tys.kind(*a))) {
                (true, Some(TyKind::FnPtr(ps, r, _, _))) => {
                    s.push_str(&format!("({})", list(tcx.tys.list(*ps))));
                    if !matches!(tcx.tys.kind(*r), TyKind::Tuple(v) if v.is_empty()) {
                        s.push_str(&format!(" -> {}", type_name(prog, tcx, *r)));
                    }
                }
                (true, Some(TyKind::Tuple(ps))) => {
                    s.push_str(&format!("({})", list(tcx.tys.list(*ps))));
                    if let Some((_, r, _)) = tcx.tys.list(*bindings).first() {
                        if !matches!(tcx.tys.kind(*r), TyKind::Tuple(v) if v.is_empty()) {
                            s.push_str(&format!(" -> {}", type_name(prog, tcx, *r)));
                        }
                    }
                }
                _ => {
                    if !args.is_empty() {
                        s.push_str(&format!("<{}>", list(tcx.tys.list(*args))));
                    }
                }
            }
            for a in autos {
                s.push_str(" + ");
                s.push_str(&a);
            }
            s
        }
        _ => ty_to_string(false, prog, tcx, t),
    }
}

/// A trait object's trait names as rustc prints them (pretty_print_dyn_existential): the
/// principal, then the auto traits sorted by path. An object of auto traits only stores its
/// first auto trait as the principal (`dyn Send + Sync`): all of them sort, the first leads.
fn dyn_trait_names(d: DefId, autos: &[DefId], path: impl Fn(DefId) -> String) -> (String, Vec<String>) {
    let auto_only = autos.contains(&d);
    let mut names: Vec<String> = autos.iter().filter(|a| auto_only || **a != d).map(|a| path(*a)).collect();
    names.sort();
    let head = if auto_only { names.remove(0) } else { path(d) };
    (head, names)
}

/// The impl whose body (through nested fn bodies) declares `owner`: the impl `Self` constructors
/// of nested items refer to.
fn outer_impl(prog: &Program, owner: DefId) -> Option<DefId> {
    let mut d = owner;
    loop {
        let o = *prog.block_owner.get(&prog.def(d).scope)?;
        let parent = prog.def(o).parent;
        if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Impl {
            return Some(parent);
        }
        d = o;
    }
}

/// An item's path as rustc's type_name prints it: an item in a fn body is under that fn
/// (`krate::main::Wrapper`); a method's path goes through its impl's self type with the
/// impl's own parameters left open (`krate::Bar<_>::foo`).
pub fn item_path(prog: &Program, tcx: &Tcx, d: DefId) -> String {
    let def = prog.def(d);
    let name = prog.syms.str(def.name);
    // an item collected in a fn body's block module: the fn's path, then the item
    if let Some(&owner) = prog.block_owner.get(&def.scope) {
        if owner != d && matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Fn | DefKind::Trait | DefKind::TypeAlias | DefKind::Const | DefKind::Static) && prog.def(def.parent).kind == DefKind::Mod {
            return format!("{}::{}", item_path(prog, tcx, owner), name);
        }
    }
    let parent = def.parent;
    if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Impl {
        if let Some(&st) = tcx.impl_self.get(&parent) {
            if let TyKind::Adt(sd, args, _) = tcx.tys.kind(st) {
                let mut s = item_path(prog, tcx, *sd);
                if !args.is_empty() {
                    s.push_str(&format!("<{}>", vec!["_"; tcx.tys.list(*args).len()].join(", ")));
                }
                return format!("{}::{}", s, name);
            }
        }
    }
    // under a trait or enum, or in a body whose owner is its def parent (rustc's DefCollector,
    // def_collector.rs visit_block): the parent's path, so an impl above prints its self type
    if parent != crate::program::NO_DEF && !matches!(prog.def(parent).kind, DefKind::Mod | DefKind::Impl) {
        return format!("{}::{}", item_path(prog, tcx, parent), name);
    }
    prog.def_path(d)
}

pub fn ty_to_string(stable_closures: bool, prog: &Program, tcx: &Tcx, t: TyId) -> String {
    match tcx.tys.kind(t) {
        TyKind::Bool => "bool".to_string(),
        TyKind::Char => "char".to_string(),
        TyKind::Str => "str".to_string(),
        TyKind::Int(p) | TyKind::Float(p) => crate::types::Types::prim_name(*p).to_string(),
        TyKind::Foreign(d) => prog.def_path(*d),
        TyKind::Never => "!".to_string(),
        TyKind::Tuple(v) => {
            let mut s = "(".to_string();
            for (i, x) in tcx.tys.list(*v).iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&ty_to_string(stable_closures, prog, tcx, *x));
            }
            if tcx.tys.list(*v).len() == 1 {
                s.push(',');
            }
            s.push(')');
            s
        }
        TyKind::Array(e, n) => format!("[{}; {}]", ty_to_string(stable_closures, prog, tcx, *e), tcx.tys.const_to_string(*n)),
        TyKind::Const(v) => tcx.tys.const_to_string(*v),
        TyKind::Slice(e) => format!("[{}]", ty_to_string(stable_closures, prog, tcx, *e)),
        TyKind::Ref(m, e, _) => format!("&{}{}", if *m { "mut " } else { "" }, ty_to_string(stable_closures, prog, tcx, *e)),
        TyKind::Ptr(m, e) => format!("*{} {}", if *m { "mut" } else { "const" }, ty_to_string(stable_closures, prog, tcx, *e)),
        TyKind::Adt(d, args, _) => {
            let mut s = prog.name(*d).to_string();
            if !args.is_empty() {
                s.push('<');
                for (i, x) in tcx.tys.list(*args).iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&ty_to_string(stable_closures, prog, tcx, *x));
                }
                s.push('>');
            }
            s
        }
        TyKind::FnPtr(ps, r, _, _) => {
            let mut s = "fn(".to_string();
            for (i, x) in tcx.tys.list(*ps).iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&ty_to_string(stable_closures, prog, tcx, *x));
            }
            s.push_str(") -> ");
            s.push_str(&ty_to_string(stable_closures, prog, tcx, *r));
            s
        }
        TyKind::FnDef(d, _, _) => format!("fn item {}", prog.def_path(*d)),
        TyKind::Param(i) => format!("T{}", i),
        TyKind::Infer(v) => format!("?{}", v),
        TyKind::IntVar(v) => format!("?I{}", v),
        TyKind::FloatVar(v) => format!("?F{}", v),
        TyKind::Bound(d, b) => format!("^{}.{}:{:?}", d, b.var.0, b.kind),
        TyKind::Placeholder(p) => format!("!U{}.{}", p.universe.0, p.bound.0),
        TyKind::Closure(f, e, crate::types::ClosureArgs { parent_args: owner, .. }) => {
            if stable_closures {
                let owner = match tcx.tys.kind(*owner) {
                    TyKind::FnDef(d, _, _) => Some(*d),
                    _ => None,
                };
                crate::program::stable_closure_name(prog, *f, *e, owner)
            } else {
                format!("closure#{}", e)
            }
        }
        TyKind::Dyn(d, args, _, _, _, _, autos) => {
            let (head, autos) = dyn_trait_names(*d, tcx.tys.list(*autos), |a| prog.def_path(a));
            let mut s = format!("dyn {}", head);
            if !args.is_empty() {
                s.push('<');
                for (i, a) in tcx.tys.list(*args).iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&ty_to_string(stable_closures, prog, tcx, *a));
                }
                s.push('>');
            }
            for a in autos {
                s.push_str(" + ");
                s.push_str(&a);
            }
            s
        }
        TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { def_id: d }, args, regions: _ }) => {
            let st = match tcx.tys.list(*args).first() {
                Some(t) => ty_to_string(stable_closures, prog, tcx, *t),
                None => "?".to_string(),
            };
            format!("<{} as {}>::{}", st, prog.name(prog.def(*d).parent), prog.name(*d))
        }
        TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { def_id: d }, .. }) => {
            let mut s = "impl ".to_string();
            if let Some(o) = tcx.opaques.get(d) {
                for (k, (td, _)) in o.bounds.iter().enumerate() {
                    if k > 0 {
                        s.push_str(" + ");
                    }
                    s.push_str(prog.name(*td));
                }
            }
            s
        }
        TyKind::Alias(_, alias) => format!("{}<{}>", prog.name(alias.kind.def_id()), tcx.tys.list(alias.args).iter().map(|ty| ty_to_string(stable_closures, prog, tcx, *ty)).collect::<Vec<_>>().join(", ")),
        TyKind::Error => "{error}".to_string(),
    }
}

pub fn key_of(tcx: &Tcx, t: TyId) -> TyKey {
    ty_key(&tcx.tys, t)
}

pub fn prim_of(tcx: &Tcx, t: TyId) -> Option<Prim> {
    match tcx.tys.kind(t) {
        TyKind::Int(p) | TyKind::Float(p) => Some(*p),
        TyKind::Bool => Some(Prim::Bool),
        TyKind::Char => Some(Prim::Char),
        _ => None,
    }
}

/// `..` or `name @ ..` (the rest of a slice pattern).
pub fn is_rest_pat(ast: &Ast, p: PatId) -> bool {
    match ast.pat(p) {
        Pat::Rest => true,
        Pat::Ident { sub: Some(s), .. } => matches!(ast.pat(*s), Pat::Rest),
        _ => false,
    }
}

/// Walk a resolved type without allocating a set in the type-checker's hot path.
pub fn type_dependencies(tcx: &Tcx, ty: TyId, out: &mut impl Extend<DefId>) {
    fn walk(tcx: &Tcx, ty: TyId, out: &mut impl Extend<DefId>, depth: usize) {
        if depth > 64 { return; }
        match tcx.tys.kind(ty) {
            TyKind::Adt(d, a, _) | TyKind::FnDef(d, a, _) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { def_id: d }, args: a, regions: _ }) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { def_id: d }, args: a, regions: _ }) => {
                out.extend([*d]);
                for t in tcx.tys.list(*a) { walk(tcx, *t, out, depth + 1); }
            }
            TyKind::Ref(_, t, _) | TyKind::Ptr(_, t) | TyKind::Slice(t) | TyKind::Array(t, _) => walk(tcx, *t, out, depth + 1),
            TyKind::Tuple(a) => for t in tcx.tys.list(*a) { walk(tcx, *t, out, depth + 1); },
            TyKind::FnPtr(a, t, _, _) => {
                for x in tcx.tys.list(*a) { walk(tcx, *x, out, depth + 1); }
                walk(tcx, *t, out, depth + 1);
            }
            _ => {}
        }
    }
    walk(tcx, ty, out, 0);
}

#[cfg(test)]
mod solve;

/// core's `#[rustc_on_unimplemented]` message of a binary operator trait (`i32: Add<u8>`
/// unmet, rustc E0277): library/core/src/ops/arith.rs, bit.rs, cmp.rs.
fn binop_unimplemented(op: crate::ast::BinOp, x: &str, y: &str) -> String {
    use crate::ast::BinOp::*;
    match op {
        Add => format!("cannot add `{}` to `{}`", y, x),
        Sub => format!("cannot subtract `{}` from `{}`", y, x),
        Mul => format!("cannot multiply `{}` by `{}`", x, y),
        Div => format!("cannot divide `{}` by `{}`", x, y),
        Rem => format!("cannot calculate the remainder of `{}` divided by `{}`", x, y),
        BitAnd => format!("no implementation for `{} & {}`", x, y),
        BitOr => format!("no implementation for `{} | {}`", x, y),
        BitXor => format!("no implementation for `{} ^ {}`", x, y),
        Shl => format!("no implementation for `{} << {}`", x, y),
        Shr => format!("no implementation for `{} >> {}`", x, y),
        Eq | Ne => format!("can't compare `{}` with `{}`", x, y),
        Lt | Le | Gt | Ge => format!("can't compare `{}` with `{}`", x, y),
        And | Or => format!("mismatched types: expected `bool`, found `{}`", y),
    }
}

/// An operator trait with no impl for the operands (rustc op.rs report_binop_failure): E0368
/// for a compound assignment, else E0369 with the operator's wording.
fn op_trait_unimplemented(tr: &str, x: &str, y: &str) -> String {
    let assign = |op: &str| format!("E0368: binary assignment operation `{}` cannot be applied to type `{}`", op, x);
    match tr {
        "Add" => format!("E0369: cannot add `{}` to `{}`", y, x),
        "Sub" => format!("E0369: cannot subtract `{}` from `{}`", y, x),
        "Mul" => format!("E0369: cannot multiply `{}` by `{}`", x, y),
        "Div" => format!("E0369: cannot divide `{}` by `{}`", x, y),
        "Rem" => format!("E0369: cannot calculate the remainder of `{}` divided by `{}`", x, y),
        "BitAnd" => format!("E0369: no implementation for `{} & {}`", x, y),
        "BitOr" => format!("E0369: no implementation for `{} | {}`", x, y),
        "BitXor" => format!("E0369: no implementation for `{} ^ {}`", x, y),
        "Shl" => format!("E0369: no implementation for `{} << {}`", x, y),
        "Shr" => format!("E0369: no implementation for `{} >> {}`", x, y),
        "AddAssign" => assign("+="), "SubAssign" => assign("-="), "MulAssign" => assign("*="),
        "DivAssign" => assign("/="), "RemAssign" => assign("%="), "BitAndAssign" => assign("&="),
        "BitOrAssign" => assign("|="), "BitXorAssign" => assign("^="), "ShlAssign" => assign("<<="),
        "ShrAssign" => assign(">>="),
        _ => format!("E0277: the trait bound `{}: {}<{}>` is not satisfied", x, tr, y),
    }
}
