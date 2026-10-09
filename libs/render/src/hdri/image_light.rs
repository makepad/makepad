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

use makepad_draw::*;
use makepad_render_material::ibl::EnvMap;
use std::collections::HashMap;
use std::sync::Arc;
use super::studio::light_frame;
use super::{HdriParams, ImageFit, LightImageParams};

/// The longest side a light's picture is kept at. A light rarely spans more
/// than 60 degrees, which an 8K map draws about 1400 texels wide; beyond
/// 2048 a picture only costs memory (16 bytes a texel).
pub const MAX_LIGHT_IMAGE_SIDE: usize = 2048;

/// The loaded pictures an `Env` may use, by key. The maps are in the FILE
/// orientation `image::load_image` returns: row 0 on top, no column roll.
/// Cloning is cheap: the pixels are shared.
#[derive(Clone, Debug, Default)]
pub struct ImageSet {
    pub images: HashMap<String, Arc<EnvMap>>,
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
}

impl LightImage {
    /// The picture `params` names, or None when the key is empty, not in
    /// `images`, or the picture is empty: the light is then drawn plain.
    pub fn new(params: &LightImageParams, images: &ImageSet) -> Option<LightImage> {
        if params.key.is_empty() {
            return None;
        }
        let map = images.get(&params.key)?.clone();
        if map.width == 0 || map.height == 0 || map.data.len() != map.width * map.height {
            return None;
        }
        let mask = params.alpha_as_mask;
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for t in &map.data {
            let w = if !mask {
                1.0
            } else if t[3].is_finite() {
                t[3].clamp(0.0, 1.0) as f64
            } else {
                0.0
            };
            sum += texel_luminance(t) as f64 * w;
            weight += w;
        }
        let mean = if weight > 0.0 { sum / weight } else { 0.0 };
        Some(LightImage {
            fit: params.fit(),
            mask,
            // A black (or fully transparent) picture emits nothing.
            gain: if mean > 1.0e-9 { (1.0 / mean) as f32 } else { 0.0 },
            aspect: map.width as f32 / map.height as f32,
            // A light due north on the horizon: the viewer's right is east, +X.
            x_sign: if light_frame(0.0, 0.0, 0.0).right.x >= 0.0 { 1.0 } else { -1.0 },
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
    pub fn shade(&self, p: Vec2f, half: Vec2f) -> (Vec3f, f32) {
        // The box as the viewer sees it: u to the right, v down (row 0 on top).
        let mut u = 0.5 + 0.5 * self.x_sign * p.x / half.x.max(1.0e-6);
        let mut v = 0.5 - 0.5 * p.y / half.y.max(1.0e-6);
        // How much wider than the box the picture is.
        let wide = self.aspect / (half.x / half.y.max(1.0e-6)).max(1.0e-6);
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
        let (rgb, alpha) = self.bilinear(u, v);
        (rgb * self.gain, alpha)
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
    use crate::hdri::{dir_from_az_el, Env, LightParams};
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
            LightImage::new(&LightImageParams { key: "strip".to_string(), fit: fit.to_string(), alpha_as_mask: false }, &strip).unwrap()
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
        let params = LightImageParams { key: "edge".to_string(), fit: "stretch".to_string(), alpha_as_mask: true };
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
}
