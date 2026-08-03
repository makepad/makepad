//! Smooth heightfield terrain generation.
//!
//! Pure: no `Cx`, no script VM, no GPU. That matters for two reasons. The
//! obvious one is testability — the whole landscape can be generated and
//! measured in a unit test, and dumped to a PNG to be looked at. The less
//! obvious one is that a host without a script VM (arcade's built-in demo
//! world) still needs terrain, and previously could not have it: the only
//! generator lived inside the `game.terrain` script verb.
//!
//! # What this replaces
//!
//! The original generator was single-octave value noise. One frequency means
//! detail at exactly one scale, which reads as melted blobs — no ridgelines,
//! no small features on big slopes, nothing to suggest the ground was made by
//! anything. It also indexed the noise by CELL INDEX rather than world
//! position, so raising `cells` for a smoother mesh silently generated a
//! completely different landscape. Resolution should buy detail, never a new
//! world; frequencies here are per world unit.
//!
//! # Determinism
//!
//! Two devices must generate byte-identical terrain from one seed, so the
//! only transcendental used is [`makepad_game_math::pow`]. Everything else is
//! add/sub/mul/div and an integer hash, all of which IEEE-754 pins exactly.

use makepad_math::*;

/// Where the ground stops being ground and becomes a wall, in metres of rise
/// per metre of run. Above this a surface reads as cliff regardless of how
/// high it is, which is what `rock` colours and what a walker cannot climb.
pub const DEFAULT_ROCK_SLOPE: f32 = 0.85;

/// A flattened disc — a plaza, a village green, an airstrip. Terrain that is
/// interesting everywhere is terrain you cannot put a town on, so the ability
/// to say "flat HERE" is not a special case, it is the other half of the
/// feature.
#[derive(Clone, Copy, Debug)]
pub struct TerrainPlaza {
    pub center_x: f32,
    pub center_z: f32,
    /// Fully flat inside this radius.
    pub radius: f32,
    /// Blends back to natural terrain across this much further out.
    pub ramp: f32,
    pub height: f32,
}

/// Colours by height AND slope.
///
/// Height alone paints a landscape in horizontal stripes, which is why
/// band-only terrain reads as a contour map. Slope is what puts rock on the
/// cliff faces and grass on the shelf directly above them, and it is most of
/// the reason real terrain looks like terrain.
#[derive(Clone, Copy, Debug)]
pub struct TerrainPalette {
    /// Valley floors and plains.
    pub low: Vec4f,
    /// Hilltops and uplands.
    pub high: Vec4f,
    /// Steep faces, applied over whatever the height said.
    pub rock: Vec4f,
    /// A shoreline band just above `water_level`.
    pub sand: Vec4f,
    pub water_level: f32,
    /// How far above the water the sand reaches.
    pub sand_band: f32,
    /// Slope at which `rock` fully takes over.
    pub rock_slope: f32,
    /// Per-vertex tint jitter, 0..1. A landscape shaded by a pure function of
    /// height and slope looks printed; a little noise in the colour is what
    /// makes flat-shaded triangles read as ground.
    pub mottle: f32,
}

impl Default for TerrainPalette {
    fn default() -> Self {
        Self {
            low: vec4(0.33, 0.55, 0.28, 1.0),
            high: vec4(0.46, 0.60, 0.34, 1.0),
            rock: vec4(0.44, 0.42, 0.40, 1.0),
            sand: vec4(0.76, 0.71, 0.50, 1.0),
            water_level: f32::MIN,
            sand_band: 1.2,
            rock_slope: DEFAULT_ROCK_SLOPE,
            mottle: 0.06,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TerrainParams {
    pub seed: u64,
    /// World size of one side. The grid is square and centred on the origin.
    pub span: f32,
    /// Vertices per side.
    pub cells: usize,
    /// Height the landscape varies around.
    pub base: f32,
    /// Peak-to-trough height range before clamping.
    pub amp: f32,
    /// World size of the LARGEST landform, in metres. Stated as a size rather
    /// than a frequency because that is the question a caller can actually
    /// answer: "how far apart are the hills?" is knowable, "what is a good
    /// value for freq?" is not.
    pub feature_size: f32,
    /// How many times to add a finer, weaker copy of the noise. 1 is the old
    /// single-scale behaviour; 4-5 is where a slope starts having texture.
    pub octaves: u32,
    /// Frequency multiplier per octave.
    pub lacunarity: f32,
    /// Amplitude multiplier per octave.
    pub gain: f32,
    /// Domain warp, in units of `feature_size`. Displacing the sample point by
    /// another noise field bends the contours; without it value noise makes
    /// round blobs on a visible grid, with it you get flowing ridges and
    /// valleys. The single highest-value knob here.
    pub warp: f32,
    /// 0 = rolling hills, 1 = ridged (sharp crests, rounded valleys).
    pub ridged: f32,
    /// Exponent on the normalised height. Above 1 pushes the landscape toward
    /// its floor, so you get plains with occasional hills rather than
    /// wall-to-wall lumpiness — which is what makes terrain playable, since a
    /// world with no flat ground has nowhere to put anything.
    pub flatten: f32,
    /// Quantise heights to this step. 0 disables. NOT defaulted on: the old
    /// generator terraced to 1.0 unit by default, which chopped every smooth
    /// slope into stairs.
    pub terrace: f32,
    pub min_height: f32,
    pub max_height: f32,
    pub plaza: Option<TerrainPlaza>,
    /// How much MORE relief the map edge gets than its middle. 0 is uniform;
    /// 3.0 means the outskirts swing about four times as far as the centre.
    ///
    /// This answers the tension that otherwise makes terrain a choice between
    /// boring and unusable: dial the noise up and the middle of the map is
    /// nowhere you can put a town or drive a car; leave it down and the
    /// horizon is a flat green line. Growing the relief outward gives a
    /// playable basin ringed by something worth looking at, and doubles as a
    /// soft boundary — players turn back at hills without hitting an
    /// invisible wall.
    ///
    /// It scales the noise rather than ADDING a radial ramp, which is the
    /// version that does not work: a ramp is a pure function of radius, so it
    /// has no noise in it and renders as a smooth machined ring separating two
    /// flat plains. Scaling keeps the landscape's own shapes and just makes
    /// them bigger further out.
    pub rim_relief: f32,
    /// Fraction of the half-span at which the ground starts growing (0..1).
    pub rim_start: f32,
    pub palette: TerrainPalette,
}

impl Default for TerrainParams {
    fn default() -> Self {
        Self {
            seed: 0,
            span: 128.0,
            cells: 129,
            base: 0.0,
            amp: 9.0,
            feature_size: 70.0,
            octaves: 5,
            lacunarity: 2.0,
            gain: 0.5,
            warp: 0.35,
            ridged: 0.0,
            flatten: 1.35,
            terrace: 0.0,
            min_height: f32::MIN,
            max_height: f32::MAX,
            plaza: None,
            rim_relief: 0.0,
            rim_start: 0.45,
            palette: TerrainPalette::default(),
        }
    }
}

/// The generated field. Deliberately not `sim::Terrain`: this crate does not
/// depend on the simulation, and the caller assembling the two extra fields is
/// cheaper than the coupling would be.
#[derive(Clone, Debug)]
pub struct TerrainField {
    pub cells: usize,
    pub cell_size: f32,
    /// World x/z of vertex (0,0).
    pub origin: f32,
    pub heights: Vec<f32>,
    pub colors: Vec<Vec4f>,
    pub min_height: f32,
    pub max_height: f32,
}

impl TerrainField {
    #[inline]
    pub fn height(&self, gx: usize, gz: usize) -> f32 {
        self.heights[gz * self.cells + gx]
    }

    /// Ground height at a world position, matching `sim::Terrain::height_at`'s
    /// triangle split so a preview and the real collision agree.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let fx = (x - self.origin) / self.cell_size;
        let fz = (z - self.origin) / self.cell_size;
        let max = (self.cells - 1) as f32;
        if fx < 0.0 || fz < 0.0 || fx >= max || fz >= max {
            return None;
        }
        let ix = fx as usize;
        let iz = fz as usize;
        let u = fx - ix as f32;
        let v = fz - iz as f32;
        let h = |gx: usize, gz: usize| self.height(gx, gz);
        let (h00, h10, h01, h11) = (h(ix, iz), h(ix + 1, iz), h(ix, iz + 1), h(ix + 1, iz + 1));
        Some(if u + v < 1.0 {
            h00 + (h10 - h00) * u + (h01 - h00) * v
        } else {
            h11 + (h01 - h11) * (1.0 - u) + (h10 - h11) * (1.0 - v)
        })
    }
}

/// One lattice value in 0..1. Integer avalanche, so neighbouring cells are
/// uncorrelated and the same (seed, x, z) is the same value on every device.
#[inline]
fn lattice(seed: u64, x: i64, z: i64) -> f32 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((x as u64).wrapping_mul(0x2545_F491_4F6C_DD1D))
        .wrapping_add((z as u64).wrapping_mul(0x27D4_EB2F_1656_67C5));
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 11) as f32 / (1u64 << 53) as f32
}

#[inline]
fn smoothstep01(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear value noise in 0..1 at a point in lattice space.
fn value_noise(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = floor_i64(x);
    let z0 = floor_i64(z);
    let tx = x - x0 as f32;
    let tz = z - z0 as f32;
    let sx = smoothstep01(tx);
    let sz = smoothstep01(tz);
    let h00 = lattice(seed, x0, z0);
    let h10 = lattice(seed, x0 + 1, z0);
    let h01 = lattice(seed, x0, z0 + 1);
    let h11 = lattice(seed, x0 + 1, z0 + 1);
    h00 + (h10 - h00) * sx + (h01 - h00) * sz + (h00 - h10 - h01 + h11) * sx * sz
}

/// `f32::floor` on the negative side of zero is the classic off-by-one in
/// lattice noise: `as i64` truncates toward zero, so x = -0.3 and x = +0.3
/// would share cell 0 and the field would mirror about the origin.
#[inline]
fn floor_i64(x: f32) -> i64 {
    let t = x as i64;
    if x < 0.0 && (x - t as f32) != 0.0 {
        t - 1
    } else {
        t
    }
}

/// Multi-octave noise, optionally ridged. Range is NOT 0..1 — see
/// [`generate`], which normalises the whole field to its own extents.
fn fbm(params: &TerrainParams, x: f32, z: f32) -> f32 {
    let mut amplitude = 1.0f32;
    let mut total = 0.0f32;
    let mut fx = x;
    let mut fz = z;
    for octave in 0..params.octaves.max(1) {
        let n = value_noise(params.seed ^ (octave as u64).wrapping_mul(0x9E37_79B9), fx, fz);
        // Ridged: fold the field about its midpoint so the maxima become
        // creases. Blended rather than switched, so `ridged` can be dialled.
        let ridge = 1.0 - (n * 2.0 - 1.0).abs();
        let v = n + (ridge - n) * params.ridged;
        total += v * amplitude;
        amplitude *= params.gain;
        fx *= params.lacunarity;
        fz *= params.lacunarity;
    }
    total
}

/// Generate a heightfield.
pub fn generate(params: &TerrainParams) -> TerrainField {
    let cells = params.cells.max(2);
    let cell_size = params.span / (cells - 1) as f32;
    let origin = -params.span * 0.5;
    // Frequency is per WORLD UNIT, so `cells` changes how finely the same
    // landscape is sampled and nothing else.
    let freq = 1.0 / params.feature_size.max(0.001);

    // Pass one: the raw field. Normalising against its OWN extents rather
    // than a theoretical range is what makes `amp` mean literal peak-to-trough
    // relief at any octave count. Dividing by the sum of octave amplitudes
    // instead — the textbook form — quietly shrinks the range as octaves are
    // added, because summing decorrelated fields concentrates them about the
    // mean. Five octaves then come out FLATTER than one, which is the opposite
    // of why anyone adds octaves.
    let mut raw = Vec::with_capacity(cells * cells);
    for gz in 0..cells {
        for gx in 0..cells {
            let wx = origin + gx as f32 * cell_size;
            let wz = origin + gz as f32 * cell_size;
            let (nx, nz) = (wx * freq, wz * freq);

            // Domain warp: offset the sample point by a second, coarser noise
            // field. Two different constant offsets give two decorrelated
            // fields out of one function.
            let (sx, sz) = if params.warp != 0.0 {
                let wxo = value_noise(params.seed ^ 0xA5A5_1234, nx + 5.2, nz + 1.3) - 0.5;
                let wzo = value_noise(params.seed ^ 0x5A5A_4321, nx + 9.7, nz + 7.1) - 0.5;
                (nx + wxo * params.warp * 2.0, nz + wzo * params.warp * 2.0)
            } else {
                (nx, nz)
            };
            raw.push(fbm(params, sx, sz));
        }
    }
    let (raw_lo, raw_hi) = raw
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let raw_span = (raw_hi - raw_lo).max(1e-6);

    let mut heights = Vec::with_capacity(cells * cells);
    for gz in 0..cells {
        for gx in 0..cells {
            let wx = origin + gx as f32 * cell_size;
            let wz = origin + gz as f32 * cell_size;

            let mut h = (raw[gz * cells + gx] - raw_lo) / raw_span;
            if params.flatten != 1.0 {
                h = makepad_game_math::pow(h.clamp(0.0, 1.0), params.flatten.max(0.01));
            }

            // Relief grows toward the edge. Deliberately NOT clamped at the
            // half-span: clamping puts every corner at the same value, which
            // is a flat high plateau ringing the map — the same artifact in a
            // different costume.
            let mut local_amp = params.amp;
            if params.rim_relief != 0.0 {
                let half = params.span * 0.5;
                let d = (wx * wx + wz * wz).sqrt() / half.max(0.001);
                let start = params.rim_start.clamp(0.0, 0.999);
                if d > start {
                    let t = ((d - start) / (1.0 - start)).max(0.0);
                    local_amp *= 1.0 + params.rim_relief * smoothstep01(t.min(1.0)) * t.max(1.0);
                }
            }
            let mut top = params.base + h * local_amp;


            if let Some(p) = params.plaza {
                let dx = wx - p.center_x;
                let dz = wz - p.center_z;
                let d = (dx * dx + dz * dz).sqrt();
                if d < p.radius {
                    top = p.height;
                } else if d < p.radius + p.ramp {
                    // Smoothstepped, not linear: a linear ramp leaves a
                    // visible crease where it meets the natural ground.
                    let t = smoothstep01((d - p.radius) / p.ramp.max(0.001));
                    top = p.height + (top - p.height) * t;
                }
            }
            if params.terrace > 0.0 {
                top = (top / params.terrace).floor() * params.terrace;
            }
            heights.push(top.clamp(params.min_height, params.max_height));
        }
    }

    let (min_height, max_height) = heights
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), h| (lo.min(*h), hi.max(*h)));

    let colors = shade(params, cells, cell_size, origin, &heights, min_height, max_height);

    TerrainField {
        cells,
        cell_size,
        origin,
        heights,
        colors,
        min_height,
        max_height,
    }
}

/// Per-vertex colour from height, slope and a little noise.
fn shade(
    params: &TerrainParams,
    cells: usize,
    cell_size: f32,
    origin: f32,
    heights: &[f32],
    min_height: f32,
    max_height: f32,
) -> Vec<Vec4f> {
    let pal = params.palette;
    let span = (max_height - min_height).max(0.0001);
    let mut colors = Vec::with_capacity(cells * cells);
    for gz in 0..cells {
        for gx in 0..cells {
            let h = heights[gz * cells + gx];

            // Central differences, clamped at the border. Slope is what
            // separates a cliff from a hill of the same height.
            let at = |x: usize, z: usize| heights[z.min(cells - 1) * cells + x.min(cells - 1)];
            let xm = at(gx.saturating_sub(1), gz);
            let xp = at(gx + 1, gz);
            let zm = at(gx, gz.saturating_sub(1));
            let zp = at(gx, gz + 1);
            let run_x = if gx == 0 || gx + 1 >= cells { cell_size } else { cell_size * 2.0 };
            let run_z = if gz == 0 || gz + 1 >= cells { cell_size } else { cell_size * 2.0 };
            let dx = (xp - xm) / run_x;
            let dz = (zp - zm) / run_z;
            let slope = (dx * dx + dz * dz).sqrt();

            let t = ((h - min_height) / span).clamp(0.0, 1.0);
            let mut color = lerp4(pal.low, pal.high, smoothstep01(t));

            // Shoreline, if there is water at all.
            if pal.water_level > f32::MIN && h < pal.water_level + pal.sand_band {
                let d = (h - pal.water_level).max(0.0) / pal.sand_band.max(0.001);
                color = lerp4(pal.sand, color, smoothstep01(d.clamp(0.0, 1.0)));
            }

            // Rock over the top: fades in from 60% of the cliff slope so the
            // transition has width instead of being a hard outline.
            let lo = pal.rock_slope * 0.6;
            let steep = ((slope - lo) / (pal.rock_slope - lo).max(0.001)).clamp(0.0, 1.0);
            color = lerp4(color, pal.rock, smoothstep01(steep));

            if pal.mottle > 0.0 {
                let wx = origin + gx as f32 * cell_size;
                let wz = origin + gz as f32 * cell_size;
                // Fine, seed-independent-of-the-height-field jitter so the
                // mottling does not simply restate the contours.
                let n = value_noise(params.seed ^ 0xDEAD_BEEF, wx * 0.35, wz * 0.35) - 0.5;
                let k = 1.0 + n * pal.mottle * 2.0;
                color = vec4(
                    (color.x * k).clamp(0.0, 1.0),
                    (color.y * k).clamp(0.0, 1.0),
                    (color.z * k).clamp(0.0, 1.0),
                    color.w,
                );
            }
            colors.push(color);
        }
    }
    colors
}

#[inline]
fn lerp4(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.z + (b.z - a.z) * t,
        a.w + (b.w - a.w) * t,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> TerrainParams {
        TerrainParams {
            seed: 0xC0FFEE,
            span: 128.0,
            cells: 129,
            ..Default::default()
        }
    }

    #[test]
    fn the_same_seed_makes_the_same_landscape_every_time() {
        let a = generate(&params());
        let b = generate(&params());
        assert_eq!(a.heights, b.heights, "terrain must be bit-identical per seed");
        assert_eq!(a.colors.len(), b.colors.len());
    }

    #[test]
    fn a_different_seed_makes_a_different_landscape() {
        let a = generate(&params());
        let mut p = params();
        p.seed = 0xBADF00D;
        let b = generate(&p);
        assert_ne!(a.heights, b.heights);
    }

    /// The fault that made resolution and shape the same knob: the old
    /// generator indexed noise by cell index, so asking for a finer mesh
    /// silently produced a different world. Sampling the SAME world positions
    /// at two resolutions must agree to within the interpolation error.
    #[test]
    fn raising_the_resolution_refines_the_landscape_rather_than_replacing_it() {
        let mut coarse = params();
        coarse.cells = 65;
        let mut fine = params();
        fine.cells = 257;
        let a = generate(&coarse);
        let b = generate(&fine);

        let mut worst = 0.0f32;
        for i in 0..20 {
            for j in 0..20 {
                let x = -50.0 + i as f32 * 5.0;
                let z = -50.0 + j as f32 * 5.0;
                let (ha, hb) = (a.height_at(x, z).unwrap(), b.height_at(x, z).unwrap());
                worst = worst.max((ha - hb).abs());
            }
        }
        let relief = a.max_height - a.min_height;
        assert!(
            worst < relief * 0.25,
            "same world sampled at two resolutions differs by {worst:.2} of {relief:.2} relief \
             — resolution is changing the landscape, not the detail"
        );
    }

    /// Single-octave noise has energy at exactly one scale, so a slope has no
    /// texture on it: zoom in and it is a plane.
    ///
    /// Measured as CURVATURE, not slope. Slope is the intuitive choice and it
    /// is the wrong one — at the default `gain: 0.5` / `lacunarity: 2.0` every
    /// octave contributes equally to slope (that is what makes fBm
    /// self-similar), so once total relief is normalised, adding octaves
    /// redistributes roughness rather than adding any, and a slope-based test
    /// reports no difference while the terrain visibly gains detail. The
    /// second difference is what actually answers "is there structure at the
    /// scale I am sampling", which is the thing octaves are for.
    #[test]
    fn octaves_add_detail_that_one_octave_does_not_have() {
        let mut one = params();
        one.octaves = 1;
        one.warp = 0.0;
        let mut many = params();
        many.octaves = 5;
        many.warp = 0.0;

        let curvature = |f: &TerrainField| {
            let mut total = 0.0f32;
            let mut n = 0u32;
            for gz in 0..f.cells {
                for gx in 1..f.cells - 1 {
                    let d2 = f.height(gx + 1, gz) - 2.0 * f.height(gx, gz) + f.height(gx - 1, gz);
                    total += d2.abs();
                    n += 1;
                }
            }
            total / n as f32
        };
        let c1 = curvature(&generate(&one));
        let c5 = curvature(&generate(&many));
        assert!(
            c5 > c1 * 3.0,
            "five octaves have barely more fine structure than one ({c5:.4} vs {c1:.4}) — the \
             octave loop is not contributing"
        );
    }

    /// Relief must be what the caller asked for regardless of octave count.
    /// The textbook normalisation (divide by the sum of octave amplitudes)
    /// fails this: it makes `amp` an upper bound that more octaves quietly
    /// walk away from, so tuning octaves silently re-tunes the height too.
    #[test]
    fn amp_means_relief_at_any_octave_count() {
        for octaves in [1u32, 3, 5, 8] {
            let mut p = params();
            p.octaves = octaves;
            p.flatten = 1.0;
            p.amp = 12.0;
            let f = generate(&p);
            let relief = f.max_height - f.min_height;
            assert!(
                (relief - 12.0).abs() < 0.01,
                "{octaves} octaves gave {relief:.2} of relief for amp 12.0"
            );
        }
    }

    /// The point of `rim_relief`: a playable middle and a scenic edge.
    #[test]
    fn rim_relief_makes_the_edge_wilder_than_the_middle() {
        let mut p = params();
        p.rim_relief = 4.0;
        p.rim_start = 0.4;
        let f = generate(&p);
        // Relief measured over the central quarter vs the outer band.
        let band = |lo: f32, hi: f32| {
            let half = p.span * 0.5;
            let (mut min, mut max) = (f32::MAX, f32::MIN);
            for gz in 0..f.cells {
                for gx in 0..f.cells {
                    let wx = f.origin + gx as f32 * f.cell_size;
                    let wz = f.origin + gz as f32 * f.cell_size;
                    let d = (wx * wx + wz * wz).sqrt() / half;
                    if d >= lo && d < hi {
                        let h = f.height(gx, gz);
                        min = min.min(h);
                        max = max.max(h);
                    }
                }
            }
            max - min
        };
        let middle = band(0.0, 0.3);
        let edge = band(0.75, 1.0);
        assert!(
            edge > middle * 2.5,
            "edge relief {edge:.2} is not meaningfully wilder than the middle's {middle:.2}"
        );
    }

    /// Clamping the radial term at the half-span puts every corner at the same
    /// value — a flat high plateau ringing the map, which is the ring artifact
    /// wearing a different hat. The corners must still be landscape.
    #[test]
    fn the_corners_are_not_a_flat_plateau() {
        let mut p = params();
        p.rim_relief = 4.0;
        p.rim_start = 0.4;
        let f = generate(&p);
        let half = p.span * 0.5;
        let (mut min, mut max) = (f32::MAX, f32::MIN);
        for gz in 0..f.cells {
            for gx in 0..f.cells {
                let wx = f.origin + gx as f32 * f.cell_size;
                let wz = f.origin + gz as f32 * f.cell_size;
                if (wx * wx + wz * wz).sqrt() / half > 1.05 {
                    let h = f.height(gx, gz);
                    min = min.min(h);
                    max = max.max(h);
                }
            }
        }
        let relief = max - min;
        assert!(
            relief > (f.max_height - f.min_height) * 0.2,
            "beyond the inscribed circle the ground only varies by {relief:.2} — the radial term              is saturating into a plateau"
        );
    }

    #[test]
    fn a_plaza_is_actually_flat_and_blends_out() {
        let mut p = params();
        p.plaza = Some(TerrainPlaza {
            center_x: 0.0,
            center_z: 0.0,
            radius: 20.0,
            ramp: 12.0,
            height: 2.0,
        });
        let f = generate(&p);
        // Dead flat inside the radius — this is where a town gets built.
        for i in 0..12 {
            for j in 0..12 {
                let x = -14.0 + i as f32 * 2.5;
                let z = -14.0 + j as f32 * 2.5;
                if (x * x + z * z).sqrt() < 18.0 {
                    let h = f.height_at(x, z).unwrap();
                    assert!(
                        (h - 2.0).abs() < 0.01,
                        "plaza is not flat at ({x}, {z}): {h}"
                    );
                }
            }
        }
        // And well outside it, the landscape is back.
        let outside = f.height_at(50.0, 50.0).unwrap();
        assert!((outside - 2.0).abs() > 0.01, "plaza swallowed the whole map");
    }

    /// `flatten` exists so a world has somewhere to put a town. If it does not
    /// actually move the mass of the landscape downward it is decoration.
    #[test]
    fn flatten_pushes_the_landscape_toward_its_floor() {
        let mut linear = params();
        linear.flatten = 1.0;
        let mut flattened = params();
        flattened.flatten = 2.5;
        let mean = |f: &TerrainField| f.heights.iter().sum::<f32>() / f.heights.len() as f32;
        assert!(
            mean(&generate(&flattened)) < mean(&generate(&linear)),
            "flatten did not lower the average ground level"
        );
    }

    /// Noise that mirrors about the origin is the signature of truncating
    /// toward zero instead of flooring. It is invisible in a heightmap thumbnail
    /// and obvious once you stand on the seam.
    #[test]
    fn the_landscape_is_not_mirrored_about_the_origin() {
        let f = generate(&params());
        let mut matches = 0;
        let mut total = 0;
        for i in 1..40 {
            let d = i as f32 * 1.5;
            let (a, b) = (f.height_at(d, 3.0).unwrap(), f.height_at(-d, 3.0).unwrap());
            if (a - b).abs() < 0.001 {
                matches += 1;
            }
            total += 1;
        }
        assert!(
            matches * 4 < total,
            "{matches}/{total} sample pairs mirror across x=0 — lattice floor is truncating"
        );
    }

    #[test]
    fn steep_ground_is_coloured_as_rock_and_flat_ground_is_not() {
        let mut p = params();
        p.amp = 40.0; // guarantee some genuinely steep faces
        p.flatten = 1.0;
        let f = generate(&p);
        let pal = p.palette;
        let rockiness = |c: Vec4f| {
            // Distance from grass toward rock, along the axis that separates
            // them: rock is grey, grass is green.
            (c.x - pal.low.x).abs() + (pal.low.y - c.y).abs()
        };
        let mut steepest = (0.0f32, 0usize);
        let mut flattest = (f32::MAX, 0usize);
        for gz in 1..f.cells - 1 {
            for gx in 1..f.cells - 1 {
                let dx = (f.height(gx + 1, gz) - f.height(gx - 1, gz)) / (2.0 * f.cell_size);
                let dz = (f.height(gx, gz + 1) - f.height(gx, gz - 1)) / (2.0 * f.cell_size);
                let slope = (dx * dx + dz * dz).sqrt();
                let idx = gz * f.cells + gx;
                if slope > steepest.0 {
                    steepest = (slope, idx);
                }
                if slope < flattest.0 {
                    flattest = (slope, idx);
                }
            }
        }
        assert!(
            rockiness(f.colors[steepest.1]) > rockiness(f.colors[flattest.1]),
            "the steepest face is not more rock-coloured than the flattest"
        );
    }
}

/// Offline preview: writes a hill-shaded PPM of a generated field.
///
/// Not a test — a way to LOOK at the generator. Terrain is judged by eye and
/// numeric assertions cannot tell "rolling hills" from "melted blobs", so the
/// alternative to this is tuning noise parameters by adjective.
///
/// `cargo test -p makepad-game-gen preview_terrain -- --ignored --nocapture`
#[cfg(test)]
pub fn write_preview_ppm(field: &TerrainField, path: &str) -> std::io::Result<()> {
    use std::io::Write;
    let n = field.cells;
    let mut out = Vec::with_capacity(n * n * 3 + 32);
    write!(out, "P6\n{n} {n}\n255\n")?;
    // Sun from the north-west, the cartographic convention — relief read under
    // light from any other direction inverts for most people.
    let sun = vec3f(-0.5, 0.75, -0.43).normalize();
    for gz in 0..n {
        for gx in 0..n {
            let h = |x: usize, z: usize| field.height(x.min(n - 1), z.min(n - 1));
            let dx = (h(gx + 1, gz) - h(gx.saturating_sub(1), gz)) / (2.0 * field.cell_size);
            let dz = (h(gx, gz + 1) - h(gx, gz.saturating_sub(1))) / (2.0 * field.cell_size);
            let normal = vec3f(-dx, 1.0, -dz).normalize();
            let lambert = Vec3f::dot(&normal, sun).max(0.0);
            let shade = 0.35 + 0.65 * lambert;
            let c = field.colors[gz * n + gx];
            out.push((c.x * shade * 255.0).clamp(0.0, 255.0) as u8);
            out.push((c.y * shade * 255.0).clamp(0.0, 255.0) as u8);
            out.push((c.z * shade * 255.0).clamp(0.0, 255.0) as u8);
        }
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod preview {
    use super::*;

    #[test]
    #[ignore = "writes a preview image; run explicitly"]
    fn preview_terrain() {
        let dir = std::env::var("TERRAIN_PREVIEW_DIR").unwrap_or_else(|_| "/tmp".into());
        let variants: [(&str, TerrainParams); 5] = [
            (
                "old-single-octave",
                TerrainParams {
                    seed: 0xC0FFEE,
                    span: 128.0,
                    cells: 257,
                    octaves: 1,
                    warp: 0.0,
                    flatten: 1.0,
                    terrace: 1.0,
                    ..Default::default()
                },
            ),
            (
                "new-default",
                TerrainParams {
                    seed: 0xC0FFEE,
                    span: 128.0,
                    cells: 257,
                    ..Default::default()
                },
            ),
            (
                "new-with-plaza",
                TerrainParams {
                    seed: 0xC0FFEE,
                    span: 128.0,
                    cells: 257,
                    plaza: Some(TerrainPlaza {
                        center_x: 0.0,
                        center_z: 0.0,
                        radius: 22.0,
                        ramp: 16.0,
                        height: 0.0,
                    }),
                    ..Default::default()
                },
            ),
            (
                "basin-with-rim",
                TerrainParams {
                    seed: 0xC0FFEE,
                    span: 128.0,
                    cells: 257,
                    amp: 7.0,
                    rim_relief: 4.0,
                    rim_start: 0.40,
                    plaza: Some(TerrainPlaza {
                        center_x: 0.0,
                        center_z: 0.0,
                        radius: 20.0,
                        ramp: 18.0,
                        height: 0.0,
                    }),
                    ..Default::default()
                },
            ),
            (
                "ridged-mountains",
                TerrainParams {
                    seed: 0xC0FFEE,
                    span: 128.0,
                    cells: 257,
                    amp: 26.0,
                    ridged: 0.85,
                    flatten: 1.0,
                    ..Default::default()
                },
            ),
        ];
        for (name, p) in &variants {
            let f = generate(p);
            let path = format!("{dir}/terrain-{name}.ppm");
            write_preview_ppm(&f, &path).unwrap();
            println!(
                "{name}: relief {:.2} ({:.2}..{:.2}) -> {path}",
                f.max_height - f.min_height,
                f.min_height,
                f.max_height
            );
        }
    }
}
