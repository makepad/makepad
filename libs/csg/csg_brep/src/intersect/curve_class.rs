//! Curve classification: a curve split into homogeneous intervals by where it meets
//! the boundaries of the object it is classified against, each interval carrying the
//! point classifications of its ends and interior (`SmCurveClassification`).
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//! The file is split over several plan units (`intersect.curve_class_p1`, `_p2`, ...);
//! each lives in its own submodule and adds methods to [`CurveClassification`] (or
//! free functions).

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;
pub mod p7;
pub mod p8;
pub mod p9;

pub use p1::{
    apply_point_solution, compute_face_point_class, cull_or_refine_cci_in_3d, get_cci_geometry_in,
    get_cci_surface_gaps, get_connected_touches, get_curve_curve_angle, get_winding_count,
    interval_obj_zone_tol_3d, make_consistent_classifications, object_dimension,
    set_xsect_curve_through_point, set_xsect_edge_through_point_in, src_zone_tol_3d_of,
    xsect_tol_2d_or_3d_for_classify_pair, ApplyPointSolution, ApplyPointSolutionInput,
    CciGeometryRefs, CciSurfaceGaps, CrvDirOnFaceType, CurveClassification, CurveInterval,
    IntervalPosition, TouchData, XSectThroughPoint, CURVE_CLASSIFICATION_TYPE_STRING,
    MAX_INTERVALS,
};
pub use p2::{refine_cci_to_surfaces, RefineCci, ResetOptions};
pub use p3::{
    change_edge_point_class_to_near_vertex, curve_is_degenerate, fix_problems_by_squeezing,
    FoundInterval, NO_INTERVAL,
};
pub use p4::{squeeze_out_smallest_interval, ClassificationInit, LoopCrossings};
pub use p7::{
    set_do_filleting_geometry, set_do_filleting_geometry_mut, CoincidentMaybe, PartialCoincidence,
    VertexObjects, UNDEF_LOOP_INDEX,
};
