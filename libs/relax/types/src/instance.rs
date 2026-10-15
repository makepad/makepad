// ported from N:rustc_middle/src/ty/instance.rs @8d1a764
use crate::consts::GenericArg;
use crate::lang_items::LangItem;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use crate::upvar::ClosureKind;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Instance {
    pub def: InstanceKind,
    pub args: Vec<GenericArg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstanceKind {
    Item(DefId),
    Intrinsic(DefId),
    Virtual(DefId, usize),
    Shim(ShimKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShimKind {
    VTable(DefId),
    Reify(DefId, Option<ReifyReason>),
    FnPtr(DefId, TyId),
    ClosureOnce { call_once: DefId, closure: DefId, track_caller: bool },
    DropGlue(DefId, Option<TyId>),
    Clone(DefId, TyId),
    FnPtrAsPtr(DefId, TyId),
    FnPtrFromPtr(DefId, TyId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReifyReason { FnPtr, Vtable }

impl InstanceKind {
    pub fn def_id(self) -> DefId {
        match self {
            Self::Item(def) | Self::Intrinsic(def) | Self::Virtual(def, _) => def,
            Self::Shim(shim) => shim.def_id(),
        }
    }
}
impl ShimKind {
    pub fn def_id(self) -> DefId {
        match self {
            Self::VTable(def) | Self::Reify(def, _) | Self::FnPtr(def, _) |
            Self::DropGlue(def, _) | Self::Clone(def, _) |
            Self::FnPtrAsPtr(def, _) | Self::FnPtrFromPtr(def, _) => def,
            Self::ClosureOnce { call_once, .. } => call_once,
        }
    }
}
impl Instance {
    pub fn compact_args(&self, tcx: &Tcx) -> Vec<TyId> {
        let mut args = Vec::new();
        for arg in &self.args { match *arg {
            GenericArg::Type(t) => args.push(t),
            GenericArg::Const(c) => args.push(tcx.tys.intern(TyKind::Const(c))),
            GenericArg::Lifetime(_) => {}
        } }
        args
    }

    /// instance.rs:656-712. KCFI is not a Relax codegen feature.
    pub fn resolve_for_fn_ptr(tcx: &Tcx, prog: &Program, def: DefId, args: &[TyId]) -> Result<Self, crate::next_solver::codegen::CodegenObligationError> {
        let mut instance = tcx.resolve_instance_raw(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), def, args)?;
        match instance.def {
            InstanceKind::Item(d) if tcx.fn_sig(prog, d).is_some_and(|s| s.track_caller) => instance.def = InstanceKind::Shim(ShimKind::Reify(d, None)),
            InstanceKind::Virtual(d, _) => instance.def = InstanceKind::Shim(ShimKind::Reify(d, None)),
            _ => {}
        }
        Ok(instance)
    }

    /// instance.rs:715-782: unsized by-value receivers and caller-location ABI.
    pub fn expect_resolve_for_vtable(tcx: &Tcx, prog: &Program, def: DefId, args: &[TyId]) -> Self {
        let original = tcx.fn_sig(prog, def).expect("vtable method signature");
        // instance.rs:715-722: self_kind is nonzero exactly for the
        // vtable-reachable methods whose generics include Self.
        if original.params.first().is_some_and(|p| matches!(tcx.tys.kind(*p), TyKind::Param(0))) && original.self_kind != 0 {
            let indices = tcx.generic_indices_of(prog, def);
            let regions = tcx.tys.intern_region_args(&vec![crate::types::RegionId::ERASED; indices.lifetimes.len()]);
            return Self { def: InstanceKind::Shim(ShimKind::VTable(def)), args: tcx.complete_args(prog, def, args, regions) };
        }
        let mut instance = crate::instance::Instance::expect_resolve(tcx, prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), def, args);
        match instance.def {
            InstanceKind::Item(d) if tcx.fn_sig(prog, d).is_some_and(|s| s.track_caller) && !original.track_caller && prog.def(prog.def(d).parent).kind != DefKind::Trait => {
                if prog.def(d).kind == DefKind::Closure {
                    let indices = tcx.generic_indices_of(prog, def);
                    let regions = tcx.tys.intern_region_args(&vec![crate::types::RegionId::ERASED; indices.lifetimes.len()]);
                    instance = Self { def: InstanceKind::Shim(ShimKind::Reify(def, None)), args: tcx.complete_args(prog, def, args, regions) };
                } else { instance.def = InstanceKind::Shim(ShimKind::Reify(d, None)); }
            }
            InstanceKind::Virtual(d, _) => instance.def = InstanceKind::Shim(ShimKind::Reify(d, None)),
            _ => {}
        }
        instance
    }

    pub fn new_raw(def: DefId, args: Vec<GenericArg>) -> Self { Self { def: InstanceKind::Item(def), args } }

    /// instance.rs:785-798,833-862. Closure types retain Relax's approved N4
    /// storage identity; the FnOnce adapter arguments are the call trait's args.
    pub fn resolve_closure(tcx: &Tcx, prog: &Program, ty: TyId, requested_kind: ClosureKind) -> Self {
        let TyKind::Closure(file, expr, closure_args) = tcx.tys.kind(ty) else { panic!("expected closure") };
        let node = if *expr & crate::tcx::ASYNC_FN_BODY != 0 {
            let TyKind::FnDef(owner, _, _) = tcx.tys.kind(closure_args.parent_args) else { panic!("coroutine owner") };
            crate::def_collector::NodeId::AsyncBody(prog.def(*owner).item)
        } else { crate::def_collector::NodeId::Closure(crate::ast::ExprId(*expr)) };
        let closure = prog.node_id_to_def_id[&(*file, node)];
        let actual_kind = tcx.tys.to_opt_closure_kind(closure_args.kind_ty()).expect("resolved closure kind");
        match needs_fn_once_adapter_shim(actual_kind, requested_kind) {
            Ok(true) => {
                let fn_once = tcx.lang_items.get(LangItem::FnOnce).expect("FnOnce lang item");
                let call_once = prog.traits[prog.def(fn_once).sub as usize].iter().copied()
                    .find(|d| prog.def(*d).kind == DefKind::AssocFn).expect("FnOnce call method");
                // `closure_track_caller` is a cut gated mechanism. In
                // particular, a closure does not inherit its owner's attribute.
                let track_caller = false;
                let TyKind::FnPtr(inputs, _, _, _) = tcx.tys.kind(closure_args.sig_as_fn_ptr_ty()) else { panic!("closure signature") };
                let tupled_inputs = tcx.normalize_erasing_late_bound_regions(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), tcx.tys.intern(TyKind::Tuple(inputs.clone())));
                Self { def: InstanceKind::Shim(ShimKind::ClosureOnce { call_once, closure, track_caller }), args: vec![GenericArg::Type(ty), GenericArg::Type(tupled_inputs)] }
            }
            _ => {
                // Synthetic closure args are the parent args followed by kind,
                // signature and tupled upvars, as ClosureArgs::split in nightly.
                let TyKind::FnDef(parent, args, regions) = tcx.tys.kind(closure_args.parent_args) else { panic!("closure parent args") };
                let mut args = tcx.complete_args(prog, *parent, tcx.tys.list(*args), *regions);
                args.push(GenericArg::Type(closure_args.kind_ty()));
                args.push(GenericArg::Type(closure_args.sig_as_fn_ptr_ty()));
                args.push(GenericArg::Type(closure_args.tupled_upvars_ty));
                Self::new_raw(closure, args)
            }
        }
    }
}

/// instance.rs:996-1026. Keep the invalid kind pairs distinct from an adapter.
pub fn needs_fn_once_adapter_shim(actual: ClosureKind, requested: ClosureKind) -> Result<bool, ()> {
    match (actual, requested) {
        (ClosureKind::Fn, ClosureKind::Fn) |
        (ClosureKind::FnMut, ClosureKind::FnMut) |
        (ClosureKind::FnOnce, ClosureKind::FnOnce) |
        (ClosureKind::Fn, ClosureKind::FnMut) => Ok(false),
        (ClosureKind::Fn | ClosureKind::FnMut, ClosureKind::FnOnce) => Ok(true),
        (ClosureKind::FnMut | ClosureKind::FnOnce, _) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round22_fn_once_adapter_kind_table() {
        use ClosureKind::*;
        for (actual, requested, expected) in [
            (Fn, Fn, Ok(false)), (Fn, FnMut, Ok(false)), (Fn, FnOnce, Ok(true)),
            (FnMut, Fn, Err(())), (FnMut, FnMut, Ok(false)), (FnMut, FnOnce, Ok(true)),
            (FnOnce, Fn, Err(())), (FnOnce, FnMut, Err(())), (FnOnce, FnOnce, Ok(false)),
        ] { assert_eq!(needs_fn_once_adapter_shim(actual, requested), expected); }
    }
}

/// rustc_middle/src/ty/instance.rs:413-444: count repeated subtrees, caching
/// their size. The concrete folder is a read-only visitor over Relax's type arena.
pub fn type_length(tcx: &Tcx, args: &[TyId]) -> usize {
    use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
    struct Visitor { type_length: usize, cache: crate::fxhash::HashMap<TyId, usize> }
    impl TypeFolder for Visitor {
        fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
            // Compact generic const arguments are not an additional type node.
            if let TyKind::Const(ct) = tys.kind(ty) { self.fold_const(tys, *ct, depth); return ty; }
            if let Some(&length) = self.cache.get(&ty) { self.type_length += length; return ty; }
            let prev = self.type_length;
            self.type_length += 1;
            super_fold_ty(tys, ty, self, depth);
            if self.type_length > 16 { self.cache.insert(ty, self.type_length - prev); }
            ty
        }
        fn fold_const(&mut self, tys: &crate::types::Types, ct: crate::consts::Const, depth: u32) -> crate::consts::Const {
            self.type_length += 1;
            super_fold_const(tys, ct, self, depth);
            ct
        }
    }
    let mut visitor = Visitor { type_length: 0, cache: Default::default() };
    for &ty in args { visitor.fold_ty(&tcx.tys, ty, 0); }
    visitor.type_length
}

impl Instance {
    /// rustc_middle/src/ty/instance.rs:613-652. The -Z eager enforcement
    /// option is cut; failed resolution still diagnoses excessive type length.
    pub fn expect_resolve(tcx: &Tcx, prog: &Program, env: crate::tcx::TypingEnv, def: DefId, args: &[TyId]) -> Self {
        match tcx.resolve_instance_raw(prog, env, def, args) {
            Ok(instance) => instance,
            Err(crate::next_solver::codegen::CodegenObligationError::Ambiguity) => {
                let length = type_length(tcx, args);
                let mut limit = 1usize << 24;
                if let Some(krate) = prog.crates.get(crate::layout::root_crate() as usize) {
                    let file = &prog.files[prog.mods[krate.root_mod as usize].file as usize];
                    for attr in &file.ast.root_attrs {
                        if attr.toks.hi == attr.toks.lo + 3 && file.tok_text(attr.toks.lo) == "type_length_limit" && file.tok_text(attr.toks.lo + 1) == "=" {
                            if let Ok(value) = file.tok_text(attr.toks.lo + 2).trim_matches('"').parse() { limit = value; }
                        }
                    }
                }
                if length > limit {
                    tcx.dcx.fatal(crate::next_solver::fulfill::ObligationCause { body_id: def, span: 0 },
                        format!("reached the type-length limit while instantiating `{}` (type length {length}, limit {limit})", prog.def_path(def)));
                }
                panic!("failed to resolve instance for {}", prog.def_path(def));
            }
            Err(error) => panic!("failed to resolve instance for {}: {error:?}", prog.def_path(def)),
        }
    }
}
