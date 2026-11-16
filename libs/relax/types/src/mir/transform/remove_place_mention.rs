//! Remove PlaceMention statements before the final CFG simplification.
use crate::mir::*;

/// Port of rustc_mir_transform/src/remove_place_mention.rs:16-24.
/// Uses rustc's default (mir_preserve_ub is false).
pub fn remove_place_mention(body: &mut Body) {
    for block in &mut body.basic_blocks {
        block.statements.retain(|statement| !matches!(statement.kind,
            StatementKind::PlaceMention(..) | StatementKind::Nop));
    }
}
