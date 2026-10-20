// ported from N:rustc_type_ir/src/solve/mod.rs @8d1a764
// ported from N:rustc_type_ir/src/predicate.rs @8d1a764
// ported from N:rustc_type_ir/src/predicate_kind.rs @8d1a764
use crate::consts::{AliasConst, Const, GenericArg};
use crate::program::DefId;
pub use crate::tcx::ParamEnvId;
use std::sync::Arc;
pub mod canonical;
pub mod fold;
pub mod typing_mode;
use typing_mode::{NoSolutionOrRerunNonErased, QueryResultOrRerunNonErased, AccessedOpaques};
pub mod response;
pub use response::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParamEnv { pub clauses: Arc<[Predicate]> }
impl std::hash::Hash for ParamEnv {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        crate::solver_work::count(crate::solver_work::ENV_HASHES);
        crate::solver_work::add(crate::solver_work::ENV_HASH_CLAUSES, self.clauses.len() as u64);
        std::hash::Hash::hash(&self.clauses, state);
    }
}
use crate::types::{RegionId, TyId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Term { Ty(TyId), Const(Const) }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraitRef { pub def_id: DefId, pub args: Vec<GenericArg> }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AliasTerm { Ty(TyId), Const(AliasConst) }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionClause { pub projection_term: AliasTerm, pub term: Term }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SubtypePredicate { pub a_is_expected: bool, pub a: TyId, pub b: TyId }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClausePolarity { Positive, Negative }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraitClause { pub trait_ref: TraitRef, pub polarity: ClausePolarity }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClauseKind {
    Trait(TraitClause),
    Projection(ProjectionClause),
    RegionOutlives(RegionId, RegionId),
    TypeOutlives(TyId, RegionId),
    WellFormed(Term),
    ConstEvaluatable(Const),
    ConstArgHasType(Const, TyId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CoercePredicate { pub a: TyId, pub b: TyId }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NormalizesTo { pub alias: AliasTerm, pub term: Term }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PredicateKind {
    Clause(ClauseKind),
    DynCompatible(DefId),
    Subtype(SubtypePredicate),
    Coerce(CoercePredicate),
    ConstEquate(Const, Const),
    Ambiguous,
    NormalizesTo(NormalizesTo),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoundVariableKind { Ty, Region(crate::types::BoundRegionKind), Const }

/// predicate.rs: a predicate retains its binder even when no variables are bound.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Predicate { pub kind: PredicateKind, pub bound_vars: Arc<[BoundVariableKind]> }
impl Predicate {
    /// rustc_type_ir/flags.rs: aggregate the cached flags of each predicate child.
    pub(crate) fn flags(&self, tys: &crate::types::Types) -> u32 {
        fn arg(tys: &crate::types::Types, a: GenericArg) -> u32 {
            match a {
                GenericArg::Type(t) => tys.flags(t).bits(),
                GenericArg::Const(c) => tys.const_flags(c).0,
                GenericArg::Lifetime(r) => tys.region_flags(r),
            }
        }
        let term = |t: Term| match t { Term::Ty(t) => tys.flags(t).bits(), Term::Const(c) => tys.const_flags(c).0 };
        let alias = |a: &AliasTerm| match a {
            AliasTerm::Ty(t) => tys.flags(*t).bits(),
            AliasTerm::Const(a) => a.args.iter().fold(0, |flags, a| flags | arg(tys, *a)),
        };
        match &self.kind {
            PredicateKind::Clause(c) => match c {
                ClauseKind::Trait(t) => t.trait_ref.args.iter().fold(0, |flags, a| flags | arg(tys, *a)),
                ClauseKind::Projection(p) => alias(&p.projection_term) | term(p.term),
                ClauseKind::RegionOutlives(a, b) => tys.region_flags(*a) | tys.region_flags(*b),
                ClauseKind::TypeOutlives(t, r) => tys.flags(*t).bits() | tys.region_flags(*r),
                ClauseKind::WellFormed(t) => term(*t),
                ClauseKind::ConstEvaluatable(c) => tys.const_flags(*c).0,
                ClauseKind::ConstArgHasType(c, t) => tys.const_flags(*c).0 | tys.flags(*t).bits(),
            },
            PredicateKind::Subtype(p) => tys.flags(p.a).bits() | tys.flags(p.b).bits(),
            PredicateKind::Coerce(p) => tys.flags(p.a).bits() | tys.flags(p.b).bits(),
            PredicateKind::ConstEquate(a, b) => tys.const_flags(*a).0 | tys.const_flags(*b).0,
            PredicateKind::NormalizesTo(p) => alias(&p.alias) | term(p.term),
            PredicateKind::DynCompatible(_) | PredicateKind::Ambiguous => 0,
        }
    }

    pub fn dummy(kind: PredicateKind) -> Self { Self { kind, bound_vars: Arc::from([]) } }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Goal { pub param_env: ParamEnvId, pub predicate: Predicate }

pub mod delegate;

pub mod collect;
pub mod elaborate;
pub mod rpitit;

pub mod search_graph;
pub mod eval_ctxt;
mod goals;
pub type QueryResult = Result<canonical::Canonical<Response>, NoSolution>;

fn response_values_are_identity(tys: &crate::types::Types, response: &canonical::Canonical<Response>, ignore_regions: bool) -> bool {
    for (index, value) in response.value.var_values.iter().enumerate() {
        let identity = match value {
            GenericArg::Lifetime(r) => ignore_regions || matches!(tys.region_kind(*r), crate::types::RegionKind::ReBound(0, var) if var.0 == index as u32),
            GenericArg::Type(ty) => matches!(tys.kind(*ty), crate::types::TyKind::Bound(0, bound) if bound.var.0 == index as u32),
            GenericArg::Const(ct) => matches!(tys.const_kind(*ct), crate::consts::ConstKind::Bound(0, bound) if bound.var.0 == index as u32),
        };
        if !identity { return false; }
    }
    true
}
pub fn has_only_region_constraints(tys: &crate::types::Types, response: &canonical::Canonical<Response>) -> bool {
    response_values_are_identity(tys, response, true) && response.value.external_constraints.opaque_types.is_empty()
        && response.value.external_constraints.normalization_nested_goals.is_empty()
}
pub fn has_no_inference_or_external_constraints(tys: &crate::types::Types, response: &canonical::Canonical<Response>) -> bool {
    response_values_are_identity(tys, response, false) && response.value.external_constraints.is_empty()
}

pub mod placeholder;
pub mod normalize;

pub mod assembly;
mod trait_goals;

mod project_goals;

pub mod wf;

pub mod fulfill;
pub mod param_env;
pub mod type_op;
pub mod dropck_outlives;

pub mod implied_outlives_bounds;

mod object;

pub(crate) mod dyn_compatibility;

mod type_of;

mod opaque_types;

#[cfg(test)]
mod tests;

fn map_err_to_rerun<T>(result: Result<T, NoSolutionOrRerunNonErased>) -> Result<Result<T, NoSolution>, NoSolutionOrRerunNonErased> {
    match result { Ok(r) => Ok(Ok(r)), Err(NoSolutionOrRerunNonErased::NoSolution) => Ok(Err(NoSolution)), Err(e) => Err(e) }
}

mod unsize;

pub mod coherence;

pub mod sizedness;
mod normalize_at;

pub mod inspect;
pub mod select;
pub mod codegen;
pub mod vtable;
mod compare_impl_item;
pub mod evaluate_obligation;

impl Predicate {
    /// rustc_type_ir/src/visit.rs: binder declarations alone do not escape.
    pub fn has_bound_vars(&self, tys: &crate::types::Types) -> bool {
        struct Finder(bool);
        impl crate::types::fold::TypeFolder for Finder {
            fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: u32) -> TyId {
                self.0 |= tys.has_vars_bound_at_or_above(t, depth); t
            }
            fn fold_const(&mut self, tys: &crate::types::Types, c: Const, depth: u32) -> Const {
                self.0 |= tys.has_vars_bound_at_or_above(tys.intern(crate::types::TyKind::Const(c)), depth); c
            }
            fn fold_region(&mut self, tys: &crate::types::Types, r: crate::types::RegionId, depth: u32) -> crate::types::RegionId {
                self.0 |= matches!(tys.region_kind(r), crate::types::RegionKind::ReBound(d, _) if d >= depth); r
            }
        }
        let mut finder = Finder(false);
        fold::fold_predicate_kind(tys, &self.kind, &mut finder, 0); finder.0
    }
}

pub mod fast_reject;
pub mod overlap;
pub mod specialization_graph;
pub mod inherent_impls_overlap;
