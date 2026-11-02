//! Primitive sweeps, skinning, blends and offset ruled surfaces.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_repeated_linear_sweep`] | repeated `CreateLinearSweep` |
//! | [`PrimitiveCreation::create_rotational_sweep`] | full `CreateRotationalSweep` |
//! | [`PrimitiveCreation::create_partial_rotational_sweep`] | partial `CreateRotationalSweep` |
//! | [`PrimitiveCreation::create_swung_primitive`] | `CreateSwungPrimitive` |
//! | [`PrimitiveCreation::create_curve_sweep`] | `CreateCurveSweep` |
//! | [`PrimitiveCreation::create_skin_from_faces`] | `CreateSkinFromFaces` |
//! | [`PrimitiveCreation::create_blend_primitive`] | `CreateBlendPrimitive` |
//! | [`ruled_surface_from_offset`] | `sm_CreateRuledSurfaceFromOffsetCurve` |
//!
//! Deviations: typed ids and owned geometry replace raw pointers. Topology
//! services share the existing `SweepAccess` boundary via [`PrimitiveAccess`];
//! a caller must supply its concrete resolver. Geometry preparation is factored
//! into [`prepare_curve_sweep`]. Scaling sampling is capped at 1,000,000 points;
//! the reused sweep engine caps repetitions at 4096. No recursion here.
//! Missing topology edits propagate `Unported` while their complete caller tails
//! remain implemented; successful topology construction is not yet certified.

mod access;
mod construct;
mod geometry;
mod ruled;
mod sweeps;

pub use super::p1::PrimitiveCreation;
pub use access::PrimitiveAccess;
pub use construct::BlendPrimitiveOptions;
pub use geometry::{prepare_curve_sweep, PreparedCurveSweep};
pub use ruled::{ruled_surface_from_offset, OffsetRuleOptions};
pub use sweeps::RepeatedSweepOptions;
