// ported from N:rustc_hir_typeck/src/closure.rs @8d1a764
// ported from N:rustc_hir_typeck/src/fn_ctxt/_impl.rs @8d1a764
//! Bound signatures survive typeck; bodies use their liberated counterparts.
use super::*;
use crate::types::{BoundVar, LateParamKind, RegionId, RegionKind, RegionScope};

#[derive(Clone, Copy, Debug)]
pub struct ClosureSignatures {
    /// The closure's one signature (rustc bound_sig): the expected signature
    /// when the supplied one merged with it, otherwise the supplied one.
    pub bound: TyId,
    /// FnSig encoded as a FnPtr with a dummy binder (no variables of its own).
    /// The body's inputs and output (rustc liberated_sig): after a merge, the
    /// resolved supplied types (closure.rs:857-865).
    pub liberated: TyId,
    /// S' = E' from the successful, whole supplied/expected merge. Both sides
    /// have dummy binders. S' is the user-provided signature with its own
    /// binder instantiated; E' is `bound` liberated.
    pub annotation_eq: Option<(TyId, TyId)>,
}

impl ClosureSignatures {
    /// `bound` liberated: the signature the closure's universal regions come
    /// from (rustc_borrowck universal_regions.rs compute_inputs_and_output
    /// reads `args.as_closure().sig()`, the bound signature). Without a merge
    /// it is `liberated`; with one it is E', and the annotation is only an
    /// ascription of S' onto it (type_check/input_output.rs
    /// check_signature_annotation), so no region of the expected signature
    /// exists apart from the closure's inputs.
    pub fn liberated_bound(&self) -> TyId {
        self.annotation_eq.map_or(self.liberated, |(_, expected)| expected)
    }
}

impl Fcx<'_> {
    pub(super) fn normalize_signature(&mut self, t: TyId) -> TyId {
        let t = self.infcx.deeply_resolve_ignoring_regions(t);
        self.normalize_body_type(t)
    }

    pub(super) fn init_body_regions(&mut self) {
        for (_, r) in &mut self.env.regions.names {
            if let RegionKind::ReBound(0, var) = *r {
                *r = RegionKind::ReLateParam(RegionScope::Item(self.b.def), LateParamKind::Anon(var.0));
            }
        }
        self.env.regions.enter_body();
    }

    pub(super) fn fresh_region(&self) -> RegionId {
        self.infcx.next_region_var()
    }

    /// Field substitution preserves bound FnPtr types in both the declaration
    /// and the receiver's type arguments. Lifetime arguments use declaration
    /// indices and shift only when escaping a nested binder.
    pub(super) fn instantiate_field_type(&self, ty: TyId, params: crate::types::RegionArgsId, regions: crate::types::RegionArgsId, args: &[TyId]) -> TyId {
        let params = self.tcx.tys.region_args(params);
        let regions = self.tcx.tys.region_args(regions);
        let ty = self.tcx.tys.map_regions(ty, &|r, depth| {
            if let RegionKind::ReEarlyParam(i) = self.tcx.tys.region_kind(r) {
                let r = params.iter().position(|r| self.tcx.tys.region_kind(*r) == RegionKind::ReEarlyParam(i))
                    .and_then(|j| regions.get(j).copied()).unwrap_or(RegionId::ERASED);
                match self.tcx.tys.region_kind(r) {
                    RegionKind::ReBound(d, v) => self.tcx.tys.intern_region(RegionKind::ReBound(d + depth, v)),
                    _ => r,
                }
            } else { r }
        }, 0);
        self.tcx.tys.subst(ty, args)
    }

    pub(super) fn body_signature(&mut self, sig: &FnSigT, args: &[TyId]) -> (Vec<TyId>, TyId) {
        let ps: Vec<_> = sig.params.iter().map(|t| self.tcx.tys.subst_at_depth(*t, args, 1)).collect();
        let ret = self.tcx.tys.subst_at_depth(sig.ret, args, 1);
        let vars: Vec<_> = (0..sig.bound_regions).map(|v| self.tcx.tys.intern_region(
            RegionKind::ReLateParam(RegionScope::Item(self.b.def), LateParamKind::Anon(v)))).collect();
        let (ps, ret) = self.instantiate_signature_parts(&ps, ret, &vars);
        // Most item signatures need no writeback. Keep the existing normalization
        // scope when they do, including its projection-depth limit.
        if ps.iter().chain(std::iter::once(&ret)).any(|t| self.tcx.tys.has_infer(*t) || self.tcx.tys.may_project(*t)) {
            let opened = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&ps), ret, sig.header, 0));
            let opened = self.normalize_signature(opened);
            let TyKind::FnPtr(ps, ret, _, _) = *self.tcx.tys.kind(opened) else { unreachable!() };
            (self.tcx.tys.list(ps).to_vec(), ret)
        } else {
            (ps, ret)
        }
    }

    /// One more type under an item signature's binder (an async fn's declared output),
    /// instantiated and normalized exactly as `body_signature` treats the signature.
    pub(super) fn liberated_signature_output(&mut self, sig: &FnSigT, ty: TyId, args: &[TyId]) -> TyId {
        let ty = self.tcx.tys.subst_at_depth(ty, args, 1);
        let vars: Vec<_> = (0..sig.bound_regions).map(|v| self.tcx.tys.intern_region(
            RegionKind::ReLateParam(RegionScope::Item(self.b.def), LateParamKind::Anon(v)))).collect();
        let (_, ty) = self.instantiate_signature_parts(&[], ty, &vars);
        self.normalize_body_type(ty)
    }

    // rustc anonymize_bound_vars: number used variables in encounter order,
    // dropping unused names. Nested binders retain their own variable indices.
    fn anonymize_signature(&self, sig: TyId) -> TyId {
        let TyKind::FnPtr(ps, ret, c, _) = self.tcx.tys.kind(sig) else { unreachable!() };
        let vars = std::cell::RefCell::new(HashMap::default());
        let rename = |r, depth| match self.tcx.tys.region_kind(r) {
            RegionKind::ReBound(d, var) if d == depth => {
                let mut vars = vars.borrow_mut();
                let next = vars.len() as u32;
                let new = *vars.entry(var).or_insert(next);
                self.tcx.tys.intern_region(RegionKind::ReBound(d, BoundVar(new)))
            }
            _ => r,
        };
        let ps = self.tcx.tys.mk_list(&self.tcx.tys.list(*ps).iter().map(|t| self.tcx.tys.map_regions(*t, &rename, 0)).collect::<Vec<_>>());
        let ret = self.tcx.tys.map_regions(*ret, &rename, 0);
        let count = vars.borrow().len() as u32;
        self.tcx.tys.intern(TyKind::FnPtr(ps, ret, *c, count))
    }

    /// Instantiate a call's item signature and its direct Fn-family bounds
    /// together. Early parameters belong to this call; the Fn bound's own late
    /// parameters stay bound. Register the projections before checking arguments
    /// so deduction also works through blocks, borrows, and pending Self vars.
    pub(super) fn instantiate_call_signature(&mut self, d: DefId, args: &[crate::consts::GenericArg], sig: &FnSigT) -> (Vec<TyId>, TyId) {
        let poly = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&sig.params), sig.ret, sig.header, sig.bound_regions));
        let poly = self.tcx.instantiate_ty(self.prog, d, args, poly);
        let opened = self.infcx.instantiate_binder_with_infer(poly);
        let TyKind::FnPtr(ps, ret, _, _) = *self.tcx.tys.kind(opened) else { unreachable!() };
        (self.tcx.tys.list(ps).to_vec(), ret)
    }

    /// Remove exactly one binder. Nested binders protect their own variables;
    /// escaping variables shift down, as in rustc's BoundVarReplacer.
    pub(super) fn instantiate_signature(&self, sig: TyId, regions: &[RegionId]) -> TyId {
        let TyKind::FnPtr(ps, ret, c, _) = self.tcx.tys.kind(sig) else { unreachable!() };
        let (ps, ret) = self.instantiate_signature_parts(self.tcx.tys.list(*ps), *ret, regions);
        self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&ps), ret, *c, 0))
    }

    /// Item calls/bodies only need the payload; closure typeck results also
    /// retain the FnPtr constructed by `instantiate_signature` above.
    pub(super) fn instantiate_signature_parts(&self, ps: &[TyId], ret: TyId, regions: &[RegionId]) -> (Vec<TyId>, TyId) {
        let replace = |r, depth| match self.tcx.tys.region_kind(r) {
            RegionKind::ReBound(d, var) if d == depth => regions[var.0 as usize],
            RegionKind::ReBound(d, var) if d > depth => self.tcx.tys.intern_region(RegionKind::ReBound(d - 1, var)),
            _ => r,
        };
        let ty = |t| self.tcx.tys.map_regions(t, &replace, 0);
        (ps.iter().map(|t| ty(*t)).collect(), ty(ret))
    }

    fn closure_sigs(&mut self, e: ExprId, bound: TyId) -> ClosureSignatures {
        let TyKind::FnPtr(_, _, _, n) = self.tcx.tys.kind(bound) else { unreachable!() };
        let scope = RegionScope::Closure { owner: self.b.def, expr: e.0 };
        let regions: Vec<_> = (0..*n).map(|v| self.tcx.tys.intern_region(
            RegionKind::ReLateParam(scope, LateParamKind::Anon(v)))).collect();
        let liberated = self.instantiate_signature(bound, &regions);
        let liberated = self.normalize_signature(liberated);
        ClosureSignatures { bound, liberated, annotation_eq: None }
    }

    /// rustc closure.rs:876: annotations share one binder; absent types create
    /// inference variables. Elided references in separate inputs are distinct.
    fn supplied_closure_signature(&mut self, e: ExprId, params: &[ClosureParam], ret: Option<crate::ast::TyId>) -> TyId {
        let outer = self.env.regions.clone();
        self.env.regions = outer.with_binder(Some(crate::tcx::resolve_bound_vars::BinderId::Closure(e.0)));
        let mut ps = Vec::new();
        for p in params {
            ps.push(match p.ty { Some(t) => self.lower_ty(t), None => self.fresh(0) });
        }
        let bound = self.env.regions.bound_count();
        // rustc_resolve/late.rs:1186-1196: closure output elision is Infer,
        // not the function signature's single-input elision rule.
        self.env.regions.enter_body();
        let ret = match ret { Some(t) => self.lower_ty(t), None => self.fresh(0) };
        self.env.regions = outer;
        let sig = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&ps), ret, FnHeader::RUST, bound));
        // rustc_hir_typeck/src/closure.rs:944: retain the supplied signature before it is
        // normalized and before the expected signature constrains it.
        if self.keep_user_types {
            let canonical = crate::next_solver::canonical::user_type::canonicalize_user_signature(&self.infcx, sig);
            self.b.user_provided_sigs.insert(e.0, std::sync::Arc::new(canonical));
        }
        self.normalize_signature(sig)
    }

    /// rustc closure.rs:300-520: a function pointer or Fn-family Output
    /// projection determines the expected signature from ordinary callable clauses.
    /// Declaration region contracts retain binders alongside selection clauses.
    pub(super) fn deduce_closure_signature(&mut self, expected: TyId, is_async: bool) -> (Option<TyId>, Option<crate::upvar::ClosureKind>) {
        let expected = self.infcx.shallow_resolve(expected);
        match self.tcx.tys.kind(expected).clone() {
            TyKind::FnPtr(..) if !is_async => (Some(expected), Some(crate::upvar::ClosureKind::Fn)),
            TyKind::Dyn(principal, ..) => {
                // closure.rs:305-316: only projection bounds supply the signature;
                // the principal itself, not its supertraits, supplies the kind.
                // (rustc lowers the projections the supertraits fix, `trait Foo: Fn(Bar)`'s
                // `Output = ()`, into the object type itself: dyn_trait.rs:225-285)
                let mut clauses = self.tcx.object_predicates(self.prog, expected);
                if let Some(principal) = clauses.first().cloned() {
                    clauses.extend(self.tcx.supertraits(self.prog, principal));
                }
                clauses.retain(|p| matches!(p.kind, PredicateKind::Clause(ClauseKind::Projection(_))));
                let (sig, _) = self.deduce_closure_signature_from_predicates(expected, is_async, clauses);
                let kind = match self.tcx.lang_items.by_def.get(&principal) {
                    Some(crate::lang_items::LangItem::Fn) => Some(crate::upvar::ClosureKind::Fn),
                    Some(crate::lang_items::LangItem::FnMut) => Some(crate::upvar::ClosureKind::FnMut),
                    Some(crate::lang_items::LangItem::FnOnce) => Some(crate::upvar::ClosureKind::FnOnce),
                    _ => None,
                };
                (sig, kind)
            }
            TyKind::Infer(v) => {
                let clauses = self.obligations_for_self_ty(v);
                let (sig, kind) = self.deduce_closure_signature_from_predicates(expected, is_async, clauses);
                if sig.is_some() { return (sig, kind); }
                // N2 owns inspect_obligations.rs:108-140's nested proof visitor.
                // Until that port, a defining opaque's item bounds provide its
                // nested obligations; retain a kind even without a signature.
                let values = self.infcx.inner.borrow_mut().opaque_types().iter_lookup_table().to_vec();
                let Some((key, _)) = values.iter().find(|(_, h)| self.infcx.shallow_resolve(h.ty) == expected) else { return (None, kind) };
                let opaque = self.tcx.alias_ty_with_args(crate::types::AliasTyKind::Opaque { def_id: key.def_id }, &key.args);
                let clauses = self.alias_bound_clauses(opaque);
                let (sig, opaque_kind) = self.deduce_closure_signature_from_predicates(opaque, is_async, clauses);
                (sig, kind.or(opaque_kind))
            }
            TyKind::Alias(_, crate::types::AliasTy { kind: crate::types::AliasTyKind::Opaque { .. }, .. }) => {
                let clauses = self.alias_bound_clauses(expected);
                self.deduce_closure_signature_from_predicates(expected, is_async, clauses)
            }
            _ => (None, None),
        }
    }

    /// fn_ctxt/inspect_obligations.rs:76-100 (obligations_for_self_ty_next): the pending obligations
    /// and the goals nested in them (a unique applicable candidate's, down to depth 5) whose self
    /// type is the unresolved variable `vid`.
    fn obligations_for_self_ty(&mut self, vid: u32) -> Vec<Predicate> {
        let mut out = Vec::new();
        for obligation in self.fulfillment_cx.pending_obligations() {
            // a goal that mentions no variable of `vid`'s class cannot be stalled on it (nor can
            // anything nested in it; skipping the proof-tree walk keeps this cheap)
            if !self.predicate_mentions_var_class(&obligation.goal.predicate, vid) { continue; }
            self.nested_obligations_for_self_ty(obligation.goal.clone(), 0, vid, &mut out);
        }
        let flags = crate::types::TypeFlags::HAS_PLACEHOLDER.bits();
        out.into_iter().filter_map(|p| {
            let mut resolver = crate::infer::resolve::DeepResolverIgnoringRegions::new(&self.infcx);
            let p = crate::next_solver::fold::fold_predicate(&self.tcx.tys, &p, &mut resolver);
            (p.flags(&self.tcx.tys) & flags == 0).then_some(p)
        }).collect()
    }

    fn predicate_mentions_var_class(&self, predicate: &Predicate, vid: u32) -> bool {
        let mut resolver = crate::infer::resolve::DeepResolverIgnoringRegions::new(&self.infcx);
        let p = crate::next_solver::fold::fold_predicate(&self.tcx.tys, predicate, &mut resolver);
        if p.flags(&self.tcx.tys) & crate::types::TypeFlags::HAS_TY_INFER.bits() == 0 { return false; }
        struct Finder<'a> { fcx: &'a Fcx<'a>, vid: u32, found: bool }
        impl crate::types::fold::TypeFolder for Finder<'_> {
            fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: u32) -> TyId {
                if self.found { return t; }
                if let TyKind::Infer(v) = tys.kind(t) {
                    let mut inner = self.fcx.infcx.inner.borrow_mut();
                    let mut table = inner.type_variables();
                    if table.root_var(*v) == table.root_var(self.vid) { self.found = true; }
                    return t;
                }
                crate::types::fold::super_fold_ty(tys, t, self, depth)
            }
        }
        let mut f = Finder { fcx: self, vid, found: false };
        crate::next_solver::fold::fold_predicate(&self.tcx.tys, &p, &mut f);
        f.found
    }

    /// inspect_obligations.rs:155-200 NestedObligationsForSelfTy::visit_goal.
    fn nested_obligations_for_self_ty(&self, goal: Goal, depth: usize, vid: u32, out: &mut Vec<Predicate>) {
        const MAX_DEPTH: usize = 5;
        if depth >= MAX_DEPTH { return; }
        let root = self.infcx.evaluate_root_goal_for_proof_tree(self.prog, goal);
        let inspect = crate::next_solver::inspect::InspectGoal::new(&self.infcx, self.prog, root);
        if inspect.result.is_ok_and(|c| c.is_yes()) { return; }
        if self.predicate_has_self_ty(&inspect.goal.predicate, vid) { out.push(inspect.goal.predicate.clone()); }
        // a unique way to prove the goal: look at what it needs (`impl<F: FnOnce(u32)> Trait<F> for ()`
        // and `(): Trait<?0>` give the nested `?0: FnOnce(u32)`)
        if let Some(candidate) = inspect.unique_applicable_candidate() {
            for nested in candidate.instantiate_nested_goals() {
                self.nested_obligations_for_self_ty(nested, depth + 1, vid, out);
            }
        }
    }

    /// inspect_obligations.rs:35-66 (predicate_has_self_ty, type_matches_expected_vid): a trait or
    /// trait-projection predicate whose self type is unified with `vid`.
    fn predicate_has_self_ty(&self, predicate: &Predicate, vid: u32) -> bool {
        let self_ty = match &predicate.kind {
            PredicateKind::Clause(ClauseKind::Trait(c)) => match c.trait_ref.args.first() { Some(crate::consts::GenericArg::Type(t)) => *t, _ => return false },
            PredicateKind::Clause(ClauseKind::Projection(p)) => {
                let AliasTerm::Ty(alias) = p.projection_term else { return false };
                match self.tcx.tys.kind(alias) {
                    TyKind::Alias(_, crate::types::AliasTy { kind: crate::types::AliasTyKind::Projection { .. }, args, .. }) => match self.tcx.tys.list(*args).first() { Some(t) => *t, None => return false },
                    _ => return false,
                }
            }
            _ => return false,
        };
        let expected = self.tcx.tys.intern(TyKind::Infer(vid));
        self.same_self_ty(self_ty, expected)
    }

    /// Equal types, or type variables with the same unification root.
    fn same_self_ty(&self, a: TyId, b: TyId) -> bool {
        let (a, b) = (self.infcx.shallow_resolve(a), self.infcx.shallow_resolve(b));
        if a == b { return true; }
        match (self.tcx.tys.kind(a), self.tcx.tys.kind(b)) {
            (TyKind::Infer(x), TyKind::Infer(y)) => { let (x, y) = (*x, *y); let mut inner = self.infcx.inner.borrow_mut(); let mut table = inner.type_variables(); table.root_var(x) == table.root_var(y) }
            _ => false,
        }
    }

    /// closure.rs:335-444 and deduce_sig_from_projection :446-475: elaborated self
    /// clauses; an FnOnce::Output (async closure: AsyncFnOnce::Output, or FnOnce::Output
    /// of a two-part bound) projection gives the whole signature unless it mentions the
    /// expected type, and the most permissive Fn-family (async closure: AsyncFn-family
    /// first) trait gives the kind. An async closure is a closure returning its
    /// coroutine here (TyKind::CoroutineClosure is M7's), so its signature's output is
    /// that future: from AsyncFnOnce::Output it is left to inference, the projection's
    /// obligation relates the future's Output through the async callable candidate;
    /// from a two-part `FnOnce(..) -> Fut` bound it is `Fut` itself.
    fn deduce_closure_signature_from_predicates(&mut self, expected_ty: TyId, is_async: bool, clauses: Vec<Predicate>) -> (Option<TyId>, Option<crate::upvar::ClosureKind>) {
        use crate::upvar::ClosureKind;
        let mut expected_sig = None;
        let mut expected_kind = None;
        let mut elaborated = Vec::new();
        for clause in clauses { for p in self.tcx.supertraits(self.prog, clause) { if !elaborated.contains(&p) { elaborated.push(p); } } }
        for clause in elaborated {
            let bound_regions = clause.bound_vars.len() as u32;
            let (trait_def, self_ty) = match &clause.kind {
                PredicateKind::Clause(ClauseKind::Projection(p)) => {
                    let AliasTerm::Ty(alias) = p.projection_term else { continue };
                    let TyKind::Alias(_, alias) = self.tcx.tys.kind(alias).clone() else { continue };
                    let trait_def = self.prog.def(alias.kind.def_id()).parent;
                    let Some(&self_ty) = self.tcx.tys.list(alias.args).first() else { continue };
                    if !self.same_self_ty(self_ty, expected_ty) { continue; }
                    let projection = self.tcx.lang_items.by_def.get(&alias.kind.def_id()).copied();
                    let deduces = match projection {
                        Some(crate::lang_items::LangItem::FnOnceOutput) => true,
                        Some(crate::lang_items::LangItem::AsyncFnOnceOutput) => is_async,
                        _ => false,
                    };
                    if expected_sig.is_none() && deduces {
                        let term = match (projection, p.term) {
                            (Some(crate::lang_items::LangItem::AsyncFnOnceOutput), _) => Term::Ty(self.fresh(0)),
                            (_, term) => term,
                        };
                        if let (Some(&inputs), Term::Ty(ret)) = (self.tcx.tys.list(alias.args).get(1), term) {
                            if let TyKind::Tuple(ps) = self.tcx.tys.kind(inputs).clone() {
                                let sig = self.tcx.tys.intern(TyKind::FnPtr(ps, ret, FnHeader::RUST, bound_regions));
                                let sig = self.normalize_signature(sig);
                                let mut mentions = MentionsTy { expected_ty, found: false };
                                crate::types::fold::TypeFolder::fold_ty(&mut mentions, &self.tcx.tys, sig, 0);
                                if !mentions.found {
                                    expected_sig = Some(sig);
                                    if let (Some(crate::lang_items::LangItem::AsyncFnOnceOutput), Term::Ty(inner_ret)) = (projection, p.term) {
                                        // the coroutine's return type, bound under the same binder
                                        let inner = self.tcx.tys.intern(TyKind::FnPtr(ps, inner_ret, FnHeader::RUST, bound_regions));
                                        self.async_expected_inner = Some(self.normalize_signature(inner));
                                    }
                                }
                            }
                        }
                    }
                    (trait_def, self_ty)
                }
                PredicateKind::Clause(ClauseKind::Trait(c)) => {
                    let Some(crate::consts::GenericArg::Type(self_ty)) = c.trait_ref.args.first().copied() else { continue };
                    (c.trait_ref.def_id, self_ty)
                }
                _ => continue,
            };
            if !self.same_self_ty(self_ty, expected_ty) { continue; }
            let found_kind = match self.tcx.lang_items.by_def.get(&trait_def) {
                Some(crate::lang_items::LangItem::Fn) => ClosureKind::Fn,
                Some(crate::lang_items::LangItem::FnMut) => ClosureKind::FnMut,
                Some(crate::lang_items::LangItem::FnOnce) => ClosureKind::FnOnce,
                Some(crate::lang_items::LangItem::AsyncFn) if is_async => ClosureKind::Fn,
                Some(crate::lang_items::LangItem::AsyncFnMut) if is_async => ClosureKind::FnMut,
                Some(crate::lang_items::LangItem::AsyncFnOnce) if is_async => ClosureKind::FnOnce,
                _ => continue,
            };
            match (expected_kind, found_kind) {
                (None, _) => expected_kind = Some(found_kind),
                (Some(ClosureKind::FnMut), ClosureKind::Fn) => expected_kind = Some(ClosureKind::Fn),
                (Some(ClosureKind::FnOnce), ClosureKind::Fn | ClosureKind::FnMut) => expected_kind = Some(found_kind),
                _ => {}
            }
        }
        (expected_sig, expected_kind)
    }

    pub(super) fn closure_signatures(&mut self, e: ExprId, params: &[ClosureParam], ret: Option<crate::ast::TyId>, expected: Option<TyId>) -> ClosureSignatures {
        let is_async = matches!(self.ast().expr(e).kind, ExprKind::Closure(row) if self.ast().row(row).is_async);
        self.async_expected_inner = None;
        let expected = expected.and_then(|t| {
            let (sig, kind) = self.deduce_closure_signature(t, is_async);
            // The same direct Fn-family bound supplies the signature and the
            // kind that can force a move async closure's coroutine captures.
            if let Some(kind) = kind { self.expected_closure_kinds.insert(e.0, kind); }
            sig.map(|sig| self.normalize_signature(sig))
        });
        let supplied = self.supplied_closure_signature(e, params, ret);
        let Some(expected) = expected else { return self.closure_sigs(e, supplied) };
        let TyKind::FnPtr(ps, r, _, n) = self.tcx.tys.kind(expected).clone() else { unreachable!() };
        if self.tcx.tys.list(ps).len() != params.len() {
            self.err(self.ast().expr(e).lo, format!("E0593: closure is expected to take {} arguments, but it takes {} arguments", self.tcx.tys.list(ps).len(), params.len()));
            return self.closure_sigs(e, supplied);
        }
        // This signature uses the safe Rust header of a reified closure pointer
        // (rustc_hir_typeck/src/coercion.rs:1083).
        // Bound regions already use anonymous variable indices.
        // Error recovery must not make the closure body's return type rigid.
        let r = if r == self.tcx.tys.error { self.fresh(0) } else { r };
        let expected = self.tcx.tys.intern(TyKind::FnPtr(ps, r, FnHeader::RUST, n));
        let expected = self.anonymize_signature(expected);
        let mut sigs = self.closure_sigs(e, expected);
        if let Some(inner) = self.async_expected_inner.take() {
            // rustc coroutine-closure `return_ty`: the async body's return type is the
            // deduced `AsyncFnOnce::Output`, with the closure's own late-bound regions.
            let inner = self.anonymize_signature(inner);
            let (TyKind::FnPtr(_, _, _, n_inner), TyKind::FnPtr(_, _, _, n)) = (self.tcx.tys.kind(inner).clone(), self.tcx.tys.kind(expected).clone()) else { unreachable!() };
            if n_inner == n {
                let scope = RegionScope::Closure { owner: self.b.def, expr: e.0 };
                let regions: Vec<_> = (0..n).map(|v| self.tcx.tys.intern_region(RegionKind::ReLateParam(scope, LateParamKind::Anon(v)))).collect();
                let TyKind::FnPtr(ps, ret, _, _) = self.tcx.tys.kind(inner).clone() else { unreachable!() };
                let (_, ret) = self.instantiate_signature_parts(self.tcx.tys.list(ps), ret, &regions);
                let ret = self.normalize_signature(ret);
                self.async_body_expected.insert(e.0, ret);
            }
        }
        let TyKind::FnPtr(_, _, _, n) = self.tcx.tys.kind(supplied) else { unreachable!() };
        let vars: Vec<_> = (0..*n).map(|_| self.fresh_region()).collect();
        let instantiated = self.instantiate_signature(supplied, &vars);
        // rustc closure.rs:822 commit_if_ok: rollback the *whole* merge if
        // either an input or the output fails, then use the supplied signature.
        let mark = self.start_snapshot();
        if self.eq(sigs.liberated, instantiated) {
            sigs.annotation_eq = Some((instantiated, sigs.liberated));
            // rustc closure.rs:856-865 checks the body using the resolved
            // supplied inputs and supplied output, after equating S' and E'.
            self.commit_from(mark);
            sigs.liberated = self.infcx.deeply_resolve_ignoring_regions(instantiated);
            sigs
        } else {
            self.rollback_to(mark);
            self.closure_sigs(e, supplied)
        }
    }

    /// The signature of an async closure whose body return type came from the deduced
    /// `AsyncFnOnce::Output`: its coroutine mentions the closure's late regions, which
    /// are the bound signature's own binder (rustc coroutine-closure signature).
    pub(super) fn async_closure_bound_signature(&mut self, e: ExprId) -> TyId {
        let sig = self.b.closure_sigs[&e.0].bound;
        if !self.async_body_expected.contains_key(&e.0) { return sig; }
        let scope = RegionScope::Closure { owner: self.b.def, expr: e.0 };
        let TyKind::FnPtr(ps, ret, c, n) = self.tcx.tys.kind(sig).clone() else { return sig; };
        let ret = self.infcx.shallow_resolve(ret);
        let ret = self.tcx.tys.map_regions(ret, &|r, depth| match self.tcx.tys.region_kind(r) {
            RegionKind::ReLateParam(s, LateParamKind::Anon(v)) if s == scope => self.tcx.tys.intern_region(RegionKind::ReBound(depth, BoundVar(v))),
            _ => r,
        }, 0);
        let bound = self.tcx.tys.intern(TyKind::FnPtr(ps, ret, c, n));
        self.b.closure_sigs.get_mut(&e.0).unwrap().bound = bound;
        bound
    }

    pub(super) fn finish_closure_signatures(&mut self) {
        for (e, mut sig) in std::mem::take(&mut self.b.closure_sigs) {
            sig.bound = self.normalize_signature(sig.bound);
            sig.liberated = self.normalize_signature(sig.liberated);
            // Type inference resolves an unannotated output's type by copying
            // the body's result type. Its free regions must remain inferred in
            // the parent, not become the closure's input-only late parameters.
            // A declared/expected bound output is still ReBound in `bound`.
            if sig.annotation_eq.is_none() && !self.async_body_expected.contains_key(&e) {
                let scope = RegionScope::Closure { owner:self.b.def, expr:e };
                let replacements = std::cell::RefCell::new(HashMap::default());
                if let TyKind::FnPtr(_,ret,_,_) = self.tcx.tys.kind(sig.bound) {
                    self.tcx.tys.map_regions(*ret,&|r,_| {
                        if matches!(self.tcx.tys.region_kind(r),RegionKind::ReLateParam(s,_) if s==scope) {
                            replacements.borrow_mut().entry(r).or_insert_with(||self.fresh_region());
                        } r
                    },0);
                }
                let replacements = replacements.into_inner();
                let replace_output = |t| {
                    let TyKind::FnPtr(ps,ret,c,n) = self.tcx.tys.kind(t) else { return t; };
                    let ret=self.tcx.tys.map_regions(*ret,&|r,_|replacements.get(&r).copied().unwrap_or(r),0);
                    self.tcx.tys.intern(TyKind::FnPtr(ps.clone(),ret,*c,*n))
                };
                sig.bound=replace_output(sig.bound);
                sig.liberated=replace_output(sig.liberated);
            }
            sig.annotation_eq = sig.annotation_eq.map(|(a, b)| (self.normalize_signature(a), self.normalize_signature(b)));
            self.b.closure_sigs.insert(e, sig);
        }
        for (e, t) in std::mem::take(&mut self.b.inline_const_types) {
            let t = self.normalize_signature(t);
            self.b.inline_const_types.insert(e, t);
        }
    }
}

/// closure.rs:375-389: a deduced signature must not mention the expected type.
struct MentionsTy { expected_ty: TyId, found: bool }
impl crate::types::fold::TypeFolder for MentionsTy {
    fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: u32) -> TyId {
        if t == self.expected_ty { self.found = true; return t; }
        crate::types::fold::super_fold_ty(tys, t, self, depth)
    }
}
