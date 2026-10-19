//! rustc_hir_analysis well-formedness checks of definitions that need no body:
//! constrained_generic_params.rs, impl_wf_check.rs (E0207), check/wfcheck.rs
//! `check_variances_for_type_defn` (E0392) and check/check.rs
//! `check_type_alias_type_params_are_used` (E0091).
use crate::ast::{GenericParamKind, ItemKind};
use crate::consts::{ConstKind, GenericArg};
use crate::fxhash::HashSet;
use crate::next_solver::{AliasTerm, ClauseKind, Predicate, PredicateKind, Term};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::variance::{GenericIndices, V};
use crate::types::{AliasTy, AliasTyKind, RegionKind, TyId, TyKind};

/// A generic parameter by its index in the full generics list (lifetimes included).
type Parameter = u32;

struct Collector<'a> {
    tcx: &'a Tcx,
    indices: &'a GenericIndices,
    include_nonconstraining: bool,
    out: Vec<Parameter>,
}

impl Collector<'_> {
    fn ty(&mut self, t: TyId) {
        match self.tcx.tys.kind(t).clone() {
            // projections are not injective in general
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. } | AliasTyKind::Inherent { .. } | AliasTyKind::Opaque { .. }, .. }) if !self.include_nonconstraining => {}
            TyKind::Param(i) => { if let Some(&full) = self.indices.types_consts.get(i as usize) { self.out.push(full); } }
            TyKind::Ref(_, t, r) => { self.region(r); self.ty(t); }
            TyKind::Ptr(_, t) | TyKind::Slice(t) => self.ty(t),
            TyKind::Array(t, len) => { self.ty(t); self.ct(len); }
            TyKind::Const(c) => self.ct(c),
            TyKind::Tuple(ts) => for &t in self.tcx.tys.list(ts) { self.ty(t); },
            TyKind::FnPtr(ps, ret, _, _) => { for &p in self.tcx.tys.list(ps) { self.ty(p); } self.ty(ret); }
            TyKind::Adt(_, ts, rs) | TyKind::FnDef(_, ts, rs) => {
                for &t in self.tcx.tys.list(ts) { self.ty(t); }
                for &r in self.tcx.tys.region_args(rs).iter() { self.region(r); }
            }
            TyKind::Alias(_, AliasTy { args, regions, .. }) => {
                for &t in self.tcx.tys.list(args) { self.ty(t); }
                for &r in self.tcx.tys.region_args(regions).iter() { self.region(r); }
            }
            TyKind::Dyn(_, ts, bindings, rs, r, _, _) => {
                self.region(r);
                for &t in self.tcx.tys.list(ts) { self.ty(t); }
                for &r in self.tcx.tys.region_args(rs).iter() { self.region(r); }
                for &(_, t, _) in self.tcx.tys.list(bindings) { self.ty(t); }
            }
            _ => {}
        }
    }

    fn region(&mut self, r: crate::types::RegionId) {
        if let RegionKind::ReEarlyParam(i) = self.tcx.tys.region_kind(r) { self.out.push(i); }
    }

    fn ct(&mut self, c: crate::consts::Const) {
        match self.tcx.tys.const_kind(c) {
            ConstKind::Alias(..) if !self.include_nonconstraining => {}
            ConstKind::Param(p) => { if let Some(&full) = self.indices.types_consts.get(p.index as usize) { self.out.push(full); } }
            ConstKind::Alias(_, alias) => for &arg in alias.args.iter() { self.arg(arg); },
            _ => {}
        }
    }

    fn arg(&mut self, a: GenericArg) {
        match a {
            GenericArg::Type(t) => self.ty(t),
            GenericArg::Const(c) => self.ct(c),
            GenericArg::Lifetime(r) => self.region(r),
        }
    }
}

impl Tcx {
    /// constrained_generic_params.rs `parameters_for` of a type.
    fn parameters_for_ty(&self, indices: &GenericIndices, t: TyId, include_nonconstraining: bool) -> Vec<Parameter> {
        let mut c = Collector { tcx: self, indices, include_nonconstraining, out: Vec::new() };
        c.ty(t);
        c.out
    }

    fn parameters_for_args(&self, indices: &GenericIndices, args: &[GenericArg], include_nonconstraining: bool) -> Vec<Parameter> {
        let mut c = Collector { tcx: self, indices, include_nonconstraining, out: Vec::new() };
        for &a in args { c.arg(a); }
        c.out
    }

    /// `identify_constrained_generic_params` / `setup_constraining_predicates`: parameters that a
    /// projection predicate `<..> = Term` fixes once all of its inputs are known.
    /// The trait an associated item belongs to.
    fn assoc_trait(&self, prog: &Program, item: DefId) -> DefId { prog.def(item).parent }

    fn identify_constrained_generic_params(&self, prog: &Program, indices: &GenericIndices, predicates: &[Predicate], impl_trait_ref: Option<&crate::next_solver::TraitRef>, input: &mut HashSet<Parameter>) {
        let mut remaining: Vec<&Predicate> = predicates.iter().filter(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Projection(_)))).collect();
        let mut changed = true;
        while changed {
            changed = false;
            let mut j = 0;
            while j < remaining.len() {
                let PredicateKind::Clause(ClauseKind::Projection(projection)) = &remaining[j].kind else { unreachable!() };
                let (inputs, same_trait) = match &projection.projection_term {
                    AliasTerm::Ty(alias) => {
                        let TyKind::Alias(_, a) = self.tys.kind(*alias).clone() else { j += 1; continue };
                        // (`prog` for the trait of the projected item)
                        let inputs: Vec<Parameter> = {
                            let mut c = Collector { tcx: self, indices, include_nonconstraining: true, out: Vec::new() };
                            c.ty(*alias);
                            c.out
                        };
                        // an attempt to project out an associated type of the impl's own trait ref
                        let same = impl_trait_ref.is_some_and(|tr| {
                            // the whole trait ref: type and const arguments, then lifetimes
                            let tc: Vec<TyId> = tr.args.iter().filter_map(|x| match *x {
                                GenericArg::Type(t) => Some(t),
                                GenericArg::Const(c) => Some(self.tys.intern(TyKind::Const(c))),
                                GenericArg::Lifetime(_) => None,
                            }).collect();
                            let lts: Vec<crate::types::RegionId> = tr.args.iter().filter_map(|x| if let GenericArg::Lifetime(r) = *x { Some(r) } else { None }).collect();
                            let regions = self.tys.region_args(a.regions);
                            tr.def_id == self.assoc_trait(prog, a.kind.def_id()) && self.tys.list(a.args).len() >= tc.len() && self.tys.list(a.args)[..tc.len()] == tc[..]
                                && regions.len() >= lts.len() && regions[..lts.len()] == lts[..]
                        });
                        (inputs, same)
                    }
                    AliasTerm::Const(_) => { j += 1; continue; }
                };
                if !same_trait && inputs.iter().all(|p| input.contains(p)) {
                    let determined = match &projection.term {
                        Term::Ty(t) => self.parameters_for_ty(indices, *t, false),
                        Term::Const(c) => { let mut col = Collector { tcx: self, indices, include_nonconstraining: false, out: Vec::new() }; col.ct(*c); col.out }
                    };
                    input.extend(determined);
                    remaining.swap_remove(j);
                    changed = true;
                } else { j += 1; }
            }
        }
    }

    /// impl_wf_check.rs `check_impl_wf`: every type and const parameter of an impl appears in the
    /// trait ref or self type, or is fixed by a projection predicate (E0207); a lifetime
    /// parameter too when an associated type's value uses it.
    pub fn check_impl_params_constrained(&self, prog: &Program, imp: DefId) -> Vec<String> {
        let Some(&self_ty) = self.impl_self.get(&imp) else { return Vec::new() };
        if self.tys.has_error(self_ty) { return Vec::new(); }
        let def = prog.def(imp);
        let ast = &prog.files[def.file as usize].ast;
        let Some(item) = ast.as_impl(&ast.item(def.item).kind) else { return Vec::new() };
        let indices = self.generic_indices_of(prog, imp).clone();
        let of_trait = self.impl_trait.contains_key(&imp);
        let trait_ref = of_trait.then(|| self.identity_impl_trait_ref(prog, imp));
        if let Some(tr) = &trait_ref {
            if tr.args.iter().any(|a| matches!(*a, GenericArg::Type(t) if self.tys.has_error(t))) { return Vec::new(); }
        }
        let mut input: HashSet<Parameter> = match &trait_ref {
            Some(tr) => self.parameters_for_args(&indices, &tr.args, false),
            None => self.parameters_for_ty(&indices, self_ty, false),
        }.into_iter().collect();
        let predicates = self.own_clauses_of(prog, imp).to_vec();
        self.identify_constrained_generic_params(prog, &indices, &predicates, trait_ref, &mut input);
        // lifetimes used by an associated type's value
        let mut in_assoc: HashSet<Parameter> = HashSet::default();
        let mut lifetimes_checked = false;
        let mut errors = Vec::new();
        for (index, p) in ast.list(item.generics.params).iter().enumerate() {
            let index = index as u32;
            let name = prog.files[def.file as usize].text(p.name);
            let what = match p.kind {
                GenericParamKind::Type(..) => "type",
                GenericParamKind::Const(..) => "const",
                GenericParamKind::Lifetime(_) => {
                    if !lifetimes_checked {
                        lifetimes_checked = true;
                        let state = crate::tcx::EvalState::new();
                        let cx = crate::tcx::EvalCx::new(prog, self, crate::tcx::TypingEnv::fully_monomorphized(prog), &state);
                        for &it in &prog.impls[def.sub as usize].items {
                            let d = prog.def(it);
                            if d.kind != DefKind::AssocTy { continue; }
                            // the associated type of a fn returning `impl Trait` in the trait is the fn's output
                            if let Some(relax_front::assoc::ImplTraitInTraitData::Impl { fn_def_id }) = prog.impl_trait_in_trait_data.get(&it) {
                                if let Some(sig) = self.fn_sig(prog, *fn_def_id) { in_assoc.extend(self.parameters_for_ty(&indices, sig.ret, true)); }
                                continue;
                            }
                            if let Some(t) = self.impl_assoc_ty(&cx, prog, imp, d.name) {
                                in_assoc.extend(self.parameters_for_ty(&indices, t, true));
                            }
                        }
                    }
                    if in_assoc.contains(&index) && !input.contains(&index) { "lifetime" } else { continue }
                }
            };
            if matches!(p.kind, GenericParamKind::Lifetime(_)) { } else if input.contains(&index) { continue; }
            errors.push(prog.record_error(def.file, p.name.lo, "typeck", format!("E0207: the {} parameter `{}` is not constrained by the impl trait, self type, or predicates", what, name)));
        }
        errors
    }

    /// rustc `HasErrorDeep`: an error in a field type, through the fields of the ADTs it names.
    fn error_deep(&self, d: DefId, seen: &mut Vec<DefId>) -> bool {
        fn walk(tcx: &Tcx, t: TyId, seen: &mut Vec<DefId>, depth: u32) -> bool {
            if depth > 32 { return false; }
            if tcx.tys.has_error(t) { return true; }
            match tcx.tys.kind(t).clone() {
                TyKind::Adt(d, args, _) => {
                    let mut hit = tcx.tys.list(args).iter().any(|&a| walk(tcx, a, seen, depth + 1));
                    if !hit && !seen.contains(&d) {
                        seen.push(d);
                        if let Some(a) = tcx.adts.get(&d) { hit = a.variants.iter().any(|v| v.fields.iter().any(|f| walk(tcx, f.ty, seen, depth + 1))); }
                    }
                    hit
                }
                TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) | TyKind::Slice(i) | TyKind::Array(i, _) => walk(tcx, i, seen, depth + 1),
                TyKind::Tuple(l) => tcx.tys.list(l).iter().any(|&a| walk(tcx, a, seen, depth + 1)),
                _ => false,
            }
        }
        self.adts.get(&d).is_some_and(|adt| adt.variants.iter().any(|v| v.fields.iter().any(|f| walk(self, f.ty, seen, 0))))
    }

    /// check_variances_for_type_defn: a parameter of a struct, enum or union that no field uses
    /// and no projection predicate constrains (E0392).
    pub fn check_type_defn_variances(&self, prog: &Program, d: DefId) -> Vec<String> {
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let Some(generics) = ast.kind_generics(&ast.item(def.item).kind) else { return Vec::new() };
        if generics.params.is_empty() { return Vec::new(); }
        let variances = self.variances_of(prog, d);
        let indices = self.generic_indices_of(prog, d).clone();
        let mut constrained: HashSet<Parameter> = variances.iter().enumerate().filter(|(_, v)| **v != V::Bi).map(|(i, _)| i as u32).collect();
        self.identify_constrained_generic_params(prog, &indices, self.own_clauses_of(prog, d), None, &mut constrained);
        // an error deep in a field type: the unused parameter is not reported
        if self.error_deep(d, &mut vec![d]) { return Vec::new(); }
        let mut errors = Vec::new();
        for (index, p) in ast.list(generics.params).iter().enumerate() {
            if constrained.contains(&(index as u32)) || variances.get(index).is_none() { continue; }
            let what = match p.kind { GenericParamKind::Lifetime(_) => "lifetime", GenericParamKind::Type(..) => "type", GenericParamKind::Const(..) => "const" };
            let name = prog.files[def.file as usize].text(p.name);
            errors.push(prog.record_error(def.file, p.name.lo, "typeck", format!("E0392: {} parameter `{}` is never used", what, name)));
        }
        errors
    }

    /// The type a type alias is declared as, lowered afresh: its errors are the alias's own
    /// (rustc `type_of` of every item), `alias_ty` caches the first lowering for users.
    pub fn check_type_alias(&self, prog: &Program, d: DefId) -> Vec<String> {
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let ItemKind::TypeAlias(_) = ast.item(def.item).kind else { return Vec::new() };
        let Some(generics) = ast.kind_generics(&ast.item(def.item).kind) else { return Vec::new() };
        let state = crate::tcx::EvalState::new();
        state.errors.replace(Some(Vec::new()));
        let cx = crate::tcx::EvalCx::new(prog, self, crate::tcx::TypingEnv::fully_monomorphized(prog), &state);
        let ty = self.lower_alias_rhs(&cx, prog, d);
        let mut errors = state.errors.take().unwrap_or_default();
        if !errors.is_empty() || self.tys.has_error(ty) { return errors; }
        // check_type_alias_type_params_are_used: a type parameter the aliased type never
        // mentions (E0091)
        let indices = self.generic_indices_of(prog, d).clone();
        let used: HashSet<Parameter> = self.parameters_for_ty(&indices, ty, true).into_iter().collect();
        for (index, p) in ast.list(generics.params).iter().enumerate() {
            if matches!(p.kind, GenericParamKind::Type(..)) && !used.contains(&(index as u32)) {
                let name = prog.files[def.file as usize].text(p.name);
                errors.push(prog.record_error(def.file, p.name.lo, "typeck", format!("E0091: type parameter `{}` is never used", name)));
            }
        }
        errors
    }
}

/// rustc_hir_analysis check/wfcheck.rs `check_method_receiver` for a method that writes its
/// receiver type (`self: Box<Self>`): the receiver must be `Self` or reach it through the
/// autoderef chain, every step implementing `LegacyReceiver`. Where rustc would accept it with
/// `arbitrary_self_types` (a `Deref` chain) or `arbitrary_self_types_pointers` (raw pointers),
/// it reports the feature gate (E0658); otherwise E0307, or E0801 for a generic receiver.
/// A receiver or `Self` that mentions a projection or opaque type is not judged.
impl Tcx {
    pub fn check_method_receiver(&self, prog: &Program, m: DefId) -> Vec<String> {
        use crate::next_solver::{Goal, collect};
        let def = prog.def(m);
        if def.kind != DefKind::AssocFn { return Vec::new(); }
        let f = &prog.files[def.file as usize];
        let item = f.ast.item(def.item);
        let ItemKind::Fn(sig_id, _, _) = item.kind else { return Vec::new() };
        if !matches!(f.ast.fn_sig(sig_id).self_param, Some(crate::ast::SelfParam::Typed(..))) { return Vec::new(); }
        let parent = prog.def(def.parent);
        let self_ty = match parent.kind {
            DefKind::Impl => match self.impl_self.get(&def.parent) { Some(&t) => t, None => return Vec::new() },
            DefKind::Trait => self.tys.intern(TyKind::Param(0)),
            _ => return Vec::new(),
        };
        let Some(sig) = self.fn_sig(prog, m) else { return Vec::new() };
        let Some(&recv) = sig.params.first() else { return Vec::new() };
        let (recv, self_ty) = (self.tys.erase_all_regions(recv), self.tys.erase_all_regions(self_ty));
        for t in [recv, self_ty] {
            if self.tys.has_error(t) || self.tys.has_assoc(t) || self.tys.has_opaque(t) || self.tys.has_infer(t) { return Vec::new(); }
        }
        let Some(legacy) = self.lang_items.get(crate::lang_items::LangItem::LegacyReceiver) else { return Vec::new() };
        let env = self.param_env_id(prog, m);
        let parent_params = self.generic_indices_of(prog, def.parent).types_consts.len() as u32;
        // 0: neither feature, 1: arbitrary_self_types, 2: with pointers
        let valid = |level: u8| -> Result<(), bool> {
            let infcx = crate::infer::InferCtxt::for_item(self, prog, crate::tcx::TypingMode::non_body_analysis(), m);
            let same = |a: TyId, b: TyId| infcx.at(prog, env.clone()).eq(a, b).is_ok();
            if same(self_ty, recv) { return Ok(()); }
            let generic_param = |t: TyId| matches!(self.tys.kind(t), TyKind::Param(i) if *i >= parent_params);
            if generic_param(recv) { return Err(true); }
            let mut cur = recv;
            for _ in 0..32 {
                if generic_param(cur) { return Err(true); }
                if same(self_ty, cur) { return Ok(()); }
                if level == 0 {
                    // (a trait object implements no `LegacyReceiver`; asking would need the dyn
                    // compatibility of a trait whose own signature is being checked)
                    if matches!(self.tys.kind(cur), TyKind::Dyn(..)) { break; }
                    let goal = Goal { param_env: env.clone(), predicate: collect::trait_predicate(legacy, vec![GenericArg::Type(cur)]) };
                    if !infcx.evaluate_obligation(prog, goal).must_apply_modulo_regions() { break; }
                }
                let next = match self.tys.kind(cur) {
                    TyKind::Ref(_, t, _) => Some(*t),
                    TyKind::Ptr(_, t) if level == 2 => Some(*t),
                    TyKind::Ptr(..) | TyKind::Error | TyKind::Infer(_) | TyKind::Dyn(..) => None,
                    _ => crate::typeck::probe::overloaded_deref_ty(&infcx, prog, &env, cur),
                };
                let Some(next) = next else { break };
                cur = self.tys.erase_all_regions(next);
            }
            Err(false)
        };
        let Err(generic) = valid(0) else { return Vec::new() };
        let shown = crate::typeck::ty_to_string(false, prog, self, recv);
        let pos = self_param_pos(prog, def.file, item.lo, item.name.hi);
        let crate_features = prog.crates[def.krate as usize].features(prog);
        if !generic {
            if valid(1).is_ok() && !crate_features.has("arbitrary_self_types") && !crate_features.has("arbitrary_self_types_pointers") {
                return vec![prog.record_error(def.file, pos, "typeck", format!("E0658: `{}` cannot be used as the type of `self` without the `arbitrary_self_types` feature", shown))];
            }
            if valid(2).is_ok() && !crate_features.has("arbitrary_self_types_pointers") {
                return vec![prog.record_error(def.file, pos, "typeck", format!("E0658: `{}` cannot be used as the type of `self` without the `arbitrary_self_types_pointers` feature", shown))];
            }
        }
        if crate_features.has("arbitrary_self_types") || crate_features.has("arbitrary_self_types_pointers") {
            // (the features change which chains are valid; Relax does not model their errors)
            return Vec::new();
        }
        if generic {
            vec![prog.record_error(def.file, pos, "typeck", format!("E0801: invalid generic `self` parameter type: `{}`", shown))]
        } else {
            vec![prog.record_error(def.file, pos, "typeck", format!("E0307: invalid `self` parameter type: `{}`", shown))]
        }
    }
}

/// The position of the `self` of a method's parameter list (after its name).
fn self_param_pos(prog: &Program, file: u32, lo: u32, name_hi: u32) -> u32 {
    let f = &prog.files[file as usize];
    let b = f.bytes(name_hi, (name_hi + 400).min(f.src_len));
    b.windows(4).position(|w| w == b"self").map_or(lo, |i| name_hi + i as u32)
}

/// Trait obligations of rustc check/wfcheck.rs (`makepad check` only).
pub mod obligations {
    // Item well-formedness: the trait obligations of rustc_hir_analysis/src/check/wfcheck.rs
    // (check_item, check_type_defn, check_item_fn / check_fn_or_method, check_impl,
    // check_where_clauses, check_associated_item, check_static_item). Region obligations are
    // the region checker's; every obligation here is registered in a fulfillment context under
    // the item's own parameter environment and reported when it cannot hold.
    use crate::consts::GenericArg;
    use crate::next_solver::fulfill::{NextSolverError, ObligationCause, ObligationCtxt, PredicateObligation};
    use crate::next_solver::{collect, AliasTerm, ClauseKind, ClausePolarity, Goal, ParamEnvId, Predicate, PredicateKind, Term};
    use crate::program::{DefId, DefKind, Program};
    use crate::tcx::Tcx;
    use crate::types::{TyId, TyKind};
    use crate::ast::{self, ItemKind};

    struct Wfcx<'a, 'tcx> {
        prog: &'a Program,
        tcx: &'a Tcx,
        infcx: &'a crate::infer::InferCtxt<'tcx>,
        ocx: ObligationCtxt<'a, 'tcx>,
        env: ParamEnvId,
        def: DefId,
        /// only dyn-compatibility goals are kept (a trait definition: its environment is not built)
        only_dyn: bool,
    }

    impl<'a, 'tcx> Wfcx<'a, 'tcx> {
        fn register(&mut self, goal: Goal, pos: u32) {
            if self.only_dyn && !matches!(goal.predicate.kind, PredicateKind::DynCompatible(_)) { return; }
            self.ocx.engine.register_predicate_obligation(self.infcx, PredicateObligation { goal, recursion_depth: 0, cause: ObligationCause { span: pos, body_id: self.def } });
        }

        /// wfcheck.rs WfCheckingCtxt::deeply_normalize. A type that cannot be normalized stays as
        /// written: its own well-formedness reports the unmet bound.
        fn normalize(&mut self, ty: TyId) -> TyId {
            if !self.tcx.tys.has_assoc(ty) { return ty; }
            let mut ocx = ObligationCtxt::new(self.infcx, self.prog);
            ocx.cause = ObligationCause { span: 0, body_id: self.def };
            ocx.deeply_normalize_ty(&self.env, ty).unwrap_or(ty)
        }

        /// WfCheckingCtxt::register_wf_obligation: the obligations that make `term` well formed
        /// (rustc_trait_selection wf.rs), expanded here so a failure names the unmet bound.
        fn register_wf(&mut self, ty: TyId, pos: u32) {
            if self.tcx.tys.has_error(ty) || !arity_ok(self.tcx, self.prog, ty) { return; }
            if let Ok(Some(goals)) = crate::next_solver::wf::unnormalized_obligations(self.infcx, self.prog, &self.env, Term::Ty(ty)) {
                for g in goals {
                    // (constants are evaluated by their own check)
                    if matches!(g.predicate.kind, PredicateKind::Clause(ClauseKind::ConstEvaluatable(_))) { continue; }
                    self.register(g, pos);
                }
            }
        }

        fn register_bound(&mut self, ty: TyId, trait_def: Option<DefId>, pos: u32) {
            if self.tcx.tys.has_error(ty) { return; }
            let Some(td) = trait_def else { return };
            let goal = Goal { param_env: self.env.clone(), predicate: collect::trait_predicate(td, vec![GenericArg::Type(ty)]) };
            self.register(goal, pos);
        }

        fn sized(&mut self, ty: TyId, pos: u32) {
            let sized = self.tcx.lang_items.get(crate::lang_items::LangItem::Sized);
            self.register_bound(ty, sized, pos);
        }

        /// wf.rs clause_obligations: what makes one where-clause well formed.
        fn clause_obligations(&mut self, p: &Predicate, pos: u32) {
            let tys = &self.tcx.tys;
            // (bound regions do not matter to well-formedness; bound types and consts do)
        let escaping = |t: TyId| { let f = tys.flags(t); f.contains(crate::types::TypeFlags::HAS_TY_BOUND) || f.contains(crate::types::TypeFlags::HAS_CT_BOUND) };
            match &p.kind {
                PredicateKind::Clause(ClauseKind::Trait(t)) => {
                    if t.polarity == ClausePolarity::Negative { return; }
                    let def = t.trait_ref.def_id;
                    let args = &t.trait_ref.args;
                    // wf.rs nominal_obligations: the trait's own predicates at these arguments
                    // (the `Self: Sized`-like predicates of a `?Sized`-declaring trait included)
                    if self.tcx.lang_items.get(crate::lang_items::LangItem::Sized) != Some(def) {
                        for q in self.tcx.clauses_of(self.prog, def) {
                            // (bounds on the supertraits' associated types, `Super<Assoc: Bound>`, are implied
                            // predicates rustc elaborates into the environment; this one's elaboration stops
                            // at `Self: Super`)
                            if matches!(&q.kind, PredicateKind::Clause(ClauseKind::Trait(u)) if matches!(u.trait_ref.args.first(), Some(GenericArg::Type(t)) if matches!(self.tcx.tys.kind(*t), TyKind::Alias(..)))) { continue; }
                            let q = self.tcx.instantiate_predicate(self.prog, def, args, &q);
                            if crate::next_solver::wf::predicate_has_escaping_bound_vars(tys, &q) { continue; }
                            // the trait's `Self: Trait` is the clause itself
                            if matches!(&q.kind, PredicateKind::Clause(ClauseKind::Trait(u)) if u.trait_ref.def_id == def) { continue; }
                            self.register(Goal { param_env: self.env.clone(), predicate: q }, pos);
                        }
                    }
                    for a in args {
                        if let GenericArg::Type(t) = a { if !escaping(*t) { self.register_wf(*t, pos); } }
                    }
                }
                PredicateKind::Clause(ClauseKind::TypeOutlives(t, _)) => { if !escaping(*t) { self.register_wf(*t, pos); } }
                PredicateKind::Clause(ClauseKind::Projection(pc)) => {
                    if let AliasTerm::Ty(a) = &pc.projection_term { if !escaping(*a) { self.register_wf(*a, pos); } }
                    if let Term::Ty(t) = pc.term { if !escaping(t) { self.register_wf(t, pos); } }
                }
                PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(t))) => { if !escaping(*t) { self.register_wf(*t, pos); } }
            PredicateKind::Clause(ClauseKind::ConstArgHasType(_, t)) => { if !escaping(*t) { self.register_wf(*t, pos); } }
                _ => {}
            }
        }

        /// wfcheck.rs check_where_clauses: every where-clause of the item (the parent's included) is
        /// well formed.
        fn check_where_clauses(&mut self, pos: u32) {
            for p in self.tcx.clauses_of(self.prog, self.def) {
                self.clause_obligations(&p, pos);
            }
        }

        /// wfcheck.rs check_false_global_bounds: a where-clause that mentions no parameter must hold
        /// (unless `trivial_bounds`); the clauses are elaborated, WellFormed ones left out.
        fn check_false_global_bounds(&mut self) {
            let d = self.prog.def(self.def);
            if self.prog.crates[d.krate as usize].features(self.prog).has("trivial_bounds") { return; }
            // type aliases bounds are ignored
            if d.kind == DefKind::TypeAlias { return; }
            let pos = self.prog.files[d.file as usize].ast.item(d.item).lo;
            let tys = &self.tcx.tys;
            for p in self.tcx.clauses_of(self.prog, self.def) {
                for p in self.tcx.supertraits(self.prog, p) {
                    if matches!(p.kind, PredicateKind::Clause(ClauseKind::WellFormed(_))) { continue; }
                    let flags = p.flags(tys);
                    let local = crate::types::TypeFlags::HAS_PARAM.bits() | crate::types::TypeFlags::HAS_ANY_INFER.bits() | crate::types::TypeFlags::HAS_PLACEHOLDER.bits() | crate::types::TypeFlags::HAS_BOUND_VARS.bits() | crate::types::TypeFlags::HAS_ALIAS.bits();
                    if flags & local != 0 || !p.bound_vars.is_empty() { continue; }
                    // the trait's own `Self: Trait` of a trait definition is not a user bound
                    if let PredicateKind::Clause(ClauseKind::Trait(t)) = &p.kind { if t.trait_ref.def_id == self.def { continue; } }
                    self.register(Goal { param_env: ParamEnvId::EMPTY, predicate: p }, pos);
                }
            }
        }

        /// Solve what was registered; an obligation that cannot hold is an error. Ambiguity is
        /// left to the rest of the check.
        fn finish(mut self, file: u32) -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for e in self.ocx.try_evaluate_obligations() {
                let (code, o) = match e {
                    NextSolverError::TrueError(o) => ("E0277", o),
                    NextSolverError::Overflow(o) => ("E0275", o),
                    NextSolverError::Ambiguity(_) => continue,
                };
                let Some(msg) = self.message(code, &o) else { continue };
                if !seen.insert((o.cause.span, msg.clone())) { continue; }
                out.push(self.prog.record_error(file, o.cause.span, "typeck", msg));
            }
            out
        }

        fn message(&self, code: &str, o: &PredicateObligation) -> Option<String> {
            fulfillment_message(self.infcx, self.prog, code, &o.goal)
        }
    }

    /// The failing leaf of a well-formedness goal: `WellFormed(T)` stands for the obligations of
    /// `T`'s components (wf.rs), and the first of them that cannot hold is what is reported.
    pub(crate) fn leaf_goal(infcx: &crate::infer::InferCtxt<'_>, prog: &Program, goal: &Goal, depth: u32) -> Goal {
        if depth > 8 { return goal.clone(); }
        if let PredicateKind::Clause(ClauseKind::WellFormed(term)) = &goal.predicate.kind {
            if let Ok(Some(goals)) = crate::next_solver::wf::unnormalized_obligations(infcx, prog, &goal.param_env, term.clone()) {
                for g in goals {
                    if g == *goal { continue; }
                    if matches!(infcx.evaluate_obligation(prog, g.clone()), crate::next_solver::evaluate_obligation::EvaluationResult::EvaluatedToErr) {
                        return leaf_goal(infcx, prog, &g, depth + 1);
                    }
                }
            }
        }
        goal.clone()
    }

    /// rustc's wording of an unmet obligation (report_selection_error); None: not one of ours to
    /// report (regions, constants).
    pub(crate) fn fulfillment_message(infcx: &crate::infer::InferCtxt<'_>, prog: &Program, code: &str, goal: &Goal) -> Option<String> {
        let tcx = infcx.tcx;
        let goal = leaf_goal(infcx, prog, goal, 0);
        let ty_str = |t: TyId| crate::typeck::ty_to_string(false, prog, tcx, infcx.deeply_resolve_ignoring_regions(t));
        match &goal.predicate.kind {
            PredicateKind::Clause(ClauseKind::Trait(p)) => Some(format!("{code}: {}", crate::typeck::trait_error_message(prog, tcx, code, p, &mut |t| ty_str(t)))),
            PredicateKind::Clause(ClauseKind::Projection(p)) => {
                let (AliasTerm::Ty(a), Term::Ty(t)) = (&p.projection_term, &p.term) else { return None };
                Some(format!("E0271: type mismatch resolving `{}` == `{}`", ty_str(*a), ty_str(*t)))
            }
            PredicateKind::DynCompatible(d) => Some(format!("E0038: the trait `{}` is not dyn compatible", prog.name(*d))),
            PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(t))) => Some(format!("E0277: the type `{}` is not well-formed", ty_str(*t))),
            // regions, constants and the rest belong to their own checks
            _ => None,
        }
    }

    fn with_wfcx(prog: &Program, tcx: &Tcx, def: DefId, f: impl FnOnce(&mut Wfcx<'_, '_>)) -> Vec<String> {
        let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), def);
        let env = tcx.param_env_id(prog, def);
        let mut w = Wfcx { prog, tcx, infcx: &infcx, ocx: ObligationCtxt::new(&infcx, prog), env, def, only_dyn: false };
        w.check_false_global_bounds();
        f(&mut w);
        w.finish(prog.def(def).file)
    }

    /// check_well_formed for one definition (a body-less, header-level check: bodies are the
    /// type checker's).
    pub fn check_def(prog: &Program, tcx: &Tcx, did: DefId) -> Vec<String> {
        let def = prog.def(did);
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&did) { return Vec::new(); }
        let ast = &prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        match def.kind {
            DefKind::Struct | DefKind::Enum | DefKind::Union => with_wfcx(prog, tcx, did, |w| check_type_defn(w, ast, item)),
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => with_wfcx(prog, tcx, did, |w| check_fn(w, ast, item)),
            DefKind::Impl => {
                let mut errors = check_drop_impl(prog, tcx, did, ast, item);
                errors.extend(with_wfcx(prog, tcx, did, |w| check_impl(w, ast, item)));
                errors
            }
            // (a trait's own environment is not built here: `param_env_id` of `trait Qux: Foo + AsRef<Self::Bar>` never
            // returns -- solver, crash lane)
            DefKind::Trait => {
                // dyn-compatibility of the `dyn` types its bounds mention needs no environment
                let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), did);
                let mut w = Wfcx { prog, tcx, infcx: &infcx, ocx: ObligationCtxt::new(&infcx, prog), env: ParamEnvId::EMPTY, def: did, only_dyn: true };
                w.check_where_clauses(item.lo);
                w.finish(def.file)
            }
            DefKind::Static => with_wfcx(prog, tcx, did, |w| check_static(w, ast, item)),
            DefKind::AssocConst => with_wfcx(prog, tcx, did, |w| check_assoc_const(w, ast, item)),
            DefKind::AssocTy => with_wfcx(prog, tcx, did, |w| check_assoc_ty(w, ast, item)),
            _ => Vec::new(),
        }
    }

    fn check_type_defn(w: &mut Wfcx<'_, '_>, ast: &ast::Ast, item: &ast::Item) {
        let Some(adt) = w.tcx.adts.get(&w.def) else { return };
        let mut fields: Vec<Vec<ast::FieldDef>> = Vec::new();
        let data = |d: &ast::VariantData| match d { ast::VariantData::Unit => Vec::new(), ast::VariantData::Tuple(l) | ast::VariantData::Struct(l) => ast.list(*l).to_vec() };
        let all_sized = match &item.kind {
            ItemKind::Struct(_) => { fields.push(data(&ast.as_struct(&item.kind).unwrap().data)); false }
            ItemKind::Union(_) => { fields.push(ast.list(ast.as_union(&item.kind).unwrap().fields).to_vec()); true }
            ItemKind::Enum(_) => { for v in ast.list(ast.as_enum(&item.kind).unwrap().variants) { fields.push(data(&v.data)); } true }
            _ => return,
        };
        let packed = adt.repr_packed != 0;
        for (variant, written) in adt.variants.iter().zip(&fields) {
            let n = variant.fields.len();
            let tys: Vec<TyId> = variant.fields.iter().map(|f| f.ty).collect();
            let pos_of = |i: usize| written.get(i).map_or(item.lo, |f| f.ty_span.0);
            for (i, &ty) in tys.iter().enumerate() {
                let ty = w.normalize(ty);
                w.register_wf(ty, pos_of(i));
            }
            // For DST, or when drop needs to copy things around (a packed type whose tail needs drop),
            // all intermediate types must be sized; all fields except possibly the last otherwise.
            let needs_drop_copy = packed && n > 0 && {
                let tail = w.tcx.tys.erase_regions(tys[n - 1]);
                !w.tcx.tys.has_infer(tail) && w.tcx.needs_drop(w.prog, crate::tcx::TypingEnv { limit: w.infcx.recursion_limit, param_env: w.env.clone(), mode: crate::tcx::TypingMode::non_body_analysis() }, tail)
            };
            let unsized_len = if all_sized || n == 0 || needs_drop_copy { 0 } else { 1 };
            for (i, &ty) in tys[..n - unsized_len].iter().enumerate() {
                let ty = w.normalize(ty);
                w.sized(ty, pos_of(i));
            }
        }
        w.check_where_clauses(item.lo);
    }

    /// The fn's signature with its late-bound regions liberated, one component at a time.
    fn check_fn(w: &mut Wfcx<'_, '_>, ast: &ast::Ast, item: &ast::Item) {
        let Some(sig) = w.tcx.fn_sig(w.prog, w.def).cloned() else { return };
        let ItemKind::Fn(s, body, _) = &item.kind else { return };
        let fsig = ast.fn_sig(*s);
        let has_body = body.is_some() || fsig.lazy_body.is_some();
        let liberate = |t: TyId| crate::next_solver::dyn_compatibility::liberate_late_bound_regions(w.tcx, w.def, t);
        let params: Vec<TyId> = sig.params.iter().map(|&t| liberate(t)).collect();
        let ret = liberate(sig.ret);
        let own = ast.list(fsig.params);
        let skip = params.len().saturating_sub(own.len());
        for (i, &ty) in params.iter().enumerate() {
            let pos = if i >= skip { ty_pos(ast, own[i - skip].ty).unwrap_or(item.name.lo) } else { item.name.lo };
            let ty = w.normalize(ty);
            w.register_wf(ty, pos);
        }
        let ret = w.normalize(ret);
        let ret_pos = fsig.ret.and_then(|t| ty_pos(ast, t)).unwrap_or(item.name.lo);
        w.register_wf(ret, ret_pos);
        w.check_where_clauses(item.lo);
        // If the function has a body, additionally require that the return type is sized.
        if has_body { w.sized(ret, ret_pos); }
    }

    fn check_impl(w: &mut Wfcx<'_, '_>, ast: &ast::Ast, item: &ast::Item) {
        let imp = ast.as_impl(&item.kind).unwrap();
        if imp.negative { return; }
        let indices = w.tcx.generic_indices_of(w.prog, w.def);
        let args = indices.identity_args(&w.tcx.tys);
        if w.tcx.impl_trait.contains_key(&w.def) {
            let trait_ref = w.tcx.impl_trait_ref(w.prog, w.def, &args);
            if trait_ref.args.iter().any(|a| matches!(*a, GenericArg::Type(t) if w.tcx.tys.has_error(t))) { return; }
            let self_pos = ty_pos(ast, imp.self_ty).unwrap_or(item.lo);
            // wf.rs trait_obligations: the trait's own predicates at the impl's arguments, and each
            // argument well formed
            let p = collect::trait_predicate(trait_ref.def_id, trait_ref.args.clone());
            let PredicateKind::Clause(ClauseKind::Trait(tc)) = &p.kind else { unreachable!() };
            let def = trait_ref.def_id;
            for q in w.tcx.clauses_of(w.prog, def) {
                if matches!(&q.kind, PredicateKind::Clause(ClauseKind::Trait(u)) if matches!(u.trait_ref.args.first(), Some(GenericArg::Type(t)) if matches!(w.tcx.tys.kind(*t), TyKind::Alias(..)))) { continue; }
                let q = w.tcx.instantiate_predicate(w.prog, def, &tc.trait_ref.args, &q);
                if crate::next_solver::wf::predicate_has_escaping_bound_vars(&w.tcx.tys, &q) { continue; }
                if matches!(&q.kind, PredicateKind::Clause(ClauseKind::Trait(u)) if u.trait_ref.def_id == def) { continue; }
                // Elaborate::All: what the trait's bounds imply holds too
                for e in w.tcx.supertraits(w.prog, q) {
                    if matches!(&e.kind, PredicateKind::Clause(ClauseKind::Trait(u)) if u.trait_ref.def_id == def) { continue; }
                    w.register(Goal { param_env: w.env.clone(), predicate: e }, self_pos);
                }
            }
            for a in &trait_ref.args {
                if let GenericArg::Type(t) = a { let t = w.normalize(*t); w.register_wf(t, self_pos); }
            }
        } else if let Some(&self_ty) = w.tcx.impl_self.get(&w.def) {
            let self_ty = w.normalize(self_ty);
            w.register_wf(self_ty, ty_pos(ast, imp.self_ty).unwrap_or(item.lo));
        }
        w.check_where_clauses(item.lo);
    }

    fn check_static(w: &mut Wfcx<'_, '_>, ast: &ast::Ast, item: &ast::Item) {
        let ItemKind::Static(mutable, _, _) = &item.kind else { return };
        let Some(ty) = w.tcx.const_ty(w.prog, w.def) else { return };
        // (a type with an unevaluated constant, `[u8; len(NAME)]`, is evaluated by the constant check)
        if w.tcx.tys.flags(ty).contains(crate::types::TypeFlags::HAS_ALIAS) { return; }
        let ty = w.normalize(ty);
        w.register_wf(ty, item.name.lo);
        w.sized(ty, item.name.lo);
        // Ensure that the end result is `Sync` in a non-thread local `static`.
        let attrs: Vec<ast::Attr> = ast.list(item.attrs).to_vec();
        let thread_local = relax_front::lang_items::has_attr(w.prog, w.prog.def(w.def).file, &attrs, "thread_local");
        if !*mutable && !thread_local {
            let sync = w.tcx.lang_items.get(crate::lang_items::LangItem::Sync);
            w.register_bound(ty, sync, item.name.lo);
        }
    }

    fn check_assoc_const(w: &mut Wfcx<'_, '_>, _ast: &ast::Ast, item: &ast::Item) {
        let Some(ty) = w.tcx.const_ty(w.prog, w.def) else { return };
        let ty = w.normalize(ty);
        w.register_wf(ty, item.name.lo);
        if let ItemKind::Const(_, init) = &item.kind { if init.is_some() { w.sized(ty, item.name.lo); } }
    }

    fn check_assoc_ty(w: &mut Wfcx<'_, '_>, ast: &ast::Ast, item: &ast::Item) {
        let Some(alias) = ast.as_alias(&item.kind) else { return };
        if alias.ty.is_none() { return; }
        let Ok(ty) = w.tcx.type_of(w.prog, w.def) else { return };
        let ty = w.normalize(ty);
        w.register_wf(ty, item.name.lo);
    }

    /// Where a written type starts: types carry no span of their own, a path its position.
    fn ty_pos(ast: &ast::Ast, t: ast::TyId) -> Option<u32> {
        match ast.ty(t) {
            ast::Ty::Path(p) => Some(p.lo),
            ast::Ty::Ref(_, _, i) | ast::Ty::Ptr(_, i) | ast::Ty::Slice(i) | ast::Ty::Array(i, _) | ast::Ty::Paren(i) => ty_pos(ast, *i),
            ast::Ty::Tuple(l) => ast.list(*l).first().and_then(|&i| ty_pos(ast, i)),
            _ => None,
        }
    }

    /// A plain `fn main()` of the Rust ABI: `#[start]` / `#[no_mangle]` / `extern \"C\" fn main` are other entry points.
    pub fn is_entry_main(prog: &Program, main: DefId) -> bool {
        let def = prog.def(main);
        let ast = &prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        let ItemKind::Fn(s, _, _) = &item.kind else { return false };
        let fsig = ast.fn_sig(*s);
        let attrs: Vec<ast::Attr> = ast.list(item.attrs).to_vec();
        let other_entry = ["start", "no_mangle", "export_name"].iter().any(|n| relax_front::lang_items::has_attr(prog, def.file, &attrs, n));
        fsig.abi.is_none() && fsig.params.is_empty() && fsig.self_param.is_none() && !other_entry
    }

    /// rustc_hir_analysis/src/check/entry.rs check_main_fn_ty: the return type of the entry
    /// function implements `Termination`.
    pub fn check_main(prog: &Program, tcx: &Tcx, main: DefId) -> Vec<String> {
        let def = prog.def(main);
        let ast = &prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        let ItemKind::Fn(s, _, _) = &item.kind else { return Vec::new() };
        let Some(term) = tcx.lang_items.get(crate::lang_items::LangItem::Termination) else { return Vec::new() };
        let Some(sig) = tcx.fn_sig(prog, main) else { return Vec::new() };
        if sig.n_generics != 0 || tcx.tys.has_error(sig.ret) { return Vec::new(); }
        let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), main);
        let env = ParamEnvId::EMPTY;
        let mut ocx = ObligationCtxt::new(&infcx, prog);
        let ret = ocx.normalize_ty(&env, sig.ret);
        ocx.register_obligation(Goal { param_env: env.clone(), predicate: collect::trait_predicate(term, vec![GenericArg::Type(ret)]) });
        let bad = ocx.try_evaluate_obligations().into_iter().any(|e| matches!(e, NextSolverError::TrueError(_)));
        if !bad { return Vec::new(); }
        let pos = ast.fn_sig(*s).ret.and_then(|t| ty_pos(ast, t)).unwrap_or(item.name.lo);
        let shown = crate::typeck::ty_to_string(false, prog, tcx, infcx.deeply_resolve_ignoring_regions(ret));
        vec![prog.record_error(def.file, pos, "typeck", format!("E0277: `main` has invalid return type `{}`", shown))]
    }

    /// rustc_hir_analysis/src/coherence/builtin.rs visit_implementation_of_drop: `Drop` is implemented
    /// for structs, enums and unions of the crate only (E0120).
    fn check_drop_impl(prog: &Program, tcx: &Tcx, imp: DefId, ast: &ast::Ast, item: &ast::Item) -> Vec<String> {
        let Some(&td) = tcx.impl_trait.get(&imp) else { return Vec::new() };
        if tcx.lang_items.get(crate::lang_items::LangItem::Drop) != Some(td) { return Vec::new(); }
        let Some(&self_ty) = tcx.impl_self.get(&imp) else { return Vec::new() };
        let def = prog.def(imp);
        match tcx.tys.kind(self_ty) {
            TyKind::Error => Vec::new(),
            TyKind::Adt(d, _, _) if prog.def(*d).krate == def.krate => Vec::new(),
            _ => {
                let pos = ast.as_impl(&item.kind).and_then(|i| ty_pos(ast, i.self_ty)).unwrap_or(item.lo);
                vec![prog.record_error(def.file, pos, "typeck", "E0120: the `Drop` trait may only be implemented for local structs, enums, and unions".to_string())]
            }
        }
    }

    /// Whether every ADT / fn item / alias in `ty` carries as many arguments as its definition has
    /// parameters; a path with the wrong count is reported (E0107) and has no well-formedness.
    pub(crate) fn arity_ok(tcx: &Tcx, prog: &Program, ty: TyId) -> bool {
        use crate::types::fold::{TypeFolder, super_fold_ty};
        use crate::types::{Types, AliasTyKind};
        struct V<'a> { tcx: &'a Tcx, prog: &'a Program, ok: bool }
        impl TypeFolder for V<'_> {
            fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
                if !self.ok { return t; }
                let (def, n, r) = match tys.kind(t) {
                    TyKind::Adt(d, a, r) | TyKind::FnDef(d, a, r) => (Some(*d), tys.list(*a).len(), tys.region_args(*r).len()),
                    TyKind::Alias(_, a) if matches!(a.kind, AliasTyKind::Projection { .. }) => (Some(a.kind.def_id()), tys.list(a.args).len(), tys.region_args(a.regions).len()),
                    _ => (None, 0, 0),
                };
                if let TyKind::Dyn(p, a, _, r, ..) = tys.kind(t) {
                    let ix = self.tcx.generic_indices_of(self.prog, *p);
                    if ix.types_consts.len() != tys.list(*a).len() + 1 || ix.lifetimes.len() != tys.region_args(*r).len() { self.ok = false; return t; }
                }
                if let Some(d) = def {
                    let ix = self.tcx.generic_indices_of(self.prog, d);
                    if ix.types_consts.len() != n || ix.lifetimes.len() != r { self.ok = false; return t; }
                    // each slot's kind (type or const) must match the declaration's
                    let args = match tys.kind(t) { TyKind::Adt(_, a, _) | TyKind::FnDef(_, a, _) => Some(*a), TyKind::Alias(_, a) => Some(a.args), _ => None };
                    if let Some(a) = args {
                        let decl = self.tcx.declaration_env(self.prog, d);
                        let off = if matches!(tys.kind(t), TyKind::Alias(..)) { 0 } else { 0 };
                        for (i, &arg) in tys.list(a).iter().enumerate() {
                            let want_const = decl.consts.iter().any(|c| c.0 as usize == i + off);
                            if want_const != matches!(tys.kind(arg), TyKind::Const(_)) { self.ok = false; return t; }
                        }
                    }
                }
                super_fold_ty(tys, t, self, depth)
            }
        }
        let mut v = V { tcx, prog, ok: true };
        v.fold_ty(&tcx.tys, ty, 0);
        v.ok
    }
}
