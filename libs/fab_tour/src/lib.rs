//! # makepad-fab-tour — automated cinematic fly/walk-throughs
//!
//! Give it a building; it gives you camera tracks worth watching, and proof
//! that none of them fly through a wall.
//!
//! ```no_run
//! use makepad_fab_tour::*;
//! # fn main() { let scene = synthetic::villa();
//! let site = SiteAnalysis::analyse(&scene, &AnalysisConfig::default());
//! let tracks = shots::all_shots(&site, &ShotOptions::default());
//! for t in &tracks {
//!     let report = qa::check(&site, t, &QaLimits::default());
//!     assert!(report.passed(), "{}", report.summary());
//! }
//! # }
//! ```
//!
//! ## The layers
//!
//! | module | what it does |
//! |---|---|
//! | [`scene`] | `TourScene`, the flat input contract: triangles + element classes + storeys |
//! | [`voxel`] | occupancy (solid / sealed / opaque), the exact-EDT clearance field, interior vs exterior |
//! | [`analysis`] | storeys, the watershed room graph, portals, stairs, entrance, façades, POI ranking |
//! | [`route`] | A\* over the walkable lattice, string pull, room visit order |
//! | [`path`] | waypoints → centripetal spline → collision relax → constant-speed, eased, gaze-smoothed track |
//! | [`shots`] | the shot catalogue: reveal, approach, walkthrough, fly-through, orbit, storey reveal, full tour |
//! | [`qa`] | fly every track headless and assert it is safe, comfortable and truthful |
//! | [`raster`] | a tiny z-buffered renderer so a human can judge the shots |
//!
//! ## The one law
//!
//! Everything that asks "is there room here" asks
//! [`analysis::ClearanceField`], and nothing asks anything else. The planner,
//! the string pull, the spline relaxer and the QA harness share one oracle.
//!
//! The Doom walker (`libs/render/src/{level,player_nav}.rs`) paid for this
//! lesson: its navigation graph and its body used two nearly-identical wall
//! tests, and the sliver of disagreement between them was a band of ledge
//! heights the graph offered and the body then refused, forever. Two clearance
//! functions is a bug with a delay fuse. `tests/one_clearance.rs` keeps it one.
//!
//! ## Coordinates
//!
//! Right-handed, **Z up**, metres — inherited from `makepad-fab-shell` and
//! never converted again.

pub mod analysis;
pub mod geom;
pub mod path;
pub mod qa;
pub mod raster;
pub mod route;
pub mod scene;
pub mod shots;
pub mod synthetic;
pub mod track;
pub mod voxel;

pub use analysis::{
    AnalysisConfig, Body, ClearMode, ClearanceField, Entrance, Facade, Opening, Portal, Room,
    SiteAnalysis, StairLink, StoreyPlan, WalkEntryPose,
};
pub use path::{Gaze, MotionProfile, Waypoint};
pub use qa::{check, check_all, QaFailure, QaKind, QaLimits, QaReport};
pub use scene::{TourClass, TourElement, TourScene, TourSceneBuilder, TourStorey};
pub use shots::{all_shots, full_tour, OrbitTarget, ShotOptions};
pub use track::{CameraTrack, ShotKind, TourKey, TrackNote};
pub use voxel::{VoxelConfig, VoxelGrid};

pub use makepad_math;

/// Analyse and generate in one call — what the app's worker thread wants.
pub fn plan(scene: &TourScene) -> (SiteAnalysis, Vec<CameraTrack>) {
    let site = SiteAnalysis::analyse(scene, &AnalysisConfig::default());
    let tracks = all_shots(&site, &ShotOptions::default());
    (site, tracks)
}
