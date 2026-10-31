// Testing candidates
//
// After candidates have been simplified, the only match pairs that
// remain are those that require some sort of test. The functions here
// identify what tests are needed, perform the tests, and then filter
// the candidates based on the result.
//
// rustc_mir_build/src/builder/matches/test.rs
use super::util::OrderedMap;
use super::{MatchPairTree, PatConstKind, SliceLenOp, Test, TestBranch, TestKind, TestableCase};
use crate::consts::{Const as TyConst, Value};
use crate::lang_items::LangItem;
use crate::mir::build::thir::RangeEnd;
use crate::mir::build::{BuildResult, Builder, UnsupportedReason};
use crate::mir::{
    BasicBlock, BinOp, CallSource, CastKind, Const, ConstOperand, ConstValue, FakeReadCause, Mutability, Operand, Place, Rvalue, SourceInfo, Span, SwitchTargets,
    TerminatorKind, UnwindAction,
};
use crate::program::{DefId, DefKind, Prim};
use crate::types::{TyId, TyKind};
use std::sync::Arc;

impl TerminatorKind {
    /// rustc `TerminatorKind::if_`: `switchInt(cond) -> [0: else, otherwise: then]`.
    pub(super) fn if_(cond: Operand, then_block: BasicBlock, else_block: BasicBlock) -> TerminatorKind {
        TerminatorKind::SwitchInt { discr: cond, targets: SwitchTargets { values: vec![0], targets: vec![else_block], otherwise: then_block } }
    }
}

impl Builder<'_> {
    /// Identifies what test is needed to decide if `match_pair` is applicable.
    ///
    /// It is a bug to call this with a not-fully-simplified pattern.
    pub(super) fn pick_test_for_match_pair(&mut self, match_pair: &MatchPairTree) -> BuildResult<Test> {
        let kind = match &match_pair.testable_case {
            TestableCase::Variant { adt_def, variant_index: _ } => TestKind::Switch { adt_def: *adt_def },

            TestableCase::Constant { value: _, kind: PatConstKind::Bool } => TestKind::If,
            TestableCase::Constant { value: _, kind: PatConstKind::IntOrChar } => TestKind::SwitchInt,
            TestableCase::Constant { value, kind: PatConstKind::String } => TestKind::StringEq { value: value.clone() },
            TestableCase::Constant { value, kind: PatConstKind::Float | PatConstKind::Other } => TestKind::ScalarEq { value: value.clone() },

            TestableCase::Range(range) => TestKind::Range(Arc::clone(range)),

            TestableCase::Slice { len, op } => TestKind::SliceLen { len: *len, op: *op },

            TestableCase::Deref { temp, mutability } => TestKind::Deref { temp: temp.clone(), mutability: *mutability },

            TestableCase::Never => TestKind::Never,

            // Or-patterns are not tested directly; instead they are expanded into subcandidates,
            // which are then distinguished by testing whatever non-or patterns they contain.
            TestableCase::Or { .. } => {
                return Err(self.error(match_pair.pattern_span, UnsupportedReason::UnresolvedFact("or-pattern reached test selection")));
            }
        };

        Ok(Test { span: match_pair.pattern_span, kind })
    }

    pub(super) fn perform_test(
        &mut self,
        match_start_span: Span,
        scrutinee_span: Span,
        block: BasicBlock,
        otherwise_block: BasicBlock,
        place: Place,
        test: &Test,
        target_blocks: OrderedMap<TestBranch, BasicBlock>,
    ) -> BuildResult<()> {
        let (prog, tcx) = (self.root.cx.prog, self.root.cx.tcx);
        let tys = &tcx.tys;
        let place_ty = self.place_ty(&place)?;
        let target_block = |branch: TestBranch| target_blocks.get(&branch).copied().unwrap_or(otherwise_block);

        let source_info = self.source_info(test.span);
        match &test.kind {
            TestKind::Switch { adt_def } => {
                let otherwise_block = target_block(TestBranch::Failure);
                let Some(adt) = tcx.adts.get(adt_def) else {
                    return Err(self.error(test.span, UnsupportedReason::UnresolvedFact("switch on an unknown ADT")));
                };
                let mut values = Vec::new();
                let mut targets = Vec::new();
                for (idx, discr) in adt.discriminants(tcx, prog) {
                    if let Some(&block) = target_blocks.get(&TestBranch::Variant(crate::mir::VariantIdx(idx))) {
                        values.push(discr.val);
                        targets.push(block);
                    }
                }
                let switch_targets = SwitchTargets { values, targets, otherwise: otherwise_block };
                let discr_ty = tys.prim(adt.repr_int.unwrap_or(Prim::Isize));
                let discr = self.temp(discr_ty, test.span);
                let si = self.source_info(scrutinee_span);
                self.cfg.push_assign(block, si, discr.clone(), Rvalue::Discriminant(place));
                let si = self.source_info(match_start_span);
                self.cfg.terminate(block, si, TerminatorKind::SwitchInt { discr: Operand::Move(discr), targets: switch_targets });
            }

            TestKind::SwitchInt => {
                // The switch may be inexhaustive so we have a catch-all block
                let otherwise_block = target_block(TestBranch::Failure);
                let mut values = Vec::new();
                let mut targets = Vec::new();
                for (branch, &block) in target_blocks.iter() {
                    if let TestBranch::Constant(value) = branch {
                        let Some(bits) = super::pattern_bits(value) else {
                            return Err(self.error(test.span, UnsupportedReason::UnresolvedFact("switch value is not a scalar")));
                        };
                        values.push(bits);
                        targets.push(block);
                    }
                }
                let terminator = TerminatorKind::SwitchInt { discr: Operand::Copy(place), targets: SwitchTargets { values, targets, otherwise: otherwise_block } };
                let si = self.source_info(match_start_span);
                self.cfg.terminate(block, si, terminator);
            }

            TestKind::If => {
                let success_block = target_block(TestBranch::Success);
                let fail_block = target_block(TestBranch::Failure);
                let terminator = TerminatorKind::if_(Operand::Copy(place), success_block, fail_block);
                let si = self.source_info(match_start_span);
                self.cfg.terminate(block, si, terminator);
            }

            TestKind::StringEq { value } => {
                let success_block = target_block(TestBranch::Success);
                let fail_block = target_block(TestBranch::Failure);

                let ref_str_ty = self.ref_ty(false, tys.prim(Prim::Str));

                // The string constant we're testing against has type `str`, but calling
                // `<str as PartialEq>::eq` requires `&str` operands.
                //
                // Because `str` and `&str` have the same valtree representation, we can "cast" to
                // the desired type by just replacing the type.
                if !matches!(tys.kind(value.ty), TyKind::Str) {
                    return Err(self.error(test.span, UnsupportedReason::UnresolvedFact("string test against a non-str value")));
                }
                let expected_value = Value { ty: ref_str_ty, valtree: value.valtree.clone() };
                let expected_value_operand = self.literal_operand(test.span, Const::Ty(ref_str_ty, TyConst::new_value(tys, expected_value.valtree, ref_str_ty)));

                // Similarly, the scrutinized place has type `str`, but we need `&str`. Get a
                // reference by doing `let actual_value_ref_place: &str = &place`.
                let actual_value_ref_place = self.temp(ref_str_ty, test.span);
                let si = self.source_info(test.span);
                self.cfg.push_assign(
                    block,
                    si,
                    actual_value_ref_place.clone(),
                    Rvalue::Ref(crate::mir::Region::ERASED, crate::mir::BorrowKind::Shared, place),
                );

                // Compare two strings using `<str as std::cmp::PartialEq>::eq`. (Interestingly this
                // means that exhaustiveness analysis relies, for soundness, on the `PartialEq` impl
                // for `str` to be correct!)
                self.string_compare(block, success_block, fail_block, source_info, expected_value_operand, Operand::Copy(actual_value_ref_place))?;
            }

            TestKind::ScalarEq { value } => {
                let success_block = target_block(TestBranch::Success);
                let fail_block = target_block(TestBranch::Failure);

                let expected_value_ty = value.ty;
                let expected_value_operand = self.literal_operand(test.span, Const::Ty(value.ty, TyConst::new_value(tys, value.valtree.clone(), value.ty)));
                let actual_value_place = place;

                // (Pattern types (`ty::Pat`) do not exist in Relax: no transmute to the base type.)
                if !matches!(tys.kind(expected_value_ty), TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Ptr(..)) {
                    return Err(self.error(test.span, UnsupportedReason::UnresolvedFact("scalar test against a non-scalar value")));
                }

                self.compare(block, success_block, fail_block, source_info, BinOp::Eq, expected_value_operand, Operand::Copy(actual_value_place));
            }

            TestKind::Range(range) => {
                let success = target_block(TestBranch::Success);
                let fail = target_block(TestBranch::Failure);
                // Test `val` by computing `lo <= val && val <= hi`, using primitive comparisons.
                let val = Operand::Copy(place);

                let intermediate_block = if !range.lo.is_finite() {
                    block
                } else if !range.hi.is_finite() {
                    success
                } else {
                    self.cfg.start_new_block()
                };

                if let Some(lo) = range.lo.as_finite() {
                    let lo = Const::Ty(range.ty, TyConst::new_value(tys, lo.clone(), range.ty));
                    let lo = self.literal_operand(test.span, lo);
                    self.compare(block, intermediate_block, fail, source_info, BinOp::Le, lo, val.clone());
                };

                if let Some(hi) = range.hi.as_finite() {
                    let hi = Const::Ty(range.ty, TyConst::new_value(tys, hi.clone(), range.ty));
                    let hi = self.literal_operand(test.span, hi);
                    let op = match range.end {
                        RangeEnd::Included => BinOp::Le,
                        RangeEnd::Excluded => BinOp::Lt,
                    };
                    self.compare(intermediate_block, success, fail, source_info, op, val, hi);
                }
            }

            TestKind::SliceLen { len, op } => {
                let usize_ty = tys.prim(Prim::Usize);
                let actual = self.temp(usize_ty, test.span);

                // actual = len(place)
                let length_op = self.len_of_slice_or_array(block, place, test.span, source_info)?;
                self.cfg.push_assign(block, source_info, actual.clone(), Rvalue::Use(length_op));

                // expected = <N>
                let expected = self.push_usize(block, source_info, *len);

                let success_block = target_block(TestBranch::Success);
                let fail_block = target_block(TestBranch::Failure);
                // result = actual == expected OR result = actual < expected
                // branch based on result
                self.compare(
                    block,
                    success_block,
                    fail_block,
                    source_info,
                    match op {
                        SliceLenOp::Equal => BinOp::Eq,
                        SliceLenOp::GreaterOrEqual => BinOp::Ge,
                    },
                    Operand::Move(actual),
                    Operand::Move(expected),
                );
            }

            TestKind::Deref { temp, mutability } => {
                let ty = place_ty.ty;
                let target = target_block(TestBranch::Success);
                self.call_deref(block, target, place, *mutability, ty, temp.clone(), test.span)?;
            }

            TestKind::Never => {
                // Check that the place is initialized.
                self.cfg.push_fake_read(block, source_info, FakeReadCause::ForMatchedPlace(None), place);
                // A never pattern is only allowed on an uninhabited type, so validity of the data
                // implies unreachability.
                self.cfg.terminate(block, source_info, TerminatorKind::Unreachable);
            }
        }
        Ok(())
    }

    /// Perform `let temp = <ty as Deref>::deref(&place)`.
    /// or `let temp = <ty as DerefMut>::deref_mut(&mut place)`.
    pub(super) fn call_deref(
        &mut self,
        block: BasicBlock,
        target_block: BasicBlock,
        place: Place,
        mutability: Mutability,
        ty: TyId,
        temp: Place,
        span: Span,
    ) -> BuildResult<()> {
        let (trait_item, method) = match mutability {
            Mutability::Not => (LangItem::Deref, "deref"),
            Mutability::Mut => (LangItem::DerefMut, "deref_mut"),
        };
        let borrow_kind = super::util::ref_pat_borrow_kind(mutability);
        let source_info = self.source_info(span);
        let Some(trait_item) = self.root.cx.tcx.lang_items.get(trait_item) else {
            return Err(self.error(span, UnsupportedReason::UnresolvedFact("Deref lang item")));
        };
        let method = self.trait_method(trait_item, method, &[ty], span)?;
        let ref_src = self.temp(self.ref_ty(mutability == Mutability::Mut, ty), span);
        // `let ref_src = &src_place;`
        // or `let ref_src = &mut src_place;`
        self.cfg.push_assign(block, source_info, ref_src.clone(), Rvalue::Ref(crate::mir::Region::ERASED, borrow_kind, place));
        // `let temp = <Ty as Deref>::deref(ref_src);`
        // or `let temp = <Ty as DerefMut>::deref_mut(ref_src);`
        self.cfg.terminate(
            block,
            source_info,
            TerminatorKind::Call {
                func: Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_: method })),
                args: vec![Operand::Move(ref_src)],
                destination: temp,
                target: Some(target_block),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: source_info.span,
                arg_spans: vec![span],
            },
        );
        Ok(())
    }

    /// Compare using the provided built-in comparison operator
    fn compare(&mut self, block: BasicBlock, success_block: BasicBlock, fail_block: BasicBlock, source_info: SourceInfo, op: BinOp, left: Operand, right: Operand) {
        let bool_ty = self.root.cx.tcx.tys.bool_;
        let result = self.temp(bool_ty, source_info.span);

        // result = op(left, right)
        self.cfg.push_assign(block, source_info, result.clone(), Rvalue::BinaryOp(op, Box::new((left, right))));

        // branch based on result
        self.cfg.terminate(block, source_info, TerminatorKind::if_(Operand::Move(result), success_block, fail_block));
    }

    /// Compare two values of type `&str` using `<str as std::cmp::PartialEq>::eq`.
    fn string_compare(
        &mut self,
        block: BasicBlock,
        success_block: BasicBlock,
        fail_block: BasicBlock,
        source_info: SourceInfo,
        expect: Operand,
        val: Operand,
    ) -> BuildResult<()> {
        let tys = &self.root.cx.tcx.tys;
        let str_ty = tys.prim(Prim::Str);
        let Some(eq_def_id) = self.root.cx.tcx.lang_items.get(LangItem::PartialEq) else {
            return Err(self.error(source_info.span, UnsupportedReason::UnresolvedFact("PartialEq lang item")));
        };
        let method = self.trait_method(eq_def_id, "eq", &[str_ty, str_ty], source_info.span)?;

        let bool_ty = tys.bool_;
        let eq_result = self.temp(bool_ty, source_info.span);
        let eq_block = self.cfg.start_new_block();
        self.cfg.terminate(
            block,
            source_info,
            TerminatorKind::Call {
                func: Operand::Constant(Box::new(ConstOperand {
                    span: source_info.span,
                    // This constant comes from user input (a constant in a pattern): no user type.
                    user_ty: None,
                    const_: method,
                })),
                args: vec![val, expect],
                destination: eq_result.clone(),
                target: Some(eq_block),
                unwind: UnwindAction::Continue,
                call_source: CallSource::MatchCmp,
                fn_span: source_info.span,
                arg_spans: vec![Span::default(), Span::default()],
            },
        );
        self.diverge_from(block)?;

        // check the result
        self.cfg.terminate(eq_block, source_info, TerminatorKind::if_(Operand::Move(eq_result), success_block, fail_block));
        Ok(())
    }

    /// `trait_method`: the zero-sized function item of the trait's method `name`, instantiated with
    /// `args` (the Self type first).
    fn trait_method(&self, trait_def_id: DefId, method_name: &str, args: &[TyId], span: Span) -> BuildResult<Const> {
        let (prog, tcx) = (self.root.cx.prog, self.root.cx.tcx);
        let item = prog
            .syms
            .get(method_name)
            .and_then(|name| prog.traits[prog.def(trait_def_id).sub as usize].iter().copied().find(|&item| prog.def(item).name == name && prog.def(item).kind == DefKind::AssocFn));
        let Some(item) = item else {
            return Err(self.error(span, UnsupportedReason::UnresolvedFact("trait method not found")));
        };
        let method_ty = tcx.tys.intern(TyKind::FnDef(item, tcx.tys.mk_list(args), crate::types::RegionArgsId::EMPTY));
        Ok(Const::Val(ConstValue::ZeroSized, method_ty))
    }
}

#[allow(dead_code)]
fn _unused(_: CastKind) {}
