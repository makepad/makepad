//! Portable material pixels prepared on workers, including explicit mip data.
use makepad_draw::{ImageBuffer, makepad_platform::{CompressedTextureFormat,Cx,HostBuf,Texture,TextureFormat,TextureUpdated,TextureWrap}};
use makepad_draw::makepad_platform::thread::{self, TaskPool};
pub use makepad_draw::makepad_platform::CompressedTextureFormat as GpuCompressedFormat;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageSize(pub usize, pub usize);

impl ImageSize {
    pub(crate) fn read(bytes: &[u8], dimension: usize) -> Result<Self, String> {
        let (w, h) = if crate::texture_pack::is_ktx2(bytes) {
            let reader = makepad_texcomp::ktx2::Reader::new(bytes)?;
            (reader.header().width as usize, reader.header().height.max(1) as usize)
        } else {
            png_size(bytes).ok_or("generated PNG has no dimensions")?
        };
        if w == 0 || h == 0 || w > dimension || h > dimension {
            return Err("generated PNG exceeds dimension limit".into());
        }
        Ok(Self(w, h))
    }

    pub(crate) fn at(self, level: usize) -> Self {
        Self((self.0 >> level).max(1), (self.1 >> level).max(1))
    }

    fn bytes(mut self, mips: bool) -> usize {
        let mut total = 0usize;
        loop {
            total = total.saturating_add(self.0.saturating_mul(self.1).saturating_mul(4));
            if !mips || self == Self(1, 1) { return total; }
            self = self.at(1);
        }
    }

    pub(crate) fn fit(self, mut level: usize, limit: usize, mips: bool) -> usize {
        while self.at(level) != Self(1, 1) && self.at(level).bytes(mips) > limit {
            level += 1;
        }
        level
    }
}

/// Charges for one worker preparation. Array layers share the first image's dimensions,
/// including layers filled with a constant. Only actual image sources count in the report.
#[derive(Default)]
pub(crate) struct ImageBudget {
    pub(crate) fixed: usize,
    images: Vec<(ImageSize, bool, usize)>,
    count: usize,
}

impl ImageBudget {
    fn add(&mut self, size: ImageSize, mips: bool, layers: usize, sources: usize) {
        self.images.push((size, mips, layers));
        self.count += sources;
    }

    /// Each primitive charges its material's payload even when several primitives use one material.
    pub(crate) fn repeated(&mut self, material: Self, uses: usize) {
        self.fixed = self.fixed.saturating_add(material.fixed.saturating_mul(uses));
        self.images.extend(material.images.into_iter().map(|(size, mips, layers)| (size, mips, layers.saturating_mul(uses))));
        self.count += material.count;
    }

    pub(crate) fn pixels(&mut self, bytes: &[u8], dimension: usize) -> Result<(), String> {
        let size = ImageSize::read(bytes, dimension)?;
        if crate::texture_pack::is_ktx2(bytes) {
            // Sky and atlas consumers expand just level zero of a load-ready texture.
            self.fixed = self.fixed.saturating_add(size.bytes(false));
        } else {
            self.add(size, false, 1, 1);
        }
        Ok(())
    }

    pub(crate) fn level(&self, budget: usize) -> usize {
        let cost = |k| self.images.iter().fold(self.fixed, |sum, &(size, mips, copies)| {
            sum.saturating_add(size.at(k).bytes(mips).saturating_mul(copies))
        });
        let total = cost(0);
        let mut level = 0;
        if total <= budget { return 0; }
        while self.images.iter().any(|(s, _, _)| s.at(level) != ImageSize(1, 1)) {
            level += 1;
            if cost(level) <= budget { break; }
        }
        if level > 0 {
            let n = self.count;
            let key = makepad_draw::LiveId::from_str("model_texture_reduction")
                .bytes_append(&n.to_le_bytes()).bytes_append(&total.to_le_bytes()).bytes_append(&level.to_le_bytes());
            if makepad_draw::first_report(key.0) {
                let mib = 1024.0 * 1024.0;
                let factor = 1usize << level;
                makepad_draw::log!("model textures {:.2} MiB over the prepare budget {:.2} MiB: {n} images reduced to 1/{factor} per side", total as f64 / mib, budget as f64 / mib);
            }
        }
        level
    }
}

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
    /// `makepadShading.crown`: the layer's normals are crown normals, so the
    /// foliage lane does not flip them toward the viewer.
    pub crown: bool,
    /// This layer is the model's far stand-in from this distance (metres,
    /// 0 = an ordinary layer): the other layers stop there (impostor.rs).
    pub impostor: f32,
    /// A number plate: its digit cells show each
    /// copy's own number (`color_adjust.w` = -(1 + number)).
    pub plate: bool,
    pub msdf: Option<MsdfText>,
    /// A vehicle signal lamp (`makepadShading.lamp`): the emission above is
    /// the lamp fully lit, and each copy scales it by its own vehicle's
    /// brake and reverse state ([`lamp_gain`]).
    pub lamp: Option<makepad_gltf::GlbLamp>,
    /// A generated terrain tile's paint (glTF extras `makepadSplat`): the
    /// layers it blends per pixel, drawn by the splat lane (shaders/splat.rs).
    pub splat: Option<std::sync::Arc<SplatSurface>>,
}

/// Linear colours and texel distances for an MSDF base layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MsdfText {
    pub fill: [f32; 4],
    pub border: [f32; 4],
    pub border_width: f32,
    pub range: f32,
    pub plate_cells: Option<[f32; 4]>,
}

/// The most layers a terrain tile blends (gen level mesh.rs SPLAT_LAYERS).
pub const SPLAT_LAYERS: usize = 8;

/// A terrain tile's paint: its layers in paint order (a later one covers an
/// earlier one) and the weight map, texel (x, z) at world xz `origin +
/// (x, z) * cell`, layers 4p..4p+3 in the RGBA of page p (pages stacked
/// along z, `rows` each).
#[derive(Clone, Debug, PartialEq)]
pub struct SplatSurface {
    pub layers: Vec<SplatLayer>,
    /// The weight map, or a worn road part's band map ([`SplatBand`]).
    pub weights_png: Vec<u8>,
    pub origin: [f32; 2],
    pub cell: f32,
    pub width: f32,
    pub rows: f32,
    pub band: Option<SplatBand>,
}

/// A worn road part's paint (gen level road.rs, glb.rs `band`): the
/// renderer's band masks (shaders/splat.rs), drawn from its vertices'
/// band coordinates (the AO lane, model.rs `MODEL_VERTEX_FLOATS`) and its
/// band map, which rides the weight-map slot (`weights_png`): a single
/// mesh surface reads no weights.
#[derive(Clone, Debug, PartialEq)]
pub struct SplatBand {
    /// Arc of the part's first band coordinate, and the spans its two
    /// coordinates decode over (arc past `base`; lateral centred on the
    /// centre line), metres.
    pub base: f32,
    pub span: f32,
    pub lateral: f32,
    /// The map: metres of arc per texel, texels per row (each row starts
    /// on the previous row's last texel), rows.
    pub cell: f32,
    pub row: f32,
    pub rows: f32,
    /// The carriageway's half width; a closed road's arc (0: open).
    pub half: f32,
    pub length: f32,
    /// Amounts 0..1: the worn line, sealant seams, repairs, dirty edges.
    pub wear: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplatLayer {
    /// Albedo with the height in alpha; None paints `color` (sRGB).
    pub albedo_png: Option<Vec<u8>>,
    pub color: [f32; 3],
    pub normal_png: Option<Vec<u8>>,
    /// UV scale, 1/metres.
    pub scale: f32,
    pub roughness: f32,
    pub metallic: f32,
    pub bump: f32,
    /// Parallax depth in metres; zero skips every relief read.
    pub relief: f32,
    pub wet: f32,
    pub puddles: f32,
    /// World macro variation over the tiled texture: [brightness, warm/cool
    /// hue, roughness] amplitudes (0..1) and the size in metres of its middle
    /// octave; size 0 = none (shaders/splat.rs).
    pub macro_variation: [f32; 4],
    /// Hex-tiled reads (makepadSplat `tiling: "stochastic"`, shaders/splat.rs).
    pub stochastic: bool,
}

/// How lit a signal lamp of `role` is on a copy whose vehicle shows
/// `signals`, 0..1 of its authored emission. `lamp_level` is the frame's
/// headlight level (0 in daylight, 1 at night or in fog): tail lamps glow
/// dimly with the lights on, as the running lamps of a real car do.
pub fn lamp_gain(role: makepad_gltf::GlbLamp, signals: makepad_scene::light::LampSignals, lamp_level: f32) -> f32 {
    /// A tail lamp's running glow against its braking glow.
    const TAIL_RUNNING: f32 = 0.3;
    let on = |b: bool| if b { 1.0f32 } else { 0.0 };
    match role {
        makepad_gltf::GlbLamp::Tail => on(signals.brake).max(TAIL_RUNNING * lamp_level),
        makepad_gltf::GlbLamp::Brake => on(signals.brake),
        makepad_gltf::GlbLamp::Reverse => on(signals.reverse),
    }
}
impl MaterialSurface {
    /// Leave a worn road part's band off (its vertices' AO lane carries
    /// something else: model.rs `PrimSpan::band`): the paint draws plain.
    pub fn drop_band(&mut self) {
        if let Some(splat) = self.splat.as_ref().filter(|s| s.band.is_some()) {
            let mut plain = (**splat).clone();
            plain.band = None;
            plain.weights_png = Vec::new();
            self.splat = Some(std::sync::Arc::new(plain));
        }
    }
    pub fn base_semantic(&self) -> PixelSemantic {
        if self.msdf.is_some() { PixelSemantic::Data }
        else if self.alpha_mode == 1 { PixelSemantic::MaskedColor }
        else { PixelSemantic::Color }
    }
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
    fn default()->Self {Self{wind:0.0,clearcoat:0.0,flake:0.0,rim:0.0,crown:false,impostor:0.0,plate:false,msdf:None,lamp:None,fur:None,normal_png:None,normal_scale:1.0,occlusion_png:None,occlusion_strength:1.0,
        emissive_png:None,emissive:[0.0;3],alpha_mode:0,alpha_cutoff:0.5,base_alpha:1.0,double_sided:false,triplanar:0.0,splat:None}}
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
    MaskedColor,
    /// Data holding one 16-bit value per texel, red its high byte and green
    /// its low byte, and an 8-bit value of its own in blue. Linear filtering
    /// of both bytes is linear in the 16-bit value, so the GPU's reads blend
    /// it exactly; each mip averages the 16-bit values and splits the mean
    /// again, where averaging each byte would round both (an error of half a
    /// high-byte step), and averages blue as a byte. A far stand-in's depth
    /// and occlusion atlas (impostor.rs) is one.
    Data16 }

#[derive(Clone)]
pub struct PreparedTexture {pub width:usize,pub height:usize,
    /// Shared mip chain: a clone shares it with the load memo (`prepared_once`), products, and upload.
    pub data:HostBuf<u32>,pub max_level:usize,
    /// Content hash (size + level-0 texels), computed on the worker: the
    /// renderer's texture cache shares one GPU texture per distinct image
    /// across every model and chunk that uses it.
    pub hash:u64,
    /// Key of the full chain's stored form in the prepared-texture store, present there or
    /// owed by an encode queued in this process, so it can be restored after a reduced upload.
    pub stored:Option<u64>,
    /// The mip chain in the device's block-compressed format (transcoded from
    /// the stored Basis/UASTC form), when the host enabled a compression
    /// target. `data` is then empty: only the blocks go to the GPU.
    /// Shared mip chain: a clone shares it with the load memo (`prepared_once`), products, and upload.
    pub compressed:Option<(CompressedTextureFormat,HostBuf<u8>)>}
/// A binding's content-cached GPU texture, including every uploaded mip and array layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureCharge {
    pub key: u64,
    pub bytes: usize,
    pub width: usize,
    pub height: usize,
    pub layers: usize,
    pub mips: usize,
    pub compressed: Option<CompressedTextureFormat>,
    pub slot: TextureSlot,
    /// Stored-form keys for restoring the full chain: one for a 2D texture, one per array
    /// layer only when every layer has one, or empty when no complete stored form is known.
    pub stored: Vec<u64>,
    /// A shader addresses this image in texels (nearest magnification, MSDF text or an
    /// impostor atlas), so dropping mips changes its appearance beyond losing sharpness.
    pub texel_addressed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureSlot {
    Base, Detail, Orm, Normal, Occlusion, Emissive,
    SplatAlbedo, SplatNormal, SplatWeights,
}

impl TextureCharge {
    /// The deepest first mip an upload can use without a partial block at its top level.
    pub fn deepest_first_level(&self) -> usize {
        deepest_first_level(self.width, self.height, self.mips.saturating_sub(1), self.compressed.is_some())
    }
    pub fn rgba_compressible(&self) -> bool {
        self.compressed.is_none() && self.width * self.height >= COMPRESS_MIN_TEXELS
    }
    /// Whether the renderer drops this texture's CPU chain after upload (it remakes it from the
    /// stored form when the GPU device is replaced): see [`crate::texture_restore`].
    pub fn drops_cpu_chain(&self) -> bool {
        crate::texture_restore::drops_cpu_chain(self.compressed.is_some(), &self.stored)
    }
    /// GPU bytes per mip, finest first, over every layer: what dropping a texture's top mips frees.
    /// A mip halves each side down to 1 texel; RGBA takes 4 bytes a texel, every block format
    /// 16 bytes per 4x4 block. Sums to `bytes`.
    pub fn mip_bytes(&self) -> Vec<u64> {
        (0..self.mips.max(1)).map(|level| {
            let side = |n: usize| n.checked_shr(level as u32).unwrap_or(0).max(1);
            let (w, h) = (side(self.width), side(self.height));
            let bytes = if self.compressed.is_some() { w.div_ceil(4) * h.div_ceil(4) * 16 } else { w * h * 4 };
            (bytes * self.layers.max(1)) as u64
        }).collect()
    }
}

fn deepest_first_level(width: usize, height: usize, max_level: usize, compressed: bool) -> usize {
    if !compressed { return max_level; }
    // Every backend requires a block texture's top level to contain whole 4x4 blocks.
    (0..=max_level).rev().find(|&level| {
        (width >> level).max(1) % 4 == 0 && (height >> level).max(1) % 4 == 0
    }).unwrap_or(0)
}

/// Arrays share only when their layer content and order agree with the upload.
pub fn splat_array_key(layers: &[std::sync::Arc<PreparedTexture>]) -> u64 {
    layers.iter().fold(0x5b1a_7a77_u64 ^ layers.len() as u64, |h, t| {
        (h ^ t.hash).wrapping_mul(0x100_0000_01b3).rotate_left(29)
    })
}

fn splat_array_format(layers: &[std::sync::Arc<PreparedTexture>]) -> Option<CompressedTextureFormat> {
    let format = layers[0].compressed.as_ref()?.0;
    layers.iter().all(|t| t.compressed.as_ref().is_some_and(|(f, _)| *f == format)).then_some(format)
}

/// `layers` gathered layer-major into one chain keyed by [`splat_array_key`] (each layer's chain
/// as it is, levels 0..=max_level, then the next layer's), once per distinct array while loads
/// run ([`prepared_once`]): every tile with the same layers shares it. Block-compressed when every
/// layer is in one format ([`splat_array_format`]), else RGBA.
fn gather_layers(layers: &[std::sync::Arc<PreparedTexture>]) -> std::sync::Arc<PreparedTexture> {
    prepared_once((splat_array_key(layers), layers.len(), u8::MAX - 2), || Ok(gathered(layers))).expect("gathering layers cannot fail")
}

/// [`gather_layers`] without the memo.
fn gathered(layers: &[std::sync::Arc<PreparedTexture>]) -> PreparedTexture { gathered_from(layers, 0) }

/// `layers` gathered layer-major from mip `k` on (each layer's levels `k..=max_level`, then the next
/// layer's): the array an upload from level `k` takes. Copies only the kept levels.
fn gathered_from(layers: &[std::sync::Arc<PreparedTexture>], k: usize) -> PreparedTexture {
    fn concat<'a, T: Copy + 'a>(parts: impl Iterator<Item = &'a [T]> + Clone) -> Vec<T> {
        let mut out = Vec::with_capacity(parts.clone().map(<[T]>::len).sum());
        for part in parts { out.extend_from_slice(part); }
        out
    }
    let first = &layers[0];
    let format = splat_array_format(layers);
    PreparedTexture {
        width: (first.width >> k).max(1), height: (first.height >> k).max(1), max_level: first.max_level - k,
        hash: splat_array_key(layers), stored: None,
        data: match format { Some(_) => Vec::new().into(), None => concat(layers.iter().map(|t| &t.data[t.tail(k)])).into() },
        compressed: format.map(|format| (format, concat(layers.iter().map(|t| t.compressed.as_ref().map_or(&[][..], |(_, b)| &b[t.tail(k)]))).into())),
    }
}

/// An array's stored-form keys: one per layer when every layer has one, else none.
fn layers_stored(layers: &[std::sync::Arc<PreparedTexture>]) -> Vec<u64> {
    layers.iter().map(|t| t.stored).collect::<Option<Vec<_>>>().unwrap_or_default()
}

pub(crate) fn splat_array_charge(layers: &[std::sync::Arc<PreparedTexture>], slot: TextureSlot) -> TextureCharge {
    let first = &layers[0];
    TextureCharge {
        key: splat_array_key(layers),
        bytes: layers.iter().map(|t| t.bytes()).sum(),
        width: first.width,
        height: first.height,
        layers: layers.len(),
        mips: first.max_level + 1,
        compressed: splat_array_format(layers),
        slot,
        stored: layers_stored(layers),
        texel_addressed: false,
    }
}

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
pub(crate) fn linear(v:f32)->f32{if v<=0.04045{v/12.92}else{((v+0.055)/1.055).powf(2.4)}}
fn srgb(v:f32)->f32{if v<=0.0031308{v*12.92}else{1.055*v.max(0.0).powf(1.0/2.4)-0.055}}
/// A mip texel's sRGB byte: `(srgb(v).clamp(0, 1) * 255 + 0.5) as u32`.
fn srgb_byte_exact(v:f32)->u32{(srgb(v).clamp(0.0,1.0)*255.0+0.5)as u32}
/// The same byte without a powf: the smallest f32 at which each byte value
/// starts (the byte only grows with v), found once by bisection over the
/// float bit patterns with [`srgb_byte_exact`] itself, so every v gets the
/// byte srgb_byte_exact gives it.
pub(crate) fn srgb_byte(v:f32)->u32{
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
/// [`PreparedTexture::remake`] through the process's installed store; `None` without one.
pub fn remake_stored(stored: &[u64], from_level: usize, hash: u64) -> Option<Result<PreparedTexture, String>> {
    PreparedTexture::remake(prepared_texture_store()?, stored, from_level, hash)
}
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
/// In-place retunes and GPU bytes written since launch, separate from uploads.
pub static RETUNE_STATS: [std::sync::atomic::AtomicUsize; 2] = [const { std::sync::atomic::AtomicUsize::new(0) }; 2];
/// [`RETUNE_STATS`] as `[retunes, GPU bytes written]`.
pub fn retune_stats() -> [usize; 2] {
    RETUNE_STATS.each_ref().map(|a| a.load(std::sync::atomic::Ordering::Relaxed))
}
/// Worker-microseconds spent transcoding Basis textures since launch.
pub static TRANSCODE_MICROS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// UI-thread work of `Renderer::upload_static_preview` since launch: calls, microseconds in
/// total, the largest call's microseconds, and texture bytes an upload copied (chains are
/// shared and splat arrays gathered on the preparing worker; reducing a chain copies its
/// retained mips). Uploads run in the frame that installs a model, so these are what installs add to frames.
pub static UPLOAD_UI: [std::sync::atomic::AtomicUsize; 4] = [const { std::sync::atomic::AtomicUsize::new(0) }; 4];
/// [`UPLOAD_UI`] as `[calls, micros, largest call micros, bytes copied]`.
pub fn upload_ui_stats() -> [usize; 4] {
    UPLOAD_UI.each_ref().map(|a| a.load(std::sync::atomic::Ordering::Relaxed))
}
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
/// Run `f`, charging its time to `stage`. A pool job gives the pool a turn first when one is due:
/// a load prepares its images one after another in one job (a level's materials, hundreds of
/// decodes, remaps and hashes), which at a cold start on a 2-core share ran 0.3-0.9 s without a
/// checkpoint. Each stage builds new data and holds no shared lock.
pub fn texture_work<R>(stage: TextureWork, f: impl FnOnce() -> R) -> R {
    let _ = thread::checkpoint_when_due();
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
pub(crate) fn work_clock() -> Option<std::time::Instant> {
    Some(std::time::Instant::now())
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn work_clock() -> Option<std::time::Instant> {
    None
}
pub(crate) fn work_micros(started: Option<std::time::Instant>) -> usize {
    started.map_or(0, |started| started.elapsed().as_micros() as usize)
}
/// Source identity and the number of box-halvings actually applied (including a budget backstop).
type LoadMemoKey = ((u64, usize, u8), usize);

/// Images already prepared this load, by content: every terrain tile of a
/// level uses the same material images (flight: 552 uses in 64 tiles, 32
/// distinct), and each tile prepared (decoded and mipped, or remapped) all
/// of them again. Bounded; it goes when the last preparation in progress
/// ends ([`Preparing`]). Its chains are shared with what was prepared, so
/// that frees only images nothing uses any more.
static REMAPPED: std::sync::Mutex<Vec<(LoadMemoKey, std::sync::Arc<PreparedTexture>)>> = std::sync::Mutex::new(Vec::new());
const REMAPPED_BYTES_KEPT: usize = 96 * 1024 * 1024;

fn memoized_texture(key: LoadMemoKey) -> Option<std::sync::Arc<PreparedTexture>> {
    REMAPPED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, t)| t.clone())
}

/// The texture for content `key` (hash, byte length, kind), prepared by
/// `prepare` only if this load has not prepared it yet.
pub fn prepared_once(key: (u64, usize, u8), prepare: impl FnOnce() -> Result<PreparedTexture, String>) -> Result<std::sync::Arc<PreparedTexture>, String> {
    prepared_at((key, 0), prepare)
}

fn prepared_at(key: LoadMemoKey, prepare: impl FnOnce() -> Result<PreparedTexture, String>) -> Result<std::sync::Arc<PreparedTexture>, String> {
    if let Some(texture) = memoized_texture(key) { return Ok(texture); }
    let claim = match conversion_turn(key) {
        Turn::Done(texture) => return Ok(texture),
        Turn::Convert(claim) => claim,
    };
    let converting = Converting::begin(claim);
    let texture = std::sync::Arc::new(prepare()?);
    let mut kept = REMAPPED.lock().unwrap_or_else(|e| e.into_inner());
    if !kept.iter().any(|(k, _)| *k == key) { kept.push((key, texture.clone())); }
    while kept.len() > 1 && kept.iter().map(|(_, t)| t.bytes()).sum::<usize>() > REMAPPED_BYTES_KEPT { kept.remove(0); }
    drop(kept);
    converting.done(texture.clone());
    Ok(texture)
}

/// The images preparations are converting now, so that one which needs an image another is
/// converting takes that one's result instead of converting it again: a level's terrain tiles
/// start together and share their material images, and each converted all of them (a 24 KB tile
/// took 400 ms). `done` holds the result for the waiters still to take it.
struct Conversion { key: LoadMemoKey, claim: u64, done: Option<std::sync::Arc<PreparedTexture>>, waiters: usize }
static CONVERSIONS: std::sync::Mutex<Vec<Conversion>> = std::sync::Mutex::new(Vec::new());
static CONVERTED: std::sync::Condvar = std::sync::Condvar::new();
static NEXT_CLAIM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
thread_local! {
    /// Conversions running on this thread: a thread converting one image does not wait for
    /// another, so no two converters wait on each other.
    static CONVERTING_HERE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

enum Turn { Done(std::sync::Arc<PreparedTexture>), Convert(Option<u64>) }

/// Whether this thread converts `key` (with the claim others wait on, or none) or takes the
/// result of the preparation converting it. Only a pool job waits, on its worker and without its
/// CPU permit ([`thread::without_permit`]); the UI thread, or a thread already converting an
/// image, converts it itself as before.
fn conversion_turn(key: LoadMemoKey) -> Turn {
    let may_wait = CONVERTING_HERE.with(|c| c.get()) == 0 && thread::current_class().is_some();
    loop {
        let claim = {
            let mut list = CONVERSIONS.lock().unwrap_or_else(|e| e.into_inner());
            let Some(at) = list.iter().position(|c| c.key == key) else {
                // A conversion that ended between the memo check and this lock left its image there.
                if let Some(texture) = memoized_texture(key) { return Turn::Done(texture); }
                let claim = NEXT_CLAIM.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                list.push(Conversion { key, claim, done: None, waiters: 0 });
                return Turn::Convert(Some(claim));
            };
            if let Some(texture) = list[at].done.clone() { return Turn::Done(texture); }
            if !may_wait { return Turn::Convert(None); }
            list[at].waiters += 1;
            list[at].claim
        };
        if let Some(texture) = thread::without_permit(|| converted(claim)) { return Turn::Done(texture); }
    }
}

/// The image conversion `claim` made, once it ends; none when it failed, and its waiters
/// convert the image themselves.
fn converted(claim: u64) -> Option<std::sync::Arc<PreparedTexture>> {
    let mut list = CONVERSIONS.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let at = list.iter().position(|c| c.claim == claim)?;
        if let Some(texture) = list[at].done.clone() {
            list[at].waiters -= 1;
            if list[at].waiters == 0 { list.remove(at); }
            return Some(texture);
        }
        list = CONVERTED.wait(list).unwrap_or_else(|e| e.into_inner());
    }
}

/// One conversion on this thread. Its claim's waiters get the result ([`Converting::done`]);
/// a conversion that fails or unwinds wakes them to convert the image themselves.
struct Converting(Option<u64>);
impl Converting {
    fn begin(claim: Option<u64>) -> Self {
        CONVERTING_HERE.with(|c| c.set(c.get() + 1));
        Self(claim)
    }
    fn done(mut self, texture: std::sync::Arc<PreparedTexture>) {
        if let Some(claim) = self.0.take() { finish_conversion(claim, Some(texture)); }
    }
}
impl Drop for Converting {
    fn drop(&mut self) {
        CONVERTING_HERE.with(|c| c.set(c.get() - 1));
        if let Some(claim) = self.0.take() { finish_conversion(claim, None); }
    }
}

fn finish_conversion(claim: u64, texture: Option<std::sync::Arc<PreparedTexture>>) {
    let mut list = CONVERSIONS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(at) = list.iter().position(|c| c.claim == claim) {
        match texture {
            Some(texture) if list[at].waiters > 0 => list[at].done = Some(texture),
            _ => { list.remove(at); }
        }
    }
    drop(list);
    CONVERTED.notify_all();
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

/// Preparations in progress in this process ([`Preparing`]).
static PREPARING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A model or surface preparation in progress. The load memo ([`prepared_once`]) serves the
/// preparations that overlap; when the last one ends the memo goes, in every app, without the
/// app having to say its loads are done. Every preparation holds one; a host that prepares a
/// batch one model after another (a level's tiles on a few workers) holds one over the batch,
/// so an image its models share is prepared once. Nested scopes count once each.
pub struct Preparing(());
impl Preparing {
    pub fn begin() -> Self {
        PREPARING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self(())
    }
}
impl Drop for Preparing {
    fn drop(&mut self) {
        // A preparation that begins while this one releases loses only the memo's dedupe.
        if PREPARING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 { release_remapped_ktx2(); }
    }
}

/// Drop the images [`remapped_ktx2`] kept (no preparation is in progress; the
/// GPU copies are shared by content in the renderer's texture cache).
fn release_remapped_ktx2() {
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
/// is used without decoding at all; otherwise `make` decodes and mips it, and
/// a large result comes with its [`StoreBasis`], the encode that lets the next
/// load skip the decode. The caller runs that once nothing waits for the pool
/// (a level's models and surfaces): a 4k image fans out over every worker for
/// seconds (26 s under the JIT), and a load that encoded first waited that long
/// for a form it does not use.
pub fn device_texture_from_source(source: &[u8], semantic: PixelSemantic, make: impl FnOnce() -> Result<PreparedTexture, String>) -> Result<(PreparedTexture, Option<StoreBasis>), String> {
    let (Some(target), Some(store)) = (compressed_texture_target(), prepared_texture_store()) else { return make().map(|texture| (texture, None)) };
    // Small images are not compressed: no store lookup for them.
    if source.len() < PREPARED_TEXTURE_CACHE_MIN_SOURCE_BYTES { return make().map(|texture| (texture, None)); }
    let key = basis_texture_key(source, semantic);
    if let Some(mut texture) = PreparedTexture::from_store(store, key, target, key) {
        texture.stored = Some(key);
        return Ok((texture, None));
    }
    let mut texture = make()?;
    let later = (texture.width * texture.height >= COMPRESS_MIN_TEXELS).then(|| StoreBasis::new(key, &texture, false));
    if later.is_some() { texture.stored = Some(key); }
    Ok((texture, later))
}

/// A decoded source image's stored form, to encode and store for later loads
/// ([`device_texture_from_source`], [`storing_device_textures`]): UASTC, or BC5
/// for a normal map that keeps only X and Y. It shares the mip chain
/// until it runs.
pub struct StoreBasis { key: u64, texture: PreparedTexture, bc5: bool, registration: u64 }

/// The chains every live [`StoreBasis`] holds, by registration: a clone of its texture (sharing
/// the chain), so [`load_held_texture_bytes`] sees an encode wherever it waits (the owed queue,
/// a host's own list, a pool job) until it has run or is dropped.
static ENCODE_CHAINS: std::sync::Mutex<Vec<(u64, PreparedTexture)>> = std::sync::Mutex::new(Vec::new());
static ENCODE_REGISTRATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Drop for StoreBasis {
    fn drop(&mut self) {
        let mut chains = ENCODE_CHAINS.lock().unwrap_or_else(|e| e.into_inner());
        let gone = chains.iter().position(|(registration, _)| *registration == self.registration).map(|i| chains.swap_remove(i));
        // A large chain is freed after the lock, not under it.
        drop(chains);
        drop(gone);
    }
}

/// Bytes and count of texture chains only the load machinery keeps alive: the load memo
/// ([`prepared_once`], an entry nothing else holds) and stored-form encodes waiting or running
/// ([`StoreBasis`]). A chain something else also holds (the uploaded texture in `Cx`, a
/// prepared model not yet installed) is its holder's to count. For an app's memory book at its
/// settle: two short lists, locked briefly one after the other.
pub fn load_held_texture_bytes() -> (u64, usize) {
    let (mut bytes, mut chains) = (0u64, 0usize);
    load_held_chains(|_, chain_bytes| { bytes += chain_bytes as u64; chains += 1; });
    (bytes, chains)
}

/// [`load_held_texture_bytes`] per chain: `visit(allocation, bytes)`.
fn load_held_chains(mut visit: impl FnMut(usize, usize)) {
    // allocation -> (load references, holders, bytes)
    let mut held: std::collections::HashMap<usize, (usize, usize, usize)> = std::collections::HashMap::new();
    let mut note = |texture: &PreparedTexture, references: usize| {
        let (allocation, holders, bytes) = texture.chain();
        held.entry(allocation).or_insert((0, holders, bytes)).0 += references;
    };
    // A memo entry another holder shares (a load in flight) is that load's.
    for (_, texture) in REMAPPED.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if std::sync::Arc::strong_count(texture) == 1 { note(texture, 1); }
    }
    // An encode holds its chain twice: its own texture and the registry's clone.
    for (_, texture) in ENCODE_CHAINS.lock().unwrap_or_else(|e| e.into_inner()).iter() { note(texture, 2); }
    for (allocation, (references, holders, bytes)) in held {
        if references == holders { visit(allocation, bytes); }
    }
}

/// Stored-form encodes since launch: (count, RGBA bytes encoded, worker microseconds).
pub static STORE_ENCODES: [std::sync::atomic::AtomicUsize; 3] = [const { std::sync::atomic::AtomicUsize::new(0) }; 3];

impl StoreBasis {
    /// The encode of `texture`'s stored form under `key`, registered so [`load_held_texture_bytes`]
    /// sees the chain it holds.
    fn new(key: u64, texture: &PreparedTexture, bc5: bool) -> Self {
        let registration = ENCODE_REGISTRATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ENCODE_CHAINS.lock().unwrap_or_else(|e| e.into_inner()).push((registration, texture.clone()));
        Self { key, texture: texture.clone(), bc5, registration }
    }
    /// Encode it through `encode` (a pool executor: it fans out) and store it.
    pub fn run(self, encode: &(dyn Fn(usize, &(dyn Fn(usize) + Sync)) + Sync)) {
        use std::sync::atomic::Ordering::Relaxed;
        let started = work_clock();
        let ktx = if self.bc5 { self.texture.to_bc5_ktx2(self.key, encode).ok() }
            else { self.texture.to_uastc(false, makepad_texcomp::uastc::Quality::Fast, encode).ok().and_then(|uastc| uastc.to_ktx2_with(Some(self.key)).ok()) };
        if let (Some(store), Some(ktx)) = (prepared_texture_store(), ktx) { store.put_basis(self.key, &ktx); }
        STORE_ENCODES[0].fetch_add(1, Relaxed);
        STORE_ENCODES[1].fetch_add(self.texture.bytes(), Relaxed);
        STORE_ENCODES[2].fetch_add(work_micros(started), Relaxed);
    }
}

thread_local! {
    /// Set on a preparation thread while [`storing_device_textures`] runs.
    static STORING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Encodes owed by [`storing_device_textures`] preparations, for the host to run on its pool,
/// and every key owed since launch (an image is encoded once per process, however many
/// preparations or passes name it).
static OWED: std::sync::Mutex<(Vec<StoreBasis>, Option<std::collections::HashSet<u64>>)> = std::sync::Mutex::new((Vec::new(), None));

/// Run a preparation whose images have no load-ready form of their own (a
/// product the host builds again on every launch): each large PNG image is
/// looked up in the prepared-texture store by its bytes, and a stored block
/// form is used as it is (no decode, no mips); a miss decodes as before and
/// owes its encode, which the host takes with [`take_owed_texture_encodes`]
/// and runs once nothing waits for its pool. Without a compression target or
/// a store this changes nothing.
pub fn storing_device_textures<R>(f: impl FnOnce() -> R) -> R {
    let before = STORING.with(|storing| storing.replace(true));
    let out = f();
    STORING.with(|storing| storing.set(before));
    out
}

thread_local! {
    static CONVERSION_POOL: std::cell::RefCell<Option<TaskPool>> = const { std::cell::RefCell::new(None) };
}

/// Run a preparation whose stored image lookups and remaps can share the host's pool.
/// Outside a pool job the preparing thread fetches the images in order. A nested
/// scope restores the previous pool even when the preparation unwinds.
pub fn spreading_conversions<R>(pool: &TaskPool, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<TaskPool>);
    impl Drop for Restore {
        fn drop(&mut self) { CONVERSION_POOL.with(|slot| slot.replace(self.0.take())); }
    }
    let _restore = Restore(CONVERSION_POOL.with(|slot| slot.replace(Some(pool.clone()))));
    f()
}

/// The encodes [`storing_device_textures`] preparations owe, oldest first.
pub fn take_owed_texture_encodes() -> Vec<StoreBasis> {
    std::mem::take(&mut OWED.lock().unwrap_or_else(|e| e.into_inner()).0)
}

/// Width and height from a PNG's header, without decoding it.
fn png_size(bytes: &[u8]) -> Option<(usize, usize)> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR") { return None; }
    let read = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize);
    Some((read(16)?, read(20)?))
}

/// The store key of `source`'s stored form: BC5 and UASTC forms of one image differ.
fn stored_form_key(source: &[u8], semantic: PixelSemantic, bc5: bool) -> u64 {
    basis_texture_key(source, semantic) ^ (bc5 as u64) << 63
}

/// Read only level descriptions when budgeting a device texture. A device without the
/// matching block format expands every supplied level to RGBA.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DeviceLayout {
    size: ImageSize,
    levels: usize,
    format: Option<GpuCompressedFormat>,
    bytes: usize,
}

impl DeviceLayout {
    fn read(bytes: &[u8]) -> Result<Self, String> {
        let (levels, format) = if let Ok(view) = makepad_texcomp::rgtc::Bc5View::parse(bytes) {
            (view.levels, bc5_normal_maps().then_some(GpuCompressedFormat::Bc5))
        } else {
            let target = compressed_texture_target();
            if target == Some(GpuCompressedFormat::Bc5) { return Err("UASTC cannot be remapped to BC5".into()); }
            (makepad_texcomp::basis::UastcView::parse(bytes)?.levels, target)
        };
        let &(w, h, _) = levels.first().ok_or("empty texture")?;
        let bytes = levels.iter().fold(0usize, |n, &(w, h, blocks)| {
            n.saturating_add(if format.is_some() { blocks.len() } else { ImageSize(w as usize, h as usize).bytes(false) })
        });
        Ok(Self { size: ImageSize(w as usize, h as usize), levels: levels.len(), format, bytes })
    }
}

enum StoredForm { Encoded(Vec<u8>), Prepared(std::sync::Arc<PreparedTexture>) }

struct StoredMap {
    memo: (u64, usize, u8),
    layout: DeviceLayout,
    form: StoredForm,
}

impl StoredMap {
    fn lookup(store: &dyn PreparedTextureStore, key: u64, source_len: usize, target: ImageSize) -> Option<Self> {
        let memo = (key, source_len, u8::MAX - 1);
        if let Some(t) = memoized_texture((memo, 0)) {
            let layout = DeviceLayout { size: ImageSize(t.width, t.height), levels: t.max_level + 1,
                format: t.compressed.as_ref().map(|(f, _)| *f), bytes: t.bytes() };
            if layout.size == target { return Some(Self { memo, layout, form: StoredForm::Prepared(t) }); }
        }
        let bytes = store.get_basis(key)?;
        match DeviceLayout::read(&bytes) {
            Ok(layout) if layout.size == target => Some(Self { memo, layout, form: StoredForm::Encoded(bytes) }),
            _ => { makepad_draw::log!("stored texture {key:016x} is unusable: its source is decoded"); None }
        }
    }

    fn prepare(&self) -> Result<std::sync::Arc<PreparedTexture>, String> {
        match &self.form {
            StoredForm::Prepared(texture) => Ok(texture.clone()),
            StoredForm::Encoded(bytes) => prepared_once(self.memo, || {
                let mut texture = texture_work(TextureWork::Remap, || crate::texture_pack::texture_from_ktx2(bytes))
                    .ok_or("stored texture is not KTX2")??;
                texture.hash = self.memo.0;
                texture.stored = Some(self.memo.0);
                Ok(texture)
            }),
        }
    }
}

struct PngMap {
    size: ImageSize,
    memo: (u64, usize, u8),
    store_key: Option<(&'static dyn PreparedTextureStore, u64)>,
}

impl PngMap {
    fn new(bytes: &[u8], size: ImageSize, hash: u64, semantic: PixelSemantic, store_size: Option<ImageSize>, bc5: bool) -> Self {
        let mut store_key = None;
        if let Some(target) = store_size.filter(|_| size.0 * size.1 >= COMPRESS_MIN_TEXELS && STORING.with(|s| s.get()) && compressed_texture_target().is_some()) {
            if let Some(store) = prepared_texture_store() {
                let mut key = stored_form_key(bytes, semantic, bc5);
                if target != size {
                    // A terrain array resampled to another layer's dimensions has its own stored form.
                    key = makepad_draw::LiveId(key).str_append("splat_size")
                        .bytes_append(&(target.0 as u32).to_le_bytes()).bytes_append(&(target.1 as u32).to_le_bytes()).0;
                }
                store_key = Some((store, key));
            }
        }
        Self { size, memo: (hash, bytes.len(), semantic as u8), store_key }
    }

    fn lookup(&self, target: ImageSize) -> Option<StoredMap> {
        let (store, key) = self.store_key?;
        StoredMap::lookup(store, key, self.memo.1, target)
    }

    fn decoded(&self, bytes: &[u8], semantic: PixelSemantic, level: usize) -> Result<std::sync::Arc<PreparedTexture>, String> {
        prepared_at((self.memo, level), || texture_work(TextureWork::DecodeMip, || {
            Ok(PreparedTexture::prepare(crate::renderer::decode_model_image(bytes, 4096, level)?, semantic))
        }))
    }
}

pub(crate) struct MapPlan {
    kind: MapSource,
    semantic: PixelSemantic,
}

/// A store hit keeps its fixed charge even if its remap reports an error at preparation.
struct FetchedMap {
    bytes: usize,
    texture: Result<std::sync::Arc<PreparedTexture>, String>,
}

enum MapSource { Solid(u32), Ktx2(usize), Png(PngMap, std::sync::OnceLock<Option<FetchedMap>>) }

impl MapPlan {
    pub(crate) fn new(source: Option<&[u8]>, fallback: u32, semantic: PixelSemantic, store: bool, budget: &mut ImageBudget) -> Result<Self, String> {
        Self::with_hash(source, Self::source_hash(source), fallback, semantic, store, budget)
    }

    fn source_hash(source: Option<&[u8]>) -> u64 {
        source.filter(|b| !crate::texture_pack::is_ktx2(b)).map_or(0, |b| texture_work(TextureWork::Hash, || image_content_hash(b)))
    }

    pub(crate) fn with_hash(source: Option<&[u8]>, hash: u64, fallback: u32, semantic: PixelSemantic, store: bool, budget: &mut ImageBudget) -> Result<Self, String> {
        let plan = Self::planned(source, hash, fallback, semantic, store)?;
        plan.fetch();
        plan.charge(budget);
        Ok(plan)
    }

    pub(crate) fn planned(source: Option<&[u8]>, hash: u64, fallback: u32, semantic: PixelSemantic, store: bool) -> Result<Self, String> {
        let kind = match source {
            None => MapSource::Solid(fallback),
            Some(b) if crate::texture_pack::is_ktx2(b) => MapSource::Ktx2(DeviceLayout::read(b)?.bytes),
            Some(b) => {
                let size = ImageSize::read(b, 4096)?;
                let png = PngMap::new(b, size, hash, semantic, store.then_some(size), matches!(semantic, PixelSemantic::Normal));
                MapSource::Png(png, Default::default())
            }
        };
        Ok(Self { kind, semantic })
    }

    fn fetch(&self) -> Option<&FetchedMap> {
        let MapSource::Png(png, stored) = &self.kind else { return None };
        stored.get_or_init(|| {
            let hit = png.lookup(png.size)?;
            Some(FetchedMap { bytes: hit.layout.bytes, texture: hit.prepare() })
        }).as_ref()
    }

    pub(crate) fn charge(&self, budget: &mut ImageBudget) {
        let fixed = match &self.kind {
            MapSource::Solid(_) => 4,
            MapSource::Ktx2(bytes) => *bytes,
            MapSource::Png(png, _) => match self.fetch() {
                Some(hit) => hit.bytes,
                None => return budget.add(png.size, true, 1, 1),
            },
        };
        budget.fixed = budget.fixed.saturating_add(fixed);
    }

    pub(crate) fn prepare(&self, source: Option<&[u8]>, level: usize, remaining: &mut usize) -> Result<std::sync::Arc<PreparedTexture>, String> {
        let texture = match &self.kind {
            MapSource::Solid(color) => {
                let mut image = ImageBuffer::default();
                image.width = 1; image.height = 1; image.data = vec![*color];
                std::sync::Arc::new(PreparedTexture::prepare(image, self.semantic))
            }
            MapSource::Ktx2(_) => remapped_ktx2(source.expect("planned KTX2")).expect("KTX2 bytes")?,
            MapSource::Png(png, _) => {
                if let Some(hit) = self.fetch() { hit.texture.clone()? } else {
                    let level = png.size.fit(level, *remaining, true);
                    let mut texture = png.decoded(source.expect("planned PNG"), self.semantic, level)?;
                    if let Some((_, key)) = png.store_key.filter(|_| level == 0) {
                        owe(key, &texture, matches!(self.semantic, PixelSemantic::Normal));
                        std::sync::Arc::make_mut(&mut texture).stored = Some(key);
                    }
                    texture
                }
            }
        };
        *remaining = remaining.saturating_sub(texture.bytes());
        Ok(texture)
    }
}

/// Fetch the stored forms before the preparation chooses its budget level.
pub(crate) fn fetch_ahead(plans: &[&MapPlan]) {
    if thread::current_class().is_some() {
        if let Some(pool) = CONVERSION_POOL.with(|slot| slot.borrow().clone()) {
            pool.fan_out(plans.len(), |i| { plans[i].fetch(); });
            return;
        }
    }
    for plan in plans { plan.fetch(); }
}

/// Queue the stored form of `texture` under `key`, once per process however many models name it.
fn owe(key: u64, texture: &PreparedTexture, bc5: bool) {
    let mut owed = OWED.lock().unwrap_or_else(|e| e.into_inner());
    if owed.1.get_or_insert_with(Default::default).insert(key) { owed.0.push(StoreBasis::new(key, texture, bc5)); }
}

/// Level 0's alpha distribution: from the RGBA chain, or decoded from its
/// BC7 or ASTC blocks (a stored layer has no RGBA).
fn level0_alpha_histogram(texture: &PreparedTexture) -> [u32; 256] {
    let mut histogram = [0u32; 256];
    let (w, h) = (texture.width, texture.height);
    match &texture.compressed {
        None => for pixel in &texture.data[..(w * h).min(texture.data.len())] { histogram[(pixel >> 24) as usize] += 1; },
        Some((format, blocks)) => {
            let (bx, by) = (w.div_ceil(4), h.div_ceil(4));
            for (i, block) in blocks.chunks_exact(16).take(bx * by).enumerate() {
                let block: &[u8; 16] = block.try_into().unwrap();
                let texels = match format {
                    CompressedTextureFormat::Bc7 => makepad_texcomp::bc7::decode_block(block),
                    CompressedTextureFormat::Astc4x4 => makepad_texcomp::astc::decode_block(block).unwrap_or([[0; 4]; 16]),
                    CompressedTextureFormat::Bc5 => [[0, 0, 0, 255]; 16],
                };
                let (x0, y0) = ((i % bx) * 4, (i / bx) * 4);
                for (t, texel) in texels.iter().enumerate() {
                    if x0 + t % 4 < w && y0 + t / 4 < h { histogram[texel[3] as usize] += 1; }
                }
            }
        }
    }
    histogram
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
#[cfg(test)]
pub(crate) static BC5_NORMAL_MAPS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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

/// Convert whole row bands through the encoder's executor. Each item owns its
/// bytes; collect them in row order even when the executor finishes in reverse.
fn rgba_rows(texels: &[u32], width: usize, run: &dyn Fn(usize, &(dyn Fn(usize) + Sync)),
    pixel: impl Fn(u32) -> [u8; 4] + Sync) -> Vec<u8> {
    if texels.is_empty() { return Vec::new(); }
    let band_len = 16384usize.div_ceil(width) * width;
    let bands: Vec<_> = texels.chunks(band_len).map(|src| (src, std::sync::OnceLock::new())).collect();
    run(bands.len(), &|index| {
        let (src, slot) = &bands[index];
        let bytes: Vec<u8> = src.iter().copied().flat_map(&pixel).collect();
        assert!(slot.set(bytes).is_ok(), "RGBA band run more than once");
    });
    let mut bands = bands.into_iter().map(|(_, slot)| slot.into_inner().expect("RGBA band was not run"));
    let mut rgba = bands.next().unwrap();
    rgba.reserve_exact(texels.len() * 4 - rgba.len());
    for band in bands { rgba.extend_from_slice(&band); }
    rgba
}

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
                let mut sum=[0.0f32;4];let mut sum16=0u32;
                for(dy,dx)in[(0,0),(0,1),(1,0),(1,1)] {
                    let pixel=data[start+(y*2+dy).min(h-1)*w+(x*2+dx).min(w-1)];
                    sum16+=(pixel>>8)&0xffff;
                    let bytes=[(pixel>>16)&255,(pixel>>8)&255,pixel&255];
                    let mut c=[bytes[0]as f32/255.0,bytes[1]as f32/255.0,bytes[2]as f32/255.0,(pixel>>24)as f32/255.0];
                    match semantic {PixelSemantic::Color|PixelSemantic::MaskedColor=>{for i in 0..3{c[i]=to_linear[bytes[i]as usize]*c[3];}},PixelSemantic::Normal=>{for i in 0..3{c[i]=c[i]*2.0-1.0;}},PixelSemantic::Data|PixelSemantic::Data16=>{}}
                    for i in 0..4{sum[i]+=c[i]*0.25;}
                }
                let byte=|v:f32|(v.clamp(0.0,1.0)*255.0+0.5)as u32;
                let rgb=match semantic {
                    PixelSemantic::Color|PixelSemantic::MaskedColor=>{let v=|i:usize|if sum[3]>1e-8{sum[i]/sum[3]}else{0.0};[srgb_byte(v(0)),srgb_byte(v(1)),srgb_byte(v(2))]},
                    PixelSemantic::Normal=>{let length=(sum[0]*sum[0]+sum[1]*sum[1]+sum[2]*sum[2]).sqrt();let n=|i:usize|if length>1e-8{sum[i]/length*0.5+0.5}else{if i==2{1.0}else{0.5}};[byte(n(0)),byte(n(1)),byte(n(2))]},
                    PixelSemantic::Data=>[byte(sum[0]),byte(sum[1]),byte(sum[2])],
                    PixelSemantic::Data16=>{let q=(sum16+2)/4;[q>>8,q&255,byte(sum[2])]},
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
        Self{width,height,data:data.into(),max_level,hash,stored:None,compressed:None}
    }
    /// The deepest first mip an upload can use without a partial block at its top level.
    pub fn deepest_first_level(&self) -> usize {
        deepest_first_level(self.width, self.height, self.max_level, self.compressed.is_some())
    }
    /// The buffer holding the chain (the blocks when compressed, else the texels):
    /// its allocation, how many handles hold it, and its bytes.
    fn chain(&self) -> (usize, usize, usize) {
        match &self.compressed {
            Some((_, blocks)) => (blocks.allocation(), blocks.holders(), blocks.len()),
            None => (self.data.allocation(), self.data.holders(), self.data.len() * 4),
        }
    }
    /// Levels `k..=max_level` of the chain: texels (RGBA) or bytes (blocks: every block format
    /// here is 16-byte 4x4 blocks), in the layout `prepare`, `from_uastc_view` and the platform's
    /// level walk use, finest first, each `(w >> l).max(1)` by `(h >> l).max(1)`.
    fn tail(&self, k: usize) -> std::ops::Range<usize> {
        let level_size = |level: usize| {
            let (w, h) = ((self.width >> level).max(1), (self.height >> level).max(1));
            if self.compressed.is_some() { w.div_ceil(4) * h.div_ceil(4) * 16 } else { w * h }
        };
        let start: usize = (0..k).map(level_size).sum();
        start..start + (k..=self.max_level).map(level_size).sum::<usize>()
    }
    /// The same image holding levels `k..=max_level` only, with its content and stored-form keys
    /// (the renderer's cache and the host name the texture by them). The kept levels are copied:
    /// a shared sub-range would keep the whole chain alive.
    pub fn from_level(&self, k: usize) -> Self {
        debug_assert!(k <= self.deepest_first_level());
        if k == 0 { return self.clone(); }
        let tail = self.tail(k);
        let (data, compressed) = match &self.compressed {
            Some((format, blocks)) => (Vec::new().into(), Some((*format, blocks[tail].to_vec().into()))),
            None => (self.data[tail].to_vec().into(), None),
        };
        Self {
            width: (self.width >> k).max(1), height: (self.height >> k).max(1),
            max_level: self.max_level - k, hash: self.hash, stored: self.stored, data, compressed,
        }
    }
    /// The texture its stored form makes, levels `from_level..` only, named `hash` (the
    /// texture's content key in the renderer's cache) with `stored` set: `stored` holds one key
    /// for a 2D texture, or one per layer for an array, which is gathered layer-major as a
    /// load gathers it. For raising a texture uploaded without its finest mips, and restoring
    /// one whose CPU copy is gone. `None` while a stored form is not in `store` (its encode is
    /// still owed); `Some(Err)` when one is unusable. Runs on a worker: a store read and a
    /// transcode of the kept levels.
    pub fn remake(store: &dyn PreparedTextureStore, stored: &[u64], from_level: usize, hash: u64) -> Option<Result<PreparedTexture, String>> {
        if stored.is_empty() { return Some(Err("no stored form".into())); }
        if stored.len() == 1 {
            let key = stored[0];
            let bytes = store.get_basis(key)?;
            return Some(texture_work(TextureWork::Remap, || crate::texture_pack::texture_from_ktx2_from(&bytes, from_level, Some(hash)))
                .unwrap_or_else(|| Err("stored texture is not KTX2".into()))
                .map(|mut texture| { texture.stored = Some(key); texture }));
        }
        let layers = stored.iter().map(|key| Self::remake(store, std::slice::from_ref(key), from_level, *key)
            .map(|texture| texture.map(std::sync::Arc::new))).collect::<Option<Vec<_>>>()?;
        Some(layers.into_iter().collect::<Result<Vec<_>, _>>().map(|layers| {
            let mut texture = gathered_from(&layers, 0);
            texture.hash = hash;
            texture.stored = None;
            texture
        }))
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
        Texture::new_with_format(cx,self.into_format())
    }
    /// Write this chain into `texture` in place of what it holds (another first mip, another size or
    /// format): every handle sharing `texture` draws it from the next frame; the platform
    /// reallocates when the size or format changes. Counted in [`RETUNE_STATS`].
    pub fn retune(self,cx:&mut Cx,texture:&Texture) {
        use std::sync::atomic::Ordering::Relaxed;
        RETUNE_STATS[0].fetch_add(1,Relaxed);
        RETUNE_STATS[1].fetch_add(self.bytes(),Relaxed);
        *texture.get_format(cx)=self.into_format();
    }
    fn into_format(self)->TextureFormat {
        if let Some((format,blocks))=self.compressed {
            return TextureFormat::VecMipCompressed{width:self.width,height:self.height,format,data:Some(blocks),max_level:Some(self.max_level),wrap:TextureWrap::Repeat,updated:TextureUpdated::Full};
        }
        TextureFormat::VecMipBGRAu8_32{width:self.width,height:self.height,data:Some(self.data),max_level:Some(self.max_level),wrap:TextureWrap::Repeat,updated:TextureUpdated::Full}
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
    pub(crate) fn charge(&self, slot: TextureSlot, texel_addressed: bool) -> TextureCharge {
        TextureCharge {
            key: self.hash, bytes: self.bytes(), width: self.width, height: self.height,
            layers: 1, mips: self.max_level + 1,
            compressed: self.compressed.as_ref().map(|(format, _)| *format), slot,
            stored: self.stored.into_iter().collect(), texel_addressed,
        }
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
        let transcode:fn(&[u8],&mut Vec<u8>)=match target {
            CompressedTextureFormat::Bc7=>makepad_texcomp::transcode::uastc_to_bc7_into,
            CompressedTextureFormat::Astc4x4=>makepad_texcomp::transcode::uastc_to_astc_into,
            // Normal maps are stored as BC5; UASTC never becomes BC5.
            CompressedTextureFormat::Bc5=>return None,
        };
        // A 4k chain took 0.29 s at a 2-core share: a pool job gives way between runs of blocks
        // (each 16-byte block transcodes on its own; this builds new data under no shared lock).
        for &(_,_,level) in &view.levels {
            for run in level.chunks(16*4096) {
                let _=thread::checkpoint_when_due();
                transcode(run,&mut blocks);
            }
        }
        Some(Self{width:width as usize,height:height as usize,data:Vec::new().into(),max_level:view.levels.len()-1,hash,stored:None,compressed:Some((target,blocks.into()))})
    }
    /// The device form of a stored Basis texture, transcoded.
    pub fn from_store(store:&dyn PreparedTextureStore,basis_key:u64,target:CompressedTextureFormat,hash:u64)->Option<Self>{
        store.get_basis(basis_key).and_then(|ktx|Self::from_basis(&ktx,target,hash))
    }
    /// Encode this RGBA chain to Basis/UASTC (KTX2 bytes, one level per mip).
    pub fn to_basis(&self,srgb:bool,encode: &(dyn Fn(usize, &(dyn Fn(usize) + Sync)) + Sync))->Option<Vec<u8>>{
        // Fast (about 1 dB under Default, 4x quicker), using the caller's executor.
        self.to_uastc(srgb,makepad_texcomp::uastc::Quality::Fast,encode).ok()?.to_ktx2().ok()
    }
    /// This chain's red and green (a normal map's X and Y) as a BC5 KTX2 with
    /// `content_id`, encoded through the supplied executor.
    pub fn to_bc5_ktx2(&self,content_id:u64,encode: &(dyn Fn(usize, &(dyn Fn(usize) + Sync)) + Sync))->Result<Vec<u8>,String>{
        let (mut w,mut h,mut offset)=(self.width,self.height,0usize);
        let mut levels=Vec::new();
        for _ in 0..=self.max_level {
            let texels=self.data.get(offset..offset+w*h).ok_or("mip chain shorter than its levels")?;
            let rgba=rgba_rows(texels,w,encode,|t|[(t>>16) as u8,(t>>8) as u8,0,255]);
            levels.push((w as u32,h as u32,makepad_texcomp::rgtc::encode_bc5_image(&rgba,w as u32,h as u32,encode)));
            offset+=w*h;w=(w/2).max(1);h=(h/2).max(1);
        }
        let refs:Vec<(u32,u32,&[u8])>=levels.iter().map(|(w,h,d)|(*w,*h,d.as_slice())).collect();
        makepad_texcomp::rgtc::bc5_to_ktx2(&refs,Some(content_id))
    }
    /// This RGBA chain as UASTC at a chosen quality through the supplied executor.
    pub fn to_uastc(&self,srgb:bool,quality:makepad_texcomp::uastc::Quality,encode: &(dyn Fn(usize, &(dyn Fn(usize) + Sync)) + Sync))->Result<makepad_texcomp::basis::UastcTexture,String>{
        let (mut w,mut h,mut offset)=(self.width,self.height,0usize);
        let mut levels=Vec::new();
        for _ in 0..=self.max_level {
            let n=w*h;
            let texels=self.data.get(offset..offset+n).ok_or("mip chain shorter than its levels")?;
            // BGRA words (0xAARRGGBB) to RGBA bytes.
            let rgba=rgba_rows(texels,w,encode,|t|[(t>>16) as u8,(t>>8) as u8,t as u8,(t>>24) as u8]);
            levels.push((w as u32,h as u32,rgba));
            offset+=n;w=(w/2).max(1);h=(h/2).max(1);
        }
        let refs:Vec<(u32,u32,&[u8])>=levels.iter().map(|(w,h,d)|(*w,*h,d.as_slice())).collect();
        makepad_texcomp::basis::encode_mips(&refs,quality,srgb,encode)
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
pub struct PreparedSurface {pub definition:MaterialSurface,pub normal:PreparedTexture,pub occlusion:PreparedTexture,pub emissive:PreparedTexture,pub splat:Option<PreparedSplat>}
#[derive(Clone)]
pub struct UploadedSurface {pub definition:std::sync::Arc<MaterialSurface>,pub normal:Texture,pub occlusion:Texture,pub emissive:Texture,pub splat:Option<UploadedSplat>}

/// A terrain paint's pixels: each layer's albedo (height in alpha) and
/// normal map at one size per kind, so each kind uploads as one 2D array,
/// and the weight map.
#[derive(Clone)]
pub struct PreparedSplat {
    pub albedo: Vec<std::sync::Arc<PreparedTexture>>,
    pub normal: Vec<std::sync::Arc<PreparedTexture>>,
    /// Each kind's layers gathered layer-major, the array upload's layout ([`gather_layers`]),
    /// on the preparing worker: the upload passes the chain on by reference instead of
    /// gathering it in the frame.
    pub albedo_array: std::sync::Arc<PreparedTexture>,
    pub normal_array: std::sync::Arc<PreparedTexture>,
    pub weights: PreparedTexture,
    pub params: [[f32; 4]; SPLAT_LAYERS],
    /// Per-layer relief and surface factors (shader parameters, not pixels).
    pub surface: [[f32; 4]; SPLAT_LAYERS],
    /// Per-layer world macro variation (SplatLayer::macro_variation).
    pub macro_variation: [[f32; 4]; SPLAT_LAYERS],
    /// Per-layer hex tiling, 1 or 0: layer i at [i / 4][i % 4]
    /// (uniforms splat_hex_a, splat_hex_b).
    pub stochastic: [[f32; 4]; 2],
    pub map: [f32; 4],
    pub size: [f32; 4],
    /// A worn road part's band uniforms (`splat_band`, `splat_band_map`,
    /// `splat_wear`); all zero without one.
    pub band: [[f32; 4]; 3],
}

/// The splat lane's bindings (shaders/splat.rs): the albedo and normal
/// arrays (shared by every tile with the same layers), the weight map, and
/// per layer (UV scale, roughness, metallic, normal strength).
#[derive(Clone)]
pub struct UploadedSplat {
    pub albedo: Texture,
    pub normal: Texture,
    pub weights: Texture,
    pub params: [[f32; 4]; SPLAT_LAYERS],
    /// Per-layer relief and surface factors (shader parameters, not pixels).
    pub surface: [[f32; 4]; SPLAT_LAYERS],
    /// Per-layer world macro variation (SplatLayer::macro_variation).
    pub macro_variation: [[f32; 4]; SPLAT_LAYERS],
    /// Per-layer hex tiling, 1 or 0: layer i at [i / 4][i % 4]
    /// (uniforms splat_hex_a, splat_hex_b).
    pub stochastic: [[f32; 4]; 2],
    /// Weight map: world x, z of texel 0, texels per metre.
    pub map: [f32; 4],
    /// Weight map: texels per row, rows per page, pages, layers.
    pub size: [f32; 4],
    /// A worn road part's band: (base, span, lateral span, half width),
    /// (metres per texel, texels per row, rows, closed length), wear
    /// amounts. All zero: no band.
    pub band: [[f32; 4]; 3],
}

struct SplatMap {
    png: PngMap,
    stored: Option<StoredMap>,
}

struct SplatKindPlan {
    maps: Vec<Option<SplatMap>>,
    size: ImageSize,
    stored_bytes: Option<usize>,
}

impl SplatKindPlan {
    fn new(sources: &[Option<&[u8]>], semantic: PixelSemantic) -> Result<Self, String> {
        let sizes = sources.iter().map(|source| source.map(|b| {
            if crate::texture_pack::is_ktx2(b) { return Err("terrain paint layers are PNG images".into()); }
            ImageSize::read(b, 4096)
        }).transpose()).collect::<Result<Vec<_>, _>>()?;
        // A missing or small source keeps this whole array in RGBA.
        let size = sizes.iter().flatten().copied().next().unwrap_or(ImageSize(1, 1));
        let store = sizes.iter().all(|s| s.is_some_and(|s| s.0 * s.1 >= COMPRESS_MIN_TEXELS));
        let maps: Vec<_> = sources.iter().zip(sizes).map(|(source, dims)| source.zip(dims).map(|(b, dims)| {
            let hash = texture_work(TextureWork::Hash, || image_content_hash(b));
            let png = PngMap::new(b, dims, hash, semantic, store.then_some(size), false);
            SplatMap { stored: png.lookup(size), png }
        })).collect();
        let layout = maps.iter().flatten().next().and_then(|m| m.stored.as_ref()).map(|s| s.layout);
        let stored_bytes = layout.filter(|l| l.format.is_some() && maps.iter().all(|m| {
            m.as_ref().and_then(|m| m.stored.as_ref()).is_some_and(|s| s.layout == *l)
        })).map(|l| l.bytes.saturating_mul(maps.len()));
        Ok(Self { maps, size, stored_bytes })
    }

    fn charge(&self, budget: &mut ImageBudget) {
        if let Some(bytes) = self.stored_bytes {
            budget.fixed = budget.fixed.saturating_add(bytes);
        } else {
            let sources = self.maps.iter().flatten().count();
            if sources == 0 { budget.fixed = budget.fixed.saturating_add(4 * self.maps.len()); }
            else { budget.add(self.size, true, self.maps.len(), sources); }
        }
    }

    fn prepare<'a>(&self, sources: impl Iterator<Item = Option<&'a [u8]>>, fill: impl Fn(usize) -> u32, semantic: PixelSemantic, remaining: &mut usize, level: usize) -> Result<Vec<std::sync::Arc<PreparedTexture>>, String> {
        let level = self.size.fit(level, *remaining / self.maps.len(), true);
        let ImageSize(w, h) = self.size.at(level);
        let mut layers = Vec::with_capacity(self.maps.len());
        for (i, (map, bytes)) in self.maps.iter().zip(sources).enumerate() {
            let texture = if let Some(SplatMap { png: map, stored }) = map {
                if self.stored_bytes.is_some() {
                    stored.as_ref().expect("complete stored array").prepare()?
                } else {
                    let bytes = bytes.expect("planned terrain PNG");
                    let texture = if map.size.at(level) == ImageSize(w, h) {
                        map.decoded(bytes, semantic, level)?
                    } else {
                        let image = crate::renderer::decode_model_image(bytes, 4096, level)?;
                        let mut out = ImageBuffer::default();
                        out.width = w; out.height = h;
                        out.data = (0..w * h).map(|k| image.data[(k / w) * image.height / h * image.width + (k % w) * image.width / w]).collect();
                        std::sync::Arc::new(PreparedTexture::prepare(out, semantic))
                    };
                    if let Some((_, key)) = map.store_key.filter(|_| level == 0 && stored.is_none()) {
                        owe(key, &texture, false);
                    }
                    texture
                }
            } else {
                let mut image = ImageBuffer::default();
                image.width = w; image.height = h; image.data = vec![fill(i); w * h];
                std::sync::Arc::new(PreparedTexture::prepare(image, semantic))
            };
            *remaining = remaining.saturating_sub(texture.bytes());
            layers.push(texture);
        }
        Ok(layers)
    }
}

struct SplatPlan {
    albedo: SplatKindPlan,
    normal: SplatKindPlan,
    weight_bytes: usize,
}

impl SplatPlan {
    fn new(splat: &SplatSurface) -> Result<Self, String> {
        let layers = &splat.layers[..splat.layers.len().min(SPLAT_LAYERS)];
        if layers.is_empty() { return Err("terrain paint without layers".into()); }
        let weights = if splat.weights_png.is_empty() && layers.len() == 1 { ImageSize(1, 1) }
            else { ImageSize::read(&splat.weights_png, 4096)? };
        let albedo = layers.iter().map(|l| l.albedo_png.as_deref()).collect::<Vec<_>>();
        let normal = layers.iter().map(|l| l.normal_png.as_deref()).collect::<Vec<_>>();
        Ok(Self { albedo: SplatKindPlan::new(&albedo, PixelSemantic::MaskedColor)?,
            normal: SplatKindPlan::new(&normal, PixelSemantic::Normal)?, weight_bytes: weights.bytes(true) })
    }

    fn charge(&self, budget: &mut ImageBudget) {
        budget.fixed = budget.fixed.saturating_add(self.weight_bytes);
        self.albedo.charge(budget);
        self.normal.charge(budget);
    }
}

impl PreparedSplat {
    pub fn prepare(splat: &SplatSurface, remaining: &mut usize) -> Result<Self, String> {
        let mut budget = ImageBudget::default();
        let plan = SplatPlan::new(splat)?;
        plan.charge(&mut budget);
        let level = budget.level(*remaining);
        Self::prepare_planned(splat, &plan, remaining, level)
    }

    fn prepare_planned(splat: &SplatSurface, plan: &SplatPlan, remaining: &mut usize, level: usize) -> Result<Self, String> {
        let layers = &splat.layers[..splat.layers.len().min(SPLAT_LAYERS)];
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let albedo = plan.albedo.prepare(layers.iter().map(|l| l.albedo_png.as_deref()),
            |i| { let c = layers[i].color; 0x8000_0000 | byte(c[0]) << 16 | byte(c[1]) << 8 | byte(c[2]) },
            PixelSemantic::MaskedColor, remaining, level)?;
        let normal = plan.normal.prepare(layers.iter().map(|l| l.normal_png.as_deref()), |_| 0x8080_80ff,
            PixelSemantic::Normal, remaining, level)?;
        let image = if splat.weights_png.is_empty() && layers.len() == 1 {
            let mut image = ImageBuffer::default();
            image.width = 1;
            image.height = 1;
            image.data = vec![0xffff_0000];
            image
        } else {
            // The weights say where each layer goes and may stack two RGBA
            // pages, so they are never reduced: only the dimension bound
            // applies. The layers above carry the budget's reduction.
            texture_work(TextureWork::DecodeMip, || crate::renderer::decode_generated_png(&splat.weights_png, 4096, 4096 * 4096 * 4))?
        };
        let weights = PreparedTexture::prepare(image, PixelSemantic::Data);
        *remaining = remaining.saturating_sub(weights.bytes());
        let mut params = [[0.0f32; 4]; SPLAT_LAYERS];
        for (p, l) in params.iter_mut().zip(layers) {
            *p = [l.scale, l.roughness, l.metallic, if l.normal_png.is_some() { l.bump } else { 0.0 }];
        }
        let mut surface = [[0.0f32; 4]; SPLAT_LAYERS];
        for ((p, l), texture) in surface.iter_mut().zip(layers).zip(&albedo) {
            // The height distribution is not uniform (equalised recipes
            // retain part of their raw relief). Convert a share to a level
            // once on this worker, never during a frame.
            let mut level = 0.0;
            if l.puddles > 0.0 {
                let histogram = level0_alpha_histogram(texture);
                let count = texture.width * texture.height;
                let target = (l.puddles * count as f32).ceil() as u32;
                let mut total = 0;
                for (height, n) in histogram.iter().enumerate() {
                    total += n;
                    if total >= target { level = height as f32 / 255.0; break; }
                }
            }
            *p = [l.relief, l.wet, l.puddles, level];
        }
        let mut macro_variation = [[0.0f32; 4]; SPLAT_LAYERS];
        for (m, l) in macro_variation.iter_mut().zip(layers) { *m = l.macro_variation; }
        let mut stochastic = [[0.0f32; 4]; 2];
        for (i, l) in layers.iter().enumerate() { stochastic[i / 4][i % 4] = if l.stochastic { 1.0 } else { 0.0 }; }
        let pages = layers.len().div_ceil(4) as f32;
        let band = splat.band.as_ref().map_or([[0.0; 4]; 3], |b| [[b.base, b.span, b.lateral, b.half], [b.cell, b.row, b.rows, b.length], b.wear]);
        let (albedo_array, normal_array) = (gather_layers(&albedo), gather_layers(&normal));
        Ok(Self { albedo, normal, albedo_array, normal_array, weights, params, surface, macro_variation, stochastic, map: [splat.origin[0], splat.origin[1], 1.0 / splat.cell.max(1e-6), 0.0], size: [splat.width, splat.rows, pages, layers.len() as f32], band })
    }

    /// Gather the arrays again after the layers changed (they are gathered at preparation).
    /// Not through the load memo: changed layers keep their content keys.
    pub fn gather_arrays(&mut self) {
        self.albedo_array = std::sync::Arc::new(gathered(&self.albedo));
        self.normal_array = std::sync::Arc::new(gathered(&self.normal));
    }

    /// Upload, sharing the arrays (by their layers' content) and the
    /// weight map (by its own) through the renderer's texture cache. The
    /// arrays were gathered on the preparing worker; a reduced upload gathers only the kept mips.
    pub fn upload(self, cx: &mut Cx, cache: &mut std::collections::HashMap<u64, Texture>, first_level: &dyn Fn(u64) -> usize) -> UploadedSplat {
        let array = |cx: &mut Cx, cache: &mut std::collections::HashMap<u64, Texture>, prepared: &PreparedTexture, layers: &[std::sync::Arc<PreparedTexture>]| -> Texture {
            if let Some(t) = cache.get(&prepared.hash) { return t.clone(); }
            let k = first_level(prepared.hash).min(prepared.deepest_first_level());
            let reduced;
            let upload = if k > 0 {
                // Each layer's kept levels, gathered in this frame: at most a quarter of the array.
                reduced = gathered_from(layers, k);
                UPLOAD_UI[3].fetch_add(reduced.bytes(), std::sync::atomic::Ordering::Relaxed);
                &reduced
            } else { prepared };
            let stored = layers_stored(layers);
            let layers = layers.len();
            let (width, height, max_level) = (upload.width, upload.height, Some(upload.max_level));
            let texture = match &upload.compressed {
                Some((format, blocks)) => Texture::new_with_format(cx, TextureFormat::VecArrayMipCompressed { width, height, layers, format: *format, data: Some(blocks.clone()), max_level, wrap: TextureWrap::Repeat, updated: TextureUpdated::Full }),
                None => Texture::new_with_format(cx, TextureFormat::VecArrayMipBGRAu8_32 { width, height, layers, data: Some(upload.data.clone()), max_level, wrap: TextureWrap::Repeat, updated: TextureUpdated::Full }),
            };
            crate::texture_restore::note(cx, &texture, upload.compressed.is_some(), &stored, k, prepared.hash);
            cache.insert(prepared.hash, texture.clone());
            texture
        };
        let albedo = array(cx, cache, &self.albedo_array, &self.albedo);
        let normal = array(cx, cache, &self.normal_array, &self.normal);
        let weights = if let Some(texture) = cache.get(&self.weights.hash) { texture.clone() } else {
            let k = first_level(self.weights.hash).min(self.weights.deepest_first_level());
            let weights = if k > 0 {
                let reduced = self.weights.from_level(k);
                UPLOAD_UI[3].fetch_add(reduced.bytes(), std::sync::atomic::Ordering::Relaxed);
                reduced
            } else { self.weights };
            let (hash, stored, compressed) = (weights.hash, weights.stored, weights.compressed.is_some());
            let texture = weights.upload_cached(cx, cache);
            crate::texture_restore::note(cx, &texture, compressed, stored.as_slice(), k, hash);
            texture
        };
        UploadedSplat { albedo, normal, weights, params: self.params, surface: self.surface, macro_variation: self.macro_variation, stochastic: self.stochastic, map: self.map, size: self.size, band: self.band }
    }

    pub fn bytes(&self) -> usize { self.albedo.iter().chain(&self.normal).map(|t| t.bytes()).sum::<usize>() + self.weights.bytes() }
}

pub(crate) struct SurfacePlan {
    normal: MapPlan,
    occlusion: MapPlan,
    emissive: MapPlan,
    splat: Option<SplatPlan>,
}

impl SurfacePlan {
    pub(crate) fn new(surface: &MaterialSurface, budget: &mut ImageBudget) -> Result<Self, String> {
        let plan = Self::planned(surface)?;
        plan.fetch();
        plan.charge(budget);
        Ok(plan)
    }

    pub(crate) fn planned(surface: &MaterialSurface) -> Result<Self, String> {
        let depth = if surface.impostor > 0.0 { PixelSemantic::Data16 } else { PixelSemantic::Data };
        let map = |bytes: Option<&[u8]>, fallback, semantic| {
            MapPlan::planned(bytes, MapPlan::source_hash(bytes), fallback, semantic, true)
        };
        Ok(Self {
            normal: map(surface.normal_png.as_deref(), 0xff80_80ff, PixelSemantic::Normal)?,
            occlusion: map(surface.occlusion_png.as_deref(), 0xffff_ffff, depth)?,
            emissive: map(surface.emissive_png.as_deref(), 0xffff_ffff, PixelSemantic::Color)?,
            splat: surface.splat.as_deref().map(SplatPlan::new).transpose()?,
        })
    }

    pub(crate) fn maps(&self) -> [&MapPlan; 3] { [&self.normal, &self.occlusion, &self.emissive] }

    fn fetch(&self) {
        for map in self.maps() { map.fetch(); }
    }

    pub(crate) fn charge(&self, budget: &mut ImageBudget) {
        for map in self.maps() { map.charge(budget); }
        if let Some(splat) = &self.splat { splat.charge(budget); }
    }

    pub(crate) fn prepare(&self, mut definition: MaterialSurface, remaining: &mut usize, level: usize) -> Result<PreparedSurface, String> {
        let normal = self.normal.prepare(definition.normal_png.take().as_deref(), level, remaining)?;
        let occlusion = self.occlusion.prepare(definition.occlusion_png.take().as_deref(), level, remaining)?;
        let emissive = self.emissive.prepare(definition.emissive_png.take().as_deref(), level, remaining)?;
        let splat = self.splat.as_ref().zip(definition.splat.take()).map(|(plan, s)| {
            PreparedSplat::prepare_planned(&s, plan, remaining, level)
        }).transpose()?;
        Ok(PreparedSurface { definition, normal: std::sync::Arc::unwrap_or_clone(normal),
            occlusion: std::sync::Arc::unwrap_or_clone(occlusion), emissive: std::sync::Arc::unwrap_or_clone(emissive), splat })
    }
}

impl PreparedSurface {
    pub fn prepare(definition: MaterialSurface, remaining: &mut usize) -> Result<Self, String> {
        let _preparing = Preparing::begin();
        let mut budget = ImageBudget::default();
        let plan = SurfacePlan::new(&definition, &mut budget)?;
        let level = budget.level(*remaining);
        plan.prepare(definition, remaining, level)
    }
    pub fn bytes(&self)->usize{self.normal.bytes()+self.occlusion.bytes()+self.emissive.bytes()+self.splat.as_ref().map_or(0,PreparedSplat::bytes)}
    pub fn upload(self,cx:&mut Cx)->UploadedSurface {UploadedSurface{definition:std::sync::Arc::new(self.definition),normal:self.normal.upload(cx),occlusion:self.occlusion.upload(cx),emissive:self.emissive.upload(cx),splat:self.splat.map(|s|s.upload(cx,&mut Default::default(),&|_|0))}}
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn stored_remakes_keep_the_requested_mips_and_keys() {
        use crate::texture_pack::texture_from_ktx2;
        use std::{collections::HashMap, sync::{Arc, Mutex}};
        struct Store(Mutex<HashMap<u64, Vec<u8>>>);
        impl PreparedTextureStore for Store {
            fn get_basis(&self, key: u64) -> Option<Vec<u8>> { self.0.lock().unwrap().get(&key).cloned() }
            fn put_basis(&self, key: u64, bytes: &[u8]) { self.0.lock().unwrap().insert(key, bytes.to_vec()); }
        }
        let same_chain = |got: &PreparedTexture, want: &PreparedTexture, property: &str| {
            assert_eq!((got.width, got.height, got.max_level), (want.width, want.height, want.max_level), "{property}: dimensions and mip count");
            assert_eq!(&got.data[..], &want.data[..], "{property}: RGBA bytes");
            assert_eq!(got.compressed.as_ref().map(|(f, b)| (*f, &b[..])), want.compressed.as_ref().map(|(f, b)| (*f, &b[..])), "{property}: format and compressed blocks");
        };
        let make = |offset: u32| {
            let mut image = ImageBuffer::default();
            image.width = 16; image.height = 16;
            image.data = (0..256u32).map(|i| 0x8000_0000 | ((i + offset) % 256) << 16 | ((i * 3 + offset) % 256) << 8 | (i * 7 + offset) % 256).collect();
            PreparedTexture::prepare(image, PixelSemantic::Data)
        };
        let (a, b, normal, missing, invalid) = (0xa, 0xb, 0xc, 0xd, 0xe);
        let store = Store(Mutex::new(HashMap::new()));
        let rgba = make(0);
        let bytes = rgba.to_uastc(false, makepad_texcomp::uastc::Quality::Fast, &makepad_texcomp::run_inline).unwrap().to_ktx2_with(Some(a)).unwrap();
        store.put_basis(a, &bytes);
        let full = texture_from_ktx2(&bytes).unwrap().unwrap();
        assert_eq!(full.hash, a, "the default KTX2 load retains its content id");
        let tail = PreparedTexture::remake(&store, &[a], 1, 0x77).unwrap().unwrap();
        same_chain(&tail, &full.from_level(1), "UASTC remake from level 1");
        assert_eq!((tail.hash, tail.stored), (0x77, Some(a)), "a 2D remake keeps the requested cache key and stored key");
        let restored = PreparedTexture::remake(&store, &[a], 0, 0x78).unwrap().unwrap();
        same_chain(&restored, &full, "UASTC full-chain restore");
        assert_eq!((restored.hash, restored.stored), (0x78, Some(a)), "a full restore keeps the requested cache key and stored key");
        assert!(PreparedTexture::remake(&store, &[missing], 1, 0x77).is_none(), "a missing stored form waits for its encode");
        assert!(matches!(PreparedTexture::remake(&store, &[], 0, 0x77), Some(Err(e)) if e == "no stored form"), "an empty stored-key list is unusable");
        store.put_basis(invalid, b"not KTX2");
        assert!(matches!(PreparedTexture::remake(&store, &[invalid], 0, 0x77), Some(Err(_))), "a stored form that is not KTX2 is unusable");
        for k in [full.max_level + 1, usize::MAX] {
            assert!(matches!(PreparedTexture::remake(&store, &[a], k, 0x77), Some(Err(_))), "a UASTC file without requested level {k} is unusable");
        }

        let bytes_b = make(61).to_uastc(false, makepad_texcomp::uastc::Quality::Fast, &makepad_texcomp::run_inline).unwrap().to_ktx2_with(Some(b)).unwrap();
        store.put_basis(b, &bytes_b);
        let full_b = texture_from_ktx2(&bytes_b).unwrap().unwrap();
        let layers = [Arc::new(full.from_level(1)), Arc::new(full_b.from_level(1))];
        let array = PreparedTexture::remake(&store, &[a, b], 1, 0x79).unwrap().unwrap();
        same_chain(&array, &gathered_from(&layers, 0), "array remake gathers kept mips layer-major");
        assert_eq!((array.hash, array.stored), (0x79, None), "an array uses the requested cache key and leaves stored keys to its charge");
        assert!(PreparedTexture::remake(&store, &[a, missing], 1, 0x79).is_none(), "an array waits for every layer's stored form");
        assert!(PreparedTexture::remake(&store, &[invalid, missing], 0, 0x79).is_none(), "a missing array layer remains pending even when another is unusable");
        assert!(matches!(PreparedTexture::remake(&store, &[a, invalid], 0, 0x79), Some(Err(_))), "an unusable array layer is reported when all are present");

        let bc5 = rgba.to_bc5_ktx2(normal, &makepad_texcomp::run_inline).unwrap();
        store.put_basis(normal, &bc5);
        let _bc5 = BC5_NORMAL_MAPS_TEST_LOCK.lock().unwrap();
        let before = bc5_normal_maps();
        for compressed in [false, true] {
            set_bc5_normal_maps(compressed);
            let full = texture_from_ktx2(&bc5).unwrap().unwrap();
            let tail = PreparedTexture::remake(&store, &[normal], 1, 0x7a).unwrap().unwrap();
            same_chain(&tail, &full.from_level(1), "BC5 remake from level 1");
            assert_eq!(tail.compressed.is_some(), compressed, "BC5 remake follows the device's normal-map format");
            assert_eq!((tail.hash, tail.stored), (0x7a, Some(normal)), "BC5 remake keeps the requested cache key and stored key");
            assert!(matches!(PreparedTexture::remake(&store, &[normal], full.max_level + 1, 0x7a), Some(Err(_))), "a BC5 file without the requested level is unusable");
        }
        set_bc5_normal_maps(before);
        drop(_bc5);

        // Present the same UASTC form inline and through the fixture store.
        let _load = Preparing::begin();
        let png = Cx::encode_rgba_as_png(16, 16, &[127; 16 * 16 * 4]).unwrap();
        let source = Some(png.as_slice());
        let hash = image_content_hash(&png);
        let mut eager = ImageBudget::default();
        let raw = MapPlan::with_hash(source, hash, 0, PixelSemantic::Data, false, &mut eager).unwrap();
        let packed = MapPlan::with_hash(Some(&bytes), a, 0, PixelSemantic::Data, false, &mut eager).unwrap();
        let mut eager_left = 4096;
        let raw_texture = raw.prepare(source, 1, &mut eager_left).unwrap();
        let packed_texture = packed.prepare(Some(&bytes), 1, &mut eager_left).unwrap();

        static LOOKUPS: std::sync::OnceLock<Store> = std::sync::OnceLock::new();
        let lookups = LOOKUPS.get_or_init(|| Store(Mutex::new(HashMap::new())));
        lookups.put_basis(a, &bytes);
        let planned = |key| {
            let mut plan = MapPlan::planned(source, hash, 0, PixelSemantic::Data, false).unwrap();
            let MapSource::Png(image, _) = &mut plan.kind else { unreachable!() };
            image.store_key = Some((lookups, key));
            plan
        };
        let (miss, hit) = (planned(missing), planned(a));
        spreading_conversions(&TaskPool::closed(), || {
            fetch_ahead(&[&miss, &hit]);
            // Both decisions must survive a store change between fetch and charge.
            let form = lookups.0.lock().unwrap().remove(&a).unwrap();
            lookups.put_basis(missing, &form);
            let mut budget = ImageBudget::default();
            miss.charge(&mut budget);
            hit.charge(&mut budget);
            assert_eq!((budget.fixed, budget.count), (eager.fixed, eager.count), "stored bytes and source count");
            assert!(budget.images == eager.images, "reducible image charges");
            let mut left = 4096;
            same_chain(&miss.prepare(source, 1, &mut left).unwrap(), &raw_texture, "PNG miss");
            let texture = hit.prepare(source, 1, &mut left).unwrap();
            same_chain(&texture, &packed_texture, "prefetched stored PNG");
            assert_eq!((texture.hash, texture.stored, left), (a, Some(a), eager_left), "stored identity and remaining budget");
        });
        lookups.0.lock().unwrap().clear();
    }

    #[test]
    fn load_held_chains_are_those_only_the_memo_and_encodes_hold() {
        let held = |allocation: usize| { let mut found = None; load_held_chains(|a, bytes| if a == allocation { found = Some(bytes) }); found };
        // The test's scope keeps the memo while it reads it (other tests' scopes may end meanwhile).
        let scope = Preparing::begin();
        let make = || Ok(PreparedTexture { width: 2, height: 2, data: vec![1u32, 2, 3, 4, 5].into(), max_level: 1, hash: 0x10ad, stored: None, compressed: None });
        let key = (0x10ad_4e1d_c4a1_0001, 20, 7);
        let texture = prepared_once(key, make).unwrap();
        let allocation = texture.data.allocation();
        assert_eq!(held(allocation), None, "a load in flight holds the memo's texture: it is the load's");
        let uploaded = texture.data.clone();
        drop(texture);
        assert_eq!(held(allocation), None, "a chain the renderer also holds is the renderer's");
        drop(uploaded);
        assert_eq!(held(allocation), Some(20), "the memo alone holds it: five texels of four bytes");
        let encode = StoreBasis::new(0x10ad, &prepared_once(key, make).unwrap(), false);
        assert_eq!(held(allocation), Some(20), "the memo and an owed encode hold it once");
        drop(encode);
        assert_eq!(held(allocation), Some(20), "the encode's registration went with it");
        drop(scope);
    }
    #[test]
    fn splat_preparation_keeps_roughness_alpha_and_quantiles() {
        let albedo: Vec<u8> = [32u8, 96, 160, 224].into_iter().flat_map(|h| [128, 128, 128, h]).collect();
        let normal: Vec<u8> = [16u8, 80, 176, 240].into_iter().flat_map(|r| [128, 128, 255, r]).collect();
        let layer = SplatLayer {
            albedo_png: Some(Cx::encode_rgba_as_png(2, 2, &albedo).unwrap()), color: [0.5; 3],
            normal_png: Some(Cx::encode_rgba_as_png(2, 2, &normal).unwrap()),
            scale: 0.5, roughness: 0.7, metallic: 0.0, bump: 0.0, relief: 0.04, wet: 0.6, puddles: 0.25, macro_variation: [0.2, 0.05, 0.1, 14.0], stochastic: true,
        };
        let source = SplatSurface { layers: vec![layer], weights_png: Vec::new(), origin: [0.0; 2], cell: 1.0, width: 1.0, rows: 1.0, band: None };
        let prepared = PreparedSplat::prepare(&source, &mut 4096).unwrap();
        assert_eq!(prepared.params[0], [0.5, 0.7, 0.0, 0.0], "roughness remains available with bump zero");
        assert_eq!(prepared.surface[0], [0.04, 0.6, 0.25, 32.0 / 255.0], "puddle share selects the lowest quarter");
        assert_eq!(prepared.normal[0].data[..4].iter().map(|p| p >> 24).collect::<Vec<_>>(), [16, 80, 176, 240], "level-zero roughness alpha survives");
        assert_eq!(prepared.normal[0].data[4] >> 24, 128, "mip alpha averages linearly");
        assert_eq!(prepared.size[3], 1.0, "single mesh surface needs no weight map read");
        assert_eq!(prepared.macro_variation[0], [0.2, 0.05, 0.1, 14.0], "the layer's macro variation reaches its uniform");
        assert_eq!(prepared.stochastic, [[1.0, 0.0, 0.0, 0.0], [0.0; 4]], "the layer's hex tiling reaches its uniform and unused layers stay off");
    }

    #[test]
    fn mip_bytes_split_the_uploaded_chain() {
        // Odd, non-square sides: the RGBA chain and its BC7 transcode both halve to 1x1.
        let (w, h) = (37, 20);
        let mut image = ImageBuffer::default();
        image.width = w; image.height = h;
        let mut x = 0x9e37_79b9u32;
        image.data = (0..w * h).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; x | 0xff00_0000 }).collect();
        let rgba = PreparedTexture::prepare(image, PixelSemantic::Color);
        let charge = rgba.charge(TextureSlot::Base, false);
        let mips = charge.mip_bytes();
        assert_eq!((mips.len(), mips[0], *mips.last().unwrap()), (charge.mips, (w * h * 4) as u64, 4), "RGBA mips run from level 0 to one texel");
        assert_eq!(mips.iter().sum::<u64>(), rgba.bytes() as u64, "RGBA mips sum to the upload");
        let ktx2 = rgba.to_basis(true, &|n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f)).expect("UASTC encode");
        let bc7 = PreparedTexture::from_basis(&ktx2, CompressedTextureFormat::Bc7, rgba.hash).expect("BC7 transcode");
        let charge = bc7.charge(TextureSlot::Base, false);
        let mips = charge.mip_bytes();
        assert_eq!((mips.len(), mips[0], *mips.last().unwrap()), (charge.mips, (10 * 5 * 16) as u64, 16), "block mips count 4x4 blocks, one at the tail");
        assert_eq!(mips.iter().sum::<u64>(), bc7.bytes() as u64, "BC7 mips sum to the upload");
        let array = TextureCharge { layers: 5, ..charge };
        assert_eq!(array.mip_bytes(), mips.iter().map(|b| b * 5).collect::<Vec<_>>(), "an array's mip holds every layer");
        // Uploads from a first level: RGBA may start at any level; a block chain only at a level
        // of whole 4x4 blocks, which 37x20 never has.
        let charge = rgba.charge(TextureSlot::Base, false);
        assert_eq!((rgba.deepest_first_level(), charge.deepest_first_level()), (rgba.max_level, rgba.max_level), "RGBA starts at any level");
        let tail = rgba.from_level(2);
        assert_eq!((tail.width, tail.height, tail.max_level, tail.hash), (9, 5, rgba.max_level - 2, rgba.hash), "level 2 on keeps the content key");
        assert_eq!(&tail.data[..], &rgba.data[w * h + 18 * 10..], "the kept levels are the chain's tail, texel for texel");
        assert_eq!(charge.mip_bytes()[2..].iter().sum::<u64>(), tail.bytes() as u64, "the charge's tail bytes are what uploads");
        assert_eq!((bc7.deepest_first_level(), bc7.charge(TextureSlot::Base, false).deepest_first_level()), (0, 0), "37x20 BC7 has no level of whole blocks");
        let mut square = ImageBuffer::default();
        square.width = 16; square.height = 16;
        square.data = (0..256u32).map(|i| i.wrapping_mul(0x9e37_79b9) | 0xff00_0000).collect();
        let square = PreparedTexture::prepare(square, PixelSemantic::Color);
        let ktx2 = square.to_basis(true, &|n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f)).expect("UASTC encode");
        let square = PreparedTexture::from_basis(&ktx2, CompressedTextureFormat::Bc7, square.hash).expect("BC7 transcode");
        assert_eq!(square.deepest_first_level(), 2, "16x16 BC7 starts at most at 4x4");
        let tail = square.from_level(2);
        let blocks = |t: &PreparedTexture| t.compressed.as_ref().unwrap().1.clone();
        assert_eq!((tail.width, tail.height, tail.max_level), (4, 4, square.max_level - 2), "level 2 on of a block chain");
        assert_eq!(&blocks(&tail)[..], &blocks(&square)[(16 + 4) * 16..], "the kept levels are the chain's tail, block for block");
        assert_eq!(square.charge(TextureSlot::Base, false).mip_bytes()[2..].iter().sum::<u64>(), tail.bytes() as u64, "the charge's block tail is what uploads");
    }

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
        let t=|a:u32|PreparedTexture{width:2,height:1,data:vec![0x00ff_ffff|a<<24,0xffff_ffff].into(),max_level:0,hash:0xa11a_0000|a as u64,stored:None,compressed:None};
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
    /// A 16-bit value split over red and green keeps its exact mean in every
    /// mip, across high-byte boundaries where the low byte wraps 255 -> 0.
    #[test]fn data16_mips_average_the_whole_value(){
        let n=8;let mut image=ImageBuffer::default();image.width=n;image.height=n;
        let value=|i:usize|0x12f0u32+(i as u32*7)%40;
        // Blue: a byte of its own (a stand-in's occlusion), 0 or 255 by texel.
        let blue=|i:usize|if (i/3)%2==0{0u32}else{255};
        image.data=(0..n*n).map(|i|{let q=value(i);0xff00_0000|(q>>8)<<16|(q&255)<<8|blue(i)}).collect();
        let texture=PreparedTexture::prepare(image,PixelSemantic::Data16);
        let mut level=(0..n*n).map(value).collect::<Vec<_>>();
        let mut blues=(0..n*n).map(|i|blue(i) as f32/255.0).collect::<Vec<_>>();
        let(mut w,mut start)=(n,0usize);
        while w>1 {
            start+=w*w;let nw=w/2;
            level=(0..nw*nw).map(|i|{let(x,y)=(i%nw*2,i/nw*2);(level[y*w+x]+level[y*w+x+1]+level[(y+1)*w+x]+level[(y+1)*w+x+1]+2)/4}).collect();
            blues=(0..nw*nw).map(|i|{let(x,y)=(i%nw*2,i/nw*2);(blues[y*w+x]+blues[y*w+x+1]+blues[(y+1)*w+x]+blues[(y+1)*w+x+1])*0.25}).collect();
            for(i,want)in level.iter().enumerate(){
                let t=texture.data[start+i];
                assert_eq!((t>>8)&0xffff,*want,"level {}x{} texel {i}: high {} low {}",nw,nw,(t>>16)&255,(t>>8)&255);
                let b=(t&255) as f32/255.0;
                assert!((b-blues[i]).abs()<=1.0/255.0,"level {}x{} texel {i}: blue {b} averages as its own byte, not {}",nw,nw,blues[i]);
            }
            w=nw;
        }
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
        assert_eq!(texture.data.as_slice(),expect.as_slice());
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

#[cfg(test)]
mod lamp_tests {
    use super::*;
    use makepad_gltf::GlbLamp;
    use makepad_scene::light::LampSignals;
    #[test]
    fn signal_lamps_follow_each_cars_brake_and_reverse() {
        let cruising = LampSignals::default();
        let braking = LampSignals { brake: true, reverse: false };
        let reversing = LampSignals { brake: false, reverse: true };
        // Daylight: only a signal lights a lamp.
        assert_eq!(lamp_gain(GlbLamp::Tail, cruising, 0.0), 0.0);
        assert_eq!(lamp_gain(GlbLamp::Tail, braking, 0.0), 1.0);
        assert_eq!(lamp_gain(GlbLamp::Brake, braking, 0.0), 1.0);
        assert_eq!(lamp_gain(GlbLamp::Reverse, reversing, 0.0), 1.0);
        assert_eq!(lamp_gain(GlbLamp::Reverse, braking, 0.0), 0.0);
        // After dark the tail lamps run dimly; braking still reads brighter,
        // and a stop lamp stays dark until the brake.
        let night = lamp_gain(GlbLamp::Tail, cruising, 1.0);
        assert!(night > 0.1 && night < 0.5);
        assert_eq!(lamp_gain(GlbLamp::Tail, braking, 1.0), 1.0);
        assert_eq!(lamp_gain(GlbLamp::Brake, cruising, 1.0), 0.0);
    }
}
