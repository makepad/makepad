//! Analytic daylight sky — the Preetham model, split CPU/GPU.
//!
//! "A Practical Analytic Model for Daylight" (Preetham, Shirley, Smits '99):
//! sky luminance/chromaticity in Yxy, each channel a Perez function
//!
//! ```text
//! F(theta, gamma) = (1 + A e^(B / cos theta)) (1 + C e^(D gamma) + E cos^2 gamma)
//! ```
//!
//! over view zenith angle `theta` and sun angle `gamma`, normalised by the
//! zenith value: `channel = zenith * F(theta, gamma) / F(0, theta_s)`.
//!
//! Everything that depends only on the SUN — the five Perez coefficients per
//! channel (functions of turbidity), the zenith Yxy, and the `1 / F(0,
//! theta_s)` normaliser — is computed HERE, once per frame, and shipped to
//! [`crate::shaders::DrawSceneSky`] as six vec4 lanes. The shader evaluates
//! only the per-pixel half: two angles, the Perez product, Yxy -> XYZ ->
//! linear sRGB, and one exposure/Reinhard tone-map (the model's output is
//! absolute kcd/m² — unmapped it blows out any 8-bit target).
//!
//! Night is not Preetham's domain: below ~2 degrees of sun elevation the
//! coefficients diverge, so the frame clamps the model at the horizon and
//! carries a separate `night` blend the shader uses to fade to a dark
//! blue-black dome.

use makepad_draw::*;

/// Angular radius of the solar disc (0.2665 degrees).
pub const SUN_RADIUS: f32 = 0.004_65;
/// Perez luminance (kcd/m2) to linear scene-radiance units.
pub const DAYLIGHT_SCALE: f32 = 0.025;
/// Mean-luminance floor used only by the exposure meter.
pub const EXPOSURE_LUMINANCE_FLOOR: f32 = 0.03;
/// Metering target for the mean sky luminance.
pub const EXPOSURE_KEY: f32 = 0.12;

const NIGHT_HORIZON: Vec3f = Vec3f {
    x: 0.0040,
    y: 0.0060,
    z: 0.0120,
};
const NIGHT_ZENITH: Vec3f = Vec3f {
    x: 0.0012,
    y: 0.0020,
    z: 0.0050,
};

/// Civil date used by the shared NOAA solar-position calculation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkyDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Default for SkyDate {
    fn default() -> Self {
        Self {
            year: 2024,
            month: 6,
            day: 21,
        }
    }
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month.clamp(1, 12) {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// NOAA Solar Calculator, Julian-century form, without atmospheric
/// refraction. `time_local` is a civil decimal hour and `tz_offset` is hours
/// east of UTC. Returns geometric elevation and azimuth clockwise from north.
pub fn noaa_solar_position(
    date: SkyDate,
    time_local: f32,
    tz_offset: f32,
    latitude: f32,
    longitude: f32,
) -> (f32, f32) {
    let month = date.month.clamp(1, 12);
    let day = date.day.clamp(1, days_in_month(date.year, month));
    let (mut year, mut adjusted_month) = (date.year, month as i32);
    if adjusted_month <= 2 {
        year -= 1;
        adjusted_month += 12;
    }
    let century = (year as f64 / 100.0).floor();
    let correction = 2.0 - century + (century / 4.0).floor();
    let midnight_jd = (365.25 * (year as f64 + 4716.0)).floor()
        + (30.6001 * (adjusted_month as f64 + 1.0)).floor()
        + day as f64
        + correction
        - 1524.5;
    let local_minutes = time_local.clamp(0.0, 24.0) as f64 * 60.0;
    let jd = midnight_jd + local_minutes / 1440.0 - tz_offset as f64 / 24.0;
    let t = (jd - 2451545.0) / 36525.0;
    let to_rad = std::f64::consts::PI / 180.0;
    let to_deg = 180.0 / std::f64::consts::PI;

    let mut l0 = (280.46646 + t * (36000.76983 + t * 0.0003032)) % 360.0;
    if l0 < 0.0 {
        l0 += 360.0;
    }
    let anomaly = 357.52911 + t * (35999.05029 - 0.0001537 * t);
    let eccentricity = 0.016708634 - t * (0.000042037 + 0.0000001267 * t);
    let anomaly_rad = anomaly * to_rad;
    let centre = anomaly_rad.sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
        + (2.0 * anomaly_rad).sin() * (0.019993 - 0.000101 * t)
        + (3.0 * anomaly_rad).sin() * 0.000289;
    let omega = 125.04 - 1934.136 * t;
    let lambda = l0 + centre - 0.00569 - 0.00478 * (omega * to_rad).sin();
    let obliquity_seconds = 21.448 - t * (46.815 + t * (0.00059 - t * 0.001813));
    let obliquity = 23.0
        + (26.0 + obliquity_seconds / 60.0) / 60.0
        + 0.00256 * (omega * to_rad).cos();
    let declination = ((obliquity * to_rad).sin() * (lambda * to_rad).sin())
        .clamp(-1.0, 1.0)
        .asin();
    let y = (0.5 * obliquity * to_rad).tan().powi(2);
    let l0_rad = l0 * to_rad;
    let equation_of_time = 4.0
        * to_deg
        * (y * (2.0 * l0_rad).sin()
            - 2.0 * eccentricity * anomaly_rad.sin()
            + 4.0 * eccentricity * y * anomaly_rad.sin() * (2.0 * l0_rad).cos()
            - 0.5 * y * y * (4.0 * l0_rad).sin()
            - 1.25 * eccentricity * eccentricity * (2.0 * anomaly_rad).sin());
    let solar_minutes = (local_minutes + equation_of_time + 4.0 * longitude as f64
        - 60.0 * tz_offset as f64)
        .rem_euclid(1440.0);
    let hour_angle = (solar_minutes / 4.0 - 180.0) * to_rad;
    let lat = latitude.clamp(-90.0, 90.0) as f64 * to_rad;
    let cos_zenith = (lat.sin() * declination.sin()
        + lat.cos() * declination.cos() * hour_angle.cos())
    .clamp(-1.0, 1.0);
    let zenith = cos_zenith.acos();
    let elevation = 90.0 - zenith * to_deg;
    let azimuth = (hour_angle
        .sin()
        .atan2(hour_angle.cos() * lat.sin() - declination.tan() * lat.cos())
        * to_deg
        + 180.0)
        .rem_euclid(360.0);
    (elevation as f32, azimuth as f32)
}

/// Everything the sky shader needs for one frame of analytic sky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyFrame {
    /// Perez A,B,C,D for the Y (luminance) channel.
    pub pz_y: Vec4f,
    /// Perez A,B,C,D for the x chromaticity channel.
    pub pz_x: Vec4f,
    /// Perez A,B,C,D for the y chromaticity channel.
    pub pz_yc: Vec4f,
    /// Perez E per channel: (E_Y, E_x, E_y, 0).
    pub pz_e: Vec4f,
    /// 1 / F(0, theta_s) per channel: (Y, x, y, 0).
    pub pz_f0: Vec4f,
    /// Zenith values (Y_z, x_z, y_z) and the night blend in w (0 = full
    /// day, 1 = full night).
    pub zenith: Vec4f,
    /// Unit MODEL sun direction (y up), w = exposure for the tone-map.
    /// Clamped at ~2 degrees elevation — the Perez normaliser diverges
    /// below; use only for the model's circumsolar terms.
    pub sun: Vec4f,
    /// The TRUE unit sun direction, unclamped — the visible disc, Mie
    /// glow and afterglow follow THIS one below the horizon (painting
    /// them around the clamped direction froze the setting sun ~2
    /// degrees up while the fades played out).
    pub sun_true: Vec4f,
    /// The model's horizon colour, tone-mapped, averaged around the compass
    /// — the FOG colour that matches this sky (fog is one colour, the
    /// horizon is not; the average is the only seam-free choice).
    pub fog_rgb: Vec3f,
}

/// One Perez channel's five coefficients from turbidity `t`.
fn perez(t: f32, rows: [[f32; 2]; 5]) -> [f32; 5] {
    [
        rows[0][0] * t + rows[0][1],
        rows[1][0] * t + rows[1][1],
        rows[2][0] * t + rows[2][1],
        rows[3][0] * t + rows[3][1],
        rows[4][0] * t + rows[4][1],
    ]
}

/// The published coefficient tables (Preetham et al., appendix).
const PEREZ_Y: [[f32; 2]; 5] = [
    [0.1787, -1.4630],
    [-0.3554, 0.4275],
    [-0.0227, 5.3251],
    [0.1206, -2.5771],
    [-0.0670, 0.3703],
];
const PEREZ_X: [[f32; 2]; 5] = [
    [-0.0193, -0.2592],
    [-0.0665, 0.0008],
    [-0.0004, 0.2125],
    [-0.0641, -0.8989],
    [-0.0033, 0.0452],
];
const PEREZ_YC: [[f32; 2]; 5] = [
    [-0.0167, -0.2608],
    [-0.0950, 0.0092],
    [-0.0079, 0.2102],
    [-0.0441, -1.6537],
    [-0.0109, 0.0529],
];

/// Zenith chromaticity: cubic in theta_s, quadratic in turbidity (the
/// published 3x4 matrices).
fn zenith_chroma(t: f32, ths: f32, m: [[f32; 4]; 3]) -> f32 {
    let (t2, th2, th3) = (t * t, ths * ths, ths * ths * ths);
    let row = |r: [f32; 4]| r[0] * th3 + r[1] * th2 + r[2] * ths + r[3];
    t2 * row(m[0]) + t * row(m[1]) + row(m[2])
}

const ZENITH_X: [[f32; 4]; 3] = [
    [0.00166, -0.00375, 0.00209, 0.0],
    [-0.02903, 0.06377, -0.03202, 0.00394],
    [0.11693, -0.21196, 0.06052, 0.25886],
];
const ZENITH_YC: [[f32; 4]; 3] = [
    [0.00275, -0.00610, 0.00317, 0.0],
    [-0.04214, 0.08970, -0.04153, 0.00516],
    [0.15346, -0.26756, 0.06670, 0.26688],
];

/// F(theta, gamma) for one channel.
fn perez_f(c: &[f32; 5], cos_theta: f32, gamma: f32, cos_gamma: f32) -> f32 {
    (1.0 + c[0] * (c[1] / cos_theta.max(0.01)).exp())
        * (1.0 + c[2] * (c[3] * gamma).exp() + c[4] * cos_gamma * cos_gamma)
}

/// Yxy -> display RGB: Reinhard on the LUMINANCE (chromaticity survives —
/// per-channel mapping washed the zenith grey), XYZ -> linear sRGB, a
/// hue-preserving normalise where a saturated blue pushes one channel past
/// 1, and the 1/2.2 display gamma (the engine authors every other colour in
/// display space). The shader carries the identical math; this copy exists
/// so the fog colour goes through the same path.
fn yxy_to_rgb(y_lum: f32, x: f32, yc: f32, exposure: f32) -> Vec3f {
    let yc = yc.max(1.0e-4);
    let yt = (y_lum * exposure).max(0.0);
    let yt = yt / (1.0 + yt);
    let big_x = x * (yt / yc);
    let big_z = (1.0 - x - yc) * (yt / yc);
    let r = (3.2406 * big_x - 1.5372 * yt - 0.4986 * big_z).max(0.0);
    let g = (-0.9689 * big_x + 1.8758 * yt + 0.0415 * big_z).max(0.0);
    let b = (0.0557 * big_x - 0.2040 * yt + 1.0570 * big_z).max(0.0);
    let m = r.max(g).max(b).max(1.0);
    vec3f(
        (r / m).powf(1.0 / 2.2),
        (g / m).powf(1.0 / 2.2),
        (b / m).powf(1.0 / 2.2),
    )
}

/// Build one frame of sky parameters from the sun direction (y up, unit)
/// and turbidity. `exposure` scales the model's absolute luminance into the
/// display range — 0.12 lands the default noon near the hand-painted sky's
/// brightness.
pub fn preetham_frame(sun_dir: Vec3f, turbidity: f32, exposure: f32) -> SkyFrame {
    let t = turbidity.clamp(1.2, 10.0);
    // Clamp the MODEL's sun at ~2 degrees elevation: below that the Perez
    // normaliser diverges. The true elevation still drives the night blend,
    // so the visible sky keeps darkening smoothly through sunset.
    let el = sun_dir.y.clamp(-1.0, 1.0).asin();
    let el_model = el.max(0.035);
    let ths = std::f32::consts::FRAC_PI_2 - el_model;
    // The engine's visible dusk is deliberately compact: start fading as
    // the disc meets the horizon and reach the unpolluted night dome at the
    // end of civil twilight (-6 degrees). Keeping a Perez sunset fraction
    // until astronomical twilight leaves even a small remainder brighter
    // than the entire night sky, so midnight reads as golden hour. This is
    // a smooth angular ramp, never a hard elevation clamp.
    let night = {
        let night_end = -6.0f32.to_radians();
        let t = ((0.02 - el) / (0.02 - night_end)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };

    let y5 = perez(t, PEREZ_Y);
    let x5 = perez(t, PEREZ_X);
    let yc5 = perez(t, PEREZ_YC);

    // Zenith luminance (kcd/m²).
    let chi = (4.0 / 9.0 - t / 120.0) * (std::f32::consts::PI - 2.0 * ths);
    let zen_y = ((4.0453 * t - 4.9710) * chi.tan() - 0.2155 * t + 2.4192).max(0.001);
    let zen_x = zenith_chroma(t, ths, ZENITH_X);
    let zen_yc = zenith_chroma(t, ths, ZENITH_YC);

    let cos_ths = ths.cos();
    let f0 = |c: &[f32; 5]| 1.0 / perez_f(c, 1.0, ths, cos_ths);
    let (f0_y, f0_x, f0_yc) = (f0(&y5), f0(&x5), f0(&yc5));

    // The model's sun for the shader: the clamped-elevation direction, so
    // shader gamma agrees with the CPU normaliser at grazing sun.
    let horiz = vec2f(sun_dir.x, sun_dir.z);
    let hl = (horiz.x * horiz.x + horiz.y * horiz.y).sqrt().max(1.0e-5);
    let (ce, se) = (el_model.cos(), el_model.sin());
    let model_sun = vec3f(horiz.x / hl * ce, se, horiz.y / hl * ce);

    // Fog: the tone-mapped model at 2 degrees above the horizon, averaged
    // over 8 compass directions, faded toward night.
    let mut fog = vec3f(0.0, 0.0, 0.0);
    let vth = std::f32::consts::FRAC_PI_2 - 0.035;
    let (vc, vs) = (vth.cos(), vth.sin());
    for k in 0..8 {
        let az = k as f32 / 8.0 * std::f32::consts::TAU;
        let v = vec3f(az.cos() * vs, vc, az.sin() * vs);
        let cg = (v.x * model_sun.x + v.y * model_sun.y + v.z * model_sun.z).clamp(-1.0, 1.0);
        let g = cg.acos();
        let f = |c: &[f32; 5], zen: f32, inv: f32| zen * perez_f(c, vc, g, cg) * inv;
        let rgb = yxy_to_rgb(
            f(&y5, zen_y, f0_y),
            f(&x5, zen_x, f0_x),
            f(&yc5, zen_yc, f0_yc),
            exposure,
        );
        fog = fog + rgb;
    }
    fog = fog * (1.0 / 8.0);
    let night_fog = vec3f(0.05, 0.06, 0.09);
    fog = fog + (night_fog - fog) * night;

    SkyFrame {
        pz_y: vec4(y5[0], y5[1], y5[2], y5[3]),
        pz_x: vec4(x5[0], x5[1], x5[2], x5[3]),
        pz_yc: vec4(yc5[0], yc5[1], yc5[2], yc5[3]),
        pz_e: vec4(y5[4], x5[4], yc5[4], 0.0),
        pz_f0: vec4(f0_y, f0_x, f0_yc, 0.0),
        zenith: vec4(zen_y, zen_x, zen_yc, night),
        sun: vec4(model_sun.x, model_sun.y, model_sun.z, exposure),
        sun_true: {
            let d = sun_dir.normalize();
            vec4(d.x, d.y, d.z, 0.0)
        },
        fog_rgb: fog,
    }
}

/// Texture-free CPU transcription of [`crate::shaders::DrawSceneSkyAnalytic`].
///
/// The game can layer its star panorama over this result. Callers without
/// that asset still get the shader's two deterministic point-star layers;
/// framebuffer dithering is deliberately omitted from this reference.
pub fn analytic_sky_rgb(
    frame: &SkyFrame,
    direction: Vec3f,
    star_rows: [Vec4f; 3],
) -> Vec3f {
    let v = direction.normalize();
    let cos_theta = v.y.max(0.01);
    let model_sun = vec3f(frame.sun.x, frame.sun.y, frame.sun.z);
    let cos_gamma = v.dot(model_sun).clamp(-1.0, 1.0);
    let gamma = cos_gamma.acos();
    let eval = |coefficients: Vec4f, e: f32| {
        (1.0 + coefficients.x * (coefficients.y / cos_theta).exp())
            * (1.0
                + coefficients.z * (coefficients.w * gamma).exp()
                + e * cos_gamma * cos_gamma)
    };
    let luminance = frame.zenith.x * eval(frame.pz_y, frame.pz_e.x) * frame.pz_f0.x;
    let chroma_x = frame.zenith.y * eval(frame.pz_x, frame.pz_e.y) * frame.pz_f0.y;
    let chroma_y =
        (frame.zenith.z * eval(frame.pz_yc, frame.pz_e.z) * frame.pz_f0.z).max(0.0001);
    let mut day = yxy_to_rgb(luminance, chroma_x, chroma_y, frame.sun.w);
    day = day * (1.0 - 0.65 * (-v.y * 3.0).clamp(0.0, 1.0));

    let true_sun = vec3f(frame.sun_true.x, frame.sun_true.y, frame.sun_true.z);
    let sun_gamma = v.dot(true_sun).clamp(-1.0, 1.0).acos();
    let extinction = |height: f32, floor: f32| {
        let depth = -0.485 / (height + 0.033).max(floor).powf(0.75);
        vec3f(
            (0.39 * depth).exp(),
            (0.57 * depth).exp(),
            depth.exp(),
        ) * 2.0
    };
    let view_absorption = extinction(v.y, 0.02);
    let sun_absorption = extinction(true_sun.y, 0.012);
    let limb = 1.0 - smoothstep(0.048, 0.055, sun_gamma);
    let mie_d = (1.0 - (sun_gamma * 0.55).powf(0.1)).clamp(0.0, 1.0);
    let mie = mie_d * mie_d * (3.0 - 2.0 * mie_d) * 1.4;
    day = day
        + (view_absorption * (limb * 20.0) + sun_absorption * mie)
            * ((v.y + 0.033) * 90.0 + 0.5).clamp(0.0, 1.0);

    let night_blend = frame.zenith.w;
    let night = vec3f(0.010, 0.012, 0.020)
        + (vec3f(0.002, 0.003, 0.006) - vec3f(0.010, 0.012, 0.020))
            * (v.y * 1.4).clamp(0.0, 1.0);
    let mut result = day + (night - day) * night_blend;

    if night_blend > 0.0 && v.y > -1.0 / 60.0 {
        let row = |r: Vec4f| r.x * v.x + r.y * v.y + r.z * v.z;
        let sx = row(star_rows[0]);
        let sy = row(star_rows[1]).clamp(-1.0, 1.0);
        let sz = row(star_rows[2]);
        let u = sz.atan2(sx) / std::f32::consts::TAU + 0.5;
        let vv = 0.5 - sy.asin() / std::f32::consts::PI;
        let point_layer = |width: f32,
                           height: f32,
                           hx: f32,
                           hy: f32,
                           keep: f32,
                           radius: f32,
                           power: f32| {
            let gx = u * width;
            let gy = vv * height;
            let ix = gx.floor();
            let iy = gy.floor();
            let hash = fract((ix * hx + iy * hy).sin() * 43_758.5453);
            let dx = fract(gx) - 0.5;
            let dy = fract(gy) - 0.5;
            let point = (1.0 - (dx * dx + dy * dy).sqrt() * radius)
                .clamp(0.0, 1.0)
                .powf(power);
            (if hash >= keep { 1.0 } else { 0.0 }, point, hash)
        };
        let (keep0, point0, hash0) =
            point_layer(1600.0, 800.0, 127.1, 311.7, 0.995, 2.0, 3.0);
        let spark = keep0 * point0 * (0.3 + 0.7 * fract(hash0 * 57.31));
        let (keep1, point1, hash1) =
            point_layer(400.0, 200.0, 269.5, 183.3, 0.992, 2.4, 4.0);
        let spark2 = keep1 * point1 * (0.5 + 0.5 * fract(hash1 * 43.7));
        let fade = night_blend * (v.y * 6.0 + 0.1).clamp(0.0, 1.0);
        result = result
            + (vec3f(0.85, 0.9, 1.0) * spark
                + vec3f(1.0, 0.97, 0.9) * spark2)
                * fade;
    }
    result
}

/// Colour of DIRECT sunlight after the atmosphere, for a sun height `sun_y`
/// (= sine of elevation, y-up): the same Beer-Lambert extinction curve the
/// analytic sky evaluates for its visible disc and Mie glow
/// (`DrawSceneSkyAnalytic`'s `abss` term, [`analytic_sky_rgb`]'s
/// `extinction`), normalised at the overhead sun so a noon rig keeps its
/// authored colour. The engine deliberately has no second reddening curve:
/// the gold that reaches a west facade at dusk is the gold the sky paints
/// around the disc, so surface tint and sky gradient can never disagree.
pub fn sun_transmittance(sun_y: f32) -> Vec3f {
    let optical = |y: f32| -0.485 / (y + 0.033).max(0.012).powf(0.75);
    let depth = optical(sun_y.clamp(-1.0, 1.0)) - optical(1.0);
    vec3f(
        (0.39 * depth).exp(),
        (0.57 * depth).exp(),
        depth.exp(),
    )
}

/// Linear-radiance sky shared by the realtime renderer and ray tracer.
///
/// The fields are also the exact uniform lanes evaluated by both GPU
/// shaders. The older display-space [`SkyFrame`] and [`preetham_frame`] stay
/// available for callers that use that API directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharedSkyFrame {
    pub pz_y: Vec4f,
    pub pz_x: Vec4f,
    pub pz_yc: Vec4f,
    pub pz_e: Vec4f,
    /// (1/F_Y, 1/F_x, 1/F_y, below-horizon dimming).
    pub pz_f0: Vec4f,
    /// Zenith Yxy; w is unused.
    pub zenith: Vec4f,
    /// Clamped Perez sun direction and daylight scale.
    pub sun_model: Vec4f,
    /// True sun direction and cos(SUN_RADIUS).
    pub sun_dir: Vec4f,
    /// Linear RGB solar-disc radiance.
    pub sun_radiance: Vec4f,
    /// World up; w is one while the solar disc is above the horizon.
    pub up: Vec4f,
    /// Stable catalogue east.
    pub star_east: Vec4f,
    /// (daylight weight, twilight weight, star fade, sun elevation radians).
    pub blend: Vec4f,
    /// Mean upper-dome RGB and luminance in w, excluding the tiny sun disc.
    pub mean: Vec4f,
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn linear_rgb(y_lum: f32, x: f32, yc: f32) -> Vec3f {
    let yc = yc.max(1.0e-4);
    let y_lum = y_lum.max(0.0);
    let big_x = x * (y_lum / yc);
    let big_z = (1.0 - x - yc) * (y_lum / yc);
    vec3f(
        (3.2406 * big_x - 1.5372 * y_lum - 0.4986 * big_z).max(0.0),
        (-0.9689 * big_x + 1.8758 * y_lum + 0.0415 * big_z).max(0.0),
        (0.0557 * big_x - 0.2040 * y_lum + 1.0570 * big_z).max(0.0),
    )
}

fn basis(up: Vec3f) -> (Vec3f, Vec3f) {
    let reference = if up.z.abs() < 0.9 {
        vec3f(0.0, 0.0, 1.0)
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    let east = Vec3f::cross(reference, up).normalize();
    (east, Vec3f::cross(up, east).normalize())
}

fn fract(value: f32) -> f32 {
    value - value.floor()
}

/// Sparse deterministic catalogue. It is exactly dark until the sun passes
/// below civil twilight (-6 degrees).
pub fn star_radiance(frame: &SharedSkyFrame, direction: Vec3f) -> Vec3f {
    if frame.blend.z <= 0.0 {
        return Vec3f::default();
    }
    let view = direction.normalize();
    let up = vec3f(frame.up.x, frame.up.y, frame.up.z);
    let altitude = view.dot(up);
    if altitude <= 0.0 {
        return Vec3f::default();
    }
    let east = vec3f(frame.star_east.x, frame.star_east.y, frame.star_east.z);
    let north = Vec3f::cross(up, east).normalize();
    let catalogue = vec3f(view.dot(east), altitude, view.dot(north));
    let u = catalogue.z.atan2(catalogue.x) / std::f32::consts::TAU + 0.5;
    let v = catalogue.y.clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5;
    let gx = u * 2048.0;
    let gy = v * 1024.0;
    let ix = gx.floor();
    let iy = gy.floor();
    let hash = fract((ix * 127.1 + iy * 311.7).sin() * 43_758.547);
    if hash < 0.9985 {
        return Vec3f::default();
    }
    let dx = fract(gx) - 0.5;
    let dy = fract(gy) - 0.5;
    let point = (1.0 - (dx * dx + dy * dy).sqrt() * 5.0)
        .clamp(0.0, 1.0)
        .powi(6);
    let brightness = 0.025 + 0.075 * fract(hash * 57.31);
    let horizon = smoothstep(0.0, 0.08, altitude);
    vec3f(0.82, 0.89, 1.0) * (point * brightness * frame.blend.z * horizon)
}

fn radiance_without_stars(frame: &SharedSkyFrame, direction: Vec3f) -> Vec3f {
    let view = direction.normalize();
    let up = vec3f(frame.up.x, frame.up.y, frame.up.z);
    let altitude = view.dot(up);
    let cos_theta = altitude.max(0.01);
    let model_sun = vec3f(frame.sun_model.x, frame.sun_model.y, frame.sun_model.z);
    let cos_gamma = view.dot(model_sun).clamp(-1.0, 1.0);
    let gamma = cos_gamma.acos();
    let eval = |coefficients: Vec4f, e: f32| {
        (1.0 + coefficients.x * (coefficients.y / cos_theta).exp())
            * (1.0
                + coefficients.z * (coefficients.w * gamma).exp()
                + e * cos_gamma * cos_gamma)
    };
    let luminance = frame.zenith.x * eval(frame.pz_y, frame.pz_e.x) * frame.pz_f0.x;
    let chroma_x = frame.zenith.y * eval(frame.pz_x, frame.pz_e.y) * frame.pz_f0.y;
    let chroma_y = frame.zenith.z * eval(frame.pz_yc, frame.pz_e.z) * frame.pz_f0.z;
    let below = (-altitude * 3.0).clamp(0.0, 1.0);
    let day = linear_rgb(luminance * frame.sun_model.w, chroma_x, chroma_y)
        * ((1.0 - below * frame.pz_f0.w) * frame.blend.x);

    let sky_height = altitude.max(0.0).sqrt();
    let night = (NIGHT_HORIZON + (NIGHT_ZENITH - NIGHT_HORIZON) * sky_height)
        * (1.0 - frame.blend.x)
        * (1.0 - 0.55 * below);

    let true_sun = vec3f(frame.sun_dir.x, frame.sun_dir.y, frame.sun_dir.z);
    let sun_gamma = view.dot(true_sun).clamp(-1.0, 1.0).acos();
    let horizon_band = (-altitude.abs() * 7.0).exp();
    let circumsolar = (-sun_gamma * 3.5).exp();
    let dusk = (vec3f(0.010, 0.018, 0.045) * (0.35 + 0.65 * sky_height)
        + vec3f(0.10, 0.032, 0.006) * (horizon_band * circumsolar))
        * frame.blend.y;
    day + night + dusk
}

/// Linear sky radiance including twilight and stars, excluding the solar
/// disc (which is sampled separately for next-event estimation).
pub fn radiance(frame: &SharedSkyFrame, direction: Vec3f) -> Vec3f {
    radiance_without_stars(frame, direction) + star_radiance(frame, direction)
}

/// Environment radiance with an optional visible solar disc.
pub fn environment_radiance(
    frame: &SharedSkyFrame,
    direction: Vec3f,
    include_sun: bool,
) -> Vec3f {
    let view = direction.normalize();
    let mut result = radiance(frame, view);
    if include_sun
        && frame.up.w > 0.5
        && view.dot(vec3f(frame.sun_dir.x, frame.sun_dir.y, frame.sun_dir.z))
            >= frame.sun_dir.w
    {
        result = result
            + vec3f(
                frame.sun_radiance.x,
                frame.sun_radiance.y,
                frame.sun_radiance.z,
            );
    }
    result
}

fn sample_mean(frame: &SharedSkyFrame) -> Vec3f {
    let up = vec3f(frame.up.x, frame.up.y, frame.up.z);
    let (east, north) = basis(up);
    let mut sum = Vec3f::default();
    const HEIGHT_SAMPLES: usize = 4;
    const AZIMUTH_SAMPLES: usize = 8;
    for height in 0..HEIGHT_SAMPLES {
        let z = (height as f32 + 0.5) / HEIGHT_SAMPLES as f32;
        let radius = (1.0 - z * z).sqrt();
        for azimuth in 0..AZIMUTH_SAMPLES {
            let phi = std::f32::consts::TAU * (azimuth as f32 + 0.5)
                / AZIMUTH_SAMPLES as f32;
            let view = east * (radius * phi.cos())
                + north * (radius * phi.sin())
                + up * z;
            sum = sum + radiance_without_stars(frame, view);
        }
    }
    sum * (1.0 / (HEIGHT_SAMPLES * AZIMUTH_SAMPLES) as f32)
}

/// Construct the one production sky frame in any caller-provided coordinate
/// system. `star_east` fixes the deterministic catalogue orientation.
pub fn sky_frame(
    sun_dir: Vec3f,
    up: Vec3f,
    star_east: Vec3f,
    turbidity: f32,
    sky_strength: f32,
    sun_strength: f32,
) -> SharedSkyFrame {
    let up = up.normalize();
    let sun_dir = sun_dir.normalize();
    let star_east = {
        let horizontal = star_east - up * star_east.dot(up);
        if horizontal.length() > 1.0e-5 {
            horizontal.normalize()
        } else {
            basis(up).0
        }
    };
    let turbidity = turbidity.clamp(1.2, 10.0);
    let elevation = sun_dir.dot(up).clamp(-1.0, 1.0).asin();
    let elevation_deg = elevation.to_degrees();
    let model_elevation = elevation.max(0.035);
    let theta_s = std::f32::consts::FRAC_PI_2 - model_elevation;
    let day = smoothstep(-12.0, 0.0, elevation_deg);
    let twilight = smoothstep(-12.0, -3.0, elevation_deg)
        * (1.0 - smoothstep(2.0, 8.0, elevation_deg));
    let stars = smoothstep(6.0, 12.0, -elevation_deg);

    let y5 = perez(turbidity, PEREZ_Y);
    let x5 = perez(turbidity, PEREZ_X);
    let yc5 = perez(turbidity, PEREZ_YC);
    let chi = (4.0 / 9.0 - turbidity / 120.0)
        * (std::f32::consts::PI - 2.0 * theta_s);
    let zenith_y = ((4.0453 * turbidity - 4.9710) * chi.tan()
        - 0.2155 * turbidity
        + 2.4192)
        .max(0.001);
    let zenith_x = zenith_chroma(turbidity, theta_s, ZENITH_X);
    let zenith_yc = zenith_chroma(turbidity, theta_s, ZENITH_YC);
    let cos_theta_s = theta_s.cos();
    let normalizer = |coefficients: &[f32; 5]| {
        1.0 / perez_f(coefficients, 1.0, theta_s, cos_theta_s)
    };

    let horizontal = sun_dir - up * sun_dir.dot(up);
    let model_sun = if horizontal.length() > 1.0e-5 {
        horizontal.normalize() * model_elevation.cos() + up * model_elevation.sin()
    } else {
        up * model_elevation.sin() + basis(up).0 * model_elevation.cos()
    };

    let solid_angle = std::f32::consts::TAU * (1.0 - SUN_RADIUS.cos());
    let sun_visible = elevation > -SUN_RADIUS && sun_strength > 0.0;
    let sun_radiance = if sun_visible {
        let air_mass = 1.0
            / (elevation.sin().max(0.0)
                + 0.15 * (elevation_deg.max(0.0) + 3.885).powf(-1.253));
        let beta = vec3f(0.035, 0.06, 0.12) * (0.4 + 0.15 * turbidity);
        let tint = vec3f(
            (-beta.x * air_mass).exp(),
            (-beta.y * air_mass).exp(),
            (-beta.z * air_mass).exp(),
        );
        tint * (std::f32::consts::PI * (sun_strength / 4.0) / solid_angle)
    } else {
        Vec3f::default()
    };

    let mut frame = SharedSkyFrame {
        pz_y: vec4f(y5[0], y5[1], y5[2], y5[3]),
        pz_x: vec4f(x5[0], x5[1], x5[2], x5[3]),
        pz_yc: vec4f(yc5[0], yc5[1], yc5[2], yc5[3]),
        pz_e: vec4f(y5[4], x5[4], yc5[4], 0.0),
        pz_f0: vec4f(
            normalizer(&y5),
            normalizer(&x5),
            normalizer(&yc5),
            0.65,
        ),
        zenith: vec4f(zenith_y, zenith_x, zenith_yc, 0.0),
        sun_model: vec4f(
            model_sun.x,
            model_sun.y,
            model_sun.z,
            DAYLIGHT_SCALE * sky_strength.max(0.0),
        ),
        sun_dir: vec4f(sun_dir.x, sun_dir.y, sun_dir.z, SUN_RADIUS.cos()),
        sun_radiance: vec4f(sun_radiance.x, sun_radiance.y, sun_radiance.z, 0.0),
        up: vec4f(
            up.x,
            up.y,
            up.z,
            if sun_visible { 1.0 } else { 0.0 },
        ),
        star_east: vec4f(star_east.x, star_east.y, star_east.z, 0.0),
        blend: vec4f(day, twilight, stars, elevation),
        mean: Vec4f::default(),
    };
    let mean = sample_mean(&frame);
    frame.mean = vec4f(mean.x, mean.y, mean.z, luminance(mean));
    frame
}

/// Test/furnace environment that uses the shared frame ABI.
pub fn uniform_white(radiance_value: f32, up: Vec3f) -> SharedSkyFrame {
    let up = up.normalize();
    let east = basis(up).0;
    let mut frame = SharedSkyFrame {
        pz_y: Vec4f::default(),
        pz_x: Vec4f::default(),
        pz_yc: Vec4f::default(),
        pz_e: Vec4f::default(),
        pz_f0: vec4f(1.0, 1.0, 1.0, 0.0),
        zenith: vec4f(radiance_value, 0.3127, 0.3290, 0.0),
        sun_model: vec4f(0.0, 1.0, 0.0, 1.0),
        sun_dir: vec4f(0.0, 1.0, 0.0, SUN_RADIUS.cos()),
        sun_radiance: Vec4f::default(),
        up: vec4f(up.x, up.y, up.z, 0.0),
        star_east: vec4f(east.x, east.y, east.z, 0.0),
        blend: vec4f(1.0, 0.0, 0.0, 0.0),
        mean: vec4f(
            radiance_value,
            radiance_value,
            radiance_value,
            radiance_value,
        ),
    };
    let mean = radiance(&frame, up);
    frame.mean = vec4f(mean.x, mean.y, mean.z, luminance(mean));
    frame
}

pub fn luminance(rgb: Vec3f) -> f32 {
    0.2126 * rgb.x + 0.7152 * rgb.y + 0.0722 * rgb.z
}

/// Reflected-light meter: expose the mean dome toward 12% grey. The floor is
/// in the meter only, so it cannot lift a black pixel.
pub fn exposure(frame: &SharedSkyFrame, compensation_ev: f32) -> f32 {
    let metered = frame.mean.w.max(EXPOSURE_LUMINANCE_FLOOR);
    (EXPOSURE_KEY / metered) * 2.0f32.powf(compensation_ev.clamp(-12.0, 12.0))
}

/// Solar-disc solid angle times radiance: the direct irradiance used by NEE
/// and by the realtime directional-light approximation.
pub fn sun_irradiance(frame: &SharedSkyFrame) -> Vec3f {
    let solid_angle = std::f32::consts::TAU * (1.0 - frame.sun_dir.w);
    vec3f(
        frame.sun_radiance.x,
        frame.sun_radiance.y,
        frame.sun_radiance.z,
    ) * solid_angle
}

fn aces_channel(value: f32) -> f32 {
    (value * (2.51 * value + 0.03) / (value * (2.43 * value + 0.59) + 0.14))
        .clamp(0.0, 1.0)
}

/// ACES-fit plus display gamma, matching the realtime and tracer output
/// shaders.
pub fn display_rgb(linear: Vec3f, exposure_value: f32) -> Vec3f {
    let color = linear * exposure_value;
    vec3f(
        aces_channel(color.x).powf(1.0 / 2.2),
        aces_channel(color.y).powf(1.0 / 2.2),
        aces_channel(color.z).powf(1.0 / 2.2),
    )
}

/// Seam-free fog colour from eight shared-model samples above the horizon.
pub fn shared_fog_rgb(frame: &SharedSkyFrame, exposure_value: f32) -> Vec3f {
    let up = vec3f(frame.up.x, frame.up.y, frame.up.z);
    let (east, north) = basis(up);
    let mut sum = Vec3f::default();
    let z = 0.035f32.sin();
    let radius = (1.0 - z * z).sqrt();
    for sample in 0..8 {
        let phi = std::f32::consts::TAU * (sample as f32 + 0.5) / 8.0;
        sum = sum
            + radiance(
                frame,
                east * (radius * phi.cos()) + north * (radius * phi.sin()) + up * z,
            );
    }
    display_rgb(sum * 0.125, exposure_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_at(elev_deg: f32) -> SkyFrame {
        let e = elev_deg.to_radians();
        preetham_frame(vec3f(e.cos(), e.sin(), 0.0), 2.5, 0.12)
    }

    /// Evaluate the full model CPU-side for a view direction — the test's
    /// mirror of the shader math.
    fn sky_rgb(f: &SkyFrame, v: Vec3f) -> Vec3f {
        let ct = v.y.max(0.01);
        let cg = (v.x * f.sun.x + v.y * f.sun.y + v.z * f.sun.z).clamp(-1.0, 1.0);
        let g = cg.acos();
        let ch = |abcd: Vec4f, e: f32, zen: f32, inv: f32| {
            zen * perez_f(&[abcd.x, abcd.y, abcd.z, abcd.w, e], ct, g, cg) * inv
        };
        yxy_to_rgb(
            ch(f.pz_y, f.pz_e.x, f.zenith.x, f.pz_f0.x),
            ch(f.pz_x, f.pz_e.y, f.zenith.y, f.pz_f0.y),
            ch(f.pz_yc, f.pz_e.z, f.zenith.z, f.pz_f0.z),
            f.sun.w,
        )
    }

    #[test]
    fn noon_zenith_is_blue_and_not_blown_out() {
        let f = frame_at(60.0);
        let up = sky_rgb(&f, vec3f(0.0, 1.0, 0.0));
        assert!(up.z > up.x, "zenith should lean blue: {up:?}");
        assert!(up.z > 0.25 && up.z < 0.98, "zenith blue in range: {up:?}");
        assert!(f.zenith.w == 0.0, "no night blend at noon");
    }

    #[test]
    fn horizon_is_brighter_than_zenith_in_daylight() {
        let f = frame_at(45.0);
        let up = sky_rgb(&f, vec3f(0.0, 1.0, 0.0));
        let hz = sky_rgb(&f, vec3f(0.0, 0.05, -1.0).normalize());
        let lum = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        assert!(
            lum(hz) > lum(up),
            "haze brightens the horizon: hz {hz:?} up {up:?}"
        );
    }

    #[test]
    fn circumsolar_glow_beats_the_opposite_sky() {
        let f = frame_at(25.0);
        let sun = vec3f(f.sun.x, f.sun.y, f.sun.z);
        let near = (sun + vec3f(0.0, 0.12, 0.0)).normalize();
        let away = vec3f(-sun.x, sun.y, -sun.z).normalize();
        let lum = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        assert!(lum(sky_rgb(&f, near)) > lum(sky_rgb(&f, away)));
    }

    #[test]
    fn sunset_reddens_the_sun_side_horizon() {
        let f = frame_at(3.0);
        let sun = vec3f(f.sun.x, f.sun.y, f.sun.z);
        let hz = vec3f(sun.x, 0.05, sun.z).normalize();
        let c = sky_rgb(&f, hz);
        assert!(c.x > c.z, "sunset horizon leans red: {c:?}");
    }

    #[test]
    fn night_blend_rises_below_the_horizon() {
        assert_eq!(frame_at(30.0).zenith.w, 0.0);
        // Dusk is a smooth hand-off, not a snap at the horizon.
        let dusk = frame_at(-2.0);
        assert!(dusk.zenith.w > 0.2 && dusk.zenith.w < 0.7, "{}", dusk.zenith.w);
        // At the end of civil twilight the bright Perez horizon is gone;
        // later hours therefore contain only the night dome and stars.
        let night = frame_at(-6.1);
        assert!(night.zenith.w == 1.0, "{}", night.zenith.w);
        // Fog follows: night fog is the dark blue floor.
        assert!(night.fog_rgb.z < 0.15 && night.fog_rgb.z > night.fog_rgb.x);
    }

    #[test]
    fn coefficients_are_finite_across_the_whole_day() {
        for deg in -90..=90 {
            let f = frame_at(deg as f32);
            for v in [f.pz_y, f.pz_x, f.pz_yc, f.pz_e, f.pz_f0, f.zenith, f.sun] {
                assert!(
                    v.x.is_finite() && v.y.is_finite() && v.z.is_finite() && v.w.is_finite(),
                    "non-finite at {deg}: {v:?}"
                );
            }
            let c = sky_rgb(&f, vec3f(0.3, 0.4, -0.5).normalize());
            assert!(c.x.is_finite() && c.y.is_finite() && c.z.is_finite());
            assert!(c.x <= 1.0 && c.y <= 1.0 && c.z <= 1.0, "tone-mapped: {c:?}");
        }
    }
}

#[cfg(test)]
mod shared_tests {
    use super::*;

    fn frame(elevation_deg: f32) -> SharedSkyFrame {
        let elevation = elevation_deg.to_radians();
        sky_frame(
            vec3f(elevation.cos(), elevation.sin(), 0.0),
            vec3f(0.0, 1.0, 0.0),
            vec3f(1.0, 0.0, 0.0),
            2.5,
            1.0,
            4.0,
        )
    }

    #[test]
    fn stars_start_strictly_after_civil_twilight() {
        let civil = frame(-6.0);
        assert_eq!(frame(-5.99).blend.z, 0.0);
        assert_eq!(civil.blend.z, 0.0);
        assert!(frame(-9.0).blend.z > 0.0);
        for y in 1..16 {
            for x in 0..32 {
                let phi = std::f32::consts::TAU * x as f32 / 32.0;
                let z = y as f32 / 16.0;
                let radius = (1.0 - z * z).sqrt();
                assert_eq!(
                    star_radiance(&civil, vec3f(radius * phi.cos(), z, radius * phi.sin())),
                    Vec3f::default()
                );
            }
        }
    }

    #[test]
    fn exposure_floor_is_a_meter_not_a_black_lift() {
        let night = frame(-20.0);
        assert!(exposure(&night, 0.0).is_finite());
        assert_eq!(Vec3f::default() * exposure(&night, 0.0), Vec3f::default());
    }

    #[test]
    fn day_twilight_and_night_are_ordered() {
        let noon = frame(60.0);
        let dusk = frame(-4.0);
        let night = frame(-18.0);
        assert_eq!(noon.blend.x, 1.0);
        assert!(dusk.blend.y > 0.0);
        assert_eq!(night.blend.x, 0.0);
        assert!(
            noon.mean.w > night.mean.w * 8.0,
            "{} vs {}",
            noon.mean.w,
            night.mean.w
        );
    }

    #[test]
    fn noaa_reference_includes_civil_offset() {
        let position = noaa_solar_position(
            SkyDate {
                year: 2024,
                month: 1,
                day: 1,
            },
            12.0,
            11.0,
            -33.8688,
            151.2093,
        );
        assert!((position.0 - 73.27).abs() <= 0.5, "{position:?}");
        let error = ((position.1 - 53.52 + 180.0).rem_euclid(360.0) - 180.0).abs();
        assert!(error <= 0.5, "{position:?}");
    }
}
