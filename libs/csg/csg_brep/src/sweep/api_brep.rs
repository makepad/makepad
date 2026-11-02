//! High-level B-rep sweeps (`SM_API`): linear, rotational, draft (taper), pipe and
//! curve sweeps of profile curves, faces or edges into solids, sweeps along a planar
//! path, and non-manifold translational and rotational sweeps of a B-rep's own faces,
//! edges and vertices.
//!
//! Upstream: `source/SM_API/src/SmApiBrep.cpp`, `source/SM_API/inc/SmApiBrep.h`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | `copy_curves_for_consuming_sweep` (private)  | `SmApiCopyCurvesForConsumingSweep` (static)    |
//! | [`create_linear_sweep`]                      | `SmApiCreateLinearSweep`                       |
//! | [`create_rotational_sweep`]                  | `SmApiCreateRotationalSweep`                   |
//! | [`create_draft_sweep`]                       | `SmApiCreateDraftSweep`                        |
//! | [`create_pipe_sweep`]                        | `SmApiCreatePipeSweep`                         |
//! | [`create_sweep_along_planar_path`]           | `SmApiCreateSweepAlongPlanarPath`              |
//! | [`create_curve_sweep`], [`CurveSweep`]       | `SmApiCreateCurveSweep`                        |
//! | [`create_curve_sweep_with_placements`], [`PlacedCurveSweep`] | `SmApiCreateCurveSweep` with the source fork's `pOptPlacements` |
//! | [`create_curve_sweep_from_faces`]            | `SmApiCreateCurveSweepFromFaces`               |
//! | [`create_curve_sweep_from_edges`]            | `SmApiCreateCurveSweepFromEdges`               |
//! | [`create_taper_extrude`]                     | `SmApiCreateTaperExtrude`                      |
//! | [`create_taper_extrude_with_corner`], [`TaperExtrude`] | `SmApiCreateTaperExtrude` (fork: `eCorner`, `pOptRefusal`, `pOptRefusedCorner`) |
//! | [`create_linear_sweep_with_repetitions`]     | `SmApiCreateLinearSweepWithRepetitions`        |
//! | [`create_rotational_sweep_with_repetitions`] | `SmApiCreateRotationalSweepWithRepetitions`    |
//! | `selection_is_owned_by` (private)            | `smApiSweepSelectionIsOwnedBy` (static template) |
//! | [`non_manifold_sweep`], [`SweepSelection`], [`RegionBrep`] | `SmApiNonManifoldSweep`          |
//! | [`non_manifold_rotational_sweep`]            | `SmApiNonManifoldRotationalSweep`              |
//!
//! Deviations:
//! - **Out-params.** The created B-rep (and, for the curve sweeps, its start, side and
//!   end faces) is the return value; on failure there is none (upstream clears
//!   `rpResult`). The optional face arrays of the curve sweeps are always returned.
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`], used for the
//!   curve copies.
//! - **Cached topology.** Every returned B-rep has the port's own cached fields
//!   ([`crate::topology::Edge::vertices`], `Face::shell`, ..., not maintained by topology
//!   edits) rewritten from its use links by [`Brep::refresh_cached_topology`]; nothing
//!   else changes.
//! - **Pointers.** A `NULL` curve that upstream tests for is `None`: the entries of the
//!   consumed curve arrays (copied first, `SM_ERR_INVALID_INPUT` for `NULL`), the
//!   entries of a curve-sweep profile (passed on, upstream skips them), and the path,
//!   scale-reference and scale curves of the curve and pipe sweeps. The profile and
//!   path curves of [`create_sweep_along_planar_path`], which upstream dereferences
//!   unchecked, are references. Upstream's path and scale curves are
//!   `SmBSplineCurve*`, which includes lines and ellipses; they are [`Curve`]s here.
//!   The faces and edges of the curve sweeps are ids into
//!   the caller's [`Brep`]. A `NULL` `pOptions` is `None` (the default options).
//! - **Types.** `SmBoolean` flags are `bool` (`bCornerType` of
//!   [`create_draft_sweep`]: any non-zero value is a fillet corner); the cap selectors
//!   `iCapEnds` / `iEndCaps` stay `i32`; `ULONG` repetition counts are `u64`.
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; errors of
//!   the curve copies pass through unchanged (`SM_ERR` for a curve kind without
//!   `Copy`, e.g. a Hermite curve).
//! - **Non-manifold sweeps.** [`non_manifold_sweep`] and
//!   [`non_manifold_rotational_sweep`] sweep the caller's B-rep in place
//!   (`pBrepToSweep`) through a [`PrimitiveModel`] over it, into its infinite region.
//!   `pRegionBrep` is `region`: `None` is `NULL`, [`RegionBrep::Swept`] is
//!   `pBrepToSweep` itself (`&mut` and `&` borrows of one B-rep cannot coexist) and
//!   [`RegionBrep::Other`] another B-rep, which is `SM_ERR_INVALID_INPUT`
//!   (`source/SM_API/src/SmApiBrep.cpp:1822`, `:1881`). The
//!   selections are ids into it ([`SweepSelection`]); a `None` entry is a `NULL`
//!   handle, and every entry must be live topology of the B-rep. On an error after
//!   validation the B-rep keeps the sweep's partial edits, as upstream.
//! - **Attributes.** A B-rep owns its attributes ([`Brep::add_attribute`]). The
//!   non-manifold sweeps carry the swept B-rep's attributes through the sweep's
//!   copies, splits, merges and deletions as upstream's notifications do, through
//!   [`PrimitiveModel`]'s attribute store. The curve sweeps from faces and edges copy
//!   only the edges' curves, as upstream, so the result carries none of the input
//!   objects' attributes (upstream's would sit on the curves, which hold no
//!   attributes here) and the inputs keep theirs.
//! - Every entry point validates and copies its inputs as upstream, then runs the
//!   `SmPrimitiveCreation` method it wraps on a new B-rep through a
//!   [`PrimitiveModel`]. Path and scale curves become their `SmBSplineCurve` part.
//!   The faces and edges of the curve sweeps are `(&Brep, id)` pairs: upstream
//!   objects reach their own B-rep.
//! - **source fork (source fork 5d3939e, WR-523, #1450).** The fork's
//!   optional `pOptPlacements` output of `SmApiCreateCurveSweep` is the sibling
//!   [`create_curve_sweep_with_placements`], so [`create_curve_sweep`] keeps its
//!   signature and does no extra work; the fork's `bProfileAtPathStart` is
//!   [`SweepOptions::profile_at_path_start`] (WR-522), off by default.

use crate::curve::curve::p2::CurveKind;
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::primitive::primitive_creation::model::PrimitiveModel;
use crate::primitive::primitive_creation::p1::PrimitiveCreation;
use crate::primitive::primitive_creation::p3::RepeatedSweepOptions;
use crate::primitive::primitive_creation::SweepOptions;
use crate::sweep::rotational_sweep_geometry::RotationalSweepGeometry;
use crate::sweep::topology_sweep::engine::SweepTargets;
use crate::sweep::topology_sweep::geometry::SweepGeometry;
use crate::sweep::topology_sweep::{TopologySweep, TopologySweepOptions};
use crate::sweep::trans_sweep_geometry::TranslationalSweepGeometry;
use crate::topology::brep::p1::is_live_topology_member;
use crate::topology::{Brep, EdgeId, FaceId, VertexId};

const OP_COPY: &str = "sweep::api_brep::copy_curves_for_consuming_sweep";
const OP_PIPE: &str = "sweep::api_brep::create_pipe_sweep";
const OP_PLANAR_PATH: &str = "sweep::api_brep::create_sweep_along_planar_path";
const OP_CURVE_SWEEP: &str = "sweep::api_brep::create_curve_sweep";
const OP_FROM_FACES: &str = "sweep::api_brep::create_curve_sweep_from_faces";

const OP_FROM_EDGES: &str = "sweep::api_brep::create_curve_sweep_from_edges";
const OP_TAPER: &str = "sweep::api_brep::create_taper_extrude";
const OP_LINEAR_REPS: &str = "sweep::api_brep::create_linear_sweep_with_repetitions";
const OP_ROTATIONAL_REPS: &str = "sweep::api_brep::create_rotational_sweep_with_repetitions";
const OP_NON_MANIFOLD: &str = "sweep::api_brep::non_manifold_sweep";

/// How the gaps between offset profile curves are filled by a draft sweep or taper
/// extrusion (`SmOffsetCornerType`).
pub use crate::curve::composite_curve::OffsetCornerType;

/// Why a draft corner was refused, and where (`SmTaperCornerRefusal`,
/// `pOptRefusedCorner`; source change).
pub use crate::primitive::primitive_creation::p2::{CornerRefusal, TaperCornerRefusal};

/// The result of [`create_taper_extrude_with_corner`]: the solid, or the corner the
/// draft could not close (the fork's `SM_ERR_INVALID_INPUT` with `pOptRefusal` set;
/// no B-rep is returned then).
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // returned once, by value, like the `Brep` of `create_taper_extrude`
pub enum TaperExtrude {
    /// `rpResult`: the drafted solid.
    Solid(Brep),
    /// `pOptRefusal` and `pOptRefusedCorner`: the refused corner.
    Refused(CornerRefusal),
}

/// A swept solid and its faces (the outputs of the curve sweeps).
#[derive(Debug, Clone, Default)]
pub struct CurveSweep {
    /// `rpResult`: the swept solid.
    pub brep: Brep,
    /// `pOptStartFaces`: the start cap faces.
    pub start_faces: Vec<FaceId>,
    /// `pOptSideFaces`: the swept side faces.
    pub side_faces: Vec<FaceId>,
    /// `pOptEndFaces`: the end cap faces.
    pub end_faces: Vec<FaceId>,
}

/// A curve sweep and where it placed the profile (the outputs of
/// [`create_curve_sweep_with_placements`]; the source fork's `pOptPlacements`).
#[derive(Debug, Clone)]
pub struct PlacedCurveSweep {
    /// The swept solid and its faces, as [`create_curve_sweep`] returns them.
    pub sweep: CurveSweep,
    /// `pOptPlacements`: the rigid motions carrying the profile curves as given onto
    /// the sweep's first, middle and last sections.
    pub placements: [Axis2Placement; 3],
}

/// Deep-copies the caller's profile curves for the consuming `SmPrimitiveCreation`
/// sweeps, which strip or delete the curves they are given
/// (`SmApiCopyCurvesForConsumingSweep`). The caller's curves are never changed.
///
/// # Errors
///
/// `InvalidInput` for a `None` curve; the error of the first failing copy (`SM_ERR`
/// for a curve kind without `Copy`). Curves after the first failure are not copied.
fn copy_curves_for_consuming_sweep(
    k: &Kernel,
    input_curves: &[Option<&Curve>],
) -> Result<Vec<Curve>> {
    let mut curve_copies = Vec::new();
    for input in input_curves {
        let Some(input) = input else {
            return Err(SolidError::invalid_input(OP_COPY, "NULL curve"));
        };
        curve_copies.push(input.copy(k)?);
    }
    Ok(curve_copies)
}

/// The solid swept by `curves_to_sweep` along `sweep_dir` over `sweep_dist`
/// (`SmApiCreateLinearSweep`): one repetition, capped at both ends with `cap_ends`.
/// The caller's curves are copied, never changed.
///
/// # Errors
///
/// `InvalidInput` for a `None` curve; the error of a failing curve copy;
/// [`SolidError::Unported`] otherwise (`SmPrimitiveCreation::CreateLinearSweep`).
pub fn create_linear_sweep(
    k: &mut Kernel,
    curves_to_sweep: &[Option<&Curve>],
    sweep_dir: Vec3,
    sweep_dist: f64,
    cap_ends: bool,
) -> Result<Brep> {
    // Sweep copies of the inputs: CreateLinearSweep consumes the curves.
    let curve_copies = copy_curves_for_consuming_sweep(k, curves_to_sweep)?;

    let mut curve_copies: Vec<Option<Curve>> = curve_copies.into_iter().map(Some).collect();
    build(k, |pc, model| {
        pc.create_repeated_linear_sweep(
            model,
            PrimitiveModel::MAIN,
            &mut curve_copies,
            sweep_dir,
            sweep_dist,
            repeated(1, cap_ends),
        )
    })
}

/// The solid swept by revolving `curves_to_sweep` about the axis through `base_pt`
/// along `axis` by `angle_deg` degrees (`SmApiCreateRotationalSweep`): one
/// repetition, capped at both ends with `cap_ends`. The caller's curves are copied,
/// never changed.
///
/// # Errors
///
/// `InvalidInput` for a `None` curve; the error of a failing curve copy;
/// [`SolidError::Unported`] otherwise (`SmPrimitiveCreation::CreateRotationalSweep`).
pub fn create_rotational_sweep(
    k: &mut Kernel,
    curves_to_sweep: &[Option<&Curve>],
    base_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
    cap_ends: bool,
) -> Result<Brep> {
    // Sweep copies of the inputs: CreateRotationalSweep consumes the curves.
    let curve_copies = copy_curves_for_consuming_sweep(k, curves_to_sweep)?;

    let mut curve_copies: Vec<Option<Curve>> = curve_copies.into_iter().map(Some).collect();
    build(k, |pc, model| {
        pc.create_partial_rotational_sweep(
            model,
            PrimitiveModel::MAIN,
            &mut curve_copies,
            base_pt,
            axis,
            angle_deg,
            repeated(1, cap_ends),
        )
    })
}

/// The solid extruded from `curves_to_sweep` to `height` with a draft (taper)
/// angle of `angle_deg` degrees (`SmApiCreateDraftSweep`). `corner_type` selects
/// fillet corners (`true`) or linear extensions (`false`) between the offset curves;
/// `cap_ends` is `0` none, `1` at the curves, `2` at the offset, `3` both ends. The
/// curve normal is computed by the extrusion (a zero normal is passed). The caller's
/// curves are copied, never changed.
///
/// # Errors
///
/// `InvalidInput` for a `None` curve; the error of a failing curve copy;
/// [`SolidError::Unported`] otherwise (`SmBrep::SmBrep`, until the B-rep constructor is ported).
pub fn create_draft_sweep(
    k: &mut Kernel,
    curves_to_sweep: &[Option<&Curve>],
    height: f64,
    angle_deg: f64,
    corner_type: bool,
    cap_ends: i32,
) -> Result<Brep> {
    // Sweep copies of the inputs: CreateTaperExtrude consumes the curves.
    let curve_copies = copy_curves_for_consuming_sweep(k, curves_to_sweep)?;

    // Let CreateTaperExtrude compute the normal
    let crv_normal = Vec3::new(0.0, 0.0, 0.0);

    let corner = if corner_type {
        OffsetCornerType::FilletCorner
    } else {
        OffsetCornerType::LinearExtension
    };

    let mut crv_normal = crv_normal;
    build(k, |pc, model| {
        pc.create_taper_extrude(
            model,
            PrimitiveModel::MAIN,
            curve_copies,
            height,
            angle_deg,
            cap_ends,
            &mut crv_normal,
            corner,
        )
    })
}

/// The pipe of radius `pipe_radius` swept along `path_curve`
/// (`SmApiCreatePipeSweep`), capped at both ends with `cap_ends`; not
/// translational.
///
/// # Errors
///
/// `InvalidInput` for no path curve or a radius `<= 0` (a NaN radius passes, as
/// upstream); [`SolidError::Unported`] otherwise
/// (`SmBrep::SmBrep`, until the B-rep constructor is ported).
pub fn create_pipe_sweep(
    k: &mut Kernel,
    pipe_radius: f64,
    path_curve: Option<&Curve>,
    cap_ends: bool,
) -> Result<Brep> {
    let Some(path_curve) = path_curve else {
        return Err(SolidError::invalid_input(OP_PIPE, "no path curve"));
    };
    if pipe_radius <= 0.0 {
        return Err(SolidError::invalid_input(OP_PIPE, "pipe radius <= 0"));
    }

    let path = bspline_of(path_curve, OP_PIPE)?;
    let approx_tol = 0.0;
    let translational_sweep = false;
    build(k, |pc, model| {
        let (mut start, mut side, mut end) = (Vec::new(), Vec::new(), Vec::new());
        pc.create_pipe_sweep(
            model,
            PrimitiveModel::MAIN,
            pipe_radius,
            &path,
            approx_tol,
            cap_ends,
            translational_sweep,
            &mut start,
            &mut side,
            &mut end,
        )
    })
}

/// The solid swept by the profile boundary `profile_curves` along the ordered,
/// endpoint-connected planar path `path_curves` (`SmApiCreateSweepAlongPlanarPath`).
/// `move_profile` anchors the profile center (`true`) or its first point (`false`)
/// at the path start; `cap_ends` is `0` none, `1` profile/start, `2` far end, `3`
/// both (ignored for a closed path). The inputs are not changed.
///
/// # Errors
///
/// `InvalidInput` for no profile or path curve or `cap_ends` outside `0..=3`;
/// [`SolidError::Unported`] otherwise
/// (`SmBrep::SmBrep`, until the B-rep constructor is ported).
pub fn create_sweep_along_planar_path(
    k: &mut Kernel,
    profile_curves: &[&Curve],
    path_curves: &[&Curve],
    move_profile: bool,
    cap_ends: i32,
) -> Result<Brep> {
    if profile_curves.is_empty() || path_curves.is_empty() {
        return Err(SolidError::invalid_input(
            OP_PLANAR_PATH,
            "no profile or path curves",
        ));
    }
    if !(0..=3).contains(&cap_ends) {
        return Err(SolidError::invalid_input(
            OP_PLANAR_PATH,
            "cap selector outside 0..=3",
        ));
    }

    let tol = ZONE_TOL_3D;
    let cap_start = (cap_ends & 1) != 0;
    let cap_end = (cap_ends & 2) != 0;

    let profiles: Vec<Curve> = profile_curves.iter().map(|c| (*c).clone()).collect();
    let paths: Vec<Curve> = path_curves.iter().map(|c| (*c).clone()).collect();
    // The static entry point makes its own B-rep in the context of the model's.
    let (_, mut result) = PrimitiveModel::build_new(k, |_, model| {
        let id = PrimitiveCreation::create_sweep_along_planar_path(
            model,
            PrimitiveModel::MAIN,
            &profiles,
            &paths,
            tol,
            cap_start,
            cap_end,
            move_profile,
        )?;
        model.take_brep(id)
    })?;
    // (not upstream: the port's cached topology fields, true for the caller)
    result.refresh_cached_topology();
    Ok(result)
}

/// The solid swept by `profile_curves` along `path_curve`, scaled as
/// `scale_curve` relative to `scale_reference` when given, with `options` (`None`:
/// the defaults) (`SmApiCreateCurveSweep`). The kernel sweeps copies of the
/// profile; `None` profile entries are passed on (upstream skips them).
///
/// # Errors
///
/// `InvalidInput` for an empty profile or no path curve; [`SolidError::Unported`]
/// otherwise (`SmPrimitiveCreation::CreateCurveSweep`).
pub fn create_curve_sweep(
    k: &mut Kernel,
    profile_curves: &[Option<&Curve>],
    path_curve: Option<&Curve>,
    scale_reference: Option<&Curve>,
    scale_curve: Option<&Curve>,
    options: Option<&SweepOptions>,
) -> Result<CurveSweep> {
    let Some(path_curve) = path_curve.filter(|_| !profile_curves.is_empty()) else {
        return Err(SolidError::invalid_input(
            OP_CURVE_SWEEP,
            "no profile curves or no path curve",
        ));
    };

    let options = options.copied().unwrap_or_default();

    let approx_tol = ZONE_TOL_3D;
    let path = bspline_of(path_curve, OP_CURVE_SWEEP)?;
    let scale = scale_curve
        .map(|c| bspline_of(c, OP_CURVE_SWEEP))
        .transpose()?;
    let profiles: Vec<Option<Curve>> = profile_curves.iter().map(|c| c.cloned()).collect();
    curve_sweep(k, |pc, model, faces| {
        pc.create_curve_sweep(
            model,
            PrimitiveModel::MAIN,
            &profiles,
            &path,
            scale_reference,
            scale.as_ref(),
            approx_tol,
            options,
            true,
            &mut faces.0,
            &mut faces.1,
            &mut faces.2,
        )
    })
}

/// [`create_curve_sweep`] that also says where it placed the profile
/// (`SmApiCreateCurveSweep` with the source fork's `pOptPlacements`,
/// source fork 5d3939e, WR-523, #1450): the rigid motions carrying the profile
/// curves as given onto the first, middle and last sections the sweep placed. Each
/// placement maps the global system onto the motion's image (`transform_point` of a
/// profile point is that point on the section). The solid and faces are the same as
/// [`create_curve_sweep`]'s.
///
/// # Errors
///
/// Those of [`create_curve_sweep`]; `InvalidInput` with a scale curve (a scaled
/// sweep's sections are no rigid images of the profile).
pub fn create_curve_sweep_with_placements(
    k: &mut Kernel,
    profile_curves: &[Option<&Curve>],
    path_curve: Option<&Curve>,
    scale_reference: Option<&Curve>,
    scale_curve: Option<&Curve>,
    options: Option<&SweepOptions>,
) -> Result<PlacedCurveSweep> {
    let Some(path_curve) = path_curve.filter(|_| !profile_curves.is_empty()) else {
        return Err(SolidError::invalid_input(
            OP_CURVE_SWEEP,
            "no profile curves or no path curve",
        ));
    };

    let options = options.copied().unwrap_or_default();

    let approx_tol = ZONE_TOL_3D;
    let path = bspline_of(path_curve, OP_CURVE_SWEEP)?;
    let scale = scale_curve
        .map(|c| bspline_of(c, OP_CURVE_SWEEP))
        .transpose()?;
    let profiles: Vec<Option<Curve>> = profile_curves.iter().map(|c| c.cloned()).collect();
    let (sweep, placements) = curve_sweep_with(k, |pc, model, faces| {
        pc.create_curve_sweep_with_placements(
            model,
            PrimitiveModel::MAIN,
            &profiles,
            &path,
            scale_reference,
            scale.as_ref(),
            approx_tol,
            options,
            true,
            &mut faces.0,
            &mut faces.1,
            &mut faces.2,
        )
    })?;
    Ok(PlacedCurveSweep { sweep, placements })
}

/// The solid swept by the boundaries of `faces` (each with its B-rep) along
/// `path_curve` (`SmApiCreateCurveSweepFromFaces`), scaled and optioned as
/// [`create_curve_sweep`]. The faces and their curves are not changed.
///
/// # Errors
///
/// `InvalidInput` for no faces or no path curve; those of
/// `SmPrimitiveCreation::CreateCurveSweepFromFaces`.
pub fn create_curve_sweep_from_faces(
    k: &mut Kernel,
    faces: &[(&Brep, FaceId)],
    path_curve: Option<&Curve>,
    scale_reference: Option<&Curve>,
    scale_curve: Option<&Curve>,
    options: Option<&SweepOptions>,
) -> Result<CurveSweep> {
    let Some(path_curve) = path_curve.filter(|_| !faces.is_empty()) else {
        return Err(SolidError::invalid_input(
            OP_FROM_FACES,
            "no faces or no path curve",
        ));
    };

    let options = options.copied().unwrap_or_default();

    let approx_tol = ZONE_TOL_3D;
    let (path, scale_reference, scale) =
        sweep_curves(path_curve, scale_reference, scale_curve, OP_FROM_FACES)?;
    curve_sweep(k, |pc, model, out| {
        let input = model.add_owner_breps(faces)?;
        pc.create_curve_sweep_from_faces(
            model,
            PrimitiveModel::MAIN,
            &input,
            &path,
            scale_reference.as_ref(),
            scale.as_ref(),
            approx_tol,
            options,
            &mut out.0,
            &mut out.1,
            &mut out.2,
        )
    })
}

/// The solid swept by the curves of `edges` (each with its B-rep) along
/// `path_curve` (`SmApiCreateCurveSweepFromEdges`), scaled and optioned as
/// [`create_curve_sweep`]. The edges and their curves are not changed.
///
/// # Errors
///
/// `InvalidInput` for no edges or no path curve; those of
/// `SmPrimitiveCreation::CreateCurveSweepFromEdges`.
pub fn create_curve_sweep_from_edges(
    k: &mut Kernel,
    edges: &[(&Brep, EdgeId)],
    path_curve: Option<&Curve>,
    scale_reference: Option<&Curve>,
    scale_curve: Option<&Curve>,
    options: Option<&SweepOptions>,
) -> Result<CurveSweep> {
    let Some(path_curve) = path_curve.filter(|_| !edges.is_empty()) else {
        return Err(SolidError::invalid_input(
            OP_FROM_EDGES,
            "no edges or no path curve",
        ));
    };

    let options = options.copied().unwrap_or_default();

    let approx_tol = ZONE_TOL_3D;
    let (path, scale_reference, scale) =
        sweep_curves(path_curve, scale_reference, scale_curve, OP_FROM_EDGES)?;
    curve_sweep(k, |pc, model, out| {
        let input = model.add_owner_breps(edges)?;
        pc.create_curve_sweep_from_edges(
            model,
            PrimitiveModel::MAIN,
            &input,
            &path,
            scale_reference.as_ref(),
            scale.as_ref(),
            approx_tol,
            options,
            &mut out.0,
            &mut out.1,
            &mut out.2,
        )
    })
}

/// The solid extruded from `curves` to `height` with a draft angle of
/// `draft_angle_deg` degrees (`SmApiCreateTaperExtrude`). `end_caps` is `0` none,
/// `1` at the curves, `2` at the offset, `3` both; `fillet_corner` selects fillet
/// corners over linear extensions. The curve normal is computed by the extrusion.
/// The caller's curves are copied, never changed.
///
/// As in the C++ code, only the sign of `draft_angle_deg` picks the side (positive
/// inner, negative outer) and the in-plane offset is `|height * tan(|draft_angle_deg|)|`,
/// so an angle over 90 does not give the outer draft the C++ header promises. The
/// curves must form one chain (`DOC-5` in `plan/upstream-issues.md`).
///
/// # Errors
///
/// `InvalidInput` for no curves or a `None` curve, or for a corner refused under
/// `fixes.taper_tangent_corner_planes` (WR-521; [`create_taper_extrude_with_corner`]
/// reports why and where); the error of a failing curve copy; [`SolidError::Unported`]
/// otherwise (`SmBrep::SmBrep`, until the B-rep constructor is ported).
pub fn create_taper_extrude(
    k: &mut Kernel,
    curves: &[Option<&Curve>],
    height: f64,
    draft_angle_deg: f64,
    end_caps: i32,
    fillet_corner: bool,
) -> Result<Brep> {
    let corner = if fillet_corner {
        OffsetCornerType::FilletCorner
    } else {
        OffsetCornerType::LinearExtension
    };
    match create_taper_extrude_with_corner(k, curves, height, draft_angle_deg, end_caps, corner)? {
        TaperExtrude::Solid(brep) => Ok(brep),
        TaperExtrude::Refused(_) => {
            Err(SolidError::invalid_input(OP_TAPER, "draft corner refused"))
        }
    }
}

/// [`create_taper_extrude`] with the corner rule as an argument and a refused corner
/// as part of the result (source change, source fork 54148db: the fork's
/// `SmApiCreateTaperExtrude(..., eCorner, rpResult, pOptRefusal, pOptRefusedCorner)`).
///
/// - [`OffsetCornerType::LinearExtension`]: tangent lines across a corner's gap; with
///   `fixes.taper_tangent_corner_planes` (WR-521, off by default) a profile of lines
///   and circular arcs with a sharp corner continues its walls as their tangent planes
///   instead (two lines meet at a corner edge) and may be refused.
/// - [`OffsetCornerType::FilletCorner`]: an arc about the corner.
/// - [`OffsetCornerType::CurveExtension`]: each wall continues as itself (a line's
///   plane, an arc's cone) to meet its neighbour along their exact seam; a profile with
///   a curve that is neither a line nor an arc is refused at a sharp corner
///   ([`TaperCornerRefusal::NotExact`]), and so is a corner whose walls do not meet or
///   a wall that closes before the far end. Without a sharp corner it is
///   `LinearExtension`.
/// - [`OffsetCornerType::LinearChamfer`]: as the C++ passes it on to the offsetter.
///
/// # Errors
///
/// As [`create_taper_extrude`], except a refusal, which is
/// [`TaperExtrude::Refused`].
pub fn create_taper_extrude_with_corner(
    k: &mut Kernel,
    curves: &[Option<&Curve>],
    height: f64,
    draft_angle_deg: f64,
    end_caps: i32,
    corner: OffsetCornerType,
) -> Result<TaperExtrude> {
    if curves.is_empty() {
        return Err(SolidError::invalid_input(OP_TAPER, "no curves"));
    }

    // Sweep copies of the inputs: CreateTaperExtrude consumes the curves.
    let curve_copies = copy_curves_for_consuming_sweep(k, curves)?;

    let mut normal = Vec3::new(0.0, 0.0, 0.0);
    let (mut brep, refusal) = PrimitiveModel::build_new(k, |pc, model| {
        pc.create_taper_extrude_with_refusal(
            model,
            PrimitiveModel::MAIN,
            curve_copies,
            height,
            draft_angle_deg,
            end_caps,
            &mut normal,
            corner,
        )
    })?;
    if let Some(refusal) = refusal {
        return Ok(TaperExtrude::Refused(refusal));
    }
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(TaperExtrude::Solid(brep))
}

/// The solid swept by `curves_to_sweep` along `sweep_dir` over `sweep_dist`,
/// repeated end to end `repetitions` times (`SmApiCreateLinearSweepWithRepetitions`).
/// The caller's curves are copied, never changed.
///
/// # Errors
///
/// `InvalidInput` for no curves or a `None` curve; the error of a failing curve
/// copy; [`SolidError::Unported`] otherwise
/// (`SmPrimitiveCreation::CreateLinearSweep`).
pub fn create_linear_sweep_with_repetitions(
    k: &mut Kernel,
    curves_to_sweep: &[Option<&Curve>],
    sweep_dir: Vec3,
    sweep_dist: f64,
    repetitions: u64,
    cap_ends: bool,
) -> Result<Brep> {
    if curves_to_sweep.is_empty() {
        return Err(SolidError::invalid_input(OP_LINEAR_REPS, "no curves"));
    }

    // Sweep copies of the inputs: CreateLinearSweep consumes the curves.
    let curve_copies = copy_curves_for_consuming_sweep(k, curves_to_sweep)?;

    let mut curve_copies: Vec<Option<Curve>> = curve_copies.into_iter().map(Some).collect();
    build(k, |pc, model| {
        pc.create_repeated_linear_sweep(
            model,
            PrimitiveModel::MAIN,
            &mut curve_copies,
            sweep_dir,
            sweep_dist,
            repeated(repetitions, cap_ends),
        )
    })
}

/// The solid swept by revolving `curves_to_sweep` about the axis through `base_pt`
/// along `axis` by `angle_deg` degrees, repeated end to end `repetitions` times
/// (`SmApiCreateRotationalSweepWithRepetitions`). The caller's curves are copied,
/// never changed.
///
/// # Errors
///
/// `InvalidInput` for no curves or a `None` curve; the error of a failing curve
/// copy; [`SolidError::Unported`] otherwise
/// (`SmPrimitiveCreation::CreateRotationalSweep`).
pub fn create_rotational_sweep_with_repetitions(
    k: &mut Kernel,
    curves_to_sweep: &[Option<&Curve>],
    base_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
    repetitions: u64,
    cap_ends: bool,
) -> Result<Brep> {
    if curves_to_sweep.is_empty() {
        return Err(SolidError::invalid_input(OP_ROTATIONAL_REPS, "no curves"));
    }

    // Sweep copies of the inputs: CreateRotationalSweep consumes the curves.
    let curve_copies = copy_curves_for_consuming_sweep(k, curves_to_sweep)?;

    let mut curve_copies: Vec<Option<Curve>> = curve_copies.into_iter().map(Some).collect();
    build(k, |pc, model| {
        pc.create_partial_rotational_sweep(
            model,
            PrimitiveModel::MAIN,
            &mut curve_copies,
            base_pt,
            axis,
            angle_deg,
            repeated(repetitions, cap_ends),
        )
    })
}

/// `nRepetitions` and `bCapEnds` with `bTestContinuity` at its default, `TRUE`.
fn repeated(repetitions: u64, cap_ends: bool) -> RepeatedSweepOptions {
    RepeatedSweepOptions {
        repetitions,
        cap_ends,
        test_continuity: true,
    }
}

/// The topology a non-manifold sweep sweeps one dimension higher (`pOptFaces`,
/// `pOptEdges`, `pOptVertices`): ids into the swept B-rep. `None` is a `NULL` array and a
/// `None` entry a `NULL` handle; with all three arrays `None` the whole B-rep is swept.
#[derive(Debug, Clone, Copy, Default)]
pub struct SweepSelection<'a> {
    /// Faces to sweep into solids.
    pub faces: Option<&'a [Option<FaceId>]>,
    /// Edges to sweep into faces.
    pub edges: Option<&'a [Option<EdgeId>]>,
    /// Vertices to sweep into edges.
    pub vertices: Option<&'a [Option<VertexId>]>,
}

/// The `pRegionBrep` argument of the non-manifold sweeps, when not `NULL`.
#[derive(Debug, Clone, Copy)]
pub enum RegionBrep<'a> {
    /// `pRegionBrep == pBrepToSweep`.
    Swept,
    /// A B-rep other than the swept one (rejected).
    Other(&'a Brep),
}

/// `if( pRegionBrep && pRegionBrep != pBrepToSweep ) return SM_ERR_INVALID_INPUT;`
fn check_region_brep(region: Option<RegionBrep<'_>>) -> Result<()> {
    match region {
        None | Some(RegionBrep::Swept) => Ok(()),
        Some(RegionBrep::Other(_)) => Err(SolidError::invalid_input(
            OP_NON_MANIFOLD,
            "region B-rep other than the swept B-rep",
        )),
    }
}

/// Whether every entry of `selection` is live topology of `brep`
/// (`smApiSweepSelectionIsOwnedBy` of each array; a `NULL` array passes, a `NULL`
/// entry does not: `IsLiveTopologyMember(NULL)` is false).
fn selection_is_owned_by(brep: &Brep, selection: &SweepSelection<'_>) -> bool {
    fn owned<T: Copy>(brep: &Brep, list: Option<&[Option<T>]>, e: fn(T) -> EntityRef) -> bool {
        list.is_none_or(|l| l.iter().all(|x| is_live_topology_member(brep, x.map(e))))
    }
    owned(brep, selection.faces, EntityRef::Face)
        && owned(brep, selection.edges, EntityRef::Edge)
        && owned(brep, selection.vertices, EntityRef::Vertex)
}

/// `SmTopologySweep sSweep(sSweepGeom, bDoStitching, FALSE, FALSE, bDoMerge)` and
/// `DoSweep(pBrepToSweep, pBrepToSweep->GetInfiniteRegion(), pOptFaces, pOptEdges,
/// pOptVertices, NULL, NULL, NULL)` on `brep` in place.
fn non_manifold_sweep_with(
    k: &mut Kernel,
    brep: &mut Brep,
    geometry: &SweepGeometry,
    do_stitching: bool,
    do_merge: bool,
    selection: SweepSelection<'_>,
) -> Result<()> {
    const MAIN: crate::topology::BrepId = PrimitiveModel::MAIN;
    let mut sweep = TopologySweep::new(TopologySweepOptions {
        stitching: do_stitching,
        merge: do_merge,
        ..TopologySweepOptions::default()
    });
    let region = brep.infinite_region().map(|r| (MAIN, r));
    let mut targets = SweepTargets {
        faces_higher: selection
            .faces
            .map(|l| l.iter().map(|f| f.map(|f| (MAIN, f))).collect()),
        edges_higher: selection
            .edges
            .map(|l| l.iter().map(|e| e.map(|e| (MAIN, e))).collect()),
        vertices_higher: selection
            .vertices
            .map(|l| l.iter().map(|v| v.map(|v| (MAIN, v))).collect()),
        ..SweepTargets::default()
    };
    let mut model = PrimitiveModel::new(k, core::mem::take(brep));
    let result = sweep.do_sweep(&mut model, geometry, Some(MAIN), region, &mut targets);
    // The caller gets its (swept or partly edited) B-rep back, as upstream's
    // pBrepToSweep: `PrimitiveModel::delete_brep` refuses MAIN, and the sweep merges
    // MAIN only as the destination, whose B-rep a merge keeps.
    *brep = model.into_brep()?;
    // (not upstream: the port's cached topology fields, true again for the caller)
    brep.refresh_cached_topology();
    result
}

/// Sweeps the selected faces, edges and vertices of `brep` (the whole B-rep when
/// nothing is selected) along `sweep_dir` over `sweep_dist`, in place, into its
/// infinite region (`SmApiNonManifoldSweep`; `region` is `pRegionBrep`): only
/// `sweep_dir`'s direction is used. `do_stitching` stitches after the sweep; `do_merge`
/// merges the swept topology (catching self-intersections).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] (`brep` unchanged) for a [`RegionBrep::Other`] region,
/// a selection entry that is `None` or not live topology of `brep`, and for a
/// zero-length `sweep_dir`; those of the sweep.
#[allow(clippy::too_many_arguments)] // upstream's arguments
pub fn non_manifold_sweep(
    k: &mut Kernel,
    brep: &mut Brep,
    region: Option<RegionBrep<'_>>,
    sweep_dir: Vec3,
    sweep_dist: f64,
    do_stitching: bool,
    do_merge: bool,
    selection: SweepSelection<'_>,
) -> Result<()> {
    // Only pBrepToSweep is stitched, validated and returned: another region B-rep
    // is rejected.
    check_region_brep(region)?;
    // A null, foreign or stale handle crashes or silently corrupts the B-rep below.
    if !selection_is_owned_by(brep, &selection) {
        return Err(SolidError::invalid_input(
            OP_NON_MANIFOLD,
            "selection entry not live topology of the B-rep",
        ));
    }
    let sweep_vec = sweep_dir
        .unitize()
        .map_err(|_| SolidError::invalid_input(OP_NON_MANIFOLD, "zero sweep direction"))?
        * sweep_dist;
    let geometry = SweepGeometry::Translation(TranslationalSweepGeometry::new(sweep_vec));
    non_manifold_sweep_with(k, brep, &geometry, do_stitching, do_merge, selection)
}

/// Sweeps the selected faces, edges and vertices of `brep` (the whole B-rep when
/// nothing is selected) by `angle_deg` degrees about the axis through `base_pt` along
/// `axis`, in place (`SmApiNonManifoldRotationalSweep`, tolerance `SM_ZONE_TOL_3D`).
/// `region`, `do_stitching` and `do_merge` are those of [`non_manifold_sweep`].
///
/// # Errors
///
/// [`SolidError::InvalidInput`] (`brep` unchanged) for a [`RegionBrep::Other`] region
/// or a selection entry that is `None` or not live topology of `brep`; those of the
/// sweep.
#[allow(clippy::too_many_arguments)] // upstream's arguments
pub fn non_manifold_rotational_sweep(
    k: &mut Kernel,
    brep: &mut Brep,
    region: Option<RegionBrep<'_>>,
    base_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
    do_stitching: bool,
    do_merge: bool,
    selection: SweepSelection<'_>,
) -> Result<()> {
    check_region_brep(region)?;
    if !selection_is_owned_by(brep, &selection) {
        return Err(SolidError::invalid_input(
            OP_NON_MANIFOLD,
            "selection entry not live topology of the B-rep",
        ));
    }
    let geometry = SweepGeometry::Rotation(RotationalSweepGeometry::new(
        base_pt,
        axis,
        angle_deg,
        ZONE_TOL_3D,
    ));
    non_manifold_sweep_with(k, brep, &geometry, do_stitching, do_merge, selection)
}

/// A new B-rep filled by `body` (`new SmBrep()`, `SmPrimitiveCreation`, the call).
fn build(
    k: &mut Kernel,
    body: impl FnOnce(&mut PrimitiveCreation, &mut PrimitiveModel<'_>) -> Result<()>,
) -> Result<Brep> {
    PrimitiveModel::build_new(k, body).map(|(mut brep, ())| {
        // (not upstream: the port's cached topology fields, true for the caller)
        brep.refresh_cached_topology();
        brep
    })
}

/// The start, side and end faces of a curve sweep.
type SweepFaces = (Vec<FaceId>, Vec<FaceId>, Vec<FaceId>);

/// A curve sweep into a new B-rep with its face lists.
fn curve_sweep(
    k: &mut Kernel,
    body: impl FnOnce(&mut PrimitiveCreation, &mut PrimitiveModel<'_>, &mut SweepFaces) -> Result<()>,
) -> Result<CurveSweep> {
    curve_sweep_with(k, body).map(|(sweep, ())| sweep)
}

/// [`curve_sweep`] whose `body` also returns a value.
fn curve_sweep_with<R>(
    k: &mut Kernel,
    body: impl FnOnce(&mut PrimitiveCreation, &mut PrimitiveModel<'_>, &mut SweepFaces) -> Result<R>,
) -> Result<(CurveSweep, R)> {
    let (mut brep, ((start_faces, side_faces, end_faces), extra)) =
        PrimitiveModel::build_new(k, |pc, model| {
            let mut faces = SweepFaces::default();
            let extra = body(pc, model, &mut faces)?;
            Ok((faces, extra))
        })?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok((
        CurveSweep {
            brep,
            start_faces,
            side_faces,
            end_faces,
        },
        extra,
    ))
}

/// The `SmBSplineCurve` a path or scale curve is (upstream's parameter type): a
/// B-spline, or the B-spline part of a line or conic derived from it.
fn bspline_of(curve: &Curve, op: &'static str) -> Result<BSplineCurve> {
    match curve.clone().into_kind() {
        CurveKind::BSpline(b) => Ok(b),
        CurveKind::Line(l) => Ok(l.base_with_nurb()?.into_owned()),
        CurveKind::Ellipse(e) => Ok(e.base().clone()),
        CurveKind::Hyperbola(h) => Ok(h.base().clone()),
        CurveKind::Parabola(p) => Ok(p.base().clone()),
        _ => Err(SolidError::invalid_input(
            op,
            "curve is not an SmBSplineCurve",
        )),
    }
}

/// The path, scale-reference and scale curves of a face or edge curve sweep as
/// `SmBSplineCurve`s.
fn sweep_curves(
    path: &Curve,
    scale_reference: Option<&Curve>,
    scale: Option<&Curve>,
    op: &'static str,
) -> Result<(BSplineCurve, Option<BSplineCurve>, Option<BSplineCurve>)> {
    Ok((
        bspline_of(path, op)?,
        scale_reference.map(|c| bspline_of(c, op)).transpose()?,
        scale.map(|c| bspline_of(c, op)).transpose()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::primitive::api_curves;

    fn circle() -> Curve {
        Curve::from(api_curves::create_circle(Point3::new(0.0, 0.0, 0.0), 2.0).expect("circle"))
    }

    fn line() -> Curve {
        Curve::from(
            api_curves::create_line_segment(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0))
                .expect("line"),
        )
    }

    fn hermite() -> Curve {
        let o = Point3::new(0.0, 0.0, 0.0);
        let x = Vec3::new(1.0, 0.0, 0.0);
        Curve::from(HermiteCurve::new(o, x, Point3::new(1.0, 1.0, 0.0), x, 3))
    }

    fn path() -> Curve {
        let pts = [0.0, 3.0, 7.0, 10.0].map(|z| Point3::new(0.0, 0.0, z));
        Curve::from(api_curves::create_curve(&pts).expect("path"))
    }

    fn invalid<T>(r: &Result<T>) -> bool {
        matches!(r, Err(SolidError::InvalidInput { .. }))
    }

    #[test]
    fn consuming_sweeps_copy_then_run_or_defer() {
        let mut k = Kernel::default();
        let (c, l, h) = (circle(), line(), hermite());
        let ok = [Some(&c), Some(&l)];
        let z = Vec3::new(0.0, 0.0, 1.0);
        let o = Point3::new(0.0, 0.0, 0.0);
        // the copies are made and the sweep runs (topology.brep_p4 is ported)
        let b = create_linear_sweep(&mut k, &ok, z, 10.0, true).expect("linear sweep");
        assert_eq!(
            (
                b.face_list().len(),
                b.edge_list().len(),
                b.vertex_list().len()
            ),
            (2, 7, 6)
        );
        // no profile curves: a successful empty sweep, as upstream
        let b = create_linear_sweep(&mut k, &[], z, 10.0, true).expect("empty sweep");
        assert!(b.face_list().is_empty());
        assert!(b.edge_list().is_empty());
        assert!(b.vertex_list().is_empty());
        assert!(invalid(&create_linear_sweep(
            &mut k,
            &[Some(&c), None],
            z,
            1.0,
            false
        )));
        // the circle around the axis sweeps a surface degenerate to a curve
        assert!(invalid(&create_rotational_sweep(
            &mut k, &ok, o, z, 90.0, false
        )));
        // the draft runs through the taper's merges (boolean.merge_p2 is ported)
        let b = create_draft_sweep(&mut k, &ok, 10.0, -5.0, true, 3).expect("draft sweep");
        assert_eq!(
            (
                b.face_list().len(),
                b.edge_list().len(),
                b.vertex_list().len()
            ),
            (2, 3, 2)
        );
        // no curves: CreateTaperExtrude's SM_ERR
        assert!(
            create_draft_sweep(&mut k, &[], 10.0, -5.0, false, 3).is_err_and(|e| !e.is_unported())
        );
        assert!(invalid(&create_draft_sweep(
            &mut k,
            &[None],
            10.0,
            5.0,
            false,
            0
        )));
        // SM_ERR of the Hermite curve's base Copy, before the later NULL is seen
        let err =
            create_rotational_sweep(&mut k, &[Some(&h), None], o, z, 90.0, false).unwrap_err();
        assert!(matches!(
            err,
            SolidError::Upstream {
                code: crate::error::SM_ERR,
                ..
            }
        ));
        assert!(invalid(&create_taper_extrude(
            &mut k,
            &[],
            10.0,
            5.0,
            3,
            false
        )));
        assert!(invalid(&create_taper_extrude(
            &mut k,
            &[Some(&l), None],
            10.0,
            5.0,
            3,
            false
        )));
        let b = create_taper_extrude(&mut k, &ok, 10.0, 5.0, 3, true).expect("taper extrude");
        assert_eq!(
            (
                b.face_list().len(),
                b.edge_list().len(),
                b.vertex_list().len()
            ),
            (2, 3, 2)
        );
        assert!(invalid(&create_linear_sweep_with_repetitions(
            &mut k,
            &[],
            z,
            5.0,
            2,
            false
        )));
        // capped repeated sweeps need exactly one repetition: SM_ERR
        assert!(
            create_linear_sweep_with_repetitions(&mut k, &ok, z, 5.0, 0, true)
                .is_err_and(|e| !e.is_unported())
        );
        assert!(invalid(&create_rotational_sweep_with_repetitions(
            &mut k,
            &[],
            o,
            z,
            5.0,
            2,
            false
        )));
        assert!(invalid(&create_rotational_sweep_with_repetitions(
            &mut k, &ok, o, z, 5.0, 2, false
        )));
    }

    #[test]
    fn pipe_and_planar_path_validate_then_run_or_defer() {
        let mut k = Kernel::default();
        let p = path();
        assert!(invalid(&create_pipe_sweep(&mut k, 1.0, None, true)));
        assert!(invalid(&create_pipe_sweep(&mut k, 0.0, Some(&p), true)));
        assert!(invalid(&create_pipe_sweep(&mut k, -1.0, Some(&p), false)));
        // a NaN radius passes the API's validation; upstream returns an empty B-rep
        let b = create_pipe_sweep(&mut k, f64::NAN, Some(&p), false).expect("nan pipe");
        assert!(b.face_list().is_empty());
        assert!(b.edge_list().is_empty());
        assert!(b.vertex_list().is_empty());
        let b = create_pipe_sweep(&mut k, 1.0, Some(&p), false).expect("pipe");
        assert_eq!(b.face_list().len(), 1);
        let capped = create_pipe_sweep(&mut k, 1.0, Some(&p), true).expect("capped pipe");
        assert!(crate::topology::brep::p2::is_manifold_solid(&capped).unwrap());

        let (c, l) = (circle(), line());
        assert!(invalid(&create_sweep_along_planar_path(
            &mut k,
            &[],
            &[&l],
            true,
            3
        )));
        assert!(invalid(&create_sweep_along_planar_path(
            &mut k,
            &[&c],
            &[],
            true,
            3
        )));
        assert!(invalid(&create_sweep_along_planar_path(
            &mut k,
            &[&c],
            &[&l],
            true,
            -1
        )));
        assert!(invalid(&create_sweep_along_planar_path(
            &mut k,
            &[&c],
            &[&l],
            true,
            4
        )));
        for caps in 0..=3 {
            let result = create_sweep_along_planar_path(&mut k, &[&c], &[&l], false, caps)
                .expect("planar path");
            let cap_count = (caps & 1) + ((caps >> 1) & 1);
            assert_eq!(
                (
                    result.face_list().len(),
                    result.edge_list().len(),
                    result.vertex_list().len()
                ),
                (1 + cap_count as usize, 3, 2)
            );
            assert_eq!(
                crate::topology::brep::p2::is_manifold_solid(&result).unwrap(),
                caps == 3
            );
        }
    }

    #[test]
    fn curve_sweeps_validate_then_run_or_defer() {
        let mut k = Kernel::default();
        let p = path();
        let c = circle();
        let opts = SweepOptions {
            cap_ends: true,
            ..SweepOptions::default()
        };
        assert!(invalid(&create_curve_sweep(
            &mut k,
            &[],
            Some(&p),
            None,
            None,
            None
        )));
        assert!(invalid(&create_curve_sweep(
            &mut k,
            &[Some(&c)],
            None,
            None,
            None,
            None
        )));
        let swept = create_curve_sweep(&mut k, &[Some(&c)], Some(&p), Some(&p), None, Some(&opts))
            .expect("capped curve sweep");
        assert!(crate::topology::brep::p2::is_manifold_solid(&swept.brep).unwrap());
        // no B-spline profile left once the NULL entry is skipped
        assert!(invalid(&create_curve_sweep(
            &mut k,
            &[None],
            Some(&p),
            None,
            None,
            None
        )));
        assert!(invalid(&create_curve_sweep_from_faces(
            &mut k,
            &[],
            Some(&p),
            None,
            None,
            None
        )));
        assert!(invalid(&create_curve_sweep_from_edges(
            &mut k,
            &[],
            Some(&p),
            None,
            None,
            None
        )));
    }

    /// The surface kinds of a B-rep's faces, in face order.
    fn face_kinds(b: &Brep) -> Vec<&'static str> {
        use crate::surface::surface::SurfaceKind;
        b.face_list()
            .iter()
            .map(
                |&f| match b.face(f).unwrap().surface.as_ref().unwrap().kind() {
                    SurfaceKind::Cone(c) if c.is_cylinder_class() => "cylinder",
                    SurfaceKind::Cone(_) => "cone",
                    SurfaceKind::Sphere(_) => "sphere",
                    SurfaceKind::Torus(_) => "torus",
                    SurfaceKind::Plane(_) => "plane",
                    SurfaceKind::SurfOfRevolution(_) => "revolution",
                    _ => "other",
                },
            )
            .collect()
    }

    #[test]
    fn a_whole_turn_of_a_profile_gives_each_curve_its_analytic_face() {
        // Not upstream (WR-448, #545, `fixes.revolve_analytic_profile_faces`): upstream
        // makes an SmSurfOfRevolution of every curve of a profile longer than one curve
        let mut k = Kernel::default();
        let seg =
            |a: Vec3, b: Vec3| Curve::from(BSplineCurve::create_line_segment(3, a, b).unwrap());
        let arc = |c: Vec3, x: Vec3, y: Vec3, r: f64, s: f64, e: f64| {
            Curve::from(
                BSplineCurve::create_circle_segment(
                    3,
                    &Axis2Placement::new(c, x, y),
                    r,
                    s,
                    e,
                    crate::nurbs::b_spline_curve::NurbCircleParam::Quadratic,
                )
                .unwrap(),
            )
        };
        // a drilled hole: its wall and the drill point
        let wall = seg(Vec3::new(2.0, 0.0, 5.0), Vec3::new(2.0, 0.0, 1.0));
        let point = seg(Vec3::new(2.0, 0.0, 1.0), Vec3::ZERO);
        // a dome on its disc
        let dome = arc(Vec3::ZERO, Vec3::X, Vec3::Z, 3.0, 0.0, 90.0);
        let rim = seg(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0));
        // a torus from two half circles
        let outer = arc(Vec3::new(5.0, 0.0, 0.0), -Vec3::Z, Vec3::X, 1.0, 0.0, 180.0);
        let inner = arc(Vec3::new(5.0, 0.0, 0.0), Vec3::Z, -Vec3::X, 1.0, 0.0, 180.0);
        // by default (the fix off), upstream's surfaces of revolution
        for (fix, expected) in [
            (false, ["revolution", "revolution"]),
            (true, ["cylinder", "cone"]),
        ] {
            k.fixes.revolve_analytic_profile_faces = fix;
            let b = create_rotational_sweep(
                &mut k,
                &[Some(&wall), Some(&point)],
                Vec3::ZERO,
                Vec3::Z,
                360.0,
                false,
            )
            .unwrap();
            assert_eq!(face_kinds(&b), expected);
        }
        for (curves, expected) in [
            ([Some(&dome), Some(&rim)], ["sphere", "plane"]),
            ([Some(&outer), Some(&inner)], ["torus", "torus"]),
        ] {
            let b = create_rotational_sweep(&mut k, &curves, Vec3::ZERO, Vec3::Z, 360.0, false)
                .unwrap();
            assert_eq!(face_kinds(&b), expected);
            assert!(crate::topology::brep::p2::is_manifold_solid(&b).unwrap());
        }
    }

    /// A quarter disc of radius 20 turned a whole turn about z: a dome. Its arc is a
    /// sketch's arc through (20, 0), a rounded (14.142136, 14.142136) and (0, 20), whose
    /// centre (c, 0, c), c = 1.28e-6, is off the axis by far less than the B-rep
    /// tolerance (source rig shell/4847, #1123).
    fn rounded_quarter_disc(k: &mut Kernel, fix: bool) -> Brep {
        k.fixes.revolve_analytic_profile_faces = true;
        k.fixes.revolve_arc_centre_near_axis_is_sphere = fix;
        let (c, r, a0) = (
            1.2846627950203038e-06,
            19.99999871533725,
            -6.42331438769116e-08,
        );
        let to_deg = 180.0 / std::f64::consts::PI;
        let arc = Curve::from(
            BSplineCurve::create_circle_segment(
                3,
                &Axis2Placement::new(Vec3::new(c, 0.0, c), Vec3::X, Vec3::Z),
                r,
                a0 * to_deg,
                (a0 + 1.5707964552611844) * to_deg,
                crate::nurbs::b_spline_curve::NurbCircleParam::Quadratic,
            )
            .unwrap(),
        );
        let seg =
            |a: Vec3, b: Vec3| Curve::from(BSplineCurve::create_line_segment(3, a, b).unwrap());
        let rim = seg(Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0));
        let axis = seg(Vec3::new(0.0, 0.0, 20.0), Vec3::ZERO);
        create_rotational_sweep(
            k,
            &[Some(&rim), Some(&arc), Some(&axis)],
            Vec3::ZERO,
            Vec3::Z,
            360.0,
            false,
        )
        .unwrap()
    }

    #[test]
    fn issue_1123_arc_centred_within_tolerance_of_the_axis_is_a_sphere_with_its_pole_on_it() {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};
        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        for fix in [false, true] {
            let mut k = Kernel::default();
            let mut b = rounded_quarter_disc(&mut k, fix);
            let dome = *b.face_list().last().unwrap();
            let surface = b.face(dome).unwrap().surface.as_ref().unwrap();
            let d = surface.natural_uv_domain_if_built();
            // the farthest the dome's surface strays from the sphere of radius 20 about the origin
            let mut stray = 0.0_f64;
            for i in 0..=8 {
                for j in 0..=8 {
                    let p = surface
                        .evaluate_point(&k, d.evaluate(f64::from(i) / 8.0, f64::from(j) / 8.0))
                        .unwrap();
                    stray = stray.max((p.length() - 20.0).abs());
                }
            }
            let pole_off = crate::topology::brep::p1::vertices(&b, None)
                .unwrap()
                .iter()
                .map(|&v| crate::topology::vertex::point(&b, v).unwrap())
                .filter(|p| p.z > 10.0)
                .map(|p| p.x.hypot(p.y))
                .fold(0.0, f64::max);
            let volume = brep_volume(&k, &mut b, 1e-8, &attributes).unwrap();
            // 2/3 pi 20^3
            let want = 2.0 / 3.0 * std::f64::consts::PI * 8000.0;
            if fix {
                assert_eq!(face_kinds(&b), ["plane", "sphere"]);
                assert!(stray <= 1e-9, "dome strays {stray} from its sphere");
                assert!(pole_off <= 1e-12, "pole {pole_off} off the axis");
                assert!((volume - want).abs() <= 1e-6 * want, "{volume} vs {want}");
                assert!(crate::topology::brep::p2::is_manifold_solid(&b).unwrap());
            } else {
                // the arc's own circle: a torus 1.28e-6 about the axis, off the sphere
                assert_eq!(face_kinds(&b), ["plane", "torus"]);
                assert!(stray > 1e-7, "{stray}");
            }
        }
    }
}
