//! Off-thread prepared model payloads and their uploaded/loaded GPU forms.

use super::*;

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
pub fn decode_generated_png_within(bytes: &[u8], dimension: usize, byte_limit: usize) -> Result<ImageBuffer, String> {
    match decode_generated_png(bytes, dimension, byte_limit) {
        Err(e) if e.contains("budget") => {
            let mut image = decode_generated_png(bytes, dimension, dimension * dimension * 4)?;
            let (w0, h0) = (image.width, image.height);
            while image.data.len() * 4 > byte_limit.max(4) && (image.width > 1 || image.height > 1) {
                let (w, h) = (image.width, image.height);
                let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
                let mut data = Vec::with_capacity(nw * nh);
                for y in 0..nh {
                    for x in 0..nw {
                        let mut sum = [0u32; 4];
                        for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                            let p = image.data[(y * 2 + dy).min(h - 1) * w + (x * 2 + dx).min(w - 1)];
                            for (i, s) in sum.iter_mut().enumerate() {
                                *s += (p >> (i * 8)) & 255;
                            }
                        }
                        data.push(sum.iter().enumerate().fold(0u32, |acc, (i, s)| acc | (((s + 2) / 4) << (i * 8))));
                    }
                }
                image.data = data;
                image.width = nw;
                image.height = nh;
            }
            log!("generated texture {w0}x{h0} over the prepare budget: downscaled to {}x{}", image.width, image.height);
            Ok(image)
        }
        other => other,
    }
}

/// Upload a prepared generated atlas with repeat addressing and one level.
/// This never builds CPU mips, including when image-cache mipmaps are enabled.
pub fn upload_generated_texture(cx: &mut Cx, image: ImageBuffer) -> Texture {
    Texture::new_with_format(cx, TextureFormat::VecMipBGRAu8_32 {
        width: image.width, height: image.height, data: Some(image.data),
        max_level: Some(0), wrap: TextureWrap::Repeat, updated: TextureUpdated::Full,
    })
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
    pub(super) collider_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    pub(super) occluder_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    pub(super) anim_parts: Vec<PreparedAnimPreview>,
    pub(super) driven_parts: Vec<PreparedDrivenPreview>,
    pub(super) sky: Option<PreparedSky>,
    pub(super) min: Vec3f, pub(super) max: Vec3f, pub(super) prelit: bool,
}

impl PreparedStaticPreview {
    pub fn lod_distances(&self)->impl Iterator<Item=f32>+'_ {self.lods.iter().map(|(distance,_)|*distance)}
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
    pub fn prepare_with_sidecars(mut model:StaticModel,ao_png:Option<&[u8]>,sdf_bytes:Option<&[u8]>)->Result<Self,String>{
        let ao=ao_png.map(|bytes|decode_generated_png(bytes,4096,32*1024*1024)).transpose()?;
        let sdf=sdf_bytes.and_then(|bytes|crate::shadow_sdf::ShadowSdfAtlas::from_shadowsdf(bytes).map(|v|v.0));
        if model.texture_uri.is_some() && model.texture_png.is_none() { return Err("model references an external texture without supplied pixels".into()); }
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        let check = |vertices: &[f32], indices: &[u32]| -> Result<(),String> {
            if vertices.len()%stride != 0 || indices.len()%3 != 0 || indices.iter().any(|&i|i as usize >= vertices.len()/stride)
                || vertices.chunks_exact(stride).any(|v|v[..3].iter().any(|x|!x.is_finite())) { return Err("prepared model has invalid geometry".into()); }
            Ok(())
        };
        check(&model.vertices,&model.indices)?;
        if model.indices.len()<3 && model.anim_parts.is_empty() && model.driven_parts.is_empty() { return Err("prepared model has no triangles".into()); }
        let positions:Vec<Vec3f> = model.vertices.chunks_exact(stride).map(|v|vec3f(v[0],v[1],v[2])).collect();
        let mesh_indices = std::sync::Arc::new(model.indices.clone());
        let lm_source=ao.as_ref().map(|image|std::sync::Arc::new(crate::lightmap::LmMeshSource{
            caster:crate::ao::MeshRaycaster::new(positions.clone(),model.indices.clone(),model.min,model.max),
            ao_uv:model.vertices.chunks_exact(stride).map(|v|crate::model::unpack_ao_uv(v[6])).collect(),
            albedo:model.vertices.chunks_exact(stride).map(|v|{let b=v[5].to_bits();vec3f((b&255)as f32/255.0,((b>>8)&255)as f32/255.0,((b>>16)&255)as f32/255.0)}).collect(),
            ao_w:image.width,ao_h:image.height,
        }));
        let bake_stream=ao.as_ref().map(|_|{
            let mut vertices=Vec::with_capacity(model.indices.len()*stride);let mut indices=Vec::with_capacity(model.indices.len());
            for tri in model.indices.chunks_exact(3){let a=positions[tri[0]as usize];let b=positions[tri[1]as usize];let c=positions[tri[2]as usize];
                let n=Vec3f::cross(b-a,c-a);let length=n.length();let n=if length>1e-12{n*(1.0/length)}else{vec3f(0.0,1.0,0.0)};
                let (x,y)=crate::skin::oct_encode(n);let normal=makepad_draw::pack_pair_f16(x,y);
                for index in tri{let offset=*index as usize*stride;indices.push((vertices.len()/stride)as u32);vertices.extend_from_slice(&model.vertices[offset..offset+stride]);let at=vertices.len()-stride+3;vertices[at]=normal;}
            }(indices,vertices)
        });
        let occluder_parts = model.collider_parts();
        let collider_parts = { let boxes=model.voxel_collider_boxes(); if boxes.is_empty(){occluder_parts.clone()}else{boxes} };
        let remaining = std::cell::Cell::new(128usize*1024*1024);
        // Many layers of one model share an image (a map's 28 meshes use
        // ONE atlas): decode, mip and charge each distinct image once, and
        // share the prepared pixels (upload dedupes them too).
        let shared = std::cell::RefCell::new(std::collections::HashMap::<(u64,usize,u32,u8),std::sync::Arc<crate::material_surface::PreparedTexture>>::new());
        let image = |bytes:Option<&[u8]>,fallback:u32,semantic:crate::material_surface::PixelSemantic| -> Result<std::sync::Arc<crate::material_surface::PreparedTexture>,String> {
            // The same image as colour and as a masked cutout mips differently.
            let kind=semantic as u8;
            // The load-ready form: pre-mipped UASTC (KTX2) in the GLB, keyed by
            // its builder-assigned id and remapped to the device format (no
            // decode, mips or hashing).
            if let Some(bytes)=bytes.filter(|b|crate::texture_pack::is_ktx2(b)) {
                let key=(crate::texture_pack::content_id(bytes).unwrap_or(0),bytes.len(),1,kind);
                if let Some(texture)=shared.borrow().get(&key){return Ok(texture.clone());}
                let texture=crate::material_surface::remapped_ktx2(bytes).expect("KTX2 bytes")?;
                remaining.set(remaining.get().saturating_sub(texture.bytes()));
                shared.borrow_mut().insert(key,texture.clone());
                return Ok(texture);
            }
            let key=match bytes {
                Some(bytes)=>(crate::material_surface::texture_work(crate::material_surface::TextureWork::Hash,||bytes.iter().fold(0xcbf2_9ce4_8422_2325u64,|h,b|(h^*b as u64).wrapping_mul(0x100_0000_01b3))),bytes.len(),0,kind),
                None=>(0,0,fallback,kind),
            };
            if let Some(texture)=shared.borrow().get(&key){return Ok(texture.clone());}
            // PNG images (a level's material images, content published before
            // the load-ready form) decode and mip here, once per load for all
            // the models that use them.
            let prepare=||{
            let store=crate::material_surface::prepared_texture_store();
            // A host cache of prepared textures skips the decode and the mips.
            // Small images prepare faster than a cache round trip.
            let store_key=bytes.filter(|b|b.len()>=crate::material_surface::PREPARED_TEXTURE_CACHE_MIN_SOURCE_BYTES).map(|b|crate::material_surface::texture_work(crate::material_surface::TextureWork::Hash,||crate::material_surface::prepared_texture_key(b,semantic)));
            let cached=match (store,store_key) {
                (Some(store),Some(store_key))=>crate::material_surface::texture_work(crate::material_surface::TextureWork::Cache,||store.get(store_key)).filter(|t|t.data.len()*4<=remaining.get()),
                _=>None,
            };
            let texture=match cached {
                Some(texture)=>{ remaining.set(remaining.get()-texture.width*texture.height*4); texture }
                None=>{
                    let decoded=if let Some(bytes)=bytes {
                        let image=crate::material_surface::texture_work(crate::material_surface::TextureWork::DecodeMip,||decode_generated_png_within(bytes,4096,remaining.get()))?;
                        remaining.set(remaining.get().saturating_sub(image.data.len()*4)); image
                    } else { let mut image=ImageBuffer::default();image.width=1;image.height=1;image.data=vec![fallback];image };
                    let texture=crate::material_surface::texture_work(crate::material_surface::TextureWork::DecodeMip,||crate::material_surface::PreparedTexture::prepare(decoded,semantic));
                    if let (Some(store),Some(store_key))=(store,store_key) {
                        if texture.width*texture.height>=crate::material_surface::PREPARED_TEXTURE_CACHE_MIN_TEXELS { crate::material_surface::texture_work(crate::material_surface::TextureWork::Cache,||store.put(store_key,&texture)); }
                    }
                    texture
                }
            };
            Ok(texture)
            };
            let texture=match bytes { Some(_)=>crate::material_surface::prepared_once((key.0,key.1,key.3),prepare)?, None=>std::sync::Arc::new(prepare()?) };
            shared.borrow_mut().insert(key,texture.clone());
            Ok(texture)
        };
        let layer = |vertices:Vec<f32>,indices:Vec<u32>,png:Option<Vec<u8>>,detail:Option<Vec<u8>>,detail_scale:[f32;2],mut pbr:crate::model::PbrMaterial| -> Result<PreparedStaticLayer,String> {
            check(&vertices,&indices)?;
            // An alpha-tested layer keeps its cutout coverage down the mips.
            let masked=pbr.surface.as_ref().is_some_and(|s|s.alpha_mode==1);
            let texture=image(png.as_deref(),0xffff_ffff,if masked{crate::material_surface::PixelSemantic::MaskedColor}else{crate::material_surface::PixelSemantic::Color})?;
            let detail_on=detail.is_some();let detail=image(detail.as_deref(),0xff80_8080,crate::material_surface::PixelSemantic::Color)?;
            let orm_on=pbr.orm_png.is_some();let orm=image(pbr.orm_png.as_deref(),0xffff_ffff,crate::material_surface::PixelSemantic::Data)?;
            let surface=pbr.surface.as_ref().map(|surface|{let mut budget=remaining.get();let prepared=crate::material_surface::PreparedSurface::prepare(surface.as_ref().clone(),&mut budget)?;remaining.set(budget);Ok::<_,String>(prepared)}).transpose()?;
            pbr.orm_png=None;
            let cutout=texture.cutout();
            Ok(PreparedStaticLayer { vertices,indices,texture,detail,detail_scale:if detail_on{detail_scale}else{[0.0,0.0]},orm,orm_on,surface,pbr,cutout })
        };
        let mut layers=std::mem::take(&mut model.draw_layers).into_iter();
        let main = if let Some(first)=layers.next(){ layer(first.vertices,first.indices,first.texture_png,first.detail_png,first.detail_scale,first.pbr)? }
            else {layer(std::mem::take(&mut model.vertices),std::mem::take(&mut model.indices),model.texture_png.take(),model.detail_png.take(),model.detail_scale,model.pbr.clone())?};
        let mut extra=Vec::new();
        for value in layers {extra.push(layer(value.vertices,value.indices,value.texture_png,value.detail_png,value.detail_scale,value.pbr)?);}
        let mut anim_parts=Vec::new();
        for mut def in std::mem::take(&mut model.anim_parts) {
            let collider=std::sync::Arc::new(def.collider_boxes());
            let mut draws=Vec::new();
            for value in std::mem::take(&mut def.layers) {draws.push(layer(value.vertices,value.indices,value.texture_png,value.detail_png,value.detail_scale,value.pbr)?);}
            if draws.is_empty(){draws.push(layer(std::mem::take(&mut def.vertices),std::mem::take(&mut def.indices),None,None,[0.0,0.0],Default::default())?);}
            def.vertices.clear();
            anim_parts.push(PreparedAnimPreview{def:std::sync::Arc::new(def),draws,collider});
        }
        let mut driven_parts=Vec::new();
        for mut def in std::mem::take(&mut model.driven_parts) {
            let mut draws=Vec::new();
            for value in std::mem::take(&mut def.layers){draws.push(layer(value.vertices,value.indices,value.texture_png,value.detail_png,value.detail_scale,value.pbr)?);}
            driven_parts.push(PreparedDrivenPreview{def:std::sync::Arc::new(def),draws});
        }
        let sky=if let Some(mut part)=model.sky.take(){
            check(&part.vertices,&part.indices)?;
            let positions=std::sync::Arc::new(part.vertices.chunks_exact(stride).map(|v|vec3f(v[0],v[1],v[2])).collect());
            let mesh_indices=std::sync::Arc::new(part.indices.clone());
            // Sky images stay single-level (their wrap seam would pick a
            // tiny mip), decoded directly within the same budget.
            let sky_image=|bytes:Option<&[u8]>,fallback:u32|->Result<ImageBuffer,String>{
                if let Some(bytes)=bytes { let image=decode_generated_png_within(bytes,4096,remaining.get())?; remaining.set(remaining.get().saturating_sub(image.data.len()*4)); Ok(image) }
                else { let mut image=ImageBuffer::default();image.width=1;image.height=1;image.data=vec![fallback];Ok(image) }
            };
            let tex0=sky_image(part.images.first().map(Vec::as_slice),0xffff_ffff)?;
            let tex1=sky_image(part.images.get(1).map(Vec::as_slice),0)?;
            let vertices=std::mem::take(&mut part.vertices);let indices=std::mem::take(&mut part.indices);part.images.clear();
            Some(PreparedSky{part:std::sync::Arc::new(part),vertices,indices,positions,mesh_indices,tex0,tex1})
        }else{None};
        Ok(Self{lods:Vec::new(),morph:None,ao,lm_source,bake_stream,sdf,emitters:Default::default(),main,extra,positions:std::sync::Arc::new(positions),mesh_indices,
            authored_collisions:Default::default(),collider_parts:std::sync::Arc::new(collider_parts),occluder_parts:std::sync::Arc::new(occluder_parts),anim_parts,driven_parts,sky,
            min:model.min,max:model.max,prelit:model.prelit})
    }
    /// The GPU textures this asset uploads, as (content hash, bytes), each
    /// once: identical images share one GPU texture across assets (see
    /// `upload_static_preview`), so a host charging memory per asset can
    /// charge a shared texture once.
    pub fn texture_charges(&self)->Vec<(u64,usize)> {
        let mut out=Vec::new();
        self.collect_texture_charges(&mut out);
        out
    }
    fn collect_texture_charges(&self,out:&mut Vec<(u64,usize)>) {
        for (_,lod) in &self.lods { lod.collect_texture_charges(out); }
        for l in std::iter::once(&self.main).chain(&self.extra).chain(self.anim_parts.iter().flat_map(|p|&p.draws)).chain(self.driven_parts.iter().flat_map(|p|&p.draws)) {
            let surface=l.surface.iter().flat_map(|s|[&s.normal,&s.occlusion,&s.emissive]);
            for t in [&*l.texture,&*l.detail,&*l.orm].into_iter().chain(surface) {
                if !out.iter().any(|(h,_)|*h==t.hash) { out.push((t.hash,t.bytes())); }
            }
        }
    }
    pub fn upload_bytes(&self)->usize {
        self.lods.iter().map(|(_,m)|m.upload_bytes()).sum::<usize>()+self.morph.as_ref().map_or(0,|m|m.pixels.len()*4)+self.ao.as_ref().map_or(0,|v|v.data.len()*4)+self.sdf.as_ref().map_or(0,|v|v.pixels.len())+self.bake_stream.as_ref().map_or(0,|(i,v)|(i.len()+v.len())*4)
            +{let mut seen=std::collections::HashSet::new();std::iter::once(&self.main).chain(&self.extra).chain(self.anim_parts.iter().flat_map(|p|&p.draws)).chain(self.driven_parts.iter().flat_map(|p|&p.draws)).map(|l|l.bytes_unique(&mut seen)).sum::<usize>()}
            +self.sky.as_ref().map_or(0,|s|(s.vertices.len()+s.indices.len()+s.tex0.data.len()+s.tex1.data.len())*4)
    }
}
#[derive(Clone)]
pub(super) struct PreparedStaticLayer {
    pub(super) vertices:Vec<f32>,pub(super) indices:Vec<u32>,pub(super) texture:std::sync::Arc<crate::material_surface::PreparedTexture>,pub(super) detail:std::sync::Arc<crate::material_surface::PreparedTexture>,pub(super) detail_scale:[f32;2],
    pub(super) orm:std::sync::Arc<crate::material_surface::PreparedTexture>,pub(super) orm_on:bool,pub(super) surface:Option<crate::material_surface::PreparedSurface>,pub(super) pbr:crate::model::PbrMaterial,
    /// The base texture can fail the alpha test (found on the worker; opaque.rs).
    pub(super) cutout:bool,
}
impl PreparedStaticLayer {
    /// Geometry and surface bytes, plus each image not already in `seen`
    /// (layers share images; a shared atlas is uploaded and counted once).
    pub(super) fn bytes_unique(&self,seen:&mut std::collections::HashSet<*const crate::material_surface::PreparedTexture>)->usize{
        let mut images=0;
        for t in [&self.texture,&self.detail,&self.orm]{if seen.insert(std::sync::Arc::as_ptr(t)){images+=t.bytes();}}
        (self.vertices.len()+self.indices.len())*4+images+self.surface.as_ref().map_or(0,|s|s.bytes())
    }
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
    pub(super) morph:Option<crate::asset_morph::UploadedMorph>,
    pub(super) ao:Option<Texture>,pub(super) lm_source:Option<std::sync::Arc<crate::lightmap::LmMeshSource>>,pub(super) bake_geometry:Option<std::rc::Rc<Geometry>>,pub(super) sdf:Option<(Texture,SdfMeta)>,
    pub(super) emitters: std::sync::Arc<Vec<crate::asset_lights::AssetLightEmitter>>,
    pub(super) extra_draws: Vec<(std::rc::Rc<Geometry>, Texture, Texture, [f32; 2], LayerMaterial)>,
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
    pub(super) authored_collisions: std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>,
    pub(super) collider_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    pub(super) occluder_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
    pub(super) positions: std::sync::Arc<Vec<Vec3f>>,
    pub(super) indices: std::sync::Arc<Vec<u32>>,
    pub(super) anim_parts: Vec<LoadedAnimPart>,
    pub(super) driven_parts: Vec<LoadedDrivenPart>,
    pub(super) sky: Option<LoadedSky>,
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
    /// Physics collider boxes in model space — triangle-derived voxel boxes
    /// (model.rs voxel_collider_boxes): legs, decks, braces, openings.
    pub(super) authored_collisions: std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>,
    pub(super) collider_parts: std::sync::Arc<Vec<(Vec3f, Vec3f)>>,
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
