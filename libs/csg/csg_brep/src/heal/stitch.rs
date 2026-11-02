//! Stitching coincident topology: gluing coincident vertex and edge pairs of a
//! B-rep, face-pair stitching and consistent face orientation (`SmStitch`).
//!
//! Upstream: `source/SMLib/src/SmStitch.cpp`, `source/SMLib/inc/SmStitch.h`, and the
//! stitch entry points of `source/SM_API/src/SmApiBrep.cpp`. Split by the port plan
//! into [`p1`] (the callback, the vertex/edge pair gluing, face-pair stitching, normal
//! unification and the API entry points) and [`p2`] (`DoStitching`, `StitchIntoSolid`,
//! `StitchIntoShell`, `SplitEdgesWithVertices`, the one-list `StitchVertexPairs`).

pub mod p1;
pub mod p2;
