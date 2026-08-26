//! Geometry queries shared by picking, navigation, tools and culling:
//! rays, hits, frustums, snapping, measurement math.

use crate::model::ids::ElementId;
use crate::model::scene::Scene;
use crate::model::state::SceneState;
use makepad_math::{Aabb, Mat4f, Plane, Vec3f, Vec4f};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3f,
    /// Unit length.
    pub dir: Vec3f,
}

impl Ray {
    pub fn new(origin: Vec3f, dir: Vec3f) -> Self {
        Ray {
            origin,
            dir: dir.normalize(),
        }
    }

    pub fn at(&self, t: f32) -> Vec3f {
        self.origin + self.dir * t
    }

    /// Slab test. Returns the entry distance, or `None` when missed.
    pub fn intersect_aabb(&self, b: &Aabb) -> Option<f32> {
        let mut tmin = f32::NEG_INFINITY;
        let mut tmax = f32::INFINITY;
        let o = [self.origin.x, self.origin.y, self.origin.z];
        let d = [self.dir.x, self.dir.y, self.dir.z];
        let mn = [b.min.x, b.min.y, b.min.z];
        let mx = [b.max.x, b.max.y, b.max.z];
        for i in 0..3 {
            if d[i].abs() < 1e-12 {
                if o[i] < mn[i] || o[i] > mx[i] {
                    return None;
                }
            } else {
                let inv = 1.0 / d[i];
                let mut t0 = (mn[i] - o[i]) * inv;
                let mut t1 = (mx[i] - o[i]) * inv;
                if t0 > t1 {
                    std::mem::swap(&mut t0, &mut t1);
                }
                tmin = tmin.max(t0);
                tmax = tmax.min(t1);
                if tmin > tmax {
                    return None;
                }
            }
        }
        if tmax < 0.0 {
            return None;
        }
        Some(tmin.max(0.0))
    }

    /// Möller–Trumbore. Returns `(t, u, v)` for a front- or back-facing hit.
    pub fn intersect_triangle(&self, a: Vec3f, b: Vec3f, c: Vec3f) -> Option<(f32, f32, f32)> {
        let e1 = b - a;
        let e2 = c - a;
        let p = Vec3f::cross(self.dir, e2);
        let det = e1.dot(p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = self.origin - a;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = Vec3f::cross(s, e1);
        let v = self.dir.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = e2.dot(q) * inv;
        if t <= 0.0 {
            return None;
        }
        Some((t, u, v))
    }

    /// Ray vs. plane `a*x+b*y+c*z+d = 0`.
    pub fn intersect_plane(&self, plane: &Plane) -> Option<f32> {
        let n = Vec3f {
            x: plane.a,
            y: plane.b,
            z: plane.c,
        };
        let denom = n.dot(self.dir);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = -(n.dot(self.origin) + plane.d) / denom;
        if t < 0.0 {
            None
        } else {
            Some(t)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub element: ElementId,
    /// Index into `Scene::batches`.
    pub batch: u32,
    /// Triangle number inside that batch.
    pub triangle: u32,
    pub t: f32,
    pub point: Vec3f,
    /// Geometric (flat) normal, unit, facing the ray origin.
    pub normal: Vec3f,
    /// Barycentric `(u, v)` of the hit inside the triangle.
    pub bary: [f32; 2],
}

/// Six planes, normals pointing inward. Built from a clip-space matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    /// Gribb–Hartmann extraction from a column-major view-projection matrix
    /// (`makepad_math::Mat4f` layout).
    pub fn from_view_proj(m: &Mat4f) -> Self {
        let v = &m.v;
        // row i of the matrix: elements v[i], v[i+4], v[i+8], v[i+12]
        let row = |i: usize| Vec4f {
            x: v[i],
            y: v[i + 4],
            z: v[i + 8],
            w: v[i + 12],
        };
        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);
        let mk = |p: Vec4f| {
            let n = Vec3f {
                x: p.x,
                y: p.y,
                z: p.z,
            };
            let l = n.length().max(1e-12);
            Plane {
                a: p.x / l,
                b: p.y / l,
                c: p.z / l,
                d: p.w / l,
            }
        };
        let add = |a: Vec4f, b: Vec4f| Vec4f {
            x: a.x + b.x,
            y: a.y + b.y,
            z: a.z + b.z,
            w: a.w + b.w,
        };
        let sub = |a: Vec4f, b: Vec4f| Vec4f {
            x: a.x - b.x,
            y: a.y - b.y,
            z: a.z - b.z,
            w: a.w - b.w,
        };
        Frustum {
            planes: [
                mk(add(r3, r0)), // left
                mk(sub(r3, r0)), // right
                mk(add(r3, r1)), // bottom
                mk(sub(r3, r1)), // top
                mk(add(r3, r2)), // near
                mk(sub(r3, r2)), // far
            ],
        }
    }

    /// Conservative box test: true when the box is not fully outside any plane.
    pub fn intersects_aabb(&self, b: &Aabb) -> bool {
        for p in &self.planes {
            let px = if p.a >= 0.0 { b.max.x } else { b.min.x };
            let py = if p.b >= 0.0 { b.max.y } else { b.min.y };
            let pz = if p.c >= 0.0 { b.max.z } else { b.min.z };
            if p.a * px + p.b * py + p.c * pz + p.d < 0.0 {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnapKind {
    Vertex,
    EdgeMidpoint,
    Edge,
    Face,
    /// Intersection with the ground plane (Z = 0) when nothing else is hit.
    Ground,
}

impl SnapKind {
    /// Lower wins when two candidates are both inside the snap radius: a
    /// corner beats an edge beats the face it lies on, which is what a
    /// draughtsman expects.
    pub fn priority(self) -> u8 {
        match self {
            SnapKind::Vertex => 0,
            SnapKind::EdgeMidpoint => 1,
            SnapKind::Edge => 2,
            SnapKind::Face => 3,
            SnapKind::Ground => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SnapKind::Vertex => "Vertex",
            SnapKind::EdgeMidpoint => "Midpoint",
            SnapKind::Edge => "Edge",
            SnapKind::Face => "Face",
            SnapKind::Ground => "Ground",
        }
    }
}

/// World → window points (layout points, y down); `None` when the point is
/// behind the camera. Lane E builds one from `api::ViewProjector`; the scene
/// layer never learns what a camera is.
pub type ScreenProject<'a> = &'a dyn Fn(Vec3f) -> Option<[f32; 2]>;

/// Closest point to `p` on segment `a..b`, and the parameter along it.
pub fn closest_point_on_segment(a: Vec3f, b: Vec3f, p: Vec3f) -> (Vec3f, f32) {
    let ab = b - a;
    let len2 = ab.dot(ab);
    if len2 < 1e-12 {
        return (a, 0.0);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (a + ab * t, t)
}

pub(crate) fn screen_dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapHit {
    pub kind: SnapKind,
    pub point: Vec3f,
    pub element: Option<ElementId>,
    /// Unit normal of the snapped face, when there is one.
    pub normal: Option<Vec3f>,
    /// Distance from the cursor in screen pixels.
    pub screen_dist: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapOptions {
    pub vertex: bool,
    pub edge_midpoint: bool,
    pub edge: bool,
    pub face: bool,
    /// Snap search radius in screen pixels.
    pub radius_px: f32,
}

impl Default for SnapOptions {
    fn default() -> Self {
        SnapOptions {
            vertex: true,
            edge_midpoint: true,
            edge: true,
            face: true,
            radius_px: 12.0,
        }
    }
}

/// Candidate triangles a single snap will consider. A curtain wall or a
/// terrain mesh near the cursor must not turn one snap into a frame.
const MAX_SNAP_TRIANGLES: usize = 4096;

/// Snap the cursor to the model — the CAD osnap the measure tool runs on every
/// pointer move.
///
/// `ray` is the pick ray under the cursor, `project` maps a world point to
/// window points (lane E builds it from `api::ViewProjector`; the scene layer
/// never learns what a camera is) and `cursor` is the pointer in the same
/// window points. Candidates are ranked by CAD priority — vertex, then edge
/// midpoint, then edge, then the face itself — and within a priority by screen
/// distance, so a corner inside the radius always wins.
///
/// Section planes and the exploded view are honoured through
/// [`Scene::pick`], and every candidate point is tested against the section
/// half-spaces too: you can never snap to geometry the cut removed.
///
/// Returns the ground-plane intersection when the ray misses the model, which
/// is what makes measuring a site distance possible at all; `None` only when
/// the ray misses everything including Z = 0.
pub fn snap(
    scene: &Scene,
    ray: &Ray,
    state: &SceneState,
    opts: &SnapOptions,
    project: ScreenProject,
    cursor: [f32; 2],
) -> Option<SnapHit> {
    let hit = if scene.is_empty() {
        None
    } else {
        scene.pick(ray, state)
    };
    let Some(hit) = hit else {
        let t = ray.intersect_plane(&Plane {
            a: 0.0,
            b: 0.0,
            c: 1.0,
            d: 0.0,
        })?;
        return Some(SnapHit {
            kind: SnapKind::Ground,
            point: ray.at(t),
            element: None,
            normal: Some(Vec3f {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            }),
            screen_dist: 0.0,
        });
    };

    let mut best: Option<SnapHit> = None;
    let mut offer = |kind: SnapKind, point: Vec3f, element: Option<ElementId>, normal: Option<Vec3f>| {
        let enabled = match kind {
            SnapKind::Vertex => opts.vertex,
            SnapKind::EdgeMidpoint => opts.edge_midpoint,
            SnapKind::Edge => opts.edge,
            SnapKind::Face | SnapKind::Ground => true,
        };
        if !enabled || !state.section.keeps(point) {
            return;
        }
        let Some(s) = project(point) else { return };
        let d = screen_dist(s, cursor);
        if d > opts.radius_px {
            return;
        }
        let better = match &best {
            None => true,
            Some(b) => {
                let (pa, pb) = (kind.priority(), b.kind.priority());
                pa < pb || (pa == pb && d < b.screen_dist)
            }
        };
        if better {
            best = Some(SnapHit {
                kind,
                point,
                element,
                normal,
                screen_dist: d,
            });
        }
    };

    // The face point is always available; every sharper candidate outranks it.
    offer(
        SnapKind::Face,
        hit.point,
        Some(hit.element),
        Some(hit.normal),
    );

    // Screen radius → world radius, measured at the hit by projecting three
    // small world steps. No camera type crosses the layer boundary.
    let mut ppm = 0.0f32;
    if let Some(centre) = project(hit.point) {
        for axis in [
            Vec3f { x: 0.05, y: 0.0, z: 0.0 },
            Vec3f { x: 0.0, y: 0.05, z: 0.0 },
            Vec3f { x: 0.0, y: 0.0, z: 0.05 },
        ] {
            if let Some(s) = project(hit.point + axis) {
                ppm = ppm.max(screen_dist(s, centre) / 0.05);
            }
        }
    }
    let world_r = if ppm > 1e-3 {
        (opts.radius_px / ppm).clamp(1e-4, 1e4)
    } else {
        0.25
    };

    let offset = state.explode.offset(scene, hit.element);
    let mask = state.visibility_mask(scene);
    let visible = |e: ElementId| mask.get(e.index()).copied().unwrap_or(true);
    let mut candidates: Vec<(u32, u32, ElementId)> = Vec::new();
    scene.bvh.triangles_in_sphere(
        hit.point - offset,
        world_r * 1.5,
        &visible,
        &mut candidates,
    );
    if candidates.len() > MAX_SNAP_TRIANGLES {
        // Keep the ones nearest the pick rather than an arbitrary prefix.
        let centre = hit.point - offset;
        candidates.sort_by(|a, b| {
            let da = tri_dist2(scene, *a, centre);
            let db = tri_dist2(scene, *b, centre);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(MAX_SNAP_TRIANGLES);
    }

    for (bi, tri, element) in candidates {
        let batch = &scene.batches[bi as usize];
        let (a, b, c) = batch.triangle(tri);
        let disp = state.explode.offset(scene, element);
        let (a, b, c) = (a + disp, b + disp, c + disp);
        for p in [a, b, c] {
            offer(SnapKind::Vertex, p, Some(element), None);
        }
        for (p, q) in [(a, b), (b, c), (c, a)] {
            offer(
                SnapKind::EdgeMidpoint,
                (p + q) * 0.5,
                Some(element),
                None,
            );
            // Closest point on the edge measured in *screen* space, so the
            // snapped point tracks the cursor along the edge.
            if opts.edge {
                if let (Some(sp), Some(sq)) = (project(p), project(q)) {
                    let ab = [sq[0] - sp[0], sq[1] - sp[1]];
                    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
                    if len2 > 1e-6 {
                        let ap = [cursor[0] - sp[0], cursor[1] - sp[1]];
                        let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0);
                        offer(SnapKind::Edge, p + (q - p) * t, Some(element), None);
                    }
                }
            }
        }
    }
    best.or(Some(SnapHit {
        kind: SnapKind::Face,
        point: hit.point,
        element: Some(hit.element),
        normal: Some(hit.normal),
        screen_dist: 0.0,
    }))
}

fn tri_dist2(scene: &Scene, t: (u32, u32, ElementId), p: Vec3f) -> f32 {
    let (a, b, c) = scene.batches[t.0 as usize].triangle(t.1);
    let m = (a + b + c) * (1.0 / 3.0);
    (m - p).dot(m - p)
}

/// What a measurement is measuring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MeasureKind {
    #[default]
    Distance,
    Area,
    Angle,
}

/// Straight-line distance.
pub fn distance(a: Vec3f, b: Vec3f) -> f32 {
    (b - a).length()
}

/// Area of a planar polygon (Newell's method). Works for any orientation.
pub fn polygon_area(points: &[Vec3f]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut n = Vec3f::default();
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        n += Vec3f::cross(a, b);
    }
    n.length() * 0.5
}

/// Angle at `vertex` between `a` and `b`, in degrees.
pub fn angle_deg(a: Vec3f, vertex: Vec3f, b: Vec3f) -> f32 {
    let u = (a - vertex).normalize();
    let v = (b - vertex).normalize();
    u.dot(v).clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_unit_box() {
        let b = Aabb {
            min: Vec3f {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            max: Vec3f {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        };
        let r = Ray::new(
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: 5.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        assert!((r.intersect_aabb(&b).unwrap() - 4.0).abs() < 1e-5);
    }

    /// A fake camera: orthographic top-down, 100 window points per meter, so
    /// screen distances in the test are exactly world distances × 100.
    fn top_down(p: Vec3f) -> Option<[f32; 2]> {
        Some([p.x * 100.0, -p.y * 100.0])
    }

    #[test]
    fn snap_prefers_a_corner_then_a_midpoint_then_the_face() {
        let scene = crate::model::scene::Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let state = crate::model::state::SceneState::default();
        let opts = SnapOptions::default();

        // Straight down onto the roof somewhere; the face is always available.
        let c = crate::model::bounds::aabb_center(&scene.bounds);
        let down = Vec3f {
            x: 0.0,
            y: 0.0,
            z: -1.0,
        };
        let ray = Ray::new(
            Vec3f {
                x: c.x,
                y: c.y,
                z: scene.bounds.max.z + 10.0,
            },
            down,
        );
        let hit = scene.pick(&ray, &state).expect("roof");
        let face = snap(
            &scene,
            &ray,
            &state,
            &opts,
            &top_down,
            top_down(hit.point).unwrap(),
        )
        .expect("snap");
        assert!(face.screen_dist <= opts.radius_px);

        // Aim at a triangle corner: the vertex must win over the face.
        let (a, _, _) = scene.batches[hit.batch as usize].triangle(hit.triangle);
        let at_corner = snap(
            &scene,
            &ray,
            &state,
            &opts,
            &top_down,
            top_down(a).unwrap(),
        )
        .expect("snap");
        assert_eq!(at_corner.kind, SnapKind::Vertex, "{at_corner:?}");
        assert!((at_corner.point - a).length() < 1e-3);

        // Disabling vertex snapping must fall back, never silently keep it.
        let no_vertex = SnapOptions {
            vertex: false,
            ..opts
        };
        let fallback = snap(
            &scene,
            &ray,
            &state,
            &no_vertex,
            &top_down,
            top_down(a).unwrap(),
        )
        .expect("snap");
        assert_ne!(fallback.kind, SnapKind::Vertex);
    }

    #[test]
    fn snap_falls_back_to_the_ground_plane() {
        let scene = crate::model::scene::Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let state = crate::model::state::SceneState::default();
        // A ray well outside the house, aimed at Z = 0.
        let origin = Vec3f {
            x: scene.bounds.max.x + 50.0,
            y: scene.bounds.max.y + 50.0,
            z: 20.0,
        };
        let ray = Ray::new(
            origin,
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let hit = snap(
            &scene,
            &ray,
            &state,
            &SnapOptions::default(),
            &top_down,
            [0.0, 0.0],
        )
        .expect("ground");
        assert_eq!(hit.kind, SnapKind::Ground);
        assert!(hit.point.z.abs() < 1e-4);
        assert!(hit.element.is_none());
    }

    #[test]
    fn snap_never_lands_on_geometry_a_section_removed() {
        use crate::model::state::{SectionPlane, SectionState};
        let scene = crate::model::scene::Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let c = crate::model::bounds::aabb_center(&scene.bounds);
        let ray = Ray::new(
            Vec3f {
                x: c.x,
                y: c.y,
                z: scene.bounds.max.z + 10.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let mut state = crate::model::state::SceneState::default();
        let open = scene.pick(&ray, &state).expect("roof");
        let cut_at = open.point.z - 0.5;
        state.section = SectionState {
            enabled: true,
            planes: vec![SectionPlane {
                plane: Plane {
                    a: 0.0,
                    b: 0.0,
                    c: -1.0,
                    d: cut_at,
                },
                enabled: true,
                source: None,
            }],
            boxed: None,
            caps: true,
            cap_color: [0.5; 4],
        };
        if let Some(h) = snap(
            &scene,
            &ray,
            &state,
            &SnapOptions::default(),
            &top_down,
            top_down(open.point).unwrap(),
        ) {
            assert!(
                h.point.z <= cut_at + 1e-3,
                "snapped to {} above the cut at {cut_at}",
                h.point.z
            );
        }
    }

    #[test]
    fn square_area_and_right_angle() {
        let p = [
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            Vec3f {
                x: 2.0,
                y: 0.0,
                z: 0.0,
            },
            Vec3f {
                x: 2.0,
                y: 2.0,
                z: 0.0,
            },
            Vec3f {
                x: 0.0,
                y: 2.0,
                z: 0.0,
            },
        ];
        assert!((polygon_area(&p) - 4.0).abs() < 1e-5);
        assert!((angle_deg(p[1], p[0], p[3]) - 90.0).abs() < 1e-3);
    }
}
