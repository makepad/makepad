//! Structural validation following rustc_mir_transform/src/validate.rs:124,246,285,1453.
//! TODO (rustc validate.rs:567,1456,1582): normalized assignment/call relations, Copy and
//! Sized obligations, ADT projections, and needs_drop require later semantic consumers.
use super::visit::{NonUseContext, PlaceContext, Visitor};
use super::*;
use crate::tcx::Tcx;
use crate::types::TyKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    /// Empty for the main body, outermost promotion first otherwise.
    pub promoted: Vec<PromotedId>,
    pub bb: Option<BasicBlock>,
    /// statements.len() identifies the terminator; None identifies body metadata.
    pub statement_index: Option<usize>,
    pub message: String,
}

pub fn validate(body: &Body, tcx: &Tcx) -> Vec<ValidationError> {
    let mut checker = Checker { body, tcx, errors: Vec::new(), at: None, next_promoted: 0 };
    checker.check_body();
    checker.errors
}
struct Checker<'a> {
    body: &'a Body,
    tcx: &'a Tcx,
    errors: Vec<ValidationError>,
    at: Option<Location>,
    next_promoted: u32,
}
impl Checker<'_> {
    fn fail(&mut self, message: &str) {
        self.errors.push(ValidationError {
            promoted: Vec::new(),
            bb: self.at.map(|at| at.block),
            statement_index: self.at.map(|at| at.statement_index),
            message: message.into(),
        });
    }
    fn runtime(&self) -> bool {
        matches!(self.body.phase, MirPhase::Runtime(_))
    }
    fn check_body(&mut self) {
        if self.body.basic_blocks.is_empty() {
            self.fail("body has no entry block");
        }
        if self.body.basic_blocks.first().is_some_and(|bb| bb.is_cleanup) {
            self.fail("entry block is cleanup");
        }
        if self.body.local_decls.is_empty() {
            self.fail("body has no return local");
        }
        if self.body.arg_count as usize >= self.body.local_decls.len() {
            self.fail("argument count exceeds locals");
        }
        for (i, decl) in self.body.local_decls.iter().enumerate() {
            let valid = if i == 0 {
                decl.kind == LocalKind::Return
            } else if i <= self.body.arg_count as usize {
                decl.kind == LocalKind::Arg
            } else {
                matches!(decl.kind, LocalKind::UserVar | LocalKind::Temp)
            };
            if !valid {
                self.fail("local kind disagrees with argument/return position");
            }
        }
        if let Some(arg) = self.body.spread_arg {
            if arg.0 == 0 || arg.0 > self.body.arg_count {
                self.fail("spread argument is not an argument");
            }
        }
        if self.body.source_scopes.is_empty() {
            self.fail("body has no source scope");
        }
        for scope in &self.body.source_scopes {
            let mut next = scope.parent_scope;
            let mut count = 0;
            while let Some(parent) = next {
                let Some(parent) = self.body.source_scopes.get(parent.0 as usize) else {
                    self.fail("source scope parent is out of range");
                    break;
                };
                count += 1;
                if count > self.body.source_scopes.len() {
                    self.fail("source scope cycle");
                    break;
                }
                next = parent.parent_scope;
            }
        }
        // Promoted bodies have independent local/block/scope indices.
        self.super_body(self.body);
    }
    fn place_ty(&self, place: &Place) -> Option<TyId> {
        Some(place.ty(self.body, self.tcx)?.ty)
    }
    fn operand_ty(&self, op: &Operand) -> Option<TyId> {
        match op {
            Operand::Copy(place) | Operand::Move(place) => self.place_ty(place),
            Operand::Constant(c) => match c.const_ {
                Const::Val(_, ty) => Some(ty),
                Const::Unevaluated { .. } => None,
            },
        }
    }
    fn rvalue_ty(&self, rv: &Rvalue) -> Option<TyId> {
        match rv {
            Rvalue::Use(op) | Rvalue::UnaryOp(UnOp::Not | UnOp::Neg, op) => self.operand_ty(op),
            Rvalue::CopyForDeref(place) => self.place_ty(place),
            Rvalue::Cast(_, _, ty) => Some(*ty),
            Rvalue::Len(_) | Rvalue::NullaryOp(NullOp::SizeOf | NullOp::AlignOf | NullOp::OffsetOf(_), _) => Some(self.tcx.tys.usize_),
            Rvalue::NullaryOp(NullOp::UbChecks | NullOp::ContractChecks, _) => Some(self.tcx.tys.bool_),
            Rvalue::BinaryOp(op, args) => match op {
                BinOp::Eq | BinOp::Lt | BinOp::Le | BinOp::Ne | BinOp::Ge | BinOp::Gt => Some(self.tcx.tys.bool_),
                BinOp::AddWithOverflow | BinOp::SubWithOverflow | BinOp::MulWithOverflow | BinOp::Cmp => None,
                _ => self.operand_ty(&args.0),
            },
            _ => None,
        }
    }
    fn primitive(&self, ty: TyId) -> bool {
        (ty.0 as usize) < self.tcx.tys.len()
            && matches!(self.tcx.tys.kind(ty), TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_))
    }
}
impl Visitor for Checker<'_> {
    fn visit_body(&mut self, body: &Body) {
        // super_body visits promotions in index order; each has its own index spaces.
        let id = self.next_promoted;
        self.next_promoted += 1;
        for mut error in validate(body, self.tcx) {
            error.promoted.insert(0, PromotedId(id));
            self.errors.push(error);
        }
    }
    fn visit_basic_block_data(&mut self, bb: BasicBlock, data: &BasicBlockData) {
        for (i, stmt) in data.statements.iter().enumerate() {
            self.visit_statement(stmt, Location { block: bb, statement_index: i });
        }
        let at = Location { block: bb, statement_index: data.statements.len() };
        self.at = Some(at);
        match &data.terminator {
            Some(term) => self.visit_terminator(term, at),
            None => self.fail("block has no terminator"),
        }
        self.at = None;
    }
    fn visit_local(&mut self, local: &Local, context: PlaceContext, _at: Location) {
        if local.0 as usize >= self.body.local_decls.len() {
            self.fail("local is out of range");
        }
        if matches!(context, PlaceContext::NonUse(NonUseContext::StorageLive | NonUseContext::StorageDead))
            && local.0 <= self.body.arg_count
        {
            self.fail("storage marker on return or argument local");
        }
    }
    fn visit_ty(&mut self, ty: &TyId) {
        if ty.0 as usize >= self.tcx.tys.len() {
            self.fail("type is out of range");
        }
    }
    fn visit_source_scope(&mut self, scope: &SourceScope) {
        if scope.0 as usize >= self.body.source_scopes.len() {
            self.fail("source scope is out of range");
        }
    }
    fn visit_user_type_index(&mut self, user: &UserTypeAnnotationIndex) {
        if user.0 as usize >= self.body.user_type_annotations.len() {
            self.fail("user type annotation is out of range");
        }
    }
    fn visit_target(&mut self, target: &BasicBlock, unwind: bool, at: Location) {
        if *target == BasicBlock(0) {
            self.fail("entry block must not have predecessors");
        }
        let Some(dest) = self.body.basic_blocks.get(target.0 as usize) else {
            self.fail("target block is out of range");
            return;
        };
        let src = &self.body.basic_blocks[at.block.0 as usize];
        // rustc validate.rs:146: normal edges preserve cleanup status; unwind enters cleanup.
        if if unwind { src.is_cleanup || !dest.is_cleanup } else { src.is_cleanup != dest.is_cleanup } {
            self.fail("edge violates cleanup invariants");
        }
    }
    fn visit_unwind(&mut self, unwind: &UnwindAction, at: Location) {
        let cleanup = self.body.basic_blocks[at.block.0 as usize].is_cleanup;
        match unwind {
            UnwindAction::Cleanup(target) => self.visit_target(target, true, at),
            UnwindAction::Continue if cleanup => self.fail("unwind continue in cleanup block"),
            UnwindAction::Terminate(UnwindTerminateReason::InCleanup) if !cleanup => self.fail("terminate InCleanup outside cleanup block"),
            _ => {}
        }
    }
    fn visit_statement(&mut self, stmt: &Statement, at: Location) {
        self.at = Some(at);
        match &stmt.kind {
            // PlaceMention remains legal at runtime (rustc_mir_transform/src/validate.rs:333).
            StatementKind::FakeRead(..) | StatementKind::AscribeUserType(..) if self.runtime() => {
                self.fail("analysis statement in runtime MIR")
            }
            StatementKind::Assign(place, rv) => {
                if let (Some(left), Some(right)) = (self.place_ty(place), self.rvalue_ty(rv)) {
                    if self.primitive(left) && self.primitive(right) && left != right {
                        self.fail("assignment has incompatible primitive types");
                    }
                }
            }
            _ => {}
        }
        self.super_statement(stmt, at);
    }
    fn visit_rvalue(&mut self, rv: &Rvalue, at: Location) {
        if self.runtime() && matches!(rv, Rvalue::Ref(_, BorrowKind::Fake(_), _)) {
            self.fail("fake borrow in runtime MIR");
        }
        self.super_rvalue(rv, at);
    }
    fn visit_terminator(&mut self, term: &Terminator, at: Location) {
        self.at = Some(at);
        let cleanup = self.body.basic_blocks[at.block.0 as usize].is_cleanup;
        match &term.kind {
            TerminatorKind::SwitchInt { targets, .. } => {
                if targets.values.len() != targets.targets.len() {
                    self.fail("switch value/target count mismatch");
                }
                let mut values = std::collections::HashSet::new();
                for value in &targets.values {
                    if !values.insert(*value) {
                        self.fail("duplicate switch value");
                    }
                }
            }
            TerminatorKind::FalseEdge { .. } | TerminatorKind::FalseUnwind { .. } if self.runtime() => {
                self.fail("false edge in runtime MIR")
            }
            TerminatorKind::Yield { .. } | TerminatorKind::CoroutineDrop => {
                if self.runtime() {
                    self.fail("coroutine terminator after coroutine lowering");
                }
                if self.body.coroutine.is_none() {
                    self.fail("coroutine terminator outside coroutine body");
                }
            }
            TerminatorKind::UnwindResume | TerminatorKind::UnwindTerminate(_) if !cleanup => {
                self.fail("unwind terminator outside cleanup block")
            }
            TerminatorKind::Return if cleanup => self.fail("return from cleanup block"),
            TerminatorKind::InlineAsm(_) => self.fail("inline assembly is unsupported"),
            TerminatorKind::Call { args, arg_spans, .. } | TerminatorKind::TailCall { args, arg_spans, .. } => {
                if !arg_spans.is_empty() && args.len() != arg_spans.len() {
                    self.fail("call argument span count mismatch");
                }
            }
            // Drop remains legal in every phase: rustc_middle/src/mir/syntax.rs:743.
            // Post-elaboration it executes unconditional glue (or is a nop for no-glue types).
            _ => {}
        }
        self.super_terminator(term, at);
    }
}
