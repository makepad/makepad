// ported from N:rustc_trait_selection/src/solve/normalize.rs @8d1a764
use super::*;
use crate::infer::InferCtxt;
use crate::program::Program;
use crate::types::{Types, TyKind, UniverseIndex};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
use crate::consts::{ConstKind, IsRigid};
use super::placeholder::BoundVarReplacer;

impl InferCtxt<'_> {
    /// normalize.rs:22-35: failed eager normalization is represented by fresh
    /// terms and projection obligations; fulfillment owns the eventual errors.
    pub fn normalize_ty(&self, prog: &Program, env: &ParamEnvId, ty: TyId) -> (TyId, Vec<Goal>) {
        match self.normalize_ty_with_universes(prog, env, ty) {
            Ok(result) => result,
            Err(_) => {
                let mut folder = ReplaceAliasWithInfer::new(self, env);
                let ty = folder.fold_ty(&self.tcx.tys, self.deeply_resolve_ignoring_regions(ty), 0);
                (ty, folder.obligations)
            }
        }
    }
    pub fn normalize_predicate(&self, prog: &Program, env: &ParamEnvId, p: &Predicate) -> (Predicate, Vec<Goal>) {
        match self.normalize_predicate_with_universes(prog, env, p) {
            Ok(result) => result,
            Err(_) => {
                let mut resolver = crate::infer::resolve::DeepResolverIgnoringRegions::new(self);
                let p = fold::fold_predicate(&self.tcx.tys, p, &mut resolver);
                let mut folder = ReplaceAliasWithInfer::new(self, env);
                let p = if matches!(p.kind, PredicateKind::Clause(ClauseKind::WellFormed(_))) { p }
                    else { fold::fold_predicate(&self.tcx.tys, &p, &mut folder) };
                (p, folder.obligations)
            }
        }
    }
}
struct ReplaceAliasWithInfer<'a, 'tcx> {
    infcx: &'a InferCtxt<'tcx>, env: &'a ParamEnvId, obligations: Vec<Goal>, universes: Vec<Option<UniverseIndex>>,
}
impl<'a, 'tcx> ReplaceAliasWithInfer<'a, 'tcx> {
    fn new(infcx: &'a InferCtxt<'tcx>, env: &'a ParamEnvId) -> Self { Self { infcx, env, obligations: Vec::new(), universes: Vec::new() } }
    fn term_to_infer(&mut self, alias: AliasTerm) -> Term {
        let term = match alias { AliasTerm::Ty(_) => Term::Ty(self.infcx.next_ty_var()), AliasTerm::Const(_) => Term::Const(self.infcx.next_const_var()) };
        self.obligations.push(Goal { param_env: self.env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: alias, term }))) });
        term
    }
}
impl TypeFolder for ReplaceAliasWithInfer<'_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        self.universes.resize(depth as usize, None);
        let ty = super_fold_ty(tys, ty, self, depth);
        self.universes.resize(depth as usize, None);
        if !matches!(tys.kind(ty), TyKind::Alias(IsRigid::No, _)) { return ty; }
        if tys.has_vars_bound_at_or_above(ty, 0) {
            let mapped = BoundVarReplacer::new(self.infcx, &mut self.universes).fold_ty(tys, ty, 0);
            self.term_to_infer(AliasTerm::Ty(mapped)); ty
        } else { let Term::Ty(ty) = self.term_to_infer(AliasTerm::Ty(ty)) else { unreachable!() }; ty }
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        self.universes.resize(depth as usize, None);
        let ct = super_fold_const(tys, ct, self, depth);
        self.universes.resize(depth as usize, None);
        let ConstKind::Alias(IsRigid::No, alias) = tys.const_kind(ct) else { return ct; };
        if tys.has_vars_bound_at_or_above(tys.intern(TyKind::Const(ct)), 0) {
            let mapped = BoundVarReplacer::new(self.infcx, &mut self.universes).fold_const(tys, ct, 0);
            let ConstKind::Alias(_, alias) = tys.const_kind(mapped) else { unreachable!() };
            self.term_to_infer(AliasTerm::Const(alias)); ct
        } else { let Term::Const(ct) = self.term_to_infer(AliasTerm::Const(alias)) else { unreachable!() }; ct }
    }
}
