//! The B-rep object (`SmBrep`): construction from serialized data, editing and
//! queries, split into the units `topology.brep_p1` .. `topology.brep_p10`.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust      | Upstream                                                        |
//! |-----------|-----------------------------------------------------------------|
//! | [`cutting`] | `SmBrepCutting`, `SmPlaneCutter`, `SmApiCut` (unit `topology.brep_cutting`) |
//! | [`local_operation`] | `LocalOperation`                                     |
//! | [`mirror`]| `Mirror` (unit `topology.brep_mirror`)                          |
//! | [`p1`]    | see the module                                                  |
//! | [`p2`]    | see the module                                                  |
//! | [`p4`]    | `~SmBrep`, `DeleteEdge`, `DeleteVertex`, `StitchFaces`, `ShrinkGeometry` |
//! | [`p5`]    | B-rep editing and transformation operations, part 5             |
//! | [`p6`]    | see the module                                                  |
//! | [`p7`]    | see the module                                                  |
//! | [`planar_section`] | `CreatePlanarSectionCurves` (unit `topology.brep_planar_section`) |
//! | [`p8`]    | making faces from surfaces and from loops of curves              |
//! | [`p10`]   | `MakeTopologyFromData`, `ProjectAndTrim`, `RemoveAllTopology`, `TestClosureSplitRegion` (edge version) |
//!
//! Deviations: the B-rep records are the arenas of [`crate::topology::Brep`].

pub mod cutting;
pub mod local_operation;
pub mod mirror;
pub mod p1;
pub mod p10;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;
pub mod p7;
pub mod p8;
pub mod p9;
pub mod planar_section;
pub mod validation;
