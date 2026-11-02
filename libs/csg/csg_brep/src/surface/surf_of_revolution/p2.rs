//! `SmSurfOfRevolution`, part 2: the STEP domain adjustment and NURB trim, the STEP
//! inversion, the offset surface, the dispatch of the surface/surface intersection
//! and the recogniser that makes a surface of revolution of a B-spline surface.
//!
//! Upstream: `source/SMLib/src/SmSurfOfRevolution.cpp`,
//! `source/SMLib/inc/SmSurfOfRevolution.h`.
//!
//! | Rust                                                        | Upstream                                  |
//! |-------------------------------------------------------------|-------------------------------------------|
//! | [`SurfOfRevolution::adjust_step_uv_domain`]                 | `AdjustSTEPUVDomain`                      |
//! | [`SurfOfRevolution::step_inversion`]                        | `STEPInversion`                           |
//! | [`SurfOfRevolution::create_canonical`]                      | `CreateCanonical` (co-ported in `p1`)     |
//! | [`SurfOfRevolution::make_nurb`]                             | `MakeNurb` (co-ported in `p1`)            |
//! | [`SurfOfRevolution::create_offset_surface`]                 | `CreateOffsetSurface`                     |
//! | [`SurfOfRevolution::global_surface_intersect`]              | `GlobalSurfaceIntersect`                  |
//! | [`SurfOfRevolution::is_nurb_surface_surf_of_revolution`]    | `IsNurbSurfaceSurfOfRevolution`           |
//! | [`SurfOfRevolution::trim_with_domain`]                      | `TrimWithDomain`                          |
//!
//! Deviations:
//! - **Arguments.** Out-params become return values; `SmContext` arguments, the
//!   `Notify` calls and `TrimWithDomain`'s in/out domain (upstream never writes it) are
//!   dropped. `STEPInversion` and `GlobalSurfaceIntersect` take the kernel for the
//!   conversions and intersections they run.
//! - **`STEPInversion`** returns the parameter and location only on success: upstream
//!   writes them before returning `SM_ERR` for a point outside the sweep interval or
//!   not on the generator, and the port returns that error alone.
//! - **Generator.** As in `p1` the generator is a [`Curve`] derived from
//!   `SmBSplineCurve`; `CreateOffsetSurface`'s composite-curve branch for any other
//!   generator is unreachable. Upstream's `STEPInversion` switches the generator's
//!   out-of-bounds flag on; the surface's flag always sets the generator's too
//!   ([`SurfOfRevolution::set_out_of_bounds_enabled`]), so the generator is solved on a
//!   copy with the flag on.
//! - **`CreateOffsetSurface`** without a non-degenerate sweep circle (upstream asserts)
//!   passes no sweep curve to `MakeNurbWithSweepParams`.
//! - **`GlobalSurfaceIntersect`** reaches `SmSphere` and `SmCone` through
//!   [`SurfOfRevolution::intersect_with_surf_of_revolution`] once those surface kinds
//!   exist; until then a sphere or cone `other` cannot be given.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates.

use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_point_distance;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::Point3;
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::LocationType;
use crate::nurbs::b_spline_surface::{BSplineSurface, IsoCurveOptions, SurfParamType};
use crate::surface::plane::p2::PlaneOwner;
use crate::surface::surface::p1::SurfaceIntersectContext;
use crate::surface::surface::p2::{
    base_global_surface_intersect_into, Surface, SurfaceIntersection, SurfaceKind,
    B_SPLINE_SURFACE_TYPE,
};

use super::p1::{
    gen_adjust_step_interval, gen_base, gen_base_mut, gen_max_analytic_domain, gen_step_interval,
    not_b_spline, SurfOfRevolution, SweepIsoFn,
};
use super::p3::intersect_revolved_into;

const OP_ADJUST: &str = "surface::surf_of_revolution::p2::SurfOfRevolution::adjust_step_uv_domain";
const OP_STEP_INV: &str = "surface::surf_of_revolution::p2::SurfOfRevolution::step_inversion";
const OP_TRIM: &str = "surface::surf_of_revolution::p2::SurfOfRevolution::trim_with_domain";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// `SM_ERR_OUTSIDE_OF_DOMAIN` (`SmMessages.h`).
const SM_ERR_OUTSIDE_OF_DOMAIN: i32 = 1020;

/// The solution object of solves whose solutions name no object.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NoHandle;

impl SolutionObject for NoHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
}

/// The outputs of [`SurfOfRevolution::step_inversion_with_outputs`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StepInversionOutputs {
    /// STEP parameters: angle in degrees, generator parameter (`rdAnalUVParameter`).
    pub(crate) uv: Point2,
    /// The point's location (`reLocation`).
    pub(crate) location: LocationType,
    /// The point is in the sweep interval (upstream returns `SM_ERR` otherwise).
    pub(crate) inside: bool,
}

/// A curve named by the solutions of a curve intersection: its natural interval (the
/// solvers' duplicate tests read it through the object, as upstream does).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

impl SurfOfRevolution {
    // ---- domain edits ---------------------------------------------------------------

    /// Sets the STEP domain `new_step_uv_domain` (degrees by generator parameters),
    /// adjusts the generator to its V interval and rebuilds the NURB and the polar
    /// converter (`AdjustSTEPUVDomain`). Nothing happens when the domain is the
    /// current one (within `SM_EFF_ZERO`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator's
    /// `AdjustSTEPInterval` and of [`Self::make_nurb`].
    pub fn adjust_step_uv_domain(
        &mut self,
        k: &Kernel,
        new_step_uv_domain: &Extent2d,
    ) -> Result<()> {
        self.adjust_step_uv_domain_using(k, new_step_uv_domain, &Self::own_sweep_iso)
    }

    /// [`Self::adjust_step_uv_domain`] with the polar converter rebuilt from the sweep
    /// iso curves of `iso` (upstream's virtual `Reparameterize` / `MakeNurb` /
    /// `CreateIsoParametricCurve`: a derived class such as `SmCone` passes its
    /// override).
    ///
    /// # Errors
    ///
    /// Those of [`Self::adjust_step_uv_domain`].
    pub(crate) fn adjust_step_uv_domain_using(
        &mut self,
        k: &Kernel,
        new_step_uv_domain: &Extent2d,
        iso: SweepIsoFn<'_>,
    ) -> Result<()> {
        // no work - input domain = current domain
        if new_step_uv_domain.is_contained_by(&self.anal_uv_domain, EFF_ZERO)
            && self
                .anal_uv_domain
                .is_contained_by(new_step_uv_domain, EFF_ZERO)
        {
            return Ok(());
        }

        // set surface analytic domain
        self.anal_uv_domain = *new_step_uv_domain;

        // set GenCurve analytic domain from V part of new surface domain
        let v_ivl = self.anal_uv_domain.v_interval();
        let gen = self
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_ADJUST))?;
        gen_adjust_step_interval(k, gen, &v_ivl)?;

        // build Nurb domain from Analytic domain
        let mut nurb_domain = *new_step_uv_domain;
        if self.swap_uv {
            nurb_domain.transpose();
        }

        // set Surface and GenCurve nurb domain (status ignored)
        ignore_status(self.reparameterize_using(k, &nurb_domain, iso))?;

        // rebuild the associated m_pNurb and PolarConverter
        self.make_nurb_with_sweep_params_using(k, None, iso)?;
        Ok(())
    }

    /// Trims to the NURB domain `new_nurb_uv_domain` (`TrimWithDomain`): the STEP
    /// domain and the generator are updated to it, the NURB is rebuilt on the
    /// trimmed STEP domain and then reparameterised onto the new NURB domain, and the
    /// polar converter is rebuilt (statuses of the update, the reparameterisation and
    /// the rebuild ignored). The surface geometry is preserved; the NURB
    /// parameterisation between the domain corners may differ slightly.
    ///
    /// # Errors
    ///
    /// `SM_ERR_OUTSIDE_OF_DOMAIN` (1020) when the domain is not inside the current NURB
    /// domain (within `SM_EFF_ZERO_PARAM`); those of [`Self::make_nurb`].
    pub fn trim_with_domain(&mut self, k: &Kernel, new_nurb_uv_domain: &Extent2d) -> Result<()> {
        self.trim_with_domain_using(
            k,
            new_nurb_uv_domain,
            &|s, k, uv| s.convert_uv_from_nurbs_to_step_or_written(k, uv),
            &Self::own_sweep_iso,
        )
    }

    /// [`Self::trim_with_domain`] with upstream's virtual calls taken from a derived
    /// class (`SmSphere`, `SmTorus`): `convert` is its `ConvertUVFromNURBSToSTEP` (in
    /// `UpdateAnalyticalDomain`) and `iso` its `CreateIsoParametricCurve` (in
    /// `MakeNurb`, `Reparameterize` and `RebuildPolarConverter`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::trim_with_domain`]; those of `convert` (upstream ignores the
    /// status and reads the corner it may not have written).
    pub(crate) fn trim_with_domain_using(
        &mut self,
        k: &Kernel,
        new_nurb_uv_domain: &Extent2d,
        convert: &dyn Fn(&Self, &Kernel, Point2) -> Result<Point2>,
        iso: SweepIsoFn<'_>,
    ) -> Result<()> {
        // locals
        let current = self.base.natural_uv_domain();

        // check input - new trim UV domain must be inside current natural UV domain
        if !new_nurb_uv_domain.is_contained_by(&current, EFF_ZERO_PARAM) {
            return Err(SolidError::Upstream {
                operation: OP_TRIM,
                code: SM_ERR_OUTSIDE_OF_DOMAIN,
                entity: None,
            });
        }

        // no work - new and current domains are same size
        if current.is_contained_by(new_nurb_uv_domain, EFF_ZERO_PARAM) {
            return Ok(());
        }

        // Update just the analytic domain and the GenCurve (both analytic and NURB)
        let min_anal_uv = convert(self, k, new_nurb_uv_domain.min())?;
        let max_anal_uv = convert(self, k, new_nurb_uv_domain.max())?;
        ignore_status(self.update_analytical_domain_from_step(
            k,
            new_nurb_uv_domain,
            min_anal_uv,
            max_anal_uv,
        ))?;

        // rebuild Nurbs surface from updated analytic domain. (right shape - wrong
        // UVDomain)
        self.make_nurb_with_sweep_params_using(k, None, iso)?;

        // adjust Nurb Surface UVDomain (GetOrCreateGwNurbPointer: the virtual MakeNurb
        // when missing, status ignored)
        if self.base.nurb.is_none() {
            ignore_status(self.make_nurb_with_sweep_params_using(k, None, iso))?;
        }
        ignore_status(self.reparameterize_using(k, new_nurb_uv_domain, iso))?;

        // rebuild the sweep polar converter
        ignore_status(self.rebuild_polar_converter(k, iso))?;
        Ok(())
    }

    // ---- inversion ------------------------------------------------------------------

    /// The STEP parameters of `point_on_surf` (which must lie on the surface within
    /// `tol_3d`) and its location (`STEPInversion`): degrees about the axis and the
    /// generator's STEP parameter. A non-planar generator runs
    /// [`BSplineSurface::step_inversion`] over the NURB image of `anal_domain`. A planar
    /// generator is solved analytically: the point is swung onto the generator's plane,
    /// the sweep angle (the larger of two on a seam when it is below the end of the
    /// sweep) gives the U parameter, the point's distance from the axis a pole, and
    /// the swung point is intersected with the generator for the V parameter. On a
    /// seam, `uv_guess` picks the end nearer to it.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a point outside the sweep interval or not on the generator; those of
    /// the conversions, the start-plane transform and the generator solve;
    /// [`SolidError::Unported`] from [`BSplineSurface::step_inversion`] (non-planar
    /// generator).
    pub fn step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        let r =
            self.step_inversion_with_outputs(k, anal_domain, point_on_surf, tol_3d, uv_guess)?;
        if r.inside {
            Ok((r.uv, r.location))
        } else {
            Err(SolidError::sm_err(OP_STEP_INV))
        }
    }

    /// The non-planar generator branch of `STEPInversion`: the general B-spline
    /// inversion over the NURB image of `anal_domain`, with the parameter conversions
    /// upstream calls virtually (`ConvertUVFromSTEPToNURBS` /
    /// `ConvertUVFromNURBSToSTEP`): this class's own, or those of a derived class that
    /// inherits `STEPInversion` (`SmTorus`, `SmCone`). The B-spline inversion's own
    /// virtual solves run on `surface` (the derived surface upstream's `this` is) when
    /// given, else on a plain B-spline surface of this NURB
    /// ([`BSplineSurface::step_inversion`]).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::step_inversion`] and of `to_step`;
    /// [`SolidError::Unported`] from `to_nurbs` (its other errors are ignored, as
    /// upstream ignores the status).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_inversion_non_planar(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
        surface: Option<&Surface>,
        to_nurbs: impl Fn(Point2) -> Result<Point2>,
        to_step: impl Fn(Point2) -> Result<Point2>,
    ) -> Result<StepInversionOutputs> {
        // Not planar, must use the general solver.

        // Have to convert the analytical domain to a NURBS domain (the statuses are
        // ignored: a failed conversion leaves the STEP point).
        let mut nurbs_domain = Extent2d::new();
        let step_min = anal_domain.min();
        let nurbs_min = ignored(to_nurbs(step_min))?.unwrap_or(step_min);
        nurbs_domain.add_point(nurbs_min);
        let step_max = anal_domain.max();
        let nurbs_max = ignored(to_nurbs(step_max))?.unwrap_or(step_max);
        nurbs_domain.add_point(nurbs_max);

        let (nurbs_parameter, location) = match surface {
            Some(surface) => self.base.step_inversion_on(
                k,
                surface,
                &nurbs_domain,
                point_on_surf,
                tol_3d,
                uv_guess,
            )?,
            None => self
                .base
                .step_inversion(k, &nurbs_domain, point_on_surf, tol_3d, uv_guess)?,
        };
        let step = to_step(nurbs_parameter)?;
        Ok(StepInversionOutputs {
            uv: step,
            location,
            inside: true,
        })
    }

    /// [`Self::step_inversion`] with the outputs upstream writes before returning
    /// `SM_ERR` for a point outside the sweep interval (`inside` is `false` then),
    /// for derived classes that read them (`SmCone`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::step_inversion`] except the outside-the-sweep `SM_ERR`.
    pub(crate) fn step_inversion_with_outputs(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<StepInversionOutputs> {
        // pass call along for nonPlanar generator curves
        if !self.planar_generator {
            return self.step_inversion_non_planar(
                k,
                anal_domain,
                point_on_surf,
                tol_3d,
                uv_guess,
                None,
                |uv| self.convert_uv_from_step_to_nurbs(k, uv),
                |uv| self.convert_uv_from_nurbs_to_step(k, uv),
            );
        }

        // Generator curve is planar, so we can solve it analytically.

        // Rotate Point to StartPlane
        let sp = self.transform_point_to_start_plane(point_on_surf, tol_3d, false)?;
        let num_angles = sp.angles_deg.len();
        let angle0 = sp.angles_deg.first().copied().unwrap_or(0.0);
        let angle1 = sp.angles_deg.get(1).copied().unwrap_or(0.0);

        // set output U value (angle about rotation axis)
        //  For points on seams and larger angle is less than 360
        //  then the point is a little closer to the high side, else use low side value
        let u_domain = self.anal_uv_domain.u_interval();
        let mut u = if num_angles > 1 && angle1 < u_domain.max() - EFF_ZERO {
            angle1
        } else {
            angle0
        };

        // get Point/RotationAxis distance
        let (dist, _) = line_point_distance(
            self.position.origin(),
            self.position.z_axis(),
            point_on_surf,
        );

        // set output point classification
        let mut location = if dist < tol_3d {
            LocationType::Pole
        } else if num_angles == 2 {
            LocationType::USeam
        } else if sp.inside {
            LocationType::Interior
        } else {
            LocationType::Exterior
        };

        // If a guess was given and we're on the seam, set u-parameter to what's closer
        // to the guess.
        if location == LocationType::USeam {
            if let Some(guess) = uv_guess {
                if guess.x > u_domain.mid() {
                    u = u_domain.max();
                }
            }
        }

        // intersect rotated point with genCurve
        let mut gen = self.gen()?.clone();
        let mut ivl = gen_step_interval(&gen)?;
        if self.is_out_of_bounds_enabled() {
            ivl = gen_max_analytic_domain(k, &gen)?;
            if let Some(b) = gen_base_mut(&mut gen) {
                b.set_out_of_bounds_enabled(true);
            }
        }
        let solutions = gen.global_point_solve_step::<NoHandle>(
            k,
            &ivl,
            SolverOperation::Intersect,
            sp.point,
            tol_3d,
            None,
            None,
            SolutionRequested::All,
            None,
            None,
        )?;

        // error - rotated point not on GenCurve
        let Some(first) = solutions.first() else {
            return Err(SolidError::sm_err(OP_STEP_INV));
        };

        // set output V coordinate
        let mut v = first.start.param(0)?;

        // for points on closed genCurve seams - update point classification
        if solutions.len() > 1
            && gen.is_closed(k, &ivl, tol_3d)?
            && ivl.is_value_on_boundary(v, EFF_ZERO).0
        {
            location = match location {
                LocationType::Interior => LocationType::VSeam,
                LocationType::USeam => LocationType::UvSeam,
                other => other,
            };

            // On the v-seam. If a guess was given, set v-parameter to what's closer to
            // the guess.
            if let Some(guess) = uv_guess {
                let v_domain = self.anal_uv_domain.v_interval();
                if guess.y > v_domain.mid() {
                    v = v_domain.max();
                }
            }
        }

        // all done
        Ok(StepInversionOutputs {
            uv: Point2::new(u, v),
            location,
            inside: sp.inside,
        })
    }

    // ---- offsets --------------------------------------------------------------------

    /// The offset surface at the signed distance `signed_offset_distance` (negative:
    /// opposite the surface normal) approximated within `approx_tol_3d`
    /// (`CreateOffsetSurface`): the generator is offset in its plane (lines and circles
    /// stay lines and circles), simplified, cut at its crossing of the axis (the larger
    /// part is kept) and swept by [`Self::make_nurb_with_sweep_params`] along the first
    /// non-degenerate sweep circle of the surface. The offset surface has the STEP
    /// domain of this one with the offset generator's STEP interval.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the directrices, the generator
    /// offset, the knot removal, the axis segment and the sweep;
    /// [`SolidError::Unported`] where the offset approximation is.
    pub fn create_offset_surface(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
        approx_tol_3d: f64,
    ) -> Result<Self> {
        // locals
        let input_nurb_domain = self.base.natural_uv_domain();
        let gen_curve_nurb_ivl = if self.swap_uv {
            input_nurb_domain.u_interval()
        } else {
            input_nurb_domain.v_interval()
        };

        // Get a nonDegenerate SweepCurve, to pass to MakeNurbWithSweepParams().
        let mut sweep_curve: Option<Curve> = None;
        let cnt: usize = 7;
        for ii in 0..cnt {
            let frac = if ii == 0 {
                0.0
            } else if ii == cnt - 1 {
                1.0
            } else {
                ii as f64 / (cnt - 1) as f64
            };
            let param = gen_curve_nurb_ivl.evaluate(frac);
            let dir = self.create_directrix_at_generator_param(k, param)?;
            if dir.radius > EFF_ZERO_SQRT {
                sweep_curve = Some(dir.directrix);
                break;
            }
        }

        // Offset the genCurve.

        // Get the offset normal vector.
        let offset_distance = signed_offset_distance.abs();
        let sign = if (self.swap_uv || signed_offset_distance < 0.0)
            && !(self.swap_uv && signed_offset_distance < 0.0)
        {
            -1.0
        } else {
            1.0
        };

        // Gen curve is in our (local) x-z plane, so the plane normal is our y axis.
        let plane_norm = (-self.position.y_axis()) * sign;

        // CreateSimpleOffset() preserves SmLines and SmCircles.
        let gen = self.gen()?;
        let tol = approx_tol_3d / 10.0;
        let mut off_curve = match gen.kind() {
            CurveKind::Line(l) => {
                Curve::from(l.create_simple_offset(plane_norm, offset_distance)?.0)
            }
            CurveKind::Ellipse(e) => {
                e.create_simple_offset(k, tol, plane_norm, offset_distance, false)?
                    .0
            }
            CurveKind::BSpline(_) | CurveKind::Hyperbola(_) | CurveKind::Parabola(_) => {
                Curve::from(
                    gen_base(gen)?
                        .create_simple_offset(k, tol, plane_norm, offset_distance, false)?
                        .0,
                )
            }
            _ => return Err(not_b_spline()),
        };

        // simplify the curve if possible
        let base = gen_base_mut(&mut off_curve).ok_or_else(not_b_spline)?;
        base.remove_extra_knots(approx_tol_3d / 10.0)?;

        // Trim the offset curve with the axis of rev to avoid self intersecting results
        let z_axis = self.position.z_axis();
        let mut domain = off_curve.natural_interval();
        let boxes = ignored(off_curve.calculate_bounding_box(
            k,
            &domain,
            BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            },
            true,
        ))?;
        let bbox = boxes.and_then(|b| b.normal).unwrap_or_default();
        let height = bbox.max_dimension() * 1.5; // multiply max dimension by some factor to be sure
        let origin = self.position.origin();
        // Not upstream (WR-343, source issue #973), opt-in
        // `Kernel::fixes.revolution_offset_axis_both_sides`: the axis segment spans the offset
        // curve's whole extent along the axis, on both sides of the frame's origin (it ran
        // from the origin along +z only and missed an offset crossing the axis below it),
        // and the part kept is the one on the generatrix's side (positive radius).
        let both_sides = k.fixes.revolution_offset_axis_both_sides;
        let (axis_start, axis_end) = if both_sides {
            let (mut axial_min, mut axial_max) = (BIG_DOUBLE, -BIG_DOUBLE);
            for corner in bbox.corners() {
                let axial = (corner - origin).dot(z_axis);
                axial_min = axial_min.min(axial);
                axial_max = axial_max.max(axial);
            }
            (
                origin + (axial_min - height) * z_axis,
                origin + (axial_max + height) * z_axis,
            )
        } else {
            (origin, origin + height * z_axis)
        };
        let axis_line = Curve::from(Line::create_line_segment(3, axis_start, axis_end, None)?);
        let axis_ivl = axis_line.natural_interval();
        let opts = CurveIntersectOptions {
            handles: [Some(CurveHandle(domain)), Some(CurveHandle(axis_ivl))],
            ..CurveIntersectOptions::default()
        };
        let solutions = ignored(off_curve.global_curve_intersect::<CurveHandle>(
            k,
            &domain,
            &axis_line,
            &axis_ivl,
            approx_tol_3d,
            &opts,
        ))?;
        if let Some(sols) = solutions {
            if sols.len() == 1 {
                // if intersection is at endpoint, do not bother trimming
                let param = sols
                    .as_slice()
                    .first()
                    .map_or(Ok(0.0), |s| s.start.param(0))?;
                let trim = if both_sides {
                    domain.min() != param && domain.max() != param
                } else {
                    domain.min() != param || domain.max() != param
                };
                if trim {
                    let keep_high = if both_sides {
                        // (WR-343) keep the part on the generatrix's side of the axis
                        // (positive radius along the frame's x axis), not the longer part
                        let low_mid = off_curve.evaluate_point(k, 0.5 * (domain.min() + param))?;
                        let high_mid = off_curve.evaluate_point(k, 0.5 * (param + domain.max()))?;
                        let x_axis = self.position.x_axis();
                        (high_mid - origin).dot(x_axis) > (low_mid - origin).dot(x_axis)
                    } else {
                        // Take the largest portion for now
                        // Eventually use a better algorithm
                        param < domain.mid()
                    };
                    if keep_high {
                        let _ = domain.set_min(param);
                    } else {
                        let _ = domain.set_max(param);
                    }
                    // may snap the interval by tol to existing knots (status ignored)
                    ignore_status(off_curve.trim(k, &mut domain, TrimMode::Notify))?;
                }
            }
        }

        // The offset surface's V interval is the offset curve's STEP interval.
        let mut offset_surf_domain = self.anal_uv_domain;
        offset_surf_domain.set_v_interval(&gen_step_interval(&off_curve)?);

        // create the new offset surface of revolution
        let mut off_surf = Self::new(
            k,
            off_curve,
            self.position.origin(),
            self.position.x_axis(),
            self.position.y_axis(),
            offset_surf_domain,
            self.swap_uv,
        )?;

        // rebuild the underlying Nurb Surface - with current parameterization
        off_surf.make_nurb_with_sweep_params(k, sweep_curve.as_ref())?;
        Ok(off_surf)
    }

    // ---- intersections --------------------------------------------------------------

    /// The intersection curves of this surface over `uv_domain` with `other` over
    /// `other_uv_domain` (`GlobalSurfaceIntersect`): the special cases of a plane or
    /// another surface of revolution ([`Self::intersect_with_plane`],
    /// [`Self::intersect_with_surf_of_revolution`]) and, where they need more, the
    /// general surface intersection ([`base_global_surface_intersect`](crate::surface::surface::p2::base_global_surface_intersect)), which
    /// resets the output arrays first (`SmSurfaceIntersector::DoIntersection`).
    ///
    /// # Errors
    ///
    /// Those of the special cases; [`SolidError::Unported`] from the general
    /// intersection (`SmAdvSurfaceIntersector`).
    pub fn global_surface_intersect(
        &self,
        this: &Surface,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<SurfaceIntersection> {
        self.global_surface_intersect_in::<crate::surface::surface::p4::NoObject>(
            this,
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn global_surface_intersect_in<O: SolutionObject + Copy + PartialEq>(
        &self,
        this: &Surface,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        context: Option<SurfaceIntersectContext<'_, O>>,
    ) -> Result<SurfaceIntersection> {
        let mut out = SurfaceIntersection::default();
        self.global_surface_intersect_into(
            this,
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            context,
            &mut out,
        )?;
        Ok(out)
    }

    /// [`Self::global_surface_intersect_in`] into `out` (upstream's output arrays). On an
    /// error `out` keeps what the failing intersection appended before it, as upstream
    /// leaves its output arrays. The general intersection, when a special case needs
    /// more, resets them first (`SmSurfaceIntersector::DoIntersection`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn global_surface_intersect_into<O: SolutionObject + Copy + PartialEq>(
        &self,
        this: &Surface,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        context: Option<SurfaceIntersectContext<'_, O>>,
        out: &mut SurfaceIntersection,
    ) -> Result<()> {
        let mut need_nurb_intersection = true;
        match other.kind_with_nurb(k)? {
            SurfaceKind::Plane(p) => {
                need_nurb_intersection = self.intersect_with_plane_owned_using_into(
                    k,
                    p,
                    PlaneOwner::NoFace,
                    other_uv_domain,
                    approx_tol_3d,
                    &Self::own_sweep_iso,
                    out,
                )?;
            }
            SurfaceKind::Cone(c) if c.type_code() == crate::surface::surface::p2::CONE_TYPE => {
                need_nurb_intersection = self.intersect_with_cone_into(
                    k,
                    uv_domain,
                    c.as_ref(),
                    other_uv_domain,
                    approx_tol_3d,
                    out,
                )?;
            }
            SurfaceKind::SurfOfRevolution(o) => {
                need_nurb_intersection = intersect_revolved_into(
                    self,
                    k,
                    uv_domain,
                    o.as_ref(),
                    other_uv_domain,
                    approx_tol_3d,
                    out,
                )?;
            }
            SurfaceKind::Sphere(o) => {
                need_nurb_intersection = self.intersect_with_sphere_into(
                    k,
                    uv_domain,
                    o.as_ref(),
                    other_uv_domain,
                    approx_tol_3d,
                    out,
                )?;
            }
            // (an SmCylinder is not SmCone_TYPE, an SmTorus not SmSurfOfRevolution_TYPE:
            // the default branch)
            SurfaceKind::Torus(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::BSpline(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => {}
        }

        // If we're here, then analytic intersections were not found
        if need_nurb_intersection {
            base_global_surface_intersect_into(
                k,
                this,
                uv_domain,
                other,
                other_uv_domain,
                use_surface_edges,
                approx_tol_3d,
                ang_tol_rad,
                context,
                out,
            )?;
        }
        Ok(())
    }

    // ---- recognition ----------------------------------------------------------------

    /// The surface of revolution `test_surface` is, or the one a B-spline surface
    /// makes (`IsNurbSurfaceSurfOfRevolution`): a surface of revolution is copied;
    /// otherwise the B-spline surface must be rational and of degree 2 in at least one
    /// direction whose iso curves in the other direction are coaxial circular arcs on
    /// parallel planes starting and ending on common planes
    /// ([`super::test_for_iso_circles`] at `tolerance_scale * ANALYTIC_TOL_SCALE *
    /// SM_EFF_ZERO * (1 + |corner points|)`, constant-v circles first); the iso curve at
    /// the start of the other direction becomes the generator, which must be planar,
    /// non-degenerate and coplanar with the circles' axis. The new surface has the
    /// NURB domain of `test_surface`. `None` when it is not a surface of revolution.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, the iso-circle tests, the iso curve and the generator
    /// queries.
    pub fn is_nurb_surface_surf_of_revolution(
        k: &Kernel,
        test_surface: &Surface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        // low work - already a SmSurfOfRevolution
        if let SurfaceKind::SurfOfRevolution(s) = test_surface.kind() {
            return Ok(Some(s.copy()));
        }

        // no work - not a BSplineSurface
        if !test_surface.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
            return Ok(None);
        }
        let plane_base;
        let tgt: &BSplineSurface = match test_surface.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => s,
            // (an SmSurfOfExtrusion is an SmBSplineSurface: its NURB is tested)
            SurfaceKind::SurfOfExtrusion(s) => s.base(),
            SurfaceKind::Plane(p) => {
                plane_base = p.base_for_query();
                &plane_base
            }

            SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => return Ok(None),
        };
        Self::is_nurb_surface_surf_of_revolution_base(k, tgt, tolerance_scale)
    }

    /// The recogniser of [`Self::is_nurb_surface_surf_of_revolution`] for the
    /// B-spline surface `tgt`.
    fn is_nurb_surface_surf_of_revolution_base(
        k: &Kernel,
        tgt: &BSplineSurface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        // no work - not a degree 2 rational surface in at least 1 direction
        let deg_u = tgt.degree(SurfParamType::U);
        let deg_v = tgt.degree(SurfParamType::V);
        if !tgt.is_rational() || (deg_u != 2 && deg_v != 2) {
            return Ok(None);
        }

        // evaluate min/max domain points (statuses ignored upstream; the points are read)
        let uv_domain = tgt.natural_uv_domain();
        let min_pnt = tgt.evaluate_point(uv_domain.min())?;
        let max_pnt = tgt.evaluate_point(uv_domain.max())?;

        // scaled zero value
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + min_pnt.max_dimension() + max_pnt.max_dimension());

        // made up AngTolRad value
        let ang_tol_rad = EFF_ZERO_SQRT * tolerance_scale;

        // test sequence of U direction (V Constant) isoCurves for circularity and
        // parallelism
        let mut circles_v = None;
        let mut is_revolution_no_swap_uv = false;
        if deg_u == 2 {
            let c =
                super::test_for_iso_circles(k, tgt, SurfParamType::V, scaled_zero, ang_tol_rad)?;
            is_revolution_no_swap_uv =
                c.found_circles && c.parallel_planes && c.coaxial_centers && c.planar_gen_curve;
            circles_v = Some(c);
        }

        // test sequence of V direction (U Constant) isoCurves
        let mut circles_u = None;
        let mut is_revolution_with_swap_uv = false;
        if !is_revolution_no_swap_uv && deg_v == 2 {
            let c =
                super::test_for_iso_circles(k, tgt, SurfParamType::U, scaled_zero, ang_tol_rad)?;
            is_revolution_with_swap_uv =
                c.found_circles && c.parallel_planes && c.coaxial_centers && c.planar_gen_curve;
            circles_u = Some(c);
        }

        // no work - no sweep direction
        if !is_revolution_with_swap_uv && !is_revolution_no_swap_uv {
            return Ok(None);
        }

        // Now that the surface is a revolution of some generating curve
        // Let GenCurve = iso-curve from the 'other' parameter
        let (gen_dir, swap_uv, param, circles) = if is_revolution_no_swap_uv {
            // constant V isoParamCurves are circles
            (SurfParamType::U, false, uv_domain.min().x, circles_v)
        } else {
            // constant U isoParamCurves are circles
            (SurfParamType::V, true, uv_domain.min().y, circles_u)
        };
        let Some(circles) = circles else {
            return Ok(None);
        };
        let (Some(ref_frame), Some(angles)) = (circles.reference_frame, circles.angles) else {
            return Ok(None);
        };
        let (min_ang_deg, max_ang_deg) = (angles.min(), angles.max());
        let iso_crv = tgt
            .create_iso_parametric_curve(k, gen_dir, param, IsoCurveOptions::default())?
            .curve;

        // no work - GenCurve is degenerate
        if iso_crv.is_degenerate(k, scaled_zero, None)? {
            return Ok(None);
        }

        // Verify GenCurve is planar and coplanar with refFrame ZAxis.
        let iso_curve = Curve::from(iso_crv);
        let linear_tol =
            EFF_ZERO * (1.0 + crate::math::max2(min_pnt.max_dimension(), max_pnt.max_dimension()));
        let planar = iso_curve.is_planar(k, scaled_zero)?;
        let linear = iso_curve.is_linear(k, linear_tol, false)?;
        let is_linear = linear.linear;

        // verify that plane containing genCurve also contains refFrame Z axis
        let origin_point_dist = (ref_frame.origin() - planar.point).dot(planar.normal).abs();
        let frame_plane_ortho = planar.normal.dot(ref_frame.z_axis()).abs();
        let frame_line_co_planar = if is_linear {
            (ref_frame.origin() - planar.point)
                .dot(ref_frame.z_axis().cross(linear.line_tangent))
                .abs()
        } else {
            1.0
        };
        if !is_linear && origin_point_dist >= scaled_zero {
            return Ok(None);
        }
        if (is_linear || frame_plane_ortho >= scaled_zero)
            && (!is_linear || frame_line_co_planar >= scaled_zero)
        {
            return Ok(None);
        }

        // make SurfOfRevolution
        let anal_ivl = gen_step_interval(&iso_curve)?;
        let anal_uv_domain =
            Extent2d::from_bounds(min_ang_deg, anal_ivl.min(), max_ang_deg, anal_ivl.max());
        let mut surf = Self::new(
            k,
            iso_curve,
            ref_frame.origin(),
            ref_frame.x_axis(),
            ref_frame.y_axis(),
            anal_uv_domain,
            swap_uv,
        )?;

        // reparameterize the NURB so that AnalyticSurface->NurbDomain ==
        // InputSurface->NurbDomain (status ignored)
        ignore_status(surf.reparameterize(k, &tgt.natural_uv_domain()))?;
        Ok(Some(surf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::vector3d::Vec3;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};

    /// The generator's radius (along the frame's x axis) at both ends of the offset
    /// surface's generator.
    fn end_radii(k: &Kernel, offset: &SurfOfRevolution) -> [f64; 2] {
        let gen = offset.gen().unwrap();
        let ivl = gen.natural_interval();
        [ivl.min(), ivl.max()].map(|t| gen.evaluate_point(k, t).unwrap().x)
    }

    /// source issue #973: a surface of revolution about +z through the origin
    /// whose generator (in the x-z plane, from the axis at z = -1 out to (4, 0, -5)) lies
    /// below the frame's origin. Offset to one side, the generator crosses the axis below
    /// the origin too: the axis segment, which ran from the origin along +z only, missed
    /// the crossing, and the offset generator was left reaching past the axis (a negative
    /// radius). With the opt-in it is cut at the axis.
    #[test]
    fn issue_973_offset_cut_at_axis_below_origin() {
        let gen = Curve::from(BSplineCurve::from_nurb(
            NurbCurve::new(
                vec![
                    CPoint::new(0.0, 0.0, -1.0, NOW),
                    CPoint::new(4.0, 0.0, -5.0, NOW),
                ],
                1,
                vec![0.0, 0.0, 1.0, 1.0],
            )
            .unwrap(),
            3,
            false,
        ));
        let k = Kernel::default();
        let mut surface = SurfOfRevolution::new(
            &k,
            gen,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent2d::from_bounds(0.0, 0.0, 360.0, 1.0),
            false,
        )
        .unwrap();
        surface.make_nurb(&k).unwrap();
        let min_radius = |both_sides: bool| -> f64 {
            let mut k = Kernel::default();
            k.fixes.revolution_offset_axis_both_sides = both_sides;
            [1.0, -1.0]
                .into_iter()
                .flat_map(|d| end_radii(&k, &surface.create_offset_surface(&k, d, 1e-6).unwrap()))
                .fold(f64::INFINITY, f64::min)
        };
        // upstream: one of the two offsets keeps a stretch past the axis
        assert!(min_radius(false) < -0.1);
        // the opt-in cuts it at the axis
        assert!(min_radius(true) > -1e-6);
    }
}
