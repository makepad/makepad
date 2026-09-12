//! Entity-space fixture placement. Uses the same pose as the visible chassis;
//! no cached transforms or geometry revisions, and no per-frame script writes.
use crate::{lightmap::LmLight, Renderer};
use makepad_draw::*;
use makepad_scene::{Entity, EntityLight, World, MAX_ENTITY_LIGHTS};

pub fn place_entity_light(owner: &Entity, light: &EntityLight) -> Option<LmLight> {
    if !light.enabled || light.intensity <= 0.0 || light.validate().is_err() {
        return None;
    }
    let matrix = Renderer::rigid_transform(owner);
    let pos = matrix
        .transform_vec4(vec4(
            light.pos.x * owner.scale.x,
            light.pos.y * owner.scale.y,
            light.pos.z * owner.scale.z,
            1.0,
        ))
        .to_vec3f();
    let dir = matrix
        .transform_vec4(vec4(light.dir.x, light.dir.y, light.dir.z, 0.0))
        .to_vec3f();
    let len = dir.length();
    if !pos.length().is_finite() || !len.is_finite() || len < 1.0e-6 {
        return None;
    }
    let mut placed = LmLight::omni(pos, light.color * light.intensity, light.range);
    placed.dir = dir * (1.0 / len);
    placed.cone = light.spot.then_some((light.inner_angle, light.outer_angle));
    placed.shadows = light.shadows;
    Some(placed)
}

pub fn append_entity_lights(world: &World, out: &mut Vec<LmLight>) {
    append_entity_lights_with_model_headlights(world, out, &[]);
}

pub fn append_entity_lights_with_model_headlights(world: &World, out: &mut Vec<LmLight>, model_owners: &[u64]) {
    for entity in &world.entities {
        // Hidden chassis still own visible models and fixtures.
        for light in entity.lights.iter().take(MAX_ENTITY_LIGHTS) {
            if matches!(light.name.as_str(), "headlight_left" | "headlight_right")
                && model_owners.contains(&entity.id)
            {
                continue;
            }
            if let Some(placed) = place_entity_light(entity, light) {
                out.push(placed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_exterior_suppresses_only_standard_headlights_and_restores_on_removal() {
        let mut world = World::new();
        let mut car = Entity { id: 17, scale: vec3f(1.0, 1.0, 1.0), half: vec3f(1.0, 0.5, 2.0), ..Default::default() };
        car.set_headlights(true).unwrap();
        car.set_light(EntityLight { name: "cabin".into(), ..Default::default() }).unwrap();
        world.push_entity(car).unwrap();
        let mut lights = Vec::new();
        append_entity_lights_with_model_headlights(&world, &mut lights, &[17]);
        assert_eq!(lights.len(), 1, "the independent cabin light remains");
        assert!(world.entity(17).unwrap().lights.iter().all(|light| light.enabled));
        lights.clear();
        append_entity_lights_with_model_headlights(&world, &mut lights, &[]);
        assert_eq!(lights.len(), 3, "generic headlights return after exterior removal");
    }
    #[test]
    fn lights_follow_translation_yaw_scale_and_hidden_owners() {
        let mut world = World::new();
        let mut e = Entity {
            id: 1,
            pos: vec3f(10.0, 2.0, 3.0),
            yaw: std::f32::consts::FRAC_PI_2,
            scale: vec3f(2.0, 2.0, 2.0),
            hidden: true,
            ..Default::default()
        };
        let light = EntityLight {
            pos: vec3f(0.0, 0.0, -1.0),
            spot: true,
            shadows: true,
            ..Default::default()
        };
        let expected = Renderer::rigid_transform(&e)
            .transform_vec4(vec4(0.0, 0.0, -2.0, 1.0))
            .to_vec3f();
        e.set_light(light.clone()).unwrap();
        world.push_entity(e).unwrap();
        let mut out = Vec::new();
        append_entity_lights(&world, &mut out);
        assert_eq!(out.len(), 1);
        assert!((out[0].pos - expected).length() < 1.0e-5);
        assert!(out[0].dir.x.abs() > 0.999 && out[0].dir.z.abs() < 1.0e-5);
        assert_eq!(
            out[0].radius, light.range,
            "scale must not change light reach"
        );
        assert_eq!(out[0].color, light.color * light.intensity);
        assert_eq!(out[0].cone, Some((15.0, 25.0)));
        assert!(out[0].shadows);
        world.entity_mut(1).unwrap().pos.y += 5.0;
        let moved = place_entity_light(world.entity(1).unwrap(), &light).unwrap();
        assert!((moved.pos.y - out[0].pos.y - 5.0).abs() < 1.0e-5);
        world.entity_mut(1).unwrap().lights[0].enabled = false;
        out.clear();
        append_entity_lights(&world, &mut out);
        assert!(out.is_empty());
    }
    #[test]
    fn rigid_pitch_rotates_emission_and_bad_pose_is_rejected() {
        let s = (std::f32::consts::FRAC_PI_4).sin();
        let mut e = Entity {
            kind: makepad_scene::BodyKind::Rigid,
            scale: vec3f(1.0, 1.0, 1.0),
            orient: Quat {
                x: s,
                y: 0.0,
                z: 0.0,
                w: s,
            },
            ..Default::default()
        };
        let light = EntityLight::default();
        let placed = place_entity_light(&e, &light).unwrap();
        assert!(placed.dir.y > 0.999 && placed.dir.z.abs() < 1.0e-5);
        e.pos.x = f32::NAN;
        assert!(place_entity_light(&e, &light).is_none());
    }
}
