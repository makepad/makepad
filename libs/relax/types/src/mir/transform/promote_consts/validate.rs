//! Checks whether locals that appear in a promotion context (`Candidate`) are actually promotable:
//! rustc_mir_transform/src/promote_consts.rs `Validator` (1.97.1).
use super::super::const_check::{ConstContext, ConstCx};
use super::super::qualifs::{in_rvalue, last_projection, operand_ty, place_prefix_ty, place_ty, HasMutInterior, NeedsDrop, Qualif};
use super::{is_thread_local_static, stmt_at, Candidate, StmtOrTerm, TempState};
use crate::mir::*;
use crate::program::DefKind;
use crate::types::TyKind;

struct Unpromotable;

/// Checks whether locals that appear in a promotion context (`Candidate`) are actually promotable.
struct Validator<'a> {
    ccx: &'a ConstCx<'a>,
    temps: &'a mut [TempState],
    /// For backwards compatibility, we are promoting function calls in `const`/`static` initializers.
    /// But we want to avoid evaluating code that might panic and that otherwise would not have been
    /// evaluated, so we only promote such calls in basic blocks that are guaranteed to execute. In
    /// other words, we only promote such calls in basic blocks that are definitely not dead code.
    /// Here we cache the result of computing that set of basic blocks.
    promotion_safe_blocks: Option<Vec<bool>>,
}

impl Validator<'_> {
    fn validate_candidate(&mut self, candidate: Candidate) -> Result<(), Unpromotable> {
        let StmtOrTerm::Stmt(statement) = stmt_at(self.ccx.body, candidate.location) else { unreachable!("candidate is not a statement") };
        let StatementKind::Assign(_, Rvalue::Ref(_, kind, place)) = &statement.kind else { unreachable!("candidate is not a borrow") };

        // We can only promote interior borrows of promotable temps (non-temps don't get promoted
        // anyway).
        self.validate_local(place.local)?;

        // The reference operation itself must be promotable. (Needs to come after `validate_local` to
        // avoid ICEs.)
        self.validate_ref(*kind, place)?;

        // We do not check all the projections (they do not get promoted anyway), but we do stay away
        // from promoting anything involving a dereference.
        if place.projection.contains(&PlaceElem::Deref) {
            return Err(Unpromotable);
        }

        Ok(())
    }

    fn qualif_local<Q: Qualif>(&mut self, local: Local) -> bool {
        let TempState::Defined { location: loc, .. } = self.temps[local.0 as usize] else {
            return false;
        };

        let ccx = self.ccx;
        match stmt_at(ccx.body, loc) {
            StmtOrTerm::Stmt(statement) => {
                let StatementKind::Assign(_, rhs) = &statement.kind else { unreachable!("{statement:?} is not an assignment") };
                in_rvalue::<Q, _>(ccx, &mut |l| self.qualif_local::<Q>(l), rhs)
            }
            StmtOrTerm::Term(terminator) => {
                assert!(matches!(terminator.kind, TerminatorKind::Call { .. }));
                let return_ty = ccx.body.local_decls[local.0 as usize].ty;
                Q::in_any_value_of_ty(ccx, return_ty)
            }
        }
    }

    fn validate_local(&mut self, local: Local) -> Result<(), Unpromotable> {
        let TempState::Defined { location: loc, uses, valid } = self.temps[local.0 as usize] else {
            return Err(Unpromotable);
        };

        // We cannot promote things that need dropping, since the promoted value would not get dropped.
        if self.qualif_local::<NeedsDrop>(local) {
            return Err(Unpromotable);
        }

        if valid {
            return Ok(());
        }

        let ccx = self.ccx;
        let ok = match stmt_at(ccx.body, loc) {
            StmtOrTerm::Stmt(statement) => {
                let StatementKind::Assign(_, rhs) = &statement.kind else { unreachable!("{statement:?} is not an assignment") };
                self.validate_rvalue(rhs)
            }
            StmtOrTerm::Term(terminator) => match &terminator.kind {
                TerminatorKind::Call { func, args, .. } => self.validate_call(func, args, loc.block),
                TerminatorKind::Yield { .. } => Err(Unpromotable),
                kind => unreachable!("{kind:?} not promotable"),
            },
        };

        self.temps[local.0 as usize] = match ok {
            Ok(()) => TempState::Defined { location: loc, uses, valid: true },
            Err(_) => TempState::Unpromotable,
        };

        ok
    }

    fn validate_place(&mut self, place: &Place) -> Result<(), Unpromotable> {
        let ccx = self.ccx;
        let Some((place_base, elem)) = last_projection(place) else {
            return self.validate_local(place.local);
        };

        // Validate topmost projection, then recurse.
        match &elem {
            // Recurse directly.
            PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } | PlaceElem::Subtype(_) => {}

            // Never recurse.
            PlaceElem::OpaqueCast(..) | PlaceElem::Downcast(..) => {
                return Err(Unpromotable);
            }

            PlaceElem::Deref => {
                // When a static is used by-value, that gets desugared to `*STATIC_ADDR`, and we need
                // to be able to promote this. So check if this deref matches that specific pattern.

                // We need to make sure this is a `Deref` of a local with no further projections.
                let is_static_deref = (|| {
                    if !place_base.projection.is_empty() {
                        return None;
                    }
                    let TempState::Defined { location, .. } = self.temps[place_base.local.0 as usize] else { return None };
                    let StmtOrTerm::Stmt(def_stmt) = stmt_at(ccx.body, location) else { return None };
                    let StatementKind::Assign(_, Rvalue::Use(Operand::Constant(c))) = &def_stmt.kind else { return None };
                    let did = c.const_.check_static_ptr(&ccx.body.allocs)?;
                    // Evaluating a promoted may not read statics except if it got promoted from a
                    // static (this is a CTFE check). So we can only promote static accesses inside
                    // statics.
                    matches!(ccx.const_kind, Some(ConstContext::Static(_))).then_some(())?;
                    (!is_thread_local_static(ccx.prog, did)).then_some(())
                })();
                if is_static_deref.is_none() {
                    return Err(Unpromotable);
                }
                // Recurse.
            }
            PlaceElem::Index(local) => {
                // Only accept if we can predict the index and are indexing an array.
                let ok = (|| {
                    let TempState::Defined { location: loc, .. } = self.temps[local.0 as usize] else { return None };
                    let StmtOrTerm::Stmt(statement) = stmt_at(ccx.body, loc) else { return None };
                    let StatementKind::Assign(_, Rvalue::Use(Operand::Constant(c))) = &statement.kind else { return None };
                    if !should_evaluate_for_promotion_checks(ccx, &c.const_) {
                        return None;
                    }
                    let (idx, _) = try_eval_scalar(ccx, &c.const_)?;
                    // Determine the type of the thing we are indexing.
                    let TyKind::Array(_, len) = *ccx.tcx.tys.kind(place_ty(ccx, &place_base)) else { return None };
                    // It's an array; determine its length.
                    let len = len.try_to_target_usize(&ccx.tcx.tys)?;
                    // If the index is in-bounds, go ahead.
                    (idx < len as u128).then_some(())
                })();
                if ok.is_none() {
                    return Err(Unpromotable);
                }
                self.validate_local(*local)?;
                // Recurse.
            }

            PlaceElem::Field(..) => {
                let base_ty = place_ty(ccx, &place_base);
                if matches!(*ccx.tcx.tys.kind(base_ty), TyKind::Adt(d, ..) if ccx.tcx.adts.get(&d).is_some_and(|a| a.is_union())) {
                    // No promotion of union field accesses.
                    return Err(Unpromotable);
                }
            }
        }

        self.validate_place(&place_base)
    }

    fn validate_operand(&mut self, operand: &Operand) -> Result<(), Unpromotable> {
        let ccx = self.ccx;
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.validate_place(place),

            // `RuntimeChecks` behaves different in const-eval and runtime MIR, so we do not promote it.
            Operand::RuntimeChecks(_) => Err(Unpromotable),

            // The qualifs for a constant (e.g. `HasMutInterior`) are checked in `validate_rvalue` upon
            // access.
            Operand::Constant(c) => {
                if let Some(def_id) = c.const_.check_static_ptr(&ccx.body.allocs) {
                    // Only allow statics (not consts) to refer to other statics.
                    let is_static = matches!(ccx.const_kind, Some(ConstContext::Static(_)));
                    if !is_static {
                        return Err(Unpromotable);
                    }

                    if is_thread_local_static(ccx.prog, def_id) {
                        return Err(Unpromotable);
                    }
                }

                Ok(())
            }
        }
    }

    fn validate_ref(&mut self, kind: BorrowKind, place: &Place) -> Result<(), Unpromotable> {
        let ccx = self.ccx;
        match kind {
            // Reject these borrow types just to be safe.
            BorrowKind::Fake(_) | BorrowKind::Mut { kind: MutBorrowKind::ClosureCapture } => {
                return Err(Unpromotable);
            }

            BorrowKind::Shared => {
                let has_mut_interior = self.qualif_local::<HasMutInterior>(place.local);
                if has_mut_interior {
                    return Err(Unpromotable);
                }
            }

            // FIXME: consider changing this to only promote &mut [] for default borrows, also
            // forbidding two phase borrows
            BorrowKind::Mut { kind: MutBorrowKind::Default | MutBorrowKind::TwoPhaseBorrow } => {
                let ty = place_ty(ccx, place);

                // In theory, any zero-sized value could be borrowed mutably without consequences.
                // However, only &mut [] is allowed right now.
                let TyKind::Array(_, len) = *ccx.tcx.tys.kind(ty) else { return Err(Unpromotable) };
                let Some(0) = len.try_to_target_usize(&ccx.tcx.tys) else { return Err(Unpromotable) };
            }
        }

        Ok(())
    }

    fn validate_rvalue(&mut self, rvalue: &Rvalue) -> Result<(), Unpromotable> {
        let ccx = self.ccx;
        match rvalue {
            Rvalue::Use(operand) | Rvalue::Repeat(operand, _) => {
                self.validate_operand(operand)?;
            }
            Rvalue::CopyForDeref(place) => {
                let op = &Operand::Copy(place.clone());
                self.validate_operand(op)?
            }

            Rvalue::Discriminant(place) => self.validate_place(place)?,

            Rvalue::ThreadLocalRef(_) => return Err(Unpromotable),

            // ptr-to-int casts are not possible in consts and thus not promotable
            Rvalue::Cast(CastKind::PointerExposeProvenance, _, _) => return Err(Unpromotable),

            // all other casts including int-to-ptr casts are fine, they just use the integer value at
            // pointer type.
            Rvalue::Cast(_, operand, _) => {
                self.validate_operand(operand)?;
            }

            Rvalue::UnaryOp(op, operand) => {
                match op {
                    // These operations can never fail.
                    UnOp::Neg | UnOp::Not | UnOp::PtrMetadata => {}
                }

                self.validate_operand(operand)?;
            }

            Rvalue::BinaryOp(op, operands) => {
                let (lhs, rhs) = (&operands.0, &operands.1);
                let lhs_ty = operand_ty(ccx, lhs);

                if let TyKind::Ptr(..) | TyKind::FnPtr(..) = ccx.tcx.tys.kind(lhs_ty) {
                    // Raw and fn pointer operations are not allowed inside consts and thus not
                    // promotable.
                    return Err(Unpromotable);
                }

                match op {
                    BinOp::Div | BinOp::Rem => {
                        if let TyKind::Int(prim) = *ccx.tcx.tys.kind(lhs_ty) {
                            let (bytes, signed) = ccx.tcx.tys.int_info(prim);
                            let bits = bytes * 8;
                            let mask = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
                            // Integer division: the RHS must be a non-zero const.
                            let rhs_val = match rhs {
                                Operand::Constant(rhs_c) if should_evaluate_for_promotion_checks(ccx, &rhs_c.const_) => match try_eval_scalar(ccx, &rhs_c.const_) {
                                    // for the zero test, int vs uint does not matter
                                    Some((rhs_val, _)) if rhs_val & mask != 0 => rhs_val & mask,
                                    // value not known or 0 -- not okay
                                    _ => return Err(Unpromotable),
                                },
                                _ => return Err(Unpromotable),
                            };
                            // Furthermore, for signed division, we also have to exclude `int::MIN / -1`.
                            if signed && rhs_val == mask {
                                // The RHS is -1, so we have to be careful. But is the LHS int::MIN?
                                let lhs_ok = match lhs {
                                    Operand::Constant(lhs_c) if should_evaluate_for_promotion_checks(ccx, &lhs_c.const_) => match try_eval_scalar(ccx, &lhs_c.const_) {
                                        Some((lhs_val, _)) => lhs_val & mask != 1u128 << (bits - 1),
                                        None => false,
                                    },
                                    _ => false,
                                };
                                if !lhs_ok {
                                    // value not known or int::MIN -- not okay
                                    return Err(Unpromotable);
                                }
                            }
                        }
                    }
                    // The remaining operations can never fail.
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Le
                    | BinOp::Lt
                    | BinOp::Ge
                    | BinOp::Gt
                    | BinOp::Cmp
                    | BinOp::Offset
                    | BinOp::Add
                    | BinOp::AddUnchecked
                    | BinOp::AddWithOverflow
                    | BinOp::Sub
                    | BinOp::SubUnchecked
                    | BinOp::SubWithOverflow
                    | BinOp::Mul
                    | BinOp::MulUnchecked
                    | BinOp::MulWithOverflow
                    | BinOp::BitXor
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::Shl
                    | BinOp::ShlUnchecked
                    | BinOp::Shr
                    | BinOp::ShrUnchecked => {}
                }

                self.validate_operand(lhs)?;
                self.validate_operand(rhs)?;
            }

            Rvalue::RawPtr(_, place) => {
                // We accept `&raw *`, i.e., raw reborrows -- creating a raw pointer is no problem,
                // only using it is.
                if let Some((place_base, PlaceElem::Deref)) = last_projection(place) {
                    let base_ty = place_ty(ccx, &place_base);
                    if let TyKind::Ref(..) = ccx.tcx.tys.kind(base_ty) {
                        return self.validate_place(&place_base);
                    }
                }
                return Err(Unpromotable);
            }

            Rvalue::Ref(_, kind, place) => {
                // Special-case reborrows to be more like a copy of the reference.
                let mut place_simplified = place.clone();
                if let Some((place_base, PlaceElem::Deref)) = last_projection(&place_simplified) {
                    let base_ty = place_ty(ccx, &place_base);
                    if let TyKind::Ref(..) = ccx.tcx.tys.kind(base_ty) {
                        place_simplified = place_base;
                    }
                }

                self.validate_place(&place_simplified)?;

                // Check that the reference is fine (using the original place!). (Needs to come after
                // `validate_place` to avoid ICEs.)
                self.validate_ref(*kind, place)?;
            }

            Rvalue::Aggregate(_, operands) => {
                for o in operands {
                    self.validate_operand(o)?;
                }
            }
        }

        Ok(())
    }

    /// Computes the sets of blocks of this MIR that are definitely going to be executed if the
    /// function returns successfully. That makes it safe to promote calls in them that might fail.
    fn promotion_safe_blocks(body: &Body) -> Vec<bool> {
        let mut safe_blocks = vec![false; body.basic_blocks.len()];
        let mut safe_block = 0usize;
        loop {
            if safe_blocks[safe_block] {
                break;
            }
            safe_blocks[safe_block] = true;
            // Let's see if we can find another safe block.
            safe_block = match body.basic_blocks[safe_block].terminator.as_ref().map(|t| &t.kind) {
                Some(TerminatorKind::Goto { target }) => target.0 as usize,
                Some(TerminatorKind::Call { target: Some(target), .. }) | Some(TerminatorKind::Drop { target, .. }) => {
                    // This calls a function or the destructor. `target` does not get executed if the
                    // callee loops or panics. But in both cases the const already fails to evaluate,
                    // so we are fine considering `target` a safe block for promotion.
                    target.0 as usize
                }
                Some(TerminatorKind::Assert { target, .. }) => {
                    // Similar to above, we only consider successful execution.
                    target.0 as usize
                }
                _ => {
                    // No next safe block.
                    break;
                }
            };
        }
        safe_blocks
    }

    /// Returns whether the block is "safe" for promotion, which means it cannot be dead code. We use
    /// this to avoid promoting operations that can fail in dead code.
    fn is_promotion_safe_block(&mut self, block: BasicBlock) -> bool {
        let body = self.ccx.body;
        let safe_blocks = self.promotion_safe_blocks.get_or_insert_with(|| Self::promotion_safe_blocks(body));
        safe_blocks[block.0 as usize]
    }

    fn validate_call(&mut self, callee: &Operand, args: &[Operand], block: BasicBlock) -> Result<(), Unpromotable> {
        let ccx = self.ccx;
        // Validate the operands. If they fail, there's no question -- we cannot promote.
        self.validate_operand(callee)?;
        for arg in args {
            self.validate_operand(arg)?;
        }

        // Functions marked `#[rustc_promotable]` are explicitly allowed to be promoted, so we can
        // accept them at this point.
        let fn_ty = operand_ty(ccx, callee);
        if let TyKind::FnDef(def_id, ..) = *ccx.tcx.tys.kind(fn_ty) {
            if ccx.tcx.fn_sig(ccx.prog, def_id).is_some_and(|sig| sig.rustc_promotable) {
                return Ok(());
            }
        }

        // Ideally, we'd stop here and reject the rest. But for backward compatibility, we have to
        // accept some promotion in const/static initializers. Inline consts are explicitly excluded,
        // they are more recent so we have no backwards compatibility reason to allow more promotion
        // inside of them.
        let promote_all_fn = matches!(ccx.const_kind, Some(ConstContext::Static(_) | ConstContext::Const { inline: false }));
        if !promote_all_fn {
            return Err(Unpromotable);
        }
        // Make sure the callee is a `const fn`.
        let is_const_fn = match *ccx.tcx.tys.kind(fn_ty) {
            TyKind::FnDef(def_id, ..) => {
                matches!(ccx.prog.def(def_id).kind, DefKind::Fn | DefKind::AssocFn) && ccx.tcx.fn_sig(ccx.prog, def_id).is_some_and(|sig| sig.is_const)
            }
            _ => false,
        };
        if !is_const_fn {
            return Err(Unpromotable);
        }
        // The problem is, this may promote calls to functions that panic. We don't want to introduce
        // compilation errors if there's a panic in a call in dead code. So we ensure that this is not
        // dead code.
        if !self.is_promotion_safe_block(block) {
            return Err(Unpromotable);
        }
        // This passed all checks, so let's accept.
        Ok(())
    }
}

/// Can we try to evaluate a given constant at this point in compilation? Attempting to evaluate a
/// const block before borrow-checking would be a query cycle; a `Const::Ty` is a parameter or an
/// already-valtree constant that never turns into a MIR value for promotion.
fn should_evaluate_for_promotion_checks(ccx: &ConstCx<'_>, constant: &Const) -> bool {
    match constant {
        Const::Ty(..) => false,
        Const::Val(..) => true,
        // anonymous constants include inline `const { .. }` blocks: not evaluated
        Const::Unevaluated { def, .. } => ccx.prog.def(*def).kind != DefKind::AnonConst,
    }
}

/// `try_eval_scalar_int`: the bits and byte size of a scalar constant.
fn try_eval_scalar(ccx: &ConstCx<'_>, constant: &Const) -> Option<(u128, u8)> {
    match constant {
        Const::Val(ConstValue::Scalar(Scalar::Int { bits, size }), _) => Some((*bits, *size)),
        Const::Unevaluated { def, args, promoted: None, .. } => {
            let tree = ccx.tcx.const_eval_resolve(ccx.prog, ccx.env, *def, args).ok()?;
            let leaf = tree.try_to_leaf()?;
            Some((leaf.to_bits(leaf.size()), leaf.size()))
        }
        _ => None,
    }
}

pub(super) fn validate_candidates(ccx: &ConstCx<'_>, temps: &mut [TempState], mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut validator = Validator { ccx, temps, promotion_safe_blocks: None };

    candidates.retain(|&candidate| validator.validate_candidate(candidate).is_ok());
    candidates
}

#[allow(dead_code)]
fn _unused(ccx: &ConstCx<'_>, place: &Place) -> crate::types::TyId {
    place_prefix_ty(ccx, place, 0)
}
