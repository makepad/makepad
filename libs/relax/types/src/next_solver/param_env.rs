// ported from N:rustc_ty_utils/src/ty.rs @8d1a764
use super::*;
use crate::program::{Program, DefId, DefKind};
use crate::tcx::{Tcx, FnSigT, TypingMode};
use crate::types::{TyKind, AliasTyKind};
use crate::infer::InferCtxt;
use crate::next_solver::fulfill::{ObligationCtxt, ObligationCause};
use crate::types::{Types, RegionId, RegionKind};
use crate::types::fold::{TypeFolder, super_fold_ty};

/// ty.rs:209-288: default trait bodies can define their RPITIT's opaque.
struct ImplTraitInTraitFinder<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    clauses: &'a mut Vec<Predicate>,
    fn_def_id: DefId,
    bound_vars: Vec<crate::next_solver::BoundVariableKind>,
    seen: Vec<DefId>,
    failed: bool,
}
struct ShiftToFnBinder { depth: u32, failed: bool }
impl TypeFolder for ShiftToFnBinder {
    fn fold_region(&mut self, tys: &Types, region: RegionId, depth: u32) -> RegionId {
        if let RegionKind::ReBound(index, var) = tys.region_kind(region) {
            if depth != 0 || index < self.depth {
                self.failed = true;
                return tys.intern_region(RegionKind::ReError);
            }
            tys.intern_region(RegionKind::ReBound(index - self.depth, var))
        } else { region }
    }
}
impl TypeFolder for ImplTraitInTraitFinder<'_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Alias(_, alias) = tys.kind(ty) {
            if let AliasTyKind::Projection { def_id } = alias.kind {
                let owner = match self.prog.impl_trait_in_trait_data.get(&def_id) {
                    Some(relax_front::assoc::ImplTraitInTraitData::Trait { fn_def_id, .. }) |
                    Some(relax_front::assoc::ImplTraitInTraitData::Impl { fn_def_id }) => Some(*fn_def_id),
                    None => None,
                };
                if owner == Some(self.fn_def_id) && !self.seen.contains(&def_id) {
                    self.seen.push(def_id);
                    let mut shift = ShiftToFnBinder { depth, failed: false };
                    let shifted = shift.fold_ty(tys, ty, 0);
                    self.failed |= shift.failed;
                    let TyKind::Alias(_, shifted_alias) = tys.kind(shifted) else { unreachable!() };
                    let args = self.tcx.complete_args(self.prog, def_id, tys.list(shifted_alias.args), shifted_alias.regions);
                    let default = match self.tcx.type_of(self.prog, def_id) {
                        Ok(default) => self.tcx.instantiate_ty(self.prog, def_id, &args, default),
                        Err(_) => { self.failed = true; tys.error },
                    };
                    self.clauses.push(Predicate {
                        kind: PredicateKind::Clause(ClauseKind::Projection(crate::next_solver::ProjectionClause {
                            projection_term: AliasTerm::Ty(shifted), term: Term::Ty(default),
                        })),
                        bound_vars: self.bound_vars.clone().into(),
                    });
                    let args = self.tcx.complete_args(self.prog, def_id, tys.list(alias.args), alias.regions);
                    for bound in self.tcx.item_bounds(self.prog, def_id) {
                        let bound = self.tcx.instantiate_predicate(self.prog, def_id, &args, &bound);
                        crate::next_solver::fold::fold_predicate_kind(tys, &bound.kind, self, depth + 1);
                    }
                }
            }
        }
        super_fold_ty(tys, ty, self, depth)
    }
}

pub(super) fn add_default_body_rpitit_clauses(prog: &Program, tcx: &Tcx, def: DefId, sig: &FnSigT, clauses: &mut Vec<Predicate>) -> bool {
    if prog.def(def).kind != DefKind::AssocFn || prog.def(prog.def(def).parent).kind != DefKind::Trait { return true; }
    let mut finder = ImplTraitInTraitFinder {
        tcx, prog, clauses, fn_def_id: def,
        bound_vars: vec![crate::next_solver::BoundVariableKind::Region(crate::types::BoundRegionKind::Anon); sig.bound_regions as usize],
        seen: Vec::new(), failed: false,
    };
    for &ty in &sig.params { finder.fold_ty(&tcx.tys, ty, 0); }
    finder.fold_ty(&tcx.tys, sig.ret, 0);
    !finder.failed
}

pub(crate) fn param_env(tcx: &Tcx, prog: &Program, def: DefId) -> ParamEnv {
    // N:rustc_ty_utils/src/ty.rs:151-153; ty/util.rs:601-664.
    // Ordinary anonymous consts have their own environment; inline consts,
    // and closures inherit the typeck root's assumptions.
    let mut root = def;
    loop {
        let child = match prog.def(root).kind {
            DefKind::Closure => true,
            DefKind::AnonConst => matches!(prog.def_id_to_node_id.get(&root),
                Some((_, relax_front::def_collector::NodeId::InlineConst(_)))),
            _ => false,
        };
        if !child { break; }
        root = prog.def(root).parent;
    }
    if root != def { return tcx.param_env(prog, root); }
    let infcx = InferCtxt::for_item(tcx, prog, TypingMode::non_body_analysis(), def);
    let mut clauses = tcx.clauses_of(prog, def);
    if prog.def(def).kind == DefKind::AssocFn && prog.def(prog.def(def).parent).kind == DefKind::Trait {
        let d = prog.def(def);
        let ast = &prog.files[d.file as usize].ast;
        if matches!(ast.item(d.item).kind, crate::ast::ItemKind::Fn(_, Some(_), _)) || prog.fn_body(d.file, d.item).ok().flatten().is_some() {
            if let Some(sig) = tcx.fn_sig(prog, def) {
                if !add_default_body_rpitit_clauses(prog, tcx, def, &sig, &mut clauses) {
                    infcx.errors.report(infcx.cause, "invalid return-position impl Trait in default trait body".into());
                }
            }
        }
    }
    let env = normalize_param_env_or_error(tcx, prog, ParamEnv { clauses: clauses.into() }, infcx.cause);
    env
}

struct Rigidness { types_rigid: bool, opaques_only: bool }
impl TypeFolder for Rigidness {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        let ty = super_fold_ty(tys, ty, self, depth);
        if let TyKind::Alias(_, alias) = tys.kind(ty) {
            if !self.opaques_only || matches!(alias.kind, AliasTyKind::Opaque { .. }) {
                return tys.intern(TyKind::Alias(if self.types_rigid { crate::consts::IsRigid::Yes } else { crate::consts::IsRigid::No }, alias.clone()));
            }
        }
        ty
    }
    fn fold_const(&mut self, tys: &Types, ct: crate::consts::Const, depth: u32) -> crate::consts::Const {
        let ct = crate::types::fold::super_fold_const(tys, ct, self, depth);
        if !self.types_rigid && !self.opaques_only {
            if let crate::consts::ConstKind::Alias(_, alias) = tys.const_kind(ct) {
                return tys.intern_const(crate::consts::ConstKind::Alias(crate::consts::IsRigid::No, alias));
            }
        }
        ct
    }
}
fn set_projection_term_to_non_rigid(tys: &Types, clauses: &mut [Predicate]) {
    for clause in clauses {
        if let PredicateKind::Clause(ClauseKind::Projection(projection)) = &mut clause.kind {
            projection.term = fold::fold_term(tys, projection.term, &mut Rigidness { types_rigid: false, opaques_only: false }, 1);
        }
    }
}
struct ReplaceInferAndNonRigidAlias<'a, 'tcx> { infcx: &'a InferCtxt<'tcx>, replace_regions: bool }
impl TypeFolder for ReplaceInferAndNonRigidAlias<'_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        let ty = super_fold_ty(tys, self.infcx.shallow_resolve(ty), self, depth);
        match tys.kind(ty) {
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Alias(crate::consts::IsRigid::No, _) => tys.error,
            _ => ty,
        }
    }
    fn fold_const(&mut self, tys: &Types, ct: crate::consts::Const, depth: u32) -> crate::consts::Const {
        let ct = crate::types::fold::super_fold_const(tys, self.infcx.shallow_resolve_const(ct), self, depth);
        match tys.const_kind(ct) {
            crate::consts::ConstKind::Infer(_) | crate::consts::ConstKind::Alias(crate::consts::IsRigid::No, _) => tys.intern_const(crate::consts::ConstKind::Error),
            _ => ct,
        }
    }
    fn fold_region(&mut self, tys: &Types, region: RegionId, _: u32) -> RegionId {
        if self.replace_regions && matches!(tys.region_kind(region), RegionKind::ReVar(_)) { tys.intern_region(RegionKind::ReError) } else { region }
    }
}
fn replace_infer_and_non_rigid_alias_with_error(infcx: &InferCtxt<'_>, clauses: Vec<Predicate>, replace_regions: bool) -> Vec<Predicate> {
    let mut folder = ReplaceInferAndNonRigidAlias { infcx, replace_regions };
    let mut result = Vec::with_capacity(clauses.len());
    for clause in clauses { result.push(fold::fold_predicate(&infcx.tcx.tys, &clause, &mut folder)); }
    result
}

/// rustc_trait_selection/src/traits/mod.rs:330-433, including its deliberate
/// ignoring of region errors and its delayed-bug recovery for unconstrained vars.
fn do_normalize_clauses(tcx: &Tcx, prog: &Program, cause: ObligationCause, elaborated_env: ParamEnv, clauses: Vec<Predicate>) -> Vec<Predicate> {
    let mut infcx = InferCtxt::for_item(tcx, prog, TypingMode::non_body_analysis(), cause.body_id);
    infcx.cause = cause;

    let original = clauses.clone();
    let mut rigid_bounds = Vec::new();
    for p in &*elaborated_env.clauses { rigid_bounds.push(fold::fold_predicate(&tcx.tys, p, &mut Rigidness { types_rigid: true, opaques_only: false })); }
    set_projection_term_to_non_rigid(&tcx.tys, &mut rigid_bounds);
    let elaborated_env = tcx.intern_param_env(ParamEnv { clauses: rigid_bounds.into() });
    let mut ocx = ObligationCtxt::new(&infcx, prog);
    let mut normalized = Vec::new();
    for p in clauses { normalized.push(ocx.normalize_predicate(&elaborated_env, &p)); }
    set_projection_term_to_non_rigid(&tcx.tys, &mut normalized);
    for p in &mut normalized { *p = fold::fold_predicate(&tcx.tys, p, &mut Rigidness { types_rigid: false, opaques_only: true }); }
    // (the next solver normalizes the environment lazily, rustc traits/mod.rs do_normalize_predicates returns
    // the predicates unchanged under `next_trait_solver_globally`: an environment that does not normalize
    // eagerly stays as it is written, there is nothing to report)
    if !ocx.engine.evaluate_obligations_error_on_ambiguity(&infcx, prog).is_empty() { return original; }
    // There are no assumed WF types here; this cannot fail to build the outlives env.
    let _ = infcx.resolve_regions(prog, tcx.intern_param_env(ParamEnv { clauses: normalized.clone().into() }), &[]);
    match infcx.deeply_resolve_clauses_via_region_graph(&normalized) {
        Ok(clauses) => {
            // A region variable no constraint reaches (an unconstrained impl lifetime) stays in the
            // clauses as in rustc's lexical resolution, but belongs to this inference context only:
            // another context cannot resolve it, so it becomes an error region (as the Err path below does).
            struct EraseVars;
            impl TypeFolder for EraseVars {
                fn fold_region(&mut self, tys: &Types, r: crate::types::RegionId, _: u32) -> crate::types::RegionId {
                    if matches!(tys.region_kind(r), crate::types::RegionKind::ReVar(_)) { tys.intern_region(crate::types::RegionKind::ReError) } else { r }
                }
            }
            clauses.iter().map(|c| fold::fold_predicate(&tcx.tys, c, &mut EraseVars)).collect()
        }
        Err(error) => {
            tcx.dcx.delayed_bug(cause, format!("inference variables in normalized parameter environment: {error:?}"));
            replace_infer_and_non_rigid_alias_with_error(&infcx, normalized, true)
        }
    }
}

/// rustc_trait_selection/src/traits/mod.rs:441-580: normalize non-outlives
/// clauses first, then TypeOutlives using the partially normalized environment.
pub fn normalize_param_env_or_error(tcx: &Tcx, prog: &Program, env: ParamEnv, cause: ObligationCause) -> ParamEnv {
    let mut clauses = Vec::new();
    let mut seen = crate::fxhash::HashSet::default();
    for p in &*env.clauses {
        for p in tcx.supertraits(prog, p.clone()) { if seen.insert(p.clone()) { clauses.push(p); } }
    }
    struct HasAliases(bool);
    impl TypeFolder for HasAliases {
        fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
            self.0 |= matches!(tys.kind(ty), TyKind::Alias(..)); super_fold_ty(tys, ty, self, depth)
        }
        fn fold_const(&mut self, tys: &Types, ct: crate::consts::Const, depth: u32) -> crate::consts::Const {
            self.0 |= matches!(tys.const_kind(ct), crate::consts::ConstKind::Alias(..)); crate::types::fold::super_fold_const(tys, ct, self, depth)
        }
    }
    let mut has_aliases = HasAliases(false);
    for p in &clauses { fold::fold_predicate(&tcx.tys, p, &mut has_aliases); }
    let elaborated_env = ParamEnv { clauses: clauses.clone().into() };
    if !has_aliases.0 { return elaborated_env; }
    let mut outlives = Vec::new();
    clauses.retain(|p| { if matches!(p.kind, PredicateKind::Clause(ClauseKind::TypeOutlives(..))) { outlives.push(p.clone()); false } else { true } });
    let mut clauses = do_normalize_clauses(tcx, prog, cause, elaborated_env, clauses);
    let mut outlives_env = clauses.clone(); outlives_env.extend_from_slice(&outlives);
    clauses.extend(do_normalize_clauses(tcx, prog, cause, ParamEnv { clauses: outlives_env.into() }, outlives));
    ParamEnv { clauses: clauses.into() }
}
