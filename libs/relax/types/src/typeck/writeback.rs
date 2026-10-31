// ported from N:rustc_hir_typeck/src/writeback.rs @8d1a764
use super::*;

struct WritebackCx<'a, 'tcx> {
    infcx: &'a crate::infer::InferCtxt<'tcx>,
    prog: &'a Program,
    param_env: crate::next_solver::ParamEnvId,
    normalize: bool,
    unresolved: bool,
}
impl WritebackCx<'_, '_> {
    fn resolve(&mut self, ty: &mut TyId) {
        // writeback.rs:968-992: fulfillment may have unlocked projections.
        if self.normalize {
            let mut ocx = crate::next_solver::fulfill::ObligationCtxt::new(self.infcx, self.prog);
            *ty = match ocx.deeply_normalize_ty(&self.param_env, *ty) {
                Ok(ty) => ty,
                Err(_) => { self.unresolved = true; self.infcx.tcx.tys.error }
            };
        } else {
            *ty = self.infcx.deeply_resolve_ignoring_regions(*ty);
        }
        // writeback.rs Resolver::fold_region: the typeck's region variables mean nothing outside it; they are erased, and
        // borrowck takes each erased region of a closure's signature as an external universal region of the closure.
        let tys = &self.infcx.tcx.tys;
        *ty = tys.map_regions(*ty, &|r, _| if matches!(tys.region_kind(r), crate::types::RegionKind::ReVar(_)) { tys.intern_region(crate::types::RegionKind::ReErased) } else { r }, 0);
        if self.infcx.tcx.tys.has_infer(*ty) {
            self.unresolved = true;
            *ty = self.infcx.tcx.tys.error;
        }
        // rustc_hir_typeck/src/writeback.rs:868-872: taint final results containing errors.
        if self.infcx.tcx.tys.flags(*ty).intersects(crate::types::TypeFlags::HAS_ERROR) { self.infcx.set_tainted_by_errors(); }
    }
    fn resolve_args(&mut self, args: &mut [TyId]) { for ty in args { self.resolve(ty); } }
    fn resolve_list(&mut self, args: &mut crate::types::TyList) {
        if args.is_empty() { return; }
        let mut v = self.infcx.tcx.tys.list(*args).to_vec();
        self.resolve_args(&mut v);
        *args = self.infcx.tcx.tys.mk_list(&v);
    }
    fn resolve_place(&mut self, place: &mut crate::expr_use::Place) {
        self.resolve(&mut place.root_ty);
        for (_, ty) in &mut place.projections { self.resolve(ty); }
    }
}
impl Fcx<'_> {
    /// Resolve after fulfillment. Body regions belong to borrowck and remain
    /// in typeck(def) for the BIR borrowck consumer (round 28 D6). Nightly
    /// writeback.rs:997-1006 erases these; Relax erases the codegen copy in
    /// instantiate_mir_and_normalize_erasing_regions, never the borrowck input.
    pub(super) fn resolve_type_vars_in_body(&mut self) {
        let mut wbcx = WritebackCx { infcx: &self.infcx, prog: self.prog, param_env: self.param_env, normalize: self.b.errors.is_empty(), unresolved: false };
        let body = &mut self.b;
        wbcx.resolve_args(&mut body.args);
        // Nightly writeback visits populated node_types, not untyped AST arena slots.
        for i in 0..body.expr_ty.len() { if body.expr_typed.get(i) { wbcx.resolve(&mut body.expr_ty[i]); } }
        for i in 0..body.pat_ty.len() { if body.pat_typed.get(i) { wbcx.resolve(&mut body.pat_ty[i]); } }
        wbcx.resolve(&mut body.ret);
        for ty in body.hidden_types.values_mut() { wbcx.resolve(ty); }
        for (_, ty, _) in body.awaits.iter_mut() { wbcx.resolve(ty); }
        for local in &mut body.locals { wbcx.resolve(&mut local.ty); }
        for args in body.res_args.values_mut() { wbcx.resolve_list(args); }
        for method in body.methods.values_mut().chain(body.binops.values_mut()) { wbcx.resolve_list(&mut method.args); }
        for (_, args) in body.ov_index.values_mut().chain(body.mutable_index.values_mut()).chain(body.std_index.values_mut()).chain(body.for_next.values_mut()) { wbcx.resolve_list(args); }
        for (_, args, ty) in body.for_into.values_mut() { wbcx.resolve_list(args); wbcx.resolve(ty); }
        for (_, args, ty) in body.try_branch.values_mut() { wbcx.resolve_list(args); wbcx.resolve(ty); }
        for (_, args) in body.try_from_residual.values_mut() { wbcx.resolve_list(args); }
        for adjust in body.adjust_payloads_mut() {
            wbcx.resolve(&mut adjust.target);
            match &mut adjust.kind {
                adjustments::Adjust::Deref(Some((_, args))) => wbcx.resolve_list(args),
                adjustments::Adjust::Coerce(Coerce::ToDyn(a, b) | Coerce::Upcast(a, b)) => { wbcx.resolve(a); wbcx.resolve(b); }
                _ => {}
            }
        }
        for sig in body.closure_sigs.values_mut() {
            wbcx.resolve(&mut sig.bound); wbcx.resolve(&mut sig.liberated);
            if let Some((a, b)) = &mut sig.annotation_eq { wbcx.resolve(a); wbcx.resolve(b); }
        }
        for call in body.call_traits.values_mut() { wbcx.resolve(&mut call.self_ty); wbcx.resolve(&mut call.tupled_args); }
        for ty in body.inline_const_types.values_mut() { wbcx.resolve(ty); }
        for captures in body.min_captures.values_mut() {
            for capture in &mut captures.captures { wbcx.resolve_place(&mut capture.place); }
            for (place, _, _) in &mut captures.fake_reads { wbcx.resolve_place(place); }
            if let Some(coroutine) = &mut captures.coroutine {
                for capture in &mut coroutine.by_move { wbcx.resolve_place(&mut capture.place); }
                for capture in &mut coroutine.child_captures {
                    for (_, ty) in &mut capture.projections { wbcx.resolve(ty); }
                }
            }
        }
        if wbcx.unresolved && self.b.errors.is_empty() { self.err(self.last_lo, "E0282: type annotations needed".into()); }
    }
}
