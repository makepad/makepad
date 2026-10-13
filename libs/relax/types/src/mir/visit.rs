//! Plain visitors following rustc_middle/src/mir/visit.rs:80,333,1120,1323.
//! Both visitors walk nested operands, projection indices, metadata and promoted bodies.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonMutatingUseContext {
    Inspect,
    Copy,
    Move,
    SharedBorrow,
    FakeBorrow,
    RawBorrow,
    PlaceMention,
    Projection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutatingUseContext {
    Store,
    SetDiscriminant,
    AsmOutput,
    Call,
    Yield,
    Drop,
    Borrow,
    RawBorrow,
    Projection,
    Retag,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonUseContext {
    StorageLive,
    StorageDead,
    AscribeUserTy(Variance),
    VarDebugInfo,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceContext {
    NonMutatingUse(NonMutatingUseContext),
    MutatingUse(MutatingUseContext),
    NonUse(NonUseContext),
}
use MutatingUseContext as W;
use NonMutatingUseContext as R;
use NonUseContext as N;
use PlaceContext::{MutatingUse as Write, NonMutatingUse as Read, NonUse as Non};
impl PlaceContext {
    pub fn is_use(self) -> bool {
        !matches!(self, Non(_))
    }
    pub fn is_mutating_use(self) -> bool {
        matches!(self, Write(_))
    }
    pub fn is_drop(self) -> bool {
        self == Write(W::Drop)
    }
    pub fn is_borrow(self) -> bool {
        matches!(self, Read(R::SharedBorrow) | Write(W::Borrow))
    }
}

pub trait Visitor {
    fn visit_body(&mut self, body: &Body) {
        self.super_body(body);
    }
    fn super_body(&mut self, body: &Body) {
        self.visit_def(&body.source.owner);
        match &body.source.kind {
            BodyKind::Promoted(parent, _) => self.visit_def(parent),
            BodyKind::Shim(ShimKind::DropGlue(ty) | ShimKind::Clone(ty)) => self.visit_ty(ty),
            _ => {}
        }
        for decl in &body.local_decls {
            self.visit_local_decl(decl);
        }
        for scope in &body.source_scopes {
            self.visit_span(&scope.span);
            if let Some(parent) = &scope.parent_scope {
                self.visit_source_scope(parent);
            }
        }
        for annotation in &body.user_type_annotations {
            self.visit_user_type(annotation);
        }
        for debug in &body.var_debug_info {
            self.visit_source_info(&debug.source_info);
            self.visit_place(&debug.place, Non(N::VarDebugInfo), Location::default());
        }
        if let Some(local) = &body.spread_arg {
            self.visit_local(local, Non(N::VarDebugInfo), Location::default());
        }
        if let Some(coro) = &body.coroutine {
            self.visit_ty(&coro.yield_ty);
            self.visit_ty(&coro.resume_ty);
        }
        for (i, block) in body.basic_blocks.iter().enumerate() {
            self.visit_basic_block_data(BasicBlock(i as u32), block);
        }
        for promoted in &body.promoted {
            self.visit_body(promoted);
        }
    }
    fn visit_local_decl(&mut self, decl: &LocalDecl) {
        self.visit_ty(&decl.ty);
        self.visit_source_info(&decl.source_info);
        for user in &decl.user_ty {
            self.visit_user_type_projection(user, Location::default());
        }
        match &decl.local_info {
            LocalInfo::User { match_place: Some((place, span)), .. } => {
                if let Some(place) = place {
                    self.visit_place(place, Non(N::VarDebugInfo), Location::default());
                }
                self.visit_span(span);
            }
            LocalInfo::StaticRef { def_id, .. } => self.visit_def(def_id),
            LocalInfo::GuardBinding(local) => self.visit_local(local, Non(N::VarDebugInfo), Location::default()),
            _ => {}
        }
    }
    fn visit_basic_block_data(&mut self, bb: BasicBlock, data: &BasicBlockData) {
        for (i, statement) in data.statements.iter().enumerate() {
            self.visit_statement(statement, Location { block: bb, statement_index: i });
        }
        if let Some(term) = &data.terminator {
            self.visit_terminator(term, Location { block: bb, statement_index: data.statements.len() });
        }
    }
    fn visit_statement(&mut self, statement: &Statement, at: Location) {
        self.super_statement(statement, at);
    }
    fn super_statement(&mut self, statement: &Statement, at: Location) {
        self.visit_source_info(&statement.source_info);
        match &statement.kind {
            StatementKind::Assign(place, rv) => {
                self.visit_place(place, Write(W::Store), at);
                self.visit_rvalue(rv, at);
            }
            StatementKind::FakeRead(_, place) => self.visit_place(place, Read(R::Inspect), at),
            StatementKind::SetDiscriminant { place, .. } => self.visit_place(place, Write(W::SetDiscriminant), at),
            StatementKind::StorageLive(local) => self.visit_local(local, Non(N::StorageLive), at),
            StatementKind::StorageDead(local) => self.visit_local(local, Non(N::StorageDead), at),
            StatementKind::PlaceMention(place) => self.visit_place(place, Read(R::PlaceMention), at),
            StatementKind::AscribeUserType(place, user, variance) => {
                self.visit_place(place, Non(N::AscribeUserTy(*variance)), at);
                self.visit_user_type_projection(user, at);
            }
            StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(op)) => self.visit_operand(op, at),
            StatementKind::Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping { src, dst, count }) => {
                self.visit_operand(src, at);
                self.visit_operand(dst, at);
                self.visit_operand(count, at);
            }
            StatementKind::ConstEvalCounter | StatementKind::Nop => {}
        }
    }
    fn visit_terminator(&mut self, term: &Terminator, at: Location) {
        self.super_terminator(term, at);
    }
    fn super_terminator(&mut self, term: &Terminator, at: Location) {
        self.visit_source_info(&term.source_info);
        match &term.kind {
            TerminatorKind::Goto { target } => self.visit_target(target, false, at),
            TerminatorKind::SwitchInt { discr, targets } => {
                self.visit_operand(discr, at);
                for target in &targets.targets {
                    self.visit_target(target, false, at);
                }
                self.visit_target(&targets.otherwise, false, at);
            }
            TerminatorKind::Drop { place, target, unwind, .. } => {
                self.visit_place(place, Write(W::Drop), at);
                self.visit_target(target, false, at);
                self.visit_unwind(unwind, at);
            }
            TerminatorKind::Call { func, args, destination, target, unwind, fn_span, arg_spans, .. } => {
                self.visit_operand(func, at);
                for arg in args {
                    self.visit_operand(arg, at);
                }
                self.visit_place(destination, Write(W::Call), at);
                if let Some(target) = target {
                    self.visit_target(target, false, at);
                }
                self.visit_unwind(unwind, at);
                self.visit_span(fn_span);
                for span in arg_spans {
                    self.visit_span(span);
                }
            }
            TerminatorKind::TailCall { func, args, fn_span, arg_spans } => {
                self.visit_operand(func, at);
                for arg in args {
                    self.visit_operand(arg, at);
                }
                self.visit_span(fn_span);
                for span in arg_spans {
                    self.visit_span(span);
                }
            }
            TerminatorKind::Assert { cond, msg, target, unwind, .. } => {
                self.visit_operand(cond, at);
                self.visit_assert(msg, at);
                self.visit_target(target, false, at);
                self.visit_unwind(unwind, at);
            }
            TerminatorKind::Yield { value, resume, resume_arg, drop } => {
                self.visit_operand(value, at);
                self.visit_place(resume_arg, Write(W::Yield), at);
                self.visit_target(resume, false, at);
                if let Some(target) = drop {
                    self.visit_target(target, false, at);
                }
            }
            TerminatorKind::FalseEdge { real_target, imaginary_target } => {
                self.visit_target(real_target, false, at);
                self.visit_target(imaginary_target, false, at);
            }
            TerminatorKind::FalseUnwind { real_target, unwind } => {
                self.visit_target(real_target, false, at);
                self.visit_unwind(unwind, at);
            }
            TerminatorKind::Return => self.visit_local(&Local(0), Read(R::Move), at),
            TerminatorKind::UnwindResume
            | TerminatorKind::UnwindTerminate(_)
            | TerminatorKind::Unreachable
            | TerminatorKind::CoroutineDrop
            | TerminatorKind::InlineAsm(_) => {}
        }
    }
    fn visit_rvalue(&mut self, rv: &Rvalue, at: Location) {
        self.super_rvalue(rv, at);
    }
    fn super_rvalue(&mut self, rv: &Rvalue, at: Location) {
        match rv {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.visit_operand(op, at),
            Rvalue::Repeat(op, len) => {
                self.visit_operand(op, at);
                self.visit_const(len, at);
            }
            Rvalue::Ref(region, kind, place) => {
                self.visit_region(region);
                let context = match kind {
                    BorrowKind::Shared => Read(R::SharedBorrow),
                    BorrowKind::Fake(_) => Read(R::FakeBorrow),
                    BorrowKind::Mut { .. } => Write(W::Borrow),
                };
                self.visit_place(place, context, at);
            }
            Rvalue::RawPtr(kind, place) => {
                let context = match kind {
                    RawPtrKind::Const => Read(R::RawBorrow),
                    RawPtrKind::Mut => Write(W::RawBorrow),
                    RawPtrKind::FakeForPtrMetadata => Read(R::Inspect),
                };
                self.visit_place(place, context, at)
            }
            Rvalue::Len(place) | Rvalue::Discriminant(place) => self.visit_place(place, Read(R::Inspect), at),
            Rvalue::CopyForDeref(place) => self.visit_place(place, Read(R::Inspect), at),
            Rvalue::Cast(_, op, ty) | Rvalue::ShallowInitBox(op, ty) => {
                self.visit_operand(op, at);
                self.visit_ty(ty);
            }
            Rvalue::BinaryOp(_, ops) => {
                self.visit_operand(&ops.0, at);
                self.visit_operand(&ops.1, at);
            }
            Rvalue::NullaryOp(_, ty) => self.visit_ty(ty),
            Rvalue::ThreadLocalRef(def) => self.visit_def(def),
            Rvalue::Aggregate(kind, ops) => {
                self.visit_aggregate_kind(kind);
                for op in ops {
                    self.visit_operand(op, at);
                }
            }
        }
    }
    fn visit_generic_arg(&mut self, arg: &crate::consts::GenericArg) {
        match arg {
            crate::consts::GenericArg::Type(ty) => self.visit_ty(ty),
            crate::consts::GenericArg::Lifetime(r) => self.visit_type_region(r),
            crate::consts::GenericArg::Const(ct) => self.visit_type_const(ct),
        }
    }
    fn visit_type_region(&mut self, _region: &crate::types::RegionId) {}
    fn visit_type_const(&mut self, _ct: &crate::consts::Const) {}
    fn visit_aggregate_kind(&mut self, kind: &AggregateKind) {
        match kind {
            AggregateKind::Array(ty) | AggregateKind::RawPtr(ty, _) => self.visit_ty(ty),
            AggregateKind::Adt(def, _, args, user, _) => {
                self.visit_def(def);
                for ty in args {
                    self.visit_generic_arg(ty);
                }
                if let Some(user) = user {
                    self.visit_user_type_index(user);
                }
            }
            AggregateKind::Closure(_, args) | AggregateKind::Coroutine(_, args) => {
                for ty in args {
                    self.visit_generic_arg(ty);
                }
            }
            AggregateKind::Tuple => {}
        }
    }
    fn visit_operand(&mut self, op: &Operand, at: Location) {
        self.super_operand(op, at);
    }
    fn super_operand(&mut self, op: &Operand, at: Location) {
        match op {
            Operand::Copy(place) => self.visit_place(place, Read(R::Copy), at),
            Operand::Move(place) => self.visit_place(place, Read(R::Move), at),
            Operand::Constant(c) => {
                self.visit_span(&c.span);
                if let Some(user) = &c.user_ty {
                    self.visit_user_type_index(user);
                }
                self.visit_const(&c.const_, at);
            }
        }
    }
    fn visit_const(&mut self, c: &Const, _at: Location) {
        match c {
            Const::Val(value, ty) => {
                self.visit_ty(ty);
                match value {
                    ConstValue::Scalar(Scalar::Ptr { provenance: CtfeProvenance { alloc, .. }, .. })
                    | ConstValue::Slice { alloc, .. }
                    | ConstValue::Indirect { alloc, .. } => self.visit_alloc_id(alloc),
                    ConstValue::Scalar(Scalar::Int { .. }) | ConstValue::ZeroSized => {}
                }
            }
            Const::Unevaluated { def, args, .. } => {
                self.visit_def(def);
                for arg in args {
                    self.visit_generic_arg(arg);
                }
            }
        }
    }
    fn visit_place(&mut self, place: &Place, context: PlaceContext, at: Location) {
        self.super_place(place, context, at);
    }
    fn super_place(&mut self, place: &Place, mut context: PlaceContext, at: Location) {
        if !place.projection.is_empty() && context.is_use() {
            context = if context.is_mutating_use() { Write(W::Projection) } else { Read(R::Projection) };
        }
        self.visit_local(&place.local, context, at);
        let mut i = place.projection.len();
        while i != 0 {
            i -= 1;
            self.visit_projection_elem(&place.projection[i], context, at);
        }
    }
    fn visit_projection_elem(&mut self, elem: &PlaceElem, context: PlaceContext, at: Location) {
        match elem {
            PlaceElem::Field(_, ty) | PlaceElem::OpaqueCast(ty) | PlaceElem::Subtype(ty) => self.visit_ty(ty),
            PlaceElem::Index(local) => self.visit_local(local, if context.is_use() { Read(R::Copy) } else { context }, at),
            PlaceElem::Deref | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } | PlaceElem::Downcast(..) => {}
        }
    }
    fn visit_user_type_projection(&mut self, user: &UserTypeProjection, at: Location) {
        self.visit_user_type_index(&user.base);
        for elem in &user.projection {
            self.visit_projection_elem(elem, Non(N::AscribeUserTy(Variance::Invariant)), at);
        }
    }
    fn visit_assert(&mut self, msg: &AssertKind, at: Location) {
        match msg {
            AssertKind::BoundsCheck { len: a, index: b }
            | AssertKind::Overflow(_, a, b)
            | AssertKind::MisalignedPointerDereference { required: a, found: b } => {
                self.visit_operand(a, at);
                self.visit_operand(b, at);
            }
            AssertKind::OverflowNeg(op) | AssertKind::DivisionByZero(op) | AssertKind::RemainderByZero(op) => self.visit_operand(op, at),
            AssertKind::ResumedAfterReturn(_) | AssertKind::ResumedAfterPanic(_) | AssertKind::NullPointerDereference => {}
        }
    }
    fn visit_unwind(&mut self, unwind: &UnwindAction, at: Location) {
        if let UnwindAction::Cleanup(target) = unwind {
            self.visit_target(target, true, at);
        }
    }
    fn visit_source_info(&mut self, info: &SourceInfo) {
        self.visit_span(&info.span);
        self.visit_source_scope(&info.scope);
    }
    fn visit_local(&mut self, _local: &Local, _context: PlaceContext, _at: Location) {}
    fn visit_target(&mut self, _target: &BasicBlock, _unwind: bool, _at: Location) {}
    fn visit_ty(&mut self, _ty: &TyId) {}
    fn visit_def(&mut self, _def: &DefId) {}
    fn visit_region(&mut self, _region: &Region) {}
    fn visit_span(&mut self, _span: &Span) {}
    fn visit_source_scope(&mut self, _scope: &SourceScope) {}
    fn visit_user_type_index(&mut self, _index: &UserTypeAnnotationIndex) {}
    fn visit_user_type(&mut self, _user: &UserTypeId) {}
    fn visit_alloc_id(&mut self, _alloc: &AllocId) {}
}

pub trait MutVisitor {
    fn visit_body(&mut self, body: &mut Body) {
        self.super_body(body);
    }
    fn super_body(&mut self, body: &mut Body) {
        self.visit_def(&mut body.source.owner);
        match &mut body.source.kind {
            BodyKind::Promoted(parent, _) => self.visit_def(parent),
            BodyKind::Shim(ShimKind::DropGlue(ty) | ShimKind::Clone(ty)) => self.visit_ty(ty),
            _ => {}
        }
        for decl in &mut body.local_decls {
            self.visit_local_decl(decl);
        }
        for scope in &mut body.source_scopes {
            self.visit_span(&mut scope.span);
            if let Some(parent) = &mut scope.parent_scope {
                self.visit_source_scope(parent);
            }
        }
        for annotation in &mut body.user_type_annotations {
            self.visit_user_type(annotation);
        }
        for debug in &mut body.var_debug_info {
            self.visit_source_info(&mut debug.source_info);
            self.visit_place(&mut debug.place, Non(N::VarDebugInfo), Location::default());
        }
        if let Some(local) = &mut body.spread_arg {
            self.visit_local(local, Non(N::VarDebugInfo), Location::default());
        }
        if let Some(coro) = &mut body.coroutine {
            self.visit_ty(&mut coro.yield_ty);
            self.visit_ty(&mut coro.resume_ty);
        }
        for (i, block) in body.basic_blocks.iter_mut().enumerate() {
            self.visit_basic_block_data(BasicBlock(i as u32), block);
        }
        for promoted in &mut body.promoted {
            self.visit_body(promoted);
        }
    }
    fn visit_local_decl(&mut self, decl: &mut LocalDecl) {
        self.visit_ty(&mut decl.ty);
        self.visit_source_info(&mut decl.source_info);
        for user in &mut decl.user_ty {
            self.visit_user_type_projection(user, Location::default());
        }
        match &mut decl.local_info {
            LocalInfo::User { match_place: Some((place, span)), .. } => {
                if let Some(place) = place {
                    self.visit_place(place, Non(N::VarDebugInfo), Location::default());
                }
                self.visit_span(span);
            }
            LocalInfo::StaticRef { def_id, .. } => self.visit_def(def_id),
            LocalInfo::GuardBinding(local) => self.visit_local(local, Non(N::VarDebugInfo), Location::default()),
            _ => {}
        }
    }
    fn visit_basic_block_data(&mut self, bb: BasicBlock, data: &mut BasicBlockData) {
        for (i, statement) in data.statements.iter_mut().enumerate() {
            self.visit_statement(statement, Location { block: bb, statement_index: i });
        }
        if let Some(term) = &mut data.terminator {
            self.visit_terminator(term, Location { block: bb, statement_index: data.statements.len() });
        }
    }
    fn visit_statement(&mut self, statement: &mut Statement, at: Location) {
        self.super_statement(statement, at);
    }
    fn super_statement(&mut self, statement: &mut Statement, at: Location) {
        self.visit_source_info(&mut statement.source_info);
        match &mut statement.kind {
            StatementKind::Assign(place, rv) => {
                self.visit_place(place, Write(W::Store), at);
                self.visit_rvalue(rv, at);
            }
            StatementKind::FakeRead(_, place) => self.visit_place(place, Read(R::Inspect), at),
            StatementKind::SetDiscriminant { place, .. } => self.visit_place(place, Write(W::SetDiscriminant), at),
            StatementKind::StorageLive(local) => self.visit_local(local, Non(N::StorageLive), at),
            StatementKind::StorageDead(local) => self.visit_local(local, Non(N::StorageDead), at),
            StatementKind::PlaceMention(place) => self.visit_place(place, Read(R::PlaceMention), at),
            StatementKind::AscribeUserType(place, user, variance) => {
                self.visit_place(place, Non(N::AscribeUserTy(*variance)), at);
                self.visit_user_type_projection(user, at);
            }
            StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(op)) => self.visit_operand(op, at),
            StatementKind::Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping { src, dst, count }) => {
                self.visit_operand(src, at);
                self.visit_operand(dst, at);
                self.visit_operand(count, at);
            }
            StatementKind::ConstEvalCounter | StatementKind::Nop => {}
        }
    }
    fn visit_terminator(&mut self, term: &mut Terminator, at: Location) {
        self.super_terminator(term, at);
    }
    fn super_terminator(&mut self, term: &mut Terminator, at: Location) {
        self.visit_source_info(&mut term.source_info);
        match &mut term.kind {
            TerminatorKind::Goto { target } => self.visit_target(target, false, at),
            TerminatorKind::SwitchInt { discr, targets } => {
                self.visit_operand(discr, at);
                for target in &mut targets.targets {
                    self.visit_target(target, false, at);
                }
                self.visit_target(&mut targets.otherwise, false, at);
            }
            TerminatorKind::Drop { place, target, unwind, .. } => {
                self.visit_place(place, Write(W::Drop), at);
                self.visit_target(target, false, at);
                self.visit_unwind(unwind, at);
            }
            TerminatorKind::Call { func, args, destination, target, unwind, fn_span, arg_spans, .. } => {
                self.visit_operand(func, at);
                for arg in args {
                    self.visit_operand(arg, at);
                }
                self.visit_place(destination, Write(W::Call), at);
                if let Some(target) = target {
                    self.visit_target(target, false, at);
                }
                self.visit_unwind(unwind, at);
                self.visit_span(fn_span);
                for span in arg_spans {
                    self.visit_span(span);
                }
            }
            TerminatorKind::TailCall { func, args, fn_span, arg_spans } => {
                self.visit_operand(func, at);
                for arg in args {
                    self.visit_operand(arg, at);
                }
                self.visit_span(fn_span);
                for span in arg_spans {
                    self.visit_span(span);
                }
            }
            TerminatorKind::Assert { cond, msg, target, unwind, .. } => {
                self.visit_operand(cond, at);
                self.visit_assert(msg, at);
                self.visit_target(target, false, at);
                self.visit_unwind(unwind, at);
            }
            TerminatorKind::Yield { value, resume, resume_arg, drop } => {
                self.visit_operand(value, at);
                self.visit_place(resume_arg, Write(W::Yield), at);
                self.visit_target(resume, false, at);
                if let Some(target) = drop {
                    self.visit_target(target, false, at);
                }
            }
            TerminatorKind::FalseEdge { real_target, imaginary_target } => {
                self.visit_target(real_target, false, at);
                self.visit_target(imaginary_target, false, at);
            }
            TerminatorKind::FalseUnwind { real_target, unwind } => {
                self.visit_target(real_target, false, at);
                self.visit_unwind(unwind, at);
            }
            TerminatorKind::Return => {
                // rustc visit.rs:531: return's implicit _0 operand cannot be renamed.
                let mut local = Local(0);
                self.visit_local(&mut local, Read(R::Move), at);
                assert_eq!(local, Local(0), "MutVisitor changed return's implicit local");
            }
            TerminatorKind::UnwindResume
            | TerminatorKind::UnwindTerminate(_)
            | TerminatorKind::Unreachable
            | TerminatorKind::CoroutineDrop
            | TerminatorKind::InlineAsm(_) => {}
        }
    }
    fn visit_rvalue(&mut self, rv: &mut Rvalue, at: Location) {
        self.super_rvalue(rv, at);
    }
    fn super_rvalue(&mut self, rv: &mut Rvalue, at: Location) {
        match rv {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.visit_operand(op, at),
            Rvalue::Repeat(op, len) => {
                self.visit_operand(op, at);
                self.visit_const(len, at);
            }
            Rvalue::Ref(region, kind, place) => {
                self.visit_region(region);
                let context = match kind {
                    BorrowKind::Shared => Read(R::SharedBorrow),
                    BorrowKind::Fake(_) => Read(R::FakeBorrow),
                    BorrowKind::Mut { .. } => Write(W::Borrow),
                };
                self.visit_place(place, context, at);
            }
            Rvalue::RawPtr(kind, place) => {
                let context = match kind {
                    RawPtrKind::Const => Read(R::RawBorrow),
                    RawPtrKind::Mut => Write(W::RawBorrow),
                    RawPtrKind::FakeForPtrMetadata => Read(R::Inspect),
                };
                self.visit_place(place, context, at)
            }
            Rvalue::Len(place) | Rvalue::Discriminant(place) => self.visit_place(place, Read(R::Inspect), at),
            Rvalue::CopyForDeref(place) => self.visit_place(place, Read(R::Inspect), at),
            Rvalue::Cast(_, op, ty) | Rvalue::ShallowInitBox(op, ty) => {
                self.visit_operand(op, at);
                self.visit_ty(ty);
            }
            Rvalue::BinaryOp(_, ops) => {
                self.visit_operand(&mut ops.0, at);
                self.visit_operand(&mut ops.1, at);
            }
            Rvalue::NullaryOp(_, ty) => self.visit_ty(ty),
            Rvalue::ThreadLocalRef(def) => self.visit_def(def),
            Rvalue::Aggregate(kind, ops) => {
                self.visit_aggregate_kind(kind);
                for op in ops {
                    self.visit_operand(op, at);
                }
            }
        }
    }
    fn visit_generic_arg(&mut self, arg: &mut crate::consts::GenericArg) {
        match arg {
            crate::consts::GenericArg::Type(ty) => self.visit_ty(ty),
            crate::consts::GenericArg::Lifetime(r) => self.visit_type_region(r),
            crate::consts::GenericArg::Const(ct) => self.visit_type_const(ct),
        }
    }
    fn visit_type_region(&mut self, _region: &mut crate::types::RegionId) {}
    fn visit_type_const(&mut self, _ct: &mut crate::consts::Const) {}
    fn visit_aggregate_kind(&mut self, kind: &mut AggregateKind) {
        match kind {
            AggregateKind::Array(ty) | AggregateKind::RawPtr(ty, _) => self.visit_ty(ty),
            AggregateKind::Adt(def, _, args, user, _) => {
                self.visit_def(def);
                for ty in args {
                    self.visit_generic_arg(ty);
                }
                if let Some(user) = user {
                    self.visit_user_type_index(user);
                }
            }
            AggregateKind::Closure(_, args) | AggregateKind::Coroutine(_, args) => {
                for ty in args {
                    self.visit_generic_arg(ty);
                }
            }
            AggregateKind::Tuple => {}
        }
    }
    fn visit_operand(&mut self, op: &mut Operand, at: Location) {
        self.super_operand(op, at);
    }
    fn super_operand(&mut self, op: &mut Operand, at: Location) {
        match op {
            Operand::Copy(place) => self.visit_place(place, Read(R::Copy), at),
            Operand::Move(place) => self.visit_place(place, Read(R::Move), at),
            Operand::Constant(c) => {
                self.visit_span(&mut c.span);
                if let Some(user) = &mut c.user_ty {
                    self.visit_user_type_index(user);
                }
                self.visit_const(&mut c.const_, at);
            }
        }
    }
    fn visit_const(&mut self, c: &mut Const, _at: Location) {
        match c {
            Const::Val(value, ty) => {
                self.visit_ty(ty);
                match value {
                    ConstValue::Scalar(Scalar::Ptr { provenance: CtfeProvenance { alloc, .. }, .. })
                    | ConstValue::Slice { alloc, .. }
                    | ConstValue::Indirect { alloc, .. } => self.visit_alloc_id(alloc),
                    ConstValue::Scalar(Scalar::Int { .. }) | ConstValue::ZeroSized => {}
                }
            }
            Const::Unevaluated { def, args, .. } => {
                self.visit_def(def);
                for arg in args {
                    self.visit_generic_arg(arg);
                }
            }
        }
    }
    fn visit_place(&mut self, place: &mut Place, context: PlaceContext, at: Location) {
        self.super_place(place, context, at);
    }
    fn super_place(&mut self, place: &mut Place, context: PlaceContext, at: Location) {
        self.visit_local(&mut place.local, context, at);
        for elem in &mut place.projection {
            self.visit_projection_elem(elem, context, at);
        }
    }
    fn visit_projection_elem(&mut self, elem: &mut PlaceElem, context: PlaceContext, at: Location) {
        match elem {
            PlaceElem::Field(_, ty) | PlaceElem::OpaqueCast(ty) | PlaceElem::Subtype(ty) => self.visit_ty(ty),
            PlaceElem::Index(local) => self.visit_local(local, if context.is_use() { Read(R::Copy) } else { context }, at),
            PlaceElem::Deref | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } | PlaceElem::Downcast(..) => {}
        }
    }
    fn visit_user_type_projection(&mut self, user: &mut UserTypeProjection, at: Location) {
        self.visit_user_type_index(&mut user.base);
        for elem in &mut user.projection {
            self.visit_projection_elem(elem, Non(N::AscribeUserTy(Variance::Invariant)), at);
        }
    }
    fn visit_assert(&mut self, msg: &mut AssertKind, at: Location) {
        match msg {
            AssertKind::BoundsCheck { len: a, index: b }
            | AssertKind::Overflow(_, a, b)
            | AssertKind::MisalignedPointerDereference { required: a, found: b } => {
                self.visit_operand(a, at);
                self.visit_operand(b, at);
            }
            AssertKind::OverflowNeg(op) | AssertKind::DivisionByZero(op) | AssertKind::RemainderByZero(op) => self.visit_operand(op, at),
            AssertKind::ResumedAfterReturn(_) | AssertKind::ResumedAfterPanic(_) | AssertKind::NullPointerDereference => {}
        }
    }
    fn visit_unwind(&mut self, unwind: &mut UnwindAction, at: Location) {
        if let UnwindAction::Cleanup(target) = unwind {
            self.visit_target(target, true, at);
        }
    }
    fn visit_source_info(&mut self, info: &mut SourceInfo) {
        self.visit_span(&mut info.span);
        self.visit_source_scope(&mut info.scope);
    }
    fn visit_local(&mut self, _local: &mut Local, _context: PlaceContext, _at: Location) {}
    fn visit_target(&mut self, _target: &mut BasicBlock, _unwind: bool, _at: Location) {}
    fn visit_ty(&mut self, _ty: &mut TyId) {}
    fn visit_def(&mut self, _def: &mut DefId) {}
    fn visit_region(&mut self, _region: &mut Region) {}
    fn visit_span(&mut self, _span: &mut Span) {}
    fn visit_source_scope(&mut self, _scope: &mut SourceScope) {}
    fn visit_user_type_index(&mut self, _index: &mut UserTypeAnnotationIndex) {}
    fn visit_user_type(&mut self, _user: &mut UserTypeId) {}
    fn visit_alloc_id(&mut self, _alloc: &mut AllocId) {}
}
