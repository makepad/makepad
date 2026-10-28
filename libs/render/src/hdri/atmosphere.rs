//! Clear-sky atmosphere for the HDRI generator.
//!
//! Scattering by air (Rayleigh) and haze (Mie, two Cornette-Shanks lobes),
//! with ozone absorption, on a spherical Earth, in eight spectral bands taken
//! to RGB at the end. The marching scheme follows wwwtyro/glsl-atmosphere
//! (Unlicense, https://github.com/wwwtyro/glsl-atmosphere): march the view ray
//! through the shell and, at every sample, take the transmittance toward the
//! sun. What an environment map needs beyond that shader is added here:
//! ozone, the Earth's shadow (so twilight is right), a tabulated transmittance
//! toward the sun in place of the nested march, every order of scattering
//! after the first (Hillaire 2020, with the ground's bounce), a lit ground
//! below the horizon and an energy-preserving sun disc.
//!
//! ## Calibration
//!
//! The haze (its depth, phase and colour) is fitted against real clear-sky
//! HDRIs at sun heights from 1 to 85 deg, compared at the same sun height:
//! the sky's radiance on rings from the zenith down to 3 deg and against the
//! angle from the sun, its colour, and the sky's light on level ground
//! against the sun's. A clear noon's sky puts about 0.13 of the sun's light
//! on level ground (real ones 0.10-0.15), the glow round the sun falls as
//! real ones do, and a sunset's zenith stays blue.
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
//! ground irradiance of about 16.5 in luminance ((17.4, 16.5, 14.4) rgb), so
//! a white wall facing it reads E / pi = 5.3 plus a few tenths from the sky,
//! level ground gets 2.2 from the sky on top of the sun's 16.5, a clear zenith
//! 30 deg from the sun sits at about half a unit and the 0.53 deg disc at
//! about 3e5.
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
/// luminance of about half a unit, and sun, sky, clouds and ground keep their
/// physical ratios to one another.
pub const SUN_IRRADIANCE: f32 = 20.0;

const PLANET_RADIUS: f64 = 6_360_000.0;
const TOP_RADIUS: f64 = 6_420_000.0;
/// The viewer stands 200 m up, so a level view ray clears the ground.
const VIEWER_HEIGHT: f64 = 200.0;
const RAYLEIGH_SCALE_HEIGHT: f64 = 8_000.0;
const MIE_SCALE_HEIGHT: f64 = 1_200.0;
/// The light is carried in BANDS spectral bands, 40 nm wide and centred on
/// BAND_NM, and taken to RGB at the end (`Medium::rgb`). Three channels at
/// 680, 550 and 440 nm (Bruneton's and Hillaire's real-time choice) cannot
/// hold a low sun: the red channel's 680 nm crosses the air 2.6x more easily
/// than the light a camera's red sees (around 600 nm), and it misses ozone's
/// Chappuis band, which peaks at 600 nm and is what keeps a twilight zenith
/// blue (Hulburt 1953). With them, a 1 deg sun's zenith and a dawn's sky
/// came out grey-purple where real HDRIs are blue.
const BANDS: usize = 8;
type Spectrum = [f64; BANDS];
const BAND_NM: Spectrum = [400.0, 440.0, 480.0, 520.0, 560.0, 600.0, 640.0, 680.0];
/// CIE 1931 2 deg colour-matching functions (x, y, z), 380..700 nm every 10 nm.
const CIE_XYZ: [[f64; 3]; 33] = [
    [0.001368, 0.000039, 0.006450], [0.004243, 0.000120, 0.020050], [0.014310, 0.000396, 0.067850],
    [0.043510, 0.001210, 0.207400], [0.134380, 0.004000, 0.645600], [0.283900, 0.011600, 1.385600],
    [0.348280, 0.023000, 1.747060], [0.336200, 0.038000, 1.772110], [0.290800, 0.060000, 1.669200],
    [0.195360, 0.090980, 1.287640], [0.095640, 0.139020, 0.812950], [0.032010, 0.208020, 0.465180],
    [0.004900, 0.323000, 0.272000], [0.009300, 0.503000, 0.158200], [0.063270, 0.710000, 0.078250],
    [0.165500, 0.862000, 0.042160], [0.290400, 0.954000, 0.020300], [0.433450, 0.994950, 0.008750],
    [0.594500, 0.995000, 0.003900], [0.762100, 0.952000, 0.002100], [0.916300, 0.870000, 0.001650],
    [1.026300, 0.757000, 0.001100], [1.062200, 0.631000, 0.000800], [1.002600, 0.503000, 0.000340],
    [0.854450, 0.381000, 0.000190], [0.642400, 0.265000, 0.000050], [0.447900, 0.175000, 0.000020],
    [0.283500, 0.107000, 0.000000], [0.164900, 0.061000, 0.000000], [0.087400, 0.032000, 0.000000],
    [0.046770, 0.017000, 0.000000], [0.022700, 0.008210, 0.000000], [0.011359, 0.004102, 0.000000],
];
/// The sun's spectral irradiance above the air, W m^-2 nm^-1, 380..700 nm
/// every 10 nm (the spectrum Bruneton's precomputed-scattering demo uses).
/// Only its shape matters: the bands are scaled so that it is white.
const SOLAR_SPECTRUM: [f64; 33] = [
    1.01249, 1.14716, 1.72765, 1.73054, 1.6887, 1.61253, 1.91198, 2.03474, 2.02042, 2.02212, 1.93377,
    1.95809, 1.91686, 1.8298, 1.8685, 1.8931, 1.85149, 1.8504, 1.8341, 1.8345, 1.8147, 1.78158, 1.7533,
    1.6965, 1.68194, 1.64654, 1.6048, 1.52143, 1.55622, 1.5113, 1.474, 1.4482, 1.41018,
];
/// Haze 1, "a clear day": the aerosol's vertical optical depth at 550 nm.
/// Measured clear days sit at 0.05-0.15 (continental sites; a visibility of
/// 40-70 km). This one is fitted against real clear-sky HDRIs at sun heights
/// from 1 to 85 deg: the sky's light on level ground against the sun's, and
/// the glow round the sun. The scale height puts its sea-level extinction at
/// depth / MIE_SCALE_HEIGHT.
const AEROSOL_DEPTH: f64 = 0.07;
/// The aerosol's extinction falls with wavelength as lambda^-1.3 (the
/// Angstrom exponent of continental haze), so it is a little bluer than white.
const AEROSOL_ANGSTROM: f64 = 1.3;
/// Haze absorbs a little as well: it scatters 90 % of what it takes out.
const AEROSOL_ALBEDO: f64 = 0.9;
/// Haze scatters mostly forward, with a narrow peak round the sun (its
/// diffraction) over a broad lobe: the sum of two Cornette-Shanks phases,
/// the peak carrying MIE_PEAK_SHARE of the light. One lobe cannot do both:
/// a single g of 0.76 lit the sky 6-15 deg from the sun 1.4x too bright
/// against real HDRIs while it lit the first 5 deg too dim. Together their
/// mean cosine is 0.78, a continental haze's.
const MIE_G_BROAD: f64 = 0.65;
const MIE_G_PEAK: f64 = 0.95;
const MIE_PEAK_SHARE: f64 = 0.25;
/// Ozone only absorbs, in the Chappuis band (500-700 nm, peaking at 600):
/// over the long grazing paths of twilight that is what keeps the zenith
/// blue. Its absorption cross section in m^2, 380..700 nm every 10 nm
/// (Bruneton's, after Serdyuchenko et al. 2014), and its peak number
/// density in m^-3: 300 Dobson units spread over the tent below.
const OZONE_CROSS_SECTION: [f64; 33] = [
    2.818e-28, 6.636e-28, 1.527e-27, 2.763e-27, 5.52e-27, 8.451e-27, 1.582e-26, 2.316e-26, 3.669e-26,
    4.924e-26, 7.752e-26, 9.016e-26, 1.48e-25, 1.602e-25, 2.139e-25, 2.755e-25, 3.091e-25, 3.5e-25,
    4.266e-25, 4.672e-25, 4.398e-25, 4.701e-25, 5.019e-25, 4.305e-25, 3.74e-25, 3.215e-25, 2.662e-25,
    2.238e-25, 1.852e-25, 1.473e-25, 1.209e-25, 9.423e-26, 7.455e-26,
];
const OZONE_PEAK_DENSITY: f64 = 300.0 * 2.687e20 / 15_000.0;
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

/// Multiple scattering, after Hillaire, "A Scalable and Production Ready Sky
/// and Atmosphere Rendering Technique" (EGSR 2020): at every height and sun
/// angle, the second order of scattering (the sun's light scattered once on
/// the way, plus what the lit ground bounces up, seen from that point in
/// MS_DIRS directions) is summed to all orders as a geometric series, and the
/// view march adds it back as an isotropic source. That is what keeps a real
/// sky's light bright overhead, carries the ground's light into the air and
/// keeps the sky glowing after sunset. Our sun is fixed, so the table only
/// spans the sun angles the view rays can meet: the viewer's own, +- the
/// arc a ray to the top of the air crosses (under 8 deg).
const MS_HEIGHTS: usize = 12;
const MS_ANGLES: usize = 10;
const MS_DIRS: usize = 32;
const MS_STEPS: usize = 12;
const MS_ANGLE_REACH_DEG: f64 = 9.0;

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
        let multi = MultiScatter::new(&medium, &table, sun_sin as f64);
        let lut = build_sky_lut(&medium, &table, sun_sin as f64, &multi);
        let (ambient, sky_irradiance) = hemisphere_integrals(&lut);
        let sun_irradiance = medium.rgb(&exact_transmittance(&medium, sun));
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
        self.medium.rgb(&exact_transmittance(&self.medium, dir)) * (1.0 / SUN_IRRADIANCE)
    }

    /// SUN_IRRADIANCE x transmittance along the sun: what a surface facing
    /// the sun receives at the ground.
    pub fn sun_irradiance(&self) -> Vec3f {
        self.sun_irradiance
    }

    /// What a surface facing the sun receives `height_m` above the ground
    /// (the viewer's own height or more): the light a cloud deck is lit by.
    /// Up there the sun crosses less of the haze, which sits low, so a low
    /// sun's light on the clouds is far brighter than at the ground; and a
    /// sun just under the viewer's horizon still lights a deck high enough.
    pub fn sun_irradiance_at(&self, height_m: f32) -> Vec3f {
        let h = (height_m as f64).max(VIEWER_HEIGHT);
        match column_depth(PLANET_RADIUS + h, self.sun_dir.y.clamp(-1.0, 1.0) as f64, EXACT_STEPS) {
            Some(column) => self.medium.rgb(&self.medium.extinction(column).map(|e| (-e).exp())),
            None => Vec3f::default(),
        }
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

    /// The disc as [`Self::sun_disc`] draws it, for a viewer that draws it
    /// itself (`Env::sky_discs`): None when it is hidden. Its radiance at a
    /// direction `theta` off `dir` is `radiance x disc_profile(theta, radius,
    /// softness)`, 0 below the horizon.
    pub fn disc_look(&self) -> Option<SunDiscLook> {
        self.disc_visible.then(|| SunDiscLook {
            dir: self.sun_dir,
            radius: self.disc_radius,
            softness: self.disc_softness,
            limb_darkening: LIMB_DARKENING,
            radiance: self.disc_radiance,
        })
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
/// at sea level, per band; and the bands' way back to the scene's RGB.
#[derive(Clone, Copy)]
struct Medium {
    rayleigh: Spectrum,
    mie_scattering: Spectrum,
    mie_extinction: Spectrum,
    ozone: Spectrum,
    /// The ground's albedo, for the light it bounces back into the air.
    ground: Spectrum,
    to_rgb: [Spectrum; 3],
}

impl Medium {
    /// `p` is held by clamp() (`Atmosphere::new`).
    fn new(p: &AtmosphereParams) -> Medium {
        let (air, haze, ozone) = (p.air as f64, p.haze as f64, p.ozone as f64);
        let mie_extinction: Spectrum = std::array::from_fn(|k| {
            haze * AEROSOL_DEPTH / MIE_SCALE_HEIGHT * (BAND_NM[k] / 550.0).powf(-AEROSOL_ANGSTROM)
        });
        let cross = band_ozone_cross_sections();
        // The ground's colour spread over the bands: blue at 440 nm, green at
        // 550, red at 640, linear between and held beyond.
        let [gr, gg, gb] = p.ground_color.map(|g| g as f64);
        let ground = BAND_NM.map(|nm| {
            if nm <= 440.0 {
                gb
            } else if nm <= 550.0 {
                gb + (gg - gb) * (nm - 440.0) / 110.0
            } else if nm <= 640.0 {
                gg + (gr - gg) * (nm - 550.0) / 90.0
            } else {
                gr
            }
        });
        Medium {
            // Rayleigh: 1.24062e-6 / lambda^4 per metre, lambda in um (Bruneton's air).
            rayleigh: BAND_NM.map(|nm| air * 1.24062e-6 / (nm / 1000.0).powi(4)),
            mie_scattering: mie_extinction.map(|e| e * AEROSOL_ALBEDO),
            mie_extinction,
            ozone: cross.map(|c| c * OZONE_PEAK_DENSITY * ozone),
            ground,
            to_rgb: band_to_rgb(),
        }
    }

    /// Per-band extinction for (air, haze, ozone) densities: a coefficient
    /// per metre for point densities, an optical depth for integrated ones.
    fn extinction(&self, d: [f64; 3]) -> Spectrum {
        std::array::from_fn(|k| self.rayleigh[k] * d[0] + self.mie_extinction[k] * d[1] + self.ozone[k] * d[2])
    }

    /// Per-band scattering coefficient for point densities (air, haze, ozone).
    fn scattering(&self, d: [f64; 3]) -> Spectrum {
        std::array::from_fn(|k| self.rayleigh[k] * d[0] + self.mie_scattering[k] * d[1])
    }

    /// A spectrum, per unit of the sun's light at the top of the air, as the
    /// scene's linear Rec.709 RGB: SUN_IRRADIANCE per channel for the sun
    /// itself. A deeply reddened spectrum (a sun on the horizon) lies outside
    /// Rec.709 and its blue would fall below zero (the matching functions'
    /// negative lobes); a channel is held at a millionth of the colour's sum
    /// instead, inside the gamut and never exactly 0, which a light's colour
    /// is divided by.
    fn rgb(&self, s: &Spectrum) -> Vec3f {
        let c: [f64; 3] = std::array::from_fn(|c| {
            let mut sum = 0.0;
            for k in 0..BANDS {
                sum += self.to_rgb[c][k] * s[k];
            }
            sum * SUN_IRRADIANCE as f64
        });
        let floor = 1.0e-6 * (c[0].max(0.0) + c[1].max(0.0) + c[2].max(0.0));
        let c = c.map(|v| v.max(floor));
        vec3f(c[0] as f32, c[1] as f32, c[2] as f32)
    }
}

/// The weight of each 10 nm sample (from 380 nm) in each band: 1 inside it,
/// a half on a border it shares with the next band.
fn band_share(i: usize, k: usize) -> f64 {
    let nm = 380.0 + 10.0 * i as f64;
    let d = (nm - BAND_NM[k]).abs();
    if d < 19.9 {
        1.0
    } else if d < 20.1 {
        0.5
    } else {
        0.0
    }
}

/// Each band's ozone cross section: the mean over its 10 nm samples.
fn band_ozone_cross_sections() -> Spectrum {
    std::array::from_fn(|k| {
        let (mut sum, mut weight) = (0.0, 0.0);
        for (i, c) in OZONE_CROSS_SECTION.iter().enumerate() {
            let w = band_share(i, k);
            sum += w * c;
            weight += w;
        }
        sum / weight
    })
}

/// The bands' weights in linear Rec.709: the CIE 1931 colour-matching
/// functions times the sun's spectrum at the top of the air, summed over
/// each band and taken to RGB, each channel scaled so that the sun's own
/// spectrum (1 in every band) is white. White is therefore the sun above
/// the air, the scene's white as it always was.
fn band_to_rgb() -> [Spectrum; 3] {
    const XYZ_TO_RGB: [[f64; 3]; 3] = [
        [3.2406, -1.5372, -0.4986],
        [-0.9689, 1.8758, 0.0415],
        [0.0557, -0.2040, 1.0570],
    ];
    let mut w = [[0.0f64; BANDS]; 3];
    for (i, (xyz, sun)) in CIE_XYZ.iter().zip(SOLAR_SPECTRUM).enumerate() {
        for k in 0..BANDS {
            let share = band_share(i, k) * sun;
            if share == 0.0 {
                continue;
            }
            for c in 0..3 {
                let m = XYZ_TO_RGB[c];
                w[c][k] += share * (m[0] * xyz[0] + m[1] * xyz[1] + m[2] * xyz[2]);
            }
        }
    }
    for row in &mut w {
        let total: f64 = row.iter().sum();
        row.iter_mut().for_each(|v| *v /= total);
    }
    w
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

/// Distance from radius `r` along `mu` to the ground (the ray must hit it).
fn distance_to_ground(r: f64, mu: f64) -> f64 {
    let disc = r * r * (mu * mu - 1.0) + PLANET_RADIUS * PLANET_RADIUS;
    (-r * mu - disc.max(0.0).sqrt()).max(0.0)
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

/// Spectral transmittance from the viewer to space along `dir`; zero when
/// the ground is in the way.
fn exact_transmittance(medium: &Medium, dir: Vec3f) -> Spectrum {
    let length = dir.length();
    if length.is_nan() || length <= 0.0 {
        return [0.0; BANDS];
    }
    let mu = (dir.y / length).clamp(-1.0, 1.0) as f64;
    match column_depth(PLANET_RADIUS + VIEWER_HEIGHT, mu, EXACT_STEPS) {
        Some(column) => medium.extinction(column).map(|e| (-e).exp()),
        None => [0.0; BANDS],
    }
}

/// Each band's transmittance toward space over (height, angle above
/// grazing). Stored as transmittance, not optical depth, so a lookup is a
/// blend with no exponential: the sky-view march makes over a million.
struct DepthTable {
    transmittance: Vec<[f32; BANDS]>,
}

impl DepthTable {
    fn new(medium: &Medium) -> DepthTable {
        let mut transmittance = Vec::with_capacity(DEPTH_HEIGHTS * DEPTH_ANGLES);
        for j in 0..DEPTH_HEIGHTS {
            let y = j as f64 / (DEPTH_HEIGHTS - 1) as f64;
            let r = PLANET_RADIUS + (TOP_RADIUS - PLANET_RADIUS) * y * y;
            let mu_graze = grazing_mu(r);
            for i in 0..DEPTH_ANGLES {
                let x = i as f64 / (DEPTH_ANGLES - 1) as f64;
                let mu = mu_graze + (1.0 - mu_graze) * x * x;
                // Every column is at or above the grazing ray by construction.
                let ext = medium.extinction(integrate_column(r, mu, DEPTH_STEPS));
                transmittance.push(ext.map(|e| (-e).exp() as f32));
            }
        }
        DepthTable { transmittance }
    }

    /// Transmittance toward space from radius `r` along `mu`. The planet's
    /// shadow is decided exactly, not blurred by the table.
    fn transmittance(&self, r: f64, mu: f64) -> Spectrum {
        self.transmittance_at(&DepthHeight::new(r), mu)
    }

    /// `transmittance` at a height whose part of the lookup is done: a march
    /// asks once per step for every sun direction it lights the step with.
    fn transmittance_at(&self, h: &DepthHeight, mu: f64) -> Spectrum {
        if hits_ground(h.r, mu) {
            return [0.0; BANDS];
        }
        let x = ((mu - h.mu_graze) * h.inv_span).clamp(0.0, 1.0).sqrt();
        let fx = x * (DEPTH_ANGLES - 1) as f64;
        let x0 = (fx as usize).min(DEPTH_ANGLES - 2);
        let (ty, tx) = (h.ty, fx - x0 as f64);
        let row0 = h.row;
        let row1 = row0 + DEPTH_ANGLES;
        let (a, b) = (&self.transmittance[row0 + x0], &self.transmittance[row0 + x0 + 1]);
        let (c, d) = (&self.transmittance[row1 + x0], &self.transmittance[row1 + x0 + 1]);
        std::array::from_fn(|k| {
            let near = a[k] as f64 + (b[k] as f64 - a[k] as f64) * tx;
            let far = c[k] as f64 + (d[k] as f64 - c[k] as f64) * tx;
            near + (far - near) * ty
        })
    }
}

/// The height's share of a `DepthTable` lookup.
#[derive(Clone, Copy, Default)]
struct DepthHeight {
    r: f64,
    mu_graze: f64,
    inv_span: f64,
    /// The first texel of the lower row, and the weight of the upper one.
    row: usize,
    ty: f64,
}

impl DepthHeight {
    fn new(r: f64) -> DepthHeight {
        let y = ((r - PLANET_RADIUS) / (TOP_RADIUS - PLANET_RADIUS)).clamp(0.0, 1.0).sqrt();
        let mu_graze = grazing_mu(r);
        let fy = y * (DEPTH_HEIGHTS - 1) as f64;
        let y0 = (fy as usize).min(DEPTH_HEIGHTS - 2);
        DepthHeight { r, mu_graze, inv_span: 1.0 / (1.0 - mu_graze), row: y0 * DEPTH_ANGLES, ty: fy - y0 as f64 }
    }
}

fn rayleigh_phase(nu: f64) -> f64 {
    3.0 / (16.0 * std::f64::consts::PI) * (1.0 + nu * nu)
}

/// The haze's phase: the broad lobe and the narrow peak (MIE_PEAK_SHARE).
fn mie_phase(nu: f64) -> f64 {
    (1.0 - MIE_PEAK_SHARE) * cornette_shanks(nu, MIE_G_BROAD) + MIE_PEAK_SHARE * cornette_shanks(nu, MIE_G_PEAK)
}

/// Cornette-Shanks phase (as in glsl-atmosphere): Henyey-Greenstein with a
/// (1 + nu^2) factor that fits haze better; it integrates to 1 over the sphere.
fn cornette_shanks(nu: f64, g: f64) -> f64 {
    let gg = g * g;
    3.0 / (8.0 * std::f64::consts::PI) * ((1.0 - gg) * (1.0 + nu * nu))
        / ((2.0 + gg) * (1.0 + gg - 2.0 * g * nu).powf(1.5))
}

/// The multiple-scattering source (Hillaire 2020, section 5.5): the radiance
/// of all orders above the first, averaged over directions, per unit of the
/// sun's light above the air, over (height, cosine of the sun's zenith
/// angle). A view sample adds scattering x this.
struct MultiScatter {
    psi: Vec<Spectrum>,
    /// The sun's cosine at the first and the last column.
    mu0: f64,
    mu1: f64,
}

/// One step of a march whose geometry does not depend on the sun: where it
/// is, and what it scatters toward the ray's origin per unit of light (air
/// and haze apart, with the view transmittance and the step length in).
#[derive(Clone, Copy)]
struct RayStep {
    /// Position, the ray's origin at (0, r0, 0) and y up.
    p: [f64; 3],
    r: f64,
    air: Spectrum,
    haze: Spectrum,
    /// The step's height in the depth table and in the multiple-scattering one.
    depth: DepthHeight,
    ms_row: usize,
    ms_ty: f64,
}

impl Default for RayStep {
    fn default() -> RayStep {
        RayStep { p: [0.0; 3], r: 0.0, air: [0.0; BANDS], haze: [0.0; BANDS], depth: DepthHeight::default(), ms_row: 0, ms_ty: 0.0 }
    }
}

impl MultiScatter {
    fn new(medium: &Medium, table: &DepthTable, sun_sin: f64) -> MultiScatter {
        let sun_theta = sun_sin.clamp(-1.0, 1.0).acos();
        let reach = MS_ANGLE_REACH_DEG.to_radians();
        let mu0 = (sun_theta + reach).min(std::f64::consts::PI).cos();
        let mu1 = (sun_theta - reach).max(0.0).cos();
        let dirs = sphere_directions();
        let inv_4pi = 1.0 / (4.0 * std::f64::consts::PI);
        let n = dirs.len() as f64;
        // The point sits at (0, r, 0) and the sun in the x-y plane.
        let suns: Vec<[f64; 2]> = (0..MS_ANGLES)
            .map(|i| {
                let mu = mu0 + (mu1 - mu0) * i as f64 / (MS_ANGLES - 1) as f64;
                [(1.0 - mu * mu).max(0.0).sqrt(), mu]
            })
            .collect();
        let mut psi = vec![[0.0f64; BANDS]; MS_HEIGHTS * MS_ANGLES];
        let mut steps = [RayStep::default(); MS_STEPS];
        let mut second = vec![[0.0f64; BANDS]; MS_ANGLES];
        for j in 0..MS_HEIGHTS {
            let y = j as f64 / (MS_HEIGHTS - 1) as f64;
            let r = PLANET_RADIUS + (TOP_RADIUS - PLANET_RADIUS) * y * y;
            second.iter_mut().for_each(|l| *l = [0.0; BANDS]);
            let mut transfer = [0.0f64; BANDS];
            for w in &dirs {
                let ground = hits_ground(r, w[1]);
                let length = if ground { distance_to_ground(r, w[1]) } else { distance_to_top(r, w[1]) };
                // The march is the same for every sun angle; only the light
                // that reaches each step differs.
                let mut depth = [0.0f64; BANDS];
                for (s, step) in steps.iter_mut().enumerate() {
                    let (t, ds) = segment(s, MS_STEPS, length);
                    let p = [w[0] * t, r + w[1] * t, w[2] * t];
                    let rp = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                    let dens = densities(rp - PLANET_RADIUS);
                    let ext = medium.extinction(dens);
                    let sca = medium.scattering(dens);
                    let mut weight = [0.0f64; BANDS];
                    for k in 0..BANDS {
                        let view_t = (-(depth[k] + 0.5 * ext[k] * ds)).exp();
                        depth[k] += ext[k] * ds;
                        weight[k] = view_t * sca[k] * ds;
                        transfer[k] += weight[k];
                    }
                    *step = RayStep { p, r: rp, air: weight, depth: DepthHeight::new(rp), ..RayStep::default() };
                }
                let ground_at = ground.then(|| {
                    let p = [w[0] * length, r + w[1] * length, w[2] * length];
                    let rp = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                    let t: Spectrum = depth.map(|d| (-d).exp());
                    (p, rp, t)
                });
                for (sun, acc) in suns.iter().zip(second.iter_mut()) {
                    for step in &steps {
                        let mu_sun = (step.p[0] * sun[0] + step.p[1] * sun[1]) / step.r;
                        let sun_t = table.transmittance_at(&step.depth, mu_sun);
                        // The first scattering is taken as isotropic, as in the paper.
                        for k in 0..BANDS {
                            acc[k] += step.air[k] * sun_t[k] * inv_4pi;
                        }
                    }
                    if let Some((p, rp, view_t)) = ground_at {
                        // The lit ground: Lambertian, its albedo x E cos / pi.
                        let mu_sun = (p[0] * sun[0] + p[1] * sun[1]) / rp;
                        if mu_sun > 0.0 {
                            let sun_t = table.transmittance(PLANET_RADIUS, mu_sun);
                            for k in 0..BANDS {
                                acc[k] += view_t[k] * sun_t[k] * mu_sun * medium.ground[k] / std::f64::consts::PI;
                            }
                        }
                    }
                }
            }
            // Every order above the second is the one before it times the
            // share an isotropic radiance scatters again: 1 / (1 - f).
            let f: Spectrum = std::array::from_fn(|k| (transfer[k] / n).min(0.99));
            for (i, l) in second.iter().enumerate() {
                psi[j * MS_ANGLES + i] = std::array::from_fn(|k| l[k] / n / (1.0 - f[k]));
            }
        }
        MultiScatter { psi, mu0, mu1 }
    }

    /// The height's share of a lookup at radius `r`: the first texel of the
    /// lower row and the weight of the upper one.
    fn height(r: f64) -> (usize, f64) {
        let y = ((r - PLANET_RADIUS) / (TOP_RADIUS - PLANET_RADIUS)).clamp(0.0, 1.0).sqrt();
        let fy = y * (MS_HEIGHTS - 1) as f64;
        let y0 = (fy as usize).min(MS_HEIGHTS - 2);
        (y0 * MS_ANGLES, fy - y0 as f64)
    }

    /// The source at the height `height` names, with the sun at cosine
    /// `mu_sun` to the vertical there.
    fn at(&self, (row0, ty): (usize, f64), mu_sun: f64) -> Spectrum {
        let x = if self.mu1 > self.mu0 { ((mu_sun - self.mu0) / (self.mu1 - self.mu0)).clamp(0.0, 1.0) } else { 0.0 };
        let fx = x * (MS_ANGLES - 1) as f64;
        let x0 = (fx as usize).min(MS_ANGLES - 2);
        let tx = fx - x0 as f64;
        let row1 = row0 + MS_ANGLES;
        let (a, b) = (&self.psi[row0 + x0], &self.psi[row0 + x0 + 1]);
        let (c, d) = (&self.psi[row1 + x0], &self.psi[row1 + x0 + 1]);
        std::array::from_fn(|k| {
            let near = a[k] + (b[k] - a[k]) * tx;
            let far = c[k] + (d[k] - c[k]) * tx;
            near + (far - near) * ty
        })
    }
}

/// MS_DIRS directions spread evenly over the sphere (a spherical Fibonacci set).
fn sphere_directions() -> Vec<[f64; 3]> {
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    (0..MS_DIRS)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / MS_DIRS as f64;
            let ring = (1.0 - y * y).max(0.0).sqrt();
            let phi = golden * i as f64;
            [ring * phi.cos(), y, ring * phi.sin()]
        })
        .collect()
}

/// The view march along a ray at elevation sine `view_sin`. It is the same
/// for every azimuth, so a row of the sky-view table marches once.
fn view_steps(medium: &Medium, view_sin: f64) -> [RayStep; VIEW_STEPS] {
    let r0 = PLANET_RADIUS + VIEWER_HEIGHT;
    let length = distance_to_top(r0, view_sin);
    let mut steps = [RayStep::default(); VIEW_STEPS];
    // Extinction optical depth from the viewer to the start of the current segment.
    let mut depth = [0.0f64; BANDS];
    for (i, step) in steps.iter_mut().enumerate() {
        let (t, ds) = segment(i, VIEW_STEPS, length);
        let r = radius_at(r0, view_sin, t);
        let dens = densities(r - PLANET_RADIUS);
        let ext = medium.extinction(dens);
        let (mut air, mut haze) = ([0.0f64; BANDS], [0.0f64; BANDS]);
        for k in 0..BANDS {
            let view_t = (-(depth[k] + 0.5 * ext[k] * ds)).exp();
            depth[k] += ext[k] * ds;
            air[k] = view_t * medium.rayleigh[k] * dens[0] * ds;
            haze[k] = view_t * medium.mie_scattering[k] * dens[1] * ds;
        }
        // `p` holds (t, 0, 0): the distance along the ray is all `in_scatter` needs.
        let (ms_row, ms_ty) = MultiScatter::height(r);
        *step = RayStep { p: [t, 0.0, 0.0], r, air, haze, depth: DepthHeight::new(r), ms_row, ms_ty };
    }
    steps
}

/// Radiance reaching the viewer along a ray marched as `steps`, per unit of
/// the sun's light above the air, with the sun at elevation sine `sun_sin`
/// and view-sun cosine `nu`: single scattering plus the multiple-scattering
/// source.
fn in_scatter(table: &DepthTable, steps: &[RayStep], sun_sin: f64, nu: f64, multi: &MultiScatter) -> Spectrum {
    let r0 = PLANET_RADIUS + VIEWER_HEIGHT;
    let phase_r = rayleigh_phase(nu);
    let phase_m = mie_phase(nu);
    let mut light = [0.0f64; BANDS];
    for step in steps {
        let (t, r) = (step.p[0], step.r);
        // The sun's direction is fixed but the local vertical turns along the
        // ray: mu_sun = dot(sample position, sun) / |sample position|.
        let mu_sun = (r0 * sun_sin + t * nu) / r;
        let sun_t = table.transmittance_at(&step.depth, mu_sun);
        let psi = multi.at((step.ms_row, step.ms_ty), mu_sun);
        for k in 0..BANDS {
            light[k] += sun_t[k] * (phase_r * step.air[k] + phase_m * step.haze[k]) + (step.air[k] + step.haze[k]) * psi[k];
        }
    }
    light
}

fn build_sky_lut(medium: &Medium, table: &DepthTable, sun_sin: f64, multi: &MultiScatter) -> Vec<Vec3f> {
    let sun_cos = (1.0 - sun_sin * sun_sin).max(0.0).sqrt();
    let mut lut = Vec::with_capacity(LUT_WIDTH * LUT_HEIGHT);
    for j in 0..LUT_HEIGHT {
        let v = j as f64 / (LUT_HEIGHT - 1) as f64;
        let elevation = std::f64::consts::FRAC_PI_2 * v * v;
        let (view_sin, view_cos) = elevation.sin_cos();
        let steps = view_steps(medium, view_sin);
        for i in 0..LUT_WIDTH {
            let phi = std::f64::consts::PI * i as f64 / (LUT_WIDTH - 1) as f64;
            // The sun sits at azimuth 0 of the table's frame.
            let nu = view_cos * sun_cos * phi.cos() + view_sin * sun_sin;
            lut.push(medium.rgb(&in_scatter(table, &steps, sun_sin, nu, multi)));
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

/// The sun's disc as `Atmosphere::sun_disc` draws it ([`Atmosphere::disc_look`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunDiscLook {
    /// The disc's centre, unit length.
    pub dir: Vec3f,
    /// Its angular radius in radians: the soft edge is `softness` x radius
    /// wide, centred on it.
    pub radius: f32,
    pub softness: f32,
    /// Linear limb darkening: 1 - limb_darkening x (1 - sqrt(1 - (theta / outer)^2)).
    pub limb_darkening: f32,
    /// The radiance where the profile is 1 (the centre).
    pub radiance: Vec3f,
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
        // SUN_IRRADIANCE puts a clear high-sun zenith at about half a unit.
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
        // Four times the albedo is four times the light, and a little more:
        // the brighter ground also lights the air above it, which lights it
        // back (the multiple scattering's ground bounce).
        let ratio = luminance(bright.sky(down)) / luminance(dim.sky(down));
        assert!(ratio >= 4.0 && ratio < 4.0 * 1.05, "ground is linear in its colour, with its bounce: {ratio}");
        let sky_ratio = luminance(bright.sky_irradiance()) / luminance(dim.sky_irradiance());
        assert!(sky_ratio > 1.0 && sky_ratio < 1.2, "the bright ground lights the sky a little: {sky_ratio}");
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
        // Zenith transmittance in luminance: Rayleigh 0.11, haze 0.07 and
        // ozone 0.03 of optical depth in the green.
        let t = luminance(noon.sun_irradiance()) / SUN_IRRADIANCE;
        assert!(t > 0.80 && t < 0.86, "zenith transmittance {t}");
        // A white wall facing the high sun: E / pi, about 5.3 at 20.0.
        let wall = luminance(noon.sun_irradiance()) / std::f32::consts::PI;
        let want = 0.825 * SUN_IRRADIANCE / std::f32::consts::PI;
        assert!((wall / want - 1.0).abs() < 0.05, "noon white wall {wall} vs {want}");
        // The sky's light on level ground against the sun's: a clear noon's
        // 0.10-0.15 (real HDRIs and measured diffuse fractions).
        let share = luminance(noon.sky_irradiance()) / luminance(noon.sun_irradiance());
        assert!(share > 0.10 && share < 0.16, "sky / sun on level ground {share}");
        // A clear high-sun zenith, 30 deg from the sun: about half a unit.
        let high = atmo_at(60.0, 0.0);
        let zenith = luminance(high.sky(vec3f(0.0, 1.0, 0.0))) / SUN_IRRADIANCE;
        assert!(zenith > 0.015 && zenith < 0.04, "zenith / SUN_IRRADIANCE {zenith}");
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
