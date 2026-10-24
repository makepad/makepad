//! Nebulas (plan hdri-7 §3), written from general techniques: a seed's twelve candidate clouds placed along the Milky
//! Way's plane (anywhere when it is off), each a sprite on its own tangent plane whose structure (domain-warped noise,
//! ridged filaments and wisps, a hot star's cavity and rim, cellular knots, dust lanes) is built once per seed and
//! kept, its look composed per texel: Amount, Size, Brightness and Realism change no sprite, so only a typed seed
//! builds anything (D2).
//!
//! **Realism** blends two looks of one layout (D8): at 0 astrophotography's colours (hydrogen-alpha red-magenta,
//! oxygen-III teal, reflection blue, dark dust; peaks about 45 G0), at 1 the naked eye's faint grey patches (peaks
//! about 3.5 G0, read two levels softer). Luminance blends geometrically, chroma by 1 - (1 - r)², the structure through
//! the softer level.
//!
//! **Held and released.** A sprite has six levels, 8 to 256 texels across, each the same function with its octaves
//! cut at its own Nyquist on the scale of all of them (`fbm3_frac_full`, as the Milky Way's: a coarse level's noise
//! keeps the fine one's contrast) and its knots widened, their light kept; a lookup reads the finest level whose texel
//! is at least its own, the coarsest when none is, the finest at texel 0.

use makepad_draw::*;
use super::lazy_grid::LazyGrid;
use super::night::{mag_arcsec2_to_radiance, star_kernel, STAR_MAG_LIMIT, STAR_REACH2};
use super::noise::{fbm3_frac_full, hash01, hash_u32, octaves_for, worley3};
use super::NebulaParams;
use crate::sky::luminance;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Arc, Mutex};

/// Candidates per seed; Amount shows them one by one.
pub(super) const NEBULA_MAX: usize = 12;
/// Seeds whose sprites the memo keeps beside those a map still holds.
pub(super) const NEBULA_KEEP: usize = 2;
/// A sprite's levels, texels across.
pub(super) const SPRITE_LEVELS: [usize; 6] = [8, 16, 32, 64, 128, 256];
/// The block's seed's salt (its own stream: the nebulas never re-roll the stars).
const NEBULA_SALT: u32 = 0x4e45_4255;
/// The brightest a cloud shows at Realism 0 (about 45 G0, "as photographed") and at Realism 1 (about 3.5 G0, as the
/// eye sees it), mag/arcsec², at Brightness 1.
const ART_PEAK_MU: f32 = 17.76;
const REAL_PEAK_MU: f32 = 20.53;
/// The geometric blend's floor, a share of each look's peak: the blend is continuous where one look is dark.
const FLOOR: f32 = 1.0e-3;
/// The looks' colours (luminance 1 once `new` scales them): a designed astrophotography palette (D7: photometric
/// line weights would turn the clouds cyan), and a cool grey of saturation under 0.1.
const HA_RGB: Vec3f = vec3f(1.0, 0.12, 0.35);
const O3_RGB: Vec3f = vec3f(0.05, 0.9, 0.75);
const REFLECTION_RGB: Vec3f = vec3f(0.25, 0.45, 1.0);
const REAL_RGB: Vec3f = vec3f(0.95, 0.99, 1.05);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NebulaKind {
    /// Hydrogen-alpha, a little oxygen.
    Emission,
    /// An oxygen-III core in a hydrogen-alpha shell: a hot star's bubble.
    BiColour,
    /// Blue starlight scattered by dust.
    Reflection,
    /// Dust alone.
    Dark,
}

/// A seed's candidate `i`: what it is, its radius at Size 1, its turn and its placement numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Candidate {
    kind: NebulaKind,
    radius_deg: f32,
    angle: f32,
    u1: f32,
    u2: f32,
    u3: f32,
}

fn candidate(seed: u32, i: usize) -> Candidate {
    let h = |k: i32| hash01(i as i32, k, 0, seed);
    let kind = match h(0) {
        x if x < 0.40 => NebulaKind::Emission,
        x if x < 0.65 => NebulaKind::BiColour,
        x if x < 0.85 => NebulaKind::Reflection,
        _ => NebulaKind::Dark,
    };
    let size = h(1);
    Candidate { kind, radius_deg: 1.5 + 5.5 * size * size, angle: TAU * h(2), u1: h(3), u2: h(4), u3: h(5) }
}

/// Where candidate `c` sits in star space: with the Milky Way's axes (centre, pole, l 90) along its plane, l weighted
/// toward the inner galaxy and b Gaussian (5 degrees for the glowing kinds, 15 for reflection and dark clouds); without
/// them uniform on the sphere.
fn place(c: &Candidate, axes: Option<[Vec3f; 3]>) -> Vec3f {
    match axes {
        Some([centre, pole, l90]) => {
            let side = if c.u1 < 0.5 { -1.0 } else { 1.0 };
            let l = side * PI * (2.0 * (c.u1 - 0.5)).abs().powf(1.5);
            let sigma = match c.kind {
                NebulaKind::Emission | NebulaKind::BiColour => 5.0f32,
                _ => 15.0,
            }
            .to_radians();
            let gauss = (-2.0 * (1.0 - c.u2).max(1.0e-7).ln()).sqrt() * (TAU * c.u3).cos();
            let b = (sigma * gauss).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
            (centre * (b.cos() * l.cos()) + l90 * (b.cos() * l.sin()) + pole * b.sin()).normalize()
        }
        None => {
            let z = 2.0 * c.u2 - 1.0;
            let phi = TAU * c.u1;
            let r = (1.0 - z * z).max(0.0).sqrt();
            vec3f(r * phi.cos(), z, r * phi.sin())
        }
    }
}

/// What a sprite is: its kind, its noise's slice and seed.
#[derive(Clone, Copy, Debug)]
struct SpriteDef {
    kind: NebulaKind,
    z: f32,
    seed: u32,
}

/// A seed's twelve sprites, each six `LazyGrid`s (H-alpha, O III, reflection, dust depth), filled as bakes read them.
#[derive(Debug)]
pub(super) struct Sprites {
    defs: [SpriteDef; NEBULA_MAX],
    grids: Vec<[LazyGrid<4>; 6]>,
}

impl Sprites {
    fn new(seed: u32) -> Sprites {
        #[cfg(test)]
        super::probe::count(super::probe::Work::NebulaSprites);
        let defs = std::array::from_fn(|i| SpriteDef { kind: candidate(seed, i).kind, z: 17.0 * i as f32 + 3.1, seed: hash_u32(seed ^ (i as u32 + 1).wrapping_mul(0x9e37_79b9)) });
        let grids = (0..NEBULA_MAX).map(|_| std::array::from_fn(|l| LazyGrid::new(SPRITE_LEVELS[l], SPRITE_LEVELS[l], false))).collect();
        Sprites { defs, grids }
    }

    /// Sprite `i`'s level `level` at its tangent-plane point (px, py) in [-1, 1]².
    fn sample(&self, i: usize, level: usize, px: f32, py: f32) -> [f32; 4] {
        let (def, size) = (self.defs[i], SPRITE_LEVELS[level]);
        let fill = move |x: usize, y: usize| sprite_texel(&def, size, (x as f32 + 0.5) / size as f32 * 2.0 - 1.0, (y as f32 + 0.5) / size as f32 * 2.0 - 1.0);
        self.grids[i][level].sample(0.5 * (px + 1.0), 0.5 * (py + 1.0), &fill)
    }
}

/// The sprites the maps share, by seed (the `volumetric::NOISE` precedent): built once, kept while a map holds them,
/// and NEBULA_KEEP more besides.
static SPRITES: Mutex<Vec<(u32, Arc<Sprites>)>> = Mutex::new(Vec::new());

/// A seed's sprites, shared by every map of that seed. Making them only allocates their empty rows (the rows fill as
/// bakes read them), so it is done under the lock. Room is made by dropping the oldest that no map holds.
fn sprites_for(seed: u32) -> Arc<Sprites> {
    let mut kept = SPRITES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, sprites)) = kept.iter().find(|(key, _)| *key == seed) {
        return sprites.clone();
    }
    while kept.len() >= NEBULA_KEEP {
        match kept.iter().position(|(_, sprites)| Arc::strong_count(sprites) == 1) {
            Some(i) => {
                kept.remove(i);
            }
            None => break,
        }
    }
    let sprites = Arc::new(Sprites::new(seed));
    kept.push((seed, sprites.clone()));
    sprites
}

/// What a sprite holds at its tangent-plane point (px, py) at a level `size` texels across: H-alpha, O III and
/// reflection emission (shapes, 0 to about 1.3) and the dust's optical depth. A function of (def, size, p) alone, its
/// octaves cut at the level's Nyquist (each keeping its weight in the full count, `fbm3_frac_full`) and its knots
/// widened to two texels with their light kept.
fn sprite_texel(def: &SpriteDef, size: usize, px: f32, py: f32) -> [f32; 4] {
    let r = (px * px + py * py).sqrt();
    if !(r < 1.0) {
        return [0.0; 4];
    }
    let texel = 2.0 / size as f32;
    let oct = |wavelength: f32, most: f32| octaves_for(wavelength, texel, most);
    let (z, s) = (def.z, def.seed);
    let envelope = 1.0 - smoothstep(0.45, 1.0, r);
    let wx = fbm3_frac_full(px * 1.5, py * 1.5, z, oct(0.67, 3.0), 3.0, s) - 0.5;
    let wy = fbm3_frac_full(px * 1.5 + 5.2, py * 1.5 + 1.3, z, oct(0.67, 3.0), 3.0, s ^ 1) - 0.5;
    let (qx, qy) = (px + 0.6 * wx, py + 0.6 * wy);
    let body = fbm3_frac_full(qx * 2.0, qy * 2.0, z + 3.7, oct(0.5, 5.0), 5.0, s ^ 2);
    let ridge = 1.0 - (2.0 * fbm3_frac_full(qx * 3.0, qy * 3.0, z + 7.1, oct(0.33, 6.0), 6.0, s ^ 3) - 1.0).abs();
    let wisps = ridge * ridge * ridge;
    // knots: within w of a cellular point (cells a quarter of the sprite), w at least two texels, the light kept
    let w = (8.0 * texel).max(0.3);
    let knot = (1.0 - smoothstep(0.0, w, worley3(qx * 4.0, qy * 4.0, z, s ^ 4))) * (0.3 / w) * (0.3 / w);
    let lanes = smoothstep(0.55, 0.75, fbm3_frac_full(qx * 2.5 + 9.1, qy * 2.5, z + 1.9, oct(0.4, 5.0), 5.0, s ^ 5)) * envelope;
    match def.kind {
        NebulaKind::Emission => {
            let ha = envelope * (0.35 * body + 0.65 * wisps + 0.8 * knot);
            [ha, 0.15 * ha, 0.0, lanes]
        }
        NebulaKind::BiColour => {
            let shell = smoothstep(0.2, 0.45, r) * envelope;
            let core = 1.0 - smoothstep(0.0, 0.5, r);
            [shell * (0.4 + 0.6 * wisps) + 0.8 * knot * envelope, core * (0.5 + 0.5 * wisps), 0.0, 0.8 * lanes]
        }
        NebulaKind::Reflection => [0.0, 0.0, envelope * (0.5 * body + 0.5 * wisps + 0.5 * knot), 0.6 * lanes],
        NebulaKind::Dark => [0.0, 0.0, 0.0, 1.5 * envelope * (0.6 + 0.8 * body)],
    }
}

/// The sprite level a lookup of `texel` radians reads for a cloud of angular `radius`: the finest whose texel
/// (2 radius / size) is at least the lookup's; the coarsest when none is; the finest at texel 0.
fn level_for(radius: f32, texel: f32) -> usize {
    (0..SPRITE_LEVELS.len()).rev().find(|&l| 2.0 * radius / SPRITE_LEVELS[l] as f32 >= texel).unwrap_or(0)
}

/// A visible candidate, placed: its sprite, its frame (centre, the sprite's x and y), radius, reach, Amount weight.
#[derive(Clone, Debug)]
struct Placed {
    index: usize,
    centre: Vec3f,
    e1: Vec3f,
    e2: Vec3f,
    radius: f32,
    tan_radius: f32,
    cos_reach: f32,
    /// How far its embedded stars reach: STAR_REACH past 0.6 radius (`cos_reach` when it holds none).
    cos_star_reach: f32,
    weight: f32,
}

impl Placed {
    /// Star-space `s` (`cos_t` its dot with the centre) on the sprite's tangent plane, in [-1, 1]², or None off the disc.
    fn sprite_point(&self, s: Vec3f, cos_t: f32) -> Option<(f32, f32)> {
        if !(cos_t > 0.0) {
            return None;
        }
        let scale = 1.0 / (cos_t * self.tan_radius);
        let (px, py) = (s.dot(self.e1) * scale, s.dot(self.e2) * scale);
        (px * px + py * py < 1.0).then_some((px, py))
    }
}

/// A star inside cloud `cloud` (index into `placed`), drawn with the stars' own kernel.
#[derive(Clone, Debug)]
struct Embedded {
    cloud: usize,
    dir: Vec3f,
    flux: f32,
    colour: Vec3f,
}

/// The nebulas of one map, built once per map from the block as clamp() holds it.
#[derive(Clone, Debug)]
pub(super) struct Nebulas {
    sprites: Arc<Sprites>,
    placed: Vec<Placed>,
    stars: Vec<Embedded>,
    realism: f32,
    /// Brightness x the night's lift x the astronomical-twilight fade.
    gain: f32,
    art_peak: f32,
    real_peak: f32,
    /// The geometric blend of the two floors (what an empty texel would show), taken off so it shows nothing.
    floor: f32,
    /// The art chroma's share given to the real one: 1 - (1 - r)².
    chroma: f32,
    /// The dust's depth: 1 at Realism 0, 0.6 at 1.
    dust_k: f32,
    ha_rgb: Vec3f,
    o3_rgb: Vec3f,
    reflection_rgb: Vec3f,
    real_rgb: Vec3f,
}

impl Nebulas {
    /// `axes`: the galactic axes when the Milky Way is drawn (placement along its plane), else None (uniform); `gain`:
    /// lift x the astronomical-twilight fade; `star_gain`: a magnitude-STAR_MAG_LIMIT star's irradiance (lift and star
    /// brightness in) x that fade; `star_colors`: the stars' palette.
    pub(super) fn new(p: &NebulaParams, axes: Option<[Vec3f; 3]>, gain: f32, star_gain: f32, star_colors: &[Vec3f]) -> Nebulas {
        let seed = hash_u32(p.seed ^ NEBULA_SALT);
        let sprites = sprites_for(seed);
        let (mut placed, mut stars) = (Vec::new(), Vec::new());
        for i in 0..NEBULA_MAX {
            let weight = (p.amount * NEBULA_MAX as f32 - i as f32).clamp(0.0, 1.0);
            if !(weight > 0.0) {
                continue;
            }
            let c = candidate(seed, i);
            let centre = place(&c, axes);
            let radius = (c.radius_deg * p.size).to_radians().min(1.2);
            let reference = if centre.y.abs() < 0.9 { vec3f(0.0, 1.0, 0.0) } else { vec3f(1.0, 0.0, 0.0) };
            let t1 = Vec3f::cross(reference, centre).normalize();
            let t2 = Vec3f::cross(centre, t1);
            let (sa, ca) = c.angle.sin_cos();
            let (e1, e2) = (t1 * ca + t2 * sa, t2 * ca - t1 * sa);
            let cos_reach = (1.05 * radius).cos();
            let holds_stars = c.kind != NebulaKind::Dark && star_gain > 0.0 && !star_colors.is_empty();
            let cos_star_reach = if holds_stars { (0.6 * radius + STAR_REACH2.sqrt().asin()).min(PI).cos() } else { cos_reach };
            placed.push(Placed { index: i, centre, e1, e2, radius, tan_radius: radius.tan(), cos_reach, cos_star_reach, weight });
            if holds_stars {
                let count = 3 + (hash01(i as i32, 6, 0, seed) * 6.0) as usize;
                for k in 0..count {
                    let h = |m: i32| hash01(i as i32, 10 + 4 * k as i32 + m, 0, seed);
                    let (a, off) = (TAU * h(0), 0.6 * radius * h(1).sqrt());
                    let dir = (centre * off.cos() + (e1 * a.cos() + e2 * a.sin()) * off.sin()).normalize();
                    let magnitude = 3.0 + 3.0 * h(2);
                    let colour = star_colors[((h(3) * star_colors.len() as f32) as usize).min(star_colors.len() - 1)];
                    stars.push(Embedded { cloud: placed.len() - 1, dir, flux: star_gain * 10.0f32.powf(0.4 * (STAR_MAG_LIMIT - magnitude)) * weight, colour });
                }
            }
        }
        let r = p.realism;
        let (art_peak, real_peak) = (mag_arcsec2_to_radiance(ART_PEAK_MU), mag_arcsec2_to_radiance(REAL_PEAK_MU));
        let one = |c: Vec3f| c / luminance(c);
        Nebulas {
            sprites,
            placed,
            stars,
            realism: r,
            gain: gain * p.brightness,
            art_peak,
            real_peak,
            floor: (FLOOR * art_peak).powf(1.0 - r) * (FLOOR * real_peak).powf(r),
            chroma: 1.0 - (1.0 - r) * (1.0 - r),
            dust_k: 1.0 - 0.4 * r,
            ha_rgb: one(HA_RGB),
            o3_rgb: one(O3_RGB),
            reflection_rgb: one(REFLECTION_RGB),
            real_rgb: one(REAL_RGB),
        }
    }

    /// At star-space `s`: the clouds' light (their embedded stars included; before extinction and the horizon fade) and
    /// the transmittance of their dust for what lies behind (the band, the stars). `texel` picks the sprite level.
    pub(super) fn at(&self, s: Vec3f, texel: f32) -> (Vec3f, f32) {
        let (mut gas, mut stars, mut through) = (Vec3f::default(), Vec3f::default(), 1.0f32);
        for (k, c) in self.placed.iter().enumerate() {
            let cos_t = s.dot(c.centre);
            if !(cos_t >= c.cos_reach.min(c.cos_star_reach)) {
                continue;
            }
            if cos_t >= c.cos_star_reach {
                stars += self.stars_of(k, s);
            }
            if !(cos_t >= c.cos_reach) {
                continue;
            }
            let Some((px, py)) = c.sprite_point(s, cos_t) else { continue };
            let level = level_for(c.radius, texel);
            let v = self.sprites.sample(c.index, level, px, py);
            let tn = (-v[3] * self.dust_k).exp();
            let raw = v[0] + v[1] + v[2];
            if raw > 0.0 {
                let soft = self.sprites.sample(c.index, level.saturating_sub(2), px, py);
                let la = (self.art_peak * raw.min(1.0)).max(FLOOR * self.art_peak);
                let lr = (self.real_peak * (soft[0] + soft[1] + soft[2]).min(1.0)).max(FLOOR * self.real_peak);
                let lum = (la.powf(1.0 - self.realism) * lr.powf(self.realism) - self.floor).max(0.0);
                let art = (self.ha_rgb * v[0] + self.o3_rgb * v[1] + self.reflection_rgb * v[2]) * (1.0 / raw);
                let colour = art + (self.real_rgb - art) * self.chroma;
                // its own dust in front of half of it
                gas += colour * (lum * (0.5 + 0.5 * tn) * c.weight);
            }
            through *= 1.0 - c.weight * (1.0 - tn);
        }
        (gas * self.gain + stars, through)
    }

    /// Cloud `k`'s embedded stars at star-space `s`.
    fn stars_of(&self, k: usize, s: Vec3f) -> Vec3f {
        let mut stars = Vec3f::default();
        for star in self.stars.iter().filter(|star| star.cloud == k) {
            let d2 = Vec3f::cross(s, star.dir).length_squared();
            if d2 < STAR_REACH2 {
                stars += star.colour * (star.flux * star_kernel(d2));
            }
        }
        stars
    }

    /// The embedded stars alone at star-space `s`, as `at` adds them: points that Star brightness scales and no dust
    /// dims (`widths_agree` takes them off what the dusts' dimming of the field's stars is checked against).
    #[cfg(test)]
    pub(super) fn embedded_stars(&self, s: Vec3f) -> Vec3f {
        let mut stars = Vec3f::default();
        for (k, c) in self.placed.iter().enumerate() {
            if s.dot(c.centre) >= c.cos_star_reach {
                stars += self.stars_of(k, s);
            }
        }
        stars
    }

    /// `at`'s transmittance alone, bit for bit, without the gas, its stars and its colours: what the stars behind the
    /// clouds keep (`NightSky::dust_before_stars` reads it at texel 0).
    pub(super) fn transmittance(&self, s: Vec3f, texel: f32) -> f32 {
        let mut through = 1.0f32;
        for c in &self.placed {
            let cos_t = s.dot(c.centre);
            if !(cos_t >= c.cos_reach) {
                continue;
            }
            let Some((px, py)) = c.sprite_point(s, cos_t) else { continue };
            let v = self.sprites.sample(c.index, level_for(c.radius, texel), px, py);
            through *= 1.0 - c.weight * (1.0 - (-v[3] * self.dust_k).exp());
        }
        through
    }

    /// Each visible cloud's centre (star space) and angular radius (radians).
    pub(super) fn centres(&self) -> Vec<(Vec3f, f32)> {
        self.placed.iter().map(|c| (c.centre, c.radius)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::atmosphere::SUN_IRRADIANCE;

    /// The default glow's zenith before the lift (G0): night.rs's glow_color x GLOW_SCALE.
    fn g0() -> f32 {
        luminance(vec3f(0.02, 0.03, 0.06)) * 5.0e-8 * SUN_IRRADIANCE
    }

    fn saturation(c: Vec3f) -> f32 {
        let (hi, lo) = (c.x.max(c.y).max(c.z), c.x.min(c.y).min(c.z));
        if hi > 0.0 { (hi - lo) / hi } else { 0.0 }
    }

    /// 41 x 41 looks over cloud `i`'s disc, in star space.
    fn disc_looks(n: &Nebulas, i: usize) -> Vec<Vec3f> {
        let c = &n.placed[i];
        let mut out = Vec::new();
        for y in 0..41 {
            for x in 0..41 {
                let (px, py) = (x as f32 / 20.0 - 1.0, y as f32 / 20.0 - 1.0);
                if px * px + py * py < 0.98 {
                    out.push((c.centre + (c.e1 * px + c.e2 * py) * c.tan_radius).normalize());
                }
            }
        }
        out
    }

    /// The 99th percentile of the gas's luminance over cloud 0's disc at texel 0 (the finest level), in G0.
    fn p99_in_g0(n: &Nebulas) -> f32 {
        let mut l: Vec<f32> = disc_looks(n, 0).iter().map(|s| luminance(n.at(*s, 0.0).0) / g0()).collect();
        l.sort_by(f32::total_cmp);
        l[l.len() * 99 / 100]
    }

    /// Only candidate 0 shows (Amount one twelfth), with no embedded stars (star gain 0).
    fn first_cloud(seed: u32, realism: f32) -> Nebulas {
        let p = NebulaParams { amount: 1.0 / NEBULA_MAX as f32, realism, seed, ..NebulaParams::default() };
        Nebulas::new(&p, None, 1.0, 0.0, &[Vec3f::all(1.0)])
    }

    /// Plan hdri-7 §3, §5 (refs §5.3): at Brightness 1 a cloud's brightest part is 30 to 60 G0 at Realism 0 (as
    /// photographed) and 2 to 5 at Realism 1 (as the eye sees it); half way it is the geometric mean of the two within
    /// 20 %, and it falls steadily from 0 to 1.
    #[test]
    fn a_nebula_is_as_bright_as_its_realism_says() {
        let mut seen = 0;
        for seed in 1..=8u32 {
            if candidate(hash_u32(seed ^ NEBULA_SALT), 0).kind == NebulaKind::Dark {
                continue;
            }
            seen += 1;
            let peaks: Vec<f32> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|r| p99_in_g0(&first_cloud(seed, *r))).collect();
            println!("seed {seed}: peaks in G0 at Realism 0..1: {peaks:.2?}");
            assert!((30.0..=60.0).contains(&peaks[0]), "seed {seed}: Realism 0 peaks at {}", peaks[0]);
            assert!((2.0..=5.0).contains(&peaks[4]), "seed {seed}: Realism 1 peaks at {}", peaks[4]);
            assert!(peaks.windows(2).all(|w| w[1] < w[0]), "seed {seed}: not falling steadily: {peaks:?}");
            let geometric = (peaks[0] * peaks[4]).sqrt();
            assert!((peaks[2] / geometric - 1.0).abs() <= 0.2, "seed {seed}: half way {} against {geometric}", peaks[2]);
        }
        assert!(seen >= 3, "the premise: glowing first clouds ({seen})");
    }

    /// Realism 1 is nearly grey (saturation at most 0.12 wherever it shows); Realism 0 is the astrophotograph's colours
    /// (spec §3): saturation at least 0.5 where hydrogen-alpha leads; teal (green and blue over red) where oxygen-III
    /// leads; blue the largest channel where reflection leads. "Leads" for oxygen-III is over four times the other two
    /// together: hydrogen-alpha's red is about three times as luminous per unit, so a third of it already greys the teal.
    /// Seeds 1 to 24, so every kind is among the first clouds.
    #[test]
    fn a_nebulas_colour_follows_its_realism() {
        let texel = TAU / 1024.0;
        let (mut red, mut teal, mut blue) = (0, 0, 0);
        for seed in 1..=24u32 {
            if candidate(hash_u32(seed ^ NEBULA_SALT), 0).kind == NebulaKind::Dark {
                continue;
            }
            let (art, real) = (first_cloud(seed, 0.0), first_cloud(seed, 1.0));
            let looks = disc_looks(&art, 0);
            let most = looks.iter().map(|s| luminance(real.at(*s, texel).0)).fold(0.0, f32::max);
            let c0 = &art.placed[0];
            for s in &looks {
                let grey = real.at(*s, texel).0;
                if luminance(grey) >= 0.1 * most {
                    assert!(saturation(grey) <= 0.12, "seed {seed}: Realism 1 shows {grey:?}");
                }
                let k = 1.0 / (s.dot(c0.centre) * c0.tan_radius);
                let v = art.sprites.sample(c0.index, level_for(c0.radius, texel), s.dot(c0.e1) * k, s.dot(c0.e2) * k);
                let c = art.at(*s, texel).0;
                if v[0] > 0.2 && v[0] > 2.0 * (v[1] + v[2]) {
                    red += 1;
                    assert!(saturation(c) >= 0.5, "seed {seed}: hydrogen-alpha shows {c:?}");
                }
                if v[1] > 0.2 && v[1] > 4.0 * (v[0] + v[2]) {
                    teal += 1;
                    assert!(c.y > c.x && c.z > c.x, "seed {seed}: oxygen-III shows {c:?}, not teal");
                }
                if v[2] > 0.2 && v[2] > 2.0 * (v[0] + v[1]) {
                    blue += 1;
                    assert!(c.z > c.x && c.z > c.y, "seed {seed}: reflection shows {c:?}, not blue");
                }
            }
        }
        assert!(red > 50 && teal > 20 && blue > 20, "the premise: hydrogen-alpha {red}, oxygen-III {teal}, reflection {blue} looks");
    }

    /// Plan hdri-7 §3 ("each with bright knots and embedded stars"): a glowing cloud holds 3 to 8 stars at magnitudes 3
    /// to 6, drawn with the stars' own kernel; a dark cloud holds none. The gas off (gain 0), `at` is the stars alone: at
    /// each embedded star its own peak (its flux x the kernel's centre, at least), and 0.3 degrees off it (six core
    /// sigmas) under half of that, unless another of the cloud's stars stands within half a degree there.
    #[test]
    fn embedded_stars_show_in_the_glowing_clouds() {
        let (faint, bright) = (10f32.powf(0.4 * (STAR_MAG_LIMIT - 6.0)), 10f32.powf(0.4 * (STAR_MAG_LIMIT - 3.0)));
        let (mut glowing, mut dark) = (0, 0);
        for seed in 1..=24u32 {
            let p = NebulaParams { amount: 1.0 / NEBULA_MAX as f32, seed, ..NebulaParams::default() };
            let n = Nebulas::new(&p, None, 0.0, 1.0, &[Vec3f::all(1.0)]);
            if candidate(hash_u32(seed ^ NEBULA_SALT), 0).kind == NebulaKind::Dark {
                dark += 1;
                assert!(n.stars.is_empty(), "seed {seed}: a dark cloud holds no stars");
                continue;
            }
            glowing += 1;
            assert!((3..=8).contains(&n.stars.len()), "seed {seed}: {} embedded stars", n.stars.len());
            let side = n.placed[0].e1;
            for e in &n.stars {
                assert!(e.flux >= faint * 0.999 && e.flux <= bright * 1.001, "seed {seed}: flux {} (magnitudes 3 to 6)", e.flux);
                let peak = luminance(n.at(e.dir, 0.0).0);
                assert!(peak >= 0.999 * e.flux * star_kernel(0.0), "seed {seed}: the star's peak {peak}");
                let off = (e.dir + side * 0.3f32.to_radians()).normalize();
                if n.stars.iter().filter(|o| o.dir != e.dir).all(|o| o.dir.dot(off) < 0.5f32.to_radians().cos()) {
                    assert!(luminance(n.at(off, 0.0).0) < 0.5 * peak, "seed {seed}: a point, not a patch");
                }
            }
        }
        assert!(glowing >= 3 && dark >= 1, "the premise: both kinds among the first clouds ({glowing} glowing, {dark} dark)");
    }

    /// A seed keeps its layout across Realism, Brightness and Size (Size scales the radii), Amount shows a prefix of
    /// it, and another seed is another layout.
    #[test]
    fn a_seed_keeps_its_layout() {
        let at = |q: &NebulaParams| Nebulas::new(q, None, 1.0, 0.0, &[Vec3f::all(1.0)]).centres();
        let p = NebulaParams { amount: 1.0, ..NebulaParams::default() };
        let base = at(&p);
        assert_eq!(base.len(), NEBULA_MAX);
        assert_eq!(at(&NebulaParams { realism: 1.0, ..p.clone() }), base);
        assert_eq!(at(&NebulaParams { brightness: 5.0, ..p.clone() }), base);
        for (a, b) in at(&NebulaParams { size: 2.0, ..p.clone() }).iter().zip(&base) {
            assert_eq!(a.0, b.0);
            assert!((a.1 - 2.0 * b.1).abs() < 1.0e-6, "{} vs {}", a.1, b.1);
        }
        assert_eq!(at(&NebulaParams { amount: 0.5, ..p.clone() })[..], base[..6]);
        assert_ne!(at(&NebulaParams { seed: 8, ..p }), base);
    }

    /// Amount fades the candidates in one by one, never dropping one already in.
    #[test]
    fn amount_fades_them_in_one_by_one() {
        let total = |amount: f32| {
            Nebulas::new(&NebulaParams { amount, ..NebulaParams::default() }, None, 1.0, 0.0, &[Vec3f::all(1.0)]).placed.iter().map(|c| c.weight).sum::<f32>()
        };
        let mut last = -1.0;
        for i in 0..=24 {
            let w = total(i as f32 / 24.0);
            assert!(w >= last, "{i}: {w} after {last}");
            last = w;
        }
        assert_eq!((total(0.0), total(0.5), total(1.0)), (0.0, 6.0, NEBULA_MAX as f32));
    }

    /// With the Milky Way's axes the glowing clouds hug its plane (|b| about 5 degrees RMS), reflection and dark ones
    /// spread wider (about 15); without them they are uniform on the sphere.
    #[test]
    fn they_gather_along_the_milky_way() {
        let axes = [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)];
        let (mut glowing, mut wide, mut free) = (Vec::new(), Vec::new(), Vec::new());
        for seed in 0..300u32 {
            let seed = hash_u32(seed ^ NEBULA_SALT);
            for i in 0..NEBULA_MAX {
                let c = candidate(seed, i);
                let b = place(&c, Some(axes)).y.clamp(-1.0, 1.0).asin().to_degrees();
                match c.kind {
                    NebulaKind::Emission | NebulaKind::BiColour => glowing.push(b),
                    _ => wide.push(b),
                }
                free.push(place(&c, None));
            }
        }
        let rms = |v: &[f32]| (v.iter().map(|b| b * b).sum::<f32>() / v.len() as f32).sqrt();
        assert!((3.5..=7.0).contains(&rms(&glowing)), "glowing clouds at {} degrees RMS", rms(&glowing));
        assert!((10.0..=20.0).contains(&rms(&wide)), "reflection and dark clouds at {} degrees RMS", rms(&wide));
        let n = free.len() as f32;
        let (mean_y, mean_abs) = (free.iter().map(|d| d.y).sum::<f32>() / n, free.iter().map(|d| d.y.abs()).sum::<f32>() / n);
        assert!(mean_y.abs() < 0.05 && (mean_abs - 0.5).abs() < 0.05, "uniform: mean y {mean_y}, mean |y| {mean_abs}");
        assert!(free.iter().all(|d| (d.length() - 1.0).abs() < 1.0e-5));
    }

    /// D2: a seed's sprites are built once and shared: by every map of that seed, whatever Amount, Size, Realism or
    /// Brightness say; a new seed builds its own. (Seeds no other test uses; a map that holds its sprites keeps them
    /// in the memo.)
    #[test]
    fn a_seeds_sprites_are_built_once_and_shared() {
        use super::super::probe::{counted, Work};
        let p = NebulaParams { seed: 0x51ab_0001, ..NebulaParams::default() };
        let before = counted(Work::NebulaSprites);
        let a = Nebulas::new(&p, None, 1.0, 0.0, &[Vec3f::all(1.0)]);
        assert_eq!(counted(Work::NebulaSprites), before + 1);
        for q in [NebulaParams { realism: 1.0, ..p.clone() }, NebulaParams { amount: 1.0, ..p.clone() }, NebulaParams { size: 3.0, ..p.clone() }, NebulaParams { brightness: 5.0, ..p.clone() }] {
            let b = Nebulas::new(&q, None, 1.0, 0.0, &[Vec3f::all(1.0)]);
            assert!(Arc::ptr_eq(&a.sprites, &b.sprites), "{q:?}");
        }
        assert_eq!(counted(Work::NebulaSprites), before + 1, "no slider but the seed builds sprites");
        let _c = Nebulas::new(&NebulaParams { seed: 0x51ab_0002, ..p }, None, 1.0, 0.0, &[Vec3f::all(1.0)]);
        assert_eq!(counted(Work::NebulaSprites), before + 2, "a new seed builds its own");
    }

    /// Sprite rows filled on four threads hold the bits rows filled on one do.
    #[test]
    fn sprite_rows_filled_on_many_threads_hold_the_same_bits() {
        let (a, b) = (Sprites::new(0x51ab_0003), Sprites::new(0x51ab_0003));
        let looks: Vec<(f32, f32)> = (0..600).map(|i| ((i as f32 * 0.618_034).fract() * 2.0 - 1.0, (i as f32 * 0.414_214).fract() * 2.0 - 1.0)).collect();
        std::thread::scope(|s| {
            for t in 0..4 {
                let (sprites, looks) = (&a, &looks);
                s.spawn(move || looks.iter().skip(t).step_by(4).for_each(|(x, y)| { sprites.sample(t, 3, *x, *y); }));
            }
        });
        for (i, (x, y)) in looks.iter().enumerate() {
            assert_eq!(a.sample(i % 4, 3, *x, *y).map(f32::to_bits), b.sample(i % 4, 3, *x, *y).map(f32::to_bits));
        }
    }

    /// The level a lookup reads: the finest whose texel is at least the lookup's; the finest at texel 0, the coarsest
    /// when every level is finer.
    #[test]
    fn the_level_follows_the_texel() {
        let r = 3.0f32.to_radians();
        assert_eq!(level_for(r, 0.0), SPRITE_LEVELS.len() - 1);
        assert_eq!(level_for(r, 1.0), 0);
        for texel in [TAU / 512.0, TAU / 1024.0, TAU / 4096.0] {
            let l = level_for(r, texel);
            assert!(2.0 * r / SPRITE_LEVELS[l] as f32 >= texel || l == 0, "{texel}: level {l}");
            if l + 1 < SPRITE_LEVELS.len() {
                assert!(2.0 * r / (SPRITE_LEVELS[l + 1] as f32) < texel, "{texel}: level {l} is not the finest such");
            }
        }
    }
}
