// ported from N:rustc_traits/src/codegen.rs @8d1a764
// ported from N:rustc_ty_utils/src/instance.rs @8d1a764
use super::*;
use super::select::ImplSource;
use super::fulfill::ObligationCtxt;
use crate::infer::InferCtxt;
use crate::program::Program;
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{RegionId, TyKind};
use crate::instance::{Instance, InstanceKind, ShimKind};
use crate::program::DefKind;
use crate::lang_items::LangItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodegenObligationError { Ambiguity, Unimplemented, UnconstrainedParam }
impl Tcx {
    /// rustc_ty_utils/src/instance.rs:17-438 (the resolve_instance_raw query's provider; the
    /// query: Tcx::resolve_instance_raw, type_queries.rs). All callers retain the instance kind.
    pub(crate) fn resolve_instance_raw_uncached(&self, prog: &Program, typing_env: TypingEnv, def: crate::program::DefId, compact: &[TyId]) -> Result<Instance, CodegenObligationError> {
        let indices = self.generic_indices_of(prog, def);
        let regions = self.tys.intern_region_args(&vec![RegionId::ERASED; indices.lifetimes.len()]);
        let mut args = self.complete_args(prog, def, compact, regions);
        for arg in &mut args {
            *arg = match *arg {
                GenericArg::Type(t) => GenericArg::Type(self.try_normalize_after_erasing_regions(prog, typing_env, t).map_err(|_| CodegenObligationError::Unimplemented)?),
                GenericArg::Const(c) => GenericArg::Const(self.tys.expect_const(self.try_normalize_after_erasing_regions(prog, typing_env, self.tys.intern(TyKind::Const(c))).map_err(|_| CodegenObligationError::Unimplemented)?)),
                GenericArg::Lifetime(_) => GenericArg::Lifetime(RegionId::ERASED),
            };
        }
        let parent = prog.def(def).parent;
        if parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Trait {
            return self.resolve_associated_item(prog, typing_env, def, parent, args);
        }
        let kind = if self.fn_item_sig(prog, def).is_some_and(|s| s.header.abi == crate::types::Abi::RelaxIntrinsic) {
            InstanceKind::Intrinsic(def)
        } else if self.lang_items.get(LangItem::DropGlue) == Some(def) {
            let GenericArg::Type(ty) = args[0] else { unreachable!() };
            let ty = if self.needs_drop(prog, typing_env, ty) {
                match self.tys.kind(ty) {
                    TyKind::Closure(..) | TyKind::Tuple(..) | TyKind::Adt(..) | TyKind::Dyn(..) | TyKind::Array(..) | TyKind::Slice(..) => Some(ty),
                    // rustc instance.rs:61 "Drop shims can only be built from ADTs": `return Ok(None)`,
                    // too generic to resolve (a type parameter or projection that may need drop) --
                    // not an unmet obligation (a generic body's `drop_in_place::<T>` was a delayed bug)
                    _ => return Err(CodegenObligationError::Ambiguity),
                }
            } else { None };
            InstanceKind::Shim(ShimKind::DropGlue(def, ty))
        } else { InstanceKind::Item(def) };
        // DEBLOAT: async drop and LLVM-only intrinsics are not Relax language features.
        Ok(Instance { def: kind, args })
    }

    fn resolve_associated_item(&self, prog: &Program, typing_env: TypingEnv, def: DefId, parent: DefId, rcvr_args: Vec<GenericArg>) -> Result<Instance, CodegenObligationError> {
        use super::assembly::BuiltinImplSource;
        let indices = self.generic_indices_of(prog, parent);
        let parent_count = indices.types_consts.len() + indices.lifetimes.len();
        let tr = TraitRef { def_id: parent, args: rcvr_args[..parent_count].to_vec() };
        let selected = self.codegen_select_candidate(prog, typing_env, tr.clone())?;
        let GenericArg::Type(self_ty) = tr.args[0] else { unreachable!() };
        match selected {
            ImplSource::UserDefined { impl_def_id, mut args, .. } => {
                // DEBLOAT(round9): specialization is rejected at collection. Every
                // impl is a final leaf, including its inherited trait defaults.
                let leaf = self.fetch_eligible_assoc_item(prog, def, impl_def_id);
                let rigid = !matches!(self.tys.kind(self_ty), TyKind::Param(_) | TyKind::Alias(..) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error | TyKind::Bound(..) | TyKind::Placeholder(_));
                if !rigid {
                    let mut sized = false;
                    for p in self.clauses_of(prog, impl_def_id) {
                        let p = self.instantiate_predicate(prog, impl_def_id, &args, &p);
                        if let PredicateKind::Clause(ClauseKind::Trait(t)) = p.kind {
                            sized |= self.lang_items.get(LangItem::Sized) == Some(t.trait_ref.def_id) && t.trait_ref.args[0] == GenericArg::Type(self_ty);
                        }
                    }
                    if !sized { return Err(CodegenObligationError::Ambiguity); }
                }
                if !self.associated_item_has_value(prog, leaf) { return Err(CodegenObligationError::Unimplemented); }
                // specialize/mod.rs:109-159, translate_args: a trait default
                // uses the selected impl's trait args; method args are rebased
                // unchanged. With specialization cut, there is no other impl node.
                if leaf == def { args = self.impl_trait_ref(prog, impl_def_id, &args).args; }
                args.extend_from_slice(&rcvr_args[parent_count..]);
                let identity = self.generic_indices_of(prog, leaf).identity_args(&self.tys);
                if args.len() != identity.len() || args.iter().zip(&identity).any(|(a,b)| std::mem::discriminant(a) != std::mem::discriminant(b)) { return Err(CodegenObligationError::UnconstrainedParam); }
                if leaf != def { self.compare_impl_item(prog, leaf).map_err(|_| CodegenObligationError::Unimplemented)?; }
                Ok(Instance::new_raw(leaf, args))
            }
            ImplSource::Builtin(BuiltinImplSource::Object(_), _) => {
                // Final trait methods are a cut feature, not default trait bodies.
                if !self.tys.is_concrete(self_ty) || rcvr_args.iter().any(|a| match *a { GenericArg::Type(t) => !self.tys.is_concrete(t), GenericArg::Const(c) => !self.tys.is_concrete(self.tys.intern(TyKind::Const(c))), _ => false }) { return Err(CodegenObligationError::Ambiguity); }
                let base = self.first_method_vtable_slot(prog, &tr);
                let entries = self.own_existential_vtable_entries(prog, parent);
                let offset = entries.iter().position(|&d| d == def).ok_or(CodegenObligationError::Unimplemented)?;
                Ok(Instance { def: InstanceKind::Virtual(def, base + offset), args: rcvr_args })
            }
            ImplSource::Builtin(BuiltinImplSource::Misc | BuiltinImplSource::Trivial, _) => {
                let lang = self.lang_items.by_def.get(&parent).copied();
                let kind = match lang {
                    Some(LangItem::Clone) => {
                        if prog.name(def) == "clone" {
                            if !matches!(self.tys.kind(self_ty), TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Closure(..) | TyKind::Tuple(..)) { return Err(CodegenObligationError::Unimplemented); }
                            InstanceKind::Shim(ShimKind::Clone(def, self_ty))
                        } else { assert_eq!(prog.name(def), "clone_from"); InstanceKind::Item(def) }
                    }
                    Some(LangItem::FnPtrTrait) => {
                        if !matches!(self.tys.kind(self_ty), TyKind::FnPtr(..)) { return Err(CodegenObligationError::Unimplemented); }
                        match self.lang_items.by_def.get(&def) {
                            Some(LangItem::FnPtrAsPtr) => InstanceKind::Shim(ShimKind::FnPtrAsPtr(def, self_ty)),
                            Some(LangItem::FnPtrFromPtr) => InstanceKind::Shim(ShimKind::FnPtrFromPtr(def, self_ty)),
                            _ => InstanceKind::Item(def),
                        }
                    }
                    Some(LangItem::Fn | LangItem::FnMut | LangItem::FnOnce | LangItem::AsyncFn | LangItem::AsyncFnMut | LangItem::AsyncFnOnce) => {
                        if matches!(lang, Some(LangItem::Fn | LangItem::FnMut | LangItem::FnOnce)) {
                            debug_assert!(matches!(prog.name(def), "call" | "call_mut" | "call_once"), "builtin call trait item requires a shim");
                        }
                        let requested = match lang.unwrap() { LangItem::Fn | LangItem::AsyncFn => crate::upvar::ClosureKind::Fn, LangItem::FnMut | LangItem::AsyncFnMut => crate::upvar::ClosureKind::FnMut, _ => crate::upvar::ClosureKind::FnOnce };
                        match self.tys.kind(self_ty) {
                            TyKind::Closure(..) => return Ok(Instance::resolve_closure(self, prog, self_ty, requested)),
                            TyKind::FnDef(..) | TyKind::FnPtr(..) => InstanceKind::Shim(ShimKind::FnPtr(def, self_ty)),
                            _ => panic!("builtin callable instance for a non-callable type"),
                        }
                    }
                    Some(LangItem::Future) if self.is_coroutine(self_ty) => {
                        // EXCEPTION(M7): coroutine bodies use closure slots until MIR coroutines.
                        if self.lang_items.get(LangItem::FuturePoll) == Some(def) {
                            return Ok(Instance::resolve_closure(self, prog, self_ty, crate::upvar::ClosureKind::Fn));
                        }
                        assert!(self.associated_item_has_value(prog, def));
                        InstanceKind::Item(def)
                    }
                    // DEBLOAT: coroutine/gen/async-gen, transmutability, reflection
                    // and async-drop mechanisms are feature-gated and not represented.
                    _ => return Err(CodegenObligationError::Unimplemented),
                };
                Ok(Instance { def: kind, args: rcvr_args })
            }
            ImplSource::Param(_) | ImplSource::Builtin(BuiltinImplSource::TraitUpcasting(_), _) => Err(CodegenObligationError::Ambiguity),
        }
    }

    pub(crate) fn associated_item_has_value(&self, prog: &Program, def: DefId) -> bool {
        let d = prog.def(def);
        match &prog.files[d.file as usize].ast.item(d.item).kind {
            crate::ast::ItemKind::Fn(_, body, _) => body.is_some() || prog.fn_body(d.file, d.item).ok().flatten().is_some(),
            crate::ast::ItemKind::Const(_, value) => value.is_some(),
            crate::ast::ItemKind::TypeAlias(r) => prog.files[d.file as usize].ast.row(*r).ty.is_some(),
            _ => false,
        }
    }

    /// codegen.rs:26-105 (the codegen_select_candidate query's provider; the query:
    /// Tcx::codegen_select_candidate, type_queries.rs): selection and fulfillment share one
    /// inference owner; regions are erased only after its impl arguments are fully resolved.
    pub(crate) fn codegen_select_candidate_uncached(&self, prog: &Program, typing_env: TypingEnv, trait_ref: TraitRef) -> Result<ImplSource, CodegenObligationError> {
        let infcx = InferCtxt::new(self, typing_env.mode, typing_env.limit);

        let param_env = typing_env.param_env;
        let goal = Goal { param_env, predicate: collect::trait_predicate(trait_ref.def_id, trait_ref.args) };
        let mut selected = match infcx.select_in_new_trait_solver(prog, goal) {
            Ok(Some(selected)) => selected,
            Ok(None) => return Err(CodegenObligationError::Ambiguity),
            Err(_) => return Err(CodegenObligationError::Unimplemented),
        };
        let nested = match &mut selected {
            ImplSource::UserDefined { nested, .. } | ImplSource::Builtin(_, nested) | ImplSource::Param(nested) => std::mem::take(nested),
        };
        let mut ocx = ObligationCtxt::new(&infcx, prog);
        ocx.register_obligations(nested);
        ocx.evaluate_obligations_error_on_ambiguity().map_err(|_| CodegenObligationError::Unimplemented)?;
        if let ImplSource::UserDefined { args, .. } = &mut selected {
            for arg in args {
                *arg = match *arg {
                    GenericArg::Lifetime(_) => GenericArg::Lifetime(RegionId::ERASED),
                    GenericArg::Type(t) => GenericArg::Type(self.tys.erase_regions(infcx.deeply_resolve_ignoring_regions(t))),
                    GenericArg::Const(c) => {
                        let t = self.tys.intern(TyKind::Const(c));
                        let t = self.tys.erase_regions(infcx.deeply_resolve_ignoring_regions(t));
                        GenericArg::Const(self.tys.expect_const(t))
                    }
                };
                let unresolved = match *arg { GenericArg::Type(t) => self.tys.has_infer(t), GenericArg::Const(c) => c.has_non_region_infer(&self.tys), GenericArg::Lifetime(_) => false };
                if unresolved { return Err(CodegenObligationError::UnconstrainedParam); }
            }
        }
        Ok(selected)
    }
}

impl Tcx {
    /// rustc_traits/src/normalize_erasing_regions.rs:20-66; type_ir/fold.rs:594.
    /// A typing-environment change invalidates the previous rigidness proof.
    pub fn try_normalize_after_erasing_regions(&self, prog: &Program, typing_env: TypingEnv, ty: TyId) -> Result<TyId, NoSolution> {
        crate::solver_work::count(crate::solver_work::NORMALIZE);
        // rustc_middle::ty::normalize_erasing_regions: alias-free values need only erasure.
        if !self.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_ALIAS) {
            crate::solver_work::count(crate::solver_work::NORMALIZE_SKIPPED);
            return if self.tys.has_infer(ty) { Err(NoSolution) } else { Ok(self.tys.erase_regions(ty)) };
        }
        let ty = self.tys.erase_regions(self.tys.set_aliases_to_non_rigid(ty));
        let infcx = InferCtxt::new(self, typing_env.mode, typing_env.limit);

        let mut ocx = ObligationCtxt::new(&infcx, prog);
        let ty = ocx.deeply_normalize_ty(&typing_env.param_env, ty)?;
        if self.tys.has_infer(ty) { return Err(NoSolution); }
        Ok(self.tys.erase_regions(ty))
    }
}
