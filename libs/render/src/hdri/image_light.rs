//! Image lights (phase 1b): a picture as a light's content, and the set of
//! loaded images an `Env` draws from.
//!
//! `Env::new` stays pure: it never opens a file. The app loads pictures
//! (`image::load_image`, on a background job) into an [`ImageSet`] under a
//! key, the params name the key, and `Env::new_with` looks it up. A key that
//! is not in the set draws the plain light, so a preset whose picture has
//! moved still loads and still lights.
//!
//! A light's picture fills its box (width x height, turned by its roll) and
//! is cut by its shape, soft edge and hotspot like any other light. It is
//! **normalised to mean luminance 1** and then multiplied by the light's
//! colour, so `intensity_ev`, Kelvin, source and gels keep their meaning: a
//! picture of a window at +3 EV puts out what a plain softbox at +3 EV does,
//! and a HDR picture of a lamp keeps its hot core.
//!
//! With `alpha_as_mask` the picture's alpha cuts the light out (a PNG of a
//! lamp or a window frame). Alpha comes from 8-bit files; `.hdr` has none
//! and reads as opaque.
//!
//! P1: a picture's Penumbra fades its border out over a band inside it (in
//! fit the letterboxed picture's own border, in stretch and fill the light's
//! edge, which widens inward; `LightImageParams::penumbra`). P2: its Mask
//! softness box-filters a masked picture's premultiplied texels over its own
//! share of the picture's smaller side (`LightImageParams::mask_softness`),
//! exactly, through a summed-area table one per picture (`SummedArea`, which
//! image gobos share too).

use makepad_draw::*;
use makepad_render_material::ibl::EnvMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use super::studio::light_frame;
use super::skyline::SkylineBand;
use super::{HdriParams, ImageFit, LightImageParams};

/// The longest side a light's picture is kept at. A light rarely spans more
/// than 60 degrees, which an 8K map draws about 1400 texels wide; beyond
/// 2048 a picture only costs memory (16 bytes a texel).
pub const MAX_LIGHT_IMAGE_SIDE: usize = 2048;

/// How wide Mask softness 1 box-filters a masked picture's cut-out edges
/// (P2, the owner's decision): this share of the picture's smaller side,
/// across, in its texels (`LightImageParams::mask_softness`). A 512 x 256
/// logo blurs over 12.8 texels at 1, a 2048 square picture over 102.4.
pub const MASK_SOFTNESS_SHARE: f32 = 0.05;

/// The loaded pictures an `Env` may use, by key. The maps are in the FILE
/// orientation `image::load_image` returns: row 0 on top, no column roll;
/// only the base image (B15, `BaseImageParams::key`) is an equirect in the
/// engine convention (`image::load_equirect`). Cloning is cheap: the pixels
/// are shared.
#[derive(Clone, Debug, Default)]
pub struct ImageSet {
    pub images: HashMap<String, Arc<EnvMap>>,
    /// The skyline's strip (`HdriParams::skyline`), rendered by the app; None
    /// draws no skyline.
    pub skyline: Option<Arc<SkylineBand>>,
}

impl ImageSet {
    pub fn get(&self, key: &str) -> Option<&Arc<EnvMap>> {
        self.images.get(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.images.contains_key(key)
    }

    pub fn insert(&mut self, key: impl Into<String>, map: EnvMap) {
        self.images.insert(key.into(), Arc::new(map));
    }

    /// The keys, sorted, so lists and messages come out the same every time.
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.images.keys().cloned().collect();
        keys.sort();
        keys
    }
}

/// What is worked out once per loaded picture and kept while it lives: one
/// entry per live picture, found by the address of its allocation, shared by
/// every light, gobo and `Env` that shows the picture. `Env::new_with` runs
/// per preview bake, so nothing may read every texel of a large picture each
/// time (D15): a base image's facts (`composite::image_facts`), a gobo's
/// summed-area table, a light picture's mean. A dead `Weak` still holds its
/// allocation, so an address is never reused while its entry is here; dead
/// entries are dropped whenever a new one is added, and by
/// `composite::drop_dead_facts`, which a host that frees a picture calls.
pub(crate) struct PictureMemo<T> {
    entries: Mutex<Vec<(Weak<EnvMap>, Arc<T>)>>,
}

impl<T> PictureMemo<T> {
    pub(crate) const fn new() -> PictureMemo<T> {
        PictureMemo { entries: Mutex::new(Vec::new()) }
    }

    /// What is kept for `map`, made by `make` on the first call and shared
    /// from then on. Made outside the lock, so other pictures' lookups do not
    /// wait for it; two threads may make the same picture's at once, and the
    /// first one's stays.
    pub(crate) fn get(&self, map: &Arc<EnvMap>, make: impl FnOnce(&EnvMap) -> T) -> Arc<T> {
        let address = Arc::as_ptr(map);
        if let Ok(entries) = self.entries.lock() {
            if let Some((_, kept)) = entries.iter().find(|(picture, _)| picture.as_ptr() == address) {
                return kept.clone();
            }
        }
        let made = Arc::new(make(map));
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|(picture, _)| picture.strong_count() > 0);
            if let Some((_, first)) = entries.iter().find(|(picture, _)| picture.as_ptr() == address) {
                return first.clone();
            }
            entries.push((Arc::downgrade(map), made.clone()));
        }
        made
    }

    /// Drops what is kept for every picture whose last `Arc` is gone.
    pub(crate) fn drop_dead(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|(picture, _)| picture.strong_count() > 0);
        }
    }

    /// Whether something is kept for `picture`. A dead `Weak` still holds
    /// its allocation, so no other picture can take its address while the
    /// caller holds it: this can be asked after the picture is gone.
    fn holds(&self, picture: &Weak<EnvMap>) -> bool {
        self.entries.lock().map_or(false, |entries| entries.iter().any(|(kept, _)| kept.as_ptr() == picture.as_ptr()))
    }
}

/// Prefix sums of the `N` numbers each texel of a picture gives: entry
/// (x, y) of the (width + 1) x (height + 1) table sums the texels left of
/// column x and above row y. It gives the exact mean of those numbers over a
/// box of any size from sixteen reads each. In f64: a 2048 x 2048 picture
/// sums four million texels, where f32 would already lose a texel's own
/// fractions. So a table is (width + 1) x (height + 1) x N x 8 bytes, and one
/// is built per picture and kind, for the first light, gobo or map that needs
/// it, and shared by every other (`PictureMemo`, D15): an image gobo's
/// (`gobo::PassTable`, one number a texel, what it passes: 33.6 MB at 2048)
/// and a light picture's softened mask (`SoftTable`, four: its premultiplied
/// colour and its alpha, 134.3 MB at 2048).
#[derive(Debug)]
pub(crate) struct SummedArea<const N: usize> {
    width: usize,
    height: usize,
    sums: Vec<[f64; N]>,
}

impl<const N: usize> SummedArea<N> {
    /// The table of `texel`'s numbers over `img`, which has texels: its width
    /// and height are above 0 and it holds that many (the callers check).
    pub(crate) fn build(img: &EnvMap, texel: impl Fn(&[f32; 4]) -> [f64; N]) -> SummedArea<N> {
        let (w, h) = (img.width, img.height);
        let stride = w + 1;
        let mut sums = vec![[0.0f64; N]; stride * (h + 1)];
        for (y, texels) in img.data.chunks_exact(w).enumerate() {
            // Row y + 1 of the table is row y plus the running sum along texel row y.
            let (above, below) = sums.split_at_mut((y + 1) * stride);
            let mut row = [0.0f64; N];
            for ((t, up), out) in texels.iter().zip(&above[y * stride + 1..]).zip(&mut below[1..stride]) {
                let numbers = texel(t);
                for k in 0..N {
                    row[k] += numbers[k];
                    out[k] = up[k] + row[k];
                }
            }
        }
        SummedArea { width: w, height: h, sums }
    }

    /// The numbers integrated over 0..x by 0..y, in texels (texel (i, j)
    /// covers i..i+1 by j..j+1), with the edge texels held past the
    /// picture's border, so that it runs on linearly outside it (negative
    /// left of and above the picture). Inside a texel the integral is
    /// exactly the bilinear blend of the table at the texel's corners, and an
    /// edge texel's blend carried on past the border is exactly the held edge.
    fn integral(&self, x: f64, y: f64) -> [f64; N] {
        let stride = self.width + 1;
        // `as usize` saturates: left of or above the picture is texel 0.
        let i = (x.floor() as usize).min(self.width - 1);
        let j = (y.floor() as usize).min(self.height - 1);
        let (tx, ty) = (x - i as f64, y - j as f64);
        let at = |i: usize, j: usize| &self.sums[j * stride + i];
        let mut out = [0.0f64; N];
        for k in 0..N {
            let top = at(i, j)[k] + (at(i + 1, j)[k] - at(i, j)[k]) * tx;
            let bottom = at(i, j + 1)[k] + (at(i + 1, j + 1)[k] - at(i, j + 1)[k]) * tx;
            out[k] = top + (bottom - top) * ty;
        }
        out
    }

    /// The mean of the numbers over the box centred at (x, y) with
    /// half-widths `rx` and `ry`, in texels as `integral` places them, the
    /// edge texels held past the border. The box is never narrower than a
    /// texel: one texel across, its mean is the bilinear lookup itself, so a
    /// box-filtered picture is never blockier than the picture.
    pub(crate) fn box_mean(&self, x: f64, y: f64, rx: f64, ry: f64) -> [f64; N] {
        let (w, h) = (self.width as f64, self.height as f64);
        let rx = rx.max(0.5);
        let ry = ry.max(0.5);
        // A box wholly past the border sees the held edge alone, as it does
        // at the border plus its half-width: holding the centre there keeps
        // the sums small. max/min hold a NaN at the near side.
        let x = x.max(-rx).min(w + rx);
        let y = y.max(-ry).min(h + ry);
        let (a, b, c, d) = (self.integral(x + rx, y + ry), self.integral(x - rx, y + ry), self.integral(x + rx, y - ry), self.integral(x - rx, y - ry));
        let mut out = [0.0f64; N];
        for k in 0..N {
            out[k] = (a[k] - b[k] - c[k] + d[k]) / (4.0 * rx * ry);
        }
        out
    }

    /// The picture's width and height, in texels.
    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn height(&self) -> usize {
        self.height
    }

    /// What the table takes in memory, in bytes.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.sums.len() * std::mem::size_of::<[f64; N]>()
    }
}

/// Every image key the params name, sorted and without repeats; "" (no
/// picture) is left out. Lights that are switched off count too: they can be
/// switched back on. So does the name on a gobo of another kind: the person
/// can switch the kind back to an image. The app loads these and holds the
/// assistant to them, so every place the params can name a light's picture
/// is listed here: image lights (B14) and image gobos (B8). The base image
/// (B15) is NOT listed: it is an equirect with its own loader, and
/// everything on this list is loaded as a light's picture.
pub fn referenced_keys(params: &HdriParams) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for light in &params.lights {
        if let Some(image) = &light.image {
            keys.push(image.key.clone());
        }
        if let Some(gobo) = &light.gobo {
            keys.push(gobo.image.clone());
        }
    }
    keys.retain(|key| !key.is_empty());
    keys.sort();
    keys.dedup();
    keys
}

/// Rec.709 luminance of a texel; NaN and negatives read as 0.
fn texel_luminance(t: &[f32; 4]) -> f32 {
    let clean = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    0.2126 * clean(t[0]) + 0.7152 * clean(t[1]) + 0.0722 * clean(t[2])
}

/// Box-shrinks a picture by a whole factor until its longest side fits
/// `max_side`, keeping its aspect (`EnvMap::resized` only makes 2:1 maps).
/// Colour is averaged weighted by alpha, so transparent texels do not bleed
/// their colour into an edge. A picture that already fits comes back as it is.
pub fn shrink_to(map: EnvMap, max_side: usize) -> EnvMap {
    let longest = map.width.max(map.height);
    let max_side = max_side.max(1);
    if longest <= max_side || map.data.len() != map.width * map.height {
        return map;
    }
    let k = longest.div_ceil(max_side);
    let (width, height) = (map.width.div_ceil(k), map.height.div_ceil(k));
    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0f64; 3];
            let mut plain = [0.0f64; 3];
            let (mut alpha, mut count) = (0.0f64, 0.0f64);
            for sy in y * k..((y + 1) * k).min(map.height) {
                for sx in x * k..((x + 1) * k).min(map.width) {
                    let t = map.data[sy * map.width + sx];
                    let a = if t[3].is_finite() { t[3].clamp(0.0, 1.0) as f64 } else { 0.0 };
                    for c in 0..3 {
                        sum[c] += t[c] as f64 * a;
                        plain[c] += t[c] as f64;
                    }
                    alpha += a;
                    count += 1.0;
                }
            }
            // A fully transparent block keeps its plain average: its colour
            // is never shown, but it must stay finite.
            let rgb = if alpha > 0.0 { sum.map(|v| v / alpha) } else { plain.map(|v| v / count) };
            data.push([rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, (alpha / count) as f32]);
        }
    }
    EnvMap { width, height, data }
}

/// A light picture's mean luminance, which normalises it: over the whole
/// picture, and over its visible part (weighted by alpha) for a light whose
/// alpha masks it. Both from one walk over its texels, made once per picture
/// and shared by every light and `Env` that shows it (D15, L:112: up to four
/// million texels, serially, before a bake's rows fan out). 0 for a picture
/// with nothing to weigh.
struct PictureMeans {
    whole: f64,
    visible: f64,
}

/// The means of every live light picture (`PictureMemo`).
static MEANS: PictureMemo<PictureMeans> = PictureMemo::new();

/// A light picture's softened mask (P1; P2's `LightImageParams::mask_softness`):
/// the summed-area table of its premultiplied texels (`premultiplied`), from
/// which a box filter of any size is exact.
type SoftTable = SummedArea<4>;

/// The softened-mask tables of every live light picture (`PictureMemo`).
static SOFT_TABLES: PictureMemo<SoftTable> = PictureMemo::new();

/// What one texel gives a softened mask's table (`SoftTable`): its colour
/// times its alpha, and its alpha, read as `LightImage::bilinear` reads a
/// masked picture (NaN and negative colours as 0, alpha held to 0..=1, a
/// NaN alpha as clear).
fn premultiplied(t: &[f32; 4]) -> [f64; 4] {
    let clean = |x: f32| if x.is_finite() && x > 0.0 { x as f64 } else { 0.0 };
    let alpha = if t[3].is_finite() { t[3].clamp(0.0, 1.0) as f64 } else { 0.0 };
    [clean(t[0]) * alpha, clean(t[1]) * alpha, clean(t[2]) * alpha, alpha]
}

/// The half-width, in texels each way, of the box Mask softness `s` (as
/// clamp() holds it) filters a `width` x `height` picture's cut-out edges
/// over: `s` x `MASK_SOFTNESS_SHARE` of its smaller side across (P2). 0 at
/// Mask softness 0: no box, the sharp cut-out.
fn mask_box_half(s: f32, width: usize, height: usize) -> f64 {
    0.5 * s as f64 * MASK_SOFTNESS_SHARE as f64 * width.min(height) as f64
}

/// Drops the means and the softened-mask tables of every picture whose last
/// `Arc` is gone (`composite::drop_dead_facts` calls this).
pub(super) fn drop_dead_light_facts() {
    MEANS.drop_dead();
    SOFT_TABLES.drop_dead();
}

/// Whether a softened-mask table is kept for `picture` (P2): a host that
/// drops a picture nothing names any more can see its table went with it,
/// once `composite::drop_dead_facts` ran. Ask with a `Weak` taken while the
/// picture lived: it still names the picture's allocation after it is gone.
pub fn keeps_soft_table(picture: &Weak<EnvMap>) -> bool {
    SOFT_TABLES.holds(picture)
}

impl PictureMeans {
    fn walk(map: &EnvMap) -> PictureMeans {
        #[cfg(test)]
        super::probe::count(super::probe::Work::PictureMean);
        let (mut sum, mut count) = (0.0f64, 0.0f64);
        let (mut visible, mut weight) = (0.0f64, 0.0f64);
        for t in &map.data {
            let alpha = if t[3].is_finite() { t[3].clamp(0.0, 1.0) as f64 } else { 0.0 };
            let luminance = texel_luminance(t) as f64;
            sum += luminance;
            count += 1.0;
            visible += luminance * alpha;
            weight += alpha;
        }
        let mean = |sum: f64, weight: f64| if weight > 0.0 { sum / weight } else { 0.0 };
        PictureMeans { whole: mean(sum, count), visible: mean(visible, weight) }
    }
}

/// One light's picture, ready to sample.
#[derive(Clone, Debug)]
pub struct LightImage {
    map: Arc<EnvMap>,
    fit: ImageFit,
    /// The picture's alpha cuts the light out.
    mask: bool,
    /// 1 / the picture's mean luminance (over its visible part when masked).
    gain: f32,
    /// Width over height.
    aspect: f32,
    /// +1 when the frame's `right` points to the viewer's right, -1 when it
    /// points left (it does: studio.rs follows ibl::softbox). Measured once
    /// rather than assumed, so a picture never comes out mirrored.
    x_sign: f32,
    /// `LightImageParams::penumbra` as clamp() holds it.
    penumbra: f32,
    /// The depth of the light the picture lies in, the scale its Softness
    /// takes (`with_depth`); None reads half the box's shorter side.
    depth: Option<f32>,
    /// The softened mask's table (`SoftTable`), shared by every light that
    /// softens this picture, and the half-width, in texels each way, of the
    /// box its Mask softness filters over (P2, `mask_box_half`).
    soft: Option<Arc<SoftTable>>,
    soft_half: f64,
}

impl LightImage {
    /// The picture laid in a light whose Softness takes `depth` (tangent
    /// units; `ShapedLight::depth` for a shaped light): in stretch and fill
    /// its Penumbra's band is that share of it.
    pub(super) fn with_depth(self, depth: f32) -> LightImage {
        LightImage { depth: Some(depth), ..self }
    }

    /// How far the light's own soft edge widens inward, as a share of its
    /// depth (P1): Penumbra in stretch and fill, where the picture covers
    /// the light and its border is the light's edge; 0 in fit, where the
    /// picture's border is its own and `shade` fades it.
    pub(super) fn edge_inset(&self) -> f32 {
        match self.fit {
            ImageFit::Fit => 0.0,
            ImageFit::Stretch | ImageFit::Fill => self.penumbra,
        }
    }

    /// The picture `params` names, or None when the key is empty, not in
    /// `images`, or the picture is empty: the light is then drawn plain.
    /// The block is read as clamp() holds it (D17). A picture whose alpha
    /// masks it and whose Mask softness is above 0 (P2) shares its softened
    /// mask's table with every light that softens it.
    pub fn new(params: &LightImageParams, images: &ImageSet) -> Option<LightImage> {
        if params.key.is_empty() {
            return None;
        }
        let map = images.get(&params.key)?.clone();
        if map.width == 0 || map.height == 0 || map.data.len() != map.width * map.height {
            return None;
        }
        let params = &params.clamped();
        let mask = params.alpha_as_mask;
        let means = MEANS.get(&map, PictureMeans::walk);
        let mean = if mask { means.visible } else { means.whole };
        let penumbra = params.penumbra();
        // Only a mask cuts edges out, and a box of 0 softens nothing.
        let soft_half = mask_box_half(params.mask_softness(), map.width, map.height);
        let soft = (mask && soft_half > 0.0).then(|| {
            SOFT_TABLES.get(&map, |map| {
                #[cfg(test)]
                super::probe::count(super::probe::Work::SoftMask);
                SummedArea::build(map, premultiplied)
            })
        });
        Some(LightImage {
            fit: params.fit(),
            mask,
            // A black (or fully transparent) picture emits nothing.
            gain: if mean > 1.0e-9 { (1.0 / mean) as f32 } else { 0.0 },
            aspect: map.width as f32 / map.height as f32,
            // A light due north on the horizon: the viewer's right is east, +X.
            x_sign: if light_frame(0.0, 0.0, 0.0).right.x >= 0.0 { 1.0 } else { -1.0 },
            penumbra,
            depth: None,
            soft,
            soft_half,
            map,
        })
    }

    /// The picture at `p`, a point of the light's tangent plane as
    /// `studio::project` gives it, for a light of half extents `half`
    /// (tan(width/2), tan(height/2)).
    ///
    /// Returns the colour multiplier (mean luminance 1 over the picture) and
    /// the alpha, 0..=1. Alpha is 0 outside a letterboxed picture and where a
    /// masked picture is transparent; without the mask it is 1 on the picture.
    /// Past the light's box the picture's edge texels repeat, so a soft edge
    /// carries the edge colour.
    ///
    /// P1: in fit the alpha fades out over the band inside the letterboxed
    /// picture's border (`LightImageParams::penumbra`), along each of its
    /// axes; in stretch and fill the light's own edge widens instead
    /// (`edge_inset`). P2: a softened mask is read through its box filter,
    /// Mask softness's share of the picture's smaller side across. At
    /// Penumbra 0 and Mask softness 0 this is the lookup as it was, bit for
    /// bit.
    pub fn shade(&self, p: Vec2f, half: Vec2f) -> (Vec3f, f32) {
        // The box as the viewer sees it: u to the right, v down (row 0 on top).
        let mut u = 0.5 + 0.5 * self.x_sign * p.x / half.x.max(1.0e-6);
        let mut v = 0.5 - 0.5 * p.y / half.y.max(1.0e-6);
        // How much wider than the box the picture is.
        let wide = self.aspect / (half.x / half.y.max(1.0e-6)).max(1.0e-6);
        let mut fade = 1.0;
        match self.fit {
            ImageFit::Stretch => {}
            ImageFit::Fit => {
                // The whole picture inside the box; the rest of the box is dark.
                if wide > 1.0 {
                    v = 0.5 + (v - 0.5) * wide;
                } else {
                    u = 0.5 + (u - 0.5) / wide;
                }
                if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                    return (Vec3f::default(), 0.0);
                }
                if self.penumbra > 0.0 {
                    // The picture's border is its own: it fades out inside
                    // it, to 0 at it, smoothly along each axis (the product
                    // rounds the corners, as a blurred rectangle's).
                    let laid = self.laid(half, wide);
                    let band = self.band(half, laid);
                    fade = smoothstep(0.0, band, laid.x - p.x.abs()) * smoothstep(0.0, band, laid.y - p.y.abs());
                }
            }
            ImageFit::Fill => {
                // The box covered; the picture's overhang is cropped.
                if wide > 1.0 {
                    u = 0.5 + (u - 0.5) / wide;
                } else {
                    v = 0.5 + (v - 0.5) * wide;
                }
            }
        }
        // Clamp to the texel centres of the border: a picture must not bleed
        // its left edge into its right (`EnvMap::sample_uv` wraps u, it is
        // made for equirects).
        let (w, h) = (self.map.width as f32, self.map.height as f32);
        let u = u.max(0.5 / w).min(1.0 - 0.5 / w);
        let v = v.max(0.5 / h).min(1.0 - 0.5 / h);
        let (rgb, alpha) = match &self.soft {
            Some(table) => self.softened(table, u, v),
            None => self.bilinear(u, v),
        };
        (rgb * self.gain, alpha * fade)
    }

    /// The whole picture's half extents in the tangent plane of a box of
    /// half extents `half`, as its fit lays it (`wide`: how much wider than
    /// the box the picture is): the box itself in stretch; in fit the box's
    /// width or height and the other side by the picture's aspect, inside
    /// the box; in fill the other way round, past it.
    fn laid(&self, half: Vec2f, wide: f32) -> Vec2f {
        let half = vec2f(half.x.max(1.0e-6), half.y.max(1.0e-6));
        match (self.fit, wide > 1.0) {
            (ImageFit::Stretch, _) => half,
            (ImageFit::Fit, true) | (ImageFit::Fill, false) => vec2f(half.x, half.y / wide),
            (ImageFit::Fit, false) | (ImageFit::Fill, true) => vec2f(half.x * wide, half.y),
        }
    }

    /// The band, tangent units, the picture's border fades over: Penumbra x
    /// the picture's depth.
    /// In fit that is its inradius, half the letterboxed picture's shorter
    /// side (`laid`); in stretch and fill, where the picture covers the light,
    /// the light's own depth (`with_depth`; half the box's shorter side when
    /// none was given).
    fn band(&self, half: Vec2f, laid: Vec2f) -> f32 {
        match self.fit {
            ImageFit::Fit => self.penumbra * laid.x.min(laid.y),
            ImageFit::Stretch | ImageFit::Fill => self.penumbra * self.depth.unwrap_or(half.x.min(half.y)),
        }
    }

    /// The softened mask at (u, v), inside the texel centres: the mean of
    /// the premultiplied picture (`premultiplied`) over the box Mask
    /// softness spans round it (`soft_half` texels each way, `mask_box_half`),
    /// its colour divided back out of its alpha. A hard cut-out edge so
    /// becomes a ramp the box wide, centred on it, and the colour fades with
    /// the alpha: a transparent texel's colour never shows, so no dark
    /// fringe. The edge texels are held past the picture's border, as the
    /// bilinear lookup holds them.
    fn softened(&self, table: &SoftTable, u: f32, v: f32) -> (Vec3f, f32) {
        let (w, h) = (self.map.width as f64, self.map.height as f64);
        let m = table.box_mean(u as f64 * w, v as f64 * h, self.soft_half, self.soft_half);
        // Below a billionth the box holds no picture, whatever the sums' rounding says.
        if !(m[3] > 1.0e-9) {
            return (Vec3f::default(), 0.0);
        }
        let colour = |c: f64| (c / m[3]).max(0.0) as f32;
        (vec3f(colour(m[0]), colour(m[1]), colour(m[2])), (m[3] as f32).min(1.0))
    }

    /// The bilinear lookup at (u, v), inside the texel centres: the colour
    /// weighted by alpha when the picture masks (premultiplied, then divided
    /// out), so a transparent texel's colour, which a PNG keeps, never
    /// fringes an edge; NaN and negatives read as 0, a NaN alpha as clear.
    fn bilinear(&self, u: f32, v: f32) -> (Vec3f, f32) {
        let (width, height) = (self.map.width, self.map.height);
        let fx = (u * width as f32 - 0.5).max(0.0);
        let fy = (v * height as f32 - 0.5).max(0.0);
        let (x0, y0) = ((fx as usize).min(width - 1), (fy as usize).min(height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
        let clean = |x: f32| if x.is_finite() && x > 0.0 { x } else { 0.0 };
        let (mut rgb, mut alpha) = (Vec3f::default(), 0.0);
        for (x, y, weight) in [(x0, y0, (1.0 - tx) * (1.0 - ty)), (x1, y0, tx * (1.0 - ty)), (x0, y1, (1.0 - tx) * ty), (x1, y1, tx * ty)] {
            let t = self.map.data[y * width + x];
            let a = if !self.mask {
                1.0
            } else if t[3].is_finite() {
                t[3].clamp(0.0, 1.0)
            } else {
                0.0
            };
            rgb += vec3f(clean(t[0]), clean(t[1]), clean(t[2])) * (a * weight);
            alpha += a * weight;
        }
        if !(alpha > 0.0) {
            return (Vec3f::default(), 0.0);
        }
        // Without the mask the weights sum to 1 but for rounding; the alpha
        // is exactly 1 there.
        (rgb / alpha, if self.mask { alpha.min(1.0) } else { 1.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::studio::{light_color, project};
    use crate::hdri::{dir_from_az_el, Env, GoboParams, LightParams, IMAGE_FIT_NAMES};
    use crate::sky::luminance;

    fn picture(width: usize, height: usize, texel: impl Fn(usize, usize) -> [f32; 4]) -> EnvMap {
        let data = (0..width * height).map(|i| texel(i % width, i / width)).collect();
        EnvMap { width, height, data }
    }

    fn set_of(key: &str, map: EnvMap) -> ImageSet {
        let mut set = ImageSet::default();
        set.insert(key, map);
        set
    }

    /// A hard-edged 40 x 40 degree rect due east on the horizon, showing `key`.
    fn image_light(key: &str) -> LightParams {
        LightParams {
            azimuth_deg: 90.0,
            elevation_deg: 0.0,
            width_deg: 40.0,
            height_deg: 40.0,
            corner: 0.0,
            softness: 0.0,
            intensity_ev: 2.0,
            image: Some(LightImageParams { key: key.to_string(), ..LightImageParams::default() }),
            ..LightParams::default()
        }
    }

    fn dark_studio(lights: Vec<LightParams>) -> HdriParams {
        let mut p = HdriParams::default();
        p.mode = "studio".to_string();
        p.studio.top = [0.0; 3];
        p.studio.horizon = [0.0; 3];
        p.studio.floor = [0.0; 3];
        p.lights = lights;
        p
    }

    /// The direction of the point (u, v) of a light's box as the viewer sees
    /// it: u to the right, v down, both 0..1.
    fn box_dir(light: &LightParams, u: f32, v: f32) -> Vec3f {
        let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
        let sx = if light_frame(0.0, 0.0, 0.0).right.x >= 0.0 { 1.0 } else { -1.0 };
        let hx = (0.5 * light.width_deg).to_radians().tan();
        let hy = (0.5 * light.height_deg).to_radians().tan();
        (f.center + f.right * (sx * (2.0 * u - 1.0) * hx) + f.up * ((1.0 - 2.0 * v) * hy)).normalize()
    }

    /// D15 (L:112): a picture's mean is walked once, not by every light and
    /// every `Env` that shows it (a 2048 x 2048 picture is four million
    /// texels, walked serially before the rows fan out). Two lights showing
    /// the same picture, one masked by its alpha, and a held drag's next
    /// `Env` walk its texels once; each light's gain is still 1 over its own
    /// mean (the whole picture's, or its visible part's), to the bit.
    #[test]
    fn a_pictures_mean_is_walked_once_for_every_light_and_env() {
        use crate::hdri::probe::{counted, Work};
        let map = picture(8, 4, |x, y| [0.1 + 0.2 * x as f32, 0.3 * y as f32, 1.0, if x < 3 { 0.0 } else { 1.0 }]);
        let images = set_of("lamp", map.clone());
        let plain = image_light("lamp");
        let masked = LightParams {
            azimuth_deg: 200.0,
            image: Some(LightImageParams { key: "lamp".to_string(), alpha_as_mask: true, ..LightImageParams::default() }),
            ..image_light("lamp")
        };
        let mut params = dark_studio(vec![plain.clone(), masked.clone()]);
        let before = counted(Work::PictureMean);
        let _ = Env::new_with(&params, &images);
        params.lights[0].azimuth_deg = 91.0;
        let _ = Env::new_with(&params, &images);
        assert_eq!(counted(Work::PictureMean), before + 1, "two lights and two maps walk the picture once");
        // The means, as the light worked them out before they were kept.
        let mean = |mask: bool| {
            let (mut sum, mut weight) = (0.0f64, 0.0f64);
            for t in &map.data {
                let w = if mask { t[3] as f64 } else { 1.0 };
                sum += texel_luminance(t) as f64 * w;
                weight += w;
            }
            (1.0 / (sum / weight)) as f32
        };
        let gain = |light: &LightParams| LightImage::new(light.image.as_ref().unwrap(), &images).expect("loaded").gain;
        assert_eq!(gain(&plain), mean(false));
        assert_eq!(gain(&masked), mean(true));
        assert_ne!(mean(false), mean(true), "the visible part has a mean of its own");
        assert_eq!(counted(Work::PictureMean), before + 1);
    }

    #[test]
    fn an_image_light_keeps_its_level() {
        // A HDR picture: a hot texel in a dim field.
        let map = picture(8, 4, |x, y| if (x, y) == (5, 1) { [40.0, 30.0, 10.0, 1.0] } else { [0.2 + 0.05 * x as f32, 0.3, 0.1 * y as f32, 1.0] });
        let light = image_light("lamp");
        let env = Env::new_with(&dark_studio(vec![light.clone()]), &set_of("lamp", map));
        // At the picture's texel centres the bilinear lookup is the texel
        // itself (to f32 rounding of the direction, which the hot texel
        // magnifies), so their mean is the picture's mean: luminance 1 times
        // the light's colour.
        let mut sum = 0.0;
        for y in 0..4 {
            for x in 0..8 {
                sum += luminance(env.radiance(box_dir(&light, (x as f32 + 0.5) / 8.0, (y as f32 + 0.5) / 4.0)));
            }
        }
        let mean = sum / 32.0;
        let want = luminance(light_color(&light));
        assert!((want - 4.0).abs() < 1.0e-3, "6500 K at +2 EV: {want}");
        assert!((mean / want - 1.0).abs() < 0.01, "the light keeps 2^intensity_ev: {mean} vs {want}");
        // The hot texel keeps its ratio to the field: this is not a tone-mapped picture.
        let hot = luminance(env.radiance(box_dir(&light, 5.5 / 8.0, 1.5 / 4.0)));
        let dim = luminance(env.radiance(box_dir(&light, 0.5 / 8.0, 0.5 / 4.0)));
        assert!(hot > 50.0 * dim, "{hot} vs {dim}");
    }

    #[test]
    fn the_picture_is_upright_and_not_mirrored() {
        // Red top left, green top right, blue bottom left, white bottom right.
        let map = picture(2, 2, |x, y| match (x, y) {
            (0, 0) => [1.0, 0.0, 0.0, 1.0],
            (1, 0) => [0.0, 1.0, 0.0, 1.0],
            (0, 1) => [0.0, 0.0, 1.0, 1.0],
            _ => [1.0, 1.0, 1.0, 1.0],
        });
        let mut light = image_light("card");
        light.rgb = Some([1.0; 3]);
        let env = Env::new_with(&dark_studio(vec![light]), &set_of("card", map));
        // Facing east, north (a lower azimuth) is on the left.
        let top_left = env.radiance(dir_from_az_el(75.0, 12.0));
        let top_right = env.radiance(dir_from_az_el(105.0, 12.0));
        let bottom_left = env.radiance(dir_from_az_el(75.0, -12.0));
        let bottom_right = env.radiance(dir_from_az_el(105.0, -12.0));
        assert!(top_left.x > 0.0 && top_left.y == 0.0 && top_left.z == 0.0, "{top_left:?}");
        assert!(top_right.y > 0.0 && top_right.x == 0.0 && top_right.z == 0.0, "{top_right:?}");
        assert!(bottom_left.z > 0.0 && bottom_left.x == 0.0 && bottom_left.y == 0.0, "{bottom_left:?}");
        assert!(bottom_right.x > 0.0 && bottom_right.x == bottom_right.y && bottom_right.y == bottom_right.z, "{bottom_right:?}");
        // Outside the light nothing is drawn.
        assert_eq!(env.radiance(dir_from_az_el(90.0, 40.0)), Vec3f::default());
        assert_eq!(env.radiance(dir_from_az_el(270.0, 0.0)), Vec3f::default());
    }

    #[test]
    fn fit_letterboxes_and_fill_crops() {
        // A 4:1 strip on a square box: texel x is x + 1 in the red channel.
        let strip = set_of("strip", picture(4, 1, |x, _| [x as f32 + 1.0, 0.0, 0.0, 1.0]));
        let image = |fit: &str| {
            LightImage::new(&LightImageParams { key: "strip".to_string(), fit: fit.to_string(), alpha_as_mask: false, ..LightImageParams::default() }, &strip).unwrap()
        };
        let half = vec2f(0.5, 0.5);
        // Points of the box by what the viewer sees: x to the right, y up, -1..1.
        let sx = if light_frame(0.0, 0.0, 0.0).right.x >= 0.0 { 1.0 } else { -1.0 };
        let at = |image: &LightImage, x: f32, y: f32| image.shade(vec2f(sx * x * 0.5, y * 0.5), half);
        // The mean of 1, 2, 3, 4 is 2.5 in red: luminance 0.2126 * 2.5.
        let unit = 1.0 / (0.2126 * 2.5);
        let (stretch, fit, fill) = (image("stretch"), image("fit"), image("fill"));
        // Stretch: the whole strip across the box, at any height.
        assert!((at(&stretch, -0.75, 0.9).0.x - unit).abs() < 1.0e-5);
        assert!((at(&stretch, 0.75, -0.9).0.x - 4.0 * unit).abs() < 1.0e-5);
        assert_eq!(at(&stretch, 0.0, 0.9).1, 1.0);
        // Fit: the strip keeps its shape, a quarter of the box high; above and below it the box is dark.
        assert!((at(&fit, -0.75, 0.0).0.x - unit).abs() < 1.0e-5);
        assert_eq!(at(&fit, -0.75, 0.2).1, 1.0);
        assert_eq!(at(&fit, -0.75, 0.3), (Vec3f::default(), 0.0));
        assert_eq!(at(&fit, 0.0, -0.9).1, 0.0);
        // Fill: the box is covered by the strip's middle quarter (between texels 2 and 3).
        let left = at(&fill, -1.0, 0.9).0.x / unit;
        let right = at(&fill, 1.0, -0.9).0.x / unit;
        assert!((left - 2.0).abs() < 1.0e-4 && (right - 3.0).abs() < 1.0e-4, "{left} .. {right}");
        // Past the box the edge texel repeats (a soft edge keeps the edge colour).
        assert!((at(&stretch, -1.4, 0.0).0.x - unit).abs() < 1.0e-5);
        assert!((at(&stretch, 1.4, 0.0).0.x - 4.0 * unit).abs() < 1.0e-5);
    }

    #[test]
    fn alpha_cuts_the_light_out() {
        // Left half opaque white, right half transparent (with a bright colour that must not show).
        let cutout = || picture(2, 1, |x, _| if x == 0 { [1.0, 1.0, 1.0, 1.0] } else { [9.0, 9.0, 9.0, 0.0] });
        let mut light = image_light("cutout");
        light.rgb = Some([1.0; 3]);
        light.intensity_ev = 0.0;
        light.image.as_mut().unwrap().alpha_as_mask = true;
        let mut p = dark_studio(vec![light.clone()]);
        p.studio.top = [0.25; 3];
        p.studio.horizon = [0.25; 3];
        p.studio.floor = [0.25; 3];
        let env = Env::new_with(&p, &set_of("cutout", cutout()));
        let (left, right) = (dir_from_az_el(78.0, 0.0), dir_from_az_el(102.0, 0.0));
        // The visible part alone sets the level: white at luminance 1 over the backdrop.
        assert!((env.radiance(left) - Vec3f::all(1.25)).length() < 1.0e-5, "{:?}", env.radiance(left));
        assert_eq!(env.radiance(right), Vec3f::all(0.25), "the transparent half leaves the backdrop alone");
        // Without the mask the whole picture shines, the bright half included.
        p.lights[0].image.as_mut().unwrap().alpha_as_mask = false;
        let open = Env::new_with(&p, &set_of("cutout", cutout()));
        assert!(open.radiance(right).x > open.radiance(left).x);
        // A Multiply picture is a cut-out flag: black where it is opaque, nothing where it is clear.
        p.lights[0].image.as_mut().unwrap().alpha_as_mask = true;
        p.lights[0].blend = "multiply".to_string();
        p.lights[0].rgb = Some([0.0; 3]);
        let flag = Env::new_with(&p, &set_of("cutout", cutout()));
        assert_eq!(flag.radiance(left), Vec3f::default());
        assert_eq!(flag.radiance(right), Vec3f::all(0.25));
    }

    #[test]
    fn a_transparent_texels_colour_never_shows_at_an_edge() {
        // Opaque white beside a transparent texel whose (straight) colour is
        // magenta, as a PNG keeps it: the magenta must not fringe the edge.
        let map = picture(2, 1, |x, _| if x == 0 { [1.0, 1.0, 1.0, 1.0] } else { [1.0, 0.0, 1.0, 0.0] });
        let params = LightImageParams { key: "edge".to_string(), fit: "stretch".to_string(), alpha_as_mask: true, ..LightImageParams::default() };
        let image = LightImage::new(&params, &set_of("edge", map)).unwrap();
        // Halfway between the two texel centres: half covered, and white.
        let (colour, alpha) = image.shade(vec2f(0.0, 0.0), vec2f(0.5, 0.5));
        assert!((alpha - 0.5).abs() < 1.0e-6, "{alpha}");
        assert!((colour - Vec3f::all(1.0)).length() < 1.0e-6, "{colour:?}");
    }

    #[test]
    fn a_missing_picture_draws_the_plain_light() {
        let light = image_light("gone.png");
        let plain = LightParams { image: None, ..light.clone() };
        let with = Env::new(&dark_studio(vec![light.clone()]));
        let without = Env::new(&dark_studio(vec![plain]));
        assert_eq!(with.bake(32), without.bake(32));
        // Env::new is Env::new_with over an empty set.
        let empty = Env::new_with(&dark_studio(vec![light.clone()]), &ImageSet::default());
        assert_eq!(empty.bake(32), with.bake(32));
        // An empty key, an empty picture and a black picture are no picture either.
        let none = LightImageParams::default();
        assert!(LightImage::new(&none, &set_of("", picture(1, 1, |_, _| [1.0; 4]))).is_none());
        let broken = EnvMap { width: 2, height: 2, data: Vec::new() };
        assert!(LightImage::new(light.image.as_ref().unwrap(), &set_of("gone.png", broken)).is_none());
        let black = LightImage::new(light.image.as_ref().unwrap(), &set_of("gone.png", picture(2, 2, |_, _| [0.0, 0.0, 0.0, 1.0]))).unwrap();
        assert_eq!(black.shade(vec2f(0.0, 0.0), vec2f(0.3, 0.3)).0, Vec3f::default(), "a black picture emits nothing, and no NaN");
        // An Env with pictures still crosses threads.
        fn shareable<T: Send + Sync>() {}
        shareable::<Env>();
        shareable::<ImageSet>();
    }

    /// D3 (L:119): a picture on a stroke placed by its points is laid in the
    /// box round the stroke, as its gobo, aim and wear are: the light's own
    /// azimuth no longer places it, and a stroke far from that azimuth is
    /// drawn all along, not cut where the light's own plane ends.
    #[test]
    fn a_picture_on_a_stroke_follows_the_stroke() {
        // A grey ramp from left to right, so the picture's place on the stroke shows.
        let set = set_of("ramp", picture(8, 2, |x, _| [0.05 + 0.3 * x as f32, 0.05 + 0.3 * x as f32, 0.05 + 0.3 * x as f32, 1.0]));
        // Along the horizon 10 degrees up, from azimuth 80 to 160.
        let stroke = |azimuth_deg: f32| LightParams {
            shape: "stroke".to_string(),
            azimuth_deg,
            stroke: Some(crate::hdri::StrokeParams { points: vec![[80.0, 10.0], [120.0, 10.0], [160.0, 10.0]], width_deg: 4.0, ..Default::default() }),
            ..image_light("ramp")
        };
        let draw = |azimuth_deg: f32| {
            let env = Env::new_with(&dark_studio(vec![stroke(azimuth_deg)]), &set);
            (0..=80).map(|i| luminance(env.radiance(dir_from_az_el(80.0 + i as f32, 10.0)))).collect::<Vec<f32>>()
        };
        let (far, near) = (draw(0.0), draw(120.0));
        assert_eq!(far, near, "the light's own azimuth places nothing");
        let lit = far.iter().filter(|l| **l > 0.0).count();
        assert!(lit > 75, "the stroke is drawn all along: {lit} of {} samples", far.len());
        // The ramp runs along the stroke from its left end to its right.
        assert!(far[70] > 1.5 * far[10], "{} against {}", far[70], far[10]);
    }

    #[test]
    fn shrink_keeps_the_aspect_and_weights_by_alpha() {
        let map = picture(4, 2, |x, y| [x as f32, y as f32, 1.0, 1.0]);
        assert_eq!(shrink_to(map.clone(), 4), map, "a picture that fits is left alone");
        let small = shrink_to(map.clone(), 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.data, vec![[0.5, 0.5, 1.0, 1.0], [2.5, 0.5, 1.0, 1.0]]);
        // 5 x 3 at a factor of 2 keeps its ragged edge: 3 x 2.
        let odd = shrink_to(picture(5, 3, |_, _| [1.0, 1.0, 1.0, 1.0]), 3);
        assert_eq!((odd.width, odd.height), (3, 2));
        assert!(odd.data.iter().all(|t| *t == [1.0, 1.0, 1.0, 1.0]));
        // A transparent texel's colour does not bleed into its block.
        let edge = shrink_to(picture(2, 1, |x, _| if x == 0 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 9.0, 0.0, 0.0] }), 1);
        assert_eq!(edge.data, vec![[1.0, 0.0, 0.0, 0.5]]);
    }

    #[test]
    fn referenced_keys_lists_each_picture_once() {
        let mut p = dark_studio(vec![image_light("b.png"), image_light("a.png"), image_light("b.png"), image_light("")]);
        p.lights[1].enabled = false;
        p.lights.push(LightParams::default());
        assert_eq!(referenced_keys(&p), ["a.png", "b.png"]);
        assert!(referenced_keys(&HdriParams::default()).is_empty());
        let mut set = set_of("b.png", picture(1, 1, |_, _| [1.0; 4]));
        set.insert("a.png", picture(1, 1, |_, _| [1.0; 4]));
        assert_eq!(set.keys(), ["a.png", "b.png"]);
        assert!(set.contains("a.png") && !set.contains("c.png"));
        // The light's frame puts `project` and the picture in the same plane.
        let light = image_light("a.png");
        let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
        assert!(project(&f, box_dir(&light, 0.5, 0.5)).unwrap().length() < 1.0e-6);
    }

    /// The image block of picture `key` shown `fit`, with a penumbra.
    fn soft_image(key: &str, fit: &str, penumbra: f32) -> LightImageParams {
        LightImageParams { key: key.to_string(), fit: fit.to_string(), penumbra: Some(penumbra), ..LightImageParams::default() }
    }

    /// P1, P2: a picture without a penumbra and without a mask softness, or
    /// with either at 0 (or held to 0: -1, NaN), draws exactly as before P1,
    /// in every fit, masked or not, and so does a mask softness on a picture
    /// whose alpha does not cut it out (it has no cut-out edges to soften):
    /// the map, the key and the keyless copy, bit for bit. Against the code
    /// before P1 by a throwaway probe (P1's report); here against the block
    /// without the fields.
    #[test]
    fn no_penumbra_and_no_mask_softness_draw_the_picture_as_before() {
        let images = set_of("logo", picture(12, 6, |x, y| [0.2 + 0.1 * x as f32, 0.5, 0.1 * y as f32, if (x + 2 * y) % 5 == 0 { 0.0 } else { 1.0 }]));
        let fingerprint = |image: LightImageParams| {
            let light = LightParams { key: true, softness: 0.3, image: Some(image), ..image_light("logo") };
            let env = Env::new_with(&dark_studio(vec![light]), &images);
            let key = env.sun().expect("a key");
            (env.bake(48).data, env.bake_without_key(48).expect("a keyless copy").data, [key.radiance.x, key.radiance.y, key.radiance.z, key.facing, key.cos_cover])
        };
        for &fit in IMAGE_FIT_NAMES {
            for alpha_as_mask in [false, true] {
                let bare = LightImageParams { key: "logo".to_string(), fit: fit.to_string(), alpha_as_mask, ..LightImageParams::default() };
                let want = fingerprint(bare.clone());
                let mut variants = vec![(Some(0.0), None), (None, Some(0.0)), (Some(0.0), Some(0.0)), (Some(-1.0), Some(-1.0)), (None, Some(f32::NAN))];
                if !alpha_as_mask {
                    variants.push((None, Some(0.5)));
                }
                for (penumbra, mask_softness) in variants {
                    let got = fingerprint(LightImageParams { penumbra, mask_softness, ..bare.clone() });
                    assert!(got == want, "{fit}, mask {alpha_as_mask}, penumbra {penumbra:?}, mask softness {mask_softness:?}: the drawing moved");
                }
            }
        }
    }

    /// P1 (the owner's "5G" logo): in fit, Penumbra fades the picture out
    /// over a band inside its own border, smoothly and to 0 at the border,
    /// Penumbra x its inradius (half its shorter side) wide along both of
    /// its axes. Penumbra 1 fades right to its centre, and only the border
    /// fades: the colour is the sharp picture's.
    #[test]
    fn penumbra_fades_a_fitted_picture_inside_its_border() {
        // A 4:1 strip in a square box: the box's width and a quarter of its
        // height, so its border runs at y = 0.125 and x = 0.5.
        let strip = set_of("strip", picture(8, 2, |x, _| [1.0 + 0.25 * x as f32, 0.5, 0.25, 1.0]));
        let image = |penumbra: f32| LightImage::new(&soft_image("strip", "fit", penumbra), &strip).unwrap();
        let half = vec2f(0.5, 0.5);
        let (across, along) = (0.125f32, 0.5f32);
        let sharp = image(0.0);
        for penumbra in [0.3f32, 1.0] {
            let soft = image(penumbra);
            let band = penumbra * across;
            let mut last: Option<f32> = None;
            // Up the strip's middle from its centre, out past its border.
            for i in 0..=500 {
                let y = 0.2 * i as f32 / 500.0;
                let p = vec2f(0.0, y);
                let (colour, alpha) = soft.shade(p, half);
                let want = smoothstep(0.0, band, across - y);
                assert!((alpha - want).abs() < 1.0e-5, "penumbra {penumbra} at y {y}: alpha {alpha}, the band says {want}");
                if let Some(last) = last {
                    assert!(alpha <= last + 1.0e-6, "penumbra {penumbra}: the fade rises outward at y {y}: {last} then {alpha}");
                    assert!(last - alpha < 0.02, "penumbra {penumbra}: a step at y {y}: {last} then {alpha}");
                }
                last = Some(alpha);
                if y >= across {
                    assert_eq!(alpha, 0.0, "penumbra {penumbra}: at and past the border nothing, y {y}");
                } else {
                    assert!((colour - sharp.shade(p, half).0).length() < 1.0e-6, "penumbra {penumbra}: only the border fades, y {y}");
                }
            }
            // Along its length the band is as wide: Penumbra x the shorter side.
            for i in 0..=200 {
                let x = along * i as f32 / 200.0;
                let alpha = soft.shade(vec2f(x, 0.0), half).1;
                let want = smoothstep(0.0, band, along - x);
                assert!((alpha - want).abs() < 1.0e-5, "penumbra {penumbra} at x {x}: alpha {alpha}, the band says {want}");
            }
        }
        // Penumbra 1 fades right to the centre: full there, below full anywhere else across.
        let full = image(1.0);
        assert_eq!(full.shade(vec2f(0.0, 0.0), half).1, 1.0);
        assert!(full.shade(vec2f(0.0, 0.002), half).1 < 1.0);
        // The other letterbox: a 1:4 picture, its sides inside the box.
        let post = set_of("post", picture(2, 8, |_, y| [1.0, 0.1 * y as f32, 0.5, 1.0]));
        let soft = LightImage::new(&soft_image("post", "fit", 0.5), &post).unwrap();
        for i in 0..=200 {
            let x = 0.2 * i as f32 / 200.0;
            let alpha = soft.shade(vec2f(x, 0.0), half).1;
            let want = smoothstep(0.0, 0.5 * 0.125, 0.125 - x);
            assert!((alpha - want).abs() < 1.0e-5, "a post at x {x}: alpha {alpha}, the band says {want}");
        }
    }

    /// P1: the map draws the fitted picture's fade: its light is the sharp
    /// picture's times the fade (the light's own Softness included, at the
    /// sides where the picture meets the box), and nothing past the picture
    /// is lit.
    #[test]
    fn a_fitted_pictures_penumbra_is_what_the_map_draws() {
        let strip = set_of("strip", picture(8, 2, |x, _| [0.2 + 0.1 * x as f32, 1.0, 0.5, 1.0]));
        let light = |penumbra: f32| LightParams { softness: 0.3, image: Some(soft_image("strip", "fit", penumbra)), ..image_light("strip") };
        let sharp = Env::new_with(&dark_studio(vec![light(0.0)]), &strip);
        let soft = Env::new_with(&dark_studio(vec![light(0.4)]), &strip);
        let l = light(0.4);
        let f = light_frame(l.azimuth_deg, l.elevation_deg, l.roll_deg);
        // The 40 degree square box; the strip a quarter of its height.
        let half = (0.5 * l.width_deg).to_radians().tan();
        let (across, along) = (half / 4.0, half);
        let band = 0.4 * across;
        let fade = |p: Vec2f| smoothstep(0.0, band, across - p.y.abs()) * smoothstep(0.0, band, along - p.x.abs());
        let mut lit = 0;
        for j in 0..=120 {
            for i in 0..=120 {
                let d = box_dir(&l, -0.1 + 1.2 * i as f32 / 120.0, 0.3 + 0.4 * j as f32 / 120.0);
                let p = project(&f, d).unwrap();
                let (a, b) = (sharp.radiance(d), soft.radiance(d));
                // Past the border (by more than f32 places a direction on it).
                if p.x.abs() > along * (1.0 + 1.0e-5) || p.y.abs() > across * (1.0 + 1.0e-5) {
                    assert_eq!(b, Vec3f::default(), "past the picture nothing is lit, at {p:?}");
                    continue;
                }
                assert!((b - a * fade(p)).length() <= 1.0e-5 * a.length().max(1.0), "at {p:?}: {b:?}, the sharp light {a:?} x {}", fade(p));
                lit += (b.max_elem() > 0.0) as usize;
            }
        }
        assert!(lit > 2000, "{lit} lit samples");
    }

    /// P1: in stretch and fill the picture covers the light, so its border
    /// is the light's own edge, where Softness already ramps. Penumbra widens
    /// that one ramp inward by Penumbra x the light's depth (half a rect's
    /// shorter side), its outer end staying where Softness puts it: the
    /// bands add into one smoothstep from (Softness / 2 + Penumbra) x depth
    /// inside to Softness / 2 x depth outside, not two falloffs multiplied.
    /// At Penumbra 0 the edge is Softness's alone; at Softness 0 it fades
    /// inside the border only.
    #[test]
    fn in_stretch_and_fill_penumbra_widens_the_lights_own_edge() {
        let flat = set_of("flat", picture(6, 4, |_, _| [1.0, 1.0, 1.0, 1.0]));
        let (hx, hy) = (25f32.to_radians().tan(), 15f32.to_radians().tan());
        for fit in ["stretch", "fill"] {
            for (softness, penumbra) in [(0.0f32, 0.4f32), (0.3, 0.0), (0.3, 0.4), (0.6, 1.0)] {
                let light = LightParams {
                    width_deg: 50.0,
                    height_deg: 30.0,
                    softness,
                    rgb: Some([1.0; 3]),
                    intensity_ev: 0.0,
                    image: Some(soft_image("flat", fit, penumbra)),
                    ..image_light("flat")
                };
                let env = Env::new_with(&dark_studio(vec![light.clone()]), &flat);
                let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
                let a = 0.5 * (softness * hy).max(1.0e-4);
                let b = penumbra * hy;
                // Across the light's middle, from its centre out past its reach.
                for i in 0..=400 {
                    let x = 1.3 * (hx + a) * i as f32 / 400.0;
                    let d = (f.center + f.right * x).normalize();
                    // The rect's signed distance along its middle: the side's, or the deepest point's.
                    let distance = (x - hx).max(-hy);
                    let want = 1.0 - smoothstep(-(a + b), a, distance);
                    let got = luminance(env.radiance(d));
                    assert!((got - want).abs() < 2.0e-4, "{fit}, softness {softness}, penumbra {penumbra}, at x {x}: {got}, one ramp says {want}");
                }
            }
        }
    }

    /// P1: every shape's own edge widens, in stretch and fill, by the one
    /// ramp a rect's does (`in_stretch_and_fill_penumbra_widens_the_lights_own_edge`):
    /// 1 - smoothstep(-(S/2 + P) x depth, S/2 x depth, sdf), each shape with
    /// the depth its Softness takes (`ShapedLight::depth`, studio.rs's
    /// inradius). It is checked analytically along a line out of the light's
    /// middle, past its reach, so a light dimmed flat or gone black fails:
    /// - a stroke across its half width (its depth);
    /// - a disc (here a circle: |p| - r, depth r);
    /// - a ring (| |p| - its middle radius | - its half thickness, the depth);
    /// - the default hexagon, out through the middle of a side (x - its
    ///   apothem, the depth: how deep its centre sits inside it);
    /// - a scrim, through the middle of a side (a rect's distance, depth its
    ///   half side), times its cloth's glow from the middle.
    /// Over the whole box, at Penumbra 0 each light is the plain one, and
    /// above it never brighter and dark where the plain light is (its reach
    /// stays).
    #[test]
    fn penumbra_widens_every_shapes_own_edge_in_stretch_and_fill() {
        let flat = set_of("flat", picture(4, 4, |_, _| [1.0, 1.0, 1.0, 1.0]));
        let pictured = |light: &LightParams, fit: &str, penumbra: f32| LightParams { image: Some(soft_image("flat", fit, penumbra)), ..light.clone() };
        // A stroke along the horizon, 6 degrees wide: across it at azimuth
        // 90 the distance from its centre line is the elevation.
        let stroke = LightParams {
            shape: "stroke".to_string(),
            softness: 0.4,
            rgb: Some([1.0; 3]),
            intensity_ev: 0.0,
            stroke: Some(crate::hdri::StrokeParams { points: vec![[60.0, 0.0], [120.0, 0.0]], width_deg: 6.0, ..Default::default() }),
            ..image_light("flat")
        };
        let hw = 3f32.to_radians();
        let edge = 0.5 * 0.4 * hw;
        for fit in ["stretch", "fill"] {
            for penumbra in [0.0f32, 0.5, 1.0] {
                let env = Env::new_with(&dark_studio(vec![pictured(&stroke, fit, penumbra)]), &flat);
                for i in 0..=200 {
                    let elevation = 4.0 * i as f32 / 200.0;
                    let got = luminance(env.radiance(dir_from_az_el(90.0, elevation)));
                    let want = 1.0 - smoothstep(hw - edge - penumbra * hw, hw + edge, elevation.to_radians());
                    assert!((got - want).abs() < 2.0e-3, "{fit}: a stroke at penumbra {penumbra}, {elevation} degrees off its line: {got}, one ramp says {want}");
                }
            }
        }
        // The 40 x 40 degree box (`image_light`), half side h: a disc in it is
        // a circle of radius h, and the hexagon's corners lie on that circle.
        let h = 20f32.to_radians().tan();
        let apothem = (std::f32::consts::PI / 6.0).cos() * h;
        // Each shape: the light, its depth, and its signed distance and the
        // glow that multiplies its ramp, x out along the middle.
        type AlongX = fn(f32, f32) -> f32;
        let shapes: [(LightParams, f32, AlongX, AlongX); 4] = [
            (LightParams { shape: "disc".to_string(), ..image_light("flat") }, h, |x, h| x - h, |_, _| 1.0),
            (LightParams { shape: "ring".to_string(), inner: 0.4, ..image_light("flat") }, 0.3 * h, |x, h| (x - 0.7 * h).abs() - 0.3 * h, |_, _| 1.0),
            (
                LightParams { shape: "polygon".to_string(), ..image_light("flat") },
                apothem,
                |x, h| x - (std::f32::consts::PI / 6.0).cos() * h,
                |_, _| 1.0,
            ),
            // The cloth glows with a Gaussian of sigma 0.5 box units (its default).
            (LightParams { shape: "scrim".to_string(), ..image_light("flat") }, h, |x, h| x - h, |x, h| (-2.0 * (x / h) * (x / h)).exp()),
        ];
        for (shape, depth, sdf, glow) in &shapes {
            let name = shape.shape.clone();
            for fit in ["stretch", "fill"] {
                for (softness, penumbra) in [(0.0f32, 0.3f32), (0.0, 0.6), (0.0, 1.0), (0.3, 0.0), (0.3, 0.3), (0.3, 0.6), (0.3, 1.0), (0.6, 1.0)] {
                    let light = LightParams { softness, rgb: Some([1.0; 3]), intensity_ev: 0.0, ..shape.clone() };
                    let env = Env::new_with(&dark_studio(vec![pictured(&light, fit, penumbra)]), &flat);
                    let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
                    let a = 0.5 * (softness * depth).max(1.0e-4);
                    let b = penumbra * depth;
                    // From the middle out past the reach (the disc's, the
                    // farthest, ends at h + a <= 1.3 h).
                    for i in 0..=400 {
                        let x = 1.4 * h * i as f32 / 400.0;
                        let d = (f.center + f.right * x).normalize();
                        let want = (1.0 - smoothstep(-(a + b), a, sdf(x, h))) * glow(x, h);
                        let got = luminance(env.radiance(d));
                        assert!(
                            (got - want).abs() < 2.0e-4,
                            "{name} in {fit}, softness {softness}, penumbra {penumbra}, at x {x} (sdf {}): {got}, one ramp says {want}",
                            sdf(x, h)
                        );
                    }
                }
            }
            let light = LightParams { softness: 0.3, rgb: Some([1.0; 3]), intensity_ev: 0.0, ..shape.clone() };
            let grid = |env: &Env| {
                (0..=60)
                    .flat_map(|j| (0..=60).map(move |i| (i, j)))
                    .map(|(i, j)| luminance(env.radiance(box_dir(&light, -0.2 + 1.4 * i as f32 / 60.0, -0.2 + 1.4 * j as f32 / 60.0))))
                    .collect::<Vec<f32>>()
            };
            let plain = grid(&Env::new(&dark_studio(vec![light.clone()])));
            for fit in ["stretch", "fill"] {
                let draw = |penumbra: f32| grid(&Env::new_with(&dark_studio(vec![pictured(&light, fit, penumbra)]), &flat));
                for (k, (z, p)) in draw(0.0).iter().zip(&plain).enumerate() {
                    assert!((z - p).abs() < 1.0e-5, "{name} in {fit} at penumbra 0, sample {k}: {z} vs the plain light's {p}");
                }
                for (k, (s, p)) in draw(0.6).iter().zip(&plain).enumerate() {
                    assert!(*s <= p + 1.0e-5, "{name} in {fit}, sample {k}: {s} is brighter than the plain light's {p}");
                    if *p == 0.0 {
                        assert_eq!(*s, 0.0, "{name} in {fit}, sample {k}: lit past the plain light's reach");
                    }
                }
            }
        }
    }

    /// P2 (a check finding on P1): an elongated disc in stretch or fill
    /// whose widened ramp reaches deeper than the disc itself (Softness / 2
    /// + Penumbra above 1) showed a bow-tie pinch at its centre. A3's
    /// first-order ellipse distance (`studio::ellipse_metric`) is (k0 - 1)
    /// times the half axis along the way in, so the centre read as deep as
    /// the long half axis seen along the disc and as the short one seen
    /// across it. A 4:1 disc at Softness 1, Penumbra 1 now has no local
    /// minimum along either axis and its one maximum at the centre, which
    /// every direction reaches alike (no pinch), in both fits; it falls away
    /// from the centre along the long axis too (no plateau). Its bright core
    /// has no waist (no bow-tie): just below its peak (0.5 % and 2 % down)
    /// it is nowhere narrower across than at the centre. The same holds part
    /// way there, where review round 1 found the bow-tie still (Softness 1
    /// with Penumbra 0.75 and 0.55, Softness 0.6 with Penumbra 0.8), and
    /// where the ramp stays within the disc but deep enough for the
    /// first-order levels to waist (Softness 0.6 with Penumbra 0.7, 0.3 and
    /// 0.25, Softness 0.3 with Penumbra 0.45, Softness 1 with Penumbra
    /// 0.05), and for a 10:1 disc as well.
    #[test]
    fn a_long_discs_deep_ramp_peaks_at_its_centre_without_a_pinch() {
        let flat = set_of("flat", picture(4, 4, |_, _| [1.0, 1.0, 1.0, 1.0]));
        let hy = 10f32.to_radians().tan();
        let cases = [
            ("stretch", 1.0f32, 1.0f32),
            ("fill", 1.0, 1.0),
            ("stretch", 1.0, 0.75),
            ("fill", 1.0, 0.55),
            ("stretch", 0.6, 0.8),
            ("fill", 0.6, 0.7),
            ("stretch", 0.6, 0.3),
            ("fill", 0.3, 0.45),
            ("stretch", 1.0, 0.05),
            ("fill", 0.6, 0.25),
        ];
        for (aspect, (fit, softness, penumbra)) in [4.0f32, 10.0].into_iter().flat_map(|aspect| cases.map(|case| (aspect, case))) {
            let hx = aspect * hy;
            let light = LightParams {
                shape: "disc".to_string(),
                width_deg: 2.0 * hx.atan().to_degrees(),
                height_deg: 20.0,
                softness,
                rgb: Some([1.0; 3]),
                intensity_ev: 0.0,
                image: Some(soft_image("flat", fit, penumbra)),
                ..image_light("flat")
            };
            let env = Env::new_with(&dark_studio(vec![light.clone()]), &flat);
            let fit = format!("{aspect}:1, {fit}, softness {softness}, penumbra {penumbra}");
            let f = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
            let at = |x: f32, y: f32| luminance(env.radiance((f.center + f.right * x + f.up * y).normalize()));
            let centre = at(0.0, 0.0);
            assert!(centre > 0.5, "{fit}: the centre is lit: {centre}");
            // Along each axis, from the centre out past the reach both ways:
            // never rising, so no local minimum, and the centre's the most.
            for (name, axis) in [("the long axis", vec2f(1.5 * hx, 0.0)), ("the short axis", vec2f(0.0, 1.5 * hx))] {
                for side in [1.0f32, -1.0] {
                    let mut last = centre;
                    for i in 1..=600 {
                        let t = side * i as f32 / 600.0;
                        let got = at(axis.x * t, axis.y * t);
                        assert!(got <= last + 1.0e-6, "{fit}, {name} at {t}: it rises outward from {last} to {got} (a local minimum)");
                        last = got;
                    }
                }
            }
            // The one maximum is the centre's: nothing over the disc is brighter.
            for j in 0..=40 {
                for i in 0..=80 {
                    let (x, y) = (1.2 * hx * (i as f32 / 40.0 - 1.0), 1.2 * hy * (j as f32 / 20.0 - 1.0));
                    let got = at(x, y);
                    assert!(got <= centre + 1.0e-6, "{fit} at ({x}, {y}): {got} is brighter than the centre's {centre}");
                }
            }
            // No pinch: every way into the centre ends at the centre's value.
            for k in 0..64 {
                let angle = std::f32::consts::TAU * k as f32 / 64.0;
                let near = at(1.0e-3 * hx * angle.cos(), 1.0e-3 * hy * angle.sin());
                assert!((near - centre).abs() < 2.0e-3, "{fit}: {near} just off the centre at {} degrees, {centre} at it", angle.to_degrees());
            }
            // Its bright core has no waist: just below the peak it is
            // nowhere narrower across than at the centre (a bow-tie's two
            // lobes meet there).
            for drop in [0.005f32, 0.02] {
                let level = centre * (1.0 - drop);
                let width_at = |x: f32| (0..=600).map(|j| 1.2 * hy * j as f32 / 600.0).take_while(|y| at(x, *y) >= level).last().unwrap_or(0.0);
                let waist = width_at(0.0);
                for t in [0.05f32, 0.1, 0.2, 0.3, 0.5] {
                    let w = width_at(t * hx);
                    assert!(w <= waist + 1.0e-3 * hy, "{fit}: {drop} below the peak the core is {w} across at {t} of the long half axis, {waist} at the centre (a waist)");
                }
            }
            if 0.5 * softness + penumbra <= 1.0 {
                continue;
            }
            // One maximum, not a plateau round it: past the disc's depth the
            // light falls away from the centre along the long axis too.
            for t in [0.25f32, 0.5, 0.75] {
                let got = at(t * hx, 0.0);
                assert!(got < centre - 1.0e-3, "{fit}: {got} at {t} of the long half axis, as bright as the centre's {centre} (a plateau)");
            }
        }
    }

    /// P2, review round 1 (M1): a picture's Penumbra widens the light's own
    /// edge only; the bare shape stays the plain shape. A long disc filled
    /// with sky (a window, which shows the sky through its bare shape, not
    /// the picture) with a stretched picture at Penumbra 1 draws, and hands
    /// the key, exactly as the same window without the picture: its glass
    /// does not read the exact ellipse distance the widened ramp reads.
    #[test]
    fn a_long_disc_windows_glass_ignores_its_pictures_penumbra() {
        let flat = set_of("flat", picture(4, 4, |_, _| [1.0, 1.0, 1.0, 1.0]));
        let window = |image: Option<LightImageParams>| LightParams {
            key: true,
            shape: "disc".to_string(),
            width_deg: 80.0,
            height_deg: 20.0,
            softness: 1.0,
            gobo: Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() }),
            image,
            ..image_light("flat")
        };
        let params = |image| HdriParams { lights: vec![window(image)], ..dark_studio(Vec::new()) };
        let with = Env::new_with(&params(Some(soft_image("flat", "stretch", 1.0))), &flat);
        let without = Env::new_with(&params(None), &flat);
        let (a, b) = (with.bake(64), without.bake(64));
        let lit = b.data.iter().filter(|t| t[1] > 1.0e-4).count();
        assert!(lit > 20, "the test needs the window to show the sky: {lit} texels");
        for (i, (x, y)) in a.data.iter().zip(&b.data).enumerate() {
            assert_eq!(x, y, "texel {i}: the window with a picture at Penumbra 1 shows {x:?}, without it {y:?}");
        }
        let key = |env: &Env| env.sun().map(|k| [k.radiance.x, k.radiance.y, k.radiance.z, k.facing, k.cos_cover]);
        assert_eq!(key(&with), key(&without), "the key the window hands over");
    }

    /// The mean of the premultiplied picture (colour x alpha, and alpha)
    /// over the box centred on (x, y) with half-widths (rx, ry), in texels
    /// (texel (i, j) covers i..i+1 by j..j+1), each at least half a texel,
    /// the edge texels held past the border: by brute force, as the gobo's
    /// test computes its mask's.
    fn premultiplied_box_mean(img: &EnvMap, x: f64, y: f64, rx: f64, ry: f64) -> [f64; 4] {
        let (w, h) = (img.width, img.height);
        let (rx, ry) = (rx.max(0.5), ry.max(0.5));
        let cover = |lo: f64, hi: f64, i: usize, n: usize| {
            let start = if i == 0 { f64::NEG_INFINITY } else { i as f64 };
            let end = if i + 1 == n { f64::INFINITY } else { (i + 1) as f64 };
            (hi.min(end) - lo.max(start)).max(0.0)
        };
        let mut sum = [0.0f64; 4];
        for j in 0..h {
            let down = cover(y - ry, y + ry, j, h);
            for i in 0..w {
                let k = cover(x - rx, x + rx, i, w) * down;
                let t = img.data[j * w + i];
                let a = t[3].clamp(0.0, 1.0) as f64;
                for c in 0..3 {
                    sum[c] += t[c] as f64 * a * k;
                }
                sum[3] += a * k;
            }
        }
        sum.map(|s| s / (4.0 * rx * ry))
    }

    /// The image block of picture `key` shown `fit`, its alpha cutting it
    /// out, its cut-out edges softened by `mask_softness` (P2).
    fn masked_image(key: &str, fit: &str, mask_softness: f32) -> LightImageParams {
        LightImageParams { key: key.to_string(), fit: fit.to_string(), alpha_as_mask: true, mask_softness: Some(mask_softness), ..LightImageParams::default() }
    }

    /// P2: the half-width, in texels, of the box Mask softness `s` filters a
    /// `w` x `h` picture over, as the owner's decision states it: the box is
    /// `s` x 5 % of the picture's smaller side across, each way.
    fn mask_box_half(s: f32, w: usize, h: usize) -> f64 {
        0.5 * s as f64 * 0.05 * w.min(h) as f64
    }

    /// P1, P2: Mask softness is an exact box filter of the premultiplied
    /// picture, Mask softness x 5 % of the picture's smaller side across in
    /// texels (`mask_box_half`): its alpha is the box's mean alpha, its
    /// colour the box's mean premultiplied colour over that, in every fit,
    /// whatever the Penumbra (which in fit fades the border on top of it).
    #[test]
    fn softened_mask_edges_are_a_box_filter_of_the_premultiplied_picture() {
        let hash = |x: usize, y: usize| ((x * 7919 + y * 104729) % 1009) as f32 / 1008.0;
        let (w, h) = (96usize, 64usize);
        let img = picture(w, h, |x, y| [1.5 * hash(x, y), hash(y, x), 0.3 + 0.5 * hash(x + 3, y), if hash(y + 5, x) < 0.3 { 0.0 } else { 1.0 }]);
        let images = set_of("noise", img.clone());
        let half = vec2f(0.6, 0.3);
        for &fit in IMAGE_FIT_NAMES {
            for (mask_softness, penumbra) in [(1.0f32, 0.0f32), (0.5, 0.0), (1.0, 0.3)] {
                let params = LightImageParams { penumbra: Some(penumbra), ..masked_image("noise", fit, mask_softness) };
                let image = LightImage::new(&params, &images).unwrap();
                // The picture's tangent-plane half extents, and the Penumbra's band.
                let wide = 1.5 / (half.x / half.y);
                let pic = match fit {
                    "fit" => if wide > 1.0 { vec2f(half.x, half.y / wide) } else { vec2f(half.x * wide, half.y) },
                    "fill" => if wide > 1.0 { vec2f(half.x * wide, half.y) } else { vec2f(half.x, half.y / wide) },
                    _ => half,
                };
                let band = penumbra * pic.x.min(pic.y);
                let r = mask_box_half(mask_softness, w, h);
                for j in 0..=24 {
                    for i in 0..=30 {
                        let p = vec2f(-0.66 + 1.32 * i as f32 / 30.0, -0.44 + 0.88 * j as f32 / 24.0);
                        let (colour, alpha) = image.shade(p, half);
                        // Where shade reads the picture: u to the viewer's right, held to the texel centres.
                        let u = (0.5 + 0.5 * image.x_sign * p.x / pic.x).max(0.5 / w as f32).min(1.0 - 0.5 / w as f32);
                        let v = (0.5 - 0.5 * p.y / pic.y).max(0.5 / h as f32).min(1.0 - 0.5 / h as f32);
                        let name = format!("{fit}, mask softness {mask_softness}, penumbra {penumbra}, at {p:?}");
                        if fit == "fit" && (p.x.abs() > pic.x || p.y.abs() > pic.y) {
                            assert_eq!(alpha, 0.0, "{name}: past the picture");
                            continue;
                        }
                        let m = premultiplied_box_mean(&img, u as f64 * w as f64, v as f64 * h as f64, r, r);
                        let fade = if fit == "fit" && penumbra > 0.0 { smoothstep(0.0, band, pic.x - p.x.abs()) * smoothstep(0.0, band, pic.y - p.y.abs()) } else { 1.0 };
                        let want_alpha = m[3].min(1.0) as f32 * fade;
                        assert!((alpha - want_alpha).abs() < 1.0e-5, "{name}: alpha {alpha}, the box mean {want_alpha}");
                        if m[3] > 1.0e-6 {
                            let want = vec3f((m[0] / m[3]) as f32, (m[1] / m[3]) as f32, (m[2] / m[3]) as f32) * image.gain;
                            assert!((colour - want).length() < 1.0e-4 * want.length().max(1.0), "{name}: colour {colour:?}, the box mean {want:?}");
                        }
                    }
                }
            }
        }
    }

    /// How closely the width a softened hard edge ramps over is held to the
    /// one Mask softness states (`mask_box_half`), as a share of it: the
    /// 10 % to 90 % rise of a box filter's ramp is 0.8 of its width, found
    /// here between samples a 200th of the ramp apart (0.6 % at most).
    const EDGE_WIDTH_TOLERANCE: f64 = 0.01;

    /// P2 (the owner's decision): Mask softness blurs a hard cut-out edge
    /// over its own width, Mask softness x 5 % of the picture's smaller side
    /// in texels: the alpha is a box filter's ramp that wide, centred on the
    /// edge, scaling with the value and with the picture's size, whatever the
    /// Penumbra. And the colour fades with its alpha: no dark fringe where
    /// the alpha was 0 and a PNG keeps black, the colour just inside the edge
    /// is the picture's (P1's premultiplied filter, kept).
    #[test]
    fn a_softened_hard_edge_ramps_over_its_mask_softness_without_a_dark_fringe() {
        // Left half an opaque orange, right half clear over black.
        let edge = |w: usize, h: usize| set_of("edge", picture(w, h, |x, _| if x < w / 2 { [1.0, 0.5, 0.25, 1.0] } else { [0.0, 0.0, 0.0, 0.0] }));
        let half = vec2f(0.5, 0.25);
        for (w, h) in [(400usize, 200usize), (160, 80)] {
            let images = edge(w, h);
            let sharp = LightImage::new(&masked_image("edge", "stretch", 0.0), &images).unwrap();
            for mask_softness in [0.25f32, 0.5, 1.0] {
                // The ramp's width in texels: 2.5, 5 and 10 on the larger picture, 1, 2 and 4 on the smaller.
                let width = 2.0 * mask_box_half(mask_softness, w, h);
                for penumbra in [None, Some(0.4)] {
                    let soft = LightImage::new(&LightImageParams { penumbra, ..masked_image("edge", "stretch", mask_softness) }, &images).unwrap();
                    let orange = vec3f(1.0, 0.5, 0.25) * soft.gain;
                    let name = format!("{w} x {h}, mask softness {mask_softness}, penumbra {penumbra:?}");
                    // Texel columns from the edge (at w / 2), out across the ramp.
                    let at = |t: f64| {
                        let u = (0.5 * w as f64 + t) / w as f64;
                        soft.shade(vec2f(soft.x_sign * (2.0 * u as f32 - 1.0) * half.x, 0.1), half)
                    };
                    let (mut rise10, mut rise90) = (None, None);
                    let mut softer = 0;
                    for i in 0..=400 {
                        let t = width * (-1.0 + 2.0 * i as f64 / 400.0);
                        let (colour, alpha) = at(t);
                        let want = (0.5 - t / width).max(0.0).min(1.0) as f32;
                        assert!((alpha - want).abs() < 1.0e-4, "{name}, {t} texels from the edge: alpha {alpha}, a ramp {width} texels wide says {want}");
                        if alpha > 1.0e-6 {
                            assert!((colour - orange).length() < 1.0e-4, "{name}, {t} texels from the edge: {colour:?}, a dark fringe (the picture is {orange:?})");
                        }
                        // The ramp falls to the right: its 90 % point comes first.
                        if alpha <= 0.9 && rise90.is_none() {
                            rise90 = Some(t);
                        }
                        if alpha <= 0.1 && rise10.is_none() {
                            rise10 = Some(t);
                        }
                        let u = (0.5 * w as f64 + t) / w as f64;
                        let sharp_alpha = sharp.shade(vec2f(soft.x_sign * (2.0 * u as f32 - 1.0) * half.x, 0.1), half).1;
                        softer += ((alpha - sharp_alpha).abs() > 0.05) as usize;
                    }
                    let measured = (rise10.expect("it falls to 0.1") - rise90.expect("it falls to 0.9")) / 0.8;
                    assert!((measured / width - 1.0).abs() < EDGE_WIDTH_TOLERANCE, "{name}: the edge ramps over {measured} texels, Mask softness says {width}");
                    // One texel across, the box is the bilinear lookup itself.
                    if width >= 2.0 {
                        assert!(softer > 120, "{name}: the softened edge is the sharp one at all but {softer} samples");
                    }
                }
            }
        }
    }

    /// P1, P2, D15: a picture's softened mask is one summed-area table, built
    /// for the first light that softens it and shared by every other light
    /// and later `Env` that shows it, whatever their Mask softness
    /// (`SummedArea`, as image gobos share theirs), 32 bytes a table entry
    /// (134.3 MB at 2048 x 2048), and freed with the picture
    /// (`keeps_soft_table` says so to a host that drops the picture). At
    /// Mask softness 0 (None, or held to 0) or without Alpha cuts out no
    /// table is built; Penumbra plays no part.
    #[test]
    fn a_softened_pictures_table_is_built_once_shared_and_freed() {
        use crate::hdri::probe::{counted, Work};
        let texels = |x: usize, y: usize| [0.5, 0.25 * (x % 4) as f32, 0.1 * (y % 3) as f32, if (x / 3 + y / 2) % 2 == 0 { 1.0 } else { 0.0 }];
        let images = set_of("logo", picture(32, 16, texels));
        let picture_of = Arc::downgrade(images.get("logo").unwrap());
        assert!(!keeps_soft_table(&picture_of), "nothing softened it yet");
        let block = |mask_softness: f32| masked_image("logo", "fit", mask_softness);
        let lit = |azimuth_deg: f32, mask_softness: f32| LightParams { azimuth_deg, image: Some(block(mask_softness)), ..image_light("logo") };
        let before = counted(Work::SoftMask);
        let env = Env::new_with(&dark_studio(vec![lit(90.0, 0.3), lit(200.0, 0.8)]), &images);
        assert_eq!(counted(Work::SoftMask), before + 1, "two lights share one table");
        assert!(keeps_soft_table(&picture_of));
        let dragged = Env::new_with(&dark_studio(vec![lit(91.0, 0.3), lit(200.0, 0.8)]), &images);
        assert_eq!(counted(Work::SoftMask), before + 1, "a held drag's next Env reads the same table");
        let table = |params: &LightImageParams| LightImage::new(params, &images).expect("the picture").soft;
        let (a, b) = (table(&block(0.3)).expect("a table"), table(&block(1.0)).expect("a table"));
        assert!(Arc::ptr_eq(&a, &b), "one table, whatever the mask softness");
        assert!(Arc::ptr_eq(&a, &table(&LightImageParams { penumbra: Some(0.6), ..block(0.3) }).expect("a table")), "and whatever the penumbra");
        assert_eq!(a.bytes(), 33 * 17 * 32, "four f64 sums an entry");
        // At 0, held to 0, or without the mask: none built, none kept.
        for off in [
            block(0.0),
            LightImageParams { mask_softness: None, ..block(0.5) },
            LightImageParams { mask_softness: Some(-1.0), ..block(0.5) },
            LightImageParams { mask_softness: Some(f32::NAN), ..block(0.5) },
            LightImageParams { penumbra: Some(0.8), ..block(0.0) },
            LightImageParams { alpha_as_mask: false, ..block(0.5) },
        ] {
            assert!(table(&off).is_none(), "{off:?} keeps a table");
        }
        let fresh = set_of("logo", picture(32, 16, texels));
        let off = LightParams { image: Some(LightImageParams { penumbra: Some(0.8), ..block(0.0) }), ..image_light("logo") };
        let _ = Env::new_with(&dark_studio(vec![off]), &fresh);
        assert_eq!(counted(Work::SoftMask), before + 1, "a light that does not soften built a table");
        assert!(!keeps_soft_table(&Arc::downgrade(fresh.get("logo").unwrap())));
        // Another picture with the same texels has its own; a table goes with its picture.
        let twin = LightImage::new(&block(0.5), &fresh).unwrap().soft.expect("a table");
        assert!(!Arc::ptr_eq(&a, &twin));
        assert_eq!(counted(Work::SoftMask), before + 2);
        let kept = Arc::downgrade(&a);
        drop((a, b, env, dragged, images));
        assert!(picture_of.upgrade().is_none(), "the picture is gone");
        crate::hdri::composite::drop_dead_facts();
        assert!(kept.upgrade().is_none(), "a freed picture's table is freed with it");
        assert!(!keeps_soft_table(&picture_of), "and its entry with it");
    }

    /// D17: a picture reads its block as clamp() holds it: a Penumbra past 1
    /// draws as 1, a NaN one as 0; and so does its Mask softness (P2).
    #[test]
    fn a_pictures_penumbra_reads_as_clamp_holds_it() {
        let strip = set_of("strip", picture(8, 2, |_, _| [1.0, 1.0, 1.0, 1.0]));
        let shade = |penumbra: f32| LightImage::new(&soft_image("strip", "fit", penumbra), &strip).unwrap().shade(vec2f(0.0, 0.05), vec2f(0.5, 0.5));
        assert_eq!(shade(7.0), shade(1.0));
        assert_eq!(shade(f32::NAN), shade(0.0));
        assert!(shade(1.0).1 < shade(0.0).1, "Penumbra 1 fades the strip");
        // A hard alpha edge down the middle of a 40 x 40 picture: Mask softness 1 blurs it over 2 texels.
        let edge = set_of("edge", picture(40, 40, |x, _| [1.0, 1.0, 1.0, if x < 20 { 1.0 } else { 0.0 }]));
        let shade = |mask_softness: f32| LightImage::new(&masked_image("edge", "stretch", mask_softness), &edge).unwrap().shade(vec2f(0.0, 0.0), vec2f(0.5, 0.5));
        // Just to the viewer's left of the edge, half a texel in.
        let shade_in = |mask_softness: f32| {
            let image = LightImage::new(&masked_image("edge", "stretch", mask_softness), &edge).unwrap();
            image.shade(vec2f(-image.x_sign * 0.5 / 40.0, 0.0), vec2f(0.5, 0.5))
        };
        assert_eq!(shade(7.0), shade(1.0));
        assert_eq!(shade_in(7.0), shade_in(1.0));
        assert_eq!(shade_in(f32::NAN), shade_in(0.0));
        assert_eq!(shade_in(-1.0), shade_in(0.0));
        assert!(shade_in(1.0).1 < shade_in(0.0).1, "Mask softness 1 blurs the edge");
    }
}
