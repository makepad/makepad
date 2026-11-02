//! Primitive creation state, planar-path helpers and primitive entry points.
//!
//! Upstream: `source/SMLib/inc/SmPrimitiveCreation.h`,
//! `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation`] | `SmPrimitiveCreation` |
//! | [`EntityMaps`] | the four `m_vMap*` members |
//! | [`PrimitiveCreation::init_entity_maps`] | `InitEntityMaps` |
//! | [`PrimitiveCreation::copy_entity_maps`] | `CopyEntityMaps` |
//! | [`find_pivot_point`] | `sm_FindPivotPoint` |
//! | [`extend_path_curves`] | `ExtendPathCurves` |
//! | [`position_profile_curves`] | `PositionProfileCurves` |
//! | [`offset_profile`] | static `OffsetProfile` |
//!
//! Deviations: owned curves and typed arena ids replace pointers. The four maps
//! use ordered maps; comparisons observe counts and lookups, never address hash order.
//! `CopyEntityMaps` copies FROM the primitive TO the sweep, despite its name.
//! Ignored statuses retain the previous output and propagate only Unported.
//! No recursion. Primitive topology construction stops at the first unported
//! B-rep edit; it never fabricates faces from arena records.

mod boolean;
mod offset_profile;
mod primitives;

pub use offset_profile::offset_profile;
pub use primitives::{analytic_rotational_sweep, create_circle, create_rectangle, Boolean2d};

use std::collections::BTreeMap;

use crate::curve::curve::p2::Curve;
use crate::curve::curve::p3::compute_crvs_normal;
use crate::error::{ignore_status, ignored, EntityRef, Result};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::crv_adv::p1::CurveEnd;
use crate::topology::RegionId;

/// Four topology associations of a primitive or topology sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityMaps {
    /// Original entity to its translated counterpart.
    pub to_same: BTreeMap<EntityRef, EntityRef>,
    /// Original entity to the entity swept from it.
    pub to_higher: BTreeMap<EntityRef, EntityRef>,
    /// Translated entity to its original.
    pub from_same: BTreeMap<EntityRef, EntityRef>,
    /// Swept entity to its generating entity.
    pub from_lower: BTreeMap<EntityRef, EntityRef>,
}

/// Target region, tag and associations of `SmPrimitiveCreation`.
#[derive(Debug, Clone, Default)]
pub struct PrimitiveCreation {
    region: Option<RegionId>,
    tag_id: i64,
    /// Topology associations from the latest operation.
    pub maps: EntityMaps,
}

impl PrimitiveCreation {
    /// Constructor. `use_analytics` is unused upstream too.
    #[must_use]
    pub fn new(region: Option<RegionId>, tag_id: i64, _use_analytics: bool) -> Self {
        Self {
            region,
            tag_id,
            maps: EntityMaps::default(),
        }
    }

    /// The target region (`NULL` for the protected default constructor).
    #[must_use]
    pub const fn region(&self) -> Option<RegionId> {
        self.region
    }

    /// The caller's tag (`0` means no tagging).
    #[must_use]
    pub const fn tag_id(&self) -> i64 {
        self.tag_id
    }

    /// Change the target region (`Become`).
    pub fn become_region(&mut self, region: Option<RegionId>) {
        self.region = region;
    }

    /// Empty all four maps, retaining the region and tag.
    pub fn init_entity_maps(&mut self) {
        self.maps = EntityMaps::default();
    }

    /// Replace the sweep's maps with this primitive's maps (`CopyEntityMaps`).
    pub fn copy_entity_maps(&self, sweep: &mut EntityMaps) {
        sweep.clone_from(&self.maps);
    }
}

/// Join point of two links. Upstream compares squared distance with `tol`,
/// without squaring that tolerance; otherwise it returns the second curve's end.
/// Failed evaluations keep the default undefined point.
pub fn find_pivot_point(first: &BSplineCurve, second: &BSplineCurve, tol: f64) -> Result<Point3> {
    let a = first.natural_interval();
    let b = second.natural_interval();
    let start1 = ignored(first.evaluate_point(a.min()))?.unwrap_or(Vec3::UNDEF);
    let start2 = ignored(second.evaluate_point(b.min()))?.unwrap_or(Vec3::UNDEF);
    if (start1 - start2).length_squared() < tol {
        return Ok(start2);
    }
    let end1 = ignored(first.evaluate_point(a.max()))?.unwrap_or(Vec3::UNDEF);
    if (end1 - start2).length_squared() < tol {
        return Ok(start2);
    }
    Ok(ignored(second.evaluate_point(b.max()))?.unwrap_or(Vec3::UNDEF))
}

fn evaluate_keep(curve: &BSplineCurve, t: f64, from_left: bool, out: &mut [Vec3; 2]) -> Result<()> {
    if let Some(values) = ignored(curve.evaluate(t, 1, from_left, false))? {
        for (target, value) in out.iter_mut().zip(values) {
            *target = value;
        }
    }
    Ok(())
}

fn extend(curve: &mut BSplineCurve, distance: f64, end: CurveEnd) -> Result<()> {
    if let Some(Some(next)) =
        ignored(curve.create_extended_curve_by_distance(distance, end, ContinuityType::G1))?
    {
        *curve = next;
    }
    Ok(())
}

/// Extend non-parallel path joins by arc length. The original input list stays
/// unchanged: each output replaces the local pointer upstream, while neighbours
/// are always read from the original list. The derivative buffers persist across
/// iterations, just as the two upstream stack arrays do.
pub fn extend_path_curves(
    paths: &[BSplineCurve],
    closed: bool,
    periodic: bool,
    distance: f64,
) -> Result<Vec<BSplineCurve>> {
    let mut output = Vec::new();
    let mut before = [Vec3::UNDEF; 2];
    let mut after = [Vec3::UNDEF; 2];
    for (index, source) in paths.iter().enumerate() {
        let mut path = source.clone();
        if let Some(previous) = index.checked_sub(1).and_then(|i| paths.get(i)) {
            evaluate_keep(
                previous,
                previous.natural_interval().max(),
                true,
                &mut before,
            )?;
            evaluate_keep(&path, path.natural_interval().min(), false, &mut after)?;
            if !before
                .get(1)
                .copied()
                .unwrap_or(Vec3::UNDEF)
                .is_parallel_to(after.get(1).copied().unwrap_or(Vec3::UNDEF), 2.0)
            {
                extend(&mut path, distance, CurveEnd::Start)?;
            }
        } else if closed && !periodic {
            extend(&mut path, distance, CurveEnd::Start)?;
        }
        if let Some(next) = paths.get(index + 1) {
            evaluate_keep(&path, path.natural_interval().max(), true, &mut before)?;
            evaluate_keep(next, next.natural_interval().min(), false, &mut after)?;
            if !before
                .get(1)
                .copied()
                .unwrap_or(Vec3::UNDEF)
                .is_parallel_to(after.get(1).copied().unwrap_or(Vec3::UNDEF), 2.0)
            {
                extend(&mut path, distance, CurveEnd::End)?;
            }
        } else if closed && !periodic {
            extend(&mut path, distance, CurveEnd::End)?;
        }
        output.push(path);
    }
    Ok(output)
}

/// Copy and position a profile at the path start (`PositionProfileCurves`).
/// The tolerance is unused upstream. Copies are appended before transforming
/// the entire output list, including any entries supplied by the caller.
pub fn position_profile_curves(
    k: &Kernel,
    profiles: &[Curve],
    _tol: f64,
    path: &Curve,
    output: &mut Vec<Curve>,
) -> Result<()> {
    let mut total = Extent3d::new();
    let mut bounds = Extent3d::new();
    for profile in profiles {
        if let Some(boxes) = ignored(profile.calculate_bounding_box(
            k,
            &profile.natural_interval(),
            BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            },
            true,
        ))? {
            if let Some(normal) = boxes.normal {
                bounds = normal;
            }
        }
        total.add_point(bounds.min());
        total.add_point(bounds.max());
        // A failed Copy produces NULL upstream, then Transform dereferences it.
        output.push(profile.copy(k)?);
    }
    let mut translation = Axis2Placement::IDENTITY;
    ignore_status(translation.set_canonical(-total.mid(), Vec3::X, Vec3::Y))?;
    for curve in output.iter_mut() {
        ignore_status(curve.transform(k, &translation, None))?;
    }
    let normal = ignored(compute_crvs_normal(k, profiles))?.unwrap_or(Vec3::UNDEF);
    let mut orientation = Axis2Placement::IDENTITY;
    let angle = ignored(Vec3::X.angle_between(normal))?.unwrap_or(0.0);
    orientation.rotate_about_axis(-angle, Vec3::X.cross(normal));
    for curve in output.iter_mut() {
        ignore_status(curve.transform(k, &orientation, None))?;
    }
    let mut start = [Vec3::UNDEF; 2];
    if let Some(values) = ignored(path.evaluate(k, path.natural_interval().min(), 1, true, true))? {
        for (target, value) in start.iter_mut().zip(values) {
            *target = value;
        }
    }
    let origin = start.first().copied().unwrap_or(Vec3::UNDEF);
    let tangent = start.get(1).copied().unwrap_or(Vec3::UNDEF);
    let mut y = Vec3::Z.cross(tangent);
    if tangent.z.abs() > tangent.x.abs() && tangent.z.abs() > tangent.y.abs() {
        y = Vec3::Y;
    }
    // MakeUnitOrthoVectors writes/unitizes X before any later failure.
    // Its caller ignores that status but still uses the partially written axes.
    let mut x = tangent.unitize_or_z();
    if let Some([next_x, next_y, _]) = ignored(tangent.make_unit_ortho_vectors(Some(y)))? {
        x = next_x;
        y = next_y;
    }
    let mut placement = Axis2Placement::IDENTITY;
    ignore_status(placement.set_canonical(origin, x, y))?;
    for curve in output.iter_mut() {
        ignore_status(curve.transform(k, &placement, None))?;
    }
    Ok(())
}
