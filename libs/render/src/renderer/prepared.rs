//! Off-thread prepared model payloads and their uploaded/loaded GPU forms.

use super::*;
use crate::material_surface::{ImageBudget, ImageSize, MapPlan, SurfacePlan};
pub use crate::material_surface::{TextureCharge, TextureSlot};

/// Decode generated PNG pixels with limits enforced by the decoder before
/// allocation. Animated PNGs are decoded as a single frame in this lane.
pub fn decode_generated_png(bytes: &[u8], dimension: usize, byte_limit: usize) -> Result<ImageBuffer, String> {
    // The load-ready form (KTX2/UASTC) stands in for the PNG.
    if let Some(image) = crate::texture_pack::image_from_ktx2(bytes) {
        let image = image?;
        if image.width > dimension || image.height > dimension { return Err("generated PNG exceeds dimension limit".into()); }
        if image.data.len() * 4 > byte_limit { return Err("generated PNG exceeds decoded pixel budget".into()); }
        return Ok(image);
    }
    use makepad_draw::makepad_zune_png::{PngDecoder, makepad_zune_core::{bytestream::ZCursor, options::DecoderOptions}};
    let options = DecoderOptions::default().set_max_width(dimension).set_max_height(dimension)
        .set_strict_mode(true).png_set_confirm_crc(true).png_set_strip_to_8bit(true).png_set_decode_animated(false);
    let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(|e| format!("generated PNG header: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or("generated PNG has no dimensions")?;
    let output_bytes = width.checked_mul(height).and_then(|pixels| pixels.checked_mul(4)).ok_or("generated PNG dimensions overflow")?;
    if output_bytes > byte_limit { return Err("generated PNG exceeds decoded pixel budget".into()); }
    let pixels = decoder.decode_raw().map_err(|e| format!("generated PNG pixels: {e:?}"))?;
    ImageBuffer::new(&pixels, width, height).map_err(|e| format!("generated PNG pixels: {e:?}"))
}

/// [`decode_generated_png`] that DEGRADES past the pixel budget: an image
/// too large for what is left is decoded at its own size and box-halved
/// until it fits (logged), instead of failing the whole model.
pub fn decode_generated_png_within(bytes: &[u8], dimension: usize, byte_limit: usize, what: &str) -> Result<ImageBuffer, String> {
    let mut image = decode_generated_png(bytes, dimension, usize::MAX)?;
    let (w0, h0) = (image.width, image.height);
    let level = ImageSize(w0, h0).fit(0, byte_limit, false);
    halve_image(&mut image, level);
    if level > 0 {
        let (w, h) = (image.width, image.height);
        let mut key = live_id!(generated_texture_reduced).str_append(what);
        for size in [w0, h0, w, h] { key = key.bytes_append(&size.to_le_bytes()); }
        if first_report(key.0) {
            let factor = 1usize << level;
            log!("generated {what} {w0}x{h0} over the prepare budget: reduced to {w}x{h} (1/{factor} per side)");
        }
    }
    Ok(image)
}

fn halve_image(image: &mut ImageBuffer, levels: usize) {
    for _ in 0..levels {
        let (w, h) = (image.width, image.height);
        if (w, h) == (1, 1) { break; }
        let ImageSize(nw, nh) = ImageSize(w, h).at(1);
        let mut data = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0u32; 4];
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let p = image.data[(y * 2 + dy).min(h - 1) * w + (x * 2 + dx).min(w - 1)];
                    for (i, s) in sum.iter_mut().enumerate() { *s += (p >> (i * 8)) & 255; }
                }
                data.push(sum.iter().enumerate().fold(0u32, |acc, (i, s)| acc | (((s + 2) / 4) << (i * 8))));
            }
        }
        image.data = data;
        image.width = nw;
        image.height = nh;
    }
}

pub(crate) fn decode_model_image(bytes: &[u8], dimension: usize, level: usize) -> Result<ImageBuffer, String> {
    let mut image = decode_generated_png(bytes, dimension, usize::MAX)?;
    halve_image(&mut image, level);
    Ok(image)
}

pub(crate) fn decode_pixels_within(bytes: &[u8], dimension: usize, level: usize, remaining: usize) -> Result<ImageBuffer, String> {
    let mut image = decode_generated_png(bytes, dimension, usize::MAX)?;
    if !crate::texture_pack::is_ktx2(bytes) {
        let level = ImageSize(image.width, image.height).fit(level, remaining, false);
        halve_image(&mut image, level);
    }
    Ok(image)
}

/// Upload a prepared generated atlas with repeat addressing and one level.
/// This never builds CPU mips, including when image-cache mipmaps are enabled.
pub fn upload_generated_texture(cx: &mut Cx, image: ImageBuffer) -> Texture {
    Texture::new_with_format(cx, TextureFormat::VecMipBGRAu8_32 {
        width: image.width, height: image.height, data: Some(image.data.into()),
        max_level: Some(0), wrap: TextureWrap::Repeat, updated: TextureUpdated::Full,
    })
}

/// A model's physics collider boxes (model space): the triangle-derived
/// voxel boxes ([`crate::model::voxel_boxes`]), or its curated parts for a
/// degenerate mesh. Worked out on the first ask, from the mesh the model
/// keeps anyway: only a host with physics asks (a game's props), and a
/// scene's items (a film's meshes) never do; voxelizing each item at load
/// was most of a large scene's first draw on a browser's UI thread.
#[derive(Clone, Default)]
pub(super) struct Colliders {
    boxes: std::sync::Arc<std::sync::OnceLock<Vec<(Vec3f, Vec3f)>>>,
    mesh: Option<std::sync::Arc<ColliderMesh>>,
}

struct ColliderMesh {
    positions: std::sync::Arc<Vec<Vec3f>>,
    indices: std::sync::Arc<Vec<u32>>,
    min: Vec3f,
    max: Vec3f,
    fallback: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
}

impl Colliders {
    pub(super) fn from_mesh(positions: std::sync::Arc<Vec<Vec3f>>, indices: std::sync::Arc<Vec<u32>>, min: Vec3f, max: Vec3f, fallback: std::sync::Arc<Vec<(Vec3f, Vec3f)>>) -> Self {
        Self { boxes: Default::default(), mesh: Some(std::sync::Arc::new(ColliderMesh { positions, indices, min, max, fallback })) }
    }

    pub(super) fn boxes(&self) -> &[(Vec3f, Vec3f)] {
        self.boxes.get_or_init(|| {
            let Some(m) = &self.mesh else { return Vec::new() };
            let boxes = crate::model::voxel_boxes_of(|i| m.positions[i as usize], &m.indices, m.min, m.max);
            if boxes.is_empty() { m.fallback.to_vec() } else { boxes }
        })
    }
}

/// Worker-prepared generated mesh, decoded material layers and rigid animated
/// parts. All fields are private so upload receives only validated products.
/// Clone on a worker when separate transient views need independent buffers.
#[derive(Clone)]
pub struct PreparedStaticPreview {
    pub(super) lods:Vec<(f32,PreparedStaticPreview)>,
    pub(super) morph:Option<crate::asset_morph::AssetMorph>,
    pub(super) ao: Option<ImageBuffer>,
    pub(super) lm_source: Option<std::sync::Arc<crate::lightmap::LmMeshSource>>,
    pub(super) bake_stream: Option<(Vec<u32>,Vec<f32>)>,
    pub(super) sdf: Option<crate::shadow_sdf::ShadowSdfAtlas>,
    pub(super) emitters: std::sync::Arc<Vec<crate::asset_lights::AssetLightEmitter>>,
    pub(super) main: PreparedStaticLayer,
    pub(super) extra: Vec<PreparedStaticLayer>,
    pub(super) positions: std::sync::Arc<Vec<Vec3f>>,
    pub(super) mesh_indices: std::sync::Arc<Vec<u32>>,
    pub(super) authored_collisions: std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>,
    pub(super) collider_parts: Colliders,
    pub(super) occluder_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    pub(super) anim_parts: Vec<PreparedAnimPreview>,
    pub(super) driven_parts: Vec<PreparedDrivenPreview>,
    pub(super) sky: Option<PreparedSky>,
    pub(super) liquids: std::sync::Arc<Vec<crate::model::LiquidSurface>>,
    pub(super) min: Vec3f, pub(super) max: Vec3f, pub(super) prelit: bool,
}

/// A model's draw layers can bind the same atlas many times. Keep its plan, charge and
/// prepared pixels together so repeated bindings spend the model's budget just once.
struct LayerMap {
    plan: MapPlan,
    texture: std::cell::OnceCell<std::sync::Arc<crate::material_surface::PreparedTexture>>,
}

impl LayerMap {
    fn prepare(&self, bytes: Option<&[u8]>, level: usize, remaining: &mut usize) -> Result<std::sync::Arc<crate::material_surface::PreparedTexture>, String> {
        if let Some(texture) = self.texture.get() { return Ok(texture.clone()); }
        let texture = self.plan.prepare(bytes, level, remaining)?;
        let _ = self.texture.set(texture.clone());
        Ok(texture)
    }
}

struct LayerImages {
    base: std::rc::Rc<LayerMap>,
    detail: std::rc::Rc<LayerMap>,
    orm: std::rc::Rc<LayerMap>,
    surface: Option<std::rc::Rc<SurfacePlan>>,
}

impl PreparedStaticPreview {
    /// Bytes this asset keeps on the CPU once uploaded: the picking and shadow-ray geometry
    /// (positions, indices), the lightmap source and the authored collision meshes, over every
    /// LOD level. The upload copies are dropped at upload and are not counted.
    pub fn retained_cpu_bytes(&self) -> usize {
        // Cloned level products may share buffers, so each retained Arc is charged once.
        let mut positions = std::collections::HashSet::new();
        let mut indices = std::collections::HashSet::new();
        let mut lightmaps = std::collections::HashSet::new();
        let mut collisions = std::collections::HashSet::new();
        let mut bytes = 0;
        for model in std::iter::once(self).chain(self.lods.iter().map(|(_, model)| model)) {
            if positions.insert(std::sync::Arc::as_ptr(&model.positions)) {
                bytes += model.positions.len() * std::mem::size_of::<Vec3f>();
            }
            if indices.insert(std::sync::Arc::as_ptr(&model.mesh_indices)) {
                bytes += model.mesh_indices.len() * std::mem::size_of::<u32>();
            }
            if let Some(source) = &model.lm_source {
                if lightmaps.insert(std::sync::Arc::as_ptr(source)) {
                    bytes += source.ao_uv.len() * 8 + source.albedo.len() * 12 + source.caster.bytes();
                }
            }
            for collision in model.authored_collisions.iter() {
                if collisions.insert(std::sync::Arc::as_ptr(&collision.mesh)) {
                    bytes += collision.mesh.positions.len() * 12 + collision.mesh.indices.len() * 4;
                }
            }
        }
        bytes
    }

    pub fn lod_distances(&self)->impl Iterator<Item=f32>+'_ {self.lods.iter().map(|(distance,_)|*distance)}
    /// Geometry per distance level, finest first. Model-owned sidecars stay on
    /// `[0]`: morph, AO, shadow SDF, bake streams and sky (including its two images)
    /// are not shared through the content cache or replaced by distance layers.
    pub fn level_geometry_bytes(&self) -> Vec<usize> {
        let mut levels: Vec<usize> = std::iter::once(self).chain(self.lods.iter().map(|(_, m)| m))
            .map(|m| m.layers().map(|l| (l.vertices.len() + l.indices.len()) * 4).sum())
            .collect();
        levels[0] += std::iter::once(self).chain(self.lods.iter().map(|(_, m)| m))
            .map(|m| {
                m.morph.as_ref().map_or(0, |v| v.pixels.len() * 4)
                    + m.ao.as_ref().map_or(0, |v| v.data.len() * 4)
                    + m.sdf.as_ref().map_or(0, |v| v.pixels.len())
                    + m.bake_stream.as_ref().map_or(0, |(i, v)| (i.len() + v.len()) * 4)
                    + m.sky.as_ref().map_or(0, |s| (s.vertices.len() + s.indices.len() + s.tex0.data.len() + s.tex1.data.len()) * 4)
            }).sum::<usize>();
        levels
    }

    /// Content-cache keys bound by each distance level, once per key per level.
    pub fn level_texture_keys(&self) -> Vec<Vec<u64>> {
        std::iter::once(self).chain(self.lods.iter().map(|(_, m)| m)).map(|m| {
            let mut seen = std::collections::HashSet::new();
            let mut keys = Vec::new();
            m.visit_texture_charges(&mut |charge| {
                if seen.insert(charge.key) { keys.push(charge.key); }
            });
            keys
        }).collect()
    }

    pub fn triangles_at_distance(&self,distance:f32)->usize{
        let n=if distance.is_finite()&&distance>=0.{self.lods.partition_point(|(d,_)|*d<=distance)}else{0};
        let m=if n==0{self}else{&self.lods[n-1].1};
        (m.main.indices.len()+m.extra.iter().map(|p|p.indices.len()).sum::<usize>()+m.anim_parts.iter().flat_map(|p|&p.draws).chain(m.driven_parts.iter().flat_map(|p|&p.draws)).map(|p|p.indices.len()).sum::<usize>())/3
    }
    pub fn with_lods(mut self,lods:Vec<(f32,Self)>)->Result<Self,String>{
        crate::asset_lod::validate_distances(lods.iter().map(|(d,_)|*d))?;
        if lods.iter().any(|(_,m)|!m.lods.is_empty()){return Err("nested prepared LODs are unsupported".into());}
        for(_,m)in &lods{self.min=vec3f(self.min.x.min(m.min.x),self.min.y.min(m.min.y),self.min.z.min(m.min.z));self.max=vec3f(self.max.x.max(m.max.x),self.max.y.max(m.max.y),self.max.z.max(m.max.z));}
        self.lods=lods;Ok(self)
    }
    pub fn with_morph(mut self,morph:Option<crate::asset_morph::AssetMorph>)->Self{if let Some(m)=&morph{let extent=m.extent*8.0*m.targets as f32;self.min=self.min-vec3f(extent,extent,extent);self.max=self.max+vec3f(extent,extent,extent);}self.morph=morph;self}
    pub fn with_collision_metadata(mut self, collisions: std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>) -> Self { self.authored_collisions = collisions; self }
    pub fn with_emitters(mut self, emitters: std::sync::Arc<Vec<crate::asset_lights::AssetLightEmitter>>) -> Self { self.emitters = emitters; self }
    /// All parsing, bounded image decoding and collision extraction is worker
    /// work. The corresponding upload only consumes validated vectors.
    pub fn prepare(model: StaticModel) -> Result<Self, String> { Self::prepare_with_sidecars(model,None,None) }
    pub fn prepare_with_sidecars(model: StaticModel, ao_png: Option<&[u8]>, sdf_bytes: Option<&[u8]>) -> Result<Self, String> {
        Self::prepare_with_budget(model, ao_png, sdf_bytes, 128 * 1024 * 1024)
    }

    pub(super) fn prepare_with_budget(mut model: StaticModel, ao_png: Option<&[u8]>, sdf_bytes: Option<&[u8]>, budget: usize) -> Result<Self, String> {
        use crate::material_surface::{PixelSemantic, TextureWork, fetch_ahead, image_content_hash, texture_work};
        use std::rc::Rc;
        enum Charge { Map(Rc<LayerMap>), Surface(Rc<SurfacePlan>) }
        let _preparing = crate::material_surface::Preparing::begin();
        let sdf = sdf_bytes.and_then(|bytes| crate::shadow_sdf::ShadowSdfAtlas::from_shadowsdf(bytes).map(|v| v.0));
        if model.texture_uri.is_some() && model.texture_png.is_none() { return Err("model references an external texture without supplied pixels".into()); }
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        let check = |vertices: &[f32], indices: &[u32]| -> Result<(),String> {
            if vertices.len()%stride != 0 || indices.len()%3 != 0 || indices.iter().any(|&i|i as usize >= vertices.len()/stride)
                || vertices.chunks_exact(stride).any(|v|v[..3].iter().any(|x|!x.is_finite())) { return Err("prepared model has invalid geometry".into()); }
            Ok(())
        };
        check(&model.vertices,&model.indices)?;
        let mut charges = Vec::new();
        let mut shared = std::collections::HashMap::new();
        let mut geometry = sdf.as_ref().map_or(0, |s| s.pixels.len());
        let mut plan_layer = |vertices: &[f32], indices: &[u32], base: Option<&[u8]>, detail: Option<&[u8]>, pbr: &crate::model::PbrMaterial| -> Result<LayerImages, String> {
            geometry += (vertices.len() + indices.len()) * 4;
            let mut map = |bytes: Option<&[u8]>, fallback, semantic: PixelSemantic, store| -> Result<Rc<LayerMap>, String> {
                let ktx = bytes.is_some_and(crate::texture_pack::is_ktx2);
                let hash = bytes.map_or(0, |b| crate::texture_pack::content_id(b)
                    .unwrap_or_else(|| texture_work(TextureWork::Hash, || image_content_hash(b))));
                let key = (hash, bytes.map_or(0, <[u8]>::len), ktx, bytes.is_none().then_some(fallback), semantic as u8, store);
                let entry = match shared.entry(key) {
                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let map = Rc::new(LayerMap {
                            plan: MapPlan::planned(bytes, hash, fallback, semantic, store)?,
                            texture: Default::default(),
                        });
                        charges.push(Charge::Map(map.clone()));
                        e.insert(map)
                    }
                };
                Ok(entry.clone())
            };
            let surface = pbr.surface.as_deref();
            let base = map(base, 0xffff_ffff, surface.map_or(PixelSemantic::Color, |s| s.base_semantic()), !surface.is_some_and(|s| s.msdf.is_some()))?;
            let detail = map(detail, 0xff80_8080, PixelSemantic::Color, true)?;
            let orm = map(pbr.orm_png.as_deref(), 0xffff_ffff, PixelSemantic::Data, true)?;
            let surface = surface.map(SurfacePlan::planned).transpose()?.map(|plan| {
                let plan = Rc::new(plan);
                charges.push(Charge::Surface(plan.clone()));
                plan
            });
            Ok(LayerImages { base, detail, orm, surface })
        };
        let merged = if model.draw_layers.is_empty() {
            Some(plan_layer(&model.vertices, &model.indices, model.texture_png.as_deref(), model.detail_png.as_deref(), &model.pbr)?)
        } else { None };
        let mut plan_draws = |layers: &[crate::model::StaticDrawLayer]| {
            layers.iter().map(|l| plan_layer(&l.vertices, &l.indices, l.texture_png.as_deref(), l.detail_png.as_deref(), &l.pbr)).collect::<Result<Vec<_>, String>>()
        };
        let draws = plan_draws(&model.draw_layers)?;
        let mut animated = model.anim_parts.iter().map(|p| plan_draws(&p.layers)).collect::<Result<Vec<_>, _>>()?;
        let driven = model.driven_parts.iter().map(|p| plan_draws(&p.layers)).collect::<Result<Vec<_>, _>>()?;
        for (part, plans) in model.anim_parts.iter().zip(&mut animated) {
            if plans.is_empty() { plans.push(plan_layer(&part.vertices, &part.indices, None, None, &Default::default())?); }
        }
        drop(shared); // each binding now holds its plan until its layer is prepared
        let mut maps = Vec::with_capacity(charges.len());
        for charge in &charges {
            match charge {
                Charge::Map(map) => maps.push(&map.plan),
                Charge::Surface(surface) => maps.extend(surface.maps()),
            }
        }
        fetch_ahead(&maps);
        drop(maps);
        let mut inventory = ImageBudget::default();
        for charge in charges {
            match charge {
                Charge::Map(map) => map.plan.charge(&mut inventory),
                Charge::Surface(surface) => surface.charge(&mut inventory),
            }
        }
        if let Some(sky) = &model.sky {
            geometry += (sky.vertices.len() + sky.indices.len()) * 4;
            for bytes in sky.images.iter().take(2) { inventory.pixels(bytes, 4096)?; }
            inventory.fixed += 4 * (2usize.saturating_sub(sky.images.len()));
        }
        if let Some(bytes) = ao_png {
            inventory.pixels(bytes, 4096)?;
            geometry += model.indices.len() * (stride + 1) * 4; // bake stream
        }
        inventory.fixed = inventory.fixed.saturating_add(geometry);
        let level = inventory.level(budget);
        let mut remaining = budget.saturating_sub(geometry);
        let mut pixels = |bytes: &[u8]| -> Result<ImageBuffer, String> {
            let image = decode_pixels_within(bytes, 4096, level, remaining)?;
            remaining = remaining.saturating_sub(image.data.len() * 4);
            Ok(image)
        };
        let ao = ao_png.map(&mut pixels).transpose()?;
        // Empty geometry is a valid distance level; its upload has no draw indices.
        let positions:Vec<Vec3f> = model.vertices.chunks_exact(stride).map(|v|vec3f(v[0],v[1],v[2])).collect();
        let mesh_indices = std::sync::Arc::new(model.indices.clone());
        let lm_source=ao.as_ref().map(|image|std::sync::Arc::new(crate::lightmap::LmMeshSource::new(
            crate::ao::MeshRaycaster::new(positions.clone(),model.indices.clone(),model.min,model.max),
            model.vertices.chunks_exact(stride).map(|v|crate::model::unpack_ao_uv(v[6])).collect(),
            model.vertices.chunks_exact(stride).map(|v|{let b=v[5].to_bits();vec3f((b&255)as f32/255.0,((b>>8)&255)as f32/255.0,((b>>16)&255)as f32/255.0)}).collect(),
            image.width,image.height,
        )));
        let bake_stream=ao.as_ref().map(|_|{
            let mut vertices=Vec::with_capacity(model.indices.len()*stride);let mut indices=Vec::with_capacity(model.indices.len());
            for tri in model.indices.chunks_exact(3){let a=positions[tri[0]as usize];let b=positions[tri[1]as usize];let c=positions[tri[2]as usize];
                let n=Vec3f::cross(b-a,c-a);let length=n.length();let n=if length>1e-12{n*(1.0/length)}else{vec3f(0.0,1.0,0.0)};
                let (x,y)=crate::skin::oct_encode(n);let normal=makepad_draw::pack_pair_f16(x,y);
                for index in tri{let offset=*index as usize*stride;indices.push((vertices.len()/stride)as u32);vertices.extend_from_slice(&model.vertices[offset..offset+stride]);let at=vertices.len()-stride+3;vertices[at]=normal;}
            }(indices,vertices)
        });
        let occluder_parts = model.collider_parts();
        let layer = |name: std::sync::Arc<str>, vertices: Vec<f32>, indices: Vec<u32>, png: Option<Vec<u8>>,
                     detail: Option<Vec<u8>>, detail_scale: [f32; 2], mut pbr: crate::model::PbrMaterial, plan: LayerImages, remaining: &mut usize| -> Result<PreparedStaticLayer, String> {
            check(&vertices,&indices)?;
            let msdf=pbr.surface.as_ref().is_some_and(|s|s.msdf.is_some());
            let texture = plan.base.prepare(png.as_deref(), level, remaining)?;
            let detail_on = detail.is_some();
            let detail = plan.detail.prepare(detail.as_deref(), level, remaining)?;
            let orm_on = pbr.orm_png.is_some();
            let orm = plan.orm.prepare(pbr.orm_png.as_deref(), level, remaining)?;
            let surface = plan.surface.zip(pbr.surface.as_deref()).map(|(plan, surface)| plan.prepare(surface.clone(), remaining, level)).transpose()?;
            pbr.orm_png=None;
            let cutout=msdf || texture.cutout();
            Ok(PreparedStaticLayer { name, vertices,indices,texture,detail,detail_scale:if detail_on{detail_scale}else{[0.0,0.0]},orm,orm_on,surface,pbr,cutout })
        };
        let split_layer = |(value, plan): (crate::model::StaticDrawLayer, LayerImages), remaining: &mut usize| {
            layer(value.name, value.vertices, value.indices, value.texture_png, value.detail_png, value.detail_scale, value.pbr, plan, remaining)
        };
        let mut layers=std::mem::take(&mut model.draw_layers).into_iter().zip(draws);
        let main = if let Some(first) = layers.next() {
            split_layer(first, &mut remaining)?
        } else {
            layer(
                model.material_name.clone(),
                std::mem::take(&mut model.vertices),
                std::mem::take(&mut model.indices),
                model.texture_png.take(),
                model.detail_png.take(),
                model.detail_scale,
                model.pbr.clone(), merged.expect("merged layer plan"), &mut remaining,
            )?
        };
        let mut extra=Vec::new();
        for value in layers {extra.push(split_layer(value, &mut remaining)?);}
        let mut anim_parts=Vec::new();
        for (mut def, plans) in std::mem::take(&mut model.anim_parts).into_iter().zip(animated) {
            let collider=std::sync::Arc::new(def.collider_boxes());
            let mut draws=Vec::new();
            if def.layers.is_empty() {
                draws.push(layer(Default::default(), std::mem::take(&mut def.vertices), std::mem::take(&mut def.indices),
                    None, None, [0.0, 0.0], Default::default(), plans.into_iter().next().expect("animated layer plan"), &mut remaining)?);
            } else {
                for value in std::mem::take(&mut def.layers).into_iter().zip(plans) { draws.push(split_layer(value, &mut remaining)?); }
            }
            def.vertices.clear();
            anim_parts.push(PreparedAnimPreview{def:std::sync::Arc::new(def),draws,collider});
        }
        let mut driven_parts=Vec::new();
        for (mut def, plans) in std::mem::take(&mut model.driven_parts).into_iter().zip(driven) {
            let mut draws=Vec::new();
            for value in std::mem::take(&mut def.layers).into_iter().zip(plans) { draws.push(split_layer(value, &mut remaining)?); }
            driven_parts.push(PreparedDrivenPreview{def:std::sync::Arc::new(def),draws});
        }
        let sky=if let Some(mut part)=model.sky.take(){
            check(&part.vertices,&part.indices)?;
            let positions=std::sync::Arc::new(part.vertices.chunks_exact(stride).map(|v|vec3f(v[0],v[1],v[2])).collect());
            let mesh_indices=std::sync::Arc::new(part.indices.clone());
            // Sky images stay single-level (their wrap seam would pick a
            // tiny mip), decoded directly within the same budget.
            let mut sky_image=|bytes:Option<&[u8]>,fallback:u32|->Result<ImageBuffer,String>{
                if let Some(bytes) = bytes {
                    let image = decode_pixels_within(bytes, 4096, level, remaining)?;
                    remaining = remaining.saturating_sub(image.data.len() * 4);
                    Ok(image)
                } else { remaining = remaining.saturating_sub(4); let mut image=ImageBuffer::default();image.width=1;image.height=1;image.data=vec![fallback];Ok(image) }
            };
            let tex0=sky_image(part.images.first().map(Vec::as_slice),0xffff_ffff)?;
            let tex1=sky_image(part.images.get(1).map(Vec::as_slice),0)?;
            let vertices=std::mem::take(&mut part.vertices);let indices=std::mem::take(&mut part.indices);part.images.clear();
            Some(PreparedSky{part:std::sync::Arc::new(part),vertices,indices,positions,mesh_indices,tex0,tex1})
        }else{None};
        let positions=std::sync::Arc::new(positions);let occluder_parts=std::sync::Arc::new(occluder_parts);
        let collider_parts=Colliders::from_mesh(positions.clone(),mesh_indices.clone(),model.min,model.max,occluder_parts.clone());
        Ok(Self{lods:Vec::new(),morph:None,ao,lm_source,bake_stream,sdf,emitters:Default::default(),main,extra,positions,mesh_indices,
            authored_collisions:Default::default(),collider_parts,occluder_parts,anim_parts,driven_parts,sky,liquids:std::sync::Arc::new(std::mem::take(&mut model.liquids)),
            min:model.min,max:model.max,prelit:model.prelit})
    }
    /// GPU textures shared through the renderer's content cache, once per key
    /// across every distance level. A key used in several slots keeps its first slot.
    pub fn texture_charges(&self) -> Vec<TextureCharge> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for model in std::iter::once(self).chain(self.lods.iter().map(|(_, m)| m)) {
            model.visit_texture_charges(&mut |charge| {
                if seen.insert(charge.key) { out.push(charge); }
            });
        }
        out
    }

    fn layers(&self) -> impl Iterator<Item = &PreparedStaticLayer> {
        std::iter::once(&self.main).chain(&self.extra)
            .chain(self.anim_parts.iter().flat_map(|p| &p.draws))
            .chain(self.driven_parts.iter().flat_map(|p| &p.draws))
    }

    fn visit_texture_charges(&self, visit: &mut impl FnMut(TextureCharge)) {
        use crate::material_surface::splat_array_charge;
        for layer in self.layers() {
            let texel_addressed = layer.pbr.mag_nearest || layer.surface.as_ref()
                .is_some_and(|surface| surface.definition.msdf.is_some() || surface.definition.impostor > 0.0);
            visit(layer.texture.charge(TextureSlot::Base, texel_addressed));
            visit(layer.detail.charge(TextureSlot::Detail, texel_addressed));
            visit(layer.orm.charge(TextureSlot::Orm, texel_addressed));
            if let Some(surface) = &layer.surface {
                visit(surface.normal.charge(TextureSlot::Normal, texel_addressed));
                visit(surface.occlusion.charge(TextureSlot::Occlusion, texel_addressed));
                visit(surface.emissive.charge(TextureSlot::Emissive, texel_addressed));
                if let Some(splat) = &surface.splat {
                    for (layers, slot) in [(&splat.albedo, TextureSlot::SplatAlbedo), (&splat.normal, TextureSlot::SplatNormal)] {
                        let mut charge = splat_array_charge(layers, slot);
                        charge.texel_addressed = texel_addressed;
                        visit(charge);
                    }
                    visit(splat.weights.charge(TextureSlot::SplatWeights, true));
                }
            }
        }
    }

    pub fn upload_bytes(&self) -> usize {
        self.level_geometry_bytes().iter().sum::<usize>()
            + self.texture_charges().iter().map(|t| t.bytes).sum::<usize>()
    }
}
#[derive(Clone)]
pub(super) struct PreparedStaticLayer {
    pub(super) name: std::sync::Arc<str>,
    pub(super) vertices:Vec<f32>,pub(super) indices:Vec<u32>,pub(super) texture:std::sync::Arc<crate::material_surface::PreparedTexture>,pub(super) detail:std::sync::Arc<crate::material_surface::PreparedTexture>,pub(super) detail_scale:[f32;2],
    pub(super) orm:std::sync::Arc<crate::material_surface::PreparedTexture>,pub(super) orm_on:bool,pub(super) surface:Option<crate::material_surface::PreparedSurface>,pub(super) pbr:crate::model::PbrMaterial,
    /// The base texture can fail the alpha test (found on the worker; opaque.rs).
    pub(super) cutout:bool,
}
#[derive(Clone)]
pub(super) struct PreparedAnimPreview {pub(super) def:std::sync::Arc<crate::model::AnimPart>,pub(super) draws:Vec<PreparedStaticLayer>,pub(super) collider:std::sync::Arc<Vec<(Vec3f,Vec3f)>>}
#[derive(Clone)]
pub(super) struct PreparedDrivenPreview {pub(super) def:std::sync::Arc<crate::model::DrivenPart>,pub(super) draws:Vec<PreparedStaticLayer>}
#[derive(Clone)]
pub(super) struct PreparedSky {pub(super) part:std::sync::Arc<crate::model::SkyPart>,pub(super) vertices:Vec<f32>,pub(super) indices:Vec<u32>,pub(super) positions:std::sync::Arc<Vec<Vec3f>>,pub(super) mesh_indices:std::sync::Arc<Vec<u32>>,pub(super) tex0:ImageBuffer,pub(super) tex1:ImageBuffer}

/// UI-owned uploaded handles and immutable CPU metadata, reusable in any
/// renderer without re-parsing or copying the mesh on the UI thread.
#[derive(Clone)]
pub struct UploadedStaticPreview {
    pub(super) lods:Vec<(f32,UploadedStaticPreview)>,
    pub(super) ordinary_shadow_bounds: Option<std::rc::Rc<super::lights::OrdinaryShadowBounds>>,
    pub(super) morph:Option<crate::asset_morph::UploadedMorph>,
    pub(super) ao:Option<Texture>,pub(super) lm_source:Option<std::sync::Arc<crate::lightmap::LmMeshSource>>,pub(super) bake_geometry:Option<std::rc::Rc<Geometry>>,pub(super) sdf:Option<(Texture,SdfMeta)>,
    pub(super) emitters: std::sync::Arc<Vec<crate::asset_lights::AssetLightEmitter>>,
    pub(super) extra_draws: Vec<(std::rc::Rc<Geometry>, Texture, Texture, [f32; 2], LayerMaterial)>,
    pub(super) liquids: std::sync::Arc<Vec<crate::model::LiquidSurface>>,
    pub(super) geometry: std::rc::Rc<Geometry>,
    pub(super) texture: Texture,
    pub(super) detail: Texture,
    pub(super) detail_scale: [f32;2],
    pub(super) material: LayerMaterial,
    pub(super) wants_pbr: bool,
    pub(super) prelit: bool,
    pub(super) triangles: usize,
    pub(super) min: Vec3f,
    pub(super) max: Vec3f,
    pub(super) stand_in: Option<Vec3f>,
    pub(super) authored_collisions: std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>,
    pub(super) collider_parts: Colliders,
    pub(super) occluder_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    pub(super) positions: std::sync::Arc<Vec<Vec3f>>,
    pub(super) indices: std::sync::Arc<Vec<u32>>,
    pub(super) anim_parts: Vec<LoadedAnimPart>,
    pub(super) driven_parts: Vec<LoadedDrivenPart>,
    pub(super) sky: Option<LoadedSky>,
}

impl UploadedStaticPreview {
    /// Route every occurrence of this named layer, including its LODs and rigid
    /// parts, before installing the model. Refreshes the lighting geometry
    /// when a layer is first excluded. Returns the number of marked layers.
    pub fn require_traced_layer(&mut self, cx: &mut Cx, layer: &str, program: &str, role: TracedLayerRole) -> usize {
        let mut count = 0;
        let mut changed = false;
        let program: std::sync::Arc<str> = program.into();
        for material in std::iter::once(&mut self.material)
            .chain(self.extra_draws.iter_mut().map(|l| &mut l.4))
            .chain(self.anim_parts.iter_mut().flat_map(|p| p.draws.iter_mut().map(|l| &mut l.4)))
            .chain(self.driven_parts.iter_mut().flat_map(|p| p.draws.iter_mut().map(|l| &mut l.4))) {
            if material.name.as_ref() == layer {
                changed |= material.traced_required.is_none();
                material.traced_required = Some((program.clone(), role));
                count += 1;
            }
        }
        if changed {
            self.exclude_traced_from_shadows(cx);
        }
        for (_, lod) in &mut self.lods { count += lod.require_traced_layer(cx, layer, &program, role); }
        count
    }
}

#[derive(Clone)]
pub(super) struct UploadedSkinMaterial {
    pub(super) geometry:std::rc::Rc<Geometry>,pub(super) base:Texture,pub(super) orm:Texture,pub(super) metallic:f32,pub(super) roughness:f32,pub(super) surface:crate::material_surface::UploadedSurface,
}

#[derive(Clone)]
pub struct UploadedSkinRig {
    pub(super) lods:Vec<(f32,UploadedSkinRig)>,
    pub(super) base_texture:Option<Texture>,
    pub(super) morph:Option<crate::asset_morph::UploadedMorph>,
    pub(super) sdf:Option<Option<(Texture,SdfMeta)>>,
    pub(super) materials: Vec<UploadedSkinMaterial>,
    pub(super) geometry: std::rc::Rc<Geometry>,
    pub(super) ao_map: Texture,
}

impl UploadedSkinRig {
    /// Consumes worker-validated buffers; no geometry copying or decoding.
    pub fn with_lods(mut self,cx:&mut Cx,lods:Vec<crate::asset_lod::PreparedSkinLod>)->Self{
        self.lods=lods.into_iter().map(|lod|{let mut rig=Self::upload_with_materials(cx,lod.rest,lod.materials).with_morph(cx,lod.morph);rig.base_texture=Some(upload_generated_texture(cx,lod.texture));(lod.distance,rig)}).collect();self
    }
    pub fn with_morph(mut self,cx:&mut Cx,morph:Option<crate::asset_morph::AssetMorph>)->Self{self.morph=morph.map(|m|m.upload(cx));self}
    pub fn with_prepared_sdf(mut self,cx:&mut Cx,sdf:Option<crate::shadow_sdf::ShadowSdfAtlas>)->Self{self.sdf=Some(sdf.map(|atlas|Renderer::upload_sdf_atlas(cx,atlas)));self}
    pub fn upload_with_materials(cx:&mut Cx,rest:crate::skin::SkinRestGpu,parts:Vec<crate::skin::PreparedSkinPart>)->Self {
        let mut uploaded=Self::upload(cx,rest);
        uploaded.materials=parts.into_iter().map(|part| {
            let geometry=Geometry::new(cx);geometry.update(cx,part.indices,part.vertices);
            UploadedSkinMaterial{geometry:std::rc::Rc::new(geometry),base:part.base.upload(cx),orm:part.orm.upload(cx),metallic:part.metallic,roughness:part.roughness,surface:part.surface.upload(cx)}
        }).collect();uploaded
    }
    pub fn upload(cx: &mut Cx, rest: crate::skin::SkinRestGpu) -> Self {
        let geometry = Geometry::new(cx);
        geometry.update(cx, rest.indices, rest.vertices);
        let ao_map = Texture::new_with_format(cx, TextureFormat::VecRu8 {
            width: rest.ao_size, height: rest.ao_size, data: Some(rest.ao_pixels),
            unpack_row_length: None, updated: TextureUpdated::Full,
        });
        Self { lods:Vec::new(),base_texture:None,morph:None,sdf:None,geometry: std::rc::Rc::new(geometry), ao_map, materials:Vec::new() }
    }
}

/// A stock prop resident on the GPU: geometry uploaded once, plus the pack
/// atlas it samples. Thousands of models share a few dozen atlases, which is
/// what keeps a whole pack's worth of props cheap to draw.
#[derive(Clone)]
pub(super) struct LoadedModel {
    pub(super) lods:Vec<(f32,LoadedModel)>,
    pub(super) ordinary_shadow_bounds: Option<std::rc::Rc<super::lights::OrdinaryShadowBounds>>,
    pub(super) morph:Option<crate::asset_morph::UploadedMorph>,
    pub(super) prepared_sdf:Option<Option<(Texture,SdfMeta)>>,
    pub(super) emitters: std::sync::Arc<Vec<crate::asset_lights::AssetLightEmitter>>,
    pub(super) geometry: std::rc::Rc<Geometry>,
    pub(super) texture: Texture,
    pub(super) detail: Texture,
    pub(super) detail_scale: [f32; 2],
    /// Extra (geometry, albedo, detail, scale, material) draws for world GLBs
    /// that embed one PNG per tile. The first layer is `geometry`/`texture`.
    pub(super) extra_draws: Vec<(std::rc::Rc<Geometry>, Texture, Texture, [f32; 2], LayerMaterial)>,
    /// Layer 0's material (the merged stream's, for a single-layer model).
    pub(super) material: LayerMaterial,
    /// Draw this model through [`DrawScenePbr`] instead of
    /// [`DrawSceneSkinned`]. Decided ONCE here, at load, from the glTF
    /// material: true when the model is not prelit and some layer's material
    /// carries real shininess ([`crate::model::PbrMaterial::is_shiny`]).
    ///
    /// Per model rather than per layer because the shader is a property of
    /// the draw item, and splitting one prop across two shaders would double
    /// its draw calls to give a matte layer a lobe it cannot show anyway.
    pub(super) wants_pbr: bool,
    /// COLOR_0 is a baked lightmap — skip the analytic sun multiply.
    pub(super) prelit: bool,
    pub(super) triangles: usize,
    /// Model-space bounds, kept so a caller can build a collider without
    /// re-parsing the GLB. A prop the player walks through is not in the
    /// world, it is painted on it.
    pub(super) min: Vec3f,
    pub(super) max: Vec3f,
    /// The model-space centre of the model's far stand-in (impostor.rs), when
    /// it has one: the stand-in casts at that centre's depth in the outer
    /// cascades, and the model's own layers compare no deeper (shaders/csm.rs
    /// `csm_own_limit`), so it never shadows the model it stands in for.
    pub(super) stand_in: Option<Vec3f>,
    /// Physics collider boxes in model space — triangle-derived voxel boxes
    /// (model.rs voxel_collider_boxes): legs, decks, braces, openings.
    pub(super) authored_collisions: std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>,
    pub(super) collider_parts: Colliders,
    /// Light-bake occluder boxes — the OLD curated primitive parts: few and
    /// face-aligned. Voxel boxes as occluders smeared streaks over every
    /// sloped roof (a stepped AABB pokes through the surface) and tripled
    /// the bake; physics and light need DIFFERENT simplifications.
    pub(super) occluder_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    /// The light baker's view of this model — triangles + grid + chart uvs —
    /// present only when the model has its OWN AO layout (the layout is what
    /// gives every placed copy a lightmap parameterisation for free).
    pub(super) lm_source: Option<std::sync::Arc<crate::lightmap::LmMeshSource>>,
    /// Rigid parts the game drives between named states (doors, lifts). Not
    /// in `geometry`: they move, so they are neither baked into the static
    /// lightmap nor part of the model's collider.
    pub(super) anim_parts: Vec<LoadedAnimPart>,
    /// Rigid parts whose complete model-space pose is supplied by each
    /// ModelInstance (vehicle wheels are the first consumer).
    pub(super) driven_parts: Vec<LoadedDrivenPart>,
    /// The map's sky surfaces, drawn by view direction instead of lit. Also
    /// out of `geometry`: the bake must never see them (they would shadow
    /// the whole level from above) and the sun must never shade them.
    pub(super) sky: Option<LoadedSky>,
    /// A level's liquid surfaces (StaticModel::liquids), handed to the host
    /// for the water renderer; never drawn by the model lanes.
    pub(super) liquids: std::sync::Arc<Vec<crate::model::LiquidSurface>>,
    /// The model's own triangles in MODEL space, kept after the GPU upload
    /// consumed the packed stream. This is the level-collision source
    /// (`level.rs` builds its BVH from it) — 16 bytes a vertex-and-index
    /// against re-parsing a 100 MB GLB to answer "what did the player walk
    /// into". Anim parts are deliberately absent: they move, and a BVH built
    /// over them would be wrong the moment a door opened.
    pub(super) mesh_positions: std::sync::Arc<Vec<Vec3f>>,
    pub(super) mesh_indices: std::sync::Arc<Vec<u32>>,
    /// The GPU light bake's variant of `geometry`: identical positions and
    /// chart uvs, but the normal lane holds the FLAT WINDING face normal.
    /// The CPU bake's `sun_bit` classified every texel against the winding
    /// normal of its owning triangle (a dedup'd double-sided kit face whose
    /// kept twin was authored inward carries a vertex normal OPPOSITE its
    /// winding — the crypt's paver-floor sheets), so the gather must see the
    /// winding, not the shading normal. Present iff `lm_source` is.
    pub(super) bake_geometry: Option<std::rc::Rc<Geometry>>,
}

/// One resident [`crate::model::AnimPart`]: its definition (states, clip,
/// bounds) plus the uploaded geometry it draws through.
#[derive(Clone)]
pub(super) struct LoadedAnimPart {
    pub(super) def: std::sync::Arc<crate::model::AnimPart>,
    /// (geometry, albedo, detail, detail scale) per texture, mirroring the
    /// static model's own layer list so a part draws through the same code.
    pub(super) draws: Vec<(std::rc::Rc<Geometry>, Texture, Texture, [f32; 2],LayerMaterial)>,
    /// Node-local collider boxes, derived once at load.
    pub(super) collider: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
}

#[derive(Clone)]
pub(super) struct LoadedDrivenPart {
    pub(super) def: std::sync::Arc<crate::model::DrivenPart>,
    pub(super) draws: Vec<(std::rc::Rc<Geometry>, Texture, Texture, [f32; 2],LayerMaterial)>,
}

/// Whether a sky projection's layer images get a mip chain.
///
/// A sky is sampled by DIRECTION, so `atan2` puts a branch cut in the
/// longitude: at one heading, two neighbouring pixels differ by a whole
/// texture period in u. The colour is fine — the sampler repeats — but the
/// hardware picks the mip level from exactly that derivative, so those
/// pixels collapse to the smallest level and the sky wears a one-pixel line
/// down the seam. (Measured on Doom E1M1: a dark column in the middle of the
/// sky, wherever the cut happened to be.)
///
/// A CYLINDER strip (Doom, Duke) is never minified — 256 texels x 4 wraps is
/// under 3 texels per degree against 8+ pixels per degree in any view that
/// exists — so its chain buys nothing and costs the seam. It ships without
/// one, and the seam has nowhere to come from.
///
/// The other two keep theirs: QUAKE_SCROLL has no cut at all (its uv is a
/// smooth function of the direction) and its zenith swirl genuinely
/// minifies; CUBE (Q3 equirect) minifies hard at the poles, where dropping
/// mips would trade a hairline for sparkle. Cube still has the cut — the
/// real fix there is an explicit LOD from ray-space derivatives, which needs
/// a repeat sampler that takes a lod.
pub(super) fn sky_wants_mips(projection: crate::model::SkyProjection) -> bool {
    !matches!(projection, crate::model::SkyProjection::Cylinder)
}

pub(super) fn sky_trace_enabled() -> bool {
    std::env::var_os("MAKEPAD_SKY_TRACE").is_some()
}

/// A resident [`crate::model::SkyPart`]: the faces plus their layer images.
#[derive(Clone)]
pub(super) struct LoadedSky {
    pub(super) part: std::sync::Arc<crate::model::SkyPart>,
    pub(super) geometry: std::rc::Rc<Geometry>,
    /// Layer 0 and layer 1 textures. Layer 1 is a 1x1 transparent stand-in
    /// unless the map is a two-layer Quake sky, so the shader samples
    /// unconditionally and every projection stays one draw.
    pub(super) tex0: Texture,
    pub(super) tex1: Texture,
    /// One-shot trace bits: bit 0 records a frustum rejection, bit 1 a draw
    /// submission. Kept per resident model so switching maps is traced too.
    pub(super) draw_trace: u8,
    /// Model-space triangles, kept for the same reason `mesh_positions` is:
    /// a sky face is still a WALL to a walker (Doom's sky brushes are
    /// solid), even though it is never lit or shadowed.
    pub(super) positions: std::sync::Arc<Vec<Vec3f>>,
    pub(super) indices: std::sync::Arc<Vec<u32>>,
}

/// The model-space centre of a far stand-in's quad: its corners are the
/// centre plus and minus the radius in the model's XY plane (impostor.rs),
/// so the middle of its vertices' bounds.
pub(super) fn stand_in_centre(vertices: &[f32]) -> Option<Vec3f> {
    let mut points = vertices.chunks_exact(crate::model::MODEL_VERTEX_FLOATS).map(|v| vec3f(v[0], v[1], v[2]));
    let first = points.next()?;
    let (lo, hi) = points.fold((first, first), |(lo, hi), p| {
        (vec3f(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)), vec3f(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)))
    });
    Some((lo + hi) * 0.5)
}
