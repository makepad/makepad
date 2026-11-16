//! Remove analysis-only MIR after borrow checking.
use crate::mir::*;

/// Port of rustc_mir_transform/src/cleanup_post_borrowck.rs:27-84.
/// Relax has no Coverage statements or CFG caches; user_ty is a vector.
pub fn cleanup_post_borrowck(body: &mut Body) {
    for block in &mut body.basic_blocks {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::AscribeUserType(..)
                | StatementKind::Assign(_, Rvalue::Ref(_, BorrowKind::Fake(_), _))
                | StatementKind::FakeRead(..)
                | StatementKind::BackwardIncompatibleDropHint { .. } => {
                    statement.kind = StatementKind::Nop;
                }
                StatementKind::Assign(_, Rvalue::Cast(kind @ CastKind::PointerCoercion(
                    PointerCoercion::ArrayToPointer | PointerCoercion::MutToConstPointer, _), ..)) => {
                    *kind = CastKind::PtrToPtr;
                }
                _ => {}
            }
        }
        let terminator = block.terminator.as_mut().expect("missing terminator");
        match terminator.kind {
            TerminatorKind::FalseEdge { real_target, .. }
            | TerminatorKind::FalseUnwind { real_target, .. } => {
                terminator.kind = TerminatorKind::Goto { target: real_target };
            }
            _ => {}
        }
    }
    body.user_type_annotations.clear();
    for decl in &mut body.local_decls { decl.user_ty.clear(); }
}
