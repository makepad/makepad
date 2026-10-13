// ported from N:rustc_infer/src/infer/snapshot/fudge.rs @8d1a764
use super::super::InferCtxt;
use crate::consts::{Const, ConstKind, InferConst};
use crate::types::{RegionId, RegionKind, TyId, TyKind, Types};
use crate::types::fold::{TypeFolder, super_fold_const, super_fold_ty};
use std::ops::Range;

#[derive(Clone, Copy)]
pub struct VariableLengths {
    type_var_len: usize,
    region_constraints_len: usize,
    int_var_len: usize,
    float_var_len: usize,
    const_var_len: usize,
}

struct SnapshotVarData {
    type_vars: Range<u32>,
    region_vars: Range<u32>,
    int_vars: Range<u32>,
    float_vars: Range<u32>,
    const_vars: Range<u32>,
}

impl SnapshotVarData {
    fn new(infcx: &InferCtxt<'_>, before: VariableLengths) -> Self {
        let after = infcx.variable_lengths();
        Self {
            type_vars: before.type_var_len as u32..after.type_var_len as u32,
            region_vars: before.region_constraints_len as u32..after.region_constraints_len as u32,
            int_vars: before.int_var_len as u32..after.int_var_len as u32,
            float_vars: before.float_var_len as u32..after.float_var_len as u32,
            const_vars: before.const_var_len as u32..after.const_var_len as u32,
        }
    }
    fn is_empty(&self) -> bool {
        self.type_vars.is_empty() && self.region_vars.is_empty() && self.int_vars.is_empty()
            && self.float_vars.is_empty() && self.const_vars.is_empty()
    }
}

impl InferCtxt<'_> {
    pub fn variable_lengths(&self) -> VariableLengths {
        let mut inner = self.inner.borrow_mut();
        VariableLengths {
            type_var_len: inner.type_variables().num_vars(),
            region_constraints_len: inner.unwrap_region_constraints().num_region_vars(),
            int_var_len: inner.int_unification_table().len(),
            float_var_len: inner.float_unification_table().len(),
            const_var_len: inner.const_unification_table().len(),
        }
    }

    /// fudge.rs:110: resolve inside the probe, then replace popped variables in
    /// the expectation. Constraints from the probe never escape with the value.
    pub fn fudge_inference_if_ok<E>(&self, f: impl FnOnce() -> Result<TyId, E>) -> Result<TyId, E> {
        let before = self.variable_lengths();
        let (vars, value) = self.probe(|_| {
            let value = f()?;
            Ok((SnapshotVarData::new(self, before), self.deeply_resolve_ignoring_regions(value)))
        })?;
        if vars.is_empty() { return Ok(value); }
        Ok(InferenceFudger { infcx: self, snapshot_vars: vars }.fold_ty(&self.tcx.tys, value, 0))
    }
}

struct InferenceFudger<'a, 'tcx> { infcx: &'a InferCtxt<'tcx>, snapshot_vars: SnapshotVarData }

impl TypeFolder for InferenceFudger<'_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        match tys.kind(ty) {
            TyKind::Infer(v) if self.snapshot_vars.type_vars.contains(v) => self.infcx.next_ty_var(),
            TyKind::IntVar(v) if self.snapshot_vars.int_vars.contains(v) => self.infcx.next_int_var(),
            TyKind::FloatVar(v) if self.snapshot_vars.float_vars.contains(v) => self.infcx.next_float_var(),
            _ if tys.flags(ty).intersects(crate::types::TypeFlags::HAS_INFER) => super_fold_ty(tys, ty, self, depth),
            _ => ty,
        }
    }
    fn fold_region(&mut self, tys: &Types, region: RegionId, _: u32) -> RegionId {
        match tys.region_kind(region) {
            RegionKind::ReVar(v) if self.snapshot_vars.region_vars.contains(&v) => self.infcx.next_region_var(),
            _ => region,
        }
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        match tys.const_kind(ct) {
            ConstKind::Infer(InferConst::Var(v)) if self.snapshot_vars.const_vars.contains(&v.0) => self.infcx.next_const_var(),
            _ => super_fold_const(tys, ct, self, depth),
        }
    }
}
