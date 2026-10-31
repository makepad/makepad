// ported from N:rustc_borrowck/src/region_infer/opaque_types/member_constraints.rs @8d1a764
use crate::types::AliasTy;
use super::{DefiningUse, region_ctxt::RegionCtxt};
use crate::types::{Types, RegionKind, UniverseIndex};
use crate::types::fold::TypeFolder;

pub fn apply_member_constraints(rcx: &mut RegionCtxt<'_>, tcx: &crate::tcx::Tcx, prog: &crate::program::Program, defining_uses: &[DefiningUse]) {
    let tys = &tcx.tys;
    let mut member_constraints = vec![vec![]; rcx.constraint_sccs.num_sccs()];
    for (i, defining_use) in defining_uses.iter().enumerate() {
        let mut visitor = CollectMemberConstraintsVisitor { tcx, prog, rcx, members: &mut member_constraints, defining_use: i };
        visitor.fold_ty(tys, defining_use.hidden_type, 0);
    }
    for a in rcx.constraint_sccs.all_sccs() {
        for &b in rcx.constraint_sccs.successors(a) { rcx.scc_values.add_region(a,b); }
        for &i in &member_constraints[a] { apply_member_constraint(rcx, a, &defining_uses[i].arg_regions); }
    }
}
fn apply_member_constraint(rcx: &mut RegionCtxt<'_>, member: usize, arg_regions: &[usize]) {
    if rcx.scc_annotations[member].max_placeholder_universe_reached() != UniverseIndex::ROOT { return; }
    let mut choices: Vec<_> = arg_regions.iter().map(|&r| rcx.representative(r).rvid()).filter(|&choice| {
        rcx.scc_values.universal_regions_outlived_by(member).all(|lower| rcx.universal_region_relations.outlives(choice, lower))
    }).collect();
    for ub in rcx.rev_scc_graph.upper_bounds(member) { choices.retain(|&r| rcx.universal_region_relations.outlives(ub,r)); }
    let totally_ordered_subset = choices.iter().copied().filter(|&a| choices.iter().all(|&b| {
        rcx.universal_region_relations.outlives(a,b) || rcx.universal_region_relations.outlives(b,a)
    }));
    let Some(min_choice) = totally_ordered_subset.reduce(|a,b| match (
        rcx.universal_region_relations.outlives(a,b), rcx.universal_region_relations.outlives(b,a)) {
        (true,true) => a.min(b), (true,false) => b, (false,true) => a,
        _ => unreachable!("incomparable regions in total order"),
    }) else { return; };
    rcx.scc_values.add_region(member, rcx.constraint_sccs.scc(min_choice));
}
struct CollectMemberConstraintsVisitor<'a, 'b> {
    tcx: &'a crate::tcx::Tcx, prog: &'a crate::program::Program, rcx: &'a RegionCtxt<'b>, members: &'a mut [Vec<usize>], defining_use: usize,
}
impl TypeFolder for CollectMemberConstraintsVisitor<'_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: crate::types::TyId, depth: u32) -> crate::types::TyId {
        if !tys.flags(ty).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS) { return ty; }
        if let crate::types::TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: signature, tupled_upvars_ty: upvars, .. }) = tys.kind(ty) {
            // The owner supplies the parent generic arguments, which are not members.
            self.fold_ty(tys,*signature,depth);
            self.fold_ty(tys,*upvars,depth);
            ty
        } else if let Some((variance, args, rs)) = match tys.kind(ty) {
            crate::types::TyKind::Alias(_, AliasTy { kind, args, regions }) => self.tcx.alias_declared_variances(self.prog, *kind).map(|v| (Some(v), args, *regions)),
            _ => None,
        } {
            // member_constraints.rs:179-189: arguments of an alias with declared variances
            // (opaques, RPITITs) get member constraints unless bivariant (uncaptured).
            for (i,&t) in tys.list(*args).iter().enumerate() {
                if !super::bivariant(&variance,false,i) { self.fold_ty(tys,t,depth); }
            }
            for (i,&r) in tys.region_args(rs).iter().enumerate() {
                if !super::bivariant(&variance,true,i) { self.fold_region(tys,r,depth); }
            }
            ty
        } else { crate::types::fold::super_fold_ty(tys,ty,self,depth) }
    }
    fn fold_region(&mut self, tys: &Types, r: crate::types::RegionId, _: u32) -> crate::types::RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(..) => (),
            RegionKind::ReVar(v) => self.members[self.rcx.constraint_sccs.scc(v as usize)].push(self.defining_use),
            _ => panic!("opaque member was not renumbered"),
        }
        r
    }
}
