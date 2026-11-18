// ported from N:rustc_type_ir/src/outlives.rs @8d1a764
use crate::tcx::Tcx;
use crate::program::{DefId, Program};
use crate::types::{AliasTy, RegionId, RegionKind, TyId, TyKind, Types};
use crate::types::fold::{TypeFolder, super_fold_ty};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub enum Component { Region(RegionId), Param(TyId), Placeholder(TyId), Alias(TyId), EscapingAlias(Vec<Component>), UnresolvedInferenceVariable(TyId) }
pub fn push_outlives_components(tcx: &Tcx, prog: &Program, ty: TyId, out: &mut Vec<Component>) {
    OutlivesCollector { tcx, prog, out, visited: HashSet::new() }.fold_ty(&tcx.tys, ty, 0);
}
struct OutlivesCollector<'a> { tcx: &'a Tcx, prog: &'a Program, out: &'a mut Vec<Component>, visited: HashSet<TyId> }
impl TypeFolder for OutlivesCollector<'_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if !self.visited.insert(ty) { return ty; }
        match tys.kind(ty) {
            TyKind::FnDef(_, args, _) => { for &t in tys.list(*args) { self.fold_ty(tys, t, depth); } }
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: upvars, .. }) => { self.fold_ty(tys, *upvars, depth); }
            TyKind::Param(_) => self.out.push(Component::Param(ty)),
            TyKind::Placeholder(_) => self.out.push(Component::Placeholder(ty)),
            TyKind::Alias(_, alias) => {
                if !tys.has_vars_bound_at_or_above(ty, 0) { self.out.push(Component::Alias(ty)); }
                else { let mut components = Vec::new(); compute_alias_components_recursive(self.tcx, self.prog, alias, &mut components); self.out.push(Component::EscapingAlias(components)); }
            }
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => self.out.push(Component::UnresolvedInferenceVariable(ty)),
            TyKind::Bound(..) => {},
            _ => { super_fold_ty(tys, ty, self, depth); }
        }
        ty
    }
    fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId {
        if !matches!(tys.region_kind(r), RegionKind::ReBound(..)) { self.out.push(Component::Region(r)); } r
    }
}

pub fn compute_alias_components_recursive(tcx: &Tcx, prog: &Program, alias: &AliasTy, out: &mut Vec<Component>) {
    let mut collector = OutlivesCollector { tcx, prog, out, visited: HashSet::new() };
    for &ty in tcx.tys.list(alias.args) { collector.fold_ty(&tcx.tys, ty, 0); }
    for (index, &r) in tcx.tys.region_args(alias.regions).iter().enumerate() {
        if alias_region_is_bivariant(tcx, prog, alias, index) { continue; }
        collector.fold_region(&tcx.tys, r, 0);
    }
}

/// rustc_hir_analysis/src/variance/mod.rs:98-239: declaration-computed variance.
pub fn alias_region_is_bivariant(tcx: &Tcx, prog: &Program, alias: &AliasTy, index: usize) -> bool {
    if let Some(variances) = tcx.opt_alias_variances(prog, alias.kind) {
        let indices = tcx.generic_indices_of(prog, alias.kind.def_id());
        return variances[indices.lifetimes[index] as usize] == crate::types::variance::V::Bi;
    }
    false
}

/// The callable whose signature supplies an advisory lifetime requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Callable { FnItem(DefId), Closure }

/// Collect free signature regions separately, so the borrow check keeps its ordinary outlives components.
pub fn push_signature_regions(tcx: &Tcx, prog: &Program, ty: TyId, out: &mut Vec<(RegionId, Callable)>) {
    SignatureCollector { tcx, prog, out, visited: HashSet::new() }.fold_ty(&tcx.tys, ty, 0);
}
struct SignatureCollector<'a> { tcx: &'a Tcx, prog: &'a Program, out: &'a mut Vec<(RegionId, Callable)>, visited: HashSet<TyId> }
impl TypeFolder for SignatureCollector<'_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if !self.visited.insert(ty) { return ty; }
        match tys.kind(ty) {
            TyKind::FnDef(def, args, regions) => {
                for &t in tys.list(*args) { self.fold_ty(tys, t, depth); }
                let tcx = self.tcx;
                if let Some(sig) = tcx.fn_item_sig(self.prog, *def) {
                    let args = tcx.complete_args(self.prog, *def, tys.list(*args), *regions);
                    let mut collector = SignatureRegions { callable: Callable::FnItem(*def), out: self.out, visited: HashSet::new() };
                    for &t in sig.params.iter().chain(std::iter::once(&sig.ret)) {
                        let t = tcx.instantiate_ty(self.prog, *def, &args, t);
                        collector.fold_ty(tys, t, depth);
                    }
                }
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: upvars, closure_sig_as_fn_ptr_ty: sig, .. }) => {
                self.fold_ty(tys, *upvars, depth);
                SignatureRegions { callable: Callable::Closure, out: self.out, visited: HashSet::new() }.fold_ty(tys, *sig, depth);
            }
            TyKind::Alias(_, alias) => {
                // Match OutlivesCollector: only escaping aliases expose their type arguments here.
                if tys.has_vars_bound_at_or_above(ty, 0) { for &t in tys.list(alias.args) { self.fold_ty(tys, t, depth); } }
            }
            TyKind::Param(_) | TyKind::Placeholder(_) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Bound(..) => {},
            _ => { super_fold_ty(tys, ty, self, depth); }
        }
        ty
    }
}

struct SignatureRegions<'a> { callable: Callable, out: &'a mut Vec<(RegionId, Callable)>, visited: HashSet<TyId> }
impl TypeFolder for SignatureRegions<'_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if self.visited.insert(ty) { super_fold_ty(tys, ty, self, depth); }
        ty
    }
    fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId {
        // A signature contributes only free regions; bound regions and type parameters add no requirement.
        if !matches!(tys.region_kind(r), RegionKind::ReBound(..)) && !self.out.contains(&(r, self.callable)) { self.out.push((r, self.callable)); }
        r
    }
}
