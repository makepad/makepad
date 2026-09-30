//! World lights: the lights a document or a host puts in the frame, in world
//! space and physical-ish units. Entity lights (`EntityLight`) stay the
//! game's authored fixtures; both reach the same clustered light list.
use makepad_math::*;

pub const MAX_WORLD_LIGHTS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowSpec {
    pub enabled: bool,
    /// Depth bias in world units; 0 = the renderer's default.
    pub bias: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// The key light. `dir` points from the scene toward the sun.
    Sun { dir: Vec3f, color: Vec3f, lux: f32, shadow: ShadowSpec },
    /// `cone` = (inner, outer) half-angles in degrees.
    Spot { pos: Vec3f, dir: Vec3f, color: Vec3f, intensity: f32, range: f32, cone: Vec2f, shadow: bool },
    /// A point light.
    Lamp { pos: Vec3f, color: Vec3f, intensity: f32, range: f32, shadow: bool },
    /// Hemisphere ambient.
    Sky { top: Vec3f, ground: Vec3f, intensity: f32 },
}

impl Light {
    pub fn validate(&self) -> Result<(), &'static str> {
        let finite = |v: Vec3f| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
        let colour = |c: Vec3f| finite(c) && c.x >= 0.0 && c.y >= 0.0 && c.z >= 0.0;
        let unit = |d: Vec3f| finite(d) && d.dot(d) > 1.0e-12;
        let level = |i: f32| i.is_finite() && (0.0..=1.0e6).contains(&i);
        let range = |r: f32| r.is_finite() && (0.01..=1.0e5).contains(&r);
        match *self {
            Light::Sun { dir, color, lux, .. } => {
                if !unit(dir) || !colour(color) || !level(lux) { return Err("sun needs a direction, a colour and lux in 0..1e6"); }
            }
            Light::Spot { pos, dir, color, intensity, range: r, cone, .. } => {
                if !finite(pos) || !unit(dir) || !colour(color) || !level(intensity) || !range(r) {
                    return Err("spot needs a position, direction, colour, intensity and range");
                }
                if !(cone.x.is_finite() && cone.y.is_finite() && 0.0 <= cone.x && cone.x <= cone.y && cone.y < 90.0) {
                    return Err("spot cone needs 0 <= inner <= outer < 90 degrees");
                }
            }
            Light::Lamp { pos, color, intensity, range: r, .. } => {
                if !finite(pos) || !colour(color) || !level(intensity) || !range(r) {
                    return Err("lamp needs a position, colour, intensity and range");
                }
            }
            Light::Sky { top, ground, intensity } => {
                if !colour(top) || !colour(ground) || !level(intensity) { return Err("sky light needs colours and an intensity"); }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lights_validate() {
        let lamp = Light::Lamp { pos: vec3f(0.0, 2.0, 0.0), color: vec3f(1.0, 0.9, 0.8), intensity: 20.0, range: 8.0, shadow: false };
        assert!(lamp.validate().is_ok());
        let spot = Light::Spot { pos: Vec3f::default(), dir: vec3f(0.0, -1.0, 0.0), color: vec3f(1.0, 1.0, 1.0), intensity: 5.0, range: 10.0, cone: vec2f(20.0, 10.0), shadow: false };
        assert!(spot.validate().is_err(), "inner > outer");
        let sun = Light::Sun { dir: Vec3f::default(), color: vec3f(1.0, 1.0, 1.0), lux: 1.0, shadow: ShadowSpec::default() };
        assert!(sun.validate().is_err(), "zero direction");
        assert!(Light::Sky { top: vec3f(-1.0, 0.0, 0.0), ground: Vec3f::default(), intensity: 1.0 }.validate().is_err());
    }
}
