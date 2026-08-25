//! Lane B. `fab::model::RenderBatch` → `makepad_render::StaticModel`.
//!
//! Two conversions happen here and nowhere else:
//!
//! 1. **Axis.** Fab world is Z up (source application / Fab). `libs/render` is
//!    Y up (its ground slab, its hemisphere ambient, its sun). Every position
//!    and normal handed to the renderer goes through [`to_render`]; the
//!    camera, the sun and the explode offsets take the same turn, so what the
//!    renderer sees is one rigid rotation of the Fab world and the composed
//!    view-projection over Fab points is unchanged — which is what keeps the
//!    realtime and path-traced viewports pixel-identical (§L2' parity gate).
//!    Nothing else in the app ever converts: picking, measuring and the BVH
//!    stay in Z-up meters.
//!
//! 2. **Vertex packing.** `GameMeshVertexAo` is 7 floats: pos.xyz, oct-normal,
//!    f16 uv, unorm8 colour+AO, unorm16x2 `ao_uv`. Fab has no lightmap
//!    charts, so the `ao_uv` lane carries the **element index** instead — the
//!    per-element lookup the static shader reads when `elem_ctl.w` is on
//!    (`libs/render/src/shaders.rs`, DrawSceneSkinned). That is what lets
//!    hide / isolate / explode resolve on the GPU with zero re-upload.
//!
//! Material base colour is folded into the vertex tint. Published materials
//! are partitioned by `(albedo image, metallic, roughness)` so the renderer
//! uploads each decoded image and preserves the MAT surface factors; models
//! without published textures keep the original one-draw merged path.

use crate::api::*;
use makepad_render::{pack_ao_uv, PbrMaterial, StaticDrawLayer, StaticModel};
use makepad_widgets::*;
use makepad_zune_png::makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_png::makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_png::makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;

/// Same octahedral encode as `libs/render/src/skin.rs` (`pub(crate)` there).
/// The static-mesh shader's `oct_decode` is the inverse.
fn oct_encode(n: Vec3f) -> (f32, f32) {
    let l1 = n.x.abs() + n.y.abs() + n.z.abs();
    if l1 < 1.0e-8 {
        return (0.0, 0.0);
    }
    let (x, y, z) = (n.x / l1, n.y / l1, n.z / l1);
    if z >= 0.0 {
        (x, y)
    } else {
        let sx = if x >= 0.0 { 1.0 } else { -1.0 };
        let sy = if y >= 0.0 { 1.0 } else { -1.0 };
        ((1.0 - y.abs()) * sx, (1.0 - x.abs()) * sy)
    }
}

/// Fab world (Z up, right-handed) → `libs/render` world (Y up, right-handed).
/// Determinant +1, so winding and normals survive.
#[inline]
pub fn to_render(v: Vec3f) -> Vec3f {
    vec3(v.x, v.z, -v.y)
}

/// The inverse of [`to_render`].
#[inline]
pub fn from_render(v: Vec3f) -> Vec3f {
    vec3(v.x, -v.z, v.y)
}

/// The merged triangle soup, in Fab space, in exactly the order
/// [`pack_scene`] packs it. The AO bake runs over this on a worker thread and
/// its result indexes straight back into the packed vertex stream.
pub struct MergedMesh {
    pub positions: Vec<Vec3f>,
    pub normals: Vec<Vec3f>,
    pub indices: Vec<u32>,
}

/// Walk the batches in batch order, vertex order. Both the packer and the AO
/// worker call this, so vertex `i` means the same thing in both.
pub fn merge(scene: &Scene) -> MergedMesh {
    let mut vcount = 0usize;
    let mut icount = 0usize;
    for b in &scene.batches {
        vcount += b.vertex_count();
        icount += b.indices.len();
    }
    let mut m = MergedMesh {
        positions: Vec::with_capacity(vcount),
        normals: Vec::with_capacity(vcount),
        indices: Vec::with_capacity(icount),
    };
    for b in &scene.batches {
        let base = m.positions.len() as u32;
        for v in 0..b.vertex_count() as u32 {
            m.positions.push(b.position(v));
            let n = b.normal(v);
            m.normals.push(if n.length() > 1e-6 {
                n.normalize()
            } else {
                vec3(0.0, 0.0, 1.0)
            });
        }
        for i in &b.indices {
            m.indices.push(base + *i);
        }
    }
    m
}

/// Element index of packed vertex `i`, batch order.
pub fn merged_elements(scene: &Scene) -> Vec<u32> {
    let mut out = Vec::new();
    for b in &scene.batches {
        for v in 0..b.vertex_count() as u32 {
            out.push(b.element(v).0);
        }
    }
    out
}

/// Pack the whole scene into one vertex-coloured `StaticModel`.
///
/// `ao` — one occlusion value per merged vertex, `None` before the bake
/// lands (everything fully lit). Re-packing with a fresh `ao` is the only
/// reason to build a second model for the same scene.
fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if rgba.len() != expected {
        return None;
    }
    let options = EncoderOptions::default()
        .set_width(width as usize)
        .set_height(height as usize)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(&rgba[..expected], options);
    let mut out = Vec::new();
    encoder.encode(&mut out).ok()?;
    Some(out)
}

pub fn pack_scene(scene: &Scene, ao: Option<&[f32]>) -> StaticModel {
    let mut vertices: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut min = vec3(f32::MAX, f32::MAX, f32::MAX);
    let mut max = vec3(f32::MIN, f32::MIN, f32::MIN);
    let mut vi = 0usize;

    // PNG-encoding every decoded texture is by far the most expensive step
    // of a pack, and textures never change on a material edit — only the
    // tint does. Cache the encodes by pixel-buffer identity so a live
    // colour drag repacks vertices without re-encoding a single image.
    let pngs: Vec<Option<Vec<u8>>> = {
        use std::collections::HashMap;
        use std::sync::Mutex;
        static PNG_CACHE: Mutex<Option<HashMap<(usize, usize), Option<Vec<u8>>>>> =
            Mutex::new(None);
        let mut guard = PNG_CACHE.lock().unwrap();
        let cache = guard.get_or_insert_with(HashMap::new);
        if cache.len() > 512 {
            cache.clear();
        }
        scene
            .textures
            .iter()
            .map(|t| {
                let key = (t.rgba.as_ptr() as usize, t.rgba.len());
                cache
                    .entry(key)
                    .or_insert_with(|| encode_png_rgba(t.width, t.height, &t.rgba))
                    .clone()
            })
            .collect()
    };
    let textured = pngs.iter().any(|p| p.is_some());
    let layered = textured
        || (!scene.materials_are_derived
            && scene
                .materials
                .iter()
                .any(|m| m.metallic > 0.0 || m.roughness < 0.99));

    struct Layer {
        texture: Option<u32>,
        pbr: PbrMaterial,
        png: Option<Vec<u8>>,
        vertices: Vec<f32>,
        indices: Vec<u32>,
        uvs: Vec<[f32; 2]>,
    }
    let mut layers: Vec<Layer> = Vec::new();

    for b in &scene.batches {
        let mat = scene.material(b.material);
        let tex = mat.and_then(|m| m.texture);
        let (mr, mg, mb) = match mat {
            Some(m) => (m.base_color[0], m.base_color[1], m.base_color[2]),
            None => (0.72, 0.72, 0.70),
        };
        let texture = tex.filter(|t| {
            (*t as usize) < pngs.len() && pngs[*t as usize].is_some()
        });
        let pbr = PbrMaterial {
            metallic: mat.map(|m| m.metallic).unwrap_or(0.0),
            roughness: mat.map(|m| m.roughness).unwrap_or(1.0),
            orm_png: None,
        };
        let layer_i = if layered {
            layers
                .iter()
                .position(|layer| {
                    layer.texture == texture
                        && layer.pbr.metallic.to_bits() == pbr.metallic.to_bits()
                        && layer.pbr.roughness.to_bits() == pbr.roughness.to_bits()
                })
                .unwrap_or_else(|| {
                    let png = texture.and_then(|t| pngs.get(t as usize).cloned().flatten());
                    layers.push(Layer {
                        texture,
                        pbr: pbr.clone(),
                        png,
                        vertices: Vec::new(),
                        indices: Vec::new(),
                        uvs: Vec::new(),
                    });
                    layers.len() - 1
                })
        } else {
            0
        };
        let mut pack_into = |
            verts: &mut Vec<f32>,
            inds: &mut Vec<u32>,
            mut raw_uvs: Option<&mut Vec<[f32; 2]>>,
            vi: &mut usize,
        | {
            let base = (verts.len() / makepad_render::MODEL_VERTEX_FLOATS) as u32;
            for v in 0..b.vertex_count() as u32 {
                let p = to_render(b.position(v));
                let n = b.normal(v);
                let n = to_render(if n.length() > 1e-6 {
                    n.normalize()
                } else {
                    vec3(0.0, 0.0, 1.0)
                });
                let uv = b.uv(v);
                if let Some(raw) = raw_uvs.as_deref_mut() {
                    raw.push(uv);
                }
                let vertex = b.vertex(v);
                let elem = vertex.element_id().0;
                let priority = vertex._pad.max(0.0).min(4095.0) as u16;
                let occ = ao.and_then(|a| a.get(*vi).copied()).unwrap_or(1.0);
                let (ox, oy) = oct_encode(n);
                verts.extend_from_slice(&[
                    p.x,
                    p.y,
                    p.z,
                    makepad_widgets::makepad_draw::pack_pair_f16(ox, oy),
                    makepad_widgets::makepad_draw::pack_pair_f16(uv[0], uv[1]),
                    makepad_widgets::makepad_draw::pack_unorm8x4(mr, mg, mb, occ),
                    pack_element_priority(elem, priority),
                ]);
                min.x = min.x.min(p.x);
                min.y = min.y.min(p.y);
                min.z = min.z.min(p.z);
                max.x = max.x.max(p.x);
                max.y = max.y.max(p.y);
                max.z = max.z.max(p.z);
                *vi += 1;
            }
            for i in &b.indices {
                inds.push(base + *i);
            }
        };
        // Always fill the merged stream (AO bake / single-draw path).
        pack_into(&mut vertices, &mut indices, None, &mut vi);
        if layered {
            // Layer packing must not re-consume the AO cursor — restore it.
            vi -= b.vertex_count();
            let layer = &mut layers[layer_i];
            pack_into(
                &mut layer.vertices,
                &mut layer.indices,
                Some(&mut layer.uvs),
                &mut vi,
            );
        }
    }
    if vertices.is_empty() {
        min = vec3(0.0, 0.0, 0.0);
        max = vec3(1.0, 1.0, 1.0);
    }
    let mut draw_layers: Vec<StaticDrawLayer> = Vec::new();
    let mut texture_png = None;
    let mut model_pbr = PbrMaterial {
        metallic: 0.0,
        roughness: 1.0,
        orm_png: None,
    };
    if layered {
        draw_layers = layers
            .into_iter()
            .filter(|layer| layer.indices.len() >= 3)
            .map(|layer| StaticDrawLayer {
                uvs: layer.uvs,
                vertices: layer.vertices,
                indices: layer.indices,
                texture_png: layer.png,
                detail_png: None,
                detail_scale: [0.0, 0.0],
                pbr: layer.pbr,
            })
            .collect();
        // A single textured draw stays on the merged stream + `texture_png`
        // (the renderer only splits when `draw_layers.len() > 1`).
        if draw_layers.len() == 1 {
            let layer = draw_layers.pop().unwrap();
            texture_png = layer.texture_png;
            model_pbr = layer.pbr;
            draw_layers.clear();
        }
    }
    StaticModel {
        vertices,
        indices,
        texture_uri: None,
        texture_png,
        min,
        max,
        parts: Vec::new(),
        ground_ao: None,
        draw_layers,
        detail_png: None,
        detail_scale: [0.0, 0.0],
        prelit: false,
        anim_parts: Vec::new(),
        driven_parts: Vec::new(),
        sky: None,
        pbr: model_pbr,
    }
}

/// Element index into the `ao_uv` lane: unorm16x2, low half in the first
/// axis. The static shader's element hook decodes exactly this.
#[inline]
pub fn pack_element(id: u32) -> f32 {
    let lo = (id & 0xFFFF) as f32 / 65535.0;
    let hi = ((id >> 16) & 0xFFFF) as f32 / 65535.0;
    pack_ao_uv(lo, hi)
}

/// Pack a 20-bit element id plus a 12-bit part priority into the existing
/// unorm16x2 lane. The element limit is far above Fab's practical range;
/// no vertex stride or GPU dispatch grows.
#[inline]
pub fn pack_element_priority(id: u32, priority: u16) -> f32 {
    debug_assert!(id < (1 << 20), "Fab element id exceeds packed render lane");
    let code = ((priority.min(4095) as u32) << 20) | (id & 0x000f_ffff);
    pack_element(code)
}

/// A stable content hash of the scene's geometry — the key the AO cache is
/// filed under. Samples the vertex stream rather than hashing 100 MB of it.
pub fn scene_hash(scene: &Scene) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x100_0000_01b3);
    };
    eat(scene.elements.len() as u64);
    eat(scene.stats.triangles as u64);
    eat(scene.stats.vertices as u64);
    for b in &scene.batches {
        eat(b.vertices.len() as u64);
        eat(b.indices.len() as u64);
        let n = b.vertices.len();
        let step = (n / 512).max(1);
        let mut i = 0;
        while i < n {
            eat(b.vertices[i].to_bits() as u64);
            i += step;
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_turn_round_trips_and_keeps_handedness() {
        let v = vec3(1.0, 2.0, 3.0);
        let r = from_render(to_render(v));
        assert!((r.x - v.x).abs() < 1e-6 && (r.y - v.y).abs() < 1e-6 && (r.z - v.z).abs() < 1e-6);
        // Z up becomes Y up.
        let up = to_render(vec3(0.0, 0.0, 1.0));
        assert!(up.y > 0.99);
        // Right-handed stays right-handed: x cross y == z.
        let x = to_render(vec3(1.0, 0.0, 0.0));
        let y = to_render(vec3(0.0, 1.0, 0.0));
        let z = to_render(vec3(0.0, 0.0, 1.0));
        let c = Vec3f::cross(x, y);
        assert!((c.x - z.x).abs() < 1e-6 && (c.y - z.y).abs() < 1e-6 && (c.z - z.z).abs() < 1e-6);
    }

    #[test]
    fn element_survives_the_ao_uv_lane() {
        for id in [0u32, 1, 255, 706, 65535, 65536, 1_000_000] {
            let packed = makepad_render::unpack_ao_uv(pack_element(id));
            let lo = (packed[0] * 65535.0).round() as u32;
            let hi = (packed[1] * 65535.0).round() as u32;
            assert_eq!(lo + hi * 65536, id, "element {id} did not survive packing");
        }
    }

    #[test]
    fn element_and_part_priority_share_the_lane_exactly() {
        for (id, priority) in [(0u32, 0u16), (706, 17), (1_000_000, 4095)] {
            let packed = makepad_render::unpack_ao_uv(pack_element_priority(id, priority));
            let lo = (packed[0] * 65535.0).round() as u32;
            let hi = (packed[1] * 65535.0).round() as u32;
            let code = lo + hi * 65536;
            assert_eq!(code & 0x000f_ffff, id);
            assert_eq!(code >> 20, priority as u32);
        }
    }

    #[test]
    fn packs_the_demo_house() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let m = pack_scene(&scene, None);
        assert_eq!(
            m.vertices.len() % makepad_render::MODEL_VERTEX_FLOATS,
            0,
            "vertex stream must be a whole number of vertices"
        );
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.len() >= 3);
        // Z up becomes Y up: the house is taller in render-y than the
        // scene's z extent is wide in render-z.
        assert!(m.max.y > m.min.y);
        let merged = merge(&scene);
        assert_eq!(
            merged.positions.len(),
            m.vertices.len() / makepad_render::MODEL_VERTEX_FLOATS,
            "merge() and pack_scene() must agree on vertex order"
        );
    }

    #[test]
    fn published_texture_and_surface_factors_reach_realtime_layers() {
        let mut source = crate::model::demo::demo_house();
        source.textures.push(crate::model::TextureData {
            width: 2,
            height: 2,
            rgba: [255, 64, 16, 255].repeat(4),
        });
        source.materials[0].texture = Some(0);
        source.materials[0].metallic = 0.75;
        source.materials[0].roughness = 0.2;
        let scene = Scene::from_model(source, &mut |_| {});
        let model = pack_scene(&scene, None);

        let textured = model.draw_layers.iter().find(|layer| layer.texture_png.is_some());
        if let Some(layer) = textured {
            assert_eq!(layer.uvs.len(), layer.vertices.len() / makepad_render::MODEL_VERTEX_FLOATS);
            assert!((layer.pbr.metallic - 0.75).abs() < 1e-6);
            assert!((layer.pbr.roughness - 0.2).abs() < 1e-6);
            assert!(layer.texture_png.as_ref().is_some_and(|png| png.starts_with(&[0x89, b'P', b'N', b'G'])));
        } else {
            // A one-material scene is represented by the model-level lane.
            assert!(model.texture_png.as_ref().is_some_and(|png| png.starts_with(&[0x89, b'P', b'N', b'G'])));
            assert!((model.pbr.metallic - 0.75).abs() < 1e-6);
            assert!((model.pbr.roughness - 0.2).abs() < 1e-6);
        }
    }
}
