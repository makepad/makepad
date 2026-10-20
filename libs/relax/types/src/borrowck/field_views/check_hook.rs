//! Relaxed Rust in the C-NLL4 conflict check: field views (A1) and extended two-phase borrows
//! (A3). Included from check.rs as a child module, so it extends the private checker state.
//!
//! Re-port of the BIR nll.rs `access_fv` / `parts` / `ext_two_phase` / `fv_explain` onto the
//! C-NLL2 place model (`places_conflict`, `BorrowSet`, two-phase activation). For a conflicting
//! access against a loan the verdict is one of:
//!  - accepted only with extended two-phase borrows: a `&mut` reserved for a call argument and
//!    not used until the call may be borrowed again meanwhile (`f(vm, g(vm))`);
//!  - accepted only with field views: the access's and the loan's parts (the places the callee /
//!    closure a borrow is fed to uses, from its summary) do not conflict;
//!  - the conflict stands (with a note naming what the callees touch, if views were involved).

use super::*;
use crate::borrowck::field_views::{callee_of, summaries, Feed, FieldViews, FvRecordRaw};
use crate::borrowck::borrowck_errors::{FvNote, FvWho};

pub(super) struct FvRun<'a> {
    pub views: &'a FieldViews<'a>,
    /// single-use temps holding a borrow that feed a call argument or a closure capture
    feeds: crate::fxhash::HashMap<Local, Feed>,
    parts: crate::fxhash::HashMap<usize, Option<Vec<(Place, BorrowKind)>>>,
}
impl<'a> FvRun<'a> {
    pub(super) fn new(views: &'a FieldViews<'a>, body: &Body, _borrows: &BorrowSet) -> Self {
        Self { views, feeds: crate::borrowck::field_views::find_feeds(body), parts: Default::default() }
    }
}

pub(super) struct FvOutcome {
    /// the conflict disappears under field views / extended two-phase borrows
    pub accepted: bool,
    pub note: Option<FvNote>,
    pub record: Option<FvRecordRaw>,
}

impl<'a> MirBorrowckCtxt<'a> {
    /// The loan an access creates: the borrow of `place` made at `at`.
    pub(super) fn fv_own_loan(&self, at: Location, place: &Place) -> Option<usize> {
        self.fv.as_ref()?;
        self.borrows.borrows_at_location(&at)?.iter().copied().find(|&i| self.borrows[i].borrowed_place == *place)
    }

    fn fv_feed(&self, li: usize) -> Option<Feed> {
        let b = &self.borrows[li];
        if !b.assigned_place.projection.is_empty() { return None; }
        self.fv.as_ref()?.feeds.get(&b.assigned_place.local).copied()
    }

    /// The parts of a borrow fed to a call / closure whose summary is known (None: the whole
    /// place): the borrowed place extended by each summary part, mutable only where both the
    /// summary and the loan are.
    fn fv_parts(&mut self, li: usize) -> Option<Vec<(Place, BorrowKind)>> {
        if let Some(p) = self.fv.as_ref()?.parts.get(&li) { return p.clone(); }
        let r = self.fv_parts_uncached(li);
        self.fv.as_mut()?.parts.insert(li, r.clone());
        r
    }
    fn fv_parts_uncached(&self, li: usize) -> Option<Vec<(Place, BorrowKind)>> {
        let views = self.fv.as_ref()?.views;
        let feed = self.fv_feed(li)?;
        let l = &self.borrows[li];
        // a borrow of a value reached through derefs and fields (`*self`, `*cx`, `cx.vm`, a
        // local) is narrowed; element / variant places stay whole
        if l.borrowed_place.projection.iter().any(|p| matches!(p, PlaceElem::Index(_) | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } | PlaceElem::Downcast(..))) {
            return None;
        }
        let summ = match feed {
            Feed::Call(bi, ai) => {
                let Some(Terminator { kind: TerminatorKind::Call { func, .. }, .. }) = &self.body.basic_blocks[bi].terminator else { return None };
                views.with_env(self.tcx, |env| {
                    let (d, a) = callee_of(env, func)?;
                    summaries(env, d, &a, 0).get(ai).cloned()
                })?
            }
            Feed::Closure(ce, k) => views.closure_fv.get(&ce)?.get(k).cloned()?,
        };
        match summ {
            crate::borrowck::field_views::Summary::Whole => None,
            crate::borrowck::field_views::Summary::Parts(v) => Some(
                v.into_iter().map(|(sub, m)| {
                    let mut q = l.borrowed_place.clone();
                    q.projection.extend(sub);
                    (q, if m && matches!(l.kind, BorrowKind::Mut { .. }) { l.kind } else { BorrowKind::Shared })
                }).collect(),
            ),
        }
    }

    /// Extended two-phase borrows: the reserved `&mut` (not yet active) may be borrowed again by
    /// an access that cannot move what the reservation points to: the reserved place is the
    /// accessed place or reached from it through fields only.
    fn fv_ext_two_phase(&self, place: &Place, access: ReadOrWrite, lplace: &Place, lkind: BorrowKind, li: usize, at: Location) -> bool {
        if !matches!(lkind, BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow }) { return false; }
        if is_active(|a, b| self.cfg.dominates(a, b), &self.borrows[li], at) { return false; }
        let ok_access = match access {
            Write(WriteKind::MutableBorrow(_)) | Reservation(_) | Read(ReadKind::Borrow(_)) | Read(ReadKind::Copy) => true,
            Activation(_, a) => a != li,
            _ => false,
        };
        if !ok_access { return false; }
        let k = place.projection.len();
        lplace.projection.len() <= k || lplace.projection[k..].iter().all(|x| matches!(x, PlaceElem::Field(..)))
    }

    pub(super) fn fv_outcome(&mut self, li: usize, place: &Place, depth: AccessDepth, access: ReadOrWrite, at: Location, span: Span) -> Option<FvOutcome> {
        self.fv.as_ref()?;
        // Dropping or deallocating storage conflicts with every part of the place.
        if matches!(access, Write(WriteKind::StorageDeadOrDrop)) { return None; }
        let (lplace, lkind) = (self.borrows[li].borrowed_place.clone(), self.borrows[li].kind);
        // extended two-phase borrows
        if self.fv_ext_two_phase(place, access, &lplace, lkind, li, at) {
            let reserved = self.span_at(self.borrows[li].reserve_location);
            // the export to rustc: the argument holding this access is evaluated into a local
            // before the statement
            let mut record = None;
            if let Some(Feed::Call(bi, _)) = self.fv_feed(li) {
                if let Some(Terminator { kind: TerminatorKind::Call { arg_spans, .. }, .. }) = &self.body.basic_blocks[bi].terminator {
                    if let Some(k) = arg_spans.iter().position(|a| a.file == span.file && a.lo <= span.lo && span.hi <= a.hi && a.hi > a.lo) {
                        record = Some(FvRecordRaw { span: arg_spans[k], place: None, default: false, access: span, hoist: Some(arg_spans[..k].to_vec()) });
                    }
                }
            }
            return Some(FvOutcome { accepted: true, note: Some(FvNote::TwoPhase { reserved }), record });
        }
        // field views: the access's own parts against the loan's parts
        let own = match access { Activation(_, a) => Some(a), _ => self.fv_own };
        let acc_parts: Option<Vec<(Place, ReadOrWrite)>> = own.and_then(|o| self.fv_parts(o)).map(|v| {
            v.into_iter().map(|(q, k)| {
                let is_mut = matches!(k, BorrowKind::Mut { .. });
                let r2 = match access {
                    Activation(w, a) => if is_mut { Activation(w, a) } else { Read(ReadKind::Borrow(BorrowKind::Shared)) },
                    Reservation(_) | Write(WriteKind::MutableBorrow(_)) | Read(ReadKind::Borrow(_)) => if is_mut { access } else { Read(ReadKind::Borrow(BorrowKind::Shared)) },
                    x => x,
                };
                (q, r2)
            }).collect()
        });
        let loan_parts = self.fv_parts(li);
        let remains = if acc_parts.is_none() && loan_parts.is_none() {
            true
        } else {
            let ap = acc_parts.unwrap_or_else(|| vec![(place.clone(), access)]);
            let lp = loan_parts.unwrap_or_else(|| vec![(lplace.clone(), lkind)]);
            ap.iter().any(|(aq, arw)| lp.iter().any(|(lq, lk)| self.pair_conflicts(aq, depth, *arw, lq, *lk, li, at)))
        };
        if !remains {
            // accepted only with field views. The export moves the field-level borrow (the
            // loan, or the access when the loan is the whole value) out around the statement.
            let why = self.fv_explain(own, li);
            let (fspan, fplace) = if lplace.projection.iter().any(|x| matches!(x, PlaceElem::Field(..))) {
                (self.span_at(self.borrows[li].reserve_location), lplace.clone())
            } else { (span, place.clone()) };
            let views = self.fv.as_ref()?.views;
            let fty = self.fv_place_ty(&fplace);
            let default = views.default_trait.is_some_and(|td| {
                self.tcx.type_implements_trait(views.prog, views.typing_env, crate::next_solver::TraitRef { def_id: td, args: vec![crate::consts::GenericArg::Type(fty)] }).must_apply_modulo_regions()
            });
            let record = FvRecordRaw { span: fspan, place: Some(fplace), default, access: span, hoist: None };
            return Some(FvOutcome { accepted: true, note: Some(FvNote::Narrowed { why }), record: Some(record) });
        }
        // the conflict remains through field views: name what the call touches
        let involved = self.fv.as_ref()?.parts.get(&li).is_some_and(|x| x.is_some()) || own.is_some_and(|o| self.fv.as_ref().is_some_and(|f| f.parts.get(&o).is_some_and(|x| x.is_some())));
        let note = involved.then(|| FvNote::Remaining { why: self.fv_explain(own, li) });
        Some(FvOutcome { accepted: false, note, record: None })
    }

    fn fv_place_ty(&self, place: &Place) -> crate::types::TyId { self.ty(place) }

    /// "`m` uses mutably `self.counter`": the parts of the access's and the loan's views.
    fn fv_explain(&mut self, own: Option<usize>, li: usize) -> Vec<FvWho> {
        let mut out = Vec::new();
        for x in [own, Some(li)].into_iter().flatten() {
            let Some(parts) = self.fv_parts(x) else { continue };
            let (callee, is_closure) = match self.fv_feed(x) {
                Some(Feed::Call(bi, _)) => match &self.body.basic_blocks[bi].terminator {
                    Some(Terminator { kind: TerminatorKind::Call { func, .. }, .. }) => {
                        let views = self.fv.as_ref().unwrap().views;
                        (views.with_env(self.tcx, |env| callee_of(env, func)).map(|(d, _)| d), false)
                    }
                    _ => (None, false),
                },
                Some(Feed::Closure(..)) => (None, true),
                None => continue,
            };
            out.push(FvWho { callee, is_closure, parts: parts.into_iter().map(|(q, k)| (q, matches!(k, BorrowKind::Mut { .. }))).collect() });
        }
        out
    }
}
