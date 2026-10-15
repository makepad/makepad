//! Type queries the borrow checker needs, over `&Program` + `&Tcx` only (usable from a
//! background thread under FRONT_RW.read(); no Unit).

use crate::next_solver::evaluate_obligation::EvaluationResult;
use crate::next_solver::select::ImplSource;
use crate::fxhash::HashMap;
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use std::cell::RefCell;

pub struct Tx<'a> {
    pub errors: crate::infer::diagnostics::DiagCtxt,
    pub cx: crate::tcx::EvalCx<'a>,
    pub fv: std::rc::Rc<super::fv::SummaryCx>,
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
    pub pending_traits: RefCell<Vec<(crate::next_solver::Goal, DefId)>>,
    pub obligation_owner: std::cell::Cell<DefId>,
    copy_memo: RefCell<HashMap<TyId, bool>>,
    rawptr_memo: RefCell<HashMap<TyId, bool>>,
    lang_memo: RefCell<HashMap<&'static str, Option<DefId>>>,
    pub box_def: Option<DefId>,
    pub copy_trait: Option<DefId>,
    /// crate indices of core / alloc / std
    std_crates: Vec<u32>,
    /// polymorphic check of a generic fn: per generic param, bounded by Copy
    pub param_copy: RefCell<Vec<bool>>,
    /// checking a generic body polymorphically (typeck's picks there are less faithful)
    pub polymorphic: std::cell::Cell<bool>,
    /// closures built by the checker: Copy when every capture is (by value Copy, or a shared
    /// borrow) -- rustc's rule; Relax's own upvar types hide `&mut` captures as pointers
    pub closure_copy: RefCell<HashMap<(u32, u32), bool>>,
    /// The body's clauses for the native solver.
    pub param_env: RefCell<crate::next_solver::ParamEnv>,
    pub(super) param_env_id: std::cell::Cell<crate::tcx::ParamEnvId>,
}

impl<'a> Tx<'a> {
    /// The types of an interned list (typeck's recorded generic args).
    #[inline]
    pub fn list(&self, l: crate::types::TyList) -> &'a [TyId] {
        self.tcx.tys.list(l)
    }
    fn infer_ctxt(&self) -> crate::infer::InferCtxt<'a> {
        let infcx = crate::infer::InferCtxt::for_item(
            self.tcx, self.prog, self.cx.typing_env.mode, self.obligation_owner.get());

        infcx
    }

    pub fn new(prog: &'a Program, tcx: &'a Tcx, env: crate::tcx::TypingEnv, state: &'a crate::tcx::EvalState) -> Tx<'a> {
        let mut std_crates = Vec::new();
        for (i, c) in prog.crates.iter().enumerate() {
            if matches!(prog.syms.str(c.name), "core" | "alloc" | "std") {
                std_crates.push(i as u32);
            }
        }
        let errors = tcx.dcx.clone();
        let mut t = Tx {
            cx: crate::tcx::EvalCx::new(prog, &tcx, env, state),
            errors,
            fv: Default::default(),
            prog,
            tcx,
            pending_traits: RefCell::new(Vec::new()),
            obligation_owner: std::cell::Cell::new(NO_DEF),
            copy_memo: RefCell::new(HashMap::default()),
            rawptr_memo: RefCell::new(HashMap::default()),
            lang_memo: RefCell::new(HashMap::default()),
            box_def: None,
            copy_trait: tcx.copy_trait,
            std_crates,
            param_copy: RefCell::new(Vec::new()),
            polymorphic: std::cell::Cell::new(false),
            closure_copy: RefCell::new(HashMap::default()),
            // N:rustc_ty_utils/src/ty.rs:150-156 and solve/mod.rs:280-319:
            // const-only bodies need their ConstArgHasType clauses too.
            param_env: RefCell::new(tcx.param_env_by_id(env.param_env).clone()),
            param_env_id: std::cell::Cell::new(env.param_env),
        };
        t.box_def = t.lang("boxed::Box");
        if t.copy_trait.is_none() {
            t.copy_trait = t.lang("marker::Copy");
        }
        t
    }

    /// For a polymorphic check of `def`: which of its generic params are bounded by Copy (the
    /// fn's own bounds and its impl's).
    pub fn reset_copy_memo(&self) {
        self.copy_memo.borrow_mut().clear();
    }

    /// Back to a monomorphic body (after a polymorphic one).
    pub fn reset_env(&self) {
        if self.polymorphic.get() {
            self.polymorphic.set(false);
            self.param_copy.borrow_mut().clear();
            self.copy_memo.borrow_mut().clear();
            self.param_env_id.set(self.cx.typing_env.param_env);
            *self.param_env.borrow_mut() = self.tcx.param_env_by_id(self.cx.typing_env.param_env).clone();
        }
    }

    pub fn set_generic_env(&self, def: DefId, n: usize) {
        let id = self.tcx.param_env_id(self.prog, def);
        self.param_env_id.set(id);
        *self.param_env.borrow_mut() = self.tcx.param_env_by_id(id).clone();
        let mut v = vec![false; n];
        let ct = match self.copy_trait {
            Some(c) => c,
            None => return,
        };
        let implies_copy = |td: DefId| td == ct || self.tcx.trait_closure.get(&td).map_or(false, |c| c.contains(&ct));
        let mark = |bs: &Vec<(u32, DefId, Vec<TyId>)>, v: &mut Vec<bool>| {
            for (p, td, _) in bs {
                if (*p as usize) < n && implies_copy(*td) {
                    v[*p as usize] = true;
                }
            }
        };
        if let Some(bs) = self.tcx.fn_trait_bounds(self.prog, def) {
            mark(bs, &mut v);
        }
        let parent = self.prog.def(def).parent;
        if let Some(bs) = self.tcx.impl_trait_bounds.get(&parent) {
            mark(bs, &mut v);
        }
        // the raw bounds (Copy itself is not in the tables above): the fn's environment
        // includes its impl's params
        let env = self.tcx.item_env(self.prog, def);
        let module = self.prog.def(def).scope;
        for (pi, file, path) in &env.bounds {
            if (*pi as usize) < n && !v[*pi as usize] {
                if let Some(td) = self.tcx.resolve_type_path_def(self.prog, *file, module, path) {
                    if implies_copy(td) {
                        v[*pi as usize] = true;
                    }
                }
            }
        }
        // a trait's Self: Copy when the trait has Copy as a supertrait
        if self.prog.def(parent).kind == DefKind::Trait && n > 0 && implies_copy(parent) {
            v[0] = true;
        }
        *self.param_copy.borrow_mut() = v;
        self.polymorphic.set(true);
        self.copy_memo.borrow_mut().clear();
    }

    /// BIR adapter, deleted with the BIR (C-NLL5): the normalized shape of a type the BIR
    /// builder and the region-slot templates lay out, through the native solver's
    /// Normalize type operation (type_op/normalize.rs, rustc_traits type_op.rs:41).
    /// This layout query has no region numbering: the body's region constraints from
    /// normalization are produced where its regions are numbered (nll.rs, through
    /// fully_perform_op). A failed operation keeps the unnormalized type, as
    /// type_check/canonical.rs:205 normalize_with_category; a region variable the
    /// operation leaves unresolved is a fresh slot (erased) to the layout.
    pub fn normalize(&self, ty: TyId) -> TyId {
        self.normalize_in_env(ty, &self.param_env_id.get())
    }

    pub(super) fn normalize_in_env(&self, ty: TyId, param_env: &crate::next_solver::ParamEnvId) -> TyId {
        let tys = &self.tcx.tys;
        let ty = self.cx.infcx.map_or(ty, |infcx| infcx.deeply_resolve_ignoring_regions(ty));
        if !tys.may_project(ty) && !tys.has_opaque(ty) { return ty; }
        // Capture analysis runs during type checking with the body's inference context
        // (upvar::analyze): its types can still hold that table's variables, which the fresh
        // context of the type operation below cannot resolve. They are resolved in the body's
        // context first (rustc expr_use_visitor.rs, FnCtxt: resolve_vars_if_possible in its own
        // InferCtxt); a variable the body has not inferred yet is left as it is.
        let ty = match self.cx.infcx {
            Some(body) => {
                let ty = body.deeply_resolve_ignoring_regions(ty);
                if tys.has_infer(ty) { return ty; }
                ty
            }
            None => ty,
        };
        let infcx = self.infer_ctxt();
        let param_env = param_env.clone();
        with_foreign_region_vars(&infcx, ty, |ty| in_signature_binder(tys, ty, |value| {
            let op = crate::next_solver::type_op::ParamEnvAnd { param_env, value: crate::next_solver::type_op::Normalize { value } };
            crate::next_solver::type_op::TypeOp::fully_perform(op, &infcx, self.prog).map(|out| out.output).ok()
        })).unwrap_or(ty)
    }

    pub fn kind(&self, t: TyId) -> TyKind {
        let t = self.cx.infcx.map_or(t, |infcx| infcx.shallow_resolve(t));
        self.tcx.tys.kind(t).clone()
    }

    pub fn is_std_def(&self, d: DefId) -> bool {
        d != NO_DEF && self.std_crates.contains(&self.prog.def(d).krate)
    }

    /// `core::<path>` (also found through core's re-exports), "::"-separated.
    pub fn lang(&self, path: &'static str) -> Option<DefId> {
        if let Some(r) = self.lang_memo.borrow().get(path) {
            return *r;
        }
        let r = self.lang_uncached(path);
        self.lang_memo.borrow_mut().insert(path, r);
        r
    }

    fn lang_uncached(&self, path: &str) -> Option<DefId> {
        let prog = self.prog;
        let core = prog.syms.get("core")?;
        let mut cur = None;
        for c in 0..prog.crates.len() {
            if prog.crates[c].name == core {
                cur = Some(prog.mods[prog.crates[c].root_mod as usize].def);
            }
        }
        let mut cur = cur?;
        for p in path.split("::") {
            let s = prog.syms.get(p)?;
            cur = match prog.lookup_in_container(cur, s, true) {
                Some(d) => d,
                None => prog.lookup_in_container(cur, s, false)?,
            };
        }
        Some(cur)
    }

    pub fn ref_ty(&self, m: bool, t: TyId) -> TyId {
        self.tcx.tys.intern(TyKind::Ref(m, t, crate::types::RegionId::ERASED))
    }

    pub fn unit(&self) -> TyId {
        self.tcx.tys.unit
    }

    pub fn box_inner(&self, t: TyId) -> Option<TyId> {
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, a, _) if Some(*d) == self.box_def && !a.is_empty() => Some(self.tcx.tys.list(*a)[0]),
            _ => None,
        }
    }

    /// The pointee of a builtin deref (references, raw pointers, Box).
    pub fn builtin_deref(&self, t: TyId) -> Option<TyId> {
        match self.tcx.tys.kind(t) {
            TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) => Some(*i),
            _ => self.box_inner(t),
        }
    }

    pub fn adt_name(&self, d: DefId) -> &str {
        self.prog.name(d)
    }

    /// Field `fi` of variant `vi` of an ADT (or tuple element `fi`), substituted.
    pub fn field_ty(&self, t: TyId, vi: u32, fi: u32) -> Option<TyId> {
        match self.tcx.tys.kind(t) {
            TyKind::Tuple(v) => self.tcx.tys.list(*v).get(fi as usize).copied(),
            TyKind::Adt(d, args, _) => {
                let adt = self.tcx.adts.get(d)?;
                adt.variants.get(vi as usize)?.fields.get(fi as usize)?;
                Some(self.tcx.field_ty(self.prog, self.cx.typing_env, *d, vi, fi, self.tcx.tys.list(*args)))
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.field_ty(*up, 0, fi),
            _ => None,
        }
    }

    /// Whether a value of `t` holds a raw pointer (`*T`, or a struct around one: NonNull,
    /// Vec's buffer, Rc, ...) inline or inside a Box: memory reached through it is not part of
    /// any place the checker sees, so a field view must treat such a field as the whole value.
    /// References end the walk (what they point to is a place of its own).
    pub fn holds_raw_ptr(&self, t: TyId) -> bool {
        self.holds_raw_ptr_d(t, 0)
    }

    /// Whether every raw pointer in a value of `t` sits inside a std type that owns the memory
    /// it points to (Vec<u32>, String, Box<Vec<T>>, Rc<RefCell<T>>, user structs of those): a
    /// `*mut T` field of a user type (pointing anywhere) is not.
    pub fn owning(&self, t: TyId) -> bool {
        self.owning_d(t, 0)
    }

    fn owning_d(&self, t: TyId, depth: u32) -> bool {
        if depth > 12 {
            return false;
        }
        if !self.holds_raw_ptr(t) {
            return true;
        }
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, args, _) if self.is_std_def(*d) => self.tcx.tys.list(*args).iter().all(|a| self.owning_d(*a, depth + 1)),
            TyKind::Adt(d, _, _) => {
                let nv = self.tcx.adts.get(d).map_or(0, |a| a.variants.len());
                (0..nv as u32).all(|vi| (0..self.n_fields(t, vi) as u32).all(|fi| self.field_ty(t, vi, fi).map_or(true, |ft| self.owning_d(ft, depth + 1))))
            }
            TyKind::Tuple(v) => self.tcx.tys.list(*v).iter().all(|a| self.owning_d(*a, depth + 1)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.owning_d(*e, depth + 1),
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.owning_d(*up, depth + 1),
            _ => false,
        }
    }

    fn holds_raw_ptr_d(&self, t: TyId, depth: u32) -> bool {
        if depth > 12 {
            return true;
        }
        if let Some(b) = self.rawptr_memo.borrow().get(&t) {
            return *b;
        }
        // cycles (a recursive type through Box) answer false while being computed
        self.rawptr_memo.borrow_mut().insert(t, false);
        let r = match self.tcx.tys.kind(t) {
            TyKind::Ptr(..) => true,
            TyKind::Tuple(v) => self.tcx.tys.list(*v).iter().any(|x| self.holds_raw_ptr_d(*x, depth + 1)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.holds_raw_ptr_d(*e, depth + 1),
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.holds_raw_ptr_d(*up, depth + 1),
            TyKind::Adt(d, _, _) => {
                if let Some(inner) = self.box_inner(t) {
                    self.holds_raw_ptr_d(inner, depth + 1)
                } else {
                    let nv = self.tcx.adts.get(d).map_or(0, |a| a.variants.len());
                    (0..nv as u32).any(|vi| (0..self.n_fields(t, vi) as u32).any(|fi| self.field_ty(t, vi, fi).map_or(false, |ft| self.holds_raw_ptr_d(ft, depth + 1))))
                }
            }
            _ => false,
        };
        self.rawptr_memo.borrow_mut().insert(t, r);
        r
    }

    pub fn n_fields(&self, t: TyId, vi: u32) -> usize {
        match self.tcx.tys.kind(t) {
            TyKind::Tuple(v) => self.tcx.tys.list(*v).len(),
            TyKind::Adt(d, _, _) => self.tcx.adts.get(d).and_then(|a| a.variants.get(vi as usize)).map_or(0, |v| v.fields.len()),
            _ => 0,
        }
    }

    pub fn field_name(&self, t: TyId, vi: u32, fi: u32) -> String {
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, _, _) => match self.tcx.adts.get(d).and_then(|a| a.variants.get(vi as usize)).and_then(|v| v.fields.get(fi as usize)) {
                Some(f) => self.prog.syms.str(f.name).to_string(),
                None => fi.to_string(),
            },
            _ => fi.to_string(),
        }
    }

    pub fn is_enum(&self, t: TyId) -> bool {
        matches!(self.tcx.tys.kind(t), TyKind::Adt(d, _, _) if self.tcx.adts.get(d).map_or(false, |a| a.is_enum))
    }

    pub fn is_union(&self, t: TyId) -> bool {
        matches!(self.tcx.tys.kind(t), TyKind::Adt(d, _, _) if self.tcx.adts.get(d).map_or(false, |a| a.is_union))
    }

    pub fn is_packed(&self, t: TyId) -> bool {
        matches!(self.tcx.tys.kind(t), TyKind::Adt(d, _, _) if self.tcx.adts.get(d).map_or(false, |a| a.repr_packed != 0))
    }

    /// `ty: td<args>` in the body's environment, lifetimes erased: borrowck proves trait
    /// properties modulo regions (rustc type_is_copy_modulo_regions).
    fn trait_goal(&self, td: DefId, ty: TyId, args: &[TyId]) -> crate::next_solver::Goal {
        let tys = &self.tcx.tys;
        let mut compact = Vec::with_capacity(args.len() + 1);
        compact.push(tys.erase_regions(ty));
        for &a in args { compact.push(tys.erase_regions(a)); }
        let lifetimes = self.tcx.generic_indices_of(self.prog, td).lifetimes.len();
        let regions = tys.intern_region_args(&vec![crate::types::RegionId::ERASED; lifetimes]);
        let args = self.tcx.complete_args(self.prog, td, &compact, regions);
        crate::next_solver::Goal { param_env: self.param_env_id.get(), predicate: crate::next_solver::collect::trait_predicate(td, args) }
    }

    /// Borrowck retains undecided properties as obligations in its explicit environment.
    fn retain_ambiguous(&self, goal: crate::next_solver::Goal) {
        let owner = self.obligation_owner.get();
        let mut pending = self.pending_traits.borrow_mut();
        if !pending.iter().any(|(g, d)| *g == goal && *d == owner) { pending.push((goal, owner)); }
    }

    /// rustc_trait_selection/src/traits/query/evaluate_obligation.rs:95-114.
    pub fn evaluate_trait(&self, td: DefId, ty: TyId, args: &[TyId]) -> EvaluationResult {
        let goal = self.trait_goal(td, ty, args);
        let result = match self.cx.infcx {
            Some(infcx) => infcx.evaluate_obligation(self.prog, goal.clone()),
            None => self.infer_ctxt().evaluate_obligation(self.prog, goal.clone()),
        };
        if result == EvaluationResult::EvaluatedToAmbig { self.retain_ambiguous(goal); }
        result
    }

    /// The user-written impl proving `ty: td<args>` (rustc_traits/src/codegen.rs:26-50
    /// selection in the body's environment); None for builtin, object and where-clause
    /// sources, ambiguity and failure.
    pub fn select_impl(&self, td: DefId, ty: TyId, args: &[TyId]) -> Option<DefId> {
        let goal = self.trait_goal(td, ty, args);
        let infcx = self.infer_ctxt();
        match infcx.select_in_new_trait_solver(self.prog, goal.clone()) {
            Ok(Some(ImplSource::UserDefined { impl_def_id, .. })) => Some(impl_def_id),
            Ok(Some(_)) | Err(_) => None,
            Ok(None) => { self.retain_ambiguous(goal); None }
        }
    }

    /// The active body's interned clauses, including set_generic_env/reset_env transitions.
    pub fn typing_env(&self) -> crate::tcx::TypingEnv {
        crate::tcx::TypingEnv { param_env: self.param_env_id.get(), ..self.cx.typing_env }
    }

    /// rustc_ty_utils/src/instance.rs:108-131: an unresolvable call (a where-clause
    /// source, ambiguity) stays on the trait item, as generic MIR keeps it.
    pub fn resolve_trait_method(&self, def: DefId, args: &[TyId]) -> (DefId, Vec<TyId>) {
        let typing_env = self.typing_env();
        match self.tcx.resolve_instance_raw(self.prog, typing_env, def, args) {
            Ok(instance) => match instance.def {
                crate::instance::InstanceKind::Item(d) if self.prog.def(d).kind != crate::program::DefKind::Closure => (d, instance.compact_args(self.tcx)),
                _ => (def, args.to_vec()),
            },
            // ty_utils/instance.rs resolve_associated_item: an ambiguous selection is
            // Ok(None) (too generic: the trait item stays); Unimplemented/UnconstrainedParam
            // are a delayed bug (typeck reported the unsatisfied obligation)
            Err(crate::next_solver::codegen::CodegenObligationError::Ambiguity) => (def, args.to_vec()),
            Err(e) => {
                self.errors.delayed_bug(crate::next_solver::fulfill::ObligationCause { span: 0, body_id: self.obligation_owner.get() },
                    format!("resolving {} failed: {:?}", self.prog.def_path(def), e));
                (def, args.to_vec())
            }
        }
    }

    /// rustc type_is_copy_modulo_regions: only a proven Copy goal permits a copy.
    pub fn is_copy(&self, t: TyId) -> bool {
        if let Some(value) = self.copy_memo.borrow().get(&t) { return *value; }
        let result = match self.copy_trait {
            Some(td) => self.evaluate_trait(td, t, &[]),
            None => return matches!(self.kind(t), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ref(false, _, _) | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::FnDef(..)),
        };
        if result == EvaluationResult::EvaluatedToAmbig { return false; }
        let value = result.must_apply_modulo_regions();
        self.copy_memo.borrow_mut().insert(t, value);
        value
    }


    pub fn has_drop_impl(&self, t: TyId) -> bool {
        matches!(self.tcx.tys.kind(t), TyKind::Adt(d, ..) if self.tcx.adt_destructor(*d).is_some())
    }

    pub fn needs_drop(&self, t: TyId) -> bool {
        self.tcx.needs_drop(self.prog, self.cx.typing_env, t)
    }

    /// Dynamically sized: str, slices, dyn, and structs / tuples ending in one.
    pub fn is_unsized(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => true,
            TyKind::Tuple(v) => self.tcx.tys.list(v).last().map_or(false, |x| self.is_unsized(*x)),
            TyKind::Adt(d, _, _) => match self.tcx.adts.get(&d) {
                Some(a) if !a.is_enum && !a.is_union => {
                    let n = self.n_fields(t, 0);
                    n > 0 && self.field_ty(t, 0, n as u32 - 1).map_or(false, |ft| ft != t && self.is_unsized(ft))
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub fn ty_str(&self, t: TyId) -> String {
        crate::typeck::ty_to_string(false, self.prog, self.tcx, t)
    }

    pub fn def_kind(&self, d: DefId) -> DefKind {
        self.prog.def(d).kind
    }

    pub fn crate_of(&self, d: DefId) -> u32 {
        self.prog.def(d).krate
    }
}

/// Region variables of another inference context (typeck's, which writeback keeps) cannot
/// enter `infcx`: each is replaced by a variable of `infcx` for the operation and restored
/// in its result. A variable the operation itself created is erased (a fresh slot).
pub fn with_foreign_region_vars(infcx: &crate::infer::InferCtxt<'_>, ty: TyId, f: impl FnOnce(TyId) -> Option<TyId>) -> Option<TyId> {
    use crate::types::{RegionId, RegionKind};
    let tys = &infcx.tcx.tys;
    let map = RefCell::new(Vec::<(RegionId, RegionId)>::new());
    let local = if tys.flags(ty).contains(crate::types::TypeFlags::HAS_RE_INFER) {
        tys.map_regions(ty, &|r, _| {
            if !matches!(tys.region_kind(r), RegionKind::ReVar(_)) { return r; }
            if let Some(&(_, l)) = map.borrow().iter().find(|(f, _)| *f == r) { return l; }
            let l = infcx.next_region_var();
            map.borrow_mut().push((r, l));
            l
        }, 0)
    } else { ty };
    let out = f(local)?;
    let map = map.into_inner();
    Some(tys.map_regions(out, &|r, _| match tys.region_kind(r) {
        RegionKind::ReVar(_) => map.iter().find(|(_, l)| *l == r).map_or(RegionId::ERASED, |(f, _)| *f),
        _ => r,
    }, 0))
}

/// A type taken out of a signature can still mention the signature's late-bound regions.
/// It is normalized under that binder (nightly normalizes the whole `PolyFnSig`): wrapped
/// as the single input of a function-pointer binder and unwrapped afterwards.
pub fn in_signature_binder(tys: &crate::types::Types, ty: TyId, f: impl FnOnce(TyId) -> Option<TyId>) -> Option<TyId> {
    if !tys.has_vars_bound_at_or_above(ty, 0) { return f(ty); }
    let bound = std::cell::Cell::new(0u32);
    let outer = std::cell::Cell::new(false);
    tys.map_regions(ty, &|r, depth| {
        if let crate::types::RegionKind::ReBound(d, v) = tys.region_kind(r) {
            if d == depth { bound.set(bound.get().max(v.0 + 1)); } else if d > depth { outer.set(true); }
        }
        r
    }, 0);
    // Escaping bound types/consts or regions of an outer binder: not a signature's own binder.
    if outer.get() || bound.get() == 0 { return None; }
    let wrapped = tys.intern(TyKind::FnPtr(tys.mk_list(&[ty]), tys.unit, crate::types::FnHeader::RUST, bound.get()));
    match tys.kind(f(wrapped)?) { TyKind::FnPtr(ps, ..) => tys.list(*ps).first().copied(), _ => None }
}
