//! Texture-free path-tracer transcription of the engine's analytic sky.
//!
//! The realtime shader is the source of truth. This module keeps its own f32
//! implementation so the CPU reference and path-tracing shader can be tested
//! independently against that source.

use crate::scene::Sun;
use makepad_draw::*;

/// Angular radius used by the tracer's explicit direct-light sampler.
pub const SUN_RADIUS: f32 = 0.004_65;
const ENGINE_SKY_EXPOSURE: f32 = 0.1;

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

fn perez(turbidity: f32, rows: [[f32; 2]; 5]) -> [f32; 5] {
    rows.map(|row| row[0] * turbidity + row[1])
}

fn perez_f(coefficients: &[f32; 5], cos_theta: f32, gamma: f32, cos_gamma: f32) -> f32 {
    (1.0 + coefficients[0] * (coefficients[1] / cos_theta.max(0.01)).exp())
        * (1.0
            + coefficients[2] * (coefficients[3] * gamma).exp()
            + coefficients[4] * cos_gamma * cos_gamma)
}

fn zenith_chroma(turbidity: f32, theta_s: f32, matrix: [[f32; 4]; 3]) -> f32 {
    let t2 = turbidity * turbidity;
    let theta2 = theta_s * theta_s;
    let theta3 = theta2 * theta_s;
    let row = |r: [f32; 4]| r[0] * theta3 + r[1] * theta2 + r[2] * theta_s + r[3];
    t2 * row(matrix[0]) + turbidity * row(matrix[1]) + row(matrix[2])
}

fn smoothstep(a: f32, b: f32, value: f32) -> f32 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn fract(value: f32) -> f32 {
    value - value.floor()
}

/// Uniform lanes consumed by both tracer implementations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyUniforms {
    pub pz_y: Vec4f,
    pub pz_x: Vec4f,
    pub pz_yc: Vec4f,
    pub pz_e: Vec4f,
    pub pz_f0: Vec4f,
    /// Engine zenith Yxy and night blend in w.
    pub zenith: Vec4f,
    /// Clamped engine-model sun and embedded sky exposure in w.
    pub sun_model: Vec4f,
    /// True sun and cos of the direct-light sampler radius in w.
    pub sun_dir: Vec4f,
    /// Physical direct-light radiance, kept separate from the visible sky.
    pub sun_radiance: Vec4f,
    /// World up and direct-light enabled flag in w.
    pub up: Vec4f,
    /// World direction to celestial-catalogue rotation.
    pub star_r0: Vec4f,
    pub star_r1: Vec4f,
    pub star_r2: Vec4f,
    /// Overall environment multiplier.
    pub sky_strength: f32,
    /// Non-zero only for the furnace's uniform environment.
    pub uniform_value: f32,
}

fn celestial_rows_y_up(hours: f32, latitude_deg: f32) -> [Vec4f; 3] {
    let latitude = latitude_deg.to_radians();
    let pole = vec3f(0.0, latitude.sin(), -latitude.cos());
    let up = vec3f(0.0, 1.0, 0.0);
    let along = up.y * pole.y;
    let mut u = vec3f(-pole.x * along, up.y - pole.y * along, -pole.z * along);
    if u.length() < 1.0e-3 {
        u = vec3f(0.0, 0.0, -1.0);
        let projected = u.z * pole.z;
        u = vec3f(-pole.x * projected, -pole.y * projected, u.z - pole.z * projected);
    }
    let u = u.normalize();
    let w = Vec3f::cross(pole, u);
    let hour_angle = ((hours - 12.0) * 15.0).to_radians();
    let (cos_hour, sin_hour) = (hour_angle.cos(), hour_angle.sin());
    let x = u * cos_hour - w * sin_hour;
    let z = u * sin_hour + w * cos_hour;
    [
        vec4f(x.x, x.y, x.z, 0.0),
        vec4f(pole.x, pole.y, pole.z, 0.0),
        vec4f(z.x, z.y, z.z, 0.0),
    ]
}

fn celestial_rows(hours: f32, latitude_deg: f32, up: Vec3f) -> [Vec4f; 3] {
    let rows = celestial_rows_y_up(hours, latitude_deg);
    if up.z.abs() > up.y.abs() {
        // Inverse of Fab's (x, z, -y) turn into the engine's Y-up world.
        rows.map(|row| vec4f(row.x, -row.z, row.y, 0.0))
    } else {
        rows
    }
}

/// Build a sky with a stable noon celestial frame.
pub fn sky_uniforms(sun: &Sun, up: Vec3f) -> SkyUniforms {
    sky_uniforms_at_time(sun, up, 12.0, 0.0)
}

/// Build a sky whose star catalogue uses the same civil-hour rotation as the
/// engine renderer.
pub fn sky_uniforms_at_time(
    sun: &Sun,
    up: Vec3f,
    hours: f32,
    latitude_deg: f32,
) -> SkyUniforms {
    let up = up.normalize();
    let sun_dir = sun.dir.normalize();
    let turbidity = sun.turbidity.clamp(1.2, 10.0);
    let elevation = sun_dir.dot(up).clamp(-1.0, 1.0).asin();
    let model_elevation = elevation.max(0.035);
    let theta_s = std::f32::consts::FRAC_PI_2 - model_elevation;
    // Match the engine dome: the bright Perez sunset is fully gone at the
    // end of civil twilight instead of leaking into a moonless midnight.
    let night = smoothstep(0.02, -6.0f32.to_radians(), elevation);
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
        up * model_elevation.sin() + vec3f(1.0, 0.0, 0.0) * model_elevation.cos()
    };

    let solid_angle = std::f32::consts::TAU * (1.0 - SUN_RADIUS.cos());
    let sun_visible = elevation > 0.0 && sun.sun_strength > 0.0;
    let sun_radiance = if sun_visible {
        let elevation_deg = elevation.to_degrees();
        let air_mass = 1.0
            / (elevation.sin()
                + 0.15 * (elevation_deg + 3.885).max(0.1).powf(-1.253));
        let beta = vec3f(0.035, 0.06, 0.12) * (0.4 + 0.15 * turbidity);
        let tint = vec3f(
            (-beta.x * air_mass).exp(),
            (-beta.y * air_mass).exp(),
            (-beta.z * air_mass).exp(),
        );
        tint * (std::f32::consts::PI * (sun.sun_strength / 4.0) / solid_angle)
    } else {
        Vec3f::default()
    };
    let rows = celestial_rows(hours, latitude_deg, up);
    SkyUniforms {
        pz_y: vec4f(y5[0], y5[1], y5[2], y5[3]),
        pz_x: vec4f(x5[0], x5[1], x5[2], x5[3]),
        pz_yc: vec4f(yc5[0], yc5[1], yc5[2], yc5[3]),
        pz_e: vec4f(y5[4], x5[4], yc5[4], 0.0),
        pz_f0: vec4f(
            normalizer(&y5),
            normalizer(&x5),
            normalizer(&yc5),
            0.0,
        ),
        zenith: vec4f(zenith_y, zenith_x, zenith_yc, night),
        sun_model: vec4f(
            model_sun.x,
            model_sun.y,
            model_sun.z,
            ENGINE_SKY_EXPOSURE,
        ),
        sun_dir: vec4f(sun_dir.x, sun_dir.y, sun_dir.z, SUN_RADIUS.cos()),
        sun_radiance: vec4f(sun_radiance.x, sun_radiance.y, sun_radiance.z, 0.0),
        up: vec4f(up.x, up.y, up.z, if sun_visible { 1.0 } else { 0.0 }),
        star_r0: rows[0],
        star_r1: rows[1],
        star_r2: rows[2],
        sky_strength: sun.sky_strength.max(0.0),
        uniform_value: 0.0,
    }
}

impl SkyUniforms {
    pub fn with_exposure_ev(mut self, exposure_ev: f32) -> Self {
        self.sun_model.w *= 2.0f32.powf(exposure_ev.clamp(-12.0, 12.0));
        self
    }

    pub fn uniform_white(value: f32) -> Self {
        let mut sky = sky_uniforms(&Sun::default(), vec3f(0.0, 1.0, 0.0));
        sky.sun_radiance = Vec4f::default();
        sky.up.w = 0.0;
        sky.sky_strength = 0.0;
        sky.uniform_value = value.max(0.0);
        sky
    }

    pub fn sun_sample_probability(&self) -> f32 {
        if self.up.w > 0.5 && self.sun_radiance.x > 0.0 {
            1.0
        } else {
            0.0
        }
    }

    /// Engine sky colour for a world-space direction. The visible stylised
    /// sun belongs to this function; `sun_radiance` is only for direct-light
    /// sampling and is not added a second time to camera rays.
    fn radiance_impl(&self, direction: Vec3f, visible_sun: bool) -> Vec3f {
        if self.uniform_value > 0.0 {
            return vec3f(self.uniform_value, self.uniform_value, self.uniform_value);
        }
        if self.sky_strength <= 0.0 {
            return Vec3f::default();
        }
        let view = direction.normalize();
        let up = vec3f(self.up.x, self.up.y, self.up.z);
        let altitude = view.dot(up);
        let cos_theta = altitude.max(0.01);
        let model_sun = vec3f(self.sun_model.x, self.sun_model.y, self.sun_model.z);
        let cos_gamma = view.dot(model_sun).clamp(-1.0, 1.0);
        let gamma = cos_gamma.acos();
        let eval = |coefficients: Vec4f, e: f32| {
            (1.0 + coefficients.x * (coefficients.y / cos_theta).exp())
                * (1.0
                    + coefficients.z * (coefficients.w * gamma).exp()
                    + e * cos_gamma * cos_gamma)
        };
        let luminance = self.zenith.x * eval(self.pz_y, self.pz_e.x) * self.pz_f0.x;
        let chroma_x = self.zenith.y * eval(self.pz_x, self.pz_e.y) * self.pz_f0.y;
        let chroma_y =
            (self.zenith.z * eval(self.pz_yc, self.pz_e.z) * self.pz_f0.z).max(0.0001);
        let mut mapped_y = (luminance * self.sun_model.w).max(0.0);
        mapped_y /= 1.0 + mapped_y;
        let big_x = chroma_x * (mapped_y / chroma_y);
        let big_z = (1.0 - chroma_x - chroma_y) * (mapped_y / chroma_y);
        let rgb = vec3f(
            (3.2406 * big_x - 1.5372 * mapped_y - 0.4986 * big_z).max(0.0),
            (-0.9689 * big_x + 1.8758 * mapped_y + 0.0415 * big_z).max(0.0),
            (0.0557 * big_x - 0.204 * mapped_y + 1.057 * big_z).max(0.0),
        );
        let maximum = rgb.x.max(rgb.y).max(rgb.z).max(1.0);
        let mut day = vec3f(
            (rgb.x / maximum).powf(1.0 / 2.2),
            (rgb.y / maximum).powf(1.0 / 2.2),
            (rgb.z / maximum).powf(1.0 / 2.2),
        ) * (1.0 - 0.65 * (-altitude * 3.0).clamp(0.0, 1.0));

        let true_sun = vec3f(self.sun_dir.x, self.sun_dir.y, self.sun_dir.z);
        let sun_gamma = view.dot(true_sun).clamp(-1.0, 1.0).acos();
        let extinction = |height: f32, floor: f32| {
            let depth = -0.485 / (height + 0.033).max(floor).powf(0.75);
            vec3f(
                (0.39 * depth).exp(),
                (0.57 * depth).exp(),
                depth.exp(),
            ) * 2.0
        };
        let view_absorption = extinction(altitude, 0.02);
        let sun_absorption = extinction(true_sun.dot(up), 0.012);
        let limb = 1.0 - smoothstep(0.048, 0.055, sun_gamma);
        let mie_d = (1.0 - (sun_gamma * 0.55).powf(0.1)).clamp(0.0, 1.0);
        let mie = mie_d * mie_d * (3.0 - 2.0 * mie_d) * 1.4;
        let disc = if visible_sun { limb * 20.0 } else { 0.0 };
        day = day
            + (view_absorption * disc + sun_absorption * mie)
                * ((altitude + 0.033) * 90.0 + 0.5).clamp(0.0, 1.0);

        let night = vec3f(0.010, 0.012, 0.020)
            + (vec3f(0.002, 0.003, 0.006) - vec3f(0.010, 0.012, 0.020))
                * (altitude * 1.4).clamp(0.0, 1.0);
        let night_blend = self.zenith.w;
        let mut result = day + (night - day) * night_blend;

        if night_blend > 0.0 && altitude > -1.0 / 60.0 {
            let row = |r: Vec4f| r.x * view.x + r.y * view.y + r.z * view.z;
            let sx = row(self.star_r0);
            let sy = row(self.star_r1).clamp(-1.0, 1.0);
            let sz = row(self.star_r2);
            let u = sz.atan2(sx) / std::f32::consts::TAU + 0.5;
            let v = 0.5 - sy.asin() / std::f32::consts::PI;
            let point_layer = |width: f32,
                               height: f32,
                               hx: f32,
                               hy: f32,
                               keep: f32,
                               radius: f32,
                               power: f32| {
                let gx = u * width;
                let gy = v * height;
                let hash = fract((gx.floor() * hx + gy.floor() * hy).sin() * 43_758.5453);
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
            let fade = night_blend * (altitude * 6.0 + 0.1).clamp(0.0, 1.0);
            result = result
                + (vec3f(0.85, 0.9, 1.0) * spark
                    + vec3f(1.0, 0.97, 0.9) * spark2)
                    * fade;
        }
        result * self.sky_strength
    }

    /// Smooth transport environment. The large presentation disc is omitted;
    /// direct lighting already has an explicit physical-disc sampler.
    pub fn radiance(&self, direction: Vec3f) -> Vec3f {
        self.radiance_impl(direction, false)
    }

    pub fn environment_radiance(&self, direction: Vec3f, include_sun: bool) -> Vec3f {
        self.radiance_impl(direction, include_sun)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_error(a: f32, b: f32) -> f32 {
        (a - b).abs() / a.abs().max(b.abs()).max(1.0e-5)
    }

    /// Literal f32 transcription of the tracer sky shader in `gpu.rs`.
    fn gpu_shader_sky(sky: &SkyUniforms, direction: Vec3f) -> Vec3f {
        if sky.uniform_value > 0.0 {
            return vec3f(sky.uniform_value, sky.uniform_value, sky.uniform_value);
        }
        if sky.sky_strength <= 0.0 {
            return Vec3f::default();
        }
        let view = direction.normalize();
        let up = vec3f(sky.up.x, sky.up.y, sky.up.z);
        let altitude = view.dot(up);
        let cos_theta = altitude.max(0.01);
        let model_sun = vec3f(sky.sun_model.x, sky.sun_model.y, sky.sun_model.z);
        let cos_gamma = view.dot(model_sun).clamp(-1.0, 1.0);
        let gamma = cos_gamma.acos();
        let eval = |coefficients: Vec4f, e: f32| {
            (1.0 + coefficients.x * (coefficients.y / cos_theta).exp())
                * (1.0
                    + coefficients.z * (coefficients.w * gamma).exp()
                    + e * cos_gamma * cos_gamma)
        };
        let luminance = sky.zenith.x * eval(sky.pz_y, sky.pz_e.x) * sky.pz_f0.x;
        let chroma_x = sky.zenith.y * eval(sky.pz_x, sky.pz_e.y) * sky.pz_f0.y;
        let chroma_y =
            (sky.zenith.z * eval(sky.pz_yc, sky.pz_e.z) * sky.pz_f0.z).max(0.0001);
        let mut mapped_y = (luminance * sky.sun_model.w).max(0.0);
        mapped_y /= 1.0 + mapped_y;
        let big_x = chroma_x * (mapped_y / chroma_y);
        let big_z = (1.0 - chroma_x - chroma_y) * (mapped_y / chroma_y);
        let rgb = vec3f(
            (3.2406 * big_x - 1.5372 * mapped_y - 0.4986 * big_z).max(0.0),
            (-0.9689 * big_x + 1.8758 * mapped_y + 0.0415 * big_z).max(0.0),
            (0.0557 * big_x - 0.204 * mapped_y + 1.057 * big_z).max(0.0),
        );
        let maximum = rgb.x.max(rgb.y).max(rgb.z).max(1.0);
        let mut day = vec3f(
            (rgb.x / maximum).powf(0.4545454),
            (rgb.y / maximum).powf(0.4545454),
            (rgb.z / maximum).powf(0.4545454),
        ) * (1.0 - 0.65 * (-altitude * 3.0).clamp(0.0, 1.0));

        let true_sun = vec3f(sky.sun_dir.x, sky.sun_dir.y, sky.sun_dir.z);
        let sun_gamma = view.dot(true_sun).clamp(-1.0, 1.0).acos();
        let sun_height = true_sun.dot(up);
        let depth = -0.485 / (sun_height + 0.033).max(0.012).powf(0.75);
        let absorption = vec3f(
            (0.39 * depth).exp(),
            (0.57 * depth).exp(),
            depth.exp(),
        ) * 2.0;
        let mie_d = (1.0 - (sun_gamma * 0.55).powf(0.1)).clamp(0.0, 1.0);
        let mie = mie_d * mie_d * (3.0 - 2.0 * mie_d) * 1.4;
        day = day
            + absorption
                * mie
                * ((altitude + 0.033) * 90.0 + 0.5).clamp(0.0, 1.0);

        let night = vec3f(0.010, 0.012, 0.020)
            + (vec3f(0.002, 0.003, 0.006) - vec3f(0.010, 0.012, 0.020))
                * (altitude * 1.4).clamp(0.0, 1.0);
        let mut result = day + (night - day) * sky.zenith.w;
        let row = |r: Vec4f| r.x * view.x + r.y * view.y + r.z * view.z;
        let sx = row(sky.star_r0);
        let sy = row(sky.star_r1).clamp(-1.0, 1.0);
        let sz = row(sky.star_r2);
        let u = sz.atan2(sx) / std::f32::consts::TAU + 0.5;
        let v = 0.5 - sy.asin() / std::f32::consts::PI;
        let point_layer = |width: f32,
                           height: f32,
                           hx: f32,
                           hy: f32,
                           keep: f32,
                           radius: f32,
                           power: f32| {
            let gx = u * width;
            let gy = v * height;
            let hash = fract((gx.floor() * hx + gy.floor() * hy).sin() * 43_758.5453);
            let dx = fract(gx) - 0.5;
            let dy = fract(gy) - 0.5;
            let point = (1.0 - (dx * dx + dy * dy).sqrt() * radius)
                .clamp(0.0, 1.0)
                .powf(power);
            ((hash >= keep) as u8 as f32, point, hash)
        };
        let (keep0, point0, hash0) =
            point_layer(1600.0, 800.0, 127.1, 311.7, 0.995, 2.0, 3.0);
        let spark = keep0 * point0 * (0.3 + 0.7 * fract(hash0 * 57.31));
        let (keep1, point1, hash1) =
            point_layer(400.0, 200.0, 269.5, 183.3, 0.992, 2.4, 4.0);
        let spark2 = keep1 * point1 * (0.5 + 0.5 * fract(hash1 * 43.7));
        let fade = sky.zenith.w * (altitude * 6.0 + 0.1).clamp(0.0, 1.0);
        result = result
            + (vec3f(0.85, 0.9, 1.0) * spark + vec3f(1.0, 0.97, 0.9) * spark2)
                * fade;
        result * sky.sky_strength
    }

    #[test]
    fn engine_and_tracer_skies_agree_for_eight_directions_at_nine_hours() {
        let up = vec3f(0.0, 1.0, 0.0);
        let directions = [
            vec3f(0.0, 1.0, 0.0),
            vec3f(1.0, 0.04, 0.0).normalize(),
            vec3f(-1.0, 0.04, 0.0).normalize(),
            vec3f(0.0, 0.04, 1.0).normalize(),
            vec3f(0.0, 0.04, -1.0).normalize(),
            vec3f(0.7, 0.5, 0.2).normalize(),
            vec3f(-0.3, 0.65, 0.7).normalize(),
            vec3f(0.2, -0.08, -0.9).normalize(),
        ];
        for hour in [0.0f32, 3.0, 4.0, 8.0, 12.0, 16.0, 20.0, 21.0, 23.0] {
            let sun = Sun {
                dir: Sun::from_time(hour, 52.0, up),
                ..Default::default()
            };
            let tracer = sky_uniforms_at_time(&sun, up, hour, 52.0);
            let engine_frame = makepad_render::sky::preetham_frame(
                sun.dir,
                sun.turbidity,
                ENGINE_SKY_EXPOSURE,
            );
            let engine_rows = makepad_render::sun::celestial_rows(hour, 52.0);
            let mut hour_max_error = 0.0f32;
            for direction in directions {
                let engine = makepad_render::sky::analytic_sky_rgb(
                    &engine_frame,
                    direction,
                    engine_rows,
                );
                let traced = tracer.environment_radiance(direction, true);
                for (expected, actual) in [
                    (engine.x, traced.x),
                    (engine.y, traced.y),
                    (engine.z, traced.z),
                ] {
                    let error = relative_error(expected, actual);
                    hour_max_error = hour_max_error.max(error);
                    assert!(
                        error <= 0.02,
                        "hour {hour}, direction {direction:?}: engine {engine:?}, tracer {traced:?}, error {:.3}%",
                        error * 100.0
                    );
                }
            }
            println!(
                "engine/tracer sky parity at {hour:>4.1}h: max error {:.4}%",
                hour_max_error * 100.0
            );
        }
    }

    #[test]
    fn night_stars_exist_and_rotate_with_civil_time() {
        let up = vec3f(0.0, 1.0, 0.0);
        let elevation = -13.0f32.to_radians();
        let sun = Sun {
            dir: vec3f(elevation.cos(), elevation.sin(), 0.0),
            ..Default::default()
        };
        let at_21 = sky_uniforms_at_time(&sun, up, 21.0, 52.0);
        let at_23 = sky_uniforms_at_time(&sun, up, 23.0, 52.0);
        assert_eq!(at_21.zenith.w, 1.0);
        assert_eq!(at_23.zenith.w, 1.0);

        let mut brightest = 0.0f32;
        let mut largest_rotation_change = 0.0f32;
        for y in 1..96 {
            let altitude = y as f32 / 96.0;
            let radius = (1.0 - altitude * altitude).sqrt();
            for x in 0..192 {
                let azimuth = std::f32::consts::TAU * x as f32 / 192.0;
                let direction = vec3f(
                    radius * azimuth.cos(),
                    altitude,
                    radius * azimuth.sin(),
                );
                let a = at_21.radiance(direction);
                let b = at_23.radiance(direction);
                brightest = brightest.max(a.x.max(a.y).max(a.z));
                largest_rotation_change = largest_rotation_change
                    .max((a.x - b.x).abs().max((a.y - b.y).abs()).max((a.z - b.z).abs()));
            }
        }
        assert!(brightest > 0.08, "brightest night sample {brightest}");
        assert!(
            largest_rotation_change > 0.04,
            "star field did not rotate: largest change {largest_rotation_change}"
        );
    }

    #[test]
    fn cpu_gpu_shared_sky_parity() {
        let up = vec3f(0.0, 1.0, 0.0);
        for hour in [0.0f32, 3.0, 6.0, 12.0, 18.0, 21.0, 23.0] {
            let sky = sky_uniforms_at_time(
                &Sun {
                    dir: Sun::from_time(hour, 52.0, up),
                    ..Default::default()
                },
                up,
                hour,
                52.0,
            );
            for direction in [
                up,
                vec3f(1.0, 0.04, 0.0).normalize(),
                vec3f(-0.3, 0.65, 0.7).normalize(),
            ] {
                let cpu = sky.radiance(direction);
                let gpu = gpu_shader_sky(&sky, direction);
                for (expected, actual) in
                    [(cpu.x, gpu.x), (cpu.y, gpu.y), (cpu.z, gpu.z)]
                {
                    assert!(
                        relative_error(expected, actual) <= 0.000_01,
                        "hour {hour}, direction {direction:?}: CPU {cpu:?}, GPU {gpu:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn uniform_white_is_grey_everywhere() {
        let sky = SkyUniforms::uniform_white(1.0);
        for direction in [
            vec3f(0.0, 1.0, 0.0),
            vec3f(1.0, 0.0, 0.0),
            vec3f(0.3, 0.5, -0.7),
            vec3f(0.0, -1.0, 0.0),
        ] {
            assert_eq!(sky.radiance(direction.normalize()), vec3f(1.0, 1.0, 1.0));
        }
    }

    #[test]
    fn night_has_no_direct_sun() {
        let sky = sky_uniforms(
            &Sun {
                dir: vec3f(0.3, -0.5, 0.2).normalize(),
                ..Default::default()
            },
            vec3f(0.0, 1.0, 0.0),
        );
        assert_eq!(sky.sun_sample_probability(), 0.0);
        assert_eq!(sky.sun_radiance, Vec4f::default());
    }
}
