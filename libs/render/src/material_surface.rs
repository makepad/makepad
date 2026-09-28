//! Portable material pixels prepared on workers, including explicit mip data.
use makepad_draw::{ImageBuffer, makepad_platform::{Cx,Texture,TextureFormat,TextureUpdated,TextureWrap}};

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialSurface {
    pub fur: Option<makepad_gltf::GlbFurMaterial>,
    pub normal_png: Option<Vec<u8>>,
    pub normal_scale: f32,
    pub occlusion_png: Option<Vec<u8>>,
    pub occlusion_strength: f32,
    pub emissive_png: Option<Vec<u8>>,
    pub emissive: [f32;3],
    /// 0 opaque, 1 mask, 2 blend.
    pub alpha_mode: u8,
    pub alpha_cutoff: f32,
    pub base_alpha: f32,
    pub double_sided: bool,
    /// UV scale (1/metres) of world-position triplanar texturing; 0 = the
    /// mesh's own UVs. Generated terrain asks for it (glTF extras
    /// `makepadTriplanar`) so cliffs keep a seamless, unstretched texture.
    pub triplanar: f32,
}
impl Default for MaterialSurface {
    fn default()->Self {Self{fur:None,normal_png:None,normal_scale:1.0,occlusion_png:None,occlusion_strength:1.0,
        emissive_png:None,emissive:[0.0;3],alpha_mode:0,alpha_cutoff:0.5,base_alpha:1.0,double_sided:false,triplanar:0.0}}
}

pub(crate) fn fur_params(fur: Option<makepad_gltf::GlbFurMaterial>) -> makepad_draw::Vec4f {
    fur.filter(|fur| fur.valid()).map_or_else(Default::default, |fur| {
        makepad_draw::vec4(fur.length as f32, fur.density as f32, fur.scale as f32, fur.seed as f32)
    })
}

/// Fixed layer and triangle ceilings apply on every platform, including Quest.
/// The source mesh stays resident once; only draw instances are repeated.
pub(crate) fn fur_shell_count(length: f32, transform: &makepad_draw::Mat4f, distance: f32, triangles: usize, budget: &mut usize) -> usize {
    if length <= 0.0 || triangles == 0 || !distance.is_finite() { return 0; }
    // Account for model scaling, including miniature construction previews.
    // This angular-size heuristic is independent of the headset eye: both
    // eyes use the scene camera and retain the same number of layers.
    let scale = [0, 4, 8].into_iter().map(|i| {
        (transform.v[i].powi(2) + transform.v[i + 1].powi(2) + transform.v[i + 2].powi(2)).sqrt()
    }).fold(0.0f32, f32::max);
    let detail = length * scale * 800.0 / distance.max(0.1);
    if !detail.is_finite() { return 0; }
    let desired = if detail < 1.0 { 0 } else if detail < 3.0 { 2 } else if detail < 6.0 { 4 } else { 6 };
    let shells = desired.min(*budget / triangles);
    *budget -= shells * triangles;
    shells
}

#[derive(Clone,Copy)]
pub enum PixelSemantic { Color, Data, Normal,
    /// Base colour of an alpha-TESTED (MASK) material: filtered like
    /// `Color`, with each mip's alpha coverage kept (see `prepare`).
    MaskedColor }

#[derive(Clone)]
pub struct PreparedTexture {pub width:usize,pub height:usize,pub data:Vec<u32>,pub max_level:usize,
    /// Content hash (size + level-0 texels), computed on the worker: the
    /// renderer's texture cache shares one GPU texture per distinct image
    /// across every model and chunk that uses it.
    pub hash:u64}
fn linear(v:f32)->f32{if v<=0.04045{v/12.92}else{((v+0.055)/1.055).powf(2.4)}}
fn srgb(v:f32)->f32{if v<=0.0031308{v*12.92}else{1.055*v.max(0.0).powf(1.0/2.4)-0.055}}
/// A host's cache of prepared (mipped) textures, keyed by
/// [`prepared_texture_key`] of the SOURCE image bytes. Opt-in: a host that
/// installs none prepares every image as before. `get` and `put` run on
/// preparation workers; `put` must not block on I/O.
pub trait PreparedTextureStore: Send + Sync {
    fn get(&self, key: u64) -> Option<PreparedTexture>;
    fn put(&self, key: u64, texture: &PreparedTexture);
}
static TEXTURE_STORE: std::sync::OnceLock<Box<dyn PreparedTextureStore>> = std::sync::OnceLock::new();
/// Install the process's prepared-texture cache (first call wins).
pub fn set_prepared_texture_store(store: Box<dyn PreparedTextureStore>) { let _ = TEXTURE_STORE.set(store); }
pub(crate) fn prepared_texture_store() -> Option<&'static dyn PreparedTextureStore> { TEXTURE_STORE.get().map(|s| s.as_ref()) }
/// Bumped whenever [`PreparedTexture::prepare`] or the serialized layout
/// changes, so cached textures from an older build are never used.
pub const PREPARED_TEXTURE_FORMAT: u32 = 1;
/// Cache key of a source image: its bytes, its semantic (colour and masked
/// colour mip differently) and the format version.
pub fn prepared_texture_key(source: &[u8], semantic: PixelSemantic) -> u64 {
    let h = source.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3));
    h ^ (source.len() as u64).rotate_left(29) ^ ((semantic as u64) << 56) ^ ((PREPARED_TEXTURE_FORMAT as u64) << 48).rotate_left(7)
}
/// Images smaller than this prepare faster than a cache round trip.
pub const PREPARED_TEXTURE_CACHE_MIN_TEXELS: usize = 256 * 256;
/// Source images (encoded bytes) smaller than this are not looked up.
pub const PREPARED_TEXTURE_CACHE_MIN_SOURCE_BYTES: usize = 32 * 1024;

impl PreparedTexture {
    /// Serialized form: magic, format, width, height, max_level, hash, texels
    /// (level 0 then every mip, BGRA8 little-endian words).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.data.len() * 4);
        out.extend_from_slice(b"MPTX");
        for v in [PREPARED_TEXTURE_FORMAT, self.width as u32, self.height as u32, self.max_level as u32] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(&self.hash.to_le_bytes());
        for t in &self.data { out.extend_from_slice(&t.to_le_bytes()); }
        out
    }
    /// The inverse of [`Self::to_bytes`]; `None` for another format or a
    /// truncated / inconsistent payload (the caller then prepares from source).
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 28 || &bytes[..4] != b"MPTX" { return None; }
        let word = |i: usize| u32::from_le_bytes(bytes[4 + i * 4..8 + i * 4].try_into().unwrap());
        if word(0) != PREPARED_TEXTURE_FORMAT { return None; }
        let (width, height, max_level) = (word(1) as usize, word(2) as usize, word(3) as usize);
        let hash = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
        // Texel count of the full chain this header promises.
        let (mut w, mut h, mut texels) = (width, height, width * height);
        for _ in 0..max_level { w = (w / 2).max(1); h = (h / 2).max(1); texels += w * h; }
        if width == 0 || height == 0 || width > 16384 || height > 16384 || bytes.len() != 28 + texels * 4 { return None; }
        let data = bytes[28..].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        Some(Self { width, height, data, max_level, hash })
    }
    pub fn prepare(image:ImageBuffer,semantic:PixelSemantic)->Self {
        let (width,height)=(image.width,image.height);let mut data=image.data;
        let hash=data.iter().fold(0xcbf2_9ce4_8422_2325u64^((width as u64)<<32|height as u64)^(semantic as u64).rotate_left(17),|h,t|(h^*t as u64).wrapping_mul(0x100_0000_01b3));
        // Alpha coverage (MaskedColor): box filtering pulls every mip's
        // alpha toward its mean, so an alpha-TESTED texture (a mask cutout, a terrain
        // overlay's height × splat weight) loses its coverage with distance
        // — a splat layer under weight ~0.5 vanished outright past a few
        // metres. Each level's alpha is remapped to level 0's distribution
        // (histogram match over the 256 byte values), which keeps the share
        // of texels above ANY cutoff, whatever the weight multiplies in.
        let alpha_hist=|texels:&[u32]|{let mut hist=[0u32;256];for t in texels{hist[(t>>24)as usize]+=1;}hist};
        let hist0=alpha_hist(&data);
        let varied=hist0.iter().filter(|c|**c>0).count()>1;
        let(mut w,mut h,mut start,mut max_level)=(width,height,0usize,0usize);
        while w>1||h>1 {
            let(nw,nh)=((w/2).max(1),(h/2).max(1));let offset=data.len();
            for y in 0..nh {for x in 0..nw {
                let mut sum=[0.0f32;4];
                for(dy,dx)in[(0,0),(0,1),(1,0),(1,1)] {
                    let pixel=data[start+(y*2+dy).min(h-1)*w+(x*2+dx).min(w-1)];
                    let mut c=[((pixel>>16)&255)as f32/255.0,((pixel>>8)&255)as f32/255.0,(pixel&255)as f32/255.0,(pixel>>24)as f32/255.0];
                    match semantic {PixelSemantic::Color|PixelSemantic::MaskedColor=>{for i in 0..3{c[i]=linear(c[i])*c[3];}},PixelSemantic::Normal=>{for i in 0..3{c[i]=c[i]*2.0-1.0;}},PixelSemantic::Data=>{}}
                    for i in 0..4{sum[i]+=c[i]*0.25;}
                }
                match semantic {
                    PixelSemantic::Color|PixelSemantic::MaskedColor=>{for i in 0..3{sum[i]=srgb(if sum[3]>1e-8{sum[i]/sum[3]}else{0.0});}},
                    PixelSemantic::Normal=>{let length=(sum[0]*sum[0]+sum[1]*sum[1]+sum[2]*sum[2]).sqrt();for i in 0..3{sum[i]=if length>1e-8{sum[i]/length*0.5+0.5}else{if i==2{1.0}else{0.5}};}},
                    PixelSemantic::Data=>{},
                }
                let byte=|v:f32|(v.clamp(0.0,1.0)*255.0+0.5)as u32;
                data.push(byte(sum[3])<<24|byte(sum[0])<<16|byte(sum[1])<<8|byte(sum[2]));
            }}
            if varied && matches!(semantic,PixelSemantic::MaskedColor) {
                let level=&mut data[offset..];
                let lut=coverage_lut(&alpha_hist(level),&hist0);
                for t in level.iter_mut(){*t=(*t&0x00ff_ffff)|(lut[(*t>>24)as usize]as u32)<<24;}
            }
            start=offset;w=nw;h=nh;max_level+=1;
        }
        Self{width,height,data,max_level,hash}
    }
    /// Upload through a content-keyed cache: an identical image already on
    /// the GPU (another model, another terrain chunk) is shared, not copied.
    pub fn upload_cached(self,cx:&mut Cx,cache:&mut std::collections::HashMap<u64,Texture>)->Texture {
        if let Some(texture)=cache.get(&self.hash){return texture.clone();}
        let hash=self.hash;let texture=self.upload(cx);cache.insert(hash,texture.clone());texture
    }
    pub fn upload(self,cx:&mut Cx)->Texture {
        Texture::new_with_format(cx,TextureFormat::VecMipBGRAu8_32{width:self.width,height:self.height,data:Some(self.data),max_level:Some(self.max_level),wrap:TextureWrap::Repeat,updated:TextureUpdated::Full})
    }
    pub fn bytes(&self)->usize{self.data.len()*4}
}

/// Byte → byte map sending `level`'s alpha distribution onto `reference`'s:
/// a value at cumulative share q of the level maps to the reference value at
/// the same share (the middle of the value's own rank range).
fn coverage_lut(level:&[u32;256],reference:&[u32;256])->[u8;256]{
    let total=|h:&[u32;256]|h.iter().map(|c|*c as u64).sum::<u64>().max(1);
    let (lt,rt)=(total(level),total(reference));
    let mut lut=[0u8;256];let mut below=0u64;let mut r=0usize;let mut r_below=0u64;
    for v in 0..256 {
        let mid=(below*2+level[v]as u64) as f64/(2.0*lt as f64);
        while r<255 && ((r_below+reference[r]as u64) as f64/rt as f64)<mid {r_below+=reference[r]as u64;r+=1;}
        lut[v]=r as u8;below+=level[v]as u64;
    }
    lut
}

#[derive(Clone)]
pub struct PreparedSurface {pub definition:MaterialSurface,pub normal:PreparedTexture,pub occlusion:PreparedTexture,pub emissive:PreparedTexture}
#[derive(Clone)]
pub struct UploadedSurface {pub definition:std::sync::Arc<MaterialSurface>,pub normal:Texture,pub occlusion:Texture,pub emissive:Texture}
impl PreparedSurface {
    pub fn prepare(mut definition:MaterialSurface,remaining:&mut usize)->Result<Self,String>{
        let mut image=|bytes:Option<Vec<u8>>,fallback,semantic|->Result<PreparedTexture,String>{
            let image=if let Some(bytes)=bytes{crate::renderer::decode_generated_png(&bytes,4096,*remaining)?}else{
                let mut image=ImageBuffer::default();image.width=1;image.height=1;image.data=vec![fallback];image};
            let texture=PreparedTexture::prepare(image,semantic);
            *remaining=remaining.checked_sub(texture.bytes()).ok_or("material maps exceed texture byte budget")?;Ok(texture)
        };
        let normal=image(definition.normal_png.take(),0xff80_80ff,PixelSemantic::Normal)?;
        let occlusion=image(definition.occlusion_png.take(),0xffff_ffff,PixelSemantic::Data)?;
        let emissive=image(definition.emissive_png.take(),0xffff_ffff,PixelSemantic::Color)?;
        Ok(Self{definition,normal,occlusion,emissive})
    }
    pub fn bytes(&self)->usize{self.normal.bytes()+self.occlusion.bytes()+self.emissive.bytes()}
    pub fn upload(self,cx:&mut Cx)->UploadedSurface {UploadedSurface{definition:std::sync::Arc::new(self.definition),normal:self.normal.upload(cx),occlusion:self.occlusion.upload(cx),emissive:self.emissive.upload(cx)}}
}

#[cfg(test)]
mod tests{
    use super::*;
    /// A height-in-alpha texture tested at 0.7 × weight keeps its coverage
    /// down the mip chain instead of averaging to 0.5 and dropping out.
    #[test]fn alpha_tested_coverage_survives_the_mip_chain(){
        let n=64;let mut image=ImageBuffer::default();image.width=n;image.height=n;
        let mut x=0x1234_5678u32;
        image.data=(0..n*n).map(|_|{x^=x<<13;x^=x>>17;x^=x<<5;((x>>24)<<24)|0x0080_8080}).collect();
        let cover=|texels:&[u32],cut:f32|texels.iter().filter(|t|((**t>>24)as f32/255.0)*0.7>cut).count()as f32/texels.len()as f32;
        let level0=cover(&image.data,0.5);
        let texture=PreparedTexture::prepare(image,PixelSemantic::MaskedColor);
        // Level 3 is 8x8, starting after 64²+32²+16².
        let start=n*n+32*32+16*16;let level3=cover(&texture.data[start..start+64],0.5);
        assert!(level0>0.2&&level0<0.35,"{level0}");
        assert!((level3-level0).abs()<0.1,"coverage {level0} at level 0 but {level3} at level 3");
    }
    #[test]fn color_mips_filter_linear_premultiplied_pixels(){
        let mut image=ImageBuffer::default();image.width=2;image.height=1;image.data=vec![0xffff_ffff,0xff00_0000];
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);let r=(texture.data[2]>>16)&255;assert!((187..=189).contains(&r));
        let mut image=ImageBuffer::default();image.width=2;image.height=1;image.data=vec![0xffff_0000,0x0000_00ff];
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);assert_eq!(texture.data[2]&0x00ff_ffff,0x00ff_0000);assert_eq!(texture.data[2]>>24,128);
    }
}

#[cfg(test)]
mod prepared_texture_bytes_tests {
    use super::*;
    #[test]
    fn serialized_mip_chain_round_trips_and_refuses_other_formats() {
        let mut image = ImageBuffer::default(); image.width = 5; image.height = 3;
        image.data = (0..15u32).map(|i| 0xff00_0000 | i * 0x0102_03).collect();
        let texture = PreparedTexture::prepare(image, PixelSemantic::MaskedColor);
        let bytes = texture.to_bytes();
        let back = PreparedTexture::from_bytes(&bytes).expect("round trip");
        assert_eq!((back.width, back.height, back.max_level, back.hash), (texture.width, texture.height, texture.max_level, texture.hash));
        assert_eq!(back.data, texture.data);
        assert!(PreparedTexture::from_bytes(&bytes[..bytes.len() - 4]).is_none(), "truncated");
        let mut other = bytes.clone(); other[4] = other[4].wrapping_add(1);
        assert!(PreparedTexture::from_bytes(&other).is_none(), "another format version");
        assert_ne!(prepared_texture_key(b"png", PixelSemantic::Color), prepared_texture_key(b"png", PixelSemantic::MaskedColor));
    }
}
