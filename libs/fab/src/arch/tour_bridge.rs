//! `fab::model::Scene` → [`TourScene`].
//!
//! Lane A owns the scene layer; this crate only reads it. The conversion is a
//! flatten of `Scene::snapshot()` (already world-space, already merged by
//! material) re-sorted into element order, plus a coarsening of
//! `ElementClass` into [`TourClass`].

use crate::document::Document;
use crate::model::{ElementClass, Scene};
use makepad_fab_tour::geom::{aabb_empty, aabb_union_point};
use makepad_fab_tour::scene::{TourClass, TourElement, TourScene, TourStorey};
use makepad_math::{vec3, Vec3f};

pub fn class_of(c: &ElementClass) -> TourClass {
    match c {
        ElementClass::Wall => TourClass::Wall,
        ElementClass::Slab => TourClass::Slab,
        ElementClass::Roof => TourClass::Roof,
        ElementClass::Column => TourClass::Column,
        ElementClass::Beam => TourClass::Beam,
        ElementClass::Door => TourClass::Door,
        ElementClass::Window => TourClass::Window,
        ElementClass::Skylight => TourClass::Skylight,
        ElementClass::Opening => TourClass::Opening,
        ElementClass::Stair => TourClass::Stair,
        ElementClass::Railing => TourClass::Railing,
        ElementClass::CurtainWall => TourClass::CurtainWall,
        ElementClass::Furniture => TourClass::Furniture,
        ElementClass::Lamp => TourClass::Lamp,
        ElementClass::Zone => TourClass::Zone,
        ElementClass::Site => TourClass::Site,
        // Shells and morphs are usually envelope; treat them as walls so they
        // block rather than silently letting a camera through.
        ElementClass::Shell | ElementClass::Morph | ElementClass::Mesh => TourClass::Wall,
        _ => TourClass::Other,
    }
}

/// Flatten any tagged editable document into the tour planner's mesh contract.
pub fn from_document(document: &Document) -> TourScene {
    let mut out = TourScene {
        name: document.name().to_string(),
        bounds: aabb_empty(),
        ..Default::default()
    };
    let mut storeys = std::collections::HashMap::new();
    for collection in document.collections() {
        if let Some(level) = collection.level() {
            let index = out.storeys.len();
            storeys.insert(collection.id, index);
            out.storeys.push(TourStorey {
                name: collection.name.clone(),
                elevation: level as f32,
                height: collection
                    .properties
                    .get("arch.height_m")
                    .and_then(crate::document::Value::number)
                    .unwrap_or(0.0) as f32,
            });
        }
    }
    for object in document.objects() {
        let first_tri = out.triangle_count() as u32;
        let element_index = out.elements.len() as u32;
        let mut bounds = aabb_empty();
        let mut triangles = 0;
        for instance in &object.meshes {
            let Some(mesh) = document.mesh(instance.mesh) else { continue };
            let transform = makepad_math::Mat4f::mul(
                &object.transform.matrix,
                &instance.transform.matrix,
            );
            for triangle in mesh.indices.chunks_exact(3) {
                let base = out.positions.len() as u32;
                let mut valid = true;
                for index in triangle {
                    let Some(position) = mesh.positions.get(*index as usize) else {
                        valid = false;
                        break;
                    };
                    let point = transform.transform_vec4(makepad_math::vec4(
                        position[0], position[1], position[2], 1.0,
                    ));
                    let point = vec3(point.x, point.y, point.z);
                    out.positions.push(point);
                    bounds = aabb_union_point(&bounds, point);
                    out.bounds = aabb_union_point(&out.bounds, point);
                }
                if valid {
                    out.indices.extend_from_slice(&[base, base + 1, base + 2]);
                    out.tri_element.push(element_index);
                    triangles += 1;
                } else {
                    out.positions.truncate(base as usize);
                }
            }
        }
        out.elements.push(TourElement {
            name: object.name.clone(),
            class: tagged_class(object.semantic_class()),
            storey: object
                .collections
                .iter()
                .find_map(|collection| storeys.get(collection).copied())
                .unwrap_or(usize::MAX),
            first_tri,
            tri_count: triangles,
            bounds: if triangles == 0 { aabb_empty() } else { bounds },
        });
    }
    out
}

fn tagged_class(class: Option<&str>) -> TourClass {
    match class.unwrap_or_default().to_ascii_lowercase().as_str() {
        "wall" => TourClass::Wall,
        "slab" | "floor" => TourClass::Slab,
        "roof" => TourClass::Roof,
        "column" => TourClass::Column,
        "beam" => TourClass::Beam,
        "door" => TourClass::Door,
        "window" => TourClass::Window,
        "opening" => TourClass::Opening,
        "stair" => TourClass::Stair,
        "railing" => TourClass::Railing,
        "furniture" => TourClass::Furniture,
        "light" | "lamp" => TourClass::Lamp,
        "room" | "zone" => TourClass::Zone,
        "site" => TourClass::Site,
        _ => TourClass::Other,
    }
}

impl From<&Scene> for TourScene {
    fn from(scene: &Scene) -> TourScene {
        let mut out = TourScene {
            name: scene.name.clone(),
            bounds: aabb_empty(),
            ..Default::default()
        };
        for st in &scene.stories {
            out.storeys.push(TourStorey {
                name: st.name.clone(),
                elevation: st.elevation,
                height: st.height,
            });
        }
        let story_index: std::collections::HashMap<u32, usize> = scene
            .stories
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.0, i))
            .collect();

        for e in &scene.elements {
            let first_tri = out.triangle_count() as u32;
            let ei = out.elements.len() as u32;
            let mut bounds = aabb_empty();
            let mut tris = 0u32;
            for (bi, first, count) in &e.ranges {
                let Some(batch) = scene.batches.get(*bi as usize) else {
                    continue;
                };
                let end = (*first + *count) as usize;
                let mut i = *first as usize;
                while i + 2 < end.min(batch.indices.len()) {
                    let base = out.positions.len() as u32;
                    for k in 0..3 {
                        let v = batch.vertex(batch.indices[i + k]);
                        let p = vec3(v.position[0], v.position[1], v.position[2]);
                        out.positions.push(p);
                        bounds = aabb_union_point(&bounds, p);
                        out.bounds = aabb_union_point(&out.bounds, p);
                    }
                    out.indices
                        .extend_from_slice(&[base, base + 1, base + 2]);
                    out.tri_element.push(ei);
                    tris += 1;
                    i += 3;
                }
            }
            out.elements.push(TourElement {
                name: e.name.clone(),
                class: class_of(&e.class),
                storey: e
                    .story
                    .and_then(|s| story_index.get(&s.0).copied())
                    .unwrap_or(usize::MAX),
                first_tri,
                tri_count: tris,
                bounds: if tris == 0 { aabb_empty() } else { bounds },
            });
        }
        if out.positions.is_empty() {
            out.bounds = scene.bounds;
        }
        let _: Vec3f = out.bounds.min;
        out
    }
}
