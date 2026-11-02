//! Precise edge, face and B-rep properties and their `SM_API` query contracts.
//!
//! Upstream: `source/SMLib/src/SmEdge.cpp`, `source/SMLib/src/SmFace.cpp`,
//! `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmTopoTypes.h`,
//! `source/SM_API/src/SmApiQueries.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`edge_properties`] | `SmEdge::ComputePreciseProperties` |
//! | [`face_properties`], [`boundary_properties`] | `SmFace::ComputePreciseProperties` |
//! | [`brep_properties`] | `SmBrep::ComputePreciseProperties` |
//! | [`MassPropertyBehavior`] | `SmMassPropertyBehavior` |
//! | [`edge_length`], [`face_area`], [`face_centroid`] | `SmApiEdgeComputeLength`, `SmApiFaceComputeArea`, `SmApiFaceComputeCentroid` |
//! | [`brep_area`], [`brep_volume`], [`brep_mass_properties`] | `SmApiBrepComputeArea`, `SmApiBrepComputeVolume`, `SmApiBrepComputeMassProperties` |
//!
//! Deviations: arena ids replace pointers, results replace output arguments; attribute
//! storage and object keys are supplied by the caller. Oriented trim construction,
//! B-rep enumeration, bounding boxes, and region classification remain calls into
//! their unported units. `boundary_properties` exposes the integration after trim
//! construction for callers already holding oriented trim curves. Nested integration
//! has a fixed two-level call structure, not input-driven recursion. With
//! `UpstreamFixes::precise_properties_split_at_surface_knots` (WR-494) both levels split
//! at the surface's knots, as the source fork's source fork@08d9a14 does.
//! With `UpstreamFixes::brep_area_between_solid_and_void` (WR-535) a volume-separating
//! faceuse adds its volume only where it bounds a non void region and its area only
//! where the area behaviour selects it (source fork@d2d9586).
//! With `UpstreamFixes::precise_properties_trim_base_v` (WR-563, #1556) the iso-u
//! integrals start at the trims' lowest v, not at the face UV domain's.
use super::{OdeIntegrator, OdeOptions};
use crate::curve::curve::Curve;
use crate::geom::attribute::{ids, AttributeStore, AttributeValue, ObjectKey};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, PI};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::surface::surface::Surface;
use crate::topology::face::p2::{
    trim_curve_breaks, OdeArea, PrecisePropertyLayout, PPF_ALL, PPF_AREA, PPF_AREA_FIRST_MOMENTS,
};
use crate::topology::{edge, vertex, Brep, EdgeId, FaceId, FaceuseId, RegionId, VertexId};
use crate::{EntityRef, Result, SolidError};

const OP: &str = "mass::integrator::properties";

/// Surface/volume measures and the eight moment vectors of upstream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreciseProperties {
    /// Surface area.
    pub area: f64,
    /// Enclosed/tube volume.
    pub volume: f64,
    /// Density-weighted volume (B-rep layer only).
    pub mass: f64,
    /// Area first, second, products, axial; then volume/mass equivalents.
    pub moments: [Vec3; 8],
}
/// Edge length and tube properties.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgeProperties {
    /// Curve length on the edge interval.
    pub length: f64,
    /// Tube properties; its last four vectors are geometric volume integrals.
    pub properties: PreciseProperties,
}
/// Controls of `SmMassPropertyBehavior`.
#[derive(Debug, Clone, Copy)]
pub struct MassPropertyBehavior {
    /// Default density, overridden by per-entity mass attributes.
    pub density: f64,
    /// Thickness of sheet faces.
    pub face_thickness: f64,
    /// Radius of wire-edge tubes.
    pub edge_radius: f64,
    /// Radius of isolated-vertex spheres.
    pub vertex_radius: f64,
    /// Area policy: exterior of solid (0), once per face (1), both uses (2).
    pub face_area: u32,
    /// Volume policy: solid regions (0), thickness only (1).
    pub face_volume: u32,
}
impl Default for MassPropertyBehavior {
    fn default() -> Self {
        Self {
            density: 1.0,
            face_thickness: 0.0,
            edge_radius: 0.0,
            vertex_radius: 0.0,
            face_area: 0,
            face_volume: 0,
        }
    }
}
/// The session attribute store and the identities of topology records in it.
pub struct PropertyAttributes<'a> {
    /// Attribute storage.
    pub store: &'a AttributeStore,
    /// Converts an arena entity to its attribute identity.
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectKey,
}
impl PropertyAttributes<'_> {
    fn mass_vector(&self, entity: EntityRef, default: Vec3) -> Result<Vec3> {
        let Some(h) = self
            .store
            .find_attribute((self.key_of)(entity), ids::MASS_PROPERTIES)
        else {
            return Ok(default);
        };
        match self
            .store
            .get(h)
            .map(crate::geom::attribute::Attribute::value)
        {
            Some(AttributeValue::Vector3d(v) | AttributeValue::Color(v)) => Ok(*v),
            _ => Err(SolidError::invalid_input(
                OP,
                "mass attribute is not a vector",
            )),
        }
    }
    fn has_mass(&self, entity: EntityRef) -> bool {
        self.store
            .find_attribute((self.key_of)(entity), ids::MASS_PROPERTIES)
            .is_some()
    }
}

/// `SmEdge::ComputePreciseProperties` on a live edge.
pub fn edge_properties(
    k: &Kernel,
    b: &Brep,
    e: EdgeId,
    accuracy: f64,
    origin: Point3,
    radius: f64,
) -> Result<EdgeProperties> {
    let curve = edge::curve(b, e)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    curve_properties(k, curve, &edge::interval(b, e)?, accuracy, origin, radius)
}
/// The precise edge integration on an already-bound curve and interval.
pub fn curve_properties(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    accuracy: f64,
    origin: Point3,
    radius: f64,
) -> Result<EdgeProperties> {
    let eval = edge::OdeEdgeLength::new(curve, origin, radius);
    let approx = curve.approximate_length(k, interval, 10, false)?.0;
    let mut y = [0.0; 27];
    let mut scale = [BIG_DOUBLE; 27];
    if let Some(s) = scale.first_mut() {
        *s = approx / 10.0;
    }
    let mut scratch = [0.0; 28];
    OdeIntegrator::default().integrate(
        &mut y,
        interval.min(),
        interval.max(),
        OdeOptions {
            accuracy,
            initial_step: (interval.max() - interval.min()) / 2.0,
            minimum_step: 0.0,
            scale: Some(&scale),
        },
        |t, _, dy| {
            eval.evaluate(k, t, &mut scratch)?;
            for (d, v) in dy.iter_mut().zip(scratch.iter().skip(1)) {
                *d = *v;
            }
            Ok(())
        },
    )?;
    let mut iter = y.into_iter();
    let length = iter.next().unwrap_or(0.0);
    let mut properties = PreciseProperties {
        area: iter.next().unwrap_or(0.0),
        volume: iter.next().unwrap_or(0.0),
        ..PreciseProperties::default()
    };
    for m in &mut properties.moments {
        *m = Vec3::new(
            iter.next().unwrap_or(0.0),
            iter.next().unwrap_or(0.0),
            iter.next().unwrap_or(0.0),
        );
    }
    Ok(EdgeProperties { length, properties })
}

/// `SmFace::CreateOrientedTrimBoundary(ctx, TRUE, TRUE, ..)` (unit `topology.face_p1`) on
/// the caller's B-rep: the UV trim curves it makes stay there, as upstream. The returned
/// curves' edge-use owners are not read here.
fn boundary(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<Vec<Curve>> {
    let key_of = |e: EntityRef| crate::geom::object::ObjectRef {
        type_code: 0,
        key: ObjectKey(match e {
            EntityRef::Edgeuse(eu) => u64::from(eu.index()),
            _ => 0,
        }),
    };
    crate::topology::face::p1::create_oriented_trim_boundary(k, b, f, true, true, None, &key_of)
        .map(|t| t.curves)
}
/// `SmFace::ComputePreciseProperties`; the default property mask is `PPF_ALL`. Missing UV
/// trim curves of the face are made on `b`.
pub fn face_properties(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    orientation: OrientType,
    accuracy: f64,
    origin: Point3,
    estimated_area: f64,
    thickness: f64,
    flags: u64,
) -> Result<PreciseProperties> {
    valid_flags(flags)?;
    let trims = boundary(k, b, f)?;
    let fbox = face_box(k, b, f)?;
    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    boundary_properties(
        k,
        surface,
        &trims,
        &b.face(f)?.uv_domain,
        &fbox,
        orientation,
        accuracy,
        origin,
        estimated_area,
        thickness,
        flags,
    )
}
/// The lowest v of the union of the trims' bounding boxes (as
/// `CalculateUVDomainFromUVTrimCurves` builds the trim domain), no lower than the face's
/// UV domain; the domain's lowest v without trims.
fn trims_base_v(k: &Kernel, trims: &[Curve], domain: &Extent2d) -> Result<f64> {
    use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    let mut base = BIG_DOUBLE;
    for trim in trims {
        let bbox = trim
            .calculate_bounding_box(k, &trim.natural_interval(), req, true)?
            .normal
            .ok_or_else(|| SolidError::sm_err(OP))?;
        let v = bbox.min().y;
        if v < base {
            base = v;
        }
    }
    let min_v = domain.min().y;
    Ok(if trims.is_empty() || base < min_v {
        min_v
    } else {
        base
    })
}
fn valid_flags(flags: u64) -> Result<()> {
    if flags == 0 || flags & !PPF_ALL != 0 {
        Err(SolidError::invalid_input(OP, "invalid property mask"))
    } else {
        Ok(())
    }
}
/// Integrates the same precise face properties after oriented trim construction.
/// `trims` must have the orientation and order returned by `CreateOrientedTrimBoundary`;
/// `face_box` is the face's bounding box (`CalculateBoundingBox`), which sets the error
/// scale of the volume and moment components.
pub fn boundary_properties(
    k: &Kernel,
    surface: &Surface,
    trims: &[Curve],
    domain: &Extent2d,
    face_box: &Extent3d,
    orientation: OrientType,
    mut accuracy: f64,
    origin: Point3,
    estimated_area: f64,
    thickness: f64,
    flags: u64,
) -> Result<PreciseProperties> {
    // the moment groups' extra length dimensions
    const POWERS: [u32; 8] = [1, 2, 2, 2, 2, 3, 3, 3];
    valid_flags(flags)?;
    if accuracy < EFF_ZERO {
        accuracy = EFF_ZERO_SQ;
    }
    let layout = PrecisePropertyLayout::new(flags);
    let mut scale = vec![BIG_DOUBLE; layout.num_components];
    let origin_distance = face_box.min_distance(&Extent3d::from_point(origin));
    let sign = if orientation == OrientType::Same && thickness < EFF_ZERO {
        -1.0
    } else {
        1.0
    };
    // The base of every iso-u integral: upstream's `GetUVDomain().GetMin().y`. Any base
    // gives the same loop integral (a different base adds a function of u alone, whose
    // integral round a closed loop is zero), but one far below the trims makes each
    // integral long and the area a small difference of large terms. With
    // `fixes.precise_properties_trim_base_v` (WR-563, #1556) it is the trims' lowest v.
    let base_v = if k.fixes.precise_properties_trim_base_v {
        trims_base_v(k, trims, domain)?
    } else {
        domain.min().y
    };
    let mut result = PreciseProperties::default();
    for trim in trims {
        let area_scale = estimated_area / 10.0;
        if let Some(s) = scale.first_mut() {
            *s = area_scale;
        }
        // Scale volume and moment errors by a length per extra dimension. sqrt(area) is
        // a tenth of the body size; grow it once the origin is farther than the body.
        // With `fixes.precise_properties_body_length_scale` (WR-472; the source
        // fork's source fork@073d4740) the length is the body size, ten times
        // sqrt(area), or the origin's distance when larger: each component is then held
        // to the area's share of its own size, where 1.0.1's tenth holds each extra power
        // ten times tighter, below roundoff at tight accuracies.
        let length = if k.fixes.precise_properties_body_length_scale {
            let a = 10.0 * estimated_area.sqrt();
            let b = origin_distance;
            if a > b {
                a
            } else {
                b
            }
        } else {
            let a = estimated_area.sqrt();
            let b = origin_distance / 10.0;
            if a > b {
                a
            } else {
                b
            }
        };
        if layout.volume != 0 {
            if let Some(s) = scale.get_mut(layout.volume - 1) {
                *s = area_scale * length;
            }
        }
        for (&index, &power) in layout.moments.iter().zip(POWERS.iter()) {
            if index == 0 {
                continue;
            }
            let moment_scale = area_scale * length.powf(f64::from(power));
            for cc in 0..3 {
                if let Some(s) = scale.get_mut(index - 1 + cc) {
                    *s = moment_scale;
                }
            }
        }
        let eval = OdeArea {
            kernel: k,
            surface,
            uv_curve: trim,
            min_v: base_v,
            iso_curve_accuracy: accuracy / 10.0,
            y_scale: &scale,
            origin,
            face_thickness: thickness,
            layout: &layout,
        };
        let ivl = trim.natural_interval();

        // Integrate each knot span of the natural interval separately: the RK error
        // estimate assumes a smooth integrand, so a step across a knot can hide a large
        // error.
        //
        // With `fixes.precise_properties_split_at_surface_knots` (WR-494; the source
        // fork's source fork@08d9a14) the pieces are those between the trim
        // curve's knots and its crossings of the surface's knot lines u = k, where the
        // integrand's derivatives jump too.
        let knots = if k.fixes.precise_properties_split_at_surface_knots {
            trim_curve_breaks(k, trim, surface)?
        } else {
            trim.knots(k, Some(&ivl))?.0
        };

        // A coarse first Runge-Kutta step can trip the integrator's abort guard for one
        // traversal direction, so a reversed cap loop fails where its twin succeeds. Try
        // a span/2 first step, then fall back to finer steps.
        let mut y = vec![0.0; layout.num_components];
        let mut span_y = vec![0.0; layout.num_components];
        let mut scratch = vec![0.0; layout.num_components + 1];
        for pair in knots.windows(2) {
            let &[a, b] = pair else {
                continue;
            };
            // (`!(dB > dA)`: also an unordered pair)
            if k.fixes.precise_properties_split_at_surface_knots
                && b.partial_cmp(&a) != Some(std::cmp::Ordering::Greater)
            {
                continue; // a crossing at a knot
            }
            let mut status = Err(SolidError::sm_err(OP));
            for divisor in [2.0, 8.0, 64.0, 512.0, 4096.0] {
                status = OdeIntegrator::default()
                    .integrate(
                        &mut span_y,
                        a,
                        b,
                        OdeOptions {
                            accuracy: accuracy / 10.0,
                            initial_step: (b - a) / divisor,
                            minimum_step: 0.0,
                            scale: Some(&scale),
                        },
                        |t, _, dy| {
                            // A zero U tangent leaves the caller's derivative array
                            // untouched upstream. Preserve this particular stage's
                            // values, rather than another stage's shared scratch.
                            if let Some(unused) = scratch.first_mut() {
                                *unused = 0.0;
                            }
                            for (s, d) in scratch.iter_mut().skip(1).zip(dy.iter()) {
                                *s = *d;
                            }
                            eval.evaluate(k, t, &mut scratch)?;
                            for (d, v) in dy.iter_mut().zip(scratch.iter().skip(1)) {
                                *d = *v;
                            }
                            Ok(())
                        },
                    )
                    // Success with no steps means the integrator skipped the span.
                    .and_then(|counts| {
                        if counts.good + counts.reduced == 0 {
                            Err(SolidError::sm_err(OP))
                        } else {
                            Ok(())
                        }
                    });
                if status.is_ok() {
                    break;
                }
                if status.as_ref().is_err_and(SolidError::is_unported) {
                    break;
                }
            }
            // Propagate failure instead of dropping the boundary: a silent skip would
            // return success with zero/partial properties.
            status?;
            for (total, part) in y.iter_mut().zip(span_y.iter()) {
                *total += *part;
            }
        }
        let value = |index: usize| y.get(index.wrapping_sub(1)).copied().unwrap_or(0.0);
        if layout.area != 0 {
            result.area += value(layout.area);
        }
        if layout.volume != 0 {
            result.volume += value(layout.volume) * sign;
        }
        for (kk, (m, &index)) in result
            .moments
            .iter_mut()
            .zip(layout.moments.iter())
            .enumerate()
        {
            if index == 0 {
                continue;
            }
            let moment_sign = if kk < 4 { 1.0 } else { sign };
            m.x += value(index) * moment_sign;
            m.y += value(index + 1) * moment_sign;
            m.z += value(index + 2) * moment_sign;
        }
    }
    Ok(result)
}

fn face_box(k: &Kernel, b: &Brep, f: FaceId) -> Result<Extent3d> {
    crate::query::face::p1::calculate_bounding_box(k, b, f, false)
}
fn faces(b: &Brep) -> Result<Vec<FaceId>> {
    crate::topology::brep::p1::faces(b, None)
}
fn edges(b: &Brep) -> Result<Vec<EdgeId>> {
    crate::topology::brep::p1::edges(b, None)
}
fn vertices(b: &Brep) -> Result<Vec<VertexId>> {
    crate::topology::brep::p1::vertices(b, None)
}
/// The region on the side of face use `fu`.
fn faceuse_region(b: &Brep, fu: FaceuseId) -> Result<RegionId> {
    let shell = b
        .faceuse(fu)?
        .shell
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    b.shell(shell)?
        .region
        .ok_or_else(|| SolidError::null_pointer(OP))
}
/// `SmRegion::IsVoid` as a C truth value (`UNSURE`, 2, counts as void).
fn region_void(b: &Brep, r: RegionId) -> Result<bool> {
    Ok(b.region(r)?.is_void() != 0)
}
fn manifold_solid(b: &Brep) -> Result<bool> {
    crate::topology::brep::p2::is_manifold_solid(b)
}
fn add(total: &mut PreciseProperties, delta: &PreciseProperties, density: f64) {
    total.area += delta.area;
    total.volume += delta.volume;
    total.mass += delta.volume * density;
    for (i, (t, d)) in total
        .moments
        .iter_mut()
        .zip(delta.moments.iter())
        .enumerate()
    {
        *t += if i > 3 { *d * density } else { *d };
    }
}
/// [`add`] of a volume-separating faceuse with WR-535: its volume, mass and volume
/// moments (`4..`) only with `volume`, its area and area moments (`..4`) only with
/// `area`.
fn add_selected(
    total: &mut PreciseProperties,
    delta: &PreciseProperties,
    density: f64,
    volume: bool,
    area: bool,
) {
    if volume {
        total.volume += delta.volume;
        total.mass += delta.volume * density;
        for (t, d) in total.moments.iter_mut().zip(delta.moments.iter()).skip(4) {
            *t += *d * density;
        }
    }
    if area {
        total.area += delta.area;
        for (t, d) in total.moments.iter_mut().zip(delta.moments.iter()).take(4) {
            *t += *d;
        }
    }
}
/// `SmBrep::ComputePreciseProperties`, including density attributes and wire/point solids.
/// Missing UV trim curves of the integrated faces are made on `b`.
pub fn brep_properties(
    k: &Kernel,
    b: &mut Brep,
    accuracy: f64,
    origin: Point3,
    behavior: MassPropertyBehavior,
    attributes: &PropertyAttributes<'_>,
) -> Result<PreciseProperties> {
    let bbox = qbrep::calculate_bounding_box(k, &mut *b, BoundingBoxOptions::default())?;
    let size = bbox.size();
    let estimated = size.z.max(size.x.max(size.y));
    let estimated = estimated * estimated / 100.0;
    let mut total = PreciseProperties::default();
    for f in faces(b)? {
        let mut mass = attributes.mass_vector(
            EntityRef::Face(f),
            Vec3::new(behavior.density, behavior.face_thickness, 0.0),
        )?;
        if mass.x < EFF_ZERO {
            mass.x = 1.0;
        }
        let up = b
            .face(f)?
            .faceuse
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let mate = b
            .faceuse(up)?
            .mate
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let mut separating = faceuse_region(b, up)? != faceuse_region(b, mate)?;
        for (j, fu) in [up, mate].into_iter().enumerate() {
            let mut do_area = false;
            let mut do_volume = false;
            if behavior.face_volume == 1 {
                separating = false;
            }
            if !separating && mass.y > EFF_ZERO && j == 0 {
                do_area = true;
            }
            match behavior.face_area {
                0 => {
                    let other = b
                        .faceuse(fu)?
                        .mate
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    if !region_void(b, faceuse_region(b, fu)?)?
                        && region_void(b, faceuse_region(b, other)?)?
                    {
                        do_area = true;
                    }
                }
                1 => {
                    if j == 0 {
                        do_area = true;
                    }
                }
                2 => do_area = true,
                _ => {}
            }
            if behavior.face_volume == 0 && !region_void(b, faceuse_region(b, fu)?)? {
                do_volume = true;
            }
            if !(do_area || do_volume) {
                continue;
            }
            if separating || mass.y > EFF_ZERO {
                let orientation = b.faceuse(fu)?.orientation;
                let delta = face_properties(
                    k,
                    b,
                    f,
                    orientation,
                    accuracy,
                    origin,
                    estimated,
                    if separating { 0.0 } else { mass.y },
                    PPF_ALL,
                )?;
                if separating && k.fixes.brep_area_between_solid_and_void {
                    // Not upstream (WR-535, the fork's source fork@d2d9586): the
                    // faceuse's volume and mass enter only when it bounds a non void region,
                    // its area and area moments only when the area behaviour selects it: a
                    // face between two solid regions bounds both volumes but is no part of
                    // the area
                    add_selected(&mut total, &delta, mass.x, do_volume, do_area);
                } else {
                    add(&mut total, &delta, mass.x);
                }
                if !separating {
                    break;
                }
            }
        }
    }
    for e in edges(b)? {
        if !edge::is_wire(b, e)? {
            continue;
        }
        let mut mass = attributes.mass_vector(
            EntityRef::Edge(e),
            Vec3::new(behavior.density, behavior.edge_radius, 0.0),
        )?;
        let delta = edge_properties(k, b, e, accuracy, origin, mass.y)?;
        if mass.x < EFF_ZERO {
            mass.x = 1.0;
        }
        add(&mut total, &delta.properties, mass.x);
    }
    for v in vertices(b)? {
        if !vertex::is_shell_vertex(b, v)? {
            continue;
        }
        let mut mass = attributes.mass_vector(
            EntityRef::Vertex(v),
            Vec3::new(behavior.density, behavior.vertex_radius, 0.0),
        )?;
        if mass.x < EFF_ZERO {
            mass.x = 1.0;
        }
        let volume = 4.0 * PI * mass.y.powf(3.0) / 3.0;
        let area = 4.0 * PI * mass.y * mass.y;
        total.volume += volume;
        total.area += area;
        total.mass += volume * mass.x;
        // Upstream uses world coordinates here even when the computation origin differs.
        let p = b.vertex(v)?.point;
        let [m0, m1, m2, m3, m4, m5, m6, m7] = &mut total.moments;
        *m0 += area * p;
        *m1 += area * Vec3::new(p.x * p.x, p.y * p.y, p.z * p.z);
        m2.x += area * p.y * p.z;
        m2.y += area * p.z * p.x;
        m2.z += area * p.x * p.y;
        m3.x += area * (p.y * p.y + p.z * p.z);
        m3.y += area * (p.x * p.x + p.z * p.z);
        m3.z += area * (p.x * p.x + p.y * p.y);
        *m4 += volume * p * mass.x;
        *m5 += volume * mass.x * Vec3::new(p.x * p.x, p.y * p.y, p.z * p.z);
        m6.x += volume * mass.x * p.y * p.z;
        m6.y += volume * mass.x * p.z * p.x;
        m6.z += volume * mass.x * p.x * p.y;
        m7.x += volume * mass.x * (p.y * p.y + p.z * p.z);
        m7.y += volume * mass.x * (p.x * p.x + p.z * p.z);
        m7.z += volume * mass.x * (p.x * p.x + p.y * p.y);
    }
    Ok(total)
}
/// `SmApiEdgeComputeLength` (the curve's virtual Length). The edge's curve is owned by
/// the edge, so its zone tolerance (`SmTol::GetZoneTol3d(pCurve)`) is the edge's.
pub fn edge_length(k: &Kernel, b: &Brep, e: EdgeId, accuracy: f64) -> Result<f64> {
    let c = edge::curve(b, e)?.ok_or_else(|| SolidError::sm_err(OP))?;
    let owner_zone_tol_3d = if c.owner().is_some() {
        Some(edge::tolerance(b, e)?)
    } else {
        None
    };
    c.length_with_zone_tol(k, &edge::interval(b, e)?, accuracy, owner_zone_tol_3d)
}
fn area_estimate(bbox: &Extent3d) -> f64 {
    let s = bbox.size();
    let d = s.x.max(s.y).max(s.z);
    d * d / 100.0
}
/// `SmApiFaceComputeArea`: rejects a non-finite accuracy and integrates only area.
pub fn face_area(k: &Kernel, b: &mut Brep, f: FaceId, accuracy: f64) -> Result<f64> {
    valid_api_number(accuracy)?;
    let bbox = face_box(k, b, f)?;
    Ok(face_properties(
        k,
        b,
        f,
        OrientType::Same,
        accuracy,
        bbox.mid(),
        area_estimate(&bbox),
        0.0,
        PPF_AREA,
    )?
    .area
    .abs())
}
/// `SmApiFaceComputeCentroid`: rejects a non-finite accuracy; one area/first-moment
/// integration.
pub fn face_centroid(k: &Kernel, b: &mut Brep, f: FaceId, accuracy: f64) -> Result<(f64, Point3)> {
    valid_api_number(accuracy)?;
    let bbox = face_box(k, b, f)?;
    let origin = bbox.mid();
    let p = face_properties(
        k,
        b,
        f,
        OrientType::Same,
        accuracy,
        origin,
        area_estimate(&bbox),
        0.0,
        PPF_AREA | PPF_AREA_FIRST_MOMENTS,
    )?;
    if p.area.abs() < EFF_ZERO {
        return Err(SolidError::invalid_input(OP, "zero area"));
    }
    Ok((
        p.area.abs(),
        origin + *p.moments.first().ok_or_else(|| SolidError::sm_err(OP))? / p.area,
    ))
}
/// `SmApiBrepComputeArea`, including open shells and sheets.
pub fn brep_area(k: &Kernel, b: &mut Brep, accuracy: f64) -> Result<f64> {
    valid_api_number(accuracy)?;
    let mut area = 0.0;
    for f in faces(b)? {
        area += face_area(k, b, f, accuracy)?;
    }
    Ok(area)
}
/// `SmApiBrepComputeVolume`, computed about the bounding-box midpoint.
pub fn brep_volume(
    k: &Kernel,
    b: &mut Brep,
    accuracy: f64,
    attributes: &PropertyAttributes<'_>,
) -> Result<f64> {
    valid_api_number(accuracy)?;
    let bbox = qbrep::calculate_bounding_box(k, &mut *b, BoundingBoxOptions::default())?;
    Ok(brep_properties(
        k,
        b,
        accuracy,
        bbox.mid(),
        MassPropertyBehavior::default(),
        attributes,
    )?
    .volume)
}
/// Result of `SmApiBrepComputeMassProperties`.
#[derive(Debug, Clone, PartialEq)]
pub struct MassProperties {
    /// Precise measures and density-weighted moment vectors.
    pub properties: PreciseProperties,
    /// Center of mass in world coordinates.
    pub centroid: Point3,
    /// Mass axial inertia about the caller's origin.
    pub inertia: Vec3,
    /// Raw positive products yz, zx, xy about that origin.
    pub products: Vec3,
}
fn valid_api_number(value: f64) -> Result<()> {
    if !value.is_finite() || value.abs() > BIG_DOUBLE / 100.0 {
        Err(SolidError::invalid_input(OP, "invalid numeric argument"))
    } else {
        Ok(())
    }
}
/// `SmApiBrepComputeMassProperties`, with upstream validation: integrated once about the
/// bounding-box midpoint, the moments then shifted to `origin` (which avoids
/// cancellation for a distant origin).
pub fn brep_mass_properties(
    k: &Kernel,
    b: &mut Brep,
    accuracy: f64,
    density: f64,
    origin: Point3,
    attributes: &PropertyAttributes<'_>,
) -> Result<MassProperties> {
    valid_api_number(accuracy)?;
    valid_api_number(density)?;
    if density < EFF_ZERO {
        return Err(SolidError::invalid_input(OP, "density below epsilon"));
    }
    for v in [origin.x, origin.y, origin.z] {
        valid_api_number(v)?;
    }
    if faces(b)?
        .into_iter()
        .any(|f| attributes.has_mass(EntityRef::Face(f)))
        || edges(b)?
            .into_iter()
            .any(|e| attributes.has_mass(EntityRef::Edge(e)))
        || vertices(b)?
            .into_iter()
            .any(|v| attributes.has_mass(EntityRef::Vertex(v)))
    {
        return Err(SolidError::invalid_input(
            OP,
            "per-entity mass attributes override density",
        ));
    }
    if !manifold_solid(b)? {
        return Err(SolidError::invalid_input(OP, "not a manifold solid"));
    }
    let mid = qbrep::calculate_bounding_box(k, &mut *b, BoundingBoxOptions::default())?.mid();
    let properties = brep_properties(
        k,
        b,
        accuracy,
        mid,
        MassPropertyBehavior {
            density,
            ..MassPropertyBehavior::default()
        },
        attributes,
    )?;
    valid_api_number(properties.mass)?;
    valid_api_number(properties.volume)?;
    if properties.mass < EFF_ZERO || properties.volume < EFF_ZERO {
        return Err(SolidError::invalid_input(OP, "nonpositive mass or volume"));
    }
    // [4] = static moment S about the midpoint; [7] = moments of inertia
    // (y^2+z^2, ...); [6] = products (yz, zx, xy) as raw positive integrals.
    let moment = |i: usize| {
        properties
            .moments
            .get(i)
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP))
    };
    let static_moment = moment(4)?;
    // Shift to origin: with d = mid - origin, the integral of (a+d_a)(b+d_b) dm gains
    // d_a S_b + d_b S_a + d_a d_b m.
    let d = mid - origin;
    let comp = |v: Vec3, i: usize| match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    };
    let mass = properties.mass;
    let shift = |a: usize, b: usize| {
        comp(d, a) * comp(static_moment, b)
            + comp(d, b) * comp(static_moment, a)
            + comp(d, a) * comp(d, b) * mass
    };
    let inertia = moment(7)?
        + Vec3::new(
            shift(1, 1) + shift(2, 2),
            shift(0, 0) + shift(2, 2),
            shift(0, 0) + shift(1, 1),
        );
    let products = moment(6)? + Vec3::new(shift(1, 2), shift(2, 0), shift(0, 1));
    let centroid = mid + static_moment / mass;
    Ok(MassProperties {
        properties,
        centroid,
        inertia,
        products,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::{Attribute, Behavior};

    #[test]
    fn vector_attribute_subclasses_supply_mass_values() {
        let value = Vec3::new(2.0, 0.3, 0.0);
        for attribute in [
            Attribute::vector3d(ids::MASS_PROPERTIES, value, Behavior::Copy),
            Attribute::color(ids::MASS_PROPERTIES, value, Behavior::Copy),
        ] {
            let mut store = AttributeStore::default();
            let key = ObjectKey(7);
            let handle = store.insert(attribute).unwrap();
            store.add_attribute(key, handle).unwrap();
            let attrs = PropertyAttributes {
                store: &store,
                key_of: &|_| key,
            };
            assert_eq!(
                attrs
                    .mass_vector(EntityRef::Edge(EdgeId::from_index(0)), Vec3::ZERO)
                    .unwrap(),
                value
            );
        }
    }
    #[test]
    fn face_area_makes_missing_uv_trims_on_the_callers_brep() {
        // Upstream's `SmFace::ComputePreciseProperties` builds its oriented trim boundary on
        // the face itself, so the UV trims it makes stay on the B-rep.
        let mut k = Kernel::default();
        let mut b =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 1.0, 1.0, 1.0)
                .unwrap();
        let uses: Vec<_> = b.edgeuses().ids().collect();
        for u in uses {
            b.edgeuse_mut(u).unwrap().uv_trim_curve = None;
        }
        let f = *faces(&b).unwrap().first().unwrap();
        let area = face_area(&k, &mut b, f, 1e-6).unwrap();
        assert!((area - 1.0).abs() <= 1e-9, "{area}");
        assert!(
            b.edgeuses().iter().any(|(_, u)| u.uv_trim_curve.is_some()),
            "the trims are made on the box itself"
        );
    }
    /// WR-535 (the fork's source fork@d2d9586): the merge of two overlapping 10
    /// mm cubes has three solid regions; its area counts only the faces between solid
    /// and void (800, not 1200 with the two inner faces counted from both sides), and
    /// its volume is the union's.
    #[test]
    fn merged_cubes_area_counts_solid_void_faces_only() {
        use crate::primitive::api_primitives::create_box;
        let mut k = Kernel::default();
        k.fixes.brep_area_between_solid_and_void = true;
        let a = create_box(&mut k, Point3::ZERO, 10.0, 10.0, 10.0).unwrap();
        let b = create_box(&mut k, Point3::new(5.0, 0.0, 0.0), 10.0, 10.0, 10.0).unwrap();
        let mut merged = crate::boolean::api_brep::boolean_merge(&mut k, a, b).unwrap();
        let store = AttributeStore::default();
        let attrs = PropertyAttributes {
            store: &store,
            key_of: &|_| ObjectKey(0),
        };
        let p = brep_properties(
            &k,
            &mut merged,
            1e-6,
            Point3::ZERO,
            MassPropertyBehavior::default(),
            &attrs,
        )
        .unwrap();
        assert!((p.area - 800.0).abs() <= 1e-6, "area {}", p.area);
        assert!((p.volume - 1500.0).abs() <= 1e-6, "volume {}", p.volume);
    }
}
