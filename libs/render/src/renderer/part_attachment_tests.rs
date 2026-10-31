use super::*;

fn center(m: Mat4f) -> Vec3f {
    vec3f(m.v[12], m.v[13], m.v[14])
}

#[test]
fn mover_parts_follow_translation_and_owner_local_heading() {
    let mut owner = Entity {
        id: 1,
        kind: BodyKind::Mover,
        pos: vec3f(3.0, 2.0, 5.0),
        yaw: std::f32::consts::FRAC_PI_2,
        scale: vec3f(1.0, 1.0, 1.0),
        ..Default::default()
    };
    let part = Part {
        id: 2,
        owner: 1,
        offset: vec3f(0.0, 0.0, -2.0),
        ..Default::default()
    };

    let first = center(Renderer::part_transform(&owner, &part));
    let expected = owner.pos + makepad_scene::heading_to_forward(owner.yaw) * 2.0;
    assert!((first - expected).length() < 1.0e-5, "part offset was world-space: {first:?}");

    let delta = vec3f(7.0, -0.5, 4.0);
    owner.pos += delta;
    let moved = center(Renderer::part_transform(&owner, &part));
    assert!((moved - first - delta).length() < 1.0e-5, "part did not ride its mover");
}
