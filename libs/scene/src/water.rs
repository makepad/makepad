//! Published water surfaces, the shared wave coefficients and the sea look.
//!
//! A water surface's waves are up to [`MAX_WAVES`] analytic [`WaterWave`]s
//! (local/plans/water.md): the sim sums them per tick (the physics truth:
//! buoyancy, swimming, boats) and the renderer's vertex shader adds the same
//! expression. A [`SeaLook`] derives them from a wind spectrum and a seed,
//! so every machine shows the same sea; the small ripples are a tiling
//! detail normal map made from the same spectrum ([`SeaLook::detail_map`]).
//! The waves are deterministic (`makepad_math::deterministic`).
use makepad_math::deterministic as dm;
use makepad_math::*;

/// One analytic wave component. All fields are the RAW coefficients the
/// canonical expression consumes — the render path uploads them unmodified
/// (no unit conversion that could drift between CPU and GPU).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterWave {
    /// Unit travel direction on the ground plane.
    pub dir_x: f32,
    pub dir_z: f32,
    /// Amplitude, world units (crest = level + amp for a steady wave).
    pub amp: f32,
    /// Spatial frequency k = τ / wavelength.
    pub k: f32,
    /// Temporal frequency ω = k · phase-speed.
    pub omega: f32,
    /// Phase offset, radians.
    pub phase: f32,
    /// 0 = steady. ≥ 1 = set-wave train: the envelope `(0.5+0.5·cos(phase/
    /// group))²` makes every `group`-th crest the big one (surf sets).
    pub group: f32,
}

impl WaterWave {
    /// Build from the authorable quantities: direction (normalized here),
    /// amplitude, wavelength and phase speed.
    pub fn new(dir_x: f32, dir_z: f32, amp: f32, len: f32, speed: f32) -> Self {
        let l = dm::hypot(dir_x, dir_z);
        let (dx, dz) = if l > 1.0e-6 {
            (dir_x / l, dir_z / l)
        } else {
            (1.0, 0.0)
        };
        let k = std::f32::consts::TAU / len.max(0.05);
        Self {
            dir_x: dx,
            dir_z: dz,
            amp: amp.max(0.0),
            k,
            omega: k * speed.max(0.0),
            phase: 0.0,
            group: 0.0,
        }
    }
}

/// Wave slots the render surface mirrors per volume. The sim accepts no more
/// either (the verbs warn and drop extras), so physics and visuals can never
/// disagree about which waves exist.
pub const MAX_WAVES: usize = 8;

/// Texels per side of the detail normal map.
pub const DETAIL_N: usize = 128;

/// How a body of water looks and how its wind sea is made. Presets are data
/// ([`SeaLook::preset`]); every field is tunable from a game's Splash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeaLook {
    /// Unit wind direction on the ground plane (waves travel along it).
    pub wind_dir: [f32; 2],
    /// Wind speed, m/s at 10 m: the spectrum's peak and its energy.
    pub wind: f32,
    /// Fetch, metres of open water the wind blew over (a lake is short).
    pub fetch: f32,
    /// Multiplies every wave height (a sandbox world is smaller than an
    /// ocean; 1 = the physical spectrum).
    pub height: f32,
    /// Strength of the small ripples (the detail normal map), 0..2.
    pub detail: f32,
    /// Share of wave energy spread away from the wind direction (0..1).
    pub spread: f32,
    pub seed: u32,
    /// The world's gravity (deep-water dispersion ω² = g·k).
    pub gravity: f32,
    /// Whitecap amount: how readily a pinched crest foams (0 = never).
    pub foam: f32,
    /// Seconds a whitecap's foam takes to fade to a third.
    pub foam_decay: f32,
    /// Foam where the water is shallow (surf on the beach), 0..1.
    pub shore_foam: f32,
    /// Linear colour the water scatters in the shallows / in the deep.
    pub shallow: [f32; 3],
    pub deep: [f32; 3],
    /// Metres of water a view sees through before it is mostly colour.
    pub clarity: f32,
    /// Strength of the caustics on the seabed (0 = none).
    pub caustics: f32,
    /// Light the liquid gives off itself, linear (lava, glowing slime).
    pub glow: [f32; 3],
}

impl Default for SeaLook {
    fn default() -> Self {
        Self::preset("calm").unwrap()
    }
}

/// The preset table: (name, wind, fetch, height, detail, spread, foam,
/// foam_decay, shore_foam). Colours come from the biome (`temperate`).
const PRESETS: &[(&str, f32, f32, f32, f32, f32, f32, f32, f32)] = &[
    ("calm", 3.5, 5_000.0, 1.0, 0.6, 0.25, 0.15, 2.0, 0.5),
    ("choppy", 9.0, 30_000.0, 1.0, 0.9, 0.2, 0.8, 3.0, 0.8),
    ("storm", 17.0, 60_000.0, 0.75, 1.1, 0.15, 1.6, 5.0, 1.0),
];

/// Biome colours: (name, shallow, deep, clarity, caustics, glow). Linear
/// RGB. Slime and lava are liquids rather than waters: murky, glowing.
const BIOMES: &[(&str, [f32; 3], [f32; 3], f32, f32, [f32; 3])] = &[
    ("tropical", [0.05, 0.62, 0.55], [0.005, 0.10, 0.20], 14.0, 1.0, [0.0; 3]),
    ("temperate", [0.06, 0.32, 0.30], [0.006, 0.05, 0.09], 7.0, 0.7, [0.0; 3]),
    ("arctic", [0.08, 0.30, 0.36], [0.008, 0.045, 0.08], 9.0, 0.5, [0.0; 3]),
    ("murky", [0.10, 0.20, 0.10], [0.02, 0.05, 0.04], 2.0, 0.15, [0.0; 3]),
    ("lake", [0.08, 0.26, 0.22], [0.01, 0.06, 0.07], 4.0, 0.5, [0.0; 3]),
    ("slime", [0.12, 0.45, 0.05], [0.03, 0.12, 0.01], 0.6, 0.0, [0.05, 0.25, 0.02]),
    ("lava", [0.35, 0.06, 0.01], [0.12, 0.02, 0.0], 0.2, 0.0, [3.0, 0.9, 0.12]),
];

impl SeaLook {
    pub const PRESETS: [&'static str; 3] = ["calm", "choppy", "storm"];
    pub const BIOMES: [&'static str; 7] = ["tropical", "temperate", "arctic", "murky", "lake", "slime", "lava"];

    /// A named sea state with temperate colours; None for an unknown name.
    pub fn preset(name: &str) -> Option<Self> {
        let p = PRESETS.iter().find(|p| p.0 == name)?;
        let mut look = Self {
            wind_dir: [0.0, 1.0],
            wind: p.1,
            fetch: p.2,
            height: p.3,
            detail: p.4,
            spread: p.5,
            seed: 1,
            gravity: 9.81,
            foam: p.6,
            foam_decay: p.7,
            shore_foam: p.8,
            shallow: [0.0; 3],
            deep: [0.0; 3],
            clarity: 1.0,
            caustics: 0.0,
            glow: [0.0; 3],
        };
        look.set_biome("temperate");
        Some(look)
    }

    /// Apply a biome's colours; false for an unknown name.
    pub fn set_biome(&mut self, name: &str) -> bool {
        let Some(b) = BIOMES.iter().find(|b| b.0 == name) else { return false };
        self.shallow = b.1;
        self.deep = b.2;
        self.clarity = b.3;
        self.caustics = b.4;
        self.glow = b.5;
        true
    }

    /// Colours from one authored sRGB colour (`game.water({color})`): it is
    /// the shallow tint; the deep colour is the same hue, far darker.
    pub fn set_colour(&mut self, srgb: Vec4f) {
        let lin = |c: f32| {
            let c = c.clamp(0.0, 1.0);
            if c <= 0.04045 { c / 12.92 } else { dm::pow((c + 0.055) / 1.055, 2.4) }
        };
        let s = [lin(srgb.x), lin(srgb.y), lin(srgb.z)];
        self.shallow = s;
        self.deep = [s[0] * 0.12, s[1] * 0.16, s[2] * 0.22];
    }

    pub fn set_wind_dir(&mut self, x: f32, z: f32) {
        let l = dm::hypot(x, z);
        if l > 1.0e-6 {
            self.wind_dir = [x / l, z / l];
        }
    }

    fn g(&self) -> f32 {
        self.gravity.max(0.1)
    }

    /// JONSWAP peak frequency ωp, rad/s.
    pub fn peak_omega(&self) -> f32 {
        let (u, f, g) = (self.wind.max(0.5), self.fetch.max(100.0), self.g());
        22.0 * dm::pow(g * g / (u * f), 1.0 / 3.0)
    }

    /// Peak wavelength, metres.
    pub fn peak_wavelength(&self) -> f32 {
        let wp = self.peak_omega();
        std::f32::consts::TAU * self.g() / (wp * wp)
    }

    /// JONSWAP frequency spectrum S(ω) (m²·s), heights scaled by `height`.
    pub fn spectrum(&self, omega: f32) -> f32 {
        if omega <= 1.0e-4 {
            return 0.0;
        }
        let (u, f, g) = (self.wind.max(0.5), self.fetch.max(100.0), self.g());
        let alpha = 0.076 * dm::pow(u * u / (f * g), 0.22);
        let wp = self.peak_omega();
        let sigma = if omega <= wp { 0.07 } else { 0.09 };
        let d = (omega - wp) / (sigma * wp);
        let r = dm::exp(-0.5 * d * d);
        let q = wp / omega;
        let pm = alpha * g * g / (omega * omega * omega * omega * omega) * dm::exp(-1.25 * q * q * q * q);
        pm * dm::pow(3.3, r) * self.height * self.height
    }

    /// The swell band as analytic waves: [`MAX_WAVES`] components around the
    /// spectral peak, spread about the wind, seeded. This is what the sim
    /// sums for physics and what the vertex shader adds.
    pub fn swell_waves(&self) -> Vec<WaterWave> {
        let wp = self.peak_omega();
        let g = self.g();
        let mut rng = Rng::new(self.seed as u64 ^ 0x5357_454c_4c00);
        let (lo, hi) = (0.72 * wp, 1.7 * wp);
        let n = MAX_WAVES;
        let dw = (hi - lo) / n as f32;
        let base = dm::atan2(self.wind_dir[1], self.wind_dir[0]);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            // Jittered frequencies: no two waves share a period, so the sum
            // never visibly repeats.
            let w = lo + dw * (i as f32 + 0.25 + 0.5 * rng.uniform());
            let e = self.spectrum(w) * dw;
            let amp = dm::sqrt(2.0 * e.max(0.0));
            let spread = 0.35 + 0.9 * self.spread;
            let a = base + spread * (rng.uniform() * 2.0 - 1.0);
            let (s, c) = dm::sincos(a);
            let k = w * w / g;
            let mut wave = WaterWave::new(c, s, amp, std::f32::consts::TAU / k, w / k);
            wave.phase = rng.uniform() * std::f32::consts::TAU;
            out.push(wave);
        }
        out
    }

    /// The detail normal map: the wind sea's short waves (a quarter of the
    /// peak wavelength and down) as a tiling `DETAIL_N²` map of slopes, one
    /// metre per `size / DETAIL_N`. Lattice waves only, so it tiles exactly;
    /// returned as BGRA8 texels (x slope in R, z slope in G, 0.5 = flat,
    /// 0 and 1 = ∓1), the tile size in metres and the slope the texels are
    /// normalised by (1 in the texture = `slope` of the summed field). The
    /// renderer scrolls two copies of it at different scales and directions;
    /// made once per look.
    pub fn detail_map(&self) -> (Vec<u32>, f32, f32) {
        let n = DETAIL_N;
        let size = (self.peak_wavelength() * 0.5).clamp(4.0, 24.0);
        let dk = std::f32::consts::TAU / size;
        let mut rng = Rng::new((self.seed as u64) << 8 ^ 0x4445_5441_494c);
        // ~96 lattice waves around the wind, energy from the spectrum's tail
        // (k⁻⁴ slopes flatten out, so every octave reads).
        let mut waves: Vec<(f32, f32, f32, f32)> = Vec::new();
        for i in 0..96 {
            let oct = 2.0 + 14.0 * (i as f32 / 96.0) * (0.75 + 0.5 * rng.uniform());
            let base = dm::atan2(self.wind_dir[1], self.wind_dir[0]);
            let a = base + (rng.uniform() * 2.0 - 1.0) * (0.7 + 1.6 * self.spread);
            let (s, c) = dm::sincos(a);
            let (nx, nz) = ((c * oct).round(), (s * oct).round());
            if nx == 0.0 && nz == 0.0 {
                continue;
            }
            let k = dm::hypot(nx, nz) * dk;
            // Slope amplitude ~ k·amp with amp ~ k^-1.6: gentle falloff.
            let slope = dm::pow(k / dk, -0.6);
            waves.push((nx * dk, nz * dk, slope, rng.uniform() * std::f32::consts::TAU));
        }
        let mut sx = vec![0.0f32; n * n];
        let mut sz = vec![0.0f32; n * n];
        let mut peak = 1.0e-6f32;
        for z in 0..n {
            for x in 0..n {
                let (px, pz) = (x as f32 * size / n as f32, z as f32 * size / n as f32);
                let (mut ax, mut az) = (0.0, 0.0);
                for (kx, kz, a, ph) in &waves {
                    // Visual only and made once: the platform cosine.
                    let c = (kx * px + kz * pz + ph).cos() * a;
                    let k = dm::hypot(*kx, *kz);
                    ax += c * kx / k;
                    az += c * kz / k;
                }
                sx[z * n + x] = ax;
                sz[z * n + x] = az;
                peak = peak.max(ax.abs()).max(az.abs());
            }
        }
        let q = |v: f32| ((v / peak * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u32;
        let texels = (0..n * n).map(|i| 0xFF00_0000 | q(sx[i]) << 16 | q(sz[i]) << 8 | 0x80).collect();
        (texels, size, peak)
    }
}

/// SplitMix64 with Box-Muller: the seeded randomness of a sea.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn uniform(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A water surface given as a mesh instead of a flat level (a river's
/// ribbon down its channel, an imported level's liquid faces): world
/// positions, triangles, and the vertex spacing in metres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub spacing: f32,
}

impl WaterMesh {
    /// Does the mesh cover (x, z) seen from above?
    pub fn covers_xz(&self, x: f32, z: f32) -> bool {
        self.indices.chunks_exact(3).any(|t| {
            let (Some(a), Some(b), Some(c)) = (
                self.positions.get(t[0] as usize),
                self.positions.get(t[1] as usize),
                self.positions.get(t[2] as usize),
            ) else {
                return false;
            };
            let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
            if d.abs() < 1.0e-9 {
                return false;
            }
            let u = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
            let v = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
            u >= 0.0 && v >= 0.0 && u + v <= 1.0
        })
    }
}

/// A visible water region, with its still surface at `max.y`.
#[derive(Clone, Debug)]
pub struct WaterSurface {
    pub min: Vec3f,
    pub max: Vec3f,
    pub waves: Vec<WaterWave>,
    pub color: Vec4f,
    pub entity: u64,
    pub draw_sheet: bool,
    /// The sea look: colours, ripples, foam.
    pub look: SeaLook,
    /// A sea: the surface is drawn to the horizon, not clamped to the box.
    pub unbounded: bool,
    /// Drawn as this mesh (world positions) instead of the box's top.
    pub mesh: Option<std::sync::Arc<WaterMesh>>,
}
impl WaterSurface {
    pub fn level(&self) -> f32 { self.max.y }
    pub fn amp_sum(&self) -> f32 { self.waves.iter().map(|w| w.amp).sum() }
    /// The swell's height at (x, z, t): the canonical sum (the sim's own
    /// expression; the renderer uses it to know whether the eye is under
    /// water).
    pub fn swell_height(&self, x: f32, z: f32, t: f32) -> f32 {
        let mut h = self.level();
        for w in &self.waves {
            let phase = w.k * (w.dir_x * x + w.dir_z * z) - w.omega * t + w.phase;
            let (s, _) = dm::sincos(phase);
            let env = if w.group > 0.0 {
                let e = 0.5 + 0.5 * dm::cos(phase / w.group);
                e * e
            } else {
                1.0
            };
            h += w.amp * env * s;
        }
        h
    }
    /// Does the surface cover (x, z)? A mesh-drawn one only where its
    /// mesh is (a level's pool, not the box around all of its pools).
    pub fn covers_xz(&self, x: f32, z: f32) -> bool {
        self.unbounded
            || (x >= self.min.x && x <= self.max.x && z >= self.min.z && z <= self.max.z
                && self.mesh.as_ref().is_none_or(|m| m.covers_xz(x, z)))
    }
}
#[derive(Clone, Default)]
pub struct WaterView {
    pub rev: u64,
    pub volumes: Vec<WaterSurface>,
    /// The ground under the water (the sim's heightfield, drawn or not):
    /// the renderer's depth colour, shore foam and caustics read it.
    pub bed: Option<std::sync::Arc<crate::Terrain>>,
}

/// Wave-sum time for a tick. f32 on purpose: the shader receives the same
/// f32, so both sides reduce identically. Accuracy (not determinism) of the
/// sum degrades as ω·t grows — days of continuous runtime; documented.
#[inline]
pub fn tick_time(tick: u64, dt: f32) -> f32 {
    tick as f32 * dt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_order_by_wave_height() {
        let hs = |name: &str| {
            let l = SeaLook::preset(name).unwrap();
            l.swell_waves().iter().map(|w| w.amp * w.amp * 0.5).sum::<f32>().sqrt() * 4.0
        };
        let (calm, choppy, storm) = (hs("calm"), hs("choppy"), hs("storm"));
        assert!(calm < choppy && choppy < storm, "Hs calm {calm} choppy {choppy} storm {storm}");
        assert!(calm > 0.01 && storm < 6.0, "Hs calm {calm} storm {storm}");
    }

    #[test]
    fn swell_and_detail_are_deterministic() {
        let l = SeaLook::preset("choppy").unwrap();
        assert_eq!(l.swell_waves(), l.swell_waves());
        assert_eq!(l.detail_map().0, l.detail_map().0);
        let mut other = l;
        other.seed = 2;
        assert_ne!(l.swell_waves(), other.swell_waves());
    }
}
