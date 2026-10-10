//! Studio lighting: the backdrop gradient and the light list.
//!
//! A map is a base layer (this backdrop, or the outdoor sky) with an ordered list
//! of lights evaluated over it. Each light lives in its own angular frame (centre,
//! right, up), and its shape is a signed distance in that frame's gnomonic
//! tangent plane. A softbox therefore keeps its shape at the zenith instead of
//! smearing the way an equirect rectangle would.
//!
//! The frame is `ibl::softbox`'s (libs/render_material/src/ibl.rs:414-432):
//! `right = cross(+Y, center)`, `up = cross(center, right)`. The shapes generalise
//! that softbox and the knob's `rect_d` (apps/storybook/src/knob/bake.rs:697):
//! - a rounded rect;
//! - a disc (an ellipse when width != height);
//! - a ring, `abs(r - R) - w`;
//!
//! with a roll, a soft edge of any width, a hotspot and an Add or Multiply blend.
//!
//! All colours are linear Rec.709. A light's colour is its peak radiance: the
//! Kelvin/tint colour (luminance 1) or an explicit rgb, times 2^intensity_ev.

use makepad_draw::*;
use super::{az_el_from_dir, dir_from_az_el, Blend, EnvSun, LightParams, LightShape, StudioParams};
use crate::sky::luminance;
use super::shaped::{stroke_box, ShapedLight};
use super::shape::ArcPath;
use super::noise::hash01;
use super::gels::gel_stack;
use super::spectrum::light_source_rgb;
use super::image_light::{ImageSet, LightImage};
use super::light_fx::LightFx;

/// Strength of a full ±1 tint: at +1 green is cut to about a third of red and blue.
const TINT_STRENGTH: f32 = 0.4;

/// Clamps into [lo, hi]. A NaN becomes `lo`, because `f32::max` ignores NaN; this
/// is unlike `f32::clamp`, which passes NaN through.
fn sane(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}

/// Linear Rec.709 colour of a blackbody at `kelvin` (1000..=20000), scaled to luminance 1.
///
/// **Source of the fit.** The chromaticity comes from Krystek's rational fit of the
/// Planckian locus in CIE 1960 (u, v): M. Krystek, "An algorithm to calculate
/// correlated colour temperature", Color Research & Application 10(1), 1985. The
/// coefficients are as reproduced in Wikipedia's "Planckian locus" article.
///
/// **Accuracy.** The fit is within about 1e-4 in (u, v) from 1000 K to 15000 K.
/// Extrapolated to 20000 K it drifts by about 5e-4 (measured against the Robertson
/// 1968 isotemperature table and a cubic-spline locus), which cannot be seen at that
/// blue end.
///
/// **Conversion.** (u, v) goes to (x, y), then to XYZ with Y = 1, then to linear
/// Rec.709 through the same XYZ matrix sky.rs uses.
///
/// **Below about 1900 K** the locus leaves the Rec.709 gamut and blue goes
/// negative. Blue is clipped to 0 and the colour rescaled, so the luminance stays 1.
pub fn kelvin_to_rgb(kelvin: f32) -> Vec3f {
    let t = sane(kelvin, 1000.0, 20000.0);
    let t2 = t * t;
    let u = (0.860_117_73 + 1.541_182_6e-4 * t + 1.286_412_2e-7 * t2)
        / (1.0 + 8.424_202e-4 * t + 7.081_451_4e-7 * t2);
    let v = (0.317_398_73 + 4.228_062_6e-5 * t + 4.204_816_8e-8 * t2)
        / (1.0 - 2.897_418_2e-5 * t + 1.614_560_6e-7 * t2);
    let d = 2.0 * u - 8.0 * v + 4.0;
    let x = 3.0 * u / d;
    let y = 2.0 * v / d;
    let big_x = x / y;
    let big_z = (1.0 - x - y) / y;
    let rgb = vec3f(
        (3.2406 * big_x - 1.5372 - 0.4986 * big_z).max(0.0),
        (-0.9689 * big_x + 1.8758 + 0.0415 * big_z).max(0.0),
        (0.0557 * big_x - 0.2040 + 1.0570 * big_z).max(0.0),
    );
    rgb / luminance(rgb).max(1.0e-6)
}

/// A green/magenta correction as an RGB multiplier.
/// - -1 is a strong plus-green, +1 a strong minus-green (magenta), 0 is neutral.
/// - It is scaled to luminance 1, so it shifts the hue of a white light without
///   changing its brightness.
pub fn tint_rgb(tint: f32) -> Vec3f {
    let t = sane(tint, -1.0, 1.0) * TINT_STRENGTH;
    let m = vec3f(1.0 + t, 1.0 - t, 1.0 + t);
    m / luminance(m)
}

/// A light's peak radiance, times 2^intensity_ev, behind its gels.
/// - With `rgb` set, that explicit colour is used as given.
/// - Else with a named `source` (1b), the colour is that lamp's spectrum
///   (spectrum.rs) times tint, at luminance 1; the Kelvin is not read, except
///   by the parametric LED (`led`), whose spectrum is built at the Kelvin and
///   the `source_quality` knob.
/// - Otherwise the colour is Kelvin times tint, renormalised to luminance 1 so
///   the tint cannot change the brightness of a coloured light either.
/// - Gels (1b) filter the result, every gel and every light alike, as a sheet
///   in front of a lamp does (gels.rs): a conversion gel is the filter that
///   moves a blackbody by its mireds. So a Kelvin light keeps its colour and
///   level when switched to RGB, and a strong CTB is not stopped at the
///   Kelvin colours' 20000 K end (D10, L:86).
pub fn light_color(light: &LightParams) -> Vec3f {
    let gels = gel_stack(light.gels.as_deref().unwrap_or(&[]));
    let base = if let Some(rgb) = light.rgb {
        vec3f(rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0))
    } else {
        // A named source's spectrum gives the colour in place of the Kelvin's,
        // tinted the same way (the knob LED's Kelvin is part of its spectrum).
        let white = light_source_rgb(light).unwrap_or_else(|| kelvin_to_rgb(sane(light.kelvin, 1000.0, 20000.0)));
        let c = white * tint_rgb(light.tint);
        c / luminance(c).max(1.0e-6)
    };
    base * gels.conversion * gels.filter * 2.0f32.powf(sane(light.intensity_ev, -10.0, 20.0))
}

/// A light's angular frame. `center` points at the light. `right` and `up` span the
/// tangent plane its shape is drawn in; both are unit length and perpendicular to
/// `center`, and `right x up = center`.
#[derive(Clone, Copy, Debug)]
pub struct LightFrame {
    pub center: Vec3f,
    pub right: Vec3f,
    pub up: Vec3f,
}

/// The frame of a light at (azimuth, elevation), turned by `roll_deg`.
///
/// **Unrolled** it is `ibl::softbox`'s basis: `right = normalize(cross(+Y, center))`,
/// `up = cross(center, right)`. `right` is the horizontal direction of decreasing
/// azimuth, which is the viewer's left when facing the light; `up` tilts back over
/// the viewer's head as the light climbs.
///
/// **At the poles** `cross(+Y, center)` vanishes and any perpendicular will do. The
/// azimuth's own horizontal direction is used, which is the limit of the basis as
/// the light climbs, so the frame stays continuous up to the zenith.
///
/// **Roll.** Positive roll turns the shape counter-clockwise as the viewer sees it.
/// Because `right` points to the viewer's left, that turn carries `right` toward
/// `-up`.
pub fn light_frame(azimuth_deg: f32, elevation_deg: f32, roll_deg: f32) -> LightFrame {
    let center = dir_from_az_el(azimuth_deg, elevation_deg);
    let mut right = Vec3f::cross(vec3f(0.0, 1.0, 0.0), center);
    if right.length_squared() < 1.0e-8 {
        let (sin_az, cos_az) = azimuth_deg.to_radians().sin_cos();
        right = vec3f(-cos_az, 0.0, -sin_az);
    }
    let right = right.normalize();
    let up = Vec3f::cross(center, right);
    let (sin_r, cos_r) = roll_deg.to_radians().sin_cos();
    LightFrame {
        center,
        right: right * cos_r - up * sin_r,
        up: up * cos_r + right * sin_r,
    }
}

/// Gnomonic tangent-plane coordinates of `dir` in `frame`: `dir`'s right and up
/// components divided by its component along the centre. Great circles through the
/// frame map to straight lines, so a rect keeps straight sides.
///
/// Returns None when `dir` is more than about 87 degrees from the centre (dot <
/// [`PROJECT_MIN_COS`]), where the projection runs off to infinity.
pub fn project(frame: &LightFrame, dir: Vec3f) -> Option<Vec2f> {
    let c = dir.dot(frame.center);
    // Written negated so a NaN direction is rejected too.
    if !(c >= PROJECT_MIN_COS) {
        return None;
    }
    Some(vec2f(dir.dot(frame.right) / c, dir.dot(frame.up) / c))
}

/// [`project`]'s limit: the smallest dot with the centre it places, about 87
/// degrees out, where the tangent plane is already 20 units from the centre.
pub const PROJECT_MIN_COS: f32 = 0.05;

/// [`project`], held at its limit: a direction further out than the plane
/// reaches comes back on the limit's circle, along its own bearing round the
/// centre, so what is laid in the plane goes on continuously past it. Only a
/// stroke draws that far from its middle (D3, L:180: a grid or a window there
/// stepped to fully bright where the plane ended). None only straight behind
/// the centre, where there is no bearing, and for NaN.
pub fn project_held(frame: &LightFrame, dir: Vec3f) -> Option<Vec2f> {
    if let Some(t) = project(frame, dir) {
        return Some(t);
    }
    let (x, y) = (dir.dot(frame.right), dir.dot(frame.up));
    let bearing = (x * x + y * y).sqrt();
    if !(bearing > 1.0e-6) {
        return None;
    }
    let c = PROJECT_MIN_COS;
    let limit = (1.0 - c * c).sqrt() / c;
    Some(vec2f(x, y) * (limit / bearing))
}

/// Narrowest soft edge, in tangent units (about 0.006 degrees). A "hard" edge still
/// gets this much, so the step is resolved below a pixel and never becomes a
/// stair-step in an 8K bake.
const MIN_EDGE: f32 = 1.0e-4;
/// With hotspot 1 the edge of the shape is e^-4 (about 2%) of its centre.
const HOTSPOT_FALLOFF: f32 = 4.0;
/// Grid points per axis of the key light's integral over its reach box
/// ([`key_emission`]). A grid four times finer moves the built-in keys by under
/// 0.01 % and the worst case in the tests, a hard ring a twentieth of its radius thick,
/// by 0.09 %.
const KEY_GRID: usize = 256;

/// D2: the widest a stroke may spread round its middle and still be the key,
/// in degrees: the widest key cone a rect or a disc gets (half their 170
/// degree limit). A stroke round the whole horizon has no middle to light
/// from, and a ring round the sky lies far out all round its middle: the
/// engine's light would come from straight up, lighting little and no
/// vertical surface, while all of the stroke's light left the map's lighting.
pub const KEY_MAX_SPREAD_DEG: f32 = 85.0;

/// D2: whether `light` is a stroke that spreads wider round its middle than a
/// key of [`KEY_MAX_SPREAD_DEG`] can, so that marked as the key it is not
/// handed to the engine: it stays a light of the map, lighting the scene
/// through the map like any other, and under a sky the sun leads. The panel
/// says so where the key is chosen. False for every other light.
///
/// A stroke spreads that wide in either of two ways, both measured on its
/// centre line round its mean direction (its middle, the key's axis):
/// - it reaches further than the limit from its middle (its widest angle from
///   it, plus its widest half width: the key cone), as a strip round the
///   horizon or a half circle does;
/// - it lies all round its middle further out than the light of an even disc
///   of the limit's radius does: a surface facing its middle receives less of
///   a thin line along it than (1 + cos 85°) / 2 = 0.544, that disc's share.
///   L:41's ring at elevation 10 reaches only 82 degrees, but every point of
///   it is 80 degrees out (0.17); a ring is refused below about elevation 33,
///   and a 160 degree arch, which runs through its middle, gives 0.71.
pub fn spreads_too_wide_to_be_key(light: &LightParams) -> bool {
    ShapedLight::new(&LightParams { enabled: true, ..light.clone() }).is_some_and(|shaped| too_wide_to_lead(&shaped))
}

/// [`spreads_too_wide_to_be_key`] for a light the shape core draws.
fn too_wide_to_lead(shaped: &ShapedLight) -> bool {
    let limit = KEY_MAX_SPREAD_DEG.to_radians().cos();
    let reaches_too_far = shaped.stroke_cones().is_some_and(|(_, cos_radius, _)| !(cos_radius >= limit));
    // An even disc of half-angle r: a surface facing it receives
    // π sin²r / 2π(1 - cos r) = (1 + cos r) / 2 of its light.
    let lies_too_far_out = shaped.stroke_line_facing().is_some_and(|facing| !(facing >= 0.5 * (1.0 + limit)));
    reaches_too_far || lies_too_far_out
}

/// Coarse cells per axis of [`over_cone`]'s grid over a key's covering cone.
const CONE_CELLS: usize = 64;
/// The most samples [`over_cone`] takes in all: past it every cell it refines
/// gets fewer, in proportion.
const CONE_BUDGET: usize = 1 << 18;
/// The most jittered samples per axis one coarse cell takes.
const CONE_FINE_MAX: usize = 64;
/// [`over_cone`]'s jitter seed: fixed, so the same light always gives the same key.
const CONE_JITTER_SEED: u32 = 0x636f_6e65;
/// Samples per stroke reach (half its width, soft edge included) along each
/// axis of a cell a stroke key draws in: a hard edge then averages out along
/// the stroke (a hard straight 4 degree stroke gives 0.08049 of its cone
/// against the exact 0.0805), and a tapering tip is still seen.
const STROKE_SAMPLES_PER_REACH: f32 = 8.0;
/// The most samples per axis a cell of a stroke key takes. A stroke thinner
/// than a cell's side over 2 (0.3 degrees on a 60 degree arch) gets fewer per
/// reach, which the jitter along its length still averages (13 ms for that
/// key in release, against 55 ms at 32).
const STROKE_FINE_MAX: usize = 16;
/// Samples per axis of every cell of a window filled with sky as the key (D1):
/// the sky is smooth, and the glass's frame and bars, hard edges at any
/// angle, average out under the jitter (the default window within 0.1 % of
/// what a 1024 wide map draws).
const WINDOW_FINE: usize = 2;
/// Samples per disc radius along each axis of a cell a disc (the sun or the
/// moon) shows through a window in: the disc is far smaller than a cell.
const DISC_SAMPLES_PER_RADIUS: f32 = 8.0;

/// The backdrop gradient and the enabled lights, prepared once per map.
#[derive(Clone, Debug)]
pub struct Studio {
    top: Vec3f,
    horizon: Vec3f,
    floor: Vec3f,
    /// horizon_softness as degrees of elevation over which the horizon colour hands over.
    soft_deg: f32,
    lights: Vec<PreparedLight>,
    key: Option<EnvSun>,
    /// The key's place in `lights`: the light a lookup without the key skips.
    key_index: Option<usize>,
    /// What hangs in front of each light (1b: gobo, doors and grid, wear),
    /// in step with `lights`.
    fx: Vec<LightFx>,
    /// False while no light has any: `apply_lights` then runs the loop as it was.
    any_fx: bool,
}

impl Studio {
    /// `lights` are taken in list order; disabled ones are dropped here. The key is the
    /// first enabled Add light marked `key`, unless it is a stroke that spreads too wide
    /// to lead (D2, [`spreads_too_wide_to_be_key`]). A flag (Multiply) never leads the
    /// lighting.
    ///
    /// The key carries exactly the light the map draws of it (D1), measured from the
    /// map's own drawing once per map: its shape, soft edge, hotspot, picture and ramp,
    /// what hangs in front of it, and the flags, gels, cloth and windows filled with
    /// sky after it. The key's cone covers the larger of width and height, and its
    /// radiance is that emission spread over the cone ([`EnvSun`]'s one meaning), from
    /// the same integral as the share a surface facing the key receives (`facing`) and
    /// the cone that holds its whole reach (`cos_cover`):
    /// - a light in its own box (a rect, disc, ring, polygon or scrim, with or without a
    ///   picture) by [`key_emission`] over that box, times what passes what hangs in
    ///   front of it (`key_share`);
    /// - a stroke (1b), which its points place, over the cones round its centre line,
    ///   by [`over_cone`], where its band runs;
    /// - a window filled with sky, whose light is the sky, by `Env`, which knows that
    ///   sky (`measured_window_key`): until then `key()` is None.
    ///
    /// No pictures: an image light is drawn plain (see `new_with`).
    pub fn new(studio: &StudioParams, lights: &[LightParams]) -> Studio {
        Studio::new_with(studio, lights, &ImageSet::default(), false)
    }

    /// `new` with the pictures image lights show (`Env::new_with` passes
    /// them through). A light whose key is not in `images` is drawn plain.
    /// `against_sky`: the lights hang against the sky (Sky mode), where a
    /// window filled with sky hides what is behind its frame and bars
    /// (`LightFx::against_sky`, D12).
    pub(super) fn new_with(studio: &StudioParams, lights: &[LightParams], images: &ImageSet, against_sky: bool) -> Studio {
        let rgb = |c: [f32; 3]| vec3f(c[0].max(0.0), c[1].max(0.0), c[2].max(0.0));
        let enabled: Vec<&LightParams> = lights.iter().filter(|l| l.enabled).collect();
        let prepared: Vec<PreparedLight> = enabled.iter().map(|l| PreparedLight::new(l, images)).collect();
        let key_index = enabled
            .iter()
            .zip(&prepared)
            .position(|(l, p)| l.key && l.blend() == Blend::Add && !p.shaped.as_ref().is_some_and(too_wide_to_lead));
        // What hangs in front of each light, built for the same lights in the
        // same order as the prepared lights above. Before the key: a flag's
        // gobo cuts what the flag takes out of the key (B8).
        let fx: Vec<LightFx> = enabled.iter().map(|l| LightFx::new(l, images).against_sky(against_sky)).collect();
        let any_fx = fx.iter().any(|fx| !fx.is_plain());
        let mut built = Studio {
            top: rgb(studio.top),
            horizon: rgb(studio.horizon),
            floor: rgb(studio.floor),
            soft_deg: sane(studio.horizon_softness, 0.01, 1.0) * 90.0,
            lights: prepared,
            key: None,
            key_index,
            fx,
            any_fx,
        };
        built.key = key_index.and_then(|i| built.measured_key(i));
        built
    }

    /// The key light `index` hands the engine, measured from what the map
    /// draws of it (D1). None for a window filled with sky: its light is the
    /// sky, which `Env` measures it against (`measured_window_key`).
    fn measured_key(&self, index: usize) -> Option<EnvSun> {
        let light = &self.lights[index];
        let fx = &self.fx[index];
        if fx.fills_with_sky() {
            return None;
        }
        let stroke = light.shaped.as_ref().and_then(|s| Some((s.stroke_cones()?, s.stroke_path()?)));
        if let Some(((axis, cos_radius, cos_cover), path)) = stroke {
            // A stroke's own light, after what hangs in front of it, under the
            // lights after it: what the map draws of it, sampled where its band
            // runs (its extras are not taken as a share apart, as a boxed key's
            // are: the samples along its band see them).
            let flags = dimmers(&self.lights[index + 1..], &self.fx[index + 1..]);
            let black = Vec3f::default();
            let drawn = |d: Vec3f| dimmed(&flags, d, fx.apply(d, black, light.draw(d, black, false)));
            let integral = over_cone(axis, cos_cover, &stroke_cells(path, cos_cover), &drawn);
            return Some(EnvSun { dir: axis, radiance: EnvSun::cone_radiance(integral.emission, cos_radius), cos_radius, facing: integral.facing, cos_cover });
        }
        let integral = key_emission(&self.lights[index..], &self.fx[index..], KEY_GRID);
        let key = EnvSun {
            dir: light.frame.center,
            // Over the cone the f32 cosine names, the one every consumer of
            // the key computes, so the emission comes back exactly however
            // small the light (`EnvSun::cone_radiance`).
            radiance: EnvSun::cone_radiance(integral.emission, light.key_cos),
            cos_radius: light.key_cos,
            facing: integral.facing,
            // The reach box's cone can only be wider than the nominal one
            // (it holds the half-size on both sides); the min keeps rounding
            // from putting it above.
            cos_cover: integral.cos_cover.min(light.key_cos),
        };
        // The engine's key light carries what gets past its own gobo, doors and
        // grid, times its face's wear (its radiance; its cone, facing share and
        // covering cone are its shape's, which the extras only cut).
        Some(EnvSun { radiance: key.radiance * self.key_share(index), ..key })
    }

    /// Whether the key is a window filled with sky that `Env` has yet to
    /// measure against its sky ([`Self::measured_window_key`]).
    pub(super) fn key_waits_for_sky(&self) -> bool {
        self.key.is_none() && self.key_index.is_some_and(|i| self.fx[i].fills_with_sky())
    }

    /// The key when it is a window filled with sky (D1, L:136): the sky it
    /// lets in, through its glass at 2^intensity_ev and its gobo, times its
    /// face's wear, under the lights after it, exactly the light the map shows
    /// through it and the keyless copy lacks (the copy keeps the glass and lets
    /// no sky in: `apply_lights_sky`). Not its own Kelvin colour, which the
    /// window never shows. `sky(d)` is the sky the window shows along `d` (the
    /// map's own frame): the map's base in Sky mode, the lightless sky behind
    /// a studio. `hot` are the discs in it (direction, angular radius), which
    /// a bake area-averages and this integral samples finely too. `fine(d,
    /// radius)` is the spacing the sky must be sampled at within `radius` of
    /// `d` where it draws a line thinner than a cell's samples (a storm's
    /// bolt, which a bake averages over its texels), None where it is smooth:
    /// a cell the line may cross takes that spacing. None when the key is no
    /// such window.
    pub(super) fn measured_window_key(
        &self,
        sky: &dyn Fn(Vec3f) -> Vec3f,
        hot: &[(Vec3f, f32)],
        fine: &dyn Fn(Vec3f, f32) -> Option<f32>,
    ) -> Option<EnvSun> {
        let index = self.key_index.filter(|&i| self.fx[i].fills_with_sky())?;
        let (light, fx) = (&self.lights[index], &self.fx[index]);
        let (axis, cos_radius, cos_cover) = light.key_cones();
        let flags = dimmers(&self.lights[index + 1..], &self.fx[index + 1..]);
        let drawn = |d: Vec3f| {
            let glass = fx.glass(d, self.cover(index, d));
            // Only the glass is looked through: the sky is worked out nowhere else.
            if !(glass > 0.0) {
                return Vec3f::default();
            }
            dimmed(&flags, d, fx.through_glass(d, glass, sky(d)))
        };
        let stroke = light.shaped.as_ref().and_then(|s| s.stroke_path()).map(|path| stroke_cells(path, cos_cover));
        let side = cone_cell(cos_cover);
        let cells = |centre: Vec3f| {
            let own = match &stroke {
                Some(stroke) => stroke(centre),
                None => WINDOW_FINE,
            };
            // A disc behind the glass: its cell is sampled finely enough to find it.
            let disc = hot
                .iter()
                .filter(|(dir, r)| centre.dot(*dir).clamp(-1.0, 1.0).acos() < side * std::f32::consts::FRAC_1_SQRT_2 + r)
                .map(|(_, r)| (side / (r / DISC_SAMPLES_PER_RADIUS)).ceil() as usize)
                .max()
                .unwrap_or(0);
            // A line behind the glass that may cross the cell: sampled at its own spacing.
            let line = fine(centre, side * std::f32::consts::FRAC_1_SQRT_2).map_or(0, |step| (side / step).ceil() as usize);
            own.max(disc).max(line)
        };
        let integral = over_cone(axis, cos_cover, &cells, &drawn);
        Some(EnvSun { dir: axis, radiance: EnvSun::cone_radiance(integral.emission, cos_radius), cos_radius, facing: integral.facing, cos_cover })
    }

    /// Takes the key `measured_window_key` measured; a key of any other kind
    /// was measured when the studio was built and is kept.
    pub(super) fn set_window_key(&mut self, key: Option<EnvSun>) {
        if self.key_waits_for_sky() {
            self.key = key;
        }
    }

    /// Whether any light is drawn over the base: with none, nothing stands in
    /// front of the sun or the moon.
    pub(super) fn has_lights(&self) -> bool {
        !self.lights.is_empty()
    }

    /// The share of the key's emission that gets past its gobo, doors and
    /// grid, times its wear (B10): `LightFx::key_share`, weighted by what the
    /// key puts into the map, its own drawing (shape, soft edge, hotspot,
    /// picture) under the flags, the cloth and the windows after it, as
    /// `key_emission` dims it. So the engine's key light loses what the map
    /// loses (plan 2's N3 removes exactly that). Sampled over the key's reach
    /// box. (A stroke's key samples its extras with its own drawing, along
    /// its band.)
    fn key_share(&self, index: usize) -> f32 {
        let light = &self.lights[index];
        let flags = dimmers(&self.lights[index + 1..], &self.fx[index + 1..]);
        let drawn = |dir: Vec3f| luminance(dimmed(&flags, dir, light.draw(dir, Vec3f::default(), false)));
        self.fx[index].key_share(&light.frame, light.reach, &drawn)
    }

    /// The vertical gradient.
    /// - The horizon colour holds exactly at elevation 0.
    /// - Above it the gradient reaches `top`, and below it `floor`, over
    ///   `horizon_softness x 90 degrees` of elevation.
    pub fn backdrop(&self, dir: Vec3f) -> Vec3f {
        let elevation = sane(dir.y, -1.0, 1.0).asin().to_degrees();
        if elevation >= 0.0 {
            self.horizon + (self.top - self.horizon) * smoothstep(0.0, self.soft_deg, elevation)
        } else {
            self.horizon + (self.floor - self.horizon) * smoothstep(0.0, self.soft_deg, -elevation)
        }
    }

    /// Draws the lights over `base` in list order. A window filled with sky
    /// shows `base` through it here; `Env` calls `apply_lights_sky` with the
    /// map's real sky.
    pub fn apply_lights(&self, dir: Vec3f, base: Vec3f) -> Vec3f {
        self.apply_lights_sky(dir, base, &|_| base, false)
    }

    /// [`Self::apply_lights`] without the key light (N3: the map without its
    /// key is the source of the engine's lighting copy). Skipping it is the
    /// map with the key disabled, except that a scrim key's cloth stays
    /// (`PreparedLight::hide`) and so does a sky window's glass, letting no
    /// sky in: every other light, the flags after the key included, draws as
    /// before.
    pub fn apply_lights_without_key(&self, dir: Vec3f, base: Vec3f) -> Vec3f {
        self.apply_lights_sky(dir, base, &|_| base, true)
    }

    /// `apply_lights` with `sky(dir)`, the sky a window filled with sky
    /// (`GoboParams::fill_sky`) shows. `sky` is called only for a direction
    /// such a window covers. `without_key` leaves the key light out (plan 2's
    /// N3, `apply_lights_without_key`; `Env::radiance_with` under
    /// `KeyTerm::Light`).
    ///
    /// Each light is drawn by its own code (`draw_lights`); what hangs in
    /// front of it then scales what it did (`LightFx::apply`), so a key left
    /// out leaves what it hides scaled as in the map and nothing of its own.
    pub(super) fn apply_lights_sky(&self, dir: Vec3f, base: Vec3f, sky: &dyn Fn(Vec3f) -> Vec3f, without_key: bool) -> Vec3f {
        let skip = if without_key { self.key_index } else { None };
        if !self.any_fx {
            return self.draw_lights(0..self.lights.len(), dir, base, skip);
        }
        let mut c = base;
        for (i, fx) in self.fx.iter().enumerate() {
            if fx.is_plain() {
                c = self.draw_lights(i..i + 1, dir, c, skip);
            } else if fx.fills_with_sky() {
                // A window is its glass showing the sky. Left out as the key it
                // keeps its glass and lets no sky in, as a scrim key keeps its
                // cloth: what is behind the glass (and against the sky, behind
                // its frame and bars: `LightFx::hides`) stays hidden, and the
                // map without it lacks exactly the sky the key carries (D1).
                let cover = self.cover(i, dir);
                if cover > 0.0 {
                    c = if skip == Some(i) { c * (1.0 - fx.hides(dir, cover)) } else { fx.window(dir, cover, c, sky(dir)) };
                }
            } else {
                let after = self.draw_lights(i..i + 1, dir, c, skip);
                c = fx.apply(dir, c, after);
            }
        }
        c
    }

    /// How much of light `index` covers `dir` ([`PreparedLight::cover`]).
    fn cover(&self, index: usize, dir: Vec3f) -> f32 {
        self.lights[index].cover(dir)
    }

    /// Draws the lights `range` of the list over `base`, in order, each with its own shape, content and blend.
    /// - **Add** accumulates the light's colour.
    /// - **Multiply** scales everything drawn so far by its colour: fully at the core,
    ///   not at all outside. That is how a black flag cuts a softbox or the sky.
    /// - Polygons, strokes and scrims follow the same rules through the shape core,
    ///   except that an Add scrim is cloth: it hides what is behind it, then glows.
    /// - An image light's colour is its picture at that point of its box, times
    ///   the light's colour; the picture's alpha, when it masks, fades the light in.
    /// - The light at `skip` (the key, for plan 2's keyless copy) loses its own light
    ///   and keeps what it hides (`PreparedLight::hide`).
    fn draw_lights(&self, range: std::ops::Range<usize>, dir: Vec3f, base: Vec3f, skip: Option<usize>) -> Vec3f {
        let mut c = base;
        for (i, light) in self.lights.iter().enumerate().skip(range.start).take(range.len()) {
            c = light.draw(dir, c, skip == Some(i));
        }
        c
    }

    /// The key light in the map's own frame (unrotated, unscaled). Its cone covers the
    /// larger of width and height (a stroke's, its centre line and half width), and its
    /// radiance is the light's whole emission as the map draws it, over that cone, not
    /// its peak colour: a thin strip or a ring that fills a tenth of its cone hands over
    /// about a tenth of its colour, and a key half under a flag half of it. `facing` is
    /// the share of that emission a surface facing the key receives (a wide softbox's is
    /// below 1) and `cos_cover` the cone around the key's centre that holds its reach.
    /// None without a key, and for a window filled with sky until `Env` has measured it
    /// against its sky.
    pub fn key(&self) -> Option<EnvSun> {
        self.key
    }
}

/// What one pass over the key light gives.
struct KeyIntegral {
    /// The key's whole emission, ∫ L dΩ per channel.
    emission: Vec3f,
    /// ∫ L cosθ dΩ / ∫ L dΩ in luminance, θ measured from the key's centre: the
    /// share of the emission a surface facing the key receives. 1 for a key that
    /// draws nothing.
    facing: f32,
    /// The cosine of the half-angle of the cone around the key's centre that holds
    /// the whole reach box, corners and soft edge included.
    cos_cover: f32,
}

/// The key light's whole emission, ∫ L dΩ, exactly as `apply_lights` paints it, with
/// its cosine-weighted share and the cone that covers its reach.
/// - `lights[0]` is the key: its shape, corner, ring, soft edge, hotspot and roll all
///   come in through its own `mask`.
/// - A light the shape core draws (1b: a polygon, B5's scrim) gives its `emit` instead
///   of A3's mask, and a shaped flag dims the key through `ShapedLight::compose`, as
///   `apply_lights` draws both. Its outline lies inside its box, so the grid holds it
///   and the box's corner cone (`cos_cover`) covers it.
/// - A key with a picture (1b, B14) weighs each point by its drawing there, the
///   picture's colour and alpha included (D1, L:123), not by its shape alone.
/// - Every Multiply light after it scales it as it does in the map (its picture,
///   cut-out alpha and all), and so do the two Add lights that hide what is behind
///   them: a scrim's cloth (B5, `ShapedLight::hide`) and a window filled with sky,
///   whose glass shows the sky in its place ([`dimmers`]). Any other Add light after
///   it is a light of its own, and a light before it touches only what came before.
/// - What hangs in front of each of those (`fx`, in step with `lights`) scales what it
///   does, as in the map: a flag with a gobo dims the key only behind the gobo's
///   opaque parts (B8).
///
/// A midpoint grid of n x n points over the key's reach box in its own tangent plane,
/// where dΩ = dx dy / (1 + x² + y²)^(3/2) and the cosine of the angle from the centre
/// is 1 / sqrt(1 + x² + y²). Outside that box the mask is exactly zero, so nothing is
/// missed, and the farthest point of the box is its corner: the covering cone
/// (`PreparedLight::box_cover`).
fn key_emission(lights: &[PreparedLight], fx: &[LightFx], n: usize) -> KeyIntegral {
    let Some((key, after)) = lights.split_first() else {
        return KeyIntegral { emission: Vec3f::default(), facing: 1.0, cos_cover: 1.0 };
    };
    let flags = dimmers(after, fx.get(1..).unwrap_or(&[]));
    // A picture gives the key its colour point by point: its drawing over black,
    // colour and all, is the weight, and the sum is not scaled by the colour again.
    let pictured = key.image.is_some();
    let n = n.max(1);
    let cell = vec2f(2.0 * key.reach.x / n as f32, 2.0 * key.reach.y / n as f32);
    let mut sum = [0.0f64; 3];
    let mut weighted = [0.0f64; 3];
    for j in 0..n {
        let y = -key.reach.y + (j as f32 + 0.5) * cell.y;
        let mut row = Vec3f::default();
        let mut row_weighted = Vec3f::default();
        for i in 0..n {
            let x = -key.reach.x + (i as f32 + 0.5) * cell.x;
            // `project`'s limit: past a dot of 0.05 with the centre nothing is drawn,
            // and the dot here is 1 / sqrt(r2).
            let r2 = 1.0 + x * x + y * y;
            if !(r2 <= 400.0) {
                continue;
            }
            let dir = || (key.frame.center + key.frame.right * x + key.frame.up * y).normalize();
            // The weight `apply_lights` gives the key's colour here: A3's mask, the
            // shape core's emission for a light it draws (1b: a polygon, B5's scrim),
            // or the whole drawing of a key with a picture.
            let (m, tint) = if pictured {
                let drawn = key.draw(dir(), Vec3f::default(), false);
                (if drawn.max_elem() > 0.0 { 1.0 } else { 0.0 }, drawn)
            } else {
                let m = match &key.shaped {
                    Some(shaped) => shaped.sample(dir()).emit,
                    None => key.mask(vec2f(x, y)),
                };
                (m, Vec3f::all(1.0))
            };
            if !(m > 0.0) {
                continue;
            }
            let cos = 1.0 / r2.sqrt();
            let mut w = Vec3f::all(m * cos / r2) * tint;
            if !flags.is_empty() {
                w = dimmed(&flags, dir(), w);
            }
            row += w;
            row_weighted += w * cos;
        }
        sum[0] += row.x as f64;
        sum[1] += row.y as f64;
        sum[2] += row.z as f64;
        weighted[0] += row_weighted.x as f64;
        weighted[1] += row_weighted.y as f64;
        weighted[2] += row_weighted.z as f64;
    }
    let area = (cell.x * cell.y) as f64;
    let color = if pictured { Vec3f::all(1.0) } else { key.color };
    let scaled = |s: [f64; 3]| color * vec3f((s[0] * area) as f32, (s[1] * area) as f32, (s[2] * area) as f32);
    let emission = scaled(sum);
    // Luminance weighs the channels as the eye does: a coloured gel's key still
    // gets one share.
    let (total, cosine) = (luminance(emission), luminance(scaled(weighted)));
    let facing = if total > 0.0 { (cosine / total).clamp(0.0, 1.0) } else { 1.0 };
    KeyIntegral { emission, facing, cos_cover: key.box_cover() }
}

/// What dims the key as the map draws it, of the lights listed after it: the
/// flags and gels, the scrims' cloth and the windows filled with sky in front
/// of it (a window's glass shows the sky in place of what is behind it), each
/// with what hangs in front of it (`fx`, in step with `after`).
fn dimmers<'a>(after: &'a [PreparedLight], fx: &'a [LightFx]) -> Vec<(&'a PreparedLight, &'a LightFx)> {
    after
        .iter()
        .zip(fx)
        .filter(|(l, fx)| l.blend == Blend::Multiply || l.shaped.as_ref().is_some_and(|s| s.hides_behind()) || fx.fills_with_sky())
        .collect()
}

/// `w`, the key's light along `dir`, as the lights in `flags` ([`dimmers`])
/// leave it in the map.
fn dimmed(flags: &[(&PreparedLight, &LightFx)], dir: Vec3f, w: Vec3f) -> Vec3f {
    let mut w = w;
    for &(flag, fx) in flags {
        if fx.fills_with_sky() {
            // As `apply_lights_sky` draws a window: what is behind its glass
            // (its shape's cover, past its gobo's bars) gives way to the sky,
            // and against the sky its frame and bars hide it too, none of it
            // the key's light, so the key keeps 1 - what it hides.
            let cover = flag.cover(dir);
            if cover > 0.0 {
                w = w * (1.0 - fx.hides(dir, cover));
            }
            continue;
        }
        let before = w;
        // As `apply_lights` draws the flag (`PreparedLight::draw`): its shape,
        // its picture and the picture's alpha, and a scrim's cloth without its
        // own glow (the scrim left out, so only what it hides is drawn).
        w = flag.draw(dir, w, flag.blend != Blend::Multiply);
        // Then what hangs in front of it scales what it did, as
        // `apply_lights_sky` does: a flag's gobo leaves the key through its
        // open parts (B8).
        if !fx.is_plain() {
            w = fx.apply(dir, before, w);
        }
    }
    w
}

/// For [`over_cone`] over the covering cone (`cos_cover`) of the stroke whose
/// centre line is `path`: how many samples per axis the cell round a direction
/// takes. Where the stroke may draw (its path widened by a cell's reach finds
/// the cell), enough for STROKE_SAMPLES_PER_REACH of them per stroke reach;
/// elsewhere none.
fn stroke_cells(path: &ArcPath, cos_cover: f32) -> impl Fn(Vec3f) -> usize {
    let side = cone_cell(cos_cover);
    let fine = ((side * STROKE_SAMPLES_PER_REACH / path.reach()).ceil() as usize).clamp(1, STROKE_FINE_MAX);
    let scan = path.widened(side * std::f32::consts::FRAC_1_SQRT_2);
    move |centre: Vec3f| if scan.nearest(centre).dist < scan.reach() { fine } else { 0 }
}

/// The side of one of [`over_cone`]'s coarse cells over the cone of
/// `cos_cover`, in radians. Every direction in a cell lies within
/// side / sqrt(2) of its centre.
fn cone_cell(cos_cover: f32) -> f32 {
    2.0 * cos_cover.clamp(-1.0, 1.0).acos() / CONE_CELLS as f32
}

/// ∫ light(d) dΩ over the cone of `cos_cover` round `axis`, which must hold
/// everything `light` draws, with its cosine-weighted share about `axis`: a
/// key as the map draws it, for the keys that do not lie in a box of their
/// own frame (a stroke, which its points place) or whose light is not their
/// colour (a window filled with sky).
///
/// The cone is laid flat by the azimuthal equidistant projection round
/// `axis`, (u, v) = θ (cos φ, sin φ) with θ the angle from it, so its cells
/// are even in angle however wide it is (a tangent plane stretches them past
/// 60 degrees and ends at 90), and dΩ = sin θ / θ du dv. A grid of
/// CONE_CELLS x CONE_CELLS cells ([`cone_cell`]) covers the square round it;
/// `cells(centre)` says how many jittered samples along each axis a cell
/// takes: 0 where nothing is drawn within side / sqrt(2) of its centre, which
/// holds the whole cell (the projection never stretches an angle). So a thin
/// stroke is sampled finely along its band and nowhere else. Over
/// CONE_BUDGET samples in all, every cell takes fewer, in proportion.
fn over_cone(axis: Vec3f, cos_cover: f32, cells: &dyn Fn(Vec3f) -> usize, light: &dyn Fn(Vec3f) -> Vec3f) -> KeyIntegral {
    let (azimuth_deg, elevation_deg) = az_el_from_dir(axis);
    let frame = light_frame(azimuth_deg, elevation_deg, 0.0);
    let reach = cos_cover.clamp(-1.0, 1.0).acos();
    let side = cone_cell(cos_cover);
    let at = |u: f32, v: f32| {
        let theta = (u * u + v * v).sqrt();
        if !(theta > 1.0e-9) {
            return frame.center;
        }
        let (sin, cos) = theta.sin_cos();
        frame.center * cos + (frame.right * u + frame.up * v) * (sin / theta)
    };
    let corner = |k: usize| -reach + k as f32 * side;
    let mut visited = Vec::new();
    let mut total = 0usize;
    for j in 0..CONE_CELLS {
        for i in 0..CONE_CELLS {
            let (u, v) = (corner(i) + 0.5 * side, corner(j) + 0.5 * side);
            // A cell wholly outside the cone holds nothing.
            if (u * u + v * v).sqrt() - side * std::f32::consts::FRAC_1_SQRT_2 > reach {
                continue;
            }
            let m = cells(at(u, v)).min(CONE_FINE_MAX);
            if m > 0 {
                visited.push((i, j, m));
                total += m * m;
            }
        }
    }
    let thin = if total > CONE_BUDGET { (CONE_BUDGET as f32 / total as f32).sqrt() } else { 1.0 };
    let (mut sum, mut weighted) = ([0.0f64; 3], [0.0f64; 3]);
    for (i, j, m) in visited {
        let m = ((m as f32 * thin) as usize).max(1);
        let step = side / m as f32;
        let (mut cell_sum, mut cell_weighted) = (Vec3f::default(), Vec3f::default());
        for b in 0..m {
            for a in 0..m {
                // Every sample of the grid has its own place in the hash.
                let (hx, hy) = ((i * CONE_FINE_MAX + a) as i32, (j * CONE_FINE_MAX + b) as i32);
                let u = corner(i) + (a as f32 + hash01(hx, hy, 0, CONE_JITTER_SEED)) * step;
                let v = corner(j) + (b as f32 + hash01(hx, hy, 1, CONE_JITTER_SEED)) * step;
                let theta = (u * u + v * v).sqrt();
                // Nothing is drawn past the cone.
                if !(theta <= reach) {
                    continue;
                }
                let c = light(at(u, v));
                if !(c.max_elem() > 0.0) {
                    continue;
                }
                let w = if theta > 1.0e-6 { c * (theta.sin() / theta) } else { c };
                cell_sum += w;
                cell_weighted += w * theta.cos();
            }
        }
        let area = (step * step) as f64;
        for (s, c) in sum.iter_mut().zip([cell_sum.x, cell_sum.y, cell_sum.z]) {
            *s += c as f64 * area;
        }
        for (s, c) in weighted.iter_mut().zip([cell_weighted.x, cell_weighted.y, cell_weighted.z]) {
            *s += c as f64 * area;
        }
    }
    let emission = vec3f(sum[0] as f32, sum[1] as f32, sum[2] as f32);
    let cosine = luminance(vec3f(weighted[0] as f32, weighted[1] as f32, weighted[2] as f32));
    let total = luminance(emission);
    let facing = if total > 0.0 { (cosine / total).clamp(0.0, 1.0) } else { 1.0 };
    KeyIntegral { emission, facing, cos_cover }
}

/// One enabled light, reduced to what the per-pixel evaluation needs.
#[derive(Clone, Debug)]
struct PreparedLight {
    /// The light's frame; a stroke's is the box round it (`stroke_box`).
    frame: LightFrame,
    shape: LightShape,
    blend: Blend,
    /// Half extents in the tangent plane: tan(width/2), tan(height/2); a
    /// stroke's are its box's.
    half: Vec2f,
    /// Rounded-rect corner radius, in tangent units.
    corner: f32,
    /// The ring's centre line and half thickness, as fractions of the outer radius.
    ring_mid: f32,
    ring_half: f32,
    /// Width of the soft edge, in tangent units.
    band: f32,
    /// Outside this box (the half extents plus the edge) the light is exactly zero.
    reach: Vec2f,
    /// Exponent scale of the Gaussian centre falloff (HOTSPOT_FALLOFF x hotspot).
    hotspot: f32,
    /// The key cone's cosine when this light is the key in its own box: the
    /// cone over the larger of width and height.
    key_cos: f32,
    color: Vec3f,
    /// Polygons, strokes and scrims (1b): the shape core draws the light.
    shaped: Option<ShapedLight>,
    /// A picture as the light's content (1b); None draws the plain colour.
    image: Option<LightImage>,
}

impl PreparedLight {
    fn new(l: &LightParams, images: &ImageSet) -> PreparedLight {
        let width = sane(l.width_deg, 0.1, 170.0);
        let height = sane(l.height_deg, 0.1, 170.0);
        // A stroke's points place it, not its frame: its picture is laid in
        // the box round the stroke itself (D3, L:119), as its gobo, aim and
        // wear are (`light_fx::face_axes`). Nothing else of a stroke reads its
        // frame or its box: the shape core draws it and its key is measured
        // round its own cones.
        let (frame, half) = match stroke_box(l) {
            Some(stroke) => stroke,
            None => (light_frame(l.azimuth_deg, l.elevation_deg, l.roll_deg), vec2f((0.5 * width).to_radians().tan(), (0.5 * height).to_radians().tan())),
        };
        let short = half.x.min(half.y);
        let inner = sane(l.inner, 0.0, 0.95);
        let ring_half = 0.5 * (1.0 - inner);
        let shape = l.shape();
        // The deepest point inside the shape. Softness 1 ramps the edge right to it,
        // so "fully soft" means the same thing for a wide softbox and a thin ring.
        let inradius = match shape {
            LightShape::Ring => ring_half * short,
            // The shape core draws polygons, strokes and scrims (shaped.rs, B2-B5).
            // `short` still sizes the band, and so the reach box key_emission
            // integrates a polygon or scrim key over, which must hold its soft
            // edge (B2, B5). A stroke never reads it: its points place it.
            LightShape::Rect | LightShape::Disc | LightShape::Polygon | LightShape::Stroke | LightShape::Scrim => short,
        };
        let band = (sane(l.softness, 0.0, 1.0) * inradius).max(MIN_EDGE);
        PreparedLight {
            frame,
            shape,
            blend: l.blend(),
            half,
            corner: sane(l.corner, 0.0, 1.0) * short,
            ring_mid: 0.5 * (1.0 + inner),
            ring_half,
            band,
            reach: vec2f(half.x + band, half.y + band),
            hotspot: HOTSPOT_FALLOFF * sane(l.hotspot, 0.0, 1.0),
            key_cos: (0.5 * sane(l.width_deg.max(l.height_deg), 0.1, 170.0)).to_radians().cos(),
            color: light_color(l),
            shaped: ShapedLight::new(l),
            image: l.image.as_ref().and_then(|image| LightImage::new(image, images)),
        }
    }

    /// This light drawn over `acc` as the map draws it: its shape, content and
    /// blend, and a picture faded in by its alpha. `left_out` draws it as the
    /// key left out of the map (`skip`, N3): it loses its own light, while
    /// whatever it hides stays hidden (B5's scrim, `hide`); for a light that
    /// hides nothing (an A3 light, a polygon, a stroke, a picture) that is the
    /// same as skipping it.
    fn draw(&self, dir: Vec3f, acc: Vec3f, left_out: bool) -> Vec3f {
        let Some(image) = &self.image else {
            return if left_out { self.hide(dir, acc) } else { self.over(dir, acc, self.color) };
        };
        // The picture is only looked up where the light can reach. Held at the
        // plane's limit: only a stroke reaches past it, and there its picture's
        // edge goes on (a light in its own box draws nothing so far out).
        let Some(p) = project_held(&self.frame, dir) else { return acc };
        if !self.reaches(dir, p) {
            return acc;
        }
        let (texel, alpha) = image.shade(p, self.half);
        if !(alpha > 0.0) {
            return acc;
        }
        let lit = if left_out { self.hide(dir, acc) } else { self.over(dir, acc, self.color * texel) };
        // Alpha fades the whole light in. For Add and Multiply alike that
        // is a mix toward what the light makes of the pixel.
        if alpha >= 1.0 { lit } else { acc + (lit - acc) * alpha }
    }

    /// How much of this light covers `dir`, 0..=1, whatever its colour: the
    /// glass of a window filled with sky. A plain light drawn in white over
    /// black is its own coverage (shape, soft edge and hotspot). A shaped
    /// light is asked directly: a stroke with a ramp carries its own colour,
    /// so drawing it in white would not give its coverage.
    fn cover(&self, dir: Vec3f) -> f32 {
        match &self.shaped {
            Some(shaped) if shaped.cull().contains(dir) => shaped.sample(dir).emit,
            Some(_) => 0.0,
            None => self.over(dir, Vec3f::default(), Vec3f::all(1.0)).x,
        }
    }

    /// The cone that holds this light's reach box, corners and soft edge
    /// included: 1 / sqrt(1 + rx² + ry²), no smaller than `project`'s 0.05
    /// limit, past which nothing is drawn.
    fn box_cover(&self) -> f32 {
        (1.0 / (1.0 + self.reach.x * self.reach.x + self.reach.y * self.reach.y).sqrt()).max(0.05)
    }

    /// The key's cones when this light is the key, `(axis, cos_radius,
    /// cos_cover)`: a stroke's round its centre line, any other light's round
    /// the centre of its box (the larger of width and height, and the box's
    /// corner).
    fn key_cones(&self) -> (Vec3f, f32, f32) {
        match self.shaped.as_ref().and_then(|s| s.stroke_cones()) {
            Some(cones) => cones,
            None => (self.frame.center, self.key_cos, self.box_cover().min(self.key_cos)),
        }
    }

    /// This light drawn over `acc` in the colour `color`.
    fn over(&self, dir: Vec3f, acc: Vec3f, color: Vec3f) -> Vec3f {
        if let Some(shaped) = &self.shaped {
            return shaped.compose(dir, acc, color);
        }
        let Some(p) = project(&self.frame, dir) else { return acc };
        let m = self.mask(p);
        if !(m > 0.0) {
            return acc;
        }
        match self.blend {
            Blend::Add => acc + color * m,
            Blend::Multiply => acc * (Vec3f::all(1.0) + (color - Vec3f::all(1.0)) * m),
        }
    }

    /// This light over `acc` with its own light left out: what the key leaves
    /// of the map in the engine's keyless lighting copy (plan 2's N3). What it
    /// hides stays hidden (a scrim's cloth, through the shape core); an A3
    /// light adds only, so it leaves `acc`.
    fn hide(&self, dir: Vec3f, acc: Vec3f) -> Vec3f {
        match &self.shaped {
            Some(shaped) => shaped.hide(dir, acc),
            None => acc,
        }
    }

    /// A cheap test that the light can touch `dir`, whose tangent-plane point
    /// is `p`: the cull cone of a shaped light, the reach box of the others.
    fn reaches(&self, dir: Vec3f, p: Vec2f) -> bool {
        match &self.shaped {
            Some(shaped) => shaped.cull().contains(dir),
            None => p.x.abs() <= self.reach.x && p.y.abs() <= self.reach.y,
        }
    }

    /// Coverage 0..=1 at tangent-plane point `p`: the soft edge times the hotspot falloff.
    fn mask(&self, p: Vec2f) -> f32 {
        // Cheap box cull first. Most of the sphere is far from any light.
        if p.x.abs() > self.reach.x || p.y.abs() > self.reach.y {
            return 0.0;
        }
        // Signed distance to the edge (negative inside) and the squared radial
        // position used for the hotspot (0 at the centre, 1 at the edge).
        let (distance, r2) = match self.shape {
            LightShape::Rect => {
                let nx = p.x / self.half.x;
                let ny = p.y / self.half.y;
                (sd_round_rect(p, self.half, self.corner), nx * nx + ny * ny)
            }
            LightShape::Disc => {
                let (k0, scale) = ellipse_metric(p, self.half);
                ((k0 - 1.0) * scale, k0 * k0)
            }
            LightShape::Ring => {
                // abs(r - R) - w, with the radius measured in the ellipse's own units.
                let (k0, scale) = ellipse_metric(p, self.half);
                let off = (k0 - self.ring_mid).abs();
                let across = off / self.ring_half;
                ((off - self.ring_half) * scale, across * across)
            }
            // Drawn by the shape core. Reaching here means its outline could not
            // be built (a stroke with fewer than two points): it covers nothing.
            LightShape::Polygon | LightShape::Stroke | LightShape::Scrim => return 0.0,
        };
        let edge = 1.0 - smoothstep(-0.5 * self.band, 0.5 * self.band, distance);
        if edge <= 0.0 {
            return 0.0;
        }
        if self.hotspot > 0.0 {
            edge * (-self.hotspot * r2).exp()
        } else {
            edge
        }
    }
}

/// Signed distance to a rect with half extents `half` and corner radius `radius`
/// (iq's rounded box, the knob's `rect_d`). Everything is in tangent-plane units.
fn sd_round_rect(p: Vec2f, half: Vec2f, radius: f32) -> f32 {
    let qx = p.x.abs() - half.x + radius;
    let qy = p.y.abs() - half.y + radius;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - radius
}

/// Two values for point `p` and an ellipse with half axes `half`:
/// - the normalised elliptical radius k0, which is 1 on the ellipse;
/// - the tangent-plane length of one unit of k0 near `p`, k0 / |grad k0|.
///
/// This is iq's first-order ellipse distance. It is exact for a circle and close
/// near the edge, which is where the soft edge reads it.
fn ellipse_metric(p: Vec2f, half: Vec2f) -> (f32, f32) {
    let nx = p.x / half.x;
    let ny = p.y / half.y;
    let k0 = (nx * nx + ny * ny).sqrt();
    let gx = nx / half.x;
    let gy = ny / half.y;
    let k1 = (gx * gx + gy * gy).sqrt();
    let scale = if k1 > 1.0e-12 { k0 / k1 } else { half.x.min(half.y) };
    (k0, scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::{az_el_from_dir, vec, Env, HdriParams};
    use makepad_render_material::ibl::{self, EnvMap};

    #[test]
    fn kelvin_has_luminance_one_and_warms_as_it_drops() {
        for kelvin in [1000.0, 1900.0, 2700.0, 4000.0, 5600.0, 6500.0, 10000.0, 20000.0] {
            let c = kelvin_to_rgb(kelvin);
            assert!(c.is_finite() && c.x >= 0.0 && c.y >= 0.0 && c.z >= 0.0, "{kelvin} K: {c:?}");
            assert!((luminance(c) - 1.0).abs() < 1.0e-4, "{kelvin} K: luminance {}", luminance(c));
        }
        // 6500 K sits on the Planckian locus just below D65: white within a few percent.
        let daylight = kelvin_to_rgb(6500.0);
        for channel in [daylight.x, daylight.y, daylight.z] {
            assert!((channel - 1.0).abs() < 0.06, "6500 K: {daylight:?}");
        }
        let tungsten = kelvin_to_rgb(2700.0);
        assert!(tungsten.x > tungsten.y && tungsten.y > tungsten.z, "2700 K: {tungsten:?}");
        // Red over blue falls steadily as the temperature climbs (blue is unclipped from 2000 K).
        let mut last = f32::MAX;
        for kelvin in (2000..=20000).step_by(500) {
            let c = kelvin_to_rgb(kelvin as f32);
            let red_over_blue = c.x / c.z;
            assert!(red_over_blue < last, "{kelvin} K");
            last = red_over_blue;
        }
        // Out-of-range and NaN inputs clamp to the ends instead of poisoning the map.
        assert_eq!(kelvin_to_rgb(f32::NAN), kelvin_to_rgb(1000.0));
        assert_eq!(kelvin_to_rgb(1.0e9), kelvin_to_rgb(20000.0));
    }

    #[test]
    fn tint_shifts_green_and_magenta_at_constant_luminance() {
        assert!((tint_rgb(0.0) - Vec3f::all(1.0)).length() < 1.0e-5);
        let magenta = tint_rgb(1.0);
        let green = tint_rgb(-1.0);
        assert!(magenta.y < magenta.x && magenta.y < magenta.z, "{magenta:?}");
        assert!(green.y > green.x && green.y > green.z, "{green:?}");
        for c in [magenta, green, tint_rgb(0.3)] {
            assert!((luminance(c) - 1.0).abs() < 1.0e-5, "{c:?}");
        }
    }

    #[test]
    fn light_color_is_kelvin_times_ev_or_explicit_rgb() {
        // The default light: 6500 K, tint 0, +3 EV.
        let mut light = LightParams::default();
        assert!((luminance(light_color(&light)) - 8.0).abs() < 1.0e-3);
        // Tint changes the hue, not the brightness.
        light.tint = 0.5;
        assert!((luminance(light_color(&light)) - 8.0).abs() < 1.0e-3);
        // An explicit colour overrides Kelvin and tint and is used as given.
        light.rgb = Some([0.2, 0.4, 0.6]);
        light.intensity_ev = 1.0;
        assert!((light_color(&light) - vec3f(0.4, 0.8, 1.2)).length() < 1.0e-6);
    }

    #[test]
    fn light_frames_are_orthonormal_and_roll_turns_them() {
        for (az, el, roll) in [(0.0, 0.0, 0.0), (135.0, 30.0, 20.0), (270.0, 89.0, -45.0), (10.0, 90.0, 0.0), (300.0, -60.0, 170.0)] {
            let f = light_frame(az, el, roll);
            for v in [f.center, f.right, f.up] {
                assert!((v.length() - 1.0).abs() < 1.0e-5, "({az}, {el}, {roll})");
            }
            assert!(f.center.dot(f.right).abs() < 1.0e-5, "({az}, {el}, {roll})");
            assert!(f.center.dot(f.up).abs() < 1.0e-5, "({az}, {el}, {roll})");
            assert!(f.right.dot(f.up).abs() < 1.0e-5, "({az}, {el}, {roll})");
            // Right-handed, as ibl::softbox's basis: right x up = center.
            assert!((Vec3f::cross(f.right, f.up) - f.center).length() < 1.0e-5, "({az}, {el}, {roll})");
        }
        // ibl's basis. Facing north on the horizon, right = cross(+Y, -Z) = -X, which is
        // the viewer's LEFT, and up is +Y. Facing east, right = cross(+Y, +X) = -Z.
        let north = light_frame(0.0, 0.0, 0.0);
        assert!((north.right - vec3f(-1.0, 0.0, 0.0)).length() < 1.0e-6);
        assert!((north.up - vec3f(0.0, 1.0, 0.0)).length() < 1.0e-6);
        let east = light_frame(90.0, 0.0, 0.0);
        assert!((east.right - vec3f(0.0, 0.0, -1.0)).length() < 1.0e-6);
        // Positive roll turns the shape counter-clockwise as the viewer sees it: after
        // 90 degrees the axis that pointed to the viewer's left (-X) points down.
        let rolled = light_frame(0.0, 0.0, 90.0);
        assert!((rolled.right - vec3f(0.0, -1.0, 0.0)).length() < 1.0e-6);
        assert!((rolled.up - vec3f(-1.0, 0.0, 0.0)).length() < 1.0e-6);
        // The frame stays continuous as a light climbs to the zenith.
        let top = light_frame(30.0, 90.0, 0.0);
        let near_top = light_frame(30.0, 89.9, 0.0);
        assert!((top.right - near_top.right).length() < 0.01 && (top.up - near_top.up).length() < 0.01);
    }

    #[test]
    fn project_is_gnomonic_and_rejects_the_far_side() {
        let tilted = light_frame(90.0, 20.0, 0.0);
        let p = project(&tilted, tilted.center).expect("the centre projects");
        assert!(p.x.abs() < 1.0e-6 && p.y.abs() < 1.0e-6);
        let east = light_frame(90.0, 0.0, 0.0);
        // 10 degrees further round the horizon (clockwise: the viewer's right) is
        // tan(10) along -right, because ibl's right points to the viewer's left.
        let q = project(&east, dir_from_az_el(100.0, 0.0)).expect("in front");
        assert!((q.x + 10f32.to_radians().tan()).abs() < 1.0e-5 && q.y.abs() < 1.0e-6, "{q:?}");
        // 10 degrees up is tan(10) up.
        let q = project(&east, dir_from_az_el(90.0, 10.0)).expect("in front");
        assert!(q.x.abs() < 1.0e-6 && (q.y - 10f32.to_radians().tan()).abs() < 1.0e-5, "{q:?}");
        // Behind, at 90 degrees, and NaN are all rejected.
        assert!(project(&east, dir_from_az_el(270.0, 0.0)).is_none());
        assert!(project(&east, dir_from_az_el(180.0, 0.0)).is_none());
        assert!(project(&east, vec3f(f32::NAN, 0.0, 0.0)).is_none());
    }

    fn black_studio() -> StudioParams {
        StudioParams {
            top: [0.0; 3],
            horizon: [0.0; 3],
            floor: [0.0; 3],
            ..Default::default()
        }
    }

    /// A hard-edged black flag: a Multiply light whose colour is zero.
    fn flag_at(azimuth_deg: f32, elevation_deg: f32, size_deg: f32) -> LightParams {
        LightParams {
            name: "Flag".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg: size_deg,
            height_deg: size_deg,
            corner: 0.0,
            softness: 0.0,
            blend: "multiply".to_string(),
            rgb: Some([0.0; 3]),
            ..Default::default()
        }
    }

    #[test]
    fn backdrop_blends_top_horizon_and_floor() {
        let mut s = StudioParams {
            top: [1.0, 0.0, 0.0],
            horizon: [0.0, 1.0, 0.0],
            floor: [0.0, 0.0, 1.0],
            ..Default::default()
        };
        s.horizon_softness = 0.2; // 18 degrees of elevation
        let studio = Studio::new(&s, &[]);
        assert_eq!(studio.backdrop(vec3f(0.0, 1.0, 0.0)), vec3f(1.0, 0.0, 0.0));
        assert_eq!(studio.backdrop(vec3f(0.0, -1.0, 0.0)), vec3f(0.0, 0.0, 1.0));
        assert!((studio.backdrop(vec3f(1.0, 0.0, 0.0)) - vec3f(0.0, 1.0, 0.0)).length() < 1.0e-6);
        // Half way up the 18 degree band the gradient is half way.
        let half = studio.backdrop(dir_from_az_el(0.0, 9.0));
        assert!((half.x - 0.5).abs() < 1.0e-3 && (half.y - 0.5).abs() < 1.0e-3, "{half:?}");
        let half_down = studio.backdrop(dir_from_az_el(0.0, -9.0));
        assert!((half_down.z - 0.5).abs() < 1.0e-3 && (half_down.y - 0.5).abs() < 1.0e-3, "{half_down:?}");
        // Past the band it is all top.
        assert!((studio.backdrop(dir_from_az_el(0.0, 20.0)) - vec3f(1.0, 0.0, 0.0)).length() < 1.0e-6);
    }

    #[test]
    fn a_light_is_its_colour_at_the_centre_and_nothing_outside() {
        for shape in ["rect", "disc", "ring"] {
            let light = LightParams {
                shape: shape.to_string(),
                azimuth_deg: 30.0,
                elevation_deg: 10.0,
                width_deg: 40.0,
                height_deg: 20.0,
                ..Default::default()
            };
            let color = light_color(&light);
            let studio = Studio::new(&black_studio(), &[light]);
            let centre = dir_from_az_el(30.0, 10.0);
            let got = studio.apply_lights(centre, Vec3f::default());
            if shape == "ring" {
                assert_eq!(got, Vec3f::default(), "a ring is dark in its middle");
                // Its centre line (inner 0.5, so 75% of the half width) is at full strength.
                let frame = light_frame(30.0, 10.0, 0.0);
                let x = 0.75 * 20f32.to_radians().tan();
                let on_line = (frame.center + frame.right * x).normalize();
                let lit = studio.apply_lights(on_line, Vec3f::default());
                assert!((lit - color).length() < 1.0e-4 * luminance(color), "ring: {lit:?}");
            } else {
                assert!((got - color).length() < 1.0e-5 * luminance(color), "{shape}: {got:?}");
            }
            // Outside the shape, and behind the viewer, the base passes through untouched.
            assert_eq!(studio.apply_lights(dir_from_az_el(30.0, 40.0), Vec3f::all(0.25)), Vec3f::all(0.25), "{shape}");
            assert_eq!(studio.apply_lights(dir_from_az_el(210.0, -10.0), Vec3f::all(0.25)), Vec3f::all(0.25), "{shape}");
        }
    }

    #[test]
    fn multiply_scales_only_what_came_before_it() {
        // The default light: az 0, el 30, 30x20 degrees, 6500 K at +3 EV.
        let soft = LightParams::default();
        let flag = flag_at(0.0, 30.0, 10.0);
        let centre = dir_from_az_el(0.0, 30.0);
        let grey = Vec3f::all(0.5);
        // A flag listed after the softbox blacks out the backdrop and the softbox behind it.
        let after = Studio::new(&black_studio(), &[soft.clone(), flag.clone()]);
        assert_eq!(after.apply_lights(centre, grey), Vec3f::default());
        // Listed before, it blacks out the backdrop only; the softbox stays on top.
        let before = Studio::new(&black_studio(), &[flag, soft.clone()]);
        let got = before.apply_lights(centre, grey);
        assert!((got - light_color(&soft)).length() < 1.0e-5 * luminance(light_color(&soft)), "{got:?}");
    }

    #[test]
    fn disabled_lights_are_skipped_and_the_first_add_key_leads() {
        let off = LightParams {
            enabled: false,
            key: true,
            ..Default::default()
        };
        let key = LightParams {
            key: true,
            azimuth_deg: 200.0,
            elevation_deg: 35.0,
            width_deg: 30.0,
            height_deg: 20.0,
            ..Default::default()
        };
        let studio = Studio::new(&black_studio(), &[off, key.clone()]);
        // The disabled light at (0, 30) draws nothing.
        assert_eq!(studio.apply_lights(dir_from_az_el(0.0, 30.0), Vec3f::default()), Vec3f::default());
        let sun = studio.key().expect("the enabled key light");
        assert!((sun.dir - dir_from_az_el(200.0, 35.0)).length() < 1.0e-6);
        // Its colour, times the share of its 15 degree cone the 30 x 20 rect fills.
        let fill = sun.radiance.y / light_color(&key).y;
        assert!((0.8..0.86).contains(&fill), "fill {fill}");
        assert!((sun.radiance - light_color(&key) * fill).length() < 1.0e-5 * sun.radiance.length());
        assert!((sun.cos_radius - 15f32.to_radians().cos()).abs() < 1.0e-6);
        let mut flag_key = flag_at(90.0, 0.0, 10.0);
        flag_key.key = true;
        assert!(Studio::new(&black_studio(), &[flag_key]).key().is_none(), "a flag is never the key");
    }

    /// The direction at the centre of an equirect texel, in ibl's convention.
    fn uv_dir(u: f32, v: f32) -> Vec3f {
        vec(ibl::equirect_uv_to_dir([u, v]))
    }

    fn studio_params(lights: Vec<LightParams>) -> HdriParams {
        HdriParams {
            mode: "studio".to_string(),
            studio: black_studio(),
            lights,
            ..Default::default()
        }
    }

    /// A round light with a hotspot, so it has a single brightest direction.
    fn round_light(azimuth_deg: f32, elevation_deg: f32) -> LightParams {
        LightParams {
            shape: "disc".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg: 20.0,
            height_deg: 20.0,
            softness: 0.5,
            hotspot: 1.0,
            ..Default::default()
        }
    }

    fn peak_pixel(map: &EnvMap) -> (usize, usize) {
        let mut best = (0usize, f32::MIN);
        for (i, t) in map.data.iter().enumerate() {
            let l = luminance(vec3f(t[0], t[1], t[2]));
            if l > best.1 {
                best = (i, l);
            }
        }
        (best.0 % map.width, best.0 / map.width)
    }

    #[test]
    fn a_light_peaks_at_its_pixel() {
        let (w, h) = (64, 32);
        // Aim the light at the centre of pixel (40, 10).
        let (az, el) = az_el_from_dir(uv_dir(40.5 / w as f32, 10.5 / h as f32));
        // ibl's convention: north (-Z) is at u = 0.5, so azimuth a sits at u = 0.5 + a/360.
        assert!((az - (40.5 / 64.0 - 0.5) * 360.0).abs() < 1.0e-3, "az {az}");
        let env = Env::new(&studio_params(vec![round_light(az, el)]));
        let map = env.bake(w);
        assert_eq!((map.width, map.height), (w, h));
        assert_eq!(peak_pixel(&map), (40, 10));
    }

    #[test]
    fn rotation_moves_a_light_by_rotation_deg() {
        let (w, h) = (64, 32);
        let (az, el) = az_el_from_dir(uv_dir(40.5 / w as f32, 10.5 / h as f32));
        let mut p = studio_params(vec![round_light(az, el)]);
        // rotate_y has ibl's sign: +90 takes -Z to -X, so content at azimuth a shows at
        // a - rotation_deg. 45 degrees is 8 of 64 columns, toward -u.
        p.rotation_deg = 45.0;
        assert_eq!(peak_pixel(&Env::new(&p).bake(w)), (32, 10));
        p.rotation_deg = -90.0;
        assert_eq!(peak_pixel(&Env::new(&p).bake(w)), (56, 10));
    }

    #[test]
    fn a_multiply_flag_darkens_only_its_area() {
        let (w, h) = (64, 32);
        let mut p = studio_params(Vec::new());
        p.studio.top = [0.5; 3];
        p.studio.horizon = [0.5; 3];
        p.studio.floor = [0.5; 3];
        let soft = LightParams {
            azimuth_deg: 90.0,
            elevation_deg: 0.0,
            width_deg: 60.0,
            height_deg: 40.0,
            ..Default::default()
        };
        p.lights = vec![soft.clone()];
        let open = Env::new(&p).bake(w);
        p.lights = vec![soft, flag_at(90.0, 0.0, 12.0)];
        let flagged = Env::new(&p).bake(w);
        let frame = light_frame(90.0, 0.0, 0.0);
        let mut darkened = 0;
        for y in 0..h {
            for x in 0..w {
                let dir = uv_dir((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let at = project(&frame, dir);
                // The flag's half size is tan(6 degrees) = 0.105.
                let inside = at.is_some_and(|q| q.x.abs() < 0.1 && q.y.abs() < 0.1);
                let outside = at.is_none_or(|q| q.x.abs() > 0.12 || q.y.abs() > 0.12);
                let i = y * w + x;
                for k in 0..3 {
                    assert!(flagged.data[i][k] <= open.data[i][k], "pixel ({x}, {y}) got brighter");
                    if inside {
                        assert_eq!(flagged.data[i][k], 0.0, "pixel ({x}, {y}) is under the flag");
                    }
                    if outside {
                        assert_eq!(flagged.data[i][k], open.data[i][k], "pixel ({x}, {y}) is beside the flag");
                    }
                }
                if inside {
                    assert!(open.data[i][0] > 0.0, "pixel ({x}, {y}) is lit before the flag");
                    darkened += 1;
                }
            }
        }
        assert!(darkened > 0);
    }

    #[test]
    fn a_disabled_light_leaves_the_map_untouched() {
        let mut off = round_light(90.0, 10.0);
        off.enabled = false;
        let mut p = studio_params(Vec::new());
        p.studio.horizon = [0.3; 3];
        let without = Env::new(&p).bake(32);
        p.lights = vec![off];
        assert_eq!(Env::new(&p).bake(32), without);
    }

    #[test]
    fn the_key_light_is_the_env_sun_in_world_space() {
        let key = LightParams {
            key: true,
            azimuth_deg: 200.0,
            elevation_deg: 35.0,
            width_deg: 30.0,
            height_deg: 20.0,
            ..Default::default()
        };
        let mut p = studio_params(vec![LightParams::default(), key.clone()]);
        p.intensity_ev = 1.0;
        let sun = Env::new(&p).sun().expect("a key light");
        assert!((sun.dir - dir_from_az_el(200.0, 35.0)).length() < 1.0e-5);
        let expected = Studio::new(&p.studio, &p.lights).key().expect("a key light").radiance * 2.0;
        assert!((sun.radiance - expected).length() < 1.0e-5 * luminance(expected));
        assert!((sun.cos_radius - 15f32.to_radians().cos()).abs() < 1.0e-6);
        assert!(sun.validate().is_ok(), "the key is a valid scene key: {sun:?}");
        // The map's yaw turns the key with it, counter-clockwise seen from above
        // (ibl's sign): azimuth 200 shows at 170.
        p.rotation_deg = 30.0;
        let turned = Env::new(&p).sun().expect("a key light");
        assert!((turned.dir - dir_from_az_el(170.0, 35.0)).length() < 1.0e-5);
        p.lights[1].enabled = false;
        assert!(Env::new(&p).sun().is_none(), "a studio without a key has no sun");
    }

    /// `params` with the backdrop black and every Add light but the key taken out:
    /// what is left is the key as `apply_lights` paints it, behind the flags. A
    /// scrim (B5) stays as bare cloth, its colour black: it hides what is behind
    /// it, the key too when it is listed after it, and adds nothing.
    fn the_key_alone(params: &HdriParams) -> HdriParams {
        let key = params.lights.iter().position(|l| l.enabled && l.key && l.blend() == Blend::Add).expect("a key light");
        let kept = |(i, l): (usize, &LightParams)| {
            if i == key || l.blend() == Blend::Multiply {
                Some(l.clone())
            } else if l.shape() == LightShape::Scrim {
                Some(LightParams { rgb: Some([0.0; 3]), ..l.clone() })
            } else {
                None
            }
        };
        HdriParams {
            studio: black_studio(),
            lights: params.lights.iter().enumerate().filter_map(kept).collect(),
            ..params.clone()
        }
    }

    /// Runs f(0..n) on every core, for the fine bakes below.
    fn on_all_cores(n: usize, f: &(dyn Fn(usize) + Sync)) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = AtomicUsize::new(0);
        let cores = std::thread::available_parallelism().map_or(4, |c| c.get()).min(16);
        std::thread::scope(|scope| {
            for _ in 0..cores {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    f(i);
                });
            }
        });
    }

    /// A baked map's ∫ L dΩ: each texel times the exact solid angle of its cell.
    fn map_emission(map: &EnvMap) -> Vec3f {
        use std::f64::consts::{PI, TAU};
        let (w, h) = (map.width, map.height);
        let mut sum = [0.0f64; 3];
        for y in 0..h {
            let (top, bottom) = (PI * y as f64 / h as f64, PI * (y + 1) as f64 / h as f64);
            let cell = TAU / w as f64 * (top.cos() - bottom.cos());
            for t in &map.data[y * w..(y + 1) * w] {
                for k in 0..3 {
                    sum[k] += t[k] as f64 * cell;
                }
            }
        }
        vec3f(sum[0] as f32, sum[1] as f32, sum[2] as f32)
    }

    /// Keys that exercise every term the key's integral must honour, each with
    /// the light list it sits in.
    fn awkward_keys() -> Vec<(&'static str, Vec<LightParams>)> {
        let ring = LightParams {
            key: true,
            shape: "ring".to_string(),
            azimuth_deg: 70.0,
            elevation_deg: 20.0,
            width_deg: 36.0,
            height_deg: 18.0,
            roll_deg: 35.0,
            inner: 0.8,
            softness: 0.0,
            ..Default::default()
        };
        let strip = LightParams {
            key: true,
            azimuth_deg: 250.0,
            elevation_deg: 10.0,
            width_deg: 8.0,
            height_deg: 60.0,
            roll_deg: 60.0,
            corner: 0.5,
            softness: 0.4,
            hotspot: 0.6,
            ..Default::default()
        };
        // A coloured Multiply gel: it keeps red and cuts green and blue.
        let gel = LightParams {
            name: "Gel".to_string(),
            azimuth_deg: 262.0,
            elevation_deg: 4.0,
            width_deg: 14.0,
            height_deg: 14.0,
            softness: 0.5,
            blend: "multiply".to_string(),
            rgb: Some([1.0, 0.5, 0.25]),
            ..Default::default()
        };
        // An Add light over the strip, after it: it is not the key's light.
        let over = LightParams { azimuth_deg: 250.0, elevation_deg: 10.0, ..Default::default() };
        // 1b (B2): a rolled, rounded hexagon key with a soft edge and a hotspot, the
        // shape core's emission, under a star-shaped blue gel, a shaped flag.
        let hexagon = LightParams {
            key: true,
            shape: "polygon".to_string(),
            azimuth_deg: 150.0,
            elevation_deg: 25.0,
            width_deg: 40.0,
            height_deg: 28.0,
            roll_deg: 20.0,
            corner: 0.3,
            softness: 0.2,
            hotspot: 0.5,
            polygon: Some(crate::hdri::PolygonParams::default()),
            ..Default::default()
        };
        let star_gel = LightParams {
            name: "Star gel".to_string(),
            shape: "polygon".to_string(),
            azimuth_deg: 152.0,
            elevation_deg: 27.0,
            width_deg: 18.0,
            height_deg: 18.0,
            softness: 0.3,
            blend: "multiply".to_string(),
            rgb: Some([0.25, 0.5, 1.0]),
            polygon: Some(crate::hdri::PolygonParams { sides: 5, star: 0.5, points: Vec::new() }),
            ..Default::default()
        };
        // 1b (B5): a soft, rolled rect key whose northern half (toward increasing
        // azimuth) is behind a scrim listed after it. A scrim is the one Add light
        // that hides what is behind it, so the map shows only the key's other half,
        // and the key carries only that.
        let screened = LightParams {
            key: true,
            azimuth_deg: 300.0,
            elevation_deg: 20.0,
            width_deg: 40.0,
            height_deg: 24.0,
            roll_deg: 15.0,
            softness: 0.3,
            hotspot: 0.4,
            ..Default::default()
        };
        let scrim = LightParams {
            name: "Scrim".to_string(),
            shape: "scrim".to_string(),
            azimuth_deg: 314.0,
            elevation_deg: 24.0,
            width_deg: 30.0,
            height_deg: 30.0,
            corner: 0.3,
            softness: 0.4,
            scrim: Some(crate::hdri::ScrimParams { source_size: 0.4, frame: 0.1 }),
            ..Default::default()
        };
        vec![
            ("a hard elliptical ring, rolled", vec![ring]),
            (
                "a rolled strip under a flag and a gel, with a flag before it and a light over it",
                vec![flag_at(250.0, 10.0, 30.0), strip, flag_at(250.0, 10.0, 8.0), gel, over],
            ),
            ("a rolled hexagon key under a star-shaped gel", vec![hexagon, star_gel]),
            ("a rolled rect key half behind a scrim listed after it", vec![screened, scrim]),
        ]
    }

    #[test]
    fn the_key_carries_the_lights_emission_over_its_cone() {
        // radiance x 2 pi (1 - cos_radius) is the light's whole ∫ L dΩ as the map
        // draws it: its shape, edge, hotspot and roll, and the flags after it.
        let mut cases: Vec<(String, HdriParams)> = ["Three-point", "Top softbox", "Rim pair", "Overcast dome", "Ring light"]
            .iter()
            .map(|name| (name.to_string(), crate::hdri::presets::preset(name).unwrap()))
            .collect();
        for (name, lights) in awkward_keys() {
            cases.push((name.to_string(), studio_params(lights)));
        }
        let mut off = Vec::new();
        for (name, p) in cases {
            let key = Env::new(&p).sun().expect("a key light");
            let cone = std::f32::consts::TAU * (1.0 - key.cos_radius);
            let got = key.radiance * cone;
            let want = map_emission(&Env::new(&the_key_alone(&p)).bake_par(2048, on_all_cores));
            let ratio = [got.x / want.x, got.y / want.y, got.z / want.z];
            if !ratio.iter().all(|r| (r - 1.0).abs() < 0.01) {
                off.push(format!("{name}: {:.4} {:.4} {:.4}", ratio[0], ratio[1], ratio[2]));
            }
        }
        assert!(off.is_empty(), "the key over its cone vs the map's emission (r g b):\n{}", off.join("\n"));
    }

    /// Every built-in key and the awkward ones, as light lists, plus two that are
    /// hardest on the integral's grid: the thinnest hard ring and the widest soft disc.
    fn key_light_lists() -> Vec<(String, Vec<LightParams>)> {
        let mut lists: Vec<(String, Vec<LightParams>)> = ["Three-point", "Top softbox", "Rim pair", "Overcast dome", "Ring light"]
            .iter()
            .map(|name| (name.to_string(), crate::hdri::presets::preset(name).unwrap().lights))
            .collect();
        for (name, lights) in awkward_keys() {
            lists.push((name.to_string(), lights));
        }
        let thin = LightParams { key: true, shape: "ring".to_string(), inner: 0.95, softness: 0.0, width_deg: 40.0, height_deg: 40.0, ..Default::default() };
        let wide = LightParams { key: true, shape: "disc".to_string(), width_deg: 170.0, height_deg: 170.0, softness: 1.0, hotspot: 0.3, ..Default::default() };
        lists.push(("the thinnest hard ring".to_string(), vec![thin]));
        lists.push(("the widest soft disc".to_string(), vec![wide]));
        lists
    }

    /// The enabled key's emission on an n x n grid.
    fn emission_on(lights: &[LightParams], n: usize) -> Vec3f {
        let studio = Studio::new(&black_studio(), lights);
        let enabled: Vec<&LightParams> = lights.iter().filter(|l| l.enabled).collect();
        let i = enabled.iter().position(|l| l.key && l.blend() == Blend::Add).expect("a key light");
        key_emission(&studio.lights[i..], &studio.fx[i..], n).emission
    }

    #[test]
    fn the_keys_integral_has_converged() {
        let mut worst = 0.0f32;
        for (name, lights) in key_light_lists() {
            let (grid, finer) = (emission_on(&lights, KEY_GRID), emission_on(&lights, 4 * KEY_GRID));
            for (g, f) in [(grid.x, finer.x), (grid.y, finer.y), (grid.z, finer.z)] {
                assert!(f > 0.0, "{name}: {finer:?}");
                worst = worst.max((g / f - 1.0).abs());
            }
            println!("{name}: {:.5}", grid.y / finer.y - 1.0);
        }
        assert!(worst < 1.0e-3, "a key moves by {worst} on a four times finer grid");
    }

    #[test]
    fn a_hard_rect_key_carries_its_solid_angle() {
        // A hard, square-cornered rect without a hotspot: its emission is its colour
        // times its exact solid angle, 4 atan(tx ty / sqrt(1 + tx² + ty²)).
        let key = LightParams { key: true, corner: 0.0, softness: 0.0, width_deg: 50.0, height_deg: 16.0, roll_deg: 25.0, ..Default::default() };
        let (tx, ty) = (25f32.to_radians().tan(), 8f32.to_radians().tan());
        let solid = 4.0 * (tx * ty / (1.0 + tx * tx + ty * ty).sqrt()).atan();
        let sun = Studio::new(&black_studio(), std::slice::from_ref(&key)).key().expect("a key light");
        let cone = std::f32::consts::TAU * (1.0 - sun.cos_radius);
        let want = light_color(&key) * solid;
        assert!((sun.radiance * cone - want).length() < 1.0e-3 * want.length(), "{:?} vs {want:?}", sun.radiance * cone);
    }

    /// M6: a key as small as a light may be (0.1 degrees) carries the integral
    /// exactly through the f32 cone it stores, and so does a sun-sized one:
    /// `EnvSun::irradiance` gives `key_emission`'s emission back to the
    /// float's rounding.
    #[test]
    fn a_small_key_carries_its_emission_through_its_f32_cone() {
        for size_deg in [0.1f32, 0.53] {
            let light = LightParams { key: true, shape: "disc".to_string(), width_deg: size_deg, height_deg: size_deg, softness: 0.0, ..Default::default() };
            let key = Studio::new(&black_studio(), std::slice::from_ref(&light)).key().expect("a key light");
            let want = emission_on(std::slice::from_ref(&light), KEY_GRID);
            let got = key.irradiance();
            for (g, w) in [(got.x, want.x), (got.y, want.y), (got.z, want.z)] {
                assert!(w > 0.0 && (g / w - 1.0).abs() < 1.0e-6, "{size_deg} deg: {g} for {w}");
            }
        }
    }

    /// A baked map's cosine-weighted share, Σ L cosθ dΩ / Σ L dΩ in luminance, θ
    /// measured from `dir`: what a surface facing `dir` receives of the map's
    /// whole emission.
    fn map_facing(map: &EnvMap, dir: Vec3f) -> f32 {
        use std::f64::consts::{PI, TAU};
        let (w, h) = (map.width, map.height);
        let (mut all, mut weighted) = (0.0f64, 0.0f64);
        for y in 0..h {
            let (top, bottom) = (PI * y as f64 / h as f64, PI * (y + 1) as f64 / h as f64);
            let cell = TAU / w as f64 * (top.cos() - bottom.cos());
            for x in 0..w {
                let t = map.data[y * w + x];
                let l = luminance(vec3f(t[0], t[1], t[2])) as f64 * cell;
                let d = vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32]));
                all += l;
                weighted += l * d.dot(dir) as f64;
            }
        }
        (weighted / all) as f32
    }

    #[test]
    fn the_key_faces_with_the_share_of_its_emission_the_map_delivers() {
        // facing is ∫ L cosθ dΩ / ∫ L dΩ over the key's reach, from the same pass
        // as its radiance: checked against the key as the map draws it.
        let mut cases: Vec<(String, HdriParams)> = ["Three-point", "Top softbox", "Rim pair", "Overcast dome", "Ring light"]
            .iter()
            .map(|name| (name.to_string(), crate::hdri::presets::preset(name).unwrap()))
            .collect();
        for (name, lights) in awkward_keys() {
            cases.push((name.to_string(), studio_params(lights)));
        }
        let mut off = Vec::new();
        for (name, p) in cases {
            let key = Env::new(&p).sun().expect("a key light");
            assert!(key.validate().is_ok(), "{name}: {key:?}");
            let want = map_facing(&Env::new(&the_key_alone(&p)).bake_par(1024, on_all_cores), key.dir);
            if (key.facing - want).abs() > 0.01 {
                off.push(format!("{name}: facing {:.4}, the map's {want:.4}", key.facing));
            }
        }
        assert!(off.is_empty(), "the key's facing share vs the map's:\n{}", off.join("\n"));
    }

    #[test]
    fn the_built_in_keys_face_as_the_spec_measured() {
        // The numbers the EnvSun doc quotes. The dome is a 110 degree disc, the
        // softbox 70 degrees wide: a surface facing them gets less than the
        // whole of radiance x solid angle.
        for (name, facing) in [("Overcast dome", 0.78), ("Top softbox", 0.93), ("Rim pair", 0.95), ("Three-point", 0.98), ("Ring light", 0.98)] {
            let key = Env::new(&crate::hdri::presets::preset(name).unwrap()).sun().expect("a key light");
            assert!((key.facing - facing).abs() < 0.01, "{name}: facing {} vs {facing}", key.facing);
        }
        // A key a fraction of a degree wide faces fully.
        let tiny = LightParams { key: true, width_deg: 0.2, height_deg: 0.2, ..Default::default() };
        let key = Studio::new(&black_studio(), std::slice::from_ref(&tiny)).key().expect("a key light");
        assert!(key.facing > 0.9999 && key.facing <= 1.0, "{}", key.facing);
        // A key that draws nothing (a black light) has no direction to weigh: it faces fully.
        let dark = LightParams { key: true, rgb: Some([0.0; 3]), ..Default::default() };
        let key = Studio::new(&black_studio(), std::slice::from_ref(&dark)).key().expect("a key light");
        assert_eq!((key.radiance, key.facing), (Vec3f::default(), 1.0));
        assert!(key.validate().is_ok(), "{key:?}");
    }

    #[test]
    fn nothing_of_the_key_is_drawn_outside_its_covering_cone() {
        // cos_cover is the cone of the key's reach box: its corners and its soft
        // edge lie inside it, so the keyless bake, which evaluates only that cone
        // again (N3), and remove_sun's fill both take all of the key. The map is
        // black outside the cone, exactly, on a one degree grid over the sphere
        // and on rings just past its edge.
        for (name, lights) in key_light_lists() {
            let alone = the_key_alone(&studio_params(lights));
            let env = Env::new(&alone);
            let key = env.sun().expect("a key light");
            assert!(key.validate().is_ok(), "{name}: {key:?}");
            assert!(key.cos_cover < key.cos_radius, "{name}: the reach box is wider than the cone, {key:?}");
            let mut lit = 0;
            for iy in 0..180 {
                for ix in 0..360 {
                    let d = dir_from_az_el(ix as f32 + 0.5, iy as f32 - 89.5);
                    let seen = env.radiance(d) != Vec3f::default();
                    lit += seen as usize;
                    assert!(!seen || d.dot(key.dir) >= key.cos_cover, "{name}: lit at {:?}, outside the cone", az_el_from_dir(d));
                }
            }
            assert!(lit > 0, "{name}");
            // Rings of directions a hair and a few percent past the cone's edge.
            let side = Vec3f::cross(key.dir, if key.dir.y.abs() < 0.9 { vec3f(0.0, 1.0, 0.0) } else { vec3f(1.0, 0.0, 0.0) }).normalize();
            let up = Vec3f::cross(key.dir, side);
            let edge = key.cos_cover.acos();
            for scale in [1.0005f32, 1.01, 1.05] {
                let theta = (edge * scale).min(std::f32::consts::FRAC_PI_2);
                for i in 0..720 {
                    let phi = std::f32::consts::TAU * i as f32 / 720.0;
                    let d = key.dir * theta.cos() + (side * phi.cos() + up * phi.sin()) * theta.sin();
                    assert_eq!(env.radiance(d), Vec3f::default(), "{name}: lit {scale} x the cone's edge away, at ring angle {i}");
                }
            }
        }
    }

    /// N3: the engine lights a generated map from its keyless copy
    /// (`Env::bake_keyless_par`, through `envmap::bake_generated_env_map`), which
    /// must be the map with exactly the key light switched off: the flags after
    /// it, the light over it and the shape core's lights kept. Texel for texel:
    /// the copy re-evaluates the key's covering cone through the lookup without
    /// it, and outside the cone the key draws nothing.
    #[test]
    fn a_keys_lighting_copy_is_the_map_with_the_key_switched_off() {
        for (name, lights) in awkward_keys() {
            let p = studio_params(lights);
            let baked = crate::hdri::envmap::bake_generated_env_map(&p, 64, on_all_cores);
            let mut off = p.clone();
            let key = off.lights.iter().position(|l| l.enabled && l.key && l.blend() == Blend::Add).expect("a key light");
            off.lights[key].enabled = false;
            assert!(baked.keyless.as_ref() == Some(&Env::new(&off).bake(64)), "{name}: the keyless copy is not the map without its key");
            assert!(baked.keyless.as_ref() != Some(&baked.map), "{name}: the key drew nothing");
        }
    }

    /// A stroke key (1b, B3): a soft 60 degree arch with a hotspot and a
    /// brightness ramp to a third.
    fn arch_stroke() -> LightParams {
        LightParams {
            key: true,
            shape: "stroke".to_string(),
            softness: 0.6,
            hotspot: 0.3,
            stroke: Some(crate::hdri::StrokeParams {
                points: vec![[60.0, 0.0], [90.0, 8.0], [120.0, 0.0]],
                width_deg: 4.0,
                end_rgb: Some([0.3, 0.3, 0.3]),
                ..crate::hdri::StrokeParams::default()
            }),
            ..Default::default()
        }
    }

    /// Plan 2's `facing` and `cos_cover` for a stroke key come from the shape
    /// core, and they are the map's: the facing share within 0.01 of the baked
    /// map's cosine share, and no lit direction outside the covering cone.
    #[test]
    fn a_stroke_keys_facing_and_covering_cone_are_the_maps() {
        let env = Env::new(&the_key_alone(&studio_params(vec![arch_stroke()])));
        let key = env.sun().expect("the stroke is the key light");
        assert!(key.validate().is_ok(), "{key:?}");
        assert!(key.cos_cover < key.cos_radius, "the soft edge reaches past the key cone: {key:?}");
        let want = map_facing(&env.bake_par(1024, on_all_cores), key.dir);
        assert!((key.facing - want).abs() < 0.01, "facing {} vs the map's {want}", key.facing);
        for iy in 0..180 {
            for ix in 0..360 {
                let d = dir_from_az_el(ix as f32 + 0.5, iy as f32 - 89.5);
                assert!(env.radiance(d) == Vec3f::default() || d.dot(key.dir) >= key.cos_cover, "lit at {:?}, outside the covering cone", az_el_from_dir(d));
            }
        }
    }

    /// Stroke keys that exercise every term of the shape core's key integral,
    /// each soft enough for a 2K bake to resolve its edge (a hard edge lying
    /// along a texel row is off by a few per cent at 2K, and the other way at 4K).
    fn stroke_keys() -> Vec<(&'static str, LightParams)> {
        let stroke = |points: Vec<[f32; 2]>, width_deg: f32, softness: f32, hotspot: f32| LightParams {
            key: true,
            shape: "stroke".to_string(),
            softness,
            hotspot,
            stroke: Some(crate::hdri::StrokeParams { points, width_deg, ..crate::hdri::StrokeParams::default() }),
            ..Default::default()
        };
        let with = |mut light: LightParams, edit: &dyn Fn(&mut crate::hdri::StrokeParams)| {
            edit(light.stroke.as_mut().expect("a stroke block"));
            light
        };
        vec![
            ("a soft arch with a hotspot and a brightness ramp", arch_stroke()),
            ("a glowing tube: fully soft, with a full hotspot", stroke(vec![[60.0, -5.0], [120.0, -5.0]], 6.0, 1.0, 1.0)),
            ("a short wide stroke, a third of it its round ends", stroke(vec![[80.0, 10.0], [100.0, 10.0]], 10.0, 0.5, 0.5)),
            (
                "a taper to nothing that also fades to black",
                with(stroke(vec![[60.0, 5.0], [120.0, 5.0]], 6.0, 0.4, 0.0), &|s| {
                    s.profile = vec![1.0, 0.0];
                    s.end_rgb = Some([0.0; 3]);
                }),
            ),
            (
                "a closed loop with a Kelvin ramp",
                with(stroke(vec![[80.0, 0.0], [100.0, 0.0], [90.0, 15.0]], 3.0, 0.5, 0.0), &|s| {
                    s.closed = true;
                    s.end_kelvin = Some(2000.0);
                }),
            ),
            ("a dot: two points half a degree apart", stroke(vec![[90.0, 20.0], [90.5, 20.0]], 8.0, 0.5, 0.4)),
            ("the arch over a light without points", LightParams { key: true, shape: "stroke".to_string(), ..Default::default() }),
            // D1 (L:40): a band wider than the loop it runs round laps over
            // itself, so it draws more than its length times its width.
            (
                "a closed loop 45 degrees wide",
                with(stroke(vec![[80.0, 0.0], [100.0, 0.0], [90.0, 15.0]], 45.0, 0.5, 0.0), &|s| s.closed = true),
            ),
        ]
    }

    /// D2: a stroke that spreads wider than an 85 degree key (the widest a
    /// rect's or a disc's key cone gets) is not handed to the engine as the
    /// key, whichever way it spreads: reaching more than 85 degrees from its
    /// middle (a strip round the whole horizon, which has no middle to light
    /// from, or half way round), or lying all round its middle further out
    /// than an even 85 degree disc's light does (L:41's ring at elevation 10,
    /// every point 80 degrees from the zenith it would light from, facing
    /// 0.17). Its light stays in the map's lighting, and under a sky the sun
    /// leads again. An arch that reaches just under the limit is still the
    /// key, and so is a ring light drawn as a stroke overhead.
    #[test]
    fn a_stroke_that_circles_the_sky_is_not_the_key() {
        let strip = |points: Vec<[f32; 2]>, closed: bool| LightParams {
            key: true,
            shape: "stroke".to_string(),
            stroke: Some(crate::hdri::StrokeParams { points, closed, width_deg: 4.0, ..crate::hdri::StrokeParams::default() }),
            ..Default::default()
        };
        let ring = |elevation_deg: f32| strip((0..6).map(|i| [i as f32 * 60.0, elevation_deg]).collect(), true);
        let round = ring(0.0);
        let half = strip(vec![[0.0, 5.0], [90.0, 5.0], [180.0, 5.0]], false);
        let arch = strip(vec![[10.0, 0.0], [90.0, 0.0], [170.0, 0.0]], false);
        let low_ring = ring(10.0);
        for (name, light, on_line) in [("round the horizon", &round, 0.0), ("half way round", &half, 5.0), ("a ring at elevation 10", &low_ring, 10.0)] {
            assert!(spreads_too_wide_to_be_key(light), "{name}");
            assert!(Studio::new(&black_studio(), std::slice::from_ref(light)).key().is_none(), "{name}: not the key");
            let sky = HdriParams { lights: vec![light.clone()], ..Default::default() };
            assert!(matches!(Env::new(&sky).key(), Some((crate::hdri::KeyTerm::Sun, _))), "{name}: the sun leads under a sky");
            // Drawn as before: it is a light, only not the key.
            assert!(Env::new(&studio_params(vec![light.clone()])).radiance(dir_from_az_el(90.0, on_line)) != Vec3f::default(), "{name}");
        }
        assert!(!spreads_too_wide_to_be_key(&arch), "82 degrees from its middle");
        let key = Studio::new(&black_studio(), &[arch]).key().expect("the arch is the key");
        assert!(key.validate().is_ok() && key.cos_radius > 85f32.to_radians().cos(), "{key:?}");
        // A halo 60 degrees across overhead lights from the zenith as a ring
        // light does: a surface facing up receives 0.87 of it.
        let halo = ring(60.0);
        assert!(!spreads_too_wide_to_be_key(&halo), "a ring at elevation 60");
        // The even 85 degree disc's share, 0.544, is a ring's at elevation 33.
        assert!(spreads_too_wide_to_be_key(&ring(31.0)) && !spreads_too_wide_to_be_key(&ring(35.0)), "the limit lies near elevation 33");
        let key = Studio::new(&black_studio(), &[halo]).key().expect("the halo is the key");
        assert!(key.validate().is_ok() && key.dir.y > 0.99 && key.facing > 0.8, "{key:?}");
        // Only a stroke spreads: a rect as wide as a light may be still leads.
        let wide = LightParams { key: true, width_deg: 170.0, height_deg: 170.0, ..Default::default() };
        assert!(!spreads_too_wide_to_be_key(&wide) && Studio::new(&black_studio(), &[wide]).key().is_some());
    }

    /// `EnvSun`'s one meaning for a stroke key: radiance x the key cone is the
    /// stroke's whole emission as the map draws it, its round ends, soft edge,
    /// hotspot, taper and ramp included (to half a per cent: the shape core's
    /// sum comes within 0.2 % of a 2K bake for each of these), and its facing
    /// share is the map's.
    #[test]
    fn a_stroke_key_carries_its_emission_over_its_cone() {
        let mut off = Vec::new();
        for (name, light) in stroke_keys() {
            let p = studio_params(vec![light]);
            let key = Env::new(&p).sun().expect("the stroke is the key light");
            assert!(key.validate().is_ok(), "{name}: {key:?}");
            let map = Env::new(&the_key_alone(&p)).bake_par(2048, on_all_cores);
            let got = key.radiance * (std::f32::consts::TAU * (1.0 - key.cos_radius));
            let want = map_emission(&map);
            let ratio = [got.x / want.x, got.y / want.y, got.z / want.z];
            let facing = map_facing(&map, key.dir);
            if !ratio.iter().all(|r| (r - 1.0).abs() < 0.005) || (key.facing - facing).abs() > 0.01 {
                off.push(format!("{name}: emission {:.4} {:.4} {:.4}, facing {:.4} vs the map's {facing:.4}", ratio[0], ratio[1], ratio[2], key.facing));
            }
        }
        assert!(off.is_empty(), "the stroke key over its cone vs the map (r g b):\n{}", off.join("\n"));
    }

    /// N3 for a stroke key: the engine's lighting copy is the map with the stroke
    /// switched off, texel for texel, and a rect inside the stroke's covering
    /// cone (under the arch) stays in it. The arch is taken with each of its
    /// colourings: a ramped stroke takes its colour from its ramp, not from the
    /// colour the light loop hands it, so a ramped key leaves the copy only
    /// through B2's `ShapedLight::hide`.
    #[test]
    fn a_stroke_keys_lighting_copy_is_the_map_with_the_stroke_switched_off() {
        let with_ramp = |end_kelvin: Option<f32>, end_rgb: Option<[f32; 3]>| {
            let mut arch = arch_stroke();
            let stroke = arch.stroke.as_mut().expect("the arch is a stroke");
            stroke.end_kelvin = end_kelvin;
            stroke.end_rgb = end_rgb;
            arch
        };
        let arches = [
            ("the arch's brightness ramp (end_rgb)", arch_stroke()),
            ("a Kelvin ramp (end_kelvin)", with_ramp(Some(2000.0), None)),
            ("one colour all along", with_ramp(None, None)),
        ];
        for (name, arch) in arches {
            let rect = LightParams { azimuth_deg: 90.0, elevation_deg: 12.0, width_deg: 10.0, height_deg: 6.0, ..Default::default() };
            let p = studio_params(vec![arch, rect]);
            let baked = crate::hdri::envmap::bake_generated_env_map(&p, 128, on_all_cores);
            let mut off = p.clone();
            off.lights[0].enabled = false;
            assert!(baked.keyless.as_ref() == Some(&Env::new(&off).bake(128)), "{name}: the keyless copy is not the map without the stroke");
            assert!(baked.keyless.as_ref() != Some(&baked.map), "{name}: the stroke drew nothing");
        }
    }

    /// A scrim listed after the key hides it as a flag would (B5): a key wholly
    /// behind the cloth draws nothing in the map, so it hands the engine no light.
    #[test]
    fn a_key_behind_a_scrim_carries_nothing() {
        let key = LightParams { key: true, azimuth_deg: 90.0, elevation_deg: 0.0, width_deg: 20.0, height_deg: 20.0, ..Default::default() };
        let scrim = LightParams { shape: "scrim".to_string(), azimuth_deg: 90.0, elevation_deg: 0.0, width_deg: 40.0, height_deg: 40.0, softness: 0.0, ..Default::default() };
        let sun = Studio::new(&black_studio(), &[key, scrim]).key().expect("the rect is the key");
        assert_eq!(sun.radiance, Vec3f::default(), "{sun:?}");
        assert!(sun.validate().is_ok(), "{sun:?}");
    }

    /// N3 for a scrim key (B5): the engine's lighting copy loses the cloth's
    /// glow and keeps the cloth, so what the scrim hides stays hidden: the copy
    /// is the map with the scrim's colour black, not the map without the scrim.
    #[test]
    fn a_scrim_keys_lighting_copy_keeps_its_cloth() {
        let behind = LightParams { azimuth_deg: 90.0, elevation_deg: 0.0, width_deg: 30.0, height_deg: 30.0, ..Default::default() };
        let scrim = LightParams {
            key: true,
            shape: "scrim".to_string(),
            azimuth_deg: 90.0,
            elevation_deg: 0.0,
            width_deg: 40.0,
            height_deg: 40.0,
            scrim: Some(crate::hdri::ScrimParams { source_size: 0.5, frame: 0.05 }),
            ..Default::default()
        };
        let p = studio_params(vec![behind, scrim]);
        let baked = crate::hdri::envmap::bake_generated_env_map(&p, 64, on_all_cores);
        let mut dark = p.clone();
        dark.lights[1].rgb = Some([0.0; 3]);
        assert!(baked.keyless.as_ref() == Some(&Env::new(&dark).bake(64)), "the copy is the map with the glow gone and the cloth kept");
        let mut off = p.clone();
        off.lights[1].enabled = false;
        assert!(baked.keyless.as_ref() != Some(&Env::new(&off).bake(64)), "the light behind the cloth stays hidden in the copy");
    }

    #[test]
    fn a_hard_rect_keys_covering_cone_ends_at_its_corner() {
        // Square corners, no soft edge: the farthest lit point is the corner of the
        // box, tan(25) by tan(8) in the tangent plane, and the cone ends there.
        let light = LightParams { key: true, corner: 0.0, softness: 0.0, width_deg: 50.0, height_deg: 16.0, roll_deg: 25.0, ..Default::default() };
        let p = the_key_alone(&studio_params(vec![light.clone()]));
        let env = Env::new(&p);
        let key = env.sun().expect("a key light");
        let frame = light_frame(light.azimuth_deg, light.elevation_deg, light.roll_deg);
        let (tx, ty) = (25f32.to_radians().tan(), 8f32.to_radians().tan());
        let at = |x: f32, y: f32| (frame.center + frame.right * x + frame.up * y).normalize();
        let corner = at(tx * 0.999, ty * 0.999);
        assert!(luminance(env.radiance(corner)) > 0.0, "the corner is lit");
        let corner_deg = corner.dot(key.dir).acos().to_degrees();
        let cover_deg = key.cos_cover.acos().to_degrees();
        assert!(cover_deg >= corner_deg && cover_deg < corner_deg + 0.05, "cone {cover_deg} deg, corner {corner_deg} deg");
        // A hair past the box's long side the map is black.
        let wide = (frame.center + frame.right * (tx * 1.001) + frame.up * (ty * 0.999)).normalize();
        assert_eq!(env.radiance(wide), Vec3f::default());
    }

    #[test]
    fn lights_overlay_the_sky_mode_too() {
        // Sky mode (the default). Whatever the sky layers draw, a light adds its colour on top.
        let mut p = HdriParams::default();
        let dir = dir_from_az_el(60.0, 20.0);
        let without = Env::new(&p).radiance(dir);
        let light = LightParams {
            azimuth_deg: 60.0,
            elevation_deg: 20.0,
            ..Default::default()
        };
        p.lights = vec![light.clone()];
        let with = Env::new(&p).radiance(dir);
        let expected = light_color(&light);
        assert!(((with - without) - expected).length() < 1.0e-3 * luminance(expected), "{with:?} - {without:?}");
    }
}
