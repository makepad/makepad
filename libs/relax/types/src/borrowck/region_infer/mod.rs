// ported from N:rustc_borrowck/src/region_infer/mod.rs @8d1a764
mod region_context;
pub mod values;
pub mod reverse_sccs;
pub mod opaque_types;
mod scc;
pub use region_context::*;
pub use scc::ConstraintSccs;
use super::constraints::RegionVid;
use crate::types::{RegionId, UniverseIndex, TyId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NllRegionVariableOrigin { FreeRegion, Placeholder(RegionId), Existential }
#[derive(Clone, Copy, Debug)]
pub struct RegionDefinition {
    pub origin: NllRegionVariableOrigin,
    pub universe: UniverseIndex,
    pub external_name: Option<RegionId>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Representative { FreeRegion(RegionVid), Placeholder(RegionVid), Existential(RegionVid) }
impl Representative {
    pub fn rvid(self) -> RegionVid {
        match self { Self::FreeRegion(r) | Self::Placeholder(r) | Self::Existential(r) => r }
    }
    pub fn new(r: RegionVid, d: &RegionDefinition) -> Self {
        match d.origin {
            NllRegionVariableOrigin::FreeRegion => Self::FreeRegion(r),
            NllRegionVariableOrigin::Placeholder(_) => Self::Placeholder(r),
            NllRegionVariableOrigin::Existential => Self::Existential(r),
        }
    }
}
#[derive(Clone, Debug)]
pub enum RegionErrorKind {
    RegionError { longer_fr: RegionVid, shorter_fr: RegionVid, is_reported: bool },
    PlaceholderOutlivesIllegalRegion { longer_fr: RegionVid, illegally_outlived_r: RegionVid },
    TypeTest { test: usize },
}
#[derive(Clone, Debug)]
pub struct TypeTest { pub generic_kind: TyId, pub lower_bound: RegionVid, pub span: super::constraints::Span, pub verify_bound: VerifyBound }
#[derive(Clone, Debug)]
pub enum VerifyBound { IfEq { ty: TyId, bound: RegionId }, IsEmpty, OutlivedBy(RegionVid), AnyBound(Vec<VerifyBound>), AllBounds(Vec<VerifyBound>) }

#[derive(Clone, Copy, Debug)]
pub struct ClosureTypeRequirement { pub subject: TyId, pub outlived_free_region: RegionVid, pub blame_span: super::constraints::Span }

#[cfg(test)]
mod tests;
