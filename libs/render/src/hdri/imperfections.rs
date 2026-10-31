//! Surface imperfections of a light's diffuser (B10): wrinkles (ridged
//! noise), dust (hashed specks and a broad smudge) and a woven cloth (the
//! twill of draw/src/shader/finish.rs, ported to the CPU). They multiply the
//! face's emission around 1 and are renormalised over the light's rectangle,
//! so the light's mean emission, and with it intensity_ev, stays true. They
//! only show where the map is seen sharply (mirror-like reflections);
//! diffuse lighting averages them away.
//!
//! Positions are face coordinates; the patterns are laid out in s-units
//! (the light's smaller half size is 1), so they keep their scale on a wide
//! light.

use super::*;
use super::noise::{fbm2, hash01, value2};
use std::sync::Mutex;

/// Pattern sizes in s-units: a weave cell, a dust cell (at most one speck
/// each), and the wrinkles' base frequency.
const WEAVE_CELL: f32 = 0.045;
const DUST_CELL: f32 = 0.035;
const WRINKLE_FREQ: f32 = 2.5;
/// Strata per axis of the mean that normalises the patterns.
const MEAN_SAMPLES: i32 = 96;
/// Normalisers kept (`kept_norm`): a few bytes each, so enough for every worn
/// light of a busy studio and the sizes a drag of one passes through.
const NORMS_KEPT: usize = 256;

/// The normalisers sampled last, by what they depend on: the three amounts,
/// the seed and the face's half extents, as bits (D15, L:194). A held drag
/// builds every light's wear again for each bake, about 4.3 ms per worn
/// light in release before the bake's first row; a drag that keeps a face's
/// wear and size reads its normaliser here. The oldest goes first.
static NORMS: Mutex<Vec<([u32; 6], f32)>> = Mutex::new(Vec::new());

/// `sample()`, the normaliser of the wear and face that `key` names, kept so
/// that the next build of the same face reads it.
fn kept_norm(key: [u32; 6], sample: impl FnOnce() -> f32) -> f32 {
    if let Ok(norms) = NORMS.lock() {
        if let Some((_, norm)) = norms.iter().find(|(kept, _)| *kept == key) {
            return *norm;
        }
    }
    // Sampled outside the lock, so other faces' lookups do not wait for it.
    let norm = sample();
    if let Ok(mut norms) = NORMS.lock() {
        if !norms.iter().any(|(kept, _)| *kept == key) {
            if norms.len() >= NORMS_KEPT {
                norms.remove(0);
            }
            norms.push((key, norm));
        }
    }
    norm
}

/// The wear of one light's face, built once per light.
#[derive(Clone, Debug)]
pub struct Imperfections {
    wrinkle: f32,
    dust: f32,
    weave: f32,
    seed: u32,
    /// The s-unit: the light's smaller tangent half extent.
    s: f32,
    /// The raw pattern's mean over the face; `factor` divides by it.
    norm: f32,
}

impl Imperfections {
    /// The wear of a light with tangent half extents `half`. None when
    /// nothing is set, so a clean light pays nothing. The normaliser is kept
    /// for the wear and the face (`kept_norm`), so the same face built again
    /// (a held drag's next map) does not sample it again. The wear is read as
    /// clamp() holds it (`ImperfectionParams::clamped`, D17), so its ranges
    /// and its NaN rule are `ImperfectionParams::clamp_fields`' alone.
    pub fn new(p: &ImperfectionParams, half: Vec2f) -> Option<Imperfections> {
        let p = &p.clamped();
        let (wrinkle, dust, weave) = (p.wrinkle, p.dust, p.weave);
        if wrinkle == 0.0 && dust == 0.0 && weave == 0.0 {
            return None;
        }
        let half = vec2f(half.x.max(1.0e-4), half.y.max(1.0e-4));
        let mut im = Imperfections { wrinkle, dust, weave, seed: p.seed, s: half.x.min(half.y), norm: 1.0 };
        // Everything `mean_raw` reads: the amounts, the seed, and the face (its s-unit is the smaller half).
        let key = [wrinkle.to_bits(), dust.to_bits(), weave.to_bits(), p.seed, half.x.to_bits(), half.y.to_bits()];
        im.norm = kept_norm(key, || im.mean_raw(half));
        if !(im.norm > 1.0e-4) {
            return None;
        }
        Some(im)
    }

    /// The emission multiplier at face coordinates `q`: around 1, with mean
    /// 1 over the face.
    pub fn factor(&self, q: Vec2f) -> f32 {
        // A NaN direction leaves the light as it is.
        if !(q.x.is_finite() && q.y.is_finite()) {
            return 1.0;
        }
        self.raw(q / self.s) / self.norm
    }

    /// The product of the three patterns at `g` (s-units), before renormalising.
    fn raw(&self, g: Vec2f) -> f32 {
        let mut m = 1.0;
        if self.wrinkle > 0.0 {
            m *= self.wrinkles(g);
        }
        if self.dust > 0.0 {
            m *= self.dust_at(g);
        }
        if self.weave > 0.0 {
            m *= 1.0 - self.weave * 0.85 * (1.0 - weave(g, WEAVE_CELL));
        }
        m.max(0.0)
    }

    /// Folds in the diffusion cloth: ridged noise, warped a little so the
    /// folds wander. The crests catch light (up to about +80 %), the rest
    /// sits a little darker.
    fn wrinkles(&self, g: Vec2f) -> f32 {
        let w = g * WRINKLE_FREQ;
        let warp_x = fbm2(w.x * 0.5 + 3.1, w.y * 0.5, 2, self.seed ^ 0x51) - 0.5;
        let warp_y = fbm2(w.x * 0.5, w.y * 0.5 + 7.7, 2, self.seed ^ 0x52) - 0.5;
        let ridge = ridged(w.x + warp_x * 1.2, w.y + warp_y * 1.2, self.seed ^ 0x53);
        (1.0 + self.wrinkle * (1.2 * ridge - 0.4)).max(0.05)
    }

    /// Dust: at most one dark speck per cell, at a hashed spot, size and
    /// opacity, in about a third of the cells at full dust; over that, a
    /// broad smudge of slightly darker patches.
    fn dust_at(&self, g: Vec2f) -> f32 {
        let c = g / DUST_CELL;
        let (fx, fy) = (c.x.floor(), c.y.floor());
        let (ix, iy) = (fx as i32, fy as i32);
        let mut m = 1.0;
        if hash01(ix, iy, 0, self.seed) < self.dust * 0.3 {
            // The centre stays 0.25..0.75 of the cell and the radius under
            // 0.22, so a speck never crosses into a cell that is not looking.
            let centre = vec2f(0.25 + 0.5 * hash01(ix, iy, 1, self.seed), 0.25 + 0.5 * hash01(ix, iy, 2, self.seed));
            let radius = 0.06 + 0.16 * hash01(ix, iy, 3, self.seed);
            let opacity = 0.5 + 0.5 * hash01(ix, iy, 4, self.seed);
            let dist = (vec2f(c.x - fx, c.y - fy) - centre).length();
            m *= 1.0 - opacity * (1.0 - smoothstep(radius * 0.5, radius, dist));
        }
        let smudge = fbm2(g.x * 1.3 + 11.0, g.y * 1.3 - 7.0, 3, self.seed ^ 0x5d);
        m * (1.0 - self.dust * 0.4 * smoothstep(0.55, 0.8, smudge))
    }

    /// The raw pattern's mean over the light's rectangle `half`: jittered
    /// strata, weighted by solid angle (the gnomonic plane's
    /// dΩ = dA / (1 + |q|²)^(3/2)). That is how the map integrates the
    /// light's power, so dividing by it keeps the power.
    fn mean_raw(&self, half: Vec2f) -> f32 {
        #[cfg(test)]
        super::probe::count(super::probe::Work::WearNorm);
        let n = MEAN_SAMPLES;
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for j in 0..n {
            for i in 0..n {
                let jx = hash01(i, j, 11, self.seed);
                let jy = hash01(i, j, 12, self.seed);
                let q = vec2f(
                    ((i as f32 + jx) / n as f32 * 2.0 - 1.0) * half.x,
                    ((j as f32 + jy) / n as f32 * 2.0 - 1.0) * half.y,
                );
                let w = (1.0 + q.x * q.x + q.y * q.y).powf(-1.5) as f64;
                sum += w * self.raw(q / self.s) as f64;
                weight += w;
            }
        }
        (sum / weight.max(1.0e-12)) as f32
    }
}

/// Ridged multifractal noise over A2's value noise, 0..=1: each octave is
/// folded about its middle (1 - |2n - 1|) and squared, so the noise's 0.5
/// level lines become sharp crests, and an octave only shows where the one
/// before it was already near a crest. That keeps the detail on the ridges
/// and leaves the valleys smooth, the way a creased cloth reads.
fn ridged(x: f32, y: f32, seed: u32) -> f32 {
    let (mut px, mut py) = (x, y);
    let (mut amplitude, mut sum, mut carry) = (0.5f32, 0.0f32, 1.0f32);
    for octave in 0..4u32 {
        let n = value2(px, py, seed.wrapping_add(octave.wrapping_mul(0x9e37_79b9)));
        let fold = 1.0 - (2.0 * n - 1.0).abs();
        let ridge = fold * fold;
        sum += ridge * amplitude * carry;
        carry = ridge;
        amplitude *= 0.5;
        // fbm2's own octave step: turn by a 3-4-5 angle, double, shift.
        let (rx, ry) = (0.8 * px - 0.6 * py, 0.6 * px + 0.8 * py);
        px = rx * 2.0 + 17.13;
        py = ry * 2.0 + 5.71;
    }
    // 0.5 + 0.25 + 0.125 + 0.0625.
    (sum / 0.9375).min(1.0)
}

/// GLSL's floor-based fract. The prelude's `fract` truncates, which differs
/// for negative inputs, and the shader's hash relies on floor.
fn fr(x: f32) -> f32 {
    x - x.floor()
}

/// finish.rs's sin-free hash (`f_hash`), 0..1.
fn f_hash(p: Vec2f) -> f32 {
    let a = vec3f(fr(p.x * 0.1031), fr(p.y * 0.1031), fr(p.x * 0.1031));
    let k = a.dot(vec3f(a.y, a.z, a.x) + vec3f(33.33, 33.33, 33.33));
    let b = a + vec3f(k, k, k);
    fr((b.x + b.y) * b.z)
}

/// finish.rs's value noise (`f_noise`) with its quintic fade, 0..1.
fn f_noise(p: Vec2f) -> f32 {
    let i = vec2f(p.x.floor(), p.y.floor());
    let f = p - i;
    let ux = f.x * f.x * f.x * (f.x * (f.x * 6.0 - 15.0) + 10.0);
    let uy = f.y * f.y * f.y * (f.y * (f.y * 6.0 - 15.0) + 10.0);
    let a = f_hash(i);
    let b = f_hash(i + vec2f(1.0, 0.0));
    let c = f_hash(i + vec2f(0.0, 1.0));
    let d = f_hash(i + vec2f(1.0, 1.0));
    mix_f32(mix_f32(a, b, ux), mix_f32(c, d, ux), uy)
}

/// A two-over-two twill, ported line for line from finish.rs's `weave`:
/// brightness 0..1 at `p` with cells `scale` wide. Each tow floats over two
/// cells, is round across, dips where it passes under at either end, and the
/// tows running along x catch more light than the others. The one change
/// from the shader is the floor on `scale`: 0.5 there guarded a size in
/// points, 1e-4 here guards a division.
pub fn weave(p: Vec2f, scale: f32) -> f32 {
    let q = p / scale.max(1.0e-4);
    let cell = vec2f(q.x.floor(), q.y.floor());
    let f = q - cell;
    // Which of the twill's four phases this cell is in.
    let n = (fr((cell.x + cell.y) * 0.25) * 4.0 + 0.5).floor();
    // The shader's step(n, 1.5): 1 for phases 0 and 1, where the tow runs along x.
    let horiz = if n > 1.5 { 0.0 } else { 1.0 };
    let across = mix_f32(f.x, f.y, horiz);
    let run = n - 2.0 * (1.0 - horiz);
    let along = (run + mix_f32(f.y, f.x, horiz)) * 0.5;
    let first = vec2f(cell.x - mix_f32(0.0, run, horiz), cell.y - mix_f32(run, 0.0, horiz));
    let round = (across * std::f32::consts::PI).sin().max(0.0).powf(0.6);
    let fibre = 0.82 + 0.18 * f_noise(vec2f(along * 3.0 + first.x * 3.7, across * 5.0 + first.y * 5.3));
    let dip = smoothstep(0.0, 0.12, along) * smoothstep(0.0, 0.12, 1.0 - along);
    (round * fibre * (0.45 + 0.55 * dip) * (0.78 + 0.22 * horiz)).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid-angle weighted mean of the factor over the rectangle `half`, on a
    /// regular grid offset from the normaliser's own jittered samples.
    fn mean_factor(im: &Imperfections, half: Vec2f, n: usize) -> f32 {
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for j in 0..n {
            for i in 0..n {
                let q = vec2f(
                    ((i as f32 + 0.37) / n as f32 * 2.0 - 1.0) * half.x,
                    ((j as f32 + 0.61) / n as f32 * 2.0 - 1.0) * half.y,
                );
                let w = (1.0 + q.x * q.x + q.y * q.y).powf(-1.5) as f64;
                sum += w * im.factor(q) as f64;
                weight += w;
            }
        }
        (sum / weight) as f32
    }

    #[test]
    fn nothing_set_is_no_imperfection() {
        assert!(ImperfectionParams::default().is_clear());
        assert!(Imperfections::new(&ImperfectionParams::default(), vec2f(0.3, 0.2)).is_none());
        let broken = ImperfectionParams { dust: f32::NAN, ..ImperfectionParams::default() };
        assert!(Imperfections::new(&broken, vec2f(0.3, 0.2)).is_none(), "NaN reads as 0");
    }

    #[test]
    fn the_weave_port_matches_the_shader_at_known_points() {
        // Across a tow's edge (f.y = 0 in a cell whose tows run along x) it is 0.
        assert_eq!(weave(vec2f(0.5, 0.0), 1.0), 0.0);
        // The middle of an x tow: round = 1, no dip, so only the fibre noise 0.82..1 is left.
        let mid = weave(vec2f(0.5, 0.5), 1.0);
        assert!((0.82..=1.0).contains(&mid), "{mid}");
        // Cell (2, 0) runs its tows along y, which catch 0.78 of the light.
        let cross = weave(vec2f(2.5, 0.5), 1.0);
        assert!((0.63..=0.79).contains(&cross), "{cross}");
        for i in 0..500 {
            let p = vec2f(i as f32 * 0.0731 - 17.0, i as f32 * 0.0419 - 9.0);
            let w = weave(p, 0.7);
            assert!((0.0..=1.0).contains(&w), "{w} at {p:?}");
        }
    }

    #[test]
    fn the_wrinkles_are_ridges() {
        // Ridged noise: mostly low, with crests that reach well up.
        let values: Vec<f32> = (0..80 * 80).map(|k| ridged((k % 80) as f32 * 0.137, (k / 80) as f32 * 0.173, 7)).collect();
        assert!(values.iter().all(|v| (0.0..=1.0).contains(v)));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean > 0.2 && mean < 0.5, "{mean}");
        assert!(values.iter().any(|v| *v > 0.6) && values.iter().any(|v| *v < 0.1));
    }

    #[test]
    fn imperfections_keep_the_light_s_mean() {
        let half = vec2f(0.4, 0.3);
        for (wrinkle, dust, weave) in [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0), (0.5, 0.5, 0.5)] {
            let im = Imperfections::new(&ImperfectionParams { wrinkle, dust, weave, seed: 7 }, half).expect("something set");
            let m = mean_factor(&im, half, 300);
            assert!((m - 1.0).abs() < 0.015, "wrinkle {wrinkle} dust {dust} weave {weave}: mean {m}");
        }
    }

    #[test]
    fn each_imperfection_varies_the_light() {
        let half = vec2f(0.4, 0.3);
        for (wrinkle, dust, weave) in [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)] {
            let im = Imperfections::new(&ImperfectionParams { wrinkle, dust, weave, seed: 3 }, half).unwrap();
            let samples: Vec<f32> = (0..4000)
                .map(|k| {
                    let q = vec2f(((k % 80) as f32 / 79.0 * 2.0 - 1.0) * half.x, ((k / 80) as f32 / 49.0 * 2.0 - 1.0) * half.y);
                    im.factor(q)
                })
                .collect();
            let mean = samples.iter().sum::<f32>() / samples.len() as f32;
            let var = samples.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / samples.len() as f32;
            assert!(var > 1.0e-3, "wrinkle {wrinkle} dust {dust} weave {weave}: variance {var}");
            assert!(samples.iter().all(|s| s.is_finite() && *s >= 0.0 && *s < 4.0));
        }
    }

    /// D15 (L:194): a worn face's normaliser is sampled once for the wear
    /// and the face it fits. A held drag builds every light's wear again for
    /// each bake (about 4.3 ms per worn light in release, before a bake's
    /// first row); the normaliser depends on the wear and the face's size
    /// alone, so a drag that keeps them reuses it, to the bit, and another
    /// seed or size samples its own. (A seed no other test uses: the kept
    /// normalisers are shared by every thread.)
    #[test]
    fn a_worn_faces_normaliser_is_reused() {
        use crate::hdri::probe::{counted, Work};
        use crate::hdri::{Env, HdriParams, LightParams};
        let p = ImperfectionParams { wrinkle: 0.4, dust: 0.6, weave: 0.2, seed: 0x4837_0194 };
        let half = vec2f(0.41, 0.27);
        let before = counted(Work::WearNorm);
        let a = Imperfections::new(&p, half).unwrap();
        let b = Imperfections::new(&p, half).unwrap();
        assert_eq!(counted(Work::WearNorm), before + 1, "the same wear on the same face is sampled once");
        assert_eq!(a.norm.to_bits(), b.norm.to_bits());
        let other_seed = Imperfections::new(&ImperfectionParams { seed: p.seed + 1, ..p.clone() }, half).unwrap();
        let other_size = Imperfections::new(&p, vec2f(0.42, 0.27)).unwrap();
        assert_eq!(counted(Work::WearNorm), before + 3, "another seed or size samples its own");
        assert_ne!(other_seed.norm, a.norm);
        assert_ne!(other_size.norm, a.norm);
        assert_eq!(a.norm.to_bits(), a.mean_raw(half).to_bits(), "the kept normaliser is the one sampled");
        // A held drag of a worn light across the sky: its face keeps its size.
        let mut params = HdriParams { mode: "studio".to_string(), ..HdriParams::default() };
        params.lights.push(LightParams { imperfections: Some(ImperfectionParams { seed: p.seed + 2, ..p.clone() }), ..LightParams::default() });
        let counted_before = counted(Work::WearNorm);
        for azimuth_deg in [10.0, 10.5, 11.0] {
            params.lights[0].azimuth_deg = azimuth_deg;
            let _ = Env::new(&params);
        }
        assert_eq!(counted(Work::WearNorm), counted_before + 1, "a drag samples the face once");
    }

    #[test]
    fn imperfections_are_fixed_by_their_seed() {
        let half = vec2f(0.4, 0.3);
        let p = ImperfectionParams { wrinkle: 0.5, dust: 0.5, weave: 0.5, seed: 21 };
        let a = Imperfections::new(&p, half).unwrap();
        let b = Imperfections::new(&p, half).unwrap();
        let c = Imperfections::new(&ImperfectionParams { seed: 22, ..p }, half).unwrap();
        let mut differs = false;
        for k in 0..400 {
            let q = vec2f(((k % 20) as f32 / 19.0 * 2.0 - 1.0) * half.x, ((k / 20) as f32 / 19.0 * 2.0 - 1.0) * half.y);
            assert_eq!(a.factor(q), b.factor(q));
            differs |= a.factor(q) != c.factor(q);
        }
        assert!(differs);
        assert_eq!(a.factor(vec2f(f32::NAN, 0.0)), 1.0, "a NaN direction leaves the light as it is");
    }
}
