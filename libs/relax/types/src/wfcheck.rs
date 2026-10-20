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
        /// errors of checks that are not obligations (region outlives)
        extra: Vec<String>,
        /// rustc `assumed_wf_types`: the types whose well-formedness the item's regions may assume
        assumed_wf: Vec<TyId>,
        /// the normalized form of each type already normalized (one fresh region variable set per type, as rustc's wfcx shares)
        normalized: std::collections::HashMap<TyId, TyId>,
    }

    impl<'a, 'tcx> Wfcx<'a, 'tcx> {
        fn register(&mut self, goal: Goal, pos: u32) {
            self.ocx.engine.register_predicate_obligation(self.infcx, PredicateObligation { goal, recursion_depth: 0, cause: ObligationCause { span: pos, body_id: self.def } });
        }

        /// wfcheck.rs WfCheckingCtxt::deeply_normalize. A type that cannot be normalized stays as
        /// written: its own well-formedness reports the unmet bound.
        fn normalize(&mut self, ty: TyId) -> TyId {
            if !self.tcx.tys.has_assoc(ty) { return ty; }
            if let Some(&done) = self.normalized.get(&ty) { return done; }
            let mut ocx = ObligationCtxt::new(self.infcx, self.prog);
            ocx.cause = ObligationCause { span: 0, body_id: self.def };
            let done = ocx.deeply_normalize_ty(&self.env, ty).unwrap_or(ty);
            self.normalized.insert(ty, done);
            done
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
            let mut out: Vec<String> = std::mem::take(&mut self.extra);
            let mut seen = std::collections::HashSet::new();
            // wfcheck.rs: `select_all_or_error` -- an ambiguous obligation is an error (E0283)
            let fulfillment = self.ocx.engine.evaluate_obligations_error_on_ambiguity(self.infcx, self.prog);
            let fulfilled = fulfillment.is_empty();
            for e in fulfillment {
                let (code, o) = match e {
                    NextSolverError::TrueError(o) => ("E0277", o),
                    NextSolverError::Overflow(o) => ("E0275", o),
                    NextSolverError::Ambiguity(o) => ("E0283", o),
                };
                // (an obligation that mentions an error type is a consequence of that error)
                if predicate_mentions_error(self.infcx, &o.goal.predicate) { continue; }
                let Some(msg) = self.message(code, &o) else { continue };
                if !seen.insert((o.cause.span, msg.clone())) { continue; }
                out.push(self.prog.record_error(file, o.cause.span, "typeck", msg));
            }
            // wfcheck.rs `resolve_regions_and_report_errors`: the region obligations the well-formedness goals registered,
            // under the item's outlives environment and the types it may assume well formed.
            if fulfilled {
                let outlives = self.outlives_environment();
                for e in self.infcx.resolve_regions_with_outlives_env(self.prog, &outlives) {
                    if let crate::infer::lexical_region_resolve::RegionResolutionError::GenericBoundFailure(ty, sub, span) = e {
                        if let Some(msg) = self.generic_bound_failure(ty, sub) {
                            let span = if span == 0 { self.prog.files[file as usize].ast.item(self.prog.def(self.def).item).lo } else { span };
                            if seen.insert((span, msg.clone())) { out.push(self.prog.record_error(file, span, "typeck", msg)); }
                        }
                    }
                }
            }
            out
        }

        /// rustc `OutlivesEnvironment::new` of a well-formedness check: the item's outlives clauses (an ADT's
        /// `inferred_outlives_of` among them) and the bounds the assumed well-formed types imply, those the contracts
        /// of the types they name give (RFC 2093 included) next to the ones the implied-bounds query finds.
        fn outlives_environment(&self) -> crate::infer::outlives::env::OutlivesEnvironment {
            use crate::infer::outlives::env::{OutlivesBound, OutlivesEnvironment};
            use crate::tcx::outlives::OutlivesPredicate;
            let tcx = self.tcx;
            let tys = &tcx.tys;
            let param_env = tcx.param_env_by_id(self.env.clone()).clone();
            let known: Vec<Predicate> = param_env.clauses.iter().filter(|p| matches!(p.kind, PredicateKind::Clause(ClauseKind::TypeOutlives(..)))).cloned().collect();
            let mut bounds = Vec::new();
            for &ty in &self.assumed_wf { bounds.extend(crate::next_solver::implied_outlives_bounds::implied_outlives_bounds(self.infcx, self.prog, &self.env, ty)); }
            // (region variables the query's equalities unified are one: the region of an obligation resolves the same way)
            let mut resolver = crate::infer::resolve::DeepVariableResolver::new(self.infcx);
            use crate::types::fold::TypeFolder;
            for bound in &mut bounds {
                *bound = match *bound {
                    OutlivesBound::RegionSubRegion(a, b) => OutlivesBound::RegionSubRegion(resolver.fold_region(tys, a, 0), resolver.fold_region(tys, b, 0)),
                    OutlivesBound::RegionSubParam(r, t) => OutlivesBound::RegionSubParam(resolver.fold_region(tys, r, 0), resolver.fold_ty(tys, t, 0)),
                    OutlivesBound::RegionSubAlias(r, t) => OutlivesBound::RegionSubAlias(resolver.fold_region(tys, r, 0), resolver.fold_ty(tys, t, 0)),
                };
            }
            if tcx.adts.contains_key(&self.def) {
                // (the type's own `inferred_outlives_of` are among its predicates)
                let mut pairs = Vec::new();
                for p in crate::borrowck::implied_bounds::ImpliedBounds::inferred_outlives(tcx, self.prog, self.def) {
                    match p {
                        OutlivesPredicate::Region(a, b) => bounds.push(OutlivesBound::RegionSubRegion(b, a)),
                        OutlivesPredicate::Type(t, r) => pairs.push(if matches!(tys.kind(t), TyKind::Alias(..)) { OutlivesBound::RegionSubAlias(r, t) } else { OutlivesBound::RegionSubParam(r, t) }),
                    }
                }
                bounds.extend(pairs);
            }
            OutlivesEnvironment::from_normalized_bounds(tys, param_env, known, bounds)
        }

        /// `construct_generic_bound_failure` (E0309, E0310, E0311): the message of a type that must outlive `sub`
        /// and cannot be shown to.
        fn generic_bound_failure(&self, ty: TyId, sub: crate::types::RegionId) -> Option<String> {
            use crate::types::{LateParamKind, RegionKind};
            let tcx = self.tcx;
            let ty = self.infcx.deeply_resolve_ignoring_regions(ty);
            let what = match tcx.tys.kind(ty) {
                TyKind::Param(_) => "parameter type",
                TyKind::Placeholder(..) => "placeholder type",
                TyKind::Alias(..) => "associated type",
                _ => return None,
            };
            let named = |r| -> bool {
                match tcx.tys.region_kind(r) {
                    RegionKind::ReLateParam(_, LateParamKind::Named(_)) => true,
                    RegionKind::ReEarlyParam(_) => {
                        let contract = tcx.fn_sig(self.prog, self.def).map(|s| &s.region_contract).or_else(|| tcx.nominal_region_contracts.get(&self.def));
                        contract.and_then(|c| c.regions.iter().find(|(_, x)| *x == r)).map_or(true, |(n, _)| n != "'_")
                    }
                    _ => false,
                }
            };
            let code = match tcx.tys.region_kind(sub) {
                RegionKind::ReStatic => "E0310",
                RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..) if named(sub) => "E0309",
                _ => "E0311",
            };
            Some(format!("{code}: the {what} `{}` may not live long enough", crate::typeck::ty_to_string(false, self.prog, tcx, ty)))
        }

        fn message(&self, code: &str, o: &PredicateObligation) -> Option<String> {
            fulfillment_message(self.infcx, self.prog, code, &o.goal)
        }
    }

    /// rustc `references_error()`: a predicate that mentions an error type or constant (once its
    /// inference variables are resolved) is the consequence of an error already reported.
    pub(crate) fn predicate_mentions_error(infcx: &crate::infer::InferCtxt<'_>, pred: &Predicate) -> bool {
        struct HasError<'t, 'u>(&'t crate::types::Types, bool, &'t crate::infer::InferCtxt<'u>);
        impl crate::types::fold::TypeFolder for HasError<'_, '_> {
            fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
                let ty = self.2.deeply_resolve_ignoring_regions(ty);
                if self.0.has_error(ty) { self.1 = true; return ty; }
                crate::types::fold::super_fold_ty(tys, ty, self, depth)
            }
            fn fold_const(&mut self, tys: &crate::types::Types, ct: crate::consts::Const, depth: u32) -> crate::consts::Const {
                if matches!(tys.const_kind(ct), crate::consts::ConstKind::Error) { self.1 = true; return ct; }
                crate::types::fold::super_fold_const(tys, ct, self, depth)
            }
        }
        let mut h = HasError(&infcx.tcx.tys, false, infcx);
        crate::next_solver::fold::fold_predicate(&infcx.tcx.tys, pred, &mut h);
        h.1
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

    /// rustc_ty_utils implied_bounds.rs `assumed_wf_types`: a fn assumes its liberated inputs and output (and its impl's
    /// header), an impl its header (the self type and trait arguments), an associated item its parent's.
    fn assumed_wf_types(w: &mut Wfcx<'_, '_>, def: DefId) -> Vec<TyId> {
        let (prog, tcx) = (w.prog, w.tcx);
        let d = prog.def(def);
        let mut out = Vec::new();
        let header = |imp: DefId| -> Vec<TyId> {
            let mut v = vec![];
            if let Some(&t) = tcx.impl_self.get(&imp) { v.push(t); }
            if let Some(args) = tcx.impl_trait_args.get(&imp) { v.extend_from_slice(args); }
            v
        };
        let parent_impl = (d.parent != crate::program::NO_DEF && prog.def(d.parent).kind == DefKind::Impl).then_some(d.parent);
        match d.kind {
            DefKind::Fn | DefKind::AssocFn => {
                if let Some(imp) = parent_impl { out.extend(header(imp)); }
                if let Some(sig) = tcx.fn_sig(prog, def) {
                    let liberate = |t: TyId| crate::next_solver::dyn_compatibility::liberate_late_bound_regions(tcx, def, t);
                    out.extend(sig.params.iter().map(|&t| liberate(t)));
                    out.push(liberate(sig.ret));
                }
            }
            DefKind::Impl => out.extend(header(def)),
            DefKind::AssocConst | DefKind::AssocTy => { if let Some(imp) = parent_impl { out.extend(header(imp)); } }
            _ => {}
        }
        out.into_iter().map(|t| w.normalize(t)).collect()
    }

    fn with_wfcx(prog: &Program, tcx: &Tcx, def: DefId, f: impl FnOnce(&mut Wfcx<'_, '_>)) -> Vec<String> {
        let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), def);
        let env = tcx.param_env_id(prog, def);
        let mut w = Wfcx { prog, tcx, infcx: &infcx, ocx: ObligationCtxt::new(&infcx, prog), env, def, extra: Vec::new(), assumed_wf: Vec::new(), normalized: Default::default() };
        w.assumed_wf = assumed_wf_types(&mut w, def);
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
            DefKind::Struct | DefKind::Enum | DefKind::Union => with_wfcx(prog, tcx, did, |w| { check_type_defn(w, ast, item); check_defaults(w, ast, item); }),
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                let mut errors = with_wfcx(prog, tcx, did, |w| check_fn(w, ast, item));
                if def.kind != DefKind::ForeignFn { errors.extend(check_opaque_cycles(prog, tcx, did, item.lo)); }
                if matches!(item.kind, ItemKind::Fn(sig, _, _) if ast.fn_sig(sig).asyncness) { errors.extend(check_async_recursion(prog, tcx, did, item.lo)); }
                errors
            }
            DefKind::Impl => {
                let mut errors = check_drop_impl(prog, tcx, did, ast, item);
                errors.extend(with_wfcx(prog, tcx, did, |w| check_impl(w, ast, item)));
                errors
            }
            DefKind::Trait => with_wfcx(prog, tcx, did, |w| { check_defaults(w, ast, item); w.check_where_clauses(item.lo); }),
            DefKind::Static | DefKind::ForeignStatic => with_wfcx(prog, tcx, did, |w| check_static(w, ast, item)),
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
        for (variant, written) in adt.variants.iter().zip(&fields) {
            for (i, f) in variant.fields.iter().enumerate() {
                dyn_object_bounds(w, f.ty, written.get(i).map_or(item.lo, |f| f.ty_span.0));
            }
        }
    }

    /// E0478 for the `dyn Trait<'a> + 'b` types in a type definition's fields: the object lifetime
    /// bound `'b` must outlive the lifetimes the trait's supertraits require of `Self`
    /// (rustc wf `object_region_bounds`). Only early-bound parameter regions and `'static` are judged.
    fn dyn_object_bounds(w: &mut Wfcx<'_, '_>, ty: TyId, pos: u32) {
        let (prog, tcx) = (w.prog, w.tcx);
        let mut dyns: Vec<(DefId, Vec<crate::types::RegionId>, crate::types::RegionId)> = Vec::new();
        fn find(tcx: &Tcx, t: TyId, out: &mut Vec<(DefId, Vec<crate::types::RegionId>, crate::types::RegionId)>, depth: u32) {
            if depth > 16 { return; }
            match tcx.tys.kind(t).clone() {
                TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) | TyKind::Slice(i) | TyKind::Array(i, _) => find(tcx, i, out, depth + 1),
                TyKind::Tuple(l) => for &x in tcx.tys.list(l) { find(tcx, x, out, depth + 1) },
                TyKind::Adt(_, ts, _) => for &x in tcx.tys.list(ts) { find(tcx, x, out, depth + 1) },
                TyKind::Dyn(p, _, _, rs, r, _, _) => out.push((p, tcx.tys.region_args(rs).to_vec(), r)),
                _ => {}
            }
        }
        find(tcx, ty, &mut dyns, 0);
        if dyns.is_empty() { return; }
        let mut known: Vec<(crate::types::RegionId, crate::types::RegionId)> = Vec::new();
        for q in tcx.clauses_of(prog, w.def) {
            match &q.kind {
                PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)) => known.push((*a, *b)),
                PredicateKind::Clause(ClauseKind::TypeOutlives(..)) => return,
                _ => {}
            }
        }
        let adt = tcx.adts.get(&w.def);
        if let Some(adt) = adt {
            for v in &adt.variants { for f in &v.fields { let mut part = Vec::new(); if wf_region_pairs(tcx, f.ty, &mut part, 0).is_some() { known.extend(part); } } }
        }
        let kind = |r: crate::types::RegionId| tcx.tys.region_kind(r);
        let simple = |r: crate::types::RegionId| matches!(kind(r), crate::types::RegionKind::ReEarlyParam(_) | crate::types::RegionKind::ReStatic);
        if known.iter().any(|&(a, b)| !simple(a) || !simple(b)) { return; }
        let holds = |from: crate::types::RegionId, to: crate::types::RegionId| -> bool {
            if from == to || kind(from) == crate::types::RegionKind::ReStatic { return true; }
            let mut seen = vec![from];
            let mut i = 0;
            while i < seen.len() {
                let cur = seen[i];
                i += 1;
                for &(a, b) in &known {
                    if a == cur && !seen.contains(&b) {
                        if b == to { return true; }
                        seen.push(b);
                    }
                }
            }
            false
        };
        for (principal, rs, obj) in dyns {
            if !simple(obj) { continue; }
            for q in tcx.clauses_of(prog, principal) {
                let PredicateKind::Clause(ClauseKind::TypeOutlives(t, r)) = &q.kind else { continue };
                if !matches!(tcx.tys.kind(*t), TyKind::Param(0)) { continue; }
                let required = match kind(*r) {
                    crate::types::RegionKind::ReStatic => Some(*r),
                    // (a trait's parameter 0 is `Self`; its lifetimes follow)
                    crate::types::RegionKind::ReEarlyParam(k) if k >= 1 => rs.get(k as usize - 1).copied(),
                    _ => None,
                };
                let Some(required) = required else { continue };
                if !simple(required) { continue; }
                if !holds(obj, required) {
                    w.extra.push(prog.record_error(prog.def(w.def).file, pos, "typeck", "E0478: lifetime bound not satisfied".to_string()));
                    return;
                }
            }
        }
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

    /// E0478 for a trait whose supertraits include `'static` (`trait T: 'static`): the impl's self
    /// type must outlive `'static`, so each region in it must be `'static` or be bounded by
    /// `'static` in the impl's where-clauses. Type parameters are not judged here.
    fn impl_static_supertrait(w: &mut Wfcx<'_, '_>, tr: &crate::next_solver::TraitRef, pos: u32) {
        let (prog, tcx) = (w.prog, w.tcx);
        let static_super = tcx.clauses_of(prog, tr.def_id).iter().any(|q| matches!(&q.kind, PredicateKind::Clause(ClauseKind::TypeOutlives(t, r))
            if matches!(tcx.tys.kind(*t), TyKind::Param(0)) && tcx.tys.region_kind(*r) == crate::types::RegionKind::ReStatic));
        if !static_super { return; }
        let Some(GenericArg::Type(self_ty)) = tr.args.first().copied() else { return };
        fn regions(tcx: &Tcx, t: TyId, out: &mut Vec<crate::types::RegionId>, depth: u32) -> bool {
            if depth > 16 { return false; }
            match tcx.tys.kind(t).clone() {
                TyKind::Ref(_, i, r) => { out.push(r); regions(tcx, i, out, depth + 1) }
                TyKind::Ptr(_, i) | TyKind::Slice(i) | TyKind::Array(i, _) => regions(tcx, i, out, depth + 1),
                TyKind::Tuple(l) => tcx.tys.list(l).to_vec().into_iter().all(|x| regions(tcx, x, out, depth + 1)),
                TyKind::Adt(_, ts, rs) => { out.extend(tcx.tys.region_args(rs).iter().copied()); tcx.tys.list(ts).to_vec().into_iter().all(|x| regions(tcx, x, out, depth + 1)) }
                TyKind::Param(_) | TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Str | TyKind::Never => true,
                _ => false,
            }
        }
        let mut rs = Vec::new();
        if !regions(tcx, self_ty, &mut rs, 0) { return; }
        let mut bounded: Vec<crate::types::RegionId> = Vec::new();
        for q in tcx.clauses_of(prog, w.def) {
            match &q.kind {
                PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)) if tcx.tys.region_kind(*b) == crate::types::RegionKind::ReStatic => bounded.push(*a),
                PredicateKind::Clause(ClauseKind::RegionOutlives(..)) | PredicateKind::Clause(ClauseKind::TypeOutlives(..)) => return,
                _ => {}
            }
        }
        for r in rs {
            let k = tcx.tys.region_kind(r);
            if !matches!(k, crate::types::RegionKind::ReEarlyParam(_)) || bounded.contains(&r) { continue; }
            w.extra.push(prog.record_error(prog.def(w.def).file, pos, "typeck", "E0478: lifetime bound not satisfied".to_string()));
            return;
        }
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
            impl_static_supertrait(w, &trait_ref, self_pos);
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
        // an `extern` static may be unsized only through an extern type tail
        let foreign = w.prog.def(w.def).kind == DefKind::ForeignStatic;
        if !(foreign && matches!(w.tcx.tys.kind(ty), TyKind::Foreign(_))) { w.sized(ty, item.name.lo); }
        if foreign { return; }
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
        region_outlives_in_impl_assoc_ty(w, ty, item.name.lo);
    }

    /// The region facts a type's well-formedness yields (rustc `wf::obligations` /
    /// `implied_bounds`): for `&'r T`, every region of `T` outlives `'r`. Pairs (longer, shorter);
    /// None when the type has a part this reduced walk does not follow (aliases, dyn, fn pointers).
    fn wf_region_pairs(tcx: &Tcx, t: TyId, out: &mut Vec<(crate::types::RegionId, crate::types::RegionId)>, depth: u32) -> Option<()> {
        if depth > 32 { return None; }
        fn regions_in(tcx: &Tcx, t: TyId, out: &mut Vec<crate::types::RegionId>, depth: u32) -> Option<()> {
            if depth > 32 { return None; }
            match tcx.tys.kind(t).clone() {
                TyKind::Ref(_, i, r) => { out.push(r); regions_in(tcx, i, out, depth + 1) }
                TyKind::Ptr(_, i) | TyKind::Slice(i) | TyKind::Array(i, _) => regions_in(tcx, i, out, depth + 1),
                TyKind::Tuple(l) => { for &x in tcx.tys.list(l) { regions_in(tcx, x, out, depth + 1)?; } Some(()) }
                TyKind::Adt(_, ts, rs) => {
                    for &x in tcx.tys.list(ts) { regions_in(tcx, x, out, depth + 1)?; }
                    out.extend(tcx.tys.region_args(rs).iter().copied());
                    Some(())
                }
                TyKind::Param(_) | TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Str | TyKind::Never => Some(()),
                _ => None,
            }
        }
        match tcx.tys.kind(t).clone() {
            TyKind::Ref(_, i, r) => {
                let mut inner = Vec::new();
                regions_in(tcx, i, &mut inner, depth + 1)?;
                out.extend(inner.into_iter().map(|x| (x, r)));
                wf_region_pairs(tcx, i, out, depth + 1)
            }
            TyKind::Ptr(_, i) | TyKind::Slice(i) | TyKind::Array(i, _) => wf_region_pairs(tcx, i, out, depth + 1),
            TyKind::Tuple(l) => { for &x in tcx.tys.list(l) { wf_region_pairs(tcx, x, out, depth + 1)?; } Some(()) }
            TyKind::Adt(_, ts, _) => { for &x in tcx.tys.list(ts) { wf_region_pairs(tcx, x, out, depth + 1)?; } Some(()) }
            TyKind::Param(_) | TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Str | TyKind::Never => Some(()),
            _ => None,
        }
    }

    /// E0491 for the type of an associated type of a trait impl: a reference `&'r T` needs the
    /// regions of `T` to outlive `'r`, which the impl's where-clauses and the implied bounds of
    /// its header must provide (rustc region resolution of the WF obligations). Only early-bound
    /// parameter regions and `'static` are judged; any part this walk cannot follow skips it.
    fn region_outlives_in_impl_assoc_ty(w: &mut Wfcx<'_, '_>, ty: TyId, pos: u32) {
        let (prog, tcx) = (w.prog, w.tcx);
        let def = prog.def(w.def);
        let parent = def.parent;
        if def.kind != DefKind::AssocTy || prog.def(parent).kind != DefKind::Impl || !tcx.impl_trait.contains_key(&parent) { return; }
        let mut need = Vec::new();
        if wf_region_pairs(tcx, ty, &mut need, 0).is_none() || need.is_empty() { return; }
        let indices = tcx.generic_indices_of(prog, parent);
        let args = indices.identity_args(&tcx.tys);
        let tr = tcx.impl_trait_ref(prog, parent, &args);
        let mut known: Vec<(crate::types::RegionId, crate::types::RegionId)> = Vec::new();
        for a in tr.args.iter() {
            if let GenericArg::Type(t) = a { let t = w.normalize(*t); if wf_region_pairs(tcx, t, &mut known, 0).is_none() { return; } }
        }
        for q in tcx.clauses_of(prog, w.def) {
            if let PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)) = &q.kind { known.push((*a, *b)); }
            if matches!(&q.kind, PredicateKind::Clause(ClauseKind::TypeOutlives(..))) { return; }
        }
        let kind = |r: crate::types::RegionId| tcx.tys.region_kind(r);
        let simple = |r: crate::types::RegionId| matches!(kind(r), crate::types::RegionKind::ReEarlyParam(_) | crate::types::RegionKind::ReStatic);
        if need.iter().any(|&(a, b)| !simple(a) || !simple(b)) || known.iter().any(|&(a, b)| !simple(a) || !simple(b)) { return; }
        let holds = |from: crate::types::RegionId, to: crate::types::RegionId| -> bool {
            if from == to || kind(from) == crate::types::RegionKind::ReStatic { return true; }
            let mut seen = vec![from];
            let mut i = 0;
            while i < seen.len() {
                let cur = seen[i];
                i += 1;
                for &(a, b) in &known {
                    if a == cur && !seen.contains(&b) {
                        if b == to || false { return true; }
                        seen.push(b);
                    }
                }
            }
            false
        };
        for &(from, to) in &need {
            if !holds(from, to) {
                let shown = crate::typeck::ty_to_string(false, prog, tcx, w.infcx.deeply_resolve_ignoring_regions(ty));
                w.extra.push(prog.record_error(def.file, pos, "typeck", format!("E0491: in type `{}`, reference has a longer lifetime than the data it references", shown)));
                return;
            }
        }
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
            TyKind::Adt(d, args, regions) if prog.def(*d).krate == def.krate => always_applicable(prog, tcx, imp, item, def.file, *d, *args, *regions),
            _ => {
                let pos = ast.as_impl(&item.kind).and_then(|i| ty_pos(ast, i.self_ty)).unwrap_or(item.lo);
                vec![prog.record_error(def.file, pos, "typeck", "E0120: the `Drop` trait may only be implemented for local structs, enums, and unions".to_string())]
            }
        }
    }

    /// rustc_hir_analysis/src/check/always_applicable.rs check_drop_impl: the generic arguments of the
    /// impl's self type are the impl's own distinct parameters (E0366), and every predicate of the impl
    /// holds in the environment of the type's definition (E0367).
    fn always_applicable(prog: &Program, tcx: &Tcx, imp: DefId, item: &ast::Item, file: u32, adt: DefId, args: crate::types::TyList, regions: crate::types::RegionArgsId) -> Vec<String> {
        use crate::types::RegionKind;
        let tys = &tcx.tys;
        // uses_unique_generic_params(adt_to_impl_args, CheckRegions::OnlyParam)
        let mut seen_tys = Vec::new();
        let mut unique = true;
        for &a in tys.list(args) {
            let ok = match tys.kind(a) {
                TyKind::Param(..) => true,
                TyKind::Const(c) => matches!(tys.const_kind(*c), crate::consts::ConstKind::Param(..)),
                _ => false,
            };
            if !ok || seen_tys.contains(&a) { unique = false; }
            seen_tys.push(a);
        }
        let mut seen_regions = Vec::new();
        for &r in tys.region_args(regions).iter() {
            if !matches!(tys.region_kind(r), RegionKind::ReEarlyParam(_)) || seen_regions.contains(&r) { unique = false; }
            seen_regions.push(r);
        }
        let trait_name = "Drop";
        if !unique {
            return vec![prog.record_error(file, item.lo, "typeck", format!("E0366: `{}` impls cannot be specialized", trait_name))];
        }
        if tys.list(args).iter().any(|&t| tys.has_error(t)) { return Vec::new(); }
        // the predicates of the type, at the impl's arguments
        let adt_args = tcx.complete_args(prog, adt, tys.list(args), regions);
        let adt_env = tcx.param_env_id(prog, adt);
        let clauses: Vec<_> = tcx.param_env_by_id(adt_env).clauses.iter().map(|p| tcx.instantiate_predicate(prog, adt, &adt_args, p)).collect();
        let env = tcx.intern_param_env(crate::next_solver::ParamEnv { clauses: clauses.into() });
        let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), imp);
        let mut ocx = ObligationCtxt::new(&infcx, prog);
        // the impl's parameters are inference variables, bound by equating its self type with the
        // type at the impl's own arguments
        let fresh = infcx.fresh_args_for_item(prog, imp);
        let fresh_self = tcx.instantiate_ty(prog, imp, &fresh, tcx.impl_self[&imp]);
        let mut goals = match infcx.at(prog, env.clone()).eq(tcx.impl_self[&imp], fresh_self) {
            Ok(goals) => goals,
            Err(_) => return Vec::new(),
        };
        for p in tcx.own_clauses_of(prog, imp) {
            let p = &tcx.instantiate_predicate(prog, imp, &fresh, p);
            let predicate = ocx.normalize_predicate(&env, p);
            goals.push(Goal { param_env: env.clone(), predicate });
        }
        ocx.register_obligations(goals);
        let errors = ocx.engine.evaluate_obligations_error_on_ambiguity(&infcx, prog);
        let msg = |what: String| format!("E0367: `{}` impl requires `{}` but the {} it is implemented for does not", trait_name, what, match prog.def(adt).kind { DefKind::Enum => "enum", DefKind::Union => "union", _ => "struct" });
        if !errors.is_empty() {
            return vec![prog.record_error(file, item.lo, "typeck", msg("a predicate".to_string()))];
        }
        if !infcx.resolve_regions(prog, env, &[]).is_empty() {
            return vec![prog.record_error(file, item.lo, "typeck", msg("an outlives bound".to_string()))];
        }
        Vec::new()
    }

    /// rustc_hir_analysis/src/check/check.rs check_opaque_for_cycles + rustc_middle/src/ty/util.rs
    /// OpaqueTypeExpander: expanding the hidden type of an opaque the fn defines must not reach the
    /// opaque again (E0720).
    /// rustc_query_impl handle_cycle_error.rs layout_of: the layout of a coroutine that holds the
    /// future it awaits (a recursive `async fn` call not behind a pointer) needs itself: E0733.
    /// The edges are the awaited operands, which are live across their suspension point.
    fn check_async_recursion(prog: &Program, tcx: &Tcx, owner: DefId, pos: u32) -> Vec<String> {
        fn coroutines_in(prog: &Program, tcx: &Tcx, t: TyId, depth: u32, out: &mut Vec<(u32, u32, DefId)>) {
            if depth > 24 { return; }
            match tcx.tys.kind(t).clone() {
                TyKind::Closure(file, ce, args) if tcx.coroutines.contains(&(file, ce)) => {
                    if let TyKind::FnDef(owner, ..) = tcx.tys.kind(args.parent_args) { if !out.contains(&(file, ce, *owner)) { out.push((file, ce, *owner)); } }
                }
                TyKind::Alias(_, a) => if let crate::types::AliasTyKind::Opaque { def_id } = a.kind {
                    let Ok(hidden) = tcx.type_of(prog, def_id) else { return };
                    let targs: Vec<TyId> = tcx.tys.list(a.args).to_vec();
                    let args = tcx.complete_args(prog, def_id, &targs, a.regions);
                    coroutines_in(prog, tcx, tcx.instantiate_ty(prog, def_id, &args, hidden), depth + 1, out);
                },
                TyKind::Tuple(v) => for &x in tcx.tys.list(v) { coroutines_in(prog, tcx, x, depth + 1, out); },
                TyKind::Array(e, _) => coroutines_in(prog, tcx, e, depth + 1, out),
                TyKind::Adt(d, a, r) => if let Some(adt) = tcx.adts.get(&d) {
                    if prog.def(d).kind != DefKind::Struct && !adt.is_enum { return; }
                    let args = tcx.complete_args(prog, d, tcx.tys.list(a), r);
                    for v in &adt.variants { for f in &v.fields { coroutines_in(prog, tcx, tcx.instantiate_ty(prog, d, &args, f.ty), depth + 1, out); } }
                },
                _ => {}
            }
        }
        let body = tcx.typeck(prog, owner);
        if !body.errors.is_empty() { return Vec::new(); }
        let file = prog.def(owner).file;
        let starts: Vec<u32> = body.closures.iter().copied().filter(|ce| tcx.coroutines.contains(&(file, *ce))).collect();
        for start in starts {
            // depth-first over "awaits a future holding coroutine"
            let mut seen: Vec<(u32, u32)> = Vec::new();
            let mut stack = vec![(file, start, owner)];
            while let Some((f, ce, o)) = stack.pop() {
                let b = tcx.typeck(prog, o);
                if !b.errors.is_empty() { continue; }
                for &(c, t, _) in b.awaits.iter() {
                    if c != ce { continue; }
                    let mut found = Vec::new();
                    coroutines_in(prog, tcx, t, 0, &mut found);
                    for (nf, nce, no) in found {
                        if nf == file && nce == start && no == owner {
                            return vec![prog.record_error(file, pos, "typeck", "E0733: recursion in an async fn requires boxing".to_string())];
                        }
                        if !seen.contains(&(nf, nce)) { seen.push((nf, nce)); stack.push((nf, nce, no)); }
                    }
                }
                let _ = f;
            }
        }
        Vec::new()
    }

    fn check_opaque_cycles(prog: &Program, tcx: &Tcx, owner: DefId, pos: u32) -> Vec<String> {
        use crate::types::fold::{TypeFolder, super_fold_ty};
        use crate::types::{AliasTyKind, Types};
        struct Expander<'a> { prog: &'a Program, tcx: &'a Tcx, seen: Vec<DefId>, primary: DefId, found_recursion: bool, found_any: bool }
        impl TypeFolder for Expander<'_> {
            fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
                if self.found_any { return t; }
                if let TyKind::Alias(_, a) = tys.kind(t) {
                    if let AliasTyKind::Opaque { def_id } = a.kind {
                        if self.seen.contains(&def_id) {
                            self.found_any = true;
                            self.found_recursion = def_id == self.primary;
                            return t;
                        }
                        let Ok(hidden) = self.tcx.type_of(self.prog, def_id) else { return t };
                        let mut targs = Vec::new();
                        for &x in tys.list(a.args) { targs.push(self.fold_ty(tys, x, depth)); }
                        if self.found_any { return t; }
                        let args = self.tcx.complete_args(self.prog, def_id, &targs, a.regions);
                        let concrete = self.tcx.instantiate_ty(self.prog, def_id, &args, hidden);
                        self.seen.push(def_id);
                        let expanded = self.fold_ty(tys, concrete, 0);
                        self.seen.pop();
                        return expanded;
                    }
                }
                if !tys.flags(t).contains(crate::types::TypeFlags::HAS_TY_OPAQUE) { return t; }
                super_fold_ty(tys, t, self, depth)
            }
        }
        let id = tcx.opaque_types_defined_by(prog, owner);
        let mut errors = Vec::new();
        for &o in tcx.defining_opaques(id) {
            if tcx.typeck(prog, owner).errors.len() > 0 { break; }
            let indices = tcx.generic_indices_of(prog, o);
            let tys = &tcx.tys;
            let regions = tys.intern_region_args(&vec![crate::types::RegionId::ERASED; indices.lifetimes.len()]);
            let opaque = tys.intern(TyKind::Alias(crate::consts::IsRigid::No, crate::types::AliasTy { kind: AliasTyKind::Opaque { def_id: o }, args: tys.mk_list(&indices.identity_types_consts(tys)), regions }));
            let mut e = Expander { prog, tcx, seen: Vec::new(), primary: o, found_recursion: false, found_any: false };
            e.fold_ty(tys, opaque, 0);
            if e.found_recursion { errors.push(prog.record_error(prog.def(owner).file, pos, "typeck", "E0720: cannot resolve opaque type".to_string())); }
        }
        errors
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

    /// wfcheck.rs check_where_clauses, the defaults part: a default that mentions no parameter is
    /// well formed, and a predicate whose only parameter has such a default holds with it
    /// (`struct Bounds<T: Copy = String>`).
    fn check_defaults(w: &mut Wfcx<'_, '_>, ast: &ast::Ast, item: &ast::Item) {
        use crate::types::TypeFlags;
        let Some(generics) = ast.kind_generics(&item.kind).copied() else { return };
        let def = w.prog.def(w.def);
        let indices = w.tcx.generic_indices_of(w.prog, w.def);
        let mut args = indices.identity_args(&w.tcx.tys);
        let env = w.tcx.item_env(w.prog, w.def);
        let state = crate::tcx::EvalState::new();
        state.errors.replace(Some(Vec::new()));
        let cx = crate::tcx::EvalCx::new(w.prog, w.tcx, crate::tcx::TypingEnv::fully_monomorphized(w.prog), &state);
        let mut any = false;
        // (a trait's first type slot is `Self`)
        let mut k = usize::from(def.kind == DefKind::Trait);
        for p in ast.list(generics.params) {
            match p.kind {
                ast::GenericParamKind::Lifetime(_) => continue,
                ast::GenericParamKind::Type(_, Some(t)) => {
                    let d = w.tcx.lower_ty(&cx, w.prog, def.file, def.scope, &env, t);
                    let f = w.tcx.tys.flags(d);
                    if !f.intersects(TypeFlags::HAS_PARAM) && !f.intersects(TypeFlags::HAS_ANY_INFER) && !w.tcx.tys.has_error(d) && crate::wfcheck::obligations::arity_ok(w.tcx, w.prog, d) {
                        w.register_wf(d, ty_pos(ast, t).unwrap_or(item.lo));
                        if let Some(&full) = indices.types_consts.get(k) { args[full as usize] = GenericArg::Type(d); any = true; }
                    }
                }
                _ => {}
            }
            k += 1;
        }
        let _ = state.errors.take();
        if !any { return; }
        let own = w.tcx.clauses_of(w.prog, w.def);
        for q in &own {
            let flags = q.flags(&w.tcx.tys);
            // a clause with several parameters or any region is not checked (the other params may be given)
            if flags & TypeFlags::HAS_RE_PARAM.bits() != 0 || flags & TypeFlags::HAS_RE_BOUND.bits() != 0 { continue; }
            // several parameters in one clause: the others may be given, so it is not checked
            if count_params(&w.tcx.tys, q) > 1 { continue; }
            let inst = w.tcx.instantiate_predicate(w.prog, w.def, &args, q);
            if inst.flags(&w.tcx.tys) & (TypeFlags::HAS_PARAM.bits() | TypeFlags::HAS_BOUND_VARS.bits() | TypeFlags::HAS_ANY_INFER.bits()) != 0 { continue; }
            if own.iter().any(|o| *o == inst) { continue; }
            if !matches!(inst.kind, PredicateKind::Clause(ClauseKind::Trait(_))) { continue; }
            w.register(Goal { param_env: w.env.clone(), predicate: inst }, item.lo);
        }
    }

    /// The number of distinct type / const parameters a clause mentions.
    fn count_params(tys: &crate::types::Types, p: &Predicate) -> usize {
        use crate::types::fold::{TypeFolder, super_fold_ty}; use crate::next_solver::fold::fold_predicate;
        struct C(std::collections::HashSet<u32>);
        impl TypeFolder for C {
            fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: u32) -> TyId {
                if let TyKind::Param(i) = tys.kind(t) { self.0.insert(*i); }
                super_fold_ty(tys, t, self, depth)
            }
        }
        let mut c = C(Default::default());
        fold_predicate(tys, p, &mut c);
        c.0.len()
    }
}
