// Source: src/CORTAOParameters.h, CORTAO v1.0.2, commit f1b18d7772.
// MIT License
//
// Copyright (c) 2026 Daniel Isheden
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Modified:
// - Rust API, f64 CPU math, f32 uniforms, unpacked resolution, radius default 0.6.
// - Decompose clip cameras into a rigid view and projection with one skew term;
//   use y-down UVs and positive ortho depth, rejecting invalid camera matrices
//   and non-affine planar homographies that the ortho terms cannot represent.
// - Include radius in orthographic mip bias; clamp slope compensation to >= 1
//   and the apparent-radius coefficient below 1 even after f32 rounding.
// - Use backfaceBoost 0.5 from the struct; the README recommends 0.0.
// - Add atlas layout, inverse child rule, and tests; the child rule and its
//   forward test model derive from src/shaders/CORTAOPrepass.h at the same commit.
// - Check image sizes and coordinate overflow; keep generated mip axes >= 2.

//! Camera decomposition, pass uniforms, and packed-depth atlas coordinates.

/// Resolution of the internal AO chain; the output always has the input's size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CortaoResolution {
    /// Half above about 2560x1600 device pixels (4K and up), Full below.
    #[default]
    Auto,
    Full,
    Half,
}

/// Auto's switch to Half: more device pixels than 2560x1600.
pub const CORTAO_AUTO_HALF_PIXELS: u64 = 2560 * 1600;

impl CortaoResolution {
    /// Whether a frame of `[width, height]` device pixels runs the half-size chain.
    pub fn is_half(self, [width, height]: [u32; 2]) -> bool {
        match self {
            Self::Auto => u64::from(width) * u64::from(height) > CORTAO_AUTO_HALF_PIXELS,
            Self::Full => false,
            Self::Half => true,
        }
    }

    /// `full`, `half` or `auto` (a host's setting or environment knob).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "auto" => Some(Self::Auto),
            "full" => Some(Self::Full),
            "half" => Some(Self::Half),
            _ => None,
        }
    }
}

/// User-facing CORTAO controls. Distances use the camera's world frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CortaoConfig {
    /// Half trades fine AO detail for fewer shaded pixels, then upsamples with
    /// depth-aware weights; Auto (the default) takes Half only for 4K-class frames.
    pub resolution: CortaoResolution,
    /// maxRadius: positive AO radius in world units; defaults to 0.6.
    pub radius: f32,
    /// Not in CORTAO: the smallest relief that occludes, in world units. A tap counts only
    /// when it rises more than this above the centre's tangent plane, so surface texture
    /// lower than that (a print's beads, a bead's layer lines) darkens nothing while a
    /// contact or an overhang still does. Above 0 the centre's plane also comes from the
    /// depth twice this far away on each axis, so the relief tilts neither the plane nor the
    /// taps. 0 (the default): the surface's own normal, every tap above the plane.
    pub min_occluder: f32,
    /// maxScreenspaceRadiusHeightRatio: largest disc radius / image height (0.2).
    pub max_screen_radius: f32,
    /// mipBias: sampling mip offset; lower values read finer levels (-4.0).
    pub mip_bias: f32,
    /// maxSlopeCompensation: maximum enlargement on slopes, at least 1 (4.0).
    pub max_slope_compensation: f32,
    /// backfaceThreshold: normal/direction dot threshold for backfaces, 0..=1 (0.5).
    pub backface_threshold: f32,
    /// backfaceBoost: extra backface slope compensation, 0..=1. Defaults to 0.5
    /// as in the reference struct; the README instead recommends 0.0.
    pub backface_boost: f32,
    /// filterDepthSensitivity: rejection of depth-plane error, nonnegative (512.0).
    pub filter_depth_sensitivity: f32,
    /// filterNormalSensitivity: rejection of normal differences, nonnegative (16.0).
    pub filter_normal_sensitivity: f32,
}

impl Default for CortaoConfig {
    fn default() -> Self {
        Self {
            resolution: CortaoResolution::Auto,
            radius: 0.6,
            min_occluder: 0.0,
            max_screen_radius: 0.2,
            mip_bias: -4.0,
            max_slope_compensation: 4.0,
            backface_threshold: 0.5,
            backface_boost: 0.5,
            filter_depth_sensitivity: 512.0,
            filter_normal_sensitivity: 16.0,
        }
    }
}

/// A column-major clip camera expressed in CORTAO's rigid, -z-forward view frame.
/// Depth is clip z/w in 0..=1; UV is `(0.5*ndc.x + 0.5, 0.5 - 0.5*ndc.y)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CortaoCamera {
    /// Whether projection is affine rather than perspective.
    pub ortho: bool,
    /// (ndc.x, ndc.y, depth, 1) -> homogeneous view position; divide xyz by w.
    pub view_from_clip: [f32; 16],
    /// Packed-depth law over the clip volume's depth extent ([`DEPTH_CODES`]).
    /// Law 1 is linear for ortho: `[1, d_lo, s, 1 / s]`, with a step of about
    /// extent * 2^-22. Law 0 is pseudo-log for perspective: `[0, d_lo, 1 / d_lo, k]`.
    /// It rounds the f32 bits of `max(depth / d_lo, 1)` above 1.0 by `k` bits;
    /// a step is `d_lo * 2^(floor(log2(depth / d_lo)) + k - 23)`.
    /// Closed windows use the least `k` in 0..=9 that fits the far/near ratio
    /// (capped at 2^126). The code window ends at
    /// `d_lo * from_bits(((DEPTH_CODES - 1) << k) + 0x3F800000)`;
    /// an open high end uses k = 6, about 31.75 binades in float-bit space.
    pub depth_code: [f32; 4],
    /// Rigid world -> view, determinant +/-1, last row (0, 0, 0, 1).
    pub view_from_world: [f32; 16],
    /// Horizontal projection scale in texture coordinates.
    pub px: f32,
    /// Vertical projection scale in y-down texture coordinates.
    pub py: f32,
    /// Horizontal z coefficient (perspective) or constant offset (orthographic).
    pub pz: f32,
    /// Vertical z coefficient (perspective) or constant offset (orthographic).
    pub pw: f32,
    /// Contribution of view y to the horizontal texture coordinate.
    pub skew: f32,
}

impl CortaoCamera {
    /// Factor a world-to-clip matrix using f64 RQ decomposition.
    ///
    /// Perspective: `u = (px*x + skew*y + pz*z)/(-z)`,
    /// `v = (py*y + pw*z)/(-z)`. Orthographic:
    /// `u = px*x + skew*y + pz`, `v = py*y + pw`.
    /// Returns `None` for non-finite, singular, or unrepresentable cameras.
    /// In particular, a projective 2D map with a nonconstant w row and an eye
    /// at infinity cannot be represented by these orthographic parameters.
    pub fn from_clip(clip_from_world: &[f32; 16]) -> Option<Self> {
        let camera = Camera64::from_clip(clip_from_world)?;
        let depth_code = depth_code(&camera.view_from_clip, camera.ortho)?;
        let view_from_clip = camera.view_from_clip.map(|x| x as f32);
        let view_from_world = camera.view_from_world.map(|x| x as f32);
        let [px, py, pz, pw, skew] = camera.projection.map(|x| x as f32);
        if !view_from_clip
            .iter()
            .chain(&view_from_world)
            .all(|x| x.is_finite())
            || ![px, py, pz, pw, skew].iter().all(|x| x.is_finite())
            || px <= 0.0
            || py >= 0.0
        {
            return None;
        }
        Some(Self {
            ortho: camera.ortho,
            view_from_clip,
            depth_code,
            view_from_world,
            px,
            py,
            pz,
            pw,
            skew,
        })
    }
}

/// Codes 0..DEPTH_CODES cover the depth window, rounded to nearest: 0 is d_lo.
/// A code is stored above the ten normal bits as the sign bit
/// (code >= DEPTH_CODES / 2) and the 21-bit field `code % (DEPTH_CODES / 2)
/// + 8192`, whose exponent stays in 1..=254: the R32F texel is always a normal
/// float (r32f_low_bits proves those survive) and never 0, no surface. Linear
/// law 1 ends at d_hi, with steps extent / (DEPTH_CODES - 1), about extent * 2^-22.
/// Pseudo-log law 0 rounds the f32 bits of max(depth / d_lo, 1) above 1.0 by
/// k bits; its steps are d_lo * 2^(floor(log2(depth / d_lo)) + k - 23).
/// The least k in 0..=9 fits the closed far/near ratio (capped at 2^126), and
/// the last code decodes to d_lo * from_bits(((DEPTH_CODES - 1) << k) + 0x3F800000).
/// An open high end uses k = 6: about 31.75 binades in float-bit space.
const DEPTH_CODES: u32 = 2 * 254 * 8192;

/// Keep the existing window refusals, then choose linear law 1 for ortho or
/// pseudo-log law 0 for perspective. For a closed perspective window, k is the
/// least shift in 0..=9 for which (bits(f32(min(d_hi / d_lo, 2^126))) - bits(1.0))
/// >> k fits DEPTH_CODES - 1. Open windows use k = 6 (about 31.75 float-bit
/// binades), rather than the former logarithmic window's 32 binades.
/// Law 0 returns [0, d_lo, 1 / d_lo, k]; code c decodes to
/// d_lo * from_bits((c << k) + 0x3F800000), up to c = DEPTH_CODES - 1.
/// Its step at depth d is d_lo * 2^(floor(log2(d / d_lo)) + k - 23).
fn depth_code(view_from_clip: &[f64; 16], ortho: bool) -> Option<[f32; 4]> {
    let (mut d_lo, mut d_hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut w_min, mut w_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut open_high = false;
    for corner in 0..8 {
        let h = mul_vec(
            view_from_clip,
            [
                if corner & 1 == 0 { -1.0 } else { 1.0 },
                if corner & 2 == 0 { -1.0 } else { 1.0 },
                if corner & 4 == 0 { 0.0 } else { 1.0 },
                1.0,
            ],
        );
        let depth = -h[2] / h[3];
        let open = h[3] <= 0.0 || !depth.is_finite();
        if ortho {
            // The orthographic decomposition already requires a closed depth span.
            if open {
                return None;
            }
            d_lo = d_lo.min(depth);
            d_hi = d_hi.max(depth);
        } else {
            // Perspective homogeneous view z is the same negative constant at
            // every corner: depth = 1 / (scale * w). Since w is affine over
            // the volume, its largest corner value gives the closed low end.
            // Its smallest gives the high end unless w reaches/crosses zero
            // (or depth is non-finite), in which case the high end is open.
            if h[3] > w_max {
                w_max = h[3];
                d_lo = depth;
            }
            if h[3] < w_min {
                w_min = h[3];
                d_hi = depth;
            }
            open_high |= open;
        }
    }
    if (!ortho && w_max <= 0.0) || d_lo <= 0.0 || !d_lo.is_finite() {
        return None;
    }
    if open_high {
        d_hi = d_lo * 2.0_f64.powi(32);
    }
    let extent = if ortho {
        d_hi - d_lo
    } else {
        (d_hi / d_lo).log2()
    };
    let s = f64::from(DEPTH_CODES - 1) / extent;
    if s <= 0.0 || !s.is_finite() {
        return None;
    }
    let code = [if ortho { 1.0 } else { 0.0 }, d_lo, s, 1.0 / s].map(|x| x as f32);
    // Preserve the original f32 representability refusals for both laws.
    if !code.iter().all(|x| x.is_finite()) {
        return None;
    }
    if ortho {
        return Some(code);
    }
    let inv_lo = 1.0 / code[1];
    if !inv_lo.is_finite() {
        return None;
    }
    let k = if open_high {
        6
    } else {
        let ratio = (d_hi / d_lo).min(2.0_f64.powi(126)) as f32;
        let span = ratio.to_bits() - 0x3f80_0000;
        (0..=9).find(|&k| span >> k <= DEPTH_CODES - 1)?
    };
    Some([0.0, code[1], inv_lo, k as f32])
}

// Keep the decomposition before uniform rounding available to the f64 proofs.
struct Camera64 {
    ortho: bool,
    view_from_clip: [f64; 16],
    view_from_world: [f64; 16],
    projection: [f64; 5],
}

impl Camera64 {
    fn from_clip(input: &[f32; 16]) -> Option<Self> {
        let mut m = input.map(f64::from);
        let inverse = inverse4(&m)?;
        let c = [inverse[8], inverse[9], inverse[10], inverse[11]];
        let ortho = c[3].abs() <= 1e-5 * c[0].abs().max(c[1].abs()).max(c[2].abs());
        let scale = if ortho { m[15] } else { length(row3(&m, 3)) };
        if scale == 0.0 || !scale.is_finite() {
            return None;
        }
        for value in &mut m {
            *value /= scale;
        }
        let a1 = row3(&m, 0);
        let a2 = row3(&m, 1);
        let (q1, q2, q3, translation, projection) = if ortho {
            let k22 = length(a2);
            let q2 = unit(a2)?;
            let k12 = dot(a1, q2);
            let r = subtract(a1, scaled(q2, k12));
            let k11 = length(r);
            let q1 = unit(r)?;
            // The eye-at-infinity test also accepts projective 2D maps. Their
            // varying denominator cannot be encoded by an affine projection.
            // Allow only float noise relative to the image projection scales.
            if length(row3(&m, 3)) > 1e-5 * k11.min(k22) {
                return None;
            }
            let mut q3 = cross(q1, q2);
            if dot(q3, row3(&m, 2)) > 0.0 {
                q3 = scaled(q3, -1.0);
            }
            let p0 = dehomogenize(mul_vec(&inverse, [0.0, 0.0, 0.0, 1.0]))?;
            let p1 = dehomogenize(mul_vec(&inverse, [0.0, 0.0, 1.0, 1.0]))?;
            let depth_span = dot(q3, subtract(p1, p0)).abs();
            if depth_span == 0.0 || !depth_span.is_finite() {
                return None;
            }
            let t3 = -dot(q3, p0) - 1e-3 * depth_span;
            (
                q1,
                q2,
                q3,
                [0.0, 0.0, t3],
                [
                    0.5 * k11,
                    -0.5 * k22,
                    0.5 * m[12] + 0.5,
                    0.5 - 0.5 * m[13],
                    0.5 * k12,
                ],
            )
        } else {
            let q3 = scaled(unit(row3(&m, 3))?, -1.0);
            let k23 = dot(a2, q3);
            let r = subtract(a2, scaled(q3, k23));
            let k22 = length(r);
            let q2 = unit(r)?;
            let k13 = dot(a1, q3);
            let k12 = dot(a1, q2);
            let r = subtract(subtract(a1, scaled(q2, k12)), scaled(q3, k13));
            let k11 = length(r);
            let q1 = unit(r)?;
            let t3 = -m[15];
            let t2 = (m[13] - k23 * t3) / k22;
            let t1 = (m[12] - k12 * t2 - k13 * t3) / k11;
            (
                q1,
                q2,
                q3,
                [t1, t2, t3],
                [
                    0.5 * k11,
                    -0.5 * k22,
                    0.5 * k13 - 0.5,
                    -0.5 * k23 - 0.5,
                    0.5 * k12,
                ],
            )
        };
        let mut view_from_world = [0.0; 16];
        for row in 0..3 {
            for col in 0..3 {
                view_from_world[col * 4 + row] = [q1, q2, q3][row][col];
            }
            view_from_world[12 + row] = translation[row];
        }
        view_from_world[15] = 1.0;
        Some(Self {
            ortho,
            view_from_clip: mul_mat(&view_from_world, &inverse),
            view_from_world,
            projection,
        })
    }
}

fn row3(m: &[f64; 16], row: usize) -> [f64; 3] {
    [m[row], m[4 + row], m[8 + row]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn scaled(a: [f64; 3], scale: f64) -> [f64; 3] {
    a.map(|x| x * scale)
}

fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let norm = length(a);
    (norm > 0.0 && norm.is_finite()).then(|| scaled(a, 1.0 / norm))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn mul_vec(m: &[f64; 16], v: [f64; 4]) -> [f64; 4] {
    std::array::from_fn(|row| (0..4).map(|col| m[col * 4 + row] * v[col]).sum())
}

fn mul_mat(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    std::array::from_fn(|i| {
        let col = i / 4;
        let row = i % 4;
        (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum()
    })
}

fn dehomogenize(h: [f64; 4]) -> Option<[f64; 3]> {
    let p = [h[0] / h[3], h[1] / h[3], h[2] / h[3]];
    p.iter().all(|x| x.is_finite()).then_some(p)
}

// Gauss-Jordan with pivoting. Normalize rows first: the pivot product is then
// det(M) / product(row norms), so singularity rejection is homogeneous-scale free.
fn inverse4(m: &[f64; 16]) -> Option<[f64; 16]> {
    if !m.iter().all(|x| x.is_finite()) {
        return None;
    }
    let mut augmented = [[0.0; 8]; 4];
    for row in 0..4 {
        let norm = (0..4)
            .map(|col| m[col * 4 + row].powi(2))
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 || !norm.is_finite() {
            return None;
        }
        for col in 0..4 {
            augmented[row][col] = m[col * 4 + row] / norm;
        }
        augmented[row][4 + row] = 1.0 / norm;
    }
    let mut relative_det = 1.0;
    for col in 0..4 {
        let mut pivot_row = col;
        for row in col + 1..4 {
            if augmented[row][col].abs() > augmented[pivot_row][col].abs() {
                pivot_row = row;
            }
        }
        augmented.swap(col, pivot_row);
        let pivot = augmented[col][col];
        if pivot == 0.0 {
            return None;
        }
        relative_det *= pivot;
        for value in &mut augmented[col] {
            *value /= pivot;
        }
        let pivot_values = augmented[col];
        for (row, values) in augmented.iter_mut().enumerate() {
            if row != col {
                let factor = values[col];
                for k in 0..8 {
                    values[k] -= factor * pivot_values[k];
                }
            }
        }
    }
    if relative_det.abs() <= 64.0 * f64::EPSILON {
        return None;
    }
    let inverse = std::array::from_fn(|i| augmented[i % 4][4 + i / 4]);
    inverse.iter().all(|x| x.is_finite()).then_some(inverse)
}

/// CPU values uploaded to the main pass, with one extra term for camera skew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CortaoMainParams {
    /// Input image width and height in pixels.
    pub resolution: [f32; 2],
    /// Frame/sample seed shared with the packed-depth prepass.
    pub random_seed: u32,
    /// sqrt(2) / (k11 * width); per unit depth in perspective, view units in ortho.
    pub normal_bias: f32,
    /// Maximum world-space AO radius.
    pub radius: f32,
    /// (ux, uy, zx, zy): dir.x = u*ux + v*uxv + zx, dir.y = v*uy + zy.
    /// Perspective position is (dir*d, -d); orthographic is (dir, -d).
    pub unproject: [f32; 4],
    /// uxv, the unprojection term contributed by projection skew.
    pub unproject_skew: f32,
    /// (px, py, pz, pw), with the same meaning as in [`CortaoCamera`].
    pub projection: [f32; 4],
    /// View-y contribution to the projected horizontal coordinate.
    pub projection_skew: f32,
    /// Upper bound on the sampling sphere's radius divided by linear depth; < 1.
    pub max_apparent_circle_radius_coefficient: f32,
    /// Resolution-adjusted mip offset, including world radius for ortho.
    pub mip_bias: f32,
    /// Reciprocal of the maximum slope enlargement, clamped to at most 1.
    pub inverse_max_slope_compensation: f32,
    /// Normal/direction dot threshold for the backface heuristic.
    pub backface_threshold: f32,
    /// Fraction of additional slope compensation applied to backfaces.
    pub backface_boost: f32,
}

/// Derive main-pass uniforms for a positive image size and valid configuration.
pub fn main_params(
    camera: &CortaoCamera,
    width: u32,
    height: u32,
    seed: u32,
    config: &CortaoConfig,
) -> CortaoMainParams {
    assert!(width > 0 && height > 0, "image dimensions must be positive");
    debug_assert!(config.radius.is_finite() && config.radius > 0.0);
    debug_assert!(config.max_screen_radius.is_finite() && config.max_screen_radius > 0.0);
    let [px, py, pz, pw, skew] =
        [camera.px, camera.py, camera.pz, camera.pw, camera.skew].map(f64::from);
    let ux = 1.0 / px;
    let uy = 1.0 / py;
    let uxv = -skew / (px * py);
    let mut zx = (pz - skew * pw / py) / px;
    let mut zy = pw / py;
    if camera.ortho {
        zx = -zx;
        zy = -zy;
    }
    let k11 = 2.0 * px;
    let k22 = -2.0 * py;
    let m = (1.0 / k22) * config.max_screen_radius as f64 * 2.0;
    // f64 can round to 1 for extreme fields of view; f32 rounds there sooner.
    let below_one = f32::from_bits(1.0_f32.to_bits() - 1) as f64;
    let coefficient = (m / (m * m + 1.0).sqrt()).clamp(0.0, below_one) as f32;
    debug_assert!(coefficient < 1.0);
    let mut mip_bias = (k22 * height as f64 * 0.5).log2() + config.mip_bias as f64;
    if camera.ortho {
        mip_bias += (config.radius as f64).log2();
    }
    CortaoMainParams {
        resolution: [width as f32, height as f32],
        random_seed: seed,
        normal_bias: (2.0_f64.sqrt() * (1.0 / k11) / width as f64) as f32,
        radius: config.radius,
        unproject: [ux, uy, zx, zy].map(|x| x as f32),
        unproject_skew: uxv as f32,
        projection: [camera.px, camera.py, camera.pz, camera.pw],
        projection_skew: camera.skew,
        max_apparent_circle_radius_coefficient: coefficient,
        mip_bias: mip_bias as f32,
        inverse_max_slope_compensation: (1.0 / (config.max_slope_compensation as f64).max(1.0))
            as f32,
        backface_threshold: config.backface_threshold,
        backface_boost: config.backface_boost,
    }
}

/// The rigid map from `current`'s view frame to `previous`'s (column-major),
/// which the temporal resolve reprojects a pixel's view point with.
pub fn history_from_view(previous: &CortaoCamera, current: &CortaoCamera) -> Option<[f32; 16]> {
    let previous = previous.view_from_world.map(f64::from);
    let world_from_view = inverse4(&current.view_from_world.map(f64::from))?;
    let m = mul_mat(&previous, &world_from_view).map(|x| x as f32);
    m.iter().all(|x| x.is_finite()).then_some(m)
}

/// CPU values uploaded to the sparse spatial filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CortaoFilterParams {
    /// Input image width and height in pixels.
    pub resolution: [f32; 2],
    /// Rejection strength for relative depth-plane error.
    pub depth_sensitivity: f32,
    /// Rejection strength for packed normal differences.
    pub normal_sensitivity: f32,
}

/// Copy the filter controls for a positive image size.
pub fn filter_params(width: u32, height: u32, config: &CortaoConfig) -> CortaoFilterParams {
    assert!(width > 0 && height > 0, "image dimensions must be positive");
    CortaoFilterParams {
        resolution: [width as f32, height as f32],
        depth_sensitivity: config.filter_depth_sensitivity,
        normal_sensitivity: config.filter_normal_sensitivity,
    }
}

/// Rectangles containing the packed-depth levels in one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CortaoAtlas {
    /// Atlas texture width, equal to the input image width.
    pub width: u32,
    /// Atlas texture height, including the row of smaller levels.
    pub height: u32,
    /// Number of valid entries in `sizes` and `offsets`, in 1..=8.
    pub levels: u32,
    /// Pixel extent of each level; unused entries are zero.
    pub sizes: [[u32; 2]; 8],
    /// Top-left pixel of each level; unused entries are zero.
    pub offsets: [[u32; 2]; 8],
}

impl CortaoAtlas {
    /// Level 0 is (w, h) at (0, 0); level k >= 1 is
    /// (max(w>>k, 1), max(h>>k, 1)) at (sum of widths of levels 1..k-1, h).
    /// The atlas is w by (h + max(h>>1, 1)). The level count is
    /// max(1, min(8, 1 + floor(log2(min(w, h) / 64)))). Both axes of the
    /// last level are >= 2 whenever w and h are >= 2.
    /// Panics for zero dimensions or an atlas height that overflows u32.
    pub fn new(width: u32, height: u32) -> Self {
        Self::with_min(width, height, 64)
    }

    /// Use a different minimum mip dimension, for example 8 in child-rule tests.
    /// A minimum below 2 is raised to 2 so every mip child remains in bounds.
    pub fn with_min(width: u32, height: u32, min_size: u32) -> Self {
        assert!(width > 0 && height > 0, "image dimensions must be positive");
        assert!(min_size > 0, "minimum mip dimension must be positive");
        let mut atlas = Self {
            width,
            height: height
                .checked_add((height >> 1).max(1))
                .expect("atlas height overflows u32"),
            levels: 1,
            sizes: [[0; 2]; 8],
            offsets: [[0; 2]; 8],
        };
        atlas.sizes[0] = [width, height];
        let mut x = 0;
        while atlas.levels < 8 && (width.min(height) >> atlas.levels) >= min_size.max(2) {
            let level = atlas.levels as usize;
            let size = [(width >> level).max(1), (height >> level).max(1)];
            atlas.sizes[level] = size;
            atlas.offsets[level] = [x, height];
            x += size[0];
            atlas.levels += 1;
        }
        atlas
    }
}

/// CORTAO's prepass keeps ONE child per texel: at level k the child of (x, y)
/// is (2x + ((y ^ (seed >> 2k)) & 1), 2y + ((x ^ (seed >> (2k+1))) & 1)).
/// Walk from `level` down to 0 to recover the source pixel. Panics for levels
/// outside 0..8 or coordinates whose level-0 result would overflow u32.
pub fn level0_pixel(level: u32, texel: [u32; 2], seed: u32) -> [u32; 2] {
    assert!(level < 8, "CORTAO supports at most eight levels");
    assert!(
        texel.iter().all(|&coord| coord <= u32::MAX >> level),
        "pixel coordinate overflow"
    );
    let [mut x, mut y] = texel;
    for k in (1..=level).rev() {
        let child_x = 2 * x + ((y ^ (seed >> (2 * k))) & 1);
        let child_y = 2 * y + ((x ^ (seed >> (2 * k + 1))) & 1);
        x = child_x;
        y = child_y;
    }
    [x, y]
}

// Literal parameter formulas from populateMainPassParameters, with [column][row]
// indexing and the v1.0.2 unprojectParamsW sign. Only the arithmetic precision
// and unpacked resolution differ; this deliberately has no camera decomposition.
#[cfg(test)]
fn reference_main_params(
    projection: &[[f32; 4]; 4],
    inverse: &[[f32; 4]; 4],
    width: u32,
    height: u32,
    seed: u32,
    flip_y: bool,
    config: &CortaoConfig,
) -> CortaoMainParams {
    let proj_x = projection[0][0] as f64;
    let proj_y = projection[1][1] as f64;
    let proj_offset_x = projection[2][0] as f64;
    let proj_offset_y = projection[2][1] as f64;
    let inv_x = inverse[0][0] as f64;
    let inv_y = inverse[1][1] as f64;
    let inv_offset_x = inverse[3][0] as f64;
    let inv_offset_y = inverse[3][1] as f64;
    let (unproject, projection) = if flip_y {
        (
            [
                2.0 * inv_x,
                -2.0 * inv_y,
                -inv_x + inv_offset_x,
                inv_y + inv_offset_y,
            ],
            [
                0.5 * proj_x,
                -0.5 * proj_y,
                -0.5 + 0.5 * proj_offset_x,
                -0.5 - 0.5 * proj_offset_y,
            ],
        )
    } else {
        (
            [
                2.0 * inv_x,
                2.0 * inv_y,
                -inv_x + inv_offset_x,
                -inv_y + inv_offset_y,
            ],
            [
                0.5 * proj_x,
                0.5 * proj_y,
                -0.5 + 0.5 * proj_offset_x,
                -0.5 + 0.5 * proj_offset_y,
            ],
        )
    };
    let max_radius = inv_y * config.max_screen_radius as f64 * 2.0;
    CortaoMainParams {
        resolution: [width as f32, height as f32],
        random_seed: seed,
        normal_bias: (inv_x / width as f64 * 2.0_f64.sqrt()) as f32,
        radius: config.radius,
        unproject: unproject.map(|x| x as f32),
        unproject_skew: 0.0,
        projection: projection.map(|x| x as f32),
        projection_skew: 0.0,
        max_apparent_circle_radius_coefficient: (max_radius
            / (max_radius * max_radius + 1.0).sqrt())
            as f32,
        mip_bias: ((proj_y * height as f64 * 0.5).log2() + config.mip_bias as f64) as f32,
        inverse_max_slope_compensation: (1.0 / config.max_slope_compensation as f64) as f32,
        backface_threshold: config.backface_threshold,
        backface_boost: config.backface_boost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_rows(rows: [[f64; 4]; 4]) -> [f64; 16] {
        std::array::from_fn(|i| rows[i % 4][i / 4])
    }

    fn identity() -> [f64; 16] {
        from_rows([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    fn perspective(fov: f64, aspect: f64, near: f64, far: f64, offset: [f64; 2]) -> [f64; 16] {
        let y = 1.0 / (0.5 * fov.to_radians()).tan();
        from_rows([
            [y / aspect, 0.0, offset[0], 0.0],
            [0.0, y, offset[1], 0.0],
            [0.0, 0.0, far / (near - far), far * near / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ])
    }

    fn orthographic() -> [f64; 16] {
        from_rows([
            [0.5, 0.0, 0.0, -0.2],
            [0.0, 0.75, 0.0, 0.3],
            [0.0, 0.0, -1.0 / 19.5, -0.5 / 19.5],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    // Mat4f::ortho(0, 800, 0, 600, 100, -100, 1, 1), including its depth offset.
    fn ui_orthographic() -> [f64; 16] {
        from_rows([
            [2.0 / 800.0, 0.0, 0.0, -1.0],
            [0.0, -2.0 / 600.0, 0.0, 1.0],
            [0.0, 0.0, -1.0 / 200.0, 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    fn view_transform() -> [f64; 16] {
        from_rows([
            [0.8, 0.0, 0.6, 1.25],
            [0.36, 0.8, -0.48, -2.0],
            [-0.48, 0.6, 0.64, -3.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    fn close(actual: f64, expected: f64, tolerance: f64, label: &str) {
        assert!(
            (actual - expected).abs() <= tolerance * expected.abs().max(1.0),
            "{label}: actual {actual}, expected {expected}"
        );
    }

    fn relative(actual: f32, expected: f32, label: &str) {
        let scale = (actual as f64).abs().max((expected as f64).abs());
        assert!(
            (actual as f64 - expected as f64).abs() <= 1e-5 * scale,
            "{label}: actual {actual}, expected {expected}"
        );
    }

    // f32 mirrors of the lexical shader helpers, the sign bit and field offset included.
    fn pack_depth(depth: f32, code: [f32; 4]) -> u32 {
        let mut c = if code[0] > 0.5 {
            ((depth - code[1]) * code[2])
                .round()
                .clamp(0.0, (DEPTH_CODES - 1) as f32) as u32
        } else {
            let k = code[3] as u32;
            let c = (depth * code[2])
                .max(1.0)
                .to_bits()
                .wrapping_sub(0x3f80_0000)
                .wrapping_add(1_u32.wrapping_shl(k) >> 1)
                .wrapping_shr(k);
            c.min(DEPTH_CODES - 1)
        };
        if c >= DEPTH_CODES / 2 {
            c = c.wrapping_add(16384);
        }
        c.wrapping_add(8192).wrapping_shl(10)
    }

    fn unpack_depth(packed: u32, code: [f32; 4]) -> f32 {
        let c = (packed >> 10)
            .wrapping_sub(8192)
            .wrapping_sub((packed >> 31) << 14);
        if code[0] > 0.5 {
            code[1] + c as f32 * code[3]
        } else {
            code[1] * f32::from_bits(c.wrapping_shl(code[3] as u32).wrapping_add(0x3f80_0000))
        }
    }

    #[test]
    fn depth_codes_round_trip_and_clamp_as_normal_floats() {
        for (label, input, law) in [
            ("perspective", perspective(60.0, 1.5, 0.1, 1000.0, [0.0, 0.0]), 0.0),
            ("orthographic", orthographic(), 1.0),
        ] {
            let input = input.map(|x| x as f32);
            let precise = Camera64::from_clip(&input).expect("closed camera decomposes");
            let camera = CortaoCamera::from_clip(&input).expect("closed camera has depth codes");
            let code = camera.depth_code;
            assert_eq!(code[0], law, "{label}: packing law follows projection");
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for corner in 0..8 {
                let h = mul_vec(
                    &precise.view_from_clip,
                    [
                        if corner & 1 == 0 { -1.0 } else { 1.0 },
                        if corner & 2 == 0 { -1.0 } else { 1.0 },
                        if corner & 4 == 0 { 0.0 } else { 1.0 },
                        1.0,
                    ],
                );
                let depth = -h[2] / h[3];
                lo = lo.min(depth);
                hi = hi.max(depth);
            }
            assert!(
                (f64::from(code[1]) / lo - 1.0).abs() < 1e-6,
                "{label}: low end matches the nearest clip corner"
            );
            let last = pack_depth(f32::INFINITY, code);
            let high = unpack_depth(last, code);
            if camera.ortho {
                assert!(
                    (f64::from(high) / hi - 1.0).abs() < 1e-6,
                    "{label}: high end matches the farthest clip corner"
                );
            } else {
                assert!(
                    (f64::from(code[1]) / 0.1 - 1.0).abs() < 1e-6,
                    "perspective: low end matches near 0.1"
                );
                assert_eq!(
                    code[2], 1.0 / code[1],
                    "perspective: scale is the low end's reciprocal"
                );
                // 1000 / 0.1 is about 10000: its float-bit span fits at k = 5,
                // but not k = 4, even with the clip matrix's f32 rounding.
                assert_eq!(
                    code[3], 5.0,
                    "perspective: near 0.1 / far 1000 needs shift 5"
                );
                assert_eq!(
                    high,
                    code[1] * f32::from_bits(((DEPTH_CODES - 1) << 5) + 0x3f80_0000),
                    "perspective: saturation uses the full code window"
                );
            }
            for i in 0..=4096 {
                let t = f64::from(i) / 4096.0;
                let depth = if camera.ortho {
                    lo + (hi - lo) * t
                } else {
                    lo * (hi / lo).powf(t)
                } as f32;
                let packed = pack_depth(depth, code);
                let decoded = unpack_depth(packed | (i as u32 & 1023), code);
                let step = if camera.ortho {
                    f64::from(code[3])
                } else {
                    let binade = (f64::from(depth) / f64::from(code[1])).log2().floor() as i32;
                    f64::from(code[1]) * 2.0_f64.powi(binade + code[3] as i32 - 23)
                };
                assert!(
                    (f64::from(decoded) - f64::from(depth)).abs() <= step,
                    "{label}: round trip at {depth} gave {decoded}, beyond one code step {step}"
                );
                assert!(
                    (1..=254).contains(&((packed >> 23) & 255)),
                    "{label}: in-window bits must be a normal float, never 0"
                );
            }
            let first = 8192 << 10;
            assert_eq!(last, (1 << 31) | ((DEPTH_CODES / 2 - 1 + 8192) << 10), "{label}: the last code");
            for (depth, expected) in [
                (0.0, first),
                (0.5 * lo as f32, first),
                (code[1], first),
                (2.0 * high, last),
                (f32::INFINITY, last),
            ] {
                let packed = pack_depth(depth, code);
                assert_eq!(packed, expected, "{label}: outside depth clamps to its end code");
                assert!(
                    (1..=254).contains(&((packed >> 23) & 255)),
                    "{label}: clamped bits must be a normal float, never 0"
                );
                assert_eq!(
                    unpack_depth(packed | 1023, code),
                    unpack_depth(packed, code),
                    "{label}: low normal bits never alter depth"
                );
            }
        }

        // Reverse clip depth keeps even huge closed ratios finite at both ends:
        // view z = -1, w = 1 / hi at clip z = 0 and w = 1 at clip z = 1.
        let mut view_from_clip = from_rows([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, -1.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        // With the 2^126 cap, k = 8 already fits; k = 9 is never the least shift.
        for k in 0..=8 {
            let first_span = if k == 0 { 1 } else { DEPTH_CODES << (k - 1) };
            let last_span = ((DEPTH_CODES << k) - 1).min(126 << 23);
            for span in [first_span, last_span] {
                let hi = f32::from_bits(0x3f80_0000 + span);
                view_from_clip[15] = 1.0 / f64::from(hi);
                view_from_clip[11] = 1.0 - view_from_clip[15];
                let code = depth_code(&view_from_clip, false).expect("closed pseudo-log window");
                assert_eq!(code[3], k as f32, "span {span}: expected least shift {k}");
                assert!(
                    span >> k <= DEPTH_CODES - 1,
                    "shift {k}, span {span}: far end fits"
                );
                if k > 0 {
                    assert!(
                        span >> (k - 1) > DEPTH_CODES - 1,
                        "shift {k}, span {span}: the smaller shift must not fit"
                    );
                }
                let decoded = unpack_depth(pack_depth(hi, code), code);
                let step = 2.0_f64.powi(f64::from(hi).log2().floor() as i32 + k as i32 - 23);
                assert!(
                    (f64::from(decoded) - f64::from(hi)).abs() <= step,
                    "shift {k}, span {span}: far end {hi} decoded to {decoded}, beyond one step {step}"
                );
            }
        }
        view_from_clip[15] = 2.0_f64.powi(-128);
        view_from_clip[11] = 1.0 - view_from_clip[15];
        assert_eq!(
            depth_code(&view_from_clip, false).expect("ratio capped at 2^126")[3],
            8.0,
            "a closed ratio above 2^126 is capped before choosing its shift"
        );
        view_from_clip[15] = 0.5;
        view_from_clip[11] = 0.5;
        view_from_clip[14] = -f64::from(f32::from_bits(1));
        assert!(
            depth_code(&view_from_clip, false).is_none(),
            "a low end with a non-finite f32 reciprocal must be refused"
        );
    }

    #[test]
    fn open_perspective_depth_windows_start_at_the_closed_low_end() {
        let mut infinite = perspective(60.0, 1.5, 0.1, 1000.0, [0.0, 0.0]);
        infinite[10] = -1.0;
        infinite[14] = -0.1;
        // View w goes from +10 at the near corners through zero to -10 at
        // the far corners. Negative far depths cannot define the low end.
        let beyond = perspective(60.0, 1.5, 0.1, -0.1, [0.0, 0.0]);
        for (label, input, crosses_infinity) in [
            ("infinite far", infinite, false),
            ("beyond infinity", beyond, true),
        ] {
            let input = input.map(|x| x as f32);
            let precise = Camera64::from_clip(&input).expect("open camera decomposes");
            let camera = CortaoCamera::from_clip(&input).expect("open camera has depth codes");
            let code = camera.depth_code;
            assert!(!camera.ortho, "{label}: open camera remains perspective");
            assert_eq!(code[0], 0.0, "{label}: open camera uses the pseudo-log law");
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    let near = mul_vec(&precise.view_from_clip, [x, y, 0.0, 1.0]);
                    let far = mul_vec(&precise.view_from_clip, [x, y, 1.0, 1.0]);
                    assert!(near[3] > 0.0, "{label}: near corners have positive w");
                    if crosses_infinity {
                        assert!(far[3] < 0.0, "{label}: far corners pass beyond infinity");
                    } else {
                        assert_eq!(far[3], 0.0, "{label}: far corners lie at infinity");
                    }
                    let near_depth = -near[2] / near[3];
                    assert!(
                        (f64::from(code[1]) / near_depth - 1.0).abs() < 1e-6,
                        "{label}: low end is the near corners' depth"
                    );
                }
            }
            assert_eq!(
                code[2], 1.0 / code[1],
                "{label}: scale is the low end's reciprocal"
            );
            assert_eq!(code[3], 6.0, "{label}: open window uses shift 6");
            let high = unpack_depth(pack_depth(f32::INFINITY, code), code);
            assert_eq!(
                high,
                code[1] * f32::from_bits(((DEPTH_CODES - 1) << 6) + 0x3f80_0000),
                "{label}: high end spans about 31.75 float-bit binades from the closed low end"
            );
        }
    }

    #[test]
    fn perspective_parameters_match_the_reference() {
        let config = CortaoConfig::default();
        for fov in [35.0, 70.0, 115.0] {
            for aspect in [0.75, 1.0, 16.0 / 9.0] {
                for offset in [[0.0, 0.0], [0.23, -0.31]] {
                    for (near, far) in [(0.1, 100.0), (1.0, 17.0)] {
                        let input = perspective(fov, aspect, near, far, offset).map(|x| x as f32);
                        let inverse = inverse4(&input.map(f64::from)).unwrap();
                        let projection = std::array::from_fn(|col| {
                            std::array::from_fn(|row| input[col * 4 + row])
                        });
                        let inverse = std::array::from_fn(|col| {
                            std::array::from_fn(|row| inverse[col * 4 + row] as f32)
                        });
                        let camera = CortaoCamera::from_clip(&input).unwrap();
                        assert!(!camera.ortho);
                        let params = main_params(&camera, 1920, 1080, 0xdeadbeef, &config);
                        let reference = reference_main_params(
                            &projection,
                            &inverse,
                            1920,
                            1080,
                            0xdeadbeef,
                            true,
                            &config,
                        );
                        assert_eq!(params.resolution, reference.resolution);
                        assert_eq!(params.random_seed, reference.random_seed);
                        for (actual, expected) in
                            params.unproject.into_iter().zip(reference.unproject)
                        {
                            relative(actual, expected, "unproject");
                        }
                        for (actual, expected) in
                            params.projection.into_iter().zip(reference.projection)
                        {
                            relative(actual, expected, "projection");
                        }
                        for (label, actual, expected) in [
                            ("normal bias", params.normal_bias, reference.normal_bias),
                            ("radius", params.radius, reference.radius),
                            (
                                "unproject skew",
                                params.unproject_skew,
                                reference.unproject_skew,
                            ),
                            (
                                "projection skew",
                                params.projection_skew,
                                reference.projection_skew,
                            ),
                            (
                                "radius coefficient",
                                params.max_apparent_circle_radius_coefficient,
                                reference.max_apparent_circle_radius_coefficient,
                            ),
                            ("mip bias", params.mip_bias, reference.mip_bias),
                            (
                                "inverse slope",
                                params.inverse_max_slope_compensation,
                                reference.inverse_max_slope_compensation,
                            ),
                            (
                                "backface threshold",
                                params.backface_threshold,
                                reference.backface_threshold,
                            ),
                            (
                                "backface boost",
                                params.backface_boost,
                                reference.backface_boost,
                            ),
                        ] {
                            relative(actual, expected, label);
                        }
                    }
                }
            }
        }
    }

    fn assert_rigid_and_recomposed(input: &[f32; 16], camera: &Camera64) {
        let view = &camera.view_from_world;
        for row in 0..3 {
            for col in 0..3 {
                let expected = if row == col { 1.0 } else { 0.0 };
                close(
                    dot(row3(view, row), row3(view, col)),
                    expected,
                    1e-9,
                    "Q Q^T",
                );
            }
        }
        for col in 0..4 {
            assert_eq!(view[col * 4 + 3], if col == 3 { 1.0 } else { 0.0 });
        }
        let [px, py, pz, pw, skew] = camera.projection;
        let projection = if camera.ortho {
            from_rows([
                [2.0 * px, 2.0 * skew, 0.0, 2.0 * pz - 1.0],
                [0.0, -2.0 * py, 0.0, 1.0 - 2.0 * pw],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ])
        } else {
            from_rows([
                [2.0 * px, 2.0 * skew, 2.0 * pz + 1.0, 0.0],
                [0.0, -2.0 * py, -2.0 * pw - 1.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, -1.0, 0.0],
            ])
        };
        let m = input.map(f64::from);
        let scale = if camera.ortho {
            m[15]
        } else {
            length(row3(&m, 3))
        };
        let recomposed = mul_mat(&projection, view);
        for row in [0, 1, 3] {
            for col in 0..4 {
                let index = col * 4 + row;
                close(recomposed[index], m[index] / scale, 1e-9, "K [Q|t]");
            }
        }
    }

    fn random(state: &mut u32) -> f64 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *state as f64 / u32::MAX as f64
    }

    fn assert_round_trips(input: [f64; 16], expected_ortho: bool) {
        let input = input.map(|x| x as f32);
        let full = input.map(f64::from);
        let precise = Camera64::from_clip(&input).unwrap();
        let camera = CortaoCamera::from_clip(&input).unwrap();
        assert_eq!(camera.ortho, expected_ortho);
        assert_rigid_and_recomposed(&input, &precise);
        let view = camera.view_from_world.map(f64::from);
        let view_from_clip = camera.view_from_clip.map(f64::from);
        let params = main_params(&camera, 1280, 720, 19, &CortaoConfig::default());
        let [px, py, pz, pw, skew] =
            [camera.px, camera.py, camera.pz, camera.pw, camera.skew].map(f64::from);
        let [ux, uy, zx, zy] = params.unproject.map(f64::from);
        let mut state = 0x12345678;
        for _ in 0..32 {
            let depth = 2.0 + 10.0 * random(&mut state);
            let target = [
                (random(&mut state) - 0.5) * depth,
                (random(&mut state) - 0.5) * depth,
                -depth,
            ];
            // Invert the rigid frame explicitly, choosing random points in front.
            let rigid = &precise.view_from_world;
            let shifted = subtract(target, [rigid[12], rigid[13], rigid[14]]);
            let mut world = [0.0, 0.0, 0.0, 1.0];
            for col in 0..3 {
                world[col] = (0..3).map(|row| rigid[col * 4 + row] * shifted[row]).sum();
            }
            let point = mul_vec(&view, world);
            let [x, y, z, _] = point;
            let depth = -z;
            assert!(
                depth > 0.0,
                "points in front must have positive linear depth"
            );
            let clip = mul_vec(&full, world);
            let ndc = dehomogenize(clip).unwrap();
            let uv = [0.5 * ndc[0] + 0.5, 0.5 - 0.5 * ndc[1]];
            let projected = if camera.ortho {
                [px * x + skew * y + pz, py * y + pw]
            } else {
                [
                    (px * x + skew * y + pz * z) / depth,
                    (py * y + pw * z) / depth,
                ]
            };
            close(projected[0], uv[0], 1e-5, "projected u");
            close(projected[1], uv[1], 1e-5, "projected v");
            let factor = if camera.ortho { 1.0 } else { depth };
            let reconstructed = [
                (uv[0] * ux + uv[1] * params.unproject_skew as f64 + zx) * factor,
                (uv[1] * uy + zy) * factor,
                -depth,
            ];
            let from_clip =
                dehomogenize(mul_vec(&view_from_clip, [ndc[0], ndc[1], ndc[2], 1.0])).unwrap();
            for axis in 0..3 {
                close(
                    reconstructed[axis],
                    point[axis],
                    1e-4,
                    "parameter unprojection",
                );
                close(from_clip[axis], point[axis], 1e-4, "clip unprojection");
            }
        }
    }

    #[test]
    fn cameras_round_trip_with_rigid_views_and_recompose_in_f64() {
        let symmetric = perspective(65.0, 1.7, 0.2, 80.0, [0.0, 0.0]);
        let off_centre = perspective(80.0, 0.9, 0.3, 30.0, [0.17, -0.21]);
        let view = view_transform();
        let mut mirror = identity();
        mirror[0] = -1.0;
        let mut scale = identity();
        scale[0] = 2.0;
        scale[10] = 0.3;
        let mut skewed = off_centre;
        skewed[4] = 0.35;
        let ortho = orthographic();
        let mut skewed_ortho = ortho;
        skewed_ortho[4] = 0.15;
        // An invertible 3D lift of a rectangle-to-quad map. The additional z
        // contribution to w gives a finite eye and a full-rank x/y/w camera.
        let projective_lift = from_rows([
            [1.1, 0.15, 0.0, 30.0],
            [0.05, 0.9, 0.0, 20.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0002, -0.0001, -0.02, 1.0],
        ]);
        for (input, ortho) in [
            (symmetric, false),
            (off_centre, false),
            (mul_mat(&off_centre, &view), false),
            (mul_mat(&symmetric, &mirror), false),
            (mul_mat(&symmetric, &scale), false),
            (mul_mat(&skewed, &view), false),
            (symmetric.map(|x| x * 1e-12), false),
            (symmetric.map(|x| x * 1e12), false),
            (ortho, true),
            (mul_mat(&ortho, &view), true),
            (skewed_ortho, true),
            (ui_orthographic(), true),
            (mul_mat(&ui_orthographic(), &projective_lift), false),
        ] {
            assert_round_trips(input, ortho);
        }
        let mirrored = mul_mat(&symmetric, &mirror).map(|x| x as f32);
        let rigid = Camera64::from_clip(&mirrored).unwrap().view_from_world;
        close(
            dot(row3(&rigid, 0), cross(row3(&rigid, 1), row3(&rigid, 2))),
            -1.0,
            1e-9,
            "mirrored camera determinant",
        );
    }

    #[test]
    fn detects_projection_type_and_rejects_invalid_matrices() {
        let mut tiny_eye = perspective(70.0, 1.5, 0.2, 50.0, [0.0, 0.0]);
        tiny_eye[11] = -1e-7;
        assert!(
            !CortaoCamera::from_clip(&tiny_eye.map(|x| x as f32))
                .unwrap()
                .ortho
        );
        let mut noisy_ortho = orthographic();
        noisy_ortho[3] = 1e-8;
        noisy_ortho[11] = -1e-8;
        for scale in [1e-12, 1.0, 1e12] {
            let input = noisy_ortho.map(|x| (x * scale) as f32);
            assert!(CortaoCamera::from_clip(&input).unwrap().ortho);
        }
        assert!(CortaoCamera::from_clip(&[0.0; 16]).is_none());
        let mut singular = identity();
        for col in 0..4 {
            singular[col * 4 + 1] = singular[col * 4];
        }
        assert!(CortaoCamera::from_clip(&singular.map(|x| x as f32)).is_none());
        let mut nearly_singular = identity();
        nearly_singular[4] = 1.0;
        nearly_singular[0] = 1e-16;
        assert!(CortaoCamera::from_clip(&nearly_singular.map(|x| x as f32)).is_none());
        for value in [f32::NAN, f32::INFINITY] {
            let mut invalid = identity().map(|x| x as f32);
            invalid[0] = value;
            assert!(CortaoCamera::from_clip(&invalid).is_none());
        }
    }

    #[test]
    fn planar_homographies_cannot_supply_a_rigid_camera() {
        // The quad_matrix shape has a zero third column and a depth row
        // proportional to w, so the exact map is singular even after an ortho.
        let planar = from_rows([
            [1.1, 0.15, 0.0, 30.0],
            [0.05, 0.9, 0.0, 20.0],
            [0.0001, -0.00005, 0.0, 0.5],
            [0.0002, -0.0001, 0.0, 1.0],
        ]);
        assert!(
            CortaoCamera::from_clip(&mul_mat(&ui_orthographic(), &planar).map(|x| x as f32))
                .is_none()
        );
        // Preserving z makes the 4x4 invertible, but leaves an eye at infinity
        // with a nonconstant denominator: not an affine orthographic camera.
        let mut invertible = planar;
        invertible[10] = 1.0;
        let input = mul_mat(&ui_orthographic(), &invertible).map(|x| x as f32);
        assert!(inverse4(&input.map(f64::from)).is_some());
        assert!(CortaoCamera::from_clip(&input).is_none());
    }

    #[test]
    fn uniform_guards_and_orthographic_radius_bias() {
        let config = CortaoConfig {
            max_screen_radius: 10.0,
            max_slope_compensation: 0.25,
            ..CortaoConfig::default()
        };
        let camera = CortaoCamera::from_clip(
            &perspective(179.999, 2.0, 0.1, 100.0, [0.0, 0.0]).map(|x| x as f32),
        )
        .unwrap();
        let params = main_params(&camera, 1920, 1080, 7, &config);
        assert!(params.max_apparent_circle_radius_coefficient < 1.0);
        assert!(params.max_apparent_circle_radius_coefficient > 0.0);
        assert_eq!(params.inverse_max_slope_compensation, 1.0);
        let camera = CortaoCamera::from_clip(&orthographic().map(|x| x as f32)).unwrap();
        let one = CortaoConfig {
            radius: 1.0,
            ..config
        };
        let two = CortaoConfig {
            radius: 2.0,
            ..config
        };
        let a = main_params(&camera, 1280, 720, 0, &one);
        let b = main_params(&camera, 1280, 720, 0, &two);
        close(
            b.mip_bias as f64 - a.mip_bias as f64,
            1.0,
            1e-6,
            "ortho radius mip shift",
        );
        let inverse = inverse4(&orthographic()).unwrap();
        let rigid = camera.view_from_world.map(f64::from);
        for depth in [0.0, 0.5, 1.0] {
            let world = dehomogenize(mul_vec(&inverse, [0.0, 0.0, depth, 1.0])).unwrap();
            let point = mul_vec(&rigid, [world[0], world[1], world[2], 1.0]);
            assert!(
                point[2] < 0.0,
                "entire orthographic depth interval is in front"
            );
        }
        let filter = filter_params(321, 123, &config);
        assert_eq!(filter.resolution, [321.0, 123.0]);
        assert_eq!(filter.depth_sensitivity, config.filter_depth_sensitivity);
        assert_eq!(filter.normal_sensitivity, config.filter_normal_sensitivity);
        assert_eq!(CortaoConfig::default().backface_boost, 0.5);
    }

    #[test]
    fn atlas_levels_fit_without_overlap() {
        for (width, height, levels) in [(1920, 1080, 5), (3840, 2160, 6), (64, 64, 1), (1, 1, 1)] {
            assert_eq!(CortaoAtlas::new(width, height).levels, levels);
        }
        for (width, height) in [
            (1920, 1080),
            (3840, 2160),
            (64, 64),
            (1, 1),
            (2, 2),
            (97, 61),
            (1, 128),
            (128, 1),
            (32768, 32768),
        ] {
            for min_size in [1, 8, 64] {
                let atlas = CortaoAtlas::with_min(width, height, min_size);
                assert_eq!(atlas.offsets[0], [0, 0]);
                assert_eq!(atlas.sizes[0], [width, height]);
                assert_eq!(atlas.width, width);
                assert_eq!(atlas.height, height + (height >> 1).max(1));
                assert!((1..=8).contains(&atlas.levels));
                let last = atlas.sizes[atlas.levels as usize - 1];
                if width >= 2 && height >= 2 {
                    assert!(last[0] >= 2 && last[1] >= 2);
                }
                for i in 0..atlas.levels as usize {
                    let [x, y] = atlas.offsets[i];
                    let [w, h] = atlas.sizes[i];
                    assert!(x + w <= atlas.width && y + h <= atlas.height);
                    for j in 0..i {
                        let [other_x, other_y] = atlas.offsets[j];
                        let [other_w, other_h] = atlas.sizes[j];
                        assert!(
                            x + w <= other_x
                                || other_x + other_w <= x
                                || y + h <= other_y
                                || other_y + other_h <= y,
                            "atlas levels {i} and {j} overlap"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn child_lookup_matches_the_forward_prepass_loop() {
        for (width, height) in [(97, 61), (128, 128)] {
            let atlas = CortaoAtlas::with_min(width, height, 8);
            for seed in [0, 1, 0xdeadbeef, u32::MAX] {
                let mut maps: Vec<Vec<Option<[u32; 2]>>> = atlas.sizes[..atlas.levels as usize]
                    .iter()
                    .map(|size| vec![None; (size[0] * size[1]) as usize])
                    .collect();
                for y in 0..height {
                    for x in 0..width {
                        let original = [x, y];
                        maps[0][(y * width + x) as usize] = Some(original);
                        let mut pixel = original;
                        let mut resolution = [width, height];
                        for level in 1..atlas.levels {
                            let next = [pixel[0] >> 1, pixel[1] >> 1];
                            let dx = seed >> (level * 2);
                            let dy = seed >> (level * 2 + 1);
                            let wrong_pixel =
                                ((pixel[0] ^ next[1] ^ dx) | (pixel[1] ^ next[0] ^ dy)) & 1 != 0;
                            pixel = next;
                            resolution = resolution.map(|value| (value >> 1).max(1));
                            if wrong_pixel || pixel[0] >= resolution[0] || pixel[1] >= resolution[1]
                            {
                                break;
                            }
                            let slot = &mut maps[level as usize]
                                [(pixel[1] * resolution[0] + pixel[0]) as usize];
                            assert!(slot.is_none(), "prepass must choose exactly one child");
                            *slot = Some(original);
                        }
                    }
                }
                for level in 0..atlas.levels {
                    let [w, h] = atlas.sizes[level as usize];
                    for y in 0..h {
                        for x in 0..w {
                            let source = level0_pixel(level, [x, y], seed);
                            assert!(source[0] < width && source[1] < height);
                            assert_eq!(
                                maps[level as usize][(y * w + x) as usize],
                                Some(source),
                                "prepass source at level {level}, texel ({x}, {y}), seed {seed}"
                            );
                        }
                    }
                }
            }
        }
    }
}
