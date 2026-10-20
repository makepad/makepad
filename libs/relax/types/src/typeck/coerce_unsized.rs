// ported from N:rustc_hir_typeck/src/coercion.rs @8d1a764 (coerce_unsized, CoerceVisitor)
//! Unsizing coercions (`&[T; N]` to `&[T]`, `Box<T>` to `Box<dyn Trait>`, `&S<[T; N]>` to
//! `&S<[T]>`): `Source: CoerceUnsized<Target>` is proved by the solver and its proof tree is
//! walked to decide whether the coercion happens (rustc `CoerceVisitor`).
use super::*;
use crate::next_solver::inspect::InspectGoal;
use crate::next_solver::Certainty;
use crate::lang_items::LangItem;

/// What an unsizing coercion turns the pointee tails into.
pub(super) enum UnsizeKind {
    /// source tail -> trait object tail
    ToDyn(TyId, TyId),
    /// trait object -> trait object (upcast, dropping auto traits)
    Upcast(TyId, TyId),
    /// the source tail is not known yet: classified once it is
    Deferred,
    /// array -> slice, struct tails thereof
    Unsize,
}

/// rustc_trait_selection/src/solve/inspect/analyse.rs `ProofTreeVisitor::config`: the default `InspectConfig`.
const INSPECT_MAX_DEPTH: usize = 10;

struct CoerceVisitor { errored: bool }

impl Fcx<'_> {
    /// rustc coerce_unsized (next solver): `None` when the coercion does not apply (nothing is
    /// changed then). `Some` leaves the `CoerceUnsized` obligation pending and the
    /// inference constraints of the proof in place.
    pub(super) fn coerce_unsized(&mut self, source: TyId, target: TyId) -> Option<UnsizeKind> {
        let (source, target) = (self.shallow(source), self.shallow(target));
        let (ks, kt) = (self.tcx.tys.kind(source).clone(), self.tcx.tys.kind(target).clone());
        // We don't apply any coercions in case either the source or target aren't sufficiently
        // well known but tend to instead just equate them both.
        if matches!(ks, TyKind::Infer(_)) || matches!(kt, TyKind::Infer(_)) { return None; }
        // Targets that are never the right-hand side of `L: CoerceUnsized<R>` (rustc's fast
        // path): only pointers, references and ADTs can be.
        if !matches!(kt, TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::Adt(..)) { return None; }
        if !self.could_unsize(&ks, &kt) { return None; }
        let unsize = self.tcx.lang_items.get(LangItem::Unsize)?;
        let coerce_unsized = self.tcx.lang_items.get(LangItem::CoerceUnsized)?;
        // Reborrows are handled before selecting `Source: CoerceUnsized<Target>`.
        match (&ks, &kt) {
            (TyKind::Ref(false, ..), TyKind::Ref(true, ..) | TyKind::Ptr(true, _)) => return None,
            _ => {}
        }
        let snapshot = self.start_snapshot();
        // The `CoerceUnsized` target is a fresh variable related to the expected type.
        let coerce_target = self.fresh(0);
        if !self.eq(coerce_target, target) { self.rollback_to(snapshot); return None; }
        let goal = self.trait_goal(coerce_unsized, source, &[coerce_target]);
        let mut visitor = CoerceVisitor { errored: false };
        if self.visit_coerce_goal(&mut visitor, goal.clone(), 0, unsize, coerce_unsized) {
            self.rollback_to(snapshot);
            return None;
        }
        let lo = self.last_lo;
        self.commit_from(snapshot);
        self.register_goal(goal, lo);
        Some(self.unsize_kind(source, target))
    }

    /// A cheap necessary condition (the solver decides): some pointee pair of the two types
    /// differs and could be related by `Unsize`.
    fn could_unsize(&mut self, ks: &TyKind, kt: &TyKind) -> bool {
        let (a, d) = match (ks, kt) {
            (TyKind::Ref(..) | TyKind::Ptr(..), TyKind::Ref(_, d, _) | TyKind::Ptr(_, d)) => {
                let a = match ks { TyKind::Ref(_, a, _) | TyKind::Ptr(_, a) => *a, _ => unreachable!() };
                (a, *d)
            }
            (TyKind::Adt(d1, xs, _), TyKind::Adt(d2, ys, _)) if d1 == d2 && self.tcx.tys.list(*xs).len() == self.tcx.tys.list(*ys).len() => {
                let (xs, ys) = (self.tcx.tys.list(*xs).to_vec(), self.tcx.tys.list(*ys).to_vec());
                return (0..xs.len()).any(|i| { let (x, y) = (self.shallow(xs[i]), self.shallow(ys[i])); x != y && self.pointee_pair_could_unsize(x, y) });
            }
            _ => return false,
        };
        let (a, d) = (self.shallow(a), self.shallow(d));
        a != d && self.pointee_pair_could_unsize(a, d)
    }

    fn pointee_pair_could_unsize(&mut self, a: TyId, d: TyId) -> bool {
        let (a, d) = (self.shallow(a), self.shallow(d));
        match self.tcx.tys.kind(d).clone() {
            TyKind::Dyn(..) => true,
            TyKind::Slice(_) => matches!(self.tcx.tys.kind(a), TyKind::Array(..) | TyKind::Adt(..) | TyKind::Infer(_)),
            TyKind::Adt(d2, ys, _) => match self.tcx.tys.kind(a).clone() {
                // a struct unsizes through its last field, whatever that field's type is
                // (`Foo<i32, Count<N>>` to `Foo<u32, ()>` when the field type does)
                TyKind::Adt(d1, xs, _) if d1 == d2 && self.prog.def(d1).kind == DefKind::Struct && self.tcx.tys.list(xs).len() == self.tcx.tys.list(ys).len() => {
                    let (xs, ys) = (self.tcx.tys.list(xs).to_vec(), self.tcx.tys.list(ys).to_vec());
                    (0..xs.len()).any(|i| self.shallow(xs[i]) != self.shallow(ys[i]))
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Recursively visit goals to decide whether an unsizing is possible. `true`: it is not.
    fn visit_coerce_goal(&mut self, visitor: &mut CoerceVisitor, goal: Goal, depth: usize, unsize: DefId, coerce_unsized: DefId) -> bool {
        let PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(pred)) = &goal.predicate.kind else { return false };
        // only `Unsize` and `CoerceUnsized` goals are looked at
        if pred.trait_ref.def_id != unsize && pred.trait_ref.def_id != coerce_unsized { return false; }
        if depth >= INSPECT_MAX_DEPTH {
            // on_recursion_limit: do not commit an unsizing while looking for the error source
            return visitor.errored;
        }
        let is_unsize = pred.trait_ref.def_id == unsize;
        let args = pred.trait_ref.args.clone();
        let root = self.infcx.evaluate_root_goal_for_proof_tree(self.prog, goal);
        let certainty = root.result.as_ref().map(|r| r.value.certainty).map_err(|_| ());
        if let Ok(Certainty::Maybe(_)) = certainty {
            // `$0: Unsize<dyn Trait>` where `$0: Sized`: unsizing definitely has to happen.
            if is_unsize {
                if let (Some(crate::consts::GenericArg::Type(self_ty)), Some(crate::consts::GenericArg::Type(target))) = (args.first(), args.get(1)) {
                    let (self_ty, target) = (self.shallow(*self_ty), self.shallow(*target));
                    if matches!(self.tcx.tys.kind(target), TyKind::Dyn(..)) && matches!(self.tcx.tys.kind(self_ty), TyKind::Infer(_)) && self.type_var_is_sized(self_ty) {
                        return false;
                    }
                }
            }
        }
        let nested = {
            let inspect = InspectGoal::new(&self.infcx, self.prog, root);
            match certainty {
                // If we prove the goal, continue recursing.
                Ok(Certainty::Yes) => return false,
                Err(()) => {
                    visitor.errored = true;
                    // Even with no solution, continue recursing if there is a single candidate for
                    // which we're shallowly certain it holds, to find the real error source.
                    let candidates = inspect.candidates();
                    match &candidates[..] {
                        [only] if only.shallow_certainty.is_yes() && only.has_final_state() => only.instantiate_nested_goals(),
                        _ => return true,
                    }
                }
                Ok(Certainty::Maybe(_)) => match inspect.unique_applicable_candidate() {
                    Some(cand) if cand.shallow_certainty.is_yes() => cand.instantiate_nested_goals(),
                    _ => return true,
                },
            }
        };
        self.visit_coerce_nested(visitor, nested, depth, unsize, coerce_unsized)
    }

    fn visit_coerce_nested(&mut self, visitor: &mut CoerceVisitor, nested: Vec<Goal>, depth: usize, unsize: DefId, coerce_unsized: DefId) -> bool {
        for goal in nested {
            if self.visit_coerce_goal(visitor, goal, depth + 1, unsize, coerce_unsized) { return true; }
        }
        false
    }

    /// The tails the coercion unsizes: through pointers and struct tails to the types that
    /// become / are trait objects, slices.
    fn unsize_tails(&mut self, source: TyId, target: TyId) -> Option<(TyId, TyId)> {
        let (ks, kt) = (self.tcx.tys.kind(source).clone(), self.tcx.tys.kind(target).clone());
        let (mut s, mut d) = match (ks, kt) {
            (TyKind::Ref(_, a, _) | TyKind::Ptr(_, a), TyKind::Ref(_, d, _) | TyKind::Ptr(_, d)) => (a, d),
            (TyKind::Adt(d1, xs, _), TyKind::Adt(d2, ys, _)) if d1 == d2 => {
                let (xs, ys) = (self.tcx.tys.list(xs).to_vec(), self.tcx.tys.list(ys).to_vec());
                let mut found = None;
                for i in 0..xs.len().min(ys.len()) { let (x, y) = (self.shallow(xs[i]), self.shallow(ys[i])); if x != y { found = Some((xs[i], ys[i])); } }
                found?
            }
            _ => return None,
        };
        loop {
            let (ss, dd) = (self.shallow(s), self.shallow(d));
            if matches!(self.tcx.tys.kind(dd), TyKind::Dyn(..) | TyKind::Slice(_)) { return Some((ss, dd)); }
            match (self.tcx.tys.kind(ss).clone(), self.tcx.tys.kind(dd).clone()) {
                (TyKind::Adt(a1, xa, _), TyKind::Adt(a2, ya, _)) if a1 == a2 && self.prog.def(a1).kind == DefKind::Struct && self.tcx.tys.list(xa).len() == self.tcx.tys.list(ya).len() => {
                    let (xa, ya) = (self.tcx.tys.list(xa).to_vec(), self.tcx.tys.list(ya).to_vec());
                    let mut next = None;
                    for j in 0..xa.len() { let (x, y) = (self.shallow(xa[j]), self.shallow(ya[j])); if x != y { next = Some((xa[j], ya[j])); } }
                    match next { Some((x, y)) => { s = x; d = y; } None => return Some((ss, dd)) }
                }
                _ => return Some((ss, dd)),
            }
        }
    }

    fn unsize_kind(&mut self, source: TyId, target: TyId) -> UnsizeKind {
        let Some((s, d)) = self.unsize_tails(source, target) else { return UnsizeKind::Unsize };
        match (self.tcx.tys.kind(s).clone(), self.tcx.tys.kind(d).clone()) {
            (TyKind::Dyn(..), TyKind::Dyn(..)) => UnsizeKind::Upcast(s, d),
            (TyKind::Infer(_), TyKind::Dyn(..)) => UnsizeKind::Deferred,
            (_, TyKind::Dyn(..)) => UnsizeKind::ToDyn(s, d),
            _ => UnsizeKind::Unsize,
        }
    }
}
