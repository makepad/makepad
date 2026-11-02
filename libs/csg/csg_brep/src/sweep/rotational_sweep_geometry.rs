//! Geometry of a rotational sweep: the surfaces, curves and points made by revolving
//! B-rep vertices, edges and faces about an axis.
//!
//! Upstream: `source/SMLib/src/SmRotationalSweepGeometry.cpp`,
//! `source/SMLib/inc/SmRotationalSweepGeometry.h`.
//!
//! | Rust                                                   | Upstream                                      |
//! |--------------------------------------------------------|-----------------------------------------------|
//! | [`RotationalSweepGeometry`]                            | `SmRotationalSweepGeometry`                   |
//! | [`RotationalSweepGeometry::new`]                       | `SmRotationalSweepGeometry(..)`               |
//! | [`RotationalSweepGeometry::copy`]                      | `Copy`                                        |
//! | (dropped)                                              | `~SmRotationalSweepGeometry`                  |
//! | [`RotationalSweepGeometry::is_sweep_closed`]           | `IsSweepClosed`                               |
//! | [`RotationalSweepGeometry::check_curve_sweep`]         | `CheckCurveSweep`                             |
//! | [`RotationalSweepGeometry::start_coord_system`]        | `GetStartCoordSystem`                         |
//! | [`RotationalSweepGeometry::end_coord_system`]          | `GetEndCoordSystem`                           |
//! | [`RotationalSweepGeometry::sweep_transform`]           | `GetSweepTransform`                           |
//! | [`RotationalSweepGeometry::vertex_sweep_higher`]       | `VertexSweepHigher`                           |
//! | [`RotationalSweepGeometry::vertex_sweep_same`]         | `VertexSweepSame`                             |
//! | [`RotationalSweepGeometry::edge_sweep_higher`], [`EdgeSweepHigher`] | `EdgeSweepHigher`                |
//! | [`RotationalSweepGeometry::edge_sweep_same`]           | `EdgeSweepSame`                               |
//! | [`RotationalSweepGeometry::face_sweep_same`]           | `FaceSweepSame`                               |
//! | `check_edge_curve_line` (private)                      | `sm_CheckEdgeCurveLine` (static)              |
//! | `check_edge_vertices_on_axis` (private)                | `sm_CheckEdgeVerticesOnAxis` (static)         |
//! | `check_edge_on_axis` (private)                         | `sm_CheckEdgeOnAxis` (static)                 |
//! | `create_rot_srf` (private)                             | `sm_CreateRotSrf` (static)                    |
//!
//! Deviations:
//! - **Out-params.** Results are return values. [`EdgeSweepHigher`] owns the 3D (and
//!   UV) trimming curves; upstream's extra curve pointers (`rpNewStartCurve`,
//!   `rpNewFarEndCurve`, `rpStartVertCurve`, `rpEndVertCurve`) are indices into
//!   [`EdgeSweepHigher::trim_curves`]. A vertex on the axis gives `None` from
//!   [`RotationalSweepGeometry::vertex_sweep_higher`] (upstream: a `NULL` curve and an
//!   unset tolerance); a full turn gives `None` from
//!   [`RotationalSweepGeometry::face_sweep_same`].
//! - **Context.** `SmContext` is the caller's [`Kernel`].
//! - **Base class.** `SmSweepGeometryCreation` is the [`SweepGeometryCreation`] field
//!   `base` (CO-PORTED in `sweep::topology_sweep`).
//! - **Casts.** `SM_CAST_PTR(SmBSplineCurve, curve)` is the B-spline part of the
//!   B-spline kinds (B-spline, line, ellipse/circle, hyperbola, parabola).
//!   `IsSegmentLine` on an `SmLine` dispatches to its line branch (`TRUE`); the port
//!   does the same. Its `SER` of a failed evaluation returns the nonzero status as
//!   `TRUE`; the port maps such errors to `true`.
//! - **Out-of-range read.** `sm_CheckEdgeOnAxis` reads `sSolutions[1]` when there is
//!   exactly one solution: past the array's size but inside its zero-filled
//!   allocation (`SM_SOLUTION_ARRAY_MIN_SIZE` is 4), so the port reads a
//!   zero-filled slot (`SM_ST_NODES_ONLY`). See `plan/findings.md`.
//! - **Disabled code.** `SM_SWEEP_CHECKS` is not defined upstream, so
//!   [`RotationalSweepGeometry::check_curve_sweep`] only runs the rotation-on-curve
//!   test (its sweep amount is unused); `SM_DEBUG_CODE` blocks, `SM_ASSERT`s and
//!   `smos_WriteBuffer` messages are dropped.
//! - **Analytic revolutions** (not upstream: WR-448, source issue #545;
//!   source change, source fork 0b143f7). With
//!   [`RotationalSweepGeometry::set_analytic_revolutions`] (off by default, so the
//!   default follows `c9c5979`; `PrimitiveCreation`'s partial rotational sweep sets it
//!   from `Kernel::fixes.revolve_analytic_profile_faces`), `EdgeSweepHigher` gives a line or circular arc its
//!   cylinder, cone, sphere or torus in the B-spline surface's layout (a line
//!   falling along the axis: the reversed line's, its cylinder or cone built from the
//!   low end and never inside out, as 9853e54's `AnalyticSurface`, #1463) instead of
//!   `CopyAndAddAnalytics` of the B-spline. The trims are the analytic surface's own
//!   boundary; the normal is the analytic surface's, taken before the copy as the
//!   fork does (`AnalyticSurface` replaces `sm_CreateRotSrf`).
//! - **Recursion.** None.

use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::line::Line;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::{line_closest_point, line_point_distance};
use crate::geom::solution_array::{ObjectKind, SolutionObject, SolutionType};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, NurbCircleParam};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surface::Surface;
use crate::topology::{edge, face, vertex, Brep, EdgeId, FaceId, VertexId};

use super::analytic_revolution::analytic_revolution;
use super::topology_sweep::{SweepCheckType, SweepGeometryCreation};

const OP_LINE: &str = "sweep::rotational_sweep_geometry::check_edge_curve_line";
const OP_VERTS: &str = "sweep::rotational_sweep_geometry::check_edge_vertices_on_axis";
const OP_ON_AXIS: &str = "sweep::rotational_sweep_geometry::check_edge_on_axis";
const OP_ROT_SRF: &str = "sweep::rotational_sweep_geometry::create_rot_srf";
const OP_CLOSED: &str =
    "sweep::rotational_sweep_geometry::RotationalSweepGeometry::is_sweep_closed";
const OP_START_CS: &str =
    "sweep::rotational_sweep_geometry::RotationalSweepGeometry::start_coord_system";
const OP_VERTEX_SAME: &str =
    "sweep::rotational_sweep_geometry::RotationalSweepGeometry::vertex_sweep_same";
const OP_EDGE_HIGHER: &str =
    "sweep::rotational_sweep_geometry::RotationalSweepGeometry::edge_sweep_higher";
const OP_EDGE_SAME: &str =
    "sweep::rotational_sweep_geometry::RotationalSweepGeometry::edge_sweep_same";
const OP_FACE_SAME: &str =
    "sweep::rotational_sweep_geometry::RotationalSweepGeometry::face_sweep_same";

/// `smos_Max` of two doubles.
fn smos_max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `SM_DEG2RAD(a)` = `a * (SM_PI / 180.0)`.
fn deg2rad(a: f64) -> f64 {
    a * (PI / 180.0)
}

/// `SM_CAST_PTR(SmBSplineCurve, curve)`: the B-spline (base) of the B-spline kinds.
const fn bspline_curve_of(curve: &Curve) -> Option<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Hermite(_)
        | CurveKind::Conic(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

/// A curve named by the solutions of a curve intersection (upstream's curve
/// pointer): the solvers' duplicate tests read its natural interval.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(u8, Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// `sm_CheckEdgeCurveLine`: whether the edge curve is a line over `interval` (five
/// samples, tolerance `eps`).
fn check_edge_curve_line(curve: Option<&Curve>, interval: &Extent1d, eps: f64) -> Result<bool> {
    let Some(curve) = curve else {
        return Err(SolidError::sm_err(OP_LINE));
    };
    let Some(bsp) = bspline_curve_of(curve) else {
        return Err(SolidError::sm_err(OP_LINE));
    };
    // SmLine::IsSegmentLine (virtual dispatch): the SmLine branch returns TRUE after
    // its (infallible) end point evaluations
    if matches!(curve.kind(), CurveKind::Line(_)) {
        return Ok(true);
    }
    let sample_points = 5;
    match bsp.is_segment_line(interval, sample_points, eps) {
        Ok(seg) => Ok(seg.is_line),
        Err(e) if e.is_unported() => Err(e),
        // SER inside an SmBoolean function: the nonzero status reads as TRUE
        Err(_) => Ok(true),
    }
}

/// The outputs of `sm_CheckEdgeVerticesOnAxis` (its `bParallelArg`, which no caller
/// reads, is not kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VerticesOnAxis {
    start_on: bool,
    end_on: bool,
    perp: bool,
}

/// The start vertex of `e` and its other vertex (upstream dereferences both).
fn start_and_other(b: &Brep, e: EdgeId, op: &'static str) -> Result<(VertexId, VertexId)> {
    let start = edge::start_vertex(b, e)?.ok_or_else(|| SolidError::null_pointer(op))?;
    let end = edge::other_vertex(b, e, Some(start))?.ok_or_else(|| SolidError::null_pointer(op))?;
    Ok((start, end))
}

/// `sm_CheckEdgeVerticesOnAxis`: whether the edge's end vertices are within `eps` of
/// the axis and, for a line, whether it is parallel or perpendicular to the axis.
fn check_edge_vertices_on_axis(
    base_pt: Point3,
    axis: Vec3,
    b: &Brep,
    e: EdgeId,
    edge_is_line: bool,
    eps: f64,
) -> Result<VerticesOnAxis> {
    // init output
    let mut out = VerticesOnAxis {
        start_on: false,
        end_on: false,
        perp: false,
    };

    // locals
    let (start_vtx, end_vtx) = start_and_other(b, e, OP_VERTS)?;
    let start_pt = vertex::point(b, start_vtx)?;
    let end_pt = vertex::point(b, end_vtx)?;

    // distance from edge start / end point to rotation axis
    let (start_dist, _) = line_point_distance(base_pt, axis, start_pt);
    let (end_dist, _) = line_point_distance(base_pt, axis, end_pt);

    // set output: remember if EdgeEndPoints are on rotation axis
    if start_dist < eps {
        out.start_on = true;
    }
    if end_dist < eps {
        out.end_on = true;
    }

    // when curve is linear - set perpendicular and parallel checks
    if edge_is_line {
        let line_vec = end_pt - start_pt;

        // NonZero length lines can be parallel or perpendicular
        if line_vec.length_squared() > eps * eps {
            out.perp = axis.is_perpendicular_to(line_vec, EFF_ZERO_DEG);
        }
    }

    // all done
    Ok(out)
}

/// `sm_CheckEdgeOnAxis`: whether the edge is coincident with the rotation axis.
///
/// # Errors
///
/// `SM_ERR` when a curved edge with both ends on the axis runs along only part of it
/// ("edge crosses axis"); those of the intersection.
fn check_edge_on_axis(
    k: &Kernel,
    base_pt: Point3,
    axis: Vec3,
    b: &Brep,
    e: EdgeId,
    edge_is_line: bool,
    eps: f64,
) -> Result<bool> {
    // see if edge->EndVertices are on rotation axis
    let on = check_edge_vertices_on_axis(base_pt, axis, b, e, edge_is_line, eps)?;

    // low work - either endVertex is not on rotation axis
    if !on.start_on || !on.end_on {
        return Ok(false);
    }

    // low work - edge is linear (and both end-points are on the rotation axis)
    if edge_is_line {
        return Ok(true);
    }

    // arrive here when non-linear edge has both endpoints on the rotation axis
    let curve = edge::curve(b, e)?.ok_or_else(|| SolidError::null_pointer(OP_ON_AXIS))?;

    // line->Vertex end points
    let start_vtx = edge::vertex(b, e)?.ok_or_else(|| SolidError::null_pointer(OP_ON_AXIS))?;
    let end_vtx = edge::other_vertex(b, e, Some(start_vtx))?
        .ok_or_else(|| SolidError::null_pointer(OP_ON_AXIS))?;
    let start_pt = vertex::point(b, start_vtx)?;
    let end_pt = vertex::point(b, end_vtx)?;

    // rotation axis parameters closest to Start/End Vtx locations
    let mut line_min = line_closest_point(base_pt, axis, start_pt);
    let mut line_max = line_closest_point(base_pt, axis, end_pt);

    // order the rotation axis parameters
    if line_max < line_min {
        core::mem::swap(&mut line_max, &mut line_min);
    }

    // make a temporary SmLine object for the rotation axis
    let axis_line = Curve::from(Line::infinite(base_pt, axis, 3, false));

    // intersect the curve with the rotation line
    let edge_iv = edge::interval(b, e)?;
    let this_ivl = curve.natural_interval();
    let other_ivl = axis_line.natural_interval();
    let opts = CurveIntersectOptions {
        skip_global_coincidence_check: false,
        handles: [
            Some(CurveHandle(0, this_ivl)),
            Some(CurveHandle(1, other_ivl)),
        ],
        analytic_handles: [
            Some(CurveHandle(2, this_ivl)),
            Some(CurveHandle(3, other_ivl)),
        ],
    };
    let solutions = curve.global_curve_intersect(
        k,
        &edge_iv,
        &axis_line,
        &Extent1d::from_min_max(line_min, line_max),
        eps,
        &opts,
    )?;
    let n_solutions = solutions.len();

    // sSolutions[1] with one solution reads the zero-filled slot after the last one
    let second_type = solutions
        .get(1)
        .map_or(SolutionType::NodesOnly, |s| s.solution_type);

    // no intersections, more than one, or one single valued solution: not on
    if n_solutions == 0 || n_solutions > 1 || second_type == SolutionType::SingleValue {
        return Ok(false);
    }

    // Must be coincident segment
    let first = solutions
        .get(0)
        .ok_or_else(|| SolidError::sm_err(OP_ON_AXIS))?;

    // Get start and end parameters of range on axis
    let axis_on_start = first.start.param(1)?;
    let axis_on_end = first.end.param(1)?;

    // for the edge to be ON, this segment of the axis must not be less than the one
    // we had above
    if axis_on_start > line_min + eps || axis_on_end < line_max - eps {
        // 'RotSweep: edge crosses axis, xsection IGNORED'
        return Err(SolidError::sm_err(OP_ON_AXIS));
    }
    Ok(true)
}

/// `sm_CreateRotSrf`: the B-spline surface of revolution of `curve` (which must be of
/// a B-spline kind) about the axis through `base_pt` along `axis` by `angle_deg`
/// degrees, its circular direction v (the start trim curve parameter is always
/// `SM_SP_U`).
pub(crate) fn create_rot_srf(
    k: &Kernel,
    curve: &Curve,
    base_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
) -> Result<BSplineSurface> {
    // only support BSpline Curves for now
    let bsc = bspline_curve_of(curve).ok_or_else(|| SolidError::null_pointer(OP_ROT_SRF))?;

    // pass the call along (an SmLine generator parallel to the axis gets an arc
    // length sweep parameterisation)
    let line_vector = match curve.kind() {
        CurveKind::Line(l) => Some(l.line_vector()),
        _ => None,
    };
    BSplineSurface::create_surf_of_revolution_with_line_vector(
        k,
        bsc,
        line_vector,
        base_pt,
        axis,
        angle_deg,
        false,
        None,
    )
}

/// The sweep geometry of [`RotationalSweepGeometry::edge_sweep_higher`]. The four
/// curve fields name entries of [`EdgeSweepHigher::trim_curves`] (upstream's curve
/// pointers into `r3DTrimmingCurves`).
#[derive(Debug)]
pub struct EdgeSweepHigher {
    /// `rpNewSurface`: the swept surface; `None` when the edge lies on the axis.
    pub surface: Option<Surface>,
    /// `rdNewFaceTol` (`-1` when not set).
    pub face_tol: f64,
    /// `rpNewStartCurve`: the edge curve in its original position.
    pub start_curve: Option<usize>,
    /// `rdNewStartCurveTol` (`-1` when not set).
    pub start_curve_tol: f64,
    /// `rpNewFarEndCurve`: the edge curve at the end of the sweep.
    pub far_end_curve: Option<usize>,
    /// `rdNewEdgeTol` (`-1` when not set).
    pub edge_tol: f64,
    /// `rpStartVertCurve`: the sweep curve of the start vertex.
    pub start_vert_curve: Option<usize>,
    /// `rdNewStartVertEdgeTol` (`-1` when not set).
    pub start_vert_edge_tol: f64,
    /// `rpEndVertCurve`: the sweep curve of the end vertex.
    pub end_vert_curve: Option<usize>,
    /// `rdNewEndVertEdgeTol` (`-1` when not set).
    pub end_vert_edge_tol: f64,
    /// `r3DTrimmingCurves`: the ordered 3D trimming curves of the outer loop.
    pub trim_curves: Vec<Curve>,
    /// `rUVTrimmingCurves`: their UV lines (empty for the planar sweep of a line
    /// perpendicular to the axis).
    pub uv_trim_curves: Vec<Line>,
    /// `rOrients`: the orientation of each trimming curve in the loop.
    pub orients: Vec<OrientType>,
    /// `rsNewSurfaceNormal`: the surface normal at the minimum corner of the domain;
    /// `None` when not computed (upstream leaves the caller's value).
    pub normal: Option<Vec3>,
}

impl EdgeSweepHigher {
    /// The initialised outputs (`NULL` pointers, tolerances `-1`, empty arrays).
    const fn empty() -> Self {
        Self {
            surface: None,
            face_tol: -1.0,
            start_curve: None,
            start_curve_tol: -1.0,
            far_end_curve: None,
            edge_tol: -1.0,
            start_vert_curve: None,
            start_vert_edge_tol: -1.0,
            end_vert_curve: None,
            end_vert_edge_tol: -1.0,
            trim_curves: Vec::new(),
            uv_trim_curves: Vec::new(),
            orients: Vec::new(),
            normal: None,
        }
    }
}

/// The trimming curves of [`RotationalSweepGeometry::edge_sweep_higher`] by role
/// while the loop is assembled (upstream's pointers into `a3DTrimmingCurves`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// `a3DTrimmingCurves[0]`: the base curve.
    Base,
    /// `a3DTrimmingCurves[1]`: the first side curve.
    Side1,
    /// `a3DTrimmingCurves[2]`: the swept base curve.
    Swept,
    /// `a3DTrimmingCurves[3]`: the second side curve.
    Side3,
}

/// The geometry creator of a rotational sweep (`SmRotationalSweepGeometry`): an axis
/// through a base point, an angle in degrees and a tolerance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotationalSweepGeometry {
    /// The `SmSweepGeometryCreation` members (CO-PORTED).
    pub base: SweepGeometryCreation,
    /// `m_sBasePt`: a point of the rotation axis.
    base_pt: Point3,
    /// `m_sAxis`: the unit direction of the rotation axis.
    axis: Vec3,
    /// `m_dAngle`: the sweep angle in degrees.
    angle: f64,
    /// `m_dEps`: the tolerance for geometry on the axis, zero sweep lengths, lines.
    eps: f64,
    /// source change (source fork 0b143f7): sweep a line or circular arc
    /// to its analytic surface (see [`Self::set_analytic_revolutions`]).
    analytic_revolutions: bool,
}

impl RotationalSweepGeometry {
    /// `SmRotationalSweepGeometry(Center, Axis, Angle, Eps)`: the axis is unitized
    /// (a zero axis becomes `(0, 0, 1)`, as upstream's `Unitize`). The angle should
    /// be in `(0, 360]` away from `0` and `360` (upstream only asserts it).
    #[must_use]
    pub fn new(center: Point3, axis: Vec3, angle_deg: f64, eps: f64) -> Self {
        Self {
            base: SweepGeometryCreation::new(eps),
            base_pt: center,
            axis: axis.unitize_or_z(),
            angle: angle_deg,
            eps,
            analytic_revolutions: false,
        }
    }

    /// `Copy`: a new sweep geometry from this one's data (the axis is unitized again)
    /// with the same analytics settings.
    #[must_use]
    pub fn copy(&self) -> Self {
        let mut new = Self::new(self.base_pt, self.axis, self.angle, self.eps);
        new.base.set_make_analytics(self.base.make_analytics);
        new.analytic_revolutions = self.analytic_revolutions;
        new
    }

    /// source change (source fork 0b143f7, source issue #545):
    /// with `make_analytics` also set, [`Self::edge_sweep_higher`] gives an edge
    /// whose curve is a line or circular arc in a plane through the axis its
    /// cylinder, cone, sphere or torus ([`analytic_revolution`]) in place of the
    /// B-spline surface of revolution, laid out as that B-spline so that its boundary
    /// makes the same loop. Off by default, as upstream at `c9c5979`.
    pub fn set_analytic_revolutions(&mut self, on: bool) {
        self.analytic_revolutions = on;
    }

    /// Whether [`Self::set_analytic_revolutions`] is on.
    #[must_use]
    pub const fn analytic_revolutions(&self) -> bool {
        self.analytic_revolutions
    }

    /// `m_sBasePt`.
    #[must_use]
    pub const fn base_point(&self) -> Point3 {
        self.base_pt
    }

    /// `m_sAxis` (unit).
    #[must_use]
    pub const fn axis(&self) -> Vec3 {
        self.axis
    }

    /// `m_dAngle` (degrees).
    #[must_use]
    pub const fn angle(&self) -> f64 {
        self.angle
    }

    /// `m_dEps`.
    #[must_use]
    pub const fn eps(&self) -> f64 {
        self.eps
    }

    /// `IsSweepClosed`: whether `repeats` sweeps make a full turn (360 degrees within
    /// `SM_EFF_ZERO`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when they exceed a full turn.
    pub fn is_sweep_closed(&self, repeats: u64) -> Result<bool> {
        let d_repeats = repeats as f64;
        let mut ret = false;
        if 360.0 - EFF_ZERO < d_repeats * self.angle && d_repeats * self.angle < 360.0 + EFF_ZERO {
            ret = true;
        }
        if d_repeats * self.angle > 360.0 + EFF_ZERO {
            return Err(SolidError::sm_err(OP_CLOSED));
        }
        Ok(ret)
    }

    /// `CheckCurveSweep`: whether sweeping `curve` over `interval` about the axis
    /// through `sweep_point` along `sweep_vec` gives a valid surface. Only the
    /// rotation-about-an-interior-point test is compiled upstream: a curve/axis
    /// intersection off the interval ends (within `dist_tol_3d`) is
    /// [`SweepCheckType::RotOnCurve`], else [`SweepCheckType::Okay`]. `sweep_amount`
    /// is not used.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::intersect_with_line`].
    pub fn check_curve_sweep(
        &self,
        k: &Kernel,
        curve: &Curve,
        interval: &Extent1d,
        dist_tol_3d: f64,
        sweep_point: Point3,
        sweep_vec: Vec3,
        sweep_amount: f64,
    ) -> Result<SweepCheckType> {
        // (SM_SWEEP_CHECKS is not defined: the sweep amount is unused)
        let _ = sweep_amount;

        // SM_SC_ROT_ON_CURVE: Check for illegal rotations about curve mid points

        // intersect the curve with the rotation line
        let sweep_axis = Line::infinite(sweep_point, sweep_vec, 3, false);
        let axis_ivl = sweep_axis.natural_interval();
        let x = curve.intersect_with_line(k, interval, &sweep_axis, &axis_ivl, dist_tol_3d)?;

        // for every solution - check for midCurve rotations
        for sol in &x.solutions {
            let param = sol.start.param(0)?;

            // when the solution is not an end point
            if !interval.is_value_on_boundary(param, dist_tol_3d).0 {
                // attempting to rotate a curve about a mid point
                return Ok(SweepCheckType::RotOnCurve);
            }
        }

        // arrive here after passing all tests
        Ok(SweepCheckType::Okay)
    }

    /// `GetStartCoordSystem`: the frame at `pt` with X along the axis and Y pointing
    /// away from it.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `pt` is on the axis (within `SM_EFF_ZERO` scaled by its size).
    pub fn start_coord_system(&self, pt: Point3) -> Result<Axis2Placement> {
        // Get the vector from the given point to the axis, perpendicular.
        let mut diff_vec = pt - self.base_pt;
        let dot = diff_vec.dot(self.axis);
        let pt_on_axis = self.base_pt + dot * self.axis;
        diff_vec = pt - pt_on_axis;

        let scaled_zero = EFF_ZERO * (1.0 + pt.max_dimension());
        let len = diff_vec.length();
        if len < scaled_zero {
            return Err(SolidError::sm_err(OP_START_CS));
        }

        diff_vec /= len;

        let mut cs = Axis2Placement::default();
        // (status not checked upstream)
        let _ = cs.set_canonical(pt, self.axis, diff_vec);
        Ok(cs)
    }

    /// `GetEndCoordSystem`: [`Self::start_coord_system`] rotated by the sweep angle
    /// about the axis (not for a full turn).
    ///
    /// # Errors
    ///
    /// Those of [`Self::start_coord_system`].
    pub fn end_coord_system(&self, pt: Point3) -> Result<Axis2Placement> {
        // Get the start C.S. and rotate by m_dAngle about m_sBasePt/m_sAxis.
        let mut cs = self.start_coord_system(pt)?;

        // No need to rotate by 360.
        if self.angle < 360.0 - EFF_ZERO {
            cs.rotate_about_axis_at_point(deg2rad(self.angle), self.base_pt, self.axis);
        }
        Ok(cs)
    }

    /// `GetSweepTransform`: the rotation by the sweep angle about the axis (identity
    /// for a full turn).
    #[must_use]
    pub fn sweep_transform(&self) -> Axis2Placement {
        let mut cs = Axis2Placement::default();
        cs.init();

        // No need to rotate by 360.
        if self.angle < 360.0 - EFF_ZERO {
            cs.rotate_about_axis_at_point(deg2rad(self.angle), self.base_pt, self.axis);
        }
        cs
    }

    /// `VertexSweepHigher`: the degree 2 circular arc swept by vertex `v`, with the
    /// vertex tolerance; `None` when the vertex is on the axis (within the larger of
    /// the sweep and vertex tolerances).
    ///
    /// # Errors
    ///
    /// Those of [`Vec3::unitize`] and [`BSplineCurve::create_circle_segment`].
    pub fn vertex_sweep_higher(&self, b: &Brep, v: VertexId) -> Result<Option<(Curve, f64)>> {
        let original_pt = vertex::point(b, v)?;
        let orig_vertex_tolerance = vertex::tolerance(b, v)?;

        let rot_center_par = self.axis.dot(original_pt - self.base_pt);
        let rot_center = self.base_pt + rot_center_par * self.axis;

        let eps = smos_max(self.eps, orig_vertex_tolerance);

        let mut x_vect = original_pt - rot_center;
        if x_vect.length() < eps {
            // point lies on axis
            return Ok(None);
        }
        x_vect = x_vect.unitize()?;

        let mut y_vect = x_vect.cross(self.axis);
        let discr = y_vect.length(); // sin angle between axis and center-to-point
        if discr < eps {
            // point lies on axis
            return Ok(None);
        }
        y_vect = y_vect.unitize()?;

        let z_vect = x_vect.cross(y_vect);

        if z_vect.dot(self.axis) < 0.0 {
            y_vect = -y_vect;
        }

        let rad = (rot_center - original_pt).length();

        let mut a2p = Axis2Placement::default();
        // (status not checked upstream)
        let _ = a2p.set_canonical(rot_center, x_vect, y_vect);

        // degree 2
        let circle = BSplineCurve::create_circle_segment(
            3,
            &a2p,
            rad,
            0.0,
            self.angle,
            NurbCircleParam::Quadratic,
        )?;

        Ok(Some((Curve::from(circle), orig_vertex_tolerance)))
    }

    /// `VertexSweepSame`: vertex `v`'s point rotated by the sweep angle (unrotated for
    /// a full turn) and its tolerance.
    ///
    /// # Errors
    ///
    /// Those of [`Self::is_sweep_closed`]; `SM_ERR` when the rotated point is within
    /// the vertex tolerance of the original.
    pub fn vertex_sweep_same(&self, b: &Brep, v: VertexId) -> Result<(Point3, f64)> {
        let sweep_is_closed = self.is_sweep_closed(1)?;
        let point = vertex::point(b, v)?;
        let tolerance = vertex::tolerance(b, v)?;

        if sweep_is_closed {
            return Ok((point, tolerance));
        }

        let angle_rad = self.angle * PI / 180.0;

        let mut ref_frame = Axis2Placement::default();
        ref_frame.rotate_about_axis_at_point(angle_rad, self.base_pt, self.axis);
        let copied_pt = ref_frame.transform_point(point);

        let di = (copied_pt - point).length();
        if di < tolerance {
            // This is a problem for now
            return Err(SolidError::sm_err(OP_VERTEX_SAME));
        }

        Ok((copied_pt, tolerance))
    }

    /// `EdgeSweepHigher`: the surface swept by edge `e` and the trimming curves of its
    /// outer loop. The surface is the B-spline surface of revolution of the edge
    /// curve trimmed to the edge, or for a line perpendicular to the axis a bilinear
    /// plane trimmed by the revolution's boundary curves (with only a circle when a
    /// full turn degenerates one side); copied as an analytic surface when
    /// `make_analytics` is set. An edge on the axis gives no surface.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an edge without B-spline curve, an edge crossing the axis, a
    /// curve that does not sweep into a valid surface ([`Self::check_curve_sweep`]),
    /// or end points that do not match the swept boundary; those of the called
    /// constructors and queries.
    pub fn edge_sweep_higher(&self, k: &Kernel, b: &Brep, e: EdgeId) -> Result<EdgeSweepHigher> {
        // Init outputs
        let mut out = EdgeSweepHigher::empty();

        // locals
        let original_edge_curve = edge::curve(b, e)?;
        let original_edge_ivl = edge::interval(b, e)?;
        let brep_tol = b.tolerance();
        let eps = smos_max(self.eps, brep_tol);

        // check for input line
        let is_line = check_edge_curve_line(original_edge_curve, &original_edge_ivl, eps)?;

        // check edge on axis
        let on_edge = check_edge_on_axis(k, self.base_pt, self.axis, b, e, is_line, eps)?;

        // no work - edge is on axis and it's not transformed by the rotation
        if on_edge {
            return Ok(out);
        }
        let original_edge_curve =
            original_edge_curve.ok_or_else(|| SolidError::sm_err(OP_EDGE_HIGHER))?;

        // check to see if the curve will sweep into a valid surface
        let sweep_check = self.check_curve_sweep(
            k,
            original_edge_curve,
            &original_edge_ivl,
            eps,
            self.base_pt,
            self.axis,
            self.angle,
        )?;
        if sweep_check != SweepCheckType::Okay {
            return Err(SolidError::sm_err(OP_EDGE_HIGHER));
        }

        // StartVertex: According to the CURVE, the edge may be inverted
        let (start_vtx, end_vtx) = start_and_other(b, e, OP_EDGE_HIGHER)?;
        let start_pt = vertex::point(b, start_vtx)?;
        let end_pt = vertex::point(b, end_vtx)?;

        let st_vtx_tol = vertex::tolerance(b, start_vtx)?;
        let end_vtx_tol = vertex::tolerance(b, end_vtx)?;
        let orig_edge_tol = edge::tolerance(b, e)?;

        // Have to copy the original edge curve because we have to trim it.
        let mut copied_orig_edge_curve = original_edge_curve.copy(k)?;
        let mut trim_ivl = original_edge_ivl;
        copied_orig_edge_curve.trim(k, &mut trim_ivl, TrimMode::Notify)?;

        let on_axis = check_edge_vertices_on_axis(self.base_pt, self.axis, b, e, is_line, eps)?;

        // This setting seems to be needed for CreateLinearSweep
        let start_trim_curve_param = SurfParamType::U;
        let mut is_plane = false;
        let (new_surf, surf_to_trim) = if is_line && on_axis.perp {
            // Creates the surface by SmBSplineSurface::CreateSurfOfRevolution
            let surf_to_trim = create_rot_srf(
                k,
                &copied_orig_edge_curve,
                self.base_pt,
                self.axis,
                self.angle,
            )?;

            let (start_v_rad, _) = line_point_distance(self.base_pt, self.axis, start_pt);
            let (end_v_rad, _) = line_point_distance(self.base_pt, self.axis, end_pt);

            let line_start_v_par = line_closest_point(self.base_pt, self.axis, start_pt);
            let line_end_v_par = line_closest_point(self.base_pt, self.axis, end_pt);

            // These 2 coincide within eps:
            let start_v_base_pt = self.base_pt + self.axis * line_start_v_par;
            let end_v_base_pt = self.base_pt + self.axis * line_end_v_par;

            // Remaining on the safe side:
            let discr = (start_v_base_pt - end_v_base_pt).length();
            if discr > eps {
                return Err(SolidError::sm_err(OP_EDGE_HIGHER));
            }

            let rot_center = 0.5 * (start_v_base_pt + end_v_base_pt);

            // the farther vertex gives more accurate results
            let mut x_vect = if start_v_rad < end_v_rad {
                end_pt - end_v_base_pt
            } else {
                start_pt - start_v_base_pt
            };

            let z_dir = self.axis.unitize()?; // perhaps it is unit already

            let mut y_vect = z_dir.cross(x_vect);

            x_vect = 1.1 * x_vect;
            y_vect = 1.1 * y_vect;

            // The 4 corners of the plane (swollen a bit to avoid numerics)
            let plane_uv00 = rot_center - x_vect - y_vect;
            let plane_uv10 = rot_center + x_vect - y_vect;

            let plane_uv01 = rot_center - x_vect + y_vect;
            let plane_uv11 = rot_center + x_vect + y_vect;

            // (upstream passes the corners in this order)
            let new_surf = BSplineSurface::create_bilinear_surface(
                plane_uv00, plane_uv01, plane_uv10, plane_uv11,
            )?;
            is_plane = true;
            (new_surf, Some(Surface::from(surf_to_trim)))
        } else {
            // Creates the surface by SmBSplineSurface::CreateSurfOfRevolution
            let new_surf = create_rot_srf(
                k,
                &copied_orig_edge_curve,
                self.base_pt,
                self.axis,
                self.angle,
            )?;
            (new_surf, None)
        };
        let mut new_surf = Surface::from(new_surf);

        // source change (source fork 0b143f7, 9853e54): the cylinder,
        // cone, sphere or torus the curve sweeps, in place of the B-spline before its
        // normal is taken (AnalyticSurface precedes it), which CopyAndAddAnalytics
        // does not give
        let analytic = if self.base.make_analytics && self.analytic_revolutions && !is_plane {
            analytic_revolution(
                k,
                &copied_orig_edge_curve,
                self.base_pt,
                self.axis,
                self.angle,
                eps,
                Some(&new_surf),
            )?
        } else {
            None
        };
        let is_analytic = analytic.is_some();
        if let Some(surface) = analytic {
            new_surf = surface;
        }

        let start_uv = new_surf.natural_uv_domain_with_nurb(k)?.min();
        let normal = new_surf.evaluate_normal(k, start_uv, Side::Left, Side::Left)?;
        out.normal = Some(normal);

        let mut flip_orients = false;
        if let Some(to_trim) = &surf_to_trim {
            let start_param = to_trim.natural_uv_domain_with_nurb(k)?.min();
            let plane_normal = to_trim.evaluate_normal(k, start_param, Side::Left, Side::Left)?;
            let dot = normal.dot(plane_normal);
            if dot.abs() < 0.8 {
                return Err(SolidError::sm_err(OP_EDGE_HIGHER));
            }
            if dot < 0.0 {
                flip_orients = true;
            }
        }

        // Create corresponding analytics if requested
        if self.base.make_analytics && !is_analytic {
            // Copy pNewSurf, when possible as an analytic surface
            new_surf = new_surf.copy_and_add_analytics(k)?;
        }

        // eStartTrimCurveParam fixes which of the umin/vmin line of the surface
        // represents the copy of the edge
        let natural = {
            let s = surf_to_trim.as_ref().unwrap_or(&new_surf);
            s.create_natural_uv_trim_curves(
                k,
                &s.natural_uv_domain_with_nurb(k)?,
                start_trim_curve_param,
                false,
            )?
        };
        let mut a3d: Vec<Option<Curve>> = natural.curves_3d.into_iter().map(Some).collect();
        let mut a_uv: Vec<Option<Line>> = natural.uv_curves.into_iter().map(Some).collect();
        let mut a_orients = natural.orients;
        if a3d.len() < 4 || a_uv.len() < 4 || a_orients.len() < 4 {
            return Err(SolidError::sm_err(OP_EDGE_HIGHER));
        }
        let mut roles = [Role::Base, Role::Side1, Role::Swept, Role::Side3];

        let (curve_start_pt, curve_end_pt) = {
            let ec = a3d
                .first()
                .and_then(Option::as_ref)
                .ok_or_else(|| SolidError::sm_err(OP_EDGE_HIGHER))?;
            let ivl = ec.natural_interval();
            (
                ec.evaluate_point(k, ivl.min())?,
                ec.evaluate_point(k, ivl.max())?,
            )
        };

        // The edge curve may be oriented arbitrarily wrt edge
        let mut swap_start_end = false;
        let st_st = (curve_start_pt - start_pt).length();
        let st_end = (curve_start_pt - end_pt).length();
        let end_st = (curve_end_pt - start_pt).length();
        let end_end = (curve_end_pt - end_pt).length();
        if st_end < st_st {
            swap_start_end = true;
            if st_end > eps || end_st > eps {
                return Err(SolidError::sm_err(OP_EDGE_HIGHER));
            }
        } else if end_end > eps || st_st > eps {
            return Err(SolidError::sm_err(OP_EDGE_HIGHER));
        }

        // With the purpose to orient the loop correctly, here we invert the
        // orientations, swap the side curves, and flip the flag too.
        if flip_orients {
            swap_start_end = !swap_start_end;
            for o in a_orients.iter_mut().take(4) {
                *o = reverse_orientation(*o);
            }
            a3d.swap(1, 3);
            roles.swap(1, 3);
            a_orients.swap(1, 3);
            if surf_to_trim.is_none() {
                a_uv.swap(1, 3);
            }
        }

        // Now, create the results:
        let keep_uv = surf_to_trim.is_none();
        let mut is_degenerate = false;
        let mut r3d: Vec<(Role, Curve)> = Vec::new();
        let mut r_uv: Vec<Line> = Vec::new();
        let mut r_orients: Vec<OrientType> = Vec::new();
        let take = |v: &mut Vec<Option<Curve>>, i: usize| {
            v.get_mut(i)
                .and_then(Option::take)
                .ok_or_else(|| SolidError::sm_err(OP_EDGE_HIGHER))
        };
        let take_uv = |v: &mut Vec<Option<Line>>, i: usize| {
            v.get_mut(i)
                .and_then(Option::take)
                .ok_or_else(|| SolidError::sm_err(OP_EDGE_HIGHER))
        };
        let orient_at = |v: &[OrientType], i: usize| {
            v.get(i)
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP_EDGE_HIGHER))
        };
        let role_at = |i: usize| roles.get(i).copied().unwrap_or(Role::Base);

        //------0----- processing the base curve
        r3d.push((role_at(0), take(&mut a3d, 0)?));
        r_orients.push(orient_at(&a_orients, 0)?);
        if keep_uv {
            r_uv.push(take_uv(&mut a_uv, 0)?);
        }
        let mut start_curve = Some(role_at(0));
        out.start_curve_tol = orig_edge_tol;

        //----1----- the first side curve (NULL if degenerate)
        let mut start_vert_curve = None;
        let mut end_vert_curve = None;
        let c = take(&mut a3d, 1)?;
        let uv = take_uv(&mut a_uv, 1)?;
        let side = if c.is_degenerate(k, out.start_curve_tol, None)? {
            is_degenerate = true;
            None
        } else {
            r3d.push((role_at(1), c));
            r_orients.push(orient_at(&a_orients, 1)?);
            if keep_uv {
                r_uv.push(uv);
            }
            Some(role_at(1))
        };

        if swap_start_end {
            out.end_vert_edge_tol = end_vtx_tol;
            end_vert_curve = side;
        } else {
            out.start_vert_edge_tol = st_vtx_tol;
            start_vert_curve = side;
        }

        //----2----- the swept base curve
        r3d.push((role_at(2), take(&mut a3d, 2)?));
        r_orients.push(orient_at(&a_orients, 2)?);
        if keep_uv {
            r_uv.push(take_uv(&mut a_uv, 2)?);
        }
        out.edge_tol = orig_edge_tol;
        let mut far_end_curve = if self.angle == 360.0 {
            None
        } else {
            Some(role_at(2))
        };

        //----3----- the second side curve (NULL if degenerate)
        let c = take(&mut a3d, 3)?;
        let uv = take_uv(&mut a_uv, 3)?;
        let side = if c.is_degenerate(k, out.start_curve_tol, None)? {
            is_degenerate = true;
            None
        } else {
            r3d.push((role_at(3), c));
            r_orients.push(orient_at(&a_orients, 3)?);
            if keep_uv {
                r_uv.push(uv);
            }
            Some(role_at(3))
        };

        if !edge::is_closed(b, e)? {
            if swap_start_end {
                out.start_vert_edge_tol = st_vtx_tol;
                start_vert_curve = side;
            } else {
                out.end_vert_edge_tol = end_vtx_tol;
                end_vert_curve = side;
            }
        }

        // If we have a degenerate point and a plane we can trim it with just a circle.
        if is_degenerate && is_plane && self.angle > 360.0 - EFF_ZERO_SQRT {
            start_curve = None;
            remove_first(&mut r3d, &mut r_uv, &mut r_orients, keep_uv);
            if (start_vert_curve.is_none() && !swap_start_end)
                || (end_vert_curve.is_none() && swap_start_end)
            {
                remove_first(&mut r3d, &mut r_uv, &mut r_orients, keep_uv);
            } else {
                r3d.pop();
                if keep_uv {
                    r_uv.pop();
                }
                r_orients.pop();
            }
            far_end_curve = None;
        }

        // All done
        let index_of =
            |role: Option<Role>| role.and_then(|r| r3d.iter().position(|(x, _)| *x == r));
        out.start_curve = index_of(start_curve);
        out.far_end_curve = index_of(far_end_curve);
        out.start_vert_curve = index_of(start_vert_curve);
        out.end_vert_curve = index_of(end_vert_curve);
        out.trim_curves = r3d.into_iter().map(|(_, c)| c).collect();
        out.uv_trim_curves = r_uv;
        out.orients = r_orients;
        out.surface = Some(new_surf);
        out.face_tol = orig_edge_tol;
        Ok(out)
    }

    /// `EdgeSweepSame`: edge `e`'s curve, trimmed to the edge, rotated by the sweep
    /// angle (unrotated for an edge on the axis or a full turn), and its tolerance.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an edge without (B-spline) curve or crossing the axis; those of
    /// the copy, trim and transform.
    pub fn edge_sweep_same(&self, k: &Kernel, b: &Brep, e: EdgeId) -> Result<(Curve, f64)> {
        let original_edge_curve = edge::curve(b, e)?;
        let original_edge_iv = edge::interval(b, e)?;
        let original_edge_tol = edge::tolerance(b, e)?;
        let Some(original_edge_curve) = original_edge_curve else {
            return Err(SolidError::sm_err(OP_EDGE_SAME));
        };

        let brep_tol = b.tolerance();
        let eps = smos_max(self.eps, brep_tol);

        let is_line = check_edge_curve_line(Some(original_edge_curve), &original_edge_iv, eps)?;

        let edge_on_axis = check_edge_on_axis(
            k,
            self.base_pt,
            self.axis,
            b,
            e,
            is_line,
            smos_max(self.eps, original_edge_tol),
        )?;

        let mut new_edge_curve = original_edge_curve.copy(k)?;
        let mut trim_ivl = edge::interval(b, e)?;
        new_edge_curve.trim(k, &mut trim_ivl, TrimMode::Notify)?;

        if edge_on_axis || self.angle == 360.0 {
            return Ok((new_edge_curve, smos_max(self.eps, original_edge_tol)));
        }

        let angle_rad = self.angle * PI / 180.0;

        let mut ref_frame = Axis2Placement::default();
        ref_frame.rotate_about_axis_at_point(angle_rad, self.base_pt, self.axis);
        new_edge_curve.transform(k, &ref_frame, None)?;

        Ok((new_edge_curve, original_edge_tol))
    }

    /// `FaceSweepSame`: face `f`'s surface (copied as an analytic surface where
    /// possible) rotated by the sweep angle, and the face tolerance; `None` for a full
    /// turn.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a face without surface; those of the copy and
    /// transform.
    pub fn face_sweep_same(
        &self,
        k: &Kernel,
        b: &Brep,
        f: FaceId,
    ) -> Result<Option<(Surface, f64)>> {
        if (self.angle - 360.0).abs() < EFF_ZERO {
            return Ok(None); // Nothing required for this case
        }

        let orig_face_tol = face::p2::tolerance(b, f)?;

        let original_face_surface =
            face::p2::surface(b, f)?.ok_or_else(|| SolidError::null_pointer(OP_FACE_SAME))?;

        // Copy pOriginalFaceSurface, when possible as an analytic surface
        let mut new_face_surf = original_face_surface.copy_and_add_analytics(k)?;

        let angle_rad = self.angle * PI / 180.0;

        let mut ref_frame = Axis2Placement::default();
        ref_frame.rotate_about_axis_at_point(angle_rad, self.base_pt, self.axis);
        new_face_surf.transform(k, &ref_frame, None)?;

        Ok(Some((new_face_surf, orig_face_tol)))
    }
}

/// `SM_REVERSE_ORIENTATION(a)`: `SM_OT_OPPOSITE` for `SM_OT_SAME`, else `SM_OT_SAME`.
const fn reverse_orientation(a: OrientType) -> OrientType {
    if matches!(a, OrientType::Same) {
        OrientType::Opposite
    } else {
        OrientType::Same
    }
}

/// `RemoveAt(0)` on the output arrays (the UV array only when it is filled).
fn remove_first(
    r3d: &mut Vec<(Role, Curve)>,
    r_uv: &mut Vec<Line>,
    r_orients: &mut Vec<OrientType>,
    keep_uv: bool,
) {
    if !r3d.is_empty() {
        r3d.remove(0);
    }
    if keep_uv && !r_uv.is_empty() {
        r_uv.remove(0);
    }
    if !r_orients.is_empty() {
        r_orients.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::{
        Edge, Edgeuse, EdgeuseId, EdgeuseOwner, Region, Shell, Vertex, Vertexuse, VertexuseOwner,
    };

    /// A wire edge on the segment `a` -> `b` (parameters `[0, 1]`), as
    /// `MakeShellVertex` + `MakeWireEdgeVertex` link it.
    fn wire(a: Point3, c: Point3) -> (Brep, EdgeId) {
        let line = Line::create_line_segment(3, a, c, None).unwrap();
        wire_on(Curve::from(line), a, c, Extent1d::from_min_max(0.0, 1.0))
    }

    /// A wire edge from `a` to `c` on `curve` over `interval`.
    fn wire_on(curve: Curve, a: Point3, c: Point3, interval: Extent1d) -> (Brep, EdgeId) {
        let mut br = Brep::new();
        let r = br
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let s = br
            .add_shell_record(Shell {
                faces: Vec::new(),
                region: Some(r),
                ..Shell::default()
            })
            .unwrap();
        let v0 = br
            .add_vertex_record(Vertex {
                point: a,
                tolerance: 1e-6,
                ..Vertex::default()
            })
            .unwrap();
        let v1 = br
            .add_vertex_record(Vertex {
                point: c,
                tolerance: 1e-6,
                ..Vertex::default()
            })
            .unwrap();
        let e = br
            .add_edge_record(Edge {
                vertices: [v0, v1],
                edgeuses: vec![EdgeuseId::from_index(0), EdgeuseId::from_index(1)],
                interval,
                tolerance: 1e-6,
                curve: Some(curve),
            })
            .unwrap();
        for (i, v) in [v0, v1].into_iter().enumerate() {
            let eu = EdgeuseId::from_index(i as u32);
            let vu = br
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(v),
                    owner: VertexuseOwner::Edgeuse(Some(eu)),
                    ..Vertexuse::default()
                })
                .unwrap();
            br.add_edgeuse_record(Edgeuse {
                edge: Some(e),
                owner: EdgeuseOwner::Shell(Some(s)),
                vertexuse: Some(vu),
                orientation: if i == 0 {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                },
                ..Edgeuse::default()
            })
            .unwrap();
        }
        (br, e)
    }

    fn z_sweep(angle: f64) -> RotationalSweepGeometry {
        let mut g = RotationalSweepGeometry::new(Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0), angle, 1e-6);
        g.base.set_make_analytics(false);
        g
    }

    #[test]
    fn constructor_unitizes_axis_and_copies() {
        let g = RotationalSweepGeometry::new(Vec3::X, Vec3::new(0.0, 3.0, 0.0), 90.0, 1e-3);
        assert_eq!(g.axis(), Vec3::Y);
        let z = RotationalSweepGeometry::new(Vec3::X, Vec3::ZERO, 90.0, 1e-3);
        assert_eq!(z.axis(), Vec3::Z);
        let mut h = g;
        h.base.set_make_analytics(false);
        let c = h.copy();
        assert_eq!(c, h);
    }

    #[test]
    fn sweep_closed() {
        let g = z_sweep(90.0);
        assert!(!g.is_sweep_closed(1).unwrap());
        assert!(g.is_sweep_closed(4).unwrap());
        assert!(g.is_sweep_closed(5).is_err());
        assert!(z_sweep(360.0).is_sweep_closed(1).unwrap());
    }

    #[test]
    fn coord_systems() {
        let g = z_sweep(90.0);
        assert!(g.start_coord_system(Vec3::new(0.0, 0.0, 5.0)).is_err());
        let cs = g.start_coord_system(Vec3::new(2.0, 0.0, 1.0)).unwrap();
        assert_eq!(cs.origin(), Vec3::new(2.0, 0.0, 1.0));
        let end = g.end_coord_system(Vec3::new(2.0, 0.0, 1.0)).unwrap();
        assert!((end.origin() - Vec3::new(0.0, 2.0, 1.0)).length() < 1e-12);
        let t = g.sweep_transform();
        assert!((t.transform_point(Vec3::X) - Vec3::Y).length() < 1e-12);
        let full = z_sweep(360.0).sweep_transform();
        assert_eq!(full.transform_point(Vec3::X), Vec3::X);
    }

    #[test]
    fn vertex_sweeps() {
        let k = &Kernel::default();
        let (b, e) = wire(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let g = z_sweep(90.0);
        let [v0, v1] = b.edge(e).unwrap().vertices;
        let (arc, tol) = g.vertex_sweep_higher(&b, v0).unwrap().unwrap();
        assert_eq!(tol, 1e-6);
        let end = arc.evaluate_point(k, arc.natural_interval().max()).unwrap();
        assert!((end - Vec3::Y).length() < 1e-12);
        assert!(g.vertex_sweep_higher(&b, v1).unwrap().is_none());
        let (p, _) = g.vertex_sweep_same(&b, v0).unwrap();
        assert!((p - Vec3::Y).length() < 1e-12);
        // a vertex on the axis does not move
        assert!(g.vertex_sweep_same(&b, v1).is_err());
    }

    #[test]
    fn edge_sweeps() {
        let k = Kernel::default();
        // a slanted line: a cone
        let (b, e) = wire(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 1.0));
        let g = z_sweep(90.0);
        let s = g.edge_sweep_higher(&k, &b, e).unwrap();
        assert!(s.surface.is_some());
        assert_eq!(s.trim_curves.len(), 4);
        assert_eq!(s.uv_trim_curves.len(), 4);
        assert_eq!(s.start_curve, Some(0));
        assert!(s.normal.is_some());
        let (c, tol) = g.edge_sweep_same(&k, &b, e).unwrap();
        assert_eq!(tol, 1e-6);
        let p = c.evaluate_point(&k, c.natural_interval().min()).unwrap();
        assert!((p - Vec3::Y).length() < 1e-12);

        // a line perpendicular to the axis from the axis, full turn: a disc
        let (b, e) = wire(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0));
        let s = z_sweep(360.0).edge_sweep_higher(&k, &b, e).unwrap();
        assert!(s.uv_trim_curves.is_empty());
        assert_eq!(s.trim_curves.len(), 1);
        assert_eq!(s.start_curve, None);

        // a line on the axis
        let (b, e) = wire(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0));
        let s = g.edge_sweep_higher(&k, &b, e).unwrap();
        assert!(s.surface.is_none());
        assert_eq!(s.face_tol, -1.0);
    }

    #[test]
    fn analytic_revolutions_keep_the_b_spline_loop() {
        use crate::surface::surface::SurfaceKind;
        let k = Kernel::default();
        let arc = |centre: Point3, x: Vec3, y: Vec3, r: f64, start: f64, end: f64| {
            let c = BSplineCurve::create_circle_segment(
                3,
                &Axis2Placement::new(centre, x, y),
                r,
                start,
                end,
                NurbCircleParam::Quadratic,
            )
            .unwrap();
            let ivl = c.natural_interval();
            let (a, b) = (
                c.evaluate_point(ivl.min()).unwrap(),
                c.evaluate_point(ivl.max()).unwrap(),
            );
            wire_on(Curve::from(c), a, b, ivl)
        };
        let cases = [
            (
                "cylinder",
                wire(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 3.0)),
            ),
            (
                "cone",
                wire(Vec3::new(2.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0)),
            ),
            ("drill point", wire(Vec3::new(1.0, 0.0, 1.0), Vec3::ZERO)),
            (
                "sphere",
                arc(Vec3::ZERO, -Vec3::Z, Vec3::X, 3.0, 0.0, 180.0),
            ),
            ("dome", arc(Vec3::ZERO, Vec3::X, Vec3::Z, 2.0, 0.0, 90.0)),
            (
                "torus",
                arc(Vec3::new(5.0, 0.0, 0.0), Vec3::X, Vec3::Z, 1.5, 0.0, 360.0),
            ),
            // a full minor circle from the bottom (the partial torus's)
            (
                "minor circle from the bottom",
                arc(Vec3::new(5.0, 0.0, 0.0), -Vec3::Z, Vec3::X, 1.5, 0.0, 360.0),
            ),
            (
                "fillet",
                arc(Vec3::new(4.0, 0.0, 1.0), Vec3::Z, Vec3::X, 1.0, 0.0, 90.0),
            ),
            // a tube range across 0 degrees (-60..30 from X towards Z)
            (
                "fillet",
                arc(Vec3::new(4.0, 0.0, 1.0), Vec3::X, Vec3::Z, 1.0, -60.0, 30.0),
            ),
        ];
        for (name, (b, e)) in cases {
            let upstream = RotationalSweepGeometry::new(Vec3::ZERO, Vec3::Z, 90.0, 1e-6);
            assert!(!upstream.analytic_revolutions());
            let mut g = upstream.copy();
            g.set_analytic_revolutions(true);
            assert!(g.copy().analytic_revolutions());
            let old = upstream.edge_sweep_higher(&k, &b, e).unwrap();
            let new = g.edge_sweep_higher(&k, &b, e).unwrap();
            let surface = new.surface.as_ref().unwrap();
            let analytic = match surface.kind() {
                SurfaceKind::Cone(c) => {
                    if c.is_cylinder_class() {
                        "cylinder"
                    } else {
                        "cone"
                    }
                }
                SurfaceKind::Sphere(_) => "sphere",
                SurfaceKind::Torus(_) => "torus",
                _ => "other",
            };
            let expected = match name {
                "dome" => "sphere",
                "drill point" => "cone",
                "fillet" | "minor circle from the bottom" => "torus",
                n => n,
            };
            assert_eq!(analytic, expected, "{name}");
            let (n_new, n_old) = (new.normal.unwrap(), old.normal.unwrap());
            // #1463, #1490: never inside out (fork 9853e54)
            let inside_out = match surface.kind() {
                SurfaceKind::Cone(c) => c.inside_out(),
                SurfaceKind::Sphere(s) => s.inside_out(),
                SurfaceKind::Torus(t) => t.inside_out(),
                _ => false,
            };
            assert!(!inside_out, "{name}");
            if (n_new - n_old).length() >= 1e-12 {
                // a curve falling along the axis or round its circle is laid out as the
                // reversed curve's B-spline: the loop runs the other way round, its
                // normal the analytic surface's at the curve's other end
                assert!(
                    matches!(
                        name,
                        "cone" | "drill point" | "fillet" | "minor circle from the bottom"
                    ),
                    "{name}: {n_new:?} {n_old:?}"
                );
                assert_eq!(new.trim_curves.len(), old.trim_curves.len(), "{name}");
                assert_eq!(new.face_tol, old.face_tol, "{name}");
                continue;
            }
            // the same loop: roles, orientations, tolerances and the normal (the
            // analytic surface's, to rounding)
            assert!(
                (n_new - n_old).length() < 1e-12,
                "{name}: {n_new:?} {n_old:?}"
            );
            assert_eq!(new.trim_curves.len(), old.trim_curves.len(), "{name}");
            assert_eq!(new.uv_trim_curves.len(), old.uv_trim_curves.len(), "{name}");
            assert_eq!(new.orients, old.orients, "{name}");
            assert_eq!(
                (
                    new.start_curve,
                    new.far_end_curve,
                    new.start_vert_curve,
                    new.end_vert_curve
                ),
                (
                    old.start_curve,
                    old.far_end_curve,
                    old.start_vert_curve,
                    old.end_vert_curve
                ),
                "{name}"
            );
            assert_eq!(new.face_tol, old.face_tol, "{name}");
            // each 3D trim has the B-spline trim's ends and lies on the new surface
            for (i, (c, d)) in new.trim_curves.iter().zip(&old.trim_curves).enumerate() {
                let (ci, di) = (c.natural_interval(), d.natural_interval());
                // (a closed trim collapsed to a point still has matching ends)
                let lc = c.approximate_length(&k, &ci, 16, false).unwrap().0;
                let ld = d.approximate_length(&k, &di, 16, false).unwrap().0;
                assert!(
                    (lc - ld).abs() <= 1e-3 * ld,
                    "{name} trim {i}: length {lc} {ld}"
                );
                assert!(!c.is_degenerate(&k, 1e-6, None).unwrap(), "{name} trim {i}");
                for (t, s) in [(ci.min(), di.min()), (ci.max(), di.max())] {
                    let p = c.evaluate_point(&k, t).unwrap();
                    let q = d.evaluate_point(&k, s).unwrap();
                    assert!((p - q).length() < 1e-9, "{name} trim {i}: {p:?} {q:?}");
                }
            }
            for (c, uv) in new.trim_curves.iter().zip(&new.uv_trim_curves) {
                let (ci, ui) = (c.natural_interval(), uv.natural_interval());
                for f in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let p = c.evaluate_point(&k, ci.evaluate(f)).unwrap();
                    let w = uv.evaluate_point(ui.evaluate(f));
                    let q = surface
                        .evaluate_point(&k, crate::geom::vector2d::Point2::new(w.x, w.y))
                        .unwrap();
                    assert!(
                        (p - q).length() < 1e-9,
                        "{name} uv trim at {f}: {p:?} {q:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rot_on_curve() {
        let k = Kernel::default();
        let g = z_sweep(90.0);
        let line = Curve::from(
            Line::create_line_segment(3, Vec3::new(-1.0, 0.0, 0.0), Vec3::X, None).unwrap(),
        );
        let r = g
            .check_curve_sweep(
                &k,
                &line,
                &Extent1d::from_min_max(0.0, 1.0),
                1e-6,
                Vec3::ZERO,
                Vec3::Z,
                90.0,
            )
            .unwrap();
        assert_eq!(r, SweepCheckType::RotOnCurve);
        let (b, e) = wire(Vec3::new(-1.0, 0.0, 0.0), Vec3::X);
        assert!(g.edge_sweep_higher(&k, &b, e).is_err());
    }
}
