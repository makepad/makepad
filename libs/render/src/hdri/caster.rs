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
//! over it untouched.
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
use super::studio::{project, sd_round_rect, LightFrame, MIN_EDGE};
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
        Some(CasterLayer { strength: settings.strength, tint: vec3f(t[0], t[1], t[2]), frame, sx, soft })
    }

    /// No raster at all (softness 0, and R5: no bloom): drawn exactly as its flag.
    pub(crate) fn is_plain(&self) -> bool {
        self.soft.is_none()
    }

    /// The coverage m' 0..=1 at `dir` (map frame): the softness raster's box
    /// filter where there is one, else `sharp(dir)`, the analytic coverage of
    /// the flag (`studio::flag_coverage`). A NaN covers nothing.
    pub(crate) fn coverage(&self, dir: Vec3f, sharp: &dyn Fn(Vec3f) -> f32) -> f32 {
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
}

/// A caster's sharp coverage in its face coordinates (x to the viewer's
/// right, y up, tangent units; `LightFx::face`): its flag's box, with the
/// flag's hard edge, times what its pattern blocks. It is the coverage the
/// studio draws the flag with (`studio::flag_coverage`); here it is rastered.
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

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A caster at (az, el), `width_deg` x `height_deg`, its pattern `gobo` (None: solid).
    fn caster(azimuth_deg: f32, elevation_deg: f32, width_deg: f32, height_deg: f32, gobo: Option<GoboParams>, settings: CasterParams) -> LightParams {
        LightParams { azimuth_deg, elevation_deg, width_deg, height_deg, gobo, caster: Some(settings), ..LightParams::new_caster() }
    }

    fn leaves(density: f32) -> GoboParams {
        GoboParams { kind: "leaves".to_string(), density, seed: 3, ..GoboParams::default() }
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
        let patterns = [None, Some(leaves(0.6)), Some(GoboParams { kind: "blinds".to_string(), slats: 8, ..GoboParams::default() }), Some(GoboParams::default())];
        let key = LightParams { key: true, ..softbox(5.0, 25.0) };
        for gobo in patterns {
            let c = caster(0.0, 25.0, 50.0, 40.0, gobo.clone(), settings.clone());
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
            p.lights.push(caster(90.0, 0.0, 30.0, 30.0, None, soft(softness)));
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
                let c = caster(0.0, 25.0, 50.0, 40.0, mask(name), soft(softness));
                assert!(!caster_drawn(&c, &images), "{name:?}: a missing picture");
                assert!(CasterLayer::new(&c, &images).is_none(), "{name:?}: no layer");
                let env = Env::new_with(&grey_studio(vec![key.clone(), c]), &images);
                assert!(env.bake(64) == open_map, "softness {softness}, {name:?}: a caster with a missing picture drew");
                assert_eq!(env.sun().expect("a key").radiance, open_key, "softness {softness}, {name:?}: it dimmed the key");
            }
            let c = caster(0.0, 25.0, 50.0, 40.0, mask("stripes"), soft(softness));
            assert!(caster_drawn(&c, &images));
            let env = Env::new_with(&grey_studio(vec![key.clone(), c]), &images);
            assert!(env.bake(64) != open_map, "softness {softness}: the loaded stripes draw");
            let share = luminance(env.sun().expect("a key").radiance) / luminance(open_key);
            assert!(share < 0.8, "softness {softness}: the stripes dim the key, which keeps {share}");
        }
        // Every other caster draws, on or off (the studio leaves out what is off).
        assert!(caster_drawn(&caster(0.0, 25.0, 50.0, 40.0, None, soft(0.0)), &ImageSet::default()));
        assert!(caster_drawn(&LightParams { enabled: false, ..caster(0.0, 25.0, 50.0, 40.0, Some(leaves(0.5)), soft(0.0)) }, &ImageSet::default()));
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
        let sharp = Env::new(&grey_studio(vec![key.clone(), caster(0.0, 25.0, 50.0, 40.0, Some(leaves(0.5)), soft(0.0))]));
        sharp.bake(32);
        sharp.sun();
        assert_eq!(counted(Work::CasterSoft), before, "a sharp caster rasters nothing");
        let p = grey_studio(vec![key, caster(0.0, 25.0, 50.0, 40.0, Some(leaves(0.5)), soft(0.4)), caster(90.0, 10.0, 30.0, 30.0, None, soft(1.0))]);
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
            ("leaves", leaves(0.5)),
            ("dense leaves", leaves(0.9)),
            ("fern", GoboParams { kind: "fern".to_string(), density: 0.6, ..GoboParams::default() }),
            ("window", GoboParams::default()),
            ("blinds", GoboParams { kind: "blinds".to_string(), slats: 10, ..GoboParams::default() }),
        ];
        // The widest blur reaches CASTER_SOFT_SCALE of the shorter half extent past the box.
        let pad = CASTER_SOFT_SCALE * 22.5f32.to_radians().tan() + 0.01;
        for (name, gobo) in patterns {
            let sharp_caster = caster(0.0, 20.0, 60.0, 45.0, Some(gobo), soft(0.0));
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
            ("leaves", caster(30.0, 20.0, 60.0, 45.0, Some(leaves(0.6)), soft(0.5))),
            ("a rolled window", LightParams { roll_deg: 20.0, ..caster(200.0, 40.0, 40.0, 30.0, Some(GoboParams::default()), soft(1.0)) }),
            ("a small solid", caster(120.0, -10.0, 8.0, 5.0, None, soft(0.2))),
            ("leaves at a slight softness", caster(300.0, 10.0, 90.0, 60.0, Some(leaves(0.8)), soft(0.05))),
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
}
