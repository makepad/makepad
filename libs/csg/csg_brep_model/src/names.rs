//! Face names of exact bodies: what a selector such as `m.edge(block.top, block.front)` or
//! `round.face(m.edge(block.top, block.front))` resolves against.
//!
//! A generator names its fresh faces by role. A modifying step carries its inputs' names to the
//! faces that come from them: an output face takes the names of the input faces whose surface its
//! sample points lie on, and where several such input faces could hold it, of those whose trimmed
//! area overlaps its own (a point of either inside the other). The faces a step makes are named by
//! what they come from: a blend by the edge whose rails it runs along, a corner patch by the faces
//! round it, a shell's inner face by the outer face it is offset from. Names never ride on kernel
//! attributes and never come from ids or order.
//!
//! The engine builds a body's names on its worker only when something asks for them (a named
//! selector, the entities a tool reads): a part nobody names pays nothing.

use crate::guard::BrepError;
use crate::ops::Body;
use crate::profile::{sketch_curves, Frame};
use crate::query;
use crate::tess::{face_kind, tessellate, TessMesh, TessOptions};
use makepad_csg_brep::curve::curve::Curve;
use makepad_csg_brep::intersect::point_class::PointClassType;
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::math::Point3;
use makepad_csg_brep::query::api_queries::{curve_closest_point, face_bounding_box, face_classify_uv, face_internal_point, surface_closest_point};
use makepad_csg_brep::topology::edge;
use makepad_csg_brep::topology::edgeuse::p1 as eu;
use makepad_csg_brep::topology::{brep, EdgeId, FaceId};
use makepad_csg_graph::edges::{Face, FaceName, Role};
use makepad_csg_graph::eval::plane_frame;
use makepad_csg_graph::{BrepOp, Key};
use std::collections::{BTreeMap, BTreeSet};

/// A face's names.
pub type Names = BTreeSet<FaceName>;

/// On a surface (mm).
const ON: f64 = 1e-4;
/// On an offset surface (mm): offset surfaces may be approximations.
const ON_OFFSET: f64 = 2e-3;
/// Sample points per face beside its interior point.
const SAMPLES: usize = 6;

fn p3(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn faces_of(b: &Body) -> Result<Vec<FaceId>, BrepError> {
    Ok(brep::p1::faces(b, None)?)
}

/// The faces an edge bounds (a seam bounds one face twice: once here).
pub fn edge_faces(b: &Body, e: EdgeId) -> Result<Vec<FaceId>, BrepError> {
    let mut out = Vec::new();
    for u in edge::edgeuses(b, e, None, None)? {
        if !eu::is_loop_edgeuse(b, u)? {
            continue;
        }
        if let Some(f) = eu::face(b, u)? {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    Ok(out)
}

/// The names of a generator step's fresh body (`key` its step), face by face in the body's order.
pub fn classify(k: &Kernel, b: &mut Body, key: Key, op: &BrepOp) -> Result<Vec<Names>, BrepError> {
    let faces = faces_of(b)?;
    // An extrude's or revolve's sketch curves, in the frame its sides were made in.
    let sides = match op {
        BrepOp::Extrude { sketch, plane, height, both } => {
            let (u, v, w) = plane_frame(*plane);
            let start = if *both { -height * 0.5 } else { 0.0 };
            let frame = Frame { origin: [w.x * start, w.y * start, w.z * start], u: [u.x, u.y, u.z], v: [v.x, v.y, v.z] };
            Some((sketch_curves(k, sketch, &frame)?.into_iter().flatten().collect::<Vec<_>>(), frame.origin, [w.x, w.y, w.z]))
        }
        BrepOp::Revolve { sketch, .. } => {
            let frame = Frame { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0] };
            Some((sketch_curves(k, sketch, &frame)?.into_iter().flatten().collect::<Vec<_>>(), [0.0; 3], [0.0, 0.0, 1.0]))
        }
        _ => None,
    };
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let (_, normal) = face_kind(k, b, f)?;
        let role = match op {
            BrepOp::Box { .. } => normal.map(|n| {
                let axis = (0..3).max_by(|a, b| n[*a].abs().total_cmp(&n[*b].abs())).unwrap_or(1);
                [Face::Left, Face::Right, Face::Bottom, Face::Top, Face::Back, Face::Front][axis * 2 + (n[axis] > 0.0) as usize]
            }).map(Role::Named),
            BrepOp::Cylinder { .. } => Some(Role::Named(match normal {
                Some(n) if n[1] > 0.99 => Face::Top,
                Some(n) if n[1] < -0.99 => Face::Bottom,
                _ => Face::Wall,
            })),
            BrepOp::Sphere { .. } | BrepOp::Torus { .. } => Some(Role::Named(Face::Wall)),
            BrepOp::Extrude { .. } => {
                let Some((curves, origin, w)) = sides.as_ref() else { continue };
                match normal {
                    Some(n) if dot(n, *w) > 0.999 => Some(Role::Named(Face::Top)),
                    Some(n) if dot(n, *w) < -0.999 => Some(Role::Named(Face::Bottom)),
                    _ => {
                        let p = p3(face_internal_point(k, b, f)?);
                        let h = dot(sub(p, *origin), *w);
                        let q = [p[0] - w[0] * h, p[1] - w[1] * h, p[2] - w[2] * h];
                        nearest_curve(k, curves, q)?.map(|(i, _)| Role::Side(i as u32))
                    }
                }
            }
            BrepOp::Revolve { .. } => {
                let Some((curves, _, _)) = sides.as_ref() else { continue };
                let p = p3(face_internal_point(k, b, f)?);
                let q = [(p[0] * p[0] + p[2] * p[2]).sqrt(), p[1], 0.0];
                match nearest_curve(k, curves, q)? {
                    Some((i, d)) if d < 1e-3 => Some(Role::Side(i as u32)),
                    _ if p[2].abs() < 1e-6 && p[0] > 0.0 => Some(Role::Named(Face::Start)),
                    _ => Some(Role::Named(Face::End)),
                }
            }
            BrepOp::Sweep { sketch, path, .. } => match normal {
                Some(_) if !path.is_empty() => {
                    let p = p3(face_internal_point(k, b, f)?);
                    let (a, z) = (path[0], path[path.len() - 1]);
                    Some(Role::Named(if dot(sub(p, a), sub(p, a)) < dot(sub(p, z), sub(p, z)) { Face::Start } else { Face::End }))
                }
                // A sweep's sides have names only where the sketch is one curve: which segment a
                // side came from is not kept along a path.
                None if sketch.shapes.len() == 1 => Some(Role::Side(0)),
                _ => None,
            },
            BrepOp::Loft { .. } => match normal {
                Some(n) if n[1] < -0.999 => Some(Role::Named(Face::Start)),
                Some(n) if n[1] > 0.999 => Some(Role::Named(Face::End)),
                _ => None,
            },
            _ => return Err(BrepError::Input("names: not a generator".into())),
        };
        out.push(role.map(|role| Names::from([FaceName { origin: key, role }])).unwrap_or_default());
    }
    Ok(out)
}

/// The curve nearest `q` and how far it is.
fn nearest_curve(k: &Kernel, curves: &[Curve], q: [f64; 3]) -> Result<Option<(usize, f64)>, BrepError> {
    let mut best: Option<(usize, f64)> = None;
    for (i, c) in curves.iter().enumerate() {
        let d = curve_closest_point(k, c, Point3::new(q[0], q[1], q[2]))?.closest.distance;
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((i, d));
        }
    }
    Ok(best)
}

/// How far `p` is from the triangles of one face of a mesh.
fn mesh_distance(m: &TessMesh, face: u32, p: [f64; 3]) -> f64 {
    let v = |i: u32| {
        let q = m.mesh.vertices[i as usize];
        [q.x, q.y, q.z]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let mut best = f64::MAX;
    for (t, tri) in m.mesh.triangles.iter().enumerate() {
        if m.tri_face[t] != face {
            continue;
        }
        let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
        let n = cross(sub(b, a), sub(c, a));
        let nn = dot(n, n);
        if nn < 1e-30 {
            continue;
        }
        let inside = |u: [f64; 3], w: [f64; 3]| dot(cross(sub(w, u), sub(p, u)), n) >= 0.0;
        let d = if inside(a, b) && inside(b, c) && inside(c, a) {
            (dot(sub(p, a), n) / nn.sqrt()).abs()
        } else {
            [(a, b), (b, c), (c, a)]
                .iter()
                .map(|&(u, w)| {
                    let e = sub(w, u);
                    let s = (dot(sub(p, u), e) / dot(e, e).max(1e-30)).clamp(0.0, 1.0);
                    let q = [u[0] + e[0] * s, u[1] + e[1] * s, u[2] + e[2] * s];
                    dot(sub(p, q), sub(p, q)).sqrt()
                })
                .fold(f64::MAX, f64::min)
        };
        best = best.min(d);
    }
    best
}

/// How `p` lies on face `g` (at `offset` from its surface, within `tol`): None off its surface;
/// Some(true) inside its trimmed area; Some(false) on its surface but outside or on its boundary.
fn lies_on(k: &Kernel, body: &mut Body, g: FaceId, p: [f64; 3], offset: f64, tol: f64) -> Result<Option<bool>, BrepError> {
    let found = {
        let Some(surface) = body.face(g)?.surface.as_ref() else { return Ok(None) };
        surface_closest_point(k, surface, Point3::new(p[0], p[1], p[2])).ok()
    };
    let Some(cp) = found else { return Ok(None) };
    if (cp.closest.distance - offset).abs() > tol {
        return Ok(None);
    }
    let bb = face_bounding_box(k, body, g, false)?;
    let (lo, hi) = (p3(bb.min()), p3(bb.max()));
    let reach = offset + tol + 1e-6;
    if (0..3).any(|i| p[i] < lo[i] - reach || p[i] > hi[i] + reach) {
        return Ok(Some(false));
    }
    for uv in &cp.uvs {
        let c = match face_classify_uv(k, body, g, *uv) {
            Ok(c) => c,
            // The classifier is not ported for every surface: the face's own triangles then.
            Err(_) => {
                let mesh = tessellate(k, body, &TessOptions { chord: 0.005, ..TessOptions::default() })?;
                if mesh_distance(&mesh, g.index() as u32, p) < 0.02 { PointClassType::Face } else { PointClassType::Nothing }
            }
        };
        if c == PointClassType::Face {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

/// Sample points of each face: its interior point and the centres of a few of its triangles, put
/// back on its surface.
fn samples(k: &Kernel, b: &mut Body) -> Result<Vec<Vec<[f64; 3]>>, BrepError> {
    let faces = faces_of(b)?;
    let mesh = tessellate(k, b, &TessOptions { chord: 0.05, ..TessOptions::default() })?;
    let mut by_face: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (t, f) in mesh.tri_face.iter().enumerate() {
        by_face.entry(*f).or_default().push(t);
    }
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let mut pts = vec![p3(face_internal_point(k, b, f)?)];
        let tris = by_face.get(&(f.index() as u32)).map(Vec::as_slice).unwrap_or_default();
        let pick = SAMPLES.min(tris.len());
        for j in 0..pick {
            let t = mesh.mesh.triangles[tris[j * tris.len() / pick]];
            let v = |i: u32| {
                let q = mesh.mesh.vertices[i as usize];
                [q.x, q.y, q.z]
            };
            let (a, bb, c) = (v(t[0]), v(t[1]), v(t[2]));
            let centre = [(a[0] + bb[0] + c[0]) / 3.0, (a[1] + bb[1] + c[1]) / 3.0, (a[2] + bb[2] + c[2]) / 3.0];
            let Some(surface) = b.face(f)?.surface.as_ref() else { continue };
            if let Ok(cp) = surface_closest_point(k, surface, Point3::new(centre[0], centre[1], centre[2])) {
                pts.push(p3(cp.closest.point));
            }
        }
        out.push(pts);
    }
    Ok(out)
}

/// The input faces an output face comes from: those its sample points lie on the surface of;
/// where there are several such same-surface candidates, only those holding one of its points in
/// their trimmed area, or one of whose points it holds in its own.
#[allow(clippy::too_many_arguments)]
fn carriers(
    k: &Kernel,
    out: &mut Body,
    face: FaceId,
    pts: &[[f64; 3]],
    inputs: &mut [Body],
    in_pts: &mut [Option<Vec<Vec<[f64; 3]>>>],
    offset: f64,
    tol: f64,
) -> Result<Vec<(usize, usize)>, BrepError> {
    let mut candidates: Vec<((usize, usize), bool)> = Vec::new();
    for (j, body) in inputs.iter_mut().enumerate() {
        for (q, g) in faces_of(body)?.iter().enumerate() {
            let mut on = None;
            for p in pts {
                match lies_on(k, body, *g, *p, offset, tol)? {
                    Some(true) => {
                        on = Some(true);
                        break;
                    }
                    Some(false) => on = Some(false),
                    None => {}
                }
            }
            if let Some(inside) = on {
                candidates.push(((j, q), inside));
            }
        }
    }
    if candidates.len() <= 1 {
        return Ok(candidates.into_iter().map(|(c, _)| c).collect());
    }
    let mut kept = Vec::new();
    for ((j, q), inside) in candidates {
        let mut keep = inside;
        if !keep {
            if in_pts[j].is_none() {
                in_pts[j] = Some(samples(k, &mut inputs[j])?);
            }
            for p in in_pts[j].as_ref().map(|s| s[q].clone()).unwrap_or_default() {
                if lies_on(k, out, face, p, offset, tol)? == Some(true) {
                    keep = true;
                    break;
                }
            }
        }
        if keep {
            kept.push((j, q));
        }
    }
    Ok(kept)
}

/// The names of a pull's prism (`normal` and `on` the pulled face's, moved `by`), face by face:
/// its far cap, where the moved face lies, takes the pulled face's names `pulled`; the rest none.
pub fn pull_caps(k: &Kernel, prism: &mut Body, normal: [f64; 3], on: [f64; 3], by: f64, pulled: &Names) -> Result<Vec<Names>, BrepError> {
    let faces = faces_of(prism)?;
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let (_, n) = face_kind(k, prism, f)?;
        let cap = match n {
            Some(n) if dot(n, normal).abs() > 0.999 => (dot(sub(p3(face_internal_point(k, prism, f)?), on), normal) - by).abs() < ON * 10.0,
            _ => false,
        };
        out.push(if cap { pulled.clone() } else { Names::new() });
    }
    Ok(out)
}

/// Only the narrow cylinder carries names into the drilled body. Its far cap faces along
/// the drill direction, and supplies the floor of a blind hole.
pub fn hole_faces(k: &Kernel, shaft: &mut Body, key: Key, dir: [f64; 3], blind: bool) -> Result<Vec<Names>, BrepError> {
    let mut out = Vec::new();
    for f in faces_of(shaft)? {
        let (_, normal) = face_kind(k, shaft, f)?;
        let face = match normal {
            None => Some(Face::Wall),
            Some(n) if blind && dot(n, dir) > 0.0 => Some(Face::Floor),
            _ => None,
        };
        out.push(face.map(|f| Names::from([FaceName { origin: key, role: Role::Named(f) }])).unwrap_or_default());
    }
    Ok(out)
}

/// An edge a fillet or chamfer took: its points, and the names of its two faces.
#[derive(Clone, Debug)]
pub struct Picked {
    pub points: Vec<[f64; 3]>,
    pub sides: [Names; 2],
}

/// The edges `ids` of a body with names `names`, as a blend step records them.
pub fn picked(k: &Kernel, b: &mut Body, names: &[Names], ids: &[EdgeId]) -> Result<Vec<Picked>, BrepError> {
    let infos = query::edges(k, b)?;
    let faces = faces_of(b)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(info) = infos.iter().find(|e| e.id == *id) else { continue };
        let fs = edge_faces(b, *id)?;
        if fs.len() != 2 {
            continue;
        }
        let side = |f: FaceId| faces.iter().position(|g| *g == f).map(|i| names[i].clone()).unwrap_or_default();
        out.push(Picked { points: info.points.clone(), sides: [side(fs[0]), side(fs[1])] });
    }
    Ok(out)
}

/// What a modifying step's names need beyond its bodies.
pub struct Step<'a> {
    pub key: Key,
    /// An offset's distance (its faces lie that far from their carriers).
    pub offset: f64,
    /// A shell's wall: an inner face lies that far inside its outer one.
    pub shell: f64,
    /// A fillet's or chamfer's edges and size; `fillet` tells which.
    pub blend: Option<(&'a [Picked], f64, bool)>,
}

/// How far `p` is from a polyline.
pub(crate) fn polyline_distance(points: &[[f64; 3]], p: [f64; 3]) -> f64 {
    points
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = sub(b, a);
            let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-30)).clamp(0.0, 1.0);
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
            dot(sub(p, q), sub(p, q)).sqrt()
        })
        .fold(f64::MAX, f64::min)
}

/// The names of a modifying step's output body (`inputs` as the step took them, `in_names`
/// theirs), face by face in the body's order.
pub fn carry(k: &Kernel, out: &mut Body, inputs: &mut [Body], in_names: &[Vec<Names>], step: &Step) -> Result<Vec<Names>, BrepError> {
    let faces = faces_of(out)?;
    let pts = samples(k, out)?;
    let mut names: Vec<Names> = vec![Names::new(); faces.len()];
    let mut new_face = vec![false; faces.len()];
    let mut in_pts: Vec<Option<Vec<Vec<[f64; 3]>>>> = vec![None; inputs.len()];
    let tol = if step.offset != 0.0 { ON_OFFSET } else { ON };
    for (i, f) in faces.iter().enumerate() {
        let mut from = carriers(k, out, *f, &pts[i], inputs, &mut in_pts, step.offset.abs(), tol)?;
        let mut inner = false;
        if from.is_empty() && step.shell > 0.0 {
            from = carriers(k, out, *f, &pts[i], inputs, &mut in_pts, step.shell, ON_OFFSET)?;
            inner = !from.is_empty();
        }
        for (j, q) in &from {
            for n in &in_names[*j][*q] {
                names[i].insert(if inner { FaceName { origin: step.key, role: Role::Inner(Some(Box::new(n.clone()))) } } else { n.clone() });
            }
        }
        if inner {
            names[i].insert(FaceName { origin: step.key, role: Role::Inner(None) });
        }
        new_face[i] = from.is_empty();
    }
    let Some((edges, size, fillet)) = step.blend else { return Ok(names) };
    // The result's adjacency, with the edges each pair of faces shares.
    let at = |f: FaceId| faces.iter().position(|g| *g == f);
    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); faces.len()];
    let mut shared: BTreeMap<(usize, usize), Vec<EdgeId>> = BTreeMap::new();
    for e in brep::p1::edges(out, None)? {
        let fs: Vec<usize> = edge_faces(out, e)?.into_iter().filter_map(at).collect();
        for x in &fs {
            for y in &fs {
                if x != y {
                    adj[*x].insert(*y);
                    shared.entry((*x, *y)).or_default().push(e);
                }
            }
        }
    }
    let infos = query::edges(k, out)?;
    let info = |e: EdgeId| infos.iter().find(|x| x.id == e);
    // A rail: a fillet is tangent to its supports across the shared edge; a chamfer meets them
    // along an edge that keeps its distance from the input edge.
    let along = |e: &query::EdgeInfo, p: &Picked| {
        let (lo, hi) = e.points.iter().map(|x| polyline_distance(&p.points, *x)).fold((f64::MAX, f64::MIN), |(a, b), d| (a.min(d), b.max(d)));
        lo > 0.1 * size && hi - lo < 0.15 * size
    };
    let rail = |i: usize, j: usize, p: &Picked| shared.get(&(i, j)).is_some_and(|es| es.iter().any(|e| info(*e).is_some_and(|x| if fillet { x.smooth } else { along(x, p) })));
    // A blend face runs along the picked edge whose two faces are its rails; every piece of a
    // split edge takes that edge's one name.
    let mut corner = Vec::new();
    let mut along_of: Vec<Vec<usize>> = vec![Vec::new(); faces.len()];
    for i in 0..faces.len() {
        if !new_face[i] {
            continue;
        }
        let fits: Vec<usize> = (0..edges.len())
            .filter(|e| {
                let p = &edges[*e];
                let rails: Names = adj[i].iter().filter(|j| !new_face[**j] && rail(i, **j, p)).flat_map(|j| names[*j].iter().cloned()).collect();
                p.sides.iter().all(|side| side.iter().any(|n| rails.contains(n)))
            })
            .collect();
        if fits.is_empty() {
            corner.push(i);
            continue;
        }
        for e in &fits {
            for a in &edges[*e].sides[0] {
                for b in &edges[*e].sides[1] {
                    let (a, b) = if a <= b { (a, b) } else { (b, a) };
                    names[i].insert(FaceName { origin: step.key, role: Role::Blend(Box::new(a.clone()), Box::new(b.clone())) });
                }
            }
        }
        along_of[i] = fits;
    }
    // A corner patch: the input faces of the edges whose blends meet at it.
    for i in corner {
        let mut round: BTreeSet<FaceName> = BTreeSet::new();
        for j in &adj[i] {
            for e in &along_of[*j] {
                for side in &edges[*e].sides {
                    round.extend(side.iter().cloned());
                }
            }
        }
        if !round.is_empty() {
            names[i].insert(FaceName { origin: step.key, role: Role::Corner(round.into_iter().collect()) });
        }
    }
    Ok(names)
}

/// The faces of a body (`names` its names) that carry `name`, by position in the body's order.
pub fn faces_named(names: &[Names], name: &FaceName) -> Vec<usize> {
    (0..names.len()).filter(|i| names[*i].iter().any(|n| matches(n, name))).collect()
}

/// A face's name answers a selector's: the same, or a corner that holds every face the selector's
/// corner names.
fn matches(have: &FaceName, want: &FaceName) -> bool {
    match (&have.role, &want.role) {
        (Role::Corner(h), Role::Corner(w)) => have.origin == want.origin && w.iter().all(|x| h.contains(x)),
        _ => have == want,
    }
}

/// The edges of a body between a face named `a` and one named `b`: every piece.
pub fn edges_named(b: &Body, names: &[Names], a: &FaceName, b_name: &FaceName) -> Result<Vec<EdgeId>, BrepError> {
    let faces = faces_of(b)?;
    let (sa, sb) = (faces_named(names, a), faces_named(names, b_name));
    let mut out = Vec::new();
    for e in brep::p1::edges(b, None)? {
        let fs: Vec<usize> = edge_faces(b, e)?.into_iter().filter_map(|f| faces.iter().position(|g| *g == f)).collect();
        if fs.len() == 2 && ((sa.contains(&fs[0]) && sb.contains(&fs[1])) || (sa.contains(&fs[1]) && sb.contains(&fs[0]))) {
            out.push(e);
        }
    }
    Ok(out)
}

/// A name as a script writes it, without the part it belongs to: `top`, `side(2)`,
/// `face(m.edge(top, front))`, `inner()`.
pub fn describe(n: &FaceName) -> String {
    match &n.role {
        Role::Named(f) => f.name().to_string(),
        Role::Side(i) => format!("side({i})"),
        Role::Inner(None) => "inner()".into(),
        Role::Inner(Some(of)) => format!("inner({})", describe(of)),
        Role::Blend(a, b) => format!("face(m.edge({}, {}))", describe(a), describe(b)),
        Role::Corner(round) => format!("corner({})", round.iter().map(describe).collect::<Vec<_>>().join(", ")),
    }
}

/// `start` and the edges continuing it tangent at shared vertices (`all`: every edge with its
/// samples, ends included).
pub fn chain<I: Copy + PartialEq>(all: &[(I, &[[f64; 3]])], start: &[I]) -> Vec<I> {
    // Each end of an edge: its point and the edge's tangent leaving through it, from the quadratic
    // through its first three samples (a chord alone turns half a sample's angle off an arc's
    // tangent).
    let ends = |p: &[[f64; 3]]| -> Vec<([f64; 3], [f64; 3])> {
        let n = p.len();
        if n < 2 {
            return Vec::new();
        }
        let out = |a: [f64; 3], b: [f64; 3], c: Option<[f64; 3]>| {
            let d = match c {
                Some(c) => [0, 1, 2].map(|i| 3.0 * a[i] - 4.0 * b[i] + c[i]),
                None => sub(a, b),
            };
            let l = dot(d, d).sqrt().max(1e-30);
            [d[0] / l, d[1] / l, d[2] / l]
        };
        let third = |i: usize| (n >= 3).then(|| p[i]);
        vec![(p[0], out(p[0], p[1], third(2))), (p[n - 1], out(p[n - 1], p[n - 2], third(n.saturating_sub(3))))]
    };
    let straight = (3.0f64).to_radians().cos();
    let mut out: Vec<I> = start.to_vec();
    let mut at = 0;
    while at < out.len() {
        let Some(e) = all.iter().find(|x| x.0 == out[at]) else {
            at += 1;
            continue;
        };
        for (p, t) in ends(e.1) {
            for f in all {
                if out.contains(&f.0) {
                    continue;
                }
                let meets = ends(f.1).into_iter().any(|(q, u)| dot(sub(p, q), sub(p, q)) < 1e-8 && dot(t, u) < -straight);
                if meets {
                    out.push(f.0);
                }
            }
        }
        at += 1;
    }
    out
}
