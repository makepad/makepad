// ported from N:rustc_hir_typeck/src/opaque_types.rs @8d1a764
// ported from N:rustc_middle/src/ty/opaque_types.rs @8d1a764
// ported from N:rustc_ty_utils/src/opaque_types.rs @8d1a764
use super::*;
use crate::consts::{GenericArg, ConstKind, Const};
use crate::types::{Types, RegionId};
use crate::types::fold::{TypeFolder, super_fold_ty};
use crate::infer::opaque_types::{OpaqueTypeKey, ProvisionalHiddenType};

impl Tcx {
    /// opaque_types.rs:315-377: defining scopes contain opaque definitions,
    /// including nested return opaques and body children, rather than functions.
    pub fn opaque_types_defined_by(&self, prog: &Program, owner: DefId) -> crate::tcx::DefiningOpaquesId {
        self.intern_defining_opaques(prog.opaque_types_defined_by.get(&owner).cloned().unwrap_or_default())
    }
}
impl Fcx<'_> {
    /// opaque_types.rs:28-35: before fallback, defining uses may guide
    /// inference; nothing is reported yet.
    pub(super) fn try_handle_opaque_type_uses_next(&mut self) {
        let opaque_types = self.clone_opaque_types();
        self.compute_definition_site_hidden_types(opaque_types, false);
    }

    /// opaque_types.rs:46-54: after fallback and closure analysis.
    pub(super) fn handle_opaque_type_uses_next(&mut self) {
        let opaque_types = self.clone_opaque_types();
        let entries = self.infcx.inner.borrow_mut().opaque_types().num_entries();
        assert!(self.checked_opaque_types_storage_entries.replace(entries).is_none());
        self.compute_definition_site_hidden_types(opaque_types, true);
    }

    /// opaque_types.rs:277-291: normalization during writeback must not silently
    /// create unchecked defining uses. A delayed bug is an internal error here.
    pub(super) fn detect_opaque_types_added_during_writeback(&mut self) {
        let entries = self.checked_opaque_types_storage_entries.take().unwrap();
        let mut inner = self.infcx.inner.borrow_mut();
        let storage = inner.opaque_types();
        for (key, _) in storage.opaque_types_added_since(entries) {
            assert!(!self.b.errors.is_empty(), "unexpected cyclic definition of `{}`", self.prog.def_path(key.def_id));
        }
        // The inference context is dropped immediately after finish, releasing
        // the storage that nightly explicitly takes here.
    }

    /// rustc_infer/src/infer/mod.rs clone_opaque_types: the lookup table and
    /// the duplicate entries.
    fn clone_opaque_types(&self) -> Vec<(OpaqueTypeKey, ProvisionalHiddenType)> {
        let mut inner = self.infcx.inner.borrow_mut();
        let storage = inner.opaque_types();
        let mut values = storage.iter_lookup_table().to_vec();
        values.extend_from_slice(storage.iter_duplicate_entries());
        values
    }

    /// opaque_types.rs:91-220: per opaque the infcx may define, the first use
    /// with defining args and a hidden type free of inference variables
    /// determines the declaration's hidden type; every use is then equated with
    /// its instantiation. Without one, the second pass reports the missing use.
    fn compute_definition_site_hidden_types(&mut self, mut opaque_types: Vec<(OpaqueTypeKey, ProvisionalHiddenType)>, error_on_missing_defining_use: bool) {
        for (key, hidden) in opaque_types.iter_mut() {
            let mut args = Vec::with_capacity(key.args.len());
            for arg in key.args.iter() {
                args.push(match *arg {
                    GenericArg::Type(t) => GenericArg::Type(self.infcx.deeply_resolve_ignoring_regions(t)),
                    GenericArg::Const(c) => GenericArg::Const(self.tcx.tys.expect_const(self.infcx.deeply_resolve_ignoring_regions(self.tcx.tys.intern(TyKind::Const(c))))),
                    arg => arg,
                });
            }
            key.args = args.into();
            hidden.ty = self.infcx.deeply_resolve_ignoring_regions(hidden.ty);
        }
        let crate::tcx::TypingMode::Typeck { defining_opaque_types_and_generators } = self.infcx.typing_mode else { unreachable!() };
        let defining = self.tcx.defining_opaques(defining_opaque_types_and_generators).to_vec();
        for def_id in defining {
            match self.prog.def(def_id).kind { DefKind::OpaqueTy => {}, DefKind::Closure => continue, _ => unreachable!("not opaque or generator") }
            let mut usage_kind = UsageKind::None;
            for (key, hidden) in &opaque_types {
                if key.def_id != def_id { continue; }
                usage_kind.merge(self.consider_opaque_type_use(key, *hidden));
                if let UsageKind::HasDefiningUse(_) = usage_kind { break; }
            }
            if let UsageKind::HasDefiningUse(ty) = usage_kind {
                for (key, hidden) in &opaque_types {
                    if key.def_id != def_id { continue; }
                    let expected = self.tcx.instantiate_ty(self.prog, def_id, &key.args, ty);
                    // demand_eqtype
                    if !self.eq(expected, hidden.ty) {
                        let (a, b) = (self.ty_str(expected), self.ty_str(hidden.ty));
                        self.err(self.last_lo, format!("E0308: mismatched types: expected `{}`, found `{}`", a, b));
                    }
                }
                self.b.hidden_types.insert(def_id, ty);
            }
            if !error_on_missing_defining_use { continue; }
            match usage_kind {
                UsageKind::HasDefiningUse(_) => continue,
                // report_item_does_not_constrain_error; a body already tainted
                // reports nothing more (opaque_types.rs:162-170,186-188).
                UsageKind::None => if self.b.errors.is_empty() {
                    self.err(self.last_lo, "item does not constrain the opaque type, but has it in its signature".into());
                },
                UsageKind::NonDefiningUse => self.err(self.last_lo, "item does not constrain the opaque type, but has it in its signature (non-defining use)".into()),
                UsageKind::UnconstrainedHiddenType => if self.b.errors.is_empty() {
                    self.err(self.last_lo, "E0282: type annotations needed (cannot infer type of hidden type of opaque)".into());
                },
            }
            self.b.hidden_types.insert(def_id, self.tcx.tys.error);
        }
    }

    /// opaque_types.rs:222-262 with rustc_trait_selection/src/opaque_types.rs
    /// opaque_type_has_defining_use_args (HirTypeck: lifetimes are ignored).
    fn consider_opaque_type_use(&mut self, key: &OpaqueTypeKey, hidden: ProvisionalHiddenType) -> UsageKind {
        let mut seen = Vec::new();
        for arg in key.args.iter() {
            let is_param = match *arg {
                GenericArg::Lifetime(_) => continue,
                GenericArg::Type(t) => matches!(self.tcx.tys.kind(t), TyKind::Param(_)),
                GenericArg::Const(c) => matches!(self.tcx.tys.const_kind(c), ConstKind::Param(_)),
            };
            if !is_param || seen.contains(arg) { return UsageKind::NonDefiningUse; }
            seen.push(*arg);
        }
        if self.tcx.tys.has_infer(hidden.ty) { return UsageKind::UnconstrainedHiddenType; }
        let ty = {
            let mut ocx = crate::next_solver::fulfill::ObligationCtxt::new(&self.infcx, self.prog);
            ocx.deeply_normalize_ty(&self.param_env, hidden.ty)
        };
        let ty = match ty { Ok(ty) => ty, Err(_) => { self.err(self.last_lo, "E0277: the opaque's hidden type cannot be normalized".into()); self.tcx.tys.error } };
        // rustc_middle/src/ty/mod.rs remap_generic_params_to_declaration_params.
        let identity = self.tcx.generic_indices_of(self.prog, key.def_id).identity_args(&self.tcx.tys);
        let mut mapper = ReverseMapper { map: key.args.iter().copied().zip(identity).collect(), failed: false };
        let ty = mapper.fold_ty(&self.tcx.tys, ty, 0);
        if mapper.failed {
            self.err(self.last_lo, "generic parameter is part of the hidden type but not used in the opaque type's parameter list".into());
        }
        UsageKind::HasDefiningUse(ty)
    }
}

/// opaque_types.rs:56-89.
#[derive(Clone, Copy)]
enum UsageKind {
    None,
    // nightly keeps the use for its diagnostic span (minimal diagnostics here)
    NonDefiningUse,
    UnconstrainedHiddenType,
    HasDefiningUse(TyId),
}
impl UsageKind {
    fn merge(&mut self, other: UsageKind) {
        match (&*self, &other) {
            (UsageKind::HasDefiningUse(_), _) | (_, UsageKind::None) => unreachable!(),
            (UsageKind::None, _) => *self = other,
            (UsageKind::NonDefiningUse | UsageKind::UnconstrainedHiddenType, UsageKind::NonDefiningUse) => {}
            (UsageKind::NonDefiningUse | UsageKind::UnconstrainedHiddenType, UsageKind::UnconstrainedHiddenType | UsageKind::HasDefiningUse(_)) => *self = other,
        }
    }
}
struct ReverseMapper { map: Vec<(GenericArg, GenericArg)>, failed: bool }
impl TypeFolder for ReverseMapper {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if matches!(tys.kind(ty), TyKind::Param(_)) {
            for &(from, to) in &self.map { if from == GenericArg::Type(ty) { let GenericArg::Type(t) = to else { unreachable!() }; return t; } }
            self.failed = true; return tys.error;
        }
        super_fold_ty(tys, ty, self, depth)
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, _depth: u32) -> Const {
        if matches!(tys.const_kind(ct), ConstKind::Param(_)) {
            for &(from, to) in &self.map { if from == GenericArg::Const(ct) { let GenericArg::Const(c) = to else { unreachable!() }; return c; } }
            self.failed = true; return tys.intern_const(ConstKind::Error);
        }
        // rustc_middle/src/ty/opaque_types.rs:209: only parameter consts remap.
        ct
    }
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
        // ty/mod.rs:935-943: HIR's hidden type erases free regions; borrowck
        // computes the final captured lifetimes from its own constraints.
        if matches!(tys.region_kind(r), crate::types::RegionKind::ReBound(db, _) if db < depth) { r } else { RegionId::ERASED }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round28_reverse_mapper_reports_only_unmapped_position() {
        let tys = Types::new();
        let param = tys.intern(TyKind::Param(0));
        let tuple = tys.intern(TyKind::Tuple(tys.mk_list(&[param, tys.u32_])));
        let mut mapper = ReverseMapper { map: Vec::new(), failed: false };
        let mapped = mapper.fold_ty(&tys, tuple, 0);
        assert!(mapper.failed);
        assert!(tys.kind(mapped) == &TyKind::Tuple(tys.mk_list(&[tys.error, tys.u32_])));
        let param = tys.intern_const(ConstKind::Param(crate::consts::ParamConst { index: 0 }));
        assert_eq!(mapper.fold_const(&tys, param, 0), tys.intern_const(ConstKind::Error));
        let value = tys.const_usize(3);
        assert_eq!(mapper.fold_const(&tys, value, 0), value);
    }
}
