//! Free geometric utility functions (`smgu_*`): distances, segment, circle, plane,
//! sphere and ellipse intersections, best-fit shapes, small dense matrices and
//! curve continuity.
//!
//! Upstream: `source/SMLib/src/SmGeomUtility.cpp`, `source/SMLib/inc/SmGeomUtility.h`.
//! The file is split over several plan units (`geom.geom_utility_p1`, `_p2`, ...);
//! each lives in its own submodule.

pub mod p1;
pub mod p2;
