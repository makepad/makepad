//! Ruled surfaces marrying offset curves to their generating curves.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`ruled_surface_from_offset`] | `sm_CreateRuledSurfaceFromOffsetCurve` |
//!
//! Deviations: the offset map is explicit, as in `CompositeCurve`'s offset output.
//! Mutated offset/generator curves and marriage flags survive errors. The returned
//! map is the surface attribute; callers attach it using their attribute store.
//! No recursion. Invalid indices return errors instead of upstream undefined reads.
//! Opt-in `Kernel::fixes.taper_pivot_nearest_corner` (WR-402): when the two links of a
//! corner meet at both ends (a two-curve loop), the pivot is the joint nearest the
//! corner's offset curve, not `sm_FindPivotPoint`'s first coincident pair.

use crate::curve::composite_curve::OffsetMapAttribute;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::geom_utility::p1::circle_from_3_points;
use crate::kernel::Kernel;
use crate::math::{Vec3, EFF_ZERO};
use crate::nurbs::b_spline_curve::p1::{BSplineCurve, BSplineCurveForm};
use crate::nurbs::b_spline_surface::p1::{BSplineSurface, SurfParamType};
use crate::primitive::primitive_creation::p1::find_pivot_point;

const OP: &str = "primitive::primitive_creation::p3::ruled_surface_from_offset";

/// Tolerances and placement for an offset ruled surface.
#[derive(Debug, Clone, Copy)]
pub struct OffsetRuleOptions {
    /// B-rep tolerance, also used in degree reduction and sliver rejection.
    pub tolerance: f64,
    /// Tighter tolerance for pivot selection and knot compatibility.
    pub same_point_tolerance: f64,
    /// Signed offset distance (used to stop a cone at its apex).
    pub offset: f64,
    /// Final placement of the offset curve.
    pub movement: Axis2Placement,
    /// Surface-intersection fallback, rather than the offset-curve method.
    pub surface_intersection: bool,
}

/// Build a ruled surface, retaining the modified offset and generator curves.
/// `generators` are the composite's parent curves; `intervals` are the original
/// input array's first two domains (upstream intentionally uses those domains).
/// `unmarried` records whether each original generator remains unused.
/// A missing map returns `SM_ERR` after reducing the offset's degree.
pub fn ruled_surface_from_offset(
    k: &Kernel,
    offset_curve: &mut BSplineCurve,
    map: Option<OffsetMapAttribute>,
    generators: &mut [BSplineCurve],
    intervals: [crate::geom::extent1d::Extent1d; 2],
    first_input_rational: bool,
    unmarried: &mut [bool],
    missing_faces: &mut bool,
    options: OffsetRuleOptions,
) -> Result<(BSplineSurface, Option<OffsetMapAttribute>)> {
    ignore_status(offset_curve.degree_reduction(options.tolerance, true))?;
    let map = map.ok_or_else(|| SolidError::sm_err(OP))?;
    let index = usize::try_from(map.curve).map_err(|_| SolidError::sm_err(OP))?;
    if !options.surface_intersection
        && !map.corner_offset
        && !unmarried
            .get(index)
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP))?
    {
        *missing_faces = true;
    }
    let source = generators
        .get(index)
        .ok_or_else(|| SolidError::sm_err(OP))?;
    let mut second = source.clone();
    let mut first = offset_curve.clone();
    let mut first_is_offset = true;
    let mut second_is_generator = true;
    if map.corner_offset {
        let other = usize::try_from(map.other_curve).map_err(|_| SolidError::sm_err(OP))?;
        let other = generators
            .get(other)
            .ok_or_else(|| SolidError::sm_err(OP))?;
        let pivot = if k.fixes.taper_pivot_nearest_corner {
            pivot_nearest_corner(source, other, options.same_point_tolerance, &first)?
        } else {
            find_pivot_point(source, other, options.same_point_tolerance)?
        };
        second = BSplineCurve::create_point_curve(pivot)?;
        second_is_generator = false;
        ignore_status(first.transform(&options.movement, None))?;
    } else {
        if source.form() == BSplineCurveForm::CircularArc {
            let interval = source.natural_interval();
            let oi = first.natural_interval();
            let start = ignored(source.evaluate_point(interval.min()))?.unwrap_or(Vec3::UNDEF);
            let end = ignored(source.evaluate_point(interval.max()))?.unwrap_or(Vec3::UNDEF);
            let offset_start = ignored(first.evaluate_point(oi.min()))?.unwrap_or(Vec3::UNDEF);
            if start.distance_squared(offset_start) > end.distance_squared(offset_start) {
                let mid = ignored(source.evaluate_point(interval.mid()))?.unwrap_or(Vec3::UNDEF);
                // The upstream ignores this status. Undefined circle outputs on
                // a failed three-point construction cannot safely form a NURB.
                let circle = circle_from_3_points(start, mid, end, 3, false)?;
                first = BSplineCurve::create_point_curve(circle.center)?;
                first_is_offset = false;
                let origin = options.movement.origin();
                let origin = Vec3::new(
                    circle.radius / options.offset * origin.x,
                    circle.radius / options.offset * origin.y,
                    circle.radius / options.offset * origin.z,
                );
                let movement = Axis2Placement::new(
                    origin,
                    options.movement.x_axis(),
                    options.movement.y_axis(),
                );
                ignore_status(first.transform(&movement, None))?;
            } else {
                ignore_status(first.transform(&options.movement, None))?;
            }
        } else {
            ignore_status(first.transform(&options.movement, None))?;
        }
        *unmarried
            .get_mut(index)
            .ok_or_else(|| SolidError::sm_err(OP))? = false;
    }
    // Transform precedes the missing-face early return upstream.
    if first_is_offset {
        *offset_curve = first.clone();
    }
    if *missing_faces && !options.surface_intersection {
        return Err(SolidError::sm_err(OP));
    }
    let [domain0, domain1] = intervals;
    let length0 = Curve::from(first.clone())
        .approximate_length(k, &domain0, 3, false)?
        .0;
    if length0 < 2.0 * options.tolerance
        && Curve::from(second.clone())
            .approximate_length(k, &domain1, 3, false)?
            .0
            < 2.0 * options.tolerance
    {
        return Err(SolidError::sm_err(OP));
    }
    let mut rules = [first, second];
    let compatible = BSplineCurve::make_curves_compatible(&mut rules, options.same_point_tolerance);
    // Compatibility mutates both originals even when it fails.
    if let Some(first) = rules.first().filter(|_| first_is_offset) {
        *offset_curve = first.clone();
    }
    if let (Some(second), Some(target)) = (
        rules.get(1).filter(|_| second_is_generator),
        generators.get_mut(index),
    ) {
        *target = second.clone();
    }
    compatible?;
    let [first, second] = &mut rules;
    if map.corner_offset && first_input_rational {
        if first.is_degenerate(k, EFF_ZERO, None)? {
            ignore_status(first.match_weights(second))?;
        } else if second.is_degenerate(k, EFF_ZERO, None)? {
            ignore_status(second.match_weights(first))?;
        }
    }
    if first_is_offset {
        *offset_curve = first.clone();
    }
    let surface = BSplineSurface::create_ruled_surface(first, second, SurfParamType::U)?;
    Ok((surface, options.surface_intersection.then_some(map)))
}

/// The joint of `first` and `second` nearest the ends of the untransformed
/// `corner` curve (WR-402). Two links closing a loop meet at both ends, and
/// `find_pivot_point` gives both corners the same joint; with one joint this is
/// `find_pivot_point`. Coincidence uses its squared-distance test.
fn pivot_nearest_corner(
    first: &BSplineCurve,
    second: &BSplineCurve,
    tol: f64,
    corner: &BSplineCurve,
) -> Result<Vec3> {
    let point = |c: &BSplineCurve, t: f64| -> Result<Vec3> {
        Ok(ignored(c.evaluate_point(t))?.unwrap_or(Vec3::UNDEF))
    };
    let (a, b, c) = (
        first.natural_interval(),
        second.natural_interval(),
        corner.natural_interval(),
    );
    let (start1, end1) = (point(first, a.min())?, point(first, a.max())?);
    let (start2, end2) = (point(second, b.min())?, point(second, b.max())?);
    let joints: Vec<Vec3> = [
        (start1, start2),
        (end1, start2),
        (start1, end2),
        (end1, end2),
    ]
    .into_iter()
    .filter(|(p, q)| (*p - *q).length_squared() < tol)
    .map(|(_, q)| q)
    .collect();
    if joints.len() < 2 {
        return find_pivot_point(first, second, tol);
    }
    let (corner_start, corner_end) = (point(corner, c.min())?, point(corner, c.max())?);
    let mut best: Option<(f64, Vec3)> = None;
    for joint in joints {
        let score = joint.distance_squared(corner_start) + joint.distance_squared(corner_end);
        if best.is_none_or(|(least, _)| score < least) {
            best = Some((score, joint));
        }
    }
    match best {
        Some((_, joint)) => Ok(joint),
        None => find_pivot_point(first, second, tol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent1d::Extent1d;

    #[test]
    fn missing_face_keeps_the_transform_and_marriage_side_effect() -> Result<()> {
        let mut offset = BSplineCurve::create_line_segment(3, Vec3::Y, Vec3::X + Vec3::Y)?;
        let mut generators = [
            BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X)?,
            BSplineCurve::create_line_segment(3, Vec3::X, Vec3::X + Vec3::Y)?,
        ];
        let map = OffsetMapAttribute::new(0, false, false, 0, 0, 1, None, 0.0);
        let mut unmarried = [false, true];
        let mut missing = false;
        let mut movement = Axis2Placement::IDENTITY;
        movement.translate(2.0 * Vec3::Z);
        let k = Kernel::default();
        let result = ruled_surface_from_offset(
            &k,
            &mut offset,
            Some(map),
            &mut generators,
            [Extent1d::from_min_max(0.0, 1.0); 2],
            false,
            &mut unmarried,
            &mut missing,
            OffsetRuleOptions {
                tolerance: 1e-5,
                same_point_tolerance: 1e-8,
                offset: 1.0,
                movement,
                surface_intersection: false,
            },
        );
        assert!(result.is_err());
        assert!(missing);
        assert_eq!(unmarried, [false, true]);
        assert_eq!(offset.evaluate_point(0.0)?, Vec3::Y + 2.0 * Vec3::Z);
        Ok(())
    }

    /// An outward draft of a half disc (a line and an arc closing the loop) with
    /// linear-extension corners and both caps is a closed solid with the opt-in:
    /// each corner rules to its own joint (WR-402).
    #[test]
    fn issue_479_two_curve_loop_taper_is_closed() {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};
        use crate::primitive::api_curves::{create_arc, create_line_segment};
        use crate::sweep::api_brep::create_taper_extrude;
        use crate::topology::brep::p2::is_manifold_solid;

        let line = Curve::from(
            create_line_segment(Vec3::new(-20.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0)).unwrap(),
        );
        let arc = Curve::from(create_arc(Vec3::ZERO, 20.0, 0.0, 180.0).unwrap());
        let curves = [Some(&line), Some(&arc)];

        let mut k = Kernel::default();
        k.fixes.taper_pivot_nearest_corner = true;
        let mut solid = create_taper_extrude(&mut k, &curves, 10.0, -3.0, 3, false).unwrap();
        assert!(is_manifold_solid(&solid).unwrap());
        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        let volume = brep_volume(&k, &mut solid, 1e-6, &attributes).unwrap();
        // pi (20 + t)^2 / 2 + 2 (20 + t) t integrated over the draft
        assert!((volume - 6555.9139).abs() < 1e-2, "volume {volume}");
    }
}
