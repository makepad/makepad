//! `SceneSnapshot` — the flat, immutable view of a scene that offline-style
//! renderers (the progressive path tracer, lane F) upload to GPU data
//! textures **once**. Per-frame CPU→GPU uploads are a Metal hazard
//! (`local/agent_state/vj-transport-redesign/metal-audit.md`), so this type is
//! built on the loader thread, wrapped in `Arc`, and never mutated.
//!
//! Camera is *not* part of the snapshot: it changes every frame and travels
//! separately (`libs/fab/src/api.rs::Camera`). Lights are: the sun comes
//! from the analytic sky settings the app passes alongside; emissive
//! materials are discoverable from `materials[i].emissive`.

use crate::model::batch::Vertex;
use crate::model::ids::MaterialId;
use crate::model::model::{ElementClass, TextureData};
use crate::model::scene::Scene;
use makepad_math::Aabb;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct SnapshotMaterial {
    pub albedo: [f32; 4],
    pub emission: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub ior: f32,
    pub transmission: f32,
    pub double_sided: bool,
    /// Index into `SceneSnapshot::textures`, or `u32::MAX`.
    pub texture: u32,
}

/// Snapshot of one texture: same bytes as the scene's `TextureData`, kept
/// as a separate list so renderers can pack an atlas.
#[derive(Clone, Debug, Default)]
pub struct SnapshotTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Per-element semantics for scene analysis (tours: room graph, doors,
/// stairs, storeys). Index == `ElementId.0` == `triangle_element[i]`.
#[derive(Clone, Debug, Default)]
pub struct SnapshotElement {
    pub name: String,
    pub class: ElementClass,
    /// `StoryId.0`, or `u32::MAX`.
    pub story: u32,
    /// `LayerId.0`, or `u32::MAX`.
    pub layer: u32,
    /// `ElementId.0` of the parent, or `u32::MAX`.
    pub parent: u32,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub triangle_count: u32,
    /// Triangles owned by this element, as indices into the triangle list
    /// (`indices[3*t..3*t+3]`). Concatenated per element so a consumer can
    /// walk one element's geometry without scanning `triangle_element`.
    pub first_triangle_ref: u32,
}

#[derive(Clone, Debug, Default)]
pub struct SnapshotStory {
    pub name: String,
    /// Meters, Z up.
    pub elevation: f32,
    pub height: f32,
}

/// An emitting triangle. The path tracer samples these directly instead of
/// hunting for emissive materials in the triangle list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct SnapshotLight {
    /// Triangle number in [`SceneSnapshot::indices`].
    pub triangle: u32,
    /// Radiance, linear RGB.
    pub emission: [f32; 3],
    /// World-space centroid and geometric normal, for cheap sampling.
    pub center: [f32; 3],
    pub normal: [f32; 3],
    /// Triangle area, m² — the sampling weight.
    pub area: f32,
}

/// The viewpoint the *model* published (Fab `HomeView`), so the viewer can
/// open on the shot the architect chose. Distinct from the live camera, which
/// changes every frame and travels separately (`api::Camera`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnapshotCamera {
    pub eye: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub fov_y_deg: f32,
    pub perspective: bool,
    /// Sun azimuth / altitude in degrees as published with the viewpoint.
    pub sun: Option<[f32; 2]>,
}

#[derive(Clone, Debug)]
pub struct SceneSnapshot {
    /// Scene generation this snapshot belongs to.
    pub generation: u64,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list into the arrays above.
    pub indices: Vec<u32>,
    /// One per triangle.
    pub triangle_material: Vec<u32>,
    /// One per triangle: owning `ElementId.0` (for per-element visibility).
    pub triangle_element: Vec<u32>,
    /// Render-only coplanar metadata, one entry per triangle. Positions and
    /// indices above remain the canonical source geometry used by queries.
    pub triangle_priority: Vec<u16>,
    pub triangle_coplanar_group: Vec<u32>,
    pub materials: Vec<SnapshotMaterial>,
    pub textures: Vec<SnapshotTexture>,
    /// One per element (index == `ElementId.0`).
    pub elements: Vec<SnapshotElement>,
    /// Triangle numbers grouped by element; `elements[e].first_triangle_ref ..
    /// elements[e].first_triangle_ref + triangle_count` slices this.
    pub element_triangles: Vec<u32>,
    pub stories: Vec<SnapshotStory>,
    /// Emitting triangles, gathered from emissive materials.
    pub lights: Vec<SnapshotLight>,
    /// The model's published default viewpoint, when it has one.
    pub camera: Option<SnapshotCamera>,
    /// True when `materials` are the class-derived fallback palette rather
    /// than the model's own (see [`crate::model::palette`]).
    pub materials_are_derived: bool,
    pub bounds: Aabb,
}

impl Default for SceneSnapshot {
    fn default() -> Self {
        SceneSnapshot {
            generation: 0,
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            triangle_material: Vec::new(),
            triangle_element: Vec::new(),
            triangle_priority: Vec::new(),
            triangle_coplanar_group: Vec::new(),
            materials: Vec::new(),
            textures: Vec::new(),
            elements: Vec::new(),
            element_triangles: Vec::new(),
            stories: Vec::new(),
            lights: Vec::new(),
            camera: None,
            materials_are_derived: false,
            bounds: crate::model::bounds::aabb_empty(),
        }
    }
}

impl SceneSnapshot {
    pub fn from_scene(scene: &Scene) -> SceneSnapshot {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        let mut triangle_material = Vec::new();
        let mut triangle_element = Vec::new();
        let mut triangle_priority = Vec::new();
        let mut triangle_coplanar_group = Vec::new();
        for batch in &scene.batches {
            let base = positions.len() as u32;
            for vi in 0..batch.vertex_count() as u32 {
                let Vertex {
                    position,
                    normal,
                    uv,
                    ..
                } = batch.vertex(vi);
                positions.push(position);
                normals.push(normal);
                uvs.push(uv);
            }
            let mat = batch.material.0;
            let mut i = 0;
            while i + 2 < batch.indices.len() {
                indices.push(base + batch.indices[i]);
                indices.push(base + batch.indices[i + 1]);
                indices.push(base + batch.indices[i + 2]);
                triangle_material.push(mat);
                triangle_element.push(batch.element(batch.indices[i]).0);
                let tri = (i / 3) as u32;
                triangle_priority.push(batch.draw_priority_of_triangle(tri));
                triangle_coplanar_group.push(batch.coplanar_group_of_triangle(tri));
                i += 3;
            }
        }
        let materials: Vec<SnapshotMaterial> = scene
            .materials
            .iter()
            .map(|m| SnapshotMaterial {
                albedo: m.base_color,
                emission: m.emissive,
                roughness: m.roughness,
                metallic: m.metallic,
                ior: m.ior,
                transmission: m.transmission,
                double_sided: m.double_sided,
                texture: m.texture.unwrap_or(u32::MAX),
            })
            .collect();
        let textures = scene
            .textures
            .iter()
            .map(|t: &TextureData| SnapshotTexture {
                width: t.width,
                height: t.height,
                rgba: t.rgba.clone(),
            })
            .collect();
        // per-element triangle lists (bucket sort by element)
        let n_el = scene.elements.len();
        let mut counts = vec![0u32; n_el];
        for &e in &triangle_element {
            if (e as usize) < n_el {
                counts[e as usize] += 1;
            }
        }
        let mut first = vec![0u32; n_el];
        let mut acc = 0u32;
        for i in 0..n_el {
            first[i] = acc;
            acc += counts[i];
        }
        let mut cursor = first.clone();
        let mut element_triangles = vec![0u32; acc as usize];
        for (tri, &e) in triangle_element.iter().enumerate() {
            if (e as usize) < n_el {
                element_triangles[cursor[e as usize] as usize] = tri as u32;
                cursor[e as usize] += 1;
            }
        }
        let elements = scene
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| SnapshotElement {
                name: e.name.clone(),
                class: e.class.clone(),
                story: e.story.map(|s| s.0).unwrap_or(u32::MAX),
                layer: e.layer.map(|l| l.0).unwrap_or(u32::MAX),
                parent: e.parent.map(|p| p.0).unwrap_or(u32::MAX),
                bounds_min: [e.bounds.min.x, e.bounds.min.y, e.bounds.min.z],
                bounds_max: [e.bounds.max.x, e.bounds.max.y, e.bounds.max.z],
                triangle_count: counts[i],
                first_triangle_ref: first[i],
            })
            .collect();
        let stories = scene
            .stories
            .iter()
            .map(|s| SnapshotStory {
                name: s.name.clone(),
                elevation: s.elevation,
                height: s.height,
            })
            .collect();
        // Emitting triangles, so a renderer never has to scan for them.
        let mut lights: Vec<SnapshotLight> = Vec::new();
        for (tri, m) in triangle_material.iter().enumerate() {
            let Some(mat) = materials.get(*m as usize) else {
                continue;
            };
            let e = mat.emission;
            if e[0] + e[1] + e[2] <= 1e-4 {
                continue;
            }
            let (a, b, c) = (
                positions[indices[tri * 3] as usize],
                positions[indices[tri * 3 + 1] as usize],
                positions[indices[tri * 3 + 2] as usize],
            );
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len <= 1e-12 {
                continue;
            }
            lights.push(SnapshotLight {
                triangle: tri as u32,
                emission: e,
                center: [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ],
                normal: [n[0] / len, n[1] / len, n[2] / len],
                area: len * 0.5,
            });
        }

        let camera = scene.home_camera.map(|c| SnapshotCamera {
            eye: [c.eye.x, c.eye.y, c.eye.z],
            forward: [c.forward.x, c.forward.y, c.forward.z],
            up: [c.up.x, c.up.y, c.up.z],
            fov_y_deg: c.fov_y_deg,
            perspective: c.perspective,
            sun: c.sun,
        });

        SceneSnapshot {
            generation: scene.generation,
            positions,
            normals,
            uvs,
            indices,
            triangle_material,
            triangle_element,
            triangle_priority,
            triangle_coplanar_group,
            materials,
            textures,
            elements,
            element_triangles,
            stories,
            lights,
            camera,
            materials_are_derived: scene.materials_are_derived,
            bounds: scene.bounds,
        }
    }

    /// Triangle numbers owned by element `e`.
    pub fn element_triangles(&self, e: u32) -> &[u32] {
        match self.elements.get(e as usize) {
            Some(el) => {
                let a = el.first_triangle_ref as usize;
                &self.element_triangles[a..a + el.triangle_count as usize]
            }
            None => &[],
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn material(&self, id: MaterialId) -> Option<&SnapshotMaterial> {
        self.materials.get(id.index())
    }
}
