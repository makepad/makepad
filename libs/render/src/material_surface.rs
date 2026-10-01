//! Portable material pixels prepared on workers, including explicit mip data.
use makepad_draw::{ImageBuffer, makepad_platform::{CompressedTextureFormat,Cx,Texture,TextureFormat,TextureUpdated,TextureWrap}};
pub use makepad_draw::makepad_platform::CompressedTextureFormat as GpuCompressedFormat;

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
    /// Negative: the same scale on an up-facing part, drawn with the top
    /// projection only (the side weights are negligible there).
    pub triplanar: f32,
    /// glTF extras `makepadShading`: foliage wind, clear coat, flake (0..1).
    pub wind: f32,
    pub clearcoat: f32,
    pub flake: f32,
    /// A soft Fresnel rim of sky and back-lit sun (toy plastic, felt).
    pub rim: f32,
    /// This layer is the model's far stand-in from this distance (metres,
    /// 0 = an ordinary layer): the other layers stop there (impostor.rs).
    pub impostor: f32,
    /// A number plate (`stencil_plate` layout): its digit cells show each
    /// copy's own number (`color_adjust.w` = -(1 + number)).
    pub plate: bool,
}
impl MaterialSurface {
    /// The shading terms packed for a spare float lane (`tex_mag.y` on the
    /// PBR lane, `fur_layer.y` on the skinned one): rim * 255 * 65536 +
    /// clearcoat * 255 * 256 + flake * 255, exact in an f32 (24 bits).
    /// 0 = plain.
    pub fn packed_shading(&self) -> f32 {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round();
        q(self.rim) * 65536.0 + q(self.clearcoat) * 256.0 + q(self.flake)
    }
}
impl Default for MaterialSurface {
    fn default()->Self {Self{wind:0.0,clearcoat:0.0,flake:0.0,rim:0.0,impostor:0.0,plate:false,fur:None,normal_png:None,normal_scale:1.0,occlusion_png:None,occlusion_strength:1.0,
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
    pub hash:u64,
    /// The mip chain in the device's block-compressed format (transcoded from
    /// the stored Basis/UASTC form), when the host enabled a compression
    /// target. `data` is then empty: only the blocks go to the GPU.
    pub compressed:Option<(CompressedTextureFormat,Vec<u8>)>}
/// The lowest alpha (0..255) any texel of one BC7 block can decode to: the
/// smaller alpha endpoint (texels interpolate between the two). Modes 0-3
/// carry no alpha; in modes 4 and 5 `rotation` swaps a colour channel into
/// alpha. A reserved mode byte reads as fully transparent.
fn bc7_min_alpha(b:u128)->u32{
    let bits=|off:u32,n:u32|((b>>off)&((1u128<<n)-1)) as u32;
    let (e5,e6,e7)=(|x:u32|(x<<3)|(x>>2),|x:u32|(x<<2)|(x>>4),|x:u32|(x<<1)|(x>>6));
    match (b as u8).trailing_zeros() {
        0..=3=>255,
        4=>{let (ch,e):(u32,&dyn Fn(u32)->u32)=match bits(5,2){0=>(38,&e6),r=>(8+(r-1)*10,&e5)};let w=if ch==38{6}else{5};e(bits(ch,w)).min(e(bits(ch+w,w)))}
        5=>{match bits(6,2){0=>bits(50,8).min(bits(58,8)),r=>{let ch=8+(r-1)*14;e7(bits(ch,7)).min(e7(bits(ch+7,7)))}}}
        6=>((bits(49,7)<<1)|bits(63,1)).min((bits(56,7)<<1)|bits(64,1)),
        7=>(0..4).map(|i|e6((bits(74+i*5,5)<<1)|bits(94+i,1))).min().unwrap(),
        _=>0,
    }
}
fn linear(v:f32)->f32{if v<=0.04045{v/12.92}else{((v+0.055)/1.055).powf(2.4)}}
fn srgb(v:f32)->f32{if v<=0.0031308{v*12.92}else{1.055*v.max(0.0).powf(1.0/2.4)-0.055}}
/// A mip texel's sRGB byte: `(srgb(v).clamp(0, 1) * 255 + 0.5) as u32`.
fn srgb_byte_exact(v:f32)->u32{(srgb(v).clamp(0.0,1.0)*255.0+0.5)as u32}
/// The same byte without a powf: the smallest f32 at which each byte value
/// starts (the byte only grows with v), found once by bisection over the
/// float bit patterns with [`srgb_byte_exact`] itself, so every v gets the
/// byte srgb_byte_exact gives it (checked over every float in 0..1 by the
/// ignored test `srgb_byte_table_matches_every_float`).
fn srgb_byte(v:f32)->u32{
    static STARTS:std::sync::OnceLock<[f32;255]>=std::sync::OnceLock::new();
    let starts=STARTS.get_or_init(||std::array::from_fn(|i|{
        let want=i as u32+1;
        let (mut lo,mut hi)=(0u32,1.0f32.to_bits());
        while lo<hi { let mid=lo+(hi-lo)/2; if srgb_byte_exact(f32::from_bits(mid))>=want {hi=mid} else {lo=mid+1} }
        f32::from_bits(lo)
    }));
    starts.partition_point(|start|*start<=v)as u32
}
/// A host's store of Basis/UASTC KTX2 forms of source images (keyed by
/// [`basis_texture_key`]), for hosts that set a compression target: encoding
/// UASTC is far slower than any lookup. Mipped RGBA chains are not stored:
/// decoding and mipping a source image costs about what fetching its stored
/// chain did. Both methods run on preparation workers; `put_basis` must not
/// block on I/O.
pub trait PreparedTextureStore: Send + Sync {
    fn get_basis(&self, key: u64) -> Option<Vec<u8>>;
    fn put_basis(&self, key: u64, ktx2: &[u8]);
}

static TEXTURE_STORE: std::sync::OnceLock<Box<dyn PreparedTextureStore>> = std::sync::OnceLock::new();
/// Install the process's prepared-texture cache (first call wins).
pub fn set_prepared_texture_store(store: Box<dyn PreparedTextureStore>) { let _ = TEXTURE_STORE.set(store); }
pub(crate) fn prepared_texture_store() -> Option<&'static dyn PreparedTextureStore> { TEXTURE_STORE.get().map(|s| s.as_ref()) }
/// Part of every source image key ([`prepared_texture_key`]): bumped when
/// [`PreparedTexture::prepare`] changes the chains it makes, so stored forms
/// derived from older chains are never used.
pub const PREPARED_TEXTURE_FORMAT: u32 = 1;
/// Key of a source image: its bytes, its semantic (colour and masked colour
/// mip differently) and the format version.
pub fn prepared_texture_key(source: &[u8], semantic: PixelSemantic) -> u64 {
    let h = source.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3));
    h ^ (source.len() as u64).rotate_left(29) ^ ((semantic as u64) << 56) ^ ((PREPARED_TEXTURE_FORMAT as u64) << 48).rotate_left(7)
}
/// In-memory dedupe key of an image's bytes (with its length): four
/// independent 64-bit lanes, so it runs at memory speed. A load hashes the
/// same material images once per model that names them (a level's terrain
/// tiles: hundreds of megabytes), where a byte-at-a-time hash was most of
/// the texture work. Never persisted: stored keys use
/// [`prepared_texture_key`].
pub fn image_content_hash(bytes: &[u8]) -> u64 {
    const K: [u64; 4] = [0x9e37_79b9_7f4a_7c15, 0xc2b2_ae3d_27d4_eb4f, 0x1656_67b1_9e37_79f9, 0xd6e8_feb8_6659_fd93];
    let mut h = [K[0] ^ bytes.len() as u64, K[1], K[2], K[3]];
    let mut chunks = bytes.chunks_exact(32);
    for c in &mut chunks {
        for (i, h) in h.iter_mut().enumerate() {
            let v = u64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().unwrap());
            *h = (*h ^ v).wrapping_mul(K[i]).rotate_left(29);
        }
    }
    let mut tail = [0u8; 32];
    tail[..chunks.remainder().len()].copy_from_slice(chunks.remainder());
    for (i, h) in h.iter_mut().enumerate() {
        let v = u64::from_le_bytes(tail[i * 8..i * 8 + 8].try_into().unwrap());
        *h = (*h ^ v).wrapping_mul(K[i]).rotate_left(29);
    }
    let mut out = h[0];
    for (i, v) in h.iter().enumerate().skip(1) { out = (out ^ v.rotate_left(17 * i as u32)).wrapping_mul(K[0]); }
    out ^ (out >> 32)
}
/// Images at least this large go to the GPU block-compressed when the host
/// set a compression target (smaller ones save little).
pub const COMPRESS_MIN_TEXELS: usize = 128 * 128;
/// Bumped when the UASTC encoder or its settings change output, so stored
/// Basis textures from an older encoder are not reused.
pub const BASIS_ENCODER_VERSION: u32 = 1; // Fast and Default outputs are both valid UASTC: no bump needed

/// (textures, of which block-compressed, GPU bytes, compressed GPU bytes,
/// RGBA bytes of textures at least COMPRESS_MIN_TEXELS large) uploaded since launch.
static UPLOAD_STATS: [std::sync::atomic::AtomicUsize; 5] = [const { std::sync::atomic::AtomicUsize::new(0) }; 5];
/// Worker-microseconds spent transcoding Basis textures since launch.
pub static TRANSCODE_MICROS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Per-load texture work since launch, (count, worker microseconds) per
/// stage: the "work removed" measurement of the load-ready form.
#[derive(Clone, Copy, Debug)]
pub enum TextureWork {
    /// PNG decode plus mip generation (the legacy path).
    DecodeMip,
    /// Load-ready KTX2: UASTC remapped to the device format (or decoded
    /// to RGBA when there is no compression target).
    Remap,
    /// Hashing source bytes for store and dedupe keys.
    Hash,
}
pub static TEXTURE_WORK: [[std::sync::atomic::AtomicUsize; 2]; 3] = [const { [const { std::sync::atomic::AtomicUsize::new(0) }; 2] }; 3];
/// Run `f`, charging its time to `stage`.
pub fn texture_work<R>(stage: TextureWork, f: impl FnOnce() -> R) -> R {
    let started = work_clock();
    let result = f();
    let slot = &TEXTURE_WORK[stage as usize];
    slot[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    slot[1].fetch_add(work_micros(started), std::sync::atomic::Ordering::Relaxed);
    result
}
/// The work counters' clock: `Instant` natively; none in the browser,
/// where std has no clock (the counters count work there, not time).
#[cfg(not(target_arch = "wasm32"))]
fn work_clock() -> Option<std::time::Instant> {
    Some(std::time::Instant::now())
}
#[cfg(target_arch = "wasm32")]
fn work_clock() -> Option<std::time::Instant> {
    None
}
fn work_micros(started: Option<std::time::Instant>) -> usize {
    started.map_or(0, |started| started.elapsed().as_micros() as usize)
}
/// Images already prepared this load, by content: every terrain tile of a
/// level uses the same material images (flight: 552 uses in 64 tiles, 32
/// distinct), and each tile prepared (decoded and mipped, or remapped) all
/// of them again. Bounded; the host releases it when its loads are done
/// ([`release_remapped_ktx2`]).
static REMAPPED: std::sync::Mutex<Vec<((u64, usize, u8), std::sync::Arc<PreparedTexture>)>> = std::sync::Mutex::new(Vec::new());
const REMAPPED_BYTES_KEPT: usize = 96 * 1024 * 1024;

/// The texture for content `key` (hash, byte length, kind), prepared by
/// `prepare` only if this load has not prepared it yet.
pub fn prepared_once(key: (u64, usize, u8), prepare: impl FnOnce() -> Result<PreparedTexture, String>) -> Result<std::sync::Arc<PreparedTexture>, String> {
    if let Some(texture) = REMAPPED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return Ok(texture.1.clone());
    }
    let texture = std::sync::Arc::new(prepare()?);
    let mut kept = REMAPPED.lock().unwrap_or_else(|e| e.into_inner());
    if !kept.iter().any(|(k, _)| *k == key) { kept.push((key, texture.clone())); }
    while kept.len() > 1 && kept.iter().map(|(_, t)| t.bytes()).sum::<usize>() > REMAPPED_BYTES_KEPT { kept.remove(0); }
    Ok(texture)
}

/// `bytes` (a load-ready KTX2) as this device's texture, remapped once per
/// distinct image while a load is in progress. `None` when not KTX2.
pub fn remapped_ktx2(bytes: &[u8]) -> Option<Result<std::sync::Arc<PreparedTexture>, String>> {
    if !crate::texture_pack::is_ktx2(bytes) { return None; }
    let remap = || texture_work(TextureWork::Remap, || crate::texture_pack::texture_from_ktx2(bytes)).expect("KTX2 bytes");
    Some(match crate::texture_pack::content_id(bytes) {
        Some(id) => prepared_once((id, bytes.len(), u8::MAX), remap),
        None => remap().map(std::sync::Arc::new),
    })
}

/// Drop the images [`remapped_ktx2`] kept (the host's loads are done; the
/// GPU copies are shared by content in the renderer's texture cache).
pub fn release_remapped_ktx2() {
    let mut kept = REMAPPED.lock().unwrap_or_else(|e| e.into_inner());
    if !kept.is_empty() { kept.clear(); }
}

/// `[(count, ms)]` per [`TextureWork`] stage.
pub fn texture_work_stats() -> [(usize, usize); 3] {
    std::array::from_fn(|i| (TEXTURE_WORK[i][0].load(std::sync::atomic::Ordering::Relaxed), TEXTURE_WORK[i][1].load(std::sync::atomic::Ordering::Relaxed) / 1000))
}
pub fn prepared_texture_upload_stats() -> [usize; 5] {
    use std::sync::atomic::Ordering::Relaxed;
    UPLOAD_STATS.each_ref().map(|a| a.load(Relaxed))
}

/// A texture from an encoded source image in the form this device samples:
/// with a compression target, a stored Basis form keyed by the source bytes
/// is used without decoding at all; otherwise `make` decodes and mips it
/// (and a large result is encoded to Basis in the background for next time).
pub fn device_texture_from_source(source: &[u8], semantic: PixelSemantic, make: impl FnOnce() -> Result<PreparedTexture, String>) -> Result<PreparedTexture, String> {
    let (Some(target), Some(store)) = (compressed_texture_target(), prepared_texture_store()) else { return make() };
    // Small images are not compressed: no store lookup for them.
    if source.len() < PREPARED_TEXTURE_CACHE_MIN_SOURCE_BYTES { return make(); }
    let key = basis_texture_key(source, semantic);
    if let Some(texture) = PreparedTexture::from_store(store, key, target, key) { return Ok(texture); }
    let texture = make()?;
    if texture.width * texture.height >= COMPRESS_MIN_TEXELS { encode_basis_later(key, &texture); }
    Ok(texture)
}

/// One background thread encodes new images to Basis and stores them, so a
/// first load never waits on the encoder. The queue is bounded: when full, an
/// image is simply encoded on a later load instead.
pub(crate) fn encode_basis_later(key: u64, texture: &PreparedTexture) {
    use std::sync::mpsc::{sync_channel, SyncSender};
    static QUEUE: std::sync::OnceLock<SyncSender<(u64, PreparedTexture)>> = std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (sender, receiver) = sync_channel::<(u64, PreparedTexture)>(32);
        let _ = std::thread::Builder::new().name("basis-encode".into()).spawn(move || {
            while let Ok((key, texture)) = receiver.recv() {
                if let (Some(store), Some(ktx)) = (prepared_texture_store(), texture.to_basis(false)) { store.put_basis(key, &ktx); }
            }
        });
        sender
    });
    let _ = queue.try_send((key, texture.clone()));
}

static COMPRESSED_TARGET: std::sync::OnceLock<Option<CompressedTextureFormat>> = std::sync::OnceLock::new();
/// Opt in to GPU block-compressed textures: `target` is the format this
/// device samples (from `GpuInfo`). First call wins; hosts that never call
/// this upload RGBA as before.
pub fn set_compressed_texture_target(target: Option<CompressedTextureFormat>) { let _ = COMPRESSED_TARGET.set(target); }
/// Pick the target from GPU capabilities: ASTC where sampled (lossless from
/// UASTC), else BC7.
pub fn compressed_target_for(bc7: bool, astc4x4: bool) -> Option<CompressedTextureFormat> {
    if astc4x4 { Some(CompressedTextureFormat::Astc4x4) } else if bc7 { Some(CompressedTextureFormat::Bc7) } else { None }
}
pub(crate) fn compressed_texture_target() -> Option<CompressedTextureFormat> { COMPRESSED_TARGET.get().copied().flatten() }
static BC5_NORMAL_MAPS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Opt in to uploading load-ready normal maps (stored as BC5: X and Y, Z
/// rebuilt in the shader) as they are: set it when the device samples BC
/// (`GpuInfo::texture_bc7`). Without it they are decoded to RGBA.
pub fn set_bc5_normal_maps(enabled: bool) { BC5_NORMAL_MAPS.store(enabled, std::sync::atomic::Ordering::Relaxed); }
pub(crate) fn bc5_normal_maps() -> bool { BC5_NORMAL_MAPS.load(std::sync::atomic::Ordering::Relaxed) }
/// Store key of an image's Basis/UASTC form.
pub fn basis_texture_key(source: &[u8], semantic: PixelSemantic) -> u64 {
    prepared_texture_key(source, semantic) ^ (0xBA515u64 << 40) ^ ((BASIS_ENCODER_VERSION as u64) << 20)
}
/// Source images (encoded bytes) smaller than this are not looked up.
pub const PREPARED_TEXTURE_CACHE_MIN_SOURCE_BYTES: usize = 32 * 1024;

impl PreparedTexture {
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
        // sRGB bytes to linear once per byte value, not per texel read: the
        // same linear() of the same 256 inputs, without four powf per texel.
        let to_linear:[f32;256]=std::array::from_fn(|b|linear(b as f32/255.0));
        let(mut w,mut h,mut start,mut max_level)=(width,height,0usize,0usize);
        while w>1||h>1 {
            let(nw,nh)=((w/2).max(1),(h/2).max(1));let offset=data.len();
            for y in 0..nh {for x in 0..nw {
                let mut sum=[0.0f32;4];
                for(dy,dx)in[(0,0),(0,1),(1,0),(1,1)] {
                    let pixel=data[start+(y*2+dy).min(h-1)*w+(x*2+dx).min(w-1)];
                    let bytes=[(pixel>>16)&255,(pixel>>8)&255,pixel&255];
                    let mut c=[bytes[0]as f32/255.0,bytes[1]as f32/255.0,bytes[2]as f32/255.0,(pixel>>24)as f32/255.0];
                    match semantic {PixelSemantic::Color|PixelSemantic::MaskedColor=>{for i in 0..3{c[i]=to_linear[bytes[i]as usize]*c[3];}},PixelSemantic::Normal=>{for i in 0..3{c[i]=c[i]*2.0-1.0;}},PixelSemantic::Data=>{}}
                    for i in 0..4{sum[i]+=c[i]*0.25;}
                }
                let byte=|v:f32|(v.clamp(0.0,1.0)*255.0+0.5)as u32;
                let rgb=match semantic {
                    PixelSemantic::Color|PixelSemantic::MaskedColor=>{let v=|i:usize|if sum[3]>1e-8{sum[i]/sum[3]}else{0.0};[srgb_byte(v(0)),srgb_byte(v(1)),srgb_byte(v(2))]},
                    PixelSemantic::Normal=>{let length=(sum[0]*sum[0]+sum[1]*sum[1]+sum[2]*sum[2]).sqrt();let n=|i:usize|if length>1e-8{sum[i]/length*0.5+0.5}else{if i==2{1.0}else{0.5}};[byte(n(0)),byte(n(1)),byte(n(2))]},
                    PixelSemantic::Data=>[byte(sum[0]),byte(sum[1]),byte(sum[2])],
                };
                data.push(byte(sum[3])<<24|rgb[0]<<16|rgb[1]<<8|rgb[2]);
            }}
            if varied && matches!(semantic,PixelSemantic::MaskedColor) {
                let level=&mut data[offset..];
                let lut=coverage_lut(&alpha_hist(level),&hist0);
                for t in level.iter_mut(){*t=(*t&0x00ff_ffff)|(lut[(*t>>24)as usize]as u32)<<24;}
            }
            start=offset;w=nw;h=nh;max_level+=1;
        }
        Self{width,height,data,max_level,hash,compressed:None}
    }
    /// Upload through a content-keyed cache: an identical image already on
    /// the GPU (another model, another terrain chunk) is shared, not copied.
    pub fn upload_cached(self,cx:&mut Cx,cache:&mut std::collections::HashMap<u64,Texture>)->Texture {
        if let Some(texture)=cache.get(&self.hash){return texture.clone();}
        let hash=self.hash;let texture=self.upload(cx);cache.insert(hash,texture.clone());texture
    }
    pub fn upload(self,cx:&mut Cx)->Texture {
        use std::sync::atomic::Ordering::Relaxed;
        UPLOAD_STATS[0].fetch_add(1,Relaxed);
        UPLOAD_STATS[1].fetch_add(self.compressed.is_some() as usize,Relaxed);
        UPLOAD_STATS[2].fetch_add(self.bytes(),Relaxed);
        if self.compressed.is_some() { UPLOAD_STATS[3].fetch_add(self.bytes(),Relaxed); }
        else if self.width*self.height>=COMPRESS_MIN_TEXELS {
            UPLOAD_STATS[4].fetch_add(self.bytes(),Relaxed);
        }
        if let Some((format,blocks))=self.compressed {
            return Texture::new_with_format(cx,TextureFormat::VecMipCompressed{width:self.width,height:self.height,format,data:Some(blocks),max_level:Some(self.max_level),wrap:TextureWrap::Repeat,updated:TextureUpdated::Full});
        }
        Texture::new_with_format(cx,TextureFormat::VecMipBGRAu8_32{width:self.width,height:self.height,data:Some(self.data),max_level:Some(self.max_level),wrap:TextureWrap::Repeat,updated:TextureUpdated::Full})
    }
    /// Whether a texel of this image would fail the model shaders' alpha
    /// test (alpha under one half). An image that cannot draws its layer
    /// through the no-discard pipeline (renderer/opaque.rs). From the level-0
    /// texels, or for BC7 from each level-0 block's alpha endpoints (every
    /// texel of a block lies between them); unknown formats count as
    /// cutout. Memoised by content hash: atlases are shared across layers.
    pub fn cutout(&self)->bool{
        static MEMO:std::sync::Mutex<Option<std::collections::HashMap<u64,bool>>>=std::sync::Mutex::new(None);
        if let Some(known)=MEMO.lock().ok().and_then(|m|m.as_ref().and_then(|m|m.get(&self.hash).copied())) {return known;}
        let cutout=match &self.compressed {
            None=>self.data[..(self.width*self.height).min(self.data.len())].iter().any(|t|(t>>24)<128),
            Some((CompressedTextureFormat::Bc7,blocks))=>{
                let n=self.width.div_ceil(4)*self.height.div_ceil(4);
                blocks.len()<n*16||blocks[..n*16].chunks_exact(16).any(|b|bc7_min_alpha(u128::from_le_bytes(b.try_into().unwrap()))<128)
            }
            Some(_)=>true,
        };
        if let Ok(mut memo)=MEMO.lock() {memo.get_or_insert_with(Default::default).insert(self.hash,cutout);}
        cutout
    }
    /// Bytes this texture takes on the GPU.
    pub fn bytes(&self)->usize{match &self.compressed {Some((_,blocks))=>blocks.len(),None=>self.data.len()*4}}
    /// Replace the RGBA chain with `target` blocks transcoded from `uastc`
    /// (a Basis/UASTC KTX2 of this image).
    pub fn from_basis(ktx2:&[u8],target:CompressedTextureFormat,hash:u64)->Option<Self>{
        struct Timed(Option<std::time::Instant>);
        impl Drop for Timed { fn drop(&mut self){ TRANSCODE_MICROS.fetch_add(work_micros(self.0),std::sync::atomic::Ordering::Relaxed); } }
        let _timed=Timed(work_clock());
        Self::from_uastc_view(&makepad_texcomp::basis::UastcView::parse(ktx2).ok()?,target,hash)
    }
    /// UASTC levels (borrowed from their KTX2) remapped block by block
    /// straight into the one upload buffer.
    pub fn from_uastc_view(view:&makepad_texcomp::basis::UastcView,target:CompressedTextureFormat,hash:u64)->Option<Self>{
        let &(width,height,_)=view.levels.first()?;
        let mut blocks=Vec::with_capacity(view.levels.iter().map(|l|l.2.len()).sum());
        for &(_,_,level) in &view.levels {
            match target {
                CompressedTextureFormat::Bc7=>makepad_texcomp::transcode::uastc_to_bc7_into(level,&mut blocks),
                CompressedTextureFormat::Astc4x4=>makepad_texcomp::transcode::uastc_to_astc_into(level,&mut blocks),
                // Normal maps are stored as BC5; UASTC never becomes BC5.
                CompressedTextureFormat::Bc5=>return None,
            }
        }
        Some(Self{width:width as usize,height:height as usize,data:Vec::new(),max_level:view.levels.len()-1,hash,compressed:Some((target,blocks))})
    }
    /// The device form of a stored Basis texture, transcoded.
    pub fn from_store(store:&dyn PreparedTextureStore,basis_key:u64,target:CompressedTextureFormat,hash:u64)->Option<Self>{
        store.get_basis(basis_key).and_then(|ktx|Self::from_basis(&ktx,target,hash))
    }
    /// Encode this RGBA chain to Basis/UASTC (KTX2 bytes, one level per mip).
    pub fn to_basis(&self,srgb:bool)->Option<Vec<u8>>{
        // Background work: Fast (about 1 dB under Default, 4x quicker) on a
        // quarter of the cores, so a 4k sky is ready within seconds.
        let threads=std::thread::available_parallelism().map_or(1,|n|(n.get()/4).max(1));
        self.to_uastc(srgb,makepad_texcomp::uastc::Quality::Fast,threads).ok()?.to_ktx2().ok()
    }
    /// This chain's red and green (a normal map's X and Y) as a BC5 KTX2 with
    /// `content_id`, encoded on `threads` cores (builders).
    pub fn to_bc5_ktx2(&self,content_id:u64,threads:usize)->Result<Vec<u8>,String>{
        let (mut w,mut h,mut offset)=(self.width,self.height,0usize);
        let mut levels=Vec::new();
        for _ in 0..=self.max_level {
            let texels=self.data.get(offset..offset+w*h).ok_or("mip chain shorter than its levels")?;
            let rgba:Vec<u8>=texels.iter().flat_map(|t|[(t>>16) as u8,(t>>8) as u8,0,255]).collect();
            levels.push((w as u32,h as u32,makepad_texcomp::rgtc::encode_bc5_image(&rgba,w as u32,h as u32,threads.max(1))));
            offset+=w*h;w=(w/2).max(1);h=(h/2).max(1);
        }
        let refs:Vec<(u32,u32,&[u8])>=levels.iter().map(|(w,h,d)|(*w,*h,d.as_slice())).collect();
        makepad_texcomp::rgtc::bc5_to_ktx2(&refs,Some(content_id))
    }
    /// This RGBA chain as UASTC at a chosen quality on `threads` cores (builders).
    pub fn to_uastc(&self,srgb:bool,quality:makepad_texcomp::uastc::Quality,threads:usize)->Result<makepad_texcomp::basis::UastcTexture,String>{
        let (mut w,mut h,mut offset)=(self.width,self.height,0usize);
        let mut levels=Vec::new();
        for _ in 0..=self.max_level {
            let n=w*h;
            let texels=self.data.get(offset..offset+n).ok_or("mip chain shorter than its levels")?;
            // BGRA words (0xAARRGGBB) to RGBA bytes.
            let rgba:Vec<u8>=texels.iter().flat_map(|t|[(t>>16) as u8,(t>>8) as u8,*t as u8,(t>>24) as u8]).collect();
            levels.push((w as u32,h as u32,rgba));
            offset+=n;w=(w/2).max(1);h=(h/2).max(1);
        }
        let refs:Vec<(u32,u32,&[u8])>=levels.iter().map(|(w,h,d)|(*w,*h,d.as_slice())).collect();
        makepad_texcomp::basis::encode_mips(&refs,quality,srgb,threads.max(1))
    }
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
            if let Some(texture)=bytes.as_deref().and_then(remapped_ktx2) {
                let texture=std::sync::Arc::unwrap_or_clone(texture?);
                *remaining=remaining.checked_sub(texture.bytes()).ok_or("material maps exceed texture byte budget")?;return Ok(texture);
            }
            let texture=if let Some(bytes)=bytes{
                // Once per load for every model that uses the image (a
                // level's tiles share their material maps).
                let hash=texture_work(TextureWork::Hash,||image_content_hash(&bytes));
                let budget=*remaining;
                std::sync::Arc::unwrap_or_clone(prepared_once((hash,bytes.len(),semantic as u8),||{
                    let image=texture_work(TextureWork::DecodeMip,||crate::renderer::decode_generated_png(&bytes,4096,budget))?;
                    Ok(texture_work(TextureWork::DecodeMip,||PreparedTexture::prepare(image,semantic)))
                })?)
            }else{
                let mut image=ImageBuffer::default();image.width=1;image.height=1;image.data=vec![fallback];PreparedTexture::prepare(image,semantic)};

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
    #[test]
    fn image_content_hash_tells_bytes_and_lengths_apart() {
        let a: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        assert_eq!(image_content_hash(&a), image_content_hash(&a.clone()));
        // Every length around the 32-byte blocks, and a change in the tail,
        // the last block and the first byte, each give another key.
        let mut seen = std::collections::HashSet::new();
        for n in 0..100 { assert!(seen.insert(image_content_hash(&a[..n])), "length {n}"); }
        for at in [0, 500, 990, 999] {
            let mut b = a.clone();
            b[at] ^= 1;
            assert_ne!(image_content_hash(&a), image_content_hash(&b), "byte {at}");
        }
        assert_ne!(image_content_hash(&[0u8; 64]), image_content_hash(&[0u8; 65]));
    }
    #[test]
    fn bc7_block_alpha_floor() {
        // Mode 1 (no alpha), and a reserved all-zero mode byte.
        assert_eq!(bc7_min_alpha(0b10),255);
        assert_eq!(bc7_min_alpha(0),0);
        // Mode 6: 7-bit alpha endpoints plus a p-bit each.
        let m6=|a0:u128,a1:u128,p:u128|1u128<<6|a0<<49|a1<<56|p<<63|p<<64;
        assert_eq!(bc7_min_alpha(m6(127,127,1)),255);
        assert_eq!(bc7_min_alpha(m6(127,63,0)),126);
        // Mode 5, rotation 0: 8-bit alpha endpoints.
        assert_eq!(bc7_min_alpha(1u128<<5|200u128<<50|255u128<<58),200);
        // Mode 5, rotation 1: red (7-bit, expanded) becomes alpha.
        assert_eq!(bc7_min_alpha(1u128<<5|1u128<<6|127u128<<8|127u128<<15|0u128<<50),255);
        // Level-0 texels of an RGBA chain.
        let t=|a:u32|PreparedTexture{width:2,height:1,data:vec![0x00ff_ffff|a<<24,0xffff_ffff],max_level:0,hash:0xa11a_0000|a as u64,compressed:None};
        assert!(!t(255).cutout());
        assert!(!t(128).cutout());
        assert!(t(127).cutout());
    }
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
    #[test]
    #[ignore]
    fn srgb_byte_table_matches_every_float(){
        // Every float from 0 to 1 (about a billion): release, some seconds.
        let mut bits=0u32;
        while bits<=1.0f32.to_bits() { let v=f32::from_bits(bits); assert_eq!(srgb_byte(v),srgb_byte_exact(v),"{v}"); bits+=1; }
    }
    #[test]fn srgb_byte_table_matches_at_every_step(){
        for v in [-1.0f32,-0.0,0.0,f32::NAN,1.0,2.0,f32::INFINITY] { assert_eq!(srgb_byte(v),srgb_byte_exact(v),"{v}"); }
        // Around every byte boundary, and on a fine grid.
        for i in 0..=100_000u32 { let v=i as f32/100_000.0; assert_eq!(srgb_byte(v),srgb_byte_exact(v),"{v}"); }
        let mut v=1.0e-6f32;
        while v<1.0 { for d in [-2i32,-1,0,1,2] { let w=f32::from_bits((v.to_bits() as i32+d)as u32); assert_eq!(srgb_byte(w),srgb_byte_exact(w),"{w}"); } v*=1.0003; }
    }
    #[test]fn color_mips_match_linear_per_texel(){
        // The byte table must give the very bytes linear() per texel gave.
        let (w,h)=(16usize,8usize);
        let mut seed=0x1234_5678u32;
        let data:Vec<u32>=(0..w*h).map(|_|{seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;seed}).collect();
        let mut image=ImageBuffer::default();image.width=w;image.height=h;image.data=data.clone();
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);
        let mut expect=data;
        let (mut w,mut h,mut start)=(w,h,0);
        while w>1||h>1 {
            let (nw,nh)=((w/2).max(1),(h/2).max(1));let offset=expect.len();
            for y in 0..nh {for x in 0..nw {
                let mut sum=[0.0f32;4];
                for (dy,dx) in [(0,0),(0,1),(1,0),(1,1)] {
                    let p=expect[start+(y*2+dy).min(h-1)*w+(x*2+dx).min(w-1)];
                    let mut c=[((p>>16)&255)as f32/255.0,((p>>8)&255)as f32/255.0,(p&255)as f32/255.0,(p>>24)as f32/255.0];
                    for i in 0..3{c[i]=linear(c[i])*c[3];}
                    for i in 0..4{sum[i]+=c[i]*0.25;}
                }
                for i in 0..3{sum[i]=srgb(if sum[3]>1e-8{sum[i]/sum[3]}else{0.0});}
                let byte=|v:f32|(v.clamp(0.0,1.0)*255.0+0.5)as u32;
                expect.push(byte(sum[3])<<24|byte(sum[0])<<16|byte(sum[1])<<8|byte(sum[2]));
            }}
            start=offset;w=nw;h=nh;
        }
        assert_eq!(texture.data,expect);
    }
    #[test]fn color_mips_filter_linear_premultiplied_pixels(){
        let mut image=ImageBuffer::default();image.width=2;image.height=1;image.data=vec![0xffff_ffff,0xff00_0000];
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);let r=(texture.data[2]>>16)&255;assert!((187..=189).contains(&r));
        let mut image=ImageBuffer::default();image.width=2;image.height=1;image.data=vec![0xffff_0000,0x0000_00ff];
        let texture=PreparedTexture::prepare(image,PixelSemantic::Color);assert_eq!(texture.data[2]&0x00ff_ffff,0x00ff_0000);assert_eq!(texture.data[2]>>24,128);
    }
}

#[cfg(test)]
mod prepared_texture_key_tests {
    use super::*;
    #[test]
    fn source_keys_tell_semantics_apart() {
        assert_ne!(prepared_texture_key(b"png", PixelSemantic::Color), prepared_texture_key(b"png", PixelSemantic::MaskedColor));
    }
}
