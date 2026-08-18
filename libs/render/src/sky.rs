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
    // Night blend: starts as the sun touches the horizon, complete at
    // ASTRONOMICAL twilight (-18 degrees — the real end of dusk; the old
    // linear-to--10 ramp went pitch black in half the true angular
    // distance). Smoothstepped: the first degrees after sunset keep
    // their glow (civil dusk at -6 is still bright) and the final fade
    // to black is gentle instead of a linear dump.
    let night = {
        let t = ((0.02 - el) / 0.334).clamp(0.0, 1.0);
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
        // Civil dusk (-2..-6 deg) keeps most of its glow — real twilight
        // lingers; the old linear ramp was already half black here.
        let dusk = frame_at(-2.0);
        assert!(dusk.zenith.w > 0.01 && dusk.zenith.w < 0.25, "{}", dusk.zenith.w);
        let civil = frame_at(-6.0);
        assert!(civil.zenith.w > 0.2 && civil.zenith.w < 0.55, "{}", civil.zenith.w);
        // Nautical twilight is well on the way to dark...
        let nautical = frame_at(-12.0);
        assert!(nautical.zenith.w > 0.6 && nautical.zenith.w < 0.97, "{}", nautical.zenith.w);
        // ...and full night arrives at ASTRONOMICAL twilight, -18 deg.
        let night = frame_at(-18.5);
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
