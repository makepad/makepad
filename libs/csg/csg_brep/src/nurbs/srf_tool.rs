//! Tool routines that act on NLib NURBS surfaces: knot insertion and refinement,
//! splitting, Bezier decomposition, iso-curve and sub-patch extraction, knot
//! removal, joining and compatibility.
//!
//! Upstream: `source/SMLib/src/NL_SrfTool.cpp` (declarations in
//! `source/SMLib/inc/NL_SrfTool.h`). The file is split over several plan units
//! (`nlib.srf_tool_p1`, `_p2`, ...); each lives in its own submodule.

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
