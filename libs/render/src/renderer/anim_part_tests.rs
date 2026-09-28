use super::*;
use crate::model::tests::{rooms_and_door_glb, Door};

/// The importer-shaped fixture's door: closed at t=0 (on the floor),
/// open at t=1 (lifted 3), authored open.
fn door() -> crate::model::AnimPart {
    let mut m = StaticModel::parse_glb(&rooms_and_door_glb(Door::Animated)).unwrap();
    assert_eq!(m.anim_parts.len(), 1);
    m.anim_parts.pop().unwrap()
}

fn lift(part: &crate::model::AnimPart, time: f32) -> f32 {
    part.transform_at(time).v[13]
}

#[test]
fn an_untriggered_part_sits_in_the_authored_default() {
    let part = door();
    let states = ModelStates::default();
    let (state, time, target) =
        states.clock(&ModelTarget::Model("map".into()), "map", &part);
    assert_eq!(state, 1, "extras.default = open");
    assert!((time - 1.0).abs() < 1.0e-6);
    assert!((target - 1.0).abs() < 1.0e-6);
    assert!((lift(&part, time) - 3.0).abs() < 1.0e-6);
}

#[test]
fn closing_travels_the_clip_in_the_blend_time() {
    let part = door();
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("map".into());
    assert!(states.set(key.clone(), &part, "closed", 1.0));

    // Half the blend, half the travel — linear in time, by contract.
    states.tick(0.5);
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!((time - 0.5).abs() < 1.0e-5, "time {time}");
    assert!((lift(&part, time) - 1.5).abs() < 1.0e-4);

    // The rest of it, and then it stops dead rather than overshooting.
    states.tick(0.5);
    let (state, time, _) = states.clock(&key, "map", &part);
    assert_eq!(state, 0);
    assert!(time.abs() < 1.0e-6, "time {time}");
    states.tick(5.0);
    let (_, time, target) = states.clock(&key, "map", &part);
    assert!(time.abs() < 1.0e-6 && (time - target).abs() < 1.0e-6);
    assert!(lift(&part, time).abs() < 1.0e-6, "closed sits on the floor");
}

#[test]
fn a_command_mid_move_reverses_from_where_the_part_is() {
    let part = door();
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("map".into());
    states.set(key.clone(), &part, "closed", 1.0);
    states.tick(0.5);
    let (_, half, _) = states.clock(&key, "map", &part);

    // Re-aimed at open from exactly half way: no jump, and the reversal
    // takes its own blend rather than resuming the old one.
    states.set(key.clone(), &part, "open", 1.0);
    let (state, time, target) = states.clock(&key, "map", &part);
    assert_eq!(state, 1);
    assert!((time - half).abs() < 1.0e-6, "the door jumped: {time} vs {half}");
    assert!((target - 1.0).abs() < 1.0e-6);

    states.tick(0.5);
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!((time - 0.75).abs() < 1.0e-5, "time {time}");
    states.tick(0.5);
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!((time - 1.0).abs() < 1.0e-6, "time {time}");
    assert!((lift(&part, time) - 3.0).abs() < 1.0e-5);
}

#[test]
fn localgen_manifest_clip_loops_until_a_game_command_overrides_it() {
    let mut part = door();
    part.kind = Some("localgen-bob".into());
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("prop".into());

    states.tick(0.25);
    let (_, idle, _) = states.clock(&key, "prop", &part);
    assert!((idle - 0.25).abs() < 1.0e-6);
    states.tick(1.0);
    let (_, wrapped, _) = states.clock(&key, "prop", &part);
    assert!((wrapped - 0.25).abs() < 1.0e-6);

    assert!(states.set(key.clone(), &part, "closed", 0.0));
    states.tick(0.25);
    let (_, overridden, _) = states.clock(&key, "prop", &part);
    assert!(overridden.abs() < 1.0e-6);
}

#[test]
fn a_zero_blend_snaps_and_an_unknown_state_changes_nothing() {
    let part = door();
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("map".into());
    assert!(states.set(key.clone(), &part, "closed", 0.0));
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!(time.abs() < 1.0e-6, "zero blend is instant");

    assert!(!states.set(key.clone(), &part, "ajar", 1.0), "no such state");
    let (state, time, _) = states.clock(&key, "map", &part);
    assert_eq!(state, 0);
    assert!(time.abs() < 1.0e-6);
}

/// The collision half: a closed door is a wall in the doorway, an open
/// one is not. Same boxes, moved by the same matrix the draw uses.
#[test]
fn collider_boxes_move_with_the_part() {
    let part = door();
    let local = part.collider_boxes();
    assert!(!local.is_empty());
    let mut instance = Mat4f::identity();
    instance.v[13] = 10.0; // the level itself is placed 10 up

    let world_at = |time: f32| {
        let m = Mat4f::mul(&instance, &part.transform_at(time));
        world_boxes(&m, &local)
    };
    let (closed, cmin, cmax) = world_at(part.state_time(0));
    let (open, omin, omax) = world_at(part.state_time(1));
    assert_eq!(closed.len(), local.len());
    // Closed: the slab fills the opening at the level's own height.
    assert!((cmin.y - 10.0).abs() < 1.0e-4, "{cmin:?}");
    assert!((cmax.y - 13.0).abs() < 1.0e-4, "{cmax:?}");
    // Open: the same slab has risen out of the way by the clip's travel.
    assert!((omin.y - 13.0).abs() < 1.0e-4, "{omin:?}");
    assert!((omax.y - 16.0).abs() < 1.0e-4, "{omax:?}");
    assert!(open.iter().zip(&closed).all(|(o, c)| {
        (o.0.y - c.0.y - 3.0).abs() < 1.0e-3 && (o.0.x - c.0.x).abs() < 1.0e-4
    }));
    // Half way is half way here too — a door caught moving is where it
    // looks, not snapped to an end state.
    let (_, hmin, _) = world_at(0.5);
    assert!((hmin.y - 11.5).abs() < 1.0e-3, "{hmin:?}");
}

#[test]
fn a_slot_command_wins_over_a_model_command() {
    let part = door();
    let mut states = ModelStates::default();
    states.set(ModelTarget::Model("map".into()), &part, "closed", 0.0);
    states.set(ModelTarget::Instance(2), &part, "open", 0.0);

    // Slot 2 asked for open; every other copy follows the model command.
    let (state, _, _) = states.clock(&ModelTarget::Instance(2), "map", &part);
    assert_eq!(state, 1);
    let (state, _, _) = states.clock(&ModelTarget::Instance(3), "map", &part);
    assert_eq!(state, 0, "an unaddressed slot follows the model command");
}

/// Slot numbers only mean anything against one placed list.
#[test]
fn a_changed_placed_scene_drops_slot_commands_but_keeps_model_ones() {
    let part = door();
    let mut renderer = Renderer::default();
    renderer.model_anim_state.set(ModelTarget::Model("map".into()), &part, "closed", 0.0);
    renderer.model_anim_state.set(ModelTarget::Instance(0), &part, "closed", 0.0);

    let mut transform = Mat4f::identity();
    transform.v[12] = 7.0;
    renderer.set_models(vec![ModelInstance {
        model: "map".to_string(),
        transform,
        tint: vec4(1.0, 1.0, 1.0, 1.0),
        color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
        dynamic: false,
        depth_order: 0.0,
        custom_material: None,
        part_poses: Vec::new(),
    }]);
    assert!(renderer
        .model_anim_state
        .map
        .keys()
        .all(|(t, _)| matches!(t, ModelTarget::Model(_))));
    assert_eq!(renderer.model_anim_state.map.len(), 1);
}

#[test]
fn model_instance_appearance_is_per_copy_and_neutral_by_default() {
    let bounds = (vec3f(-1.0, 0.0, -1.0), vec3f(1.0, 2.0, 1.0));
    let frame = Mat4f::identity();
    let neutral = ModelInstance::on_body("dog".into(), bounds, 1.0, 1.0, &frame);
    assert_eq!(neutral.tint, vec4(1.0, 1.0, 1.0, 1.0));
    assert_eq!(neutral.color_adjust, vec4(0.0, 1.0, 1.0, 0.0));

    let blue = neutral
        .clone()
        .with_tint(vec4(0.2, 0.5, 1.0, 1.0))
        .with_color_adjust(vec4(210.0, 1.1, 0.9, 0.0));
    assert_eq!(blue.tint, vec4(0.2, 0.5, 1.0, 1.0));
    assert_eq!(blue.color_adjust, vec4(210.0, 1.1, 0.9, 0.0));
    assert_eq!(neutral.tint, vec4(1.0, 1.0, 1.0, 1.0));
}

#[test]
fn authored_model_origin_and_front_follow_full_bank_pitch_frame() {
    let origin = vec3f(0.3, -0.7, 1.2);
    for angles in [vec3f(0.0,0.0,0.0), vec3f(0.31,1.1,-0.64)] {
        let mut frame = Mat4f::rotation(angles);
        frame.v[12] = 17.0; frame.v[13] = 28.0; frame.v[14] = -9.0;
        let instance = ModelInstance::on_body_authored("trainer".into(), 2.0, origin, &frame);
        for (model_point, body_point) in [
            (origin, vec3f(0.0,0.0,0.0)),
            (origin + vec3f(0.0,0.0,1.0),vec3f(0.0,0.0,-2.0)),
            (origin + vec3f(1.0,0.0,0.0),vec3f(-2.0,0.0,0.0)),
            (origin + vec3f(0.0,1.0,0.0),vec3f(0.0,2.0,0.0)),
        ] {
            let got = instance.transform.transform_vec4(vec4(model_point.x,model_point.y,model_point.z,1.0));
            let expected = frame.transform_vec4(vec4(body_point.x,body_point.y,body_point.z,1.0));
            assert!((got.x-expected.x).abs()<1e-5 && (got.y-expected.y).abs()<1e-5 && (got.z-expected.z).abs()<1e-5);
        }
    }
}

#[test]
fn model_targets_come_from_ids_and_slots() {
    assert_eq!(
        ModelTarget::from("maps/e1m1"),
        ModelTarget::Model("maps/e1m1".to_string())
    );
    assert_eq!(ModelTarget::from(4usize), ModelTarget::Instance(4));
}

/// An ordinary prop has no parts, and every query says so without
/// pretending the model is missing.
#[test]
fn a_model_without_parts_answers_empty() {
    let renderer = Renderer::default();
    assert!(renderer.model_anim_part_names("kit/lamp").is_empty());
    assert!(renderer.model_anim_part("kit/lamp", "door_1").is_none());
    assert!(renderer.anim_part_boxes().is_empty());
    assert!(renderer.model_states("kit/lamp").is_empty());
}
