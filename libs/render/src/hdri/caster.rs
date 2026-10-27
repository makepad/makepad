//! Shadow casters (redesign §5): a layer of the list that adds no light and
//! multiplies what is drawn before it by its transmission,
//!
//! ```text
//! acc * (1 - s*m + s*m*tint)
//! ```
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
/// A softness raster's cell is never finer than this many of the map's texels (M5): the held
/// 512 preview rasters a caster as coarsely as it draws it, not as finely as the released one.
const SOFT_TEXELS_PER_CELL: f32 = 1.0;
/// The same for a map narrower than CASTER_REFERENCE_WIDTH, the held preview (M5, fix round 4 of
/// S3's review): a little coarser than it draws, so four leaves casters at softness 0.3 raster
/// a third fewer pattern samples (a leaves pattern's sample is the raster's dearest part) on
/// every held bake. The released preview and an export keep SOFT_TEXELS_PER_CELL.
const SOFT_HELD_TEXELS_PER_CELL: f32 = 1.25;
/// The rows of a caster raster one call of a row runner takes (`rows_by`): `cancelled` is
/// polled between such calls.
const RASTER_ROWS_PER_RUN: usize = 64;

/// How a caster raster's rows are spread (M5): `run(n, row)` calls `row(0..n)`, each once,
/// as `Env::bake_par`'s runner does (`pool.fan_out` inside a Heavy job, a plain loop
/// elsewhere). A runner may skip rows once its job is overtaken: a bloom is then not made, and
/// a softness raster is left empty (the job drops the map).
pub(crate) type RowRunner<'a> = &'a dyn Fn(usize, &(dyn Fn(usize) + Sync));

/// The runner of a raster built on the caller's thread alone.
pub(crate) fn serial_rows(n: usize, row: &(dyn Fn(usize) + Sync)) {
    (0..n).for_each(row);
}

/// `row(0..rows)` through `run`, at most RASTER_ROWS_PER_RUN at a call and as many at each
/// (68 rows go as 34 and 34, not 64 and 4, so no call is a thread's row alone), `cancelled`
/// polled before each call. None when it says so, or when the runner skipped a row.
fn rows_by<T: Send + Sync>(rows: usize, run: RowRunner, cancelled: &dyn Fn() -> bool, row: &(dyn Fn(usize) -> T + Sync)) -> Option<Vec<T>> {
    rows_in_calls(rows, RASTER_ROWS_PER_RUN, run, cancelled, row)
}

/// `rows_by` with at most `per_call` rows at a call.
fn rows_in_calls<T: Send + Sync>(rows: usize, per_call: usize, run: RowRunner, cancelled: &dyn Fn() -> bool, row: &(dyn Fn(usize) -> T + Sync)) -> Option<Vec<T>> {
    let out: Vec<std::sync::OnceLock<T>> = (0..rows).map(|_| std::sync::OnceLock::new()).collect();
    let per_run = rows.div_ceil(rows.div_ceil(per_call.max(1)).max(1)).max(1);
    for start in (0..rows).step_by(per_run) {
        if cancelled() {
            return None;
        }
        let n = per_run.min(rows - start);
        run(n, &|k| {
            let _ = out[start + k].set(row(start + k));
        });
    }
    out.into_iter().map(|done| done.into_inner()).collect()
}

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
    /// `SummedArea<1>`: once per `Env`, for the map `width` wide the Env is
    /// built for (its cells no finer than that map's texel), whatever width
    /// it is baked at. `run` spreads its rows (`RowRunner`).
    pub(crate) fn new(light: &LightParams, images: &ImageSet, width: usize, run: RowRunner) -> Option<CasterLayer> {
        if !caster_drawn(light, images) {
            return None;
        }
        let flag = caster_flag(light);
        let settings = light.caster_params();
        // The flag's own box: the studio draws it in this frame and half extents.
        let (frame, sx, half) = face_axes(&flag);
        let radius = settings.softness * CASTER_SOFT_SCALE * half.x.min(half.y);
        let texel = std::f32::consts::TAU / width.max(2) as f32;
        let min_cell = texel * if width < CASTER_REFERENCE_WIDTH { SOFT_HELD_TEXELS_PER_CELL } else { SOFT_TEXELS_PER_CELL };
        let soft = (radius > 0.0).then(|| Arc::new(SoftCoverage::build(&Pattern::new(&flag, half, images), half, radius, min_cell, run)));
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

    /// About the mean of m before bloom over the box of half sides `half` round point `t` of
    /// this caster's tangent plane, for a soft caster (`SoftCoverage::mean_over`); None for a
    /// sharp one, whose pattern is looked at where it is wanted.
    fn mean_unbloomed(&self, t: Vec2f, half: Vec2f) -> Option<f32> {
        self.soft.as_ref().map(|soft| soft.mean_over(vec2f(self.sx * t.x, t.y), half).max(0.0).min(1.0))
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

    /// Takes the bloom away (`Studio::clear_caster_blooms`): the caster is as it was built.
    pub(crate) fn clear_bloom(&mut self) {
        self.bloom = None;
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
    /// `half`, blurred `radius` each way, its cells no finer than `min_cell`
    /// (the map's texel times SOFT_TEXELS_PER_CELL, or SOFT_HELD_TEXELS_PER_CELL
    /// for the held preview); `run` spreads its rows.
    fn build(pattern: &Pattern, half: Vec2f, radius: f32, min_cell: f32, run: RowRunner) -> SoftCoverage {
        #[cfg(test)]
        super::probe::count(super::probe::Work::CasterSoft);
        // Along each axis: the box and the blur's reach on both sides in
        // cells of radius / SOFT_CELLS_PER_RADIUS, no finer than
        // `min_cell`, coarser when the samples would pass
        // SOFT_SAMPLE_BUDGET, then an empty cell on each side.
        let span = vec2f(2.0 * (half.x + radius), 2.0 * (half.y + radius));
        let target = (radius / SOFT_CELLS_PER_RADIUS).max(min_cell);
        let samples = (span.x / target).ceil().max(2.0) * (span.y / target).ceil().max(2.0) * (SOFT_SUBSAMPLES * SOFT_SUBSAMPLES) as f32;
        let coarsen = (samples / SOFT_SAMPLE_BUDGET as f32).sqrt().max(1.0);
        let inner = |span: f32| ((span / (target * coarsen)).ceil() as usize).clamp(2, CASTER_RASTER_MAX_SIDE - 2);
        let (nx, ny) = (inner(span.x), inner(span.y));
        let cell = vec2f(span.x / nx as f32, span.y / ny as f32);
        let ext = vec2f(half.x + radius + cell.x, half.y + radius + cell.y);
        let (w, h) = (nx + 2, ny + 2);
        // The rows inside the ring of empty cells, each built where `run` puts it, all in one
        // call. Nothing here cancels them (a raster this size is a few milliseconds' work); a
        // runner that skipped rows was overtaken, its whole map is dropped, and the raster
        // stays empty.
        let inside = rows_in_calls(h - 2, usize::MAX, run, &|| false, &|r| (1..w - 1).map(|i| cell_value(pattern, ext, cell, i, r + 1)).collect::<Vec<f32>>());
        let mut data = vec![[0.0f32; 4]; w * h];
        for (r, row) in inside.into_iter().flatten().enumerate() {
            for (k, value) in row.into_iter().enumerate() {
                data[(r + 1) * w + k + 1][0] = value;
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

    /// About the mean of `at` over the box of half sides `half` round face point `q`, 0..=1, in
    /// one look: the blur's box widened to the spread of both boxes together (sqrt(r² + h²) each
    /// way, the box of the same second moment). A bloom raster reads its cells' coverage so (fix
    /// round 3 of S3's review), where it took a look per map texel.
    fn mean_over(&self, q: Vec2f, half: Vec2f) -> f32 {
        let (x, y, _, _) = self.texel_box(q);
        let widened = |h: f32, cell: f32| ((self.radius * self.radius + h * h).sqrt() / cell) as f64;
        self.table.box_mean(x, y, widened(half.x, self.cell.x), widened(half.y, self.cell.y))[0] as f32
    }
}

/// The bright light, in EV above the threshold times tangent-plane area (about steradians
/// near a caster's centre), that thins a caster by 1 - 1/e at the peak of its bloom: a
/// half-degree disc (6.0e-5) 2.5 EV over its threshold. The default sun (about 2^18 against
/// the default 2^10, 0.53 degrees across: 8 EV over 6.7e-5, 5.4e-4) thins the leaves next to
/// it by about 97 % at bloom 1.
const BLOOM_REF_ENERGY: f64 = 1.5e-4;
/// A bloom raster's cell is never finer than a sixth of its bloom length at
/// CASTER_REFERENCE_WIDTH, a finer share in proportion to a wider map's (up to
/// BLOOM_SAMPLES_PER_LENGTH): the kernel then reaches at most that many cells either way, and
/// the cell-averaged kernel blurs it (`kernel_pass`). A long bloom needs no finer cells to be
/// smooth, so the held preview, which shares the released one's raster (D6), stays cheap with
/// several long blooms (M5); an export keeps a fine one. What a cell holds is still sampled
/// as finely as before (BLOOM_SAMPLES_PER_LENGTH).
const BLOOM_CELLS_PER_LENGTH: f32 = 6.0;
/// A cell's light is the mean over its area (S3 review, fix round 2: a cell's centre alone
/// held all of a light smaller than the cell or none of it, and read a leaves caster's gaps
/// some 36 times per kernel area). The light behind is sampled no coarser than a 32nd of the
/// bloom length, the map's texel, or what the raster's room allows (the cell this raster had
/// before its cells grew to BLOOM_CELLS_PER_LENGTH), up to BLOOM_CELL_SAMPLES_MAX samples a
/// side; the caster's coverage, which costs far less to look at, at the map's texel, up to
/// BLOOM_COVERAGE_LOOKS_MAX looks a side, each sample of the light behind times the mean of
/// the looks over its own part of the cell (a soft caster's mean in one look,
/// `SoftCoverage::mean_over`: fix round 3 of S3's review, M5).
const BLOOM_SAMPLES_PER_LENGTH: f32 = 32.0;
const BLOOM_CELL_SAMPLES_MAX: usize = 8;
const BLOOM_COVERAGE_LOOKS_MAX: usize = 16;
/// A cell where nothing asks for finer samples takes the light behind it at its four corners
/// only, shared with its neighbours, and the caster's coverage at its samples: the light is
/// read bilinearly between the corners where they differ by at most this many EV and all pass
/// the threshold. A cell whose corners differ more (an edge of the light behind) takes the
/// light at every sample, and one whose corners are all well under the threshold
/// (BLOOM_NEAR_EV) holds none.
const BLOOM_SMOOTH_EV: f32 = 0.5;
/// Fix round 3 of S3's review: what the cells' corners alone may miss of what is not a light
/// listed before the caster. A cell whose corners are all at or under the threshold, but one of
/// them within BLOOM_NEAR_EV of it (a cloud top, the band over the horizon, under a low
/// threshold), takes the light behind at its samples; one whose corners are smooth takes a look
/// at its centre too, and takes its samples where that look is more than BLOOM_CENTRE_EV off the
/// corners' mean (a band brighter between them). What the base holds finer than that (a base
/// image's highlights, a storm's bolt) the base reports itself (`Env::base_peak`): the cells it
/// may brighten past their corners take the light behind at the spacing it asks for, from the
/// cell's own count to BLOOM_LIGHT_SAMPLES_MAX a side (BLOOM_CELL_BUDGET).
const BLOOM_NEAR_EV: f32 = 2.0;
const BLOOM_CENTRE_EV: f32 = 0.1;
/// The most cells the base's detail alone asks to sample finely that are kept as points
/// (`BloomPoint`, each the light of its cell where it lies), as a light no wider than a cell is:
/// a highlight or a few of them. A base that asks for more (a bright photograph) is rastered,
/// each cell's light still where it lies in it (`CellLight`).
const BLOOM_FINE_POINTS_MAX: usize = 32;
/// The cells a light listed before the caster may reach (`BrightBox`) take the light behind at
/// samples this many to its narrowest feature (a plain light) or to the map's texel where that
/// is finer (a light with a picture, a shape core or something hung in front of it), and at the
/// map's texel where their corners are not all alike inside it (its edge may cross them), so
/// the light holds still as it moves; from the cell's own count to BLOOM_LIGHT_SAMPLES_MAX a
/// side (BLOOM_CELL_BUDGET). A light no wider than BLOOM_LIGHT_POINT_CELLS takes the same
/// spacing on a grid of its own.
const BLOOM_LIGHT_SAMPLES_PER_FEATURE: f32 = 6.0;
const BLOOM_LIGHT_SAMPLES_MAX: usize = 16;
/// M5 (fix rounds 3 and 4 of S3's review): the most lookups behind the casters the bloom rasters
/// of one map take past their cells' corners (the samples of the lights' cells, of the base's
/// detail and of the cells whose corners are not alike) at CASTER_REFERENCE_WIDTH, in proportion
/// to a wider map's texels, shared alike by its blooming casters, so four cost no more than one
/// may. Where a raster's cells ask for more than its share, their samples a side are scaled
/// alike, never under what a cell keeps (`CasterBloom::build`: the base's detail, a cell near
/// the threshold or whose centre is off its corners); a light's other cell left one sample reads
/// its corners. A light with detail or dim parts that fills the raster asked for the map's texel
/// over every cell: behind four soft blooming casters, a picture light took the held 512 bake's
/// pre-pass to 65 % of the bake (hdri_perf's M5; about 22 % so held). The discs' cells and grids
/// are not counted.
const BLOOM_CELL_BUDGET: usize = 1 << 13;
/// A light whose reach in the caster's plane is at most this many cells each way from its
/// middle is sampled as a disc is, on a grid of its own square kept as points (`BloomPoint`),
/// so the kernel blurs it from where it is, not from the cells it falls in.
const BLOOM_LIGHT_POINT_CELLS: f32 = 1.0;
/// A map `width` wide has a bloom raster of at most width / 12 cells a side, the width held to
/// at least CASTER_REFERENCE_WIDTH (D6): 85 at the held 512 preview and the released 1024 one,
/// which so read the same raster, 341 at 4K. This keeps the pre-pass a small share of the bake
/// (hdri_perf's `a_bloom_casters_pre_pass_stays_well_under_its_bake`).
const BLOOM_WIDTH_PER_SIDE: usize = 12;
/// The fewest cells a side, for the smallest maps.
const BLOOM_MIN_SIDE: usize = 16;
/// The share of a raster's side a caster's box and soft edge take at most: a bloom up to a
/// quarter of the box's half extent long fits in the rest at the same cell.
const BLOOM_BOX_SHARE: f32 = 0.8;
/// The most cells either way the raster is blurred by the kernel itself (`kernel_pass`): a
/// kernel reaching farther (a bloom more than about 7.5 cells long) takes three box passes.
const BLOOM_KERNEL_TAPS: usize = 8;
/// Samples per disc radius along each axis of a cell a disc too near the plane's edge to
/// project can touch (`Touch::Angle`), so a disc smaller than a cell is area-averaged into it
/// and neither pops nor vanishes as it moves; from 2 to BLOOM_DISC_SAMPLES_MAX a side.
const BLOOM_DISC_SAMPLES_PER_RADIUS: f32 = 6.0;
/// 12, not 24: on a volumetric sky each sample marches the clouds, and 24 a side over the
/// sun's cells took the held 512 pre-pass to 64 % of its bake (hdri_perf).
const BLOOM_DISC_SAMPLES_MAX: usize = 12;
/// A disc in the caster's plane (`Touch::Plane`) is sampled on a grid of its own square,
/// apart from the raster, at a spacing of at most a third of its reach and at most half the
/// kernel's box width w (D6), so a bloom shorter than the disc still sees it as a disc. The
/// grid does not depend on the raster's cell, so the held and the released previews sample a
/// disc alike.
const BLOOM_DISC_GRID_PER_REACH: f32 = 3.0;
const BLOOM_DISC_GRID_PER_WIDTH: f32 = 2.0;
/// The most samples a side of a disc's grid, each a lookup behind the caster: 256 for a bloom
/// shorter than about 0.4 of the disc's reach, 36 for one longer than its reach (about 0.3
/// degrees beside the default sun). A disc that asks for more (a large sun or moon behind a
/// short bloom) has grid steps wider than that spacing; each sample then looks at the caster's
/// own coverage over its step at the spacing asked (BLOOM_DISC_SUBSAMPLES_MAX at most a side,
/// no lookup behind), and spreads its light over the part of its step that shows
/// (`BloomPoint`), so the bloom thins a band along the caster's edge, not beads at the samples.
const BLOOM_DISC_GRID_MAX: usize = 16;
/// The most looks at the caster's coverage a side of one sample of a disc's capped grid.
const BLOOM_DISC_SUBSAMPLES_MAX: usize = 16;
/// A disc's grid of this many looks or fewer (the default sun's 6 x 6 beside a long bloom) is
/// built on the caller's thread: a call of the pool's runner costs more.
const BLOOM_DISC_SERIAL_LOOKS: usize = 64;
/// A point whose light spreads over less than this share of the kernel's box width w is blurred
/// by the kernel itself (`spline`), the limit its spread kernel tends to.
const BLOOM_POINT_MIN_SPREAD: f32 = 1.0e-3;
/// Samples a side of a raster cell a disc's square covers part of: what lies outside the
/// square is the cell's, what lies inside it the disc's.
const BLOOM_CELL_SAMPLES: usize = 4;
/// A share below this is none: far from any bright light, the summed-area tables' rounding
/// leaves exactly 0.
const BLOOM_FLUSH: f64 = 1.0e-6;

/// A light listed before a caster, as its bloom's pre-pass sees it (`Studio::bright_boxes`,
/// `CasterBloom::build`): where it may add light, the cone round `axis` (map frame) of
/// cosine `cos_cover` and, unless its points place it (a stroke), the box of half extents
/// `reach` in `frame`'s tangent plane; its narrowest feature (tangent units); and whether it
/// has detail inside its box (a picture, a shape core, something hung in front of it).
#[derive(Clone, Copy, Debug)]
pub(crate) struct BrightBox {
    pub(crate) axis: Vec3f,
    pub(crate) cos_cover: f32,
    pub(crate) frame_reach: Option<(LightFrame, Vec2f)>,
    pub(crate) feature: f32,
    pub(crate) detailed: bool,
}

/// Where a light of `BrightBox` touches a bloom raster: the box round its reach in the
/// caster's plane (centre, half sides), or, when some of it lies too near the plane's edge to
/// project, the cosine within which a cell's centre may lie of its axis.
enum LightReach {
    Plane(Vec2f, Vec2f),
    Angle(Vec3f, f32),
}

/// Where a bright disc touches a bloom raster (`CasterBloom::build`).
enum Touch {
    /// Its centre in the caster's tangent plane and its reach there (half the side of the
    /// square its grid covers): it is sampled on that grid and blurred as points
    /// (`BloomPoint`); the raster keeps what lies outside the square.
    Plane(Vec2f, f32),
    /// Its centre (map frame), the cosine within which a cell's centre may lie for the cell to
    /// touch it, and the samples a side such a cell takes, averaged into the raster.
    Angle(Vec3f, f32, usize),
}

/// A share of a disc's light behind a caster, kept apart from the raster (D6): its centre in
/// the caster's tangent plane, S / BLOOM_REF_ENERGY it brings at the kernel's peak, and the half
/// sides of the box it spreads evenly over (its light's own spread, matched by its second
/// moments: the part of a grid step, or of a group of steps, that shows past the caster). It is
/// blurred by the kernel over that box wherever it is looked up (`spread`), so a bloom shorter
/// than a raster cell, or than a disc's grid step, still blooms it over its own length.
#[derive(Clone, Copy, Debug)]
struct BloomPoint {
    at: Vec2f,
    weight: f32,
    half: Vec2f,
}

/// A disc's light over part of its grid (`CasterBloom::build`): b x area summed, and its first
/// and second moments about the disc's centre (each look's own square of side q adding
/// q² / 12), tangent units.
#[derive(Clone, Copy, Debug, Default)]
struct Moments {
    mass: f64,
    x: f64,
    y: f64,
    xx: f64,
    yy: f64,
}

impl Moments {
    /// `mass` spread evenly over the square of side `q` centred at `x`, `y`.
    fn add(&mut self, mass: f64, x: f32, y: f32, q: f32) {
        let (x, y, own) = (x as f64, y as f64, (q as f64) * (q as f64) / 12.0);
        self.mass += mass;
        self.x += mass * x;
        self.y += mass * y;
        self.xx += mass * (x * x + own);
        self.yy += mass * (y * y + own);
    }

    fn merge(&mut self, other: &Moments) {
        self.mass += other.mass;
        self.x += other.x;
        self.y += other.y;
        self.xx += other.xx;
        self.yy += other.yy;
    }

    /// This light times `factor`, where it lies unchanged.
    fn scaled(&self, factor: f64) -> Moments {
        Moments { mass: self.mass * factor, x: self.x * factor, y: self.y * factor, xx: self.xx * factor, yy: self.yy * factor }
    }

    /// The point that holds this light, about `centre`: at its centroid, spread over the box
    /// of its second moments (half side sqrt(3 var), the box with the same spread).
    fn point(&self, centre: Vec2f) -> BloomPoint {
        let (cx, cy) = (self.x / self.mass, self.y / self.mass);
        let half = |second: f64, mean: f64| (3.0 * (second / self.mass - mean * mean).max(0.0)).sqrt() as f32;
        BloomPoint { at: vec2f(centre.x + cx as f32, centre.y + cy as f32), weight: (self.mass / BLOOM_REF_ENERGY) as f32, half: vec2f(half(self.xx, cx), half(self.yy, cy)) }
    }
}

/// A point's kernel along one axis, `d` from its centre: the peak-1 kernel (`spline`, box width
/// `w`) blurred over the point's box of half side `half`, its integral that of `spline` (4/3 w).
/// The kernel itself for a point of no spread.
fn spread(d: f32, half: f32, w: f32) -> f32 {
    if half <= BLOOM_POINT_MIN_SPREAD * w {
        return spline(d / w);
    }
    let (d, half, w) = (d as f64, half as f64, w as f64);
    (4.0 / 3.0 * w / (2.0 * half) * (spline_cdf((d + half) / w) - spline_cdf((d - half) / w))) as f32
}

/// The profile of three boxes of width 1 along one axis, its peak scaled to 1, at `u` box
/// widths from its centre: a quadratic B-spline that reaches 1.5 widths. The bloom's kernel is
/// it along both axes, exactly what three box passes of the raster approach.
fn spline(u: f32) -> f32 {
    let u = u.abs();
    if u <= 0.5 {
        1.0 - 4.0 / 3.0 * u * u
    } else if u < 1.5 {
        2.0 / 3.0 * (1.5 - u) * (1.5 - u)
    } else {
        0.0
    }
}

/// The light a bloom raster's cell holds (`CasterBloom::build`): the mean of b over it, and its
/// centroid in the cell, in cells from its middle (-0.5..=0.5 each way).
#[derive(Clone, Copy, Debug, Default)]
struct CellLight {
    b: f32,
    x: f32,
    y: f32,
}

impl CellLight {
    /// Sums `b` at (x, y) cells from the middle (`mean` makes the sums a mean).
    fn add(&mut self, b: f32, x: f32, y: f32) {
        self.b += b;
        self.x += b * x;
        self.y += b * y;
    }

    /// The sums of `count` samples as their mean times `share`, and their centroid.
    fn mean(self, count: usize, share: f32) -> CellLight {
        if !(self.b > 0.0) {
            return CellLight::default();
        }
        CellLight { b: self.b / count as f32 * share, x: self.x / self.b, y: self.y / self.b }
    }
}

/// What a bloom raster's cell takes the light behind at (`CasterBloom::build`).
#[derive(Clone, Copy, Debug)]
enum Plan {
    /// Nothing: every corner well under the threshold.
    Dark,
    /// Its corners, read bilinearly: alike, its centre on their mean, or a light's cell that
    /// BLOOM_CELL_BUDGET left one sample.
    Corners,
    /// n x n samples, for a disc beside it (not held to BLOOM_CELL_BUDGET).
    Disc(usize),
    /// `want` x `want` samples, for a light that reaches it, or where its corners are not alike
    /// or its centre is off them. BLOOM_CELL_BUDGET never scales it under `keep` a side: what
    /// its corners may miss of the base (fix round 4 of S3's review), `fine` its detail
    /// (`peak`).
    Samples { want: usize, keep: usize, fine: bool },
    /// n x n samples, for the base's detail alone.
    Fine(usize),
}

/// How much of a cell centred at `x`, `y` with side `cell` the discs' squares cover, 0..=1
/// (overlapping squares counted once each; held to 1).
fn square_share(squares: &[(Vec2f, f32)], x: f32, y: f32, cell: f32) -> f32 {
    let overlap = |a: f32, c: f32, h: f32| ((a + 0.5 * cell).min(c + h) - (a - 0.5 * cell).max(c - h)).max(0.0);
    let covered: f32 = squares.iter().map(|&(c, h)| overlap(x, c.x, h) * overlap(y, c.y, h)).sum();
    (covered / (cell * cell)).min(1.0)
}

/// One caster's bloom (redesign §5): the share g (0..=1) of bright light behind it, blurred
/// over its bloom length. It lives on a raster in the caster's own tangent plane, covering
/// its box, its soft edge, the bloom length round them and two cells more, and, for the
/// bright discs in that plane (the sun, the moon), the lights no wider than a cell and a base's
/// few highlights, in points blurred where they are looked up (D6; fix rounds 2 and 3 of S3's
/// review). Built once per `Env` from what the map draws behind the caster
/// (`Env::content_before`) and the caster's own coverage; read unchanged by every lookup.
#[derive(Clone, Debug)]
pub(crate) struct CasterBloom {
    frame: LightFrame,
    /// Half the raster's extent in tangent units: cell (0, 0) has its corner at -ext.
    ext: Vec2f,
    /// A cell's side in tangent units.
    cell: f32,
    cols: usize,
    rows: usize,
    /// g per cell, row by row, row 0 at -ext.y: the raster's own share, without the points.
    g: Vec<f32>,
    /// The discs' light (`BloomPoint`), and the box width w of the kernel that blurs it,
    /// tangent units.
    points: Vec<BloomPoint>,
    width: f32,
    /// The box the points reach, tangent units: their own boxes widened by 1.5 w.
    points_min: Vec2f,
    points_max: Vec2f,
    /// The bright light the raster and the points hold, the sum of b x area (the tests).
    #[cfg(test)]
    energy: f64,
}

impl CasterBloom {
    /// The bloom of `caster`, which has none yet, for a map `width` wide.
    /// - b, per direction: how far above the threshold the light behind it is, in EV
    ///   (log2(L / 2^bloom_threshold_ev); 0 at and under the threshold and for NaN). It is
    ///   weighted by the share that shows past the caster's own pattern, 1 - m (its gaps,
    ///   and all of it round its edges), whatever its strength and tint. `behind(d)` is what
    ///   the map draws at `d` (map frame) before the caster, times 2^intensity_ev. `sharp(d)`
    ///   is the caster's analytic coverage, which its softness raster softens.
    /// - The raster is laid out for a map `width` wide, held to at least
    ///   CASTER_REFERENCE_WIDTH (D6), so the held and the released previews read one raster.
    /// - A disc of `hot` (map frame; radius in radians) that projects onto the caster's plane
    ///   is sampled on a grid of its own square there, fine enough for the disc and for the
    ///   kernel (BLOOM_DISC_GRID_*; where the grid is capped, each sample looks at the caster's
    ///   coverage over its step at that spacing), and kept as points spread over the part of
    ///   the square that shows. A light of `bright` (the lights listed before the caster, map
    ///   frame) no wider than BLOOM_LIGHT_POINT_CELLS cells is kept as points the same way, on
    ///   a grid as fine as its drawing asks (BLOOM_LIGHT_SAMPLES_PER_FEATURE).
    /// - A raster cell holds the mean of b over its area (fix round 2 of S3's review), placed
    ///   where it lies in the cell (`CellLight`, fix round 3): the light behind at the cells'
    ///   corners, read bilinearly between them where they are smooth (BLOOM_SMOOTH_EV) and the
    ///   cell's centre agrees (BLOOM_CENTRE_EV), at the spacing the raster's cells had before
    ///   they grew with the length where not (BLOOM_SAMPLES_PER_LENGTH), none where every corner
    ///   is well under the threshold (BLOOM_NEAR_EV); the caster's coverage at the map's texel.
    ///   A cell a wider light reaches takes the light behind at the spacing that light asks
    ///   for, and at the map's texel where the light's edge may cross it; a cell `peak` (the
    ///   base's detail: what it may draw within a radius of a direction, and the spacing it
    ///   asks for) may brighten past its corners takes the spacing that asks for, its light
    ///   kept as a point while such cells are few (BLOOM_FINE_POINTS_MAX); a cell a square
    ///   covers part of takes only the samples outside the squares, times the share outside
    ///   them; a cell a disc too near the plane's edge to project touches takes a grid fine
    ///   enough for the disc. All but the discs' cells are held to BLOOM_CELL_BUDGET, shared alike
    ///   by the map's `casters` blooming casters (M5), never under what each keeps (fix round 4).
    /// - S: the bright light within the bloom length, nearer counting more: b blurred by the
    ///   kernel of three boxes of width w = 2/3 of the bloom length, scaled so its peak is 1.
    ///   On the raster that is the kernel as it falls over each cell while it spans few cells
    ///   (`kernel_pass`, D6), else three box passes (summed-area tables). The points are
    ///   blurred by the kernel over their boxes (`spread`). A longer length only widens the
    ///   kernel, so S only grows with it.
    /// - g = 1 - exp(-S / BLOOM_REF_ENERGY); 0 below BLOOM_FLUSH.
    /// `run` spreads the raster's rows and each disc's grid rows (M5). Counted as
    /// probe::Work::CasterBloom. None when `cancelled` (polled between the runner's calls and
    /// per pass) or when the runner skipped a row.
    pub(crate) fn build(
        caster: &CasterLayer,
        width: usize,
        casters: usize,
        hot: &[(Vec3f, f32)],
        bright: &[BrightBox],
        peak: Option<&(dyn Fn(Vec3f, f32) -> Option<(f32, f32)> + Sync)>,
        behind: &(dyn Fn(Vec3f) -> Vec3f + Sync),
        sharp: &(dyn Fn(Vec3f) -> f32 + Sync),
        cancelled: &dyn Fn() -> bool,
        run: RowRunner,
    ) -> Option<CasterBloom> {
        #[cfg(test)]
        super::probe::count(super::probe::Work::CasterBloom);
        let p = &caster.params;
        // D6: no map below the reference width rasters its bloom more coarsely, so the held
        // preview's raster is the released one's; a wider map (an export) gets a finer one.
        let width = width.max(CASTER_REFERENCE_WIDTH);
        let len = p.bloom_length_deg.to_radians().tan();
        // What the coverage reaches: the box, widened by the softness blur (decision 10).
        let soft = p.softness * CASTER_SOFT_SCALE * caster.half.x.min(caster.half.y);
        let reach = vec2f(caster.half.x + soft + len, caster.half.y + soft + len);
        let texel = std::f32::consts::TAU / width.max(2) as f32;
        let side = (width / BLOOM_WIDTH_PER_SIDE).clamp(BLOOM_MIN_SIDE, CASTER_RASTER_MAX_SIDE);
        // Two cells of padding a side are added below, and up to one more to fill the grid, so
        // the reach takes side - 6 at most. The box and its soft edge take at most
        // BLOOM_BOX_SHARE of that, so a bloom that fits in the rest leaves the cell as it is.
        let room = (side - 6) as f32;
        let box_reach = caster.half.x.max(caster.half.y) + soft;
        let per_length = (BLOOM_CELLS_PER_LENGTH * width as f32 / CASTER_REFERENCE_WIDTH as f32).min(BLOOM_SAMPLES_PER_LENGTH);
        // The spacing a cell's light is sampled at: the cell as it was before the cells grew
        // with the length (fix round 2 of S3's review). The cell is a whole number of such
        // spacings or less.
        let sample_step = texel.max(len / BLOOM_SAMPLES_PER_LENGTH).max(2.0 * box_reach / (BLOOM_BOX_SHARE * room)).max(2.0 * reach.x.max(reach.y) / room);
        let cell = sample_step.max(len / per_length);
        let samples = ((cell / sample_step).ceil() as usize).clamp(1, BLOOM_CELL_SAMPLES_MAX);
        let looks = ((cell / texel).ceil() as usize).clamp(samples, BLOOM_COVERAGE_LOOKS_MAX.max(samples));
        // The cells lie on a grid round the caster's centre, an even count a side, so a longer
        // bloom of the same cell only adds cells at the border (D6: cells that moved with the
        // length sampled the pattern elsewhere, and a little more bloom could thin less).
        let (half_cols, half_rows) = (((reach.x + 2.0 * cell) / cell).ceil(), ((reach.y + 2.0 * cell) / cell).ceil());
        let ext = vec2f(half_cols * cell, half_rows * cell);
        let (cols, rows) = (2 * half_cols as usize, 2 * half_rows as usize);
        // The kernel: three box passes of a third of the length each way, a box `w` wide
        // (tangent units), `radius` cells each way.
        let radius = (len / (3.0 * cell)) as f64;
        let w = 2.0 * radius * cell as f64;
        let threshold = 2.0f32.powf(p.bloom_threshold_ev);
        let frame = caster.frame;
        let at = |x: f32, y: f32| (frame.center + frame.right * x + frame.up * y).normalize();
        // b at one direction: how far the light behind is over the threshold, in EV, times
        // the share that shows past the caster. The coverage first: behind a fully covering
        // part nothing shows, and the lookup behind is the slow part.
        let shows = |d: Vec3f| -> f32 {
            let m = caster.unbloomed(d, sharp);
            if !(m < 1.0) { 0.0 } else { 1.0 - m.max(0.0) }
        };
        // The share that shows over the box of half sides (hx, hy) round tangent point (x, y), in
        // one look for a soft caster; None for a sharp one, which is looked at point by point.
        let shows_over = |x: f32, y: f32, hx: f32, hy: f32| -> Option<f32> { caster.mean_unbloomed(vec2f(x, y), vec2f(hx, hy)).map(|m| 1.0 - m) };
        // How far the light behind is over the threshold, in EV, under it too (-inf for none or
        // NaN): the cells' corners keep how near they come (BLOOM_NEAR_EV).
        let ev_signed = |d: Vec3f| -> f32 {
            let lum = luminance(behind(d));
            if !(lum > 0.0) { f32::NEG_INFINITY } else { (lum.min(MAX_RADIANCE) / threshold).log2() }
        };
        let ev_behind = |d: Vec3f| -> f32 { ev_signed(d).max(0.0) };
        let ev_over = |d: Vec3f| -> f32 {
            let open = shows(d);
            if !(open > 0.0) { 0.0 } else { ev_behind(d) * open }
        };
        // Each disc. In the caster's tangent plane a disc is its centre's projection and its
        // reach there, the outward side of its radius, which the plane stretches most
        // (tan(theta + r) - tan(theta)): the square of that half-side round its centre holds
        // it. Only a disc too near the plane's edge to project (past PROJECT_MIN_COS) falls
        // back to an angle round its centre (its radius and a cell's half diagonal), and the
        // cells it touches take a grid fine enough for it.
        let touches: Vec<Touch> = hot
            .iter()
            .filter(|&&(c, r)| c.is_finite() && c.length() > 0.0 && r > 0.0)
            .map(|&(c, r)| {
                let c = c.normalize();
                let theta = c.dot(frame.center).clamp(-1.0, 1.0).acos();
                if (theta + r).cos() >= PROJECT_MIN_COS {
                    let centre = vec2f(c.dot(frame.right), c.dot(frame.up)) / theta.cos();
                    Touch::Plane(centre, (theta + r).tan() - theta.tan())
                } else {
                    let n = ((BLOOM_DISC_SAMPLES_PER_RADIUS * cell / r).ceil() as usize).clamp(2, BLOOM_DISC_SAMPLES_MAX);
                    Touch::Angle(c, (r + 0.75 * cell).min(std::f32::consts::PI).cos(), n)
                }
            })
            .collect();
        // Each light listed before the caster (`bright`): where it reaches in the caster's plane
        // (the box round its reach box's corners, which the plane keeps straight-sided, while
        // all of them project; else its cone, widened by a cell), and the spacing its drawing
        // asks for (BLOOM_LIGHT_SAMPLES_PER_FEATURE).
        let light_reach = |light: &BrightBox| -> (LightReach, f32) {
            let corners = light.frame_reach.and_then(|(f, r)| {
                [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                    .iter()
                    .map(|&(sx, sy)| project(&frame, (f.center + f.right * (sx * r.x) + f.up * (sy * r.y)).normalize()))
                    .collect::<Option<Vec<Vec2f>>>()
            });
            let reach = match corners {
                Some(q) => {
                    let lo = q.iter().fold(Vec2f::all(f32::INFINITY), |lo, q| vec2f(lo.x.min(q.x), lo.y.min(q.y)));
                    let hi = q.iter().fold(Vec2f::all(f32::NEG_INFINITY), |hi, q| vec2f(hi.x.max(q.x), hi.y.max(q.y)));
                    LightReach::Plane((lo + hi) * 0.5, (hi - lo) * 0.5)
                }
                None => LightReach::Angle(light.axis.normalize(), (light.cos_cover.clamp(-1.0, 1.0).acos() + 0.75 * cell).min(std::f32::consts::PI).cos()),
            };
            let feature = light.feature / BLOOM_LIGHT_SAMPLES_PER_FEATURE;
            (reach, if light.detailed { feature.min(texel) } else { feature })
        };
        // The squares that lie on the raster (a square off it reaches nothing the caster
        // covers), each sampled on a grid of its own into points: the discs', and the lights'
        // no wider than BLOOM_LIGHT_POINT_CELLS cells, whose grids are held to the spacing they
        // ask for (`square_steps`; none for a disc). Every other light is sampled on the raster,
        // in the cells it reaches.
        let on_raster = |c: Vec2f, h: f32| (c.x.abs() - h) < ext.x && (c.y.abs() - h) < ext.y;
        let (mut squares, mut square_steps): (Vec<(Vec2f, f32)>, Vec<f32>) = (Vec::new(), Vec::new());
        for touch in &touches {
            if let Touch::Plane(c, h) = *touch {
                if on_raster(c, h) {
                    squares.push((c, h));
                    square_steps.push(f32::INFINITY);
                }
            }
        }
        let mut raster_lights = Vec::new();
        for light in bright {
            match light_reach(light) {
                (LightReach::Plane(c, h), step) if h.x.max(h.y) <= BLOOM_LIGHT_POINT_CELLS * cell => {
                    if on_raster(c, h.x.max(h.y)) {
                        squares.push((c, h.x.max(h.y)));
                        square_steps.push(step);
                    }
                }
                other => raster_lights.push(other),
            }
        }
        let in_square = |earlier: &[(Vec2f, f32)], x: f32, y: f32| earlier.iter().any(|&(c, h)| (x - c.x).abs() < h && (y - c.y).abs() < h);
        let centre_of = |i: usize, j: usize| vec2f(-ext.x + (i as f32 + 0.5) * cell, -ext.y + (j as f32 + 0.5) * cell);
        // Whether a light on the raster reaches the cell centred at `c`.
        let light_reaches = |reach: &LightReach, c: Vec2f| match *reach {
            LightReach::Plane(m, h) => (c.x - m.x).abs() < 0.5 * cell + h.x && (c.y - m.y).abs() < 0.5 * cell + h.y,
            LightReach::Angle(axis, cos) => at(c.x, c.y).dot(axis) >= cos,
        };
        // The samples a side the cell centred at `c` takes the light behind at for the discs (a
        // square over part of it, or a disc that only an angle finds), 0 for none.
        let disc_fine = |c: Vec2f| -> usize {
            let near_square = squares.iter().any(|&(q, h)| (c.x - q.x).abs() < 0.5 * cell + h && (c.y - q.y).abs() < 0.5 * cell + h);
            let angle = touches.iter().any(|touch| matches!(touch, Touch::Angle(..)));
            let d = if angle { at(c.x, c.y) } else { Vec3f::default() };
            touches
                .iter()
                .filter_map(|touch| match *touch {
                    Touch::Angle(q, cos, n) if d.dot(q) >= cos => Some(n.max(samples)),
                    _ => None,
                })
                .chain(near_square.then_some(BLOOM_CELL_SAMPLES.max(samples)))
                .max()
                .unwrap_or(0)
        };
        // The mean of b over the cell centred at (x, y), and where in the cell it lies
        // (`CellLight`): the light behind at `n` x `n` samples (those outside the squares), each
        // times what shows over its own part of the cell at enough looks a side for `looks` over
        // the cell (none taken behind where nothing shows), times the share of the cell outside
        // the squares.
        let sampled = |x: f32, y: f32, n: usize| -> CellLight {
            let k = looks.div_ceil(n).max(1);
            let step = cell / n as f32;
            let (mut light, mut outside) = (CellLight::default(), 0usize);
            for sj in 0..n {
                let fy = (sj as f32 + 0.5) / n as f32 - 0.5;
                let sy = y + fy * cell;
                for si in 0..n {
                    let fx = (si as f32 + 0.5) / n as f32 - 0.5;
                    let sx = x + fx * cell;
                    if in_square(&squares, sx, sy) {
                        continue;
                    }
                    outside += 1;
                    let open = shows_over(sx, sy, 0.5 * step, 0.5 * step).unwrap_or_else(|| {
                        let mut open = 0.0f32;
                        for v in 0..k {
                            let ly = sy + ((v as f32 + 0.5) / k as f32 - 0.5) * step;
                            for u in 0..k {
                                open += shows(at(sx + ((u as f32 + 0.5) / k as f32 - 0.5) * step, ly));
                            }
                        }
                        open / (k * k) as f32
                    });
                    if open > 0.0 {
                        light.add(open * ev_behind(at(sx, sy)), fx, fy);
                    }
                }
            }
            if outside == 0 { CellLight::default() } else { light.mean(outside, 1.0 - square_share(&squares, x, y, cell)) }
        };
        // The light behind at the cells' corners, (cols + 1) x (rows + 1), in EV over the
        // threshold (`ev_signed`; none in a square, whose disc the points hold), a row at a time
        // where `run` puts it.
        let corner_row = |j: usize| -> Vec<f32> {
            (0..=cols)
                .map(|i| {
                    let (x, y) = (-ext.x + i as f32 * cell, -ext.y + j as f32 * cell);
                    if in_square(&squares, x, y) { f32::NEG_INFINITY } else { ev_signed(at(x, y)) }
                })
                .collect()
        };
        let corners = rows_by(rows + 1, run, cancelled, &corner_row)?;
        let corners_of = |i: usize, j: usize| [corners[j][i], corners[j][i + 1], corners[j + 1][i], corners[j + 1][i + 1]];
        let span = |e: &[f32; 4]| e.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        // Corners alike (BLOOM_SMOOTH_EV): all over the threshold and near each other.
        let alike = |(lo, hi): (f32, f32)| lo > 0.0 && hi - lo <= BLOOM_SMOOTH_EV;
        // Each light on the raster, with the samples a side a cell it reaches asks for where the
        // cell's corners are alike (at the spacing the light asks for) and where they are not
        // (its edge or its detail may cross the cell: at the map's texel too).
        let lights: Vec<(LightReach, usize, usize)> = raster_lights
            .into_iter()
            .map(|(reach, step)| {
                let wanted = if step > 0.0 { ((cell / step).ceil() as usize).clamp(1, BLOOM_LIGHT_SAMPLES_MAX) } else { BLOOM_LIGHT_SAMPLES_MAX };
                (reach, wanted.max(samples), wanted.max(looks).max(samples))
            })
            .collect();
        // What each cell takes (`Plan`), a row at a time where `run` puts it: the samples a disc
        // asks for; those a light that reaches it asks for, or the base's detail (`peak`) where
        // that may pass the threshold and the corners' brightest by more than BLOOM_SMOOTH_EV;
        // nothing where every corner is BLOOM_NEAR_EV or more under the threshold; its own
        // samples where its corners are not alike, or alike but its centre is off their mean;
        // else the corners, read bilinearly.
        let half_diagonal = std::f32::consts::FRAC_1_SQRT_2 * cell;
        let most = BLOOM_LIGHT_SAMPLES_MAX.max(samples);
        let plan_row = |j: usize| -> Vec<Plan> {
            (0..cols)
                .map(|i| {
                    let c = centre_of(i, j);
                    let n = disc_fine(c);
                    if n > 0 {
                        return Plan::Disc(n);
                    }
                    let e = corners_of(i, j);
                    let (lo, hi) = span(&e);
                    let smooth = alike((lo, hi));
                    let light = lights.iter().filter(|(reach, ..)| light_reaches(reach, c)).map(|&(_, inside, edge)| if smooth { inside } else { edge }).max().unwrap_or(0);
                    let fine = peak.and_then(|peak| peak(at(c.x, c.y), half_diagonal)).map_or(0, |(lum, step)| {
                        let over = if lum > 0.0 { (lum.min(MAX_RADIANCE) / threshold).log2() } else { f32::NEG_INFINITY };
                        if !(over > 0.0 && over > hi + BLOOM_SMOOTH_EV) {
                            0
                        } else if step > 0.0 {
                            ((cell / step).ceil() as usize).clamp(samples, most)
                        } else {
                            most
                        }
                    });
                    // A light's cell keeps the samples the base's detail asks for; the rest of
                    // what it asks for is the light's own, scaled with the light's other cells
                    // (keeping its dim parts near the threshold too took the budget from its
                    // bright ones, which then read their corners: a panel with blinds bloomed a
                    // quarter more light than it has).
                    if light > 0 {
                        return Plan::Samples { want: light.max(fine), keep: fine, fine: fine > 0 };
                    }
                    if fine > 0 {
                        return Plan::Fine(fine);
                    }
                    if !(hi > -BLOOM_NEAR_EV) {
                        return Plan::Dark;
                    }
                    let own = Plan::Samples { want: samples, keep: samples, fine: false };
                    if !smooth {
                        return own;
                    }
                    let mean = 0.25 * (e[0] + e[1] + e[2] + e[3]);
                    if (ev_signed(at(c.x, c.y)) - mean).abs() <= BLOOM_CENTRE_EV { Plan::Corners } else { own }
                })
                .collect()
        };
        let mut plans: Vec<Plan> = rows_by(rows, run, cancelled, &plan_row)?.concat();
        // M5 (fix rounds 3 and 4 of S3's review): where the cells ask for more than `budget`, the
        // samples a side of each are scaled alike, so that those past what the cells keep come to
        // the rest of it, but never under what each keeps: the base's detail all it asks for
        // while few cells have any (BLOOM_FINE_POINTS_MAX), else its cell's own samples; a cell
        // no light reaches that is near the threshold or whose centre is off its corners, its own
        // samples. A light's cell left under two a side reads its corners, which hold its light
        // already.
        let budget = BLOOM_CELL_BUDGET * (width / CASTER_REFERENCE_WIDTH).pow(2) / casters.max(1);
        let few = plans.iter().filter(|plan| matches!(plan, Plan::Fine(_) | Plan::Samples { fine: true, .. })).count() <= BLOOM_FINE_POINTS_MAX;
        let keep_of = |plan: &Plan| match *plan {
            Plan::Samples { keep, fine: true, .. } | Plan::Fine(keep) if !few => keep.min(samples),
            Plan::Samples { keep, .. } | Plan::Fine(keep) => keep,
            _ => 0,
        };
        let demand: usize = plans.iter().map(|plan| match *plan { Plan::Samples { want: n, .. } | Plan::Fine(n) => n * n, _ => 0 }).sum();
        if demand > budget {
            let kept: usize = plans.iter().map(|plan| keep_of(plan).pow(2)).sum();
            let scale = (budget.saturating_sub(kept) as f32 / demand as f32).sqrt();
            for plan in &mut plans {
                let keep = keep_of(plan);
                *plan = match *plan {
                    Plan::Samples { want, fine, .. } => {
                        let n = ((want as f32 * scale) as usize).max(keep);
                        // A single sample is about what the corners hold already: they are read.
                        if n < 2 && keep == 0 { Plan::Corners } else { Plan::Samples { want: n, keep: n, fine } }
                    }
                    Plan::Fine(n) => Plan::Fine(((n as f32 * scale) as usize).max(keep)),
                    other => other,
                };
            }
        }
        // The cells only the base's detail asks to sample, kept as points while they are few
        // (BLOOM_FINE_POINTS_MAX); else rastered as the rest.
        let pointed: Vec<usize> = {
            let few: Vec<usize> = plans.iter().enumerate().filter(|(_, plan)| matches!(plan, Plan::Fine(_))).map(|(k, _)| k).take(BLOOM_FINE_POINTS_MAX + 1).collect();
            if few.len() <= BLOOM_FINE_POINTS_MAX { few } else { Vec::new() }
        };
        let as_points = !pointed.is_empty();
        // b on the raster, a row at a time where `run` puts it.
        let raster_row = |j: usize| -> Vec<CellLight> {
            (0..cols)
                .map(|i| {
                    let c = centre_of(i, j);
                    match plans[j * cols + i] {
                        Plan::Dark => return CellLight::default(),
                        Plan::Fine(_) if as_points => return CellLight::default(),
                        Plan::Disc(n) | Plan::Samples { want: n, .. } | Plan::Fine(n) => return sampled(c.x, c.y, n),
                        Plan::Corners => {}
                    }
                    // The light behind bilinearly between the corners (none at those under the
                    // threshold: a cell BLOOM_CELL_BUDGET left one sample may have some), times
                    // what shows at each look (a soft caster's coverage over each quarter of the
                    // cell, in one look).
                    let e = corners_of(i, j).map(|v| v.max(0.0));
                    let bilinear = |u: f32, v: f32| (e[0] * (1.0 - u) + e[1] * u) * (1.0 - v) + (e[2] * (1.0 - u) + e[3] * u) * v;
                    let mut light = CellLight::default();
                    if caster.soft.is_some() {
                        for (u, v) in [(0.25f32, 0.25f32), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                            let open = shows_over(c.x + (u - 0.5) * cell, c.y + (v - 0.5) * cell, 0.25 * cell, 0.25 * cell).unwrap_or(0.0);
                            if open > 0.0 {
                                light.add(open * bilinear(u, v), u - 0.5, v - 0.5);
                            }
                        }
                        return light.mean(4, 1.0);
                    }
                    for lj in 0..looks {
                        let v = (lj as f32 + 0.5) / looks as f32;
                        for li in 0..looks {
                            let u = (li as f32 + 0.5) / looks as f32;
                            let open = shows(at(c.x + (u - 0.5) * cell, c.y + (v - 0.5) * cell));
                            if open > 0.0 {
                                light.add(open * bilinear(u, v), u - 0.5, v - 0.5);
                            }
                        }
                    }
                    light.mean(looks * looks, 1.0)
                })
                .collect()
        };
        let lights_by_cell: Vec<CellLight> = rows_by(rows, run, cancelled, &raster_row)?.concat();
        // Each cell's light where it lies in the cell (fix round 3 of S3's review: a light that
        // fills part of a cell bloomed from the cell's middle), split between the cell and the
        // ones beside it towards its centroid, bilinearly, so the raster keeps its centroid; a
        // cell at the raster's border keeps what would leave it.
        let mut b = vec![0.0f32; cols * rows];
        for (index, light) in lights_by_cell.iter().enumerate() {
            if !(light.b > 0.0) {
                continue;
            }
            let (i, j) = (index % cols, index / cols);
            let toward = |at: usize, f: f32, count: usize| -> (usize, f32) {
                let next = if f < 0.0 { at.checked_sub(1) } else { Some(at + 1).filter(|&n| n < count) };
                next.map_or((at, 0.0), |n| (n, f.abs().min(0.5)))
            };
            let ((ni, ax), (nj, ay)) = (toward(i, light.x, cols), toward(j, light.y, rows));
            b[j * cols + i] += light.b * (1.0 - ax) * (1.0 - ay);
            b[j * cols + ni] += light.b * ax * (1.0 - ay);
            b[nj * cols + i] += light.b * (1.0 - ax) * ay;
            b[nj * cols + ni] += light.b * ax * ay;
        }
        // The squares, each on its own grid (a look in an earlier square is that one's), into
        // points merged to about half the kernel's width apart.
        let mut points = Vec::new();
        #[cfg(test)]
        let mut point_energy = 0.0f64;
        for (k, &(c, h)) in squares.iter().enumerate() {
            let spacing = (h / BLOOM_DISC_GRID_PER_REACH).min(w as f32 / BLOOM_DISC_GRID_PER_WIDTH).min(square_steps[k]);
            let wanted = ((2.0 * h / spacing).ceil() as usize).max(1);
            let n = wanted.clamp(2, BLOOM_DISC_GRID_MAX);
            // Where the grid is capped, a sample's step is wider than the spacing asked: it looks
            // at the caster's coverage `sub` x `sub` times over its step (and behind it once, at
            // its centre), so an edge across the step is placed to a `sub`th of it.
            let sub = wanted.div_ceil(n).clamp(1, BLOOM_DISC_SUBSAMPLES_MAX);
            let groups = ((2.0 * h * BLOOM_DISC_GRID_PER_WIDTH / w as f32).ceil() as usize).clamp(1, n);
            let step = 2.0 * h / n as f32;
            let q = step / sub as f32;
            let area = (q as f64) * (q as f64);
            // The light of each sample of the grid, a row at a time where `run` puts it.
            let place = |s: usize, centre: f32| centre - h + (s as f32 + 0.5) * step;
            let grid_row = |sj: usize| -> Vec<Moments> {
                let sy = place(sj, c.y);
                (0..n)
                    .map(|si| {
                        let sx = place(si, c.x);
                        let mut light = Moments::default();
                        if sub == 1 {
                            if !in_square(&squares[..k], sx, sy) {
                                light.add(ev_over(at(sx, sy)) as f64 * area, sx - c.x, sy - c.y, q);
                            }
                            return light;
                        }
                        for v in 0..sub {
                            let y = sy + ((v as f32 + 0.5) / sub as f32 - 0.5) * step;
                            for u in 0..sub {
                                let x = sx + ((u as f32 + 0.5) / sub as f32 - 0.5) * step;
                                if !in_square(&squares[..k], x, y) {
                                    light.add(shows(at(x, y)) as f64 * area, x - c.x, y - c.y, q);
                                }
                            }
                        }
                        if light.mass > 0.0 { light.scaled(ev_behind(at(sx, sy)) as f64) } else { light }
                    })
                    .collect()
            };
            // A grid of few looks is not worth a call of the runner.
            let grid_run: RowRunner = if n * n * sub * sub <= BLOOM_DISC_SERIAL_LOOKS { &serial_rows } else { run };
            let values = rows_by(n, grid_run, cancelled, &grid_row)?;
            let mut sums = vec![Moments::default(); groups * groups];
            for (sj, row) in values.iter().enumerate() {
                for (si, light) in row.iter().enumerate() {
                    if light.mass > 0.0 {
                        sums[(sj * groups / n) * groups + si * groups / n].merge(light);
                    }
                }
            }
            for light in &sums {
                if light.mass / BLOOM_REF_ENERGY > BLOOM_FLUSH {
                    #[cfg(test)]
                    {
                        point_energy += light.mass;
                    }
                    points.push(light.point(c));
                }
            }
        }
        // The few cells the base's detail alone asks for, each the point of its light: n x n
        // samples of the light behind, each times what shows over its own part of the cell.
        let cell_light = |t: usize| -> Moments {
            let (i, j) = (pointed[t] % cols, pointed[t] / cols);
            let Plan::Fine(n) = plans[pointed[t]] else { return Moments::default() };
            let k = looks.div_ceil(n).max(1);
            let (step, sub) = (cell / n as f32, cell / (n * k) as f32);
            let area = (sub as f64) * (sub as f64);
            let c = centre_of(i, j);
            let mut light = Moments::default();
            for sj in 0..n {
                let sy = (sj as f32 + 0.5) * step - 0.5 * cell;
                for si in 0..n {
                    let sx = (si as f32 + 0.5) * step - 0.5 * cell;
                    let mut sample = Moments::default();
                    for v in 0..k {
                        let y = sy + ((v as f32 + 0.5) / k as f32 - 0.5) * step;
                        for u in 0..k {
                            let x = sx + ((u as f32 + 0.5) / k as f32 - 0.5) * step;
                            sample.add(shows(at(c.x + x, c.y + y)) as f64 * area, x, y, sub);
                        }
                    }
                    if sample.mass > 0.0 {
                        light.merge(&sample.scaled(ev_behind(at(c.x + sx, c.y + sy)) as f64));
                    }
                }
            }
            light
        };
        let pointed_lights = if pointed.is_empty() { Vec::new() } else { rows_by(pointed.len(), run, cancelled, &cell_light)? };
        for (t, light) in pointed_lights.into_iter().enumerate() {
            if light.mass / BLOOM_REF_ENERGY > BLOOM_FLUSH {
                #[cfg(test)]
                {
                    point_energy += light.mass;
                }
                points.push(light.point(centre_of(pointed[t] % cols, pointed[t] / cols)));
            }
        }
        #[cfg(test)]
        let energy = b.iter().map(|&v| v as f64).sum::<f64>() * cell as f64 * cell as f64 + point_energy;
        // The kernel, three boxes of a third of the length: it reaches exactly the length. While
        // it spans few cells (D6: a box no wider than a cell, at the cells' centres, is the cell
        // itself, so three such passes spread nothing), the cells are blurred by the kernel
        // itself, as it falls over each cell (`kernel_pass`); a longer one takes three box
        // passes. The cells within two of the raster's border are never read for a share where
        // the caster covers, so the edge held there does no harm.
        let mut grid = b;
        if grid.iter().any(|&v| v > 0.0) {
            if cancelled() {
                return None;
            }
            if kernel_taps(2.0 * radius) <= BLOOM_KERNEL_TAPS {
                grid = kernel_pass(&grid, cols, rows, 2.0 * radius);
            } else {
                for _ in 0..3 {
                    if cancelled() {
                        return None;
                    }
                    grid = box_mean_pass(&grid, cols, rows, radius);
                }
            }
        }
        // Three boxes of side w sum w^6 x their means; their kernel's peak is (3 w² / 4)². So
        // S, the blur with a kernel of peak 1, is (16 / 9) w² x the triple mean, w in tangent
        // units.
        let per_mean = 16.0 / 9.0 * w * w / BLOOM_REF_ENERGY;
        let g = grid
            .iter()
            .map(|&mean| {
                let g = 1.0 - (-(mean as f64) * per_mean).exp();
                if g > BLOOM_FLUSH { g.min(1.0) as f32 } else { 0.0 }
            })
            .collect();
        let w = w as f32;
        let (mut points_min, mut points_max) = (Vec2f::all(f32::INFINITY), Vec2f::all(f32::NEG_INFINITY));
        for point in &points {
            let reach = Vec2f::all(1.5 * w) + point.half;
            points_min = vec2f(points_min.x.min(point.at.x - reach.x), points_min.y.min(point.at.y - reach.y));
            points_max = vec2f(points_max.x.max(point.at.x + reach.x), points_max.y.max(point.at.y + reach.y));
        }
        Some(CasterBloom {
            frame,
            ext,
            cell,
            cols,
            rows,
            g,
            points,
            width: w,
            points_min,
            points_max,
            #[cfg(test)]
            energy,
        })
    }

    /// g at `dir` (map frame): the raster read bilinearly between cell centres, with the
    /// points' S added to its own, 1 - (1 - g) exp(-S_points / BLOOM_REF_ENERGY); 0 off the
    /// raster, and exactly 0 where its four cells are and no point reaches.
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
        let g = (top + (bottom - top) * ty).clamp(0.0, 1.0);
        let s = self.points_at(p);
        if s > 0.0 { (1.0 - (1.0 - g) * (-s).exp()).clamp(0.0, 1.0) } else { g }
    }

    /// The points' S / BLOOM_REF_ENERGY at tangent point `p`: each one's weight times the
    /// kernel over its box (`spread` along both axes, w wide). 0 past their reach.
    fn points_at(&self, p: Vec2f) -> f32 {
        if !(p.x > self.points_min.x && p.y > self.points_min.y && p.x < self.points_max.x && p.y < self.points_max.y) {
            return 0.0;
        }
        let (w, reach) = (self.width, 1.5 * self.width);
        let mut s = 0.0;
        for point in &self.points {
            let (dx, dy) = (p.x - point.at.x, p.y - point.at.y);
            if dx.abs() < reach + point.half.x && dy.abs() < reach + point.half.y {
                s += point.weight * spread(dx, point.half.x, w) * spread(dy, point.half.y, w);
            }
        }
        s
    }

    /// Whether any cell or point thins. False when nothing behind the caster passes its
    /// threshold; the caster then keeps no raster and draws exactly as it does without bloom.
    pub(crate) fn thins_anything(&self) -> bool {
        !self.points.is_empty() || self.g.iter().any(|&g| g > 0.0)
    }

    /// The bright light the raster and the points hold, the sum of b x area, in EV x tangent
    /// units².
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

/// The share of the bloom's kernel along one axis (three boxes of width 1, `spline`'s profile,
/// its integral 1) that lies before `t` box widths from its centre.
fn spline_cdf(t: f64) -> f64 {
    if t <= -1.5 {
        0.0
    } else if t <= -0.5 {
        (1.5 + t).powi(3) / 6.0
    } else if t <= 0.5 {
        0.5 + 0.75 * t - t * t * t / 3.0
    } else if t < 1.5 {
        1.0 - (1.5 - t).powi(3) / 6.0
    } else {
        1.0
    }
}

/// The cells either way a kernel of boxes `width` cells wide reaches from a cell's centre.
fn kernel_taps(width: f64) -> usize {
    (1.5 * width + 0.5).ceil() as usize
}

/// `grid` (cols x rows, row by row) blurred by the bloom's kernel, three boxes `width` cells
/// wide, exactly as it falls over each cell: every cell becomes the kernel's mean of the cells
/// round its centre, each weighed by the kernel's share over it, along each axis in turn (the
/// kernel is the product of the two). Edge cells are held past the border, as `box_mean_pass`
/// holds them. The weights are those of the continuous kernel, so a wider one weighs every
/// cell at least as much relative to its peak.
fn kernel_pass(grid: &[f32], cols: usize, rows: usize, width: f64) -> Vec<f32> {
    let taps = kernel_taps(width) as isize;
    let weights: Vec<f64> = (-taps..=taps).map(|d| spline_cdf((d as f64 + 0.5) / width) - spline_cdf((d as f64 - 0.5) / width)).collect();
    let (len, reach) = (weights.len(), taps as usize);
    // Along each row (a row wholly of zeros stays zeros: M5, most of a raster holds no light),
    // then down each column, a row of sums at a time; each cell's taps summed in the same order.
    let mut across = vec![0.0f32; cols * rows];
    for j in 0..rows {
        let row = &grid[j * cols..(j + 1) * cols];
        if row.iter().all(|&v| v == 0.0) {
            continue;
        }
        for i in 0..cols {
            let mut sum = 0.0f64;
            if i >= reach && i + reach < cols {
                for (&weight, &v) in weights.iter().zip(&row[i - reach..i - reach + len]) {
                    sum += weight * v as f64;
                }
            } else {
                for (k, &weight) in weights.iter().enumerate() {
                    sum += weight * row[(i as isize + k as isize - taps).clamp(0, cols as isize - 1) as usize] as f64;
                }
            }
            across[j * cols + i] = sum.max(0.0) as f32;
        }
    }
    let mut out = vec![0.0f32; cols * rows];
    let mut sums = vec![0.0f64; cols];
    for j in 0..rows {
        sums.iter_mut().for_each(|sum| *sum = 0.0);
        for (k, &weight) in weights.iter().enumerate() {
            let y = (j as isize + k as isize - taps).clamp(0, rows as isize - 1) as usize;
            for (sum, &v) in sums.iter_mut().zip(&across[y * cols..(y + 1) * cols]) {
                *sum += weight * v as f64;
            }
        }
        for (cell, &sum) in out[j * cols..(j + 1) * cols].iter_mut().zip(&sums) {
            *cell = sum.max(0.0) as f32;
        }
    }
    out
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
                assert!(CasterLayer::new(&c, &images, CASTER_REFERENCE_WIDTH, &serial_rows).is_none(), "{name:?}: no layer");
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
    /// the studio draws it black at full strength for a map `width` wide, on
    /// an n x n midpoint grid of its tangent plane (where a box filter keeps
    /// area).
    fn mean_coverage(c: &LightParams, pad: f32, n: usize, width: usize) -> f64 {
        let black = LightParams { caster: Some(CasterParams { strength: 1.0, tint: [0.0; 3], ..c.caster_params() }), ..c.clone() };
        let studio = Studio::new_with(&StudioParams::default(), &[black], &crate::hdri::ImageSet::default(), false, width, &serial_rows);
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
    /// a soft caster covers what the sharp one does, to within 2.5 %; in the
    /// held preview (512) too, whose raster's cells are a little coarser
    /// (SOFT_HELD_TEXELS_PER_CELL, M5).
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
            let sharp = mean_coverage(&sharp_caster, pad, 600, CASTER_REFERENCE_WIDTH);
            assert!(sharp > 0.05 && sharp < 0.95, "{name}: {sharp}");
            for width in [CASTER_REFERENCE_WIDTH, 512] {
                for softness in [0.3f32, 1.0] {
                    let blurred = mean_coverage(&LightParams { caster: Some(soft(softness)), ..sharp_caster.clone() }, pad, 300, width);
                    println!("{name} at softness {softness}, {width} wide: sharp {sharp:.4}, soft {blurred:.4}");
                    assert!((sharp - blurred).abs() < 0.025 * sharp, "{name} at softness {softness}, {width} wide: sharp {sharp}, soft {blurred}");
                }
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
            let layer = CasterLayer::new(&c, &images, CASTER_REFERENCE_WIDTH, &serial_rows).expect("a caster that draws");
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
    /// spot shows and the share is the bloom's alone. The spot is handed to the pre-pass as a
    /// light listed before the caster is (`BrightBox`), so the cells it may reach are sampled
    /// finely enough for it, however it falls between the cells' corners.
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
        let caster = CasterLayer::new(&light, &ImageSet::default(), CASTER_REFERENCE_WIDTH, &serial_rows).expect("a solid caster draws");
        let spot = dir_from_az_el(3.0, 20.0);
        let cos_spot = 0.5f32.to_radians().cos();
        let behind = |d: Vec3f| if d.dot(spot) >= cos_spot { Vec3f::all(16384.0) } else { Vec3f::all(0.5) };
        let open = |_: Vec3f| 0.0;
        let bright = [BrightBox { axis: spot, cos_cover: cos_spot, frame_reach: None, feature: 1f32.to_radians().tan(), detailed: false }];
        let bloom = CasterBloom::build(&caster, 1024, 1, &[], &bright, None, &behind, &open, &|| false, &serial_rows).expect("nothing cancels it");
        assert!(bloom.thins_anything(), "the spot is bright");
        assert!(bloom.share(spot) > 0.5, "on the spot: {}", bloom.share(spot));
        // 10 degrees from the spot: past 6 degrees of bloom, the spot's half degree and three cells.
        for (az, el) in [(13.0, 20.0), (-7.0, 20.0), (3.0, 30.0), (3.0, 10.0), (-15.0, 32.0)] {
            assert_eq!(bloom.share(dir_from_az_el(az, el)), 0.0, "at azimuth {az}, elevation {el}");
        }
        assert!(CasterBloom::build(&caster, 1024, 1, &[], &bright, None, &behind, &open, &|| true, &serial_rows).is_none(), "a cancelled pre-pass stops");
    }

    /// The default sky's sun at azimuth 200, 30 degrees up, and a solid 40 x 30 degree caster
    /// whose bottom edge runs through the sun's centre (the caster's centre is 15 degrees above
    /// it): half the disc shows below the edge, and a bloom `length_deg` long at strength 1
    /// thins the edge over it.
    fn sun_at_the_edge(length_deg: f32) -> HdriParams {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.azimuth_deg = 200.0;
        p.sky.sun.elevation_deg = 30.0;
        p.lights = vec![caster_at(200.0, 45.0, 40.0, 30.0, None, CasterParams { bloom_strength: 1.0, bloom_length_deg: length_deg, ..CasterParams::default() })];
        p
    }

    /// `sun_at_the_edge(length_deg)` for a preview `width` wide (`Env::new_for_width`).
    fn edge_env(length_deg: f32, width: usize) -> Env {
        Env::new_for_width(&sun_at_the_edge(length_deg), &ImageSet::default(), width, &|| false).expect("nothing cancels it")
    }

    /// The coverage the first caster's bloom takes away round the sun (`sun_at_the_edge`), in
    /// steradians (`thinned`): in the tangent plane at the sun, from a degree under it to the
    /// bloom length and 3 degrees more above it and either side.
    fn thinned_round_the_sun(env: &Env, length_deg: f32) -> f64 {
        let reach = (length_deg + 3.0).to_radians().tan();
        thinned(env, 0, light_frame(200.0, 30.0, 0.0), (-reach, reach), (-1f32.to_radians().tan(), reach))
    }

    /// The coverage the bloom of the caster at studio entry `entry` takes away over `xs` x `ys`
    /// of the tangent plane of `frame`, in steradians: the sum of m - m' (its coverage without
    /// bloom less its coverage with it) over a 240 x 240 grid, each point weighted by its solid
    /// angle. 0 when the caster kept no bloom; bloom never thickens it.
    fn thinned(env: &Env, entry: usize, frame: LightFrame, xs: (f32, f32), ys: (f32, f32)) -> f64 {
        let caster = env.studio.caster(entry).expect("the caster draws");
        if caster.bloom().is_none() {
            return 0.0;
        }
        let sharp = |d: Vec3f| env.studio.caster_sharp(entry, d);
        let n = 240;
        let (dx, dy) = ((xs.1 - xs.0) / n as f32, (ys.1 - ys.0) / n as f32);
        let mut sum = 0.0f64;
        for j in 0..n {
            let y = ys.0 + (j as f32 + 0.5) * dy;
            for i in 0..n {
                let x = xs.0 + (i as f32 + 0.5) * dx;
                let d = (frame.center + frame.right * x + frame.up * y).normalize();
                let thinned = caster.unbloomed(d, &sharp) - caster.coverage(d, &sharp);
                assert!(thinned >= 0.0, "bloom thickened the caster at ({x}, {y}) by {}", -thinned);
                // A tangent-plane element dx dy is dx dy / (1 + x² + y²)^(3/2) steradians.
                sum += thinned as f64 * (dx * dy) as f64 / ((1.0 + x * x + y * y) as f64).powf(1.5);
            }
        }
        sum
    }

    /// A 10 degree softbox at EV 13 (3 EV over the default threshold) behind the middle of a
    /// 40 x 30 degree leaves caster due north, 25 degrees up, with a bloom `length_deg` long at
    /// strength 1, in a grey studio: an extended bright light, which the raster carries.
    fn softbox_behind_leaves(length_deg: f32) -> HdriParams {
        let light = LightParams { width_deg: 10.0, height_deg: 10.0, softness: 0.2, intensity_ev: 13.0, ..softbox(0.0, 25.0) };
        let caster = caster_at(0.0, 25.0, 40.0, 30.0, Some(leaves(0.6, 3)), CasterParams { bloom_strength: 1.0, bloom_length_deg: length_deg, ..CasterParams::default() });
        grey_studio(vec![light, caster])
    }

    /// The coverage the caster of `softbox_behind_leaves` loses to its bloom, in steradians
    /// (`thinned`): over the softbox and the bloom length and 2 degrees more round it.
    fn thinned_round_the_softbox(env: &Env, length_deg: f32) -> f64 {
        let reach = (length_deg + 7.0).to_radians().tan();
        thinned(env, 1, light_frame(0.0, 25.0, 0.0), (-reach, reach), (-reach, reach))
    }

    /// The two D6 scenes at `length_deg` for a preview `width` wide: the coverage the bloom takes
    /// off the caster round the sun (whose disc is sampled on a grid of its own) and round the
    /// softbox (which the raster carries), and the cell of each one's raster.
    fn d6_scenes(length_deg: f32, width: usize) -> [(&'static str, f64, f32); 2] {
        let cell = |env: &Env, entry: usize| env.studio.caster(entry).and_then(|c| c.bloom()).map_or(f32::NAN, |b| b.cell());
        let sun = edge_env(length_deg, width);
        let softbox = Env::new_for_width(&softbox_behind_leaves(length_deg), &ImageSet::default(), width, &|| false).expect("nothing cancels it");
        [("the sun", thinned_round_the_sun(&sun, length_deg), cell(&sun, 0)), ("a softbox", thinned_round_the_softbox(&softbox, length_deg), cell(&softbox, 1))]
    }

    /// D6 (plan 4's final review): a bloom shorter than a raster cell still acts, and every
    /// length past it thins more, round the sun and round a softbox, in the held 512 preview.
    /// Its rasters' cells here are about two thirds of a degree (the released preview's, which
    /// the held one shares); a kernel of a third of the length each way was held to half a cell,
    /// so every length under about 1.5 cells thinned as much as every other (or, as the
    /// raster's cells moved with the length, less).
    #[test]
    fn a_bloom_shorter_than_a_raster_cell_still_thins_and_grows_with_its_length() {
        let lengths = [0.1f32, 0.25, 0.5, 1.0, 1.5, 2.0, 3.0];
        let by_length: Vec<[(&str, f64, f32); 2]> = lengths.iter().map(|&l| d6_scenes(l, 512)).collect();
        for scene in 0..2 {
            let name = by_length[0][scene].0;
            let thinned: Vec<f64> = by_length.iter().map(|s| s[scene].1).collect();
            println!("{name}: thinned at 512 by length {lengths:?}: {thinned:?}");
            for (&length_deg, s) in lengths.iter().zip(&by_length) {
                if length_deg <= 0.5 {
                    assert!(s[scene].2 > length_deg.to_radians().tan(), "{name}: the premise, {length_deg} degrees is under a cell ({})", s[scene].2);
                }
            }
            assert!(thinned[0] > 0.0, "{name}: a bloom of a tenth of a degree thins nothing");
            for (w, l) in thinned.windows(2).zip(lengths.windows(2)) {
                assert!(w[1] > w[0] * 1.01, "{name}: {} degrees of bloom thin {:.4e} sr, {} degrees {:.4e} sr", l[0], w[0], l[1], w[1]);
            }
        }
    }

    /// D6: the held (512) and the released (1024) previews thin a caster alike, from a tenth of
    /// a degree of bloom to 20 degrees, within HELD_RELEASED of the released thinning: round the
    /// sun, whose disc both sample on the same grid of its own, and round a softbox, which the
    /// raster carries (both previews raster it at the released preview's cells).
    #[test]
    fn held_and_released_previews_bloom_alike() {
        let mut off = Vec::new();
        for length_deg in [0.1f32, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0] {
            for (held, released) in d6_scenes(length_deg, 512).into_iter().zip(d6_scenes(length_deg, 1024)) {
                let (name, held, released) = (held.0, held.1, released.1);
                println!("{name:9} {length_deg:5.2} degrees: held {held:.4e} sr, released {released:.4e} sr, {:+.2} %", 100.0 * (held / released - 1.0));
                assert!(released > 0.0, "{name}, {length_deg} degrees: the released preview thins nothing");
                if !((held - released).abs() <= HELD_RELEASED * released) {
                    off.push(format!("{name} at {length_deg} degrees: held {held:.3e} sr, released {released:.3e} sr, {:+.1} %", 100.0 * (held / released - 1.0)));
                }
            }
        }
        assert!(off.is_empty(), "the held and released previews differ by more than {} %: {}", 100.0 * HELD_RELEASED, off.join(", "));
    }

    /// How far the held and released previews' bloom may differ (`held_and_released_previews_bloom_alike`):
    /// 2 % of the released thinning, round a disc and round light the raster carries alike.
    const HELD_RELEASED: f64 = 0.02;

    /// D6 (plan 4's fix wave, review of S3): a disc far wider than its bloom is long (a 20
    /// degree sun at softness 0.2, 11 degrees out to its soft edge, behind a 0.5 degree bloom)
    /// asks for more grid steps across it than BLOOM_DISC_GRID_MAX allows, so its grid is 1.6
    /// degrees apart and the kernel reaches 0.5. It still thins the solid 40 x 30 degree caster
    /// whose bottom edge crosses it in a band along that edge, not in beads at its samples,
    /// wherever the edge falls between them: at nine places of the disc a fifth of a degree
    /// apart (a grid step in all), along a line 0.15 degrees inside the edge, over 5 degrees
    /// either side of the disc's centre, the share never falls under half its most, and the
    /// line's mean share never under a quarter of its most at any place. (The edge is placed to
    /// the spacing a grid asks for, half the kernel's box width, as round a disc whose grid is
    /// not capped, so this far into the kernel's tail the band's share still moves with it by
    /// up to about 2.5 times: 0.15 to 0.36 here, 0.15 to 0.42 round a 0.6 degree disc.)
    #[test]
    fn a_large_disc_at_a_casters_edge_thins_it_in_a_band_not_in_beads() {
        use crate::hdri::{dir_from_az_el, ImageSet};
        let light = caster_at(0.0, 20.0, 40.0, 30.0, None, CasterParams { bloom_strength: 1.0, bloom_length_deg: 0.5, ..CasterParams::default() });
        let caster = CasterLayer::new(&light, &ImageSet::default(), CASTER_REFERENCE_WIDTH, &serial_rows).expect("a solid caster draws");
        let (frame, half) = (caster.frame, caster.half);
        let solid = |d: Vec3f| match project(&frame, d) {
            Some(t) if t.x.abs() <= half.x && t.y.abs() <= half.y => 1.0,
            _ => 0.0,
        };
        let outer = 11f32.to_radians();
        let line_y = -half.y + 0.15f32.to_radians();
        let (mut means, mut beads) = (Vec::new(), Vec::new());
        for k in 0..9 {
            // The disc's centre on the caster's bottom edge (5 degrees up), then lower.
            let sun = dir_from_az_el(0.0, 5.0 - 0.2 * k as f32);
            let behind = |d: Vec3f| if d.dot(sun) >= outer.cos() { Vec3f::all(16384.0) } else { Vec3f::all(0.5) };
            let bloom = CasterBloom::build(&caster, CASTER_REFERENCE_WIDTH, 1, &[(sun, outer)], &[], None, &behind, &solid, &|| false, &serial_rows).expect("nothing cancels it");
            let reach = 5f32.to_radians().tan();
            let shares: Vec<f32> = (0..=200)
                .map(|i| {
                    let x = -reach + 2.0 * reach * i as f32 / 200.0;
                    bloom.share((frame.center + frame.right * x + frame.up * line_y).normalize())
                })
                .collect();
            let (min, max) = shares.iter().fold((f32::INFINITY, 0.0f32), |(lo, hi), &s| (lo.min(s), hi.max(s)));
            let mean = shares.iter().map(|&s| s as f64).sum::<f64>() / shares.len() as f64;
            println!("disc {:.1} degrees under the edge: share along the edge {min:.4} to {max:.4}, mean {mean:.4}", 0.2 * k as f32);
            if !(max > 0.0 && min >= 0.5 * max) {
                beads.push(format!("{:.1} degrees under the edge: {min:.4} to {max:.4}", 0.2 * k as f32));
            }
            means.push(mean);
        }
        assert!(beads.is_empty(), "the share 0.15 degrees inside the edge is not a band (none, or falling under half its most between samples): {}", beads.join(", "));
        let (lo, hi) = means.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), &m| (lo.min(m), hi.max(m)));
        assert!(lo >= 0.25 * hi, "the band's mean share falls from {hi:.4} to {lo:.4} as the disc moves: {means:?}");
    }

    /// The bright light behind the caster at studio entry `entry` (b, its EV over the bloom's
    /// threshold times the share that shows past the caster's own pattern, as the pre-pass reads
    /// it), by brute force on an `nx` x `ny` midpoint grid over `xs` x `ys` of its tangent plane:
    /// each sample where b > 0, as its point and b x its area (tangent units).
    fn bright_by_brute_force(env: &Env, entry: usize, xs: (f32, f32), ys: (f32, f32), (nx, ny): (usize, usize)) -> Vec<(Vec2f, f64)> {
        let caster = env.studio.caster(entry).expect("the caster draws");
        let sharp = |d: Vec3f| env.studio.caster_sharp(entry, d);
        let at = Sampling::bake(CASTER_REFERENCE_WIDTH, None);
        let threshold = 2.0f32.powf(caster.params.bloom_threshold_ev);
        let (dx, dy) = ((xs.1 - xs.0) / nx as f32, (ys.1 - ys.0) / ny as f32);
        let mut bright = Vec::new();
        for j in 0..ny {
            let y = ys.0 + (j as f32 + 0.5) * dy;
            for i in 0..nx {
                let x = xs.0 + (i as f32 + 0.5) * dx;
                let d = (caster.frame.center + caster.frame.right * x + caster.frame.up * y).normalize();
                let lum = luminance(env.content_before(d, entry, at));
                if lum > threshold {
                    let b = (lum.min(MAX_RADIANCE) / threshold).log2() * (1.0 - caster.unbloomed(d, &sharp));
                    if b > 0.0 {
                        bright.push((vec2f(x, y), b as f64 * (dx * dy) as f64));
                    }
                }
            }
        }
        bright
    }

    /// How the bloom of the caster at `entry` holds `bright` (`bright_by_brute_force`, which
    /// must hold all of it): its raster's and points' bright light over the brute force's, and
    /// the largest difference of its share from the brute force's blur, 1 - exp(-S) with S the
    /// kernel (`spline` along both axes, w = 2/3 of the length) over `bright` / BLOOM_REF_ENERGY,
    /// on a 41 x 31 grid over the caster's box.
    fn against_the_brute_force(env: &Env, entry: usize, bright: &[(Vec2f, f64)]) -> (f64, f32) {
        let caster = env.studio.caster(entry).expect("the caster draws");
        let Some(bloom) = caster.bloom() else { return (0.0, 1.0) };
        let total: f64 = bright.iter().map(|&(_, e)| e).sum();
        let w = 2.0 / 3.0 * caster.params.bloom_length_deg.to_radians().tan();
        let mut worst = 0.0f32;
        for j in 0..31 {
            let y = caster.half.y * (-1.0 + 2.0 * (j as f32 + 0.5) / 31.0);
            for i in 0..41 {
                let x = caster.half.x * (-1.0 + 2.0 * (i as f32 + 0.5) / 41.0);
                let s: f64 = bright.iter().map(|&(q, e)| e * (spline((x - q.x) / w) * spline((y - q.y) / w)) as f64).sum::<f64>() / BLOOM_REF_ENERGY;
                let want = 1.0 - (-s).exp();
                let got = bloom.share((caster.frame.center + caster.frame.right * x + caster.frame.up * y).normalize());
                worst = worst.max((got - want as f32).abs());
            }
        }
        (bloom.energy() / total, worst)
    }

    /// Fix round 2 of S3's review: a long bloom's raster (cells of a sixth of the length, 3.5
    /// degrees under a 20 degree bloom) still blooms a light far smaller than a cell as its
    /// own area, wherever it stands. A 2 degree softbox at EV 10.5 (half an EV over the
    /// threshold) beside a solid 40 x 30 degree caster's edge, 3 degrees out, at nine places
    /// 0.4 degrees apart along it (a cell in all): the bloom holds its bright light to within
    /// SMALL_LIGHT_ENERGY of the brute force's, and its share is the brute force's blur to within
    /// SMALL_LIGHT_SHARE everywhere on the caster. (A cell sampled at its centre alone held all
    /// of such a light or none of it: as it moved, it bloomed 2.4 times its light, then
    /// nothing. Such a light is now kept as points, as a disc is.)
    #[test]
    fn a_light_smaller_than_a_cell_blooms_as_its_own_area_wherever_it_stands() {
        use crate::hdri::dir_from_az_el;
        let mut off = Vec::new();
        for k in 0..9 {
            let el = 25.0 + 0.4 * k as f32;
            let light = LightParams { width_deg: 2.0, height_deg: 2.0, softness: 0.1, intensity_ev: 10.5, ..softbox(-27.0, el) };
            let caster = caster_at(0.0, 25.0, 40.0, 30.0, None, CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() });
            let env = Env::new(&grey_studio(vec![light, caster]));
            if let Some(cell) = env.studio.caster(1).and_then(|c| c.bloom()).map(|b| b.cell()) {
                assert!(cell > 3.0f32.to_radians().tan(), "the premise: a cell ({} degrees) is wider than the light", cell.atan().to_degrees());
            }
            // The light's place in the caster's plane, and its box with its soft edge and more.
            let frame = env.studio.caster(1).expect("the caster draws").frame;
            let centre = dir_from_az_el(-27.0, el);
            let (cx, cy) = (centre.dot(frame.right) / centre.dot(frame.center), centre.dot(frame.up) / centre.dot(frame.center));
            let r = 2.0f32.to_radians().tan();
            let bright = bright_by_brute_force(&env, 1, (cx - r, cx + r), (cy - r, cy + r), (100, 100));
            let (energy, share) = against_the_brute_force(&env, 1, &bright);
            println!("light at elevation {el:.1}: bright light {energy:.4} of the brute force's, share off by {share:.4}");
            if !((energy - 1.0).abs() <= SMALL_LIGHT_ENERGY && share <= SMALL_LIGHT_SHARE) {
                off.push(format!("at elevation {el:.1}: {energy:.3} of its light, share off by {share:.3}"));
            }
        }
        assert!(off.is_empty(), "a light smaller than a cell does not bloom as its area: {}", off.join(", "));
    }

    /// `a_light_smaller_than_a_cell_blooms_as_its_own_area_wherever_it_stands`'s tolerances.
    const SMALL_LIGHT_ENERGY: f64 = 0.02;
    const SMALL_LIGHT_SHARE: f32 = 0.01;

    /// Fix round 2 of S3's review: a long bloom over a leaves caster blooms the light that shows
    /// through its gaps, not the gaps its cells' centres happen to fall in. A dim 30 x 20 degree
    /// softbox (its middle 0.003 EV over the caster's threshold, so the share stays well under 1)
    /// behind a 40 x 30 degree leaves caster with 20 degrees of bloom, at five places 0.7 degrees
    /// apart: the raster holds the bright light to within LEAVES_ENERGY of the brute force's, and
    /// its share is the brute force's blur to within LEAVES_SHARE everywhere on the caster, as the
    /// softbox moves.
    #[test]
    fn a_long_bloom_over_leaves_is_the_blur_of_what_shows_through_them_as_the_light_moves() {
        use crate::hdri::dir_from_az_el;
        let mut off = Vec::new();
        for k in 0..5 {
            let az = 0.7 * k as f32;
            let light = LightParams { width_deg: 30.0, height_deg: 20.0, softness: 0.2, ..softbox(az, 25.0) };
            let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() };
            let mut p = grey_studio(vec![light, caster_at(0.0, 25.0, 40.0, 30.0, Some(leaves(0.6, 3)), settings)]);
            // The threshold 0.003 EV under the softbox's middle.
            let lum = luminance(Env::new(&bloom_at(p.clone(), 0.0)).content_before(dir_from_az_el(az, 25.0), 1, Sampling::bake(CASTER_REFERENCE_WIDTH, None)));
            p.lights[1].caster = Some(CasterParams { bloom_threshold_ev: lum.log2() - 0.003, ..p.lights[1].caster_params() });
            let env = Env::new(&p);
            let (xs, ys) = (21f32.to_radians().tan(), 14f32.to_radians().tan());
            let bright = bright_by_brute_force(&env, 1, (-xs, xs), (-ys, ys), (280, 190));
            let (energy, share) = against_the_brute_force(&env, 1, &bright);
            println!("softbox at azimuth {az:.1}: bright light {energy:.4} of the brute force's, share off by {share:.4}");
            if !((energy - 1.0).abs() <= LEAVES_ENERGY && share <= LEAVES_SHARE) {
                off.push(format!("at azimuth {az:.1}: {energy:.3} of its light, share off by {share:.3}"));
            }
        }
        assert!(off.is_empty(), "a long bloom over leaves is not the blur of what shows through them: {}", off.join(", "));
    }

    /// `a_long_bloom_over_leaves_is_the_blur_of_what_shows_through_them_as_the_light_moves`'s
    /// tolerances: the brute force's own grid moves its bright light by about 1 % (at the
    /// softbox's middle, where its edges fall alike on both sides).
    const LEAVES_ENERGY: f64 = 0.04;
    const LEAVES_SHARE: f32 = 0.025;

    /// Fix round 3 of S3's review: a highlight in a base image (no light of the list, no sun)
    /// smaller than a cell blooms as its own area wherever it falls between the cells' corners.
    /// A photograph 1024 wide, dark (0.5) but for a spot a degree across at 2^12 (2 EV over the
    /// default threshold), with a broad glow under the threshold far behind the camera that
    /// keeps the spot from being taken for a sun; a solid 40 x 30 degree caster with 20 degrees
    /// of bloom (cells of 3.5 degrees), the spot about 3 degrees outside its edge, at nine places
    /// 0.4 degrees apart (a cell in all): the bloom holds the spot's bright light to within
    /// IMAGE_HIGHLIGHT_ENERGY of the brute force's, and its share is the brute force's blur to
    /// within IMAGE_HIGHLIGHT_SHARE everywhere on the caster. (Read at the cells' corners alone,
    /// such a spot bloomed only where a corner fell on it.)
    #[test]
    fn a_highlight_in_a_base_image_blooms_wherever_it_falls_between_the_cells_corners() {
        use crate::hdri::{dir_from_az_el, BaseImageParams, ImageSet};
        let mut off = Vec::new();
        for k in 0..9 {
            let el = 25.0 + 0.4 * k as f32;
            let spot = dir_from_az_el(-27.0, el);
            let glow = dir_from_az_el(180.0, 30.0);
            let photo = ibl::EnvMap::from_fn(1024, |d| {
                let d = vec3f(d[0], d[1], d[2]);
                let v = if d.dot(spot) >= 0.5f32.to_radians().cos() {
                    4096.0
                } else if d.dot(glow) >= 25f32.to_radians().cos() {
                    900.0
                } else {
                    0.5
                };
                [v; 3]
            });
            let mut images = ImageSet::default();
            images.insert("photo", photo);
            let caster = caster_at(0.0, 25.0, 40.0, 30.0, None, CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() });
            let mut p = grey_studio(vec![caster]);
            p.base_image = Some(BaseImageParams { key: "photo".to_string(), ..BaseImageParams::default() });
            let env = Env::new_with(&p, &images);
            assert!(env.base_image.as_ref().expect("the photograph is the base").drawn_sun().is_none(), "the premise: the spot is no sun");
            if let Some(cell) = env.studio.caster(0).and_then(|c| c.bloom()).map(|b| b.cell()) {
                assert!(cell > 3.0f32.to_radians().tan(), "the premise: a cell ({} degrees) is wider than the spot", cell.atan().to_degrees());
            }
            let frame = env.studio.caster(0).expect("the caster draws").frame;
            let (cx, cy) = (spot.dot(frame.right) / spot.dot(frame.center), spot.dot(frame.up) / spot.dot(frame.center));
            let r = 2.0f32.to_radians().tan();
            let bright = bright_by_brute_force(&env, 0, (cx - r, cx + r), (cy - r, cy + r), (100, 100));
            let (energy, share) = against_the_brute_force(&env, 0, &bright);
            println!("spot at elevation {el:.1}: bright light {energy:.4} of the brute force's, share off by {share:.4}");
            if !((energy - 1.0).abs() <= IMAGE_HIGHLIGHT_ENERGY && share <= IMAGE_HIGHLIGHT_SHARE) {
                off.push(format!("at elevation {el:.1}: {energy:.3} of its light, share off by {share:.3}"));
            }
        }
        assert!(off.is_empty(), "a highlight of the base image does not bloom as its area: {}", off.join(", "));
    }

    /// `a_highlight_in_a_base_image_blooms_wherever_it_falls_between_the_cells_corners`'s tolerances.
    const IMAGE_HIGHLIGHT_ENERGY: f64 = 0.05;
    const IMAGE_HIGHLIGHT_SHARE: f32 = 0.02;

    /// Fix round 3 of S3's review: a storm's bolt, a line far thinner than a cell, blooms
    /// wherever it falls between the cells' corners, though the storm round it is far under the
    /// threshold (`Env::base_peak`). The threshold an EV under the bolt; a solid 40 x 30 degree
    /// caster with 20 degrees of bloom, its left edge about 3 degrees right of the bolt, at nine
    /// places 0.4 degrees apart (a cell in all, 3.5 degrees): the bright light its bloom holds
    /// stays within BOLT_SPREAD of its mean, and is never none.
    #[test]
    fn a_storms_bolt_blooms_wherever_it_falls_between_the_cells_corners() {
        use crate::hdri::{dir_from_az_el, WeatherParams};
        let mut p = HdriParams::default();
        p.sky.weather = Some(WeatherParams { kind: "storm".to_string(), lightning: true, ..WeatherParams::default() });
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = 20.0;
        // The bolt: the brightest of the storm, as a caster's pre-pass reads it.
        let storm = Env::new(&p);
        let at = Sampling::bake(CASTER_REFERENCE_WIDTH, None);
        let (mut bolt, mut brightest) = ((0.0f32, 0.0f32), 0.0f32);
        for i in 0..1440 {
            for j in 0..30 {
                let (az, el) = (-180.0 + 0.25 * i as f32, 5.0 + 0.5 * j as f32);
                let lum = luminance(storm.content_before(dir_from_az_el(az, el), 0, at));
                if lum > brightest {
                    (bolt, brightest) = ((az, el), lum);
                }
            }
        }
        // The threshold an EV under the bolt's brightest; the storm round it is far under it.
        let threshold_ev = brightest.log2() - 1.0;
        let round = luminance(storm.content_before(dir_from_az_el(bolt.0 + 10.0, bolt.1), 0, at));
        assert!(round.log2() < threshold_ev - BLOOM_NEAR_EV, "the premise: the storm 10 degrees off the bolt ({round}) is far under the threshold ({threshold_ev} EV)");
        p.sky.sun.azimuth_deg = bolt.0 + 180.0;
        let mut energies = Vec::new();
        for k in 0..9 {
            let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, bloom_threshold_ev: threshold_ev, ..CasterParams::default() };
            p.lights = vec![caster_at(bolt.0 + 23.0 + 0.4 * k as f32, bolt.1, 40.0, 30.0, None, settings)];
            let env = Env::new(&p);
            energies.push(env.studio.caster(0).and_then(|c| c.bloom()).map_or(0.0, |b| b.energy()));
        }
        let mean = energies.iter().sum::<f64>() / energies.len() as f64;
        let by_place: Vec<String> = energies.iter().map(|e| format!("{:.3}", e / mean)).collect();
        println!("the bolt at {bolt:?} ({brightest:.0}): the bloom's bright light by place, of its mean {mean:.4e}: {}", by_place.join(" "));
        assert!(energies.iter().all(|&e| e > 0.0 && (e / mean - 1.0).abs() <= BOLT_SPREAD), "the bolt does not bloom alike as it moves: {} of the mean {mean:.4e}", by_place.join(" "));
    }

    /// `a_storms_bolt_blooms_wherever_it_falls_between_the_cells_corners`'s tolerance.
    const BOLT_SPREAD: f64 = 0.15;

    /// Fix round 3 of S3's review: a sky under a low threshold blooms what passes it between
    /// the cells' corners. The cloudy sky (coverage 0.6, cirrus 0.4, the sun behind the camera)
    /// behind a 40 x 30 degree leaves caster with 20 degrees of bloom, its threshold at the
    /// sky's 85th percentile over the caster, so what passes is cloud tops and the band over the
    /// horizon, a few degrees across or less: the raster holds the bright light (all of it, the
    /// border cells too) to within SKY_ENERGY of the brute force's. (Read at the corners alone,
    /// it lost 5 to 12 % of it: cloud tops between the corners of cells whose corners stayed
    /// under the threshold, and the band brighter between corners alike. The share is not held
    /// here: with bright light all round the caster, the raster's blur, its cells a sixth of the
    /// length, reaches past the length by about a cell and a half, so far from the brightest light
    /// its share is up to about 0.15 over the brute force's, as it was before this fix.)
    #[test]
    fn a_sky_under_a_low_threshold_blooms_what_passes_between_the_cells_corners() {
        let mut off = Vec::new();
        for seed in [1u32, 2, 3] {
            let mut p = HdriParams::default();
            p.seed = seed;
            p.sky.sun.mode = "manual".to_string();
            p.sky.sun.azimuth_deg = 180.0;
            p.sky.sun.elevation_deg = 30.0;
            p.sky.clouds.coverage = 0.6;
            p.sky.clouds.cirrus = 0.4;
            let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() };
            p.lights = vec![caster_at(0.0, 25.0, 40.0, 30.0, Some(leaves(0.6, 3)), settings)];
            // The threshold: the sky's 85th percentile over the caster's box.
            let probe = Env::new(&bloom_at(p.clone(), 0.0));
            let frame = probe.studio.caster(0).expect("the caster draws").frame;
            let (xs, ys) = (21f32.to_radians().tan(), 14f32.to_radians().tan());
            let mut lums: Vec<f32> = (0..45 * 60)
                .map(|i| {
                    let (x, y) = (-xs + 2.0 * xs * ((i % 60) as f32 + 0.5) / 60.0, -ys + 2.0 * ys * ((i / 60) as f32 + 0.5) / 45.0);
                    luminance(probe.content_before((frame.center + frame.right * x + frame.up * y).normalize(), 0, Sampling::bake(CASTER_REFERENCE_WIDTH, None)))
                })
                .collect();
            lums.sort_by(|a, b| a.total_cmp(b));
            let threshold_ev = lums[lums.len() * 85 / 100].log2();
            p.lights[0].caster = Some(CasterParams { bloom_threshold_ev: threshold_ev, ..p.lights[0].caster_params() });
            let env = Env::new(&p);
            // All the raster holds: the box, the bloom length round it and its border cells.
            let ext = env.studio.caster(0).and_then(|c| c.bloom()).expect("the sky blooms the caster").ext;
            let bright = bright_by_brute_force(&env, 0, (-ext.x, ext.x), (-ext.y, ext.y), (360, 300));
            let (energy, share) = against_the_brute_force(&env, 0, &bright);
            println!("seed {seed}, threshold {threshold_ev:.2} EV: bright light {energy:.4} of the brute force's, share off by {share:.4}");
            if !((energy - 1.0).abs() <= SKY_ENERGY) {
                off.push(format!("seed {seed}: {energy:.3} of its light"));
            }
        }
        assert!(off.is_empty(), "a sky under a low threshold does not bloom what passes it: {}", off.join(", "));
    }

    /// `a_sky_under_a_low_threshold_blooms_what_passes_between_the_cells_corners`'s tolerance: the
    /// raster's cells at the samples BLOOM_CELL_BUDGET leaves them hold it to within about 3 %.
    const SKY_ENERGY: f64 = 0.04;

    /// Fix round 3 of S3's review: a light with detail inside it (a picture, a shape core,
    /// something hung in front) that fills a long bloom's raster keeps to BLOOM_CELL_BUDGET
    /// lookups behind the caster beyond the cells' corners, where its edges and its detail may
    /// cross the cells as much as inside it. Behind an open 40 x 30 degree caster with 20
    /// degrees of bloom (a raster of 3.5 degree cells, each ten map texels across), a light
    /// over all of the raster whose content is a checker of degree squares 2 and 4 EV over the
    /// threshold, so no cell's corners are alike. Fix round 4 (M5): the map's blooming casters
    /// share the budget alike, so one of four keeps to a quarter of it.
    #[test]
    fn a_detailed_light_keeps_to_its_sample_budget_where_no_cells_corners_are_alike() {
        use crate::hdri::{dir_from_az_el, ImageSet};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let light = caster_at(0.0, 20.0, 40.0, 30.0, None, CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() });
        let caster = CasterLayer::new(&light, &ImageSet::default(), CASTER_REFERENCE_WIDTH, &serial_rows).expect("a solid caster draws");
        let lookups = AtomicUsize::new(0);
        let behind = |d: Vec3f| {
            lookups.fetch_add(1, Ordering::Relaxed);
            let (az, el) = (d.x.atan2(-d.z).to_degrees(), d.y.asin().to_degrees());
            let odd = (az.floor() as i64 + el.floor() as i64).rem_euclid(2) == 1;
            Vec3f::all(if odd { 16384.0 } else { 4096.0 })
        };
        let open = |_: Vec3f| 0.0;
        let axis = dir_from_az_el(0.0, 20.0);
        let frame = light_frame(0.0, 20.0, 0.0);
        let reach = vec2f(60f32.to_radians().tan(), 55f32.to_radians().tan());
        let bright = [BrightBox { axis, cos_cover: 70f32.to_radians().cos(), frame_reach: Some((frame, reach)), feature: 2.0 * reach.y, detailed: true }];
        for casters in [1, 4] {
            lookups.store(0, Ordering::Relaxed);
            let bloom = CasterBloom::build(&caster, CASTER_REFERENCE_WIDTH, casters, &[], &bright, None, &behind, &open, &|| false, &serial_rows).expect("nothing cancels it");
            assert!(bloom.thins_anything(), "the light is bright");
            let corners = (bloom.cols + 1) * (bloom.rows + 1);
            let beyond = lookups.load(Ordering::Relaxed) - corners;
            let budget = BLOOM_CELL_BUDGET / casters;
            println!("one of {casters} casters, {} cells: {corners} corners and {beyond} lookups more", bloom.cols * bloom.rows);
            assert!(beyond <= budget, "one of {casters} casters: the light took {beyond} lookups beyond the corners, over its budget of {budget}");
        }
    }

    /// Fix round 4 of S3's review: what the cells' corners miss of the base still blooms as it
    /// does alone when a detailed light beside it asks the raster for far more than
    /// BLOOM_CELL_BUDGET (the budget scaled such cells to their corners, which hold none of it, or
    /// to a few samples). Behind an open 40 x 30 degree caster with 20 degrees of bloom (cells of
    /// 3.5 degrees): a light over the raster's right part, a checker of degree squares 2 and 4 EV
    /// over the threshold whose detail asks for the most samples a cell takes; on the left, 4 to
    /// 7 degrees outside the caster's edge, a highlight a degree across half an EV over the
    /// threshold that the base reports (`peak`), and a bump 2 degrees across an eighth of an EV
    /// over it in a patch an EV under it (a cloud top), at five places 0.7 degrees apart. Where the
    /// light's bloom does not reach (past 28 degrees left of the caster's middle), the share is
    /// the share the base alone blooms to within BESIDE_A_LIGHT_SHARE.
    #[test]
    fn what_the_base_draws_between_the_corners_blooms_alike_beside_a_light_past_the_budget() {
        use crate::hdri::{dir_from_az_el, ImageSet};
        let light = caster_at(0.0, 20.0, 40.0, 30.0, None, CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() });
        let caster = CasterLayer::new(&light, &ImageSet::default(), CASTER_REFERENCE_WIDTH, &serial_rows).expect("a solid caster draws");
        let threshold = 2.0f32.powf(caster.params.bloom_threshold_ev);
        let open = |_: Vec3f| 0.0;
        // The light: the cone of 35 degrees round azimuth 40, its edge 5 degrees right of the
        // caster's middle.
        let (axis, cone) = (dir_from_az_el(40.0, 20.0), 35f32.to_radians().cos());
        let bright = [BrightBox { axis, cos_cover: cone, frame_reach: None, feature: 0.05f32.to_radians(), detailed: true }];
        let checker = |d: Vec3f| {
            let (az, el) = (d.x.atan2(-d.z).to_degrees(), d.y.asin().to_degrees());
            if (az.floor() as i64 + el.floor() as i64).rem_euclid(2) == 1 { 16.0 * threshold } else { 4.0 * threshold }
        };
        // Half an EV and an eighth of one over the threshold, so the share stays under 1.
        let (spot_over, bump_over) = (std::f32::consts::SQRT_2, 2f32.powf(0.125));
        let mut off = Vec::new();
        for k in 0..5 {
            let az = -24.0 - 0.7 * k as f32;
            let (spot, bump) = (dir_from_az_el(az, 26.0), dir_from_az_el(az, 12.0));
            let base = move |d: Vec3f| -> Vec3f {
                Vec3f::all(if d.dot(spot) >= 0.5f32.to_radians().cos() {
                    spot_over * threshold
                } else if d.dot(bump) >= 1f32.to_radians().cos() {
                    bump_over * threshold
                } else if d.dot(bump) >= 6f32.to_radians().cos() {
                    0.5 * threshold
                } else {
                    0.5
                })
            };
            let lit = |d: Vec3f| if d.dot(axis) >= cone { Vec3f::all(checker(d)) } else { base(d) };
            // The base reports the highlight, and asks for a fifth of a degree round it.
            let peak = move |d: Vec3f, radius: f32| (d.dot(spot) >= (radius + 0.5f32.to_radians()).cos()).then_some((spot_over * threshold, 0.2f32.to_radians()));
            let peak: Option<&(dyn Fn(Vec3f, f32) -> Option<(f32, f32)> + Sync)> = Some(&peak);
            let alone = CasterBloom::build(&caster, CASTER_REFERENCE_WIDTH, 1, &[], &[], peak, &base, &open, &|| false, &serial_rows).expect("nothing cancels it");
            let beside = CasterBloom::build(&caster, CASTER_REFERENCE_WIDTH, 1, &[], &bright, peak, &lit, &open, &|| false, &serial_rows).expect("nothing cancels it");
            if k == 0 {
                // The premise: the light's cells ask for many times the budget.
                let reached = (0..beside.rows).flat_map(|j| (0..beside.cols).map(move |i| (i, j))).filter(|&(i, j)| {
                    let (x, y) = (-beside.ext.x + (i as f32 + 0.5) * beside.cell, -beside.ext.y + (j as f32 + 0.5) * beside.cell);
                    (caster.frame.center + caster.frame.right * x + caster.frame.up * y).normalize().dot(axis) >= cone
                });
                let asked = reached.count() * BLOOM_LIGHT_SAMPLES_MAX * BLOOM_LIGHT_SAMPLES_MAX;
                assert!(asked > 4 * BLOOM_CELL_BUDGET, "the premise: the light asks for {asked} lookups, not many times the budget");
            }
            let (mut worst, mut most) = (0.0f32, 0.0f32);
            for j in 0..35 {
                for i in 0..11 {
                    let d = dir_from_az_el(-28.0 - i as f32, 2.0 + j as f32);
                    most = most.max(alone.share(d));
                    worst = worst.max((beside.share(d) - alone.share(d)).abs());
                }
            }
            println!("highlight and bump at azimuth {az:.1}: the share alone up to {most:.3}, beside the light off by {worst:.4}");
            assert!(most > 0.1, "the premise: the highlight and the bump bloom alone ({most})");
            if worst > BESIDE_A_LIGHT_SHARE {
                off.push(format!("at azimuth {az:.1}: off by {worst:.3} of up to {most:.3}"));
            }
        }
        assert!(off.is_empty(), "what the base draws between the corners does not bloom alike beside a light past the budget: {}", off.join(", "));
    }

    /// `what_the_base_draws_between_the_corners_blooms_alike_beside_a_light_past_the_budget`'s
    /// tolerance.
    const BESIDE_A_LIGHT_SHARE: f32 = 0.01;

    /// Fix round 3 of S3's review: a soft caster's bloom reads its coverage over each cell, or
    /// each sample's part of it, in one look (`SoftCoverage::mean_over`, M5: it took a look per
    /// map texel), and blooms as it does looking at that coverage point by point (the same caster
    /// handed over as a sharp one whose coverage is the soft one's). A 40 x 30 degree leaves
    /// caster with 10 degrees of bloom, at softness 0.05 (a blur far narrower than a cell) and
    /// 0.3, in front of light 0.004 EV over its threshold that is even (its cells' corners
    /// alike) or a checker of degree squares at it and 0.008 EV over it (no cell's corners alike,
    /// its samples held to BLOOM_CELL_BUDGET), so the share stays well under 1: the bright light
    /// within ONE_LOOK_ENERGY, the share within ONE_LOOK_SHARE everywhere on the caster.
    #[test]
    fn a_soft_casters_bloom_reads_its_coverage_over_a_cell_in_one_look() {
        use crate::hdri::ImageSet;
        let threshold = 2.0f32.powf(10.0);
        let even = |_: Vec3f| Vec3f::all(threshold * 2.0f32.powf(0.004));
        let checker = |d: Vec3f| {
            let (az, el) = (d.x.atan2(-d.z).to_degrees(), d.y.asin().to_degrees());
            let odd = (az.floor() as i64 + el.floor() as i64).rem_euclid(2) == 1;
            Vec3f::all(threshold * 2.0f32.powf(if odd { 0.008 } else { 0.0 }))
        };
        let mut off = Vec::new();
        for softness in [0.05f32, 0.3] {
            let light = caster_at(0.0, 25.0, 40.0, 30.0, Some(leaves(0.6, 3)), CasterParams { softness, bloom_strength: 1.0, bloom_length_deg: 10.0, ..CasterParams::default() });
            let soft = CasterLayer::new(&light, &ImageSet::default(), CASTER_REFERENCE_WIDTH, &serial_rows).expect("the caster draws");
            assert!(soft.soft.is_some(), "the premise: a soft caster");
            let looked = CasterLayer { soft: None, ..soft.clone() };
            let point_by_point = |d: Vec3f| soft.unbloomed(d, &|_| f32::NAN);
            for (name, behind) in [("even", &even as &(dyn Fn(Vec3f) -> Vec3f + Sync)), ("checker", &checker)] {
                let build = |caster: &CasterLayer| CasterBloom::build(caster, CASTER_REFERENCE_WIDTH, 1, &[], &[], None, behind, &point_by_point, &|| false, &serial_rows).expect("nothing cancels it");
                let (one_look, looks) = (build(&soft), build(&looked));
                let energy = one_look.energy() / looks.energy();
                let mut share = 0.0f32;
                for j in 0..31 {
                    let y = soft.half.y * (-1.0 + 2.0 * (j as f32 + 0.5) / 31.0);
                    for i in 0..41 {
                        let x = soft.half.x * (-1.0 + 2.0 * (i as f32 + 0.5) / 41.0);
                        let d = (soft.frame.center + soft.frame.right * x + soft.frame.up * y).normalize();
                        share = share.max((one_look.share(d) - looks.share(d)).abs());
                    }
                }
                let middle = looks.share(soft.frame.center);
                println!("softness {softness}, {name} light: bright light {energy:.4} of the looks', share off by {share:.4} (the looks' share at the middle {middle:.3})");
                if !((energy - 1.0).abs() <= ONE_LOOK_ENERGY && share <= ONE_LOOK_SHARE) {
                    off.push(format!("softness {softness}, {name}: {energy:.4} of the light, share off by {share:.4}"));
                }
            }
        }
        assert!(off.is_empty(), "a soft caster's bloom in one look is not its bloom looked at point by point: {}", off.join(", "));
    }

    /// `a_soft_casters_bloom_reads_its_coverage_over_a_cell_in_one_look`'s tolerances.
    const ONE_LOOK_ENERGY: f64 = 0.01;
    const ONE_LOOK_SHARE: f32 = 0.004;

    /// Fix round 4 of S3's review: a light with detail whose cells ask for more than
    /// BLOOM_CELL_BUDGET blooms the light it has, its cells scaled alike. A 60 x 40 degree panel
    /// at EV 10.3 (0.3 EV over the threshold) with blinds hung in front of it, behind a 40 x 30
    /// degree leaves caster with 10 degrees of bloom, at three places 0.7 degrees apart: the
    /// raster holds the bright light to within DETAILED_LIGHT_ENERGY of the brute force's. (With
    /// the blinds' dim slats kept at their own samples, the bright ones were left their corners
    /// and the raster held 19 to 26 % too much. Its share is not held here: at the budget the
    /// slats are sampled at two a cell side, and it is off by up to about 0.4, as before.)
    #[test]
    fn a_detailed_light_past_the_budget_blooms_the_light_it_has() {
        let mut off = Vec::new();
        for k in 0..3 {
            let az = 0.7 * k as f32;
            let blinds = GoboParams { kind: "blinds".to_string(), slats: 8, distance: 0.3, ..GoboParams::default() };
            let panel = LightParams { width_deg: 60.0, height_deg: 40.0, softness: 0.2, intensity_ev: 10.3, gobo: Some(blinds), ..softbox(az, 25.0) };
            let tree = caster_at(0.0, 25.0, 40.0, 30.0, Some(leaves(0.6, 3)), CasterParams { bloom_strength: 1.0, bloom_length_deg: 10.0, ..CasterParams::default() });
            let env = Env::new(&grey_studio(vec![panel, tree]));
            let ext = env.studio.caster(1).and_then(|c| c.bloom()).expect("the panel blooms the caster").ext;
            let bright = bright_by_brute_force(&env, 1, (-ext.x, ext.x), (-ext.y, ext.y), (480, 384));
            let (energy, share) = against_the_brute_force(&env, 1, &bright);
            println!("panel at azimuth {az:.1}: bright light {energy:.4} of the brute force's, share off by {share:.4}");
            if !((energy - 1.0).abs() <= DETAILED_LIGHT_ENERGY) {
                off.push(format!("at azimuth {az:.1}: {energy:.3} of its light"));
            }
        }
        assert!(off.is_empty(), "a detailed light past the budget does not bloom the light it has: {}", off.join(", "));
    }

    /// `a_detailed_light_past_the_budget_blooms_the_light_it_has`'s tolerance.
    const DETAILED_LIGHT_ENERGY: f64 = 0.1;

    /// `p` with every caster's bloom strength set to `strength`.
    fn bloom_at(mut p: HdriParams, strength: f32) -> HdriParams {
        for light in p.lights.iter_mut().filter(|l| l.is_caster()) {
            light.caster = Some(CasterParams { bloom_strength: strength, ..light.caster_params() });
        }
        p
    }

    /// M4 (plan 4's final review), the bloom's sources: a caster blooms from what is listed
    /// before it, never from what draws over it. The softbox of `softbox_behind_leaves` listed
    /// before the leaves thins them; listed after, nothing bright stands behind them, so the
    /// caster keeps no bloom and the map is the map with bloom off, bit for bit.
    #[test]
    fn a_caster_blooms_from_what_is_listed_before_it_only() {
        let before = softbox_behind_leaves(5.0);
        let after = HdriParams { lights: vec![before.lights[1].clone(), before.lights[0].clone()], ..before.clone() };
        let env = Env::new(&before);
        assert!(env.studio.caster(1).is_some_and(|c| c.bloom().is_some()), "the softbox listed before the caster blooms it");
        assert!(env.bake(64) != Env::new(&bloom_at(before, 0.0)).bake(64), "and the bloom shows");
        let env = Env::new(&after);
        assert!(env.studio.caster(0).is_some_and(|c| c.bloom().is_none()), "the softbox listed after the caster draws over it and blooms nothing");
        assert!(env.bake(64) == Env::new(&bloom_at(after, 0.0)).bake(64), "bit for bit the map with bloom off");
    }

    /// M4: a caster blooms from the light that shows past its own pattern (its gaps and round
    /// its edges), whatever its strength and tint. The sun deep behind a solid caster (its
    /// edges 15 degrees off, the bloom 5 degrees long) shows nowhere, so the caster keeps no
    /// bloom, at strength 1 and at 0.5, and the map is the map with bloom off; the same caster
    /// with its edge through the sun blooms.
    #[test]
    fn a_sun_deep_behind_a_solid_caster_blooms_nothing() {
        for strength in [1.0f32, 0.5] {
            let mut p = sun_at_the_edge(5.0);
            p.lights[0].elevation_deg = 30.0;
            p.lights[0].caster = Some(CasterParams { strength, ..p.lights[0].caster_params() });
            let env = Env::new(&p);
            assert!(env.studio.caster(0).is_some_and(|c| c.bloom().is_none()), "strength {strength}: a hidden sun blooms the caster");
            assert!(env.bake(64) == Env::new(&bloom_at(p, 0.0)).bake(64), "strength {strength}: bit for bit the map with bloom off");
        }
        assert!(edge_env(5.0, CASTER_REFERENCE_WIDTH).studio.caster(0).is_some_and(|c| c.bloom().is_some()), "the sun beside its edge blooms it");
    }

    /// M4: a caster sees the bloom of the casters listed before it. Two leaves casters stand
    /// in front of the softbox of `softbox_behind_leaves`, the second over the first: with the
    /// first's bloom on, more of the softbox shows past it, so the second's raster holds more
    /// bright light than with the first's bloom off.
    #[test]
    fn a_caster_sees_the_bloom_of_the_casters_before_it() {
        let mut p = softbox_behind_leaves(5.0);
        p.lights.push(caster_at(0.0, 25.0, 30.0, 30.0, Some(leaves(0.4, 7)), CasterParams { bloom_strength: 1.0, bloom_length_deg: 5.0, ..CasterParams::default() }));
        let second = |p: &HdriParams| Env::new(p).studio.caster(2).and_then(|c| c.bloom()).expect("the second caster blooms").energy();
        let mut first_off = p.clone();
        first_off.lights[1].caster = Some(CasterParams { bloom_strength: 0.0, ..p.lights[1].caster_params() });
        let (on, off) = (second(&p), second(&first_off));
        assert!(on > 1.01 * off, "the second caster's bright light: {on:.4e} with the first's bloom, {off:.4e} without");
    }
}
