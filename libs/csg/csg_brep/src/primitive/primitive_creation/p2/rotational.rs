//! Partial cones, spheres and tori as rotational sweeps of their profiles.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_partial_cone`] | six-argument `CreateCone` |
//! | [`PrimitiveCreation::create_partial_sphere`] | four-argument `CreateSphere` |
//! | [`PrimitiveCreation::create_partial_torus`] | five-argument `CreateTorus` |
//!
//! Deviations: the active `USE_NEW_ANALYTICS` branch sends a full turn to p1's
//! analytic constructors. Edit mode is enabled before the profile is built and,
//! as upstream, stays enabled when a later step fails. No recursion.

use super::PrimitiveCreationAccess;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::math::{EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::p1::{BSplineCurve, NurbCircleParam};
use crate::primitive::primitive_creation::{p1::PrimitiveCreation, p3::RepeatedSweepOptions};
use crate::topology::BrepId;

const OP: &str = "primitive::primitive_creation::p2::rotational";

/// Upstream's full-turn test: `|angle - 360| < 360 * SM_EFF_ZERO`.
fn full_rotation(rotation_deg: f64) -> bool {
    (rotation_deg - 360.0).abs() < 360.0 * EFF_ZERO
}

/// One revolution of the profile, capped unless it is a full turn
/// (`CreateRotationalSweep(curves, origin, axis, angle, 1, cap)`, testing continuity).
fn sweep_options(full: bool) -> RepeatedSweepOptions {
    RepeatedSweepOptions {
        repetitions: 1,
        cap_ends: !full,
        test_continuity: true,
    }
}

impl PrimitiveCreation {
    /// A cone or cylinder of `height` from `base_radius` to `top_radius`, turned from
    /// `start_deg` to `end_deg` about the frame's Z axis (six-argument `CreateCone`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a height below `SM_EFF_ZERO`, a turn of almost zero degrees, a
    /// zero-length side or axis, or fewer than three profile lines; those of the
    /// line construction and of the analytic or rotational sweep.
    pub fn create_partial_cone(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        height: f64,
        base_radius: f64,
        top_radius: f64,
        start_deg: f64,
        end_deg: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        // empty entity maps
        self.init_entity_maps();

        if height < EFF_ZERO {
            return Err(SolidError::sm_err(OP));
        }

        // Check for special cases: a full 360 degree sweep is analytic, a zero
        // sweep fails
        let rotation = end_deg - start_deg;
        if rotation.abs() < EFF_ZERO {
            return Err(SolidError::sm_err(OP));
        }
        let full = full_rotation(rotation);
        if full {
            return a.with_primitive_brep(brep, |k, b| {
                self.create_cone(k, b, height, base_radius, top_radius, frame)
            });
        }

        // Construct partial cone
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }
        a.set_editing(brep, true)?;

        let origin = frame.origin();
        // The axis of the cone is the Z axis of the Frame
        let axis = frame.z_axis();
        let start_rad = start_deg * PI / 180.0;
        let mut ref_frame = *frame;
        ref_frame.rotate_about_axis(start_rad, axis);
        let x_axis = ref_frame.x_axis();

        let top_base = origin + height * axis;
        let start = origin + base_radius * x_axis;
        let end = top_base + top_radius * x_axis;

        let mut curves = Vec::new();
        let tol = EFF_ZERO * (1.0 + end.max_dimension() + start.max_dimension());

        if end.distance(start) > tol {
            curves.push(Some(Curve::from(BSplineCurve::create_line_segment(
                3, start, end,
            )?)));
        } else {
            // Zero length side of a cone is not legal
            return Err(SolidError::sm_err(OP));
        }
        if origin.distance(start) > tol {
            curves.push(Some(Curve::from(BSplineCurve::create_line_segment(
                3, origin, start,
            )?)));
        }
        if origin.distance(top_base) > tol {
            curves.push(Some(Curve::from(BSplineCurve::create_line_segment(
                3, origin, top_base,
            )?)));
        } else {
            // Zero length side of a cone is not legal
            return Err(SolidError::sm_err(OP));
        }
        if end.distance(top_base) > tol {
            curves.push(Some(Curve::from(BSplineCurve::create_line_segment(
                3, end, top_base,
            )?)));
        }
        if curves.len() < 3 {
            // Some problem here with not enough curves
            return Err(SolidError::sm_err(OP));
        }

        self.create_partial_rotational_sweep(
            a,
            brep,
            &mut curves,
            origin,
            axis,
            rotation,
            sweep_options(full),
        )?;
        a.set_editing(brep, false)?;
        Ok(())
    }

    /// A sphere of `radius` turned from `start_deg` to `end_deg` about the frame's Z
    /// axis (four-argument `CreateSphere`): a diameter line and a half circle swept.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the circle frame cannot be built; those of the line and
    /// circle construction and of the analytic or rotational sweep.
    pub fn create_partial_sphere(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        radius: f64,
        start_deg: f64,
        end_deg: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        // empty entity maps
        self.init_entity_maps();

        let rotation = end_deg - start_deg;
        let full = full_rotation(rotation);
        if full {
            return a.with_primitive_brep(brep, |k, b| self.create_sphere(k, b, radius, frame));
        }

        // Construct partial sphere
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }
        a.set_editing(brep, true)?;

        // convert from degrees to radians
        let start_rad = start_deg * PI / 180.0;
        let origin = frame.origin();
        let axis = frame.z_axis();
        let mut ref_frame1 = *frame;
        ref_frame1.rotate_about_axis(start_rad, axis);
        let y_rf1 = ref_frame1.y_axis();

        let start = origin + radius * axis;
        let end = origin - radius * axis;
        let line = BSplineCurve::create_line_segment(3, start, end)?;

        // We need a frame for the circle: its X is -Axis, its Z is the Y of RefFrame1
        let mut ref_frame3 = Axis2Placement::IDENTITY;
        if ref_frame3
            .set_canonical(origin, -axis, y_rf1.cross(axis))
            .is_err()
        {
            return Err(SolidError::sm_err(OP));
        }

        // The circle generator can cope all angles.
        let circle = BSplineCurve::create_circle_segment(
            3,
            &ref_frame3,
            radius,
            0.0,
            180.0,
            NurbCircleParam::Quadratic,
        )?;

        let mut curves = vec![Some(Curve::from(line)), Some(Curve::from(circle))];
        self.create_partial_rotational_sweep(
            a,
            brep,
            &mut curves,
            origin,
            axis,
            rotation,
            sweep_options(full),
        )?;
        a.set_editing(brep, false)?;
        Ok(())
    }

    /// A torus of `major` and `minor` radii turned from `start_deg` to `end_deg`
    /// about the frame's Z axis (five-argument `CreateTorus`): one circle swept.
    /// A horn torus (equal radii) starts its circle at -180 degrees, so that the
    /// singularity lies on the seam.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the circle frame cannot be built; those of the circle
    /// construction and of the analytic or rotational sweep.
    pub fn create_partial_torus(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        major: f64,
        minor: f64,
        start_deg: f64,
        end_deg: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        // empty entity maps
        self.init_entity_maps();

        let rotation = end_deg - start_deg;
        let full = full_rotation(rotation);
        if full {
            return a
                .with_primitive_brep(brep, |k, b| self.create_torus(k, b, major, minor, frame));
        }

        // Construct partial torus
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }
        a.set_editing(brep, true)?;

        let start_rad = start_deg * PI / 180.0;
        let axis = frame.z_axis();
        let origin = frame.origin();
        let mut ref_frame1 = *frame;
        ref_frame1.rotate_about_axis(start_rad, axis);
        let start_axis = ref_frame1.x_axis();
        let y_rf1 = ref_frame1.y_axis();

        let center = origin + major * start_axis;

        // We need a frame for the full circle: its x is the -axis, its Z is YRF1
        let mut ref_frame3 = Axis2Placement::IDENTITY;
        if ref_frame3
            .set_canonical(center, -axis, (-y_rf1).cross(axis))
            .is_err()
        {
            return Err(SolidError::sm_err(OP));
        }

        // if minor radius = major radius, adjust start and end angle -180 to 180
        let (circle_start, circle_end) = if minor == major {
            (-180.0, 180.0)
        } else {
            (0.0, 360.0)
        };
        let circle = BSplineCurve::create_circle_segment(
            3,
            &ref_frame3,
            minor,
            circle_start,
            circle_end,
            NurbCircleParam::Quadratic,
        )?;

        let mut curves = vec![Some(Curve::from(circle))];
        self.create_partial_rotational_sweep(
            a,
            brep,
            &mut curves,
            origin,
            axis,
            rotation,
            sweep_options(full),
        )?;
        a.set_editing(brep, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_turn_is_within_360_times_the_effective_zero() {
        assert!(full_rotation(360.0));
        assert!(full_rotation(360.0 + 3.5e-10));
        assert!(full_rotation(360.0 - 3.5e-10));
        assert!(!full_rotation(360.0 + 3.7e-10));
        assert!(!full_rotation(359.9));
        assert!(!full_rotation(-360.0));
    }

    #[test]
    fn one_revolution_is_capped_unless_it_is_a_full_turn() {
        let partial = sweep_options(false);
        assert_eq!(
            (
                partial.repetitions,
                partial.cap_ends,
                partial.test_continuity
            ),
            (1, true, true)
        );
        assert!(!sweep_options(true).cap_ends);
    }
}
