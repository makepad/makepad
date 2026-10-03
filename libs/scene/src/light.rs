//! Authored, entity-local lights. Pure data: render budgets and shadow maps
//! belong to the device, not the authoritative simulation.
use makepad_math::*;

pub const MAX_ENTITY_LIGHTS: usize = 16;
pub const MAX_LIGHT_NAME_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct EntityLight {
    pub name: String,
    pub pos: Vec3f,
    /// Entity-local emission direction; engine forward is -Z.
    pub dir: Vec3f,
    pub color: Vec3f,
    pub intensity: f32,
    pub range: f32,
    pub spot: bool,
    /// Half angles in degrees, 0 <= inner <= outer < 90.
    pub inner_angle: f32,
    pub outer_angle: f32,
    pub enabled: bool,
    pub shadows: bool,
}

impl Default for EntityLight {
    fn default() -> Self {
        Self {
            name: "light".into(),
            pos: Vec3f::default(),
            dir: vec3f(0.0, 0.0, -1.0),
            color: vec3f(1.0, 1.0, 1.0),
            intensity: 2.0,
            range: 15.0,
            spot: false,
            inner_angle: 15.0,
            outer_angle: 25.0,
            enabled: true,
            shadows: false,
        }
    }
}

impl EntityLight {
    pub fn validate(&self) -> Result<(), &'static str> {
        let finite = |v: Vec3f| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
        if self.name.is_empty() || self.name.len() > MAX_LIGHT_NAME_BYTES {
            return Err("name must contain 1..64 bytes");
        }
        if !finite(self.pos) || !finite(self.dir) || !finite(self.color) {
            return Err("position, direction and color must be finite");
        }
        let d = self.dir.dot(self.dir);
        if !d.is_finite() || d < 1.0e-12 {
            return Err("direction must be nonzero");
        }
        if !self.intensity.is_finite() || !(0.0..=10000.0).contains(&self.intensity) {
            return Err("intensity must be in 0..10000");
        }
        if self.color.x < 0.0
            || self.color.y < 0.0
            || self.color.z < 0.0
            || self.color.x > 1.0
            || self.color.y > 1.0
            || self.color.z > 1.0
        {
            return Err("color must be in 0..1; use intensity for brightness");
        }
        if !self.range.is_finite() || !(0.05..=10000.0).contains(&self.range) {
            return Err("range must be in 0.05..10000");
        }
        if !self.inner_angle.is_finite()
            || !self.outer_angle.is_finite()
            || self.inner_angle < 0.0
            || self.inner_angle > self.outer_angle
            || !(0.5..=85.0).contains(&self.outer_angle)
        {
            return Err("cone half-angles require 0 <= inner <= outer, outer in 0.5..85 degrees");
        }
        Ok(())
    }
}

/// Replaces only this named light; editing lights never dirties geometry.
pub fn set_light(lights: &mut Vec<EntityLight>, light: EntityLight) -> Result<(), &'static str> {
    light.validate()?;
    if let Some(old) = lights.iter_mut().find(|l| l.name == light.name) {
        *old = light;
    } else {
        if lights.len() >= MAX_ENTITY_LIGHTS {
            return Err("at most 16 lights per entity");
        }
        lights.push(light);
    }
    Ok(())
}

/// The vehicle lamp names: two headlights and two taillights.
pub const LAMP_NAMES: [&str; 4] = ["headlight_left", "headlight_right", "taillight_left", "taillight_right"];

/// A vehicle lamp: lit by the renderer only after dark or in fog
/// ([`lamp_level`]), however the game left it enabled.
pub fn is_vehicle_lamp(name: &str) -> bool {
    LAMP_NAMES.contains(&name)
}

/// How much a vehicle's lamps shine: 0 in daylight, 1 at night; fog
/// (`fog_density`, the sky's exponential density) lights them too.
pub fn lamp_level(sun_dir_y: f32, fog_density: f32) -> f32 {
    let t = ((0.22 - sun_dir_y) / 0.3).clamp(0.0, 1.0);
    let night = t * t * (3.0 - 2.0 * t);
    let fog = (fog_density / 0.02).clamp(0.0, 1.0);
    night.max(fog)
}

const TAIL_INTENSITY: f32 = 0.6;
const BRAKE_INTENSITY: f32 = 1.8;

/// Standard vehicle fixtures in body space (x right, y up, -Z forward): two
/// headlight spots on the nose and two red taillights on the tail. The
/// offsets scale with the chassis until [`set_lamp_points`] puts them where
/// the model's lamps are; reach and intensity do not scale. `shadows`
/// makes the headlights cast (a player's car), never the taillights.
pub fn set_vehicle_lamps(lights: &mut Vec<EntityLight>, half: Vec3f, enabled: bool, shadows: bool) -> Result<(), &'static str> {
    let missing = LAMP_NAMES.iter().filter(|name| !lights.iter().any(|l| l.name == **name)).count();
    if lights.len() + missing > MAX_ENTITY_LIGHTS {
        return Err("not enough light slots for the vehicle lamps");
    }
    for (i, name) in LAMP_NAMES.iter().enumerate() {
        if let Some(light) = lights.iter_mut().find(|l| l.name == *name) {
            light.enabled = enabled;
            continue;
        }
        let side = if i % 2 == 0 { -1.0 } else { 1.0 };
        let light = if i < 2 {
            EntityLight {
                name: (*name).into(),
                pos: vec3f(side * half.x * 0.7, half.y * 0.15, -half.z - 0.08),
                // Dipped: the pool starts at the bumper and reaches ahead.
                dir: vec3f(0.0, -0.32, -1.0),
                color: vec3f(1.0, 0.94, 0.82),
                intensity: 4.0,
                range: 30.0,
                spot: true,
                inner_angle: 14.0,
                outer_angle: 40.0,
                enabled,
                shadows,
            }
        } else {
            EntityLight {
                name: (*name).into(),
                // A small red glow hugging the tail, not a pool on the road.
                pos: vec3f(side * half.x * 0.7, half.y * 0.15, half.z + 0.08),
                dir: vec3f(0.0, 0.0, 1.0),
                color: vec3f(1.0, 0.06, 0.04),
                intensity: TAIL_INTENSITY,
                range: 1.8,
                spot: false,
                inner_angle: 30.0,
                outer_angle: 70.0,
                enabled,
                shadows: false,
            }
        };
        set_light(lights, light)?;
    }
    Ok(())
}

/// The headlights and taillights where the model's lamps are (body frame);
/// a lamp the model has none of keeps its place.
pub fn set_lamp_points(lights: &mut [EntityLight], head: Option<[Vec3f; 2]>, tail: Option<[Vec3f; 2]>) {
    for light in lights.iter_mut() {
        let at = match light.name.as_str() {
            "headlight_left" => head.map(|h| h[0]),
            "headlight_right" => head.map(|h| h[1]),
            "taillight_left" => tail.map(|t| t[0]),
            "taillight_right" => tail.map(|t| t[1]),
            _ => None,
        };
        if let Some(at) = at.filter(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
            light.pos = at;
        }
    }
}

/// Brake lights: the taillights brighten while braking.
pub fn set_brake_lights(lights: &mut [EntityLight], braking: bool) {
    for light in lights.iter_mut().filter(|l| l.name.starts_with("taillight_")) {
        light.intensity = if braking { BRAKE_INTENSITY } else { TAIL_INTENSITY };
    }
}

/// [`set_vehicle_lamps`] with casting headlights (a player's car).
pub fn set_headlights(lights: &mut Vec<EntityLight>, half: Vec3f, enabled: bool) -> Result<(), &'static str> {
    set_vehicle_lamps(lights, half, enabled, true)
}
impl crate::Entity {
    pub fn set_light(&mut self, light: EntityLight) -> Result<(), &'static str> {
        set_light(&mut self.lights, light)
    }
    pub fn set_headlights(&mut self, enabled: bool) -> Result<(), &'static str> {
        set_headlights(&mut self.lights, self.half, enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lights_replace_validate_clone_and_follow_owner_lifetime() {
        let mut w = crate::World::new();
        let mut e = crate::Entity::default();
        e.set_light(EntityLight::default()).unwrap();
        e.set_light(EntityLight {
            intensity: 3.0,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(e.lights.len(), 1);
        assert!(e
            .set_light(EntityLight {
                range: f32::NAN,
                ..Default::default()
            })
            .is_err());
        assert_eq!(e.lights[0].intensity, 3.0);
        w.entities.push(e);
        let snap = w.clone();
        w.entities.clear();
        assert!(w.entities.is_empty());
        assert_eq!(snap.entities[0].lights.len(), 1);
    }
    #[test]
    fn headlights_are_two_named_forward_spots_and_budget_is_bounded() {
        let mut e = crate::Entity {
            half: vec3f(1.0, 0.5, 2.0),
            ..Default::default()
        };
        e.set_headlights(true).unwrap();
        assert_eq!(e.lights.len(), 4);
        assert!(e.lights[..2]
            .iter()
            .all(|l| l.spot && l.shadows && l.dir.z < 0.0 && l.pos.z < -2.0));
        assert!(e.lights[2..].iter().all(|l| !l.shadows && l.pos.z > 2.0 && l.color.x > l.color.y), "red taillights at the back");
        e.lights[0].intensity = 7.0;
        e.set_headlights(false).unwrap();
        assert!(e.lights.iter().all(|l| !l.enabled));
        assert_eq!(e.lights[0].intensity, 7.0);
        set_brake_lights(&mut e.lights, true);
        assert!(e.lights[2].intensity > TAIL_INTENSITY);
        set_lamp_points(&mut e.lights, Some([vec3f(-0.4, 0.1, -2.3), vec3f(0.4, 0.1, -2.3)]), None);
        assert_eq!(e.lights[0].pos, vec3f(-0.4, 0.1, -2.3));
        assert!(lamp_level(0.9, 0.0) < 1e-6 && lamp_level(-0.3, 0.0) > 0.99 && lamp_level(0.9, 0.05) > 0.99);
        for i in 4..MAX_ENTITY_LIGHTS {
            e.set_light(EntityLight {
                name: format!("l{i}"),
                ..Default::default()
            })
            .unwrap();
        }
        assert!(e.set_light(EntityLight::default()).is_err());
    }
    #[test]
    fn headlights_capacity_failure_is_atomic() {
        let mut e = crate::Entity::default();
        for i in 0..MAX_ENTITY_LIGHTS - 3 {
            e.set_light(EntityLight {
                name: format!("l{i}"),
                ..Default::default()
            })
            .unwrap();
        }
        let before = e.lights.clone();
        assert!(e.set_headlights(true).is_err());
        assert_eq!(e.lights, before);
    }
}
