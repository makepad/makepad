//! The analytic surface a revolved profile curve sweeps: a cylinder, cone, sphere or
//! torus bounded to the curve and the turn (source change).
//!
//! Upstream: none. source fix source fork 0b143f7 (branch
//! `port-revolve-analytic`, source issue #545). At `c9c5979`
//! `SmPrimitiveCreation::CreateRotationalSweep` makes analytic faces only for a
//! one-curve profile (`sm_CreateAnalyticRotationalSweep`). Each curve of a longer
//! profile gets an `SmSurfOfRevolution` (a whole turn) or the B-spline surface of
//! revolution of `SmRotationalSweepGeometry::EdgeSweepHigher` (a partial turn), whose
//! `SmBSplineSurface::CopyAndAddAnalytics` recognises no cylinder, cone, sphere or torus
//! the face's trims can stay on.
//!
//! | Rust                    | Upstream      |
//! |-------------------------|---------------|
//! | [`analytic_revolution`] | (source)  |
//!
//! Deviations: not in upstream. The surface is laid out as `sm_CreateRotSrf`'s B-spline
//! surface of revolution: `u` the turn (`0` at the curve, rising with the rotation about
//! the axis), `v` the curve from its start to its end; for a line falling along the axis,
//! the reversed line's (`v` from its end), its cylinder or cone built from the low end
//! and never inside out, as 9853e54's `AnalyticSurface` (#1463); likewise an arc
//! falling round its circle, its sphere or torus never inside out (#1490). It is only returned when its
//! corners, the curve's start tangent and the turn's sense agree with that layout, so a
//! caller can swap it in for the B-spline and keep the trims and loop the B-spline
//! layout gives. Any other curve, or a construction that fails, gives `None` and the
//! caller keeps the upstream surface. No recursion.

use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, Result};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{line_closest_point, line_point_distance};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::surface::cone::Cone;
use crate::surface::sphere::{Sphere, SphereOptions};
use crate::surface::surface::Surface;
use crate::surface::torus::{Torus, TorusOptions};
use crate::sweep::rotational_sweep_geometry::create_rot_srf;

/// Curve samples tested for lying in the half-plane through the axis.
const PLANE_SAMPLES: u32 = 8;

/// The largest distance, relative to the size of the geometry, by which the analytic
/// surface may miss the B-spline one. The surface replaces the B-spline, so the curve
/// must be a line or arc to rounding, not only within the caller's (B-rep) tolerance:
/// a short wavy curve lies within a B-rep tolerance of 1e-6 of some arc.
const FIT_REL: f64 = 1.0e-9;

/// The analytic surface swept by revolving `curve` (over its natural interval) by
/// `angle_deg` degrees about the axis through `base_pt` along `axis`: a cylinder for a
/// line parallel to the axis, a cone for another line that is not perpendicular to
/// it, a sphere for a circular arc centred on the axis and a torus for one off it, the
/// arc in a plane through the axis. Bounded to the curve and the turn, and laid out as
/// the B-spline surface of revolution `reference` (`sm_CreateRotSrf`'s, made here when
/// `None`; see the module header): the result must reproduce its corners, its tangent
/// along the curve at the start and its sense of turn, and its natural boundary must
/// have the B-spline's: the same degenerate sides and the other sides' lengths.
/// `tol` is the distance tolerance of those tests and of the shape recognition, capped
/// at `FIT_REL` times the size of the curve and axis point (`1 +` their largest
/// coordinate), so that only a curve that is a line or arc to rounding qualifies.
///
/// `None` for any other curve (a line perpendicular to the axis sweeps a plane), a
/// curve not in one half-plane through the axis, or a surface that does not pass.
///
/// # Errors
///
/// [`crate::SolidError::Unported`] from the curve queries or surface construction;
/// other failures give `None`.
pub fn analytic_revolution(
    k: &Kernel,
    curve: &Curve,
    base_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
    tol: f64,
    reference: Option<&Surface>,
) -> Result<Option<Surface>> {
    let Some(axis) = ignored(axis.unitize())? else {
        return Ok(None);
    };
    // (written so that a NaN turn or tolerance is refused)
    let usable = angle_deg > 0.0 && angle_deg <= 360.0 + EFF_ZERO && tol > 0.0;
    if !usable {
        return Ok(None);
    }
    let Some(ends) = ignored(curve.ends(k))? else {
        return Ok(None);
    };
    let size = ends
        .start
        .max_dimension()
        .max(ends.end.max_dimension())
        .max(base_pt.max_dimension());
    // WR-471: the caller's own tolerance, which decides if an arc is centred on the axis
    let axis_tol = if k.fixes.revolve_arc_centre_near_axis_is_sphere {
        tol
    } else {
        0.0
    };
    // WR-552: the caller's own tolerance, within which a circle reaching the axis is
    // the horn torus's
    let horn_tol = if k.fixes.revolve_arc_touching_axis_is_horn_torus {
        tol
    } else {
        0.0
    };
    let tol = tol.min(FIT_REL * (1.0 + size));
    let Some(Some((candidate, reversed))) = ignored(candidate(
        k, curve, base_pt, axis, angle_deg, tol, axis_tol, horn_tol,
    ))?
    else {
        return Ok(None);
    };
    let made;
    let reference = match reference {
        Some(surface) if !reversed => surface,
        _ => {
            // a curve falling along the axis or round its circle: laid out as the
            // reversed curve's B-spline
            let made_srf = if reversed {
                let Some(back) = ignored(reversed_curve(k, curve, ends.start, ends.end))? else {
                    return Ok(None);
                };
                create_rot_srf(k, &back, base_pt, axis, angle_deg)
            } else {
                create_rot_srf(k, curve, base_pt, axis, angle_deg)
            };
            let Some(surface) = ignored(made_srf)? else {
                return Ok(None);
            };
            made = Surface::from(surface);
            &made
        }
    };
    let expected = ignored(Layout::of(k, reference, base_pt, axis))?;
    let actual = ignored(Layout::of(k, &candidate, base_pt, axis))?;
    let same_layout = matches!((expected, actual),
        (Some(expected), Some(actual)) if actual.matches(&expected, tol));
    if !same_layout {
        return Ok(None);
    }
    // the face is trimmed by the natural boundary: a side the analytic surface makes
    // wrong (before #899, a torus's full minor circle off angle 0, whose iso circle
    // collapsed) keeps the B-spline
    let expected = ignored(boundary(k, reference, tol))?;
    let actual = ignored(boundary(k, &candidate, tol))?;
    Ok(match (expected, actual) {
        (Some(expected), Some(actual)) if same_boundary(&expected, &actual) => Some(candidate),
        _ => None,
    })
}

/// Relative difference allowed between the lengths of matching boundary sides: the
/// lengths are polyline estimates, which depend on the parameterisation.
const LENGTH_REL: f64 = 1.0e-3;

/// Samples of each boundary side's length estimate.
const LENGTH_SAMPLES: usize = 16;

/// The natural boundary sides of `surface` (from the umin side, as `EdgeSweepHigher`
/// asks for them): each one's estimated length and whether it is degenerate within
/// `tol`.
fn boundary(k: &Kernel, surface: &Surface, tol: f64) -> Result<Vec<(f64, bool)>> {
    let sides = surface.create_natural_uv_trim_curves(
        k,
        &surface.natural_uv_domain_with_nurb(k)?,
        SurfParamType::U,
        false,
    )?;
    sides
        .curves_3d
        .iter()
        .map(|c| {
            let ivl = c.natural_interval();
            Ok((
                c.approximate_length(k, &ivl, LENGTH_SAMPLES, false)?.0,
                c.is_degenerate(k, tol, None)?,
            ))
        })
        .collect()
}

/// Whether two boundaries have the same sides: the same count, the same degenerate
/// ones and the others' lengths within [`LENGTH_REL`].
fn same_boundary(expected: &[(f64, bool)], actual: &[(f64, bool)]) -> bool {
    expected.len() == actual.len()
        && expected
            .iter()
            .zip(actual)
            .all(|(&(le, de), &(la, da))| de == da && (de || (la - le).abs() <= LENGTH_REL * le))
}

/// The unit direction from the axis to `point`, `None` within `tol` of the axis.
fn radial(base_pt: Point3, axis: Vec3, point: Point3, tol: f64) -> Option<Vec3> {
    let foot = base_pt + axis * line_closest_point(base_pt, axis, point);
    let offset = point - foot;
    let length = offset.length();
    (length > tol).then(|| offset / length)
}

/// An angle in degrees taken into `[0, 360)`; within `360 * SM_EFF_ZERO` below a
/// whole turn it is `0`.
fn turn_into_range(angle_deg: f64) -> f64 {
    let a = angle_deg % 360.0;
    let a = if a < 0.0 { a + 360.0 } else { a };
    if 360.0 - a <= 360.0 * EFF_ZERO {
        0.0
    } else {
        a
    }
}

/// The tube angle range of an arc starting at `start_deg` and sweeping `sweep_deg`,
/// inside `[-360, 360]`: `SmEllipse::AdjustSTEPInterval` (through the torus's iso
/// circles) documents `-360 <= Min <= Max <= 360` and collapses an arc past it to one
/// parameter (`WR-211`). It starts in `[0, 360)` when the arc ends by `360`, else a
/// turn lower (the horn torus of `CreateTorus` also starts below 0).
fn tube_range(start_deg: f64, sweep_deg: f64) -> (f64, f64) {
    let low = turn_into_range(start_deg);
    let high = low + sweep_deg;
    if high <= 360.0 * (1.0 + EFF_ZERO) {
        (low, high.min(360.0))
    } else {
        (low - 360.0, high - 360.0)
    }
}

/// Whether the curve lies in the half-plane bounded by the axis and holding `x`
/// (`y = axis x x`), within `tol`, at its ends and `PLANE_SAMPLES - 1` inner points.
fn in_half_plane(
    k: &Kernel,
    curve: &Curve,
    base_pt: Point3,
    x: Vec3,
    y: Vec3,
    tol: f64,
) -> Result<bool> {
    let interval = curve.natural_interval();
    for i in 0..=PLANE_SAMPLES {
        let t = interval.evaluate(f64::from(i) / f64::from(PLANE_SAMPLES));
        let p = curve.evaluate_point(k, t)? - base_pt;
        if p.dot(y).abs() > tol || p.dot(x) < -tol {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `curve` run the other way (from `end` to `start`): a line its segment, another
/// curve a copy with its parameterisation reversed.
fn reversed_curve(k: &Kernel, curve: &Curve, start: Point3, end: Point3) -> Result<Curve> {
    if matches!(curve.kind(), CurveKind::Line(_)) {
        return Ok(Curve::from(BSplineCurve::create_line_segment(
            3, end, start,
        )?));
    }
    let mut back = curve.clone();
    let _ = back.reverse_parameterization(k, &curve.natural_interval())?;
    Ok(back)
}

/// The analytic surface for the curve's shape, before the layout test, and whether it
/// is laid out as the reversed curve's B-spline (a line falling along the axis, an arc
/// falling round its circle).
fn candidate(
    k: &Kernel,
    curve: &Curve,
    base_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
    tol: f64,
    axis_tol: f64,
    horn_tol: f64,
) -> Result<Option<(Surface, bool)>> {
    let ends = curve.ends(k)?;
    let (start_dist, _) = line_point_distance(base_pt, axis, ends.start);
    let (end_dist, _) = line_point_distance(base_pt, axis, ends.end);

    if curve.is_linear(k, EFF_ZERO, false)?.linear {
        let start_h = line_closest_point(base_pt, axis, ends.start);
        let end_h = line_closest_point(base_pt, axis, ends.end);
        let height = end_h - start_h;
        // perpendicular to the axis: a plane, which the callers make
        if height.abs() <= tol {
            return Ok(None);
        }
        // from the end farther from the axis (the other may be the apex)
        let far = if start_dist >= end_dist {
            ends.start
        } else {
            ends.end
        };
        let Some(x) = radial(base_pt, axis, far, tol) else {
            return Ok(None);
        };
        let y = axis.cross(x);
        if !in_half_plane(k, curve, base_pt, x, y, tol)? {
            return Ok(None);
        }
        // the generator line rises along the axis from the origin at the curve's low
        // end, never inside out (#1463: source fork 9853e54's AnalyticSurface;
        // an inside-out cylinder's NURB is recognised with its axis reversed by
        // IsNurbSurfaceCylinder). A curve falling along the axis is laid out as the
        // reversed curve's B-spline (v from its end).
        let reversed = height < 0.0;
        let (low_h, bot, top) = if reversed {
            (end_h, end_dist, start_dist)
        } else {
            (start_h, start_dist, end_dist)
        };
        let inside_out = false;
        let origin = base_pt + axis * low_h;
        let mut cone = if (start_dist - end_dist).abs() <= tol {
            Cone::new_cylinder_bounded(
                origin,
                x,
                y,
                bot,
                0.0,
                angle_deg,
                height.abs(),
                false,
                inside_out,
                false,
            )
        } else {
            Cone::new_bounded(
                origin,
                x,
                y,
                bot,
                top,
                0.0,
                angle_deg,
                height.abs(),
                false,
                inside_out,
                false,
            )
        };
        cone.make_nurb(k)?;
        return Ok(Some((Surface::from(cone), reversed)));
    }

    let Some((frame, radius, arc_start, arc_end)) = curve.is_arc(k, 8, tol)? else {
        return Ok(None);
    };
    let sweep = arc_end - arc_start;
    let is_arc = sweep > 0.0 && sweep <= 360.0 * (1.0 + EFF_ZERO);
    if !is_arc {
        return Ok(None);
    }
    let centre = frame.origin();
    let (centre_dist, _) = line_point_distance(base_pt, axis, centre);
    let interval = curve.natural_interval();
    let mid = curve.evaluate_point(k, interval.evaluate(0.5))?;
    let to_deg = 180.0 / PI;

    if centre_dist <= tol.max(axis_tol) {
        // a sphere: its meridian is the arc, in the plane of x and the axis
        let Some(x) = radial(base_pt, axis, mid, tol) else {
            return Ok(None);
        };
        let y = axis.cross(x);
        if !in_half_plane(k, curve, base_pt, x, y, tol)? {
            return Ok(None);
        }
        // WR-471: a centre off the axis by more than `tol` but within `axis_tol` makes the
        // sphere centred on the axis through the arc's two ends (the corners the
        // neighbouring profile edges share), not the arc's own circle moved onto the axis,
        // which misses both ends and leaves a pole parallel of radius about
        // sqrt(2 axis_tol R) where the arc ends on the axis
        let (mut centre, mut radius) = (centre, radius);
        if centre_dist > tol {
            let (a0, a1) = (
                line_closest_point(base_pt, axis, ends.start),
                line_closest_point(base_pt, axis, ends.end),
            );
            let h = if (a1 - a0).abs() > axis_tol {
                0.5 * (a0 + a1)
                    + 0.5 * (end_dist - start_dist) * (end_dist + start_dist) / (a1 - a0)
            } else {
                line_closest_point(base_pt, axis, centre)
            };
            centre = base_pt + axis * h;
            radius = 0.5 * ((a0 - h).hypot(start_dist) + (a1 - h).hypot(end_dist));
        }
        let latitude = |p: Point3| {
            let d = p - centre;
            d.dot(axis).atan2(d.dot(x)) * to_deg
        };
        let (start_lat, end_lat) = (latitude(ends.start), latitude(ends.end));
        // the meridian rises from x towards the axis (x cross axis is -y)
        // #1490: never inside out (fork 9853e54 builds the sphere over its latitude
        // range); a falling meridian is laid out as the reversed arc's B-spline
        let rising = frame.z_axis().dot(-y) > 0.0;
        let (low, high) = if rising {
            (start_lat, end_lat)
        } else {
            (end_lat, start_lat)
        };
        let inside_out = false;
        if high - low <= EFF_ZERO {
            return Ok(None);
        }
        let mut sphere = Sphere::new(
            k,
            centre,
            x,
            y,
            Extent2d::from_bounds(0.0, low, angle_deg, high),
            radius,
            SphereOptions {
                swap_uv: false,
                inside_out,
                circle_nurb: None,
            },
        )?;
        sphere.make_nurb(k)?;
        return Ok(Some((Surface::from(sphere), !rising)));
    }

    // a torus: its minor circle is the arc's, centred off the axis
    let Some(x) = radial(base_pt, axis, centre, tol) else {
        return Ok(None);
    };
    let y = axis.cross(x);
    if !in_half_plane(k, curve, base_pt, x, y, tol)? {
        return Ok(None);
    }
    let origin = base_pt + axis * line_closest_point(base_pt, axis, centre);
    let minor_angle = |p: Point3| {
        let d = p - centre;
        d.dot(axis).atan2(d.dot(x)) * to_deg
    };
    // #1490: never inside out (fork 9853e54's `CreateCanonical(.., FALSE, FALSE, ..)`
    // over the tube angle range from its low end); an arc falling round its minor
    // circle is laid out as the reversed arc's B-spline
    let rising = frame.z_axis().dot(-y) > 0.0;
    let start = if rising {
        minor_angle(ends.start)
    } else {
        minor_angle(ends.end)
    };
    let inside_out = false;
    let (low, high) = tube_range(start, sweep);
    // WR-552 (source fork@c4e378d): a circle that reaches the axis to within the
    // caller's tolerance, as the sweep takes a vertex there to be on it, sweeps a horn
    // torus, its tube through the axis there. Swept as it lay (a hair inside, a spindle
    // torus; outside, a ring one), the minor circles crossed or missed the axis where the
    // topology collapses, and the face's loop there came out turned over. The snap moves
    // the surface by at most that difference, so the layout test below keeps it only when
    // the difference is within the fit tolerance (else the B-spline stays).
    let major = if (centre_dist - radius).abs() <= horn_tol {
        radius
    } else {
        centre_dist
    };
    let mut torus = Torus::new(
        k,
        origin,
        x,
        y,
        Extent2d::from_bounds(0.0, low, angle_deg, high),
        major,
        radius,
        TorusOptions {
            swap_uv: false,
            inside_out,
            circle_nurb: None,
        },
    )?;
    torus.make_nurb(k)?;
    Ok(Some((Surface::from(torus), !rising)))
}

/// What a surface of revolution's natural domain shows of its layout.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// The points at the corners `(umin, vmin)`, `(umin, vmax)`, `(umax, vmin)`,
    /// `(umax, vmax)`.
    corners: [Point3; 4],
    /// The tangent along `v` at `(umin, vmin)`.
    start_tangent: Vec3,
    /// The sign of the `u` tangent against the rotation about the axis, at the
    /// domain's centre.
    turn: f64,
}

impl Layout {
    /// The layout of `surface` about the axis through `base_pt` along unit `axis`.
    fn of(k: &Kernel, surface: &Surface, base_pt: Point3, axis: Vec3) -> Result<Self> {
        let d = surface.natural_uv_domain_with_nurb(k)?;
        let mut corners = [Point3::ZERO; 4];
        for (corner, (u, v)) in
            corners
                .iter_mut()
                .zip([(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)])
        {
            *corner = surface.evaluate_point(k, d.evaluate(u, v))?;
        }
        let flags = EvaluateFlags::default();
        let start = surface.evaluate(k, d.evaluate(0.0, 0.0), 0, 1, flags)?;
        let start_tangent = start
            .first()
            .and_then(|row| row.get(1))
            .copied()
            .unwrap_or(Vec3::ZERO);
        let centre = surface.evaluate(k, d.evaluate(0.5, 0.5), 1, 0, flags)?;
        let point = centre.first().and_then(|row| row.first()).copied();
        let du = centre.get(1).and_then(|row| row.first()).copied();
        let turn = match (point, du) {
            (Some(p), Some(du)) => du.dot(axis.cross(p - base_pt)).signum(),
            _ => 0.0,
        };
        Ok(Self {
            corners,
            start_tangent,
            turn,
        })
    }

    /// Whether this layout is `other`'s: the corners within `tol`, the start
    /// tangents the same way and the same, defined, sense of turn.
    fn matches(&self, other: &Self, tol: f64) -> bool {
        self.corners
            .iter()
            .zip(&other.corners)
            .all(|(a, b)| (*a - *b).length() <= tol)
            && self.start_tangent.dot(other.start_tangent) > 0.0
            && self.turn != 0.0
            && self.turn == other.turn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::CurveKind;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::nurbs::b_spline_curve::{BSplineCurve, NurbCircleParam};
    use crate::nurbs::b_spline_surface::BSplineSurface;

    fn bspline_rev(k: &Kernel, curve: &Curve, angle: f64) -> Surface {
        let CurveKind::BSpline(b) = curve.kind() else {
            panic!("B-spline profile")
        };
        Surface::from(
            BSplineSurface::create_surf_of_revolution_with_line_vector(
                k,
                b,
                None,
                Vec3::ZERO,
                Vec3::Z,
                angle,
                false,
                None,
            )
            .unwrap(),
        )
    }

    fn line(a: Vec3, b: Vec3) -> Curve {
        Curve::from(BSplineCurve::create_line_segment(3, a, b).unwrap())
    }

    /// The arc of radius `r` about `centre` in the XZ plane from `start` to `end`
    /// degrees, measured from `x` towards `y`.
    fn arc(centre: Vec3, x: Vec3, y: Vec3, r: f64, start: f64, end: f64) -> Curve {
        Curve::from(
            BSplineCurve::create_circle_segment(
                3,
                &Axis2Placement::new(centre, x, y),
                r,
                start,
                end,
                NurbCircleParam::Quadratic,
            )
            .unwrap(),
        )
    }

    fn kind(s: &Surface) -> &'static str {
        use crate::surface::surface::SurfaceKind;
        match s.kind() {
            SurfaceKind::Cone(c) if c.is_cylinder_class() => "cylinder",
            SurfaceKind::Cone(_) => "cone",
            SurfaceKind::Sphere(_) => "sphere",
            SurfaceKind::Torus(_) => "torus",
            _ => "other",
        }
    }

    /// The expected kind of a profile that keeps the B-spline surface.
    const UPSTREAM: &str = "upstream";

    /// Every profile kind, both ways along the curve: (name, curve, expected kind).
    fn profiles() -> Vec<(&'static str, Curve, &'static str)> {
        let mut out = vec![
            (
                "cylinder up",
                line(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 3.0)),
                "cylinder",
            ),
            (
                "cylinder down",
                line(Vec3::new(2.0, 0.0, 3.0), Vec3::new(2.0, 0.0, 0.0)),
                "cylinder",
            ),
            (
                "cone up",
                line(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 1.0)),
                "cone",
            ),
            (
                "cone down",
                line(Vec3::new(2.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0)),
                "cone",
            ),
            (
                "drill point",
                line(Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 0.0)),
                "cone",
            ),
            (
                "apex first",
                line(Vec3::new(0.0, 0.0, 2.0), Vec3::new(1.5, 0.0, 0.0)),
                "cone",
            ),
        ];
        // the partial sphere's half circle: x = -Z, z = Y, pole to pole
        out.push((
            "half circle",
            arc(Vec3::ZERO, -Vec3::Z, Vec3::X, 3.0, 0.0, 180.0),
            "sphere",
        ));
        out.push((
            "half circle down",
            arc(Vec3::ZERO, Vec3::Z, Vec3::X, 3.0, 0.0, 180.0),
            "sphere",
        ));
        out.push((
            "dome",
            arc(Vec3::ZERO, Vec3::X, Vec3::Z, 2.0, 0.0, 90.0),
            "sphere",
        ));
        out.push((
            "dome down",
            arc(Vec3::ZERO, Vec3::Z, Vec3::X, 2.0, 0.0, 90.0),
            "sphere",
        ));
        // a full minor circle from its outer point (tube angle 0)
        out.push((
            "minor circle",
            arc(Vec3::new(5.0, 0.0, 0.0), Vec3::X, Vec3::Z, 1.5, 0.0, 360.0),
            "torus",
        ));
        out.push((
            "minor circle reversed",
            arc(Vec3::new(5.0, 0.0, 0.0), Vec3::X, -Vec3::Z, 1.5, 0.0, 360.0),
            "torus",
        ));
        // full minor circles from another point (the torus's iso circles there are
        // whole since #899). The partial torus's circle: x = -Z, about Y, from the
        // bottom
        out.push((
            "minor circle from the bottom",
            arc(Vec3::new(5.0, 0.0, 0.0), -Vec3::Z, Vec3::X, 1.5, 0.0, 360.0),
            "torus",
        ));
        out.push((
            "minor circle from the bottom reversed",
            arc(
                Vec3::new(5.0, 0.0, 0.0),
                -Vec3::Z,
                -Vec3::X,
                1.5,
                0.0,
                360.0,
            ),
            "torus",
        ));
        out.push((
            "horn minor circle",
            arc(
                Vec3::new(5.0, 0.0, 0.0),
                -Vec3::Z,
                Vec3::X,
                5.0,
                -180.0,
                180.0,
            ),
            "torus",
        ));
        out.push((
            "fillet arc",
            arc(
                Vec3::new(4.0, 0.0, 1.0),
                Vec3::X,
                Vec3::Z,
                1.0,
                270.0,
                360.0,
            ),
            "torus",
        ));
        out.push((
            "fillet arc reversed",
            arc(Vec3::new(4.0, 0.0, 1.0), Vec3::Z, Vec3::X, 1.0, 0.0, 90.0),
            "torus",
        ));
        out
    }

    #[test]
    fn each_profile_curve_sweeps_its_analytic_surface_in_the_b_spline_layout() {
        let k = Kernel::default();
        for angle in [90.0, 200.0, 360.0] {
            for (name, curve, expected) in profiles() {
                let reference = bspline_rev(&k, &curve, angle);
                for r in [None, Some(&reference)] {
                    let s = analytic_revolution(&k, &curve, Vec3::ZERO, Vec3::Z, angle, 1e-6, r)
                        .unwrap();
                    assert_eq!(
                        s.as_ref().map_or(UPSTREAM, kind),
                        expected,
                        "{name} {angle}"
                    );
                }
            }
        }
    }

    /// The distance from `p`, turned back into the half-plane `y = 0, x >= 0`, to the
    /// line or circle the profile lies on.
    fn off_profile(curve: &Curve, p: Vec3) -> f64 {
        let k = Kernel::default();
        let q = Vec3::new((p.x * p.x + p.y * p.y).sqrt(), 0.0, p.z);
        let ends = curve.ends(&k).unwrap();
        if let Some((frame, r, _, _)) = curve.is_arc(&k, 8, 1e-9).unwrap() {
            return ((q - frame.origin()).length() - r).abs();
        }
        let d = (ends.end - ends.start).unitize().unwrap();
        (q - ends.start).cross(d).length()
    }

    #[test]
    fn the_analytic_surface_is_the_swept_one() {
        let k = Kernel::default();
        for (name, curve, expected) in profiles() {
            if expected == UPSTREAM {
                continue;
            }
            let reference = bspline_rev(&k, &curve, 120.0);
            let s = analytic_revolution(
                &k,
                &curve,
                Vec3::ZERO,
                Vec3::Z,
                120.0,
                1e-6,
                Some(&reference),
            )
            .unwrap()
            .unwrap();
            let a = s.natural_uv_domain_if_built();
            for i in 0..=6 {
                for j in 0..=6 {
                    let (f, g) = (f64::from(i) / 6.0, f64::from(j) / 6.0);
                    let p = s.evaluate_point(&k, a.evaluate(f, g)).unwrap();
                    assert!(off_profile(&curve, p) < 1e-9, "{name} {f} {g}: {p:?}");
                    let azimuth = p.y.atan2(p.x).to_degrees();
                    if p.x.hypot(p.y) > 1e-3 {
                        assert!(
                            (-1e-9..=120.0 + 1e-9).contains(&azimuth),
                            "{name}: {azimuth}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_curve_only_near_an_arc_keeps_the_upstream_surface() {
        // sweep.api_surfaces gen0102: a 0.1 mm wave lies within a 1e-6 tolerance of an
        // arc, but its B-spline surface is not that torus (2.2e-7 off at a corner)
        let k = Kernel::default();
        let ys = [
            4.6447970745125225e-5,
            4.436047980382209e-5,
            3.1159569933491e-5,
            2.843411450601963e-5,
            1.5526105242107882e-5,
            2.5365927447340398e-5,
        ];
        let points: Vec<_> = ys
            .iter()
            .enumerate()
            .map(|(i, &y)| Vec3::new(2e-5 * i as f64, y, 0.0))
            .collect();
        let wave = Curve::from(crate::primitive::api_curves::create_curve(&points).unwrap());
        for tol in [1e-6, 1e-9] {
            let r = analytic_revolution(&k, &wave, Vec3::ZERO, Vec3::X, 180.03, tol, None);
            assert!(r.unwrap().is_none(), "{tol}");
        }
        // true lines and arcs still qualify at any scale
        for s in [1e-5, 1.0, 1e5] {
            let cases = [
                (
                    line(Vec3::new(2.0, 0.0, 0.0) * s, Vec3::new(1.0, 0.0, 3.0) * s),
                    "cone",
                ),
                (
                    arc(
                        Vec3::new(5.0, 0.0, 0.0) * s,
                        Vec3::X,
                        Vec3::Z,
                        1.5 * s,
                        0.0,
                        360.0,
                    ),
                    "torus",
                ),
                (
                    arc(Vec3::ZERO, Vec3::X, Vec3::Z, 2.0 * s, 0.0, 90.0),
                    "sphere",
                ),
            ];
            for (c, expected) in cases {
                let reference = bspline_rev(&k, &c, 90.0);
                let r =
                    analytic_revolution(&k, &c, Vec3::ZERO, Vec3::Z, 90.0, 1e-6, Some(&reference))
                        .unwrap()
                        .unwrap_or_else(|| panic!("{expected} at {s}"));
                assert_eq!(kind(&r), expected, "{s}");
            }
        }
    }

    #[test]
    fn tube_ranges_stay_inside_a_turn_each_way() {
        assert_eq!(tube_range(-90.0, 360.0), (-90.0, 270.0));
        assert_eq!(tube_range(0.0, 360.0), (0.0, 360.0));
        assert_eq!(tube_range(-1e-14, 360.0), (0.0, 360.0));
        assert_eq!(tube_range(270.0, 90.0), (270.0, 360.0));
        assert_eq!(tube_range(300.0, 90.0), (-60.0, 30.0));
        assert_eq!(tube_range(-180.0, 360.0), (-180.0, 180.0));
    }

    #[test]
    fn other_curves_keep_the_upstream_surface() {
        let k = Kernel::default();
        let none = |c: &Curve| {
            analytic_revolution(&k, c, Vec3::ZERO, Vec3::Z, 90.0, 1e-6, None)
                .unwrap()
                .is_none()
        };
        // perpendicular to the axis (a plane), on the axis, skew to the axis
        assert!(none(&line(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0)
        )));
        assert!(none(&line(Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0))));
        assert!(none(&line(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0)
        )));
        // an arc not in a plane through the axis
        assert!(none(&arc(
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::X,
            Vec3::Y,
            1.0,
            0.0,
            90.0
        )));
        // an arc in a plane through the axis, crossing to its other side
        assert!(none(&arc(Vec3::ZERO, Vec3::X, Vec3::Z, 2.0, 0.0, 270.0)));
        // no turn, no axis
        let c = line(Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 3.0));
        assert!(
            analytic_revolution(&k, &c, Vec3::ZERO, Vec3::Z, 0.0, 1e-6, None)
                .unwrap()
                .is_none()
        );
        assert!(
            analytic_revolution(&k, &c, Vec3::ZERO, Vec3::ZERO, 90.0, 1e-6, None)
                .unwrap()
                .is_none()
        );
    }
}
