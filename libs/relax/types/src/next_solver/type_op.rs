// ported from N:rustc_trait_selection/src/traits/query/type_op/mod.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/query/type_op/custom.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/query/type_op/normalize.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/query/type_op/prove_predicate.rs @8d1a764
// ported from N:rustc_traits/src/type_op.rs @8d1a764 (type_op_normalize, type_op_prove_predicate)
// ported from N:rustc_infer/src/infer/canonical/query_response.rs @8d1a764 (make_query_region_constraints)
//
// DEBLOAT: the canonical-query wrapper (canonicalize_query / enter_canonical_trait_query /
// instantiate_nll_query_response_and_region_obligations) is the old solver's caching layer.
// With the next solver its provider runs the same ObligationCtxt operation, so each op is
// performed directly in the caller's inference context under scrape_region_constraints.
// Region assumptions (-Zhigher-ranked-assumptions) and error_info (diagnostics) are cut.
use super::*;
use super::fulfill::ObligationCtxt;
use crate::types::TyKind;
use crate::borrowck::constraints::ConstraintCategory;
use crate::infer::InferCtxt;
use crate::infer::region_constraints::{ConstraintKind, RegionConstraintData};
use crate::program::Program;
use crate::tcx::outlives::OutlivesPredicate;

/// rustc_middle ty::RegionConstraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionConstraint { Outlives(OutlivesPredicate), Eq(RegionId, RegionId) }
impl RegionConstraint {
    pub fn iter_outlives(self) -> impl Iterator<Item = OutlivesPredicate> {
        let (first, second) = match self {
            Self::Outlives(p) => (p, None),
            Self::Eq(a, b) => (OutlivesPredicate::Region(a, b), Some(OutlivesPredicate::Region(b, a))),
        };
        std::iter::once(first).chain(second)
    }
}
/// rustc_middle infer/canonical.rs QueryRegionConstraint(s).
#[derive(Clone, Copy, Debug)]
pub struct QueryRegionConstraint { pub constraint: RegionConstraint, pub category: ConstraintCategory, pub visible_for_leak_check: bool }
#[derive(Clone, Debug, Default)]
pub struct QueryRegionConstraints { pub constraints: Vec<QueryRegionConstraint> }
impl QueryRegionConstraints {
    pub fn is_empty(&self) -> bool { self.constraints.is_empty() }
}

pub struct TypeOpOutput<R> { pub output: R, pub constraints: Option<QueryRegionConstraints> }

pub trait TypeOp: Sized {
    type Output;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<Self::Output>, NoSolution>;
    /// rustc `TypeOp::fully_perform` keeps the canonical query input for `error_info`
    /// (diagnostics rerun the query to blame a span for a higher-ranked failure). The input
    /// here is the query's own (types resolved, not canonicalized: the diagnostic reruns in
    /// the same inference context).
    fn input(&self) -> TypeOpInput { TypeOpInput::Custom }
}

/// The input of a type op, as retained for error reporting (rustc `TypeOpInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeOpInput {
    Normalize(ParamEnvId, TyId),
    ProvePredicate(ParamEnvId, Predicate),
    AscribeUserType(ParamEnvId, TyId, crate::types::user_type::UserType),
    DropckOutlives(ParamEnvId, TyId),
    /// a closure-defined op (opaque relations, instantiated clauses): no replayable input
    Custom,
}

#[derive(Clone, Debug)]
pub struct ParamEnvAnd<Q> { pub param_env: ParamEnvId, pub value: Q }
#[derive(Clone, Copy, Debug)]
pub struct Normalize { pub value: TyId }
/// `Normalize` of a signature: the opaque types the body may define stay rigid aliases. The default solver
/// never touches an opaque type when it normalizes; only relating one to its hidden type (`relate_opaque`)
/// defines it, so a callee's `impl Trait` output or the body's own signature is no defining use.
#[derive(Clone, Copy, Debug)]
pub struct NormalizeSignature { pub value: TyId }
#[derive(Clone, Debug)]
pub struct ProvePredicate { pub predicate: Predicate }

/// normalize.rs:14 try_fast_path; rustc_traits type_op.rs:41 type_op_normalize with the
/// next solver's deep normalization (at.query_normalize).
fn normalize_type_op(infcx: &InferCtxt<'_>, prog: &Program, param_env: ParamEnvId, ty: TyId) -> Result<TypeOpOutput<TyId>, NoSolution> {
    let tys = &infcx.tcx.tys;
    if !tys.may_project(ty) && !tys.has_opaque(ty) { return Ok(TypeOpOutput { output: ty, constraints: None }); }
    scrape_region_constraints(infcx, prog, |ocx| ocx.deeply_normalize_ty(&param_env, ty))
}
/// Normalization never defines an opaque type (the default solver does not touch opaques there; only relating
/// one to its hidden type does): the opaques the body may define are not defining while it runs. Their
/// occurrences it leaves rigid are returned to the opaque, still to be defined by whatever they are related to,
/// unless `rigid` (a signature's).
fn normalize_without_defining(infcx: &InferCtxt<'_>, prog: &Program, param_env: ParamEnvId, ty: TyId, rigid: bool) -> Result<TypeOpOutput<TyId>, NoSolution> {
    let mode = infcx.typing_mode.get();
    let crate::tcx::TypingMode::PostTypeckUntilBorrowck { defining_opaque_types } = mode else { return normalize_type_op(infcx, prog, param_env, ty) };
    infcx.typing_mode.set(crate::tcx::TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: Default::default() });
    let result = normalize_type_op(infcx, prog, param_env, ty);
    infcx.typing_mode.set(mode);
    if rigid { return result; }
    let defining = infcx.tcx.defining_opaques(defining_opaque_types);
    result.map(|mut out| { out.output = unrigid_opaques(infcx.tcx, out.output, defining); out })
}
impl TypeOp for ParamEnvAnd<Normalize> {
    type Output = TyId;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<TyId>, NoSolution> {
        normalize_without_defining(infcx, prog, self.param_env, self.value.value, false)
    }
    fn input(&self) -> TypeOpInput { TypeOpInput::Normalize(self.param_env, self.value.value) }
}
impl TypeOp for ParamEnvAnd<NormalizeSignature> {
    type Output = TyId;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<TyId>, NoSolution> {
        normalize_without_defining(infcx, prog, self.param_env, self.value.value, true)
    }
    fn input(&self) -> TypeOpInput { TypeOpInput::Normalize(self.param_env, self.value.value) }
}
/// prove_predicate.rs:10 (the WF fast path; sizedness has no Relax fast path here) and
/// type_op_prove_predicate_with_cause.
impl TypeOp for ParamEnvAnd<ProvePredicate> {
    type Output = ();
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<()>, NoSolution> {
        // (proving a predicate never defines an opaque type either: see `NormalizeSignature`)
        let mode = infcx.typing_mode.get();
        if matches!(mode, crate::tcx::TypingMode::PostTypeckUntilBorrowck { .. }) {
            infcx.typing_mode.set(crate::tcx::TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: Default::default() });
        }
        let result = scrape_region_constraints(infcx, prog, |ocx| {
            ocx.register_obligation(Goal { param_env: self.param_env.clone(), predicate: self.value.predicate.clone() });
            Ok(())
        });
        infcx.typing_mode.set(mode);
        result
    }
    fn input(&self) -> TypeOpInput { TypeOpInput::ProvePredicate(self.param_env, self.value.predicate.clone()) }
}

/// ascribe_user_type.rs: relate the MIR type to a user annotation.
#[derive(Clone, Debug)]
pub struct AscribeUserType { pub mir_ty: TyId, pub user_ty: crate::types::user_type::UserType }

/// ascribe_user_type.rs:44 type_op_ascribe_user_type_with_span.
impl TypeOp for ParamEnvAnd<AscribeUserType> {
    type Output = ();
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<()>, NoSolution> {
        use crate::types::user_type::UserTypeKind;
        let ParamEnvAnd { param_env: env, value: AscribeUserType { mir_ty, user_ty } } = self;
        scrape_region_constraints(infcx, prog, |ocx| {
            let wf = |term: Term| Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(term))) };
            match &user_ty.kind {
                // :74 relate_mir_and_user_ty
                UserTypeKind::Ty(user) => {
                    ocx.register_obligation(wf(Term::Ty(*user)));
                    let user = ocx.normalize_ty(&env, *user);
                    ocx.eq(&env, mir_ty, user)?;
                }
                // :95 relate_mir_and_user_args
                UserTypeKind::TypeOf(def, user_args) => {
                    let tcx = ocx.infcx.tcx;
                    let args = &user_args.args;
                    let mut ty = user_type_of(tcx, prog, *def, args);
                    // Relax has one DefId for a tuple struct and its constructor (rustc has two): an annotation
                    // that names the constructor is related to a `FnDef`, one that names the ADT (a constructor
                    // call's aggregate) to the ADT type.
                    if let TyKind::FnDef(d, a, r) = *tcx.tys.kind(ty) {
                        if matches!(prog.def(d).kind, crate::program::DefKind::Struct | crate::program::DefKind::Variant) && !matches!(tcx.tys.kind(mir_ty), TyKind::FnDef(..)) {
                            let adt = if prog.def(d).kind == crate::program::DefKind::Variant { prog.def(d).parent } else { d };
                            ty = tcx.tys.intern(TyKind::Adt(adt, a, r));
                        }
                    }
                    let ty = ocx.normalize_ty(&env, ty);
                    ocx.eq(&env, mir_ty, ty)?;
                    // the predicates that come with `def`, normalized
                    let owner = if prog.def(*def).kind == crate::program::DefKind::Variant { prog.def(*def).parent } else { *def };
                    for p in tcx.clauses_of(prog, owner) {
                        let p = tcx.instantiate_predicate(prog, owner, args, &p);
                        let predicate = ocx.normalize_predicate(&env, &p);
                        ocx.register_obligation(Goal { param_env: env.clone(), predicate });
                    }
                    for arg in args {
                        match *arg {
                            GenericArg::Type(t) => ocx.register_obligation(wf(Term::Ty(t))),
                            GenericArg::Const(c) => ocx.register_obligation(wf(Term::Const(c))),
                            GenericArg::Lifetime(_) => {}
                        }
                    }
                    if let Some(self_ty) = user_args.user_self_ty {
                        ocx.register_obligation(wf(Term::Ty(self_ty.self_ty)));
                        let self_ty_n = ocx.normalize_ty(&env, self_ty.self_ty);
                        let impl_self = tcx.impl_self.get(&self_ty.impl_def_id).copied().ok_or(NoSolution)?;
                        let impl_self = tcx.instantiate_ty(prog, self_ty.impl_def_id, args, impl_self);
                        let impl_self = ocx.normalize_ty(&env, impl_self);
                        ocx.eq(&env, self_ty_n, impl_self)?;
                    }
                }
            }
            // :56 bounds from `impl Trait` in bindings
            for clause in &user_ty.bounds { ocx.register_obligation(Goal { param_env: env.clone(), predicate: clause.clone() }); }
            Ok(())
        })
    }
    fn input(&self) -> TypeOpInput { TypeOpInput::AscribeUserType(self.param_env, self.value.mir_ty, self.value.user_ty.clone()) }
}

/// `type_of(def).instantiate(args)` for the definitions a user annotation names: a fn item is
/// its `FnDef` type, a tuple constructor the same, other constructors and ADTs the ADT type,
/// a const or static its declared type.
pub fn user_type_of(tcx: &crate::tcx::Tcx, prog: &Program, def: DefId, args: &[GenericArg]) -> TyId {
    use crate::program::DefKind;
    let (mut compact, mut regions) = (Vec::new(), Vec::new());
    for arg in args {
        match *arg {
            GenericArg::Type(t) => compact.push(t),
            GenericArg::Const(c) => compact.push(tcx.tys.intern(TyKind::Const(c))),
            GenericArg::Lifetime(r) => regions.push(r),
        }
    }
    let (list, regions) = (tcx.tys.mk_list(&compact), tcx.tys.intern_region_args(&regions));
    match prog.def(def).kind {
        DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => tcx.tys.intern(TyKind::FnDef(def, list, regions)),
        DefKind::Variant | DefKind::Struct | DefKind::Union => {
            let (adt, vi) = if prog.def(def).kind == DefKind::Variant { (prog.def(def).parent, prog.def(def).sub as usize) } else { (def, 0) };
            let tuple = tcx.adts.get(&adt).is_some_and(|a| a.variants.get(vi).is_some_and(|v| v.shape == 1) && !a.is_union());
            if tuple { tcx.tys.intern(TyKind::FnDef(def, list, regions)) } else { tcx.tys.intern(TyKind::Adt(adt, list, regions)) }
        }
        DefKind::Enum => tcx.tys.intern(TyKind::Adt(def, list, regions)),
        _ => {
            let ty = tcx.type_of(prog, def).unwrap_or_else(|e| panic!("annotated definition has a declared type: {e:?} {:?} {}", prog.def(def).kind, prog.def_path(def)));
            tcx.instantiate_ty(prog, def, args, ty)
        }
    }
}

/// custom.rs:16.
pub struct CustomTypeOp<F> { closure: F }
impl<F> CustomTypeOp<F> { pub fn new(closure: F) -> Self { Self { closure } } }
impl<R, F: FnOnce(&mut ObligationCtxt<'_, '_>) -> Result<R, NoSolution>> TypeOp for CustomTypeOp<F> {
    type Output = R;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<R>, NoSolution> {
        scrape_region_constraints(infcx, prog, self.closure)
    }
}

/// custom.rs:68-150: run `op`, prove what it registered, then scrape the region
/// constraints and type-outlives obligations it produced.
pub fn scrape_region_constraints<R>(infcx: &InferCtxt<'_>, prog: &Program,
    op: impl FnOnce(&mut ObligationCtxt<'_, '_>) -> Result<R, NoSolution>) -> Result<TypeOpOutput<R>, NoSolution> {
    let pre_obligations = infcx.take_registered_region_obligations();
    assert!(pre_obligations.is_empty(), "scrape_region_constraints: incoming region obligations = {pre_obligations:?}");
    let value = infcx.commit_if_ok(|_| {
        let mut ocx = ObligationCtxt::new(infcx, prog);
        let value = op(&mut ocx)?;
        ocx.evaluate_obligations_error_on_ambiguity()?;
        Ok(value)
    })?;
    let region_obligations = infcx.take_registered_region_obligations();
    let region_constraint_data = infcx.take_and_reset_region_constraints();
    let region_constraints = make_query_region_constraints(region_obligations, &region_constraint_data);
    let constraints = if region_constraints.is_empty() { None } else { Some(region_constraints) };
    Ok(TypeOpOutput { output: value, constraints })
}

/// query_response.rs:618: sub (<=) constraints become outlives (>=).
pub fn make_query_region_constraints(outlives_obligations: Vec<(TyId, RegionId)>, region_constraints: &RegionConstraintData) -> QueryRegionConstraints {
    assert!(region_constraints.verifys.is_empty());
    let mut constraints = Vec::with_capacity(region_constraints.constraints.len() + outlives_obligations.len());
    for c in &region_constraints.constraints {
        let constraint = match c.kind {
            ConstraintKind::VarSubVar | ConstraintKind::RegSubVar | ConstraintKind::VarSubReg | ConstraintKind::RegSubReg =>
                RegionConstraint::Outlives(OutlivesPredicate::Region(c.sup, c.sub)),
            ConstraintKind::VarEqVar | ConstraintKind::VarEqReg | ConstraintKind::RegEqReg => RegionConstraint::Eq(c.sup, c.sub),
        };
        constraints.push(QueryRegionConstraint { constraint, category: ConstraintCategory::Boring, visible_for_leak_check: c.visible_for_leak_check });
    }
    for (sup_type, sub_region) in outlives_obligations {
        // We don't do leak checks for type outlives.
        constraints.push(QueryRegionConstraint { constraint: RegionConstraint::Outlives(OutlivesPredicate::Type(sup_type, sub_region)),
            category: ConstraintCategory::Boring, visible_for_leak_check: false });
    }
    QueryRegionConstraints { constraints }
}


/// `set_aliases_to_non_rigid` for the opaque aliases `defining` only.
fn unrigid_opaques(tcx: &crate::tcx::Tcx, ty: TyId, defining: &[crate::program::DefId]) -> TyId {
    use crate::types::fold::{TypeFolder, super_fold_ty};
    struct F<'a>(&'a [crate::program::DefId]);
    impl TypeFolder for F<'_> {
        fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
            if !tys.has_opaque(ty) { return ty; }
            let ty = super_fold_ty(tys, ty, self, depth);
            match tys.kind(ty) {
                TyKind::Alias(crate::consts::IsRigid::Yes, alias) if matches!(alias.kind, crate::types::AliasTyKind::Opaque { def_id } if self.0.contains(&def_id)) =>
                    tys.intern(TyKind::Alias(crate::consts::IsRigid::No, alias.clone())),
                _ => ty,
            }
        }
    }
    F(defining).fold_ty(&tcx.tys, ty, 0)
}
