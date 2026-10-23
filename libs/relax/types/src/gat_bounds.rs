//! rustc_hir_analysis check/wfcheck.rs `check_gat_where_clauses` ("missing required bound on
//! `Gat`"): a generic associated type must declare the outlives bounds that every other item of
//! its trait which names it already implies (a method's signature, an associated type's bounds),
//! so impls keep the most flexibility (issue #87479). `makepad check` only.

use crate::next_solver::{ClauseKind, ParamEnv, ParamEnvId, Predicate, PredicateKind};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::{AliasTy, AliasTyKind, RegionId, RegionKind, TyId, TyKind};

/// The arguments of every use of `gat` in `tys` (GATArgsCollector): (region, its index in the
/// GAT's lifetime list) and (type, its index in the GAT's type list).
fn gat_args(tcx: &Tcx, gat: DefId, tys: &[TyId]) -> (Vec<(RegionId, usize)>, Vec<(TyId, usize)>) {
    struct V<'a> { tcx: &'a Tcx, gat: DefId, regions: Vec<(RegionId, usize)>, types: Vec<(TyId, usize)> }
    impl crate::types::fold::TypeFolder for V<'_> {
        fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: crate::types::DebruijnIndex) -> TyId {
            if let TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { def_id }, args, regions, .. }) = tys.kind(t) {
                if *def_id == self.gat {
                    for (i, r) in self.tcx.tys.region_args(*regions).iter().enumerate() {
                        if !matches!(tys.region_kind(*r), RegionKind::ReBound(..)) && !self.regions.contains(&(*r, i)) { self.regions.push((*r, i)); }
                    }
                    for (i, a) in tys.list(*args).iter().enumerate() {
                        if matches!(tys.kind(*a), TyKind::Const(_)) { continue; }
                        if !self.types.contains(&(*a, i)) { self.types.push((*a, i)); }
                    }
                }
            }
            crate::types::fold::super_fold_ty(tys, t, self, depth)
        }
    }
    let mut v = V { tcx, gat, regions: Vec::new(), types: Vec::new() };
    for &t in tys { crate::types::fold::TypeFolder::fold_ty(&mut v, &tcx.tys, t, 0); }
    (v.regions, v.types)
}

/// test_region_obligations: does the region check of `env` and the well-formed `wf` types
/// prove the constraint `add` registers?
fn holds(prog: &Program, tcx: &Tcx, item: DefId, env: &ParamEnvId, wf: &[TyId], add: impl FnOnce(&crate::infer::InferCtxt<'_>)) -> bool {
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), item);
    add(&infcx);
    infcx.resolve_regions(prog, env.clone(), wf).is_empty()
}

fn ty_known_to_outlive(prog: &Program, tcx: &Tcx, item: DefId, env: &ParamEnvId, wf: &[TyId], ty: TyId, r: RegionId) -> bool {
    holds(prog, tcx, item, env, wf, |infcx| infcx.register_ty_outlives(ty, r))
}

fn region_known_to_outlive(prog: &Program, tcx: &Tcx, item: DefId, env: &ParamEnvId, wf: &[TyId], a: RegionId, b: RegionId) -> bool {
    holds(prog, tcx, item, env, wf, |infcx| infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(&tcx.tys, crate::infer::region_constraints::SubregionOrigin::Misc, b, a, false))
}

fn clause(kind: ClauseKind) -> Predicate {
    Predicate { kind: PredicateKind::Clause(kind), bound_vars: Vec::new().into() }
}

/// gather_gat_bounds: the outlives clauses (over the GAT's own parameters) that `to_check`'s uses
/// of the GAT imply under `env`; None when it does not name the GAT.
fn gather(prog: &Program, tcx: &Tcx, item: DefId, env: &ParamEnvId, to_check: &[TyId], wf: &[TyId], gat: DefId) -> Option<Vec<Predicate>> {
    let (regions, types) = gat_args(tcx, gat, to_check);
    if regions.is_empty() && types.is_empty() { return None; }
    let indices = tcx.generic_indices_of(prog, gat);
    let mut out: Vec<Predicate> = Vec::new();
    let region_param = |i: usize| indices.lifetimes.get(i).map(|&d| tcx.tys.intern_region(RegionKind::ReEarlyParam(d)));
    for &(ra, ia) in &regions {
        if matches!(tcx.tys.region_kind(ra), RegionKind::ReStatic | RegionKind::ReError) { continue; }
        let Some(pa) = region_param(ia) else { continue };
        for &(ty, it) in &types {
            if ty_known_to_outlive(prog, tcx, item, env, wf, ty, ra) {
                let p = clause(ClauseKind::TypeOutlives(tcx.tys.intern(TyKind::Param(it as u32)), pa));
                if !out.contains(&p) { out.push(p); }
            }
        }
        for &(rb, ib) in &regions {
            if matches!(tcx.tys.region_kind(rb), RegionKind::ReStatic | RegionKind::ReError) || ra == rb { continue; }
            let Some(pb) = region_param(ib) else { continue };
            if region_known_to_outlive(prog, tcx, item, env, wf, ra, rb) {
                let p = clause(ClauseKind::RegionOutlives(pa, pb));
                if !out.contains(&p) { out.push(p); }
            }
        }
    }
    Some(out)
}

pub fn check(prog: &Program, tcx: &Tcx, trait_def: DefId) -> Vec<String> {
    let items: Vec<DefId> = (0..prog.defs.len() as u32).map(DefId)
        .filter(|&d| { let x = prog.def(d); x.parent == trait_def && matches!(x.kind, DefKind::AssocFn | DefKind::AssocTy) })
        .collect();
    let gats: Vec<DefId> = items.iter().copied().filter(|&d| prog.def(d).kind == DefKind::AssocTy && {
        let own = tcx.generic_indices_of(prog, d);
        let parent = tcx.generic_indices_of(prog, trait_def);
        own.lifetimes.len() + own.types_consts.len() > parent.lifetimes.len() + parent.types_consts.len()
    }).collect();
    if gats.is_empty() { return Vec::new(); }
    let mut required: Vec<(DefId, Vec<Predicate>)> = Vec::new();
    loop {
        let mut again = false;
        for &gat in &gats {
            let mut new: Option<Vec<Predicate>> = None;
            for &item in &items {
                if item == gat { continue; }
                let env = tcx.param_env_id(prog, item);
                let found = match prog.def(item).kind {
                    DefKind::AssocFn => {
                        let Some(sig) = tcx.fn_sig(prog, item) else { continue };
                        let lib = |t: TyId| crate::next_solver::dyn_compatibility::liberate_late_bound_regions(tcx, item, t);
                        let inputs: Vec<TyId> = sig.params.iter().map(|&t| lib(t)).collect();
                        let mut all = inputs.clone();
                        all.push(lib(sig.ret));
                        gather(prog, tcx, item, &env, &all, &inputs, gat)
                    }
                    _ => {
                        // augment_param_env: the bounds already found missing on this GAT
                        let env = match required.iter().find(|(d, _)| *d == item) {
                            Some((_, extra)) if !extra.is_empty() => {
                                let mut clauses: Vec<Predicate> = tcx.param_env_by_id(env.clone()).clauses.to_vec();
                                clauses.extend(extra.iter().cloned());
                                tcx.intern_param_env(ParamEnv { clauses: clauses.into() })
                            }
                            _ => env,
                        };
                        let mut tys = Vec::new();
                        for b in tcx.explicit_item_bounds(prog, item) { collect_tys(&b.kind, &mut tys); }
                        gather(prog, tcx, item, &env, &tys, &[], gat)
                    }
                };
                if let Some(found) = found {
                    match &mut new {
                        Some(n) => n.retain(|b| found.contains(b)),
                        None => new = Some(found),
                    }
                }
            }
            if let Some(new) = new {
                let slot = match required.iter().position(|(d, _)| *d == gat) {
                    Some(i) => i,
                    None => { required.push((gat, Vec::new())); required.len() - 1 }
                };
                for p in new {
                    if !required[slot].1.contains(&p) { required[slot].1.push(p); again = true; }
                }
            }
        }
        if !again { break; }
    }
    let mut errors = Vec::new();
    for (gat, bounds) in required {
        if prog.impl_trait_in_trait_data.contains_key(&gat) { continue; }
        let env = tcx.param_env_id(prog, gat);
        let missing = bounds.iter().filter(|p| match &p.kind {
            PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)) => !region_known_to_outlive(prog, tcx, gat, &env, &[], *a, *b),
            PredicateKind::Clause(ClauseKind::TypeOutlives(t, r)) => !ty_known_to_outlive(prog, tcx, gat, &env, &[], *t, *r),
            _ => false,
        }).count();
        if missing == 0 { continue; }
        let def = prog.def(gat);
        let f = &prog.files[def.file as usize];
        let item = f.ast.item(def.item);
        errors.push(prog.record_error(def.file, item.lo, "typeck", format!("missing required bound{} on `{}`", if missing > 1 { "s" } else { "" }, f.text(item.name))));
    }
    errors
}

/// The types of a clause (explicit_item_bounds are visited as a whole by rustc's collector).
fn collect_tys(k: &PredicateKind, out: &mut Vec<TyId>) {
    use crate::consts::GenericArg;
    match k {
        PredicateKind::Clause(ClauseKind::Trait(t)) => { for a in t.trait_ref.args.iter() { if let GenericArg::Type(t) = a { out.push(*t); } } }
        PredicateKind::Clause(ClauseKind::Projection(p)) => {
            if let crate::next_solver::AliasTerm::Ty(a) = &p.projection_term { out.push(*a); }
            if let crate::next_solver::Term::Ty(t) = &p.term { out.push(*t); }
        }
        PredicateKind::Clause(ClauseKind::TypeOutlives(t, _)) => out.push(*t),
        _ => {}
    }
}
