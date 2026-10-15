// ported from N:rustc_hir_typeck/src/method/probe.rs @8d1a764
// Feature cuts: arbitrary_self_types{,_pointers}, pin_ergonomics,
// supertrait_item_shadowing, unstable-library stability filtering. Diagnostic
// suggestion probes, import provenance and private-candidate notes are debloated.
use super::*;
use crate::consts::GenericArg;
use crate::next_solver::{ClausePolarity, ParamEnvId};
use crate::next_solver::fulfill::ObligationCtxt;
use crate::next_solver::canonical::{Canonical, method_response::{self, QueryResponse}};

#[derive(Clone, PartialEq, Eq)]
enum CandidateKind {
    InherentImplCandidate(DefId),
    ObjectCandidate(Predicate),
    WhereClauseCandidate(Predicate),
    TraitCandidate(Option<Predicate>),
}
#[derive(Clone, PartialEq, Eq)]
struct Candidate { item: DefId, kind: CandidateKind }
struct CandidateStep {
    self_ty: TyId,
    response: Canonical<QueryResponse>,
    autoderefs: usize,
    unsize: bool,
    self_ty_is_opaque: bool,
    from_unsafe_deref: bool,
    bad_ty: bool,
}

/// probe.rs:640-794, hir_analysis/autoderef.rs:59-201. Autoderef runs in
/// a separate inference context; each step captures its own canonical response.
/// Later derefs must not constrain an earlier receiver (method-probe-no-guessing).
fn method_autoderef_steps(fcx: &Fcx<'_>, receiver: TyId) -> (Vec<CandidateStep>, Vec<GenericArg>, bool) {
    let (goal, original) = method_response::canonicalize_method_autoderef_goal(&fcx.infcx, fcx.param_env.clone(), receiver);
    let infcx = crate::infer::InferCtxt::new(fcx.tcx, fcx.infcx.typing_mode, fcx.infcx.recursion_limit);
    let (goal, values) = method_response::instantiate_method_autoderef_goal(&infcx, &goal);
    for (key, ty) in goal.predefined_opaques_in_body { infcx.inner.borrow_mut().opaque_types().register(key, crate::infer::opaque_types::ProvisionalHiddenType { ty }); }
    let previous = infcx.inner.borrow_mut().opaque_types().num_entries();
    let mut steps = Vec::new();
    let mut cur = infcx.deeply_resolve_ignoring_regions(goal.self_ty);
    let mut derefs = 0usize;
    let mut from_unsafe_deref = false;
    let mut overflow = false;
    loop {
        let self_ty_is_opaque = match fcx.tcx.tys.kind(cur) { TyKind::Infer(var) => infcx.has_opaques_with_sub_unified_hidden_type(*var), _ => false };
        let response = method_response::make_query_response_ignoring_pending_obligations(&infcx, &values, cur, previous);
        steps.push(CandidateStep { self_ty: cur, response, autoderefs: derefs, unsize: false, self_ty_is_opaque, from_unsafe_deref, bad_ty: matches!(fcx.tcx.tys.kind(cur), TyKind::Error) || (matches!(fcx.tcx.tys.kind(cur), TyKind::Infer(_)) && !self_ty_is_opaque) });
        if let TyKind::Array(element, _) = fcx.tcx.tys.kind(cur) {
            let ty = fcx.tcx.tys.intern(TyKind::Slice(*element));
            let response = method_response::make_query_response_ignoring_pending_obligations(&infcx, &values, ty, previous);
            steps.push(CandidateStep { self_ty: ty, response, autoderefs: derefs, unsize: true, self_ty_is_opaque: false, from_unsafe_deref, bad_ty: false });
            break;
        }
        if derefs > infcx.recursion_limit(fcx.prog) { overflow = true; break; }
        let next = match fcx.tcx.tys.kind(cur) {
            TyKind::Ref(_, ty, _) => Some(*ty),
            TyKind::Ptr(_, ty) => { from_unsafe_deref = true; Some(*ty) }
            TyKind::Error => None,
            TyKind::Infer(_) if !self_ty_is_opaque => None,
            _ => overloaded_deref_ty(&infcx, fcx.prog, &goal.param_env, cur),
        };
        let Some(next) = next else { break; };
        cur = infcx.deeply_resolve_ignoring_regions(next); derefs += 1;
    }
    (steps, original, overflow)
}
/// rustc_hir_analysis/src/autoderef.rs:149-202. Pending normalization goals
/// belong to the deref query; confirmation records the chosen adjustments.
fn overloaded_deref_ty(infcx: &crate::infer::InferCtxt<'_>, prog: &Program, env: &ParamEnvId, ty: TyId) -> Option<TyId> {
    let tcx = infcx.tcx;
    let td = tcx.lang_items.get(crate::lang_items::LangItem::Deref)?;
    let target = tcx.lang_items.get(crate::lang_items::LangItem::DerefTarget)?;
    let goal = Goal { param_env: env.clone(), predicate: crate::next_solver::collect::trait_predicate(td, vec![GenericArg::Type(ty)]) };
    if !infcx.root_goal_may_hold_opaque_types_jank(prog, goal) { return None; }
    let projection = tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: target }, &[GenericArg::Type(ty)]);
    let mut ocx = ObligationCtxt::new(infcx, prog);
    let ty = ocx.normalize_ty(env, projection);
    if !ocx.try_evaluate_obligations().is_empty() { return None; }
    Some(infcx.deeply_resolve_ignoring_regions(ty))
}

pub(super) struct Pick {
    pub item: DefId,
    pub imp: DefId,
    pub self_ty: TyId,
    pub autoderefs: usize,
    pub autoref: u8,
    pub args: Vec<GenericArg>,
}

impl Fcx<'_> {
    fn assemble_candidates_for_bounds(&self, clauses: Vec<Predicate>, name: Sym, kind: u8, candidates: &mut Vec<Candidate>) {
        for clause in clauses {
            let PredicateKind::Clause(ClauseKind::Trait(ref tr)) = clause.kind else { continue; };
            if tr.polarity != ClausePolarity::Positive { continue; }
            for &item in &self.prog.traits[self.prog.def(tr.trait_ref.def_id).sub as usize] {
                if self.prog.def(item).name != name || self.prog.def(item).kind != DefKind::AssocFn { continue; }
                if !self.tcx.fn_sig(self.prog, item).is_some_and(|s| s.self_kind != 0) { continue; }
                let kind = match kind {
                    0 => CandidateKind::WhereClauseCandidate(clause.clone()),
                    1 => CandidateKind::ObjectCandidate(clause.clone()),
                    _ => CandidateKind::TraitCandidate(Some(clause.clone())),
                };
                let candidate = Candidate { item, kind };
                if !candidates.contains(&candidate) { candidates.push(candidate); }
            }
        }
    }

    /// :880-1203: collect candidates before testing receiver adjustments. Bounds on
    /// a parameter and object methods have inherent priority; alias bounds extend
    /// the trait candidates. All predicates retain their binders and lifetime args.
    fn assemble_probe_candidates(&self, steps: &[CandidateStep], name: Sym) -> (Vec<Candidate>, Vec<Candidate>) {
        let mut inherent = Vec::new();
        let mut extension = Vec::new();
        for step in steps {
            let ty = step.self_ty;
            let key = ty_key(&self.tcx.tys, ty);
            if let Some(impls) = self.tcx.inherent.get(&key) {
                for &imp in impls {
                    for &item in &self.prog.impls[self.prog.def(imp).sub as usize].items {
                        if self.prog.def(item).name != name || self.prog.def(item).kind != DefKind::AssocFn { continue; }
                        if !self.tcx.fn_sig(self.prog, item).is_some_and(|s| s.self_kind != 0) || !self.item_visible(item) { continue; }
                        let candidate = Candidate { item, kind: CandidateKind::InherentImplCandidate(imp) };
                        if !inherent.contains(&candidate) { inherent.push(candidate); }
                    }
                }
            }
            match self.tcx.tys.kind(ty) {
                TyKind::Param(_) => {
                    let mut clauses = Vec::new();
                    for p in self.tcx.param_env_by_id(self.param_env).clauses.iter() {
                        if let PredicateKind::Clause(ClauseKind::Trait(tr)) = &p.kind {
                            if tr.trait_ref.args.first() == Some(&GenericArg::Type(ty)) { clauses.push(p.clone()); }
                        }
                    }
                    self.assemble_candidates_for_bounds(clauses, name, 0, &mut inherent);
                }
                TyKind::Dyn(td, args, _, regions, _, bound, _) if *td != NO_DEF => {
                    let mut compact = vec![ty]; compact.extend_from_slice(self.tcx.tys.list(*args));
                    let mut p = crate::next_solver::collect::trait_predicate(*td, self.tcx.complete_args(self.prog, *td, &compact, *regions));
                    p.bound_vars = vec![crate::next_solver::BoundVariableKind::Region(crate::types::BoundRegionKind::Anon); *bound as usize].into();
                    self.assemble_candidates_for_bounds(self.tcx.supertraits(self.prog, p), name, 1, &mut inherent);
                }
                TyKind::Alias(..) => self.assemble_candidates_for_bounds(self.alias_bound_clauses(ty), name, 2, &mut extension),
                _ => {}
            }
        }
        for td in self.traits_in_scope() {
            if self.prog.def(td).kind != DefKind::Trait { continue; }
            for &item in &self.prog.traits[self.prog.def(td).sub as usize] {
                if self.prog.def(item).name != name || self.prog.def(item).kind != DefKind::AssocFn { continue; }
                if !self.tcx.fn_sig(self.prog, item).is_some_and(|s| s.self_kind != 0) { continue; }
                let candidate = Candidate { item, kind: CandidateKind::TraitCandidate(None) };
                if !extension.contains(&candidate) { extension.push(candidate); }
            }
        }
        (inherent, extension)
    }

    /// :2087-2105: #[rustc_skip_during_method_dispatch] applies to the trait,
    /// independent of the spelling of any method in it.
    fn skip_during_method_dispatch(&self, td: DefId, receiver: TyId) -> bool {
        let ed = self.prog.crates[self.prog.mods[self.module as usize].krate as usize].edition;
        let array = matches!(self.tcx.tys.kind(receiver), TyKind::Array(..)) && ed < 2021;
        let boxed_slice = match self.tcx.tys.kind(receiver) {
            TyKind::Adt(d, args, _) if self.tcx.lang_items.get(crate::lang_items::LangItem::OwnedBox) == Some(*d) => ed < 2024 && self.tcx.tys.list(*args).first().is_some_and(|t| matches!(self.tcx.tys.kind(*t), TyKind::Slice(_))),
            _ => false,
        };
        if !array && !boxed_slice { return false; }
        let def = self.prog.def(td);
        let file = &self.prog.files[def.file as usize];
        for attr in file.ast.list(file.ast.item(def.item).attrs) {
            if crate::cfg::tok_str(self.prog.src(def.file), attr.toks.lo as usize) != b"rustc_skip_during_method_dispatch" { continue; }
            for i in attr.toks.lo + 1..attr.toks.hi {
                let token = crate::cfg::tok_str(self.prog.src(def.file), i as usize);
                if (array && token == b"array") || (boxed_slice && token == b"boxed_slice") { return true; }
            }
        }
        false
    }

    /// :2010-2275: transform the receiver, relate it, then evaluate predicates
    /// and leak-check. The caller owns the rollback boundary.
    fn consider_probe(&self, candidate: &Candidate, receiver: TyId, steps: &[CandidateStep], original: &[GenericArg], obligations: &[Goal], probing: bool) -> Option<(TyId, Vec<GenericArg>, Vec<Goal>)> {
        let mut ocx = ObligationCtxt::new(&self.infcx, self.prog);
        ocx.register_obligations(obligations.to_vec());
        if !ocx.try_evaluate_obligations().is_empty() { return None; }
        let (self_ty, parent_args, parent) = match &candidate.kind {
            CandidateKind::InherentImplCandidate(imp) => {
                let args = self.infcx.fresh_args_for_item(self.prog, *imp);
                let self_ty = self.tcx.instantiate_ty(self.prog, *imp, &args, self.tcx.impl_self[imp]);
                (self_ty, args, *imp)
            }
            CandidateKind::ObjectCandidate(p) | CandidateKind::WhereClauseCandidate(p) | CandidateKind::TraitCandidate(Some(p)) => {
                let PredicateKind::Clause(ClauseKind::Trait(tr)) = self.infcx.instantiate_predicate_binder_with_infer(p) else { unreachable!() };
                let GenericArg::Type(self_ty) = tr.trait_ref.args[0] else { unreachable!() };
                if matches!(candidate.kind, CandidateKind::WhereClauseCandidate(_)) {
                    let self_ty = ocx.normalize_ty(&self.param_env, self_ty);
                    if !matches!(self.tcx.tys.kind(self.infcx.shallow_resolve(self_ty)), TyKind::Param(_)) { return None; }
                }
                (self_ty, tr.trait_ref.args, tr.trait_ref.def_id)
            }
            CandidateKind::TraitCandidate(None) => {
                let td = self.prog.def(candidate.item).parent;
                let args = self.infcx.fresh_args_for_item(self.prog, td);
                let GenericArg::Type(self_ty) = args[0] else { unreachable!() };
                (self_ty, args, td)
            }
        };
        if matches!(candidate.kind, CandidateKind::TraitCandidate(_)) && self.skip_during_method_dispatch(parent, receiver) { return None; }
        let mut full = self.infcx.fresh_args_for_item(self.prog, candidate.item);
        full[..parent_args.len()].copy_from_slice(&parent_args);
        // :2604-2640: probe erases only the method's own lifetime arguments
        // and its late-bound regions. Confirmation instantiates fresh regions.
        if probing {
            for arg in &mut full[parent_args.len()..] {
                if matches!(arg, GenericArg::Lifetime(_)) { *arg = GenericArg::Lifetime(crate::types::RegionId::ERASED); }
            }
        }
        let sig = self.tcx.fn_sig(self.prog, candidate.item)?;
        let raw = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&sig.params), sig.ret, sig.header, sig.bound_regions));
        let raw = self.tcx.instantiate_ty(self.prog, candidate.item, &full, raw);
        let raw = if probing {
            let TyKind::FnPtr(ps, ret, header, _) = self.tcx.tys.kind(raw) else { unreachable!() };
            let header = *header;
            let mut tys = self.tcx.tys.list(*ps).to_vec(); tys.push(*ret);
            let payload = self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(&tys)));
            let payload = self.tcx.tys.instantiate_bound_regions(payload, |_| crate::types::RegionId::ERASED);
            let TyKind::Tuple(tys) = self.tcx.tys.kind(payload) else { unreachable!() };
            let tys = self.tcx.tys.list(*tys);
            self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&tys[..tys.len() - 1]), *tys.last().unwrap(), header, 0))
        } else { self.infcx.instantiate_binder_with_infer(raw) };
        let TyKind::FnPtr(ps, ret, _, _) = self.tcx.tys.kind(raw) else { unreachable!() };
        let xform_self_ty = ocx.normalize_ty(&self.param_env, self.tcx.tys.list(*ps)[0]);
        let goals = self.infcx.at(self.prog, self.param_env).sub(receiver, xform_self_ty).ok()?;
        ocx.register_obligations(goals);
        let mut trait_goal = None;
        match &candidate.kind {
            CandidateKind::InherentImplCandidate(imp) => {
                ocx.normalize_ty(&self.param_env, *ret);
                for p in self.tcx.clauses_of(self.prog, *imp) {
                    let p = self.tcx.instantiate_predicate(self.prog, *imp, &parent_args, &p);
                    let p = ocx.normalize_predicate(&self.param_env, &p);
                    ocx.register_obligation(Goal { param_env: self.param_env, predicate: p });
                }
            }
            CandidateKind::TraitCandidate(_) => {
                let goal = Goal { param_env: self.param_env, predicate: crate::next_solver::collect::trait_predicate(parent, parent_args) };
                trait_goal = Some(goal.clone()); ocx.register_obligation(goal);
            }
            _ => {}
        }
        if !ocx.try_evaluate_obligations().is_empty() { return None; }
        // :2280-2368: the candidate must still hold with defining opaques rigid.
        if let Some(goal) = trait_goal { if !self.infcx.root_goal_may_hold_opaque_types_jank(self.prog, goal) { return None; } }
        // :2317-2368: a defining opaque in the autoderef chain must remain
        // an unconstrained type variable after applying the candidate.
        for step in steps {
            if !step.self_ty_is_opaque { continue; }
            let constrained = self.infcx.probe(|_| {
                let Ok((ty, goals)) = method_response::probe_instantiate_query_response(&self.infcx, self.prog, &self.param_env, original, &step.response) else { return false; };
                let mut ocx = ObligationCtxt::new(&self.infcx, self.prog); ocx.register_obligations(goals);
                ocx.try_evaluate_obligations().is_empty() && !matches!(self.tcx.tys.kind(self.infcx.deeply_resolve_ignoring_regions(ty)), TyKind::Infer(_))
            });
            if constrained { return None; }
        }
        Some((self.infcx.deeply_resolve_ignoring_regions(self_ty), full, ocx.engine.pending_obligations().into_iter().map(|o| o.goal).collect()))
    }

    /// :1772-1843,2371-2406: collapse only candidates from the same trait;
    /// different traits or inherent impls are E0034, including ambiguous probes.
    fn consider_candidates(&mut self, receiver: TyId, candidates: &[Candidate], steps: &[CandidateStep], original: &[GenericArg], obligations: &[Goal], lo: u32) -> Result<Option<Candidate>, ()> {
        let mut applicable = Vec::new();
        for candidate in candidates {
            let ok = self.infcx.probe(|snapshot| {
                let universe = self.infcx.universe();
                self.consider_probe(candidate, receiver, steps, original, obligations, true).is_some() && self.infcx.leak_check(universe, Some(snapshot)).is_ok()
            });
            if ok { applicable.push(candidate.clone()); }
        }
        if applicable.len() > 1 {
            if let Some(candidate) = self.collapse_candidates_to_trait_pick(&applicable) { return Ok(Some(candidate)); }
            self.err(lo, "E0034: multiple applicable items in scope".into());
            return Err(());
        }
        Ok(applicable.pop())
    }

    fn collapse_candidates_to_trait_pick(&self, candidates: &[Candidate]) -> Option<Candidate> {
        let td = self.prog.def(candidates[0].item).parent;
        if self.prog.def(td).kind != DefKind::Trait { return None; }
        for candidate in &candidates[1..] { if self.prog.def(candidate.item).parent != td { return None; } }
        Some(Candidate { item: candidates[0].item, kind: CandidateKind::TraitCandidate(None) })
    }

    /// method/confirm.rs:366-406: a typed receiver may still be Arc<dyn Tr>,
    /// Box<dyn Tr> or a reference. Extract its object without recording further
    /// receiver adjustments: these derefs recover Self, not the call operand.
    fn extract_existential_trait_ref(&self, mut ty: TyId, parent: DefId) -> Predicate {
        loop {
            ty = self.infcx.deeply_resolve_ignoring_regions(ty);
            match self.tcx.tys.kind(ty) {
                TyKind::Dyn(td, args, _, regions, _, bound, _) => {
                    let mut compact = vec![ty]; compact.extend_from_slice(self.tcx.tys.list(*args));
                    let mut p = crate::next_solver::collect::trait_predicate(*td, self.tcx.complete_args(self.prog, *td, &compact, *regions));
                    p.bound_vars = vec![crate::next_solver::BoundVariableKind::Region(crate::types::BoundRegionKind::Anon); *bound as usize].into();
                    return self.tcx.supertraits(self.prog, p).into_iter().find(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Trait(tr)) if tr.trait_ref.def_id == parent)).expect("object supertrait");
                }
                TyKind::Ref(_, inner, _) | TyKind::Ptr(_, inner) => ty = *inner,
                _ => ty = overloaded_deref_ty(&self.infcx, self.prog, &self.param_env, ty).expect("object pick never dereferenced to an object"),
            }
        }
    }

    /// :1304-1450,1568-1723: at each autoderef step, try value, shared
    /// autoref, mutable autoref, then *mut-to-*const. Inherent picks precede
    /// extension picks at each receiver adjustment, never across adjustments.
    pub(super) fn pick_all_method(&mut self, expr: ExprId, receiver: TyId, name: Sym, lo: u32) -> Result<Option<Pick>, ()> {
        let receiver = self.normalize_body_type(receiver);
        self.cx.state.impl_read.set(true);
        let (mut steps, original, overflow) = method_autoderef_steps(self, receiver);
        if overflow { self.err(lo, "E0055: reached the recursion limit while auto-dereferencing".into()); return Err(()); }
        // probe.rs:473-563: an unresolved final autoderef type is an error,
        // with the 2015 raw-pointer compatibility lint and diverging-var exception.
        if let Some(step) = steps.last().filter(|s| s.bad_ty) {
            let (ty, _) = method_response::probe_instantiate_query_response(&self.infcx, self.prog, &self.param_env, &original, &step.response).expect("bad autoderef response");
            let edition = self.prog.crates[self.prog.mods[self.module as usize].krate as usize].edition;
            let diverging = if let TyKind::Infer(var) = self.tcx.tys.kind(self.infcx.shallow_resolve(ty)) {
                let mut inner = self.infcx.inner.borrow_mut();
                let root = inner.type_variables().sub_unification_table_root_var(*var);
                self.diverging.iter().any(|t| match self.tcx.tys.kind(*t) {
                    TyKind::Infer(other) => inner.type_variables().sub_unification_table_root_var(*other) == root,
                    _ => false,
                })
            } else { false };
            if step.from_unsafe_deref && edition < 2018 {
                self.tcx.dcx.warn(crate::next_solver::fulfill::ObligationCause { body_id: self.b.def, span: lo }, "type annotations needed for raw pointer method call (future incompatible)".into());
            } else if diverging {
                self.tcx.dcx.warn(crate::next_solver::fulfill::ObligationCause { body_id: self.b.def, span: lo }, "method call on diverging inference variable".into());
                self.eq(ty, self.tcx.tys.never);
            } else {
                if !self.infcx.tainted_by_errors() && !self.tcx.tys.has_error(ty) { self.err(lo, "E0282: type annotations needed for method receiver".into()); }
                self.eq(ty, self.tcx.tys.error);
                return Err(());
            }
        }
        let snapshot = self.start_snapshot();
        // probe.rs:566: all generalized candidate variables and query replay
        // assignments live only for this probe. Confirmation recomputes autoderef.
        let result = (|| {
            for step in &mut steps { step.self_ty = method_response::generalize_response(&self.infcx, &step.response); }
            let (inherent, extension) = self.assemble_probe_candidates(&steps, name);
            for step in &steps {
                if step.from_unsafe_deref || self.tcx.tys.has_error(step.self_ty) { continue; }
                let (self_ty, obligations) = method_response::probe_instantiate_query_response(&self.infcx, self.prog, &self.param_env, &original, &step.response).expect("applicable autoderef response");
                for adjustment in 0..4 {
                    if step.unsize && adjustment == 0 { continue; }
                    let receiver = match adjustment {
                        0 => self_ty,
                        1 | 2 => self.tcx.tys.intern(TyKind::Ref(adjustment == 2, self_ty, crate::types::RegionId::ERASED)),
                        _ => match self.tcx.tys.kind(self_ty) { TyKind::Ptr(true, t) => self.tcx.tys.intern(TyKind::Ptr(false, *t)), _ => continue },
                    };
                    for candidates in [&inherent, &extension] {
                        if let Some(candidate) = self.consider_candidates(receiver, candidates, &steps, &original, &obligations, lo)? {
                            let (autoderefs, autoref) = if adjustment == 0 {
                                match self.tcx.tys.kind(self_ty) {
                                    TyKind::Ref(m, _, _) => (step.autoderefs + 1, if *m { 2 } else { 1 }),
                                    _ => (step.autoderefs, 0),
                                }
                            } else { (step.autoderefs, if adjustment == 3 { 0 } else { adjustment }) };
                            return Ok(Some((candidate, autoderefs, autoref, step.unsize, adjustment == 3)));
                        }
                    }
                }
            }
            Ok(None)
        })();
        self.rollback_to(snapshot);
        let Some((mut candidate, autoderefs, autoref, unsize, const_ptr)) = result? else { return Ok(None); };
        let mut adjusted = self.shallow(receiver);
        for step in 0..autoderefs { adjusted = self.deref_step(expr.0, step, adjusted).expect("confirmed autoderef step"); }
        if unsize { let TyKind::Array(element, _) = self.tcx.tys.kind(adjusted) else { panic!("unsize method receiver"); }; adjusted = self.tcx.tys.intern(TyKind::Slice(*element)); }
        // confirm.rs fresh_receiver_args reconstructs object and trait picks;
        // only a where-clause pick retains the caller's original predicate.
        match candidate.kind {
            CandidateKind::TraitCandidate(_) => candidate.kind = CandidateKind::TraitCandidate(None),
            CandidateKind::ObjectCandidate(_) => {
                let parent = self.prog.def(candidate.item).parent;
                if !self.tcx.dyn_compatibility_violations(self.prog, parent).is_empty() || self.tcx.tys.has_error(adjusted) {
                    self.err(lo, "E0038: trait is not dyn compatible".into()); return Err(());
                }
                candidate.kind = CandidateKind::ObjectCandidate(self.extract_existential_trait_ref(adjusted, parent));
            }
            _ => {}
        }
        if autoref != 0 { adjusted = self.tcx.tys.intern(TyKind::Ref(autoref == 2, adjusted, self.infcx.next_region_var())); }
        if const_ptr { let TyKind::Ptr(_, ty) = self.tcx.tys.kind(adjusted) else { unreachable!() }; adjusted = self.tcx.tys.intern(TyKind::Ptr(false, *ty)); }
        let Some((self_ty, args, goals)) = self.consider_probe(&candidate, adjusted, &[], &[], &[], false) else { panic!("applicable method became inapplicable"); };
        for goal in goals { self.register_goal(goal, lo); }
        let imp = match candidate.kind { CandidateKind::InherentImplCandidate(imp) => imp, _ => NO_DEF };
        Ok(Some(Pick { item: candidate.item, imp, self_ty, autoderefs, autoref, args }))
    }
}
