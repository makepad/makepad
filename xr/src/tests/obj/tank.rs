#[test]
fn head_relative_drive_direction_uses_head_yaw() {
    let controller = XrController {
        buttons: XrController::ACTIVE,
        stick: vec2f(0.0, 1.0),
        ..XrController::default()
    };
    let yaw_right = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), -std::f32::consts::FRAC_PI_2);

    let (direction, amount) = stick_deadzone_scaled_direction(&controller, yaw_right, 0.16)
        .expect("active stick should produce a drive direction");

    assert!((direction - vec3f(1.0, 0.0, 0.0)).length() < 0.0001);
    assert!((amount - 1.0).abs() < 0.0001);
}

#[test]
fn track_commands_turn_right_for_target_on_right() {
    let pose = Pose::default();
    let desired_direction = vec3f(1.0, 0.0, 0.0);
    let (left, right) =
        differential_track_commands(pose, vec3f(0.0, 0.0, 0.0), desired_direction, 1.0, 1.35, 1.45);

    assert!(left < 0.0);
    assert!(right > 0.0);
}

#[test]
fn track_commands_reverse_when_target_is_behind_body() {
    let pose = Pose::default();
    let desired_direction = vec3f(0.0, 0.0, 1.0);
    let (left, right) =
        differential_track_commands(pose, vec3f(0.0, 0.0, 0.0), desired_direction, 1.0, 1.35, 1.45);

    assert!(left < 0.0);
    assert!(right < 0.0);
}
