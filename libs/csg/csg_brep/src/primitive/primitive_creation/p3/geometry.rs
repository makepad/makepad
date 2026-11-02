//! Geometry preparation for primitive curve sweeps.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp` (`CreateCurveSweep`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`prepare_curve_sweep`] | geometry preparation inside `CreateCurveSweep` |
//!
//! Deviations: owned copies retain the original inputs; matrices and partial
//! outputs are explicit. Scaling sampling is bounded to 1,000,000 points; no
//! recursion. Null/non-B-spline profiles are skipped, as the upstream cast does.
//! **A planar path's frame starts on the plane's normal** (opt-in
//! `Kernel::fixes.sweep_frame_on_planar_path_normal`, WR-407; source change,
//! source fork 1b6f5fa, source issue #434; see
//! [`prepare_curve_sweep`]).
//! **The profile can be swept where it lies** (`SweepOptions::profile_at_path_start`,
//! the source fork's `bProfileAtPathStart`, source fork 2dd71a0, WR-522,
//! #1450; off by default): the path's start is the profile's reference point.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p3::compute_crvs_normal;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::ZONE_TOL_3D;
use crate::kernel::Kernel;
use crate::math::{Vec3, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::primitive::primitive_creation::SweepOptions;

const OP: &str = "primitive::primitive_creation::p3::prepare_curve_sweep";

pub(super) fn bspline(curve: &Curve) -> Option<BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(c) => Some(c.clone()),
        CurveKind::Line(c) => Some(c.base().clone()),
        CurveKind::Ellipse(c) => Some(c.base().clone()),
        CurveKind::Parabola(c) => Some(c.base().clone()),
        CurveKind::Hyperbola(c) => Some(c.base().clone()),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GeometryHandle(pub Extent1d);
impl SolutionObject for GeometryHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// The copied curves and inverse placements consumed by sweep construction.
#[derive(Debug)]
pub struct PreparedCurveSweep {
    /// Profile copies, subdivided when requested, in upstream order.
    pub profiles: Vec<BSplineCurve>,
    /// Path translated to the origin and rotated to start along X.
    pub path: BSplineCurve,
    /// Optional sampled/interpolated scaling law.
    pub scale: Option<BSplineCurve>,
    /// Undo the path's initial tangent rotation.
    pub rotation_back: Axis2Placement,
    /// Return to the initial path start.
    pub path_back: Axis2Placement,
    /// Return to the initial profile reference point.
    pub profile_back: Axis2Placement,
    /// Move the profile's reference point to the origin (`sProfileToOrigin`).
    pub profile_to_origin: Axis2Placement,
    /// Turn the path's start tangent onto x (`sPathRot`).
    pub rotation: Axis2Placement,
    /// Orient the profile square to the path (`sProfileTrans`), only with
    /// `orient_profile_perpendicular_to_path`.
    pub orientation: Option<Axis2Placement>,
}

pub(super) fn split_profiles(k: &Kernel, curves: &mut Vec<BSplineCurve>) -> Result<()> {
    let mut index = 0;
    while let Some(curve) = curves.get(index) {
        // SubdivideAtDiscontinuity resets its output before any fallible call.
        let pieces = ignored(curve.subdivide_at_discontinuity(k, ContinuityType::C1, 0.5))?
            .unwrap_or_default();
        if pieces.len() > 1 {
            let count = pieces.len();
            curves.splice(index..=index, pieces.iter().cloned());
            index += count;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn scaling(
    k: &Kernel,
    path: &mut BSplineCurve,
    reference: &Curve,
    scale: &BSplineCurve,
) -> Result<BSplineCurve> {
    let mut scale = scale.clone();
    let domain = path.natural_interval();
    scale.edit_parameterization(&domain)?;
    let count = path
        .number_of_unique_knots()?
        .max(scale.number_of_unique_knots()?)
        .checked_mul(10)
        .ok_or_else(|| SolidError::invalid_input(OP, "scaling sample overflow"))?
        .max(30);
    if count > 1_000_000 {
        return Err(SolidError::invalid_input(
            OP,
            "scaling sampling exceeds 1000000",
        ));
    }
    // Unlike path/scale, upstream never copy-constructs pScaleReference.
    // Keep its virtual natural interval, point solve and point evaluation.
    let ri = reference.natural_interval();
    let options = GlobalPointSolveOptions {
        handle: Some(GeometryHandle(ri)),
        analytic_handle: Some(GeometryHandle(ri)),
        ..Default::default()
    };
    let distance = |point: Vec3| -> Result<f64> {
        let solutions = reference.global_point_solve(
            k,
            &ri,
            SolverOperation::Minimize,
            point,
            EFF_ZERO_SQRT,
            SolutionRequested::Single,
            &options,
        )?;
        let solution = solutions
            .get(0)
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let projected = reference.evaluate_point(k, solution.start.param(0)?)?;
        Ok(point.distance(projected))
    };
    let factor = 1.0 / distance(scale.evaluate_point(domain.min())?)?;
    let mut points = Vec::new();
    let mut params = Vec::new();
    for index in 0..=count {
        let t = domain.evaluate(index as f64 * 1.0 / count as f64);
        params.push(t);
        let value = distance(scale.evaluate_point(t)?)? * factor;
        points.push(Vec3::new(value, value, value));
    }
    let mut scaled = BSplineCurve::interpolate_points(
        &points,
        Some(&params),
        3,
        None,
        None,
        false,
        ParamMethod::ChordLength,
    )?;
    scaled.edit_parameterization(&domain)?;
    let mut pair = [path.clone(), scaled];
    let result = BSplineCurve::make_curves_compatible(&mut pair, 1.0e-8);
    let [next_path, scaled] = pair;
    *path = next_path;
    result?;
    Ok(scaled)
}

/// Copy, optionally scale, translate and orient a curve sweep's geometry.
/// This is the complete preparation block of `CreateCurveSweep`, before building
/// composites and faces. The caller retains its Kernel and approximation policy.
/// The scale reference keeps its analytic dispatch; path and scale are NURB copies,
/// matching the upstream copy constructors.
pub fn prepare_curve_sweep(
    k: &Kernel,
    profiles: &[Option<Curve>],
    path: &BSplineCurve,
    scale_reference: Option<&Curve>,
    scale_curve: Option<&BSplineCurve>,
    options: SweepOptions,
    test_continuity: bool,
) -> Result<PreparedCurveSweep> {
    if profiles.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    let mut path = path.clone();
    let mut profiles: Vec<_> = profiles.iter().flatten().filter_map(bspline).collect();
    if test_continuity {
        split_profiles(k, &mut profiles)?;
    }
    let scale = match (scale_reference, scale_curve) {
        (Some(reference), Some(scale)) => Some(scaling(k, &mut path, reference, scale)?),
        _ => None,
    };
    let values = path.evaluate(path.natural_interval().min(), 1, true, false)?;
    let start = values
        .first()
        .copied()
        .ok_or_else(|| SolidError::sm_err(OP))?;
    let mut path_to_origin = Axis2Placement::IDENTITY;
    path_to_origin.translate(-start);
    ignore_status(path.transform(&path_to_origin, None))?;
    let mut reference = Vec3::ZERO;
    if options.profile_at_path_start {
        // source fork (source fork 2dd71a0, WR-522, #1450): the profile moves
        // with the path's start, so it keeps its place relative to the path.
        reference = start;
    } else if options.move_profile_center_to_path {
        let mut total = Extent3d::new();
        let mut bounds = Extent3d::new();
        for profile in &profiles {
            let curve = Curve::from(profile.clone());
            if let Some(next) = ignored(curve.calculate_bounding_box(
                k,
                &curve.natural_interval(),
                crate::nurbs::b_spline_curve::p2::BoundingBoxRequest {
                    normal: true,
                    ..Default::default()
                },
                true,
            ))? {
                if let Some(next) = next.normal {
                    bounds = next;
                }
            }
            total.add_point(bounds.min());
            total.add_point(bounds.max());
        }
        reference = total.mid();
    } else {
        let first = profiles
            .first()
            .ok_or_else(|| SolidError::invalid_input(OP, "no B-spline profiles"))?;
        if let Some(point) = ignored(first.evaluate_point(first.natural_interval().min()))? {
            reference = point;
        }
    }
    let mut profile_to_origin = Axis2Placement::IDENTITY;
    profile_to_origin.translate(-reference);
    for profile in &mut profiles {
        ignore_status(profile.transform(&profile_to_origin, None))?;
    }
    let values = path.evaluate(path.natural_interval().min(), 1, true, false)?;
    let tangent = values
        .get(1)
        .copied()
        .ok_or_else(|| SolidError::sm_err(OP))?;
    let mut from_x = tangent.unitize_or_z();
    let mut from_y = Vec3::UNDEF;
    if let Some([x, y, _]) = ignored(tangent.make_unit_ortho_vectors(None))? {
        from_x = x;
        from_y = y;
    }
    // source change, source fork 1b6f5fa (source issue #434): the path
    // is turned so that this frame becomes the global one, and the sweep (`N_CreateSweepScale`,
    // `Z0` = global z) carries the profile on a frame that starts there and is computed from
    // point to point along the path. Along a planar path the plane's normal is the one
    // direction such a frame keeps; `MakeUnitOrthoVectors` of a start tangent along +y puts z
    // in the path's plane instead, and a wall's flat top swept along a half circle then comes
    // out with its control points spread about its plane (2.8e-5 mm in the #434 case against a
    // 1e-5 zone tolerance), which a boolean with another wall on that plane cannot resolve. So
    // a planar path starts the frame with its z on the plane's normal; the top is then flat to
    // the rounding. Opt-in (`Kernel::fixes.sweep_frame_on_planar_path_normal`, WR-407);
    // upstream keeps the `MakeUnitOrthoVectors` frame.
    if k.fixes.sweep_frame_on_planar_path_normal {
        let plane = path.is_planar(ZONE_TOL_3D)?;
        // `SmCurve::IsPlanar(SM_ZONE_TOL_3D, NULL, &sPlaneNormal)` with
        // `sPlaneNormal.Unitize()` in place before the test.
        if let Some(plane_normal) = plane.is_planar.then_some(plane.normal) {
            if let Ok(normal) = plane_normal.unitize() {
                if normal.dot(from_x).abs() < EFF_ZERO_SQRT {
                    let from_z = normal - normal.dot(from_x) * from_x;
                    if from_z.unitize().is_ok() {
                        from_y = from_z.cross(from_x);
                    } else if let Some([x, y, _]) = ignored(tangent.make_unit_ortho_vectors(None))?
                    {
                        from_x = x;
                        from_y = y;
                    }
                }
            }
        }
    }
    let rotation = Axis2Placement::from_to(
        &Axis2Placement::new(Vec3::ZERO, from_x, from_y),
        &Axis2Placement::IDENTITY,
    );
    ignore_status(path.transform(&rotation, None))?;
    for profile in &mut profiles {
        ignore_status(profile.transform(&rotation, None))?;
    }
    let mut orientation = None;
    if options.orient_profile_perpendicular_to_path {
        let as_curves: Vec<_> = profiles.iter().cloned().map(Curve::from).collect();
        let mut normal = match compute_crvs_normal(k, &as_curves) {
            Ok(normal) => normal,
            Err(SolidError::Unported { upstream, unit }) => {
                return Err(SolidError::Unported { upstream, unit })
            }
            Err(SolidError::Upstream { code: 1001, .. }) => {
                let first = profiles.first().ok_or_else(|| SolidError::sm_err(OP))?;
                let values = ignored(first.evaluate(
                    first.natural_interval().evaluate(0.0),
                    1,
                    true,
                    false,
                ))?;
                Vec3::X.cross(
                    values
                        .as_ref()
                        .and_then(|v| v.get(1))
                        .copied()
                        .unwrap_or(Vec3::UNDEF),
                )
            }
            Err(error) => return Err(error),
        };
        ignore_status(compute_crvs_normal(k, &[Curve::from(path.clone())]))?;
        if normal.dot(Vec3::X) < 0.0 {
            normal = -normal;
        }
        let angle = ignored(Vec3::X.angle_between(normal))?.unwrap_or(0.0);
        let mut placement = Axis2Placement::IDENTITY;
        placement.rotate_about_axis(-angle, Vec3::X.cross(normal));
        for profile in &mut profiles {
            ignore_status(profile.transform(&placement, None))?;
        }
        orientation = Some(placement);
    }
    Ok(PreparedCurveSweep {
        profiles,
        path,
        scale,
        rotation_back: rotation.invert(),
        path_back: path_to_origin.invert(),
        profile_back: profile_to_origin.invert(),
        profile_to_origin,
        rotation,
        orientation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_keeps_analytic_scale_reference() -> Result<()> {
        use crate::curve::line::Line;

        let k = Kernel::default();
        // Imported analytic data and its cached NURB can differ in the last bits.
        // The scale curve moves from distance 1 to distance 2 from x = 2.
        let point = Vec3::new(2.0, 0.0, 0.0);
        let cached_point = Vec3::new(f64::from_bits(point.x.to_bits() + 2), 0.0, 0.0);
        let reference = Curve::from(Line::from_parts(
            BSplineCurve::create_line_segment(3, cached_point, cached_point + Vec3::Z)?,
            point,
            Vec3::Z,
            Extent1d::from_min_max(0.0, 1.0),
            1.0,
            false,
        ));
        let path = BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::Z)?;
        let scale = BSplineCurve::create_line_segment(
            3,
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(4.0, 0.0, 1.0),
        )?;
        let profiles = [Some(Curve::from(path.clone()))];
        let prepare = |reference: &Curve| -> Result<BSplineCurve> {
            Ok(prepare_curve_sweep(
                &k,
                &profiles,
                &path,
                Some(reference),
                Some(&scale),
                SweepOptions::default(),
                false,
            )?
            .scale
            .expect("both scaling inputs produce a scale law"))
        };
        let analytic = prepare(&reference)?;
        assert_eq!(
            analytic.evaluate_point(analytic.natural_interval().max())?,
            Vec3::new(2.0, 2.0, 2.0)
        );
        // Slicing the caller's reference must fail the exact endpoint assertion.
        let sliced = prepare(&Curve::from(bspline(&reference).unwrap()))?;
        assert_ne!(
            sliced.evaluate_point(sliced.natural_interval().max())?,
            Vec3::new(2.0, 2.0, 2.0)
        );
        Ok(())
    }

    /// The #434 half circle in the XY plane, start tangent +Y, with a 60-high wall profile
    /// at its start: the largest `|z|` of the prepared path's samples and the prepared
    /// profile's height vector.
    fn issue_434_half_circle_frame(k: &Kernel) -> Result<(f64, Vec3)> {
        use crate::geom::axis2_placement::Axis2Placement;
        use crate::nurbs::b_spline_curve::p1::NurbCircleParam;

        let path = BSplineCurve::create_circle_segment(
            3,
            &Axis2Placement::new(Vec3::ZERO, Vec3::X, Vec3::Y),
            50.0,
            0.0,
            180.0,
            NurbCircleParam::Quadratic,
        )?;
        // The wall's height: 60 along world z in the plane through the path start that is
        // perpendicular to the tangent.
        let vertical = Curve::from(BSplineCurve::create_line_segment(
            3,
            Vec3::new(44.0, 0.0, 0.0),
            Vec3::new(44.0, 0.0, 60.0),
        )?);
        let prepared = prepare_curve_sweep(
            k,
            &[Some(vertical)],
            &path,
            None,
            None,
            SweepOptions::default(),
            false,
        )?;
        let domain = prepared.path.natural_interval();
        let mut max_z = 0.0_f64;
        for step in 0..=8 {
            let t = domain.evaluate(f64::from(step) / 8.0);
            max_z = max_z.max(prepared.path.evaluate_point(t)?.z.abs());
        }
        let profile = prepared
            .profiles
            .first()
            .ok_or_else(|| SolidError::sm_err("no prepared profile"))?;
        let profile_domain = profile.natural_interval();
        let start = profile.evaluate_point(profile_domain.min())?;
        let end = profile.evaluate_point(profile_domain.max())?;
        Ok((max_z, end - start))
    }

    /// With `fixes.sweep_frame_on_planar_path_normal` (WR-407, #434) the frame's z is the
    /// path plane's normal: the path is turned into the global XY plane and the wall's
    /// height is the frame's fixed z.
    #[test]
    fn issue_434_planar_path_frame_keeps_the_plane_normal_as_the_frame_z() -> Result<()> {
        let mut k = Kernel::default();
        k.fixes.sweep_frame_on_planar_path_normal = true;
        let (max_z, height) = issue_434_half_circle_frame(&k)?;
        assert!(max_z < 1e-12, "path leaves the frame's plane by {max_z}");
        // (the frame's handedness may turn the height down; the sweep frame it is carried
        // on flips with it, so the solid is the same)
        assert!((height.length() - 60.0).abs() < 1e-12, "{height:?}");
        assert!(
            height.x.abs() < 1e-12 && height.y.abs() < 1e-12,
            "{height:?}"
        );
        Ok(())
    }

    /// Default (pinned SMLib `c9c5979`): `MakeUnitOrthoVectors` of the +Y start tangent
    /// puts the frame's z in the path's plane, so the path is not turned into the global
    /// XY plane (upstream behaviour, WR-407).
    #[test]
    fn issue_434_default_keeps_the_upstream_sweep_frame() -> Result<()> {
        let k = Kernel::default();
        assert!(!k.fixes.sweep_frame_on_planar_path_normal);
        let (max_z, _) = issue_434_half_circle_frame(&k)?;
        assert!(
            max_z > 1.0,
            "upstream's frame keeps the path off z = 0: {max_z}"
        );
        Ok(())
    }

    #[test]
    fn preparation_preserves_inputs_and_recovers_the_original_path() -> Result<()> {
        let k = Kernel::default();
        let start = Vec3::new(3.0, 4.0, 5.0);
        let path = BSplineCurve::create_line_segment(3, start, start + 2.0 * Vec3::Z)?;
        let profile = Curve::from(BSplineCurve::create_line_segment(
            3,
            Vec3::Y,
            Vec3::Y + Vec3::X,
        )?);
        let prepared = prepare_curve_sweep(
            &k,
            &[Some(profile.clone())],
            &path,
            None,
            None,
            SweepOptions::default(),
            false,
        )?;
        let domain = prepared.path.natural_interval();
        let placed_start = prepared.path.evaluate_point(domain.min())?;
        let placed_end = prepared.path.evaluate_point(domain.max())?;
        assert!(placed_start.length() < 1e-12);
        assert!((placed_end - 2.0 * Vec3::X).length() < 1e-12);
        let original_start = prepared
            .path_back
            .transform_point(prepared.rotation_back.transform_point(placed_start));
        assert!((original_start - start).length() < 1e-12);
        assert_eq!(path.evaluate_point(path.natural_interval().min())?, start);
        assert_eq!(
            profile.evaluate_point(&k, profile.natural_interval().min())?,
            Vec3::Y
        );
        Ok(())
    }
}
