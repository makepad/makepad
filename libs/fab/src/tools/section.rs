//! Lane E: section planes and the section box.
//!
//! A section is a set of half-spaces to **keep**: `a·x + b·y + c·z + d ≥ 0`
//! (`fab::model::SectionPlane`), plus an optional keep-box. Lane B's renderer
//! discards fragments outside them and draws the caps; this module owns where
//! the planes come from (a picked face, an axis, the selection), how a handle
//! drag moves one, and the eased animate-in that makes a cut land instead of
//! snapping into place.
//!
//! Everything here is pure geometry — the overlay draws it, `ToolSet` drives
//! it, `AppState::scene_state.section` stores it (through
//! `ShellAction::SetSection`, so lane B's `render_dirty` bookkeeping happens).

use crate::api::*;
use crate::tools::session::{SectionAnim, SectionHandle};
use makepad_widgets::*;

/// How long a section takes to slide in.
pub const ANIM_SECONDS: f32 = 0.25;

/// Cap fill, `fab.color_vp_cap` (#8a8a8a).
pub const CAP_COLOR: [f32; 4] = [0.541, 0.541, 0.541, 1.0];

pub const AXIS_LABEL: [&str; 3] = ["X", "Y", "Z"];

pub fn axis_vec(axis: usize) -> Vec3f {
    match axis {
        0 => vec3(1.0, 0.0, 0.0),
        1 => vec3(0.0, 1.0, 0.0),
        _ => vec3(0.0, 0.0, 1.0),
    }
}

fn dot(p: &Plane, v: Vec3f) -> f32 {
    p.a * v.x + p.b * v.y + p.c * v.z
}

pub fn normal(p: &Plane) -> Vec3f {
    vec3(p.a, p.b, p.c)
}

/// Signed distance of the plane from the origin along its normal.
pub fn offset(p: &Plane) -> f32 {
    -p.d
}

pub fn with_offset(p: &Plane, offset: f32) -> Plane {
    Plane { d: -offset, ..*p }
}

/// A plane through `point` whose kept half-space is the `normal` side.
pub fn plane_through(point: Vec3f, normal: Vec3f) -> Plane {
    let n = normal.normalize();
    Plane {
        a: n.x,
        b: n.y,
        c: n.z,
        d: -(n.dot(point)),
    }
}

/// Axis section through the middle of `bounds`. `positive` keeps the +axis
/// half (so "X" cuts away everything left of the middle).
pub fn plane_from_axis(bounds: &Aabb, axis: usize, positive: bool) -> SectionPlane {
    let n = if positive { axis_vec(axis) } else { -axis_vec(axis) };
    SectionPlane {
        plane: plane_through(aabb_center(bounds), n),
        enabled: true,
        source: None,
    }
}

/// Section from a picked face: cut *into* the surface, i.e. keep the half the
/// camera cannot see, which is the one the user wants to look at.
pub fn plane_from_hit(hit: &RayHit) -> SectionPlane {
    SectionPlane {
        plane: plane_through(hit.point, -hit.normal),
        enabled: true,
        source: Some(hit.element),
    }
}

pub fn flip(p: &SectionPlane) -> SectionPlane {
    SectionPlane {
        plane: Plane {
            a: -p.plane.a,
            b: -p.plane.b,
            c: -p.plane.c,
            d: -p.plane.d,
        },
        ..*p
    }
}

/// A keep-box inset from `bounds` by a fraction of each extent.
pub fn box_from_bounds(bounds: &Aabb, inset: f32) -> Aabb {
    let e = aabb_extent(bounds);
    let i = vec3(e.x * inset, e.y * inset, e.z * inset);
    Aabb {
        min: bounds.min + i,
        max: bounds.max - i,
    }
}

/// Where the plane's handle sits: the point of the plane nearest the model
/// centre, so the handle is always on screen next to the cut.
pub fn plane_anchor(p: &Plane, bounds: &Aabb) -> Vec3f {
    let c = aabb_center(bounds);
    let n = normal(p);
    c - n * (dot(p, c) + p.d)
}

/// An orthonormal pair spanning the plane, for drawing its outline.
pub fn plane_basis(p: &Plane) -> (Vec3f, Vec3f) {
    let n = normal(p).normalize();
    let helper = if n.z.abs() < 0.9 {
        vec3(0.0, 0.0, 1.0)
    } else {
        vec3(1.0, 0.0, 0.0)
    };
    let u = Vec3f::cross(helper, n).normalize();
    let v = Vec3f::cross(n, u).normalize();
    (u, v)
}

/// The four corners of the square drawn for a plane.
pub fn plane_quad(p: &Plane, bounds: &Aabb) -> [Vec3f; 4] {
    let c = plane_anchor(p, bounds);
    let (u, v) = plane_basis(p);
    let r = aabb_radius(bounds).max(0.5) * 0.85;
    [
        c - u * r - v * r,
        c + u * r - v * r,
        c + u * r + v * r,
        c - u * r + v * r,
    ]
}

/// The 12 edges of a box, as index pairs into [`box_corners`].
pub const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1), (1, 2), (2, 3), (3, 0),
    (4, 5), (5, 6), (6, 7), (7, 4),
    (0, 4), (1, 5), (2, 6), (3, 7),
];

pub fn box_corners(b: &Aabb) -> [Vec3f; 8] {
    [
        vec3(b.min.x, b.min.y, b.min.z),
        vec3(b.max.x, b.min.y, b.min.z),
        vec3(b.max.x, b.max.y, b.min.z),
        vec3(b.min.x, b.max.y, b.min.z),
        vec3(b.min.x, b.min.y, b.max.z),
        vec3(b.max.x, b.min.y, b.max.z),
        vec3(b.max.x, b.max.y, b.max.z),
        vec3(b.min.x, b.max.y, b.max.z),
    ]
}

/// Face `i` of the keep-box: −X +X −Y +Y −Z +Z. Returns centre, outward
/// normal and the coordinate the face sits at.
pub fn box_face(b: &Aabb, i: usize) -> (Vec3f, Vec3f, f32) {
    let c = aabb_center(b);
    let axis = i / 2;
    let positive = i % 2 == 1;
    let n = if positive { axis_vec(axis) } else { -axis_vec(axis) };
    let value = match (axis, positive) {
        (0, false) => b.min.x,
        (0, true) => b.max.x,
        (1, false) => b.min.y,
        (1, true) => b.max.y,
        (2, false) => b.min.z,
        _ => b.max.z,
    };
    let mut centre = c;
    match axis {
        0 => centre.x = value,
        1 => centre.y = value,
        _ => centre.z = value,
    }
    (centre, n, value)
}

/// Move face `i` of the keep-box to `value`, never crossing the far face.
pub fn set_box_face(b: &Aabb, i: usize, value: f32, limits: &Aabb) -> Aabb {
    let mut out = *b;
    let axis = i / 2;
    let positive = i % 2 == 1;
    let gap = (aabb_extent(limits).x + aabb_extent(limits).y + aabb_extent(limits).z) * 0.005 + 1e-3;
    match (axis, positive) {
        (0, false) => out.min.x = value.clamp(limits.min.x, out.max.x - gap),
        (0, true) => out.max.x = value.clamp(out.min.x + gap, limits.max.x),
        (1, false) => out.min.y = value.clamp(limits.min.y, out.max.y - gap),
        (1, true) => out.max.y = value.clamp(out.min.y + gap, limits.max.y),
        (2, false) => out.min.z = value.clamp(limits.min.z, out.max.z - gap),
        _ => out.max.z = value.clamp(out.min.z + gap, limits.max.z),
    }
    out
}

/// Current value of whatever the handle drags.
pub fn handle_value(section: &SectionState, handle: SectionHandle) -> Option<f32> {
    match handle {
        SectionHandle::Plane(i) => section.planes.get(i).map(|p| offset(&p.plane)),
        SectionHandle::BoxFace(i) => section.boxed.map(|b| box_face(&b, i).2),
    }
}

/// World position and drag axis of a handle.
pub fn handle_anchor(section: &SectionState, bounds: &Aabb, handle: SectionHandle) -> Option<(Vec3f, Vec3f)> {
    match handle {
        SectionHandle::Plane(i) => section.planes.get(i).map(|p| {
            let n = normal(&p.plane).normalize();
            (plane_anchor(&p.plane, bounds), n)
        }),
        SectionHandle::BoxFace(i) => section.boxed.map(|b| {
            let (c, n, _) = box_face(&b, i);
            (c, n)
        }),
    }
}

/// Apply a dragged value back into the section.
pub fn apply_handle(section: &mut SectionState, handle: SectionHandle, value: f32, limits: &Aabb) {
    match handle {
        SectionHandle::Plane(i) => {
            if let Some(p) = section.planes.get_mut(i) {
                p.plane = with_offset(&p.plane, value);
            }
        }
        SectionHandle::BoxFace(i) => {
            if let Some(b) = section.boxed {
                section.boxed = Some(set_box_face(&b, i, value, limits));
            }
        }
    }
}

/// Every handle a user can grab right now, with its world anchor.
pub fn handles(section: &SectionState, bounds: &Aabb) -> Vec<(SectionHandle, Vec3f, Vec3f)> {
    let mut out = Vec::new();
    if !section.enabled {
        return out;
    }
    for (i, p) in section.planes.iter().enumerate() {
        if p.enabled {
            let n = normal(&p.plane).normalize();
            out.push((SectionHandle::Plane(i), plane_anchor(&p.plane, bounds), n));
        }
    }
    if let Some(b) = section.boxed {
        for i in 0..6 {
            let (c, n, _) = box_face(&b, i);
            out.push((SectionHandle::BoxFace(i), c, n));
        }
    }
    out
}

/// The plane offset at which nothing is cut away: pushed back past the model.
fn open_offset(p: &Plane, bounds: &Aabb) -> f32 {
    let n = normal(p);
    let mut lowest = f32::INFINITY;
    for c in box_corners(bounds) {
        lowest = lowest.min(n.dot(c));
    }
    lowest - aabb_radius(bounds) * 0.05
}

/// The "nothing is cut yet" state a section animates in from.
pub fn open_state(target: &SectionState, bounds: &Aabb) -> SectionState {
    let mut out = target.clone();
    for p in &mut out.planes {
        p.plane = with_offset(&p.plane, open_offset(&p.plane, bounds));
    }
    if out.boxed.is_some() {
        out.boxed = Some(*bounds);
    }
    out
}

fn lerp(a: f32, b: f32, f: f32) -> f32 {
    a + (b - a) * f
}

fn lerp3(a: Vec3f, b: Vec3f, f: f32) -> Vec3f {
    a + (b - a) * f
}

/// Ease-out cubic — the motion law (§3.3: eases only, no bounce).
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// Blend two section states. Plane normals are taken from `b`; only the
/// offsets and the box travel, which is what an animate-in looks like.
pub fn lerp_section(a: &SectionState, b: &SectionState, f: f32) -> SectionState {
    let mut out = b.clone();
    for (i, p) in out.planes.iter_mut().enumerate() {
        if let Some(from) = a.planes.get(i) {
            let o = lerp(offset(&from.plane), offset(&p.plane), f);
            p.plane = with_offset(&p.plane, o);
        }
    }
    if let (Some(from), Some(to)) = (a.boxed, b.boxed) {
        out.boxed = Some(Aabb {
            min: lerp3(from.min, to.min, f),
            max: lerp3(from.max, to.max, f),
        });
    }
    out
}

/// Start an animate-in toward `target` from wherever the section is now.
pub fn animate_to(current: &SectionState, target: SectionState, bounds: &Aabb) -> (SectionAnim, SectionState) {
    let from = if current.enabled && current.planes.len() == target.planes.len() && current.boxed.is_some() == target.boxed.is_some() {
        current.clone()
    } else {
        open_state(&target, bounds)
    };
    let first = lerp_section(&from, &target, 0.0);
    (
        SectionAnim {
            from,
            to: target,
            t: 0.0,
            duration: ANIM_SECONDS,
        },
        first,
    )
}

/// A section with one plane, ready to hand to `ShellAction::SetSection`.
pub fn single(plane: SectionPlane) -> SectionState {
    SectionState {
        enabled: true,
        planes: vec![plane],
        boxed: None,
        caps: true,
        cap_color: CAP_COLOR,
    }
}

/// A section box, ready to hand to `ShellAction::SetSection`.
pub fn boxed(b: Aabb) -> SectionState {
    SectionState {
        enabled: true,
        planes: Vec::new(),
        boxed: Some(b),
        caps: true,
        cap_color: CAP_COLOR,
    }
}

/// Does the section keep this point? Mirrors what lane B's shader must do,
/// and is what the tools use to stay consistent with the picture.
pub fn keeps(section: &SectionState, p: Vec3f) -> bool {
    if !section.enabled {
        return true;
    }
    for sp in section.planes.iter().filter(|p| p.enabled) {
        if dot(&sp.plane, p) + sp.plane.d < 0.0 {
            return false;
        }
    }
    if let Some(b) = section.boxed {
        if p.x < b.min.x || p.x > b.max.x || p.y < b.min.y || p.y > b.max.y || p.z < b.min.z || p.z > b.max.z {
            return false;
        }
    }
    true
}

/// How many elements survive the section, by their centres. This is the same
/// half-space test lane B's shader runs per fragment, so the number in the
/// panel and the picture cannot drift apart.
pub fn kept_elements(section: &SectionState, scene: &Scene) -> (usize, usize) {
    let mut kept = 0;
    let mut total = 0;
    for e in scene.elements.iter().filter(|e| e.has_geometry()) {
        total += 1;
        if keeps(section, aabb_center(&e.bounds)) {
            kept += 1;
        }
    }
    (kept, total)
}

/// Name an axis-aligned plane by its axis, e.g. "−Y plane".
fn plane_name(p: &SectionPlane) -> String {
    let n = normal(&p.plane);
    let c = [n.x, n.y, n.z];
    for (i, v) in c.iter().enumerate() {
        if v.abs() > 0.999 {
            return format!("{}{} plane", if *v > 0.0 { "+" } else { "−" }, AXIS_LABEL[i]);
        }
    }
    "face plane".into()
}

pub fn describe(section: &SectionState) -> String {
    if !section.enabled || (section.planes.is_empty() && section.boxed.is_none()) {
        return "none".into();
    }
    let mut parts: Vec<String> = section.planes.iter().map(plane_name).collect();
    if section.boxed.is_some() {
        parts.push("box".into());
    }
    parts.join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Aabb {
        Aabb {
            min: vec3(-5.0, -4.0, 0.0),
            max: vec3(5.0, 4.0, 6.0),
        }
    }

    #[test]
    fn axis_plane_keeps_the_right_half() {
        let s = single(plane_from_axis(&bounds(), 0, true));
        assert!(keeps(&s, vec3(3.0, 0.0, 1.0)));
        assert!(!keeps(&s, vec3(-3.0, 0.0, 1.0)));
        let f = single(flip(&s.planes[0]));
        assert!(!keeps(&f, vec3(3.0, 0.0, 1.0)));
        assert!(keeps(&f, vec3(-3.0, 0.0, 1.0)));
    }

    #[test]
    fn face_section_cuts_into_the_surface() {
        // A wall face at x = 5 whose normal faces the camera at +X: keeping
        // the far side means keeping x <= 5.
        let hit = RayHit {
            element: ElementId::from_index(0),
            batch: 0,
            triangle: 0,
            t: 1.0,
            point: vec3(5.0, 0.0, 1.0),
            normal: vec3(1.0, 0.0, 0.0),
            bary: [0.0, 0.0],
        };
        let s = single(plane_from_hit(&hit));
        assert!(keeps(&s, vec3(4.0, 0.0, 1.0)));
        assert!(!keeps(&s, vec3(6.0, 0.0, 1.0)));
    }

    #[test]
    fn open_state_keeps_everything_and_animates_closed() {
        let b = bounds();
        let target = single(plane_from_axis(&b, 2, true));
        let open = open_state(&target, &b);
        for c in box_corners(&b) {
            assert!(keeps(&open, c), "open section clipped {c:?}");
        }
        let mid = lerp_section(&open, &target, 1.0);
        assert!(!keeps(&mid, vec3(0.0, 0.0, 0.5)));
    }

    #[test]
    fn box_faces_stay_ordered() {
        let b = bounds();
        let inner = box_from_bounds(&b, 0.2);
        // push +X face far past the −X face: it clamps instead of inverting
        let squashed = set_box_face(&inner, 1, -100.0, &b);
        assert!(squashed.max.x > squashed.min.x);
        assert!(keeps(&boxed(inner), aabb_center(&b)));
        assert!(!keeps(&boxed(inner), vec3(b.max.x, 0.0, 1.0)));
    }
}
