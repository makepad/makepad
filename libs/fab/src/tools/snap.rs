//! Lane E: snapping.
//!
//! `fab::model::query` publishes the *types* (`SnapKind`, `SnapHit`,
//! `SnapOptions`) but lane A has not landed `query::snap` yet, so the search
//! lives here, over the same data lane A would use: the BVH picks the triangle
//! under the cursor, and the candidates are the corners, edge midpoints and
//! edges of the triangles of that element that are near the pick, ranked by
//! CAD osnap priority (vertex → midpoint → edge → face) and then by screen
//! distance. When `query::snap` lands, `snap()` becomes a one-line forward and
//! the tests below move next to it.
//!
//! # The signature lane E needs from lane A
//!
//! ```ignore
//! impl Scene {
//!     pub fn snap(
//!         &self,
//!         ray: &Ray,
//!         /// Cursor in window points: the ranking anchor. `radius_px` is
//!         /// measured against it, so it cannot be derived from `ray`.
//!         screen: DVec2,
//!         opts: &SnapOptions,
//!         /// Section / explode awareness, so the snap agrees with the
//!         /// picture (review B2).
//!         state: &SceneState,
//!         visible: &dyn Fn(ElementId) -> bool,
//!         /// World → the same window points; passed in so `fab_scene`
//!         /// stays widget-free.
//!         project: &dyn Fn(Vec3f) -> Option<DVec2>,
//!     ) -> Option<SnapHit>;
//! }
//! ```
//!
//! Behaviour the tools depend on (all of it implemented below, so it can be
//! diffed): osnap **priority** ranking, not nearest-wins — any candidate of a
//! higher priority inside `radius_px` beats every lower-priority one, ties
//! break on `screen_dist`; `Face`/`Ground` are always available as fallbacks;
//! `SnapHit::normal` is the picked triangle's geometric normal facing the ray
//! (section-from-face uses it); every point of every measurement — distance,
//! area, angle — comes from this one call, and its `SnapKind` is kept so the
//! overlay can show why the point landed where it did.
//!
//! Known gap until lane A lands B2: this snap does **not** reject points
//! clipped away by an active section plane / box.

use crate::api::*;
use makepad_widgets::*;

/// Triangles are only scanned for an element up to this many indices; past it
/// (a merged terrain mesh, a whole curtain wall) only the picked triangle
/// contributes candidates, which keeps every snap under a frame.
const MAX_SCANNED_INDICES: usize = 90_000;

/// Osnap priority: a vertex inside the radius always beats an edge inside the
/// radius, as in every CAD tool.
fn priority(kind: SnapKind) -> u8 {
    match kind {
        SnapKind::Vertex => 0,
        SnapKind::EdgeMidpoint => 1,
        SnapKind::Edge => 2,
        SnapKind::Face => 3,
        SnapKind::Ground => 4,
    }
}

fn enabled(opts: &SnapOptions, kind: SnapKind) -> bool {
    match kind {
        SnapKind::Vertex => opts.vertex,
        SnapKind::EdgeMidpoint => opts.edge_midpoint,
        SnapKind::Edge => opts.edge,
        SnapKind::Face | SnapKind::Ground => true,
    }
}

struct Search<'a> {
    proj: &'a ViewProjector,
    screen: DVec2,
    opts: &'a SnapOptions,
    best: Option<SnapHit>,
}

impl Search<'_> {
    fn offer(&mut self, kind: SnapKind, point: Vec3f, element: Option<ElementId>, normal: Option<Vec3f>) {
        if !enabled(self.opts, kind) {
            return;
        }
        let Some(s) = self.proj.project(point) else {
            return;
        };
        let d = (s - self.screen).length() as f32;
        if d > self.opts.radius_px {
            return;
        }
        let better = match &self.best {
            None => true,
            Some(b) => {
                let (pa, pb) = (priority(kind), priority(b.kind));
                pa < pb || (pa == pb && d < b.screen_dist)
            }
        };
        if better {
            self.best = Some(SnapHit {
                kind,
                point,
                element,
                normal,
                screen_dist: d,
            });
        }
    }
}

/// Closest point on segment `a..b` to `p`, all in screen points; returns the
/// parameter along the segment.
fn closest_t(a: DVec2, b: DVec2, p: DVec2) -> f64 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 < 1e-9 {
        return 0.0;
    }
    let ap = p - a;
    ((ap.x * ab.x + ap.y * ab.y) / len2).clamp(0.0, 1.0)
}

fn lerp3(a: Vec3f, b: Vec3f, t: f32) -> Vec3f {
    a + (b - a) * t
}

/// Snap the cursor to the model. Returns `None` only for an empty scene with
/// a ray that misses the ground plane too.
pub fn snap(
    scene: &Scene,
    proj: &ViewProjector,
    screen: DVec2,
    opts: &SnapOptions,
    visible: &dyn Fn(ElementId) -> bool,
) -> Option<SnapHit> {
    let ray = proj.ray(screen);
    let hit = if scene.is_empty() {
        None
    } else {
        scene.bvh.raycast(&scene.batches, &ray, visible)
    };

    let Some(hit) = hit else {
        // Nothing under the cursor: fall back to the ground plane, which is
        // what makes measuring a site distance possible at all.
        let t = ray.intersect_plane(&Plane { a: 0.0, b: 0.0, c: 1.0, d: 0.0 })?;
        return Some(SnapHit {
            kind: SnapKind::Ground,
            point: ray.at(t),
            element: None,
            normal: Some(vec3(0.0, 0.0, 1.0)),
            screen_dist: 0.0,
        });
    };

    let mut search = Search {
        proj,
        screen,
        opts,
        best: None,
    };
    // The face point is always available; every sharper candidate outranks it.
    search.offer(SnapKind::Face, hit.point, Some(hit.element), Some(hit.normal));

    // World-space radius that matches the screen radius at the pick depth, so
    // the triangle filter costs no projections.
    let ppm = proj.points_per_meter_at(hit.point);
    let world_radius = if ppm > 1e-6 {
        (opts.radius_px as f64 / ppm) as f32
    } else {
        0.0
    };

    let element = scene.element(hit.element);
    let scan_all = element
        .map(|e| e.ranges.iter().map(|r| r.2 as usize).sum::<usize>() <= MAX_SCANNED_INDICES)
        .unwrap_or(false);

    let offer_tri = |search: &mut Search, a: Vec3f, b: Vec3f, c: Vec3f| {
        let tri = [(a, b), (b, c), (c, a)];
        for (p, q) in tri {
            if (p - hit.point).length() <= world_radius {
                search.offer(SnapKind::Vertex, p, Some(hit.element), Some(hit.normal));
            }
            let mid = (p + q) * 0.5;
            if (mid - hit.point).length() <= world_radius {
                search.offer(SnapKind::EdgeMidpoint, mid, Some(hit.element), Some(hit.normal));
            }
            if opts.edge {
                if let (Some(sp), Some(sq)) = (search.proj.project(p), search.proj.project(q)) {
                    let t = closest_t(sp, sq, screen) as f32;
                    let e = lerp3(p, q, t);
                    if (e - hit.point).length() <= world_radius * 1.5 {
                        search.offer(SnapKind::Edge, e, Some(hit.element), Some(hit.normal));
                    }
                }
            }
        }
    };

    if scan_all {
        if let Some(el) = element {
            for (bi, first, count) in el.ranges.iter().copied() {
                let Some(batch) = scene.batches.get(bi as usize) else {
                    continue;
                };
                let start = first as usize;
                let end = (start + count as usize).min(batch.indices.len());
                let mut i = start;
                while i + 2 < end {
                    let a = batch.position(batch.indices[i]);
                    let b = batch.position(batch.indices[i + 1]);
                    let c = batch.position(batch.indices[i + 2]);
                    i += 3;
                    // Cheap reject: the whole triangle out of reach.
                    if (a - hit.point).length() > world_radius * 3.0
                        && (b - hit.point).length() > world_radius * 3.0
                        && (c - hit.point).length() > world_radius * 3.0
                    {
                        continue;
                    }
                    offer_tri(&mut search, a, b, c);
                }
            }
        }
    } else if let Some(batch) = scene.batches.get(hit.batch as usize) {
        let i = hit.triangle as usize * 3;
        if i + 2 < batch.indices.len() {
            let a = batch.position(batch.indices[i]);
            let b = batch.position(batch.indices[i + 1]);
            let c = batch.position(batch.indices[i + 2]);
            offer_tri(&mut search, a, b, c);
        }
    }

    search.best
}

/// One-letter tag drawn next to the snap glyph.
pub fn tag(kind: SnapKind) -> &'static str {
    match kind {
        SnapKind::Vertex => "V",
        SnapKind::EdgeMidpoint => "M",
        SnapKind::Edge => "E",
        SnapKind::Face => "F",
        SnapKind::Ground => "G",
    }
}

pub fn label(kind: SnapKind) -> &'static str {
    match kind {
        SnapKind::Vertex => "Vertex",
        SnapKind::EdgeMidpoint => "Midpoint",
        SnapKind::Edge => "Edge",
        SnapKind::Face => "Face",
        SnapKind::Ground => "Ground",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::demo::demo_house;

    fn demo_scene() -> Scene {
        Scene::from_model(demo_house(), &mut |_| {})
    }

    /// Aiming just inside a wall's corner snaps to the exact corner, not to
    /// the face point under the cursor.
    #[test]
    fn corner_pick_snaps_to_the_corner() {
        let scene = demo_scene();
        // A wall on its own — the same thing the user gets after isolating.
        let wall = scene
            .elements
            .iter()
            .find(|e| e.class == crate::model::ElementClass::Wall && e.has_geometry())
            .expect("the demo house has walls");
        let b = wall.bounds;
        let corner = vec3(b.max.x, b.min.y, b.max.z);
        let mut cam = Camera::default();
        cam.target = aabb_center(&b);
        cam.eye = cam.target + vec3(1.0, -1.0, 0.6).normalize() * (aabb_radius(&b) * 3.0);
        cam.frame_bounds(&b, 1.5);
        cam.fit_clip_planes(&b);
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1200.0, 800.0),
        };
        let proj = ViewProjector::new(cam, rect);
        // Aim a few points inside the corner so the ray lands on the wall,
        // with the corner still inside the snap radius.
        let screen_corner = proj.project(corner).expect("corner on screen");
        let inward = (proj.project(aabb_center(&b)).unwrap() - screen_corner).normalize();
        let id = wall.id;
        let mut hit = None;
        for push in [4.0, 6.0, 9.0] {
            hit = snap(
                &scene,
                &proj,
                screen_corner + inward * push,
                &SnapOptions::default(),
                &|e| e == id,
            );
            if hit.map_or(false, |h| h.kind == SnapKind::Vertex) {
                break;
            }
        }
        let hit = hit.expect("something under the cursor");
        assert_eq!(hit.kind, SnapKind::Vertex, "{hit:?}");
        assert!(
            (hit.point - corner).length() < 1e-3,
            "snapped to {:?}, wanted {:?}",
            hit.point,
            corner
        );
    }

    /// Snapping never wanders off the element under the cursor: the face
    /// fallback is always available and always on the picked element.
    #[test]
    fn a_pick_in_the_middle_of_a_face_stays_on_the_face() {
        let scene = demo_scene();
        let wall = scene
            .elements
            .iter()
            .find(|e| e.class == crate::model::ElementClass::Wall && e.has_geometry())
            .unwrap();
        let b = wall.bounds;
        let mut cam = Camera::default();
        cam.target = aabb_center(&b);
        cam.eye = cam.target + vec3(0.6, -1.0, 0.4).normalize() * (aabb_radius(&b) * 3.0);
        cam.frame_bounds(&b, 1.5);
        let proj = ViewProjector::new(
            cam,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(1200.0, 800.0),
            },
        );
        let centre = proj.project(aabb_center(&b)).unwrap();
        let id = wall.id;
        let hit = snap(&scene, &proj, centre, &SnapOptions::default(), &|e| e == id).unwrap();
        assert_eq!(hit.element, Some(id));
        assert!(matches!(hit.kind, SnapKind::Face | SnapKind::Edge | SnapKind::Vertex));
    }

    /// With no model under the cursor the ground plane still answers, so a
    /// site distance can be measured.
    #[test]
    fn empty_ray_snaps_to_ground() {
        let scene = Scene::empty();
        let mut cam = Camera::default();
        cam.eye = vec3(0.0, -20.0, 10.0);
        cam.target = vec3(0.0, 0.0, 0.0);
        let proj = ViewProjector::new(
            cam,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(800.0, 600.0),
            },
        );
        let hit = snap(&scene, &proj, dvec2(400.0, 300.0), &SnapOptions::default(), &|_| true).unwrap();
        assert_eq!(hit.kind, SnapKind::Ground);
        assert!(hit.point.z.abs() < 1e-4);
    }
}
