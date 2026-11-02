//! Dropping 3D curves onto surfaces (`SmSurface::DropCurve` and its helpers): the
//! drop point record `SmDropPt`, span tests, seam and boundary fix-ups, the iso-curve
//! shortcut `DropIsoCurve` and `DropAndTrimCurve`.
//!
//! Upstream: `source/SMLib/src/SmSurfaceDropLib.cpp`. The file is split over several
//! plan units (`intersect.surface_drop_lib_p1`, `_p2`, `_p3`); each lives in its own
//! submodule.

pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::DropPt;
