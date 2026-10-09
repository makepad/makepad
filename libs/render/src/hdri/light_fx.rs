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
use super::gobo::Gobo;
use super::noise::hash01;
use super::shaped::{half_extents, unit_to_dir};
use super::studio::{light_frame, project, LightFrame};

/// Strata per axis for the key light's mean. Jittered, because a regular
/// grid beats against panes, slats and grid cells.
const KEY_SAMPLES: i32 = 48;
/// The jitter's seed: fixed, so the same light always gives the same key.
const KEY_JITTER_SEED: u32 = 0x6b65_7921;

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
    /// Tangent half extents of the light's rectangle.
    half: Vec2f,
    blend: Blend,
    /// 2^intensity_ev: the gain a window filled with sky shows the sky at.
    gain: f32,
    fill_sky: bool,
    gobo: Option<Gobo>,
    /// The mean of `passes` over the light's rectangle, for the key light.
    key_scale: f32,
}

impl LightFx {
    /// The extras of `light`. `images` resolves image gobos (B8); a name the
    /// app has not loaded is simply no gobo, so this never reads a file.
    pub fn new(light: &LightParams, images: &ImageSet) -> LightFx {
        let (frame, sx, half) = face_axes(light);
        let gobo = light.gobo.as_ref();
        let mut fx = LightFx {
            frame,
            sx,
            half,
            blend: light.blend(),
            gain: 2.0f32.powf(light.intensity_ev.max(-10.0).min(20.0)),
            fill_sky: gobo.is_some_and(|g| g.fill_sky),
            gobo: gobo.and_then(|g| Gobo::new(g, half, images)),
            key_scale: 1.0,
        };
        fx.key_scale = fx.mean_passes();
        fx
    }

    /// Something opaque hangs in front of the face.
    fn has_opaque(&self) -> bool {
        self.gobo.is_some()
    }

    /// The face itself is worn (B10).
    fn has_surface(&self) -> bool {
        false
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

    /// The direction through face coordinates `q` (the inverse of `face`).
    fn dir_at(&self, q: Vec2f) -> Vec3f {
        (self.frame.center + self.frame.right * (self.sx * q.x) + self.frame.up * q.y).normalize()
    }

    /// The share 0..=1 of an emitting face that shows at `dir` past
    /// everything opaque in front of it. 1 where the frame cannot place `dir`.
    pub fn passes(&self, dir: Vec3f) -> f32 {
        let Some(q) = self.face(dir) else {
            return 1.0;
        };
        let mut t = 1.0;
        if let Some(gobo) = &self.gobo {
            t *= gobo.transmission(q);
        }
        t
    }

    /// The brightness of the face itself at `dir`, around 1 with mean 1 over
    /// the face (B10's imperfections). 1 until then.
    pub fn glow(&self, _dir: Vec3f) -> f32 {
        1.0
    }

    /// The share 0..=1 of a flag that darkens at `dir`. A flag is solid
    /// until B8 lets a gobo cut it.
    fn blocks(&self, _dir: Vec3f) -> f32 {
        1.0
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
        let glass = (cover * self.passes(dir)).max(0.0).min(1.0);
        behind * (1.0 - glass) + sky * (self.gain * glass * self.glow(dir))
    }

    /// The share of the light that gets past everything in front of it, for
    /// the engine's key light.
    pub fn key_scale(&self) -> f32 {
        self.key_scale
    }

    /// The solid-angle weighted mean of `passes` over the light's rectangle.
    /// The gnomonic plane's dΩ is dA / (1 + |q|²)^(3/2). Wear averages to 1
    /// by construction, so it needs no sampling.
    fn mean_passes(&self) -> f32 {
        if !self.has_opaque() {
            return 1.0;
        }
        let n = KEY_SAMPLES;
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for j in 0..n {
            for i in 0..n {
                let jx = hash01(i, j, 0, KEY_JITTER_SEED);
                let jy = hash01(i, j, 1, KEY_JITTER_SEED);
                let q = vec2f(
                    ((i as f32 + jx) / n as f32 * 2.0 - 1.0) * self.half.x,
                    ((j as f32 + jy) / n as f32 * 2.0 - 1.0) * self.half.y,
                );
                let w = (1.0 + q.x * q.x + q.y * q.y).powf(-1.5) as f64;
                sum += w * self.passes(self.dir_at(q)) as f64;
                weight += w;
            }
        }
        ((sum / weight.max(1.0e-12)) as f32).max(0.0).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(fx.key_scale(), 1.0);
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
}
