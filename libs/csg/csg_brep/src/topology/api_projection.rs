//! `SM_API` projections: a B-rep's visible edges projected onto a plane, its silhouette
//! curves, and a curve projected onto a B-rep or a surface.
//!
//! Upstream: `source/SM_API/src/SmApiBrep.cpp`, `source/SM_API/src/SmApiCurves.cpp`,
//! `source/SM_API/inc/SmApiBrep.h`, `source/SM_API/inc/SmApiCurves.h`;
//! `source/SMLib/src/SmCrvOnSurf.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                               | Upstream                                          |
//! |------------------------------------|---------------------------------------------------|
//! | [`project_brep_onto_plane`]        | `SmApiProjectBrepOntoPlane`                       |
//! | [`create_silhouette_curves`]       | `SmApiCreateSilhouetteCurves`                     |
//! | [`project_curve_onto_brep`]        | `SmApiProjectCurve(SmBrep*, SmBSplineCurve*, ..)` |
//! | [`project_curve_onto_surface`], [`SurfaceProjection`] | `SmApiProjectCurve(const SmBSplineCurve*, const SmSurface*, ..)` |
//! | `plane_projection` (private)       | `SmCurve::CreatePlaneProjection` (virtual dispatch: the `SmBSplineCurve` override, `SM_ERR` otherwise) |
//! | `crv_on_surf_plane_projection` (private) | `SmCrvOnSurf::CreatePlaneProjection`        |
//!
//! Deviations:
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`].
//! - **Null pointers.** A null B-rep, curve or surface is `None`. Where upstream
//!   dereferences a null B-rep (`SmApiCreateSilhouetteCurves` with a usable normal) the
//!   port returns [`SolidError::InvalidInput`].
//! - **Statuses.** The `SM_ERR_INVALID_INPUT` and `SM_ERR` the wrappers return themselves
//!   are [`SolidError::Upstream`] with that code.
//! - **Output arrays.** A caller's `SmTArray<SmCurve*>` is a `Vec<Option<Curve>>`;
//!   upstream's appends and restores on failure are kept (an error leaves it as it
//!   was). The output slots are [`add_curve_output_slots`] (`SmApiAddCurveOutputSlots`).
//! - **HCR scene.** The `SmHCR` scene copy notifies through a private arena-only store,
//!   with the source in a key namespace of its own (as `sweep::topology_sweep` builds its
//!   HCR sessions): upstream's scene is a private copy nothing else sees.
//! - **Curve projections.** `SmCurve::CreatePlaneProjection` and its `SmCrvOnSurf`
//!   override are kept beside their only callers here (no unit owns them); every
//!   `SmBSplineCurve`-derived kind projects the NURB its kind makes. A kind without its
//!   NURB makes it with the curve's own owner, as upstream's virtual `MakeNurb` does on
//!   the object (an owned ellipse's edge tolerance sets its arc tolerance), but on a copy,
//!   since the curve is shared. The edge tolerance an owned ellipse needs is the one
//!   recorded on the curve ([`Curve::owner_zone_tol_3d`], as the curve cache reads it):
//!   the bare curve cannot reach its edge, so an ellipse owned through an edge, edge use
//!   or curve without that record is [`SolidError::InvalidInput`] (upstream reads
//!   `GetEdge()->GetTolerance()`). The approximation tolerance of a curve on a surface
//!   (`SmTol::GetApproxTol3d(this)`) reads the same record the same way.
//! - **Curve onto a surface.** The drop reads the `SmBSplineCurve` through an exact
//!   [`Curve`] wrapper (`clone_exact`: its knots untouched).
//! - `SM_ASSERT_VALID` and the `SM_DEBUG_CODE` drawing are not ported.
//! - **Recursion.** `plane_projection` of a curve on a surface projects the lifted
//!   B-spline once (depth 2).

use crate::blend::hcr::{Hcr, HcrOptions};
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError, SM_ERR, SM_ERR_INVALID_INPUT};
use crate::geom::assert_array::labels::{EDGEUSE_TYPE, EDGE_TYPE};
use crate::geom::attribute::ObjectKey;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent3d::Extent3d;
use crate::geom::object::ObjectRef;
use crate::geom::tol::{zone_tol_3d, SmallTopoType, ZONE_TOL_3D};
use crate::kernel::Kernel;
use crate::math::{min2, Point3, Vec3, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest, ProjectionType};
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::api_files::add_curve_output_slots;
use crate::topology::brep::p1::{create_parallel_projection_curves, ParallelProjectionOptions};
use crate::topology::brep::p2::ArenaNotifyState;
use crate::topology::Brep;
use crate::EntityRef;

const OP: &str = "topology::api_projection";
const OP_PROJ: &str = "topology::api_projection::plane_projection";

/// An `SmApiStatus` the wrapper returns itself.
const fn status(code: i32) -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code,
        entity: None,
    }
}

/// `sNormal.Unitize() != SM_SUCCESS` returns `SM_ERR_INVALID_INPUT`.
fn unit_normal(normal: Vec3) -> Result<Vec3> {
    normal.unitize().map_err(|_| status(SM_ERR_INVALID_INPUT))
}

/// `pBrep->CalculateBoundingBox(BBox)` with its status ignored (the box then stays
/// empty), and the length of its diagonal.
fn box_and_length(k: &Kernel, brep: &Brep) -> Result<(Extent3d, f64)> {
    let bbox = ignored(qbrep::calculate_bounding_box(
        k,
        brep,
        BoundingBoxOptions::default(),
    ))?
    .unwrap_or_else(Extent3d::new);
    let diagonal = bbox.size();
    let length = diagonal.length();
    Ok((bbox, length))
}

/// The identity of the source B-rep's objects for the HCR scene copy: in a key
/// namespace of its own.
fn source_ref(e: EntityRef) -> ObjectRef {
    let r = crate::topology::loopuse::notify_ref(e);
    ObjectRef {
        type_code: r.type_code,
        key: ObjectKey(r.key.0 | 1 << 40),
    }
}

/// The view of an `SmHCR`: its two constructors.
enum View<'a> {
    /// `SmHCR(.., const SmVector3d& crViewVector, ..)`.
    Vector(Vec3),
    /// `SmHCR(.., const SmAxis2Placement& crViewTransformation, ..)`.
    Transform(&'a Axis2Placement),
}

/// `SmHCR sHCR(ctx, pBrep, view, approxTol, angTol, TRUE, FALSE)`, then
/// `ComputeGlobalVisibility` (its status returned) and
/// `GetVisibleCurves`.
fn visible_curves(
    k: &mut Kernel,
    brep: &Brep,
    view: &View<'_>,
    approx_tol: f64,
    ang_tol_rad: f64,
) -> Result<Vec<Curve>> {
    let opts = HcrOptions {
        approx_tol_3d: approx_tol,
        ang_tol_rad,
        display_seams: true,
        display_smooth_edges: false,
    };
    let mut state = ArenaNotifyState::default();
    let mut hcr = match *view {
        View::Vector(vector) => Hcr::new(k, &mut state.env(None), brep, &source_ref, vector, opts)?,
        View::Transform(transform) => {
            Hcr::with_transform(k, &mut state.env(None), brep, &source_ref, transform, opts)?
        }
    };
    hcr.compute_global_visibility(k)?;
    Ok(hcr.visible_curves().to_vec())
}

/// `SmApiProjectBrepOntoPlane`: the visible edges of `brep`, looking along
/// `-plane_normal` from beyond its box (`SmHCR` of the inverse of a plane through the
/// box centre moved back by the box diagonal), each projected parallel to
/// `-plane_normal` onto the plane through `plane_point` with normal `plane_normal` (any
/// length; it is unitized). The projections are appended to `out`; on an error `out`
/// is left as it was.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no B-rep or a zero normal; those of the HCR and of the
/// curve projections.
pub fn project_brep_onto_plane(
    k: &mut Kernel,
    brep: Option<&Brep>,
    plane_point: Vec3,
    plane_normal: Vec3,
    out: &mut Vec<Option<Curve>>,
) -> Result<()> {
    let Some(brep) = brep else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };

    let plane_normal = unit_normal(plane_normal)?;

    let (bbox, length) = box_and_length(k, brep)?;

    let [_x, y, z] = plane_normal.make_unit_ortho_vectors(None)?;

    let plane_origin = bbox.mid() - plane_normal * length;

    let mut plane = Axis2Placement::default();
    // (status ignored upstream)
    let _ = plane.set_canonical(plane_origin, y, z);

    let invert_plane = plane.invert();

    let mut approx_tol = length * 1.0e-6;
    approx_tol = min2(approx_tol, brep.tolerance() * 0.1);
    let angle_tolerance_radians = 10.0 * PI / 180.0;

    let visible = visible_curves(
        k,
        brep,
        &View::Transform(&invert_plane),
        approx_tol,
        angle_tolerance_radians,
    )?;

    let direction = Vec3::new(-plane_normal.x, -plane_normal.y, -plane_normal.z);
    let size_on_entry = out.len();

    for curve in &visible {
        match plane_projection(k, curve, plane_point, plane_normal, direction) {
            Ok(projected) => out.push(Some(projected)),
            Err(e) => {
                // Drop this call's curves rather than return a partial projection.
                out.truncate(size_on_entry);
                return Err(e);
            }
        }
    }
    Ok(())
}

/// `SmApiCreateSilhouetteCurves`: copies of the visible edge and silhouette curves of
/// `brep` looking along `plane_normal` (any length; it is unitized), appended to `out`
/// in new output slots. `plane_point` is unused (upstream only draws it). On an error
/// `out` is left as it was.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for a zero normal; [`SolidError::InvalidInput`] for no B-rep
/// (upstream dereferences it); those of the HCR, of the output slots and of the curve
/// copies.
pub fn create_silhouette_curves(
    k: &mut Kernel,
    brep: Option<&Brep>,
    plane_point: Vec3,
    plane_normal: Vec3,
    out: &mut Vec<Option<Curve>>,
) -> Result<()> {
    let _ = plane_point; // only referenced inside SM_DEBUG_CODE draw upstream

    let plane_normal = unit_normal(plane_normal)?;

    let brep = brep
        .ok_or_else(|| SolidError::invalid_input(OP, "SmApiCreateSilhouetteCurves: null B-rep"))?;
    let (_, length) = box_and_length(k, brep)?;

    let approx_tol = length * 1.0e-4;
    let angle_tolerance_radians = 10.0 * PI / 180.0;

    let hcr_curves = visible_curves(
        k,
        brep,
        &View::Vector(plane_normal),
        approx_tol,
        angle_tolerance_radians,
    )?;

    let start_size = out.len();
    add_curve_output_slots(out, hcr_curves.len() as u64)?;
    for (ii, curve) in hcr_curves.iter().enumerate() {
        match curve.copy(k) {
            Ok(copy) => {
                if let Some(slot) = out.get_mut(start_size + ii) {
                    *slot = Some(copy);
                }
            }
            Err(e) => {
                out.truncate(start_size);
                return Err(e);
            }
        }
    }
    Ok(())
}

/// `SmApiProjectCurve(SmBrep*, SmBSplineCurve*, ..)`: the 3D curves of `curve` projected
/// along `projection_vector` onto the faces of `brep` (`CreateParallelProjectionCurves`
/// with its defaults), appended to `out` in new output slots. On an error, including a
/// projection that makes no curve, `out` is left as it was.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no B-rep or curve; `SM_ERR` for no projected curve; those
/// of [`create_parallel_projection_curves`] and of the output slots.
pub fn project_curve_onto_brep(
    k: &Kernel,
    brep: Option<&mut Brep>,
    curve: Option<&BSplineCurve>,
    projection_vector: Vec3,
    out: &mut Vec<Option<Curve>>,
) -> Result<()> {
    let (Some(brep), Some(curve)) = (brep, curve) else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };

    // The kernel resets its output array. Keep caller-owned curves out of it.
    // (`CreateParallelProjectionCurves` takes the curve as an SmCurve)
    let curve = Curve::from(curve.clone_exact());
    let projected = create_parallel_projection_curves(
        k,
        brep,
        &curve,
        projection_vector,
        &ParallelProjectionOptions {
            want_3d: true,
            ..ParallelProjectionOptions::default()
        },
    )?
    .curves_3d
    .unwrap_or_default();

    // check result
    if projected.is_empty() {
        return Err(status(SM_ERR));
    }

    let start_size = out.len();
    add_curve_output_slots(out, projected.len() as u64)?;
    for (ii, c) in projected.into_iter().enumerate() {
        if let Some(slot) = out.get_mut(start_size + ii) {
            *slot = Some(c);
        }
    }
    Ok(())
}

/// What [`project_curve_onto_surface`] appends to (upstream's optional output arrays).
#[derive(Debug, Default)]
pub struct SurfaceProjection<'a> {
    /// `pOptCurves2d`: the UV curves on the surface.
    pub curves_2d: Option<&'a mut Vec<Curve>>,
    /// `pOptCurves3d`: the 3D curves on the surface (one per UV curve).
    pub curves_3d: Option<&'a mut Vec<Curve>>,
}

/// `SmApiProjectCurve(const SmBSplineCurve*, const SmSurface*, ..)`: `curve` dropped
/// onto `surface` over its natural UV domain (`DropAndTrimCurve` at the length of the
/// curve's box diagonal, at least `1`, keeping every drop curve) and, when asked, each
/// UV curve lifted (`LiftCurve` at `SM_ZONE_TOL_3D`). The curves are appended to the
/// outputs only on full success.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no curve or surface; those of `DropAndTrimCurve` and of
/// `LiftCurve`.
pub fn project_curve_onto_surface(
    k: &Kernel,
    curve: Option<&BSplineCurve>,
    surface: Option<&Surface>,
    outputs: SurfaceProjection<'_>,
) -> Result<()> {
    let (Some(curve), Some(surface)) = (curve, surface) else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };

    let interval = curve.natural_interval();
    let crv_bbox = ignored(curve.calculate_bounding_box(
        &interval,
        BoundingBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        },
    ))?
    .and_then(|b| b.normal)
    .unwrap_or_else(Extent3d::new);
    let mut approx_tol = crv_bbox.size().length();
    if approx_tol < 1.0 {
        approx_tol = 1.0;
    }

    // the drop reads the curve through the `SmCurve` interface: an exact wrapper (its
    // knots untouched)
    let curve = Curve::from(curve.clone_exact());
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    let curves_on_srf_2d = surface
        .drop_and_trim_curve::<NoObject>(
            k, &domain, &curve, &interval, approx_tol, true, false, None,
        )?
        .uv_curves;

    // Lift into a local array so the outputs are only touched on full success.
    let mut curves_3d = Vec::new();
    if outputs.curves_3d.is_some() {
        for uv in &curves_on_srf_2d {
            let lifted =
                surface.lift_curve(k, &domain, uv, &uv.natural_interval(), ZONE_TOL_3D, false)?;
            curves_3d.push(lifted.curve);
        }
    }

    if let Some(out) = outputs.curves_2d {
        out.extend(curves_on_srf_2d);
    }
    if let Some(out) = outputs.curves_3d {
        out.extend(curves_3d);
    }
    Ok(())
}

/// `SmCurve::CreatePlaneProjection(ctx, SM_PT_PARALLEL, point, normal, direction)`:
/// the `SmBSplineCurve` override for every kind derived from it (on the NURB the kind
/// makes with the curve's owner, on a copy when it has none yet), the `SmCrvOnSurf` override for a curve on a
/// surface (and an `SmTangentField`, which inherits it); `SM_ERR` otherwise (the
/// `SmCurve` base).
fn plane_projection(
    k: &Kernel,
    curve: &Curve,
    plane_point: Point3,
    plane_normal: Vec3,
    direction: Vec3,
) -> Result<Curve> {
    let bs = |b: &BSplineCurve| {
        b.create_plane_projection(
            ProjectionType::Parallel,
            plane_point,
            plane_normal,
            direction,
        )
        .map(Curve::from)
    };
    // A kind without its NURB makes it as upstream's virtual `MakeNurb` does on the
    // object itself, with the curve's own owner (an owned ellipse's edge tolerance sets
    // its arc tolerance), on a copy: the caller's curve is shared.
    let owner = curve.owner();
    match curve.kind() {
        CurveKind::BSpline(c) => bs(c),
        CurveKind::Line(c) => {
            if c.base().gw_nurb().is_some() {
                bs(c.base())
            } else {
                let mut n = c.copy();
                n.make_nurb_owned(owner)?;
                bs(n.base())
            }
        }
        CurveKind::Ellipse(c) => {
            if c.base().gw_nurb().is_some() {
                bs(c.base())
            } else {
                // GetEdge()->GetTolerance(): the edge tolerance recorded on the curve; a
                // curve owned through an edge, edge use or curve without that record
                // cannot reach its edge from here
                let edge_tol = curve.owner_zone_tol_3d();
                if edge_tol.is_none() && owner.is_some_and(owner_reaches_edge) {
                    return Err(SolidError::invalid_input(
                        OP_PROJ,
                        "owned ellipse without its edge tolerance recorded",
                    ));
                }
                let mut n = c.copy(k)?;
                n.make_nurb_owned_with_edge_tol(k, owner, edge_tol)?;
                bs(n.base())
            }
        }
        CurveKind::Parabola(c) => {
            if c.base().gw_nurb().is_some() {
                bs(c.base())
            } else {
                let mut n = c.copy();
                n.make_nurb(owner)?;
                bs(n.base())
            }
        }
        CurveKind::Hyperbola(c) => {
            if c.base().gw_nurb().is_some() {
                bs(c.base())
            } else {
                let mut n = c.copy();
                n.make_nurb(owner)?;
                bs(n.base())
            }
        }
        // SmTol::GetApproxTol3d(this) = .5 * GetZoneTol3d(this)
        CurveKind::CrvOnSurf(c) => {
            let tol = 0.5 * curve_zone_tol_3d(k, curve)?;
            crv_on_surf_plane_projection(k, c, tol, plane_point, plane_normal, direction)
        }
        CurveKind::TangentField(c) => {
            let tol = 0.5 * curve_zone_tol_3d(k, curve)?;
            crv_on_surf_plane_projection(k, c.base(), tol, plane_point, plane_normal, direction)
        }
        CurveKind::Hermite(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::Conic(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::Composite(_) => Err(SolidError::sm_err(OP_PROJ)),
    }
}

/// `SmTol::GetZoneTol3d(const SmObject*)` of a curve: its edge's tolerance, as recorded
/// on the curve ([`Curve::owner_zone_tol_3d`]), else the system zone tolerance for a
/// curve no edge owns.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a curve owned through an edge, edge use or curve
/// without that record (the bare curve cannot reach its edge).
fn curve_zone_tol_3d(k: &Kernel, curve: &Curve) -> Result<f64> {
    if let Some(tol) = curve.owner_zone_tol_3d() {
        return Ok(tol);
    }
    if curve.owner().is_some_and(owner_reaches_edge) {
        return Err(SolidError::invalid_input(
            OP_PROJ,
            "owned curve without its edge tolerance recorded",
        ));
    }
    Ok(zone_tol_3d(k, SmallTopoType::NormalSize))
}

/// Whether an owner chain from `owner` leads to an edge (`SmCurve::GetEdge`): the owner
/// is an edge, an edge use or a curve.
fn owner_reaches_edge(owner: ObjectRef) -> bool {
    owner.type_code == EDGE_TYPE
        || owner.type_code == EDGEUSE_TYPE
        || crate::curve::curve::p2::is_kind_of(owner.type_code)
}

/// `SmCrvOnSurf::CreatePlaneProjection`: the curve lifted to a B-spline (`LiftCurve`
/// of its UV curve, an `SmBSplineCurve`, over the UV curve's natural interval at
/// `approx_tol`, the curve's `SmTol::GetApproxTol3d`), then that curve's projection.
fn crv_on_surf_plane_projection(
    k: &Kernel,
    c: &CrvOnSurf,
    approx_tol: f64,
    plane_point: Point3,
    plane_normal: Vec3,
    direction: Vec3,
) -> Result<Curve> {
    let uv = c.uv_curve();
    // SM_CAST_PTR(SmBSplineCurve, m_pUVCurve); NER
    if !matches!(
        uv.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Parabola(_)
            | CurveKind::Hyperbola(_)
    ) {
        return Err(SolidError::null_pointer(OP_PROJ));
    }

    // Lift this curve to a BSplineCurve
    let lifted = c.surface().lift_curve(
        k,
        c.uv_domain(),
        uv,
        &uv.natural_interval(),
        approx_tol,
        false,
    )?;

    // Pass along the call to SmBSplineCurve::CreatePlaneProjection
    plane_projection(k, &lifted.curve, plane_point, plane_normal, direction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(r: &Result<()>) -> Option<i32> {
        match r {
            Err(SolidError::Upstream { code, .. }) => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn null_and_zero_inputs() {
        let mut k = Kernel::default();
        let mut out = vec![None];
        let r = project_brep_onto_plane(&mut k, None, Vec3::ZERO, Vec3::Z, &mut out);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        let b = crate::primitive::api_primitives::create_box(&mut k, Vec3::ZERO, 1.0, 2.0, 3.0)
            .unwrap();
        let r = project_brep_onto_plane(&mut k, Some(&b), Vec3::ZERO, Vec3::ZERO, &mut out);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        let r = create_silhouette_curves(&mut k, None, Vec3::ZERO, Vec3::ZERO, &mut out);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        let r = create_silhouette_curves(&mut k, None, Vec3::ZERO, Vec3::Z, &mut out);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        let r = project_curve_onto_brep(&k, None, None, Vec3::Z, &mut out);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        let r = project_curve_onto_surface(&k, None, None, SurfaceProjection::default());
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn an_owned_conic_without_nurb_projects_the_nurb_of_its_owner() {
        use crate::curve::ellipse::Ellipse;
        let k = Kernel::default();
        let frame = Axis2Placement::default();
        let owner = Some(ObjectRef {
            type_code: 16016,
            key: ObjectKey(7),
        });
        let tol = 1.0e-3;
        let (point, normal, direction) = (Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 0.0, -1.0));

        // the circle's NURB as its edge owner makes it
        let mut reference = Ellipse::create_canonical_circle(&k, &frame, 2.0, None, None).unwrap();
        reference
            .make_nurb_owned_with_edge_tol(&k, owner, Some(tol))
            .unwrap();
        let expected = reference
            .base()
            .create_plane_projection(ProjectionType::Parallel, point, normal, direction)
            .unwrap();

        // the same circle without its NURB, owned by that edge (tolerance recorded)
        let mut bare = Ellipse::create_canonical_circle(&k, &frame, 2.0, None, None).unwrap();
        bare.base_mut().nurb = None;
        let mut curve = Curve::from(bare);
        let _ = curve.set_owner(owner, None);
        curve.set_owner_zone_tol_3d(Some(tol));
        assert!(matches!(curve.kind(), CurveKind::Ellipse(e) if e.base().gw_nurb().is_none()));

        let projected = plane_projection(&k, &curve, point, normal, direction).unwrap();
        let CurveKind::BSpline(p) = projected.kind() else {
            panic!("the projection is a B-spline");
        };
        assert_eq!(
            p.control_points().unwrap(),
            expected.control_points().unwrap()
        );
        // without the recorded edge tolerance the edge cannot be reached: an error, not a
        // deferral and not an unowned NURB
        let mut bare = Ellipse::create_canonical_circle(&k, &frame, 2.0, None, None).unwrap();
        bare.base_mut().nurb = None;
        let mut curve = Curve::from(bare);
        let _ = curve.set_owner(owner, None);
        let r = plane_projection(&k, &curve, point, normal, direction);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }
}
