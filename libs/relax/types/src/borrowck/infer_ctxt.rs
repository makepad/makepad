// ported from N:rustc_borrowck/src/lib.rs @8d1a764 (BorrowckInferCtxt, :651-736)
use super::constraints::RegionVid;
use super::region_infer::NllRegionVariableOrigin;
use super::renumber::RegionCtxt;
use crate::fxhash::HashMap;
use crate::infer::InferCtxt;
use crate::next_solver::type_op::{ParamEnvAnd, TypeOp, TypeOpOutput};
use crate::next_solver::{NoSolution, ParamEnv};
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingMode};
use crate::types::{RegionId, RegionKind, TyId, TyKind};
use std::cell::RefCell;

pub struct BorrowckInferCtxt<'tcx> {
    pub infcx: InferCtxt<'tcx>,
    pub prog: &'tcx Program,
    pub root_def_id: DefId,
    pub param_env: ParamEnv,
    param_env_id: std::cell::OnceCell<crate::tcx::ParamEnvId>,
    pub tainted_by_errors: std::cell::Cell<bool>,
    encountered_re_error: std::cell::Cell<bool>,
    pub universal_regions: Option<super::universal_regions::UniversalRegions>,
    pub region_bound_pairs: Vec<(TyId, RegionId)>,
    pub mir_body: Option<&'tcx crate::mir::Body>,
    pub reg_var_to_origin: RefCell<HashMap<RegionVid, RegionCtxt>>,
}

impl<'tcx> BorrowckInferCtxt<'tcx> {
    /// rustc_borrowck/src/lib.rs:666: the body always carries its declaration environment,
    /// including bodies whose only parameters are regions or which have no parameters.
    pub fn new(tcx: &'tcx Tcx, prog: &'tcx Program, def_id: DefId, root_def_id: DefId, args: &[TyId]) -> Self {
        let typing_mode = TypingMode::borrowck(tcx, prog, def_id);
        let infcx = InferCtxt::for_item(tcx, prog, typing_mode, root_def_id);
        let result = tcx.param_env(prog, def_id);
        let indices = tcx.generic_indices_of(prog, def_id);
        let mut full = indices.identity_args(&tcx.tys);
        for (i, &ty) in args.iter().enumerate() {
            full[indices.types_consts[i] as usize] = match tcx.tys.kind(ty) {
                TyKind::Const(ct) => crate::consts::GenericArg::Const(*ct),
                _ => crate::consts::GenericArg::Type(ty),
            };
        }
        let mut clauses = Vec::new();
        for p in &*result.clauses { clauses.push(tcx.instantiate_predicate(prog, def_id, &full, p)); }
        let param_env = ParamEnv { clauses: clauses.into() };
        let param_env_id = std::cell::OnceCell::new();
        BorrowckInferCtxt { infcx, prog, root_def_id, param_env, param_env_id, tainted_by_errors: std::cell::Cell::new(false), encountered_re_error: std::cell::Cell::new(false), universal_regions: None, region_bound_pairs: Vec::new(), mir_body: None, reg_var_to_origin: RefCell::new(HashMap::default()) }
    }

    pub(crate) fn param_env_id(&self) -> crate::tcx::ParamEnvId {
        *self.param_env_id.get_or_init(|| self.tcx.intern_param_env(self.param_env.clone()))
    }

    pub fn next_region_var(&self, get_ctxt_fn: impl Fn() -> RegionCtxt) -> RegionId {
        let next_region = self.infcx.next_region_var();
        self.record_origin(next_region, get_ctxt_fn);
        next_region
    }

    pub fn next_nll_region_var(&self, origin: NllRegionVariableOrigin, get_ctxt_fn: impl Fn() -> RegionCtxt) -> RegionId {
        let next_region = self.infcx.next_nll_region_var(origin);
        self.record_origin(next_region, get_ctxt_fn);
        next_region
    }

    fn record_origin(&self, region: RegionId, get_ctxt_fn: impl Fn() -> RegionCtxt) {
        if cfg!(debug_assertions) {
            let RegionKind::ReVar(vid) = self.infcx.tcx.tys.region_kind(region) else { unreachable!() };
            assert!(self.reg_var_to_origin.borrow_mut().insert(vid as RegionVid, get_ctxt_fn()).is_none());
        }
    }

    pub fn fully_perform<Q>(&self, q: Q) -> Result<TypeOpOutput<<ParamEnvAnd<Q> as TypeOp>::Output>, NoSolution>
    where ParamEnvAnd<Q>: TypeOp {
        ParamEnvAnd { param_env: self.param_env_id(), value: q }.fully_perform(&self.infcx, self.prog)
    }
}

impl<'tcx> std::ops::Deref for BorrowckInferCtxt<'tcx> {
    type Target = InferCtxt<'tcx>;
    fn deref(&self) -> &InferCtxt<'tcx> { &self.infcx }
}

use super::type_check::{BorrowckTypeOps, ClauseOwner, FnSig, PolyFnSig, Locations, MirTypeckRegionConstraints, UserType};
use super::constraints::ConstraintCategory;
use crate::consts::GenericArg;
use crate::mir::{self, Location, Span, Body, Place, PlaceTy, FieldIdx, UserTypeId, AggregateKind, Rvalue, ConstOperand};
use crate::lang_items::LangItem;
use crate::types::{UniverseIndex, AliasTyKind};
use crate::types::variance::V;
use crate::next_solver::{Goal, Predicate, PredicateKind, ClauseKind, Term};
use crate::next_solver::type_op::{Normalize, ProvePredicate, CustomTypeOp};

impl<'tcx> BorrowckInferCtxt<'tcx> {
    /// type_check/mod.rs: the driver supplies the renumbered body's universal
    /// region map and implied bounds before location-aware operations begin.
    pub fn configure_mir(&mut self, body: &'tcx Body, universals: super::universal_regions::UniversalRegions, region_bound_pairs: Vec<(TyId, RegionId)>) {
        self.mir_body = Some(body);
        self.universal_regions = Some(universals);
        self.region_bound_pairs = region_bound_pairs;
    }

    pub(crate) fn location_point(&self, at: Location) -> usize {
        let body = self.mir_body.expect("MIR type ops require configure_mir");
        let mut offset = 0;
        for (b, block) in body.basic_blocks.iter().enumerate() {
            if b == at.block.0 as usize { assert!(at.statement_index <= block.statements.len()); return offset + at.statement_index; }
            offset += block.statements.len() + 1;
        }
        panic!("MIR location outside body")
    }

    fn fully_perform_op<Op: TypeOp>(&self, out: &mut MirTypeckRegionConstraints, locations: Locations, category: ConstraintCategory, op: Op) -> Result<Op::Output, NoSolution> {
        let span = match locations { Locations::All(s) => s, Locations::Single(_) => locations.span(self.mir_body.expect("MIR type ops require configure_mir")) };
        let before = self.infcx.universe();
        let result = super::type_check::canonical::fully_perform_op_raw(self,
            self.universal_regions.as_ref().expect("MIR type ops require universal regions"), &self.region_bound_pairs,
            &self.param_env.clauses, out, locations, span, category, op);
        if result.is_err() { self.tainted_by_errors.set(true); }
        for u in before.0 + 1..=self.infcx.universe().0 { out.universe_causes.push((UniverseIndex(u), super::type_check::UniverseInfo::TypeOp)); }
        result
    }

    fn clause_owner(&self, owner: ClauseOwner) -> DefId {
        match owner {
            ClauseOwner::Def(def) => def,
            ClauseOwner::Closure(id) | ClauseOwner::Coroutine(id) => {
                use relax_front::def_collector::NodeId;
                if id.expr & crate::tcx::ASYNC_FN_BODY == 0 {
                    return *self.prog.node_id_to_def_id.get(&(id.file, NodeId::Closure(crate::ast::ExprId(id.expr)))).expect("closure definition");
                }
                for (&(file, node), &def) in &self.prog.node_id_to_def_id {
                    if file != id.file { continue; }
                    if let NodeId::AsyncBody(item) = node {
                        if self.prog.fn_body(file, item).expect("async body").is_some_and(|b| b.0 == id.expr & !crate::tcx::ASYNC_FN_BODY) { return def; }
                    }
                }
                panic!("async body definition")
            },
        }
    }

    fn nominal_ty(&self, def: DefId, args: &[GenericArg]) -> TyId {
        let mut compact = Vec::new(); let mut regions = Vec::new();
        for arg in args { match *arg {
            GenericArg::Type(ty) => compact.push(ty), GenericArg::Const(c) => compact.push(self.tcx.tys.intern(TyKind::Const(c))), GenericArg::Lifetime(r) => regions.push(r),
        } }
        self.tcx.tys.intern(TyKind::Adt(def, self.tcx.tys.mk_list(&compact), self.tcx.tys.intern_region_args(&regions)))
    }

    /// ty_kind/closure.rs: ClosureArgs consists of its parent's args and the
    /// synthetic kind, signature and tupled-upvars types, in that order.
    fn closure_ty(&self, id: mir::ClosureId, args: &[GenericArg]) -> TyId {
        let n = args.len().checked_sub(3).expect("closure synthetic arguments");
        let type_arg = |i| match args[i] { GenericArg::Type(t) => t, _ => panic!("closure synthetic argument is not a type") };
        let mut parent = Vec::new(); let mut regions = Vec::new();
        for arg in &args[..n] { match *arg { GenericArg::Type(t) => parent.push(t), GenericArg::Const(c) => parent.push(self.tcx.tys.intern(TyKind::Const(c))), GenericArg::Lifetime(r) => regions.push(r) } }
        let mut root = self.clause_owner(ClauseOwner::Closure(id));
        while matches!(self.prog.def(root).kind, crate::program::DefKind::Closure | crate::program::DefKind::OpaqueTy) || BorrowckTypeOps::is_inline_const(self, root) { root = self.prog.def(root).parent; }
        let owner = self.tcx.tys.intern(TyKind::FnDef(root, self.tcx.tys.mk_list(&parent), self.tcx.tys.intern_region_args(&regions)));
        let closure_args = crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: type_arg(n), closure_sig_as_fn_ptr_ty: type_arg(n + 1), tupled_upvars_ty: type_arg(n + 2) };
        self.tcx.tys.intern(TyKind::Closure(id.file, id.expr, closure_args))
    }

    fn operand_ty(&self, body: &Body, operand: &mir::Operand) -> TyId {
        match operand {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(body, self.tcx).expect("typed MIR place").ty,
            mir::Operand::Constant(c) => match &c.const_ {
                mir::Const::Val(_, ty) => *ty,
                mir::Const::Unevaluated { def, args, .. } => BorrowckTypeOps::unevaluated_const_ty(self, *def, args),
            },
        }
    }
}

impl BorrowckTypeOps for BorrowckInferCtxt<'_> {
    fn universe(&self) -> UniverseIndex { self.infcx.universe() }
    fn create_next_universe(&mut self) -> UniverseIndex { self.infcx.create_next_universe() }
    fn next_nll_region_var(&mut self, origin: NllRegionVariableOrigin, universe: UniverseIndex) -> RegionId { self.infcx.next_nll_region_var_in_universe(origin, universe) }
    fn to_region_vid(&self, r: RegionId) -> RegionVid {
        if let RegionKind::ReVar(v) = self.tcx.tys.region_kind(r) { return v as RegionVid; }
        if self.tcx.tys.region_kind(r) == RegionKind::ReError { self.encountered_re_error.set(true); }
        self.universal_regions.as_ref().expect("MIR universal regions").to_region_vid(&self.tcx.tys, r)
    }
    fn encountered_re_error(&self) -> bool { self.encountered_re_error.get() }
    fn shallow_resolve(&mut self, ty: TyId) -> TyId { self.infcx.shallow_resolve(ty) }
    fn shallow_resolve_const(&mut self, ct: crate::consts::Const) -> crate::consts::Const { self.infcx.shallow_resolve_const(ct) }
    fn next_ty_var(&mut self, _: Span) -> TyId { self.infcx.next_ty_var() }
    fn generalize_ty_var(&mut self, vid: u32, variance: V, source: TyId, _: Span) -> Result<TyId, NoSolution> {
        self.infcx.generalize_ty_var(self.prog, vid, variance, source).map_err(|_| NoSolution)
    }
    fn relate_opaque(&mut self, out: &mut MirTypeckRegionConstraints, a: TyId, variance: V, b: TyId, locations: Locations, category: ConstraintCategory) -> Result<(), NoSolution> {
        let env = self.param_env_id();
        self.fully_perform_op(out, locations, category, CustomTypeOp::new(|ocx: &mut crate::next_solver::fulfill::ObligationCtxt<'_, '_>| {
            let goals = ocx.infcx.at(ocx.prog, env).relate(a, variance, b).map_err(|_| NoSolution)?;
            ocx.register_obligations(goals); Ok(())
        }))
    }
    fn prove_trait_ref(&mut self, out: &mut MirTypeckRegionConstraints, trait_: LangItem, args: &[GenericArg], locations: Locations, category: ConstraintCategory) {
        let def = self.tcx.lang_items.get(trait_).expect("required trait lang item");
        let op = ProvePredicate { predicate: crate::next_solver::collect::trait_predicate(def, args.to_vec()) };
        let _ = self.fully_perform_op(out, locations, category, ParamEnvAnd { param_env: self.param_env_id(), value: op });
    }
    fn prove_wf(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, locations: Locations, category: ConstraintCategory) {
        let op = ProvePredicate { predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty)))) };
        let _ = self.fully_perform_op(out, locations, category, ParamEnvAnd { param_env: self.param_env_id(), value: op });
    }
    fn normalize_and_prove_instantiated_clauses(&mut self, out: &mut MirTypeckRegionConstraints, owner: ClauseOwner, args: &[GenericArg], locations: Locations) {
        let def = self.clause_owner(owner); let env = self.param_env_id();
        let _ = self.fully_perform_op(out, locations, ConstraintCategory::Predicate(locations.span(self.mir_body.expect("MIR type ops require configure_mir")), def), CustomTypeOp::new(|ocx: &mut crate::next_solver::fulfill::ObligationCtxt<'_, '_>| {
            for p in ocx.infcx.tcx.clauses_of(ocx.prog, def) {
                let p = ocx.infcx.tcx.instantiate_predicate(ocx.prog, def, args, &p);
                let predicate = ocx.normalize_predicate(&env, &p);
                ocx.register_obligation(Goal { param_env: env.clone(), predicate });
            }
            Ok(())
        }));
    }
    fn normalize(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, locations: Locations) -> TyId {
        self.fully_perform_op(out, locations, ConstraintCategory::Boring, ParamEnvAnd { param_env: self.param_env_id(), value: Normalize { value: ty } }).unwrap_or(ty)
    }
    fn deeply_normalize_sig(&mut self, out: &mut MirTypeckRegionConstraints, sig: &FnSig, at: Location) -> Result<FnSig, ()> {
        let value = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&sig.inputs), sig.output, sig.header, 0));
        let ty = self.fully_perform_op(out, Locations::Single(at), ConstraintCategory::Boring, ParamEnvAnd { param_env: self.param_env_id(), value: Normalize { value } }).map_err(|_| ())?;
        let TyKind::FnPtr(inputs, output, header, _) = self.tcx.tys.kind(ty) else { unreachable!() };
        Ok(FnSig { inputs: self.tcx.tys.list(*inputs).to_vec(), output: *output, header: *header })
    }
    /// rustc_middle/src/ty/util.rs:256-311 struct_tail_raw (pattern types are feature-gated):
    /// the crate's recursion limit, and its error when reached.
    fn struct_tail(&mut self, out: &mut MirTypeckRegionConstraints, mut ty: TyId, at: Location) -> TyId {
        let limit = self.recursion_limit(self.prog);
        // Limit::value_within_limit: iterations 0..=limit
        for _ in 0..=limit {
            match self.tcx.tys.kind(ty) {
                TyKind::Adt(def, args, rs) if self.prog.def(*def).kind == crate::program::DefKind::Struct => {
                    let Some(field) = self.tcx.adts[def].variants[0].fields.last() else { return ty; };
                    let args = self.tcx.complete_args(self.prog, *def, self.tcx.tys.list(*args), *rs);
                    let tail = self.tcx.instantiate_ty(self.prog, *def, &args, field.ty);
                    ty = BorrowckTypeOps::normalize(self, out, tail, Locations::Single(at));
                }
                TyKind::Tuple(ts) if !ts.is_empty() => ty = *self.tcx.tys.list(*ts).last().unwrap(),
                _ => return ty,
            }
        }
        let suggested = if limit == 0 { 2 } else { limit * 2 };
        self.errors.report(self.cause, format!("reached the recursion limit finding the struct tail for `{}` (consider increasing the recursion limit by adding a `#![recursion_limit = \"{}\"]`)",
            crate::typeck::ty_to_string(false, self.prog, self.tcx, ty), suggested));
        self.tainted_by_errors.set(true); self.tcx.tys.error
    }
    fn ascribe_user_type(&mut self, _: &mut MirTypeckRegionConstraints, _: TyId, _: UserType, _: Span) { panic!("M2b: canonical user-type annotations on MIR Body") }
    fn ascribe_user_type_skip_wf(&mut self, out: &mut MirTypeckRegionConstraints, inferred: TyId, user: TyId, span: Span) {
        let env = self.param_env_id();
        let _ = self.fully_perform_op(out, Locations::All(span), ConstraintCategory::Boring, CustomTypeOp::new(|ocx: &mut crate::next_solver::fulfill::ObligationCtxt<'_, '_>| {
            let user = ocx.normalize_ty(&env, user); ocx.eq(&env, user, inferred)
        }));
    }
    fn user_type_annotation(&self, _: UserTypeId) -> (Span, TyId) { panic!("M2b: canonical user-type annotations on MIR Body") }
    fn closure_user_provided_sig(&mut self, _: &Body) -> Option<FnSig> { panic!("M2b: canonical closure user-provided signatures") }
    fn fn_def_sig(&self, def: DefId, args: &[TyId], regions: crate::types::RegionArgsId) -> PolyFnSig {
        let args = self.tcx.complete_args(self.prog, def, args, regions);
        let sig = self.tcx.fn_item_sig(self.prog, def).expect("FnDef signature");
        let ty = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&sig.params), sig.ret, sig.header, sig.bound_regions));
        let ty = self.tcx.instantiate_ty(self.prog, def, &args, ty);
        let TyKind::FnPtr(inputs, output, header, bound_regions) = self.tcx.tys.kind(ty) else { unreachable!() };
        PolyFnSig { sig: FnSig { inputs: self.tcx.tys.list(*inputs).to_vec(), output: *output, header: *header }, bound_regions: *bound_regions }
    }
    fn field_ty(&self, base: PlaceTy, field: FieldIdx) -> Option<TyId> {
        match self.tcx.tys.kind(base.ty) {
            TyKind::Adt(def, args, rs) => {
                let adt = self.tcx.adts.get(def)?;
                let v = if adt.is_enum() { base.variant_index?.0 as usize } else { 0 };
                let field = adt.variants.get(v)?.fields.get(field.0 as usize)?;
                Some(self.tcx.instantiate_ty(self.prog, *def, &self.tcx.complete_args(self.prog, *def, self.tcx.tys.list(*args), *rs), field.ty))
            }
            TyKind::Tuple(ts) => self.tcx.tys.list(*ts).get(field.0 as usize).copied(),
            TyKind::Closure(_, _, args) => match self.tcx.tys.kind(args.tupled_upvars_ty) { TyKind::Tuple(ts) => self.tcx.tys.list(*ts).get(field.0 as usize).copied(), _ => None },
            _ => None,
        }
    }
    fn aggregate_field_ty(&mut self, out: &mut MirTypeckRegionConstraints, kind: &AggregateKind, field: FieldIdx, at: Location) -> Result<TyId, usize> {
        match kind {
            AggregateKind::Adt(def, variant, args, _, active) => {
                let fields = &self.tcx.adts[def].variants[variant.0 as usize].fields;
                let field = fields.get(active.unwrap_or(field).0 as usize).ok_or(fields.len())?;
                let ty = self.tcx.instantiate_ty(self.prog, *def, args, field.ty);
                Ok(BorrowckTypeOps::normalize(self, out, ty, Locations::Single(at)))
            }
            AggregateKind::Closure(id, args) | AggregateKind::Coroutine(id, args) => {
                let ty = self.closure_ty(*id, args);
                let TyKind::Closure(_, _, args) = self.tcx.tys.kind(ty) else { unreachable!() };
                let TyKind::Tuple(ts) = self.tcx.tys.kind(args.tupled_upvars_ty) else { panic!("resolved upvars") };
                self.tcx.tys.list(*ts).get(field.0 as usize).copied().ok_or(self.tcx.tys.list(*ts).len())
            }
            AggregateKind::Array(ty) => Ok(*ty),
            AggregateKind::Tuple | AggregateKind::RawPtr(..) => unreachable!("checked without an aggregate-field query"),
        }
    }
    // rustc_middle/src/mir/statement.rs:821-877, ty/sty.rs:1710-1862.
    fn rvalue_query_ty(&self, body: &Body, rvalue: &Rvalue) -> TyId {
        let tys = &self.tcx.tys;
        match rvalue {
            Rvalue::ThreadLocalRef(def) => {
                let d = self.prog.def(*def);
                let crate::ast::ItemKind::Static(mutable, _, _) = self.prog.files[d.file as usize].ast.item(d.item).kind else { unreachable!("static declaration") };
                let ty = self.tcx.const_ty(self.prog, *def).expect("static type");
                if mutable { tys.intern(TyKind::Ptr(true, ty)) }
                else if d.kind == crate::program::DefKind::ForeignStatic { tys.intern(TyKind::Ptr(false, ty)) }
                else { tys.intern(TyKind::Ref(false, ty, tys.intern_region(RegionKind::ReStatic))) }
            },
            Rvalue::Aggregate(kind, _) => match &**kind {
                AggregateKind::Adt(def, _, args, ..) => self.nominal_ty(*def, args),
                AggregateKind::Closure(id, args) | AggregateKind::Coroutine(id, args) => self.closure_ty(*id, args),
                _ => unreachable!("structural aggregate does not require a query"),
            },
            Rvalue::Discriminant(place) => {
                let ty = place.ty(body, self.tcx).expect("typed discriminant place").ty;
                match tys.kind(ty) {
                    TyKind::Adt(def, ..) if self.tcx.adts[def].is_enum() => tys.intern(TyKind::Int(self.tcx.adts[def].repr_int.unwrap_or(crate::program::Prim::Isize))),
                    TyKind::Closure(..) if self.tcx.is_coroutine(ty) => tys.u32_,
                    TyKind::Param(_) | TyKind::Alias(..) | TyKind::Infer(_) => {
                        let Some(ad) = self.require_lang_item(LangItem::Discriminant) else { return tys.error; };
                        self.tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: ad }, &[GenericArg::Type(ty)])
                    }
                    _ => tys.u8_,
                }
            }
            Rvalue::UnaryOp(mir::UnOp::PtrMetadata, operand) => {
                let pointer = self.operand_ty(body, operand);
                let mut tail = match tys.kind(pointer) { TyKind::Ref(_, t, _) | TyKind::Ptr(_, t) => *t, _ => panic!("metadata of non-pointer") };
                let mut depth = 0;
                loop {
                    // sty.rs:1846-1861 -> util.rs:266-279 struct_tail_raw's limit and error
                    let limit = self.recursion_limit(self.prog);
                    if depth > limit {
                        self.errors.report(self.cause, format!("reached the recursion limit finding the struct tail for `{}` (consider increasing the recursion limit by adding a `#![recursion_limit = \"{}\"]`)",
                            crate::typeck::ty_to_string(false, self.prog, self.tcx, tail), if limit == 0 { 2 } else { limit * 2 }));
                        return tys.error;
                    }
                    depth += 1;
                    match tys.kind(tail) {
                        TyKind::Adt(def, args, rs) if self.prog.def(*def).kind == crate::program::DefKind::Struct => {
                            let Some(field) = self.tcx.adts[def].variants[0].fields.last() else { break; };
                            tail = self.tcx.instantiate_ty(self.prog, *def, &self.tcx.complete_args(self.prog, *def, tys.list(*args), *rs), field.ty);
                        }
                        TyKind::Tuple(ts) if !ts.is_empty() => tail = *tys.list(*ts).last().unwrap(),
                        _ => break,
                    }
                }
                match tys.kind(tail) {
                    TyKind::Str | TyKind::Slice(_) => tys.usize_,
                    TyKind::Dyn(..) => self.nominal_ty(match self.require_lang_item(LangItem::DynMetadata) { Some(def) => def, None => return tys.error }, &[GenericArg::Type(tail)]),
                    TyKind::Param(_) | TyKind::Alias(..) => self.tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: match self.require_lang_item(LangItem::Metadata) { Some(def) => def, None => return tys.error } }, &[GenericArg::Type(tail)]),
                    _ => tys.unit,
                }
            }
            Rvalue::BinaryOp(mir::BinOp::Cmp, _) => self.nominal_ty(match self.require_lang_item(LangItem::OrderingEnum) { Some(def) => def, None => return tys.error }, &[]),
            Rvalue::ShallowInitBox(_, ty) => {
                let def = match self.require_lang_item(LangItem::OwnedBox) { Some(def) => def, None => return tys.error };
                let mut args = vec![*ty];
                let state = crate::tcx::EvalState::new();
                let cx = crate::tcx::EvalCx::new(self.prog, &self.tcx, crate::tcx::TypingEnv::fully_monomorphized(self.prog), &state);
                self.tcx.fill_type_defaults(&cx, self.prog, def, &mut args, None);
                tys.intern(TyKind::Adt(def, tys.mk_list(&args), crate::types::RegionArgsId::EMPTY))
            }
            Rvalue::Repeat(op, count) => {
                let ct = match count {
                    mir::Const::Val(mir::ConstValue::Scalar(mir::Scalar::Int { bits, .. }), _) => tys.const_usize(u64::try_from(*bits).expect("usize length")),
                    mir::Const::Unevaluated { def, args, promoted } => {
                        assert!(promoted.is_none(), "array length cannot be a promoted reference");
                        use crate::consts::{ConstKind, AliasConst, AliasConstKind, IsRigid};
                        let kind = match self.prog.def(*def).kind {
                            crate::program::DefKind::AnonConst => AliasConstKind::Anon { def_id: *def },
                            crate::program::DefKind::AssocConst if self.prog.def(self.prog.def(*def).parent).kind == crate::program::DefKind::Trait => AliasConstKind::Projection { def_id: *def },
                            crate::program::DefKind::AssocConst => AliasConstKind::InherentImpl { def_id: *def },
                            crate::program::DefKind::Const => AliasConstKind::Free { def_id: *def },
                            _ => panic!("array length constant definition"),
                        };
                        tys.intern_const(ConstKind::Alias(IsRigid::No, AliasConst { kind, args: args.clone().into() }))
                    },
                    _ => panic!("array length is not a scalar"),
                };
                tys.mk_array(self.operand_ty(body, op), ct)
            }
            _ => unreachable!("rvalue has a structural type"),
        }
    }
    fn is_privately_uninhabited(&mut self, ty: TyId) -> bool {
        !super::type_check::inhabitedness::inhabited_predicate(self, ty, &mut Vec::new()).apply_ignore_module(self, &mut Vec::new())
    }
    fn unevaluated_const_ty(&self, def: DefId, args: &[GenericArg]) -> TyId {
        let ty = self.tcx.type_of(self.prog, def).expect("constant declaration type");
        self.tcx.instantiate_ty(self.prog, def, args, ty)
    }
    fn static_ptr(&self, constant: &ConstOperand) -> Option<DefId> {
        match &constant.const_ {
            // Round 29 D8: M2b owns GlobalAlloc identity, alongside user type
            // annotations and closure signatures in the round-19 typed-MIR input list.
            mir::Const::Val(mir::ConstValue::Scalar(mir::Scalar::Ptr { .. }), _) =>
                panic!("M2b: GlobalAlloc static identity for MIR pointer constants"),
            _ => None,
        }
    }
    fn static_ty(&mut self, out: &mut MirTypeckRegionConstraints, def: DefId, locations: Locations) -> TyId {
        let ty = self.tcx.const_ty(self.prog, def).expect("static declaration type");
        BorrowckTypeOps::normalize(self, out, ty, locations)
    }
    fn type_is_sized_modulo_regions(&mut self, ty: TyId) -> bool {
        let def = self.tcx.lang_items.get(LangItem::Sized).expect("Sized lang item");
        self.infcx.evaluate_obligation(self.prog, Goal { param_env: self.param_env_id(), predicate: crate::next_solver::collect::trait_predicate(def, vec![GenericArg::Type(ty)]) }).must_apply_modulo_regions()
    }
    fn is_upvar_field_projection(&self, _: &Body, _: &Place) -> Option<FieldIdx> { panic!("M2b: MIR capture metadata for upvar field projections") }
    fn simd_const_arg(&self, def: DefId) -> Option<(usize, &'static str)> {
        if !self.tcx.fn_item_sig(self.prog, def).is_some_and(|s| s.header.abi == crate::types::Abi::RelaxIntrinsic) { return None; }
        match self.prog.name(def) { "simd_shuffle" => Some((2, "simd_shuffle")), "simd_insert" => Some((1, "simd_insert")), "simd_extract" => Some((1, "simd_extract")), _ => None }
    }
    fn closure_parent_args(&self, def: ClauseOwner, args: &[GenericArg]) -> (Vec<GenericArg>, Vec<GenericArg>) {
        let closure = self.clause_owner(def);
        let mut root = closure;
        while matches!(self.prog.def(root).kind, crate::program::DefKind::Closure | crate::program::DefKind::OpaqueTy) || BorrowckTypeOps::is_inline_const(self, root) { root = self.prog.def(root).parent; }
        let identity = self.tcx.generic_indices_of(self.prog, root).identity_args(&self.tcx.tys);
        let parent = args[..identity.len()].to_vec(); (identity, parent)
    }
    fn unsized_fn_params(&self) -> bool { false } // DEBLOAT: unstable unsized_fn_params.
    fn is_inline_const(&self, def: DefId) -> bool { matches!(self.prog.def_id_to_node_id.get(&def), Some((_, relax_front::def_collector::NodeId::InlineConst(_)))) }
}

#[cfg(test)]
mod round33_tests {
    use super::*;
    use crate::borrowck::type_check::{BorrowckTypeOps, MirTypeckRegionConstraints};

    /// front-bck audit 1.12 / N:rustc_middle/src/ty/util.rs:256-311: the struct tail walk takes
    /// the crate's `#![recursion_limit]` (iterations 0..=limit) and reports reaching it.
    #[test]
    fn round33_struct_tail_recursion_limit() {
        let source = br#"
            #![no_std]
            #![recursion_limit = "4"]
            fn f() {}
        "#;
        let dir = std::env::temp_dir().join(format!("relax-r33-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, source).unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut prog = Program::new(&session);
        prog.load_crate("r33tail", file.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        std::fs::remove_dir_all(&dir).unwrap();
        let def = |n| (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.name(d) == n).unwrap();
        let f = def("f");
        let slice = tcx.tys.intern(TyKind::Slice(tcx.tys.u8_));
        // tuple layers: the walk is the same, without the (MIR-located) field normalization
        let nest = |n: usize| (0..n).fold(slice, |t, _| tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[tcx.tys.u8_, t]))));
        let at = crate::mir::Location { block: crate::mir::BasicBlock(0), statement_index: 0 };
        let mut infcx = BorrowckInferCtxt::new(&tcx, &prog, f, f, &[]);
        let mut out = MirTypeckRegionConstraints::new(0);
        // four layers: iterations 0..=3 walk them, iteration 4 sees the tail
        assert_eq!(BorrowckTypeOps::struct_tail(&mut infcx, &mut out, nest(4), at), slice);
        assert!(!infcx.tainted_by_errors.get());
        let mut infcx = BorrowckInferCtxt::new(&tcx, &prog, f, f, &[]);
        // five: iteration 5 is past the limit before it sees the tail (Limit::value_within_limit)
        assert_eq!(BorrowckTypeOps::struct_tail(&mut infcx, &mut out, nest(5), at), tcx.tys.error);
        assert!(infcx.tainted_by_errors.get(), "reaching the limit is a reported error");
    }
}
