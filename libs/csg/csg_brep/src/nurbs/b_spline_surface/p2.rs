//! `SmBSplineSurface`, part 2: analytic copies, iso-parametric curves, spheres and
//! surfaces of revolution, trimmed planes from ruled surfaces, degeneracy and bilinear
//! tests, control point access and editing, the canonical setter, sub-patches,
//! splitting, u/v swapping, the maximum analytic domain and STEP domain adjustment.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurface.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//!
//! | Rust                                              | Upstream                         |
//! |---------------------------------------------------|----------------------------------|
//! | [`BSplineSurface::copy_and_add_analytics`]        | `CopyAndAddAnalytics`            |
//! | [`BSplineSurface::create_iso_parametric_curve`], [`IsoCurveOptions`], [`IsoParametricCurve`] | `CreateIsoParametricCurve` |
//! | [`BSplineSurface::create_sphere_from_arcs`], [`SphereFromArcs`] | `CreateSphereFromArcs`   |
//! | [`BSplineSurface::create_surf_of_revolution`], [`BSplineSurface::create_surf_of_revolution_with_line_vector`] | `CreateSurfOfRevolution` |
//! | [`BSplineSurface::create_trimmed_plane_from_ruled`], [`TrimmedPlane`] | `CreateTrimmedPlaneFromRuled` |
//! | [`BSplineSurface::is_degenerate_point`]           | `IsDegeneratePoint`              |
//! | [`BSplineSurface::is_bilinear`]                   | `IsBilinear`                     |
//! | [`BSplineSurface::control_point`]                 | `GetControlPoint`                |
//! | [`BSplineSurface::control_points_expert`], [`ControlPointBlock`] | `GetControlPointsExpert` |
//! | [`BSplineSurface::max_analytic_domain`]           | `GetMaxAnalyticDomain`           |
//! | [`BSplineSurface::set_canonical`]                 | `SetCanonical`                   |
//! | [`BSplineSurface::set_control_point`]             | `SetControlPoint`                |
//! | [`BSplineSurface::copy_sub_patch`]                | `CopySubPatch`                   |
//! | [`BSplineSurface::split_at`]                      | `SplitAt`                        |
//! | [`BSplineSurface::swap_uv`]                       | `SwapUV`                         |
//! | [`BSplineSurface::adjust_step_uv_domain`]         | `AdjustSTEPUVDomain`             |
//! | `transpose` (private)                             | `SmTArray<T>::Transpose`         |
//!
//! Opt-in ([`UpstreamFixes::analytic_copy_keeps_parameterization`](crate::kernel::UpstreamFixes::analytic_copy_keeps_parameterization), WR-450):
//! `CopyAndAddAnalytics` keeps an exact copy where its analytic surface differs from the source at the same parameters.
//!
//! Deviations:
//! - **Out-params** become return values (structs where there are several). The
//!   `SmContext` arguments only choose where upstream allocates and are dropped.
//! - **Surface type.** Only the plain B-spline is ported, so `GetType()` is always
//!   `SmBSplineSurface_TYPE` here: `SetControlPoint`'s refusal for analytic surfaces
//!   and `CopyAndAddAnalytics`' "copy an analytic as is" branch are never taken. The
//!   generator of `CreateSurfOfRevolution` is a [`BSplineCurve`]; an `SmLine`
//!   generator (a separate type here, `crate::curve::line`) passes its line vector to
//!   [`BSplineSurface::create_surf_of_revolution_with_line_vector`], which takes
//!   upstream's arc length reparametrisation of the sweep circle for cylinders
//!   (`GetType() == SmLine_TYPE` and the line vector parallel to the axis).
//! - **Missing NURB.** Where upstream dereferences the `NULL` NURB of a surface built
//!   by `SmBSplineSurface()` (`GetControlPointsExpert`, `SetControlPoint`,
//!   `CopySubPatch`, `SplitAt`) the port returns `SM_ERR`
//!   ([`SolidError::Upstream`]); `CreateIsoParametricCurve` keeps its
//!   `SM_ERR_NULL_POINTER` (`NER`).
//! - **Array bounds.** `GetControlPointsExpert` checks indices also for a single
//!   point (upstream reads outside the net there) and returns the output array
//!   (sized to the last value written, unwritten slots `0`, allocated fallibly:
//!   `InvalidInput` when strides make it too large for memory). `CreateSurfOfRevolution`
//!   returns `InvalidInput` where upstream would index past the sweep circle's
//!   weights or polygon.
//! - **Ignored statuses** stay ignored: `CreateIsoParametricCurve`'s
//!   `N_SrfExtractIsoCrv` failure is reported upstream and the empty allocated curve
//!   used; the port returns `SM_ERR` there instead (only reachable for a knot
//!   multiplicity above the degree). Its `Trim` and `CreateUVIsoLine` statuses are
//!   ignored (the UV curve is then `None`), except [`SolidError::Unported`], which is
//!   propagated, as everywhere.
//! - **`CreateSphereFromArcs`** returns the indices of the input arcs in CCW order
//!   (upstream returns the same curve pointers). The recomputing `IsArc` calls whose
//!   result upstream ignores keep the previous frame and angles when they fail. Its
//!   `rOrigin` is a reference into the frame those calls overwrite, so the centre check
//!   never fires and the sphere centre is the last examined arc's (WR-103): the port
//!   reads the current frame's origin at each use.
//! - **Edit notifications.** `SetCanonical`'s `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT`
//!   (`this`, `SM_NO_GET_OWNER(this)` = null) need the wrapping [`Surface`]'s identity,
//!   so the B-spline sends them through a caller hook
//!   ([`BSplineSurface::set_canonical_notifying`], [`BSplineSurface::swap_uv_notifying`],
//!   used by `Surface::swap_uv_notifying`); the plain entry points notify nobody.
//! - **`SetCanonical`** passes no domain to `CreateCanonical` (as upstream), so the
//!   [`CanonicalSurfaceData::uv_domain`] of its argument is ignored. As upstream, only
//!   the NURB is replaced: form and knot type stay those of the surface.
//! - **Back-edges.** `CopyAndAddAnalytics` returns recognised planes, cones, surfaces
//!   of revolution and extrusions as a [`Surface`]. A recognised sphere retains a
//!   `surface.surface_p2` dispatch gap;
//!   `GetMaxAnalyticDomain` nothing unported (`SmSurface::Evaluate1stDerivatives` is
//!   `surface::surface::p1`'s, `IsPlanar` and `IsClosed` are those of
//!   `surface::surface::p7`). `IsDegeneratePoint` over another domain runs the
//!   `SmSurface` base body on a copy of the B-spline wrapped as a `Surface` (without
//!   owner).

use crate::curve::curve::p2::CRV_ON_SURF_TYPE;
use crate::curve::curve::Curve as SmCurve;
use crate::curve::curve::CurveKind;
use crate::curve::iso_curve::IsoCurve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::object::NotifyOperation;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::{
    BSplineCurve, ControlPointForm, CurveCanonical, NurbCircleParam,
};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::geom::Rectangle;
use crate::nurbs::math::{NOW, NOZ};
use crate::nurbs::nurbs_srf::{is_nsrf_degenerate, split_srf};
use crate::nurbs::srf_basic::Direction;
use crate::nurbs::srf_tool::p1::{extract_iso_crv, extract_patch};
use crate::surface::plane::Plane;
use crate::surface::surf_of_extrusion::SurfOfExtrusion;
use crate::surface::surf_of_revolution::SurfOfRevolution;
use crate::surface::surface::Surface;

use super::p1::{
    BSplineSurface, BSplineSurfaceForm, BoundingBoxRequest, CanonicalSurfaceData, SurfParamType,
};

/// The edit notification hook of [`BSplineSurface::set_canonical_notifying`]: called
/// with `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT` where upstream's `SetCanonical` notifies.
pub(crate) type EditNotify<'a> = dyn FnMut(NotifyOperation) -> Result<()> + 'a;

const OP_ISO: &str = "nurbs::b_spline_surface::p2::BSplineSurface::create_iso_parametric_curve";
const OP_SPHERE: &str = "nurbs::b_spline_surface::p2::BSplineSurface::create_sphere_from_arcs";
const OP_REV: &str = "nurbs::b_spline_surface::p2::BSplineSurface::create_surf_of_revolution";
const OP_PLANE: &str =
    "nurbs::b_spline_surface::p2::BSplineSurface::create_trimmed_plane_from_ruled";
const OP_DEGEN: &str = "nurbs::b_spline_surface::p2::BSplineSurface::is_degenerate_point";
const OP_EXPERT: &str = "nurbs::b_spline_surface::p2::BSplineSurface::control_points_expert";
const OP_SET_CP: &str = "nurbs::b_spline_surface::p2::BSplineSurface::set_control_point";
const OP_PATCH: &str = "nurbs::b_spline_surface::p2::BSplineSurface::copy_sub_patch";
const OP_SPLIT: &str = "nurbs::b_spline_surface::p2::BSplineSurface::split_at";

/// The options of [`BSplineSurface::create_iso_parametric_curve`].
#[derive(Debug, Clone, Copy, Default)]
pub struct IsoCurveOptions {
    /// Domain whose bounds replace the natural domain and to which the curve is
    /// trimmed (`pOptDomain`).
    pub domain: Option<Extent2d>,
    /// Also build the UV line of the curve (`pOptUVIsoCurve`).
    pub uv_iso_curve: bool,
}

/// The result of [`BSplineSurface::create_iso_parametric_curve`].
#[derive(Debug, Clone)]
pub struct IsoParametricCurve {
    /// The 3D iso-parametric curve (`rpNewIsoCurve`).
    pub curve: BSplineCurve,
    /// The achieved maximum gap, always `0` for the exact B-spline extraction
    /// (`pOptMaxGap3d`).
    pub max_gap_3d: f64,
    /// The 2D UV line when asked for and built (`pOptUVIsoCurve`).
    pub uv_iso_curve: Option<SmCurve>,
}

/// The result of [`BSplineSurface::create_sphere_from_arcs`].
#[derive(Debug, Clone)]
pub struct SphereFromArcs {
    /// Indices of the input arcs, ordered counter-clockwise about the sphere
    /// (`rCCWArcs`).
    pub ccw_arcs: Vec<usize>,
    /// Orientation of each of them relative to that order (`rOrients`).
    pub orients: Vec<OrientType>,
    /// The sphere, a surface of revolution (`rpNewSphere`).
    pub sphere: BSplineSurface,
}

/// The result of [`BSplineSurface::create_trimmed_plane_from_ruled`].
#[derive(Debug, Clone)]
pub struct TrimmedPlane {
    /// The planar surface (`rpPlanarNurb`).
    pub plane: BSplineSurface,
    /// 3D trimming curves (`r3DTrimmingCurves`; empty when the ruled surface is a
    /// rectangle).
    pub trimming_curves: Vec<SmCurve>,
    /// Their orientations (`rTrimOrientations`).
    pub trim_orientations: Vec<OrientType>,
}

/// The block of control points [`BSplineSurface::control_points_expert`] copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPointBlock {
    /// First u index.
    pub start_u: usize,
    /// Last u index.
    pub end_u: usize,
    /// First v index.
    pub start_v: usize,
    /// Last v index.
    pub end_v: usize,
    /// Output values between consecutive u points (`lCtrlPointUStride`).
    pub u_stride: usize,
    /// Output values between consecutive v points (`lCtrlPointVStride`).
    pub v_stride: usize,
}

/// An `SmCurve::IsArc` result: frame, radius, start and end angle (degrees).
struct ArcData {
    frame: Axis2Placement,
    radius: f64,
    start_deg: f64,
    end_deg: f64,
}

/// `SmTArray<T>::Transpose(lNumRows)`: `a` read as `num_rows` columns of
/// `len / num_rows` values becomes its transpose (nothing for `num_rows == 0`).
fn transpose<T: Copy>(a: &mut [T], num_rows: usize) {
    if num_rows < 1 {
        return;
    }
    let num_columns = a.len() / num_rows;
    let temp = a.to_vec();
    for ii in 0..num_rows {
        for jj in 0..num_columns {
            if let (Some(dst), Some(&src)) = (
                a.get_mut(ii * num_columns + jj),
                temp.get(jj * num_rows + ii),
            ) {
                *dst = src;
            }
        }
    }
}

// ---- unported callees ---------------------------------------------------------------

/// The analytic recognisers after the plane: `SmCone::IsNurbSurfaceCone`,
/// `SmSphere::IsNurbSurfaceSphere`, `SmSurfOfRevolution::IsNurbSurfaceSurfOfRevolution`
/// and `SmSurfOfExtrusion::IsNurbSurfaceSurfOfExtrusion`, in upstream's order. `None`
/// when none recognises the surface.
fn recognise_analytic(k: &Kernel, srf: &BSplineSurface) -> Result<Option<Surface>> {
    if let Some(cone) = crate::surface::cone::Cone::is_nurb_surface_cone(k, srf, 1.0)? {
        return Ok(Some(Surface::from(cone)));
    }
    if let Some(sphere) = crate::surface::sphere::Sphere::is_nurb_surface_sphere(k, srf, 1.0)? {
        return Ok(Some(Surface::from(sphere)));
    }
    let test = Surface::from(srf.copy());
    if let Some(rev) = SurfOfRevolution::is_nurb_surface_surf_of_revolution(k, &test, 1.0)? {
        return Ok(Some(Surface::from(rev)));
    }
    if let Some(extrusion) = SurfOfExtrusion::is_nurb_surface_surf_of_extrusion(k, srf, 1.0)? {
        return Ok(Some(Surface::from(extrusion)));
    }
    Ok(None)
}

/// `pArc->IsArc(lNumberOfSamplePoints, d3DTol, ..)`: the virtual call on an
/// `SmBSplineCurve *` whose real kind may be a circle (`SmCircle::IsArc`, exact) or a
/// line (never an arc); [`SmCurve::is_arc`].
fn curve_is_arc(k: &Kernel, c: &SmCurve, num_samples: u64, tol: f64) -> Result<Option<ArcData>> {
    let arc = c.is_arc(k, num_samples, tol)?;
    Ok(arc.map(|(frame, radius, start_deg, end_deg)| ArcData {
        frame,
        radius,
        start_deg,
        end_deg,
    }))
}

/// `pOptSweepCurve->SmBSplineCurve::GetCanonical(..)` and `GetNaturalInterval()` of
/// the `SmCircle` sweep curve.
///
/// # Errors
///
/// `InvalidInput` when the sweep curve is not a circle (upstream's argument is an
/// `SmCircle*`); those of [`BSplineCurve::canonical`].
fn sweep_circle(c: &SmCurve) -> Result<(CurveCanonical, Extent1d)> {
    match c.kind() {
        CurveKind::Ellipse(e) if e.is_circle_class() => {
            Ok((e.base().canonical()?, e.base().natural_interval()))
        }
        _ => Err(SolidError::invalid_input(
            OP_REV,
            "the sweep curve is not an SmCircle",
        )),
    }
}

/// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`: point, `S_u`, `S_v`.
fn evaluate_1st_derivatives(
    k: &Kernel,
    srf: &BSplineSurface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    crate::surface::surface::p1::evaluate_1st_derivatives(
        k,
        srf,
        uv,
        crate::nurbs::knots::Side::Left,
        crate::nurbs::knots::Side::Left,
    )
}

/// The parts of `SmPlane(point, x_dir, y_dir, (1, 1), domain)` that
/// `CreateTrimmedPlaneFromRuled` uses.
struct PlaneData {
    /// `EvaluateNormal` at the domain centre.
    normal: Vec3,
    /// The points at the natural domain corners `(0, 0)`, `(0, 1)`, `(1, 0)`, `(1, 1)`
    /// (relative).
    corners: [Point3; 4],
}

/// `new SmPlane(point, x_dir, y_dir, (1, 1), domain)`, its normal
/// (`EvaluateNormal`) and the points at the corners of its natural domain
/// (`EvaluatePoint` at `GetMin`, `(0, 1)`, `(1, 0)`, `GetMax`).
fn new_plane(point: Point3, x_dir: Vec3, y_dir: Vec3, domain: &Extent2d) -> PlaneData {
    let plane = Plane::new(point, x_dir, y_dir, Vec2::new(1.0, 1.0), domain);
    let normal = plane.evaluate_normal();
    let plane_domain = plane.natural_uv_domain();
    let corners = [
        plane.evaluate_point(plane_domain.min()),
        plane.evaluate_point(plane_domain.evaluate(0.0, 1.0)),
        plane.evaluate_point(plane_domain.evaluate(1.0, 0.0)),
        plane.evaluate_point(plane_domain.max()),
    ];
    PlaneData { normal, corners }
}

/// `SmSurface::CreateNaturalUVTrimCurves(domain, SM_SP_U, FALSE, ..)`: the 3D trimming
/// curves and their orientations.
fn create_natural_uv_trim_curves(
    k: &Kernel,
    srf: &BSplineSurface,
    domain: &Extent2d,
) -> Result<(Vec<SmCurve>, Vec<OrientType>)> {
    let t = Surface::from(srf.copy()).create_natural_uv_trim_curves(
        k,
        domain,
        SurfParamType::U,
        false,
    )?;
    Ok((t.curves_3d, t.orients))
}

/// `SmSurface::IsDegeneratePoint(d3DTol, pUVDomain)` (the base body, on a copy of the
/// B-spline as an [`SmSurface`](crate::surface::surface::Surface)).
fn surface_is_degenerate_point(
    k: &Kernel,
    srf: &BSplineSurface,
    tol_3d: f64,
    uv_domain: Option<&Extent2d>,
) -> Result<bool> {
    let s = Surface::from(srf.clone());
    crate::surface::surface::p1::base_is_degenerate_point(k, &s, tol_3d, uv_domain)
}

/// `SmSurface::IsPlanar()`.
fn surface_is_planar(k: &Kernel, srf: &BSplineSurface) -> Result<bool> {
    // upstream's default tolerance is SM_EFF_ZERO
    Ok(crate::surface::surface::p7::b_spline_is_planar(k, srf, EFF_ZERO)?.is_planar)
}

/// `SmSurface::IsClosed(domain, dir)`.
fn surface_is_closed(
    k: &Kernel,
    srf: &BSplineSurface,
    domain: &Extent2d,
    dir: SurfParamType,
) -> Result<bool> {
    Ok(crate::surface::surface::p7::base_is_closed(k, srf, domain, dir, None, false)?.0)
}

/// Expands `ivl` relatively by 100 (degree 1), 1 (degree 2) or 0.5.
fn expand_by_degree(ivl: &mut Extent1d, deg: usize) {
    if deg == 1 {
        ivl.expand_relative(100.0);
    } else if deg == 2 {
        ivl.expand_relative(1.0);
    } else {
        ivl.expand_relative(0.5);
    }
}

impl BSplineSurface {
    /// A copy of the surface as an analytic surface when its shape is one (plane,
    /// cone, sphere, surface of revolution or extrusion), else as a B-spline
    /// (`CopyAndAddAnalytics`).
    ///
    /// # Errors
    ///
    /// Those of the recognisers ([`Plane::is_nurb_surface_plane`],
    /// [`crate::surface::cone::Cone::is_nurb_surface_cone`], ...);
    /// [`SolidError::Unported`] from the still-unwired recognised sphere conversion.
    pub fn copy_and_add_analytics(&self, k: &Kernel) -> Result<Surface> {
        // (a surface that is not SmBSplineSurface_TYPE is copied as is: never here)
        // SmPlane
        if let Some(plane) = Plane::is_nurb_surface_plane(k, self, 1.0)? {
            return Ok(self.analytic_or_exact_copy(k, Surface::from(plane)));
        }
        // SmCone, SmSphere, SmSurfOfRevolution, SmSurfOfExtrusion
        if let Some(analytic) = recognise_analytic(k, self)? {
            return Ok(self.analytic_or_exact_copy(k, analytic));
        }
        // arrive here - make sure it's a copy
        Ok(Surface::from(self.copy()))
    }

    /// The recognised analytic `analytic`, or an exact copy when
    /// `Kernel::fixes.analytic_copy_keeps_parameterization` is on and `analytic` does not
    /// give this surface's points at the same parameters (the fork's
    /// `sm_SameParameterization`, WR-450, #387): a face split with
    /// `MakeEdgeInFaceWithVU` keeps its UV domain and trims on the copy.
    fn analytic_or_exact_copy(&self, k: &Kernel, analytic: Surface) -> Surface {
        if !k.fixes.analytic_copy_keeps_parameterization {
            return analytic;
        }
        let domain = self.natural_uv_domain();
        let steps = 6.0;
        let mut pairs = Vec::with_capacity(49);
        let mut bbox = crate::geom::extent3d::Extent3d::default();
        for i in 0..=6u8 {
            for j in 0..=6u8 {
                let uv = domain.evaluate(f64::from(i) / steps, f64::from(j) / steps);
                let (Ok(a), Ok(b)) = (self.evaluate_point(uv), analytic.evaluate_point(k, uv))
                else {
                    return Surface::from(self.copy());
                };
                bbox.add_point(a);
                pairs.push((a, b));
            }
        }
        let tol = k.tolerances.zone_tol_3d.max(1.0e-9 * bbox.size().length());
        // (`!(d <= tol)`: a NaN distance is a mismatch)
        if pairs.iter().all(|&(a, b)| a.distance(b) <= tol) {
            analytic
        } else {
            Surface::from(self.copy())
        }
    }

    /// The 3D iso-parametric curve at `iso_parameter` (`CreateIsoParametricCurve`):
    /// constant u for [`SurfParamType::U`] (a curve in v), constant v otherwise. The
    /// parameter must lie in the (optional or natural) domain within
    /// `SM_EFF_ZERO_SQRT` and is clamped to it; with an optional domain the curve is
    /// trimmed to it.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a parameter outside the domain or when the extraction fails;
    /// `SM_ERR_NULL_POINTER` without NURB; [`SolidError::Unported`] from the UV iso
    /// line or the trim.
    pub fn create_iso_parametric_curve(
        &self,
        k: &Kernel,
        surf_param: SurfParamType,
        iso_parameter: f64,
        options: IsoCurveOptions,
    ) -> Result<IsoParametricCurve> {
        let mut iso_parameter = iso_parameter;

        // get natural or optional domain min/max points
        let uv_domain = options.domain.unwrap_or_else(|| self.natural_uv_domain());
        let (min_param, max_param) = if surf_param == SurfParamType::U {
            (uv_domain.min().x, uv_domain.max().x)
        } else {
            (uv_domain.min().y, uv_domain.max().y)
        };

        // Clamp ParamValues within Tolerance to interval boundaries
        if iso_parameter < min_param || iso_parameter > max_param {
            let ivl = Extent1d::from_min_max(min_param, max_param);

            // error - ParamValue out of boundary tolerance
            if !ivl.is_value_on_boundary(iso_parameter, EFF_ZERO_SQRT).0 {
                return Err(SolidError::sm_err(OP_ISO));
            }
            iso_parameter = ivl.clamp_value(iso_parameter);
        }

        // when asked - build associated 2d UVTrimCurve line (different parameterization)
        let uv_iso_curve = if options.uv_iso_curve {
            // define the isoParameter Curve as an object
            let iso = IsoCurve::new(self.copy(), surf_param, iso_parameter, true, true);
            // build OptUVIsoCurve (status ignored)
            ignored(iso.create_uv_iso_line())?
        } else {
            None
        };

        let surface = self.nurb.as_ref().ok_or(SolidError::null_pointer(OP_ISO))?;
        // constant U curves go in V direction, constant V curves in U direction
        let dir = if surf_param == SurfParamType::U {
            Direction::V
        } else {
            Direction::U
        };

        // build the isoParameter gw_CURVE
        let cur =
            extract_iso_crv(surface, iso_parameter, dir).map_err(SolidError::nl_ser(OP_ISO))?;

        // Use it as m_pNurb in a BSplineCurve object
        let mut new_iso_curve = BSplineCurve::from_nurb(cur, 3, false);

        // when asked - trim curve to optional domain limits
        if options.domain.is_some() {
            let mut trim_ivl = if surf_param == SurfParamType::U {
                uv_domain.v_interval()
            } else {
                uv_domain.u_interval()
            };
            if k.fixes.iso_curve_sliver_domain_is_degenerate
                && trim_ivl.max() - trim_ivl.min() < EFF_ZERO
            {
                // not upstream (WR-562, opt-in; fork source fork 8088f5e): a domain
                // of no length along the curve leaves of the iso curve only a point, a
                // degenerate curve there (Trim refuses the interval, and upstream returns
                // the whole curve)
                let point = new_iso_curve.evaluate_point(trim_ivl.min())?;
                new_iso_curve = BSplineCurve::create_degenerate_curve(3, point)?;
            } else {
                // may snap the interval by tol to existing knots (status ignored)
                ignored(new_iso_curve.trim(k, &mut trim_ivl, TrimMode::Notify))?;
            }
        }

        Ok(IsoParametricCurve {
            curve: new_iso_curve,
            max_gap_3d: 0.0,
            uv_iso_curve,
        })
    }

    /// The sphere through three arcs of a common center and radius (within `tol_3d`)
    /// (`CreateSphereFromArcs`): the largest arc is revolved about the axis through
    /// the center and the point it does not share with its larger neighbour, by 110%
    /// of the neighbour's span (at most 360 degrees).
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless there are exactly three arcs of a common center and radius
    /// meeting at their ends; [`SolidError::Unported`] (`SmCurve::IsArc`).
    pub fn create_sphere_from_arcs(
        k: &Kernel,
        arcs: &[SmCurve],
        tol_3d: f64,
    ) -> Result<SphereFromArcs> {
        let total_curves = arcs.len();

        // no work - not given exactly 3 curves
        if total_curves == 4 && k.fixes.fillet_corner_sphere_from_four_arcs {
            return Self::sphere_from_four_arcs(k, arcs, tol_3d);
        }
        if total_curves != 3 {
            return Err(SolidError::sm_err(OP_SPHERE)); // Only works for 3 now
        }
        let arc = |i: usize| arcs.get(i).ok_or_else(|| SolidError::sm_err(OP_SPHERE));

        // no work - first curve is not an arc
        let Some(first) = curve_is_arc(k, arc(0)?, 5, tol_3d)? else {
            return Err(SolidError::sm_err(OP_SPHERE));
        };
        let rad = first.radius;
        let mut ref_frame = first.frame;
        let mut gen_curve_index = 0usize;

        // extract Sphere Params from Arc shapes
        let mut angle_span = first.end_deg - first.start_deg;
        // (upstream's `rOrigin` is a reference into `sRefFrame`, which every later `IsArc`
        // overwrites: the centre is read from the current frame at each use, WR-103)

        // for all other input arcs - check for Compatible centers and radii
        for i in 1..total_curves {
            // no work - other Curve is not an arc of the same radius as the 1st Curve
            let Some(a) = curve_is_arc(k, arc(i)?, 5, tol_3d)? else {
                return Err(SolidError::sm_err(OP_SPHERE));
            };
            if (a.radius - rad).abs() > tol_3d {
                return Err(SolidError::sm_err(OP_SPHERE));
            }
            ref_frame = a.frame;

            // no work - OtherCurve center is not the same as 1st Curve cent: upstream
            // compares `sRefFrame.GetOriginRef()` with `rOrigin`, a reference to that same
            // origin, so the difference is zero and the check never fires (WR-103)

            // remember the largest ArcAngle seen
            let angle_span1 = a.end_deg - a.start_deg;
            if angle_span1 > angle_span {
                gen_curve_index = i; // Will choose the largest arc as generator curve
                angle_span = angle_span1;
            }
        }
        let (mut start_ang, mut end_ang) = (0.0, 0.0);

        // Get 1st Neighbor Curve to largest Span
        let mut second_largest = (gen_curve_index + 1) % total_curves;
        if let Some(a) = curve_is_arc(k, arc(second_largest)?, 5, tol_3d)? {
            ref_frame = a.frame;
            start_ang = a.start_deg;
            end_ang = a.end_deg;
        }
        let mut ref_z_axis = ref_frame.z_axis();
        angle_span = end_ang - start_ang;

        // get other Neighbor Curve to largest Span
        let other = ((gen_curve_index + total_curves) - 1) % total_curves;
        let mut smallest = other;
        let (mut start_ang1, mut end_ang1) = (0.0, 0.0);
        if let Some(a) = curve_is_arc(k, arc(other)?, 5, tol_3d)? {
            ref_frame = a.frame;
            start_ang1 = a.start_deg;
            end_ang1 = a.end_deg;
        }
        let angle_span1 = end_ang1 - start_ang1;

        // Order the Neighbor Curves based on length
        if angle_span1 > angle_span {
            angle_span = angle_span1;
            smallest = second_largest;
            second_largest = other;
            ref_z_axis = ref_frame.z_axis();
        }

        // Increase angle span by 10% to make sure it's big enough
        angle_span *= 1.1;
        if angle_span > 360.0 {
            angle_span = 360.0;
        }

        // Now find the non common point between the second largest and the gen curve
        let ends = |c: &SmCurve| -> Result<(Point3, Point3)> {
            let ivl = c.natural_interval();
            Ok((
                c.evaluate_point(k, ivl.min())?,
                c.evaluate_point(k, ivl.max())?,
            ))
        };
        let gen_curve = arc(gen_curve_index)?;
        let (st1, end1) = ends(gen_curve)?;
        let (st2, end2) = ends(arc(second_largest)?)?;
        let (st3, _end3) = ends(arc(smallest)?)?;

        // Set 1st Curve of output = GenCurve
        let mut ccw_arcs = vec![gen_curve_index];
        let mut orients = vec![OrientType::Opposite];
        let opposite_if = |b: bool| {
            if b {
                OrientType::Opposite
            } else {
                OrientType::Same
            }
        };
        let same_if = |b: bool| {
            if b {
                OrientType::Same
            } else {
                OrientType::Opposite
            }
        };

        // Set 2nd and 3rd Curves of output based on EndPoint coincidences
        let non_common_pnt;
        if st1.distance(st2) < tol_3d {
            non_common_pnt = end1;
            ccw_arcs.push(second_largest);
            orients.push(OrientType::Same);
            ccw_arcs.push(smallest);
            orients.push(opposite_if(end1.distance(st3) < tol_3d));
        } else if st1.distance(end2) < tol_3d {
            ref_z_axis = -ref_z_axis;
            non_common_pnt = end1;
            ccw_arcs.push(second_largest);
            orients.push(OrientType::Opposite);
            ccw_arcs.push(smallest);
            orients.push(opposite_if(end1.distance(st3) < tol_3d));
        } else if end1.distance(st2) < tol_3d {
            non_common_pnt = st1;
            ccw_arcs.push(smallest);
            orients.push(same_if(st1.distance(st3) < tol_3d));
            ccw_arcs.push(second_largest);
            orients.push(OrientType::Opposite);
        } else if end1.distance(end2) < tol_3d {
            ref_z_axis = -ref_z_axis;
            non_common_pnt = st1;
            ccw_arcs.push(smallest);
            orients.push(same_if(st1.distance(st3) < tol_3d));
            ccw_arcs.push(second_largest);
            orients.push(OrientType::Same);
        } else {
            return Err(SolidError::sm_err(OP_SPHERE));
        }

        // Define ZAxis in same general direction as sRefZAxis
        let mut z_axis = ref_frame.origin() - non_common_pnt;
        if z_axis.dot(ref_z_axis) < 0.0 {
            z_axis = -z_axis;
        }
        let z_axis = z_axis.unitize_or_z();

        // Make the Sphere as a SurfOfRevolution (of the generator's NURB data: a B-spline
        // or a circle, the only kinds IsArc accepts)
        let gen_nurb = match gen_curve.kind() {
            CurveKind::BSpline(b) => b,
            CurveKind::Ellipse(e) => e.base(),
            _ => return Err(SolidError::sm_err(OP_SPHERE)),
        };
        let sphere = Self::create_surf_of_revolution(
            k,
            gen_nurb,
            ref_frame.origin(),
            z_axis,
            angle_span,
            false,
            None,
        )?;
        Ok(SphereFromArcs {
            ccw_arcs,
            orients,
            sphere,
        })
    }

    /// [`Self::create_sphere_from_arcs`] for four arcs of one centre and radius that close a
    /// loop (`Kernel::fixes.fillet_corner_sphere_from_four_arcs`; not upstream): the whole
    /// sphere, a half-meridian revolved a turn about an axis square to the way the four arcs
    /// face, its seam on the far side, so the poles and the seam stay clear of the loop.
    fn sphere_from_four_arcs(k: &Kernel, arcs: &[SmCurve], tol_3d: f64) -> Result<SphereFromArcs> {
        let err = || SolidError::sm_err(OP_SPHERE);
        let mut centre = Point3::ZERO;
        let mut radius = 0.0;
        let mut towards = Vec3::ZERO;
        let mut ends: Vec<(Point3, Point3)> = Vec::new();
        for (i, a) in arcs.iter().enumerate() {
            let arc = curve_is_arc(k, a, 5, tol_3d)?.ok_or_else(err)?;
            let c = arc.frame.origin();
            if i == 0 {
                (centre, radius) = (c, arc.radius);
            } else if c.distance(centre) > tol_3d || (arc.radius - radius).abs() > tol_3d {
                return Err(err());
            }
            let ivl = a.natural_interval();
            let (s, e) = (
                a.evaluate_point(k, ivl.min())?,
                a.evaluate_point(k, ivl.max())?,
            );
            towards = towards + (s - centre) + (e - centre);
            ends.push((s, e));
        }
        // The arcs meet end to end (in some order and direction) and close.
        let mut open = [false; 4];
        for (i, (s, e)) in ends.iter().enumerate() {
            for p in [s, e] {
                let others = ends
                    .iter()
                    .enumerate()
                    .filter(|(j, (s2, e2))| {
                        *j != i && (p.distance(*s2) < tol_3d || p.distance(*e2) < tol_3d)
                    })
                    .count();
                if others != 1 {
                    *open.get_mut(i).ok_or_else(err)? = true;
                }
            }
        }
        if open.iter().any(|&o| o)
            || radius.partial_cmp(&tol_3d) != Some(core::cmp::Ordering::Greater)
            || towards.length().partial_cmp(&tol_3d) != Some(core::cmp::Ordering::Greater)
        {
            return Err(err());
        }
        let m = towards.unitize()?;
        // Any direction square to `m` is the axis; the seam runs the way away from the loop.
        let helper = if m.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let axis = m.cross(helper).unitize()?;
        let frame = Axis2Placement::new(centre, -m, axis);
        let meridian = BSplineCurve::create_circle_segment(
            3,
            &frame,
            radius,
            -90.0,
            90.0,
            NurbCircleParam::Quadratic,
        )?;
        let sphere =
            Self::create_surf_of_revolution(k, &meridian, centre, axis, 360.0, false, None)?;
        Ok(SphereFromArcs {
            ccw_arcs: (0..4).collect(),
            orients: vec![OrientType::Same; 4],
            sphere,
        })
    }

    /// The surface of revolution of `gen_curve` about the axis through `origin` along
    /// `axis_dir` by `angle_span_deg` (0 to 360) degrees (`CreateSurfOfRevolution`):
    /// each control point sweeps a quadratic circle segment (all equal for points on
    /// the axis). The circular direction is v, or u with `swap_uv`; an optional sweep
    /// circle (`pOptSweepCurve`) sets its parametrisation. The form is
    /// `SM_SF_SURF_OF_REVOLUTION`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a zero axis or an angle outside `[-360, 360]`; `SM_ERR` when
    /// every control point is on the axis or a circle or the surface cannot be built;
    /// `InvalidInput` for a sweep curve that is not a circle.
    pub fn create_surf_of_revolution(
        k: &Kernel,
        gen_curve: &BSplineCurve,
        origin: Point3,
        axis_dir: Vec3,
        angle_span_deg: f64,
        swap_uv: bool,
        opt_sweep_curve: Option<&SmCurve>,
    ) -> Result<Self> {
        Self::create_surf_of_revolution_with_line_vector(
            k,
            gen_curve,
            None,
            origin,
            axis_dir,
            angle_span_deg,
            swap_uv,
            opt_sweep_curve,
        )
    }

    /// [`Self::create_surf_of_revolution`] of a generator that may be an `SmLine`
    /// (`CreateSurfOfRevolution` with `pGenCurve->GetType() == SmLine_TYPE`):
    /// `gen_line_vector` is the line's `GetLineVector()` (`None` for any other
    /// generator). When it is parallel to the axis within `0.001` degrees (a
    /// cylinder) and no sweep curve is given, the sweep circle is reparametrised with
    /// arc length.
    ///
    /// # Errors
    ///
    /// Those of [`Self::create_surf_of_revolution`] and of the circle's arc length
    /// reparametrisation.
    pub fn create_surf_of_revolution_with_line_vector(
        k: &Kernel,
        gen_curve: &BSplineCurve,
        gen_line_vector: Option<Vec3>,
        origin: Point3,
        axis_dir: Vec3,
        angle_span_deg: f64,
        swap_uv: bool,
        opt_sweep_curve: Option<&SmCurve>,
    ) -> Result<Self> {
        // unitized rotation axis
        let axis_dir = axis_dir.unitize()?;

        // GenCurve locals
        let crv_ivl = gen_curve.natural_interval();
        let gen = gen_curve.canonical()?;
        let crv_deg = gen.degree;
        let crv_ctrl_pts = &gen.control_points;
        let is_gen_crv_rational = !gen.weights.is_empty();

        // Sweep direction locals
        let mut cir_deg = 0usize;
        let mut cir_knots: Vec<f64> = Vec::new();
        let mut cir_knot_mult: Vec<usize> = Vec::new();
        let mut cir_weights: Vec<f64> = Vec::new();
        let mut cir_ivl = Extent1d::new();
        let mut num_cir_ctrl_pts = 0usize;
        let mut knot_type = gen.knot_type;

        // when given an option SweepCurve
        if let Some(sweep) = opt_sweep_curve {
            // use it to specify the sweep curve parameters
            let (c, ivl) = sweep_circle(sweep)?;
            cir_deg = c.degree;
            cir_knot_mult = c.knot_multiplicities;
            cir_knots = c.knots;
            knot_type = c.knot_type;
            cir_weights = c.weights;
            num_cir_ctrl_pts = c.control_points.len();
            cir_ivl = ivl;
        } else {
            // Rotate each control point about the axis to get a circle: find the first
            // non-degenerate one to size the circles' control points and knots
            for &pnt in crv_ctrl_pts {
                let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());

                // Get sPnt/RotationAxis Distance
                let mut x_vec = pnt - origin;
                let t = x_vec.dot(axis_dir);
                let center = origin + t * axis_dir;
                x_vec = pnt - center;
                let radius = x_vec.length();

                // skip points on the rotation axis
                if radius < scaled_zero {
                    continue;
                }

                // create circle segment for this point's rotational sweep
                // (operator/, not the guarded operator/= of the loop below)
                let x_unit = x_vec / radius;
                let y_vec = axis_dir.cross(x_unit);
                let ref_frame = Axis2Placement::new(center, x_unit, y_vec);
                let mut cir = BSplineCurve::create_circle_segment(
                    3,
                    &ref_frame,
                    radius,
                    0.0,
                    angle_span_deg,
                    NurbCircleParam::Quadratic,
                )?;

                // If we have a cylinder, reparametrize it with arc length
                if let Some(line_vec) = gen_line_vector {
                    if line_vec.is_parallel_to(axis_dir, 0.001) {
                        cir.reparametrize_with_arc_length()?;
                    }
                }

                // get the sweep circle parameters
                let c = cir.canonical()?;
                cir_deg = c.degree;
                cir_knot_mult = c.knot_multiplicities;
                cir_knots = c.knots;
                knot_type = c.knot_type;
                cir_weights = c.weights;
                num_cir_ctrl_pts = c.control_points.len();
                cir_ivl = cir.natural_interval();

                // quit after 1st controlPoint not on rotation axis
                break;
            }
        }

        // quit when all GenCurve ControlPoints are on rotation vector
        if num_cir_ctrl_pts == 0 {
            return Err(SolidError::sm_err(OP_REV));
        }

        // For every ControlPoint in the output Surface, build a sweep circle.
        let mut ctrl_pts: Vec<Point3> = Vec::new();
        let mut weights: Vec<f64> = Vec::new();
        for (j, &pnt) in crv_ctrl_pts.iter().enumerate() {
            let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());

            // Calculate this sweep circles new controlPoint weights
            for ii in 0..num_cir_ctrl_pts {
                let mut weight = *cir_weights.get(ii).ok_or_else(|| {
                    SolidError::invalid_input(OP_REV, "sweep circle without weights")
                })?;
                if is_gen_crv_rational {
                    weight *= *gen.weights.get(j).ok_or_else(|| {
                        SolidError::invalid_input(OP_REV, "generator weight missing")
                    })?;
                }
                weights.push(weight);
            }

            // get sweep radius for this ControlPoint
            let mut x_vec = pnt - origin;
            let t = x_vec.dot(axis_dir);
            let center = origin + t * axis_dir;
            x_vec = pnt - center;
            let radius = x_vec.length();

            // when radius is zero - make all controlPoints for this sweep circle the same
            if radius < scaled_zero {
                // Add this control point lNumCirCtrlPts times
                for _ in 0..num_cir_ctrl_pts {
                    ctrl_pts.push(pnt);
                }
                continue;
            }

            // build sweep circle for this control point and add its control points
            // to newSurface ControlPoint array
            let cir_ctrl_pts = if let Some(sweep) = opt_sweep_curve {
                // copy, scale, and move sweep circle to desired location: this lets
                // the sweep curve specify the sweep parameterization of the surface
                let CurveKind::Ellipse(sweep_ellipse) = sweep.kind() else {
                    return Err(SolidError::invalid_input(
                        OP_REV,
                        "the sweep curve is not an SmCircle",
                    ));
                };
                let mut cir = sweep_ellipse.clone();

                // set up transformation from GivenSweepCurve to needed SweepCurve
                let sweep_radius = sweep_ellipse.radius();
                let sweep_frame = sweep_ellipse.position();
                let origin_of_sweep = sweep_frame.origin();
                let mut scale = radius / sweep_radius;

                // Check for inverted circle.  [B505] This creates a transform with a
                // negative scale, which creates an SmCircle with a negative radius. That
                // works though, and the circle is transient (only its control points
                // are used).
                if x_vec.dot(sweep_frame.x_axis()) < 0.0 {
                    scale *= -1.0;
                }

                // = -dScale * rOrigin + sCenter (translate to the current origin)
                let move_vec = (-scale) * origin_of_sweep + center;
                let mut move_frame = Axis2Placement::IDENTITY;
                move_frame.translate(move_vec);
                // (the status is ignored upstream: SmEllipse::Transform refuses a scale
                // below SM_EFF_ZERO before changing anything, so the copy is used as it is)
                ignore_status(cir.transform(k, &move_frame, Some(Vec3::new(scale, scale, scale))))?;
                cir.base().polygon(&cir.base().natural_interval())?
            } else {
                // generate a sweep circle from scratch
                x_vec /= radius; // Unitize.
                let y_vec = axis_dir.cross(x_vec).unitize()?;
                let ref_frame = Axis2Placement::new(center, x_vec, y_vec);
                let cir = BSplineCurve::create_circle_segment(
                    3,
                    &ref_frame,
                    radius,
                    0.0,
                    angle_span_deg,
                    NurbCircleParam::Quadratic,
                )?;
                cir.polygon(&cir.natural_interval())?
            };
            let take = cir_ctrl_pts.get(..num_cir_ctrl_pts).ok_or_else(|| {
                SolidError::invalid_input(OP_REV, "sweep circle polygon too short")
            })?;
            ctrl_pts.extend_from_slice(take);
        }

        // create the rotational BSplineSurface
        let surf_form = BSplineSurfaceForm::SurfOfRevolution;
        let uv_domain =
            Extent2d::from_bounds(crv_ivl.min(), cir_ivl.min(), crv_ivl.max(), cir_ivl.max());

        let mut new_surface = if swap_uv {
            // UV params are already set for SwapUV - just build the BSpline
            Self::create_canonical(&CanonicalSurfaceData {
                u_degree: crv_deg,
                v_degree: cir_deg,
                control_points: &ctrl_pts,
                form: surf_form,
                u_multiplicities: &gen.knot_multiplicities,
                v_multiplicities: &cir_knot_mult,
                u_knots: &gen.knots,
                v_knots: &cir_knots,
                knot_type,
                weights: Some(&weights),
                uv_domain: Some(uv_domain),
            })?
        } else {
            // SwapUV is not set - Transpose Weights and control points arrays
            transpose(&mut ctrl_pts, num_cir_ctrl_pts);
            transpose(&mut weights, num_cir_ctrl_pts);
            let trans_domain = Extent2d::from_bounds(
                uv_domain.min().y,
                uv_domain.min().x,
                uv_domain.max().y,
                uv_domain.max().x,
            );
            // then build the BSpline
            Self::create_canonical(&CanonicalSurfaceData {
                u_degree: cir_deg,
                v_degree: crv_deg,
                control_points: &ctrl_pts,
                form: surf_form,
                u_multiplicities: &cir_knot_mult,
                v_multiplicities: &gen.knot_multiplicities,
                u_knots: &cir_knots,
                v_knots: &gen.knots,
                knot_type,
                weights: Some(&weights),
                uv_domain: Some(trans_domain),
            })?
        };

        // set form factor
        new_surface.set_b_spline_surface_form(BSplineSurfaceForm::SurfOfRevolution);
        Ok(new_surface)
    }

    /// The trimmed plane of the planar ruled surface between `curve1` and `curve2`
    /// (`CreateTrimmedPlaneFromRuled`): when both are lines spanning a rectangle, the
    /// untrimmed bilinear surface; else a plane through the ruled surface with its
    /// boundary as trimming curves.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a curve has no NURB or the surfaces cannot be built or the curves
    /// do not lie on one plane; those of `SmSurface::CreateNaturalUVTrimCurves` (the
    /// kernel builds its iso curves) for the non-rectangular case.
    pub fn create_trimmed_plane_from_ruled(
        k: &Kernel,
        curve1: &BSplineCurve,
        curve2: &BSplineCurve,
    ) -> Result<TrimmedPlane> {
        let ends = curve1.ends()?;
        let (start, end) = (ends.start, ends.end);
        let tol = EFF_ZERO * 100.0 * (1.0 + start.max_dimension() + end.max_dimension());

        if let (Some(_), Some((line_point2, line_vec2))) =
            (curve1.is_line(5, tol)?, curve2.is_line(5, tol)?)
        {
            let param1 = line_closest_point(line_point2, line_vec2, start);
            let param2 = line_closest_point(line_point2, line_vec2, end);
            if (param1 + param2 - 1.0).abs() < tol
                && ((param1 - 1.0).abs() < tol || (param2 - 1.0).abs() < tol)
            {
                // Square ruled surface - this is easy just create the nurb.
                let plane = Self::create_bilinear_surface(
                    start,
                    end,
                    line_point2 + param1 * line_vec2,
                    line_point2 + param2 * line_vec2,
                )?;
                return Ok(TrimmedPlane {
                    plane,
                    trimming_curves: Vec::new(),
                    trim_orientations: Vec::new(),
                });
            }
        }

        let ruled_surface = Self::create_ruled_surface(curve1, curve2, SurfParamType::V)?;

        let domain = ruled_surface.natural_uv_domain();
        let (pnt, du, dv) = evaluate_1st_derivatives(k, &ruled_surface, domain.evaluate(0.5, 0.5))?;
        let du = du.unitize()?;
        let dv = du.cross(dv).cross(du);
        let dv = dv.unitize()?;

        let bbox = ruled_surface
            .calculate_bounding_box(
                k,
                &domain,
                &BoundingBoxRequest {
                    normal: true,
                    expand_by_zone_tol: true,
                    ..BoundingBoxRequest::default()
                },
            )?
            .normal
            .ok_or_else(|| SolidError::sm_err(OP_PLANE))?;
        let size = bbox.size();
        let leng = size.length();
        let tmp_domain = Extent2d::from_bounds(-leng, -leng, leng, leng);
        let plane = new_plane(pnt, dv, du, &tmp_domain);

        // Make sure both curves are on the plane before continuing.
        if !curve1.is_on_plane(pnt, plane.normal, tol, None)?.on_plane {
            return Err(SolidError::sm_err(OP_PLANE));
        }
        if !curve2.is_on_plane(pnt, plane.normal, tol, None)?.on_plane {
            return Err(SolidError::sm_err(OP_PLANE));
        }

        let (trimming_curves, trim_orientations) =
            create_natural_uv_trim_curves(k, &ruled_surface, &domain)?;
        let [c0, c1, c2, c3] = plane.corners;
        let plane = Self::create_bilinear_surface(c0, c1, c2, c3)?;
        Ok(TrimmedPlane {
            plane,
            trimming_curves,
            trim_orientations,
        })
    }

    /// Whether the surface is point sized within `tol_3d` (`IsDegeneratePoint`):
    /// over the natural domain through the control net's box
    /// (`sm_IsNSrfDegenerate`), else through `SmSurface::IsDegeneratePoint`.
    ///
    /// # Errors
    ///
    /// Those of `SmSurface::IsDegeneratePoint` for a domain other than the natural
    /// one; `SM_ERR` without NURB.
    pub fn is_degenerate_point(
        &self,
        k: &Kernel,
        tol_3d: f64,
        uv_domain: Option<&Extent2d>,
    ) -> Result<bool> {
        // low work - for nonNaturalUVInterval intervals pass the call
        //  along to the general SmSurface::IsDegenerate
        if uv_domain.is_some_and(|d| !d.are_equal(&self.natural_uv_domain(), EFF_ZERO))
            || self.is_kind_of(CRV_ON_SURF_TYPE)?
        {
            return surface_is_degenerate_point(k, self, tol_3d, uv_domain);
        }

        // ensure existence of m_pNurb
        let sur = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::sm_err(OP_DEGEN))?;

        // pass the call along
        is_nsrf_degenerate(sur, tol_3d)
    }

    /// Whether the control net is 2 x 2 (`IsBilinear`; not necessarily planar).
    #[must_use]
    pub fn is_bilinear(&self) -> bool {
        if self.number_control_points(SurfParamType::U) != 2 {
            return false;
        }
        if self.number_control_points(SurfParamType::V) != 2 {
            return false;
        }
        true
    }

    /// Control point `[u_index][v_index]` in `form` and its weight
    /// (`GetControlPoint`): Euclidean with weight `1` for
    /// [`ControlPointForm::NonRational`], homogeneous with its weight for
    /// [`ControlPointForm::HomogeneousRational`], Euclidean with its weight for
    /// [`ControlPointForm::EuclideanRational`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an index outside the net; `SM_ERR` without NURB.
    pub fn control_point(
        &self,
        form: ControlPointForm,
        u_index: usize,
        v_index: usize,
    ) -> Result<(Point3, f64)> {
        // locals
        let mut tmp = [0.0, 0.0, 0.0, 1.0]; // Default for non-rational.
        let pt_size = if form == ControlPointForm::NonRational {
            3
        } else {
            4
        };
        let u_stride = pt_size; // M = 1
        let v_stride = u_stride; // N = 1

        // pass the call along
        let vals = self.control_points_expert(
            form,
            &ControlPointBlock {
                start_u: u_index,
                end_u: u_index,
                start_v: v_index,
                end_v: v_index,
                u_stride,
                v_stride,
            },
        )?;
        for (slot, v) in tmp.iter_mut().zip(&vals) {
            *slot = *v;
        }
        let [x, y, z, w] = tmp;
        Ok((Point3::new(x, y, z), w))
    }

    /// The control points of `block` in `form` (`GetControlPointsExpert`): point
    /// `[i][j]` goes to `(i - start_u) * u_stride + (j - start_v) * v_stride` as
    /// `x y z` ([`ControlPointForm::NonRational`], divided by the weight for a
    /// rational surface), `wx wy wz w` ([`ControlPointForm::HomogeneousRational`]) or
    /// `x y z w` ([`ControlPointForm::EuclideanRational`]); a missing weight is `1`.
    /// The output ends with the last value written (empty when a range is inverted,
    /// with only one point requested); other slots are `0`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for inverted or out-of-net index ranges, or (for more than one
    /// point) a `v_stride` below the point size or a `u_stride` below `v_stride`
    /// times the v count; `SM_ERR` without NURB.
    pub fn control_points_expert(
        &self,
        form: ControlPointForm,
        block: &ControlPointBlock,
    ) -> Result<Vec<f64>> {
        let ControlPointBlock {
            start_u,
            end_u,
            start_v,
            end_v,
            u_stride,
            v_stride,
        } = *block;
        // locals
        let sur = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::sm_err(OP_EXPERT))?;
        let is_rational = self.is_rational();
        let pt_size = if form == ControlPointForm::NonRational {
            3
        } else {
            4
        };

        // when fetching just one point - don't need the stride info
        let need_stride = end_u > start_u || end_v > start_v;
        let empty = start_u > end_u || start_v > end_v;
        let outside = end_u > sur.n() || end_v > sur.m();
        if need_stride {
            // Criteria for not overwriting ourselves in the output array
            let ok_v_stride = v_stride >= pt_size;
            let row_size = end_v
                .checked_add(1)
                .and_then(|end| end.checked_sub(start_v))
                .and_then(|count| v_stride.checked_mul(count));
            let ok_u_stride = row_size.is_some_and(|size| u_stride >= size);

            // check inputs - consistent indexing and strides
            if empty || outside || !ok_u_stride || !ok_v_stride {
                return Err(SolidError::invalid_input(
                    OP_EXPERT,
                    "inconsistent input args",
                ));
            }
        }
        // nothing to copy (upstream's loops do not run)
        if empty {
            return Ok(Vec::new());
        }
        if outside {
            // upstream reads outside the net
            return Err(SolidError::invalid_input(
                OP_EXPERT,
                "control point index outside the net",
            ));
        }

        let nu = end_u - start_u;
        let nv = end_v - start_v;
        let len = u_stride
            .checked_mul(nu)
            .and_then(|a| v_stride.checked_mul(nv).and_then(|b| a.checked_add(b)))
            .and_then(|a| a.checked_add(pt_size))
            .ok_or_else(|| SolidError::invalid_input(OP_EXPERT, "output too large"))?;
        // the caller's output array upstream: allocated fallibly here
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| SolidError::invalid_input(OP_EXPERT, "output too large"))?;
        out.resize(len, 0.0);

        // copy the requested array of control points into the requested output array
        for (u_cnt, i_u) in (start_u..=end_u).enumerate() {
            for (v_cnt, i_v) in (start_v..=end_v).enumerate() {
                let cpt = sur.cpt(i_u, i_v)?;
                let (x, y, z) = (cpt.x, cpt.y, cpt.z);
                let w = if cpt.w == NOW { 1.0 } else { cpt.w };
                let vals: [Option<f64>; 4] = match form {
                    ControlPointForm::NonRational => {
                        if is_rational {
                            [Some(x / w), Some(y / w), Some(z / w), None]
                        } else {
                            [Some(x), Some(y), Some(z), None]
                        }
                    }
                    ControlPointForm::HomogeneousRational => [Some(x), Some(y), Some(z), Some(w)],
                    ControlPointForm::EuclideanRational => {
                        [Some(x / w), Some(y / w), Some(z / w), Some(w)]
                    }
                };
                let base = u_cnt * u_stride + v_cnt * v_stride;
                for (k, v) in vals.iter().enumerate() {
                    if let Some(v) = v {
                        let slot = out.get_mut(base + k).ok_or_else(|| {
                            SolidError::invalid_input(OP_EXPERT, "output slot out of range")
                        })?;
                        *slot = *v;
                    }
                }
            }
        }
        Ok(out)
    }

    /// The largest reasonable domain for extensions (`GetMaxAnalyticDomain`): the
    /// natural domain expanded relatively by 100 when planar, else per direction by
    /// 100 (degree 1), 1 (degree 2) or 0.5 unless closed in that direction.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: `SmSurface::IsPlanar` and `IsClosed` are not ported.
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        let mut dom = self.natural_uv_domain();

        if surface_is_planar(k, self)? {
            dom.expand_relative(100.0);
            return Ok(dom);
        }

        // Expand u and v separately.  Depends on degree.
        let mut u_dom = dom.u_interval();

        // Do not extend closed surfaces.
        if !surface_is_closed(k, self, &dom, SurfParamType::U)? {
            expand_by_degree(&mut u_dom, self.degree(SurfParamType::U));
        }

        let mut v_dom = dom.v_interval();
        if !surface_is_closed(k, self, &dom, SurfParamType::V)? {
            expand_by_degree(&mut v_dom, self.degree(SurfParamType::V));
        }

        dom = Extent2d::from_bounds(u_dom.min(), v_dom.min(), u_dom.max(), v_dom.max());
        Ok(dom)
    }

    /// Replaces the NURB by the one of STEP canonical data (`SetCanonical`, through
    /// [`BSplineSurface::create_canonical`] without domain); form and knot type are
    /// kept.
    ///
    /// # Errors
    ///
    /// The errors of [`BSplineSurface::create_canonical`] (the surface is unchanged).
    pub fn set_canonical(&mut self, data: &CanonicalSurfaceData<'_>) -> Result<()> {
        self.set_canonical_notifying(data, &mut |_| Ok(()))
    }

    /// [`BSplineSurface::set_canonical`] sending its `Notify(SM_NO_PRE_EDIT` /
    /// `SM_NO_POST_EDIT, this, SM_NO_GET_OWNER(this), NULL)` through `notify` around
    /// the NURB swap (after the temporary surface is built, before the analytic
    /// rebuild), as upstream does.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::set_canonical`] and of `notify`.
    pub(crate) fn set_canonical_notifying(
        &mut self,
        data: &CanonicalSurfaceData<'_>,
        notify: &mut EditNotify<'_>,
    ) -> Result<()> {
        // Create a temporary one
        let tmp = Self::create_canonical(&CanonicalSurfaceData {
            uv_domain: None,
            ..*data
        })?;

        notify(NotifyOperation::PreEdit)?;

        // Swap Nurbs with this
        self.nurb = tmp.nurb;

        notify(NotifyOperation::PostEdit)?;

        // give analytics a chance to update their nested STEP data
        self.rebuild_step_from_nurb_parameters()
    }

    /// Sets control point `[u_index][v_index]` (`SetControlPoint`): from Euclidean
    /// coordinates and `weight` ([`ControlPointForm::EuclideanRational`]; all other
    /// points get weight 1 if they had none), from Euclidean coordinates with weight 1
    /// ([`ControlPointForm::NonRational`]) or from homogeneous coordinates and
    /// `weight`. The weight is stored only for a rational surface, `z` only for a 3D
    /// net.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an index outside the net or without NURB.
    pub fn set_control_point(
        &mut self,
        form: ControlPointForm,
        u_index: usize,
        v_index: usize,
        control_point: Point3,
        weight: f64,
    ) -> Result<()> {
        // (analytic surfaces refuse control point moves: never here)
        let mut control_point = control_point;
        let mut weight = weight;
        let sur = self
            .nurb
            .as_mut()
            .ok_or_else(|| SolidError::sm_err(OP_SET_CP))?;

        // error check - out of bounds indices
        if u_index > sur.n() || v_index > sur.m() {
            return Err(SolidError::sm_err(OP_SET_CP));
        }

        // convert ControlPts to homogeneous rational form
        if form == ControlPointForm::EuclideanRational {
            control_point.x *= weight;
            control_point.y *= weight;
            control_point.z *= weight;

            // make sure all the other control points are also rational
            for i in 0..=sur.n() {
                for j in 0..=sur.m() {
                    let cpti = sur.cpt_mut(i, j)?;
                    if cpti.w == NOW {
                        cpti.w = 1.0;
                    }
                }
            }
        } else if form == ControlPointForm::NonRational {
            weight = 1.0;
        }

        // Set the Control Point value
        let is_rational = sur.cpt(0, 0)?.w != NOW;
        let cpt = sur.cpt_mut(u_index, v_index)?;
        cpt.x = control_point.x;
        cpt.y = control_point.y;
        if cpt.z != NOZ {
            cpt.z = control_point.z;
        }
        if is_rational {
            cpt.w = weight;
        }
        Ok(())
    }

    /// The part of the surface over `uv_domain` intersected with the natural domain
    /// (`CopySubPatch`, through `N_SrfExtractPatch`, which snaps the bounds to knots
    /// within `1e-8`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB or when the extraction fails (also for a domain disjoint
    /// from the natural one).
    pub fn copy_sub_patch(&self, uv_domain: &Extent2d) -> Result<Self> {
        // locals
        let sur = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::sm_err(OP_PATCH))?;

        // Trim crUVDomain to NaturalUVDomain (a disjoint domain gives the Init state)
        let nat_uv_domain = self.natural_uv_domain();
        let uv = nat_uv_domain
            .intersect(uv_domain)
            .unwrap_or_else(|_| Extent2d::new());

        // pass the call along
        let new_sur = extract_patch(
            sur,
            Rectangle {
                ul: uv.min().x,
                ur: uv.max().x,
                vb: uv.min().y,
                vt: uv.max().y,
            },
        )
        .map_err(SolidError::nl_ser(OP_PATCH))?;

        // copy sNewSur back into a SmBSplineSurface object
        Ok(Self::from_gw_nurb(&new_sur))
    }

    /// Splits the surface at `param` in u ([`SurfParamType::U`]) or v
    /// ([`SurfParamType::V`]) into the parts below and above (`SplitAt`); `param`
    /// first snaps to a knot within `100 * SM_EFF_ZERO * (1 + |param|)`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for another direction, a parameter at or outside the ends, or without
    /// NURB.
    pub fn split_at(&self, param: f64, surf_param: SurfParamType) -> Result<(Self, Self)> {
        let mut param = param;
        let tol = EFF_ZERO * 100.0 * (1.0 + param.abs());

        // If we are close to a knot then split at the knot (status ignored)
        let knots = self
            .knots(surf_param, None)
            .map_or_else(|_| Vec::new(), |k| k.0);
        for &knot in &knots {
            if (knot - param).abs() < tol {
                param = knot;
            }
        }

        // locals
        let sur = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::sm_err(OP_SPLIT))?;
        let dir = match surf_param {
            SurfParamType::U => Direction::U,
            SurfParamType::V => Direction::V,
            // NL_NO: sm_SplitSrf fails before allocating
            _ => return Err(SolidError::sm_err(OP_SPLIT)),
        };

        // split the gw_SURFACE object
        let (sur_l, sur_r) = split_srf(sur, param, dir).map_err(SolidError::nl_ser(OP_SPLIT))?;

        // build BSplineSurfaces for each new gw_Surface object
        Ok((Self::from_nurb(sur_l, false), Self::from_nurb(sur_r, false)))
    }

    /// Swaps the u and v parametrisations (`SwapUV`), reversing the orientation.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without NURB; the errors of
    /// [`BSplineSurface::set_canonical`] (the swap bit is then restored).
    pub fn swap_uv(&mut self) -> Result<()> {
        self.swap_uv_notifying(&mut |_| Ok(()))
    }

    /// [`BSplineSurface::swap_uv`] with `SetCanonical`'s edit notifications sent
    /// through `notify` ([`BSplineSurface::set_canonical_notifying`]).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::swap_uv`] and of `notify`.
    pub(crate) fn swap_uv_notifying(&mut self, notify: &mut EditNotify<'_>) -> Result<()> {
        // get current surface description
        let c = self.canonical()?;

        // let analytic surfaces remember that they have been swapped
        self.toggle_swap_uv_bit();

        // Transpose Weights and control points arrays
        let num_v_cpts = self.number_control_points(SurfParamType::V);
        let mut cpts = c.control_points;
        let mut weights = c.weights;
        transpose(&mut cpts, num_v_cpts);
        transpose(&mut weights, num_v_cpts);

        // Transpose Surface definition; on failure, restore the bit
        let status = self.set_canonical_notifying(
            &CanonicalSurfaceData {
                u_degree: c.v_degree,
                v_degree: c.u_degree,
                control_points: &cpts,
                form: c.form,
                u_multiplicities: &c.v_multiplicities,
                v_multiplicities: &c.u_multiplicities,
                u_knots: &c.v_knots,
                v_knots: &c.u_knots,
                knot_type: c.knot_type,
                weights: Some(&weights),
                uv_domain: None,
            },
            notify,
        );
        if status.is_err() {
            self.toggle_swap_uv_bit();
        }
        status
    }

    /// Changes the natural domain to `new_step_uv_domain` (`AdjustSTEPUVDomain`):
    /// extends the boundary spans to its bounds where needed, then trims to it.
    ///
    /// # Errors
    ///
    /// The errors of [`BSplineSurface::expand_boundary_span`] and
    /// [`BSplineSurface::trim_with_domain`].
    pub fn adjust_step_uv_domain(&mut self, new_step_uv_domain: &Extent2d) -> Result<()> {
        // Grow the surface if indicated, then trim.
        let min_max = new_step_uv_domain.min();
        self.expand_boundary_span(min_max.x, SurfParamType::U)?;
        self.expand_boundary_span(min_max.y, SurfParamType::V)?;

        let min_max = new_step_uv_domain.max();
        self.expand_boundary_span(min_max.x, SurfParamType::U)?;
        self.expand_boundary_span(min_max.y, SurfParamType::V)?;

        let mut uv_domain = *new_step_uv_domain;

        // change the domain - note: makes coincident geometry but the parameterization may change
        self.trim_with_domain(&mut uv_domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::CPoint;
    use crate::nurbs::srf_basic::Surface;

    /// A rational quadratic arc from `p0` to `p2` with tangent corner `c` and corner
    /// weight `w` (the cosine of half the arc angle).
    fn arc3(p0: [f64; 3], c: [f64; 3], w: f64, p2: [f64; 3]) -> SmCurve {
        let pw = vec![
            CPoint::new(p0[0], p0[1], p0[2], 1.0),
            CPoint::new(c[0] * w, c[1] * w, c[2] * w, w),
            CPoint::new(p2[0], p2[1], p2[2], 1.0),
        ];
        let cur = crate::nurbs::crv_basic::Curve::new(pw, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
            .expect("curve");
        SmCurve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    /// WR-562 (fork source fork 8088f5e): an iso curve over a domain of no
    /// length along it (v in `[0.5, 0.5]`) is the whole untrimmed curve upstream (`Trim`
    /// refuses the interval) and a degenerate curve at the surface point with
    /// `iso_curve_sliver_domain_is_degenerate`.
    #[test]
    fn wr562_iso_curve_over_a_sliver_domain_is_degenerate_with_the_opt_in() {
        let surface = BSplineSurface::create_bilinear_surface(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(4.0, 2.0, 1.0),
        )
        .unwrap();
        let options = IsoCurveOptions {
            domain: Some(Extent2d::from_bounds(0.0, 0.5, 1.0, 0.5)),
            uv_iso_curve: false,
        };
        let ends = |c: &BSplineCurve| {
            let ivl = c.natural_interval();
            (
                c.evaluate_point(ivl.min()).unwrap(),
                c.evaluate_point(ivl.max()).unwrap(),
            )
        };

        let mut k = Kernel::default();
        let upstream = surface
            .create_iso_parametric_curve(&k, SurfParamType::U, 0.25, options)
            .unwrap();
        let (a, b) = ends(&upstream.curve);
        assert!(a.distance(b) > 1.0, "{a:?} {b:?}");

        k.fixes.iso_curve_sliver_domain_is_degenerate = true;
        let fixed = surface
            .create_iso_parametric_curve(&k, SurfParamType::U, 0.25, options)
            .unwrap();
        let at = surface.evaluate_point(Point2::new(0.25, 0.5)).unwrap();
        let (a, b) = ends(&fixed.curve);
        assert!(
            a.distance(at) < 1e-12 && b.distance(at) < 1e-12,
            "{a:?} {b:?} {at:?}"
        );
    }

    /// WR-103: arcs about two centres (the first two sharing their start point) build a
    /// sphere, as upstream's aliased (dead) centre check lets them.
    #[test]
    fn sphere_from_arcs_about_two_centres() {
        let k = &Kernel::default();
        let arcs = [
            arc3(
                [5.0, 0.0, 0.0],
                [5.0, 8.660_254_037_844_386, 0.0],
                0.5,
                [-2.5, 4.330_127_018_922_193, 0.0],
            ),
            arc3(
                [5.0, 0.0, 0.0],
                [5.0, 0.0, 5.0],
                std::f64::consts::FRAC_1_SQRT_2,
                [10.0, 0.0, 5.0],
            ),
            arc3(
                [10.0, 5.0, 0.0],
                [10.0, 5.0, 2.886_751_345_948_129],
                0.866_025_403_784_438_6,
                [10.0, 2.5, 4.330_127_018_922_193],
            ),
        ];
        let r = BSplineSurface::create_sphere_from_arcs(k, &arcs, 1e-6).unwrap();
        assert_eq!(r.ccw_arcs, vec![0, 1, 2]);
    }

    /// Four great-circle arcs of radius 2 about the origin, closing a loop round +z (a corner
    /// where four equal rounds meet), as rational quadratic arcs.
    fn four_arcs() -> Vec<SmCurve> {
        let (r, theta) = (2.0_f64, 40.0_f64.to_radians());
        let u = |i: usize| {
            let phi = std::f64::consts::FRAC_PI_2 * i as f64;
            [
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ]
        };
        (0..4)
            .map(|i| {
                let (a, b) = (u(i), u((i + 1) % 4));
                let cos = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let c = [0, 1, 2].map(|j| r * (a[j] + b[j]) / (1.0 + cos));
                arc3(
                    [0, 1, 2].map(|j| r * a[j]),
                    c,
                    (cos.acos() / 2.0).cos(),
                    [0, 1, 2].map(|j| r * b[j]),
                )
            })
            .collect()
    }

    /// `fillet_corner_sphere_from_four_arcs`: the four arcs get the sphere they lie on, where
    /// upstream refuses ("only works for 3 now").
    #[test]
    fn sphere_from_four_arcs_only_with_the_fix() {
        let mut k = Kernel::default();
        let arcs = four_arcs();
        assert!(BSplineSurface::create_sphere_from_arcs(&k, &arcs, 1e-6).is_err());
        k.fixes.fillet_corner_sphere_from_four_arcs = true;
        let sphere = BSplineSurface::create_sphere_from_arcs(&k, &arcs, 1e-6)
            .unwrap()
            .sphere;
        let surface = crate::surface::surface::Surface::from(sphere);
        let domain = surface.natural_uv_domain_with_nurb(&k).unwrap();
        let (lo, hi) = (domain.min(), domain.max());
        for i in 0..=6 {
            for j in 0..=6 {
                let uv = Point2::new(
                    lo.x + (hi.x - lo.x) * f64::from(i) / 6.0,
                    lo.y + (hi.y - lo.y) * f64::from(j) / 6.0,
                );
                let d = surface.evaluate_point(&k, uv).unwrap().length();
                assert!((d - 2.0).abs() < 1e-9, "{uv:?}: {d}");
            }
        }
    }

    /// A biquadratic 3 x 3 patch over `[0, 2] x [0, 1]`.
    fn patch(rational: bool) -> BSplineSurface {
        let net = (0..3)
            .map(|i| {
                (0..3)
                    .map(|j| {
                        let w = match (rational, i) {
                            (false, _) => NOW,
                            (true, 1) => 2.0,
                            (true, _) => 1.0,
                        };
                        let (x, y, z) = (f64::from(i), f64::from(j), f64::from(i * j) * 0.25);
                        if w == NOW {
                            CPoint::new(x, y, z, NOW)
                        } else {
                            CPoint::new(w * x, w * y, w * z, w)
                        }
                    })
                    .collect()
            })
            .collect();
        let sur = Surface::new(
            net,
            2,
            2,
            vec![0.0, 0.0, 0.0, 2.0, 2.0, 2.0],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .expect("surface");
        BSplineSurface::from_nurb(sur, false)
    }

    #[test]
    fn transpose_matches_smtarray() {
        let mut a = vec![1, 2, 3, 4, 5, 6]; // 2 x 3 read as 3 rows of 2 columns
        transpose(&mut a, 3);
        assert_eq!(a, vec![1, 4, 2, 5, 3, 6]);
        let mut e: Vec<i32> = Vec::new();
        transpose(&mut e, 3);
        assert!(e.is_empty());
    }

    #[test]
    fn control_points_and_bilinear() {
        let s = patch(true);
        assert!(!s.is_bilinear());
        let (p, w) = s
            .control_point(ControlPointForm::EuclideanRational, 1, 2)
            .expect("cp");
        assert_eq!((p, w), (Point3::new(1.0, 2.0, 0.5), 2.0));
        let (p, w) = s
            .control_point(ControlPointForm::NonRational, 0, 0)
            .expect("cp");
        assert_eq!((p, w), (Point3::new(0.0, 0.0, 0.0), 1.0));
        assert!(s
            .control_point(ControlPointForm::NonRational, 3, 0)
            .is_err());
        let block = ControlPointBlock {
            start_u: 0,
            end_u: 1,
            start_v: 1,
            end_v: 2,
            u_stride: 8,
            v_stride: 4,
        };
        let v = s
            .control_points_expert(ControlPointForm::HomogeneousRational, &block)
            .expect("expert");
        assert_eq!(v.len(), 16);
        assert_eq!(v.get(12..16), Some(&[2.0, 4.0, 1.0, 2.0][..]));
    }

    #[test]
    fn control_points_expert_max_end_v_rejected() {
        let s = patch(true);
        let block = ControlPointBlock {
            start_u: 0,
            end_u: 1,
            start_v: 0,
            end_v: usize::MAX,
            u_stride: 8,
            v_stride: 4,
        };
        assert_eq!(
            s.control_points_expert(ControlPointForm::HomogeneousRational, &block),
            Err(SolidError::invalid_input(
                OP_EXPERT,
                "inconsistent input args"
            ))
        );
    }

    #[test]
    fn control_points_expert_invalid_ranges_and_strides() {
        let s = patch(true);
        for (start_u, end_u, start_v, end_v, u_stride, v_stride) in [
            (0, usize::MAX, 0, 1, 8, 4),
            (0, 1, usize::MAX, usize::MAX, 8, 4),
            (usize::MAX, 0, 0, 1, 8, 4),
            (0, 1, usize::MAX, 0, 8, 4),
            (1, 0, 0, 1, 8, 4),
            (0, 1, 1, 0, 8, 4),
            (0, 1, 0, 1, usize::MAX, usize::MAX),
            (0, 1, 0, 1, usize::MAX, usize::MAX / 2 + 1),
            (0, 1, 0, 1, 8, 3),
            (0, 1, 0, 1, 7, 4),
        ] {
            let block = ControlPointBlock {
                start_u,
                end_u,
                start_v,
                end_v,
                u_stride,
                v_stride,
            };
            assert_eq!(
                s.control_points_expert(ControlPointForm::HomogeneousRational, &block),
                Err(SolidError::invalid_input(
                    OP_EXPERT,
                    "inconsistent input args"
                )),
                "{block:?}"
            );
        }
    }

    #[test]
    fn control_points_expert_single_point_ignores_strides() {
        let s = patch(true);
        for stride in [0, usize::MAX] {
            let block = ControlPointBlock {
                start_u: 1,
                end_u: 1,
                start_v: 2,
                end_v: 2,
                u_stride: stride,
                v_stride: stride,
            };
            assert_eq!(
                s.control_points_expert(ControlPointForm::HomogeneousRational, &block),
                Ok(vec![2.0, 4.0, 1.0, 2.0])
            );
            let outside = ControlPointBlock {
                start_v: usize::MAX,
                end_v: usize::MAX,
                ..block
            };
            assert_eq!(
                s.control_points_expert(ControlPointForm::HomogeneousRational, &outside),
                Err(SolidError::invalid_input(
                    OP_EXPERT,
                    "control point index outside the net"
                ))
            );
            let empty = ControlPointBlock {
                start_u: usize::MAX,
                end_u: 0,
                ..block
            };
            assert_eq!(
                s.control_points_expert(ControlPointForm::HomogeneousRational, &empty),
                Ok(Vec::new())
            );
        }
    }

    #[test]
    fn set_control_point_euclidean_makes_rational() {
        let mut s = patch(false);
        s.set_control_point(
            ControlPointForm::EuclideanRational,
            1,
            1,
            Point3::new(1.0, 1.0, 2.0),
            0.5,
        )
        .expect("set");
        assert!(s.is_rational());
        let (p, w) = s
            .control_point(ControlPointForm::EuclideanRational, 1, 1)
            .expect("cp");
        assert_eq!((p, w), (Point3::new(1.0, 1.0, 2.0), 0.5));
        assert!(s
            .set_control_point(ControlPointForm::NonRational, 5, 0, Point3::ZERO, 1.0)
            .is_err());
    }

    #[test]
    fn swap_uv_transposes() {
        let mut s = patch(true);
        let before = s.control_point(ControlPointForm::EuclideanRational, 1, 2);
        s.swap_uv().expect("swap");
        let after = s.control_point(ControlPointForm::EuclideanRational, 2, 1);
        assert_eq!(before.ok(), after.ok());
        let d = s.natural_uv_domain();
        assert_eq!((d.max().x, d.max().y), (1.0, 2.0));
    }

    #[test]
    fn split_and_sub_patch() {
        let s = patch(false);
        let (l, r) = s.split_at(1.0, SurfParamType::U).expect("split");
        assert_eq!(l.natural_uv_domain().max().x, 1.0);
        assert_eq!(r.natural_uv_domain().min().x, 1.0);
        assert!(s.split_at(0.0, SurfParamType::U).is_err());
        assert!(s.split_at(1.0, SurfParamType::Both).is_err());
        let sub = s
            .copy_sub_patch(&Extent2d::from_bounds(0.5, 0.25, 5.0, 0.75))
            .expect("patch");
        let d = sub.natural_uv_domain();
        assert_eq!(
            (d.min().x, d.max().x, d.min().y, d.max().y),
            (0.5, 2.0, 0.25, 0.75)
        );
        assert!(s
            .copy_sub_patch(&Extent2d::from_bounds(5.0, 5.0, 6.0, 6.0))
            .is_err());
    }

    #[test]
    fn iso_curve_clamps_and_trims() {
        let k = Kernel::default();
        let s = patch(false);
        let c = s
            .create_iso_parametric_curve(
                &k,
                SurfParamType::U,
                2.0 + 1e-9,
                IsoCurveOptions::default(),
            )
            .expect("iso");
        let ivl = c.curve.natural_interval();
        assert_eq!((ivl.min(), ivl.max()), (0.0, 1.0));
        assert!(s
            .create_iso_parametric_curve(&k, SurfParamType::U, 2.1, IsoCurveOptions::default())
            .is_err());
        let opts = IsoCurveOptions {
            domain: Some(Extent2d::from_bounds(0.0, 0.25, 2.0, 0.75)),
            uv_iso_curve: false,
        };
        let c = s
            .create_iso_parametric_curve(&k, SurfParamType::U, 1.0, opts)
            .expect("iso");
        let ivl = c.curve.natural_interval();
        assert_eq!((ivl.min(), ivl.max()), (0.25, 0.75));
        let opts = IsoCurveOptions {
            domain: None,
            uv_iso_curve: true,
        };
        let c = s
            .create_iso_parametric_curve(&k, SurfParamType::U, 1.0, opts)
            .expect("iso u");
        assert!(c.uv_iso_curve.is_some());
        let c = s
            .create_iso_parametric_curve(&k, SurfParamType::V, 0.5, opts)
            .expect("iso v");
        assert!(c.uv_iso_curve.is_none());
        assert!(matches!(
            BSplineSurface::new().create_iso_parametric_curve(
                &k,
                SurfParamType::U,
                0.0,
                IsoCurveOptions::default()
            ),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
    }

    #[test]
    fn revolution_of_a_line() {
        let line = BSplineCurve::create_line_segment(
            3,
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 2.0),
        )
        .expect("line");
        let s = BSplineSurface::create_surf_of_revolution(
            &Kernel::default(),
            &line,
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 3.0),
            90.0,
            false,
            None,
        )
        .expect("rev");
        assert_eq!(s.form(), BSplineSurfaceForm::SurfOfRevolution);
        assert_eq!(s.degree(SurfParamType::U), 2);
        assert_eq!(s.degree(SurfParamType::V), 1);
        let on_axis =
            BSplineCurve::create_line_segment(3, Point3::ZERO, Point3::new(0.0, 0.0, 1.0))
                .expect("axis line");
        assert!(BSplineSurface::create_surf_of_revolution(
            &Kernel::default(),
            &on_axis,
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            90.0,
            true,
            None
        )
        .is_err());
    }

    #[test]
    fn copy_and_add_analytics_preserves_extrusion_geometry() {
        let k = &Kernel::default();
        for swap_uv in [false, true] {
            let net = [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                .into_iter()
                .map(|(x, y)| vec![CPoint::new(x, y, 0.0, NOW), CPoint::new(x, y, 2.0, NOW)])
                .collect();
            let nurb = Surface::new(
                net,
                2,
                1,
                vec![-1.0, -1.0, -1.0, 3.0, 3.0, 3.0],
                vec![2.0, 2.0, 4.0, 4.0],
            )
            .unwrap();
            let mut s = BSplineSurface::from_nurb(nurb, false);
            if swap_uv {
                s.swap_uv().unwrap();
            }
            let domain = s.natural_uv_domain();
            let samples = [(0.0, 0.0), (0.25, 0.75), (0.5, 0.5), (1.0, 1.0)];
            let before: Vec<_> = samples
                .iter()
                .map(|&(u, v)| s.evaluate_point(domain.evaluate(u, v)).unwrap())
                .collect();
            let copy = s.copy_and_add_analytics(k).unwrap();
            assert!(matches!(
                copy.kind(),
                crate::surface::surface::SurfaceKind::SurfOfExtrusion(_)
            ));
            if let crate::surface::surface::SurfaceKind::SurfOfExtrusion(extrusion) = copy.kind() {
                // Only swapped, unequal domains make Reparameterize build the NURB.
                assert_eq!(extrusion.base().gw_nurb().is_some(), swap_uv);
            }
            assert_eq!(copy.natural_uv_domain_if_built(), domain);
            for ((u, v), expected) in samples.into_iter().zip(before) {
                let uv = domain.evaluate(u, v);
                assert_eq!(s.evaluate_point(uv).unwrap(), expected);
                assert!(copy.evaluate_point(k, uv).unwrap().distance(expected) < 1e-12);
            }
            if let crate::surface::surface::SurfaceKind::SurfOfExtrusion(extrusion) = copy.kind() {
                // the evaluations made the missing NURB and the extrusion keeps it
                assert!(extrusion.base().gw_nurb().is_some());
            }
        }
    }

    /// #387's half cylinder (centre (0,50), r 40, z -60..60) as source's record has
    /// it: a rational quadratic in four equal spans (weights 1, .75, .5, .5, .75, 1).
    fn rational_half_cylinder() -> BSplineSurface {
        let ring = [
            (-32.714_727_726_319_09, 26.983_775_509_593_045),
            (-25.042_652_896_183_43, 16.078_866_267_486_678),
            (6.658_860_627_247_413, 5.777_160_028_477_432),
            (39.373_588_353_566_504, 28.793_384_518_884_38),
            (40.386_802_556_454_75, 62.111_315_248_300_59),
            (32.714_727_726_319_09, 73.016_224_490_406_96),
        ];
        let w = [1.0, 0.75, 0.5, 0.5, 0.75, 1.0];
        let net = ring
            .iter()
            .zip(w)
            .map(|(&(x, y), w)| {
                [-60.0, 60.0]
                    .map(|z| CPoint::new(w * x, w * y, w * z, w))
                    .to_vec()
            })
            .collect();
        let knots = vec![0.0, 0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0];
        let nurb = Surface::new(net, 2, 1, knots, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        BSplineSurface::from_nurb(nurb, false)
    }

    /// WR-450 (#387): the analytic copy of the rational half cylinder has SMLib's own
    /// parameterization, so a face's UV trims no longer fit it. With
    /// `analytic_copy_keeps_parameterization` the copy keeps the exact surface.
    #[test]
    fn rational_half_cylinder_copy_keeps_parameterization() {
        let mut k = Kernel::default();
        let s = rational_half_cylinder();
        let domain = s.natural_uv_domain();
        let off = s.copy_and_add_analytics(&k).unwrap();
        let worst = |k: &Kernel, c: &crate::surface::surface::Surface| {
            let mut worst = 0.0_f64;
            for i in 0..=6 {
                for j in 0..=6 {
                    let uv = domain.evaluate(f64::from(i) / 6.0, f64::from(j) / 6.0);
                    let a = s.evaluate_point(uv).unwrap();
                    worst = worst.max(a.distance(c.evaluate_point(k, uv).unwrap()));
                }
            }
            worst
        };
        assert!(
            !matches!(off.kind(), crate::surface::surface::SurfaceKind::BSpline(_)),
            "recognised as analytic"
        );
        let off_gap = worst(&k, &off);
        assert!(off_gap > 1e-3, "default copy is off by {off_gap}");

        k.fixes.analytic_copy_keeps_parameterization = true;
        let on = s.copy_and_add_analytics(&k).unwrap();
        assert!(matches!(
            on.kind(),
            crate::surface::surface::SurfaceKind::BSpline(_)
        ));
        assert!(worst(&k, &on) < 1e-12);
        // a surface whose analytic form shares its parameterization stays analytic
        let ext = {
            let net = [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                .into_iter()
                .map(|(x, y)| vec![CPoint::new(x, y, 0.0, NOW), CPoint::new(x, y, 2.0, NOW)])
                .collect();
            let nurb = Surface::new(
                net,
                2,
                1,
                vec![-1.0, -1.0, -1.0, 3.0, 3.0, 3.0],
                vec![2.0, 2.0, 4.0, 4.0],
            )
            .unwrap();
            BSplineSurface::from_nurb(nurb, false)
        };
        assert!(!matches!(
            ext.copy_and_add_analytics(&k).unwrap().kind(),
            crate::surface::surface::SurfaceKind::BSpline(_)
        ));
    }

    #[test]
    fn back_edges_are_wired() {
        let k = &Kernel::default();
        let s = patch(false);
        // no recogniser takes the patch: a B-spline copy
        let copy = s.copy_and_add_analytics(k).unwrap();
        assert!(matches!(
            copy.kind(),
            crate::surface::surface::SurfaceKind::BSpline(_)
        ));
        // (wired through surface.surface_p1 / surface.surface_p7)
        assert!(s.max_analytic_domain(k,).is_ok());
        assert!(s.is_degenerate_point(k, 1e-6, None).is_ok());
        let sub = Extent2d::from_bounds(0.0, 0.0, 0.5, 0.5);
        assert!(!s.is_degenerate_point(k, 1e-6, Some(&sub)).unwrap());
        assert!(matches!(
            BSplineSurface::create_sphere_from_arcs(k, &[], 1e-6),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn trimmed_plane_from_rectangle() {
        let c1 = BSplineCurve::create_line_segment(3, Point3::ZERO, Point3::new(1.0, 0.0, 0.0))
            .expect("c1");
        let c2 = BSplineCurve::create_line_segment(
            3,
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )
        .expect("c2");
        let r = BSplineSurface::create_trimmed_plane_from_ruled(&Kernel::default(), &c1, &c2);
        // the lines run the same way: param1 = 0, param2 = 1
        let t = r.expect("plane");
        assert!(t.plane.is_bilinear());
        assert!(t.trimming_curves.is_empty());
    }
}
