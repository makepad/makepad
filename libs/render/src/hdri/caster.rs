//! Shadow casters (redesign §5): a layer of the list that adds no light and
//! multiplies what is drawn before it by its transmission,
//!
//!     acc * (1 - s*m + s*m*tint)
//!
//! where m (0..=1) is its pattern's coverage at that direction (after
//! softness, here, and after bloom, R5) and s its strength. What is drawn
//! before it is everything listed below it: the base layer (the sky with its
//! sun, moon, stars, clouds and weather, or the studio backdrop), a base
//! image, and the lights listed before it. The entries listed after it draw
//! over it untouched. R5: a caster's bloom (CasterBloom) thins m where
//! bright light shows behind it.
//!
//! A caster is laid out as a Multiply flag of its own box: its frame, width,
//! height and roll, with its `gobo` block as its pattern (`gobo: None` covers
//! the whole box), as [`caster_flag`] builds it. Without softness (and without
//! bloom) it IS that flag, exactly, so the studio draws it through the flag's
//! own code and the key follows it by the flags' rules (D1). With softness its
//! coverage is box-filtered in its tangent plane. The pattern is rastered once
//! per `Env` and read through a summed-area table (`image_light::SummedArea`,
//! as image gobos and softened picture masks are), so the blur moves coverage
//! about and loses none of the light the caster lets through.
//!
//! The light-only fields an entry keeps are never read for a caster: its
//! intensity, colour, source, gels, picture, blend, aim and surface; a
//! polygon's, stroke's or scrim's block; and its gobo's own distance,
//! fill_sky and project_on_backdrop. The studio sees only the flag.

use super::*;
use super::gobo::Gobo;
use super::image_light::SummedArea;
use super::light_fx::face_axes;
use super::noise::hash01;
use super::studio::{project, sd_round_rect, LightFrame, MIN_EDGE, PROJECT_MIN_COS};
use crate::sky::luminance;
use std::sync::Arc;

/// The blur half-width at softness 1, as a share of the caster's shorter
/// half extent (tangent units): softness 1 blurs a 60 x 45 degree caster
/// over about 6 degrees each way.
pub const CASTER_SOFT_SCALE: f32 = 0.25;
/// The most cells a caster raster (softness, and R5's bloom) has along a side.
pub const CASTER_RASTER_MAX_SIDE: usize = 512;
/// Cells per blur half-width a softness raster is laid out at: the blur's
/// box then spans 8 x 8 of them, each the mean of SOFT_SUBSAMPLES² samples.
const SOFT_CELLS_PER_RADIUS: f32 = 4.0;
/// Samples per axis of one raster cell: a cell is the pattern's mean over
/// it, so a leaf a cell wide still covers its own share.
const SOFT_SUBSAMPLES: usize = 2;
/// The most pattern samples one softness raster takes. The raster is built
/// on the bake job's thread once per Env, so a held drag pays it on every
/// bake. A slight blur on a large caster would ask for far more; it then gets
/// coarser cells (about two thirds of a degree on a 90 x 60 degree caster at
/// softness 0.05, whose blur is 0.4 degrees), so it reads a little softer
/// than asked. `hdri_perf`'s `a_soft_casters_raster_stays_well_under_a_held_bake`
/// measures the cost.
const SOFT_SAMPLE_BUDGET: usize = 1 << 16;
/// `cell_value`'s jitter seed: fixed, so the same caster always rasters the same.
const SOFT_JITTER_SEED: u32 = 0x6361_7374;

/// The flag a caster draws as without softness and bloom: shape rect, corner
/// 0, softness 0, hotspot 0, blend multiply, intensity_ev 0, rgb
/// Some((1 - s) + s * tint) per channel, and the caster's gobo with distance
/// 0, fill_sky false and project_on_backdrop false (None: none, a solid flag).
/// Nothing else: no gels, source, picture, aim, wear, polygon, stroke or
/// scrim; key false, kind None, caster None. Read from `light.clamped()`.
/// The core tests' oracle: a plain caster draws exactly as this flag.
pub fn caster_flag(light: &LightParams) -> LightParams {
    let held = light.clamped();
    let settings = held.caster_params();
    let s = settings.strength;
    // What a full coverage leaves of what is behind, per channel.
    let pass = |tint: f32| (1.0 - s) + s * tint;
    LightParams {
        enabled: held.enabled,
        name: held.name.clone(),
        shape: "rect".to_string(),
        corner: 0.0,
        azimuth_deg: held.azimuth_deg,
        elevation_deg: held.elevation_deg,
        width_deg: held.width_deg,
        height_deg: held.height_deg,
        roll_deg: held.roll_deg,
        softness: 0.0,
        hotspot: 0.0,
        intensity_ev: 0.0,
        blend: "multiply".to_string(),
        key: false,
        rgb: Some([pass(settings.tint[0]), pass(settings.tint[1]), pass(settings.tint[2])]),
        gobo: held.gobo.as_ref().map(|g| GoboParams { distance: 0.0, fill_sky: false, project_on_backdrop: false, ..g.clone() }),
        ..LightParams::default()
    }
}

/// Whether a caster draws: false when its pattern is an image mask whose
/// picture is not in `images` (or that names none). Such a caster draws as no
/// caster at all and dims no key (spec §6), where a flag with such a gobo
/// draws solid. True for every other caster; `enabled` is not looked at. The
/// inspector reads it to say why a caster draws nothing (R6).
pub fn caster_drawn(light: &LightParams, images: &ImageSet) -> bool {
    match &light.gobo {
        Some(gobo) if gobo.kind() == GoboKind::Image => {
            // As `Gobo::new` looks it up: the name as clamp() holds it.
            let gobo = gobo.clamped();
            !gobo.image.is_empty() && images.contains(&gobo.image)
        }
        _ => true,
    }
}

/// A caster as the studio draws it (`studio::draw_caster`).
#[derive(Clone, Debug)]
pub(crate) struct CasterLayer {
    /// 0..=1: how much of what is behind it a full coverage takes out.
    strength: f32,
    /// What a full coverage lets through, per channel (linear, 0..=1).
    tint: Vec3f,
    /// The flag's frame, and the sign that turns its `right` into the
    /// viewer's right (`light_fx::face_axes`).
    frame: LightFrame,
    sx: f32,
    /// The blurred coverage when softness is above 0; None draws the
    /// flag's own sharp coverage.
    soft: Option<Arc<SoftCoverage>>,
    /// The flag's tangent half extents (`face_axes`), which the bloom raster
    /// spans (R5).
    half: Vec2f,
    /// The caster block as clamp() holds it (`LightParams::caster_params`): its bloom's
    /// strength, threshold and length, and the softness that widens what the bloom raster
    /// must cover (R5).
    params: CasterParams,
    /// The bloom (R5), set once by `Env`'s pre-pass when something bright stands behind the
    /// caster. None draws it without bloom (and, with no softness raster, exactly as its
    /// flag).
    bloom: Option<Arc<CasterBloom>>,
}

impl CasterLayer {
    /// None when `caster_drawn` is false. Builds the softness raster when
    /// softness is above 0 (counted as `probe::Work::CasterSoft`), on
    /// `SummedArea<1>`: once per `Env`, whatever width it is baked at.
    pub(crate) fn new(light: &LightParams, images: &ImageSet) -> Option<CasterLayer> {
        if !caster_drawn(light, images) {
            return None;
        }
        let flag = caster_flag(light);
        let settings = light.caster_params();
        // The flag's own box: the studio draws it in this frame and half extents.
        let (frame, sx, half) = face_axes(&flag);
        let radius = settings.softness * CASTER_SOFT_SCALE * half.x.min(half.y);
        let soft = (radius > 0.0).then(|| Arc::new(SoftCoverage::build(&Pattern::new(&flag, half, images), half, radius)));
        let t = settings.tint;
        Some(CasterLayer { strength: settings.strength, tint: vec3f(t[0], t[1], t[2]), frame, sx, soft, half, params: light.caster_params(), bloom: None })
    }

    /// No raster at all (softness 0, and R5: no bloom): drawn exactly as its flag.
    pub(crate) fn is_plain(&self) -> bool {
        self.soft.is_none() && self.bloom.is_none()
    }

    /// m at `dir` (map frame) before bloom, 0..=1: the softness raster's box
    /// filter where there is one, else `sharp(dir)`, the analytic coverage of
    /// the flag (`studio::caster_sharp_coverage`). A NaN covers nothing.
    fn unbloomed(&self, dir: Vec3f, sharp: &dyn Fn(Vec3f) -> f32) -> f32 {
        let m = match &self.soft {
            None => sharp(dir),
            Some(soft) => match project(&self.frame, dir) {
                Some(t) => soft.at(vec2f(self.sx * t.x, t.y)),
                None => 0.0,
            },
        };
        // max/min, not clamp: a NaN becomes 0.
        m.max(0.0).min(1.0)
    }

    /// `acc * (1 + s * (tint - 1) * m)`: what is behind the caster where it
    /// covers `m` of it. Never above `acc` (tint and m are 0..=1); m 0 leaves
    /// `acc` exactly.
    pub(crate) fn transmit(&self, acc: Vec3f, m: f32) -> Vec3f {
        if !(m > 0.0) {
            return acc;
        }
        acc * (Vec3f::all(1.0) + (self.tint - Vec3f::all(1.0)) * (self.strength * m))
    }

    /// m' at `dir` (map frame), 0..=1: the coverage (`unbloomed`: the analytic pattern
    /// `sharp`, or its softness raster) thinned by the bloom, m * (1 - bloom_strength * g).
    /// Never above m, so a caster with bloom still adds no light.
    pub(crate) fn coverage(&self, dir: Vec3f, sharp: &dyn Fn(Vec3f) -> f32) -> f32 {
        let m = self.unbloomed(dir, sharp);
        match &self.bloom {
            Some(bloom) if m > 0.0 => m * (1.0 - self.params.bloom_strength * bloom.share(dir)),
            _ => m,
        }
    }

    /// Whether the caster asks for bloom: some strength over some length (decision 11).
    pub(crate) fn wants_bloom(&self) -> bool {
        self.params.bloom_strength > 0.0 && self.params.bloom_length_deg > 0.0
    }

    /// Hands the caster its bloom (`Studio::set_caster_bloom`): it is no longer plain.
    pub(crate) fn set_bloom(&mut self, bloom: CasterBloom) {
        self.bloom = Some(Arc::new(bloom));
    }

    /// The bloom the pre-pass kept, for the tests.
    #[cfg(test)]
    pub(crate) fn bloom(&self) -> Option<&CasterBloom> {
        self.bloom.as_deref()
    }
}

/// A caster's sharp coverage in its face coordinates (x to the viewer's
/// right, y up, tangent units; `LightFx::face`): its flag's box, with the
/// flag's hard edge, times what its pattern blocks. It is the coverage the
/// studio draws the flag with (`studio::caster_sharp_coverage`); here it is rastered.
struct Pattern {
    half: Vec2f,
    gobo: Option<Gobo>,
}

impl Pattern {
    /// `flag` is `caster_flag`'s, `half` its tangent half extents.
    fn new(flag: &LightParams, half: Vec2f, images: &ImageSet) -> Pattern {
        Pattern { half, gobo: flag.gobo.as_ref().and_then(|g| Gobo::new(g, half, images)) }
    }

    /// Coverage 0..=1 at face point `q`.
    fn at(&self, q: Vec2f) -> f32 {
        // As `PreparedLight::mask` draws a rect with no corner, softness or
        // hotspot: its narrowest edge (symmetric, so the face's x sign does not matter).
        let edge = 1.0 - smoothstep(-0.5 * MIN_EDGE, 0.5 * MIN_EDGE, sd_round_rect(q, self.half, 0.0));
        if !(edge > 0.0) {
            return 0.0;
        }
        match &self.gobo {
            Some(gobo) => edge * (1.0 - gobo.transmission(q)),
            None => edge,
        }
    }
}

/// A soft caster's coverage: the pattern's mean over each cell of a grid
/// laid over its box and the blur's reach past it, with a ring of empty
/// cells round that (the table holds its edge cells past its border, so they
/// must hold nothing), box-filtered through a summed-area table. Built once
/// per `Env` (decision 10): every bake, the key and the keyless copy read the
/// same one, independent of the width baked.
#[derive(Debug)]
struct SoftCoverage {
    table: SummedArea<1>,
    /// The raster's half extents, face coordinates: it spans -ext..ext, row 0 at the top.
    ext: Vec2f,
    /// One cell's width and height, face coordinates.
    cell: Vec2f,
    /// The blur's half-width, tangent units.
    radius: f32,
}

/// The mean of `pattern` over cell (i, j) (column i from the left, row j
/// from the top) of a raster with half extents `ext` and cells `cell`:
/// SOFT_SUBSAMPLES² samples, one at a fixed jitter in each of its sub-cells.
/// At the sub-cells' centres a window's muntins, hard bars thinner than a
/// sub-cell that repeat with the grid's own symmetry, aliased onto it: a 60 x
/// 45 degree window caster at softness 1 covered 16 % more than sharp, at
/// 0.6 5 % less. Jittered, every pattern stays within 1 %.
fn cell_value(pattern: &Pattern, ext: Vec2f, cell: Vec2f, i: usize, j: usize) -> f32 {
    let n = SOFT_SUBSAMPLES;
    let (left, top) = (-ext.x + i as f32 * cell.x, ext.y - j as f32 * cell.y);
    let mut sum = 0.0;
    for b in 0..n {
        for a in 0..n {
            // One jittered sample in each sub-cell, the same for every Env.
            let (sa, sb) = ((i * n + a) as i32, (j * n + b) as i32);
            let (ja, jb) = (hash01(sa, sb, 0, SOFT_JITTER_SEED), hash01(sa, sb, 1, SOFT_JITTER_SEED));
            let q = vec2f(left + (a as f32 + ja) / n as f32 * cell.x, top - (b as f32 + jb) / n as f32 * cell.y);
            sum += pattern.at(q);
        }
    }
    sum / (n * n) as f32
}

impl SoftCoverage {
    /// The raster of `pattern` over a caster with tangent half extents
    /// `half`, blurred `radius` each way.
    fn build(pattern: &Pattern, half: Vec2f, radius: f32) -> SoftCoverage {
        #[cfg(test)]
        super::probe::count(super::probe::Work::CasterSoft);
        // Along each axis: the box and the blur's reach on both sides in
        // cells of radius / SOFT_CELLS_PER_RADIUS, coarser when the samples
        // would pass SOFT_SAMPLE_BUDGET, then an empty cell on each side.
        let span = vec2f(2.0 * (half.x + radius), 2.0 * (half.y + radius));
        let target = radius / SOFT_CELLS_PER_RADIUS;
        let samples = (span.x / target).ceil().max(2.0) * (span.y / target).ceil().max(2.0) * (SOFT_SUBSAMPLES * SOFT_SUBSAMPLES) as f32;
        let coarsen = (samples / SOFT_SAMPLE_BUDGET as f32).sqrt().max(1.0);
        let inner = |span: f32| ((span / (target * coarsen)).ceil() as usize).clamp(2, CASTER_RASTER_MAX_SIDE - 2);
        let (nx, ny) = (inner(span.x), inner(span.y));
        let cell = vec2f(span.x / nx as f32, span.y / ny as f32);
        let ext = vec2f(half.x + radius + cell.x, half.y + radius + cell.y);
        let (w, h) = (nx + 2, ny + 2);
        let mut data = vec![[0.0f32; 4]; w * h];
        for j in 1..h - 1 {
            for i in 1..w - 1 {
                data[j * w + i][0] = cell_value(pattern, ext, cell, i, j);
            }
        }
        let raster = EnvMap { width: w, height: h, data };
        SoftCoverage { table: SummedArea::build(&raster, |t| [t[0] as f64]), ext, cell, radius }
    }

    /// Where the blur's box round face point `q` lies in the table: its
    /// centre and half-widths, in cells.
    fn texel_box(&self, q: Vec2f) -> (f64, f64, f64, f64) {
        (
            ((q.x + self.ext.x) / self.cell.x) as f64,
            ((self.ext.y - q.y) / self.cell.y) as f64,
            (self.radius / self.cell.x) as f64,
            (self.radius / self.cell.y) as f64,
        )
    }

    /// The blurred coverage at face point `q`, 0..=1. Past the raster it is
    /// 0: the blur reaches nothing there.
    fn at(&self, q: Vec2f) -> f32 {
        if !(q.x.abs() < self.ext.x && q.y.abs() < self.ext.y) {
            return 0.0;
        }
        let (x, y, rx, ry) = self.texel_box(q);
        self.table.box_mean(x, y, rx, ry)[0] as f32
    }
}

/// The bright light, in EV above the threshold times tangent-plane area (about steradians
/// near a caster's centre), that thins a caster by 1 - 1/e at the peak of its bloom: a
/// half-degree disc (6.0e-5) 2.5 EV over its threshold. The default sun (about 2^18 against
/// the default 2^10, 0.53 degrees across: 8 EV over 6.7e-5, 5.4e-4) thins the leaves next to
/// it by about 97 % at bloom 1.
const BLOOM_REF_ENERGY: f64 = 1.5e-4;
/// A bloom raster's cell is never finer than a 32nd of its bloom length: the blur that
/// follows spans that many cells either way.
const BLOOM_CELLS_PER_LENGTH: f32 = 32.0;
/// A map `width` wide has a bloom raster of at most width / 12 cells a side (42 at the held
/// 512 preview, 85 at the released 1024 one, 341 at 4K). Its pre-pass is one thread's work
/// beside a row-parallel bake, so this keeps it a small share of the bake (hdri_perf's
/// `a_bloom_casters_pre_pass_stays_well_under_its_bake`).
const BLOOM_WIDTH_PER_SIDE: usize = 12;
/// The fewest cells a side, for the smallest maps.
const BLOOM_MIN_SIDE: usize = 16;
/// Samples per disc radius along each axis of a cell a disc can touch (the sun, the moon, a
/// window's sun, an image's sun), so a disc smaller than a cell is area-averaged into it and
/// neither pops nor vanishes as it moves; from 2 to BLOOM_DISC_SAMPLES_MAX a side.
const BLOOM_DISC_SAMPLES_PER_RADIUS: f32 = 6.0;
/// 12, not 24: on a volumetric sky each sample marches the clouds, and 24 a side over the
/// sun's cells took the held 512 pre-pass to 64 % of its bake (hdri_perf); 12 keeps a small
/// sun's bloom within 5 % as it moves (`a_small_sun_behind_leaves_blooms_smoothly_as_it_moves`).
const BLOOM_DISC_SAMPLES_MAX: usize = 12;
/// A share below this is none: far from any bright light, the summed-area tables' rounding
/// leaves exactly 0.
const BLOOM_FLUSH: f64 = 1.0e-6;

/// Where a bright disc touches a bloom raster (`CasterBloom::build`).
enum Touch {
    /// Its centre in the caster's tangent plane, and how far from it a cell's centre may lie
    /// along either axis for the cell to touch it.
    Plane(Vec2f, f32),
    /// Its centre (map frame) and the cosine within which a cell's centre may lie.
    Angle(Vec3f, f32),
}

/// One caster's bloom (redesign §5): the share g (0..=1) of bright light behind it, blurred
/// over its bloom length. It lives on a raster in the caster's own tangent plane, covering
/// its box, its soft edge, the bloom length round them and two cells more. Built once per
/// `Env` from what the map draws behind the caster (`Env::content_before`) and the caster's
/// own coverage; read unchanged by every lookup.
#[derive(Clone, Debug)]
pub(crate) struct CasterBloom {
    frame: LightFrame,
    /// Half the raster's extent in tangent units: cell (0, 0) has its corner at -ext.
    ext: Vec2f,
    /// A cell's side in tangent units.
    cell: f32,
    cols: usize,
    rows: usize,
    /// g per cell, row by row, row 0 at -ext.y.
    g: Vec<f32>,
    /// The bright light the raster holds, the sum of b x cell² (the tests).
    #[cfg(test)]
    energy: f64,
}

impl CasterBloom {
    /// The bloom of `caster`, which has none yet, for a map `width` wide.
    /// - b, per cell: how far above the threshold the light behind it is, in EV
    ///   (log2(L / 2^bloom_threshold_ev); 0 at and under the threshold and for NaN). It is
    ///   weighted by the share that shows past the caster's own pattern, 1 - m (its gaps,
    ///   and all of it round its edges), whatever its strength and tint. `behind(d)` is what
    ///   the map draws at `d` (map frame) before the caster, times 2^intensity_ev. `sharp(d)`
    ///   is the caster's analytic coverage, which its softness raster softens. A cell that a
    ///   disc of `hot` (map frame; radius in radians) can touch is the mean of a grid of
    ///   samples fine enough for that disc.
    /// - S: b blurred by three box passes, each of half-width a third of the bloom length
    ///   (summed-area tables), scaled so the kernel's peak is 1. S is the bright light within
    ///   the bloom length, nearer counting more. A longer length only widens the kernel, so S
    ///   only grows with it.
    /// - g = 1 - exp(-S / BLOOM_REF_ENERGY); 0 below BLOOM_FLUSH.
    /// Counted as probe::Work::CasterBloom. None when `cancelled` (polled per row and per pass).
    pub(crate) fn build(
        caster: &CasterLayer,
        width: usize,
        hot: &[(Vec3f, f32)],
        behind: &dyn Fn(Vec3f) -> Vec3f,
        sharp: &dyn Fn(Vec3f) -> f32,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<CasterBloom> {
        #[cfg(test)]
        super::probe::count(super::probe::Work::CasterBloom);
        let p = &caster.params;
        let len = p.bloom_length_deg.to_radians().tan();
        // What the coverage reaches: the box, widened by the softness blur (decision 10).
        let soft = p.softness * CASTER_SOFT_SCALE * caster.half.x.min(caster.half.y);
        let reach = vec2f(caster.half.x + soft + len, caster.half.y + soft + len);
        let texel = std::f32::consts::TAU / width.max(2) as f32;
        let side = (width / BLOOM_WIDTH_PER_SIDE).clamp(BLOOM_MIN_SIDE, CASTER_RASTER_MAX_SIDE);
        // Two cells of padding a side are added below, so the reach takes side - 4 at most.
        let cell = texel.max(len / BLOOM_CELLS_PER_LENGTH).max(2.0 * reach.x.max(reach.y) / (side - 4) as f32);
        let ext = vec2f(reach.x + 2.0 * cell, reach.y + 2.0 * cell);
        let cols = ((2.0 * ext.x / cell).ceil() as usize).clamp(1, side);
        let rows = ((2.0 * ext.y / cell).ceil() as usize).clamp(1, side);
        let threshold = 2.0f32.powf(p.bloom_threshold_ev);
        let frame = caster.frame;
        let at = |x: f32, y: f32| (frame.center + frame.right * x + frame.up * y).normalize();
        // b at one direction. The coverage first: behind a fully covering part nothing shows,
        // and the lookup behind is the slow part.
        let ev_over = |d: Vec3f| -> f32 {
            let m = caster.unbloomed(d, sharp);
            if !(m < 1.0) {
                return 0.0;
            }
            let lum = luminance(behind(d));
            if !(lum > threshold) {
                return 0.0;
            }
            (lum.min(MAX_RADIANCE) / threshold).log2() * (1.0 - m.max(0.0))
        };
        // Each disc: where it touches the raster, and the samples a side a cell it touches
        // takes. In the caster's tangent plane a disc is its centre's projection and its
        // reach there, the outward side of its radius, which the plane stretches most
        // (tan(theta + r) - tan(theta)): a cell touches it when its square comes within that
        // reach. Only a disc too near the plane's edge to project (past PROJECT_MIN_COS)
        // falls back to an angle round its centre: its radius and a cell's half diagonal.
        let discs: Vec<(Touch, usize)> = hot
            .iter()
            .filter(|&&(c, r)| c.is_finite() && c.length() > 0.0 && r > 0.0)
            .map(|&(c, r)| {
                let n = ((BLOOM_DISC_SAMPLES_PER_RADIUS * cell / r).ceil() as usize).clamp(2, BLOOM_DISC_SAMPLES_MAX);
                let c = c.normalize();
                let theta = c.dot(frame.center).clamp(-1.0, 1.0).acos();
                let touch = if (theta + r).cos() >= PROJECT_MIN_COS {
                    let centre = vec2f(c.dot(frame.right), c.dot(frame.up)) / theta.cos();
                    Touch::Plane(centre, 0.5 * cell + (theta + r).tan() - theta.tan())
                } else {
                    Touch::Angle(c, (r + 0.75 * cell).min(std::f32::consts::PI).cos())
                };
                (touch, n)
            })
            .collect();
        let mut b = vec![0.0f32; cols * rows];
        for j in 0..rows {
            if cancelled() {
                return None;
            }
            let y = -ext.y + (j as f32 + 0.5) * cell;
            for i in 0..cols {
                let x = -ext.x + (i as f32 + 0.5) * cell;
                let d = at(x, y);
                let fine = discs
                    .iter()
                    .filter(|(touch, _)| match *touch {
                        Touch::Plane(centre, within) => (x - centre.x).abs() <= within && (y - centre.y).abs() <= within,
                        Touch::Angle(c, cos) => d.dot(c) >= cos,
                    })
                    .map(|&(_, n)| n)
                    .max();
                b[j * cols + i] = match fine {
                    None => ev_over(d),
                    Some(n) => {
                        let mut sum = 0.0f32;
                        for sj in 0..n {
                            let sy = y + ((sj as f32 + 0.5) / n as f32 - 0.5) * cell;
                            for si in 0..n {
                                let sx = x + ((si as f32 + 0.5) / n as f32 - 0.5) * cell;
                                sum += ev_over(at(sx, sy));
                            }
                        }
                        sum / (n * n) as f32
                    }
                };
            }
        }
        #[cfg(test)]
        let energy = b.iter().map(|&v| v as f64).sum::<f64>() * cell as f64 * cell as f64;
        // Three box passes of a third of the length: a kernel reaching exactly the length.
        // The cells within two of the raster's border are never read for a share where the
        // caster covers, so the tables' held edge there does no harm.
        let radius = (len / (3.0 * cell)).max(0.5) as f64;
        let mut grid = b;
        if grid.iter().any(|&v| v > 0.0) {
            for _ in 0..3 {
                if cancelled() {
                    return None;
                }
                grid = box_mean_pass(&grid, cols, rows, radius);
            }
        }
        // Three boxes of side w sum w^6 x their means; their kernel's peak is (3 w² / 4)². So
        // S, the blur with a kernel of peak 1, is (16 / 9) w² x the triple mean, w in tangent
        // units.
        let w = 2.0 * radius * cell as f64;
        let per_mean = 16.0 / 9.0 * w * w / BLOOM_REF_ENERGY;
        let g = grid
            .iter()
            .map(|&mean| {
                let g = 1.0 - (-(mean as f64) * per_mean).exp();
                if g > BLOOM_FLUSH { g.min(1.0) as f32 } else { 0.0 }
            })
            .collect();
        Some(CasterBloom {
            frame,
            ext,
            cell,
            cols,
            rows,
            g,
            #[cfg(test)]
            energy,
        })
    }

    /// g at `dir` (map frame): the raster read bilinearly between cell centres; 0 off it, and
    /// exactly 0 where its four cells are.
    pub(crate) fn share(&self, dir: Vec3f) -> f32 {
        let Some(p) = project(&self.frame, dir) else { return 0.0 };
        let u = (p.x + self.ext.x) / self.cell - 0.5;
        let v = (p.y + self.ext.y) / self.cell - 0.5;
        // Negated, so a NaN is off the raster too.
        if !(u >= -0.5 && v >= -0.5 && u <= self.cols as f32 - 0.5 && v <= self.rows as f32 - 0.5) {
            return 0.0;
        }
        let u = u.clamp(0.0, (self.cols - 1) as f32);
        let v = v.clamp(0.0, (self.rows - 1) as f32);
        let (i, j) = (u.floor() as usize, v.floor() as usize);
        let (i1, j1) = ((i + 1).min(self.cols - 1), (j + 1).min(self.rows - 1));
        let (tx, ty) = (u - i as f32, v - j as f32);
        let at = |i: usize, j: usize| self.g[j * self.cols + i];
        let top = at(i, j) + (at(i1, j) - at(i, j)) * tx;
        let bottom = at(i, j1) + (at(i1, j1) - at(i, j1)) * tx;
        (top + (bottom - top) * ty).clamp(0.0, 1.0)
    }

    /// Whether any cell thins. False when nothing behind the caster passes its threshold;
    /// the caster then keeps no raster and draws exactly as it does without bloom.
    pub(crate) fn thins_anything(&self) -> bool {
        self.g.iter().any(|&g| g > 0.0)
    }

    /// The bright light the raster holds, the sum of b x cell², in EV x tangent units².
    #[cfg(test)]
    pub(crate) fn energy(&self) -> f64 {
        self.energy
    }

    /// A cell's side in tangent units.
    #[cfg(test)]
    pub(crate) fn cell(&self) -> f32 {
        self.cell
    }
}

/// One box pass of `grid` (cols x rows, row by row): each cell becomes the mean of the box of
/// half-width `radius` cells round it, exactly, through a summed-area table (`SummedArea`,
/// whose edge cells are held past the border).
fn box_mean_pass(grid: &[f32], cols: usize, rows: usize, radius: f64) -> Vec<f32> {
    let img = EnvMap { width: cols, height: rows, data: grid.iter().map(|&v| [v, 0.0, 0.0, 1.0]).collect() };
    let table = SummedArea::<1>::build(&img, |t| [t[0] as f64]);
    let mut out = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        for i in 0..cols {
            // A rounding residue under zero is none.
            out.push(table.box_mean(i as f64 + 0.5, j as f64 + 0.5, radius, radius)[0].max(0.0) as f32);
        }
    }
    out
}

/// The caster builders the core's tests share (caster.rs, studio.rs,
/// prepare.rs): one copy, as plan 4's final review asked (D8).
#[cfg(test)]
pub(crate) mod fixtures {
    use crate::hdri::{CasterParams, GoboParams, LightParams};

    /// A shadow caster (redesign §5) as "+ Caster" builds one, at (az, el),
    /// `width_deg` x `height_deg`, its pattern `gobo` (None: solid), with `settings`.
    pub(crate) fn caster_at(azimuth_deg: f32, elevation_deg: f32, width_deg: f32, height_deg: f32, gobo: Option<GoboParams>, settings: CasterParams) -> LightParams {
        LightParams { azimuth_deg, elevation_deg, width_deg, height_deg, gobo, caster: Some(settings), ..LightParams::new_caster() }
    }

    /// A leaves pattern of `density`, grown from `seed`.
    pub(crate) fn leaves(density: f32, seed: u32) -> GoboParams {
        GoboParams { kind: "leaves".to_string(), density, seed, ..GoboParams::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fixtures::{caster_at, leaves};
    use super::super::studio::{light_frame, Studio};
    use crate::sky::luminance;
    use makepad_render_material::ibl;

    /// A studio of `lights` over a flat grey backdrop.
    fn grey_studio(lights: Vec<LightParams>) -> HdriParams {
        let mut p = HdriParams::default();
        p.mode = "studio".to_string();
        p.studio.top = [0.4; 3];
        p.studio.horizon = [0.4; 3];
        p.studio.floor = [0.4; 3];
        p.lights = lights;
        p
    }

    /// The default sky with a manual sun due north, 25 degrees up, and `lights`.
    fn sun_sky(lights: Vec<LightParams>) -> HdriParams {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.azimuth_deg = 0.0;
        p.sky.sun.elevation_deg = 25.0;
        p.lights = lights;
        p
    }

    /// A 40 x 30 degree softbox at (az, el), EV 2.
    fn softbox(azimuth_deg: f32, elevation_deg: f32) -> LightParams {
        LightParams { azimuth_deg, elevation_deg, width_deg: 40.0, height_deg: 30.0, softness: 0.3, intensity_ev: 2.0, ..LightParams::default() }
    }

    /// The default caster settings (strength 1, black) at `softness`.
    fn soft(softness: f32) -> CasterParams {
        CasterParams { softness, ..CasterParams::default() }
    }

    /// R4's oracle: a caster without softness (and without bloom) is exactly
    /// the Multiply flag `caster_flag` makes of it, texel for texel, in Studio
    /// and Sky mode, and so is the key it leaves (a softbox listed before it,
    /// the sun behind it). Every order, strength and tint rule can lean on it.
    #[test]
    fn a_plain_caster_draws_as_its_flag() {
        let settings = CasterParams { strength: 0.8, tint: [0.2, 0.5, 0.1], ..CasterParams::default() };
        let patterns = [None, Some(leaves(0.6, 3)), Some(GoboParams { kind: "blinds".to_string(), slats: 8, ..GoboParams::default() }), Some(GoboParams::default())];
        let key = LightParams { key: true, ..softbox(5.0, 25.0) };
        for gobo in patterns {
            let c = caster_at(0.0, 25.0, 50.0, 40.0, gobo.clone(), settings.clone());
            let pairs = [
                ("studio", grey_studio(vec![key.clone(), c.clone()]), grey_studio(vec![key.clone(), caster_flag(&c)])),
                ("sky", sun_sky(vec![c.clone()]), sun_sky(vec![caster_flag(&c)])),
            ];
            for (mode, with_caster, with_flag) in pairs {
                let (a, b) = (Env::new(&with_caster), Env::new(&with_flag));
                assert!(a.bake(64) == b.bake(64), "{mode}, {gobo:?}: the caster is not drawn as its flag");
                let (ka, kb) = (a.sun().expect("a key"), b.sun().expect("a key"));
                assert_eq!(ka.radiance, kb.radiance, "{mode}, {gobo:?}: the caster dims the key otherwise than its flag");
            }
        }
    }

    /// `gobo: None` is a solid caster (decision 8): its whole box covers. At
    /// strength 1 and a black tint nothing behind it shows; beyond the blur's
    /// reach the map is untouched, and nothing is ever brighter.
    #[test]
    fn a_caster_without_a_gobo_is_solid() {
        let lit = grey_studio(vec![softbox(90.0, 0.0)]);
        let open = Env::new(&lit).bake(64);
        let frame = light_frame(90.0, 0.0, 0.0);
        let half = 15f32.to_radians().tan();
        for softness in [0.0f32, 0.5] {
            let mut p = lit.clone();
            p.lights.push(caster_at(90.0, 0.0, 30.0, 30.0, None, soft(softness)));
            let shaded = Env::new(&p).bake(64);
            // The blur reaches CASTER_SOFT_SCALE x softness of the half size each way.
            let reach = softness * CASTER_SOFT_SCALE * half;
            let (mut inside, mut beside) = (0, 0);
            for (i, (o, s)) in open.data.iter().zip(&shaded.data).enumerate() {
                let (x, y) = (i % 64, i / 64);
                let dir = vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / 64.0, (y as f32 + 0.5) / 32.0]));
                let at = project(&frame, dir);
                let far_in = at.is_some_and(|q| q.x.abs().max(q.y.abs()) < half - reach - 0.02);
                let far_out = at.is_none_or(|q| q.x.abs().max(q.y.abs()) > half + reach + 0.02);
                for k in 0..3 {
                    assert!(s[k] <= o[k] * (1.0 + 1.0e-6) + 1.0e-7, "softness {softness}: ({x}, {y}) got brighter");
                    if far_in {
                        assert!(s[k] <= 1.0e-5 * o[k], "softness {softness}: ({x}, {y}) is under the caster: {} of {}", s[k], o[k]);
                    }
                    if far_out {
                        assert_eq!(s[k], o[k], "softness {softness}: ({x}, {y}) is beside the caster");
                    }
                }
                inside += far_in as usize;
                beside += far_out as usize;
            }
            assert!(inside > 10 && beside > 100, "softness {softness}: {inside} texels under it, {beside} beside");
        }
    }

    /// Spec §6: a caster whose pattern is an image mask that is not loaded
    /// (or names none) draws as no caster at all and leaves the key as it
    /// was. A Multiply flag with such a gobo draws solid; a caster does not.
    /// Loaded, the same caster draws and dims the key.
    #[test]
    fn a_caster_with_a_missing_picture_draws_nothing() {
        let mut images = ImageSet::default();
        // Opaque black stripes two texels wide on a transparent ground: half of it blocks.
        let stripes = (0..64).map(|i| if (i % 8) / 2 % 2 == 0 { [0.0, 0.0, 0.0, 1.0] } else { [1.0, 1.0, 1.0, 0.0] }).collect();
        images.insert("stripes", EnvMap { width: 8, height: 8, data: stripes });
        let mask = |name: &str| Some(GoboParams { kind: "image".to_string(), image: name.to_string(), ..GoboParams::default() });
        let key = LightParams { key: true, ..softbox(0.0, 25.0) };
        let open = Env::new_with(&grey_studio(vec![key.clone()]), &images);
        let (open_map, open_key) = (open.bake(64), open.sun().expect("a key").radiance);
        for softness in [0.0f32, 0.5] {
            for name in ["nowhere.png", ""] {
                let c = caster_at(0.0, 25.0, 50.0, 40.0, mask(name), soft(softness));
                assert!(!caster_drawn(&c, &images), "{name:?}: a missing picture");
                assert!(CasterLayer::new(&c, &images).is_none(), "{name:?}: no layer");
                let env = Env::new_with(&grey_studio(vec![key.clone(), c]), &images);
                assert!(env.bake(64) == open_map, "softness {softness}, {name:?}: a caster with a missing picture drew");
                assert_eq!(env.sun().expect("a key").radiance, open_key, "softness {softness}, {name:?}: it dimmed the key");
            }
            let c = caster_at(0.0, 25.0, 50.0, 40.0, mask("stripes"), soft(softness));
            assert!(caster_drawn(&c, &images));
            let env = Env::new_with(&grey_studio(vec![key.clone(), c]), &images);
            assert!(env.bake(64) != open_map, "softness {softness}: the loaded stripes draw");
            let share = luminance(env.sun().expect("a key").radiance) / luminance(open_key);
            assert!(share < 0.8, "softness {softness}: the stripes dim the key, which keeps {share}");
        }
        // Every other caster draws, on or off (the studio leaves out what is off).
        assert!(caster_drawn(&caster_at(0.0, 25.0, 50.0, 40.0, None, soft(0.0)), &ImageSet::default()));
        assert!(caster_drawn(&LightParams { enabled: false, ..caster_at(0.0, 25.0, 50.0, 40.0, Some(leaves(0.5, 3)), soft(0.0)) }, &ImageSet::default()));
    }

    /// Decision 10: softness rasters the caster's pattern once per Env, and a
    /// sharp caster rasters nothing (its pattern is drawn analytically). A
    /// bake, a parallel bake and the key all read the one raster; a caster
    /// that is off rasters nothing.
    #[test]
    fn a_soft_casters_raster_is_built_once_and_not_when_sharp() {
        use crate::hdri::probe::{counted, Work};
        let key = LightParams { key: true, ..softbox(0.0, 25.0) };
        let before = counted(Work::CasterSoft);
        let sharp = Env::new(&grey_studio(vec![key.clone(), caster_at(0.0, 25.0, 50.0, 40.0, Some(leaves(0.5, 3)), soft(0.0))]));
        sharp.bake(32);
        sharp.sun();
        assert_eq!(counted(Work::CasterSoft), before, "a sharp caster rasters nothing");
        let p = grey_studio(vec![key, caster_at(0.0, 25.0, 50.0, 40.0, Some(leaves(0.5, 3)), soft(0.4)), caster_at(90.0, 10.0, 30.0, 30.0, None, soft(1.0))]);
        let env = Env::new(&p);
        assert_eq!(counted(Work::CasterSoft), before + 2, "one raster per soft caster");
        let serial = env.bake(32);
        let parallel = env.bake_par(32, |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f));
        env.sun();
        assert!(serial == parallel, "a parallel bake reads the same raster");
        assert_eq!(counted(Work::CasterSoft), before + 2, "the bakes and the key read the rasters the Env built");
        let mut off = p.clone();
        off.lights[1].enabled = false;
        off.lights[2].enabled = false;
        Env::new(&off);
        assert_eq!(counted(Work::CasterSoft), before + 2, "a caster that is off rasters nothing");
    }

    /// The mean coverage of `c` over its box and `pad` past it each way, as
    /// the studio draws it black at full strength, on an n x n midpoint grid
    /// of its tangent plane (where a box filter keeps area).
    fn mean_coverage(c: &LightParams, pad: f32, n: usize) -> f64 {
        let black = LightParams { caster: Some(CasterParams { strength: 1.0, tint: [0.0; 3], ..c.caster_params() }), ..c.clone() };
        let studio = Studio::new(&StudioParams::default(), &[black]);
        let (frame, sx, half) = face_axes(&caster_flag(c));
        let ext = vec2f(half.x + pad, half.y + pad);
        let mut sum = 0.0f64;
        for j in 0..n {
            for i in 0..n {
                let q = vec2f(ext.x * (-1.0 + 2.0 * (i as f32 + 0.5) / n as f32), ext.y * (-1.0 + 2.0 * (j as f32 + 0.5) / n as f32));
                let d = (frame.center + frame.right * (sx * q.x) + frame.up * q.y).normalize();
                sum += (1.0 - studio.apply_lights(d, Vec3f::all(1.0)).x) as f64;
            }
        }
        sum / (n * n) as f64
    }

    /// Spec §5: softness blurs the whole caster through a box filter, so the
    /// light it lets through stays right: over its box and the blur's reach,
    /// a soft caster covers what the sharp one does, to within 2.5 %.
    #[test]
    fn softness_keeps_the_light_a_caster_lets_through() {
        let patterns = [
            ("leaves", leaves(0.5, 3)),
            ("dense leaves", leaves(0.9, 3)),
            ("fern", GoboParams { kind: "fern".to_string(), density: 0.6, ..GoboParams::default() }),
            ("window", GoboParams::default()),
            ("blinds", GoboParams { kind: "blinds".to_string(), slats: 10, ..GoboParams::default() }),
        ];
        // The widest blur reaches CASTER_SOFT_SCALE of the shorter half extent past the box.
        let pad = CASTER_SOFT_SCALE * 22.5f32.to_radians().tan() + 0.01;
        for (name, gobo) in patterns {
            let sharp_caster = caster_at(0.0, 20.0, 60.0, 45.0, Some(gobo), soft(0.0));
            // The sharp map's hard edges alias on a coarse grid (the window's
            // muntins read 2 % high at 300); the blurred one is smooth.
            let sharp = mean_coverage(&sharp_caster, pad, 600);
            assert!(sharp > 0.05 && sharp < 0.95, "{name}: {sharp}");
            for softness in [0.3f32, 1.0] {
                let blurred = mean_coverage(&LightParams { caster: Some(soft(softness)), ..sharp_caster.clone() }, pad, 300);
                println!("{name} at softness {softness}: sharp {sharp:.4}, soft {blurred:.4}");
                assert!((sharp - blurred).abs() < 0.025 * sharp, "{name} at softness {softness}: sharp {sharp}, soft {blurred}");
            }
        }
    }

    /// `soft`'s coverage at face point `q` by brute force: the mean of its
    /// cells' values (`cell_value` of the pattern; zero in the ring round
    /// them and past it) over the box `texel_box` places, each cell weighed
    /// by how much of the box it covers along each axis.
    fn box_mean_by_overlap(soft: &SoftCoverage, pattern: &Pattern, q: Vec2f) -> f64 {
        let (w, h) = (soft.table.width(), soft.table.height());
        let (x, y, rx, ry) = soft.texel_box(q);
        let (rx, ry) = (rx.max(0.5), ry.max(0.5));
        let cover = |lo: f64, hi: f64, i: usize| (hi.min((i + 1) as f64) - lo.max(i as f64)).max(0.0);
        let (i0, i1) = ((x - rx).floor().max(0.0) as usize, ((x + rx).ceil().max(0.0) as usize).min(w));
        let (j0, j1) = ((y - ry).floor().max(0.0) as usize, ((y + ry).ceil().max(0.0) as usize).min(h));
        let mut sum = 0.0;
        for j in j0..j1 {
            if j == 0 || j + 1 == h {
                continue;
            }
            let down = cover(y - ry, y + ry, j);
            for i in i0..i1 {
                if i == 0 || i + 1 == w {
                    continue;
                }
                sum += cell_value(pattern, soft.ext, soft.cell, i, j) as f64 * cover(x - rx, x + rx, i) * down;
            }
        }
        sum / (4.0 * rx * ry)
    }

    /// Decision 10: a soft caster's coverage is the box filter of its pattern:
    /// the pattern rastered is the flag's own coverage as the studio draws it,
    /// and the coverage at any point is the exact mean of the raster's cells
    /// over the blur's box, rolled, small, and at the raster's sample budget.
    #[test]
    fn a_soft_caster_is_a_box_filter_of_its_pattern() {
        let images = ImageSet::default();
        let cases = [
            ("leaves", caster_at(30.0, 20.0, 60.0, 45.0, Some(leaves(0.6, 3)), soft(0.5))),
            ("a rolled window", LightParams { roll_deg: 20.0, ..caster_at(200.0, 40.0, 40.0, 30.0, Some(GoboParams::default()), soft(1.0)) }),
            ("a small solid", caster_at(120.0, -10.0, 8.0, 5.0, None, soft(0.2))),
            ("leaves at a slight softness", caster_at(300.0, 10.0, 90.0, 60.0, Some(leaves(0.8, 3)), soft(0.05))),
        ];
        let white = Vec3f::all(1.0);
        for (name, c) in cases {
            let layer = CasterLayer::new(&c, &images).expect("a caster that draws");
            let soft = layer.soft.as_ref().expect("a soft caster has a raster");
            let flag = caster_flag(&c);
            let (_, _, half) = face_axes(&flag);
            let pattern = Pattern::new(&flag, half, &images);
            let (w, h) = (soft.table.width(), soft.table.height());
            assert!(w <= CASTER_RASTER_MAX_SIDE && h <= CASTER_RASTER_MAX_SIDE, "{name}: {w} x {h}");
            // The studio's drawing of a black flag of the same caster: 1 - its coverage.
            let black = caster_flag(&LightParams { caster: Some(CasterParams { strength: 1.0, tint: [0.0; 3], ..c.caster_params() }), ..c.clone() });
            let drawn = Studio::new(&StudioParams::default(), &[black]);
            let mut blurred = 0;
            for j in 0..25 {
                for i in 0..33 {
                    let at = vec2f(soft.ext.x * (-1.0 + 2.0 * (i as f32 + 0.5) / 33.0), soft.ext.y * (-1.0 + 2.0 * (j as f32 + 0.5) / 25.0));
                    let d = (layer.frame.center + layer.frame.right * (layer.sx * at.x) + layer.frame.up * at.y).normalize();
                    let t = project(&layer.frame, d).expect("in front of the caster");
                    let q = vec2f(layer.sx * t.x, t.y);
                    let flag_m = 1.0 - drawn.apply_lights(d, white).x;
                    assert!((pattern.at(q) - flag_m).abs() < 1.0e-5, "{name} at {q:?}: the rastered pattern {} is not the flag's {flag_m}", pattern.at(q));
                    // A soft caster never asks for the sharp coverage.
                    let (got, want) = (layer.coverage(d, &|_| f32::NAN), box_mean_by_overlap(soft, &pattern, q));
                    assert!((got as f64 - want).abs() < 1.0e-5, "{name} at {q:?}: {got}, the box mean is {want}");
                    if got > 0.02 && got < 0.98 {
                        blurred += 1;
                    }
                }
            }
            assert!(blurred > 20, "{name}: only {blurred} of the samples lie in a blurred edge");
        }
    }

    /// R5, the bloom's property 2: the share is exactly 0 farther than the bloom length (and
    /// the raster's cells round it) from anything bright behind the caster, and a cancelled
    /// pre-pass stops. A solid caster 40 x 30 degrees due north, 20 degrees up, 6 degrees of
    /// bloom; behind it one spot a degree across at 2^14 (4 EV over the default threshold),
    /// 3 degrees right of its centre, and a dim sky. The pattern is open (`sharp` 0), so the
    /// spot shows and the share is the bloom's alone.
    #[test]
    fn the_bloom_share_is_exactly_zero_beyond_its_length() {
        use crate::hdri::{dir_from_az_el, CasterParams, ImageSet, LightParams};
        let light = LightParams {
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 40.0,
            height_deg: 30.0,
            gobo: None,
            caster: Some(CasterParams { bloom_strength: 1.0, bloom_length_deg: 6.0, ..CasterParams::default() }),
            ..LightParams::new_caster()
        };
        let caster = CasterLayer::new(&light, &ImageSet::default()).expect("a solid caster draws");
        let spot = dir_from_az_el(3.0, 20.0);
        let cos_spot = 0.5f32.to_radians().cos();
        let behind = |d: Vec3f| if d.dot(spot) >= cos_spot { Vec3f::all(16384.0) } else { Vec3f::all(0.5) };
        let open = |_: Vec3f| 0.0;
        let bloom = CasterBloom::build(&caster, 1024, &[], &behind, &open, &|| false).expect("nothing cancels it");
        assert!(bloom.thins_anything(), "the spot is bright");
        assert!(bloom.share(spot) > 0.5, "on the spot: {}", bloom.share(spot));
        // 10 degrees from the spot: past 6 degrees of bloom, the spot's half degree and three cells.
        for (az, el) in [(13.0, 20.0), (-7.0, 20.0), (3.0, 30.0), (3.0, 10.0), (-15.0, 32.0)] {
            assert_eq!(bloom.share(dir_from_az_el(az, el)), 0.0, "at azimuth {az}, elevation {el}");
        }
        assert!(CasterBloom::build(&caster, 1024, &[], &behind, &open, &|| true).is_none(), "a cancelled pre-pass stops");
    }
}
