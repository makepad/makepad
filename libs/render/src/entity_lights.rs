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

/// Every enabled light of every entity, vehicle lamps at full strength and
/// unbudgeted (bakes, tools). The frame path is
/// [`append_entity_lights_with_model_headlights`].
pub fn append_entity_lights(world: &World, out: &mut Vec<LmLight>) {
    for entity in &world.entities {
        for light in entity.lights.iter().take(MAX_ENTITY_LIGHTS) {
            out.extend(place_entity_light(entity, light));
        }
    }
}

/// Vehicles whose lamps light the scene: the radius follows the
/// `LAMP_TARGET`-th nearest lit vehicle (never past `LAMP_RANGE`, never
/// under `LAMP_MIN_RADIUS`), at most `LAMP_CAP` vehicles. Beyond it a car
/// shows its emissive lenses only; its lamps add nothing a player could see
/// as light on the road.
pub const LAMP_TARGET: usize = 8;
pub const LAMP_CAP: usize = 12;
pub const LAMP_RANGE: f32 = 120.0;
pub const LAMP_MIN_RADIUS: f32 = 30.0;
/// Taillights are 1.8 m red glows: past this their pool is a few pixels
/// the lens already covers.
pub const TAIL_RANGE: f32 = 35.0;

/// The frame's entity lights. Vehicle lamps (`makepad_scene::light`) are
/// budgeted here, so a street of parked or driving cars costs a handful of
/// lights, not four per car:
/// - `lamp_level` dims them (0 in daylight: none at all; 1 at night or in fog);
/// - only the vehicles within a radius of `eye` light, the nearest first, at
///   most [`LAMP_CAP`]; the radius follows the [`LAMP_TARGET`]-th nearest
///   vehicle slowly (`lamp_radius` carries it between frames, 0 to start),
///   and a lamp fades over the radius' last quarter, so a lamp only ever
///   fades in or out as its car crosses the edge, never swaps between cars;
/// - taillights only within [`TAIL_RANGE`].
/// A headlight on a car whose model brings its own (`model_owners`) is left
/// to the model. Returns the vehicle lamps it added.
pub fn append_entity_lights_with_model_headlights(
    world: &World,
    out: &mut Vec<LmLight>,
    model_owners: &[u64],
    lamp_level: f32,
    eye: Vec3f,
    lamp_radius: &mut f32,
) -> usize {
    let mut cars: Vec<(f32, usize)> = Vec::new();
    for (i, entity) in world.entities.iter().enumerate() {
        // Hidden chassis still own visible models and fixtures.
        let mut lamps = false;
        for light in entity.lights.iter().take(MAX_ENTITY_LIGHTS) {
            if makepad_scene::light::is_vehicle_lamp(&light.name) {
                lamps |= light.enabled && light.intensity > 0.0;
                continue;
            }
            out.extend(place_entity_light(entity, light));
        }
        if lamps && lamp_level >= 0.02 {
            let d = (entity.pos - eye).length();
            if d.is_finite() && d < LAMP_RANGE {
                cars.push((d, i));
            }
        }
    }
    if cars.is_empty() {
        *lamp_radius = 0.0;
        return 0;
    }
    cars.sort_by(|a, b| a.0.total_cmp(&b.0));
    let target = cars.get(LAMP_TARGET).map_or(LAMP_RANGE, |c| c.0).clamp(LAMP_MIN_RADIUS, LAMP_RANGE);
    *lamp_radius = if *lamp_radius <= 0.0 { target } else { *lamp_radius + (target - *lamp_radius) * 0.03 };
    let r = *lamp_radius;
    let mut added = 0;
    for &(d, i) in cars.iter().take(LAMP_CAP) {
        let fade = ((r - d) / (r * 0.25)).clamp(0.0, 1.0);
        if fade <= 0.0 {
            break;
        }
        let entity = &world.entities[i];
        let tail_fade = ((TAIL_RANGE - d) / (TAIL_RANGE * 0.25)).clamp(0.0, 1.0);
        for light in entity.lights.iter().take(MAX_ENTITY_LIGHTS) {
            let level = match light.name.as_str() {
                "headlight_left" | "headlight_right" if model_owners.contains(&entity.id) => continue,
                "headlight_left" | "headlight_right" => fade,
                "taillight_left" | "taillight_right" => fade.min(tail_fade),
                _ => continue,
            };
            if level <= 0.0 {
                continue;
            }
            if let Some(mut placed) = place_entity_light(entity, light) {
                placed.color = placed.color * (lamp_level * level);
                out.push(placed);
                added += 1;
            }
        }
    }
    added
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
        let eye = Vec3f::default();
        append_entity_lights_with_model_headlights(&world, &mut lights, &[17], 1.0, eye, &mut 0.0);
        assert_eq!(lights.len(), 3, "the independent cabin light and the taillights remain");
        assert!(world.entity(17).unwrap().lights.iter().all(|light| light.enabled));
        lights.clear();
        append_entity_lights_with_model_headlights(&world, &mut lights, &[], 1.0, eye, &mut 0.0);
        assert_eq!(lights.len(), 5, "generic headlights return after exterior removal");
        lights.clear();
        append_entity_lights_with_model_headlights(&world, &mut lights, &[], 0.0, eye, &mut 0.0);
        assert_eq!(lights.len(), 1, "in daylight only the cabin light shines");
    }
    #[test]
    fn a_street_of_cars_lights_only_the_nearest_few() {
        let mut world = World::new();
        // 60 cars in a row, 6 m apart, eye at the first.
        for i in 0..60u64 {
            let mut car = Entity { id: i + 1, pos: vec3f(0.0, 0.0, i as f32 * 6.0), scale: vec3f(1.0, 1.0, 1.0), half: vec3f(1.0, 0.5, 2.0), ..Default::default() };
            car.set_headlights(true).unwrap();
            world.push_entity(car).unwrap();
        }
        let mut lights = Vec::new();
        let mut radius = 0.0;
        let lamps = append_entity_lights_with_model_headlights(&world, &mut lights, &[], 1.0, Vec3f::default(), &mut radius);
        assert_eq!(lamps, lights.len());
        // 240 lamp sockets; the radius sits at the 9th car (48 m), cars
        // inside it light their headlights, those within 35 m their tails.
        assert!(lamps <= LAMP_CAP * 4 && lamps >= 2 * LAMP_TARGET, "{lamps} lamps");
        assert!(lights.iter().all(|l| l.pos.z < radius + 3.0), "nothing past the radius ({radius} m)");
        assert!(lights.iter().filter(|l| l.cone.is_none()).all(|l| l.pos.z < TAIL_RANGE + 3.0), "taillights only near");
        lights.clear();
        assert_eq!(append_entity_lights_with_model_headlights(&world, &mut lights, &[], 0.0, Vec3f::default(), &mut radius), 0, "none by day");
        assert!(lights.is_empty());
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
