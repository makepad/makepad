//! The world's generic items (KERNELS.md §3.2) on the renderer's model
//! lanes. A host makes geometry resident once ([`Renderer::register_geometry`],
//! keyed by the `GeometryId` its items name); each frame, every Mesh and
//! Instances item becomes model instances drawn by the same lanes, shadows
//! and clustered lights as a game's props. A (geometry, material) pair is
//! one uploaded model, built on first use: the base colour rides the
//! instance tint, so only the rest of the material keys it. Unlit, blended,
//! masked and IBL items draw through built-in Splash materials
//! (custom_material.rs); a Splash material is drawn by the program a host
//! installed under [`splash_material_name`].
//!
//! Lines, points and cards are the host's (render-lines, the compositor):
//! the model lanes do not draw them. Kernel-written buffers resolve through
//! [`Renderer::resolve_buffer`] (makepad-render-kernels).
use super::*;
use crate::custom_material::DrawSceneCustom;
use crate::material_surface::MaterialSurface;
use makepad_render_material::{HookMask, HookSet, MaterialDesc};
use makepad_scene::{
    BaseKind, Blend, BufferRef, GeometryId, GeometryRef, InstanceSource, ItemFlags, ItemKind, LayoutId, LightingModel,
    MaterialFrame, MaterialKind, Side, TextureRef,
};
use crate::material_surface::{PixelSemantic, PreparedTexture};
use std::collections::HashMap;
use std::sync::Arc;

/// Packed instance records: a column-major model matrix (16 floats) then a
/// linear RGBA tint (4), per instance.
pub const LAYOUT_TRANSFORM_TINT: LayoutId = LayoutId(1);
pub const TRANSFORM_TINT_FLOATS: usize = 20;

/// The custom-material name a host installs a Splash program under.
pub fn splash_material_name(program: makepad_scene::MaterialProgramId) -> String {
    format!("program:{}", program.0)
}

/// Model-space triangles with per-vertex normal, uv and linear colour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Linear RGBA, 0..1; empty = white.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl GeometryData {
    pub fn validate(&self) -> Result<(), String> {
        let n = self.positions.len();
        if n == 0 || self.indices.is_empty() || self.indices.len() % 3 != 0 {
            return Err("geometry needs vertices and whole triangles".into());
        }
        if self.normals.len() != n || (!self.uvs.is_empty() && self.uvs.len() != n) || (!self.colors.is_empty() && self.colors.len() != n) {
            return Err("normals, uvs and colours must match the positions".into());
        }
        if self.indices.iter().any(|&i| i as usize >= n) {
            return Err("an index is past the vertex count".into());
        }
        if self.positions.iter().flatten().chain(self.normals.iter().flatten()).any(|v| !v.is_finite()) {
            return Err("positions and normals must be finite".into());
        }
        Ok(())
    }

    /// The model lanes' packed vertices (`geom.GameMeshVertexAo`) and bounds.
    pub fn pack(&self) -> (Vec<f32>, Vec3f, Vec3f) {
        let mut vertices = Vec::with_capacity(self.positions.len() * crate::model::MODEL_VERTEX_FLOATS);
        let (mut min, mut max) = (vec3f(f32::MAX, f32::MAX, f32::MAX), vec3f(f32::MIN, f32::MIN, f32::MIN));
        for (i, p) in self.positions.iter().enumerate() {
            let n = self.normals[i];
            let (ox, oy) = crate::skin::oct_encode(vec3f(n[0], n[1], n[2]));
            let uv = self.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            let c = self.colors.get(i).copied().unwrap_or([1.0; 4]);
            // The lanes read vertex colour as sRGB bytes (to_lin in the
            // shader): encode the linear colour.
            let s = |v: f32| if v <= 0.0031308 { v.max(0.0) * 12.92 } else { 1.055 * v.min(1.0).powf(1.0 / 2.4) - 0.055 };
            vertices.extend_from_slice(&[
                p[0], p[1], p[2],
                makepad_draw::pack_pair_f16(ox, oy),
                makepad_draw::pack_pair_f16(uv[0], uv[1]),
                makepad_draw::pack_unorm8x4(s(c[0]), s(c[1]), s(c[2]), c[3].clamp(0.0, 1.0)),
                crate::model::pack_ao_uv(0.0, 0.0),
            ]);
            min = vec3f(min.x.min(p[0]), min.y.min(p[1]), min.z.min(p[2]));
            max = vec3f(max.x.max(p[0]), max.y.max(p[1]), max.z.max(p[2]));
        }
        (vertices, min, max)
    }
}

/// The built-in Splash materials items draw through, by what they need.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Builtin {
    unlit: bool,
    blend: u8,
    ibl: bool,
}

#[derive(Default)]
pub(super) struct ItemState {
    geometries: HashMap<GeometryId, Arc<GeometryData>>,
    /// (geometry, material key) -> the uploaded model's id.
    models: HashMap<(GeometryId, u64), String>,
    /// Built-in custom materials that failed to build (not retried).
    failed: Vec<Builtin>,
    /// How many item instances this frame appended to `placed_models`.
    pub(super) appended: usize,
    /// The custom materials this frame's items draw through.
    used_custom: Vec<String>,
    /// Items this frame the model lanes do not draw (lines, points, cards,
    /// models, unresolved buffers), for a host's diagnostics.
    pub(super) skipped: usize,
    /// Images materials name (RGBA8, sRGB for colour), by reference.
    images: HashMap<TextureRef, (usize, usize, Arc<Vec<u8>>)>,
    /// Their mipped forms, by use.
    prepared: HashMap<(TextureRef, u8), Arc<PreparedTexture>>,
}

/// What of a material the uploaded model carries (the base colour is the
/// instance tint).
fn material_key(m: &MaterialFrame) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let f = |h: &mut std::collections::hash_map::DefaultHasher, v: f32| v.to_bits().hash(h);
    match &m.kind {
        MaterialKind::Pbr(p) => {
            0u8.hash(&mut h);
            for v in [p.metallic, p.roughness, p.emissive.x, p.emissive.y, p.emissive.z, p.clearcoat, p.flake, p.rim, p.normal_scale, p.occlusion_strength] {
                f(&mut h, v);
            }
            for map in [p.base_map, p.normal_map, p.metal_rough_map, p.emissive_map, p.occlusion_map] {
                map.hash(&mut h);
            }
        }
        MaterialKind::Unlit(u) => {
            1u8.hash(&mut h);
            u.map.hash(&mut h);
        }
        MaterialKind::Splash(s) => {
            2u8.hash(&mut h);
            s.program.0.hash(&mut h);
        }
    }
    (m.side == Side::Double).hash(&mut h);
    match m.blend {
        Blend::Mask { cutoff } => f(&mut h, cutoff),
        // A blended material's alpha is the layer's (the lanes blend by it).
        Blend::Over | Blend::Add | Blend::Multiply | Blend::Screen => f(&mut h, base_color(m).w),
        Blend::Opaque => {}
    }
    h.finish()
}

fn base_color(m: &MaterialFrame) -> Vec4f {
    match &m.kind {
        MaterialKind::Pbr(p) => p.base_color,
        MaterialKind::Unlit(u) => u.color,
        MaterialKind::Splash(_) => vec4(1.0, 1.0, 1.0, 1.0),
    }
}

impl Renderer {
    /// Make a geometry resident under `id` (replacing an earlier one: its
    /// models are rebuilt on next use).
    pub fn register_geometry(&mut self, id: GeometryId, data: GeometryData) -> Result<(), String> {
        data.validate()?;
        self.items.geometries.insert(id, Arc::new(data));
        let stale: Vec<String> = self.items.models.iter().filter(|((g, _), _)| *g == id).map(|(_, m)| m.clone()).collect();
        self.items.models.retain(|(g, _), _| *g != id);
        for model in stale {
            self.unload_model(&model);
        }
        Ok(())
    }

    pub fn unregister_geometry(&mut self, id: GeometryId) {
        self.items.geometries.remove(&id);
        let stale: Vec<String> = self.items.models.iter().filter(|((g, _), _)| *g == id).map(|(_, m)| m.clone()).collect();
        self.items.models.retain(|(g, _), _| *g != id);
        for model in stale {
            self.unload_model(&model);
        }
    }

    pub fn has_geometry(&self, id: GeometryId) -> bool {
        self.items.geometries.contains_key(&id)
    }

    /// Make an image resident under `r` for the materials that name it
    /// (`base_map`, the glTF maps, an unlit `map`): `rgba` is width x
    /// height RGBA8, sRGB-encoded for colour maps, linear for data. A
    /// reference names one image: register a changed image under a new one.
    pub fn register_image(&mut self, r: TextureRef, width: usize, height: usize, rgba: Arc<Vec<u8>>) -> Result<(), String> {
        if width == 0 || height == 0 || width > 8192 || height > 8192 || rgba.len() != width * height * 4 {
            return Err(format!("an image needs width x height x 4 bytes (1..8192 a side), got {width} x {height} and {} bytes", rgba.len()));
        }
        self.items.images.entry(r).or_insert((width, height, rgba));
        Ok(())
    }

    pub fn unregister_image(&mut self, r: TextureRef) {
        self.items.images.remove(&r);
        self.items.prepared.retain(|(k, _), _| *k != r);
    }

    pub fn has_image(&self, r: TextureRef) -> bool {
        self.items.images.contains_key(&r)
    }

    /// A registered image, mipped for `semantic` (once per use).
    fn item_image(&mut self, r: TextureRef, semantic: PixelSemantic) -> Option<Arc<PreparedTexture>> {
        let key = (r, semantic as u8);
        if let Some(t) = self.items.prepared.get(&key) {
            return Some(t.clone());
        }
        let (width, height, rgba) = self.items.images.get(&r)?;
        let (width, height) = (*width, *height);
        let mut image = ImageBuffer::default();
        image.width = width;
        image.height = height;
        // 0xAARRGGBB words (the lanes sample them as BGRA), bottom row
        // first: the lanes' v runs up the image, an item's uv (glTF) down.
        image.data = rgba.chunks_exact(width * 4).rev().flat_map(|row| row.chunks_exact(4)).map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect();
        let t = Arc::new(PreparedTexture::prepare(image, semantic));
        self.items.prepared.insert(key, t.clone());
        Some(t)
    }

    /// Whether every custom material the last frame's items used, and the
    /// PBR lane, has its pipeline: until then those items draw through the
    /// stock (matte) lane, so a locked-time host waits for this before it
    /// takes the frame.
    pub fn items_ready(&self, cx: &Cx) -> bool {
        // A material that is not installed (it did not build) draws through
        // the stock lane for good: nothing to wait for.
        let custom = self.items.used_custom.iter().all(|name| self.custom_material_shader(name).is_none_or(|id| cx.draw_shader_ready(id, self.hdr_output)));
        // Shiny models draw matte (no emission, no maps) until the PBR
        // pipeline exists; it is made once a frame has any.
        let pbr = self.pbr_draw.as_ref().is_none_or(|d| d.skinned.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready(id, self.hdr_output)));
        custom && pbr
    }

    /// Items the model lanes skipped last frame (lines, points, cards,
    /// models, unresolved kernel buffers).
    pub fn skipped_items(&self) -> usize {
        self.items.skipped
    }

    /// A kernel-written buffer's current records, when its producer
    /// published the generation the item names. makepad-render-kernels
    /// fills this in (its OutputRing, fenced on frame completion); until
    /// then kernel-written items are skipped.
    pub fn resolve_buffer(&mut self, _cx: &mut Cx, _buffer: &BufferRef) -> Option<(Vec<f32>, Vec<u32>)> {
        None
    }

    /// The uploaded model for a geometry under a material, built on first use.
    fn item_model(&mut self, cx: &mut Cx, geometry: GeometryId, material: &MaterialFrame, cast: bool) -> Option<String> {
        let key = (geometry, material_key(material) ^ cast as u64);
        if let Some(model) = self.items.models.get(&key) {
            return Some(model.clone());
        }
        let data = self.items.geometries.get(&geometry)?.clone();
        let (vertices, min, max) = data.pack();
        let mut pbr = crate::model::PbrMaterial { metallic: 0.0, roughness: 1.0, ..Default::default() };
        let mut surface = MaterialSurface { double_sided: material.side == Side::Double, ..Default::default() };
        if let MaterialKind::Pbr(p) = &material.kind {
            pbr.metallic = p.metallic;
            pbr.roughness = p.roughness;
            surface.emissive = [p.emissive.x, p.emissive.y, p.emissive.z];
            surface.clearcoat = p.clearcoat;
            surface.flake = p.flake;
            surface.rim = p.rim;
            surface.normal_scale = if p.normal_map.is_some() { p.normal_scale } else { 0.0 };
            surface.occlusion_strength = if p.occlusion_map.is_some() { p.occlusion_strength } else { 0.0 };
        }
        match material.blend {
            Blend::Mask { cutoff } => {
                surface.alpha_mode = 1;
                surface.alpha_cutoff = cutoff;
            }
            // The lanes take a layer's opacity from its material, not the
            // instance tint.
            Blend::Over | Blend::Add | Blend::Multiply | Blend::Screen => {
                surface.alpha_mode = 2;
                surface.base_alpha = base_color(material).w.clamp(0.0, 1.0);
            }
            Blend::Opaque => {}
        }
        pbr.surface = Some(Arc::new(surface));
        let model = crate::model::StaticModel {
            vertices,
            indices: data.indices.clone(),
            texture_uri: None,
            texture_png: None,
            min,
            max,
            parts: vec![(min, max)],
            ground_ao: None,
            draw_layers: Vec::new(),
            detail_png: None,
            detail_scale: [0.0, 0.0],
            prelit: false,
            anim_parts: Vec::new(),
            driven_parts: Vec::new(),
            sky: None,
            pbr,
        };
        let id = format!("item/{}/{:016x}", geometry.0, key.1);
        // The prepared path keeps the layer's surface (opacity, emission,
        // clear coat, sides), which the parsed-model path drops.
        let mut prepared = match PreparedStaticPreview::prepare(model) {
            Ok(p) => p,
            Err(e) => {
                log!("render: item geometry {} did not prepare: {e}", geometry.0);
                return None;
            }
        };
        // The material's maps, from the registered images (a map not
        // registered draws as its neutral image).
        let (base, normal, orm, emissive, occlusion) = match &material.kind {
            MaterialKind::Pbr(p) => (p.base_map, p.normal_map, p.metal_rough_map, p.emissive_map, p.occlusion_map),
            MaterialKind::Unlit(u) => (u.map, None, None, None, None),
            MaterialKind::Splash(_) => (None, None, None, None, None),
        };
        let masked = matches!(material.blend, Blend::Mask { .. });
        if let Some(t) = base.and_then(|r| self.item_image(r, if masked { PixelSemantic::MaskedColor } else { PixelSemantic::Color })) {
            prepared.main.cutout = t.cutout();
            prepared.main.texture = t;
        }
        if let Some(t) = orm.and_then(|r| self.item_image(r, PixelSemantic::Data)) {
            prepared.main.orm = t;
            prepared.main.orm_on = true;
        }
        let normal = normal.and_then(|r| self.item_image(r, PixelSemantic::Normal));
        let emissive = emissive.and_then(|r| self.item_image(r, PixelSemantic::Color));
        let occlusion = occlusion.and_then(|r| self.item_image(r, PixelSemantic::Data));
        if let Some(s) = prepared.main.surface.as_mut() {
            if let Some(t) = normal {
                s.normal = (*t).clone();
            }
            if let Some(t) = emissive {
                s.emissive = (*t).clone();
            }
            if let Some(t) = occlusion {
                s.occlusion = (*t).clone();
            }
        }
        self.install_static_preview(cx, &id, prepared, false)?;
        self.set_model_casts_shadow(&id, cast);
        self.items.models.insert(key, id.clone());
        Some(id)
    }

    /// The custom material an item draws through, installed on first use.
    fn item_custom(&mut self, cx: &mut Cx, material: &MaterialFrame, ibl: bool) -> Option<String> {
        if let MaterialKind::Splash(s) = &material.kind {
            return Some(splash_material_name(s.program));
        }
        let unlit = matches!(material.kind, MaterialKind::Unlit(_)) || material.lighting == LightingModel::Unlit;
        let blend = match material.blend {
            Blend::Over | Blend::Add | Blend::Multiply | Blend::Screen => 2,
            _ => 0,
        };
        let b = Builtin { unlit, blend, ibl: ibl && !unlit };
        if !b.unlit && b.blend == 0 && !b.ibl {
            return None;
        }
        let name = format!("__item{}{}{}", if b.unlit { "_unlit" } else { "" }, if b.blend == 2 { "_over" } else { "" }, if b.ibl { "_ibl" } else { "" });
        if self.custom_material_shader(&name).is_some() {
            return Some(name);
        }
        if self.items.failed.contains(&b) {
            return None;
        }
        let desc = MaterialDesc {
            kind: if b.unlit { BaseKind::Unlit } else { BaseKind::Pbr },
            blend: if b.blend == 2 { Blend::Over } else { Blend::Opaque },
            lighting: if b.unlit { LightingModel::Unlit } else { LightingModel::Lit },
            ibl: b.ibl,
        };
        let built = cx.try_with_vm(|vm| DrawSceneCustom::build(vm, &desc, &HookSet::new(), HookMask::ALL, Vec4f::default()));
        match built {
            Some(Ok(m)) => {
                self.install_custom_material(name.clone(), m);
                Some(name)
            }
            Some(Err(e)) => {
                log!("render: built-in item material {name} did not build: {e}");
                self.items.failed.push(b);
                None
            }
            None => None,
        }
    }

    /// Append this frame's item instances to the placed models (removed
    /// again by [`Self::pop_item_instances`]).
    pub(super) fn push_item_instances(&mut self, cx: &mut Cx, world: &World) {
        self.items.appended = 0;
        self.items.skipped = 0;
        self.items.used_custom.clear();
        if world.items.is_empty() {
            return;
        }
        let ibl = self.ibl_texture().is_some();
        let mut out = Vec::new();
        for item in &world.items {
            if item.validate().is_err() {
                self.items.skipped += 1;
                continue;
            }
            let (geometry, material, instances): (GeometryRef, _, Vec<(Mat4f, Vec4f)>) = match &item.kind {
                ItemKind::Mesh { geometry, material, transform } => (*geometry, *material, vec![(*transform, vec4(1.0, 1.0, 1.0, 1.0))]),
                ItemKind::Instances { geometry, material, source: InstanceSource::Packed { data, layout }, count } if *layout == LAYOUT_TRANSFORM_TINT => {
                    let list = data.chunks_exact(TRANSFORM_TINT_FLOATS).take(*count as usize).map(|r| {
                        let mut m = Mat4f::identity();
                        m.v.copy_from_slice(&r[..16]);
                        (m, vec4(r[16], r[17], r[18], r[19]))
                    }).collect();
                    (*geometry, *material, list)
                }
                _ => {
                    self.items.skipped += 1;
                    continue;
                }
            };
            let GeometryRef::Resident(geometry) = geometry else {
                self.items.skipped += 1;
                continue;
            };
            let Some(material) = world.material(material) else {
                self.items.skipped += 1;
                continue;
            };
            let cast = item.flags.contains(ItemFlags::CAST_SHADOW) && material.blend.is_opaque();
            let Some(model) = self.item_model(cx, geometry, material, cast) else {
                self.items.skipped += 1;
                continue;
            };
            let custom = self.item_custom(cx, material, ibl);
            if let Some(name) = &custom {
                if !self.items.used_custom.contains(name) {
                    self.items.used_custom.push(name.clone());
                }
            }
            let params = match &material.kind {
                MaterialKind::Unlit(u) => vec4(0.0, 0.0, 0.0, u.intensity.max(1.0e-6).log2()),
                MaterialKind::Splash(s) => s.uniforms[0],
                _ => Vec4f::default(),
            };
            let color = base_color(material);
            for (transform, tint) in instances {
                out.push(ModelInstance {
                    model: model.clone(),
                    custom_material: custom.clone().map(|name| CustomMaterialInstance { name, params }),
                    transform,
                    tint: vec4(color.x * tint.x, color.y * tint.y, color.z * tint.z, color.w * tint.w),
                    color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
                    dynamic: true,
                    depth_order: 0.0,
                    part_poses: Vec::new(),
                });
            }
        }
        self.items.appended = out.len();
        self.placed_models.extend(out);
    }

    pub(super) fn pop_item_instances(&mut self) {
        let keep = self.placed_models.len().saturating_sub(self.items.appended);
        self.placed_models.truncate(keep);
        self.items.appended = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri() -> GeometryData {
        GeometryData {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![],
            colors: vec![],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn geometry_packs_to_the_model_layout_and_is_validated() {
        let (v, min, max) = tri().pack();
        assert_eq!(v.len(), 3 * crate::model::MODEL_VERTEX_FLOATS);
        assert_eq!((min, max), (vec3f(0.0, 0.0, 0.0), vec3f(1.0, 2.0, 0.0)));
        let mut bad = tri();
        bad.indices = vec![0, 1, 3];
        assert!(bad.validate().is_err());
        bad = tri();
        bad.normals.pop();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn the_base_colour_does_not_key_the_model_but_the_rest_does() {
        let pbr = |c: f32, r: f32| MaterialFrame { kind: MaterialKind::Pbr(makepad_scene::PbrParams { base_color: vec4(c, c, c, 1.0), roughness: r, ..Default::default() }), ..Default::default() };
        assert_eq!(material_key(&pbr(0.2, 0.5)), material_key(&pbr(0.9, 0.5)));
        assert_ne!(material_key(&pbr(0.2, 0.5)), material_key(&pbr(0.2, 0.6)));
        let mut double = pbr(0.2, 0.5);
        double.side = Side::Double;
        assert_ne!(material_key(&pbr(0.2, 0.5)), material_key(&double));
    }
}
