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
use makepad_csg_brep::offset::offset_executive::p2::{offset_brep_simple, shell_brep_simple, OffsetOptions, ShellOptions};
use makepad_csg_brep::primitive::api_curves;
use makepad_csg_brep::primitive::api_primitives as prim;
use makepad_csg_brep::primitive::primitive_creation::SweepOptions;
use makepad_csg_brep::query::api_queries::brep_is_manifold_solid;
use makepad_csg_brep::sweep::api_brep::{create_curve_sweep, create_linear_sweep, create_rotational_sweep};
use makepad_csg_brep::sweep::api_surfaces::create_skin_primitive;
use makepad_csg_brep::topology::brep::{mirror, p2, p3, p5};
use makepad_csg_brep::topology::{Brep, EdgeId, FaceId};
use makepad_csg_cad::Sketch;

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

/// Revolves a sketch (u the radius, v the height, in the XY plane) about Y by `angle` degrees.
pub fn revolve(k: &mut Kernel, sketch: &Sketch, angle: f64) -> Result<Body, BrepError> {
    let frame = Frame { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0] };
    let curves = flat(sketch_curves(k, sketch, &frame)?);
    Ok(create_rotational_sweep(k, &refs(&curves), p3d([0.0; 3]), v3d([0.0, 1.0, 0.0]), angle, true)?)
}

/// Sweeps a sketch along a smooth path through `path` (at least 2 points).
pub fn sweep(k: &mut Kernel, sketch: &Sketch, path: &[[f64; 3]]) -> Result<Body, BrepError> {
    if path.len() < 2 {
        return Err(input("a sweep's path needs two points"));
    }
    let frame = Frame { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0] };
    let curves = flat(sketch_curves(k, sketch, &frame)?);
    let points: Vec<Point3> = path.iter().map(|p| p3d(*p)).collect();
    let path_curve = Curve::from(api_curves::create_curve(&points)?);
    let options = SweepOptions {
        cap_ends: true,
        move_profile_center_to_path: true,
        orient_profile_perpendicular_to_path: true,
        ..SweepOptions::default()
    };
    Ok(create_curve_sweep(k, &refs(&curves), Some(&path_curve), None, None, Some(&options))?.brep)
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
pub fn shell(k: &mut Kernel, b: &Body, thickness: f64, open: &[FaceId]) -> Result<Body, BrepError> {
    if thickness <= 0.0 {
        return Err(input("a shell needs a positive thickness"));
    }
    let options = ShellOptions { offset: OffsetOptions { distance: -thickness, extend_convex_edges: false, merge_results: true }, ..ShellOptions::default() };
    let out = shell_brep_simple(k, b, options, open)?;
    check_solid(&out, "shell")?;
    Ok(out)
}

/// The body offset by `distance` (negative shrinks).
pub fn offset(k: &mut Kernel, b: &mut Body, distance: f64) -> Result<Body, BrepError> {
    let options = OffsetOptions { distance, extend_convex_edges: false, merge_results: true };
    let out = offset_brep_simple(k, b, options)?;
    check_solid(&out, "offset")?;
    Ok(out)
}
