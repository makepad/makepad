//! Code related to match expressions. These are sufficiently complex to warrant their own module and
//! submodules. :) This main module includes the high-level algorithm, the submodules contain the
//! details.
//!
//! This also includes code for pattern bindings in `let` statements and function parameters.
//!
//! Port of rustc_mir_build/src/builder/matches/mod.rs (1.97.1). Differences, all deliberate:
//! - `BuildResult` threads through every step the builder can stop at (scope and expression lowering
//!   return `BuildError` for what is not built), and `unpack!` is `?`-based.
//! - No lint levels, coverage instrumentation, `ensure_sufficient_stack`, loop-match
//!   (`static_pattern_match`) or pinned bindings (unstable); `Place` is `Clone`, not `Copy`.
//! - `OpaqueCast` projections are not added (see `match_pair.rs`).
use crate::mir::build::scope::DropKind;
use crate::mir::build::thir::{Arm, ArmId, ExprId, ExprKind, LocalVarId, LogicalOp, Pat, PatKind};
use crate::mir::build::{BlockAnd, BlockAndExtension, BuildResult, Builder, GuardFrame, GuardFrameLocal, LocalsForNode, PlaceBuilder, UnsupportedReason};
use crate::mir::build::builder::ForGuard::{self, OutsideGuard, RefWithinGuard};
use crate::mir::{
    BasicBlock, BindingForm, BindingMode, BorrowKind, ByRef, CanonicalUserTypeAnnotation, FakeBorrowKind, FakeReadCause, Local, LocalInfo, LocalKind, Mutability, Operand,
    Place, Rvalue, SourceInfo, SourceScope, Span, Statement, StatementKind, TerminatorKind, UnOp, UserTypeAnnotationIndex, UserTypeProjection, VarBindingForm,
    VarBindingIntroduction, VarDebugInfo, Variance, VariantIdx,
};
use crate::program::{DefId, Sym};
use crate::region_scope::ScopeId;
use crate::types::TyId;
use buckets::PartitionedCandidates;
use std::mem;
use std::sync::Arc;
use user_ty::ProjectedUserTypesNode;
use util::OrderedMap;

/// `unpack!(block = expr)`: run a fallible block-threading step.
macro_rules! unpack {
    ($x:ident = $c:expr) => {{
        let BlockAnd(b, v) = $c?;
        $x = b;
        v
    }};
}

// helper functions, broken out by category:
mod buckets;
mod match_pair;
mod test;
mod user_ty;
mod util;

#[cfg(test)]
mod tests;

/// `PatKind::Constant` of `bool` type as a Rust bool.
pub(super) fn pattern_bool(value: &crate::consts::Value) -> Option<bool> {
    crate::mir::build::pattern::range::leaf_bits(&value.valtree).map(|bits| bits != 0)
}

/// The bits of a scalar pattern constant.
pub(super) fn pattern_bits(value: &crate::consts::Value) -> Option<u128> {
    crate::mir::build::pattern::range::leaf_bits(&value.valtree)
}

/// Arguments to [`Builder::then_else_break_inner`] that are usually forwarded to recursive invocations.
#[derive(Clone, Copy)]
struct ThenElseArgs {
    /// Used as the temp scope for lowering `expr`. If absent (for match guards), `self.local_scope()` is used.
    temp_scope_override: Option<ScopeId>,
    variable_source_info: SourceInfo,
    /// Determines how bindings should be handled when lowering `let` expressions.
    ///
    /// Forwarded to [`Builder::lower_let_expr`] when lowering [`ExprKind::Let`].
    declare_let_bindings: DeclareLetBindings,
}

/// Should lowering a `let` expression also declare its bindings?
///
/// Used by [`Builder::lower_let_expr`] when lowering [`ExprKind::Let`].
#[derive(Clone, Copy)]
pub enum DeclareLetBindings {
    /// Yes, declare `let` bindings as normal for `if` conditions.
    Yes,
    /// No, don't declare `let` bindings, because the caller declares them separately due to special
    /// requirements.
    ///
    /// Used for match guards and let-else.
    No,
    /// Let expressions are not permitted in this context, so it is a bug to try to lower one (e.g
    /// inside lazy-boolean-or or boolean-not).
    LetNotPermitted,
}

/// Used by [`Builder::storage_live_binding`] and [`Builder::bind_matched_candidate_for_arm_body`] to
/// decide whether to schedule drops.
#[derive(Clone, Copy, Debug)]
pub enum ScheduleDrops {
    /// Yes, the relevant functions should also schedule drops as appropriate.
    Yes,
    /// No, don't schedule drops. The caller has taken responsibility for any appropriate drops.
    No,
}

impl<'a> Builder<'a> {
    /// Lowers a condition in a way that ensures that variables bound in any let expressions are
    /// definitely initialized in the if body.
    ///
    /// If `declare_let_bindings` is false then variables created in `let` expressions will not be
    /// declared. This is for if let guards on arms with an or pattern, where the guard is lowered
    /// multiple times.
    pub fn then_else_break(
        &mut self,
        block: BasicBlock,
        expr_id: ExprId,
        temp_scope_override: Option<ScopeId>,
        variable_source_info: SourceInfo,
        declare_let_bindings: DeclareLetBindings,
    ) -> BuildResult<BlockAnd<()>> {
        self.then_else_break_inner(block, expr_id, ThenElseArgs { temp_scope_override, variable_source_info, declare_let_bindings })
    }

    fn then_else_break_inner(
        &mut self,
        block: BasicBlock, // Block that the condition and branch will be lowered into
        expr_id: ExprId,   // Condition expression to lower
        args: ThenElseArgs,
    ) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        let expr = &thir.thir[expr_id];
        let expr_span = expr.span;

        match &expr.kind {
            ExprKind::LogicalOp { op: LogicalOp::And, lhs, rhs } => {
                let lhs_then_block = self.then_else_break_inner(block, *lhs, args)?.into_block();
                let rhs_then_block = self.then_else_break_inner(lhs_then_block, *rhs, args)?.into_block();
                Ok(rhs_then_block.unit())
            }
            ExprKind::LogicalOp { op: LogicalOp::Or, lhs, rhs } => {
                let local_scope = self.local_scope();
                let (lhs_success_block, failure_block) = self.in_if_then_scope(local_scope, expr_span, |this| {
                    this.then_else_break_inner(block, *lhs, ThenElseArgs { declare_let_bindings: DeclareLetBindings::LetNotPermitted, ..args })
                })?;
                let rhs_success_block = self
                    .then_else_break_inner(failure_block, *rhs, ThenElseArgs { declare_let_bindings: DeclareLetBindings::LetNotPermitted, ..args })?
                    .into_block();

                // Make the LHS and RHS success arms converge to a common block. (We can't just make
                // LHS goto RHS, because `rhs_success_block` might contain statements that we don't
                // want on the LHS path.)
                let success_block = self.cfg.start_new_block();
                self.cfg.goto(lhs_success_block, args.variable_source_info, success_block);
                self.cfg.goto(rhs_success_block, args.variable_source_info, success_block);
                Ok(success_block.unit())
            }
            ExprKind::Unary { op: crate::mir::UnOp::Not, arg } => {
                let local_scope = self.local_scope();
                let (success_block, failure_block) = self.in_if_then_scope(local_scope, expr_span, |this| {
                    this.then_else_break_inner(block, *arg, ThenElseArgs { declare_let_bindings: DeclareLetBindings::LetNotPermitted, ..args })
                })?;
                self.break_for_else(success_block, args.variable_source_info)?;
                Ok(failure_block.unit())
            }
            ExprKind::Scope { region_scope, node: _, value } => {
                let region_scope = (*region_scope, self.source_info(expr_span));
                self.in_scope(region_scope, |this| this.then_else_break_inner(block, *value, args))
            }
            ExprKind::Use { source } => self.then_else_break_inner(block, *source, args),
            ExprKind::Let { expr, pat } => self.lower_let_expr(
                block,
                *expr,
                pat,
                Some(args.variable_source_info.scope),
                args.variable_source_info.span,
                args.declare_let_bindings,
            ),
            _ => {
                let mut block = block;
                let temp_scope = match args.temp_scope_override {
                    Some(scope) => scope,
                    None => self.local_scope(),
                };
                let mutability = Mutability::Mut;

                let place = unpack!(
                    block = self.as_temp(
                        block,
                        crate::mir::build::builder::TempLifetime { temp_lifetime: Some(temp_scope), backwards_incompatible: None },
                        expr_id,
                        mutability
                    )
                );

                let operand = Operand::Move(Place::local(place));

                let then_block = self.cfg.start_new_block();
                let else_block = self.cfg.start_new_block();
                let term = TerminatorKind::if_(operand, then_block, else_block);

                let source_info = self.source_info(expr_span);
                self.cfg.terminate(block, source_info, term);
                self.break_for_else(else_block, source_info)?;

                Ok(then_block.unit())
            }
        }
    }

    /// Generates MIR for a `match` expression.
    ///
    /// The MIR that we generate for a match looks like this.
    ///
    /// ```text
    /// [ 0. Pre-match ]
    ///        |
    /// [ 1. Evaluate Scrutinee (expression being matched on) ]
    /// [ (PlaceMention of scrutinee) ]
    ///        |
    /// [ 2. Decision tree -- check discriminants ] <--------+
    ///        |                                             |
    ///        | (once a specific arm is chosen)             |
    ///        |                                             |
    /// [pre_binding_block]                           [otherwise_block]
    ///        |                                             |
    /// [ 3. Create "guard bindings" for arm ]               |
    /// [ (create fake borrows) ]                            |
    ///        |                                             |
    /// [ 4. Execute guard code ]                            |
    /// [ (read fake borrows) ] --(guard is false)-----------+
    ///        |
    ///        | (guard results in true)
    ///        |
    /// [ 5. Create real bindings and execute arm ]
    ///        |
    /// [ Exit match ]
    /// ```
    ///
    /// All of the different arms have been stacked on top of each other to simplify the diagram. For
    /// an arm with no guard the blocks marked 3 and 4 and the fake borrows are omitted.
    ///
    /// We generate MIR in the following steps:
    ///
    /// 1. Evaluate the scrutinee and add the PlaceMention of it ([Builder::lower_scrutinee]).
    /// 2. Create the decision tree ([Builder::lower_match_tree]).
    /// 3. Determine the fake borrows that are needed from the places that were matched against and
    ///    create the required temporaries for them ([util::collect_fake_borrows]).
    /// 4. Create everything else: the guards and the arms ([Builder::lower_match_arms]).
    ///
    /// ## False edges
    ///
    /// We don't want to have the exact structure of the decision tree be visible through borrow
    /// checking. Specifically we want borrowck to think that:
    /// - at any point, any or none of the patterns and guards seen so far may have been tested;
    /// - after the match, any of the patterns may have matched.
    ///
    /// For example, all of these would fail to error if borrowck could see the real CFG (examples
    /// taken from `tests/ui/nll/match-cfg-fake-edges.rs`):
    /// ```ignore (too many errors, this is already in the test suite)
    /// let x = String::new();
    /// let _ = match true {
    ///     _ => {},
    ///     _ => drop(x),
    /// };
    /// // Borrowck must not know the second arm is never run.
    /// drop(x); //~ ERROR use of moved value
    ///
    /// let x;
    /// # let y = true;
    /// match y {
    ///     _ if { x = 2; true } => {},
    ///     // Borrowck must not know the guard is always run.
    ///     _ => drop(x), //~ ERROR used binding `x` is possibly-uninitialized
    /// };
    ///
    /// let x = String::new();
    /// # let y = true;
    /// match y {
    ///     false if { drop(x); true } => {},
    ///     // Borrowck must not know the guard is not run in the `true` case.
    ///     true => drop(x), //~ ERROR use of moved value: `x`
    ///     false => {},
    /// };
    ///
    /// # let mut y = (true, true);
    /// let r = &mut y.1;
    /// match y {
    ///     //~^ ERROR cannot use `y.1` because it was mutably borrowed
    ///     (false, true) => {}
    ///     // Borrowck must not know we don't test `y.1` when `y.0` is `true`.
    ///     (true, _) => drop(r),
    ///     (false, _) => {}
    /// };
    /// ```
    ///
    /// We add false edges to act as if we were naively matching each arm in order. What we need is a
    /// (fake) path from each candidate to the next, specifically from candidate C's pre-binding
    /// block to next candidate D's pre-binding block. For maximum precision (needed for deref
    /// patterns), we choose the earliest node on D's success path that doesn't also lead to C (to
    /// avoid loops).
    ///
    /// This turns out to be easy to compute: that block is the `start_block` of the first call to
    /// `match_candidates` where D is the first candidate in the list.
    ///
    /// For example:
    /// ```rust
    /// # let (x, y) = (true, true);
    /// match (x, y) {
    ///   (true, true) => 1,
    ///   (false, true) => 2,
    ///   (true, false) => 3,
    ///   _ => 4,
    /// }
    /// # ;
    /// ```
    /// In this example, the pre-binding block of arm 1 has a false edge to the block for result
    /// `false` of the first test on `x`. The other arms have false edges to the pre-binding blocks
    /// of the next arm.
    ///
    /// On top of this, we also add a false edge from the otherwise_block of each guard to the
    /// aforementioned start block of the next candidate, to ensure borrock doesn't rely on which
    /// guards may have run.
    pub fn match_expr(
        &mut self,
        destination: Place,
        mut block: BasicBlock,
        scrutinee_id: ExprId,
        arms: &[ArmId],
        span: Span,
        scrutinee_span: Span,
    ) -> BuildResult<BlockAnd<()>> {
        let scrutinee_place = unpack!(block = self.lower_scrutinee(block, scrutinee_id, scrutinee_span));

        // `span.shrink_to_lo().to(scrutinee_span)`
        let match_start_span = Span { file: span.file, lo: span.lo, hi: scrutinee_span.hi.max(span.lo) };
        let thir = self.thir;
        let patterns = arms
            .iter()
            .map(|&arm| {
                let arm = &thir.thir[arm];
                let has_match_guard = if arm.guard.is_some() { HasMatchGuard::Yes } else { HasMatchGuard::No };
                (&*arm.pattern, has_match_guard)
            })
            .collect();
        let built_tree = self.lower_match_tree(block, scrutinee_span, &scrutinee_place, match_start_span, patterns, false)?;

        let outer = self.source_info(span);
        self.lower_match_arms(destination, scrutinee_place, scrutinee_span, arms, built_tree, outer)
    }

    /// Evaluate the scrutinee and add the PlaceMention for it.
    pub fn lower_scrutinee(&mut self, mut block: BasicBlock, scrutinee_id: ExprId, scrutinee_span: Span) -> BuildResult<BlockAnd<PlaceBuilder>> {
        let scrutinee_place_builder = unpack!(block = self.as_place_builder(block, scrutinee_id));
        if let Some(scrutinee_place) = scrutinee_place_builder.try_to_place(self) {
            let source_info = self.source_info(scrutinee_span);
            self.cfg.push_place_mention(block, source_info, scrutinee_place);
        }

        Ok(block.and(scrutinee_place_builder))
    }

    /// Lower the bindings, guards and arm bodies of a `match` expression.
    ///
    /// The decision tree should have already been created (by [Builder::lower_match_tree]).
    ///
    /// `outer_source_info` is the SourceInfo for the whole match.
    fn lower_match_arms(
        &mut self,
        destination: Place,
        scrutinee_place_builder: PlaceBuilder,
        scrutinee_span: Span,
        arms: &[ArmId],
        built_match_tree: BuiltMatchTree,
        outer_source_info: SourceInfo,
    ) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        let BuiltMatchTree { branches, fake_borrow_temps, .. } = built_match_tree;
        let mut arm_end_blocks: Vec<BasicBlock> = Vec::with_capacity(arms.len());
        for (&arm_id, branch) in arms.iter().zip(branches) {
            let arm = &thir.thir[arm_id];
            let arm_source_info = self.source_info(arm.span);
            let arm_scope = (arm.scope, arm_source_info);
            let match_scope = self.local_scope();
            // The guard scope shares the arm's node (rustc `Scope { data: MatchGuard, ..arm.scope }`).
            let guard_scope = match arm.guard {
                Some(_) => {
                    let tree = &self.root.scope_tree;
                    let node = tree.scopes[arm.scope.0 as usize].node;
                    match tree.scope(node, crate::region_scope::ScopeData::MatchGuard) {
                        Some(scope) => Some(scope),
                        None => return Err(self.error(arm.span, UnsupportedReason::UnresolvedFact("match guard scope"))),
                    }
                }
                None => None,
            };
            let outer_source_info = outer_source_info;
            let block = self
                .in_scope(arm_scope, |this| {
                    this.opt_in_scope(guard_scope.map(|scope| (scope, arm_source_info)), |this| {
                        // `if let` guard temps needing deduplicating will be in the guard scope.
                        let old_dedup_scope = mem::replace(&mut this.fixed_temps_scope, guard_scope);

                        // `try_to_place` may fail if it is unable to resolve the given
                        // `PlaceBuilder` inside a closure. In this case, we don't want to include a
                        // scrutinee place. `scrutinee_place_builder` will fail to be resolved if the
                        // only match arm is a wildcard (`_`).
                        // Example:
                        // ```
                        // let foo = (0, 1);
                        // let c = || {
                        //    match foo { _ => () };
                        // };
                        // ```
                        let scrutinee_place = scrutinee_place_builder.try_to_place(this);
                        let opt_scrutinee_place = scrutinee_place.as_ref().map(|place| (Some(place), scrutinee_span));
                        let scope = this.declare_bindings(None, arm.span, &arm.pattern, arm.guard, opt_scrutinee_place)?;

                        let arm_block = this.bind_pattern(outer_source_info, branch, &fake_borrow_temps, scrutinee_span, Some((arm, match_scope)))?;

                        this.fixed_temps_scope = old_dedup_scope;

                        if let Some(source_scope) = scope {
                            this.source_scope = source_scope;
                        }

                        this.expr_into_dest(destination.clone(), arm_block, arm.body)
                    })
                })?
                .into_block();
            arm_end_blocks.push(block);
        }

        // all the arm blocks will rejoin here
        let end_block = self.cfg.start_new_block();

        let end_brace = self.source_info(Span { file: outer_source_info.span.file, lo: outer_source_info.span.hi.saturating_sub(1), hi: outer_source_info.span.hi });
        for arm_block in arm_end_blocks {
            let block = &self.cfg.basic_blocks[arm_block.0 as usize];
            let last_location = block.statements.last().map(|s| s.source_info);

            self.cfg.goto(arm_block, last_location.unwrap_or(end_brace), end_block);
        }

        self.source_scope = outer_source_info.scope;

        Ok(end_block.unit())
    }

    /// For a top-level `match` arm or a `let` binding, binds the variables and ascribes types, and
    /// also checks the match arm guard (if present).
    ///
    /// `arm_scope` should be `Some` if and only if this is called for a `match` arm.
    ///
    /// In the presence of or-patterns, a match arm might have multiple sub-branches representing
    /// different ways to match, with each sub-branch requiring its own bindings and its own copy of
    /// the guard. This method handles those sub-branches individually, and then has them jump
    /// together to a common block.
    ///
    /// Returns a single block that the match arm can be lowered into. (For `let` bindings, this is
    /// the code that can use the bindings.)
    fn bind_pattern(
        &mut self,
        outer_source_info: SourceInfo,
        branch: MatchTreeBranch,
        fake_borrow_temps: &[(Place, Local, FakeBorrowKind)],
        scrutinee_span: Span,
        arm_match_scope: Option<(&Arm, ScopeId)>,
    ) -> BuildResult<BasicBlock> {
        if branch.sub_branches.len() == 1 {
            let sub_branch = branch.sub_branches.into_iter().next().unwrap();
            // Avoid generating another `BasicBlock` when we only have one sub branch.
            self.bind_and_guard_matched_candidate(sub_branch, fake_borrow_temps, scrutinee_span, arm_match_scope, ScheduleDrops::Yes)
        } else {
            // It's helpful to avoid scheduling drops multiple times to save drop elaboration from
            // having to clean up the extra drops.
            //
            // If we are in a `let` then we only schedule drops for the first candidate.
            //
            // If we're in a `match` arm then we could have a case like so:
            //
            // Ok(x) | Err(x) if return => { /* ... */ }
            //
            // In this case we don't want a drop of `x` scheduled when we return: it isn't bound by
            // move until right before enter the arm. To handle this we instead unschedule it's drop
            // after each time we lower the guard. As a result, we end up with the drop order of the
            // last sub-branch we lower. To use the drop order for the first sub-branch, we lower
            // sub-branches in reverse (#142163).
            let target_block = self.cfg.start_new_block();
            let n = branch.sub_branches.len();
            for (i, sub_branch) in branch.sub_branches.into_iter().rev().enumerate() {
                debug_assert!(n > 1);
                let schedule_drops = if i == n - 1 { ScheduleDrops::Yes } else { ScheduleDrops::No };
                let binding_end = self.bind_and_guard_matched_candidate(sub_branch, fake_borrow_temps, scrutinee_span, arm_match_scope, schedule_drops)?;
                self.cfg.goto(binding_end, outer_source_info, target_block);
            }

            Ok(target_block)
        }
    }

    pub fn expr_into_pattern(&mut self, mut block: BasicBlock, irrefutable_pat: &Pat, initializer_id: ExprId) -> BuildResult<BlockAnd<()>> {
        match &irrefutable_pat.kind {
            // Optimize `let x = ...` and `let x: T = ...` to write directly into `x`, and then
            // require that `T == typeof(x)` if present.
            PatKind::Binding { mode: BindingMode(ByRef::No, _), var, subpattern: None, .. } => {
                let place = self.storage_live_binding(block, *var, irrefutable_pat.span, false, OutsideGuard, ScheduleDrops::Yes)?;
                block = self.expr_into_dest(place.clone(), block, initializer_id)?.into_block();

                // Inject a fake read, see comments on `FakeReadCause::ForLet`.
                let source_info = self.source_info(irrefutable_pat.span);
                self.cfg.push_fake_read(block, source_info, FakeReadCause::ForLet(None), place.clone());

                let ascriptions: &[_] = match irrefutable_pat.extra.as_deref() {
                    Some(extra) => extra.ascriptions.as_slice(),
                    None => &[],
                };
                for ascription in ascriptions {
                    let annotation = &ascription.annotation;
                    let ty_source_info = self.source_info(annotation.span);

                    let base = self.push_user_type_annotation(annotation.clone());
                    let stmt = Statement {
                        source_info: ty_source_info,
                        kind: StatementKind::AscribeUserType(
                            place.clone(),
                            UserTypeProjection { base, projection: Vec::new() },
                            // We always use invariant as the variance here. This is because the
                            // variance field from the ascription refers to the variance to use when
                            // applying the type to the value being matched, but this ascription
                            // applies rather to the type of the binding. e.g., in this example:
                            //
                            // ```
                            // let x: T = <expr>
                            // ```
                            //
                            // We are creating an ascription that defines the type of `x` to be
                            // exactly `T` (i.e., with invariance). The variance field, in contrast,
                            // is intended to be used to relate `T` to the type of `<expr>`.
                            Variance::Invariant,
                        ),
                    };
                    self.cfg.push(block, stmt);
                }

                self.schedule_drop_for_binding(*var, irrefutable_pat.span, OutsideGuard)?;
                Ok(block.unit())
            }

            _ => {
                let thir = self.thir;
                let initializer = &thir.thir[initializer_id];
                let place_builder = unpack!(block = self.lower_scrutinee(block, initializer_id, initializer.span));
                self.place_into_pattern(block, irrefutable_pat, place_builder, true)
            }
        }
    }

    pub fn place_into_pattern(
        &mut self,
        block: BasicBlock,
        irrefutable_pat: &Pat,
        initializer: PlaceBuilder,
        set_match_place: bool,
    ) -> BuildResult<BlockAnd<()>> {
        let built_tree =
            self.lower_match_tree(block, irrefutable_pat.span, &initializer, irrefutable_pat.span, vec![(irrefutable_pat, HasMatchGuard::No)], false)?;
        let [branch]: [MatchTreeBranch; 1] = built_tree.branches.try_into().map_err(|_| self.error(irrefutable_pat.span, UnsupportedReason::UnresolvedFact("one branch for one pattern")))?;

        // For matches and function arguments, the place that is being matched can be set when
        // creating the variables. But the place for let PATTERN = ... might not even exist until we
        // do the assignment. so we set it here instead.
        if set_match_place {
            // `try_to_place` may fail if it is unable to resolve the given `PlaceBuilder` inside a
            // closure. In this case, we don't want to include a scrutinee place.
            // `scrutinee_place_builder` will fail for destructured assignments. This is because a
            // closure only captures the precise places that it will read and as a result a closure
            // may not capture the entire tuple/struct and rather have individual places that will be
            // read in the final MIR.
            // Example:
            // ```
            // let foo = (0, 1);
            // let c = || {
            //    let (v1, v2) = foo;
            // };
            // ```
            if let Some(place) = initializer.try_to_place(self) {
                // Because or-alternatives bind the same variables, we only explore the first one.
                let first_sub_branch = &branch.sub_branches[0];
                for binding in &first_sub_branch.bindings {
                    let local = self.var_local_id(binding.var_id, OutsideGuard);
                    if let LocalInfo::User(BindingForm::Var(VarBindingForm { opt_match_place: Some((match_place, _)), .. })) =
                        &mut self.local_decls[local.0 as usize].local_info
                    {
                        *match_place = Some(place.clone());
                    } else {
                        return Err(self.error(binding.span, UnsupportedReason::UnresolvedFact("let binding to a non-user variable")));
                    }
                }
            }
        }

        let outer = self.source_info(irrefutable_pat.span);
        let block = self.bind_pattern(outer, branch, &[], irrefutable_pat.span, None)?;
        Ok(block.unit())
    }

    /// Declares the bindings of the given patterns and returns the visibility scope for the
    /// bindings in these patterns, if such a scope had to be created. NOTE: Declaring the bindings
    /// should always be done in their drop scope.
    pub fn declare_bindings(
        &mut self,
        visibility_scope: Option<SourceScope>,
        scope_span: Span,
        pattern: &Pat,
        guard: Option<ExprId>,
        opt_match_place: Option<(Option<&Place>, Span)>,
    ) -> BuildResult<Option<SourceScope>> {
        let mut visibility_scope = visibility_scope;
        self.visit_primary_bindings_special(pattern, &ProjectedUserTypesNode::None, &mut |this, name, mode, var, span, ty, user_tys| {
            let saved_scope = this.source_scope;
            let vis_scope = *visibility_scope.get_or_insert_with(|| this.new_source_scope(scope_span, None));
            let source_info = SourceInfo { span, scope: this.source_scope };
            let user_tys = user_tys.build_user_type_projections();

            this.declare_binding(
                source_info,
                vis_scope,
                name,
                mode,
                var,
                ty,
                user_tys,
                ArmHasGuard(guard.is_some()),
                opt_match_place.map(|(x, y)| (x.cloned(), y)),
                pattern.span,
            );
            this.source_scope = saved_scope;
            Ok(())
        })?;
        if let Some(guard_expr) = guard {
            self.declare_guard_bindings(guard_expr, scope_span, visibility_scope)?;
        }
        Ok(visibility_scope)
    }

    /// Declare bindings in a guard. This has to be done when declaring bindings for an arm to
    /// ensure that or patterns only have one version of each variable.
    pub fn declare_guard_bindings(&mut self, guard_expr: ExprId, scope_span: Span, visibility_scope: Option<SourceScope>) -> BuildResult<()> {
        let thir = self.thir;
        match &thir.thir[guard_expr].kind {
            ExprKind::Let { expr: _, pat: guard_pat } => {
                // FIXME: pass a proper `opt_match_place`
                self.declare_bindings(visibility_scope, scope_span, guard_pat, None, None)?;
            }
            ExprKind::Scope { value, .. } => {
                self.declare_guard_bindings(*value, scope_span, visibility_scope)?;
            }
            ExprKind::Use { source } => {
                self.declare_guard_bindings(*source, scope_span, visibility_scope)?;
            }
            ExprKind::LogicalOp { op: LogicalOp::And, lhs, rhs } => {
                self.declare_guard_bindings(*lhs, scope_span, visibility_scope)?;
                self.declare_guard_bindings(*rhs, scope_span, visibility_scope)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The scope a binding's drop is scheduled in: a desugaring's synthetic binding, or a user variable.
    fn var_scope(&self, var: LocalVarId) -> Option<ScopeId> {
        match self.thir.thir.var_scopes.get(&var.0) {
            Some(&scope) => Some(scope),
            None => self.root.scope_tree.var_scope(var.0),
        }
    }

    /// Emits a [`StatementKind::StorageLive`] for the given var, and also schedules a drop if
    /// requested (and possible).
    pub fn storage_live_binding(
        &mut self,
        block: BasicBlock,
        var: LocalVarId,
        span: Span,
        is_shorthand: bool,
        for_guard: ForGuard,
        schedule_drop: ScheduleDrops,
    ) -> BuildResult<Place> {
        let local_id = self.var_local_id(var, for_guard);
        let source_info = self.source_info(span);
        self.cfg.push(block, Statement { source_info, kind: StatementKind::StorageLive(local_id) });
        // Although there is almost always scope for given variable in corner cases like #92893 we
        // might get variable with no scope.
        if let Some(region_scope) = self.var_scope(var) {
            if matches!(schedule_drop, ScheduleDrops::Yes) {
                self.schedule_drop(span, region_scope, local_id, DropKind::Storage)?;
            }
        }
        if let LocalInfo::User(BindingForm::Var(var_info)) = &mut self.local_decls[local_id.0 as usize].local_info {
            var_info.introductions.push(VarBindingIntroduction { span, is_shorthand });
        }
        Ok(Place::local(local_id))
    }

    pub fn schedule_drop_for_binding(&mut self, var: LocalVarId, span: Span, for_guard: ForGuard) -> BuildResult<()> {
        let local_id = self.var_local_id(var, for_guard);
        if let Some(region_scope) = self.var_scope(var) {
            self.schedule_drop(span, region_scope, local_id, DropKind::Value)?;
        }
        Ok(())
    }

    /// Visits all of the "primary" bindings in a pattern, i.e. the leftmost occurrence of each
    /// variable bound by the pattern. See [`PatKind::Binding::is_primary`] for more context.
    ///
    /// This variant provides only the limited subset of binding data needed by its callers, and
    /// should be a "pure" visit without side-effects.
    pub fn visit_primary_bindings(&mut self, pattern: &Pat, f: &mut impl FnMut(&mut Self, LocalVarId, Span) -> BuildResult<()>) -> BuildResult<()> {
        let mut result = Ok(());
        pattern.walk_always(|pat| {
            if result.is_ok() {
                if let PatKind::Binding { var, is_primary: true, .. } = pat.kind {
                    result = f(self, var, pat.span);
                }
            }
        });
        result
    }

    /// Records a user type annotation in the body and returns its index.
    fn push_user_type_annotation(&mut self, annotation: CanonicalUserTypeAnnotation) -> UserTypeAnnotationIndex {
        self.canonical_user_type_annotations.push(annotation);
        UserTypeAnnotationIndex(self.canonical_user_type_annotations.len() as u32 - 1)
    }

    /// Visits all of the "primary" bindings in a pattern, while preparing additional
    /// user-type-annotation data needed by `declare_bindings`.
    ///
    /// This also has the side-effect of pushing all user type annotations onto
    /// `canonical_user_type_annotations`, so that they end up in MIR even if they aren't associated
    /// with any bindings.
    fn visit_primary_bindings_special(
        &mut self,
        pattern: &Pat,
        user_tys: &ProjectedUserTypesNode<'_>,
        f: &mut impl FnMut(&mut Self, Sym, BindingMode, LocalVarId, Span, TyId, &ProjectedUserTypesNode<'_>) -> BuildResult<()>,
    ) -> BuildResult<()> {
        // Ascriptions correspond to user-written types like `let A::<'a>(_): A<'static> = ...;`.
        //
        // Caution: Pushing user types here is load-bearing even for patterns containing no
        // bindings, to ensure that the type ends up represented in MIR _somewhere_.
        let pushed;
        let user_tys = match pattern.extra.as_deref() {
            Some(extra) if !extra.ascriptions.is_empty() => {
                let base_user_tys = extra
                    .ascriptions
                    .iter()
                    .map(|ascription| {
                        // Note that the variance doesn't apply here, as we are tracking the effect
                        // of user types on any bindings contained with subpattern.
                        self.push_user_type_annotation(ascription.annotation.clone())
                    })
                    .collect();
                pushed = user_tys.push_user_types(base_user_tys);
                &pushed
            }
            _ => user_tys,
        };

        match &pattern.kind {
            PatKind::Binding { name, mode, var, ty, subpattern, is_primary, .. } => {
                if *is_primary {
                    f(self, *name, *mode, *var, pattern.span, *ty, user_tys)?;
                }
                if let Some(subpattern) = subpattern.as_ref() {
                    self.visit_primary_bindings_special(subpattern, user_tys, f)?;
                }
            }

            PatKind::Array { prefix, slice, suffix } | PatKind::Slice { prefix, slice, suffix } => {
                let from = prefix.len() as u64;
                let to = suffix.len() as u64;
                for subpattern in prefix.iter() {
                    self.visit_primary_bindings_special(subpattern, &user_tys.index(), f)?;
                }
                if let Some(subpattern) = slice {
                    self.visit_primary_bindings_special(subpattern, &user_tys.subslice(from, to), f)?;
                }
                for subpattern in suffix.iter() {
                    self.visit_primary_bindings_special(subpattern, &user_tys.index(), f)?;
                }
            }

            PatKind::Constant { .. } | PatKind::Range { .. } | PatKind::Missing | PatKind::Wild | PatKind::Never | PatKind::Error(_) => {}

            PatKind::Deref { pin: crate::mir::build::thir::Pinnedness::Pinned, subpattern } => {
                // Project into the `Pin(_)` struct, then deref the inner `&` or `&mut`.
                self.visit_primary_bindings_special(subpattern, &user_tys.leaf(crate::mir::FieldIdx(0)).deref(), f)?;
            }
            PatKind::Deref { pin: crate::mir::build::thir::Pinnedness::Not, subpattern } => {
                self.visit_primary_bindings_special(subpattern, &user_tys.deref(), f)?;
            }

            PatKind::DerefPattern { subpattern, .. } => {
                self.visit_primary_bindings_special(subpattern, &ProjectedUserTypesNode::None, f)?;
            }

            PatKind::Leaf { subpatterns } => {
                for subpattern in subpatterns {
                    let subpattern_user_tys = user_tys.leaf(subpattern.field);
                    self.visit_primary_bindings_special(&subpattern.pattern, &subpattern_user_tys, f)?;
                }
            }

            PatKind::Variant { adt_def, args: _, variant_index, subpatterns } => {
                let name = self.root.cx.tcx.adts.get(adt_def).and_then(|a| a.variants.get(variant_index.0 as usize)).map(|v| v.name);
                for subpattern in subpatterns {
                    let subpattern_user_tys = user_tys.variant(*adt_def, name, *variant_index, subpattern.field);
                    self.visit_primary_bindings_special(&subpattern.pattern, &subpattern_user_tys, f)?;
                }
            }
            PatKind::Or { pats } => {
                // In cases where we recover from errors the primary bindings may not all be in the
                // leftmost subpattern. For example in `let (x | y) = ...`, the primary binding of
                // `y` occurs in the right subpattern
                for subpattern in pats.iter() {
                    self.visit_primary_bindings_special(subpattern, user_tys, f)?;
                }
            }
            PatKind::Guard { subpattern, .. } => {
                self.visit_primary_bindings_special(subpattern, user_tys, f)?;
            }
        }
        Ok(())
    }
}

/// Data extracted from a pattern that doesn't affect which branch is taken. Collected during
/// pattern simplification and not mutated later.
#[derive(Debug, Clone)]
struct PatternExtraData {
    /// [`Span`] of the original pattern.
    span: Span,

    /// Bindings that must be established.
    bindings: Vec<SubpatternBindings>,

    /// Types that must be asserted.
    ascriptions: Vec<Ascription>,

    /// Whether this corresponds to a never pattern.
    is_never: bool,
}

impl PatternExtraData {
    fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.ascriptions.is_empty()
    }
}

#[derive(Debug, Clone)]
enum SubpatternBindings {
    /// A single binding.
    One(Binding),
    /// Holds the place for an or-pattern's bindings. This ensures their drops are scheduled in the
    /// order the primary bindings appear. See rust-lang/rust#142163 for more information.
    FromOrPattern,
}

/// A pattern in a form suitable for lowering the match tree, with all irrefutable patterns
/// simplified away.
///
/// Here, "flat" indicates that irrefutable nodes in the pattern tree have been recursively replaced
/// with their refutable subpatterns. They are not necessarily flat in an absolute sense.
///
/// Will typically be incorporated into a [`Candidate`].
#[derive(Debug, Clone)]
struct FlatPat {
    /// To match the pattern, all of these must be satisfied...
    match_pairs: Vec<MatchPairTree>,

    extra_data: PatternExtraData,
}

impl FlatPat {
    /// Creates a `FlatPat` containing a simplified [`MatchPairTree`] list/forest for the given
    /// pattern.
    fn new(place: PlaceBuilder, pattern: &Pat, cx: &mut Builder<'_>) -> BuildResult<Self> {
        // Recursively build a tree of match pairs for the given pattern.
        let mut match_pairs = vec![];
        let mut extra_data = PatternExtraData { span: pattern.span, bindings: Vec::new(), ascriptions: Vec::new(), is_never: pattern.is_never_pattern() };
        MatchPairTree::for_pattern(place, pattern, cx, &mut match_pairs, &mut extra_data)?;

        Ok(Self { match_pairs, extra_data })
    }
}

/// Candidates are a generalization of (a) top-level match arms, and (b) sub-branches of
/// or-patterns, allowing the match-lowering process to handle them both in a mostly-uniform way.
/// For example, the list of candidates passed to [`Builder::match_candidates`] will often contain a
/// mixture of top-level candidates and or-pattern subcandidates.
///
/// At the start of match lowering, there is one candidate for each match arm. During match
/// lowering, arms with or-patterns will be expanded into a tree of candidates, where each "leaf"
/// candidate represents one of the ways for the arm pattern to successfully match.
#[derive(Debug)]
struct Candidate {
    /// For the candidate to match, all of these must be satisfied...
    ///
    /// ---
    /// Initially contains a list of match pairs created by [`FlatPat`], but is subsequently mutated
    /// (in a queue-like way) while lowering the match tree. When this list becomes empty, the
    /// candidate is fully matched and becomes a leaf (see [`Builder::select_matched_candidate`]).
    ///
    /// Key mutations include:
    ///
    /// - When a match pair is fully satisfied by a test, it is removed from the list, and its
    ///   subpairs are added instead (see [`Builder::choose_bucket_for_candidate`]).
    /// - During or-pattern expansion, any leading or-pattern is removed, and is converted into
    ///   subcandidates (see [`Builder::expand_and_match_or_candidates`]).
    /// - After a candidate's subcandidates have been lowered, a copy of any remaining or-patterns
    ///   is added to each leaf subcandidate (see [`Builder::test_remaining_match_pairs_after_or`]).
    ///
    /// Invariants:
    /// - All or-patterns ([`TestableCase::Or`]) have been sorted to the end.
    match_pairs: Vec<MatchPairTree>,

    /// ...and if this is non-empty, one of these subcandidates also has to match...
    ///
    /// ---
    /// Initially a candidate has no subcandidates; they are added (and then immediately lowered)
    /// during or-pattern expansion. Their main function is to serve as _output_ of match tree
    /// lowering, allowing later steps to see the leaf candidates that represent a match of the
    /// entire match arm.
    ///
    /// A candidate no subcandidates is either incomplete (if it has match pairs left), or is a leaf
    /// in the match tree. A candidate with one or more subcandidates is an internal node in the
    /// match tree.
    ///
    /// Invariant: at the end of match tree lowering, this must not contain an `is_never` candidate,
    /// because that would break binding consistency.
    /// - See [`Builder::remove_never_subcandidates`].
    subcandidates: Vec<Candidate>,

    /// ...and if there is a guard it must be evaluated; if it's `false` then branch to
    /// `otherwise_block`.
    ///
    /// ---
    /// For subcandidates, this is copied from the parent candidate, so it indicates whether the
    /// enclosing match arm has a guard.
    has_guard: bool,

    /// Holds extra pattern data that was prepared by [`FlatPat`], including bindings and
    /// ascriptions that must be established if this candidate succeeds.
    extra_data: PatternExtraData,

    /// When setting `self.subcandidates`, we store here the span of the or-pattern they came from.
    ///
    /// ---
    /// Invariant: it is `None` iff `subcandidates.is_empty()`.
    /// - FIXME: We sometimes don't unset this when clearing `subcandidates`.
    or_span: Option<Span>,

    /// The block before the `bindings` have been established.
    ///
    /// After the match tree has been lowered, [`Builder::lower_match_arms`] will use this as the
    /// start point for lowering bindings and guards, and then jump to a shared block containing the
    /// arm body.
    pre_binding_block: Option<BasicBlock>,

    /// The block to branch to if the guard or a nested candidate fails to match.
    otherwise_block: Option<BasicBlock>,

    /// The earliest block that has only candidates >= this one as descendents. Used for false
    /// edges, see the doc for [`Builder::match_expr`].
    false_edge_start_block: Option<BasicBlock>,
}

impl Candidate {
    fn new(place: PlaceBuilder, pattern: &Pat, has_guard: HasMatchGuard, cx: &mut Builder<'_>) -> BuildResult<Self> {
        // Use `FlatPat` to build simplified match pairs, then immediately incorporate them into a
        // new candidate.
        Ok(Self::from_flat_pat(FlatPat::new(place, pattern, cx)?, matches!(has_guard, HasMatchGuard::Yes)))
    }

    /// Incorporates an already-simplified [`FlatPat`] into a new candidate.
    fn from_flat_pat(flat_pat: FlatPat, has_guard: bool) -> Self {
        let mut this = Candidate {
            match_pairs: flat_pat.match_pairs,
            extra_data: flat_pat.extra_data,
            has_guard,
            subcandidates: Vec::new(),
            or_span: None,
            otherwise_block: None,
            pre_binding_block: None,
            false_edge_start_block: None,
        };
        this.sort_match_pairs();
        this
    }

    /// Restores the invariant that or-patterns must be sorted to the end.
    fn sort_match_pairs(&mut self) {
        self.match_pairs.sort_by_key(|pair| matches!(pair.testable_case, TestableCase::Or { .. }));
    }

    /// Returns whether the first match pair of this candidate is an or-pattern.
    fn starts_with_or_pattern(&self) -> bool {
        matches!(&*self.match_pairs, [MatchPairTree { testable_case: TestableCase::Or { .. }, .. }, ..])
    }

    /// Visit the leaf candidates (those with no subcandidates) contained in this candidate.
    fn visit_leaves(&mut self, visit_leaf: &mut impl FnMut(&mut Self) -> BuildResult<()>) -> BuildResult<()> {
        if self.subcandidates.is_empty() {
            visit_leaf(self)
        } else {
            for child in self.subcandidates.iter_mut() {
                child.visit_leaves(visit_leaf)?;
            }
            Ok(())
        }
    }

    /// Visit the leaf candidates in reverse order.
    fn visit_leaves_rev(&mut self, visit_leaf: &mut impl FnMut(&mut Self) -> BuildResult<()>) -> BuildResult<()> {
        if self.subcandidates.is_empty() {
            visit_leaf(self)
        } else {
            for child in self.subcandidates.iter_mut().rev() {
                child.visit_leaves_rev(visit_leaf)?;
            }
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
struct Binding {
    span: Span,
    source: Place,
    var_id: LocalVarId,
    binding_mode: BindingMode,
    is_shorthand: bool,
}

/// Indicates that the type of `source` must be a subtype of the user-given type `user_ty`; this is
/// basically a no-op but can influence region inference.
#[derive(Clone, Debug)]
struct Ascription {
    source: Place,
    annotation: CanonicalUserTypeAnnotation,
    variance: Variance,
}

/// Partial summary of a [`thir::Pat`], indicating what sort of test should be performed to
/// match/reject the pattern, and what the desired test outcome is. This avoids having to perform a
/// full match on [`thir::PatKind`] in some places, and helps [`TestKind::Switch`] and
/// [`TestKind::SwitchInt`] know what target values to use.
///
/// Created by [`MatchPairTree::for_pattern`], and then inspected primarily by:
/// - [`Builder::pick_test_for_match_pair`] (to choose a test)
/// - [`Builder::choose_bucket_for_candidate`] (to see how the test interacts with a match pair)
///
/// Note that or-patterns are not tested directly like the other variants. Instead they participate
/// in or-pattern expansion, where they are transformed into subcandidates. See
/// [`Builder::expand_and_match_or_candidates`].
#[derive(Debug, Clone)]
enum TestableCase {
    Variant { adt_def: DefId, variant_index: VariantIdx },
    Constant { value: crate::consts::Value, kind: PatConstKind },
    Range(Arc<crate::mir::build::thir::PatRange>),
    Slice { len: u64, op: SliceLenOp },
    Deref { temp: Place, mutability: Mutability },
    Never,
    Or { pats: Box<[FlatPat]> },
}

impl TestableCase {
    fn as_range(&self) -> Option<&crate::mir::build::thir::PatRange> {
        if let Self::Range(v) = self { Some(v.as_ref()) } else { None }
    }
}

/// Sub-classification of [`TestableCase::Constant`], which helps to avoid some redundant ad-hoc
/// checks when preparing and lowering tests.
#[derive(Debug, Clone)]
enum PatConstKind {
    /// The primitive `bool` type, which is like an integer but simpler, having only two values.
    Bool,
    /// Primitive unsigned/signed integer types, plus `char`. These types interact nicely with
    /// `SwitchInt`.
    IntOrChar,
    /// Floating-point primitives, e.g. `f32`, `f64`. These types don't support `SwitchInt` and
    /// require an equality test, but can also interact with range pattern tests.
    Float,
    /// Constant string values, tested via string equality.
    String,
    /// Any other constant-pattern is usually tested via some kind of equality check. Types that
    /// might be encountered here include:
    /// - raw pointers derived from integer values
    /// - pattern types, e.g. `pattern_type!(u32 is 1..)`
    Other,
}

/// Node in a tree of "match pairs", where each pair consists of a place to be tested, and a test to
/// perform on that place.
///
/// Each node also has a list of subpairs (possibly empty) that must also match, and some additional
/// information from the THIR pattern it represents.
#[derive(Debug, Clone)]
struct MatchPairTree {
    /// This place...
    ///
    /// ---
    /// This can be `None` if it referred to a non-captured place in a closure.
    ///
    /// Invariant: Can only be `None` when `testable_case` is `Or`. Therefore this must be `Some(_)`
    /// after or-pattern expansion.
    place: Option<Place>,

    /// ... must pass this test...
    testable_case: TestableCase,

    /// ... and these subpairs must match.
    ///
    /// ---
    /// Subpairs typically represent tests that can only be performed after their parent has
    /// succeeded. For example, the pattern `Some(3)` might have an outer match pair that tests for
    /// the variant `Some`, and then a subpair that tests its field for the value `3`.
    subpairs: Vec<Self>,

    /// Span field of the THIR pattern this node was created from.
    pattern_span: Span,
}

/// A runtime test to perform to determine which candidates match a scrutinee place.
///
/// The kind of test to perform is indicated by [`TestKind`].
#[derive(Debug)]
struct Test {
    span: Span,
    kind: TestKind,
}

/// The kind of runtime test to perform to determine which candidates match a scrutinee place. This
/// is the main component of [`Test`].
///
/// Some of these variants don't contain the constant value(s) being tested against, because those
/// values are stored in the corresponding bucketed candidates instead.
#[derive(Clone, Debug, PartialEq)]
enum TestKind {
    /// Test what enum variant a value is.
    ///
    /// The subset of expected variants is not stored here; instead they are extracted from the
    /// [`TestableCase`]s of the candidates participating in the test.
    Switch {
        /// The enum type being tested.
        adt_def: DefId,
    },

    /// Test what value an integer or `char` has.
    ///
    /// The test's target values are not stored here; instead they are extracted from the
    /// [`TestableCase`]s of the candidates participating in the test.
    SwitchInt,

    /// Test whether a `bool` is `true` or `false`.
    If,

    /// Tests the place against a string constant using string equality.
    StringEq {
        /// Constant string value to test against. Note that this value has type `str` (not `&str`).
        value: crate::consts::Value,
    },

    /// Tests the place against a constant using scalar equality.
    ScalarEq { value: crate::consts::Value },

    /// Test whether the value falls within an inclusive or exclusive range.
    Range(Arc<crate::mir::build::thir::PatRange>),

    /// Test that the length of the slice is `== len` or `>= len`.
    SliceLen { len: u64, op: SliceLenOp },

    /// Call `Deref::deref[_mut]` on the value.
    Deref {
        /// Temporary to store the result of `deref()`/`deref_mut()`.
        temp: Place,
        mutability: Mutability,
    },

    /// Assert unreachability of never patterns.
    Never,
}

/// Indicates the kind of slice-length constraint imposed by a slice pattern, or its corresponding
/// test.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SliceLenOp {
    /// The slice pattern can only match a slice with exactly `len` elements.
    Equal,
    /// The slice pattern can match a slice with `len` or more elements (i.e. it contains a `..`
    /// subpattern in the middle).
    GreaterOrEqual,
}

/// The branch to be taken after a test.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TestBranch {
    /// Success branch, used for tests with two possible outcomes.
    Success,
    /// Branch corresponding to this constant. Must be a scalar.
    Constant(crate::consts::Value),
    /// Branch corresponding to this variant.
    Variant(VariantIdx),
    /// Failure branch for tests with two possible outcomes, and "otherwise" branch for other tests.
    Failure,
}

impl TestBranch {
    fn as_constant(&self) -> Option<&crate::consts::Value> {
        if let Self::Constant(v) = self { Some(v) } else { None }
    }
}

/// `ArmHasGuard` is a wrapper around a boolean flag. It indicates whether a match arm has a guard
/// expression attached to it.
#[derive(Copy, Clone, Debug)]
pub struct ArmHasGuard(pub bool);

///////////////////////////////////////////////////////////////////////////
// Main matching algorithm

/// A sub-branch in the output of match lowering. Match lowering has generated MIR code that will
/// branch to `success_block` when the matched value matches the corresponding pattern. If there is
/// a guard, its failure must continue to `otherwise_block`, which will resume testing patterns.
#[derive(Debug, Clone)]
struct MatchTreeSubBranch {
    span: Span,
    /// The block that is branched to if the corresponding subpattern matches.
    success_block: BasicBlock,
    /// The block to branch to if this arm had a guard and the guard fails.
    otherwise_block: BasicBlock,
    /// The bindings to set up in this sub-branch.
    bindings: Vec<Binding>,
    /// The ascriptions to set up in this sub-branch.
    ascriptions: Vec<Ascription>,
    /// Whether the sub-branch corresponds to a never pattern.
    is_never: bool,
}

/// A branch in the output of match lowering.
#[derive(Debug, Clone)]
struct MatchTreeBranch {
    sub_branches: Vec<MatchTreeSubBranch>,
}

/// The result of generating MIR for a pattern-matching expression. Each input branch/arm/pattern
/// gives rise to an output `MatchTreeBranch`. If one of the patterns matches, we branch to the
/// corresponding `success_block`. If none of the patterns matches, we branch to `otherwise_block`.
///
/// Each branch is made of one of more sub-branches, corresponding to or-patterns. E.g.
/// ```ignore(illustrative)
/// match foo {
///     (x, false) | (false, x) => {}
///     (true, true) => {}
/// }
/// ```
/// Here the first arm gives the first `MatchTreeBranch`, which has two sub-branches, one for each
/// alternative of the or-pattern. They are kept separate because each needs to bind `x` to a
/// different place.
#[derive(Debug, Clone)]
pub struct BuiltMatchTree {
    branches: Vec<MatchTreeBranch>,
    otherwise_block: BasicBlock,
    /// If any of the branches had a guard, we collect here the places and locals to fakely borrow
    /// to ensure match guards can't modify the values as we match them. For more details, see
    /// [`util::collect_fake_borrows`].
    fake_borrow_temps: Vec<(Place, Local, FakeBorrowKind)>,
}

impl MatchTreeSubBranch {
    fn from_sub_candidate(candidate: Candidate, parent_data: &[PatternExtraData]) -> Self {
        debug_assert!(candidate.match_pairs.is_empty());
        MatchTreeSubBranch {
            span: candidate.extra_data.span,
            success_block: candidate.pre_binding_block.unwrap(),
            otherwise_block: candidate.otherwise_block.unwrap(),
            bindings: sub_branch_bindings(parent_data, &candidate.extra_data.bindings),
            ascriptions: parent_data.iter().flat_map(|d| &d.ascriptions).cloned().chain(candidate.extra_data.ascriptions).collect(),
            is_never: candidate.extra_data.is_never,
        }
    }
}

impl MatchTreeBranch {
    fn from_candidate(candidate: Candidate) -> Self {
        fn collect(candidate: Candidate, parent_data: &mut Vec<PatternExtraData>, out: &mut Vec<MatchTreeSubBranch>) {
            if candidate.subcandidates.is_empty() {
                out.push(MatchTreeSubBranch::from_sub_candidate(candidate, parent_data));
            } else {
                parent_data.push(candidate.extra_data);
                for child in candidate.subcandidates {
                    collect(child, parent_data, out);
                }
                parent_data.pop();
            }
        }
        let mut sub_branches = Vec::new();
        collect(candidate, &mut Vec::new(), &mut sub_branches);
        MatchTreeBranch { sub_branches }
    }
}

/// Collects the bindings for a [`MatchTreeSubBranch`], preserving the order they appear in the
/// pattern, as though the or-alternatives chosen in this sub-branch were inlined.
fn sub_branch_bindings(parents: &[PatternExtraData], leaf_bindings: &[SubpatternBindings]) -> Vec<Binding> {
    // In the common case, all bindings will be in leaves. Allocate to fit the leaf's bindings.
    let mut all_bindings = Vec::with_capacity(leaf_bindings.len());
    let mut remainder = parents.iter().map(|parent| parent.bindings.as_slice()).chain([leaf_bindings]).filter(|bindings| !bindings.is_empty());
    // Skip over unsimplified or-patterns without bindings.
    if let Some(candidate_bindings) = remainder.next() {
        push_sub_branch_bindings(&mut all_bindings, candidate_bindings, &mut remainder);
    }
    // Make sure we've included all bindings. For ill-formed patterns like `(x, _ | y)`, we may not
    // have collected all bindings yet, since we only check the first alternative when determining
    // whether to inline subcandidates' bindings. To recover, we collect the rest in an arbitrary
    // order (rustc: a delayed bug, the mismatch is an error already reported).
    while let Some(candidate_bindings) = remainder.next() {
        push_sub_branch_bindings(&mut all_bindings, candidate_bindings, &mut remainder);
    }
    all_bindings
}

/// Helper for [`sub_branch_bindings`]. Collects bindings from `candidate_bindings` into
/// `flattened`. Bindings in or-patterns are collected recursively from `remainder`.
fn push_sub_branch_bindings<'c>(
    flattened: &mut Vec<Binding>,
    candidate_bindings: &'c [SubpatternBindings],
    remainder: &mut impl Iterator<Item = &'c [SubpatternBindings]>,
) {
    for subpat_bindings in candidate_bindings {
        match subpat_bindings {
            SubpatternBindings::One(binding) => flattened.push(binding.clone()),
            SubpatternBindings::FromOrPattern => {
                // Inline bindings from an or-pattern. By construction, this always corresponds to a
                // subcandidate and its closest descendants (i.e. those from nested or-patterns, but
                // not adjacent or-patterns). To handle adjacent or-patterns, e.g. `(x | x, y | y)`,
                // we update the `remainder` to point to the first descendant candidate from outside
                // this or-pattern.
                if let Some(subcandidate_bindings) = remainder.next() {
                    push_sub_branch_bindings(flattened, subcandidate_bindings, remainder);
                }
                // else: ill-formed patterns like `x | _` have no subcandidates left to inline
                // bindings from (an error already reported).
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HasMatchGuard {
    Yes,
    No,
}

impl Builder<'_> {
    /// The entrypoint of the matching algorithm. Create the decision tree for the match expression,
    /// starting from `block`.
    ///
    /// `patterns` is a list of patterns, one for each arm. The associated boolean indicates whether
    /// the arm has a guard.
    ///
    /// `refutable` indicates whether the candidate list is refutable (for `if let` and `let else`)
    /// or not (for `let` and `match`). In the refutable case we return the block to which we branch
    /// on failure.
    pub fn lower_match_tree(
        &mut self,
        block: BasicBlock,
        scrutinee_span: Span,
        scrutinee_place_builder: &PlaceBuilder,
        match_start_span: Span,
        patterns: Vec<(&Pat, HasMatchGuard)>,
        refutable: bool,
    ) -> BuildResult<BuiltMatchTree> {
        // Assemble the initial list of candidates. These top-level candidates are 1:1 with the
        // input patterns, but other parts of match lowering also introduce subcandidates (for
        // sub-or-patterns). So inside the algorithm, the candidates list may not correspond to
        // match arms directly.
        let mut candidates: Vec<Candidate> = Vec::with_capacity(patterns.len());
        for (pat, has_guard) in patterns {
            candidates.push(Candidate::new(scrutinee_place_builder.clone(), pat, has_guard, self)?);
        }

        let fake_borrow_temps = util::collect_fake_borrows(self, &candidates, scrutinee_span, scrutinee_place_builder.base())?;

        // This will generate code to test scrutinee_place and branch to the appropriate arm block.
        // If none of the arms match, we branch to `otherwise_block`. When lowering a `match`
        // expression, exhaustiveness checking ensures that this block is unreachable.
        let mut candidate_refs = candidates.iter_mut().collect::<Vec<_>>();
        let otherwise_block = self.match_candidates(match_start_span, scrutinee_span, block, &mut candidate_refs)?;

        // Set up false edges so that the borrow-checker cannot make use of the specific CFG we
        // generated. We falsely branch from each candidate to the one below it to make it as if we
        // were testing match branches one by one in order. In the refutable case we also want a
        // false edge to the final failure block.
        let mut next_candidate_start_block = if refutable { Some(otherwise_block) } else { None };
        for candidate in candidates.iter_mut().rev() {
            let has_guard = candidate.has_guard;
            candidate.visit_leaves_rev(&mut |leaf_candidate| {
                if let Some(next_candidate_start_block) = next_candidate_start_block {
                    let source_info = self.source_info(leaf_candidate.extra_data.span);
                    // Falsely branch to `next_candidate_start_block` before reaching pre_binding.
                    let old_pre_binding = leaf_candidate.pre_binding_block.unwrap();
                    let new_pre_binding = self.cfg.start_new_block();
                    self.false_edges(old_pre_binding, new_pre_binding, next_candidate_start_block, source_info);
                    leaf_candidate.pre_binding_block = Some(new_pre_binding);
                    if has_guard {
                        // Falsely branch to `next_candidate_start_block` also if the guard fails.
                        let new_otherwise = self.cfg.start_new_block();
                        let old_otherwise = leaf_candidate.otherwise_block.unwrap();
                        self.false_edges(new_otherwise, old_otherwise, next_candidate_start_block, source_info);
                        leaf_candidate.otherwise_block = Some(new_otherwise);
                    }
                }
                assert!(leaf_candidate.false_edge_start_block.is_some());
                next_candidate_start_block = leaf_candidate.false_edge_start_block;
                Ok(())
            })?;
        }

        if !refutable {
            // Match checking ensures `otherwise_block` is actually unreachable in irrefutable cases.
            let source_info = self.source_info(scrutinee_span);

            // Matching on a scrutinee place of an uninhabited type doesn't generate any memory reads
            // by itself, and so if the place is uninitialized we wouldn't know. In order to disallow
            // the following:
            // ```rust
            // let x: !;
            // match x {}
            // ```
            // we add a dummy read on the place.
            //
            // NOTE: If we require never patterns for empty matches, those will check that the place
            // is initialized, and so this read would no longer be needed.
            let cause_matched_place = FakeReadCause::ForMatchedPlace(None);

            if let Some(scrutinee_place) = scrutinee_place_builder.try_to_place(self) {
                self.cfg.push_fake_read(otherwise_block, source_info, cause_matched_place, scrutinee_place);
            }

            self.cfg.terminate(otherwise_block, source_info, TerminatorKind::Unreachable);
        }

        Ok(BuiltMatchTree { branches: candidates.into_iter().map(MatchTreeBranch::from_candidate).collect(), otherwise_block, fake_borrow_temps })
    }

    /// The main match algorithm. It begins with a set of candidates `candidates` and has the job of
    /// generating code that branches to an appropriate block if the scrutinee matches one of these
    /// candidates. The candidates are ordered such that the first item in the list has the highest
    /// priority. When a candidate is found to match the value, we will set and generate a branch to
    /// the appropriate pre-binding block.
    ///
    /// If none of the candidates apply, we continue to the returned `otherwise_block`.
    ///
    /// Note that while `match` expressions in the Rust language are exhaustive, candidate lists
    /// passed to this method are often _non-exhaustive_. For example, the match lowering process
    /// will frequently divide up the list of candidates, and recursively call this method with a
    /// non-exhaustive subset of candidates. See [`Builder::test_candidates`] for more details on
    /// this "backtracking automata" approach.
    ///
    /// For an example of how we use `otherwise_block`, consider:
    /// ```
    /// # fn foo((x, y): (bool, bool)) -> u32 {
    /// match (x, y) {
    ///     (true, true) => 1,
    ///     (_, false) => 2,
    ///     (false, true) => 3,
    /// }
    /// # }
    /// ```
    /// For this match, we generate something like:
    /// ```
    /// # fn foo((x, y): (bool, bool)) -> u32 {
    /// if x {
    ///     if y {
    ///         return 1
    ///     } else {
    ///         // continue
    ///     }
    /// } else {
    ///     // continue
    /// }
    /// if y {
    ///     if x {
    ///         // This is actually unreachable because the `(true, true)` case was handled above,
    ///         // but we don't know that from within the lowering algorithm.
    ///         // continue
    ///     } else {
    ///         return 3
    ///     }
    /// } else {
    ///     return 2
    /// }
    /// // this is the final `otherwise_block`, which is unreachable because the match was exhaustive.
    /// unreachable!()
    /// # }
    /// ```
    ///
    /// Every `continue` is an instance of branching to some `otherwise_block` somewhere deep within
    /// the algorithm. For more details on why we lower like this, see [`Builder::test_candidates`].
    ///
    /// Note how we test `x` twice. This is the tradeoff of backtracking automata: we prefer smaller
    /// code size so we accept non-optimal code paths.
    fn match_candidates(&mut self, span: Span, scrutinee_span: Span, start_block: BasicBlock, candidates: &mut [&mut Candidate]) -> BuildResult<BasicBlock> {
        self.match_candidates_inner(span, scrutinee_span, start_block, candidates)
    }

    /// Construct the decision tree for `candidates`. Don't call this, call `match_candidates`
    /// instead to reserve sufficient stack space.
    fn match_candidates_inner(
        &mut self,
        span: Span,
        scrutinee_span: Span,
        mut start_block: BasicBlock,
        candidates: &mut [&mut Candidate],
    ) -> BuildResult<BasicBlock> {
        if let [first, ..] = candidates {
            if first.false_edge_start_block.is_none() {
                first.false_edge_start_block = Some(start_block);
            }
        }

        // Process a prefix of the candidates.
        let rest = match candidates {
            [] => {
                // If there are no candidates that still need testing, we're done.
                return Ok(start_block);
            }
            [first, remaining @ ..] if first.match_pairs.is_empty() => {
                // The first candidate has satisfied all its match pairs. We record the blocks that
                // will be needed by match arm lowering, and then continue with the remaining
                // candidates.
                let remainder_start = self.select_matched_candidate(first, start_block);
                remainder_start.and(remaining)
            }
            candidates if candidates.iter().any(|candidate| candidate.starts_with_or_pattern()) => {
                // If any candidate starts with an or-pattern, we want to expand or-patterns before
                // we do any more tests.
                //
                // The only candidate we strictly _need_ to expand here is the first one. But by
                // expanding other candidates as early as possible, we unlock more opportunities to
                // include them in test outcomes, making the match tree smaller and simpler.
                self.expand_and_match_or_candidates(span, scrutinee_span, start_block, candidates)?
            }
            candidates => {
                // The first candidate has some unsatisfied match pairs; we proceed to do more tests.
                self.test_candidates(span, scrutinee_span, candidates, start_block)?
            }
        };

        // Process any candidates that remain.
        let BlockAnd(next_block, remaining_candidates) = rest;
        start_block = next_block;
        self.match_candidates(span, scrutinee_span, start_block, remaining_candidates)
    }

    /// Link up matched candidates.
    ///
    /// For example, if we have something like this:
    ///
    /// ```ignore (illustrative)
    /// ...
    /// Some(x) if cond1 => ...
    /// Some(x) => ...
    /// Some(x) if cond2 => ...
    /// ...
    /// ```
    ///
    /// We generate real edges from:
    ///
    /// * `start_block` to the [pre-binding block] of the first pattern,
    /// * the [otherwise block] of the first pattern to the second pattern,
    /// * the [otherwise block] of the third pattern to a block with an
    ///   [`Unreachable` terminator](TerminatorKind::Unreachable).
    ///
    /// In addition, we later add fake edges from the otherwise blocks to the pre-binding block of
    /// the next candidate in the original set of candidates.
    ///
    /// [pre-binding block]: Candidate::pre_binding_block
    /// [otherwise block]: Candidate::otherwise_block
    fn select_matched_candidate(&mut self, candidate: &mut Candidate, start_block: BasicBlock) -> BasicBlock {
        assert!(candidate.otherwise_block.is_none());
        assert!(candidate.pre_binding_block.is_none());
        assert!(candidate.subcandidates.is_empty());

        candidate.pre_binding_block = Some(start_block);
        let otherwise_block = self.cfg.start_new_block();
        // Create the otherwise block for this candidate, which is the pre-binding block for the next
        // candidate.
        candidate.otherwise_block = Some(otherwise_block);
        otherwise_block
    }

    /// Takes a list of candidates such that some of the candidates' first match pairs are
    /// or-patterns. This expands as many or-patterns as possible and processes the resulting
    /// candidates. Returns the unprocessed candidates if any.
    fn expand_and_match_or_candidates<'b, 'c>(
        &mut self,
        span: Span,
        scrutinee_span: Span,
        start_block: BasicBlock,
        candidates: &'b mut [&'c mut Candidate],
    ) -> BuildResult<BlockAnd<&'b mut [&'c mut Candidate]>> {
        // We can't expand or-patterns freely. The rule is:
        // - If a candidate doesn't start with an or-pattern, we include it in the expansion list
        //   as-is (i.e. it "expands" to itself).
        // - If a candidate has an or-pattern as its only remaining match pair, we can expand it.
        // - If it starts with an or-pattern but also has other match pairs, we can expand it, but
        //   we can't process more candidates after it.
        //
        // If we didn't stop, the `otherwise` cases could get mixed up. E.g. in the following,
        // or-pattern simplification (in `merge_trivial_subcandidates`) makes it so the `1` and `2`
        // cases branch to a same block (which then tests `false`). If we took `(2, _)` in the same
        // set of candidates, when we reach the block that tests `false` we don't know whether we
        // came from `1` or `2`, hence we can't know where to branch on failure.
        //
        // ```ignore(illustrative)
        // match (1, true) {
        //     (1 | 2, false) => {},
        //     (2, _) => {},
        //     _ => {}
        // }
        // ```
        //
        // We therefore split the `candidates` slice in two, expand or-patterns in the first part,
        // and process the rest separately.
        let expand_until = candidates
            .iter()
            .position(|candidate| {
                // If a candidate starts with an or-pattern and has more match pairs, we can expand
                // it, but we must stop expanding _after_ it.
                candidate.match_pairs.len() > 1 && candidate.starts_with_or_pattern()
            })
            .map(|pos| pos + 1) // Stop _after_ the found candidate
            .unwrap_or(candidates.len()); // Otherwise, include all candidates
        let (candidates_to_expand, remaining_candidates) = candidates.split_at_mut(expand_until);

        // Expand one level of or-patterns for each candidate in `candidates_to_expand`. We take care
        // to preserve the relative ordering of candidates, so that or-patterns are expanded in their
        // parent's relative position.
        let mut expanded_candidates = Vec::new();
        for candidate in candidates_to_expand.iter_mut() {
            if candidate.starts_with_or_pattern() {
                let or_match_pair = candidate.match_pairs.remove(0);
                // Expand the or-pattern into subcandidates.
                self.create_or_subcandidates(candidate, or_match_pair)?;
                // Collect the newly created subcandidates.
                for subcandidate in candidate.subcandidates.iter_mut() {
                    expanded_candidates.push(subcandidate);
                }
                // Note that the subcandidates have been added to `expanded_candidates`, but
                // `candidate` itself has not. If the last candidate has more match pairs, they are
                // handled separately by `test_remaining_match_pairs_after_or`.
            } else {
                // A candidate that doesn't start with an or-pattern has nothing to expand, so it is
                // included in the post-expansion list as-is.
                expanded_candidates.push(candidate);
            }
        }

        // Recursively lower the part of the match tree represented by the expanded candidates. This
        // is where subcandidates actually get lowered!
        let remainder_start = self.match_candidates(span, scrutinee_span, start_block, expanded_candidates.as_mut_slice())?;

        // Postprocess subcandidates, and process any leftover match pairs. (Only the last candidate
        // can possibly have more match pairs.)
        debug_assert!({
            let mut all_except_last = candidates_to_expand.iter().rev().skip(1);
            all_except_last.all(|candidate| candidate.match_pairs.is_empty())
        });
        for candidate in candidates_to_expand.iter_mut() {
            if !candidate.subcandidates.is_empty() {
                self.merge_trivial_subcandidates(candidate);
                self.remove_never_subcandidates(candidate);
            }
        }
        // It's important to perform the above simplifications _before_ dealing with remaining match
        // pairs, to avoid exponential blowup if possible (for trivial or-patterns), and avoid
        // useless work (for never patterns).
        if let Some(last_candidate) = candidates_to_expand.last_mut() {
            self.test_remaining_match_pairs_after_or(span, scrutinee_span, last_candidate)?;
        }

        Ok(remainder_start.and(remaining_candidates))
    }

    /// Given a match-pair that corresponds to an or-pattern, expand each subpattern into a new
    /// subcandidate. Any candidate that has been expanded this way should also be postprocessed at
    /// the end of [`Self::expand_and_match_or_candidates`].
    fn create_or_subcandidates(&mut self, candidate: &mut Candidate, match_pair: MatchPairTree) -> BuildResult<()> {
        let TestableCase::Or { pats } = match_pair.testable_case else {
            return Err(self.error(match_pair.pattern_span, UnsupportedReason::UnresolvedFact("create_or_subcandidates on a non-or pair")));
        };
        candidate.or_span = Some(match_pair.pattern_span);
        candidate.subcandidates = Vec::from(pats).into_iter().map(|flat_pat| Candidate::from_flat_pat(flat_pat, candidate.has_guard)).collect();
        candidate.subcandidates[0].false_edge_start_block = candidate.false_edge_start_block;
        Ok(())
    }

    /// Try to merge all of the subcandidates of the given candidate into one. This avoids
    /// exponentially large CFGs in cases like `(1 | 2, 3 | 4, ...)`. The candidate should have been
    /// expanded with `create_or_subcandidates`.
    ///
    /// Given a pattern `(P | Q, R | S)` we (in principle) generate a CFG like so:
    ///
    /// ```text
    /// [ start ]
    ///      |
    /// [ match P, Q ]
    ///      |
    ///      +----------------------------------------+------------------------------------+
    ///      |                                        |                                    |
    ///      V                                        V                                    V
    /// [ P matches ]                           [ Q matches ]                        [ otherwise ]
    ///      |                                        |                                    |
    ///      V                                        V                                    |
    /// [ match R, S ]                          [ match R, S ]                             |
    ///      |                                        |                                    |
    ///      +--------------+------------+            +--------------+------------+        |
    ///      |              |            |            |              |            |        |
    ///      V              V            V            V              V            V        |
    /// [ R matches ] [ S matches ] [otherwise ] [ R matches ] [ S matches ] [otherwise ]  |
    ///      |              |            |            |              |            |        |
    ///      +--------------+------------|------------+--------------+            |        |
    ///      |                           |                                        |        |
    ///      |                           +----------------------------------------+--------+
    ///      |                           |
    ///      V                           V
    /// [ Success ]                 [ Failure ]
    /// ```
    ///
    /// In practice there are some complications:
    ///
    /// * If there's a guard, then the otherwise branch of the first match on `R | S` goes to a test
    ///   for whether `Q` matches, and the control flow doesn't merge into a single success block
    ///   until after the guard is tested.
    /// * If neither `P` or `Q` has any bindings or type ascriptions and there isn't a match guard,
    ///   then we create a smaller CFG like:
    ///
    /// ```text
    ///     ...
    ///      +---------------+------------+
    ///      |               |            |
    /// [ P matches ] [ Q matches ] [ otherwise ]
    ///      |               |            |
    ///      +---------------+            |
    ///      |                           ...
    /// [ match R, S ]
    ///      |
    ///     ...
    /// ```
    ///
    /// Note that this takes place _after_ the subcandidates have participated in match tree
    /// lowering.
    fn merge_trivial_subcandidates(&mut self, candidate: &mut Candidate) {
        assert!(!candidate.subcandidates.is_empty());
        if candidate.has_guard {
            // FIXME(or_patterns; matthewjasper) Don't give up if we have a guard.
            return;
        }

        // FIXME(or_patterns; matthewjasper) Try to be more aggressive here.
        let can_merge = candidate.subcandidates.iter().all(|subcandidate| subcandidate.subcandidates.is_empty() && subcandidate.extra_data.is_empty());
        if !can_merge {
            return;
        }

        let mut last_otherwise = None;
        let shared_pre_binding_block = self.cfg.start_new_block();
        // This candidate is about to become a leaf, so unset `or_span`.
        let or_span = candidate.or_span.take().unwrap();
        let source_info = self.source_info(or_span);

        if candidate.false_edge_start_block.is_none() {
            candidate.false_edge_start_block = candidate.subcandidates[0].false_edge_start_block;
        }

        // Remove the (known-trivial) subcandidates from the candidate tree, so that they aren't
        // visible after match tree lowering, and wire them all to join up at a single shared
        // pre-binding block. (Note that the subcandidates have already had their part of the match
        // tree lowered by this point, which is why we can add a goto to them.)
        for subcandidate in mem::take(&mut candidate.subcandidates) {
            let subcandidate_block = subcandidate.pre_binding_block.unwrap();
            self.cfg.goto(subcandidate_block, source_info, shared_pre_binding_block);
            last_otherwise = subcandidate.otherwise_block;
        }
        candidate.pre_binding_block = Some(shared_pre_binding_block);
        assert!(last_otherwise.is_some());
        candidate.otherwise_block = last_otherwise;
    }

    /// Never subcandidates may have a set of bindings inconsistent with their siblings, which would
    /// break later code. So we filter them out. Note that we can't filter out top-level candidates
    /// this way.
    fn remove_never_subcandidates(&mut self, candidate: &mut Candidate) {
        if candidate.subcandidates.is_empty() {
            return;
        }

        let false_edge_start_block = candidate.subcandidates[0].false_edge_start_block;
        let subcandidates = mem::take(&mut candidate.subcandidates);
        candidate.subcandidates = subcandidates
            .into_iter()
            .filter_map(|mut subcandidate| {
                if subcandidate.extra_data.is_never {
                    let _ = subcandidate.visit_leaves(&mut |leaf| {
                        let block = leaf.pre_binding_block.unwrap();
                        // That block is already unreachable but needs a terminator to make the MIR
                        // well-formed.
                        let source_info = self.source_info(leaf.extra_data.span);
                        self.cfg.terminate(block, source_info, TerminatorKind::Unreachable);
                        Ok(())
                    });
                    None
                } else {
                    Some(subcandidate)
                }
            })
            .collect();
        if candidate.subcandidates.is_empty() {
            // If `candidate` has become a leaf candidate, ensure it has a `pre_binding_block` and
            // `otherwise_block`.
            let next_block = self.cfg.start_new_block();
            candidate.pre_binding_block = Some(next_block);
            candidate.otherwise_block = Some(next_block);
            // In addition, if `candidate` doesn't have `false_edge_start_block`, it should be
            // assigned here.
            if candidate.false_edge_start_block.is_none() {
                candidate.false_edge_start_block = false_edge_start_block;
            }
        }
    }

    /// If more match pairs remain, test them after each subcandidate. We could have added them to
    /// the or-candidates during or-pattern expansion, but that would make it impossible to detect
    /// simplifiable or-patterns. That would guarantee exponentially large CFGs for cases like
    /// `(1 | 2, 3 | 4, ...)`.
    fn test_remaining_match_pairs_after_or(&mut self, span: Span, scrutinee_span: Span, candidate: &mut Candidate) -> BuildResult<()> {
        if candidate.match_pairs.is_empty() {
            return Ok(());
        }

        let or_span = candidate.or_span.unwrap_or(candidate.extra_data.span);
        let source_info = self.source_info(or_span);
        let mut last_otherwise = None;
        candidate.visit_leaves(&mut |leaf_candidate| {
            last_otherwise = leaf_candidate.otherwise_block;
            Ok(())
        })?;

        let remaining_match_pairs = mem::take(&mut candidate.match_pairs);
        // We're testing match pairs that remained after an `Or`, so the remaining pairs should all
        // be `Or` too, due to the sorting invariant.
        debug_assert!(remaining_match_pairs.iter().all(|match_pair| matches!(match_pair.testable_case, TestableCase::Or { .. })));

        // Visit each leaf candidate within this subtree, add a copy of the remaining match pairs to
        // it, and then recursively lower the rest of the match tree from that point.
        candidate.visit_leaves(&mut |leaf_candidate| {
            // At this point the leaf's own match pairs have all been lowered and removed, so
            // `extend` and assignment are equivalent, but extending can also recycle any existing
            // vector capacity.
            assert!(leaf_candidate.match_pairs.is_empty());
            leaf_candidate.match_pairs.extend(remaining_match_pairs.iter().cloned());

            let or_start = leaf_candidate.pre_binding_block.unwrap();
            let otherwise = self.match_candidates(span, scrutinee_span, or_start, &mut [&mut *leaf_candidate])?;
            // In a case like `(P | Q, R | S)`, if `P` succeeds and `R | S` fails, we know `(Q, R |
            // S)` will fail too. If there is no guard, we skip testing of `Q` by branching directly
            // to `last_otherwise`. If there is a guard, `leaf_candidate.otherwise_block` can be
            // reached by guard failure as well, so we can't skip `Q`.
            let or_otherwise = if leaf_candidate.has_guard { leaf_candidate.otherwise_block.unwrap() } else { last_otherwise.unwrap() };
            self.cfg.goto(otherwise, source_info, or_otherwise);
            Ok(())
        })
    }

    /// Pick a test to run. Which test doesn't matter as long as it is guaranteed to fully match at
    /// least one match pair. We currently simply pick the test corresponding to the first match pair
    /// of the first candidate in the list.
    ///
    /// *Note:* taking the first match pair is somewhat arbitrary, and we might do better here by
    /// choosing more carefully what to test.
    ///
    /// For example, consider the following possible match-pairs:
    ///
    /// 1. `x @ Some(P)` -- we will do a [`Switch`] to decide what variant `x` has
    /// 2. `x @ 22` -- we will do a [`SwitchInt`] to decide what value `x` has
    /// 3. `x @ 3..5` -- we will do a [`Range`] test to decide what range `x` falls in
    /// 4. etc.
    ///
    /// [`Switch`]: TestKind::Switch
    /// [`SwitchInt`]: TestKind::SwitchInt
    /// [`Range`]: TestKind::Range
    fn pick_test(&mut self, candidates: &[&mut Candidate]) -> BuildResult<(Place, Test)> {
        // Extract the match-pair from the highest priority candidate
        let match_pair = &candidates[0].match_pairs[0];
        let test = self.pick_test_for_match_pair(match_pair)?;
        // Unwrap is ok after simplification.
        let match_place = match_pair.place.clone().unwrap();

        Ok((match_place, test))
    }

    /// This is the most subtle part of the match lowering algorithm. At this point, there are no
    /// fully-satisfied candidates, and no or-patterns to expand, so we actually need to perform some
    /// sort of test to make progress.
    ///
    /// Once we pick what sort of test we are going to perform, this test will help us winnow down
    /// our candidates. So we walk over the candidates (from high to low priority) and check. We
    /// compute, for each outcome of the test, a list of (modified) candidates. If a candidate
    /// matches in exactly one branch of our test, we add it to the corresponding outcome. We also
    /// **mutate its list of match pairs** if appropriate, to reflect the fact that we know which
    /// outcome occurred.
    ///
    /// For example, if we are testing `x.0`'s variant, and we have a candidate `(x.0 @ Some(v), x.1
    /// @ 22)`, then we would have a resulting candidate of `((x.0 as Some).0 @ v, x.1 @ 22)` in the
    /// branch corresponding to `Some`. To ensure we make progress, we always pick a test that
    /// results in simplifying the first candidate.
    ///
    /// But there may also be candidates that the test doesn't apply to. The classical example is
    /// wildcards:
    ///
    /// ```
    /// # let (x, y, z) = (true, true, true);
    /// match (x, y, z) {
    ///     (true , _    , true ) => true,  // (0)
    ///     (false, false, _    ) => false, // (1)
    ///     (_    , true , _    ) => true,  // (2)
    ///     (true , _    , false) => false, // (3)
    /// }
    /// # ;
    /// ```
    ///
    /// Here, the traditional "decision tree" method would generate 2 separate code-paths for the 2
    /// possible values of `x`. This would however duplicate some candidates, which would need to be
    /// lowered several times.
    ///
    /// In some cases, this duplication can create an exponential amount of code. This is most easily
    /// seen by noticing that this method terminates with precisely the reachable arms being
    /// reachable - but that problem is trivially NP-complete.
    ///
    /// In rustc, we opt instead for the "backtracking automaton" approach. This guarantees we never
    /// duplicate a candidate (except in the presence of or-patterns). In fact this guarantee is
    /// ensured by the fact that we carry around `&mut Candidate`s which can't be duplicated.
    ///
    /// To make this work, whenever we decide to perform a test, if we encounter a candidate that
    /// could match in more than one branch of the test, we stop. We generate code for the test and
    /// for the candidates in its branches; the remaining candidates will be tested if the
    /// candidates in the branches fail to match.
    ///
    /// For example, if we test on `x` in the following:
    /// ```
    /// # fn foo((x, y, z): (bool, bool, bool)) -> u32 {
    /// match (x, y, z) {
    ///     (true , _    , true ) => 0,
    ///     (false, false, _    ) => 1,
    ///     (_    , true , _    ) => 2,
    ///     (true , _    , false) => 3,
    /// }
    /// # }
    /// ```
    /// this function generates code that looks more of less like:
    /// ```
    /// # fn foo((x, y, z): (bool, bool, bool)) -> u32 {
    /// if x {
    ///     match (y, z) {
    ///         (_, true) => return 0,
    ///         _ => {} // continue matching
    ///     }
    /// } else {
    ///     match (y, z) {
    ///         (false, _) => return 1,
    ///         _ => {} // continue matching
    ///     }
    /// }
    /// // the block here is `remainder_start`
    /// match (x, y, z) {
    ///     (_    , true , _    ) => 2,
    ///     (true , _    , false) => 3,
    ///     _ => unreachable!(),
    /// }
    /// # }
    /// ```
    ///
    /// We return the unprocessed candidates.
    fn test_candidates<'b, 'c>(
        &mut self,
        span: Span,
        scrutinee_span: Span,
        candidates: &'b mut [&'c mut Candidate],
        start_block: BasicBlock,
    ) -> BuildResult<BlockAnd<&'b mut [&'c mut Candidate]>> {
        // Choose a match pair from the first candidate, and use it to determine a test to perform
        // that will confirm or refute that match pair.
        let (match_place, test) = self.pick_test(candidates)?;

        // For each of the N possible test outcomes, build the vector of candidates that applies if
        // the test has that particular outcome. This also mutates the candidates to remove match
        // pairs that are fully satisfied by the relevant outcome.
        let PartitionedCandidates { target_candidates, remaining_candidates } = self.partition_candidates_into_buckets(&match_place, &test, candidates)?;

        // The block that we should branch to if none of the `target_candidates` match.
        let remainder_start = self.cfg.start_new_block();

        // For each outcome of the test, recursively lower the rest of the match tree from that
        // point. (Note that we haven't lowered the actual test yet!)
        let mut target_blocks: OrderedMap<TestBranch, BasicBlock> = OrderedMap::new();
        for (branch, mut candidates) in target_candidates.into_entries() {
            let branch_start = self.cfg.start_new_block();
            // Recursively lower the rest of the match tree after the relevant outcome.
            let branch_otherwise = self.match_candidates(span, scrutinee_span, branch_start, &mut *candidates)?;

            // Link up the `otherwise` block of the subtree to `remainder_start`.
            let source_info = self.source_info(span);
            self.cfg.goto(branch_otherwise, source_info, remainder_start);
            target_blocks.insert(branch, branch_start);
        }

        // Perform the chosen test, branching to one of the N subtrees prepared above (or to
        // `remainder_start` if no outcome was satisfied).
        self.perform_test(span, scrutinee_span, start_block, remainder_start, match_place, &test, target_blocks)?;

        Ok(remainder_start.and(remaining_candidates))
    }
}

///////////////////////////////////////////////////////////////////////////
// Pat binding - used for `let` and function parameters as well.

impl Builder<'_> {
    /// Lowers a `let` expression that appears in a suitable context (e.g. an `if` condition or
    /// match guard).
    ///
    /// Also used for lowering let-else statements, since they have similar needs despite not
    /// actually using `let` expressions.
    ///
    /// Use [`DeclareLetBindings`] to control whether the `let` bindings are declared or not.
    pub fn lower_let_expr(
        &mut self,
        mut block: BasicBlock,
        expr_id: ExprId,
        pat: &Pat,
        source_scope: Option<SourceScope>,
        scope_span: Span,
        declare_let_bindings: DeclareLetBindings,
    ) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        let expr_span = thir.thir[expr_id].span;
        let scrutinee = unpack!(block = self.lower_scrutinee(block, expr_id, expr_span));
        let built_tree = self.lower_match_tree(block, expr_span, &scrutinee, pat.span, vec![(pat, HasMatchGuard::No)], true)?;
        let otherwise_block = built_tree.otherwise_block;
        let [branch]: [MatchTreeBranch; 1] = built_tree.branches.try_into().map_err(|_| self.error(pat.span, UnsupportedReason::UnresolvedFact("one branch for one pattern")))?;

        let si = self.source_info(expr_span);
        self.break_for_else(otherwise_block, si)?;

        match declare_let_bindings {
            DeclareLetBindings::Yes => {
                let expr_place = scrutinee.try_to_place(self);
                let opt_expr_place = expr_place.as_ref().map(|place| (Some(place), expr_span));
                let scope_span = Span { file: pat.span.file, lo: pat.span.lo.min(scope_span.lo), hi: pat.span.hi.max(scope_span.hi) };
                self.declare_bindings(source_scope, scope_span, pat, None, opt_expr_place)?;
            }
            DeclareLetBindings::No => {} // Caller is responsible for bindings.
            DeclareLetBindings::LetNotPermitted => {
                return Err(self.error(expr_span, UnsupportedReason::UnresolvedFact("let expression not expected in this context")));
            }
        }

        let outer = self.source_info(pat.span);
        let success = self.bind_pattern(outer, branch, &[], expr_span, None)?;

        Ok(success.unit())
    }

    /// Initializes each of the bindings from the candidate by moving/copying/ref'ing the source as
    /// appropriate. Tests the guard, if any, and then branches to the arm. Returns the block for the
    /// case where the guard succeeds.
    ///
    /// Note: we do not check earlier that if there is a guard, there cannot be move bindings. We
    /// avoid a use-after-move by only moving the binding once the guard has evaluated to true (see
    /// below).
    fn bind_and_guard_matched_candidate(
        &mut self,
        sub_branch: MatchTreeSubBranch,
        fake_borrows: &[(Place, Local, FakeBorrowKind)],
        scrutinee_span: Span,
        arm_match_scope: Option<(&Arm, ScopeId)>,
        schedule_drops: ScheduleDrops,
    ) -> BuildResult<BasicBlock> {
        let block = sub_branch.success_block;

        if sub_branch.is_never {
            // This arm has a dummy body, we don't need to generate code for it. `block` is already
            // unreachable (except via false edge).
            let source_info = self.source_info(sub_branch.span);
            self.cfg.terminate(block, source_info, TerminatorKind::Unreachable);
            return Ok(self.cfg.start_new_block());
        }

        self.ascribe_types(block, sub_branch.ascriptions);

        // Lower an instance of the arm guard (if present) for this candidate, and then perform
        // bindings for the arm body.
        let guarded_arm = match arm_match_scope {
            Some((arm, match_scope)) => arm.guard.map(|guard| (arm, match_scope, guard)),
            None => None,
        };
        if let Some((arm, match_scope, guard)) = guarded_arm {
            // Bindings for guards require some extra handling to automatically insert implicit
            // references/dereferences. This always schedules storage drops, so we may need to
            // unschedule them below.
            self.bind_matched_candidate_for_guard(block, sub_branch.bindings.iter())?;
            let guard_frame = GuardFrame { locals: sub_branch.bindings.iter().map(|b| GuardFrameLocal { id: b.var_id }).collect() };
            self.guard_context.push(guard_frame);

            let scrutinee_source_info = self.source_info(scrutinee_span);
            for (place, temp, kind) in fake_borrows {
                let borrow = Rvalue::Ref(crate::mir::Region::ERASED, BorrowKind::Fake(*kind), place.clone());
                self.cfg.push_assign(block, scrutinee_source_info, Place::local(*temp), borrow);
            }

            let thir = self.thir;
            let mut guard_span = Span::default();

            let (post_guard_block, otherwise_post_guard_block) = self.in_if_then_scope(match_scope, guard_span, |this| {
                guard_span = thir.thir[guard].span;
                let si = this.source_info(arm.span);
                this.then_else_break(
                    block,
                    guard,
                    None, // Use `self.local_scope()` as the temp scope
                    si,
                    DeclareLetBindings::No, // For guards, `let` bindings are declared separately
                )
            })?;

            // If this isn't the final sub-branch being lowered, we need to unschedule drops of
            // bindings and temporaries created for and by the guard. As a result, the drop order
            // for the arm will correspond to the binding order of the final sub-branch lowered.
            if matches!(schedule_drops, ScheduleDrops::No) {
                self.clear_match_arm_and_guard_scopes(arm.scope)?;
            }

            let source_info = self.source_info(guard_span);
            // `source_map().end_point(guard_span)`
            let guard_end = self.source_info(Span { file: guard_span.file, lo: guard_span.hi.saturating_sub(1), hi: guard_span.hi });
            let _guard_frame = self.guard_context.pop();

            for (_, temp, _) in fake_borrows {
                let cause = FakeReadCause::ForMatchGuard;
                self.cfg.push_fake_read(post_guard_block, guard_end, cause, Place::local(*temp));
            }

            self.cfg.goto(otherwise_post_guard_block, source_info, sub_branch.otherwise_block);

            // We want to ensure that the matched candidates are bound after we have confirmed this
            // candidate *and* any associated guard; Binding them on `block` is too soon, because
            // that would be before we've checked the result from the guard.
            //
            // But binding them on the arm is *too late*, because then all of the candidates for a
            // single arm would be bound in the same place, that would cause a case like:
            //
            // ```rust
            // match (30, 2) {
            //     (mut x, 1) | (2, mut x) if { true } => { ... }
            //     ...                                 // ^^^^^^^ (this is `arm_block`)
            // }
            // ```
            //
            // would yield an `arm_block` something like:
            //
            // ```
            // StorageLive(_4);        // _4 is `x`
            // _4 = &mut (_1.0: i32);  // this is handling `(mut x, 1)` case
            // _4 = &mut (_1.1: i32);  // this is handling `(2, mut x)` case
            // ```
            //
            // and that is clearly not correct.
            let by_value_bindings: Vec<&Binding> = sub_branch.bindings.iter().filter(|binding| matches!(binding.binding_mode.0, ByRef::No)).collect();
            // Read all of the by reference bindings to ensure that the place they refer to can't be
            // modified by the guard.
            for binding in &by_value_bindings {
                let local_id = self.var_local_id(binding.var_id, RefWithinGuard);
                let cause = FakeReadCause::ForGuardBinding;
                self.cfg.push_fake_read(post_guard_block, guard_end, cause, Place::local(local_id));
            }
            // Only schedule drops for the last sub-branch we lower.
            self.bind_matched_candidate_for_arm_body(post_guard_block, schedule_drops, by_value_bindings.into_iter())?;

            Ok(post_guard_block)
        } else {
            // (Here, it is not too early to bind the matched candidate on `block`, because there is
            // no guard result that we have to inspect before we bind them.)
            self.bind_matched_candidate_for_arm_body(block, schedule_drops, sub_branch.bindings.iter())?;
            Ok(block)
        }
    }

    /// Append `AscribeUserType` statements onto the end of `block` for each ascription
    fn ascribe_types(&mut self, block: BasicBlock, ascriptions: impl IntoIterator<Item = Ascription>) {
        for ascription in ascriptions {
            let source_info = self.source_info(ascription.annotation.span);

            let base = self.push_user_type_annotation(ascription.annotation);
            self.cfg.push(
                block,
                Statement {
                    source_info,
                    kind: StatementKind::AscribeUserType(ascription.source, UserTypeProjection { base, projection: Vec::new() }, ascription.variance),
                },
            );
        }
    }

    /// Binding for guards is a bit different from binding for the arm body, because an extra layer
    /// of implicit reference/dereference is added.
    ///
    /// The idea is that any pattern bindings of type T will map to a `&T` within the context of the
    /// guard expression, but will continue to map to a `T` in the context of the arm body. To avoid
    /// surfacing this distinction in the user source code (which would be a severe change to the
    /// language and require far more revision to the compiler), any occurrence of the identifier in
    /// the guard expression will automatically get a deref op applied to it. (See the caller of
    /// [`Self::is_bound_var_in_guard`].)
    ///
    /// So an input like:
    ///
    /// ```ignore (illustrative)
    /// let place = Foo::new();
    /// match place { foo if inspect(foo)
    ///     => feed(foo), ... }
    /// ```
    ///
    /// will be treated as if it were really something like:
    ///
    /// ```ignore (illustrative)
    /// let place = Foo::new();
    /// match place { Foo { .. } if { let tmp1 = &place; inspect(*tmp1) }
    ///     => { let tmp2 = place; feed(tmp2) }, ... }
    /// ```
    ///
    /// And an input like:
    ///
    /// ```ignore (illustrative)
    /// let place = Foo::new();
    /// match place { ref mut foo if inspect(foo)
    ///     => feed(foo), ... }
    /// ```
    ///
    /// will be treated as if it were really something like:
    ///
    /// ```ignore (illustrative)
    /// let place = Foo::new();
    /// match place { Foo { .. } if { let tmp1 = & &mut place; inspect(*tmp1) }
    ///     => { let tmp2 = &mut place; feed(tmp2) }, ... }
    /// ```
    /// ---
    ///
    /// ## Implementation notes
    ///
    /// To encode the distinction above, we must inject the temporaries `tmp1` and `tmp2`.
    ///
    /// There are two cases of interest: binding by-value, and binding by-ref.
    ///
    /// 1. Binding by-value: Things are simple.
    ///
    ///    * Establishing `tmp1` creates a reference into the matched place. This code is emitted by
    ///      [`Self::bind_matched_candidate_for_guard`].
    ///
    ///    * `tmp2` is only initialized "lazily", after we have checked the guard. Thus, the code
    ///      that can trigger moves out of the candidate can only fire after the guard evaluated to
    ///      true. This initialization code is emitted by
    ///      [`Self::bind_matched_candidate_for_arm_body`].
    ///
    /// 2. Binding by-reference: Things are tricky.
    ///
    ///    * Here, the guard expression wants a `&&` or `&&mut` into the original input. This means
    ///      we need to borrow the reference that we create for the arm.
    ///    * So we eagerly create the reference for the arm and then take a reference to that.
    ///
    /// ---
    ///
    /// See these PRs for some historical context:
    /// - <https://github.com/rust-lang/rust/pull/49870> (introduction of autoref)
    /// - <https://github.com/rust-lang/rust/pull/59114> (always use autoref)
    fn bind_matched_candidate_for_guard<'b>(&mut self, block: BasicBlock, bindings: impl IntoIterator<Item = &'b Binding>) -> BuildResult<()> {
        // Assign each of the bindings. Since we are binding for a guard expression, this will never
        // trigger moves out of the candidate.
        for binding in bindings {
            let source_info = self.source_info(binding.span);

            // For each pattern ident P of type T, `ref_for_guard` is a reference R: &T pointing to
            // the location matched by the pattern, and every occurrence of P within a guard denotes
            // *R. Drops must be scheduled to emit `StorageDead` on the guard's failure/break
            // branches.
            let ref_for_guard =
                self.storage_live_binding(block, binding.var_id, binding.span, binding.is_shorthand, RefWithinGuard, ScheduleDrops::Yes)?;
            match binding.binding_mode.0 {
                ByRef::No => {
                    // The arm binding will be by value, so for the guard binding just take a shared
                    // reference to the matched place.
                    let rvalue = Rvalue::Ref(crate::mir::Region::ERASED, BorrowKind::Shared, binding.source.clone());
                    self.cfg.push_assign(block, source_info, ref_for_guard, rvalue);
                }
                ByRef::Yes(mutbl) => {
                    // The arm binding will be by reference, so eagerly create it now. Drops must be
                    // scheduled to emit `StorageDead` on the guard's failure/break branches.
                    let value_for_arm =
                        self.storage_live_binding(block, binding.var_id, binding.span, binding.is_shorthand, OutsideGuard, ScheduleDrops::Yes)?;

                    let rvalue = Rvalue::Ref(crate::mir::Region::ERASED, util::ref_pat_borrow_kind(mutbl), binding.source.clone());
                    self.cfg.push_assign(block, source_info, value_for_arm.clone(), rvalue);
                    // For the guard binding, take a shared reference to that reference.
                    let rvalue = Rvalue::Ref(crate::mir::Region::ERASED, BorrowKind::Shared, value_for_arm);
                    self.cfg.push_assign(block, source_info, ref_for_guard, rvalue);
                }
            }
        }
        Ok(())
    }

    fn bind_matched_candidate_for_arm_body<'b>(
        &mut self,
        block: BasicBlock,
        schedule_drops: ScheduleDrops,
        bindings: impl IntoIterator<Item = &'b Binding>,
    ) -> BuildResult<()> {
        // Assign each of the bindings. This may trigger moves out of the candidate.
        for binding in bindings {
            let source_info = self.source_info(binding.span);
            let local = self.storage_live_binding(block, binding.var_id, binding.span, binding.is_shorthand, OutsideGuard, schedule_drops)?;
            if matches!(schedule_drops, ScheduleDrops::Yes) {
                self.schedule_drop_for_binding(binding.var_id, binding.span, OutsideGuard)?;
            }
            let rvalue = match binding.binding_mode.0 {
                ByRef::No => Rvalue::Use(self.consume_by_copy_or_move(binding.source.clone())?),
                ByRef::Yes(mutbl) => Rvalue::Ref(crate::mir::Region::ERASED, util::ref_pat_borrow_kind(mutbl), binding.source.clone()),
            };
            self.cfg.push_assign(block, source_info, local, rvalue);
        }
        Ok(())
    }

    /// Each binding (`ref mut var`/`ref var`/`mut var`/`var`, where the bound `var` has type `T` in
    /// the arm body) in a pattern maps to 2 locals. The first local is a binding for occurrences of
    /// `var` in the guard, which will have type `&T`. The second local is a binding for occurrences
    /// of `var` in the arm body, which will have type `T`.
    fn declare_binding(
        &mut self,
        source_info: SourceInfo,
        visibility_scope: SourceScope,
        name: Sym,
        mode: BindingMode,
        var_id: LocalVarId,
        var_ty: TyId,
        user_ty: Vec<UserTypeProjection>,
        has_guard: ArmHasGuard,
        opt_match_place: Option<(Option<Place>, Span)>,
        pat_span: Span,
    ) {
        let debug_source_info = SourceInfo { span: source_info.span, scope: visibility_scope };
        let local = crate::mir::LocalDecl {
            mutability: mode.1,
            ty: var_ty,
            user_ty,
            source_info,
            kind: LocalKind::UserVar,
            local_info: LocalInfo::User(BindingForm::Var(VarBindingForm {
                binding_mode: mode,
                // hypothetically, `visit_primary_bindings` could try to unzip an outermost hir::Ty
                // as we descend, matching up idents in pat; but complex w/ unclear UI payoff.
                // Instead, just abandon providing diagnostic info.
                opt_ty_info: None,
                opt_match_place,
                pat_span,
                introductions: Vec::new(),
            })),
        };
        self.local_decls.push(local);
        let for_arm_body = Local(self.local_decls.len() as u32 - 1);
        let name_text = self.root.cx.prog.syms.str(name).to_string();
        self.var_debug_info.push(VarDebugInfo { name: name_text.clone(), source_info: debug_source_info, place: Place::local(for_arm_body) });
        let locals = if has_guard.0 {
            let ref_ty = self.ref_ty(false, var_ty);
            self.local_decls.push(crate::mir::LocalDecl {
                // This variable isn't mutated but has a name, so has to be immutable to avoid the
                // unused mut lint.
                mutability: Mutability::Not,
                ty: ref_ty,
                user_ty: Vec::new(),
                source_info,
                kind: LocalKind::UserVar,
                local_info: LocalInfo::User(BindingForm::RefForGuard(for_arm_body)),
            });
            let ref_for_guard = Local(self.local_decls.len() as u32 - 1);
            self.var_debug_info.push(VarDebugInfo { name: name_text, source_info: debug_source_info, place: Place::local(ref_for_guard) });
            LocalsForNode::ForGuard { ref_for_guard, for_arm_body }
        } else {
            LocalsForNode::One(for_arm_body)
        };
        self.var_indices.insert(var_id, locals);
    }
}

#[allow(dead_code)]
fn _unused(_: UnOp) {}
