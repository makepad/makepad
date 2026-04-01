#[test]
fn stick_axes_map_forward_and_turn_in_local_space() {
    let (forward, turn) = stick_deadzone_scaled_axes(vec2f(0.0, 1.0), 0.16);
    assert!((forward - 1.0).abs() < 0.0001);
    assert!(turn.abs() < 0.0001);
}

#[test]
fn stick_axes_turn_in_place_with_horizontal_input() {
    let (forward, turn) = stick_deadzone_scaled_axes(vec2f(1.0, 0.0), 0.16);
    assert!(forward.abs() < 0.0001);
    assert!((turn - 1.0).abs() < 0.0001);
}

#[test]
fn track_commands_turn_right_for_positive_turn_input() {
    let pose = Pose::default();
    let (left, right) =
        differential_track_commands(pose, vec3f(0.0, 0.0, 0.0), 0.0, 1.0, 1.35, 1.45);

    assert!(left < 0.0);
    assert!(right > 0.0);
}

#[test]
fn track_commands_reverse_when_forward_input_is_negative() {
    let pose = Pose::default();
    let (left, right) =
        differential_track_commands(pose, vec3f(0.0, 0.0, 0.0), -1.0, 0.0, 1.35, 1.45);

    assert!(left < 0.0);
    assert!(right < 0.0);
}
