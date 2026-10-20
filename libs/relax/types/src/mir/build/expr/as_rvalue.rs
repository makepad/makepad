//! rustc_mir_build/src/builder/expr/as_rvalue.rs. Owner: lane E.
//! 1.97.1 forms: no `Rvalue::Len`/`NullaryOp`/`ShallowInitBox`; no retag flag on `Use`.
use super::category::Category;
use crate::mir::build::builder::{Builder, TempLifetime};
use crate::mir::build::cfg::{BlockAnd, BlockAndExtension};
use crate::mir::build::place::PlaceBase;
use crate::mir::build::thir::{ClosureExpr, ClosureExprKind, ExprId, ExprKind};
use crate::mir::build::{BuildResult, UnsupportedReason};
use crate::mir::{
    AggregateKind, AssertKind, BasicBlock, BinOp, BorrowKind, CastKind, CoercionSource, Const, ConstOperand, ConstValue, FieldIdx,
    LocalInfo, LocalKind, MutBorrowKind, Mutability, Operand, Place, PlaceElem, Region, Rvalue, Scalar, SourceInfo, Span,
    TerminatorKind, UnOp, UnwindAction,
};
use crate::program::Prim;
use crate::region_scope::ScopeId;
use crate::types::{TyId, TyKind, Types};

impl Builder<'_> {
    /// as_rvalue.rs:`as_local_rvalue`: an rvalue suitable for use until the end of the current scope
    /// expression (not valid after an `ExprKind::Scope` has ended).
    pub fn as_local_rvalue(&mut self, block: BasicBlock, expr: ExprId) -> BuildResult<BlockAnd<Rvalue>> {
        let local_scope = self.local_scope();
        self.as_rvalue(block, TempLifetime { temp_lifetime: Some(local_scope), backwards_incompatible: None }, expr)
    }

    /// as_rvalue.rs:`as_rvalue`. Compile `expr`, yielding an rvalue.
    pub fn as_rvalue(&mut self, mut block: BasicBlock, scope: TempLifetime, expr_id: ExprId) -> BuildResult<BlockAnd<Rvalue>> {
        let expr = &self.thir.thir[expr_id];
        let (expr_span, expr_ty, kind) = (expr.span, expr.ty, expr.kind.clone());
        let source_info = self.source_info(expr_span);
        let tcx = self.root.cx.tcx;

        match kind {
            ExprKind::ThreadLocalRef(did) => Ok(block.and(Rvalue::ThreadLocalRef(did))),
            ExprKind::Scope { region_scope, value, .. } => {
                self.in_scope((region_scope, source_info), |this| this.as_rvalue(block, scope, value))
            }
            ExprKind::Repeat { value, count } => {
                if self.const_usize(count) == Some(0) {
                    self.build_zero_repeat(block, value, scope, source_info)
                } else {
                    let BlockAnd(b, value_operand) = self.as_operand(block, scope, value, LocalInfo::Boring, false)?;
                    block = b;
                    // The count as a scalar when it is known (the checker's array-length reader takes values and
                    // unevaluated constants); a generic count stays a type-level constant.
                    let count = match self.const_usize(count) {
                        Some(n) => Const::Val(ConstValue::Scalar(Scalar::Int { bits: n, size: 8 }), tcx.tys.usize_),
                        None => Const::Ty(tcx.tys.usize_, count),
                    };
                    Ok(block.and(Rvalue::Repeat(value_operand, count)))
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let BlockAnd(b, lhs) = self.as_operand(block, scope, lhs, LocalInfo::Boring, true)?;
                block = b;
                let BlockAnd(b, rhs) = self.as_operand(block, scope, rhs, LocalInfo::Boring, false)?;
                block = b;
                self.build_binary_op(block, op, expr_span, expr_ty, lhs, rhs)
            }
            ExprKind::Unary { op, arg } => {
                let BlockAnd(b, arg) = self.as_operand(block, scope, arg, LocalInfo::Boring, false)?;
                block = b;
                // Check for -MIN on signed integers
                if self.check_overflow() && op == UnOp::Neg && self.is_signed(expr_ty) {
                    let bool_ty = tcx.tys.bool_;
                    let minval = self.minval_literal(expr_span, expr_ty);
                    let is_min = self.temp(bool_ty, expr_span);
                    self.cfg.push_assign(block, source_info, is_min.clone(), Rvalue::BinaryOp(BinOp::Eq, Box::new((to_copy(&arg), minval))));
                    block = self.assert(block, Operand::Move(is_min), false, AssertKind::OverflowNeg(to_copy(&arg)), expr_span)?;
                }
                Ok(block.and(Rvalue::UnaryOp(op, arg)))
            }
            ExprKind::Cast { source } => {
                let source_expr = &self.thir.thir[source];
                let (source_ty, source_span) = (source_expr.ty, source_expr.span);
                // Casting an enum to an integer is equivalent to computing the discriminant and casting
                // the discriminant: create all the steps directly in MIR.
                let enum_def = match tcx.tys.kind(source_ty) {
                    TyKind::Adt(d, ..) if tcx.adts.get(d).is_some_and(|a| a.is_enum) => Some(*d),
                    _ => None,
                };
                let (source, ty) = if let Some(d) = enum_def {
                    let discr_ty = tcx.tys.prim(tcx.adts[&d].repr_int.unwrap_or(Prim::Isize));
                    let BlockAnd(b, temp) = self.as_temp(block, scope, source, Mutability::Not)?;
                    block = b;
                    let discr = self.temp(discr_ty, source_span);
                    self.cfg.push_assign(block, source_info, discr.clone(), Rvalue::Discriminant(Place::local(temp)));
                    (Operand::Move(discr), discr_ty)
                } else {
                    let BlockAnd(b, source) = self.as_operand(block, scope, source, LocalInfo::Boring, false)?;
                    block = b;
                    (source, source_ty)
                };
                // A fn item or capture-free closure cast to a pointer or integer is reified to its fn
                // pointer first (rustc cast.rs: the source is coerced to a fn pointer, then cast).
                let (source, ty) = match tcx.tys.kind(ty) {
                    TyKind::FnDef(..) | TyKind::Closure(..) => {
                        let fn_ptr = self.fn_ptr_of(ty, expr_span)?;
                        let safety = match tcx.tys.kind(fn_ptr) { TyKind::FnPtr(_, _, header, _) => header.safety, _ => crate::types::Safety::Safe };
                        let cast = if matches!(tcx.tys.kind(ty), TyKind::Closure(..)) { crate::mir::PointerCoercion::ClosureFnPointer(safety) } else { crate::mir::PointerCoercion::ReifyFnPointer(safety) };
                        let temp = self.temp(fn_ptr, expr_span);
                        self.cfg.push_assign(block, source_info, temp.clone(), Rvalue::Cast(CastKind::PointerCoercion(cast, CoercionSource::AsCast), source, fn_ptr));
                        (Operand::Move(temp), fn_ptr)
                    }
                    _ => (source, ty),
                };
                let cast_kind = self.mir_cast_kind(ty, expr_ty, expr_span)?;
                Ok(block.and(Rvalue::Cast(cast_kind, source, expr_ty)))
            }
            ExprKind::PointerCoercion { cast, source, is_from_as_cast } => {
                let BlockAnd(b, source) = self.as_operand(block, scope, source, LocalInfo::Boring, false)?;
                block = b;
                let origin = if is_from_as_cast { CoercionSource::AsCast } else { CoercionSource::Implicit };
                Ok(block.and(Rvalue::Cast(CastKind::PointerCoercion(cast, origin), source, expr_ty)))
            }
            ExprKind::Array { fields } => {
                // (*) rustc would be closer to codegen handling this via `into`, but that needs a
                // more complex partial-cleanup mechanism and distinguishes types with destructors.
                let el_ty = match tcx.tys.kind(expr_ty) {
                    TyKind::Array(e, _) | TyKind::Slice(e) => *e,
                    _ => return Err(self.error(expr_span, UnsupportedReason::UnresolvedFact("array expression of a non-array type"))),
                };
                let mut operands = Vec::with_capacity(fields.len());
                for &f in fields.iter() {
                    let BlockAnd(b, op) = self.as_operand(block, scope, f, LocalInfo::Boring, true)?;
                    block = b;
                    operands.push(op);
                }
                Ok(block.and(Rvalue::Aggregate(Box::new(AggregateKind::Array(el_ty)), operands)))
            }
            ExprKind::Tuple { fields } => {
                // see (*) above
                let mut operands = Vec::with_capacity(fields.len());
                for &f in fields.iter() {
                    let BlockAnd(b, op) = self.as_operand(block, scope, f, LocalInfo::Boring, true)?;
                    block = b;
                    operands.push(op);
                }
                Ok(block.and(Rvalue::Aggregate(Box::new(AggregateKind::Tuple), operands)))
            }
            ExprKind::Closure(closure) => {
                let ClosureExpr { closure: closure_id, args, upvars, fake_reads, kind: closure_kind, .. } = *closure;
                // Convert the closure fake reads, if any, from THIR places to MIR places and push them.
                // This must come before creating the operands: a fake read and a borrow of the same
                // path must not interfere (`&mut x; match x { _ => () }` inside the closure).
                for (thir_place, cause, _) in fake_reads {
                    let BlockAnd(b, place_builder) = self.as_place_builder(block, thir_place)?;
                    block = b;
                    if let Some(mir_place) = place_builder.try_to_place(self) {
                        let span = self.thir.thir[thir_place].span;
                        let si = self.source_info(span);
                        self.cfg.push_fake_read(block, si, cause, mir_place);
                    }
                }

                // see (*) above
                let mut operands = Vec::with_capacity(upvars.len());
                for &upvar in upvars.iter() {
                    let upvar_expr = &self.thir.thir[upvar];
                    let (upvar_span, upvar_ty, upvar_kind) = (upvar_expr.span, upvar_expr.ty, upvar_expr.kind.clone());
                    let operand = match Category::of(&upvar_kind) {
                        // Use as_place to avoid creating a temporary when moving a variable into a
                        // closure, so that borrowck knows which variables to mark as being used as
                        // mut. This is OK here because the upvar expressions have no side effects and
                        // act on disjoint places. This occurs when capturing by copy/move, while by
                        // reference captures use as_operand.
                        Some(Category::Place) => {
                            let BlockAnd(b, place) = self.as_place(block, upvar)?;
                            block = b;
                            self.capture_operand(place)?
                        }
                        // Turn mutable borrow captures into unique borrow captures when capturing an
                        // immutable variable. This is sound because the mutation that caused the
                        // capture will cause an error.
                        _ => match upvar_kind {
                            ExprKind::Borrow { borrow_kind: BorrowKind::Mut { kind: MutBorrowKind::Default }, arg } => {
                                let BlockAnd(b, op) = self.limit_capture_mutability(upvar_span, upvar_ty, scope.temp_lifetime, block, arg)?;
                                block = b;
                                op
                            }
                            _ => {
                                let BlockAnd(b, op) = self.as_operand(block, scope, upvar, LocalInfo::Boring, true)?;
                                block = b;
                                op
                            }
                        },
                    };
                    operands.push(operand);
                }

                let result = match closure_kind {
                    ClosureExprKind::Coroutine => AggregateKind::Coroutine(closure_id, args),
                    ClosureExprKind::Closure => AggregateKind::Closure(closure_id, args),
                    ClosureExprKind::CoroutineClosure => AggregateKind::CoroutineClosure(closure_id, args),
                };
                Ok(block.and(Rvalue::Aggregate(Box::new(result), operands)))
            }
            ExprKind::Assign { .. } | ExprKind::AssignOp { .. } => {
                let BlockAnd(b, ()) = self.stmt_expr(block, expr_id, None)?;
                block = b;
                Ok(block.and(Rvalue::Use(Operand::Constant(Box::new(ConstOperand {
                    span: expr_span,
                    user_ty: None,
                    const_: Const::Val(ConstValue::ZeroSized, tcx.tys.unit),
                })))))
            }

            ExprKind::Literal { .. }
            | ExprKind::NamedConst { .. }
            | ExprKind::NonHirLiteral { .. }
            | ExprKind::ZstLiteral { .. }
            | ExprKind::ConstParam { .. }
            | ExprKind::ConstBlock { .. }
            | ExprKind::StaticRef { .. } => {
                let constant = self.as_constant(expr_id)?;
                Ok(block.and(Rvalue::Use(Operand::Constant(Box::new(constant)))))
            }

            ExprKind::Yield { .. }
            | ExprKind::Unreachable
            | ExprKind::Block { .. }
            | ExprKind::Match { .. }
            | ExprKind::If { .. }
            | ExprKind::NeverToAny { .. }
            | ExprKind::Use { .. }
            | ExprKind::Borrow { .. }
            | ExprKind::RawBorrow { .. }
            | ExprKind::Adt(..)
            | ExprKind::Loop { .. }
            | ExprKind::LogicalOp { .. }
            | ExprKind::Call { .. }
            | ExprKind::Field { .. }
            | ExprKind::Let { .. }
            | ExprKind::Deref { .. }
            | ExprKind::Index { .. }
            | ExprKind::VarRef { .. }
            | ExprKind::UpvarRef { .. }
            | ExprKind::Break { .. }
            | ExprKind::Continue { .. }
            | ExprKind::Return { .. }
            | ExprKind::PlaceTypeAscription { .. }
            | ExprKind::ValueTypeAscription { .. } => {
                // these do not have corresponding `Rvalue` variants, so make an operand and then return that
                let BlockAnd(b, operand) = self.as_operand(block, scope, expr_id, LocalInfo::Boring, false)?;
                block = b;
                Ok(block.and(Rvalue::Use(operand)))
            }

            ExprKind::ByUse { expr, span: _ } => {
                let BlockAnd(b, operand) = self.as_operand(block, scope, expr, LocalInfo::Boring, false)?;
                block = b;
                Ok(block.and(Rvalue::Use(operand)))
            }
        }
    }

    /// as_rvalue.rs:`build_binary_op`: arithmetic with the overflow / zero-division / shift checks
    /// rustc's debug builds emit.
    pub fn build_binary_op(&mut self, mut block: BasicBlock, op: BinOp, span: Span, ty: TyId, lhs: Operand, rhs: Operand) -> BuildResult<BlockAnd<Rvalue>> {
        let source_info = self.source_info(span);
        let tcx = self.root.cx.tcx;
        let bool_ty = tcx.tys.bool_;
        let integral = matches!(tcx.tys.kind(ty), TyKind::Int(_));
        let rvalue = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul if self.check_overflow() && integral => {
                let result_tup = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[ty, bool_ty])));
                let result_value = self.temp(result_tup, span);
                let op_with_overflow = match op {
                    BinOp::Add => BinOp::AddWithOverflow,
                    BinOp::Sub => BinOp::SubWithOverflow,
                    _ => BinOp::MulWithOverflow,
                };
                self.cfg.push_assign(block, source_info, result_value.clone(), Rvalue::BinaryOp(op_with_overflow, Box::new((to_copy(&lhs), to_copy(&rhs)))));
                let mut val = result_value.clone();
                val.projection.push(PlaceElem::Field(FieldIdx(0), ty));
                let mut of = result_value;
                of.projection.push(PlaceElem::Field(FieldIdx(1), bool_ty));
                let err = AssertKind::Overflow(op, lhs, rhs);
                block = self.assert(block, Operand::Move(of), false, err, span)?;
                Rvalue::Use(Operand::Move(val))
            }
            BinOp::Shl | BinOp::Shr if self.check_overflow() && integral => {
                // For an unsigned RHS, the shift is in-range for `rhs < bits`. For a signed RHS, cast to
                // the equivalent unsigned type and do the same comparison: a negative value is *at
                // least* 128 after the cast (that's i8::MIN), an overflowing amount for every type.
                let lhs_size = self.int_size(ty, span)?;
                let mut rhs = rhs;
                let mut rhs_ty = self.operand_ty(&rhs);
                // `1 << &n`: the by-reference impls (`Shl<&T> for T`) read the amount through the reference.
                // rustc keeps an overloaded call; here the amount is copied out of the reference.
                if let (TyKind::Ref(_, inner, _), Operand::Copy(p) | Operand::Move(p)) = (tcx.tys.kind(rhs_ty).clone(), &rhs) {
                    let mut q = p.clone();
                    q.projection.push(PlaceElem::Deref);
                    let amount = self.temp(inner, span);
                    self.cfg.push_assign(block, source_info, amount.clone(), Rvalue::Use(Operand::Copy(q)));
                    rhs = Operand::Move(amount);
                    rhs_ty = inner;
                }
                let rhs_prim = match tcx.tys.kind(rhs_ty) {
                    TyKind::Int(p) => *p,
                    _ => return Err(self.error(span, UnsupportedReason::UnresolvedFact("shift by a non-integer"))),
                };
                let (rhs_size, rhs_signed) = Types::int_info(rhs_prim);
                let (unsigned_rhs, unsigned_ty) = if !rhs_signed {
                    (to_copy(&rhs), rhs_ty)
                } else {
                    let uint_ty = tcx.tys.prim(to_unsigned(rhs_prim));
                    let rhs_temp = self.temp(uint_ty, span);
                    self.cfg.push_assign(block, source_info, rhs_temp.clone(), Rvalue::Cast(CastKind::IntToInt, to_copy(&rhs), uint_ty));
                    (Operand::Move(rhs_temp), uint_ty)
                };
                // This can't overflow because the largest shiftable types are 128-bit, which fits in `u8`.
                let lhs_bits = const_from_scalar(unsigned_ty, (lhs_size * 8) as u128, rhs_size as u8, span);
                let inbounds = self.temp(bool_ty, span);
                self.cfg.push_assign(block, source_info, inbounds.clone(), Rvalue::BinaryOp(BinOp::Lt, Box::new((unsigned_rhs, lhs_bits))));
                let overflow_err = AssertKind::Overflow(op, to_copy(&lhs), to_copy(&rhs));
                block = self.assert(block, Operand::Move(inbounds), true, overflow_err, span)?;
                Rvalue::BinaryOp(op, Box::new((lhs, rhs)))
            }
            BinOp::Div | BinOp::Rem if integral => {
                // Checking division and remainder is more complex, since we always check and there are
                // two possible failure cases, divide-by-zero and overflow.
                let zero_err = if op == BinOp::Div { AssertKind::DivisionByZero(to_copy(&lhs)) } else { AssertKind::RemainderByZero(to_copy(&lhs)) };
                let overflow_err = AssertKind::Overflow(op, to_copy(&lhs), to_copy(&rhs));

                // Check for / 0
                let is_zero = self.temp(bool_ty, span);
                let zero = self.zero_literal(span, ty)?;
                self.cfg.push_assign(block, source_info, is_zero.clone(), Rvalue::BinaryOp(BinOp::Eq, Box::new((to_copy(&rhs), zero))));
                block = self.assert(block, Operand::Move(is_zero), false, zero_err, span)?;

                // We only need to check for the overflow in one case: MIN / -1, and only for signed values.
                if self.is_signed(ty) {
                    let neg_1 = self.neg_1_literal(span, ty)?;
                    let min = self.minval_literal(span, ty);
                    let is_neg_1 = self.temp(bool_ty, span);
                    let is_min = self.temp(bool_ty, span);
                    let of = self.temp(bool_ty, span);
                    // this does (rhs == -1) & (lhs == MIN). It could short-circuit instead
                    self.cfg.push_assign(block, source_info, is_neg_1.clone(), Rvalue::BinaryOp(BinOp::Eq, Box::new((to_copy(&rhs), neg_1))));
                    self.cfg.push_assign(block, source_info, is_min.clone(), Rvalue::BinaryOp(BinOp::Eq, Box::new((to_copy(&lhs), min))));
                    self.cfg.push_assign(block, source_info, of.clone(), Rvalue::BinaryOp(BinOp::BitAnd, Box::new((Operand::Move(is_neg_1), Operand::Move(is_min)))));
                    block = self.assert(block, Operand::Move(of), false, overflow_err, span)?;
                }
                Rvalue::BinaryOp(op, Box::new((lhs, rhs)))
            }
            _ => Rvalue::BinaryOp(op, Box::new((lhs, rhs))),
        };
        Ok(block.and(rvalue))
    }

    /// Recursively inspect a THIR expression and probe through unsizing operations that can be
    /// const-folded today.
    fn check_constness(&self, mut id: ExprId) -> bool {
        loop {
            let e = &self.thir.thir[id];
            match &e.kind {
                ExprKind::ValueTypeAscription { source, .. }
                | ExprKind::Use { source }
                | ExprKind::PointerCoercion { cast: crate::mir::PointerCoercion::Unsize, source, .. }
                | ExprKind::Scope { value: source, .. } => id = *source,
                kind => return matches!(Category::of(kind), Some(Category::Constant)),
            }
        }
    }

    fn build_zero_repeat(&mut self, mut block: BasicBlock, value: ExprId, scope: TempLifetime, outer_source_info: SourceInfo) -> BuildResult<BlockAnd<Rvalue>> {
        let elem_ty = self.thir.thir[value].ty;
        if self.check_constness(value) {
            // Repeating a const does nothing
        } else {
            // For a non-const, we may need to generate an appropriate `Drop`
            let BlockAnd(b, value_operand) = self.as_operand(block, scope, value, LocalInfo::Boring, false)?;
            block = b;
            if let Operand::Move(to_drop) = value_operand {
                let success = self.cfg.start_new_block();
                self.cfg.terminate(block, outer_source_info, TerminatorKind::Drop { place: to_drop, target: success, unwind: UnwindAction::Continue, replace: false });
                self.diverge_from(block)?;
                block = success;
            }
        }
        Ok(block.and(Rvalue::Aggregate(Box::new(AggregateKind::Array(elem_ty)), Vec::new())))
    }

    fn limit_capture_mutability(&mut self, upvar_span: Span, upvar_ty: TyId, temp_lifetime: Option<ScopeId>, mut block: BasicBlock, arg: ExprId) -> BuildResult<BlockAnd<Operand>> {
        let source_info = self.source_info(upvar_span);
        let temp = self.new_local(upvar_ty, Mutability::Mut, LocalKind::Temp, LocalInfo::Boring, upvar_span);
        self.cfg.push_storage_live(block, source_info, temp);

        let BlockAnd(b, arg_place_builder) = self.as_place_builder(block, arg)?;
        block = b;

        let mutability = match arg_place_builder.base() {
            // We are capturing a path that starts off a local variable in the parent. The mutability
            // of the current capture is the mutability of the local declaration in the parent.
            PlaceBase::Local(local) => self.local_decls[local.0 as usize].mutability,
            // Parent is a closure and we are capturing a path that is captured by the parent itself.
            // The mutability of the current capture is that of the capture in the parent closure.
            PlaceBase::Upvar { .. } => {
                let resolved = arg_place_builder.to_place(self);
                match resolved.projection.as_slice() {
                    [PlaceElem::Field(idx, _), ..] | [PlaceElem::Deref, PlaceElem::Field(idx, _), ..] => match self.upvars.get(idx.0 as usize) {
                        Some(upvar) => upvar.mutability,
                        None => return Err(self.error(upvar_span, UnsupportedReason::UnresolvedFact("capture of an unknown upvar"))),
                    },
                    _ => return Err(self.error(upvar_span, UnsupportedReason::UnresolvedFact("unexpected capture place"))),
                }
            }
        };

        let borrow_kind = match mutability {
            Mutability::Not => BorrowKind::Mut { kind: MutBorrowKind::ClosureCapture },
            Mutability::Mut => BorrowKind::Mut { kind: MutBorrowKind::Default },
        };
        let arg_place = arg_place_builder.to_place(self);
        self.cfg.push_assign(block, source_info, Place::local(temp), Rvalue::Ref(Region::ERASED, borrow_kind, arg_place));

        // This can be `None` if the expression's temporary scope was extended so that it can be
        // borrowed by a `const` or `static`. In that case, it's never dropped.
        if let Some(temp_lifetime) = temp_lifetime {
            self.schedule_drop_storage_and_value(upvar_span, temp_lifetime, temp)?;
        }
        Ok(block.and(Operand::Move(Place::local(temp))))
    }

    // ---- small helpers over the type tables ----

    fn const_usize(&self, count: crate::consts::Const) -> Option<u128> {
        match self.root.cx.tcx.tys.const_kind(count) {
            crate::consts::ConstKind::Value(v) => v.valtree.try_to_leaf().map(|s| s.to_bits(s.size())),
            _ => None,
        }
    }
    fn is_signed(&self, ty: TyId) -> bool {
        matches!(self.root.cx.tcx.tys.kind(ty), TyKind::Int(p) if Types::int_info(*p).1)
    }
    fn int_size(&self, ty: TyId, span: Span) -> BuildResult<u32> {
        match self.root.cx.tcx.tys.kind(ty) {
            TyKind::Int(p) => Ok(Types::int_info(*p).0),
            _ => Err(self.error(span, UnsupportedReason::UnresolvedFact("integer type expected"))),
        }
    }
    /// The type of an operand (rustc `Operand::ty`).
    fn operand_ty(&self, op: &Operand) -> TyId {
        let tcx = self.root.cx.tcx;
        match op {
            Operand::Constant(c) => c.const_.ty(&tcx.tys),
            Operand::Copy(p) | Operand::Move(p) => {
                p.projection.iter().fold(crate::mir::PlaceTy::from_ty(self.local_decls[p.local.0 as usize].ty), |ty, e| ty.projection_ty(tcx, e).expect("invalid place")).ty
            }
            Operand::RuntimeChecks(_) => tcx.tys.bool_,
        }
    }
    /// misc.rs:`consume_by_copy_or_move` (lane C's is module-private; same rule).
    pub(super) fn capture_operand(&self, place: Place) -> BuildResult<Operand> {
        let ty = self.operand_ty(&Operand::Copy(place.clone()));
        let cx = &self.root.cx;
        Ok(if cx.tcx.type_is_copy_modulo_regions(cx.prog, cx.eval, self.owner.owner, ty) { Operand::Copy(place) } else { Operand::Move(place) })
    }
    fn int_literal(&self, span: Span, ty: TyId, bits: u128) -> Operand {
        let size = match self.root.cx.tcx.tys.kind(ty) { TyKind::Int(p) => Types::int_info(*p).0 as u8, _ => 1 };
        const_from_scalar(ty, bits, size, span)
    }
    /// A `0` value of the integer type.
    fn zero_literal(&self, span: Span, ty: TyId) -> BuildResult<Operand> {
        self.int_size(ty, span)?;
        Ok(self.int_literal(span, ty, 0))
    }
    /// A `-1` value of the integer type (all bits set).
    fn neg_1_literal(&self, span: Span, ty: TyId) -> BuildResult<Operand> {
        let size = self.int_size(ty, span)?;
        let bits = if size == 16 { u128::MAX } else { (1u128 << (size * 8)) - 1 };
        Ok(self.int_literal(span, ty, bits))
    }
    /// The minimum value of the signed integer type.
    fn minval_literal(&self, span: Span, ty: TyId) -> Operand {
        let size = match self.root.cx.tcx.tys.kind(ty) { TyKind::Int(p) => Types::int_info(*p).0, _ => 4 };
        self.int_literal(span, ty, 1u128 << (size * 8 - 1))
    }

    /// The fn pointer type of a fn item / capture-free closure (typeck `fn_ptr_of`).
    fn fn_ptr_of(&self, t: TyId, span: Span) -> BuildResult<TyId> {
        let tcx = self.root.cx.tcx;
        let prog = self.root.cx.prog;
        match tcx.tys.kind(t).clone() {
            TyKind::FnDef(d, args, regions) => {
                let Some(sig) = tcx.fn_item_sig(prog, d).cloned() else { return Err(self.error(span, UnsupportedReason::UnresolvedFact("fn item without a signature"))) };
                let poly = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&sig.params), sig.ret, sig.header, sig.bound_regions));
                let args = tcx.complete_args(prog, d, tcx.tys.list(args), regions);
                Ok(tcx.instantiate_ty(prog, d, &args, poly))
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => Ok(sig),
            _ => Err(self.error(span, UnsupportedReason::UnresolvedFact("not a fn item"))),
        }
    }

    /// ty/cast.rs:`mir_cast_kind`.
    fn mir_cast_kind(&self, from_ty: TyId, cast_ty: TyId, span: Span) -> BuildResult<CastKind> {
        #[derive(PartialEq)]
        enum Ct { Int, Float, FnPtr, Ptr }
        let tcx = self.root.cx.tcx;
        let class = |t: TyId| match tcx.tys.kind(t) {
            TyKind::Bool | TyKind::Char | TyKind::Int(_) => Some(Ct::Int),
            TyKind::Float(_) => Some(Ct::Float),
            TyKind::Adt(d, ..) if tcx.adts.get(d).is_some_and(|a| a.is_enum && a.variants.iter().all(|v| v.fields.is_empty())) => Some(Ct::Int),
            TyKind::Ptr(..) => Some(Ct::Ptr),
            TyKind::FnPtr(..) => Some(Ct::FnPtr),
            _ => None,
        };
        Ok(match (class(from_ty), class(cast_ty)) {
            (Some(Ct::Ptr | Ct::FnPtr), Some(Ct::Int)) => CastKind::PointerExposeProvenance,
            (Some(Ct::Int), Some(Ct::Ptr)) => CastKind::PointerWithExposedProvenance,
            (Some(Ct::Int), Some(Ct::Int)) => CastKind::IntToInt,
            (Some(Ct::FnPtr), Some(Ct::Ptr)) => CastKind::FnPtrToPtr,
            (Some(Ct::Float), Some(Ct::Int)) => CastKind::FloatToInt,
            (Some(Ct::Int), Some(Ct::Float)) => CastKind::IntToFloat,
            (Some(Ct::Float), Some(Ct::Float)) => CastKind::FloatToFloat,
            (Some(Ct::Ptr), Some(Ct::Ptr)) => CastKind::PtrToPtr,
            // Neither side is a primitive cast type: the cast is the unsizing coercion typeck found
            // (`Box::new(x) as Box<dyn Tr>`) whose recorded target differs from the written one only
            // in erased regions, so the THIR builder did not take it for a plain coercion.
            (None, None) => CastKind::PointerCoercion(crate::mir::PointerCoercion::Unsize, CoercionSource::AsCast),
            _ => return Err(self.error(span, UnsupportedReason::UnresolvedFact("cast between non-castable types"))),
        })
    }
}

fn to_copy(op: &Operand) -> Operand {
    match op {
        Operand::Copy(p) | Operand::Move(p) => Operand::Copy(p.clone()),
        c => c.clone(),
    }
}
fn to_unsigned(p: Prim) -> Prim {
    match p {
        Prim::I8 => Prim::U8,
        Prim::I16 => Prim::U16,
        Prim::I32 => Prim::U32,
        Prim::I64 => Prim::U64,
        Prim::I128 => Prim::U128,
        Prim::Isize => Prim::Usize,
        other => other,
    }
}
/// rustc `Operand::const_from_scalar`.
fn const_from_scalar(ty: TyId, bits: u128, size: u8, span: Span) -> Operand {
    Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_: Const::Val(ConstValue::Scalar(Scalar::Int { bits, size }), ty) }))
}

impl Builder<'_> {
    /// `tcx.sess.overflow_checks()`: overflow checks are on in checked (dev) builds.
    pub fn check_overflow(&self) -> bool {
        true
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::ast::{ExprId as AstExprId, ExprKind as AstKind};
    use crate::borrowck::body::{BodyKind as OwnerKind, CheckedBody};
    use crate::mir::build::scope::{Scope, Scopes};
    use crate::mir::build::thir::{Expr, Thir, ThirBody};
    use crate::mir::build::{BuildCx, BuildRoot};
    use crate::mir::{BodyKind, MirSource, SourceScope};
    use crate::program::Program;
    use crate::tcx::{EvalCx, EvalState, Tcx, TypingEnv};

    const SRC: &str = "struct P { a: i32, b: i32 }\nstatic S: i32 = 1;\nconst C: i32 = 2;\nfn f() { let _ = (42, 7i64, 3u8, true, 'x', 2.5f32, \"hi\", b\"ab\", c\"c\"); }\n";

    pub(crate) struct Fx { pub thir: Thir, pub lits: Vec<(String, AstExprId)>, pub builder_parts: (&'static Program, &'static Tcx, &'static BuildRoot<'static>, CheckedBody) }
    pub(crate) fn fixture() -> Fx {
        let session = std::sync::Arc::new(crate::Session::default());
        let path = std::env::temp_dir().join(format!("relax-asrv-{}-{:?}.rs", std::process::id(), std::thread::current().id()));
        std::fs::write(&path, SRC).unwrap();
        let mut prog = Program::new(&session);
        prog.load_crate("custom", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
        std::fs::remove_file(path).unwrap();
        prog.resolve_imports();
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        let prog: &'static Program = Box::leak(Box::new(prog));
        let tcx: &'static Tcx = Box::leak(Box::new(tcx));
        let f = (0..prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| prog.name(d) == "f").unwrap();
        let typeck: &'static crate::typeck::Body = Box::leak(Box::new((*tcx.typeck(prog, f)).clone()));
        let state: &'static EvalState = Box::leak(Box::new(EvalState::new()));
        let env = TypingEnv::fully_monomorphized(prog);
        let eval: &'static EvalCx<'static> = Box::leak(Box::new(EvalCx::new(prog, tcx, env, state)));
        let root: &'static BuildRoot<'static> = Box::leak(Box::new(BuildRoot::new(BuildCx { prog, tcx, env, eval }, typeck).unwrap()));
        let pf = &prog.files[typeck.file as usize];
        let mut lits = Vec::new();
        for i in 0..pf.ast.exprs.len() {
            let id = AstExprId(i as u32);
            let e = pf.ast.expr(id);
            if let AstKind::Lit(_) = e.kind { lits.push((String::from_utf8_lossy(pf.bytes(e.lo, e.hi)).into_owned(), id)); }
        }
        Fx { thir: Thir::default(), lits, builder_parts: (prog, tcx, root, CheckedBody { owner: f, kind: OwnerKind::Function }) }
    }
    pub(crate) fn lit(fx: &mut Fx, text: &str, ty: TyId, neg: bool) -> ExprId {
        let ast = fx.lits.iter().find(|(t, _)| t == text).unwrap_or_else(|| panic!("no literal {text}")).1;
        fx.thir.alloc_expr(Expr { kind: ExprKind::Literal { expr: ast, neg }, ty, temp_scope: None, bid: None, span: Span::default() })
    }
    pub(crate) fn builder(fx: Fx) -> (Builder<'static>, &'static Tcx) {
        let (_, tcx, root, owner) = fx.builder_parts;
        let thir: &'static ThirBody = Box::leak(Box::new(ThirBody::for_test(fx.thir, Vec::new(), ExprId(0), tcx.tys.unit)));
        let mut scopes = Scopes::default();
        scopes.scopes.push(Scope::new(SourceScope(0), ScopeId(0)));
        let b = Builder {
            root, thir, owner, source: MirSource { owner: owner.owner, kind: BodyKind::Fn }, body_span: Span::default(), arg_count: 0, coroutine: None,
            cfg: Default::default(), scopes, source_scopes: vec![crate::mir::SourceScopeData { span: Span::default(), parent_scope: None }],
            source_scope: SourceScope(0), guard_context: Vec::new(), fixed_temps: Default::default(), fixed_temps_scope: None, var_indices: Default::default(),
            local_decls: Vec::new(), canonical_user_type_annotations: Vec::new(), upvars: Vec::new(), upvar_env: None, var_debug_info: Vec::new(),
            unit_temp: None, allocs: Default::default(), block_context: Vec::new(),
        };
        (b, tcx)
    }
    fn scalar(c: &ConstOperand) -> (u128, u8) {
        match &c.const_ { Const::Val(ConstValue::Scalar(Scalar::Int { bits, size }), _) => (*bits, *size), other => panic!("not a scalar: {other:?}") }
    }

    #[test]
    fn literals_become_typed_scalars_and_allocations() {
        let mut fx = fixture();
        let tcx = fx.builder_parts.1;
        let t = &tcx.tys;
        let ids = [
            lit(&mut fx, "42", t.i32_, false), lit(&mut fx, "42", t.i32_, true), lit(&mut fx, "7i64", t.i64_, false), lit(&mut fx, "3u8", t.u8_, false),
            lit(&mut fx, "true", t.bool_, false), lit(&mut fx, "'x'", t.char_, false), lit(&mut fx, "2.5f32", t.prim(Prim::F32), false),
            lit(&mut fx, "2.5f32", t.prim(Prim::F32), true), lit(&mut fx, "\"hi\"", t.str_ref, false),
        ];
        let (mut b, _) = builder(fx);
        assert_eq!(scalar(&b.as_constant(ids[0]).unwrap()), (42, 4));
        assert_eq!(scalar(&b.as_constant(ids[1]).unwrap()), (0xFFFF_FFD6, 4), "-42 truncated to i32");
        assert_eq!(scalar(&b.as_constant(ids[2]).unwrap()), (7, 8));
        assert_eq!(scalar(&b.as_constant(ids[3]).unwrap()), (3, 1));
        assert_eq!(scalar(&b.as_constant(ids[4]).unwrap()), (1, 1));
        assert_eq!(scalar(&b.as_constant(ids[5]).unwrap()), ('x' as u128, 4));
        assert_eq!(scalar(&b.as_constant(ids[6]).unwrap()), (2.5f32.to_bits() as u128, 4));
        assert_eq!(scalar(&b.as_constant(ids[7]).unwrap()), ((-2.5f32).to_bits() as u128, 4));
        let s = b.as_constant(ids[8]).unwrap();
        let Const::Val(ConstValue::Slice { alloc, meta }, _) = s.const_ else { panic!("str is a slice constant") };
        assert_eq!(meta, 2);
        assert_eq!(b.allocs.get(alloc).unwrap().bytes, b"hi");
    }

    #[test]
    fn statics_and_scalars_share_identity() {
        let mut fx = fixture();
        let (prog, tcx, _, _) = fx.builder_parts;
        let s = (0..prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| prog.name(d) == "S").unwrap();
        let c = (0..prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| prog.name(d) == "C").unwrap();
        let sr = fx.thir.alloc_expr(Expr { kind: ExprKind::StaticRef { def: s, ty: tcx.tys.i32_ }, ty: tcx.tys.i32_, temp_scope: None, bid: None, span: Span::default() });
        let nc = fx.thir.alloc_expr(Expr { kind: ExprKind::NamedConst { def: c, args: Vec::new(), user_ty: None }, ty: tcx.tys.i32_, temp_scope: None, bid: None, span: Span::default() });
        let (mut b, _) = builder(fx);
        let first = b.as_constant(sr).unwrap();
        let second = b.as_constant(sr).unwrap();
        assert_eq!(first.const_.check_static_ptr(&b.allocs), Some(s));
        assert_eq!(first.const_.check_static_ptr(&b.allocs), second.const_.check_static_ptr(&b.allocs));
        assert_eq!(b.allocs.entries.len(), 1, "one allocation per static");
        assert!(matches!(b.as_constant(nc).unwrap().const_, Const::Unevaluated { def, promoted: None, .. } if def == c));
    }

    fn binary(fx: &mut Fx, op: BinOp, ty: TyId, lhs: &str, rhs: &str, rhs_ty: TyId) -> ExprId {
        let (l, r) = (lit(fx, lhs, ty, false), lit(fx, rhs, rhs_ty, false));
        fx.thir.alloc_expr(Expr { kind: ExprKind::Binary { op, lhs: l, rhs: r }, ty, temp_scope: None, bid: None, span: Span::default() })
    }
    fn terminators(b: &Builder<'_>) -> Vec<&'static str> {
        b.cfg.basic_blocks.iter().filter_map(|bb| bb.terminator.as_ref()).map(|t| match t.kind { TerminatorKind::Assert { .. } => "assert", _ => "other" }).collect()
    }

    #[test]
    fn checked_arithmetic_emits_the_overflow_and_zero_checks() {
        let mut fx = fixture();
        let t = fx.builder_parts.1.tys.i32_;
        let add = binary(&mut fx, BinOp::Add, t, "42", "42", t);
        let div = binary(&mut fx, BinOp::Div, t, "42", "42", t);
        let u8_ = fx.builder_parts.1.tys.u8_;
        let shl = binary(&mut fx, BinOp::Shl, t, "42", "3u8", u8_);
        let eq = binary(&mut fx, BinOp::Eq, t, "42", "42", t);
        let (mut b, _) = builder(fx);
        let start = b.cfg.start_new_block();

        let BlockAnd(after, rv) = b.as_rvalue(start, b.local_temp_lifetime(), add).unwrap();
        assert!(matches!(rv, Rvalue::Use(Operand::Move(ref p)) if p.projection == [PlaceElem::Field(FieldIdx(0), t)]), "{rv:?}");
        assert_eq!(terminators(&b), ["assert"]);
        assert_ne!(after, start);
        let first = &b.cfg.block_data(start).statements[0];
        assert!(matches!(&first.kind, crate::mir::StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::AddWithOverflow, _))));

        // a / b: the zero check and the MIN / -1 check
        let n = b.cfg.basic_blocks.len();
        let BlockAnd(after, rv) = b.as_rvalue(after, b.local_temp_lifetime(), div).unwrap();
        assert!(matches!(rv, Rvalue::BinaryOp(BinOp::Div, _)));
        assert_eq!(b.cfg.basic_blocks.len() - n, 2, "two asserts, two continuation blocks");

        // a << b with an unsigned amount: one in-range check against the bit width
        let n = b.cfg.basic_blocks.len();
        let BlockAnd(after, rv) = b.as_rvalue(after, b.local_temp_lifetime(), shl).unwrap();
        assert!(matches!(rv, Rvalue::BinaryOp(BinOp::Shl, _)));
        assert_eq!(b.cfg.basic_blocks.len() - n, 1);

        // comparisons are unchecked
        let n = b.cfg.basic_blocks.len();
        let BlockAnd(same, rv) = b.as_rvalue(after, b.local_temp_lifetime(), eq).unwrap();
        assert!(matches!(rv, Rvalue::BinaryOp(BinOp::Eq, _)));
        assert_eq!((b.cfg.basic_blocks.len() - n, same), (0, after));
    }
}
