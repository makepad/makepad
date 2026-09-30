//! The world's generic lights (`World::lights`, KERNELS.md §3.2) into the
//! frame's clustered light list. Spots and lamps are punctual
//! (inverse-square within their range, like glTF lights); rectangles are
//! area lights (lightmap.rs `AreaRect`). The sun and sky entries stay with
//! the host's sun rig (`World::sun`) until the Stage cutover wires them.
use crate::lightmap::{AreaRect, LmLight};
use makepad_scene::{Light, World};

pub fn world_light(light: &Light) -> Option<LmLight> {
    light.validate().ok()?;
    match *light {
        Light::Lamp { pos, color, intensity, range, shadow } => {
            let mut l = LmLight::omni(pos, color * intensity, range);
            l.spot = -1.0;
            l.shadows = shadow;
            Some(l)
        }
        Light::Spot { pos, dir, color, intensity, range, cone, shadow } => {
            let mut l = LmLight::omni(pos, color * intensity, range);
            l.dir = dir.normalize();
            l.spot = -1.0;
            l.cone = Some((cone.x, cone.y));
            l.shadows = shadow;
            Some(l)
        }
        Light::Rect { pos, normal, tangent, size, color, intensity, range } => {
            let n = normal.normalize();
            // The tangent made exactly perpendicular to the normal.
            let t = (tangent - n * tangent.dot(n)).normalize();
            let mut l = LmLight::omni(pos, color * intensity, range);
            l.dir = n;
            l.area = Some(AreaRect { tangent: t, half_width: size.x * 0.5, half_height: size.y * 0.5 });
            Some(l)
        }
        Light::Sun { .. } | Light::Sky { .. } => None,
    }
}

pub fn append_world_lights(world: &World, out: &mut Vec<LmLight>) {
    out.extend(world.lights.iter().filter_map(world_light));
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_draw::*;

    #[test]
    fn world_lights_become_clustered_lights() {
        let mut world = World::new();
        world.lights.push(Light::Lamp { pos: vec3f(0.0, 3.0, 0.0), color: vec3f(1.0, 0.5, 0.25), intensity: 4.0, range: 10.0, shadow: false });
        world.lights.push(Light::Rect { pos: vec3f(0.0, 4.0, 0.0), normal: vec3f(0.0, -2.0, 0.0), tangent: vec3f(1.0, 0.3, 0.0), size: vec2f(2.0, 0.5), color: vec3f(1.0, 1.0, 1.0), intensity: 20.0, range: 12.0 });
        world.lights.push(Light::Sky { top: vec3f(1.0, 1.0, 1.0), ground: vec3f(0.1, 0.1, 0.1), intensity: 1.0 });
        world.lights.push(Light::Lamp { pos: vec3f(0.0, f32::NAN, 0.0), color: vec3f(1.0, 1.0, 1.0), intensity: 1.0, range: 1.0, shadow: false });
        let mut out = Vec::new();
        append_world_lights(&world, &mut out);
        assert_eq!(out.len(), 2, "the sky stays with the rig and a broken lamp is dropped");
        assert_eq!(out[0].color, vec3f(4.0, 2.0, 1.0));
        assert!(out[0].spot < 0.0 && out[0].area.is_none());
        let a = out[1].area.unwrap();
        assert!((a.half_width - 1.0).abs() < 1e-6 && (a.half_height - 0.25).abs() < 1e-6);
        assert!(a.tangent.dot(out[1].dir).abs() < 1e-5, "tangent made perpendicular");
        assert!((out[1].dir.y + 1.0).abs() < 1e-6);
        // The packed angle rebuilds the tangent from the normal alone.
        let (b0, b1) = crate::lightmap::area_basis(out[1].dir);
        let ang = a.angle_about(out[1].dir);
        let rebuilt = b0 * ang.cos() + b1 * ang.sin();
        assert!((rebuilt - a.tangent).length() < 1e-5);
    }
}

/// The rig with a world's own Sun (key light) and Sky (hemisphere fill)
/// in place of the host's, when the world has them.
pub fn apply_world_sun(world: &World, mut sun: crate::sun::SunLight) -> crate::sun::SunLight {
    for light in world.lights.iter().filter(|l| l.validate().is_ok()) {
        match *light {
            Light::Sun { dir, color, lux, .. } => {
                sun.dir = dir.normalize();
                sun.color = color * lux;
            }
            Light::Sky { top, ground, intensity } => {
                sun.sky = top * intensity;
                sun.ground = ground * intensity;
            }
            _ => {}
        }
    }
    sun
}

/// The world environment's fog as the lanes' (colour, density): the lanes
/// fog exponentially with distance, so a linear fog maps to the density
/// that reaches 1 - 1/e halfway between its start and end, and a height
/// fog to its density at the base. `None` = the host's fog.
pub fn world_fog(world: &World) -> Option<(makepad_draw::Vec3f, f32)> {
    use makepad_scene::Fog;
    world.environment.validate().ok()?;
    match world.environment.fog {
        Fog::Host => None,
        Fog::None => Some((makepad_draw::Vec3f::default(), 0.0)),
        Fog::Exp2 { color, density } => Some((color, density)),
        Fog::Linear { color, start, end } => Some((color, 2.0 / (start + end).max(1.0e-3))),
        Fog::Height { color, density, .. } => Some((color, density)),
    }
}
