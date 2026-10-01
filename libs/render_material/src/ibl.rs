//! Environments for image-based lighting: HDR equirectangular maps from
//! Radiance `.hdr` files or procedural presets, prefiltered for the GPU
//! into one roughness atlas plus 9 spherical-harmonic irradiance
//! coefficients.
//!
//! # Conventions
//!
//! Directions are world space, right-handed, **y up**. An equirect map's
//! row 0 is straight up (+y) and its last row straight down; `v = acos(y) /
//! π`. Longitude runs so that `u = 0.5` looks down **-z** (a default camera's
//! view direction), `u = 0.75` looks at +x, `u = 0.25` at -x and `u = 0` /
//! `u = 1` at +z:
//!
//! ```text
//! u = 0.5 + atan2(x, -z) / 2π      v = acos(y) / π
//! dir = (sin θ sin φ, cos θ, -sin θ cos φ)   φ = (u - 0.5)·2π, θ = v·π
//! ```
//!
//! Pixels are linear RGB radiance (alpha 1). Everything here is a pure
//! function of its inputs: the same map prefilters to the same bits.

use makepad_draw::makepad_platform::resource_resolver::DecodeBudget;
use std::f32::consts::{PI, TAU};

fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform 0..1 from a seed and an index.
fn rand01(seed: u64, index: u64) -> f32 {
    ((hash64(seed ^ hash64(index)) >> 40) as f32) / ((1u64 << 24) as f32)
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize_or_up(a: [f32; 3]) -> [f32; 3] {
    let l = dot3(a, a).sqrt();
    if l > 1e-20 && l.is_finite() { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 1.0, 0.0] }
}

fn any_perpendicular(n: [f32; 3]) -> [f32; 3] {
    let helper = if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    normalize_or_up(cross3(helper, n))
}

/// A linear HDR equirectangular environment (`height = width / 2` for maps
/// this module makes; loaded files keep their own size). Row-major, row 0 up.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvMap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 4]>,
}

/// Direction (need not be unit) to equirect uv (see the module docs).
pub fn dir_to_equirect_uv(dir: [f32; 3]) -> [f32; 2] {
    let d = normalize_or_up(dir);
    let u = 0.5 + d[0].atan2(-d[2]) / TAU;
    let v = d[1].clamp(-1.0, 1.0).acos() / PI;
    [u.rem_euclid(1.0), v]
}

/// Equirect uv to a unit direction (the inverse of [`dir_to_equirect_uv`]).
pub fn equirect_uv_to_dir(uv: [f32; 2]) -> [f32; 3] {
    let phi = (uv[0] - 0.5) * TAU;
    let theta = uv[1] * PI;
    let (st, ct) = theta.sin_cos();
    [st * phi.sin(), ct, -st * phi.cos()]
}

#[inline]
fn luminance(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

impl EnvMap {
    /// A constant-colour map (`width` x `width / 2`).
    pub fn constant(width: usize, color: [f32; 3]) -> Self {
        let width = width.max(2);
        let height = (width / 2).max(1);
        Self { width, height, data: vec![[color[0], color[1], color[2], 1.0]; width * height] }
    }

    /// Fill a `width` x `width / 2` map by evaluating `f(dir)` at every texel
    /// centre.
    pub fn from_fn(width: usize, mut f: impl FnMut([f32; 3]) -> [f32; 3]) -> Self {
        let width = width.max(2);
        let height = (width / 2).max(1);
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let d = equirect_uv_to_dir([(x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32]);
                let c = f(d);
                data.push([c[0], c[1], c[2], 1.0]);
            }
        }
        Self { width, height, data }
    }

    #[inline]
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.rem_euclid(self.width as i64) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.data[y * self.width + x]
    }

    /// Bilinear sample at uv (u wraps, v clamps).
    pub fn sample_uv(&self, uv: [f32; 2]) -> [f32; 4] {
        let fx = uv[0] * self.width as f32 - 0.5;
        let fy = uv[1] * self.height as f32 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut out = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            out[k] = top + (bot - top) * ty;
        }
        out
    }

    /// Bilinear radiance seen along `dir`.
    pub fn sample(&self, dir: [f32; 3]) -> [f32; 4] {
        self.sample_uv(dir_to_equirect_uv(dir))
    }

    /// Solid angle of a texel in row `y`, steradians.
    fn texel_solid_angle(&self, y: usize) -> f32 {
        let theta = (y as f32 + 0.5) / self.height as f32 * PI;
        (TAU / self.width as f32) * (PI / self.height as f32) * theta.sin()
    }

    /// Solid-angle weighted mean radiance (rgb).
    pub fn mean(&self) -> [f32; 3] {
        let mut sum = [0.0f64; 3];
        let mut w = 0.0f64;
        for y in 0..self.height {
            let sa = self.texel_solid_angle(y) as f64;
            for x in 0..self.width {
                let c = self.data[y * self.width + x];
                for k in 0..3 {
                    sum[k] += c[k] as f64 * sa;
                }
                w += sa;
            }
        }
        [(sum[0] / w) as f32, (sum[1] / w) as f32, (sum[2] / w) as f32]
    }

    /// Area-averaged resample to `width` x `width / 2` (box filter when
    /// shrinking, bilinear when growing).
    pub fn resized(&self, width: usize) -> EnvMap {
        let width = width.max(2);
        let height = (width / 2).max(1);
        if width == self.width && height == self.height {
            return self.clone();
        }
        let mut data = Vec::with_capacity(width * height);
        let sx = self.width as f32 / width as f32;
        let sy = self.height as f32 / height as f32;
        for y in 0..height {
            for x in 0..width {
                if sx <= 1.0 && sy <= 1.0 {
                    data.push(self.sample_uv([(x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32]));
                    continue;
                }
                let x0 = (x as f32 * sx).floor() as usize;
                let x1 = (((x + 1) as f32 * sx).ceil() as usize).clamp(x0 + 1, self.width);
                let y0 = (y as f32 * sy).floor() as usize;
                let y1 = (((y + 1) as f32 * sy).ceil() as usize).clamp(y0 + 1, self.height);
                let mut acc = [0.0f32; 4];
                let mut n = 0.0;
                // Weighted by solid angle: rows near the poles cover less.
                for yy in y0..y1 {
                    let w = self.texel_solid_angle(yy);
                    for xx in x0..x1 {
                        let c = self.data[yy * self.width + xx];
                        for k in 0..4 {
                            acc[k] += c[k] * w;
                        }
                        n += w;
                    }
                }
                data.push([acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]);
            }
        }
        EnvMap { width, height, data }
    }

    /// The direction of the brightest region (luminance, after averaging
    /// down to 64 x 32 so a single hot pixel does not win): where to put an
    /// automatic key light for an HDRI.
    pub fn brightest_direction(&self) -> [f32; 3] {
        let small = self.resized(64.min(self.width.max(2)));
        let mut best = (f32::MIN, 0usize);
        for (i, c) in small.data.iter().enumerate() {
            let l = luminance(*c);
            if l > best.0 {
                best = (l, i);
            }
        }
        let (x, y) = (best.1 % small.width, best.1 / small.width);
        equirect_uv_to_dir([(x as f32 + 0.5) / small.width as f32, (y as f32 + 0.5) / small.height as f32])
    }

    /// Flat RGBA f32 texels, row-major, for `TextureFormat::VecRGBAf32`.
    pub fn to_rgba_f32(&self) -> Vec<f32> {
        self.data.iter().flat_map(|c| c.iter().copied()).collect()
    }

    /// A procedural environment `width` x `width / 2`, radiance scaled by
    /// `intensity` (1 = the preset's own level) and turned `rotation_deg`
    /// about +y (positive turns the lights counter-clockwise seen from above).
    pub fn procedural(preset: &EnvPreset, width: usize, intensity: f32, rotation_deg: f32) -> EnvMap {
        let (s, c) = (rotation_deg * PI / 180.0).sin_cos();
        EnvMap::from_fn(width, |d| {
            // Evaluate the preset at the direction turned back by the rotation.
            let local = [c * d[0] - s * d[2], d[1], s * d[0] + c * d[2]];
            let v = preset.radiance(local);
            [v[0] * intensity, v[1] * intensity, v[2] * intensity]
        })
    }
}

// ---------------------------------------------------------------------------
// Presets.
// ---------------------------------------------------------------------------

/// Built-in procedural environments (`environment: Environment{preset: @studio}`).
#[derive(Clone, Debug, PartialEq)]
pub enum EnvPreset {
    /// Dark grey cyclorama, a large key softbox front-left, a fill right, a
    /// top box and a tall rim strip behind: the classic product-shot look.
    Studio,
    /// One big soft key and a dim fill on neutral grey.
    Softbox,
    /// Deep blue zenith, orange horizon, an HDR sun disc 5° up with a halo.
    Sunset,
    /// Bright even grey sky, darker ground.
    Overcast,
    /// Dark blue night with a moon, stars and a faint horizon glow.
    Night,
    /// Near-black with magenta and cyan light strips.
    Neon,
    /// A three-stop vertical gradient (linear colours).
    Gradient { top: [f32; 3], horizon: [f32; 3], bottom: [f32; 3] },
}

impl EnvPreset {
    /// Preset names as authored (`@studio`, ...), for messages.
    pub const NAMES: &'static [&'static str] = &["studio", "softbox", "sunset", "overcast", "night", "neon", "gradient"];

    /// A preset by name (`gradient` gets a neutral default ramp).
    pub fn by_name(name: &str) -> Option<EnvPreset> {
        Some(match name {
            "studio" => EnvPreset::Studio,
            "softbox" => EnvPreset::Softbox,
            "sunset" => EnvPreset::Sunset,
            "overcast" => EnvPreset::Overcast,
            "night" => EnvPreset::Night,
            "neon" => EnvPreset::Neon,
            "gradient" => EnvPreset::Gradient { top: [0.6, 0.7, 0.9], horizon: [0.9, 0.9, 0.9], bottom: [0.2, 0.2, 0.2] },
            _ => return None,
        })
    }

    /// Radiance along unit direction `d` (unrotated, intensity 1).
    pub fn radiance(&self, d: [f32; 3]) -> [f32; 3] {
        match self {
            EnvPreset::Studio => {
                // Cyclorama: floor darker, walls brighter near the horizon.
                let y = d[1];
                let wall = 0.10 + 0.06 * (1.0 - y.abs()).powf(3.0);
                let base = if y < 0.0 { 0.05 + 0.04 * (1.0 + y) } else { wall };
                let mut c = [base, base, base * 1.02];
                // Key: front-left, above.
                add(&mut c, softbox(d, dir_deg(-40.0, 30.0), 0.45, 0.35, [9.0, 8.8, 8.4]));
                // Fill: right, lower, dimmer.
                add(&mut c, softbox(d, dir_deg(55.0, 12.0), 0.35, 0.3, [2.4, 2.5, 2.7]));
                // Top box.
                add(&mut c, softbox(d, dir_deg(0.0, 80.0), 0.5, 0.5, [4.5, 4.5, 4.5]));
                // Tall rim strip behind.
                add(&mut c, softbox(d, dir_deg(160.0, 15.0), 0.08, 0.7, [6.0, 6.0, 6.2]));
                c
            }
            EnvPreset::Softbox => {
                let g = 0.18 + 0.05 * d[1];
                let mut c = [g, g, g];
                add(&mut c, softbox(d, dir_deg(-30.0, 35.0), 0.7, 0.55, [7.0, 7.0, 7.0]));
                add(&mut c, softbox(d, dir_deg(70.0, 5.0), 0.5, 0.5, [0.9, 0.9, 0.95]));
                c
            }
            EnvPreset::Sunset => {
                let y = d[1];
                let c = if y >= 0.0 {
                    let t = y.powf(0.45);
                    lerp3([1.4, 0.62, 0.26], [0.08, 0.13, 0.34], t)
                } else {
                    lerp3([0.35, 0.18, 0.1], [0.04, 0.03, 0.03], (-y).powf(0.5))
                };
                let sun = dir_deg(0.0, 5.0);
                let cosang = dot3(d, sun).clamp(-1.0, 1.0);
                let ang = cosang.acos();
                let mut c = c;
                // Disc (radius ~0.6°) and a soft halo.
                let disc = smoothstep(0.0125, 0.0095, ang);
                add(&mut c, [300.0 * disc, 190.0 * disc, 90.0 * disc]);
                let halo = (-ang * 9.0).exp();
                add(&mut c, [2.5 * halo, 1.2 * halo, 0.4 * halo]);
                c
            }
            EnvPreset::Overcast => {
                let y = d[1];
                if y >= 0.0 {
                    let t = y;
                    let v = 0.85 + 0.25 * t;
                    [v, v, v * 1.03]
                } else {
                    let v = 0.25 + 0.1 * (1.0 + y);
                    [v, v * 0.97, v * 0.93]
                }
            }
            EnvPreset::Night => {
                let y = d[1];
                let mut c = if y >= 0.0 { lerp3([0.035, 0.04, 0.08], [0.008, 0.01, 0.03], y.powf(0.5)) } else { [0.006, 0.006, 0.008] };
                // Horizon city glow.
                let glow = (-(y.abs()) * 18.0).exp();
                add(&mut c, [0.12 * glow, 0.07 * glow, 0.05 * glow]);
                // Moon.
                let moon = dir_deg(-60.0, 40.0);
                let ang = dot3(d, moon).clamp(-1.0, 1.0).acos();
                let disc = smoothstep(0.03, 0.022, ang);
                add(&mut c, [14.0 * disc, 14.0 * disc, 16.0 * disc]);
                add(&mut c, [0.15 * (-ang * 12.0).exp(), 0.15 * (-ang * 12.0).exp(), 0.2 * (-ang * 12.0).exp()]);
                // Stars: a hashed lattice on the upper hemisphere.
                if y > 0.05 {
                    let uv = dir_to_equirect_uv(d);
                    let (gx, gy) = ((uv[0] * 160.0).floor(), (uv[1] * 80.0).floor());
                    let k = hash64((gx as i64 as u64) ^ ((gy as i64 as u64) << 20));
                    if k % 23 == 0 {
                        let cx = (gx + 0.5) / 160.0;
                        let cy = (gy + 0.5) / 80.0;
                        let dd = ((uv[0] - cx) * 160.0).powi(2) + ((uv[1] - cy) * 80.0).powi(2);
                        let s = (-dd * 30.0).exp() * (0.5 + rand01(k, 1) * 2.0);
                        add(&mut c, [s, s, s * 1.1]);
                    }
                }
                c
            }
            EnvPreset::Neon => {
                let mut c = [0.015, 0.012, 0.025];
                add(&mut c, softbox(d, dir_deg(-70.0, 10.0), 0.05, 0.9, [4.0, 0.3, 2.6]));
                add(&mut c, softbox(d, dir_deg(70.0, 10.0), 0.05, 0.9, [0.3, 3.0, 4.0]));
                add(&mut c, softbox(d, dir_deg(180.0, 30.0), 0.9, 0.04, [3.0, 0.4, 2.2]));
                add(&mut c, softbox(d, dir_deg(0.0, 60.0), 0.6, 0.05, [0.4, 2.6, 3.4]));
                c
            }
            EnvPreset::Gradient { top, horizon, bottom } => {
                let y = d[1];
                if y >= 0.0 {
                    lerp3(*horizon, *top, y)
                } else {
                    lerp3(*horizon, *bottom, -y)
                }
            }
        }
    }
}

#[inline]
fn add(c: &mut [f32; 3], v: [f32; 3]) {
    c[0] += v[0];
    c[1] += v[1];
    c[2] += v[2];
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Direction at `yaw` degrees (0 = -z, positive toward +x) and `pitch`
/// degrees above the horizon.
fn dir_deg(yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = (yaw * PI / 180.0).sin_cos();
    let (sp, cp) = (pitch * PI / 180.0).sin_cos();
    [cp * sy, sp, -cp * cy]
}

/// A soft-edged rectangular light facing the origin from `center`, half
/// sizes in tangent units (gnomonic projection), radiance `color`.
fn softbox(d: [f32; 3], center: [f32; 3], half_w: f32, half_h: f32, color: [f32; 3]) -> [f32; 3] {
    let c = dot3(d, center);
    if c <= 0.0 {
        return [0.0; 3];
    }
    let mut right = cross3([0.0, 1.0, 0.0], center);
    if dot3(right, right) < 1e-8 {
        right = any_perpendicular(center);
    }
    let right = normalize_or_up(right);
    let up = cross3(center, right);
    let x = dot3(d, right) / c;
    let y = dot3(d, up) / c;
    let edge = 0.06;
    let mx = smoothstep(half_w + edge, half_w - edge, x.abs());
    let my = smoothstep(half_h + edge, half_h - edge, y.abs());
    let m = mx * my;
    [color[0] * m, color[1] * m, color[2] * m]
}

// ---------------------------------------------------------------------------
// Radiance .hdr (RGBE).
// ---------------------------------------------------------------------------

/// Load a Radiance RGBE `.hdr` file: new-style RLE and flat scanlines,
/// `FORMAT=32-bit_rle_rgbe`, `EXPOSURE=` (divided back out), and the
/// resolution orientations `-Y h +X w` (standard), `+Y`, `-X`.
/// Decode a Radiance `.hdr`. The size its header claims is multiplied out
/// with checked arithmetic, checked against the pixel data the file can hold
/// and charged to `budget` before any pixel buffer is allocated.
pub fn load_hdr(bytes: &[u8], budget: &mut DecodeBudget) -> Result<EnvMap, String> {
    let mut pos = 0usize;
    let next_line = |pos: &mut usize| -> Option<String> {
        if *pos >= bytes.len() {
            return None;
        }
        let start = *pos;
        while *pos < bytes.len() && bytes[*pos] != b'\n' {
            *pos += 1;
        }
        let line = String::from_utf8_lossy(&bytes[start..*pos]).trim_end_matches('\r').to_string();
        *pos += 1;
        Some(line)
    };
    let first = next_line(&mut pos).ok_or("empty .hdr file")?;
    if !first.starts_with("#?") {
        return Err(format!("not a Radiance .hdr file (first line `{}`; expected `#?RADIANCE`)", first.chars().take(40).collect::<String>()));
    }
    let mut exposure = 1.0f32;
    loop {
        let line = next_line(&mut pos).ok_or(".hdr header has no blank line before the resolution")?;
        if line.is_empty() {
            break;
        }
        if let Some(f) = line.strip_prefix("FORMAT=") {
            if f.trim() != "32-bit_rle_rgbe" {
                return Err(format!(".hdr format `{}` is not supported (only 32-bit_rle_rgbe; XYZE is not)", f.trim()));
            }
        } else if let Some(e) = line.strip_prefix("EXPOSURE=") {
            if let Ok(v) = e.trim().parse::<f32>() {
                if v > 0.0 && v.is_finite() {
                    exposure *= v;
                }
            }
        }
    }
    let res = next_line(&mut pos).ok_or(".hdr has no resolution line")?;
    let parts: Vec<&str> = res.split_whitespace().collect();
    if parts.len() != 4 {
        return Err(format!(".hdr resolution line `{res}` is not like `-Y 512 +X 1024`"));
    }
    let (flip_y, flip_x) = match (parts[0], parts[2]) {
        ("-Y", "+X") => (false, false),
        ("+Y", "+X") => (true, false),
        ("-Y", "-X") => (false, true),
        ("+Y", "-X") => (true, true),
        _ => return Err(format!(".hdr orientation `{res}` is not supported (only Y-major: -Y/+Y h ±X w)")),
    };
    let height: usize = parts[1].parse().map_err(|_| format!(".hdr height `{}` is not a number", parts[1]))?;
    let width: usize = parts[3].parse().map_err(|_| format!(".hdr width `{}` is not a number", parts[3]))?;
    let pixels = width.checked_mul(height).filter(|&n| width != 0 && height != 0 && n <= 64 * 1024 * 1024);
    let Some(pixels) = pixels else {
        return Err(format!(".hdr size {width}x{height} is out of range"));
    };
    // Every scanline takes at least 4 bytes (an RLE header, or one flat
    // pixel) plus 2 per 128-pixel run of each channel: a tiny file claiming
    // a huge image is refused here, not after allocating for it.
    let min_scanline = if (8..=0x7fff).contains(&width) { 4 + 8 * width.div_ceil(128) } else { 4 * width };
    if height.checked_mul(min_scanline).is_none_or(|need| need > bytes.len() - pos.min(bytes.len())) {
        return Err(format!(".hdr claims {width}x{height} pixels but the file is too short to hold them"));
    }
    // RGBE staging (4 B) + a scanline (4 B/px) + the float image (16 B/px).
    let staging = DecodeBudget::image_bytes(pixels as u64, 1, 4).map_err(|e| e.to_string())?;
    let scanline = DecodeBudget::image_bytes(width as u64, 1, 4).map_err(|e| e.to_string())?;
    let image = DecodeBudget::image_bytes(pixels as u64, 1, 16).map_err(|e| e.to_string())?;
    budget.charge(staging + scanline, "the .hdr staging buffers").map_err(|e| e.to_string())?;
    budget.charge(image, "the decoded .hdr").map_err(|e| {
        budget.release(staging + scanline);
        e.to_string()
    })?;
    let result = decode_hdr_pixels(bytes, pos, width, height, flip_x, flip_y, exposure);
    budget.release(staging + scanline);
    if result.is_err() {
        budget.release(image);
    }
    result
}

fn decode_hdr_pixels(bytes: &[u8], mut pos: usize, width: usize, height: usize, flip_x: bool, flip_y: bool, exposure: f32) -> Result<EnvMap, String> {
    let mut rgbe = vec![[0u8; 4]; width * height];
    let mut scan = vec![[0u8; 4]; width];
    for y in 0..height {
        read_scanline(bytes, &mut pos, &mut scan).map_err(|e| format!(".hdr scanline {y}: {e}"))?;
        rgbe[y * width..(y + 1) * width].copy_from_slice(&scan);
    }
    let inv = 1.0 / exposure;
    let mut data = vec![[0.0f32; 4]; width * height];
    for y in 0..height {
        let sy = if flip_y { height - 1 - y } else { y };
        for x in 0..width {
            let sx = if flip_x { width - 1 - x } else { x };
            let c = rgbe_to_float(rgbe[sy * width + sx]);
            data[y * width + x] = [c[0] * inv, c[1] * inv, c[2] * inv, 1.0];
        }
    }
    Ok(EnvMap { width, height, data })
}

fn read_scanline(bytes: &[u8], pos: &mut usize, out: &mut [[u8; 4]]) -> Result<(), String> {
    let width = out.len();
    let take = |pos: &mut usize| -> Result<u8, String> {
        let b = *bytes.get(*pos).ok_or("file ends inside the pixel data")?;
        *pos += 1;
        Ok(b)
    };
    let new_rle = (8..=0x7fff).contains(&width)
        && bytes.get(*pos) == Some(&2)
        && bytes.get(*pos + 1) == Some(&2)
        && bytes.get(*pos + 2).is_some_and(|b| b & 0x80 == 0);
    if !new_rle {
        for px in out.iter_mut() {
            *px = [take(pos)?, take(pos)?, take(pos)?, take(pos)?];
        }
        return Ok(());
    }
    *pos += 2;
    let w = ((take(pos)? as usize) << 8) | take(pos)? as usize;
    if w != width {
        return Err(format!("RLE scanline width {w} != image width {width}"));
    }
    for ch in 0..4 {
        let mut x = 0;
        while x < width {
            let count = take(pos)? as usize;
            if count > 128 {
                let n = count - 128;
                if x + n > width {
                    return Err("RLE run overflows the scanline".into());
                }
                let v = take(pos)?;
                for px in &mut out[x..x + n] {
                    px[ch] = v;
                }
                x += n;
            } else {
                if count == 0 || x + count > width {
                    return Err("bad RLE literal count".into());
                }
                for px in &mut out[x..x + count] {
                    px[ch] = take(pos)?;
                }
                x += count;
            }
        }
    }
    Ok(())
}

/// RGBE bytes to linear radiance.
pub fn rgbe_to_float(p: [u8; 4]) -> [f32; 3] {
    if p[3] == 0 {
        return [0.0; 3];
    }
    let f = 2f32.powi(p[3] as i32 - 136);
    [p[0] as f32 * f, p[1] as f32 * f, p[2] as f32 * f]
}

/// Linear radiance to RGBE bytes (for tests and writers).
pub fn float_to_rgbe(c: [f32; 3]) -> [u8; 4] {
    let m = c[0].max(c[1]).max(c[2]);
    if m < 1e-32 {
        return [0; 4];
    }
    let e = m.log2().floor() as i32 + 1;
    let scale = 256.0 / 2f32.powi(e);
    [
        (c[0] * scale).clamp(0.0, 255.0) as u8,
        (c[1] * scale).clamp(0.0, 255.0) as u8,
        (c[2] * scale).clamp(0.0, 255.0) as u8,
        (e + 128).clamp(0, 255) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Prefiltering.
// ---------------------------------------------------------------------------

/// The roughness atlas the GPU samples for specular IBL: `levels` equirect
/// maps of `width` x `width / 2` stacked vertically, level k at roughness
/// `k / (levels - 1)` (level 0 = the sharp environment). To sample
/// roughness r along `dir`: `uv = dir_to_equirect_uv(dir)`,
/// `lf = r * (levels - 1)`, sample levels `floor(lf)` and `ceil(lf)` at
/// `v_atlas = (k + clamp(v, 0.5/h, 1 - 0.5/h)) / levels` (the clamp keeps the
/// bilinear filter from bleeding across levels) and blend by `fract(lf)`.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvAtlas {
    pub width: usize,
    /// `levels * level_height`.
    pub height: usize,
    pub levels: usize,
    pub level_height: usize,
    pub data: Vec<[f32; 4]>,
}

impl EnvAtlas {
    /// One level as an [`EnvMap`] (copied).
    pub fn level(&self, k: usize) -> EnvMap {
        let k = k.min(self.levels - 1);
        let n = self.width * self.level_height;
        EnvMap { width: self.width, height: self.level_height, data: self.data[k * n..(k + 1) * n].to_vec() }
    }

    /// Flat RGBA f32 texels, row-major, for `TextureFormat::VecRGBAf32`.
    pub fn to_rgba_f32(&self) -> Vec<f32> {
        self.data.iter().flat_map(|c| c.iter().copied()).collect()
    }
}

/// A box-filtered mip chain of an equirect map, sampled trilinearly.
struct Pyramid {
    levels: Vec<EnvMap>,
}

impl Pyramid {
    fn new(env: &EnvMap, max_width: usize) -> Self {
        let mut w = env.width.min(max_width).max(4);
        // Round down to a power of two so halving stays exact.
        w = 1usize << (usize::BITS - 1 - w.leading_zeros());
        let mut levels = vec![env.resized(w)];
        while levels.last().unwrap().width > 4 {
            let next = levels.last().unwrap().resized(levels.last().unwrap().width / 2);
            levels.push(next);
        }
        Self { levels }
    }

    fn sample(&self, uv: [f32; 2], lod: f32) -> [f32; 4] {
        let max = (self.levels.len() - 1) as f32;
        let lod = lod.clamp(0.0, max);
        let l0 = lod.floor() as usize;
        let l1 = (l0 + 1).min(self.levels.len() - 1);
        let f = lod - l0 as f32;
        let a = self.levels[l0].sample_uv(uv);
        if f <= 0.0 || l0 == l1 {
            return a;
        }
        let b = self.levels[l1].sample_uv(uv);
        [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
    }
}

/// Deterministic 2D low-discrepancy point i of n.
fn hammersley(i: u32, n: u32) -> [f32; 2] {
    let bits = i.reverse_bits();
    [(i as f32 + 0.5) / n as f32, bits as f32 * (1.0 / 4_294_967_296.0)]
}

/// Tangent-space GGX samples for roughness `r` (N = V = +z): direction,
/// NdotL weight and pyramid lod (filtered importance sampling).
fn ggx_samples(r: f32, count: u32, texel_solid_angle: f32) -> Vec<([f32; 3], f32, f32)> {
    let a = (r * r).max(1e-4);
    let a2 = a * a;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let [u1, u2] = hammersley(i, count);
        let phi = TAU * u1;
        let cos_t = ((1.0 - u2) / (1.0 + (a2 - 1.0) * u2)).max(0.0).sqrt();
        let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
        let h = [sin_t * phi.cos(), sin_t * phi.sin(), cos_t];
        // L = reflect(-V, H) with V = +z.
        let l = [2.0 * cos_t * h[0], 2.0 * cos_t * h[1], 2.0 * cos_t * h[2] - 1.0];
        let ndl = l[2];
        if ndl <= 0.0 {
            continue;
        }
        let d = {
            let x = cos_t * cos_t * (a2 - 1.0) + 1.0;
            a2 / (PI * x * x)
        };
        // pdf(L) = D * NdotH / (4 VdotH) = D / 4 with N = V.
        let pdf = (d / 4.0).max(1e-8);
        let sa_sample = 1.0 / (count as f32 * pdf);
        let lod = (0.5 * (sa_sample / texel_solid_angle).log2() + 1.0).max(0.0);
        out.push((l, ndl, lod));
    }
    out
}

/// Prefilter `env` into an [`EnvAtlas`] of `levels` roughness levels, each
/// `base_width` x `base_width / 2` (base_width >= 8, levels 1..=10). GGX
/// lobes (split-sum, N = V = R) importance-sampled with a fixed Hammersley
/// set over a box-filtered pyramid of the source (at most 512 wide), so the
/// result is noise-free enough at 64 samples and deterministic.
pub fn prefilter(env: &EnvMap, base_width: usize, levels: usize) -> EnvAtlas {
    let width = base_width.max(8);
    let level_height = width / 2;
    let levels = levels.clamp(1, 10);
    let pyr = Pyramid::new(env, 512);
    let w0 = pyr.levels[0].width as f32;
    let texel_sa = 4.0 * PI / (w0 * w0 * 0.5);
    let base_lod = (w0 / width as f32).log2().max(0.0);
    let mut data = Vec::with_capacity(width * level_height * levels);
    // Per-texel directions and tangent frames, shared by every level.
    let mut frames = Vec::with_capacity(width * level_height);
    for y in 0..level_height {
        for x in 0..width {
            let uv = [(x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / level_height as f32];
            let n = equirect_uv_to_dir(uv);
            let t = any_perpendicular(n);
            let b = cross3(n, t);
            frames.push((uv, n, t, b));
        }
    }
    for k in 0..levels {
        let r = if levels == 1 { 0.0 } else { k as f32 / (levels - 1) as f32 };
        if k == 0 || r <= 0.0 {
            for (uv, ..) in &frames {
                data.push(pyr.sample(*uv, base_lod));
            }
            continue;
        }
        let samples = ggx_samples(r, 64, texel_sa);
        for (_, n, t, b) in &frames {
            let mut acc = [0.0f32; 3];
            let mut wsum = 0.0f32;
            for (l, w, lod) in &samples {
                let d = [
                    t[0] * l[0] + b[0] * l[1] + n[0] * l[2],
                    t[1] * l[0] + b[1] * l[1] + n[1] * l[2],
                    t[2] * l[0] + b[2] * l[1] + n[2] * l[2],
                ];
                let c = pyr.sample(dir_to_equirect_uv(d), lod.max(base_lod));
                acc[0] += c[0] * w;
                acc[1] += c[1] * w;
                acc[2] += c[2] * w;
                wsum += w;
            }
            let inv = 1.0 / wsum.max(1e-8);
            data.push([acc[0] * inv, acc[1] * inv, acc[2] * inv, 1.0]);
        }
    }
    EnvAtlas { width, height: level_height * levels, levels, level_height, data }
}

// ---------------------------------------------------------------------------
// Spherical-harmonic irradiance.
// ---------------------------------------------------------------------------

/// Real SH basis (bands 0..2) at unit `d`, in the order
/// `Y00, Y1-1(y), Y10(z), Y11(x), Y2-2(xy), Y2-1(yz), Y20, Y21(xz), Y22`.
pub fn sh9_basis(d: [f32; 3]) -> [f32; 9] {
    let (x, y, z) = (d[0], d[1], d[2]);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// Irradiance SH of `env`: the radiance projected on [`sh9_basis`] and
/// already convolved with the clamped cosine (band factors π, 2π/3, π/4),
/// so irradiance along a unit normal n is `E(n) = Σ c_i · Y_i(n)` (see
/// [`sh9_irradiance`]) and Lambert diffuse is `albedo · E(n) / π`.
/// A constant radiance L gives `E = π L` everywhere.
pub fn sh9(env: &EnvMap) -> [[f32; 3]; 9] {
    let src = if env.width > 256 { env.resized(256) } else { env.clone() };
    let mut c = [[0.0f64; 3]; 9];
    for y in 0..src.height {
        let sa = src.texel_solid_angle(y) as f64;
        for x in 0..src.width {
            let d = equirect_uv_to_dir([(x as f32 + 0.5) / src.width as f32, (y as f32 + 0.5) / src.height as f32]);
            let basis = sh9_basis(d);
            let px = src.data[y * src.width + x];
            for (i, b) in basis.iter().enumerate() {
                for k in 0..3 {
                    c[i][k] += px[k] as f64 * *b as f64 * sa;
                }
            }
        }
    }
    let band = [PI, TAU / 3.0, TAU / 3.0, TAU / 3.0, PI / 4.0, PI / 4.0, PI / 4.0, PI / 4.0, PI / 4.0];
    let mut out = [[0.0f32; 3]; 9];
    for i in 0..9 {
        for k in 0..3 {
            out[i][k] = (c[i][k] * band[i] as f64) as f32;
        }
    }
    out
}

/// Irradiance along unit normal `n` from [`sh9`] coefficients.
pub fn sh9_irradiance(c: &[[f32; 3]; 9], n: [f32; 3]) -> [f32; 3] {
    let b = sh9_basis(n);
    let mut e = [0.0f32; 3];
    for i in 0..9 {
        for k in 0..3 {
            e[k] += c[i][k] * b[i];
        }
    }
    e
}

/// The render lane's IBL texture (see `builtin.rs`): the prefiltered atlas
/// rows, then one meta row holding the nine SH9 irradiance coefficients
/// (texels 0..8) and `(levels, level_height, intensity, rotation)` in texel
/// 9. RGBA f32, row-major, for `TextureFormat::VecRGBAf32`.
#[derive(Clone, Debug, PartialEq)]
pub struct IblTexture {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// Build the lane texture for an environment. `intensity` scales both the
/// specular and the diffuse term; `rotation_deg` turns the environment
/// about +Y.
pub fn ibl_texture(env: &EnvMap, intensity: f32, rotation_deg: f32) -> IblTexture {
    let atlas = prefilter(env, 256, 6);
    pack_ibl(&atlas, &sh9(env), intensity, rotation_deg)
}

/// An environment's prefiltered atlas and irradiance, kept for the process
/// by `key` (a name for this one map's content): every scene lighting with
/// the same map shares one CPU prefilter, which takes a large part of a
/// second (more on a browser's UI thread). A host can run it ahead of the
/// first draw, behind its loader, on a worker: the store is taken with
/// `try_lock` and a spin (held for a map lookup), never a wait, which a
/// browser's main thread may not do.
pub fn prefiltered(key: u64, env: &EnvMap) -> std::sync::Arc<(EnvAtlas, [[f32; 3]; 9])> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
    type Kept = Option<HashMap<u64, Arc<(EnvAtlas, [[f32; 3]; 9])>>>;
    static KEPT: Mutex<Kept> = Mutex::new(None);
    fn kept() -> MutexGuard<'static, Kept> {
        loop {
            match KEPT.try_lock() {
                Ok(guard) => return guard,
                Err(TryLockError::Poisoned(error)) => return error.into_inner(),
                Err(TryLockError::WouldBlock) => std::hint::spin_loop(),
            }
        }
    }
    if let Some(kept) = kept().as_ref().and_then(|kept| kept.get(&key)) {
        return kept.clone();
    }
    let made = Arc::new((prefilter(env, 256, 6), sh9(env)));
    kept().get_or_insert_with(HashMap::new).insert(key, made.clone());
    made
}

/// [`ibl_texture`] for a map named by `key`, its prefilter shared through
/// [`prefiltered`].
pub fn ibl_texture_kept(key: u64, env: &EnvMap, intensity: f32, rotation_deg: f32) -> IblTexture {
    let kept = prefiltered(key, env);
    pack_ibl(&kept.0, &kept.1, intensity, rotation_deg)
}

pub fn pack_ibl(atlas: &EnvAtlas, sh: &[[f32; 3]; 9], intensity: f32, rotation_deg: f32) -> IblTexture {
    let width = atlas.width.max(10);
    let height = atlas.height + 1;
    let mut data = vec![0.0f32; width * height * 4];
    for y in 0..atlas.height {
        for x in 0..atlas.width {
            let c = atlas.data[y * atlas.width + x];
            data[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
    }
    let meta = atlas.height * width * 4;
    for (i, c) in sh.iter().enumerate() {
        data[meta + i * 4..meta + i * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
    }
    let intensity = if intensity.is_finite() { intensity.max(0.0) } else { 0.0 };
    let rotation = if rotation_deg.is_finite() { rotation_deg.to_radians() } else { 0.0 };
    data[meta + 36..meta + 40].copy_from_slice(&[atlas.levels as f32, atlas.level_height as f32, intensity, rotation]);
    IblTexture { width, height, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rle_channel(vals: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        while i < vals.len() {
            let mut run = 1;
            while i + run < vals.len() && vals[i + run] == vals[i] && run < 127 {
                run += 1;
            }
            if run >= 3 {
                out.push(128 + run as u8);
                out.push(vals[i]);
                i += run;
            } else {
                let start = i;
                let mut n = 0;
                while i < vals.len() && n < 128 {
                    let mut r = 1;
                    while i + r < vals.len() && vals[i + r] == vals[i] && r < 3 {
                        r += 1;
                    }
                    if r >= 3 {
                        break;
                    }
                    i += 1;
                    n += 1;
                }
                out.push(n as u8);
                out.extend_from_slice(&vals[start..start + n]);
            }
        }
    }

    fn write_hdr(w: usize, h: usize, px: &[[f32; 3]], rle: bool, exposure: Option<f32>) -> Vec<u8> {
        let mut out = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n".to_vec();
        if let Some(e) = exposure {
            out.extend_from_slice(format!("EXPOSURE={e}\n").as_bytes());
        }
        out.extend_from_slice(format!("\n-Y {h} +X {w}\n").as_bytes());
        for y in 0..h {
            let row: Vec<[u8; 4]> = (0..w).map(|x| float_to_rgbe(px[y * w + x])).collect();
            if rle {
                out.extend_from_slice(&[2, 2, (w >> 8) as u8, (w & 255) as u8]);
                for ch in 0..4 {
                    let vals: Vec<u8> = row.iter().map(|p| p[ch]).collect();
                    rle_channel(&vals, &mut out);
                }
            } else {
                for p in row {
                    out.extend_from_slice(&p);
                }
            }
        }
        out
    }

    fn pixels(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let f = i as f32;
                if i % 5 == 0 {
                    [2.0, 2.0, 2.0]
                } else {
                    [0.01 + f * 0.37, 0.5 + (f * 0.13).sin().abs() * 20.0, 100.0 / (1.0 + f)]
                }
            })
            .collect()
    }

    fn check(env: &EnvMap, px: &[[f32; 3]], scale: f32) {
        for (i, p) in px.iter().enumerate() {
            let got = env.data[i];
            for k in 0..3 {
                let want = p[k] * scale;
                let m = p[0].max(p[1]).max(p[2]) * scale;
                assert!((got[k] - want).abs() <= m / 128.0 + 1e-6, "px {i} ch {k}: {} vs {want}", got[k]);
            }
        }
    }

    #[test]
    fn rgbe_flat_roundtrip() {
        let px = pixels(3, 2);
        let env = load_hdr(&write_hdr(3, 2, &px, false, None), &mut DecodeBudget::default()).unwrap();
        assert_eq!((env.width, env.height), (3, 2));
        check(&env, &px, 1.0);
    }

    #[test]
    fn rgbe_rle_roundtrip_with_exposure() {
        let px = pixels(20, 3);
        let env = load_hdr(&write_hdr(20, 3, &px, true, Some(2.0)), &mut DecodeBudget::default()).unwrap();
        assert_eq!((env.width, env.height), (20, 3));
        check(&env, &px, 0.5);
    }

    #[test]
    fn hdr_rejects_garbage() {
        assert!(load_hdr(b"hello", &mut DecodeBudget::default()).is_err());
        assert!(load_hdr(b"#?RADIANCE\n\n-Y 2 +X 2\n\x01", &mut DecodeBudget::default()).is_err());
    }

    #[test]
    fn hdr_size_bombs_fail_before_allocating() {
        // A few bytes claiming 8192x8192 (or an overflowing size) are refused
        // on the header, and nothing is charged.
        let mut budget = DecodeBudget::default();
        for res in ["-Y 8192 +X 8192", "-Y 4294967296 +X 4294967296", "-Y 18446744073709551615 +X 2", "-Y 0 +X 5"] {
            let bytes = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n{res}\n\x02\x02\x20\x00");
            assert!(load_hdr(bytes.as_bytes(), &mut budget).is_err(), "{res}");
            assert_eq!(budget.used(), 0, "{res}");
        }
        // A real image over the budget is refused; within it, it is charged.
        let px = pixels(20, 3);
        let file = write_hdr(20, 3, &px, true, None);
        assert!(load_hdr(&file, &mut DecodeBudget::new(20 * 3 * 16)).is_err());
        let mut budget = DecodeBudget::new(1 << 20);
        load_hdr(&file, &mut budget).unwrap();
        assert_eq!(budget.used(), 20 * 3 * 16, "the float image stays charged, the staging is given back");
    }

    #[test]
    fn uv_dir_inverse() {
        assert!((dir_to_equirect_uv([0.0, 0.0, -1.0])[0] - 0.5).abs() < 1e-6);
        assert!((dir_to_equirect_uv([1.0, 0.0, 0.0])[0] - 0.75).abs() < 1e-6);
        for i in 0..200 {
            let uv = [((i * 37) % 100) as f32 / 100.0 + 0.005, ((i * 13) % 97) as f32 / 97.0 * 0.98 + 0.01];
            let back = dir_to_equirect_uv(equirect_uv_to_dir(uv));
            assert!((back[0] - uv[0]).abs() < 1e-4 && (back[1] - uv[1]).abs() < 1e-4, "{uv:?} {back:?}");
        }
    }

    fn lum_stats(env: &EnvMap) -> (f32, f32) {
        let m = env.mean();
        let mean = 0.2126 * m[0] + 0.7152 * m[1] + 0.0722 * m[2];
        let mut var = 0.0f64;
        let mut w = 0.0f64;
        for y in 0..env.height {
            let sa = env.texel_solid_angle(y) as f64;
            for x in 0..env.width {
                let l = luminance(env.data[y * env.width + x]) - mean;
                var += (l * l) as f64 * sa;
                w += sa;
            }
        }
        (mean, (var / w) as f32)
    }

    #[test]
    fn prefilter_smooths_and_preserves_energy() {
        let env = EnvMap::procedural(&EnvPreset::Studio, 256, 1.0, 0.0);
        let t = std::time::Instant::now();
        let atlas = prefilter(&env, 128, 6);
        eprintln!("prefilter 128x6 from 256: {:?}", t.elapsed());
        assert_eq!((atlas.width, atlas.height, atlas.levels), (128, 384, 6));
        let l0 = atlas.level(0);
        let src = env.resized(128);
        let (m_src, _) = lum_stats(&src);
        let (m0, v0) = lum_stats(&l0);
        assert!((m0 - m_src).abs() / m_src < 0.01, "{m0} {m_src}");
        let mut prev_var = v0;
        for k in 1..6 {
            let (m, v) = lum_stats(&atlas.level(k));
            assert!((m - m0).abs() / m0 < 0.06, "level {k} mean {m} vs {m0}");
            assert!(v <= prev_var * 1.02, "level {k} var {v} > {prev_var}");
            prev_var = v;
        }
        assert!(prev_var < v0 * 0.5);
        assert_eq!(atlas, prefilter(&env, 128, 6));
    }

    #[test]
    fn prefilter_speed_budget() {
        let env = EnvMap::procedural(&EnvPreset::Sunset, 512, 1.0, 0.0);
        let t = std::time::Instant::now();
        let atlas = prefilter(&env, 256, 6);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("prefilter 256x6 from 512: {ms:.0} ms");
        assert_eq!(atlas.data.len(), 256 * 128 * 6);
        assert!(atlas.data.iter().all(|c| c.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn sh9_constant_env_is_constant_irradiance() {
        let env = EnvMap::constant(64, [0.5, 1.0, 2.0]);
        let c = sh9(&env);
        for d in [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.5773, -0.5773, 0.5773]] {
            let e = sh9_irradiance(&c, d);
            for (k, l) in [0.5, 1.0, 2.0].iter().enumerate() {
                assert!((e[k] - PI * l).abs() / (PI * l) < 0.01, "{d:?} {e:?}");
            }
        }
    }

    #[test]
    fn sh9_sees_the_bright_side() {
        let env = EnvPreset::Gradient { top: [2.0; 3], horizon: [0.5; 3], bottom: [0.0; 3] };
        let c = sh9(&EnvMap::procedural(&env, 64, 1.0, 0.0));
        assert!(sh9_irradiance(&c, [0.0, 1.0, 0.0])[0] > 2.0 * sh9_irradiance(&c, [0.0, -1.0, 0.0])[0]);
    }

    #[test]
    fn presets_are_finite_deterministic_and_rotate() {
        for name in EnvPreset::NAMES {
            let p = EnvPreset::by_name(name).unwrap();
            let a = EnvMap::procedural(&p, 64, 1.0, 0.0);
            assert_eq!(a, EnvMap::procedural(&p, 64, 1.0, 0.0));
            assert!(a.data.iter().all(|c| c.iter().all(|v| v.is_finite() && *v >= 0.0)), "{name}");
            assert!(a.mean()[0] > 0.0, "{name}");
        }
        // The sunset's sun is at yaw 0 (-z); rotating by 90° moves it.
        let s0 = EnvMap::procedural(&EnvPreset::Sunset, 256, 1.0, 0.0).brightest_direction();
        assert!(s0[2] < -0.9, "{s0:?}");
        let s90 = EnvMap::procedural(&EnvPreset::Sunset, 256, 1.0, 90.0).brightest_direction();
        assert!(s90[0].abs() > 0.9, "{s90:?}");
    }

    #[test]
    fn the_lane_texture_carries_the_atlas_then_sh9_and_its_meta() {
        let env = EnvMap::procedural(&EnvPreset::Studio, 64, 1.0, 0.0);
        let atlas = prefilter(&env, 32, 4);
        let sh = sh9(&env);
        let t = pack_ibl(&atlas, &sh, 2.0, 90.0);
        assert_eq!((t.width, t.height), (atlas.width, atlas.height + 1));
        assert_eq!(t.data.len(), t.width * t.height * 4);
        let meta = atlas.height * t.width * 4;
        assert_eq!(&t.data[meta..meta + 3], &sh[0]);
        assert_eq!(t.data[meta + 36], atlas.levels as f32);
        assert_eq!(t.data[meta + 37], atlas.level_height as f32);
        assert_eq!(t.data[meta + 38], 2.0);
        assert!((t.data[meta + 39] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(&t.data[0..3], &atlas.data[0][0..3]);
    }
}
