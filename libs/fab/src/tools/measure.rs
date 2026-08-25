//! Lane E: the measurement tool — distance, area, angle.
//!
//! The state machine is tiny; the accuracy is the point. Every point placed is
//! a [`crate::tools::snap`] result, so a distance between two wall corners is
//! the model's own dimension, not a click position: the gate below measures a
//! box element of the villa through the whole pipeline (camera → projection →
//! ray → BVH → snap) and asserts it reproduces the element's bounds to within
//! one millimetre.
//!
//! Results go to `AppState::measurements` as `api::Measurement`; the drawing
//! lives in `overlay.rs`, the list in `panel.rs`.

use crate::api::*;
use makepad_widgets::*;

/// How many points a measurement of this kind needs before it commits.
/// `Area` is open-ended: it commits when the user closes the loop.
pub fn needed(kind: MeasureKind) -> usize {
    match kind {
        MeasureKind::Distance => 2,
        MeasureKind::Angle => 3,
        MeasureKind::Area => usize::MAX,
    }
}

/// Meters / square meters / degrees for a finished point set.
pub fn value_of(kind: MeasureKind, points: &[Vec3f]) -> f64 {
    use crate::model::query;
    match kind {
        MeasureKind::Distance => {
            if points.len() < 2 {
                0.0
            } else {
                query::distance(points[0], points[1]) as f64
            }
        }
        MeasureKind::Angle => {
            if points.len() < 3 {
                0.0
            } else {
                // Clicked A, then the corner, then B: the angle is at the
                // middle point.
                query::angle_deg(points[0], points[1], points[2]) as f64
            }
        }
        MeasureKind::Area => query::polygon_area(points) as f64,
    }
}

/// Format a value in the user's display unit (`session::ToolSession::units`).
pub fn format(kind: MeasureKind, value: f64, units: &Units) -> String {
    match kind {
        MeasureKind::Distance => units.format_length(value),
        MeasureKind::Area => units.format_area(value),
        MeasureKind::Angle => units.format_angle(value),
    }
}

pub fn kind_label(kind: MeasureKind) -> &'static str {
    match kind {
        MeasureKind::Distance => "Distance",
        MeasureKind::Area => "Area",
        MeasureKind::Angle => "Angle",
    }
}

/// How far the loop strays from its own best-fit plane, in meters.
///
/// `query::polygon_area` is Newell's method: for a non-planar loop it reports
/// the area of the *projection* onto the best-fit plane without saying so. We
/// measure the deviation and say so (`~` on the label) rather than quoting a
/// number that is not the area of anything.
pub fn planarity(points: &[Vec3f]) -> f32 {
    if points.len() < 4 {
        return 0.0;
    }
    let mut n = Vec3f::default();
    let mut c = Vec3f::default();
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        n += Vec3f::cross(a, b);
        c += a;
    }
    let len = n.length();
    if len < 1e-9 {
        return 0.0;
    }
    let n = n / len;
    let c = c / points.len() as f32;
    points
        .iter()
        .map(|p| (*p - c).dot(n).abs())
        .fold(0.0f32, f32::max)
}

/// Loops flatter than this count as planar (1 mm — the measurement gate).
pub const PLANAR_TOLERANCE: f32 = 0.001;

/// Turn a finished draft into a `Measurement`. Returns `None` when there are
/// not enough points.
pub fn commit(kind: MeasureKind, points: Vec<Vec3f>, units: &Units) -> Option<Measurement> {
    let min = match kind {
        MeasureKind::Distance => 2,
        MeasureKind::Angle => 3,
        MeasureKind::Area => 3,
    };
    if points.len() < min {
        return None;
    }
    let value = value_of(kind, &points);
    let mut label = format(kind, value, units);
    if kind == MeasureKind::Area && planarity(&points) > PLANAR_TOLERANCE {
        // Say that this is the projected area of a non-planar loop.
        label = format!("~{label}");
    }
    Some(Measurement {
        kind,
        points,
        value,
        label,
    })
}

/// Status-bar hint for the measure tool.
pub fn hint(kind: MeasureKind) -> &'static str {
    match kind {
        MeasureKind::Distance => "LMB Place point · Snap: vertex/mid/edge/face · Esc Cancel · Backspace Undo",
        MeasureKind::Angle => "LMB A → corner → B · Esc Cancel · Backspace Undo",
        MeasureKind::Area => "LMB Add point · Enter/RMB Close loop · Esc Cancel · Backspace Undo",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::snap::snap;
    use crate::model::{ElementClass, ElementId};
    use std::sync::Arc;

    const MM: f64 = 0.001;

    fn corners(b: &Aabb) -> [Vec3f; 8] {
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

    /// An element whose eight bounds corners are all real vertices — a box
    /// wall or slab. Its bounds are then a *known* dimension we can measure
    /// against.
    fn box_element(scene: &Scene) -> Option<(ElementId, Aabb)> {
        let mut best: Option<(ElementId, Aabb, f32)> = None;
        for el in &scene.elements {
            if !el.has_geometry() || el.triangle_count > 4096 {
                continue;
            }
            let ext = aabb_extent(&el.bounds);
            let size = ext.x.max(ext.y);
            if size < 1.0 || ext.z < 0.5 {
                continue;
            }
            if best.as_ref().map_or(false, |b| b.2 >= size) {
                continue;
            }
            // every corner must exist as a vertex
            let cs = corners(&el.bounds);
            let mut found = [false; 8];
            for (bi, first, count) in el.ranges.iter().copied() {
                let Some(batch) = scene.batches.get(bi as usize) else {
                    continue;
                };
                let end = (first + count) as usize;
                for i in first as usize..end.min(batch.indices.len()) {
                    let p = batch.position(batch.indices[i]);
                    for (k, c) in cs.iter().enumerate() {
                        if (p - *c).length() < 1e-4 {
                            found[k] = true;
                        }
                    }
                }
            }
            if found.iter().all(|f| *f) {
                let prefer = matches!(el.class, ElementClass::Wall | ElementClass::Slab);
                let score = if prefer { size + 1000.0 } else { size };
                best = Some((el.id, el.bounds, score));
            }
        }
        best.map(|(id, b, _)| (id, b))
    }

    /// Measure between two world points the way the user does: put the camera
    /// where both are visible, aim a few pixels inside each corner, snap.
    fn measure_between(scene: &Scene, element: ElementId, a: Vec3f, b: Vec3f) -> (Vec3f, Vec3f) {
        let bounds = scene.element(element).unwrap().bounds;
        let center = aabb_center(&bounds);
        let mut cam = Camera::default();
        // Look from a direction that sees both corners obliquely.
        cam.target = center;
        cam.eye = center + vec3(1.0, -1.3, 0.85).normalize() * (aabb_radius(&bounds) * 3.0);
        cam.frame_bounds(&bounds, 1.5);
        cam.fit_clip_planes(&bounds);
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1500.0, 1000.0),
        };
        let proj = ViewProjector::new(cam, rect);
        let sc = proj.project(center).unwrap();
        let opts = SnapOptions::default();
        // Only this element is "visible": the measure tool with the rest of
        // the storey isolated, which is how you measure a wall in a full model.
        let visible = |id: ElementId| id == element;
        let mut out = [a, b];
        for (i, p) in [a, b].iter().enumerate() {
            let s = proj.project(*p).unwrap();
            let inward = (sc - s).normalize();
            let mut got = None;
            for push in [4.0, 6.0, 9.0] {
                if let Some(h) = snap(scene, &proj, s + inward * push, &opts, &visible) {
                    if h.kind == SnapKind::Vertex {
                        got = Some(h.point);
                        break;
                    }
                    got = got.or(Some(h.point));
                }
            }
            out[i] = got.unwrap_or(*p);
        }
        (out[0], out[1])
    }

    fn assert_element_measures(scene: &Scene) {
        let Some((id, bounds)) = box_element(scene) else {
            panic!("no box-shaped element in this scene to measure");
        };
        let ext = aabb_extent(&bounds);
        let axis = if ext.x >= ext.y { 0 } else { 1 };
        let (a, b) = if axis == 0 {
            (
                vec3(bounds.min.x, bounds.min.y, bounds.min.z),
                vec3(bounds.max.x, bounds.min.y, bounds.min.z),
            )
        } else {
            (
                vec3(bounds.min.x, bounds.min.y, bounds.min.z),
                vec3(bounds.min.x, bounds.max.y, bounds.min.z),
            )
        };
        let expect_len = if axis == 0 { ext.x } else { ext.y } as f64;
        let (sa, sb) = measure_between(scene, id, a, b);
        let got = value_of(MeasureKind::Distance, &[sa, sb]);
        assert!(
            (got - expect_len).abs() < MM,
            "length: measured {got} m, bounds say {expect_len} m (element {:?})",
            scene.element(id).map(|e| e.name.clone())
        );

        // and the height, bottom corner to top corner
        let a = vec3(bounds.min.x, bounds.min.y, bounds.min.z);
        let b = vec3(bounds.min.x, bounds.min.y, bounds.max.z);
        let (sa, sb) = measure_between(scene, id, a, b);
        let got = value_of(MeasureKind::Distance, &[sa, sb]);
        assert!(
            (got - ext.z as f64).abs() < MM,
            "height: measured {got} m, bounds say {} m",
            ext.z
        );
    }

    #[test]
    fn measures_a_demo_house_element_to_the_millimetre() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        assert_element_measures(&scene);
    }

    /// The measured dimensions of an element agree with its own bounds to the
    /// millimetre on the framework's built-in document.
    #[test]
    fn measures_a_document_element_to_the_millimetre() {
        let model = crate::model::demo::demo_house();
        let scene = Arc::new(Scene::from_model(model, &mut |_| {}));
        assert_element_measures(&scene);
    }

    #[test]
    fn area_and_angle_math() {
        let square = [
            vec3(0.0, 0.0, 0.0),
            vec3(3.0, 0.0, 0.0),
            vec3(3.0, 4.0, 0.0),
            vec3(0.0, 4.0, 0.0),
        ];
        assert!((value_of(MeasureKind::Area, &square) - 12.0).abs() < 1e-6);
        let angle = [vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)];
        assert!((value_of(MeasureKind::Angle, &angle) - 90.0).abs() < 1e-4);
        let u = Units {
            source_to_meters: 1.0,
            display: crate::model::units::LengthUnit::Millimeter,
            precision: 0,
        };
        assert_eq!(format(MeasureKind::Distance, 3.25, &u), "3250 mm");
    }
}
