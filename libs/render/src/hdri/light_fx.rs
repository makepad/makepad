//! What phase 1b hangs in front of one light: a gobo (B6-B8), barn doors and
//! a honeycomb grid (B9), and the wear of its diffuser (B10).
//!
//! `Studio` keeps one `LightFx` beside each prepared light. The light's own
//! code draws it first, whatever its shape and content (rect, disc, polygon,
//! stroke, scrim, image); then `apply` scales what the light did to the map:
//!
//!     result = before + (after - before) x k
//!
//! - an emitter (Add): k = what passes the gobo, the doors and the grid,
//!   times the face's own wear;
//! - a flag (Multiply): k = what the gobo blocks, so a flag with a gobo
//!   darkens only behind the gobo's opaque parts (B8).
//!
//! One rule for every kind of light, and none of the per-light drawing code
//! has to know about it.
//!
//! Everything here is laid out in face coordinates: x to the right and y up
//! as the viewer at the centre sees the light, in tangent units.

use super::*;
use super::aim::Aim;
use super::gobo::Gobo;
use super::imperfections::Imperfections;
use super::noise::hash01;
use super::shaped::{half_extents, unit_to_dir};
use super::studio::{light_frame, project, LightFrame};

/// Strata per axis for the key light's share (`LightFx::key_share`).
/// Jittered, because a regular grid beats against panes, slats and grid cells.
/// At 64 the key stays within 2 % of what 1024-wide maps draw behind leaves,
/// fern and windows (80 lights and gobos: at most 1.6 %, 0.4 % on average);
/// 48 strata left up to 3.3 %.
const KEY_SAMPLES: i32 = 64;
/// Strata per axis for a key with a honeycomb on its face (B9). Turned off
/// its axis, a grid passes only a small spot of each cell (at grid 0.6
/// tilted 20 degrees, about a tenth of it), which 64 strata hit too seldom:
/// the key missed the map by up to 13 % there. 256 keep it within 0.6 %, for
/// about 12 ms more per map, so only a gridded key pays them.
const KEY_SAMPLES_GRID: i32 = 256;
/// The jitter's seed: fixed, so the same light always gives the same key.
const KEY_JITTER_SEED: u32 = 0x6b65_7921;
/// Midpoints per axis of the scan that finds where in its box a key draws,
/// before the strata are laid over that part (`LightFx::key_share`).
const KEY_SCAN: i32 = 48;

/// A light's frame with the two facts every extra needs about it: `sx`, the
/// sign that turns the frame's `right` into the viewer's right, and the
/// tangent half extents tan(width/2), tan(height/2).
pub fn face_axes(light: &LightParams) -> (LightFrame, f32, Vec2f) {
    let frame = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
    let (hx, hy) = half_extents(light);
    // A3's frame follows ibl::softbox, whose `right` points to the viewer's
    // left. B2's unit box runs to the viewer's right whichever way that is,
    // so one probe through it settles the sign.
    let sx = match project(&frame, unit_to_dir(light, [0.01, 0.0])) {
        Some(t) if t.x < 0.0 => -1.0,
        _ => 1.0,
    };
    (frame, sx, vec2f(hx, hy))
}

/// Everything in front of one light.
#[derive(Clone, Debug)]
pub struct LightFx {
    frame: LightFrame,
    /// +1 or -1: `frame.right * sx` is the viewer's right.
    sx: f32,
    blend: Blend,
    /// 2^intensity_ev: the gain a window filled with sky shows the sky at.
    gain: f32,
    fill_sky: bool,
    gobo: Option<Gobo>,
    /// The aimed face with its grid and doors (B9).
    aim: Option<Aim>,
    /// The wear of the face (B10).
    surface: Option<Imperfections>,
}

impl LightFx {
    /// The extras of `light`. `images` resolves image gobos (B8); a name the
    /// app has not loaded is simply no gobo, so this never reads a file.
    pub fn new(light: &LightParams, images: &ImageSet) -> LightFx {
        let (frame, sx, half) = face_axes(light);
        let gobo = light.gobo.as_ref();
        LightFx {
            frame,
            sx,
            blend: light.blend(),
            gain: 2.0f32.powf(light.intensity_ev.max(-10.0).min(20.0)),
            fill_sky: gobo.is_some_and(|g| g.fill_sky),
            gobo: gobo.and_then(|g| Gobo::new(g, half, images)),
            aim: light.aim.as_ref().and_then(|a| Aim::new(a, frame.center, frame.right * sx, frame.up, half, light.softness)),
            surface: light.imperfections.as_ref().and_then(|p| Imperfections::new(p, half)),
        }
    }

    /// Something opaque hangs in front of the face: a gobo, doors or a grid.
    fn has_opaque(&self) -> bool {
        self.gobo.is_some() || self.aim.is_some()
    }

    /// The face itself is worn (B10).
    fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    /// True when nothing is in front of the light: `Studio` then draws it
    /// exactly as plan 1a did.
    pub fn is_plain(&self) -> bool {
        !self.fill_sky && !self.has_opaque() && !self.has_surface()
    }

    /// A window: an emitter that shows the map's sky instead of its colour.
    pub fn fills_with_sky(&self) -> bool {
        self.fill_sky && self.blend == Blend::Add
    }

    /// Face coordinates of `dir`: x to the viewer's right, y up, tangent
    /// units. None behind the light (where `project` gives up).
    pub fn face(&self, dir: Vec3f) -> Option<Vec2f> {
        project(&self.frame, dir).map(|t| vec2f(self.sx * t.x, t.y))
    }

    /// The share 0..=1 of an emitting face that shows at `dir` past
    /// everything opaque in front of it: its gobo, then its doors and grid.
    /// 1 where the frame cannot place `dir`.
    pub fn passes(&self, dir: Vec3f) -> f32 {
        let Some(q) = self.face(dir) else {
            return 1.0;
        };
        let mut t = 1.0;
        if let Some(gobo) = &self.gobo {
            t *= gobo.transmission(q);
        }
        if let Some(aim) = &self.aim {
            t *= aim.transmission(dir, q);
        }
        t
    }

    /// The brightness of the face itself at `dir`, around 1 with mean 1 over
    /// the face: its wrinkles, dust and weave. 1 where the frame cannot
    /// place `dir`.
    pub fn glow(&self, dir: Vec3f) -> f32 {
        match (&self.surface, self.face(dir)) {
            (Some(surface), Some(q)) => surface.factor(q),
            _ => 1.0,
        }
    }

    /// The share 0..=1 of a flag that darkens at `dir`. A flag with a gobo
    /// is cut by it: only the gobo's opaque parts darken, as window bars or
    /// a leaf canopy against whatever is behind. Doors, grids and wear
    /// belong to emitters and leave flags alone.
    fn blocks(&self, dir: Vec3f) -> f32 {
        match (&self.gobo, self.face(dir)) {
            (Some(gobo), Some(q)) => 1.0 - gobo.transmission(q),
            _ => 1.0,
        }
    }

    /// What the light really does at `dir`, given the map `before` it and
    /// `after` its own code drew it: its effect, scaled.
    pub fn apply(&self, dir: Vec3f, before: Vec3f, after: Vec3f) -> Vec3f {
        // The light does not reach this direction: nothing to scale.
        if after == before {
            return after;
        }
        let k = match self.blend {
            Blend::Add => self.passes(dir) * self.glow(dir),
            Blend::Multiply => self.blocks(dir),
        };
        if k == 1.0 {
            return after;
        }
        let c = before + (after - before) * k;
        // Wear can lift k above 1. A light that also hides what is behind it
        // (a scrim) must not push that below zero.
        vec3f(c.x.max(0.0), c.y.max(0.0), c.z.max(0.0))
    }

    /// A window filled with sky at `dir`: where the light's shape covers
    /// (`cover` 0..=1) and its gobo passes, what is `behind` gives way to
    /// `sky`, the map's sky along `dir`, at 2^intensity_ev. The frame and the
    /// bars leave what is behind them as it was.
    pub fn window(&self, dir: Vec3f, cover: f32, behind: Vec3f, sky: Vec3f) -> Vec3f {
        let glass = self.glass(dir, cover);
        behind * (1.0 - glass) + self.through_glass(dir, glass, sky)
    }

    /// How much of a window filled with sky is glass at `dir`, 0..=1: where
    /// its shape covers (`cover`) and its gobo passes. The rest is its frame
    /// and bars, or not the window at all.
    pub fn glass(&self, dir: Vec3f, cover: f32) -> f32 {
        (cover * self.passes(dir)).max(0.0).min(1.0)
    }

    /// The sky a window lets in at `dir` through `glass` of glass: `sky` at
    /// 2^intensity_ev, times the face's wear. What the window adds to the map,
    /// and so what it carries as the key (D1).
    pub fn through_glass(&self, dir: Vec3f, glass: f32, sky: Vec3f) -> Vec3f {
        sky * (self.gain * glass * self.glow(dir))
    }

    /// The share of a key light's emission that gets past everything in front
    /// of it, times the face's own wear, for the engine's key light: the mean
    /// of `passes x glow`, weighted by what the light puts into the map along
    /// each direction, `drawn(dir)` (its own drawing in luminance, under
    /// whatever dims it in the map), and by dΩ, over the box within `reach`
    /// of `frame`'s centre in its tangent plane, which must hold all of that
    /// drawing. The gnomonic plane's dΩ is dA / (1 + |q|²)^(3/2).
    ///
    /// Weighted, because the map draws the light's emission times what
    /// passes: a disc leaves its rectangle's corners dark, a hotspot its
    /// edges, and a fern's fronds crowd the bottom edge. An unweighted mean
    /// over the rectangle misses the key by up to a factor of two there (a
    /// fern over a hotspot).
    ///
    /// The wear (B10) has mean 1 over the light's rectangle, so it keeps the
    /// key of a light that glows evenly over it. Weighted by a hotspot, a
    /// ring, a scrim's glow or a stroke, or by what a gobo lets through, its
    /// mean is not 1: the map drew such keys up to 9 % off the clean key (a
    /// stroke with full wrinkles behind leaves). So the wear is sampled with
    /// what passes, and the share can be a little above 1.
    ///
    /// A light can fill little of its box (a thin stroke in the square round
    /// its cone), so a scan first finds the part it draws in, grown by one
    /// scan cell so that an edge between two scan points stays inside, and
    /// the strata cover only that. A drawing so thin that the scan misses it
    /// keeps the whole box.
    ///
    /// 1 when nothing hangs in front of the light and its face is clean, or
    /// when it draws nothing.
    pub fn key_share(&self, frame: &LightFrame, reach: Vec2f, drawn: &dyn Fn(Vec3f) -> f32) -> f32 {
        if !self.has_opaque() && !self.has_surface() {
            return 1.0;
        }
        let dir_at = |q: Vec2f| (frame.center + frame.right * q.x + frame.up * q.y).normalize();
        let scan = KEY_SCAN as f32;
        let cell = vec2f(2.0 * reach.x / scan, 2.0 * reach.y / scan);
        let (mut lo, mut hi) = (reach, -reach);
        for j in 0..KEY_SCAN {
            for i in 0..KEY_SCAN {
                let q = vec2f(-reach.x + (i as f32 + 0.5) * cell.x, -reach.y + (j as f32 + 0.5) * cell.y);
                if drawn(dir_at(q)) > 0.0 {
                    lo = vec2f(lo.x.min(q.x - cell.x), lo.y.min(q.y - cell.y));
                    hi = vec2f(hi.x.max(q.x + cell.x), hi.y.max(q.y + cell.y));
                }
            }
        }
        (lo, hi) = (vec2f(lo.x.max(-reach.x), lo.y.max(-reach.y)), vec2f(hi.x.min(reach.x), hi.y.min(reach.y)));
        if !(lo.x < hi.x && lo.y < hi.y) {
            (lo, hi) = (-reach, reach);
        }
        let n = if self.aim.as_ref().is_some_and(|a| a.has_grid()) { KEY_SAMPLES_GRID } else { KEY_SAMPLES };
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for j in 0..n {
            for i in 0..n {
                let jx = hash01(i, j, 0, KEY_JITTER_SEED);
                let jy = hash01(i, j, 1, KEY_JITTER_SEED);
                let q = vec2f(
                    lo.x + (hi.x - lo.x) * (i as f32 + jx) / n as f32,
                    lo.y + (hi.y - lo.y) * (j as f32 + jy) / n as f32,
                );
                let dir = dir_at(q);
                let w = drawn(dir) * (1.0 + q.x * q.x + q.y * q.y).powf(-1.5);
                // Where the light draws nothing, what passes does not matter.
                if !(w > 0.0) {
                    continue;
                }
                sum += (w * self.passes(dir) * self.glow(dir)) as f64;
                weight += w as f64;
            }
        }
        let share = (sum / weight) as f32;
        if !(weight > 0.0 && share.is_finite()) {
            return 1.0;
        }
        // What passes is at most 1; only the wear lifts the share above it.
        share.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::studio::Studio;
    use crate::sky::luminance;

    fn studio(lights: Vec<LightParams>) -> HdriParams {
        let mut p = HdriParams::default();
        p.mode = "studio".to_string();
        p.lights = lights;
        p
    }

    /// A flat (no hotspot) 40 x 30 degree light at EV 0, due east and a
    /// little above the horizon.
    fn front_light() -> LightParams {
        LightParams {
            azimuth_deg: 90.0,
            elevation_deg: 5.0,
            width_deg: 40.0,
            height_deg: 30.0,
            hotspot: 0.0,
            softness: 0.1,
            intensity_ev: 0.0,
            ..LightParams::default()
        }
    }

    #[test]
    fn a_plain_light_is_left_alone() {
        let fx = LightFx::new(&front_light(), &ImageSet::default());
        assert!(fx.is_plain() && !fx.fills_with_sky());
        let d = dir_from_az_el(90.0, 5.0);
        let (before, after) = (vec3f(0.1, 0.2, 0.3), vec3f(0.6, 1.2, 2.3));
        assert_eq!(fx.apply(d, before, after), after);
        assert_eq!(fx.passes(d), 1.0);
        assert_eq!(fx.key_share(&fx.frame, vec2f(1.0, 1.0), &|_| 1.0), 1.0);
    }

    #[test]
    fn face_coordinates_run_right_and_up_as_seen() {
        let mut light = front_light();
        light.roll_deg = 25.0;
        let fx = LightFx::new(&light, &ImageSet::default());
        let (hx, hy) = half_extents(&light);
        // B2's unit box and the face agree, roll and all.
        for u in [[0.0, 0.0], [0.5, -0.25], [-1.0, 1.0]] {
            let q = fx.face(unit_to_dir(&light, u)).expect("in front of the light");
            assert!((q.x - u[0] * hx).abs() < 1.0e-4 && (q.y - u[1] * hy).abs() < 1.0e-4, "{u:?} -> {q:?}");
        }
        // Facing a light due north, east (a larger azimuth) is on the right.
        let north = LightParams { azimuth_deg: 0.0, elevation_deg: 0.0, ..front_light() };
        let fx = LightFx::new(&north, &ImageSet::default());
        let q = fx.face(dir_from_az_el(5.0, 0.0)).expect("in front of the light");
        assert!(q.x > 0.0 && q.y.abs() < 1.0e-5, "{q:?}");
        assert!(fx.face(dir_from_az_el(0.0, 5.0)).expect("in front").y > 0.0);
        assert!(fx.face(dir_from_az_el(180.0, 0.0)).is_none(), "behind the viewer");
    }

    #[test]
    fn a_gobo_never_adds_light() {
        let mut barred = front_light();
        barred.gobo = Some(GoboParams { panes_x: 4, panes_y: 3, bar: 0.2, ..GoboParams::default() });
        let open = Env::new(&studio(vec![front_light()])).bake(64);
        let shaded = Env::new(&studio(vec![barred])).bake(64);
        let mut darker = 0;
        for (o, s) in open.data.iter().zip(shaded.data.iter()) {
            for k in 0..3 {
                assert!(s[k] <= o[k] * (1.0 + 1.0e-5) + 1.0e-6, "a gobo added light: {} > {}", s[k], o[k]);
                if s[k] < o[k] - 1.0e-4 {
                    darker += 1;
                }
            }
        }
        assert!(darker > 0, "the frame and the bars took out no light at all");
    }

    #[test]
    fn a_window_filled_with_sky_shows_the_sky() {
        let mut window = front_light();
        window.gobo = Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() });
        let inside = Env::new(&studio(vec![window.clone()]));
        // The same parameters in Sky mode: the sky the window looks out at.
        let outside = Env::new(&HdriParams::default());
        let d = dir_from_az_el(90.0, 5.0);
        let (seen, sky) = (inside.radiance(d), outside.radiance(d));
        assert!(luminance(sky) > 1.0e-4, "the test needs a lit sky");
        assert!((seen - sky).length() <= 0.01 * sky.length(), "the window shows {seen:?}, the sky is {sky:?}");
        // One stop up, the sky through the window is twice as bright.
        window.intensity_ev = 1.0;
        let brighter = Env::new(&studio(vec![window])).radiance(d);
        assert!((brighter - sky * 2.0).length() <= 0.01 * sky.length() * 2.0, "{brighter:?}");
        // Beside the window the studio's backdrop is untouched.
        let beside = dir_from_az_el(180.0, 5.0);
        assert_eq!(inside.radiance(beside), Env::new(&studio(Vec::new())).radiance(beside));
    }

    #[test]
    fn the_sun_through_a_window_keeps_its_energy() {
        // A 4 degree sun is smaller than a texel of a 64 x 32 bake (5.6
        // degrees); the bake area-averages the texels on it (A1's hot spots).
        // A window that looks at the sun must get the same treatment.
        let mut sky_params = HdriParams::default();
        sky_params.sky.sun_disc.size_deg = 4.0;
        let outside = Env::new(&sky_params);
        let sun = outside.sun_dir().expect("sky mode has a sun");
        let (azimuth_deg, elevation_deg) = az_el_from_dir(sun);
        let window = LightParams {
            azimuth_deg,
            elevation_deg,
            width_deg: 40.0,
            height_deg: 40.0,
            softness: 0.0,
            gobo: Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() }),
            ..front_light()
        };
        let mut studio_params = sky_params.clone();
        studio_params.mode = "studio".to_string();
        studio_params.lights = vec![window];
        let through = Env::new(&studio_params).bake(64);
        let sky = outside.bake(64);
        let uv = ibl::dir_to_equirect_uv(arr(sun));
        let texel = ((uv[1] * 32.0) as usize).min(31) * 64 + ((uv[0] * 64.0) as usize).min(63);
        let (a, b) = (through.data[texel][1], sky.data[texel][1]);
        assert!(b > 10.0, "the sun's texel is bright: {b}");
        assert!((a - b).abs() <= 0.01 * b, "through the window {a}, outside {b}");
    }

    /// D1 (L:136): a window filled with sky as the key hands the engine the
    /// sky it lets in, not its own Kelvin colour: the key is as blue as the
    /// sky through the glass. Its lighting copy keeps the glass and lets no
    /// sky in, as a scrim key keeps its cloth, so what is behind the glass
    /// stays hidden and the copy lacks exactly the light the key carries.
    #[test]
    fn a_window_key_carries_the_sky_it_lets_in() {
        let mut window = front_light();
        window.key = true;
        window.kelvin = 2000.0;
        window.gobo = Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() });
        let p = studio(vec![window]);
        let key = Env::new(&p).sun().expect("the window is the key");
        assert!(key.validate().is_ok(), "{key:?}");
        assert!(key.radiance.z > key.radiance.x, "the sky's blue, not 2000 K: {:?}", key.radiance);
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f);
        let baked = crate::hdri::envmap::bake_generated_env_map(&p, 64, serial);
        let keyless = baked.keyless.expect("a keyless copy");
        let centre = arr(dir_from_az_el(90.0, 5.0));
        let rgb = |t: [f32; 4]| vec3f(t[0], t[1], t[2]);
        assert!(luminance(rgb(baked.map.sample(centre))) > 0.0, "the window shows the sky");
        // The glass passes 1 to the float's rounding: a few 1e-8 of the backdrop at most.
        let behind = luminance(rgb(baked.map.sample(arr(dir_from_az_el(180.0, 5.0)))));
        assert!(luminance(rgb(keyless.sample(centre))) < 1.0e-6 * behind, "the glass hides the backdrop and lets no sky in: {:?}", keyless.sample(centre));
    }

    #[test]
    fn a_gobo_dims_the_key_light_by_what_it_blocks() {
        let mut key = front_light();
        key.key = true;
        let open = Env::new(&studio(vec![key.clone()])).sun().expect("a key light").radiance;
        key.gobo = Some(GoboParams { panes_x: 3, panes_y: 3, bar: 0.4, ..GoboParams::default() });
        let barred = Env::new(&studio(vec![key])).sun().expect("a key light").radiance;
        // 3 x 3 panes with wide bars and the default frame pass about 53 %.
        let share = luminance(barred) / luminance(open);
        assert!(share > 0.4 && share < 0.65, "{share}");
    }

    /// What the map draws of a key over what the engine's key light carries:
    /// ∫ (map - the map without its key) dΩ, in luminance, on a midpoint grid
    /// over the key's covering cone (which holds all of its drawing, N3),
    /// over radiance x 2π(1 - cos_radius).
    fn map_over_key(light: &LightParams) -> f32 {
        map_over_key_among(vec![light.clone()])
    }

    /// `map_over_key` for a key among other lights (a flag after it).
    fn map_over_key_among(lights: Vec<LightParams>) -> f32 {
        let p = studio(lights);
        let studio = Studio::new(&p.studio, &p.lights);
        let key = studio.key().expect("a key light");
        let (azimuth_deg, elevation_deg) = az_el_from_dir(key.dir);
        let frame = light_frame(azimuth_deg, elevation_deg, 0.0);
        let c = key.cos_cover.max(0.05);
        let reach = (1.0 - c * c).sqrt() / c;
        let n = 256;
        let cell = 2.0 * reach / n as f32;
        let black = Vec3f::default();
        let mut sum = 0.0f64;
        for j in 0..n {
            let y = -reach + (j as f32 + 0.5) * cell;
            for i in 0..n {
                let x = -reach + (i as f32 + 0.5) * cell;
                let dir = (frame.center + frame.right * x + frame.up * y).normalize();
                let drawn = luminance(studio.apply_lights(dir, black)) - luminance(studio.apply_lights_without_key(dir, black));
                sum += (drawn * (1.0 + x * x + y * y).powf(-1.5)) as f64;
            }
        }
        (sum * (cell * cell) as f64) as f32 / luminance(key.irradiance())
    }

    /// A gobo takes out of the engine's key light what it takes out of the
    /// map: what passes, weighted by what the light itself draws there (a
    /// disc's or a stroke's outline, a hotspot), not the plain mean over its
    /// rectangle. A fern's fronds crowd its bottom edge, where a hotspot key
    /// is dim, and leaves fall where their seed puts them.
    #[test]
    fn a_keys_gobo_takes_from_the_key_what_it_takes_from_the_map() {
        let key = LightParams {
            key: true,
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
            height_deg: 45.0,
            ..LightParams::default()
        };
        let keys = [
            ("disc", LightParams { shape: "disc".to_string(), ..key.clone() }),
            ("hotspot 1 rect", LightParams { hotspot: 1.0, ..key.clone() }),
            ("ring", LightParams { shape: "ring".to_string(), softness: 0.15, ..key.clone() }),
            ("stroke", LightParams { shape: "stroke".to_string(), ..key }),
        ];
        let gobos = [
            ("leaves", GoboParams { kind: "leaves".to_string(), density: 0.4, distance: 0.3, seed: 2, ..GoboParams::default() }),
            ("fern", GoboParams { kind: "fern".to_string(), density: 0.4, distance: 0.3, seed: 1, ..GoboParams::default() }),
            ("window", GoboParams::default()),
        ];
        let mut off = Vec::new();
        for (key_name, light) in &keys {
            // The plain key's own ratio, so the test weighs only the gobo.
            let plain = map_over_key(light);
            for (gobo_name, gobo) in &gobos {
                let ratio = map_over_key(&LightParams { gobo: Some(gobo.clone()), ..light.clone() }) / plain;
                println!("{key_name} behind {gobo_name}: {ratio:.4}");
                if (ratio - 1.0).abs() > 0.02 {
                    off.push(format!("{key_name} behind {gobo_name}: the map draws {ratio:.4} of what the key carries"));
                }
            }
        }
        assert!(off.is_empty(), "{}", off.join("\n"));
    }

    /// Plan 2's N3 with a gobo on the key: the engine's lighting copy (the
    /// keyless bake) is the map with the key switched off, its gobo with it:
    /// `apply_lights_sky` leaves the key out under the gobo too.
    #[test]
    fn a_keys_gobo_leaves_the_lighting_copy_with_the_key() {
        let mut key = front_light();
        key.key = true;
        key.gobo = Some(GoboParams { panes_x: 3, panes_y: 3, bar: 0.4, ..GoboParams::default() });
        let p = studio(vec![key]);
        let baked = crate::hdri::envmap::bake_generated_env_map(&p, 64, |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f));
        let mut off = p.clone();
        off.lights[0].enabled = false;
        assert!(baked.keyless.as_ref() == Some(&Env::new(&off).bake(64)), "the keyless copy is the map without the barred key");
    }

    #[test]
    fn a_flag_with_a_gobo_only_darkens_where_the_gobo_is_opaque() {
        let flag = LightParams {
            elevation_deg: 20.0,
            width_deg: 60.0,
            height_deg: 40.0,
            blend: "multiply".to_string(),
            rgb: Some([0.0, 0.0, 0.0]),
            ..front_light()
        };
        let mut cut = flag.clone();
        cut.gobo = Some(GoboParams { kind: "blinds".to_string(), slats: 6, tilt_deg: 30.0, ..GoboParams::default() });
        // Sky mode: the flag hangs against the sky.
        let bake_with = |light: Option<LightParams>| {
            let mut p = HdriParams::default();
            p.lights = light.into_iter().collect();
            Env::new(&p).bake(64)
        };
        let (sky, solid, blinds) = (bake_with(None), bake_with(Some(flag)), bake_with(Some(cut)));
        let mut through = 0;
        for i in 0..sky.data.len() {
            for k in 0..3 {
                let (open, dark, slats) = (sky.data[i][k], solid.data[i][k], blinds.data[i][k]);
                assert!(slats <= open * (1.0 + 1.0e-5) + 1.0e-6, "a flag never brightens");
                assert!(slats + 1.0e-6 >= dark, "the cut flag is never darker than the solid one");
                if slats > dark + 1.0e-4 {
                    through += 1;
                }
            }
        }
        assert!(through > 0, "the sky shows between the slats");
    }

    /// A flag cut by a gobo takes out of the engine's key light what it
    /// takes out of the map: only its gobo's opaque parts dim the key, in
    /// the key's emission (`key_emission`) and in what weighs the key's own
    /// gobo (`key_share`) alike, as `apply_lights` draws the flag.
    #[test]
    fn a_cut_flag_dims_the_key_as_it_dims_the_map() {
        let key = LightParams {
            key: true,
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
            height_deg: 45.0,
            ..LightParams::default()
        };
        // A black flag over the key's right half and past its top.
        let flag = LightParams {
            azimuth_deg: 15.0,
            elevation_deg: 25.0,
            width_deg: 40.0,
            height_deg: 40.0,
            blend: "multiply".to_string(),
            rgb: Some([0.0, 0.0, 0.0]),
            ..front_light()
        };
        let blinds = GoboParams { kind: "blinds".to_string(), slats: 6, tilt_deg: 30.0, ..GoboParams::default() };
        let leaves = GoboParams { kind: "leaves".to_string(), density: 0.4, distance: 0.3, seed: 2, ..GoboParams::default() };
        let cases = [
            ("solid flag", key.clone(), flag.clone()),
            ("flag cut by blinds", key.clone(), LightParams { gobo: Some(blinds.clone()), ..flag.clone() }),
            ("flag cut by a window", key.clone(), LightParams { gobo: Some(GoboParams::default()), ..flag.clone() }),
            ("key behind leaves, flag cut by blinds", LightParams { gobo: Some(leaves.clone()), ..key.clone() }, LightParams { gobo: Some(blinds), ..flag }),
        ];
        let mut off = Vec::new();
        for (name, key_light, flag_light) in cases {
            // The key's own ratio without the flag, so the test weighs only the flag.
            let alone = map_over_key(&key_light);
            let ratio = map_over_key_among(vec![key_light, flag_light]) / alone;
            println!("{name}: {ratio:.4}");
            if (ratio - 1.0).abs() > 0.02 {
                off.push(format!("{name}: the map draws {ratio:.4} of what the key carries"));
            }
        }
        assert!(off.is_empty(), "{}", off.join("\n"));
    }

    #[test]
    fn a_grid_turned_away_dims_the_key() {
        let mut key = front_light();
        key.key = true;
        let open = Env::new(&studio(vec![key.clone()])).sun().expect("a key light").radiance;
        key.aim = Some(AimParams { yaw_deg: 30.0, grid: 1.0, ..AimParams::default() });
        let fx = LightFx::new(&key, &ImageSet::default());
        assert!(!fx.is_plain());
        let gridded = Env::new(&studio(vec![key.clone()])).sun().expect("a key light").radiance;
        assert!(luminance(gridded) < 0.05 * luminance(open), "a 15 degree grid turned 30 degrees away: {gridded:?}");
        // The aim leaves a flag alone.
        key.blend = "multiply".to_string();
        let flag = LightFx::new(&key, &ImageSet::default());
        let d = dir_from_az_el(90.0, 5.0);
        assert_eq!(flag.apply(d, vec3f(1.0, 1.0, 1.0), vec3f(0.2, 0.2, 0.2)), vec3f(0.2, 0.2, 0.2));
    }

    /// The doors and the grid take out of the engine's key light what they
    /// take out of the map, as a gobo does. A grid turned well off its axis
    /// passes only a small spot of each cell, which the key's strata must
    /// still find.
    #[test]
    fn a_keys_grid_and_doors_take_from_the_key_what_they_take_from_the_map() {
        let key = LightParams {
            key: true,
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
            height_deg: 45.0,
            ..LightParams::default()
        };
        let keys = [
            ("rect", key.clone()),
            ("hotspot 1 rect", LightParams { hotspot: 1.0, ..key.clone() }),
            ("disc", LightParams { shape: "disc".to_string(), ..key.clone() }),
            ("stroke", LightParams { shape: "stroke".to_string(), ..key }),
        ];
        let aims = [
            ("grid 0.3 turned 10", AimParams { grid: 0.3, yaw_deg: 10.0, ..AimParams::default() }),
            ("grid 1", AimParams { grid: 1.0, ..AimParams::default() }),
            ("grid 0.6 tilted -20", AimParams { grid: 0.6, pitch_deg: -20.0, ..AimParams::default() }),
            ("top door shut", AimParams { barn_doors: [90.0, 0.0, 0.0, 0.0], ..AimParams::default() }),
            ("top door out, tilted -30", AimParams { pitch_deg: -30.0, barn_doors: [45.0, 0.0, 0.0, 0.0], ..AimParams::default() }),
            ("side doors and grid 0.2", AimParams { grid: 0.2, barn_doors: [0.0, 60.0, 0.0, 90.0], ..AimParams::default() }),
        ];
        let mut off = Vec::new();
        for (key_name, light) in &keys {
            // The plain key's own ratio, so the test weighs only the aim.
            let plain = map_over_key(light);
            for (aim_name, aim) in &aims {
                let ratio = map_over_key(&LightParams { aim: Some(aim.clone()), ..light.clone() }) / plain;
                if (ratio - 1.0).abs() > 0.02 {
                    off.push(format!("{key_name} with {aim_name}: the map draws {ratio:.4} of what the key carries"));
                }
            }
        }
        assert!(off.is_empty(), "{}", off.join("\n"));
    }

    #[test]
    fn imperfections_keep_the_light_s_power() {
        // A big light, so a 64 x 32 bake samples its face a few hundred times.
        let plain = LightParams { width_deg: 120.0, height_deg: 90.0, elevation_deg: 0.0, ..front_light() };
        let mut worn = plain.clone();
        worn.imperfections = Some(ImperfectionParams { wrinkle: 0.5, dust: 0.5, weave: 0.5, seed: 5 });
        // Power = radiance x solid angle over the map; the backdrop cancels out.
        let power = |lights: Vec<LightParams>| {
            let map = Env::new(&studio(lights)).bake(64);
            let mut sum = 0.0f64;
            for y in 0..map.height {
                let theta = std::f64::consts::PI * (y as f64 + 0.5) / map.height as f64;
                for x in 0..map.width {
                    sum += map.data[y * map.width + x][1] as f64 * theta.sin();
                }
            }
            sum
        };
        let backdrop = power(Vec::new());
        let a = power(vec![plain.clone()]) - backdrop;
        let b = power(vec![worn.clone()]) - backdrop;
        assert!(a > 0.0);
        assert!((b / a - 1.0).abs() < 0.03, "plain {a}, worn {b}");
        // The key light is the mean, and the mean is kept: on a light that
        // glows evenly over its face the worn key is the clean one to within
        // the sampling of its share (`LightFx::key_share`, which weighs the
        // wear as the map draws it).
        let key = |mut light: LightParams| {
            light.key = true;
            Env::new(&studio(vec![light])).sun().expect("a key light").radiance
        };
        let kept = luminance(key(worn.clone())) / luminance(key(plain));
        assert!((kept - 1.0).abs() < 0.01, "the worn key carries {kept} of the clean one");
        // The map itself does differ: the wear is there.
        let fx = LightFx::new(&worn, &ImageSet::default());
        assert!(!fx.is_plain());
        let glows: Vec<f32> = (0..50).map(|k| fx.glow(dir_from_az_el(60.0 + k as f32 * 1.2, 3.0))).collect();
        assert!(glows.iter().any(|g| *g > 1.05) && glows.iter().any(|g| *g < 0.95), "{glows:?}");
    }

    /// The wear has mean 1 over the light's rectangle, but a key that does
    /// not glow evenly over it (a hotspot, a ring, a scrim's glow, a stroke)
    /// or that a gobo cuts weighs it unevenly: the map draws the key times
    /// its wear, and the engine's key light must carry that, not the clean
    /// key (plan 2's N3 removes what the map draws).
    #[test]
    fn a_worn_key_hands_the_engine_what_the_map_draws() {
        let key = LightParams {
            key: true,
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
            height_deg: 45.0,
            ..LightParams::default()
        };
        let keys = [
            ("hotspot 1 rect", LightParams { hotspot: 1.0, ..key.clone() }),
            ("ring", LightParams { shape: "ring".to_string(), softness: 0.15, ..key.clone() }),
            ("scrim", LightParams { shape: "scrim".to_string(), ..key.clone() }),
            ("stroke", LightParams { shape: "stroke".to_string(), ..key }),
        ];
        let leaves = GoboParams { kind: "leaves".to_string(), density: 0.4, distance: 0.3, seed: 2, ..GoboParams::default() };
        let wears = [
            ("wrinkles", ImperfectionParams { wrinkle: 1.0, seed: 9, ..ImperfectionParams::default() }),
            ("worn", ImperfectionParams { wrinkle: 0.8, dust: 0.4, weave: 0.5, seed: 1 }),
        ];
        let mut off = Vec::new();
        for (key_name, light) in &keys {
            for (gobo_name, gobo) in [("bare", None), ("behind leaves", Some(leaves.clone()))] {
                let clean = LightParams { gobo, ..light.clone() };
                // The clean key's own ratio, so the test weighs only the wear.
                let plain = map_over_key(&clean);
                for (wear_name, wear) in &wears {
                    let ratio = map_over_key(&LightParams { imperfections: Some(wear.clone()), ..clean.clone() }) / plain;
                    if (ratio - 1.0).abs() > 0.02 {
                        off.push(format!("{key_name} {gobo_name}, {wear_name}: the map draws {ratio:.4} of what the key carries"));
                    }
                }
            }
        }
        assert!(off.is_empty(), "{}", off.join("\n"));
    }
}
