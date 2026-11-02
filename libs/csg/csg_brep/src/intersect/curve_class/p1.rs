//! Curve classification, part 1: the interval and classification containers, their
//! accessors and storage management, the touch-list helpers, and the static helpers
//! that tighten classification-curve / edge intersections onto surf/surf/surf points.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`;
//! the `SmCurveClassification` / `SmCurveInterval` overloads of `SmTol` in
//! `source/SMLib/src/SmTol.cpp`.
//!
//! | Rust                                                   | Upstream                                         |
//! |--------------------------------------------------------|--------------------------------------------------|
//! | [`IntervalPosition`]                                   | `SmIntervalPositionType`                         |
//! | [`CrvDirOnFaceType`]                                   | `SmCrvDirOnFaceType`                             |
//! | [`TouchData`]                                          | `SmTouchData`                                    |
//! | [`get_connected_touches`]                              | `SmTouchData::GetConnectedTouches`               |
//! | [`remove_non_crossings`]                               | `SmTouchData::RemoveNonCrossings`                |
//! | [`get_winding_count`]                                  | `SmTouchData::GetWindingCount`                   |
//! | [`CurveInterval`]                                      | `SmCurveInterval`                                |
//! | [`CurveInterval::new`], [`CurveInterval::with_interval`] | `SmCurveInterval(ctx, owner)`, `SmCurveInterval(ivl, ctx, owner)` |
//! | [`CurveInterval::reset`]                               | `SmCurveInterval::ReSet`                         |
//! | [`CurveInterval::set_interval`]                        | `SmCurveInterval::SetInterval`                   |
//! | `Clone` / `Copy` (derived)                             | `SmCurveInterval::operator=`                     |
//! | [`CurveInterval::set_interval_min`], [`CurveInterval::set_interval_max`] | `SetIntervalMin`, `SetIntervalMax` (private) |
//! | [`CurveInterval::interval`], [`CurveInterval::min`], [`CurveInterval::max`] | `GetInterval`, `GetMin`, `GetMax` |
//! | [`CurveClassification`]                                | `SmCurveClassification`                          |
//! | [`CurveClassification::new`]                           | `SmCurveClassification()`                        |
//! | `Drop` (implicit)                                      | `~SmCurveClassification`                         |
//! | [`CurveClassification::size`]                          | `GetSize`                                        |
//! | [`CurveClassification::face`]                          | `GetFace`                                        |
//! | [`CurveClassification::surface1`], [`CurveClassification::surface2`] | `GetSurface1`, `GetSurface2`        |
//! | [`CurveClassification::curve`]                         | `GetCurve`                                       |
//! | [`CurveClassification::set_interval`]               | `SetInterval` (inline)                           |
//! | [`CurveClassification::interval`]                      | `GetInterval`                                    |
//! | [`CurveClassification::sub_interval`]                  | `GetSubInterval`                                 |
//! | [`CurveClassification::uv_curve1`], [`CurveClassification::uv_curve2`] | `GetUVCurve1`, `GetUVCurve2`      |
//! | [`CurveClassification::any_uv_curve1`], [`CurveClassification::any_uv_curve2`] | `GetAnyUVCurve1`, `GetAnyUVCurve2` |
//! | [`CurveClassification::topology_intersector`]          | `GetTopologyIntersector`                         |
//! | [`CurveClassification::is_curve_current`]              | `GetIsCurveCurrent`                              |
//! | [`CurveClassification::ignore_tangency`]               | `GetIgnoreTangency`                              |
//! | [`CurveClassification::set_doing_cross_hatch`]         | `SetDoingCrossHatch`                             |
//! | [`CurveClassification::doing_cross_hatch`]             | `GetDoingCrossHatch`                             |
//! | [`CurveClassification::doing_fillets`]                 | `GetDoingFillets`                                |
//! | [`CurveClassification::other_classification`]          | `GetOtherClassification`                         |
//! | [`CurveClassification::ok_to_set_tolerance`]           | `GetOKToSetTolerance`                            |
//! | [`CurveClassification::tgt_loops_only`], [`CurveClassification::set_tgt_loops_only`] | `GetTgtLoopsOnly`, `SetTgtLoopsOnly` |
//! | [`CurveClassification::tgt_loops`], [`CurveClassification::add_tgt_loop`] | `GetTgtLoops`, `AddTgtLoop`   |
//! | [`CurveClassification::src_zone_tol_3d`]               | `GetSrcZoneTol3d` (private)                      |
//! | [`CurveClassification::set_src_zone_tol_3d`]           | `SetSrcZoneTol3d` (private; through `SmTol::SetZoneTol3d`) |
//! | [`CurveClassification::at`], [`CurveClassification::at_mut`] | `GetAt`, `operator[]`                      |
//! | [`CurveClassification::set_surface1`], [`CurveClassification::set_surface2`] | `SetSurface1`, `SetSurface2` |
//! | [`CurveClassification::set_face`]                      | `SetFace`                                        |
//! | [`CurveClassification::set_ignore_tangency`]           | `SetIgnoreTangency`                              |
//! | [`CurveClassification::set_is_curve_owned`], [`CurveClassification::is_curve_owned`] | `SetIsCurveOwned`, `m_bIsCurveOwned` |
//! | [`CurveClassification::set_ok_to_set_tolerance`]       | `SetOKToSetTolerance`                            |
//! | [`CurveClassification::set_fillet_data`]               | `SetFilletData`                                  |
//! | [`CurveClassification::set_uv_curve1`], [`CurveClassification::set_uv_curve2`] | `SetUVCurve1`, `SetUVCurve2` (no plan unit: unreachable from the entry points) |
//! | [`CurveClassification::type_code`], [`CURVE_CLASSIFICATION_TYPE_STRING`], [`CurveClassification::is_kind_of`] | `GetType`, `GetTypeString`, `IsKindOf` |
//! | [`CurveClassification::classify_crv_loc`]              | `GetClassifyCrvLoc(const SmPointClassification*, double)` |
//! | [`CurveClassification::set_curve`]                     | `SetCurve`                                       |
//! | [`CurveClassification::set_intersect_data`]            | `SetIntersectData`                               |
//! | [`CurveClassification::set_is_curve_current`]          | `SetIsCurveCurrent`                              |
//! | [`CurveClassification::remove_at`]                     | `RemoveAt`                                       |
//! | [`CurveClassification::set_size`], [`MAX_INTERVALS`]   | `SetSize`                                        |
//! | [`CurveClassification::set_point_class_parameters`], [`CurveClassification::set_point_class_parameters_mut`]| `SetPointClassParameters`                        |
//! | [`CurveClassification::find_best_squeeze_end`]         | `FindBestSqueezeEnd`                             |
//! | [`CurveClassification::is_referenced`]                 | `IsReferenced`                                   |
//! | [`src_zone_tol_3d_of`]                                 | `SmTol::GetSrcZoneTol3d(const SmCurveClassification*)`, `SmTol::GetSrcZoneTol3d(const SmCurveInterval*)` |
//! | [`interval_obj_zone_tol_3d`]                           | `SmTol::GetObjZoneTol3d(const SmCurveInterval*)` |
//! | `ScaledZero for CurveInterval`                         | `SmTol::GetScaledZero(const SmCurveInterval&)`   |
//! | [`set_xsect_curve_through_point`], [`XSectThroughPoint`] | `sm_SetXSectCurveThroughPoint` (static)        |
//! | [`set_xsect_edge_through_point_in`]                    | `sm_SetXSectEdgeThroughPoint` (static)           |
//! | [`get_cci_surface_gaps`], [`CciSurfaceGaps`]           | `sm_GetCCISurfaceGaps` (static)                  |
//! | [`get_cci_geometry_in`], [`CciGeometryRefs`]           | `sm_GetCCIGeometry` (static), through an adapter |
//! | [`apply_point_solution`], [`ApplyPointSolutionInput`], [`ApplyPointSolution`] | `sm_ApplyPointSolution` (static) |
//! | [`apply_point_solution_in`], [`ApplyPointSolutionEdges`] | `sm_ApplyPointSolution` with `pEdge`/`pEMate` |
//! | [`make_consistent_classifications`]                    | `sm_MakeConsistentClassifications` (static)      |
//! | [`get_curve_curve_angle`]                              | `sm_GetCurveCurveAngle` (static)                 |
//! | [`cull_or_refine_cci_in_3d`]                           | `sm_CullOrRefineCCIIn3d` (static)                |
//! | [`compute_face_point_class`], [`compute_face_point_class_in`] | `sm_ComputeFacePointClass` (static)              |
//! | [`xsect_tol_2d_or_3d_for_classify_pair`]               | `sm_GetXSectTol2dOr3d_ForClassifyPair`           |
//! | [`object_dimension`]                                   | `sm_ObjectDimension` (static)                    |
//!
//! Deviations:
//! - **Object handles.** Every `SmObject` pointer the classification stores (curve,
//!   UV curves, surfaces, face, target loops, the other classification, the topology
//!   intersector) is a caller-chosen handle `O` ([`ClassObject`]), as in
//!   [`PointClassification`]; geometry behind a handle is reached through a
//!   [`ClassGeometry`]. `m_cpContext` is not stored (the kernel is passed explicitly).
//! - **No back pointer.** `SmCurveInterval::m_pCurveClassification` and
//!   `GetCurveClassification` are not ported: an interval lives inside its
//!   classification and is reached through it. The constructors take the owner (or
//!   `None`) only for its source zone tolerance (`SmTol::GetSrcZoneTol3d(cpOwner)`).
//! - **Storage.** `m_pData` / `m_lMaxSize` / `m_bIsBorrowed` become a `Vec` plus a flag
//!   recording whether upstream's array is allocated: growing from no array
//!   default-constructs the new intervals (their point classifications take the system
//!   zone tolerance, `SmTol::GetSrcZoneTol3d(NULL)`), growing an allocated array
//!   `ReSet`s them (`SM_UNINIT_TOL`). Borrowed arrays (`pOptBorrowedData`) are a
//!   memory-management feature of `ReSet` (unit `intersect.curve_class_p2`) and are not
//!   modelled. [`CurveClassification::set_size`] refuses more than [`MAX_INTERVALS`]
//!   intervals instead of allocating without bound.
//! - **Construction.** `with_curve_interval` initializes a single interval directly
//!   for boundary APIs; it does not co-port part 2's `ReSet` implementation.
//! - **Ownership of the curve.** `m_bIsCurveOwned` is kept as data; there is nothing to
//!   delete when a handle is replaced.
//! - **Undefined behaviour upstream** is an error here: `GetSubInterval`, `GetAt`,
//!   `operator[]`, `FindBestSqueezeEnd` past the size (upstream reads past `m_lSize`),
//!   `RemoveAt` on an empty classification (`m_lSize` wraps), a `NULL` curve where one is
//!   evaluated, an odd-length touch list in `GetWindingCount`, and missing surfaces or
//!   UV points where the static helpers dereference them. `RemoveAt` past the end of a
//!   non-empty classification drops the last interval, as upstream.
//! - **Out-params.** Status plus out-params become result structs; an out-param upstream
//!   leaves unwritten on some paths is an `Option` (`None` = not written) or is passed
//!   in and returned. `GetConnectedTouches` returns the connected touches (their count
//!   is the length). `sm_ApplyPointSolution` takes the classification as its curve
//!   (mutable: upstream edits it through a `const_cast`) and source zone tolerance.
//! - **`SmTemporaryChangeValue` on `GetOutOfBoundsEnabled`.** The surfaces a static
//!   helper evaluates out of bounds are copies with out-of-bounds evaluation enabled
//!   (B-spline surfaces and planes, the `SmBSplineSurface` kinds); the caller's
//!   surfaces are unchanged, as after upstream's restore.
//! - **Dead code.** `sm_ApplyPointSolution`'s `gwcUpdateCurves` / `gwcUpdateEdges` are
//!   constant `TRUE`, so its `FALSE` branches, and every `SM_DEBUG_CODE` block, are not
//!   ported; `SM_ASSERT*` only report.
//! - **Not ported here (other units).** `ReSet`, `InsertAt`, `operator=` and the other
//!   members of `SmCurveClassification` belong to units `intersect.curve_class_p2` ..
//!   `_p9`; `SmTouchData::operator==` (the derived `PartialEq`), `Homogenize`, `Dump`,
//!   `AssertValid` and `Draw` are not reachable from the in-scope entry points.
//! - **Callees in units not ported yet** return [`SolidError::Unported`]: every
//!   topology member (`SmEdge`, `SmVertex`, `SmFace`, `SmEdgeuse`), the surface point
//!   solves (`surface.surface_p4`, `surface.surface_p5`), and the 3D-curve branch of
//!   `SmTol::MapTo2d(.., SmCurve, SmSurface)` (`SmSurface::DropPoint`,
//!   `surface.surface_p5`; its UV-curve branch is co-ported as
//!   [`map_to_2d_on_curve`]).
//! - **Mutable topology.** `SetPointClassParameters` has a read-only form and one over
//!   topology the point solves may change
//!   ([`CurveClassification::set_point_class_parameters_mut`], through
//!   [`PointClassification::compute_point_parameters_mut`]): a face surface's point
//!   solve makes the face's trimmed surface cache there, as upstream.
//! - **Recursion.** None.

use std::borrow::Cow;

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::{Curve, CurveKind};
use crate::curve::curve::p3::{local_curve_intersect, PointSolveOptions};
use crate::error::{ignored, Result, SolidError, SM_ERR};
use crate::geom::assert_array::labels::CURVE_CLASSIFICATION_TYPE;
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{
    ang_tol_rad, eff_zero, eff_zero_param, xsect_tol3d, xsect_tol_3d_from_zones, zone_tol3d,
    zone_tol_3d, ScaledZero, SmallTopoType, UNINIT_TOL,
};
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassGeometry, ClassObject, ClassObjectKind, PointClassType,
    PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max2, Point2, Point3, Vec2, Vec3, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface_n_lib::p2::{EvaluateFlags, OrientType};
use crate::surface::surface::p1::map_to_2d_on_curve;
use crate::surface::surface::p2::surface_cache;
use crate::surface::surface::Surface;
use crate::surface::surface_point_solve::p2::with_out_of_bounds_enabled;
use crate::EntityRef;

const OP_CC: &str = "intersect::curve_class::p1::CurveClassification";
const OP_TOUCH: &str = "intersect::curve_class::p1::get_winding_count";
const OP_XSECT: &str = "intersect::curve_class::p1::set_xsect_curve_through_point";
const OP_APPLY: &str = "intersect::curve_class::p1::apply_point_solution";
const OP_GAPS: &str = "intersect::curve_class::p1::get_cci_surface_gaps";

/// The most intervals [`CurveClassification::set_size`] stores (far above anything a
/// classification reaches; upstream allocates whatever is asked for).
pub const MAX_INTERVALS: usize = 1 << 20;

/// `SM_MIN_ARRAY_SIZE` / `MIN_ARRAY_SIZE` of `SetSize`.
const MIN_ARRAY_SIZE: usize = 16;

/// `SmCurveClassification::GetTypeString`.
pub const CURVE_CLASSIFICATION_TYPE_STRING: &str = "SmCurveClassification_TYPE";

// ---- enums ------------------------------------------------------------------------------

/// Where a parameter falls relative to an interval (`SmIntervalPositionType`); the
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum IntervalPosition {
    /// `SM_IP_UNINIT`: not yet classified.
    #[default]
    Uninit = 0,
    /// `SM_IP_START`: on the start point.
    Start = 1,
    /// `SM_IP_INSIDE`: between the start and end points.
    Inside = 2,
    /// `SM_IP_END`: on the end point.
    End = 3,
    /// `SM_IP_OUTSIDE`: outside the interval.
    Outside = 4,
}

impl IntervalPosition {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// The direction of a surface vector at a surface point relative to a face
/// (`SmCrvDirOnFaceType`); the discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum CrvDirOnFaceType {
    /// `SM_CD_UNINIT`.
    #[default]
    Uninit = 0,
    /// `SM_CD_NONE`.
    None = 1,
    /// `SM_CD_IN`: points into the face.
    In = 2,
    /// `SM_CD_ON`: tangent to a face boundary.
    On = 3,
    /// `SM_CD_OUT`: points out of the face.
    Out = 4,
    /// `SM_CD_LAMINA`: out of the surface over a lamina boundary.
    Lamina = 5,
    /// `SM_CD_SEAM`: leaves a seam side bounded by an edge use.
    Seam = 6,
    /// `SM_CD_POLE`: out of the surface over a pole marked by a vertex.
    Pole = 7,
    /// `SM_CD_POLE_NO_VERTEX`: out of the surface over a pole without a vertex.
    PoleNoVertex = 8,
    /// `SM_CD_SEAM_NO_EDGEUSE`: leaves a seam side without a bounding edge use (a 3D
    /// edge may still exist on the opposite periodic side).
    SeamNoEdgeuse = 9,
}

impl CrvDirOnFaceType {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::Uninit,
            1 => Self::None,
            2 => Self::In,
            3 => Self::On,
            4 => Self::Out,
            5 => Self::Lamina,
            6 => Self::Seam,
            7 => Self::Pole,
            8 => Self::PoleNoVertex,
            9 => Self::SeamNoEdgeuse,
            _ => return None,
        })
    }
}

// ---- touch data -------------------------------------------------------------------------

/// How the classification curve arrives at or departs from a face boundary at one of
/// its point classifications (`SmTouchData`). Touch lists hold arrive/depart pairs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchData<O> {
    /// `m_pConnectedTo`: the topology the point classification connects to.
    pub connected_to: Option<O>,
    /// `m_eLoopDir`.
    pub loop_dir: CrvDirOnFaceType,
    /// `m_lLoopIndx`: the index of the loop of `connected_to`.
    pub loop_index: u64,
    /// `m_dParam`: the classification curve parameter.
    pub param: f64,
    /// `m_lPtClassifyIndx`: the index of the point classification.
    pub pt_classify_index: u64,
    /// `m_pPtClassify`: the point classification (a handle).
    pub pt_classify: Option<O>,
}

/// The touches of `touches` connected to `connected_to`, in order
/// (`SmTouchData::GetConnectedTouches`; upstream's count is the length).
#[must_use]
pub fn get_connected_touches<O: PartialEq + Copy>(
    connected_to: Option<O>,
    touches: &[TouchData<O>],
) -> Vec<TouchData<O>> {
    // for every touch
    touches
        .iter()
        .filter(|t| t.connected_to == connected_to)
        .copied()
        .collect()
}

/// `SM_ERR_ASSERT_FAILURE` (`SmMessages.h`).
const SM_ERR_ASSERT_FAILURE: i32 = 1005;

/// Filters a consumer's working touch list to transverse crossings
/// (`SmTouchData::RemoveNonCrossings`): the end directions are normalised (a pole or
/// lamina end is [`CrvDirOnFaceType::Out`]; an unused seam image,
/// [`CrvDirOnFaceType::SeamNoEdgeuse`], takes its inward neighbour's direction), each
/// `X/ON ... ON/Y` run is joined into `X/Y`, and every arrive/depart pair with the same
/// direction on both sides (same-side tangencies, struts, cracks, coincident touches)
/// is removed. The original encounter list is the caller's to keep.
///
/// # Errors
///
/// `SM_ERR_ASSERT_FAILURE` for an odd-length list (`AERN`).
pub fn remove_non_crossings<O: Copy>(touches: &mut Vec<TouchData<O>>) -> Result<()> {
    let mut count = touches.len();
    if count == 0 {
        return Ok(());
    }
    if count % 2 != 0 {
        return Err(SolidError::Upstream {
            operation: "intersect::curve_class::remove_non_crossings",
            code: SM_ERR_ASSERT_FAILURE,
            entity: None,
        });
    }
    let at = |touches: &[TouchData<O>], i: usize| -> Result<TouchData<O>> {
        touches.get(i).copied().ok_or_else(|| {
            SolidError::invalid_input(
                "intersect::curve_class::remove_non_crossings",
                "touch index out of range",
            )
        })
    };

    let put = |touches: &mut Vec<TouchData<O>>, i: usize, t: TouchData<O>| {
        if let Some(slot) = touches.get_mut(i) {
            *slot = t;
        }
    };

    // Normalize endpoint directions: poles/lamina are outside, while an unused
    // seam image matches its inward neighbor and does not create a crossing.
    for end in 0..2 {
        let index = if end == 0 { 0 } else { count - 1 };
        let neighbour = if end == 0 { 1 } else { count - 2 };
        let dir = at(touches, index)?.loop_dir;
        let new_dir = match dir {
            CrvDirOnFaceType::Pole | CrvDirOnFaceType::PoleNoVertex | CrvDirOnFaceType::Lamina => {
                CrvDirOnFaceType::Out
            }
            CrvDirOnFaceType::SeamNoEdgeuse => at(touches, neighbour)?.loop_dir,
            other => other,
        };
        if let Some(t) = touches.get_mut(index) {
            t.loop_dir = new_dir;
        }
    }

    // Join X/ON ... ON/Y into X/Y without counting the coincident interval.
    let mut write = 1;
    let mut read = 1;
    while read + 1 < count {
        let (a, b) = (at(touches, read)?, at(touches, read + 1)?);
        if a.loop_dir != CrvDirOnFaceType::On && b.loop_dir != CrvDirOnFaceType::On {
            put(touches, write, a);
            write += 1;
            put(touches, write, b);
            write += 1;
        }
        read += 2;
    }
    let last = at(touches, count - 1)?;
    put(touches, write, last);
    write += 1;
    count = write;
    write = 0;
    // Keep only arrival/departure changes; discard same-side tangencies,
    // struts, cracks, and entirely coincident touches.
    let mut read = 0;
    while read < count {
        let (a, b) = (at(touches, read)?, at(touches, read + 1)?);
        if a.loop_dir != b.loop_dir {
            put(touches, write, a);
            write += 1;
            put(touches, write, b);
            write += 1;
        }
        read += 2;
    }
    touches.truncate(write);
    Ok(())
}

/// The winding count of a single loop's touch list (`SmTouchData::GetWindingCount`):
/// the number of unmatched in/out and out/in arrive/depart pairs, and
/// [`OrientType::Opposite`] (`SM_OT_LOWERDOMAIN`) for more in/out pairs,
/// [`OrientType::Same`] (`SM_OT_UPPERDOMAIN`) for more out/in pairs, else
/// [`OrientType::Unknown`].
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a list of odd length (upstream reads past its end).
pub fn get_winding_count<O>(touches: &[TouchData<O>]) -> Result<(u64, OrientType)> {
    if touches.len() % 2 != 0 {
        return Err(SolidError::invalid_input(
            OP_TOUCH,
            "touch list of odd length",
        ));
    }

    // init locals and output
    let mut in_out_cnt: u64 = 0;
    let mut out_in_cnt: u64 = 0;

    // for every touch
    for pair in touches.chunks_exact(2) {
        let (Some(a), Some(d)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        if a.loop_dir == CrvDirOnFaceType::In && d.loop_dir == CrvDirOnFaceType::Out {
            in_out_cnt += 1;
        }
        if a.loop_dir == CrvDirOnFaceType::Out && d.loop_dir == CrvDirOnFaceType::In {
            out_in_cnt += 1;
        }
    }

    // all done - return WindingCount = unpaired In/Out to Out/In touches
    let orient = match in_out_cnt.cmp(&out_in_cnt) {
        core::cmp::Ordering::Greater => OrientType::Opposite,
        core::cmp::Ordering::Less => OrientType::Same,
        core::cmp::Ordering::Equal => OrientType::Unknown,
    };

    // smos_Labs(lInOutCnt - lOutInCnt) of the unsigned difference
    Ok((in_out_cnt.abs_diff(out_in_cnt), orient))
}

// ---- curve interval ---------------------------------------------------------------------

/// The classification of one homogeneous interval of the classification curve
/// (`SmCurveInterval`): its parameter range and the point classifications of its start,
/// interior and end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveInterval<O> {
    /// `m_vInterval`: the parameter interval (its ends may have been snapped, see the
    /// point classifications' pre-snap parameters).
    pub interval: Extent1d,
    /// `m_vStart`: the classification of the start point.
    pub start: PointClassification<O>,
    /// `m_vMid`: the classification of the interior.
    pub mid: PointClassification<O>,
    /// `m_vEnd`: the classification of the end point.
    pub end: PointClassification<O>,
}

impl<O: ClassObject> CurveInterval<O> {
    /// An interval with the "inside out" extent whose point classifications take the
    /// owner's source zone tolerance (`SmCurveInterval(cpContext, cpOwner)`;
    /// `SmTol::GetSrcZoneTol3d(cpOwner)`, the system zone tolerance without an owner).
    #[must_use]
    pub fn new(k: &Kernel, owner: Option<&CurveClassification<O>>) -> Self {
        Self::with_interval(k, &Extent1d::new(), owner)
    }

    /// An interval over `interval` (`SmCurveInterval(crInterval, cpContext, cpOwner)`).
    #[must_use]
    pub fn with_interval(
        k: &Kernel,
        interval: &Extent1d,
        owner: Option<&CurveClassification<O>>,
    ) -> Self {
        let tol = src_zone_tol_3d_of(k, owner);
        Self {
            interval: *interval,
            start: PointClassification::new(tol),
            mid: PointClassification::new(tol),
            end: PointClassification::new(tol),
        }
    }

    /// Resets the extent and the three point classifications (`ReSet`).
    pub fn reset(&mut self, k: &Kernel) {
        self.interval.init(); // m_cpContext value left alone by design
        self.start.reset(k, None);
        self.mid.reset(k, None);
        self.end.reset(k, None);
    }

    /// Sets the parameter interval (`SetInterval`).
    pub fn set_interval(&mut self, interval: &Extent1d) {
        self.interval = *interval;
    }

    /// Moves the start of the interval (`SetIntervalMin`, private upstream: use the
    /// classification's `EditIntervalMin`).
    ///
    /// # Errors
    ///
    /// Those of [`Extent1d::set_min_max`] (the interval would become negative); the
    /// interval is then unchanged.
    pub fn set_interval_min(&mut self, new_param: f64) -> Result<()> {
        let max = self.interval.max();
        self.interval.set_min_max(new_param, max)
    }

    /// Moves the end of the interval (`SetIntervalMax`, private upstream).
    ///
    /// # Errors
    ///
    /// As [`CurveInterval::set_interval_min`].
    pub fn set_interval_max(&mut self, new_param: f64) -> Result<()> {
        let min = self.interval.min();
        self.interval.set_min_max(min, new_param)
    }

    /// The parameter interval (`GetInterval`).
    #[must_use]
    pub const fn interval(&self) -> &Extent1d {
        &self.interval
    }

    /// The start parameter (`GetMin`).
    #[must_use]
    pub const fn min(&self) -> f64 {
        self.interval.min()
    }

    /// The end parameter (`GetMax`).
    #[must_use]
    pub const fn max(&self) -> f64 {
        self.interval.max()
    }
}

/// `SmTol::GetScaledZero(const SmCurveInterval&)`: the scaled zero of the interval.
impl<O> ScaledZero for CurveInterval<O> {
    fn scaled_zero(&self) -> f64 {
        self.interval.scaled_zero()
    }
}

// ---- curve classification ---------------------------------------------------------------

/// The classification of a curve against another object (`SmCurveClassification`):
/// the curve split into ascending homogeneous [`CurveInterval`]s spanning its interval.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
pub struct CurveClassification<O> {
    /// `m_bIsCurveOwned`.
    pub(super) is_curve_owned: bool,
    /// `m_cpCurve`: the curve being classified.
    pub(super) curve: Option<O>,
    /// `m_sSrcZoneTol3d`: the zone tolerance of the curve's source.
    pub(super) src_zone_tol_3d: f64,
    /// `m_cpUVCurve1`.
    pub(super) uv_curve1: Option<O>,
    /// `m_bIsCurveCurrent`.
    pub(super) is_curve_current: bool,
    /// `m_cpSurface1`.
    pub(super) surface1: Option<O>,
    /// `m_cpSurface2`.
    pub(super) surface2: Option<O>,
    /// `m_bDoingCrossHatch`.
    pub(super) doing_cross_hatch: bool,
    /// `m_bDoingFillets`.
    pub(super) doing_fillets: bool,
    /// `m_bOKToSetTolerance`.
    pub(super) ok_to_set_tolerance: bool,
    /// `m_bTgtLoopsOnly`.
    pub(super) tgt_loops_only: bool,
    /// `m_sTgtLoops`.
    pub(super) tgt_loops: Vec<O>,
    /// `m_cpOtherClassification`.
    pub(super) other_classification: Option<O>,
    /// `m_cpFace`.
    pub(super) face: Option<O>,
    /// `m_cpUVCurve2`.
    pub(super) uv_curve2: Option<O>,
    /// `m_cpTI`.
    pub(super) ti: Option<O>,
    /// `m_vInterval`: the whole interval classified.
    pub(super) interval: Extent1d,
    /// `m_pData[0 .. m_lSize]`.
    pub(super) data: Vec<CurveInterval<O>>,
    /// `m_pData != NULL`: whether upstream's interval array is allocated.
    pub(super) allocated: bool,
    /// `m_bIgnoreTangency`.
    pub(super) ignore_tangency: bool,
}

/// How `SetPointClassParameters` reaches the classifications' objects: read-only, or
/// with topology the point solves may change.
pub(super) trait PointParams<O> {
    type Geometry: ClassGeometry<O>;
    fn geometry(&self) -> &Self::Geometry;
    /// `pc.ComputePointParameters(point)`.
    fn compute(&mut self, k: &Kernel, pc: &mut PointClassification<O>, point: Point3)
        -> Result<()>;
}

/// Read-only access ([`CurveClassification::set_point_class_parameters`]).
pub(super) struct ReadOnlyParams<'g, G>(pub(super) &'g G);

impl<O: ClassObject, G: ClassGeometry<O>> PointParams<O> for ReadOnlyParams<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn compute(
        &mut self,
        k: &Kernel,
        pc: &mut PointClassification<O>,
        point: Point3,
    ) -> Result<()> {
        pc.compute_point_parameters(k, self.0, point)
    }
}

/// Mutable access ([`CurveClassification::set_point_class_parameters_mut`]).
pub(super) struct MutableParams<'g, G>(pub(super) &'g mut G);

impl<O: ClassObject, G: super::p5::ClassGeometryMut<O>> PointParams<O> for MutableParams<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn compute(
        &mut self,
        k: &Kernel,
        pc: &mut PointClassification<O>,
        point: Point3,
    ) -> Result<()> {
        pc.compute_point_parameters_mut(k, self.0, point)
    }
}

impl<O: ClassObject> CurveClassification<O> {
    /// An empty classification (`SmCurveClassification()`): no curve, no intervals,
    /// interval `[0, 1]`, source zone tolerance `SM_UNINIT_TOL`.
    #[must_use]
    pub fn new(k: &Kernel) -> Self {
        Self {
            is_curve_owned: false,
            curve: None,
            src_zone_tol_3d: zone_tol3d(k, UNINIT_TOL),
            uv_curve1: None,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            doing_cross_hatch: false,
            doing_fillets: false,
            ok_to_set_tolerance: true,
            tgt_loops_only: false,
            tgt_loops: Vec::new(),
            other_classification: None,
            face: None,
            uv_curve2: None,
            ti: None,
            interval: Extent1d::from_min_max(0.0, 1.0),
            data: Vec::new(),
            allocated: false,
            ignore_tangency: false,
        }
    }

    /// Creates a classification with one unclassified interval for a curve.
    /// This initializes the same interval and source-zone state required by the
    /// boundary intersection APIs without invoking the separately owned `ReSet`.
    #[must_use]
    pub fn with_curve_interval(k: &Kernel, curve: O, interval: Extent1d, source_zone: f64) -> Self {
        let mut result = Self::new(k);
        result.set_curve(Some(curve), false);
        result.set_src_zone_tol_3d(source_zone);
        result.interval = interval;
        result
            .data
            .push(CurveInterval::with_interval(k, &interval, Some(&result)));
        result.allocated = true;
        result
    }

    /// The same classification with every object handle (the curves, surfaces, face,
    /// target loops, other classification and the intervals' point classifications'
    /// objects) replaced by `f` of it: a change of handle type, no new state.
    ///
    /// # Errors
    ///
    /// Those of `f`.
    pub fn map_objects<P: ClassObject>(
        &self,
        f: &mut dyn FnMut(O) -> Result<P>,
    ) -> Result<CurveClassification<P>> {
        let mut data = Vec::with_capacity(self.data.len());
        for d in &self.data {
            data.push(CurveInterval {
                interval: d.interval,
                start: d.start.map_objects(&mut *f)?,
                mid: d.mid.map_objects(&mut *f)?,
                end: d.end.map_objects(&mut *f)?,
            });
        }
        let mut tgt_loops = Vec::with_capacity(self.tgt_loops.len());
        for l in &self.tgt_loops {
            tgt_loops.push(f(*l)?);
        }
        Ok(CurveClassification {
            is_curve_owned: self.is_curve_owned,
            curve: self.curve.map(&mut *f).transpose()?,
            src_zone_tol_3d: self.src_zone_tol_3d,
            uv_curve1: self.uv_curve1.map(&mut *f).transpose()?,
            is_curve_current: self.is_curve_current,
            surface1: self.surface1.map(&mut *f).transpose()?,
            surface2: self.surface2.map(&mut *f).transpose()?,
            doing_cross_hatch: self.doing_cross_hatch,
            doing_fillets: self.doing_fillets,
            ok_to_set_tolerance: self.ok_to_set_tolerance,
            tgt_loops_only: self.tgt_loops_only,
            tgt_loops,
            other_classification: self.other_classification.map(&mut *f).transpose()?,
            face: self.face.map(&mut *f).transpose()?,
            uv_curve2: self.uv_curve2.map(&mut *f).transpose()?,
            ti: self.ti.map(&mut *f).transpose()?,
            interval: self.interval,
            data,
            allocated: self.allocated,
            ignore_tangency: self.ignore_tangency,
        })
    }

    /// Sets the native cross-hatch tolerance mode (`SetDoingCrossHatch`).
    pub fn set_doing_cross_hatch(&mut self, enabled: bool) {
        self.doing_cross_hatch = enabled;
    }

    // ---- simple data access --------------------------------------------------------

    /// The number of intervals (`GetSize`; the end-point count is one more).
    #[must_use]
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// The intervals.
    #[must_use]
    pub fn intervals(&self) -> &[CurveInterval<O>] {
        &self.data
    }

    /// Sets the complete classification domain (inline `SetInterval`).
    ///
    /// # Errors
    /// Returns the interval validation error instead of ignoring an invalid domain.
    pub fn set_interval(&mut self, interval: &Extent1d) -> Result<()> {
        self.interval.set_min_max(interval.min(), interval.max())
    }

    /// The face classified against (`GetFace`).
    #[must_use]
    pub const fn face(&self) -> Option<O> {
        self.face
    }

    /// `GetSurface1`.
    #[must_use]
    pub const fn surface1(&self) -> Option<O> {
        self.surface1
    }

    /// `GetSurface2`.
    #[must_use]
    pub const fn surface2(&self) -> Option<O> {
        self.surface2
    }

    /// The curve being classified (`GetCurve`).
    #[must_use]
    pub const fn curve(&self) -> Option<O> {
        self.curve
    }

    /// The whole interval classified (`GetInterval`).
    #[must_use]
    pub const fn interval(&self) -> Extent1d {
        self.interval
    }

    /// The parameter interval of interval `index` (`GetSubInterval`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()` (upstream reads past the
    /// intervals).
    pub fn sub_interval(&self, index: usize) -> Result<Extent1d> {
        Ok(self.at(index)?.interval)
    }

    /// The surface-1 UV curve while the curve is current (`GetUVCurve1`).
    #[must_use]
    pub const fn uv_curve1(&self) -> Option<O> {
        if self.is_curve_current {
            self.uv_curve1
        } else {
            None
        }
    }

    /// The surface-2 UV curve while the curve is current (`GetUVCurve2`).
    #[must_use]
    pub const fn uv_curve2(&self) -> Option<O> {
        if self.is_curve_current {
            self.uv_curve2
        } else {
            None
        }
    }

    /// `GetAnyUVCurve1`.
    #[must_use]
    pub const fn any_uv_curve1(&self) -> Option<O> {
        self.uv_curve1
    }

    /// `GetAnyUVCurve2`.
    #[must_use]
    pub const fn any_uv_curve2(&self) -> Option<O> {
        self.uv_curve2
    }

    /// `GetTopologyIntersector`.
    #[must_use]
    pub const fn topology_intersector(&self) -> Option<O> {
        self.ti
    }

    /// `GetIsCurveCurrent`.
    #[must_use]
    pub const fn is_curve_current(&self) -> bool {
        self.is_curve_current
    }

    /// `GetIgnoreTangency`.
    #[must_use]
    pub const fn ignore_tangency(&self) -> bool {
        self.ignore_tangency
    }

    /// `GetDoingCrossHatch`.
    #[must_use]
    pub const fn doing_cross_hatch(&self) -> bool {
        self.doing_cross_hatch
    }

    /// `GetDoingFillets`.
    #[must_use]
    pub const fn doing_fillets(&self) -> bool {
        self.doing_fillets
    }

    /// `GetOtherClassification`.
    #[must_use]
    pub const fn other_classification(&self) -> Option<O> {
        self.other_classification
    }

    /// `GetOKToSetTolerance`.
    #[must_use]
    pub const fn ok_to_set_tolerance(&self) -> bool {
        self.ok_to_set_tolerance
    }

    /// `m_bIsCurveOwned` (no upstream getter).
    #[must_use]
    pub const fn is_curve_owned(&self) -> bool {
        self.is_curve_owned
    }

    /// `GetTgtLoopsOnly`.
    #[must_use]
    pub const fn tgt_loops_only(&self) -> bool {
        self.tgt_loops_only
    }

    /// `SetTgtLoopsOnly`.
    pub fn set_tgt_loops_only(&mut self, tgt_loops_only: bool) {
        self.tgt_loops_only = tgt_loops_only;
    }

    /// `GetTgtLoops`.
    #[must_use]
    pub fn tgt_loops(&self) -> &[O] {
        &self.tgt_loops
    }

    /// `AddTgtLoop`.
    pub fn add_tgt_loop(&mut self, tgt_loop: O) {
        self.tgt_loops.push(tgt_loop);
    }

    /// The source zone tolerance (`GetSrcZoneTol3d`; its `SM_ASSERT_TOL` only reports).
    #[must_use]
    pub const fn src_zone_tol_3d(&self) -> f64 {
        self.src_zone_tol_3d
    }

    /// Sets the source zone tolerance (`SetSrcZoneTol3d`, reached through
    /// `SmTol::SetZoneTol3d`).
    pub fn set_src_zone_tol_3d(&mut self, zone_tol_3d: f64) {
        self.src_zone_tol_3d = zone_tol_3d;
    }

    // ---- curve interval access -------------------------------------------------------

    /// Interval `index` (`GetAt`, `operator[]`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()`.
    pub fn at(&self, index: usize) -> Result<&CurveInterval<O>> {
        self.data.get(index).ok_or_else(bad_index)
    }

    /// Interval `index`, mutable (`GetAt`, `operator[]`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()`.
    pub fn at_mut(&mut self, index: usize) -> Result<&mut CurveInterval<O>> {
        self.data.get_mut(index).ok_or_else(bad_index)
    }

    // ---- sets ------------------------------------------------------------------------

    /// `SetSurface1`.
    pub fn set_surface1(&mut self, surface1: Option<O>) {
        self.surface1 = surface1;
    }

    /// `SetSurface2`.
    pub fn set_surface2(&mut self, surface2: Option<O>) {
        self.surface2 = surface2;
    }

    /// `SetFace`.
    pub fn set_face(&mut self, face: Option<O>) {
        self.face = face;
    }

    /// `SetIgnoreTangency`.
    pub fn set_ignore_tangency(&mut self, ignore_tangency: bool) {
        self.ignore_tangency = ignore_tangency;
    }

    /// `SetIsCurveOwned`.
    pub fn set_is_curve_owned(&mut self, is_curve_owned: bool) {
        self.is_curve_owned = is_curve_owned;
    }

    /// `SetOKToSetTolerance`.
    pub fn set_ok_to_set_tolerance(&mut self, ok_to_set_tolerance: bool) {
        self.ok_to_set_tolerance = ok_to_set_tolerance;
    }

    /// Sets the fillet data (`SetFilletData`; upstream default `bDoingFillets = TRUE`).
    pub fn set_fillet_data(&mut self, other_classification: Option<O>, doing_fillets: bool) {
        self.other_classification = other_classification;
        self.doing_fillets = doing_fillets;
    }

    /// Sets the surface-1 UV curve (`SetUVCurve1`; deleting the old one,
    /// `bDeleteOldUVCurve`, has no counterpart for a handle).
    pub fn set_uv_curve1(&mut self, uv_curve1: Option<O>) {
        self.uv_curve1 = uv_curve1;
    }

    /// Sets the surface-2 UV curve (`SetUVCurve2`).
    pub fn set_uv_curve2(&mut self, uv_curve2: Option<O>) {
        self.uv_curve2 = uv_curve2;
    }

    /// Sets the curve (`SetCurve`; upstream default `bIsCurveOwned = FALSE`); the curve
    /// becomes current.
    pub fn set_curve(&mut self, curve: Option<O>, is_curve_owned: bool) {
        // when needed - free old curve (nothing to free for a handle)

        // make assignment
        self.curve = curve;
        self.is_curve_owned = is_curve_owned;
        self.is_curve_current = true;
    }

    /// Sets the surf/surf intersection data (`SetIntersectData`; upstream defaults
    /// `bIsCurveCurrent = TRUE`, `cpTI = NULL`).
    pub fn set_intersect_data(
        &mut self,
        surface1: Option<O>,
        surface2: Option<O>,
        uv_curve2: Option<O>,
        is_curve_current: bool,
        ti: Option<O>,
    ) {
        self.surface1 = surface1;
        self.surface2 = surface2;
        self.is_curve_current = is_curve_current;
        self.uv_curve2 = uv_curve2;
        self.ti = ti;
    }

    /// Marks the curve current or not (`SetIsCurveCurrent`); a curve that is no longer
    /// current loses its UV curves.
    pub fn set_is_curve_current(&mut self, current: bool) {
        self.is_curve_current = current;
        if !current {
            self.uv_curve1 = None;
            self.uv_curve2 = None;
        }
    }

    // ---- type --------------------------------------------------------------------------

    /// `GetType`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        CURVE_CLASSIFICATION_TYPE
    }

    /// `IsKindOf`: its own type, then `SmObject`'s.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == CURVE_CLASSIFICATION_TYPE || object::is_kind_of(t)
    }

    // ---- editing -----------------------------------------------------------------------

    /// Removes interval `index` (`RemoveAt`). An index past the end of a non-empty
    /// classification drops the last interval, as upstream.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an empty classification (upstream's size
    /// wraps).
    pub fn remove_at(&mut self, index: usize) -> Result<()> {
        let size = self.data.len();
        if size == 0 {
            return Err(SolidError::invalid_input(
                OP_CC,
                "remove_at on an empty classification",
            ));
        }
        if index < size {
            self.data.remove(index);
        } else {
            // the shift loop does nothing; m_lSize -= 1
            self.data.truncate(size - 1);
        }
        Ok(())
    }

    /// Sets the number of intervals (`SetSize`). New intervals are default-constructed
    /// when no array is allocated yet, else reset; size 0 frees the array.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for more than [`MAX_INTERVALS`] intervals.
    pub fn set_size(&mut self, k: &Kernel, new_size: usize) -> Result<()> {
        if new_size > MAX_INTERVALS {
            return Err(SolidError::invalid_input(OP_CC, "too many intervals"));
        }

        // switch on NewSize
        if new_size == 0 {
            // shrink to nothing
            self.data.clear();
            self.allocated = false;
        } else if !self.allocated {
            // a new array of default-constructed intervals (no owner)
            let fresh = CurveInterval::new(k, None);
            self.data.clear();
            self.data.resize(new_size, fresh);
            self.allocated = true;
        } else if new_size > self.data.len() {
            // init any newly used SmCurveIntervals (within or beyond the allocation)
            let mut reset = CurveInterval::new(k, None);
            reset.reset(k);
            self.data.resize(new_size, reset);
        } else {
            self.data.truncate(new_size);
        }
        // (MIN_ARRAY_SIZE growth only sizes upstream's allocation)
        let _ = MIN_ARRAY_SIZE;
        Ok(())
    }

    // ---- queries ------------------------------------------------------------------------

    /// The location of a point classification with curve parameter `param`
    /// (`GetClassifyCrvLoc(const SmPointClassification*, double)`): its surf/surf/surf
    /// point when set, else the curve point.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the curve is needed but there is none or
    /// `geometry` cannot resolve it.
    pub fn classify_crv_loc(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        point_class: &PointClassification<O>,
        param: f64,
    ) -> Result<Point3> {
        // When pointClassification has a stored point
        if point_class.sss_point_flag() {
            return Ok(point_class.sss_point());
        }

        // compute curve point at pointClassification parameter (status ignored)
        let curve = self.curve_geometry(geometry)?;
        Ok(ignored(curve.evaluate_point(k, param))?.unwrap_or(Vec3::UNDEF))
    }

    /// Makes sure every start and end point classification (and every interior one
    /// with `set_mids`) has its object parameters set (`SetPointClassParameters`;
    /// upstream default `bSetMids = FALSE`): the first start and every end from the
    /// curve point at their parameter, every other start copied from the previous end.
    ///
    /// # Errors
    ///
    /// The last failure of the curve evaluations and of
    /// [`PointClassification::compute_point_parameters`] (processing continues past
    /// them, as upstream); [`SolidError::Unported`] at once; an error for a missing
    /// curve where one is evaluated.
    pub fn set_point_class_parameters(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        set_mids: bool,
    ) -> Result<()> {
        self.set_point_class_parameters_in(k, &mut ReadOnlyParams(geometry), set_mids)
    }

    /// [`CurveClassification::set_point_class_parameters`] over topology it may change:
    /// each classification's parameters are computed with
    /// [`PointClassification::compute_point_parameters_mut`] (a face surface's point
    /// solve may make the face's trimmed surface cache, as upstream).
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::set_point_class_parameters`].
    pub fn set_point_class_parameters_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl super::p5::ClassGeometryMut<O>,
        set_mids: bool,
    ) -> Result<()> {
        self.set_point_class_parameters_in(k, &mut MutableParams(geometry), set_mids)
    }

    fn set_point_class_parameters_in(
        &mut self,
        k: &Kernel,
        access: &mut impl PointParams<O>,
        set_mids: bool,
    ) -> Result<()> {
        let num_cis = self.data.len();
        let mut ret: Result<()> = Ok(());
        let mut pt = Vec3::UNDEF;

        // record a failing status, propagate Unported at once
        let mut record = |r: Result<()>| -> Result<()> {
            match r {
                Ok(()) => Ok(()),
                Err(e) if e.is_unported() => Err(e),
                Err(e) => {
                    ret = Err(e);
                    Ok(())
                }
            }
        };

        for ii in 0..num_cis {
            // Start:
            let start_set = self.at(ii)?.start.are_parameters_set();
            if !start_set {
                if ii == 0 {
                    let param = self.at(ii)?.interval.min();
                    match self
                        .curve_geometry(access.geometry())?
                        .evaluate_point(k, param)
                    {
                        Ok(p) => pt = p,
                        Err(e) => record(Err(e))?,
                    }
                    let r = access.compute(k, &mut self.at_mut(ii)?.start, pt);
                    record(r)?;
                } else {
                    let prev_end = self.at(ii - 1)?.end;
                    self.at_mut(ii)?.start = prev_end;
                }
            }

            // End:
            if !self.at(ii)?.end.are_parameters_set() {
                let param = self.at(ii)?.interval.max();
                match self
                    .curve_geometry(access.geometry())?
                    .evaluate_point(k, param)
                {
                    Ok(p) => pt = p,
                    Err(e) => record(Err(e))?,
                }
                let r = access.compute(k, &mut self.at_mut(ii)?.end, pt);
                record(r)?;
            }

            // Mid:
            if set_mids && !self.at(ii)?.mid.are_parameters_set() {
                let param = self.at(ii)?.interval.mid();
                match self
                    .curve_geometry(access.geometry())?
                    .evaluate_point(k, param)
                {
                    Ok(p) => pt = p,
                    Err(e) => record(Err(e))?,
                }
                let r = access.compute(k, &mut self.at_mut(ii)?.mid, pt);
                record(r)?;
            }
        } // End for each CurveInterval

        ret
    }

    /// Which end of interval `index` to squeeze out (`FindBestSqueezeEnd`): the end
    /// classified to the more general object kind (the first interval's start, or the
    /// last interval's end, then takes the other end's classification), else the end
    /// with the larger gap.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()`.
    pub fn find_best_squeeze_end(&mut self, index: usize) -> Result<IntervalPosition> {
        let last = self.data.len().saturating_sub(1);
        let ivl = self.at_mut(index)?;
        let start_class = ivl.start.point_class().code();
        let end_class = ivl.end.point_class().code();

        // when startPoint is more general than endPoint - squeeze startPoint
        if start_class < end_class {
            // When squeezing the first interval - set startPointClass = endPointClass
            if index == 0 {
                ivl.start = ivl.end;
            }
            return Ok(IntervalPosition::Start);
        }
        // when endPoint is more general than startPoint - squeeze endPoint
        else if start_class > end_class {
            // when squeezing the last interval - set endPointClass = startPointClass
            if index == last {
                ivl.end = ivl.start;
            }
            return Ok(IntervalPosition::End);
        }

        // arrive here when both endPoints classifiy to the same type of topology

        // squeeze the end with the greatest deviation
        Ok(if ivl.start.deviation() > ivl.end.deviation() {
            IntervalPosition::Start
        } else {
            IntervalPosition::End
        })
    }

    /// Whether `object` is the object of any point classification (`IsReferenced`);
    /// false for `None`.
    #[must_use]
    pub fn is_referenced(&self, object: Option<O>) -> bool {
        // return FALSE for NULL objects
        let Some(o) = object else {
            return false;
        };

        // for every interval
        self.data.iter().any(|ivl| {
            ivl.start.object() == Some(o)
                || ivl.mid.object() == Some(o)
                || ivl.end.object() == Some(o)
        })
    }

    /// The curve geometry, or an error for no curve.
    pub(super) fn curve_geometry<'g>(
        &self,
        geometry: &'g impl ClassGeometry<O>,
    ) -> Result<&'g Curve> {
        let c = self
            .curve
            .ok_or(SolidError::invalid_input(OP_CC, "no classification curve"))?;
        geometry
            .curve(c)
            .ok_or(SolidError::invalid_input(OP_CC, "unresolved curve handle"))
    }
}

fn bad_index() -> SolidError {
    SolidError::invalid_input(OP_CC, "interval index out of range")
}

// ---- SmTol overloads ----------------------------------------------------------------------

/// The source zone tolerance of a classification, the system zone tolerance for none
/// (`SmTol::GetSrcZoneTol3d(const SmCurveClassification*)`; also
/// `SmTol::GetSrcZoneTol3d(const SmCurveInterval*)` given the interval's owner).
#[must_use]
pub fn src_zone_tol_3d_of<O: ClassObject>(
    k: &Kernel,
    classification: Option<&CurveClassification<O>>,
) -> f64 {
    match classification {
        Some(c) => c.src_zone_tol_3d(),
        None => zone_tol_3d(k, SmallTopoType::NormalSize),
    }
}

/// The zone tolerance of an interval's interior object, the system zone tolerance for
/// no interval (`SmTol::GetObjZoneTol3d(const SmCurveInterval*)`).
///
/// # Errors
///
/// Those of [`PointClassification::obj_zone_tol_3d`].
pub fn interval_obj_zone_tol_3d<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    interval: Option<&CurveInterval<O>>,
) -> Result<f64> {
    match interval {
        Some(i) => i.mid.obj_zone_tol_3d(k, geometry),
        None => Ok(zone_tol_3d(k, SmallTopoType::NormalSize)),
    }
}

/// `SmTol::GetZoneTol3d(const SmSurface*)`: the tolerance of the surface's face, the
/// system zone tolerance for a surface without one.
fn surface_zone_tol_3d(k: &Kernel, surface: &Surface) -> Result<f64> {
    crate::surface::surface::p1::zone_tol_3d(k, surface)
}

// ---- static helpers -----------------------------------------------------------------------

/// `surface` evaluating out of bounds when it is an `SmBSplineSurface` kind
/// (`SmTemporaryChangeValue<SmBoolean>(GetOutOfBoundsEnabled(), TRUE)`): upstream changes
/// the flag on the surface itself, so this is an evaluation copy with the flag set
/// ([`with_out_of_bounds_enabled`]: knots exactly as they are, owner kept), not the copy
/// constructor, whose knot fix-up can move near-coincident knots; the surface itself
/// when nothing changes.
///
/// # Errors
///
/// Those of [`Surface::clone_for_evaluation`] (a geometry graph nested too deep).
pub(super) fn out_of_bounds(surface: &Surface) -> Result<Cow<'_, Surface>> {
    Ok(match with_out_of_bounds_enabled(surface)? {
        Some(copy) => Cow::Owned(copy),
        None => Cow::Borrowed(surface),
    })
}

/// `SmSurface::Evaluate(uv, 1, 1, TRUE, TRUE, TRUE, vals)` (status ignored upstream):
/// `vals[i][j]`, the upper half; `[1][1]` and the values of a failed evaluation stay
/// undefined.
fn surface_evaluate_11(k: &Kernel, surface: &Surface, uv: Vec2) -> Result<(bool, [[Vec3; 2]; 2])> {
    let mut vals = [[Vec3::UNDEF; 2]; 2];
    let r = surface.evaluate(
        k,
        uv,
        1,
        1,
        EvaluateFlags {
            only_upper_half: true,
            ..EvaluateFlags::default()
        },
    );
    let Some(d) = ignored(r)? else {
        return Ok((false, vals));
    };
    let at = |i: usize, j: usize| d.get(i).and_then(|r| r.get(j)).copied();
    for (i, j) in [(0, 0), (0, 1), (1, 0)] {
        if let (Some(v), Some(slot)) = (at(i, j), vals.get_mut(i).and_then(|r| r.get_mut(j))) {
            *slot = v;
        }
    }
    Ok((true, vals))
}

/// `SmCurve::Evaluate(t, 1, TRUE, pv)` (status ignored upstream): the point and
/// tangent, undefined for a failed evaluation.
fn curve_evaluate_1(k: &Kernel, curve: &Curve, t: f64) -> Result<[Vec3; 2]> {
    let mut pv = [Vec3::UNDEF; 2];
    if let Some(d) = ignored(curve.evaluate(k, t, 1, true, true))? {
        for (slot, v) in pv.iter_mut().zip(d) {
            *slot = v;
        }
    }
    Ok(pv)
}

/// The `SmBSplineCurve` part of a curve (`IsKindOf(SmBSplineCurve_TYPE)` and the cast).
fn bspline_part_mut(curve: &mut Curve) -> Option<&mut BSplineCurve> {
    match curve.kind_mut() {
        CurveKind::BSpline(c) => Some(c),
        CurveKind::Ellipse(c) => Some(c.base_mut()),
        CurveKind::Hyperbola(c) => Some(c.base_mut()),
        CurveKind::Parabola(c) => Some(c.base_mut()),
        CurveKind::Line(c) => Some(c.base_mut()),
        _ => None,
    }
}

/// The result of [`set_xsect_curve_through_point`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XSectThroughPoint {
    /// `SM_SUCCESS` (`true`) or `SM_ERR` (`false`; "the curve was not set up to be
    /// tweaked", not a failure the caller must stop on).
    pub success: bool,
    /// `bModifiedCurve`: the curves were edited to pass through the point.
    pub modified_curve: bool,
    /// `dParam` out: the curve parameter of the intersection point.
    pub param: f64,
    /// `dDeviation`: the distance from the point to the curve (`None` when upstream
    /// leaves it unwritten).
    pub deviation: Option<f64>,
    /// `bSet3DPoint`: `point_3d` was set from the first surface.
    pub set_3d_point: bool,
    /// `*p3DPoint` out: the intersection point (the caller's value unless set).
    pub point_3d: Point3,
}

/// Makes a surf/surf intersection curve (and its UV trim curves) pass through the
/// surf/surf/surf intersection point given by `uv_points` on `surfaces`
/// (`sm_SetXSectCurveThroughPoint`): the point is the first surface's point there;
/// it is projected onto the curve near `param`, and when that is farther than the
/// intersection tolerance and the surfaces' intersection tangent is defined and nearly
/// parallel to the curve, a B-spline curve gets a through point
/// ([`BSplineCurve::set_intersection_curve_through_point`]). `point_3d` is the caller's
/// value of the output point.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for fewer than two UV points with two surfaces
/// (upstream reads past them); [`SolidError::Unported`] for a surface owned by a face
/// (its zone tolerance).
pub fn set_xsect_curve_through_point(
    k: &Kernel,
    src_curve: &mut Curve,
    curve_zone_tol_3d: f64,
    uv_curves: &mut [BSplineCurve],
    surfaces: &[&Surface],
    uv_points: &[Vec2],
    param: f64,
    point_3d: Point3,
) -> Result<XSectThroughPoint> {
    // init outputs - curve not modified
    let mut out = XSectThroughPoint {
        success: false,
        modified_curve: false,
        param,
        deviation: None,
        set_3d_point: false,
        point_3d,
    };

    // check state
    let ok_inputs = uv_curves.len() <= surfaces.len()
        && surfaces.len() >= 2
        && surfaces.len() == uv_points.len();

    // 1st the surface evaluations and setting p3DPoint
    let (Some(s0), Some(s1)) = (surfaces.first(), surfaces.get(1)) else {
        return Ok(out);
    };
    let (Some(&uv0), Some(&uv1)) = (uv_points.first(), uv_points.get(1)) else {
        return Err(SolidError::invalid_input(
            OP_XSECT,
            "fewer UV points than surfaces",
        ));
    };

    // temporarily allow out of bound evaluations for surface[0], surface[1]
    let (orig0, orig1) = (*s0, *s1);
    let s0 = out_of_bounds(orig0)?;
    let s1 = out_of_bounds(orig1)?;

    // evaluate each surface at the given intersection parameter points
    let (ok0, mut surf_val0) = surface_evaluate_11(k, &s0, uv0)?;
    let (_, mut surf_val1) = surface_evaluate_11(k, &s1, uv1)?;

    // tolerances
    let xsect_obj_zone_tol_3d = max2(
        surface_zone_tol_3d(k, orig0)?,
        surface_zone_tol_3d(k, orig1)?,
    );
    let xsect_tol_3d = xsect_tol_3d_from_zones(k, curve_zone_tol_3d, xsect_obj_zone_tol_3d);
    // (sXSectTol3dSq only feeds a warning)
    let ang_bound_rad = 6.0 * ang_tol_rad();
    let scaled_zero = eff_zero();

    // let surf/surf/surf xsect point = pSurface[0] evaluation
    if ok0 {
        out.set_3d_point = true;
        out.point_3d = surf_val0[0][0];
    }

    // Abort when inputs are confused (after setting output variables).
    if !ok_inputs {
        return Ok(out);
    }

    // project p3DPoint onto pSrcCurve3d to get its param value
    let natural = src_curve.natural_interval();
    out.param = natural.clamp_value(out.param);
    let projection = ignored(src_curve.local_point_solve::<usize>(
        k,
        &natural,
        SolverOperation::Minimize,
        out.point_3d,
        &PointSolveOptions::default(),
        out.param,
    ))?;

    // when the point fails to project to the curve
    let Some((true, projection)) = projection else {
        return Ok(out);
    };

    // grab the projected point's curve param and curve/point deviation values
    out.param = projection.start.param(0)?;
    let deviation = projection.start.solution_value;
    out.deviation = Some(deviation);

    // Note: sXSectTol3d = XSectTol3d(CurveSrcZoneTol3d,Surface)
    if deviation < xsect_tol_3d {
        out.success = true;
        return Ok(out);
    }

    // evaluate the curve at the param value for pos and tangent information
    let src_crv_pt = curve_evaluate_1(k, src_curve, out.param)?;

    surf_val0[1][0] = surf_val0[1][0].unitize_or_z();
    surf_val0[0][1] = surf_val0[0][1].unitize_or_z();
    surf_val1[1][0] = surf_val1[1][0].unitize_or_z();
    surf_val1[0][1] = surf_val1[0][1].unitize_or_z();
    let norm1 = surf_val0[1][0].cross(surf_val0[0][1]);
    let norm2 = surf_val1[1][0].cross(surf_val1[0][1]);
    let mut xsect_tangent = norm1.cross(norm2);

    // skip curve modification when the throughPoint is close enough to a tangent
    // surf/surf intersection point that the tangent vector can't easily be computed
    if xsect_tangent.length_squared() <= 100.0 * scaled_zero {
        out.success = true;
        return Ok(out);
    }

    // ensure Tangent direction is same as curve's direction
    if xsect_tangent.dot(src_crv_pt[1]) < 0.0 {
        xsect_tangent = -xsect_tangent;
    }

    // Surf/Surf XSectLine Tang direction is not nearly parallel to pSrcCurve3d's tangent
    let tang_angle = xsect_tangent.angle_between(src_crv_pt[1]).unwrap_or(0.0);
    if tang_angle > ang_bound_rad {
        out.success = true;
        return Ok(out);
    }

    // unitize the tangent vector
    xsect_tangent = xsect_tangent.unitize_or_z();

    // insert throughPoint=p3DPoint (tangent=sXSectTangent3d) at dParam into pSrcCurve3d
    if let Some(bs) = bspline_part_mut(src_curve) {
        let uv_opts: Vec<Option<Vec2>> = uv_points.iter().copied().map(Some).collect();
        // the surfaces as evaluated here (upstream passes the surfaces themselves, the
        // first two with out-of-bounds evaluation on): evaluation copies, exact knots
        let evaluators: Vec<Surface> = surfaces
            .iter()
            .enumerate()
            .map(|(i, s)| match i {
                0 => s0.as_ref().clone_for_evaluation(),
                1 => s1.as_ref().clone_for_evaluation(),
                _ => s.clone_for_evaluation(),
            })
            .collect::<Result<_>>()?;
        let mut new_param = out.param;
        let r = bs.set_intersection_curve_through_point(
            k,
            &mut new_param,
            out.point_3d,
            Some(xsect_tangent),
            uv_curves,
            &evaluators,
            &uv_opts,
            0.001,
        );
        out.param = new_param;
        // When Successful - reset dDeviation
        match ignored(r)? {
            Some(modified) => {
                out.modified_curve = modified;
                out.deviation = Some(0.0);
            }
            None => out.modified_curve = false,
        }
    }

    // all done - succesfully
    out.success = true;
    Ok(out)
}

/// `sm_SetXSectEdgeThroughPoint`: [`set_xsect_curve_through_point`] for edge `e` of the
/// B-rep `b` and all of its UV trim curves: `through_point` is projected onto the edge curve
/// over the edge interval near `param`; when the two surfaces at `uv_points` meet
/// there within the intersection tolerance and their intersection tangent is defined
/// and within six angular tolerances of the curve tangent, a B-spline edge curve
/// farther than a two-thousandth of the edge tolerance gets a through point
/// ([`BSplineCurve::set_intersection_curve_through_point`], minimum span `1e-6`, no UV
/// curves: upstream resets those arrays before the call). An edge curve and interval
/// that were common get the curve's knots rescaled back to the edge interval, and the
/// edge uses' (now obsolete) UV trim curves are deleted. `surfaces` must not borrow
/// from `b` (copy face surfaces first).
///
/// The result's `success` is `false` for upstream's `SM_ERR` ("not set up to be
/// tweaked"); `point_3d` is `through_point` and `set_3d_point` is `false`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge without curve (upstream dereferences it);
/// those of the edge reads and [`SolidError::Unported`] from the callees.
pub fn set_xsect_edge_through_point_in(
    k: &Kernel,
    b: &mut crate::topology::Brep,
    e: crate::topology::EdgeId,
    through_point: Point3,
    surfaces: &[&Surface],
    uv_points: &[Vec2],
    param: f64,
) -> Result<XSectThroughPoint> {
    use crate::topology::edge;
    use crate::topology::edgeuse::p1::{set_uv_trim_curve, UvTrimCurveArg};
    const OP: &str = "intersect::curve_class::p1::set_xsect_edge_through_point_in";

    // init outputs - curve not modified
    let mut out = XSectThroughPoint {
        success: false,
        modified_curve: false,
        param,
        deviation: None,
        set_3d_point: false,
        point_3d: through_point,
    };

    // locals
    let curve_zone_tol_3d = edge::tolerance(b, e)?;
    // pEdge->GetFaces(aFace): only feeds the UV curve arrays upstream resets below
    let _faces = edge::faces(b, e, None)?;
    let mut edge_interval = edge::interval(b, e)?;
    let mut curve_interval = edge::curve(b, e)?
        .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?
        .natural_interval();
    let common_curve_edge_interval =
        edge_interval.min() == curve_interval.min() && edge_interval.max() == curve_interval.max();

    // check state - given or found valid pointers to edge, curve, points, and surfaces
    if surfaces.len() < 2 || uv_points.len() != surfaces.len() {
        return Ok(out);
    }
    let (Some(&orig0), Some(&orig1), Some(&uv0), Some(&uv1)) = (
        surfaces.first(),
        surfaces.get(1),
        uv_points.first(),
        uv_points.get(1),
    ) else {
        return Ok(out);
    };

    // project pThroughPoint onto pCurve (status ignored; no answer is SM_ERR)
    let projection = {
        let src_curve = edge::curve(b, e)?
            .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
        ignored(src_curve.local_point_solve::<usize>(
            k,
            &edge_interval,
            SolverOperation::Minimize,
            through_point,
            &PointSolveOptions::default(),
            out.param,
        ))?
    };
    let Some((true, projection)) = projection else {
        return Ok(out);
    };

    // grab the projected point's param and deviaton values
    out.param = projection.start.param(0)?;
    let solution_value = projection.start.solution_value;
    out.deviation = Some(solution_value);

    // evaluate the curve at the param value for pos and tangent information
    let src_crv_pt = {
        let src_curve = edge::curve(b, e)?
            .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
        curve_evaluate_1(k, src_curve, out.param)?
    };

    // temporarily allow out of bound evaluations for surface[0], surface[1]
    let s0 = out_of_bounds(orig0)?;
    let s1 = out_of_bounds(orig1)?;
    let (_, mut surf_val0) = surface_evaluate_11(k, &s0, uv0)?;
    let (_, mut surf_val1) = surface_evaluate_11(k, &s1, uv1)?;

    // tolerances
    let xsect_tol_3d = xsect_tol_3d_from_zones(
        k,
        curve_zone_tol_3d,
        max2(
            surface_zone_tol_3d(k, orig0)?,
            surface_zone_tol_3d(k, orig1)?,
        ),
    );
    let xsect_tol_3d_sq = xsect_tol_3d * xsect_tol_3d;
    let scaled_zero = eff_zero();
    let ang_bound_rad = 6.0 * ang_tol_rad();

    // check state - surface xsect projections are near one another
    if through_point.distance_squared(surf_val0[0][0]) > xsect_tol_3d_sq
        || through_point.distance_squared(surf_val1[0][0]) > xsect_tol_3d_sq
        || surf_val0[0][0].distance_squared(surf_val1[0][0]) > xsect_tol_3d_sq
    {
        return Ok(out);
    }

    // set through point tangent = cross(surf1Normal,surf2Normal)
    surf_val0[1][0] = surf_val0[1][0].unitize_or_z();
    surf_val0[0][1] = surf_val0[0][1].unitize_or_z();
    surf_val1[1][0] = surf_val1[1][0].unitize_or_z();
    surf_val1[0][1] = surf_val1[0][1].unitize_or_z();
    let norm1 = surf_val0[1][0].cross(surf_val0[0][1]);
    let norm2 = surf_val1[1][0].cross(surf_val1[0][1]);
    let mut xsect_tangent = norm1.cross(norm2);

    if xsect_tangent.length_squared() <= scaled_zero {
        // the surfaces are tangent at this point: Let tangent = CurrentCurveTangent
        xsect_tangent = src_crv_pt[1];
    }

    // ensure Tangent direction is same as curve's direction
    if xsect_tangent.dot(src_crv_pt[1]) < 0.0 {
        xsect_tangent = -xsect_tangent;
    }

    // skip curve modification when the throughPoint is close enough to a tangent
    // surf/surf intersection point that the tangent vector can't easily be computed
    let tang_angle = xsect_tangent.angle_between(src_crv_pt[1]).unwrap_or(0.0);
    if xsect_tangent.length_squared() <= scaled_zero || tang_angle > ang_bound_rad {
        out.success = true;
        return Ok(out);
    }

    // unitize the tangent vector
    xsect_tangent = xsect_tangent.unitize_or_z();

    // force the 3DCurve to interpolate the point
    let is_bspline = {
        let src_curve = b
            .edge_mut(e)?
            .curve
            .as_mut()
            .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
        bspline_part_mut(src_curve).is_some()
    };
    if is_bspline && solution_value > curve_zone_tol_3d / 2000.0 {
        // GWC:EXPERIMENT - the UV trim curve, surface and UV point arrays are reset
        // before the call: only the 3D curve is edited
        let minimum_span_percent = 1.0e-6;
        let mut new_param = out.param;
        let r = {
            let src_curve = b
                .edge_mut(e)?
                .curve
                .as_mut()
                .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
            let Some(bs) = bspline_part_mut(src_curve) else {
                return Err(SolidError::invalid_input(
                    OP,
                    "edge curve is not a B-spline",
                ));
            };
            bs.set_intersection_curve_through_point(
                k,
                &mut new_param,
                through_point,
                Some(xsect_tangent),
                &mut [],
                &[] as &[Surface],
                &[],
                minimum_span_percent,
            )
        };
        out.param = new_param;

        // If the curve was not changed, then we're done.
        let modified = match r {
            Ok(m) => m,
            Err(err) if err.is_unported() => return Err(err),
            Err(_) => false,
        };
        out.modified_curve = modified;
        if !modified {
            out.success = true;
            return Ok(out);
        }

        // restore common curve/edge interval if its stopped being common by scaling
        // the curve's parameterization
        edge_interval = edge::interval(b, e)?;
        curve_interval = edge::curve(b, e)?
            .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?
            .natural_interval();
        let (e_min, e_max) = (edge_interval.min(), edge_interval.max());
        let (c_min, c_max) = (curve_interval.min(), curve_interval.max());
        if common_curve_edge_interval
            && ((e_min - c_min).abs() > scaled_zero || (e_max - c_max).abs() > scaled_zero)
        {
            // change the parameter values of every pSrcCurve3d knot (the UV trim
            // curve array is empty)
            let src_curve = b
                .edge_mut(e)?
                .curve
                .as_mut()
                .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
            if let Some(bs) = bspline_part_mut(src_curve) {
                bs.scale_knot_vector(e_min, e_max)?;
            }
        }

        // reset the deviation value
        out.deviation = Some(0.0);

        // GWC:CHANGE_SSSUVTRIMCURVE - remove obsolete UVTrimCurves from edge->Edgeuses
        for eu in edge::edgeuses(b, e, None, None)? {
            ignored(set_uv_trim_curve(
                b,
                eu,
                UvTrimCurveArg::Null,
                0.0,
                true,
                None,
            ))?;
        }

        // inform the public that the curves were updated
        out.modified_curve = true;
    }

    // all done - successfully
    out.success = true;
    Ok(out)
}

/// The result of [`get_cci_surface_gaps`].
#[derive(Debug, Clone, PartialEq)]
pub struct CciSurfaceGaps {
    /// `SM_SUCCESS` (`true`), or `SM_ERR` when a point does not drop to a surface (the
    /// outputs are then partial).
    pub success: bool,
    /// `dCurveEdgeGap`: the curve point / edge point distance.
    pub curve_edge_gap: f64,
    /// `dCurveSurfaceGap`: the largest curve point / curve surface gap.
    pub curve_surface_gap: f64,
    /// `dEdgeSurfaceGap`: the largest edge point / edge surface gap.
    pub edge_surface_gap: f64,
    /// `rUVPoints`: the UV point of each drop.
    pub uv_points: Vec<Point2>,
}

/// The gaps between a classification-curve / edge-curve intersection and its surfaces
/// (`sm_GetCCISurfaceGaps`): the first two surfaces are the curve's, the rest the
/// edge's; a surface with a UV trim curve is dropped onto locally from the trim curve
/// point at the solution parameter, the others globally.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for fewer UV curve entries than surfaces;
/// [`SolidError::Unported`] for any surface: the surface point solves (units
/// `surface.surface_p5`, `surface.surface_p4`).
pub fn get_cci_surface_gaps<S: Copy>(
    k: &Kernel,
    class_curve_point: Point3,
    edge_curve_point: Point3,
    solution: &Solution<S>,
    surfaces: &[&Surface],
    uv_curves: &[Option<&Curve>],
    edge_surface_count: usize,
) -> Result<CciSurfaceGaps> {
    // NotUsed
    let _ = edge_surface_count;
    // init output
    let mut out = CciSurfaceGaps {
        success: false,
        curve_edge_gap: class_curve_point.distance(edge_curve_point),
        curve_surface_gap: 0.0,
        edge_surface_gap: 0.0,
        uv_points: Vec::new(),
    };

    // for every surface
    for (ii, surface) in surfaces.iter().enumerate() {
        // get target Point/Surface combination
        let uv_curve = *uv_curves.get(ii).ok_or(SolidError::invalid_input(
            OP_GAPS,
            "fewer UV curves than surfaces",
        ))?;
        let point = if ii < 2 {
            class_curve_point
        } else {
            edge_curve_point
        };

        // When posible use cheap localSolve else use expensive Global Solve
        let sol: Option<(f64, Point2)> = if let Some(uv_curve) = uv_curve {
            // get UVGuessPoint
            let t_param = if ii < 2 {
                solution.start.param(0)?
            } else {
                solution.start.param(1)?
            };
            let guess = ignored(uv_curve.evaluate_point(k, t_param))?.unwrap_or(Vec3::UNDEF);
            surface_local_point_solve(k, surface, point, Point2::new(guess.x, guess.y))?
        } else {
            // get classCurvePoint/surf dist without checking trim boundaries
            let _cache = surface_cache::<()>(k, surface, None)?;
            let size = surface_approximate_size(k, surface)?;
            match surface_global_point_solve(k, surface, size, point)? {
                Some(s) => Some(s),
                None => return Ok(out),
            }
        };

        // accumulate UVPoints and distances
        let Some((gap, uv)) = sol else {
            // Point failed to drop to connected surface
            return Ok(out);
        };
        out.uv_points.push(uv);

        // save max CurvePoint/Surface and EdgePoint/Surface gaps
        if ii < 2 {
            if gap > out.curve_surface_gap {
                out.curve_surface_gap = gap;
            }
        } else if gap > out.edge_surface_gap {
            out.edge_surface_gap = gap;
        }
    }

    // all done
    out.success = true;
    Ok(out)
}

/// The result of [`get_cci_geometry_in`] (`sm_GetCCIGeometry`), over the geometry the
/// handles resolve to: the classification's
/// surfaces and UV curves through the adapter, the edge's (and its Boolean mate's)
/// faces, edge uses and UV trim curves through [`ClassGeometry::topology_entity`].
/// Surfaces are told apart by identity, as upstream compares `SmSurface*`.
#[derive(Debug, Clone)]
pub struct CciGeometryRefs<'g, O> {
    /// `pEMate`: the edge's Boolean mate.
    pub emate: Option<O>,
    /// `rSurfaces`: curve surfaces, edge surfaces, the common surface again, mate
    /// surfaces.
    pub surfaces: Vec<&'g Surface>,
    /// `rEdgeuses`: the edge use of each surface (`None` for the curve's), in the
    /// B-rep of the edge (or of the mate, for its surfaces).
    pub edgeuses: Vec<Option<crate::topology::EdgeuseId>>,
    /// `rUVCurves`: the UV trim curve of each surface.
    pub uv_curves: Vec<Option<&'g Curve>>,
    /// `lEdgeSurfaceCount`.
    pub edge_surface_count: usize,
}

/// `sm_GetCCIGeometry` with an adapter: the surfaces, edge uses and UV trim curves at a
/// classification-curve / `edge` intersection, in upstream's order (curve surfaces,
/// edge surfaces, the surface the edge shares with the curve once more, the Boolean
/// mate's surfaces).
///
/// # Errors
///
/// [`SolidError::Unported`] when `geometry` cannot resolve the edge (or its mate) to a
/// B-rep, or the classification has a topology intersector whose mate lookup the
/// adapter does not answer ([`ClassGeometry::topology_intersector_mate`]);
/// [`SolidError::InvalidInput`] for a surface or UV curve handle that does not resolve
/// and for a face without a surface (upstream adds the `NULL` surface and dereferences
/// it later).
pub fn get_cci_geometry_in<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    classification: &CurveClassification<O>,
    edge: O,
) -> Result<CciGeometryRefs<'g, O>> {
    const OP: &str = "intersect::curve_class::p1::get_cci_geometry_in";
    let mut out = CciGeometryRefs {
        emate: None,
        surfaces: Vec::new(),
        edgeuses: Vec::new(),
        uv_curves: Vec::new(),
        edge_surface_count: 0,
    };

    // locals
    let resolve_surface = |h: Option<O>| -> Result<Option<&'g Surface>> {
        h.map(|h| {
            geometry
                .surface(h)
                .ok_or(SolidError::invalid_input(OP, "unresolved surface handle"))
        })
        .transpose()
    };
    let resolve_curve = |h: Option<O>| -> Result<Option<&'g Curve>> {
        h.map(|h| {
            geometry
                .curve(h)
                .ok_or(SolidError::invalid_input(OP, "unresolved UV curve handle"))
        })
        .transpose()
    };
    let surface1 = resolve_surface(classification.surface1())?;
    let surface2 = resolve_surface(classification.surface2())?;
    let mut common_surf: Option<usize> = None;

    // Part 1 - gather Surfaces, edgeuses, and UVTrimCurves connected to intersection Point

    // gather the ClassCurve Surfaces and UVTrimCurves
    if let Some(s) = surface1 {
        out.surfaces.push(s);
        out.edgeuses.push(None);
        out.uv_curves
            .push(resolve_curve(classification.uv_curve1())?);
    }
    if let Some(s) = surface2 {
        out.surfaces.push(s);
        out.edgeuses.push(None);
        out.uv_curves
            .push(resolve_curve(classification.uv_curve2())?);
    }

    // get Faces connected to Edge and Edge->BooleanMate
    let (eb, e) = cci_edge_of(geometry, edge)?;
    let mut faces: Vec<(
        &'g crate::topology::Brep,
        crate::topology::EdgeId,
        crate::topology::FaceId,
    )> = crate::topology::edge::faces(eb, e, None)?
        .into_iter()
        .map(|f| (eb, e, f))
        .collect();
    let edge_face_count = faces.len();

    // When curve classification is being done as part of a boolean operation
    if let Some(ti) = classification.topology_intersector() {
        // look for edge->BooleanMate (GetOtherMate, else GetBrepMate)
        out.emate = geometry.topology_intersector_mate(ti, edge).ok_or(
            // UNPORTED(boolean.topology_intersector_p1): SmTopologyIntersector::GetOtherMate
            // (an adapter that cannot reach the intersector behind its handle)
            // Proof (unreachable): only if topology_intersector() is Some and the adapter
            // returns no mate; the library ClassGeometry impls resolve it; 0 hits in the
            // green difftest sweep.
            SolidError::unported(
                "SmTopologyIntersector::GetOtherMate",
                "boolean.topology_intersector_p1",
            ),
        )?;

        // when pEdge has a boolean operation temporary mate
        if let Some(emate) = out.emate {
            // add in Faces and Edgeuses connected to Edge->BooleanMate
            let (mb, me) = cci_edge_of(geometry, emate)?;
            faces.extend(
                crate::topology::edge::faces(mb, me, None)?
                    .into_iter()
                    .map(|f| (mb, me, f)),
            );
        }
    }

    // gather unique surfaces and edgeuses connected to this intersection point
    for (ii, &(fb, fe, f)) in faces.iter().enumerate() {
        let surf = crate::topology::face::p2::surface(fb, f)?
            .ok_or(SolidError::invalid_input(OP, "face without a surface"))?;

        // when pSurf is not in sSurfaces list - add it
        if !out.surfaces.iter().any(|s| core::ptr::eq(*s, surf)) {
            // (pEdge->GetEdgeuseOfFace for the edge's faces, pEMate's for the mate's)
            let edgeuse = crate::topology::edge::edgeuse_of_face(fb, fe, Some(f))?;
            if let Some(eu) = edgeuse {
                out.surfaces.push(surf);
                out.edgeuses.push(Some(eu));

                // don't use pEMate UVTrimCurves (they have a different parameterization)
                out.uv_curves.push(if ii < edge_face_count {
                    crate::topology::edgeuse::p1::uv_trim_curve(fb, eu)?.0
                } else {
                    None
                });
            }

            if ii < edge_face_count {
                out.edge_surface_count += 1;
            }
        }

        // remember an edge->Surface which is shared with the curve (nonManifold may have 2)
        if surface1.is_some_and(|s| core::ptr::eq(s, surf))
            || surface2.is_some_and(|s| core::ptr::eq(s, surf))
        {
            common_surf = Some(ii);
        }

        // when we are done with edgeSurfaces
        if edge_face_count.checked_sub(1) == Some(ii) {
            if let Some(common) = common_surf.filter(|&c| c < edge_face_count) {
                // add the commonSurf a 2nd time
                let &(cb, ce, cf) = faces
                    .get(common)
                    .ok_or(SolidError::invalid_input(OP, "common face index"))?;
                let surf = crate::topology::face::p2::surface(cb, cf)?
                    .ok_or(SolidError::invalid_input(OP, "face without a surface"))?;
                if let Some(eu) = crate::topology::edge::edgeuse_of_face(cb, ce, Some(cf))? {
                    out.surfaces.push(surf);
                    out.edgeuses.push(Some(eu));

                    // don't use pEMate UVTrimCurves (they have a different parameterization)
                    out.uv_curves
                        .push(crate::topology::edgeuse::p1::uv_trim_curve(cb, eu)?.0);
                    out.edge_surface_count += 1;
                }
            }
        }
    } // end iter every face attached to this edge

    // all done (upstream only asserts that the edge shares a surface with the curve)
    Ok(out)
}

/// The B-rep edge behind edge handle `edge` ([`ClassGeometry::topology_entity`]); an
/// adapter that cannot resolve it keeps the `SmEdge::GetFaces` deferral.
fn cci_edge_of<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    edge: O,
) -> Result<(&crate::topology::Brep, crate::topology::EdgeId)> {
    match geometry.topology_entity(edge) {
        Some((b, EntityRef::Edge(e))) => Ok((b, e)),
        Some(_) => Err(SolidError::invalid_input(
            "intersect::curve_class::p1::get_cci_geometry_in",
            "handle is not an edge",
        )),
        // UNPORTED(topology.edge): SmEdge::GetFaces (an adapter that cannot resolve the
        // edge handle to its B-rep)
        // Proof (unreachable): the library ClassGeometry impls resolve the edge handle
        // (topology_entity/_mut); 0 hits in the green difftest sweep.
        None => Err(SolidError::unported("SmEdge::GetFaces", "topology.edge")),
    }
}

/// The inputs of [`apply_point_solution`] other than the curves and the solution.
#[derive(Debug, Clone, Copy)]
pub struct ApplyPointSolutionInput<'a, O> {
    /// `SmTol::GetSrcZoneTol3d(pCurveClassification)` (see [`src_zone_tol_3d_of`]).
    pub src_zone_tol_3d: f64,
    /// `pEdge`: the edge, `None` to leave the edge curve alone.
    pub edge: Option<O>,
    /// `pEMate`: the edge's Boolean mate.
    pub emate: Option<O>,
    /// `pClassCurveSurface1`, `pClassCurveUVPoint1`.
    pub class_curve_surface1: Option<(&'a Surface, Point2)>,
    /// `pClassCurveSurface2`, `pClassCurveUVPoint2`.
    pub class_curve_surface2: Option<(&'a Surface, Point2)>,
    /// `pEdgeCurveSurface1`, `pEdgeCurveUVPoint1`.
    pub edge_curve_surface1: Option<(&'a Surface, Point2)>,
    /// `pEdgeCurveSurface2`, `pEdgeCurveUVPoint2`.
    pub edge_curve_surface2: Option<(&'a Surface, Point2)>,
}

/// The result of [`apply_point_solution`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean out-params
pub struct ApplyPointSolution {
    /// `SM_SUCCESS` (`true`) or `SM_ERR` (no surface point could be evaluated).
    pub success: bool,
    /// `rSSSPoint` out: the surf/surf/surf point (the caller's value unless set).
    pub sss_point: Point3,
    /// `bSSSPointFlag`: use the surf/surf/surf point.
    pub sss_point_flag: bool,
    /// `bModifiedCurve`: the classification curve was modified.
    pub modified_curve: bool,
    /// `bModifiedEdgeCurve`.
    pub modified_edge_curve: bool,
    /// `bModifiedMateCurve`.
    pub modified_mate_curve: bool,
    /// `dDeviation`: the largest curve / surf/surf/surf point distance.
    pub deviation: f64,
}

/// Moves a classification-curve / edge intersection onto the surf/surf/surf point of
/// its surfaces (`sm_ApplyPointSolution`): the point is the first available surface
/// point; the classification curve (when given; it needs both its surfaces) is made to
/// pass through it ([`set_xsect_curve_through_point`]) and the solution's first
/// parameter moves to it. `sss_point` is the caller's value of the output point.
///
/// # Errors
///
/// [`SolidError::Unported`] with an edge (`SmEdge::GetCurve`, unit `topology.edge`);
/// [`SolidError::InvalidInput`] for a classification curve without both of its
/// surfaces (upstream dereferences the missing one); those of
/// [`set_xsect_curve_through_point`].
pub fn apply_point_solution<O, S: Copy>(
    k: &Kernel,
    src_curve: Option<&mut Curve>,
    input: &ApplyPointSolutionInput<'_, O>,
    solution: &mut Solution<S>,
    sss_point: Point3,
) -> Result<ApplyPointSolution> {
    if input.edge.is_some() {
        // this entry point has no B-rep for the edge ([`apply_point_solution_in`] has)
        // UNPORTED(topology.edge): SmEdge::GetCurve
        // Proof (unreachable): B-rep-less entry point; library callers use the `_in` twin,
        // only difftest and a unit test call this one; 0 hits in the green difftest sweep.
        return Err(SolidError::unported("SmEdge::GetCurve", "topology.edge"));
    }
    apply_point_solution_impl(
        k,
        src_curve,
        input,
        input.emate.is_some(),
        None,
        solution,
        sss_point,
    )
}

/// The edges [`apply_point_solution_in`] moves through the surf/surf/surf point:
/// `pEdge` in `brep` and its Boolean mate `pEMate`.
#[derive(Debug)]
pub struct ApplyPointSolutionEdges<'b> {
    /// The B-rep of `pEdge`.
    pub brep: &'b mut crate::topology::Brep,
    /// `pEdge`.
    pub edge: crate::topology::EdgeId,
    /// `pEMate` and its B-rep (`None`: the edge's own B-rep), `None` for no mate.
    pub mate: Option<(
        Option<&'b mut crate::topology::Brep>,
        crate::topology::EdgeId,
    )>,
}

/// [`apply_point_solution`] over B-rep edges (`sm_ApplyPointSolution` with `pEdge`):
/// after the classification curve, the edge (and then its mate, `edges.mate`) is
/// made to pass through the surf/surf/surf point with
/// [`set_xsect_edge_through_point_in`] over the edge surfaces given, starting from the
/// solution's second parameter; that parameter moves to the edge's, and the mate's
/// guess is the edge parameter mapped through the edge and mate intervals. The edge
/// handles of `input` are ignored; `edges` names them. `input`'s surfaces must not
/// borrow from the B-reps (copy face surfaces first).
///
/// # Errors
///
/// Those of [`apply_point_solution`] (without its edge deferral) and of
/// [`set_xsect_edge_through_point_in`].
pub fn apply_point_solution_in<O, S: Copy>(
    k: &Kernel,
    src_curve: Option<&mut Curve>,
    input: &ApplyPointSolutionInput<'_, O>,
    edges: Option<ApplyPointSolutionEdges<'_>>,
    solution: &mut Solution<S>,
    sss_point: Point3,
) -> Result<ApplyPointSolution> {
    let has_edge = edges.is_some();
    apply_point_solution_impl(k, src_curve, input, has_edge, edges, solution, sss_point)
}

/// `sm_ApplyPointSolution`; `has_edge` is `pEdge || pEMate`.
fn apply_point_solution_impl<O, S: Copy>(
    k: &Kernel,
    src_curve: Option<&mut Curve>,
    input: &ApplyPointSolutionInput<'_, O>,
    has_edge: bool,
    edges: Option<ApplyPointSolutionEdges<'_>>,
    solution: &mut Solution<S>,
    sss_point: Point3,
) -> Result<ApplyPointSolution> {
    // init output
    let mut out = ApplyPointSolution {
        success: false,
        sss_point,
        sss_point_flag: false,
        modified_curve: false,
        modified_edge_curve: false,
        modified_mate_curve: false,
        deviation: 0.0,
    };

    // locals (pEdge->GetCurve() only feeds upstream's gwcUpdateEdges == FALSE branch)
    let src_zone_tol_3d = input.src_zone_tol_3d;
    let mut t_deviation = 0.0;

    // let rSSSPoint = pClassCurveSurface1 evaluation at the intersection point
    let has_src = src_curve.is_some();
    let eval = if let (true, Some((s, uv))) = (has_src, input.class_curve_surface1) {
        Some(s.evaluate_point(k, uv))
    } else if let (true, Some((s, uv))) = (has_src, input.class_curve_surface2) {
        Some(s.evaluate_point(k, uv))
    } else if let (true, Some((s, uv))) = (has_edge, input.edge_curve_surface1) {
        Some(s.evaluate_point(k, uv))
    } else {
        None
    };
    match eval {
        None => return Ok(out),
        Some(Ok(p)) => out.sss_point = p,
        Some(Err(e)) if e.is_unported() => return Err(e),
        Some(Err(SolidError::Upstream { code, .. })) if code == SM_ERR => return Ok(out),
        Some(Err(_)) => {}
    }

    // set closest point on curve to surf/surf/surf intersection point
    if let Some(src_curve) = src_curve {
        // set up sm_SetXSectCurveThroughPoint arguments
        let param = solution.start.param(0)?; // init guess = curve/curve xsect solution
        let (Some((s1, uv1)), Some((s2, uv2))) =
            (input.class_curve_surface1, input.class_curve_surface2)
        else {
            return Err(SolidError::invalid_input(
                OP_APPLY,
                "classification curve without both surfaces",
            ));
        };

        // ensure surf/surf/surf solution is through point on classCurve
        let r = set_xsect_curve_through_point(
            k,
            src_curve,
            src_zone_tol_3d,
            &mut [],
            &[s1, s2],
            &[uv1, uv2],
            param,
            out.sss_point,
        )?;
        out.modified_curve = r.modified_curve;
        out.sss_point = r.point_3d;

        // inform the public - remove obsolete UVTrimCurves
        if out.modified_curve {
            out.sss_point_flag = true;
        }

        // move pSrcCurve3d parameter in rSolution to surf/surf/surf xsect point
        *solution.start.param_mut(0)? = r.param;

        // increase deviation to include distance to this curve from rSSSPoint
        if let Some(d) = r.deviation {
            t_deviation = d;
        }
        out.deviation = max2(out.deviation, t_deviation);
    }

    // set closest point on pEdge to surf/surf/surf intersection point
    if let Some(ApplyPointSolutionEdges { brep, edge, mate }) = edges {
        // set arguments for sm_SetXSectEdgeThroughPoint() call.
        let param = solution.start.param(1)?;
        let mut surfaces: Vec<&Surface> = Vec::new();
        let mut uv_points: Vec<Vec2> = Vec::new();
        if let Some((s, uv)) = input.edge_curve_surface1 {
            surfaces.push(s);
            uv_points.push(uv);
        }
        if let Some((s, uv)) = input.edge_curve_surface2 {
            surfaces.push(s);
            uv_points.push(uv);
        }

        // fix the pEdge and its UVTrimCurves - edge is only modified if it is piecewise C0
        let r = set_xsect_edge_through_point_in(
            k,
            brep,
            edge,
            out.sss_point,
            &surfaces,
            &uv_points,
            param,
        )?;
        out.modified_edge_curve = r.modified_curve;
        if let Some(d) = r.deviation {
            t_deviation = d;
        }

        // move pEdge parameter in rSolution to surf/surf/surf xsect point
        *solution.start.param_mut(1)? = r.param;

        // increase deviation to include distance to this edge from rSSSPoint
        out.deviation = max2(out.deviation, t_deviation);

        // inform the public - remove obsolete UVTrimCurves
        if out.modified_edge_curve {
            out.sss_point_flag = true;
        }

        // when there is an associated pEMate
        if let Some((mate_brep, mate_edge)) = mate {
            // use edge's solution to get a good guess on the mate (Inversion leaves
            // SM_UNDEF_DOUBLE for a degenerate interval)
            let normalized = crate::topology::edge::interval(brep, edge)?
                .inversion(r.param)
                .parameter
                .unwrap_or(crate::math::UNDEF_DOUBLE);
            let mate_brep = match mate_brep {
                Some(mb) => mb,
                None => brep,
            };
            let param1 =
                crate::topology::edge::interval(mate_brep, mate_edge)?.evaluate(normalized);

            // fix the pEMate and its UVTrimCurves
            let r1 = set_xsect_edge_through_point_in(
                k,
                mate_brep,
                mate_edge,
                out.sss_point,
                &surfaces,
                &uv_points,
                param1,
            )?;
            out.modified_mate_curve = r1.modified_curve;
            if let Some(d) = r1.deviation {
                t_deviation = d;
            }

            // increase deviation to include distance to this edge from rSSSPoint
            out.deviation = max2(out.deviation, t_deviation);
        }
    }

    // all done
    out.success = true;
    Ok(out)
}

/// Makes every interval whose interior classifies to an edge and whose start or end
/// classifies to a vertex use a vertex of that edge
/// (`sm_MakeConsistentClassifications`): for a vertex that is not one of the edge's, the
/// first edge of that vertex within the intersection tolerance of the interval midpoint
/// becomes the interior's edge, else the edge vertex nearer the classified vertex becomes
/// the end's vertex. The topology members (`SmEdge::GetVertex`, `SmVertex::GetEdges`,
/// `GetPoint`, `SmEdge::GetCurve`, `GetInterval`) go through
/// [`ClassGeometry::topology_entity`] and [`ClassGeometry::topology_handle`].
///
/// # Errors
///
/// [`SolidError::Unported`] for such an interval over handles the adapter cannot resolve
/// to a B-rep; a null-pointer error for a missing classification curve, edge, vertex or
/// edge curve upstream dereferences; those of the drop point.
pub fn make_consistent_classifications<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &mut CurveClassification<O>,
) -> Result<()> {
    const OP: &str = "sm_MakeConsistentClassifications";
    // locals
    let src_zone_tol_3d = classification.src_zone_tol_3d();

    // for every interval
    for i in 0..classification.size() {
        // If interval classifies to an Edge
        if classification.at(i)?.mid.point_class() != PointClassType::Edge {
            continue;
        }
        // the startPoint, then the endPoint, when it classifies to a Vertex (the endPoint
        // check reads the interior edge the startPoint fix may have substituted)
        for at_start in [true, false] {
            let ivl = *classification.at(i)?;
            let end = if at_start { ivl.start } else { ivl.end };
            if end.point_class() != PointClassType::Vertex {
                continue;
            }
            let classify_edge = ivl.mid.object().ok_or(SolidError::null_pointer(OP))?;
            let classify_vertex = end.object().ok_or(SolidError::null_pointer(OP))?;
            let (edge_vertex1, edge_vertex2) = super::p3::edge_vertices(geometry, classify_edge)?;

            // check that classified vertex is one of the Edge's vertices
            if classify_vertex == edge_vertex1 || classify_vertex == edge_vertex2 {
                continue;
            }

            // Try 1 - Simple ClassifyEdge substitution: the first edge attached to the
            // ClassifyVertex within XSectTol3d at the interval midpoint
            let mut fixed_problem = false;
            for vertex_edge in vertex_edges(geometry, classify_vertex)? {
                let t_mid = ivl.interval.evaluate(0.5);

                // Ivl MidPoint, XSectTol3d(SrcCrv,VertexEdge)
                let curve = classification
                    .curve()
                    .and_then(|c| geometry.curve(c))
                    .ok_or(SolidError::null_pointer(OP))?;
                let mid_pnt = curve.evaluate_point(k, t_mid)?;
                let xsect_tol_3d = xsect_tol_3d_from_zones(
                    k,
                    src_zone_tol_3d,
                    object_zone_tol_3d(k, geometry, Some(vertex_edge))?,
                );

                // drop a midInterval Point to this VertexEdge
                let edge_curve = super::p3::edge_curve(geometry, vertex_edge)?
                    .ok_or(SolidError::null_pointer(OP))?;
                let drop = edge_curve.drop_point(
                    k,
                    &super::p3::edge_interval(geometry, vertex_edge)?,
                    mid_pnt,
                    None,
                    xsect_tol_3d,
                    None,
                    SolverOperation::Minimize,
                )?;

                // if point was close enough to edge - substitute the ClassifyMid Edge target
                if drop.success {
                    classification
                        .at_mut(i)?
                        .mid
                        .set_class_object(PointClassType::Edge, Some(vertex_edge));
                    fixed_problem = true;
                    break;
                }
            }

            // Try 2 - simple ClassifyVertex substitution: the EdgeVertex closest to the
            // ClassifyVertex
            if !fixed_problem {
                let p = super::p3::vertex_point(geometry, classify_vertex)?;
                let nearer = if p.distance(super::p3::vertex_point(geometry, edge_vertex1)?)
                    < p.distance(super::p3::vertex_point(geometry, edge_vertex2)?)
                {
                    edge_vertex1
                } else {
                    edge_vertex2
                };
                let ivl_mut = classification.at_mut(i)?;
                let pc = if at_start {
                    &mut ivl_mut.start
                } else {
                    &mut ivl_mut.end
                };
                pc.set_class_object(PointClassType::Vertex, Some(nearer));
            }
        }
    }

    Ok(())
}

/// `SmVertex::GetEdges`: the handles of the edges attached to vertex handle `vertex`.
fn vertex_edges<O: ClassObject>(geometry: &impl ClassGeometry<O>, vertex: O) -> Result<Vec<O>> {
    const UPSTREAM: &str = "SmVertex::GetEdges";
    let (b, v) = super::p7::vertex_of(geometry, vertex, UPSTREAM)?;
    crate::topology::vertex::edges(b, v, None)?
        .into_iter()
        .map(|e| {
            geometry
                .topology_handle_in(b, EntityRef::Edge(e))
                .ok_or(SolidError::invalid_input(UPSTREAM, "edge without handle"))
        })
        .collect()
}

/// The angle in `[0, PI/2]` between the tangents of two curves at their parameters
/// (`sm_GetCurveCurveAngle`); `0` without either curve or when either tangent is
/// shorter than the scaled zero of the first point. `deviation` is only used by debug
/// code upstream.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations.
pub fn get_curve_curve_angle(
    k: &Kernel,
    this_curve: Option<&Curve>,
    this_param: f64,
    other_curve: Option<&Curve>,
    other_param: f64,
    deviation: f64,
) -> Result<f64> {
    let _ = deviation;
    // no work - no curves
    let (Some(this_curve), Some(other_curve)) = (this_curve, other_curve) else {
        return Ok(0.0);
    };

    // Evaluate the Curve locations
    let this_pv = curve_evaluate_1(k, this_curve, this_param)?;
    let other_pv = curve_evaluate_1(k, other_curve, other_param)?;

    // check for zero-length tangents.
    let scaled_zero = this_pv[0].scaled_zero();
    if this_pv[1].length() < scaled_zero {
        return Ok(0.0);
    }
    if other_pv[1].length() < scaled_zero {
        return Ok(0.0);
    }

    // get the angle between the tangents
    let mut angle = this_pv[1].angle_between(other_pv[1]).unwrap_or(0.0);

    // return angle from 0 to PI/2
    if angle < 0.0 {
        angle = -angle;
    }
    if angle > PI / 2.0 {
        angle = (PI - angle).abs();
    }

    // all done
    Ok(angle)
}

/// Tightens or culls the UV curve / trim curve intersections `solutions` with the
/// equivalent 3D intersections (`sm_CullOrRefineCCIIn3d`): a point solution takes the
/// 3D curve/curve intersection of the UV curve on `surface` with `curve_2` when that is
/// tighter, and is removed when there is none within `xsect_tol_3d`; a range solution
/// is removed when the curves are farther apart than `xsect_tol_3d`. As upstream, the
/// solution after a removed one is skipped.
///
/// # Errors
///
/// Those of the 3D intersection and distance computations.
pub fn cull_or_refine_cci_in_3d<S: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    uv_curve_1: &Curve,
    interval_1: &Extent1d,
    curve_2: &Curve,
    interval_2: &Extent1d,
    xsect_tol_3d: f64,
    solutions: &mut SolutionArray<S>,
) -> Result<()> {
    // project UVCurve into 3d space
    let curve_1 = Curve::from(CrvOnSurf::new(
        k,
        uv_curve_1.clone_for_evaluation()?,
        surface.clone_for_evaluation()?,
        None,
        0,
    )?);

    // for every solution
    let mut i = 0;
    while i < solutions.len() {
        let Some(sol) = solutions.get(i).copied() else {
            break;
        };

        // when the solution is a point (not an interval)
        if sol.solution_type == SolutionType::SingleValue {
            // get the 3dCurve/3Dcurve intersection
            let found = local_curve_intersect(
                k,
                &curve_1,
                interval_1,
                curve_2,
                interval_2,
                xsect_tol_3d,
                sol.start.param(0)?,
                sol.start.param(1)?,
            )?;

            // when 3dCurve/3dCurve intersection is tighter than 2dCurve/2dCurve intersection
            if let Some(x) = found {
                if x.deviation < sol.start.solution_value {
                    if let Some(s) = solutions.get_mut(i) {
                        s.start.solution_value = x.deviation;
                        *s.start.param_mut(0)? = x.my_param;
                        *s.start.param_mut(1)? = x.other_param;
                    }
                }
            }

            // when 3dCurve/3dCurve intersection is not good enough
            if found.is_none_or(|x| x.deviation > xsect_tol_3d) {
                solutions.remove_at(i, 1);
            }
        }
        // when the solution is a coincident curve pair
        else if sol.solution_type == SolutionType::RangeOfValues {
            // get quick max edge/UVTrimCurve 3d distance estimate at 8 sample points
            let sol_interval_1 = Extent1d::from_min_max(sol.start.param(0)?, sol.end.param(0)?);
            let mut max_dist = curve_1
                .curve_max_distance_between(
                    k,
                    &sol_interval_1,
                    curve_2,
                    sol.start.param(1)?,
                    sol.end.param(1)?,
                    8,
                    None,
                )?
                .distance;

            // when the quick estimate is good but not very tight
            if max_dist < xsect_tol_3d && max_dist > xsect_tol_3d / 100.00 {
                // get max dist value possible between curves by setting samplePointCount = 0
                max_dist = curve_1
                    .curve_max_distance_between(
                        k,
                        &sol_interval_1,
                        curve_2,
                        sol.start.param(1)?,
                        sol.end.param(1)?,
                        0,
                        None,
                    )?
                    .distance;
            }

            // when max edge/UVTrimCurve 3d distance is not good enough
            if max_dist > xsect_tol_3d {
                solutions.remove_at(i, 1);
            }
        }
        i += 1;
    }

    // all done
    Ok(())
}

/// Classifies a 3D point expected on `face` to it (`sm_ComputeFacePointClass`).
///
/// # Errors
///
/// [`SolidError::Unported`]: the face's surface (`SmFace::GetSurface`, unit
/// `topology.face_p2`) comes first.
pub fn compute_face_point_class<O: ClassObject>(
    point: Point3,
    src_zone_tol_3d: f64,
    face: O,
    point_class: &mut PointClassification<O>,
) -> Result<()> {
    let _ = (point, src_zone_tol_3d, face, point_class);
    // UNPORTED(topology.face_p2): SmFace::GetSurface
    // Proof (unreachable): trait default of FaceQueries::compute_face_point_class; the
    // B-rep-backed impl uses compute_face_point_class_in; 0 hits in the green difftest
    // sweep.
    Err(SolidError::unported(
        "SmFace::GetSurface",
        "topology.face_p2",
    ))
}

/// [`compute_face_point_class`] on face `f` of the B-rep `b` (`sm_ComputeFacePointClass`):
/// the point is dropped onto the face's surface over the face's UV domain with the
/// trimmed cache's point testing off (all solutions kept), retrying with ten and then
/// a hundred times the intersection tolerance of `src_zone_tol_3d` and the face's zone
/// tolerance; the closest solution's UV point is classified by `SmFace::PointClassify`
/// (`bDoXSects = TRUE`, `bUseUV = TRUE`, its status ignored as upstream).
///
/// `b` is mutable because `SmFace::PointClassify` builds the face's UV trim curves.
///
/// # Errors
///
/// `SM_ERR` when no drop finds a solution; `SM_ERR_NULL_POINTER` for a face without
/// surface or a surface whose cache cannot be built; those of the point solves;
/// [`SolidError::Unported`] from the classification.
pub fn compute_face_point_class_in(
    k: &Kernel,
    b: &mut crate::topology::Brep,
    f: crate::topology::FaceId,
    point: Point3,
    src_zone_tol_3d: f64,
    point_class: &mut PointClassification<EntityRef>,
) -> Result<()> {
    use crate::query::face::p2::{point_classify, BrepCacheFace, PointClassifyOptions};
    use crate::surface::surface::p1::{make_surface_cache, make_trim_srf_cache_with_face};
    use crate::surface::surface::p4::NoObject;
    use crate::topology::face::p2 as fp2;
    const OP: &str = "intersect::curve_class::p1::compute_face_point_class_in";

    // locals: SmSurface *pSurface = pFace->GetSurface(); NER(pSurface);
    let surface = fp2::surface(b, f)?
        .ok_or(SolidError::null_pointer(OP))?
        .clone_for_evaluation()?;
    let uv_domain = fp2::uv_domain(b, f)?;
    let face_zone_tol_3d = fp2::tolerance(b, f)?;

    // SmSurfaceCache *pSC = smsurf_GetSurfaceCache(pSurface) (the face's trimmed
    // surface cache, else its surface cache; NER when there is none). PointClassify
    // classifies through the trimmed one.
    let mut trim_cache = match make_trim_srf_cache_with_face(
        k,
        &surface,
        None::<EntityRef>,
        &mut BrepCacheFace { b: &mut *b, f },
    ) {
        Ok(c) => Some(c),
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => {
            if make_surface_cache(k, &surface, None::<EntityRef>).is_err() {
                return Err(SolidError::null_pointer(OP));
            }
            None
        }
    };

    // Project Point to Face->Surface, point testing off (every solution is kept
    // without classifying it against the trim boundaries)
    let xsect_tol_3d = xsect_tol_3d_from_zones(k, src_zone_tol_3d, face_zone_tol_3d);
    let solve = |tol: f64| {
        surface.global_point_solve_point_test_off::<NoObject>(
            k,
            Some(NoObject),
            &uv_domain,
            SolverOperation::Intersect,
            point,
            tol,
            None,
            SolutionRequested::All,
        )
    };
    let mut solutions = solve(src_zone_tol_3d)?;

    // If global point solve fails, try again with a bigger tolerance.
    if solutions.is_empty() {
        // increased tol in hopes of a solution
        solutions = solve(10.0 * xsect_tol_3d)?;

        // If point still fails to project, give it one more try.
        if solutions.is_empty() {
            // additional increased tol in hopes of a solution
            solutions = solve(100.0 * xsect_tol_3d)?;
            // signal error and give up if point still fails to project
            if solutions.is_empty() {
                return Err(SolidError::sm_err(OP));
            }
        }
    }

    // Find the best solution, and classify it to the face.
    let mut best_dist = crate::math::BIG_DOUBLE;
    let mut uv = Point2::new(0.0, 0.0);
    for sol in solutions.as_slice() {
        if sol.start.solution_value < best_dist {
            best_dist = sol.start.solution_value;
            let missing = || SolidError::invalid_input(OP, "point solve without parameters");
            uv = Point2::new(
                *sol.start.parameters.first().ok_or_else(missing)?,
                *sol.start.parameters.get(1).ok_or_else(missing)?,
            );
        }
    }

    // pFace->PointClassify(sUV, sSrcZoneTol3d, TRUE, TRUE, rPointClass) (status
    // ignored)
    ignored(point_classify(
        k,
        b,
        f,
        uv,
        point_class,
        PointClassifyOptions {
            src_zone_tol_3d,
            do_boundary_intersections: true,
            use_uv_classification: true,
            tgt_loops_only: false,
            tgt_loops: None,
            uv_space_okay: true,
            trim_cache: trim_cache.as_mut(),
        },
    ))?;

    // all done
    Ok(())
}

/// The tolerance within which two points of a classification curve coincide, in the
/// space the classification works in (`sm_GetXSectTol2dOr3d_ForClassifyPair`): for a
/// 3D curve the intersection tolerance of the two zone tolerances; for a UV curve on
/// `surface_1` the two tolerances mapped to UV space; for a UV curve without surface
/// `SM_EFF_ZERO_PARAM`.
///
/// # Errors
///
/// [`SolidError::Unported`] from the surface evaluation of `SmTol::MapTo2d`, and for
/// a curve of dimension other than 2 or 3 on a surface (`SmSurface::DropPoint`, unit
/// `surface.surface_p5`).
pub fn xsect_tol_2d_or_3d_for_classify_pair(
    k: &Kernel,
    src_curve: &Curve,
    surface_1: Option<&Surface>,
    src_curve_or_obj1_param: f64,
    src_curve_or_obj1_zone_tol_3d: f64,
    obj2_src_curve_param: f64,
    obj2_zone_tol_3d: f64,
) -> Result<f64> {
    // tolerance cases:
    if src_curve.dim() == 3 {
        // Curve3d
        Ok(xsect_tol_3d_from_zones(
            k,
            src_curve_or_obj1_zone_tol_3d,
            obj2_zone_tol_3d,
        ))
    } else if let Some(surface_1) = surface_1 {
        // Curve2d with m_cpSurface1
        let obj2_zone_tol_2d = zone_tol3d(
            k,
            map_to_2d_on_curve(
                k,
                obj2_zone_tol_3d,
                obj2_src_curve_param,
                src_curve,
                surface_1,
            )?,
        );
        let obj1_zone_tol_2d = zone_tol3d(
            k,
            map_to_2d_on_curve(
                k,
                src_curve_or_obj1_zone_tol_3d,
                src_curve_or_obj1_param,
                src_curve,
                surface_1,
            )?,
        );
        Ok(xsect_tol_3d_from_zones(
            k,
            obj1_zone_tol_2d,
            obj2_zone_tol_2d,
        ))
    } else {
        // Curve2d No m_cpSurface1
        Ok(xsect_tol3d(k, eff_zero_param()))
    }
}

/// The dimension of a topology object (`sm_ObjectDimension`): face 2, edge 1, vertex
/// 0, anything else (or none) 4.
#[must_use]
pub fn object_dimension<O: ClassObject>(object: Option<O>) -> i32 {
    match object.map(|o| o.class_kind()) {
        Some(ClassObjectKind::Face) => 2,
        Some(ClassObjectKind::Edge) => 1,
        Some(ClassObjectKind::Vertex) => 0,
        _ => 4,
    }
}

// ---- callees in units not ported yet ---------------------------------------------------

/// `SmSurface::LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, found, sol)`:
/// the gap and UV point of the answer, if found.
fn surface_local_point_solve(
    k: &Kernel,
    surface: &Surface,
    point: Point3,
    guess: Point2,
) -> Result<Option<(f64, Point2)>> {
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    // (the status is ignored upstream, only bFoundAnswer is read)
    let Some(r) = ignored(surface.local_point_solve::<()>(
        k,
        &domain,
        SolverOperation::Minimize,
        point,
        guess,
    ))?
    else {
        return Ok(None);
    };
    if r.found {
        Ok(Some((
            r.solution.start.solution_value,
            Point2::new(r.solution.start.param(0)?, r.solution.start.param(1)?),
        )))
    } else {
        Ok(None)
    }
}

/// `SmSurface::ApproximateSize`: upstream ignores the status, leaving the size zero.
fn surface_approximate_size(k: &Kernel, surface: &Surface) -> Result<Vec2> {
    match surface.approximate_size(k, None) {
        Ok(size) => Ok(size),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(Vec2::ZERO),
    }
}

/// `SmSurface::GlobalPointSolve(domain, SM_SO_MINIMIZE, point, 0.001 * |size|, &maxDrop,
/// SM_SR_SINGLE, sols)` over the natural domain (`maxDrop` is `1.0`): the gap and UV
/// point of the first solution, if any. The status is ignored (`SmCurveClass.cpp:1045`
/// checks only the solution count), so a failed solve is an empty result. Upstream
/// switches the surface cache's point test off for the solve (`m_bPointTestEnabled`,
/// `:1040`), so its solutions are not classified against a face's trim boundaries
/// ([`Surface::global_point_solve_point_test_off`]).
fn surface_global_point_solve(
    k: &Kernel,
    surface: &Surface,
    size: Vec2,
    point: Point3,
) -> Result<Option<(f64, Point2)>> {
    let tol = 0.001 * size.length();
    let solutions = ignored(
        surface.global_point_solve_point_test_off::<crate::surface::surface::p4::NoObject>(
            k,
            Some(crate::surface::surface::p4::NoObject),
            &surface.natural_uv_domain_with_nurb(k)?,
            SolverOperation::Minimize,
            point,
            tol,
            Some(1.0),
            SolutionRequested::Single,
        ),
    )?
    .unwrap_or_default();
    Ok(solutions.as_slice().first().map(|s| {
        (
            s.start.solution_value,
            Point2::new(
                s.start.parameters.first().copied().unwrap_or(0.0),
                s.start.parameters.get(1).copied().unwrap_or(0.0),
            ),
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    fn touch(dir: CrvDirOnFaceType, to: usize) -> TouchData<H> {
        TouchData {
            connected_to: Some(H(to, ClassObjectKind::Loop)),
            loop_dir: dir,
            loop_index: 0,
            param: 0.0,
            pt_classify_index: 0,
            pt_classify: None,
        }
    }

    #[test]
    fn winding_count() {
        use CrvDirOnFaceType as D;
        let l = [
            touch(D::In, 0),
            touch(D::Out, 0),
            touch(D::Out, 1),
            touch(D::In, 1),
            touch(D::In, 0),
            touch(D::Out, 0),
        ];
        assert_eq!(get_winding_count(&l).ok(), Some((1, OrientType::Opposite)));
        assert!(get_winding_count(&l[..3]).is_err());
        assert_eq!(
            get_connected_touches(Some(H(1, ClassObjectKind::Loop)), &l).len(),
            2
        );
    }

    #[test]
    fn set_size_and_remove() {
        let k = Kernel::default();
        let mut c: CurveClassification<H> = CurveClassification::new(&k);
        assert_eq!(c.interval().min(), 0.0);
        assert!(c.remove_at(0).is_err());
        c.set_size(&k, 3).ok();
        assert_eq!(c.size(), 3);
        // default constructed: system zone tolerance
        assert_eq!(
            c.at(0).map(|i| i.start.src_zone_tol_3d()).ok(),
            Some(1.0e-5)
        );
        c.remove_at(7).ok();
        assert_eq!(c.size(), 2);
        c.set_size(&k, 4).ok();
        // reset: uninit tolerance
        assert_eq!(
            c.at(3).map(|i| i.start.src_zone_tol_3d()).ok(),
            Some(UNINIT_TOL)
        );
        assert!(c.sub_interval(4).is_err());
        assert!(c.set_size(&k, MAX_INTERVALS + 1).is_err());
        c.set_size(&k, 0).ok();
        c.set_size(&k, 1).ok();
        assert_eq!(
            c.at(0).map(|i| i.start.src_zone_tol_3d()).ok(),
            Some(1.0e-5)
        );
    }

    #[test]
    fn squeeze_end_and_references() {
        let k = Kernel::default();
        let mut c: CurveClassification<H> = CurveClassification::new(&k);
        c.set_size(&k, 2).ok();
        let v = H(5, ClassObjectKind::Vertex);
        if let Ok(i) = c.at_mut(0) {
            i.start.set_class_object(PointClassType::Face, None);
            i.end.set_class_object(PointClassType::Vertex, Some(v));
        }
        assert_eq!(
            c.find_best_squeeze_end(0).ok(),
            Some(IntervalPosition::Start)
        );
        assert_eq!(
            c.at(0).map(|i| i.start.point_class()).ok(),
            Some(PointClassType::Vertex)
        );
        assert!(c.is_referenced(Some(v)));
        assert!(!c.is_referenced(None));
        assert_eq!(object_dimension(Some(v)), 0);
        assert_eq!(object_dimension::<H>(None), 4);
    }
}
