//! Clear-sky atmosphere for the HDRI generator.
//!
//! Single scattering by air (Rayleigh) and haze (Mie, with the Cornette-Shanks
//! form of the Henyey-Greenstein phase), with ozone absorption, on a spherical
//! Earth. The marching scheme follows wwwtyro/glsl-atmosphere (Unlicense,
//! https://github.com/wwwtyro/glsl-atmosphere): march the view ray through the
//! shell and, at every sample, take the optical depth toward the sun. What an
//! environment map needs beyond that shader is added here: ozone, the Earth's
//! shadow (so twilight is right), a tabulated optical depth toward the sun in
//! place of the nested march, a small multiple-scattering term, a lit ground
//! below the horizon and an energy-preserving sun disc.
//!
//! A clear sky mirrors about the sun's vertical plane, so everything that
//! depends on the view is computed once into a sky-view table indexed by the
//! view elevation and the azimuth away from the sun; `sky()` is one bilinear
//! lookup.
//!
//! Units: lengths in metres, tabulated in f64 so planet-sized numbers keep
//! their precision; radiance in the scene units fixed by `SUN_IRRADIANCE`.
//! Directions are in the map's own frame (before the map rotation): y up,
//! x east, -z north, the frame `ibl::dir_to_equirect_uv` maps.
//!
//! ## Radiance scale
//!
//! Everything is linear Rec.709 radiance in the units `SUN_IRRADIANCE` sets.
//! With 20.0 per channel at the top of the atmosphere, a high sun delivers a
//! ground irradiance of about 17.6 in luminance ((18.8, 17.4, 15.4) rgb), so
//! a white wall facing it reads E / pi = 5.6 plus a few tenths from the sky,
//! a clear zenith sits at a few tenths and the 0.53 deg disc at about 3e5.
//! Against the engine, per the spec's Conventions:
//!
//! - `ibl::EnvPreset` (render_material/src/ibl.rs:285-381) paints surfaces at
//!   0.1-1, softboxes at 2-9 and `Sunset`'s 0.6 deg disc at 300, which carries
//!   only ~0.1 of irradiance: those are looks, not a photometric scale. The
//!   generator keeps the sun-to-sky ratio real (a thousand times the preset's
//!   disc) so `ibl::prefilter` and `ibl::sh9` see a key light that agrees with
//!   `Env::sun`.
//! - The renderer's HDR lane (`SunLight::to_hdr`, sun.rs:103-117): the direct
//!   term is 0.72 x 3.0 = 2.16 and the shaders apply it as `color * N.L` with
//!   no 1/pi, so "a noon white wall sits near 2.0 before exposure"; the day
//!   fill is 0.28 x 0.8 = 0.22 per hemisphere; the analytic dome is drawn at
//!   its Preetham luminance x `HDR_SKY_GAIN` = 0.4 (renderer/frame.rs:9), and
//!   `SunLight::hdr_exposure` meters 0.75 over the rig's key, clamped to
//!   0.25..3.2.
//!
//! The generator's outdoor scale is therefore about 3x the engine's HDR lane
//! (a noon wall of 6 against 2). Nothing here rescales: `Ibl.intensity`
//! (0.35 reproduces the stock brightness) or phase 2's environment-metered
//! exposure absorbs the factor, and `tests::the_documented_scale_holds` pins
//! these numbers so this note cannot rot.

use makepad_draw::*;
use super::{dir_from_az_el, AtmosphereParams, EnvSun, SunDiscParams, SunMode, SunParams};
use crate::sky::{noaa_solar_position, SkyDate};

/// Top-of-atmosphere sun irradiance, in scene units per channel. The scale is
/// arbitrary but fixed (see the module header for how it compares with the
/// engine's presets and HDR lane): with it a clear high-sun zenith lands at a
/// luminance of a few tenths, and sun, sky, clouds and ground keep their
/// physical ratios to one another.
pub const SUN_IRRADIANCE: f32 = 20.0;

const PLANET_RADIUS: f64 = 6_360_000.0;
const TOP_RADIUS: f64 = 6_420_000.0;
/// The viewer stands 200 m up, so a level view ray clears the ground.
const VIEWER_HEIGHT: f64 = 200.0;
const RAYLEIGH_SCALE_HEIGHT: f64 = 8_000.0;
const MIE_SCALE_HEIGHT: f64 = 1_200.0;
/// Sea-level coefficients per metre at 680, 550 and 440 nm, standing in for
/// the Rec.709 primaries (the values Bruneton and Hillaire use).
const RAYLEIGH_SCATTERING: [f64; 3] = [5.802e-6, 13.558e-6, 33.1e-6];
const MIE_SCATTERING: f64 = 3.996e-6;
/// Haze absorbs a little as well: its extinction is 10 % above its scattering.
const MIE_EXTINCTION: f64 = 4.440e-6;
const MIE_G: f64 = 0.76;
/// Ozone only absorbs, mostly orange and green; over the long grazing paths of
/// twilight that is what keeps the zenith blue.
const OZONE_ABSORPTION: [f64; 3] = [0.650e-6, 1.881e-6, 0.085e-6];
/// Ozone density is a tent: 1 at 25 km, 0 at 10 km and at 40 km.
const OZONE_PEAK_HEIGHT: f64 = 25_000.0;
const OZONE_HALF_WIDTH: f64 = 15_000.0;

/// Sky-view table. Columns: azimuth away from the sun, 0..pi (the other half
/// of the sky is the mirror image). Rows: view elevation 0..90 deg on a
/// squared scale, so half of the rows sit in the lowest 22 deg, where the sky
/// changes fastest.
const LUT_WIDTH: usize = 128;
const LUT_HEIGHT: usize = 64;
/// Samples along each view ray, crowded toward the viewer.
const VIEW_STEPS: usize = 16;

/// Optical depth toward space, tabulated over height (squared scale) and the
/// angle above the ray that grazes the ground (squared again: the depth grows
/// steeply there). It replaces the light march at every view sample, which
/// keeps `Atmosphere::new` in the low milliseconds in release builds.
const DEPTH_HEIGHTS: usize = 64;
const DEPTH_ANGLES: usize = 64;
const DEPTH_STEPS: usize = 40;
/// Steps for the one-off exact queries (the sun's colour, `transmittance`).
const EXACT_STEPS: usize = 64;

/// Multiple scattering, fudged. Light scattered once in the still-lit upper
/// air scatters again into the Earth's shadow, so real skies keep glowing
/// well after sunset. We add an isotropic source at every view sample: this
/// gain x the sunlight that reaches MULTI_SCATTER_HEIGHT (sun held at the
/// horizon once it has set) x a twilight fade (see `twilight_fade`).
const MULTI_SCATTER_GAIN: f64 = 0.25;
const MULTI_SCATTER_HEIGHT: f64 = 30_000.0;

/// Below the horizon the horizon haze fades into the lit ground over this band.
const GROUND_BLEND_DEG: f32 = 3.0;
/// Linear limb-darkening coefficient of the sun in the visible.
const LIMB_DARKENING: f32 = 0.6;

pub struct Atmosphere {
    sun_dir: Vec3f,
    /// Unit horizontal direction of the sun as (x, z); (1, 0) when the sun is
    /// at the zenith, where any reference azimuth will do.
    sun_horizontal: Vec2f,
    medium: Medium,
    /// LUT_HEIGHT rows of LUT_WIDTH texels, row 0 at the horizon.
    lut: Vec<Vec3f>,
    /// Radiance of the lit ground (Lambertian, ground_color albedo).
    ground: Vec3f,
    ambient: Vec3f,
    /// The sky's irradiance on a level surface (the disc excluded).
    sky_irradiance: Vec3f,
    sun_irradiance: Vec3f,
    /// Radiance at the centre of the disc: sun irradiance / effective solid angle.
    disc_radiance: Vec3f,
    disc_radius: f32,
    disc_softness: f32,
    /// Chord length (2 sin(outer / 2)) beyond which the disc is zero.
    disc_chord: f32,
    disc_visible: bool,
}

impl Atmosphere {
    /// The sky for the sun at `sun_dir`. Both blocks are read as clamp() holds
    /// them (`clamped()`, D17), so their ranges, defaults and NaN rule are
    /// their `clamp_fields`' alone.
    pub fn new(sun_dir: Vec3f, atmo: &AtmosphereParams, disc: &SunDiscParams) -> Atmosphere {
        let (atmo, disc) = (&atmo.clamped(), &disc.clamped());
        let sun = sun_dir.normalize();
        let sun = if sun.is_finite() && sun.length() > 0.5 { sun } else { vec3f(0.0, 1.0, 0.0) };
        let horizontal = (sun.x * sun.x + sun.z * sun.z).sqrt();
        let sun_horizontal = if horizontal > 1.0e-6 {
            vec2f(sun.x / horizontal, sun.z / horizontal)
        } else {
            vec2f(1.0, 0.0)
        };
        let medium = Medium::new(atmo);
        let table = DepthTable::new(&medium);
        let sun_sin = sun.y.clamp(-1.0, 1.0);
        let multi = multi_scatter_source(&medium, sun_sin as f64, sun_sin.asin().to_degrees());
        let lut = build_sky_lut(&medium, &table, sun_sin as f64, multi);
        let (ambient, sky_irradiance) = hemisphere_integrals(&lut);
        let sun_irradiance = exact_transmittance(&medium, sun) * SUN_IRRADIANCE;
        let ground_color = vec3f(atmo.ground_color[0], atmo.ground_color[1], atmo.ground_color[2]);
        // A Lambertian ground under the sky dome and the sun: albedo x E / pi.
        let ground = ground_color
            * (sky_irradiance + sun_irradiance * sun.y.max(0.0))
            * (1.0 / std::f32::consts::PI);
        let disc_radius = (0.5 * disc.size_deg).to_radians();
        let disc_softness = disc.softness;
        let outer = disc_radius * (1.0 + 0.5 * disc_softness);
        // Dividing by the profile's own solid angle is what keeps the disc's
        // energy equal to the sun's irradiance at every size and softness.
        let solid_angle = disc_solid_angle(disc_radius, disc_softness);
        Atmosphere {
            sun_dir: sun,
            sun_horizontal,
            medium,
            lut,
            ground,
            ambient,
            sky_irradiance,
            sun_irradiance,
            disc_radiance: sun_irradiance * (1.0 / solid_angle as f32),
            disc_radius,
            disc_softness,
            disc_chord: 2.0 * (0.5 * outer).sin(),
            disc_visible: disc.visible,
        }
    }

    /// Sky radiance from `dir` without the disc: in-scattering plus the
    /// multiple-scattering term above the horizon, the lit ground below it.
    pub fn sky(&self, dir: Vec3f) -> Vec3f {
        let horizontal = (dir.x * dir.x + dir.z * dir.z).sqrt();
        // atan2 rather than asin(y): exact for non-unit input and at the zenith.
        let elevation = dir.y.atan2(horizontal);
        let phi = self.relative_azimuth(dir);
        if elevation >= 0.0 {
            return lut_lookup(&self.lut, phi, elevation);
        }
        let haze = lut_lookup(&self.lut, phi, 0.0);
        let t = smoothstep(0.0, GROUND_BLEND_DEG.to_radians(), -elevation);
        haze.mix(self.ground, t)
    }

    /// The sun disc: zero outside it, below the horizon, or when hidden.
    pub fn sun_disc(&self, dir: Vec3f) -> Vec3f {
        if !self.disc_visible || dir.y < 0.0 {
            return Vec3f::default();
        }
        let d = dir.normalize();
        // The chord gives the angle precisely even for a 0.27 deg radius,
        // where acos of a dot product runs out of f32 bits.
        let chord = (d - self.sun_dir).length();
        if chord >= self.disc_chord {
            return Vec3f::default();
        }
        let theta = 2.0 * (0.5 * chord).min(1.0).asin();
        self.disc_radiance * disc_profile(theta, self.disc_radius, self.disc_softness)
    }

    /// Transmittance from the viewer to space along `dir`; zero when the
    /// ground is in the way.
    pub fn transmittance(&self, dir: Vec3f) -> Vec3f {
        exact_transmittance(&self.medium, dir)
    }

    /// SUN_IRRADIANCE x transmittance along the sun: what a surface facing
    /// the sun receives at the ground.
    pub fn sun_irradiance(&self) -> Vec3f {
        self.sun_irradiance
    }

    /// Radiance at the centre of the disc (limb darkening makes it the peak).
    pub fn sun_radiance(&self) -> Vec3f {
        self.disc_radiance * disc_profile(0.0, self.disc_radius, self.disc_softness)
    }

    /// Mean radiance over the nominal disc cone (radius size_deg / 2):
    /// radiance x 2 pi (1 - sun_cos_radius()) is exactly `sun_irradiance()`.
    /// This is what the engine's key light wants. The cone is the one the
    /// f32 `sun_cos_radius()` names, cancellation and all, because that is
    /// the cone every consumer of the key computes (`EnvSun::cone_radiance`):
    /// a cancellation-free cone here lit a 0.1 degree sun 6 % too dark.
    pub fn sun_cone_radiance(&self) -> Vec3f {
        EnvSun::cone_radiance(self.sun_irradiance, self.sun_cos_radius())
    }

    pub fn sun_cos_radius(&self) -> f32 {
        self.disc_radius.cos()
    }

    /// Angular radius of the disc's outer edge (the radius plus half its soft edge), in radians.
    pub fn sun_outer_radius(&self) -> f32 {
        self.disc_radius * (1.0 + 0.5 * self.disc_softness)
    }

    /// Mean radiance of the upper hemisphere (the disc excluded).
    pub fn ambient(&self) -> Vec3f {
        self.ambient
    }

    /// The irradiance the sky (the disc excluded) puts on a level surface:
    /// the ground's light from above, the sun's apart.
    pub fn sky_irradiance(&self) -> Vec3f {
        self.sky_irradiance
    }

    /// Unit direction toward the sun, in the map's own frame (before the
    /// map rotation).
    pub fn sun_dir(&self) -> Vec3f {
        self.sun_dir
    }

    /// Angle between the view's and the sun's horizontal directions, 0..pi.
    fn relative_azimuth(&self, dir: Vec3f) -> f32 {
        let along = dir.x * self.sun_horizontal.x + dir.z * self.sun_horizontal.y;
        let across = dir.x * self.sun_horizontal.y - dir.z * self.sun_horizontal.x;
        // |across| folds the mirror half onto the stored one; atan2(0, 0) = 0
        // at the zenith, where every column holds the same value anyway.
        across.abs().atan2(along)
    }
}

/// Sun direction (map frame) for the sun parameters: the NOAA position in
/// time mode through `crate::sky::noaa_solar_position`, the manual angles
/// otherwise. NOAA's azimuth is clockwise from north, `dir_from_az_el`'s
/// sense, so no conversion sits between them. The sun is read as clamp()
/// holds it (`SunParams::clamped`, D17): its month is 1..=12 and its day
/// within that month, so both fit the date's bytes.
pub fn sun_direction(sun: &SunParams) -> Vec3f {
    let sun = &sun.clamped();
    match sun.mode() {
        SunMode::Time => {
            let date = SkyDate { year: sun.year, month: sun.month as u8, day: sun.day as u8 };
            let (elevation, azimuth) =
                noaa_solar_position(date, sun.hour, sun.tz_offset, sun.latitude, sun.longitude);
            dir_from_az_el(azimuth, elevation)
        }
        SunMode::Manual => dir_from_az_el(sun.azimuth_deg, sun.elevation_deg),
    }
}

/// Scattering and absorption coefficients for one parameter set, per metre
/// at sea level.
#[derive(Clone, Copy)]
struct Medium {
    rayleigh: [f64; 3],
    mie_scattering: f64,
    mie_extinction: f64,
    ozone: [f64; 3],
}

impl Medium {
    /// `p` is held by clamp() (`Atmosphere::new`).
    fn new(p: &AtmosphereParams) -> Medium {
        let (air, haze, ozone) = (p.air as f64, p.haze as f64, p.ozone as f64);
        Medium {
            rayleigh: RAYLEIGH_SCATTERING.map(|b| b * air),
            mie_scattering: MIE_SCATTERING * haze,
            mie_extinction: MIE_EXTINCTION * haze,
            ozone: OZONE_ABSORPTION.map(|b| b * ozone),
        }
    }

    /// Per-channel extinction for (air, haze, ozone) densities: a coefficient
    /// per metre for point densities, an optical depth for integrated ones.
    fn extinction(&self, d: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|c| self.rayleigh[c] * d[0] + self.mie_extinction * d[1] + self.ozone[c] * d[2])
    }
}

/// Relative densities (air, haze, ozone) at `height` metres above the ground.
fn densities(height: f64) -> [f64; 3] {
    let h = height.max(0.0);
    [
        (-h / RAYLEIGH_SCALE_HEIGHT).exp(),
        (-h / MIE_SCALE_HEIGHT).exp(),
        (1.0 - (h - OZONE_PEAK_HEIGHT).abs() / OZONE_HALF_WIDTH).max(0.0),
    ]
}

/// True when a ray from radius `r` with cosine `mu` against the local
/// vertical runs into the planet.
fn hits_ground(r: f64, mu: f64) -> bool {
    mu < 0.0 && r * r * (1.0 - mu * mu) < PLANET_RADIUS * PLANET_RADIUS
}

/// Distance from radius `r` (inside the shell) along `mu` to the top.
fn distance_to_top(r: f64, mu: f64) -> f64 {
    let disc = r * r * (mu * mu - 1.0) + TOP_RADIUS * TOP_RADIUS;
    (-r * mu + disc.max(0.0).sqrt()).max(0.0)
}

/// Radius after travelling `t` from radius `r` along `mu`.
fn radius_at(r: f64, mu: f64, t: f64) -> f64 {
    (r * r + 2.0 * r * mu * t + t * t).sqrt()
}

/// Cosine of the ray from radius `r` that just grazes the ground.
fn grazing_mu(r: f64) -> f64 {
    let s = (PLANET_RADIUS / r).min(1.0);
    -(1.0 - s * s).max(0.0).sqrt()
}

/// Midpoint and length of segment `i` of `n` over `length`, spaced
/// quadratically: samples crowd the start of the ray, where the air is
/// densest for every ray that starts low.
fn segment(i: usize, n: usize, length: f64) -> (f64, f64) {
    let u0 = i as f64 / n as f64;
    let u1 = (i + 1) as f64 / n as f64;
    let t0 = length * u0 * u0;
    let t1 = length * u1 * u1;
    (0.5 * (t0 + t1), t1 - t0)
}

/// Integrated (air, haze, ozone) densities in metres from radius `r` along
/// `mu` to space, or None when the ground is in the way.
fn column_depth(r: f64, mu: f64, steps: usize) -> Option<[f64; 3]> {
    if hits_ground(r, mu) {
        return None;
    }
    Some(integrate_column(r, mu, steps))
}

/// The same integral without the ground test. The depth table needs it for
/// its grazing column: there `hits_ground` sits exactly on its boundary and
/// rounding could turn a ray that skims the ground into "blocked", which
/// would store a depth of 0 (full sunlight) right at the terminator.
fn integrate_column(r: f64, mu: f64, steps: usize) -> [f64; 3] {
    let length = distance_to_top(r, mu);
    let mut depth = [0.0f64; 3];
    for i in 0..steps {
        let (t, ds) = segment(i, steps, length);
        // `densities` clamps the height at 0, so a skimming ray that rounds a
        // hair below the surface reads sea-level air.
        let d = densities(radius_at(r, mu, t) - PLANET_RADIUS);
        for (acc, v) in depth.iter_mut().zip(d) {
            *acc += v * ds;
        }
    }
    depth
}

fn exact_transmittance(medium: &Medium, dir: Vec3f) -> Vec3f {
    let length = dir.length();
    if length.is_nan() || length <= 0.0 {
        return Vec3f::default();
    }
    let mu = (dir.y / length).clamp(-1.0, 1.0) as f64;
    match column_depth(PLANET_RADIUS + VIEWER_HEIGHT, mu, EXACT_STEPS) {
        Some(column) => {
            let ext = medium.extinction(column);
            vec3f((-ext[0]).exp() as f32, (-ext[1]).exp() as f32, (-ext[2]).exp() as f32)
        }
        None => Vec3f::default(),
    }
}

/// Extinction optical depth toward space over (height, angle above grazing).
struct DepthTable {
    depth: Vec<[f32; 3]>,
}

impl DepthTable {
    fn new(medium: &Medium) -> DepthTable {
        let mut depth = Vec::with_capacity(DEPTH_HEIGHTS * DEPTH_ANGLES);
        for j in 0..DEPTH_HEIGHTS {
            let y = j as f64 / (DEPTH_HEIGHTS - 1) as f64;
            let r = PLANET_RADIUS + (TOP_RADIUS - PLANET_RADIUS) * y * y;
            let mu_graze = grazing_mu(r);
            for i in 0..DEPTH_ANGLES {
                let x = i as f64 / (DEPTH_ANGLES - 1) as f64;
                let mu = mu_graze + (1.0 - mu_graze) * x * x;
                // Every column is at or above the grazing ray by construction.
                let ext = medium.extinction(integrate_column(r, mu, DEPTH_STEPS));
                depth.push([ext[0] as f32, ext[1] as f32, ext[2] as f32]);
            }
        }
        DepthTable { depth }
    }

    /// Transmittance toward space from radius `r` along `mu`. The planet's
    /// shadow is decided exactly, not blurred by the table.
    fn transmittance(&self, r: f64, mu: f64) -> [f64; 3] {
        if hits_ground(r, mu) {
            return [0.0; 3];
        }
        let y = ((r - PLANET_RADIUS) / (TOP_RADIUS - PLANET_RADIUS)).clamp(0.0, 1.0).sqrt();
        let mu_graze = grazing_mu(r);
        let x = ((mu - mu_graze) / (1.0 - mu_graze)).clamp(0.0, 1.0).sqrt();
        let fy = y * (DEPTH_HEIGHTS - 1) as f64;
        let fx = x * (DEPTH_ANGLES - 1) as f64;
        let y0 = (fy as usize).min(DEPTH_HEIGHTS - 2);
        let x0 = (fx as usize).min(DEPTH_ANGLES - 2);
        let ty = fy - y0 as f64;
        let tx = fx - x0 as f64;
        let row0 = y0 * DEPTH_ANGLES;
        let row1 = row0 + DEPTH_ANGLES;
        let (a, b) = (self.depth[row0 + x0], self.depth[row0 + x0 + 1]);
        let (c, d) = (self.depth[row1 + x0], self.depth[row1 + x0 + 1]);
        std::array::from_fn(|k| {
            let near = a[k] as f64 + (b[k] as f64 - a[k] as f64) * tx;
            let far = c[k] as f64 + (d[k] as f64 - c[k] as f64) * tx;
            (-(near + (far - near) * ty)).exp()
        })
    }
}

fn rayleigh_phase(nu: f64) -> f64 {
    3.0 / (16.0 * std::f64::consts::PI) * (1.0 + nu * nu)
}

/// Cornette-Shanks phase (as in glsl-atmosphere): Henyey-Greenstein with a
/// (1 + nu^2) factor that fits haze better; it integrates to 1 over the sphere.
fn mie_phase(nu: f64) -> f64 {
    let g = MIE_G;
    let gg = g * g;
    3.0 / (8.0 * std::f64::consts::PI) * ((1.0 - gg) * (1.0 + nu * nu))
        / ((2.0 + gg) * (1.0 + gg - 2.0 * g * nu).powf(1.5))
}

/// 1 while the sun is up, then an exponential fall (a factor e every 2.5 deg)
/// that the smoothstep takes to exactly 0 by -20 deg: a bright civil
/// twilight, a faint nautical one, and black by astronomical night (-18 deg).
fn twilight_fade(sun_elevation_deg: f32) -> f32 {
    if sun_elevation_deg >= 0.0 {
        return 1.0;
    }
    (sun_elevation_deg / 2.5).exp() * smoothstep(-20.0, -14.0, sun_elevation_deg)
}

/// Isotropic source radiance of the multiple-scattering term, per unit of
/// scattering along the view ray.
fn multi_scatter_source(medium: &Medium, sun_sin: f64, sun_elevation_deg: f32) -> [f64; 3] {
    // The higher orders are fed by sunlight that has crossed the upper air:
    // take its colour at MULTI_SCATTER_HEIGHT, with the sun held at the
    // horizon once it has set (a level ray from up there clears the ground).
    let column = column_depth(PLANET_RADIUS + MULTI_SCATTER_HEIGHT, sun_sin.max(0.0), EXACT_STEPS)
        .unwrap_or([0.0; 3]);
    let ext = medium.extinction(column);
    let fade = twilight_fade(sun_elevation_deg) as f64;
    std::array::from_fn(|c| {
        MULTI_SCATTER_GAIN * SUN_IRRADIANCE as f64 * (-ext[c]).exp() * fade / (4.0 * std::f64::consts::PI)
    })
}

/// Radiance reaching the viewer along a ray at elevation sine `view_sin`,
/// with the sun at elevation sine `sun_sin` and view-sun cosine `nu`:
/// single scattering plus the multiple-scattering term.
fn in_scatter(medium: &Medium, table: &DepthTable, view_sin: f64, sun_sin: f64, nu: f64, multi: [f64; 3]) -> [f64; 3] {
    let r0 = PLANET_RADIUS + VIEWER_HEIGHT;
    let length = distance_to_top(r0, view_sin);
    let phase_r = rayleigh_phase(nu);
    let phase_m = mie_phase(nu);
    // Extinction optical depth from the viewer to the start of the current segment.
    let mut depth = [0.0f64; 3];
    let mut single = [0.0f64; 3];
    let mut multiple = [0.0f64; 3];
    for i in 0..VIEW_STEPS {
        let (t, ds) = segment(i, VIEW_STEPS, length);
        let r = radius_at(r0, view_sin, t);
        let dens = densities(r - PLANET_RADIUS);
        let ext = medium.extinction(dens);
        // The sun's direction is fixed but the local vertical turns along the
        // ray: mu_sun = dot(sample position, sun) / |sample position|.
        let mu_sun = (r0 * sun_sin + t * nu) / r;
        let sun_t = table.transmittance(r, mu_sun);
        for c in 0..3 {
            let view_t = (-(depth[c] + 0.5 * ext[c] * ds)).exp();
            depth[c] += ext[c] * ds;
            let scatter_r = medium.rayleigh[c] * dens[0];
            let scatter_m = medium.mie_scattering * dens[1];
            single[c] += view_t * sun_t[c] * (phase_r * scatter_r + phase_m * scatter_m) * ds;
            multiple[c] += view_t * (scatter_r + scatter_m) * ds;
        }
    }
    std::array::from_fn(|c| SUN_IRRADIANCE as f64 * single[c] + multi[c] * multiple[c])
}

fn build_sky_lut(medium: &Medium, table: &DepthTable, sun_sin: f64, multi: [f64; 3]) -> Vec<Vec3f> {
    let sun_cos = (1.0 - sun_sin * sun_sin).max(0.0).sqrt();
    let mut lut = Vec::with_capacity(LUT_WIDTH * LUT_HEIGHT);
    for j in 0..LUT_HEIGHT {
        let v = j as f64 / (LUT_HEIGHT - 1) as f64;
        let elevation = std::f64::consts::FRAC_PI_2 * v * v;
        let (view_sin, view_cos) = elevation.sin_cos();
        for i in 0..LUT_WIDTH {
            let phi = std::f64::consts::PI * i as f64 / (LUT_WIDTH - 1) as f64;
            // The sun sits at azimuth 0 of the table's frame.
            let nu = view_cos * sun_cos * phi.cos() + view_sin * sun_sin;
            let c = in_scatter(medium, table, view_sin, sun_sin, nu, multi);
            lut.push(vec3f(c[0] as f32, c[1] as f32, c[2] as f32));
        }
    }
    lut
}

fn lut_lookup(lut: &[Vec3f], phi: f32, elevation: f32) -> Vec3f {
    let fx = (phi / std::f32::consts::PI).clamp(0.0, 1.0) * (LUT_WIDTH - 1) as f32;
    let fy = (elevation / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0).sqrt() * (LUT_HEIGHT - 1) as f32;
    let x0 = (fx as usize).min(LUT_WIDTH - 2);
    let y0 = (fy as usize).min(LUT_HEIGHT - 2);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let row0 = y0 * LUT_WIDTH;
    let row1 = row0 + LUT_WIDTH;
    let near = lut[row0 + x0].mix(lut[row0 + x0 + 1], tx);
    let far = lut[row1 + x0].mix(lut[row1 + x0 + 1], tx);
    near.mix(far, ty)
}

/// Mean radiance of the upper hemisphere, and the irradiance it puts on a
/// level surface, both from the table (the disc is not part of the sky).
fn hemisphere_integrals(lut: &[Vec3f]) -> (Vec3f, Vec3f) {
    const ROWS: usize = 32;
    const COLUMNS: usize = 32;
    let d_el = std::f32::consts::FRAC_PI_2 / ROWS as f32;
    let d_phi = std::f32::consts::PI / COLUMNS as f32;
    let mut radiance_sum = Vec3f::default();
    let mut irradiance = Vec3f::default();
    for j in 0..ROWS {
        let el = (j as f32 + 0.5) * d_el;
        // Solid angle of the cell, doubled for the mirrored half of the sky.
        let d_omega = 2.0 * el.cos() * d_el * d_phi;
        for i in 0..COLUMNS {
            let c = lut_lookup(lut, (i as f32 + 0.5) * d_phi, el);
            radiance_sum += c * d_omega;
            irradiance += c * (d_omega * el.sin());
        }
    }
    (radiance_sum * (1.0 / (2.0 * std::f32::consts::PI)), irradiance)
}

/// Relative disc brightness at angle `theta` from its centre: a smoothstep
/// edge `softness` x radius wide, centred on the radius, times linear limb
/// darkening.
fn disc_profile(theta: f32, radius: f32, softness: f32) -> f32 {
    let inner = radius * (1.0 - 0.5 * softness);
    let outer = radius * (1.0 + 0.5 * softness);
    if theta >= outer {
        return 0.0;
    }
    let edge = if outer - inner > 1.0e-9 { 1.0 - smoothstep(inner, outer, theta) } else { 1.0 };
    let x = theta / outer;
    let limb = 1.0 - LIMB_DARKENING * (1.0 - (1.0 - x * x).max(0.0).sqrt());
    edge * limb
}

/// Integral of `disc_profile` over the sphere, in steradians.
fn disc_solid_angle(radius: f32, softness: f32) -> f64 {
    const STEPS: usize = 2048;
    let outer = (radius * (1.0 + 0.5 * softness)) as f64;
    let dt = outer / STEPS as f64;
    (0..STEPS)
        .map(|i| {
            let theta = (i as f64 + 0.5) * dt;
            disc_profile(theta as f32, radius, softness) as f64 * 2.0 * std::f64::consts::PI * theta.sin() * dt
        })
        .sum()
}

/// Local copy (as in sky.rs), so no glob can make the name ambiguous.
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::dir_from_az_el;
    use crate::sky::{luminance, noaa_solar_position, SkyDate};

    fn atmo_at(elevation_deg: f32, azimuth_deg: f32) -> Atmosphere {
        Atmosphere::new(
            dir_from_az_el(azimuth_deg, elevation_deg),
            &AtmosphereParams::default(),
            &SunDiscParams::default(),
        )
    }

    fn blue_over_red(c: Vec3f) -> f32 {
        c.z / c.x.max(1.0e-12)
    }

    fn red_over_blue(c: Vec3f) -> f32 {
        c.x / c.z.max(1.0e-12)
    }

    #[test]
    fn noon_zenith_is_bluer_than_the_horizon() {
        let atmo = atmo_at(60.0, 180.0);
        let zenith = atmo.sky(vec3f(0.0, 1.0, 0.0));
        let horizon = atmo.sky(dir_from_az_el(90.0, 1.0));
        assert!(
            blue_over_red(zenith) > 1.3 * blue_over_red(horizon),
            "zenith {zenith:?} should be bluer than the horizon {horizon:?}"
        );
        // SUN_IRRADIANCE puts a clear high-sun zenith at a few tenths.
        let lum = luminance(zenith);
        assert!(lum > 0.03 && lum < 3.0, "zenith luminance {lum}");
    }

    #[test]
    fn at_sunset_the_horizon_toward_the_sun_is_redder() {
        let atmo = atmo_at(2.0, 270.0);
        let toward = atmo.sky(dir_from_az_el(280.0, 3.0));
        let away = atmo.sky(dir_from_az_el(90.0, 3.0));
        assert!(
            red_over_blue(toward) > 1.5 * red_over_blue(away),
            "toward the sun {toward:?} vs away {away:?}"
        );
    }

    #[test]
    fn the_sky_is_dark_at_astronomical_twilight() {
        let dark = atmo_at(-18.0, 270.0);
        let mut brightest = 0.0f32;
        for el in [0.0f32, 2.0, 5.0, 10.0, 20.0, 45.0, 90.0, -10.0] {
            for az in (0..360).step_by(30) {
                brightest = brightest.max(luminance(dark.sky(dir_from_az_el(az as f32, el))));
            }
        }
        assert!(brightest < 1.0e-3, "brightest direction at -18 deg: {brightest}");
        // Civil twilight still glows: the multiple-scattering term keeps the
        // zenith from going black as soon as the ground is in shadow.
        let civil = atmo_at(-6.0, 270.0);
        let zenith = luminance(civil.sky(vec3f(0.0, 1.0, 0.0)));
        assert!(zenith > 1.0e-4, "civil twilight zenith {zenith}");
        assert!(zenith > 10.0 * brightest, "-6 deg {zenith} vs -18 deg {brightest}");
    }

    /// Integrates `sun_disc` over a square on the tangent plane at the sun,
    /// with the gnomonic Jacobian dOmega = dx dy / (1 + x^2 + y^2)^(3/2).
    fn disc_energy(atmo: &Atmosphere, half_angle: f32) -> Vec3f {
        let c = atmo.sun_dir();
        let e1 = Vec3f::cross(c, vec3f(0.0, 1.0, 0.0)).normalize();
        let e2 = Vec3f::cross(e1, c).normalize();
        let n = 240;
        let a = half_angle.tan();
        let step = 2.0 * a / n as f32;
        let mut sum = Vec3f::default();
        for j in 0..n {
            for i in 0..n {
                let x = -a + (i as f32 + 0.5) * step;
                let y = -a + (j as f32 + 0.5) * step;
                let d = (c + e1 * x + e2 * y).normalize();
                let jacobian = 1.0 / (1.0 + x * x + y * y).powf(1.5);
                sum += atmo.sun_disc(d) * (jacobian * step * step);
            }
        }
        sum
    }

    #[test]
    fn the_disc_keeps_its_energy_when_it_grows() {
        let sun = dir_from_az_el(200.0, 45.0);
        let make = |size_deg: f32| {
            Atmosphere::new(
                sun,
                &AtmosphereParams::default(),
                &SunDiscParams { size_deg, ..SunDiscParams::default() },
            )
        };
        let small = make(0.53);
        let large = make(5.0);
        let want = luminance(small.sun_irradiance());
        // At the default softness 0.2 the soft edge reaches 1.1 x the radius;
        // integrate out to 1.3 x.
        let e_small = luminance(disc_energy(&small, (0.53f32 * 0.5 * 1.3).to_radians()));
        let e_large = luminance(disc_energy(&large, (5.0f32 * 0.5 * 1.3).to_radians()));
        assert!((e_small / want - 1.0).abs() < 0.03, "0.53 deg disc: {e_small} vs {want}");
        assert!((e_large / want - 1.0).abs() < 0.03, "5 deg disc: {e_large} vs {want}");
        assert!((e_large / e_small - 1.0).abs() < 0.03, "{e_small} vs {e_large}");
        // A larger disc is dimmer per steradian, by about the area ratio (89).
        let ratio = luminance(small.sun_radiance()) / luminance(large.sun_radiance());
        assert!(ratio > 50.0 && ratio < 150.0, "radiance ratio {ratio}");
    }

    #[test]
    fn a_hidden_disc_still_lights_the_scene() {
        let sun = dir_from_az_el(100.0, 30.0);
        let hidden = Atmosphere::new(
            sun,
            &AtmosphereParams::default(),
            &SunDiscParams { visible: false, ..SunDiscParams::default() },
        );
        assert_eq!(hidden.sun_disc(sun), Vec3f::default());
        assert!(luminance(hidden.sun_radiance()) > 0.0);
        let shown = atmo_at(30.0, 100.0);
        let disc = luminance(shown.sun_disc(shown.sun_dir()));
        let sky = luminance(shown.sky(dir_from_az_el(100.0, 60.0)));
        assert!(disc > 1000.0 * sky, "disc {disc} vs sky {sky}");
    }

    #[test]
    fn sun_direction_matches_the_noaa_conversion() {
        let mut sun = SunParams {
            mode: "time".to_string(),
            year: 2024,
            month: 6,
            day: 21,
            hour: 13.7,
            tz_offset: 2.0,
            latitude: 52.37,
            longitude: 4.9,
            ..Default::default()
        };
        let got = sun_direction(&sun);
        // libs/render/tests/one_sun.rs's NOAA-to-engine conversion, reproduced.
        let (el, az) = noaa_solar_position(SkyDate { year: 2024, month: 6, day: 21 }, 13.7, 2.0, 52.37, 4.9);
        let (el, az) = (el.to_radians(), az.to_radians());
        let want = vec3f(el.cos() * az.sin(), el.sin(), -el.cos() * az.cos()).normalize();
        assert!((got - want).length() < 1.0e-5, "{got:?} vs {want:?}");
        // Solar noon in Amsterdam at midsummer (13:42 CEST): 61 deg up, due south (+z).
        let elevation = got.y.asin().to_degrees();
        assert!((elevation - 61.07).abs() < 0.5, "elevation {elevation}");
        assert!(got.z > 0.4 && got.x.abs() < 0.05, "{got:?}");
        // Manual mode is dir_from_az_el.
        sun.mode = "manual".to_string();
        sun.elevation_deg = 12.0;
        sun.azimuth_deg = 250.0;
        assert!((sun_direction(&sun) - dir_from_az_el(250.0, 12.0)).length() < 1.0e-6);
    }

    #[test]
    fn the_sky_is_continuous_across_the_azimuth_wrap() {
        // Sun due north: its vertical plane is also where azimuth wraps 360 -> 0.
        let atmo = atmo_at(20.0, 0.0);
        for el in [1.0f32, 10.0, 40.0] {
            let mut prev = luminance(atmo.sky(dir_from_az_el(-0.5, el)));
            for step in 0..=720 {
                let az = step as f32 * 0.5;
                let now = luminance(atmo.sky(dir_from_az_el(az, el)));
                let jump = (now - prev).abs() / prev.max(1.0e-6);
                assert!(jump < 0.08, "el {el}: jump {jump} at az {az}");
                prev = now;
            }
            // The table stores one half of a mirror-symmetric sky: both halves agree.
            for az in [10.0f32, 60.0, 135.0, 179.5] {
                let a = atmo.sky(dir_from_az_el(az, el));
                let b = atmo.sky(dir_from_az_el(360.0 - az, el));
                assert!((a - b).length() <= 1.0e-4 * a.length(), "el {el} az +-{az}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn the_ground_takes_its_colour_and_meets_the_horizon() {
        let sun = dir_from_az_el(150.0, 35.0);
        let grey = |g: f32| {
            Atmosphere::new(
                sun,
                &AtmosphereParams { ground_color: [g, g, g], ..AtmosphereParams::default() },
                &SunDiscParams::default(),
            )
        };
        let dim = grey(0.1);
        let bright = grey(0.4);
        let down = dir_from_az_el(0.0, -45.0);
        let ratio = luminance(bright.sky(down)) / luminance(dim.sky(down));
        assert!((ratio - 4.0).abs() < 1.0e-3, "ground is linear in its colour: {ratio}");
        // No seam at the horizon: just above and just below agree.
        for az in [0.0f32, 150.0, 300.0] {
            let above = luminance(dim.sky(dir_from_az_el(az, 0.01)));
            let below = luminance(dim.sky(dir_from_az_el(az, -0.01)));
            assert!((above - below).abs() < 0.02 * above, "az {az}: {above} vs {below}");
        }
        // Looking down through the planet transmits nothing; the zenith beats a low ray.
        assert_eq!(dim.transmittance(vec3f(0.0, -1.0, 0.0)), Vec3f::default());
        let up = luminance(dim.transmittance(vec3f(0.0, 1.0, 0.0)));
        let low = luminance(dim.transmittance(dir_from_az_el(0.0, 2.0)));
        assert!(up > low && low > 0.0, "zenith {up} vs 2 deg {low}");
    }

    #[test]
    fn the_sky_stays_finite_for_any_sun() {
        for el in [-90.0f32, -30.0, -6.0, 0.0, 0.3, 10.0, 89.99, 90.0] {
            let atmo = atmo_at(el, 33.0);
            for sky_el in [-60.0f32, -1.0, 0.0, 1.0, 30.0, 90.0] {
                for az in [0.0f32, 33.0, 120.0, 213.0] {
                    let d = dir_from_az_el(az, sky_el);
                    let c = atmo.sky(d) + atmo.sun_disc(d);
                    assert!(c.is_finite() && c.x >= 0.0 && c.y >= 0.0 && c.z >= 0.0, "sun {el}, view ({az}, {sky_el}): {c:?}");
                }
            }
            assert!(atmo.ambient().is_finite() && atmo.sun_radiance().is_finite());
        }
    }

    /// Pins the numbers the module header quotes against the engine's
    /// EnvPreset and HDR-lane scales, relative to SUN_IRRADIANCE where a
    /// retune of the constant should not break them.
    #[test]
    fn the_documented_scale_holds() {
        let noon = atmo_at(90.0, 0.0);
        // Zenith transmittance in luminance: Rayleigh 0.106, Mie 0.005 and
        // ozone 0.028 of optical depth in the green.
        let t = luminance(noon.sun_irradiance()) / SUN_IRRADIANCE;
        assert!(t > 0.84 && t < 0.92, "zenith transmittance {t}");
        // A white wall facing the high sun: E / pi, about 5.6 at 20.0.
        let wall = luminance(noon.sun_irradiance()) / std::f32::consts::PI;
        let want = 0.878 * SUN_IRRADIANCE / std::f32::consts::PI;
        assert!((wall / want - 1.0).abs() < 0.05, "noon white wall {wall} vs {want}");
        // A clear high-sun zenith: a few tenths.
        let zenith = luminance(noon.sky(vec3f(0.0, 1.0, 0.0))) / SUN_IRRADIANCE;
        assert!(zenith > 0.005 && zenith < 0.05, "zenith / SUN_IRRADIANCE {zenith}");
        // The 0.53 deg disc: about a thousand times EnvPreset::Sunset's 300.
        let disc = luminance(noon.sun_radiance()) / SUN_IRRADIANCE;
        assert!(disc > 5.0e3 && disc < 5.0e4, "disc centre / SUN_IRRADIANCE {disc}");
    }

    /// Release-only timing: `cargo test -p makepad-render --release --lib
    /// hdri::atmosphere::tests::building_the_atmosphere_is_fast -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn building_the_atmosphere_is_fast() {
        let start = std::time::Instant::now();
        let runs = 10;
        for i in 0..runs {
            let _ = atmo_at(5.0 + i as f32, 180.0);
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!("Atmosphere::new: {ms:.2} ms");
        assert!(ms < 30.0, "Atmosphere::new took {ms:.2} ms");
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;
    use crate::hdri::{rotate_y, vec, Env, HdriParams};
    use crate::sky::luminance;

    fn sky_params(elevation_deg: f32, azimuth_deg: f32) -> HdriParams {
        let mut p = HdriParams { mode: "sky".to_string(), ..Default::default() };
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = elevation_deg;
        p.sky.sun.azimuth_deg = azimuth_deg;
        p
    }

    #[test]
    fn sky_mode_is_the_atmosphere_and_intensity_scales_it() {
        let p = sky_params(35.0, 120.0);
        let env = Env::new(&p);
        let atmo = Atmosphere::new(dir_from_az_el(120.0, 35.0), &p.sky.atmosphere, &p.sky.sun_disc);
        for &(az, el) in &[(0.0f32, 5.0f32), (120.0, 35.0), (300.0, 70.0), (60.0, -30.0)] {
            let d = dir_from_az_el(az, el);
            let want = atmo.sky(d) + atmo.sun_disc(d);
            let got = env.radiance(d);
            assert!(
                (got - want).length() <= 1.0e-4 * want.length().max(1.0e-3),
                "({az}, {el}): {got:?} vs {want:?}"
            );
        }
        let mut brighter = p.clone();
        brighter.intensity_ev = 1.0;
        let d = dir_from_az_el(0.0, 30.0);
        let ratio = Env::new(&brighter).radiance(d).y / env.radiance(d).y;
        assert!((ratio - 2.0).abs() < 1.0e-4, "one EV up doubles the map: {ratio}");
    }

    #[test]
    fn the_sun_is_the_key_by_day_and_gone_at_night() {
        let sun_dir = dir_from_az_el(200.0, 30.0);
        let day = Env::new(&sky_params(30.0, 200.0));
        let key = day.sun().expect("the sun is up, so it is the key");
        assert!((key.dir - sun_dir).length() < 1.0e-5, "{:?}", key.dir);
        // Radiance over the cone is the sun's irradiance at the ground (1 %:
        // cos_radius is an f32 this close to 1).
        let atmo = Atmosphere::new(sun_dir, &AtmosphereParams::default(), &SunDiscParams::default());
        let cone = 2.0 * std::f32::consts::PI * (1.0 - key.cos_radius);
        let ratio = luminance(key.radiance * cone) / luminance(atmo.sun_irradiance());
        assert!((ratio - 1.0).abs() < 0.01, "cone irradiance ratio {ratio}");
        assert!((day.sun_dir().expect("sky mode") - sun_dir).length() < 1.0e-5);

        let night = Env::new(&sky_params(-5.0, 200.0));
        assert!(night.sun().is_none(), "no key once the sun has set");
        assert!(night.sun_dir().expect("sky mode").y < 0.0);

        let studio = HdriParams { mode: "studio".to_string(), ..Default::default() };
        let studio = Env::new(&studio);
        assert!(studio.sun_dir().is_none());
        assert!(studio.sun().is_none(), "no lights, so no key in studio mode");
    }

    /// The sun's key carries its disc's reach: a surface facing a disc under a
    /// degree wide gets all of its emission, and the covering cone is the one
    /// the drawn disc ends at, soft limb included (the cone `radiance` is
    /// averaged over is the nominal disc inside it).
    #[test]
    fn the_suns_key_is_covered_by_its_outer_limb() {
        let (az, el) = (200.0, 30.0);
        let sun_dir = dir_from_az_el(az, el);
        let p = sky_params(el, az);
        let key = Env::new(&p).sun().expect("the sun is up, so it is the key");
        assert!(key.validate().is_ok(), "{key:?}");
        assert_eq!(key.facing, 1.0);
        let atmo = Atmosphere::new(sun_dir, &p.sky.atmosphere, &p.sky.sun_disc);
        assert!((key.cos_cover - atmo.sun_outer_radius().cos()).abs() < 1.0e-7, "{} vs {}", key.cos_cover, atmo.sun_outer_radius().cos());
        assert!(key.cos_cover < key.cos_radius, "a soft limb reaches past the nominal disc");
        // And that really is where the disc ends: lit inside the cone, black outside it.
        let outer_deg = atmo.sun_outer_radius().to_degrees();
        let inside = dir_from_az_el(az, el + 0.9 * outer_deg);
        let outside = dir_from_az_el(az, el + 1.1 * outer_deg);
        assert!(inside.dot(key.dir) > key.cos_cover && luminance(atmo.sun_disc(inside)) > 0.0);
        assert!(outside.dot(key.dir) < key.cos_cover && luminance(atmo.sun_disc(outside)) == 0.0);
        // A hard-edged disc is its own covering cone.
        let mut hard = p.clone();
        hard.sky.sun_disc.softness = 0.0;
        let key = Env::new(&hard).sun().expect("the sun is up");
        assert!((key.cos_cover - key.cos_radius).abs() < 1.0e-7, "{} vs {}", key.cos_cover, key.cos_radius);
        assert!(key.validate().is_ok(), "{key:?}");
    }

    #[test]
    fn rotation_turns_the_sun_and_the_sky_together() {
        let p = sky_params(25.0, 90.0);
        let mut turned = p.clone();
        turned.rotation_deg = 40.0;
        let a = Env::new(&p);
        let b = Env::new(&turned);
        let want = rotate_y(a.sun_dir().unwrap(), 40.0);
        assert!((b.sun_dir().unwrap() - want).length() < 1.0e-5);
        assert!((b.sun().unwrap().dir - want).length() < 1.0e-5);
        // The cones and the facing share are about the key's own centre: the yaw leaves them.
        let (ka, kb) = (a.sun().unwrap(), b.sun().unwrap());
        assert_eq!((kb.cos_radius, kb.facing, kb.cos_cover), (ka.cos_radius, ka.facing, ka.cos_cover));
        for &(az, el) in &[(10.0f32, 20.0f32), (200.0, 60.0), (300.0, 5.0)] {
            let d = dir_from_az_el(az, el);
            let ca = a.radiance(d);
            let cb = b.radiance(rotate_y(d, 40.0));
            assert!((ca - cb).length() <= 1.0e-4 * ca.length(), "({az}, {el}): {ca:?} vs {cb:?}");
        }
        // ibl's sign, through the bake: +90 takes a sun at -Z (north) to -X,
        // exactly as EnvMap::procedural turns its presets (ibl.rs's own test).
        // A 10 deg disc makes sure the 64 x 32 texel grid lands on it.
        let mut north = sky_params(30.0, 0.0);
        north.sky.sun_disc.size_deg = 10.0;
        let straight = vec(Env::new(&north).bake(64).brightest_direction());
        assert!(straight.z < -0.75 && straight.y > 0.35 && straight.x.abs() < 0.15, "{straight:?}");
        north.rotation_deg = 90.0;
        let west = vec(Env::new(&north).bake(64).brightest_direction());
        assert!(west.x < -0.75 && west.y > 0.35 && west.z.abs() < 0.15, "{west:?}");
    }

    #[test]
    fn the_bake_is_finite_and_non_negative_at_any_sun() {
        for el in [-90.0f32, -18.0, -6.0, 0.0, 2.0, 45.0, 90.0] {
            let map = Env::new(&sky_params(el, 135.0)).bake(32);
            assert_eq!((map.width, map.height, map.data.len()), (32, 16, 32 * 16));
            assert!(
                map.data.iter().all(|t| {
                    t[0].is_finite() && t[1].is_finite() && t[2].is_finite()
                        && t[0] >= 0.0 && t[1] >= 0.0 && t[2] >= 0.0 && t[3] == 1.0
                }),
                "sun at {el} deg"
            );
        }
    }

    #[test]
    fn the_baked_sun_keeps_its_energy_between_texel_centres() {
        let lum_sum = |map: &makepad_render_material::ibl::EnvMap| -> f64 {
            let mut sum = 0.0f64;
            for y in 0..map.height {
                let theta = (y as f32 + 0.5) / map.height as f32 * std::f32::consts::PI;
                let d_omega = (std::f32::consts::TAU / map.width as f32) * (std::f32::consts::PI / map.height as f32) * theta.sin();
                for x in 0..map.width {
                    let t = map.data[y * map.width + x];
                    sum += (luminance(vec3f(t[0], t[1], t[2])) * d_omega) as f64;
                }
            }
            sum
        };
        // A 4 degree disc in a 64 x 32 map (5.6 degree texels): at texel
        // centres alone it would carry 0 to 2.4 times its energy, depending
        // on where it falls between them. The bake's refinement (A1's
        // refine_hot_spots, fed by Env::hot_spots) averages its texels.
        for az in [100.0f32, 101.4, 102.8, 104.2] {
            let mut p = sky_params(35.0, az);
            p.sky.sun_disc.size_deg = 4.0;
            let with_disc = lum_sum(&Env::new(&p).bake(64));
            p.sky.sun_disc.visible = false;
            let without = lum_sum(&Env::new(&p).bake(64));
            let atmo = Atmosphere::new(dir_from_az_el(az, 35.0), &p.sky.atmosphere, &p.sky.sun_disc);
            let want = luminance(atmo.sun_irradiance()) as f64;
            let got = with_disc - without;
            assert!((got / want - 1.0).abs() < 0.1, "az {az}: the baked disc carries {got} of {want}");
        }
    }

    /// A1's note for this task: `bake_par` refines only when every row was
    /// baked, and until the sun fed `hot_spots` nothing could show it.
    #[test]
    fn a_bake_cut_short_gets_no_refinement() {
        let mut p = sky_params(35.0, 100.0);
        p.sky.sun_disc.size_deg = 4.0;
        let env = Env::new(&p);
        let radiance = |d: Vec3f| env.radiance(d);
        // The texel-centre bake, and the refined one: the sun must matter here.
        let plain = crate::hdri::bake_with(64, &radiance);
        let refined = env.bake(64);
        assert_ne!(plain.data, refined.data, "the refinement changes this map");
        // Every row run: the parallel bake refines like the serial one.
        let all = env.bake_par(64, |rows, row| {
            for y in 0..rows {
                row(y);
            }
        });
        assert_eq!(all.data, refined.data);
        // The zenith row skipped (a cancelled job): no refinement, so the
        // rest of the map is the plain bake and the skipped row stays black.
        let cut = env.bake_par(64, |rows, row| {
            for y in 1..rows {
                row(y);
            }
        });
        assert_eq!(&cut.data[64..], &plain.data[64..]);
        assert!(cut.data[..64].iter().all(|t| t[0] == 0.0 && t[1] == 0.0 && t[2] == 0.0 && t[3] == 1.0));
    }
}
