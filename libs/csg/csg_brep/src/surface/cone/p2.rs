//! `SmCone` part 2: the offset cone, recognition of a cone NURB, the rebuild of the
//! analytic data from an edited NURB, and the ellipse intervals inside a trimmed cone.
//!
//! Upstream: `source/SMLib/src/SmCone.cpp`, `source/SMLib/inc/SmCone.h`;
//! `SmCylinder::IsNurbSurfaceCylinder` from `source/SMLib/src/SmCylinder.cpp`,
//! `source/SMLib/inc/SmCylinder.h` (ported here, next to the cone recogniser whose iso
//! arc test it shares).
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`Cone::create_offset_surface`]              | `SmCone::CreateOffsetSurface`                    |
//! | [`Cone::is_nurb_surface_cone`]               | `SmCone::IsNurbSurfaceCone`                      |
//! | [`Cone::is_nurb_surface_cylinder`]           | `SmCylinder::IsNurbSurfaceCylinder` (`SmCylinder.cpp`) |
//! | [`Cone::rebuild_step_from_nurb_parameters`]  | `SmCone::RebuildSTEPFromNURBParameters`          |
//! | [`ellipse_trim_intervals`]                   | `sm_GetEllipseTrimIntervals` (file static)       |
//! | `is_cone_point_in_trim_domain` (private)     | `sm_IsConePointInTrimDomain` (file static)       |
//! | [`Cone::reparameterize`]                     | `SmSurfOfRevolution::Reparameterize` (inherited; its polar converter rebuild calls the cone's iso curves) |
//! | [`Cone::trim_with_domain`]                  | `SmSurfOfRevolution::TrimWithDomain` (inherited; its conversions and NURB rebuilds call the cone's virtuals) |
//! | [`Cone::base_mut`]                           | `GetGwNurbPointer` (non-`const`, inherited)      |
//!
//! Deviations:
//! - **Out-params** become return values: the offset is `None` where upstream succeeds
//!   without a surface (both new radii below the tolerance); the recogniser's cone is
//!   `Some` when upstream returns `TRUE`; the trim intervals are returned.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values
//!   (`SmPoint3d` outputs stay `SM_UNDEF_DOUBLE`, a failed `AngleBetween` is `0`),
//!   except that [`SolidError::Unported`] always propagates.
//! - **Uninitialised reads.** A failed `ConvertUVFromNURBSToSTEP` in
//!   [`Cone::trim_with_domain`] (upstream ignores the status and reads the corner it may
//!   not have written) returns the error. `sm_GetEllipseTrimIntervals` ignores the status of
//!   `smgu_LinePlaneIntersect` and then reads its parameter: a seam line parallel to
//!   the ellipse plane (or of zero length) returns the error here. A failed
//!   `ConvertUVFromNURBSToSTEP` there leaves the default-constructed domain, as
//!   upstream. A STEP inversion of the cone that fails in
//!   the generator solve returns the error (upstream keeps the location it had already
//!   classified); its other failures leave `SM_LT_EXTERIOR`, as upstream.
//! - **Seam lines of the ellipse trim (opt-in,
//!   [`UpstreamFixes::ellipse_trim_seam_lines_span_cone_height`](crate::kernel::UpstreamFixes::ellipse_trim_seam_lines_span_cone_height),
//!   WR-490).** `sm_GetEllipseTrimIntervals` takes the end circles at the cone's own
//!   height and classifies through the cone's STEP inversion, which ignores the domain,
//!   but evaluates the two seam lines on the given domain's v range. A face trimmed along
//!   its axis then loses the seam crossing that lies in its trimmed-off part; with no
//!   crossing left the ellipse is returned as outside, and two equal cylinders with
//!   crossing axes lose half their intersection (a union drops a body, status OK). With
//!   the field set, the seam lines span the cone's whole height too and only their sweep
//!   comes from the domain, as the source fork does; the face's own trims are applied
//!   by its caller. By default the seam lines use the domain, as upstream does. Since
//!   v1.0.1 the trim circles sit at the domain's v limits and the segments are classified
//!   against the domain, so such an ellipse keeps its part inside the domain either way.
//!   See `plan/upstream-issues/WR-490--ellipse-trim-seam-lines-on-trimmed-domain.md`.
//! - **Offset factory.** The apex-truncation branch dereferences the result of
//!   `CreateCanonical` without checking its status (a `NULL` cone crashes, a
//!   non-finite height recurses without bound); here that construction's error is
//!   returned.
//! - **NURB.** As in `surface::cone::p1`, the cone is expected to have its NURB
//!   ([`Cone::make_nurb`]); upstream makes a missing one inside `const` queries, which
//!   the port does (and keeps) at the `Surface` level (`Surface::kind_with_nurb`).
//!   `Notify` (attribute copying in the recogniser) has no equivalent.
//! - **Cylinder rebuild (#713, WR-385).** With the opt-in
//!   `Kernel::fixes.cylinder_rebuild_mean_radius`, an `SmCylinder` keeps a zero
//!   semi-angle. Its recovered end radii must agree within
//!   `EFF_ZERO_SQRT * (1 + bottom_radius)`;
//!   their mean becomes its radius. A larger taper returns `SM_ERR` before editing
//!   the analytic state. The pinned C++ instead assigns a nonzero semi-angle to
//!   cylinders. Ordinary `SmCone` objects retain upstream's radius/angle rules.
//! - **Cylinder recogniser.** [`Cone::is_nurb_surface_cylinder`] returns an error
//!   where a `SER` call returns its status (a truthy result without cylinder) and
//!   `Some` where upstream returns `TRUE` with a cylinder; its NURB is a copy of the
//!   tested one, as upstream. A tested surface without NURB is `SM_ERR_NULL_POINTER`
//!   (upstream copies a null NURB). With the opt-in
//!   `Kernel::fixes.nurb_cylinder_inside_out_frame` (WR-496, #1463) an inside-out
//!   NURB gets `IsNurbSurfaceCone`'s frame (top-knot circle, reversed Z).
//! - **Recursion.** None of these functions recurses.
//! - **Debug code.** The `SM_DEBUG_CODE` checks and drawing are omitted.

use super::Cone;
use crate::curve::curve::Curve;
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::curve::line::{BoundedLineOptions, Line};
use crate::error::{ignore_status, ignored, Result, SolidError, SM_ERR};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    line_plane_intersect, line_point_distance, shell_sort_double_array,
};
use crate::geom::geom_utility::p2::{circle_plane_intersect, CurveIntersection};
use crate::geom::tol::{approx_tol3d, is_zero};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, IsoCurveOptions, LocationType, SurfParamType,
};
use crate::nurbs::nurbs_crv::to_euclid;
use crate::nurbs::nurbs_srf::copy_nurb_surface;

const OP_OFFSET: &str = "surface::cone::p2::Cone::create_offset_surface";
const OP_NURB_CYLINDER: &str = "surface::cone::p2::Cone::is_nurb_surface_cylinder";
const OP_REBUILD: &str = "surface::cone::p2::Cone::rebuild_step_from_nurb_parameters";
const OP_REPARAM: &str = "surface::cone::p2::Cone::reparameterize";
const OP_TRIM: &str = "surface::cone::p2::ellipse_trim_intervals";

/// The `SmSurfOfRevolution::STEPInversion` operation whose `SM_ERR` means "the rotated
/// point is not on the generator" (upstream sets `SM_LT_EXTERIOR` there).
const OP_REV_STEP_INV: &str = "surface::surf_of_revolution::p2::SurfOfRevolution::step_inversion";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// A circular iso curve found by `IsArc` (or the apex of a cone with a vertex).
#[derive(Debug, Clone, Copy)]
struct IsoArc {
    /// `s*RefFrame`: origin at the centre, X towards the start point.
    frame: Axis2Placement,
    /// `d*Rad`.
    radius: f64,
    /// `d*StartAng`.
    start_ang: f64,
    /// `d*EndAng`.
    end_ang: f64,
}

/// The linear (generator) direction test shared by the recogniser and the rebuild:
/// `SM_SP_V` for a u of degree 1 with two control points, `SM_SP_U` for such a v,
/// else `SM_SP_NEITHER`.
fn linear_iso_param(s: &BSplineSurface) -> SurfParamType {
    if s.degree(SurfParamType::U) == 1 && s.number_control_points(SurfParamType::U) == 2 {
        SurfParamType::V
    } else if s.degree(SurfParamType::V) == 1 && s.number_control_points(SurfParamType::V) == 2 {
        SurfParamType::U
    } else {
        SurfParamType::Neither
    }
}

/// `pIsoCrv->IsArc(5, dScaledZero, ..)` of a plain B-spline iso curve.
fn iso_arc(k: &Kernel, curve: &BSplineCurve, scaled_zero: f64) -> Result<Option<IsoArc>> {
    let arc = crate::curve::curve::p5::is_arc_base(
        k,
        curve,
        |c, tol| c.is_degenerate(k, tol, None),
        5,
        scaled_zero,
    )?;
    Ok(arc.map(|(frame, radius, start_ang, end_ang)| IsoArc {
        frame,
        radius,
        start_ang,
        end_ang,
    }))
}

/// The point at the middle of the natural interval of a degenerate iso curve (status
/// ignored upstream: `SM_UNDEF_DOUBLE` on failure).
fn mid_point(curve: &BSplineCurve) -> Result<Point3> {
    Ok(ignored(curve.evaluate_point(curve.natural_interval().mid()))?.unwrap_or(Vec3::UNDEF))
}

/// The sweep iso curve of `s` at `knot`, or `None` when its construction fails
/// (upstream `return FALSE`; an unported dependency still propagates).
fn iso_curve(
    k: &Kernel,
    s: &BSplineSurface,
    sweep: SurfParamType,
    knot: f64,
) -> Result<Option<BSplineCurve>> {
    match s.create_iso_parametric_curve(k, sweep, knot, IsoCurveOptions::default()) {
        Ok(iso) => Ok(Some(iso.curve)),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(None),
    }
}

/// The top and bottom circles of the sweep iso curves at `top_knot` and `bot_knot`
/// (with a zero-radius circle at a vertex), or `None` when they are not two arcs or an
/// arc and a degenerate curve.
fn end_circles(
    k: &Kernel,
    s: &BSplineSurface,
    sweep: SurfParamType,
    top_knot: f64,
    bot_knot: f64,
    scaled_zero: f64,
) -> Result<Option<(IsoArc, IsoArc)>> {
    // make temporary Top and Bot BSPline isoParameter curves - hopefully arc shapes
    // (a failed curve is not a cone)
    let Some(top_iso) = iso_curve(k, s, sweep, top_knot)? else {
        return Ok(None);
    };
    let Some(bot_iso) = iso_curve(k, s, sweep, bot_knot)? else {
        return Ok(None);
    };

    // classify IsoparamCurves
    let top_degenerate = top_iso.is_degenerate(k, scaled_zero, None)?;
    let bot_degenerate = bot_iso.is_degenerate(k, scaled_zero, None)?;
    let top_arc = if top_degenerate {
        None
    } else {
        iso_arc(k, &top_iso, scaled_zero)?
    };
    let bot_arc = if bot_degenerate {
        None
    } else {
        iso_arc(k, &bot_iso, scaled_zero)?
    };

    //  2 Arcs                       = were good, continue
    //  1 Arc and a degenerate curve = cone has a vertex, set zero radius arc parameters
    //  2 degenerate curves          = not a cone, error
    match (top_arc, bot_arc) {
        (Some(top), _) if bot_degenerate => {
            // cone with a vertex
            let point = mid_point(&bot_iso)?;
            let bot = IsoArc {
                frame: Axis2Placement::new(point, top.frame.x_axis(), top.frame.y_axis()),
                radius: 0.0,
                start_ang: top.start_ang,
                end_ang: top.end_ang,
            };
            Ok(Some((top, bot)))
        }
        (_, Some(bot)) if top_degenerate => {
            // cone with a vertex
            let point = mid_point(&top_iso)?;
            let top = IsoArc {
                frame: Axis2Placement::new(point, bot.frame.x_axis(), bot.frame.y_axis()),
                radius: 0.0,
                start_ang: bot.start_ang,
                end_ang: bot.end_ang,
            };
            Ok(Some((top, bot)))
        }
        (Some(top), Some(bot)) => Ok(Some((top, bot))),
        // not a cone - Top or Bot circular isoParameter curve is degenerate or not an arc
        _ => Ok(None),
    }
}

/// Top/bot start angles, end angles or x axes vary by more than `ang_tol`.
fn twisted(top: &IsoArc, bot: &IsoArc, ang_tol: f64) -> Result<bool> {
    // (AngleBetween's status is ignored upstream: its angle is then 0)
    let twist_ang = ignored(top.frame.x_axis().angle_between(bot.frame.x_axis()))?.unwrap_or(0.0);
    Ok((top.start_ang - bot.start_ang).abs() > ang_tol
        || (top.end_ang - bot.end_ang).abs() > ang_tol
        || twist_ang.abs() > ang_tol)
}

impl Cone {
    /// Not upstream (source's draft of a cylindrical wall): this cylinder turned into the
    /// cone whose radius, the cylinder's at the height `neutral_height` (from the frame's
    /// origin along its axis), changes by `slope` per unit of height. The NURB's control
    /// points move radially, each row by the radius ratio at its height (a rational circle
    /// scaled about its axis stays exact, and the rows run straight up the axis), so the
    /// cone keeps the cylinder's frame, trimmed domain and parameterization: the UV trim
    /// curves of its face stay valid on it. The analytic data is then rebuilt from the NURB
    /// ([`Self::rebuild_step_from_nurb_parameters`]). The result is a cone
    /// ([`Self::is_cylinder_class`] false), whatever the slope.
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless this is a cylinder with a NURB whose radius stays positive; those of
    /// the control point edits and of the rebuild.
    pub fn lean_cylinder(&self, k: &Kernel, slope: f64, neutral_height: f64) -> Result<Cone> {
        let err = || SolidError::sm_err("surface::cone::p2::Cone::lean_cylinder");
        let canonical = self.canonical();
        if !self.is_cylinder()
            || canonical.radius.partial_cmp(&EFF_ZERO) != Some(core::cmp::Ordering::Greater)
        {
            return Err(err());
        }
        let origin = canonical.origin.origin();
        let axis = canonical.origin.x_axis().cross(canonical.origin.y_axis());
        let axis = axis / axis.length();
        let mut cone = self.clone();
        // The result is an `SmCone`, not an `SmCylinder` (the capi's draft gets an `SmCone`
        // from `CopyAndAddAnalytics` on the edited NURB): a kept cylinder flag would make the
        // rebuild refuse the unequal radii under `cylinder_rebuild_mean_radius`, and otherwise
        // leave a cone that reports itself a cylinder (source issue #1606).
        cone.cylinder = false;
        let (nu, nv) = (
            cone.rev.base.number_control_points(SurfParamType::U),
            cone.rev.base.number_control_points(SurfParamType::V),
        );
        for i in 0..nu {
            for j in 0..nv {
                let (p, w) = cone.rev.base.control_point(
                    crate::nurbs::b_spline_curve::ControlPointForm::EuclideanRational,
                    i,
                    j,
                )?;
                let d = p - origin;
                let height = d.dot(axis);
                let radius = canonical.radius + slope * (height - neutral_height);
                if radius.partial_cmp(&EFF_ZERO) != Some(core::cmp::Ordering::Greater) {
                    return Err(err());
                }
                let moved =
                    origin + axis * height + (d - axis * height) * (radius / canonical.radius);
                cone.rev.base.set_control_point(
                    crate::nurbs::b_spline_curve::ControlPointForm::EuclideanRational,
                    i,
                    j,
                    moved,
                    w,
                )?;
            }
        }
        cone.rebuild_step_from_nurb_parameters(k)?;
        Ok(cone)
    }

    /// The inherited NURB (`SmBSplineSurface` base object), mutably: an edit of the
    /// NURB (as through upstream's `GetGwNurbPointer`) leaves the analytic data as it
    /// was until [`Self::rebuild_step_from_nurb_parameters`].
    pub fn base_mut(&mut self) -> &mut BSplineSurface {
        &mut self.rev.base
    }

    /// Maps the NURB domain onto `trim_domain` without changing the shape (the
    /// inherited `SmSurfOfRevolution::Reparameterize`): the NURB (made first when
    /// missing, with the cone's iso curves), then the generator's NURB interval, then
    /// the polar converter rebuilt with the cone's iso curves (statuses ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the NURB cannot be made; those of
    /// [`crate::surface::surf_of_revolution::SurfOfRevolution::reparameterize`].
    pub fn reparameterize(&mut self, k: &Kernel, trim_domain: &Extent2d) -> Result<()> {
        // GetOrCreateGwNurbPointer (the virtual MakeNurb with the cone's iso curves)
        if self.rev.base.gw_nurb().is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        if self.rev.base.gw_nurb().is_none() {
            return Err(SolidError::null_pointer(OP_REPARAM));
        }
        let inside_out = self.inside_out;
        self.rev
            .reparameterize_using(k, trim_domain, &|s, k, p, t| {
                Ok(super::p1::cone_iso_curve(s, inside_out, k, p, t, None, false)?.curve)
            })
    }

    /// Trims to the NURB domain `new_nurb_uv_domain` (the inherited
    /// `SmSurfOfRevolution::TrimWithDomain`): its body with the cone's
    /// `ConvertUVFromNURBSToSTEP` (in `UpdateAnalyticalDomain`) and iso curves (in
    /// `MakeNurb`, `Reparameterize` and `RebuildPolarConverter`) as its virtual calls.
    ///
    /// # Errors
    ///
    /// Those of [`crate::surface::surf_of_revolution::SurfOfRevolution::trim_with_domain`]
    /// and of [`Self::convert_uv_from_nurbs_to_step`] (upstream ignores that status and
    /// reads the corner it may not have written).
    pub fn trim_with_domain(&mut self, k: &Kernel, new_nurb_uv_domain: &Extent2d) -> Result<()> {
        let inside_out = self.inside_out;
        self.rev.trim_with_domain_using(
            k,
            new_nurb_uv_domain,
            &|s, k, uv| super::p1::cone_convert_uv_from_nurbs_to_step(s, inside_out, k, uv),
            &|s, k, p, t| Ok(super::p1::cone_iso_curve(s, inside_out, k, p, t, None, false)?.curve),
        )
    }

    /// Rebuilds the polar converter with the cone's iso curves
    /// (`RebuildPolarConverter`, which calls the virtual `CreateIsoParametricCurve`).
    fn rebuild_cone_polar_converter(&mut self, k: &Kernel) -> Result<()> {
        let inside_out = self.inside_out;
        self.rev.rebuild_polar_converter(k, &|s, k, p, t| {
            Ok(super::p1::cone_iso_curve(s, inside_out, k, p, t, None, false)?.curve)
        })
    }

    // ---- CreateOffsetSurface -----------------------------------------------------

    /// Not upstream (`SmSurface::OffsetCollapses`, WR-464): whether the offset by
    /// `signed_offset_distance` has both radii below the tolerance, i.e. shrinks the cone
    /// (or cylinder) onto its axis, where [`Self::create_offset_surface`] gives no surface.
    #[must_use]
    pub fn offset_collapses(&self, k: &Kernel, signed_offset_distance: f64, tol: f64) -> bool {
        let tol = approx_tol3d(k, tol);
        let sign =
            if self.rev.swap_uv { -1.0 } else { 1.0 } * if self.inside_out { -1.0 } else { 1.0 };
        let d = (self.semi_angle_deg * PI / 180.0).cos() * sign * signed_offset_distance;
        self.bot_radius() + d < tol && self.top_radius() + d < tol
    }

    /// The cone offset by `signed_offset_distance` along the NURB normal (negated with
    /// [`crate::surface::surf_of_revolution::SurfOfRevolution::swap_uv`] and with
    /// [`Self::inside_out`]) (`CreateOffsetSurface`). The radii change by
    /// `cos(semi) d` and the origin moves by `-sin(semi) d` along the axis:
    ///
    /// * both new radii below the tolerance (`SmApproxTol3d(approx_tol_3d)`): `None`;
    /// * one of them below it: the cone truncated at its new apex, made by
    ///   [`Self::create_canonical`] (same `swap_uv` and `inside_out`), its NURB
    ///   reparameterised to the matching part of this NURB domain (upstream scales the
    ///   domain maximum, not its span), the generator's NURB interval set to match and
    ///   the polar converter rebuilt;
    /// * else a copy with the new base radius and origin, every NURB control point
    ///   moved by the offset of its generator row (computed from the next control
    ///   point at the apex) and the generator translated by the offset vector.
    ///
    /// # Errors
    ///
    /// Those of [`Self::height`] and of [`Self::create_canonical`] (upstream then
    /// dereferences a `NULL` cone); `SM_ERR_NULL_POINTER` without generator.
    pub fn create_offset_surface(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
        approx_tol_3d: f64,
    ) -> Result<Option<Self>> {
        let approx_tol_3d = approx_tol3d(k, approx_tol_3d);

        // When UVs are swapped - reverse orientation of offset
        // When InsideOut       - reverse orientation of offset
        let offset_sign = 1.0
            * (if self.rev.swap_uv { -1.0 } else { 1.0 })
            * (if self.inside_out { -1.0 } else { 1.0 });

        // Cone locals
        let origin = self.rev.position.origin();
        let x_axis = self.rev.position.x_axis();
        let y_axis = self.rev.position.y_axis();
        let z_axis = self.rev.position.z_axis();
        let height = self.height(k)?;
        let nurb_domain = self.rev.base.natural_uv_domain();

        // offset vector and displacements
        let mut sin = (self.semi_angle_deg * PI / 180.0).sin();
        let mut cos = (self.semi_angle_deg * PI / 180.0).cos();
        let norm = (cos * x_axis - sin * z_axis).unitize_or_z();
        let off_vec = offset_sign * signed_offset_distance * norm;
        let off_z_dist = -sin * offset_sign * signed_offset_distance;
        let off_rad_dist = cos * offset_sign * signed_offset_distance;

        // get new Bot and Top radii from Old and dOffRadDist
        let old_bot_radius = self.bot_radius();
        let old_top_radius = self.top_radius();
        let mut new_bot_radius = old_bot_radius + off_rad_dist;
        let mut new_top_radius = old_top_radius + off_rad_dist;

        // If both top and bottom are negative then don't create result
        // (`Self::offset_collapses`: the test of `SmSurface::OffsetCollapses`)
        if new_bot_radius < approx_tol_3d && new_top_radius < approx_tol_3d {
            return Ok(None);
        }

        // only the BotRadius or TopRadius has gone negative
        if new_bot_radius < approx_tol_3d || new_top_radius < approx_tol_3d {
            // compute BotOrigin, Height, Bot and Top radii for new truncated cone
            let (new_height, bot_origin) = if new_bot_radius < approx_tol_3d {
                let tan_ang = height / old_top_radius;
                let new_height = new_top_radius * tan_ang;
                new_bot_radius = 0.0;
                (
                    new_height,
                    origin + (height + off_z_dist - new_height) * z_axis,
                )
            } else {
                let tan_ang = height / old_bot_radius;
                let new_height = new_bot_radius * tan_ang;
                new_top_radius = 0.0;
                (new_height, origin + off_z_dist * z_axis)
            };

            // New Cone Position - truncated at new apex position
            let new_position = Axis2Placement::new(bot_origin, x_axis, y_axis);

            // Make the new cone - preserving original cone bSwapUV and bInsideOut values
            let mut off_cone = Self::create_canonical(
                k,
                &new_position,
                new_bot_radius,
                new_top_radius,
                new_height,
                self.rev.swap_uv,
                self.inside_out,
            )?;

            // compute the offsetSurf Nurb domain to be a properly trimmed subset of the
            // input Nurb domain
            let tgt_nurb_domain = if self.rev.swap_uv {
                Extent2d::from_bounds(
                    nurb_domain.min().x,
                    nurb_domain.min().y,
                    nurb_domain.min().x + new_height / height * nurb_domain.max().x,
                    nurb_domain.max().y,
                )
            } else {
                Extent2d::from_bounds(
                    nurb_domain.min().x,
                    nurb_domain.min().y,
                    nurb_domain.max().x,
                    nurb_domain.min().y + new_height / height * nurb_domain.max().y,
                )
            };

            // update the pOffCone NurbDomain (status ignored)
            ignore_status(off_cone.reparameterize(k, &tgt_nurb_domain))?;

            // adjust the GenCurve Nurb domain to be a properly trimmed subset of the
            // input NurbDomain (status ignored)
            let tgt_gen_nurb_ivl = if self.rev.swap_uv {
                tgt_nurb_domain.u_interval()
            } else {
                tgt_nurb_domain.v_interval()
            };
            let gen = off_cone
                .rev
                .gen_curve
                .as_mut()
                .ok_or(SolidError::null_pointer(OP_OFFSET))?;
            ignore_status(gen.edit_parameterization(k, &tgt_gen_nurb_ivl))?;

            // rebuild the PolarCurve to make sure all is coordinated (status ignored)
            ignore_status(off_cone.rebuild_cone_polar_converter(k))?;

            return Ok(Some(off_cone));
        }

        // copy the cone
        let mut off_cone = self.clone();

        // modify analytic parameters - new BaseRadius and new Cone Center
        off_cone.base_radius = self.base_radius + off_rad_dist;
        ignore_status(off_cone.rev.position.set_canonical(
            origin + off_z_dist * z_axis,
            x_axis,
            y_axis,
        ))?;

        // modify every Cone Nurb ControlPoint position - preserves the input NURB domain
        if let Some(off_nurb) = off_cone.rev.base.nurb.as_mut() {
            // For every m_pNurb ControlPoint - make inner iteration the GenCurve direction
            let (i_cnt, j_cnt) = if self.rev.swap_uv {
                (off_nurb.m(), off_nurb.n())
            } else {
                (off_nurb.n(), off_nurb.m())
            };
            let cp = |ii: usize, jj: usize| {
                if self.rev.swap_uv {
                    (jj, ii)
                } else {
                    (ii, jj)
                }
            };

            // for every sweep direction control point
            for ii in 0..=i_cnt {
                // get ControlPoint[i][0] Euclidean position
                let (u0, v0) = cp(ii, 0);
                let euclid_0 = to_euclid(off_nurb.cpt(u0, v0)?);

                // get the Cone radius for ControlPoint[i][0] Radius
                let p0_dot_z = (euclid_0.x - origin.x) * z_axis.x
                    + (euclid_0.y - origin.y) * z_axis.y
                    + (euclid_0.z - origin.z) * z_axis.z;
                let center = origin + p0_dot_z * z_axis;
                let vec = Vec3::new(
                    euclid_0.x - center.x,
                    euclid_0.y - center.y,
                    euclid_0.z - center.z,
                );
                let this_radius = self.radius(p0_dot_z);

                // when the radius is zero - need to compute move from other ControlPoint
                let mv = if is_zero(this_radius) {
                    // point is an apex - need to get rotation angle from next control
                    // point in genCurve direction
                    let (u1, v1) = cp(ii, 1);
                    let rotation_euclid = to_euclid(off_nurb.cpt(u1, v1)?);
                    let rotation_vec = Vec3::new(
                        rotation_euclid.x - origin.x,
                        rotation_euclid.y - origin.y,
                        rotation_euclid.z - origin.z,
                    );
                    let this_height = rotation_vec.dot(z_axis);
                    let radial_vec = rotation_vec - this_height * z_axis;
                    let radius = radial_vec.length();
                    let height_radius = self.radius(this_height);
                    // (operator/: a plain division, unlike operator/=)
                    let radial_unit = radial_vec / radius;
                    cos = radial_unit.dot(x_axis);
                    sin = radial_unit.dot(y_axis);

                    radius / height_radius * off_rad_dist * (cos * x_axis + sin * y_axis)
                        + off_z_dist * z_axis
                } else {
                    let scale = (this_radius + off_rad_dist) / this_radius;
                    vec * (scale - 1.0) + off_z_dist * z_axis
                };

                // for every genCurve directed ControlPoint
                for jj in 0..=j_cnt {
                    // apply the move to the controlPoint
                    let (u, v) = cp(ii, jj);
                    let euclid_j = to_euclid(off_nurb.cpt(u, v)?);
                    let cp_j = off_nurb.cpt_mut(u, v)?;
                    cp_j.x = (euclid_j.x + mv.x) * cp_j.w;
                    cp_j.y = (euclid_j.y + mv.y) * cp_j.w;
                    cp_j.z = (euclid_j.z + mv.z) * cp_j.w;
                }
            }
        }

        // translate the NewCone->GenCurve from current center line (status ignored)
        let mut mv = Axis2Placement::IDENTITY;
        mv.translate(off_vec);
        let gen = off_cone
            .rev
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_OFFSET))?;
        ignore_status(gen.transform(k, &mv, None))?;

        Ok(Some(off_cone))
    }

    // ---- IsNurbSurfaceCone -------------------------------------------------------

    /// The cone `test_surface` is (`IsNurbSurfaceCone`), or `None`: the surface must be
    /// rational, of degree 1 with two control points in one direction and degree 2 in
    /// the other, whose knots have the multiplicities `[3, 2, .., 2, 3]`; its
    /// iso curves at both ends of the linear direction are coaxial circular arcs (or an
    /// arc and a degenerate curve: a vertex) over the same angles, and the iso curve
    /// halfway is a circle of the average radius on the same axis. Tolerances are
    /// `tolerance_scale * SM_EFF_ZERO_SQRT` for angles and
    /// `tolerance_scale * 100 * SM_EFF_ZERO * (1 + |min corner| + |max corner|)` for
    /// distances (upstream default scale `1`). The new bounded cone is reparameterised
    /// to the surface's NURB domain (its NURB is the cone's own, not a copy).
    ///
    /// # Errors
    ///
    /// Those of the arc test (a failed iso curve is not a cone).
    pub fn is_nurb_surface_cone(
        k: &Kernel,
        test_surface: &BSplineSurface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        // low work - not rational, not proper degree, not linear in one direction or other
        let deg_u = test_surface.degree(SurfParamType::U);
        let deg_v = test_surface.degree(SurfParamType::V);
        let uv_domain = test_surface.natural_uv_domain();
        let linear_iso_param = linear_iso_param(test_surface);

        // quit when surface is not properly parameterized for a potential cone
        if deg_u + deg_v != 3
            || !test_surface.is_rational()
            || linear_iso_param == SurfParamType::Neither
        {
            return Ok(None);
        }

        // check for proper knot vector in non-linear direction:
        // [tripleKnot, doubleKnot, doubleKnot, ..., tripleKnot]
        let (_, cir_mults) = test_surface.knots(linear_iso_param, None)?;
        if cir_mults.first() != Some(&3) || cir_mults.last() != Some(&3) {
            return Ok(None);
        }
        let inner = cir_mults.len().saturating_sub(2);
        if cir_mults.iter().skip(1).take(inner).any(|&m| m != 2) {
            return Ok(None);
        }

        // evaluate Nurb min/max domain points (statuses ignored)
        let min_pnt = ignored(test_surface.evaluate_point(uv_domain.min()))?.unwrap_or(Vec3::UNDEF);
        let max_pnt = ignored(test_surface.evaluate_point(uv_domain.max()))?.unwrap_or(Vec3::UNDEF);

        // scale zero value and angle tolerance
        let ang_tol = tolerance_scale * EFF_ZERO_SQRT;
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + min_pnt.max_dimension() + max_pnt.max_dimension());

        // branch on linear direction to find circular and linear directions
        let (swap_uv, sweep_iso_param, bot_knot, top_knot) = if deg_u == 1 {
            // Need to swap UV when mapping parameters from analytic domain to nurbs domain
            (true, SurfParamType::U, uv_domain.min().x, uv_domain.max().x)
        } else {
            (
                false,
                SurfParamType::V,
                uv_domain.min().y,
                uv_domain.max().y,
            )
        };

        // classify the top and bottom isoParameter curves
        let Some((mut top, bot)) = end_circles(
            k,
            test_surface,
            sweep_iso_param,
            top_knot,
            bot_knot,
            scaled_zero,
        )?
        else {
            return Ok(None);
        };

        // check for not a cone - top/bot start angles or top/bot end angles vary
        if twisted(&top, &bot, ang_tol)? {
            return Ok(None);
        }

        // get the cone height = length of axis vector from botCurve Center to TopCurve
        // Center
        let center_top = top.frame.origin();
        let center_bot = bot.frame.origin();
        let mut nurb_z_axis = center_top - center_bot;
        let height = nurb_z_axis.length();

        // not a cone - height of cone is degenerate (cone is a disk)
        if height < EFF_ZERO_SQRT {
            return Ok(None);
        }
        nurb_z_axis = nurb_z_axis.unitize_or_z();

        // check for not a cone - bottom arc's plane is not perpendicular to cone axis
        if !nurb_z_axis.is_parallel_to(bot.frame.z_axis(), ang_tol) {
            return Ok(None);
        }

        // check for not a cone - a middle arc must also be concentric with axis, and in
        // a parallel plane
        let mid_param = (bot_knot + top_knot) / 2.0;
        let Some(mid_iso) = iso_curve(k, test_surface, sweep_iso_param, mid_param)? else {
            return Ok(None);
        };

        // It must be circular.
        let Some(mid) = iso_arc(k, &mid_iso, scaled_zero)? else {
            return Ok(None);
        };

        // Radius must be halfway between top and bottom.
        if ((bot.radius + top.radius) / 2.0 - mid.radius).abs() > scaled_zero {
            return Ok(None);
        }

        // check for not a cone - middle arc's plane is not perpendicular to cone axis
        if !mid.frame.z_axis().is_parallel_to(nurb_z_axis, ang_tol) {
            return Ok(None);
        }

        // check for not a cone - middle arc's center is not on cone axis
        // (smgu_LinePointDistance always succeeds)
        let (dist_to_axis, _) = line_point_distance(center_top, nurb_z_axis, mid.frame.origin());
        if dist_to_axis > scaled_zero {
            return Ok(None);
        }

        // arrive here when surface has passed all not-a-cone filters

        // By definition: the circle rotation sets the Z axis for the cone when the
        // CenterBot to CenterTop vector opposes this Z axis, bInsideOut = TRUE
        let inside_out = nurb_z_axis.dot(bot.frame.z_axis()) < 0.0;

        // For the x-axis, use the one (top or bottom) at the beginning of the param
        // range in that direction.
        let x_axis = if inside_out {
            top.frame.x_axis()
        } else {
            bot.frame.x_axis()
        };

        // get STEP YAxis from XAxis and StepZAxis
        let step_z_axis = if inside_out {
            -nurb_z_axis
        } else {
            nurb_z_axis
        };
        let y_axis = step_z_axis.cross(x_axis);

        // when top and bot radius values are nearly equal - make them exactly equal
        if (bot.radius - top.radius).abs() < EFF_ZERO * (1.0 + bot.radius) {
            top.radius = bot.radius;
        }

        // build the analytic cone surface
        // bInsideOut toggles the STEP Origin between Top and Bot NurbCircles
        let mut cone = Self::new_bounded(
            if inside_out {
                top.frame.origin()
            } else {
                bot.frame.origin()
            },
            x_axis,
            y_axis,
            if inside_out { top.radius } else { bot.radius },
            if inside_out { bot.radius } else { top.radius },
            top.start_ang,
            top.end_ang,
            height,
            swap_uv,
            inside_out,
            false,
        );

        // reparameterize the NURB so that AnalyticSurface->NurbDomain ==
        // InputSurface->NurbDomain (status ignored)
        ignore_status(cone.reparameterize(k, &test_surface.natural_uv_domain()))?;

        Ok(Some(cone))
    }

    // ---- SmCylinder::IsNurbSurfaceCylinder ------------------------------------------

    /// The cylinder `test_surface` is (`SmCylinder::IsNurbSurfaceCylinder`), or `None`:
    /// the surface must be rational, of degree 1 with at most two control points in one
    /// direction and degree 2 in the other; its iso curves at both ends of the linear
    /// direction are non-degenerate circular arcs over the same angles, with the same
    /// X axis, of equal radii, on parallel planes perpendicular to the line through
    /// their centres (at least `SM_EFF_ZERO` apart). Tolerances are `SM_EFF_ZERO_SQRT`
    /// for angles and `tolerance_scale * SM_EFF_ZERO * 100 * (1 + |min corner| + |max
    /// corner|)` for distances and radii (upstream default scale `1`). The new bounded
    /// cylinder ([`Cone::is_cylinder`]) keeps a copy of the tested NURB as its own, its
    /// polar converter rebuilt from it and its generator reparameterised to the
    /// linear direction's NURB interval. Upstream keeps an inside-out NURB's frame
    /// (origin the bottom-knot circle, Z along the linear direction), which reverses
    /// the cylinder's axis; with `k.fixes.nurb_cylinder_inside_out_frame` (WR-496,
    /// #1463) the frame is the one [`Self::is_nurb_surface_cone`] gives it: origin the
    /// top-knot circle, Z against the linear direction.
    ///
    /// # Errors
    ///
    /// Upstream's `SER` statuses (returned as a `TRUE` result without cylinder): those
    /// of the iso curves; those of the degeneracy and arc tests;
    /// `SM_ERR_NULL_POINTER` for a surface without NURB (upstream copies the null NURB).
    pub fn is_nurb_surface_cylinder(
        k: &Kernel,
        test_surface: &BSplineSurface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        let deg_u = test_surface.degree(SurfParamType::U);
        let deg_v = test_surface.degree(SurfParamType::V);
        if deg_u + deg_v != 3
            || !test_surface.is_rational()
            || (deg_u == 1 && test_surface.number_control_points(SurfParamType::U) > 2)
            || (deg_v == 1 && test_surface.number_control_points(SurfParamType::V) > 2)
        {
            return Ok(None);
        }

        // (evaluation statuses ignored upstream)
        let uv_domain = test_surface.natural_uv_domain();
        let min_pnt = ignored(test_surface.evaluate_point(uv_domain.min()))?.unwrap_or(Vec3::UNDEF);
        let max_pnt = ignored(test_surface.evaluate_point(uv_domain.max()))?.unwrap_or(Vec3::UNDEF);

        let pt_tol = tolerance_scale
            * EFF_ZERO
            * ANALYTIC_TOL_SCALE
            * (1.0 + min_pnt.max_dimension() + max_pnt.max_dimension());

        let ang_tol = EFF_ZERO_SQRT;
        let (swap_uv, surf_param, bot_knot, top_knot) = if deg_u == 1 {
            // Need to swap UV when mapping parameters from analytic domain to nurbs domain
            (true, SurfParamType::U, uv_domain.min().x, uv_domain.max().x)
        } else {
            (
                false,
                SurfParamType::V,
                uv_domain.min().y,
                uv_domain.max().y,
            )
        };

        let top_iso = test_surface
            .create_iso_parametric_curve(k, surf_param, top_knot, IsoCurveOptions::default())?
            .curve;
        if top_iso.is_degenerate(k, pt_tol, None)? {
            return Ok(None);
        }
        let Some(top) = iso_arc(k, &top_iso, pt_tol)? else {
            return Ok(None);
        };

        let bot_iso = test_surface
            .create_iso_parametric_curve(k, surf_param, bot_knot, IsoCurveOptions::default())?
            .curve;
        if bot_iso.is_degenerate(k, pt_tol, None)? {
            return Ok(None);
        }
        let Some(bot) = iso_arc(k, &bot_iso, pt_tol)? else {
            return Ok(None);
        };

        if (top.start_ang - bot.start_ang).abs() > ang_tol
            || (top.end_ang - bot.end_ang).abs() > ang_tol
        {
            return Ok(None);
        }

        let center_top = top.frame.origin();
        let x_axis = top.frame.x_axis();
        // (status ignored upstream, which then reads the uninitialised angle; the arcs'
        // unit X axes never fail it: 0 here)
        let ang = ignored(x_axis.angle_between(bot.frame.x_axis()))?.unwrap_or(0.0);
        if ang.abs() > ang_tol {
            return Ok(None);
        }

        let center_bot = bot.frame.origin();
        let mut z_axis = center_top - center_bot;
        let height = z_axis.length();

        // If you don't do this, Unitize will fail
        if height < EFF_ZERO {
            return Ok(None);
        }

        z_axis = z_axis.unitize_or_z();
        if !z_axis.is_parallel_to(bot.frame.z_axis(), ang_tol) {
            return Ok(None);
        }

        let inside_out = z_axis.dot(bot.frame.z_axis()) < 0.0;

        // Not upstream (WR-496, #1463, opt-in): an inside-out NURB's STEP frame points
        // against the NURB's linear direction from its top-knot circle, as
        // IsNurbSurfaceCone builds it; upstream ("Do some correction here") keeps the
        // bottom-knot circle and the NURB's direction, reversing the cylinder's Z
        let flip = inside_out && k.fixes.nurb_cylinder_inside_out_frame;
        let (origin, z_axis) = if flip {
            (center_top, -z_axis)
        } else {
            (center_bot, z_axis)
        };

        let y_axis = z_axis.cross(x_axis);
        // Try to eliminate some numerical inaccurracies (the top radius is not used
        // further)
        let equal_radii = (bot.radius - top.radius).abs() < pt_tol;
        if !equal_radii {
            return Ok(None);
        }

        let mut cylinder = Self::new_cylinder_bounded(
            origin,
            x_axis,
            y_axis,
            bot.radius,
            top.start_ang,
            top.end_ang,
            height,
            swap_uv,
            inside_out,
            false,
        );
        // (error recovery: the constructor always makes its generator)

        // the cylinder's NURB is a copy of the tested one
        let nurb = test_surface
            .gw_nurb()
            .ok_or_else(|| SolidError::null_pointer(OP_NURB_CYLINDER))?;
        cylinder.rev.base.nurb = Some(copy_nurb_surface(nurb)).into();
        cylinder.rev.base.nurb_is_borrowed = false;

        // (Notify(SM_NO_COPY) copies the test surface's attributes: not modelled)

        // (status ignored upstream)
        ignore_status(cylinder.rebuild_cone_polar_converter(k))?;

        // (status ignored upstream)
        let gen_ivl = Extent1d::from_min_max(bot_knot, top_knot);
        if let Some(gen) = cylinder.rev.gen_curve.as_mut() {
            ignore_status(gen.edit_parameterization(k, &gen_ivl))?;
        }

        Ok(Some(cylinder))
    }

    // ---- RebuildSTEPFromNURBParameters ---------------------------------------------

    /// Rebuilds the analytic data from the NURB after an edit
    /// (`RebuildSTEPFromNURBParameters`): the circles at both ends of the linear
    /// direction (tested as [`Self::is_nurb_surface_cone`] does with the scale `1000`,
    /// the corner points evaluated on the cone, without the knot and middle circle
    /// tests) give the semi-angle (kept for equal radii by default), base radius,
    /// inside-out flag, placement (the end circle's frame), STEP domain and a new
    /// generator line over the NURB's linear interval; the polar converter is rebuilt
    /// before the generator is replaced.
    /// With `k.fixes.cylinder_rebuild_mean_radius`, cylinders require end radii
    /// equal within `EFF_ZERO_SQRT * (1 + bottom_radius)`; they use the mean radius
    /// and an exactly zero semi-angle.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the NURB is no longer a cone, or, with the fix enabled, a
    /// cylinder's radii differ beyond the reconstruction tolerance; those of the
    /// iso curves.
    pub fn rebuild_step_from_nurb_parameters(&mut self, k: &Kernel) -> Result<()> {
        // check m_pNurb - a degree 2 rational surface in at least 1 direction
        let deg_u = self.rev.base.degree(SurfParamType::U);
        let deg_v = self.rev.base.degree(SurfParamType::V);
        let nurb_uv_domain = self.rev.base.natural_uv_domain();
        let linear_iso_param = linear_iso_param(&self.rev.base);
        let linear_ivl = match linear_iso_param {
            SurfParamType::V => nurb_uv_domain.u_interval(),
            SurfParamType::U => nurb_uv_domain.v_interval(),
            _ => Extent1d::new(),
        };

        // no work - modified parameterization
        if deg_u + deg_v != 3
            || !self.rev.base.is_rational()
            || linear_iso_param == SurfParamType::Neither
        {
            // Modified Nurb SmCone: no longer has degree 1 and 2 basis functions
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // evaluate Nurb min/max domain points (the cone's EvaluatePoint; statuses
        // ignored)
        let nurb_min_point =
            ignored(self.evaluate_point(k, nurb_uv_domain.min()))?.unwrap_or(Vec3::UNDEF);
        let nurb_max_point =
            ignored(self.evaluate_point(k, nurb_uv_domain.max()))?.unwrap_or(Vec3::UNDEF);

        // scaled zero value (loose because surf is already labeled analytic)
        let tolerance_scale = 1000.0;
        let ang_tol = tolerance_scale * EFF_ZERO_SQRT;
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + nurb_min_point.max_dimension() + nurb_max_point.max_dimension());

        // branch on linear direction to find circular and linear directions
        let (swap_uv, sweep_iso_param, bot_knot, top_knot) = if deg_u == 1 {
            (
                true,
                SurfParamType::U,
                nurb_uv_domain.min().x,
                nurb_uv_domain.max().x,
            )
        } else {
            (
                false,
                SurfParamType::V,
                nurb_uv_domain.min().y,
                nurb_uv_domain.max().y,
            )
        };

        // classify the top and bottom isoParameter curves (SmBSplineSurface's)
        let Some((mut top, mut bot)) = end_circles(
            k,
            &self.rev.base,
            sweep_iso_param,
            top_knot,
            bot_knot,
            scaled_zero,
        )?
        else {
            // Modified Nurb SmCone: no longer has circular top and bottom IsoParamCurves
            return Err(SolidError::sm_err(OP_REBUILD));
        };

        // not a cone - top/bot start angles or top/bot end angles vary by more than
        // tolerance
        if twisted(&top, &bot, ang_tol)? {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // get the cone height = length of axis vector from botCurve Center to TopCurve
        // Center
        let center_top = top.frame.origin();
        let center_bot = bot.frame.origin();
        let mut nurb_z_axis = center_top - center_bot;
        let height = nurb_z_axis.length();

        // not a cone - height of cone is degenerate (cone is a disk)
        if height < EFF_ZERO_SQRT {
            return Err(SolidError::sm_err(OP_REBUILD));
        }
        nurb_z_axis = nurb_z_axis.unitize_or_z();

        // not a cone - bottom arc's plane is not perpendicular to cone axis
        if !nurb_z_axis.is_parallel_to(bot.frame.z_axis(), ang_tol) {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // when the CenterBot to CenterTop vector opposes the circle Z axis, bInsideOut
        let inside_out = nurb_z_axis.dot(bot.frame.z_axis()) < 0.0;

        // (upstream also computes the STEP Y axis from the top frame's X axis here;
        // it is unused)

        // Not upstream (WR-385, source issue #713): with the opt-in, an
        // SmCylinder's radii within SM_EFF_ZERO_SQRT * (1 + r) are its one radius (their
        // mean); further apart, the NURB is no longer a cylinder
        if self.cylinder && k.fixes.cylinder_rebuild_mean_radius {
            if (bot.radius - top.radius).abs() < EFF_ZERO_SQRT * (1.0 + bot.radius) {
                let mean = 0.5 * (bot.radius + top.radius);
                bot.radius = mean;
                top.radius = mean;
                // Equal radii skip the angle update below. Clear any angle left
                // by a previous rebuild under the pinned rule, too (#713).
                self.semi_angle_deg = 0.0;
            } else {
                return Err(SolidError::sm_err(OP_REBUILD));
            }
        }

        // when top and bot radius values are nearly equal - make them exactly equal
        if (bot.radius - top.radius).abs() < EFF_ZERO * (1.0 + bot.radius) {
            top.radius = bot.radius;
        }

        // modify m_dSemiAngleDeg for nonCylinder shapes
        let radius_diff = if inside_out {
            bot.radius - top.radius
        } else {
            top.radius - bot.radius
        };
        if radius_diff.abs() > EFF_ZERO {
            self.semi_angle_deg = radius_diff.atan2(height) * 180.0 / PI;
        }

        // set Analytic parameters
        self.base_radius = if inside_out { top.radius } else { bot.radius };
        self.inside_out = inside_out;
        self.rev.planar_generator = true;
        self.rev.position = if inside_out { top.frame } else { bot.frame };
        self.rev.swap_uv = swap_uv;
        let _ = self
            .rev
            .anal_uv_domain
            .set_min_max_coords(top.start_ang, 0.0, top.end_ang, height);

        // Make the generator line
        let line_point = self.rev.position.origin() + self.base_radius * self.rev.position.x_axis();
        let line_vec = height * self.rev.position.z_axis()
            + (self.radius(height) - self.base_radius) * self.rev.position.x_axis();
        let length = line_vec.length();
        let mut line = Line::bounded(
            line_point,
            line_vec,
            &Extent1d::from_min_max(0.0, height),
            &BoundedLineOptions {
                scale: length / height,
                dim: 3,
                nurb: None,
                inside_out,
            },
        );

        // set the NurbDomain (status ignored)
        ignore_status(line.edit_parameterization(&linear_ivl))?;

        // Build newCone PolarConverter from Nurb rotation isoParamCurve (status
        // ignored), then save the gencurve
        ignore_status(self.rebuild_cone_polar_converter(k))?;
        self.rev.gen_curve = Some(Curve::from(line));
        Ok(())
    }
}

// ---- sm_GetEllipseTrimIntervals ----------------------------------------------------

/// The location and STEP parameters the cone's `STEPInversion` writes for `point` (its
/// status is ignored upstream). The parameters are only read for a location other
/// than `SM_LT_EXTERIOR`; they are `SM_UNDEF_DOUBLE` where a failure sets that location.
fn cone_inversion(
    k: &Kernel,
    cone: &Cone,
    point: Point3,
    tol: f64,
) -> Result<(LocationType, Point2)> {
    match cone.step_inversion(k, point, tol, None) {
        Ok(inv) => Ok((inv.location, inv.uv)),
        Err(e) if e.is_unported() => Err(e),
        // rotated point not on the generator: upstream sets SM_LT_EXTERIOR
        Err(SolidError::Upstream {
            operation: OP_REV_STEP_INV,
            code: SM_ERR,
            ..
        }) => Ok((LocationType::Exterior, Point2::UNDEF)),
        Err(e) => {
            // a failed TransformPointToStartPlane returns after SM_LT_EXTERIOR is set
            let pos = cone.positive_point(k, point)?;
            if cone
                .rev
                .transform_point_to_start_plane(pos.point, tol, false)
                .is_err()
            {
                Ok((LocationType::Exterior, Point2::UNDEF))
            } else {
                Err(e)
            }
        }
    }
}

/// Whether `point` on `cone` lies inside the STEP trim domain `anal_uv_domain`
/// (`sm_IsConePointInTrimDomain`): the cone's STEP inversion ignores the domain and
/// only reports whether the point is inside the whole cone, so the height must be in
/// the domain's v interval and the angle (arbitrary at a pole) in its u interval.
fn is_cone_point_in_trim_domain(
    k: &Kernel,
    cone: &Cone,
    anal_uv_domain: &Extent2d,
    point: Point3,
    tol: f64,
) -> Result<bool> {
    let (loc, step_uv) = cone_inversion(k, cone, point, tol)?;
    if loc == LocationType::Exterior {
        return Ok(false);
    }

    if !anal_uv_domain.v_interval().contains_value(step_uv.y, tol) {
        return Ok(false);
    }

    // the sweep angle is arbitrary at a pole
    Ok(loc == LocationType::Pole
        || anal_uv_domain
            .u_interval()
            .contains_periodic_value(step_uv.x, 360.0, -1.0)
            .contained)
}

/// The segment classification of `sm_GetEllipseTrimIntervals` (`bInside[ii]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inside {
    /// `FALSE`.
    No,
    /// `TRUE`.
    Yes,
    /// `UNSURE`: a degenerate segment, classified by its neighbours.
    Unsure,
}

/// The intervals (in degrees of the ellipse's STEP domain) of the ellipse
/// `origin + x_radius cos(t) x_axis + y_radius sin(t) y_axis`, known to lie on the
/// cone, that are inside the cone trimmed to its NURB domain `uv_domain`
/// (`sm_GetEllipseTrimIntervals`). The full ellipse is intersected with the trim
/// circles at the v limits of the trim domain (two-point crossings inside the sweep
/// interval only) and the two seam lines (within the larger of the scaled zero and
/// `tol` of their ends, or always when a circle lies in the ellipse plane); the
/// crossing angles, sorted, cut it into segments whose midpoints are classified
/// against the trim domain (a degenerate segment is inside when both neighbours are
/// outside). Every inside segment is intersected periodically with
/// `[start_ang_deg, end_ang_deg]`. Fewer than two crossings give no interval.
///
/// # Errors
///
/// Those of [`Ellipse::new`]; a seam line parallel to the
/// ellipse plane or of zero length (upstream then reads uninitialised values); a STEP
/// inversion failing in the generator solve. A failed NURB to STEP conversion of the
/// domain corners is ignored as upstream does: the default-constructed domain is used.
pub fn ellipse_trim_intervals(
    k: &Kernel,
    ellipse_origin: Point3,
    ellipse_x_axis: Vec3,
    ellipse_y_axis: Vec3,
    ellipse_x_radius: f64,
    ellipse_y_radius: f64,
    ellipse_start_ang_deg: f64,
    ellipse_end_ang_deg: f64,
    cone: &Cone,
    uv_domain: &Extent2d,
    tol: f64,
) -> Result<Vec<Extent1d>> {
    // init output
    let mut ivls = Vec::new();

    // find ellipse/coneTrimBoundary intersections (at most 6)
    let mut xsect_angle: Vec<f64> = Vec::with_capacity(6);

    // ellipse locals
    let ellipse_z = ellipse_x_axis.cross(ellipse_y_axis);
    let ellipse_ivl = Extent1d::from_min_max(ellipse_start_ang_deg, ellipse_end_ang_deg);
    let full_ellipse = Ellipse::new(
        k,
        ellipse_origin,
        ellipse_x_axis,
        ellipse_y_axis,
        Extent1d::from_min_max(0.0, 360.0),
        ellipse_x_radius,
        ellipse_y_radius,
        &EllipseOptions {
            dim: 3,
            ..EllipseOptions::default()
        },
    )?;

    // map given NurbDomain into StepDomain (SmSurface::ConvertDomainFromNURBSToSTEP,
    // status ignored: a failed corner conversion leaves the default-constructed
    // domain, a reversed result is replaced by the NURB domain)
    let mut anal_uv_domain = Extent2d::new();
    if let Some(min_step_uv) = ignored(cone.convert_uv_from_nurbs_to_step(k, uv_domain.min()))? {
        if let Some(max_step_uv) = ignored(cone.convert_uv_from_nurbs_to_step(k, uv_domain.max()))?
        {
            if anal_uv_domain
                .set_min_max_coords(
                    min_step_uv.x,
                    min2(min_step_uv.y, max_step_uv.y),
                    max_step_uv.x,
                    max2(min_step_uv.y, max_step_uv.y),
                )
                .is_err()
            {
                anal_uv_domain = *uv_domain;
            }
        }
    }
    let sweep_ivl = anal_uv_domain.u_interval();

    // cone locals - bottom and top trim circles sit at the v limits of the trim domain,
    //               which may be a subset of the whole cone
    let cone_axis = cone.rev.position.z_axis();
    let bot_height = anal_uv_domain.min().y;
    let top_height = anal_uv_domain.max().y;
    let bot_radius = cone.radius(bot_height);
    let top_radius = cone.radius(top_height);
    let center_bot = cone.rev.position.origin() + bot_height * cone_axis;
    let center_top = cone.rev.position.origin() + top_height * cone_axis;

    // get cone corners [StartLine start, StartLine end, EndLine start EndLine end]
    // (statuses ignored). Opt-in (WR-490): the seam lines span the cone's whole height,
    // as the end circles and the classification do; only their sweep comes from
    // `uv_domain`. By default their v range comes from `uv_domain` too, as upstream.
    let seam_domain = if k.fixes.ellipse_trim_seam_lines_span_cone_height {
        let cone_v = cone.rev.anal_uv_domain.v_interval();
        let sweep = anal_uv_domain.u_interval();
        Extent2d::from_bounds(sweep.min(), cone_v.min(), sweep.max(), cone_v.max())
    } else {
        anal_uv_domain
    };
    let corner = |u: f64, v: f64| -> Result<Point3> {
        Ok(ignored(cone.evaluate_step_point(seam_domain.evaluate(u, v)))?.unwrap_or(Vec3::UNDEF))
    };
    let corners = [
        corner(0.0, 0.0)?,
        corner(0.0, 1.0)?,
        corner(1.0, 0.0)?,
        corner(1.0, 1.0)?,
    ];
    let [corner0, corner1, corner2, corner3] = corners;
    let start_line_vec = corner1 - corner0;
    let end_line_vec = corner3 - corner2;

    // get ellipse/TopCircle(max 2), ellipse/BotCircle(max 2) ellipse/StartSeamLine(max
    // 1), ellipse/EndSeamLine(max 1) intersections (the circle statuses are ignored:
    // their count is 0 then)
    let circle_hits = |radius: f64, center: Point3| -> Result<CurveIntersection> {
        Ok(ignored(circle_plane_intersect(
            tol,
            radius,
            center,
            cone_axis,
            ellipse_origin,
            ellipse_z,
        ))?
        .unwrap_or(CurveIntersection::Points(Vec::new())))
    };
    let mut tsect1 = circle_hits(top_radius, center_top)?;
    let mut tsect2 = circle_hits(bot_radius, center_bot)?;
    let param0 = line_plane_intersect(corner0, start_line_vec, ellipse_origin, ellipse_z)?;
    let param1 = line_plane_intersect(corner2, end_line_vec, ellipse_origin, ellipse_z)?;

    // Check for degenerate solutions, within tol. [B10]
    for tsect in [&mut tsect1, &mut tsect2] {
        if let CurveIntersection::Points(points) = tsect {
            if let [a, b] = points.as_slice() {
                if a.distance(*b) < tol {
                    points.truncate(1);
                }
            }
        }
    }
    let coincident = matches!(tsect1, CurveIntersection::Coincident)
        || matches!(tsect2, CurveIntersection::Coincident);

    // Consider all 4 possible ConeCircle/Ellipse intersections: only two-point
    // intersections, only points in the cone trim boundary (not on its seams)
    for tsect in [&tsect1, &tsect2] {
        let CurveIntersection::Points(points) = tsect else {
            continue;
        };
        if points.len() != 2 {
            continue;
        }
        for &point in points {
            // map XSectPoint to ConeUV point
            let (loc, cone_step_uv) = cone_inversion(k, cone, point, tol)?;
            // skip circle intersections on seams because the line intersections will
            // catch seam intersections
            if loc == LocationType::Interior
                && sweep_ivl
                    .contains_periodic_value(cone_step_uv.x, 360.0, -1.0)
                    .contained
            {
                // add ellipse angleDeg for intersection to in/out angle array
                xsect_angle.push(full_ellipse.step_inversion(k, point, false, 0.0).0);
            }
        }
    }

    // for StartSeamLine/ellipse and EndSeamLine/ellipse xSects
    let scaled_zero = EFF_ZERO * (1.0 + max2(corner0.max_dimension(), corner3.max_dimension()));

    // circle crossings within tol of a seam line were skipped above,
    // so seam line crossings within tol of the line ends must be kept here
    let param_tol = |line_vec: Vec3| {
        let len = line_vec.length();
        if len > EFF_ZERO {
            max2(scaled_zero, tol / len)
        } else {
            scaled_zero
        }
    };
    let start_param_tol = param_tol(start_line_vec);
    let end_param_tol = param_tol(end_line_vec);
    for (line_start, line_vec, param, param_tol) in [
        (corner0, start_line_vec, param0, start_param_tol),
        (corner2, end_line_vec, param1, end_param_tol),
    ] {
        if coincident || (param >= 0.0 - param_tol && param <= 1.0 + param_tol) {
            let point = line_start + param * line_vec;
            xsect_angle.push(full_ellipse.step_inversion(k, point, false, 0.0).0);
        }
    }

    // When there are no intersections the whole curve must lie outside the cone
    // trim domain.
    let cnt = xsect_angle.len();
    if cnt < 2 {
        return Ok(ivls);
    }

    // order the XSectPoints by angle
    shell_sort_double_array(&mut xsect_angle);

    // the start and end angles of segment ii (the last one includes the zero angle:
    // its start is moved down by 360)
    let segment = |ii: usize| -> Result<(f64, f64)> {
        let next = (ii + 1) % cnt;
        let this_ang = *xsect_angle
            .get(ii)
            .ok_or(SolidError::invalid_input(OP_TRIM, "segment index"))?;
        let next_ang = *xsect_angle
            .get(next)
            .ok_or(SolidError::invalid_input(OP_TRIM, "segment index"))?;
        let start = if next == 0 {
            this_ang - 360.0
        } else {
            this_ang
        };
        Ok((start, next_ang))
    };

    // now classify the segment midPoints as in/out
    let scaled_ang_zero = EFF_ZERO * 100.0 * 360.0;
    let mut inside = Vec::with_capacity(cnt);
    for ii in 0..cnt {
        let (start_ang_deg, end_ang_deg) = segment(ii)?;

        // mark degenerate intervals as unknown for now
        if end_ang_deg - start_ang_deg < scaled_ang_zero {
            inside.push(Inside::Unsure);
            continue;
        }

        // classify nonDegenerate interval midPoint
        let mid_ang_deg = (end_ang_deg + start_ang_deg) / 2.0;
        let mid_point = full_ellipse.evaluate_step_point(mid_ang_deg)?;

        // Remember if segment MidPointUV is within cone trim boundaries
        inside.push(
            if is_cone_point_in_trim_domain(k, cone, &anal_uv_domain, mid_point, tol)? {
                Inside::Yes
            } else {
                Inside::No
            },
        );
    }

    // add one interval to output for every inside segment
    for ii in 0..cnt {
        let next = (ii + 1) % cnt;
        let prev = (ii + cnt - 1) % cnt;
        let state = |i: usize| inside.get(i).copied();
        let (start_ang_deg, end_ang_deg) = segment(ii)?;

        // when the segment is inside
        if state(ii) == Some(Inside::Yes)
            || (state(ii) == Some(Inside::Unsure)
                && state(prev) == Some(Inside::No)
                && state(next) == Some(Inside::No))
        {
            // make and trim interval to ellipse interval (AddValue: start can be above
            // end by machine noise)
            let mut untrimmed_ivl = Extent1d::new();
            untrimmed_ivl.add_value(start_ang_deg);
            untrimmed_ivl.add_value(end_ang_deg);
            ivls.extend(ellipse_ivl.intersect_periodic(&untrimmed_ivl, 360.0, false)?);
        }
    }
    Ok(ivls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::axis2_placement::Axis2Placement;

    fn kernel() -> Kernel {
        Kernel::default()
    }

    fn canonical(k: &Kernel, bot: f64, top: f64, h: f64, swap: bool, inside: bool) -> Cone {
        Cone::create_canonical(k, &Axis2Placement::IDENTITY, bot, top, h, swap, inside).unwrap()
    }

    #[test]
    fn recognises_its_own_nurb() {
        let k = kernel();
        for (swap, inside) in [(false, false), (true, false), (false, true), (true, true)] {
            let cone = canonical(&k, 2.0, 1.0, 3.0, swap, inside);
            let found = Cone::is_nurb_surface_cone(&k, cone.rev().base(), 1.0)
                .unwrap()
                .unwrap();
            assert!((found.bot_radius() - cone.bot_radius()).abs() < 1e-9);
            assert!((found.top_radius() - cone.top_radius()).abs() < 1e-9);
            assert_eq!(found.rev().swap_uv(), swap);
        }
    }

    #[test]
    fn trim_with_domain_keeps_the_shape() {
        let k = kernel();
        for (swap, inside) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut cone = canonical(&k, 2.0, 1.0, 3.0, swap, inside);
            let d = cone.rev().base().natural_uv_domain();
            let (lo, hi) = (d.min(), d.max());
            let trim = Extent2d::from_bounds(
                lo.x + 0.25 * (hi.x - lo.x),
                lo.y + 0.125 * (hi.y - lo.y),
                lo.x + 0.75 * (hi.x - lo.x),
                lo.y + 0.5 * (hi.y - lo.y),
            );
            cone.trim_with_domain(&k, &trim).unwrap();
            let nd = cone.rev().base().natural_uv_domain();
            assert!(nd.is_contained_by(&trim, 1e-12) && trim.is_contained_by(&nd, 1e-12));
            let p = cone.rev().base().evaluate_point(trim.max()).unwrap();
            let back = cone.convert_uv_from_nurbs_to_step(&k, trim.max()).unwrap();
            let q = cone.evaluate_step_point(back).unwrap();
            assert!((p - q).length() < 1e-9);
            // the cone through the trimmed corner keeps its radius at that height
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!((r - cone.radius(p.z)).abs() < 1e-9);
            // a domain outside the current one is rejected
            assert!(cone.trim_with_domain(&k, &d).is_err());
        }
    }

    #[test]
    fn rejects_non_cones() {
        let k = kernel();
        let bs = BSplineSurface::create_bilinear_surface(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        )
        .unwrap();
        assert!(Cone::is_nurb_surface_cone(&k, &bs, 1.0).unwrap().is_none());
    }

    #[test]
    fn rebuild_keeps_the_cone() {
        let k = kernel();
        let mut cone = canonical(&k, 2.0, 1.0, 3.0, false, false);
        let before = cone.clone();
        cone.rebuild_step_from_nurb_parameters(&k).unwrap();
        assert!((cone.base_radius() - before.base_radius()).abs() < 1e-9);
        assert!((cone.semi_angle_deg() - before.semi_angle_deg()).abs() < 1e-7);
    }

    // Edit the actual homogeneous NURB net, as SwapUV/canonicalisation can do,
    // while retaining the object's SmCylinder class and its old analytic state.
    fn edit_last_radius(cone: &mut Cone, factor: f64) {
        let swap = cone.rev().swap_uv();
        let s = cone.base_mut().get_or_create_gw_nurb().unwrap();
        let (n, m) = (s.n(), s.m());
        for i in 0..=n {
            for j in 0..=m {
                if if swap { i == n } else { j == m } {
                    let p = s.cpt_mut(i, j).unwrap();
                    p.x *= factor;
                    p.y *= factor;
                }
            }
        }
    }

    fn edited_cylinder(k: &Kernel, radius: f64, top: f64, swap: bool, inside: bool) -> Cone {
        let mut c = Cone::new_cylinder_bounded(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            radius,
            0.0,
            360.0,
            3.0,
            swap,
            inside,
            false,
        );
        c.make_nurb(k).unwrap();
        edit_last_radius(&mut c, top / radius);
        c
    }

    #[test]
    fn rebuild_cylinder_roundoff_preserves_zero_semi_angle() {
        let mut k = kernel();
        k.fixes.cylinder_rebuild_mean_radius = true;
        // The radii recovered in issue #713, plus both sides of the rounding
        // allowance at different model scales and in both parameter directions.
        for (radius, top) in [(9.0000000000985, 9.0000000000757)].into_iter().chain(
            [0.009, 9.0, 90_000.0]
                .into_iter()
                .flat_map(|r| [-0.5, 0.5].map(|sign| (r, r + sign * EFF_ZERO_SQRT * (1.0 + r)))),
        ) {
            for swap in [false, true] {
                for inside in [false, true] {
                    let mut c = edited_cylinder(&k, radius, top, swap, inside);
                    c.rebuild_step_from_nurb_parameters(&k).unwrap();
                    assert!(c.is_cylinder_class());
                    assert_eq!(c.semi_angle_deg(), 0.0, "{radius} {top} {swap} {inside}");
                    let mean = (radius + top) / 2.0;
                    assert!((c.base_radius() - mean).abs() < 1e-12 * (1.0 + mean));
                    assert_eq!(c.bot_radius(), c.top_radius());
                }
            }
        }
    }

    #[test]
    fn rebuild_cylinder_rejects_a_taper_before_changing_analytic_state() {
        let mut k = kernel();
        k.fixes.cylinder_rebuild_mean_radius = true;
        for radius in [0.009, 9.0, 90_000.0] {
            for sign in [-2.0, 2.0] {
                for swap in [false, true] {
                    for inside in [false, true] {
                        let top = radius + sign * EFF_ZERO_SQRT * (1.0 + radius);
                        let mut c = edited_cylinder(&k, radius, top, swap, inside);
                        let before = c.clone();
                        assert!(c.rebuild_step_from_nurb_parameters(&k).is_err());
                        assert_eq!(c.base_radius(), before.base_radius());
                        assert_eq!(c.semi_angle_deg(), before.semi_angle_deg());
                        assert_eq!(c.rev().position(), before.rev().position());
                        assert_eq!(c.rev().step_uv_domain(), before.rev().step_uv_domain());
                    }
                }
            }
        }
    }

    #[test]
    fn rebuild_shallow_cone_keeps_its_nonzero_angle() {
        let k = kernel();
        for sign in [-0.5, 0.5] {
            let radius = 9.0;
            let top = radius + sign * EFF_ZERO_SQRT * (1.0 + radius);
            let mut c = Cone::new_bounded(
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y,
                radius,
                top,
                0.0,
                360.0,
                3.0,
                false,
                false,
                false,
            );
            c.make_nurb(&k).unwrap();
            assert!(!c.is_cylinder_class());
            let mut fixed_k = kernel();
            fixed_k.fixes.cylinder_rebuild_mean_radius = true;
            let mut fixed = c.clone();
            c.rebuild_step_from_nurb_parameters(&k).unwrap();
            fixed.rebuild_step_from_nurb_parameters(&fixed_k).unwrap();
            let expected = (top - radius).atan2(3.0) * 180.0 / PI;
            assert!((c.semi_angle_deg() - expected).abs() < 1e-12);
            assert_ne!(c.semi_angle_deg(), 0.0);
            assert_eq!(fixed.semi_angle_deg(), c.semi_angle_deg());
            assert_eq!(fixed.base_radius(), c.base_radius());
        }
    }

    #[test]
    fn rebuild_cylinder_small_arc_clears_a_stale_semi_angle() {
        let mut k = kernel();
        k.fixes.cylinder_rebuild_mean_radius = true;
        for swap in [false, true] {
            for inside in [false, true] {
                let mut c = Cone::new_cylinder_bounded(
                    Vec3::ZERO,
                    Vec3::X,
                    Vec3::Y,
                    9.0,
                    0.0,
                    360.0 / 256.0,
                    3.0,
                    swap,
                    inside,
                    false,
                );
                c.make_nurb(&k).unwrap();
                // An earlier buggy rebuild can leave this angle even after the
                // NURB is restored to an exact cylinder. Equal radii must reset it.
                c.semi_angle_deg = -4.3550699142343645e-10;
                let mut pinned = c.clone();
                pinned.rebuild_step_from_nurb_parameters(&kernel()).unwrap();
                assert_eq!(pinned.semi_angle_deg(), c.semi_angle_deg());
                c.rebuild_step_from_nurb_parameters(&k).unwrap();
                assert_eq!(c.semi_angle_deg(), 0.0);
                assert!((c.base_radius() - 9.0).abs() < 1e-8);
                assert_eq!(c.bot_radius(), c.top_radius());
            }
        }
    }

    /// source issue #713 (WR-385): a cylinder whose NURB radii differ by
    /// 2.25e-11 (rounding, as after `SwapUV`) gets a semi-angle upstream, which its own
    /// check forbids; with the opt-in the radii are its one radius.
    #[test]
    fn issue_713_cylinder_rebuild_keeps_zero_semi_angle() {
        let mut k = kernel();
        let make = |k: &Kernel| {
            let mut c = Cone::create_canonical_cylinder(k, &Axis2Placement::IDENTITY, 9.0).unwrap();
            let linear_u = c.rev.base.degree(SurfParamType::U) == 1;
            let nurb = c.rev.base.nurb.as_mut().unwrap();
            let (n, m) = (nurb.n(), nurb.m());
            for i in 0..=n {
                for j in 0..=m {
                    if (linear_u && i == n) || (!linear_u && j == m) {
                        let p = nurb.cpt_mut(i, j).unwrap();
                        p.x *= 1.0 + 2.5e-12;
                        p.y *= 1.0 + 2.5e-12;
                    }
                }
            }
            c
        };
        let mut up = make(&k);
        up.rebuild_step_from_nurb_parameters(&k).unwrap();
        assert!(up.semi_angle_deg() != 0.0);

        k.fixes.cylinder_rebuild_mean_radius = true;
        let mut fixed = make(&k);
        fixed.rebuild_step_from_nurb_parameters(&k).unwrap();
        assert_eq!(fixed.semi_angle_deg(), 0.0);
        assert!((fixed.base_radius() - 9.0).abs() < 1e-9);
    }

    #[test]
    fn offset_moves_radii() {
        let k = kernel();
        let cone = canonical(&k, 2.0, 1.0, 3.0, false, false);
        let off = cone.create_offset_surface(&k, 0.5, 1e-6).unwrap().unwrap();
        let semi = cone.semi_angle_deg() * PI / 180.0;
        assert!((off.base_radius() - (2.0 + semi.cos() * 0.5)).abs() < 1e-12);
        // both radii below the tolerance: no surface
        assert!(cone
            .create_offset_surface(&k, -10.0, 1e-6)
            .unwrap()
            .is_none());
        // one radius below the tolerance: truncated at the new apex
        let trunc = cone.create_offset_surface(&k, -1.2, 1e-6).unwrap().unwrap();
        assert!(trunc.top_radius().abs() < 1e-12);
    }

    #[test]
    fn offset_with_non_finite_truncation_height_returns() {
        // bottom radius 0 and a negative top radius: the truncation height is h / 0
        let k = kernel();
        let mut cone = Cone::new_bounded(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            0.0,
            -1.0,
            0.0,
            360.0,
            2.0,
            false,
            false,
            false,
        );
        cone.make_nurb(&k).unwrap();
        for d in [0.5, 1e-3, 3.0, -0.5] {
            let _ = cone.create_offset_surface(&k, d, 1e-6);
        }
    }

    #[test]
    fn trim_of_a_cross_section() {
        let k = kernel();
        let cone = canonical(&k, 2.0, 1.0, 3.0, false, false);
        // the circle at half height, a little tilted plane through the axis point
        let r = cone.radius(1.5);
        let ivls = ellipse_trim_intervals(
            &k,
            Vec3::new(0.0, 0.0, 1.5),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            r,
            r,
            0.0,
            360.0,
            &cone,
            &cone.rev().base().natural_uv_domain(),
            1e-6,
        );
        // the circle lies in no seam-line plane crossing: the result is whatever the
        // classification gives, but it must not fail
        assert!(ivls.is_ok());
    }

    /// WR-490: total length (deg) of the trim intervals of an ellipse on a cylinder r 10,
    /// h 100 trimmed to v >= 30, with `ellipse_trim_seam_lines_span_cone_height` set to
    /// `fix`, and of the same ellipse on the natural domain.
    fn ellipse_on_a_trimmed_cylinder(fix: bool) -> (f64, f64) {
        // A face already joined to another body: the ellipse of a plane through the axis
        // point (0, 0, 50) that meets the seam (x = 10, y = 0) only at its lowest point,
        // z = 20, in the trimmed-off part. Upstream takes the seam lines from the trimmed
        // domain: at c9c5979 no crossing at all, and the ellipse came back as outside the
        // cylinder; v1.0.1 adds the trim circle at v = 30 (see the tests below).
        let mut k = kernel();
        k.fixes.ellipse_trim_seam_lines_span_cone_height = fix;
        let cyl = canonical(&k, 10.0, 10.0, 100.0, false, false);
        let d = cyl.rev().base().natural_uv_domain();
        let (lo, hi) = (d.min(), d.max());
        let trim = Extent2d::from_bounds(lo.x, lo.y + 0.3 * (hi.y - lo.y), hi.x, hi.y);
        let step_min = cyl.convert_uv_from_nurbs_to_step(&k, trim.min()).unwrap();
        assert!(step_min.y > 20.0 + 1e-6 && step_min.y < 50.0);
        let x = Vec3::new(1.0, 0.0, -3.0) * (1.0 / 10.0_f64.sqrt());
        let rx = 1000.0_f64.sqrt();
        let lowest = Vec3::new(0.0, 0.0, 50.0) + rx * x;
        assert!((lowest - Vec3::new(10.0, 0.0, 20.0)).length() < 1e-12);
        let ivls = ellipse_trim_intervals(
            &k,
            Vec3::new(0.0, 0.0, 50.0),
            x,
            Vec3::new(0.0, 1.0, 0.0),
            rx,
            10.0,
            0.0,
            360.0,
            &cyl,
            &trim,
            1e-6,
        )
        .unwrap();
        let natural = ellipse_trim_intervals(
            &k,
            Vec3::new(0.0, 0.0, 50.0),
            x,
            Vec3::new(0.0, 1.0, 0.0),
            rx,
            10.0,
            0.0,
            360.0,
            &cyl,
            &d,
            1e-6,
        )
        .unwrap();
        let total = |v: &[Extent1d]| v.iter().map(Extent1d::length).sum::<f64>();
        (total(&ivls), total(&natural))
    }

    /// The part of the ellipse of [`ellipse_on_a_trimmed_cylinder`] inside the trimmed
    /// domain: `z = 50 - 30 cos t >= 30`, so `cos t <= 2/3`.
    fn ellipse_part_above_v30() -> f64 {
        360.0 - 2.0 * (2.0_f64 / 3.0).acos().to_degrees()
    }

    #[test]
    fn seam_crossing_below_a_trimmed_domain_keeps_the_ellipse() {
        // WR-490, opt-in: the seam lines span the cylinder's height; since v1.0.1 the
        // trim circles sit at the trim domain's v limits, so the part of the ellipse
        // inside the trimmed domain, as without the field
        let (trimmed, natural) = ellipse_on_a_trimmed_cylinder(true);
        assert!((natural - 360.0).abs() < 1e-9);
        assert!(
            (trimmed - ellipse_part_above_v30()).abs() < 1e-9,
            "{trimmed}"
        );
    }

    #[test]
    fn seam_crossing_below_a_trimmed_domain_keeps_the_part_inside_by_default() {
        // WR-490, default: v1.0.1 intersects the ellipse with the trim circle at v = 30
        // and classifies against the trim domain, so the part above it (the C++ 1.0.1
        // agrees: surface.cone_p2 fixture wr490_ellipse_on_cylinder_trimmed_v30)
        let (trimmed, natural) = ellipse_on_a_trimmed_cylinder(false);
        assert!((natural - 360.0).abs() < 1e-9);
        assert!(
            (trimmed - ellipse_part_above_v30()).abs() < 1e-9,
            "{trimmed}"
        );
    }

    /// The control points and knots of two NURBs agree bit for bit.
    fn same_nurb(a: &BSplineSurface, b: &BSplineSurface) -> bool {
        match (a.gw_nurb(), b.gw_nurb()) {
            (Some(a), Some(b)) => {
                let rows = |s: &crate::nurbs::srf_basic::Surface| {
                    s.rows()
                        .flat_map(|r| r.iter().map(|p| [p.x, p.y, p.z, p.w]))
                        .collect::<Vec<_>>()
                };
                rows(a) == rows(b) && a.u_knots() == b.u_knots() && a.v_knots() == b.v_knots()
            }
            _ => false,
        }
    }

    /// source issue #1433: the cylinder test finds the radius 5 cylinder
    /// about +Z from its NURB form and keeps exactly that NURB.
    #[test]
    fn issue_1433_cylinder_found_in_its_nurb_keeps_it() {
        let k = kernel();
        let cyl = Cone::create_canonical_cylinder(&k, &Axis2Placement::default(), 5.0).unwrap();
        let nurbs = cyl.rev().base().clone();
        let found = Cone::is_nurb_surface_cylinder(&k, &nurbs, 1.0)
            .unwrap()
            .expect("a cylinder");
        assert!(found.is_cylinder());
        assert_eq!(found.semi_angle_deg(), 0.0);
        assert!(
            (found.base_radius() - 5.0).abs() < 1e-12,
            "{}",
            found.base_radius()
        );
        let z = found
            .rev()
            .position()
            .x_axis()
            .cross(found.rev().position().y_axis());
        assert!(z.is_parallel_to(Vec3::new(0.0, 0.0, 1.0), 1e-9), "{z:?}");
        assert!(found.rev().position().origin().x.abs() < 1e-12);
        assert!(found.rev().position().origin().y.abs() < 1e-12);
        assert!(same_nurb(found.rev().base(), &nurbs));
    }

    /// A bounded partial cylinder (either UV order, inside out) is found with its
    /// radius, angles and height; a cone and a tapered cylinder NURB are not cylinders.
    #[test]
    fn cylinder_recognition_partial_and_non_matching() {
        let k = kernel();
        for (swap, inside) in [(false, false), (true, false), (false, true), (true, true)] {
            let frame = Axis2Placement::new(
                Vec3::new(1.0, -2.0, 3.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            );
            let mut cyl = Cone::new_cylinder_bounded(
                frame.origin(),
                frame.x_axis(),
                frame.y_axis(),
                2.5,
                30.0,
                200.0,
                4.0,
                swap,
                inside,
                false,
            );
            cyl.make_nurb(&k).unwrap();
            let nurbs = cyl.rev().base().clone();
            let found = Cone::is_nurb_surface_cylinder(&k, &nurbs, 1.0)
                .unwrap()
                .expect("a cylinder");
            assert!((found.base_radius() - 2.5).abs() < 1e-12);
            assert!(same_nurb(found.rev().base(), &nurbs));
            let d = found.rev().step_uv_domain();
            assert!((d.max().x - d.min().x - 170.0).abs() < 1e-9, "{d:?}");
            assert!((d.max().y - d.min().y - 4.0).abs() < 1e-9, "{d:?}");
        }
        let cone = canonical(&k, 2.0, 30.0, 3.0, false, false);
        assert!(Cone::is_nurb_surface_cylinder(&k, cone.rev().base(), 1.0)
            .unwrap()
            .is_none());
    }

    /// `lean_cylinder`: a cylinder of radius 5 (frame at (1, 2, 3), heights 0 to 10) leaning at
    /// 0.1 per unit of height, radius 5 at height 4. Same parameters point for point, the radius
    /// the straight law of the height, the cone's own data (base radius, semi-angle) rebuilt.
    #[test]
    fn lean_cylinder_keeps_the_parameters_and_changes_the_radius() {
        let mut k = kernel();
        k.fixes.cylinder_rebuild_mean_radius = true;
        let frame = Axis2Placement::new(Point3::new(1.0, 2.0, 3.0), Vec3::X, Vec3::Y);
        let mut cylinder = Cone::create_canonical_cylinder(&k, &frame, 5.0).unwrap();
        let mut domain = Extent2d::default();
        domain.set_min_max_coords(0.0, 0.0, 360.0, 10.0).unwrap();
        cylinder.adjust_step_uv_domain(&k, &domain).unwrap();
        let slope = 0.1;
        let cone = cylinder.lean_cylinder(&k, slope, 4.0).unwrap();
        assert!(!cone.is_cylinder() && !cone.is_cylinder_class());
        assert_eq!(
            cone.rev().base().natural_uv_domain(),
            cylinder.rev().base().natural_uv_domain()
        );
        assert!((cone.canonical().radius - (5.0 - 0.4)).abs() < 1e-9);
        assert!((cone.canonical().semi_angle_deg - slope.atan().to_degrees()).abs() < 1e-9);
        let nurb = cylinder.rev().base().natural_uv_domain();
        for i in 0..=6 {
            for j in 0..=3 {
                let uv = Point2::new(
                    nurb.min().x + (nurb.max().x - nurb.min().x) * f64::from(i) / 6.0,
                    nurb.min().y + (nurb.max().y - nurb.min().y) * f64::from(j) / 3.0,
                );
                let (old, new) = (
                    cylinder.rev().base().evaluate_point(uv).unwrap(),
                    cone.rev().base().evaluate_point(uv).unwrap(),
                );
                let (d_old, d_new) = (
                    old - Point3::new(1.0, 2.0, old.z),
                    new - Point3::new(1.0, 2.0, new.z),
                );
                let height = old.z - 3.0;
                assert!((new.z - old.z).abs() < 1e-9, "{uv:?}");
                assert!(
                    (d_new.length() - (5.0 + slope * (height - 4.0))).abs() < 1e-9,
                    "{uv:?}"
                );
                assert!(d_old.cross(d_new).length() < 1e-9 * 25.0, "{uv:?}");
            }
        }
        // a radius that would go through the axis is refused
        assert!(cylinder.lean_cylinder(&k, 2.0, 4.0).is_err());
    }
}
