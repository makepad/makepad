//! Structural const qualification and its flow-sensitive propagation:
//! rustc_const_eval/src/check_consts/{qualifs,resolver}.rs (1.97.1).
//!
//! A "qualif"(-ication) is a way to look for something "bad" in the MIR that would disqualify some
//! code for promotion or prevent it from evaluating at compile time. Const checking and promotion
//! use a value-based analysis (not a type-based one): qualifications propagate structurally across
//! locals; a local (or a projection of a local) assigned a qualified value becomes qualified.
//!
//! Differences from rustc, all about Relax's std and what a query can answer:
//! - `NeedsNonConstDrop` equals `NeedsDrop` unless the crate enables `const_destruct` or
//!   `const_precise_live_drops` (Relax has no `const Drop`/`[const] Destruct` impls: no drop is const).
//! - The qualifs of another constant's value come from a `ConstQualifsProvider` (rustc: the
//!   `mir_const_qualif` query on that constant's MIR); a provider that cannot answer blocks the check.
//! - A body with unresolved types, error types or coroutine locals is `Blocked` up front, so the
//!   `in_*` functions are infallible.
use super::const_check::ConstCx;
use crate::mir::visit::Visitor;
use crate::mir::*;
use crate::program::DefId;
use crate::types::{TyId, TyKind, TypeFlags};

/// A question the pass cannot answer for this body: the body is unchecked, never accepted or rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked(pub &'static str);

/// rustc_middle `ConstQualifs`: the qualifs of a const body's return place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConstQualifs {
    pub has_mut_interior: bool,
    pub needs_drop: bool,
    pub needs_non_const_drop: bool,
    pub tainted_by_errors: bool,
}

/// `tcx.mir_const_qualif(def)` for the constants a body names (checked at S11 by building and
/// checking that constant's MIR, with cycle detection).
pub trait ConstQualifsProvider {
    fn mir_const_qualif(&self, def: DefId) -> Result<ConstQualifs, Blocked>;
}

/// The provider that knows nothing: referring to a constant's value blocks the check.
pub struct NoProvider;
impl ConstQualifsProvider for NoProvider {
    fn mir_const_qualif(&self, _def: DefId) -> Result<ConstQualifs, Blocked> {
        Err(Blocked("no constant qualification provider"))
    }
}

/// qualifs.rs:`in_any_value_of_ty`.
pub fn in_any_value_of_ty(cx: &ConstCx<'_>, ty: TyId, tainted_by_errors: bool) -> ConstQualifs {
    ConstQualifs {
        has_mut_interior: HasMutInterior::in_any_value_of_ty(cx, ty),
        needs_drop: NeedsDrop::in_any_value_of_ty(cx, ty),
        needs_non_const_drop: NeedsNonConstDrop::in_any_value_of_ty(cx, ty),
        tainted_by_errors,
    }
}

/// Whether the types of `body` can be qualified at all: no error or inference types and no
/// (a coroutine is qualified conservatively: it needs drop and has interior mutability).
pub fn check_body_types(cx: &ConstCx<'_>) -> Result<(), Blocked> {
    for decl in &cx.body.local_decls {
        let flags = cx.tcx.tys.flags(decl.ty);
        if flags.intersects(TypeFlags::HAS_ERROR) || flags.intersects(TypeFlags::HAS_NON_REGION_INFER) {
            return Err(Blocked("unresolved type in constant qualification"));
        }
    }
    Ok(())
}

/// qualifs.rs:`trait Qualif`.
pub trait Qualif {
    /// Whether this `Qualif` is cleared when a local is moved from.
    const IS_CLEARED_ON_MOVE: bool = false;

    /// Whether this `Qualif` might be evaluated after the promotion and can encounter a promoted.
    const ALLOW_PROMOTED: bool = false;

    /// Extracts the field of `ConstQualifs` that corresponds to this `Qualif`.
    fn in_qualifs(qualifs: &ConstQualifs) -> bool;

    /// Returns `true` if *any* value of the given type could possibly have this `Qualif`.
    ///
    /// This function determines `Qualif`s when we cannot do a value-based analysis. Since qualif
    /// propagation is context-insensitive, this includes function arguments and values returned
    /// from a call to another function.
    ///
    /// It also determines the `Qualif`s for primitive types.
    fn in_any_value_of_ty(cx: &ConstCx<'_>, ty: TyId) -> bool;

    /// Returns `true` if the `Qualif` is structural in an ADT's fields, i.e. if we may recurse into
    /// an operand *value* to determine whether it has this `Qualif`.
    ///
    /// If this returns false, `in_any_value_of_ty` will be invoked to determine the final qualif
    /// for this ADT.
    fn is_structural_in_adt_value(cx: &ConstCx<'_>, adt: DefId) -> bool;
}

fn is_unsafe_cell(cx: &ConstCx<'_>, ty: TyId) -> bool {
    matches!(*cx.tcx.tys.kind(ty), TyKind::Adt(d, ..) if cx.tcx.adts.get(&d).is_some_and(|a| a.is_unsafe_cell()))
}

/// Constant containing interior mutability (`UnsafeCell<T>`). This must be ruled out to make sure
/// that evaluating the constant at compile-time and at *any point* during the run-time would
/// produce the same result. In particular, promotion of temporaries must not change program
/// behavior; if the promoted could be written to, that would be a problem.
pub struct HasMutInterior;

impl Qualif for HasMutInterior {
    fn in_qualifs(qualifs: &ConstQualifs) -> bool {
        qualifs.has_mut_interior
    }

    fn in_any_value_of_ty(cx: &ConstCx<'_>, ty: TyId) -> bool {
        // Avoid selecting for `UnsafeCell`
        if is_unsafe_cell(cx, ty) {
            return true;
        }
        !cx.tcx.is_freeze(cx.prog, cx.env, ty)
    }

    fn is_structural_in_adt_value(cx: &ConstCx<'_>, adt: DefId) -> bool {
        // Exactly one type, `UnsafeCell`, has the `HasMutInterior` qualif inherently. It arises
        // structurally for all other types.
        !cx.tcx.adts.get(&adt).is_some_and(|a| a.is_unsafe_cell())
    }
}

/// Constant containing an ADT that implements `Drop`. This must be ruled out because implicit
/// promotion would remove side-effects that occur as part of dropping that value. N.B., the
/// implicit promotion has to reject const Drop implementations because even if side-effects are
/// ruled out through other means, the execution of the drop could diverge.
pub struct NeedsDrop;

impl Qualif for NeedsDrop {
    const IS_CLEARED_ON_MOVE: bool = true;
    const ALLOW_PROMOTED: bool = true;

    fn in_qualifs(qualifs: &ConstQualifs) -> bool {
        qualifs.needs_drop
    }

    fn in_any_value_of_ty(cx: &ConstCx<'_>, ty: TyId) -> bool {
        cx.tcx.needs_drop(cx.prog, cx.env, ty)
    }

    fn is_structural_in_adt_value(cx: &ConstCx<'_>, adt: DefId) -> bool {
        cx.tcx.adt_destructor(adt).is_none()
    }
}

/// Constant containing an ADT that implements non-const `Drop`. This must be ruled out because we
/// cannot run `Drop` during compile-time.
pub struct NeedsNonConstDrop;

impl Qualif for NeedsNonConstDrop {
    const IS_CLEARED_ON_MOVE: bool = true;
    const ALLOW_PROMOTED: bool = true;

    fn in_qualifs(qualifs: &ConstQualifs) -> bool {
        qualifs.needs_non_const_drop
    }

    fn in_any_value_of_ty(cx: &ConstCx<'_>, ty: TyId) -> bool {
        // Relax has no const `Drop` impls: with `const_destruct` no drop is judged, without it every
        // drop is non-const.
        !cx.features.const_drops && NeedsDrop::in_any_value_of_ty(cx, ty)
    }

    fn is_structural_in_adt_value(cx: &ConstCx<'_>, adt: DefId) -> bool {
        // As soon as an ADT has a destructor, the drop becomes non-structural in its value: the
        // destructor may modify the operand being dropped.
        cx.tcx.adt_destructor(adt).is_none()
    }
}

/// The type of the value an `Operand` produces.
pub fn operand_ty(cx: &ConstCx<'_>, operand: &Operand) -> TyId {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => place_ty(cx, place),
        Operand::Constant(c) => c.const_.ty(&cx.tcx.tys),
        Operand::RuntimeChecks(_) => cx.tcx.tys.bool_,
    }
}

pub fn place_ty(cx: &ConstCx<'_>, place: &Place) -> TyId {
    place_prefix_ty(cx, place, place.projection.len())
}

/// The type of `place` after its first `n` projection elements.
pub fn place_prefix_ty(cx: &ConstCx<'_>, place: &Place, n: usize) -> TyId {
    let mut ty = PlaceTy::from_ty(cx.body.local_decls[place.local.0 as usize].ty);
    for elem in &place.projection[..n] {
        ty = ty.projection_ty(cx.tcx, elem).expect("ill-typed place in const checking");
    }
    ty.ty
}

/// A place with no `Deref` projection.
pub fn is_indirect(place: &Place) -> bool {
    place.projection.iter().any(|elem| matches!(elem, PlaceElem::Deref))
}

// FIXME: Use `mir::visit::Visitor` for the `in_*` functions if/when it supports early return.

/// Returns `true` if this `Rvalue` contains qualif `Q`.
pub fn in_rvalue<Q: Qualif, F: FnMut(Local) -> bool>(cx: &ConstCx<'_>, in_local: &mut F, rvalue: &Rvalue) -> bool {
    match rvalue {
        // a thread-local pointer is a raw pointer/reference: neither drops nor has interior mutability
        Rvalue::ThreadLocalRef(_) => false,

        Rvalue::Discriminant(place) => in_place::<Q, _>(cx, in_local, place),

        Rvalue::CopyForDeref(place) => in_place::<Q, _>(cx, in_local, place),

        Rvalue::Use(operand) | Rvalue::Repeat(operand, _) | Rvalue::UnaryOp(_, operand) | Rvalue::Cast(_, operand, _) => in_operand::<Q, _>(cx, in_local, operand),

        Rvalue::BinaryOp(_, ops) => in_operand::<Q, _>(cx, in_local, &ops.0) || in_operand::<Q, _>(cx, in_local, &ops.1),

        Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place) => {
            // Special-case reborrows to be more like a copy of the reference.
            if let Some((place_base, PlaceElem::Deref)) = last_projection(place) {
                let base_ty = place_ty(cx, &place_base);
                if matches!(cx.tcx.tys.kind(base_ty), TyKind::Ref(..)) {
                    return in_place::<Q, _>(cx, in_local, &place_base);
                }
            }

            in_place::<Q, _>(cx, in_local, place)
        }

        Rvalue::Aggregate(kind, operands) => {
            // Return early if we know that the struct or enum being constructed is always qualified.
            if let AggregateKind::Adt(adt, _variant, args, _, _) = &**kind {
                let is_union = cx.tcx.adts.get(adt).is_some_and(|a| a.is_union());
                // Don't do any value-based reasoning for unions. Also, if the ADT is not structural
                // in its fields, then we cannot recurse on its fields. Instead, we fall back to
                // checking the qualif for *any* value of the ADT.
                if is_union || !Q::is_structural_in_adt_value(cx, *adt) {
                    return Q::in_any_value_of_ty(cx, adt_ty(cx, *adt, args));
                }
            }

            // Otherwise, proceed structurally...
            operands.iter().any(|o| in_operand::<Q, _>(cx, in_local, o))
        }
    }
}

/// The ADT type an `Adt` aggregate with `args` builds (type, const and lifetime arguments).
fn adt_ty(cx: &ConstCx<'_>, adt: DefId, args: &[crate::consts::GenericArg]) -> TyId {
    use crate::consts::GenericArg;
    let tys = &cx.tcx.tys;
    let mut type_args = Vec::new();
    let mut regions = Vec::new();
    for arg in args {
        match *arg {
            GenericArg::Type(ty) => type_args.push(ty),
            GenericArg::Const(ct) => type_args.push(tys.intern(TyKind::Const(ct))),
            GenericArg::Lifetime(region) => regions.push(region),
        }
    }
    tys.intern(TyKind::Adt(adt, tys.mk_list(&type_args), tys.intern_region_args(&regions)))
}

/// The place without its last projection element, and that element.
pub fn last_projection(place: &Place) -> Option<(Place, PlaceElem)> {
    let (last, rest) = place.projection.split_last()?;
    Some((Place { local: place.local, projection: rest.to_vec() }, last.clone()))
}

/// Returns `true` if this `Place` contains qualif `Q`.
pub fn in_place<Q: Qualif, F: FnMut(Local) -> bool>(cx: &ConstCx<'_>, in_local: &mut F, place: &Place) -> bool {
    let mut len = place.projection.len();
    while len > 0 {
        let elem = &place.projection[len - 1];
        if let PlaceElem::Index(index) = elem {
            if in_local(*index) {
                return true;
            }
        }

        let proj_ty = place_prefix_ty(cx, place, len);
        if !Q::in_any_value_of_ty(cx, proj_ty) {
            return false;
        }

        // `Deref` currently unconditionally "qualifies" if `in_any_value_of_ty` returns true, i.e.,
        // we treat all qualifs as non-structural for deref projections. Generally, we can say very
        // little about `*ptr` even if we know that `ptr` satisfies all sorts of properties.
        if matches!(elem, PlaceElem::Deref) {
            // We have to assume that this qualifies.
            return true;
        }

        len -= 1;
    }

    in_local(place.local)
}

/// Returns `true` if this `Operand` contains qualif `Q`.
pub fn in_operand<Q: Qualif, F: FnMut(Local) -> bool>(cx: &ConstCx<'_>, in_local: &mut F, operand: &Operand) -> bool {
    let constant = match operand {
        Operand::Copy(place) | Operand::Move(place) => return in_place::<Q, _>(cx, in_local, place),
        Operand::RuntimeChecks(_) => return Q::in_any_value_of_ty(cx, cx.tcx.tys.bool_),
        Operand::Constant(c) => c,
    };

    // Check the qualifs of the value of `const` items.
    if let Const::Unevaluated { def, promoted, .. } = &constant.const_ {
        // Use qualifs of the type for the promoted. Promoteds in MIR body should be possible only
        // for `NeedsNonConstDrop` with precise drop checking. This is the only const check
        // performed after the promotion. Verify that with an assertion.
        assert!(promoted.is_none() || Q::ALLOW_PROMOTED);

        // Don't peek inside trait associated constants.
        let in_trait = cx.prog.def(cx.prog.def(*def).parent).kind == crate::program::DefKind::Trait;
        if promoted.is_none() && !in_trait && matches!(cx.prog.def(*def).kind, crate::program::DefKind::Const | crate::program::DefKind::AssocConst | crate::program::DefKind::AnonConst) {
            match cx.provider.mir_const_qualif(*def) {
                Ok(qualifs) => {
                    if !Q::in_qualifs(&qualifs) {
                        return false;
                    }
                }
                // an unanswerable constant is recorded; assume the qualif (the caller reports the block)
                Err(blocked) => {
                    cx.note_blocked(blocked);
                    return true;
                }
            }
            // Just in case the type is more specific than the definition, e.g., impl associated
            // const with type parameters, take it into account.
        }
    }

    // Otherwise use the qualifs of the type.
    Q::in_any_value_of_ty(cx, constant.const_.ty(&cx.tcx.tys))
}

// ---------------------------------------------------------------------------------------------
// resolver.rs: propagating qualifs between locals

/// The state of the `FlowSensitiveAnalysis`: two sets of locals, packed in one bit set
/// (`qualif` in `0..n`, `borrow` in `n..2n`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    n: usize,
    bits: crate::mir::dataflow::BitSet,
}

impl State {
    fn new_empty(n: usize) -> Self {
        State { n, bits: crate::mir::dataflow::BitSet::new_empty(2 * n) }
    }
    /// Whether a local contains the qualif.
    pub fn contains(&self, local: Local) -> bool {
        self.bits.contains(local.0 as usize)
    }
    fn insert_qualif(&mut self, local: Local) {
        self.bits.insert(local.0 as usize);
    }
    fn remove_qualif(&mut self, local: Local) {
        self.bits.remove(local.0 as usize);
    }
    fn borrowed(&self, local: Local) -> bool {
        self.bits.contains(self.n + local.0 as usize)
    }
    fn insert_borrow(&mut self, local: Local) {
        self.bits.insert(self.n + local.0 as usize);
    }
    fn remove_borrow(&mut self, local: Local) {
        self.bits.remove(self.n + local.0 as usize);
    }
    fn join(&mut self, other: &State) -> bool {
        self.bits.union(&other.bits)
    }
}

/// A `Visitor` that propagates qualifs between locals. This defines the transfer function of the
/// flow-sensitive analysis.
///
/// To account for indirect assignments, data flow conservatively assumes that local becomes
/// qualified immediately after it is borrowed or its address escapes. The borrow must allow for
/// mutation, which includes shared borrows of places with interior mutability. The type of borrowed
/// place must contain the qualif.
struct TransferFunction<'a, Q> {
    ccx: &'a ConstCx<'a>,
    state: &'a mut State,
    _qualif: std::marker::PhantomData<Q>,
}

impl<'a, Q: Qualif> TransferFunction<'a, Q> {
    fn new(ccx: &'a ConstCx<'a>, state: &'a mut State) -> Self {
        TransferFunction { ccx, state, _qualif: std::marker::PhantomData }
    }

    fn initialize_state(&mut self) {
        self.state.bits.clear();
        for i in 1..=self.ccx.body.arg_count {
            let arg_ty = self.ccx.body.local_decls[i as usize].ty;
            if Q::in_any_value_of_ty(self.ccx, arg_ty) {
                self.state.insert_qualif(Local(i));
            }
        }
    }

    fn assign_qualif_direct(&mut self, place: &Place, mut value: bool) {
        debug_assert!(!is_indirect(place));

        if !value {
            for n in 0..place.projection.len() {
                let base_ty = place_prefix_ty(self.ccx, place, n);
                let is_union = matches!(*self.ccx.tcx.tys.kind(base_ty), TyKind::Adt(d, ..) if self.ccx.tcx.adts.get(&d).is_some_and(|a| a.is_union()));
                if is_union && Q::in_any_value_of_ty(self.ccx, base_ty) {
                    value = true;
                    break;
                }
            }
        }

        match (value, place.projection.is_empty()) {
            (true, _) => self.state.insert_qualif(place.local),

            // For now, we do not clear the qualif if a local is overwritten in full by an
            // unqualified rvalue (e.g. `y = 5`). This is to be consistent with aggregates where we
            // overwrite all fields with assignments, which would not get this feature.
            (false, true) => {}

            _ => {}
        }
    }

    fn apply_call_return_effect(&mut self, place: &Place) {
        // We cannot reason about another function's internals, so use conservative type-based
        // qualification for the result of a function call.
        let return_ty = place_ty(self.ccx, place);
        let qualif = Q::in_any_value_of_ty(self.ccx, return_ty);

        if !is_indirect(place) {
            self.assign_qualif_direct(place, qualif);
        }
    }

    fn address_of_allows_mutation(&self) -> bool {
        // Exact set of permissions granted by RawPtr is undecided. Conservatively assume that it
        // might allow mutation until resolution of #56604.
        true
    }

    fn ref_allows_mutation(&self, kind: BorrowKind, place: &Place) -> bool {
        match kind {
            BorrowKind::Mut { .. } => true,
            BorrowKind::Shared | BorrowKind::Fake(_) => self.shared_borrow_allows_mutation(place),
        }
    }

    /// `&` only allow mutation if the borrowed place is `!Freeze`.
    ///
    /// This assumes that it is UB to take the address of a struct field whose type is `Freeze`, then
    /// use pointer arithmetic to derive a pointer to a *different* field of that same struct whose
    /// type is `!Freeze`. If we decide that this is not UB, we will have to check the type of the
    /// borrowed **local** instead of the borrowed **place** below. See
    /// [rust-lang/unsafe-code-guidelines#134].
    ///
    /// [rust-lang/unsafe-code-guidelines#134]: https://github.com/rust-lang/unsafe-code-guidelines/issues/134
    fn shared_borrow_allows_mutation(&self, place: &Place) -> bool {
        !self.ccx.tcx.is_freeze(self.ccx.prog, self.ccx.env, place_ty(self.ccx, place))
    }
}

impl<Q: Qualif> Visitor for TransferFunction<'_, Q> {
    fn visit_operand(&mut self, operand: &Operand, location: Location) {
        self.super_operand(operand, location);

        if !Q::IS_CLEARED_ON_MOVE {
            return;
        }

        // If a local with no projections is moved from (e.g. `x` in `y = x`), record that it no
        // longer needs to be dropped.
        if let Operand::Move(place) = operand {
            if place.projection.is_empty() {
                // For backward compatibility with the MaybeMutBorrowedLocals used in an earlier
                // implementation we retain qualif if a local had been borrowed before. This might
                // not be strictly necessary since the local is no longer initialized.
                if !self.state.borrowed(place.local) {
                    self.state.remove_qualif(place.local);
                }
            }
        }
    }

    fn visit_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        self.super_rvalue(rvalue, location);

        match rvalue {
            Rvalue::RawPtr(_mt, borrowed_place) => {
                if !is_indirect(borrowed_place) && self.address_of_allows_mutation() {
                    let place_ty = place_ty(self.ccx, borrowed_place);
                    if Q::in_any_value_of_ty(self.ccx, place_ty) {
                        self.state.insert_qualif(borrowed_place.local);
                        self.state.insert_borrow(borrowed_place.local);
                    }
                }
            }

            Rvalue::Ref(_, kind, borrowed_place) => {
                if !is_indirect(borrowed_place) && self.ref_allows_mutation(*kind, borrowed_place) {
                    let place_ty = place_ty(self.ccx, borrowed_place);
                    if Q::in_any_value_of_ty(self.ccx, place_ty) {
                        self.state.insert_qualif(borrowed_place.local);
                        self.state.insert_borrow(borrowed_place.local);
                    }
                }
            }

            Rvalue::Cast(..)
            | Rvalue::Use(..)
            | Rvalue::CopyForDeref(..)
            | Rvalue::ThreadLocalRef(..)
            | Rvalue::Repeat(..)
            | Rvalue::BinaryOp(..)
            | Rvalue::UnaryOp(..)
            | Rvalue::Discriminant(..)
            | Rvalue::Aggregate(..) => {}
        }
    }

    fn visit_statement(&mut self, statement: &Statement, location: Location) {
        match &statement.kind {
            StatementKind::StorageDead(local) => {
                self.state.remove_qualif(*local);
                self.state.remove_borrow(*local);
            }
            StatementKind::Assign(place, rvalue) => {
                let state = &*self.state;
                let qualif = in_rvalue::<Q, _>(self.ccx, &mut |l| state.contains(l), rvalue);
                if !is_indirect(place) {
                    self.assign_qualif_direct(place, qualif);
                }

                // We need to assign qualifs to the left-hand side before visiting `rvalue` since
                // qualifs can be cleared on move.
                self.visit_rvalue(rvalue, location);
            }
            _ => self.super_statement(statement, location),
        }
    }

    fn visit_terminator(&mut self, terminator: &Terminator, location: Location) {
        // The effect of assignment to the return place in `TerminatorKind::Call` is not applied
        // here; that occurs in `apply_call_return_effect`.

        // We ignore borrow on drop because custom drop impls are not allowed in consts.
        self.super_terminator(terminator, location);
    }
}

/// Every successor of a terminator (normal, unwind and imaginary edges).
pub fn successors(term: &Terminator) -> Vec<BasicBlock> {
    super::simplify::successors(&term.kind)
}

/// The dataflow results of one qualif over a body: the state at the entry of every reachable block.
pub struct QualifResults<Q> {
    entry: Vec<Option<State>>,
    /// The cursor: the block, the number of its statements already applied, and the state there.
    cursor: std::cell::RefCell<Option<(usize, usize, State)>>,
    _qualif: std::marker::PhantomData<Q>,
}

impl<Q: Qualif> QualifResults<Q> {
    /// Iterates the flow-sensitive analysis of `Q` to its fixpoint (a forward may-analysis with union
    /// joins; the transfer effects of statements and terminators are applied in rustc's order, the
    /// effect of a call's return only on its return edge).
    pub fn compute(ccx: &ConstCx<'_>) -> Self {
        let body = ccx.body;
        let n = body.local_decls.len();
        let mut entry: Vec<Option<State>> = vec![None; body.basic_blocks.len()];
        let mut start = State::new_empty(n);
        TransferFunction::<Q>::new(ccx, &mut start).initialize_state();
        entry[0] = Some(start);
        let mut queue = std::collections::VecDeque::from([0usize]);
        let mut queued = vec![false; body.basic_blocks.len()];
        queued[0] = true;
        while let Some(bb) = queue.pop_front() {
            queued[bb] = false;
            let block = &body.basic_blocks[bb];
            let mut state = entry[bb].clone().expect("queued block has a state");
            for (i, statement) in block.statements.iter().enumerate() {
                TransferFunction::<Q>::new(ccx, &mut state).visit_statement(statement, Location { block: BasicBlock(bb as u32), statement_index: i });
            }
            let term = block.terminator.as_ref().expect("unfinished MIR block");
            let at = Location { block: BasicBlock(bb as u32), statement_index: block.statements.len() };
            TransferFunction::<Q>::new(ccx, &mut state).visit_terminator(term, at);
            // the call's return place is assigned on the return edge only
            let return_edge = match &term.kind {
                TerminatorKind::Call { destination, target: Some(target), .. } => Some((*target, destination)),
                TerminatorKind::Yield { resume_arg, resume, .. } => Some((*resume, resume_arg)),
                _ => None,
            };
            for succ in successors(term) {
                let mut edge_state = state.clone();
                if let Some((target, place)) = return_edge {
                    if target == succ {
                        TransferFunction::<Q>::new(ccx, &mut edge_state).apply_call_return_effect(place);
                    }
                }
                let changed = match &mut entry[succ.0 as usize] {
                    None => {
                        entry[succ.0 as usize] = Some(edge_state);
                        true
                    }
                    Some(old) => old.join(&edge_state),
                };
                if changed && !queued[succ.0 as usize] {
                    queued[succ.0 as usize] = true;
                    queue.push_back(succ.0 as usize);
                }
            }
        }
        QualifResults { entry, cursor: std::cell::RefCell::new(None), _qualif: std::marker::PhantomData }
    }

    /// Whether `local` has the qualif just before the primary effect at `location`
    /// (`seek_before_primary_effect`). Unreachable blocks have no qualifs. The cursor resumes from
    /// where the last query stopped when it is in the same block, at or before this statement.
    pub fn contains_before(&self, ccx: &ConstCx<'_>, local: Local, location: Location) -> bool {
        let block_index = location.block.0 as usize;
        let Some(entry) = &self.entry[block_index] else { return false };
        let mut cursor = self.cursor.borrow_mut();
        let (mut next, mut state) = match cursor.take() {
            Some((block, next, state)) if block == block_index && next <= location.statement_index => (next, state),
            _ => (0, entry.clone()),
        };
        let block = &ccx.body.basic_blocks[block_index];
        while next < location.statement_index {
            TransferFunction::<Q>::new(ccx, &mut state).visit_statement(&block.statements[next], Location { block: location.block, statement_index: next });
            next += 1;
        }
        let contains = state.contains(local);
        *cursor = Some((block_index, next, state));
        contains
    }
}
