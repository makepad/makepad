// ported from N:rustc_borrowck/src/constraints/mod.rs @8d1a764
// ConstraintCategory/ReturnConstraint/AnnotationSource: N:rustc_middle/src/mir/query.rs:98-173 @8d1a764
pub mod graph;
use crate::mir::FieldIdx;
use crate::program::DefId;
use crate::types::TyId;
pub type RegionVid = usize;
pub type ConstraintSccIndex = usize;
pub type OutlivesConstraintIndex = usize;

/// Order of declaration follows nightly; blame priority is `interest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConstraintCategory {
    Return(ReturnConstraint), Yield, UseAsConst, UseAsStatic, TypeAnnotation(AnnotationSource),
    /// `unsize_to` has its region variables erased (ReErased).
    Cast { is_raw_ptr_dyn_type_cast: bool, is_implicit_coercion: bool, unsize_to: Option<TyId> },
    /// The callee's region-erased function type, when available.
    CallArgument(Option<TyId>), CopyBound, SizedBound, Assignment, Usage, OpaqueType,
    ClosureUpvar(FieldIdx), Predicate(crate::mir::Span, DefId),
    Boring, BoringNoLocation, Internal, OutlivesUnnameablePlaceholder(RegionVid),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReturnConstraint { Normal, ClosureUpvar(FieldIdx) }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnnotationSource { Ascription, Declaration, OpaqueCast, GenericArg }
impl ConstraintCategory {
    /// N:region_infer/region_context.rs:700-753. The unsizing-cast case (interest 1)
    /// depends on the blamed path and `'static`; best_blame_constraint applies it.
    pub fn interest(self) -> u8 {
        match self {
            Self::Return(_) => 0,
            Self::Yield | Self::UseAsConst | Self::UseAsStatic
            | Self::TypeAnnotation(AnnotationSource::Ascription | AnnotationSource::Declaration | AnnotationSource::OpaqueCast)
            | Self::Cast { .. } | Self::CallArgument(_) | Self::CopyBound | Self::SizedBound
            | Self::Assignment | Self::Usage | Self::ClosureUpvar(_) => 2,
            Self::TypeAnnotation(AnnotationSource::GenericArg) => 3,
            Self::Predicate(..) | Self::OpaqueType => 4, Self::Boring => 5,
            Self::BoringNoLocation => 6, Self::Internal => 7,
            Self::OutlivesUnnameablePlaceholder(_) => 8,
        }
    }
}
/// `location` is the dense point of nightly's `Locations::Single`; `None` is `Locations::All`.
/// The span is file-qualified (the MIR span of the location, or of the annotation).
#[derive(Clone, Copy, Debug)]
pub struct OutlivesConstraint {
    pub sup: RegionVid,
    pub sub: RegionVid,
    pub location: Option<usize>,
    pub span: crate::mir::Span,
    pub category: ConstraintCategory,
    /// Whether this constraint was propagated from a closure or coroutine body.
    pub from_closure: bool,
}
#[derive(Clone, Debug, Default)]
pub struct OutlivesConstraintSet { pub outlives: Vec<OutlivesConstraint> }
impl OutlivesConstraintSet {
    pub fn push(&mut self, constraint: OutlivesConstraint) {
        if constraint.sup != constraint.sub { self.outlives.push(constraint); }
    }
    pub fn graph(&self, num_region_vars: usize) -> graph::ConstraintGraph {
        graph::ConstraintGraph::new(self, num_region_vars, false)
    }
    pub fn reverse_graph(&self, num_region_vars: usize) -> graph::ConstraintGraph {
        graph::ConstraintGraph::new(self, num_region_vars, true)
    }
}

/// BIR span (lo/hi within the body's file); constraint spans are `mir::Span`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub struct Span { pub lo: u32, pub hi: u32 }
