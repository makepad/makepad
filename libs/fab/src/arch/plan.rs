//! Section/plan generation from generic tagged meshes.

use crate::arch::drawing::{Drawing2D, EdgeClass, Point, Primitive};
use crate::document::{CollectionId, Document, Object, Value};
use crate::model::UpAxis;

#[derive(Clone, Copy, Debug)]
pub struct PlanOptions {
    pub cut_height: f64,
    pub snap: f64,
    pub dimensions: bool,
    pub room_areas: bool,
    pub poche: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            cut_height: 1.2,
            snap: 0.001,
            dimensions: true,
            room_areas: true,
            poche: true,
        }
    }
}

pub fn generate(
    document: &Document,
    level: Option<CollectionId>,
    options: PlanOptions,
) -> Drawing2D {
    let base = level
        .and_then(|id| document.collection(id))
        .and_then(|collection| collection.level())
        .unwrap_or(0.0);
    let plane = base + options.cut_height;
    let mut drawing = Drawing2D {
        name: level
            .and_then(|id| document.collection(id))
            .map(|collection| collection.name.clone())
            .unwrap_or_else(|| "Plan".to_string()),
        level,
        cut_height: options.cut_height,
        ..Default::default()
    };
    let mut bounds: Option<[Point; 2]> = None;
    for object in document.objects() {
        if level.is_some() && !object.collections.contains(&level.unwrap()) {
            continue;
        }
        let class = object.semantic_class().unwrap_or_default().to_ascii_lowercase();
        let edge_class = if matches!(class.as_str(), "door" | "window" | "opening") {
            EdgeClass::Opening
        } else {
            EdgeClass::Cut
        };
        let mut object_bounds: Option<[Point; 2]> = None;
        for instance in &object.meshes {
            let Some(mesh) = document.mesh(instance.mesh) else { continue };
            let transform = makepad_math::Mat4f::mul(
                &object.transform.matrix,
                &instance.transform.matrix,
            );
            for triangle in mesh.indices.chunks_exact(3) {
                let mut points = [[0.0; 3]; 3];
                let mut valid = true;
                for (slot, index) in triangle.iter().enumerate() {
                    let Some(position) = mesh.positions.get(*index as usize) else {
                        valid = false;
                        break;
                    };
                    points[slot] = transform_point(&transform, *position);
                    let plan = project(points[slot], document.up_axis());
                    extend_bounds(&mut object_bounds, plan);
                    extend_bounds(&mut bounds, plan);
                }
                if !valid {
                    continue;
                }
                if let Some(line) = intersect(points, document.up_axis(), plane as f32, options.snap) {
                    drawing.primitives.push(Primitive::Line {
                        points: line,
                        class: edge_class,
                        object: Some(object.id),
                    });
                }
            }
        }
        if matches!(class.as_str(), "door" | "window") {
            let position = project(
                [
                    object.transform.matrix.v[12],
                    object.transform.matrix.v[13],
                    object.transform.matrix.v[14],
                ],
                document.up_axis(),
            );
            drawing.primitives.push(Primitive::Symbol {
                position: snapped(position, options.snap),
                rotation: 0.0,
                kind: class.clone(),
                object: Some(object.id),
            });
        }
        if options.room_areas && matches!(class.as_str(), "room" | "zone") {
            if let Some([min, max]) = object_bounds {
                let area = (max[0] - min[0]).abs() * (max[1] - min[1]).abs();
                drawing.primitives.push(Primitive::Text {
                    position: [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5],
                    text: format!("{}\n{area:.2} m²", object.name),
                    object: Some(object.id),
                });
            }
        }
        if options.poche && is_solid_class(&class) {
            if let Some([min, max]) = object_bounds {
                drawing.primitives.push(Primitive::Polygon {
                    points: vec![min, [max[0], min[1]], max, [min[0], max[1]]],
                    fill: [0.16, 0.16, 0.16, 0.22],
                    object: Some(object.id),
                });
            }
        }
    }
    if options.dimensions {
        if let Some([min, max]) = bounds {
            drawing.primitives.push(Primitive::Dimension {
                from: [min[0], min[1]],
                to: [max[0], min[1]],
                offset: -0.5,
                text: format!("{:.3} m", (max[0] - min[0]).abs()),
                object: None,
            });
            drawing.primitives.push(Primitive::Dimension {
                from: [max[0], min[1]],
                to: [max[0], max[1]],
                offset: 0.5,
                text: format!("{:.3} m", (max[1] - min[1]).abs()),
                object: None,
            });
        }
    }
    drawing
}

pub fn levels(document: &Document) -> Vec<(CollectionId, f64, &str)> {
    let mut levels: Vec<_> = document
        .collections()
        .iter()
        .filter_map(|collection| {
            Some((collection.id, collection.level()?, collection.name.as_str()))
        })
        .collect();
    levels.sort_by(|left, right| left.1.total_cmp(&right.1));
    levels
}

fn intersect(points: [[f32; 3]; 3], up: UpAxis, plane: f32, snap: f64) -> Option<[Point; 2]> {
    let mut intersections = Vec::with_capacity(2);
    for edge in [[0, 1], [1, 2], [2, 0]] {
        let from = points[edge[0]];
        let to = points[edge[1]];
        let a = height(from, up) - plane;
        let b = height(to, up) - plane;
        if a.abs() < 1.0e-6 && b.abs() < 1.0e-6 {
            continue;
        }
        if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
            let denominator = a - b;
            if denominator.abs() < 1.0e-8 {
                continue;
            }
            let t = a / denominator;
            let point = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
                from[2] + (to[2] - from[2]) * t,
            ];
            let point = snapped(project(point, up), snap);
            if !intersections.contains(&point) {
                intersections.push(point);
            }
        }
    }
    (intersections.len() == 2).then(|| [intersections[0], intersections[1]])
}

fn transform_point(transform: &makepad_math::Mat4f, point: [f32; 3]) -> [f32; 3] {
    let point = transform.transform_vec4(makepad_math::vec4(point[0], point[1], point[2], 1.0));
    [point.x, point.y, point.z]
}

fn height(point: [f32; 3], up: UpAxis) -> f32 {
    match up {
        UpAxis::Z => point[2],
        UpAxis::Y => point[1],
    }
}

fn project(point: [f32; 3], up: UpAxis) -> Point {
    match up {
        UpAxis::Z => [point[0] as f64, point[1] as f64],
        UpAxis::Y => [point[0] as f64, -point[2] as f64],
    }
}

fn snapped(point: Point, snap: f64) -> Point {
    if snap <= 0.0 {
        return point;
    }
    [(point[0] / snap).round() * snap, (point[1] / snap).round() * snap]
}

fn extend_bounds(bounds: &mut Option<[Point; 2]>, point: Point) {
    match bounds {
        Some([min, max]) => {
            min[0] = min[0].min(point[0]);
            min[1] = min[1].min(point[1]);
            max[0] = max[0].max(point[0]);
            max[1] = max[1].max(point[1]);
        }
        None => *bounds = Some([point, point]),
    }
}

fn is_solid_class(class: &str) -> bool {
    matches!(class, "wall" | "slab" | "roof" | "column" | "beam")
}

pub fn class_property(object: &Object) -> Option<&str> {
    object.properties.get("arch.kind").and_then(Value::text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentBuilder, Mesh, MeshInstance, Object, ObjectId};

    #[test]
    fn tagged_mesh_generates_a_snapped_cut() {
        let mut builder = DocumentBuilder::new("house");
        let mesh = builder.add_mesh(Mesh {
            positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 2.0], [0.0, 2.0, 2.0]].into(),
            indices: vec![0, 1, 2].into(),
            ..Default::default()
        });
        let mut wall = Object::new(ObjectId::default(), "Wall");
        wall.properties.insert("arch.kind".to_string(), Value::Enum("wall".to_string()));
        wall.meshes.push(MeshInstance::new(mesh));
        builder.add_object(wall);
        let drawing = generate(&builder.finish(), None, PlanOptions::default());
        assert!(drawing.primitives.iter().any(|primitive| matches!(
            primitive,
            Primitive::Line { class: EdgeClass::Cut, .. }
        )));
    }
}
