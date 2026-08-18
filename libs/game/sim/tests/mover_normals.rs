//! P2: floor and wall normals on the mover path. The sweep now reports WHAT
//! a mover stands on and what it pressed against — flat tops report up,
//! wedges and terrain report their true slope, clamped walls report the face
//! that stopped the motion. All of it is additive: the golden hash in
//! mover_golden.rs proves the motion itself never moved.

use makepad_game_sim::*;
use makepad_math::*;

fn ent(id: u64, kind: BodyKind, pos: Vec3f, half: Vec3f) -> Entity {
    Entity {
        id,
        kind,
        pos,
        half,
        collide: true,
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.6,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        speed_mult: 1.0,
        ..Default::default()
    }
}

fn ground(w: &mut GameWorld) {
    w.push_entity(ent(
        1,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(40.0, 0.5, 40.0),
    ));
}

#[test]
fn a_flat_floor_reports_an_up_normal_and_air_reports_none() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    let mut m = ent(2, BodyKind::Mover, vec3f(0.0, 3.0, 0.0), vec3f(0.3, 0.5, 0.3));
    m.gravity_scale = 1.0;
    w.push_entity(m);
    // Mid-air: no floor, no normal.
    step_world(&mut w);
    let e = w.entity(2).unwrap();
    assert!(!e.on_floor);
    assert_eq!(e.floor_normal, vec3f(0.0, 0.0, 0.0), "airborne must report no floor normal");
    // Settled: up.
    for _ in 0..60 {
        step_world(&mut w);
    }
    let e = w.entity(2).unwrap();
    assert!(e.on_floor);
    assert_eq!(e.floor_normal, vec3f(0.0, 1.0, 0.0));
    // And the accessor maps the airborne zero to up for slope math.
    assert_eq!(floor_normal_of(&Entity::default()), vec3f(0.0, 1.0, 0.0));
}

#[test]
fn a_wedge_reports_its_slope_normal() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    // 4 tall over 14 deep: slope m = 4/14, normal (0, 1, -m)/len.
    let mut ramp = ent(2, BodyKind::Static, vec3f(0.0, 2.0, 0.0), vec3f(6.0, 2.0, 7.0));
    ramp.shape = Shape::Wedge;
    w.push_entity(ramp);
    let m = ent(3, BodyKind::Mover, vec3f(0.0, 4.5, 0.0), vec3f(0.35, 0.9, 0.35));
    w.push_entity(m);
    for _ in 0..90 {
        step_world(&mut w);
    }
    let e = w.entity(3).unwrap();
    assert!(e.on_floor, "should rest on the slope");
    let n = e.floor_normal;
    let slope = 4.0f32 / 14.0;
    let len = (1.0 + slope * slope).sqrt();
    assert!((n.y - 1.0 / len).abs() < 1e-4, "normal.y {} vs {}", n.y, 1.0 / len);
    assert!((n.z - (-slope / len)).abs() < 1e-4, "normal.z {} tilts the wrong way", n.z);
    assert!(n.x.abs() < 1e-6);
}

#[test]
fn terrain_reports_the_triangle_normal_under_the_mover() {
    let mut w = GameWorld::new();
    w.reset_content();
    // A uniform east-west slope: h = x * 0.5.
    let cells = 17usize;
    let mut heights = Vec::new();
    let mut colors = Vec::new();
    for _z in 0..cells {
        for x in 0..cells {
            heights.push(x as f32 * 0.5);
            colors.push(vec4f(0.3, 0.5, 0.3, 1.0));
        }
    }
    w.terrain = Some(Terrain {
        cells,
        cell_size: 1.0,
        origin: -(cells as f32) * 0.5,
        heights,
        colors,
        revision: 1,
    });
    let m = ent(2, BodyKind::Mover, vec3f(0.0, 8.0, 0.0), vec3f(0.35, 0.9, 0.35));
    w.push_entity(m);
    for _ in 0..90 {
        step_world(&mut w);
    }
    let e = w.entity(2).unwrap();
    assert!(e.on_floor);
    let n = e.floor_normal;
    // Rises with +x, so the normal tilts toward -x; slope 0.5 per unit.
    let len = (1.0f32 + 0.25).sqrt();
    assert!((n.x - (-0.5 / len)).abs() < 1e-3, "normal.x {}", n.x);
    assert!((n.y - 1.0 / len).abs() < 1e-3, "normal.y {}", n.y);
}

#[test]
fn a_wall_hit_reports_the_face_normal_toward_the_mover() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    w.push_entity(ent(2, BodyKind::Static, vec3f(3.0, 1.5, 0.0), vec3f(0.5, 1.5, 4.0)));
    let mut m = ent(3, BodyKind::Mover, vec3f(0.0, 0.5, 0.0), vec3f(0.3, 0.5, 0.3));
    m.vel = vec3f(4.0, 0.0, 0.0);
    w.push_entity(m);
    for _ in 0..60 {
        w.entity_mut(3).unwrap().vel.x = 4.0;
        step_world(&mut w);
    }
    let e = w.entity(3).unwrap();
    assert_eq!(e.hit_wall, 2);
    assert_eq!(
        e.wall_normal,
        vec3f(-1.0, 0.0, 0.0),
        "moving +x into a wall must report the -x face"
    );
    // Walk away: the transient clears with hit_wall.
    for _ in 0..10 {
        w.entity_mut(3).unwrap().vel.x = -4.0;
        step_world(&mut w);
    }
    let e = w.entity(3).unwrap();
    assert_eq!(e.hit_wall, 0);
    assert_eq!(e.wall_normal, vec3f(0.0, 0.0, 0.0));
}
