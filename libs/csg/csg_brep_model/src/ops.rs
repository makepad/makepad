//! Bodies: primitives, sweeps from sketches, booleans, blends, shells, placement.
//!
//! Frames match the solid graph: primitives are centred on the origin and stand along Y;
//! extrusions leave their sketch plane along its normal; a revolve turns about Y with
//! sketch u the radius and v the height.

use crate::guard::BrepError;
use crate::profile::{sketch_curves, Frame};
use makepad_csg_brep::blend::fillet_executive::session::FilletSession;
use makepad_csg_brep::blend::fillet_solver::p1::FilletSurfaceGeneratorType;
use makepad_csg_brep::boolean::api_brep::{boolean_difference, boolean_intersection, boolean_union};
use makepad_csg_brep::curve::curve::Curve;
use makepad_csg_brep::geom::attribute::ObjectKey;
use makepad_csg_brep::geom::axis2_placement::Axis2Placement;
use makepad_csg_brep::geom::object::ObjectRef;
use makepad_csg_brep::kernel::{Kernel, UpstreamFixes};
use makepad_csg_brep::math::{Point3, Vec3};
use makepad_csg_brep::nurbs::b_spline_curve::BSplineCurve;
use makepad_csg_brep::nurbs::crv_fit::p1::ParamMethod;
use makepad_csg_brep::offset::offset_executive::p2::{offset_brep_simple, shell_brep_simple, OffsetOptions, ShellOptions};
use makepad_csg_brep::primitive::api_curves;
use makepad_csg_brep::primitive::api_primitives as prim;
use makepad_csg_brep::primitive::primitive_creation::SweepOptions;
use makepad_csg_brep::query::api_queries::{brep_bounding_box, brep_is_manifold_solid};
use makepad_csg_brep::sweep::api_brep::{create_curve_sweep, create_linear_sweep, create_rotational_sweep};
use makepad_csg_brep::sweep::api_surfaces::create_skin_primitive;
use makepad_csg_brep::topology::brep::{mirror, p2, p3, p5};
use makepad_csg_brep::topology::{Brep, EdgeId, FaceId};
use makepad_csg_cad::Sketch;
use makepad_csg_graph::HoleKind;

/// An exact solid. Geometry lives in its arenas; a [`Kernel`] carries the tolerances.
pub type Body = Brep;

/// Which boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolMode {
    Union,
    Difference,
    Intersection,
}

/// A kernel with every correction to its reference behaviour switched on: a clear
/// failure beats a body that is not a solid.
pub fn new_kernel() -> Kernel {
    let mut k = Kernel::default();
    k.fixes = UpstreamFixes::all();
    // Remakes a shell's inner edges as analytic curves, on a path that needs surface caches
    // bound to faces the simple shell does not bind: marched edges are within tolerance.
    k.fixes.shell_edges_on_analytics_exact = false;
    k
}

fn p3d(p: [f64; 3]) -> Point3 {
    Point3::new(p[0], p[1], p[2])
}

fn v3d(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

fn input(m: impl Into<String>) -> BrepError {
    BrepError::Input(m.into())
}

/// A copy that fillets and booleans as the original does (a plain `Clone` loses the
/// owners of its geometry).
pub fn copy_body(k: &mut Kernel, b: &Body, serial: u64) -> Result<Body, BrepError> {
    let named = |entity, domain: u64| {
        let r = p3::topology_ref(0, entity);
        ObjectRef { type_code: r.type_code, key: ObjectKey(domain + r.type_code as u64 * 1_000_000 + (r.key.0 & 0xffff_ffff)) }
    };
    let target_key = |entity| named(entity, serial * 100_000_000);
    let mut state = p2::ArenaNotifyState::new(&Brep::new());
    let mut env = state.env(None);
    env.key_of = &target_key;
    Ok(p3::api_brep_copy(k, &mut env, Some(b), &|entity| named(entity, 0))?)
}

/// Moves a body: rotation in degrees about X, then Y, then Z, then a translation.
pub fn place(k: &mut Kernel, b: &mut Body, at: [f64; 3], rotate: [f64; 3]) -> Result<(), BrepError> {
    let (sx, cx) = rotate[0].to_radians().sin_cos();
    let (sy, cy) = rotate[1].to_radians().sin_cos();
    let (sz, cz) = rotate[2].to_radians().sin_cos();
    // Columns of Rz Ry Rx.
    let x = [cz * cy, sz * cy, -sy];
    let y = [cz * sy * sx - sz * cx, sz * sy * sx + cz * cx, cy * sx];
    let placement = Axis2Placement::new(p3d(at), v3d(x), v3d(y));
    p5::transform(k, b, &placement, None, false)?;
    Ok(())
}

/// Moves a body by a frame: X to `x`, Y to `y`, Z to their cross product (`x`, `y` unit and
/// square), then to `at`.
pub fn place_frame(k: &mut Kernel, b: &mut Body, at: [f64; 3], x: [f64; 3], y: [f64; 3]) -> Result<(), BrepError> {
    p5::transform(k, b, &Axis2Placement::new(p3d(at), v3d(x), v3d(y)), None, false)?;
    Ok(())
}

/// Scales a body about the origin by (x, y, z), uniform or not.
pub fn scale(k: &mut Kernel, b: &mut Body, factor: [f64; 3]) -> Result<(), BrepError> {
    p5::transform(k, b, &Axis2Placement::IDENTITY, Some(v3d(factor)), true)?;
    Ok(())
}

/// Mirrors a body in the plane through `center` with this normal.
pub fn mirrored(k: &mut Kernel, b: &mut Body, center: [f64; 3], normal: [f64; 3]) -> Result<(), BrepError> {
    let n = normal;
    let helper = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let x = [helper[1] * n[2] - helper[2] * n[1], helper[2] * n[0] - helper[0] * n[2], helper[0] * n[1] - helper[1] * n[0]];
    let y = [n[1] * x[2] - n[2] * x[1], n[2] * x[0] - n[0] * x[2], n[0] * x[1] - n[1] * x[0]];
    let plane = Axis2Placement::new(p3d(center), v3d(x), v3d(y));
    mirror::mirror(k, b, &plane)?;
    Ok(())
}

/// A box centred on the origin.
pub fn make_box(k: &mut Kernel, size: [f64; 3]) -> Result<Body, BrepError> {
    if size.iter().any(|s| *s <= 0.0) {
        return Err(input("a box needs a positive size"));
    }
    Ok(prim::create_box(k, p3d([-size[0] * 0.5, -size[1] * 0.5, -size[2] * 0.5]), size[0], size[1], size[2])?)
}

pub fn make_sphere(k: &mut Kernel, radius: f64) -> Result<Body, BrepError> {
    if radius <= 0.0 {
        return Err(input("a sphere needs a positive radius"));
    }
    Ok(prim::create_sphere(k, p3d([0.0; 3]), radius)?)
}

/// Along Y, centred.
pub fn make_cylinder(k: &mut Kernel, radius: f64, height: f64) -> Result<Body, BrepError> {
    if radius <= 0.0 || height <= 0.0 {
        return Err(input("a cylinder needs a positive radius and height"));
    }
    // The kernel's stands on the origin along +Z; X -> X, Z -> Y.
    let mut b = prim::create_cylinder(k, p3d([0.0, 0.0, -height * 0.5]), radius, height)?;
    place(k, &mut b, [0.0; 3], [-90.0, 0.0, 0.0])?;
    Ok(b)
}

/// Along Y, centred; `top` is the radius at +Y.
pub fn make_cone(k: &mut Kernel, radius: f64, top: f64, height: f64) -> Result<Body, BrepError> {
    if radius < 0.0 || top < 0.0 || (radius == 0.0 && top == 0.0) || height <= 0.0 {
        return Err(input("a cone needs a positive height and a positive radius at one end"));
    }
    let mut b = prim::create_cone(k, p3d([0.0, 0.0, -height * 0.5]), radius, top, height)?;
    place(k, &mut b, [0.0; 3], [-90.0, 0.0, 0.0])?;
    Ok(b)
}

/// In the XZ plane.
pub fn make_torus(k: &mut Kernel, radius: f64, tube: f64) -> Result<Body, BrepError> {
    if tube <= 0.0 || radius <= tube {
        return Err(input("a torus needs radius > tube > 0"));
    }
    let mut b = prim::create_torus(k, p3d([0.0; 3]), radius, tube)?;
    place(k, &mut b, [0.0; 3], [-90.0, 0.0, 0.0])?;
    Ok(b)
}

fn refs(curves: &[Curve]) -> Vec<Option<&Curve>> {
    curves.iter().map(Some).collect()
}

fn flat(chains: Vec<Vec<Curve>>) -> Vec<Curve> {
    chains.into_iter().flatten().collect()
}

/// Extrudes a sketch placed on `frame` along `dir` (unit) by `height`.
pub fn extrude(k: &mut Kernel, sketch: &Sketch, frame: &Frame, dir: [f64; 3], height: f64) -> Result<Body, BrepError> {
    if height <= 0.0 {
        return Err(input("an extrude needs a positive height"));
    }
    let curves = flat(sketch_curves(k, sketch, frame)?);
    Ok(create_linear_sweep(k, &refs(&curves), v3d(dir), height, true)?)
}

/// The prisms of a pull of flat faces `faces` of `b` by `by` (into the body below 0): each
/// face's outline (every loop, so a face with holes keeps them) swept along its outward normal,
/// with that normal and a point on the face.
pub fn pull_prisms(k: &mut Kernel, b: &mut Body, faces: &[FaceId], by: f64) -> Result<Vec<(Body, [f64; 3], [f64; 3])>, BrepError> {
    use makepad_csg_brep::nurbs::b_spline_curve_n_lib::TrimMode;
    use makepad_csg_brep::topology::{edge, face};
    if by == 0.0 || !by.is_finite() {
        return Err(input("a pull needs a distance other than 0"));
    }
    let mut out = Vec::with_capacity(faces.len());
    for &f in faces {
        let (kind, normal) = crate::tess::face_kind(k, b, f)?;
        let n = normal.ok_or_else(|| input(format!("only a flat face pulls: this face is a {kind}")))?;
        let mut curves = Vec::new();
        for e in face::p4::edges(b, f, None, false, None)? {
            let curve = edge::curve(b, e)?.ok_or_else(|| BrepError::Kernel("a face's edge has no curve".into()))?;
            let mut copy = curve.copy(k)?;
            let mut interval = edge::interval(b, e)?;
            // (the kernel's own status of a trim to the edge is ignored, as its sweeps do)
            if let Err(e) = copy.trim(k, &mut interval, TrimMode::Silent) {
                if e.is_unported() {
                    return Err(e.into());
                }
            }
            curves.push(copy);
        }
        let on = makepad_csg_brep::query::api_queries::face_internal_point(k, b, f)?;
        let prism = create_linear_sweep(k, &refs(&curves), v3d(n), by, true)?;
        out.push((prism, n, [on.x, on.y, on.z]));
    }
    Ok(out)
}

/// Pulls flat faces `faces` of `b` along their outward normals by `by` (into the body below 0):
/// each face's prism joined on, or cut away. A result that is not a closed solid is refused.
pub fn pull(k: &mut Kernel, mut b: Body, faces: &[FaceId], by: f64) -> Result<Body, BrepError> {
    let prisms = pull_prisms(k, &mut b, faces, by)?;
    let mode = if by > 0.0 { BoolMode::Union } else { BoolMode::Difference };
    let mut out = b;
    for (prism, ..) in prisms {
        out = boolean(k, mode, out, prism)?;
    }
    Ok(out)
}

pub(super) fn drill_direction(dir: [f64; 3]) -> Result<Vec3, BrepError> {
    let v = v3d(dir);
    if !dir.iter().all(|c| c.is_finite()) || v.max_dimension() == 0.0 {
        return Err(input("a hole needs a finite, nonzero direction into the material"));
    }
    Ok((v / v.max_dimension()).unitize()?)
}

/// One cutter in the local drill frame (+Z into the material, Z = 0 at the surface).
fn drill_tool(k: &mut Kernel, kind: HoleKind, diameter: f64, depth: f64) -> Result<Body, BrepError> {
    if !diameter.is_finite() || diameter <= 0.0 || !depth.is_finite() || depth <= 0.0 {
        return Err(input("a hole needs a finite positive diameter and depth"));
    }
    let radius = diameter * 0.5;
    let above = Point3::new(0.0, 0.0, -1.0);
    let mouth = match kind {
        HoleKind::Simple => None,
        HoleKind::Counterbore { bore_diameter, bore_depth } => {
            if !bore_diameter.is_finite() || bore_diameter <= diameter || !(bore_depth > 0.0 && bore_depth < depth) {
                return Err(input("a counterbore needs a finite bore_diameter > diameter and 0 < bore_depth < depth"));
            }
            Some(prim::create_cylinder(k, above, bore_diameter * 0.5, bore_depth + 1.0)?)
        }
        HoleKind::Countersink { sink_diameter, angle } => {
            if !sink_diameter.is_finite() || sink_diameter <= diameter || !(angle > 30.0 && angle < 150.0) {
                return Err(input("a countersink needs a finite sink_diameter > diameter and an angle between 30 and 150 degrees"));
            }
            let wide = sink_diameter * 0.5;
            let sink_depth = (wide - radius) / (angle.to_radians() * 0.5).tan();
            if sink_depth >= depth {
                return Err(input("the countersink is deeper than the hole"));
            }
            let cone = prim::create_cone(k, Point3::ZERO, wide, radius, sink_depth)?;
            let collar = prim::create_cylinder(k, above, wide, 1.0)?;
            Some(boolean(k, BoolMode::Union, cone, collar)?)
        }
        HoleKind::Tapped { .. } => return Err(input("a tapped hole on an exact part is not made yet: drill the tap size as a simple hole, or use m.hole on a field part")),
    };
    let shaft = prim::create_cylinder(k, above, radius, depth + 1.0)?;
    match mouth {
        Some(mouth) => boolean(k, BoolMode::Union, shaft, mouth),
        None => Ok(shaft),
    }
}

/// Build the tool once, then place one copy at each drill point. Naming uses the simple tool
/// for every kind because only the narrow shaft and its blind far cap supply names.
pub(super) fn hole_cutters(k: &mut Kernel, b: &Body, kind: HoleKind, diameter: f64, depth: Option<f64>, at: &[[f64; 3]], dir: [f64; 3]) -> Result<Vec<Body>, BrepError> {
    let (last, rest) = at.split_last().ok_or_else(|| input("a hole needs at least one point in 'at'"))?;
    if !at.iter().flatten().all(|x| x.is_finite()) {
        return Err(input("a hole needs finite coordinates in 'at'"));
    }
    let [_, x, y] = drill_direction(dir)?.make_unit_ortho_vectors(None)?;
    let depth = match depth {
        Some(d) => d,
        None => {
            let bounds = brep_bounding_box(k, b, true)?;
            bounds.max().distance(bounds.min()) + 2.0
        }
    };
    let mut tool = drill_tool(k, kind, diameter, depth)?;
    let mut cutters = Vec::with_capacity(at.len());
    for (i, point) in rest.iter().enumerate() {
        // Domains 1 and 2 belong to the result and its input during name carry.
        let mut copy = copy_body(k, &tool, 3 + i as u64)?;
        place_frame(k, &mut copy, *point, x.to_array(), y.to_array())?;
        cutters.push(copy);
    }
    place_frame(k, &mut tool, *last, x.to_array(), y.to_array())?;
    cutters.push(tool);
    Ok(cutters)
}

/// Drill all points with a single difference against the union of their cutters.
pub fn hole(k: &mut Kernel, b: Body, kind: HoleKind, diameter: f64, depth: Option<f64>, at: &[[f64; 3]], dir: [f64; 3]) -> Result<Body, BrepError> {
    let mut cutters = hole_cutters(k, &b, kind, diameter, depth, at, dir)?.into_iter();
    let first = cutters.next().ok_or_else(|| input("a hole needs at least one point in 'at'"))?;
    let tool = cutters.try_fold(first, |a, b| boolean(k, BoolMode::Union, a, b))?;
    boolean(k, BoolMode::Difference, b, tool)
}

/// Revolves a sketch (u the radius, v the height, in the XY plane) about Y by `angle` degrees.
pub fn revolve(k: &mut Kernel, sketch: &Sketch, angle: f64) -> Result<Body, BrepError> {
    let frame = Frame { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0] };
    let curves = flat(sketch_curves(k, sketch, &frame)?);
    Ok(create_rotational_sweep(k, &refs(&curves), p3d([0.0; 3]), v3d([0.0, 1.0, 0.0]), angle, true)?)
}

/// Sweeps a sketch along a smooth path through `path` (at least 2 points), `scale[i]` times its
/// size at `path[i]` (empty: its own size all the way). The path is centripetal: cubic through
/// four or more points, a parabola through three, a line through two.
pub fn sweep(k: &mut Kernel, sketch: &Sketch, path: &[[f64; 3]], scale: &[f64]) -> Result<Body, BrepError> {
    if path.len() < 2 {
        return Err(input("a sweep's path needs two points"));
    }
    if !scale.is_empty() && scale.len() != path.len() {
        return Err(input(format!("a sweep's scale has one size per path point: {} for {} points", scale.len(), path.len())));
    }
    if scale.iter().any(|s| !(s.is_finite() && *s > 0.0)) {
        return Err(input("a sweep's scales must be above zero"));
    }
    let points: Vec<Point3> = path.iter().map(|p| p3d(*p)).collect();
    // Centripetal parameters: by the square root of each step, so a short step never loops.
    let mut params = vec![0.0];
    for w in points.windows(2) {
        let step = w[0].distance(w[1]);
        if !(step > 1e-9) {
            return Err(input("two of a sweep's path points are in the same place"));
        }
        params.push(params[params.len() - 1] + step.sqrt());
    }
    let total = params[params.len() - 1];
    params.iter_mut().for_each(|t| *t /= total);
    let path_curve = Curve::from(through(&points, &params)?);
    // The kernel sizes the section by how far a rail runs from a reference curve, relative to
    // the rail's start: here the scales as heights over a line, at the path's parameters.
    let rail = match scale.first() {
        Some(first) if scale.iter().any(|s| s != first) => {
            let heights: Vec<Point3> = scale.iter().map(|s| Point3::new(0.0, *s, 0.0)).collect();
            let reference = Curve::from(api_curves::create_line_segment(Point3::new(-1.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0))?);
            Some((reference, Curve::from(through(&heights, &params)?)))
        }
        _ => None,
    };
    let sketch = scale.first().map_or_else(|| sketch.clone(), |s| sketch.scaled(*s));
    let frame = Frame { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0] };
    let curves = flat(sketch_curves(k, &sketch, &frame)?);
    let options = SweepOptions {
        cap_ends: true,
        move_profile_center_to_path: true,
        orient_profile_perpendicular_to_path: true,
        ..SweepOptions::default()
    };
    let (reference, rail) = match &rail {
        Some((reference, rail)) => (Some(reference), Some(rail)),
        None => (None, None),
    };
    Ok(create_curve_sweep(k, &refs(&curves), Some(&path_curve), reference, rail, Some(&options))?.brep)
}

/// The smooth curve through `points` at `params`: cubic from four points, else of one degree
/// less than the points.
fn through(points: &[Point3], params: &[f64]) -> Result<BSplineCurve, BrepError> {
    if points.len() == 2 {
        return Ok(BSplineCurve::create_line_segment(3, points[0], points[1])?);
    }
    Ok(BSplineCurve::interpolate_points(points, Some(params), 3.min(points.len() - 1), None, None, false, ParamMethod::Centripetal)?)
}

/// A solid through sketches each at its own frame, of `degree` along the loft.
pub fn loft(k: &mut Kernel, sections: &[(Sketch, Frame)], degree: usize) -> Result<Body, BrepError> {
    if sections.len() < 2 {
        return Err(input("a loft needs two sections"));
    }
    let mut counts = Vec::with_capacity(sections.len());
    let mut all = Vec::new();
    for (i, (s, f)) in sections.iter().enumerate() {
        let curves = flat(sketch_curves(k, s, f).map_err(|e| input(format!("section {i}: {e}")))?);
        counts.push(curves.len());
        all.extend(curves);
    }
    if counts.iter().any(|c| *c != counts[0]) {
        return Err(input(format!("a loft's sections need the same number of curves; they have {counts:?}")));
    }
    Ok(create_skin_primitive(k, &counts, &refs(&all), degree, true)?.brep)
}

/// A boolean. Both inputs are taken.
pub fn boolean(k: &mut Kernel, mode: BoolMode, a: Body, b: Body) -> Result<Body, BrepError> {
    let out = match mode {
        BoolMode::Union => boolean_union(k, a, b)?,
        BoolMode::Difference => boolean_difference(k, a, b)?,
        BoolMode::Intersection => boolean_intersection(k, a, b)?,
    };
    check_solid(&out, "boolean")?;
    Ok(out)
}

/// A result must be a closed solid: the kernel can report success on a broken body.
pub fn check_solid(b: &Body, op: &str) -> Result<(), BrepError> {
    if makepad_csg_brep::topology::brep::p1::faces(b, None)?.is_empty() {
        return Err(BrepError::Kernel(format!("{op}: the result is empty")));
    }
    if !brep_is_manifold_solid(b)? {
        return Err(BrepError::Kernel(format!("{op}: the result is not a closed solid")));
    }
    Ok(())
}

fn blend(k: &mut Kernel, b: &mut Body, edges: &[EdgeId], size: f64, generator: FilletSurfaceGeneratorType, bevel: bool) -> Result<(), BrepError> {
    if edges.is_empty() {
        return Err(input("no edges selected"));
    }
    {
        let mut session = FilletSession::new(k, b)?;
        session.prepare_per_edge_constant_radius(k, edges, &vec![size; edges.len()], &vec![generator; edges.len()], 1, 1.0)?;
        let done = if bevel {
            session.create_corners(k, None).and_then(|()| {
                for corner in session.fillet_corners()? {
                    session.set_corner_bevel(corner, true)?;
                }
                session.do_filleting(k, None)
            })
        } else {
            session.do_fillet(k)
        };
        if let Err(e) = done {
            return Err(blend_failure(e, &session.executive().error, size, bevel));
        }
    }
    p5::stitch_and_orient(k, b, &p5::StitchAndOrientOptions::default())?;
    b.refresh_cached_topology();
    Ok(())
}

/// A failed fillet or chamfer, in the words of the executive's error record when it has one
/// (the kernel's own status alone is a bare "upstream status 1001").
fn blend_failure<O>(e: makepad_csg_brep::SolidError, info: &makepad_csg_brep::blend::fillet_executive::p3::ErrorInfo<O>, size: f64, bevel: bool) -> BrepError {
    use makepad_csg_brep::blend::fillet_geom::p1::FilletErrorType as T;
    if e.is_unported() {
        return e.into();
    }
    let what = if bevel { "chamfer" } else { "fillet" };
    let why = match info.code {
        c if c == T::NoRoom as i32 => format!("the {size} mm {what} does not fit: a face it runs onto is narrower than that (a smaller size, or more material beside the edge)"),
        c if c == T::BadMerge as i32 || c == T::BadTrim as i32 || c == T::BadInt as i32 => format!("the {size} mm {what} runs into a neighbouring face it cannot trim against (often too large for the material beside the edge: try a smaller size)"),
        c if c == T::VertexProblem as i32 || c == T::CornerUnsupported as i32 => format!("the {size} mm {what} cannot close a corner where the selected edges meet"),
        c if c == T::EdgeProblem as i32 => format!("the {size} mm {what} cannot be built along a selected edge"),
        _ => return e.into(),
    };
    let note = info.comment.trim();
    if info.code == T::NoRoom as i32 || note.is_empty() || note == "<no comment>" {
        BrepError::Kernel(why)
    } else {
        BrepError::Kernel(format!("{why} ({note})"))
    }
}

/// Rounds these edges.
pub fn fillet(k: &mut Kernel, b: &mut Body, edges: &[EdgeId], radius: f64) -> Result<(), BrepError> {
    if radius <= 0.0 {
        return Err(input("a fillet needs a positive radius"));
    }
    blend(k, b, edges, radius, FilletSurfaceGeneratorType::Circular, false)?;
    check_solid(b, "fillet")
}

/// Bevels these edges by `size` on each side.
pub fn chamfer(k: &mut Kernel, b: &mut Body, edges: &[EdgeId], size: f64) -> Result<(), BrepError> {
    if size <= 0.0 {
        return Err(input("a chamfer needs a positive size"));
    }
    blend(k, b, edges, size, FilletSurfaceGeneratorType::Linear, true)?;
    check_solid(b, "chamfer")
}

/// Hollows a body, walls `thickness` inward, leaving these faces open (none: a closed cavity).
/// A wall the part has no room for is refused, with where: the kernel trims inner faces that ran
/// past each other into a closed solid of the wrong shape ([`crossed`]), and never builds one
/// thicker than that. Nor does a shell that kept the part solid inside get out ([`shell_wrong`]):
/// it is refused too, in the same words, and so is one the checks cannot read.
///
/// Cost of the checks, on the B-rep worker: one exact volume and area of the shell, and
/// [`crossed`]'s tessellation and nearest-face searches.
pub fn shell(k: &mut Kernel, b: &Body, thickness: f64, open: &[FaceId]) -> Result<Body, BrepError> {
    if thickness <= 0.0 {
        return Err(input("a shell needs a positive thickness"));
    }
    let options = ShellOptions { offset: OffsetOptions { distance: -thickness, extend_convex_edges: false, merge_results: true }, ..ShellOptions::default() };
    let mut out = match shell_brep_simple(k, b, options, open) {
        Ok(out) => out,
        Err(e) if e.is_unported() => return Err(e.into()),
        Err(e) => {
            // a wall past half the part's thinnest extent is too thick wherever it is; any other
            // failure is the offset's own
            let bb = brep_bounding_box(k, b, true)?;
            let (lo, hi) = (bb.min(), bb.max());
            let thinnest = (hi.x - lo.x).min(hi.y - lo.y).min(hi.z - lo.z);
            if 2.0 * thickness >= thinnest {
                return Err(input(format!("a wall of {} is thicker than the part allows: it is {} across at its thinnest", mm(thickness), mm(thinnest))));
            }
            return Err(shell_refused(k, b, open, thickness, ShellWrong::Failed(e.to_string()))?);
        }
    };
    check_solid(&out, "shell")?;
    let wrong = match shell_wrong(k, b, open, &mut out, thickness) {
        Ok(None) => return Ok(out),
        Ok(Some(wrong)) => wrong,
        // a shell the checks cannot read (a face the merge left without area) is no shell either
        Err(BrepError::Kernel(e)) => ShellWrong::Unreadable(e),
        Err(e) => return Err(e),
    };
    Err(shell_refused(k, b, open, thickness, wrong)?)
}

/// How far over a round's radius a wall is "just over" it. The round's inward offset collapses,
/// and the offsets of the faces beside it meet just inside their own edges, leaving slivers as
/// wide as the wall is over the round; the merge can keep those for the walls, fail, or cross.
/// Measured on rounded housings 60-140 mm with 1 and 4 mm top rounds: walls 0.05-0.3 mm over
/// the round fail at some size each, walls 0.5 mm over and walls equal to it build at all.
const JUST_OVER_ROUND: f64 = 0.5;

/// The accuracy of [`shell_wrong`]'s volume and area.
const SHELL_CHECK_ACCURACY: f64 = 1e-3;

/// What is wrong with a shell the kernel built, or that it failed.
enum ShellWrong {
    /// The kernel failed: its words.
    Failed(String),
    /// The built shell could not be measured: the kernel's words.
    Unreadable(String),
    /// More volume than its wall can hold: the part stayed solid inside.
    Solid,
    /// Inner faces nearer than the wall to the part's at this point, by this face of the part.
    Crossed([f64; 3], Option<FaceId>),
}

/// What is wrong with `out`, `b` shelled `t` inward with `open` left open; `None`: nothing.
///
/// A shell's volume is the integral, over the wall, of the area of the surfaces between its
/// outer and inner faces: about the wall times the mean of their areas, so about half the wall
/// times its whole area (open rims included), for flat, convex and concave walls alike (at most
/// that mean where the area changes evenly, as it does on planes, cylinders and spheres). One
/// over the wall times its whole area kept the part solid inside: the merge kept slivers of its
/// inner walls in place of the walls (a housing 80-120 mm across shelled 1.2 mm on a 1 mm top
/// round came out 10 times too heavy, its open face uncut).
///
/// The bound has a factor of two to spare, so the volume and area are integrated coarsely
/// ([`SHELL_CHECK_ACCURACY`]).
fn shell_wrong(k: &mut Kernel, b: &Body, open: &[FaceId], out: &mut Body, t: f64) -> Result<Option<ShellWrong>, BrepError> {
    let (volume, area) = crate::query::volume_area(k, out, SHELL_CHECK_ACCURACY)?;
    if volume > t * area {
        return Ok(Some(ShellWrong::Solid));
    }
    Ok(crossing(k, b, open, out, t)?.map(|(at, face)| ShellWrong::Crossed(at, face)))
}

/// The refusal of `b` shelled `t` inward, in words a person can act on. A round the wall is just
/// over ([`JUST_OVER_ROUND`]) is the reason when the shell failed, stayed solid or crossed at it;
/// a round thinner still is named when the shell failed or stayed solid; otherwise the crossing,
/// or the kernel's own words.
fn shell_refused(k: &Kernel, b: &Body, open: &[FaceId], t: f64, wrong: ShellWrong) -> Result<BrepError, BrepError> {
    let just_over = |r: f64| t >= r - 1e-6 && t < r + JUST_OVER_ROUND;
    let just_over_words = |r: f64, at: [f64; 3]| {
        format!("a wall of {} is just over the {} round at {}: a shell cannot be built that close to a round's radius; a wall of {} or less, or {} or more, can", mm(t), mm(r), point(at), mm(r), mm(r + JUST_OVER_ROUND))
    };
    // the rounds a wall would hollow (an open face's own rounding is no wall): the one the wall
    // is just over, else the tightest
    let (mut over, mut tightest): (Option<(f64, [f64; 3])>, Option<(f64, [f64; 3])>) = (None, None);
    for f in makepad_csg_brep::topology::brep::p1::faces(b, None)? {
        if open.contains(&f) {
            continue;
        }
        if let Some((r, at)) = crate::tess::convex_radius(k, b, f)? {
            if just_over(r) && over.is_none_or(|(near, _)| r > near) {
                over = Some((r, at));
            }
            if tightest.is_none_or(|(least, _)| r < least) {
                tightest = Some((r, at));
            }
        }
    }
    let words = match wrong {
        ShellWrong::Crossed(at, face) => match face.map(|f| crate::tess::convex_radius(k, b, f)).transpose()?.flatten() {
            Some((r, _)) if just_over(r) => just_over_words(r, at),
            _ => format!("a wall of {} is thicker than the part allows at {}: its inside faces would cross there", mm(t), point(at)),
        },
        wrong => match (over, tightest) {
            (Some((r, at)), _) => just_over_words(r, at),
            (None, Some((r, at))) if r <= t + 1e-6 => format!("a wall of {} is thicker than the {} round at {} allows: its inside would turn inside out there", mm(t), mm(r), point(at)),
            _ => match wrong {
                ShellWrong::Failed(e) => format!("a wall of {} could not be made in this part (the kernel's inward offset failed: {e})", mm(t)),
                ShellWrong::Unreadable(e) => format!("a wall of {} could not be made in this part (the kernel's shell could not be measured: {e})", mm(t)),
                _ => format!("a wall of {} could not be made in this part: the kernel's shell stayed solid inside", mm(t)),
            },
        },
    };
    Ok(input(words))
}

/// The body offset by `distance` (negative shrinks). An offset whose faces would run past each
/// other is refused, with where ([`crossed`]).
pub fn offset(k: &mut Kernel, b: &mut Body, distance: f64) -> Result<Body, BrepError> {
    let options = OffsetOptions { distance, extend_convex_edges: false, merge_results: true };
    let mut out = offset_brep_simple(k, b, options)?;
    check_solid(&out, "offset")?;
    if let Some(at) = crossed(k, b, &[], &mut out, distance.abs())? {
        return Err(input(format!("an offset of {} is more than the part allows at {}: its faces would cross there", mm(distance), point(at))));
    }
    Ok(out)
}

/// Millimetres in words: `15.6 mm`.
fn mm(v: f64) -> String {
    let s = format!("{v:.3}");
    format!("{} mm", s.trim_end_matches('0').trim_end_matches('.'))
}

/// A point in words: `[4.4 7.8 5.6]`.
fn point(p: [f64; 3]) -> String {
    format!("[{:.1} {:.1} {:.1}]", p[0] + 0.0, p[1] + 0.0, p[2] + 0.0)
}

/// Where `out`, made from `b` by moving its faces `t` (a shell's walls, an offset), comes nearer
/// than `t` to `b`'s faces: faces moved past each other there (a wall thicker than the part, an
/// inward offset deeper than it), and the kernel trimmed them into a closed solid of another shape,
/// one that passes [`check_solid`]. `None`: every moved face stays `t` from `b`.
///
/// Sampled at `out`'s vertices and a point inside each of its faces, against `b`'s triangles
/// within a chord of a tenth of `t`, give or take two chords. A sample on `b`'s surface is a face
/// that did not move (an outer face, a shell's rim on an open face); `open` faces are no walls,
/// so nothing is measured from them.
///
/// One thinning is the shape's own, not a crossing: where a wall runs tangent onto a flat open
/// face (a round at the rim), the outside curls into the rim and the wall tapers to it. A sample
/// whose nearest wall point is on that rim, straight under it (against the open face's normal),
/// is that taper and passes. Anything else nearer than `t`, the rim's neighbourhood included,
/// is a crossing.
///
/// Cost: one tessellation of `b`, and two nearest-triangle searches per sample (O(triangles)
/// each, most rejected by their box); with open faces, the rim's edges sampled once and one
/// distance to them per sample nearer than `t`.
pub fn crossed(k: &mut Kernel, b: &Body, open: &[FaceId], out: &mut Body, t: f64) -> Result<Option<[f64; 3]>, BrepError> {
    Ok(crossing(k, b, open, out, t)?.map(|(at, _)| at))
}

/// [`crossed`], with the face of `b` nearest the crossing (none if the mesh named no face of it).
fn crossing(k: &mut Kernel, b: &Body, open: &[FaceId], out: &mut Body, t: f64) -> Result<Option<([f64; 3], Option<FaceId>)>, BrepError> {
    use makepad_csg_brep::query::api_queries::{face_internal_point, vertex_get_point};
    use makepad_csg_brep::topology::brep::p1;
    let chord = (t / 10.0).clamp(0.01, 0.2);
    let slack = 2.0 * chord + 1e-6;
    let mut copy = b.clone();
    let mesh = crate::tess::tessellate(k, &mut copy, &crate::tess::TessOptions { chord, ..crate::tess::TessOptions::default() })?;
    let open: Vec<u32> = open.iter().map(|f| f.index() as u32).collect();
    let mut samples = Vec::new();
    for v in p1::vertices(out, None)? {
        let p = vertex_get_point(out, v)?;
        samples.push([p.x, p.y, p.z]);
    }
    for f in p1::faces(out, None)? {
        let p = face_internal_point(k, out, f)?;
        samples.push([p.x, p.y, p.z]);
    }
    // the rims: each flat open face's edges with a wall, as segments, with its outward normal
    let mut rims: Vec<([f64; 3], [f64; 3], [f64; 3])> = Vec::new();
    if !open.is_empty() {
        let mut copy = b.clone();
        let flat: Vec<(u32, [f64; 3])> = crate::query::faces(k, &mut copy)?.into_iter().filter_map(|f| Some((f.id.index() as u32, f.normal?))).filter(|(f, _)| open.contains(f)).collect();
        for e in p1::edges(b, None)? {
            let faces: Vec<u32> = crate::names::edge_faces(b, e)?.into_iter().map(|f| f.index() as u32).collect();
            let Some(&(_, n)) = flat.iter().find(|(f, _)| faces.contains(f)) else { continue };
            if faces.iter().all(|f| open.contains(f)) {
                continue;
            }
            let Some(points) = crate::query::sampled(k, b, e, 64)? else { continue };
            rims.extend(points.windows(2).map(|w| (w[0], w[1], n)));
        }
    }
    let taper = |p: [f64; 3], wall: f64| {
        rims.iter().any(|&(a, c, n)| {
            let r = crate::query::on_segment(p, a, c);
            let v = [p[0] - r[0], p[1] - r[1], p[2] - r[2]];
            let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            (d - wall).abs() <= slack && d > 0.0 && -(v[0] * n[0] + v[1] * n[1] + v[2] * n[2]) / d >= 0.95
        })
    };
    for p in samples {
        let Some((face, d)) = crate::query::nearest_face(&mesh.mesh, &mesh.tri_face, p) else { continue };
        if d <= slack {
            continue;
        }
        let (by, wall) = if open.contains(&face) {
            crate::query::nearest_where(&mesh.mesh, &mesh.tri_face, p, |f| !open.contains(&f)).map_or((None, f64::INFINITY), |(f, d)| (Some(f), d))
        } else {
            (Some(face), d)
        };
        if wall < t - slack && !taper(p, wall) {
            let by = match by {
                Some(by) => p1::faces(b, None)?.into_iter().find(|f| f.index() as u32 == by),
                None => None,
            };
            return Ok(Some((p, by)));
        }
    }
    Ok(None)
}
