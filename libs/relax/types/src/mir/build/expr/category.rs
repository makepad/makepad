//! rustc_mir_build/src/builder/expr/category.rs: which lowering function an expression is evaluated by.
use crate::mir::build::thir::ExprKind;

#[derive(Debug, PartialEq, Eq)]
pub enum Category {
    /// An assignable memory location (`x`, `x.f`, `*x`, `x[i]`).
    Place,
    /// A constant that needs no temporary.
    Constant,
    /// Everything else is an rvalue, evaluated via `as_rvalue` or `into`.
    Rvalue(RvalueFunc),
}
/// Rvalues that can be lowered both ways use `AsRvalue`; those that only make sense into a destination use `Into`.
#[derive(Debug, PartialEq, Eq)]
pub enum RvalueFunc {
    Into,
    AsRvalue,
}

impl Category {
    /// category.rs:`Category::of`; None for `Scope`/`Use`-like wrappers the caller must look through.
    pub fn of(kind: &ExprKind) -> Option<Category> {
        match kind {
            ExprKind::Scope { .. } => None,
            ExprKind::VarRef { .. } | ExprKind::UpvarRef { .. } | ExprKind::Deref { .. } | ExprKind::Index { .. }
            | ExprKind::Field { .. } | ExprKind::PlaceTypeAscription { .. } | ExprKind::StaticRef { .. }
            | ExprKind::ThreadLocalRef(_) => Some(Category::Place),
            ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::NeverToAny { .. } | ExprKind::Let { .. } | ExprKind::Block { .. }
            | ExprKind::Assign { .. } | ExprKind::AssignOp { .. } | ExprKind::Break { .. } | ExprKind::Continue { .. }
            | ExprKind::Return { .. } | ExprKind::Loop { .. } | ExprKind::Call { .. } | ExprKind::Yield { .. } | ExprKind::Unreachable
            | ExprKind::Adt(..) => Some(Category::Rvalue(RvalueFunc::Into)),
            ExprKind::Literal { .. } | ExprKind::NonHirLiteral { .. } | ExprKind::ZstLiteral { .. } | ExprKind::ConstParam { .. }
            | ExprKind::ConstBlock { .. } | ExprKind::NamedConst { .. } => Some(Category::Constant),
            ExprKind::Array { .. } | ExprKind::Tuple { .. } | ExprKind::Closure(..) | ExprKind::Unary { .. } | ExprKind::Binary { .. }
            | ExprKind::LogicalOp { .. } | ExprKind::Cast { .. } | ExprKind::Use { .. } | ExprKind::ByUse { .. }
            | ExprKind::PointerCoercion { .. } | ExprKind::Repeat { .. } | ExprKind::Borrow { .. } | ExprKind::RawBorrow { .. }
            | ExprKind::ValueTypeAscription { .. } => Some(Category::Rvalue(RvalueFunc::AsRvalue)),
        }
    }
}
