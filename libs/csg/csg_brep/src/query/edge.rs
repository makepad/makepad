//! Edge queries (`SmEdge`): the bounding boxes of an edge, the closest point of an edge
//! to a point, and the radial sector classifications that order faces and curves
//! around an edge.
//!
//! Upstream: `source/SMLib/src/SmEdge.cpp`, `source/SMLib/inc/SmEdge.h`.
//!
//! | Rust                                  | Upstream                                   |
//! |---------------------------------------|--------------------------------------------|
//! | [`calculate_bounding_box`]            | `SmEdge::CalculateBoundingBox`             |
//! | [`calculate_tight_bounding_box`]      | `SmEdge::CalculateTightBoundingBox`        |
//! | [`closest_point`], [`ClosestPoint`]   | `SmEdge::ClosestPoint`                     |
//! | [`radial_sector_classify`], [`radial_sector_classify_between`], [`RadialSectorClass`] | `SmEdge::RadialSectorClassify` |
//! | [`radial_sector_of_curve_classify`], [`CurveSectorClass`] | `SmEdge::RadialSectorOfCurveClassify` |
//! | [`edge_vertices`]                     | `SmEdge::GetVertices(SmTArray&, ULONG*)` (CO-PORTED, `topology.edge`) |
//! | [`edge_faces`]                        | `SmEdge::GetFaces` (CO-PORTED, `topology.edge`) |
//! | `evaluate_binormal_step_off` (private)| `SmEdgeuse::EvaluateBinormalStepOff` (`topology.edgeuse_p2`, [`evaluate_binormal_step_off_into`]) |
//!
//! Deviations:
//! - **Object model.** The methods are free functions taking the [`Brep`] and the
//!   edge's id; edge uses are ids. `GetTolerance` is the edge's stored tolerance
//!   (`SM_USE_OLDTOL`), `GetCurve` / `GetInterval` its [`crate::topology::Edge`]
//!   fields. Where upstream dereferences a null pointer (an edge without curve in
//!   [`closest_point`] and the radial classifications, a missing radial partner,
//!   face, surface, vertex use or mate) the port returns
//!   [`SolidError::InvalidInput`]; `NER` returns `SM_ERR_NULL_POINTER`
//!   ([`SolidError::null_pointer`]).
//! - **Out-params.** The boxes of [`calculate_bounding_box`] are asked for with a
//!   [`BoundingBoxRequest`] and returned (upstream: caller-owned boxes, which are
//!   default-constructed here, so `Init` / `ReSet` of an edge without curve give the
//!   default boxes). [`calculate_tight_bounding_box`] returns `None` where upstream
//!   leaves the caller's box untouched (an edge without curve). A failing status is an
//!   error; the tight branch of [`calculate_bounding_box`] reports any failure of its
//!   two calls as `SM_ERR`, as upstream ([`SolidError::Unported`] propagates).
//! - **Same curve.** `RadialSectorClassify` compares the two edges' curve pointers; the
//!   port compares the edges (an edge owns its curve, so only an edge shares its own).
//! - **Step-offs.** `SmEdgeuse::EvaluateBinormalStepOff` (unit `topology.edgeuse_p2`)
//!   is called with the B-rep mutably (it may attach UV trim curves, as upstream's
//!   `const_cast`s): both radial classifications take `&mut Brep`. Its out-params keep
//!   the values a failing call leaves ([`evaluate_binormal_step_off_into`]), as
//!   `RadialSectorClassify` reads them after a failed step-off; the step-off vectors
//!   start at `SM_UNDEF_DOUBLE`, as upstream's default-constructed `SmVector3d`s.
//! - `pInsideFaceuses` of `RadialSectorClassify` is unused upstream and dropped; the
//!   optional attribute filters of the co-ported `GetVertices` / `GetFaces` are not
//!   ported (no caller here passes one).
//! - **Loops.** The step-off loops of both classifications keep upstream's bound (20
//!   rounds of growing step-offs). No recursion.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::euclidian_step_off;
use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BoundingBoxRequest, BoundingBoxes};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p2::PLANE_TYPE;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p2::{evaluate_binormal_step_off_into, BinormalEval, StepOffOptions};
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, VertexId};

const OP_BBOX: &str = "query::edge::calculate_bounding_box";
const OP_TIGHT: &str = "query::edge::calculate_tight_bounding_box";
const OP_CLOSEST: &str = "query::edge::closest_point";
const OP_RADIAL: &str = "query::edge::radial_sector_classify";
const OP_RADIAL_CURVE: &str = "query::edge::radial_sector_of_curve_classify";
const OP_VERTICES: &str = "query::edge::edge_vertices";

/// Most step-off rounds of the radial classifications (`lCount < 20`).
const MAX_STEP_OFF_ROUNDS: u32 = 20;

/// A null pointer upstream dereferences.
fn null(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(operation, reason)
}

/// The edge's curve (`GetCurve`), which upstream dereferences.
fn curve_of<'b>(brep: &'b Brep, edge: EdgeId, operation: &'static str) -> Result<&'b Curve> {
    brep.edge(edge)?
        .curve
        .as_ref()
        .ok_or_else(|| null(operation, "edge without curve"))
}

/// The handle naming an edge's curve (`id` 0) or its cache's analytic replacement
/// (`id` 1) in point solutions.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle {
    id: u8,
    ivl: Extent1d,
}

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.ivl)
    }
}

// ---- co-ported from topology.edge -------------------------------------------------------

/// `SmEdge::GetVertices` (array form, CO-PORTED from unit `topology.edge`): the start
/// vertex, then the end vertex when it differs; empty without primary edge use. The
/// start is the primary edge use's vertex when it runs with the curve
/// ([`OrientType::Same`]), else its mate's.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
/// missing vertex use, mate or vertex (upstream dereferences or returns it).
pub fn edge_vertices(brep: &Brep, edge: EdgeId) -> Result<Vec<VertexId>> {
    let mut out = Vec::new();
    let Some(prim) = eu1::primary_edgeuse(brep, edge)? else {
        return Ok(out);
    };
    let vertex_of = |eu: EdgeuseId| -> Result<Option<VertexId>> {
        let vu = eu1::vertexuse(brep, eu)?
            .ok_or_else(|| null(OP_VERTICES, "edge use without vertex use"))?;
        Ok(brep.vertexuse(vu)?.vertex)
    };
    let mate = eu1::mate(brep, prim)?.ok_or_else(|| null(OP_VERTICES, "edge use without mate"))?;
    let (v1, v2) = if eu1::orientation(brep, prim)? == OrientType::Same {
        (vertex_of(prim)?, vertex_of(mate)?)
    } else {
        (vertex_of(mate)?, vertex_of(prim)?)
    };
    // upstream adds a NULL start vertex, which every caller dereferences
    let v1 = v1.ok_or_else(|| null(OP_VERTICES, "vertex use without vertex"))?;
    out.push(v1);
    if let Some(v2) = v2 {
        if v2 != v1 {
            out.push(v2);
        }
    }
    Ok(out)
}

/// `SmEdge::GetFaces` (CO-PORTED from unit `topology.edge`): the distinct faces of the
/// edge's loop edge uses, in edge use list order (a loop edge use without face is
/// skipped).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`eu1::face`].
pub fn edge_faces(brep: &Brep, edge: EdgeId) -> Result<Vec<FaceId>> {
    let mut faces: Vec<FaceId> = Vec::new();
    for &eu in &brep.edge(edge)?.edgeuses {
        if eu1::is_loop_edgeuse(brep, eu)? {
            let Some(face) = eu1::face(brep, eu)? else {
                continue;
            };
            if !faces.contains(&face) {
                faces.push(face);
            }
        }
    }
    Ok(faces)
}

// ---- bounding boxes ---------------------------------------------------------------------

/// `SmEdge::CalculateBoundingBox`: the asked-for boxes of the edge's curve over the
/// edge's interval. With `tight` the axis aligned box is
/// [`calculate_tight_bounding_box`] and the other boxes come from the curve; otherwise
/// all come from [`Curve::calculate_bounding_box`] (with its default box expansion).
/// An edge without curve gives the initialised boxes.
///
/// # Errors
///
/// Those of [`Curve::calculate_bounding_box`]; with `tight`, `SM_ERR` when either call
/// fails; [`SolidError::Unported`] from unported callees.
pub fn calculate_bounding_box(
    k: &Kernel,
    brep: &Brep,
    edge: EdgeId,
    req: BoundingBoxRequest,
    tight: bool,
) -> Result<BoundingBoxes> {
    // init the asked-for boxes (the pseudo box keeps its basis, the polar box its
    // basis and domain)
    let mut out = BoundingBoxes {
        normal: req.normal.then(Extent3d::new),
        pseudo: req.pseudo.then(|| {
            let mut b = PseudoBox::default();
            b.init();
            b
        }),
        polar: req.polar.then(|| {
            let mut b = PolarBox::default();
            b.reset();
            b
        }),
    };

    // no work - no curve
    let e = brep.edge(edge)?;
    let Some(curve) = e.curve.as_ref() else {
        return Ok(out);
    };

    // when asked for a tight Axis Aligned Box
    if tight {
        let mut ok1 = true;
        let mut ok2 = true;

        if req.normal {
            match calculate_tight_bounding_box(k, brep, edge) {
                Ok(Some(b)) => out.normal = Some(b),
                Ok(None) => {}
                Err(err) if err.is_unported() => return Err(err),
                Err(_) => ok1 = false,
            }
        }

        if req.pseudo || req.polar {
            let sub = BoundingBoxRequest {
                normal: false,
                pseudo: req.pseudo,
                polar: req.polar,
            };
            match curve.calculate_bounding_box(k, &e.interval, sub, true) {
                Ok(b) => {
                    out.pseudo = b.pseudo;
                    out.polar = b.polar;
                }
                Err(err) if err.is_unported() => return Err(err),
                Err(_) => ok2 = false,
            }
        }

        // all done
        if ok1 && ok2 {
            return Ok(out);
        }
        return Err(SolidError::sm_err(OP_BBOX));
    }

    // get all the bounding boxes from the curve
    curve.calculate_bounding_box(k, &e.interval, req, true)
}

/// `SmEdge::CalculateTightBoundingBox`: the box of the edge's vertices, grown by the
/// curve's tight box over the edge's interval unless the curve's loose box already
/// lies inside it (within `SM_EFF_ZERO * (1 + its largest dimension)`). `None` for an
/// edge without curve (upstream returns without setting the box).
///
/// # Errors
///
/// Those of [`edge_vertices`], [`Curve::calculate_bounding_box`] and
/// [`Curve::calculate_tight_bounding_box`].
pub fn calculate_tight_bounding_box(
    k: &Kernel,
    brep: &Brep,
    edge: EdgeId,
) -> Result<Option<Extent3d>> {
    // Method: get the BBox of the vertices.
    // Then get the loose BBox of the curve.
    // If it's within the BBox so far (from the vertices),
    // then we're done,
    // otherwise calculate the tight BBox of the curve and union it in.
    let mut edge_bbox = Extent3d::new();
    for v in edge_vertices(brep, edge)? {
        edge_bbox.add_point(brep.vertex(v)?.point);
    }

    // Now our curve.
    let e = brep.edge(edge)?;
    let Some(curve) = e.curve.as_ref() else {
        return Ok(None);
    };

    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    let curve_bbox = curve
        .calculate_bounding_box(k, &e.interval, req, true)?
        .normal
        .ok_or_else(|| SolidError::sm_err(OP_TIGHT))?;

    let tol = EFF_ZERO * (1.0 + curve_bbox.max_dimension());

    if !curve_bbox.is_contained_by(&edge_bbox, tol) {
        // Do the tight BBox computation.
        let curve_bbox = curve.calculate_tight_bounding_box(k, &e.interval)?;

        edge_bbox = edge_bbox.union(&curve_bbox);
    }

    Ok(Some(edge_bbox))
}

// ---- closest point ----------------------------------------------------------------------

/// The result of [`closest_point`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPoint {
    /// The curve parameter of the closest point (`rdParameter`).
    pub parameter: f64,
    /// Its distance from the point (`rdDistance`).
    pub distance: f64,
}

/// `SmEdge::ClosestPoint`: the first solution of the curve's global minimising point
/// solve over the edge's interval (all solutions requested, the edge tolerance as the
/// distance tolerance); `None` when there is none.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge without curve (upstream dereferences it);
/// those of [`Curve::global_point_solve`].
pub fn closest_point(
    k: &Kernel,
    brep: &Brep,
    edge: EdgeId,
    point: Point3,
) -> Result<Option<ClosestPoint>> {
    let curve = curve_of(brep, edge, OP_CLOSEST)?;
    let e = brep.edge(edge)?;
    let ivl = curve.natural_interval();
    let opts = GlobalPointSolveOptions {
        handle: Some(CurveHandle { id: 0, ivl }),
        analytic_handle: Some(CurveHandle { id: 1, ivl }),
        ..GlobalPointSolveOptions::default()
    };

    // global point solve
    let solutions = curve.global_point_solve(
        k,
        &e.interval,
        SolverOperation::Minimize,
        point,
        e.tolerance,
        SolutionRequested::All,
        &opts,
    )?;

    // no solutions
    let Some(sol) = solutions.get(0) else {
        return Ok(None);
    };

    // found a closest point
    Ok(Some(ClosestPoint {
        parameter: sol.start.param(0)?,
        distance: sol.start.solution_value,
    }))
}

// ---- radial sectors ---------------------------------------------------------------------

/// Where [`radial_sector_classify`] puts a face (`rlResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadialSectorClass {
    /// `0`: outside the sector.
    Outside,
    /// `1`: inside the sector.
    Inside,
    /// `2`: coincident with the sector edge use's face.
    CoincidentSector,
    /// `3`: coincident with the sector edge use's mate face.
    CoincidentSectorMate,
    /// `4`: coincident with the radial edge use's face.
    CoincidentRadial,
    /// `5`: coincident with the radial edge use's mate face.
    CoincidentRadialMate,
    /// `6`: zero sector, in the sector edge use's face.
    ZeroSector,
    /// `7`: zero sector, in the sector edge use's mate face.
    ZeroSectorMate,
}

impl RadialSectorClass {
    /// Upstream's `rlResult` value.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Outside => 0,
            Self::Inside => 1,
            Self::CoincidentSector => 2,
            Self::CoincidentSectorMate => 3,
            Self::CoincidentRadial => 4,
            Self::CoincidentRadialMate => 5,
            Self::ZeroSector => 6,
            Self::ZeroSectorMate => 7,
        }
    }
}

/// Where [`radial_sector_of_curve_classify`] puts a curve (`rlResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveSectorClass {
    /// `0`: outside the sector.
    Outside,
    /// `1`: inside the sector.
    Inside,
    /// `2`: coincident with the sector edge use's face.
    CoincidentSector,
    /// `3`: coincident with the radial edge use's face.
    CoincidentRadial,
}

impl CurveSectorClass {
    /// Upstream's `rlResult` value.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Outside => 0,
            Self::Inside => 1,
            Self::CoincidentSector => 2,
            Self::CoincidentRadial => 3,
        }
    }
}

/// The out-params of `SmEdgeuse::EvaluateBinormalStepOff` the classifications read:
/// the step-off point, the binormal and the edge use tangent.
#[derive(Debug, Clone, Copy, PartialEq)]
struct StepOffVectors {
    point: Point3,
    binormal: Vec3,
    tangent: Vec3,
}

/// Upstream's default-constructed `SmVector3d` / `SmPoint3d` locals (`SM_UNDEF_DOUBLE`).
impl Default for StepOffVectors {
    fn default() -> Self {
        Self {
            point: Point3::UNDEF,
            binormal: Vec3::UNDEF,
            tangent: Vec3::UNDEF,
        }
    }
}

/// `SmEdgeuse::EvaluateBinormalStepOff(param, dist, point, binormal, &tangent, NULL,
/// center, plane_normal)`: writes the out-params in place (a failing call leaves what it
/// wrote, [`evaluate_binormal_step_off_into`]) and returns its status. The face normal
/// and UV outputs are not asked for (`NULL`); `bOKToMakeUVTrimCurve` is its default
/// `TRUE`.
fn evaluate_binormal_step_off(
    k: &Kernel,
    brep: &mut Brep,
    eu: EdgeuseId,
    param: f64,
    step_off_distance: f64,
    out: &mut StepOffVectors,
    center: Option<Point3>,
    plane_normal: Option<Vec3>,
) -> Result<()> {
    let mut eval = BinormalEval {
        point: out.point,
        binormal: out.binormal,
        tangent: out.tangent,
        face_normal: Vec3::UNDEF,
        uv: crate::math::Point2::UNDEF,
    };
    let options = StepOffOptions {
        center_point: center,
        plane_normal,
        ok_to_make_uv_trim_curve: true,
    };
    let r =
        evaluate_binormal_step_off_into(k, brep, eu, param, step_off_distance, &options, &mut eval);
    out.point = eval.point;
    out.binormal = eval.binormal;
    out.tangent = eval.tangent;
    r
}

/// The status of a vector operation upstream checks with `SER` (`Unitize`,
/// `AngleBetween` of a zero-length vector): both return `SM_ERR_INVALID_INPUT` as a
/// status (not a null dereference), which `SER` passes on unchanged.
fn vec_status<T>(r: Result<T>, operation: &'static str) -> Result<T> {
    r.map_err(|e| match e {
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => {
            SolidError::Upstream {
                operation,
                code: SM_ERR_INVALID_INPUT,
                entity: None,
            }
        }
        e => e,
    })
}

/// Runs a step-off whose failure upstream records instead of returning: `Ok(true)` when
/// it failed (stepped off the surface); [`SolidError::Unported`] propagates.
fn stepped_off(r: Result<()>) -> Result<bool> {
    match r {
        Ok(()) => Ok(false),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(true),
    }
}

/// `x * (SM_PI / 180.0)` (`SM_DEG2RAD`).
fn deg2rad(x: f64) -> f64 {
    x * (PI / 180.0)
}

/// The face surface of a loop edge use is a plane (`GetFace()->GetSurface()->
/// IsKindOf(SmPlane_TYPE)`, both pointers dereferenced).
fn on_plane(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    let face = eu1::face(brep, eu)?.ok_or_else(|| null(OP_RADIAL, "edge use without face"))?;
    brep.face(face)?
        .surface
        .as_ref()
        .ok_or_else(|| null(OP_RADIAL, "face without surface"))?
        .is_kind_of(PLANE_TYPE)
}

/// `SmEdge::RadialSectorClassify`: where the face of `classify_eu` lies relative to
/// the sector between the face of `sector_eu` and the face of its radial partner, at
/// the edge's normalised parameter `normalized_param`. The binormals of the three
/// faces (from `SmEdgeuse::EvaluateBinormalStepOff`) are compared at growing step-off
/// distances until the sector test is unambiguous, a coincidence persists, or a
/// step-off leaves a face. `classify_eu`'s edge parameter is the same when it is this
/// edge, else the drop of this edge's point onto its curve.
///
/// # Errors
///
/// `SM_ERR` when the point cannot be dropped onto the other edge or the first step-off
/// leaves a face; `SM_ERR_INVALID_INPUT` (as [`SolidError::Upstream`]) when the first
/// step-off's tangent or a binormal to measure is zero-length (a failed first step-off
/// leaves the tangent uninitialised, which `Unitize` rejects before the step-off
/// failure is looked at); [`SolidError::InvalidInput`] where upstream dereferences a null
/// pointer; `SM_ERR_NULL_POINTER` for an edge use without edge or an edge without
/// curve to classify; those of the evaluations; [`SolidError::Unported`] for the
/// step-offs' deferred callees (see [`crate::topology::edgeuse::p3`]).
pub fn radial_sector_classify(
    k: &Kernel,
    brep: &mut Brep,
    edge: EdgeId,
    normalized_param: f64,
    sector_eu: EdgeuseId,
    classify_eu: EdgeuseId,
) -> Result<RadialSectorClass> {
    radial_sector_classify_between(
        k,
        brep,
        edge,
        normalized_param,
        sector_eu,
        None,
        classify_eu,
    )
}

/// `RadialSectorClassify` with the classified edge use in a second B-rep
/// (`other_brep`; `None` is `brep` itself, as [`radial_sector_classify`]).
/// Equal arena ids in different operands do not identify the same curve. Both
/// B-reps are mutable because the step-offs may create UV trim curves.
///
/// # Errors
///
/// As [`radial_sector_classify`].
pub fn radial_sector_classify_between(
    k: &Kernel,
    brep: &mut Brep,
    edge: EdgeId,
    normalized_param: f64,
    sector_eu: EdgeuseId,
    mut other_brep: Option<&mut Brep>,
    classify_eu: EdgeuseId,
) -> Result<RadialSectorClass> {
    // locals
    let eu_radial =
        eu1::radial(brep, sector_eu)?.ok_or_else(|| null(OP_RADIAL, "edge use without radial"))?;
    let this = brep.edge(edge)?;
    let lamina_edge = eu1::edge_is_lamina(brep, edge)?;

    // get edge's position for given param
    let this_param = this.interval.evaluate(normalized_param);
    let start_point = curve_of(brep, edge, OP_RADIAL)?.evaluate_point(k, this_param)?;

    // classify edge locals
    let same_brep = other_brep.is_none();
    let other_ref: &Brep = other_brep.as_deref().unwrap_or(&*brep);
    let other_edge = other_ref
        .edgeuse(classify_eu)?
        .edge
        .ok_or(SolidError::null_pointer(OP_RADIAL))?;
    let other = other_ref.edge(other_edge)?;
    let other_curve = other
        .curve
        .as_ref()
        .ok_or(SolidError::null_pointer(OP_RADIAL))?;
    let tolerance = other.tolerance + this.tolerance;

    // get the corresponding classify edge parameter value
    // 1. when the curves are the same - reuse param value
    // 2. when curves differ - drop point to curve (over a sequence of larger seek areas)
    let other_param = if same_brep && other_edge == edge {
        this_param
    } else {
        let mut success = false;
        let mut param = 0.0;
        let mut scale = 1.0;
        // try a sequence of drop distances
        while !success && scale < 1_000_000.0 {
            let d = other_curve.drop_point(
                k,
                &other.interval,
                start_point,
                None,
                tolerance * scale,
                None,
                SolverOperation::Minimize,
            )?;
            success = d.success;
            param = d.parameter;
            scale *= 10.0;
        }
        if !success {
            // Unable to drop point to other curve.
            return Err(SolidError::sm_err(OP_RADIAL));
        }
        param
    };

    // locals for geometric classification
    let mut done = false;
    let mut inside = false;
    let mut step_off_distance = 0.0;
    let mut count: u32 = 0;
    let mut min_dist;
    let mut last_min = 0.0;
    let mut last2_min = 0.0;
    let mut ang1 = 0.0;
    let mut ang2 = 0.0;
    let mut ang3 = 0.0;
    let mut last_step_off_distance = 0.0;

    // detect special case: all planar surfaces
    let all_planes = on_plane(brep, sector_eu)?
        && on_plane(brep, eu_radial)?
        && on_plane(other_brep.as_deref().unwrap_or(&*brep), classify_eu)?;

    let mut num_consecutive_coincidences: u32 = 0;

    // the step-off vectors (Bin = binormal)
    let mut s1 = StepOffVectors::default();
    let mut s2 = StepOffVectors::default();
    let mut s3 = StepOffVectors::default();

    while !done {
        // 1. get Edgeuse->Face stepoff binormal
        let mut stepped_off_surface = stepped_off(evaluate_binormal_step_off(
            k,
            brep,
            sector_eu,
            this_param,
            step_off_distance,
            &mut s1,
            Some(start_point),
            None,
        ))?;
        let norm1 = s1.tangent.cross(s1.binormal);

        // 2. get Edgeuse->RadialPartner->face stepoff binormal
        if stepped_off(evaluate_binormal_step_off(
            k,
            brep,
            eu_radial,
            this_param,
            step_off_distance,
            &mut s2,
            Some(start_point),
            Some(s1.tangent),
        ))? {
            stepped_off_surface = true;
        }
        let norm2 = s2.tangent.cross(s2.binormal);

        // 3. get ClassifyEdge->face stepoff binormal
        if stepped_off(evaluate_binormal_step_off(
            k,
            match other_brep.as_deref_mut() {
                Some(o) => o,
                None => &mut *brep,
            },
            classify_eu,
            other_param,
            step_off_distance,
            &mut s3,
            Some(start_point),
            Some(s1.tangent),
        ))? {
            stepped_off_surface = true;
        }
        let norm3 = s3.tangent.cross(s3.binormal);

        // Project the binormal into the plane defined by sTan1.
        s1.tangent = vec_status(s1.tangent.unitize(), OP_RADIAL)?;
        let bin3 = s1.tangent.cross(s3.binormal).cross(s1.tangent);
        s3.binormal = bin3;

        // if any StepOff calculation stepped off a surface
        if stepped_off_surface {
            // its an error to step off on first pass
            if count == 0 {
                return Err(SolidError::sm_err(OP_RADIAL));
            }

            // (the angle values are from the previous iteration)
            if ang2 <= ang3 + EFF_ZERO && ang2 < 0.5 * PI / 180.0 {
                return Ok(if norm2.dot(norm3) > 0.0 {
                    RadialSectorClass::CoincidentSector
                } else {
                    RadialSectorClass::CoincidentSectorMate
                });
            } else if ang3 <= ang2 + EFF_ZERO && ang3 < 0.5 * PI / 180.0 {
                return Ok(if norm2.dot(norm3) > 0.0 {
                    RadialSectorClass::CoincidentRadial
                } else {
                    RadialSectorClass::CoincidentRadialMate
                });
            } else if ang1 < 0.5 * PI / 180.0 {
                // Zero sector.
                return Ok(if bin3.dot(norm1) > 0.0 {
                    RadialSectorClass::ZeroSector
                } else {
                    RadialSectorClass::ZeroSectorMate
                });
            }

            // Just take the best answer that we could find.
            return Ok(if inside {
                RadialSectorClass::Inside
            } else {
                RadialSectorClass::Outside
            });
        }

        // classify the binormal vector set
        inside = if lamina_edge {
            true
        } else {
            bin3.is_inside_sector(s1.binormal, norm1, s2.binormal)
        };

        done = true;

        // check distances between stepOff points to detect coincidence
        let diff2 = s1.point.distance(s3.point);
        let diff3 = s2.point.distance(s3.point);
        min_dist = diff2.min(diff3);

        // after 2 iterations and given nicely separated stepoff points
        if count > 2 && min_dist > tolerance {
            // and working with a lamina edge
            if lamina_edge {
                return Ok(if ang2 > EFF_ZERO_SQRT {
                    // face is inside
                    RadialSectorClass::Inside
                } else if norm1.dot(norm3) > 0.0 {
                    RadialSectorClass::CoincidentSector
                } else {
                    RadialSectorClass::CoincidentSectorMate
                });
            }

            // If sector angle is zero (faces of EU and its radial partner are
            // parallel) and there are no other faces, that's the same as a lamina edge.
            if ang1 < EFF_ZERO_SQRT && edge_faces(brep, edge)?.len() < 3 {
                return Ok(if ang2 > EFF_ZERO_SQRT {
                    if bin3.dot(norm1) > 0.0 {
                        RadialSectorClass::ZeroSector
                    } else {
                        RadialSectorClass::ZeroSectorMate
                    }
                } else if bin3.dot(norm1) > 0.0 {
                    RadialSectorClass::CoincidentSector
                } else {
                    RadialSectorClass::CoincidentSectorMate
                });
            }

            // get out of big loop - main exit point
            break;
        }

        // calculate the angles between the 3 different binormal vectors
        ang1 = vec_status(s1.binormal.angle_between(s2.binormal), OP_RADIAL)?;
        ang2 = vec_status(s1.binormal.angle_between(bin3), OP_RADIAL)?;
        ang3 = vec_status(s2.binormal.angle_between(bin3), OP_RADIAL)?;

        // when any of the binormal angles are less than 10 degrees then classify the
        // surfaces at an increased step off distance from the edge
        if ang1 < 10.0 * PI / 180.0 || ang2 < 10.0 * PI / 180.0 || ang3 < 10.0 * PI / 180.0 {
            if lamina_edge && ang2 > 10.0 * PI / 180.0 {
                return Ok(RadialSectorClass::Inside);
            }
            count += 1;

            if count < MAX_STEP_OFF_ROUNDS {
                done = false;
            }
            if count == 1 {
                step_off_distance = tolerance;
            }
            if count == 2 {
                step_off_distance = tolerance * 2.0;
            }
            if count > 2 {
                let denom = min_dist - last_min;
                if denom.abs() < EFF_ZERO {
                    last_step_off_distance = step_off_distance;
                    step_off_distance = 2.0 * last_step_off_distance;
                } else {
                    let ratio = (step_off_distance - last_step_off_distance) / denom;
                    last_step_off_distance = step_off_distance;
                    step_off_distance = tolerance * ratio;
                    // Don't let step distance grow or shrink by more than a factor of 3
                    if step_off_distance < 0.0 {
                        step_off_distance = 2.0 * last_step_off_distance;
                    } else if step_off_distance > 3.0 * last_step_off_distance {
                        step_off_distance = 3.0 * last_step_off_distance;
                    } else if step_off_distance < last_step_off_distance / 3.0 {
                        step_off_distance = last_step_off_distance / 3.0;
                    }
                }
            }
        }

        // Check for coincidence
        let mut coincidence = false;

        // when minDist is not changing over several iterations
        if count > 2
            && (min_dist - last_min).abs() < min_dist / 100.0 + EFF_ZERO
            && (min_dist - last2_min).abs() < min_dist / 100.0 + EFF_ZERO
        {
            coincidence = true;
        }

        // when angle between binormals has been very small over several iterations
        if count > 2 && (ang2 < deg2rad(0.01) || ang3 < deg2rad(0.01)) {
            coincidence = true;
        }

        // Make sure we have 8 consecutive coincidences prior to making the final call
        // on the coincidence situation.
        if coincidence {
            num_consecutive_coincidences += 1;
            if num_consecutive_coincidences < 8 {
                coincidence = false;
            }
        } else {
            num_consecutive_coincidences = 0;
        }

        if all_planes && (ang2 < deg2rad(0.001) || ang3 < deg2rad(0.001)) {
            coincidence = true;
        }

        if coincidence {
            // when ang2 (TargetFaceuse/ClassifyFaceuse angle) is small and less than ang3
            if ang2 <= ang3 + EFF_ZERO && ang2 < 0.5 * PI / 180.0 {
                return Ok(if norm1.dot(norm3) < 0.0 {
                    RadialSectorClass::CoincidentSectorMate
                } else {
                    RadialSectorClass::CoincidentSector
                });
            }
            // else when ang3 (RadialMateFaceuse/ClassifyFaceuse angle) is small and less
            // than ang2
            else if ang3 <= ang2 + EFF_ZERO && ang3 < 0.5 * PI / 180.0 {
                let norm4 = s2.tangent.cross(s2.binormal);
                let norm5 = s3.tangent.cross(bin3);
                return Ok(if norm4.dot(norm5) < 0.0 {
                    RadialSectorClass::CoincidentRadialMate
                } else {
                    RadialSectorClass::CoincidentRadial
                });
            }
        }

        if all_planes {
            break;
        }

        last2_min = last_min;
        last_min = min_dist;
    }

    if lamina_edge {
        return Ok(RadialSectorClass::Inside);
    }
    Ok(if inside {
        RadialSectorClass::Inside
    } else {
        RadialSectorClass::Outside
    })
}

/// `SmEdge::RadialSectorOfCurveClassify`: where `curve` (over `interval`, starting at
/// its minimum when `orientation` is [`OrientType::Same`], else at its maximum and
/// running backwards) leaves the edge relative to the sector between the face of
/// `sector_eu` and the face of its radial partner, at the edge parameter `edge_param`.
/// The curve's step-off direction is compared with the faces' binormals at growing
/// step-off distances. With `force_sectoring` a coincidence still gives
/// [`CurveSectorClass::Inside`] or [`CurveSectorClass::Outside`].
///
/// # Errors
///
/// `SM_ERR` when the curve's projected direction vanishes; `SM_ERR_INVALID_INPUT` (as
/// [`SolidError::Upstream`]) for a zero-length vector to unitize or measure;
/// [`SolidError::InvalidInput`]
/// for a sector edge use without radial partner; those of the step-offs and
/// evaluations; [`SolidError::Unported`] for the step-offs
/// (see [`crate::topology::edgeuse::p3`]).
pub fn radial_sector_of_curve_classify(
    k: &Kernel,
    brep: &mut Brep,
    edge: EdgeId,
    sector_eu: EdgeuseId,
    edge_param: f64,
    curve: &Curve,
    interval: &Extent1d,
    orientation: OrientType,
    force_sectoring: bool,
) -> Result<CurveSectorClass> {
    // locals
    let eu_radial = eu1::radial(brep, sector_eu)?
        .ok_or_else(|| null(OP_RADIAL_CURVE, "edge use without radial"))?;
    let tolerance = brep.edge(edge)?.tolerance;
    let same = orientation == OrientType::Same;
    let start_param = if same { interval.min() } else { interval.max() };
    let mut done = false;
    let mut inside = false;
    let mut count: u32 = 0;
    let mut last_min = 0.0;
    let mut last2_min = 0.0;

    let mut step_off_distance = 0.0;
    while !done {
        let mut s1 = StepOffVectors::default();
        let mut s2 = StepOffVectors::default();

        // calculate this faceuse stepOff, binormal and normal vectors.
        evaluate_binormal_step_off(
            k,
            brep,
            sector_eu,
            edge_param,
            step_off_distance,
            &mut s1,
            None,
            None,
        )?;
        let norm1 = s1.tangent.cross(s1.binormal);

        // calculate radialMate faceuse stepOff and binormal vector positions.
        evaluate_binormal_step_off(
            k,
            brep,
            eu_radial,
            edge_param,
            step_off_distance,
            &mut s2,
            None,
            None,
        )?;

        // calculate ClassificationCurve stepOff point and tangent (status ignored: a
        // failure leaves the start parameter)
        let step_param = ignored(euclid_step_off(
            k,
            curve,
            interval,
            start_param,
            same,
            step_off_distance,
        ))?
        .unwrap_or(start_param);
        let pv = curve.evaluate(k, step_param, 1, true, true)?;
        let mut tan = *pv
            .get(1)
            .ok_or_else(|| SolidError::sm_err(OP_RADIAL_CURVE))?;
        if orientation == OrientType::Opposite {
            tan = -tan;
        }
        let pnt3 = *pv
            .first()
            .ok_or_else(|| SolidError::sm_err(OP_RADIAL_CURVE))?;

        // Project binormals into plane which is perpendicular to edge
        let tan1 = vec_status(s1.tangent.unitize(), OP_RADIAL_CURVE)?;
        let norm1 = vec_status(norm1.unitize(), OP_RADIAL_CURVE)?;
        let bin2 = tan1.cross(s2.binormal).cross(tan1);
        let bin3 = tan1.cross(tan).cross(tan1);
        if bin3.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_RADIAL_CURVE));
        }

        // test the curve against the sector boundaries
        inside = bin3.is_inside_sector(s1.binormal, norm1, bin2);
        done = true;

        // If some vectors are less than 10 degrees from each other then reclassify
        // after stepping off a little.
        let ang1 = vec_status(s1.binormal.angle_between(bin2), OP_RADIAL_CURVE)?;
        let ang2 = vec_status(s1.binormal.angle_between(bin3), OP_RADIAL_CURVE)?;
        let ang3 = vec_status(bin2.angle_between(bin3), OP_RADIAL_CURVE)?;
        if ang1 < 10.0 * PI / 180.0 || ang2 < 10.0 * PI / 180.0 || ang3 < 10.0 * PI / 180.0 {
            if eu1::edge_is_lamina(brep, edge)? && ang2 > 10.0 * PI / 180.0 {
                return Ok(CurveSectorClass::Inside);
            }
            count += 1;
            if count < MAX_STEP_OFF_ROUNDS {
                done = false;
            }
            if count == 1 {
                step_off_distance = tolerance;
            }
            if count == 2 {
                step_off_distance = tolerance * 2.0;
            }
            if count > 2 {
                step_off_distance *= 4.0;
            }
        }

        // once we stepoff, step off until the points are farther apart than the
        // tolerance
        let diff2 = s1.point.distance(pnt3);
        let diff3 = s2.point.distance(pnt3);
        let min_dist = diff2.min(diff3);
        if count > 2 && min_dist > tolerance {
            if eu1::edge_is_lamina(brep, edge)? && ang2 > EFF_ZERO_SQRT {
                return Ok(CurveSectorClass::Inside);
            }
            done = true;
        }

        // Check for coincidence
        let mut coincidence = false;
        if count > 2
            && (min_dist - last_min).abs() < min_dist / 100.0 + EFF_ZERO
            && (min_dist - last2_min).abs() < min_dist / 100.0 + EFF_ZERO
        {
            coincidence = true;
        }
        if count > 2 && (ang2 < EFF_ZERO || ang3 < EFF_ZERO) {
            coincidence = true;
        }

        if coincidence {
            // We have a coincidence if we are not diverging and the angle is less than
            // 1/2 degree.
            if force_sectoring {
                // Jump out and set current result
                break;
            }

            if ang2 < ang3 && ang2 < 0.5 * PI / 180.0 {
                return Ok(CurveSectorClass::CoincidentSector);
            } else if ang3 < ang2 && ang3 < 0.5 * PI / 180.0 {
                return Ok(CurveSectorClass::CoincidentRadial);
            }
        }

        last2_min = last_min;
        last_min = min_dist;
    }

    Ok(if inside {
        CurveSectorClass::Inside
    } else {
        CurveSectorClass::Outside
    })
}

/// `crCurve.EuclidianStepOff(crInterval, dStartParam, eOrientation, dist, dist/100,
/// bClampped, dStepParam)`: the step parameter.
fn euclid_step_off(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    start_param: f64,
    forward: bool,
    step_off_distance: f64,
) -> Result<f64> {
    Ok(euclidian_step_off(
        k,
        curve,
        interval,
        start_param,
        forward,
        step_off_distance,
        step_off_distance / 100.0,
    )?
    .end_param)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::topology::{
        Edge, Edgeuse, EdgeuseOwner, Region, Shell, Vertex, Vertexuse, VertexuseOwner,
    };

    /// A wire edge along a line from `a` to `b` over `[0, 1]`, as
    /// `SmBrep::CreateWireEdgeFromCurve` links it (two shell edge uses, the first
    /// running with the curve).
    fn wire(a: Point3, b: Point3, with_curve: bool) -> (Brep, EdgeId) {
        let mut br = Brep::new();
        let r = br
            .add_region_record(Region {
                in_brep: true,
                ..Region::new()
            })
            .unwrap();
        let s = br
            .add_shell_record(Shell {
                faces: Vec::new(),
                region: Some(r),
                shell_type: crate::topology::shell::ShellType::Wire,
                members: crate::topology::shell::ShellMembers::Edgeuse(EdgeuseId::from_index(0)),
            })
            .unwrap();
        br.region_mut(r).unwrap().shells.push(s);
        let v0 = br
            .add_vertex_record(Vertex {
                point: a,
                tolerance: 1e-6,
                ..Vertex::default()
            })
            .unwrap();
        let v1 = br
            .add_vertex_record(Vertex {
                point: b,
                tolerance: 1e-6,
                ..Vertex::default()
            })
            .unwrap();
        let eu0 = EdgeuseId::from_index(0);
        let eu1_ = EdgeuseId::from_index(1);
        let line = Line::bounded(
            a,
            b - a,
            &Extent1d::from_min_max(0.0, 1.0),
            &BoundedLineOptions::default(),
        );
        let e = br
            .add_edge_record(Edge {
                vertices: [v0, v1],
                edgeuses: vec![eu0, eu1_],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-6,
                curve: with_curve.then(|| Curve::from(line)),
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
            br.vertex_mut(v).unwrap().vertexuses.push(vu);
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

    #[test]
    fn vertices_and_tight_box_of_a_line() {
        let k = Kernel::default();
        let (b, e) = wire(Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0), true);
        assert_eq!(edge_vertices(&b, e).unwrap().len(), 2);
        assert!(edge_faces(&b, e).unwrap().is_empty());
        let bb = calculate_tight_bounding_box(&k, &b, e).unwrap().unwrap();
        assert_eq!(bb.min(), Vec3::ZERO);
        assert_eq!(bb.max(), Vec3::new(1.0, 2.0, 3.0));
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        };
        let tight = calculate_bounding_box(&k, &b, e, req, true).unwrap();
        assert_eq!(tight.normal, Some(bb));
    }

    #[test]
    fn edge_without_curve() {
        let k = Kernel::default();
        let (b, e) = wire(Vec3::ZERO, Vec3::X, false);
        assert_eq!(calculate_tight_bounding_box(&k, &b, e).unwrap(), None);
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: true,
            polar: true,
        };
        let boxes = calculate_bounding_box(&k, &b, e, req, false).unwrap();
        assert_eq!(boxes.normal, Some(Extent3d::new()));
        assert!(matches!(
            closest_point(&k, &b, e, Vec3::ZERO),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn closest_point_on_a_line() {
        let k = Kernel::default();
        let (b, e) = wire(Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), true);
        let cp = closest_point(&k, &b, e, Vec3::new(1.0, 1.0, 0.0))
            .unwrap()
            .unwrap();
        let at = b
            .edge(e)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .evaluate_point(&k, cp.parameter)
            .unwrap();
        assert!(at.distance(Vec3::X) < 1e-12);
        assert!((cp.distance - 1.0).abs() < 1e-12);
    }

    #[test]
    fn cross_brep_classification_does_not_alias_equal_local_ids() {
        let k = Kernel::default();
        let (mut b, e) = wire(Vec3::ZERO, Vec3::X, true);
        let mut other = Brep::new();
        let eu = EdgeuseId::from_index(0);
        // The primary has eu 0; the other operand does not. Reading the primary
        // for both operands would reach the plane check instead of rejecting it.
        assert!(matches!(
            radial_sector_classify_between(&k, &mut b, e, 0.5, eu, Some(&mut other), eu),
            Err(SolidError::NotFound { .. })
        ));
    }

    #[test]
    fn radial_classifications_reach_the_step_off() {
        let k = Kernel::default();
        let (mut b, e) = wire(Vec3::ZERO, Vec3::X, true);
        let eu = EdgeuseId::from_index(0);
        // a wire edge use has no face: upstream dereferences it
        assert!(matches!(
            radial_sector_classify(&k, &mut b, e, 0.5, eu, eu),
            Err(SolidError::InvalidInput { .. })
        ));
        let c = b.edge(e).unwrap().curve.clone().unwrap();
        let r = radial_sector_of_curve_classify(
            &k,
            &mut b,
            e,
            eu,
            0.5,
            &c,
            &Extent1d::from_min_max(0.0, 1.0),
            OrientType::Same,
            false,
        );
        // the step-off reaches `EvaluateBinormal`, which returns `SM_ERR_INVALID_INPUT`
        // for an edge use outside a loop (without dereferencing it)
        assert!(
            matches!(r, Err(SolidError::Upstream { code: 1007, .. })),
            "{r:?}"
        );
        assert_eq!(RadialSectorClass::ZeroSectorMate.code(), 7);
        assert_eq!(CurveSectorClass::CoincidentRadial.code(), 3);
    }
}
