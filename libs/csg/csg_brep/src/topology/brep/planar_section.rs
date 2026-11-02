//! Planar sections: the curves where an infinite plane cuts a surface
//! ([`surface_planar_section_curves`]) or the faces of a B-rep
//! ([`create_planar_section_curves`]), with the bounded plane they intersect
//! ([`create_plane_from_bbox`]).
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/src/SmSurface.cpp`,
//! `source/SMLib/inc/SmBrep.h`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust                                  | Upstream                                     |
//! |---------------------------------------|----------------------------------------------|
//! | [`create_planar_section_curves`], [`PlanarSectionOptions`], [`SectionOutputs`], [`PlanarSection`] | `SmBrep::CreatePlanarSectionCurves` |
//! | [`surface_planar_section_curves`], [`face_planar_section_curves`], [`SurfaceSection`] | `SmSurface::CreatePlanarSectionCurves` (no override) |
//! | [`create_plane_from_bbox`]            | `SmSurface::CreatePlaneFromBBox` (static)    |
//!
//! Deviations:
//! - **Outputs.** The optional output arrays are the [`SectionOutputs`] flags of
//!   [`PlanarSectionOptions`] and `Option` fields of the result; the curves are owned
//!   values, so upstream's delete lists and cleanup objects have no counterpart. The
//!   faces are [`FaceId`]s.
//! - **Face context.** A face's surface is intersected, dropped onto and lifted as an
//!   evaluation copy (keeping its owner and cache slot) through the B-rep's
//!   [`BrepFaceServices`], so its trimmed cache reaches the face as upstream's does
//!   through `GetFace()`. `smsurf_GetSurfaceCache(this)` before the intersection is that
//!   context's [`SurfaceSolveContext::surface_cache_state`] (upstream only reads the
//!   cache's continuity there, and ignores it).
//! - **Statuses.** A face whose surface section fails is skipped, as upstream's
//!   `continue`; so is a face whose trimming fails. [`SolidError::Unported`] always
//!   propagates. `GlobalCoincidenceChecker`'s status is ignored, as upstream: its
//!   solutions are read as a failed check left them
//!   ([`Curve::global_coincidence_checker_with_partial`]). `IsNurbSurfacePlane` failing inside (upstream's `SER` there reads as a
//!   `TRUE` result with no plane) leaves no plane, so the surface has no section.
//! - **Getters.** Upstream's getters that report no status (`IsPlanar`, `IsDegenerate`,
//!   `IsEmbedded`, `GetEdges`, `GetCurve`, `GetInterval`) propagate only
//!   [`SolidError::Unported`] and [`SolidError::NotFound`] here; any other failure reads
//!   as `false` or an empty list or interval. A face edge without curve (upstream
//!   dereferences it) is `SM_ERR_NULL_POINTER`.
//! - **Forced UV curves.** As upstream, the replacement of a 3D section curve by the
//!   lift of its first drop is tied to the first drop curve (`kk == 0`), even when that
//!   one is rejected and a later one is kept (that one is then added).
//! - **Not ported here.** The `SM_DEBUG_CODE` drawing blocks and
//!   `SM_VALIDATE_INTERSECTORS`.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::SolutionType;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_drop_lib::p3::DropSolveContext;
use crate::kernel::Kernel;
use crate::math::{max2, max4, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::query::brep::BrepFaceServices;
use crate::query::face::p3::{surface_bounding_box, surface_is_planar};
use crate::surface::plane::Plane;
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::brep::p1 as brep1;
use crate::topology::brep_cache::CacheObject;
use crate::topology::edge;
use crate::topology::face::{p2 as face2, p4 as face4};
use crate::topology::{Brep, FaceId};

const OP_PLANE: &str = "topology::brep::planar_section::create_plane_from_bbox";
const OP_SECTION: &str = "topology::brep::planar_section::create_planar_section_curves";

/// `SmSurface::CreatePlaneFromBBox`: the plane through `plane_point` with normal
/// `plane_normal`, bounded to a square twice the size of the bounding sphere of `bbox`
/// around the sphere centre's projection, as the analytic plane `IsNurbSurfacePlane`
/// makes of the bilinear surface through its corners. `None` when the plane misses the
/// box (by more than `opt_tol`, `0` scaling the tolerance to the box and the plane
/// point) or the bounding sphere.
///
/// # Errors
///
/// Those of `Unitize` (a zero normal), `MakeUnitOrthoVectors` and
/// `CreateBilinearSurface`; `SM_ERR` when the bilinear surface is not a plane;
/// [`SolidError::Unported`] from the plane recognition.
pub fn create_plane_from_bbox(
    k: &Kernel,
    bbox: &Extent3d,
    plane_point: Point3,
    plane_normal: Vec3,
    opt_tol: f64,
) -> Result<Option<Surface>> {
    // locals
    let sphere = bbox.sphere_bound();
    let (sph_center, sph_radius) = (sphere.center, sphere.radius);
    let unit_normal = plane_normal.unitize()?;

    // Check the closest approach of the plane and the box.

    // Figure up a tol first.
    let size1 = bbox.size().length_squared();
    let size2 = bbox.min().length_squared();
    let size3 = bbox.max().length_squared();
    let size4 = plane_point.length_squared();
    let size = max4(size1, size2, size3, size4);
    let scaled_zero = EFF_ZERO * (1.0 + size.sqrt());
    let tol = max2(opt_tol, scaled_zero);

    // (upstream measures with the normal as given, not the unitized one)
    if bbox.distance_to_plane(plane_point, plane_normal).abs() > tol {
        return Ok(None);
    }

    // Drop the point on the sphere onto the plane.
    let proj_point = sph_center.project_point_to_plane(plane_point, unit_normal);

    // If the distance to the plane is greater than the the sphere radius, no
    // intersection exists - just exit.
    if sph_radius < proj_point.distance(sph_center) {
        return Ok(None);
    }

    // Use the difference vector to create a coordinate system for the plane.
    let diff = proj_point - plane_point;
    let dist_to_cent = diff.length();
    let [_z_ax, x_ax, y_ax] = plane_normal.make_unit_ortho_vectors(Some(diff))?;

    // Now that we have the plane coordinate system create four points on the plane which
    // surround the projection of the sphere onto the plane.
    let proj_radius = sph_radius * 2.0; // Make the plane twice the size of the sphere
    let p00 = plane_point + (dist_to_cent - proj_radius) * x_ax - proj_radius * y_ax;
    let p10 = plane_point + (dist_to_cent + proj_radius) * x_ax - proj_radius * y_ax;
    let p11 = plane_point + (dist_to_cent + proj_radius) * x_ax + proj_radius * y_ax;
    let p01 = plane_point + (dist_to_cent - proj_radius) * x_ax + proj_radius * y_ax;

    // Now create the plane
    let bilinear = BSplineSurface::create_bilinear_surface(p00, p10, p01, p11)?;
    match Plane::is_nurb_surface_plane(k, &bilinear, 1.0) {
        Ok(Some(plane)) => Ok(Some(Surface::from(plane))),
        Ok(None) => Err(SolidError::sm_err(OP_PLANE)),
        Err(e) if e.is_unported() => Err(e),
        // (an `SER` inside reads as `TRUE` and leaves no plane)
        Err(_) => Ok(None),
    }
}

/// The result of [`surface_planar_section_curves`].
#[derive(Debug, Clone, Default)]
pub struct SurfaceSection {
    /// `pOpt3DSectionCurves`: the 3D intersection curves.
    pub curves_3d: Vec<Curve>,
    /// `pOptUVSectionCurves`: their UV curves on the surface (`None` for an
    /// intersector that makes none).
    pub uv_curves: Vec<Option<Curve>>,
}

/// `SmSurface::CreatePlanarSectionCurves`: the curves where the infinite plane through
/// `plane_point` with normal `plane_normal` cuts `surface` over `uv_domain`. `bbox` is
/// the surface's `CalculateBoundingBox(uv_domain)`; the plane is bounded to it
/// ([`create_plane_from_bbox`], with `approx_tol_3d` or `0`) and intersected with the
/// surface (`GlobalSurfaceIntersect`, the surface's boundary edges used). `context`
/// (the face solve context and the surface's handle in it) serves a face-owned
/// surface's cache.
///
/// # Errors
///
/// Those of [`create_plane_from_bbox`], the surface cache and `GlobalSurfaceIntersect`.
pub fn surface_planar_section_curves(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    bbox: &Extent3d,
    plane_point: Point3,
    plane_normal: Vec3,
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
    context: Option<(
        &mut dyn SurfaceSolveContext<CacheObject>,
        Option<CacheObject>,
    )>,
) -> Result<SurfaceSection> {
    // make temporary nurb plane which is larger than the surface BBox.
    let tol = approx_tol_3d.unwrap_or(0.0);
    let Some(plane) = create_plane_from_bbox(k, bbox, plane_point, plane_normal, tol)? else {
        // No intersection with box just return.
        return Ok(SurfaceSection::default());
    };
    let plane_domain = plane.natural_uv_domain_with_nurb(k)?;

    // intersect the surface with the plane
    // (the surface's bounding edges do intersect the plane, the plane's do not)
    let use_surface_edges = [true, false];
    let found = if let Some((context, handle)) = context {
        // get surface cache (upstream then reads its continuity and ignores it)
        context.surface_cache_state(k, surface, handle)?;
        surface.global_surface_intersect_with_context(
            k,
            [handle, None],
            context,
            uv_domain,
            &plane,
            &plane_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
        )?
    } else {
        crate::surface::surface::p1::note_surface_cache(k, surface);
        surface.global_surface_intersect(
            k,
            uv_domain,
            &plane,
            &plane_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
        )?
    };
    Ok(SurfaceSection {
        curves_3d: found.curves_3d,
        uv_curves: found.surface1_uv_curves,
    })
}

/// `pFace->GetSurface()->CreatePlanarSectionCurves(crContext, pFace->GetUVDomain(), ..)`
/// as [`create_planar_section_curves`] calls it on face `f` of `b`: `surface` is an
/// evaluation copy of the face's surface (same owner and cache slot) and `uv_domain`
/// the face's UV domain; the box is the surface's `CalculateBoundingBox(uv_domain)` and
/// the solves run through the B-rep's [`BrepFaceServices`].
///
/// # Errors
///
/// As [`surface_planar_section_curves`], and those of the bounding box.
pub fn face_planar_section_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: &Surface,
    uv_domain: &Extent2d,
    plane_point: Point3,
    plane_normal: Vec3,
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
) -> Result<SurfaceSection> {
    let bbox = surface_bounding_box(k, b, f, surface, uv_domain)?;
    let mut services = BrepFaceServices::new(&mut *b);
    surface_planar_section_curves(
        k,
        surface,
        uv_domain,
        &bbox,
        plane_point,
        plane_normal,
        approx_tol_3d,
        ang_tol_rad,
        Some((&mut services, Some(CacheObject::Surface(f)))),
    )
}

/// The outputs [`create_planar_section_curves`] is asked for (upstream's optional
/// output arrays).
#[derive(Debug, Clone, Copy, Default)]
pub struct SectionOutputs {
    /// `pOpt3DSectionCurves`.
    pub curves_3d: bool,
    /// `pOptUVSectionCurves`.
    pub uv_curves: bool,
    /// `pOptFacesOfUVCurves`.
    pub faces: bool,
}

/// The optional arguments of [`create_planar_section_curves`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PlanarSectionOptions {
    /// `cpdOpt3DApproximationTol`: the largest distance between a section curve and the
    /// surface (each face's tolerance when `None`).
    pub approx_tol_3d: Option<f64>,
    /// `cpdOptAngleTolRadians`.
    pub ang_tol_rad: Option<f64>,
    /// The outputs asked for.
    pub outputs: SectionOutputs,
    /// `bForceUVClassify`: give a section curve without UV curve one by dropping it on
    /// the surface (replacing the 3D curve by the drop's lift); only with
    /// [`SectionOutputs::uv_curves`].
    pub force_uv_classify: bool,
}

/// The result of [`create_planar_section_curves`].
#[derive(Debug, Clone, Default)]
pub struct PlanarSection {
    /// `pOpt3DSectionCurves`: the planar 3D curves (when asked for).
    pub curves_3d: Option<Vec<Curve>>,
    /// `pOptUVSectionCurves`: the UV trim curve of each section curve (when asked for).
    pub uv_curves: Option<Vec<Option<Curve>>>,
    /// `pOptFacesOfUVCurves`: the face of each UV curve, or of each 3D curve when a
    /// face gave no UV curves (when asked for).
    pub faces: Option<Vec<FaceId>>,
}

/// `SmBrep::CreatePlanarSectionCurves`: the section of `b` by the infinite plane
/// through `plane_point` with normal `plane_normal`. Every face's surface is
/// sectioned over the face's UV domain ([`surface_planar_section_curves`]); section
/// curves that are degenerate stay, those coincident with a section curve of an earlier
/// face or (on a planar face) with an embedded edge of the face are removed; with
/// `force_uv_classify` and the UV output, a curve without UV curve gets one; the curves are
/// then trimmed to the face (`CreateCurvesByTrimming`).
///
/// Warning (upstream): if the plane crosses a surface seam you may get 2 sets of curves.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `SM_ERR_NULL_POINTER` for a face without
/// surface; those of the planarity test, the embedded edge test and, with
/// `force_uv_classify`, of `DropAndTrimCurve`, the drop curve evaluations, `DropPoint`
/// and `LiftCurve` (upstream's `SER`); [`SolidError::Unported`].
pub fn create_planar_section_curves(
    k: &Kernel,
    b: &mut Brep,
    plane_point: Point3,
    plane_normal: Vec3,
    opts: &PlanarSectionOptions,
) -> Result<PlanarSection> {
    // init outputs
    let mut out_uv: Vec<Option<Curve>> = Vec::new();
    let mut out_faces: Vec<FaceId> = Vec::new();

    // the 3D section curves of the faces so far, which later section curves are checked
    // against (upstream's local array stands in when the caller asks for none, but only
    // the caller's array is appended to: without the 3D output it stays empty)
    let mut section_3d: Vec<Curve> = Vec::new();

    // get all faces
    let faces = brep1::faces(b, None)?;

    // for every face
    for f in faces {
        let uv_domain = face2::uv_domain(b, f)?;
        // the face's surface, as an evaluation copy (same owner and cache slot)
        let surface = face2::surface(b, f)?
            .ok_or_else(|| SolidError::null_pointer(OP_SECTION))?
            .clone_for_evaluation()?;
        let face_tol = face2::tolerance(b, f)?;

        let tol = opts.approx_tol_3d.unwrap_or(face_tol);

        let is_planar = status_free(surface_is_planar(k, b, f, &surface, tol), false)?;

        // create surface planar sections - put output curves into s3DCurves and sUVCurves
        let section = face_planar_section_curves(
            k,
            b,
            f,
            &surface,
            &uv_domain,
            plane_point,
            plane_normal,
            opts.approx_tol_3d,
            opts.ang_tol_rad,
        );
        let (mut curves_3d, mut uv_curves) = match section {
            Ok(s) => (s.curves_3d, s.uv_curves),
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => continue,
        };
        // (one UV entry per 3D curve, as upstream indexes them)
        uv_curves.resize_with(curves_3d.len(), || None);

        // for every surface/plane intersection (the list grows with forced UV curves)
        let mut index = 0;
        while index < curves_3d.len() {
            let Some(curve) = curves_3d.get(index) else {
                break;
            };
            let orig_3d = curve.clone();
            let ivl = orig_3d.natural_interval();
            let param_tol = 1.0 / 100.0 * (EFF_ZERO_SQRT + ivl.length());

            // skip degenerate curves - currently they stay in the output
            if status_free(orig_3d.is_degenerate(k, EFF_ZERO, None), false)? {
                index += 1;
                continue;
            }

            // remove curves coincident with existing solutions and embedded edges on planar
            // faces
            let is_full_coincidence = |other: &Curve, other_ivl: &Extent1d| -> Result<bool> {
                // (status ignored upstream: the solutions are read as the check
                // left them)
                let (found, status) = orig_3d.global_coincidence_checker_with_partial::<NoObject>(
                    k,
                    &ivl,
                    other,
                    other_ivl,
                    tol,
                    true,
                    [None, None],
                );
                if let Err(e) = status {
                    if e.is_unported() {
                        return Err(e);
                    }
                }
                let found = found.solutions;
                Ok(match found.as_slice() {
                    [s] => {
                        s.solution_type == SolutionType::RangeOfValues
                            && ivl.classify_point(s.start.param(0)?, param_tol)
                                == ExtentPointType::Start
                            && ivl.classify_point(s.end.param(0)?, param_tol)
                                == ExtentPointType::End
                    }
                    _ => false,
                })
            };

            // for all accumulated 3dSection curves - check for coincidence
            let mut coin_with_section = false;
            for other in &section_3d {
                if is_full_coincidence(other, &other.natural_interval())? {
                    coin_with_section = true;
                    break;
                }
            }

            // skip section curves from embedded edges on planar faces
            let mut coin_with_embedded = false;
            if !coin_with_section && is_planar {
                // for every face->Embedded edge
                for e in status_free(face4::edges(b, f, None, false, None), Vec::new())? {
                    // skip nonEmbedded edges
                    if !status_free(edge::is_embedded(k, b, e, Some(f), true), false)? {
                        continue;
                    }

                    // check for coincidence
                    let face_curve =
                        status_free(edge::curve(b, e).map(Option::<&Curve>::cloned), None)?
                            .ok_or_else(|| SolidError::null_pointer(OP_SECTION))?;
                    let edge_ivl = status_free(edge::interval(b, e), Extent1d::new())?;
                    if is_full_coincidence(&face_curve, &edge_ivl)? {
                        coin_with_embedded = true;
                        break;
                    }
                }
            }

            if coin_with_section || coin_with_embedded {
                // remove the curve from the solution array, and move on to the next
                // solution (at the same index)
                curves_3d.remove(index);
                uv_curves.remove(index);
                continue;
            }

            // arrive here after culling duplicate and embedded solutions and skipping
            // degenerate solutions; next make sure 3DCurves have UVCurves
            if opts.force_uv_classify
                && opts.outputs.uv_curves
                && uv_curves.get(index).is_some_and(Option::is_none)
            {
                force_uv_curves(
                    k,
                    b,
                    f,
                    &surface,
                    &uv_domain,
                    face_tol,
                    index,
                    &orig_3d,
                    &mut curves_3d,
                    &mut uv_curves,
                )?;
            }
            index += 1;
        }

        // Trim curves to face (Calls SmFace::CurveOnClassify())
        let to_trim_3d: Vec<Option<Curve>> = curves_3d.into_iter().map(Some).collect();
        let trimmed = match face2::create_curves_by_trimming(
            k,
            b,
            f,
            opts.approx_tol_3d,
            Some(&to_trim_3d),
            opts.outputs.uv_curves.then_some(uv_curves.as_slice()),
            true,
            true,
        ) {
            Ok(t) => t,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => continue,
        };
        let trim_3d = trimmed.curves_3d.unwrap_or_default();
        let trim_uv = trimmed.uv_curves.unwrap_or_default();

        // add results to output
        if opts.outputs.faces {
            if trim_uv.is_empty() {
                // [B179] If pOptFaces but no UV curves, save faces associated with 3d
                // curves.
                out_faces.extend(std::iter::repeat_n(f, trim_3d.len()));
            } else {
                // save the face associated with each section UVcurve
                out_faces.extend(std::iter::repeat_n(f, trim_uv.len()));
            }
        }
        if opts.outputs.curves_3d {
            section_3d.extend(trim_3d);
        }
        if opts.outputs.uv_curves {
            out_uv.extend(trim_uv);
        }
    }

    Ok(PlanarSection {
        curves_3d: opts.outputs.curves_3d.then_some(section_3d),
        uv_curves: opts.outputs.uv_curves.then_some(out_uv),
        faces: opts.outputs.faces.then_some(out_faces),
    })
}

/// The forced UV curves of section curve `index` (`bForceUVClassify`): the curve is
/// dropped onto the face's surface (`DropAndTrimCurve`); each drop curve whose surface
/// point near its middle drops back onto the original 3D curve and whose lift stays
/// within ten face tolerances of the surface gives a UV curve and the lift as its 3D
/// curve: the first drop curve's replaces the entry at `index`, later ones are added.
fn force_uv_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: &Surface,
    uv_domain: &Extent2d,
    face_tol: f64,
    index: usize,
    orig_3d: &Curve,
    curves_3d: &mut Vec<Curve>,
    uv_curves: &mut Vec<Option<Curve>>,
) -> Result<()> {
    let handle = Some(CacheObject::Surface(f));
    let mut services = BrepFaceServices::new(&mut *b);

    // create a 2d Parameter Curve from the 3D Intersection Curve (the trace runs on the
    // face's cache, `smsurf_GetSurfaceCache` inside `DoTrace`, held from the context)
    let mut cache = services.build_surface_cache(k, surface, handle)?;
    let drops = surface
        .drop_and_trim_curve_in_context::<NoObject, CacheObject>(
            k,
            Some(&mut cache),
            uv_domain,
            orig_3d,
            &orig_3d.natural_interval(),
            face_tol / 3.0,
            true,
            true,
            None,
            Some(DropSolveContext {
                solver: &mut services,
                handle,
            }),
        )?
        .uv_curves;

    // for every 2d Parameter Curve generated
    for (kk, drop) in drops.into_iter().enumerate() {
        // Test near mid point to see if it looks good before lifting.
        let drop_ivl = drop.natural_interval();
        let p = drop.evaluate_point(k, drop_ivl.evaluate(0.824))?;
        let uv = Point2::new(p.x, p.y);

        // let sPnt = Surface(pDrop(nearMidParam))
        let pnt = surface.evaluate_point(k, uv)?;

        // drop sPnt to 3D Intersection curve to prove the DropCurve is good
        let dropped = orig_3d.drop_point(
            k,
            &orig_3d.natural_interval(),
            pnt,
            None,
            face_tol * 100.0,
            None,
            SolverOperation::Minimize,
        )?;
        if !dropped.success {
            continue;
        }

        // skip UVCurves whose 3D Approximations exceed tolerance
        let lifted = surface.lift_curve_with_context(
            k,
            uv_domain,
            &drop,
            &drop_ivl,
            face_tol / 3.0,
            true,
            &mut services,
            handle,
        )?;
        if lifted.max_approx_dev > face_tol * 10.0 {
            continue;
        }

        // for the 1st DropAndTrim Curve result replace current curve with 3D
        // Approximation and UVCurve in outputs; for subsequent DropAndTrim results add
        // the 3D Approximation curve and UVCurve to outputs
        if kk == 0 {
            if let Some(slot) = uv_curves.get_mut(index) {
                *slot = Some(drop);
            }
            if let Some(slot) = curves_3d.get_mut(index) {
                *slot = lifted.curve;
            }
        } else {
            curves_3d.push(lifted.curve);
            uv_curves.push(Some(drop));
        }
    }
    Ok(())
}

/// The value of an upstream getter that reports no status (`IsPlanar`, `IsDegenerate`,
/// `IsEmbedded`, `GetEdges`, `GetCurve`, `GetInterval`): [`SolidError::Unported`] and
/// [`SolidError::NotFound`] (a dangling id) propagate, any other failure reads as
/// `fallback`.
fn status_free<T>(r: Result<T>, fallback: T) -> Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e @ (SolidError::Unported { .. } | SolidError::NotFound { .. })) => Err(e),
        Err(_) => Ok(fallback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_10(k: &mut Kernel) -> Brep {
        crate::primitive::api_primitives::create_box(k, Point3::ZERO, 10.0, 10.0, 10.0).unwrap()
    }

    #[test]
    fn plane_from_bbox_misses_a_far_box() {
        let k = Kernel::default();
        let bbox = Extent3d::from_points(Point3::ZERO, Point3::new(1.0, 1.0, 1.0));
        let far = create_plane_from_bbox(&k, &bbox, Point3::new(0.0, 0.0, 5.0), Vec3::Z, 0.0);
        assert!(far.unwrap().is_none());
        let through = create_plane_from_bbox(&k, &bbox, Point3::new(0.0, 0.0, 0.5), Vec3::Z, 0.0);
        assert!(through.unwrap().is_some());
        assert!(create_plane_from_bbox(&k, &bbox, Point3::ZERO, Vec3::ZERO, 0.0).is_err());
    }

    #[test]
    fn box_section_is_four_lines_ten_long() {
        let mut k = Kernel::default();
        let mut b = box_10(&mut k);
        let opts = PlanarSectionOptions {
            outputs: SectionOutputs {
                curves_3d: true,
                faces: true,
                ..SectionOutputs::default()
            },
            ..PlanarSectionOptions::default()
        };
        let r =
            create_planar_section_curves(&k, &mut b, Point3::new(5.0, 5.0, 5.0), Vec3::X, &opts)
                .unwrap();
        let curves = r.curves_3d.unwrap();
        assert_eq!(curves.len(), 4);
        assert_eq!(r.faces.unwrap().len(), 4);
        for c in &curves {
            let ivl = c.natural_interval();
            let a = c.evaluate_point(&k, ivl.min()).unwrap();
            let z = c.evaluate_point(&k, ivl.max()).unwrap();
            assert!((a.x - 5.0).abs() < 1e-9 && (z.x - 5.0).abs() < 1e-9);
            assert!((a.distance(z) - 10.0).abs() < 1e-9);
        }
        assert!(r.uv_curves.is_none());
    }
}
