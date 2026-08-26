//! `Scene` — the immutable, shareable product of a load. Built once from a
//! [`ModelData`] on the loader thread; consumed by every downstream layer via
//! `Arc<Scene>`.
//!
//! Build steps (all inside [`Scene::from_model`]):
//! 1. normalise units / up-axis / handedness (see `model.rs` conventions)
//! 2. flatten elements (story/layer inherited from parents when missing), and
//!    synthesise story group nodes when the source publishes a flat list
//! 3. plan the batches: one [`RenderBatch`] per **(material × spatial cell)**,
//!    sized so a whole batch is a meaningful unit of frustum culling — the
//!    Metal backend draws a geometry's whole index buffer, so a batch is the
//!    smallest thing that can be culled at all
//! 4. merge geometry into those batches in world space, stamping
//!    `Vertex::element`, de-duplicating each placement's vertices
//! 5. per-element bounds, contour ranges, scene bounds
//! 6. spatial index ([`Bvh`])
//! 7. the renderer-facing [`crate::model::snapshot::SceneSnapshot`] (lazily, see
//!    `snapshot()` — call it on the loader thread)

use crate::model::batch::{ElementRange, RenderBatch, Vertex, VERTEX_STRIDE};
use crate::model::bounds::{
    aabb_center, aabb_empty, aabb_is_empty, aabb_union, aabb_union_point, transform_dir,
    transform_point,
};
use crate::model::bvh::{Bvh, PickOptions};
use crate::model::ids::{ElementId, LayerId, MaterialId, MeshId, StoryId};
use crate::model::model::{
    CameraData, ElementClass, Handedness, MaterialData, ModelData, Property, Quantity, UpAxis,
};
use crate::model::overlap::{self, OverlapReport, DEFAULT_COPLANAR_TOL_M};
use crate::model::query::{Frustum, Ray, RayHit, ScreenProject, SnapHit, SnapOptions};
use crate::model::sheets::Sheet;
use crate::model::snapshot::SceneSnapshot;
use crate::model::state::SceneState;
use crate::model::units::Units;
use makepad_math::{Aabb, Mat4f, Vec3f};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Triangles a spatial batch aims for. Big enough that draw-call count stays
/// low (5 M triangles ≈ 40–60 batches), small enough that a batch outside the
/// frustum is worth skipping.
pub const TARGET_BATCH_TRIANGLES: usize = 128 * 1024;
/// Hard ceiling on vertices per batch, so one pathological material can never
/// produce a single multi-hundred-megabyte upload.
pub const MAX_BATCH_VERTICES: usize = 1 << 20;
/// Contour segments kept (24 bytes each). The Hillside House publishes ~1.1 M.
pub const MAX_CONTOUR_SEGMENTS: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Element {
    pub id: ElementId,
    pub guid: String,
    pub name: String,
    pub class: ElementClass,
    pub story: Option<StoryId>,
    pub layer: Option<LayerId>,
    pub parent: Option<ElementId>,
    /// World-space bounds of this element's own triangles. Empty (inverted)
    /// for pure hierarchy nodes.
    pub bounds: Aabb,
    /// Where this element's triangles live: `(batch index, first_index, index_count)`.
    pub ranges: Vec<(u32, u32, u32)>,
    pub triangle_count: u32,
    /// First contour segment in [`Scene::contours`] (6 floats each).
    pub contour_first: u32,
    pub contour_count: u32,
    pub properties: Vec<Property>,
    pub quantities: Vec<Quantity>,
}

impl Element {
    pub fn has_geometry(&self) -> bool {
        self.triangle_count > 0
    }

    /// True for the story/site nodes the scene layer synthesises so the
    /// outliner has a tree. They have no geometry and no source GUID.
    pub fn is_group(&self) -> bool {
        self.class == ElementClass::Group && self.guid.is_empty()
    }
}

/// Resolved material (== `MaterialData` with the texture moved into the
/// snapshot's image list).
#[derive(Clone, Debug)]
pub struct Material {
    pub id: MaterialId,
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub ior: f32,
    pub transmission: f32,
    pub double_sided: bool,
    pub transparent: bool,
    /// Index into `SceneSnapshot::textures`, if textured.
    pub texture: Option<u32>,
}

/// One source mesh placement/material part and its render-only coplanar
/// conflict metadata. Canonical geometry remains in [`RenderBatch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenePart {
    pub id: u32,
    pub element: ElementId,
    pub element_part: u32,
    pub mesh: MeshId,
    pub material: MaterialId,
    pub batch: u32,
    pub first_index: u32,
    pub index_count: u32,
    /// Higher wins only inside `coplanar_group`; zero means no conflict.
    pub draw_priority: u16,
    pub coplanar_group: u32,
}

#[derive(Clone, Debug)]
pub struct Story {
    pub id: StoryId,
    pub name: String,
    /// Meters, Z up.
    pub elevation: f32,
    pub height: f32,
    pub elements: Vec<ElementId>,
    /// The synthesised group node for this story, when there is one.
    pub group: Option<ElementId>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub elements: Vec<ElementId>,
}

/// A published viewpoint, normalised into scene space (meters, Z up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneCamera {
    pub eye: Vec3f,
    /// Unit view direction.
    pub forward: Vec3f,
    pub up: Vec3f,
    pub fov_y_deg: f32,
    pub perspective: bool,
    /// Sun azimuth / altitude in degrees, when the source published them.
    pub sun: Option<[f32; 2]>,
}

impl SceneCamera {
    /// A look-at target `distance` meters ahead, for `Camera { eye, target }`.
    pub fn target_at(&self, distance: f32) -> Vec3f {
        self.eye + self.forward * distance.max(0.01)
    }
}

#[derive(Clone, Debug)]
pub struct SceneCameraSet {
    pub name: String,
    pub cameras: Vec<(String, SceneCamera)>,
}

/// Parent/child relation, precomputed for the outliner.
#[derive(Clone, Debug, Default)]
pub struct ElementTree {
    pub roots: Vec<ElementId>,
    /// `children[element.index()]`.
    pub children: Vec<Vec<ElementId>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneStats {
    /// Source elements — the building's own, excluding synthesised groups.
    pub elements: u32,
    pub elements_with_geometry: u32,
    /// Story/site nodes the scene layer added to give the outliner a tree.
    pub group_nodes: u32,
    pub triangles: u32,
    pub vertices: u32,
    pub batches: u32,
    pub materials: u32,
    pub textures: u32,
    /// Authored contour segments kept (the hidden-line / ink source).
    pub contour_segments: u32,
    /// True when the contour budget clipped the list.
    pub contours_truncated: bool,
    pub bvh_nodes: u32,
    pub build_ms: f32,
    /// Bytes of CPU geometry held by `batches`.
    pub geometry_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Scene {
    /// Canonical editable source when this scene came through a loader.
    pub document: Option<Arc<crate::document::Document>>,
    pub name: String,
    pub source_path: Option<std::path::PathBuf>,
    pub units: Units,
    pub metadata: Vec<(String, String)>,
    pub elements: Vec<Element>,
    pub materials: Vec<Material>,
    /// True when `materials` came from [`crate::model::palette`] because the source
    /// published none — a display default, not the model's own appearance.
    pub materials_are_derived: bool,
    pub stories: Vec<Story>,
    pub layers: Vec<Layer>,
    pub tree: ElementTree,
    pub batches: Vec<RenderBatch>,
    pub parts: Vec<ScenePart>,
    pub overlap_report: OverlapReport,
    pub bounds: Aabb,
    pub bvh: Bvh,
    /// World-space contour segments (stride 6: ax ay az bx by bz) — the
    /// authored architecture outlines, for the hidden-line / ink overlay.
    /// Sliced per element by `Element::contour_first/contour_count`.
    pub contours: Vec<f32>,
    pub sheets: Vec<Sheet>,
    /// Published viewpoints, as the source grouped them.
    pub cameras: Vec<SceneCameraSet>,
    /// The source's default viewpoint (Fab `HomeView`).
    pub home_camera: Option<SceneCamera>,
    pub stats: SceneStats,
    /// Monotonic per process; the renderer re-uploads when it changes.
    pub generation: u64,
    /// Bumped only when the GEOMETRY was built — a material-only edit bumps
    /// `generation` but leaves this alone, so the pick-pass geometry, model
    /// hash and AO bake are not rebuilt per colour-drag step.
    pub geometry_generation: u64,
    snapshot: OnceLock<Arc<SceneSnapshot>>,
    /// Decoded textures, moved out of `MaterialData` (index == `Material::texture`).
    pub textures: Vec<crate::model::model::TextureData>,
    /// `story_rank[story.index()]` — position by elevation, low to high.
    story_rank: Vec<u32>,
    /// Vertical spacing the exploded view uses between stories, meters.
    story_spacing: f32,
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// One contiguous run of triangles: an element's slice of one mesh placement
/// under one material. The unit the batch planner shuffles.
struct Placement {
    element: u32,
    part: u32,
    element_part: u32,
    /// Index into `model.meshes`.
    mesh: u32,
    world: Mat4f,
    material: MaterialId,
    first_index: u32,
    index_count: u32,
    triangles: u32,
    center: Vec3f,
    /// Filled by the planner.
    batch: u32,
}

impl Scene {
    /// Build the optimized runtime scene from the canonical editable document.
    pub fn from_document(
        document: crate::document::Document,
        progress: &mut dyn FnMut(f32),
    ) -> Scene {
        let source = Arc::new(document);
        let mut scene = Scene::from_model(source.as_ref().clone().into_model_data(), progress);
        scene.document = Some(source);
        scene
    }

    /// As [`Scene::from_document`], with named build stages for progress UI.
    pub fn from_document_with(
        document: crate::document::Document,
        progress: &mut dyn FnMut(&'static str, f32),
    ) -> Scene {
        let source = Arc::new(document);
        let mut scene = Scene::from_model_with(source.as_ref().clone().into_model_data(), progress);
        scene.document = Some(source);
        scene
    }

    /// A scene with nothing in it. Bounds are empty; `generation` is 0 so the
    /// renderer treats it as "nothing to upload".
    pub fn empty() -> Scene {
        Scene {
            document: None,
            name: String::new(),
            source_path: None,
            units: Units::default(),
            metadata: Vec::new(),
            elements: Vec::new(),
            materials: Vec::new(),
            materials_are_derived: false,
            stories: Vec::new(),
            layers: Vec::new(),
            tree: ElementTree::default(),
            batches: Vec::new(),
            parts: Vec::new(),
            overlap_report: OverlapReport::default(),
            bounds: aabb_empty(),
            bvh: Bvh::default(),
            contours: Vec::new(),
            sheets: Vec::new(),
            cameras: Vec::new(),
            home_camera: None,
            stats: SceneStats::default(),
            generation: 0,
            geometry_generation: 0,
            snapshot: OnceLock::new(),
            textures: Vec::new(),
            story_rank: Vec::new(),
            story_spacing: 3.0,
        }
    }

    /// Build the scene. Runs on the loader thread; never call from the UI
    /// thread with a large model. `progress` gets `0..=1` for the build stage.
    pub fn from_model(model: ModelData, progress: &mut dyn FnMut(f32)) -> Scene {
        Scene::from_model_with(model, &mut |_, f| progress(f))
    }

    /// As [`Scene::from_model`], with a named stage alongside the fraction so
    /// the loader can say what it is doing ("batches", "bvh", "snapshot").
    pub fn from_model_with(
        mut model: ModelData,
        progress: &mut dyn FnMut(&'static str, f32),
    ) -> Scene {
        let t0 = std::time::Instant::now();
        let scale = model.units.source_to_meters;
        let normalise = normalisation_matrix(model.up_axis, scale);
        let flip_winding = model.handedness == Handedness::Left;

        // ---- 1. materials + textures --------------------------------------
        let materials_are_derived = model.materials.is_empty() && !model.elements.is_empty();
        if materials_are_derived {
            model.materials = crate::model::palette::palette();
        }
        let source_textures = std::mem::take(&mut model.textures);
        let mut textures = Vec::with_capacity(source_textures.len());
        let mut texture_remap = vec![None; source_textures.len()];
        for (source_index, texture) in source_textures.into_iter().enumerate() {
            let expected = (texture.width as usize)
                .checked_mul(texture.height as usize)
                .and_then(|n| n.checked_mul(4));
            if texture.width == 0 || texture.height == 0 || expected != Some(texture.rgba.len()) {
                continue;
            }
            texture_remap[source_index] = Some(textures.len() as u32);
            textures.push(texture);
        }
        let mut materials: Vec<Material> = model
            .materials
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let texture = m
                    .texture
                    .and_then(|source| texture_remap.get(source).copied().flatten());
                material_from_data(MaterialId::from_index(i), m, texture)
            })
            .collect();
        let default_material = MaterialId::from_index(materials.len());
        materials.push(material_from_data(
            default_material,
            &MaterialData {
                name: "Default".into(),
                ..Default::default()
            },
            None,
        ));
        progress("materials", 0.03);

        // ---- 2. elements (flatten story/layer through parents) -------------
        let n_source = model.elements.len();
        let mut elements: Vec<Element> = Vec::with_capacity(n_source + 8);
        for (i, e) in model.elements.iter().enumerate() {
            elements.push(Element {
                id: ElementId::from_index(i),
                guid: e.guid.clone(),
                name: if e.name.is_empty() {
                    format!("{} {}", e.class.label(), i)
                } else {
                    e.name.clone()
                },
                class: e.class.clone(),
                story: e.story,
                layer: e.layer,
                parent: e.parent.filter(|p| p.index() < n_source && p.index() != i),
                bounds: aabb_empty(),
                ranges: Vec::new(),
                triangle_count: 0,
                contour_first: 0,
                contour_count: 0,
                properties: e.properties.clone(),
                quantities: e.quantities.clone(),
            });
        }
        for i in 0..n_source {
            if elements[i].story.is_none() || elements[i].layer.is_none() {
                let mut p = elements[i].parent;
                let mut guard = 0;
                while let Some(pid) = p {
                    if elements[i].story.is_none() {
                        elements[i].story = elements[pid.index()].story;
                    }
                    if elements[i].layer.is_none() {
                        elements[i].layer = elements[pid.index()].layer;
                    }
                    if elements[i].story.is_some() && elements[i].layer.is_some() {
                        break;
                    }
                    p = elements[pid.index()].parent;
                    guard += 1;
                    if guard > 64 {
                        break;
                    }
                }
            }
        }
        progress("elements", 0.06);

        // ---- 3. plan the batches ------------------------------------------
        // Cache each mesh's local bounds once; the planner needs a world
        // centroid per placement and nothing else.
        let mut mesh_bounds: Vec<Aabb> = Vec::with_capacity(model.meshes.len());
        for m in &model.meshes {
            let mut b = aabb_empty();
            for p in &m.positions {
                b = aabb_union_point(
                    &b,
                    Vec3f {
                        x: p[0],
                        y: p[1],
                        z: p[2],
                    },
                );
            }
            mesh_bounds.push(b);
        }

        // When the palette is ours, each element's appearance is decided by its
        // class (plus name/layer hints) once, here, and travels on the mesh
        // placement — Fab shares meshes between elements of different classes.
        let derived: Vec<MaterialId> = if materials_are_derived {
            elements
                .iter()
                .map(|e| {
                    let layer = e
                        .layer
                        .and_then(|l| model.layers.get(l.index()))
                        .map(|l| l.name.to_lowercase())
                        .unwrap_or_default();
                    crate::model::palette::slot_for(&e.class, &e.name.to_lowercase(), &layer).material()
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut placements: Vec<Placement> = Vec::new();
        let mut next_part = 0u32;
        for (ei, e) in model.elements.iter().enumerate() {
            let mut element_part = 0u32;
            let element_world = Mat4f::mul(&normalise, &e.transform);
            for mref in &e.meshes {
                let Some(mesh) = model.meshes.get(mref.mesh.index()) else {
                    continue;
                };
                if mesh.indices.len() < 3 || mesh.positions.is_empty() {
                    continue;
                }
                let world = Mat4f::mul(&element_world, &mref.transform);
                let center = {
                    let b = mesh_bounds[mref.mesh.index()];
                    if aabb_is_empty(&b) {
                        Vec3f::default()
                    } else {
                        transform_point(&world, aabb_center(&b))
                    }
                };
                let override_material = mref.material.or_else(|| derived.get(ei).copied());
                let groups: Vec<(MaterialId, u32, u32)> = if mesh.submeshes.is_empty() {
                    vec![(
                        override_material.unwrap_or(default_material),
                        0,
                        mesh.indices.len() as u32,
                    )]
                } else {
                    mesh.submeshes
                        .iter()
                        .map(|s| {
                            let m = override_material.unwrap_or(
                                if s.material.index() < materials.len() - 1 {
                                    s.material
                                } else {
                                    default_material
                                },
                            );
                            (m, s.first_index, s.index_count)
                        })
                        .collect()
                };
                for (material, first_index, index_count) in groups {
                    let end = ((first_index + index_count) as usize).min(mesh.indices.len());
                    let count = end.saturating_sub(first_index as usize) as u32;
                    if count < 3 {
                        continue;
                    }
                    placements.push(Placement {
                        element: ei as u32,
                        part: next_part,
                        element_part,
                        mesh: mref.mesh.0,
                        world,
                        material,
                        first_index,
                        index_count: count,
                        triangles: count / 3,
                        center,
                        batch: 0,
                    });
                    next_part += 1;
                    element_part += 1;
                }
            }
        }
        let plan = plan_batches(&mut placements, &materials);
        progress("batches", 0.12);

        // ---- 4. merge geometry --------------------------------------------
        let mut batches: Vec<RenderBatch> = plan
            .iter()
            .map(|(material, transparent)| RenderBatch {
                material: *material,
                transparent: *transparent,
                vertices: Vec::new(),
                indices: Vec::new(),
                element_ranges: Vec::new(),
                bounds: aabb_empty(),
            })
            .collect();

        let max_mesh_verts = model
            .meshes
            .iter()
            .map(|m| m.positions.len())
            .max()
            .unwrap_or(0);
        let mut world_pos: Vec<Vec3f> = vec![Vec3f::default(); max_mesh_verts];
        let mut world_nrm: Vec<Vec3f> = vec![Vec3f::default(); max_mesh_verts];
        let mut remap: Vec<u32> = vec![0; max_mesh_verts];
        let mut stamp: Vec<u32> = vec![0; max_mesh_verts];
        let mut cur_stamp: u32 = 0;
        let mut last_key: Option<(u32, Mat4f)> = None;
        let mut parts: Vec<Option<ScenePart>> = vec![None; next_part as usize];

        let total = placements.len().max(1);
        for (pi, p) in placements.iter().enumerate() {
            let mesh = &model.meshes[p.mesh as usize];
            let nv = mesh.positions.len();
            // Placements of the same mesh under the same transform are adjacent
            // after the plan sort only by luck, so recompute unless identical.
            if last_key.map_or(true, |(m, w)| m != p.mesh || w.v != p.world.v) {
                for (i, q) in mesh.positions.iter().enumerate() {
                    world_pos[i] = transform_point(
                        &p.world,
                        Vec3f {
                            x: q[0],
                            y: q[1],
                            z: q[2],
                        },
                    );
                }
                if mesh.normals.len() == nv {
                    for (i, q) in mesh.normals.iter().enumerate() {
                        world_nrm[i] = transform_dir(
                            &p.world,
                            Vec3f {
                                x: q[0],
                                y: q[1],
                                z: q[2],
                            },
                        )
                        .normalize();
                    }
                }
                last_key = Some((p.mesh, p.world));
            }
            let has_normals = mesh.normals.len() == nv;
            let has_uvs = mesh.uvs.len() == nv;
            let batch = &mut batches[p.batch as usize];
            let range_start = batch.indices.len() as u32;
            let mut el_bounds = aabb_empty();

            cur_stamp = cur_stamp.wrapping_add(1);
            if cur_stamp == 0 {
                stamp.iter_mut().for_each(|s| *s = 0);
                cur_stamp = 1;
            }
            let start = p.first_index as usize;
            let end = (start + p.index_count as usize).min(mesh.indices.len());
            let mut i = start;
            while i + 3 <= end {
                let (a, mut b, mut c) = (
                    mesh.indices[i] as usize,
                    mesh.indices[i + 1] as usize,
                    mesh.indices[i + 2] as usize,
                );
                i += 3;
                if a >= nv || b >= nv || c >= nv {
                    continue;
                }
                if flip_winding {
                    std::mem::swap(&mut b, &mut c);
                }
                if has_normals {
                    for &vi in &[a, b, c] {
                        if stamp[vi] != cur_stamp {
                            stamp[vi] = cur_stamp;
                            remap[vi] = batch.vertex_count() as u32;
                            let pos = world_pos[vi];
                            batch.push_vertex(&Vertex {
                                position: [pos.x, pos.y, pos.z],
                                element: p.element as f32,
                                normal: [world_nrm[vi].x, world_nrm[vi].y, world_nrm[vi].z],
                                _pad: 0.0,
                                uv: if has_uvs { mesh.uvs[vi] } else { [0.0, 0.0] },
                                _pad2: [0.0, 0.0],
                            });
                            batch.bounds = aabb_union_point(&batch.bounds, pos);
                            el_bounds = aabb_union_point(&el_bounds, pos);
                        }
                        batch.indices.push(remap[vi]);
                    }
                } else {
                    // No source normals: flat shading, so the three corners
                    // cannot be shared with the neighbouring triangle.
                    let flat = Vec3f::cross(
                        world_pos[b] - world_pos[a],
                        world_pos[c] - world_pos[a],
                    )
                    .normalize();
                    for &vi in &[a, b, c] {
                        let pos = world_pos[vi];
                        batch.indices.push(batch.vertex_count() as u32);
                        batch.push_vertex(&Vertex {
                            position: [pos.x, pos.y, pos.z],
                            element: p.element as f32,
                            normal: [flat.x, flat.y, flat.z],
                            _pad: 0.0,
                            uv: if has_uvs { mesh.uvs[vi] } else { [0.0, 0.0] },
                            _pad2: [0.0, 0.0],
                        });
                        batch.bounds = aabb_union_point(&batch.bounds, pos);
                        el_bounds = aabb_union_point(&el_bounds, pos);
                    }
                }
            }
            let range_count = batch.indices.len() as u32 - range_start;
            if range_count > 0 {
                batch.element_ranges.push(ElementRange {
                    element: ElementId::from_index(p.element as usize),
                    part: p.part,
                    first_index: range_start,
                    index_count: range_count,
                    draw_priority: 0,
                    coplanar_group: 0,
                });
                parts[p.part as usize] = Some(ScenePart {
                    id: p.part,
                    element: ElementId::from_index(p.element as usize),
                    element_part: p.element_part,
                    mesh: MeshId(p.mesh),
                    material: p.material,
                    batch: p.batch,
                    first_index: range_start,
                    index_count: range_count,
                    draw_priority: 0,
                    coplanar_group: 0,
                });
                let el = &mut elements[p.element as usize];
                el.ranges.push((p.batch, range_start, range_count));
                el.triangle_count += range_count / 3;
                el.bounds = aabb_union(&el.bounds, &el_bounds);
            }
            if pi % 256 == 0 {
                progress("geometry", 0.12 + 0.48 * (pi as f32 / total as f32));
            }
        }
        progress("geometry", 0.60);

        // ---- 4b. coplanar analysis + render-only metadata ----------------
        let mut parts: Vec<ScenePart> = parts
            .into_iter()
            .enumerate()
            .map(|(id, p)| {
                p.unwrap_or_else(|| {
                    let planned = placements
                        .iter()
                        .find(|p| p.part as usize == id)
                        .expect("part came from a placement");
                    ScenePart {
                        id: id as u32,
                        element: ElementId(planned.element),
                        element_part: planned.element_part,
                        mesh: MeshId(planned.mesh),
                        material: planned.material,
                        batch: planned.batch,
                        first_index: 0,
                        index_count: 0,
                        draw_priority: 0,
                        coplanar_group: 0,
                    }
                })
            })
            .collect();
        let mut overlap_report = overlap::analyze(
            &batches,
            &parts,
            &elements,
            &materials,
            DEFAULT_COPLANAR_TOL_M,
        );
        overlap::resolve(&mut overlap_report, &mut parts, &elements, &materials);
        for batch in &mut batches {
            for range in &mut batch.element_ranges {
                let Some(part) = parts.get(range.part as usize) else {
                    continue;
                };
                range.draw_priority = part.draw_priority;
                range.coplanar_group = part.coplanar_group;
                // A placement owns its vertices (the remap stamp resets per
                // placement), so only padding changes; position/index/query
                // data stays byte-for-byte canonical.
                let start = range.first_index as usize;
                let end = (start + range.index_count as usize).min(batch.indices.len());
                for &vertex in &batch.indices[start..end] {
                    let o = vertex as usize * VERTEX_STRIDE;
                    batch.vertices[o + 7] = part.draw_priority as f32;
                    batch.vertices[o + 10] = part.coplanar_group as f32;
                }
            }
        }
        progress("overlap", 0.62);

        // ---- 5. contours, per element, in element order --------------------
        let mut contours: Vec<f32> = Vec::new();
        let mut truncated = false;
        for (ei, e) in model.elements.iter().enumerate() {
            let first = (contours.len() / 6) as u32;
            let element_world = Mat4f::mul(&normalise, &e.transform);
            for mref in &e.meshes {
                let Some(mesh) = model.meshes.get(mref.mesh.index()) else {
                    continue;
                };
                if mesh.contour_edges.is_empty() {
                    continue;
                }
                let world = Mat4f::mul(&element_world, &mref.transform);
                let nv = mesh.positions.len();
                let mut k = 0;
                while k + 1 < mesh.contour_edges.len() {
                    let (a, b) = (
                        mesh.contour_edges[k] as usize,
                        mesh.contour_edges[k + 1] as usize,
                    );
                    k += 2;
                    if a >= nv || b >= nv {
                        continue;
                    }
                    if contours.len() / 6 >= MAX_CONTOUR_SEGMENTS {
                        truncated = true;
                        break;
                    }
                    let pa = transform_point(
                        &world,
                        Vec3f {
                            x: mesh.positions[a][0],
                            y: mesh.positions[a][1],
                            z: mesh.positions[a][2],
                        },
                    );
                    let pb = transform_point(
                        &world,
                        Vec3f {
                            x: mesh.positions[b][0],
                            y: mesh.positions[b][1],
                            z: mesh.positions[b][2],
                        },
                    );
                    contours.extend_from_slice(&[pa.x, pa.y, pa.z, pb.x, pb.y, pb.z]);
                }
            }
            let count = (contours.len() / 6) as u32 - first;
            elements[ei].contour_first = first;
            elements[ei].contour_count = count;
        }
        progress("contours", 0.66);

        // ---- 6. stories, layers, groups, tree ------------------------------
        let mut bounds = aabb_empty();
        for b in &batches {
            bounds = aabb_union(&bounds, &b.bounds);
        }
        let mut stories: Vec<Story> = model
            .stories
            .iter()
            .map(|s| Story {
                id: s.id,
                name: s.name.clone(),
                elevation: s.elevation * scale,
                height: s.height * scale,
                elements: Vec::new(),
                group: None,
            })
            .collect();
        let mut layers: Vec<Layer> = model
            .layers
            .iter()
            .map(|l| Layer {
                id: l.id,
                name: l.name.clone(),
                visible: l.visible,
                elements: Vec::new(),
            })
            .collect();

        // Story ranks by elevation (the exploded view's ordering).
        let mut order: Vec<usize> = (0..stories.len()).collect();
        order.sort_by(|a, b| {
            stories[*a]
                .elevation
                .partial_cmp(&stories[*b].elevation)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut story_rank = vec![0u32; stories.len()];
        for (rank, si) in order.iter().enumerate() {
            story_rank[*si] = rank as u32;
        }
        let mut story_spacing = 0.0f32;
        for w in order.windows(2) {
            story_spacing = story_spacing.max(stories[w[1]].elevation - stories[w[0]].elevation);
        }
        if story_spacing <= 0.01 {
            story_spacing = if aabb_is_empty(&bounds) {
                3.0
            } else {
                ((bounds.max.z - bounds.min.z) / stories.len().max(1) as f32).max(0.5)
            };
        }

        // Fab publishes a flat element list — no parent/child at all — so the
        // outliner gets its tree here: one group node per populated story.
        let flat = elements.iter().all(|e| e.parent.is_none());
        let mut group_nodes = 0u32;
        if flat && !stories.is_empty() && elements.len() > 1 {
            let mut used = vec![false; stories.len()];
            for e in elements.iter() {
                if let Some(s) = e.story {
                    if let Some(u) = used.get_mut(s.index()) {
                        *u = true;
                    }
                }
            }
            for si in order.iter().copied() {
                if !used[si] {
                    continue;
                }
                let id = ElementId::from_index(elements.len());
                let name = if stories[si].name.is_empty() {
                    format!("Storey {:+}", stories[si].elevation)
                } else {
                    stories[si].name.clone()
                };
                elements.push(Element {
                    id,
                    guid: String::new(),
                    name,
                    class: ElementClass::Group,
                    story: Some(stories[si].id),
                    layer: None,
                    parent: None,
                    bounds: aabb_empty(),
                    ranges: Vec::new(),
                    triangle_count: 0,
                    contour_first: 0,
                    contour_count: 0,
                    properties: Vec::new(),
                    quantities: Vec::new(),
                });
                stories[si].group = Some(id);
                group_nodes += 1;
            }
            for i in 0..elements.len() {
                if elements[i].is_group() {
                    continue;
                }
                if let Some(s) = elements[i].story {
                    if let Some(g) = stories.get(s.index()).and_then(|st| st.group) {
                        elements[i].parent = Some(g);
                    }
                }
            }
        }

        let n_all = elements.len();
        let mut tree = ElementTree {
            roots: Vec::new(),
            children: vec![Vec::new(); n_all],
        };
        // Roots come out groups-first (already in elevation order), then the
        // loose elements — a storeyless orphan should not push "Ground floor"
        // off the top of the outliner.
        let mut loose: Vec<ElementId> = Vec::new();
        for e in &elements {
            match e.parent {
                Some(p) if p.index() < n_all => tree.children[p.index()].push(e.id),
                _ if e.is_group() => tree.roots.push(e.id),
                _ => loose.push(e.id),
            }
            if let Some(s) = e.story {
                if let Some(st) = stories.get_mut(s.index()) {
                    if !e.is_group() {
                        st.elements.push(e.id);
                    }
                }
            }
            if let Some(l) = e.layer {
                if let Some(ly) = layers.get_mut(l.index()) {
                    ly.elements.push(e.id);
                }
            }
        }
        tree.roots.append(&mut loose);
        // Group bounds = the union of their children's, so framing a story works.
        for si in 0..stories.len() {
            if let Some(g) = stories[si].group {
                let mut b = aabb_empty();
                for c in &tree.children[g.index()] {
                    if elements[c.index()].has_geometry() {
                        b = aabb_union(&b, &elements[c.index()].bounds);
                    }
                }
                elements[g.index()].bounds = b;
            }
        }
        progress("tree", 0.70);

        // ---- 7. cameras ----------------------------------------------------
        let cameras: Vec<SceneCameraSet> = model
            .camera_sets
            .iter()
            .map(|s| SceneCameraSet {
                name: s.name.clone(),
                cameras: s
                    .cameras
                    .iter()
                    .map(|c| (c.name.clone(), scene_camera(c, &normalise, scale)))
                    .collect(),
            })
            .collect();
        let home_camera = model
            .home_camera
            .as_ref()
            .map(|c| scene_camera(c, &normalise, scale));

        // ---- 8. spatial index ---------------------------------------------
        let bvh = Bvh::build_with(&batches, &mut |f| progress("bvh", 0.70 + 0.28 * f));

        let stats = SceneStats {
            elements: n_source as u32,
            elements_with_geometry: elements.iter().filter(|e| e.has_geometry()).count() as u32,
            group_nodes,
            triangles: batches.iter().map(|b| b.triangle_count() as u32).sum(),
            vertices: batches.iter().map(|b| b.vertex_count() as u32).sum(),
            batches: batches.len() as u32,
            materials: materials.len() as u32,
            textures: textures.len() as u32,
            contour_segments: (contours.len() / 6) as u32,
            contours_truncated: truncated,
            bvh_nodes: bvh.node_count() as u32,
            build_ms: t0.elapsed().as_secs_f32() * 1000.0,
            geometry_bytes: batches
                .iter()
                .map(|b| (b.vertices.len() * 4 + b.indices.len() * 4) as u64)
                .sum::<u64>()
                + (contours.len() * 4) as u64,
        };
        progress("done", 1.0);

        let build_generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Scene {
            document: None,
            name: model.name,
            source_path: model.source_path,
            units: model.units,
            metadata: model.metadata,
            elements,
            materials,
            materials_are_derived,
            stories,
            layers,
            tree,
            batches,
            parts,
            overlap_report,
            bounds,
            bvh,
            contours,
            sheets: model.sheets,
            cameras,
            home_camera,
            stats,
            generation: build_generation,
            geometry_generation: build_generation,
            snapshot: OnceLock::new(),
            textures,
            story_rank,
            story_spacing,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    pub fn element(&self, id: ElementId) -> Option<&Element> {
        self.elements.get(id.index())
    }

    pub fn children(&self, id: ElementId) -> &[ElementId] {
        self.tree
            .children
            .get(id.index())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn material(&self, id: MaterialId) -> Option<&Material> {
        self.materials.get(id.index())
    }

    /// Live edit from the colour picker: set one material's base colour.
    /// The base colour is folded into the vertex tint at pack time (see
    /// `viewport/pack.rs`), so this only has to touch the material table and
    /// bump `generation` — the next `ensure_uploaded` repacks with the new
    /// colour. The cached snapshot is dropped for the same reason. Alpha is
    /// stored but batch transparency classes are fixed at build; an alpha
    /// crossing 1.0 does not re-sort batches.
    pub fn set_material_base_color(&mut self, id: MaterialId, rgba: [f32; 4]) -> bool {
        let Some(m) = self.materials.get_mut(id.index()) else {
            return false;
        };
        if m.base_color == rgba {
            return false;
        }
        m.base_color = rgba;
        // Keep the editable source in step, so a future document save writes
        // the colour the user is looking at. The scene's material order is
        // the document's (see `Document::into_model_data`), with one extra
        // "Default" appended at the end that has no document counterpart.
        if let Some(doc) = self.document.as_mut() {
            let doc = Arc::make_mut(doc);
            doc.set_material_base_color_by_index(id.index(), rgba);
        }
        self.snapshot = OnceLock::new();
        self.generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// The authored contour segments of one element: `6 * n` floats,
    /// `ax ay az bx by bz` per segment, world space.
    pub fn element_contours(&self, id: ElementId) -> &[f32] {
        match self.element(id) {
            Some(e) if e.contour_count > 0 => {
                let a = e.contour_first as usize * 6;
                let b = a + e.contour_count as usize * 6;
                &self.contours[a..b.min(self.contours.len())]
            }
            _ => &[],
        }
    }

    /// Position of a story in the elevation order (0 = lowest).
    pub fn story_rank(&self, id: StoryId) -> u32 {
        self.story_rank.get(id.index()).copied().unwrap_or(0)
    }

    /// Vertical distance the exploded view puts between two stories.
    pub fn story_spacing(&self) -> f32 {
        self.story_spacing
    }

    /// Bounds of a set of elements (own geometry only). `None` if none of
    /// them has geometry.
    pub fn bounds_of(&self, ids: impl IntoIterator<Item = ElementId>) -> Option<Aabb> {
        let mut b = aabb_empty();
        for id in ids {
            if let Some(e) = self.element(id) {
                if e.has_geometry() {
                    b = aabb_union(&b, &e.bounds);
                }
            }
        }
        if aabb_is_empty(&b) {
            None
        } else {
            Some(b)
        }
    }

    /// Story an element belongs to, walking up if needed.
    pub fn story_of(&self, id: ElementId) -> Option<&Story> {
        self.element(id)
            .and_then(|e| e.story)
            .and_then(|s| self.stories.get(s.index()))
    }

    /// Case-insensitive substring search over names, classes and GUIDs.
    pub fn search(&self, needle: &str) -> Vec<ElementId> {
        let n = needle.to_lowercase();
        if n.is_empty() {
            return Vec::new();
        }
        self.elements
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&n)
                    || e.class.label().to_lowercase().contains(&n)
                    || e.guid.to_lowercase().contains(&n)
            })
            .map(|e| e.id)
            .collect()
    }

    /// Elements grouped by class, most populous first — the outliner's funnel
    /// filter and the "select similar" command.
    pub fn group_by_class(&self) -> Vec<(ElementClass, Vec<ElementId>)> {
        let mut out: Vec<(ElementClass, Vec<ElementId>)> = Vec::new();
        for e in &self.elements {
            if e.is_group() {
                continue;
            }
            match out.iter_mut().find(|(c, _)| *c == e.class) {
                Some((_, v)) => v.push(e.id),
                None => out.push((e.class.clone(), vec![e.id])),
            }
        }
        out.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        out
    }

    /// Element with this source GUID (case-insensitive).
    pub fn element_by_guid(&self, guid: &str) -> Option<&Element> {
        self.elements
            .iter()
            .find(|e| e.guid.eq_ignore_ascii_case(guid))
    }

    // ---- queries -------------------------------------------------------

    /// **The** pick: closest visible hit along `ray`, honouring hidden /
    /// isolated / layer / story visibility, the section half-spaces *and* the
    /// exploded view — so a click always selects what is on screen.
    pub fn pick(&self, ray: &Ray, state: &SceneState) -> Option<RayHit> {
        if self.batches.is_empty() {
            return None;
        }
        let mask = state.visibility_mask(self);
        self.pick_masked(ray, state, &mask)
    }

    /// [`Scene::pick`] with a visibility mask the caller already built (hover
    /// runs per mouse move; the mask is the same one the lookup texture uses).
    pub fn pick_masked(&self, ray: &Ray, state: &SceneState, mask: &[bool]) -> Option<RayHit> {
        let visible = |e: ElementId| mask.get(e.index()).copied().unwrap_or(true);
        if state.explode.amount.abs() >= 1e-4 {
            return self.pick_exploded(ray, state, &visible);
        }
        if state.section.is_inactive() {
            return self.bvh.raycast(&self.batches, ray, &visible);
        }
        let keeps = |p: Vec3f| state.section.keeps(p);
        self.bvh.raycast_opt(
            &self.batches,
            ray,
            &PickOptions {
                visible: &visible,
                accept: Some(&keeps),
                max_t: f32::INFINITY,
                cull_backfaces: false,
            },
        )
    }

    /// Exploded picking: the BVH is built in assembled space, so each element
    /// is tested with the ray moved by −offset. Only elements whose *displaced*
    /// bounds the ray crosses are touched.
    fn pick_exploded(
        &self,
        ray: &Ray,
        state: &SceneState,
        visible: &dyn Fn(ElementId) -> bool,
    ) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        for e in &self.elements {
            if !e.has_geometry() || !visible(e.id) {
                continue;
            }
            let offset = state.explode.offset(self, e.id);
            let moved = Aabb {
                min: e.bounds.min + offset,
                max: e.bounds.max + offset,
            };
            match ray.intersect_aabb(&moved) {
                Some(t) if best.map_or(true, |h| t <= h.t) => {}
                _ => continue,
            }
            let local = Ray {
                origin: ray.origin - offset,
                dir: ray.dir,
            };
            for (bi, first, count) in &e.ranges {
                let batch = &self.batches[*bi as usize];
                let first_tri = first / 3;
                for t in first_tri..first_tri + count / 3 {
                    let (a, b, c) = batch.triangle(t);
                    let Some((th, u, v)) = local.intersect_triangle(a, b, c) else {
                        continue;
                    };
                    if best.map_or(false, |h| th >= h.t) {
                        continue;
                    }
                    let point = local.at(th) + offset;
                    if !state.section.keeps(point) {
                        continue;
                    }
                    let mut n = Vec3f::cross(b - a, c - a).normalize();
                    if n.dot(ray.dir) > 0.0 {
                        n = -n;
                    }
                    best = Some(RayHit {
                        element: e.id,
                        batch: *bi,
                        triangle: t,
                        t: th,
                        point,
                        normal: n,
                        bary: [u, v],
                    });
                }
            }
        }
        best
    }

    /// Elements whose world bounds intersect the frustum and that are visible.
    /// The renderer culls by batch; this is what the status bar and the tools
    /// count.
    pub fn visible_elements(
        &self,
        frustum: &Frustum,
        state: &SceneState,
        out: &mut Vec<ElementId>,
    ) {
        self.bvh.frustum_elements(&self.batches, frustum, out);
        out.retain(|e| state.is_visible(self, *e));
    }

    /// Batches whose bounds intersect the frustum — the draw list. Batches are
    /// the smallest unit the GPU can skip (a geometry's index buffer is drawn
    /// whole), which is why `from_model` splits them spatially.
    pub fn visible_batches(&self, frustum: &Frustum, out: &mut Vec<u32>) {
        out.clear();
        for (i, b) in self.batches.iter().enumerate() {
            if !aabb_is_empty(&b.bounds) && frustum.intersects_aabb(&b.bounds) {
                out.push(i as u32);
            }
        }
    }

    /// Snap the cursor to the model. See [`crate::model::query::snap`].
    pub fn snap(
        &self,
        ray: &Ray,
        state: &SceneState,
        opts: &SnapOptions,
        project: ScreenProject,
        cursor: [f32; 2],
    ) -> Option<SnapHit> {
        crate::model::query::snap(self, ray, state, opts, project, cursor)
    }

    /// Renderer-facing flat snapshot, built once on first request (do that on
    /// the loader thread: call it right after `from_model`).
    pub fn snapshot(&self) -> Arc<SceneSnapshot> {
        self.snapshot
            .get_or_init(|| Arc::new(SceneSnapshot::from_scene(self)))
            .clone()
    }
}

/// Assign every placement a batch, keyed by (material × spatial cell), and
/// return the batch descriptors in draw order (opaque first, transparent last).
fn plan_batches(
    placements: &mut [Placement],
    materials: &[Material],
) -> Vec<(MaterialId, bool)> {
    // Per material: how many triangles, and where they are.
    let mut tris: HashMap<u32, (usize, Aabb)> = HashMap::new();
    for p in placements.iter() {
        let e = tris
            .entry(p.material.0)
            .or_insert((0, aabb_empty()));
        e.0 += p.triangles as usize;
        e.1 = aabb_union_point(&e.1, p.center);
    }
    // Grid divisions per material: enough cells that a batch is roughly
    // TARGET_BATCH_TRIANGLES, and never more than 6 per axis (216 cells).
    let grid: HashMap<u32, (u32, Aabb)> = tris
        .iter()
        .map(|(m, (t, b))| {
            let want = (*t as f32 / TARGET_BATCH_TRIANGLES as f32).ceil().max(1.0);
            let n = (want.cbrt().ceil() as u32).clamp(1, 6);
            (*m, (n, *b))
        })
        .collect();

    let cell_of = |p: &Placement| -> u32 {
        let Some((n, b)) = grid.get(&p.material.0) else {
            return 0;
        };
        if *n <= 1 || aabb_is_empty(b) {
            return 0;
        }
        let e = b.max - b.min;
        let f = |v: f32, lo: f32, ext: f32| -> u32 {
            if ext <= 1e-6 {
                0
            } else {
                (((v - lo) / ext * *n as f32) as i32).clamp(0, *n as i32 - 1) as u32
            }
        };
        let (x, y, z) = (
            f(p.center.x, b.min.x, e.x),
            f(p.center.y, b.min.y, e.y),
            f(p.center.z, b.min.z, e.z),
        );
        (z * n * n) + (y * n) + x
    };

    let transparent_of = |m: MaterialId| -> bool {
        materials
            .get(m.index())
            .map(|mm| mm.transparent)
            .unwrap_or(false)
    };

    // Sort into draw order: opaque first, then by material, then by cell. That
    // is also the emission order, so each batch's element ranges come out
    // sorted by `first_index` and `element_of_triangle` can binary-search.
    let mut keyed: Vec<(bool, u32, u32, usize)> = placements
        .iter()
        .enumerate()
        .map(|(i, p)| (transparent_of(p.material), p.material.0, cell_of(p), i))
        .collect();
    keyed.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));

    let mut descriptors: Vec<(MaterialId, bool)> = Vec::new();
    let mut cur_key: Option<(bool, u32, u32)> = None;
    let mut cur_vertices = 0usize;
    let mut assign: Vec<u32> = vec![0; placements.len()];
    for (transparent, material, cell, i) in keyed {
        let key = (transparent, material, cell);
        // Upper bound on the vertices this placement adds.
        let need = placements[i].index_count as usize;
        let split = match cur_key {
            Some(k) if k == key => cur_vertices + need > MAX_BATCH_VERTICES,
            _ => true,
        };
        if split {
            descriptors.push((MaterialId(material), transparent));
            cur_key = Some(key);
            cur_vertices = 0;
        }
        cur_vertices += need;
        assign[i] = descriptors.len() as u32 - 1;
    }
    for (i, p) in placements.iter_mut().enumerate() {
        p.batch = assign[i];
    }
    // Emission must follow the same order the plan sorted into.
    placements.sort_by_key(|p| p.batch);
    descriptors
}

fn scene_camera(c: &CameraData, normalise: &Mat4f, scale: f32) -> SceneCamera {
    let eye = transform_point(
        normalise,
        Vec3f {
            x: c.position[0],
            y: c.position[1],
            z: c.position[2],
        },
    );
    let _ = scale;
    // yaw 0 looks along +Y; positive yaw rotates counter-clockwise about up.
    let (sy, cy) = (c.yaw.sin(), c.yaw.cos());
    let cp = c.pitch.cos();
    let forward = transform_dir(
        normalise,
        Vec3f {
            x: -sy * cp,
            y: cy * cp,
            z: c.pitch.sin(),
        },
    )
    .normalize();
    let up = transform_dir(
        normalise,
        Vec3f {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        },
    )
    .normalize();
    SceneCamera {
        eye,
        forward,
        up,
        fov_y_deg: if c.fov_y > 0.01 {
            c.fov_y.to_degrees()
        } else {
            60.0
        },
        perspective: c.perspective,
        sun: c.sun,
    }
}

fn material_from_data(id: MaterialId, m: &MaterialData, texture: Option<u32>) -> Material {
    Material {
        id,
        name: m.name.clone(),
        base_color: m.base_color,
        metallic: m.metallic,
        roughness: m.roughness,
        emissive: m.emissive,
        ior: m.ior,
        transmission: m.transmission,
        double_sided: m.double_sided,
        transparent: m.base_color[3] < 0.999 || m.transmission > 0.001,
        texture,
    }
}

/// Source → scene: scale to meters and rotate Y-up sources to Z-up.
fn normalisation_matrix(up: UpAxis, scale: f32) -> Mat4f {
    let s = Mat4f {
        v: [
            scale, 0.0, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };
    match up {
        UpAxis::Z => s,
        // Y-up → Z-up: (x, y, z) → (x, -z, y)
        UpAxis::Y => {
            let r = Mat4f {
                v: [
                    1.0, 0.0, 0.0, 0.0, // col 0
                    0.0, 0.0, 1.0, 0.0, // col 1: y → z
                    0.0, -1.0, 0.0, 0.0, // col 2: z → -y
                    0.0, 0.0, 0.0, 1.0,
                ],
            };
            Mat4f::mul(&r, &s)
        }
    }
}

/// Bytes one vertex costs in the CPU-side batches (documented so the budget
/// line can be checked directly from code).
pub const VERTEX_BYTES: usize = VERTEX_STRIDE * 4;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::state::{ExplodeMode, ExplodeState, SectionPlane, SectionState};
    use makepad_math::Plane;

    #[test]
    fn demo_house_builds() {
        let model = crate::model::demo::demo_house();
        let scene = Scene::from_model(model, &mut |_| {});
        assert!(scene.stats.triangles > 0);
        assert!(!aabb_is_empty(&scene.bounds));
        assert!(scene.bounds.max.z > scene.bounds.min.z);
        assert!(scene.elements.iter().any(|e| e.class == ElementClass::Wall));
        let snap = scene.snapshot();
        assert_eq!(snap.indices.len() as u32, scene.stats.triangles * 3);
        // The demo house publishes its own materials, so nothing is derived.
        assert!(!scene.materials_are_derived);
    }

    #[test]
    fn y_up_source_becomes_z_up() {
        let mut model = crate::model::demo::demo_house();
        model.up_axis = UpAxis::Y;
        let scene = Scene::from_model(model, &mut |_| {});
        let ext = crate::model::bounds::aabb_extent(&scene.bounds);
        assert!(ext.z > 0.0);
    }

    #[test]
    fn shared_vertices_are_not_duplicated() {
        // Every box in the demo house has 24 vertices and 12 triangles; with
        // per-placement de-duplication the vertex count must stay far below
        // 3 × triangles.
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        assert!(
            scene.stats.vertices < scene.stats.triangles * 3,
            "{} verts for {} tris",
            scene.stats.vertices,
            scene.stats.triangles
        );
    }

    #[test]
    fn overlap_resolution_only_changes_render_padding() {
        use crate::model::model::{ElementData, MeshData, MeshRef, Property, PropertyValue};

        let source_positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let model = ModelData {
            name: "canonical overlap".into(),
            meshes: vec![MeshData {
                positions: source_positions.clone(),
                indices: vec![0, 1, 2],
                ..Default::default()
            }],
            elements: vec![
                ElementData {
                    id: ElementId(0),
                    name: "lower-priority face".into(),
                    class: ElementClass::Unknown,
                    meshes: vec![MeshRef::identity(MeshId(0))],
                    properties: vec![Property {
                        group: "Properties".into(),
                        name: "arch.priority".into(),
                        value: PropertyValue::Number(1.0),
                    }],
                    ..Default::default()
                },
                ElementData {
                    id: ElementId(1),
                    name: "higher-priority face".into(),
                    class: ElementClass::Unknown,
                    meshes: vec![MeshRef::identity(MeshId(0))],
                    properties: vec![Property {
                        group: "Properties".into(),
                        name: "arch.priority".into(),
                        value: PropertyValue::Number(9.0),
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let scene = Scene::from_model(model, &mut |_| {});
        assert_eq!(scene.overlap_report.pairs.len(), 1);
        let overlap = &scene.overlap_report.pairs[0];
        assert!(overlap.draw_priority_b > overlap.draw_priority_a);

        for part in &scene.parts {
            let batch = &scene.batches[part.batch as usize];
            let tri = &batch.indices
                [part.first_index as usize..(part.first_index + part.index_count) as usize];
            let actual: Vec<[f32; 3]> = tri
                .iter()
                .map(|&i| batch.vertex(i).position)
                .collect();
            assert_eq!(actual, source_positions, "canonical triangle was moved");
        }
        let ray = Ray::new(
            Vec3f { x: 0.2, y: 0.2, z: 1.0 },
            Vec3f { x: 0.0, y: 0.0, z: -1.0 },
        );
        let hit = scene.pick(&ray, &SceneState::default()).expect("canonical pick");
        assert!((hit.point.z).abs() < 1.0e-7, "render bias leaked into picking");
        assert!((crate::model::query::polygon_area(&[
            Vec3f { x: 0.0, y: 0.0, z: 0.0 },
            Vec3f { x: 1.0, y: 0.0, z: 0.0 },
            Vec3f { x: 0.0, y: 1.0, z: 0.0 },
        ]) - 0.5)
            .abs()
            < 1.0e-7);
    }

    #[test]
    fn element_ranges_stay_sorted_per_batch() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        for b in &scene.batches {
            let mut last = 0;
            for r in &b.element_ranges {
                assert!(r.first_index >= last, "ranges out of order");
                last = r.first_index + r.index_count;
            }
            assert_eq!(last as usize, b.indices.len());
            // Every triangle resolves back to its element.
            for t in [0u32, (b.triangle_count() as u32).saturating_sub(1)] {
                if b.triangle_count() > 0 {
                    assert!(b.element_of_triangle(t).is_some());
                }
            }
        }
    }

    #[test]
    fn transparent_batches_come_last() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let mut seen_transparent = false;
        for b in &scene.batches {
            if b.transparent {
                seen_transparent = true;
            } else {
                assert!(!seen_transparent, "opaque batch after a transparent one");
            }
        }
    }

    #[test]
    fn pick_respects_visibility_and_sections() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let c = aabb_center(&scene.bounds);
        let ray = Ray::new(
            Vec3f {
                x: c.x,
                y: c.y,
                z: scene.bounds.max.z + 50.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let mut state = SceneState::default();
        let hit = scene.pick(&ray, &state).expect("roof hit");
        // Cut everything above the hit away: the pick must fall through to
        // whatever is below, never back to the removed roof.
        state.section = SectionState {
            enabled: true,
            planes: vec![SectionPlane {
                plane: Plane {
                    a: 0.0,
                    b: 0.0,
                    c: -1.0,
                    d: hit.point.z - 0.05,
                },
                enabled: true,
                source: None,
            }],
            boxed: None,
            caps: true,
            cap_color: [0.5, 0.5, 0.5, 1.0],
        };
        let under = scene.pick(&ray, &state);
        if let Some(u) = under {
            assert!(u.point.z <= hit.point.z - 0.04, "picked the clipped half");
        }
        // Hiding the element that was hit must expose something else or nothing.
        let mut hidden = SceneState::default();
        hidden.set_hidden(hit.element, true);
        let after = scene.pick(&ray, &hidden);
        assert!(after.map_or(true, |h| h.element != hit.element));
    }

    #[test]
    fn pick_follows_the_exploded_view() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let mut state = SceneState::default();
        // Find something on a story that will move.
        let c = aabb_center(&scene.bounds);
        let ray = Ray::new(
            Vec3f {
                x: c.x,
                y: c.y,
                z: scene.bounds.max.z + 50.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let before = scene.pick(&ray, &state).expect("hit");
        state.set_explode(ExplodeState {
            amount: 1.0,
            mode: ExplodeMode::ByStory,
        });
        let after = scene.pick(&ray, &state).expect("hit while exploded");
        let offset = state.explode.offset(&scene, after.element);
        // The reported point is in displaced space, so it must sit on the
        // displaced element, not the assembled one.
        let b = scene.element(after.element).unwrap().bounds;
        assert!(after.point.z <= b.max.z + offset.z + 1e-2);
        assert!(after.point.z >= b.min.z + offset.z - 1e-2);
        let _ = before;
    }

    #[test]
    fn contours_are_sliced_per_element() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let mut total = 0;
        for e in &scene.elements {
            let c = scene.element_contours(e.id);
            assert_eq!(c.len() % 6, 0);
            total += c.len();
        }
        assert_eq!(total, scene.contours.len());
    }

    #[test]
    fn story_groups_only_appear_for_flat_sources() {
        // The demo house publishes its own hierarchy → nothing synthesised.
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        assert_eq!(scene.stats.group_nodes, 0);

        // Strip the hierarchy and the story tree comes back.
        let mut model = crate::model::demo::demo_house();
        for e in &mut model.elements {
            e.parent = None;
        }
        let scene = Scene::from_model(model, &mut |_| {});
        assert!(scene.stats.group_nodes > 0);
        assert_eq!(scene.stats.elements, scene.elements.len() as u32 - scene.stats.group_nodes);
        for r in &scene.tree.roots {
            let e = scene.element(*r).unwrap();
            assert!(e.is_group() || e.story.is_none(), "{} is a loose root", e.name);
        }
    }
}
