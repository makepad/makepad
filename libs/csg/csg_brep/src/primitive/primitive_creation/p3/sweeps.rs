//! Linear and rotational primitive sweep control.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_repeated_linear_sweep`] | seven-argument `CreateLinearSweep` |
//! | [`PrimitiveCreation::create_rotational_sweep`] | full `CreateRotationalSweep` |
//! | [`PrimitiveCreation::create_partial_rotational_sweep`] | partial `CreateRotationalSweep` |
//!
//! Deviations: typed services and owned curve slots replace pointers; null curve
//! dereferences become errors. The existing sweep engine bounds repetitions to
//! 4096. No additional recursion. Unavailable calls retain their full caller tails.
//! Not upstream (WR-409, source issue #1113, opt-in
//! `Kernel::fixes.sheet_sweep_joins_profile_wires`): a profile that bounds no planar
//! face becomes wires joined where its curves meet (`join_profile_wire_ends`).
//! Not upstream (WR-442, source issue #478, opt-in
//! `Kernel::fixes.revolve_nested_shells_as_cavities`): after a full turn's
//! stitch, a closed shell inside another becomes that solid's cavity.
//! Not upstream (WR-448, source issue #545, opt-in
//! `Kernel::fixes.revolve_analytic_profile_faces`; source change, source fork
//! 0b143f7): upstream makes analytic faces only for a one-curve profile. With the fix
//! a whole turn gives each other curve its cylinder, cone, sphere or torus
//! ([`analytic_revolution`]) before falling back to the `SmSurfOfRevolution`, and a
//! partial turn sweeps with [`RotationalSweepGeometry::set_analytic_revolutions`] on.
//! With the fix a whole turn also skips the one-curve special case
//! (`sm_CreateAnalyticRotationalSweep`), as fork 9853e54 deletes it: a one-curve profile
//! goes through the same per-curve rule, so a whole circle's torus has its seam at the
//! profile (#1491).

use super::{construct::target, geometry::bspline, PrimitiveAccess};
use crate::curve::curve::Curve;
use crate::curve::ellipse::Ellipse;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::geom_utility::{
    p1::{line_closest_point, line_point_distance},
    p2::ContinuityType,
};
use crate::math::{max4, Point3, Vec3, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::primitive::primitive_creation::p1::{analytic_rotational_sweep, PrimitiveCreation};
use crate::surface::surf_of_revolution::p1::SurfOfRevolution;
use crate::surface::surface::Surface;
use crate::sweep::analytic_revolution::analytic_revolution;
use crate::sweep::rotational_sweep_geometry::RotationalSweepGeometry;
use crate::sweep::topology_sweep::{
    engine::SweepTargets, geometry::SweepGeometry, SweepEntity, TopologySweep,
};
use crate::sweep::trans_sweep_geometry::TranslationalSweepGeometry;
use crate::topology::{BrepId, FaceId};
const OP: &str = "primitive::primitive_creation::p3::sweeps";

/// Flags shared by the repeated linear and partial rotational sweeps.
#[derive(Debug, Clone, Copy)]
pub struct RepeatedSweepOptions {
    /// Number of incremental sweeps, including zero as accepted upstream.
    pub repetitions: u64,
    /// Create end caps on the first and last profiles; capped sweeps require at least
    /// one repetition.
    pub cap_ends: bool,
    /// Split inputs at C1 discontinuities (default true).
    pub test_continuity: bool,
}
impl Default for RepeatedSweepOptions {
    fn default() -> Self {
        Self {
            repetitions: 1,
            cap_ends: false,
            test_continuity: true,
        }
    }
}

fn split(a: &impl PrimitiveAccess, b: BrepId, curves: &mut Vec<Option<Curve>>) -> Result<()> {
    let mut index = 0;
    while let Some(slot) = curves.get(index) {
        let curve = slot.as_ref().ok_or_else(|| SolidError::null_pointer(OP))?;
        let curve = bspline(curve)
            .ok_or_else(|| SolidError::invalid_input(OP, "non B-spline continuity input"))?;
        let pieces =
            ignored(curve.subdivide_at_discontinuity(a.kernel(b)?, ContinuityType::C1, 0.5))?
                .unwrap_or_default();
        if pieces.len() > 1 {
            let count = pieces.len();
            curves.splice(
                index..=index,
                pieces.iter().cloned().map(|c| Some(Curve::from(c))),
            );
            index += count;
        } else {
            index += 1;
        }
    }
    Ok(())
}
fn entity(b: BrepId, item: EntityRef) -> Result<SweepEntity> {
    match item {
        EntityRef::Region(i) => Ok(SweepEntity::Region(b, i)),
        EntityRef::Face(i) => Ok(SweepEntity::Face(b, i)),
        EntityRef::Edge(i) => Ok(SweepEntity::Edge(b, i)),
        EntityRef::Vertex(i) => Ok(SweepEntity::Vertex(b, i)),
        _ => Err(SolidError::invalid_input(
            OP,
            "non-sweep entity in primitive map",
        )),
    }
}
fn copy_maps(p: &PrimitiveCreation, b: BrepId, sweep: &mut TopologySweep) -> Result<()> {
    for (input, output) in [
        (&p.maps.to_same, &mut sweep.to_same),
        (&p.maps.to_higher, &mut sweep.to_higher),
        (&p.maps.from_same, &mut sweep.from_same),
        (&p.maps.from_lower, &mut sweep.from_lower),
    ] {
        output.clear();
        for (&from, &to) in input {
            output.insert(Some(entity(b, from)?), Some(entity(b, to)?));
        }
    }
    Ok(())
}
fn targets(
    b: BrepId,
    faces: &[FaceId],
    edges: &[crate::topology::EdgeId],
    cap: bool,
) -> SweepTargets {
    if cap {
        SweepTargets {
            faces_higher: Some(faces.iter().map(|&f| Some((b, f))).collect()),
            ..Default::default()
        }
    } else {
        SweepTargets {
            edges_higher: Some(edges.iter().map(|&e| Some((b, e))).collect()),
            ..Default::default()
        }
    }
}
/// Not upstream (WR-409, source issue #1113, opt-in
/// `Kernel::fixes.sheet_sweep_joins_profile_wires`; the source fork's
/// `sm_CreateJoinedWireEdges`): the profile's wire edges `edges`, in the curves' order,
/// joined where they meet. An end within the B-rep's tolerance of an earlier wire's end
/// is glued to that earlier vertex (`GlueVertices`), so two curves meeting at a point
/// share one vertex there. Without it the topology sweep's copies of two coincident
/// vertices came out coincident, each with a preimage, and its stitch refused them: a
/// partial revolve or repeated linear sweep of an open profile of more than one curve
/// failed. Nothing is done without the opt-in.
///
/// # Errors
///
/// Those of the vertex queries and of [`crate::topology::brep::p5::glue_vertices`].
fn join_profile_wire_ends(
    a: &mut impl PrimitiveAccess,
    brep: BrepId,
    edges: &[crate::topology::EdgeId],
) -> Result<()> {
    use crate::topology::{edge, vertex, VertexId};
    if !a.kernel(brep)?.fixes.sheet_sweep_joins_profile_wires {
        return Ok(());
    }
    a.with_primitive_brep(brep, |k, b| {
        let tol = b.tolerance();
        // the distinct vertices of the wires so far
        let mut vertices: Vec<VertexId> = Vec::new();
        for &e in edges {
            let start = edge::start_vertex(b, e)?;
            let end = edge::other_vertex(b, e, start)?;
            for (kk, v) in [start, end].into_iter().enumerate() {
                let Some(v) = v else {
                    continue;
                };
                if kk == 1 && Some(v) == start {
                    continue;
                }
                let point = vertex::point(b, v)?;
                let mut found = None;
                for &w in &vertices {
                    if vertex::point(b, w)?.distance(point) <= tol {
                        found = Some(w);
                        break;
                    }
                }
                match found {
                    Some(w) => {
                        crate::topology::brep::p5::glue_vertices(k, b, Some(w), Some(v), None)?;
                    }
                    None => vertices.push(v),
                }
            }
        }
        Ok(())
    })
}

impl PrimitiveCreation {
    /// Repeated linear sweep, with the active upstream analytic fast path.
    pub fn create_repeated_linear_sweep(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        curves: &mut Vec<Option<Curve>>,
        direction: Vec3,
        distance: f64,
        options: RepeatedSweepOptions,
    ) -> Result<()> {
        self.init_entity_maps();
        if options.cap_ends && options.repetitions == 0 {
            return Err(SolidError::sm_err(OP));
        }
        let region = target(self, brep)?;
        let tolerance = a.brep(brep)?.tolerance();
        a.set_editing(brep, true)?;
        let previous = a.kernel(brep)?.context.doing_boolean();
        a.kernel_mut(brep)?.context.set_doing_boolean(true);
        let result = (|| {
            if options.test_continuity {
                split(a, brep, curves)?;
            }
            if options.repetitions == 1 {
                // Active USE_NEW_ANALYTICS branch, including distance==0.
                let owned: Vec<_> = curves.iter_mut().filter_map(Option::take).collect();
                return a.with_primitive_brep(brep, |k, b| {
                    self.create_linear_sweep(k, b, owned, direction, distance, options.cap_ends)
                });
            }
            let mut faces = Vec::new();
            let mut edges = Vec::new();
            // the profile the end cap is copied from (the planar faces consume the curves)
            let profile: Vec<Curve> = if options.cap_ends {
                curves.iter().flatten().cloned().collect()
            } else {
                Vec::new()
            };
            if let Some(()) = ignored(a.planar_faces(region, curves, tolerance, &mut faces))? {
                for &face in &faces {
                    edges.extend(a.face_edges((brep, face))?);
                }
            } else {
                for slot in curves.iter_mut() {
                    if slot.is_none() {
                        return Err(SolidError::sm_err(OP));
                    }
                    let Some(edge) = ignored(a.wire(brep, slot))? else {
                        return Err(SolidError::sm_err(OP));
                    };
                    edges.push(edge);
                }
                join_profile_wire_ends(a, brep, &edges)?;
                faces.clear();
            }
            if distance == 0.0 {
                return Ok(());
            }
            // without planar faces there is nothing to cap
            let cap = options.cap_ends && !faces.is_empty();

            // Each step is distance along the unit direction; a zero vector is rejected.
            let sweep_dir = direction.unitize()?;

            let mut translation = TranslationalSweepGeometry::new(distance * sweep_dir);
            translation.base.eps = tolerance;
            let step = translation.sweep_transform();
            let geometry = SweepGeometry::Translation(translation);
            let mut sweep = TopologySweep {
                do_tagging: self.tag_id() > 0,
                tag_id: self.tag_id(),
                repetitions: options.repetitions,
                ..Default::default()
            };

            // when asked and it's possible - build the end cap: the profile moved by
            // every step
            if cap {
                let mut end_curves = Vec::new();
                for curve in &profile {
                    let mut end_curve = curve.copy(a.kernel(brep)?)?;
                    for _ in 0..options.repetitions {
                        ignore_status(end_curve.transform(a.kernel(brep)?, &step, None))?;
                    }
                    end_curves.push(Some(end_curve));
                }
                let mut end_faces = Vec::new();
                a.planar_faces(region, &mut end_curves, tolerance, &mut end_faces)?;
                sweep.do_stitching = true; // stitch the sides to the caps
            }

            let mut selected = targets(brep, &faces, &edges, false);
            sweep.do_sweep(a, &geometry, Some(brep), Some(region), &mut selected)?;

            if cap {
                // both caps have the profile's orientation
                a.orient(brep, true, false)?;
            } else {
                // unrelated clean up - we didn't use the tmp faces (if any - delete them)
                for face in faces {
                    ignore_status(a.delete_primitive_face((brep, face)))?;
                }
            }
            copy_maps(self, brep, &mut sweep)?;
            a.set_editing(brep, false)?;
            Ok(())
        })();
        a.kernel_mut(brep)?.context.set_doing_boolean(previous);
        result
    }

    /// Full rotation: prefer analytic surfaces, then circular planes or revolutions.
    /// Failed planar face construction still consumes that source curve upstream.
    pub fn create_rotational_sweep(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        curves: &mut [Option<Curve>],
        axis_base: Point3,
        axis: Vec3,
    ) -> Result<()> {
        self.init_entity_maps();
        let _ = target(self, brep)?;
        a.set_editing(brep, true)?;
        // Not upstream (#1491, with `fixes.revolve_analytic_profile_faces`): fork
        // 9853e54 deletes the one-curve special case (sm_CreateAnalyticRotationalSweep),
        // whose whole-circle torus puts its seam a quarter turn from the profile; the
        // per-curve rule below covers a one-curve profile too
        let one_curve_special_case = !a.kernel(brep)?.fixes.revolve_analytic_profile_faces;
        if let (true, [Some(curve)]) = (one_curve_special_case, &mut *curves) {
            if ignored(a.with_primitive_brep(brep, |k, b| {
                analytic_rotational_sweep(k, b, curve, axis_base, axis)
            }))?
            .is_some()
            {
                if let Some(slot) = curves.first_mut() {
                    *slot = None;
                }
                return Ok(());
            }
        }
        for slot in curves.iter_mut() {
            let curve = slot.as_ref().ok_or_else(|| SolidError::null_pointer(OP))?;
            let length = ignored(curve.length(a.kernel(brep)?, &curve.natural_interval(), 0.001))?
                .unwrap_or(0.0);
            if length < EFF_ZERO {
                return Err(SolidError::invalid_input(OP, "degenerate revolved curve"));
            }
            let ends = curve.ends(a.kernel(brep)?)?;
            let (start_dist, _) = line_point_distance(axis_base, axis, ends.start);
            let (end_dist, _) = line_point_distance(axis_base, axis, ends.end);
            let length_scale = max4(start_dist, end_dist, length, EFF_ZERO_SQRT);
            let angle_tol = a.brep(brep)?.tolerance() / length_scale * (180.0 / PI);
            let mut face = None;
            if curve.is_linear(a.kernel(brep)?, EFF_ZERO, false)?.linear {
                let mut tangent = ends.end - ends.start;
                tangent /= tangent.length();
                if tangent.is_perpendicular_to(axis, angle_tol) {
                    let zero = crate::geom::tol::scaled_zero2(&ends.start, &ends.end);
                    let param = line_closest_point(axis_base, axis, ends.start);
                    let origin = axis_base + param * axis;
                    if start_dist > end_dist {
                        tangent *= -1.0;
                    }
                    let mut frame = Axis2Placement::IDENTITY;
                    ignore_status(frame.set_canonical(origin, tangent, axis.cross(tangent)))?;
                    let mut circles = Vec::new();
                    if start_dist > zero {
                        circles.push(Curve::from(Ellipse::create_canonical_circle(
                            a.kernel(brep)?,
                            &frame,
                            start_dist,
                            None,
                            None,
                        )?));
                    }
                    if end_dist > zero {
                        circles.push(Curve::from(Ellipse::create_canonical_circle(
                            a.kernel(brep)?,
                            &frame,
                            end_dist,
                            None,
                            None,
                        )?));
                    }
                    let infinite = a
                        .brep(brep)?
                        .infinite_region()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let tolerance = a.brep(brep)?.tolerance();
                    ignore_status(a.planar_face((brep, infinite), circles, tolerance, &mut face))?;
                    *slot = None;
                }
            }
            if face.is_none() {
                if let Some(curve) = slot.as_ref() {
                    // Not upstream (WR-448, #545): the curve's cylinder, cone,
                    // sphere or torus, else the surface of revolution
                    let analytic = if a.kernel(brep)?.fixes.revolve_analytic_profile_faces {
                        let tolerance = a.brep(brep)?.tolerance();
                        analytic_revolution(
                            a.kernel(brep)?,
                            curve,
                            axis_base,
                            axis,
                            360.0,
                            tolerance,
                            None,
                        )?
                    } else {
                        None
                    };
                    let rev = match analytic {
                        Some(surface) => Some(surface),
                        None => ignored(SurfOfRevolution::create_canonical(
                            a.kernel(brep)?,
                            curve.clone(),
                            axis_base,
                            axis,
                            false,
                        ))?
                        .map(Surface::from),
                    };
                    let mut success = false;
                    if let Some(surface) = rev {
                        let domain = surface.natural_uv_domain_with_nurb(a.kernel(brep)?)?;
                        success =
                            ignored(a.surface_face(brep, surface, domain, &mut face))?.is_some();
                    }
                    if face.is_none() || !success {
                        *slot = None;
                    }
                }
            }
        }
        ignore_status(a.stitch_and_orient(brep))?;
        // not upstream (WR-442, #478, opt-in): a closed shell inside another is its cavity
        if a.kernel(brep)?.fixes.revolve_nested_shells_as_cavities {
            a.with_brep_mut(brep, |k, b, _, env| {
                crate::topology::brep::p6::nest_closed_shells_as_cavities(k, b, env)
            })?;
        }
        a.set_editing(brep, false)?;
        Ok(())
    }

    /// Partial rotation and its full-revolution dispatch, axis-interior rejection,
    /// sweep, ordered face deletion and callback stitching.
    pub fn create_partial_rotational_sweep(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        curves: &mut Vec<Option<Curve>>,
        axis_base: Point3,
        axis: Vec3,
        angle_deg: f64,
        options: RepeatedSweepOptions,
    ) -> Result<()> {
        self.init_entity_maps();
        let region = target(self, brep)?;
        let previous = a.kernel(brep)?.context.doing_boolean();
        a.kernel_mut(brep)?.context.set_doing_boolean(true);
        let result = (|| {
            if options.test_continuity {
                split(a, brep, curves)?;
            }
            if angle_deg >= 359.99999 {
                return self.create_rotational_sweep(a, brep, curves, axis_base, axis);
            }
            if options.cap_ends && options.repetitions != 1 {
                return Err(SolidError::sm_err(OP));
            }
            let tolerance = a.brep(brep)?.tolerance();
            a.set_editing(brep, true)?;
            let mut faces = Vec::new();
            let mut edges = Vec::new();
            if ignored(a.planar_faces(region, curves, tolerance, &mut faces))?.is_none() {
                for slot in curves.iter_mut() {
                    let Some(edge) = ignored(a.wire(brep, slot))? else {
                        return Err(SolidError::sm_err(OP));
                    };
                    edges.push(edge);
                }
                join_profile_wire_ends(a, brep, &edges)?;
                faces.clear();
            }
            let base = faces.first().copied();
            for &face in &faces {
                edges.extend(a.face_edges((brep, face))?);
            }
            let mut cap = options.cap_ends;
            if let Some(face) = base {
                let bound = a.primitive_face_box((brep, face))?.sphere_bound();
                let direction = axis.unitize()?;
                let param = line_closest_point(axis_base, direction, bound.center);
                let point = axis_base + param * direction;
                let start = point - 2.0 * bound.radius * direction;
                let end = point + 2.0 * bound.radius * direction;
                let axis_curve = Curve::from(BSplineCurve::create_line_segment(3, start, end)?);
                if a.axis_inside_face((brep, face), &axis_curve)? {
                    a.delete_primitive_faces(brep, &[face])?;
                    return Err(SolidError::Upstream {
                        operation: OP,
                        code: 1025,
                        entity: None,
                    });
                }
            } else {
                cap = false;
            }
            let mut rotation = RotationalSweepGeometry::new(axis_base, axis, angle_deg, tolerance);
            // Not upstream (WR-448, #545): analytic side faces
            rotation.set_analytic_revolutions(a.kernel(brep)?.fixes.revolve_analytic_profile_faces);
            let geometry = SweepGeometry::Rotation(rotation);
            let mut sweep = TopologySweep {
                repetitions: options.repetitions,
                ..Default::default()
            };
            if self.tag_id() > 0 {
                sweep.do_tagging = true;
                sweep.tag_id = self.tag_id();
            }
            let mut selected = targets(brep, &faces, &edges, cap);
            if cap {
                sweep.do_sweep(a, &geometry, Some(brep), Some(region), &mut selected)?;
            } else {
                sweep.do_stitching = false;
                sweep.do_sweep(a, &geometry, Some(brep), Some(region), &mut selected)?;
                ignore_status(a.delete_primitive_faces(brep, &faces))?;
                a.stitch(brep, &mut sweep, tolerance, false)?;
            }
            copy_maps(self, brep, &mut sweep)?;
            a.set_editing(brep, false)?;
            Ok(())
        })();
        a.kernel_mut(brep)?.context.set_doing_boolean(previous);
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::curve::curve::Curve;
    use crate::kernel::Kernel;
    use crate::math::{Point3, Vec3};
    use crate::primitive::api_curves;
    use crate::sweep::api_brep::create_rotational_sweep;

    fn segment(a: [f64; 3], b: [f64; 3]) -> Curve {
        let [ax, ay, az] = a;
        let [bx, by, bz] = b;
        Curve::from(
            api_curves::create_line_segment(Point3::new(ax, ay, az), Point3::new(bx, by, bz))
                .expect("segment"),
        )
    }

    /// A full turn of a half annulus about its diameter is a hollow ball with the
    /// opt-in; upstream makes the inner ball a second solid.
    #[test]
    fn issue_478_full_turn_nested_shell_is_a_cavity() {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};

        let (o, x) = (Point3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let arc = |r: f64| Curve::from(api_curves::create_arc(o, r, 0.0, 180.0).unwrap());
        let profile = [
            segment([18.5, 0.0, 0.0], [40.0, 0.0, 0.0]),
            arc(40.0),
            segment([-40.0, 0.0, 0.0], [-18.5, 0.0, 0.0]),
            arc(18.5),
        ];
        let curves: Vec<Option<&Curve>> = profile.iter().map(Some).collect();

        let mut k = Kernel::default();
        k.fixes.revolve_nested_shells_as_cavities = true;
        let mut ball = create_rotational_sweep(&mut k, &curves, o, x, 360.0, true).unwrap();
        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        let volume = brep_volume(&k, &mut ball, 1e-6, &attributes).unwrap();
        // 4/3 pi (40^3 - 18.5^3)
        let want = 4.0 / 3.0 * std::f64::consts::PI * (40.0_f64.powi(3) - 18.5_f64.powi(3));
        assert!(
            (volume - want).abs() < 1e-3 * want,
            "volume {volume}, want {want}"
        );
        assert!(crate::topology::brep::p2::is_manifold_solid(&ball).unwrap());

        // WR-443: a difference with the hollow ball as the tool keeps what lies in the
        // hollow. A block wholly inside it keeps its 512; one crossing the inner sphere
        // loses only its part outside r 18.5 (the reference's sphere-difference control).
        for (corner, size, want) in [
            ([-4.0, -4.0, -4.0], 8.0, 512.0),
            ([8.0, -4.0, -4.0], 12.0, 653.342_908_027_093_8),
        ] {
            let [x0, y0, z0] = corner;
            let block = crate::primitive::api_primitives::create_box(
                &mut k,
                Point3::new(x0, y0, z0),
                size,
                8.0,
                8.0,
            )
            .unwrap();
            let mut cut = crate::boolean::api_brep::boolean_difference(
                &mut k,
                block,
                ball.clone_standing_for_self().unwrap(),
            )
            .unwrap();
            let got = brep_volume(&k, &mut cut, 1e-8, &attributes).unwrap();
            assert!(
                (got - want).abs() < 1e-6 * want,
                "block at {corner:?} minus the hollow ball: volume {got}, want {want}"
            );
        }
    }

    /// A square with a square hole (20 x 20 at r 10 to 30, the hole 8 x 8 centred in it)
    /// turned a whole turn about z is a ring with a ring-shaped void: with the opt-ins
    /// (WR-529, the fork's source fork@38b4212) the void is not counted as
    /// material, and the volume is Pappus's, area 336 about the centroid at r 20.
    #[test]
    fn holed_profile_full_turn_leaves_the_hole_a_void() {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};

        let square = |x0: f64, z0: f64, side: f64| {
            let (x1, z1) = (x0 + side, z0 + side);
            [
                segment([x0, 0.0, z0], [x1, 0.0, z0]),
                segment([x1, 0.0, z0], [x1, 0.0, z1]),
                segment([x1, 0.0, z1], [x0, 0.0, z1]),
                segment([x0, 0.0, z1], [x0, 0.0, z0]),
            ]
        };
        let mut profile = square(10.0, 0.0, 20.0).to_vec();
        profile.extend(square(16.0, 6.0, 8.0));
        let curves: Vec<Option<&Curve>> = profile.iter().map(Some).collect();
        let (o, z) = (Point3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));

        let mut k = Kernel::default();
        k.fixes.revolve_nested_shells_as_cavities = true;
        k.fixes.revolve_holed_profile_void = true;
        let mut ring = create_rotational_sweep(&mut k, &curves, o, z, 360.0, true).unwrap();
        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        let volume = brep_volume(&k, &mut ring, 1e-6, &attributes).unwrap();
        let want = (400.0 - 64.0) * 2.0 * std::f64::consts::PI * 20.0;
        assert!(
            (volume - want).abs() < 1e-3 * want,
            "volume {volume}, want {want}"
        );
        let check = crate::topology::brep::validation::check_brep(
            &mut k,
            &mut ring,
            crate::topology::brep::validation::CheckLevel::Level2,
        )
        .unwrap();
        assert_eq!(check.failure_count(), 0, "{:?}", check.lines());
    }

    /// An open chain of two lines (out from r 6 to 8, then up 2) revolved half a turn into
    /// a sheet joins at its meeting vertex with the opt-in (WR-409): two faces, the flat
    /// ring sector and the cylinder, of area (8^2 - 6^2) pi / 2 + 8 pi 2.
    #[test]
    fn issue_1113_open_chain_revolves_into_a_joined_sheet() {
        use crate::mass::integrator::properties::{face_area, face_properties};
        use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
        use crate::topology::face::p2::PPF_AREA;

        let chain = [
            segment([6.0, 0.0, 0.0], [8.0, 0.0, 0.0]),
            segment([8.0, 0.0, 0.0], [8.0, 0.0, 2.0]),
        ];
        let curves: Vec<Option<&Curve>> = chain.iter().map(Some).collect();
        let (o, z) = (Point3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));

        let mut k = Kernel::default();
        k.fixes.sheet_sweep_joins_profile_wires = true;
        let mut sheet = create_rotational_sweep(&mut k, &curves, o, z, 180.0, false).unwrap();
        let faces = crate::topology::brep::p1::faces(&sheet, None)
            .unwrap()
            .len();
        assert_eq!(faces, 2);
        // Joined at (8, 0, 0): the profile's three vertices and their copies at 180
        // degrees (unjoined, the coincident ends are two vertices and the sweep fails).
        let vertices = crate::topology::brep::p1::vertices(&sheet, None)
            .unwrap()
            .len();
        assert_eq!(vertices, 6);
        let edges = crate::topology::brep::p1::edges(&sheet, None)
            .unwrap()
            .len();
        assert_eq!(edges, 7);
        // The area is 14 pi (the half annulus) plus 16 pi (the half cylinder), per face
        // through `face_properties` at accuracy 1e-9: `brep_area` / `face_area` clamp the
        // accuracy to [1e-4, 1e-1] (`SmApiFaceComputeArea`), which allows 7.7e-5 here.
        // The bound 1e-8 relative is ten times the integration accuracy.
        let pi = std::f64::consts::PI;
        let mut areas = Vec::new();
        for f in crate::topology::brep::p1::faces(&sheet, None).unwrap() {
            let est = face_area(&k, &mut sheet, f, 1e-4).unwrap();
            let a = face_properties(
                &k,
                &mut sheet,
                f,
                OrientType::Same,
                1e-9,
                Point3::new(0.0, 0.0, 0.0),
                est,
                0.0,
                PPF_AREA,
            )
            .unwrap()
            .area
            .abs();
            areas.push(a);
        }
        areas.sort_by(f64::total_cmp);
        for (got, want) in areas.iter().zip([14.0 * pi, 16.0 * pi]) {
            assert!(
                (got - want).abs() <= 1e-8 * want,
                "face area {got}, want {want}"
            );
        }
    }
}
