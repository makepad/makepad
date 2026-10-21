// ported from N:rustc_hir_analysis/src/collect/generics_of.rs @8d1a764
// ported from N:rustc_hir_analysis/src/collect/resolve_bound_vars.rs @8d1a764
// ported from N:rustc_hir_analysis/src/collect/clauses_of.rs @8d1a764
// ported from N:rustc_hir_analysis/src/collect/item_bounds.rs @8d1a764
use super::*;
use crate::program::{DefKind, Program};
use crate::tcx::{Tcx, EvalCx};
use crate::types::{Types, TyKind, AliasTy, AliasTyKind, RegionKind};
use crate::types::variance::GenericIndices;
use crate::types::fold::{TypeFolder, super_fold_ty};
use crate::consts::IsRigid;
use relax_front::assoc::ImplTraitInTraitData;

#[derive(Clone, Hash)]
pub struct OpaqueGenerics {
    pub parent: GenericIndices,
    pub captured_lifetimes: Vec<RegionId>,
}

impl Tcx {
    pub(crate) fn opaque_generics(&self, prog: &Program, opaque: DefId) -> OpaqueGenerics {
        crate::type_queries::Providers { tcx: self, prog }.opaque_generics(opaque).expect("opaque_generics query")
    }
    pub(crate) fn opaque_generics_uncached(&self, prog: &Program, opaque: DefId) -> OpaqueGenerics {
        let table = self.resolve_bound_vars(prog, prog.opaque_owner(opaque));
        let mut captured_lifetimes = Vec::new();
        for &(arg, _) in table.opaque_captured_lifetimes.get(&opaque).expect("collected opaque captures") {
            captured_lifetimes.push(self.tys.intern_region(table.region_for_arg(arg)));
        }
        OpaqueGenerics { parent: table.generic_indices.clone(), captured_lifetimes }
    }

    /// `map_opaque_lifetime_to_parent_lifetime` of each lifetime the opaque captures.
    pub(crate) fn opaque_captured_parent_lifetimes(&self, prog: &Program, opaque: DefId) -> Vec<RegionId> {
        let table = self.resolve_bound_vars(prog, prog.opaque_owner(opaque));
        table.opaque_captured_parent_lifetimes(opaque, prog.opaque_owner(opaque)).into_iter().map(|k| self.tys.intern_region(k)).collect()
    }

    /// hir_ty_lowering/mod.rs:3631-3690: opaque arguments use the resolved
    /// capture map. Lowering never discovers or publishes declaration inputs.
    pub(crate) fn lower_opaque_ty(&self, prog: &Program, opaque: DefId) -> TyId {
        let declaration = self.opaque_generics(prog, opaque);
        let own_def = prog.opaque_to_rpitit.get(&opaque).copied().unwrap_or(opaque);
        let kind = if own_def == opaque { AliasTyKind::Opaque { def_id: opaque } } else { AliasTyKind::Projection { def_id: own_def } };
        let args = declaration.parent.identity_types_consts(&self.tys);
        let mut regions = Vec::new();
        for &index in &declaration.parent.lifetimes { regions.push(self.tys.intern_region(RegionKind::ReEarlyParam(index))); }
        regions.extend_from_slice(&declaration.captured_lifetimes);
        self.tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind, args: self.tys.mk_list(&args), regions: self.tys.intern_region_args(&regions) }))
    }

    pub(crate) fn rpitit_generic_indices(&self, prog: &Program, def_id: DefId) -> Option<GenericIndices> {
        let opaque = match prog.impl_trait_in_trait_data.get(&def_id) {
            Some(ImplTraitInTraitData::Trait { opaque_def_id, .. }) => *opaque_def_id,
            Some(ImplTraitInTraitData::Impl { fn_def_id }) => {
                let trait_assoc = prog.trait_item_def_id[&def_id];
                let trait_indices = self.generic_indices_of(prog, trait_assoc);
                let trait_parent = self.generic_indices_of(prog, prog.def(trait_assoc).parent);
                let mut indices = self.generic_indices_of(prog, prog.def(*fn_def_id).parent).clone();
                let old_count = (trait_parent.types_consts.len() + trait_parent.lifetimes.len()) as u32;
                let new_count = (indices.types_consts.len() + indices.lifetimes.len()) as u32;
                for &i in &trait_indices.lifetimes { if i >= old_count { indices.lifetimes.push(i - old_count + new_count); } }
                for &i in &trait_indices.types_consts { if i >= old_count { indices.types_consts.push(i - old_count + new_count); } }
                for &i in &trait_indices.consts { if i >= old_count { indices.consts.push(i - old_count + new_count); } }
                return Some(indices);
            }
            None if prog.def(def_id).kind == DefKind::OpaqueTy => def_id,
            None => return None,
        };
        let declaration = self.opaque_generics(prog, opaque);
        let mut indices = declaration.parent;
        let offset = (indices.lifetimes.len() + indices.types_consts.len()) as u32;
        for index in 0..declaration.captured_lifetimes.len() { indices.lifetimes.push(offset + index as u32); }
        Some(indices)
    }
}

impl Tcx {
    pub(crate) fn rpitit_clauses(&self, prog: &Program, def: DefId) -> Option<Vec<Predicate>> {
        match *prog.impl_trait_in_trait_data.get(&def)? {
            ImplTraitInTraitData::Trait { fn_def_id, opaque_def_id } => {
                let mut clauses = self.clauses_of(prog, fn_def_id);
                let data = self.opaque_generics(prog, opaque_def_id);
                let offset = (data.parent.lifetimes.len() + data.parent.types_consts.len()) as u32;
                for (index, &orig) in data.captured_lifetimes.iter().enumerate() {
                    if !matches!(self.tys.region_kind(orig), RegionKind::ReEarlyParam(_)) { continue; }
                    let dup = self.tys.intern_region(RegionKind::ReEarlyParam(offset + index as u32));
                    clauses.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(orig, dup))));
                    clauses.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(dup, orig))));
                }
                Some(clauses)
            }
            ImplTraitInTraitData::Impl { fn_def_id } => {
                let impl_def = prog.def(fn_def_id).parent;
                let trait_assoc = prog.trait_item_def_id[&def];
                let parent = self.generic_indices_of(prog, impl_def);
                let count = parent.lifetimes.len() + parent.types_consts.len();
                let mut args = self.impl_trait_ref(prog, impl_def, &parent.identity_args(&self.tys)).args;
                args.extend_from_slice(&self.generic_indices_of(prog, def).identity_args(&self.tys)[count..]);
                let mut clauses = self.clauses_of(prog, impl_def);
                for p in self.own_clauses_of(prog, trait_assoc) { clauses.push(self.instantiate_predicate(prog, trait_assoc, &args, &p)); }
                Some(clauses)
            }
        }
    }

    pub fn own_clauses_of(&self, prog: &Program, def: DefId) -> &[Predicate] {
        &self.declaration_queries(prog, def).own_clauses.get_or_init_cyclic(|| self.own_clauses_uncached(prog, def)).0
    }

    /// The errors of lowering the definition's clauses (see `own_clauses_uncached`).
    pub fn own_clauses_errors(&self, prog: &Program, def: DefId) -> &[String] {
        &self.declaration_queries(prog, def).own_clauses.get_or_init_cyclic(|| self.own_clauses_uncached(prog, def)).1
    }

    /// rustc_hir_analysis/src/collect/item_bounds.rs: elaboration belongs to the provider.
    pub fn item_bounds(&self, prog: &Program, def_id: DefId) -> &[Predicate] {
        self.declaration_queries(prog, def_id).item_bounds.get_or_init_cyclic(|| self.item_bounds_uncached(prog, def_id))
    }
    fn item_bounds_uncached(&self, prog: &Program, def_id: DefId) -> Vec<Predicate> {
        let mut out = Vec::new();
        for bound in self.explicit_item_bounds(prog, def_id) {
            for predicate in self.supertraits(prog, bound.clone()) {
                if !out.contains(&predicate) { out.push(predicate); }
            }
        }
        out
    }
    pub fn explicit_item_bounds(&self, prog: &Program, def_id: DefId) -> &[Predicate] {
        self.declaration_queries(prog, def_id).explicit_item_bounds.get_or_init_cyclic(|| self.explicit_item_bounds_uncached(prog, def_id))
    }
    pub(crate) fn explicit_item_bounds_uncached(&self, prog: &Program, def_id: DefId) -> Vec<Predicate> {
        match prog.impl_trait_in_trait_data.get(&def_id) {
            Some(ImplTraitInTraitData::Trait { opaque_def_id, .. }) => return self.opaque_explicit_bounds(prog, *opaque_def_id),
            Some(ImplTraitInTraitData::Impl { .. }) => return Vec::new(),
            None => {}
        }
        if prog.def(def_id).kind == DefKind::OpaqueTy {
            let mut bounds = self.opaque_explicit_bounds(prog, def_id);
            let mut folder = AssocTyToOpaque { tcx: self, prog, fn_def_id: prog.opaque_owner(def_id) };
            for predicate in &mut bounds { *predicate = fold::fold_predicate(&self.tys, predicate, &mut folder); }
            return bounds;
        }
        self.lower_assoc_item_bounds(prog, def_id, &crate::tcx::EvalState::new())
    }

    /// The bounds written on an associated type (`type A: Bound;`), lowered with `state`'s
    /// error capture.
    fn lower_assoc_item_bounds(&self, prog: &Program, def_id: DefId, state: &crate::tcx::EvalState) -> Vec<Predicate> {
        let def = prog.def(def_id);
        let ast = &prog.files[def.file as usize].ast;
        let Some(&crate::ast::TypeAliasItem { bounds, .. }) = ast.as_alias(&ast.item(def.item).kind) else { return Vec::new(); };
        let mut env = self.declaration_env(prog, def_id);
        let indices = self.generic_indices_of(prog, def_id);
        let mut regions = Vec::new();
        for &i in &indices.lifetimes { regions.push(self.tys.intern_region(RegionKind::ReEarlyParam(i))); }
        let alias = self.tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id }, args: self.tys.mk_list(&indices.identity_types_consts(&self.tys)), regions: self.tys.intern_region_args(&regions) }));
        env.regions = env.regions.with_binder(None);
        let cx = EvalCx::new(prog, self, crate::tcx::TypingEnv::fully_monomorphized(&prog), state);
        let mut bounds_out = Vec::new();
        self.lower_bounds(&cx, def_id, &env, alias, bounds, true, super::collect::PredicateFilter::All, &mut bounds_out);
        // item_bounds.rs:72-90: also collect `where Self::Assoc: Trait` from the parent trait's where clauses
        let trait_def = prog.def(def_id).parent;
        if prog.def(trait_def).kind == DefKind::Trait {
            for clause in self.own_clauses_of(prog, trait_def) {
                if let Some(clause) = self.remap_gat_vars_and_recurse_into_nested_projections(prog, trait_def, def_id, clause) {
                    if !bounds_out.contains(&clause) { bounds_out.push(clause); }
                }
            }
        }
        bounds_out
    }

    /// item_bounds.rs:112-232: a trait where clause whose self type is (a projection of) the associated
    /// item, `for<'a> Self::Gat<'a>: Bound<'b>` becoming a bound of the GAT over its own parameters.
    fn remap_gat_vars_and_recurse_into_nested_projections(&self, prog: &Program, trait_def: DefId, assoc_item: DefId, clause: &Predicate) -> Option<Predicate> {
        let mut clause_ty = match &clause.kind {
            PredicateKind::Clause(ClauseKind::Trait(t)) => match t.trait_ref.args.first()? { GenericArg::Type(t) => *t, _ => return None },
            PredicateKind::Clause(ClauseKind::Projection(p)) => {
                let AliasTerm::Ty(alias) = p.projection_term else { return None };
                let TyKind::Alias(_, a) = self.tys.kind(alias) else { return None };
                *self.tys.list(a.args).first()?
            }
            PredicateKind::Clause(ClauseKind::TypeOutlives(ty, _)) => *ty,
            _ => return None,
        };
        let trait_identity = self.generic_indices_of(prog, trait_def).identity_args(&self.tys);
        let gat_vars: Vec<GenericArg> = loop {
            let TyKind::Alias(_, a) = self.tys.kind(clause_ty) else { return None };
            let AliasTyKind::Projection { def_id: alias_def } = a.kind else { return None };
            let full = self.complete_args(prog, alias_def, self.tys.list(a.args), a.regions);
            if prog.def(alias_def).parent == trait_def && full[..trait_identity.len().min(full.len())] == trait_identity[..] && alias_def == assoc_item {
                break full[trait_identity.len()..].to_vec();
            }
            // a nested projection: its self type may be the item
            clause_ty = *self.tys.list(a.args).first()?;
        };
        if gat_vars.is_empty() { return Some(clause.clone()); }
        // every GAT argument must be a distinct bound variable of the clause's own binder
        let mut mapping: Vec<(u32, GenericArg)> = Vec::new();
        for (k, var) in gat_vars.iter().enumerate() {
            let index = (trait_identity.len() + k) as u32;
            let (bound, mapped) = match var {
                GenericArg::Lifetime(r) => match self.tys.region_kind(*r) {
                    RegionKind::ReBound(0, v) => (v.0, GenericArg::Lifetime(self.tys.intern_region(RegionKind::ReEarlyParam(index)))),
                    _ => return None,
                },
                GenericArg::Type(t) => match self.tys.kind(*t) {
                    TyKind::Bound(0, bt) => (bt.var.0, GenericArg::Type(self.tys.intern(TyKind::Param(index)))),
                    _ => return None,
                },
                GenericArg::Const(_) => return None,
            };
            if mapping.iter().any(|(b, _)| *b == bound) { return None; }
            mapping.push((bound, mapped));
        }
        struct MapAndCompress<'a> { tcx: &'a Tcx, mapping: Vec<(u32, GenericArg)>, still: Vec<(u32, BoundVariableKind)>, kinds: &'a [BoundVariableKind] }
        impl MapAndCompress<'_> {
            fn new_var(&mut self, old: u32) -> u32 {
                if let Some(i) = self.still.iter().position(|(o, _)| *o == old) { return i as u32; }
                self.still.push((old, self.kinds[old as usize].clone()));
                (self.still.len() - 1) as u32
            }
        }
        impl TypeFolder for MapAndCompress<'_> {
            fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
                if let RegionKind::ReBound(d, v) = tys.region_kind(r) {
                    if d + 1 == depth {
                        if let Some((_, GenericArg::Lifetime(m))) = self.mapping.iter().find(|(b, _)| *b == v.0) { return *m; }
                        let n = self.new_var(v.0);
                        return tys.intern_region(RegionKind::ReBound(d, crate::types::BoundVar(n)));
                    }
                }
                r
            }
            fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
                if let TyKind::Bound(d, bt) = tys.kind(t) {
                    if *d + 1 == depth {
                        if let Some((_, GenericArg::Type(m))) = self.mapping.iter().find(|(b, _)| *b == bt.var.0) { return *m; }
                        let n = self.new_var(bt.var.0);
                        return tys.intern(TyKind::Bound(*d, crate::types::BoundTy { var: crate::types::BoundVar(n), kind: bt.kind }));
                    }
                }
                super_fold_ty(tys, t, self, depth)
            }
        }
        let mut folder = MapAndCompress { tcx: self, mapping, still: Vec::new(), kinds: &clause.bound_vars };
        let _ = folder.tcx;
        let folded = fold::fold_predicate(&self.tys, clause, &mut folder);
        let bound_vars: Vec<BoundVariableKind> = folder.still.iter().map(|(_, k)| k.clone()).collect();
        Some(Predicate { kind: folded.kind, bound_vars: bound_vars.into() })
    }

    /// The errors of lowering an associated type's bounds (wfcheck lowers them for every item).
    pub(crate) fn assoc_item_bounds_errors(&self, prog: &Program, def_id: DefId) -> Vec<String> {
        let state = crate::tcx::EvalState::new();
        state.errors.replace(Some(Vec::new()));
        self.lower_assoc_item_bounds(prog, def_id, &state);
        state.errors.take().unwrap_or_default()
    }
}

/// rustc_hir_analysis/src/variance/mod.rs:98-239.
impl Tcx {
    pub(crate) fn variance_of_opaque(&self, prog: &Program, root: DefId) -> Vec<crate::types::variance::V> {
        let declaration = self.opaque_generics(prog, root);
        let parent = &declaration.parent;
        let captures = declaration.captured_lifetimes.len();
        let bounds = self.explicit_item_bounds(prog, root);
        struct OpaqueTypeLifetimeCollector<'a> { tcx: &'a Tcx, prog: &'a Program, root: DefId, used: Vec<u32> }
        impl TypeFolder for OpaqueTypeLifetimeCollector<'_> {
            fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId {
                if let RegionKind::ReEarlyParam(i) = tys.region_kind(r) { if !self.used.contains(&i) { self.used.push(i); } }
                r
            }
            fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
                if let TyKind::Alias(_, a) = tys.kind(t) {
                    if let AliasTyKind::Opaque { def_id } = a.kind {
                        let mut ancestor = def_id;
                        while ancestor != self.root && ancestor != crate::program::NO_DEF { ancestor = self.prog.def(ancestor).parent; }
                        if def_id != self.root && ancestor == self.root {
                            let indices = self.tcx.generic_indices_of(self.prog, def_id);
                            let variances = self.tcx.variances_of(self.prog, def_id);
                            for &t in tys.list(a.args) { self.fold_ty(tys, t, depth); }
                            for (i, &r) in tys.region_args(a.regions).iter().enumerate() { if variances[indices.lifetimes[i] as usize] != crate::types::variance::V::Bi { self.fold_region(tys, r, depth); } }
                            return t;
                        }
                    }
                }
                super_fold_ty(tys, t, self, depth)
            }
        }
        let mut collector = OpaqueTypeLifetimeCollector { tcx: self, prog, root, used: Vec::new() };
        for p in bounds {
            match &p.kind {
                PredicateKind::Clause(ClauseKind::Trait(t)) => { for &a in &t.trait_ref.args[1..] { fold::fold_arg(&self.tys, a, &mut collector, 1); } }
                PredicateKind::Clause(ClauseKind::Projection(p)) => {
                    let args = match &p.projection_term {
                        AliasTerm::Ty(t) => { let TyKind::Alias(_, a) = self.tys.kind(*t) else { unreachable!() }; self.complete_args(prog, a.kind.def_id(), self.tys.list(a.args), a.regions) }
                        AliasTerm::Const(a) => a.args.to_vec(),
                    };
                    for &a in &args[1..] { fold::fold_arg(&self.tys, a, &mut collector, 1); }
                    match p.term { Term::Ty(t) => { collector.fold_ty(&self.tys, t, 1); } Term::Const(c) => { collector.fold_const(&self.tys, c, 1); } }
                }
                PredicateKind::Clause(ClauseKind::TypeOutlives(_, r)) => { collector.fold_region(&self.tys, *r, 1); }
                _ => { fold::fold_predicate(&self.tys, p, &mut collector); }
            }
        }
        let owner = prog.opaque_owner(root);
        let container = prog.def(owner).parent;
        let trait_regions = if prog.def(container).kind == DefKind::Trait { self.generic_indices_of(prog, container).lifetimes.as_slice() } else { &[] };
        use crate::types::variance::V;
        let mut result = vec![V::Inv; parent.lifetimes.len() + parent.types_consts.len() + captures];
        for &index in &parent.lifetimes {
            if !trait_regions.contains(&index) && !collector.used.contains(&index) { result[index as usize] = V::Bi; }
        }
        result
    }
}

/// rustc_hir_analysis/src/collect/item_bounds.rs:545-571.
struct AssocTyToOpaque<'a> { tcx: &'a Tcx, prog: &'a Program, fn_def_id: DefId }
impl TypeFolder for AssocTyToOpaque<'_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Alias(_, alias) = tys.kind(ty) {
            if let AliasTyKind::Projection { def_id } = alias.kind {
                if let Some(ImplTraitInTraitData::Trait { fn_def_id, .. }) = self.prog.impl_trait_in_trait_data.get(&def_id) {
                    if *fn_def_id == self.fn_def_id {
                        let args = self.tcx.complete_args(self.prog, def_id, tys.list(alias.args), alias.regions);
                        return self.tcx.instantiate_ty(self.prog, def_id, &args, self.tcx.type_of(self.prog, def_id).expect("trait RPITIT type"));
                    }
                }
            }
        }
        super_fold_ty(tys, ty, self, depth)
    }
}

impl Tcx {
    /// item_bounds.rs: opaque bounds are lowered in their own early binder.
    fn opaque_explicit_bounds(&self, prog: &Program, opaque: DefId) -> Vec<Predicate> {
        let owner = prog.opaque_owner(opaque);
        let def = prog.def(owner);
        let ast = &prog.files[def.file as usize].ast;
        let declaration = self.opaque_generics(prog, opaque);
        let mut indices = declaration.parent;
        let offset = (indices.lifetimes.len() + indices.types_consts.len()) as u32;
        for i in 0..declaration.captured_lifetimes.len() { indices.lifetimes.push(offset + i as u32); }
        let own_def = prog.opaque_to_rpitit.get(&opaque).copied().unwrap_or(opaque);
        let kind = if own_def == opaque { AliasTyKind::Opaque { def_id: opaque } } else { AliasTyKind::Projection { def_id: own_def } };
        let target = self.alias_ty_with_args(kind, &indices.identity_args(&self.tys));
        let mut env = self.item_env(prog, owner);
        env.opaque_owner = Some(owner);
        env.regions = env.regions.with_binder(None);
        let state = crate::tcx::EvalState::new();
        let cx = EvalCx::new(prog, self, crate::tcx::TypingEnv { param_env: Default::default(), mode: crate::tcx::TypingMode::non_body_analysis(), limit: crate::tcx::RecursionLimit::for_item(prog, owner) }, &state);
        let mut result = Vec::new();
        match prog.def_id_to_node_id[&opaque].1 {
            relax_front::def_collector::NodeId::OpaqueTy(ty) => {
                let crate::ast::Ty::ImplTrait(bounds) = *ast.ty(ty) else { unreachable!() };
                self.lower_bounds(&cx, owner, &env, target, bounds, true, super::collect::PredicateFilter::All, &mut result);
            }
            relax_front::def_collector::NodeId::AsyncReturn(item) => {
                // item_bounds.rs: opaque types, including desugared async
                // returns, receive the implicit sizedness bounds.
                self.lower_bounds(&cx, owner, &env, target, Default::default(), true, super::collect::PredicateFilter::All, &mut result);
                let crate::ast::ItemKind::Fn(sig, _, _) = ast.item(item).kind else { unreachable!() };
                let output = ast.fn_sig(sig).ret.map_or(self.tys.unit, |ty| self.lower_ty(&cx, prog, def.file, self.item_module(prog, owner), &env, ty));
                if let (Some(future), Some(output_def)) = (self.lang_items.get(crate::lang_items::LangItem::Future), self.lang_items.get(crate::lang_items::LangItem::FutureOutput)) {
                    result.push(collect::trait_predicate(future, vec![GenericArg::Type(target)]));
                    let projection = self.tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: output_def }, args: self.tys.mk_list(&[target]), regions: crate::types::RegionArgsId::EMPTY }));
                    result.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(projection), term: Term::Ty(self.tys.shift_in(output, 1)) }))));
                }
            }
            _ => panic!("non-opaque declaration"),
        }
        result
    }
}
