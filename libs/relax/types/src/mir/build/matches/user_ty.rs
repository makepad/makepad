//! rustc_mir_build/src/builder/matches/user_ty.rs: a linked list of user-type projections built on
//! the stack while visiting a THIR pattern (avoids cloning a partly built projection per node).
use crate::mir::{FieldIdx, UserTypeAnnotationIndex, UserTypeProjection, UserTypeProjectionElem, VariantIdx};
use crate::program::{DefId, Sym};

/// One of a list of "operations" that lazily build projections of user-specified types.
#[derive(Debug)]
pub(super) enum ProjectedUserTypesOp {
    PushUserTypes { base_types: Vec<UserTypeAnnotationIndex> },
    Index,
    Subslice { from: u64, to: u64 },
    Deref,
    Leaf { field: FieldIdx },
    Variant { name: Option<Sym>, variant: VariantIdx, field: FieldIdx },
}

#[derive(Debug)]
pub(super) enum ProjectedUserTypesNode<'a> {
    None,
    Chain { parent: &'a Self, op: ProjectedUserTypesOp },
}

impl<'a> ProjectedUserTypesNode<'a> {
    pub(super) fn push_user_types(&'a self, base_types: Vec<UserTypeAnnotationIndex>) -> Self {
        assert!(!base_types.is_empty());
        // Pushing one or more base user types always makes the chain non-empty.
        Self::Chain { parent: self, op: ProjectedUserTypesOp::PushUserTypes { base_types } }
    }

    /// Push another projection op onto the chain, but only if it is already non-empty.
    fn maybe_push(&'a self, op_fn: impl FnOnce() -> ProjectedUserTypesOp) -> Self {
        match self {
            Self::None => Self::None,
            Self::Chain { .. } => Self::Chain { parent: self, op: op_fn() },
        }
    }

    pub(super) fn index(&'a self) -> Self {
        self.maybe_push(|| ProjectedUserTypesOp::Index)
    }

    pub(super) fn subslice(&'a self, from: u64, to: u64) -> Self {
        self.maybe_push(|| ProjectedUserTypesOp::Subslice { from, to })
    }

    pub(super) fn deref(&'a self) -> Self {
        self.maybe_push(|| ProjectedUserTypesOp::Deref)
    }

    pub(super) fn leaf(&'a self, field: FieldIdx) -> Self {
        self.maybe_push(|| ProjectedUserTypesOp::Leaf { field })
    }

    /// `name` is the variant's name (`adt_def.variant(variant).name`).
    pub(super) fn variant(&'a self, _adt_def: DefId, name: Option<Sym>, variant: VariantIdx, field: FieldIdx) -> Self {
        self.maybe_push(|| ProjectedUserTypesOp::Variant { name, variant, field })
    }

    /// The chain's ops from the root, in application order.
    fn ops(&'a self) -> Vec<&'a ProjectedUserTypesOp> {
        let mut ops = Vec::new();
        let mut next = self;
        while let Self::Chain { parent, op } = next {
            ops.push(op);
            next = parent;
        }
        ops.reverse();
        ops
    }

    /// Assembles this chain of user-type projections into a proper data structure.
    pub(super) fn build_user_type_projections(&self) -> Vec<UserTypeProjection> {
        if matches!(self, Self::None) {
            return Vec::new();
        }
        let ops = self.ops();
        // The first op is always `PushUserTypes`; other ops are only added when there is a user type.
        assert!(matches!(ops.first(), Some(ProjectedUserTypesOp::PushUserTypes { .. })));
        let mut projections: Vec<UserTypeProjection> = vec![];
        for op in ops {
            match op {
                ProjectedUserTypesOp::PushUserTypes { base_types } => {
                    assert!(!base_types.is_empty());
                    for &base in base_types {
                        projections.push(UserTypeProjection { base, projection: vec![] })
                    }
                }
                ProjectedUserTypesOp::Index => {
                    for p in &mut projections {
                        p.projection.push(UserTypeProjectionElem::Index)
                    }
                }
                ProjectedUserTypesOp::Subslice { from, to } => {
                    for p in &mut projections {
                        p.projection.push(UserTypeProjectionElem::Subslice { from: *from, to: *to, from_end: true })
                    }
                }
                ProjectedUserTypesOp::Deref => {
                    for p in &mut projections {
                        p.projection.push(UserTypeProjectionElem::Deref)
                    }
                }
                ProjectedUserTypesOp::Leaf { field } => {
                    for p in &mut projections {
                        p.projection.push(UserTypeProjectionElem::Field(*field))
                    }
                }
                ProjectedUserTypesOp::Variant { name, variant, field } => {
                    for p in &mut projections {
                        p.projection.push(UserTypeProjectionElem::Downcast(*name, *variant));
                        p.projection.push(UserTypeProjectionElem::Field(*field));
                    }
                }
            }
        }
        projections
    }
}
