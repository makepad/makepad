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
use super::{dir_from_az_el, Blend, EnvSun, LightParams, LightShape, StudioParams};
use crate::sky::luminance;
use super::shaped::ShapedLight;
use super::gels::{gel_stack, shifted_kelvin};
use super::spectrum::light_source_rgb;

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
/// - Gels (1b) filter the result. A conversion gel moves a Kelvin light's
///   Kelvin by its mireds and takes the light an ideal filter of that shift
///   costs; every other gel, and a conversion gel on an explicit colour or a
///   named source, is an RGB transmission (gels.rs).
pub fn light_color(light: &LightParams) -> Vec3f {
    let gels = gel_stack(light.gels.as_deref().unwrap_or(&[]));
    let base = if let Some(rgb) = light.rgb {
        // An explicit colour has no Kelvin to move: a conversion gel filters it.
        vec3f(rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)) * gels.conversion
    } else if let Some(source) = light_source_rgb(light) {
        // A named source: its spectrum's colour in place of the Kelvin's,
        // tinted the same way. It has no Kelvin to move either (the knob
        // LED's Kelvin is part of its spectrum).
        let c = source * tint_rgb(light.tint);
        c / luminance(c).max(1.0e-6) * gels.conversion
    } else {
        let kelvin = sane(light.kelvin, 1000.0, 20000.0);
        // A conversion gel moves the Kelvin by its mireds, so the hue is
        // exactly the shifted blackbody's.
        let c = kelvin_to_rgb(shifted_kelvin(kelvin, gels.mired)) * tint_rgb(light.tint);
        let c = c / luminance(c).max(1.0e-6);
        if gels.mired == 0.0 {
            c
        } else {
            // The light the gel costs: what its filter passes of this
            // source (kelvin_to_rgb has luminance 1).
            c * luminance(kelvin_to_rgb(kelvin) * gels.conversion)
        }
    };
    base * gels.filter * 2.0f32.powf(sane(light.intensity_ev, -10.0, 20.0))
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
/// Returns None when `dir` is more than about 87 degrees from the centre (dot < 0.05),
/// where the projection runs off to infinity.
pub fn project(frame: &LightFrame, dir: Vec3f) -> Option<Vec2f> {
    let c = dir.dot(frame.center);
    // Written negated so a NaN direction is rejected too.
    if !(c >= 0.05) {
        return None;
    }
    Some(vec2f(dir.dot(frame.right) / c, dir.dot(frame.up) / c))
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
}

impl Studio {
    /// `lights` are taken in list order; disabled ones are dropped here. The key is the
    /// first enabled Add light marked `key`. A flag (Multiply) never leads the lighting.
    ///
    /// The key's cone covers the larger of width and height, and its radiance is the
    /// light's emission spread over that cone ([`EnvSun`]'s one meaning), integrated
    /// here once per map by [`key_emission`], which also gives the share a surface
    /// facing the key receives (`facing`) and the cone that holds the whole reach
    /// (`cos_cover`).
    /// A stroke (1b) is placed by its points, not by its azimuth and elevation, so its
    /// cone, its radiance (the same meaning), its facing share and its covering cone
    /// come from the shape core instead.
    pub fn new(studio: &StudioParams, lights: &[LightParams]) -> Studio {
        let rgb = |c: [f32; 3]| vec3f(c[0].max(0.0), c[1].max(0.0), c[2].max(0.0));
        let enabled: Vec<&LightParams> = lights.iter().filter(|l| l.enabled).collect();
        let prepared: Vec<PreparedLight> = enabled.iter().map(|l| PreparedLight::new(l)).collect();
        let key_index = enabled.iter().position(|l| l.key && l.blend() == Blend::Add);
        let key = key_index.map(|i| {
            let l = enabled[i];
            if let Some(shaped) = &prepared[i].shaped {
                if let (Some(radiance), Some((facing, cos_cover))) = (shaped.key_color(prepared[i].color), shaped.key_spread()) {
                    let (dir, cos_radius) = shaped.key_cone();
                    return EnvSun { dir, radiance, cos_radius, facing, cos_cover: cos_cover.min(cos_radius) };
                }
            }
            let radius = (0.5 * sane(l.width_deg.max(l.height_deg), 0.1, 170.0)).to_radians();
            let integral = key_emission(&prepared[i..], KEY_GRID);
            EnvSun {
                dir: dir_from_az_el(l.azimuth_deg, l.elevation_deg),
                // Over the cone the f32 cosine names, the one every consumer of
                // the key computes, so the emission comes back exactly however
                // small the light (`EnvSun::cone_radiance`).
                radiance: EnvSun::cone_radiance(integral.emission, radius.cos()),
                cos_radius: radius.cos(),
                facing: integral.facing,
                // The reach box's cone can only be wider than the nominal one
                // (it holds the half-size on both sides); the min keeps rounding
                // from putting it above.
                cos_cover: integral.cos_cover.min(radius.cos()),
            }
        });
        Studio {
            top: rgb(studio.top),
            horizon: rgb(studio.horizon),
            floor: rgb(studio.floor),
            soft_deg: sane(studio.horizon_softness, 0.01, 1.0) * 90.0,
            lights: prepared,
            key,
            key_index,
        }
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

    /// Draws the lights over `base` in list order.
    /// - **Add** accumulates the light's colour.
    /// - **Multiply** scales everything drawn so far by its colour: fully at the core,
    ///   not at all outside. That is how a black flag cuts a softbox or the sky.
    /// - Polygons, strokes and scrims follow the same rules through the shape core,
    ///   except that an Add scrim is cloth: it hides what is behind it, then glows.
    pub fn apply_lights(&self, dir: Vec3f, base: Vec3f) -> Vec3f {
        self.apply_lights_skipping(dir, base, None)
    }

    /// [`Self::apply_lights`] without the key light (N3: the map without its
    /// key is the source of the engine's lighting copy). Skipping it is
    /// exactly the map with the key disabled: every other light, the flags
    /// after the key included, draws as before.
    pub fn apply_lights_without_key(&self, dir: Vec3f, base: Vec3f) -> Vec3f {
        self.apply_lights_skipping(dir, base, self.key_index)
    }

    fn apply_lights_skipping(&self, dir: Vec3f, base: Vec3f, skip: Option<usize>) -> Vec3f {
        let mut c = base;
        for (i, light) in self.lights.iter().enumerate() {
            if let Some(shaped) = &light.shaped {
                // The key left out (`skip`, N3) loses its own light, while
                // whatever it hides stays hidden (B5's scrim is cloth in front
                // of what is behind it): `ShapedLight::hide`. For a light that
                // hides nothing that is the same as skipping it.
                c = if skip == Some(i) { shaped.hide(dir, c) } else { shaped.compose(dir, c, light.color) };
                continue;
            }
            if skip == Some(i) {
                continue;
            }
            let Some(p) = project(&light.frame, dir) else { continue };
            let m = light.mask(p);
            if !(m > 0.0) {
                continue;
            }
            match light.blend {
                Blend::Add => c += light.color * m,
                Blend::Multiply => c *= Vec3f::all(1.0) + (light.color - Vec3f::all(1.0)) * m,
            }
        }
        c
    }

    /// The key light in the map's own frame (unrotated, unscaled). Its cone covers the
    /// larger of width and height, and its radiance is the light's whole emission over
    /// that cone, not its peak colour: a thin strip or a ring that fills a tenth of its
    /// cone hands over about a tenth of its colour. `facing` is the share of that
    /// emission a surface facing the key receives (a wide softbox's is below 1) and
    /// `cos_cover` the cone around the key's centre that holds its reach box.
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
/// - Every Multiply light after it scales it as it does in the map, and so does a
///   scrim's cloth (B5), the one Add light that hides what is behind it
///   (`ShapedLight::hide`). Any other Add light after it is a light of its own, and a
///   light before it touches only what came before.
///
/// A midpoint grid of n x n points over the key's reach box in its own tangent plane,
/// where dΩ = dx dy / (1 + x² + y²)^(3/2) and the cosine of the angle from the centre
/// is 1 / sqrt(1 + x² + y²). Outside that box the mask is exactly zero, so nothing is
/// missed, and the farthest point of the box is its corner: the covering cone's
/// cosine is 1 / sqrt(1 + rx² + ry²), no smaller than `project`'s 0.05 limit, past
/// which nothing is drawn.
fn key_emission(lights: &[PreparedLight], n: usize) -> KeyIntegral {
    let Some((key, after)) = lights.split_first() else {
        return KeyIntegral { emission: Vec3f::default(), facing: 1.0, cos_cover: 1.0 };
    };
    // What dims the key as the map draws it: the flags and gels, and the scrims'
    // cloth in front of it.
    let flags: Vec<&PreparedLight> = after
        .iter()
        .filter(|l| l.blend == Blend::Multiply || l.shaped.as_ref().is_some_and(|s| s.hides_behind()))
        .collect();
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
            // The weight `apply_lights` gives the key's colour here: A3's mask, or the
            // shape core's emission for a light it draws (1b: a polygon, B5's scrim).
            let m = match &key.shaped {
                Some(shaped) => shaped.sample(dir()).emit,
                None => key.mask(vec2f(x, y)),
            };
            if !(m > 0.0) {
                continue;
            }
            let cos = 1.0 / r2.sqrt();
            let mut w = Vec3f::all(m * cos / r2);
            if !flags.is_empty() {
                let dir = dir();
                for flag in &flags {
                    // As `apply_lights` draws the flag: a shaped one through the shape
                    // core, and a scrim's cloth without its own glow.
                    if let Some(shaped) = &flag.shaped {
                        w = if flag.blend == Blend::Multiply { shaped.compose(dir, w, flag.color) } else { shaped.hide(dir, w) };
                        continue;
                    }
                    let Some(q) = project(&flag.frame, dir) else { continue };
                    let f = flag.mask(q);
                    if f > 0.0 {
                        w *= Vec3f::all(1.0) + (flag.color - Vec3f::all(1.0)) * f;
                    }
                }
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
    let scaled = |s: [f64; 3]| key.color * vec3f((s[0] * area) as f32, (s[1] * area) as f32, (s[2] * area) as f32);
    let emission = scaled(sum);
    // Luminance weighs the channels as the eye does: a coloured gel's key still
    // gets one share.
    let (total, cosine) = (luminance(emission), luminance(scaled(weighted)));
    let facing = if total > 0.0 { (cosine / total).clamp(0.0, 1.0) } else { 1.0 };
    let cos_cover = (1.0 / (1.0 + key.reach.x * key.reach.x + key.reach.y * key.reach.y).sqrt()).max(0.05);
    KeyIntegral { emission, facing, cos_cover }
}

/// One enabled light, reduced to what the per-pixel evaluation needs.
#[derive(Clone, Debug)]
struct PreparedLight {
    frame: LightFrame,
    shape: LightShape,
    blend: Blend,
    /// Half extents in the tangent plane: tan(width/2), tan(height/2).
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
    color: Vec3f,
    /// Polygons, strokes and scrims (1b): the shape core draws the light.
    shaped: Option<ShapedLight>,
}

impl PreparedLight {
    fn new(l: &LightParams) -> PreparedLight {
        let width = sane(l.width_deg, 0.1, 170.0);
        let height = sane(l.height_deg, 0.1, 170.0);
        let half = vec2f((0.5 * width).to_radians().tan(), (0.5 * height).to_radians().tan());
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
            frame: light_frame(l.azimuth_deg, l.elevation_deg, l.roll_deg),
            shape,
            blend: l.blend(),
            half,
            corner: sane(l.corner, 0.0, 1.0) * short,
            ring_mid: 0.5 * (1.0 + inner),
            ring_half,
            band,
            reach: vec2f(half.x + band, half.y + band),
            hotspot: HOTSPOT_FALLOFF * sane(l.hotspot, 0.0, 1.0),
            color: light_color(l),
            shaped: ShapedLight::new(l),
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
        key_emission(&studio.lights[i..], n).emission
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
        ]
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
